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

//! Concrete rustic repository opening, encryption keys and explicit options.

use super::blob_backend::BlobBackend;
use super::failure_classification;
use rustic_core::repofile::{Chunker, MasterKey};
use rustic_core::{
    BackupOptions, ConfigOptions, Credentials, KeyOptions, OpenStatus, ParentOptions,
    Repository as RusticRepository, RepositoryBackends, RepositoryOptions, RusticResult,
    SnapshotGroupCriterion,
};
use std::fmt::{Debug, Formatter};
use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::sync::Arc;

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

/// Opens the repository, or makes it when the scope has none. A new repository gets
/// [`config_options`].
///
/// When another writer makes the config file of the repository first, the call opens the
/// repository of that writer.
pub(super) fn open_or_create(
    backend: Arc<BlobBackend>,
    key: &RepositoryKey,
) -> RusticResult<RusticRepository<OpenStatus>> {
    let repository = unopened(backend.clone())?;
    let credentials = Credentials::Masterkey(key.master_key());
    match repository.config_id()? {
        Some(_) => repository.open(&credentials),
        None => match repository.init(&credentials, &KeyOptions::default(), &config_options()) {
            Ok(repository) => Ok(repository),
            Err(error) if failure_classification::is_config_exists(&*error) => {
                unopened(backend)?.open(&credentials)
            }
            Err(error) => Err(error),
        },
    }
}

/// Opens the repository, or gives `None` when the scope has none.
pub(super) fn open_existing(
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

/// The options of each repository: no rustic cache, and an index for a restore that also holds the
/// blobs of the packs that a prune marked for deletion. A save can reuse a blob of a pack that a
/// prune marks at the same time, because the prune did not see the new snapshot; the next prune
/// sees the snapshot and keeps the pack.
fn repository_options() -> RepositoryOptions {
    RepositoryOptions::default()
        .no_cache(true)
        .index_marked_packs(true)
}

/// The chunker of a new repository. It cuts files into content-defined chunks, so an insert moves
/// only the chunks around it.
const CHUNKER: Chunker = Chunker::Rabin;

/// The zstd level of a new repository, which compresses each blob.
const ZSTD_LEVEL: i32 = 3;

/// Whether each write into a repository made with these options checks its data. The write
/// decrypts and decompresses each blob and each file again after it encrypts them. So data that
/// does not read back gives an error before it is written.
const EXTRA_VERIFY: bool = true;

/// The options of a repository that a save makes: [`CHUNKER`], [`ZSTD_LEVEL`] and
/// [`EXTRA_VERIFY`].
fn config_options() -> ConfigOptions {
    ConfigOptions::default()
        .set_chunker(CHUNKER)
        .set_compression(ZSTD_LEVEL)
        .set_extra_verify(EXTRA_VERIFY)
}

/// The options that each save shares. The snapshot keeps the paths relative to the saved tree. The
/// group of the parent has no criterion. Each parallel stage of the save has `threads` threads.
/// `None` is the number of CPUs that the process can use.
pub(super) fn backup_options(threads: Option<NonZeroUsize>) -> BackupOptions {
    BackupOptions::default()
        .as_path(PathBuf::from("/"))
        .parent_opts(ParentOptions::default().group_by(SnapshotGroupCriterion::new()))
        .threads(threads)
}

#[cfg(test)]
mod tests;
