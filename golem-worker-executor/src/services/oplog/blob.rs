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

use crate::services::oplog::multilayer::OplogArchive;
use crate::services::oplog::reader::{
    OplogReadError, OplogReadSource, fail_stop, verify_persisted_entries,
};
use crate::services::oplog::{
    CompressedOplogChunk, OplogArchiveService, cursor_value, next_scan_cursor, scan_modes,
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
use std::collections::BTreeMap;
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
        Arc::new(
            BlobOplogArchive::new(
                owned_agent_id.clone(),
                agent_mode,
                self.blob_storage.clone(),
                self.level,
            )
            .await,
        )
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

    async fn delete(&self, owned_agent_id: &OwnedAgentId, agent_mode: AgentMode) {
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
            .unwrap_or_else(|err| {
                panic!(
                    "failed to drop compressed oplog for worker {} in blob storage: {err}",
                    owned_agent_id.agent_id
                )
            });
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
            .unwrap_or_else(|err| {
                panic!(
                    "failed to check existence of compressed oplog for worker {} in blob storage: {err}",
                    owned_agent_id.agent_id
                )
            })
    }

    async fn scan_for_component(
        &self,
        environment_id: &EnvironmentId,
        component_id: &ComponentId,
        modes: Option<AgentMode>,
        cursor: ScanCursor,
        _count: u64,
    ) -> Result<(ScanCursor, Vec<OwnedAgentId>), WorkerExecutorError> {
        let layer = cursor.layer;
        let (active_mode, next_mode) = scan_modes(modes, cursor.cursor);
        let cursor_val = cursor_value(cursor.cursor);

        if cursor_val != 0 {
            return Err(WorkerExecutorError::unknown(
                "Cannot use cursor with blob oplog archive",
            ));
        }

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

        // Storage cursor is always 0 (single-page scan), so let next_scan_cursor
        // advance to the next mode if there is one.
        let next_cursor = next_scan_cursor(0, active_mode, next_mode, layer);
        Ok((next_cursor, owned_agent_ids))
    }

    async fn get_last_index(
        &self,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
    ) -> OplogIndex {
        if BlobOplogArchive::exists(
            owned_agent_id.clone(),
            agent_mode,
            self.blob_storage.clone(),
            self.level,
        )
        .await
        {
            let entries = BlobOplogArchive::entries(
                owned_agent_id.clone(),
                agent_mode,
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

impl BlobOplogArchive {
    pub async fn new(
        owned_agent_id: OwnedAgentId,
        agent_mode: AgentMode,
        blob_storage: Arc<dyn BlobStorage + Send + Sync>,
        level: usize,
    ) -> Self {
        let exists = Self::exists(
            owned_agent_id.clone(),
            agent_mode,
            blob_storage.clone(),
            level,
        )
        .await;
        let created = AtomicBool::new(exists);
        let entries = Mutex::new(if exists {
            Self::entries(
                owned_agent_id.clone(),
                agent_mode,
                blob_storage.clone(),
                level,
            )
            .await
        } else {
            BTreeMap::new()
        });

        BlobOplogArchive {
            owned_agent_id,
            agent_mode,
            blob_storage,
            level,
            created,
            entries,
            cache: Mutex::new(EvictingCacheMap::new()),
        }
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
            cache: Mutex::new(EvictingCacheMap::new()),
        }
    }

    async fn ensure_is_created(&self) {
        // `create_dir` is idempotent in every blob storage backend, so racing creators are
        // harmless.
        if self.created.load(Ordering::Acquire) {
            return;
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
            .unwrap_or_else(|err| {
                panic!(
                    "failed to create compressed oplog directory for worker {} in blob storage: {err}",
                    self.owned_agent_id.agent_id
                )
            });

        self.created.store(true, Ordering::Release);
    }

    pub(crate) async fn exists(
        owned_agent_id: OwnedAgentId,
        agent_mode: AgentMode,
        blob_storage: Arc<dyn BlobStorage + Send + Sync>,
        level: usize,
    ) -> bool {
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
            .unwrap_or_else(|err| {
                panic!(
                    "failed to check existence of compressed oplog for worker {} in blob storage: {err}",
                    owned_agent_id.agent_id
                )
            })
    }

    pub(crate) async fn entries(
        owned_agent_id: OwnedAgentId,
        agent_mode: AgentMode,
        blob_storage: Arc<dyn BlobStorage + Send + Sync>,
        level: usize,
    ) -> BTreeMap<OplogIndex, PathBuf> {
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
            .unwrap_or_else(|err| {
                panic!(
                "failed to list entries of compressed oplog for worker {} in blob storage: {err}",
                owned_agent_id.agent_id
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
                // The chunk may have been dropped by a concurrent `drop_prefix` between copying
                // its key and fetching it. If its key is gone from the entries map, treat it as
                // the layer boundary; otherwise the storage is genuinely inconsistent.
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
    async fn read_source(&self, idx: OplogIndex, n: u64) -> BTreeMap<OplogIndex, OplogEntry> {
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
        self.ensure_is_created().await;

        if chunk.is_empty() {
            return 0;
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

            total_bytes += compressed_chunk.compressed_data.len() as u64;

            // The `entries` lock must not be held across the storage write: an async lock held
            // across IO by a wasmtime store-polled future can deadlock the store
            // (wasmtime#11869/#11870). The chunk becomes visible to readers only after the write
            // succeeded, which is the same observable order as before.
            self.blob_storage
                .with("blob_oplog", "append")
                .put(
                    BlobStorageNamespace::CompressedOplog {
                        environment_id: self.owned_agent_id.environment_id(),
                        component_id: self.owned_agent_id.component_id(),
                        agent_mode: self.agent_mode,
                        level: self.level,
                    },
                    &path,
                    &compressed_chunk,
                )
                .await
                .unwrap_or_else(|err| {
                    panic!(
                        "failed to store compressed oplog chunk for worker {} in blob storage: {err}",
                        self.owned_agent_id.agent_id
                    )
                });

            self.entries.lock().unwrap().insert(oplog_index, path);
        }

        total_bytes
    }

    async fn verify_persisted(&self, entries: &[(OplogIndex, OplogEntry)]) {
        let Some((start, _)) = entries.first() else {
            return;
        };
        let uncached = Self::new(
            self.owned_agent_id.clone(),
            self.agent_mode,
            self.blob_storage.clone(),
            self.level,
        )
        .await;
        let actual = uncached.read_source(*start, entries.len() as u64).await;
        fail_stop(verify_persisted_entries(
            OplogReadSource::Archive(self.level),
            entries,
            actual,
        ));
    }

    async fn current_oplog_index(&self) -> OplogIndex {
        let entries = self.entries.lock().unwrap();
        entries
            .keys()
            .last()
            .copied()
            .unwrap_or_else(|| OplogIndex::from_u64(0))
    }

    async fn drop_prefix(&self, last_dropped_id: OplogIndex) -> u64 {
        self.ensure_is_created().await;

        // The keys are removed from the map before the blobs are deleted, so concurrent readers
        // either still find the chunk in storage or observe its key gone from the map and treat
        // it as the layer boundary.
        let (idx_to_drop, is_empty) = {
            let mut entries = self.entries.lock().unwrap();
            let idx_to_drop = entries
                .keys()
                .filter(|key| **key <= last_dropped_id)
                .cloned()
                .collect::<Vec<_>>();
            for idx in &idx_to_drop {
                let _ = entries.remove(idx);
            }
            (idx_to_drop, entries.is_empty())
        };

        let drop_count = idx_to_drop.len();
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

        self.blob_storage
            .with("blob_oplog", "drop_prefix")
            .delete_many(ns, &to_drop)
            .await
            .unwrap_or_else(|err| {
                panic!(
                    "failed to drop compressed oplog chunks for worker {} in blob storage: {err}",
                    self.owned_agent_id.agent_id
                )
            });

        if is_empty {
            let was_created = self.created.swap(false, Ordering::AcqRel);
            if was_created {
                self.blob_storage
                .with("blob_oplog", "drop_prefix")
                .delete_dir(BlobStorageNamespace::CompressedOplog {
                    environment_id: self.owned_agent_id.environment_id(),
                    component_id: self.owned_agent_id.component_id(),
                    agent_mode: self.agent_mode,
                    level: self.level,
                },
                Path::new(&self.owned_agent_id.agent_name())).await.unwrap_or_else(|err| {
                    panic!(
                        "failed to drop compressed oplog directory for worker {} in blob storage: {err}",
                        self.owned_agent_id.agent_id
                    )
                });
            }
        }

        drop_count as u64
    }

    async fn length(&self) -> u64 {
        let entries = self.entries.lock().unwrap();
        entries.len() as u64
    }

    async fn get_last_index(&self) -> OplogIndex {
        self.current_oplog_index().await
    }
}
