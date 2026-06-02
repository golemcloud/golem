// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
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

use crate::components::worker_executor::WorkerExecutor;
use crate::components::worker_executor_cluster::WorkerExecutorCluster;
use async_trait::async_trait;
use std::sync::Arc;

/// A `WorkerExecutorCluster` whose individual executors are not directly
/// reachable (e.g. cloud mode, where executors run inside the cluster).
///
/// Lifecycle teardown methods (`kill_all`, `restart_all`) are no-ops so that
/// `kill_all()` completes. `is_running()` returns `true` so that
/// `ensure_all_deps_running()` is a no-op. Per-executor operations panic with a
/// clear message.
pub struct UnavailableWorkerExecutorCluster;

#[async_trait]
impl WorkerExecutorCluster for UnavailableWorkerExecutorCluster {
    fn size(&self) -> usize {
        panic!("worker_executor_cluster() is not available in cloud mode");
    }

    async fn kill_all(&self) {}

    async fn restart_all(&self) {}

    async fn stop(&self, _index: usize) {
        panic!("worker_executor_cluster() is not available in cloud mode");
    }

    async fn start(&self, _index: usize) {
        panic!("worker_executor_cluster() is not available in cloud mode");
    }

    fn to_vec(&self) -> Vec<Arc<dyn WorkerExecutor>> {
        panic!("worker_executor_cluster() is not available in cloud mode");
    }

    async fn stopped_indices(&self) -> Vec<usize> {
        panic!("worker_executor_cluster() is not available in cloud mode");
    }

    async fn started_indices(&self) -> Vec<usize> {
        panic!("worker_executor_cluster() is not available in cloud mode");
    }

    async fn is_running(&self) -> bool {
        true
    }
}
