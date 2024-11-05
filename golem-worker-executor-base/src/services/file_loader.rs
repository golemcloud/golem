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
use std::sync::Weak;
use std::{path::PathBuf, sync::Arc};
use tempfile::NamedTempFile;
use tokio::fs::{create_dir_all, hard_link};
use tokio::io::AsyncWriteExt;
use tracing::debug;

use crate::error::GolemError;

use golem_service_base::service::initial_component_files::InitialComponentFilesService;

/// Interface for loading files and making them available to workers.
/// This will hardlink to a temporary directory to avoid copying files between workers. Beware
/// that hardlinking is only possible within the same filesystem.
// TODO: add bookkeeping for files that are no longer needed.
pub struct FileLoader {
    initial_component_files_service: Arc<InitialComponentFilesService>,
    cache: Mutex<HashMap<InitialComponentFileKey, Weak<NamedTempFile>>>,
}

impl FileLoader {
    pub fn new(initial_component_files_service: Arc<InitialComponentFilesService>) -> Self {
        Self {
            initial_component_files_service,
            cache: Mutex::new(HashMap::new()),
        }
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
            create_dir_all(parent).await?;
        };

        let cache_entry = self.get_or_add_cache_entry(key).await?;
        let cache_entry_path = cache_entry.path();

        debug!(
            "Hardlinking {} to {}",
            cache_entry_path.display(),
            target.display()
        );
        hard_link(cache_entry_path, target).await?;

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
            create_dir_all(parent).await?;
        };

        // fast path for files that are already in cache
        {
            let cache_guard = self.cache.lock().await;
            if let Some(cache_entry) = cache_guard.get(key).and_then(|weak| weak.upgrade()) {
                let source_path = cache_entry.path();
                debug!("Copying {} to {}", source_path.display(), target.display());
                tokio::fs::copy(source_path, target).await?;
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
    ) -> Result<Arc<NamedTempFile>, anyhow::Error> {
        // get or create a new file in the cache. Also return a boolean indicating if the file was already in the cache.
        // If not we are going to download it. This is seperated into two steps to minimize the time the lock is held.
        let (cache_entry, ready) = {
            let mut cache_guard = self.cache.lock().await;

            let result =
                if let Some(cache_entry) = cache_guard.get(key).and_then(|weak| weak.upgrade()) {
                    // we have a file, we can just return it
                    (cache_entry, true)
                } else {
                    // we never had a file or it was dropped, we need to add a new one

                    let file = tempfile::NamedTempFile::new()?;
                    // store the file so we can reuse it later
                    let cache_entry = Arc::new(file);
                    // we don't want to keep a copy if we are the only ones holding it, so we use a weak reference
                    cache_guard.insert(key.clone(), Arc::downgrade(&cache_entry));
                    (cache_entry, false)
                };

            // make sure we held the lock until we are done with the dance.
            drop(cache_guard);
            result
        };

        let cache_entry_path = cache_entry.path();

        // if the file was not ready, download it
        if !ready {
            let data = self
                .initial_component_files_service
                .get(key)
                .await
                .map_err(|e| anyhow!(e))?
                .ok_or_else(|| anyhow!("File not found"))?;

            debug!("Writing {} to cache {}", key, cache_entry_path.display());

            tokio::fs::write(cache_entry_path, data).await?;
        };

        Ok(cache_entry)
    }
}

// Opaque token for read-only files. This is used to ensure that the file is not deleted while it is in use.
// Make sure to not drop this token until you are done with the file.
pub struct FileUseToken {
    _cache_entry: Arc<NamedTempFile>,
}
