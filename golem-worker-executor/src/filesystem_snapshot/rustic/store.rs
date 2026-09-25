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

use super::backend::BlobBackend;
use super::fault::{Operation, classify, is_file_missing, is_storage_failure, storage_failure};
use super::files::SnapshotFiles;
use super::priority::LowPriority;
use super::prune::{PruneLedger, prune_due, read_ledger, write_ledger};
use super::publish::{SnapshotStage, StagedSnapshot, publish};
use super::scope::{copy_scope, delete_scope};
use super::{
    ChangeDetection as RusticChangeDetection, PruneReport, PruneSettings, RepackLimits,
    RepositoryKey, RepositorySettings, SaveSettings, backup_options, open_existing, open_or_create,
    prune, restore_snapshot, run_blocking,
};
use crate::filesystem_snapshot::{
    ChangeDetection, FilesystemSnapshotStore, SnapshotInfo, SnapshotName, SnapshotScope,
    SnapshotStoreError, newest_first, snapshot_time,
};
use crate::services::golem_config::FilesystemSnapshotStoreConfig;
use anyhow::Context;
use async_trait::async_trait;
use golem_common::model::Timestamp;
use golem_service_base::storage::blob::BlobStorage;
use rustic_core::jiff::tz::TimeZone;
use rustic_core::jiff::{Timestamp as SnapshotTime, Zoned};
use rustic_core::repofile::{SnapshotFile, SnapshotId};
use rustic_core::{
    BackupOptions, DevIdOption, LocalSourceSaveOptions, Open, PathList,
    Repository as RusticRepository, RestoreOptions, SnapshotOptions,
};
use serde::{Deserialize, Serialize};
use std::num::NonZeroUsize;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use tokio::runtime::Handle;
use tokio_util::sync::{CancellationToken, DropGuard};
use tokio_util::task::TaskTracker;

/// The packed bytes that deleted snapshots must free before a delete prunes the scope.
const PRUNE_THRESHOLD_BYTES: u64 = 64 * 1024 * 1024;

/// How long a pack that a prune marks stays before a later prune deletes it. It is also the
/// shortest time between two prunes of one scope. It must be longer than the longest save and the
/// longest restore.
const PRUNE_GRACE: Duration = Duration::from_secs(15 * 60);

/// The settings of the store: the rustic settings of each operation, and the prune threshold.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct StorePolicy {
    /// The longest time that one blob storage call waits for an answer.
    pub(super) deadline: Duration,
    /// The settings of a repository that a save makes.
    pub(super) repository: RepositorySettings,
    /// The number of threads of each parallel stage of a save. `None` is the number of CPUs that
    /// the process can use.
    pub(super) save_threads: Option<NonZeroUsize>,
    /// The number of threads that read packs in a restore.
    pub(super) restore_reader_threads: NonZeroUsize,
    /// The settings of a prune. `keep_delete` is also the shortest time between two prunes.
    pub(super) prune: PruneSettings,
    /// The packed bytes that deleted snapshots must free before a delete prunes.
    pub(super) prune_threshold: u64,
}

impl StorePolicy {
    /// Gives the policy with the values of the configuration.
    pub(super) fn from_config(config: &FilesystemSnapshotStoreConfig) -> Self {
        Self {
            deadline: config.storage_call_deadline(),
            repository: RepositorySettings::DEFAULT,
            save_threads: Some(config.save_threads()),
            restore_reader_threads: config.restore_reader_threads(),
            prune: PruneSettings {
                fast_repack: true,
                keep_delete: PRUNE_GRACE,
                repack: RepackLimits::Rustic,
            },
            prune_threshold: PRUNE_THRESHOLD_BYTES,
        }
    }
}

/// The options of a save of the store: a failed read of an entry fails the save, and no device id
/// is kept. `SizeMtime` compares with the parent that the id names, and each other case reads
/// every file.
fn store_backup_options(
    policy: &StorePolicy,
    parent: Option<(SnapshotId, ChangeDetection)>,
) -> BackupOptions {
    let base = |detection| {
        backup_options(&SaveSettings {
            threads: policy.save_threads,
            detection,
        })
        .fail_on_read_error(true)
        .ignore_save_opts(LocalSourceSaveOptions::default().set_devid(DevIdOption::No))
    };
    match parent {
        Some((id, ChangeDetection::SizeMtime)) => {
            let options = base(RusticChangeDetection::SizeMtime);
            let parent_opts = options
                .parent_opts
                .clone()
                .parents(vec![id.to_hex().to_string()]);
            options.parent_opts(parent_opts)
        }
        None | Some((_, ChangeDetection::Full)) => {
            let options = base(RusticChangeDetection::Ctime);
            let parent_opts = options.parent_opts.clone().force(true);
            options.parent_opts(parent_opts)
        }
    }
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
    /// Counts the blocking tasks, the backends and the deletes of dropped publishes.
    tracker: TaskTracker,
    /// Runs saves and prunes at a low priority.
    low_priority: LowPriority,
}

impl RusticSnapshotStore {
    /// Gives the store over the blob storage, with the key and the values of the configuration.
    pub(crate) fn new(
        storage: Arc<dyn BlobStorage>,
        config: &FilesystemSnapshotStoreConfig,
    ) -> Self {
        Self::with_policy(
            storage,
            RepositoryKey::new(*config.repository_key().bytes()),
            StorePolicy::from_config(config),
        )
    }

    pub(super) fn with_policy(
        storage: Arc<dyn BlobStorage>,
        key: RepositoryKey,
        policy: StorePolicy,
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
        }
    }

    /// Stops each operation at its next storage call, and waits until no blocking task and no
    /// backend of the store remains; later operations give `Storage`. The runtime must not drop
    /// before it returns, because a storage call after its time driver stops aborts the process.
    pub(crate) async fn shut_down(&self) {
        self.root.cancel();
        self.tracker.close();
        self.tracker.wait().await;
    }

    /// Gives the number of blocking tasks, backends and deletes of the store that have not ended.
    #[cfg(test)]
    pub(super) fn work_in_flight(&self) -> usize {
        self.tracker.len()
    }

    /// Starts an operation. The token of the operation is cancelled when the guard drops.
    fn start(&self) -> Result<(CancellationToken, DropGuard), SnapshotStoreError> {
        if self.root.is_cancelled() {
            return Err(SnapshotStoreError::Storage {
                retryable: false,
                source: anyhow::anyhow!("the filesystem snapshot store is shut down"),
            });
        }
        let token = self.root.child_token();
        Ok((token.clone(), token.drop_guard()))
    }

    /// Gives a backend over the repository of the scope for the operation with the token.
    fn backend(
        &self,
        scope: &SnapshotScope,
        token: &CancellationToken,
    ) -> Result<BlobBackend, SnapshotStoreError> {
        let runtime = Handle::try_current()
            .context("a filesystem snapshot operation needs an async runtime")
            .map_err(|source| SnapshotStoreError::Storage {
                retryable: false,
                source,
            })?;
        Ok(BlobBackend::new(
            self.storage.clone(),
            scope.0.clone(),
            runtime,
            self.policy.deadline,
        )
        .cancelled_by(token.clone())
        .tracked_by(self.tracker.token()))
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

    fn files(&self, scope: &SnapshotScope) -> SnapshotFiles {
        SnapshotFiles {
            storage: self.storage.clone(),
            namespace: scope.0.clone(),
            deadline: self.policy.deadline,
        }
    }

    /// Adds the freed bytes to the ledger of the scope, and prunes the repository when a prune is
    /// due. The ledger keeps the freed bytes before the prune starts, so a delete that runs again
    /// after a failed prune prunes again.
    async fn prune_when_due(
        &self,
        scope: &SnapshotScope,
        token: &CancellationToken,
        freed: u64,
    ) -> Result<(), SnapshotStoreError> {
        let files = self.files(scope);
        let ledger = read_ledger(&files)
            .await
            .map_err(storage_failure)?
            .with_deleted(freed);
        if freed > 0 {
            write_ledger(&files, &ledger)
                .await
                .map_err(storage_failure)?;
        }
        let now = Timestamp::now_utc();
        if !prune_due(
            &ledger,
            now,
            self.policy.prune_threshold,
            self.policy.prune.keep_delete,
        ) {
            return Ok(());
        }
        let backend = Arc::new(self.backend(scope, token)?);
        let key = self.key.clone();
        let settings = self.policy.prune;
        let low_priority = self.low_priority;
        let report = self
            .blocking(Operation::Prune, move || {
                low_priority.run("fs-snap-prune", move || prune(backend, &key, &settings))
            })
            .await?;
        let marked_packs = report.as_ref().is_some_and(leaves_marked_packs);
        write_ledger(&files, &PruneLedger::after_prune(now, marked_packs))
            .await
            .map_err(storage_failure)
    }
}

#[async_trait]
impl FilesystemSnapshotStore for RusticSnapshotStore {
    async fn save(
        &self,
        scope: &SnapshotScope,
        name: &SnapshotName,
        tree: &Path,
        parent: Option<(&SnapshotName, ChangeDetection)>,
    ) -> Result<SnapshotInfo, SnapshotStoreError> {
        let (token, _guard) = self.start()?;
        check_tree(tree).await?;
        let stage = Arc::new(SnapshotStage::default());
        let backend = Arc::new(self.backend(scope, &token)?.staging_in(stage.clone()));
        let key = self.key.clone();
        let policy = self.policy;
        let name = name.clone();
        let tree: Box<Path> = tree.into();
        let parent = parent.map(|(parent, detection)| (parent.clone(), detection));
        let low_priority = self.low_priority;
        let staged = self
            .blocking(Operation::Save, move || {
                low_priority.run("fs-snap-save", move || {
                    stage_save(backend, &stage, &key, &policy, &name, &tree, parent)
                })
            })
            .await?;
        let (staged, info) = staged.ok_or(SnapshotStoreError::AlreadyExists)?;
        publish(&self.files(scope), &staged, &self.tracker)
            .await
            .map_err(storage_failure)?;
        Ok(info)
    }

    async fn restore(
        &self,
        scope: &SnapshotScope,
        name: &SnapshotName,
        into: &Path,
    ) -> Result<SnapshotInfo, SnapshotStoreError> {
        let (token, _guard) = self.start()?;
        check_destination(into).await?;
        let backend = Arc::new(self.backend(scope, &token)?);
        let key = self.key.clone();
        let options = store_restore_options(&self.policy);
        let name = name.clone();
        let into: Box<Path> = into.into();
        self.blocking(Operation::Restore, move || {
            let Some(repository) = open_existing(backend, &key)? else {
                return Ok(Lookup::Missing);
            };
            match lookup(scope_snapshots(&repository)?, &name) {
                Lookup::Found(snapshot, info) => {
                    restore_snapshot(repository, &snapshot, &into, &options)?;
                    Ok(Lookup::Found(snapshot, info))
                }
                other => Ok(other),
            }
        })
        .await?
        .into_info()?
        .ok_or(SnapshotStoreError::NotFound)
    }

    async fn stat(
        &self,
        scope: &SnapshotScope,
        name: &SnapshotName,
    ) -> Result<Option<SnapshotInfo>, SnapshotStoreError> {
        let (token, _guard) = self.start()?;
        let backend = Arc::new(self.backend(scope, &token)?);
        let key = self.key.clone();
        let name = name.clone();
        self.blocking(Operation::Repository, move || {
            Ok(match open_existing(backend, &key)? {
                Some(repository) => lookup(scope_snapshots(&repository)?, &name),
                None => Lookup::Missing,
            })
        })
        .await?
        .into_info()
    }

    async fn list(
        &self,
        scope: &SnapshotScope,
    ) -> Result<Box<[(SnapshotName, SnapshotInfo)]>, SnapshotStoreError> {
        let (token, _guard) = self.start()?;
        let backend = Arc::new(self.backend(scope, &token)?);
        let key = self.key.clone();
        self.blocking(Operation::Repository, move || {
            Ok(match open_existing(backend, &key)? {
                Some(repository) => newest_first(listed(scope_snapshots(&repository)?)),
                None => Box::default(),
            })
        })
        .await
    }

    async fn delete(
        &self,
        scope: &SnapshotScope,
        name: &SnapshotName,
    ) -> Result<(), SnapshotStoreError> {
        let (token, _guard) = self.start()?;
        let backend = Arc::new(self.backend(scope, &token)?);
        let key = self.key.clone();
        let name = name.clone();
        let freed = self
            .blocking(Operation::Repository, move || {
                let Some(repository) = open_existing(backend, &key)? else {
                    return Ok(None);
                };
                let named = scope_snapshots(&repository)?
                    .readable
                    .into_iter()
                    .filter(|snapshot| snapshot.label == name.as_str())
                    .collect::<Box<[_]>>();
                let ids = named
                    .iter()
                    .map(|snapshot| snapshot.id)
                    .collect::<Box<[SnapshotId]>>();
                repository.delete_snapshots(&ids)?;
                Ok(Some(named.iter().map(added_packed_bytes).sum::<u64>()))
            })
            .await?;
        match freed {
            Some(freed) => self.prune_when_due(scope, &token, freed).await,
            None => Ok(()),
        }
    }

    async fn delete_scope(&self, scope: &SnapshotScope) -> Result<(), SnapshotStoreError> {
        let _operation = self.start()?;
        delete_scope(&self.files(scope))
            .await
            .map_err(storage_failure)
    }

    async fn copy_scope(
        &self,
        from: &SnapshotScope,
        to: &SnapshotScope,
    ) -> Result<(), SnapshotStoreError> {
        let _operation = self.start()?;
        copy_scope(&self.files(from), &self.files(to))
            .await
            .map_err(storage_failure)
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

/// Checks that the tree of a save is a directory at an absolute path.
async fn check_tree(tree: &Path) -> Result<(), SnapshotStoreError> {
    let metadata = tokio::fs::metadata(tree)
        .await
        .map_err(SnapshotStoreError::Source)?;
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

/// Checks that the directory of a restore is an empty directory with a UTF-8 path.
async fn check_destination(into: &Path) -> Result<(), SnapshotStoreError> {
    let refused = |kind, reason: &str| {
        SnapshotStoreError::Destination(std::io::Error::new(
            kind,
            format!("the directory {} {reason}", into.display()),
        ))
    };
    let metadata = tokio::fs::metadata(into)
        .await
        .map_err(SnapshotStoreError::Destination)?;
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
    let first = tokio::fs::read_dir(into)
        .await
        .map_err(SnapshotStoreError::Destination)?
        .next_entry()
        .await
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
) -> anyhow::Result<Option<(StagedSnapshot, SnapshotInfo)>> {
    let (repository, _) = open_or_create(backend, key, &policy.repository)?;
    let before = scope_snapshots(&repository)?;
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
    let created_at = snapshot_time(whole_millis_from(Timestamp::now_utc()), newest);
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
    if has_name(&scope_snapshots(&repository)?, name) {
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

/// Reads each snapshot file of the repository. A failed storage call fails the read, a file that a
/// delete removed after the listing is left out, and each other failure counts as a failed check.
fn scope_snapshots<S: Open>(repository: &RusticRepository<S>) -> anyhow::Result<ScopeSnapshots> {
    repository.list::<SnapshotId>()?.try_fold(
        ScopeSnapshots {
            readable: Vec::new(),
            unreadable: false,
        },
        |mut found, id| match repository.get_file::<SnapshotFile>(&id) {
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
        },
    )
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
fn named<'a>(snapshots: &'a [SnapshotFile], name: &SnapshotName) -> Option<&'a SnapshotFile> {
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
/// packs, and packs of an earlier prune whose grace period is not over. The report does not count a
/// marked pack that no index lists, so the next due prune removes it.
fn leaves_marked_packs(report: &PruneReport) -> bool {
    report.packs_unused > 0 || report.packs_repacked > 0 || report.marked_packs_kept > 0
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
