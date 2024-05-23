use crate::service::worker::TypedResult;
use crate::worker_bridge_execution::{RefinedWorkerResponse, WorkerRequest};
use async_trait::async_trait;
use golem_common::model::ComponentId;
use golem_service_base::model::FunctionResult;
use golem_wasm_ast::analysis::AnalysedFunction;
use golem_wasm_rpc::TypeAnnotatedValue;
use std::fmt::Display;

#[async_trait]
pub trait WorkerRequestExecutor {
    async fn execute(
        &self,
        resolved_worker_request: WorkerRequest,
    ) -> Result<WorkerResponse, WorkerRequestExecutorError>;
}

// The result of a worker execution from worker-bridge,
// which is a combination of function metadata and the type-annotated-value representing the actual result
pub struct WorkerResponse {
    pub result: TypedResult,
}

impl WorkerResponse {
    pub fn new(result: TypeAnnotatedValue, function_result_types: Vec<FunctionResult>) -> Self {
        WorkerResponse {
            result: TypedResult {
                result,
                function_result_types,
            },
        }
    }
}

impl WorkerResponse {
    pub fn refined(&self) -> Result<RefinedWorkerResponse, String> {
        RefinedWorkerResponse::from_worker_response(self)
    }
}

#[derive(Clone, Debug)]
pub struct WorkerRequestExecutorError(String);

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

pub struct NoopWorkerRequestExecutor;

#[async_trait]
impl WorkerRequestExecutor for NoopWorkerRequestExecutor {
    async fn execute(
        &self,
        _worker_request_params: WorkerRequest,
    ) -> Result<WorkerResponse, WorkerRequestExecutorError> {
        Err(WorkerRequestExecutorError(
            "NoopWorkerRequestExecutor".to_string(),
        ))
    }
}
