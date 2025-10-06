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

use crate::gateway_execution::GatewayResolvedWorkerRequest;
use crate::service::component::ComponentService;
use crate::service::worker::WorkerService;
use async_trait::async_trait;
use golem_common::model::agent::AgentId;
use golem_common::model::auth::{AuthCtx, TokenSecret};
use golem_common::model::WorkerId;
use golem_common::SafeDisplay;
use golem_wasm_rpc::ValueAndType;
use std::collections::BTreeMap;
use std::fmt::Display;
use std::sync::Arc;
use tracing::debug;

#[async_trait]
pub trait GatewayWorkerRequestExecutor: Send + Sync {
    async fn execute(
        &self,
        resolved_worker_request: GatewayResolvedWorkerRequest,
    ) -> Result<WorkerResponse, WorkerRequestExecutorError>;
}

// The result of a worker execution from worker-bridge,
// which is a combination of function metadata and the type-annotated-value representing the actual result
pub struct WorkerResponse {
    pub result: Option<ValueAndType>,
}

impl WorkerResponse {
    pub fn new(result: Option<ValueAndType>) -> Self {
        WorkerResponse { result }
    }
}

#[derive(Clone, Debug)]
pub struct WorkerRequestExecutorError(String);

impl std::error::Error for WorkerRequestExecutorError {}

impl Display for WorkerRequestExecutorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl<T: AsRef<str>> From<T> for WorkerRequestExecutorError {
    fn from(err: T) -> Self {
        WorkerRequestExecutorError(err.as_ref().to_string())
    }
}

pub struct GatewayWorkerRequestExecutorDefault {
    worker_service: Arc<dyn WorkerService>,
    component_service: Arc<dyn ComponentService>,
    component_service_token: TokenSecret,
}

impl GatewayWorkerRequestExecutorDefault {
    pub fn new(
        worker_service: Arc<dyn WorkerService>,
        component_service: Arc<dyn ComponentService>,
        component_service_token: TokenSecret,
    ) -> Self {
        Self {
            worker_service,
            component_service,
            component_service_token,
        }
    }
}

#[async_trait]
impl GatewayWorkerRequestExecutor for GatewayWorkerRequestExecutorDefault {
    async fn execute(
        &self,
        resolved_worker_request: GatewayResolvedWorkerRequest,
    ) -> Result<WorkerResponse, WorkerRequestExecutorError> {
        let component = self
            .component_service
            .get_by_version(
                &resolved_worker_request.component_id,
                resolved_worker_request.component_version,
                &AuthCtx::new(self.component_service_token.clone()),
            )
            .await
            .map_err(|err| WorkerRequestExecutorError(err.to_safe_string()))?;
        let raw_worker_name = resolved_worker_request.worker_name.to_string();
        let worker_name = AgentId::parse(resolved_worker_request.worker_name, &component.metadata)
            .ok()
            .map(|agent_id| agent_id.to_string())
            .unwrap_or(raw_worker_name);

        WorkerId::validate_worker_name(&worker_name)?;
        debug!(
            component_id = resolved_worker_request.component_id.to_string(),
            function_name = resolved_worker_request.function_name,
            worker_name = worker_name,
            "Executing invocation",
        );

        let worker_id = WorkerId {
            component_id: resolved_worker_request.component_id.clone(),
            worker_name,
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
                    wasi_config_vars: Some(BTreeMap::new().into()),
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
