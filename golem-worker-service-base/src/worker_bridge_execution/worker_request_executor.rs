use crate::worker_bridge_execution::WorkerRequest;
use async_trait::async_trait;
use std::fmt::Display;
use crate::service::worker::TypedResult;

#[async_trait]
pub trait WorkerRequestExecutor<Response> {
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
