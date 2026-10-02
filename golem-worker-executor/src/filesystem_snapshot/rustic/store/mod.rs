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
//! A save makes the snapshot visible in one step: the backend keeps the snapshot file of the
//! backup, and the save writes that file after the blocking work returns. Each operation has a
//! cancellation token that its drop cancels, so the threads of a dropped operation stop at their
//! next storage call. The store counts each blocking task and each backend in a task tracker, and
//! [`RusticSnapshotStore::shut_down`] waits for them.

use super::backend::{BlobBackend, KEPT_PACKS_LIMIT};
use super::claim::Claim;
use super::fault::{
    Operation, classify, is_file_missing, is_snapshot_missing, is_storage_failure, storage_failure,
};
use super::files::SnapshotFiles;
use super::priority::LowPriority;
use super::prune::{
    Percent, PrunePolicy, belongs_to_ledger, due_prune, read_ledger, record_freed, remove_freed,
    remove_old_claims, remove_older_ledgers, write_ledger,
};
use super::publish::{SnapshotStage, StagedSnapshot, publish};
use super::scope::{copy_scope, delete_scope};
use super::spawner::Spawner;
use super::{
    PruneReport, PruneSettings, RepositoryKey, backup_options, open_existing, open_or_create,
    prune, restore_snapshot, run_blocking,
};
use crate::filesystem_snapshot::clock::{Clock, SystemClock};
use crate::filesystem_snapshot::{
    AgentSnapshots, ChangeDetection, FilesystemSnapshotStore, SnapshotInfo, SnapshotName,
    SnapshotStoreError, newest_first, snapshot_time,
};
use crate::sandbox_filesystem::{NativeOperation, NativeStorageProfile, execute_native};
use crate::services::golem_config::FilesystemSnapshotStoreConfig;
use anyhow::Context;
use async_trait::async_trait;
use futures::future::{self, Either};
use golem_common::model::Timestamp;
use golem_service_base::storage::blob::BlobStorage;
use rustic_core::jiff::tz::TimeZone;
use rustic_core::jiff::{Timestamp as SnapshotTime, Zoned};
use rustic_core::repofile::{SnapshotFile, SnapshotId};
use rustic_core::{
    BackupOptions, DevIdOption, FileType, LocalSourceSaveOptions, Open, PathList,
    Repository as RusticRepository, RestoreOptions, SnapshotOptions,
};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::num::NonZeroUsize;
use std::path::Path;
use std::pin::pin;
use std::sync::Arc;
use std::time::Duration;
use tokio::runtime::Handle;
use tokio_util::sync::{CancellationToken, DropGuard};
use tokio_util::task::TaskTracker;

/// The share of the size of the repository that deleted snapshots must free before a delete prunes
/// the scope. The threshold is 10% of the size, rounded down to a whole byte, so about 10%.
const PRUNE_THRESHOLD: Percent = Percent(10);

/// How long a pack that a prune marks stays before a later prune deletes it. It must be longer than
/// the longest save and the longest restore. Two prunes of one scope never run at once. The next
/// prune waits a full hold from the newest claim marker that was written. The hold follows from
/// this time: this time less one storage call deadline, but at least one deadline, plus the margin
/// for clock skew, plus two deadlines. When writes fail, only the gap between two prunes can be
/// shorter.
const PRUNE_GRACE: Duration = Duration::from_secs(15 * 60);

/// The settings of the store: the rustic settings of each operation, and the prune threshold.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct StorePolicy {
    /// The longest time that one blob storage call waits for an answer.
    pub(super) deadline: Duration,
    /// The number of threads of each parallel stage of a save. `None` is the number of CPUs that
    /// the process can use.
    pub(super) save_threads: Option<NonZeroUsize>,
    /// The number of threads that read packs in a restore.
    pub(super) restore_reader_threads: NonZeroUsize,
    /// The settings of a prune. Two prunes never run at once. The next prune waits a full hold from
    /// the newest claim marker that was written. The hold follows from `keep_delete`: that time
    /// less one storage call deadline, but at least one deadline, plus the margin for clock skew,
    /// plus two deadlines. When writes fail, only the gap between two prunes can be shorter.
    pub(super) prune: PruneSettings,
    /// The share of the size of the repository that deleted snapshots must free before a delete
    /// prunes.
    pub(super) prune_threshold: Percent,
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
        }
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
    /// The parent of the token of each operation.
    root: CancellationToken,
    /// Counts the blocking tasks, the checks of local paths, the backends, the blob calls of the
    /// store, the publishes, the deletes of dropped publishes, and the claims with their
    /// release and final-marker tasks.
    tracker: TaskTracker,
    /// Runs saves and prunes at a low priority.
    low_priority: LowPriority,
    /// Gives the wall time that the store compares with the times from storage, and the times that
    /// it writes.
    clock: Arc<dyn Clock>,
}

/// The number of times that a prune plans again when a snapshot file that it listed is gone at
/// its read.
const PRUNE_ATTEMPTS: usize = 3;

/// The error of a delete whose prune found a snapshot file gone at each attempt. A concurrent
/// delete removed the files, so a retry of the delete can prune.
fn snapshots_changed_error() -> SnapshotStoreError {
    SnapshotStoreError::Storage {
        retryable: true,
        source: anyhow::anyhow!(
            "a concurrent delete removed a snapshot file at each attempt of the prune"
        ),
    }
}

/// Gives the runtime of the operation that calls it.
fn runtime() -> Result<Handle, SnapshotStoreError> {
    Handle::try_current()
        .context("a filesystem snapshot operation needs an async runtime")
        .map_err(|source| SnapshotStoreError::Storage {
            retryable: false,
            source,
        })
}

/// The error of a save whose cancel fired before its publish.
fn cancelled_save_error() -> SnapshotStoreError {
    SnapshotStoreError::Storage {
        retryable: false,
        source: anyhow::anyhow!(
            "the save of the filesystem snapshot was cancelled before its publish"
        ),
    }
}

/// The error of an operation of a store that is shut down.
fn shut_down_error() -> SnapshotStoreError {
    SnapshotStoreError::Storage {
        retryable: false,
        source: anyhow::anyhow!("the filesystem snapshot store is shut down"),
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
    pub(super) fn with_policy(
        storage: Arc<dyn BlobStorage>,
        key: RepositoryKey,
        policy: StorePolicy,
        clock: Arc<dyn Clock>,
    ) -> Self {
        // The global rayon pool starts at its first use, and its threads keep the priority of the
        // thread that starts it. It starts here, at the normal priority, before a save or a prune.
        let _ = rayon::current_num_threads();
        Self {
            storage,
            key,
            policy,
            root: CancellationToken::new(),
            tracker: TaskTracker::new(),
            low_priority: LowPriority::new(policy.save_threads),
            clock,
        }
    }

    /// Cancels each operation, so each running storage call ends and no new call starts, and later
    /// operations give `Storage`. Some calls run after the cancel by design, because no cancel
    /// ends them: a publish that started before the cancel runs to its end, and a claim is
    /// released, or gets the final marker of its prune when the prune started. A save that
    /// reaches its publish after the cancel publishes nothing and gives `Storage`. The call waits
    /// until no blocking task, check of a local path, backend, blob call of the store, publish,
    /// delete of a dropped publish, claim, or release or final marker of a claim
    /// remains. A blob call that is not polled holds the wait until it is polled again, and then
    /// it ends at once. The runtime must not drop before it returns, because a storage call after
    /// its time driver stops aborts the process.
    pub(crate) async fn shut_down(&self) {
        self.root.cancel();
        self.tracker.close();
        self.tracker.wait().await;
    }

    /// Gives the number of blocking tasks, checks of local paths, backends, blob calls of the store,
    /// publishes, deletes of dropped publishes, and claims with their release and
    /// final-marker tasks that have not ended.
    #[cfg(test)]
    pub(super) fn work_in_flight(&self) -> usize {
        self.tracker.len()
    }

    /// Starts an operation. The token of the operation is cancelled when the guard drops.
    fn start(&self) -> Result<(CancellationToken, DropGuard), SnapshotStoreError> {
        if self.root.is_cancelled() {
            return Err(shut_down_error());
        }
        let token = self.root.child_token();
        Ok((token.clone(), token.drop_guard()))
    }

    /// Gives a backend over the repository of the blobs, whose calls follow the policy of the
    /// blobs.
    fn backend(&self, files: SnapshotFiles) -> Result<BlobBackend, SnapshotStoreError> {
        Ok(BlobBackend::new(files, runtime()?, KEPT_PACKS_LIMIT).tracked_by(self.tracker.token()))
    }

    /// Gives the spawner of the work that the store runs as a task, so that the work also ends when
    /// the caller of an operation stops waiting: the runtime of the operation, and the tracker of
    /// the store.
    fn spawner(&self) -> Result<Spawner, SnapshotStoreError> {
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

    /// Gives a backend over the repository of the scope for the operation with the token.
    fn scope_backend(
        &self,
        scope: &AgentSnapshots,
        token: &CancellationToken,
    ) -> Result<BlobBackend, SnapshotStoreError> {
        self.backend(self.files(scope, token))
    }

    /// Runs the task on a blocking thread that the tracker counts, and classifies its error.
    async fn blocking<T: Send + 'static>(
        &self,
        operation: Operation,
        task: impl FnOnce() -> anyhow::Result<T> + Send + 'static,
    ) -> Result<T, SnapshotStoreError> {
        let tracked = self.tracker.token();
        run_blocking(move || {
            let _tracked = tracked;
            task()
        })
        .await
        .map_err(|error| classify(operation, error))
    }

    /// Gives the blobs of the scope for the operation with the token.
    fn files(&self, scope: &AgentSnapshots, token: &CancellationToken) -> SnapshotFiles {
        SnapshotFiles::new(
            self.storage.clone(),
            (*scope.0).clone(),
            self.policy.deadline,
            token.clone(),
            self.tracker.clone(),
        )
    }

    /// Prunes the repository when a prune is due. The records of freed bytes stay until a prune
    /// succeeds, so a later prune counts them again. It lists the packs only when their size can
    /// make a prune due.
    /// A due prune runs only after the delete takes a claim of its ledger, and only when a second
    /// read of the ledger after the claim finds the time of the last prune of the first read. When
    /// it finds another time, the claim is released and the delete gives no error. So a delete
    /// prunes only with a claim in the claim directory of the ledger that the second read gives.
    /// After an error before the prune ran, the claim is released, so a retry of the delete prunes
    /// again, and a claim write that the storage completes after that release can delay that prune
    /// by up to the hold of a claim. A prune that found a snapshot file gone at each attempt
    /// changed nothing, so it counts as an error before the prune ran.
    /// After a prune that started, the claim stays on each outcome, also when the prune or its
    /// ledger write fails, or when the lease skips its ledger write. So the next prune waits a full
    /// hold from the newest claim marker that was written, which is the end of the prune unless the
    /// final marker write failed. Two prunes never run at once; when writes fail, only the gap
    /// between them can be shorter. A prune that succeeds deletes each claim of its ledger.
    async fn prune_when_due(
        &self,
        scope: &AgentSnapshots,
        token: &CancellationToken,
    ) -> Result<(), SnapshotStoreError> {
        let files = self.files(scope, token);
        let policy = self.prune_policy();
        let Some(due) = due_prune(&files, &*self.clock, &policy)
            .await
            .map_err(storage_failure)?
        else {
            return Ok(());
        };
        // The token of the tracker comes before the check of the cancel, so either the delete sees
        // a shut down and makes no more storage calls, or `shut_down` waits for the claim and for
        // each call that the claim makes.
        let tracked = self.tracker.token();
        if self.root.is_cancelled() {
            return Err(shut_down_error());
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
        .await
        .map_err(storage_failure)?
        else {
            return Ok(());
        };
        // Before the prune starts, an error releases the claim, so a retry of the delete prunes
        // again. When the second read of the ledger finds another time of the last prune than the
        // first read, the claim is released too, and the delete gives no error. A prune that
        // started can have marked packs, so its claim stays.
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
                return Err(snapshots_changed_error());
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
        write_ledger(&claim.leased(&files), ended, marked_packs)
            .await
            .map_err(storage_failure)?;
        remove_older_ledgers(&files, ended).await;
        remove_freed(&files, &due.records).await;
        remove_old_claims(&files, ended).await;
        Ok(())
    }

    /// Publishes the staged snapshot file of a save, unless `cancel`, the cancel of the save, fired
    /// first. The publish is the commit point, so no cancel ends it once it started. The tracker counts it from the call, so a `shut_down` that has not cancelled yet
    /// waits for it, and the deadline limits that wait. The check of the cancel and the start of the
    /// write are in the first poll of the returned future. So a publish never starts after the
    /// cancel.
    fn publish_staged(
        &self,
        scope: &AgentSnapshots,
        staged: StagedSnapshot,
        cancel: CancellationToken,
    ) -> impl Future<Output = Result<(), SnapshotStoreError>> + Send + 'static {
        let files = self.files(scope, &self.root).detached();
        let (root, tracker) = (self.root.clone(), self.tracker.clone());
        self.tracker.track_future(async move {
            if root.is_cancelled() {
                // No snapshot file is written. A later prune marks the packs of the save.
                return Err(shut_down_error());
            }
            if cancel.is_cancelled() {
                return Err(cancelled_save_error());
            }
            let spawner = Spawner {
                tracker,
                runtime: runtime()?,
            };
            publish(&files, &staged, &spawner)
                .await
                .map_err(storage_failure)
        })
    }

    /// Checks the ledger again and builds the backend of the prune. It gives `None` when the second
    /// read finds another time of the last prune than the first read. So a prune runs only with a
    /// claim in the claim directory of the ledger that the second read gives.
    async fn prepare_prune(
        &self,
        files: &SnapshotFiles,
        claim: &Claim,
    ) -> Result<Option<Arc<BlobBackend>>, SnapshotStoreError> {
        // A prune writes its ledger before it deletes the claims, so a delete that claims in a
        // directory that such a prune removed sees the new ledger here.
        let again = read_ledger(files, &*self.clock)
            .await
            .map_err(storage_failure)?;
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
    ) -> Result<Option<bool>, SnapshotStoreError> {
        let key = self.key.clone();
        let settings = self.policy.prune;
        let low_priority = self.low_priority;
        let start = claim.start();
        // The plan of a prune reads each snapshot file before the prune changes the repository.
        // A forget of another delete can remove a listed file before its read, so the prune
        // plans again from a new listing.
        let pruning = self.blocking(Operation::Prune, move || {
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
        // The claim gets a new marker while the prune runs, so a prune slower than the grace
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
            .map(|report| {
                report
                    .map(|report| report.as_ref().is_some_and(leaves_marked_packs))
                    .map_err(|error| classify(Operation::Prune, error))
            })
            .transpose()
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
    ) -> Result<SnapshotInfo, SnapshotStoreError> {
        let (token, _guard) = self.start()?;
        // A cancel of the save cancels its operation, so each later storage call of the save
        // fails. The link ends with the operation, and the tracker counts it.
        {
            let (cancel, token) = (cancel.clone(), token.clone());
            self.tracker.spawn(async move {
                tokio::select! {
                    () = cancel.cancelled() => token.cancel(),
                    () = token.cancelled() => {}
                }
            });
        }
        self.check_tree(tree).await?;
        let stage = Arc::new(SnapshotStage::default());
        let backend = Arc::new(self.scope_backend(scope, &token)?.staging_in(stage.clone()));
        let key = self.key.clone();
        let policy = self.policy;
        let name = name.clone();
        let tree: Box<Path> = tree.into();
        let parent = parent.map(|(parent, detection)| (parent.clone(), detection));
        let low_priority = self.low_priority;
        let clock = self.clock.clone();
        let staged = self
            .blocking(Operation::Save, move || {
                low_priority.run("fs-snap-save", move || {
                    stage_save(
                        backend, &stage, &key, &policy, &name, &tree, parent, &*clock,
                    )
                })
            })
            .await;
        // The backup has returned, so the store reads the tree no more. A cancel before this
        // point publishes nothing, whatever the backup gave.
        if cancel.is_cancelled() {
            return Err(cancelled_save_error());
        }
        let (staged, info) = staged?.ok_or(SnapshotStoreError::AlreadyExists)?;
        self.publish_staged(scope, staged, cancel.clone()).await?;
        Ok(info)
    }

    async fn restore(
        &self,
        scope: &AgentSnapshots,
        name: &SnapshotName,
        into: &Path,
    ) -> Result<SnapshotInfo, SnapshotStoreError> {
        let (token, _guard) = self.start()?;
        self.check_destination(into).await?;
        let backend = Arc::new(self.scope_backend(scope, &token)?);
        let key = self.key.clone();
        let options = store_restore_options(&self.policy);
        let name = name.clone();
        let into: Box<Path> = into.into();
        // The index load of a restore uses rayon, so the restore runs in a pool of its own, with the
        // reader threads of a restore. The blocking thread starts the threads of that pool, so they
        // have its normal priority.
        let pool = LowPriority {
            threads: Some(self.policy.restore_reader_threads),
            ..self.low_priority
        };
        self.blocking(Operation::Restore, move || {
            pool.in_own_pool("fs-snap-restore", move || {
                let Some(repository) = open_existing(backend.clone(), &key)? else {
                    return Ok(Lookup::Missing);
                };
                match lookup(scope_snapshots(&repository, &backend)?, &name) {
                    Lookup::Found(snapshot, info) => {
                        restore_snapshot(repository, &snapshot, &into, &options)?;
                        Ok(Lookup::Found(snapshot, info))
                    }
                    other => Ok(other),
                }
            })
        })
        .await?
        .into_info()?
        .ok_or(SnapshotStoreError::NotFound)
    }

    async fn stat(
        &self,
        scope: &AgentSnapshots,
        name: &SnapshotName,
    ) -> Result<Option<SnapshotInfo>, SnapshotStoreError> {
        let (token, _guard) = self.start()?;
        let backend = Arc::new(self.scope_backend(scope, &token)?);
        let key = self.key.clone();
        let name = name.clone();
        self.blocking(Operation::Repository, move || {
            Ok(match open_existing(backend.clone(), &key)? {
                Some(repository) => lookup(scope_snapshots(&repository, &backend)?, &name),
                None => Lookup::Missing,
            })
        })
        .await?
        .into_info()
    }

    async fn list(
        &self,
        scope: &AgentSnapshots,
    ) -> Result<Box<[(SnapshotName, SnapshotInfo)]>, SnapshotStoreError> {
        let (token, _guard) = self.start()?;
        let backend = Arc::new(self.scope_backend(scope, &token)?);
        let key = self.key.clone();
        self.blocking(Operation::Repository, move || {
            Ok(match open_existing(backend.clone(), &key)? {
                Some(repository) => newest_first(listed(scope_snapshots(&repository, &backend)?)),
                None => Box::default(),
            })
        })
        .await
    }

    async fn delete(
        &self,
        scope: &AgentSnapshots,
        names: &[SnapshotName],
    ) -> Result<(), SnapshotStoreError> {
        let (token, _guard) = self.start()?;
        let backend = Arc::new(self.scope_backend(scope, &token)?);
        let key = self.key.clone();
        let names = names
            .iter()
            .map(|name| Box::<str>::from(name.as_str()))
            .collect::<std::collections::HashSet<_>>();
        let found = self
            .blocking(Operation::Repository, move || {
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
            .await?;
        let Some((repository, ids, freed)) = found else {
            return Ok(());
        };
        // The record comes before the forget, so a stop between the two cannot lose the bytes. A
        // forget that then fails gives its error, and the record stays, which only brings a prune
        // earlier.
        let files = self.files(scope, &token);
        if freed > 0 {
            let snapshots = ids
                .iter()
                .map(|id| id.to_hex().to_string().into_boxed_str())
                .collect::<Box<[_]>>();
            record_freed(&files, freed, &snapshots)
                .await
                .map_err(storage_failure)?;
        }
        // The forget deletes the snapshot files with rayon, so it runs in a pool of its own. The
        // blocking thread starts the threads of that pool, so they have its normal priority.
        let pool = self.low_priority;
        self.blocking(Operation::Repository, move || {
            pool.in_own_pool("fs-snap-delete", move || {
                repository.delete_snapshots(&ids)?;
                Ok(())
            })
        })
        .await?;
        self.prune_when_due(scope, &token).await
    }

    async fn delete_all(&self, scope: &AgentSnapshots) -> Result<(), SnapshotStoreError> {
        let (token, _guard) = self.start()?;
        delete_scope(&self.files(scope, &token))
            .await
            .map_err(storage_failure)
    }

    async fn copy_all(
        &self,
        from: &AgentSnapshots,
        to: &AgentSnapshots,
    ) -> Result<(), SnapshotStoreError> {
        let (token, _guard) = self.start()?;
        copy_scope(&self.files(from, &token), &self.files(to, &token))
            .await
            .map_err(storage_failure)
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

impl Lookup {
    fn into_info(self) -> Result<Option<SnapshotInfo>, SnapshotStoreError> {
        match self {
            Self::Found(_, info) => Ok(Some(info)),
            Self::Missing => Ok(None),
            Self::Corrupt(error) => Err(SnapshotStoreError::Corrupt(error)),
        }
    }
}

impl RusticSnapshotStore {
    /// Runs the check of a local path on a blocking thread. A thread that fails gives the error of
    /// the path. The tracker counts the check until it ends, so `shut_down` waits for it, also when
    /// the operation drops first.
    async fn check_path(
        &self,
        operation: NativeOperation,
        path: &Path,
        error: fn(std::io::Error) -> SnapshotStoreError,
        check: impl FnOnce(&Path) -> Result<(), SnapshotStoreError> + Send + 'static,
    ) -> Result<(), SnapshotStoreError> {
        let path: Box<Path> = path.into();
        let tracked = self.tracker.token();
        execute_native(NativeStorageProfile::Unknown, operation, move || {
            let _tracked = tracked;
            check(&path)
        })
        .await
        .map_err(|failed| error(std::io::Error::other(failed)))?
    }

    /// Checks that the tree of a save is a directory at an absolute path.
    async fn check_tree(&self, tree: &Path) -> Result<(), SnapshotStoreError> {
        self.check_path(
            NativeOperation::Metadata,
            tree,
            SnapshotStoreError::Source,
            tree_is_valid,
        )
        .await
    }

    /// Checks that the directory of a restore is an empty directory with a UTF-8 path.
    async fn check_destination(&self, into: &Path) -> Result<(), SnapshotStoreError> {
        self.check_path(
            NativeOperation::DirectoryEnumeration,
            into,
            SnapshotStoreError::Destination,
            destination_is_valid,
        )
        .await
    }
}

fn tree_is_valid(tree: &Path) -> Result<(), SnapshotStoreError> {
    let metadata = std::fs::metadata(tree).map_err(SnapshotStoreError::Source)?;
    if !tree.is_absolute() || !metadata.is_dir() {
        return Err(SnapshotStoreError::Source(std::io::Error::new(
            std::io::ErrorKind::NotADirectory,
            format!(
                "the tree {} is not a directory at an absolute path",
                tree.display()
            ),
        )));
    }
    Ok(())
}

fn destination_is_valid(into: &Path) -> Result<(), SnapshotStoreError> {
    let refused = |kind, reason: &str| {
        SnapshotStoreError::Destination(std::io::Error::new(
            kind,
            format!("the directory {} {reason}", into.display()),
        ))
    };
    let metadata = std::fs::metadata(into).map_err(SnapshotStoreError::Destination)?;
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
    let first = std::fs::read_dir(into)
        .map_err(SnapshotStoreError::Destination)?
        .next()
        .transpose()
        .map_err(SnapshotStoreError::Destination)?;
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
/// no parent.
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
