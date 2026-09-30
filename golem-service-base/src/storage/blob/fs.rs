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
    BLOB_STREAM_CHUNK_SIZE, BlobMetadata, BlobRangeStream, BlobStorage, BlobStorageNamespace,
    ExistsResult, blob_parent_to_string, blob_path_is_root, blob_path_to_string, validate_range,
    validate_relative_blob_path,
};
use anyhow::{Context, Error, anyhow};
use async_trait::async_trait;
use bytes::Bytes;
use futures::TryStreamExt;
use futures::stream::BoxStream;
use golem_common::model::Timestamp;
use std::path::{Path, PathBuf};
use std::time::SystemTime;
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};
use tokio_stream::StreamExt;
use typed_path::{Utf8UnixComponent, Utf8UnixPath, Utf8UnixPathBuf};

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

    fn namespace_path(&self, namespace: &BlobStorageNamespace) -> PathBuf {
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

    fn encode_path_component(component: &str) -> Vec<String> {
        let encoded = hex::encode(component);
        let chunks = encoded.as_bytes().chunks(240);
        let chunk_count = chunks.len();
        chunks
            .enumerate()
            .map(|(index, chunk)| {
                let marker = if index + 1 == chunk_count { "e-" } else { "c-" };
                format!("{marker}{}", str::from_utf8(chunk).unwrap())
            })
            .collect()
    }

    fn decode_path_component(encoded: &str) -> Result<String, Error> {
        let decoded = hex::decode(encoded)
            .with_context(|| format!("Invalid filesystem blob path component: {encoded:?}"))?;
        String::from_utf8(decoded)
            .with_context(|| format!("Invalid UTF-8 filesystem blob path component: {encoded:?}"))
    }

    fn path_of(&self, namespace: &BlobStorageNamespace, path: &Path) -> Result<PathBuf, Error> {
        validate_relative_blob_path(path)?;
        let path = blob_path_to_string(path)?;
        let mut result = self.namespace_path(namespace);

        for component in Utf8UnixPath::new(&path).components() {
            match component {
                Utf8UnixComponent::Normal(component) => {
                    for encoded in Self::encode_path_component(component) {
                        result.push(encoded);
                    }
                }
                Utf8UnixComponent::CurDir => {}
                Utf8UnixComponent::ParentDir | Utf8UnixComponent::RootDir => unreachable!(),
            }
        }

        Ok(result)
    }

    fn logical_path(
        &self,
        namespace: &BlobStorageNamespace,
        path: &Path,
    ) -> Result<PathBuf, Error> {
        let relative = path.strip_prefix(self.namespace_path(namespace))?;
        let mut result = Utf8UnixPathBuf::new();
        let mut encoded = String::new();

        for component in relative.components() {
            let component = component
                .as_os_str()
                .to_str()
                .ok_or_else(|| anyhow!("Invalid UTF-8 filesystem blob path: {path:?}"))?;
            if let Some(chunk) = component.strip_prefix("c-") {
                encoded.push_str(chunk);
            } else if let Some(chunk) = component.strip_prefix("e-") {
                encoded.push_str(chunk);
                result.push(Self::decode_path_component(&encoded)?);
                encoded.clear();
            } else {
                return Err(anyhow!(
                    "Invalid filesystem blob path component: {component:?}"
                ));
            }
        }

        if !encoded.is_empty() {
            return Err(anyhow!("Incomplete filesystem blob path: {path:?}"));
        }

        Ok(PathBuf::from(result.as_str()))
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
        let full_path = self.path_of(&namespace, path)?;
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
        let full_path = self.path_of(&namespace, path)?;
        self.ensure_path_is_inside_root(&full_path)?;

        if async_fs::metadata(&full_path).await.is_ok() {
            let file = tokio::fs::File::open(&full_path).await?;
            let stream = tokio_util::io::ReaderStream::new(file);
            Ok(Some(Box::pin(stream.map_err(|err| err.into()))))
        } else {
            Ok(None)
        }
    }

    async fn get_range_stream(
        &self,
        _target_label: &'static str,
        _op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
        offset: u64,
        length: u64,
    ) -> Result<Option<BlobRangeStream>, Error> {
        validate_relative_blob_path(path)?;
        let full_path = self.path_of(&namespace, path)?;
        self.ensure_path_is_inside_root(&full_path)?;
        let mut file = match tokio::fs::File::open(full_path).await {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        let metadata = file.metadata().await?;
        anyhow::ensure!(metadata.is_file(), "Blob is not a regular file");
        let total_size = metadata.len();
        validate_range(offset, length, total_size)?;
        file.seek(std::io::SeekFrom::Start(offset)).await?;
        let stream =
            tokio_util::io::ReaderStream::with_capacity(file.take(length), BLOB_STREAM_CHUNK_SIZE);
        Ok(Some(BlobRangeStream {
            total_size,
            stream: Box::pin(stream.map_err(Error::from)),
        }))
    }

    async fn get_metadata(
        &self,
        _target_label: &'static str,
        _op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> Result<Option<BlobMetadata>, Error> {
        validate_relative_blob_path(path)?;
        let full_path = self.path_of(&namespace, path)?;
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
        let full_path = self.path_of(&namespace, path)?;
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
        let full_path = self.path_of(&namespace, path)?;
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
        let full_path = self.path_of(&namespace, path)?;
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
        validate_relative_blob_path(path)?;
        let full_path = self.path_of(&namespace, path)?;
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
        validate_relative_blob_path(path)?;
        let full_path = self.path_of(&namespace, path)?;
        self.ensure_path_is_inside_root(&full_path)?;

        let mut entries = async_fs::read_dir(&full_path).await?;

        let mut result = Vec::new();
        let mut pending = Vec::new();
        while let Some(entry) = TryStreamExt::try_next(&mut entries).await? {
            pending.push(entry.path());
        }

        while let Some(path) = pending.pop() {
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or_else(|| anyhow!("Invalid UTF-8 filesystem blob path: {path:?}"))?;
            if name.starts_with("c-") {
                let mut entries = async_fs::read_dir(&path).await?;
                while let Some(entry) = TryStreamExt::try_next(&mut entries).await? {
                    pending.push(entry.path());
                }
            } else if name.starts_with("e-") {
                result.push(self.logical_path(&namespace, &path)?);
            } else {
                return Err(anyhow!("Invalid filesystem blob path component: {name:?}"));
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

        let full_path = self.path_of(&namespace, path)?;
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
        validate_relative_blob_path(path)?;
        let full_path = self.path_of(&namespace, path)?;
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
        validate_relative_blob_path(from)?;
        validate_relative_blob_path(to)?;
        let from_full_path = self.path_of(&namespace, from)?;
        let to_full_path = self.path_of(&namespace, to)?;
        self.ensure_path_is_inside_root(&from_full_path)?;
        self.ensure_path_is_inside_root(&to_full_path)?;

        let logical_parent = blob_parent_to_string(to)?;
        let logical_parent_path = self.path_of(&namespace, Path::new(&logical_parent))?;
        let metadata = async_fs::metadata(&logical_parent_path).await?;
        if !metadata.is_dir() {
            return Err(anyhow!(
                "Blob destination parent is not a directory: {logical_parent:?}"
            ));
        }
        if let Some(encoded_parent) = to_full_path.parent()
            && encoded_parent != logical_parent_path
        {
            async_fs::create_dir_all(encoded_parent).await?;
        }

        async_fs::copy(&from_full_path, &to_full_path).await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::FileSystemBlobStorage;
    use crate::storage::blob::BlobStorageNamespace;
    use golem_common::model::environment::EnvironmentId;
    use std::path::{Component, Path, PathBuf};
    use test_r::test;

    #[test]
    fn filesystem_paths_encode_contract_components() {
        let storage = FileSystemBlobStorage {
            root: PathBuf::from("root"),
        };
        let namespace = BlobStorageNamespace::CustomStorage {
            environment_id: EnvironmentId::new(),
        };

        for logical in [
            r"photos/animals\cat.png",
            r"photos/..\..\other-environment\victim",
            r"photos/C:\cats\kitten.png",
            r"photos/\\server\share\kitten.png",
        ] {
            let physical = storage.path_of(&namespace, Path::new(logical)).unwrap();
            assert!(
                !physical
                    .components()
                    .any(|component| matches!(component, Component::ParentDir))
            );
            assert_eq!(
                storage.logical_path(&namespace, &physical).unwrap(),
                PathBuf::from(logical)
            );
        }

        assert_ne!(
            storage
                .path_of(&namespace, Path::new("photos/animals/cat.png"))
                .unwrap(),
            storage
                .path_of(&namespace, Path::new(r"photos/animals\cat.png"))
                .unwrap()
        );

        let first = storage.path_of(&namespace, Path::new("AAA")).unwrap();
        let second = storage.path_of(&namespace, Path::new("AA[")).unwrap();
        assert!(
            !first
                .to_str()
                .unwrap()
                .eq_ignore_ascii_case(second.to_str().unwrap())
        );

        let long_name = "a".repeat(255);
        let physical = storage.path_of(&namespace, Path::new(&long_name)).unwrap();
        assert!(physical.components().all(|component| {
            component
                .as_os_str()
                .to_str()
                .is_some_and(|component| component.len() <= 242)
        }));
        assert_eq!(
            storage.logical_path(&namespace, &physical).unwrap(),
            PathBuf::from(long_name)
        );
    }
}
