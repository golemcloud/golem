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

use super::RemoteServiceConfig;
use super::authorised_request;
use crate::model::auth::{AuthCtx, UserAuthCtx};
use async_trait::async_trait;
use golem_api_grpc::proto::golem::auth::v1::cloud_auth_service_client::CloudAuthServiceClient;
use golem_api_grpc::proto::golem::auth::v1::{
    AuthenticateTokenRequest, authenticate_token_response,
};
use golem_api_grpc::proto::golem::common::ErrorBody;
use golem_api_grpc::proto::golem::worker::v1::{
    UnknownError, WorkerExecutionError, worker_error, worker_execution_error,
};
use golem_common::SafeDisplay;
use golem_common::client::{GrpcClient, GrpcClientConfig};
use golem_common::model::RetryConfig;
use golem_common::model::auth::TokenSecret;
use golem_common::retries::with_retries;
use std::fmt::Display;
use tonic::Status;
use tonic::codec::CompressionEncoding;
use tonic::transport::Channel;
use uuid::Uuid;

#[async_trait]
pub trait AuthService: Send + Sync {
    async fn authenticate_token(&self, token: TokenSecret) -> Result<AuthCtx, AuthServiceError>;
}

pub struct AuthServiceDefault {
    auth_service_client: GrpcClient<CloudAuthServiceClient<Channel>>,
    access_token: Uuid,
    retry_config: RetryConfig,
}

impl AuthServiceDefault {
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
                ..Default::default()
            },
        );
        Self {
            auth_service_client,
            access_token: config.access_token,
            retry_config: config.retries.clone(),
        }
    }
}

#[async_trait]
impl AuthService for AuthServiceDefault {
    async fn authenticate_token(&self, token: TokenSecret) -> Result<AuthCtx, AuthServiceError> {
        let result: Result<AuthCtx, AuthClientError> = with_retries(
            "auth",
            "authenticate-token",
            None,
            &self.retry_config,
            &(self.auth_service_client.clone(), token, self.access_token),
            |(client, token, access_token)| {
                Box::pin(async move {
                    let response = client
                        .call("get-account", move |client| {
                            let request = authorised_request(
                                AuthenticateTokenRequest {
                                    secret: Some(token.0.into()),
                                },
                                access_token,
                            );

                            Box::pin(client.authenticate_token(request))
                        })
                        .await?
                        .into_inner();
                    match response.result {
                        None => Err("Empty response".to_string().into()),
                        Some(authenticate_token_response::Result::Success(payload)) => {
                            let user_auth_ctx: UserAuthCtx =
                                payload.auth_ctx.unwrap().try_into()?;
                            Ok(AuthCtx::User(user_auth_ctx))
                        }
                        Some(authenticate_token_response::Result::Error(error)) => {
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
