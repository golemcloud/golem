use std::fmt::Display;

use async_trait::async_trait;
use cloud_common::model::TokenSecret;
use golem_api_grpc::proto::golem::template::template_service_client::TemplateServiceClient;
use golem_api_grpc::proto::golem::template::{
    get_template_response, get_templates_response, GetTemplateRequest, GetTemplatesRequest,
};
use golem_common::config::RetryConfig;
use golem_common::model::ProjectId;
use golem_common::model::TemplateId;
use golem_common::retries::with_retries;
use http::Uri;
use tonic::Status;
use tracing::info;
use uuid::Uuid;
use golem_service_base::model::Template;
use crate::app_config::TemplateServiceConfig;
use crate::model::TemplateView;
use crate::UriBackConversion;

#[async_trait]
pub trait TemplateService {
    async fn get_template(
        &self,
        template_id: &TemplateId,
        version: i32
    ) -> Result<Vec<TemplateView>, TemplateError>;

    async fn get_templates(
        &self,
        project_id: &ProjectId,
        request_ctx: &TokenSecret,
    ) -> Result<Vec<TemplateView>, TemplateError>;
}

#[derive(Clone)]
pub struct TemplateServiceDefault {
    pub uri: Uri,
    pub access_token: Uuid,
    pub retry_config: RetryConfig,
}

impl TemplateServiceDefault {
    pub fn new(config: &TemplateServiceConfig) -> Self {
        Self {
            uri: config.uri(),
            access_token: config.access_token,
            retry_config: config.retries.clone(),
        }
    }
}

#[async_trait]
impl TemplateService for TemplateServiceDefault {
    async fn get_template(
        &self,
        template_id: &TemplateId,
        version: i32
    ) -> Result<Vec<Template>, TemplateError> {
        let desc = format!("Getting template: {}", template_id);
        info!("{}", &desc);
        with_retries(
            &desc,
            "template",
            "get_template",
            &self.retry_config,
            &(self.uri.clone(), template_id.clone(), token.clone()),
            |(uri, id, access_token)| {
                Box::pin(async move {
                    let mut client = TemplateServiceClient::connect(uri.as_http_02()).await?;
                    let request = GetTemplateRequest {
                        template_id: Some(id.clone().into()),
                    }

                    let response = client.get_template(request).await?.into_inner();

                    match response.result {
                        None => Err("Empty response".to_string().into()),
                        Some(get_template_response::Result::Success(response)) => {
                            let template_views = response
                                .templates
                                .iter()
                                .map(|template| template.clone().try_into())
                                .collect::<Result<Vec<TemplateView>, String>>();
                            Ok(template_views?)
                        }
                        Some(get_template_response::Result::Error(error)) => Err(error.into()),
                    }
                })
            },
            TemplateError::is_retriable,
        )
            .await
    }

    async fn get_templates(
        &self,
        project_id: &ProjectId,
        token: &TokenSecret,
    ) -> Result<Vec<TemplateView>, TemplateError> {
        let desc = format!("Getting templates for project: {}", project_id);
        info!("{}", &desc);
        with_retries(
            &desc,
            "template",
            "get_templates",
            &self.retry_config,
            &(self.uri.clone(), project_id.clone(), token.clone()),
            |(uri, id, token)| {
                Box::pin(async move {
                    let mut client = TemplateServiceClient::connect(uri.as_http_02()).await?;
                    let request = authorised_request(
                        GetTemplatesRequest {
                            project_id: Some(id.clone().into()),
                            template_name: None,
                        },
                        &token.value,
                    );
                    let response = client.get_templates(request).await?.into_inner();

                    match response.result {
                        None => Err("Empty response".to_string().into()),
                        Some(get_templates_response::Result::Success(response)) => {
                            let template_views = response
                                .templates
                                .iter()
                                .map(|template| template.clone().try_into())
                                .collect::<Result<Vec<TemplateView>, String>>();
                            Ok(template_views?)
                        }
                        Some(get_templates_response::Result::Error(error)) => Err(error.into()),
                    }
                })
            },
            TemplateError::is_retriable,
        )
            .await
    }
}

#[derive(Debug)]
pub enum TemplateError {
    Connection(Status),
    Transport(tonic::transport::Error),
    Server(golem_api_grpc::proto::golem::template::TemplateError),
    Other(String),
}

impl TemplateError {
    fn is_retriable(&self) -> bool {
        matches!(self, TemplateError::Connection(_))
    }
}

impl From<golem_api_grpc::proto::golem::template::TemplateError> for TemplateError {
    fn from(value: golem_api_grpc::proto::golem::template::TemplateError) -> Self {
        TemplateError::Server(value)
    }
}

impl From<Status> for TemplateError {
    fn from(value: Status) -> Self {
        TemplateError::Connection(value)
    }
}

impl From<tonic::transport::Error> for TemplateError {
    fn from(value: tonic::transport::Error) -> Self {
        TemplateError::Transport(value)
    }
}

impl From<String> for TemplateError {
    fn from(value: String) -> Self {
        TemplateError::Other(value)
    }
}

impl Display for TemplateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TemplateError::Server(err) => match &err.error {
                Some(
                    golem_api_grpc::proto::golem::template::template_error::Error::BadRequest(
                        errors,
                    ),
                ) => {
                    write!(f, "Invalid request: {:?}", errors.errors)
                }
                Some(
                    golem_api_grpc::proto::golem::template::template_error::Error::InternalError(
                        error,
                    ),
                ) => {
                    write!(f, "Internal server error: {}", error.error)
                }
                Some(golem_api_grpc::proto::golem::template::template_error::Error::NotFound(
                         error,
                     )) => {
                    write!(f, "Template not found: {}", error.error)
                }
                Some(
                    golem_api_grpc::proto::golem::template::template_error::Error::Unauthorized(
                        error,
                    ),
                ) => {
                    write!(f, "Unauthorized: {}", error.error)
                }
                Some(
                    golem_api_grpc::proto::golem::template::template_error::Error::LimitExceeded(
                        error,
                    ),
                ) => {
                    write!(f, "Template limit reached: {}", error.error)
                }
                Some(
                    golem_api_grpc::proto::golem::template::template_error::Error::AlreadyExists(
                        error,
                    ),
                ) => {
                    write!(f, "Template already exists: {}", error.error)
                }
                None => write!(f, "Empty error response"),
            },
            TemplateError::Connection(status) => write!(f, "Connection error: {status}"),
            TemplateError::Transport(error) => write!(f, "Transport error: {error}"),
            TemplateError::Other(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for TemplateError {
    // TODO
    // fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
    //     Some(&self.source)
    // }
}
