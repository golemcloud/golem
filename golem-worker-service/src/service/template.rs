use std::fmt::Display;

use crate::app_config::TemplateServiceConfig;
use crate::UriBackConversion;
use async_trait::async_trait;
use golem_api_grpc::proto::golem::template::template_service_client::TemplateServiceClient;
use golem_api_grpc::proto::golem::template::{
    get_latest_template_version_response, get_template_metadata_response,
    GetLatestTemplateVersionRequest, GetVersionedTemplateRequest,
};
use golem_common::config::RetryConfig;
use golem_common::model::TemplateId;
use golem_common::retries::with_retries;
use golem_service_base::model::Template;
use http::Uri;
use tonic::Status;
use tracing::info;

#[async_trait]
pub trait TemplateService {
    async fn get_by_version(
        &self,
        template_id: &TemplateId,
        version: i32,
    ) -> Result<Option<Template>, TemplateError>;

    async fn get_latest_version(&self, template_id: &TemplateId) -> Result<i32, TemplateError>;
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
    async fn get_latest_version(&self, template_id: &TemplateId) -> Result<i32, TemplateError> {
        let desc = format!("Getting latest version of template: {}", template_id);
        info!("{}", &desc);
        with_retries(
            &desc,
            "template",
            "get_latest_version",
            &self.retry_config,
            &(self.uri.clone(), template_id.clone()),
            |(uri, id)| {
                Box::pin(async move {
                    let mut client = TemplateServiceClient::connect(uri.as_http_02()).await?;
                    let request = GetLatestTemplateVersionRequest {
                        template_id: Some(id.clone().into()),
                    };

                    let response = client
                        .get_latest_template_version(request)
                        .await?
                        .into_inner();

                    match response.result {
                        None => Err("Empty response".to_string().into()),
                        Some(get_latest_template_version_response::Result::Success(response)) => {
                            Ok(response)
                        }
                        Some(get_latest_template_version_response::Result::Error(error)) => {
                            Err(error.into())
                        }
                    }
                })
            },
            TemplateError::is_retriable,
        )
        .await
    }
    async fn get_by_version(
        &self,
        template_id: &TemplateId,
        version: i32,
    ) -> Result<Option<Template>, TemplateError> {
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

                    let response = client.get_template_metadata(request).await?.into_inner();

                    match response.result {
                        None => Err("Empty response".to_string().into()),
                        Some(get_template_metadata_response::Result::Success(response)) => {
                            let template_view: Result<
                                Option<golem_service_base::model::Template>,
                                TemplateError,
                            > = match response.template {
                                Some(template) => {
                                    let template: golem_service_base::model::Template =
                                        template.clone().try_into().unwrap();
                                    Ok(Some(template))
                                }
                                None => Err("Empty response".to_string().into()),
                            };
                            Ok(template_view?)
                        }
                        Some(get_template_metadata_response::Result::Error(error)) => {
                            Err(error.into())
                        }
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
