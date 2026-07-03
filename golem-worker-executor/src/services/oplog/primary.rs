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

use crate::metrics::oplog::{record_oplog_call, record_oplog_storage_retry};
use crate::metrics::storage::{
    STORAGE_TYPE_OPLOG, record_storage_bytes_written, record_storage_objects_deleted,
    record_storage_objects_written,
};
use crate::model::ExecutionStatus;
use crate::services::oplog::{
    CommitLevel, OpenOplogs, Oplog, OplogConstructor, OplogService, OrderedOplogStart,
    PendingUpload, ReservedPayload, cursor_value, next_scan_cursor, scan_modes,
};
use crate::storage::indexed::{
    IndexedStorage, IndexedStorageError, IndexedStorageLabelledApi, IndexedStorageMetaNamespace,
    IndexedStorageNamespace,
};
use async_trait::async_trait;
use futures::FutureExt;
use golem_common::model::RetryConfig;
use golem_common::model::account::AccountId;
use golem_common::model::agent::AgentMode;
use golem_common::model::component::ComponentId;
use golem_common::model::environment::EnvironmentId;
use golem_common::model::oplog::{
    OplogEntry, OplogIndex, PayloadId, PersistenceLevel, RawOplogPayload,
};
use golem_common::model::{AgentId, AgentMetadata, AgentStatusRecord, OwnedAgentId, ScanCursor};
use golem_common::read_only_lock;
use golem_common::retries::get_delay;
use golem_service_base::error::worker_executor::WorkerExecutorError;
use golem_service_base::storage::blob::{BlobStorage, BlobStorageNamespace};
use std::cmp::{max, min};
use std::collections::{BTreeMap, VecDeque};
use std::fmt::{Debug, Formatter};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use tracing::{error, warn};

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

/// The primary oplog service implementation, suitable for direct use (top level of a multi-layered setup).
///
/// Stores and retrieves individual oplog entries from the `IndexedStorage` implementation configured for
/// the executor.
#[derive(Clone, Debug)]
pub struct PrimaryOplogService {
    indexed_storage: Arc<dyn IndexedStorage + Send + Sync>,
    blob_storage: Arc<dyn BlobStorage + Send + Sync>,
    replicas: u8,
    max_operations_before_commit: u64,
    max_operations_before_commit_in_persist_nothing: u64,
    max_payload_size: usize,
    retry_config: RetryConfig,
    oplogs: OpenOplogs,
}

impl PrimaryOplogService {
    pub async fn new(
        indexed_storage: Arc<dyn IndexedStorage + Send + Sync>,
        blob_storage: Arc<dyn BlobStorage + Send + Sync>,
        max_operations_before_commit: u64,
        max_operations_before_commit_in_persist_nothing: u64,
        max_payload_size: usize,
        retry_config: RetryConfig,
    ) -> Self {
        let replicas = retry_storage_op(&retry_config, "number_of_replicas", "global", || {
            let is = indexed_storage.clone();
            async move { is.with("oplog", "new").number_of_replicas().await }
        })
        .await;
        Self {
            indexed_storage,
            blob_storage,
            replicas,
            max_operations_before_commit,
            max_operations_before_commit_in_persist_nothing,
            max_payload_size,
            retry_config,
            oplogs: OpenOplogs::new("primary oplog"),
        }
    }

    fn oplog_key(agent_id: &AgentId) -> String {
        agent_id.to_redis_key()
    }

    pub fn key_prefix(component_id: &ComponentId) -> String {
        component_id.0.to_string()
    }

    async fn get_last_index_from_storage(
        indexed_storage: &(dyn IndexedStorage + Send + Sync),
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
        retry_config: &RetryConfig,
    ) -> OplogIndex {
        let key = Self::oplog_key(&owned_agent_id.agent_id);
        let agent_id = owned_agent_id.agent_id();
        OplogIndex::from_u64(
            retry_storage_op(retry_config, "get_last_index", &key, || {
                let ns = IndexedStorageNamespace::OpLog {
                    agent_id: agent_id.clone(),
                    agent_mode,
                };
                let key = key.clone();
                async move {
                    indexed_storage
                        .with_entity("oplog", "get_last_index", "entry")
                        .last_id(ns, &key)
                        .await
                }
            })
            .await
            .unwrap_or_default(),
        )
    }

    pub fn get_agent_id_from_key(key: &str, component_id: &ComponentId) -> AgentId {
        let redis_prefix = format!("{}:", component_id.0);
        if key.starts_with(&redis_prefix) {
            let agent_name = &key[redis_prefix.len()..];
            AgentId {
                agent_id: agent_name.to_string(),
                component_id: *component_id,
            }
        } else {
            panic!("Failed to get worker id from indexed storage key: {key}")
        }
    }

    async fn upload_raw_payload(
        blob_storage: Arc<dyn BlobStorage + Send + Sync>,
        max_payload_size: usize,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
        data: Vec<u8>,
    ) -> Result<RawOplogPayload, String> {
        if data.len() > max_payload_size {
            let payload_id: PayloadId = PayloadId::new();
            let md5_hash = md5::compute(&data).to_vec();

            blob_storage
                .put_raw(
                    "oplog",
                    "upload_payload",
                    BlobStorageNamespace::OplogPayload {
                        environment_id: owned_agent_id.environment_id(),
                        agent_id: owned_agent_id.agent_id(),
                        agent_mode,
                    },
                    Path::new(&format!("{}/{}", hex::encode(&md5_hash), payload_id.0)),
                    &data,
                )
                .await
                .map_err(|e| format!("Failed uploading oplog data to the blob store {e}"))?;

            Ok(RawOplogPayload::External {
                payload_id,
                md5_hash,
            })
        } else {
            Ok(RawOplogPayload::SerializedInline(data))
        }
    }

    async fn download_raw_payload(
        blob_storage: Arc<dyn BlobStorage + Send + Sync>,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
        payload_id: PayloadId,
        md5_hash: Vec<u8>,
    ) -> Result<Vec<u8>, String> {
        blob_storage
                    .get_raw(
                        "oplog",
                        "download_payload",
                        BlobStorageNamespace::OplogPayload {
                            environment_id: owned_agent_id.environment_id(),
                            agent_id: owned_agent_id.agent_id(),
                            agent_mode,
                        },
                        Path::new(&format!("{}/{}", hex::encode(&md5_hash), payload_id.0)),
                    )
                    .await
                    .map_err(|e| format!("Failed downloading oplog data from the blob store {e}"))?
                    .ok_or(format!("Payload not found (worker: {owned_agent_id}, payload_id: {payload_id}, md5 hash: {md5_hash:02X?})"))
    }
}

#[async_trait]
impl OplogService for PrimaryOplogService {
    async fn create(
        &self,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
        initial_entry: OplogEntry,
        initial_worker_metadata: AgentMetadata,
        last_known_status: read_only_lock::arc_swap::ReadOnlyView<AgentStatusRecord>,
        execution_status: read_only_lock::std::ReadOnlyLock<ExecutionStatus>,
    ) -> Arc<dyn Oplog> {
        record_oplog_call("create");

        let key = Self::oplog_key(&owned_agent_id.agent_id);
        let already_exists: bool = {
            let is = self.indexed_storage.clone();
            let agent_id = owned_agent_id.agent_id();
            let key = key.clone();
            retry_storage_op(&self.retry_config, "create_exists", &key, || {
                let is = is.clone();
                let ns = IndexedStorageNamespace::OpLog {
                    agent_id: agent_id.clone(),
                    agent_mode,
                };
                let key = key.clone();
                async move { is.with("oplog", "create").exists(ns, &key).await }
            })
            .await
        };

        if already_exists {
            panic!("oplog for worker {owned_agent_id} already exists in indexed storage")
        }

        {
            let is = self.indexed_storage.clone();
            let agent_id = owned_agent_id.agent_id();
            let key = key.clone();
            retry_storage_op(&self.retry_config, "create_append", &key, || {
                let is = is.clone();
                let ns = IndexedStorageNamespace::OpLog {
                    agent_id: agent_id.clone(),
                    agent_mode,
                };
                let key = key.clone();
                let entry = initial_entry.clone();
                async move {
                    is.with_entity("oplog", "create", "entry")
                        .append(ns, &key, 1, &entry)
                        .await
                }
            })
            .await;
        }

        self.open(
            owned_agent_id,
            agent_mode,
            Some(OplogIndex::INITIAL),
            initial_worker_metadata,
            last_known_status,
            execution_status,
        )
        .await
    }

    async fn open(
        &self,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
        last_oplog_index: Option<OplogIndex>,
        initial_worker_metadata: AgentMetadata,
        _last_known_status: read_only_lock::arc_swap::ReadOnlyView<AgentStatusRecord>,
        _execution_status: read_only_lock::std::ReadOnlyLock<ExecutionStatus>,
    ) -> Arc<dyn Oplog> {
        record_oplog_call("open");

        let key = Self::oplog_key(&owned_agent_id.agent_id);

        self.oplogs
            .get_or_open(
                &owned_agent_id.agent_id,
                CreateOplogConstructor::new(
                    self.indexed_storage.clone(),
                    self.blob_storage.clone(),
                    self.replicas,
                    self.max_operations_before_commit,
                    self.max_operations_before_commit_in_persist_nothing,
                    self.max_payload_size,
                    self.retry_config.clone(),
                    key,
                    last_oplog_index,
                    owned_agent_id.clone(),
                    agent_mode,
                    initial_worker_metadata.created_by,
                ),
            )
            .await
    }

    async fn get_last_index(
        &self,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
    ) -> OplogIndex {
        record_oplog_call("get_last_index");
        Self::get_last_index_from_storage(
            &*self.indexed_storage,
            owned_agent_id,
            agent_mode,
            &self.retry_config,
        )
        .await
    }

    async fn delete(&self, owned_agent_id: &OwnedAgentId, agent_mode: AgentMode) {
        record_oplog_call("delete");

        {
            let is = self.indexed_storage.clone();
            let agent_id = owned_agent_id.agent_id();
            let key = Self::oplog_key(&owned_agent_id.agent_id);
            retry_storage_op(&self.retry_config, "delete", &key, || {
                let is = is.clone();
                let ns = IndexedStorageNamespace::OpLog {
                    agent_id: agent_id.clone(),
                    agent_mode,
                };
                let key = key.clone();
                async move { is.with("oplog", "delete").delete(ns, &key).await }
            })
            .await;
        }
    }

    async fn read(
        &self,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
        idx: OplogIndex,
        n: u64,
    ) -> BTreeMap<OplogIndex, OplogEntry> {
        record_oplog_call("read");

        {
            let is = self.indexed_storage.clone();
            let agent_id = owned_agent_id.agent_id();
            let key = Self::oplog_key(&owned_agent_id.agent_id);
            let start: u64 = idx.into();
            let end: u64 = idx.range_end(n).into();
            retry_storage_op(&self.retry_config, "read", &key, || {
                let is = is.clone();
                let ns = IndexedStorageNamespace::OpLog {
                    agent_id: agent_id.clone(),
                    agent_mode,
                };
                let key = key.clone();
                async move {
                    is.with_entity("oplog", "read", "entry")
                        .read(ns, &key, start, end)
                        .await
                }
            })
            .await
            .into_iter()
            .map(|(k, v): (u64, OplogEntry)| (OplogIndex::from_u64(k), v))
            .collect()
        }
    }

    async fn exists(&self, owned_agent_id: &OwnedAgentId, agent_mode: AgentMode) -> bool {
        record_oplog_call("exists");

        {
            let is = self.indexed_storage.clone();
            let agent_id = owned_agent_id.agent_id();
            let key = Self::oplog_key(&owned_agent_id.agent_id);
            retry_storage_op(&self.retry_config, "exists", &key, || {
                let is = is.clone();
                let ns = IndexedStorageNamespace::OpLog {
                    agent_id: agent_id.clone(),
                    agent_mode,
                };
                let key = key.clone();
                async move { is.with("oplog", "exists").exists(ns, &key).await }
            })
            .await
        }
    }

    async fn scan_for_component(
        &self,
        environment_id: &EnvironmentId,
        component_id: &ComponentId,
        modes: Option<AgentMode>,
        cursor: ScanCursor,
        count: u64,
    ) -> Result<(ScanCursor, Vec<OwnedAgentId>), WorkerExecutorError> {
        record_oplog_call("scan");

        let (active_mode, next_mode) = scan_modes(modes, cursor.cursor);
        let cursor_val = cursor_value(cursor.cursor);

        let (next_cursor_val, keys) = {
            let is = self.indexed_storage.clone();
            let prefix = Self::key_prefix(component_id);
            retry_storage_op(&self.retry_config, "scan", &prefix, || {
                let is = is.clone();
                let prefix = prefix.clone();
                async move {
                    is.with("oplog", "scan")
                        .scan(
                            IndexedStorageMetaNamespace::Oplog {
                                agent_mode: active_mode,
                            },
                            Some(&prefix),
                            cursor_val,
                            count,
                        )
                        .await
                }
            })
            .await
        };

        let next_cursor = next_scan_cursor(next_cursor_val, active_mode, next_mode, cursor.layer);
        let owned_agent_ids = keys
            .into_iter()
            .map(|key| OwnedAgentId {
                agent_id: Self::get_agent_id_from_key(&key, component_id),
                environment_id: *environment_id,
            })
            .collect();

        Ok((next_cursor, owned_agent_ids))
    }

    async fn upload_raw_payload(
        &self,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
        data: Vec<u8>,
    ) -> Result<RawOplogPayload, String> {
        Self::upload_raw_payload(
            self.blob_storage.clone(),
            self.max_payload_size,
            owned_agent_id,
            agent_mode,
            data,
        )
        .await
    }

    async fn download_raw_payload(
        &self,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
        payload_id: PayloadId,
        md5_hash: Vec<u8>,
    ) -> Result<Vec<u8>, String> {
        Self::download_raw_payload(
            self.blob_storage.clone(),
            owned_agent_id,
            agent_mode,
            payload_id,
            md5_hash,
        )
        .await
    }
}

#[derive(Clone)]
struct CreateOplogConstructor {
    indexed_storage: Arc<dyn IndexedStorage + Send + Sync>,
    blob_storage: Arc<dyn BlobStorage + Send + Sync>,
    replicas: u8,
    max_operations_before_commit: u64,
    max_operations_before_commit_in_persist_nothing: u64,
    max_payload_size: usize,
    retry_config: RetryConfig,
    key: String,
    last_oplog_idx: Option<OplogIndex>,
    owned_agent_id: OwnedAgentId,
    agent_mode: AgentMode,
    account_id: AccountId,
}

impl CreateOplogConstructor {
    #[allow(clippy::too_many_arguments)]
    fn new(
        indexed_storage: Arc<dyn IndexedStorage + Send + Sync>,
        blob_storage: Arc<dyn BlobStorage + Send + Sync>,
        replicas: u8,
        max_operations_before_commit: u64,
        max_operations_before_commit_in_persist_nothing: u64,
        max_payload_size: usize,
        retry_config: RetryConfig,
        key: String,
        last_oplog_idx: Option<OplogIndex>,
        owned_agent_id: OwnedAgentId,
        agent_mode: AgentMode,
        account_id: AccountId,
    ) -> Self {
        Self {
            indexed_storage,
            blob_storage,
            replicas,
            max_operations_before_commit,
            max_operations_before_commit_in_persist_nothing,
            max_payload_size,
            retry_config,
            key,
            last_oplog_idx,
            owned_agent_id,
            agent_mode,
            account_id,
        }
    }
}

#[async_trait]
impl OplogConstructor for CreateOplogConstructor {
    async fn create_oplog(self, close: Box<dyn FnOnce() + Send + Sync>) -> Arc<dyn Oplog> {
        let last_oplog_idx = match self.last_oplog_idx {
            Some(idx) => idx,
            None => {
                PrimaryOplogService::get_last_index_from_storage(
                    &*self.indexed_storage,
                    &self.owned_agent_id,
                    self.agent_mode,
                    &self.retry_config,
                )
                .await
            }
        };
        Arc::new(PrimaryOplog::new(
            self.indexed_storage,
            self.blob_storage,
            self.replicas,
            self.max_operations_before_commit,
            self.max_operations_before_commit_in_persist_nothing,
            self.max_payload_size,
            self.retry_config,
            self.key,
            last_oplog_idx,
            self.owned_agent_id,
            self.agent_mode,
            self.account_id,
            close,
        ))
    }
}

/// The primary oplog behind an actor boundary.
///
/// A dedicated tokio task (the actor) exclusively owns [`PrimaryOplogState`]; every `Oplog`
/// method sends an [`OplogJob`] over an unbounded channel and awaits the job's oneshot reply.
/// There is deliberately **no shared lock** between callers.
///
/// This shape is required for deadlock freedom, not just style. `Oplog` methods are awaited from
/// two kinds of callers that must never block each other through shared ownership:
///
/// * futures polled by wasmtime's store event loop (concurrent p3 durable host calls), and
/// * host code running on wasm fibers that suspend while *keeping the store* (async libcalls such
///   as the `memory.grow` resource limiter, and non-concurrent `wrap_async`-style host functions
///   like the p2 stdio streams).
///
/// While such a fiber is suspended, the event loop cannot poll any store-polled future
/// (wasmtime's documented store-blocking limitation, wasmtime#11869/#11870). With a shared lock —
/// even a lock whose critical sections are purely synchronous — a store-polled future that is
/// queued on the lock becomes its next owner on FIFO handoff and then cannot run until the event
/// loop polls it, so a fiber queued behind it deadlocks the whole store. With the actor, callers
/// only await oneshot completions, which never make an unpolled caller the owner of anything, so
/// fiber-side waits always make progress as long as the actor task (polled directly by tokio)
/// does.
///
/// The actor must therefore never await anything that is produced by a store event loop; it only
/// performs storage I/O.
///
/// ORDERING (Start determinism): `add_start_with_reserved_raw_payload` relies on jobs being
/// processed in the order they were enqueued. `mpsc::UnboundedSender::send` is synchronous, and
/// callers enqueue as their first non-awaiting step, so enqueue order equals the order in which
/// concurrent durable calls initiated their operation — the same guarantee the previous
/// FIFO-fair mutex provided via `lock()` acquisition order.
struct PrimaryOplog {
    jobs: tokio::sync::mpsc::UnboundedSender<OplogJob>,
    actor: tokio::task::JoinHandle<()>,
    key: String,
    close: Option<Box<dyn FnOnce() + Send + Sync>>,
}

/// A request processed by the [`PrimaryOplog`] actor task, which exclusively owns the oplog
/// state. Mutating jobs that grow the buffer past the commit threshold run the resulting commit
/// inside the actor before replying, preserving the pre-actor behavior where `add` blocked the
/// caller on a threshold-triggered commit.
enum OplogJob {
    Add {
        entry: OplogEntry,
        done: tokio::sync::oneshot::Sender<OplogIndex>,
    },
    AddPair {
        start: OplogEntry,
        make_second: Box<dyn FnOnce(OplogIndex) -> OplogEntry + Send>,
        done: tokio::sync::oneshot::Sender<(OplogIndex, OplogIndex)>,
    },
    AddStart {
        serialized_request: Vec<u8>,
        build_start: Box<dyn FnOnce(RawOplogPayload) -> Result<OplogEntry, String> + Send>,
        done: tokio::sync::oneshot::Sender<Result<OrderedOplogStart, String>>,
    },
    Commit {
        level: CommitLevel,
        done: tokio::sync::oneshot::Sender<BTreeMap<OplogIndex, OplogEntry>>,
    },
    DropPrefix {
        last_dropped_id: OplogIndex,
        done: tokio::sync::oneshot::Sender<u64>,
    },
    CurrentIndex {
        done: tokio::sync::oneshot::Sender<OplogIndex>,
    },
    LastAddedNonHintEntry {
        done: tokio::sync::oneshot::Sender<Option<OplogIndex>>,
    },
    /// Snapshots the state needed for reads and replica waits; the caller performs the storage
    /// I/O itself, off the actor, so large reads do not head-of-line block writes.
    Reader {
        done: tokio::sync::oneshot::Sender<OplogReader>,
    },
    /// Snapshots the state needed for payload blob uploads/downloads.
    BlobContext {
        done: tokio::sync::oneshot::Sender<OplogBlobContext>,
    },
    SwitchPersistenceLevel {
        level: PersistenceLevel,
        done: tokio::sync::oneshot::Sender<()>,
    },
}

/// Snapshot of the state needed to upload/download oplog payload blobs outside the actor.
struct OplogBlobContext {
    blob_storage: Arc<dyn BlobStorage + Send + Sync>,
    owned_agent_id: OwnedAgentId,
    agent_mode: AgentMode,
    account_id: AccountId,
    max_payload_size: usize,
}

impl Drop for PrimaryOplog {
    fn drop(&mut self) {
        // In-flight `Oplog` calls borrow `self`, so at this point no caller can be awaiting a
        // job reply anymore and aborting the actor cannot lose an observed operation.
        self.actor.abort();
        if let Some(close) = self.close.take() {
            close();
        }
    }
}

impl PrimaryOplog {
    #[allow(clippy::too_many_arguments)]
    fn new(
        indexed_storage: Arc<dyn IndexedStorage + Send + Sync>,
        blob_storage: Arc<dyn BlobStorage + Send + Sync>,
        replicas: u8,
        max_operations_before_commit: u64,
        max_operations_before_commit_in_persist_nothing: u64,
        max_payload_size: usize,
        retry_config: RetryConfig,
        key: String,
        last_oplog_idx: OplogIndex,
        owned_agent_id: OwnedAgentId,
        agent_mode: AgentMode,
        account_id: AccountId,
        close: Box<dyn FnOnce() + Send + Sync>,
    ) -> Self {
        let mut state = PrimaryOplogState {
            indexed_storage,
            blob_storage,
            replicas,
            max_operations_before_commit,
            max_operations_before_commit_in_persist_nothing,
            max_payload_size,
            retry_config,
            key: key.clone(),
            buffer: VecDeque::new(),
            last_committed_idx: last_oplog_idx,
            last_oplog_idx,
            owned_agent_id,
            agent_mode,
            account_id,
            last_added_non_hint_entry: None,
            persistence_level: PersistenceLevel::Smart,
            pending_uploads: Vec::new(),
        };

        let (jobs, mut job_rx) = tokio::sync::mpsc::unbounded_channel::<OplogJob>();
        let actor = tokio::spawn(async move {
            while let Some(job) = job_rx.recv().await {
                match job {
                    OplogJob::Add { entry, done } => {
                        record_oplog_call("add");
                        let idx = state.push(entry);
                        if state.over_commit_threshold() {
                            state.commit(CommitLevel::Always).await;
                        }
                        let _ = done.send(idx);
                    }
                    OplogJob::AddPair {
                        start,
                        make_second,
                        done,
                    } => {
                        record_oplog_call("add_pair");
                        let first_idx = state.push(start);
                        let second = make_second(first_idx);
                        let second_idx = state.push(second);
                        if state.over_commit_threshold() {
                            state.commit(CommitLevel::Always).await;
                        }
                        let _ = done.send((first_idx, second_idx));
                    }
                    OplogJob::AddStart {
                        serialized_request,
                        build_start,
                        done,
                    } => {
                        record_oplog_call("add_start_with_reserved_raw_payload");
                        // ORDERING (Start determinism) — CRITICAL SECTION: reserving the payload,
                        // building the `Start`, and assigning its index happen as one non-yielding
                        // step here on the actor, so a concurrently enqueued writer cannot
                        // interleave its own `Start` and reorder the deterministic replay
                        // sequence. The reservation only *starts* the (possibly large) blob
                        // upload; it is not awaited here. Durability of the blob before any
                        // referencing entry is committed is enforced by the commit barrier in
                        // `append`.
                        //
                        // The no-`.await` window is enforced at compile time by the `!Send`
                        // `guard`: this actor future must stay `Send` for `tokio::spawn`, so a
                        // refactor holding the guard across an `.await` is rejected rather than
                        // silently breaking ordering. Do not move `drop(guard)` before `push`.
                        let result = {
                            let ReservedPayload {
                                raw,
                                pending,
                                guard,
                            } = state.reserve_raw_payload(serialized_request);
                            match build_start(raw) {
                                Ok(entry) => {
                                    let index = state.push(entry.clone());
                                    drop(guard);
                                    Ok(OrderedOplogStart {
                                        index,
                                        entry,
                                        pending_upload: pending,
                                    })
                                }
                                Err(err) => Err(err),
                            }
                        };
                        if result.is_ok() && state.over_commit_threshold() {
                            state.commit(CommitLevel::Always).await;
                        }
                        let _ = done.send(result);
                    }
                    OplogJob::Commit { level, done } => {
                        let result = state.commit(level).await;
                        let _ = done.send(result);
                    }
                    OplogJob::DropPrefix {
                        last_dropped_id,
                        done,
                    } => {
                        let before = state.reader().length().await;
                        state.drop_prefix(last_dropped_id).await;
                        let remaining = state.reader().length().await;
                        if remaining == 0 {
                            state.delete().await;
                        }
                        let dropped = before - remaining;
                        if dropped > 0 {
                            let account_id = state.account_id.to_string();
                            let environment_id = state.owned_agent_id.environment_id().to_string();
                            record_storage_objects_deleted(
                                STORAGE_TYPE_OPLOG,
                                &account_id,
                                &environment_id,
                                dropped,
                            );
                        }
                        let _ = done.send(dropped);
                    }
                    OplogJob::CurrentIndex { done } => {
                        let _ = done.send(state.last_oplog_idx);
                    }
                    OplogJob::LastAddedNonHintEntry { done } => {
                        let _ = done.send(state.last_added_non_hint_entry);
                    }
                    OplogJob::Reader { done } => {
                        let _ = done.send(state.reader());
                    }
                    OplogJob::BlobContext { done } => {
                        let _ = done.send(OplogBlobContext {
                            blob_storage: state.blob_storage.clone(),
                            owned_agent_id: state.owned_agent_id.clone(),
                            agent_mode: state.agent_mode,
                            account_id: state.account_id,
                            max_payload_size: state.max_payload_size,
                        });
                    }
                    OplogJob::SwitchPersistenceLevel { level, done } => {
                        record_oplog_call("switch_persistence_level");
                        state.switch_persistence_level(level);
                        let _ = done.send(());
                    }
                }
            }
        });

        Self {
            jobs,
            actor,
            key,
            close: Some(close),
        }
    }

    /// Sends a job to the actor and waits for its reply.
    ///
    /// Panics if the actor task is gone: the actor is only aborted from `Drop` (when no caller
    /// can be in flight anymore), so a missing reply means the actor itself panicked and the
    /// oplog's state is no longer trustworthy.
    async fn run_job<R>(
        &self,
        make_job: impl FnOnce(tokio::sync::oneshot::Sender<R>) -> OplogJob,
    ) -> R {
        let (done, done_rx) = tokio::sync::oneshot::channel();
        if self.jobs.send(make_job(done)).is_err() {
            panic!("Oplog actor for {} terminated unexpectedly", self.key);
        }
        match done_rx.await {
            Ok(result) => result,
            Err(_) => panic!(
                "Oplog actor for {} dropped a request without replying",
                self.key
            ),
        }
    }
}

/// A snapshot of [`PrimaryOplogState`] sufficient to serve reads. Reads run on this snapshot
/// after the state lock has been released, so read I/O never holds the lock (see the
/// lock-discipline note on [`PrimaryOplog::state`]). The buffer snapshot keeps `read_many`'s
/// visibility of not-yet-committed entries.
struct OplogReader {
    indexed_storage: Arc<dyn IndexedStorage + Send + Sync>,
    retry_config: RetryConfig,
    key: String,
    owned_agent_id: OwnedAgentId,
    agent_mode: AgentMode,
    last_committed_idx: OplogIndex,
    buffer: VecDeque<OplogEntry>,
    replicas: u8,
}

impl OplogReader {
    async fn read(&self, oplog_index: OplogIndex) -> OplogEntry {
        record_oplog_call("read");

        let entries: Vec<(u64, OplogEntry)> = {
            let is = self.indexed_storage.clone();
            let agent_id = self.owned_agent_id.agent_id();
            let agent_mode = self.agent_mode;
            let key = self.key.clone();
            let idx: u64 = oplog_index.into();
            retry_storage_op(&self.retry_config, "read", &key, || {
                let is = is.clone();
                let ns = IndexedStorageNamespace::OpLog {
                    agent_id: agent_id.clone(),
                    agent_mode,
                };
                let key = key.clone();
                async move {
                    is.with_entity("oplog", "read", "entry")
                        .read(ns, &key, idx, idx)
                        .await
                }
            })
            .await
        };

        entries
            .into_iter()
            .next()
            .unwrap_or_else(|| {
                panic!(
                    "Missing oplog entry {oplog_index} for {} in indexed storage",
                    self.key
                )
            })
            .1
    }

    async fn read_many(&self, oplog_index: OplogIndex, n: u64) -> BTreeMap<OplogIndex, OplogEntry> {
        record_oplog_call("read_many");

        let last_idx = oplog_index.range_end(n);
        let mut result: BTreeMap<OplogIndex, OplogEntry> = {
            let is = self.indexed_storage.clone();
            let agent_id = self.owned_agent_id.agent_id();
            let agent_mode = self.agent_mode;
            let key = self.key.clone();
            let start: u64 = oplog_index.into();
            let end: u64 = last_idx.into();
            retry_storage_op(&self.retry_config, "read_many", &key, || {
                let is = is.clone();
                let ns = IndexedStorageNamespace::OpLog {
                    agent_id: agent_id.clone(),
                    agent_mode,
                };
                let key = key.clone();
                async move {
                    is.with_entity("oplog", "read", "entry")
                        .read(ns, &key, start, end)
                        .await
                }
            })
            .await
            .into_iter()
            .map(|(idx, entry)| (OplogIndex::from_u64(idx), entry))
            .collect()
        };

        if last_idx < self.last_committed_idx {
            // The whole range is already committed, no further action needed
            result
        } else {
            // There can be some uncommitted entries in the buffer
            let uncommitted_count = last_idx.distance_from(self.last_committed_idx);
            let buffered_to_take =
                min(max(0, uncommitted_count), self.buffer.len() as i64) as usize;

            let mut current = self.last_committed_idx;
            for idx in 0..buffered_to_take {
                current = current.next();
                let entry = self.buffer[idx].clone();
                result.insert(current, entry);
            }

            result
        }
    }

    async fn length(&self) -> u64 {
        record_oplog_call("length");

        {
            let is = self.indexed_storage.clone();
            let agent_id = self.owned_agent_id.agent_id();
            let agent_mode = self.agent_mode;
            let key = self.key.clone();
            retry_storage_op(&self.retry_config, "length", &key, || {
                let is = is.clone();
                let ns = IndexedStorageNamespace::OpLog {
                    agent_id: agent_id.clone(),
                    agent_mode,
                };
                let key = key.clone();
                async move { is.with("oplog", "length").length(ns, &key).await }
            })
            .await
        }
    }
}

struct PrimaryOplogState {
    indexed_storage: Arc<dyn IndexedStorage + Send + Sync>,
    blob_storage: Arc<dyn BlobStorage + Send + Sync>,
    replicas: u8,
    max_operations_before_commit: u64,
    max_operations_before_commit_in_persist_nothing: u64,
    max_payload_size: usize,
    retry_config: RetryConfig,
    key: String,
    buffer: VecDeque<OplogEntry>,
    last_oplog_idx: OplogIndex,
    last_committed_idx: OplogIndex,
    owned_agent_id: OwnedAgentId,
    agent_mode: AgentMode,
    account_id: AccountId,
    last_added_non_hint_entry: Option<OplogIndex>,
    persistence_level: PersistenceLevel,
    /// In-flight external payload uploads started by [`PrimaryOplogState::reserve_raw_payload`] but
    /// not yet known to be durable. The commit barrier in `append` waits on these before persisting
    /// any buffered entries, so no committed entry can reference a not-yet-written blob.
    pending_uploads: Vec<PendingUpload>,
}

impl PrimaryOplogState {
    /// Computes a payload reference and, for an external (large) payload, spawns its blob upload and
    /// registers it for the commit barrier — **without** awaiting the upload.
    ///
    /// Intentionally a non-`async` `fn`: computing the reference, spawning the upload, and
    /// registering the [`PendingUpload`] happen as one non-yielding step under the held state lock.
    /// The caller (`PrimaryOplog::add_start_with_reserved_raw_payload`) keeps the lock held and
    /// builds and `push`es the `Start` from this reference with no `.await` in between, so concurrent
    /// calls' `Start` entries stay in initiation order. That no-`.await` window is enforced at
    /// compile time by the returned `!Send` [`ReserveGuard`].
    fn reserve_raw_payload(&mut self, data: Vec<u8>) -> ReservedPayload {
        if data.len() > self.max_payload_size {
            let payload_id = PayloadId::new();
            let md5_hash = md5::compute(&data).to_vec();
            let path = format!("{}/{}", hex::encode(&md5_hash), payload_id.0);
            let data_len = data.len() as u64;

            let blob_storage = self.blob_storage.clone();
            let environment_id = self.owned_agent_id.environment_id();
            let agent_id = self.owned_agent_id.agent_id();
            let agent_mode = self.agent_mode;
            let account_id = self.account_id.to_string();
            let environment_id_label = environment_id.to_string();

            let upload = async move {
                blob_storage
                    .put_raw(
                        "oplog",
                        "upload_payload",
                        BlobStorageNamespace::OplogPayload {
                            environment_id,
                            agent_id,
                            agent_mode,
                        },
                        Path::new(&path),
                        &data,
                    )
                    .await
                    .map_err(|e| format!("Failed uploading oplog data to the blob store {e}"))?;
                record_storage_bytes_written(
                    STORAGE_TYPE_OPLOG,
                    &account_id,
                    &environment_id_label,
                    data_len,
                );
                Ok::<(), String>(())
            };

            let upload = tokio::spawn(upload)
                .map(|joined| {
                    joined.unwrap_or_else(|join_err| {
                        Err(format!("oplog payload upload task failed: {join_err}"))
                    })
                })
                .boxed()
                .shared();
            let pending = PendingUpload::spawned(upload);
            self.pending_uploads.push(pending.clone());
            ReservedPayload::new(
                RawOplogPayload::External {
                    payload_id,
                    md5_hash,
                },
                pending,
            )
        } else {
            ReservedPayload::new(
                RawOplogPayload::SerializedInline(data),
                PendingUpload::already_durable(),
            )
        }
    }

    async fn append(&mut self, entries: Vec<OplogEntry>) -> BTreeMap<OplogIndex, OplogEntry> {
        record_oplog_call("append");

        // Commit barrier: every deferred external payload reserved during this session must be
        // durably written to blob storage before the entries (which may reference it) are persisted
        // to indexed storage. `append` flushes the whole buffer, so waiting on all outstanding
        // uploads is correct. A permanent upload failure is treated like a permanent storage
        // failure (see `retry_storage_op`): there is no safe way to commit a dangling reference.
        if !self.pending_uploads.is_empty() {
            let pending = std::mem::take(&mut self.pending_uploads);
            for upload in pending {
                if let Err(err) = upload.wait().await {
                    panic!(
                        "Oplog payload upload failed for key '{}', cannot commit referencing entries: {err}",
                        self.key
                    );
                }
            }
        }

        let entry_count = entries.len() as u64;
        let mut pairs = Vec::with_capacity(entries.len());
        let mut last_idx = self.last_committed_idx;
        for entry in entries {
            let oplog_idx = last_idx.next();
            pairs.push((oplog_idx.into(), entry));
            last_idx = oplog_idx;
        }
        let pairs_ref: Vec<(u64, &OplogEntry)> = pairs.iter().map(|(id, e)| (*id, e)).collect();
        let bytes_written = {
            let is = self.indexed_storage.clone();
            let agent_id = self.owned_agent_id.agent_id();
            let agent_mode = self.agent_mode;
            let key = self.key.clone();
            retry_storage_op(&self.retry_config, "append", &key, || {
                let is = is.clone();
                let ns = IndexedStorageNamespace::OpLog {
                    agent_id: agent_id.clone(),
                    agent_mode,
                };
                let key = key.clone();
                let pairs_ref = &pairs_ref;
                async move {
                    is.with_entity("oplog", "append", "entry")
                        .append_many(ns, &key, pairs_ref)
                        .await
                }
            })
            .await
        };

        if entry_count > 0 {
            let account_id = self.account_id.to_string();
            let environment_id = self.owned_agent_id.environment_id().to_string();
            record_storage_bytes_written(
                STORAGE_TYPE_OPLOG,
                &account_id,
                &environment_id,
                bytes_written,
            );
            record_storage_objects_written(
                STORAGE_TYPE_OPLOG,
                &account_id,
                &environment_id,
                entry_count,
            );
        }
        drop(pairs_ref);

        self.last_committed_idx = last_idx;
        BTreeMap::from_iter(
            pairs
                .into_iter()
                .map(|(idx, entry)| (OplogIndex::from_u64(idx), entry)),
        )
    }

    /// Pushes an entry into the in-memory buffer and advances the oplog index,
    /// without checking the commit threshold. Callers must run [`maybe_commit`]
    /// afterwards. Used by `add_pair` to buffer a `Start`/`End` pair before a
    /// single commit-threshold check, so the pair is never split by a commit.
    fn push(&mut self, entry: OplogEntry) -> OplogIndex {
        let is_hint = entry.is_hint();
        self.buffer.push_back(entry);
        self.last_oplog_idx = self.last_oplog_idx.next();
        if !is_hint {
            self.last_added_non_hint_entry = Some(self.last_oplog_idx);
        }
        self.last_oplog_idx
    }

    /// Snapshots everything needed to serve reads without holding the state lock across storage
    /// I/O (see the lock-discipline note on [`PrimaryOplog::state`]).
    fn reader(&self) -> OplogReader {
        OplogReader {
            indexed_storage: self.indexed_storage.clone(),
            retry_config: self.retry_config.clone(),
            key: self.key.clone(),
            owned_agent_id: self.owned_agent_id.clone(),
            agent_mode: self.agent_mode,
            last_committed_idx: self.last_committed_idx,
            buffer: self.buffer.clone(),
            replicas: self.replicas,
        }
    }

    /// Whether the buffer has grown past the commit threshold and a commit should be scheduled.
    /// The commit itself always runs on the committer task; see the lock-discipline note on
    /// [`PrimaryOplog::state`].
    fn over_commit_threshold(&self) -> bool {
        let limit = match &self.persistence_level {
            PersistenceLevel::PersistNothing => {
                self.max_operations_before_commit_in_persist_nothing
            }
            PersistenceLevel::PersistRemoteSideEffects | PersistenceLevel::Smart => {
                self.max_operations_before_commit
            }
        };
        self.buffer.len() > limit as usize
    }

    async fn commit(&mut self, level: CommitLevel) -> BTreeMap<OplogIndex, OplogEntry> {
        record_oplog_call("commit");

        if level == CommitLevel::Always
            || self.persistence_level != PersistenceLevel::PersistNothing
        {
            let entries = self.buffer.drain(..).collect::<Vec<OplogEntry>>();
            self.append(entries).await
        } else {
            BTreeMap::new()
        }
    }

    async fn drop_prefix(&self, last_dropped_id: OplogIndex) {
        record_oplog_call("drop_prefix");

        {
            let is = self.indexed_storage.clone();
            let agent_id = self.owned_agent_id.agent_id();
            let agent_mode = self.agent_mode;
            let key = self.key.clone();
            let dropped_id: u64 = last_dropped_id.into();
            retry_storage_op(&self.retry_config, "drop_prefix", &key, || {
                let is = is.clone();
                let ns = IndexedStorageNamespace::OpLog {
                    agent_id: agent_id.clone(),
                    agent_mode,
                };
                let key = key.clone();
                async move {
                    is.with("oplog", "drop_prefix")
                        .drop_prefix(ns, &key, dropped_id)
                        .await
                }
            })
            .await;
        }
    }

    async fn delete(&self) {
        record_oplog_call("delete");

        {
            let is = self.indexed_storage.clone();
            let agent_id = self.owned_agent_id.agent_id();
            let agent_mode = self.agent_mode;
            let key = self.key.clone();
            retry_storage_op(&self.retry_config, "delete", &key, || {
                let is = is.clone();
                let ns = IndexedStorageNamespace::OpLog {
                    agent_id: agent_id.clone(),
                    agent_mode,
                };
                let key = key.clone();
                async move { is.with("oplog", "delete").delete(ns, &key).await }
            })
            .await;
        }
    }

    fn switch_persistence_level(&mut self, level: PersistenceLevel) {
        self.persistence_level = level;
    }
}

impl Debug for PrimaryOplog {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.key)
    }
}

#[async_trait]
impl Oplog for PrimaryOplog {
    async fn add(&self, entry: OplogEntry) -> OplogIndex {
        self.run_job(|done| OplogJob::Add { entry, done }).await
    }

    async fn add_pair(
        &self,
        start: OplogEntry,
        make_second: Box<dyn FnOnce(OplogIndex) -> OplogEntry + Send>,
    ) -> (OplogIndex, OplogIndex) {
        self.run_job(|done| OplogJob::AddPair {
            start,
            make_second,
            done,
        })
        .await
    }

    async fn drop_prefix(&self, last_dropped_id: OplogIndex) -> u64 {
        self.run_job(|done| OplogJob::DropPrefix {
            last_dropped_id,
            done,
        })
        .await
    }

    async fn commit(&self, level: CommitLevel) -> BTreeMap<OplogIndex, OplogEntry> {
        self.run_job(|done| OplogJob::Commit { level, done }).await
    }

    async fn current_oplog_index(&self) -> OplogIndex {
        self.run_job(|done| OplogJob::CurrentIndex { done }).await
    }

    async fn last_added_non_hint_entry(&self) -> Option<OplogIndex> {
        self.run_job(|done| OplogJob::LastAddedNonHintEntry { done })
            .await
    }

    async fn wait_for_replicas(&self, replicas: u8, timeout: Duration) -> bool {
        record_oplog_call("wait_for_replicas");

        self.run_job(|done| OplogJob::Commit {
            level: CommitLevel::Always,
            done,
        })
        .await;
        let reader = self.run_job(|done| OplogJob::Reader { done }).await;
        let replicas = replicas.min(reader.replicas);
        match reader
            .indexed_storage
            .with("oplog", "wait_for_replicas")
            .wait_for_replicas(replicas, timeout)
            .await
        {
            Ok(n) => n == replicas,
            Err(err) => {
                error!("Failed to wait for replicas to sync indexed storage: {err}");
                false
            }
        }
    }

    async fn read(&self, oplog_index: OplogIndex) -> OplogEntry {
        let reader = self.run_job(|done| OplogJob::Reader { done }).await;
        reader.read(oplog_index).await
    }

    async fn read_many(&self, oplog_index: OplogIndex, n: u64) -> BTreeMap<OplogIndex, OplogEntry> {
        let reader = self.run_job(|done| OplogJob::Reader { done }).await;
        reader.read_many(oplog_index, n).await
    }

    async fn length(&self) -> u64 {
        let reader = self.run_job(|done| OplogJob::Reader { done }).await;
        reader.length().await
    }

    async fn upload_raw_payload(&self, data: Vec<u8>) -> Result<RawOplogPayload, String> {
        let ctx = self.run_job(|done| OplogJob::BlobContext { done }).await;
        let data_len = data.len() as u64;
        let result = PrimaryOplogService::upload_raw_payload(
            ctx.blob_storage,
            ctx.max_payload_size,
            &ctx.owned_agent_id,
            ctx.agent_mode,
            data,
        )
        .await;
        if let Ok(RawOplogPayload::External { .. }) = &result {
            // Only count bytes that were actually uploaded externally
            record_storage_bytes_written(
                STORAGE_TYPE_OPLOG,
                &ctx.account_id.to_string(),
                &ctx.owned_agent_id.environment_id().to_string(),
                data_len,
            );
        }
        result
    }

    async fn download_raw_payload(
        &self,
        payload_id: PayloadId,
        md5_hash: Vec<u8>,
    ) -> Result<Vec<u8>, String> {
        let ctx = self.run_job(|done| OplogJob::BlobContext { done }).await;
        PrimaryOplogService::download_raw_payload(
            ctx.blob_storage,
            &ctx.owned_agent_id,
            ctx.agent_mode,
            payload_id,
            md5_hash,
        )
        .await
    }

    async fn add_start_with_reserved_raw_payload(
        &self,
        serialized_request: Vec<u8>,
        build_start: Box<dyn FnOnce(RawOplogPayload) -> Result<OplogEntry, String> + Send>,
    ) -> Result<OrderedOplogStart, String> {
        // ORDERING (Start determinism): the job is enqueued synchronously here — there is no
        // `.await` between a subtask initiating its durable operation and this send — and the
        // actor assigns `Start` indices strictly in job order, so initiation order becomes
        // `Start`-index order exactly as the ordering contract requires. See the ordering note on
        // [`PrimaryOplog`].
        self.run_job(|done| OplogJob::AddStart {
            serialized_request,
            build_start,
            done,
        })
        .await
    }

    async fn switch_persistence_level(&self, mode: PersistenceLevel) {
        self.run_job(|done| OplogJob::SwitchPersistenceLevel { level: mode, done })
            .await
    }
}
