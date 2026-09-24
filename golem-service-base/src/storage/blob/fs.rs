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

use super::ErasedReplayableStream;
use crate::storage::blob::{
    BlobMetadata, BlobMissingError, BlobStorage, BlobStorageNamespace, ExistsResult,
    blob_path_is_root, reject_root_blob_path, validate_relative_blob_path,
};
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

const BLOB_FILE: &str = "~blob";
const DIRECTORY_MARKER_FILE: &str = "~dir";

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

    /// Attaches synchronously to an already-prepared blob storage root.
    ///
    /// Unlike [`Self::new`], this constructor:
    /// - does **not** create the root, `compilation_cache`, or `custom_data`
    ///   subdirectories (the caller is responsible for providing a fully
    ///   prepared root);
    /// - uses synchronous `std::fs::canonicalize` so it can be called from
    ///   non-async contexts.
    ///
    /// Intended for test-only "attach to a parent-prepared filesystem"
    /// flows such as the worker-side reconstruction of test fixture
    /// clusters in the test-r `Hosted` scope.
    pub fn attach_existing(root: &Path) -> Result<Self, Error> {
        let canonical = std::fs::canonicalize(root)
            .map_err(|err| anyhow!("Failed to canonicalize blob storage root: {err}"))?;
        Ok(Self { root: canonical })
    }

    fn namespace_root(&self, namespace: &BlobStorageNamespace) -> PathBuf {
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
                agent_id,
                agent_mode,
            } => {
                result.push("oplog_payload");
                result.push(super::agent_mode_prefix(*agent_mode));
                result.push(environment_id.to_string());
                // The filesystem backend needs a bounded worker-derived path component because
                // very long agent ids can exceed local filename limits on many operating systems.
                result.push(Self::filesystem_safe_oplog_payload_agent_key(agent_id));
            }
            BlobStorageNamespace::CompressedOplog {
                environment_id,
                component_id,
                agent_mode,
                level,
            } => {
                result.push("compressed_oplog");
                result.push(super::agent_mode_prefix(*agent_mode));
                result.push(environment_id.to_string());
                result.push(component_id.to_string());
                result.push(level.to_string());
            }
            BlobStorageNamespace::InitialAgentFiles { environment_id } => {
                result.push("initial_agent_files");
                result.push(environment_id.to_string());
            }
            BlobStorageNamespace::Components { environment_id } => {
                result.push("component_store");
                result.push(environment_id.to_string());
            }
        }

        result
    }

    fn node_of(&self, namespace: &BlobStorageNamespace, path: &Path) -> PathBuf {
        let mut result = self.namespace_root(namespace);
        for component in path.components() {
            if let std::path::Component::Normal(name) = component {
                let name = name
                    .to_str()
                    .expect("blob paths are validated before mapping");
                if name.starts_with('~') {
                    result.push(format!("~{name}"));
                } else {
                    result.push(name);
                }
            }
        }
        result
    }

    fn blob_of(&self, namespace: &BlobStorageNamespace, path: &Path) -> PathBuf {
        self.node_of(namespace, path).join(BLOB_FILE)
    }

    fn marker_of(&self, namespace: &BlobStorageNamespace, path: &Path) -> PathBuf {
        self.node_of(namespace, path).join(DIRECTORY_MARKER_FILE)
    }

    async fn prune_empty_nodes(
        &self,
        namespace: &BlobStorageNamespace,
        path: &Path,
    ) -> Result<(), Error> {
        let namespace_root = self.namespace_root(namespace);
        let mut node = self.node_of(namespace, path);
        while node != namespace_root {
            match async_fs::remove_dir(&node).await {
                Ok(()) => {}
                Err(err)
                    if matches!(
                        err.kind(),
                        std::io::ErrorKind::NotFound | std::io::ErrorKind::DirectoryNotEmpty
                    ) =>
                {
                    break;
                }
                Err(err) => return Err(err.into()),
            }
            let Some(parent) = node.parent() else {
                break;
            };
            node = parent.to_path_buf();
        }
        Ok(())
    }

    fn ensure_path_is_inside_root(&self, path: &Path) -> Result<(), Error> {
        if !path.starts_with(&self.root) {
            Err(anyhow!("Path {path:?} is not within: {:?}", self.root))
        } else {
            Ok(())
        }
    }

    pub fn filesystem_safe_oplog_payload_agent_key(
        agent_id: &golem_common::model::AgentId,
    ) -> String {
        let logical = agent_id.to_string();
        let digest = blake3::hash(logical.as_bytes()).to_hex();

        let mut sanitized_prefix = String::with_capacity(32);
        for ch in agent_id.agent_id.chars() {
            if sanitized_prefix.len() >= 32 {
                break;
            }

            if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_') {
                sanitized_prefix.push(ch);
            } else {
                sanitized_prefix.push('_');
            }
        }

        if sanitized_prefix.is_empty() {
            sanitized_prefix.push_str("agent");
        }

        format!("{sanitized_prefix}-{digest}")
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
        validate_relative_blob_path(path)?;
        if blob_path_is_root(path) {
            return Ok(None);
        }
        let full_path = self.blob_of(&namespace, path);
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
        validate_relative_blob_path(path)?;
        if blob_path_is_root(path) {
            return Ok(None);
        }
        let full_path = self.blob_of(&namespace, path);
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
        validate_relative_blob_path(path)?;
        if blob_path_is_root(path) {
            return Ok(None);
        }
        let blob_path = self.blob_of(&namespace, path);
        let marker_path = self.marker_of(&namespace, path);
        let full_path = if async_fs::metadata(&blob_path).await.is_ok() {
            blob_path
        } else {
            marker_path
        };
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
        validate_relative_blob_path(path)?;
        reject_root_blob_path(path)?;
        let full_path = self.blob_of(&namespace, path);
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
        validate_relative_blob_path(path)?;
        reject_root_blob_path(path)?;
        let full_path = self.blob_of(&namespace, path);
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
        validate_relative_blob_path(path)?;
        if blob_path_is_root(path) {
            return Ok(());
        }
        let full_path = self.blob_of(&namespace, path);
        self.ensure_path_is_inside_root(&full_path)?;

        match async_fs::remove_file(&full_path).await {
            Ok(()) => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => return Err(err.into()),
        }
        self.prune_empty_nodes(&namespace, path).await?;
        Ok(())
    }

    async fn create_dir(
        &self,
        _target_label: &'static str,
        _op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> Result<(), Error> {
        validate_relative_blob_path(path)?;
        if blob_path_is_root(path) {
            return Ok(());
        }
        let full_path = self.marker_of(&namespace, path);
        self.ensure_path_is_inside_root(&full_path)?;

        async_fs::create_dir_all(full_path.parent().unwrap()).await?;
        async_fs::write(&full_path, []).await?;

        Ok(())
    }

    async fn list_dir(
        &self,
        _target_label: &'static str,
        _op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> Result<Vec<PathBuf>, Error> {
        validate_relative_blob_path(path)?;
        let full_path = self.node_of(&namespace, path);
        self.ensure_path_is_inside_root(&full_path)?;

        let mut entries = match async_fs::read_dir(&full_path).await {
            Ok(entries) => entries,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(err) => return Err(err.into()),
        };

        let mut result = Vec::new();
        while let Some(entry) = TryStreamExt::try_next(&mut entries).await? {
            if !entry.file_type().await?.is_dir() {
                continue;
            }
            let encoded_name = entry.file_name();
            let Some(encoded_name) = encoded_name.to_str() else {
                continue;
            };
            let name = if let Some(name) = encoded_name.strip_prefix("~~") {
                format!("~{name}")
            } else {
                encoded_name.to_string()
            };
            let child = path.join(name);
            if async_fs::metadata(entry.path().join(BLOB_FILE))
                .await
                .is_ok()
            {
                result.push(child.clone());
            }
            if async_fs::metadata(entry.path().join(DIRECTORY_MARKER_FILE))
                .await
                .is_ok()
            {
                result.push(child);
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
        validate_relative_blob_path(path)?;

        if blob_path_is_root(path) {
            return Ok(false);
        }

        let full_path = self.node_of(&namespace, path);
        self.ensure_path_is_inside_root(&full_path)?;

        let mut entries = match async_fs::read_dir(&full_path).await {
            Ok(entries) => entries,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(err) => return Err(err.into()),
        };
        let mut deleted = false;
        while let Some(entry) = TryStreamExt::try_next(&mut entries).await? {
            if entry.file_name() == BLOB_FILE {
                continue;
            }
            deleted = true;
            if entry.file_type().await?.is_dir() {
                async_fs::remove_dir_all(entry.path()).await?;
            } else {
                async_fs::remove_file(entry.path()).await?;
            }
        }
        self.prune_empty_nodes(&namespace, path).await?;
        Ok(deleted)
    }

    async fn exists(
        &self,
        _target_label: &'static str,
        _op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> Result<ExistsResult, Error> {
        validate_relative_blob_path(path)?;
        if blob_path_is_root(path) {
            return Ok(ExistsResult::Directory);
        }
        let full_path = self.node_of(&namespace, path);
        self.ensure_path_is_inside_root(&full_path)?;

        if async_fs::metadata(full_path.join(BLOB_FILE)).await.is_ok() {
            Ok(ExistsResult::File)
        } else if async_fs::metadata(&full_path).await.is_ok() {
            Ok(ExistsResult::Directory)
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
        validate_relative_blob_path(from)?;
        validate_relative_blob_path(to)?;
        reject_root_blob_path(from)?;
        reject_root_blob_path(to)?;
        match self
            .get_raw(_target_label, _op_label, namespace.clone(), from)
            .await?
        {
            Some(data) => {
                self.put_raw(_target_label, _op_label, namespace, to, &data)
                    .await
            }
            None => Err(BlobMissingError {
                path: from.to_path_buf(),
            }
            .into()),
        }
    }
}
