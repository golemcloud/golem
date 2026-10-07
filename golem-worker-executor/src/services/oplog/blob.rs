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
use crate::services::oplog::multilayer::{OplogArchive, OplogArchiveResult};
use crate::services::oplog::reader::{OplogReadError, OplogReadSource, verify_persisted_entries};
use crate::services::oplog::{CompressedOplogChunk, OplogArchiveService, OplogError, OplogFence};
use crate::storage::indexed::{
    IndexedStorage, IndexedStorageError, IndexedStorageMetaNamespace, IndexedStorageNamespace,
};
use async_trait::async_trait;
use desert_rust::BinaryCodec;
use evicting_cache_map::EvictingCacheMap;
use golem_common::model::agent::AgentMode;
use golem_common::model::component::ComponentId;
use golem_common::model::environment::EnvironmentId;
use golem_common::model::oplog::{OplogEntry, OplogIndex};
use golem_common::model::{OwnedAgentId, RetryConfig, ScanCursor, ShardEpoch};
use golem_service_base::error::worker_executor::WorkerExecutorError;
use golem_service_base::storage::blob::{
    BlobStorage, BlobStorageLabelledApi, BlobStorageNamespace, join_blob_key,
};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tracing::warn;
use uuid::Uuid;

const MANIFEST_LABELS: StreamLabels = StreamLabels {
    svc: "blob_oplog",
    entity: "blob_chunk",
    op: "blob_manifest",
    what: "blob oplog manifest",
};

/// An oplog archive implementation that stores compressed chunks of the oplog in the configured
/// blob storage.
///
/// Blob storage has no conditional write, so which chunks belong to the oplog is not decided by
/// the objects present: a manifest in indexed storage lists them, and is written as the owner of
/// the agent's shard epoch like every other archive level. An object only its writer knows about
/// (a former owner's refused write, or one whose manifest entry a crash kept from landing) is
/// never read.
#[derive(Debug)]
pub struct BlobOplogArchiveService {
    blob_storage: Arc<dyn BlobStorage + Send + Sync>,
    indexed_storage: Arc<dyn IndexedStorage + Send + Sync>,
    level: usize,
    retry_config: RetryConfig,
}

impl BlobOplogArchiveService {
    const MAX_CHUNK_SIZE: usize = 4096;
    const CACHE_SIZE: usize = 4096;

    pub fn new(
        blob_storage: Arc<dyn BlobStorage + Send + Sync>,
        indexed_storage: Arc<dyn IndexedStorage + Send + Sync>,
        level: usize,
        retry_config: RetryConfig,
    ) -> Self {
        BlobOplogArchiveService {
            blob_storage,
            indexed_storage,
            level,
            retry_config,
        }
    }

    /// The level's manifest for an agent, asserting no epoch.
    fn manifest(
        &self,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
    ) -> FencedIndexedStream {
        FencedIndexedStream::new(
            owned_agent_id.agent_id(),
            BlobOplogArchive::manifest_namespace(owned_agent_id, agent_mode, self.level),
            MANIFEST_LABELS,
            self.indexed_storage.clone(),
            self.retry_config.clone(),
        )
    }
}

#[async_trait]
impl OplogArchiveService for BlobOplogArchiveService {
    async fn open(
        &self,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
        shard_epoch: Option<ShardEpoch>,
    ) -> Arc<dyn OplogArchive + Send + Sync> {
        Arc::new(
            BlobOplogArchive::opened(
                owned_agent_id.clone(),
                agent_mode,
                self.blob_storage.clone(),
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

    /// Removes the manifest with its recorded generation, then every object of the agent. Runs
    /// after the fenced delete of the agent's primary oplog, as for the compressed levels. The
    /// manifest goes first, so a failure in between leaves objects nothing lists, not a manifest
    /// listing objects that are gone.
    async fn delete(
        &self,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
    ) -> OplogArchiveResult<()> {
        self.manifest(owned_agent_id, agent_mode).delete().await?;
        self.blob_storage
            .with("blob_oplog", "delete")
            .delete_dir(
                BlobOplogArchive::blob_namespace(owned_agent_id, agent_mode, self.level),
                Path::new(&owned_agent_id.agent_name()),
            )
            .await
            .map(|_| ())
            .map_err(|error| {
                format!(
                    "failed to drop compressed oplog for worker {} in blob storage: {error}",
                    owned_agent_id.agent_id
                )
            })
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
            .unwrap_or_else(|error| panic!("Failed to check blob oplog archive existence: {error}"))
    }

    async fn try_exists(
        &self,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
    ) -> OplogArchiveResult<bool> {
        self.manifest(owned_agent_id, agent_mode).exists().await
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
            MANIFEST_LABELS,
            environment_id,
            component_id,
            modes,
            cursor,
            count,
            |agent_mode| IndexedStorageMetaNamespace::BlobOplogManifest { agent_mode, level },
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
            .unwrap_or_else(|error| panic!("Failed to read blob oplog archive index: {error}"))
    }

    async fn try_get_last_index(
        &self,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
    ) -> OplogArchiveResult<OplogIndex> {
        self.manifest(owned_agent_id, agent_mode).last_id().await
    }
}

/// A chunk's entry in the manifest: the object holding its bytes, and how many oplog entries it
/// holds. The manifest keys it by the chunk's last oplog index.
#[derive(Debug, Clone, BinaryCodec)]
#[desert(evolution())]
struct BlobChunkRef {
    object: String,
    count: u64,
}

#[derive(Debug)]
struct BlobOplogArchive {
    owned_agent_id: OwnedAgentId,
    agent_mode: AgentMode,
    blob_storage: Arc<dyn BlobStorage + Send + Sync>,
    indexed_storage: Arc<dyn IndexedStorage + Send + Sync>,
    level: usize,
    retry_config: RetryConfig,
    /// The chunks of this level, the only record of which objects belong to the oplog.
    manifest: FencedIndexedStream,
    /// `cache` is guarded by a `std` mutex rather than an async lock: the archive is used both by
    /// wasmtime store-polled futures (durable host calls) and by independent tokio tasks. Tokio's
    /// fair locks hand ownership to a queued waiter at wake time, before it is polled, so a
    /// store-polled future queued on an async lock could become its owner while the store is
    /// unable to poll it (wasmtime#11869/#11870), wedging every other user of the archive. Every
    /// critical section below is synchronous and never spans an `await`.
    #[allow(clippy::type_complexity)]
    cache: Mutex<
        EvictingCacheMap<
            OplogIndex,
            OplogEntry,
            { BlobOplogArchiveService::CACHE_SIZE },
            fn(OplogIndex, OplogEntry) -> (),
        >,
    >,
}

impl BlobOplogArchive {
    /// A handle that writes as the owner of `shard_epoch`, recorded on the manifest; `None`
    /// records and asserts nothing, for reads and executors without a shard assignment.
    async fn opened(
        owned_agent_id: OwnedAgentId,
        agent_mode: AgentMode,
        blob_storage: Arc<dyn BlobStorage + Send + Sync>,
        indexed_storage: Arc<dyn IndexedStorage + Send + Sync>,
        level: usize,
        retry_config: RetryConfig,
        shard_epoch: Option<ShardEpoch>,
    ) -> Self {
        let manifest = FencedIndexedStream::opened(
            owned_agent_id.agent_id(),
            Self::manifest_namespace(&owned_agent_id, agent_mode, level),
            MANIFEST_LABELS,
            indexed_storage.clone(),
            retry_config.clone(),
            shard_epoch,
        )
        .await;
        BlobOplogArchive {
            owned_agent_id,
            agent_mode,
            blob_storage,
            indexed_storage,
            level,
            retry_config,
            manifest,
            cache: Mutex::new(EvictingCacheMap::new()),
        }
    }

    fn manifest_namespace(
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
        level: usize,
    ) -> IndexedStorageNamespace {
        IndexedStorageNamespace::BlobOplogManifest {
            agent_id: owned_agent_id.agent_id(),
            agent_mode,
            level,
        }
    }

    fn blob_namespace(
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
        level: usize,
    ) -> BlobStorageNamespace {
        BlobStorageNamespace::CompressedOplog {
            environment_id: owned_agent_id.environment_id(),
            component_id: owned_agent_id.component_id(),
            agent_mode,
            level,
        }
    }

    fn namespace(&self) -> BlobStorageNamespace {
        Self::blob_namespace(&self.owned_agent_id, self.agent_mode, self.level)
    }

    /// A name no other write uses, so no writer can overwrite an object the manifest refers to.
    fn new_object(&self, last_idx: OplogIndex) -> String {
        join_blob_key(
            &self.owned_agent_id.agent_name(),
            &format!("{last_idx}-{}", Uuid::new_v4()),
        )
    }

    /// Deletes objects no manifest entry refers to. Best effort: an object left behind is never
    /// read, and goes when the agent is deleted.
    async fn delete_objects(&self, objects: Vec<String>) {
        if objects.is_empty() {
            return;
        }
        let paths = objects.into_iter().map(PathBuf::from).collect::<Vec<_>>();
        if let Err(error) = self
            .blob_storage
            .with("blob_oplog", "delete_objects")
            .delete_many(self.namespace(), &paths)
            .await
        {
            warn!(
                agent_id = %self.owned_agent_id,
                error = %error,
                "Failed to delete blob oplog objects no longer in the manifest"
            );
        }
    }

    /// The chunk the manifest lists as ending at `last_idx`.
    async fn listed(
        &self,
        last_idx: OplogIndex,
    ) -> Result<Option<BlobChunkRef>, IndexedStorageError> {
        Ok(self
            .manifest
            .closest::<BlobChunkRef>(last_idx)
            .await?
            .filter(|(idx, _)| *idx == last_idx)
            .map(|(_, chunk_ref)| chunk_ref))
    }

    /// The entries of the chunk a manifest entry refers to, or `None` when its object is gone.
    async fn load_chunk(
        &self,
        last_idx: OplogIndex,
        chunk_ref: &BlobChunkRef,
    ) -> Result<Option<Vec<(OplogIndex, OplogEntry)>>, OplogReadError> {
        let source = OplogReadSource::Archive(self.level);
        let Some(chunk) = self
            .blob_storage
            .with("blob_oplog", "read")
            .get::<CompressedOplogChunk>(self.namespace(), Path::new(&chunk_ref.object))
            .await
            .map_err(|error| {
                OplogReadError::source_failure(
                    source,
                    format!(
                        "failed to read compressed oplog for worker {} in blob storage: {error}",
                        self.owned_agent_id
                    ),
                )
            })?
        else {
            return Ok(None);
        };
        if chunk.count != chunk_ref.count {
            return Err(OplogReadError::corruption(
                source,
                format!(
                    "compressed oplog chunk ending at {last_idx} holds {} entries but the manifest lists {}",
                    chunk.count, chunk_ref.count
                ),
            ));
        }
        chunk.decode(source, last_idx).map(Some)
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
        let manifest_failure = |error| {
            OplogReadError::source_failure(
                source,
                format!(
                    "failed to read the blob oplog manifest for worker {} in indexed storage: {error}",
                    self.owned_agent_id
                ),
            )
        };
        let Some((last_idx, chunk_ref)) = self
            .manifest
            .closest::<BlobChunkRef>(end_of_range)
            .await
            .map_err(manifest_failure)?
        else {
            return Ok(None);
        };

        let Some(entries) = self.load_chunk(last_idx, &chunk_ref).await? else {
            // The chunk may have been dropped by a concurrent `drop_prefix` between reading its
            // manifest entry and fetching it. If the entry is gone from the manifest, treat it
            // as the layer boundary; otherwise the storage is inconsistent.
            let still_listed = self
                .manifest
                .closest::<BlobChunkRef>(last_idx)
                .await
                .map_err(manifest_failure)?
                .is_some_and(|(idx, listed)| idx == last_idx && listed.object == chunk_ref.object);
            return if still_listed {
                Err(OplogReadError::corruption(
                    source,
                    format!("compressed chunk ending at {last_idx} is missing"),
                ))
            } else {
                Ok(None)
            };
        };

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

    /// Checks that the chunk `listed` under `sub_chunk`'s last index holds the same oplog
    /// entries, as it does when an earlier attempt of the same transfer listed it.
    async fn covers(
        &self,
        listed: &BlobChunkRef,
        sub_chunk: &[(OplogIndex, OplogEntry)],
    ) -> Result<(), String> {
        let last_idx = sub_chunk.last().unwrap().0;
        let Some(existing) = self
            .load_chunk(last_idx, listed)
            .await
            .map_err(|error| error.to_string())?
        else {
            return Err(format!(
                "the blob oplog manifest lists a missing chunk ending at {last_idx}"
            ));
        };
        let incoming_start = sub_chunk.first().unwrap().0;
        if existing
            .first()
            .is_none_or(|(existing_start, _)| *existing_start > incoming_start)
        {
            return Err(format!(
                "the blob oplog manifest lists a shorter chunk ending at {last_idx}"
            ));
        }
        let actual = existing
            .into_iter()
            .filter(|(idx, _)| *idx >= incoming_start)
            .collect();
        verify_persisted_entries(OplogReadSource::Archive(self.level), sub_chunk, actual)
            .map_err(|error| error.to_string())
    }
}

#[async_trait]
impl OplogArchive for BlobOplogArchive {
    async fn read_source(
        &self,
        idx: OplogIndex,
        n: u64,
    ) -> OplogArchiveResult<BTreeMap<OplogIndex, OplogEntry>> {
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

    /// Stores each sub-chunk as an object, then lists it in the manifest as the owner of the
    /// shard epoch. An object is deleted again only when it can never be listed: its listing met
    /// an index the manifest already holds, or was refused while the manifest does not list it.
    /// After any other failure it is kept, because its entry may still land.
    async fn append(&self, chunk: &[(OplogIndex, OplogEntry)]) -> Result<u64, OplogError> {
        if chunk.is_empty() {
            return Ok(0);
        }
        self.manifest.refuse_if_fenced()?;

        let mut total_bytes = 0u64;

        for sub_chunk in chunk.chunks(BlobOplogArchiveService::MAX_CHUNK_SIZE) {
            let last_idx = sub_chunk.last().unwrap().0;
            let object = self.new_object(last_idx);

            let entries: Vec<OplogEntry> =
                sub_chunk.iter().map(|(_, entry)| entry.clone()).collect();

            let compressed_chunk = CompressedOplogChunk::compress(entries).map_err(|error| {
                OplogError::Maintenance(format!("failed to compress oplog chunk: {error}"))
            })?;

            self.blob_storage
                .with("blob_oplog", "append")
                .put(self.namespace(), Path::new(&object), &compressed_chunk)
                .await
                .map_err(|error| {
                    OplogError::Maintenance(format!(
                        "failed to store compressed oplog chunk for worker {} in blob storage: {error}",
                        self.owned_agent_id.agent_id
                    ))
                })?;

            let chunk_ref = BlobChunkRef {
                object: object.clone(),
                count: compressed_chunk.count,
            };
            match self.manifest.append_unless_held(last_idx, &chunk_ref).await {
                Ok(true) => total_bytes += compressed_chunk.compressed_data.len() as u64,
                // The storage answered that the index is held. This attempt stored nothing and
                // nothing of it is still on its way, so its object can never be listed. The
                // append counts as stored when the listed chunk holds the same entries, as it
                // does when an earlier attempt of the same transfer listed it.
                Ok(false) => {
                    self.delete_objects(vec![object]).await;
                    let listed = self.listed(last_idx).await.map_err(|error| {
                        OplogError::Maintenance(format!(
                            "failed to read the blob oplog manifest for {} after an append met a held index: {error}",
                            self.owned_agent_id.agent_id
                        ))
                    })?;
                    let Some(listed) = listed else {
                        return Err(OplogError::Maintenance(format!(
                            "the blob oplog manifest for {} does not accept a chunk ending at {last_idx}",
                            self.owned_agent_id.agent_id
                        )));
                    };
                    self.covers(&listed, sub_chunk).await.map_err(|error| {
                        OplogError::Maintenance(format!(
                            "failed to append blob oplog chunk for {}: {error}",
                            self.owned_agent_id.agent_id
                        ))
                    })?;
                }
                // The shard's new owner decides what this level holds, and no entry that asserts
                // this epoch can be stored any more. The object is deleted unless the manifest
                // lists it: an append repeated after a lost reply is refused on its second run
                // although its first run stored the entry.
                Err(refused @ OplogError::Fenced(_)) => {
                    if matches!(
                        self.listed(last_idx).await,
                        Ok(listed) if listed.as_ref().is_none_or(|listed| listed.object != object)
                    ) {
                        self.delete_objects(vec![object]).await;
                    }
                    return Err(refused);
                }
                // The append failed without a verdict: this attempt's entry may have landed, may
                // still land, or another attempt listed the chunk first. The object stays in
                // every case. Even where another object is listed now, an entry that arrives
                // after a trim has removed that listing is stored under the index it freed.
                Err(append_error) => {
                    let listed = self.listed(last_idx).await.map_err(|error| {
                        OplogError::Maintenance(format!(
                            "failed to reconcile blob oplog append for {} after {append_error}: {error}",
                            self.owned_agent_id.agent_id
                        ))
                    })?;
                    match listed {
                        // This attempt's entry landed after all.
                        Some(listed) if listed.object == object => {
                            total_bytes += compressed_chunk.compressed_data.len() as u64
                        }
                        // Another object is listed for this chunk. The append counts as stored
                        // when that object holds the same entries.
                        Some(listed) => {
                            self.covers(&listed, sub_chunk).await.map_err(|error| {
                                OplogError::Maintenance(format!(
                                    "failed to append blob oplog chunk for {} after {append_error}: {error}",
                                    self.owned_agent_id.agent_id
                                ))
                            })?;
                        }
                        None => return Err(append_error),
                    }
                }
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
        let uncached = Self::opened(
            self.owned_agent_id.clone(),
            self.agent_mode,
            self.blob_storage.clone(),
            self.indexed_storage.clone(),
            self.level,
            self.retry_config.clone(),
            None,
        )
        .await;
        let actual = uncached.read_source(*start, entries.len() as u64).await?;
        verify_persisted_entries(OplogReadSource::Archive(self.level), entries, actual)
            .map_err(|error| error.to_string())
    }

    async fn current_oplog_index(&self) -> OplogArchiveResult<OplogIndex> {
        self.manifest.last_id().await
    }

    /// Trims the manifest as the owner of the shard epoch, and only then deletes the objects it
    /// no longer lists. The agent's directory is not deleted here: that delete is recursive and
    /// unfenced, and could remove objects a newer owner or a concurrent writer has just stored.
    async fn drop_prefix(&self, last_dropped_id: OplogIndex) -> Result<u64, OplogError> {
        self.manifest.refuse_if_fenced()?;
        let dropped = self
            .manifest
            .read::<BlobChunkRef>(OplogIndex::NONE, last_dropped_id)
            .await
            .map_err(OplogError::Maintenance)?;
        self.manifest.drop_prefix(last_dropped_id).await?;
        *self.cache.lock().unwrap() = EvictingCacheMap::new();
        let drop_count = dropped.len() as u64;
        self.delete_objects(dropped.into_iter().map(|(_, chunk)| chunk.object).collect())
            .await;
        Ok(drop_count)
    }

    async fn length(&self) -> OplogArchiveResult<u64> {
        self.manifest.length().await
    }

    async fn get_last_index(&self) -> OplogArchiveResult<OplogIndex> {
        self.current_oplog_index().await
    }

    fn fence(&self) -> Option<OplogFence> {
        self.manifest.fence()
    }
}
