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

//! The concrete rustic snapshot adapter, its construction, shutdown and store-call routing.

mod blob_backend;
mod blob_io;
mod call_execution;
mod cpu_priority;
mod failure_classification;
mod pruning;
mod publication;
mod repository;
mod repository_layout;
mod repository_namespace;
mod restore;
mod save;
mod snapshot_catalog;
mod snapshot_deletion;
mod store_policy;
#[cfg(test)]
mod test_support;
#[cfg(test)]
mod tests;
mod tracked_tasks;

#[cfg(any(test, feature = "test-utils"))]
pub(crate) use call_execution::run_delay;

use crate::filesystem_snapshot::clock::{Clock, SystemClock};
use crate::filesystem_snapshot::operation_drain::{OperationDrain, OperationLease};
use crate::filesystem_snapshot::{
    AgentSnapshots, CallError, ChangeDetection, FilesystemSnapshotStore, ReadError, RestoreFailure,
    RunSlots, SaveError, SnapshotInfo, SnapshotName, Unlimited, Withdrawal,
};
use crate::services::golem_config::FilesystemSnapshotStoreConfig;
use anyhow::Context;
use async_trait::async_trait;
use blob_backend::{BlobBackend, KEPT_PACKS_LIMIT, MAX_SNAPSHOT_FILE_READS};
use blob_io::SnapshotFiles;
use call_execution::{Answers, Kind, OwnName, Shell};
use cpu_priority::LowPriority;
use golem_service_base::storage::blob::BlobStorage;
use repository::RepositoryKey;
use restore::destination::destination_is_valid;
#[cfg(test)]
use restore::destination::{ClearHook, NoHook};
use save::source_tree::tree_is_valid;
use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use store_policy::StorePolicy;
use tokio::runtime::Handle;
use tokio_util::sync::{CancellationToken, DropGuard};
use tokio_util::task::TaskTracker;
use tracked_tasks::{Spawner, run_blocking};

/// The storage call deadlines allowed by the publish/prune bound.
pub(in crate::filesystem_snapshot) const STORAGE_CALL_DEADLINES: std::ops::RangeInclusive<
    Duration,
> = publication::MIN_PUBLISH_TRY..=store_policy::MAX_STORAGE_CALL_DEADLINE;

/// A filesystem snapshot store that keeps one rustic repository for each scope in blob storage.
pub(crate) struct RusticSnapshotStore {
    storage: Arc<dyn BlobStorage>,
    key: RepositoryKey,
    policy: StorePolicy,
    /// The parent of the token of each run, and the stop of each wait between two tries.
    root: CancellationToken,
    /// Counts the blocking tasks, the checks of local paths, the backends, the blob calls of the
    /// store, the links of the cancels of saves, and the claims with their release and final-marker
    /// tasks.
    tracker: TaskTracker,
    /// Runs saves and prunes at a low priority.
    low_priority: LowPriority,
    /// Gives the wall time that the store compares with the times from storage, and the times that
    /// it writes.
    clock: Arc<dyn Clock>,
    /// The work of each incarnation in this process, which a delete of all its snapshots waits
    /// for.
    operations: OperationDrain,
    /// The slots of the reads ahead of snapshot files, which the backends of the store share.
    read_slots: Arc<tokio::sync::Semaphore>,
    /// The points of the clear of the directory of a restore where a test acts.
    #[cfg(test)]
    clear_hook: Arc<dyn ClearHook>,
    /// The failure of each prune that failed, for the tests.
    #[cfg(test)]
    failed_prunes: Arc<std::sync::Mutex<Vec<anyhow::Error>>>,
}

/// Gives the runtime of the operation that calls it.
fn runtime() -> anyhow::Result<Handle> {
    Handle::try_current().context("a filesystem snapshot operation needs an async runtime")
}

/// The answers of a call whose answer is a [`CallError`].
fn call_answers<T>() -> Answers<Result<T, CallError>> {
    Answers {
        failed: |failed| Err(CallError::Failed(failed)),
        stopped: |cause| Err(CallError::Stopped(cause)),
    }
}

/// The answers of a save.
fn save_answers() -> Answers<Result<SnapshotInfo, SaveError>> {
    Answers {
        failed: |failed| Err(SaveError::Failed(failed)),
        stopped: |cause| Err(SaveError::Stopped(cause)),
    }
}

/// The answers of a save to what the check of its own name found.
fn save_own_name() -> OwnName<Result<SnapshotInfo, SaveError>> {
    OwnName {
        saved: Ok,
        name_in_use: || Err(SaveError::NameInUse),
    }
}

/// The answers of a restore.
fn restore_answers() -> Answers<Result<SnapshotInfo, RestoreFailure>> {
    Answers {
        failed: |failed| Err(RestoreFailure::Failed(failed)),
        stopped: |cause| Err(RestoreFailure::Stopped(cause)),
    }
}

/// The answers of a `stat`.
fn read_answers() -> Answers<Result<Option<SnapshotInfo>, ReadError>> {
    Answers {
        failed: |failed| Err(ReadError::Failed(failed)),
        stopped: |_| Err(ReadError::Stopped),
    }
}

impl RusticSnapshotStore {
    /// Gives the store over the blob storage, with the key and the values of the configuration,
    /// and the clock of the host.
    pub(crate) fn new(
        storage: Arc<dyn BlobStorage>,
        config: &FilesystemSnapshotStoreConfig,
    ) -> Self {
        Self::with_policy(
            storage,
            RepositoryKey::new(*config.repository_key().bytes()),
            StorePolicy::from_config(config),
            Arc::new(SystemClock),
        )
    }

    /// Gives the store over the blob storage, with the key, the policy and the clock.
    fn with_policy(
        storage: Arc<dyn BlobStorage>,
        key: RepositoryKey,
        policy: StorePolicy,
        clock: Arc<dyn Clock>,
    ) -> Self {
        // The global rayon pool starts at its first use, and its threads keep the priority of the
        // thread that starts it. It starts here, at the normal priority, before a save or a prune.
        let _ = rayon::current_num_threads();
        let low_priority = LowPriority::new(policy.save_threads);
        Self {
            storage,
            key,
            policy,
            root: CancellationToken::new(),
            tracker: TaskTracker::new(),
            low_priority,
            clock,
            operations: OperationDrain::default(),
            read_slots: Arc::new(tokio::sync::Semaphore::new(MAX_SNAPSHOT_FILE_READS)),
            #[cfg(test)]
            clear_hook: Arc::new(NoHook),
            #[cfg(test)]
            failed_prunes: Arc::default(),
        }
    }

    /// Takes the failure of each prune that failed since the last take.
    #[cfg(test)]
    fn take_failed_prunes(&self) -> Vec<anyhow::Error> {
        std::mem::take(
            &mut *self
                .failed_prunes
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        )
    }

    /// Gives the store with the slots of the reads ahead of snapshot files `slots`.
    #[cfg(test)]
    fn reading_under(self, slots: Arc<tokio::sync::Semaphore>) -> Self {
        Self {
            read_slots: slots,
            ..self
        }
    }

    /// Gives the store with the points of the clear where `hook` acts.
    #[cfg(test)]
    fn with_clear_hook(self, hook: Arc<dyn ClearHook>) -> Self {
        Self {
            clear_hook: hook,
            ..self
        }
    }

    /// Cancels each run, so each running storage call ends and no new call starts, and later
    /// calls give `Stopped`. A claim is still released, or gets the final marker of its prune when
    /// the prune started, because no cancel ends those calls. The call waits until no blocking
    /// task, check of a local path, backend, blob call of the store, link of a cancel, claim, or
    /// release or final marker of a claim remains. A blob call that is not polled holds the wait
    /// until it is polled again, and then it ends at once. The runtime must not drop before it
    /// returns, because a storage call after its time driver stops aborts the process.
    pub(crate) async fn shut_down(&self) {
        self.root.cancel();
        self.tracker.close();
        self.tracker.wait().await;
    }

    /// Gives the number of blocking tasks, checks of local paths, backends, blob calls of the
    /// store, links of cancels, and claims with their release and final-marker tasks that have not
    /// ended.
    #[cfg(test)]
    fn work_in_flight(&self) -> usize {
        self.tracker.len()
    }

    /// Starts a run. The token of the run is cancelled when the guard drops.
    fn run_token(&self) -> (CancellationToken, DropGuard) {
        let token = self.root.child_token();
        (token.clone(), token.drop_guard())
    }

    /// Gives the shell of a call of the method `kind` with the limiter `slots`, and the cancel of a
    /// save.
    fn shell<'a>(
        &'a self,
        kind: Kind,
        slots: &'a dyn RunSlots,
        cancel: Option<&'a CancellationToken>,
    ) -> Shell<'a> {
        Shell {
            kind,
            slots,
            root: &self.root,
            cancel,
            retry: &self.policy.retry,
        }
    }

    /// Gives a backend over the repository of the blobs, whose calls follow the policy of the
    /// blobs.
    fn backend(&self, files: SnapshotFiles) -> anyhow::Result<BlobBackend> {
        Ok(BlobBackend::new(files, runtime()?, KEPT_PACKS_LIMIT)
            .tracked_by(self.tracker.token())
            .reading_under(self.read_slots.clone()))
    }

    /// Gives the spawner of the work that the store runs as a task, so that the work also ends when
    /// the caller of an operation stops waiting: the runtime of the operation, and the tracker of
    /// the store.
    fn spawner(&self, work: &OperationLease) -> anyhow::Result<Spawner> {
        Ok(Spawner {
            tracker: self.tracker.clone(),
            runtime: runtime()?,
            operation: work.clone(),
        })
    }

    /// Gives the share of a new operation of `scope` in the work of its incarnation.
    fn work(&self, scope: &AgentSnapshots) -> OperationLease {
        self.operations.begin(scope)
    }

    /// Gives a backend over the repository of the scope for the run with the token.
    fn scope_backend(
        &self,
        work: &OperationLease,
        scope: &AgentSnapshots,
        token: &CancellationToken,
    ) -> anyhow::Result<BlobBackend> {
        self.backend(self.files(work, scope, token))
    }

    /// Runs the task on a blocking thread that the tracker and the work `work` of the incarnation
    /// of the operation count.
    async fn blocking<T: Send + 'static>(
        &self,
        work: &OperationLease,
        task: impl FnOnce() -> anyhow::Result<T> + Send + 'static,
    ) -> anyhow::Result<T> {
        let tracked = (self.tracker.token(), work.clone());
        run_blocking(move || {
            let _tracked = tracked;
            task()
        })
        .await
    }

    /// Gives the blobs of the scope for the run with the token, which hold the work `work` of the
    /// incarnation of the operation. A wait between two tries of a call ends at the shutdown.
    fn files(
        &self,
        work: &OperationLease,
        scope: &AgentSnapshots,
        token: &CancellationToken,
    ) -> SnapshotFiles {
        SnapshotFiles::new(
            self.storage.clone(),
            (*scope.0).clone(),
            self.policy.deadline,
            token.clone(),
            self.tracker.clone(),
        )
        .with_retry_stop(self.root.clone())
        .with_tries(self.policy.in_call_tries())
        .of_work(work)
    }

    /// Links the cancel of a save to the token of a run, so a cancel of the save cancels the run.
    /// The link ends with the run, and the tracker counts it.
    fn link_cancel(
        &self,
        work: &OperationLease,
        cancel: &CancellationToken,
        token: &CancellationToken,
    ) {
        let (cancel, token, work) = (cancel.clone(), token.clone(), work.clone());
        self.tracker.spawn(async move {
            let _work = work;
            tokio::select! {
                () = cancel.cancelled() => token.cancel(),
                () = token.cancelled() => {}
            }
        });
    }
}

#[async_trait]
impl FilesystemSnapshotStore for RusticSnapshotStore {
    async fn save(
        &self,
        scope: &AgentSnapshots,
        name: &SnapshotName,
        tree: &Path,
        parent: Option<(&SnapshotName, ChangeDetection)>,
        cancel: &CancellationToken,
        slots: &dyn RunSlots,
    ) -> Result<SnapshotInfo, SaveError> {
        // The call takes its share of the work of the incarnation first, so a later delete of all
        // snapshots waits for it.
        let work = &self.work(scope);
        if self.root.is_cancelled() {
            return Err(SaveError::Stopped(Withdrawal::Stopped));
        }
        self.check_path(tree, tree_is_valid)
            .await
            .map_err(SaveError::Source)?;
        let parent = parent.map(|(parent, detection)| (parent.clone(), detection));
        self.shell(Kind::Save, slots, Some(cancel))
            .save(
                save_answers(),
                save_own_name(),
                |t0| self.policy.backup_end(t0),
                |start| self.save_run(work, scope, name, tree, parent.clone(), cancel, start.t0),
                |own| self.own_name_check(work, scope, name, own),
            )
            .await
    }

    async fn restore(
        &self,
        scope: &AgentSnapshots,
        name: &SnapshotName,
        into: &Path,
        slots: &dyn RunSlots,
    ) -> Result<SnapshotInfo, RestoreFailure> {
        // The call takes its share of the work of the incarnation first, so a later delete of all
        // snapshots waits for it.
        let work = &self.work(scope);
        if self.root.is_cancelled() {
            return Err(RestoreFailure::Stopped(Withdrawal::Stopped));
        }
        self.check_path(into, destination_is_valid)
            .await
            .map_err(RestoreFailure::Destination)?;
        self.shell(Kind::Restore, slots, None)
            .call(restore_answers(), |_| {
                self.restore_run(work, scope, name, into)
            })
            .await
    }

    async fn stat(
        &self,
        scope: &AgentSnapshots,
        name: &SnapshotName,
    ) -> Result<Option<SnapshotInfo>, ReadError> {
        if self.root.is_cancelled() {
            return Err(ReadError::Stopped);
        }
        let work = &self.work(scope);
        self.shell(Kind::Stat, &Unlimited, None)
            .call(read_answers(), |_| self.stat_run(work, scope, name))
            .await
    }

    async fn list(
        &self,
        scope: &AgentSnapshots,
        slots: &dyn RunSlots,
    ) -> Result<Box<[(SnapshotName, SnapshotInfo)]>, CallError> {
        let work = &self.work(scope);
        self.shell(Kind::List, slots, None)
            .call(call_answers(), |_| self.list_run(work, scope))
            .await
    }

    async fn delete(
        &self,
        scope: &AgentSnapshots,
        names: &[SnapshotName],
        slots: &dyn RunSlots,
    ) -> Result<(), CallError> {
        let names = names
            .iter()
            .map(|name| Box::<str>::from(name.as_str()))
            .collect::<HashSet<_>>();
        let names = &names;
        let work = &self.work(scope);
        self.shell(Kind::Delete, slots, None)
            .call(call_answers(), |_| self.delete_run(work, scope, names))
            .await
    }

    async fn delete_all(
        &self,
        scope: &AgentSnapshots,
        slots: &dyn RunSlots,
    ) -> Result<(), CallError> {
        // The drain comes before the first take, so the call holds no slot while it waits for the
        // work of the incarnation that began before it. A shutdown ends the wait.
        let work = &tokio::select! {
            biased;
            () = self.root.cancelled() => self.work(scope),
            work = self.operations.drain_before(scope) => work,
        };
        self.shell(Kind::DeleteAll, slots, None)
            .call(call_answers(), |_| self.delete_all_run(work, scope))
            .await
    }

    async fn copy_all(
        &self,
        from: &AgentSnapshots,
        to: &AgentSnapshots,
        slots: &dyn RunSlots,
    ) -> Result<(), CallError> {
        let works = &(self.work(from), self.work(to));
        self.shell(Kind::Copy, slots, None)
            .call(call_answers(), |start| {
                self.copy_run(works, from, to, start.number > 1)
            })
            .await
    }

    async fn shut_down(&self) {
        RusticSnapshotStore::shut_down(self).await
    }
}
