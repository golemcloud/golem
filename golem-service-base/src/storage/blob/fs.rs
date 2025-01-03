// Copyright 2024-2025 Golem Cloud
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
use futures::TryStreamExt;
use golem_common::model::Timestamp;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::time::SystemTime;
use tokio::io::AsyncWriteExt;
use tokio_stream::StreamExt;

use super::ReplayableStream;

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
                result.push("compressed_oplog");
                result.push(account_id.to_string());
                result.push(component_id.to_string());
                result.push(level.to_string());
            }
            BlobStorageNamespace::InitialComponentFiles { account_id } => {
                result.push("initial_component_files");
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

    async fn get_stream(
        &self,
        _target_label: &'static str,
        _op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> Result<Option<Pin<Box<dyn futures::Stream<Item = Result<Bytes, String>> + Send>>>, String>
    {
        let full_path = self.path_of(&namespace, path);
        self.ensure_path_is_inside_root(&full_path)?;

        if async_fs::metadata(&full_path).await.is_ok() {
            let file = tokio::fs::File::open(&full_path)
                .await
                .map_err(|err| format!("Failed to open file at {full_path:?}: {err}"))?;
            let stream = tokio_util::io::ReaderStream::new(file);
            Ok(Some(Box::pin(stream.map_err(|err| err.to_string()))))
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

    async fn put_stream(
        &self,
        _target_label: &'static str,
        _op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
        stream: &dyn ReplayableStream<Item = Result<Bytes, String>>,
    ) -> Result<(), String> {
        let full_path = self.path_of(&namespace, path);
        self.ensure_path_is_inside_root(&full_path)?;

        if let Some(parent) = full_path.parent() {
            if async_fs::metadata(parent).await.is_err() {
                async_fs::create_dir_all(parent).await.map_err(|err| {
                    format!("Failed to create parent directory {parent:?}: {err}")
                })?;
            }
        }

        let file = tokio::fs::File::create(&full_path)
            .await
            .map_err(|err| format!("Failed to create file at {full_path:?}: {err}"))?;

        let mut writer = tokio::io::BufWriter::new(file);

        let mut stream = stream.make_stream().await?;
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|err| err.to_string())?;
            writer
                .write_all(&chunk)
                .await
                .map_err(|err| err.to_string())?;
        }

        writer.flush().await.map_err(|err| err.to_string())?;
        Ok(())
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
        let namespace_root = self.path_of(&namespace, Path::new(""));
        let full_path = self.path_of(&namespace, path);
        self.ensure_path_is_inside_root(&full_path)?;

        let mut entries = async_fs::read_dir(&full_path)
            .await
            .map_err(|err| err.to_string())?;

        let mut result = Vec::new();
        while let Some(entry) = TryStreamExt::try_next(&mut entries)
            .await
            .map_err(|err| err.to_string())?
        {
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
    ) -> Result<bool, String> {
        let full_path = self.path_of(&namespace, path);
        self.ensure_path_is_inside_root(&full_path)?;

        let result = async_fs::remove_dir_all(&full_path).await;

        if let Err(err) = result {
            if err.kind() == std::io::ErrorKind::NotFound {
                Ok(false)
            } else {
                Err(err.to_string())
            }
        } else {
            Ok(true)
        }
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
