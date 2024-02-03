use crate::model::{GolemError, Pod};
use async_trait::async_trait;
use golem_api_grpc::proto::golem::workerexecutor::worker_executor_client::WorkerExecutorClient;
use std::time::Duration;
use poem_openapi::types::ToJSON;
use tonic::transport::Channel;
use golem_common::cache::{BackgroundEvictionMode, Cache, FullCacheEvictionMode, SimpleCache};


#[async_trait]
pub trait WorkerExecutorClients: Send + Sync {
    async fn lookup(&self, pod: &Pod) -> Result<WorkerExecutorClient<Channel>, String>;
}

pub struct WorkerExecutorClientsDefault {
    cache:  Cache<Pod, (), WorkerExecutorClient<Channel>, GolemError> ,
}

impl WorkerExecutorClientsDefault {
    pub fn new(max_capacity: usize, time_to_idle: Duration) -> WorkerExecutorClientsDefault {
        WorkerExecutorClientsDefault {
           cache: Cache::new(
            Some(max_capacity),
            FullCacheEvictionMode::LeastRecentlyUsed(1),
            BackgroundEvictionMode::OlderThan {
            ttl: time_to_idle,
            period: Duration::from_secs(60),
        },
        "worker_connect_client",
        )
        }
    }
}

#[async_trait]
impl WorkerExecutorClients for WorkerExecutorClientsDefault {
    async fn lookup(&self, pod: &Pod) -> Result<WorkerExecutorClient<Channel>, String> {
        self.cache.get_or_insert_simple(&pod.clone(), async {
            let uri: http_02::Uri = pod.uri().to_string().parse().unwrap();
            WorkerExecutorClient::connect(uri)
                .await
                .map_err(|e| e.to_string())?
        }).await.map_err(|err| err.to_json_string())

    }
}
