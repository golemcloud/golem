use async_trait::async_trait;
use futures_util::FutureExt;
use std::fmt::Display;
use std::sync::Arc;

use golem_wasm_rpc::protobuf::type_annotated_value::TypeAnnotatedValue;

use golem_common::model::{ComponentId, IdempotencyKey};
use golem_common::SafeDisplay;
use rib::{RibByteCode, RibFunctionInvoke, RibInput, RibResult};

use crate::gateway_execution::{GatewayResolvedWorkerRequest, GatewayWorkerRequestExecutor};

// A wrapper service over original RibInterpreter concerning
// the details of the worker service.
#[async_trait]
pub trait WorkerServiceRibInterpreter {
    // Evaluate a Rib byte against a specific worker.
    // RibByteCode may have actual function calls.
    async fn evaluate(
        &self,
        worker_name: Option<&str>,
        component_id: &ComponentId,
        idempotency_key: &Option<IdempotencyKey>,
        rib_byte_code: &RibByteCode,
        rib_input: &RibInput,
    ) -> Result<RibResult, EvaluationError>;
}

#[derive(Debug, PartialEq)]
pub struct EvaluationError(pub String);

impl Display for EvaluationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl SafeDisplay for EvaluationError {
    fn to_safe_string(&self) -> String {
        self.0.clone()
    }
}


impl From<String> for EvaluationError {
    fn from(err: String) -> Self {
        EvaluationError(err)
    }
}

pub struct DefaultRibInterpreter {
    worker_request_executor: Arc<dyn GatewayWorkerRequestExecutor + Sync + Send>,
}

impl DefaultRibInterpreter {
    pub fn from_worker_request_executor(
        worker_request_executor: Arc<dyn GatewayWorkerRequestExecutor + Sync + Send>,
    ) -> Self {
        DefaultRibInterpreter {
            worker_request_executor,
        }
    }
}

#[async_trait]
impl WorkerServiceRibInterpreter for DefaultRibInterpreter {
    async fn evaluate(
        &self,
        worker_name: Option<&str>,
        component_id: &ComponentId,
        idempotency_key: &Option<IdempotencyKey>,
        expr: &RibByteCode,
        rib_input: &RibInput,
    ) -> Result<RibResult, EvaluationError> {
        let executor = self.worker_request_executor.clone();

        let worker_invoke_function: RibFunctionInvoke = Arc::new({
            let component_id = component_id.clone();
            let idempotency_key = idempotency_key.clone();
            let worker_name = worker_name.map(|s| s.to_string()).clone();

            move |function_name: String, parameters: Vec<TypeAnnotatedValue>| {
                let component_id = component_id.clone();
                let worker_name = worker_name.clone();
                let idempotency_key = idempotency_key.clone();
                let executor = executor.clone();

                async move {
                    let worker_request = GatewayResolvedWorkerRequest {
                        component_id,
                        worker_name,
                        function_name,
                        function_params: parameters,
                        idempotency_key,
                    };

                    executor
                        .execute(worker_request)
                        .await
                        .map(|v| v.result)
                        .map_err(|e| e.to_string())
                }
                .boxed() // This ensures the future is boxed with the correct type
            }
        });
        let result = rib::interpret(expr, rib_input, worker_invoke_function)
            .await
            .map_err(EvaluationError)?;
        Ok(result)
    }
}
