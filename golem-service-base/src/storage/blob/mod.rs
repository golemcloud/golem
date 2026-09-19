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
            None => Err(anyhow!("Blob storage entry not found: {from:?}")),
        }
    }

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

    pub async fn list_blobs_below(
        &self,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> Result<Box<[ListedBlob]>, Error> {
        self.storage
            .list_blobs_below(self.svc_name, self.api_name, namespace, path)
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

/// Gives the bytes from `start` to `end` of a whole blob. Both offsets are inclusive.
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

pub(crate) fn validate_relative_blob_path(path: &Path) -> Result<(), Error> {
    if path.is_absolute() {
        return Err(anyhow!("Blob path must be relative: {path:?}"));
    }

    for component in path.components() {
        match component {
            Component::Normal(_) | Component::CurDir => {}
            Component::ParentDir => {
                return Err(anyhow!(
                    "Blob path cannot contain parent traversal: {path:?}"
                ));
            }
            Component::RootDir | Component::Prefix(_) => {
                return Err(anyhow!("Blob path must be relative: {path:?}"));
            }
        }
    }

    Ok(())
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

pub(crate) fn blob_path_to_string(path: &Path) -> Result<String, Error> {
    path.to_str()
        .map(|s| s.to_string())
        .ok_or_else(|| anyhow!("Blob path must be valid UTF-8: {path:?}"))
}

pub(crate) fn blob_parent_to_string(path: &Path) -> Result<String, Error> {
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

pub(crate) fn blob_file_name_to_string(path: &Path) -> Result<String, Error> {
    path.file_name()
        .ok_or_else(|| anyhow!("Path must have a file name: {path:?}"))
        .and_then(|name| {
            name.to_str()
                .map(|s| s.to_string())
                .ok_or_else(|| anyhow!("Blob path must be valid UTF-8: {path:?}"))
        })
}

#[cfg(test)]
mod tests {
    use super::{BlobRangeError, blob_range};
    use pretty_assertions::assert_eq;
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
}
