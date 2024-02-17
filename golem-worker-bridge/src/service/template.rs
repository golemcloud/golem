use std::fmt::Display;

use async_trait::async_trait;
use golem_api_grpc::proto::golem::template::template_service_client::TemplateServiceClient;
use golem_api_grpc::proto::golem::template::{get_versioned_template_response, GetVersionedTemplateRequest};
use golem_common::config::RetryConfig;
use golem_common::model::TemplateId;
use golem_common::retries::with_retries;
use http::Uri;
use tonic::Status;
use tracing::info;
use golem_service_base::model::Template;
use crate::app_config::TemplateServiceConfig;
use crate::UriBackConversion;

#[async_trait]
pub trait TemplateService {
    async fn get_versioned_template(
        &self,
        template_id: &TemplateId,
        version: i32
    ) -> Result<Option<Template>, TemplateError>;
}

#[derive(Clone)]
pub struct TemplateServiceDefault {
    pub uri: Uri,
    pub retry_config: RetryConfig,
}

impl TemplateServiceDefault {
    pub fn new(config: &TemplateServiceConfig) -> Self {
        Self {
            uri: config.uri(),
            retry_config: config.retries.clone(),
        }
    }
}

#[async_trait]
impl TemplateService for TemplateServiceDefault {
    async fn get_versioned_template(
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
            &(self.uri.clone(), template_id.clone()),
            |(uri, id)| {
                Box::pin(async move {
                    let mut client = TemplateServiceClient::connect(uri.as_http_02()).await?;
                    let request = GetVersionedTemplateRequest {
                        template_id: Some(id.clone().into()),
                        version,
                    };

                    let response = client.get_versioned_template(request).await?.into_inner();

                    match response.result {
                        None => Err("Empty response".to_string().into()),
                        Some(get_versioned_template_response::Result::Success(response)) => {
                            let template_views = match response.template {
                                Some(template) => {
                                    let template = template.clone().try_into();
                                    Ok(template?)
                                }
                                None => Err("Empty response".to_string().into()),
                            };
                            Ok(template_views?)
                        }
                        Some(get_versioned_template_response::Result::Error(error)) => Err(error.into()),
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
