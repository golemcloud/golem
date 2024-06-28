use std::sync::Arc;

use async_trait::async_trait;
use golem_worker_service_base::auth::EmptyAuthCtx;
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
    use golem_common::model::CallingConvention;
    use golem_service_base::model::WorkerId;
    use golem_wasm_rpc::json::get_json_from_typed_value;
    use golem_worker_service_base::auth::EmptyAuthCtx;
    use serde_json::Value;

    use golem_worker_service_base::worker_bridge_execution::{
        WorkerRequest, WorkerRequestExecutorError, WorkerResponse,
    };
    use tracing::info;

    pub(crate) async fn execute(
        default_executor: &UnauthorisedWorkerRequestExecutor,
        worker_request_params: WorkerRequest,
    ) -> Result<WorkerResponse, WorkerRequestExecutorError> {
        let worker_name = worker_request_params.worker_name;

        let component_id = worker_request_params.component_id;

        let worker_id = WorkerId::new(component_id.clone(), worker_name.clone())?;

        info!(
            "Executing request for component: {}, worker: {}, function: {}",
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

        info!(
            "Executing request for component: {}, worker: {}, idempotency key: {}, invocation params: {:?}",
            component_id, worker_name.clone(), idempotency_key_str, invoke_parameters
        );

        let mut invoke_parameters_values = vec![];

        for param in invoke_parameters {
            let value = get_json_from_typed_value(&param);
            invoke_parameters_values.push(value);
        }

        let invoke_result = default_executor
            .worker_service
            .invoke_and_await_function_typed_value(
                &worker_id,
                worker_request_params.idempotency_key,
                worker_request_params.function_name.to_string(),
                Value::Array(invoke_parameters_values),
                &CallingConvention::Component,
                None,
                empty_worker_metadata(),
                &EmptyAuthCtx::default(),
            )
            .await
            .map_err(|e| e.to_string())?;

        Ok(WorkerResponse {
            result: invoke_result,
        })
    }
}
