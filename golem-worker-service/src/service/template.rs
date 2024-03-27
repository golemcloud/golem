use golem_worker_service_base::auth::EmptyAuthCtx;
use std::sync::Arc;

pub type TemplateService = Arc<
    dyn golem_worker_service_base::service::template::TemplateService<EmptyAuthCtx> + Sync + Send,
>;
