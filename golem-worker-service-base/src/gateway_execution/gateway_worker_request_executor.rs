use crate::gateway_execution::GatewayResolvedWorkerRequest;
use async_trait::async_trait;

use golem_wasm_rpc::protobuf::type_annotated_value::TypeAnnotatedValue;
use std::fmt::Display;

// How to execute a worker-request that's formed
// as part of gateway
#[async_trait]
pub trait GatewayWorkerRequestExecutor {
    async fn execute(
        &self,
        resolved_worker_request: GatewayResolvedWorkerRequest,
    ) -> Result<WorkerResponse, GatewayWorkerRequestExecutorError>;
}

// The result of a worker execution from worker-bridge,
// which is a combination of function metadata and the type-annotated-value representing the actual result
pub struct WorkerResponse {
    pub result: TypeAnnotatedValue,
}

impl WorkerResponse {
    pub fn new(result: TypeAnnotatedValue) -> Self {
        WorkerResponse { result }
    }
}

#[derive(Clone, Debug)]
pub struct GatewayWorkerRequestExecutorError(String);

impl Display for GatewayWorkerRequestExecutorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl<T: AsRef<str>> From<T> for GatewayWorkerRequestExecutorError {
    fn from(err: T) -> Self {
        GatewayWorkerRequestExecutorError(err.as_ref().to_string())
    }
}
