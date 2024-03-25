use async_trait::async_trait;
use std::sync::Arc;

use golem_common::model::TemplateId;
use golem_service_base::model::Template;
use golem_worker_service_base::{
    auth::{CommonNamespace, EmptyAuthCtx},
    service::template::{TemplateResult, TemplateServiceError},
};

pub type TemplateService = Arc<
    dyn golem_worker_service_base::service::template::TemplateService<EmptyAuthCtx, CommonNamespace>
        + Sync
        + Send,
>;

pub struct TemplateServiceNoop;

#[async_trait]
impl<AuthCtx, Namespace>
    golem_worker_service_base::service::template::TemplateService<AuthCtx, Namespace>
    for TemplateServiceNoop
{
    async fn get_by_version(
        &self,
        _template_id: &TemplateId,
        _version: i32,
        _auth: &AuthCtx,
    ) -> TemplateResult<Template, Namespace> {
        Err(TemplateServiceError::internal("Not implemented"))
    }

    async fn get_latest(
        &self,
        _template_id: &TemplateId,
        _auth: &AuthCtx,
    ) -> TemplateResult<Template, Namespace> {
        Err(TemplateServiceError::internal("Not implemented"))
    }
}
