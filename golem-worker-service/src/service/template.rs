use std::fmt::Display;

use async_trait::async_trait;
use golem_api_grpc::proto::golem::template::template_service_client::TemplateServiceClient;
use golem_api_grpc::proto::golem::template::{
    get_template_metadata_response, GetLatestTemplateRequest, GetVersionedTemplateRequest,
};
use golem_common::config::RetryConfig;
use golem_common::model::TemplateId;
use golem_common::retries::with_retries;
use golem_service_base::model::Template;
use golem_worker_service_base::app_config::TemplateServiceConfig;
use golem_worker_service_base::service::error::TemplateServiceBaseError;
use golem_worker_service_base::UriBackConversion;
use http::Uri;
use tonic::Status;
use tracing::info;

#[async_trait]
pub trait TemplateService {
    async fn get_by_version(
        &self,
        template_id: &TemplateId,
        version: i32,
    ) -> Result<Option<Template>, TemplateServiceBaseError>;

    async fn get_latest(
        &self,
        template_id: &TemplateId,
    ) -> Result<Template, TemplateServiceBaseError>;
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
    async fn get_latest(
        &self,
        template_id: &TemplateId,
    ) -> Result<Template, TemplateServiceBaseError> {
        let desc = format!("Getting latest version of template: {}", template_id);
        info!("{}", &desc);
        with_retries(
            &desc,
            "template",
            "get_latest",
            &self.retry_config,
            &(self.uri.clone(), template_id.clone()),
            |(uri, id)| {
                Box::pin(async move {
                    let mut client = TemplateServiceClient::connect(uri.as_http_02()).await?;
                    let request = GetLatestTemplateRequest {
                        template_id: Some(id.clone().into()),
                    };

                    let response = client
                        .get_latest_template_metadata(request)
                        .await?
                        .into_inner();

                    match response.result {
                        None => Err("Empty response".to_string().into()),
                        Some(get_template_metadata_response::Result::Success(response)) => {
                            let template_view: Result<
                                golem_service_base::model::Template,
                                TemplateServiceBaseError,
                            > = match response.template {
                                Some(template) => {
                                    let template: golem_service_base::model::Template =
                                        template.clone().try_into().map_err(|_| {
                                            TemplateServiceBaseError::Other(
                                                "Response conversion error".to_string(),
                                            )
                                        })?;
                                    Ok(template)
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
            TemplateServiceBaseError::is_retriable,
        )
        .await
    }
    async fn get_by_version(
        &self,
        template_id: &TemplateId,
        version: i32,
    ) -> Result<Option<Template>, TemplateServiceBaseError> {
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
                                TemplateServiceBaseError,
                            > = match response.template {
                                Some(template) => {
                                    let template: golem_service_base::model::Template =
                                        template.clone().try_into().map_err(|_| {
                                            TemplateServiceBaseError::Other(
                                                "Response conversion error".to_string(),
                                            )
                                        })?;
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
            TemplateServiceBaseError::is_retriable,
        )
        .await
    }
}

pub struct TemplateServiceNoop {}

#[async_trait]
impl TemplateService for TemplateServiceNoop {
    async fn get_by_version(
        &self,
        _template_id: &TemplateId,
        _version: i32,
    ) -> Result<Option<Template>, TemplateServiceBaseError> {
        Ok(None)
    }

    async fn get_latest(
        &self,
        _template_id: &TemplateId,
    ) -> Result<Template, TemplateServiceBaseError> {
        Err(TemplateServiceBaseError::Other(
            "Not implemented".to_string(),
        ))
    }
}
