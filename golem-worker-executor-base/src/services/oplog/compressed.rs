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

use std::cmp::min;
use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;
use bincode::{Decode, Encode};
use evicting_cache_map::EvictingCacheMap;
use tokio::sync::RwLock;

use crate::error::GolemError;
use golem_common::model::oplog::{OplogEntry, OplogIndex};
use golem_common::model::{AccountId, ComponentId, OwnedWorkerId, ScanCursor, WorkerId};
use golem_common::serialization::{deserialize, serialize};

use crate::services::oplog::multilayer::{OplogArchive, OplogArchiveService};
use crate::services::oplog::PrimaryOplogService;
use crate::storage::indexed::{IndexedStorage, IndexedStorageLabelledApi, IndexedStorageNamespace};

#[derive(Debug)]
pub struct CompressedOplogArchiveService {
    indexed_storage: Arc<dyn IndexedStorage + Send + Sync>,
    level: usize,
}

impl CompressedOplogArchiveService {
    const CACHE_SIZE: usize = 4096;
    const ZSTD_LEVEL: i32 = 0;

    pub fn new(indexed_storage: Arc<dyn IndexedStorage + Send + Sync>, level: usize) -> Self {
        Self {
            indexed_storage,
            level,
        }
    }

    fn compressed_oplog_key(worker_id: &WorkerId) -> String {
        worker_id.to_redis_key()
    }
}

#[async_trait]
impl OplogArchiveService for CompressedOplogArchiveService {
    async fn open(&self, owned_worker_id: &OwnedWorkerId) -> Arc<dyn OplogArchive + Send + Sync> {
        Arc::new(CompressedOplogArchive::new(
            owned_worker_id.worker_id(),
            self.indexed_storage.clone(),
            self.level,
        ))
    }

    async fn delete(&self, owned_worker_id: &OwnedWorkerId) {
        self.indexed_storage
            .with("compressed_oplog", "delete")
            .delete(IndexedStorageNamespace::CompressedOpLog { level: self.level }, &Self::compressed_oplog_key(&owned_worker_id.worker_id))
            .await
            .unwrap_or_else(|err| {
                panic!("failed to drop compressed oplog for worker {owned_worker_id} in indexed storage: {err}")
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
        self.indexed_storage
            .with("compressed_oplog", "exists")
            .exists(
                IndexedStorageNamespace::CompressedOpLog { level: self.level },
                &Self::compressed_oplog_key(&owned_worker_id.worker_id),
            )
            .await
            .unwrap_or_else(|err| {
                panic!("failed to check if compressed oplog exists for worker {owned_worker_id} in indexed storage: {err}")
            })
    }

    async fn scan_for_component(
        &self,
        account_id: &AccountId,
        component_id: &ComponentId,
        cursor: ScanCursor,
        count: u64,
    ) -> Result<(ScanCursor, Vec<OwnedWorkerId>), GolemError> {
        let ScanCursor { cursor, layer } = cursor;
        let (cursor, keys) = self
            .indexed_storage
            .with("compressed_oplog", "scan")
            .scan(
                IndexedStorageNamespace::CompressedOpLog { level: self.level },
                &PrimaryOplogService::key_pattern(component_id),
                cursor,
                count,
            )
            .await
            .unwrap_or_else(|err| {
                panic!("failed to scan for component {component_id} in indexed storage: {err}")
            });

        Ok((
            ScanCursor { cursor, layer },
            keys.into_iter()
                .map(|key| OwnedWorkerId {
                    worker_id: PrimaryOplogService::get_worker_id_from_key(&key, component_id),
                    account_id: account_id.clone(),
                })
                .collect(),
        ))
    }

    async fn get_last_index(&self, owned_worker_id: &OwnedWorkerId) -> OplogIndex {
        let key = Self::compressed_oplog_key(&owned_worker_id.worker_id);
        OplogIndex::from_u64(
            self.indexed_storage
                .with_entity("compressed_oplog", "current_oplog_index", "compressed_entry")
                .last_id(IndexedStorageNamespace::CompressedOpLog { level: self.level }, &key)
                .await
                .unwrap_or_else(|err| {
                    panic!("failed to get last entry from compressed oplog for worker {owned_worker_id} in indexed storage: {err}")
                }).unwrap_or_default(),
        )
    }
}

#[derive(Debug)]
pub struct CompressedOplogArchive {
    worker_id: WorkerId,
    key: String,
    indexed_storage: Arc<dyn IndexedStorage + Send + Sync>,
    #[allow(clippy::type_complexity)]
    cache: RwLock<
        EvictingCacheMap<
            OplogIndex,
            OplogEntry,
            { CompressedOplogArchiveService::CACHE_SIZE },
            fn(OplogIndex, OplogEntry) -> (),
        >,
    >,
    level: usize,
}

impl CompressedOplogArchive {
    pub fn new(
        worker_id: WorkerId,
        indexed_storage: Arc<dyn IndexedStorage + Send + Sync>,
        level: usize,
    ) -> Self {
        let key = CompressedOplogArchiveService::compressed_oplog_key(&worker_id);
        Self {
            worker_id,
            key,
            indexed_storage,
            cache: RwLock::new(EvictingCacheMap::new()),
            level,
        }
    }

    async fn read_and_cache_chunk(&self, idx: OplogIndex) -> Result<Option<OplogIndex>, String> {
        if let Some((last_idx, chunk)) = self
            .indexed_storage
            .with_entity("compressed_oplog", "read", "compressed_entry")
            .closest::<CompressedOplogChunk>(
                IndexedStorageNamespace::CompressedOpLog { level: self.level },
                &self.key,
                idx.into(),
            )
            .await?
        {
            let entries = chunk.decompress()?;
            let mut cache = self.cache.write().await;

            let mut idx = last_idx - chunk.count + 1;
            for entry in entries {
                cache.insert(OplogIndex::from_u64(idx), entry);
                idx += 1;
            }

            Ok(Some(OplogIndex::from_u64(last_idx)))
        } else {
            Ok(None)
        }
    }
}

/// Currently only the background-transfer fiber calls `append` and `drop_prefix` on oplog archives,
/// so here it is not protected by a lock. If this changes, we need to add a lock here, similar
/// to the `PrimaryOplog` implementation.
#[async_trait]
impl OplogArchive for CompressedOplogArchive {
    async fn read(
        &self,
        idx: OplogIndex,
        n: u64,
    ) -> BTreeMap<golem_common::model::oplog::OplogIndex, OplogEntry> {
        let worker_id = &self.worker_id;

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
                panic!("failed to read compressed oplog for worker {worker_id} in indexed storage: {err}")
            });
            if fetched_last_idx.is_some() {
                before = last_idx;
            } else if result.is_empty() {
                // We allow to have a gap on the right side of the query - as we cannot guarantee
                // that the 'n' parameter is exactly matches the available number of elements. However,
                // there must not be any gaps in the middle.
                if let Some(idx) = self.indexed_storage.with_entity("compressed_oplog", "read", "compressed_entry")
                    .last_id(IndexedStorageNamespace::CompressedOpLog { level: self.level }, &self.key)
                    .await
                    .unwrap_or_else(|err| {
                        panic!("failed to get first entry from compressed oplog for worker {worker_id} in indexed storage: {err}")
                    }) {
                    last_idx = min(last_idx, OplogIndex::from_u64(idx));
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
        if !chunk.is_empty() {
            let worker_id = &self.worker_id;

            let mut cache = self.cache.write().await;
            for (idx, entry) in &chunk {
                cache.insert(*idx, entry.clone());
            }

            let last_id = chunk.last().unwrap().0;
            let chunk = chunk.into_iter().map(|(_, entry)| entry).collect();
            let compressed_chunk = CompressedOplogChunk::compress(chunk)
                .unwrap_or_else(|err| panic!("failed to compress oplog chunk: {err}"));

            self.indexed_storage
                .with_entity("compressed_oplog", "append", "compressed_entry")
                .append(
                    IndexedStorageNamespace::CompressedOpLog { level: self.level },
                    &self.key,
                    last_id.into(),
                    &compressed_chunk,
                )
                .await
                .unwrap_or_else(|err| {
                    panic!("failed to append compressed oplog chunk for worker {worker_id} in indexed storage: {err}")
                });
        }
    }

    async fn current_oplog_index(&self) -> OplogIndex {
        let worker_id = &self.worker_id;
        OplogIndex::from_u64(
            self.indexed_storage
                .with_entity("compressed_oplog", "current_oplog_index", "compressed_entry")
                .last_id(IndexedStorageNamespace::CompressedOpLog { level: self.level }, &self.key)
                .await
                .unwrap_or_else(|err| {
                    panic!("failed to get the last entry from compressed oplog for worker {worker_id} in indexed storage: {err}")
                }).unwrap_or_default(),
        )
    }

    async fn drop_prefix(&self, last_dropped_id: OplogIndex) {
        let mut cache = self.cache.write().await;

        let idx_to_evict = cache
            .iter()
            .filter(|(idx, _)| **idx <= last_dropped_id)
            .map(|(idx, _)| *idx)
            .collect::<Vec<_>>();

        for idx in idx_to_evict {
            cache.remove(&idx);
        }

        let worker_id = &self.worker_id;
        self.indexed_storage.with("compressed_oplog", "drop_prefix")
            .drop_prefix(IndexedStorageNamespace::CompressedOpLog { level: self.level }, &self.key, last_dropped_id.into())
            .await
            .unwrap_or_else(|err| {
                panic!("failed to drop prefix from compressed oplog for worker {worker_id} in indexed storage: {err}")
            });
        let remaining = self.length().await;
        if remaining == 0 {
            self.indexed_storage.with("compressed_oplog", "drop_prefix")
                .delete(IndexedStorageNamespace::CompressedOpLog { level: self.level }, &self.key)
                .await
                .unwrap_or_else(|err| {
                    panic!("failed to drop compressed oplog for worker {worker_id} in indexed storage: {err}")
                });
        }
    }

    async fn length(&self) -> u64 {
        self.indexed_storage
            .with("compressed_oplog", "length")
            .length(
                IndexedStorageNamespace::CompressedOpLog { level: self.level },
                &self.key,
            )
            .await
            .unwrap_or_else(|err| {
                panic!("failed to get compressed oplog length from indexed storage: {err}")
            })
    }

    async fn get_last_index(&self) -> OplogIndex {
        self.current_oplog_index().await
    }
}

#[derive(Debug, Clone, Encode, Decode)]
pub struct CompressedOplogChunk {
    pub count: u64,
    pub compressed_data: Vec<u8>,
}

impl CompressedOplogChunk {
    pub fn compress(entries: Vec<OplogEntry>) -> Result<Self, String> {
        let count = entries.len() as u64;
        let uncompressed_data =
            serialize(&entries).map_err(|err| format!("failed to serialize oplog chunk: {err}"))?;
        let compressed_data = zstd::encode_all(
            &*uncompressed_data,
            CompressedOplogArchiveService::ZSTD_LEVEL,
        )
        .map_err(|err| format!("failed to compress oplog chunk: {err}"))?;
        Ok(Self {
            count,
            compressed_data,
        })
    }

    pub fn decompress(&self) -> Result<Vec<OplogEntry>, String> {
        let uncompressed_data = zstd::decode_all(&*self.compressed_data)
            .map_err(|err| format!("failed to decompress oplog chunk: {err}"))?;
        deserialize(&uncompressed_data)
            .map_err(|err| format!("failed to deserialize oplog chunk: {err}"))
    }
}
