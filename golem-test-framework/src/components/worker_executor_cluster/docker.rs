// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use crate::components::redis::Redis;
use crate::components::shard_manager::ShardManager;
use crate::components::worker_executor::docker::DockerWorkerExecutor;
use crate::components::worker_executor::WorkerExecutor;
use crate::components::worker_executor_cluster::WorkerExecutorCluster;
use crate::components::worker_service::WorkerService;
use crate::components::{cloud_service::CloudService, component_service::ComponentService};
use async_trait::async_trait;
use std::collections::HashSet;
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{info, Instrument, Level};

pub struct DockerWorkerExecutorCluster {
    worker_executors: Vec<Arc<DockerWorkerExecutor>>,
    stopped_indices: Arc<Mutex<HashSet<usize>>>,
}

impl DockerWorkerExecutorCluster {
    async fn make_worker_executor(
        prefix: &str,
        redis: Arc<dyn Redis>,
        component_service: Arc<dyn ComponentService>,
        shard_manager: Arc<dyn ShardManager>,
        worker_service: Arc<dyn WorkerService>,
        verbosity: Level,
        shared_client: bool,
        cloud_service: Arc<dyn CloudService>,
    ) -> Arc<DockerWorkerExecutor> {
        Arc::new(
            DockerWorkerExecutor::new(
                prefix,
                redis,
                component_service,
                shard_manager,
                worker_service,
                verbosity,
                shared_client,
                cloud_service,
            )
            .await,
        )
    }

    pub async fn new(
        size: usize,
        unique_network_id: &str,
        redis: Arc<dyn Redis>,
        component_service: Arc<dyn ComponentService>,
        shard_manager: Arc<dyn ShardManager>,
        worker_service: Arc<dyn WorkerService>,
        verbosity: Level,
        shared_client: bool,
        cloud_service: Arc<dyn CloudService>,
    ) -> Self {
        info!("Starting a cluster of golem-worker-executors of size {size}");
        let mut worker_executors_joins = Vec::new();

        for _ in 0..size {
            let unique_network_id_clone = unique_network_id.to_string();
            let redis = redis.clone();
            let component_service = component_service.clone();
            let shard_manager = shard_manager.clone();
            let worker_service = worker_service.clone();
            let cloud_service = cloud_service.clone();
            let worker_executor_join = tokio::spawn(
                async move {
                    let unique_network_id_clone = unique_network_id_clone.clone();
                    Self::make_worker_executor(
                        &unique_network_id_clone,
                        redis.clone(),
                        component_service.clone(),
                        shard_manager.clone(),
                        worker_service.clone(),
                        verbosity,
                        shared_client,
                        cloud_service.clone(),
                    )
                    .await
                }
                .in_current_span(),
            );

            worker_executors_joins.push(worker_executor_join);
        }

        let mut worker_executors = Vec::new();

        for join in worker_executors_joins {
            worker_executors.push(join.await.expect("Failed to join"));
        }

        Self {
            worker_executors,
            stopped_indices: Arc::new(Mutex::new(HashSet::new())),
        }
    }
}

#[async_trait]
impl WorkerExecutorCluster for DockerWorkerExecutorCluster {
    fn size(&self) -> usize {
        self.worker_executors.len()
    }

    async fn kill_all(&self) {
        info!("Killing all worker executors");
        for worker_executor in &self.worker_executors {
            worker_executor.kill().await;
        }
    }

    async fn restart_all(&self) {
        info!("Restarting all worker executors");
        for worker_executor in &self.worker_executors {
            worker_executor.restart().await;
        }
    }

    async fn stop(&self, index: usize) {
        let mut stopped = self.stopped_indices.lock().await;
        if !stopped.contains(&index) {
            self.worker_executors[index].stop().await;
            stopped.insert(index);
        }
    }

    async fn start(&self, index: usize) {
        if self.stopped_indices().await.contains(&index) {
            self.worker_executors[index].start().await;
            self.stopped_indices.lock().await.remove(&index);
        }
    }

    fn to_vec(&self) -> Vec<Arc<dyn WorkerExecutor>> {
        self.worker_executors
            .iter()
            .map(|we| we.clone() as Arc<dyn WorkerExecutor>)
            .collect()
    }

    async fn stopped_indices(&self) -> Vec<usize> {
        self.stopped_indices.lock().await.iter().copied().collect()
    }

    async fn started_indices(&self) -> Vec<usize> {
        let all_indices = HashSet::from_iter(0..self.worker_executors.len());
        let stopped_indices = self.stopped_indices.lock().await;
        all_indices.difference(&stopped_indices).copied().collect()
    }
}
