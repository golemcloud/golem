use crate::auth::{CloudAuthCtx, CloudNamespace};
use crate::clients::project::{ProjectError, ProjectService};
use crate::model::{ProjectAction, TokenSecret};
use crate::SafeDisplay;
use async_trait::async_trait;
use golem_api_grpc::proto::golem::common::ErrorBody;
use golem_api_grpc::proto::golem::worker::v1::{
    worker_error, worker_execution_error, UnknownError, WorkerExecutionError,
};
use golem_common::model::{AccountId, ProjectId};
use std::str::FromStr;
use std::sync::Arc;
use tonic::metadata::MetadataMap;
use tracing::debug;
use uuid::Uuid;

#[async_trait]
pub trait BaseAuthService {
    async fn is_authorized(
        &self,
        project_id: &ProjectId,
        permission: ProjectAction,
        ctx: &CloudAuthCtx,
    ) -> Result<CloudNamespace, AuthServiceError>;
}

#[derive(Clone)]
pub struct CloudAuthService {
    project_service: Arc<dyn ProjectService + Sync + Send>,
}

impl CloudAuthService {
    pub fn new(project_service: Arc<dyn ProjectService + Sync + Send>) -> Self {
        Self { project_service }
    }
}

#[async_trait]
impl BaseAuthService for CloudAuthService {
    async fn is_authorized(
        &self,
        project_id: &ProjectId,
        permission: ProjectAction,
        ctx: &CloudAuthCtx,
    ) -> Result<CloudNamespace, AuthServiceError> {
        let project_actions = self
            .project_service
            .get_actions(project_id, &ctx.token_secret)
            .await?;
        let project_id = project_actions.project_id.clone();
        let account_id: AccountId = project_actions.owner_account_id;
        let actions = project_actions.actions.actions;
        let has_permission = actions.contains(&permission);

        debug!("is_authorized - project_id: {project_id}, action: {permission:?}, actions: {actions:?}, has_permission: {has_permission}");

        if has_permission {
            Ok(CloudNamespace::new(project_id, account_id))
        } else {
            Err(AuthServiceError::Forbidden(format!(
                "No permission {permission:?}"
            )))
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum AuthServiceError {
    #[error("Unauthorized: {0}")]
    Unauthorized(String),
    #[error("Forbidden: {0}")]
    Forbidden(String),
    #[error("Internal error: {0}")]
    InternalClientError(String),
}

impl AuthServiceError {
    pub fn internal_client_error(error: impl AsRef<str>) -> Self {
        Self::InternalClientError(error.as_ref().to_string())
    }
}

impl SafeDisplay for AuthServiceError {
    fn to_safe_string(&self) -> String {
        match self {
            AuthServiceError::Unauthorized(_) => self.to_string(),
            AuthServiceError::Forbidden(_) => self.to_string(),
            AuthServiceError::InternalClientError(_) => self.to_string(),
        }
    }
}

impl From<ProjectError> for AuthServiceError {
    fn from(value: ProjectError) -> Self {
        use cloud_api_grpc::proto::golem::cloud::project::v1::project_error;

        match value {
            ProjectError::Server(err) => match err.error {
                Some(project_error::Error::BadRequest(errors)) => {
                    AuthServiceError::internal_client_error(errors.errors.join(", "))
                }
                Some(project_error::Error::InternalError(error)) => {
                    AuthServiceError::internal_client_error(error.error)
                }
                Some(project_error::Error::Unauthorized(error)) => {
                    AuthServiceError::Unauthorized(error.error)
                }
                Some(project_error::Error::LimitExceeded(error)) => {
                    AuthServiceError::Forbidden(error.error)
                }
                Some(project_error::Error::NotFound(error)) => {
                    AuthServiceError::Forbidden(error.error)
                }
                None => AuthServiceError::internal_client_error("Unknown error"),
            },
            ProjectError::Connection(status) => {
                AuthServiceError::internal_client_error(format!("Connection error: {status}"))
            }
            ProjectError::Transport(error) => {
                AuthServiceError::internal_client_error(format!("Transport error: {error}"))
            }
            ProjectError::Unknown(error) => {
                AuthServiceError::internal_client_error(format!("Unknown error: {error}"))
            }
        }
    }
}

impl From<AuthServiceError> for golem_api_grpc::proto::golem::worker::v1::WorkerError {
    fn from(value: AuthServiceError) -> Self {
        let error = match value {
            AuthServiceError::Unauthorized(_) => worker_error::Error::Unauthorized(ErrorBody {
                error: value.to_string(),
            }),
            AuthServiceError::Forbidden(_) => worker_error::Error::Unauthorized(ErrorBody {
                error: value.to_string(),
            }),
            // TODO: this used to be unauthorized. How do we handle internal server errors?
            AuthServiceError::InternalClientError(message) => {
                worker_error::Error::InternalError(WorkerExecutionError {
                    error: Some(worker_execution_error::Error::Unknown(UnknownError {
                        details: message,
                    })),
                })
            }
        };
        golem_api_grpc::proto::golem::worker::v1::WorkerError { error: Some(error) }
    }
}

pub fn authorised_request<T>(request: T, access_token: &Uuid) -> tonic::Request<T> {
    let mut req = tonic::Request::new(request);
    req.metadata_mut().insert(
        "authorization",
        format!("Bearer {}", access_token).parse().unwrap(),
    );
    req
}

pub fn get_authorisation_token(metadata: MetadataMap) -> Option<TokenSecret> {
    let auth = metadata
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .map(|v| v.to_string());

    match auth {
        Some(a) if a.to_lowercase().starts_with("bearer ") => {
            let t = &a[7..a.len()];
            TokenSecret::from_str(t.trim()).ok()
        }
        _ => None,
    }
}
