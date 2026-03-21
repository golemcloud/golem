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

use anyhow::anyhow;
use async_lock::Mutex;
use futures::TryStreamExt;
use golem_common::model::component::ComponentFileContentHash;
use golem_common::model::environment::EnvironmentId;
use golem_service_base::error::worker_executor::WorkerExecutorError;
use golem_service_base::service::initial_component_files::InitialComponentFilesService;
use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::AtomicU64;
use std::sync::{Mutex as StdMutex, Weak};
use std::{path::PathBuf, sync::Arc};
use tempfile::TempDir;
use tokio::io::AsyncWriteExt;
use tokio::sync::OwnedSemaphorePermit;
use tracing::debug;

use crate::services::active_workers::StorageSemaphore;

// Opaque token for read-only files. This is used to ensure that the file is not deleted while it is in use.
// Make sure to not drop this token until you are done with the file.
#[derive(Debug, Clone)]
pub struct FileUseToken {
    _handle: Arc<CacheEntry>,
}

/// Interface for loading files and making them available to workers.
///
/// This will hardlink to a temporary directory to avoid copying files between workers. Beware
/// that hardlinking is only possible within the same filesystem.
pub struct FileLoader {
    initial_component_files_service: Arc<InitialComponentFilesService>,
    cache_dir: TempDir,
    // Note: The cache is shared between accounts. One account no accessing data from another account
    // is implicitly done by the key being a hash of the content.
    cache: Cache,
    // When the last reference to a file is dropped, the file is deleted.
    // We need to ensure that no one else is using the file while we are deleting it.
    // To do that, give every file a unique number.
    item_counter: AtomicU64,
    /// Executor-wide storage semaphore. When set, acquiring a new cache entry
    /// (i.e. downloading the file for the first time) also acquires semaphore
    /// permits proportional to the file size. The permit is embedded in the
    /// cache entry and released automatically when the last `FileUseToken`
    /// holding that entry is dropped. Subsequent workers that hardlink to the
    /// same cache file do not acquire additional permits.
    storage_semaphore: StdMutex<Option<Arc<StorageSemaphore>>>,
}

impl FileLoader {
    pub fn new(
        initial_component_files_service: Arc<InitialComponentFilesService>,
    ) -> Result<Self, anyhow::Error> {
        let cache_dir = tempfile::Builder::new()
            .prefix("golem-initial-component-files")
            .tempdir()?;

        Ok(Self {
            initial_component_files_service,
            cache: Mutex::new(HashMap::new()),
            cache_dir,
            item_counter: AtomicU64::new(0),
            storage_semaphore: StdMutex::new(None),
        })
    }

    /// Wire up the executor-wide storage semaphore. Must be called after both
    /// `FileLoader` and `ActiveWorkers` have been constructed.
    pub fn set_storage_semaphore(&self, semaphore: Arc<StorageSemaphore>) {
        *self.storage_semaphore.lock().unwrap() = Some(semaphore);
    }

    /// Read-only files can be safely shared between workers. Download once to cache and hardlink to target.
    /// The file will only be valid until the token is dropped.
    ///
    /// `file_size` is the size of the file in bytes. It is used to acquire the
    /// executor-wide storage semaphore permit on the first download (cache miss).
    /// Cache hits (hardlinks) do not acquire additional permits.
    pub async fn get_read_only_to(
        &self,
        environment_id: EnvironmentId,
        key: ComponentFileContentHash,
        target: &PathBuf,
        file_size: u64,
    ) -> Result<FileUseToken, WorkerExecutorError> {
        self.get_read_only_to_impl(environment_id, key, target, file_size)
            .await
            .map_err(|e| {
                WorkerExecutorError::initial_file_download_failed(
                    target.display().to_string(),
                    e.to_string(),
                )
            })
    }

    /// Read-write files are copied to target.
    pub async fn get_read_write_to(
        &self,
        environment_id: EnvironmentId,
        key: ComponentFileContentHash,
        target: &PathBuf,
    ) -> Result<(), WorkerExecutorError> {
        self.get_read_write_to_impl(environment_id, key, target)
            .await
            .map_err(|e| {
                WorkerExecutorError::initial_file_download_failed(
                    target.display().to_string(),
                    e.to_string(),
                )
            })
    }

    async fn get_read_only_to_impl(
        &self,
        environment_id: EnvironmentId,
        key: ComponentFileContentHash,
        target: &PathBuf,
        file_size: u64,
    ) -> Result<FileUseToken, anyhow::Error> {
        if let Some(parent) = target.parent() {
            tokio::fs::create_dir_all(parent).await?;
        };

        let cache_entry = self
            .get_or_add_cache_entry(environment_id, key, file_size)
            .await?;

        // peek at the cache entry. It's fine to not hold the lock here.
        // as long as we keep a ref to the cache entry, the file will not be deleted
        let cache_entry_path = {
            let cache_entry_guard = cache_entry.lock().await;
            cache_entry_guard
                .as_ref()
                .map_err(|e| anyhow!(e.clone()))?
                .path
                .clone()
        };

        debug!(
            "Hardlinking {} to {}",
            cache_entry_path.display(),
            target.display()
        );
        tokio::fs::hard_link(&cache_entry_path, target).await?;

        Ok(FileUseToken {
            _handle: cache_entry,
        })
    }

    async fn get_read_write_to_impl(
        &self,
        environment_id: EnvironmentId,
        key: ComponentFileContentHash,
        target: &PathBuf,
    ) -> Result<(), anyhow::Error> {
        if let Some(parent) = target.parent() {
            tokio::fs::create_dir_all(parent).await?;
        };

        // fast path for files that are already in cache
        {
            let cache_guard = self.cache.lock().await;
            let cache_entry = cache_guard.get(&key).and_then(|weak| weak.upgrade());

            // make sure we drop the lock to not block other threads
            drop(cache_guard);

            if let Some(cache_entry) = cache_entry {
                // peek at the cache entry. It's fine to not hold the lock here.
                // as long as we keep a ref to the cache entry, the file will not be deleted
                let cache_entry_path = {
                    let cache_entry_guard = cache_entry.lock().await;
                    cache_entry_guard
                        .as_ref()
                        .map_err(|e| anyhow!(e.clone()))?
                        .path
                        .clone()
                };

                // copy the file to the target
                debug!(
                    "Copying {} to {}",
                    cache_entry_path.display(),
                    target.display()
                );
                tokio::fs::copy(&cache_entry_path, target).await?;
                return Ok(());
            }
        }

        // alternative, download the file directly to the target
        self.download_file_to_path(environment_id, target, key)
            .await?;
        Ok(())
    }

    async fn get_or_add_cache_entry(
        &self,
        environment_id: EnvironmentId,
        key: ComponentFileContentHash,
        file_size: u64,
    ) -> Result<Arc<CacheEntry>, anyhow::Error> {
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
                    cache_entry = Arc::new(Mutex::new(Err("File not downloaded yet".to_string())));

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
                let path = self.cache_dir.path().join(counter.to_string());

                // Acquire the executor-wide storage permit before touching the
                // filesystem. This is a cache miss: we are about to write
                // `file_size` new bytes to the shared cache directory. If the
                // pool is exhausted we fail immediately; the worker will be
                // retried once space is freed.
                let storage_permit = {
                    let sem_opt = self.storage_semaphore.lock().unwrap().clone();
                    if let Some(sem) = sem_opt {
                        match sem.try_acquire(file_size).await {
                            Some(permit) => Some(permit),
                            None => {
                                *prelocked_entry =
                                    Err("Executor storage pool exhausted".to_string());
                                self.cache.lock().await.remove(&key);
                                return Err(anyhow!(
                                    "Executor storage pool exhausted for initial component file"
                                ));
                            }
                        }
                    } else {
                        None
                    }
                };

                match self
                    .download_file_to_path_as_read_only(environment_id, &path, key)
                    .await
                {
                    Ok(()) => {
                        // we successfully downloaded the file and set it to read-only, set the cache entry to the file
                        *prelocked_entry = Ok(InitializedCacheEntry {
                            path: path.clone(),
                            _storage_permit: storage_permit,
                        });
                    }
                    Err(e) => {
                        // we failed to set the file to read-only, we need to fail the entry, remove it from the cache and return the error
                        // storage_permit is dropped here, returning permits to the semaphore
                        *prelocked_entry = Err(format!("Other thread failed to download: {e}"));
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
        key: ComponentFileContentHash,
    ) -> Result<(), anyhow::Error> {
        self.download_file_to_path(environment_id, path, key)
            .await?;
        self.set_path_read_only(path).await?;
        Ok(())
    }

    async fn download_file_to_path(
        &self,
        environment_id: EnvironmentId,
        path: &Path,
        key: ComponentFileContentHash,
    ) -> Result<(), anyhow::Error> {
        debug!("Downloading {} to {}", key, path.display());

        let mut data = self
            .initial_component_files_service
            .get(environment_id, key)
            .await
            .map_err(|e| anyhow!(e))?
            .ok_or_else(|| anyhow!("File not found"))?;

        let file = tokio::fs::File::create(path).await?;
        let mut writer = tokio::io::BufWriter::new(file);

        while let Some(chunk) = data.try_next().await.map_err(|e| anyhow!(e))? {
            writer.write_all(&chunk).await?;
        }

        writer.flush().await?;
        Ok(())
    }

    async fn set_path_read_only(&self, path: &Path) -> Result<(), anyhow::Error> {
        let mut perms = tokio::fs::metadata(path).await?.permissions();
        perms.set_readonly(true);
        tokio::fs::set_permissions(path, perms).await?;
        Ok(())
    }
}

// Scary type, let's break it down:
// Outer Mutex: This is the lock that protects the cache from concurrent access.
// HashMap: The cache itself, mapping keys to weak references to the cache entries.
// InitialComponentFileKey: The key used to identify the cache entry.
// Weak: A weak reference to the cache entry. This is used to avoid keeping the cache entry alive if no one else is using it.
// Mutex: The cache entry itself. This is used to ensure that no one is accessing the file while it is being downloaded.
// Result: The result of the cache entry. This is used to store the file path and any errors that occurred while downloading the file.
// InitializedCacheEntry: The cache entry itself. This is used to store the file path and ensure that the file is deleted when the cache entry is dropped.
type Cache = Mutex<HashMap<ComponentFileContentHash, Weak<CacheEntry>>>;

type CacheEntry = Mutex<Result<InitializedCacheEntry, String>>;

#[derive(Debug)]
struct InitializedCacheEntry {
    path: PathBuf,
    /// Storage semaphore permit held for the lifetime of this cache entry.
    /// Acquired on cache miss (first download); `None` when no semaphore is
    /// configured. Dropped automatically when the last `FileUseToken` holding
    /// this entry is released, returning the permits to the executor pool.
    _storage_permit: Option<OwnedSemaphorePermit>,
}

impl Drop for InitializedCacheEntry {
    fn drop(&mut self) {
        debug!("Removing file {}", self.path.display());
        if let Err(e) = std::fs::remove_file(&self.path) {
            tracing::error!("Failed to remove file {}: {}", self.path.display(), e);
        }
        // _storage_permit is dropped here, returning permits to the semaphore.
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::active_workers::StorageSemaphore;
    use golem_common::model::environment::EnvironmentId;
    use golem_common::widen_infallible;
    use golem_service_base::replayable_stream::ReplayableStream as _;
    use golem_service_base::service::initial_component_files::InitialComponentFilesService;
    use golem_service_base::storage::blob::memory::InMemoryBlobStorage;
    use std::time::Duration;
    use test_r::test;

    test_r::enable!();

    /// Build a `FileLoader` + semaphore sharing a single in-memory blob store,
    /// and upload `content` so it can be fetched via `get_read_only_to`.
    ///
    /// Returns `(loader, semaphore, content_hash, env_id)`.
    async fn setup(
        pool_bytes: usize,
        content: &[u8],
    ) -> (
        FileLoader,
        Arc<StorageSemaphore>,
        ComponentFileContentHash,
        EnvironmentId,
    ) {
        let blob = Arc::new(InMemoryBlobStorage::new());

        // One service instance for uploading, one for the loader — both share
        // the same underlying blob store.
        let upload_svc = Arc::new(InitialComponentFilesService::new(blob.clone()));
        let loader_svc = Arc::new(InitialComponentFilesService::new(blob));

        let loader = FileLoader::new(loader_svc).unwrap();
        let semaphore = Arc::new(StorageSemaphore::new(pool_bytes, Duration::from_millis(1)));
        loader.set_storage_semaphore(semaphore.clone());

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

        (loader, semaphore, hash, env_id)
    }

    /// A single `get_read_only_to` for a fresh file acquires permits equal to
    /// the file size (rounded up to KB).
    #[test]
    async fn ro_first_load_acquires_semaphore_permits() {
        let content = b"hello world"; // 11 bytes → rounds up to 1 KB = 1 permit
        let pool_bytes = 4 * 1024;
        let (loader, semaphore, hash, env_id) = setup(pool_bytes, content).await;

        let dir = tempfile::tempdir().unwrap();
        let _token = loader
            .get_read_only_to(env_id, hash, &dir.path().join("f.txt"), content.len() as u64)
            .await
            .unwrap();

        assert_eq!(semaphore.available_bytes(), 3 * 1024);
    }

    /// A second `get_read_only_to` for the **same content hash** must NOT
    /// consume additional semaphore permits — the file is already in the local
    /// filesystem cache and only a hardlink is created, adding zero disk blocks.
    #[test]
    async fn ro_second_load_of_same_file_does_not_consume_additional_permits() {
        let content = b"hello world";
        let pool_bytes = 4 * 1024;
        let (loader, semaphore, hash, env_id) = setup(pool_bytes, content).await;

        let dir = tempfile::tempdir().unwrap();
        let _t1 = loader
            .get_read_only_to(env_id, hash, &dir.path().join("f1.txt"), content.len() as u64)
            .await
            .unwrap();

        let permits_after_first = semaphore.available_bytes();

        let _t2 = loader
            .get_read_only_to(env_id, hash, &dir.path().join("f2.txt"), content.len() as u64)
            .await
            .unwrap();

        assert_eq!(
            semaphore.available_bytes(),
            permits_after_first,
            "second load of cached RO file must not consume extra semaphore permits"
        );
    }

    /// When all `FileUseToken`s for a cached entry are dropped, the semaphore
    /// permits are returned to the pool.
    #[test]
    async fn ro_permits_released_when_all_tokens_dropped() {
        let content = b"hello world";
        let pool_bytes = 4 * 1024;
        let (loader, semaphore, hash, env_id) = setup(pool_bytes, content).await;

        let dir = tempfile::tempdir().unwrap();
        let t1 = loader
            .get_read_only_to(env_id, hash, &dir.path().join("f1.txt"), content.len() as u64)
            .await
            .unwrap();
        let t2 = loader
            .get_read_only_to(env_id, hash, &dir.path().join("f2.txt"), content.len() as u64)
            .await
            .unwrap();

        let after_load = semaphore.available_bytes();
        drop(t1);
        assert_eq!(semaphore.available_bytes(), after_load, "t2 still alive");
        drop(t2);
        assert_eq!(
            semaphore.available_bytes(),
            pool_bytes as u64,
            "all tokens dropped — full pool must be restored"
        );
    }
}
