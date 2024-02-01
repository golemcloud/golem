use std::collections::HashSet;
use std::sync::Arc;

use cloud_api_grpc::proto::golem::cloud::projectgrant::cloud_project_grant_service_server::CloudProjectGrantService;
use cloud_api_grpc::proto::golem::cloud::projectgrant::{
    create_project_grant_response, delete_project_grant_response, get_project_grant_response,
    get_project_grants_response, CreateProjectGrantRequest, CreateProjectGrantResponse,
    DeleteProjectGrantRequest, DeleteProjectGrantResponse, GetProjectGrantRequest,
    GetProjectGrantResponse, GetProjectGrantsRequest, GetProjectGrantsResponse,
    GetProjectGrantsSuccessResponse,
};
use cloud_api_grpc::proto::golem::cloud::projectgrant::{
    project_grant_error, ProjectGrant, ProjectGrantDataRequest, ProjectGrantError,
};

use cloud_common::model::ProjectGrantId;
use cloud_common::model::ProjectPolicyId;
use golem_api_grpc::proto::golem::common::{Empty, ErrorBody, ErrorsBody};
use golem_common::model::ProjectId;
use tonic::metadata::MetadataMap;
use tonic::{Request, Response, Status};

use crate::auth::AccountAuthorisation;
use crate::grpcapi::get_authorisation_token;
use crate::model;
use crate::service::auth::{AuthService, AuthServiceError};
use crate::service::{project_grant, project_policy};

impl From<AuthServiceError> for ProjectGrantError {
    fn from(value: AuthServiceError) -> Self {
        let error = match value {
            AuthServiceError::InvalidToken(error) => {
                project_grant_error::Error::Unauthorized(ErrorBody { error })
            }
            AuthServiceError::Unexpected(error) => {
                project_grant_error::Error::Unauthorized(ErrorBody { error })
            }
        };
        ProjectGrantError { error: Some(error) }
    }
}

impl From<project_grant::ProjectGrantError> for ProjectGrantError {
    fn from(value: project_grant::ProjectGrantError) -> Self {
        let error = match value {
            project_grant::ProjectGrantError::Internal(error) => {
                project_grant_error::Error::InternalError(ErrorBody { error })
            }
            project_grant::ProjectGrantError::Unauthorized(error) => {
                project_grant_error::Error::Unauthorized(ErrorBody { error })
            }
            project_grant::ProjectGrantError::ProjectIdNotFound(_) => {
                project_grant_error::Error::BadRequest(ErrorsBody {
                    errors: vec!["Project not found".to_string()],
                })
            }
        };
        ProjectGrantError { error: Some(error) }
    }
}

impl From<project_policy::ProjectPolicyError> for ProjectGrantError {
    fn from(value: project_policy::ProjectPolicyError) -> Self {
        let error = match value {
            project_policy::ProjectPolicyError::Internal(error) => {
                project_grant_error::Error::InternalError(ErrorBody { error })
            }
        };
        ProjectGrantError { error: Some(error) }
    }
}

fn bad_request_error(error: &str) -> ProjectGrantError {
    ProjectGrantError {
        error: Some(project_grant_error::Error::BadRequest(ErrorsBody {
            errors: vec![error.to_string()],
        })),
    }
}

pub struct ProjectGrantGrpcApi {
    pub auth_service: Arc<dyn AuthService + Sync + Send>,
    pub project_grant_service: Arc<dyn project_grant::ProjectGrantService + Sync + Send>,
    pub project_policy_service: Arc<dyn project_policy::ProjectPolicyService + Sync + Send>,
}

impl ProjectGrantGrpcApi {
    async fn auth(&self, metadata: MetadataMap) -> Result<AccountAuthorisation, ProjectGrantError> {
        match get_authorisation_token(metadata) {
            Some(t) => self
                .auth_service
                .authorization(&t)
                .await
                .map_err(|e| e.into()),
            None => Err(ProjectGrantError {
                error: Some(project_grant_error::Error::Unauthorized(ErrorBody {
                    error: "Missing token".into(),
                })),
            }),
        }
    }

    async fn get_by_project(
        &self,
        request: GetProjectGrantsRequest,
        metadata: MetadataMap,
    ) -> Result<GetProjectGrantsSuccessResponse, ProjectGrantError> {
        let auth = self.auth(metadata).await?;
        let project_id: ProjectId = request
            .project_id
            .and_then(|id| id.try_into().ok())
            .ok_or_else(|| bad_request_error("Missing project id"))?;

        let values = self
            .project_grant_service
            .get_by_project(&project_id, &auth)
            .await?;

        let grants = values
            .iter()
            .map(|a| a.clone().into())
            .collect::<Vec<ProjectGrant>>();

        Ok(GetProjectGrantsSuccessResponse { data: grants })
    }

    async fn get(
        &self,
        request: GetProjectGrantRequest,
        metadata: MetadataMap,
    ) -> Result<ProjectGrant, ProjectGrantError> {
        let auth = self.auth(metadata).await?;

        let project_id: ProjectId = request
            .project_id
            .and_then(|id| id.try_into().ok())
            .ok_or_else(|| bad_request_error("Missing project id"))?;

        let grant_id: ProjectGrantId = request
            .grant_id
            .and_then(|id| id.try_into().ok())
            .ok_or_else(|| bad_request_error("Missing project grant id"))?;

        let result = self
            .project_grant_service
            .get(&project_id, &grant_id, &auth)
            .await?;

        match result {
            Some(grant) => Ok(grant.into()),
            None => Err(ProjectGrantError {
                error: Some(project_grant_error::Error::NotFound(ErrorBody {
                    error: "Project grant not found".to_string(),
                })),
            }),
        }
    }

    async fn delete(
        &self,
        request: DeleteProjectGrantRequest,
        metadata: MetadataMap,
    ) -> Result<(), ProjectGrantError> {
        let auth = self.auth(metadata).await?;

        let project_id: ProjectId = request
            .project_id
            .and_then(|id| id.try_into().ok())
            .ok_or_else(|| bad_request_error("Missing project id"))?;

        let grant_id: ProjectGrantId = request
            .grant_id
            .and_then(|id| id.try_into().ok())
            .ok_or_else(|| bad_request_error("Missing project grant id"))?;

        self.project_grant_service
            .delete(&project_id, &grant_id, &auth)
            .await?;

        Ok(())
    }

    async fn create(
        &self,
        request: CreateProjectGrantRequest,
        metadata: MetadataMap,
    ) -> Result<ProjectGrant, ProjectGrantError> {
        let auth = self.auth(metadata).await?;

        let grantor_project_id: ProjectId = request
            .project_id
            .and_then(|id| id.try_into().ok())
            .ok_or_else(|| bad_request_error("Missing project id"))?;

        let data: ProjectGrantDataRequest = request
            .data
            .ok_or_else(|| bad_request_error("Missing data"))?;

        let grantee_account_id: golem_common::model::AccountId = data
            .grantee_account_id
            .map(|id| id.into())
            .ok_or_else(|| bad_request_error("Missing account id"))?;

        let project_policy_id = match data.project_policy_id.and_then(|id| id.value) {
            Some(policy_id) => ProjectPolicyId(policy_id.into()),
            None => {
                let actions: HashSet<model::ProjectAction> = data
                    .project_actions
                    .into_iter()
                    .map(|action| action.try_into())
                    .collect::<Result<_, _>>()
                    .map_err(|_| bad_request_error("Invalid project actions"))?;

                let policy = model::ProjectPolicy {
                    id: ProjectPolicyId::new_v4(),
                    name: data.project_policy_name,
                    project_actions: model::ProjectActions { actions },
                };
                self.project_policy_service.create(&policy).await?;
                policy.id
            }
        };

        let grant = model::ProjectGrant {
            id: ProjectGrantId::new_v4(),
            data: model::ProjectGrantData {
                grantee_account_id,
                grantor_project_id,
                project_policy_id,
            },
        };

        self.project_grant_service.create(&grant, &auth).await?;

        Ok(grant.into())
    }
}

#[async_trait::async_trait]
impl CloudProjectGrantService for ProjectGrantGrpcApi {
    async fn get_project_grants(
        &self,
        request: Request<GetProjectGrantsRequest>,
    ) -> Result<Response<GetProjectGrantsResponse>, Status> {
        let (m, _, r) = request.into_parts();
        match self.get_by_project(r, m).await {
            Ok(result) => Ok(Response::new(GetProjectGrantsResponse {
                result: Some(get_project_grants_response::Result::Success(result)),
            })),
            Err(err) => Ok(Response::new(GetProjectGrantsResponse {
                result: Some(get_project_grants_response::Result::Error(err)),
            })),
        }
    }

    async fn delete_project_grant(
        &self,
        request: Request<DeleteProjectGrantRequest>,
    ) -> Result<Response<DeleteProjectGrantResponse>, Status> {
        let (m, _, r) = request.into_parts();
        match self.delete(r, m).await {
            Ok(_) => Ok(Response::new(DeleteProjectGrantResponse {
                result: Some(delete_project_grant_response::Result::Success(Empty {})),
            })),
            Err(err) => Ok(Response::new(DeleteProjectGrantResponse {
                result: Some(delete_project_grant_response::Result::Error(err)),
            })),
        }
    }

    async fn get_project_grant(
        &self,
        request: Request<GetProjectGrantRequest>,
    ) -> Result<Response<GetProjectGrantResponse>, Status> {
        let (m, _, r) = request.into_parts();
        match self.get(r, m).await {
            Ok(result) => Ok(Response::new(GetProjectGrantResponse {
                result: Some(get_project_grant_response::Result::Success(result)),
            })),
            Err(err) => Ok(Response::new(GetProjectGrantResponse {
                result: Some(get_project_grant_response::Result::Error(err)),
            })),
        }
    }

    async fn create_project_grant(
        &self,
        request: Request<CreateProjectGrantRequest>,
    ) -> Result<Response<CreateProjectGrantResponse>, Status> {
        let (m, _, r) = request.into_parts();
        match self.create(r, m).await {
            Ok(result) => Ok(Response::new(CreateProjectGrantResponse {
                result: Some(create_project_grant_response::Result::Success(result)),
            })),
            Err(err) => Ok(Response::new(CreateProjectGrantResponse {
                result: Some(create_project_grant_response::Result::Error(err)),
            })),
        }
    }
}
