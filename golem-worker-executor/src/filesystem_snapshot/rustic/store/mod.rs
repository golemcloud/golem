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

use super::backend::{BlobBackend, KEPT_PACKS_LIMIT, MAX_SNAPSHOT_FILE_READS};
use super::claim::Claim;
use super::clear;
#[cfg(test)]
use super::clear::{ClearHook, NoHook};
use super::fault::{
    Phase, RestoreError, SaveFault, is_file_missing, is_index_missing, is_snapshot_missing,
    is_storage_failure, missing_blob, missing_file_path, restore_error, rustic_storage_end,
    save_fault, storage_end,
};
use super::files::{LateWrites, SnapshotFiles};
use super::priority::LowPriority;
use super::prune::{
    Percent, PrunePolicy, belongs_to_ledger, due_prune, read_ledger, record_freed, refresh_period,
    remove_freed, remove_old_claims, remove_older_ledgers, write_ledger,
};
use super::publish::{
    PublishBound, Published, SnapshotStage, StagedSnapshot, backup_end, index_read_bound, publish,
};
use super::reload::{self, Found, Observed, Outcome, Reread, Step};
use super::runs::{
    Answers, Checked, Ended, Kind, LateWrite, OwnFile, Ran, RunEnd, Shell, copy_end,
};
use super::scope::{copy_scope, delete_scope};
use super::spawner::Spawner;
use super::{
    PruneReport, PruneSettings, RepositoryKey, backup_options, open_existing, open_or_create,
    prune, run_blocking,
};
use crate::filesystem_snapshot::clock::{Clock, SystemClock};
use crate::filesystem_snapshot::{
    AgentSnapshots, CallError, ChangeDetection, Failed, FilesystemSnapshotStore, ReadError,
    RestoreFailure, RunSlots, SaveError, SnapshotInfo, SnapshotName, Unlimited, Withdrawal,
    newest_first, snapshot_time,
};
use crate::sandbox_filesystem::{NativeOperation, NativeStorageProfile, execute_native};
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
/// it by reading the snapshots again, and a pack that a prune only marked with a `Failed` that a
/// later restore can clear.
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
    /// The slots of the reads ahead of snapshot files, which the backends of the store share.
    read_slots: Arc<tokio::sync::Semaphore>,
    /// The points of the clear of the directory of a restore where a test acts.
    #[cfg(test)]
    clear_hook: Arc<dyn ClearHook>,
    /// The failure of each prune that failed, for the tests.
    #[cfg(test)]
    failed_prunes: Arc<std::sync::Mutex<Vec<anyhow::Error>>>,
}

/// The number of times that a prune plans again when a snapshot file that it listed is gone at
/// its read.
const PRUNE_ATTEMPTS: usize = 3;

/// The most steps of one run of a restore. A run loads at most once more for each index file that
/// its listings give, so a run that reaches this bound found new index files at each step.
const MOST_RESTORE_STEPS: usize = 1024;

/// Gives the runtime of the operation that calls it.
fn runtime() -> anyhow::Result<Handle> {
    Handle::try_current().context("a filesystem snapshot operation needs an async runtime")
}

/// The answers of a call whose answer is a [`CallError`].
fn call_answers<T>() -> Answers<Result<T, CallError>> {
    Answers {
        failed: |failed| Err(CallError::Failed(failed)),
        stopped: |cause| Err(CallError::Stopped(cause)),
        saved: |_| {
            Err(CallError::Failed(Failed::new(anyhow::anyhow!(
                "the filesystem snapshot call checked a name that it did not write"
            ))))
        },
        name_in_use: || {
            Err(CallError::Failed(Failed::new(anyhow::anyhow!(
                "the filesystem snapshot call checked a name that it did not write"
            ))))
        },
    }
}

/// The answers of a save.
fn save_answers() -> Answers<Result<SnapshotInfo, SaveError>> {
    Answers {
        failed: |failed| Err(SaveError::Failed(failed)),
        stopped: |cause| Err(SaveError::Stopped(cause)),
        saved: Ok,
        name_in_use: || Err(SaveError::NameInUse),
    }
}

/// The answers of a restore.
fn restore_answers() -> Answers<Result<SnapshotInfo, RestoreFailure>> {
    Answers {
        failed: |failed| Err(RestoreFailure::Failed(failed)),
        stopped: |cause| Err(RestoreFailure::Stopped(cause)),
        saved: |_| {
            Err(RestoreFailure::Failed(Failed::new(anyhow::anyhow!(
                "a restore checked a name that it did not write"
            ))))
        },
        name_in_use: || {
            Err(RestoreFailure::Failed(Failed::new(anyhow::anyhow!(
                "a restore checked a name that it did not write"
            ))))
        },
    }
}

/// The answers of a `stat`.
fn read_answers() -> Answers<Result<Option<SnapshotInfo>, ReadError>> {
    Answers {
        failed: |failed| Err(ReadError::Failed(failed)),
        stopped: |_| Err(ReadError::Stopped),
        saved: |_| {
            Err(ReadError::Failed(Failed::new(anyhow::anyhow!(
                "a stat checked a name that it did not write"
            ))))
        },
        name_in_use: || {
            Err(ReadError::Failed(Failed::new(anyhow::anyhow!(
                "a stat checked a name that it did not write"
            ))))
        },
    }
}

/// The check of a call that writes no snapshot file. The shell asks it only after a publish.
async fn no_check(_own: OwnFile) -> Checked {
    Checked::Absent
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
    fn spawner(&self) -> anyhow::Result<Spawner> {
        Ok(Spawner {
            tracker: self.tracker.clone(),
            runtime: runtime()?,
        })
    }

    /// Gives the values of the prune decision from the policy of the store.
    fn prune_policy(&self) -> PrunePolicy {
        PrunePolicy {
            grace: self.policy.prune.keep_delete,
            deadline: self.policy.deadline,
            threshold: self.policy.prune_threshold,
        }
    }

    /// Gives a backend over the repository of the scope for the run with the token.
    fn scope_backend(
        &self,
        scope: &AgentSnapshots,
        token: &CancellationToken,
    ) -> anyhow::Result<BlobBackend> {
        self.backend(self.files(scope, token))
    }

    /// Runs the task on a blocking thread that the tracker counts.
    async fn blocking<T: Send + 'static>(
        &self,
        task: impl FnOnce() -> anyhow::Result<T> + Send + 'static,
    ) -> anyhow::Result<T> {
        let tracked = self.tracker.token();
        run_blocking(move || {
            let _tracked = tracked;
            task()
        })
        .await
    }

    /// Gives the blobs of the scope for the run with the token. A wait between two tries of a call
    /// ends at the shutdown.
    fn files(&self, scope: &AgentSnapshots, token: &CancellationToken) -> SnapshotFiles {
        SnapshotFiles::new(
            self.storage.clone(),
            (*scope.0).clone(),
            self.policy.deadline,
            token.clone(),
            self.tracker.clone(),
        )
        .with_retry_stop(self.root.clone())
        .with_tries(self.policy.in_call_tries())
    }

    /// Links the cancel of a save to the token of a run, so a cancel of the save cancels the run.
    /// The link ends with the run, and the tracker counts it.
    fn link_cancel(&self, cancel: &CancellationToken, token: &CancellationToken) {
        let (cancel, token) = (cancel.clone(), token.clone());
        self.tracker.spawn(async move {
            tokio::select! {
                () = cancel.cancelled() => token.cancel(),
                () = token.cancelled() => {}
            }
        });
    }

    /// Prunes the repository when a prune is due, and gives the failure of a prune that did not
    /// end. The records of freed bytes stay until a prune succeeds, so a later prune counts them
    /// again. It lists the packs only when their size can make a prune due. The claim protocol
    /// makes one try of each blob call, because a try that lost its answer can still land, and the
    /// protocol counts each such landing.
    ///
    /// A due prune runs only after the delete takes a claim of its ledger, and only when a second
    /// read of the ledger after the claim finds the time of the last prune of the first read. When
    /// it finds another time, the claim is released and the prune gives no error. So a delete
    /// prunes only with a claim in the claim directory of the ledger that the second read gives.
    /// After an error before the prune ran, the claim is released, so the next delete prunes
    /// again. A prune that found a snapshot file gone at each attempt changed nothing, so it counts
    /// as an error before the prune ran. After a prune that started, the claim stays on each
    /// outcome, also when the prune or its ledger write fails, or when the lease skips its ledger
    /// write. A prune that succeeds deletes each claim of its ledger.
    async fn prune_when_due(
        &self,
        scope: &AgentSnapshots,
        token: &CancellationToken,
    ) -> anyhow::Result<()> {
        let files = self.files(scope, token).once();
        let policy = self.prune_policy();
        let Some(due) = due_prune(&files, &*self.clock, &policy).await? else {
            return Ok(());
        };
        // The token of the tracker comes before the check of the cancel, so either the delete sees
        // a shut down and makes no more storage calls, or `shut_down` waits for the claim and for
        // each call that the claim makes.
        let tracked = self.tracker.token();
        if self.root.is_cancelled() {
            anyhow::bail!("the filesystem snapshot store is shut down");
        }
        let spawner = self.spawner()?;
        let Some(claim) = Claim::take(
            &files,
            due.claim,
            &policy,
            self.clock.clone(),
            spawner,
            tracked,
        )
        .await?
        else {
            return Ok(());
        };
        // Before the prune starts, an error releases the claim, so the next delete prunes again.
        // When the second read of the ledger finds another time of the last prune than the first
        // read, the claim is released too, and the prune gives no error. A prune that started can
        // have marked packs, so its claim stays.
        let backend = match self.prepare_prune(&files, &claim).await {
            Ok(Some(backend)) => backend,
            other => {
                claim.release().await;
                return other.map(|_| ());
            }
        };
        // No attempt of a prune that found a snapshot file gone changed the repository, so no
        // prune ran, and the claim goes.
        let pruned = match self.run_prune(backend, &files, &claim).await {
            Ok(None) => {
                claim.release().await;
                anyhow::bail!(
                    "a concurrent delete removed a snapshot file at each attempt of the prune"
                );
            }
            other => other.map(|marked| marked.unwrap_or_default()),
        };
        // The final marker holds the claim for the hold from the end of this prune, also when the
        // prune or its ledger write fails. It goes through the files of the claim, which no cancel
        // ends, so a prune that a shut down stopped also gets it.
        claim.finish().await;
        let marked_packs = pruned?;
        let ended = self.clock.now();
        // The lease fences the ledger write as it fences the calls of rustic. A write that would
        // start after the lease ran out is not sent, and a write that starts before is bounded by
        // the time left, so the entry never lands after another delete can take the claim over.
        // A prune whose ledger write the lease skipped keeps its claim and runs no cleanup.
        write_ledger(&claim.leased(&files), ended, marked_packs).await?;
        remove_older_ledgers(&files, ended).await;
        remove_freed(&files, &due.records).await;
        remove_old_claims(&files, ended).await;
        Ok(())
    }

    /// Checks the ledger again and builds the backend of the prune. It gives `None` when the second
    /// read finds another time of the last prune than the first read. So a prune runs only with a
    /// claim in the claim directory of the ledger that the second read gives.
    async fn prepare_prune(
        &self,
        files: &SnapshotFiles,
        claim: &Claim,
    ) -> anyhow::Result<Option<Arc<BlobBackend>>> {
        // A prune writes its ledger before it deletes the claims, so a delete that claims in a
        // directory that such a prune removed sees the new ledger here.
        let again = read_ledger(files, &*self.clock).await?;
        if !belongs_to_ledger(claim.directory(), &again) {
            return Ok(None);
        }
        Ok(Some(Arc::new(self.backend(claim.leased(files))?)))
    }

    /// Runs the prune with a new marker of the claim at each refresh period, and tells whether the
    /// prune leaves marked packs. It gives `None` when each attempt found a snapshot file gone.
    async fn run_prune(
        &self,
        backend: Arc<BlobBackend>,
        files: &SnapshotFiles,
        claim: &Claim,
    ) -> anyhow::Result<Option<bool>> {
        let key = self.key.clone();
        let settings = self.policy.prune;
        let low_priority = self.low_priority;
        let start = claim.start();
        // The plan of a prune reads each snapshot file before the prune changes the repository.
        // A forget of another delete can remove a listed file before its read, so the prune
        // plans again from a new listing.
        let pruning = self.blocking(move || {
            low_priority.run("fs-snap-prune", move || {
                // A delete that dropped before this point released the claim, so no prune runs.
                if !start.start() {
                    return Ok(None);
                }
                Ok(
                    std::iter::repeat_with(|| prune(backend.clone(), &key, &settings))
                        .take(PRUNE_ATTEMPTS)
                        .find(|attempt| {
                            !attempt
                                .as_ref()
                                .is_err_and(|error| is_snapshot_missing(&**error))
                        }),
                )
            })
        });
        // The claim gets a new marker while the prune runs, so a prune slower than one refresh
        // period keeps its claim. The markers stop when the prune ends or the operation is
        // cancelled. The tracker counts the whole step, so no timer of it runs after a shut down.
        let refreshing = claim.keep_fresh(files);
        let attempts = self
            .tracker
            .track_future(async {
                match future::select(pin!(pruning), pin!(refreshing)).await {
                    Either::Left((pruned, _)) => pruned,
                    Either::Right(((), pruning)) => pruning.await,
                }
            })
            .await?;
        attempts
            .map(|report| report.map(|report| report.as_ref().is_some_and(leaves_marked_packs)))
            .transpose()
    }

    /// One run of a save: the backup, then the publish of its snapshot file. A backup that has not
    /// returned at `backup_end` is cancelled, and the run publishes nothing.
    #[allow(clippy::too_many_arguments)]
    async fn save_run(
        &self,
        scope: &AgentSnapshots,
        name: &SnapshotName,
        tree: &Path,
        parent: Option<(SnapshotName, ChangeDetection)>,
        cancel: &CancellationToken,
        t0: Instant,
    ) -> Ran<Result<SnapshotInfo, SaveError>> {
        let (token, _guard) = self.run_token();
        self.link_cancel(cancel, &token);
        let stage = Arc::new(SnapshotStage::default());
        let backend = match self.scope_backend(scope, &token) {
            Ok(backend) => Arc::new(backend.staging_in(stage.clone())),
            Err(error) => return Ran::Ended(Ended::new(RunEnd::Permanent, error)),
        };
        let key = self.key.clone();
        let policy = self.policy.clone();
        let name = name.clone();
        let tree: Box<Path> = tree.into();
        let low_priority = self.low_priority;
        let clock = self.clock.clone();
        let mut backup = pin!(self.blocking(move || {
            low_priority.run("fs-snap-save", move || {
                Ok(stage_save_again_after_a_missing_index(
                    &backend, &stage, &key, &policy, &name, &tree, parent, &*clock,
                ))
            })
        }));
        let backup_end = self.policy.backup_end(t0);
        let staged = match backup_end {
            Some(end) => {
                tokio::select! {
                    staged = &mut backup => Some(staged),
                    () = tokio::time::sleep_until(tokio::time::Instant::from_std(end)) => None,
                }
            }
            None => Some((&mut backup).await),
        };
        let staged = match staged {
            Some(staged) => staged,
            None => {
                // The backup did not end in time: the run stops it, and waits until the store reads
                // the tree no more.
                token.cancel();
                let _ = backup.await;
                return Ran::Ended(Ended::new(
                    RunEnd::BoundPassed,
                    anyhow::anyhow!(
                        "the backup of the save would take longer than the filesystem snapshot store allows"
                    ),
                ));
            }
        };
        // The backup has returned, so the store reads the tree no more. A cancel before this
        // point publishes nothing, whatever the backup gave.
        if cancel.is_cancelled() {
            return Ran::Ended(Ended::new(
                RunEnd::Cancelled,
                anyhow::anyhow!(
                    "the save of the filesystem snapshot was cancelled before its publish"
                ),
            ));
        }
        let (staged, info) = match staged.and_then(|staged| staged) {
            Ok(Some(staged)) => staged,
            Ok(None) => return Ran::Answered(Err(SaveError::NameInUse)),
            Err(error) => return save_failure(error),
        };
        let bound = self.policy.index_read_bound(t0);
        match publish(&self.files(scope, &self.root), &staged, bound, cancel).await {
            Published::Written => Ran::Answered(Ok(info)),
            Published::NotWritten { end, failure, late } => Ran::Ended(Ended {
                end,
                failure,
                late: late.map(|until| LateWrite {
                    until,
                    own: Some(OwnFile {
                        path: staged.path.clone(),
                        info,
                    }),
                }),
            }),
        }
    }

    /// Checks the own name of a save after a publish that ended without an answer: the file that
    /// the last publish staged, another snapshot file with the name, no such file, or a check that
    /// could not read. It holds no slot and ends only at the shutdown.
    async fn own_name_check(
        &self,
        scope: &AgentSnapshots,
        name: &SnapshotName,
        own: OwnFile,
    ) -> Checked {
        let OwnFile { path, info } = own;
        let files = self.files(scope, &self.root);
        let listed = match files
            .list_below("check_name", Path::new(FileType::Snapshot.dirname()))
            .await
        {
            Ok(listed) => listed,
            Err(_) => return Checked::Unreadable,
        };
        if listed.iter().any(|blob| *blob.path == *path) {
            return Checked::Found(info);
        }
        let key = self.key.clone();
        let name = name.clone();
        let backend = match self.backend(files) {
            Ok(backend) => Arc::new(backend),
            Err(_) => return Checked::Unreadable,
        };
        let named = self
            .blocking(move || {
                Ok(match open_existing(backend.clone(), &key)? {
                    Some(repository) => has_name(&scope_snapshots(&repository, &backend)?, &name),
                    None => false,
                })
            })
            .await;
        match named {
            Ok(true) => Checked::Other,
            Ok(false) => Checked::Absent,
            Err(_) => Checked::Unreadable,
        }
    }

    /// One run of a restore, as [`reload::next_step`] decides each step.
    async fn restore_run(
        &self,
        scope: &AgentSnapshots,
        name: &SnapshotName,
        into: &Path,
    ) -> Ran<Result<SnapshotInfo, RestoreFailure>> {
        let (token, _guard) = self.run_token();
        let backend = match self.scope_backend(scope, &token) {
            Ok(backend) => Arc::new(backend),
            Err(error) => return Ran::Ended(Ended::new(RunEnd::Permanent, error)),
        };
        let run = RestoreRun {
            backend,
            key: self.key.clone(),
            name: name.clone(),
            into: into.into(),
            options: store_restore_options(&self.policy),
            #[cfg(test)]
            hook: self.clear_hook.clone(),
        };
        // The index load of a restore uses rayon, so the restore runs in a pool of its own, with
        // the reader threads of a restore. The blocking thread starts the threads of that pool, so
        // they have its normal priority.
        let pool = LowPriority {
            threads: Some(self.policy.restore_reader_threads),
            ..self.low_priority
        };
        self.blocking(move || pool.in_own_pool("fs-snap-restore", move || Ok(run.run())))
            .await
            .unwrap_or_else(|error| Ran::Ended(ended_by(error)))
    }

    /// One run of a `stat`.
    async fn stat_run(
        &self,
        scope: &AgentSnapshots,
        name: &SnapshotName,
    ) -> Ran<Result<Option<SnapshotInfo>, ReadError>> {
        let (token, _guard) = self.run_token();
        let backend = match self.scope_backend(scope, &token) {
            Ok(backend) => Arc::new(backend),
            Err(error) => return Ran::Ended(Ended::new(RunEnd::Permanent, error)),
        };
        let key = self.key.clone();
        let name = name.clone();
        match self
            .blocking(move || {
                Ok(match open_existing(backend.clone(), &key)? {
                    Some(repository) => lookup(scope_snapshots(&repository, &backend)?, &name),
                    None => Lookup::Missing,
                })
            })
            .await
        {
            Ok(Lookup::Found(_, info)) => Ran::Answered(Ok(Some(info))),
            Ok(Lookup::Missing) => Ran::Answered(Ok(None)),
            Ok(Lookup::Corrupt(error)) => Ran::Answered(Err(ReadError::Corrupt(error))),
            Err(error) => Ran::Ended(ended_by(error)),
        }
    }

    /// One run of a `list`.
    async fn list_run(
        &self,
        scope: &AgentSnapshots,
    ) -> Ran<Result<Box<[(SnapshotName, SnapshotInfo)]>, CallError>> {
        let (token, _guard) = self.run_token();
        let backend = match self.scope_backend(scope, &token) {
            Ok(backend) => Arc::new(backend),
            Err(error) => return Ran::Ended(Ended::new(RunEnd::Permanent, error)),
        };
        let key = self.key.clone();
        match self
            .blocking(move || {
                Ok(match open_existing(backend.clone(), &key)? {
                    Some(repository) => {
                        newest_first(listed(scope_snapshots(&repository, &backend)?))
                    }
                    None => Box::default(),
                })
            })
            .await
        {
            Ok(listed) => Ran::Answered(Ok(listed)),
            Err(error) => Ran::Ended(ended_by(error)),
        }
    }

    /// One run of a delete: the record of the freed bytes, the forget of the snapshot files, and
    /// then the prune when one is due. The run answers when the forget succeeded; a prune that
    /// fails gives a warning and a count, and the next delete prunes.
    async fn delete_run(
        &self,
        scope: &AgentSnapshots,
        names: &HashSet<Box<str>>,
    ) -> Ran<Result<(), CallError>> {
        let (token, _guard) = self.run_token();
        let backend = match self.scope_backend(scope, &token) {
            Ok(backend) => Arc::new(backend),
            Err(error) => return Ran::Ended(Ended::new(RunEnd::Permanent, error)),
        };
        let key = self.key.clone();
        let names = names.clone();
        let found = self
            .blocking(move || {
                let Some(repository) = open_existing(backend.clone(), &key)? else {
                    return Ok(None);
                };
                // One listing finds every snapshot of the batch.
                let named = scope_snapshots(&repository, &backend)?
                    .readable
                    .into_iter()
                    .filter(|snapshot| names.contains(snapshot.label.as_str()))
                    .collect::<Box<[_]>>();
                let ids = named
                    .iter()
                    .map(|snapshot| snapshot.id)
                    .collect::<Box<[SnapshotId]>>();
                let freed = named.iter().map(added_packed_bytes).sum::<u64>();
                Ok(Some((repository, ids, freed)))
            })
            .await;
        let (repository, ids, freed) = match found {
            Ok(Some(found)) => found,
            Ok(None) => return Ran::Answered(Ok(())),
            Err(error) => return Ran::Ended(ended_by(error)),
        };
        // The record comes before the forget, so a stop between the two cannot lose the bytes. A
        // forget that then fails ends the run, and the record stays, which only brings a prune
        // earlier.
        let files = self.files(scope, &token);
        if freed > 0 {
            let snapshots = ids
                .iter()
                .map(|id| id.to_hex().to_string().into_boxed_str())
                .collect::<Box<[_]>>();
            if let Err(error) = record_freed(&files, freed, &snapshots).await {
                return Ran::Ended(Ended::new(storage_end(&error), error));
            }
        }
        // The forget deletes the snapshot files with rayon, so it runs in a pool of its own. The
        // blocking thread starts the threads of that pool, so they have its normal priority.
        let pool = self.low_priority;
        if let Err(error) = self
            .blocking(move || {
                pool.in_own_pool("fs-snap-delete", move || {
                    repository.delete_snapshots(&ids)?;
                    Ok(())
                })
            })
            .await
        {
            return Ran::Ended(ended_by(error));
        }
        if let Err(error) = self.prune_when_due(scope, &token).await {
            warn!(
                snapshots = ?scope,
                error = %format!("{error:#}"),
                "A prune of the filesystem snapshots of an agent failed; the next delete prunes again"
            );
            crate::metrics::filesystem_snapshots::record_leaked_cleanup("prune");
            #[cfg(test)]
            self.failed_prunes
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(error);
        }
        Ran::Answered(Ok(()))
    }

    /// One run of a delete of all snapshots.
    async fn delete_all_run(&self, scope: &AgentSnapshots) -> Ran<Result<(), CallError>> {
        let (token, _guard) = self.run_token();
        match delete_scope(&self.files(scope, &token)).await {
            Ok(()) => Ran::Answered(Ok(())),
            Err(error) => Ran::Ended(Ended::new(storage_end(&error), error)),
        }
    }

    /// One run of a copy. `again` tells that an earlier run of the call ran. A write of the run
    /// that ended without an answer gives the instant after which it landed or never lands.
    async fn copy_run(
        &self,
        from: &AgentSnapshots,
        to: &AgentSnapshots,
        again: bool,
    ) -> Ran<Result<(), CallError>> {
        let (token, _guard) = self.run_token();
        let late = Arc::new(LateWrites::default());
        let copied = copy_scope(
            &self.files(from, &token).recording(late.clone()),
            &self.files(to, &token).recording(late.clone()),
            again,
        )
        .await;
        match copied {
            Ok(()) => match late.until(self.policy.deadline) {
                Some(until) => Ran::AnsweredAfter(Ok(()), LateWrite { until, own: None }),
                None => Ran::Answered(Ok(())),
            },
            Err(error) => Ran::Ended(Ended {
                end: copy_end(&error),
                failure: error.into_failure(),
                late: late
                    .until(self.policy.deadline)
                    .map(|until| LateWrite { until, own: None }),
            }),
        }
    }
}

/// Gives the run of a save whose backup failed with `error`: a failed storage call ends the run, a
/// local I/O error answers `Source`, and each other error cannot change.
fn save_failure(error: anyhow::Error) -> Ran<Result<SnapshotInfo, SaveError>> {
    match save_fault(&error) {
        SaveFault::Ended(end) => Ran::Ended(Ended::new(end, error)),
        SaveFault::Source(kind) => Ran::Answered(Err(SaveError::Source(std::io::Error::new(
            kind,
            format!("{error:#}"),
        )))),
    }
}

/// Runs [`stage_save`] again with an empty stage after a listed index file was gone at its read,
/// once for each such file. A file that is gone in two loads ends the backup with its error.
#[allow(clippy::too_many_arguments)]
fn stage_save_again_after_a_missing_index(
    backend: &Arc<BlobBackend>,
    stage: &SnapshotStage,
    key: &RepositoryKey,
    policy: &StorePolicy,
    name: &SnapshotName,
    tree: &Path,
    parent: Option<(SnapshotName, ChangeDetection)>,
    clock: &dyn Clock,
) -> anyhow::Result<Option<(StagedSnapshot, SnapshotInfo)>> {
    let attempt = |missed: HashSet<Box<Path>>| {
        let _ = stage.take();
        match stage_save(
            backend.clone(),
            stage,
            key,
            policy,
            name,
            tree,
            parent.clone(),
            clock,
        ) {
            Err(error) if is_index_missing(error.as_ref()) => {
                match missing_file_path(error.as_ref()).map(Box::<Path>::from) {
                    Some(path) if !missed.contains(&path) => ControlFlow::Continue(
                        missed.into_iter().chain(std::iter::once(path)).collect(),
                    ),
                    _ => ControlFlow::Break(Err(error)),
                }
            }
            result => ControlFlow::Break(result),
        }
    };
    match std::iter::repeat(()).try_fold(HashSet::new(), |missed, ()| attempt(missed)) {
        ControlFlow::Break(result) => result,
        ControlFlow::Continue(_) => Err(anyhow::anyhow!("the backup ended without an answer")),
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
        if self.root.is_cancelled() {
            return Err(SaveError::Stopped(Withdrawal::Stopped));
        }
        self.check_path(NativeOperation::Metadata, tree, tree_is_valid)
            .await
            .map_err(SaveError::Source)?;
        let parent = parent.map(|(parent, detection)| (parent.clone(), detection));
        self.shell(Kind::Save, slots, Some(cancel))
            .call(
                save_answers(),
                |t0| self.policy.backup_end(t0),
                |start| self.save_run(scope, name, tree, parent.clone(), cancel, start.t0),
                |own| self.own_name_check(scope, name, own),
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
        if self.root.is_cancelled() {
            return Err(RestoreFailure::Stopped(Withdrawal::Stopped));
        }
        self.check_path(
            NativeOperation::DirectoryEnumeration,
            into,
            destination_is_valid,
        )
        .await
        .map_err(RestoreFailure::Destination)?;
        self.shell(Kind::Restore, slots, None)
            .call(
                restore_answers(),
                |_| None,
                |_| self.restore_run(scope, name, into),
                no_check,
            )
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
        self.shell(Kind::Stat, &Unlimited, None)
            .call(
                read_answers(),
                |_| None,
                |_| self.stat_run(scope, name),
                no_check,
            )
            .await
    }

    async fn list(
        &self,
        scope: &AgentSnapshots,
        slots: &dyn RunSlots,
    ) -> Result<Box<[(SnapshotName, SnapshotInfo)]>, CallError> {
        self.shell(Kind::List, slots, None)
            .call(call_answers(), |_| None, |_| self.list_run(scope), no_check)
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
        self.shell(Kind::Delete, slots, None)
            .call(
                call_answers(),
                |_| None,
                |_| self.delete_run(scope, names),
                no_check,
            )
            .await
    }

    async fn delete_all(
        &self,
        scope: &AgentSnapshots,
        slots: &dyn RunSlots,
    ) -> Result<(), CallError> {
        self.shell(Kind::DeleteAll, slots, None)
            .call(
                call_answers(),
                |_| None,
                |_| self.delete_all_run(scope),
                no_check,
            )
            .await
    }

    async fn copy_all(
        &self,
        from: &AgentSnapshots,
        to: &AgentSnapshots,
        slots: &dyn RunSlots,
    ) -> Result<(), CallError> {
        self.shell(Kind::Copy, slots, None)
            .call(
                call_answers(),
                |_| None,
                |start| self.copy_run(from, to, start.number > 1),
                no_check,
            )
            .await
    }

    async fn shut_down(&self) {
        RusticSnapshotStore::shut_down(self).await
    }
}

/// The state of one run of a restore, which [`reload::next_step`] drives. It runs on a blocking
/// thread.
struct RestoreRun {
    backend: Arc<BlobBackend>,
    key: RepositoryKey,
    name: SnapshotName,
    into: Box<Path>,
    options: RestoreOptions,
    #[cfg(test)]
    hook: Arc<dyn ClearHook>,
}

/// What a run of a restore holds between its steps.
struct Restoring {
    /// The index files that a listing of the run gave.
    listed: HashSet<Box<Path>>,
    /// The index files that a load of the run found gone.
    missed: HashSet<Box<Path>>,
    /// The snapshot of the name and its info, after a load found it.
    found: Option<(SnapshotFile, SnapshotInfo)>,
    /// The repository with the index of the last load that found the name.
    indexed: Option<RusticRepository<IndexedFullStatus>>,
    /// The error that the last step gave.
    last_error: Option<anyhow::Error>,
    /// The error of the check, whose blob a read of the marked packs looks for.
    check_error: Option<anyhow::Error>,
    /// The pack that a step found gone.
    missing_pack: Option<Id>,
    /// Whether the run wrote into the directory.
    written: bool,
    /// The I/O error of a write into the directory.
    destination: Option<std::io::Error>,
    /// The info of a run that wrote the whole tree.
    restored: Option<SnapshotInfo>,
}

/// The result of one step of a run of a restore.
enum Stepped {
    Observed(Observed),
    Restored(SnapshotInfo),
}

impl RestoreRun {
    /// Runs the steps of the restore until one gives the end of the run.
    fn run(self) -> Ran<Result<SnapshotInfo, RestoreFailure>> {
        // A directory that is not empty holds what an earlier run wrote, also when its clear failed
        // part way.
        if let Err(error) = self.clear_if_written() {
            return Ran::Ended(error);
        }
        let mut restoring = Restoring {
            listed: HashSet::new(),
            missed: HashSet::new(),
            found: None,
            indexed: None,
            last_error: None,
            check_error: None,
            missing_pack: None,
            written: false,
            destination: None,
            restored: None,
        };
        let first = self.load(&mut restoring);
        let ended = (0..MOST_RESTORE_STEPS).try_fold(first, |observed, _| {
            let observed = match observed {
                Stepped::Restored(info) => {
                    restoring.restored = Some(info);
                    return ControlFlow::Break(None);
                }
                Stepped::Observed(observed) => observed,
            };
            let step = reload::next_step(&observed, &restoring.listed, &restoring.missed);
            match (&observed, step) {
                (Observed::IndexFileMissing(path), Step::LoadAgain) => {
                    restoring.missed.insert(path.clone());
                }
                (Observed::IndexListedAgain(names), Step::LoadAgain) => {
                    restoring.listed.extend(names.iter().cloned());
                }
                _ => {}
            }
            match step {
                Step::Check => ControlFlow::Continue(self.check_and_write(&mut restoring)),
                Step::LoadAgain => ControlFlow::Continue(self.load(&mut restoring)),
                Step::ListIndexAgain => ControlFlow::Continue(self.list_index(&mut restoring)),
                Step::ReadSnapshotsAgain(reason) => {
                    ControlFlow::Continue(self.read_snapshots(&mut restoring, reason))
                }
                Step::ReadMarked => ControlFlow::Continue(self.read_marked(&mut restoring)),
                Step::LoadIndexAgain => ControlFlow::Continue(self.load_index(&mut restoring)),
                Step::ClearAndEnd(end) => ControlFlow::Break(Some(match self.clear() {
                    Ok(()) => Ran::Ended(Ended::new(end, last_error(&mut restoring))),
                    Err(error) => Ran::Ended(error),
                })),
                Step::RunEnded(end) => ControlFlow::Break(Some(Ran::Ended(Ended::new(
                    end,
                    last_error(&mut restoring),
                )))),
                Step::Answer(outcome) => {
                    ControlFlow::Break(Some(Ran::Answered(Err(answer(outcome, &mut restoring)))))
                }
            }
        });
        match ended {
            ControlFlow::Break(Some(ran)) => ran,
            ControlFlow::Break(None) => match restoring.restored {
                Some(info) => Ran::Answered(Ok(info)),
                None => Ran::Ended(Ended::new(RunEnd::CallFailed, last_error(&mut restoring))),
            },
            ControlFlow::Continue(_) => Ran::Ended(Ended::new(
                RunEnd::CallFailed,
                anyhow::anyhow!("the restore found new index files at each of its steps"),
            )),
        }
    }

    /// Clears the directory when it is not empty. A failed clear ends the run: on unix a new run
    /// clears again, and elsewhere no run can clear.
    fn clear_if_written(&self) -> Result<(), Ended> {
        match clear::is_empty(&self.into) {
            Ok(true) => Ok(()),
            Ok(false) => self.clear(),
            Err(error) => Err(Ended::new(
                RunEnd::CallFailed,
                anyhow::Error::new(error).context("read the directory of the restore"),
            )),
        }
    }

    /// Removes every entry of the directory, as [`clear::clear`] says.
    fn clear(&self) -> Result<(), Ended> {
        #[cfg(test)]
        let cleared = clear::clear_with(&self.into, &*self.hook);
        #[cfg(not(test))]
        let cleared = clear::clear(&self.into);
        cleared.map_err(|error| {
            let end = if error.kind() == std::io::ErrorKind::Unsupported {
                RunEnd::Permanent
            } else {
                RunEnd::CallFailed
            };
            Ended::new(
                end,
                anyhow::Error::new(error)
                    .context("remove what an earlier run of the restore wrote"),
            )
        })
    }

    /// Loads the repository, the snapshot of the name and the index.
    fn load(&self, restoring: &mut Restoring) -> Stepped {
        let loaded = (|| {
            let Some(repository) = open_existing(self.backend.clone(), &self.key)? else {
                return Ok(Stepped::Observed(Observed::ConfigMissing));
            };
            match lookup(scope_snapshots(&repository, &self.backend)?, &self.name) {
                Lookup::Missing => Ok(Stepped::Observed(Observed::Loaded(reload::Lookup::Missing))),
                Lookup::Corrupt(error) => {
                    restoring.last_error = Some(error);
                    Ok(Stepped::Observed(Observed::Loaded(reload::Lookup::Corrupt)))
                }
                Lookup::Found(snapshot, info) => {
                    let indexed = repository.to_indexed();
                    restoring.listed.extend(self.backend.take_index_listed());
                    restoring.indexed = Some(indexed?);
                    restoring.found = Some((*snapshot, info));
                    Ok(Stepped::Observed(Observed::Loaded(reload::Lookup::Found)))
                }
            }
        })();
        loaded.unwrap_or_else(|error: anyhow::Error| {
            restoring.listed.extend(self.backend.take_index_listed());
            Stepped::Observed(self.observed_error(restoring, error))
        })
    }

    /// Checks that the index of the last load holds the whole tree, without a write, and then
    /// writes the tree with the same index.
    fn check_and_write(&self, restoring: &mut Restoring) -> Stepped {
        let (Some((snapshot, info)), Some(repository)) =
            (restoring.found.clone(), restoring.indexed.take())
        else {
            // A check follows only a load that found the name. When the run holds no such load,
            // it ends without a claim about the name, and a new run loads again.
            restoring.last_error = Some(anyhow::anyhow!(
                "the restore came to its check without a load that found the name"
            ));
            return Stepped::Observed(Observed::CallFailed {
                written: restoring.written,
            });
        };
        let checked = (|| {
            let into = self.into.to_str().ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "the directory of a restore must have a UTF-8 path",
                )
            })?;
            let destination = LocalDestination::new(into, false, false)?;
            let node = repository.node_from_snapshot_and_path(&snapshot, "")?;
            anyhow::Ok((destination, node))
        })();
        let (destination, node) = match checked {
            Ok(checked) => checked,
            Err(error) => return Stepped::Observed(self.observed_check_error(restoring, error)),
        };
        let entries = match repository.ls(&node, &LsOptions::default()) {
            Ok(entries) => entries,
            Err(error) => {
                return Stepped::Observed(
                    self.observed_check_error(restoring, anyhow::Error::from(error)),
                );
            }
        };
        if let Err(error) =
            repository.prepare_restore(&self.options, entries.clone(), &destination, true)
        {
            return Stepped::Observed(
                self.observed_check_error(restoring, anyhow::Error::from(error)),
            );
        }
        restoring.written = true;
        let written = repository
            .prepare_restore(&self.options, entries.clone(), &destination, false)
            .and_then(|plan| repository.restore(plan, &self.options, entries, &destination));
        match written {
            Ok(()) => Stepped::Restored(info),
            Err(error) => {
                Stepped::Observed(self.observed_write_error(restoring, anyhow::Error::from(error)))
            }
        }
    }

    /// Lists the index files again.
    fn list_index(&self, restoring: &mut Restoring) -> Stepped {
        match self.backend.list_with_size(FileType::Index) {
            Ok(_) => {
                Stepped::Observed(Observed::IndexListedAgain(self.backend.take_index_listed()))
            }
            Err(error) => {
                Stepped::Observed(self.observed_error(restoring, anyhow::Error::from(error)))
            }
        }
    }

    /// Reads the snapshot files again, and looks for the name.
    fn read_snapshots(&self, restoring: &mut Restoring, reason: Reread) -> Stepped {
        let read = (|| {
            let Some(repository) = open_existing(self.backend.clone(), &self.key)? else {
                return Ok(Found::NoName);
            };
            Ok(
                match lookup(scope_snapshots(&repository, &self.backend)?, &self.name) {
                    Lookup::Found(..) => Found::Name,
                    Lookup::Missing => Found::NoName,
                    Lookup::Corrupt(error) => {
                        restoring.last_error = Some(error);
                        Found::Corrupt
                    }
                },
            )
        })();
        match read {
            Ok(found) => Stepped::Observed(Observed::SnapshotsReadAgain { reason, found }),
            Err(error) => Stepped::Observed(self.observed_error(restoring, error)),
        }
    }

    /// Tells whether the blob that the check missed is only in packs that a prune marked for
    /// deletion. It reads the listed index files that the run did not find gone. A listed index
    /// file that is gone means that a prune wrote the index again after the listing, so the run
    /// observes the missing file and loads again, as at a load. The error of the check names the
    /// blob by the start of its id, so a marked blob whose id has that start counts. When the
    /// error names no blob, any marked pack counts.
    fn read_marked(&self, restoring: &mut Restoring) -> Stepped {
        let blob = restoring
            .check_error
            .as_ref()
            .and_then(|error| missing_blob(error.as_ref()));
        // The files are read in the order of their paths, so a run reads them in the same order
        // each time.
        let mut listed = restoring
            .listed
            .difference(&restoring.missed)
            .cloned()
            .collect::<Vec<_>>();
        listed.sort();
        let read = (|| {
            let Some(repository) = open_existing(self.backend.clone(), &self.key)? else {
                return Ok(None);
            };
            listed
                .iter()
                .try_fold(false, |marked, path| {
                    if marked {
                        return Ok(true);
                    }
                    let Some(id) = path
                        .file_name()
                        .and_then(|name| name.to_str())
                        .and_then(|name| name.parse::<Id>().ok())
                    else {
                        return Ok(false);
                    };
                    let index = match repository.get_file::<IndexFile>(&IndexId::from(id)) {
                        Ok(index) => index,
                        Err(error) => return Err(anyhow::Error::from(error)),
                    };
                    Ok(index.packs_to_delete.iter().any(|pack| match &blob {
                        Some(start) => pack
                            .blobs
                            .iter()
                            .any(|indexed| indexed.id.to_hex().starts_with(&**start)),
                        None => !pack.blobs.is_empty(),
                    }))
                })
                .map(Some)
        })();
        match read {
            // A delete of all snapshots removed the repository after the load.
            Ok(None) => Stepped::Observed(Observed::ConfigMissing),
            Ok(Some(only_marked)) => Stepped::Observed(Observed::MarkedRead { only_marked }),
            Err(error) => Stepped::Observed(self.observed_error(restoring, error)),
        }
    }

    /// Loads the index again, and tells whether it lists the pack that was gone. An index file that
    /// is gone during the load counts as an index that does not list the pack.
    fn load_index(&self, restoring: &mut Restoring) -> Stepped {
        let Some(pack) = restoring.missing_pack else {
            return Stepped::Observed(Observed::PackIndexed(false));
        };
        let loaded = (|| {
            let Some(repository) = open_existing(self.backend.clone(), &self.key)? else {
                return Ok(None);
            };
            let ids = repository.list::<IndexId>()?.collect::<Vec<_>>();
            ids.iter()
                .try_fold(false, |indexed, id| {
                    if indexed {
                        return Ok(true);
                    }
                    match repository.get_file::<IndexFile>(id) {
                        Ok(index) => Ok(index.packs.iter().any(|listed| *listed.id == pack)),
                        Err(error) if is_file_missing(&*error) => Ok(false),
                        Err(error) => Err(anyhow::Error::from(error)),
                    }
                })
                .map(Some)
        })();
        let _ = self.backend.take_index_listed();
        match loaded {
            // A delete of all snapshots removed the repository after the load.
            Ok(None) => Stepped::Observed(Observed::ConfigMissing),
            Ok(Some(indexed)) => Stepped::Observed(Observed::PackIndexed(indexed)),
            Err(error) if is_index_missing(error.as_ref()) => {
                Stepped::Observed(Observed::PackIndexed(false))
            }
            Err(error) => Stepped::Observed(self.observed_error(restoring, error)),
        }
    }

    /// Gives what a failed load or read observed.
    fn observed_error(&self, restoring: &mut Restoring, error: anyhow::Error) -> Observed {
        observe(restoring, error, Phase::Load)
    }

    /// Gives what a failed check observed.
    fn observed_check_error(&self, restoring: &mut Restoring, error: anyhow::Error) -> Observed {
        observe(restoring, error, Phase::Check)
    }

    /// Gives what a failed write observed.
    fn observed_write_error(&self, restoring: &mut Restoring, error: anyhow::Error) -> Observed {
        observe(restoring, error, Phase::Write)
    }
}

/// Gives what a run of a restore observed after the error of a failed step in `phase`, and keeps
/// what the next steps need: the missing pack, the error of the check, the I/O error of the
/// directory, and the error that the run ends with.
fn observe(restoring: &mut Restoring, error: anyhow::Error, phase: Phase) -> Observed {
    let written = restoring.written;
    let observed = match restore_error(&error, phase) {
        RestoreError::PackMissing(pack) => {
            restoring.missing_pack = pack;
            Observed::PackMissing { written }
        }
        RestoreError::FileMissing(path) => Observed::IndexFileMissing(path),
        RestoreError::ConfigMissing => Observed::ConfigMissing,
        RestoreError::Cancelled => Observed::Cancelled,
        RestoreError::Permanent { range } => Observed::Permanent { range },
        RestoreError::CallFailed => Observed::CallFailed { written },
        RestoreError::Destination(kind) => {
            restoring.destination = Some(std::io::Error::new(kind, format!("{error:#}")));
            Observed::Destination
        }
        RestoreError::CheckFailed => {
            restoring.check_error = Some(anyhow::anyhow!("{error:#}"));
            Observed::CheckFailed
        }
        RestoreError::Corrupt => Observed::Permanent { range: true },
    };
    restoring.last_error = Some(error);
    observed
}

/// Gives the error that a run of a restore ends with.
fn last_error(restoring: &mut Restoring) -> anyhow::Error {
    restoring
        .last_error
        .take()
        .unwrap_or_else(|| anyhow::anyhow!("the run of the restore ended"))
}

/// Gives the answer of a restore for the outcome of a step.
fn answer(outcome: Outcome, restoring: &mut Restoring) -> RestoreFailure {
    match outcome {
        Outcome::NotFound => RestoreFailure::NotFound,
        Outcome::Corrupt => RestoreFailure::Corrupt(last_error(restoring)),
        Outcome::MarkedOnly => RestoreFailure::Failed(Failed::new(anyhow::anyhow!(
            "a pack of the snapshot is marked for deletion; a later restore can succeed"
        ))),
        Outcome::Destination => RestoreFailure::Destination(
            restoring
                .destination
                .take()
                .unwrap_or_else(|| std::io::Error::other(last_error(restoring))),
        ),
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
        operation: NativeOperation,
        path: &Path,
        check: impl FnOnce(&Path) -> std::io::Result<()> + Send + 'static,
    ) -> std::io::Result<()> {
        let path: Box<Path> = path.into();
        let tracked = self.tracker.token();
        execute_native(NativeStorageProfile::Unknown, operation, move || {
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

/// Backs up the tree with the snapshot file in the stage, and gives the staged file with the info
/// of the snapshot. The result is `None` when a snapshot of the scope already has the name. The
/// parent is the snapshot that [`lookup`] finds for its name, and a name without a snapshot gives
/// no parent. The backup dedups only against the packs that an index file lists.
fn stage_save(
    backend: Arc<BlobBackend>,
    stage: &SnapshotStage,
    key: &RepositoryKey,
    policy: &StorePolicy,
    name: &SnapshotName,
    tree: &Path,
    parent: Option<(SnapshotName, ChangeDetection)>,
    clock: &dyn Clock,
) -> anyhow::Result<Option<(StagedSnapshot, SnapshotInfo)>> {
    // The init of the fork checks the config a second time before its write, so a save that loses
    // the race to create the repository can fail at that check with an error that is not
    // `ConfigExists`. A config that is there after the error is the repository of the winner.
    let repository = match open_or_create(backend.clone(), key) {
        Ok(repository) => repository,
        Err(error) => open_existing(backend.clone(), key)
            .ok()
            .flatten()
            .ok_or(error)?,
    };
    let (before, listed) = scope_snapshots_except(&repository, &backend, &HashSet::new())?;
    if has_name(&before, name) {
        return Ok(None);
    }
    let parent = parent.and_then(|(parent, detection)| {
        named(&before.readable, &parent).map(|snapshot| (snapshot.id, detection))
    });
    let newest = before
        .readable
        .iter()
        .filter_map(snapshot_info)
        .map(|info| info.created_at)
        .max();
    let created_at = snapshot_time(whole_millis_from(clock.now()), newest);
    // rustic strips the root from the path of each entry. The canonical root is the path that the
    // walk of rustic gives, also when the caller gives a path through a symlink such as
    // `/proc/self/fd/N`.
    let tree = std::fs::canonicalize(tree)?;
    let content = tree_content(&tree)?;
    let snapshot = SnapshotOptions::default()
        .label(name.as_str().to_string())
        .time(snapshot_zoned(created_at)?)
        .description(serde_json::to_string(&content)?)
        .to_snapshot()?;
    let repository = repository.to_indexed_ids()?;
    repository.backup(
        &store_backup_options(policy, parent),
        &PathList::from_iter(Some(tree)),
        snapshot,
    )?;
    let staged = stage
        .take()
        .context("the backup gave no snapshot file to the stage")?;
    // Only a file that is new since the first read can have the name now: a late publish of an
    // earlier attempt of the same job. So the save reads only those files again.
    let (since, _) = scope_snapshots_except(&repository, &backend, &listed)?;
    if has_name(&since, name) {
        return Ok(None);
    }
    Ok(Some((
        staged,
        SnapshotInfo {
            created_at,
            files: content.files,
            bytes: content.bytes,
        },
    )))
}

/// Reads each snapshot file of the repository, whose blobs `backend` reads. A failed storage call
/// fails the read, a file that a delete removed after the listing is left out, and each other
/// failure counts as a failed check.
fn scope_snapshots<S: Open>(
    repository: &RusticRepository<S>,
    backend: &BlobBackend,
) -> anyhow::Result<ScopeSnapshots> {
    scope_snapshots_except(repository, backend, &HashSet::new()).map(|(found, _)| found)
}

/// Reads each snapshot file of the repository whose id is not in `known`, as
/// [`scope_snapshots`] does, and gives also the ids of every snapshot file that the listing found.
/// The files are read ahead concurrently through `backend` before rustic reads and decrypts them
/// one after the other, so the reads of the storage overlap. The order of the files does not
/// matter, because each reader sorts or filters them.
fn scope_snapshots_except<S: Open>(
    repository: &RusticRepository<S>,
    backend: &BlobBackend,
    known: &HashSet<SnapshotId>,
) -> anyhow::Result<(ScopeSnapshots, HashSet<SnapshotId>)> {
    let listed = repository.list::<SnapshotId>()?.collect::<HashSet<_>>();
    let new = listed
        .iter()
        .filter(|id| !known.contains(id))
        .copied()
        .collect::<Vec<_>>();
    backend.read_ahead(FileType::Snapshot, new.iter().map(|id| **id));
    new.into_iter()
        .try_fold(
            ScopeSnapshots {
                readable: Vec::new(),
                unreadable: false,
            },
            |found, id| read_snapshot_file(repository, found, id),
        )
        .map(|found| (found, listed))
}

/// Adds the snapshot file `id` of the repository to `found`, as [`scope_snapshots`] says.
fn read_snapshot_file<S: Open>(
    repository: &RusticRepository<S>,
    mut found: ScopeSnapshots,
    id: SnapshotId,
) -> anyhow::Result<ScopeSnapshots> {
    match repository.get_file::<SnapshotFile>(&id) {
        Ok(mut snapshot) => {
            snapshot.id = id;
            found.readable.push(snapshot);
            Ok(found)
        }
        Err(error) if is_storage_failure(&*error) => Err(anyhow::Error::from(error)),
        Err(error) if is_file_missing(&*error) => Ok(found),
        Err(_) => Ok(ScopeSnapshots {
            unreadable: true,
            ..found
        }),
    }
}

fn has_name(found: &ScopeSnapshots, name: &SnapshotName) -> bool {
    found
        .readable
        .iter()
        .any(|snapshot| snapshot.label == name.as_str())
}

/// Finds the snapshot with the name. Of the snapshot files with the name, the one with the least
/// time and id wins. When no file has the name and a file failed its integrity check, the result is
/// `Corrupt`, because that file can have the name.
fn lookup(found: ScopeSnapshots, name: &SnapshotName) -> Lookup {
    match (named(&found.readable, name), found.unreadable) {
        (Some(snapshot), _) => match snapshot_info(snapshot) {
            Some(info) => Lookup::Found(Box::new(snapshot.clone()), info),
            None => Lookup::Corrupt(anyhow::anyhow!(
                "the snapshot {} does not describe its tree",
                snapshot.id
            )),
        },
        (None, true) => Lookup::Corrupt(anyhow::anyhow!(
            "a snapshot file of the scope failed its integrity check"
        )),
        (None, false) => Lookup::Missing,
    }
}

/// Of the snapshot files with the name, gives the one with the least time and id.
pub(super) fn named<'a>(
    snapshots: &'a [SnapshotFile],
    name: &SnapshotName,
) -> Option<&'a SnapshotFile> {
    snapshots
        .iter()
        .filter(|snapshot| snapshot.label == name.as_str())
        .min_by_key(|snapshot| (snapshot.time.timestamp(), snapshot.id))
}

/// Gives the name and the info of each snapshot whose label is a name and whose description
/// parses. Of the files with one name, only the one that [`lookup`] takes stays.
fn listed(found: ScopeSnapshots) -> Vec<(SnapshotName, SnapshotInfo)> {
    let mut snapshots = found.readable;
    snapshots.sort_by(|left, right| {
        (&left.label, left.time.timestamp(), left.id).cmp(&(
            &right.label,
            right.time.timestamp(),
            right.id,
        ))
    });
    snapshots.dedup_by(|later, first| later.label == first.label);
    snapshots
        .iter()
        .filter_map(|snapshot| {
            SnapshotName::new(&snapshot.label)
                .ok()
                .zip(snapshot_info(snapshot))
        })
        .collect()
}

/// Gives the info of a snapshot from its time and its description.
fn snapshot_info(snapshot: &SnapshotFile) -> Option<SnapshotInfo> {
    let content = serde_json::from_str::<TreeContent>(snapshot.description.as_deref()?).ok()?;
    let created_at = u64::try_from(snapshot.time.timestamp().as_millisecond()).ok()?;
    Some(SnapshotInfo {
        created_at: Timestamp::from(created_at),
        files: content.files,
        bytes: content.bytes,
    })
}

/// Tells whether a later prune removes packs that this prune leaves marked: unused packs, repacked
/// packs, packs that no index lists, and packs of an earlier prune whose grace period is not over.
/// A marked pack is in an index after the prune, so a later prune does not count it as unindexed.
fn leaves_marked_packs(report: &PruneReport) -> bool {
    report.packs_unused > 0
        || report.packs_repacked > 0
        || report.packs_unindexed > 0
        || report.marked_packs_kept > 0
}

/// Gives the packed bytes that the save of the snapshot added to the repository.
fn added_packed_bytes(snapshot: &SnapshotFile) -> u64 {
    snapshot
        .summary
        .as_ref()
        .map_or(0, |summary| summary.data_added_packed)
}

/// Gives the first time in whole milliseconds that is not before the time. A snapshot keeps its
/// time in milliseconds, so the time of a save is not before the call.
fn whole_millis_from(time: Timestamp) -> Timestamp {
    let truncated = Timestamp::from(time.to_millis());
    if truncated < time {
        Timestamp::from(time.to_millis().saturating_add(1))
    } else {
        truncated
    }
}

/// Gives the time as the time of a snapshot, in UTC.
fn snapshot_zoned(time: Timestamp) -> anyhow::Result<Zoned> {
    Ok(SnapshotTime::from_millisecond(i64::try_from(time.to_millis())?)?.to_zoned(TimeZone::UTC))
}

/// Counts the names of the regular files below the root, and the sum of their sizes. The walk
/// reads the metadata of each entry and does not follow a symlink.
fn tree_content(root: &Path) -> std::io::Result<TreeContent> {
    fn walk(directory: &Path, content: TreeContent) -> std::io::Result<TreeContent> {
        std::fs::read_dir(directory)?.try_fold(content, |content, entry| {
            let entry = entry?;
            let kind = entry.file_type()?;
            if kind.is_dir() {
                walk(&entry.path(), content)
            } else if kind.is_file() {
                Ok(TreeContent {
                    files: content.files + 1,
                    bytes: content.bytes + entry.metadata()?.len(),
                })
            } else {
                Ok(content)
            }
        })
    }
    walk(root, TreeContent { files: 0, bytes: 0 })
}

#[cfg(test)]
mod tests;
