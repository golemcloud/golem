use crate::auth::{CloudAuthCtx, CloudNamespace};
use crate::config::RemoteCloudServiceConfig;
use crate::model::{ProjectAction, TokenSecret};
use async_trait::async_trait;
use cloud_api_grpc::proto::golem::cloud::auth::v1::cloud_auth_service_client::CloudAuthServiceClient;
use cloud_api_grpc::proto::golem::cloud::auth::v1::{
    authorize_project_action_response, get_account_response, AuthorizeProjectActionRequest,
    GetAccountRequest,
};
use golem_api_grpc::proto::golem::common::ErrorBody;
use golem_api_grpc::proto::golem::worker::v1::{
    worker_error, worker_execution_error, UnknownError, WorkerExecutionError,
};
use golem_common::client::{GrpcClient, GrpcClientConfig};
use golem_common::model::{AccountId, ProjectId, RetryConfig};
use golem_common::retries::with_retries;
use golem_common::SafeDisplay;
use std::fmt::Display;
use std::str::FromStr;
use tonic::codec::CompressionEncoding;
use tonic::metadata::MetadataMap;
use tonic::transport::Channel;
use tonic::Status;
use uuid::Uuid;

#[async_trait]
pub trait BaseAuthService: Send + Sync {
    async fn get_account(&self, ctx: &CloudAuthCtx) -> Result<AccountId, AuthServiceError>;

    async fn authorize_project_action(
        &self,
        project_id: &ProjectId,
        permission: ProjectAction,
        ctx: &CloudAuthCtx,
    ) -> Result<CloudNamespace, AuthServiceError>;
}

#[derive(Clone)]
pub struct CloudAuthService {
    auth_service_client: GrpcClient<CloudAuthServiceClient<Channel>>,
    retry_config: RetryConfig,
}

impl CloudAuthService {
    pub fn new(config: &RemoteCloudServiceConfig) -> Self {
        let auth_service_client: GrpcClient<CloudAuthServiceClient<Channel>> = GrpcClient::new(
            "auth",
            |channel| {
                CloudAuthServiceClient::new(channel)
                    .send_compressed(CompressionEncoding::Gzip)
                    .accept_compressed(CompressionEncoding::Gzip)
            },
            config.uri(),
            GrpcClientConfig {
                retries_on_unavailable: config.retries.clone(),
                ..Default::default() // TODO
            },
        );
        Self {
            auth_service_client,
            retry_config: config.retries.clone(),
        }
    }
}

#[async_trait]
impl BaseAuthService for CloudAuthService {
    async fn get_account(&self, ctx: &CloudAuthCtx) -> Result<AccountId, AuthServiceError> {
        let result: Result<AccountId, AuthClientError> = with_retries(
            "auth",
            "get-account",
            None,
            &self.retry_config,
            &(self.auth_service_client.clone(), ctx.token_secret.value),
            |(client, token)| {
                Box::pin(async move {
                    let response = client
                        .call("get-account", move |client| {
                            let request = authorised_request(GetAccountRequest {}, token);

                            Box::pin(client.get_account(request))
                        })
                        .await?
                        .into_inner();
                    match response.result {
                        None => Err("Empty response".to_string().into()),
                        Some(get_account_response::Result::Success(payload)) => Ok(AccountId {
                            value: payload.account_id.unwrap().name,
                        }),
                        Some(get_account_response::Result::Error(error)) => Err(error.into()),
                    }
                })
            },
            AuthClientError::is_retriable,
        )
        .await;

        result.map_err(|e| e.into())
    }

    async fn authorize_project_action(
        &self,
        project_id: &ProjectId,
        action: ProjectAction,
        ctx: &CloudAuthCtx,
    ) -> Result<CloudNamespace, AuthServiceError> {
        let result: Result<CloudNamespace, AuthClientError> = with_retries(
            "auth",
            "authorize-project-action",
            Some(format!("{action:}")),
            &self.retry_config,
            &(
                self.auth_service_client.clone(),
                project_id.clone(),
                action.clone(),
                ctx.token_secret.value,
            ),
            |(client, project_id, action, token)| {
                Box::pin(async move {
                    let response = client
                        .call("authorize-project-action", move |client| {
                            let request = authorised_request(
                                AuthorizeProjectActionRequest {
                                    project_id: Some(project_id.clone().into()),
                                    action: action.clone() as i32,
                                },
                                token,
                            );

                            Box::pin(client.authorize_project_action(request))
                        })
                        .await?
                        .into_inner();
                    match response.result {
                        None => Err("Empty response".to_string().into()),
                        Some(authorize_project_action_response::Result::Success(payload)) => {
                            let account_id = AccountId {
                                value: payload.project_owner_account_id.unwrap().name,
                            };
                            Ok(CloudNamespace {
                                account_id,
                                project_id: project_id.clone(),
                            })
                        }
                        Some(authorize_project_action_response::Result::Error(error)) => {
                            Err(error.into())
                        }
                    }
                })
            },
            AuthClientError::is_retriable,
        )
        .await;

        result.map_err(|e| e.into())
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

#[derive(Debug)]
pub enum AuthClientError {
    Server(cloud_api_grpc::proto::golem::cloud::auth::v1::AuthError),
    Connection(Status),
    Transport(tonic::transport::Error),
    Unknown(String),
}

impl From<cloud_api_grpc::proto::golem::cloud::auth::v1::AuthError> for AuthClientError {
    fn from(value: cloud_api_grpc::proto::golem::cloud::auth::v1::AuthError) -> Self {
        Self::Server(value)
    }
}

impl From<Status> for AuthClientError {
    fn from(value: Status) -> Self {
        Self::Connection(value)
    }
}

impl From<tonic::transport::Error> for AuthClientError {
    fn from(value: tonic::transport::Error) -> Self {
        Self::Transport(value)
    }
}

impl From<String> for AuthClientError {
    fn from(value: String) -> Self {
        Self::Unknown(value)
    }
}

impl AuthClientError {
    fn is_retriable(error: &AuthClientError) -> bool {
        matches!(
            error,
            AuthClientError::Connection(_) | AuthClientError::Transport(_)
        )
    }
}

impl From<AuthClientError> for AuthServiceError {
    fn from(value: AuthClientError) -> Self {
        use cloud_api_grpc::proto::golem::cloud::auth::v1::auth_error::Error;

        match value {
            AuthClientError::Server(err) => match err.error {
                Some(Error::BadRequest(errors)) => {
                    AuthServiceError::internal_client_error(errors.errors.join(", "))
                }
                Some(Error::InternalError(error)) => {
                    AuthServiceError::internal_client_error(error.error)
                }
                Some(Error::Unauthorized(error)) => AuthServiceError::Unauthorized(error.error),
                None => AuthServiceError::internal_client_error("Unknown error"),
            },
            AuthClientError::Connection(status) => {
                AuthServiceError::internal_client_error(format!("Connection error: {status}"))
            }
            AuthClientError::Transport(error) => {
                AuthServiceError::internal_client_error(format!("Transport error: {error}"))
            }
            AuthClientError::Unknown(error) => {
                AuthServiceError::internal_client_error(format!("Unknown error: {error}"))
            }
        }
    }
}

impl Display for AuthClientError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        use cloud_api_grpc::proto::golem::cloud::auth::v1::auth_error::Error;

        match &self {
            AuthClientError::Server(err) => match &err.error {
                Some(Error::BadRequest(errors)) => {
                    write!(f, "Invalid request: {:?}", errors.errors)
                }
                Some(Error::InternalError(error)) => {
                    write!(f, "Internal server error: {}", error.error)
                }
                Some(Error::Unauthorized(error)) => write!(f, "Unauthorized: {}", error.error),
                None => write!(f, "Unknown error"),
            },
            AuthClientError::Connection(status) => write!(f, "Connection error: {status}"),
            AuthClientError::Transport(error) => write!(f, "Transport error: {error}"),
            AuthClientError::Unknown(error) => write!(f, "Unknown error: {error}"),
        }
    }
}

impl std::error::Error for AuthClientError {}

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
