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

//! The resources and registrations of one store operation and its runs.
//!
//! Construction borrows the operation lease and the store resources. It creates no tracker token
//! and captures no runtime. Each backend, spawned task and tracked future holds its own registration.

use super::blob_backend::{BlobBackend, TREE_PACK_CACHE_LIMIT};
use super::blob_io::SnapshotFiles;
use super::call_execution::{Ended, RunEnd};
use super::cpu_priority::LowPriority;
use super::repository::RepositoryKey;
use super::tracked_tasks::{Spawner, run_blocking};
use crate::filesystem_snapshot::agent_snapshots::AgentSnapshots;
use crate::filesystem_snapshot::clock::Clock;
use crate::filesystem_snapshot::operation_drain::OperationLease;
use anyhow::Context;
use golem_service_base::storage::blob::BlobStorage;
use std::sync::Arc;
use std::time::Duration;
use tokio::runtime::Handle;
use tokio::sync::Semaphore;
use tokio_util::sync::{CancellationToken, DropGuard};
use tokio_util::task::TaskTracker;
use tokio_util::task::task_tracker::TaskTrackerToken;

pub(super) struct Operation<'a> {
    pub(super) storage: &'a Arc<dyn BlobStorage>,
    pub(super) root: &'a CancellationToken,
    pub(super) tracker: &'a TaskTracker,
    pub(super) lease: &'a OperationLease,
    pub(super) key: &'a RepositoryKey,
    pub(super) low_priority: LowPriority,
    pub(super) clock: &'a Arc<dyn Clock>,
    pub(super) deadline: Duration,
    pub(super) tries: u32,
    pub(super) read_slots: &'a Arc<Semaphore>,
}

/// A run's token, cancellation guard and backend, in acquisition order. Bindings in this order
/// drop in reverse order; a retained task can own the backend for longer.
pub(super) type BackendRun = (CancellationToken, DropGuard, Arc<BlobBackend>);

fn runtime() -> anyhow::Result<Handle> {
    Handle::try_current().context("a filesystem snapshot operation needs an async runtime")
}

impl Operation<'_> {
    /// The incarnation registered by this operation's lease.
    pub(super) fn agent(&self) -> &AgentSnapshots {
        self.lease.agent()
    }

    pub(super) fn root(&self) -> &CancellationToken {
        self.root
    }

    pub(super) fn deadline(&self) -> Duration {
        self.deadline
    }

    pub(super) fn registration(&self) -> TaskTrackerToken {
        self.tracker.token()
    }

    pub(super) fn run_token(&self) -> (CancellationToken, DropGuard) {
        let token = self.root.child_token();
        (token.clone(), token.drop_guard())
    }

    /// Starts a run with unmodified blob handles. Save configures its own handles before opening
    /// its backend because the per-run tracker and late-write recorder must count those handles.
    pub(super) fn begin_run(&self) -> Result<BackendRun, Ended> {
        let (token, guard) = self.run_token();
        let backend = self
            .backend(self.files(&token))
            .map_err(|error| Ended::new(RunEnd::Permanent, error))?;
        Ok((token, guard, Arc::new(backend)))
    }

    pub(super) fn backend(&self, files: SnapshotFiles) -> anyhow::Result<BlobBackend> {
        Ok(BlobBackend::new(files, runtime()?, TREE_PACK_CACHE_LIMIT)
            .tracked_by(self.tracker.token())
            .reading_under(self.read_slots.clone()))
    }

    pub(super) fn spawner(&self) -> anyhow::Result<Spawner> {
        Ok(Spawner {
            tracker: self.tracker.clone(),
            runtime: runtime()?,
            operation: self.lease.clone(),
        })
    }

    pub(super) async fn blocking<T: Send + 'static>(
        &self,
        task: impl FnOnce() -> anyhow::Result<T> + Send + 'static,
    ) -> anyhow::Result<T> {
        let tracked = (self.tracker.token(), self.lease.clone());
        run_blocking(move || {
            let _tracked = tracked;
            task()
        })
        .await
    }

    pub(super) fn track<F: Future>(&self, future: F) -> impl Future<Output = F::Output> {
        self.tracker.track_future(future)
    }

    pub(super) fn files(&self, token: &CancellationToken) -> SnapshotFiles {
        SnapshotFiles::new(
            self.storage.clone(),
            self.agent().namespace().clone(),
            self.deadline,
            token.clone(),
            self.tracker.clone(),
        )
        .with_retry_stop(self.root.clone())
        .with_tries(self.tries)
        .of_operation(self.lease)
    }

    pub(super) fn link_cancel(&self, cancel: &CancellationToken, token: &CancellationToken) {
        let (cancel, token, lease) = (cancel.clone(), token.clone(), self.lease.clone());
        self.tracker.spawn(async move {
            let _lease = lease;
            tokio::select! {
                () = cancel.cancelled() => token.cancel(),
                () = token.cancelled() => {}
            }
        });
    }
}

#[cfg(test)]
mod tests;
