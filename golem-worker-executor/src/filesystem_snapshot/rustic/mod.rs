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
mod fault;
mod files;
mod priority;
mod prune;
mod publish;
mod scope;
mod store;

pub(crate) use store::RusticSnapshotStore;

#[cfg(test)]
mod tests;

use crate::sandbox_filesystem::{NativeOperation, NativeStorageProfile, execute_native};
use anyhow::Context;
use backend::BlobBackend;
use rustic_core::jiff::Span;
use rustic_core::repofile::{Chunker, MasterKey, SnapshotFile};
use rustic_core::{
    BackupOptions, ConfigOptions, Credentials, KeyOptions, LocalDestination, LsOptions, OpenStatus,
    ParentOptions, PruneOptions, PruneStats, Repository as RusticRepository, RepositoryBackends,
    RepositoryOptions, RestoreOptions, RusticResult, SnapshotGroupCriterion,
};
use std::fmt::{Debug, Formatter};
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

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

/// The settings of one prune. A prune keeps the repack limits of rustic: up to 5% unused data stays
/// after the prune, and a prune repacks at most 10% of the repository.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct PruneSettings {
    /// Whether a repack copies the blobs as they are. Without it, a repack decrypts,
    /// decompresses, compresses and encrypts each blob that it keeps.
    pub(super) fast_repack: bool,
    /// How long a pack stays after a prune marks it for deletion. A later prune deletes a marked
    /// pack when this time is over.
    pub(super) keep_delete: Duration,
}

/// The numbers of the plan of a prune that tell whether it leaves marked packs.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct PruneReport {
    /// The packs that no snapshot uses.
    pub(super) packs_unused: u64,
    pub(super) packs_repacked: u64,
    /// The packs that an earlier prune marked and that stay marked.
    pub(super) marked_packs_kept: u64,
    /// The packs that no index lists. The prune marks each of them.
    pub(super) packs_unindexed: u64,
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

/// Writes the tree of the snapshot into the empty directory `into`.
fn restore_snapshot(
    repository: RusticRepository<OpenStatus>,
    snapshot: &SnapshotFile,
    into: &Path,
    options: &RestoreOptions,
) -> anyhow::Result<()> {
    let repository = repository.to_indexed()?;
    let into = into
        .to_str()
        .context("the directory of a restore must have a UTF-8 path")?;
    let destination = LocalDestination::new(into, false, false)?;
    let node = repository.node_from_snapshot_and_path(snapshot, "")?;
    let entries = repository.ls(&node, &LsOptions::default())?;
    let plan = repository.prepare_restore(options, entries.clone(), &destination, false)?;
    repository.restore(plan, options, entries, &destination)?;
    Ok(())
}

/// Prunes the repository: deletes the packs that an earlier prune marked and whose time to stay
/// is over, marks the packs that no snapshot uses, and repacks the packs that hold both used and
/// unused blobs. The result is `None` when the scope has no repository.
fn prune(
    backend: Arc<BlobBackend>,
    key: &RepositoryKey,
    settings: &PruneSettings,
) -> anyhow::Result<Option<PruneReport>> {
    let Some(repository) = open_existing(backend, key)? else {
        return Ok(None);
    };
    let options = prune_options(settings)?;
    let plan = repository.prune_plan(&options)?;
    let report = prune_report(&plan.stats);
    repository.prune(&options, plan)?;
    Ok(Some(report))
}

/// Opens the repository, or makes it when the scope has none. A new repository gets
/// [`config_options`].
///
/// When another writer makes the config file of the repository first, the call opens the
/// repository of that writer.
fn open_or_create(
    backend: Arc<BlobBackend>,
    key: &RepositoryKey,
) -> RusticResult<RusticRepository<OpenStatus>> {
    let repository = unopened(backend.clone())?;
    let credentials = Credentials::Masterkey(key.master_key());
    match repository.config_id()? {
        Some(_) => repository.open(&credentials),
        None => match repository.init(&credentials, &KeyOptions::default(), &config_options()) {
            Ok(repository) => Ok(repository),
            Err(error) if fault::is_config_exists(&*error) => unopened(backend)?.open(&credentials),
            Err(error) => Err(error),
        },
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

/// The chunker of a new repository. It cuts files into content-defined chunks, so an insert moves
/// only the chunks around it.
const CHUNKER: Chunker = Chunker::Rabin;

/// The zstd level of a new repository, which compresses each blob.
const ZSTD_LEVEL: i32 = 3;

/// Whether a save of a new repository decompresses and decrypts each pack again before it writes
/// the pack, so a pack that does not read back is never written.
const EXTRA_VERIFY: bool = true;

/// The options of a repository that a save makes: [`CHUNKER`], [`ZSTD_LEVEL`] and
/// [`EXTRA_VERIFY`].
fn config_options() -> ConfigOptions {
    ConfigOptions::default()
        .set_chunker(CHUNKER)
        .set_compression(ZSTD_LEVEL)
        .set_extra_verify(EXTRA_VERIFY)
}

/// The options of a save.
///
/// A snapshot keeps the paths relative to the saved tree. The group of the parent has no
/// criterion.
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
    Ok(PruneOptions::default()
        .fast_repack(settings.fast_repack)
        .keep_delete(Span::try_from(settings.keep_delete)?))
}

/// Gives the numbers of the plan of a prune that tell whether it leaves marked packs.
fn prune_report(stats: &PruneStats) -> PruneReport {
    PruneReport {
        packs_unused: stats.packs.unused,
        packs_repacked: stats.packs.repack,
        marked_packs_kept: stats.packs_to_delete.keep,
        packs_unindexed: stats.packs_unref,
    }
}
