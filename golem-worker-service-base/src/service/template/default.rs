use crate::service::template::TemplateServiceError;
use crate::UriBackConversion;

use async_trait::async_trait;
use golem_api_grpc::proto::golem::template::template_service_client::TemplateServiceClient;
use golem_api_grpc::proto::golem::template::{
    get_template_metadata_response, GetLatestTemplateRequest, GetVersionedTemplateRequest,
};
use golem_common::config::RetryConfig;
use golem_common::model::TemplateId;
use golem_common::retries::with_retries;
use golem_service_base::model::Template;
use http::Uri;
use tracing::info;

#[async_trait]
pub trait TemplateService {
    async fn get_by_version(
        &self,
        template_id: &TemplateId,
        version: i32,
    ) -> Result<Option<Template>, TemplateServiceError>;

    async fn get_latest(&self, template_id: &TemplateId) -> Result<Template, TemplateServiceError>;
}

#[derive(Clone)]
pub struct TemplateServiceDefault {
    uri: Uri,
    retry_config: RetryConfig,
}

impl TemplateServiceDefault {
    pub fn new(uri: Uri, retry_config: RetryConfig) -> Self {
        Self { uri, retry_config }
    }
}

#[async_trait]
impl TemplateService for TemplateServiceDefault {
    async fn get_latest(&self, template_id: &TemplateId) -> Result<Template, TemplateServiceError> {
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
                        None => Err(TemplateServiceError::internal("Empty response")),
                        Some(get_template_metadata_response::Result::Success(response)) => {
                            let template_view: Result<
                                golem_service_base::model::Template,
                                TemplateServiceError,
                            > = match response.template {
                                Some(template) => {
                                    let template: golem_service_base::model::Template =
                                        template.clone().try_into().map_err(|_| {
                                            TemplateServiceError::internal(
                                                "Response conversion error",
                                            )
                                        })?;
                                    Ok(template)
                                }
                                None => {
                                    Err(TemplateServiceError::internal("Empty template response"))
                                }
                            };
                            Ok(template_view?)
                        }
                        Some(get_template_metadata_response::Result::Error(error)) => {
                            Err(error.into())
                        }
                    }
                })
            },
            TemplateServiceError::is_retriable,
        )
        .await
    }
    async fn get_by_version(
        &self,
        template_id: &TemplateId,
        version: i32,
    ) -> Result<Option<Template>, TemplateServiceError> {
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
                        None => Err(TemplateServiceError::internal("Empty response")),

                        Some(get_template_metadata_response::Result::Success(response)) => {
                            let template_view: Result<
                                Option<golem_service_base::model::Template>,
                                TemplateServiceError,
                            > = match response.template {
                                Some(template) => {
                                    let template: golem_service_base::model::Template =
                                        template.clone().try_into().map_err(|_| {
                                            TemplateServiceError::internal(
                                                "Response conversion error",
                                            )
                                        })?;
                                    Ok(Some(template))
                                }
                                None => {
                                    Err(TemplateServiceError::internal("Empty template response"))
                                }
                            };
                            Ok(template_view?)
                        }
                        Some(get_template_metadata_response::Result::Error(error)) => {
                            Err(error.into())
                        }
                    }
                })
            },
            TemplateServiceError::is_retriable,
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
    ) -> Result<Option<Template>, TemplateServiceError> {
        Ok(None)
    }

    async fn get_latest(
        &self,
        _template_id: &TemplateId,
    ) -> Result<Template, TemplateServiceError> {
        Err(TemplateServiceError::internal("Not implemented"))
    }
}
