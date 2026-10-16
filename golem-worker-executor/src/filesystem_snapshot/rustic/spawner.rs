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

//! The runtime and the tracker of the work that the store runs as a task, so that the work also
//! ends when its caller stops waiting.

use crate::filesystem_snapshot::agent_work::OperationWork;
use tokio::runtime::Handle;
use tokio::task::JoinHandle;
use tokio_util::task::TaskTracker;

/// Runs work of the store as a task on a runtime that the store gave, and the tracker of the store
/// and the work of the incarnation of the operation count the task. The cleanup of a release or a
/// drop of a claim runs through it, so no drop looks up a runtime. When the runtime is gone, the
/// task does not run.
#[derive(Clone, Debug)]
pub(super) struct Spawner {
    pub(super) tracker: TaskTracker,
    pub(super) runtime: Handle,
    /// The work of the incarnation of the operation that made the spawner.
    pub(super) operation: OperationWork,
}

impl Spawner {
    /// Runs the work as a task that the tracker and the work of the incarnation count.
    pub(super) fn spawn(&self, work: impl Future<Output = ()> + Send + 'static) -> JoinHandle<()> {
        let operation = self.operation.clone();
        self.tracker.spawn_on(
            async move {
                let _operation = operation;
                work.await
            },
            &self.runtime,
        )
    }
}
