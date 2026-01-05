use crate::api::common::ApiEndpointError;
use crate::service::auth::AuthService;
use crate::service::component::ComponentService;
use crate::service::worker::WorkerService;
use chrono::{DateTime, Utc};
use golem_common::model::agent::{AgentTypeName, DataValue};
use golem_common::model::application::ApplicationName;
use golem_common::model::environment::EnvironmentName;
use golem_common::model::{Empty, IdempotencyKey};
use golem_common::recorded_http_api_request;
use golem_service_base::api_tags::ApiTags;
use golem_service_base::model::auth::{AuthCtx, GolemSecurityScheme};
use poem_openapi::param::Header;
use poem_openapi::payload::Json;
use poem_openapi_derive::{Enum, Object, OpenApi, Union};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tracing::Instrument;
use uuid::Uuid;

type Result<T> = std::result::Result<T, ApiEndpointError>;

struct AgentsApi {
    component_service: Arc<dyn ComponentService>,
    worker_service: Arc<WorkerService>,
    auth_service: Arc<dyn AuthService>,
}

#[OpenApi(prefix_path = "/v1/agents", tag = ApiTags::Agent)]
impl AgentsApi {
    pub fn new(
        component_service: Arc<dyn ComponentService>,
        worker_service: Arc<WorkerService>,
        auth_service: Arc<dyn AuthService>,
    ) -> Self {
        Self {
            component_service,
            worker_service,
            auth_service,
        }
    }

    #[oai(path = "/invoke-agent", method = "post", operation_id = "invoke_agent")]
    async fn invoke_agent(
        &self,
        mut request: AgentInvocationRequest,
        #[oai(name = "Idempotency-Key")] idempotency_key: Header<Option<IdempotencyKey>>,
        token: GolemSecurityScheme,
    ) -> Result<Json<AgentInvocationResult>> {
        let auth = self.auth_service.authenticate_token(token.secret()).await?;

        request.idempotency_key = request.idempotency_key.or(idempotency_key.0);

        let record = recorded_http_api_request!(
            "invoke_agent",
            app = %request.app_name,
            env = %request.env_name,
            agent_type = %request.agent_type_name,
            idempotency_key = request.idempotency_key.as_ref().as_ref().map(|v| v.value.clone()),
            method = %request.method_name
        );

        let response = self
            .invoke_agent_internal(request, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response).map(Json)
    }

    async fn invoke_agent_internal(
        &self,
        request: AgentInvocationRequest,
        auth: AuthCtx,
    ) -> Result<AgentInvocationResult> {
        todo!()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Enum)]
#[oai(discriminator_name = "type", one_of = true)]
#[serde(tag = "type")]
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
    pub parameters: DataValue,
    pub phantom_id: Option<Uuid>,
    pub method_name: String,
    pub method_parameters: DataValue,
    pub mode: AgentInvocationMode,
    pub schedule_at: Option<DateTime<Utc>>,
    pub idempotency_key: Option<IdempotencyKey>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Object)]
#[oai(rename_all = "camelCase")]
#[serde(rename_all = "camelCase")]
pub struct AgentInvocationResult {
    pub result: Option<DataValue>,
}
