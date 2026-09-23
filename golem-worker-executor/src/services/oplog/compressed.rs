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
use crate::services::oplog::multilayer::{OplogArchive, OplogArchiveService};
use crate::services::oplog::reader::{
    OplogReadError, OplogReadSource, fail_stop, verify_persisted_entries,
};
use crate::services::oplog::{
    PrimaryOplogService, decode_scan_cursor, next_scan_cursor, retry_scan_storage_op,
};
use crate::storage::indexed::{
    IndexedStorage, IndexedStorageError, IndexedStorageLabelledApi, IndexedStorageMetaNamespace,
    IndexedStorageNamespace,
};
use anyhow::anyhow;
use async_trait::async_trait;
use desert_rust::BinaryCodec;
use evicting_cache_map::EvictingCacheMap;
use golem_common::model::RetryConfig;
use golem_common::model::agent::AgentMode;
use golem_common::model::component::ComponentId;
use golem_common::model::environment::EnvironmentId; // used in scan_for_component
use golem_common::model::oplog::{OplogEntry, OplogIndex};
use golem_common::model::{AgentId, OwnedAgentId, ScanCursor};
use golem_common::retries::get_delay;
use golem_common::serialization::{deserialize, serialize};
use golem_service_base::error::worker_executor::WorkerExecutorError;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
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

/// Appends one already-serialized compressed chunk, retrying transient failures like
/// [`retry_storage_op`]. A permanent failure is reconciled against storage before it is treated
/// as fatal, because this append is not protected by a shard epoch (it is driven by whichever
/// executor's transfer fiber is running, primary-owner or not) and its background task can be
/// aborted between steps - including after this append lands but before the `drop_source_prefix`
/// that would have advanced the source past it. The owner's next transfer then chunks from the
/// same unadvanced point, so a chunk it writes under an id that already exists holds the identical
/// entries and bytes. A duplicate-id failure whose stored content matches what this attempt would
/// have written is that resumed transfer catching up, not corruption, and is treated as success
/// rather than panicking on the storage's key conflict.
async fn append_compressed_chunk(
    retry_config: &RetryConfig,
    indexed_storage: &(dyn IndexedStorage + Send + Sync),
    namespace: &IndexedStorageNamespace,
    key: &str,
    id: u64,
    value: Vec<u8>,
) {
    let mut attempts = 0u32;
    loop {
        attempts += 1;
        let error = match indexed_storage
            .with_entity("compressed_oplog", "append", "compressed_entry")
            .append_raw(namespace.clone(), key, id, value.clone(), None)
            .await
        {
            Ok(()) => return,
            Err(error) => error,
        };

        if let IndexedStorageError::Transient(msg) = &error {
            if let Some(delay) = get_delay(retry_config, attempts) {
                record_oplog_storage_retry("compressed_append");
                warn!(
                    op = "compressed_append",
                    key = key,
                    attempt = attempts,
                    delay_ms = delay.as_millis() as u64,
                    "Transient indexed storage error, retrying: {msg}"
                );
                tokio::time::sleep(delay).await;
                continue;
            }
            panic!(
                "Indexed storage operation 'compressed_append' failed for key '{key}' after {attempts} attempts: Transient storage error: {msg}"
            );
        }

        if stored_chunk_matches(retry_config, indexed_storage, namespace, key, id, &value).await {
            return;
        }
        panic!("Indexed storage operation 'compressed_append' failed for key '{key}': {error}");
    }
}

/// Reads back the chunk stored at `id` and compares it byte-for-byte with `expected`. Used only to
/// tell a resumed transfer's harmless repeat write apart from a genuine conflict - see
/// [`append_compressed_chunk`].
async fn stored_chunk_matches(
    retry_config: &RetryConfig,
    indexed_storage: &(dyn IndexedStorage + Send + Sync),
    namespace: &IndexedStorageNamespace,
    key: &str,
    id: u64,
    expected: &[u8],
) -> bool {
    let actual = retry_storage_op(retry_config, "compressed_append_reconcile", key, || {
        let namespace = namespace.clone();
        async move {
            indexed_storage
                .with_entity(
                    "compressed_oplog",
                    "compressed_append_reconcile",
                    "compressed_entry",
                )
                .read_raw(namespace, key, id, id)
                .await
        }
    })
    .await;
    actual
        .into_iter()
        .find(|(actual_id, _)| *actual_id == id)
        .is_some_and(|(_, bytes)| bytes == expected)
}

#[derive(Debug)]
pub struct CompressedOplogArchiveService {
    indexed_storage: Arc<dyn IndexedStorage + Send + Sync>,
    level: usize,
    retry_config: RetryConfig,
}

impl CompressedOplogArchiveService {
    const MAX_CHUNK_SIZE: usize = 4096;
    const CACHE_SIZE: usize = 2048;
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
    async fn open(
        &self,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
    ) -> Arc<dyn OplogArchive + Send + Sync> {
        Arc::new(CompressedOplogArchive::new(
            owned_agent_id.agent_id(),
            agent_mode,
            self.indexed_storage.clone(),
            self.level,
            self.retry_config.clone(),
        ))
    }

    async fn open_fresh(
        &self,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
    ) -> Arc<dyn OplogArchive + Send + Sync> {
        Arc::new(CompressedOplogArchive::new(
            owned_agent_id.agent_id(),
            agent_mode,
            self.indexed_storage.clone(),
            self.level,
            self.retry_config.clone(),
        ))
    }

    async fn delete(&self, owned_agent_id: &OwnedAgentId, agent_mode: AgentMode) {
        let is = self.indexed_storage.clone();
        let agent_id = owned_agent_id.agent_id();
        let level = self.level;
        let key = Self::compressed_oplog_key(&owned_agent_id.agent_id);
        retry_storage_op(&self.retry_config, "compressed_delete", &key, || {
            let is = is.clone();
            let ns = IndexedStorageNamespace::CompressedOpLog {
                agent_id: agent_id.clone(),
                agent_mode,
                level,
            };
            let key = key.clone();
            async move { is.with("compressed_oplog", "delete").delete(ns, &key).await }
        })
        .await;
    }

    async fn read_source(
        &self,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
        idx: OplogIndex,
        n: u64,
    ) -> BTreeMap<OplogIndex, OplogEntry> {
        let archive = self.open(owned_agent_id, agent_mode).await;
        archive.read_source(idx, n).await
    }

    async fn exists(&self, owned_agent_id: &OwnedAgentId, agent_mode: AgentMode) -> bool {
        let is = self.indexed_storage.clone();
        let agent_id = owned_agent_id.agent_id();
        let level = self.level;
        let key = Self::compressed_oplog_key(&owned_agent_id.agent_id);
        retry_storage_op(&self.retry_config, "compressed_exists", &key, || {
            let is = is.clone();
            let ns = IndexedStorageNamespace::CompressedOpLog {
                agent_id: agent_id.clone(),
                agent_mode,
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
        modes: Option<AgentMode>,
        cursor: ScanCursor,
        count: u64,
    ) -> Result<(ScanCursor, Vec<OwnedAgentId>), WorkerExecutorError> {
        let state = decode_scan_cursor(&cursor, modes)?;
        let active_mode = state.mode;

        let (next_resume, keys) = {
            let is = self.indexed_storage.clone();
            let level = self.level;
            let prefix = PrimaryOplogService::key_prefix(component_id);
            let resume = state.resume.clone();
            retry_scan_storage_op(&self.retry_config, "compressed_scan", &prefix, || {
                let is = is.clone();
                let prefix = prefix.clone();
                let resume = resume.clone();
                async move {
                    is.with("compressed_oplog", "scan")
                        .scan_stable(
                            IndexedStorageMetaNamespace::CompressedOplog {
                                agent_mode: active_mode,
                                level,
                            },
                            Some(&prefix),
                            resume,
                            count,
                        )
                        .await
                }
            })
            .await?
        };

        let next_cursor = next_scan_cursor(state, modes, next_resume)?;
        Ok((
            next_cursor,
            keys.into_iter()
                .map(|key| OwnedAgentId {
                    agent_id: PrimaryOplogService::get_agent_id_from_key(&key, component_id),
                    environment_id: *environment_id,
                })
                .collect(),
        ))
    }

    async fn get_last_index(
        &self,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
    ) -> OplogIndex {
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
                        agent_mode,
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

    fn scan_namespace(&self, agent_mode: AgentMode) -> Option<IndexedStorageMetaNamespace> {
        Some(IndexedStorageMetaNamespace::CompressedOplog {
            agent_mode,
            level: self.level,
        })
    }
}

#[derive(Debug)]
pub struct CompressedOplogArchive {
    agent_id: AgentId,
    agent_mode: AgentMode,
    key: String,
    indexed_storage: Arc<dyn IndexedStorage + Send + Sync>,
    retry_config: RetryConfig,
    /// A `std` mutex rather than an async lock: `read_source` and `append` are awaited both by
    /// wasmtime store-polled futures (durable host calls) and by independent tokio tasks. Tokio's
    /// fair locks hand ownership to a queued waiter at wake time, before it is polled, so a
    /// store-polled future queued on an async lock could become its owner while the store is
    /// unable to poll it (wasmtime#11869/#11870), wedging every other user of the cache. Every
    /// critical section below is synchronous and never spans an `await`.
    #[allow(clippy::type_complexity)]
    cache: Mutex<
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
        agent_mode: AgentMode,
        indexed_storage: Arc<dyn IndexedStorage + Send + Sync>,
        level: usize,
        retry_config: RetryConfig,
    ) -> Self {
        let key = CompressedOplogArchiveService::compressed_oplog_key(&agent_id);
        Self {
            agent_id,
            agent_mode,
            key,
            indexed_storage,
            retry_config,
            cache: Mutex::new(EvictingCacheMap::new()),
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
    ) -> Result<Option<Vec<(OplogIndex, OplogEntry)>>, OplogReadError> {
        let source = OplogReadSource::Archive(self.level);
        let (last_idx_in_chunk, chunk) = if let Some((last_idx_in_chunk, chunk)) = self
            .indexed_storage
            .with_entity("compressed_oplog", "read", "compressed_entry")
            .closest::<CompressedOplogChunk>(
                IndexedStorageNamespace::CompressedOpLog {
                    agent_id: self.agent_id.clone(),
                    agent_mode: self.agent_mode,
                    level: self.level,
                },
                &self.key,
                end_of_range.into(),
            )
            .await
            .map_err(|error| {
                OplogReadError::source_failure(
                    source,
                    format!(
                        "failed to read compressed oplog for worker {} in indexed storage: {error}",
                        self.agent_id
                    ),
                )
            })? {
            (last_idx_in_chunk, chunk)
        } else {
            return Ok(None);
        };

        let entries = chunk.decompress().map_err(|error| {
            OplogReadError::corruption(
                source,
                format!(
                    "failed to decode compressed oplog chunk ending at {last_idx_in_chunk}: {error}"
                ),
            )
        })?;
        if chunk.count == 0 || entries.len() as u64 != chunk.count {
            return Err(OplogReadError::corruption(
                source,
                format!(
                    "compressed oplog chunk ending at {last_idx_in_chunk} declares {} entries but contains {}",
                    chunk.count,
                    entries.len()
                ),
            ));
        }
        let first_idx_in_chunk = last_idx_in_chunk.checked_sub(chunk.count - 1).ok_or_else(
            || {
                OplogReadError::corruption(
                    source,
                    format!(
                        "compressed oplog chunk ending at {last_idx_in_chunk} has invalid count {}",
                        chunk.count
                    ),
                )
            },
        )?;
        let mut cache = self.cache.lock().unwrap();

        let mut collected = Vec::new();

        for (current_idx, entry) in (first_idx_in_chunk..).zip(entries) {
            let oplog_index = OplogIndex::from_u64(current_idx);

            cache.insert(oplog_index, entry.clone());

            if oplog_index >= beginning_of_range && oplog_index <= end_of_range {
                collected.push((oplog_index, entry));
            }
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
    async fn read_source(
        &self,
        idx: OplogIndex,
        n: u64,
    ) -> BTreeMap<golem_common::model::oplog::OplogIndex, OplogEntry> {
        if n == 0 {
            return BTreeMap::new();
        }

        let mut result = BTreeMap::new();
        let mut last_idx = idx.range_end(n);

        while last_idx >= idx {
            {
                let mut cache = self.cache.lock().unwrap();

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
            if let Some(chunk) = fail_stop(self.fetch_and_cache_range(idx, last_idx).await) {
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

    async fn append(&self, chunk: &[(OplogIndex, OplogEntry)]) -> u64 {
        if chunk.is_empty() {
            return 0;
        }

        // The cache lock must not be held across the storage writes below: `append` can be
        // reached from host-call contexts (through ephemeral oplogs), and an async lock held
        // across IO by a store-polled future can deadlock the store (wasmtime#11869/#11870).
        {
            let mut cache = self.cache.lock().unwrap();
            for (idx, entry) in chunk {
                cache.insert(*idx, entry.clone());
            }
        }

        let mut total_bytes = 0u64;

        for sub_chunk in chunk.chunks(CompressedOplogArchiveService::MAX_CHUNK_SIZE) {
            let last_id = sub_chunk.last().unwrap().0;

            let entries: Vec<OplogEntry> =
                sub_chunk.iter().map(|(_, entry)| entry.clone()).collect();

            let compressed_chunk = CompressedOplogChunk::compress(entries)
                .unwrap_or_else(|err| panic!("failed to compress oplog chunk: {err}"));

            total_bytes += compressed_chunk.compressed_data.len() as u64;

            {
                let ns = IndexedStorageNamespace::CompressedOpLog {
                    agent_id: self.agent_id.clone(),
                    agent_mode: self.agent_mode,
                    level: self.level,
                };
                let last_id_val: u64 = last_id.into();
                let value = serialize(&compressed_chunk)
                    .unwrap_or_else(|err| panic!("failed to serialize oplog chunk: {err}"));
                append_compressed_chunk(
                    &self.retry_config,
                    self.indexed_storage.as_ref(),
                    &ns,
                    &self.key,
                    last_id_val,
                    value,
                )
                .await;
            }
        }

        total_bytes
    }

    async fn verify_persisted(&self, entries: &[(OplogIndex, OplogEntry)]) {
        let Some((start, _)) = entries.first() else {
            return;
        };
        let uncached = Self::new(
            self.agent_id.clone(),
            self.agent_mode,
            self.indexed_storage.clone(),
            self.level,
            self.retry_config.clone(),
        );
        let actual = uncached.read_source(*start, entries.len() as u64).await;
        fail_stop(verify_persisted_entries(
            OplogReadSource::Archive(self.level),
            entries,
            actual,
        ));
    }

    async fn current_oplog_index(&self) -> OplogIndex {
        let is = self.indexed_storage.clone();
        let agent_id = self.agent_id.clone();
        let agent_mode = self.agent_mode;
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
                        agent_mode,
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
            let agent_mode = self.agent_mode;
            let level = self.level;
            let key = self.key.clone();
            let dropped_id: u64 = last_dropped_id.into();
            retry_storage_op(&self.retry_config, "compressed_drop_prefix", &key, || {
                let is = is.clone();
                let ns = IndexedStorageNamespace::CompressedOpLog {
                    agent_id: agent_id.clone(),
                    agent_mode,
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
            let agent_mode = self.agent_mode;
            let level = self.level;
            let key = self.key.clone();
            retry_storage_op(&self.retry_config, "compressed_delete", &key, || {
                let is = is.clone();
                let ns = IndexedStorageNamespace::CompressedOpLog {
                    agent_id: agent_id.clone(),
                    agent_mode,
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
        let agent_mode = self.agent_mode;
        let level = self.level;
        let key = self.key.clone();
        retry_storage_op(&self.retry_config, "compressed_length", &key, || {
            let is = is.clone();
            let ns = IndexedStorageNamespace::CompressedOpLog {
                agent_id: agent_id.clone(),
                agent_mode,
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
