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

use crate::components::component_service::ComponentService;
use crate::components::redis::Redis;
use crate::components::shard_manager::ShardManager;
use crate::components::worker_executor::spawned::SpawnedWorkerExecutor;
use crate::components::worker_executor::WorkerExecutor;
use crate::components::worker_executor_cluster::WorkerExecutorCluster;
use crate::components::worker_service::WorkerService;
use async_trait::async_trait;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{info, Instrument, Level};

pub struct SpawnedWorkerExecutorCluster {
    worker_executors: Vec<Arc<dyn WorkerExecutor + Send + Sync + 'static>>,
    stopped_indices: Arc<Mutex<HashSet<usize>>>,
}

impl SpawnedWorkerExecutorCluster {
    async fn make_worker_executor(
        executable: PathBuf,
        working_directory: PathBuf,
        http_port: u16,
        grpc_port: u16,
        redis: Arc<dyn Redis + Send + Sync + 'static>,
        component_service: Arc<dyn ComponentService + Send + Sync + 'static>,
        shard_manager: Arc<dyn ShardManager + Send + Sync + 'static>,
        worker_service: Arc<dyn WorkerService + 'static>,
        verbosity: Level,
        out_level: Level,
        err_level: Level,
        shared_client: bool,
    ) -> Arc<dyn WorkerExecutor + Send + Sync + 'static> {
        Arc::new(
            SpawnedWorkerExecutor::new(
                &executable,
                &working_directory,
                http_port,
                grpc_port,
                redis,
                component_service,
                shard_manager,
                worker_service,
                verbosity,
                out_level,
                err_level,
                shared_client,
            )
            .await,
        )
    }

    pub async fn new(
        size: usize,
        base_http_port: u16,
        base_grpc_port: u16,
        executable: &Path,
        working_directory: &Path,
        redis: Arc<dyn Redis + Send + Sync + 'static>,
        component_service: Arc<dyn ComponentService + Send + Sync + 'static>,
        shard_manager: Arc<dyn ShardManager + Send + Sync + 'static>,
        worker_service: Arc<dyn WorkerService + 'static>,
        verbosity: Level,
        out_level: Level,
        err_level: Level,
        shared_client: bool,
    ) -> Self {
        Self::new_base(
            size,
            base_http_port,
            base_grpc_port,
            executable,
            working_directory,
            redis,
            component_service,
            shard_manager,
            worker_service,
            verbosity,
            out_level,
            err_level,
            shared_client,
        )
        .await
    }

    pub async fn new_base(
        size: usize,
        base_http_port: u16,
        base_grpc_port: u16,
        executable: &Path,
        working_directory: &Path,
        redis: Arc<dyn Redis + Send + Sync + 'static>,
        component_service: Arc<dyn ComponentService + Send + Sync + 'static>,
        shard_manager: Arc<dyn ShardManager + Send + Sync + 'static>,
        worker_service: Arc<dyn WorkerService + 'static>,
        verbosity: Level,
        out_level: Level,
        err_level: Level,
        shared_client: bool,
    ) -> Self {
        info!("Starting a cluster of golem-worker-executors of size {size}");
        let mut worker_executors_joins = Vec::new();

        for i in 0..size {
            let http_port = base_http_port + i as u16;
            let grpc_port = base_grpc_port + i as u16;

            let worker_executor_join = tokio::spawn(
                Self::make_worker_executor(
                    executable.to_path_buf(),
                    working_directory.to_path_buf(),
                    http_port,
                    grpc_port,
                    redis.clone(),
                    component_service.clone(),
                    shard_manager.clone(),
                    worker_service.clone(),
                    verbosity,
                    out_level,
                    err_level,
                    shared_client,
                )
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
impl WorkerExecutorCluster for SpawnedWorkerExecutorCluster {
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
