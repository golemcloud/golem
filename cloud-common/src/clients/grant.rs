use crate::clients::auth::authorised_request;
use crate::config::RemoteCloudServiceConfig;
use crate::model::{Role, TokenSecret};
use async_trait::async_trait;
use cloud_api_grpc::proto::golem::cloud::grant::v1::cloud_grant_service_client::CloudGrantServiceClient;
use cloud_api_grpc::proto::golem::cloud::grant::v1::get_self_grants_response;
use cloud_api_grpc::proto::golem::cloud::grant::v1::grant_error::Error;
use golem_api_grpc::proto::golem::common::Empty;
use golem_common::client::{GrpcClient, GrpcClientConfig};
use golem_common::model::{AccountId, RetryConfig};
use golem_common::retries::with_retries;
use golem_common::SafeDisplay;
use std::fmt::Display;
use tonic::codec::CompressionEncoding;
use tonic::transport::Channel;
use tonic::Status;

pub struct AccountWithRoles {
    pub account_id: AccountId,
    pub roles: Vec<Role>,
}

#[async_trait]
pub trait GrantService {
    async fn get_self_grants(&self, token: &TokenSecret) -> Result<AccountWithRoles, GrantError>;
}

pub struct GrantServiceDefault {
    grant_client: GrpcClient<CloudGrantServiceClient<Channel>>,
    retry_config: RetryConfig,
}

impl GrantServiceDefault {
    pub fn new(config: &RemoteCloudServiceConfig) -> Self {
        let grant_client: GrpcClient<CloudGrantServiceClient<Channel>> = GrpcClient::new(
            "grant",
            |channel| {
                CloudGrantServiceClient::new(channel)
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
            grant_client,
            retry_config: config.retries.clone(),
        }
    }
}

#[async_trait]
impl GrantService for GrantServiceDefault {
    async fn get_self_grants(&self, token: &TokenSecret) -> Result<AccountWithRoles, GrantError> {
        with_retries(
            "grant",
            "get-self-grants",
            None,
            &self.retry_config,
            &(self.grant_client.clone(), token.clone()),
            |(client, token)| {
                Box::pin(async move {
                    let response = client
                        .call("get-self-grants", move |client| {
                            let request = authorised_request(Empty {}, &token.value);

                            Box::pin(client.get_self_grants(request))
                        })
                        .await?
                        .into_inner();
                    match response.result {
                        None => Err("Empty response".to_string().into()),
                        Some(get_self_grants_response::Result::Success(response)) => {
                            Ok(AccountWithRoles {
                                account_id: response
                                    .account_id
                                    .ok_or("Missing account_id in response".to_string())?
                                    .into(),
                                roles: response
                                    .roles
                                    .into_iter()
                                    .map(|r| r.try_into())
                                    .collect::<Result<Vec<_>, _>>()?,
                            })
                        }
                        Some(get_self_grants_response::Result::Error(error)) => Err(error.into()),
                    }
                })
            },
            GrantError::is_retriable,
        )
        .await
    }
}

#[derive(Debug)]
pub enum GrantError {
    Server(cloud_api_grpc::proto::golem::cloud::grant::v1::GrantError),
    Connection(Status),
    Transport(tonic::transport::Error),
    Unknown(String),
}

impl From<cloud_api_grpc::proto::golem::cloud::grant::v1::GrantError> for GrantError {
    fn from(value: cloud_api_grpc::proto::golem::cloud::grant::v1::GrantError) -> Self {
        Self::Server(value)
    }
}

impl From<Status> for GrantError {
    fn from(value: Status) -> Self {
        Self::Connection(value)
    }
}

impl From<tonic::transport::Error> for GrantError {
    fn from(value: tonic::transport::Error) -> Self {
        Self::Transport(value)
    }
}

impl From<String> for GrantError {
    fn from(value: String) -> Self {
        Self::Unknown(value)
    }
}

impl GrantError {
    fn is_retriable(error: &GrantError) -> bool {
        matches!(error, GrantError::Connection(_) | GrantError::Transport(_))
    }
}

impl Display for GrantError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self {
            GrantError::Server(err) => match &err.error {
                Some(Error::BadRequest(errors)) => {
                    write!(f, "Invalid request: {:?}", errors.errors)
                }
                Some(Error::InternalError(error)) => {
                    write!(f, "Internal server error: {}", error.error)
                }
                Some(Error::Unauthorized(error)) => write!(f, "Unauthorized: {}", error.error),
                Some(Error::NotFound(error)) => {
                    write!(f, "Account not found: {}", error.error)
                }
                None => write!(f, "Unknown error"),
            },
            GrantError::Connection(status) => write!(f, "Connection error: {status}"),
            GrantError::Transport(error) => write!(f, "Transport error: {error}"),
            GrantError::Unknown(error) => write!(f, "Unknown error: {error}"),
        }
    }
}

impl SafeDisplay for GrantError {
    fn to_safe_string(&self) -> String {
        match self {
            GrantError::Server(_) => self.to_string(),
            GrantError::Connection(_) => self.to_string(),
            GrantError::Transport(_) => self.to_string(),
            GrantError::Unknown(_) => self.to_string(),
        }
    }
}

impl std::error::Error for GrantError {}
