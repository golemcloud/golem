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
use std::fmt::Debug;
use std::ops::RangeInclusive;
use std::path::{Path, PathBuf};

pub mod fs;
pub mod memory;
pub mod s3;
pub mod sqlite;

pub(crate) use normalized_path::{NormalizedBlobPath, normalized_blob_path};

/// Keeps blobs at the paths of a namespace.
///
/// A path names a blob or a directory, and a directory is not a blob. So a read at the path of a
/// directory finds no blob, a delete at that path removes no blob, and a write at that path is an
/// error. A path is at the root of the namespace when it has no name in it, for example an empty
/// path or `.`, and the root is a directory. A directory is there while a blob is below it, at
/// any depth, and a directory that `create_dir` made is there until `delete_dir` removes it. A
/// directory that `create_dir` made keeps a size of zero and a time, which `get_metadata` gives.
#[async_trait]
pub trait BlobStorage: Debug + Send + Sync {
    /// Gives the bytes of the blob at the path, or nothing if the path has no blob.
    ///
    /// A directory has no blob at its path, and a root path is a directory.
    async fn get_raw(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> Result<Option<Vec<u8>>, Error>;

    /// Gives the bytes of the blob at the path as a stream, or nothing if the path has no blob.
    ///
    /// A directory has no blob at its path, and a root path is a directory.
    async fn get_stream(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> Result<Option<BoxStream<'static, Result<Bytes, Error>>>, Error>;

    /// Reads the bytes from `start` to `end` of a blob. Both offsets are inclusive.
    ///
    /// The result has `end - start + 1` bytes. `None` means that no blob has the path. A
    /// directory has no blob at its path, and a root path is a directory. A range with a byte
    /// that is not in the blob gives an error that downcasts to [`BlobRangeError`]: an `end` at
    /// or after the length of the blob, a `start` after `end`, and each range of an empty blob.
    /// A `start` after `end` gives this error before the backend reads the blob.
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

    /// Tells the size and the time of the blob at the path, or nothing if the path has no blob.
    ///
    /// A directory that `create_dir` made is the one path without a blob that has metadata: it
    /// gives a size of zero and the time of the last `create_dir`. A directory that only holds
    /// blobs gives nothing, and so does a root path.
    async fn get_metadata(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> Result<Option<BlobMetadata>, Error>;

    /// Writes the bytes as the blob at the path, over the blob that was there.
    ///
    /// A blob cannot be where a directory is, so a root path is an error.
    async fn put_raw(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
        data: &[u8],
    ) -> Result<(), Error>;

    /// Writes the bytes as the blob at the path when the path has no blob.
    ///
    /// When the path has no blob, the call writes the blob and gives [`PutIfAbsent::Written`]. When
    /// the path has a blob, the call writes nothing and gives [`PutIfAbsent::AlreadyExists`]. The
    /// check and the write are one step. So when two calls write one path at the same time, one
    /// call gives `Written` and the other gives `AlreadyExists`. The rules of [`BlobNameError`]
    /// apply as for `put_raw`, and a root path gives [`BlobNameError::NoName`] on every backend.
    ///
    /// The S3 backend sends the request again after an error that one more attempt can pass, as
    /// `put_raw` does. When the response to an attempt that wrote the blob does not arrive, the
    /// next attempt finds that blob. The call then gives `AlreadyExists`, although the call
    /// wrote the blob.
    async fn put_raw_if_absent(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
        data: &[u8],
    ) -> Result<PutIfAbsent, Error>;

    /// Writes the bytes of the stream as the blob at the path, over the blob that was there.
    ///
    /// A blob cannot be where a directory is, so a root path is an error.
    async fn put_stream(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
        stream: &dyn ErasedReplayableStream<Item = Result<Vec<u8>, Error>, Error = Error>,
    ) -> Result<(), Error>;

    /// Removes the blob at the path.
    ///
    /// A path that has no blob changes nothing. A directory has no blob at its path, and a root
    /// path is a directory.
    async fn delete(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> Result<(), Error>;

    /// Removes the blob at every one of the paths.
    ///
    /// A path that has no blob changes nothing. A directory has no blob at its path, and a root
    /// path is a directory.
    ///
    /// The backend reads every path before it removes the first blob, so a path that breaks a
    /// rule of a name gives a [`BlobNameError`] and the call removes no blob at all. The rule
    /// holds for the names and for nothing else: an error of the backend part way through the
    /// paths leaves the blobs that the backend removed before that error removed, and the S3
    /// backend sends the keys in more than one request when they do not fit in one, so the
    /// removal is not one operation.
    async fn delete_many(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        paths: &[PathBuf],
    ) -> Result<(), Error> {
        for path in paths {
            normalized_blob_path(path)?;
        }

        for path in paths {
            self.delete(target_label, op_label, namespace.clone(), path)
                .await?;
        }
        Ok(())
    }

    /// Makes a directory at the path.
    ///
    /// A root path changes nothing and leaves no entry behind. A path is at the root when it has
    /// no name in it, for example an empty path or `.`. A second call on the same path adds
    /// nothing and removes nothing, and it gives the directory the time of that call.
    async fn create_dir(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> Result<(), Error>;

    /// Lists the blobs that are directly below the path, and each directory that `create_dir`
    /// made below the path, at any depth.
    ///
    /// Each path in the result is relative to the root of the namespace. A directory that
    /// `create_dir` made is in the result at its own path, so a directory that sits two names
    /// below the path is in the result with both names. A directory that only holds blobs is
    /// not in the result, because the storage keeps no entry for it, and a blob that is not
    /// directly below the path is not in it either. Each path is in the result one time, also
    /// when a blob and a directory hold that path.
    ///
    /// Returns an empty list if the path holds nothing. A path that has nothing at it holds
    /// nothing, and so does the root of a namespace that has nothing in it.
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

    /// Tells what the path has.
    ///
    /// Returns `Directory` for a root path, whatever the namespace holds. A path is at the root
    /// when it has no name in it, for example an empty path or `.`. Returns `Directory` for a
    /// path that has blobs below it, at any depth, also when the storage keeps no entry for
    /// that directory. Returns `File` for a path that has a blob. Returns `DoesNotExist` for
    /// every other path.
    async fn exists(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> Result<ExistsResult, Error>;

    /// Writes the blob at the `from` path as the blob at the `to` path, and keeps the blob at
    /// `from`.
    ///
    /// A copy onto the same path writes nothing and changes nothing, and two forms of one path
    /// are the same path. A blob cannot be where a directory is, so a root path at either end
    /// gives [`BlobNameError::NoName`]. A `from` path with no blob at it gives an error that
    /// downcasts to [`BlobMissingError`], the copy onto the same path as well, and writes
    /// nothing to `to`. One read gives that error, and it is permanent, so the operation does no
    /// more work.
    async fn copy(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        from: &Path,
        to: &Path,
    ) -> Result<(), Error> {
        // A copy onto the same path writes nothing, and it still needs the blob that it reads.
        if blob_copy_changes_nothing(&normalized_blob_path(from)?, &normalized_blob_path(to)?)? {
            return match self.exists(target_label, op_label, namespace, from).await? {
                ExistsResult::File => Ok(()),
                _ => Err(BlobMissingError {
                    path: from.to_path_buf(),
                }
                .into()),
            };
        }

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

    /// Writes the blob at the `from` path as the blob at the `to` path, and then deletes the
    /// blob at `from`.
    ///
    /// A move onto the same path keeps the blob where it is, and two forms of one path are the
    /// same path. A blob cannot be where a directory is, so a root path at either end gives
    /// [`BlobNameError::NoName`]. The copy comes before the delete, so each error of `copy` is
    /// an error of `move` and the blob at `from` stays: a `from` path with no blob at it gives
    /// [`BlobMissingError`] from the default `copy`, the move onto the same path as well.
    async fn r#move(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        from: &Path,
        to: &Path,
    ) -> Result<(), Error> {
        // A move onto the same path keeps the blob, so it is the copy and no delete.
        if blob_copy_changes_nothing(&normalized_blob_path(from)?, &normalized_blob_path(to)?)? {
            return self.copy(target_label, op_label, namespace, from, to).await;
        }

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

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
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
    /// The filesystem snapshots of one agent. Each agent has its own location on each backend.
    FilesystemSnapshots {
        environment_id: EnvironmentId,
        agent_id: AgentId,
    },
}

/// What [`BlobStorage::put_raw_if_absent`] did.
#[must_use]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PutIfAbsent {
    /// The path had no blob, and the call wrote the blob.
    Written,
    /// The path had a blob, and the call wrote nothing.
    AlreadyExists,
}

/// Gives one path segment for an agent, which a backend can use as a directory name.
///
/// The segment is the agent name with each character that is not an ASCII letter, a digit, `-` or
/// `_` replaced by `_`. The name is cut to 32 characters, and an empty name gives `agent`. Then
/// come `-` and the blake3 hash of the full agent id, which holds the component id and the agent
/// name. So the segment has at most 97 bytes, and it holds no separator and no `.` segment. The
/// hash makes it very unlikely that two agents get the same segment. An agent name can be longer
/// than a file name, and it can hold `/`, `\` and `.` segments. So a backend does not use the agent
/// name itself.
pub fn agent_path_segment(agent_id: &AgentId) -> String {
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
/// when the storage holds none there. The S3 backend has a `copy` of its own, which sends one
/// `CopyObject` request and gives this error for the code `NoSuchKey` of the source key. The
/// default `move` is a copy and then a delete of the source, so it gives the error too, and it
/// deletes nothing. A guest picks the source container name and the source object name of
/// `copy_object` and of `move_object`, so the path is of the guest. Each backend names the
/// path as the guest wrote it, and not in the normalized form that the storage uses. Each
/// [`BlobNameError`] does the same, because the guest reads the message, except
/// [`BlobNameError::NoName`], which names the one form of the path. The one form of a path
/// with no name in it is the empty path, and each spelling of such a path says the same thing
/// to the guest.
///
/// The error is permanent. `blob_store_error` in
/// `golem_worker_executor::services::blob_store` maps it to `BlobStoreError::NotFound`, and
/// `classify_blob_store_error` in `golem_worker_executor::durable_host::blobstore` makes that
/// permanent, so the guest gets the error at once and the executor does not retry it: a retry
/// cannot make the storage hold the blob.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("the blob storage has no blob at the path {path:?}")]
pub struct BlobMissingError {
    /// The path of the blob that the storage does not hold, as the guest wrote it.
    pub path: PathBuf,
}

/// The name of the object that the S3 backend writes to record a directory, because S3 has no
/// directories.
///
/// [`BlobNameError::Reserved`] keeps the name for that object, and `check_blob_name` gives that
/// error for a name whose last segment is this one. Each backend applies the rule, so a name
/// that the in-memory backend accepts is a name that the S3 backend accepts.
pub(crate) const DIR_MARKER: &str = "__dir_marker";

/// The error of a blob name that the storage cannot use.
///
/// A guest picks the name of a container and the name of an object, and
/// `golem_worker_executor::services::blob_store` makes the path of the blob of the two. A name
/// that breaks a rule gets this error before a backend reads or writes anything, so the name
/// costs no request and no retry.
///
/// The first group of rules is of the blob path, and `normalized_blob_path` applies each of
/// them. Every backend calls that function for every path that it gets, so every backend gives
/// the same answer for a name: `NotRelative` and `ParentDir` for a path that leaves the
/// namespace, `NotUtf8` for a path that the one form cannot hold as text, and then `NulByte`,
/// `DotSegment` and `Reserved` for the text of that form (`check_blob_name`). The three rules
/// of the text are rules of S3 or of MinIO, and one is the name that the S3 backend keeps for
/// its own object.
///
/// Each operation that writes a blob applies `NoName` as well: the write operations of the
/// in-memory and the S3 backends apply it to the path of the blob
/// (`NormalizedBlobPath::reject_root`), `copy` and `move` apply it to both of their paths
/// (`blob_copy_changes_nothing`), and the in-memory and the SQLite backends apply it to each
/// path whose last name they read (`NormalizedBlobPath::file_name_text`).
///
/// The second group is of the object key of the S3 backend, which applies it to the full key:
/// the namespace prefix, the separators and the name (`S3BlobStorage::key_of`). `TooLong` is
/// the one rule of that group, because S3 and MinIO measure the full key and only that backend
/// builds it. The key gets the three rules of the text a second time there, so a namespace
/// prefix of the configuration gets them too.
///
/// The error is permanent, whichever rule it names and whichever backend gives it.
/// `blob_store_error` in `golem_worker_executor::services::blob_store` maps it to
/// `BlobStoreError::InvalidInput`, and `classify_blob_store_error` in
/// `golem_worker_executor::durable_host::blobstore` makes that permanent, so the guest gets
/// the error at once and the executor does not retry.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum BlobNameError {
    /// The blob path is not relative: it is absolute, it has a root name in it, or it starts
    /// with a prefix of Windows (`blob_path_starts_with_windows_prefix`). Such a path does not
    /// name a blob below the root of its namespace.
    ///
    /// A prefix of Windows names a drive or a server there, and it is a name of the path on
    /// unix, so the rule that catches it reads the text of the path. One error holds both
    /// rules because both say the same thing about the path: it does not stay below the root
    /// of the namespace, wherever the process that holds the storage runs.
    #[error("the blob path must be relative: {path:?}")]
    NotRelative { path: PathBuf },
    /// The blob path has a `..` name in it. Such a name goes up from the name before it, so
    /// the path can name a blob above the root of its namespace. A `..` name is a name of the
    /// path, which is `std::path::Component::ParentDir`.
    ///
    /// [`BlobNameError::DotSegment`] holds the neighbouring rule, which reads a name as MinIO
    /// reads an object key: at `\` as well as at `/`, and without the whitespace around a
    /// segment. A segment that is not a `..` name of the path, for example `" .. "`, gets that
    /// error and not this one.
    #[error("the blob path has a `..` name in it: {path:?}")]
    ParentDir { path: PathBuf },
    /// The blob path is not valid UTF-8. The one form of the path is text
    /// (`normalized_blob_path`), and the in-memory, SQLite and S3 backends keep it as text.
    #[error("the blob path must be valid UTF-8: {path:?}")]
    NotUtf8 { path: PathBuf },
    /// The blob path has no name in it, so it is at the root of its namespace
    /// (`NormalizedBlobPath::is_root`) and names no blob. An empty path has no name in it, and
    /// so does a path that only has `.` in it. A guest that gives an empty container name and
    /// an empty object name makes such a path.
    ///
    /// A root path is a directory, and a blob cannot be where a directory is, so an operation
    /// that writes a blob at such a path gives this error: `put_raw` and `put_stream` of the
    /// in-memory and the S3 backends (`NormalizedBlobPath::reject_root`), and `copy` and `move`
    /// of each backend, at either of their two paths (`blob_copy_changes_nothing`). An
    /// operation that reads a blob gives `Ok(None)` for a root path, and `exists` gives
    /// `Directory`.
    ///
    /// The in-memory and the SQLite backends hold a blob by the name of its directory and the
    /// name of the blob itself (`NormalizedBlobPath::file_name_text`), and a root path gives no
    /// such name, so each of them gives this error for a root path that reaches that
    /// function.
    #[error("the blob path has no name in it: {path:?}")]
    NoName { path: PathBuf },
    /// The object key has `length` bytes of UTF-8, which is more than `max`, the largest
    /// number of bytes that S3 accepts in an object key. S3 rejects such a key.
    #[error(
        "the object key of the blob name has {length} bytes of UTF-8, and S3 accepts at most {max}; the key holds the namespace prefix before the name"
    )]
    TooLong { length: usize, max: usize },
    /// The blob name has a NUL byte. MinIO rejects an object key with such a byte, and the
    /// filesystem backend cannot write a name with it either.
    #[error("the blob name has a NUL byte")]
    NulByte,
    /// The blob name has a segment that is `.` or `..` without the whitespace around it.
    /// MinIO rejects such an object key, and reads `\` as a separator like `/`. `segment` is
    /// the segment with its whitespace.
    #[error(
        "the blob name has the segment {segment:?}, which is `.` or `..` without the whitespace around it; `\\` is a separator like `/`"
    )]
    DotSegment { segment: String },
    /// The last segment of the blob name is `marker`, the name of the object that the S3
    /// backend writes to record a directory ([`DIR_MARKER`]). The blob listing leaves that
    /// name out, so a blob with that name would stay out of a snapshot.
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
    blob_positions(blob.len(), start, end).map(|positions| &blob[positions])
}

/// Gives the positions of the bytes from `start` to `end` in a blob of `length` bytes. Both
/// offsets are inclusive, and the rules of [`blob_range`] apply.
pub(crate) fn blob_positions(
    length: usize,
    start: u64,
    end: u64,
) -> Result<RangeInclusive<usize>, BlobRangeError> {
    (start <= end)
        .then(|| usize::try_from(start).ok().zip(usize::try_from(end).ok()))
        .flatten()
        .filter(|&(_, last)| last < length)
        .map(|(first, last)| first..=last)
        .ok_or(BlobRangeError { start, end })
}

/// Applies the rules of [`BlobNameError`] that read the text of a name, in this order: the NUL
/// byte, then the `.` or `..` segment, then the reserved last segment.
///
/// `normalized_blob_path` applies them to the one form of each blob path, so every backend
/// applies them. The S3 backend applies them a second time to the full object key
/// (`S3BlobStorage::checked_key`), which holds the namespace prefix before the name.
///
/// The name is split at `\` as well as at `/`, and the whitespace around a segment goes away
/// before the segment is read, because that is how MinIO reads an object key. The reserved
/// segment is the last segment at `/` only: [`DIR_MARKER`] is reserved because the S3 backend
/// writes the object that records a directory at the key of the directory, a `/`, and that
/// name.
pub(crate) fn check_blob_name(name: &str) -> Result<(), BlobNameError> {
    if name.contains('\0') {
        return Err(BlobNameError::NulByte);
    }
    if let Some(segment) = name
        .split(['/', '\\'])
        .find(|segment| matches!(segment.trim(), "." | ".."))
    {
        return Err(BlobNameError::DotSegment {
            segment: segment.to_string(),
        });
    }
    if name.rsplit('/').next() == Some(DIR_MARKER) {
        return Err(BlobNameError::Reserved { marker: DIR_MARKER });
    }
    Ok(())
}

/// Tells if the text of a path starts with a prefix of Windows: one ASCII letter and a `:`,
/// which names a drive, or `\\`, which starts the name of a server, of a device or of a
/// verbatim path.
///
/// Windows reads such a prefix as the start of a path that its own root holds, and
/// `Path::components` gives it there as [`Component::Prefix`]. Unix has no such prefix, so
/// `Path::components` gives the same text as a name of the path and nothing else refuses it.
/// The rule reads the text, so the host that runs the process does not change the answer.
///
/// The rule reads one letter and a `:` because that is what Windows reads: `C:x` names the
/// place that the current directory of the drive `C` holds. A `:` after more than one letter,
/// or somewhere else in the path, is a character of a name.
pub(crate) fn blob_path_starts_with_windows_prefix(text: &str) -> bool {
    let bytes = text.as_bytes();
    text.starts_with(r"\\") || matches!(bytes, [letter, b':', ..] if letter.is_ascii_alphabetic())
}

/// Holds the one form of a blob path and the one function that makes it.
///
/// The field of [`NormalizedBlobPath`] is private to this module, and no backend is in it, so
/// `normalized_blob_path` is the one way to make the type.
mod normalized_path {
    use super::{
        BlobNameError, blob_path_starts_with_windows_prefix, blob_path_to_string, check_blob_name,
    };
    use std::borrow::Cow;
    use std::ops::Deref;
    use std::path::{Component, Path};

    /// The one form of a relative blob path (`normalized_blob_path`).
    ///
    /// Every backend gets the path of a caller, makes this form of it, and stores that form.
    /// The functions that make a key of a path take this type and nothing else, so a path
    /// that has not been through `normalized_blob_path` cannot reach them and no comment has
    /// to say that it must not.
    ///
    /// The form borrows the path of the caller when that path is already in its one form, so
    /// this form of such a path allocates nothing. The operation that follows still builds the
    /// key of its backend from the form.
    ///
    /// The type gives the path itself to a caller that reads it, and that caller has a
    /// `&Path` (`Deref`). A caller that makes a key has to name the type, and the four
    /// backends do: `S3BlobStorage::key_of`, `FileSystemBlobStorage::path_of`,
    /// `InMemoryBlobStorage::blob_key`, and the parts of the key that the in-memory and the
    /// SQLite backends bind.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub(crate) struct NormalizedBlobPath<'a>(Cow<'a, Path>);

    impl NormalizedBlobPath<'static> {
        /// Gives the path at the root of a namespace, which has no name in it.
        pub(crate) fn root() -> Self {
            Self(Cow::Borrowed(Path::new("")))
        }
    }

    impl NormalizedBlobPath<'_> {
        /// Tells if the path is at the root of a namespace.
        ///
        /// A path is at the root when it has no name in it. An empty path is at the root,
        /// and so is a path that only has `.` in it, because the one form keeps no `.`.
        pub(crate) fn is_root(&self) -> bool {
            !self
                .0
                .components()
                .any(|component| matches!(component, Component::Normal(_)))
        }

        /// Gives [`BlobNameError::NoName`] if the path is at the root of a namespace.
        ///
        /// A path at the root is a directory, and a blob cannot be where a directory is. A
        /// guest can give an empty container name and an empty object name, so the path is of
        /// the guest and the error is permanent.
        pub(crate) fn reject_root(&self) -> Result<(), BlobNameError> {
            if self.is_root() {
                Err(BlobNameError::NoName {
                    path: self.0.to_path_buf(),
                })
            } else {
                Ok(())
            }
        }

        /// Gives the text of the path, which is the key that a backend stores.
        pub(crate) fn text(&self) -> Result<String, BlobNameError> {
            blob_path_to_string(&self.0)
        }

        /// Gives the text of the path of the directory that holds the blob at this path.
        ///
        /// The root of the namespace is the empty text.
        pub(crate) fn parent_text(&self) -> Result<String, BlobNameError> {
            match self.0.parent() {
                Some(parent) => blob_path_to_string(parent),
                None => Ok(String::new()),
            }
        }

        /// Gives the text of the last name of the path.
        ///
        /// A path that is not valid UTF-8 gives the same [`BlobNameError`] as
        /// `blob_path_to_string`, which `normalized_blob_path` has already refused. A path
        /// with no name in it is at the root of its namespace (`is_root`) and gives
        /// [`BlobNameError::NoName`]: a guest can pick two empty names, so the path is of the
        /// guest and so is the error. The two errors are permanent.
        pub(crate) fn file_name_text(&self) -> Result<String, BlobNameError> {
            self.0
                .file_name()
                .ok_or_else(|| BlobNameError::NoName {
                    path: self.0.to_path_buf(),
                })
                .and_then(|name| {
                    name.to_str().map(|name| name.to_string()).ok_or_else(|| {
                        BlobNameError::NotUtf8 {
                            path: self.0.to_path_buf(),
                        }
                    })
                })
        }
    }

    impl Deref for NormalizedBlobPath<'_> {
        type Target = Path;

        fn deref(&self) -> &Path {
            &self.0
        }
    }

    impl AsRef<Path> for NormalizedBlobPath<'_> {
        fn as_ref(&self) -> &Path {
            &self.0
        }
    }

    /// Gives the one form of a relative blob path, or an error.
    ///
    /// The form holds the names of the path and one separator between two names. A `.` and
    /// an extra separator are not names, so they go away, and a path at the root of a
    /// namespace becomes the empty path. Two paths that name the same blob get the same form.
    /// An absolute path, a path with `..` in it, and a path with a prefix of Windows give a
    /// [`BlobNameError`], which is permanent.
    ///
    /// The form is then read as text, so a path that is not valid UTF-8 gives
    /// [`BlobNameError::NotUtf8`]. `blob_path_starts_with_windows_prefix` gives
    /// [`BlobNameError::NotRelative`] for a path that Windows holds outside the namespace,
    /// and `check_blob_name` gives the rule that the text breaks. The text is what a backend
    /// stores, and the rules read `\` as a separator, which the names of the path do not
    /// (`Path::components` reads `\` as a name on unix). Every backend calls this function
    /// for every path that it gets, so every backend gives the same error for the same name,
    /// on every host.
    pub(crate) fn normalized_blob_path(
        path: &Path,
    ) -> Result<NormalizedBlobPath<'_>, BlobNameError> {
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
        let normalized = if names_length + names_count.saturating_sub(1) == path.as_os_str().len() {
            Cow::Borrowed(path)
        } else {
            Cow::Owned(
                path.components()
                    .filter(|component| matches!(component, Component::Normal(_)))
                    .collect(),
            )
        };

        // The error names the path as the caller gave it, because the guest reads the message.
        let text = normalized.to_str().ok_or_else(|| BlobNameError::NotUtf8 {
            path: path.to_path_buf(),
        })?;
        // The rule holds for the one form, so a path whose one form starts with a prefix of
        // Windows gets the error as well, and the one form of an accepted path is accepted.
        if blob_path_starts_with_windows_prefix(text) {
            return Err(BlobNameError::NotRelative {
                path: path.to_path_buf(),
            });
        }
        check_blob_name(text)?;

        Ok(NormalizedBlobPath(normalized))
    }
}

/// Tells if a copy from one path to the other changes nothing, or gives a [`BlobNameError`].
///
/// A copy onto the same path changes nothing, because the blob is already there. Both paths are
/// in their one form, so two forms of one path are the same path. A root path at either end
/// gives [`BlobNameError::NoName`], because a blob cannot be where a directory is.
pub(crate) fn blob_copy_changes_nothing(
    from: &NormalizedBlobPath,
    to: &NormalizedBlobPath,
) -> Result<bool, BlobNameError> {
    from.reject_root()?;
    to.reject_root()?;

    Ok(from == to)
}

/// Tells if the path names the root of its namespace.
///
/// A path names the root when it has no name in it: an empty path, and a path that only has `.`
/// in it, because a `.` is not a name (`NormalizedBlobPath::is_root`). The root is a directory,
/// so it names no blob.
///
/// A path that breaks a rule of a name does not name the root, whatever else it holds: a `..`
/// path and an absolute path give `false` here, and the backend that reads such a path gives the
/// rule that it breaks.
///
/// `golem_worker_executor::services::blob_store` reads this for the container name that a guest
/// gives, because a name that names the root names the namespace and not a container.
pub fn blob_path_is_root(path: &Path) -> bool {
    normalized_blob_path(path).is_ok_and(|path| path.is_root())
}

/// Gives the text of the path, or a [`BlobNameError`], which is permanent.
pub(crate) fn blob_path_to_string(path: &Path) -> Result<String, BlobNameError> {
    path.to_str()
        .map(|s| s.to_string())
        .ok_or_else(|| BlobNameError::NotUtf8 {
            path: path.to_path_buf(),
        })
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

#[cfg(test)]
mod tests {
    use super::{
        BlobNameError, BlobRangeError, agent_path_segment, blob_path_to_string, blob_range,
        normalized_blob_path,
    };
    use golem_common::model::AgentId;
    use golem_common::model::component::ComponentId;
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
            paths.map(|path| normalized_blob_path(Path::new(path)).map(|path| path.to_path_buf()));

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

    /// A path that is already in its one form is the one form, and the result borrows it, so
    /// this form of such a path allocates nothing. Each operation of each backend makes this
    /// form of the path that it gets, and then builds the key of its backend from the form.
    #[test]
    fn the_one_form_of_a_path_that_is_already_in_it_borrows_that_path() {
        let path = Path::new("dir/blob");

        let normalized = normalized_blob_path(path).unwrap();

        assert!(std::ptr::eq(&*normalized, path));
    }

    /// A path that starts with a prefix of Windows names a place outside the namespace on that
    /// host: `C:` names a drive and `\\` names a server. Windows gives such a prefix as
    /// `Component::Prefix` and unix gives it as a name of the path, so the rule reads the text
    /// of the one form and both hosts give the same error for the same path.
    #[test]
    fn a_path_that_starts_with_a_prefix_of_windows_is_not_relative() {
        let paths = [
            "C:/escape",
            "C:\\escape",
            "C:escape",
            "c:",
            "\\\\server\\share",
            "./C:/escape",
        ];

        let results =
            paths.map(|path| normalized_blob_path(Path::new(path)).map(|path| path.to_path_buf()));

        assert_eq!(
            results,
            paths.map(|path| Err(BlobNameError::NotRelative {
                path: PathBuf::from(path)
            }))
        );
    }

    /// The rule reads the prefix as Windows reads it: one letter and a `:` at the start of the
    /// path. A `:` that is somewhere else, and a name that has more than one letter before the
    /// `:`, name a blob.
    #[test]
    fn a_colon_that_is_not_a_prefix_of_windows_names_a_blob() {
        let paths = ["note:1", "a/C:/b", "ab:cd", "1:/x"];

        let results =
            paths.map(|path| normalized_blob_path(Path::new(path)).map(|path| path.to_path_buf()));

        assert_eq!(results, paths.map(|path| Ok(PathBuf::from(path))));
    }

    /// The rules of the text hold for the one form of the path, so a `.` name that the one
    /// form removes is not a `.` segment, and a `\` that is a name of the path on unix is a
    /// separator of the text.
    #[test]
    fn the_one_form_of_a_path_gives_the_rule_that_its_text_breaks() {
        let paths = [
            "a\0b",
            " . ",
            "a/ .. /b",
            "a\\..\\b",
            "dir/__dir_marker",
            "__dir_marker",
            "a/./b",
            "a/__dir_marker/b",
        ];

        let results =
            paths.map(|path| normalized_blob_path(Path::new(path)).map(|path| path.to_path_buf()));

        assert_eq!(
            results,
            [
                Err(BlobNameError::NulByte),
                Err(BlobNameError::DotSegment {
                    segment: " . ".to_string()
                }),
                Err(BlobNameError::DotSegment {
                    segment: " .. ".to_string()
                }),
                Err(BlobNameError::DotSegment {
                    segment: "..".to_string()
                }),
                Err(BlobNameError::Reserved {
                    marker: "__dir_marker"
                }),
                Err(BlobNameError::Reserved {
                    marker: "__dir_marker"
                }),
                Ok(PathBuf::from("a/b")),
                Ok(PathBuf::from("a/__dir_marker/b")),
            ]
        );
    }

    /// The guest gives its names as text, so only a path of another source can break this
    /// rule. `normalized_blob_path` gives the error, so no `NormalizedBlobPath` holds such a
    /// path and the functions that read the one form cannot get one.
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
            (
                normalized_blob_path(path).map(|path| path.to_path_buf()),
                blob_path_to_string(path),
            ),
            (Err(expected.clone()), Err(expected))
        );
    }

    /// A guest picks the name of its container and the name of its object, so it can give two
    /// names that make a path with no name in it. The in-memory and the SQLite backends read
    /// the last name of the path, and the rule is of the name, so the error is permanent. The
    /// one form of such a path is the empty path, and the error names that form.
    #[test]
    fn a_path_with_no_name_in_it_gives_the_name_rule() {
        let paths = ["", "."];

        let results = paths.map(|path| {
            normalized_blob_path(Path::new(path))
                .and_then(|path| path.file_name_text())
                .map(|_| ())
        });

        assert_eq!(
            results,
            [
                Err(BlobNameError::NoName {
                    path: PathBuf::from("")
                }),
                Err(BlobNameError::NoName {
                    path: PathBuf::from("")
                }),
            ]
        );
    }

    /// The segment is the agent name with each character that is not an ASCII letter, a digit, `-`
    /// or `_` replaced by `_`. The name is cut to 32 characters, and an empty name gives `agent`.
    /// Then come `-` and the blake3 hash of the full agent id. The first agent name here is longer
    /// than 32 characters and holds characters that the segment replaces. The second name is empty.
    #[test]
    fn the_path_segment_of_an_agent_keeps_its_form() {
        let component_id =
            ComponentId(uuid::Uuid::parse_str("0d9f6c1e-2b8a-4f3d-9e7c-5a4b3c2d1e0f").unwrap());
        let segments = [r#"counter("a/../b", 12345678901234567890)"#, ""].map(|agent| {
            agent_path_segment(&AgentId {
                component_id,
                agent_id: agent.to_string(),
            })
        });

        assert_eq!(
            segments,
            [
                "counter__a____b___12345678901234-97f0841e19646b6f20282e1d316187fb327b2df867064639c28e15d22dd797ec"
                    .to_string(),
                "agent-6a683fb8dbe943ef400d01ca02589e2b93eb9e982cfff9bf930fdcad6787107f".to_string(),
            ]
        );
    }

    #[test]
    fn blob_path_to_string_gives_the_text_of_the_path() {
        assert_eq!(blob_path_to_string(Path::new("a/b")), Ok("a/b".to_string()));
    }
}
