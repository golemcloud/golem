use crate::auth::AccountAuthorisation;
use crate::grpcapi::get_authorisation_token;
use crate::model;
use crate::service::auth::{AuthService, AuthServiceError};
use crate::service::project_policy;
use cloud_api_grpc::proto::golem::cloud::projectpolicy::cloud_project_policy_service_server::CloudProjectPolicyService;
use cloud_api_grpc::proto::golem::cloud::projectpolicy::{
    create_project_policy_response, get_project_policy_response, CreateProjectPolicyRequest,
    CreateProjectPolicyResponse, GetProjectPolicyRequest, GetProjectPolicyResponse,
};
use cloud_api_grpc::proto::golem::cloud::projectpolicy::{
    project_policy_error, ProjectPolicy, ProjectPolicyError,
};
use cloud_common::model::ProjectPolicyId;
use golem_api_grpc::proto::golem::common::ErrorBody;
use std::sync::Arc;
use tonic::metadata::MetadataMap;
use tonic::{Request, Response, Status};

impl From<AuthServiceError> for ProjectPolicyError {
    fn from(value: AuthServiceError) -> Self {
        let error = match value {
            AuthServiceError::InvalidToken(error) => {
                project_policy_error::Error::Unauthorized(ErrorBody { error })
            }
            AuthServiceError::Unexpected(error) => {
                project_policy_error::Error::Unauthorized(ErrorBody { error })
            }
        };
        ProjectPolicyError { error: Some(error) }
    }
}

impl From<project_policy::ProjectPolicyError> for ProjectPolicyError {
    fn from(value: project_policy::ProjectPolicyError) -> Self {
        let error = match value {
            project_policy::ProjectPolicyError::Internal(error) => {
                project_policy_error::Error::InternalError(ErrorBody { error })
            }
        };
        ProjectPolicyError { error: Some(error) }
    }
}

fn bad_request_error(error: &str) -> ProjectPolicyError {
    ProjectPolicyError {
        error: Some(project_policy_error::Error::NotFound(ErrorBody {
            error: error.to_string(),
        })),
    }
}

pub struct ProjectPolicyGrpcApi {
    pub auth_service: Arc<dyn AuthService + Sync + Send>,
    pub project_policy_service: Arc<dyn project_policy::ProjectPolicyService + Sync + Send>,
}

impl ProjectPolicyGrpcApi {
    async fn auth(
        &self,
        metadata: MetadataMap,
    ) -> Result<AccountAuthorisation, ProjectPolicyError> {
        match get_authorisation_token(metadata) {
            Some(t) => self
                .auth_service
                .authorization(&t)
                .await
                .map_err(|e| e.into()),
            None => Err(ProjectPolicyError {
                error: Some(project_policy_error::Error::Unauthorized(ErrorBody {
                    error: "Missing token".into(),
                })),
            }),
        }
    }

    async fn get(
        &self,
        request: GetProjectPolicyRequest,
        metadata: MetadataMap,
    ) -> Result<ProjectPolicy, ProjectPolicyError> {
        // FIXME auth check
        self.auth(metadata).await?;
        let id: ProjectPolicyId = request
            .project_policy_id
            .and_then(|id| id.try_into().ok())
            .ok_or_else(|| bad_request_error("Missing project policy id"))?;

        let result = self.project_policy_service.get(&id).await?;

        match result {
            Some(policy) => Ok(policy.into()),
            None => Err(ProjectPolicyError {
                error: Some(project_policy_error::Error::NotFound(ErrorBody {
                    error: "Project policy not found".to_string(),
                })),
            }),
        }
    }

    async fn create(
        &self,
        request: CreateProjectPolicyRequest,
        metadata: MetadataMap,
    ) -> Result<ProjectPolicy, ProjectPolicyError> {
        // FIXME auth check
        self.auth(metadata).await?;
        let policy: model::ProjectPolicy = request
            .project_policy_data
            .map(|p| {
                let project_actions: model::ProjectActions = p
                    .actions
                    .and_then(|a| a.try_into().ok())
                    .unwrap_or(model::ProjectActions::empty());
                model::ProjectPolicy {
                    id: ProjectPolicyId::new_v4(),
                    name: p.name,
                    project_actions,
                }
            })
            .ok_or_else(|| bad_request_error("Missing project policy data"))?;

        self.project_policy_service.create(&policy).await?;
        Ok(ProjectPolicy::from(policy))
    }
}

#[async_trait::async_trait]
impl CloudProjectPolicyService for ProjectPolicyGrpcApi {
    async fn create_project_policy(
        &self,
        request: Request<CreateProjectPolicyRequest>,
    ) -> Result<Response<CreateProjectPolicyResponse>, Status> {
        let (m, _, r) = request.into_parts();
        match self.create(r, m).await {
            Ok(result) => Ok(Response::new(CreateProjectPolicyResponse {
                result: Some(create_project_policy_response::Result::Success(result)),
            })),
            Err(err) => Ok(Response::new(CreateProjectPolicyResponse {
                result: Some(create_project_policy_response::Result::Error(err)),
            })),
        }
    }

    async fn get_project_policy(
        &self,
        request: Request<GetProjectPolicyRequest>,
    ) -> Result<Response<GetProjectPolicyResponse>, Status> {
        let (m, _, r) = request.into_parts();
        match self.get(r, m).await {
            Ok(result) => Ok(Response::new(GetProjectPolicyResponse {
                result: Some(get_project_policy_response::Result::Success(result)),
            })),
            Err(err) => Ok(Response::new(GetProjectPolicyResponse {
                result: Some(get_project_policy_response::Result::Error(err)),
            })),
        }
    }
}
