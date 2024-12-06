use std::sync::Arc;

pub type WorkerIdentityService = Arc<
    dyn golem_worker_service_base::service::worker_identity::WorkerIdentityService + Sync + Send,
>;
