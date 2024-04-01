use std::fmt::Display;
use async_trait::async_trait;
use crate::worker_bridge::worker_response::WorkerResponse;
use crate::worker_bridge::WorkerRequest;

#[async_trait]
pub trait WorkerRequestExecutor<Response> {
    async fn execute(
        &self,
        resolved_worker_request: WorkerRequest,
    ) -> Result<WorkerResponse, WorkerRequestExecutorError>;
}

pub struct WorkerRequestExecutorError(String);

impl Display for WorkerRequestExecutorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<String> for WorkerRequestExecutorError {
    fn from(err: String) -> Self {
        WorkerRequestExecutorError(err)
    }
}