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

use std::cmp::min;
use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;
use bincode::{Decode, Encode};
use dashmap::DashMap;
use evicting_cache_map::EvictingCacheMap;
use tokio::sync::RwLock;
use tracing::debug;

use golem_common::model::oplog::OplogEntry;
use golem_common::model::WorkerId;
use golem_common::serialization::{deserialize, serialize};

use crate::preview2::golem::api::host::OplogIndex;
use crate::services::oplog::multilayer::OplogLayer;
use crate::storage::indexed::{IndexedStorage, IndexedStorageLabelledApi, IndexedStorageNamespace};

#[derive(Debug)]
pub struct CompressedOplogLayer {
    indexed_storage: Arc<dyn IndexedStorage + Send + Sync>,
    #[allow(clippy::type_complexity)]
    cache: DashMap<
        WorkerId,
        RwLock<
            EvictingCacheMap<
                OplogIndex,
                OplogEntry,
                { CompressedOplogLayer::CACHE_SIZE },
                fn(OplogIndex, OplogEntry) -> (),
            >,
        >,
    >,
    level: u8,
}

impl CompressedOplogLayer {
    const CACHE_SIZE: usize = 4096;
    const ZSTD_LEVEL: i32 = 0;

    pub fn new(indexed_storage: Arc<dyn IndexedStorage + Send + Sync>, level: u8) -> Self {
        Self {
            indexed_storage,
            cache: DashMap::new(),
            level,
        }
    }

    fn compressed_oplog_key(worker_id: &WorkerId) -> String {
        worker_id.to_redis_key()
    }

    async fn read_and_cache_chunk(
        &self,
        worker_id: &WorkerId,
        idx: OplogIndex,
    ) -> Result<Option<OplogIndex>, String> {
        if let Some((last_idx, chunk)) = self
            .indexed_storage
            .with_entity("compressed_oplog", "create", "compressed_entry")
            .closest::<CompressedOplogChunk>(
                IndexedStorageNamespace::CompressedOpLog { level: self.level },
                &Self::compressed_oplog_key(worker_id),
                idx,
            )
            .await?
        {
            let entries = chunk.decompress()?;

            debug!(
                "read {} compressed entries for idx {idx}, adding to cache",
                entries.len()
            );

            let cache = self
                .cache
                .entry(worker_id.clone())
                .or_insert(RwLock::new(EvictingCacheMap::new()));
            let mut cache = cache.write().await;

            let mut idx = last_idx - chunk.count + 1;
            for entry in entries {
                debug!("<read_and_cache_chunk> caching compressed entry for idx {idx}");
                cache.insert(idx, entry);
                idx += 1;
            }

            Ok(Some(last_idx))
        } else {
            debug!("no compressed entries found for idx {idx}");
            Ok(None)
        }
    }
}

#[async_trait]
impl OplogLayer for CompressedOplogLayer {
    async fn read(
        &self,
        worker_id: &WorkerId,
        idx: OplogIndex,
        n: u64,
    ) -> BTreeMap<golem_common::model::oplog::OplogIndex, OplogEntry> {
        let mut result = BTreeMap::new();
        let mut last_idx = idx + n;
        let mut before = u64::MAX;

        debug!("starting read {n} compressed entries for worker {worker_id} from {idx}");

        while last_idx > idx {
            debug!("last_idx: {last_idx}");
            {
                let cache = self
                    .cache
                    .entry(worker_id.clone())
                    .or_insert(RwLock::new(EvictingCacheMap::new()));
                let mut cache = cache.write().await;

                debug!("=> start reading cache, last_idx: {last_idx}");
                while let Some(entry) = cache.get(&last_idx) {
                    result.insert(last_idx, entry.clone());
                    if last_idx == idx {
                        break;
                    } else {
                        last_idx -= 1;
                    }
                }
                debug!("=> finished reading cache, last_idx: {last_idx}");
                drop(cache);
            }

            if before == last_idx {
                debug!("before: {before} == last_idx: {last_idx}");
                panic!("No entries found in cache, even though fetch returned true. This should never happen");
            }

            let fetched_last_idx = self.read_and_cache_chunk(worker_id, last_idx).await.unwrap_or_else(|err| {
                panic!("failed to read compressed oplog for worker {worker_id} in indexed storage: {err}")
            });
            if let Some(fetched_last_idx) = fetched_last_idx {
                debug!("fetched_last_idx: {fetched_last_idx}");
                before = last_idx;
            } else if result.is_empty() {
                // We allow to have a gap on the right side of the query - as we cannot guarantee
                // that the 'n' parameter is exactly matches the available number of elements. However,
                // there must not be any gaps in the middle.
                if let Some(idx) = self.indexed_storage.with_entity("compressed_oplog", "get_first_index", "compressed_entry")
                    .last_id(IndexedStorageNamespace::CompressedOpLog { level: self.level }, &Self::compressed_oplog_key(worker_id))
                    .await
                    .unwrap_or_else(|err| {
                        panic!("failed to get first entry from compressed oplog for worker {worker_id} in indexed storage: {err}")
                    }) {
                    last_idx = min(last_idx, idx);
                } else {
                    debug!("no compressed entries found for worker {worker_id}, finishing read");
                    break;
                }
            } else {
                debug!("no more compressed entries found for worker {worker_id}, finishing read");
                // We go newer towards older entries so if we didn't fetch the chunk we reached the
                // boundary of this layer
                break;
            }
        }

        result
    }

    async fn append(&self, worker_id: &WorkerId, chunk: Vec<(OplogIndex, OplogEntry)>) {
        if !chunk.is_empty() {
            let cache = self
                .cache
                .entry(worker_id.clone())
                .or_insert(RwLock::new(EvictingCacheMap::new()));
            let mut cache = cache.write().await;
            for (idx, entry) in &chunk {
                debug!("<append> caching compressed entry for idx {idx}");
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
                    &Self::compressed_oplog_key(worker_id),
                    last_id,
                    &compressed_chunk,
                )
                .await
                .unwrap_or_else(|err| {
                    panic!("failed to append compressed oplog chunk for worker {worker_id} in indexed storage: {err}")
                });
        }
    }

    async fn drop_prefix(
        &self,
        worker_id: &WorkerId,
        last_dropped_id: golem_common::model::oplog::OplogIndex,
    ) {
        self.indexed_storage.with("compressed_oplog", "drop_prefix")
            .drop_prefix(IndexedStorageNamespace::CompressedOpLog { level: self.level }, &Self::compressed_oplog_key(worker_id), last_dropped_id)
            .await
            .unwrap_or_else(|err| {
                panic!("failed to drop prefix from compressed oplog for worker {worker_id} in indexed storage: {err}")
            });
    }

    async fn delete(&self, worker_id: &WorkerId) {
        self.indexed_storage
            .with("compressed_oplog", "delete")
            .delete(IndexedStorageNamespace::CompressedOpLog { level: self.level }, &Self::compressed_oplog_key(worker_id))
            .await
            .unwrap_or_else(|err| {
                panic!("failed to drop compressed oplog for worker {worker_id} in indexed storage: {err}")
            });
    }

    async fn length(&self, worker_id: &WorkerId) -> u64 {
        self.indexed_storage
            .with("compressed_oplog", "length")
            .length(
                IndexedStorageNamespace::CompressedOpLog { level: self.level },
                &Self::compressed_oplog_key(worker_id),
            )
            .await
            .unwrap_or_else(|err| {
                panic!("failed to get compressed oplog length from indexed storage: {err}")
            })
    }

    async fn get_first_index(
        &self,
        worker_id: &WorkerId,
    ) -> golem_common::model::oplog::OplogIndex {
        let result: Option<(u64, CompressedOplogChunk)> = self.indexed_storage
            .with_entity("compressed_oplog", "get_first_index", "compressed_entry").
            first(IndexedStorageNamespace::CompressedOpLog { level: self.level }, &Self::compressed_oplog_key(worker_id))
            .await
            .unwrap_or_else(|err| {
                panic!("failed to get first entry from compressed oplog for worker {worker_id} in indexed storage: {err}")
            });
        if let Some((chunk_last_idx, chunk)) = result {
            debug!(
                "*** first entry has id {chunk_last_idx}, containing {} entries",
                chunk.count
            );
            chunk_last_idx - chunk.count + 1
        } else {
            0
        }
    }
}

#[derive(Debug, Clone, Encode, Decode)]
struct CompressedOplogChunk {
    count: u64,
    compressed_data: Vec<u8>,
}

impl CompressedOplogChunk {
    pub fn compress(entries: Vec<OplogEntry>) -> Result<Self, String> {
        let count = entries.len() as u64;
        let uncompressed_data =
            serialize(&entries).map_err(|err| format!("failed to serialize oplog chunk: {err}"))?;
        let compressed_data =
            zstd::encode_all(&*uncompressed_data, CompressedOplogLayer::ZSTD_LEVEL)
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
