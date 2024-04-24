use golem_worker_service_base::auth::EmptyAuthCtx;
use std::sync::Arc;

pub type ComponentService = Arc<
    dyn golem_worker_service_base::service::component::ComponentService<EmptyAuthCtx> + Sync + Send,
>;
