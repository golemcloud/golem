// Copyright 2024-2025 Golem Cloud
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

use crate::components::worker_executor::provided::ProvidedWorkerExecutor;
use crate::components::worker_executor::WorkerExecutor;
use crate::components::worker_executor_cluster::WorkerExecutorCluster;
use async_trait::async_trait;
use std::sync::Arc;
use tracing::info;

pub struct ProvidedWorkerExecutorCluster {
    worker_executor: Arc<dyn WorkerExecutor + Send + Sync + 'static>,
}

impl ProvidedWorkerExecutorCluster {
    pub fn new(host: String, http_port: u16, grpc_port: u16, shared_client: bool) -> Self {
        info!("Using an already running cluster of golem-worker-executors of size 1");
        let worker_executor =
            ProvidedWorkerExecutor::new(host, http_port, grpc_port, shared_client);
        Self {
            worker_executor: Arc::new(worker_executor),
        }
    }
}

#[async_trait]
impl WorkerExecutorCluster for ProvidedWorkerExecutorCluster {
    fn size(&self) -> usize {
        1
    }

    async fn kill_all(&self) {
        self.worker_executor.kill().await
    }

    async fn restart_all(&self) {
        self.worker_executor.restart().await
    }

    async fn stop(&self, _index: usize) {}

    async fn start(&self, _index: usize) {}

    fn to_vec(&self) -> Vec<Arc<dyn WorkerExecutor + Send + Sync + 'static>> {
        vec![self.worker_executor.clone()]
    }

    async fn stopped_indices(&self) -> Vec<usize> {
        vec![]
    }

    async fn started_indices(&self) -> Vec<usize> {
        vec![0]
    }
}
