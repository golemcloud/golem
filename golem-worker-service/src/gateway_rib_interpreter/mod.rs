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

use crate::gateway_execution::{GatewayResolvedWorkerRequest, GatewayWorkerRequestExecutor};
use async_trait::async_trait;
use golem_common::model::auth::Namespace;
use golem_common::model::invocation_context::InvocationContextStack;
use golem_common::model::{ComponentId, IdempotencyKey};
use golem_common::SafeDisplay;
use golem_wasm_ast::analysis::AnalysedType;
use golem_wasm_rpc::protobuf::type_annotated_value::TypeAnnotatedValue;
use golem_wasm_rpc::ValueAndType;
use rib::{
    ComponentDependencyKey, EvaluatedFnArgs, EvaluatedFqFn, EvaluatedWorkerName, InstructionId,
    RibByteCode, RibComponentFunctionInvoke, RibFunctionInvokeResult, RibInput, RibResult,
};
use std::fmt::Display;
use std::sync::Arc;

// A wrapper service over original RibInterpreter
// Note that to execute a RibByteCode, there is no need to provide
// worker_name and component details from outside, as these are already
// encoded in the RibByteCode itself.
// This implies file-server handlers and http-handlers will only execute
// rib that's devoid of any instantiation of worker or worker function invocation
#[async_trait]
pub trait WorkerServiceRibInterpreter: Send + Sync {
    // Evaluate a Rib byte against a specific worker.
    // RibByteCode may have actual function calls.
    async fn evaluate(
        &self,
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

pub struct DefaultRibInterpreter {
    worker_request_executor: Arc<dyn GatewayWorkerRequestExecutor>,
}

impl DefaultRibInterpreter {
    pub fn from_worker_request_executor(
        worker_request_executor: Arc<dyn GatewayWorkerRequestExecutor>,
    ) -> Self {
        DefaultRibInterpreter {
            worker_request_executor,
        }
    }

    pub fn rib_invoke(
        &self,
        idempotency_key: Option<IdempotencyKey>,
        invocation_context: InvocationContextStack,
        namespace: Namespace,
    ) -> Arc<dyn RibComponentFunctionInvoke + Sync + Send> {
        Arc::new(WorkerServiceRibInvoke {
            idempotency_key,
            invocation_context,
            executor: self.worker_request_executor.clone(),
            namespace,
        })
    }
}

#[async_trait]
impl WorkerServiceRibInterpreter for DefaultRibInterpreter {
    async fn evaluate(
        &self,
        idempotency_key: Option<IdempotencyKey>,
        invocation_context: InvocationContextStack,
        expr: RibByteCode,
        rib_input: RibInput,
        namespace: Namespace,
    ) -> Result<RibResult, RibRuntimeError> {
        let worker_invoke_function =
            self.rib_invoke(idempotency_key, invocation_context, namespace);

        let result = rib::interpret(expr, rib_input, worker_invoke_function, None)
            .await
            .map_err(|err| RibRuntimeError(err.to_string()))?;
        Ok(result)
    }
}

struct WorkerServiceRibInvoke {
    idempotency_key: Option<IdempotencyKey>,
    invocation_context: InvocationContextStack,
    executor: Arc<dyn GatewayWorkerRequestExecutor>,
    namespace: Namespace,
}

#[async_trait]
impl RibComponentFunctionInvoke for WorkerServiceRibInvoke {
    async fn invoke(
        &self,
        component_dependency_key: ComponentDependencyKey,
        _instruction_id: &InstructionId,
        worker_name: Option<EvaluatedWorkerName>,
        function_name: EvaluatedFqFn,
        parameters: EvaluatedFnArgs,
        _return_type: Option<AnalysedType>,
    ) -> RibFunctionInvokeResult {
        let worker_name: Option<String> = worker_name.map(|x| x.0);
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
            component_id: ComponentId(component_dependency_key.component_id),
            worker_name,
            function_name,
            function_params,
            idempotency_key,
            invocation_context,
            namespace,
        };

        let tav_opt = executor.execute(worker_request).await.map(|v| v.result)?;

        tav_opt
            .map(|tav| ValueAndType::try_from(tav).map_err(|x| x.into()))
            .transpose()
    }
}
