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

use crate::components::component_service::ComponentService;
use crate::components::k8s::{K8sNamespace, K8sRoutingType};
use crate::components::redis::Redis;
use crate::components::shard_manager::ShardManager;
use crate::components::worker_executor::k8s::K8sWorkerExecutor;
use crate::components::worker_executor::WorkerExecutor;
use crate::components::worker_executor_cluster::WorkerExecutorCluster;
use crate::components::worker_service::WorkerService;
use async_trait::async_trait;
use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;
use tracing::{info, Level};

pub struct K8sWorkerExecutorCluster {
    worker_executors: Vec<Arc<dyn WorkerExecutor + Send + Sync + 'static>>,
    stopped_indices: Arc<Mutex<HashSet<usize>>>,
}

impl K8sWorkerExecutorCluster {
    async fn make_worker_executor(
        idx: usize,
        namespace: K8sNamespace,
        routing_type: K8sRoutingType,
        redis: Arc<dyn Redis + Send + Sync + 'static>,
        component_service: Arc<dyn ComponentService + Send + Sync + 'static>,
        shard_manager: Arc<dyn ShardManager + Send + Sync + 'static>,
        worker_service: Arc<dyn WorkerService + Send + Sync + 'static>,
        verbosity: Level,
        timeout: Duration,
        service_annotations: Option<std::collections::BTreeMap<String, String>>,
        shared_client: bool,
    ) -> Arc<dyn WorkerExecutor + Send + Sync + 'static> {
        Arc::new(
            K8sWorkerExecutor::new(
                &namespace,
                &routing_type,
                idx,
                verbosity,
                redis,
                component_service,
                shard_manager,
                worker_service,
                timeout,
                service_annotations,
                shared_client,
            )
            .await,
        )
    }

    pub async fn new(
        size: usize,
        namespace: &K8sNamespace,
        routing_type: &K8sRoutingType,
        redis: Arc<dyn Redis + Send + Sync + 'static>,
        component_service: Arc<dyn ComponentService + Send + Sync + 'static>,
        shard_manager: Arc<dyn ShardManager + Send + Sync + 'static>,
        worker_service: Arc<dyn WorkerService + Send + Sync + 'static>,
        verbosity: Level,
        timeout: Duration,
        service_annotations: Option<std::collections::BTreeMap<String, String>>,
        shared_client: bool,
    ) -> Self {
        info!("Starting a cluster of golem-worker-executors of size {size}");
        let mut worker_executors_joins = Vec::new();

        for idx in 0..size {
            let worker_executor_join = tokio::spawn(Self::make_worker_executor(
                idx,
                namespace.clone(),
                routing_type.clone(),
                redis.clone(),
                component_service.clone(),
                shard_manager.clone(),
                worker_service.clone(),
                verbosity,
                timeout,
                service_annotations.clone(),
                shared_client,
            ));

            worker_executors_joins.push(worker_executor_join);
        }

        let mut worker_executors = Vec::new();

        for join in worker_executors_joins {
            worker_executors.push(join.await.expect("Failed to join."));
        }

        Self {
            worker_executors,
            stopped_indices: Arc::new(Mutex::new(HashSet::new())),
        }
    }
}

#[async_trait]
impl WorkerExecutorCluster for K8sWorkerExecutorCluster {
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
            self.worker_executors[index].kill().await;
            stopped.insert(index);
        }
    }

    async fn start(&self, index: usize) {
        if self.stopped_indices().await.contains(&index) {
            self.worker_executors[index].restart().await;
            self.stopped_indices.lock().await.remove(&index);
        }
    }

    fn to_vec(&self) -> Vec<Arc<dyn WorkerExecutor + Send + Sync + 'static>> {
        self.worker_executors.to_vec()
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
