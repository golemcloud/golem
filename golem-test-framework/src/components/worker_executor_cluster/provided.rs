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
use crate::components::worker_executor::provided::ProvidedWorkerExecutor;
use crate::components::worker_executor_cluster::WorkerExecutorCluster;
use async_trait::async_trait;
use std::sync::Arc;
use tracing::info;

pub struct ProvidedWorkerExecutorCluster {
    workers: Vec<Arc<dyn WorkerExecutor>>,
}

impl ProvidedWorkerExecutorCluster {
    /// Single-member legacy constructor (kept for the existing benchmark
    /// caller that only knows about one already-running worker
    /// executor). Prefer [`Self::from_endpoints`] for new code so the
    /// reported cluster size matches reality.
    pub fn new(host: String, grpc_port: u16) -> Self {
        info!("Using an already running cluster of golem-worker-executors of size 1");
        Self::from_endpoints(vec![(host, grpc_port)])
    }

    /// Build a worker-side cluster handle that attaches to a
    /// parent-owned set of worker executors. Each entry of `endpoints`
    /// is a `(host, grpc_port)` pair pointing at one already-running
    /// worker executor in the parent's cluster.
    ///
    /// Used by `EnvBasedTestDependencies::from_descriptor` (Hosted
    /// reconstruction) so the worker side reports the correct cluster
    /// `size()` / `to_vec()` for tests that walk the cluster (the
    /// previous `size() == 1` hardcoding silently broke tests that
    /// expected the configured cluster size).
    pub fn from_endpoints(endpoints: Vec<(String, u16)>) -> Self {
        info!(
            "Attaching to an already-running cluster of {} golem-worker-executors",
            endpoints.len()
        );
        let workers = endpoints
            .into_iter()
            .map(|(host, port)| {
                let exec: Arc<dyn WorkerExecutor> =
                    Arc::new(ProvidedWorkerExecutor::new(host, port));
                exec
            })
            .collect();
        Self { workers }
    }
}

#[async_trait]
impl WorkerExecutorCluster for ProvidedWorkerExecutorCluster {
    fn size(&self) -> usize {
        self.workers.len()
    }

    async fn kill_all(&self) {
        // Worker-side cluster handles never own the underlying processes.
        // Tests that need to kill/restart/stop/start cluster members must
        // stay on a `Shared` (parent-owned) cluster handle; calling
        // these from a `Hosted` worker handle would either silently no-op
        // (the previous behaviour) or — once `ProvidedWorkerExecutor`
        // panics on `kill()` — only kill the worker subprocess's local
        // view, not the parent-owned process. Neither is safe, so
        // panic with an actionable message instead.
        panic!(
            "ProvidedWorkerExecutorCluster::kill_all is unsupported: \
             worker-side `Hosted` cluster handles cannot control \
             parent-owned worker-executor processes. Tests that need \
             lifecycle control must keep `EnvBasedTestDependencies` as \
             a `Shared` dep (or migrate via a future HostedRpc control plane)."
        );
    }

    async fn restart_all(&self) {
        panic!(
            "ProvidedWorkerExecutorCluster::restart_all is unsupported: \
             worker-side `Hosted` cluster handles cannot control \
             parent-owned worker-executor processes. Tests that need \
             lifecycle control must keep `EnvBasedTestDependencies` as \
             a `Shared` dep (or migrate via a future HostedRpc control plane)."
        );
    }

    async fn restart_all_with_extra_env_vars(&self, _extra_env_vars: Vec<(String, String)>) {
        panic!(
            "ProvidedWorkerExecutorCluster::restart_all_with_extra_env_vars is unsupported: \
             worker-side `Hosted` cluster handles cannot control \
             parent-owned worker-executor processes. Route lifecycle calls \
             through `WorkerExecutorClusterControlStub::restart_all_with_env_vars` \
             instead, which dispatches to the parent-owned cluster."
        );
    }

    async fn stop(&self, index: usize) {
        panic!(
            "ProvidedWorkerExecutorCluster::stop({index}) is unsupported: \
             worker-side `Hosted` cluster handles cannot control \
             parent-owned worker-executor processes. Tests that need \
             lifecycle control must keep `EnvBasedTestDependencies` as \
             a `Shared` dep (or migrate via a future HostedRpc control plane)."
        );
    }

    async fn start(&self, index: usize) {
        panic!(
            "ProvidedWorkerExecutorCluster::start({index}) is unsupported: \
             worker-side `Hosted` cluster handles cannot control \
             parent-owned worker-executor processes. Tests that need \
             lifecycle control must keep `EnvBasedTestDependencies` as \
             a `Shared` dep (or migrate via a future HostedRpc control plane)."
        );
    }

    fn to_vec(&self) -> Vec<Arc<dyn WorkerExecutor>> {
        self.workers.clone()
    }

    async fn stopped_indices(&self) -> Vec<usize> {
        // All parent-owned worker executors are assumed running; the
        // worker side has no authoritative view of stopped indices and
        // intentionally does not expose one.
        vec![]
    }

    async fn started_indices(&self) -> Vec<usize> {
        (0..self.workers.len()).collect()
    }

    async fn is_running(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_r::test;

    fn cluster() -> ProvidedWorkerExecutorCluster {
        ProvidedWorkerExecutorCluster::from_endpoints(vec![
            ("h1".to_string(), 9091),
            ("h2".to_string(), 9092),
            ("h3".to_string(), 9093),
        ])
    }

    #[test]
    async fn legacy_single_constructor_reports_size_one() {
        let c = ProvidedWorkerExecutorCluster::new("h0".to_string(), 9090);
        assert_eq!(c.size(), 1);
        assert_eq!(c.to_vec().len(), 1);
        assert!(c.is_running().await);
        assert!(c.stopped_indices().await.is_empty());
        assert_eq!(c.started_indices().await, vec![0]);
    }

    #[test]
    async fn from_endpoints_preserves_size_and_indices() {
        let c = cluster();
        assert_eq!(c.size(), 3);
        assert_eq!(c.to_vec().len(), 3);
        assert!(c.is_running().await);
        assert!(c.stopped_indices().await.is_empty());
        assert_eq!(c.started_indices().await, vec![0, 1, 2]);
    }

    #[test]
    #[should_panic(expected = "kill_all is unsupported")]
    async fn kill_all_panics_on_worker_side() {
        cluster().kill_all().await;
    }

    #[test]
    #[should_panic(expected = "restart_all is unsupported")]
    async fn restart_all_panics_on_worker_side() {
        cluster().restart_all().await;
    }

    #[test]
    #[should_panic(expected = "stop(1) is unsupported")]
    async fn stop_panics_on_worker_side() {
        cluster().stop(1).await;
    }

    #[test]
    #[should_panic(expected = "start(2) is unsupported")]
    async fn start_panics_on_worker_side() {
        cluster().start(2).await;
    }
}
