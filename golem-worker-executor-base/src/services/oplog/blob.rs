// Copyright 2024-2025 Golem Cloud
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

use crate::error::GolemError;
use crate::services::oplog::multilayer::OplogArchive;
use crate::services::oplog::{CompressedOplogChunk, OplogArchiveService};
use async_lock::RwLockUpgradableReadGuard;
use async_trait::async_trait;
use evicting_cache_map::EvictingCacheMap;
use golem_common::model::oplog::{OplogEntry, OplogIndex};
use golem_common::model::{AccountId, ComponentId, OwnedWorkerId, ScanCursor, WorkerId};
use golem_service_base::storage::blob::{
    BlobStorage, BlobStorageLabelledApi, BlobStorageNamespace, ExistsResult,
};
use std::cmp::min;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::RwLock;

/// An oplog archive implementation that uses the configured blob storage to store compressed
/// chunks of the oplog.
#[derive(Debug)]
pub struct BlobOplogArchiveService {
    blob_storage: Arc<dyn BlobStorage + Send + Sync>,
    level: usize,
}

impl BlobOplogArchiveService {
    const CACHE_SIZE: usize = 4096;

    pub fn new(blob_storage: Arc<dyn BlobStorage + Send + Sync>, level: usize) -> Self {
        BlobOplogArchiveService {
            blob_storage,
            level,
        }
    }
}

#[async_trait]
impl OplogArchiveService for BlobOplogArchiveService {
    async fn open(&self, owned_worker_id: &OwnedWorkerId) -> Arc<dyn OplogArchive + Send + Sync> {
        Arc::new(
            BlobOplogArchive::new(
                owned_worker_id.clone(),
                self.blob_storage.clone(),
                self.level,
            )
            .await,
        )
    }

    async fn delete(&self, owned_worker_id: &OwnedWorkerId) {
        self.blob_storage
            .delete_dir(
                "blob_oplog",
                "delete",
                BlobStorageNamespace::CompressedOplog {
                    account_id: owned_worker_id.account_id(),
                    component_id: owned_worker_id.component_id(),
                    level: self.level,
                },
                Path::new(&owned_worker_id.worker_name()),
            )
            .await
            .unwrap_or_else(|err| {
                panic!(
                    "failed to drop compressed oplog for worker {} in blob storage: {err}",
                    owned_worker_id.worker_id
                )
            });
    }

    async fn read(
        &self,
        owned_worker_id: &OwnedWorkerId,
        idx: OplogIndex,
        n: u64,
    ) -> BTreeMap<OplogIndex, OplogEntry> {
        let archive = self.open(owned_worker_id).await;
        archive.read(idx, n).await
    }

    async fn exists(&self, owned_worker_id: &OwnedWorkerId) -> bool {
        self.blob_storage
            .with("blob_oplog", "exists")
            .exists(
                BlobStorageNamespace::CompressedOplog {
                    account_id: owned_worker_id.account_id(),
                    component_id: owned_worker_id.component_id(),
                    level: self.level,
                },
                Path::new(&owned_worker_id.worker_name()),
            )
            .await
            .map(|exists| exists == ExistsResult::Directory)
            .unwrap_or_else(|err| {
                panic!(
                    "failed to check existence of compressed oplog for worker {} in blob storage: {err}",
                    owned_worker_id.worker_id
                )
            })
    }

    async fn scan_for_component(
        &self,
        account_id: &AccountId,
        component_id: &ComponentId,
        cursor: ScanCursor,
        _count: u64,
    ) -> Result<(ScanCursor, Vec<OwnedWorkerId>), GolemError> {
        if cursor.cursor == 0 {
            let blob_storage = self.blob_storage.with("blob_oplog", "scan_for_component");
            let owned_worker_ids = if blob_storage.exists(
                BlobStorageNamespace::CompressedOplog {
                    account_id: account_id.clone(),
                    component_id: component_id.clone(),
                    level: self.level,
                },
                Path::new(""),
            ).await.map_err(|err| {
                GolemError::unknown(format!("Failed to check if compressed oplog root for component {component_id} exists in blob storage: {err}"))
            })? == ExistsResult::Directory
            {
                let paths = blob_storage
                    .list_dir(
                    BlobStorageNamespace::CompressedOplog {
                    account_id: account_id.clone(),
                    component_id: component_id.clone(),
                    level: self.level,
                },
                Path::new(""),
            ).await.map_err(|err| {
                GolemError::unknown(format!("Failed to list entries of compressed oplog for component {component_id} in blob storage: {err}"))
            })?;

                paths
                    .into_iter()
                    .map(|path| {
                        let worker_name = path.file_name().unwrap().to_str().unwrap();
                        OwnedWorkerId {
                            account_id: account_id.clone(),
                            worker_id: WorkerId {
                                component_id: component_id.clone(),
                                worker_name: worker_name.to_string(),
                            },
                        }
                    })
                    .collect()
            } else {
                Vec::new()
            };

            Ok((
                ScanCursor {
                    cursor: 0,
                    layer: cursor.layer,
                },
                owned_worker_ids,
            ))
        } else {
            Err(GolemError::unknown(
                "Cannot use cursor with blob oplog archive",
            ))
        }
    }

    async fn get_last_index(&self, owned_worker_id: &OwnedWorkerId) -> OplogIndex {
        if BlobOplogArchive::exists(
            owned_worker_id.clone(),
            self.blob_storage.clone(),
            self.level,
        )
        .await
        {
            let entries = BlobOplogArchive::entries(
                owned_worker_id.clone(),
                self.blob_storage.clone(),
                self.level,
            )
            .await;
            entries.keys().last().copied().unwrap_or(OplogIndex::NONE)
        } else {
            OplogIndex::NONE
        }
    }
}

#[derive(Debug)]
struct BlobOplogArchive {
    owned_worker_id: OwnedWorkerId,
    blob_storage: Arc<dyn BlobStorage + Send + Sync>,
    level: usize,
    entries: Arc<RwLock<BTreeMap<OplogIndex, PathBuf>>>,
    created: Arc<async_lock::RwLock<bool>>,
    #[allow(clippy::type_complexity)]
    cache: RwLock<
        EvictingCacheMap<
            OplogIndex,
            OplogEntry,
            { BlobOplogArchiveService::CACHE_SIZE },
            fn(OplogIndex, OplogEntry) -> (),
        >,
    >,
}

impl BlobOplogArchive {
    pub async fn new(
        owned_worker_id: OwnedWorkerId,
        blob_storage: Arc<dyn BlobStorage + Send + Sync>,
        level: usize,
    ) -> Self {
        let exists = Self::exists(owned_worker_id.clone(), blob_storage.clone(), level).await;
        let created = Arc::new(async_lock::RwLock::new(exists));
        let entries = Arc::new(RwLock::new(if exists {
            Self::entries(owned_worker_id.clone(), blob_storage.clone(), level).await
        } else {
            BTreeMap::new()
        }));

        BlobOplogArchive {
            owned_worker_id,
            blob_storage,
            level,
            created,
            entries,
            cache: RwLock::new(EvictingCacheMap::new()),
        }
    }

    async fn ensure_is_created(&self) {
        let created = self.created.upgradable_read().await;
        if !*created {
            let mut created = RwLockUpgradableReadGuard::upgrade(created).await;
            self.blob_storage
                .with("blob_oplog", "new")
                .create_dir(
                    BlobStorageNamespace::CompressedOplog {
                        account_id: self.owned_worker_id.account_id(),
                        component_id: self.owned_worker_id.component_id(),
                        level: self.level,
                    },
                    Path::new(&self.owned_worker_id.worker_name()),
                )
                .await
                .unwrap_or_else(|err| {
                    panic!(
                        "failed to create compressed oplog directory for worker {} in blob storage: {err}",
                        self.owned_worker_id.worker_id
                    )
                });

            *created = true;
        }
    }

    pub(crate) async fn exists(
        owned_worker_id: OwnedWorkerId,
        blob_storage: Arc<dyn BlobStorage + Send + Sync>,
        level: usize,
    ) -> bool {
        blob_storage
            .with("blob_oplog", "exists")
            .exists(
                BlobStorageNamespace::CompressedOplog {
                    account_id: owned_worker_id.account_id(),
                    component_id: owned_worker_id.component_id(),
                    level,
                },
                Path::new(&owned_worker_id.worker_name()),
            )
            .await
            .map(|exists| exists == ExistsResult::Directory)
            .unwrap_or_else(|err| {
                panic!(
                    "failed to check existence of compressed oplog for worker {} in blob storage: {err}",
                    owned_worker_id.worker_id
                )
            })
    }

    pub(crate) async fn entries(
        owned_worker_id: OwnedWorkerId,
        blob_storage: Arc<dyn BlobStorage + Send + Sync>,
        level: usize,
    ) -> BTreeMap<OplogIndex, PathBuf> {
        let paths = blob_storage
            .with("blob_oplog", "new")
            .list_dir(
                BlobStorageNamespace::CompressedOplog {
                    account_id: owned_worker_id.account_id(),
                    component_id: owned_worker_id.component_id(),
                    level,
                },
                Path::new(&owned_worker_id.worker_name()),
            )
            .await
            .unwrap_or_else(|err| {
                panic!(
                "failed to list entries of compressed oplog for worker {} in blob storage: {err}",
                owned_worker_id.worker_id
            )
            });

        paths
            .into_iter()
            .map(|path| {
                let idx = Self::path_to_oplog_index(&path);
                (idx, path)
            })
            .collect::<BTreeMap<OplogIndex, PathBuf>>()
    }

    pub(crate) fn path_to_oplog_index(path: &Path) -> OplogIndex {
        path.file_name()
            .and_then(|s| s.to_str())
            .and_then(|s| s.parse::<u64>().ok())
            .map(OplogIndex::from_u64)
            .unwrap_or_else(|| panic!("failed to parse oplog index from path: {path:?}"))
    }

    pub(crate) fn oplog_index_to_path(&self, idx: OplogIndex) -> PathBuf {
        let mut path = PathBuf::new();
        path.push(self.owned_worker_id.worker_name());
        path.push(idx.to_string());
        path
    }

    async fn read_and_cache_chunk(&self, idx: OplogIndex) -> Result<Option<OplogIndex>, String> {
        let entries = self.entries.read().await;
        let last_idx = entries.keys().find(|k| **k >= idx);
        if let Some(last_idx) = last_idx {
            let chunk: CompressedOplogChunk = self
                .blob_storage
                .with("blob_oplog", "read")
                .get(
                    BlobStorageNamespace::CompressedOplog {
                        account_id: self.owned_worker_id.account_id(),
                        component_id: self.owned_worker_id.component_id(),
                        level: self.level,
                    },
                    &self.oplog_index_to_path(*last_idx),
                )
                .await?
                .ok_or(format!("compressed chunk for {last_idx} not found"))?;

            let entries = chunk.decompress()?;
            let mut cache = self.cache.write().await;

            let mut idx = Into::<u64>::into(*last_idx) - chunk.count + 1;
            for entry in entries {
                cache.insert(OplogIndex::from_u64(idx), entry);
                idx += 1;
            }

            Ok(Some(*last_idx))
        } else {
            Ok(None)
        }
    }
}

#[async_trait]
impl OplogArchive for BlobOplogArchive {
    async fn read(&self, idx: OplogIndex, n: u64) -> BTreeMap<OplogIndex, OplogEntry> {
        let owned_worker_id = &self.owned_worker_id;

        let mut result = BTreeMap::new();
        let mut last_idx = idx.range_end(n);
        let mut before = OplogIndex::from_u64(u64::MAX);

        while last_idx >= idx {
            {
                let mut cache = self.cache.write().await;

                while let Some(entry) = cache.get(&last_idx) {
                    result.insert(last_idx, entry.clone());
                    if last_idx == idx {
                        break;
                    } else {
                        last_idx = last_idx.previous();
                    }
                }
                drop(cache);
            }

            if before == last_idx {
                // No entries found in cache, even though fetch returned true. This means we reached the beginning of the stream
                break;
            }

            if result.len() == (n as usize) {
                // We are done fetching all the results
                break;
            }

            let fetched_last_idx = self.read_and_cache_chunk(last_idx).await.unwrap_or_else(|err| {
                panic!("failed to read compressed oplog for worker {owned_worker_id} in blob storage: {err}")
            });
            if fetched_last_idx.is_some() {
                before = last_idx;
            } else if result.is_empty() {
                // We allow to have a gap on the right side of the query - as we cannot guarantee
                // that the 'n' parameter is exactly matches the available number of elements. However,
                // there must not be any gaps in the middle.
                let entries = self.entries.read().await;
                if let Some((idx, _)) = entries.last_key_value() {
                    last_idx = min(last_idx, *idx);
                } else {
                    break;
                }
            } else {
                // We never go towards older entries so if we didn't fetch the chunk we reached the
                // boundary of this layer
                break;
            }
        }

        result
    }

    async fn append(&self, chunk: Vec<(OplogIndex, OplogEntry)>) {
        self.ensure_is_created().await;

        if let Some(last) = chunk.last() {
            let oplog_index = last.0;
            let path = self.oplog_index_to_path(oplog_index);

            let chunk = chunk.into_iter().map(|(_, entry)| entry).collect();
            let compressed_chunk = CompressedOplogChunk::compress(chunk)
                .unwrap_or_else(|err| panic!("failed to compress oplog chunk: {err}"));

            let mut entries = self.entries.write().await;
            self.blob_storage.with(
                "blob_oplog",
                "append").put(
                BlobStorageNamespace::CompressedOplog {
                    account_id: self.owned_worker_id.account_id(),
                    component_id: self.owned_worker_id.component_id(),
                    level: self.level
                },
                &path,
                &compressed_chunk,
            ).await.unwrap_or_else(|err| {
                panic!(
                    "failed to store compressed oplog chunk for worker {} in blob storage: {err}", self.owned_worker_id.worker_id
                )
            });
            entries.insert(oplog_index, path);
        }
    }

    async fn current_oplog_index(&self) -> OplogIndex {
        let entries = self.entries.read().await;
        entries
            .keys()
            .last()
            .copied()
            .unwrap_or_else(|| OplogIndex::from_u64(0))
    }

    async fn drop_prefix(&self, last_dropped_id: OplogIndex) {
        self.ensure_is_created().await;

        let mut entries = self.entries.write().await;
        let mut cache = self.cache.write().await;

        let idx_to_evict = cache
            .iter()
            .filter(|(idx, _)| **idx <= last_dropped_id)
            .map(|(idx, _)| *idx)
            .collect::<Vec<_>>();

        for idx in idx_to_evict {
            cache.remove(&idx);
        }

        let idx_to_drop = entries
            .keys()
            .filter(|key| **key <= last_dropped_id)
            .cloned()
            .collect::<Vec<_>>();

        let to_drop = idx_to_drop
            .iter()
            .map(|idx| {
                let mut path = PathBuf::new();
                path.push(self.owned_worker_id.worker_name());
                path.push(idx.to_string());
                path
            })
            .collect::<Vec<_>>();

        let ns = BlobStorageNamespace::CompressedOplog {
            account_id: self.owned_worker_id.account_id(),
            component_id: self.owned_worker_id.component_id(),
            level: self.level,
        };

        self.blob_storage
            .with("blob_oplog", "drop_prefix")
            .delete_many(ns, &to_drop)
            .await
            .unwrap_or_else(|err| {
                panic!(
                    "failed to drop compressed oplog chunks for worker {} in blob storage: {err}",
                    self.owned_worker_id.worker_id
                )
            });

        for idx in idx_to_drop {
            let _ = entries.remove(&idx);
        }

        if entries.len() == 0 {
            let mut created = self.created.write().await;
            if *created {
                self.blob_storage
                .with("blob_oplog", "drop_prefix")
                .delete_dir(BlobStorageNamespace::CompressedOplog {
                    account_id: self.owned_worker_id.account_id(),
                    component_id: self.owned_worker_id.component_id(),
                    level: self.level,
                },
                Path::new(&self.owned_worker_id.worker_name())).await.unwrap_or_else(|err| {
                    panic!(
                        "failed to drop compressed oplog directory for worker {} in blob storage: {err}",
                        self.owned_worker_id.worker_id
                    )
                });
                *created = false;
            }
        }
    }

    async fn length(&self) -> u64 {
        let entries = self.entries.read().await;
        entries.len() as u64
    }

    async fn get_last_index(&self) -> OplogIndex {
        self.current_oplog_index().await
    }
}
