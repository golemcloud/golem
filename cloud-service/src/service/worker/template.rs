use std::sync::Arc;

use async_trait::async_trait;
use golem_common::model::TemplateId;
use golem_service_base::model::Template as BaseTemplate;
use golem_service_base::model::VersionedTemplateId;
use golem_worker_service_base::service::template::{
    TemplateResult, TemplateService as BaseTemplateService,
    TemplateServiceError as BaseTemplateError,
};

use crate::auth::AccountAuthorisation;
use crate::service::template::{TemplateError, TemplateService};

#[derive(Clone)]
pub struct TemplateServiceWrapper {
    template_service: Arc<dyn TemplateService + Send + Sync>,
}

impl TemplateServiceWrapper {
    pub fn new(template_service: Arc<dyn TemplateService + Send + Sync>) -> Self {
        Self { template_service }
    }
}

#[async_trait]
impl BaseTemplateService<AccountAuthorisation> for TemplateServiceWrapper {
    async fn get_by_version(
        &self,
        template_id: &TemplateId,
        version: i32,
        auth_ctx: &AccountAuthorisation,
    ) -> TemplateResult<BaseTemplate> {
        let versioned = VersionedTemplateId {
            template_id: template_id.clone(),
            version,
        };

        let template = self
            .template_service
            .get_by_version(&versioned, auth_ctx)
            .await?
            .ok_or(BaseTemplateError::NotFound(format!(
                "Template not found: {template_id}",
            )))?;

        let template = convert_template(template);

        Ok(template)
    }

    async fn get_latest(
        &self,
        template_id: &TemplateId,
        auth_ctx: &AccountAuthorisation,
    ) -> TemplateResult<BaseTemplate> {
        let template = self
            .template_service
            .get_latest_version(template_id, auth_ctx)
            .await?
            .ok_or(BaseTemplateError::NotFound(format!(
                "Template not found: {template_id}",
            )))?;

        let template = convert_template(template);

        Ok(template)
    }
}

impl From<TemplateError> for BaseTemplateError {
    fn from(error: TemplateError) -> Self {
        match error {
            TemplateError::AlreadyExists(_) => BaseTemplateError::AlreadyExists(error.to_string()),
            TemplateError::UnknownTemplateId(_)
            | TemplateError::UnknownVersionedTemplateId(_)
            | TemplateError::UnknownProjectId(_) => BaseTemplateError::NotFound(error.to_string()),
            TemplateError::Unauthorized(e) => BaseTemplateError::Unauthorized(e),
            TemplateError::LimitExceeded(e) => BaseTemplateError::Forbidden(e),
            TemplateError::TemplateProcessing(e) => {
                BaseTemplateError::BadRequest(vec![e.to_string()])
            }
            TemplateError::Internal(e) => BaseTemplateError::Internal(anyhow::Error::msg(e)),
        }
    }
}

fn convert_template(template: crate::model::Template) -> BaseTemplate {
    BaseTemplate {
        versioned_template_id: template.versioned_template_id,
        user_template_id: template.user_template_id,
        protected_template_id: template.protected_template_id,
        template_name: template.template_name,
        template_size: template.template_size,
        metadata: template.metadata,
    }
}
