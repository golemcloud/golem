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
#[cfg_attr(not(test), allow(dead_code))]
mod fault;

#[cfg(test)]
mod holding;
#[cfg(test)]
mod tests;

use super::{SnapshotName, SnapshotScope};
use crate::sandbox_filesystem::{NativeOperation, NativeStorageProfile, execute_native};
use anyhow::Context;
use backend::BlobBackend;
use bytesize::ByteSize;
use golem_common::model::Timestamp;
use golem_service_base::storage::blob::BlobStorage;
use rustic_core::jiff::Span;
use rustic_core::repofile::{Chunker, ConfigFile, MasterKey, SnapshotFile};
use rustic_core::{
    BackupOptions, ConfigOptions, Credentials, KeyOptions, LimitOption, LocalDestination,
    LsOptions, Open, OpenStatus, ParentOptions, PathList, PruneOptions, PruneStats,
    Repository as RusticRepository, RepositoryBackends, RepositoryOptions, RestoreOptions,
    RusticResult, SnapshotGroupCriterion, SnapshotOptions,
};
use std::fmt::{Debug, Formatter};
use std::num::{NonZeroI32, NonZeroU32, NonZeroUsize};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::runtime::Handle;

/// The longest time that one call of a repository waits for the blob storage.
///
/// A call that gets no answer within this time fails, and its operation fails with it. The value
/// stops a call that does not return. It is not a limit for a slow call. On S3, with the retries of
/// the S3 storage, a write of a pack took at most 1.7 s with eight saves at the same time. A ranged
/// read of a pack took at most 1.5 s under the CPU request of an executor. Keep the value at least
/// 10 times the longest measured call.
pub(super) const STORAGE_CALL_DEADLINE: Duration = Duration::from_secs(30);

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
    /// Finds the packs that a prune deletes or repacks.
    PrunePlan,
    /// Repacks, marks and deletes the packs of the plan of a prune.
    Prune,
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

/// How a repository cuts files into chunks.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum Chunking {
    /// Content-defined chunks of about 1 MiB, the default of rustic.
    #[default]
    Rabin,
    /// Chunks of the fixed size in bytes. Only the last chunk of a file can be smaller.
    Fixed(NonZeroU32),
}

/// How a repository compresses the data that it keeps.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum Compression {
    /// The default level of zstd, which is level 3. The config file of the repository holds no
    /// compression, and rustic gives zstd the level 0, which zstd reads as its default level.
    #[default]
    Default,
    /// No compression. The config file of the repository holds the compression 0.
    Off,
    /// The zstd level, from 1 to 22 or from -7 to -1. A level of 0 is not a level here, because
    /// rustic reads the compression 0 as no compression.
    Level(NonZeroI32),
}

/// The settings that a repository gets when a save makes it. A repository that exists keeps its
/// own settings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct RepositorySettings {
    pub(super) chunking: Chunking,
    pub(super) compression: Compression,
    /// Whether a save decompresses and decrypts each pack again before it writes the pack.
    pub(super) extra_verify: bool,
}

impl RepositorySettings {
    /// The settings of rustic: Rabin chunks, the default zstd level, and the extra verification.
    pub(super) const DEFAULT: Self = Self {
        chunking: Chunking::Rabin,
        compression: Compression::Default,
        extra_verify: true,
    };
}

impl Default for RepositorySettings {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// How a save finds the files that did not change since its parent.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum ChangeDetection {
    /// A file is unchanged when its type, size, modification time and change time equal those in
    /// the parent. This is the default of rustic. A copy of a tree gives each file a new change
    /// time, so a save of a copy reads every file.
    #[default]
    Ctime,
    /// A file is unchanged when its type, size and modification time equal those in the parent.
    /// A save does not see a change that keeps the size and gives the file its old modification
    /// time again.
    SizeMtime,
}

/// The settings of one save.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct SaveSettings {
    /// The number of threads of each parallel stage of the save. `None` is the number of CPUs
    /// that the process can use.
    pub(super) threads: Option<NonZeroUsize>,
    pub(super) detection: ChangeDetection,
}

impl SaveSettings {
    /// The settings of rustic: the number of CPUs, and the change detection of rustic.
    pub(super) const DEFAULT: Self = Self {
        threads: None,
        detection: ChangeDetection::Ctime,
    };
}

impl Default for SaveSettings {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// Which packs a prune repacks.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum RepackLimits {
    /// The limits of rustic: up to 5% unused data stays after the prune, and a prune repacks at
    /// most 10% of the repository.
    #[default]
    Rustic,
    /// No unused data stays, and a prune repacks without a limit. Each pack that holds a blob
    /// that no snapshot uses is repacked or deleted.
    Unlimited,
}

/// The settings of one prune.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct PruneSettings {
    /// Whether a repack copies the blobs as they are. Without it, a repack decrypts,
    /// decompresses, compresses and encrypts each blob that it keeps.
    pub(super) fast_repack: bool,
    /// How long a pack stays after a prune marks it for deletion. A later prune deletes a marked
    /// pack when this time is over.
    pub(super) keep_delete: Duration,
    pub(super) repack: RepackLimits,
}

impl Default for PruneSettings {
    /// The settings of rustic: no fast repack, 23 hours before a marked pack goes, and the
    /// limits of rustic.
    fn default() -> Self {
        Self {
            fast_repack: false,
            keep_delete: Duration::from_secs(23 * 3600),
            repack: RepackLimits::default(),
        }
    }
}

/// What a prune did: the plan of the prune, in bytes and packs.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct PruneReport {
    pub(super) packs_used: u64,
    pub(super) packs_partly_used: u64,
    pub(super) packs_unused: u64,
    pub(super) packs_repacked: u64,
    pub(super) packs_kept: u64,
    /// The packs that the prune deletes, among the packs that an earlier prune marked.
    pub(super) marked_packs_deleted: u64,
    /// The bytes of the marked packs that the prune deletes.
    pub(super) marked_bytes_deleted: u64,
    /// The packs that an earlier prune marked and that stay marked.
    pub(super) marked_packs_kept: u64,
    /// The bytes of the blobs that snapshots use.
    pub(super) bytes_used: u64,
    /// The bytes of the blobs that no snapshot uses.
    pub(super) bytes_unused: u64,
    /// The bytes of the unused blobs in the packs that the prune removes.
    pub(super) bytes_removed: u64,
    /// The bytes of the blobs that the repack copies.
    pub(super) bytes_repacked: u64,
    /// The bytes of the unused blobs that the repack leaves out.
    pub(super) bytes_repack_removed: u64,
    pub(super) index_files: u64,
    pub(super) index_files_rebuilt: u64,
    pub(super) phases: Box<[PhaseTime]>,
}

/// What an inspection of a repository found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct InspectReport {
    /// The number of snapshots of the repository.
    pub(super) snapshots: u64,
    /// Whether a snapshot has the name.
    pub(super) found: bool,
    /// The settings of the repository, as its config file gives them.
    pub(super) settings: RepositorySettings,
    pub(super) phases: Box<[PhaseTime]>,
}

/// The rustic repository of one scope in blob storage.
///
/// Each operation opens the repository again, with the master key and without the rustic cache.
/// Each operation runs rustic on a blocking thread of the async runtime, and must be called from a
/// task of that runtime. That runtime must be a multi-thread runtime, because each call on the
/// storage has a deadline. A snapshot has its name as its label.
pub(super) struct Repository {
    storage: Arc<dyn BlobStorage>,
    scope: SnapshotScope,
    key: RepositoryKey,
    deadline: Duration,
    settings: RepositorySettings,
}

impl Repository {
    /// Gives the repository of the scope in the storage, which the key opens. Each call on the
    /// storage waits for at most `deadline`, and a call without an answer fails its operation.
    pub(super) fn new(
        storage: Arc<dyn BlobStorage>,
        scope: SnapshotScope,
        key: RepositoryKey,
        deadline: Duration,
    ) -> Self {
        Self {
            storage,
            scope,
            key,
            deadline,
            settings: RepositorySettings::default(),
        }
    }

    /// Gives the repository with the settings that a save uses when it makes the repository.
    pub(super) fn with_settings(self, settings: RepositorySettings) -> Self {
        Self { settings, ..self }
    }

    /// Saves the directory tree `tree` as a snapshot with the name, with the default settings of
    /// a save.
    pub(super) async fn save(
        &self,
        name: &SnapshotName,
        tree: &Path,
    ) -> anyhow::Result<SaveReport> {
        self.save_with(name, tree, SaveSettings::default()).await
    }

    /// Saves the directory tree `tree` as a snapshot with the name.
    ///
    /// A save in a scope without a repository makes the repository first, with the settings of
    /// this value. The newest snapshot of the scope is the parent of the save, so the save reads
    /// only the files that `settings` finds changed since that snapshot. The snapshot keeps the
    /// paths relative to `tree`.
    pub(super) async fn save_with(
        &self,
        name: &SnapshotName,
        tree: &Path,
        settings: SaveSettings,
    ) -> anyhow::Result<SaveReport> {
        let backend = self.backend()?;
        let key = self.key.clone();
        let repository_settings = self.settings;
        let name = name.clone();
        let tree: Box<Path> = tree.into();
        run_blocking(move || save(backend, &key, &repository_settings, &settings, &name, &tree))
            .await
    }

    /// Restores the newest snapshot with the name into the empty directory `into`.
    ///
    /// `reader_threads` is the number of threads that read the data of the files. Each of these
    /// threads holds the data of one read, so the number sets the memory of the restore. `None`
    /// keeps the default of rustic, which is 20 threads. The result is `None` when no snapshot has
    /// the name.
    pub(super) async fn restore(
        &self,
        name: &SnapshotName,
        into: &Path,
        reader_threads: Option<NonZeroUsize>,
    ) -> anyhow::Result<Option<RestoreReport>> {
        let backend = self.backend()?;
        let key = self.key.clone();
        let name = name.clone();
        let into: Box<Path> = into.into();
        run_blocking(move || restore(backend, &key, &name, &into, reader_threads)).await
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

    /// Prunes the repository: deletes the packs that an earlier prune marked and whose time to
    /// stay is over, marks the packs that no snapshot uses, and repacks the packs that hold both
    /// used and unused blobs. The result is `None` when the scope has no repository.
    pub(super) async fn prune(
        &self,
        settings: PruneSettings,
    ) -> anyhow::Result<Option<PruneReport>> {
        let backend = self.backend()?;
        let key = self.key.clone();
        run_blocking(move || prune(backend, &key, &settings)).await
    }

    /// Opens the repository, finds the snapshots with the name, and loads the index, as a
    /// restore does before it reads data. The result is `None` when the scope has no repository.
    pub(super) async fn inspect(
        &self,
        name: &SnapshotName,
    ) -> anyhow::Result<Option<InspectReport>> {
        let backend = self.backend()?;
        let key = self.key.clone();
        let name = name.clone();
        run_blocking(move || inspect(backend, &key, &name)).await
    }

    /// Gives a backend over the namespace of the scope, which waits on the current runtime for at
    /// most the deadline.
    fn backend(&self) -> anyhow::Result<Arc<BlobBackend>> {
        let runtime = Handle::try_current()
            .context("a filesystem snapshot operation needs an async runtime")?;
        Ok(Arc::new(BlobBackend::new(
            self.storage.clone(),
            self.scope.0.clone(),
            runtime,
            self.deadline,
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
    repository_settings: &RepositorySettings,
    settings: &SaveSettings,
    name: &SnapshotName,
    tree: &Path,
) -> anyhow::Result<SaveReport> {
    let started = Instant::now();
    let (repository, opening) = open_or_create(backend, key, repository_settings)?;
    let open = PhaseTime {
        phase: opening,
        wall: started.elapsed(),
    };
    let (repository, index) = timed(OperationPhase::IndexLoad, || repository.to_indexed_ids())?;
    let (snapshot, backup) = timed(OperationPhase::Backup, || {
        repository.backup(
            &backup_options(settings),
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
    reader_threads: Option<NonZeroUsize>,
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
    let options = RestoreOptions::default().reader_threads(reader_threads);
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

fn prune(
    backend: Arc<BlobBackend>,
    key: &RepositoryKey,
    settings: &PruneSettings,
) -> anyhow::Result<Option<PruneReport>> {
    let (repository, open) = timed(OperationPhase::Open, || open_existing(backend, key))?;
    let Some(repository) = repository else {
        return Ok(None);
    };
    let options = prune_options(settings)?;
    let (plan, planning) = timed(OperationPhase::PrunePlan, || {
        repository.prune_plan(&options)
    })?;
    let report = prune_report(&plan.stats);
    let ((), pruning) = timed(OperationPhase::Prune, || repository.prune(&options, plan))?;
    Ok(Some(PruneReport {
        phases: Box::new([open, planning, pruning]),
        ..report
    }))
}

fn inspect(
    backend: Arc<BlobBackend>,
    key: &RepositoryKey,
    name: &SnapshotName,
) -> anyhow::Result<Option<InspectReport>> {
    let (repository, open) = timed(OperationPhase::Open, || open_existing(backend, key))?;
    let Some(repository) = repository else {
        return Ok(None);
    };
    let ((snapshots, found), lookup) = timed(OperationPhase::Lookup, || {
        repository.get_all_snapshots().map(|snapshots| {
            (
                snapshots.len(),
                snapshots
                    .iter()
                    .any(|snapshot| snapshot.label == name.as_str()),
            )
        })
    })?;
    let settings = repository_settings(repository.config())?;
    let (_, index) = timed(OperationPhase::IndexLoad, || repository.to_indexed())?;
    Ok(Some(InspectReport {
        snapshots: u64::try_from(snapshots)?,
        found,
        settings,
        phases: Box::new([open, lookup, index]),
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
    settings: &RepositorySettings,
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
                &config_options(settings),
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

/// The options of a repository that a save makes.
///
/// A setting that equals the default of rustic stays unset, so the config file of a repository
/// with the default settings is the config file that rustic writes without options.
fn config_options(settings: &RepositorySettings) -> ConfigOptions {
    let options = match settings.chunking {
        Chunking::Rabin => ConfigOptions::default(),
        Chunking::Fixed(size) => ConfigOptions::default()
            .set_chunker(Chunker::FixedSize)
            .set_chunk_size(ByteSize::b(u64::from(size.get()))),
    };
    let options = match settings.compression {
        Compression::Default => options,
        Compression::Off => options.set_compression(0),
        Compression::Level(level) => options.set_compression(level.get()),
    };
    if settings.extra_verify {
        options
    } else {
        options.set_extra_verify(false)
    }
}

/// Gives the settings of a repository from its config file, or an error when the config file has
/// fixed chunks whose size is not a size of [`Chunking::Fixed`].
fn repository_settings(config: &ConfigFile) -> anyhow::Result<RepositorySettings> {
    Ok(RepositorySettings {
        chunking: match config.chunker() {
            Chunker::Rabin => Chunking::Rabin,
            Chunker::FixedSize => Chunking::Fixed(
                u32::try_from(config.chunk_size())
                    .ok()
                    .and_then(NonZeroU32::new)
                    .with_context(|| {
                        format!(
                            "the repository has fixed chunks of {} bytes, which is not 1 to {} bytes",
                            config.chunk_size(),
                            u32::MAX
                        )
                    })?,
            ),
        },
        compression: match config.compression.map(NonZeroI32::new) {
            None => Compression::Default,
            Some(None) => Compression::Off,
            Some(Some(level)) => Compression::Level(level),
        },
        extra_verify: config.extra_verify(),
    })
}

/// The options of a save.
///
/// A snapshot keeps the paths relative to the saved tree. The parent of a save is the newest
/// snapshot of the repository, because the group of the parent has no criterion.
fn backup_options(settings: &SaveSettings) -> BackupOptions {
    BackupOptions::default()
        .as_path(PathBuf::from("/"))
        .parent_opts(
            ParentOptions::default()
                .group_by(SnapshotGroupCriterion::new())
                .ignore_ctime(settings.detection == ChangeDetection::SizeMtime),
        )
        .threads(settings.threads)
}

/// The options of a prune.
fn prune_options(settings: &PruneSettings) -> anyhow::Result<PruneOptions> {
    let options = PruneOptions::default()
        .fast_repack(settings.fast_repack)
        .keep_delete(Span::try_from(settings.keep_delete)?);
    Ok(match settings.repack {
        RepackLimits::Rustic => options,
        RepackLimits::Unlimited => options
            .max_unused(LimitOption::Percentage(0))
            .max_repack(LimitOption::Unlimited),
    })
}

/// Gives the numbers of the plan of a prune, without phase times.
fn prune_report(stats: &PruneStats) -> PruneReport {
    let blobs = stats.size_sum();
    PruneReport {
        packs_used: stats.packs.used,
        packs_partly_used: stats.packs.partly_used,
        packs_unused: stats.packs.unused,
        packs_repacked: stats.packs.repack,
        packs_kept: stats.packs.keep,
        marked_packs_deleted: stats.packs_to_delete.remove,
        marked_bytes_deleted: stats.size_to_delete.remove,
        marked_packs_kept: stats.packs_to_delete.keep,
        bytes_used: blobs.used,
        bytes_unused: blobs.unused,
        bytes_removed: blobs.remove,
        bytes_repacked: blobs.repack,
        bytes_repack_removed: blobs.repackrm,
        index_files: stats.index_files,
        index_files_rebuilt: stats.index_files_rebuild,
        phases: Box::default(),
    }
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
