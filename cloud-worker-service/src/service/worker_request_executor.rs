use std::sync::Arc;

use crate::service::worker::WorkerService;
use async_trait::async_trait;
use cloud_common::auth::CloudNamespace;
use golem_common::model::{TargetWorkerId, WorkerId};
use golem_worker_service_base::gateway_execution::{
    GatewayResolvedWorkerRequest, GatewayWorkerRequestExecutor, WorkerRequestExecutorError,
    WorkerResponse,
};
use tracing::debug;

pub struct CloudGatewayWorkerRequestExecutor {
    worker_service: Arc<dyn WorkerService + Sync + Send>,
}

impl CloudGatewayWorkerRequestExecutor {
    pub fn new(worker_service: Arc<dyn WorkerService + Sync + Send>) -> Self {
        Self { worker_service }
    }
}

#[async_trait]
impl GatewayWorkerRequestExecutor<CloudNamespace> for CloudGatewayWorkerRequestExecutor {
    async fn execute(
        &self,
        resolved_worker_request: GatewayResolvedWorkerRequest<CloudNamespace>,
    ) -> Result<WorkerResponse, WorkerRequestExecutorError> {
        let worker_name_opt_validated = resolved_worker_request
            .worker_name
            .map(|w| WorkerId::validate_worker_name(w.as_str()).map(|_| w))
            .transpose()?;

        debug!(
            component_id = resolved_worker_request.component_id.to_string(),
            function_name = resolved_worker_request.function_name,
            worker_name_opt_validated,
            "Executing invocation",
        );

        let worker_id = TargetWorkerId {
            component_id: resolved_worker_request.component_id.clone(),
            worker_name: worker_name_opt_validated.clone(),
        };

        let type_annotated_value = self
            .worker_service
            .validate_and_invoke_and_await_typed(
                &worker_id,
                resolved_worker_request.idempotency_key,
                resolved_worker_request.function_name.to_string(),
                resolved_worker_request.function_params,
                None,
                resolved_worker_request.namespace,
            )
            .await
            .map_err(|e| format!("Error when executing resolved worker request. Error: {e}"))?;

        Ok(WorkerResponse {
            result: type_annotated_value,
        })
    }
}
