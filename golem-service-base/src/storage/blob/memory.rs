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
    BlobMetadata, BlobStorage, BlobStorageNamespace, ExistsResult, blob_file_name_to_string,
    blob_parent_to_string, blob_path_is_root, blob_path_to_string, reject_root_blob_path,
    validate_relative_blob_path,
};
use anyhow::Error;
use async_trait::async_trait;
use bytes::Bytes;
use futures::stream::BoxStream;
use futures::{Stream, TryStreamExt};
use golem_common::model::Timestamp;
use std::{
    path::{Path, PathBuf},
    pin::Pin,
};

#[derive(Debug, Clone, Eq, PartialEq, Hash)]
struct Key {
    namespace: BlobStorageNamespace,
    dir: String,
    file: Option<String>,
}

#[derive(Debug, Clone)]
enum Entry {
    Directory {
        created_at: Timestamp,
    },
    File {
        data: Vec<u8>,
        metadata: BlobMetadata,
    },
}

#[derive(Debug)]
pub struct InMemoryBlobStorage {
    data: scc::HashMap<Key, Entry>,
}

impl Default for InMemoryBlobStorage {
    fn default() -> Self {
        Self::new()
    }
}

impl InMemoryBlobStorage {
    pub fn new() -> Self {
        Self {
            data: scc::HashMap::new(),
        }
    }
}

#[async_trait]
impl BlobStorage for InMemoryBlobStorage {
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
        let dir = blob_parent_to_string(path)?;
        let key = blob_file_name_to_string(path)?;

        let key = Key {
            namespace,
            dir,
            file: Some(key),
        };

        Ok(self
            .data
            .read_async(&key, |_, entry| match entry {
                Entry::File { data, .. } => Some(data.clone()),
                _ => None,
            })
            .await
            .flatten())
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
        let dir = blob_parent_to_string(path)?;
        let file = blob_file_name_to_string(path)?;

        let key = Key {
            namespace,
            dir,
            file: Some(file),
        };

        Ok(self
            .data
            .read_async(&key, |_, entry| match entry {
                Entry::File { data, .. } => {
                    let stream = tokio_stream::once(Ok(Bytes::from(data.clone())));
                    let boxed: Pin<Box<dyn Stream<Item = Result<Bytes, Error>> + Send>> =
                        Box::pin(stream);
                    Some(boxed)
                }
                _ => None,
            })
            .await
            .flatten())
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
        let dir = blob_parent_to_string(path)?;
        let file = blob_file_name_to_string(path)?;

        let key = Key {
            namespace: namespace.clone(),
            dir,
            file: Some(file),
        };

        let blob = self
            .data
            .read_async(&key, |_, entry| match entry {
                Entry::File { metadata, .. } => Some(metadata.clone()),
                _ => None,
            })
            .await
            .flatten();
        if blob.is_some() {
            return Ok(blob);
        }

        let directory = Key {
            namespace,
            dir: blob_path_to_string(path)?,
            file: None,
        };
        Ok(self
            .data
            .read_async(&directory, |_, entry| match entry {
                Entry::Directory { created_at } => Some(BlobMetadata {
                    last_modified_at: *created_at,
                    size: 0,
                }),
                Entry::File { .. } => None,
            })
            .await
            .flatten())
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
        let dir = blob_parent_to_string(path)?;
        let file = blob_file_name_to_string(path)?;

        let key = Key {
            namespace: namespace.clone(),
            dir: dir.clone(),
            file: Some(file.clone()),
        };

        let size = data.len() as u64;
        let entry = Entry::File {
            data: data.to_vec(),
            metadata: BlobMetadata {
                size,
                last_modified_at: Timestamp::now_utc(),
            },
        };

        self.data.upsert_async(key, entry).await;

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
        let dir = blob_parent_to_string(path)?;
        let file = blob_file_name_to_string(path)?;

        let stream = stream.make_stream_erased().await?;
        let data = stream.try_collect::<Vec<_>>().await?.concat();
        let size = data.len() as u64;
        let entry = Entry::File {
            data,
            metadata: BlobMetadata {
                size,
                last_modified_at: Timestamp::now_utc(),
            },
        };

        let key = Key {
            namespace: namespace.clone(),
            dir: dir.clone(),
            file: Some(file.clone()),
        };

        self.data.upsert_async(key, entry).await;

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
        let dir = blob_parent_to_string(path)?;
        let file = blob_file_name_to_string(path)?;

        let key = Key {
            namespace: namespace.clone(),
            dir: dir.clone(),
            file: Some(file.clone()),
        };

        self.data.remove_async(&key).await;
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
        let dir = blob_path_to_string(path)?;

        let key = Key {
            namespace: namespace.clone(),
            dir: dir.clone(),
            file: None,
        };

        let entry = Entry::Directory {
            created_at: Timestamp::now_utc(),
        };
        self.data.upsert_async(key, entry).await;

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
        let dir = blob_path_to_string(path)?;

        let mut entries = Vec::new();
        self.data
            .iter_async(|key, entry| {
                if key.namespace == namespace {
                    if key.dir == dir {
                        if let Some(file) = &key.file {
                            entries.push(path.join(file));
                        }
                    } else if key.file.is_none()
                        && matches!(entry, Entry::Directory { .. })
                        && Path::new(&key.dir).parent() == Some(path)
                    {
                        entries.push(PathBuf::from(&key.dir));
                    }
                }
                true
            })
            .await;

        Ok(entries)
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

        let dir = blob_path_to_string(path)?;

        let nested = format!("{dir}/");
        let mut deleted = false;
        self.data
            .retain_async(|below, _| {
                let in_directory = below.namespace == namespace
                    && (below.dir == dir || below.dir.starts_with(&nested));
                if in_directory {
                    deleted = true;
                }
                !in_directory
            })
            .await;

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
        let path_str = blob_path_to_string(path)?;
        let file_key = Key {
            namespace: namespace.clone(),
            dir: blob_parent_to_string(path)?,
            file: Some(blob_file_name_to_string(path)?),
        };
        if self.data.contains_async(&file_key).await {
            return Ok(ExistsResult::File);
        }
        let nested = format!("{path_str}/");
        if self
            .data
            .any_async(|key, _| {
                key.namespace == namespace && (key.dir == path_str || key.dir.starts_with(&nested))
            })
            .await
            .is_some()
        {
            Ok(ExistsResult::Directory)
        } else {
            Ok(ExistsResult::DoesNotExist)
        }
    }
}
