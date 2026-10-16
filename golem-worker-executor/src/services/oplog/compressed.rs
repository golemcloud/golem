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

use crate::services::oplog::fenced_stream::{
    FencedIndexedStream, StreamLabels, scan_component_streams,
};
use crate::services::oplog::multilayer::{OplogArchive, OplogArchiveResult, OplogArchiveService};
use crate::services::oplog::reader::{OplogReadError, OplogReadSource, verify_persisted_entries};
use crate::services::oplog::{OplogError, OplogFence};
use crate::storage::indexed::{
    IndexedStorage, IndexedStorageMetaNamespace, IndexedStorageNamespace,
};
use anyhow::anyhow;
use async_trait::async_trait;
use desert_rust::BinaryCodec;
use evicting_cache_map::EvictingCacheMap;
use golem_common::model::agent::AgentMode;
use golem_common::model::component::ComponentId;
use golem_common::model::environment::EnvironmentId; // used in scan_for_component
use golem_common::model::oplog::{OplogEntry, OplogIndex};
use golem_common::model::{AgentId, OwnedAgentId, RetryConfig, ScanCursor, ShardEpoch};
use golem_common::serialization::{deserialize, serialize};
use golem_service_base::error::worker_executor::WorkerExecutorError;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

const LABELS: StreamLabels = StreamLabels {
    svc: "compressed_oplog",
    entity: "compressed_entry",
    op: "compressed",
    what: "compressed oplog",
};

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

    /// The level's stream for an agent, asserting no epoch.
    fn stream(&self, owned_agent_id: &OwnedAgentId, agent_mode: AgentMode) -> FencedIndexedStream {
        let agent_id = owned_agent_id.agent_id();
        FencedIndexedStream::new(
            agent_id.clone(),
            CompressedOplogArchive::namespace(agent_id, agent_mode, self.level),
            LABELS,
            self.indexed_storage.clone(),
            self.retry_config.clone(),
        )
    }
}

#[async_trait]
impl OplogArchiveService for CompressedOplogArchiveService {
    async fn open(
        &self,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
        shard_epoch: Option<ShardEpoch>,
    ) -> Arc<dyn OplogArchive + Send + Sync> {
        Arc::new(
            CompressedOplogArchive::opened(
                owned_agent_id.agent_id(),
                agent_mode,
                self.indexed_storage.clone(),
                self.level,
                self.retry_config.clone(),
                shard_epoch,
            )
            .await,
        )
    }

    async fn open_fresh(
        &self,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
        shard_epoch: Option<ShardEpoch>,
    ) -> Arc<dyn OplogArchive + Send + Sync> {
        self.open(owned_agent_id, agent_mode, shard_epoch).await
    }

    async fn delete(
        &self,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
    ) -> OplogArchiveResult<()> {
        self.stream(owned_agent_id, agent_mode).delete().await
    }

    async fn read_source(
        &self,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
        idx: OplogIndex,
        n: u64,
    ) -> BTreeMap<OplogIndex, OplogEntry> {
        let archive = self.open(owned_agent_id, agent_mode, None).await;
        archive.read_source(idx, n).await.unwrap_or_else(|error| {
            panic!("Oplog archive read failed for {owned_agent_id}: {error}")
        })
    }

    async fn exists(&self, owned_agent_id: &OwnedAgentId, agent_mode: AgentMode) -> bool {
        self.try_exists(owned_agent_id, agent_mode)
            .await
            .unwrap_or_else(|error| {
                panic!("Failed to check compressed oplog archive existence: {error}")
            })
    }

    async fn try_exists(
        &self,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
    ) -> OplogArchiveResult<bool> {
        self.stream(owned_agent_id, agent_mode).exists().await
    }

    async fn scan_for_component(
        &self,
        environment_id: &EnvironmentId,
        component_id: &ComponentId,
        modes: Option<AgentMode>,
        cursor: ScanCursor,
        count: u64,
    ) -> Result<(ScanCursor, Vec<OwnedAgentId>), WorkerExecutorError> {
        let level = self.level;
        scan_component_streams(
            &self.indexed_storage,
            &self.retry_config,
            LABELS,
            environment_id,
            component_id,
            modes,
            cursor,
            count,
            |agent_mode| IndexedStorageMetaNamespace::CompressedOplog { agent_mode, level },
        )
        .await
    }

    async fn get_last_index(
        &self,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
    ) -> OplogIndex {
        self.try_get_last_index(owned_agent_id, agent_mode)
            .await
            .unwrap_or_else(|error| {
                panic!("Failed to read compressed oplog archive index: {error}")
            })
    }

    async fn try_get_last_index(
        &self,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
    ) -> OplogArchiveResult<OplogIndex> {
        self.stream(owned_agent_id, agent_mode).last_id().await
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
    /// The level's chunks, written as the owner of the shard epoch the handle was opened with.
    stream: FencedIndexedStream,
}

impl CompressedOplogArchive {
    /// A handle that asserts no epoch, for reads and for executors without a shard assignment.
    pub fn new(
        agent_id: AgentId,
        agent_mode: AgentMode,
        indexed_storage: Arc<dyn IndexedStorage + Send + Sync>,
        level: usize,
        retry_config: RetryConfig,
    ) -> Self {
        let stream = FencedIndexedStream::new(
            agent_id.clone(),
            Self::namespace(agent_id.clone(), agent_mode, level),
            LABELS,
            indexed_storage.clone(),
            retry_config.clone(),
        );
        Self::with_stream(
            agent_id,
            agent_mode,
            indexed_storage,
            level,
            retry_config,
            stream,
        )
    }

    /// A handle that writes as the owner of `shard_epoch`: records it on this level's key (a
    /// monotonic compare-and-set, as the primary oplog does at open) and asserts it on every
    /// write. A newer owner's record refuses the open, and the handle is returned already fenced.
    pub async fn opened(
        agent_id: AgentId,
        agent_mode: AgentMode,
        indexed_storage: Arc<dyn IndexedStorage + Send + Sync>,
        level: usize,
        retry_config: RetryConfig,
        shard_epoch: Option<ShardEpoch>,
    ) -> Self {
        let stream = FencedIndexedStream::opened(
            agent_id.clone(),
            Self::namespace(agent_id.clone(), agent_mode, level),
            LABELS,
            indexed_storage.clone(),
            retry_config.clone(),
            shard_epoch,
        )
        .await;
        Self::with_stream(
            agent_id,
            agent_mode,
            indexed_storage,
            level,
            retry_config,
            stream,
        )
    }

    fn with_stream(
        agent_id: AgentId,
        agent_mode: AgentMode,
        indexed_storage: Arc<dyn IndexedStorage + Send + Sync>,
        level: usize,
        retry_config: RetryConfig,
        stream: FencedIndexedStream,
    ) -> Self {
        Self {
            agent_id,
            agent_mode,
            indexed_storage,
            retry_config,
            cache: Mutex::new(EvictingCacheMap::new()),
            level,
            stream,
        }
    }

    fn namespace(
        agent_id: AgentId,
        agent_mode: AgentMode,
        level: usize,
    ) -> IndexedStorageNamespace {
        IndexedStorageNamespace::CompressedOpLog {
            agent_id,
            agent_mode,
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
        let Some((last_idx_in_chunk, chunk)) = self
            .stream
            .closest::<CompressedOplogChunk>(end_of_range)
            .await
            .map_err(|error| {
                OplogReadError::source_failure(
                    source,
                    format!(
                        "failed to read compressed oplog for worker {} in indexed storage: {error}",
                        self.agent_id
                    ),
                )
            })?
        else {
            return Ok(None);
        };

        let entries = chunk.decode(source, last_idx_in_chunk)?;
        let mut cache = self.cache.lock().unwrap();

        let mut collected = Vec::new();

        for (oplog_index, entry) in entries {
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
    ) -> OplogArchiveResult<BTreeMap<golem_common::model::oplog::OplogIndex, OplogEntry>> {
        if n == 0 {
            return Ok(BTreeMap::new());
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
            if let Some(chunk) = self
                .fetch_and_cache_range(idx, last_idx)
                .await
                .map_err(|error| error.to_string())?
            {
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

        Ok(result)
    }

    async fn append(&self, chunk: &[(OplogIndex, OplogEntry)]) -> Result<u64, OplogError> {
        if chunk.is_empty() {
            return Ok(0);
        }
        self.stream.refuse_if_fenced()?;

        let mut total_bytes = 0u64;

        for sub_chunk in chunk.chunks(CompressedOplogArchiveService::MAX_CHUNK_SIZE) {
            let last_id = sub_chunk.last().unwrap().0;

            let entries: Vec<OplogEntry> =
                sub_chunk.iter().map(|(_, entry)| entry.clone()).collect();

            let compressed_chunk = CompressedOplogChunk::compress(entries).map_err(|error| {
                OplogError::Maintenance(format!("failed to compress oplog chunk: {error}"))
            })?;

            total_bytes += compressed_chunk.compressed_data.len() as u64;

            match self.stream.append(last_id, &compressed_chunk).await {
                Ok(()) => {}
                // A refused append wrote nothing, so there is nothing to reconcile: the shard's
                // new owner decides what this level holds.
                Err(refused @ OplogError::Fenced(_)) => return Err(refused),
                Err(append_error) => {
                    let first_id = sub_chunk.first().unwrap().0;
                    let uncached = Self::new(
                        self.agent_id.clone(),
                        self.agent_mode,
                        self.indexed_storage.clone(),
                        self.level,
                        self.retry_config.clone(),
                    );
                    let actual = uncached
                        .read_source(first_id, sub_chunk.len() as u64)
                        .await
                        .map_err(|read_error| {
                            OplogError::Maintenance(format!(
                                "failed to reconcile compressed oplog append for {} after {append_error}: {read_error}",
                                self.agent_id
                            ))
                        })?;
                    verify_persisted_entries(
                        OplogReadSource::Archive(self.level),
                        sub_chunk,
                        actual,
                    )
                    .map_err(|_| append_error)?;
                }
            }

            // Publish only data that was persisted or reconciled as persisted. The cache lock is
            // deliberately acquired after storage IO so host-call polling cannot deadlock the store.
            let mut cache = self.cache.lock().unwrap();
            for (idx, entry) in sub_chunk {
                cache.insert(*idx, entry.clone());
            }
        }

        Ok(total_bytes)
    }

    async fn verify_persisted(
        &self,
        entries: &[(OplogIndex, OplogEntry)],
    ) -> OplogArchiveResult<()> {
        let Some((start, _)) = entries.first() else {
            return Ok(());
        };
        let uncached = Self::new(
            self.agent_id.clone(),
            self.agent_mode,
            self.indexed_storage.clone(),
            self.level,
            self.retry_config.clone(),
        );
        let actual = uncached.read_source(*start, entries.len() as u64).await?;
        verify_persisted_entries(OplogReadSource::Archive(self.level), entries, actual)
            .map_err(|error| error.to_string())
    }

    async fn current_oplog_index(&self) -> OplogArchiveResult<OplogIndex> {
        self.stream.last_id().await
    }

    async fn drop_prefix(&self, last_dropped_id: OplogIndex) -> Result<u64, OplogError> {
        self.stream.refuse_if_fenced()?;
        let before = self.length().await.map_err(OplogError::Maintenance)?;
        let remaining = self.stream.drop_prefix(last_dropped_id).await?;
        // Ephemeral writers may append newer chunks while this maintenance operation is in flight.
        Ok(before.saturating_sub(remaining))
    }

    async fn length(&self) -> OplogArchiveResult<u64> {
        self.stream.length().await
    }

    async fn get_last_index(&self) -> OplogArchiveResult<OplogIndex> {
        self.current_oplog_index().await
    }

    fn fence(&self) -> Option<OplogFence> {
        self.stream.fence()
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

    /// The chunk's entries with their oplog indices, given the index of its last entry. Fails as
    /// corruption of `source` when the chunk does not decode, or holds a different number of
    /// entries than it declares.
    pub(crate) fn decode(
        &self,
        source: OplogReadSource,
        last_idx: OplogIndex,
    ) -> Result<Vec<(OplogIndex, OplogEntry)>, OplogReadError> {
        let entries = self.decompress().map_err(|error| {
            OplogReadError::corruption(
                source,
                format!("failed to decode compressed oplog chunk ending at {last_idx}: {error}"),
            )
        })?;
        if self.count == 0 || entries.len() as u64 != self.count {
            return Err(OplogReadError::corruption(
                source,
                format!(
                    "compressed oplog chunk ending at {last_idx} declares {} entries but contains {}",
                    self.count,
                    entries.len()
                ),
            ));
        }
        let first_idx = last_idx
            .as_u64()
            .checked_sub(self.count - 1)
            .ok_or_else(|| {
                OplogReadError::corruption(
                    source,
                    format!(
                        "compressed oplog chunk ending at {last_idx} has invalid count {}",
                        self.count
                    ),
                )
            })?;
        Ok((first_idx..)
            .zip(entries)
            .map(|(idx, entry)| (OplogIndex::from_u64(idx), entry))
            .collect())
    }
}
