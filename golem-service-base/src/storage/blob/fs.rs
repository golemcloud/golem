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

use super::ErasedReplayableStream;
use crate::storage::blob::{BlobMetadata, BlobStorage, BlobStorageNamespace, ExistsResult};
use anyhow::{Context, Error, anyhow};
use async_trait::async_trait;
use bytes::Bytes;
use futures::TryStreamExt;
use futures::stream::BoxStream;
use golem_common::model::Timestamp;
use std::path::{Path, PathBuf};
use std::time::SystemTime;
use tokio::io::AsyncWriteExt;
use tokio_stream::StreamExt;

#[derive(Debug)]
pub struct FileSystemBlobStorage {
    root: PathBuf,
}

impl FileSystemBlobStorage {
    pub async fn new(root: &Path) -> Result<Self, Error> {
        if async_fs::metadata(root).await.is_err() {
            async_fs::create_dir_all(root)
                .await
                .map_err(|err| anyhow!("Failed to create local blob storage: {err}"))?
        }
        let canonical = async_fs::canonicalize(root).await?;

        let compilation_cache = canonical.join("compilation_cache");

        if async_fs::metadata(&compilation_cache).await.is_err() {
            async_fs::create_dir_all(&compilation_cache)
                .await
                .context("Failed to create compilation_cache directory")?;
        }

        let custom_data = canonical.join("custom_data");

        if async_fs::metadata(&custom_data).await.is_err() {
            async_fs::create_dir_all(&custom_data)
                .await
                .context("Failed to create custom_data directory")?;
        }

        Ok(Self { root: canonical })
    }

    fn path_of(&self, namespace: &BlobStorageNamespace, path: &Path) -> PathBuf {
        let mut result = self.root.clone();

        match namespace {
            BlobStorageNamespace::CompilationCache { environment_id } => {
                result.push("compilation_cache");
                result.push(environment_id.to_string());
            }
            BlobStorageNamespace::CustomStorage { environment_id } => {
                result.push("custom_data");
                result.push(environment_id.to_string());
            }
            BlobStorageNamespace::OplogPayload {
                environment_id,
                worker_id,
            } => {
                result.push("oplog_payload");
                result.push(environment_id.to_string());
                result.push(worker_id.to_string());
            }
            BlobStorageNamespace::CompressedOplog {
                environment_id,
                component_id,
                level,
            } => {
                result.push("compressed_oplog");
                result.push(environment_id.to_string());
                result.push(component_id.to_string());
                result.push(level.to_string());
            }
            BlobStorageNamespace::InitialComponentFiles { environment_id } => {
                result.push("initial_component_files");
                result.push(environment_id.to_string());
            }
            BlobStorageNamespace::Components { environment_id } => {
                result.push("component_store");
                result.push(environment_id.to_string());
            }
        }

        result.push(path);
        result
    }

    fn ensure_path_is_inside_root(&self, path: &Path) -> Result<(), Error> {
        if !path.starts_with(&self.root) {
            Err(anyhow!("Path {path:?} is not within: {:?}", self.root))
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
    ) -> Result<Option<Vec<u8>>, Error> {
        let full_path = self.path_of(&namespace, path);
        self.ensure_path_is_inside_root(&full_path)?;

        if async_fs::metadata(&full_path).await.is_ok() {
            let data = async_fs::read(&full_path).await?;
            Ok(Some(data))
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
    ) -> Result<Option<BoxStream<'static, Result<Bytes, Error>>>, Error> {
        let full_path = self.path_of(&namespace, path);
        self.ensure_path_is_inside_root(&full_path)?;

        if async_fs::metadata(&full_path).await.is_ok() {
            let file = tokio::fs::File::open(&full_path).await?;
            let stream = tokio_util::io::ReaderStream::new(file);
            Ok(Some(Box::pin(stream.map_err(|err| err.into()))))
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
    ) -> Result<Option<BlobMetadata>, Error> {
        let full_path = self.path_of(&namespace, path);
        self.ensure_path_is_inside_root(&full_path)?;

        if let Ok(metadata) = async_fs::metadata(&full_path).await {
            let last_modified_at = metadata
                .modified()?
                .duration_since(SystemTime::UNIX_EPOCH)?
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
    ) -> Result<(), Error> {
        let full_path = self.path_of(&namespace, path);
        self.ensure_path_is_inside_root(&full_path)?;

        if let Some(parent) = full_path.parent()
            && async_fs::metadata(parent).await.is_err()
        {
            async_fs::create_dir_all(parent).await?;
        }

        async_fs::write(&full_path, data).await?;

        Ok(())
    }

    async fn put_stream(
        &self,
        _target_label: &'static str,
        _op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
        stream: &dyn ErasedReplayableStream<Item = Result<Vec<u8>, Error>, Error = Error>,
    ) -> Result<(), Error> {
        let full_path = self.path_of(&namespace, path);
        self.ensure_path_is_inside_root(&full_path)?;

        if let Some(parent) = full_path.parent()
            && async_fs::metadata(parent).await.is_err()
        {
            async_fs::create_dir_all(parent).await?;
        }

        let file = tokio::fs::File::create(&full_path).await?;

        let mut writer = tokio::io::BufWriter::new(file);

        let mut stream = stream.make_stream_erased().await?;
        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            writer.write_all(&chunk).await?;
        }

        writer.flush().await?;
        Ok(())
    }

    async fn delete(
        &self,
        _target_label: &'static str,
        _op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> Result<(), Error> {
        let full_path = self.path_of(&namespace, path);
        self.ensure_path_is_inside_root(&full_path)?;

        async_fs::remove_file(&full_path).await?;
        Ok(())
    }

    async fn create_dir(
        &self,
        _target_label: &'static str,
        _op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> Result<(), Error> {
        let full_path = self.path_of(&namespace, path);
        self.ensure_path_is_inside_root(&full_path)?;

        async_fs::create_dir_all(&full_path).await?;

        Ok(())
    }

    async fn list_dir(
        &self,
        _target_label: &'static str,
        _op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> Result<Vec<PathBuf>, Error> {
        let namespace_root = self.path_of(&namespace, Path::new(""));
        let full_path = self.path_of(&namespace, path);
        self.ensure_path_is_inside_root(&full_path)?;

        let mut entries = async_fs::read_dir(&full_path).await?;

        let mut result = Vec::new();
        while let Some(entry) = TryStreamExt::try_next(&mut entries).await? {
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
    ) -> Result<bool, Error> {
        let full_path = self.path_of(&namespace, path);
        self.ensure_path_is_inside_root(&full_path)?;

        let result = async_fs::remove_dir_all(&full_path).await;

        if let Err(err) = result {
            if err.kind() == std::io::ErrorKind::NotFound {
                Ok(false)
            } else {
                Err(err.into())
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
    ) -> Result<ExistsResult, Error> {
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
    ) -> Result<(), Error> {
        let from_full_path = self.path_of(&namespace, from);
        let to_full_path = self.path_of(&namespace, to);
        self.ensure_path_is_inside_root(&from_full_path)?;
        self.ensure_path_is_inside_root(&to_full_path)?;

        async_fs::copy(&from_full_path, &to_full_path).await?;
        Ok(())
    }
}
