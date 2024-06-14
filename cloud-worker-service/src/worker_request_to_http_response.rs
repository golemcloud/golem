use std::sync::Arc;

use crate::service::worker::WorkerService;
use async_trait::async_trait;
use golem_common::model::CallingConvention;
use golem_service_base::model::WorkerId;
use golem_wasm_rpc::json::get_json_from_typed_value;
use golem_worker_service_base::worker_bridge_execution::{
    WorkerRequest, WorkerRequestExecutor, WorkerRequestExecutorError, WorkerResponse,
};
use serde_json::Value;

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

#[async_trait]
impl WorkerRequestExecutor for CloudWorkerRequestToHttpResponse {
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
            &auth,
        )
        .await
        .map_err(|e| e.to_string())?;

    Ok(WorkerResponse {
        result: invoke_result,
    })
}
