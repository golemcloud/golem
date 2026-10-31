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
use super::cpu_priority::LowPriority;
use super::repository::{RepositoryKey, open_existing};
use super::run_outcome::{Ended, RunEnd};
use super::tracked_tasks::{CleanupTasks, run_blocking};
use crate::filesystem_snapshot::agent_snapshots::AgentSnapshots;
use crate::filesystem_snapshot::clock::Clock;
use crate::filesystem_snapshot::operation_drain::OperationLease;
use anyhow::Context;
use golem_service_base::storage::blob::BlobStorage;
use rustic_core::{OpenStatus, Repository as RusticRepository};
use std::sync::Arc;
use std::time::Duration;
use tokio::runtime::Handle;
use tokio::sync::Semaphore;
use tokio_util::sync::{CancellationToken, DropGuard};
use tokio_util::task::TaskTracker;

/// Named borrowed inputs keep the operation independent of the store type. Construction does
/// not register work.
pub(super) struct Resources<'a> {
    pub(super) storage: &'a Arc<dyn BlobStorage>,
    pub(super) root: &'a CancellationToken,
    pub(super) tracker: &'a TaskTracker,
    pub(super) key: &'a RepositoryKey,
    pub(super) low_priority: LowPriority,
    pub(super) clock: &'a Arc<dyn Clock>,
    pub(super) deadline: Duration,
    pub(super) tries: u32,
    pub(super) read_slots: &'a Arc<Semaphore>,
}

pub(super) struct Operation<'a> {
    resources: Resources<'a>,
    lease: &'a OperationLease,
}

/// Owns cleanup in field order: an untransferred backend, then cancellation, then the token.
pub(super) struct BackendRun {
    /// Present until transfer into a task. The cancellation guard remains after transfer.
    backend: Option<Arc<BlobBackend>>,
    _guard: DropGuard,
    token: CancellationToken,
}

impl BackendRun {
    pub(super) fn token(&self) -> &CancellationToken {
        &self.token
    }

    /// Transfers the backend into its blocking task without retaining another backend owner.
    pub(super) fn take_backend(&mut self) -> Arc<BlobBackend> {
        self.backend
            .take()
            .expect("the run backend was already transferred")
    }
}

fn runtime() -> anyhow::Result<Handle> {
    Handle::try_current().context("a filesystem snapshot operation needs an async runtime")
}

impl<'a> Operation<'a> {
    pub(super) fn new(lease: &'a OperationLease, resources: Resources<'a>) -> Self {
        Self { resources, lease }
    }

    /// The incarnation registered by this operation's lease.
    pub(super) fn agent(&self) -> &AgentSnapshots {
        self.lease.agent()
    }

    pub(super) fn root(&self) -> &CancellationToken {
        self.resources.root
    }

    pub(super) fn deadline(&self) -> Duration {
        self.resources.deadline
    }

    pub(super) fn key(&self) -> &RepositoryKey {
        self.resources.key
    }

    pub(super) fn clock(&self) -> &Arc<dyn Clock> {
        self.resources.clock
    }

    pub(super) fn low_priority(&self) -> LowPriority {
        self.resources.low_priority
    }

    /// Registers cleanup before checking shutdown, and captures its runtime only after that check.
    /// The registration and lease-backed spawner remain one owner until cleanup is counted.
    pub(super) fn cleanup_tasks(&self) -> anyhow::Result<CleanupTasks> {
        let tracked = self.resources.tracker.token();
        if self.root().is_cancelled() {
            anyhow::bail!("the filesystem snapshot store is shut down");
        }
        Ok(CleanupTasks::new(
            self.resources.tracker.clone(),
            runtime()?,
            self.lease.clone(),
            tracked,
        ))
    }

    pub(super) fn run_token(&self) -> (CancellationToken, DropGuard) {
        let token = self.root().child_token();
        (token.clone(), token.drop_guard())
    }

    /// Starts a run with unmodified blob handles. Save configures its own handles before opening
    /// its backend because the per-run tracker and late-write recorder must count those handles.
    pub(super) fn begin_run(&self) -> Result<BackendRun, Ended> {
        let (token, guard) = self.run_token();
        let backend = self
            .backend(self.files(&token))
            .map_err(|error| Ended::new(RunEnd::Permanent, error))?;
        Ok(BackendRun {
            backend: Some(Arc::new(backend)),
            _guard: guard,
            token,
        })
    }

    pub(super) fn backend(&self, files: SnapshotFiles) -> anyhow::Result<BlobBackend> {
        Ok(BlobBackend::new(files, runtime()?, TREE_PACK_CACHE_LIMIT)
            .tracked_by(self.resources.tracker.token())
            .reading_under(self.resources.read_slots.clone()))
    }

    pub(super) async fn blocking<T: Send + 'static>(
        &self,
        task: impl FnOnce() -> anyhow::Result<T> + Send + 'static,
    ) -> anyhow::Result<T> {
        let tracked = (self.resources.tracker.token(), self.lease.clone());
        run_blocking(move || {
            let _tracked = tracked;
            task()
        })
        .await
    }

    pub(super) fn track<F: Future>(&self, future: F) -> impl Future<Output = F::Output> {
        self.resources.tracker.track_future(future)
    }

    pub(super) fn files(&self, token: &CancellationToken) -> SnapshotFiles {
        SnapshotFiles::new(
            self.resources.storage.clone(),
            self.agent().namespace().clone(),
            self.deadline(),
            token.clone(),
            self.resources.tracker.clone(),
        )
        .with_retry_stop(self.root().clone())
        .with_tries(self.resources.tries)
        .of_operation(self.lease)
    }

    pub(super) fn link_cancel(&self, cancel: &CancellationToken, token: &CancellationToken) {
        let (cancel, token, lease) = (cancel.clone(), token.clone(), self.lease.clone());
        self.resources.tracker.spawn(async move {
            let _lease = lease;
            tokio::select! {
                () = cancel.cancelled() => token.cancel(),
                () = token.cancelled() => {}
            }
        });
    }

    /// Opens and reads an existing repository in one tracked blocking task. The caller supplies
    /// its backend, so this does not start a run or change the backend's blob policy.
    pub(super) async fn read_existing<T: Send + 'static>(
        &self,
        backend: Arc<BlobBackend>,
        read: impl FnOnce(Option<RusticRepository<OpenStatus>>, &BlobBackend) -> anyhow::Result<T>
        + Send
        + 'static,
    ) -> anyhow::Result<T> {
        let key = self.key().clone();
        self.blocking(move || {
            let repository = open_existing(backend.clone(), &key)?;
            read(repository, &backend)
        })
        .await
    }
}

#[cfg(test)]
mod tests;
