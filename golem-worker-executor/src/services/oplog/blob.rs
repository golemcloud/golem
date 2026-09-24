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

use crate::services::oplog::multilayer::{OplogArchive, OplogArchiveResult};
use crate::services::oplog::reader::{OplogReadError, OplogReadSource, verify_persisted_entries};
use crate::services::oplog::{
    CompressedOplogChunk, OplogArchiveService, decode_scan_cursor, next_scan_cursor,
};
use async_trait::async_trait;
use evicting_cache_map::EvictingCacheMap;
use golem_common::model::agent::AgentMode;
use golem_common::model::component::ComponentId;
use golem_common::model::environment::EnvironmentId;
use golem_common::model::oplog::{OplogEntry, OplogIndex};
use golem_common::model::{AgentId, OwnedAgentId, ScanCursor};
use golem_service_base::error::worker_executor::WorkerExecutorError;
use golem_service_base::storage::blob::{
    BlobStorage, BlobStorageLabelledApi, BlobStorageNamespace, ExistsResult,
};
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// An oplog archive implementation that uses the configured blob storage to store compressed
/// chunks of the oplog.
#[derive(Debug)]
pub struct BlobOplogArchiveService {
    blob_storage: Arc<dyn BlobStorage + Send + Sync>,
    level: usize,
}

impl BlobOplogArchiveService {
    const MAX_CHUNK_SIZE: usize = 4096;
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
    async fn open(
        &self,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
    ) -> Arc<dyn OplogArchive + Send + Sync> {
        match BlobOplogArchive::try_new(
            owned_agent_id.clone(),
            agent_mode,
            self.blob_storage.clone(),
            self.level,
        )
        .await
        {
            Ok(archive) => Arc::new(archive),
            Err(error) => {
                tracing::warn!(
                    agent_id = %owned_agent_id.agent_id,
                    error = %error,
                    "Failed to open blob oplog archive; operations will retry lazily"
                );
                Arc::new(LazyBlobOplogArchive {
                    owned_agent_id: owned_agent_id.clone(),
                    agent_mode,
                    blob_storage: self.blob_storage.clone(),
                    level: self.level,
                    archive: Mutex::new(None),
                })
            }
        }
    }

    async fn open_fresh(
        &self,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
    ) -> Arc<dyn OplogArchive + Send + Sync> {
        Arc::new(BlobOplogArchive::new_fresh(
            owned_agent_id.clone(),
            agent_mode,
            self.blob_storage.clone(),
            self.level,
        ))
    }

    async fn delete(
        &self,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
    ) -> OplogArchiveResult<()> {
        self.blob_storage
            .delete_dir(
                "blob_oplog",
                "delete",
                BlobStorageNamespace::CompressedOplog {
                    environment_id: owned_agent_id.environment_id(),
                    component_id: owned_agent_id.component_id(),
                    agent_mode,
                    level: self.level,
                },
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
        let archive = self.open(owned_agent_id, agent_mode).await;
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
        self.blob_storage
            .with("blob_oplog", "exists")
            .exists(
                BlobStorageNamespace::CompressedOplog {
                    environment_id: owned_agent_id.environment_id(),
                    component_id: owned_agent_id.component_id(),
                    agent_mode,
                    level: self.level,
                },
                Path::new(&owned_agent_id.agent_name()),
            )
            .await
            .map(|exists| exists == ExistsResult::Directory)
            .map_err(|error| format!("failed to check existence of compressed oplog for worker {} in blob storage: {error}", owned_agent_id.agent_id))
    }

    async fn scan_for_component(
        &self,
        environment_id: &EnvironmentId,
        component_id: &ComponentId,
        modes: Option<AgentMode>,
        cursor: ScanCursor,
        _count: u64,
    ) -> Result<(ScanCursor, Vec<OwnedAgentId>), WorkerExecutorError> {
        let state = decode_scan_cursor(&cursor, modes)?;
        if state.resume.is_some() {
            return Err(WorkerExecutorError::invalid_request(
                "Blob oplog archive does not accept a storage resume cursor",
            ));
        }
        let active_mode = state.mode;

        let blob_storage = self.blob_storage.with("blob_oplog", "scan_for_component");
        let owned_agent_ids = if blob_storage.exists(
            BlobStorageNamespace::CompressedOplog {
                environment_id: *environment_id,
                component_id: *component_id,
                agent_mode: active_mode,
                level: self.level,
            },
            Path::new(""),
        ).await.map_err(|err| {
            WorkerExecutorError::unknown(format!("Failed to check if compressed oplog root for component {component_id} exists in blob storage: {err}"))
        })? == ExistsResult::Directory
        {
            let paths = blob_storage
                .list_dir(
                BlobStorageNamespace::CompressedOplog {
                environment_id: *environment_id,
                component_id: *component_id,
                agent_mode: active_mode,
                level: self.level,
            },
            Path::new(""),
        ).await.map_err(|err| {
            WorkerExecutorError::unknown(format!("Failed to list entries of compressed oplog for component {component_id} in blob storage: {err}"))
        })?;

            paths
                .into_iter()
                .map(|path| {
                    let agent_name = path.file_name().unwrap().to_str().unwrap();
                    OwnedAgentId {
                        environment_id: *environment_id,
                        agent_id: AgentId {
                            component_id: *component_id,
                            agent_id: agent_name.to_string(),
                        },
                    }
                })
                .collect()
        } else {
            Vec::new()
        };

        let next_cursor = next_scan_cursor(state, modes, None)?;
        Ok((next_cursor, owned_agent_ids))
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
        if BlobOplogArchive::exists(
            owned_agent_id.clone(),
            agent_mode,
            self.blob_storage.clone(),
            self.level,
        )
        .await?
        {
            let entries = BlobOplogArchive::entries(
                owned_agent_id.clone(),
                agent_mode,
                self.blob_storage.clone(),
                self.level,
            )
            .await?;
            Ok(entries.keys().last().copied().unwrap_or(OplogIndex::NONE))
        } else {
            Ok(OplogIndex::NONE)
        }
    }
}

#[derive(Debug)]
struct BlobOplogArchive {
    owned_agent_id: OwnedAgentId,
    agent_mode: AgentMode,
    blob_storage: Arc<dyn BlobStorage + Send + Sync>,
    level: usize,
    /// `entries`, `created` and `cache` are guarded by `std` primitives rather than async locks:
    /// the archive is used both by wasmtime store-polled futures (durable host calls) and by
    /// independent tokio tasks. Tokio's fair locks hand ownership to a queued waiter at wake
    /// time, before it is polled, so a store-polled future queued on an async lock could become
    /// its owner while the store is unable to poll it (wasmtime#11869/#11870), wedging every
    /// other user of the archive. Every critical section below is synchronous and never spans an
    /// `await`.
    entries: Mutex<BTreeMap<OplogIndex, PathBuf>>,
    deleting: Mutex<HashSet<OplogIndex>>,
    uncertain_writes: Mutex<HashSet<OplogIndex>>,
    created: AtomicBool,
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

#[derive(Debug)]
struct LazyBlobOplogArchive {
    owned_agent_id: OwnedAgentId,
    agent_mode: AgentMode,
    blob_storage: Arc<dyn BlobStorage + Send + Sync>,
    level: usize,
    archive: Mutex<Option<Arc<BlobOplogArchive>>>,
}

impl LazyBlobOplogArchive {
    async fn open(&self) -> OplogArchiveResult<Arc<BlobOplogArchive>> {
        if let Some(archive) = self.archive.lock().unwrap().clone() {
            return Ok(archive);
        }
        let candidate = Arc::new(
            BlobOplogArchive::try_new(
                self.owned_agent_id.clone(),
                self.agent_mode,
                self.blob_storage.clone(),
                self.level,
            )
            .await?,
        );
        let mut archive = self.archive.lock().unwrap();
        Ok(archive.get_or_insert(candidate).clone())
    }
}

#[async_trait]
impl OplogArchive for LazyBlobOplogArchive {
    async fn read_source(
        &self,
        idx: OplogIndex,
        n: u64,
    ) -> OplogArchiveResult<BTreeMap<OplogIndex, OplogEntry>> {
        self.open().await?.read_source(idx, n).await
    }

    async fn append(&self, chunk: &[(OplogIndex, OplogEntry)]) -> OplogArchiveResult<u64> {
        self.open().await?.append(chunk).await
    }

    async fn verify_persisted(
        &self,
        entries: &[(OplogIndex, OplogEntry)],
    ) -> OplogArchiveResult<()> {
        self.open().await?.verify_persisted(entries).await
    }

    async fn current_oplog_index(&self) -> OplogArchiveResult<OplogIndex> {
        self.open().await?.current_oplog_index().await
    }

    async fn drop_prefix(&self, last_dropped_id: OplogIndex) -> OplogArchiveResult<u64> {
        self.open().await?.drop_prefix(last_dropped_id).await
    }

    async fn length(&self) -> OplogArchiveResult<u64> {
        self.open().await?.length().await
    }

    async fn get_last_index(&self) -> OplogArchiveResult<OplogIndex> {
        self.open().await?.get_last_index().await
    }
}

impl BlobOplogArchive {
    pub async fn try_new(
        owned_agent_id: OwnedAgentId,
        agent_mode: AgentMode,
        blob_storage: Arc<dyn BlobStorage + Send + Sync>,
        level: usize,
    ) -> OplogArchiveResult<Self> {
        let exists = Self::exists(
            owned_agent_id.clone(),
            agent_mode,
            blob_storage.clone(),
            level,
        )
        .await?;
        let created = AtomicBool::new(exists);
        let entries = Mutex::new(if exists {
            Self::entries(
                owned_agent_id.clone(),
                agent_mode,
                blob_storage.clone(),
                level,
            )
            .await?
        } else {
            BTreeMap::new()
        });

        Ok(BlobOplogArchive {
            owned_agent_id,
            agent_mode,
            blob_storage,
            level,
            created,
            entries,
            deleting: Mutex::new(HashSet::new()),
            uncertain_writes: Mutex::new(HashSet::new()),
            cache: Mutex::new(EvictingCacheMap::new()),
        })
    }

    pub fn new_fresh(
        owned_agent_id: OwnedAgentId,
        agent_mode: AgentMode,
        blob_storage: Arc<dyn BlobStorage + Send + Sync>,
        level: usize,
    ) -> Self {
        BlobOplogArchive {
            owned_agent_id,
            agent_mode,
            blob_storage,
            level,
            created: AtomicBool::new(false),
            entries: Mutex::new(BTreeMap::new()),
            deleting: Mutex::new(HashSet::new()),
            uncertain_writes: Mutex::new(HashSet::new()),
            cache: Mutex::new(EvictingCacheMap::new()),
        }
    }

    async fn ensure_is_created(&self) -> OplogArchiveResult<()> {
        // `create_dir` is idempotent in every blob storage backend, so racing creators are
        // harmless.
        if self.created.load(Ordering::Acquire) {
            return Ok(());
        }
        self.blob_storage
            .with("blob_oplog", "new")
            .create_dir(
                BlobStorageNamespace::CompressedOplog {
                    environment_id: self.owned_agent_id.environment_id(),
                    component_id: self.owned_agent_id.component_id(),
                    agent_mode: self.agent_mode,
                    level: self.level,
                },
                Path::new(&self.owned_agent_id.agent_name()),
            )
            .await
            .map_err(|error| {
                format!(
                    "failed to create compressed oplog directory for worker {} in blob storage: {error}",
                    self.owned_agent_id.agent_id
                )
            })?;

        self.created.store(true, Ordering::Release);
        Ok(())
    }

    pub(crate) async fn exists(
        owned_agent_id: OwnedAgentId,
        agent_mode: AgentMode,
        blob_storage: Arc<dyn BlobStorage + Send + Sync>,
        level: usize,
    ) -> OplogArchiveResult<bool> {
        blob_storage
            .with("blob_oplog", "exists")
            .exists(
                BlobStorageNamespace::CompressedOplog {
                    environment_id: owned_agent_id.environment_id(),
                    component_id: owned_agent_id.component_id(),
                    agent_mode,
                    level,
                },
                Path::new(&owned_agent_id.agent_name()),
            )
            .await
            .map(|exists| exists == ExistsResult::Directory)
            .map_err(|error| format!("failed to check existence of compressed oplog for worker {} in blob storage: {error}", owned_agent_id.agent_id))
    }

    pub(crate) async fn entries(
        owned_agent_id: OwnedAgentId,
        agent_mode: AgentMode,
        blob_storage: Arc<dyn BlobStorage + Send + Sync>,
        level: usize,
    ) -> OplogArchiveResult<BTreeMap<OplogIndex, PathBuf>> {
        let paths = blob_storage
            .with("blob_oplog", "new")
            .list_dir(
                BlobStorageNamespace::CompressedOplog {
                    environment_id: owned_agent_id.environment_id(),
                    component_id: owned_agent_id.component_id(),
                    agent_mode,
                    level,
                },
                Path::new(&owned_agent_id.agent_name()),
            )
            .await
            .map_err(|error| format!("failed to list entries of compressed oplog for worker {} in blob storage: {error}", owned_agent_id.agent_id))?;

        paths
            .into_iter()
            .map(|path| Self::path_to_oplog_index(&path).map(|idx| (idx, path)))
            .collect::<OplogArchiveResult<BTreeMap<OplogIndex, PathBuf>>>()
    }

    pub(crate) fn path_to_oplog_index(path: &Path) -> OplogArchiveResult<OplogIndex> {
        path.file_name()
            .and_then(|s| s.to_str())
            .and_then(|s| s.parse::<u64>().ok())
            .map(OplogIndex::from_u64)
            .ok_or_else(|| format!("failed to parse oplog index from path: {path:?}"))
    }

    pub(crate) fn oplog_index_to_path(&self, idx: OplogIndex) -> PathBuf {
        let mut path = PathBuf::new();
        path.push(self.owned_agent_id.agent_name());
        path.push(idx.to_string());
        path
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
        // The `entries` lock must not be held across the storage read below: an async lock held
        // across IO by a wasmtime store-polled future can deadlock the store
        // (wasmtime#11869/#11870). The chunk key is copied out under a short lock instead.
        let last_idx = {
            let entries = self.entries.lock().unwrap();
            // Find the first chunk whose last index is >= end_of_range
            entries.keys().find(|k| **k >= end_of_range).copied()
        };

        let last_idx = if let Some(last_idx) = last_idx {
            last_idx
        } else {
            return Ok(None);
        };

        let chunk: CompressedOplogChunk = match self
            .blob_storage
            .with("blob_oplog", "read")
            .get(
                BlobStorageNamespace::CompressedOplog {
                    environment_id: self.owned_agent_id.environment_id(),
                    component_id: self.owned_agent_id.component_id(),
                    agent_mode: self.agent_mode,
                    level: self.level,
                },
                &self.oplog_index_to_path(last_idx),
            )
            .await
            .map_err(|error| {
                OplogReadError::source_failure(
                    source,
                    format!(
                        "failed to read compressed oplog for worker {} in blob storage: {error}",
                        self.owned_agent_id
                    ),
                )
            })? {
            Some(chunk) => chunk,
            None => {
                // A concurrent or indeterminate prefix deletion may have removed the object after
                // its key was selected. A pending deletion makes that absence authoritative; an
                // unmarked missing object is storage corruption.
                {
                    let mut deleting = self.deleting.lock().unwrap();
                    if deleting.remove(&last_idx) {
                        self.entries.lock().unwrap().remove(&last_idx);
                        return Ok(None);
                    }
                }
                if self.entries.lock().unwrap().contains_key(&last_idx) {
                    return Err(OplogReadError::corruption(
                        source,
                        format!("compressed chunk ending at {last_idx} is missing"),
                    ));
                } else {
                    return Ok(None);
                }
            }
        };

        let entries = chunk.decompress().map_err(|error| {
            OplogReadError::corruption(
                source,
                format!("failed to decode compressed oplog chunk ending at {last_idx}: {error}"),
            )
        })?;
        if chunk.count == 0 || entries.len() as u64 != chunk.count {
            return Err(OplogReadError::corruption(
                source,
                format!(
                    "compressed oplog chunk ending at {last_idx} declares {} entries but contains {}",
                    chunk.count,
                    entries.len()
                ),
            ));
        }
        let first_idx_in_chunk =
            last_idx
                .as_u64()
                .checked_sub(chunk.count - 1)
                .ok_or_else(|| {
                    OplogReadError::corruption(
                        source,
                        format!(
                            "compressed oplog chunk ending at {last_idx} has invalid count {}",
                            chunk.count
                        ),
                    )
                })?;
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

    async fn append(&self, chunk: &[(OplogIndex, OplogEntry)]) -> OplogArchiveResult<u64> {
        self.ensure_is_created().await?;

        if chunk.is_empty() {
            return Ok(0);
        }

        let mut total_bytes = 0u64;

        for sub_chunk in chunk.chunks(BlobOplogArchiveService::MAX_CHUNK_SIZE) {
            let last = sub_chunk.last().unwrap();
            let oplog_index = last.0;
            let path = self.oplog_index_to_path(oplog_index);

            let entries: Vec<OplogEntry> =
                sub_chunk.iter().map(|(_, entry)| entry.clone()).collect();

            let compressed_chunk = CompressedOplogChunk::compress(entries)
                .unwrap_or_else(|err| panic!("failed to compress oplog chunk: {err}"));

            let namespace = BlobStorageNamespace::CompressedOplog {
                environment_id: self.owned_agent_id.environment_id(),
                component_id: self.owned_agent_id.component_id(),
                agent_mode: self.agent_mode,
                level: self.level,
            };

            let should_reconcile = self.entries.lock().unwrap().contains_key(&oplog_index)
                || self.uncertain_writes.lock().unwrap().contains(&oplog_index);
            if should_reconcile
                && let Some(existing) = self
                    .blob_storage
                    .with("blob_oplog", "append_reconcile")
                    .get::<CompressedOplogChunk>(namespace.clone(), &path)
                    .await
                    .map_err(|error| {
                        format!(
                            "failed to reconcile compressed oplog chunk for worker {} in blob storage: {error}",
                            self.owned_agent_id.agent_id
                        )
                    })?
            {
                let existing_entries = existing.decompress().map_err(|error| {
                    format!(
                        "failed to decode existing compressed oplog chunk for worker {}: {error}",
                        self.owned_agent_id.agent_id
                    )
                })?;
                let existing_start = oplog_index
                    .as_u64()
                    .checked_sub(existing.count.saturating_sub(1))
                    .ok_or_else(|| {
                        format!(
                            "existing compressed oplog chunk ending at {oplog_index} has invalid count {}",
                            existing.count
                        )
                    })?;
                let incoming_start = sub_chunk.first().unwrap().0.as_u64();
                if existing_entries.len() as u64 != existing.count {
                    return Err(format!(
                        "existing compressed oplog chunk ending at {oplog_index} declares {} entries but contains {}",
                        existing.count,
                        existing_entries.len()
                    ));
                }
                let overlap_start = existing_start.max(incoming_start);
                let expected = &sub_chunk[(overlap_start - incoming_start) as usize..];
                let actual = existing_entries[(overlap_start - existing_start) as usize..]
                    .iter()
                    .cloned()
                    .enumerate()
                    .map(|(offset, entry)| {
                        (OplogIndex::from_u64(overlap_start + offset as u64), entry)
                    })
                    .collect();
                verify_persisted_entries(OplogReadSource::Archive(self.level), expected, actual)
                    .map_err(|error| error.to_string())?;
                if existing_start <= incoming_start {
                    self.entries.lock().unwrap().insert(oplog_index, path);
                    self.uncertain_writes.lock().unwrap().remove(&oplog_index);
                    continue;
                }
            }

            // The `entries` lock must not be held across the storage write: an async lock held
            // across IO by a wasmtime store-polled future can deadlock the store
            // (wasmtime#11869/#11870). The chunk becomes visible to readers only after the write
            // succeeded, which is the same observable order as before.
            total_bytes += compressed_chunk.compressed_data.len() as u64;
            self.uncertain_writes.lock().unwrap().insert(oplog_index);
            self.blob_storage
                .with("blob_oplog", "append")
                .put(
                    namespace,
                    &path,
                    &compressed_chunk,
                )
                .await
                .map_err(|error| {
                    format!(
                        "failed to store compressed oplog chunk for worker {} in blob storage: {error}",
                        self.owned_agent_id.agent_id
                    )
                })?;

            self.entries.lock().unwrap().insert(oplog_index, path);
            self.uncertain_writes.lock().unwrap().remove(&oplog_index);
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
        let uncached = Self::try_new(
            self.owned_agent_id.clone(),
            self.agent_mode,
            self.blob_storage.clone(),
            self.level,
        )
        .await?;
        let actual = uncached.read_source(*start, entries.len() as u64).await?;
        verify_persisted_entries(OplogReadSource::Archive(self.level), entries, actual)
            .map_err(|error| error.to_string())
    }

    async fn current_oplog_index(&self) -> OplogArchiveResult<OplogIndex> {
        let entries = self.entries.lock().unwrap();
        Ok(entries
            .keys()
            .last()
            .copied()
            .unwrap_or_else(|| OplogIndex::from_u64(0)))
    }

    async fn drop_prefix(&self, last_dropped_id: OplogIndex) -> OplogArchiveResult<u64> {
        self.ensure_is_created().await?;

        let idx_to_drop = {
            let entries = self.entries.lock().unwrap();
            entries
                .keys()
                .filter(|key| **key <= last_dropped_id)
                .cloned()
                .collect::<Vec<_>>()
        };

        let to_drop = idx_to_drop
            .iter()
            .map(|idx| {
                let mut path = PathBuf::new();
                path.push(self.owned_agent_id.agent_name());
                path.push(idx.to_string());
                path
            })
            .collect::<Vec<_>>();

        let ns = BlobStorageNamespace::CompressedOplog {
            environment_id: self.owned_agent_id.environment_id(),
            component_id: self.owned_agent_id.component_id(),
            agent_mode: self.agent_mode,
            level: self.level,
        };

        let mut dropped = 0;
        for (idx, path) in idx_to_drop.iter().zip(&to_drop) {
            self.deleting.lock().unwrap().insert(*idx);
            let result = self
                .blob_storage
                .with("blob_oplog", "drop_prefix")
                .delete(ns.clone(), path)
                .await;
            match result {
                Ok(()) => {
                    self.entries.lock().unwrap().remove(idx);
                    self.deleting.lock().unwrap().remove(idx);
                    *self.cache.lock().unwrap() = EvictingCacheMap::new();
                    dropped += 1;
                }
                Err(error) => {
                    *self.cache.lock().unwrap() = EvictingCacheMap::new();
                    return Err(format!(
                        "failed to drop compressed oplog chunk for worker {} in blob storage: {error}",
                        self.owned_agent_id.agent_id
                    ));
                }
            }
        }

        let is_empty = self.entries.lock().unwrap().is_empty();

        if is_empty {
            let was_created = self.created.swap(false, Ordering::AcqRel);
            if was_created
                && let Err(error) = self
                    .blob_storage
                    .with("blob_oplog", "drop_prefix")
                    .delete_dir(
                        BlobStorageNamespace::CompressedOplog {
                            environment_id: self.owned_agent_id.environment_id(),
                            component_id: self.owned_agent_id.component_id(),
                            agent_mode: self.agent_mode,
                            level: self.level,
                        },
                        Path::new(&self.owned_agent_id.agent_name()),
                    )
                    .await
            {
                tracing::warn!(
                    agent_id = %self.owned_agent_id.agent_id,
                    error = %error,
                    "Failed to remove empty compressed oplog directory after deleting its chunks"
                );
            }
        }

        Ok(dropped)
    }

    async fn length(&self) -> OplogArchiveResult<u64> {
        let entries = self.entries.lock().unwrap();
        Ok(entries.len() as u64)
    }

    async fn get_last_index(&self) -> OplogArchiveResult<OplogIndex> {
        self.current_oplog_index().await
    }
}
