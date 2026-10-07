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
    BLOB_STREAM_CHUNK_SIZE, BlobMetadata, BlobRangeStream, BlobStorageBackend,
    BlobStorageNamespace, ExistsResult, ListedBlob, NormalizedBlobPath, PutIfAbsent,
    agent_path_segment, blob_positions, validate_range,
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
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime};
use tokio::io::AsyncWriteExt;
use tokio_stream::StreamExt;

#[derive(Debug)]
pub struct FileSystemBlobStorage {
    root: PathBuf,
    /// A gate that each blocking write and delete passes right before its last check of the drop
    /// flag, so that a test can hold the write there.
    #[cfg(test)]
    before_commit: Option<CommitGate>,
    /// A hook that each whole and partial read runs right after its metadata check, so that a
    /// test can remove the blob there.
    #[cfg(test)]
    after_metadata: Option<CommitGate>,
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

        // An old file of the staging directory that cannot be removed only takes space, so the
        // storage starts and logs it.
        let staging = canonical.join(STAGING_DIRECTORY);
        if let Err(error) = tokio::task::spawn_blocking(move || {
            remove_old_staging_files(&staging, SystemTime::now())
        })
        .await?
        {
            tracing::warn!(
                error = %error,
                "Failed to remove old files of the staging directory of the blob storage"
            );
        }

        Ok(Self {
            root: canonical,
            #[cfg(test)]
            before_commit: None,
            #[cfg(test)]
            after_metadata: None,
        })
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
        Ok(Self {
            root: canonical,
            #[cfg(test)]
            before_commit: None,
            #[cfg(test)]
            after_metadata: None,
        })
    }

    /// Runs the hook of a test after the metadata check of a read. Production code has no hook.
    fn after_metadata(&self) {
        #[cfg(test)]
        if let Some(hook) = &self.after_metadata {
            (hook.0)();
        }
    }

    /// Gives the directory of the namespace, which holds the blobs of the namespace.
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
                fingerprint,
            } => {
                result.push("filesystem_snapshots");
                result.push(environment_id.to_string());
                result.push(agent_path_segment(agent_id));
                result.push(fingerprint.0.to_string());
            }
        }

        result
    }

    /// Gives the file of the blob at `path` in the namespace: the directory of the namespace and
    /// the encoded names of the path (`encoded_name`).
    fn path_of(
        &self,
        namespace: &BlobStorageNamespace,
        path: &NormalizedBlobPath,
    ) -> Result<PathBuf, Error> {
        Ok(path.names()?.flat_map(encoded_name).fold(
            self.namespace_path(namespace),
            |mut result, part| {
                result.push(part);
                result
            },
        ))
    }

    fn ensure_path_is_inside_root(&self, path: &Path) -> Result<(), Error> {
        if !path.starts_with(&self.root) {
            Err(anyhow!("Path {path:?} is not within: {:?}", self.root))
        } else {
            Ok(())
        }
    }

    /// Runs `work` on a blocking thread with a [`Commit`] whose drop flag this call sets when it is
    /// dropped. So a queued `work` that starts after the drop, or that reaches its last check after
    /// it, gives up.
    async fn unless_dropped<T: Send + 'static>(
        &self,
        work: impl FnOnce(&Commit) -> std::io::Result<T> + Send + 'static,
    ) -> Result<T, Error> {
        let dropped = SetOnDrop(Arc::new(AtomicBool::new(false)));
        let commit = Commit {
            dropped: dropped.0.clone(),
            #[cfg(test)]
            gate: self.before_commit.clone(),
        };
        Ok(tokio::task::spawn_blocking(move || work(&commit)).await??)
    }
}

/// Sets its flag when it is dropped.
struct SetOnDrop(Arc<AtomicBool>);

impl Drop for SetOnDrop {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}

/// What a blocking write shares with its async call.
struct Commit {
    /// True after the async call was dropped.
    dropped: Arc<AtomicBool>,
    #[cfg(test)]
    gate: Option<CommitGate>,
}

impl Commit {
    /// Gives an error of the kind [`ErrorKind::Interrupted`] when the async call was dropped.
    fn go_on(&self) -> std::io::Result<()> {
        if self.dropped.load(Ordering::Acquire) {
            Err(std::io::Error::new(
                ErrorKind::Interrupted,
                "the call of the write was dropped",
            ))
        } else {
            Ok(())
        }
    }

    /// The last check before the step that makes the write visible.
    fn before_commit(&self) -> std::io::Result<()> {
        #[cfg(test)]
        if let Some(gate) = &self.gate {
            (gate.0)();
        }
        self.go_on()
    }
}

/// A test gate of [`Commit::before_commit`].
#[cfg(test)]
#[derive(Clone)]
struct CommitGate(Arc<dyn Fn() + Send + Sync>);

#[cfg(test)]
impl std::fmt::Debug for CommitGate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("CommitGate")
    }
}

#[async_trait]
impl BlobStorageBackend for FileSystemBlobStorage {
    async fn get_raw_at(
        &self,
        _target_label: &'static str,
        _op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
    ) -> Result<Option<Vec<u8>>, Error> {
        let full_path = self.path_of(&namespace, path)?;
        self.ensure_path_is_inside_root(&full_path)?;

        if async_fs::metadata(&full_path).await.is_ok() {
            self.after_metadata();
            Ok(absent_on_not_found(async_fs::read(&full_path).await)?)
        } else {
            Ok(None)
        }
    }

    async fn get_stream_at(
        &self,
        _target_label: &'static str,
        _op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
    ) -> Result<Option<BoxStream<'static, Result<Bytes, Error>>>, Error> {
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

    async fn get_range_stream_at(
        &self,
        _target_label: &'static str,
        _op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
        offset: u64,
        length: u64,
    ) -> Result<Option<BlobRangeStream>, Error> {
        let full_path = self.path_of(&namespace, path)?;
        self.ensure_path_is_inside_root(&full_path)?;
        let mut file = match tokio::fs::File::open(full_path).await {
            Ok(file) => file,
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        let metadata = file.metadata().await?;
        anyhow::ensure!(metadata.is_file(), "Blob is not a regular file");
        let total_size = metadata.len();
        validate_range(offset, length, total_size)?;
        tokio::io::AsyncSeekExt::seek(&mut file, SeekFrom::Start(offset)).await?;
        let stream = tokio_util::io::ReaderStream::with_capacity(
            tokio::io::AsyncReadExt::take(file, length),
            BLOB_STREAM_CHUNK_SIZE,
        );
        Ok(Some(BlobRangeStream {
            total_size,
            stream: Box::pin(stream.map_err(Error::from)),
        }))
    }

    /// Reads only the bytes of the range from the file. The rules of the trait apply. A path
    /// that has no metadata has no blob, as for `get_raw`. A directory gives an error of the
    /// kind [`ErrorKind::IsADirectory`], which is the error that `get_raw` gives for it.
    async fn get_raw_slice_at(
        &self,
        _target_label: &'static str,
        _op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
        start: u64,
        end: u64,
    ) -> Result<Option<Vec<u8>>, Error> {
        let full_path = self.path_of(&namespace, path)?;
        self.ensure_path_is_inside_root(&full_path)?;

        if async_fs::metadata(&full_path).await.is_err() {
            return Ok(None);
        }
        self.after_metadata();
        let Some(mut file) = absent_on_not_found(async_fs::File::open(&full_path).await)? else {
            return Ok(None);
        };
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

    async fn get_metadata_at(
        &self,
        _target_label: &'static str,
        _op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
    ) -> Result<Option<BlobMetadata>, Error> {
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

    async fn put_raw_at(
        &self,
        _target_label: &'static str,
        _op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
        data: &[u8],
    ) -> Result<(), Error> {
        let full_path = self.path_of(&namespace, path)?;
        self.ensure_path_is_inside_root(&full_path)?;

        if matches!(namespace, BlobStorageNamespace::FilesystemSnapshots { .. }) {
            let staging = self.root.join(STAGING_DIRECTORY);
            let data: Box<[u8]> = Box::from(data);
            return self
                .unless_dropped(move |commit| write_staged(commit, &staging, &full_path, &data))
                .await;
        }

        if let Some(parent) = full_path.parent()
            && async_fs::metadata(parent).await.is_err()
        {
            async_fs::create_dir_all(parent).await?;
        }

        async_fs::write(&full_path, data).await?;

        Ok(())
    }

    async fn put_raw_if_absent_at(
        &self,
        _target_label: &'static str,
        _op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
        data: &[u8],
    ) -> Result<PutIfAbsent, Error> {
        let full_path = self.path_of(&namespace, path)?;
        self.ensure_path_is_inside_root(&full_path)?;
        let staging = self.root.join(STAGING_DIRECTORY);
        let data: Box<[u8]> = Box::from(data);

        self.unless_dropped(move |commit| write_if_absent(commit, &staging, &full_path, &data))
            .await
    }

    async fn put_stream_at(
        &self,
        _target_label: &'static str,
        _op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
        stream: &dyn ErasedReplayableStream<Item = Result<Vec<u8>, Error>, Error = Error>,
    ) -> Result<(), Error> {
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

    async fn delete_at(
        &self,
        _target_label: &'static str,
        _op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
    ) -> Result<(), Error> {
        let full_path = self.path_of(&namespace, path)?;
        self.ensure_path_is_inside_root(&full_path)?;

        if matches!(namespace, BlobStorageNamespace::FilesystemSnapshots { .. }) {
            return self
                .unless_dropped(move |commit| remove_unless_dropped(commit, &full_path))
                .await;
        }

        Ok(absent_on_not_found(async_fs::remove_file(&full_path).await).map(|_| ())?)
    }

    async fn create_dir_at(
        &self,
        _target_label: &'static str,
        _op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
    ) -> Result<(), Error> {
        let full_path = self.path_of(&namespace, path)?;
        self.ensure_path_is_inside_root(&full_path)?;

        async_fs::create_dir_all(&full_path).await?;

        Ok(())
    }

    async fn list_dir_at(
        &self,
        _target_label: &'static str,
        _op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
    ) -> Result<Vec<PathBuf>, Error> {
        let namespace_root = self.namespace_path(&namespace);
        let full_path = self.path_of(&namespace, path)?;
        self.ensure_path_is_inside_root(&full_path)?;

        Ok(
            tokio::task::spawn_blocking(move || list_names_in(&full_path, &namespace_root))
                .await??,
        )
    }

    async fn list_blobs_below_at(
        &self,
        _target_label: &'static str,
        _op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
    ) -> Result<Box<[ListedBlob]>, Error> {
        let namespace_root = self.namespace_path(&namespace);
        let full_path = self.path_of(&namespace, path)?;
        self.ensure_path_is_inside_root(&full_path)?;

        Ok(
            tokio::task::spawn_blocking(move || list_files_below(&full_path, &namespace_root))
                .await??,
        )
    }

    async fn delete_dir_at(
        &self,
        _target_label: &'static str,
        _op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
    ) -> Result<bool, Error> {
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

    async fn exists_at(
        &self,
        _target_label: &'static str,
        _op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
    ) -> Result<ExistsResult, Error> {
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

    /// Copies the file. The source is opened first, so a source with no file gives false and
    /// writes nothing, and an error of the target is never read as a missing source. The bytes go
    /// to a new file in the staging directory of the root, which then gets the name of the target
    /// in one step, as `put_raw_if_absent_at` writes. So a reader sees the whole target or the
    /// one before. As `put_raw_at` does, the copy makes the directory of the target when it is not
    /// there.
    async fn copy_between_at(
        &self,
        _target_label: &'static str,
        _op_label: &'static str,
        from_namespace: BlobStorageNamespace,
        from: &NormalizedBlobPath<'_>,
        to_namespace: BlobStorageNamespace,
        to: &NormalizedBlobPath<'_>,
    ) -> Result<bool, Error> {
        let from_full_path = self.path_of(&from_namespace, from)?;
        let to_full_path = self.path_of(&to_namespace, to)?;
        self.ensure_path_is_inside_root(&from_full_path)?;
        self.ensure_path_is_inside_root(&to_full_path)?;
        let staging = self.root.join(STAGING_DIRECTORY);

        self.unless_dropped(move |commit| {
            copy_staged(commit, &from_full_path, &staging, &to_full_path)
        })
        .await
    }
}

/// The directory at the storage root that holds the bytes of a `put_raw_if_absent` call, of a
/// copy, and of a `put_raw` call in the filesystem snapshot namespace, while the call writes them.
///
/// The directory is outside the directory of every namespace, so no listing shows a blob that is
/// only partly written. A process that stops during a write can leave a file in it, and no
/// namespace sees that file. `FileSystemBlobStorage::new` removes such a file when it is older than
/// [`STAGING_FILE_AGE`].
const STAGING_DIRECTORY: &str = ".staging";

/// The age after which `FileSystemBlobStorage::new` removes a file of the staging directory.
///
/// Services that share a root can have writes in flight, so a younger file can belong to a write
/// that still runs.
const STAGING_FILE_AGE: Duration = Duration::from_secs(60 * 60);

/// The mode that a staged `put_raw` file gets before the process umask, as `File::create` gives.
#[cfg(unix)]
const STAGED_PUT_MODE: u32 = 0o666;

/// Tells if a file of the staging directory that was last changed at `modified` is old enough at
/// `now` to remove.
fn staging_file_is_old(modified: SystemTime, now: SystemTime) -> bool {
    now.duration_since(modified)
        .is_ok_and(|age| age > STAGING_FILE_AGE)
}

/// Removes each file of `staging` that [`staging_file_is_old`] finds old. A `staging` that does not
/// exist holds nothing, and a file that another process removed first is not an error. A file that
/// cannot be removed does not stop the removal of the other files; the call then gives the first
/// error.
fn remove_old_staging_files(staging: &Path, now: SystemTime) -> std::io::Result<()> {
    let Some(entries) = absent_on_not_found(std::fs::read_dir(staging))? else {
        return Ok(());
    };
    first_error(entries.map(|entry| {
        let Some(entry) = listed_entry(entry)? else {
            return Ok(());
        };
        let Some(metadata) = listed_entry(entry.metadata())? else {
            return Ok(());
        };
        if metadata.is_file() && staging_file_is_old(metadata.modified()?, now) {
            absent_on_not_found(std::fs::remove_file(entry.path())).map(|_| ())
        } else {
            Ok(())
        }
    }))
}

/// Runs each step of `steps`, also after a step failed, and gives the first error.
fn first_error(steps: impl Iterator<Item = std::io::Result<()>>) -> std::io::Result<()> {
    // The fold consumes every step, so each one runs, and it keeps only the first error. A
    // `try_fold` would stop at the first error and skip the steps after it.
    steps
        .fold(None, |first: Option<std::io::Error>, step| {
            first.or(step.err())
        })
        .map_or(Ok(()), Err)
}

/// Gives `None` for a read that found nothing at its path, which a remove of the path after its
/// metadata was read causes.
fn absent_on_not_found<T>(read: std::io::Result<T>) -> std::io::Result<Option<T>> {
    match read {
        Ok(found) => Ok(Some(found)),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

/// Gives `None` for a part of a listing that a change during the listing took away: an entry, its
/// kind, its metadata or the listing of a directory below it. A remove gives `NotFound`, and a
/// directory that a write replaced with a file gives `NotADirectory`. Every other error stays.
fn listed_entry<T>(found: std::io::Result<T>) -> std::io::Result<Option<T>> {
    match found {
        Err(error) if error.kind() == ErrorKind::NotADirectory => Ok(None),
        found => absent_on_not_found(found),
    }
}

/// Writes `data` as the file at `target`, over the file that was there.
///
/// The bytes go to a new file in `staging` first. Then the file gets the name `target` in one step.
/// So a reader sees the whole new file or the one before. The new file gets the mode that
/// `File::create` gives. A `commit` whose call was dropped stops the write before it makes a
/// directory and before the step.
fn write_staged(
    commit: &Commit,
    staging: &Path,
    target: &Path,
    data: &[u8],
) -> std::io::Result<()> {
    commit.go_on()?;
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::create_dir_all(staging)?;
    let mut file = staged_put_file(staging)?;
    file.write_all(data)?;
    commit.before_commit()?;
    file.persist(target).map_err(|error| error.error)?;
    Ok(())
}

/// A new file in `staging` with the mode of [`STAGED_PUT_MODE`], which the umask then masks.
#[cfg(unix)]
fn staged_put_file(staging: &Path) -> std::io::Result<tempfile::NamedTempFile> {
    use std::os::unix::fs::PermissionsExt;
    tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(STAGED_PUT_MODE))
        .tempfile_in(staging)
}

/// A new file in `staging`.
#[cfg(not(unix))]
fn staged_put_file(staging: &Path) -> std::io::Result<tempfile::NamedTempFile> {
    tempfile::NamedTempFile::new_in(staging)
}

/// Removes the file at `target`. A `target` with no file changes nothing. A `commit` whose call was
/// dropped stops the remove.
fn remove_unless_dropped(commit: &Commit, target: &Path) -> std::io::Result<()> {
    commit.go_on()?;
    commit.before_commit()?;
    absent_on_not_found(std::fs::remove_file(target)).map(|_| ())
}

/// Writes `data` as the file at `target` when `target` has no file.
///
/// The bytes go to a new file in `staging` first. Then the file gets the name `target` in one step.
/// That step refuses a name that exists. So a reader sees the whole file or no file. Of two calls
/// for one `target`, only one gives `Written`. The file in `staging` goes away when the step fails.
/// A `commit` whose call was dropped stops the write before it makes a directory and before the
/// step.
fn write_if_absent(
    commit: &Commit,
    staging: &Path,
    target: &Path,
    data: &[u8],
) -> std::io::Result<PutIfAbsent> {
    commit.go_on()?;
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::create_dir_all(staging)?;
    let mut file = tempfile::NamedTempFile::new_in(staging)?;
    file.write_all(data)?;
    commit.before_commit()?;
    match file.persist_noclobber(target) {
        Ok(_) => Ok(PutIfAbsent::Written),
        Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => {
            Ok(PutIfAbsent::AlreadyExists)
        }
        Err(error) => Err(error.error),
    }
}

/// Tells whether an error of the open of the source of a copy means that the source has no file:
/// no entry at the path, or a path below a file.
fn no_source_file(kind: std::io::ErrorKind) -> bool {
    matches!(
        kind,
        std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
    )
}

/// Copies the file at `source` to `target` through a new file in `staging`, and gives false when
/// `source` has no file: no entry, a directory, or a path below a file. The source is opened and
/// checked before anything is written. The new file gets the permissions of the source before it
/// gets the name `target`; when it cannot get them, the copy fails and names no file. A `commit` whose call was dropped stops the copy before it
/// makes a directory and before the step that names the target.
fn copy_staged(
    commit: &Commit,
    source: &Path,
    staging: &Path,
    target: &Path,
) -> std::io::Result<bool> {
    commit.go_on()?;
    let mut source = match std::fs::File::open(source) {
        Ok(source) => source,
        Err(error) if no_source_file(error.kind()) => return Ok(false),
        Err(error) => return Err(error),
    };
    let metadata = source.metadata()?;
    if !metadata.is_file() {
        return Ok(false);
    }
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::create_dir_all(staging)?;
    let mut staged = tempfile::NamedTempFile::new_in(staging)?;
    // A copy between two `File`s lets the standard library use the copy of the kernel.
    std::io::copy(&mut source, staged.as_file_mut())?;
    staged.as_file().set_permissions(metadata.permissions())?;
    commit.before_commit()?;
    staged.persist(target).map_err(|error| error.error)?;
    Ok(true)
}

/// Lists each regular file below `directory`, with its blob path below `root`, the directory of
/// its namespace, and its size.
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
/// The walk does not follow symlinks, and symlinks are not in the list. An entry that a remove
/// takes away, or that a write gives another kind, during the walk can be absent from the list,
/// the walk goes on, and a directory is never in the list.
fn add_files(
    mut entries: impl Iterator<Item = std::io::Result<std::fs::DirEntry>>,
    root: &Path,
    listed: Vec<ListedBlob>,
) -> std::io::Result<Vec<ListedBlob>> {
    entries.try_fold(listed, |mut listed, entry| {
        let Some(entry) = listed_entry(entry)? else {
            return Ok(listed);
        };
        let Some(file_type) = listed_entry(entry.file_type())? else {
            return Ok(listed);
        };
        if file_type.is_dir() {
            match listed_entry(std::fs::read_dir(entry.path()))? {
                Some(below) => add_files(below, root, listed),
                None => Ok(listed),
            }
        } else if file_type.is_file() {
            // The kind of the entry can be older than the entry, so a directory that a write put
            // at the path of the file is left out by its metadata.
            let Some(metadata) = listed_entry(entry.metadata())?.filter(|found| found.is_file())
            else {
                return Ok(listed);
            };
            listed.push(ListedBlob {
                path: blob_path_of(&entry.path(), root)?.into_boxed_path(),
                size: metadata.len(),
            });
            Ok(listed)
        } else {
            Ok(listed)
        }
    })
}

/// Lists the entries of the directory of a blob path, with their blob paths below `root`, the
/// directory of the namespace: each blob and each directory of the directory.
///
/// A `directory` that does not exist holds nothing, because the directory of a namespace comes
/// into being with the first write below it. The parts of a long name (`encoded_name`) are one
/// entry. An entry that a remove takes away during the listing is not in the list.
fn list_names_in(directory: &Path, root: &Path) -> std::io::Result<Vec<PathBuf>> {
    match std::fs::read_dir(directory) {
        Ok(entries) => add_names(entries, root, Vec::new()),
        Err(err) if err.kind() == ErrorKind::NotFound => Ok(Vec::new()),
        Err(err) => Err(err),
    }
}

/// Adds the blob path of each entry of a directory to `listed`, and gives the list back. A
/// directory of a part of a long name is not an entry: the walk adds the entries below it.
fn add_names(
    mut entries: impl Iterator<Item = std::io::Result<std::fs::DirEntry>>,
    root: &Path,
    listed: Vec<PathBuf>,
) -> std::io::Result<Vec<PathBuf>> {
    entries.try_fold(listed, |mut listed, entry| {
        let Some(entry) = listed_entry(entry)? else {
            return Ok(listed);
        };
        let path = entry.path();
        if entry
            .file_name()
            .as_encoded_bytes()
            .starts_with(CONTINUED_PART.as_bytes())
        {
            match listed_entry(std::fs::read_dir(&path))? {
                Some(below) => add_names(below, root, listed),
                None => Ok(listed),
            }
        } else {
            listed.push(blob_path_of(&path, root)?);
            Ok(listed)
        }
    })
}

/// The marker of a part of an encoded name that more parts follow (`encoded_name`).
const CONTINUED_PART: &str = "c-";

/// The marker of the last part of an encoded name (`encoded_name`).
const LAST_PART: &str = "e-";

/// The largest number of hex characters in one part of an encoded name. With its marker, a part
/// has at most 242 bytes, and the filesystems of the hosts that run Golem accept a file name of
/// 255 bytes.
const PART_HEX_LENGTH: usize = 240;

/// Gives the file names that hold one name of a blob path on disk, from the first to the last.
///
/// The name is written as lowercase hex, so each character of a name keeps its meaning on every
/// host: a `\` or a `:` is not a separator or a prefix of the host, and two names that differ
/// only in the case of their letters stay two names on a filesystem that ignores case. The hex is
/// cut into parts of at most [`PART_HEX_LENGTH`] characters, so a long name stays below the name
/// limit of the filesystem. Each part but the last is a directory with the marker
/// [`CONTINUED_PART`], and the last part has the marker [`LAST_PART`].
fn encoded_name(name: &str) -> impl Iterator<Item = String> + use<> {
    let hex = hex::encode(name);
    let parts = hex.len().div_ceil(PART_HEX_LENGTH);
    (0..parts).map(move |index| {
        let marker = if index + 1 == parts {
            LAST_PART
        } else {
            CONTINUED_PART
        };
        let end = ((index + 1) * PART_HEX_LENGTH).min(hex.len());
        format!("{marker}{}", &hex[index * PART_HEX_LENGTH..end])
    })
}

/// Gives the blob path of the file or directory at `physical`, which is below `root`, the
/// directory of its namespace: the names that the file names below `root` encode
/// (`encoded_name`), with `/` between two names.
///
/// A file name that `encoded_name` does not give, and a path that ends in a part that more parts
/// follow, give an error of the kind [`ErrorKind::InvalidData`].
fn blob_path_of(physical: &Path, root: &Path) -> std::io::Result<PathBuf> {
    let invalid = || {
        std::io::Error::new(
            ErrorKind::InvalidData,
            format!("the file {physical:?} is not a file of the blob storage"),
        )
    };
    let relative = physical.strip_prefix(root).map_err(std::io::Error::other)?;
    let (names, open_hex) = relative.components().try_fold(
        (String::new(), String::new()),
        |(mut names, mut hex), component| {
            let part = component.as_os_str().to_str().ok_or_else(invalid)?;
            if let Some(chunk) = part.strip_prefix(CONTINUED_PART) {
                hex.push_str(chunk);
                Ok((names, hex))
            } else if let Some(chunk) = part.strip_prefix(LAST_PART) {
                hex.push_str(chunk);
                let name = hex::decode(&hex)
                    .ok()
                    .and_then(|bytes| String::from_utf8(bytes).ok())
                    .ok_or_else(invalid)?;
                if !names.is_empty() {
                    names.push('/');
                }
                names.push_str(&name);
                Ok((names, String::new()))
            } else {
                Err(invalid())
            }
        },
    )?;
    if open_hex.is_empty() {
        Ok(PathBuf::from(names))
    } else {
        Err(invalid())
    }
}

#[cfg(test)]
mod tests;
