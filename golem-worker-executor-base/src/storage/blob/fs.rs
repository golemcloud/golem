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

use tokio::fs::File;
use crate::storage::blob::{BlobMetadata, BlobStorage, BlobStorageNamespace, ExistsResult};
use async_trait::async_trait;
use bytes::Bytes;
use golem_common::model::{ComponentId, Timestamp, WorkerId};
use std::path::{Path, PathBuf};
use std::time::SystemTime;
use anyhow::Error;
use tokio::io::AsyncReadExt;
use tokio_stream::StreamExt;
use tracing::info;

#[derive(Debug)]
pub struct FileSystemBlobStorage {
    root: PathBuf,
}

impl FileSystemBlobStorage {
    pub async fn new(root: &Path) -> Result<Self, String> {
        if async_fs::metadata(root).await.is_err() {
            async_fs::create_dir_all(root)
                .await
                .map_err(|err| format!("Failed to create local blob storage: {err}"))?
        }
        let canonical = async_fs::canonicalize(root)
            .await
            .map_err(|err| err.to_string())?;

        let compilation_cache = canonical.join("compilation_cache");

        if async_fs::metadata(&compilation_cache).await.is_err() {
            async_fs::create_dir_all(&compilation_cache)
                .await
                .map_err(|err| format!("Failed to create compilation_cache directory: {err}"))?;
        }

        let custom_data = canonical.join("custom_data");

        if async_fs::metadata(&custom_data).await.is_err() {
            async_fs::create_dir_all(&custom_data)
                .await
                .map_err(|err| format!("Failed to create custom_data directory: {err}"))?;
        }

        Ok(Self { root: canonical })
    }

    fn path_of(&self, namespace: &BlobStorageNamespace, path: &Path) -> PathBuf {
        let mut result = self.root.clone();

        match namespace {
            BlobStorageNamespace::CompilationCache => result.push("compilation_cache"),
            BlobStorageNamespace::CustomStorage(account_id) => {
                result.push("custom_data");
                result.push(account_id.to_string());
            }
            BlobStorageNamespace::OplogPayload {
                account_id,
                worker_id,
            } => {
                result.push("oplog_payload");
                result.push(account_id.to_string());
                result.push(worker_id.to_string());
            }
            BlobStorageNamespace::CompressedOplog {
                account_id,
                component_id,
                level,
            } => {
                info!("^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^");
                result.push("compressed_oplog");
                result.push(account_id.to_string());
                result.push(component_id.to_string());
                result.push(level.to_string());
            }
            BlobStorageNamespace::InitialFileSystem(account_id) => {
                result.push("initial_file_system");
                result.push(account_id.to_string());
            }
        }

        result.push(path);
        result
    }

    fn ensure_path_is_inside_root(&self, path: &Path) -> Result<(), String> {
        if !path.starts_with(&self.root) {
            Err(format!("Path {path:?} is not within: {:?}", self.root))
        } else {
            Ok(())
        }
    }
}

#[async_trait]
impl BlobStorage for FileSystemBlobStorage {
    async fn get_raw(
        &self,
        _target_label: &'static str,
        _op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> Result<Option<Bytes>, String> {
        let full_path = self.path_of(&namespace, path);
        self.ensure_path_is_inside_root(&full_path)?;

        if async_fs::metadata(&full_path).await.is_ok() {
            let data = async_fs::read(&full_path)
                .await
                .map_err(|err| format!("Failed to read file from {full_path:?}: {err}"))?;
            Ok(Some(Bytes::from(data)))
        } else {
            Ok(None)
        }
    }

    async fn get_metadata(
        &self,
        _target_label: &'static str,
        _op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> Result<Option<BlobMetadata>, String> {
        let full_path = self.path_of(&namespace, path);
        self.ensure_path_is_inside_root(&full_path)?;

        if let Ok(metadata) = async_fs::metadata(&full_path).await {
            let last_modified_at = metadata
                .modified()
                .map_err(|err| err.to_string())?
                .duration_since(SystemTime::UNIX_EPOCH)
                .map_err(|err| err.to_string())?
                .as_millis() as u64;
            Ok(Some(BlobMetadata {
                last_modified_at: Timestamp::from(last_modified_at),
                size: metadata.len(),
            }))
        } else {
            Ok(None)
        }
    }

    async fn put_raw(
        &self,
        _target_label: &'static str,
        _op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
        data: &[u8],
    ) -> Result<(), String> {
        let full_path = self.path_of(&namespace, path);
        self.ensure_path_is_inside_root(&full_path)?;
        info!("In the file system ---------------------------------------------------");

        info!("The path is {}", path.display());
        info!("The full path is {},", full_path.display());

        info!("In the file system ---------------------------------------------------");

        if let Some(parent) = full_path.parent() {
            if async_fs::metadata(parent).await.is_err() {
                async_fs::create_dir_all(parent).await.map_err(|err| {
                    format!("Failed to create parent directory {parent:?}: {err}")
                })?;
            }
        }

        async_fs::write(&full_path, data)
            .await
            .map_err(|err| format!("Failed to store file at {full_path:?}: {err}"))
    }

    async fn delete(
        &self,
        _target_label: &'static str,
        _op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> Result<(), String> {
        let full_path = self.path_of(&namespace, path);
        self.ensure_path_is_inside_root(&full_path)?;

        async_fs::remove_file(&full_path)
            .await
            .map_err(|err| format!("Failed to delete file at {full_path:?}: {err}"))
    }

    async fn get_file(&self, path: &Path) -> Result<Vec<u8>, String> {
        let mut file = File::open(path)
            .await
            .map_err(|err| format!("Failed to open file at {path:?}: {err}"))?;

        let mut buffer = Vec::new();

        file.read_to_end(&mut buffer)
            .await
            .map_err(|err| format!("Failed to read file at {path:?}: {err}"))?;

        Ok(buffer)

    }

    async fn create_dir(
        &self,
        _target_label: &'static str,
        _op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> Result<(), String> {
        let full_path = self.path_of(&namespace, path);
        self.ensure_path_is_inside_root(&full_path)?;
        info!("Creating the dir -----------------------------------------((9099009090909");
        info!("{}",full_path.display());

        async_fs::create_dir_all(&full_path)
            .await
            .map_err(|err| err.to_string())
    }

    async fn list_dir(
        &self,
        _target_label: &'static str,
        _op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> Result<Vec<PathBuf>, String> {
        let namespace_root = self.path_of(&namespace, Path::new(""));
        let full_path = self.path_of(&namespace, path);
        self.ensure_path_is_inside_root(&full_path)?;

        let mut entries = async_fs::read_dir(&full_path)
            .await
            .map_err(|err| err.to_string())?;

        let mut result = Vec::new();
        while let Some(entry) = entries.try_next().await.map_err(|err| err.to_string())? {
            if let Ok(path) = entry.path().strip_prefix(&namespace_root) {
                result.push(path.to_path_buf());
            }
        }
        Ok(result)
    }

    async fn delete_dir(
        &self,
        _target_label: &'static str,
        _op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> Result<(), String> {
        let full_path = self.path_of(&namespace, path);
        self.ensure_path_is_inside_root(&full_path)?;

        async_fs::remove_dir_all(&full_path)
            .await
            .map_err(|err| err.to_string())
    }

    async fn exists(
        &self,
        _target_label: &'static str,
        _op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> Result<ExistsResult, String> {
        let full_path = self.path_of(&namespace, path);
        self.ensure_path_is_inside_root(&full_path)?;

        if let Ok(metadata) = async_fs::metadata(&full_path).await {
            if metadata.is_file() {
                Ok(ExistsResult::File)
            } else {
                Ok(ExistsResult::Directory)
            }
        } else {
            Ok(ExistsResult::DoesNotExist)
        }
    }

    async fn copy(
        &self,
        _target_label: &'static str,
        _op_label: &'static str,
        namespace: BlobStorageNamespace,
        from: &Path,
        to: &Path,
    ) -> Result<(), String> {
        let from_full_path = self.path_of(&namespace, from);
        let to_full_path = self.path_of(&namespace, to);
        self.ensure_path_is_inside_root(&from_full_path)?;
        self.ensure_path_is_inside_root(&to_full_path)?;

        async_fs::copy(&from_full_path, &to_full_path)
            .await
            .map_err(|err| err.to_string())?;
        Ok(())
    }

    async fn initialize_ifs(&self, worker_id: WorkerId) -> Result<(), String> {
        // Define the source and target directories
        let source_dir = format!(
            "/worker_executor_store/initial_file_system/{}/{}/extracted",
            worker_id.component_id, worker_id.component_id
        );

        let target_dir = format!(
            "/worker_executor_store/compressed_oplog/-1/{}/{}/{}",
            worker_id.component_id, worker_id.component_id, worker_id.worker_name
        );

        // Check if the source directory exists
        if !Path::new(&source_dir).exists() {
            return Err(format!("Source directory does not exist: {}", source_dir));
        }

        // Create the target directory if it doesn't exist
        if !Path::new(&target_dir).exists() {
            async_fs::create_dir_all(&target_dir)
                .await
                .map_err(|e| format!("Failed to create target directory: {}", e))?;
        }

        // Copy contents from the source to the target directory
        self.copy_dir_contents(
            "initialize_ifs",
            "copy_dir_contents",
            &PathBuf::from(source_dir),
            &PathBuf::from(target_dir),
        )
            .await
    }

    async fn copy_dir_contents(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        source: &Path,
        target: &Path,
    ) -> Result<(), String> {
        // Read entries in the source directory
        let mut entries = async_fs::read_dir(source)
            .await
            .map_err(|e| format!("Failed to read source directory: {}", e))?;

        while let Some(entry) = entries
            .try_next()
            .await
            .map_err(|e| format!("Failed to read directory entry: {}", e))?
        {
            let entry_path = entry.path();
            let target_path = target.join(entry.file_name());

            if entry_path.is_dir() {
                // Recursively copy subdirectories
                async_fs::create_dir_all(&target_path)
                    .await
                    .map_err(|e| format!("Failed to create directory: {}", e))?;
                self.copy_dir_contents(target_label, op_label, &entry_path, &target_path)
                    .await?;
            } else {
                // Manual file copy operation
                async_fs::copy(&entry_path, &target_path)
                    .await
                    .map_err(|e| format!("Failed to copy file {:?} to {:?}: {}", entry_path, target_path, e))?;
            }
        }

        Ok(())
    }

}
