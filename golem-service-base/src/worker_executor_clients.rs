use crate::model::{GolemError, GolemErrorUnknown, Pod};
use async_trait::async_trait;
use golem_api_grpc::proto::golem::workerexecutor::worker_executor_client::WorkerExecutorClient;
use golem_common::cache::{BackgroundEvictionMode, Cache, FullCacheEvictionMode, SimpleCache};
use std::time::Duration;
use tonic::transport::Channel;

#[async_trait]
pub trait WorkerExecutorClients {
    async fn lookup(&self, pod: Pod) -> Result<WorkerExecutorClient<Channel>, GolemError>;
}

pub struct WorkerExecutorClientsDefault {
    cache: Cache<Pod, (), WorkerExecutorClient<Channel>, GolemError>,
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
            ),
        }
    }
}

#[async_trait]
impl WorkerExecutorClients for WorkerExecutorClientsDefault {
    async fn lookup(&self, pod: Pod) -> Result<WorkerExecutorClient<Channel>, GolemError> {
        self.cache
            .get_or_insert_simple(&pod.clone(), || {
                Box::pin(async move {
                    let uri: http_02::Uri = pod.uri().to_string().parse().unwrap();

                    WorkerExecutorClient::connect(uri).await.map_err(|e| {
                        GolemError::Unknown(GolemErrorUnknown {
                            details: e.to_string(),
                        })
                    })
                })
            })
            .await
    }
}
