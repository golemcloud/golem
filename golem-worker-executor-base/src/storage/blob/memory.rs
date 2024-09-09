// Copyright 2024 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use crate::storage::blob::{BlobMetadata, BlobStorage, BlobStorageNamespace, ExistsResult};
use async_trait::async_trait;
use bytes::Bytes;
use dashmap::DashMap;
use golem_common::model::Timestamp;
use std::path::{Path, PathBuf};

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
            if let Some(directory) = namespace_data.get(&dir) {
                let mut result: Vec<PathBuf> = directory
                    .iter()
                    .map(|entry| {
                        let mut path = path.to_path_buf();
                        path.push(entry.key());
                        path
                    })
                    .collect();
                drop(directory);

                namespace_data
                    .iter()
                    .filter(|entry| entry.key() != &dir && entry.key().starts_with(&dir))
                    .for_each(|entry| {
                        result.push(Path::new(entry.key()).to_path_buf());
                    });

                Ok(result)
            } else {
                Ok(vec![])
            }
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
    ) -> Result<(), String> {
        let dir = path.to_string_lossy().to_string();
        self.data
            .get(&namespace)
            .and_then(|namespace_data| namespace_data.remove(&dir));
        Ok(())
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
            .map(|namespace_data| namespace_data.contains_key(path.to_string_lossy().as_ref()))
            .unwrap_or_default()
        {
            return Ok(ExistsResult::Directory);
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
