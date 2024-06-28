use std::sync::Arc;

use golem_service_base::auth::EmptyAuthCtx;

pub type WorkerService =
    Arc<dyn golem_worker_service_base::service::worker::WorkerService<EmptyAuthCtx> + Sync + Send>;
