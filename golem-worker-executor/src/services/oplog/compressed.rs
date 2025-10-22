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

use crate::services::oplog::multilayer::{OplogArchive, OplogArchiveService};
use crate::services::oplog::PrimaryOplogService;
use crate::storage::indexed::{IndexedStorage, IndexedStorageLabelledApi, IndexedStorageNamespace};
use async_trait::async_trait;
use bincode::{Decode, Encode};
use evicting_cache_map::EvictingCacheMap;
use golem_common::model::oplog::{OplogEntry, OplogIndex};
use golem_common::model::{ComponentId, OwnedWorkerId, ProjectId, ScanCursor, WorkerId};
use golem_common::serialization::{deserialize, serialize};
use golem_service_base::error::worker_executor::WorkerExecutorError;
use std::collections::BTreeMap;
use std::sync::Arc;
use tokio::sync::RwLock;

#[derive(Debug)]
pub struct CompressedOplogArchiveService {
    indexed_storage: Arc<dyn IndexedStorage + Send + Sync>,
    level: usize,
}

impl CompressedOplogArchiveService {
    const MAX_CHUNK_SIZE: usize = 4096;
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
        project_id: &ProjectId,
        component_id: &ComponentId,
        cursor: ScanCursor,
        count: u64,
    ) -> Result<(ScanCursor, Vec<OwnedWorkerId>), WorkerExecutorError> {
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
                    project_id: project_id.clone(),
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

    // Fetch a range of entries from the storage. At most one chunk of data will be returned,
    // but it will always begin with the end of the range. So a given prefix of the of the oplog might be missing,
    // but the suffix will always be correct if it is returned. Returns None if there is no chunk containing any matching data.
    async fn fetch_and_cache_range(
        &self,
        beginning_of_range: OplogIndex,
        end_of_range: OplogIndex,
    ) -> Result<Option<Vec<(OplogIndex, OplogEntry)>>, String> {
        let (last_idx_in_chunk, chunk) = if let Some((last_idx_in_chunk, chunk)) = self
            .indexed_storage
            .with_entity("compressed_oplog", "read", "compressed_entry")
            .closest::<CompressedOplogChunk>(
                IndexedStorageNamespace::CompressedOpLog { level: self.level },
                &self.key,
                end_of_range.into(),
            )
            .await?
        {
            (last_idx_in_chunk, chunk)
        } else {
            return Ok(None);
        };

        let entries = chunk.decompress()?;
        let mut cache = self.cache.write().await;

        let mut current_idx = last_idx_in_chunk - chunk.count + 1;
        let mut collected = Vec::new();

        for entry in entries {
            let oplog_index = OplogIndex::from_u64(current_idx);

            cache.insert(oplog_index, entry.clone());

            if oplog_index >= beginning_of_range && oplog_index <= end_of_range {
                collected.push((oplog_index, entry));
            }

            current_idx += 1;
        }

        if collected.is_empty() {
            // The closest chunk did not include any of the data were looking for
            return Ok(None);
        }

        Ok(Some(collected))
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

            if result.len() as u64 == n {
                // We are done fetching all the results
                break;
            }

            // we encountered an entry that is not in our cache. fetch the chunk that contains the entry and use as much as we can from it.
            // after the end of the chunk
            if let Some(chunk) = self.fetch_and_cache_range(idx, last_idx).await.unwrap_or_else(|err| {
                panic!("failed to read compressed oplog for worker {worker_id} in indexed storage: {err}")
            }) {
                last_idx = last_idx.subtract(chunk.len() as u64);
                for (index, entry) in chunk {
                    result.insert(index, entry);
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
        if chunk.is_empty() {
            return;
        }

        let worker_id = &self.worker_id;
        let mut cache = self.cache.write().await;

        for (idx, entry) in &chunk {
            cache.insert(*idx, entry.clone());
        }

        for sub_chunk in chunk.chunks(CompressedOplogArchiveService::MAX_CHUNK_SIZE) {
            let last_id = sub_chunk.last().unwrap().0;

            let entries: Vec<OplogEntry> =
                sub_chunk.iter().map(|(_, entry)| entry.clone()).collect();

            let compressed_chunk = CompressedOplogChunk::compress(entries)
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
                    panic!(
                        "failed to append compressed oplog chunk for worker {worker_id} in indexed storage: {err}"
                    )
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

    async fn drop_prefix(&self, last_dropped_id: OplogIndex) -> u64 {
        let worker_id = &self.worker_id;
        let before = self.length().await;
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
        before - remaining
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
