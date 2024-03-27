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
use golem_service_base::service::auth::{AuthService, Permission, WithAuth, WithNamespace};
use http::Uri;
use std::sync::Arc;
use tracing::info;

pub type TemplateResult<T, Namespace> = Result<WithNamespace<T, Namespace>, TemplateServiceError>;

#[async_trait]
pub trait TemplateService<AuthCtx, Namespace> {
    async fn get_by_version(
        &self,
        template_id: &TemplateId,
        version: i32,
        auth_ctx: &AuthCtx,
    ) -> TemplateResult<Template, Namespace>;

    async fn get_latest(
        &self,
        template_id: &TemplateId,
        auth_ctx: &AuthCtx,
    ) -> TemplateResult<Template, Namespace>;
}

#[derive(Clone)]
pub struct TemplateServiceDefault<AuthCtx, Namespace> {
    uri: Uri,
    retry_config: RetryConfig,
    auth_service: InnerAuthService<AuthCtx, Namespace>,
}

type InnerAuthService<AuthCtx, Namespace> =
    Arc<dyn AuthService<WithAuth<TemplateId, AuthCtx>, Namespace> + Send + Sync>;

impl<AuthCtx, Namespace> TemplateServiceDefault<AuthCtx, Namespace> {
    pub fn new(
        uri: Uri,
        retry_config: RetryConfig,
        auth_service: InnerAuthService<AuthCtx, Namespace>,
    ) -> Self {
        Self {
            uri,
            retry_config,
            auth_service,
        }
    }
}

#[async_trait]
impl<AuthCtx, Namespace> TemplateService<AuthCtx, Namespace>
    for TemplateServiceDefault<AuthCtx, Namespace>
where
    Namespace: Send + Sync,
    AuthCtx: Clone + Send + Sync,
{
    async fn get_latest(
        &self,
        template_id: &TemplateId,
        auth_ctx: &AuthCtx,
    ) -> TemplateResult<Template, Namespace> {
        let desc = format!("Getting latest version of template: {}", template_id);
        info!("{}", &desc);
        let auth_ctx = WithAuth::new(template_id.clone(), auth_ctx.clone());

        let namespace = self
            .auth_service
            .is_authorized(Permission::View, &auth_ctx)
            .await?;

        let value = with_retries(
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
            is_retriable,
        )
        .await?;

        Ok(WithNamespace { namespace, value })
    }

    async fn get_by_version(
        &self,
        template_id: &TemplateId,
        version: i32,
        auth_ctx: &AuthCtx,
    ) -> TemplateResult<Template, Namespace> {
        let desc = format!("Getting template: {}", template_id);
        info!("{}", &desc);

        let auth_ctx = WithAuth::new(template_id.clone(), auth_ctx.clone());

        let namespace = self
            .auth_service
            .is_authorized(Permission::View, &auth_ctx)
            .await?;

        let value = with_retries(
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

        Ok(WithNamespace { value, namespace })
    }
}

fn is_retriable(error: &TemplateServiceError) -> bool {
    match error {
        TemplateServiceError::Internal(error) => error.is::<tonic::Status>(),
        _ => false,
    }
}
