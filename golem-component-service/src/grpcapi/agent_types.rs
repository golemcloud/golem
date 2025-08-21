use crate::api::common::ComponentTraceErrorKind;
use crate::authed::agent_types::AuthedAgentTypesService;
use crate::grpcapi::auth;
use async_trait::async_trait;
use golem_api_grpc::proto::golem::component::v1::agent_types_service_server::AgentTypesService as GrpcAgentTypesService;
use golem_api_grpc::proto::golem::component::v1::{
    get_all_response, get_response, ComponentError, GetAllRequest, GetAllResponse,
    GetAllSuccessResponse, GetRequest, GetResponse, RegisteredAgentType,
};
use golem_common::base_model::ProjectId;
use golem_common::recorded_grpc_api_request;
use golem_service_base::grpc::proto_project_id_string;
use std::sync::Arc;
use tonic::metadata::MetadataMap;
use tonic::{Request, Response, Status};
use tracing::Instrument;

pub struct AgentTypesGrpcApi {
    agent_types_service: Arc<AuthedAgentTypesService>,
}

impl AgentTypesGrpcApi {
    pub fn new(agent_types_service: Arc<AuthedAgentTypesService>) -> Self {
        Self {
            agent_types_service,
        }
    }

    async fn get_all(
        &self,
        request: GetAllRequest,
        metadata: MetadataMap,
    ) -> Result<Vec<RegisteredAgentType>, ComponentError> {
        let auth = auth(metadata)?;
        let project_id: Option<ProjectId> = request.project_id.and_then(|id| id.try_into().ok());
        let agent_types = self
            .agent_types_service
            .get_all_agent_types(project_id, auth)
            .await?;
        Ok(agent_types
            .into_iter()
            .map(|agent_type| agent_type.into())
            .collect())
    }

    async fn get(
        &self,
        request: GetRequest,
        metadata: MetadataMap,
    ) -> Result<Option<RegisteredAgentType>, ComponentError> {
        let auth = auth(metadata)?;
        let project_id: Option<ProjectId> = request.project_id.and_then(|id| id.try_into().ok());
        let agent_type = self
            .agent_types_service
            .get_agent_type(&request.agent_type, project_id, auth)
            .await?;
        Ok(agent_type.map(|at| at.into()))
    }
}

#[async_trait]
impl GrpcAgentTypesService for AgentTypesGrpcApi {
    async fn get_all(
        &self,
        request: Request<GetAllRequest>,
    ) -> Result<Response<GetAllResponse>, Status> {
        let (m, _, r) = request.into_parts();
        let record = recorded_grpc_api_request!(
            "get_all_agent_types",
            project_id = proto_project_id_string(&r.project_id)
        );

        let response = match self.get_all(r, m).instrument(record.span.clone()).await {
            Ok(agent_types) => {
                record.succeed(get_all_response::Result::Success(GetAllSuccessResponse {
                    agent_types,
                }))
            }
            Err(error) => record.fail(
                get_all_response::Result::Error(error.clone()),
                &ComponentTraceErrorKind(&error),
            ),
        };

        Ok(Response::new(GetAllResponse {
            result: Some(response),
        }))
    }

    async fn get(&self, request: Request<GetRequest>) -> Result<Response<GetResponse>, Status> {
        let (m, _, r) = request.into_parts();
        let record = recorded_grpc_api_request!(
            "get_agent_type",
            project_id = proto_project_id_string(&r.project_id)
        );

        let response = match self.get(r, m).instrument(record.span.clone()).await {
            Ok(Some(agent_type)) => record.succeed(get_response::Result::Success(agent_type)),
            Ok(None) => {
                let error = ComponentError {
                    error: Some(
                        golem_api_grpc::proto::golem::component::v1::component_error::Error::NotFound(
                            golem_api_grpc::proto::golem::common::ErrorBody {
                                error: "Agent type not found".to_string(),
                            },
                        ),
                    ),
                };
                record.fail(
                    get_response::Result::Error(error.clone()),
                    &ComponentTraceErrorKind(&error),
                )
            }
            Err(error) => record.fail(
                get_response::Result::Error(error.clone()),
                &ComponentTraceErrorKind(&error),
            ),
        };

        Ok(Response::new(GetResponse {
            result: Some(response),
        }))
    }
}
