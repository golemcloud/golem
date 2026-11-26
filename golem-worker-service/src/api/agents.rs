use crate::api::common::ApiEndpointError;
use crate::api::invocation::InvocationFailure;
use crate::api::invocation_session::serve_public_invocation_session;
use crate::invocation_session_token::InvocationSessionTokenKeyring;
use crate::metrics::{ActiveHttpApiStream, HttpApiStreamKind};
use crate::model::WorkersMetadataResponse;
use crate::service::auth::AuthService;
use crate::service::worker::WorkerService;
use chrono::{DateTime, Utc};
use golem_common::base_model::api;
use golem_common::model::agent::AgentConfigEntryDto;
use golem_common::model::agent::AgentTypeName;
use golem_common::model::component::ComponentId;
use golem_common::model::component::ComponentRevision;
use golem_common::model::environment::EnvironmentId;
use golem_common::model::invocation_session_public::{
    INVOCATION_SESSION_SUBPROTOCOL, PublicErrorCode,
};
use golem_common::model::oplog::OplogIndex;
use golem_common::model::tool::SerializableToolRpcError;
use golem_common::model::{
    AgentFilter, AgentFingerprint, AgentId, IdempotencyKey, InvocationId, OwnedInvocationId,
    ScanCursor,
};
use golem_common::recorded_http_api_request;
use golem_common::schema::tool::Tool;
use golem_common::schema::{ExternalSchemaValue, ExternalTypedSchemaValue};
use golem_service_base::api_tags::ApiTags;
use golem_service_base::model::WorkersMetadataRequest;
use golem_service_base::model::auth::surface::Surface;
use golem_service_base::model::auth::{AuthCtx, GolemSecurityScheme};
use poem::web::websocket::{BoxWebSocketUpgraded, WebSocket};
use poem::{Request, RequestBody};
use poem_openapi::param::{Header, Path, Query};
use poem_openapi::payload::Json;
use poem_openapi::registry::{MetaParamIn, MetaSchemaRef, Registry};
use poem_openapi::types::Type;
use poem_openapi::{ApiExtractor, ApiExtractorType, ExtractParamOptions};
use poem_openapi_derive::{ApiResponse, Enum, Object, OpenApi, Union};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tracing::Instrument;

type Result<T> = std::result::Result<T, ApiEndpointError>;

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
    invocation_session_config: crate::config::InvocationSessionConfig,
}

#[OpenApi(prefix_path = "/v1/runtime/envs", tag = ApiTags::Agent)]
impl AgentsApi {
    pub fn new(
        worker_service: Arc<WorkerService>,
        auth_service: Arc<dyn AuthService>,
        invocation_session_token_keyring: Arc<InvocationSessionTokenKeyring>,
        invocation_session_config: crate::config::InvocationSessionConfig,
    ) -> Self {
        Self {
            worker_service,
            auth_service,
            invocation_session_token_keyring,
            invocation_session_config,
        }
    }

    /// Create an agent
    ///
    /// Creates an agent of the agent type from its constructor parameters. A keyed create makes
    /// the keyed agent of the parameters and fails with 409 when that key is current. A create
    /// without a key makes a new agent each time; with an `Idempotency-Key` from a caller with a
    /// caller scope, a retry returns the agent of the first attempt, and the same key with a
    /// changed request fails with 422.
    #[oai(
        path = "/:environment_id/agent-types/:agent_type_name/agents",
        method = "post",
        operation_id = "create_agent"
    )]
    async fn create_agent(
        &self,
        environment_id: Path<EnvironmentId>,
        agent_type_name: Path<AgentTypeName>,
        request: Json<CreateAgentRequest>,
        #[oai(name = "Idempotency-Key")] idempotency_key: Header<Option<IdempotencyKey>>,
        token: GolemSecurityScheme,
    ) -> Result<CreateAgentApiResponse> {
        let auth = self.auth_service.authenticate_token(token.secret()).await?;
        let record = recorded_http_api_request!(
            "create_agent",
            environment_id = %environment_id.0,
            agent_type = %agent_type_name.0,
            idempotency_key = idempotency_key.0.as_ref().map(|key| key.value.clone())
        );

        let response = self
            .worker_service
            .create_agent_rest(
                environment_id.0,
                &agent_type_name.0,
                request.0,
                idempotency_key.0,
                auth,
                Surface::Rest,
            )
            .instrument(record.span.clone())
            .await
            .map_err(Into::into);

        record
            .result(response)
            .map(|response| CreateAgentApiResponse::Created(Json(response)))
    }

    /// Upsert an agent
    ///
    /// Returns the keyed agent of the constructor parameters, and creates it when it is missing.
    #[oai(
        path = "/:environment_id/agent-types/:agent_type_name/agents/upsert",
        method = "post",
        operation_id = "upsert_agent"
    )]
    async fn upsert_agent(
        &self,
        environment_id: Path<EnvironmentId>,
        agent_type_name: Path<AgentTypeName>,
        request: Json<AgentKeyRequest>,
        token: GolemSecurityScheme,
    ) -> Result<Json<AgentReference>> {
        let auth = self.auth_service.authenticate_token(token.secret()).await?;
        let record = recorded_http_api_request!(
            "upsert_agent",
            environment_id = %environment_id.0,
            agent_type = %agent_type_name.0,
        );

        let response = self
            .worker_service
            .upsert_agent_rest(
                environment_id.0,
                &agent_type_name.0,
                request.0,
                auth,
                Surface::Rest,
            )
            .instrument(record.span.clone())
            .await
            .map_err(Into::into);

        record.result(response).map(Json)
    }

    /// Look up an agent by its key
    ///
    /// Returns the keyed agent of the constructor parameters, or 404 when it does not exist.
    /// It never creates the agent.
    #[oai(
        path = "/:environment_id/agent-types/:agent_type_name/agents/lookup",
        method = "post",
        operation_id = "lookup_agent"
    )]
    async fn lookup_agent(
        &self,
        environment_id: Path<EnvironmentId>,
        agent_type_name: Path<AgentTypeName>,
        request: Json<AgentKeyRequest>,
        token: GolemSecurityScheme,
    ) -> Result<Json<AgentReference>> {
        let auth = self.auth_service.authenticate_token(token.secret()).await?;
        let record = recorded_http_api_request!(
            "lookup_agent",
            environment_id = %environment_id.0,
            agent_type = %agent_type_name.0,
        );

        let response = self
            .worker_service
            .lookup_agent_rest(
                environment_id.0,
                &agent_type_name.0,
                request.0,
                auth,
                Surface::Rest,
            )
            .instrument(record.span.clone())
            .await
            .map_err(Into::into);

        record.result(response).map(Json)
    }

    /// List the agents of an agent type
    ///
    /// ### Filters
    ///
    /// | Property    | Comparator             | Description                   | Example                            |
    /// |-------------|------------------------|-------------------------------|------------------------------------|
    /// | key         | StringFilterComparator | Canonical key of the agent    | `key == Counter("a")`              |
    /// | id          | FilterComparator       | Full UUID of the agent        | `id == 00000007-0000-8000-8000-000000000001` |
    /// | version     | FilterComparator       | Component revision            | `version >= 0`                     |
    /// | status      | FilterComparator       | Status of the agent           | `status = Running`                 |
    /// | env.\[key\] | StringFilterComparator | Environment variable          | `env.var1 = value`                 |
    /// | createdAt   | FilterComparator       | Creation time of the agent    | `createdAt > 2024-04-01T12:10:00Z` |
    ///
    /// ### Comparators
    ///
    /// - StringFilterComparator: `eq|equal|=|==`, `ne|notequal|!=`, `like`, `notlike`
    /// - FilterComparator: `eq|equal|=|==`, `ne|notequal|!=`, `ge|greaterequal|>=`, `gt|greater|>`, `le|lessequal|<=`, `lt|less|<`
    ///
    /// Returns the metadata of the agents, and a cursor for the next page when there are more.
    #[oai(
        path = "/:environment_id/agent-types/:agent_type_name/agents",
        method = "get",
        operation_id = "get_agents_metadata"
    )]
    #[allow(clippy::too_many_arguments)]
    async fn get_agents_metadata(
        &self,
        environment_id: Path<EnvironmentId>,
        agent_type_name: Path<AgentTypeName>,
        /// Restrict results to agents whose recorded component has this name, not the type's current implementer.
        component_name: Query<Option<String>>,
        /// Filter in the form `property op value`. When it is given more than once, every filter must match.
        filter: Query<Option<Vec<String>>>,
        /// Position where to start listing. To get the next page, pass the cursor of the previous response.
        cursor: Query<Option<String>>,
        /// Count of listed values, default: 50
        count: Query<Option<u64>>,
        /// If true, calculate the most up-to-date status for each agent, default is false
        precise: Query<Option<bool>>,
        token: GolemSecurityScheme,
    ) -> Result<Json<WorkersMetadataResponse>> {
        let auth = self.auth_service.authenticate_token(token.secret()).await?;
        let record = recorded_http_api_request!(
            "get_agents_metadata",
            environment_id = %environment_id.0,
            agent_type = %agent_type_name.0,
        );

        let response = async {
            let filter = match filter.0 {
                Some(filters) if !filters.is_empty() => {
                    Some(AgentFilter::from(filters).map_err(|e| {
                        ApiEndpointError::bad_request(
                            api::error_code::INVALID_AGENT_FILTER,
                            golem_common::safe(e),
                        )
                    })?)
                }
                _ => None,
            };
            self.find_agents(
                environment_id.0,
                &agent_type_name.0,
                component_name.0.as_deref(),
                filter,
                cursor.0.map(ScanCursor::from),
                count.0,
                precise.0,
                auth,
            )
            .await
        }
        .instrument(record.span.clone())
        .await;

        record.result(response)
    }

    /// Find the agents of an agent type
    ///
    /// ### Filter types
    /// | Type      | Comparator             | Description                  | Example                                                                             |
    /// |-----------|------------------------|------------------------------|-------------------------------------------------------------------------------------|
    /// | Key       | StringFilterComparator | Canonical key of the agent   | `{ "type": "Key", "comparator": "Equal", "value": "Counter(\"a\")" }`              |
    /// | Id        | FilterComparator       | Full UUID of the agent       | `{ "type": "Id", "comparator": "Equal", "value": "00000007-0000-8000-8000-000000000001" }` |
    /// | Version   | FilterComparator       | Component revision           | `{ "type": "Version", "comparator": "GreaterEqual", "value": 0 }`                  |
    /// | Status    | FilterComparator       | Status of the agent          | `{ "type": "Status", "comparator": "Equal", "value": "Running" }`                  |
    /// | Env       | StringFilterComparator | Environment variable         | `{ "type": "Env", "name": "var1", "comparator": "Equal", "value": "value" }`       |
    /// | CreatedAt | FilterComparator       | Creation time of the agent   | `{ "type": "CreatedAt", "comparator": "Greater", "value": "2024-04-01T12:10:00Z" }`|
    /// | And       |                        | And filter combinator        | `{ "type": "And", "filters": [ ... ] }`                                            |
    /// | Or        |                        | Or filter combinator         | `{ "type": "Or", "filters": [ ... ] }`                                             |
    /// | Not       |                        | Negates the specified filter | `{ "type": "Not", "filter": { ... } }`                                             |
    ///
    /// ### Comparators
    /// - StringFilterComparator: `Equal`, `NotEqual`, `Like`, `NotLike`
    /// - FilterComparator: `Equal`, `NotEqual`, `GreaterEqual`, `Greater`, `LessEqual`, `Less`
    #[oai(
        path = "/:environment_id/agent-types/:agent_type_name/agents/find",
        method = "post",
        operation_id = "find_agents_metadata"
    )]
    async fn find_agents_metadata(
        &self,
        environment_id: Path<EnvironmentId>,
        agent_type_name: Path<AgentTypeName>,
        /// Restrict results to agents whose recorded component has this name, not the type's current implementer.
        component_name: Query<Option<String>>,
        params: Json<WorkersMetadataRequest>,
        token: GolemSecurityScheme,
    ) -> Result<Json<WorkersMetadataResponse>> {
        let auth = self.auth_service.authenticate_token(token.secret()).await?;
        let record = recorded_http_api_request!(
            "find_agents_metadata",
            environment_id = %environment_id.0,
            agent_type = %agent_type_name.0,
        );

        let response = async {
            let params = params.0;
            let filter = match params.filter {
                // `serde_json` refuses JSON nested deeper than 128, which bounds the recursion
                // of `validated` before the depth check.
                Some(filter) => Some(filter.validated().map_err(|e| {
                    ApiEndpointError::bad_request(
                        api::error_code::INVALID_AGENT_FILTER,
                        golem_common::safe(e),
                    )
                })?),
                None => None,
            };
            self.find_agents(
                environment_id.0,
                &agent_type_name.0,
                component_name.0.as_deref(),
                filter,
                params.cursor,
                params.count,
                params.precise,
                auth,
            )
            .await
        }
        .instrument(record.span.clone())
        .await;

        record.result(response)
    }

    /// Invoke an agent method
    ///
    /// Names the agent by exactly one of `agentId` or `parameters`. With `parameters`, a durable
    /// agent is upserted, and an ephemeral invocation runs on an instance of its own. With
    /// `expectedFingerprint`, a missing agent or another incarnation fails with 404 before
    /// anything runs. The idempotency key comes from the `Idempotency-Key` header.
    #[oai(
        path = "/:environment_id/agent-types/:agent_type_name/invoke",
        method = "post",
        operation_id = "invoke_agent"
    )]
    async fn invoke_agent(
        &self,
        environment_id: Path<EnvironmentId>,
        agent_type_name: Path<AgentTypeName>,
        request: Json<AgentInvocationRequest>,
        #[oai(name = "Idempotency-Key")] idempotency_key: Header<Option<IdempotencyKey>>,
        token: GolemSecurityScheme,
    ) -> Result<InvokeAgentResponse> {
        let auth = self.auth_service.authenticate_token(token.secret()).await?;
        let record = recorded_http_api_request!(
            "invoke_agent",
            environment_id = %environment_id.0,
            agent_type = %agent_type_name.0,
            idempotency_key = idempotency_key.0.as_ref().map(|key| key.value.clone()),
            method = %request.method
        );

        let response = self
            .worker_service
            .invoke_agent_rest(
                environment_id.0,
                &agent_type_name.0,
                request.0,
                idempotency_key.0,
                auth,
                Surface::Rest,
            )
            .instrument(record.span.clone())
            .await
            .map_err(Into::into);

        record.result(response).map(|result| {
            let invocation_id = invocation_id_header(&result.agent_id, result.idempotency_key);
            InvokeAgentResponse::Ok(Json(result), invocation_id, environment_id.0.to_string())
        })
    }

    /// Invoke a tool on an existing agent or a fresh ephemeral component owner
    ///
    /// Uses normal invocation permissions, queueing and idempotency. Byte stdin, stdout and stderr
    /// require an attached invocation session rather than this scalar endpoint.
    #[oai(
        path = "/:environment_id/invoke-tool",
        method = "post",
        operation_id = "invoke_tool"
    )]
    async fn invoke_tool(
        &self,
        environment_id: Path<EnvironmentId>,
        request: Json<NativeToolInvocationRequest>,
        #[oai(name = "Idempotency-Key")] idempotency_key: Header<Option<IdempotencyKey>>,
        token: GolemSecurityScheme,
    ) -> Result<InvokeToolResponse> {
        let auth = self.auth_service.authenticate_token(token.secret()).await?;
        let record = recorded_http_api_request!(
            "invoke_tool",
            environment_id = %environment_id.0,
            tool_name = %request.tool_name,
            idempotency_key = idempotency_key.0.as_ref().map(|key| key.value.clone())
        );
        let accepting = !matches!(request.mode, NativeToolInvocationMode::Lookup);
        let response = self
            .worker_service
            .invoke_tool_rest(
                environment_id.0,
                request.0,
                idempotency_key.0,
                auth,
                Surface::Rest,
            )
            .instrument(record.span.clone())
            .await
            .map_err(Into::into);
        record.result(response).map(|result| {
            let (invocation_id, environment_id) = if accepting {
                (
                    Some(invocation_id_header(
                        &result.agent_id,
                        result.idempotency_key,
                    )),
                    Some(environment_id.0.to_string()),
                )
            } else {
                (None, None)
            };
            InvokeToolResponse::Ok(Json(result), invocation_id, environment_id)
        })
    }

    /// Return the effective definition of a tool bound to an existing agent or component baseline.
    #[oai(
        path = "/:environment_id/describe-tool",
        method = "post",
        operation_id = "describe_tool"
    )]
    async fn describe_tool(
        &self,
        environment_id: Path<EnvironmentId>,
        request: Json<NativeToolDescribeRequest>,
        token: GolemSecurityScheme,
    ) -> Result<Json<NativeToolDefinition>> {
        let auth = self.auth_service.authenticate_token(token.secret()).await?;
        self.worker_service
            .describe_tool_rest(environment_id.0, request.0, auth, Surface::Rest)
            .await
            .map(Json)
            .map_err(Into::into)
    }

    /// Invoke an agent through an attached live streaming session
    ///
    /// Upgrades to a WebSocket using the required `golem.agent-invocation.v1`
    /// subprotocol. Text frames carry public v1 JSON lifecycle messages, while
    /// binary frames carry the public v1 binary envelope. The bearer token is
    /// authenticated before the upgrade and authorizes both start and resume.
    #[oai(
        path = "/:environment_id/invoke-agent-session",
        method = "get",
        operation_id = "invoke_agent_session"
    )]
    async fn invoke_agent_session(
        &self,
        environment_id: Path<EnvironmentId>,
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
        let config = self.invocation_session_config.clone();
        let environment_id = environment_id.0;

        Ok(websocket
            .protocols([INVOCATION_SESSION_SUBPROTOCOL])
            .config(self.invocation_session_config.websocket.websocket_config())
            .on_upgrade(Box::new(move |socket| {
                Box::pin(async move {
                    let _active = ActiveHttpApiStream::open(HttpApiStreamKind::InvocationSession);
                    serve_public_invocation_session(
                        socket,
                        worker_service,
                        keyring,
                        auth,
                        config,
                        environment_id,
                    )
                    .await
                })
            })))
    }
}

impl AgentsApi {
    #[allow(clippy::too_many_arguments)]
    async fn find_agents(
        &self,
        environment_id: EnvironmentId,
        agent_type_name: &AgentTypeName,
        component_name: Option<&str>,
        filter: Option<AgentFilter>,
        cursor: Option<ScanCursor>,
        count: Option<u64>,
        precise: Option<bool>,
        auth: AuthCtx,
    ) -> Result<Json<WorkersMetadataResponse>> {
        let (cursor, mut workers) = self
            .worker_service
            .find_metadata(
                environment_id,
                agent_type_name,
                component_name,
                filter,
                cursor.unwrap_or_default(),
                count.unwrap_or(50),
                precise.unwrap_or(false),
                auth,
                Surface::Rest,
            )
            .await?;
        for worker in &mut workers {
            worker.redact_host_managed_values_for_external();
        }
        Ok(Json(WorkersMetadataResponse { workers, cursor }))
    }
}

#[derive(ApiResponse, Debug)]
pub enum CreateAgentApiResponse {
    #[oai(status = 201)]
    Created(Json<AgentReference>),
}

/// The ID of an accepted invocation, as in the REST paths of invocations.
fn invocation_id_header(agent_id: &AgentId, invocation_id: InvocationId) -> String {
    OwnedInvocationId::new(*agent_id, invocation_id).to_string()
}

#[derive(ApiResponse)]
pub enum InvokeAgentResponse {
    /// The invocation was accepted. `Golem-Invocation-Id` is its invocation ID, as in the REST
    /// paths of invocations, and `Golem-Environment-Id` the environment in which the ID
    /// resolves. An awaited invocation that is accepted and then fails answers an error, which
    /// carries neither header.
    #[oai(status = 200)]
    Ok(
        Json<AgentInvocationResult>,
        #[oai(header = "Golem-Invocation-Id")] String,
        #[oai(header = "Golem-Environment-Id")] String,
    ),
}

#[derive(ApiResponse)]
pub enum InvokeToolResponse {
    /// An accepted invocation carries `Golem-Invocation-Id`, its invocation ID as in the REST
    /// paths of invocations, and `Golem-Environment-Id`, the environment in which the ID
    /// resolves. A lookup carries neither. The ID of a standalone tool invocation, which runs on
    /// a fresh owner, cannot be read through the invocation endpoints.
    #[oai(status = 200)]
    Ok(
        Json<NativeToolInvocationResponse>,
        #[oai(header = "Golem-Invocation-Id")] Option<String>,
        #[oai(header = "Golem-Environment-Id")] Option<String>,
    ),
}

#[derive(Debug, Clone, Serialize, Deserialize, Enum)]
#[oai(rename_all = "camelCase")]
#[serde(rename_all = "camelCase")]
pub enum AgentInvocationMode {
    Await,
    Schedule,
}

/// Names the agent by exactly one of `agentId` or `parameters`.
#[derive(Debug, Clone, Serialize, Deserialize, Object)]
#[oai(rename_all = "camelCase")]
#[serde(rename_all = "camelCase")]
pub struct AgentInvocationRequest {
    pub agent_id: Option<AgentId>,
    pub parameters: Option<ExternalSchemaValue>,
    pub method: String,
    pub method_parameters: ExternalSchemaValue,
    pub mode: AgentInvocationMode,
    pub schedule_at: Option<DateTime<Utc>>,
    pub expected_fingerprint: Option<AgentFingerprint>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Object)]
#[oai(rename_all = "camelCase")]
#[serde(rename_all = "camelCase")]
pub struct AgentInvocationResult {
    pub agent_id: AgentId,
    pub idempotency_key: InvocationId,
    pub result: Option<ExternalTypedSchemaValue>,
    pub component_revision: Option<ComponentRevision>,
    // Present when the invocation recorded `result`, then the agent failed during the
    // invocation's background work. A doc comment would make the schema an `allOf`, which the
    // client generator does not support.
    pub background_failure: Option<InvocationFailure>,
}

/// Select exactly one target: `agentId` for an existing real owner, or `componentId`
/// for a fresh ephemeral owner using that deployed component's baseline without constructing an agent.
/// Input is required for await and schedule, and forbidden for lookup.
#[derive(Debug, Clone, Serialize, Deserialize, Object)]
#[oai(rename_all = "camelCase")]
#[serde(rename_all = "camelCase")]
pub struct NativeToolInvocationRequest {
    pub agent_id: Option<AgentId>,
    pub component_id: Option<ComponentId>,
    pub tool_name: String,
    pub command_path: Vec<String>,
    pub input: Option<ExternalTypedSchemaValue>,
    pub mode: NativeToolInvocationMode,
    pub schedule_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Object)]
#[oai(rename_all = "camelCase")]
#[serde(rename_all = "camelCase")]
pub struct NativeToolDescribeRequest {
    pub agent_id: Option<AgentId>,
    pub component_id: Option<ComponentId>,
    pub tool_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Object)]
#[oai(rename_all = "camelCase")]
#[serde(rename_all = "camelCase")]
pub struct NativeToolDefinition {
    pub definition: Tool,
    pub component_revision: ComponentRevision,
    pub deployment_revision: golem_common::model::deployment::DeploymentRevision,
}

#[derive(Debug, Clone, Serialize, Deserialize, Enum)]
#[oai(rename_all = "camelCase")]
#[serde(rename_all = "camelCase")]
pub enum NativeToolInvocationMode {
    Await,
    Schedule,
    Lookup,
}

#[derive(Debug, Clone, Serialize, Deserialize, Object)]
#[oai(rename_all = "camelCase")]
#[serde(rename_all = "camelCase")]
pub struct NativeToolSuccess {
    pub result: Option<Box<ExternalTypedSchemaValue>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Object)]
pub struct NativeToolFailure {
    pub error: SerializableToolRpcError,
}

#[derive(Debug, Clone, Serialize, Deserialize, Union)]
#[oai(discriminator_name = "type", one_of = true)]
#[serde(tag = "type")]
pub enum NativeToolResult {
    Success(NativeToolSuccess),
    Failure(NativeToolFailure),
}

#[derive(Debug, Clone, Serialize, Deserialize, Object)]
#[oai(rename_all = "camelCase")]
#[serde(rename_all = "camelCase")]
pub struct NativeToolInvocationResponse {
    pub agent_id: AgentId,
    pub idempotency_key: InvocationId,
    pub result: Option<NativeToolResult>,
    pub status: Option<golem_common::model::InvocationStatus>,
    pub component_revision: Option<ComponentRevision>,
    pub agent_fingerprint: Option<AgentFingerprint>,
    pub oplog_index: Option<OplogIndex>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Object)]
#[oai(rename_all = "camelCase")]
#[serde(rename_all = "camelCase")]
pub struct CreateAgentRequest {
    pub parameters: ExternalSchemaValue,
    #[oai(default)]
    #[serde(default)]
    pub config: Vec<AgentConfigEntryDto>,
    #[oai(default)]
    #[serde(default)]
    pub env: HashMap<String, String>,
    /// Creates the keyed agent of the parameters, which fails when the key is current.
    #[oai(default)]
    #[serde(default)]
    pub keyed: bool,
}

/// The constructor parameters of a keyed agent.
#[derive(Debug, Clone, Serialize, Deserialize, Object)]
#[oai(rename_all = "camelCase")]
#[serde(rename_all = "camelCase")]
pub struct AgentKeyRequest {
    pub parameters: ExternalSchemaValue,
}

#[derive(Debug, Clone, Serialize, Deserialize, Object)]
#[oai(rename_all = "camelCase")]
#[serde(rename_all = "camelCase")]
pub struct AgentReference {
    pub agent_id: AgentId,
    pub agent_fingerprint: AgentFingerprint,
}

#[cfg(test)]
mod websocket_limit_tests;

#[cfg(test)]
mod tests {
    use super::{AgentInvocationRequest, CreateAgentRequest};
    use chrono::{TimeZone, Utc};
    use golem_common::schema::{SchemaValue, SecretValuePayload};
    use poem_openapi::types::{ParseFromJSON, ToJSON};
    use serde_json::{Value, json};
    use test_r::test;
    use uuid::Uuid;

    fn empty_parameter_record() -> Value {
        json!({ "kind": "record", "value": { "fields": [] } })
    }

    #[test]
    fn native_tool_failure_preserves_error_discriminator_in_rest_json() {
        use super::{NativeToolFailure, NativeToolResult};
        use golem_common::model::tool::SerializableToolRpcError;

        for error in [
            SerializableToolRpcError::Denied("not allowed".to_string()),
            SerializableToolRpcError::Cancelled,
        ] {
            let expected = serde_json::to_value(&error).unwrap();
            let result = NativeToolResult::Failure(NativeToolFailure { error });
            let wire = result.to_json().unwrap();
            assert_eq!(wire["type"], "Failure");
            assert_eq!(wire["error"], expected);
            let decoded: NativeToolResult = serde_json::from_value(wire.clone()).unwrap();
            assert_eq!(decoded.to_json(), Some(wire.clone()));
            assert!(NativeToolResult::parse_from_json(Some(wire)).is_ok());
        }
    }

    #[test]
    fn create_agent_request_preserves_canonical_schema_value_json() {
        let parameters = empty_parameter_record();
        let config_value = json!({ "arbitrary": [1, true, null] });
        let request_json = json!({
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
            "parameters": empty_parameter_record(),
            "method": "run",
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
            "parameters": [],
        });

        assert!(CreateAgentRequest::parse_from_json(Some(request_json)).is_err());
    }

    #[test]
    fn agent_requests_reject_forged_host_managed_parameters() {
        let forged = serde_json::to_value(SchemaValue::Secret(SecretValuePayload {
            secret_id: Uuid::nil(),
            config_key: None,
            version: 1,
            resolved_at: Utc.timestamp_opt(0, 0).unwrap(),
            category: None,
        }))
        .unwrap();

        let create = json!({
            "parameters": forged,
        });
        assert!(CreateAgentRequest::parse_from_json(Some(create)).is_err());

        let invoke = json!({
            "parameters": empty_parameter_record(),
            "method": "run",
            "methodParameters": forged,
            "mode": "await",
        });
        assert!(AgentInvocationRequest::parse_from_json(Some(invoke)).is_err());
    }
}
