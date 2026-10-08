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

use crate::sandbox_filesystem::{HostDirectory, HostPath};
use anyhow::anyhow;
use async_lock::Mutex;
use futures::TryStreamExt;
use golem_common::model::agent::AgentFileContentHash;
use golem_common::model::environment::EnvironmentId;
use golem_service_base::service::initial_agent_files::InitialAgentFilesService;
use golem_service_base::storage::blob::BlobFailure;
use std::collections::HashMap;
use std::ffi::OsStr;
use std::fmt::{Display, Formatter};
use std::path::Path;
use std::sync::Arc;
use std::sync::Weak;
use std::sync::atomic::AtomicU64;
use tokio::io::AsyncWriteExt;
use tracing::debug;
// Opaque token for read-only files. This is used to ensure that the file is not deleted while it is in use.
// Make sure to not drop this token until you are done with the file.
#[derive(Debug, Clone)]
pub struct FileUseToken {
    _handle: Arc<CacheEntry>,
}

#[derive(Debug, Clone)]
pub(crate) struct InitialFileSource {
    path: HostPath,
    size: u64,
    _token: FileUseToken,
}

impl InitialFileSource {
    pub(crate) fn path(&self) -> &HostPath {
        &self.path
    }

    pub(crate) fn size(&self) -> u64 {
        self.size
    }
}

/// Tells if a later load of an initial-file source can pass where a load failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum InitialFileLoadFailure {
    /// The blob storage could not give the source, or a concurrent download of it stopped
    /// before its end. A later load can pass.
    Unavailable,
    /// The source cannot load: the blob storage has no source with the key, gives a permanent
    /// error, or gives bytes that do not verify, or the local cache failed.
    Failed,
}

/// The error of a load of an initial-file source.
///
/// A concurrent load of the same source gets a clone of the error, so the reason is shared. The
/// reason names the content hash of the source, except for a stopped download, whose error is
/// the placeholder of each new cache entry and is built without formatting.
#[derive(Clone, Debug)]
pub(crate) struct InitialFileLoadError {
    pub(crate) failure: InitialFileLoadFailure,
    reason: Arc<str>,
}

impl InitialFileLoadError {
    fn failed(key: AgentFileContentHash, reason: impl Display) -> Self {
        Self {
            failure: InitialFileLoadFailure::Failed,
            reason: format!("{key}: {reason}").into(),
        }
    }

    /// The error of a failed read of the blob storage: unavailable for a transient error, and
    /// failed for a permanent one ([`BlobFailure::of`]).
    fn of_blob(key: AgentFileContentHash, error: &anyhow::Error) -> Self {
        Self {
            failure: match BlobFailure::of(error) {
                BlobFailure::Transient => InitialFileLoadFailure::Unavailable,
                BlobFailure::Permanent => InitialFileLoadFailure::Failed,
            },
            reason: format!("{key}: {error:#}").into(),
        }
    }

    /// The error that a concurrent load sees when the download of the source stopped before its
    /// end, for example because its task was dropped.
    fn download_stopped() -> Self {
        Self {
            failure: InitialFileLoadFailure::Unavailable,
            reason: "the download of the source stopped before its end".into(),
        }
    }
}

impl Display for InitialFileLoadError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "failed to load an initial-file source: {}",
            self.reason
        )
    }
}

impl std::error::Error for InitialFileLoadError {}

/// Interface for loading immutable, content-addressed initial-file sources.
pub struct FileLoader {
    initial_agent_files_service: Arc<InitialAgentFilesService>,
    cache_dir: HostDirectory,
    // Note: The cache is shared between accounts. One account no accessing data from another account
    // is implicitly done by the key being a hash of the content.
    cache: Cache,
    // When the last reference to a file is dropped, the file is deleted.
    // We need to ensure that no one else is using the file while we are deleting it.
    // To do that, give every file a unique number.
    item_counter: AtomicU64,
}

impl FileLoader {
    /// Makes a loader that keeps its sources in `cache_dir`.
    ///
    /// The loader owns the directory, so the directory and the sources in it go away with the
    /// loader.
    pub(crate) fn new(
        initial_agent_files_service: Arc<InitialAgentFilesService>,
        cache_dir: HostDirectory,
    ) -> Self {
        Self {
            initial_agent_files_service,
            cache: Mutex::new(HashMap::new()),
            cache_dir,
            item_counter: AtomicU64::new(0),
        }
    }

    /// Gives the verified source with the content hash `key` and the size `file_size`. A failure
    /// tells if a later load can pass ([`InitialFileLoadFailure`]).
    pub(crate) async fn get_source(
        &self,
        environment_id: EnvironmentId,
        key: AgentFileContentHash,
        file_size: u64,
    ) -> Result<InitialFileSource, InitialFileLoadError> {
        let cache_entry = self
            .get_or_add_cache_entry(environment_id, key, file_size)
            .await?;
        let (path, size) = {
            let cache_entry_guard = cache_entry.lock().await;
            let entry = cache_entry_guard.as_ref().map_err(Clone::clone)?;
            (entry.path.clone(), entry.size)
        };
        if size != file_size {
            return Err(InitialFileLoadError::failed(
                key,
                format!("Cached initial file size {size} does not match declared size {file_size}"),
            ));
        }
        Ok(InitialFileSource {
            path,
            size,
            _token: FileUseToken {
                _handle: cache_entry,
            },
        })
    }

    async fn get_or_add_cache_entry(
        &self,
        environment_id: EnvironmentId,
        key: AgentFileContentHash,
        file_size: u64,
    ) -> Result<Arc<CacheEntry>, InitialFileLoadError> {
        let cache_entry;
        {
            let maybe_prelocked_entry;
            {
                let mut cache_guard = self.cache.lock().await;
                if let Some(existing_cache_entry) =
                    cache_guard.get(&key).and_then(|weak| weak.upgrade())
                {
                    // we have a file, we can just return it
                    maybe_prelocked_entry = None;
                    cache_entry = existing_cache_entry;
                } else {
                    // insert an entry so no one else tries to download the file
                    cache_entry =
                        Arc::new(Mutex::new(Err(InitialFileLoadError::download_stopped())));

                    // immediately lock the entry so no one accesses the file while we are downloading it
                    maybe_prelocked_entry = Some(cache_entry.lock().await);

                    // we don't want to keep a copy if we are the only ones holding it, so we use a weak reference
                    cache_guard.insert(key, Arc::downgrade(&cache_entry));
                };
                drop(cache_guard);
            };

            // we may need to initialize the entry in case we are the first ones to access it
            if let Some(mut prelocked_entry) = maybe_prelocked_entry {
                debug!("Adding {} to cache", key);

                let counter = self
                    .item_counter
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let downloaded = match self
                    .cache_dir
                    .path()
                    .child(OsStr::new(&counter.to_string()))
                {
                    Ok(path) => self
                        .download_file_to_path_as_read_only(
                            environment_id,
                            path.as_path(),
                            key,
                            file_size,
                        )
                        .await
                        .map(|()| path),
                    Err(error) => Err(InitialFileLoadError::failed(key, error)),
                };

                match downloaded {
                    Ok(path) => {
                        // we successfully downloaded the file and set it to read-only, set the cache entry to the file
                        *prelocked_entry = Ok(InitializedCacheEntry {
                            path,
                            size: file_size,
                        });
                    }
                    Err(e) => {
                        // we failed to set the file to read-only, we need to fail the entry, remove it from the cache and return the error
                        *prelocked_entry = Err(e.clone());
                        self.cache.lock().await.remove(&key);

                        return Err(e);
                    }
                }
                drop(prelocked_entry);
            };
        };
        Ok(cache_entry)
    }

    async fn download_file_to_path_as_read_only(
        &self,
        environment_id: EnvironmentId,
        path: &Path,
        key: AgentFileContentHash,
        expected_size: u64,
    ) -> Result<(), InitialFileLoadError> {
        let failed = |error| InitialFileLoadError::failed(key, error);
        let temporary =
            tempfile::NamedTempFile::new_in(self.cache_dir.path().as_path()).map_err(failed)?;
        self.download_file(environment_id, &temporary, key, expected_size)
            .await?;
        crate::sandbox_filesystem::set_file_permissions(temporary.as_file(), true)
            .map_err(failed)?;
        temporary.as_file().sync_all().map_err(failed)?;
        temporary
            .persist_noclobber(path)
            .map_err(|error| failed(error.error))?;
        Ok(())
    }

    async fn download_file(
        &self,
        environment_id: EnvironmentId,
        temporary: &tempfile::NamedTempFile,
        key: AgentFileContentHash,
        expected_size: u64,
    ) -> Result<(), InitialFileLoadError> {
        debug!("Downloading {} to immutable cache", key);
        let failed_io = |error: std::io::Error| InitialFileLoadError::failed(key, error);
        let failed = |error: anyhow::Error| InitialFileLoadError::failed(key, error);
        let mut data = self
            .initial_agent_files_service
            .get(environment_id, key)
            .await
            .map_err(|error| InitialFileLoadError::of_blob(key, &error))?
            .ok_or_else(|| InitialFileLoadError::failed(key, "File not found"))?;

        let file = tokio::fs::File::from_std(temporary.reopen().map_err(failed_io)?);
        let mut writer = tokio::io::BufWriter::new(file);
        let mut hasher = blake3::Hasher::new();
        let mut actual_size = 0u64;

        while let Some(chunk) = data
            .try_next()
            .await
            .map_err(|error| InitialFileLoadError::of_blob(key, &error))?
        {
            actual_size =
                downloaded_size(actual_size, chunk.len() as u64, expected_size).map_err(failed)?;
            hasher.update(&chunk);
            writer.write_all(&chunk).await.map_err(failed_io)?;
        }

        writer.flush().await.map_err(failed_io)?;
        writer.get_ref().sync_all().await.map_err(failed_io)?;
        verify_download(&hasher.finalize(), &key, actual_size, expected_size).map_err(failed)
    }
}

/// Gives the size of a download after a chunk of `chunk_bytes` bytes, from the size before it.
/// A size that overflows, or that is larger than `expected_size`, gives an error.
fn downloaded_size(
    actual_size: u64,
    chunk_bytes: u64,
    expected_size: u64,
) -> Result<u64, anyhow::Error> {
    let actual_size = actual_size
        .checked_add(chunk_bytes)
        .ok_or_else(|| anyhow!("Downloaded initial file size overflowed"))?;
    if actual_size > expected_size {
        return Err(anyhow!(
            "Downloaded initial file size exceeds declared size {expected_size}"
        ));
    }
    Ok(actual_size)
}

/// Checks a whole download. The content hash must equal the hash of `key`, and then the size must
/// equal `expected_size`. The first check that fails gives the error.
fn verify_download(
    actual_hash: &blake3::Hash,
    key: &AgentFileContentHash,
    actual_size: u64,
    expected_size: u64,
) -> Result<(), anyhow::Error> {
    if actual_hash != key.0.as_blake3_hash() {
        return Err(anyhow!(
            "Downloaded initial file content hash does not match {key}"
        ));
    }
    if actual_size != expected_size {
        return Err(anyhow!(
            "Downloaded initial file size {actual_size} does not match declared size {expected_size}"
        ));
    }
    Ok(())
}

// Scary type, let's break it down:
// Outer Mutex: This is the lock that protects the cache from concurrent access.
// HashMap: The cache itself, mapping keys to weak references to the cache entries.
// InitialComponentFileKey: The key used to identify the cache entry.
// Weak: A weak reference to the cache entry. This is used to avoid keeping the cache entry alive if no one else is using it.
// Mutex: The cache entry itself. This is used to ensure that no one is accessing the file while it is being downloaded.
// Result: The result of the cache entry. This is used to store the file path and any errors that occurred while downloading the file.
// InitializedCacheEntry: The cache entry itself. This is used to store the file path and ensure that the file is deleted when the cache entry is dropped.
type Cache = Mutex<HashMap<AgentFileContentHash, Weak<CacheEntry>>>;

type CacheEntry = Mutex<Result<InitializedCacheEntry, InitialFileLoadError>>;

#[derive(Debug)]
struct InitializedCacheEntry {
    path: HostPath,
    size: u64,
}

impl InitializedCacheEntry {
    #[cfg(test)]
    fn new_for_test(path: HostPath) -> Self {
        Self { path, size: 0 }
    }
}

impl Drop for InitializedCacheEntry {
    fn drop(&mut self) {
        debug!("Removing file {}", self.path.as_path().display());
        std::fs::remove_file(self.path.as_path()).expect("Failed to remove cached component file — executor filesystem is in an inconsistent state");
    }
}

/// A blob storage for tests of the loads of initial-file sources.
#[cfg(test)]
pub(crate) mod scripted_storage {
    use anyhow::{Error, anyhow};
    use async_trait::async_trait;
    use bytes::Bytes;
    use futures::StreamExt;
    use futures::stream::BoxStream;
    use golem_service_base::storage::blob::memory::InMemoryBlobStorage;
    use golem_service_base::storage::blob::{
        BlobMetadata, BlobNameError, BlobRangeStream, BlobStorageBackend, BlobStorageNamespace,
        ExistsResult, ListedBlob, NormalizedBlobPath, PutIfAbsent,
    };
    use std::collections::VecDeque;
    use std::path::PathBuf;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// The answer of the next streamed read of a [`ScriptedSourceStorage`].
    #[derive(Clone, Copy, Debug)]
    pub(crate) enum SourceRead {
        /// The read fails with an error of the backend, which is transient.
        Unavailable,
        /// The read fails with a [`BlobNameError`], which is permanent.
        Permanent,
        /// The read opens the stream, and the stream fails with an error of the backend
        /// after its first chunk.
        BrokenChunk,
    }

    /// An in-memory blob storage whose streamed reads answer as the script tells, in order. A
    /// read after the end of the script passes. Each streamed read yields once before it
    /// answers, so a concurrent load can start while the read runs.
    #[derive(Debug)]
    pub(crate) struct ScriptedSourceStorage {
        inner: InMemoryBlobStorage,
        script: Mutex<VecDeque<SourceRead>>,
        reads: AtomicUsize,
    }

    impl ScriptedSourceStorage {
        pub(crate) fn new(script: impl IntoIterator<Item = SourceRead>) -> Self {
            Self {
                inner: InMemoryBlobStorage::new(),
                script: Mutex::new(script.into_iter().collect()),
                reads: AtomicUsize::new(0),
            }
        }

        /// How many streamed reads the storage got.
        pub(crate) fn reads(&self) -> usize {
            self.reads.load(Ordering::SeqCst)
        }
    }

    #[async_trait]
    impl BlobStorageBackend for ScriptedSourceStorage {
        async fn get_raw_at(
            &self,
            target_label: &'static str,
            op_label: &'static str,
            namespace: BlobStorageNamespace,
            path: &NormalizedBlobPath<'_>,
        ) -> Result<Option<Vec<u8>>, Error> {
            self.inner
                .get_raw_at(target_label, op_label, namespace, path)
                .await
        }

        async fn get_stream_at(
            &self,
            target_label: &'static str,
            op_label: &'static str,
            namespace: BlobStorageNamespace,
            path: &NormalizedBlobPath<'_>,
        ) -> Result<Option<BoxStream<'static, Result<Bytes, Error>>>, Error> {
            self.reads.fetch_add(1, Ordering::SeqCst);
            tokio::task::yield_now().await;
            let read = self.script.lock().unwrap().pop_front();
            match read {
                Some(SourceRead::Unavailable) => Err(anyhow!("injected 503")),
                Some(SourceRead::Permanent) => Err(BlobNameError::NulByte.into()),
                Some(SourceRead::BrokenChunk) => Ok(self
                    .inner
                    .get_stream_at(target_label, op_label, namespace, path)
                    .await?
                    .map(|stream| {
                        stream
                            .take(1)
                            .chain(futures::stream::iter([Err(anyhow!("injected reset"))]))
                            .boxed()
                    })),
                None => {
                    self.inner
                        .get_stream_at(target_label, op_label, namespace, path)
                        .await
                }
            }
        }

        async fn get_range_stream_at(
            &self,
            target_label: &'static str,
            op_label: &'static str,
            namespace: BlobStorageNamespace,
            path: &NormalizedBlobPath<'_>,
            offset: u64,
            length: u64,
        ) -> Result<Option<BlobRangeStream>, Error> {
            self.inner
                .get_range_stream_at(target_label, op_label, namespace, path, offset, length)
                .await
        }

        async fn get_metadata_at(
            &self,
            target_label: &'static str,
            op_label: &'static str,
            namespace: BlobStorageNamespace,
            path: &NormalizedBlobPath<'_>,
        ) -> Result<Option<BlobMetadata>, Error> {
            self.inner
                .get_metadata_at(target_label, op_label, namespace, path)
                .await
        }

        async fn put_raw_at(
            &self,
            target_label: &'static str,
            op_label: &'static str,
            namespace: BlobStorageNamespace,
            path: &NormalizedBlobPath<'_>,
            data: &[u8],
        ) -> Result<(), Error> {
            self.inner
                .put_raw_at(target_label, op_label, namespace, path, data)
                .await
        }

        async fn put_raw_if_absent_at(
            &self,
            target_label: &'static str,
            op_label: &'static str,
            namespace: BlobStorageNamespace,
            path: &NormalizedBlobPath<'_>,
            data: &[u8],
        ) -> Result<PutIfAbsent, Error> {
            self.inner
                .put_raw_if_absent_at(target_label, op_label, namespace, path, data)
                .await
        }

        async fn delete_at(
            &self,
            target_label: &'static str,
            op_label: &'static str,
            namespace: BlobStorageNamespace,
            path: &NormalizedBlobPath<'_>,
        ) -> Result<(), Error> {
            self.inner
                .delete_at(target_label, op_label, namespace, path)
                .await
        }

        async fn create_dir_at(
            &self,
            target_label: &'static str,
            op_label: &'static str,
            namespace: BlobStorageNamespace,
            path: &NormalizedBlobPath<'_>,
        ) -> Result<(), Error> {
            self.inner
                .create_dir_at(target_label, op_label, namespace, path)
                .await
        }

        async fn list_dir_at(
            &self,
            target_label: &'static str,
            op_label: &'static str,
            namespace: BlobStorageNamespace,
            path: &NormalizedBlobPath<'_>,
        ) -> Result<Vec<PathBuf>, Error> {
            self.inner
                .list_dir_at(target_label, op_label, namespace, path)
                .await
        }

        async fn list_blobs_below_at(
            &self,
            target_label: &'static str,
            op_label: &'static str,
            namespace: BlobStorageNamespace,
            path: &NormalizedBlobPath<'_>,
        ) -> Result<Box<[ListedBlob]>, Error> {
            self.inner
                .list_blobs_below_at(target_label, op_label, namespace, path)
                .await
        }

        async fn delete_dir_at(
            &self,
            target_label: &'static str,
            op_label: &'static str,
            namespace: BlobStorageNamespace,
            path: &NormalizedBlobPath<'_>,
        ) -> Result<bool, Error> {
            self.inner
                .delete_dir_at(target_label, op_label, namespace, path)
                .await
        }

        async fn exists_at(
            &self,
            target_label: &'static str,
            op_label: &'static str,
            namespace: BlobStorageNamespace,
            path: &NormalizedBlobPath<'_>,
        ) -> Result<ExistsResult, Error> {
            self.inner
                .exists_at(target_label, op_label, namespace, path)
                .await
        }

        async fn copy_between_at(
            &self,
            target_label: &'static str,
            op_label: &'static str,
            from_namespace: BlobStorageNamespace,
            from: &NormalizedBlobPath<'_>,
            to_namespace: BlobStorageNamespace,
            to: &NormalizedBlobPath<'_>,
        ) -> Result<bool, Error> {
            self.inner
                .copy_between_at(
                    target_label,
                    op_label,
                    from_namespace,
                    from,
                    to_namespace,
                    to,
                )
                .await
        }
    }
}

#[cfg(test)]
mod tests {
    use super::scripted_storage::{ScriptedSourceStorage, SourceRead};
    use super::*;
    use crate::sandbox_filesystem::SandboxFilesystemProvisioning;
    use crate::services::golem_config::FilesystemStorageMode;
    use futures::StreamExt;
    use golem_common::model::RetryConfig;
    use golem_common::model::environment::EnvironmentId;
    use golem_common::widen_infallible;
    use golem_service_base::replayable_stream::ReplayableStream as _;
    use golem_service_base::service::initial_agent_files::InitialAgentFilesService;
    use golem_service_base::storage::blob::memory::InMemoryBlobStorage;
    use test_r::test;

    test_r::enable!();

    /// Makes the cache directory of a loader on unmanaged storage with a temporary root.
    async fn cache_directory() -> HostDirectory {
        let (_, directories) = SandboxFilesystemProvisioning::provision(
            &FilesystemStorageMode::Temporary,
            RetryConfig::default(),
        )
        .await
        .unwrap();
        directories.initial_files
    }

    /// Build a `FileLoader` sharing a single in-memory blob store,
    /// and upload `content` so it can be fetched as a verified source.
    async fn setup(content: &[u8]) -> (FileLoader, AgentFileContentHash, EnvironmentId) {
        let blob = Arc::new(InMemoryBlobStorage::new());

        // One service instance for uploading, one for the loader — both share
        // the same underlying blob store.
        let upload_svc = Arc::new(InitialAgentFilesService::new(blob.clone()));
        let loader_svc = Arc::new(InitialAgentFilesService::new(blob));

        let loader = FileLoader::new(loader_svc, cache_directory().await);

        let env_id = EnvironmentId::new();
        let data: Vec<u8> = content.to_vec();
        let hash = upload_svc
            .put_if_not_exists(
                env_id,
                data.map_error(widen_infallible::<anyhow::Error>)
                    .map_item(|i| i.map_err(widen_infallible::<anyhow::Error>)),
            )
            .await
            .unwrap();

        (loader, hash, env_id)
    }

    #[test]
    async fn source_leases_keep_the_verified_cache_entry_alive() {
        let content = b"hello world";
        let (loader, hash, env_id) = setup(content).await;

        let source1 = loader
            .get_source(env_id, hash, content.len() as u64)
            .await
            .unwrap();
        let path = source1.path().as_path().to_path_buf();
        let source2 = loader
            .get_source(env_id, hash, content.len() as u64)
            .await
            .unwrap();
        assert_eq!(source2.path().as_path(), path);
        drop(source1);
        assert!(path.exists());
        drop(source2);
        assert!(!path.exists());
    }

    /// When file deletion fails on drop (e.g. the file was already removed by
    /// an external process), the executor must panic — a filesystem that cannot
    /// delete files is in an inconsistent state and the process should not
    /// continue. This is preferable to silently over-committing disk space.
    #[test]
    async fn ro_panics_when_file_deletion_fails() {
        let directory = cache_directory().await;
        let nonexistent = directory.path().child(OsStr::new("missing.wasm")).unwrap();
        let result = std::panic::catch_unwind(move || {
            let entry = InitializedCacheEntry::new_for_test(nonexistent);
            drop(entry);
        });
        assert!(
            result.is_err(),
            "dropping an InitializedCacheEntry with a nonexistent path must panic"
        );
    }

    #[test]
    async fn loader_keeps_sources_in_its_cache_directory_until_it_is_dropped() {
        let content = b"cached content";
        let (loader, hash, env_id) = setup(content).await;
        let cache = loader.cache_dir.path().as_path().to_path_buf();

        let source = loader
            .get_source(env_id, hash, content.len() as u64)
            .await
            .unwrap();

        assert_eq!(source.path().as_path().parent(), Some(cache.as_path()));
        assert_eq!(std::fs::read(source.path().as_path()).unwrap(), content);
        drop(source);
        drop(loader);
        assert!(
            !cache.exists(),
            "the cache directory must go away with the loader"
        );
    }

    #[test]
    async fn source_load_rejects_incorrect_declared_size() {
        let content = b"hello world";
        let (loader, hash, env_id) = setup(content).await;

        let result = loader
            .get_source(env_id, hash, content.len() as u64 + 1)
            .await;

        assert_eq!(result.unwrap_err().failure, InitialFileLoadFailure::Failed);
    }

    /// Makes a loader over a [`ScriptedSourceStorage`] with `script`, and uploads `content` to
    /// it.
    async fn scripted_setup(
        content: &[u8],
        script: impl IntoIterator<Item = SourceRead>,
    ) -> (
        FileLoader,
        Arc<ScriptedSourceStorage>,
        AgentFileContentHash,
        EnvironmentId,
    ) {
        let storage = Arc::new(ScriptedSourceStorage::new(script));
        let service = Arc::new(InitialAgentFilesService::new(storage.clone()));
        let env_id = EnvironmentId::new();
        let hash = service
            .put_if_not_exists(
                env_id,
                content
                    .to_vec()
                    .map_error(widen_infallible::<anyhow::Error>)
                    .map_item(|i| i.map_err(widen_infallible::<anyhow::Error>)),
            )
            .await
            .unwrap();
        let loader = FileLoader::new(service, cache_directory().await);
        (loader, storage, hash, env_id)
    }

    /// A read that the blob storage cannot answer now, and a stream that breaks, make an
    /// unavailable source. A permanent error of the storage, a missing source and a wrong size
    /// make a failed one.
    #[test]
    async fn a_load_is_unavailable_only_for_a_transient_error_of_the_blob_storage() {
        let content = b"scripted content";
        let reads = [
            Some(SourceRead::Unavailable),
            Some(SourceRead::BrokenChunk),
            Some(SourceRead::Permanent),
        ];

        let failures = futures::stream::iter(reads)
            .then(|read| async move {
                let (loader, _, hash, env_id) = scripted_setup(content, read).await;
                loader
                    .get_source(env_id, hash, content.len() as u64)
                    .await
                    .unwrap_err()
                    .failure
            })
            .collect::<Vec<_>>()
            .await;
        let (loader, _, _, env_id) = scripted_setup(content, None).await;
        let missing = loader
            .get_source(env_id, key_of(b"not uploaded"), 12)
            .await
            .unwrap_err()
            .failure;

        assert_eq!(
            failures,
            [
                InitialFileLoadFailure::Unavailable,
                InitialFileLoadFailure::Unavailable,
                InitialFileLoadFailure::Failed,
            ]
        );
        assert_eq!(missing, InitialFileLoadFailure::Failed);
    }

    /// A load that waits for a concurrent download of the same source gets the error of that
    /// download with its kind, and does not read the storage itself. A later load reads the
    /// storage again and passes.
    #[test]
    async fn a_waiter_gets_the_kind_of_the_failed_download_and_a_later_load_passes() {
        let content = b"shared content";
        let (loader, storage, hash, env_id) =
            scripted_setup(content, [SourceRead::Unavailable]).await;
        let size = content.len() as u64;

        let (first, second) = futures::join!(
            loader.get_source(env_id, hash, size),
            loader.get_source(env_id, hash, size)
        );
        let reads_after_failure = storage.reads();
        let later = loader.get_source(env_id, hash, size).await.unwrap();

        assert_eq!(
            (
                first.unwrap_err().failure,
                second.unwrap_err().failure,
                reads_after_failure
            ),
            (
                InitialFileLoadFailure::Unavailable,
                InitialFileLoadFailure::Unavailable,
                1
            )
        );
        assert_eq!(std::fs::read(later.path().as_path()).unwrap(), content);
        assert_eq!(storage.reads(), 2);
    }

    /// A load that waits for a download that stops before its end, because its load was
    /// dropped, gets an unavailable source.
    #[test]
    async fn a_waiter_on_a_dropped_download_gets_an_unavailable_source() {
        let content = b"dropped content";
        let (loader, _, hash, env_id) = scripted_setup(content, None).await;
        let size = content.len() as u64;
        let mut first = Box::pin(loader.get_source(env_id, hash, size));
        let mut second = Box::pin(loader.get_source(env_id, hash, size));

        assert!(futures::poll!(&mut first).is_pending());
        assert!(futures::poll!(&mut second).is_pending());
        drop(first);
        let failure = second.await.unwrap_err().failure;

        assert_eq!(failure, InitialFileLoadFailure::Unavailable);
    }

    fn key_of(content: &[u8]) -> AgentFileContentHash {
        AgentFileContentHash(golem_common::model::diff::Hash::new(blake3::hash(content)))
    }

    #[test]
    fn downloaded_size_adds_the_chunk_and_refuses_more_than_declared_or_an_overflow() {
        assert_eq!(downloaded_size(0, 4, 10).unwrap(), 4);
        assert_eq!(downloaded_size(4, 6, 10).unwrap(), 10);
        assert_eq!(
            downloaded_size(4, 7, 10).unwrap_err().to_string(),
            "Downloaded initial file size exceeds declared size 10"
        );
        assert_eq!(
            downloaded_size(u64::MAX, 1, u64::MAX)
                .unwrap_err()
                .to_string(),
            "Downloaded initial file size overflowed"
        );
    }

    #[test]
    fn verify_download_needs_the_hash_of_the_key_and_then_the_declared_size() {
        let key = key_of(b"content");

        verify_download(&blake3::hash(b"content"), &key, 7, 7).unwrap();
        assert_eq!(
            verify_download(&blake3::hash(b"other"), &key, 7, 7)
                .unwrap_err()
                .to_string(),
            format!("Downloaded initial file content hash does not match {key}")
        );
        assert_eq!(
            verify_download(&blake3::hash(b"content"), &key, 6, 7)
                .unwrap_err()
                .to_string(),
            "Downloaded initial file size 6 does not match declared size 7"
        );
        assert_eq!(
            verify_download(&blake3::hash(b"other"), &key, 6, 7)
                .unwrap_err()
                .to_string(),
            format!("Downloaded initial file content hash does not match {key}"),
            "the hash is checked before the size"
        );
    }
}
