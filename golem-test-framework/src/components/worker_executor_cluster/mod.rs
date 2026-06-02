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

pub mod panic;
pub mod provided;
pub mod spawned;

#[async_trait]
pub trait WorkerExecutorCluster: Send + Sync {
    fn size(&self) -> usize;
    async fn kill_all(&self);
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
