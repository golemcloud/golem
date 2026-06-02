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

/// Panic-on-use stub for `WorkerExecutorCluster`. Used in cloud mode where
/// direct executor process management is not available.
///
/// Operational methods panic. Lifecycle teardown methods (`kill_all`,
/// `restart_all`) are no-ops so that `BenchmarkTestDependencies::kill_all()`
/// completes safely. `is_running()` returns `true` so that
/// `ensure_all_deps_running()` is also a no-op rather than a panic.
pub struct PanicWorkerExecutorCluster;

#[async_trait]
impl WorkerExecutorCluster for PanicWorkerExecutorCluster {
    fn size(&self) -> usize {
        panic!("worker_executor_cluster() is not available in cloud mode");
    }

    async fn kill_all(&self) {
        // no-op: cloud mode has no local executor processes to kill
    }

    async fn restart_all(&self) {
        // no-op: cloud mode has no local executor processes to restart
    }

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

    /// Returns `true` so that `ensure_all_deps_running()` is a no-op in
    /// cloud mode — the cloud cluster is presumed healthy by the operator.
    async fn is_running(&self) -> bool {
        true
    }
}
