use std::fmt::Display;

use crate::config::CloudServiceConfig;
use async_trait::async_trait;
use cloud_api_grpc::proto::golem::cloud::project::cloud_project_service_client::CloudProjectServiceClient;
use cloud_api_grpc::proto::golem::cloud::project::project_error::Error;
use cloud_api_grpc::proto::golem::cloud::project::{
    get_project_actions_response, get_project_response, GetProjectActionsRequest, GetProjectRequest,
};
use cloud_common::model::{ProjectActions, ProjectAuthorisedActions, TokenSecret};
use golem_common::config::RetryConfig;
use golem_common::model::AccountId;
use golem_common::model::ProjectId;
use golem_common::retries::with_retries;
use http::Uri;
use tonic::Status;

use crate::model::ProjectView;
use crate::service::auth::authorised_request;
use crate::UriBackConversion;

#[async_trait]
pub trait ProjectService {
    async fn get(
        &self,
        project_id: &ProjectId,
        token: &TokenSecret,
    ) -> Result<ProjectView, ProjectError>;

    async fn get_actions(
        &self,
        project_id: &ProjectId,
        token: &TokenSecret,
    ) -> Result<ProjectAuthorisedActions, ProjectError>;
}

#[derive(Clone)]
pub struct ProjectServiceDefault {
    uri: Uri,
    retry_config: RetryConfig,
}

impl ProjectServiceDefault {
    pub fn new(config: &CloudServiceConfig) -> Self {
        Self {
            uri: config.uri(),
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
            Some(project_id.to_string()),
            &self.retry_config,
            &(self.uri.clone(), project_id.clone(), token.clone()),
            |(uri, id, token)| {
                Box::pin(async move {
                    let mut client = CloudProjectServiceClient::connect(uri.as_http_02()).await?;
                    let request = authorised_request(
                        GetProjectRequest {
                            project_id: Some(id.clone().into()),
                        },
                        &token.value,
                    );
                    let response = client.get_project(request).await?.into_inner();

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

    async fn get_actions(
        &self,
        project_id: &ProjectId,
        token: &TokenSecret,
    ) -> Result<ProjectAuthorisedActions, ProjectError> {
        with_retries(
            "project",
            "get-actions",
            Some(project_id.to_string()),
            &self.retry_config,
            &(self.uri.clone(), project_id.clone(), token.clone()),
            |(uri, id, token)| {
                Box::pin(async move {
                    let mut client = CloudProjectServiceClient::connect(uri.as_http_02()).await?;
                    let request = authorised_request(
                        GetProjectActionsRequest {
                            project_id: Some(id.clone().into()),
                        },
                        &token.value,
                    );

                    let response = client.get_project_actions(request).await?.into_inner();

                    match response.result {
                        None => Err("Empty response".to_string().into()),
                        Some(get_project_actions_response::Result::Success(response)) => {
                            let actions = response.try_into()?;

                            Ok(actions)
                        }
                        Some(get_project_actions_response::Result::Error(error)) => {
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
    Server(cloud_api_grpc::proto::golem::cloud::project::ProjectError),
    Connection(Status),
    Transport(tonic::transport::Error),
    Unknown(String),
}

impl From<cloud_api_grpc::proto::golem::cloud::project::ProjectError> for ProjectError {
    fn from(value: cloud_api_grpc::proto::golem::cloud::project::ProjectError) -> Self {
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

impl std::error::Error for ProjectError {
    // TODO
    // fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
    //     Some(&self.source)
    // }
}

pub struct ProjectServiceNoop {
    account_id: AccountId,
}

impl Default for ProjectServiceNoop {
    fn default() -> Self {
        Self {
            account_id: AccountId::from("a1"),
        }
    }
}

#[async_trait]
impl ProjectService for ProjectServiceNoop {
    async fn get(
        &self,
        project_id: &ProjectId,
        _: &TokenSecret,
    ) -> Result<ProjectView, ProjectError> {
        Ok(ProjectView {
            id: project_id.clone(),
            owner_account_id: self.account_id.clone(),
            name: "test".to_string(),
            description: "test".to_string(),
        })
    }

    async fn get_actions(
        &self,
        project_id: &ProjectId,
        _: &TokenSecret,
    ) -> Result<ProjectAuthorisedActions, ProjectError> {
        Ok(ProjectAuthorisedActions {
            project_id: project_id.clone(),
            owner_account_id: AccountId::from(""),
            actions: ProjectActions::empty(),
        })
    }
}
