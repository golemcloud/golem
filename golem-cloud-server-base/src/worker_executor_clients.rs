use async_trait::async_trait;
use golem_api_grpc::proto::golem::workerexecutor::worker_executor_client::WorkerExecutorClient;
use tonic::transport::Channel;
use std::sync::Arc;
use crate::model::Pod;
use dashmap::DashMap;

type WorkerExecutorCache = Arc<DashMap<Pod, WorkerExecutorClient<Channel>>>;

#[async_trait]
pub trait WorkerExecutorClients {
    async fn lookup(&self, pod: &Pod) -> Result<WorkerExecutorClient<Channel>, String>;
}

pub struct WorkerExecutorClientsDefault {
    cache: WorkerExecutorCache
}

impl WorkerExecutorClientsDefault {
    pub fn new() -> Self {
        Self {
            cache: Arc::new(DashMap::new()),
        }
    }
}

#[async_trait]
impl WorkerExecutorClients for WorkerExecutorClientsDefault {
    async fn lookup(&self, pod: &Pod) -> Result<WorkerExecutorClient<Channel>, String> {
        if let Some(client) = self.cache.get(pod) {
            return Ok(client.clone());
        }

        let client = WorkerExecutorClient::connect(pod.uri())
            .await
            .map_err(|e| e.to_string())?;

        self.cache.insert(pod.clone(), client.clone());

        Ok(client)
    }
}
