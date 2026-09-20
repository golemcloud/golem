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

use crate::replayable_stream::ErasedReplayableStream;
use anyhow::{Error, anyhow};
use async_trait::async_trait;
use bytes::Bytes;
use desert_rust::{BinaryDeserializer, BinarySerializer};
use futures::stream::BoxStream;
use golem_common::model::agent::AgentMode;
use golem_common::model::component::ComponentId;
use golem_common::model::environment::EnvironmentId;
use golem_common::model::{AgentId, Timestamp};
use golem_common::serialization::{deserialize, serialize};
use std::borrow::Cow;
use std::fmt::Debug;
use std::path::Component;
use std::path::{Path, PathBuf};

pub mod fs;
pub mod memory;
pub mod s3;
pub mod sqlite;

#[async_trait]
pub trait BlobStorage: Debug + Send + Sync {
    async fn get_raw(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> Result<Option<Vec<u8>>, Error>;

    async fn get_stream(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> Result<Option<BoxStream<'static, Result<Bytes, Error>>>, Error>;

    /// Reads the bytes from `start` to `end` of a blob. Both offsets are inclusive.
    ///
    /// The result has `end - start + 1` bytes. `None` means that no blob has the path. A range
    /// with a byte that is not in the blob gives an error that downcasts to [`BlobRangeError`]:
    /// an `end` at or after the length of the blob, a `start` after `end`, and each range of an
    /// empty blob. A `start` after `end` gives this error before the backend reads the blob.
    async fn get_raw_slice(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
        start: u64,
        end: u64,
    ) -> Result<Option<Vec<u8>>, Error> {
        if start > end {
            return Err(BlobRangeError { start, end }.into());
        }
        let data = self
            .get_raw(target_label, op_label, namespace, path)
            .await?;
        data.map(|data| {
            blob_range(&data, start, end)
                .map(<[u8]>::to_vec)
                .map_err(Error::from)
        })
        .transpose()
    }

    async fn get_metadata(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> Result<Option<BlobMetadata>, Error>;

    async fn put_raw(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
        data: &[u8],
    ) -> Result<(), Error>;

    async fn put_stream(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
        stream: &dyn ErasedReplayableStream<Item = Result<Vec<u8>, Error>, Error = Error>,
    ) -> Result<(), Error>;

    async fn delete(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> Result<(), Error>;

    async fn delete_many(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        paths: &[PathBuf],
    ) -> Result<(), Error> {
        for path in paths {
            self.delete(target_label, op_label, namespace.clone(), path)
                .await?;
        }
        Ok(())
    }

    /// Makes a directory at the path.
    ///
    /// A root path changes nothing and leaves no entry behind. A path is at the root when it has
    /// no name in it, for example an empty path or `.`. A second call on the same path changes
    /// nothing.
    async fn create_dir(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> Result<(), Error>;

    async fn list_dir(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> Result<Vec<PathBuf>, Error>;

    /// Lists each blob below a path, at all depths, with its size.
    ///
    /// Each path in the result is relative to the root of the namespace, as in `list_dir`. The
    /// result has no directories. An object that a backend writes to record a directory is not in
    /// the result. A path that does not exist, or the path of a blob, gives an empty result. Paths
    /// that differ only in case are different paths, unless the backend stores them as one blob.
    /// The order of the result is not specified.
    async fn list_blobs_below(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> Result<Box<[ListedBlob]>, Error>;

    /// Deletes the directory at the path and all the entries below it, at any depth.
    ///
    /// A root path changes nothing and returns false. A path is at the root when it has no
    /// name in it, for example an empty path or `.`. A directory that only holds blobs
    /// exists. Returns true if the path had a directory. Returns false if the path had
    /// nothing.
    async fn delete_dir(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> Result<bool, Error>;

    async fn exists(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> Result<ExistsResult, Error>;

    /// Writes the blob at `from` to `to`, and keeps the blob at `from`.
    ///
    /// A `from` with no blob at it gives an error that downcasts to [`BlobMissingError`], and
    /// writes nothing to `to`. One read gives that error, and it is permanent, so the operation
    /// does no more work.
    async fn copy(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        from: &Path,
        to: &Path,
    ) -> Result<(), Error> {
        match self
            .get_raw(target_label, op_label, namespace.clone(), from)
            .await?
        {
            Some(data) => {
                self.put_raw(target_label, op_label, namespace, to, &data)
                    .await
            }
            None => Err(BlobMissingError {
                path: from.to_path_buf(),
            }
            .into()),
        }
    }

    /// Writes the blob at `from` to `to`, and then deletes the blob at `from`.
    ///
    /// The copy comes before the delete, so each error of `copy` is an error of `move` and the
    /// blob at `from` stays. A `from` with no blob at it gives [`BlobMissingError`] from the
    /// default `copy`.
    async fn r#move(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        from: &Path,
        to: &Path,
    ) -> Result<(), Error> {
        self.copy(target_label, op_label, namespace.clone(), from, to)
            .await?;
        self.delete(target_label, op_label, namespace, from).await
    }
}

pub trait BlobStorageLabelledApi<S: BlobStorage + ?Sized> {
    fn with(&self, svc_name: &'static str, api_name: &'static str) -> LabelledBlobStorage<'_, S>;
}

impl<S: BlobStorage + ?Sized> BlobStorageLabelledApi<S> for S {
    fn with(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
    ) -> LabelledBlobStorage<'_, Self> {
        LabelledBlobStorage::new(svc_name, api_name, self)
    }
}

pub struct LabelledBlobStorage<'a, S: BlobStorage + ?Sized> {
    svc_name: &'static str,
    api_name: &'static str,
    storage: &'a S,
}

impl<'a, S: BlobStorage + ?Sized + Sync> LabelledBlobStorage<'a, S> {
    pub fn new(svc_name: &'static str, api_name: &'static str, storage: &'a S) -> Self {
        Self {
            svc_name,
            api_name,
            storage,
        }
    }

    pub async fn get_raw(
        &self,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> Result<Option<Vec<u8>>, Error> {
        self.storage
            .get_raw(self.svc_name, self.api_name, namespace, path)
            .await
    }

    pub async fn get_raw_slice(
        &self,
        namespace: BlobStorageNamespace,
        path: &Path,
        start: u64,
        end: u64,
    ) -> Result<Option<Vec<u8>>, Error> {
        self.storage
            .get_raw_slice(self.svc_name, self.api_name, namespace, path, start, end)
            .await
    }

    pub async fn get_metadata(
        &self,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> Result<Option<BlobMetadata>, Error> {
        self.storage
            .get_metadata(self.svc_name, self.api_name, namespace, path)
            .await
    }

    pub async fn put_raw(
        &self,
        namespace: BlobStorageNamespace,
        path: &Path,
        data: &[u8],
    ) -> Result<(), Error> {
        self.storage
            .put_raw(self.svc_name, self.api_name, namespace, path, data)
            .await
    }

    pub async fn delete(&self, namespace: BlobStorageNamespace, path: &Path) -> Result<(), Error> {
        self.storage
            .delete(self.svc_name, self.api_name, namespace, path)
            .await
    }

    pub async fn delete_many(
        &self,
        namespace: BlobStorageNamespace,
        paths: &[PathBuf],
    ) -> Result<(), Error> {
        self.storage
            .delete_many(self.svc_name, self.api_name, namespace, paths)
            .await
    }

    pub async fn create_dir(
        &self,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> Result<(), Error> {
        self.storage
            .create_dir(self.svc_name, self.api_name, namespace, path)
            .await
    }

    pub async fn list_dir(
        &self,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> Result<Vec<PathBuf>, Error> {
        self.storage
            .list_dir(self.svc_name, self.api_name, namespace, path)
            .await
    }

    pub async fn delete_dir(
        &self,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> Result<bool, Error> {
        self.storage
            .delete_dir(self.svc_name, self.api_name, namespace, path)
            .await
    }

    pub async fn exists(
        &self,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> Result<ExistsResult, Error> {
        self.storage
            .exists(self.svc_name, self.api_name, namespace, path)
            .await
    }

    pub async fn copy(
        &self,
        namespace: BlobStorageNamespace,
        from: &Path,
        to: &Path,
    ) -> Result<(), Error> {
        self.storage
            .copy(self.svc_name, self.api_name, namespace, from, to)
            .await
    }

    pub async fn r#move(
        &self,
        namespace: BlobStorageNamespace,
        from: &Path,
        to: &Path,
    ) -> Result<(), Error> {
        self.storage
            .r#move(self.svc_name, self.api_name, namespace, from, to)
            .await
    }

    pub async fn get<T: BinaryDeserializer>(
        &self,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> Result<Option<T>, Error> {
        match self.get_raw(namespace, path).await? {
            Some(data) => Ok(Some(deserialize(&data).map_err(|e| {
                anyhow!(e).context("Failed deserializing blob storage data")
            })?)),
            None => Ok(None),
        }
    }

    pub async fn put<T: BinarySerializer>(
        &self,
        namespace: BlobStorageNamespace,
        path: &Path,
        data: &T,
    ) -> Result<(), Error> {
        self.put_raw(
            namespace,
            path,
            &serialize(data)
                .map_err(|e| anyhow!(e).context("Failed serializing blob storage data"))?,
        )
        .await
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum BlobStorageNamespace {
    CompilationCache {
        environment_id: EnvironmentId,
    },
    InitialAgentFiles {
        environment_id: EnvironmentId,
    },
    CustomStorage {
        environment_id: EnvironmentId,
    },
    OplogPayload {
        environment_id: EnvironmentId,
        agent_id: AgentId,
        agent_mode: AgentMode,
    },
    CompressedOplog {
        environment_id: EnvironmentId,
        component_id: ComponentId,
        agent_mode: AgentMode,
        level: usize,
    },
    Components {
        environment_id: EnvironmentId,
    },
}

/// Returns the symmetric per-mode prefix used by all blob-storage backends for oplog data.
pub fn agent_mode_prefix(mode: AgentMode) -> &'static str {
    match mode {
        AgentMode::Durable => "durable",
        AgentMode::Ephemeral => "ephemeral",
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExistsResult {
    File,
    Directory,
    DoesNotExist,
}

#[derive(Debug, Clone)]
pub struct BlobMetadata {
    pub last_modified_at: Timestamp,
    pub size: u64,
}

/// The path and the size of one blob.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct ListedBlob {
    /// The path of the blob, relative to the root of its namespace.
    pub path: Box<Path>,
    /// The size of the blob in bytes.
    pub size: u64,
}

/// A ranged read asked for a byte that is not in the blob.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("the byte range {start}-{end} is not in the blob")]
pub struct BlobRangeError {
    /// The offset of the first byte of the range.
    pub start: u64,
    /// The offset of the last byte of the range.
    pub end: u64,
}

/// The storage has no blob at a path that an operation reads.
///
/// The name is good: the rules of [`BlobNameError`] accept it, and the backend can use it. The
/// storage holds no blob at it.
///
/// The default `copy` of [`BlobStorage`] reads the blob at its source path and gives this error
/// when the storage holds none there. The default `move` is that copy and then a delete of the
/// source, so it gives the error too, and it deletes nothing. A guest picks the source container
/// name and the source object name of `copy_object` and of `move_object`, so the path is of the
/// guest.
///
/// The error is permanent. `blob_store_error` in
/// `golem_worker_executor::services::blob_store` maps it to `BlobStoreError::NotFound`, and
/// `classify_blob_store_error` in `golem_worker_executor::durable_host::blobstore` makes that
/// permanent, so the guest gets the error at once and the executor does not retry it: a retry
/// cannot make the storage hold the blob.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("the blob storage has no blob at the path {path:?}")]
pub struct BlobMissingError {
    /// The path of the blob that the storage does not hold.
    pub path: PathBuf,
}

/// The error of a blob name that the storage cannot use.
///
/// A guest picks the name of a container and the name of an object, and
/// `golem_worker_executor::services::blob_store` makes the path of the blob of the two. A name
/// that breaks a rule gets this error before a backend reads or writes anything, so the name
/// costs no request and no retry.
///
/// The first group of rules is of the blob path. Each backend applies `NotRelative` and
/// `ParentDir` (`normalized_blob_path`), a backend that keeps the path as text applies
/// `NotUtf8` too (`blob_path_to_string`), and the in-memory and the SQLite backends apply
/// `NoName` (`blob_file_name_to_string`). The second group is of the object key of
/// the S3 backend, which applies it to the full key: the namespace prefix, the separators and
/// the name (`S3BlobStorage::key_of`). S3 and MinIO measure the full key. Each rule of the
/// second group is a rule of S3 or of MinIO, and one is the name that the backend keeps for
/// its own object.
///
/// The error is permanent, whichever rule it names and whichever backend gives it.
/// `blob_store_error` in `golem_worker_executor::services::blob_store` maps it to
/// `BlobStoreError::InvalidInput`, and `classify_blob_store_error` in
/// `golem_worker_executor::durable_host::blobstore` makes that permanent, so the guest gets
/// the error at once and the executor does not retry.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum BlobNameError {
    /// The blob path is not relative: it is absolute, or it has a root name or a drive prefix
    /// in it. Such a path does not name a blob below the root of its namespace.
    #[error("the blob path must be relative: {path:?}")]
    NotRelative { path: PathBuf },
    /// The blob path has a `..` name in it. Such a name goes up from the name before it, so
    /// the path can name a blob above the root of its namespace. A `..` name is a name of the
    /// path, which is `std::path::Component::ParentDir`.
    ///
    /// [`BlobNameError::DotSegment`] holds the neighbouring rule of the S3 backend, which
    /// reads an object key as MinIO reads it: at `\` as well as at `/`, and without the
    /// whitespace around a segment. A segment that is not a `..` name of the path, for example
    /// `" .. "`, gets that error and not this one.
    #[error("the blob path has a `..` name in it: {path:?}")]
    ParentDir { path: PathBuf },
    /// The blob path is not valid UTF-8. The in-memory, SQLite and S3 backends keep the path
    /// as text, so each of them reads the text of the path.
    #[error("the blob path must be valid UTF-8: {path:?}")]
    NotUtf8 { path: PathBuf },
    /// The blob path has no name in it, so it is at the root of its namespace
    /// (`blob_path_is_root`) and names no blob. An empty path has no name in it, and so does a
    /// path that only has `.` in it. A guest that gives an empty container name and an empty
    /// object name makes such a path.
    ///
    /// The in-memory and the SQLite backends hold a blob by the name of its directory and the
    /// name of the blob itself (`blob_file_name_to_string`), and a root path gives no such
    /// name, so each of them gives this error. The filesystem and the S3 backends give
    /// `Ok(None)` or an error of their own for such a path, and #3911 holds the decision of
    /// what a backend gives for a root path.
    #[error("the blob path has no name in it: {path:?}")]
    NoName { path: PathBuf },
    /// The object key has `length` bytes of UTF-8, which is more than `max`, the largest
    /// number of bytes that S3 accepts in an object key. S3 rejects such a key.
    #[error(
        "the object key of the blob name has {length} bytes of UTF-8, and S3 accepts at most {max}; the key holds the namespace prefix before the name"
    )]
    TooLong { length: usize, max: usize },
    /// The object key has a NUL byte. MinIO rejects such a key.
    #[error("the blob name has a NUL byte")]
    NulByte,
    /// The object key has a segment that is `.` or `..` without the whitespace around it.
    /// MinIO rejects such a key, and reads `\` as a separator like `/`. `segment` is the
    /// segment with its whitespace.
    #[error(
        "the blob name has the segment {segment:?}, which is `.` or `..` without the whitespace around it; `\\` is a separator like `/`"
    )]
    DotSegment { segment: String },
    /// The last segment of the object key is `marker`, the name of the object that the S3
    /// backend writes to record a directory. The blob listing leaves that name out, so a blob
    /// with that name would stay out of a snapshot.
    ///
    /// The rule applies to a directory name too, and a collision is the reason. `create_dir`
    /// of `x/__dir_marker` writes its marker object at the key `x/__dir_marker/__dir_marker`,
    /// while `exists` of `x/__dir_marker` sends a HEAD for the key `x/__dir_marker`, which is
    /// the marker object of the directory `x`. `exists` would give `File` for a directory
    /// that the guest had just made, and `get_metadata` would give the size of the marker
    /// object of `x`. The rule keeps that one key for the backend, so the collision cannot
    /// happen.
    #[error(
        "the last segment of the blob name is {marker}, which the S3 backend keeps for the object that records a directory"
    )]
    Reserved { marker: &'static str },
}

/// Gives the bytes from `start` to `end` of `blob`, which holds the full blob. Both offsets are
/// inclusive.
///
/// A range with a byte that is not in the blob gives a [`BlobRangeError`]. An `end` at or after
/// the length of the blob is not in the blob. A `start` after `end` is not in the blob. No
/// range is in an empty blob.
pub(crate) fn blob_range(blob: &[u8], start: u64, end: u64) -> Result<&[u8], BlobRangeError> {
    (start <= end)
        .then(|| usize::try_from(start).ok().zip(usize::try_from(end).ok()))
        .flatten()
        .and_then(|(first, last)| blob.get(first..=last))
        .ok_or(BlobRangeError { start, end })
}

/// Gives the one form of a relative blob path, or an error.
///
/// The form holds the names of the path and one separator between two names. A `.` and an extra
/// separator are not names, so they go away, and a path at the root of a namespace becomes the
/// empty path. Two paths that name the same blob get the same form. An absolute path, a path
/// with `..` in it, and a path with a drive letter give a [`BlobNameError`], which is
/// permanent.
pub(crate) fn normalized_blob_path(path: &Path) -> Result<Cow<'_, Path>, BlobNameError> {
    if path.is_absolute() {
        return Err(BlobNameError::NotRelative {
            path: path.to_path_buf(),
        });
    }

    let mut names_length = 0usize;
    let mut names_count = 0usize;
    for component in path.components() {
        match component {
            Component::Normal(name) => {
                names_length += name.len();
                names_count += 1;
            }
            Component::CurDir => {}
            Component::ParentDir => {
                return Err(BlobNameError::ParentDir {
                    path: path.to_path_buf(),
                });
            }
            Component::RootDir | Component::Prefix(_) => {
                return Err(BlobNameError::NotRelative {
                    path: path.to_path_buf(),
                });
            }
        }
    }

    // The path is already in its one form when its length is exactly its names plus the one
    // separator that sits between two names, so nothing has to be built.
    if names_length + names_count.saturating_sub(1) == path.as_os_str().len() {
        return Ok(Cow::Borrowed(path));
    }

    Ok(Cow::Owned(
        path.components()
            .filter(|component| matches!(component, Component::Normal(_)))
            .collect(),
    ))
}

/// Tells if the path is at the root of a namespace.
///
/// A path is at the root when it has no name in it. An empty path is at the root, and so is a
/// path that only has `.` in it.
pub(crate) fn blob_path_is_root(path: &Path) -> bool {
    !path
        .components()
        .any(|component| matches!(component, Component::Normal(_)))
}

/// Gives the text of the path, or a [`BlobNameError`], which is permanent.
pub(crate) fn blob_path_to_string(path: &Path) -> Result<String, BlobNameError> {
    path.to_str()
        .map(|s| s.to_string())
        .ok_or_else(|| BlobNameError::NotUtf8 {
            path: path.to_path_buf(),
        })
}

pub(crate) fn blob_parent_to_string(path: &Path) -> Result<String, BlobNameError> {
    match path.parent() {
        Some(parent) => blob_path_to_string(parent),
        None => Ok(String::new()),
    }
}

/// Makes the path of a blob from the path of its directory and its name.
///
/// An empty directory path is the root of the namespace. The path is made in one allocation of
/// its final size.
pub(crate) fn blob_child_path(directory: &str, name: &str) -> Box<Path> {
    let separator = if directory.is_empty() { "" } else { "/" };
    let mut path = String::with_capacity(directory.len() + separator.len() + name.len());
    path.push_str(directory);
    path.push_str(separator);
    path.push_str(name);
    PathBuf::from(path).into_boxed_path()
}

/// Gives the text of the last name of the path.
///
/// A path that is not valid UTF-8 gives the same [`BlobNameError`] as `blob_path_to_string`. A
/// path with no name in it is at the root of its namespace (`blob_path_is_root`) and gives
/// [`BlobNameError::NoName`]: a guest can pick two empty names, so the path is of the guest and
/// so is the error. The two errors are permanent.
pub(crate) fn blob_file_name_to_string(path: &Path) -> Result<String, BlobNameError> {
    path.file_name()
        .ok_or_else(|| BlobNameError::NoName {
            path: path.to_path_buf(),
        })
        .and_then(|name| {
            name.to_str()
                .map(|s| s.to_string())
                .ok_or_else(|| BlobNameError::NotUtf8 {
                    path: path.to_path_buf(),
                })
        })
}

#[cfg(test)]
mod tests {
    use super::{
        BlobNameError, BlobRangeError, blob_file_name_to_string, blob_path_to_string, blob_range,
        normalized_blob_path,
    };
    use pretty_assertions::assert_eq;
    use std::path::{Path, PathBuf};
    use test_r::test;

    #[test]
    fn blob_range_gives_the_inclusive_range_or_a_range_error() {
        let blob = b"abcdef";
        let ranges = [
            (1, 3),
            (0, 5),
            (5, 5),
            (0, 6),
            (6, 6),
            (3, 2),
            (u64::MAX, u64::MAX),
        ];

        let results = ranges.map(|(start, end)| blob_range(blob, start, end));

        assert_eq!(
            results,
            [
                Ok(&b"bcd"[..]),
                Ok(&b"abcdef"[..]),
                Ok(&b"f"[..]),
                Err(BlobRangeError { start: 0, end: 6 }),
                Err(BlobRangeError { start: 6, end: 6 }),
                Err(BlobRangeError { start: 3, end: 2 }),
                Err(BlobRangeError {
                    start: u64::MAX,
                    end: u64::MAX
                }),
            ]
        );
        assert_eq!(
            blob_range(b"", 0, 0),
            Err(BlobRangeError { start: 0, end: 0 })
        );
    }

    #[test]
    fn normalized_blob_path_gives_the_one_form_or_the_rule_that_the_name_breaks() {
        let paths = ["", ".", "a", "./a//b/", "/escape", "../escape", "a/../b"];

        let results =
            paths.map(|path| normalized_blob_path(Path::new(path)).map(|path| path.into_owned()));

        assert_eq!(
            results,
            [
                Ok(PathBuf::from("")),
                Ok(PathBuf::from("")),
                Ok(PathBuf::from("a")),
                Ok(PathBuf::from("a/b")),
                Err(BlobNameError::NotRelative {
                    path: PathBuf::from("/escape")
                }),
                Err(BlobNameError::ParentDir {
                    path: PathBuf::from("../escape")
                }),
                Err(BlobNameError::ParentDir {
                    path: PathBuf::from("a/../b")
                }),
            ]
        );
    }

    /// The guest gives its names as text, so only a path of another source can break this
    /// rule. Each function that reads the text of a path gives the one error for it.
    #[cfg(unix)]
    #[test]
    fn a_path_that_is_not_utf8_gives_the_utf8_rule() {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt;

        let path = Path::new(OsStr::from_bytes(b"a/\xff"));
        let expected = BlobNameError::NotUtf8 {
            path: path.to_path_buf(),
        };

        assert_eq!(
            (blob_path_to_string(path), blob_file_name_to_string(path)),
            (Err(expected.clone()), Err(expected))
        );
    }

    /// A guest picks the name of its container and the name of its object, so it can give two
    /// names that make a path with no name in it. The in-memory and the SQLite backends read
    /// the last name of the path, and the rule is of the name, so the error is permanent.
    #[test]
    fn a_path_with_no_name_in_it_gives_the_name_rule() {
        let paths = ["", "."];

        let results = paths.map(|path| blob_file_name_to_string(Path::new(path)));

        assert_eq!(
            results,
            [
                Err(BlobNameError::NoName {
                    path: PathBuf::from("")
                }),
                Err(BlobNameError::NoName {
                    path: PathBuf::from(".")
                }),
            ]
        );
    }

    #[test]
    fn blob_path_to_string_gives_the_text_of_the_path() {
        assert_eq!(blob_path_to_string(Path::new("a/b")), Ok("a/b".to_string()));
    }
}
