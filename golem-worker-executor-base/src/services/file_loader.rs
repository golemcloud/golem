// Copyright 2024 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use anyhow::anyhow;
use async_lock::Mutex;
use golem_common::model::{AccountId, InitialComponentFileKey};
use std::collections::HashMap;
use std::path::Path;
use std::sync::Weak;
use std::{path::PathBuf, sync::Arc};
use tempfile::TempDir;
use tracing::debug;

use crate::error::GolemError;

use golem_service_base::service::initial_component_files::InitialComponentFilesService;

// Opaque token for read-only files. This is used to ensure that the file is not deleted while it is in use.
// Make sure to not drop this token until you are done with the file.
pub struct FileUseToken {
    _handle: Arc<CacheEntry>,
}

/// Interface for loading files and making them available to workers.
/// This will hardlink to a temporary directory to avoid copying files between workers. Beware
/// that hardlinking is only possible within the same filesystem.
pub struct FileLoader {
    initial_component_files_service: Arc<InitialComponentFilesService>,
    cache_dir: TempDir,
    // Note: The cache is shared between accounts. One account no accessing data from another account
    // is implicitly done by the key being a hash of the content.
    cache: Cache,
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

        let (token, cache_entry_path) = self.get_or_add_cache_entry(account_id, key).await?;

        debug!(
            "Hardlinking {} to {}",
            cache_entry_path.display(),
            target.display()
        );
        tokio::fs::hard_link(&cache_entry_path, target).await?;

        Ok(token)
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
            if let Some(cache_entry) = cache_guard.get(key).and_then(|weak| weak.upgrade()) {
                // get access to inner data;
                let cache_entry_guard = cache_entry.lock().await;

                match cache_entry_guard.as_ref() {
                    Ok(path_handle) => {
                        debug!(
                            "Copying {} to {}",
                            path_handle.path.display(),
                            target.display()
                        );
                        tokio::fs::copy(&path_handle.path, target).await?;
                        drop(cache_entry_guard);
                        drop(cache_entry);
                        return Ok(());
                    }
                    Err(e) => {
                        return Err(anyhow!("Failed to download file: {}", e));
                    }
                };
            }
            drop(cache_guard);
        }

        // alternative, download the file directly to the target
        self.download_file_to_path(account_id, target, key).await?;
        Ok(())
    }

    async fn get_or_add_cache_entry(
        &self,
        account_id: &AccountId,
        key: &InitialComponentFileKey,
    ) -> Result<(FileUseToken, PathBuf), anyhow::Error> {
        let path = self.cache_dir.path().join(key.to_string());
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
                    cache_entry = Arc::new(Mutex::new(Err(anyhow!("File not downloaded yet"))));

                    // immediately lock the entry so no one accesses the file while we are downloading it
                    maybe_prelocked_entry = Some(cache_entry.lock().await);

                    // we don't want to keep a copy if we are the only ones holding it, so we use a weak reference
                    cache_guard.insert(key.clone(), Arc::downgrade(&cache_entry));
                };
                drop(cache_guard);
            };

            // we may need to initialize the file in case we are the first ones to access it
            if let Some(mut prelocked_entry) = maybe_prelocked_entry {
                debug!("Adding {} to cache", key);
                match self.download_file_to_path(account_id, &path, key).await {
                    Ok(()) => {
                        let mut perms = tokio::fs::metadata(&path).await?.permissions();
                        perms.set_readonly(true);
                        tokio::fs::set_permissions(&path, perms).await?;

                        // we successfully downloaded the file, set the cache entry to the file
                        *prelocked_entry = Ok(PathHandle { path: path.clone() });
                    }
                    Err(e) => {
                        // we failed to download the file, we need to fail the cache entry, remove it from the cache and return the error
                        *prelocked_entry = Err(anyhow!("Other thread failed to download: {}", e));

                        self.cache.lock().await.remove(key);

                        return Err(e);
                    }
                }
                drop(prelocked_entry);
            };
        };
        Ok((
            FileUseToken {
                _handle: cache_entry,
            },
            path,
        ))
    }

    async fn download_file_to_path(
        &self,
        account_id: &AccountId,
        path: &Path,
        key: &InitialComponentFileKey,
    ) -> Result<(), anyhow::Error> {
        debug!("Downloading {} to {}", key, path.display());

        let data = self
            .initial_component_files_service
            .get(account_id, key)
            .await
            .map_err(|e| anyhow!(e))?
            .ok_or_else(|| anyhow!("File not found"))?;

        tokio::fs::write(&path, data).await?;
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
// CacheEntry: The cache entry itself. This is used to store the file path and ensure that the file is deleted when the cache entry is dropped.
type Cache = Mutex<HashMap<InitialComponentFileKey, Weak<CacheEntry>>>;

type CacheEntry = Mutex<Result<PathHandle, anyhow::Error>>;

struct PathHandle {
    path: PathBuf,
}

impl Drop for PathHandle {
    fn drop(&mut self) {
        if let Err(e) = std::fs::remove_file(&self.path) {
            tracing::error!("Failed to remove file {}: {}", self.path.display(), e);
        }
    }
}
