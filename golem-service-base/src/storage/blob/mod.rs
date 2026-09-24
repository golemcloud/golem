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

/// Keeps blobs and explicit directory markers at paths within a namespace.
///
/// The namespace root is always a directory but has no stored marker or metadata. Reads at a root
/// path find no blob, creates and deletes at a root path change nothing, and writes, copies, and
/// moves at a root path return [`BlobNameError`]. A blob and an explicit directory marker may have
/// the same path, and a blob may be below another blob. When both exist at one path, blob reads and
/// [`BlobStorage::exists`] prefer the blob while deleting either entry preserves the other.
#[async_trait]
pub trait BlobStorage: Debug + Send + Sync {
    /// Returns the blob at `path`. Directories, including the namespace root, have no blob.
    async fn get_raw(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> Result<Option<Vec<u8>>, Error>;

    /// Returns the blob at `path` as a stream. Directories, including the namespace root, have no
    /// blob.
    async fn get_stream(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> Result<Option<BoxStream<'static, Result<Bytes, Error>>>, Error>;

    async fn get_raw_slice(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
        start: u64,
        end: u64,
    ) -> Result<Option<Vec<u8>>, Error> {
        let data = self
            .get_raw(target_label, op_label, namespace, path)
            .await?;
        Ok(data.map(|data| data[(start as usize)..(end as usize)].to_vec()))
    }

    /// Returns metadata for a blob or explicit directory marker.
    ///
    /// An implicit directory that only contains blobs and the namespace root have no metadata. A
    /// blob takes precedence when a blob and directory marker have the same path.
    async fn get_metadata(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> Result<Option<BlobMetadata>, Error>;

    /// Writes a blob, replacing only a blob already at `path`.
    ///
    /// A directory marker at the same path is preserved. A root path returns [`BlobNameError`].
    async fn put_raw(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
        data: &[u8],
    ) -> Result<(), Error>;

    /// Streams a blob into storage with the same path rules as [`BlobStorage::put_raw`].
    async fn put_stream(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
        stream: &dyn ErasedReplayableStream<Item = Result<Vec<u8>, Error>, Error = Error>,
    ) -> Result<(), Error>;

    /// Deletes a blob without deleting a directory marker at the same path.
    ///
    /// A missing blob and a root path change nothing.
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

    /// Creates or refreshes an explicit directory marker.
    ///
    /// A blob at the same path is preserved. A root path changes nothing and stores no marker.
    async fn create_dir(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> Result<(), Error>;

    /// Lists blobs and explicit directory markers directly below `path`.
    ///
    /// Implicit directories that only contain blobs are omitted. A path can occur twice when it
    /// has both a blob and an explicit directory marker. The result order is unspecified.
    async fn list_dir(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> Result<Vec<PathBuf>, Error>;

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

    /// Reports a blob, an explicit or implicit directory, or a missing path.
    ///
    /// The namespace root is always a directory. A blob takes precedence when a blob and
    /// directory marker have the same path.
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
        validate_relative_blob_path(from)?;
        validate_relative_blob_path(to)?;
        reject_root_blob_path(from)?;
        reject_root_blob_path(to)?;
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

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("the blob storage has no blob at the path {path:?}")]
pub struct BlobMissingError {
    pub path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum BlobNameError {
    #[error("the blob path must be relative: {path:?}")]
    NotRelative { path: PathBuf },
    #[error("the blob path has a `..` name in it: {path:?}")]
    ParentDir { path: PathBuf },
    #[error("the blob path must be valid UTF-8: {path:?}")]
    NotUtf8 { path: PathBuf },
    #[error("the blob path has no name in it: {path:?}")]
    NoName { path: PathBuf },
}

pub(crate) fn validate_relative_blob_path(path: &Path) -> Result<(), Error> {
    if path.is_absolute() {
        return Err(BlobNameError::NotRelative {
            path: path.to_path_buf(),
        }
        .into());
    }

    for component in path.components() {
        match component {
            Component::Normal(_) | Component::CurDir => {}
            Component::ParentDir => {
                return Err(BlobNameError::ParentDir {
                    path: path.to_path_buf(),
                }
                .into());
            }
            Component::RootDir | Component::Prefix(_) => {
                return Err(BlobNameError::NotRelative {
                    path: path.to_path_buf(),
                }
                .into());
            }
        }
    }

    if path.to_str().is_none() {
        return Err(BlobNameError::NotUtf8 {
            path: path.to_path_buf(),
        }
        .into());
    }

    Ok(())
}

/// Tells if the path is at the root of a namespace.
///
/// A path is at the root when it has no name in it. An empty path is at the root, and so is a
/// path that only has `.` in it.
pub fn blob_path_is_root(path: &Path) -> bool {
    !path
        .components()
        .any(|component| matches!(component, Component::Normal(_)))
}

pub(crate) fn blob_path_to_string(path: &Path) -> Result<String, Error> {
    path.to_str().map(|s| s.to_string()).ok_or_else(|| {
        BlobNameError::NotUtf8 {
            path: path.to_path_buf(),
        }
        .into()
    })
}

pub(crate) fn blob_parent_to_string(path: &Path) -> Result<String, Error> {
    match path.parent() {
        Some(parent) => blob_path_to_string(parent),
        None => Ok(String::new()),
    }
}

pub(crate) fn blob_file_name_to_string(path: &Path) -> Result<String, Error> {
    path.file_name()
        .ok_or_else(|| {
            Error::from(BlobNameError::NoName {
                path: PathBuf::new(),
            })
        })
        .and_then(|name| {
            name.to_str().map(|s| s.to_string()).ok_or_else(|| {
                BlobNameError::NotUtf8 {
                    path: path.to_path_buf(),
                }
                .into()
            })
        })
}

pub(crate) fn reject_root_blob_path(path: &Path) -> Result<(), Error> {
    if blob_path_is_root(path) {
        Err(BlobNameError::NoName {
            path: PathBuf::new(),
        }
        .into())
    } else {
        Ok(())
    }
}
