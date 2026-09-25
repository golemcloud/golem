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
use async_trait::async_trait;
use std::sync::Arc;

pub mod provided;
pub mod spawned;
pub mod unavailable;

#[async_trait]
pub trait WorkerExecutorCluster: Send + Sync {
    fn size(&self) -> usize;
    /// Signal and reap every member, sharing one absolute deadline. All
    /// members are attempted even if another member fails.
    async fn kill_all_and_wait(&self, deadline: tokio::time::Instant) -> anyhow::Result<()> {
        let executors = self.to_vec();
        anyhow::ensure!(
            !executors.is_empty(),
            "kill-and-wait requires a spawned cluster"
        );
        let results = futures::future::join_all(
            executors
                .iter()
                .map(|executor| executor.kill_and_wait(deadline)),
        )
        .await;
        let errors: Vec<_> = results
            .into_iter()
            .enumerate()
            .filter_map(|(index, result)| {
                result
                    .err()
                    .map(|error| format!("executor {index}: {error:#}"))
            })
            .collect();
        anyhow::ensure!(errors.is_empty(), "{}", errors.join("; "));
        Ok(())
    }

    fn all_reaped(&self) -> bool {
        let executors = self.to_vec();
        !executors.is_empty() && executors.iter().all(|executor| executor.is_reaped())
    }

    async fn restart_all(&self);

    /// Restart every worker executor in the cluster with `extra_env_vars`
    /// merged into each spawned child process's environment **for this
    /// restart only**. Implementations must NOT mutate the parent test
    /// runner's process-wide environment.
    ///
    /// Default implementation panics: only `SpawnedWorkerExecutorCluster`
    /// supports this. The worker-side `Provided*` cluster is a worker-only
    /// view and can never control parent-owned executor processes anyway.
    async fn restart_all_with_extra_env_vars(&self, _extra_env_vars: Vec<(String, String)>) {
        panic!(
            "WorkerExecutorCluster::restart_all_with_extra_env_vars is only \
             supported by SpawnedWorkerExecutorCluster; the default \
             implementation refuses to silently discard the requested env \
             overrides."
        );
    }

    async fn stop(&self, index: usize);
    async fn start(&self, index: usize);

    fn to_vec(&self) -> Vec<Arc<dyn WorkerExecutor>>;

    async fn stopped_indices(&self) -> Vec<usize>;
    async fn started_indices(&self) -> Vec<usize>;

    async fn is_running(&self) -> bool;
}
