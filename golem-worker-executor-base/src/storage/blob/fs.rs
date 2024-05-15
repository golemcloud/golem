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

use crate::storage::blob::{BlobMetadata, BlobStorage, BlobStorageNamespace, ExistsResult};
use async_trait::async_trait;
use bytes::Bytes;
use golem_common::model::Timestamp;
use std::path::{Path, PathBuf};
use std::time::SystemTime;
use tokio_stream::StreamExt;

pub struct FileSystemBlobStorage {
    root: PathBuf,
}

impl FileSystemBlobStorage {
    pub async fn new(root: &Path) -> Result<Self, String> {
        let canonical = async_fs::canonicalize(root)
            .await
            .map_err(|err| err.to_string())?;
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
    async fn get(
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
                .map_err(|err| err.to_string())?;
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

    async fn put(
        &self,
        _target_label: &'static str,
        _op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
        data: &[u8],
    ) -> Result<(), String> {
        let full_path = self.path_of(&namespace, path);
        self.ensure_path_is_inside_root(&full_path)?;

        async_fs::write(&full_path, data)
            .await
            .map_err(|err| err.to_string())
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
            .map_err(|err| err.to_string())
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
        let full_path = self.path_of(&namespace, path);
        self.ensure_path_is_inside_root(&full_path)?;

        let mut entries = async_fs::read_dir(&full_path)
            .await
            .map_err(|err| err.to_string())?;

        let mut result = Vec::new();
        while let Some(entry) = entries.try_next().await.map_err(|err| err.to_string())? {
            let path = entry.path();
            if path.is_file() {
                result.push(path);
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
}
