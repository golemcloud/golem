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

use crate::metrics::oplog::record_oplog_storage_retry;
use crate::services::oplog::PrimaryOplogService;
use crate::services::oplog::multilayer::{OplogArchive, OplogArchiveService};
use crate::storage::indexed::{
    IndexedStorage, IndexedStorageError, IndexedStorageLabelledApi, IndexedStorageMetaNamespace,
    IndexedStorageNamespace,
};
use anyhow::anyhow;
use async_trait::async_trait;
use desert_rust::BinaryCodec;
use evicting_cache_map::EvictingCacheMap;
use golem_common::model::RetryConfig;
use golem_common::model::component::ComponentId;
use golem_common::model::environment::EnvironmentId; // used in scan_for_component
use golem_common::model::oplog::{OplogEntry, OplogIndex};
use golem_common::model::{AgentId, OwnedAgentId, ScanCursor};
use golem_common::retries::get_delay;
use golem_common::serialization::{deserialize, serialize};
use golem_service_base::error::worker_executor::WorkerExecutorError;
use std::collections::BTreeMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::warn;

async fn retry_storage_op<T, F, Fut>(
    retry_config: &RetryConfig,
    op_name: &str,
    key: &str,
    mut op: F,
) -> T
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T, IndexedStorageError>>,
{
    let mut attempts = 0u32;
    loop {
        attempts += 1;
        match op().await {
            Ok(val) => return val,
            Err(IndexedStorageError::Transient(msg)) => {
                if let Some(delay) = get_delay(retry_config, attempts) {
                    record_oplog_storage_retry(op_name);
                    warn!(
                        op = op_name,
                        key = key,
                        attempt = attempts,
                        delay_ms = delay.as_millis() as u64,
                        "Transient indexed storage error, retrying: {msg}"
                    );
                    tokio::time::sleep(delay).await;
                } else {
                    panic!(
                        "Indexed storage operation '{op_name}' failed for key '{key}' after {attempts} attempts: Transient storage error: {msg}"
                    );
                }
            }
            Err(err) => {
                panic!("Indexed storage operation '{op_name}' failed for key '{key}': {err}");
            }
        }
    }
}

#[derive(Debug)]
pub struct CompressedOplogArchiveService {
    indexed_storage: Arc<dyn IndexedStorage + Send + Sync>,
    level: usize,
    retry_config: RetryConfig,
}

impl CompressedOplogArchiveService {
    const MAX_CHUNK_SIZE: usize = 4096;
    const CACHE_SIZE: usize = 4096;
    const ZSTD_LEVEL: i32 = 0;

    pub fn new(
        indexed_storage: Arc<dyn IndexedStorage + Send + Sync>,
        level: usize,
        retry_config: RetryConfig,
    ) -> Self {
        Self {
            indexed_storage,
            level,
            retry_config,
        }
    }

    fn compressed_oplog_key(agent_id: &AgentId) -> String {
        agent_id.to_redis_key()
    }
}

#[async_trait]
impl OplogArchiveService for CompressedOplogArchiveService {
    async fn open(&self, owned_agent_id: &OwnedAgentId) -> Arc<dyn OplogArchive + Send + Sync> {
        Arc::new(CompressedOplogArchive::new(
            owned_agent_id.agent_id(),
            self.indexed_storage.clone(),
            self.level,
            self.retry_config.clone(),
        ))
    }

    async fn delete(&self, owned_agent_id: &OwnedAgentId) {
        let is = self.indexed_storage.clone();
        let agent_id = owned_agent_id.agent_id();
        let level = self.level;
        let key = Self::compressed_oplog_key(&owned_agent_id.agent_id);
        retry_storage_op(&self.retry_config, "compressed_delete", &key, || {
            let is = is.clone();
            let ns = IndexedStorageNamespace::CompressedOpLog {
                agent_id: agent_id.clone(),
                level,
            };
            let key = key.clone();
            async move { is.with("compressed_oplog", "delete").delete(ns, &key).await }
        })
        .await;
    }

    async fn read(
        &self,
        owned_agent_id: &OwnedAgentId,
        idx: OplogIndex,
        n: u64,
    ) -> BTreeMap<OplogIndex, OplogEntry> {
        let archive = self.open(owned_agent_id).await;
        archive.read(idx, n).await
    }

    async fn exists(&self, owned_agent_id: &OwnedAgentId) -> bool {
        let is = self.indexed_storage.clone();
        let agent_id = owned_agent_id.agent_id();
        let level = self.level;
        let key = Self::compressed_oplog_key(&owned_agent_id.agent_id);
        retry_storage_op(&self.retry_config, "compressed_exists", &key, || {
            let is = is.clone();
            let ns = IndexedStorageNamespace::CompressedOpLog {
                agent_id: agent_id.clone(),
                level,
            };
            let key = key.clone();
            async move { is.with("compressed_oplog", "exists").exists(ns, &key).await }
        })
        .await
    }

    async fn scan_for_component(
        &self,
        environment_id: &EnvironmentId,
        component_id: &ComponentId,
        cursor: ScanCursor,
        count: u64,
    ) -> Result<(ScanCursor, Vec<OwnedAgentId>), WorkerExecutorError> {
        let ScanCursor { cursor, layer } = cursor;
        let (cursor, keys) = {
            let is = self.indexed_storage.clone();
            let level = self.level;
            let prefix = PrimaryOplogService::key_prefix(component_id);
            retry_storage_op(&self.retry_config, "compressed_scan", &prefix, || {
                let is = is.clone();
                let prefix = prefix.clone();
                async move {
                    is.with("compressed_oplog", "scan")
                        .scan(
                            IndexedStorageMetaNamespace::CompressedOplog { level },
                            Some(&prefix),
                            cursor,
                            count,
                        )
                        .await
                }
            })
            .await
        };

        Ok((
            ScanCursor { cursor, layer },
            keys.into_iter()
                .map(|key| OwnedAgentId {
                    agent_id: PrimaryOplogService::get_agent_id_from_key(&key, component_id),
                    environment_id: *environment_id,
                })
                .collect(),
        ))
    }

    async fn get_last_index(&self, owned_agent_id: &OwnedAgentId) -> OplogIndex {
        let key = Self::compressed_oplog_key(&owned_agent_id.agent_id);
        let is = self.indexed_storage.clone();
        let agent_id = owned_agent_id.agent_id();
        let level = self.level;
        OplogIndex::from_u64(
            retry_storage_op(
                &self.retry_config,
                "compressed_get_last_index",
                &key,
                || {
                    let is = is.clone();
                    let ns = IndexedStorageNamespace::CompressedOpLog {
                        agent_id: agent_id.clone(),
                        level,
                    };
                    let key = key.clone();
                    async move {
                        is.with_entity(
                            "compressed_oplog",
                            "current_oplog_index",
                            "compressed_entry",
                        )
                        .last_id(ns, &key)
                        .await
                    }
                },
            )
            .await
            .unwrap_or_default(),
        )
    }
}

#[derive(Debug)]
pub struct CompressedOplogArchive {
    agent_id: AgentId,
    key: String,
    indexed_storage: Arc<dyn IndexedStorage + Send + Sync>,
    retry_config: RetryConfig,
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
        agent_id: AgentId,
        indexed_storage: Arc<dyn IndexedStorage + Send + Sync>,
        level: usize,
        retry_config: RetryConfig,
    ) -> Self {
        let key = CompressedOplogArchiveService::compressed_oplog_key(&agent_id);
        Self {
            agent_id,
            key,
            indexed_storage,
            retry_config,
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
    ) -> anyhow::Result<Option<Vec<(OplogIndex, OplogEntry)>>> {
        let (last_idx_in_chunk, chunk) = if let Some((last_idx_in_chunk, chunk)) = self
            .indexed_storage
            .with_entity("compressed_oplog", "read", "compressed_entry")
            .closest::<CompressedOplogChunk>(
                IndexedStorageNamespace::CompressedOpLog {
                    agent_id: self.agent_id.clone(),
                    level: self.level,
                },
                &self.key,
                end_of_range.into(),
            )
            .await
            .map_err(|e| anyhow!(e))?
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
        let agent_id = &self.agent_id;

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
                panic!("failed to read compressed oplog for worker {agent_id} in indexed storage: {err}")
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

    async fn append(&self, chunk: Vec<(OplogIndex, OplogEntry)>) -> u64 {
        if chunk.is_empty() {
            return 0;
        }

        let mut cache = self.cache.write().await;
        let mut total_bytes = 0u64;

        for (idx, entry) in &chunk {
            cache.insert(*idx, entry.clone());
        }

        for sub_chunk in chunk.chunks(CompressedOplogArchiveService::MAX_CHUNK_SIZE) {
            let last_id = sub_chunk.last().unwrap().0;

            let entries: Vec<OplogEntry> =
                sub_chunk.iter().map(|(_, entry)| entry.clone()).collect();

            let compressed_chunk = CompressedOplogChunk::compress(entries)
                .unwrap_or_else(|err| panic!("failed to compress oplog chunk: {err}"));

            total_bytes += compressed_chunk.compressed_data.len() as u64;

            {
                let is = self.indexed_storage.clone();
                let agent_id_clone = self.agent_id.clone();
                let level = self.level;
                let key = self.key.clone();
                let last_id_val: u64 = last_id.into();
                retry_storage_op(&self.retry_config, "compressed_append", &key, || {
                    let is = is.clone();
                    let ns = IndexedStorageNamespace::CompressedOpLog {
                        agent_id: agent_id_clone.clone(),
                        level,
                    };
                    let key = key.clone();
                    let chunk = compressed_chunk.clone();
                    async move {
                        is.with_entity("compressed_oplog", "append", "compressed_entry")
                            .append(ns, &key, last_id_val, &chunk)
                            .await
                    }
                })
                .await;
            }
        }

        total_bytes
    }

    async fn current_oplog_index(&self) -> OplogIndex {
        let is = self.indexed_storage.clone();
        let agent_id = self.agent_id.clone();
        let level = self.level;
        let key = self.key.clone();
        OplogIndex::from_u64(
            retry_storage_op(
                &self.retry_config,
                "compressed_current_oplog_index",
                &key,
                || {
                    let is = is.clone();
                    let ns = IndexedStorageNamespace::CompressedOpLog {
                        agent_id: agent_id.clone(),
                        level,
                    };
                    let key = key.clone();
                    async move {
                        is.with_entity(
                            "compressed_oplog",
                            "current_oplog_index",
                            "compressed_entry",
                        )
                        .last_id(ns, &key)
                        .await
                    }
                },
            )
            .await
            .unwrap_or_default(),
        )
    }

    async fn drop_prefix(&self, last_dropped_id: OplogIndex) -> u64 {
        let before = self.length().await;
        {
            let is = self.indexed_storage.clone();
            let agent_id = self.agent_id.clone();
            let level = self.level;
            let key = self.key.clone();
            let dropped_id: u64 = last_dropped_id.into();
            retry_storage_op(&self.retry_config, "compressed_drop_prefix", &key, || {
                let is = is.clone();
                let ns = IndexedStorageNamespace::CompressedOpLog {
                    agent_id: agent_id.clone(),
                    level,
                };
                let key = key.clone();
                async move {
                    is.with("compressed_oplog", "drop_prefix")
                        .drop_prefix(ns, &key, dropped_id)
                        .await
                }
            })
            .await;
        }
        let remaining = self.length().await;
        if remaining == 0 {
            let is = self.indexed_storage.clone();
            let agent_id = self.agent_id.clone();
            let level = self.level;
            let key = self.key.clone();
            retry_storage_op(&self.retry_config, "compressed_delete", &key, || {
                let is = is.clone();
                let ns = IndexedStorageNamespace::CompressedOpLog {
                    agent_id: agent_id.clone(),
                    level,
                };
                let key = key.clone();
                async move {
                    is.with("compressed_oplog", "drop_prefix")
                        .delete(ns, &key)
                        .await
                }
            })
            .await;
        }
        before - remaining
    }

    async fn length(&self) -> u64 {
        let is = self.indexed_storage.clone();
        let agent_id = self.agent_id.clone();
        let level = self.level;
        let key = self.key.clone();
        retry_storage_op(&self.retry_config, "compressed_length", &key, || {
            let is = is.clone();
            let ns = IndexedStorageNamespace::CompressedOpLog {
                agent_id: agent_id.clone(),
                level,
            };
            let key = key.clone();
            async move { is.with("compressed_oplog", "length").length(ns, &key).await }
        })
        .await
    }

    async fn get_last_index(&self) -> OplogIndex {
        self.current_oplog_index().await
    }
}

#[derive(Debug, Clone, BinaryCodec)]
#[desert(evolution())]
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

    pub fn decompress(&self) -> anyhow::Result<Vec<OplogEntry>> {
        let uncompressed_data = zstd::decode_all(&*self.compressed_data)
            .map_err(|err| anyhow!("failed to decompress oplog chunk: {err}"))?;
        deserialize(&uncompressed_data)
            .map_err(|err| anyhow!("failed to deserialize oplog chunk: {err}"))
    }
}
