use crate::model::Pod;
use async_trait::async_trait;
use golem_api_grpc::proto::golem::workerexecutor::worker_executor_client::WorkerExecutorClient;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;
use tonic::transport::Channel;

type WorkerExecutorCache = Arc<Mutex<HashMap<String, WorkerExecutorClient<Channel>>>>;

#[async_trait]
pub trait WorkerExecutorClients: Send + Sync {
    async fn lookup(&self, pod: &Pod) -> Result<WorkerExecutorClient<Channel>, String>;
}

pub struct WorkerExecutorClientsDefault {
    cache: WorkerExecutorCache,
}

impl WorkerExecutorClientsDefault {
    pub fn new() -> Self {
        Self {
            cache: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

#[async_trait]
impl WorkerExecutorClients for WorkerExecutorClientsDefault {
    async fn lookup(&self, pod: &Pod) -> Result<WorkerExecutorClient<Channel>, String> {
        let mut cache = self.cache.lock().await;

        if let Some(client) = cache.get(&pod.uri().to_string()) {
            return Ok(client.clone());
        }

        let client = WorkerExecutorClient::connect(pod.uri())
            .await
            .map_err(|e| e.to_string())?;

        cache.insert(pod.uri().to_string(), client.clone());

        Ok(client)
    }
}
