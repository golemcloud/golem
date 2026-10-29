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

//! The filesystem snapshot store over the rustic repositories of the scopes.
//!
//! Each call of the store runs one or more times through the shell of `runs`, and each run takes a
//! slot of the limiter of the call. A save makes the snapshot visible in one step: the backend
//! keeps the snapshot file of the backup, and the run writes that file after the blocking work
//! returns. Each run has a cancellation token that its drop cancels, so the threads of a dropped
//! run stop at their next storage call. The store counts each blocking task and each backend in a
//! task tracker, and [`RusticSnapshotStore::shut_down`] waits for them.

mod pruning;
mod restore;
mod save;
mod snapshots;

#[cfg(test)]
use pruning::leaves_marked_packs;
#[cfg(test)]
use save::{
    RunLate, SaveExit, ran_at_shutdown, save_failure, save_ran, save_settle, settle_may_land,
    whole_millis_from,
};
pub(super) use snapshots::named;
use snapshots::{has_name, lookup, scope_snapshots, scope_snapshots_except, snapshot_info};

use super::backend::{BlobBackend, KEPT_PACKS_LIMIT, MAX_SNAPSHOT_FILE_READS};
use super::claim::Claim;
use super::clear;
#[cfg(test)]
use super::clear::{ClearHook, NoHook};
use super::fault::{
    Phase, RestoreError, SaveFault, is_file_missing, is_index_missing, is_snapshot_missing,
    is_storage_failure, missing_file_path, restore_error, rustic_storage_end, save_fault,
    storage_end,
};
use super::files::{LateWrites, SnapshotFiles, lands_by, later};
use super::priority::LowPriority;
use super::prune::{
    DueCheck, Percent, PrunePolicy, belongs_to_ledger, due_prune, read_ledger, record_freed,
    refresh_period, remove_freed, remove_old_claims, remove_older_ledgers, write_ledger,
};
use super::publish::{
    PublishBound, Published, SnapshotStage, StagedSnapshot, backup_end, index_read_bound, publish,
};
use super::reload::{self, Found, Learned, Observed, Outcome, Reread, Step};
use super::runs::{
    Answers, Checked, Ended, Kind, LateWrite, OwnFile, OwnName, Ran, RunEnd, Settle, Settled,
    Shell, answered_after, copy_end,
};
use super::scope::{copy_scope, delete_scope};
use super::spawner::Spawner;
use super::{
    PruneReport, PruneSettings, RepositoryKey, backup_options, open_existing, open_or_create,
    prune, run_blocking,
};
use crate::blocking::BlockingTask;
use crate::filesystem_snapshot::agent_work::{
    AgentWorks, OperationWork, begin_operation, drain_agent,
};
use crate::filesystem_snapshot::clock::{Clock, SystemClock};
use crate::filesystem_snapshot::{
    AgentSnapshots, CallError, ChangeDetection, FilesystemSnapshotStore, ReadError, RestoreFailure,
    RunSlots, SaveError, SnapshotInfo, SnapshotName, Unlimited, Withdrawal, newest_first,
    snapshot_time,
};
use crate::metrics::filesystem_snapshots::{
    self, PruneOutcome, PruneSkip, RemovedSource, StoredSource,
};
use crate::services::golem_config::FilesystemSnapshotStoreConfig;
use anyhow::Context;
use async_trait::async_trait;
use futures::future::{self, Either};
use golem_common::model::{RetryConfig, Timestamp};
use golem_service_base::storage::blob::BlobStorage;
use rustic_core::jiff::tz::TimeZone;
use rustic_core::jiff::{Timestamp as SnapshotTime, Zoned};
use rustic_core::repofile::{IndexFile, IndexId, SnapshotFile, SnapshotId};
use rustic_core::{
    BackupOptions, DevIdOption, FileType, Id, IndexedFullStatus, LocalDestination,
    LocalSourceSaveOptions, LsOptions, Open, PathList, ReadBackend, Repository as RusticRepository,
    RestoreOptions, SnapshotOptions,
};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::num::NonZeroUsize;
use std::ops::ControlFlow;
use std::path::Path;
use std::pin::pin;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::runtime::Handle;
use tokio_util::sync::{CancellationToken, DropGuard};
use tokio_util::task::TaskTracker;
use tracing::warn;

/// The share of the size of the repository that deleted snapshots must free before a delete prunes
/// the scope. The threshold is 10% of the size, rounded down to a whole byte, so about 10%.
const PRUNE_THRESHOLD: Percent = Percent(10);

/// How long a pack that a prune marks stays before a later prune deletes it.
///
/// A save publishes its snapshot, and writes its index files, only before the earliest time at
/// which a prune can read the index that could delete a pack of the save. That time is this grace
/// period, less two storage call deadlines, less one refresh period of a claim, after the first
/// slot of the save. A prune stops its reads and deletes of snapshot data at most one refresh
/// period plus two deadlines after the start of its newest marker write that succeeded, so this
/// holds also when its refresh writes fail and when its executor stops. The clean-ups of its ledger
/// and its claim can run later, and they touch no snapshot data. So no snapshot resolves to a pack
/// that a prune deleted, and the bound needs only that clocks differ by less than the skew margin,
/// that a write or a delete lands within one deadline of the end of its call or never, and that the
/// host does not suspend. A restore needs no bound: it answers a pack that a prune removed during
/// it by reading the snapshots again, and it reads a pack that a prune only marked, because its
/// index holds the marked packs.
const PRUNE_GRACE: Duration = Duration::from_secs(15 * 60);

/// The longest storage call deadline that the store accepts: one eighth of [`PRUNE_GRACE`], so a
/// save has time for its backup before [`index_read_bound`].
pub(super) const MAX_STORAGE_CALL_DEADLINE: Duration =
    Duration::from_millis(PRUNE_GRACE.as_millis() as u64 / 8);

/// The settings of the store: the rustic settings of each operation, the prune threshold, and the
/// runs of a call after a failed storage call.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct StorePolicy {
    /// The longest time that one blob storage call waits for an answer, over all its tries.
    pub(super) deadline: Duration,
    /// The number of threads of each parallel stage of a save. `None` is the number of CPUs that
    /// the process can use.
    pub(super) save_threads: Option<NonZeroUsize>,
    /// The number of threads that read packs in a restore.
    pub(super) restore_reader_threads: NonZeroUsize,
    /// The settings of a prune. Two prunes never run at once. The next prune waits a full hold from
    /// the newest claim marker that was written.
    pub(super) prune: PruneSettings,
    /// The share of the size of the repository that deleted snapshots must free before a delete
    /// prunes.
    pub(super) prune_threshold: Percent,
    /// The runs of one call after a run whose storage call failed.
    pub(super) retry: RetryConfig,
    /// Whether a save lands its writes before the earliest index read of a prune.
    pub(super) publish_bound: PublishBound,
    /// The tries of each blob call that the store makes outside the claim protocol of a prune.
    #[cfg(test)]
    pub(super) in_call_tries: u32,
}

impl StorePolicy {
    /// Gives the policy with the values of the configuration.
    pub(super) fn from_config(config: &FilesystemSnapshotStoreConfig) -> Self {
        Self {
            deadline: config.storage_call_deadline(),
            save_threads: Some(config.save_threads()),
            restore_reader_threads: config.restore_reader_threads(),
            prune: PruneSettings {
                fast_repack: true,
                keep_delete: PRUNE_GRACE,
            },
            prune_threshold: PRUNE_THRESHOLD,
            retry: config.storage_retry().clone(),
            publish_bound: PublishBound::On,
            #[cfg(test)]
            in_call_tries: super::files::IN_CALL_TRIES,
        }
    }

    /// The tries of each blob call outside the claim protocol of a prune.
    fn in_call_tries(&self) -> u32 {
        #[cfg(test)]
        {
            self.in_call_tries
        }
        #[cfg(not(test))]
        {
            super::files::IN_CALL_TRIES
        }
    }

    /// The earliest index read of a prune that can delete a pack of a save whose first slot was at
    /// `t0`, or `None` when the bound is off.
    fn index_read_bound(&self, t0: Instant) -> Option<Instant> {
        match self.publish_bound {
            PublishBound::On => Some(index_read_bound(
                t0,
                self.prune.keep_delete,
                self.deadline,
                refresh_period(self.prune.keep_delete, self.deadline),
            )),
            #[cfg(test)]
            PublishBound::Off => None,
        }
    }

    /// The time after which no run of a save whose first slot was at `t0` starts its backup, or
    /// `None` when the bound is off.
    fn backup_end(&self, t0: Instant) -> Option<Instant> {
        self.index_read_bound(t0)
            .map(|bound| backup_end(bound, self.deadline))
    }
}

/// The options of a save of the store: a failed read of an entry fails the save, and no device id
/// is kept. `SizeMtime` compares each file with the parent that the id names. It compares the type,
/// the size and the modification time, and not the change time or the inode. `Full`, and a save
/// without a parent, use no parent, so they read every file.
fn store_backup_options(
    policy: &StorePolicy,
    parent: Option<(SnapshotId, ChangeDetection)>,
) -> BackupOptions {
    let base = backup_options(policy.save_threads)
        .fail_on_read_error(true)
        .ignore_save_opts(LocalSourceSaveOptions::default().set_devid(DevIdOption::No));
    let parent_opts = match parent {
        // rustic compares the inodes only when `ignore_inode` is true.
        Some((id, ChangeDetection::SizeMtime)) => base
            .parent_opts
            .clone()
            .parents(vec![id.to_hex().to_string()])
            .ignore_ctime(true)
            .ignore_inode(false),
        None | Some((_, ChangeDetection::Full)) => base.parent_opts.clone().force(true),
    };
    base.parent_opts(parent_opts)
}

/// The options of a restore of the store. A metadata error fails the restore. The restore does not
/// set the owner, because a snapshot does not keep it.
fn store_restore_options(policy: &StorePolicy) -> RestoreOptions {
    RestoreOptions::default()
        .reader_threads(Some(policy.restore_reader_threads))
        .fail_on_metadata_error(true)
        .no_ownership(true)
}

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
    works: AgentWorks,
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

/// Gives the end of a run whose step of rustic failed with `error`, when no other rule of the
/// operation applies: a failed storage call, a cancel, or an error that no new run changes.
fn ended_by(error: anyhow::Error) -> Ended {
    Ended::new(
        rustic_storage_end(&error).unwrap_or(RunEnd::Permanent),
        error,
    )
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
    pub(super) fn with_policy(
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
            works: AgentWorks::default(),
            read_slots: Arc::new(tokio::sync::Semaphore::new(MAX_SNAPSHOT_FILE_READS)),
            #[cfg(test)]
            clear_hook: Arc::new(NoHook),
            #[cfg(test)]
            failed_prunes: Arc::default(),
        }
    }

    /// Takes the failure of each prune that failed since the last take.
    #[cfg(test)]
    pub(super) fn take_failed_prunes(&self) -> Vec<anyhow::Error> {
        std::mem::take(
            &mut *self
                .failed_prunes
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        )
    }

    /// Gives the store with the slots of the reads ahead of snapshot files `slots`.
    #[cfg(test)]
    pub(super) fn reading_under(self, slots: Arc<tokio::sync::Semaphore>) -> Self {
        Self {
            read_slots: slots,
            ..self
        }
    }

    /// Gives the store with the points of the clear where `hook` acts.
    #[cfg(test)]
    pub(super) fn with_clear_hook(self, hook: Arc<dyn ClearHook>) -> Self {
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
    pub(super) fn work_in_flight(&self) -> usize {
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
    fn spawner(&self, work: &OperationWork) -> anyhow::Result<Spawner> {
        Ok(Spawner {
            tracker: self.tracker.clone(),
            runtime: runtime()?,
            operation: work.clone(),
        })
    }

    /// Gives the share of a new operation of `scope` in the work of its incarnation.
    fn work(&self, scope: &AgentSnapshots) -> OperationWork {
        begin_operation(&self.works, scope)
    }

    /// Gives a backend over the repository of the scope for the run with the token.
    fn scope_backend(
        &self,
        work: &OperationWork,
        scope: &AgentSnapshots,
        token: &CancellationToken,
    ) -> anyhow::Result<BlobBackend> {
        self.backend(self.files(work, scope, token))
    }

    /// Runs the task on a blocking thread that the tracker and the work `work` of the incarnation
    /// of the operation count.
    async fn blocking<T: Send + 'static>(
        &self,
        work: &OperationWork,
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
        work: &OperationWork,
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
        work: &OperationWork,
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

    /// One run of a delete of all snapshots.
    async fn delete_all_run(
        &self,
        work: &OperationWork,
        scope: &AgentSnapshots,
    ) -> Ran<Result<(), CallError>> {
        let (token, _guard) = self.run_token();
        match delete_scope(&self.files(work, scope, &token)).await {
            Ok(()) => Ran::Answered(Ok(())),
            Err(error) => Ran::Ended(Ended::new(storage_end(&error), error)),
        }
    }

    /// One run of a copy. `again` tells that an earlier run of the call ran. A write of the run
    /// that ended without an answer gives the instant after which it landed or never lands.
    async fn copy_run(
        &self,
        works: &(OperationWork, OperationWork),
        from: &AgentSnapshots,
        to: &AgentSnapshots,
        again: bool,
    ) -> Ran<Result<(), CallError>> {
        let (token, _guard) = self.run_token();
        let late = Arc::new(LateWrites::default());
        let copied = copy_scope(
            &self.files(&works.0, from, &token).recording(late.clone()),
            &self.files(&works.1, to, &token).recording(late.clone()),
            again,
        )
        .await;
        match copied {
            Ok(()) => answered_after(Ok(()), lands_by(late.latest(), self.policy.deadline)),
            Err(error) => Ran::Ended(Ended {
                end: copy_end(&error),
                failure: error.into_failure(),
                late: lands_by(late.latest(), self.policy.deadline)
                    .map(|until| LateWrite { until, own: None }),
            }),
        }
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
            work = drain_agent(&self.works, scope) => work,
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

/// What the tree of a snapshot holds. The store keeps it as the description of the snapshot,
/// because rustic counts a symlink as a file.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct TreeContent {
    files: u64,
    bytes: u64,
}

/// The snapshot files of a scope that rustic could read, and whether a file failed its integrity
/// check. A file that a delete removed after the listing is in neither.
struct ScopeSnapshots {
    readable: Vec<SnapshotFile>,
    unreadable: bool,
}

/// What a lookup of a name found.
enum Lookup {
    Found(Box<SnapshotFile>, SnapshotInfo),
    Missing,
    Corrupt(anyhow::Error),
}

impl RusticSnapshotStore {
    /// Runs the check of a local path on a blocking thread. A thread that fails gives the error of
    /// the path. The tracker counts the check until it ends, so `shut_down` waits for it, also when
    /// the operation drops first.
    async fn check_path(
        &self,
        path: &Path,
        check: impl FnOnce(&Path) -> std::io::Result<()> + Send + 'static,
    ) -> std::io::Result<()> {
        let path: Box<Path> = path.into();
        let tracked = self.tracker.token();
        BlockingTask::Spawn
            .run(move || {
                let _tracked = tracked;
                check(&path)
            })
            .await
            .map_err(std::io::Error::other)?
    }
}

fn tree_is_valid(tree: &Path) -> std::io::Result<()> {
    let metadata = std::fs::metadata(tree)?;
    if !tree.is_absolute() || !metadata.is_dir() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotADirectory,
            format!(
                "the tree {} is not a directory at an absolute path",
                tree.display()
            ),
        ));
    }
    Ok(())
}

fn destination_is_valid(into: &Path) -> std::io::Result<()> {
    let refused = |kind, reason: &str| {
        std::io::Error::new(kind, format!("the directory {} {reason}", into.display()))
    };
    let metadata = std::fs::metadata(into)?;
    if !metadata.is_dir() {
        return Err(refused(
            std::io::ErrorKind::NotADirectory,
            "is not a directory",
        ));
    }
    if into.to_str().is_none() {
        return Err(refused(
            std::io::ErrorKind::InvalidInput,
            "does not have a UTF-8 path",
        ));
    }
    let first = std::fs::read_dir(into)?.next().transpose()?;
    match first {
        Some(_) => Err(refused(
            std::io::ErrorKind::DirectoryNotEmpty,
            "is not empty",
        )),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests;
