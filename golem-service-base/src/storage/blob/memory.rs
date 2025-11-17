// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
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
use crate::storage::blob::{BlobMetadata, BlobStorage, BlobStorageNamespace, ExistsResult};
use anyhow::Error;
use async_trait::async_trait;
use bytes::Bytes;
use futures::stream::BoxStream;
use futures::{Stream, TryStreamExt};
use golem_common::model::Timestamp;
use std::collections::HashSet;
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
        files: HashSet<String>,
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
        let dir = path
            .parent()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default();
        let key = path
            .file_name()
            .expect("Path must have a file name")
            .to_string_lossy()
            .to_string();

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
        let dir = path
            .parent()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default();
        let file = path
            .file_name()
            .expect("Path must have a file name")
            .to_string_lossy()
            .to_string();

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
        let dir = path
            .parent()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default();
        let file = path
            .file_name()
            .expect("Path must have a file name")
            .to_string_lossy()
            .to_string();

        let key = Key {
            namespace,
            dir,
            file: Some(file),
        };

        Ok(self
            .data
            .read_async(&key, |_, entry| match entry {
                Entry::File { metadata, .. } => Some(metadata.clone()),
                _ => None,
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
        let dir = path
            .parent()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default();
        let file = path
            .file_name()
            .expect("Path must have a file name")
            .to_string_lossy()
            .to_string();

        let key = Key {
            namespace: namespace.clone(),
            dir: dir.clone(),
            file: Some(file.clone()),
        };

        let parent_key = Key {
            namespace,
            dir,
            file: None,
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

        let file2 = file.clone();
        self.data
            .entry_async(parent_key)
            .await
            .and_modify(|entry| {
                if let Entry::Directory { files } = entry {
                    files.insert(file);
                }
            })
            .or_insert_with(|| Entry::Directory {
                files: HashSet::from_iter(vec![file2]),
            });

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
        let dir = path
            .parent()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default();
        let file = path
            .file_name()
            .expect("Path must have a file name")
            .to_string_lossy()
            .to_string();

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

        let parent_key = Key {
            namespace,
            dir,
            file: None,
        };

        self.data.upsert_async(key, entry).await;

        let file2 = file.clone();
        self.data
            .entry_async(parent_key)
            .await
            .and_modify(|entry| {
                if let Entry::Directory { files } = entry {
                    files.insert(file);
                }
            })
            .or_insert_with(|| Entry::Directory {
                files: HashSet::from_iter(vec![file2]),
            });

        Ok(())
    }

    async fn delete(
        &self,
        _target_label: &'static str,
        _op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> Result<(), Error> {
        let dir = path
            .parent()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default();
        let file = path
            .file_name()
            .expect("Path must have a file name")
            .to_string_lossy()
            .to_string();

        let key = Key {
            namespace: namespace.clone(),
            dir: dir.clone(),
            file: Some(file.clone()),
        };

        let parent_key = Key {
            namespace,
            dir,
            file: None,
        };

        self.data
            .update_async(&parent_key, |_, entry| {
                if let Entry::Directory { files } = entry {
                    files.remove(&file);
                }
            })
            .await;
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
        let dir = path.to_string_lossy().to_string();

        let key = Key {
            namespace: namespace.clone(),
            dir: dir.clone(),
            file: None,
        };

        let entry = Entry::Directory {
            files: HashSet::new(),
        };
        self.data.upsert_async(key, entry).await;

        let parent = path
            .parent()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default();
        let name = path
            .file_name()
            .expect("Path must have a file name")
            .to_string_lossy()
            .to_string();

        let parent_key = Key {
            namespace,
            dir: parent,
            file: None,
        };

        let name2 = name.clone();
        self.data
            .entry_async(parent_key)
            .await
            .and_modify(|entry| {
                if let Entry::Directory { files } = entry {
                    files.insert(name);
                }
            })
            .or_insert_with(|| Entry::Directory {
                files: HashSet::from_iter(vec![name2]),
            });

        Ok(())
    }

    async fn list_dir(
        &self,
        _target_label: &'static str,
        _op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> Result<Vec<PathBuf>, Error> {
        let dir = path.to_string_lossy().to_string();

        let key = Key {
            namespace,
            dir,
            file: None,
        };

        let files = self
            .data
            .read_async(&key, |_, entry| match entry {
                Entry::Directory { files, .. } => files.clone(),
                _ => HashSet::new(),
            })
            .await
            .unwrap_or_default();

        Ok(files.into_iter().map(|f| path.join(f)).collect())
    }

    async fn delete_dir(
        &self,
        _target_label: &'static str,
        _op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> Result<bool, Error> {
        let dir = path.to_string_lossy().to_string();

        let key = Key {
            namespace,
            dir: dir.clone(),
            file: None,
        };

        let result = self.data.remove_async(&key).await;

        if let Some((_, entry)) = result {
            match entry {
                Entry::Directory { files } => {
                    self.data
                        .retain_async(|k, _| {
                            if k.dir == dir {
                                !files.contains(&k.file.clone().unwrap_or_default())
                            } else {
                                true
                            }
                        })
                        .await;
                    Ok(true)
                }
                _ => Ok(false),
            }
        } else {
            Ok(false)
        }
    }

    async fn exists(
        &self,
        _target_label: &'static str,
        _op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> Result<ExistsResult, Error> {
        let path_str = path.to_string_lossy().to_string();
        let dir_key = Key {
            namespace: namespace.clone(),
            dir: path_str,
            file: None,
        };
        if self.data.contains_async(&dir_key).await {
            Ok(ExistsResult::Directory)
        } else if let Some(dir) = path.parent() {
            let dir = dir.to_string_lossy().to_string();

            if let Some(file) = path.file_name() {
                let file = file.to_string_lossy().to_string();
                let file_key = Key {
                    namespace,
                    dir,
                    file: Some(file),
                };

                if self.data.contains_async(&file_key).await {
                    Ok(ExistsResult::File)
                } else {
                    Ok(ExistsResult::DoesNotExist)
                }
            } else {
                Ok(ExistsResult::DoesNotExist)
            }
        } else {
            Ok(ExistsResult::DoesNotExist)
        }
    }
}
