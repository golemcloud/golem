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

//! Saves, restores and forgets filesystem snapshots in a rustic repository on blob storage.
//!
//! Each scope is one repository. The repository keeps its files in the blob storage namespace of
//! the scope, through [`backend::BlobBackend`]. No type of rustic is in the interface of this
//! module.

mod backend;

#[cfg(test)]
mod tests;

use super::{SnapshotName, SnapshotScope};
use crate::sandbox_filesystem::{NativeOperation, NativeStorageProfile, execute_native};
use anyhow::Context;
use backend::BlobBackend;
use golem_common::model::Timestamp;
use golem_service_base::storage::blob::BlobStorage;
use rustic_core::repofile::{MasterKey, SnapshotFile};
use rustic_core::{
    BackupOptions, ConfigOptions, Credentials, KeyOptions, LocalDestination, LsOptions, Open,
    OpenStatus, ParentOptions, PathList, Repository as RusticRepository, RepositoryBackends,
    RepositoryOptions, RestoreOptions, RusticResult, SnapshotGroupCriterion, SnapshotOptions,
};
use std::fmt::{Debug, Formatter};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::runtime::Handle;

/// The key that encrypts a repository.
///
/// The key has 64 bytes: 32 bytes of the AES-256 key, then 16 bytes of the number `k` and 16
/// bytes of the number `r` of Poly1305-AES.
#[derive(Clone, PartialEq, Eq)]
pub(super) struct RepositoryKey([u8; 64]);

impl RepositoryKey {
    /// Gives the key with the bytes.
    pub(super) fn new(bytes: [u8; 64]) -> Self {
        Self(bytes)
    }

    /// Gives the key as a rustic master key.
    fn master_key(&self) -> MasterKey {
        let (encrypt, mac) = self.0.split_at(32);
        let (k, r) = mac.split_at(16);
        let mut key = MasterKey::new();
        key.encrypt = encrypt.to_vec();
        key.mac.k = k.to_vec();
        key.mac.r = r.to_vec();
        key
    }
}

impl Debug for RepositoryKey {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("RepositoryKey(..)")
    }
}

/// A part of an operation on a repository.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum OperationPhase {
    /// Makes a repository in a scope that has none.
    Create,
    /// Opens the repository of the scope.
    Open,
    /// Finds the snapshots with the name.
    Lookup,
    /// Loads the index of the repository.
    IndexLoad,
    /// Reads the tree and writes the new data and the snapshot.
    Backup,
    /// Finds the data that the restore needs.
    RestorePlan,
    /// Writes the tree into the directory.
    Restore,
}

/// The time that one part of an operation took.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct PhaseTime {
    pub(super) phase: OperationPhase,
    pub(super) wall: Duration,
}

/// What a save did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct SaveReport {
    /// The id of the snapshot file, in hex.
    pub(super) snapshot: Box<str>,
    /// The time of the snapshot.
    pub(super) created_at: Timestamp,
    /// The id of the parent snapshot, in hex, when the save had one.
    pub(super) parent: Option<Box<str>>,
    pub(super) files_new: u64,
    pub(super) files_changed: u64,
    pub(super) files_unmodified: u64,
    pub(super) dirs_new: u64,
    pub(super) dirs_changed: u64,
    pub(super) dirs_unmodified: u64,
    /// The sum of the sizes of the files that the save read or found unchanged.
    pub(super) bytes_processed: u64,
    /// The bytes of the new blobs before compression.
    pub(super) data_added: u64,
    /// The bytes of the new blobs after compression.
    pub(super) data_added_packed: u64,
    pub(super) data_blobs: u64,
    pub(super) tree_blobs: u64,
    pub(super) phases: Box<[PhaseTime]>,
}

/// What a restore did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct RestoreReport {
    /// The number of files that the restore wrote.
    pub(super) files: u64,
    /// The number of directories that the restore made.
    pub(super) dirs: u64,
    /// The bytes that the restore wrote into files.
    pub(super) bytes: u64,
    pub(super) phases: Box<[PhaseTime]>,
}

/// The rustic repository of one scope in blob storage.
///
/// Each operation opens the repository again, with the master key and without the rustic cache.
/// Each operation runs rustic on a blocking thread of the async runtime, and must be called from a
/// task of that runtime. A snapshot has its name as its label.
pub(super) struct Repository {
    storage: Arc<dyn BlobStorage>,
    scope: SnapshotScope,
    key: RepositoryKey,
}

impl Repository {
    /// Gives the repository of the scope in the storage, which the key opens.
    pub(super) fn new(
        storage: Arc<dyn BlobStorage>,
        scope: SnapshotScope,
        key: RepositoryKey,
    ) -> Self {
        Self {
            storage,
            scope,
            key,
        }
    }

    /// Saves the directory tree `tree` as a snapshot with the name.
    ///
    /// A save in a scope without a repository makes the repository first. The newest snapshot of
    /// the scope is the parent of the save, so the save reads only the files whose metadata
    /// changed since that snapshot. The snapshot keeps the paths relative to `tree`.
    pub(super) async fn save(
        &self,
        name: &SnapshotName,
        tree: &Path,
    ) -> anyhow::Result<SaveReport> {
        let backend = self.backend()?;
        let key = self.key.clone();
        let name = name.clone();
        let tree: Box<Path> = tree.into();
        run_blocking(move || save(backend, &key, &name, &tree)).await
    }

    /// Restores the newest snapshot with the name into the empty directory `into`.
    ///
    /// The result is `None` when no snapshot has the name.
    pub(super) async fn restore(
        &self,
        name: &SnapshotName,
        into: &Path,
    ) -> anyhow::Result<Option<RestoreReport>> {
        let backend = self.backend()?;
        let key = self.key.clone();
        let name = name.clone();
        let into: Box<Path> = into.into();
        run_blocking(move || restore(backend, &key, &name, &into)).await
    }

    /// Deletes the snapshot files with the name, and gives their number.
    ///
    /// The data of a deleted snapshot stays in the repository.
    pub(super) async fn forget(&self, name: &SnapshotName) -> anyhow::Result<u64> {
        let backend = self.backend()?;
        let key = self.key.clone();
        let name = name.clone();
        run_blocking(move || forget(backend, &key, &name)).await
    }

    /// Gives a backend over the namespace of the scope, which waits on the current runtime.
    fn backend(&self) -> anyhow::Result<Arc<BlobBackend>> {
        let runtime = Handle::try_current()
            .context("a filesystem snapshot operation needs an async runtime")?;
        Ok(Arc::new(BlobBackend::new(
            self.storage.clone(),
            self.scope.0.clone(),
            runtime,
        )))
    }
}

/// Runs the task on a blocking thread of the runtime.
async fn run_blocking<R: Send + 'static>(
    task: impl FnOnce() -> anyhow::Result<R> + Send + 'static,
) -> anyhow::Result<R> {
    execute_native(
        NativeStorageProfile::Unknown,
        NativeOperation::TreeCopy,
        task,
    )
    .await?
}

fn save(
    backend: Arc<BlobBackend>,
    key: &RepositoryKey,
    name: &SnapshotName,
    tree: &Path,
) -> anyhow::Result<SaveReport> {
    let started = Instant::now();
    let (repository, opening) = open_or_create(backend, key)?;
    let open = PhaseTime {
        phase: opening,
        wall: started.elapsed(),
    };
    let (repository, index) = timed(OperationPhase::IndexLoad, || repository.to_indexed_ids())?;
    let (snapshot, backup) = timed(OperationPhase::Backup, || {
        repository.backup(
            &backup_options(),
            &PathList::from_iter(Some(tree.to_path_buf())),
            snapshot_options(name).to_snapshot()?,
        )
    })?;
    Ok(save_report(&snapshot, Box::new([open, index, backup])))
}

fn restore(
    backend: Arc<BlobBackend>,
    key: &RepositoryKey,
    name: &SnapshotName,
    into: &Path,
) -> anyhow::Result<Option<RestoreReport>> {
    let (repository, open) = timed(OperationPhase::Open, || open_existing(backend, key))?;
    let Some(repository) = repository else {
        return Ok(None);
    };
    let (snapshot, lookup) = timed(OperationPhase::Lookup, || {
        named_snapshots(&repository, name).map(|snapshots| {
            snapshots
                .into_iter()
                .max_by(|left, right| left.time.cmp(&right.time))
        })
    })?;
    let Some(snapshot) = snapshot else {
        return Ok(None);
    };
    let (repository, index) = timed(OperationPhase::IndexLoad, || repository.to_indexed())?;
    let into = into
        .to_str()
        .context("the directory of a restore must have a UTF-8 path")?;
    let destination = LocalDestination::new(into, false, false)?;
    let node = repository.node_from_snapshot_and_path(&snapshot, "")?;
    let entries = repository.ls(&node, &LsOptions::default())?;
    let options = RestoreOptions::default();
    let (plan, planning) = timed(OperationPhase::RestorePlan, || {
        repository.prepare_restore(&options, entries.clone(), &destination, false)
    })?;
    let files = plan.stats.files.restore;
    let dirs = plan.stats.dirs.restore;
    let bytes = plan.restore_size;
    let ((), writing) = timed(OperationPhase::Restore, || {
        repository.restore(plan, &options, entries, &destination)
    })?;
    Ok(Some(RestoreReport {
        files,
        dirs,
        bytes,
        phases: Box::new([open, lookup, index, planning, writing]),
    }))
}

fn forget(
    backend: Arc<BlobBackend>,
    key: &RepositoryKey,
    name: &SnapshotName,
) -> anyhow::Result<u64> {
    let Some(repository) = open_existing(backend, key)? else {
        return Ok(0);
    };
    let ids: Box<[_]> = named_snapshots(&repository, name)?
        .into_iter()
        .map(|snapshot| snapshot.id)
        .collect();
    repository.delete_snapshots(&ids)?;
    Ok(u64::try_from(ids.len())?)
}

/// Opens the repository, or makes it when the scope has none.
fn open_or_create(
    backend: Arc<BlobBackend>,
    key: &RepositoryKey,
) -> RusticResult<(RusticRepository<OpenStatus>, OperationPhase)> {
    let repository = unopened(backend)?;
    let credentials = Credentials::Masterkey(key.master_key());
    match repository.config_id()? {
        Some(_) => repository
            .open(&credentials)
            .map(|repository| (repository, OperationPhase::Open)),
        None => repository
            .init(
                &credentials,
                &KeyOptions::default(),
                &ConfigOptions::default(),
            )
            .map(|repository| (repository, OperationPhase::Create)),
    }
}

/// Opens the repository, or gives `None` when the scope has none.
fn open_existing(
    backend: Arc<BlobBackend>,
    key: &RepositoryKey,
) -> RusticResult<Option<RusticRepository<OpenStatus>>> {
    let repository = unopened(backend)?;
    match repository.config_id()? {
        Some(_) => repository
            .open(&Credentials::Masterkey(key.master_key()))
            .map(Some),
        None => Ok(None),
    }
}

fn unopened(backend: Arc<BlobBackend>) -> RusticResult<RusticRepository<()>> {
    RusticRepository::new(
        &repository_options(),
        &RepositoryBackends::new(backend, None),
    )
}

/// The options of each repository: no rustic cache.
fn repository_options() -> RepositoryOptions {
    RepositoryOptions::default().no_cache(true)
}

/// The options of each save.
///
/// A snapshot keeps the paths relative to the saved tree. The parent of a save is the newest
/// snapshot of the repository, because the group of the parent has no criterion.
fn backup_options() -> BackupOptions {
    BackupOptions::default()
        .as_path(PathBuf::from("/"))
        .parent_opts(ParentOptions::default().group_by(SnapshotGroupCriterion::new()))
}

/// The options of the snapshot of a save: the name is the label.
fn snapshot_options(name: &SnapshotName) -> SnapshotOptions {
    SnapshotOptions::default().label(name.as_str().to_string())
}

/// Gives each snapshot of the repository whose label is the name.
fn named_snapshots<S: Open>(
    repository: &RusticRepository<S>,
    name: &SnapshotName,
) -> RusticResult<Vec<SnapshotFile>> {
    Ok(repository
        .get_all_snapshots()?
        .into_iter()
        .filter(|snapshot| snapshot.label == name.as_str())
        .collect())
}

fn save_report(snapshot: &SnapshotFile, phases: Box<[PhaseTime]>) -> SaveReport {
    let summary = snapshot.summary.clone().unwrap_or_default();
    SaveReport {
        snapshot: snapshot.id.to_hex().as_str().into(),
        created_at: Timestamp::from(
            u64::try_from(snapshot.time.timestamp().as_millisecond()).unwrap_or_default(),
        ),
        parent: snapshot
            .parent
            .map(|parent| parent.to_hex().as_str().into()),
        files_new: summary.files_new,
        files_changed: summary.files_changed,
        files_unmodified: summary.files_unmodified,
        dirs_new: summary.dirs_new,
        dirs_changed: summary.dirs_changed,
        dirs_unmodified: summary.dirs_unmodified,
        bytes_processed: summary.total_bytes_processed,
        data_added: summary.data_added,
        data_added_packed: summary.data_added_packed,
        data_blobs: summary.data_blobs,
        tree_blobs: summary.tree_blobs,
        phases,
    }
}

/// Runs the work and gives its result with the time of the phase.
fn timed<T>(
    phase: OperationPhase,
    work: impl FnOnce() -> RusticResult<T>,
) -> anyhow::Result<(T, PhaseTime)> {
    let started = Instant::now();
    let value = work()?;
    Ok((
        value,
        PhaseTime {
            phase,
            wall: started.elapsed(),
        },
    ))
}
