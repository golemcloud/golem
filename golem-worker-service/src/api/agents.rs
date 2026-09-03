use crate::api::common::ApiEndpointError;
use crate::api::invocation_session::serve_public_invocation_session;
use crate::invocation_session_token::InvocationSessionTokenKeyring;
use crate::service::auth::AuthService;
use crate::service::worker::WorkerService;
use chrono::{DateTime, Utc};
use golem_common::base_model::api;
use golem_common::model::agent::AgentTypeName;
use golem_common::model::application::ApplicationName;
use golem_common::model::component::ComponentRevision;
use golem_common::model::environment::EnvironmentName;
use golem_common::model::invocation_session_public::{
    INVOCATION_SESSION_SUBPROTOCOL, PublicErrorCode,
};
use golem_common::model::worker::AgentConfigEntryDto;
use golem_common::model::{AgentId, IdempotencyKey};
use golem_common::recorded_http_api_request;
use golem_common::schema::{SchemaValue, TypedSchemaValue};
use golem_service_base::api_tags::ApiTags;
use golem_service_base::model::auth::GolemSecurityScheme;
use poem::web::websocket::{BoxWebSocketUpgraded, WebSocket, WebSocketConfig};
use poem::{Request, RequestBody};
use poem_openapi::param::Header;
use poem_openapi::payload::Json;
use poem_openapi::registry::{MetaParamIn, MetaSchemaRef, Registry};
use poem_openapi::types::Type;
use poem_openapi::{ApiExtractor, ApiExtractorType, ExtractParamOptions};
use poem_openapi_derive::{Enum, Object, OpenApi};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tracing::Instrument;
use uuid::Uuid;

type Result<T> = std::result::Result<T, ApiEndpointError>;

const INVOCATION_SESSION_MAX_MESSAGE_SIZE: usize = 32 * 1024 * 1024;

struct RequiredWebSocketSubprotocol(Option<String>);

impl<'a> ApiExtractor<'a> for RequiredWebSocketSubprotocol {
    const TYPES: &'static [ApiExtractorType] = &[ApiExtractorType::Parameter];
    const PARAM_IS_REQUIRED: bool = true;

    type ParamType = Option<String>;
    type ParamRawType = String;

    fn register(registry: &mut Registry) {
        <String as Type>::register(registry);
    }

    fn param_in() -> Option<MetaParamIn> {
        Some(MetaParamIn::Header)
    }

    fn param_schema_ref() -> Option<MetaSchemaRef> {
        Some(<String as Type>::schema_ref())
    }

    fn param_raw_type(&self) -> Option<&Self::ParamRawType> {
        self.0.as_ref()
    }

    async fn from_request(
        request: &'a Request,
        body: &mut RequestBody,
        param_opts: ExtractParamOptions<Self::ParamType>,
    ) -> poem::Result<Self> {
        Header::<Option<String>>::from_request(request, body, param_opts)
            .await
            .map(|header| Self(header.0))
    }
}

pub struct AgentsApi {
    worker_service: Arc<WorkerService>,
    auth_service: Arc<dyn AuthService>,
    invocation_session_token_keyring: Arc<InvocationSessionTokenKeyring>,
}

#[OpenApi(prefix_path = "/v1/agents", tag = ApiTags::Agent)]
impl AgentsApi {
    pub fn new(
        worker_service: Arc<WorkerService>,
        auth_service: Arc<dyn AuthService>,
        invocation_session_token_keyring: Arc<InvocationSessionTokenKeyring>,
    ) -> Self {
        Self {
            worker_service,
            auth_service,
            invocation_session_token_keyring,
        }
    }

    #[oai(path = "/invoke-agent", method = "post", operation_id = "invoke_agent")]
    async fn invoke_agent(
        &self,
        mut request: Json<AgentInvocationRequest>,
        #[oai(name = "Idempotency-Key")] idempotency_key: Header<Option<IdempotencyKey>>,
        token: GolemSecurityScheme,
    ) -> Result<Json<AgentInvocationResult>> {
        let auth = self.auth_service.authenticate_token(token.secret()).await?;

        if request.idempotency_key.is_none() {
            request.idempotency_key = idempotency_key.0;
        }

        if let Some(ref mut email) = request.owner_account_email {
            let trimmed = email.trim().to_string();
            if trimmed.is_empty() {
                return Err(ApiEndpointError::bad_request(
                    api::error_code::VALIDATION_ERROR,
                    golem_common::safe("owner_account_email cannot be empty".to_string()),
                ));
            }
            *email = trimmed;
        }

        let record = recorded_http_api_request!(
            "invoke_agent",
            app = %request.app_name,
            env = %request.env_name,
            agent_type = %request.agent_type_name,
            idempotency_key = request.idempotency_key.as_ref().as_ref().map(|v| v.value.clone()),
            method = %request.method_name
        );

        let response = self
            .worker_service
            .invoke_agent_rest(request.0, auth)
            .instrument(record.span.clone())
            .await
            .map_err(Into::into);

        record.result(response).map(Json)
    }

    /// Invoke an agent through an attached live streaming session
    ///
    /// Upgrades to a WebSocket using the required `golem.agent-invocation.v1`
    /// subprotocol. Text frames carry public v1 JSON lifecycle messages, while
    /// binary frames carry the public v1 binary envelope. The bearer token is
    /// authenticated before the upgrade and authorizes both start and resume.
    #[oai(
        path = "/invoke-agent-session",
        method = "get",
        operation_id = "invoke_agent_session"
    )]
    async fn invoke_agent_session(
        &self,
        websocket: WebSocket,
        #[oai(name = "Sec-WebSocket-Protocol")] subprotocols: RequiredWebSocketSubprotocol,
        token: GolemSecurityScheme,
    ) -> Result<BoxWebSocketUpgraded> {
        let supports_v1 = subprotocols.0.as_deref().is_some_and(|values| {
            values
                .split(',')
                .any(|value| value.trim() == INVOCATION_SESSION_SUBPROTOCOL)
        });
        if !supports_v1 {
            return Err(ApiEndpointError::bad_request(
                PublicErrorCode::UnsupportedSubprotocol.as_str(),
                golem_common::safe("unsupported WebSocket subprotocol".to_string()),
            ));
        }
        let auth = self.auth_service.authenticate_token(token.secret()).await?;
        let worker_service = self.worker_service.clone();
        let keyring = self.invocation_session_token_keyring.clone();

        Ok(websocket
            .protocols([INVOCATION_SESSION_SUBPROTOCOL])
            .config(invocation_session_websocket_config())
            .on_upgrade(Box::new(move |socket| {
                Box::pin(serve_public_invocation_session(
                    socket,
                    worker_service,
                    keyring,
                    auth,
                ))
            })))
    }

    #[oai(path = "/create-agent", method = "post", operation_id = "create_agent")]
    async fn create_agent(
        &self,
        request: Json<CreateAgentRequest>,
        token: GolemSecurityScheme,
    ) -> Result<Json<CreateAgentResponse>> {
        let auth = self.auth_service.authenticate_token(token.secret()).await?;

        let record = recorded_http_api_request!(
            "create_agent",
            app = %request.app_name,
            env = %request.env_name,
            agent_type = %request.agent_type_name,
        );

        let response = self
            .worker_service
            .create_agent_rest(request.0, auth)
            .instrument(record.span.clone())
            .await
            .map_err(Into::into);

        record.result(response).map(Json)
    }
}

fn invocation_session_websocket_config() -> WebSocketConfig {
    WebSocketConfig::default()
        .max_message_size(Some(INVOCATION_SESSION_MAX_MESSAGE_SIZE))
        .max_frame_size(Some(INVOCATION_SESSION_MAX_MESSAGE_SIZE))
        .max_write_buffer_size(2 * INVOCATION_SESSION_MAX_MESSAGE_SIZE)
}

#[derive(Debug, Clone, Serialize, Deserialize, Enum)]
#[oai(rename_all = "camelCase")]
#[serde(rename_all = "camelCase")]
pub enum AgentInvocationMode {
    Await,
    Schedule,
}

#[derive(Debug, Clone, Serialize, Deserialize, Object)]
#[oai(rename_all = "camelCase")]
#[serde(rename_all = "camelCase")]
pub struct AgentInvocationRequest {
    pub app_name: ApplicationName,
    pub env_name: EnvironmentName,
    pub agent_type_name: AgentTypeName,
    pub parameters: SchemaValue,
    pub phantom_id: Option<Uuid>,
    #[oai(default)]
    #[serde(default)]
    pub config: Vec<AgentConfigEntryDto>,
    pub method_name: String,
    pub method_parameters: SchemaValue,
    pub mode: AgentInvocationMode,
    pub schedule_at: Option<DateTime<Utc>>,
    pub idempotency_key: Option<IdempotencyKey>,
    pub deployment_revision: Option<i64>,
    pub owner_account_email: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Object)]
#[oai(rename_all = "camelCase")]
#[serde(rename_all = "camelCase")]
pub struct AgentInvocationResult {
    pub agent_id: AgentId,
    pub idempotency_key: IdempotencyKey,
    pub result: Option<TypedSchemaValue>,
    pub component_revision: Option<ComponentRevision>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Object)]
#[oai(rename_all = "camelCase")]
#[serde(rename_all = "camelCase")]
pub struct CreateAgentRequest {
    pub app_name: ApplicationName,
    pub env_name: EnvironmentName,
    pub agent_type_name: AgentTypeName,
    pub parameters: SchemaValue,
    pub phantom_id: Option<Uuid>,
    #[oai(default)]
    #[serde(default)]
    pub config: Vec<AgentConfigEntryDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Object)]
#[oai(rename_all = "camelCase")]
#[serde(rename_all = "camelCase")]
pub struct CreateAgentResponse {
    pub agent_id: AgentId,
    pub component_revision: ComponentRevision,
}

#[cfg(test)]
mod tests {
    use super::{
        AgentInvocationRequest, CreateAgentRequest, INVOCATION_SESSION_MAX_MESSAGE_SIZE,
        invocation_session_websocket_config,
    };
    use poem_openapi::types::{ParseFromJSON, ToJSON};
    use serde_json::{Value, json};
    use test_r::test;

    fn empty_parameter_record() -> Value {
        json!({ "kind": "record", "value": { "fields": [] } })
    }

    #[test]
    fn create_agent_request_preserves_canonical_schema_value_json() {
        let parameters = empty_parameter_record();
        let config_value = json!({ "arbitrary": [1, true, null] });
        let request_json = json!({
            "appName": "app",
            "envName": "env",
            "agentTypeName": "agent",
            "parameters": parameters,
            "config": [{
                "path": ["nested"],
                "value": config_value,
            }],
        });

        let request = CreateAgentRequest::parse_from_json(Some(request_json))
            .expect("canonical SchemaValue JSON must remain accepted");
        let encoded = request.to_json().expect("request must serialize to JSON");

        assert_eq!(encoded["parameters"], empty_parameter_record());
        assert_eq!(encoded["config"][0]["value"], config_value);
    }

    #[test]
    fn invoke_agent_request_preserves_canonical_schema_value_json() {
        let request_json = json!({
            "appName": "app",
            "envName": "env",
            "agentTypeName": "agent",
            "parameters": empty_parameter_record(),
            "methodName": "run",
            "methodParameters": empty_parameter_record(),
            "mode": "await",
        });

        let request = AgentInvocationRequest::parse_from_json(Some(request_json))
            .expect("canonical SchemaValue JSON must remain accepted");
        let encoded = request.to_json().expect("request must serialize to JSON");

        assert_eq!(encoded["parameters"], empty_parameter_record());
        assert_eq!(encoded["methodParameters"], empty_parameter_record());
    }

    #[test]
    fn agent_request_rejects_noncanonical_parameter_json() {
        let request_json = json!({
            "appName": "app",
            "envName": "env",
            "agentTypeName": "agent",
            "parameters": [],
        });

        assert!(CreateAgentRequest::parse_from_json(Some(request_json)).is_err());
    }

    #[test]
    fn invocation_session_websocket_config_bounds_frames_messages_and_writes() {
        let config = invocation_session_websocket_config();

        assert_eq!(
            config.max_message_size,
            Some(INVOCATION_SESSION_MAX_MESSAGE_SIZE)
        );
        assert_eq!(
            config.max_frame_size,
            Some(INVOCATION_SESSION_MAX_MESSAGE_SIZE)
        );
        assert_eq!(
            config.max_write_buffer_size,
            2 * INVOCATION_SESSION_MAX_MESSAGE_SIZE
        );
    }
}
