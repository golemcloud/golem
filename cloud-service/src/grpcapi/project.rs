use std::fmt::{Debug, Formatter};
use std::sync::Arc;

use crate::auth::AccountAuthorisation;
use crate::grpcapi::get_authorisation_token;
use crate::model;
use crate::service::auth::{AuthService, AuthServiceError};
use crate::service::project;
use crate::service::project_auth;
use crate::service::project_auth::ProjectAuthorisationService;
use cloud_api_grpc::proto::golem::cloud::project::v1::cloud_project_service_server::CloudProjectService;
use cloud_api_grpc::proto::golem::cloud::project::v1::{
    create_project_response, delete_project_response, get_default_project_response,
    get_project_actions_response, get_project_response, get_projects_response, project_error,
    CreateProjectRequest, CreateProjectResponse, CreateProjectSuccessResponse,
    DeleteProjectRequest, DeleteProjectResponse, GetDefaultProjectRequest,
    GetDefaultProjectResponse, GetProjectActionsRequest, GetProjectActionsResponse,
    GetProjectActionsSuccessResponse, GetProjectRequest, GetProjectResponse, GetProjectsRequest,
    GetProjectsResponse, GetProjectsSuccessResponse, ProjectError,
};
use cloud_api_grpc::proto::golem::cloud::project::Project;
use cloud_common::grpc::proto_project_id_string;
use golem_api_grpc::proto::golem::common::{Empty, ErrorBody, ErrorsBody};
use golem_common::metrics::api::TraceErrorKind;
use golem_common::model::ProjectId;
use golem_common::recorded_grpc_api_request;
use tonic::metadata::MetadataMap;
use tonic::{Request, Response, Status};
use tracing::Instrument;

impl From<AuthServiceError> for ProjectError {
    fn from(value: AuthServiceError) -> Self {
        let error = match value {
            AuthServiceError::InvalidToken(error) => {
                project_error::Error::Unauthorized(ErrorBody { error })
            }
            AuthServiceError::Unexpected(error) => {
                project_error::Error::Unauthorized(ErrorBody { error })
            }
        };
        ProjectError { error: Some(error) }
    }
}

impl From<project::ProjectError> for ProjectError {
    fn from(value: project::ProjectError) -> Self {
        let error = match value {
            project::ProjectError::Internal(error) => {
                project_error::Error::InternalError(ErrorBody { error })
            }
            project::ProjectError::Unauthorized(error) => {
                project_error::Error::Unauthorized(ErrorBody { error })
            }
            project::ProjectError::LimitExceeded(error) => {
                project_error::Error::LimitExceeded(ErrorBody { error })
            }
        };
        ProjectError { error: Some(error) }
    }
}

impl From<project_auth::ProjectAuthorisationError> for ProjectError {
    fn from(value: project_auth::ProjectAuthorisationError) -> Self {
        let error = match value {
            project_auth::ProjectAuthorisationError::Internal(error) => {
                project_error::Error::InternalError(ErrorBody { error })
            }
            project_auth::ProjectAuthorisationError::Unauthorized(error) => {
                project_error::Error::Unauthorized(ErrorBody { error })
            }
        };
        ProjectError { error: Some(error) }
    }
}

fn bad_request_error(error: &str) -> ProjectError {
    ProjectError {
        error: Some(project_error::Error::BadRequest(ErrorsBody {
            errors: vec![error.to_string()],
        })),
    }
}

pub struct ProjectGrpcApi {
    pub auth_service: Arc<dyn AuthService + Sync + Send>,
    pub project_service: Arc<dyn project::ProjectService + Sync + Send>,
    pub project_auth_service: Arc<dyn ProjectAuthorisationService + Sync + Send>,
}

impl ProjectGrpcApi {
    async fn auth(&self, metadata: MetadataMap) -> Result<AccountAuthorisation, ProjectError> {
        match get_authorisation_token(metadata) {
            Some(t) => self
                .auth_service
                .authorization(&t)
                .await
                .map_err(|e| e.into()),
            None => Err(ProjectError {
                error: Some(project_error::Error::Unauthorized(ErrorBody {
                    error: "Missing token".into(),
                })),
            }),
        }
    }

    async fn get_default(
        &self,
        _request: GetDefaultProjectRequest,
        metadata: MetadataMap,
    ) -> Result<Project, ProjectError> {
        let auth = self.auth(metadata).await?;

        let result = self.project_service.get_own_default(&auth).await?;
        Ok(result.into())
    }

    async fn get(
        &self,
        request: GetProjectRequest,
        metadata: MetadataMap,
    ) -> Result<Project, ProjectError> {
        let auth = self.auth(metadata).await?;

        let id: ProjectId = request
            .project_id
            .and_then(|id| id.try_into().ok())
            .ok_or_else(|| bad_request_error("Missing project id"))?;

        let result = self.project_service.get(&id, &auth).await?;

        match result {
            Some(project) => Ok(project.into()),
            None => Err(ProjectError {
                error: Some(project_error::Error::NotFound(ErrorBody {
                    error: "Project not found".to_string(),
                })),
            }),
        }
    }

    async fn get_all(
        &self,
        request: GetProjectsRequest,
        metadata: MetadataMap,
    ) -> Result<Vec<Project>, ProjectError> {
        let auth = self.auth(metadata).await?;

        let projects = match request.project_name {
            Some(name) => self.project_service.get_own_by_name(&name, &auth).await?,
            None => self.project_service.get_own(&auth).await?,
        };

        Ok(projects.into_iter().map(|p| p.into()).collect())
    }

    async fn get_actions(
        &self,
        request: GetProjectActionsRequest,
        metadata: MetadataMap,
    ) -> Result<GetProjectActionsSuccessResponse, ProjectError> {
        let auth = self.auth(metadata).await?;
        let id: ProjectId = request
            .project_id
            .and_then(|id| id.try_into().ok())
            .ok_or_else(|| bad_request_error("Missing project id"))?;

        let result = self.project_auth_service.get_by_project(&id, &auth).await?;

        let actions = result
            .actions
            .actions
            .clone()
            .iter()
            .map(|a| a.clone().into())
            .collect::<Vec<i32>>();

        Ok(GetProjectActionsSuccessResponse {
            project_id: Some(result.project_id.clone().into()),
            owner_account_id: Some(result.owner_account_id.clone().into()),
            actions,
        })
    }

    async fn delete(
        &self,
        request: DeleteProjectRequest,
        metadata: MetadataMap,
    ) -> Result<(), ProjectError> {
        let auth = self.auth(metadata).await?;
        let id: ProjectId = request
            .project_id
            .and_then(|id| id.try_into().ok())
            .ok_or_else(|| bad_request_error("Missing project id"))?;
        self.project_service.delete(&id, &auth).await?;

        Ok(())
    }

    async fn create(
        &self,
        request: CreateProjectRequest,
        metadata: MetadataMap,
    ) -> Result<Project, ProjectError> {
        let auth = self.auth(metadata).await?;

        let owner_account_id: golem_common::model::AccountId = request
            .owner_account_id
            .map(|id| id.into())
            .ok_or_else(|| bad_request_error("Missing account id"))?;

        let project = model::Project {
            project_id: ProjectId::new_v4(),
            project_data: model::ProjectData {
                name: request.name,
                owner_account_id,
                description: request.description,
                default_environment_id: "default".to_string(),
                project_type: model::ProjectType::NonDefault,
            },
        };

        self.project_service.create(&project, &auth).await?;

        Ok(project.into())
    }
}

#[async_trait::async_trait]
impl CloudProjectService for ProjectGrpcApi {
    async fn get_default_project(
        &self,
        request: Request<GetDefaultProjectRequest>,
    ) -> Result<Response<GetDefaultProjectResponse>, Status> {
        let (m, _, r) = request.into_parts();

        let record = recorded_grpc_api_request!("get_default_project",);

        let response = match self.get_default(r, m).instrument(record.span.clone()).await {
            Ok(result) => record.succeed(get_default_project_response::Result::Success(result)),
            Err(error) => record.fail(
                get_default_project_response::Result::Error(error.clone()),
                &ProjectTraceErrorKind(&error),
            ),
        };

        Ok(Response::new(GetDefaultProjectResponse {
            result: Some(response),
        }))
    }

    async fn get_projects(
        &self,
        request: Request<GetProjectsRequest>,
    ) -> Result<Response<GetProjectsResponse>, Status> {
        let (m, _, r) = request.into_parts();

        let record = recorded_grpc_api_request!("get_projects", project_name = &r.project_name);

        let response = match self.get_all(r, m).instrument(record.span.clone()).await {
            Ok(data) => record.succeed(get_projects_response::Result::Success(
                GetProjectsSuccessResponse { data },
            )),
            Err(error) => record.fail(
                get_projects_response::Result::Error(error.clone()),
                &ProjectTraceErrorKind(&error),
            ),
        };

        Ok(Response::new(GetProjectsResponse {
            result: Some(response),
        }))
    }

    async fn create_project(
        &self,
        request: Request<CreateProjectRequest>,
    ) -> Result<Response<CreateProjectResponse>, Status> {
        let (m, _, r) = request.into_parts();

        let record = recorded_grpc_api_request!("create_project", project_name = r.name);

        let response = match self.create(r, m).instrument(record.span.clone()).await {
            Ok(result) => record.succeed(create_project_response::Result::Success(
                CreateProjectSuccessResponse {
                    project: Some(result),
                },
            )),
            Err(error) => record.fail(
                create_project_response::Result::Error(error.clone()),
                &ProjectTraceErrorKind(&error),
            ),
        };

        Ok(Response::new(CreateProjectResponse {
            result: Some(response),
        }))
    }

    async fn delete_project(
        &self,
        request: Request<DeleteProjectRequest>,
    ) -> Result<Response<DeleteProjectResponse>, Status> {
        let (m, _, r) = request.into_parts();

        let record = recorded_grpc_api_request!(
            "delete_project",
            project_id = proto_project_id_string(&r.project_id)
        );

        let response = match self.delete(r, m).instrument(record.span.clone()).await {
            Ok(_) => record.succeed(delete_project_response::Result::Success(Empty {})),
            Err(error) => record.fail(
                delete_project_response::Result::Error(error.clone()),
                &ProjectTraceErrorKind(&error),
            ),
        };

        Ok(Response::new(DeleteProjectResponse {
            result: Some(response),
        }))
    }

    async fn get_project(
        &self,
        request: Request<GetProjectRequest>,
    ) -> Result<Response<GetProjectResponse>, Status> {
        let (m, _, r) = request.into_parts();

        let record = recorded_grpc_api_request!(
            "get_project",
            project_id = proto_project_id_string(&r.project_id)
        );

        let response = match self.get(r, m).instrument(record.span.clone()).await {
            Ok(result) => record.succeed(get_project_response::Result::Success(result)),
            Err(error) => record.fail(
                get_project_response::Result::Error(error.clone()),
                &ProjectTraceErrorKind(&error),
            ),
        };

        Ok(Response::new(GetProjectResponse {
            result: Some(response),
        }))
    }

    async fn get_project_actions(
        &self,
        request: Request<GetProjectActionsRequest>,
    ) -> Result<Response<GetProjectActionsResponse>, Status> {
        let (m, _, r) = request.into_parts();

        let record = recorded_grpc_api_request!(
            "get_project_actions",
            project_id = proto_project_id_string(&r.project_id)
        );

        let response = match self.get_actions(r, m).instrument(record.span.clone()).await {
            Ok(result) => record.succeed(get_project_actions_response::Result::Success(result)),
            Err(error) => record.fail(
                get_project_actions_response::Result::Error(error.clone()),
                &ProjectTraceErrorKind(&error),
            ),
        };

        Ok(Response::new(GetProjectActionsResponse {
            result: Some(response),
        }))
    }
}

pub struct ProjectTraceErrorKind<'a>(pub &'a ProjectError);

impl<'a> Debug for ProjectTraceErrorKind<'a> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl<'a> TraceErrorKind for ProjectTraceErrorKind<'a> {
    fn trace_error_kind(&self) -> &'static str {
        match &self.0.error {
            None => "None",
            Some(error) => match error {
                project_error::Error::BadRequest(_) => "BadRequest",
                project_error::Error::Unauthorized(_) => "Unauthorized",
                project_error::Error::LimitExceeded(_) => "LimitExceeded",
                project_error::Error::NotFound(_) => "NotFound",
                project_error::Error::InternalError(_) => "InternalError",
            },
        }
    }
}
