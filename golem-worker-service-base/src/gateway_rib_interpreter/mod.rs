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

use async_trait::async_trait;
use golem_common::model::invocation_context::InvocationContextStack;
use golem_common::model::{ComponentId, IdempotencyKey};
use golem_common::SafeDisplay;
use golem_wasm_rpc::protobuf::type_annotated_value::TypeAnnotatedValue;
use rib::{
    EvaluatedFnArgs, EvaluatedFqFn, EvaluatedWorkerName, InstructionId, RibByteCode,
    RibFunctionInvoke, RibFunctionInvokeResult, RibInput, RibResult,
};
use std::fmt::Display;
use std::sync::Arc;

use crate::gateway_execution::{GatewayResolvedWorkerRequest, GatewayWorkerRequestExecutor};

// A wrapper service over original RibInterpreter concerning
// the details of the worker service.
#[async_trait]
pub trait WorkerServiceRibInterpreter<Namespace> {
    // Evaluate a Rib byte against a specific worker.
    // RibByteCode may have actual function calls.
    async fn evaluate(
        &self,
        worker_name: Option<String>,
        component_id: ComponentId,
        idempotency_key: Option<IdempotencyKey>,
        invocation_context: InvocationContextStack,
        rib_byte_code: RibByteCode,
        rib_input: RibInput,
        namespace: Namespace,
    ) -> Result<RibResult, RibRuntimeError>;
}

#[derive(Debug, PartialEq)]
pub struct RibRuntimeError(pub String);

impl Display for RibRuntimeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl SafeDisplay for RibRuntimeError {
    fn to_safe_string(&self) -> String {
        self.0.clone()
    }
}

impl From<String> for RibRuntimeError {
    fn from(err: String) -> Self {
        RibRuntimeError(err)
    }
}

pub struct DefaultRibInterpreter<Namespace> {
    worker_request_executor: Arc<dyn GatewayWorkerRequestExecutor<Namespace> + Sync + Send>,
}

impl<Namespace: Clone + Send + Sync + 'static> DefaultRibInterpreter<Namespace> {
    pub fn from_worker_request_executor(
        worker_request_executor: Arc<dyn GatewayWorkerRequestExecutor<Namespace> + Sync + Send>,
    ) -> Self {
        DefaultRibInterpreter {
            worker_request_executor,
        }
    }

    pub fn rib_invoke(
        &self,
        global_worker_name: Option<String>,
        component_id: ComponentId,
        idempotency_key: Option<IdempotencyKey>,
        invocation_context: InvocationContextStack,
        namespace: Namespace,
    ) -> Arc<dyn RibFunctionInvoke + Sync + Send> {
        Arc::new(WorkerServiceRibInvoke {
            global_worker_name,
            component_id,
            idempotency_key,
            invocation_context,
            executor: self.worker_request_executor.clone(),
            namespace,
        })
    }
}

#[async_trait]
impl<Namespace: Clone + Send + Sync + 'static> WorkerServiceRibInterpreter<Namespace>
    for DefaultRibInterpreter<Namespace>
{
    async fn evaluate(
        &self,
        worker_name: Option<String>,
        component_id: ComponentId,
        idempotency_key: Option<IdempotencyKey>,
        invocation_context: InvocationContextStack,
        expr: RibByteCode,
        rib_input: RibInput,
        namespace: Namespace,
    ) -> Result<RibResult, RibRuntimeError> {
        let worker_invoke_function = self.rib_invoke(
            worker_name,
            component_id,
            idempotency_key,
            invocation_context,
            namespace,
        );

        let result = rib::interpret(expr, rib_input, worker_invoke_function)
            .await
            .map_err(|err| RibRuntimeError(err.to_string()))?;
        Ok(result)
    }
}

struct WorkerServiceRibInvoke<Namespace> {
    // For backward compatibility.
    // If there is no worker-name in the Rib (which is EvaluatedWorkerName),
    // then it tries to fall back to this global_worker_name that came in as
    // part of the API definition.
    global_worker_name: Option<String>,
    component_id: ComponentId,
    idempotency_key: Option<IdempotencyKey>,
    invocation_context: InvocationContextStack,
    executor: Arc<dyn GatewayWorkerRequestExecutor<Namespace> + Sync + Send>,
    namespace: Namespace,
}

#[async_trait]
impl<Namespace: Clone + Send + Sync + 'static> RibFunctionInvoke
    for WorkerServiceRibInvoke<Namespace>
{
    async fn invoke(
        &self,
        _instruction_id: &InstructionId,
        worker_name: Option<EvaluatedWorkerName>,
        function_name: EvaluatedFqFn,
        parameters: EvaluatedFnArgs,
    ) -> RibFunctionInvokeResult {
        let component_id = self.component_id.clone();
        let worker_name: Option<String> =
            worker_name.map(|x| x.0).or(self.global_worker_name.clone());
        let idempotency_key = self.idempotency_key.clone();
        let invocation_context = self.invocation_context.clone();
        let executor = self.executor.clone();
        let namespace = self.namespace.clone();

        let function_name = function_name.0;

        let function_params: Vec<TypeAnnotatedValue> = parameters
            .0
            .into_iter()
            .map(TypeAnnotatedValue::try_from)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|errs: Vec<String>| errs.join(", "))?;

        let worker_request = GatewayResolvedWorkerRequest {
            component_id,
            worker_name,
            function_name,
            function_params,
            idempotency_key,
            invocation_context,
            namespace,
        };

        let tav = executor.execute(worker_request).await.map(|v| v.result)?;

        tav.try_into().map_err(|err: String| err.into())
    }
}
