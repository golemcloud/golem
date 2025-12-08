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
use golem_common::model::agent::{AgentId, AgentMode};
use golem_common::model::component::{ComponentId, ComponentRevision};
use golem_common::model::invocation_context::InvocationContextStack;
use golem_common::model::{IdempotencyKey, WorkerId};
use golem_common::SafeDisplay;
use golem_service_base::model::auth::AuthCtx;
use golem_wasm::analysis::AnalysedType;
use golem_wasm::ValueAndType;
use rib::InstructionId;
use rib::{
    ComponentDependencyKey, EvaluatedFnArgs, EvaluatedFqFn, EvaluatedWorkerName, RibByteCode,
    RibComponentFunctionInvoke, RibFunctionInvokeResult, RibInput, RibResult,
};
use std::collections::BTreeMap;
use std::fmt::Display;
use std::sync::Arc;
use tracing::debug;
use uuid::Uuid;

pub struct GatewayWorkerRequestExecutor {
    worker_service: Arc<WorkerService>,
    component_service: Arc<dyn ComponentService>,
}

impl GatewayWorkerRequestExecutor {
    pub fn new(
        worker_service: Arc<WorkerService>,
        component_service: Arc<dyn ComponentService>,
    ) -> Self {
        Self {
            worker_service,
            component_service,
        }
    }

    pub async fn evaluate_rib(
        self: &Arc<Self>,
        idempotency_key: Option<IdempotencyKey>,
        invocation_context: InvocationContextStack,
        expr: RibByteCode,
        rib_input: RibInput,
    ) -> Result<RibResult, WorkerRequestExecutorError> {
        let worker_invoke_function: Arc<dyn RibComponentFunctionInvoke + Send + Sync> =
            Arc::new(self.rib_invoke(idempotency_key, invocation_context));

        let result = rib::interpret(expr, rib_input, worker_invoke_function, None)
            .await
            .map_err(|err| WorkerRequestExecutorError(err.to_string()))?;
        Ok(result)
    }

    pub async fn execute(
        &self,
        resolved_worker_request: GatewayResolvedWorkerRequest,
    ) -> Result<Option<ValueAndType>, WorkerRequestExecutorError> {
        let component = self
            .component_service
            .get_revision(
                &resolved_worker_request.component_id,
                resolved_worker_request.component_revision,
            )
            .await
            .map_err(|err| WorkerRequestExecutorError(err.to_safe_string()))?;

        let mut worker_name = resolved_worker_request.worker_name;

        if component.metadata.is_agent() {
            let agent_type_name = AgentId::parse_agent_type_name(&worker_name)
                .map_err(|err| WorkerRequestExecutorError(format!("Invalid agent ID: {err}")))?;
            let agent_type = component
                .metadata
                .find_agent_type_by_wrapper_name(agent_type_name)
                .map_err(|err| {
                    WorkerRequestExecutorError(format!("Failed to extract agent type: {err}"))
                })?
                .ok_or_else(|| WorkerRequestExecutorError("Agent type not found".to_string()))?;

            if agent_type.mode == AgentMode::Ephemeral {
                let phantom_id = Uuid::new_v4();
                let phantom_id_postfix = format!("[{phantom_id}]");
                worker_name.push_str(&phantom_id_postfix);
            }
        }

        let worker_id = WorkerId::from_component_metadata_and_worker_id(
            component.id,
            &component.metadata,
            worker_name,
        )?;

        debug!(
            component_id = resolved_worker_request.component_id.to_string(),
            function_name = resolved_worker_request.function_name,
            worker_name = worker_id.worker_name.clone(),
            "Executing invocation",
        );

        let result = self
            .worker_service
            .invoke_and_await_typed(
                &worker_id,
                resolved_worker_request.idempotency_key,
                resolved_worker_request.function_name.to_string(),
                resolved_worker_request.function_params,
                Some(golem_api_grpc::proto::golem::worker::InvocationContext {
                    parent: None,
                    env: Default::default(),
                    wasi_config_vars: Some(BTreeMap::new().into()),
                    tracing: Some(resolved_worker_request.invocation_context.into()),
                }),
                AuthCtx::impersonated_user(component.account_id),
            )
            .await
            .map_err(|e| format!("Error when executing resolved worker request. Error: {e}"))?;

        Ok(result)
    }

    fn rib_invoke(
        self: &Arc<Self>,
        idempotency_key: Option<IdempotencyKey>,
        invocation_context: InvocationContextStack,
    ) -> WorkerRequestExecutorRibInvoke {
        WorkerRequestExecutorRibInvoke {
            idempotency_key,
            invocation_context,
            executor: self.clone(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct WorkerRequestExecutorError(pub String);

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

impl SafeDisplay for WorkerRequestExecutorError {
    fn to_safe_string(&self) -> String {
        self.0.clone()
    }
}

struct WorkerRequestExecutorRibInvoke {
    executor: Arc<GatewayWorkerRequestExecutor>,
    idempotency_key: Option<IdempotencyKey>,
    invocation_context: InvocationContextStack,
}

#[async_trait]
impl RibComponentFunctionInvoke for WorkerRequestExecutorRibInvoke {
    async fn invoke(
        &self,
        component_dependency_key: ComponentDependencyKey,
        _instruction_id: &InstructionId,
        worker_name: EvaluatedWorkerName,
        function_name: EvaluatedFqFn,
        parameters: EvaluatedFnArgs,
        _return_type: Option<AnalysedType>,
    ) -> RibFunctionInvokeResult {
        let worker_name = worker_name.0;

        let idempotency_key = self.idempotency_key.clone();
        let invocation_context = self.invocation_context.clone();
        let executor = self.executor.clone();

        let function_name = function_name.0;
        let function_params: Vec<ValueAndType> = parameters.0;

        let component_id = ComponentId(component_dependency_key.component_id);
        let component_revision = ComponentRevision(component_dependency_key.component_revision);

        let worker_request = GatewayResolvedWorkerRequest {
            component_id,
            component_revision,
            worker_name,
            function_name,
            function_params,
            idempotency_key,
            invocation_context,
        };

        let result = executor.execute(worker_request).await?;
        Ok(result)
    }
}
