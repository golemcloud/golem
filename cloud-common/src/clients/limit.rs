use crate::clients::auth::authorised_request;
use crate::config::RemoteCloudServiceConfig;
use async_trait::async_trait;
use cloud_api_grpc::proto::golem::cloud::limit::v1::cloud_limits_service_client::CloudLimitsServiceClient;
use cloud_api_grpc::proto::golem::cloud::limit::v1::limits_error::Error;
use cloud_api_grpc::proto::golem::cloud::limit::v1::{
    get_resource_limits_response, update_component_limit_response, update_worker_limit_response,
    GetResourceLimitsRequest, UpdateComponentLimitRequest, UpdateWorkerLimitRequest,
};
use golem_api_grpc::proto::golem::common::ResourceLimits;
use golem_common::client::{GrpcClient, GrpcClientConfig};
use golem_common::model::RetryConfig;
use golem_common::model::{AccountId, ComponentId, WorkerId};
use golem_common::retries::with_retries;
use golem_common::SafeDisplay;
use std::fmt::Display;
use tonic::codec::CompressionEncoding;
use tonic::transport::Channel;
use tonic::Status;
use tracing::info;
use uuid::Uuid;

#[async_trait]
pub trait LimitService {
    async fn update_component_limit(
        &self,
        account_id: &AccountId,
        component_id: &ComponentId,
        count: i32,
        size: i64,
    ) -> Result<(), LimitError>;

    async fn update_worker_limit(
        &self,
        account_id: &AccountId,
        worker_id: &WorkerId,
        value: i32,
    ) -> Result<(), LimitError>;

    async fn update_worker_connection_limit(
        &self,
        account_id: &AccountId,
        worker_id: &WorkerId,
        value: i32,
    ) -> Result<(), LimitError>;

    async fn get_resource_limits(
        &self,
        account_id: &AccountId,
    ) -> Result<ResourceLimits, LimitError>;
}

pub struct LimitServiceDefault {
    limit_service_client: GrpcClient<CloudLimitsServiceClient<Channel>>,
    access_token: Uuid,
    retry_config: RetryConfig,
}

impl LimitServiceDefault {
    pub fn new(config: &RemoteCloudServiceConfig) -> Self {
        let limit_service_client: GrpcClient<CloudLimitsServiceClient<Channel>> = GrpcClient::new(
            "limit",
            |channel| {
                CloudLimitsServiceClient::new(channel)
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
            limit_service_client,
            access_token: config.access_token,
            retry_config: config.retries.clone(),
        }
    }
}

#[async_trait]
impl LimitService for LimitServiceDefault {
    async fn update_component_limit(
        &self,
        account_id: &AccountId,
        component_id: &ComponentId,
        count: i32,
        size: i64,
    ) -> Result<(), LimitError> {
        let result: Result<(), LimitClientError> = with_retries(
            "limit",
            "update-component-limit",
            Some(format!("{account_id} - {component_id}")),
            &self.retry_config,
            &(
                self.limit_service_client.clone(),
                account_id.clone(),
                component_id.clone(),
                count,
                size,
                self.access_token,
            ),
            |(client, account_id, component_id, count, size, token)| {
                Box::pin(async move {
                    let response = client
                        .call("update-component-limit", move |client| {
                            let request = authorised_request(
                                UpdateComponentLimitRequest {
                                    account_id: Some(account_id.clone().into()),
                                    component_id: Some(component_id.clone().into()),
                                    count: *count,
                                    size: *size,
                                },
                                token,
                            );

                            Box::pin(client.update_component_limit(request))
                        })
                        .await?
                        .into_inner();
                    match response.result {
                        None => Err("Empty response".to_string().into()),
                        Some(update_component_limit_response::Result::Success(_)) => Ok(()),
                        Some(update_component_limit_response::Result::Error(error)) => {
                            Err(error.into())
                        }
                    }
                })
            },
            LimitClientError::is_retriable,
        )
        .await;

        result.map_err(|e| e.into())
    }

    async fn update_worker_limit(
        &self,
        account_id: &AccountId,
        worker_id: &WorkerId,
        value: i32,
    ) -> Result<(), LimitError> {
        let result: Result<(), LimitClientError> = with_retries(
            "limit",
            "update-worker-limit",
            Some(format!("{account_id} - {worker_id}")),
            &self.retry_config,
            &(
                self.limit_service_client.clone(),
                account_id.clone(),
                worker_id.clone(),
                value,
                self.access_token,
            ),
            |(client, account_id, worker_id, value, token)| {
                Box::pin(async move {
                    let response = client
                        .call("update-worker-limit", move |client| {
                            let request = authorised_request(
                                UpdateWorkerLimitRequest {
                                    account_id: Some(account_id.clone().into()),
                                    worker_id: Some(worker_id.clone().into()),
                                    value: *value,
                                },
                                token,
                            );

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
            LimitClientError::is_retriable,
        )
        .await;

        result.map_err(|e| e.into())
    }

    async fn update_worker_connection_limit(
        &self,
        account_id: &AccountId,
        worker_id: &WorkerId,
        value: i32,
    ) -> Result<(), LimitError> {
        let result: Result<(), LimitClientError> = with_retries(
            "limit",
            "update-worker-connection-limit",
            Some(format!("{account_id} - {worker_id}")),
            &self.retry_config,
            &(
                self.limit_service_client.clone(),
                account_id.clone(),
                worker_id.clone(),
                value,
                self.access_token,
            ),
            |(client, account_id, worker_id, value, token)| {
                Box::pin(async move {
                    let response = client
                        .call("update-worker-connection-limit", move |client| {
                            let request = authorised_request(
                                UpdateWorkerLimitRequest {
                                    account_id: Some(account_id.clone().into()),
                                    worker_id: Some(worker_id.clone().into()),
                                    value: *value,
                                },
                                token,
                            );

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
            LimitClientError::is_retriable,
        )
        .await;

        result.map_err(|e| e.into())
    }

    async fn get_resource_limits(
        &self,
        account_id: &AccountId,
    ) -> Result<ResourceLimits, LimitError> {
        let desc = format!("Getting resource limits - account: {}", account_id);
        info!("{}", &desc);
        let result: Result<ResourceLimits, LimitClientError> = with_retries(
            "limit",
            "get-resource-limits",
            Some(account_id.to_string()),
            &self.retry_config,
            &(
                self.limit_service_client.clone(),
                account_id.clone(),
                self.access_token,
            ),
            |(client, id, token)| {
                Box::pin(async move {
                    let response = client
                        .call("get-resource-limits", move |client| {
                            let request = authorised_request(
                                GetResourceLimitsRequest {
                                    account_id: Some(id.clone().into()),
                                },
                                token,
                            );

                            Box::pin(client.get_resource_limits(request))
                        })
                        .await?
                        .into_inner();

                    match response.result {
                        None => Err("Empty response".to_string().into()),
                        Some(get_resource_limits_response::Result::Success(response)) => {
                            Ok(response)
                        }
                        Some(get_resource_limits_response::Result::Error(error)) => {
                            Err(error.into())
                        }
                    }
                })
            },
            LimitClientError::is_retriable,
        )
        .await;

        result.map_err(|e| e.into())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum LimitError {
    #[error("Unauthorized: {0}")]
    Unauthorized(String),
    #[error("Limit Exceeded: {0}")]
    LimitExceeded(String),
    #[error("Internal error: {0}")]
    InternalClientError(String),
}

impl LimitError {
    pub fn internal_client_error(error: impl AsRef<str>) -> Self {
        Self::InternalClientError(error.as_ref().to_string())
    }
}

impl SafeDisplay for LimitError {
    fn to_safe_string(&self) -> String {
        match self {
            LimitError::Unauthorized(_) => self.to_string(),
            LimitError::LimitExceeded(_) => self.to_string(),
            LimitError::InternalClientError(_) => self.to_string(),
        }
    }
}

#[derive(Debug)]
pub enum LimitClientError {
    Server(cloud_api_grpc::proto::golem::cloud::limit::v1::LimitsError),
    Connection(Status),
    Transport(tonic::transport::Error),
    Unknown(String),
}

impl From<LimitClientError> for LimitError {
    fn from(value: LimitClientError) -> Self {
        match value {
            LimitClientError::Server(err) => match err.error {
                Some(Error::BadRequest(errors)) => {
                    LimitError::internal_client_error(errors.errors.join(", "))
                }
                Some(Error::InternalError(error)) => LimitError::internal_client_error(error.error),
                Some(Error::Unauthorized(error)) => LimitError::Unauthorized(error.error),
                Some(Error::LimitExceeded(error)) => LimitError::LimitExceeded(error.error),
                None => LimitError::internal_client_error("Unknown error"),
            },
            LimitClientError::Connection(status) => {
                LimitError::internal_client_error(format!("Connection error: {status}"))
            }
            LimitClientError::Transport(error) => {
                LimitError::internal_client_error(format!("Transport error: {error}"))
            }
            LimitClientError::Unknown(error) => {
                LimitError::internal_client_error(format!("Unknown error: {error}"))
            }
        }
    }
}

impl From<cloud_api_grpc::proto::golem::cloud::limit::v1::LimitsError> for LimitClientError {
    fn from(value: cloud_api_grpc::proto::golem::cloud::limit::v1::LimitsError) -> Self {
        Self::Server(value)
    }
}

impl From<Status> for LimitClientError {
    fn from(value: Status) -> Self {
        Self::Connection(value)
    }
}

impl From<tonic::transport::Error> for LimitClientError {
    fn from(value: tonic::transport::Error) -> Self {
        Self::Transport(value)
    }
}

impl From<String> for LimitClientError {
    fn from(value: String) -> Self {
        Self::Unknown(value)
    }
}

impl LimitClientError {
    fn is_retriable(error: &LimitClientError) -> bool {
        matches!(
            error,
            LimitClientError::Connection(_) | LimitClientError::Transport(_)
        )
    }
}

impl Display for LimitClientError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self {
            LimitClientError::Server(err) => match &err.error {
                Some(Error::BadRequest(errors)) => {
                    write!(f, "Invalid request: {:?}", errors.errors)
                }
                Some(Error::InternalError(error)) => {
                    write!(f, "Internal server error: {}", error.error)
                }
                Some(Error::Unauthorized(error)) => write!(f, "Unauthorized: {}", error.error),
                Some(Error::LimitExceeded(error)) => {
                    write!(f, " Limit reached: {}", error.error)
                }
                None => write!(f, "Unknown error"),
            },
            LimitClientError::Connection(status) => write!(f, "Connection error: {status}"),
            LimitClientError::Transport(error) => write!(f, "Transport error: {error}"),
            LimitClientError::Unknown(error) => write!(f, "Unknown error: {error}"),
        }
    }
}

impl std::error::Error for LimitClientError {}
