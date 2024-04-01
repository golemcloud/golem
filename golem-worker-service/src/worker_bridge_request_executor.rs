
use std::sync::Arc;

use async_trait::async_trait;
use golem_worker_service_base::auth::EmptyAuthCtx;
use golem_worker_service_base::service::worker::WorkerService;
use golem_worker_service_base::worker_bridge::worker_response::WorkerResponse;
use golem_worker_service_base::worker_bridge::WorkerRequest;
use golem_worker_service_base::worker_bridge::workr_request_executor::{WorkerRequestExecutor, WorkerRequestExecutorError};

pub struct WorkerRequestToHttpResponse {
    pub worker_service: Arc<dyn WorkerService<EmptyAuthCtx> + Sync + Send>,
}

impl WorkerRequestToHttpResponse {
    pub fn new(worker_service: Arc<dyn WorkerService<EmptyAuthCtx> + Sync + Send>) -> Self {
        Self { worker_service }
    }
}

#[async_trait]
impl WorkerRequestExecutor<poem::Response> for WorkerRequestToHttpResponse {
    async fn execute(
        &self,
        worker_request_params: WorkerRequest,
    ) -> Result<WorkerResponse, WorkerRequestExecutorError>{
        internal::execute(self, worker_request_params.clone()).await
    }
}

mod internal {
    use tracing::info;
    use golem_common::model::CallingConvention;
    use golem_service_base::model::WorkerId;
    use golem_worker_service_base::auth::EmptyAuthCtx;
    use golem_worker_service_base::worker_bridge::worker_response::WorkerResponse;
    use golem_worker_service_base::worker_bridge::WorkerRequest;
    use golem_worker_service_base::worker_bridge::workr_request_executor::WorkerRequestExecutorError;
    use crate::empty_worker_metadata;
    use crate::worker_bridge_request_executor::WorkerRequestToHttpResponse;

    pub(crate) async fn execute(
        default_executor: &WorkerRequestToHttpResponse,
        worker_request_params: WorkerRequest,
    ) -> Result<WorkerResponse, WorkerRequestExecutorError> {
        let worker_name = worker_request_params.worker_id;

        let template_id = worker_request_params.template;

        let worker_id = WorkerId::new(template_id.clone(), worker_name.clone())?;

        info!(
        "Executing request for template: {}, worker: {}, function: {}",
        template_id,
        worker_name.clone(),
        worker_request_params.function
    );

        let invocation_key = default_executor
            .worker_service
            .get_invocation_key(&worker_id, &EmptyAuthCtx {})
            .await
            .map_err(|e| e.to_string())?;

        let invoke_parameters = worker_request_params.function_params;

        info!(
            "Executing request for template: {}, worker: {}, invocation key: {}, invocation params: {:?}",
            template_id, worker_name.clone(), invocation_key, invoke_parameters
        );

        let invoke_result = default_executor
            .worker_service
            .invoke_and_await_function_typed_value(
                &worker_id,
                worker_request_params.function,
                &invocation_key,
                invoke_parameters,
                &CallingConvention::Component,
                empty_worker_metadata(),
                &EmptyAuthCtx {},
            )
            .await
            .map_err(|e| e.to_string())?;

        Ok(WorkerResponse {
            result: invoke_result,
        })
    }
}
