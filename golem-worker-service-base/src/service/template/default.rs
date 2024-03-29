use crate::service::template::TemplateServiceError;
use crate::service::with_metadata;
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

pub type TemplateResult<T> = Result<T, TemplateServiceError>;

#[async_trait]
pub trait TemplateService<AuthCtx> {
    async fn get_by_version(
        &self,
        template_id: &TemplateId,
        version: i32,
        auth_ctx: &AuthCtx,
    ) -> TemplateResult<Template>;

    async fn get_latest(
        &self,
        template_id: &TemplateId,
        auth_ctx: &AuthCtx,
    ) -> TemplateResult<Template>;


}

#[derive(Clone)]
pub struct RemoteTemplateService {
    uri: Uri,
    retry_config: RetryConfig,
}

impl RemoteTemplateService {
    pub fn new(uri: Uri, retry_config: RetryConfig) -> Self {
        Self { uri, retry_config }
    }
}

#[async_trait]
impl<AuthCtx> TemplateService<AuthCtx> for RemoteTemplateService
where
    AuthCtx: IntoIterator<Item = (String, String)> + Clone + Send + Sync,
{
    async fn get_latest(
        &self,
        template_id: &TemplateId,
        metadata: &AuthCtx,
    ) -> TemplateResult<Template> {
        let desc = format!("Getting latest version of template: {}", template_id);
        info!("{}", &desc);

        let value = with_retries(
            &desc,
            "template",
            "get_latest",
            &self.retry_config,
            &(self.uri.clone(), template_id.clone(), metadata.clone()),
            |(uri, id, metadata)| {
                Box::pin(async move {
                    let mut client = TemplateServiceClient::connect(uri.as_http_02()).await?;
                    let request = GetLatestTemplateRequest {
                        template_id: Some(id.clone().into()),
                    };
                    let request = with_metadata(request, metadata.clone());

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
            is_retriable,
        )
        .await?;

        Ok(value)
    }

    async fn get_by_version(
        &self,
        template_id: &TemplateId,
        version: i32,
        metadata: &AuthCtx,
    ) -> TemplateResult<Template> {
        let desc = format!("Getting template: {}", template_id);
        info!("{}", &desc);

        let value = with_retries(
            &desc,
            "template",
            "get_template",
            &self.retry_config,
            &(self.uri.clone(), template_id.clone(), metadata.clone()),
            |(uri, id, metadata)| {
                Box::pin(async move {
                    let mut client = TemplateServiceClient::connect(uri.as_http_02()).await?;
                    let request = GetVersionedTemplateRequest {
                        template_id: Some(id.clone().into()),
                        version,
                    };

                    let request = with_metadata(request, metadata.clone());

                    let response = client.get_template_metadata(request).await?.into_inner();

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
            is_retriable,
        )
        .await?;

        Ok(value)
    }
}

fn is_retriable(error: &TemplateServiceError) -> bool {
    match error {
        TemplateServiceError::Internal(error) => error.is::<tonic::Status>(),
        _ => false,
    }
}

#[derive(Clone, Debug)]
pub struct TemplateServiceNoop {}

#[async_trait]
impl<AuthCtx> TemplateService<AuthCtx> for TemplateServiceNoop {
    async fn get_by_version(
        &self,
        _template_id: &TemplateId,
        _version: i32,
        _auth_ctx: &AuthCtx,
    ) -> TemplateResult<Template> {
        Err(TemplateServiceError::internal("Not implemented"))
    }

    async fn get_latest(
        &self,
        _template_id: &TemplateId,
        _auth_ctx: &AuthCtx,
    ) -> TemplateResult<Template> {
        Err(TemplateServiceError::internal("Not implemented"))
    }
}
