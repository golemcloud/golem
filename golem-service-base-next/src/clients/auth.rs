// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use super::authorised_request;
use super::RemoteServiceConfig;
use golem_api_grpc::proto::golem::auth::v1::cloud_auth_service_client::CloudAuthServiceClient;
use golem_api_grpc::proto::golem::auth::v1::{
    authorize_account_action_response, authorize_project_action_response, get_account_response,
    AuthorizeAccountActionRequest, AuthorizeProjectActionRequest, GetAccountRequest,
};
use golem_api_grpc::proto::golem::common::ErrorBody;
use golem_api_grpc::proto::golem::worker::v1::{
    worker_error, worker_execution_error, UnknownError, WorkerExecutionError,
};
use golem_common_next::client::{GrpcClient, GrpcClientConfig};
use golem_common_next::model::auth::{AccountAction, ProjectAction};
use golem_common_next::model::auth::{AuthCtx, Namespace};
use golem_common_next::model::{AccountId, ProjectId, RetryConfig};
use golem_common_next::retries::with_retries;
use golem_common_next::SafeDisplay;
use std::fmt::Display;
use tonic::codec::CompressionEncoding;
use tonic::transport::Channel;
use tonic::Status;

#[derive(Clone)]
pub struct AuthService {
    auth_service_client: GrpcClient<CloudAuthServiceClient<Channel>>,
    retry_config: RetryConfig,
}

impl AuthService {
    pub fn new(config: &RemoteServiceConfig) -> Self {
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

    pub async fn get_account(&self, ctx: &AuthCtx) -> Result<AccountId, AuthServiceError> {
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

    pub async fn authorize_account_action(
        &self,
        account_id: &AccountId,
        action: AccountAction,
        ctx: &AuthCtx,
    ) -> Result<(), AuthServiceError> {
        let result: Result<(), AuthClientError> = with_retries(
            "auth",
            "authorize-project-action",
            Some(format!("{action:}")),
            &self.retry_config,
            &(
                self.auth_service_client.clone(),
                account_id.clone(),
                action.clone(),
                ctx.token_secret.value,
            ),
            |(client, account_id, action, token)| {
                Box::pin(async move {
                    let response = client
                        .call("authorize-account-action", move |client| {
                            let request = authorised_request(
                                AuthorizeAccountActionRequest {
                                    account_id: Some(account_id.clone().into()),
                                    action: action.clone() as i32,
                                },
                                token,
                            );

                            Box::pin(client.authorize_account_action(request))
                        })
                        .await?
                        .into_inner();
                    match response.result {
                        None => Err("Empty response".to_string().into()),
                        Some(authorize_account_action_response::Result::Success(_)) => Ok(()),
                        Some(authorize_account_action_response::Result::Error(error)) => {
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

    pub async fn authorize_project_action(
        &self,
        project_id: &ProjectId,
        action: ProjectAction,
        ctx: &AuthCtx,
    ) -> Result<Namespace, AuthServiceError> {
        let result: Result<Namespace, AuthClientError> = with_retries(
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
                            Ok(Namespace {
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
    Server(golem_api_grpc::proto::golem::auth::v1::AuthError),
    Connection(Status),
    Transport(tonic::transport::Error),
    Unknown(String),
}

impl From<golem_api_grpc::proto::golem::auth::v1::AuthError> for AuthClientError {
    fn from(value: golem_api_grpc::proto::golem::auth::v1::AuthError) -> Self {
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
        use golem_api_grpc::proto::golem::auth::v1::auth_error::Error;

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
        use golem_api_grpc::proto::golem::auth::v1::auth_error::Error;

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
