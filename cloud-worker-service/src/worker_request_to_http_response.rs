use std::sync::Arc;

use crate::service::worker::WorkerService;
use async_trait::async_trait;
use golem_common::model::CallingConvention;
use golem_service_base::model::WorkerId;
use golem_worker_service_base::worker_bridge_execution::{
    WorkerRequest, WorkerRequestExecutor, WorkerRequestExecutorError, WorkerResponse,
};

use crate::service::auth::CloudAuthCtx;
use cloud_common::model::TokenSecret;
use tracing::info;
use uuid::Uuid;

pub struct CloudWorkerRequestToHttpResponse {
    worker_service: Arc<dyn WorkerService + Sync + Send>,
    access_token: Uuid,
}

impl CloudWorkerRequestToHttpResponse {
    pub fn new(worker_service: Arc<dyn WorkerService + Sync + Send>, access_token: Uuid) -> Self {
        Self {
            worker_service,
            access_token,
        }
    }
}

// TODO: This generic is unused.
// Error shouldn't be string, if anything should be anyhow::Error
// This allows us to downcast and have a more detailed error message.
// we should also never implement from string for an error. very bad practice and error prone.
#[async_trait]
impl WorkerRequestExecutor<poem::Response> for CloudWorkerRequestToHttpResponse {
    async fn execute(
        &self,
        resolved_worker_request: WorkerRequest,
    ) -> Result<WorkerResponse, WorkerRequestExecutorError> {
        match execute(self, resolved_worker_request).await {
            Ok(worker_response) => Ok(worker_response),
            Err(e) => Err(format!(
                "Error when executing resolved worker request. Error: {}",
                e
            ))?,
        }
    }
}

async fn execute(
    default_executor: &CloudWorkerRequestToHttpResponse,
    worker_request_params: WorkerRequest,
) -> Result<WorkerResponse, WorkerRequestExecutorError> {
    let auth = CloudAuthCtx::new(TokenSecret::new(default_executor.access_token));
    let worker_name = worker_request_params.worker_id;
    let component_id = worker_request_params.component;

    let worker_id = WorkerId::new(component_id.clone(), worker_name.clone())?;

    info!(
        "Executing request for component: {}, worker: {}, function: {}",
        component_id,
        worker_name.clone(),
        worker_request_params.function
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

    let invoke_result = default_executor
        .worker_service
        .invoke_and_await_function_typed_value(
            &worker_id,
            worker_request_params.idempotency_key,
            worker_request_params.function,
            invoke_parameters,
            &CallingConvention::Component,
            &auth,
        )
        .await
        .map_err(|e| e.to_string())?;

    Ok(WorkerResponse {
        result: invoke_result,
    })
}
