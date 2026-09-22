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
    BlobMetadata, BlobMissingError, BlobRangeError, BlobStorage, BlobStorageNamespace,
    ExistsResult, ListedBlob, NormalizedBlobPath, PutIfAbsent, agent_path_segment,
    blob_copy_changes_nothing, blob_positions, normalized_blob_path,
};
use anyhow::{Context, Error, anyhow};
use async_trait::async_trait;
use bytes::Bytes;
use futures::TryStreamExt;
use futures::io::{AsyncReadExt, AsyncSeekExt};
use futures::stream::BoxStream;
use golem_common::model::Timestamp;
use std::io::{ErrorKind, SeekFrom, Write};
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

    fn path_of(&self, namespace: &BlobStorageNamespace, path: &NormalizedBlobPath) -> PathBuf {
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
                result.push(agent_path_segment(agent_id));
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
            BlobStorageNamespace::FilesystemSnapshots {
                environment_id,
                agent_id,
            } => {
                result.push("filesystem_snapshots");
                result.push(environment_id.to_string());
                result.push(agent_path_segment(agent_id));
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
        let path = normalized_blob_path(path)?;
        let full_path = self.path_of(&namespace, &path);
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
        let path = normalized_blob_path(path)?;
        let full_path = self.path_of(&namespace, &path);
        self.ensure_path_is_inside_root(&full_path)?;

        if async_fs::metadata(&full_path).await.is_ok() {
            let file = tokio::fs::File::open(&full_path).await?;
            let stream = tokio_util::io::ReaderStream::new(file);
            Ok(Some(Box::pin(stream.map_err(|err| err.into()))))
        } else {
            Ok(None)
        }
    }

    /// Reads only the bytes of the range from the file. The rules of the trait apply. A path
    /// that has no metadata has no blob, as for `get_raw`. A directory gives an error of the
    /// kind [`ErrorKind::IsADirectory`], which is the error that `get_raw` gives for it.
    async fn get_raw_slice(
        &self,
        _target_label: &'static str,
        _op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
        start: u64,
        end: u64,
    ) -> Result<Option<Vec<u8>>, Error> {
        if start > end {
            return Err(BlobRangeError { start, end }.into());
        }
        let path = normalized_blob_path(path)?;
        let full_path = self.path_of(&namespace, &path);
        self.ensure_path_is_inside_root(&full_path)?;

        if async_fs::metadata(&full_path).await.is_err() {
            return Ok(None);
        }
        let mut file = async_fs::File::open(&full_path).await?;
        // The length comes from the open file, so a change of the path after the open does not
        // change it.
        let metadata = file.metadata().await?;
        if metadata.is_dir() {
            return Err(std::io::Error::from(ErrorKind::IsADirectory).into());
        }
        let positions = blob_positions(usize::try_from(metadata.len())?, start, end)?;
        let mut bytes = vec![0; positions.end() - positions.start() + 1];
        file.seek(SeekFrom::Start(start)).await?;
        file.read_exact(&mut bytes).await?;
        Ok(Some(bytes))
    }

    async fn get_metadata(
        &self,
        _target_label: &'static str,
        _op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> Result<Option<BlobMetadata>, Error> {
        let path = normalized_blob_path(path)?;
        let full_path = self.path_of(&namespace, &path);
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
        let path = normalized_blob_path(path)?;
        let full_path = self.path_of(&namespace, &path);
        self.ensure_path_is_inside_root(&full_path)?;

        if let Some(parent) = full_path.parent()
            && async_fs::metadata(parent).await.is_err()
        {
            async_fs::create_dir_all(parent).await?;
        }

        async_fs::write(&full_path, data).await?;

        Ok(())
    }

    async fn put_raw_if_absent(
        &self,
        _target_label: &'static str,
        _op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
        data: &[u8],
    ) -> Result<PutIfAbsent, Error> {
        let path = normalized_blob_path(path)?;
        path.reject_root()?;
        let full_path = self.path_of(&namespace, &path);
        self.ensure_path_is_inside_root(&full_path)?;
        let staging = self.root.join(STAGING_DIRECTORY);
        let data: Box<[u8]> = Box::from(data);

        Ok(
            tokio::task::spawn_blocking(move || write_if_absent(&staging, &full_path, &data))
                .await??,
        )
    }

    async fn put_stream(
        &self,
        _target_label: &'static str,
        _op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
        stream: &dyn ErasedReplayableStream<Item = Result<Vec<u8>, Error>, Error = Error>,
    ) -> Result<(), Error> {
        let path = normalized_blob_path(path)?;
        let full_path = self.path_of(&namespace, &path);
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
        let path = normalized_blob_path(path)?;
        let full_path = self.path_of(&namespace, &path);
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
        let path = normalized_blob_path(path)?;

        if path.is_root() {
            return Ok(());
        }

        let full_path = self.path_of(&namespace, &path);
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
        let path = normalized_blob_path(path)?;
        let namespace_root = self.path_of(&namespace, &NormalizedBlobPath::root());
        let full_path = self.path_of(&namespace, &path);
        self.ensure_path_is_inside_root(&full_path)?;

        // The directory of a namespace comes into being with the first write below it, so a
        // path that is not there holds nothing, and so does an untouched namespace.
        let mut entries = match async_fs::read_dir(&full_path).await {
            Ok(entries) => entries,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(err) => return Err(err.into()),
        };

        let mut result = Vec::new();
        while let Some(entry) = TryStreamExt::try_next(&mut entries).await? {
            if let Ok(path) = entry.path().strip_prefix(&namespace_root) {
                result.push(path.to_path_buf());
            }
        }
        Ok(result)
    }

    async fn list_blobs_below(
        &self,
        _target_label: &'static str,
        _op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> Result<Box<[ListedBlob]>, Error> {
        let path = normalized_blob_path(path)?;
        let namespace_root = self.path_of(&namespace, &NormalizedBlobPath::root());
        let full_path = self.path_of(&namespace, &path);
        self.ensure_path_is_inside_root(&full_path)?;

        Ok(
            tokio::task::spawn_blocking(move || list_files_below(&full_path, &namespace_root))
                .await??,
        )
    }

    async fn delete_dir(
        &self,
        _target_label: &'static str,
        _op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> Result<bool, Error> {
        let path = normalized_blob_path(path)?;

        if path.is_root() {
            return Ok(false);
        }

        let full_path = self.path_of(&namespace, &path);
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
        let path = normalized_blob_path(path)?;

        // The root of a namespace is a directory, also before the first write makes it.
        if path.is_root() {
            return Ok(ExistsResult::Directory);
        }

        let full_path = self.path_of(&namespace, &path);
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
        // `BlobMissingError` names the path as the guest wrote it. The next line makes `from`
        // the normalized path, so keep the path of the guest first.
        let guest_from = from;
        let from = normalized_blob_path(from)?;
        let to = normalized_blob_path(to)?;

        // A copy onto the same path writes nothing, and it still needs the blob that it reads.
        // `async_fs::copy` opens the target for writing before it reads the source, so with one
        // path it empties the blob.
        if blob_copy_changes_nothing(&from, &to)? {
            return match self
                .exists(_target_label, _op_label, namespace, &from)
                .await?
            {
                ExistsResult::File => Ok(()),
                _ => Err(BlobMissingError {
                    path: guest_from.to_path_buf(),
                }
                .into()),
            };
        }

        let from_full_path = self.path_of(&namespace, &from);
        let to_full_path = self.path_of(&namespace, &to);
        self.ensure_path_is_inside_root(&from_full_path)?;
        self.ensure_path_is_inside_root(&to_full_path)?;

        // As `put_raw` does, the copy makes the directory of the target when it is not there.
        if let Some(parent) = to_full_path.parent()
            && async_fs::metadata(parent).await.is_err()
        {
            async_fs::create_dir_all(parent).await?;
        }
        async_fs::copy(&from_full_path, &to_full_path).await?;
        Ok(())
    }
}

/// The directory at the storage root that holds the bytes of a `put_raw_if_absent` call while
/// the call writes them.
///
/// The directory is outside the directory of every namespace, so no listing shows a blob that is
/// only partly written. A process that stops during a write can leave a file in it, and no
/// namespace sees that file.
const STAGING_DIRECTORY: &str = ".staging";

/// Writes `data` as the file at `target` when `target` has no file.
///
/// The bytes go to a new file in `staging` first. Then the file gets the name `target` in one step.
/// That step refuses a name that exists. So a reader sees the whole file or no file. Of two calls
/// for one `target`, only one gives `Written`. The file in `staging` goes away when the step fails.
fn write_if_absent(staging: &Path, target: &Path, data: &[u8]) -> std::io::Result<PutIfAbsent> {
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::create_dir_all(staging)?;
    let mut file = tempfile::NamedTempFile::new_in(staging)?;
    file.write_all(data)?;
    match file.persist_noclobber(target) {
        Ok(_) => Ok(PutIfAbsent::Written),
        Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => {
            Ok(PutIfAbsent::AlreadyExists)
        }
        Err(error) => Err(error.error),
    }
}

/// Lists each regular file below `directory`, with its path relative to `root` and its size.
///
/// A `directory` that does not exist, or that is not a directory, gives an empty list.
fn list_files_below(directory: &Path, root: &Path) -> std::io::Result<Box<[ListedBlob]>> {
    match std::fs::read_dir(directory) {
        Ok(entries) => add_files(entries, root, Vec::new()).map(Vec::into_boxed_slice),
        Err(err)
            if matches!(
                err.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
            ) =>
        {
            Ok(Box::default())
        }
        Err(err) => Err(err),
    }
}

/// Adds each regular file below the entries of a directory to `listed`, and gives the list back.
///
/// The walk does not follow symlinks, and symlinks are not in the list.
fn add_files(
    mut entries: std::fs::ReadDir,
    root: &Path,
    listed: Vec<ListedBlob>,
) -> std::io::Result<Vec<ListedBlob>> {
    entries.try_fold(listed, |mut listed, entry| {
        let entry = entry?;
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            add_files(std::fs::read_dir(entry.path())?, root, listed)
        } else if file_type.is_file() {
            let path = entry.path();
            let relative = path.strip_prefix(root).map_err(std::io::Error::other)?;
            listed.push(ListedBlob {
                path: relative.into(),
                size: entry.metadata()?.len(),
            });
            Ok(listed)
        } else {
            Ok(listed)
        }
    })
}

#[cfg(test)]
mod tests;
