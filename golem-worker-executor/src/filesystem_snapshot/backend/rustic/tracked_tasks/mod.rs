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

use crate::filesystem::blocking::BlockingTask;
use crate::filesystem_snapshot::operation_drain::OperationLease;
use std::path::Path;
use tokio::runtime::Handle;
use tokio::task::JoinHandle;
use tokio_util::task::TaskTracker;
use tokio_util::task::task_tracker::TaskTrackerToken;

/// Keeps cleanup registered until its retained tasks enter the store tracker, and retains the
/// runtime and incarnation lease that those tasks need. A drop never looks up a runtime.
pub(super) struct CleanupTasks {
    tracker: TaskTracker,
    runtime: Handle,
    lease: OperationLease,
    _tracked: TaskTrackerToken,
}

impl CleanupTasks {
    pub(super) fn new(
        tracker: TaskTracker,
        runtime: Handle,
        lease: OperationLease,
        tracked: TaskTrackerToken,
    ) -> Self {
        Self {
            tracker,
            runtime,
            lease,
            _tracked: tracked,
        }
    }

    /// Runs the work as a task that the tracker and the work of the incarnation count.
    pub(super) fn spawn(&self, task: impl Future<Output = ()> + Send + 'static) -> JoinHandle<()> {
        let lease = self.lease.clone();
        self.tracker.spawn_on(
            async move {
                let _lease = lease;
                task.await
            },
            &self.runtime,
        )
    }
}

/// Runs the task on a blocking thread of the runtime.
pub(super) async fn run_blocking<R: Send + 'static>(
    task: impl FnOnce() -> anyhow::Result<R> + Send + 'static,
) -> anyhow::Result<R> {
    BlockingTask::Spawn.run(task).await?
}

/// Tracks a local path check through shutdown, including after its caller drops. These read-only
/// checks belong to the store tracker, not the incarnation drain cohort.
pub(super) async fn check_local_path(
    tracker: &TaskTracker,
    path: &Path,
    check: impl FnOnce(&Path) -> std::io::Result<()> + Send + 'static,
) -> std::io::Result<()> {
    let path: Box<Path> = path.into();
    let tracked = tracker.token();
    BlockingTask::Spawn
        .run(move || {
            let _tracked = tracked;
            check(&path)
        })
        .await
        .map_err(std::io::Error::other)?
}
