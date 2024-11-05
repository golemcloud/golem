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
use golem_common::model::InitialComponentFileKey;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Weak;
use std::{path::PathBuf, sync::Arc};
use tokio::io::AsyncWriteExt;
use tracing::debug;

use crate::error::GolemError;

use golem_service_base::service::initial_component_files::InitialComponentFilesService;

/// Interface for loading files and making them available to workers.
/// This will hardlink to a temporary directory to avoid copying files between workers. Beware
/// that hardlinking is only possible within the same filesystem.
pub struct FileLoader {
    initial_component_files_service: Arc<InitialComponentFilesService>,
    cache_dir: PathBuf,
    cache: Mutex<HashMap<InitialComponentFileKey, Weak<CacheEntry>>>,
}

impl FileLoader {
    pub async fn new(
        initial_component_files_service: Arc<InitialComponentFilesService>,
        cache_dir: &Path,
    ) -> Result<Self, anyhow::Error> {
        if !cache_dir.exists() {
            tokio::fs::create_dir_all(cache_dir).await?
        };

        Ok(Self {
            initial_component_files_service,
            cache: Mutex::new(HashMap::new()),
            cache_dir: cache_dir.to_owned(),
        })
    }

    /// Read-only files can be safely shared between workers. Download once to cache and hardlink to target.
    /// The file will only be valid until the token is dropped.
    pub async fn get_read_only_to(
        &self,
        key: &InitialComponentFileKey,
        target: &PathBuf,
    ) -> Result<FileUseToken, GolemError> {
        self.get_read_only_to_impl(key, target).await.map_err(|e| {
            GolemError::initial_file_download_failed(target.display().to_string(), e.to_string())
        })
    }

    /// Read-write files are copied to target.
    pub async fn get_read_write_to(
        &self,
        key: &InitialComponentFileKey,
        target: &PathBuf,
    ) -> Result<(), GolemError> {
        self.get_read_write_to_impl(key, target).await.map_err(|e| {
            GolemError::initial_file_download_failed(target.display().to_string(), e.to_string())
        })
    }

    async fn get_read_only_to_impl(
        &self,
        key: &InitialComponentFileKey,
        target: &PathBuf,
    ) -> Result<FileUseToken, anyhow::Error> {
        if let Some(parent) = target.parent() {
            tokio::fs::create_dir_all(parent).await?;
        };

        let cache_entry = self.get_or_add_cache_entry(key).await?;

        debug!(
            "Hardlinking {} to {}",
            cache_entry.path.display(),
            target.display()
        );
        tokio::fs::hard_link(&cache_entry.path, target).await?;

        let mut perms = tokio::fs::metadata(target).await?.permissions();
        perms.set_readonly(true);
        tokio::fs::set_permissions(target, perms).await?;

        Ok(FileUseToken {
            _cache_entry: cache_entry,
        })
    }

    async fn get_read_write_to_impl(
        &self,
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
                debug!(
                    "Copying {} to {}",
                    cache_entry.path.display(),
                    target.display()
                );
                tokio::fs::copy(&cache_entry.path, target).await?;
                return Ok(());
            }
            drop(cache_guard);
        }

        // alternative, download the file directly to the target
        {
            let data = self
                .initial_component_files_service
                .get(key)
                .await
                .map_err(|e| anyhow!(e))?
                .ok_or_else(|| anyhow!("File not found"))?;

            let mut file = tokio::fs::File::create(&target).await?;
            file.write_all(&data).await?;
            Ok(())
        }
    }

    async fn get_or_add_cache_entry(
        &self,
        key: &InitialComponentFileKey,
    ) -> Result<Arc<CacheEntry>, anyhow::Error> {
        let mut cache_guard = self.cache.lock().await;
        let result = if let Some(cache_entry) = cache_guard.get(key).and_then(|weak| weak.upgrade())
        {
            // we have a file, we can just return it
            cache_entry
        } else {
            // we never had a file or it was dropped, we need to download it.
            let path = self.cache_dir.join(key.to_string());

            // Downloading the user data inside the lock is not great.
            // Refactor this to load the file outside the lock and then make it available to the worker.
            let data = self
                .initial_component_files_service
                .get(key)
                .await
                .map_err(|e| anyhow!(e))?
                .ok_or_else(|| anyhow!("File not found"))?;

            debug!("Writing {} to cache {}", key, path.display());

            tokio::fs::write(&path, data).await?;

            // store the file so we can reuse it later
            let cache_entry = Arc::new(CacheEntry { path });
            // we don't want to keep a copy if we are the only ones holding it, so we use a weak reference
            cache_guard.insert(key.clone(), Arc::downgrade(&cache_entry));
            cache_entry
        };

        // make sure we held the lock until we are done with the dance.
        drop(cache_guard);
        Ok(result)
    }
}

// Opaque token for read-only files. This is used to ensure that the file is not deleted while it is in use.
// Make sure to not drop this token until you are done with the file.
pub struct FileUseToken {
    _cache_entry: Arc<CacheEntry>,
}

struct CacheEntry {
    path: PathBuf,
}

impl Drop for CacheEntry {
    fn drop(&mut self) {
        if let Err(e) = std::fs::remove_file(&self.path) {
            tracing::error!("Failed to remove file {}: {}", self.path.display(), e);
        }
    }
}
