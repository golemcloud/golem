use std::sync::Arc;

use async_trait::async_trait;
use golem_service_base::auth::EmptyAuthCtx;
use golem_worker_service_base::service::worker::WorkerService;
use golem_worker_service_base::worker_bridge_execution::{
    WorkerRequest, WorkerRequestExecutor, WorkerRequestExecutorError, WorkerResponse,
};

// The open source deviates from the proprietary codebase here, only in terms of authorisation
pub struct UnauthorisedWorkerRequestExecutor {
    pub worker_service: Arc<dyn WorkerService<EmptyAuthCtx> + Sync + Send>,
}

impl UnauthorisedWorkerRequestExecutor {
    pub fn new(worker_service: Arc<dyn WorkerService<EmptyAuthCtx> + Sync + Send>) -> Self {
        Self { worker_service }
    }
}

#[async_trait]
impl WorkerRequestExecutor for UnauthorisedWorkerRequestExecutor {
    async fn execute(
        &self,
        worker_request_params: WorkerRequest,
    ) -> Result<WorkerResponse, WorkerRequestExecutorError> {
        internal::execute(self, worker_request_params.clone()).await
    }
}

mod internal {
    use crate::empty_worker_metadata;
    use crate::worker_bridge_request_executor::UnauthorisedWorkerRequestExecutor;
    use golem_service_base::model::WorkerId;

    use golem_worker_service_base::worker_bridge_execution::{
        WorkerRequest, WorkerRequestExecutorError, WorkerResponse,
    };
    use tracing::{debug, info};

    pub(crate) async fn execute(
        default_executor: &UnauthorisedWorkerRequestExecutor,
        worker_request_params: WorkerRequest,
    ) -> Result<WorkerResponse, WorkerRequestExecutorError> {
        let worker_name = worker_request_params.worker_name;

        let component_id = worker_request_params.component_id;

        let worker_id = WorkerId::new(component_id.clone(), worker_name.clone())?;

        info!(
            "Executing request for component: {}, worker: {}, function: {:?}",
            component_id,
            worker_name.clone(),
            worker_request_params.function_name
        );

        let invoke_parameters = worker_request_params.function_params;

        let idempotency_key_str = worker_request_params
            .idempotency_key
            .clone()
            .map(|k| k.to_string())
            .unwrap_or("N/A".to_string());

        // TODO: check if these are already added from span
        info!(
            component_id = component_id.to_string(),
            worker_name,
            function_name = worker_request_params.function_name.to_string(),
            idempotency_key = idempotency_key_str,
            "Executing request",
        );

        // TODO: check if these are already added from span
        debug!(
            component_id = component_id.to_string(),
            worker_name,
            function_name = worker_request_params.function_name.to_string(),
            idempotency_key = idempotency_key_str,
            invocation_params = format!("{:?}", invoke_parameters),
            "Invocation parameters"
        );

        let type_annotated_value = default_executor
            .worker_service
            .invoke_and_await_function_typed(
                &worker_id,
                worker_request_params.idempotency_key,
                worker_request_params.function_name.to_string(),
                invoke_parameters,
                None,
                empty_worker_metadata(),
            )
            .await
            .map_err(|e| e.to_string())?;

        Ok(WorkerResponse {
            result: type_annotated_value,
        })
    }
}
