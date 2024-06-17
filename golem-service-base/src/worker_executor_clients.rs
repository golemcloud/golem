// Copyright 2024 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use async_trait::async_trait;
use golem_api_grpc::proto::golem::workerexecutor::worker_executor_client::WorkerExecutorClient;
use golem_common::cache::{BackgroundEvictionMode, Cache, FullCacheEvictionMode, SimpleCache};
use golem_common::model::Pod;
use std::time::Duration;
use tonic::transport::Channel;

#[async_trait]
pub trait WorkerExecutorClients {
    async fn lookup(&self, pod: Pod) -> Result<WorkerExecutorClient<Channel>, String>;
}

pub struct WorkerExecutorClientsDefault {
    cache: Cache<Pod, (), WorkerExecutorClient<Channel>, String>,
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
                "worker_executor_client",
            ),
        }
    }
}

#[async_trait]
impl WorkerExecutorClients for WorkerExecutorClientsDefault {
    async fn lookup(&self, pod: Pod) -> Result<WorkerExecutorClient<Channel>, String> {
        self.cache
            .get_or_insert_simple(&pod.clone(), || {
                Box::pin(async move {
                    let uri: http_02::Uri = pod.uri().to_string().parse().unwrap();
                    WorkerExecutorClient::connect(uri)
                        .await
                        .map_err(|e| e.to_string())
                })
            })
            .await
    }
}

pub struct WorkerExecutorClientsNoop {}

#[async_trait]
impl WorkerExecutorClients for WorkerExecutorClientsNoop {
    async fn lookup(&self, _pod: Pod) -> Result<WorkerExecutorClient<Channel>, String> {
        Err("Noop".to_string())
    }
}
