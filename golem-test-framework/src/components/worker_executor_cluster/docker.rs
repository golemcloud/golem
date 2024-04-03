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

use crate::components::redis::Redis;
use crate::components::shard_manager::ShardManager;
use crate::components::template_service::TemplateService;
use crate::components::worker_executor::docker::DockerWorkerExecutor;
use crate::components::worker_executor::WorkerExecutor;
use crate::components::worker_executor_cluster::WorkerExecutorCluster;
use crate::components::worker_service::WorkerService;
use std::sync::Arc;
use tracing::{info, Level};

pub struct DockerWorkerExecutorCluster {
    worker_executors: Vec<Arc<dyn WorkerExecutor + Send + Sync + 'static>>,
}

impl DockerWorkerExecutorCluster {
    pub fn new(
        size: usize,
        base_http_port: u16,
        base_grpc_port: u16,
        docker: &'static testcontainers::clients::Cli,
        redis: Arc<dyn Redis + Send + Sync + 'static>,
        template_service: Arc<dyn TemplateService + Send + Sync + 'static>,
        shard_manager: Arc<dyn ShardManager + Send + Sync + 'static>,
        worker_service: Arc<dyn WorkerService + Send + Sync + 'static>,
        verbosity: Level,
    ) -> Self {
        info!("Starting a cluster of golem-worker-executors of size {size}");
        let mut worker_executors = Vec::new();

        for i in 0..size {
            let http_port = base_http_port + i as u16;
            let grpc_port = base_grpc_port + i as u16;

            let worker_executor: Arc<dyn WorkerExecutor + Send + Sync + 'static> =
                Arc::new(DockerWorkerExecutor::new(
                    http_port,
                    grpc_port,
                    docker,
                    redis.clone(),
                    template_service.clone(),
                    shard_manager.clone(),
                    worker_service.clone(),
                    verbosity,
                ));

            worker_executors.push(worker_executor);
        }

        Self { worker_executors }
    }
}

impl WorkerExecutorCluster for DockerWorkerExecutorCluster {
    fn count(&self) -> usize {
        self.worker_executors.len()
    }

    fn kill_all(&self) {
        for worker_executor in &self.worker_executors {
            worker_executor.kill();
        }
    }

    fn restart_all(&self) {
        for worker_executor in &self.worker_executors {
            worker_executor.restart();
        }
    }

    fn to_vec(&self) -> Vec<Arc<dyn WorkerExecutor + Send + Sync + 'static>> {
        self.worker_executors.iter().cloned().collect()
    }
}
