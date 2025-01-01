// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use std::sync::Arc;

use async_trait::async_trait;
use golem_common::model::TargetWorkerId;
use golem_service_base::auth::DefaultNamespace;
use golem_service_base::model::validate_worker_name;
use golem_worker_service_base::empty_worker_metadata;
use golem_worker_service_base::gateway_execution::{
    GatewayResolvedWorkerRequest, GatewayWorkerRequestExecutor, WorkerRequestExecutorError,
    WorkerResponse,
};
use golem_worker_service_base::service::worker::WorkerService;
use tracing::{debug, info};

// The open source deviates from the proprietary codebase here, only in terms of authorisation
pub struct UnauthorisedWorkerRequestExecutor {
    pub worker_service: Arc<dyn WorkerService + Sync + Send>,
}

impl UnauthorisedWorkerRequestExecutor {
    pub fn new(worker_service: Arc<dyn WorkerService + Sync + Send>) -> Self {
        Self { worker_service }
    }
}

#[async_trait]
impl GatewayWorkerRequestExecutor<DefaultNamespace> for UnauthorisedWorkerRequestExecutor {
    async fn execute(
        &self,
        worker_request_params: GatewayResolvedWorkerRequest<DefaultNamespace>,
    ) -> Result<WorkerResponse, WorkerRequestExecutorError> {
        let worker_name_opt_validated = worker_request_params
            .worker_name
            .map(|w| validate_worker_name(w.as_str()).map(|_| w))
            .transpose()?;

        let component_id = worker_request_params.component_id;

        let worker_id = TargetWorkerId {
            component_id: component_id.clone(),
            worker_name: worker_name_opt_validated.clone(),
        };

        info!(
            "Executing request for component: {}, worker: {}, function: {:?}",
            component_id,
            worker_name_opt_validated
                .clone()
                .unwrap_or("<NA/ephemeral>".to_string()),
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
            worker_name_opt_validated,
            function_name = worker_request_params.function_name.to_string(),
            idempotency_key = idempotency_key_str,
            "Executing request",
        );

        // TODO: check if these are already added from span
        debug!(
            component_id = component_id.to_string(),
            worker_name_opt_validated,
            function_name = worker_request_params.function_name.to_string(),
            idempotency_key = idempotency_key_str,
            invocation_params = format!("{:?}", invoke_parameters),
            "Invocation parameters"
        );

        let type_annotated_value = self
            .worker_service
            .validate_and_invoke_and_await_typed(
                &worker_id,
                worker_request_params.idempotency_key,
                worker_request_params.function_name,
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
