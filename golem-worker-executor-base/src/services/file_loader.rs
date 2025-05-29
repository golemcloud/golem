// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
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

use crate::error::GolemError;
use anyhow::anyhow;
use async_lock::Mutex;
use futures::TryStreamExt;
use golem_common::model::{AccountId, InitialComponentFileKey};
use golem_service_base::service::initial_component_files::InitialComponentFilesService;
use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::AtomicU64;
use std::sync::Weak;
use std::{path::PathBuf, sync::Arc};
use tempfile::TempDir;
use tokio::io::AsyncWriteExt;
use tracing::debug;

// Opaque token for read-only files. This is used to ensure that the file is not deleted while it is in use.
// Make sure to not drop this token until you are done with the file.
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
        })
    }

    /// Read-only files can be safely shared between workers. Download once to cache and hardlink to target.
    /// The file will only be valid until the token is dropped.
    pub async fn get_read_only_to(
        &self,
        account_id: &AccountId,
        key: &InitialComponentFileKey,
        target: &PathBuf,
    ) -> Result<FileUseToken, GolemError> {
        self.get_read_only_to_impl(account_id, key, target)
            .await
            .map_err(|e| {
                GolemError::initial_file_download_failed(
                    target.display().to_string(),
                    e.to_string(),
                )
            })
    }

    /// Read-write files are copied to target.
    pub async fn get_read_write_to(
        &self,
        account_id: &AccountId,
        key: &InitialComponentFileKey,
        target: &PathBuf,
    ) -> Result<(), GolemError> {
        self.get_read_write_to_impl(account_id, key, target)
            .await
            .map_err(|e| {
                GolemError::initial_file_download_failed(
                    target.display().to_string(),
                    e.to_string(),
                )
            })
    }

    async fn get_read_only_to_impl(
        &self,
        account_id: &AccountId,
        key: &InitialComponentFileKey,
        target: &PathBuf,
    ) -> Result<FileUseToken, anyhow::Error> {
        if let Some(parent) = target.parent() {
            tokio::fs::create_dir_all(parent).await?;
        };

        let cache_entry = self.get_or_add_cache_entry(account_id, key).await?;

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
        account_id: &AccountId,
        key: &InitialComponentFileKey,
        target: &PathBuf,
    ) -> Result<(), anyhow::Error> {
        if let Some(parent) = target.parent() {
            tokio::fs::create_dir_all(parent).await?;
        };

        // fast path for files that are already in cache
        {
            let cache_guard = self.cache.lock().await;
            let cache_entry = cache_guard.get(key).and_then(|weak| weak.upgrade());

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
            }
        }

        // alternative, download the file directly to the target
        self.download_file_to_path(account_id, target, key).await?;
        Ok(())
    }

    async fn get_or_add_cache_entry(
        &self,
        account_id: &AccountId,
        key: &InitialComponentFileKey,
    ) -> Result<Arc<CacheEntry>, anyhow::Error> {
        let cache_entry;
        {
            let maybe_prelocked_entry;
            {
                let mut cache_guard = self.cache.lock().await;
                if let Some(existing_cache_entry) =
                    cache_guard.get(key).and_then(|weak| weak.upgrade())
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
                    cache_guard.insert(key.clone(), Arc::downgrade(&cache_entry));
                };
                drop(cache_guard);
            };

            // we may need to initialize the entry in case we are the first ones to access it
            if let Some(mut prelocked_entry) = maybe_prelocked_entry {
                debug!("Adding {} to cache", key);

                let counter = self
                    .item_counter
                    .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                let path = self.cache_dir.path().join(counter.to_string());

                match self
                    .download_file_to_path_as_read_only(account_id, &path, key)
                    .await
                {
                    Ok(()) => {
                        // we successfully downloaded the file and set it to read-only, set the cache entry to the file
                        *prelocked_entry = Ok(InitializedCacheEntry { path: path.clone() });
                    }
                    Err(e) => {
                        // we failed to set the file to read-only, we need to fail the entry, remove it from the cache and return the error
                        *prelocked_entry = Err(format!("Other thread failed to download: {}", e));
                        self.cache.lock().await.remove(key);

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
        account_id: &AccountId,
        path: &Path,
        key: &InitialComponentFileKey,
    ) -> Result<(), anyhow::Error> {
        self.download_file_to_path(account_id, path, key).await?;
        self.set_path_read_only(path).await?;
        Ok(())
    }

    async fn download_file_to_path(
        &self,
        account_id: &AccountId,
        path: &Path,
        key: &InitialComponentFileKey,
    ) -> Result<(), anyhow::Error> {
        debug!("Downloading {} to {}", key, path.display());

        let mut data = self
            .initial_component_files_service
            .get(account_id, key)
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
type Cache = Mutex<HashMap<InitialComponentFileKey, Weak<CacheEntry>>>;

type CacheEntry = Mutex<Result<InitializedCacheEntry, String>>;

struct InitializedCacheEntry {
    path: PathBuf,
}

impl Drop for InitializedCacheEntry {
    fn drop(&mut self) {
        tracing::debug!("Removing file {}", self.path.display());
        if let Err(e) = std::fs::remove_file(&self.path) {
            tracing::error!("Failed to remove file {}: {}", self.path.display(), e);
        }
    }
}
