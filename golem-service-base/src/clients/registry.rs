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
use crate::model::auth::{AuthCtx, UserAuthCtx};
use async_trait::async_trait;
use golem_common::client::{GrpcClient, GrpcClientConfig};
use golem_common::model::RetryConfig;
use golem_common::model::auth::TokenSecret;
use golem_common::retries::with_retries;
use std::fmt::Display;
use tonic::Status;
use tonic::codec::CompressionEncoding;
use tonic::transport::Channel;
use golem_api_grpc::proto::golem::registry::v1::registry_service_client::RegistryServiceClient;
use golem_api_grpc::proto::golem::registry::v1::{authenticate_token_response, download_component_response, get_components_response, get_plugin_registration_by_id_response, get_resource_limits_response, update_worker_limit_response, AuthenticateTokenRequest, DownloadComponentRequest, GetComponentsRequest, GetPluginRegistrationByIdRequest, GetResourceLimitsRequest, UpdateWorkerLimitRequest};
use crate::model::ResourceLimits;
use golem_common::model::WorkerId;
use golem_common::model::account::AccountId;
use tracing::info;
use golem_common::model::plugin_registration::{PluginRegistrationId};
use crate::model::plugin_registration::PluginRegistration;
use golem_common::model::component::ComponentDto;
use golem_common::model::environment::EnvironmentId;
use golem_common::model::component::{ComponentId, ComponentRevision};

#[async_trait]
// mirrors golem-api-grpc/proto/golem/registry/v1/registry_service.proto
pub trait RegistryService: Send + Sync {
    // auth api
    async fn authenticate_token(&self, token: TokenSecret) -> Result<AuthCtx, RegistryServiceError>;

    // limits api
    async fn get_resource_limits(
        &self,
        account_id: &AccountId,
    ) -> Result<ResourceLimits, RegistryServiceError>;

    async fn update_worker_limit(
        &self,
        account_id: &AccountId,
        worker_id: &WorkerId,
        value: i32,
    ) -> Result<(), RegistryServiceError>;

    async fn update_worker_connection_limit(
        &self,
        account_id: &AccountId,
        worker_id: &WorkerId,
        value: i32,
    ) -> Result<(), RegistryServiceError>;

    // plugins api
    async fn get_plugin_registration_by_id(
        &self,
        plugin_id: &PluginRegistrationId,
    ) -> Result<PluginRegistration, RegistryServiceError>;

    // components api
    async fn resolve_component_by_name(
        &self,
        environment_id: &EnvironmentId,
        component_name: &str,
    ) -> Result<Option<ComponentDto>, RegistryServiceError>;

    async fn download_component(
        &self,
        component_id: &ComponentId,
        component_revision: ComponentRevision,
    ) -> Result<Vec<u8>, RegistryServiceError>;
}

pub struct GrpcRegistryService {
    client: GrpcClient<RegistryServiceClient<Channel>>,
    retry_config: RetryConfig,
}

impl GrpcRegistryService {
    pub fn new(config: &RemoteServiceConfig) -> Self {
        let client: GrpcClient<RegistryServiceClient<Channel>> = GrpcClient::new(
            "registry",
            |channel| {
                RegistryServiceClient::new(channel)
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
            client,
            retry_config: config.retries.clone(),
        }
    }
}

#[async_trait]
impl RegistryService for GrpcRegistryService {
    async fn authenticate_token(&self, token: TokenSecret) -> Result<AuthCtx, RegistryServiceError> {
        let result: Result<AuthCtx, RegistryServiceClientError> = with_retries(
            "auth",
            "authenticate-token",
            None,
            &self.retry_config,
            &(self.client.clone(), token),
            |(client, token)| {
                Box::pin(async move {
                    let response = client
                        .call("authenticate-token", move |client| {
                            let request = AuthenticateTokenRequest {
                                secret: Some(token.0.into()),
                            };

                            Box::pin(client.authenticate_token(request))
                        })
                        .await?
                        .into_inner();
                    match response.result {
                        None => Err(RegistryServiceClientError::empty_response()),
                        Some(authenticate_token_response::Result::Success(payload)) => {
                            let user_auth_ctx: UserAuthCtx = payload.auth_ctx.unwrap().try_into()?;
                            Ok(AuthCtx::User(user_auth_ctx))
                        }
                        Some(authenticate_token_response::Result::Error(error)) => {
                            Err(error.into())
                        }
                    }
                })
            },
            RegistryServiceClientError::is_retriable,
        )
        .await;

        result.map_err(|e| e.into())
    }

    async fn get_resource_limits(
        &self,
        account_id: &AccountId,
    ) -> Result<ResourceLimits, RegistryServiceError> {
        info!(account_id = %account_id, "Getting resource limits");
        let result: Result<ResourceLimits, RegistryServiceClientError> = with_retries(
            "limit",
            "get-resource-limits",
            Some(account_id.to_string()),
            &self.retry_config,
            &(
                self.client.clone(),
                account_id.clone(),
            ),
            |(client, id)| {
                Box::pin(async move {
                    let response = client
                        .call("get-resource-limits", move |client| {
                            let request = GetResourceLimitsRequest {
                                account_id: Some(id.clone().into()),
                            };
                            Box::pin(client.get_resource_limits(request))
                        })
                        .await?
                        .into_inner();

                    match response.result {
                        None => Err("Empty response".to_string().into()),
                        Some(get_resource_limits_response::Result::Success(response)) => {
                            Ok(response.into())
                        }
                        Some(get_resource_limits_response::Result::Error(error)) => {
                            Err(error.into())
                        }
                    }
                })
            },
            RegistryServiceClientError::is_retriable,
        )
        .await;

        result.map_err(|e| e.into())
    }

    async fn update_worker_limit(
        &self,
        account_id: &AccountId,
        worker_id: &WorkerId,
        value: i32,
    ) -> Result<(), RegistryServiceError> {
        let result: Result<(), RegistryServiceClientError> = with_retries(
            "limit",
            "update-worker-limit",
            Some(format!("{account_id} - {worker_id}")),
            &self.retry_config,
            &(
                self.client.clone(),
                account_id.clone(),
                worker_id.clone(),
                value
            ),
            |(client, account_id, worker_id, value)| {
                Box::pin(async move {
                    let response = client
                        .call("update-worker-limit", move |client| {
                            let request = UpdateWorkerLimitRequest {
                                account_id: Some(account_id.clone().into()),
                                worker_id: Some(worker_id.clone().into()),
                                value: *value,
                            };

                            Box::pin(client.update_worker_limit(request))
                        })
                        .await?
                        .into_inner();

                    match response.result {
                        None => Err("Empty response".to_string().into()),
                        Some(update_worker_limit_response::Result::Success(_)) => Ok(()),
                        Some(update_worker_limit_response::Result::Error(error)) => {
                            Err(error.into())
                        }
                    }
                })
            },
            RegistryServiceClientError::is_retriable,
        )
        .await;

        result.map_err(|e| e.into())
    }

    async fn update_worker_connection_limit(
        &self,
        account_id: &AccountId,
        worker_id: &WorkerId,
        value: i32,
    ) -> Result<(), RegistryServiceError> {
        let result: Result<(), RegistryServiceClientError> = with_retries(
            "limit",
            "update-worker-connection-limit",
            Some(format!("{account_id} - {worker_id}")),
            &self.retry_config,
            &(
                self.client.clone(),
                account_id.clone(),
                worker_id.clone(),
                value
            ),
            |(client, account_id, worker_id, value)| {
                Box::pin(async move {
                    let response = client
                        .call("update-worker-connection-limit", move |client| {
                            let request = UpdateWorkerLimitRequest {
                                account_id: Some(account_id.clone().into()),
                                worker_id: Some(worker_id.clone().into()),
                                value: *value,
                            };

                            Box::pin(client.update_worker_connection_limit(request))
                        })
                        .await?
                        .into_inner();

                    match response.result {
                        None => Err("Empty response".to_string().into()),
                        Some(update_worker_limit_response::Result::Success(_)) => Ok(()),
                        Some(update_worker_limit_response::Result::Error(error)) => {
                            Err(error.into())
                        }
                    }
                })
            },
            RegistryServiceClientError::is_retriable,
        )
        .await;

        result.map_err(|e| e.into())
    }

    async fn get_plugin_registration_by_id(
        &self,
        plugin_id: &PluginRegistrationId,
    ) -> Result<PluginRegistration, RegistryServiceError> {
        let result: Result<PluginRegistration, RegistryServiceClientError> = with_retries(
            "plugins",
            "get-plugin-registration-by-id",
            Some(format!("{plugin_id}")),
            &self.retry_config,
            &(
                self.client.clone(),
                plugin_id.clone()
            ),
            |(client, plugin_id)| {
                Box::pin(async move {
                    let response = client
                        .call("get-plugin-registration-by-id", move |client| {
                            let request = GetPluginRegistrationByIdRequest {
                                id: Some(plugin_id.clone().into())
                            };

                            Box::pin(client.get_plugin_registration_by_id(request))
                        })
                        .await?
                        .into_inner();

                    match response.result {
                        None => Err(RegistryServiceClientError::empty_response()),
                        Some(get_plugin_registration_by_id_response::Result::Success(payload)) => {
                            Ok(payload.plugin.ok_or("missing plugin field".to_string())?.try_into()?)
                        },
                        Some(get_plugin_registration_by_id_response::Result::Error(error)) => {
                            Err(error.into())
                        }
                    }
                })
            },
            RegistryServiceClientError::is_retriable,
        )
        .await;

        result.map_err(|e| e.into())
    }

    async fn resolve_component_by_name(
        &self,
        environment_id: &EnvironmentId,
        component_name: &str,
    ) -> Result<Option<ComponentDto>, RegistryServiceError> {
        let result: Result<Option<ComponentDto>, RegistryServiceClientError> = with_retries(
            "components",
            "resolve-component-by-name",
            Some(format!("{environment_id} - {component_name}")),
            &self.retry_config,
            &(
                self.client.clone(),
                environment_id.clone(),
                component_name.to_string(),
            ),
            |(client, environment_id, component_name)| {
                Box::pin(async move {
                    let response = client
                        .call("resolve-component-by-name", move |client| {
                            let request = GetComponentsRequest {
                                environment_id: Some(environment_id.clone().into()),
                                component_name: Some(component_name.clone()),
                                auth_ctx: Some(AuthCtx::System.into()),
                            };

                            Box::pin(client.get_components(request))
                        })
                        .await?
                        .into_inner();

                    match response.result {
                        None => Err(RegistryServiceClientError::empty_response()),
                        Some(get_components_response::Result::Success(payload)) => {
                            let converted = payload
                                .components
                                .into_iter()
                                .next()
                                .map(|c| ComponentDto::try_from(c))
                                .transpose()?;
                            Ok(converted)
                        },
                        Some(get_components_response::Result::Error(error)) => {
                            Err(error.into())
                        }
                    }
                })
            },
            RegistryServiceClientError::is_retriable,
        )
        .await;

        result.map_err(|e| e.into())
    }

    async fn download_component(
        &self,
        component_id: &ComponentId,
        component_revision: ComponentRevision,
    ) -> Result<Vec<u8>, RegistryServiceError> {
        let result: Result<Vec<u8>, RegistryServiceClientError> = with_retries(
            "components",
            "download-component",
            Some(format!("{component_id} - {component_revision}")),
            &self.retry_config,
            &(
                self.client.clone(),
                component_id.clone(),
                component_revision,
            ),
            |(client, component_id, component_revision)| {
                Box::pin(async move {
                    let mut response = client
                        .call("download-component", move |client| {
                            let request = DownloadComponentRequest {
                                component_id: Some(component_id.clone().into()),
                                version: Some(component_revision.0),
                                auth_ctx: Some(AuthCtx::System.into()),
                            };

                            Box::pin(client.download_component(request))
                        })
                        .await?
                        .into_inner();

                    let mut bytes = Vec::new();

                    while let Some(message) = response.message().await? {
                        match message.result {
                            None => return Err(RegistryServiceClientError::empty_response()),
                            Some(download_component_response::Result::SuccessChunk(chunk)) => bytes.extend_from_slice(&chunk),
                            Some(download_component_response::Result::Error(error)) => Err(error)?
                        };
                    };

                    Ok(bytes)
                })
            },
            RegistryServiceClientError::is_retriable,
        )
        .await;

        result.map_err(|e| e.into())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum RegistryServiceError {
    #[error("BadRequest: {0:?}")]
    BadRequest(Vec<String>),
    #[error("Unauthorized: {0}")]
    Unauthorized(String),
    #[error("Forbidden: {0}")]
    LimitExceeded(String),
    #[error("Forbidden: {0}")]
    Forbidden(String),
    #[error("NotFound: {0}")]
    NotFound(String),
    #[error("AlreadyExists: {0}")]
    AlreadyExists(String),
    #[error("Internal error: {0}")]
    InternalError(String),
    #[error("Internal client error: {0}")]
    InternalClientError(String),
}

impl RegistryServiceError {
    pub fn internal_client_error(error: impl AsRef<str>) -> Self {
        Self::InternalClientError(error.as_ref().to_string())
    }
}

// impl SafeDisplay for AuthServiceError {
//     fn to_safe_string(&self) -> String {
//         match self {
//             AuthServiceError::Unauthorized(_) => self.to_string(),
//             AuthServiceError::Forbidden(_) => self.to_string(),
//             AuthServiceError::InternalClientError(_) => self.to_string(),
//         }
//     }
// }

// impl From<AuthServiceError> for golem_api_grpc::proto::golem::registry::v1::RegistryServiceError {
//     fn from(value: AuthServiceError) -> Self {
//         let error = match value {
//             AuthServiceError::Unauthorized(_) => worker_error::Error::Unauthorized(ErrorBody {
//                 error: value.to_string(),
//             }),
//             AuthServiceError::Forbidden(_) => worker_error::Error::Unauthorized(ErrorBody {
//                 error: value.to_string(),
//             }),
//             // TODO: this used to be unauthorized. How do we handle internal server errors?
//             AuthServiceError::InternalClientError(message) => {
//                 worker_error::Error::InternalError(WorkerExecutionError {
//                     error: Some(worker_execution_error::Error::Unknown(UnknownError {
//                         details: message,
//                     })),
//                 })
//             }
//         };
//         golem_api_grpc::proto::golem::worker::v1::WorkerError { error: Some(error) }
//     }
// }

#[derive(Debug)]
enum RegistryServiceClientError {
    Server(golem_api_grpc::proto::golem::registry::v1::RegistryServiceError),
    Connection(Status),
    Transport(tonic::transport::Error),
    Custom(String)
}

impl RegistryServiceClientError {
    fn empty_response() -> Self {
        Self::Custom("empty response".to_string())
    }

    fn is_retriable(error: &RegistryServiceClientError) -> bool {
        matches!(
            error,
            RegistryServiceClientError::Connection(_) | RegistryServiceClientError::Transport(_)
        )
    }
}

impl Display for RegistryServiceClientError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        use golem_api_grpc::proto::golem::registry::v1::registry_service_error::Error;

        match &self {
            Self::Server(err) => match &err.error {
                Some(Error::LimitExceeded(error)) => {
                    write!(f, "limit exceeded: {}", error.error)
                }
                Some(Error::NotFound(error)) => {
                    write!(f, "not found: {}", error.error)
                }
                Some(Error::AlreadyExists(error)) => {
                    write!(f, "already exists: {}", error.error)
                }
                Some(Error::BadRequest(errors)) => {
                    write!(f, "Invalid request: {:?}", errors.errors)
                }
                Some(Error::InternalError(error)) => {
                    write!(f, "Internal server error: {}", error.error)
                }
                Some(Error::Unauthorized(error)) => write!(f, "Unauthorized: {}", error.error),
                None => write!(f, "Unknown error"),
            },
            Self::Connection(status) => write!(f, "Connection error: {status}"),
            Self::Transport(error) => write!(f, "Transport error: {error}"),
            Self::Custom(error) => write!(f, "Internal client error: {error}")
        }
    }
}

impl std::error::Error for RegistryServiceClientError {}

impl From<String> for RegistryServiceClientError {
    fn from(value: String) -> Self {
        Self::Custom(value)
    }
}

impl From<golem_api_grpc::proto::golem::registry::v1::RegistryServiceError> for RegistryServiceClientError {
    fn from(value: golem_api_grpc::proto::golem::registry::v1::RegistryServiceError) -> Self {
        Self::Server(value)
    }
}

impl From<Status> for RegistryServiceClientError {
    fn from(value: Status) -> Self {
        Self::Connection(value)
    }
}

impl From<tonic::transport::Error> for RegistryServiceClientError {
    fn from(value: tonic::transport::Error) -> Self {
        Self::Transport(value)
    }
}

impl From<RegistryServiceClientError> for RegistryServiceError {
    fn from(value: RegistryServiceClientError) -> Self {
        use golem_api_grpc::proto::golem::registry::v1::registry_service_error::Error;

        match value {
            RegistryServiceClientError::Server(err) => match err.error {
                Some(Error::LimitExceeded(error)) => Self::LimitExceeded(error.error),
                Some(Error::NotFound(error)) => Self::NotFound(error.error),
                Some(Error::AlreadyExists(error)) => Self::AlreadyExists(error.error),
                Some(Error::BadRequest(errors)) => Self::BadRequest(errors.errors),
                Some(Error::InternalError(error)) => Self::InternalError(error.error),
                Some(Error::Unauthorized(error)) => Self::Unauthorized(error.error),
                None => Self::internal_client_error("Unknown error"),
            },
            RegistryServiceClientError::Connection(status) => {
                RegistryServiceError::internal_client_error(format!("Connection error: {status}"))
            }
            RegistryServiceClientError::Transport(error) => {
                RegistryServiceError::internal_client_error(format!("Transport error: {error}"))
            }
            RegistryServiceClientError::Custom(error) => {
                RegistryServiceError::internal_client_error(format!("Internal client error: {error}"))
            }
        }
    }
}
