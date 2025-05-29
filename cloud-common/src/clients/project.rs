use std::fmt::Display;

use crate::clients::auth::authorised_request;
use crate::config::RemoteCloudServiceConfig;
use crate::model::{ProjectView, TokenSecret};
use async_trait::async_trait;
use cloud_api_grpc::proto::golem::cloud::project::v1::cloud_project_service_client::CloudProjectServiceClient;
use cloud_api_grpc::proto::golem::cloud::project::v1::project_error::Error;
use cloud_api_grpc::proto::golem::cloud::project::v1::{
    get_default_project_response, get_project_response, GetDefaultProjectRequest, GetProjectRequest,
};
use golem_common::client::{GrpcClient, GrpcClientConfig};
use golem_common::model::ProjectId;
use golem_common::model::RetryConfig;
use golem_common::retries::with_retries;
use golem_common::SafeDisplay;
use tonic::codec::CompressionEncoding;
use tonic::transport::Channel;
use tonic::Status;

#[async_trait]
pub trait ProjectService {
    async fn get(
        &self,
        project_id: &ProjectId,
        token: &TokenSecret,
    ) -> Result<ProjectView, ProjectError>;

    async fn get_default(&self, token: &TokenSecret) -> Result<ProjectView, ProjectError>;
}

#[derive(Clone)]
pub struct ProjectServiceDefault {
    project_service_client: GrpcClient<CloudProjectServiceClient<Channel>>,
    retry_config: RetryConfig,
}

impl ProjectServiceDefault {
    pub fn new(config: &RemoteCloudServiceConfig) -> Self {
        let project_service_client: GrpcClient<CloudProjectServiceClient<Channel>> =
            GrpcClient::new(
                "project",
                |channel| {
                    CloudProjectServiceClient::new(channel)
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
            project_service_client,
            retry_config: config.retries.clone(),
        }
    }
}

#[async_trait]
impl ProjectService for ProjectServiceDefault {
    async fn get(
        &self,
        project_id: &ProjectId,
        token: &TokenSecret,
    ) -> Result<ProjectView, ProjectError> {
        with_retries(
            "project",
            "get",
            Some(format!("{project_id}")),
            &self.retry_config,
            &(
                self.project_service_client.clone(),
                project_id.clone(),
                token.clone(),
            ),
            |(client, id, token)| {
                Box::pin(async move {
                    let response = client
                        .call("get-project", move |client| {
                            let request = authorised_request(
                                GetProjectRequest {
                                    project_id: Some(id.clone().into()),
                                },
                                &token.value,
                            );

                            Box::pin(client.get_project(request))
                        })
                        .await?
                        .into_inner();

                    match response.result {
                        None => Err("Empty response".to_string().into()),
                        Some(get_project_response::Result::Success(project)) => {
                            Ok(project.try_into()?)
                        }
                        Some(get_project_response::Result::Error(error)) => Err(error.into()),
                    }
                })
            },
            ProjectError::is_retriable,
        )
        .await
    }

    async fn get_default(&self, token: &TokenSecret) -> Result<ProjectView, ProjectError> {
        with_retries(
            "project",
            "get-default",
            None,
            &self.retry_config,
            &(self.project_service_client.clone(), token.clone()),
            |(client, token)| {
                Box::pin(async move {
                    let response = client
                        .call("get-default-project", move |client| {
                            let request =
                                authorised_request(GetDefaultProjectRequest {}, &token.value);
                            Box::pin(client.get_default_project(request))
                        })
                        .await?
                        .into_inner();
                    match response.result {
                        None => Err("Empty response".to_string().into()),
                        Some(get_default_project_response::Result::Success(project)) => {
                            Ok(project.try_into()?)
                        }
                        Some(get_default_project_response::Result::Error(error)) => {
                            Err(error.into())
                        }
                    }
                })
            },
            ProjectError::is_retriable,
        )
        .await
    }
}

#[derive(Debug)]
pub enum ProjectError {
    Server(cloud_api_grpc::proto::golem::cloud::project::v1::ProjectError),
    Connection(Status),
    Transport(tonic::transport::Error),
    Unknown(String),
}

impl From<cloud_api_grpc::proto::golem::cloud::project::v1::ProjectError> for ProjectError {
    fn from(value: cloud_api_grpc::proto::golem::cloud::project::v1::ProjectError) -> Self {
        Self::Server(value)
    }
}

impl From<Status> for ProjectError {
    fn from(value: Status) -> Self {
        Self::Connection(value)
    }
}

impl From<tonic::transport::Error> for ProjectError {
    fn from(value: tonic::transport::Error) -> Self {
        Self::Transport(value)
    }
}

impl From<String> for ProjectError {
    fn from(value: String) -> Self {
        Self::Unknown(value)
    }
}

impl ProjectError {
    fn is_retriable(error: &ProjectError) -> bool {
        matches!(
            error,
            ProjectError::Connection(_) | ProjectError::Transport(_)
        )
    }
}

impl Display for ProjectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self {
            ProjectError::Server(err) => match &err.error {
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
                None => write!(f, "Unknown error"),
            },
            ProjectError::Connection(status) => write!(f, "Connection error: {status}"),
            ProjectError::Transport(error) => write!(f, "Transport error: {error}"),
            ProjectError::Unknown(error) => write!(f, "Unknown error: {error}"),
        }
    }
}

impl SafeDisplay for ProjectError {
    fn to_safe_string(&self) -> String {
        match self {
            ProjectError::Server(_) => self.to_string(),
            ProjectError::Connection(_) => self.to_string(),
            ProjectError::Transport(_) => self.to_string(),
            ProjectError::Unknown(_) => self.to_string(),
        }
    }
}

impl std::error::Error for ProjectError {}
