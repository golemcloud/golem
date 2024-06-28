use crate::service::auth::authorised_request;
use async_trait::async_trait;
use golem_common::model::AccountId;
use golem_service_base::model::{ResourceLimits, WorkerId};
use std::fmt::Display;

use crate::config::CloudServiceConfig;
use cloud_api_grpc::proto::golem::cloud::limit::cloud_limits_service_client::CloudLimitsServiceClient;
use cloud_api_grpc::proto::golem::cloud::limit::limits_error::Error;
use cloud_api_grpc::proto::golem::cloud::limit::{
    get_resource_limits_response, update_worker_limit_response, GetResourceLimitsRequest,
    UpdateWorkerLimitRequest,
};
use golem_common::config::RetryConfig;
use golem_common::retries::with_retries;
use http::Uri;
use tonic::Status;
use tracing::info;
use uuid::Uuid;

use crate::UriBackConversion;

#[derive(Debug, thiserror::Error)]
pub enum LimitError {
    #[error("Unauthorized: {0}")]
    Unauthorized(String),
    #[error("Limit Exceeded: {0}")]
    LimitExceeded(String),
    #[error("Internal error: {0}")]
    Internal(#[from] anyhow::Error),
}

impl LimitError {
    pub fn internal<M>(error: M) -> Self
    where
        M: Display,
    {
        Self::Internal(anyhow::Error::msg(error.to_string()))
    }
}

#[derive(Debug)]
pub enum LimitClientError {
    Server(cloud_api_grpc::proto::golem::cloud::limit::LimitsError),
    Connection(Status),
    Transport(tonic::transport::Error),
    Unknown(String),
}

impl From<LimitClientError> for LimitError {
    fn from(value: LimitClientError) -> Self {
        match value {
            LimitClientError::Server(err) => match err.error {
                Some(Error::BadRequest(errors)) => LimitError::internal(errors.errors.join(", ")),
                Some(Error::InternalError(error)) => LimitError::internal(error.error),
                Some(Error::Unauthorized(error)) => LimitError::Unauthorized(error.error),
                Some(Error::LimitExceeded(error)) => LimitError::LimitExceeded(error.error),
                None => LimitError::internal("Unknown error"),
            },
            LimitClientError::Connection(status) => {
                LimitError::internal(format!("Connection error: {status}"))
            }
            LimitClientError::Transport(error) => {
                LimitError::internal(format!("Transport error: {error}"))
            }
            LimitClientError::Unknown(error) => {
                LimitError::internal(format!("Unknown error: {error}"))
            }
        }
    }
}

impl From<cloud_api_grpc::proto::golem::cloud::limit::LimitsError> for LimitClientError {
    fn from(value: cloud_api_grpc::proto::golem::cloud::limit::LimitsError) -> Self {
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

#[async_trait]
pub trait LimitService {
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
    uri: Uri,
    access_token: Uuid,
    retry_config: RetryConfig,
}

impl LimitServiceDefault {
    pub fn new(config: &CloudServiceConfig) -> Self {
        Self {
            uri: config.uri(),
            access_token: config.access_token,
            retry_config: config.retries.clone(),
        }
    }
}

#[async_trait]
impl LimitService for LimitServiceDefault {
    async fn update_worker_limit(
        &self,
        account_id: &AccountId,
        worker_id: &WorkerId,
        value: i32,
    ) -> Result<(), LimitError> {
        let desc = format!(
            "Update worker limit - account: {}, worker: {}",
            account_id, worker_id
        );
        info!("{}", &desc);
        let result: Result<(), LimitClientError> = with_retries(
            &desc,
            "limit",
            "update-worker-limit",
            &self.retry_config,
            &(
                self.uri.clone(),
                account_id.clone(),
                worker_id.clone(),
                value,
                self.access_token,
            ),
            |(uri, account_id, worker_id, value, token)| {
                Box::pin(async move {
                    let mut client = CloudLimitsServiceClient::connect(uri.as_http_02()).await?;
                    let request = authorised_request(
                        UpdateWorkerLimitRequest {
                            account_id: Some(account_id.clone().into()),
                            worker_id: Some(worker_id.clone().into()),
                            value: *value,
                        },
                        token,
                    );

                    let response = client.update_worker_limit(request).await?.into_inner();

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
        let desc = format!(
            "Update worker connection limit - account: {}, worker: {}",
            account_id, worker_id
        );
        info!("{}", &desc);
        let result: Result<(), LimitClientError> = with_retries(
            &desc,
            "limit",
            "update-worker-connection-limit",
            &self.retry_config,
            &(
                self.uri.clone(),
                account_id.clone(),
                worker_id.clone(),
                value,
                self.access_token,
            ),
            |(uri, account_id, worker_id, value, token)| {
                Box::pin(async move {
                    let mut client = CloudLimitsServiceClient::connect(uri.as_http_02()).await?;
                    let request = authorised_request(
                        UpdateWorkerLimitRequest {
                            account_id: Some(account_id.clone().into()),
                            worker_id: Some(worker_id.clone().into()),
                            value: *value,
                        },
                        token,
                    );

                    let response = client
                        .update_worker_connection_limit(request)
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
            &desc,
            "limit",
            "get-resource-limits",
            &self.retry_config,
            &(self.uri.clone(), account_id.clone(), self.access_token),
            |(uri, id, token)| {
                Box::pin(async move {
                    let mut client = CloudLimitsServiceClient::connect(uri.as_http_02()).await?;
                    let request = authorised_request(
                        GetResourceLimitsRequest {
                            account_id: Some(id.clone().into()),
                        },
                        token,
                    );

                    let response = client.get_resource_limits(request).await?.into_inner();

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
            LimitClientError::is_retriable,
        )
        .await;

        result.map_err(|e| e.into())
    }
}

#[derive(Default)]
pub struct LimitServiceNoop {}

#[async_trait]
impl LimitService for LimitServiceNoop {
    async fn update_worker_limit(
        &self,
        _account_id: &AccountId,
        _worker_id: &WorkerId,
        _value: i32,
    ) -> Result<(), LimitError> {
        Ok(())
    }

    async fn update_worker_connection_limit(
        &self,
        _account_id: &AccountId,
        _worker_id: &WorkerId,
        _value: i32,
    ) -> Result<(), LimitError> {
        Ok(())
    }

    async fn get_resource_limits(
        &self,
        _account_id: &AccountId,
    ) -> Result<ResourceLimits, LimitError> {
        Ok(ResourceLimits {
            available_fuel: 1000000000000,
            max_memory_per_worker: 100 * 1024 * 1024,
        })
    }
}
