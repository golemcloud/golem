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

use std::{path::PathBuf, sync::Arc};
use anyhow::anyhow;
use golem_common::model::InitialComponentFileKey;
use tracing::debug;
use tokio::fs::File;
use tokio::io::AsyncWriteExt;
use tokio::fs::{hard_link, copy, create_dir_all};
use tempfile::TempDir;

use crate::error::GolemError;

use super::initial_component_files::InitialComponentFilesService;

/// Interface for loading files and making them available to workers.
/// This will hardlink to a temporary directory to avoid copying files between workers. Beware
/// that hardlinking is only possible within the same filesystem.
// TODO: add bookkeeping for files that are no longer needed.
pub struct FileLoader {
    cache_dir: TempDir,
    initial_component_files_service: Arc<dyn InitialComponentFilesService + Send + Sync>,
}

impl FileLoader {
    pub fn new(
        initial_component_files_service: Arc<dyn InitialComponentFilesService + Send + Sync>,
    ) -> anyhow::Result<Self> {
        let cache_dir = tempfile::Builder::new().prefix("golem-file-cache").tempdir()?;

        Ok(Self {
            cache_dir,
            initial_component_files_service,
        })
    }

    /// Read-only files can be safely shared between workers. Download once to cache and hardlink to target.
    pub async fn get_read_only_to(&self, key: &InitialComponentFileKey, target: &PathBuf) -> Result<(), GolemError> {
       self
        .get_read_only_to_impl(key, target)
        .await
        .map_err(|e| GolemError::initial_file_download_failed(target.display().to_string(), e.to_string()))
    }

    /// Read-write files are copied to target.
    pub async fn get_read_write_to(&self, key: &InitialComponentFileKey, target: &PathBuf) -> Result<(), GolemError> {
        self
            .get_read_write_to_impl(key, target)
            .await
            .map_err(|e| GolemError::initial_file_download_failed(target.display().to_string(), e.to_string()))
    }

    async fn get_read_only_to_impl(&self, key: &InitialComponentFileKey, target: &PathBuf) -> Result<(), anyhow::Error> {
        if let Some(parent) = target.parent() {
            create_dir_all(parent).await?;
        };

        let path = self.cache_dir.path().join(&key.0);
        if !path.exists() {
            let data = self.initial_component_files_service
                .get(key)
                .await
                .map_err(|e| anyhow!(e))?
                .ok_or_else(|| anyhow!("File not found"))?;

            let mut file = File::create(&path).await?;
            file.write_all(&data).await?;
        }
        debug!("Hardlinking {} to {}", path.display(), target.display());
        hard_link(path, target).await?;

        Ok(())
    }

    pub async fn get_read_write_to_impl(&self, key: &InitialComponentFileKey, target: &PathBuf) -> Result<(), anyhow::Error> {
        if let Some(parent) = target.parent() {
            create_dir_all(parent).await?;
        };

        let cache_path = self.cache_dir.path().join(&key.0);
        // we already have the file in cache so we can just copy it
        if cache_path.exists() {
            copy(cache_path, target).await?;
            Ok(())
        } else {
            let data = self.initial_component_files_service
                .get(key)
                .await
                .map_err(|e| anyhow!(e))?
                .ok_or_else(|| anyhow!("File not found"))?;

            let mut file = File::create(&target).await?;
            file.write_all(&data).await?;
            Ok(())
        }
    }
}
