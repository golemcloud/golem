use crate::clients::auth::authorised_request;
use crate::config::RemoteCloudServiceConfig;
use crate::grpc::try_decode_plugin_definition;
use crate::model::{CloudPluginOwner, CloudPluginScope, TokenSecret};
use async_trait::async_trait;
use golem_api_grpc::proto::golem::component::v1::component_error::Error;
use golem_common::client::{GrpcClient, GrpcClientConfig};
use golem_common::model::plugin::PluginDefinition;
use golem_common::model::{PluginId, RetryConfig};
use golem_common::retries::with_retries;
use golem_common::SafeDisplay;
use std::fmt::Display;
use tonic::codec::CompressionEncoding;
use tonic::transport::Channel;
use tonic::Status;

#[async_trait]
pub trait PluginServiceClient {
    async fn get(
        &self,
        name: &str,
        version: &str,
        token: &TokenSecret,
    ) -> Result<Option<PluginDefinition<CloudPluginOwner, CloudPluginScope>>, PluginError>;

    async fn get_by_id(
        &self,
        id: &PluginId,
        token: &TokenSecret,
    ) -> Result<Option<PluginDefinition<CloudPluginOwner, CloudPluginScope>>, PluginError>;
}

#[derive(Clone)]
pub struct PluginServiceClientDefault {
    plugin_service_client: GrpcClient<cloud_api_grpc::proto::golem::cloud::component::v1::plugin_service_client::PluginServiceClient<Channel>>,
    retry_config: RetryConfig,
}

impl PluginServiceClientDefault {
    pub fn new(config: &RemoteCloudServiceConfig) -> Self {
        let plugin_service_client = GrpcClient::new(
            "plugin",
            |channel| {
                cloud_api_grpc::proto::golem::cloud::component::v1::plugin_service_client::PluginServiceClient::new(channel)
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
            plugin_service_client,
            retry_config: config.retries.clone(),
        }
    }
}

#[async_trait]
impl PluginServiceClient for PluginServiceClientDefault {
    async fn get(
        &self,
        name: &str,
        version: &str,
        token: &TokenSecret,
    ) -> Result<Option<PluginDefinition<CloudPluginOwner, CloudPluginScope>>, PluginError> {
        with_retries(
            "plugin",
            "get",
            None,
            &self.retry_config,
            &(self.plugin_service_client.clone(), token.clone(), name.to_string(), version.to_string()),
            |(client, token, name, version)| {
                Box::pin(async move {
                    let response = client
                        .call("get", move |client| {
                            let request = authorised_request(
                                golem_api_grpc::proto::golem::component::v1::GetPluginRequest {
                                    name: name.to_string(),
                                    version: version.to_string(),
                                },
                                &token.value,
                            );
                            Box::pin(client.get_plugin(request))
                        })
                        .await?
                        .into_inner();

                    match response.result {
                        None => Err(PluginError::Unknown("Empty response".to_string())),
                        Some(cloud_api_grpc::proto::golem::cloud::component::v1::get_plugin_response::Result::Success(plugin)) => {
                            if let Some(plugin) = plugin.plugin {
                                Ok(Some(try_decode_plugin_definition(plugin)?))
                            } else {
                                Ok(None)
                            }
                        }
                        Some(cloud_api_grpc::proto::golem::cloud::component::v1::get_plugin_response::Result::Error(error)) => {
                            Err(PluginError::from(error))
                        }
                    }
                })
            },
            PluginError::is_retriable,
        ).await
    }

    async fn get_by_id(
        &self,
        id: &PluginId,
        token: &TokenSecret,
    ) -> Result<Option<PluginDefinition<CloudPluginOwner, CloudPluginScope>>, PluginError> {
        with_retries(
            "plugin",
            "get",
            None,
            &self.retry_config,
            &(self.plugin_service_client.clone(), token.clone(), id.clone()),
            |(client, token, id)| {
                Box::pin(async move {
                    let response = client
                        .call("get_by_id", move |client| {
                            let request = authorised_request(
                                cloud_api_grpc::proto::golem::cloud::component::v1::GetPluginByIdRequest {
                                    id: Some(id.clone().into()),
                                },
                                &token.value,
                            );
                            Box::pin(client.get_plugin_by_id(request))
                        })
                        .await?
                        .into_inner();

                    match response.result {
                        None => Err(PluginError::Unknown("Empty response".to_string())),
                        Some(cloud_api_grpc::proto::golem::cloud::component::v1::get_plugin_by_id_response::Result::Success(plugin)) => {
                            if let Some(plugin) = plugin.plugin {
                                Ok(Some(try_decode_plugin_definition(plugin)?))
                            } else {
                                Ok(None)
                            }
                        }
                        Some(cloud_api_grpc::proto::golem::cloud::component::v1::get_plugin_by_id_response::Result::Error(error)) => {
                            Err(PluginError::from(error))
                        }
                    }
                })
            },
            PluginError::is_retriable,
        ).await
    }
}

#[derive(Debug)]
pub enum PluginError {
    Server(golem_api_grpc::proto::golem::component::v1::ComponentError),
    Connection(Status),
    Transport(tonic::transport::Error),
    Unknown(String),
}

impl From<golem_api_grpc::proto::golem::component::v1::ComponentError> for PluginError {
    fn from(value: golem_api_grpc::proto::golem::component::v1::ComponentError) -> Self {
        Self::Server(value)
    }
}

impl From<Status> for PluginError {
    fn from(value: Status) -> Self {
        Self::Connection(value)
    }
}

impl From<tonic::transport::Error> for PluginError {
    fn from(value: tonic::transport::Error) -> Self {
        Self::Transport(value)
    }
}

impl From<String> for PluginError {
    fn from(value: String) -> Self {
        Self::Unknown(value)
    }
}

impl PluginError {
    fn is_retriable(error: &PluginError) -> bool {
        matches!(
            error,
            PluginError::Connection(_) | PluginError::Transport(_)
        )
    }
}

impl Display for PluginError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self {
            PluginError::Server(err) => match &err.error {
                Some(Error::BadRequest(errors)) => {
                    write!(f, "Invalid request: {:?}", errors.errors)
                }
                Some(Error::InternalError(error)) => {
                    write!(f, "Internal server error: {}", error.error)
                }
                Some(Error::NotFound(error)) => write!(f, "Project not found: {}", error.error),
                Some(Error::Unauthorized(error)) => write!(f, "Unauthorized: {}", error.error),
                Some(Error::LimitExceeded(error)) => {
                    write!(f, "Project limit reached: {}", error.error)
                }
                Some(Error::AlreadyExists(_)) => {
                    write!(f, "Plugin already exists")
                }
                None => write!(f, "Unknown error"),
            },
            PluginError::Connection(status) => write!(f, "Connection error: {status}"),
            PluginError::Transport(error) => write!(f, "Transport error: {error}"),
            PluginError::Unknown(error) => write!(f, "Unknown error: {error}"),
        }
    }
}

impl SafeDisplay for PluginError {
    fn to_safe_string(&self) -> String {
        match self {
            PluginError::Server(_) => self.to_string(),
            PluginError::Connection(_) => self.to_string(),
            PluginError::Transport(_) => self.to_string(),
            PluginError::Unknown(_) => self.to_string(),
        }
    }
}

impl std::error::Error for PluginError {}
