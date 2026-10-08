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
use futures::{StreamExt, TryStreamExt};
use golem_common::model::agent::AgentMode;
use golem_common::model::component::ComponentId;
use golem_common::model::environment::EnvironmentId;
use golem_common::model::{AgentFingerprint, AgentId, Timestamp};
use golem_common::serialization::{deserialize, serialize};
use std::fmt::Debug;
use std::ops::RangeInclusive;
use std::path::{Path, PathBuf};
use typed_path::{Utf8UnixComponent, Utf8UnixPath, Utf8UnixPathBuf};

pub mod fs;
pub mod memory;
pub mod s3;
pub mod sqlite;

pub use normalized_path::NormalizedBlobPath;
pub(crate) use normalized_path::normalized_blob_path;

pub const BLOB_STREAM_CHUNK_SIZE: usize = 64 * 1024;

pub struct BlobRangeStream {
    pub total_size: u64,
    pub stream: BoxStream<'static, Result<Bytes, Error>>,
}

fn validate_range(offset: u64, length: u64, total_size: u64) -> Result<(), Error> {
    if offset
        .checked_add(length)
        .is_none_or(|end| end > total_size)
    {
        return Err(anyhow!("Blob range outside object"));
    }
    Ok(())
}

/// Keeps blobs at the paths of a namespace.
///
/// A path names a blob or a directory, and a directory is not a blob. So a read at the path of a
/// directory finds no blob, a delete at that path removes no blob, and a write at that path is an
/// error. A path is at the root of the namespace when it has no name in it, for example an empty
/// path or `.`, and the root is a directory. A directory is there while a blob is below it, at
/// any depth, and a directory that `create_dir` made is there until `delete_dir` removes it. A
/// directory that `create_dir` made keeps a size of zero and a time, which `get_metadata` gives.
///
/// A late change: a write or a delete whose call ended without an answer, or whose call was
/// dropped, lands within one storage call deadline of the end of the call, or never. The doc of a
/// method says when this holds for it. It is an assumption about each backend: the filesystem
/// backend checks right before it makes the change visible that the call was not dropped, and a
/// change that passed that check becomes visible within one deadline; the SQLite backend runs a
/// dropped statement that already reached its connection within one deadline; and the S3 server
/// applies a request that it got in full within one deadline.
#[async_trait]
pub trait BlobStorage: sealed::Sealed + Debug + Send + Sync {
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

    /// Opens a bounded selection without collecting the object. Missing objects return
    /// None; out-of-bounds selections are errors. Empty selections are allowed, including
    /// at EOF. Chunks are at most BLOB_STREAM_CHUNK_SIZE bytes; dropping the stream releases
    /// the reader. The total size describes the opened object, not the selection.
    async fn get_range_stream(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
        offset: u64,
        length: u64,
    ) -> Result<Option<BlobRangeStream>, Error>;

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
    ) -> Result<Option<Vec<u8>>, Error>;

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
    ///
    /// In the namespace [`BlobStorageNamespace::FilesystemSnapshots`] a reader sees the whole new
    /// blob or the one before, and the rule of a late change in the doc of [`BlobStorage`] holds.
    /// The other namespaces have neither promise.
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
    ///
    /// The rule of a late change in the doc of [`BlobStorage`] holds for this call.
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
    ///
    /// The rule of a late change in the doc of [`BlobStorage`] holds for this call in the
    /// namespace `FilesystemSnapshots`.
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
    /// The storage reads every path before it removes the first blob, so a path that breaks a
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
    ) -> Result<(), Error>;

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
    ///
    /// On a strongly consistent store: a blob that exists for the whole listing is in the result.
    /// A blob that a write or a delete adds or removes during the listing can be present or
    /// absent, and the listing does not fail because of it.
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
    ) -> Result<(), Error>;

    /// Writes the blob at `from` in `from_namespace` as the blob at `to` in `to_namespace`, and
    /// keeps the blob at `from`.
    ///
    /// The rules of `copy` hold for each end: a root path at either end gives
    /// [`BlobNameError::NoName`], a `from` with no blob gives [`BlobMissingError`] and writes
    /// nothing, and the same path in the same namespace writes nothing and gives
    /// [`BlobMissingError`] when the blob is not there. Each name is checked before it becomes a
    /// key of the backend. A blob at `to` is replaced.
    ///
    /// The rule of a late change in the doc of [`BlobStorage`] holds for this call.
    async fn copy_between(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        from_namespace: BlobStorageNamespace,
        from: &Path,
        to_namespace: BlobStorageNamespace,
        to: &Path,
    ) -> Result<(), Error>;

    /// Writes the blob at the `from` path as the blob at the `to` path, and then deletes the
    /// blob at `from`.
    ///
    /// A move onto the same path keeps the blob where it is, and two forms of one path are the same
    /// path. A blob cannot be where a directory is, so a root path at either end gives
    /// [`BlobNameError::NoName`]. The copy comes before the delete, so each error of `copy` is an
    /// error of `move` and the blob at `from` stays: a `from` path with no blob at it gives the
    /// error of the copy, which is [`BlobMissingError`] where `copy` gives it, the move onto the
    /// same path as well.
    async fn r#move(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        from: &Path,
        to: &Path,
    ) -> Result<(), Error>;
}

/// Keeps [`BlobStorage`] to the one implementation that each backend gets from
/// [`BlobStorageBackend`].
mod sealed {
    pub trait Sealed {}

    impl<B: super::BlobStorageBackend> Sealed for B {}
}

/// Keeps the blobs of the namespaces of one backend.
///
/// Each backend gets [`BlobStorage`] from this trait, and that implementation applies the rules of
/// a path before a method of this trait runs. It makes the one form of each path
/// (`normalized_blob_path`), so a path that breaks a rule that `normalized_blob_path` applies gets
/// that error and no method of this trait gets the path. It gives the answer of [`BlobStorage`] at
/// a root path, so only `list_dir_at` and `list_blobs_below_at` get a root path. It gives the
/// [`BlobRangeError`] of a `start` after `end`, and the [`BlobMissingError`] of `copy` and `move`.
/// It reads every path of `delete_many` before `delete_many_at` runs.
///
/// Each method has the rules of the method of [`BlobStorage`] with the same name without `_at`,
/// except where its own doc says otherwise.
#[async_trait]
pub trait BlobStorageBackend: Debug + Send + Sync {
    /// Gives the bytes of the blob at the path, or nothing if the path has no blob.
    async fn get_raw_at(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
    ) -> Result<Option<Vec<u8>>, Error>;

    /// Gives the bytes of the blob at the path as a stream, or nothing if the path has no blob.
    ///
    /// The default reads the blob with `get_raw_at` and gives it as one chunk.
    async fn get_stream_at(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
    ) -> Result<Option<BoxStream<'static, Result<Bytes, Error>>>, Error> {
        Ok(self
            .get_raw_at(target_label, op_label, namespace, path)
            .await?
            .map(|data| futures::stream::iter([Ok(Bytes::from(data))]).boxed()))
    }

    /// Opens a bounded selection of the blob at the path, or gives nothing if the path has no
    /// blob.
    async fn get_range_stream_at(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
        offset: u64,
        length: u64,
    ) -> Result<Option<BlobRangeStream>, Error>;

    /// Reads the bytes from `start` to `end` of the blob at the path. Both offsets are inclusive,
    /// and `start` is not after `end`.
    ///
    /// The default reads the full blob with `get_raw_at` and gives the range of it
    /// (`blob_range`).
    async fn get_raw_slice_at(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
        start: u64,
        end: u64,
    ) -> Result<Option<Vec<u8>>, Error> {
        self.get_raw_at(target_label, op_label, namespace, path)
            .await?
            .map(|data| {
                blob_range(&data, start, end)
                    .map(<[u8]>::to_vec)
                    .map_err(Error::from)
            })
            .transpose()
    }

    /// Tells the size and the time of the blob at the path, or of the directory that
    /// `create_dir` made at the path, or nothing.
    async fn get_metadata_at(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
    ) -> Result<Option<BlobMetadata>, Error>;

    /// Writes the bytes as the blob at the path, over the blob that was there.
    async fn put_raw_at(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
        data: &[u8],
    ) -> Result<(), Error>;

    /// Writes the bytes as the blob at the path when the path has no blob. The check and the
    /// write are one step.
    async fn put_raw_if_absent_at(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
        data: &[u8],
    ) -> Result<PutIfAbsent, Error>;

    /// Writes the bytes of the stream as the blob at the path, over the blob that was there.
    ///
    /// The default reads the full stream and writes it with `put_raw_at`.
    async fn put_stream_at(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
        stream: &dyn ErasedReplayableStream<Item = Result<Vec<u8>, Error>, Error = Error>,
    ) -> Result<(), Error> {
        let data = stream
            .make_stream_erased()
            .await?
            .try_collect::<Vec<_>>()
            .await?
            .concat();
        self.put_raw_at(target_label, op_label, namespace, path, &data)
            .await
    }

    /// Removes the blob at the path. A path that has no blob changes nothing.
    async fn delete_at(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
    ) -> Result<(), Error>;

    /// Removes the blob at every one of the paths.
    ///
    /// The default removes them with `delete_at`, one path after the other, and stops at the
    /// first error.
    async fn delete_many_at(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        paths: &[NormalizedBlobPath<'_>],
    ) -> Result<(), Error> {
        futures::stream::iter(paths.iter().map(Ok))
            .try_for_each(|path| self.delete_at(target_label, op_label, namespace.clone(), path))
            .await
    }

    /// Makes a directory at the path.
    async fn create_dir_at(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
    ) -> Result<(), Error>;

    /// Lists the blobs that are directly below the path, and each directory that `create_dir`
    /// made below the path, at any depth. The path can be a root path.
    async fn list_dir_at(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
    ) -> Result<Vec<PathBuf>, Error>;

    /// Lists each blob below the path, at all depths, with its size. The path can be a root
    /// path.
    async fn list_blobs_below_at(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
    ) -> Result<Box<[ListedBlob]>, Error>;

    /// Deletes the directory at the path and all the entries below it, at any depth. Returns
    /// true if the path had a directory.
    async fn delete_dir_at(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
    ) -> Result<bool, Error>;

    /// Tells what the path has.
    async fn exists_at(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
    ) -> Result<ExistsResult, Error>;

    /// Tells if the path has a blob. A directory at the path is not a blob. The default reads
    /// `exists_at`.
    async fn has_blob_at(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
    ) -> Result<bool, Error> {
        Ok(self
            .exists_at(target_label, op_label, namespace, path)
            .await?
            == ExistsResult::File)
    }

    /// Writes the blob at `from` in `from_namespace` as the blob at `to` in `to_namespace`, and
    /// keeps the blob at `from`. The two ends are not the same path in the same namespace, and
    /// neither is a root path.
    ///
    /// Gives true when the copy wrote the blob, and false when `from` has no blob. False writes
    /// nothing to `to`. Each backend keeps the rule of a late change of
    /// [`BlobStorage::copy_between`].
    async fn copy_between_at(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        from_namespace: BlobStorageNamespace,
        from: &NormalizedBlobPath<'_>,
        to_namespace: BlobStorageNamespace,
        to: &NormalizedBlobPath<'_>,
    ) -> Result<bool, Error>;
}

/// Gives the one form of a path that names a blob, or `None` for a root path, which is a
/// directory and names no blob.
fn blob_path(path: &Path) -> Result<Option<NormalizedBlobPath<'_>>, BlobNameError> {
    normalized_blob_path(path).map(|path| (!path.is_root()).then_some(path))
}

/// Gives the one form of a path at which an operation writes a blob. A root path gives
/// [`BlobNameError::NoName`], because a blob cannot be where a directory is.
fn written_blob_path(path: &Path) -> Result<NormalizedBlobPath<'_>, BlobNameError> {
    let path = normalized_blob_path(path)?;
    path.reject_root()?;
    Ok(path)
}

/// Gives `Ok(())` when the copy found the blob at its source, and otherwise a
/// [`BlobMissingError`] that names the source path as the guest wrote it.
fn blob_found(found: bool, guest_from: &Path) -> Result<(), Error> {
    if found {
        Ok(())
    } else {
        Err(BlobMissingError {
            path: guest_from.to_path_buf(),
        }
        .into())
    }
}

/// The one implementation of [`BlobStorage`]. It applies the rules of a path and of a range, and
/// the answers at a root path, and then gives the operation to the backend.
#[async_trait]
impl<B: BlobStorageBackend> BlobStorage for B {
    async fn get_raw(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> Result<Option<Vec<u8>>, Error> {
        match blob_path(path)? {
            Some(path) => {
                self.get_raw_at(target_label, op_label, namespace, &path)
                    .await
            }
            None => Ok(None),
        }
    }

    async fn get_stream(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> Result<Option<BoxStream<'static, Result<Bytes, Error>>>, Error> {
        match blob_path(path)? {
            Some(path) => {
                self.get_stream_at(target_label, op_label, namespace, &path)
                    .await
            }
            None => Ok(None),
        }
    }

    async fn get_range_stream(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
        offset: u64,
        length: u64,
    ) -> Result<Option<BlobRangeStream>, Error> {
        match blob_path(path)? {
            Some(path) => {
                self.get_range_stream_at(target_label, op_label, namespace, &path, offset, length)
                    .await
            }
            None => Ok(None),
        }
    }

    async fn get_raw_slice(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
        start: u64,
        end: u64,
    ) -> Result<Option<Vec<u8>>, Error> {
        ordered_range(start, end)?;
        match blob_path(path)? {
            Some(path) => {
                self.get_raw_slice_at(target_label, op_label, namespace, &path, start, end)
                    .await
            }
            None => Ok(None),
        }
    }

    async fn get_metadata(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> Result<Option<BlobMetadata>, Error> {
        match blob_path(path)? {
            Some(path) => {
                self.get_metadata_at(target_label, op_label, namespace, &path)
                    .await
            }
            None => Ok(None),
        }
    }

    async fn put_raw(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
        data: &[u8],
    ) -> Result<(), Error> {
        let path = written_blob_path(path)?;
        self.put_raw_at(target_label, op_label, namespace, &path, data)
            .await
    }

    async fn put_raw_if_absent(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
        data: &[u8],
    ) -> Result<PutIfAbsent, Error> {
        let path = written_blob_path(path)?;
        self.put_raw_if_absent_at(target_label, op_label, namespace, &path, data)
            .await
    }

    async fn put_stream(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
        stream: &dyn ErasedReplayableStream<Item = Result<Vec<u8>, Error>, Error = Error>,
    ) -> Result<(), Error> {
        let path = written_blob_path(path)?;
        self.put_stream_at(target_label, op_label, namespace, &path, stream)
            .await
    }

    async fn delete(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> Result<(), Error> {
        match blob_path(path)? {
            Some(path) => {
                self.delete_at(target_label, op_label, namespace, &path)
                    .await
            }
            None => Ok(()),
        }
    }

    async fn delete_many(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        paths: &[PathBuf],
    ) -> Result<(), Error> {
        // Every path is read before the first blob goes. A root path names no blob, so it is
        // not in the list. The list is a `Vec` and not a boxed slice: it lives only for this
        // call, and `filter_map` does not know its length, so a boxed slice can copy the list a
        // second time to remove the spare capacity.
        let paths = paths
            .iter()
            .filter_map(|path| blob_path(path).transpose())
            .collect::<Result<Vec<_>, _>>()?;
        self.delete_many_at(target_label, op_label, namespace, &paths)
            .await
    }

    async fn create_dir(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> Result<(), Error> {
        match blob_path(path)? {
            Some(path) => {
                self.create_dir_at(target_label, op_label, namespace, &path)
                    .await
            }
            None => Ok(()),
        }
    }

    async fn list_dir(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> Result<Vec<PathBuf>, Error> {
        let path = normalized_blob_path(path)?;
        self.list_dir_at(target_label, op_label, namespace, &path)
            .await
    }

    async fn list_blobs_below(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> Result<Box<[ListedBlob]>, Error> {
        let path = normalized_blob_path(path)?;
        self.list_blobs_below_at(target_label, op_label, namespace, &path)
            .await
    }

    async fn delete_dir(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> Result<bool, Error> {
        match blob_path(path)? {
            Some(path) => {
                self.delete_dir_at(target_label, op_label, namespace, &path)
                    .await
            }
            None => Ok(false),
        }
    }

    async fn exists(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> Result<ExistsResult, Error> {
        match blob_path(path)? {
            Some(path) => {
                self.exists_at(target_label, op_label, namespace, &path)
                    .await
            }
            None => Ok(ExistsResult::Directory),
        }
    }

    async fn copy(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        from: &Path,
        to: &Path,
    ) -> Result<(), Error> {
        self.copy_between(
            target_label,
            op_label,
            namespace.clone(),
            from,
            namespace,
            to,
        )
        .await
    }

    async fn copy_between(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        from_namespace: BlobStorageNamespace,
        from: &Path,
        to_namespace: BlobStorageNamespace,
        to: &Path,
    ) -> Result<(), Error> {
        let (source, target) = (normalized_blob_path(from)?, normalized_blob_path(to)?);
        // A copy onto the same path writes nothing, and it still needs the blob that it reads.
        let found =
            if blob_copy_changes_nothing(&source, &target)? && from_namespace == to_namespace {
                self.has_blob_at(target_label, op_label, from_namespace, &source)
                    .await?
            } else {
                self.copy_between_at(
                    target_label,
                    op_label,
                    from_namespace,
                    &source,
                    to_namespace,
                    &target,
                )
                .await?
            };
        blob_found(found, from)
    }

    async fn r#move(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        from: &Path,
        to: &Path,
    ) -> Result<(), Error> {
        let (source, target) = (normalized_blob_path(from)?, normalized_blob_path(to)?);
        // A move onto the same path keeps the blob, so it is the copy and no delete.
        if blob_copy_changes_nothing(&source, &target)? {
            let found = self
                .has_blob_at(target_label, op_label, namespace, &source)
                .await?;
            return blob_found(found, from);
        }

        let found = self
            .copy_between_at(
                target_label,
                op_label,
                namespace.clone(),
                &source,
                namespace.clone(),
                &target,
            )
            .await?;
        blob_found(found, from)?;
        self.delete_at(target_label, op_label, namespace, &source)
            .await
    }
}

/// Creates a blob-storage facade that binds the service and API labels once for multiple calls.
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

/// A blob-storage facade with service and API labels bound to every operation.
pub struct LabelledBlobStorage<'a, S: BlobStorage + ?Sized> {
    svc_name: &'static str,
    api_name: &'static str,
    storage: &'a S,
}

impl<'a, S: BlobStorage + ?Sized + Sync> LabelledBlobStorage<'a, S> {
    fn record(&self, operation: &'static str) {
        crate::metrics::storage::record_logical_operation(
            "blob",
            operation,
            self.svc_name,
            self.api_name,
            "",
        );
    }

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
        self.record("get");
        self.storage
            .get_raw(self.svc_name, self.api_name, namespace, path)
            .await
    }

    pub async fn get_stream(
        &self,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> Result<Option<BoxStream<'static, Result<Bytes, Error>>>, Error> {
        self.record("get_stream");
        self.storage
            .get_stream(self.svc_name, self.api_name, namespace, path)
            .await
    }

    pub async fn get_range_stream(
        &self,
        namespace: BlobStorageNamespace,
        path: &Path,
        offset: u64,
        length: u64,
    ) -> Result<Option<BlobRangeStream>, Error> {
        self.record("get_range_stream");
        self.storage
            .get_range_stream(
                self.svc_name,
                self.api_name,
                namespace,
                path,
                offset,
                length,
            )
            .await
    }

    pub async fn get_raw_slice(
        &self,
        namespace: BlobStorageNamespace,
        path: &Path,
        start: u64,
        end: u64,
    ) -> Result<Option<Vec<u8>>, Error> {
        self.record("get_slice");
        self.storage
            .get_raw_slice(self.svc_name, self.api_name, namespace, path, start, end)
            .await
    }

    pub async fn get_metadata(
        &self,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> Result<Option<BlobMetadata>, Error> {
        self.record("get_metadata");
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
        self.record("put");
        self.storage
            .put_raw(self.svc_name, self.api_name, namespace, path, data)
            .await
    }

    pub async fn put_stream(
        &self,
        namespace: BlobStorageNamespace,
        path: &Path,
        stream: &dyn ErasedReplayableStream<Item = Result<Vec<u8>, Error>, Error = Error>,
    ) -> Result<(), Error> {
        self.record("put_stream");
        self.storage
            .put_stream(self.svc_name, self.api_name, namespace, path, stream)
            .await
    }

    pub async fn delete(&self, namespace: BlobStorageNamespace, path: &Path) -> Result<(), Error> {
        self.record("delete");
        self.storage
            .delete(self.svc_name, self.api_name, namespace, path)
            .await
    }

    pub async fn delete_many(
        &self,
        namespace: BlobStorageNamespace,
        paths: &[PathBuf],
    ) -> Result<(), Error> {
        self.record("delete_many");
        self.storage
            .delete_many(self.svc_name, self.api_name, namespace, paths)
            .await
    }

    pub async fn create_dir(
        &self,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> Result<(), Error> {
        self.record("create_dir");
        self.storage
            .create_dir(self.svc_name, self.api_name, namespace, path)
            .await
    }

    pub async fn list_dir(
        &self,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> Result<Vec<PathBuf>, Error> {
        self.record("list_dir");
        self.storage
            .list_dir(self.svc_name, self.api_name, namespace, path)
            .await
    }

    pub async fn list_blobs_below(
        &self,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> Result<Box<[ListedBlob]>, Error> {
        self.record("list_blobs_below");
        self.storage
            .list_blobs_below(self.svc_name, self.api_name, namespace, path)
            .await
    }

    pub async fn delete_dir(
        &self,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> Result<bool, Error> {
        self.record("delete_dir");
        self.storage
            .delete_dir(self.svc_name, self.api_name, namespace, path)
            .await
    }

    pub async fn exists(
        &self,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> Result<ExistsResult, Error> {
        self.record("exists");
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
        self.record("copy");
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
        self.record("move");
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
    /// The filesystem snapshots of one incarnation of an agent. Each incarnation has its own
    /// location on each backend. The locations of the incarnations of one agent share the prefix
    /// of the agent.
    FilesystemSnapshots {
        environment_id: EnvironmentId,
        agent_id: AgentId,
        fingerprint: AgentFingerprint,
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
/// `copy` and `copy_between` of [`BlobStorage`] give this error when the storage holds no blob at
/// the source path, on each backend. The in-memory backend reads its map, the SQLite backend copies
/// the row with one statement, the S3 backend sends one `CopyObject` request and reads the code
/// `NoSuchKey` of the source key, and the filesystem backend opens the source before it writes.
/// `move` is a copy and then a delete of the source, so it gives the error of the copy too, and it
/// deletes nothing. A guest picks the
/// source container name and the source object name of `copy_object` and of `move_object`, so the
/// path is of the guest. The storage names the path as the guest wrote it, and not in the
/// normalized form that the storage uses. Each [`BlobNameError`] does the same, because the guest
/// reads the message, except [`BlobNameError::NoName`], which names the one form of the path. The
/// one form of a path with no name in it is the empty path, and each spelling of such a path says
/// the same thing to the guest.
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
/// them. [`BlobStorage`] applies that function to each path of each operation before a backend
/// gets the path, so every backend gives the same answer for a name: `NotRelative` and
/// `ParentDir` for a path that leaves the namespace, `NotUtf8` for a path that the one form
/// cannot hold as text, and then `NulByte`, `DotSegment` and `Reserved` for the text of that
/// form (`check_blob_name`). The three rules of the text are rules of S3 or of MinIO, and one
/// is the name that the S3 backend keeps for its own object.
///
/// Each operation that writes a blob applies `NoName` as well, before a backend gets the path:
/// `put_raw`, `put_raw_if_absent` and `put_stream` apply it to the path of the blob
/// (`NormalizedBlobPath::reject_root`), and `copy` and `move` apply it to both of their paths
/// (`blob_copy_changes_nothing`).
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
    /// path read as a Unix path, between two `/` separators, on every host.
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
    /// that writes a blob at such a path gives this error on each backend: `put_raw`,
    /// `put_raw_if_absent` and `put_stream` (`NormalizedBlobPath::reject_root`), and `copy`
    /// and `move`, at either of their two paths (`blob_copy_changes_nothing`). An operation
    /// that reads a blob gives `Ok(None)` for a root path, and `exists` gives `Directory`.
    ///
    /// The in-memory and the SQLite backends hold a blob by the name of its directory and the
    /// name of the blob itself (`NormalizedBlobPath::file_name_text`). A root path gives no such
    /// name, so that function gives this error for it.
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

/// Tells if a later request can pass where an error of the blob storage failed.
///
/// This is the one list of the permanent errors of the blob storage. [`BlobRangeError`],
/// [`BlobNameError`] and [`BlobMissingError`] are permanent, because a retry asks for the same
/// byte, the same name or the same missing blob again. Each other error is of the backend, and is
/// transient: the backend spent its retry budget on it, and a later request can pass.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlobFailure {
    Transient,
    Permanent,
}

impl BlobFailure {
    /// The kind of `error`. A context around a permanent error keeps it permanent.
    pub fn of(error: &Error) -> Self {
        if error.is::<BlobRangeError>()
            || error.is::<BlobNameError>()
            || error.is::<BlobMissingError>()
        {
            Self::Permanent
        } else {
            Self::Transient
        }
    }
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

/// Gives a [`BlobRangeError`] when `start` is after `end`, because no byte is in such a range.
///
/// Both offsets are inclusive. This is the one rule of a range that needs no length of a blob,
/// so `get_raw_slice` of [`BlobStorage`] reads it before a backend reads the blob.
fn ordered_range(start: u64, end: u64) -> Result<(), BlobRangeError> {
    if start <= end {
        Ok(())
    } else {
        Err(BlobRangeError { start, end })
    }
}

/// Gives the positions of the bytes from `start` to `end` in a blob of `length` bytes. Both
/// offsets are inclusive, and the rules of [`blob_range`] apply.
pub(crate) fn blob_positions(
    length: usize,
    start: u64,
    end: u64,
) -> Result<RangeInclusive<usize>, BlobRangeError> {
    ordered_range(start, end)
        .ok()
        .and_then(|()| usize::try_from(start).ok().zip(usize::try_from(end).ok()))
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
/// Windows reads such a prefix as the start of a path that its own root holds. A blob path is
/// read as a Unix path on every host (`normalized_blob_path`), and Unix has no such prefix, so
/// the prefix is the text of a name there and nothing else refuses it. The rule reads the text,
/// so the host that runs the process does not change the answer.
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
///
/// A blob path has the semantics of a Unix path on every host: `/` is the one separator, and `\`
/// and a prefix of Windows are characters of a name. So the one form of a path, and the names
/// that a backend reads from it, do not change with the host that runs the process.
mod normalized_path {
    use super::{
        BlobNameError, blob_path_starts_with_windows_prefix, blob_path_to_string, check_blob_name,
    };
    use std::borrow::Cow;
    use std::ops::Deref;
    use std::path::{Path, PathBuf};
    use typed_path::{UnixComponent, UnixPath, Utf8UnixComponent, Utf8UnixPath};

    /// The one form of a relative blob path (`normalized_blob_path`).
    ///
    /// [`BlobStorage`](super::BlobStorage) makes this form of the path of each operation, and each
    /// method of [`BlobStorageBackend`](super::BlobStorageBackend) gets this form. The functions
    /// that make a key of a path take this type and nothing else, so a path that has not been
    /// through `normalized_blob_path` cannot reach them and no comment has to say that it must not.
    /// Code outside this crate can read the path of this type, and it cannot make one.
    ///
    /// The form borrows the path of the caller when that path is already in its one form, so
    /// this form of such a path allocates nothing. The operation that follows still builds the
    /// key of its backend from the form.
    ///
    /// The form is valid UTF-8 and its names are separated by `/` on every host. A backend reads
    /// the names through `text`, `parent_text` and `file_name_text`, which split at `/` only, and
    /// not through the `Path` that `Deref` gives, because `Path` splits at `\` on Windows.
    ///
    /// The type gives the path itself to a caller that reads it, and that caller has a
    /// `&Path` (`Deref`). A caller that makes a key has to name the type, and the four
    /// backends do: `S3BlobStorage::key_of`, `FileSystemBlobStorage::path_of`,
    /// `InMemoryBlobStorage::blob_key`, and the parts of the key that the in-memory and the
    /// SQLite backends bind.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct NormalizedBlobPath<'a>(Cow<'a, Path>);

    impl NormalizedBlobPath<'static> {
        /// Gives the path at the root of a namespace, which has no name in it.
        #[cfg(test)]
        pub(crate) fn root() -> Self {
            Self(Cow::Borrowed(Path::new("")))
        }
    }

    impl NormalizedBlobPath<'_> {
        /// Tells if the path is at the root of a namespace.
        ///
        /// A path is at the root when it has no name in it. An empty path is at the root,
        /// and so is a path that only has `.` in it, because the one form keeps no `.`. So the
        /// one form of a path at the root is the empty path.
        pub(crate) fn is_root(&self) -> bool {
            self.0.as_os_str().is_empty()
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
            self.unix().map(|path| {
                path.parent()
                    .map(|parent| parent.as_str().to_string())
                    .unwrap_or_default()
            })
        }

        /// Gives the text of the last name of the path.
        ///
        /// A path that is not valid UTF-8 gives [`BlobNameError::NotUtf8`], which
        /// `normalized_blob_path` has already refused. A path with no name in it is at the root
        /// of its namespace (`is_root`) and gives [`BlobNameError::NoName`]: a guest can pick
        /// two empty names, so the path is of the guest and so is the error. The two errors are
        /// permanent.
        pub(crate) fn file_name_text(&self) -> Result<String, BlobNameError> {
            self.unix()?
                .file_name()
                .map(|name| name.to_string())
                .ok_or_else(|| BlobNameError::NoName {
                    path: self.0.to_path_buf(),
                })
        }

        /// Gives the names of the path, from the first to the last. The path at the root has
        /// none.
        pub(crate) fn names(&self) -> Result<impl Iterator<Item = &str>, BlobNameError> {
            Ok(self.unix()?.iter())
        }

        /// Gives the path as a Unix path, whose one separator is `/` on every host.
        fn unix(&self) -> Result<&Utf8UnixPath, BlobNameError> {
            self.0
                .to_str()
                .map(Utf8UnixPath::new)
                .ok_or_else(|| BlobNameError::NotUtf8 {
                    path: self.0.to_path_buf(),
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
    /// The path is read as a Unix path on every host: `/` is the one separator, and `\` is a
    /// character of a name. The form holds the names of the path and one `/` between two names.
    /// A `.` and an extra separator are not names, so they go away, and a path at the root of a
    /// namespace becomes the empty path. Two paths that name the same blob get the same form.
    /// An absolute path, a path with `..` in it, and a path with a prefix of Windows give a
    /// [`BlobNameError`], which is permanent.
    ///
    /// The form is then read as text, so a path that is not valid UTF-8 gives
    /// [`BlobNameError::NotUtf8`]. `blob_path_starts_with_windows_prefix` gives
    /// [`BlobNameError::NotRelative`] for a path that Windows holds outside the namespace,
    /// and `check_blob_name` gives the rule that the text breaks. The text is what a backend
    /// stores, and the rules read `\` as a separator, which the names of the path do not.
    /// [`BlobStorage`](super::BlobStorage) calls this function for every path of every operation
    /// before a backend gets the path, so every backend gives the same error for the same name,
    /// on every host.
    pub(crate) fn normalized_blob_path(
        path: &Path,
    ) -> Result<NormalizedBlobPath<'_>, BlobNameError> {
        let unix = UnixPath::new(path.as_os_str().as_encoded_bytes());

        let (names_length, names_count) =
            unix.components()
                .try_fold(
                    (0usize, 0usize),
                    |(length, count), component| match component {
                        UnixComponent::Normal(name) => Ok((length + name.len(), count + 1)),
                        UnixComponent::CurDir => Ok((length, count)),
                        UnixComponent::ParentDir => Err(BlobNameError::ParentDir {
                            path: path.to_path_buf(),
                        }),
                        UnixComponent::RootDir => Err(BlobNameError::NotRelative {
                            path: path.to_path_buf(),
                        }),
                    },
                )?;

        // The error names the path as the caller gave it, because the guest reads the message.
        let text = path.to_str().ok_or_else(|| BlobNameError::NotUtf8 {
            path: path.to_path_buf(),
        })?;

        // The path is already in its one form when its length is exactly its names plus the one
        // separator that sits between two names, so nothing has to be built.
        let normalized: Cow<'_, str> = if names_length + names_count.saturating_sub(1) == text.len()
        {
            Cow::Borrowed(text)
        } else {
            Cow::Owned(joined_names(
                text,
                names_length + names_count.saturating_sub(1),
            ))
        };

        // The rule holds for the one form, so a path whose one form starts with a prefix of
        // Windows gets the error as well, and the one form of an accepted path is accepted.
        if blob_path_starts_with_windows_prefix(&normalized) {
            return Err(BlobNameError::NotRelative {
                path: path.to_path_buf(),
            });
        }
        check_blob_name(&normalized)?;

        Ok(NormalizedBlobPath(match normalized {
            Cow::Borrowed(text) => Cow::Borrowed(Path::new(text)),
            Cow::Owned(text) => Cow::Owned(PathBuf::from(text)),
        }))
    }

    /// Gives the names of the Unix path `text` with one `/` between two names, in a string of
    /// `length` bytes, which is the length of the result.
    fn joined_names(text: &str, length: usize) -> String {
        Utf8UnixPath::new(text)
            .components()
            .filter_map(|component| match component {
                Utf8UnixComponent::Normal(name) => Some(name),
                _ => None,
            })
            .fold(String::with_capacity(length), |mut joined, name| {
                if !joined.is_empty() {
                    joined.push('/');
                }
                joined.push_str(name);
                joined
            })
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
/// A path that breaks a rule of a name does not name the root, whatever else it holds: a `..` path
/// and an absolute path give `false` here, and the storage gives the rule that it breaks.
///
/// `golem_worker_executor::services::blob_store` reads this for the container name that a guest
/// gives, because a name that names the root names the namespace and not a container.
pub fn blob_path_is_root(path: &Path) -> bool {
    normalized_blob_path(path).is_ok_and(|path| path.is_root())
}

/// Gives the text of the one form of a blob path (`normalized_blob_path`), or the
/// [`BlobNameError`] of the rule that the path breaks.
///
/// Two paths that name the same blob give the same text, so a caller can tell that two paths
/// name one blob without a call to the storage. `golem_worker_executor::services::blob_store`
/// reads it to count each blob of an operation one time.
pub fn normalized_blob_path_text(path: &Path) -> Result<String, BlobNameError> {
    normalized_blob_path(path)?.text()
}

/// Gives the last name of the one form of a blob path (`normalized_blob_path`), read with the
/// rules of a Unix path on every host, or the [`BlobNameError`] of the rule that the path breaks.
///
/// A path at the root of its namespace has no name and gives [`BlobNameError::NoName`].
/// `golem_worker_executor::services::blob_store` reads it for the names that it lists.
pub fn blob_file_name_to_string(path: &Path) -> Result<String, BlobNameError> {
    normalized_blob_path(path)?.file_name_text()
}

/// Gives the text of the path, or a [`BlobNameError`], which is permanent.
pub(crate) fn blob_path_to_string(path: &Path) -> Result<String, BlobNameError> {
    path.to_str()
        .map(|s| s.to_string())
        .ok_or_else(|| BlobNameError::NotUtf8 {
            path: path.to_path_buf(),
        })
}

/// Joins two keys of blob storage with `/`, the separator of a blob path on every host.
///
/// The keys are text that the storage built itself, for example a prefix of the configuration
/// and a name, so the function checks nothing.
pub fn join_blob_key(parent: &str, child: &str) -> String {
    let mut path = Utf8UnixPathBuf::from(parent);
    path.push(child);
    path.into_string()
}

/// Joins two segments of a blob path that a guest gives, with `/` as the separator on every
/// host.
///
/// Both segments must be relative and hold no `..` name. A segment that is absolute gives
/// [`BlobNameError::NotRelative`], and a segment with a `..` name gives
/// [`BlobNameError::ParentDir`], so a child segment never replaces or leaves its parent. A
/// prefix of Windows and a `\` stay characters of a name here; [`BlobStorage`] applies the
/// rules of `normalized_blob_path` to the joined path.
pub fn join_blob_path(parent: &str, child: &str) -> Result<PathBuf, BlobNameError> {
    let parent = relative_blob_segment(parent)?;
    let child = relative_blob_segment(child)?;
    Ok(PathBuf::from(parent.join(child).into_string()))
}

/// Reads a segment of a blob path with the separator `/`, or gives the [`BlobNameError`] of an
/// absolute segment or of a segment with a `..` name.
fn relative_blob_segment(text: &str) -> Result<&Utf8UnixPath, BlobNameError> {
    let path = Utf8UnixPath::new(text);
    path.components()
        .try_for_each(|component| match component {
            Utf8UnixComponent::Normal(_) | Utf8UnixComponent::CurDir => Ok(()),
            Utf8UnixComponent::ParentDir => Err(BlobNameError::ParentDir {
                path: PathBuf::from(text),
            }),
            Utf8UnixComponent::RootDir => Err(BlobNameError::NotRelative {
                path: PathBuf::from(text),
            }),
        })
        .map(|()| path)
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
        BlobFailure, BlobMissingError, BlobNameError, BlobRangeError, agent_path_segment,
        blob_file_name_to_string, blob_path_to_string, blob_range, join_blob_path,
        normalized_blob_path,
    };
    use anyhow::{Context, anyhow};
    use golem_common::model::AgentId;
    use golem_common::model::component::ComponentId;
    use pretty_assertions::assert_eq;
    use std::path::{Path, PathBuf};
    use test_r::test;

    /// A range, a name and a missing blob are permanent, also under a context, and every other
    /// error of the storage is transient.
    #[test]
    fn blob_failure_is_permanent_only_for_a_range_a_name_or_a_missing_blob() {
        let permanent = || {
            [
                anyhow::Error::from(BlobRangeError { start: 3, end: 2 }),
                anyhow::Error::from(BlobNameError::NulByte),
                anyhow::Error::from(BlobMissingError {
                    path: PathBuf::from("a"),
                }),
            ]
        };

        let plain = permanent().map(|error| BlobFailure::of(&error));
        let wrapped = permanent()
            .map(|error| BlobFailure::of(&Err::<(), _>(error).context("wrapped").unwrap_err()));

        assert_eq!(plain, [BlobFailure::Permanent; 3]);
        assert_eq!(wrapped, [BlobFailure::Permanent; 3]);
        assert_eq!(
            [
                BlobFailure::of(&anyhow!("service unavailable")),
                BlobFailure::of(&anyhow!("service unavailable").context("wrapped")),
                BlobFailure::of(&anyhow::Error::from(std::io::Error::other("reset"))),
            ],
            [BlobFailure::Transient; 3]
        );
    }

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
    /// host: `C:` names a drive and `\\` names a server. A blob path is a Unix path on every
    /// host, where such a prefix is the text of a name, so the rule reads the text of the one
    /// form and both hosts give the same error for the same path.
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

    #[test]
    fn join_blob_path_uses_contract_separator() {
        assert_eq!(
            join_blob_path("photos", "animals/cat.png")
                .unwrap()
                .as_os_str(),
            "photos/animals/cat.png"
        );
        assert_eq!(
            join_blob_path("", "cat.png").unwrap().as_os_str(),
            "cat.png"
        );
        assert_eq!(
            join_blob_path("photos/", "cat.png").unwrap().as_os_str(),
            "photos/cat.png"
        );
    }

    #[test]
    fn join_blob_path_does_not_replace_parent() {
        let windows_absolute_name = join_blob_path("photos", r"C:\cats\kitten.png").unwrap();
        assert_eq!(
            windows_absolute_name.as_os_str(),
            r"photos/C:\cats\kitten.png"
        );
        assert!(normalized_blob_path(&windows_absolute_name).is_ok());

        assert!(join_blob_path("photos", "/cats/kitten.png").is_err());
        assert!(join_blob_path("photos", "../kitten.png").is_err());
    }

    #[test]
    fn blob_path_components_use_contract_separator() {
        let path = normalized_blob_path(Path::new(r"photos/animals\cat.png")).unwrap();
        assert_eq!(path.parent_text().unwrap(), "photos");
        assert_eq!(path.file_name_text().unwrap(), r"animals\cat.png");
    }

    /// A blob path is a Unix path on every host: the one form splits only at `/` and joins its
    /// names with `/`, and a `\` stays a character of a name.
    #[test]
    fn the_one_form_splits_and_joins_at_a_slash_only() {
        let path = normalized_blob_path(Path::new(r"./a\b//c\d/./e")).unwrap();

        assert_eq!(
            (
                path.text().unwrap(),
                path.parent_text().unwrap(),
                path.file_name_text().unwrap(),
                path.names().unwrap().collect::<Vec<_>>(),
            ),
            (
                r"a\b/c\d/e".to_string(),
                r"a\b/c\d".to_string(),
                "e".to_string(),
                vec![r"a\b", r"c\d", "e"],
            )
        );
    }

    /// The last name of a listed blob splits only at `/`, and a path at the root has no name.
    #[test]
    fn the_file_name_of_a_blob_path_is_its_last_name_after_a_slash() {
        assert_eq!(
            (
                blob_file_name_to_string(Path::new(r"photos/animals\cat.png")).unwrap(),
                blob_file_name_to_string(Path::new("./")).is_err(),
            ),
            (r"animals\cat.png".to_string(), true)
        );
    }

    #[test]
    fn blob_path_identity_normalizes_current_directory_components() {
        assert_eq!(
            normalized_blob_path(Path::new("./photos/./cat.png"))
                .unwrap()
                .text()
                .unwrap(),
            "photos/cat.png"
        );
    }
}
