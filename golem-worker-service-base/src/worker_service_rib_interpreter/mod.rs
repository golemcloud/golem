use async_trait::async_trait;
use futures_util::FutureExt;
use std::fmt::Display;
use std::sync::Arc;

use golem_wasm_rpc::protobuf::type_annotated_value::TypeAnnotatedValue;

use golem_common::model::{ComponentId, IdempotencyKey};

use crate::worker_binding::RibInputValue;
use rib::{RibByteCode, RibFunctionInvoke, RibInterpreterResult};

use crate::worker_bridge_execution::{
    NoopWorkerRequestExecutor, WorkerRequest, WorkerRequestExecutor,
};

// A wrapper service over original RibInterpreter concerning
// the details of the worker service.
#[async_trait]
pub trait WorkerServiceRibInterpreter {
    async fn evaluate(
        &self,
        worker_name: &str,
        component_id: &ComponentId,
        idempotency_key: &Option<IdempotencyKey>,
        rib_byte_code: &RibByteCode,
        rib_input: &RibInputValue,
    ) -> Result<RibInterpreterResult, EvaluationError>;

    async fn evaluate_pure(
        &self,
        expr: &RibByteCode,
        rib_input: &RibInputValue,
    ) -> Result<RibInterpreterResult, EvaluationError>;
}

#[derive(Debug, PartialEq)]
pub struct EvaluationError(pub String);

impl Display for EvaluationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<String> for EvaluationError {
    fn from(err: String) -> Self {
        EvaluationError(err)
    }
}

pub struct DefaultEvaluator {
    worker_request_executor: Arc<dyn WorkerRequestExecutor + Sync + Send>,
}

impl DefaultEvaluator {
    pub fn noop() -> Self {
        DefaultEvaluator {
            worker_request_executor: Arc::new(NoopWorkerRequestExecutor),
        }
    }

    pub fn from_worker_request_executor(
        worker_request_executor: Arc<dyn WorkerRequestExecutor + Sync + Send>,
    ) -> Self {
        DefaultEvaluator {
            worker_request_executor,
        }
    }
}

#[async_trait]
impl WorkerServiceRibInterpreter for DefaultEvaluator {
    async fn evaluate(
        &self,
        worker_name: &str,
        component_id: &ComponentId,
        idempotency_key: &Option<IdempotencyKey>,
        expr: &RibByteCode,
        rib_input: &RibInputValue,
    ) -> Result<RibInterpreterResult, EvaluationError> {
        let executor = self.worker_request_executor.clone();

        let worker_name = worker_name.to_string();
        let component_id = component_id.clone();
        let idempotency_key = idempotency_key.clone();

        let worker_invoke_function: RibFunctionInvoke = Arc::new(
            move |function_name: String, parameters: Vec<TypeAnnotatedValue>| {
                let worker_name = worker_name.to_string();
                let component_id = component_id.clone();
                let worker_name = worker_name.clone();
                let idempotency_key = idempotency_key.clone();
                let executor = executor.clone();

                async move {
                    let worker_request = WorkerRequest {
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
            },
        );
        rib::interpret(expr, rib_input.value.clone(), worker_invoke_function)
            .await
            .map_err(EvaluationError)
    }

    async fn evaluate_pure(
        &self,
        expr: &RibByteCode,
        rib_input: &RibInputValue,
    ) -> Result<RibInterpreterResult, EvaluationError> {
        let worker_invoke_function: RibFunctionInvoke = Arc::new(|_, _| {
            Box::pin(
                async move {
                    Err("Worker invoke function is not allowed in pure evaluation".to_string())
                }
                .boxed(),
            )
        });

        rib::interpret(expr, rib_input.value.clone(), worker_invoke_function)
            .await
            .map_err(EvaluationError)
    }
}
