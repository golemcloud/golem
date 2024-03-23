use async_trait::async_trait;
use std::sync::Arc;

use golem_common::model::TemplateId;
use golem_service_base::model::Template;
use golem_worker_service_base::{
    auth::EmptyAuthCtx,
    service::template::{TemplateResult, TemplateServiceError},
};

use super::worker::WorkerNamespace;

pub type TemplateService = Arc<
    dyn golem_worker_service_base::service::template::TemplateService<WorkerNamespace, EmptyAuthCtx>
        + Sync
        + Send,
>;

pub struct TemplateServiceNoop;

#[async_trait]
impl golem_worker_service_base::service::template::TemplateService<WorkerNamespace, EmptyAuthCtx>
    for TemplateServiceNoop
{
    async fn get_by_version(
        &self,
        _template_id: &TemplateId,
        _version: i32,
        _auth: &EmptyAuthCtx,
    ) -> TemplateResult<Template, WorkerNamespace> {
        Err(TemplateServiceError::internal("Not implemented"))
    }

    async fn get_latest(
        &self,
        _template_id: &TemplateId,
        _auth: &EmptyAuthCtx,
    ) -> TemplateResult<Template, WorkerNamespace> {
        Err(TemplateServiceError::internal("Not implemented"))
    }
}
