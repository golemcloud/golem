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
use async_trait::async_trait;
use bytes::Bytes;
use dashmap::DashMap;
use futures::stream::BoxStream;
use futures::{Stream, TryStreamExt};
use golem_common::model::Timestamp;
use std::{
    path::{Path, PathBuf},
    pin::Pin,
};

#[derive(Debug)]
pub struct InMemoryBlobStorage {
    data: DashMap<BlobStorageNamespace, DashMap<String, DashMap<String, Entry>>>,
}

#[derive(Debug)]
struct Entry {
    data: Bytes,
    metadata: BlobMetadata,
}

impl Default for InMemoryBlobStorage {
    fn default() -> Self {
        Self::new()
    }
}

impl InMemoryBlobStorage {
    pub fn new() -> Self {
        Self {
            data: DashMap::new(),
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
    ) -> Result<Option<Bytes>, String> {
        let dir = path
            .parent()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default();
        let key = path
            .file_name()
            .expect("Path must have a file name")
            .to_string_lossy()
            .to_string();
        Ok(self.data.get(&namespace).and_then(|namespace_data| {
            namespace_data
                .get(&dir)
                .and_then(|directory| directory.get(&key).map(|entry| entry.data.clone()))
        }))
    }

    async fn get_stream(
        &self,
        _target_label: &'static str,
        _op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> Result<Option<BoxStream<'static, Result<Bytes, String>>>, String> {
        let dir = path
            .parent()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default();
        let key = path
            .file_name()
            .expect("Path must have a file name")
            .to_string_lossy()
            .to_string();

        let maybe_stream = self.data.get(&namespace).and_then(|namespace_data| {
            namespace_data.get(&dir).and_then(|directory| {
                directory.get(&key).map(|entry| {
                    let data = entry.data.clone();
                    let stream = tokio_stream::once(Ok(data));
                    let boxed: Pin<Box<dyn Stream<Item = Result<Bytes, String>> + Send>> =
                        Box::pin(stream);
                    boxed
                })
            })
        });
        Ok(maybe_stream)
    }

    async fn get_metadata(
        &self,
        _target_label: &'static str,
        _op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> Result<Option<BlobMetadata>, String> {
        let dir = path
            .parent()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default();
        let key = path
            .file_name()
            .expect("Path must have a file name")
            .to_string_lossy()
            .to_string();
        Ok(self.data.get(&namespace).and_then(|namespace_data| {
            namespace_data
                .get(&dir)
                .and_then(|directory| directory.get(&key).map(|entry| entry.metadata.clone()))
        }))
    }

    async fn put_raw(
        &self,
        _target_label: &'static str,
        _op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
        data: &[u8],
    ) -> Result<(), String> {
        let dir = path
            .parent()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default();
        let key = path
            .file_name()
            .expect("Path must have a file name")
            .to_string_lossy()
            .to_string();
        let entry = Entry {
            data: Bytes::copy_from_slice(data),
            metadata: BlobMetadata {
                size: data.len() as u64,
                last_modified_at: Timestamp::now_utc(),
            },
        };
        self.data
            .entry(namespace)
            .or_default()
            .entry(dir)
            .or_default()
            .insert(key, entry);
        Ok(())
    }

    async fn put_stream(
        &self,
        _target_label: &'static str,
        _op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
        stream: &dyn ErasedReplayableStream<Item = Result<Bytes, String>, Error = String>,
    ) -> Result<(), String> {
        let dir = path
            .parent()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default();

        let key = path
            .file_name()
            .expect("Path must have a file name")
            .to_string_lossy()
            .to_string();

        let stream = stream.make_stream_erased().await?;
        let data = stream
            .try_collect::<Vec<_>>()
            .await
            .map_err(|e| e.to_string())?;
        let entry = Entry {
            data: Bytes::from(data.concat()),
            metadata: BlobMetadata {
                size: data.iter().map(|b| b.len() as u64).sum(),
                last_modified_at: Timestamp::now_utc(),
            },
        };

        self.data
            .entry(namespace)
            .or_default()
            .entry(dir)
            .or_default()
            .insert(key, entry);

        Ok(())
    }

    async fn delete(
        &self,
        _target_label: &'static str,
        _op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> Result<(), String> {
        let dir = path
            .parent()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default();
        let key = path
            .file_name()
            .expect("Path must have a file name")
            .to_string_lossy()
            .to_string();
        if let Some(namespace_data) = self.data.get(&namespace) {
            if let Some(directory) = namespace_data.get(&dir) {
                directory.remove(&key);
            }
        }

        Ok(())
    }

    async fn create_dir(
        &self,
        _target_label: &'static str,
        _op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> Result<(), String> {
        let dir = path.to_string_lossy().to_string();
        self.data
            .entry(namespace)
            .or_default()
            .entry(dir)
            .or_default();
        Ok(())
    }

    async fn list_dir(
        &self,
        _target_label: &'static str,
        _op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> Result<Vec<PathBuf>, String> {
        let dir = path.to_string_lossy().to_string();

        if let Some(namespace_data) = self.data.get(&namespace) {
            let mut result: Vec<PathBuf> = Vec::new();
            if let Some(directory) = namespace_data.get(&dir) {
                let file_result: Vec<PathBuf> = directory
                    .iter()
                    .map(|entry| {
                        let mut path = path.to_path_buf();
                        path.push(entry.key());
                        path
                    })
                    .collect();
                result.extend(file_result);
                drop(directory);
            }
            let prefix = if dir.ends_with('/') || dir.is_empty() {
                dir.to_string()
            } else {
                format!("{}/", dir)
            };
            namespace_data
                .iter()
                .filter(|entry| entry.key() != &dir && entry.key().starts_with(&prefix))
                .for_each(|entry| {
                    result.push(Path::new(entry.key()).to_path_buf());
                });

            Ok(result)
        } else {
            Ok(vec![])
        }
    }

    async fn delete_dir(
        &self,
        _target_label: &'static str,
        _op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> Result<bool, String> {
        let dir = path.to_string_lossy().to_string();
        let result = self
            .data
            .get(&namespace)
            .and_then(|namespace_data| namespace_data.remove(&dir));
        Ok(result.is_some())
    }

    async fn exists(
        &self,
        _target_label: &'static str,
        _op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> Result<ExistsResult, String> {
        if self
            .data
            .get(&namespace)
            .map(|namespace_data| {
                namespace_data.contains_key::<str>(path.to_string_lossy().as_ref())
            })
            .unwrap_or_default()
        {
            Ok(ExistsResult::Directory)
        } else if path == Path::new("") {
            if self.data.get(&namespace).is_some() {
                Ok(ExistsResult::Directory)
            } else {
                Ok(ExistsResult::DoesNotExist)
            }
        } else {
            let dir = path
                .parent()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_default();
            let key = path
                .file_name()
                .expect("Path must have a file name")
                .to_string_lossy()
                .to_string();

            if let Some(namespace_data) = self.data.get(&namespace) {
                if let Some(directory) = namespace_data.get(&dir) {
                    if directory.contains_key(&key) {
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
}
