// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use crate::service::worker::WorkerService;
use async_trait::async_trait;
use golem_common::model::auth::Namespace;
use golem_common::model::{TargetWorkerId, WorkerId};
use golem_worker_service_base::gateway_execution::{
    GatewayResolvedWorkerRequest, GatewayWorkerRequestExecutor, WorkerRequestExecutorError,
    WorkerResponse,
};
use std::sync::Arc;
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
impl GatewayWorkerRequestExecutor<Namespace> for CloudGatewayWorkerRequestExecutor {
    async fn execute(
        &self,
        resolved_worker_request: GatewayResolvedWorkerRequest<Namespace>,
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
                Some(golem_api_grpc::proto::golem::worker::InvocationContext {
                    parent: None,
                    args: vec![],
                    env: Default::default(),
                    tracing: Some(resolved_worker_request.invocation_context.into()),
                }),
                resolved_worker_request.namespace,
            )
            .await
            .map_err(|e| format!("Error when executing resolved worker request. Error: {e}"))?;

        Ok(WorkerResponse {
            result: type_annotated_value,
        })
    }
}
