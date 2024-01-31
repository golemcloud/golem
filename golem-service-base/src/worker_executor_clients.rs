use async_trait::async_trait;
use golem_api_grpc::proto::golem::workerexecutor::worker_executor_client::WorkerExecutorClient;
use tonic::transport::Channel;

use crate::model::Pod;

#[async_trait]
pub trait WorkerExecutorClients {
    async fn lookup(&self, pod: &Pod) -> Result<WorkerExecutorClient<Channel>, String>;
}

#[derive(Default)]
pub struct WorkerExecutorClientsDefault {}

// TODO caching
#[async_trait]
impl WorkerExecutorClients for WorkerExecutorClientsDefault {
    async fn lookup(&self, pod: &Pod) -> Result<WorkerExecutorClient<Channel>, String> {
        let client = WorkerExecutorClient::connect(pod.uri())
            .await
            .map_err(|e| e.to_string())?;
        Ok(client)
    }
}
