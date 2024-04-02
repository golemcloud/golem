use crate::worker_bridge_execution::worker_response::WorkerResponse;
use crate::worker_bridge_execution::WorkerRequest;
use async_trait::async_trait;
use std::fmt::Display;

#[async_trait]
pub trait WorkerRequestExecutor<Response> {
    async fn execute(
        &self,
        resolved_worker_request: WorkerRequest,
    ) -> Result<WorkerResponse, WorkerRequestExecutorError>;
}

#[derive(Clone, Debug)]
pub struct WorkerRequestExecutorError(String);

impl Display for WorkerRequestExecutorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<&str> for WorkerRequestExecutorError {
    fn from(err: &str) -> Self {
        WorkerRequestExecutorError(err.to_string())
    }
}

impl From<String> for WorkerRequestExecutorError {
    fn from(err: String) -> Self {
        WorkerRequestExecutorError(err)
    }
}
