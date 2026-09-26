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

use crate::metrics::storage::{
    STORAGE_TYPE_OPLOG_ARCHIVE, record_storage_bytes_written, record_storage_objects_deleted,
    record_storage_objects_written,
};
use crate::model::ExecutionStatus;
use crate::services::oplog::ephemeral::EphemeralOplog;
use crate::services::oplog::multilayer::BackgroundTransferMessage::{
    TransferFromLower, TransferFromPrimary,
};
use crate::services::oplog::reader::{OplogRead, OplogReadError, OplogReadSource, fail_stop};
use crate::services::oplog::{
    CommitLevel, DurableStreamBatchBuilder, IndexedReservedStartBuilder, OpenOplogs, Oplog,
    OplogAddReceipt, OplogCloseCompletion, OplogConstructor, OplogLifecycleGuard, OplogService,
    OrderedOplogStart, ReservedRawStartBuilder, decode_scan_cursor, downcast_oplog,
    first_scan_cursor,
};
use crate::storage::indexed::IndexedStorageMetaNamespace;
use async_trait::async_trait;
use futures::FutureExt;
use golem_common::model::account::AccountId;
use golem_common::model::agent::AgentMode;
use golem_common::model::component::ComponentId;
use golem_common::model::environment::EnvironmentId;
use golem_common::model::oplog::{
    AtomicOplogIndex, OplogEntry, OplogIndex, PayloadId, RawOplogPayload,
};
use golem_common::model::{AgentId, AgentMetadata, AgentStatusRecord, OwnedAgentId, ScanCursor};
use golem_common::read_only_lock;
use golem_common::related_span;
use golem_common::tracing::TraceOrigin;
use golem_service_base::error::worker_executor::WorkerExecutorError;
use nonempty_collections::NEVec;
use std::cmp::min;
use std::collections::{BTreeMap, HashMap};
use std::fmt::{Debug, Formatter};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tokio::sync::oneshot::Sender;
use tracing::{Instrument, Level, debug, info, warn};

pub(crate) type TransferFiber = Arc<Mutex<TransferFiberState>>;
type TransferFibers = Arc<Mutex<HashMap<AgentId, Weak<Mutex<TransferFiberState>>>>>;

pub(crate) struct TransferFiberState {
    transfer_fiber: Option<tokio::task::AbortHandle>,
    closed: Option<OplogCloseCompletion>,
    cancelled: bool,
}

pub(crate) fn new_transfer_fiber() -> TransferFiber {
    Arc::new(Mutex::new(TransferFiberState {
        transfer_fiber: None,
        closed: None,
        cancelled: false,
    }))
}

#[async_trait]
pub trait OplogArchiveService: Debug + Send + Sync {
    /// Opens an oplog archive for reading and writing
    async fn open(
        &self,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
    ) -> Arc<dyn OplogArchive + Send + Sync>;

    /// Opens a new, known-empty archive without probing persistent storage.
    async fn open_fresh(
        &self,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
    ) -> Arc<dyn OplogArchive + Send + Sync>;

    /// Deletes the oplog archive for a worker completely
    async fn delete(&self, owned_agent_id: &OwnedAgentId, agent_mode: AgentMode);

    /// Reads the entries physically present in this archive within the requested range.
    async fn read_source(
        &self,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
        idx: OplogIndex,
        n: u64,
    ) -> BTreeMap<OplogIndex, OplogEntry>;

    /// Checks if an oplog archive exists for a worker
    async fn exists(&self, owned_agent_id: &OwnedAgentId, agent_mode: AgentMode) -> bool;

    /// Scans the oplog archive for all workers belonging to the given component, in a paginated way.
    ///
    /// `modes` selects which agent modes to scan. `Some(mode)` scans only that mode;
    /// `None` scans both modes (durable and ephemeral). When scanning multiple modes the
    /// active mode is encoded into the returned `ScanCursor` so the caller resumes the same
    /// pagination correctly.
    async fn scan_for_component(
        &self,
        environment_id: &EnvironmentId,
        component_id: &ComponentId,
        modes: Option<AgentMode>,
        cursor: ScanCursor,
        count: u64,
    ) -> Result<(ScanCursor, Vec<OwnedAgentId>), WorkerExecutorError>;

    /// Gets the last stored oplog entry's id in the archive
    async fn get_last_index(
        &self,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
    ) -> OplogIndex;

    /// The meta-namespace whose keys list every agent this archive holds entries for, or `None`
    /// when the storage cannot list them, as for blob-backed archives.
    fn scan_namespace(&self, _agent_mode: AgentMode) -> Option<IndexedStorageMetaNamespace> {
        None
    }
}

/// Interface for secondary oplog archives - requires less functionality than the primary archive
#[async_trait]
pub trait OplogArchive: Debug {
    /// Reads the entries physically present in this archive within the requested range.
    async fn read_source(&self, idx: OplogIndex, n: u64) -> BTreeMap<OplogIndex, OplogEntry>;

    /// Append a new chunk of entries to the oplog.
    /// Returns the number of compressed bytes written to storage.
    async fn append(&self, chunk: &[(OplogIndex, OplogEntry)]) -> u64;

    /// Verifies that transferred entries can be read from persistent storage without consulting
    /// this archive handle's cache.
    async fn verify_persisted(&self, entries: &[(OplogIndex, OplogEntry)]);

    /// Gets the last appended chunk's last index
    async fn current_oplog_index(&self) -> OplogIndex;

    /// Drop a chunk of entries from the beginning of the oplog
    ///
    /// This should only be called _after_ `append` succeeded in the archive below this one
    async fn drop_prefix(&self, last_dropped_id: OplogIndex) -> u64;

    /// Gets the total number of entries in this oplog archive
    async fn length(&self) -> u64;

    /// Gets the last index in this oplog archive
    async fn get_last_index(&self) -> OplogIndex;
}

/// Wraps an `OplogArchive` to record storage metrics on writes.
#[derive(Debug)]
pub struct InstrumentedOplogArchive {
    inner: Arc<dyn OplogArchive + Send + Sync>,
    account_id: AccountId,
    environment_id: EnvironmentId,
}

impl InstrumentedOplogArchive {
    pub fn new(
        inner: Arc<dyn OplogArchive + Send + Sync>,
        account_id: AccountId,
        environment_id: EnvironmentId,
    ) -> Self {
        Self {
            inner,
            account_id,
            environment_id,
        }
    }
}

#[async_trait]
impl OplogArchive for InstrumentedOplogArchive {
    async fn read_source(&self, idx: OplogIndex, n: u64) -> BTreeMap<OplogIndex, OplogEntry> {
        self.inner.read_source(idx, n).await
    }

    async fn append(&self, chunk: &[(OplogIndex, OplogEntry)]) -> u64 {
        if chunk.is_empty() {
            return 0;
        }
        let entry_count = chunk.len() as u64;
        let account_id = self.account_id.to_string();
        let environment_id = self.environment_id.to_string();

        let bytes = self.inner.append(chunk).await;

        record_storage_bytes_written(
            STORAGE_TYPE_OPLOG_ARCHIVE,
            &account_id,
            &environment_id,
            bytes,
        );
        record_storage_objects_written(
            STORAGE_TYPE_OPLOG_ARCHIVE,
            &account_id,
            &environment_id,
            entry_count,
        );

        bytes
    }

    async fn verify_persisted(&self, entries: &[(OplogIndex, OplogEntry)]) {
        self.inner.verify_persisted(entries).await
    }

    async fn current_oplog_index(&self) -> OplogIndex {
        self.inner.current_oplog_index().await
    }

    async fn drop_prefix(&self, last_dropped_id: OplogIndex) -> u64 {
        let dropped = self.inner.drop_prefix(last_dropped_id).await;
        if dropped > 0 {
            let account_id = self.account_id.to_string();
            let environment_id = self.environment_id.to_string();
            record_storage_objects_deleted(
                STORAGE_TYPE_OPLOG_ARCHIVE,
                &account_id,
                &environment_id,
                dropped,
            );
        }
        dropped
    }

    async fn length(&self) -> u64 {
        self.inner.length().await
    }

    async fn get_last_index(&self) -> OplogIndex {
        self.inner.get_last_index().await
    }
}

#[derive(Debug)]
pub struct MultiLayerOplogService {
    pub primary: Arc<dyn OplogService>,
    pub lower: NEVec<Arc<dyn OplogArchiveService>>,

    oplogs: OpenOplogs,
    transfer_fibers: TransferFibers,

    entry_count_limit: u64,
    max_operations_before_commit_ephemeral: u64,
}

impl MultiLayerOplogService {
    pub fn new(
        primary: Arc<dyn OplogService>,
        lower: NEVec<Arc<dyn OplogArchiveService>>,
        entry_count_limit: u64,
        max_operations_before_commit_ephemeral: u64,
    ) -> Self {
        Self {
            primary,
            lower,
            oplogs: OpenOplogs::new("multi-layer oplog"),
            transfer_fibers: Arc::new(Mutex::new(HashMap::new())),
            entry_count_limit,
            max_operations_before_commit_ephemeral,
        }
    }

    async fn filter_ids_existing_on_lower_layers(
        &self,
        unfiltered_ids: Vec<OwnedAgentId>,
        agent_mode: AgentMode,
        from: usize,
    ) -> Result<Vec<OwnedAgentId>, WorkerExecutorError> {
        let mut ids = Vec::new();
        for id in unfiltered_ids {
            let mut exists_in_lower = false;
            for lower_layer in &self.lower.iter().as_slice()[from..] {
                if lower_layer.exists(&id, agent_mode).await {
                    exists_in_lower = true;
                    break;
                }
            }

            if !exists_in_lower {
                ids.push(id);
            }
        }
        Ok(ids)
    }

    pub(crate) fn register_transfer(&self, agent_id: AgentId, transfer_fiber: &TransferFiber) {
        self.transfer_fibers
            .lock()
            .unwrap()
            .insert(agent_id, Arc::downgrade(transfer_fiber));
    }

    async fn abort_transfer(&self, agent_id: &AgentId) {
        let transfer_fiber = self
            .transfer_fibers
            .lock()
            .unwrap()
            .remove(agent_id)
            .and_then(|transfer_fiber| transfer_fiber.upgrade());

        if let Some(transfer_fiber) = transfer_fiber {
            Self::cancel_transfer(&transfer_fiber)
                .await
                .expect("Oplog transfer cleanup failed");
        }
    }

    pub(crate) async fn cancel_transfer(transfer_fiber: &TransferFiber) -> Result<(), String> {
        let closed = {
            let mut state = transfer_fiber.lock().unwrap();
            state.cancelled = true;
            if let Some(transfer) = &state.transfer_fiber {
                transfer.abort();
            }
            state.closed.clone()
        };
        match closed {
            Some(closed) => closed.await,
            None => Ok(()),
        }
    }

    pub(crate) fn transfer_closed(transfer_fiber: &TransferFiber) -> OplogCloseCompletion {
        transfer_fiber
            .lock()
            .unwrap()
            .closed
            .clone()
            .expect("transfer must be constructed")
    }

    pub(crate) async fn start_transfer(
        transfer_fiber: &TransferFiber,
        start: Sender<()>,
        transfer: tokio::task::JoinHandle<()>,
    ) {
        let abort = transfer.abort_handle();
        let closed = async move {
            match transfer.await {
                Ok(()) => Ok(()),
                Err(error) if error.is_cancelled() => Ok(()),
                Err(error) => Err(error.to_string()),
            }
        }
        .boxed()
        .shared();
        let cancelled = {
            let mut transfer_fiber = transfer_fiber.lock().unwrap();
            transfer_fiber.transfer_fiber = Some(abort.clone());
            transfer_fiber.closed = Some(closed.clone());
            transfer_fiber.cancelled
        };

        if cancelled {
            abort.abort();
            closed.await.expect("Oplog transfer cleanup failed");
        } else {
            let _ = start.send(());
        }
    }

    pub(crate) fn abort_transfer_in_drop(&self, transfer_fiber: &TransferFiber) {
        let transfer = {
            let mut transfer_fiber = transfer_fiber.lock().unwrap();
            transfer_fiber.cancelled = true;
            transfer_fiber.transfer_fiber.take()
        };

        if let Some(transfer) = transfer {
            transfer.abort();
        }
    }

    pub(crate) fn unregister_transfer(&self, agent_id: &AgentId, transfer_fiber: &TransferFiber) {
        let transfer_fiber = Arc::downgrade(transfer_fiber);
        let mut transfer_fibers = self.transfer_fibers.lock().unwrap();
        if transfer_fibers
            .get(agent_id)
            .is_some_and(|registered| registered.ptr_eq(&transfer_fiber))
        {
            transfer_fibers.remove(agent_id);
        }
    }
}

impl Clone for MultiLayerOplogService {
    fn clone(&self) -> Self {
        Self {
            primary: self.primary.clone(),
            lower: self.lower.clone(),
            oplogs: self.oplogs.clone(),
            transfer_fibers: self.transfer_fibers.clone(),
            entry_count_limit: self.entry_count_limit,
            max_operations_before_commit_ephemeral: self.max_operations_before_commit_ephemeral,
        }
    }
}

#[derive(Clone)]
struct CreateOplogConstructor {
    owned_agent_id: OwnedAgentId,
    agent_mode: AgentMode,
    initial_entry: Option<OplogEntry>,
    primary: Arc<dyn OplogService>,
    service: MultiLayerOplogService,
    last_oplog_index: Option<OplogIndex>,
    fresh: bool,
    initial_worker_metadata: AgentMetadata,
    last_known_status: read_only_lock::arc_swap::ReadOnlyView<AgentStatusRecord>,
    execution_status: read_only_lock::std::ReadOnlyLock<ExecutionStatus>,
}

impl CreateOplogConstructor {
    #[allow(clippy::too_many_arguments)]
    fn new(
        owned_agent_id: OwnedAgentId,
        agent_mode: AgentMode,
        initial_entry: Option<OplogEntry>,
        primary: Arc<dyn OplogService>,
        service: MultiLayerOplogService,
        last_oplog_index: Option<OplogIndex>,
        fresh: bool,
        initial_worker_metadata: AgentMetadata,
        last_known_status: read_only_lock::arc_swap::ReadOnlyView<AgentStatusRecord>,
        execution_status: read_only_lock::std::ReadOnlyLock<ExecutionStatus>,
    ) -> Self {
        Self {
            owned_agent_id,
            agent_mode,
            initial_entry,
            primary,
            service,
            last_oplog_index,
            fresh,
            initial_worker_metadata,
            last_known_status,
            execution_status,
        }
    }
}

#[async_trait]
impl OplogConstructor for CreateOplogConstructor {
    async fn create_oplog(
        self,
        lifecycle: &mut OplogLifecycleGuard,
        close: Box<dyn FnOnce() + Send + Sync>,
    ) -> Arc<dyn Oplog> {
        let agent_mode = self.agent_mode;
        let last_oplog_index = match self.last_oplog_index {
            Some(idx) => idx,
            None => {
                self.service
                    .get_last_index(&self.owned_agent_id, agent_mode)
                    .await
            }
        };

        let account_id = self.initial_worker_metadata.created_by;
        let fingerprint = self.initial_worker_metadata.fingerprint;

        match agent_mode {
            AgentMode::Durable => {
                let primary = if let Some(initial_entry) = self.initial_entry {
                    if self.fresh {
                        self.primary
                            .create_fresh(
                                lifecycle,
                                &self.owned_agent_id,
                                agent_mode,
                                initial_entry,
                                self.initial_worker_metadata,
                                self.last_known_status,
                                self.execution_status,
                            )
                            .await
                    } else {
                        self.primary
                            .create(
                                lifecycle,
                                &self.owned_agent_id,
                                agent_mode,
                                initial_entry,
                                self.initial_worker_metadata,
                                self.last_known_status,
                                self.execution_status,
                            )
                            .await
                    }
                } else {
                    self.primary
                        .open(
                            lifecycle,
                            &self.owned_agent_id,
                            agent_mode,
                            Some(last_oplog_index),
                            self.initial_worker_metadata,
                            self.last_known_status,
                            self.execution_status,
                        )
                        .await
                };
                MultiLayerOplog::new(
                    self.owned_agent_id,
                    agent_mode,
                    account_id,
                    primary,
                    self.service,
                    close,
                )
                .await
            }
            AgentMode::Ephemeral => {
                let (tx, rx) = tokio::sync::mpsc::unbounded_channel();

                let lower = EphemeralOplog::build_lower_layers(
                    &self.service.lower,
                    &self.owned_agent_id,
                    agent_mode,
                    account_id,
                    self.service.entry_count_limit,
                    &tx,
                    self.fresh,
                )
                .await;

                if let Some(initial_entry) = self.initial_entry {
                    lower
                        .first()
                        .append(&[(OplogIndex::INITIAL, initial_entry)])
                        .await;
                }

                let transfer_fiber = new_transfer_fiber();
                self.service
                    .register_transfer(self.owned_agent_id.agent_id.clone(), &transfer_fiber);
                let (start_tx, start_rx) = tokio::sync::oneshot::channel();
                let transfer = EphemeralOplog::spawn_background_transfer(
                    self.owned_agent_id.clone(),
                    lower.clone(),
                    rx,
                    start_rx,
                );
                MultiLayerOplogService::start_transfer(&transfer_fiber, start_tx, transfer).await;

                Arc::new(
                    EphemeralOplog::new(
                        self.owned_agent_id,
                        agent_mode,
                        fingerprint,
                        last_oplog_index,
                        self.service.max_operations_before_commit_ephemeral,
                        self.primary.clone(),
                        lower,
                        tx,
                        transfer_fiber,
                        self.service,
                        close,
                    )
                    .await,
                )
            }
        }
    }
}

#[async_trait]
impl OplogService for MultiLayerOplogService {
    async fn lock_lifecycle(&self, agent_id: &AgentId) -> OplogLifecycleGuard {
        self.primary.lock_lifecycle(agent_id).await
    }

    fn set_stream_session_index(&self, index: Arc<super::StreamSessionIndexService>) {
        self.primary.set_stream_session_index(index);
    }

    fn stream_session_index(&self) -> Option<Arc<super::StreamSessionIndexService>> {
        self.primary.stream_session_index()
    }

    async fn create_staged(
        &self,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
        stage_id: uuid::Uuid,
        initial_worker_metadata: AgentMetadata,
    ) -> Result<Arc<dyn Oplog>, String> {
        self.primary
            .create_staged(
                owned_agent_id,
                agent_mode,
                stage_id,
                initial_worker_metadata,
            )
            .await
    }

    async fn staged_exists(
        &self,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
        stage_id: uuid::Uuid,
    ) -> Result<bool, String> {
        self.primary
            .staged_exists(owned_agent_id, agent_mode, stage_id)
            .await
    }

    async fn publish_staged(
        &self,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
        stage_id: uuid::Uuid,
        expected_last_index: OplogIndex,
    ) -> Result<bool, String> {
        self.primary
            .publish_staged(owned_agent_id, agent_mode, stage_id, expected_last_index)
            .await
    }

    async fn discard_staged(
        &self,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
        stage_id: uuid::Uuid,
    ) -> Result<(), String> {
        self.primary
            .discard_staged(owned_agent_id, agent_mode, stage_id)
            .await
    }

    async fn create(
        &self,
        lifecycle: &mut OplogLifecycleGuard,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
        initial_entry: OplogEntry,
        initial_worker_metadata: AgentMetadata,
        last_known_status: read_only_lock::arc_swap::ReadOnlyView<AgentStatusRecord>,
        execution_status: read_only_lock::std::ReadOnlyLock<ExecutionStatus>,
    ) -> Arc<dyn Oplog> {
        self.oplogs
            .get_or_open(
                lifecycle,
                &owned_agent_id.agent_id,
                CreateOplogConstructor::new(
                    owned_agent_id.clone(),
                    agent_mode,
                    Some(initial_entry),
                    self.primary.clone(),
                    self.clone(),
                    Some(OplogIndex::INITIAL),
                    false,
                    initial_worker_metadata,
                    last_known_status,
                    execution_status,
                ),
            )
            .await
    }

    async fn create_fresh(
        &self,
        lifecycle: &mut OplogLifecycleGuard,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
        initial_entry: OplogEntry,
        initial_worker_metadata: AgentMetadata,
        last_known_status: read_only_lock::arc_swap::ReadOnlyView<AgentStatusRecord>,
        execution_status: read_only_lock::std::ReadOnlyLock<ExecutionStatus>,
    ) -> Arc<dyn Oplog> {
        self.oplogs
            .get_or_open(
                lifecycle,
                &owned_agent_id.agent_id,
                CreateOplogConstructor::new(
                    owned_agent_id.clone(),
                    agent_mode,
                    Some(initial_entry),
                    self.primary.clone(),
                    self.clone(),
                    Some(OplogIndex::INITIAL),
                    true,
                    initial_worker_metadata,
                    last_known_status,
                    execution_status,
                ),
            )
            .await
    }

    async fn open(
        &self,
        lifecycle: &mut OplogLifecycleGuard,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
        last_oplog_index: Option<OplogIndex>,
        initial_worker_metadata: AgentMetadata,
        last_known_status: read_only_lock::arc_swap::ReadOnlyView<AgentStatusRecord>,
        execution_status: read_only_lock::std::ReadOnlyLock<ExecutionStatus>,
    ) -> Arc<dyn Oplog> {
        self.oplogs
            .get_or_open(
                lifecycle,
                &owned_agent_id.agent_id,
                CreateOplogConstructor::new(
                    owned_agent_id.clone(),
                    agent_mode,
                    None,
                    self.primary.clone(),
                    self.clone(),
                    last_oplog_index,
                    false,
                    initial_worker_metadata,
                    last_known_status,
                    execution_status,
                ),
            )
            .await
    }

    async fn get_last_index(
        &self,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
    ) -> OplogIndex {
        let mut result = self
            .primary
            .get_last_index(owned_agent_id, agent_mode)
            .await;
        if result == OplogIndex::NONE {
            for layer in &self.lower {
                let idx = layer.get_last_index(owned_agent_id, agent_mode).await;
                if idx != OplogIndex::NONE {
                    result = idx;
                    break;
                }
            }
        }
        result
    }

    async fn delete(
        &self,
        lifecycle: &mut OplogLifecycleGuard,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
    ) {
        lifecycle.assert_agent(&owned_agent_id.agent_id);
        self.abort_transfer(&owned_agent_id.agent_id).await;
        self.primary
            .delete(lifecycle, owned_agent_id, agent_mode)
            .await;
        for layer in &self.lower {
            layer.delete(owned_agent_id, agent_mode).await
        }
    }

    async fn read_exact(
        &self,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
        idx: OplogIndex,
        n: u64,
    ) -> BTreeMap<OplogIndex, OplogEntry> {
        let mut read = fail_stop(OplogRead::new(idx, n));

        if let Some((start, count)) = read.next_range() {
            let entries = self
                .primary
                .read_source(owned_agent_id, agent_mode, start, count)
                .await;
            fail_stop(read.add_source(OplogReadSource::Primary, entries));
        }

        for (level, layer) in self.lower.iter().enumerate() {
            if let Some((start, count)) = read.next_range() {
                let entries = layer
                    .read_source(owned_agent_id, agent_mode, start, count)
                    .await;
                fail_stop(read.add_source(OplogReadSource::Archive(level), entries));
            } else {
                break;
            }
        }

        fail_stop(read.finish())
    }

    async fn read_source(
        &self,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
        idx: OplogIndex,
        n: u64,
    ) -> BTreeMap<OplogIndex, OplogEntry> {
        let mut result = self
            .primary
            .read_source(owned_agent_id, agent_mode, idx, n)
            .await;
        for layer in &self.lower {
            if result.len() >= n as usize {
                break;
            }
            for (index, entry) in layer.read_source(owned_agent_id, agent_mode, idx, n).await {
                result.entry(index).or_insert(entry);
            }
        }
        result
    }

    async fn exists(&self, owned_agent_id: &OwnedAgentId, agent_mode: AgentMode) -> bool {
        if self.primary.exists(owned_agent_id, agent_mode).await {
            return true;
        }

        for layer in &self.lower {
            if agent_mode == AgentMode::Ephemeral {
                crate::metrics::ephemeral::record_lower_oplog_existence_read();
            }
            if layer.exists(owned_agent_id, agent_mode).await {
                return true;
            }
        }

        false
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
        let layer = state.layer;
        if layer > self.lower.len().get() {
            return Err(WorkerExecutorError::invalid_request(format!(
                "Invalid oplog layer in scan cursor: {layer}"
            )));
        }
        let active_mode = state.mode;

        match layer {
            0 => {
                let (new_cursor, unfiltered_ids) = self
                    .primary
                    .scan_for_component(environment_id, component_id, modes, cursor.clone(), count)
                    .await?;
                let ids = self
                    .filter_ids_existing_on_lower_layers(unfiltered_ids, active_mode, 0)
                    .await?;

                if new_cursor.is_finished() {
                    Ok((first_scan_cursor(1, modes)?, ids))
                } else {
                    Ok((new_cursor, ids))
                }
            }
            layer if layer <= self.lower.len().get() => {
                let (new_cursor, unfiltered_ids) = self.lower[layer - 1]
                    .scan_for_component(environment_id, component_id, modes, cursor.clone(), count)
                    .await?;

                let ids = self
                    .filter_ids_existing_on_lower_layers(unfiltered_ids, active_mode, layer)
                    .await?;
                if new_cursor.is_finished() && (layer + 1) <= self.lower.len().get() {
                    Ok((first_scan_cursor(layer + 1, modes)?, ids))
                } else if new_cursor.is_finished() {
                    Ok((ScanCursor::default(), ids))
                } else {
                    Ok((new_cursor, ids))
                }
            }
            _ => unreachable!(),
        }
    }

    async fn upload_raw_payload(
        &self,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
        data: Vec<u8>,
    ) -> Result<RawOplogPayload, String> {
        self.primary
            .upload_raw_payload(owned_agent_id, agent_mode, data)
            .await
    }

    async fn download_raw_payload(
        &self,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
        payload_id: PayloadId,
        md5_hash: Vec<u8>,
    ) -> Result<Vec<u8>, String> {
        self.primary
            .download_raw_payload(owned_agent_id, agent_mode, payload_id, md5_hash)
            .await
    }
}

pub struct MultiLayerOplog {
    owned_agent_id: OwnedAgentId,
    #[allow(dead_code)] // retained for diagnostics and future use
    agent_mode: AgentMode,
    primary: Arc<dyn Oplog>,
    lower: NEVec<Arc<dyn OplogArchive + Send + Sync>>,
    retired: AtomicBool,
    multi_layer_oplog_service: MultiLayerOplogService,
    transfer_fiber: TransferFiber,
    transfer: UnboundedSender<BackgroundTransferMessage>,
    last_reported_commit_index: AtomicOplogIndex,
    last_transfer_point: AtomicOplogIndex,
    close_fn: Mutex<Option<Box<dyn FnOnce() + Send + Sync>>>,
}

impl MultiLayerOplog {
    #[allow(clippy::new_ret_no_self)]
    pub async fn new(
        owned_agent_id: OwnedAgentId,
        agent_mode: AgentMode,
        account_id: AccountId,
        primary: Arc<dyn Oplog>,
        multi_layer_oplog_service: MultiLayerOplogService,
        close: Box<dyn FnOnce() + Send + Sync>,
    ) -> Arc<dyn Oplog> {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();

        let mut lower: Vec<Arc<dyn OplogArchive + Send + Sync>> = Vec::new();
        for (i, layer) in multi_layer_oplog_service.lower.iter().enumerate() {
            if i != (multi_layer_oplog_service.lower.len().get() - 1) {
                let raw = layer.open(&owned_agent_id, agent_mode).await;
                let instrumented = Arc::new(InstrumentedOplogArchive::new(
                    raw,
                    account_id,
                    owned_agent_id.environment_id(),
                ));
                lower.push(Arc::new(
                    WrappedOplogArchive::new(
                        i,
                        instrumented,
                        tx.clone(),
                        multi_layer_oplog_service.entry_count_limit,
                    )
                    .await,
                ));
            } else {
                let raw = layer.open(&owned_agent_id, agent_mode).await;
                lower.push(Arc::new(InstrumentedOplogArchive::new(
                    raw,
                    account_id,
                    owned_agent_id.environment_id(),
                )));
            }
        }
        let lower = NEVec::try_from_vec(lower).expect("At least one lower layer is required");

        let last_reported_commit_index =
            AtomicOplogIndex::from_oplog_index(primary.current_oplog_index().await);
        let mut archived_through = OplogIndex::NONE;
        for layer in &lower {
            archived_through = archived_through.max(layer.get_last_index().await);
        }
        let last_transfer_point = AtomicOplogIndex::from_oplog_index(archived_through);
        let result = Arc::new(Self {
            owned_agent_id: owned_agent_id.clone(),
            agent_mode,
            primary: primary.clone(),
            lower: lower.clone(),
            retired: AtomicBool::new(false),
            multi_layer_oplog_service: multi_layer_oplog_service.clone(),
            transfer_fiber: new_transfer_fiber(),
            transfer: tx,
            last_reported_commit_index,
            last_transfer_point,
            close_fn: Mutex::new(Some(close)),
        });
        let result_oplog: Arc<dyn Oplog> = result.clone();
        multi_layer_oplog_service.register_transfer(
            result.owned_agent_id.agent_id.clone(),
            &result.transfer_fiber,
        );
        let (start_tx, start_rx) = tokio::sync::oneshot::channel();
        let transfer_fiber = tokio::spawn(async move {
            if start_rx.await.is_ok() {
                Self::background_transfer(
                    owned_agent_id,
                    agent_mode,
                    Arc::downgrade(&result_oplog),
                    lower,
                    multi_layer_oplog_service.clone(),
                    rx,
                )
                .await;
            }
        });
        result
            .set_background_transfer(start_tx, transfer_fiber)
            .await;

        result
    }

    async fn set_background_transfer(&self, start: Sender<()>, fiber: tokio::task::JoinHandle<()>) {
        MultiLayerOplogService::start_transfer(&self.transfer_fiber, start, fiber).await;
    }

    async fn background_transfer(
        owned_agent_id: OwnedAgentId,
        agent_mode: AgentMode,
        primary: Weak<dyn Oplog>,
        lower: NEVec<Arc<dyn OplogArchive + Send + Sync>>,
        multi_layer_oplog_service: MultiLayerOplogService,
        mut rx: UnboundedReceiver<BackgroundTransferMessage>,
    ) {
        // TODO: monitor queue length

        while let Some(msg) = rx.recv().await {
            match msg {
                TransferFromPrimary {
                    last_transferred_idx,
                    mut keep_alive,
                    done,
                    transfer_origin,
                } => {
                    async {
                        info!(
                            "Transferring oplog entries up to index {last_transferred_idx} of the primary oplog to the next layer"
                        );
                        debug!("Reading entries from the primary oplog");

                        if let Some(primary) = primary.upgrade() {
                            let transfer = BackgroundTransferFromPrimary::new(
                                owned_agent_id.clone(),
                                agent_mode,
                                last_transferred_idx,
                                multi_layer_oplog_service.clone(),
                                primary.clone(),
                                lower.clone(),
                            );
                            transfer.run().await;
                            let _ = keep_alive.take();

                            if let Some(done) = done {
                                done.send(()).unwrap()
                            }
                        }
                    }
                    .instrument(related_span!(
                        transfer_origin,
                        Level::INFO,
                        "oplog_background_transfer",
                        agent_id = %owned_agent_id.agent_id,
                        from = "primary",
                        last_transferred_idx = %last_transferred_idx,
                    ))
                    .await;
                }
                TransferFromLower {
                    source,
                    last_transferred_idx,
                    mut keep_alive,
                    done,
                    drain: _,
                    transfer_origin,
                } => {
                    async {
                        info!(
                            "Transferring oplog entries up to index {last_transferred_idx} of oplog layer {source} to the next layer"
                        );
                        debug!("Reading entries from oplog layer {source}");

                        transfer_between_lower_layers(
                            source,
                            last_transferred_idx,
                            lower.clone(),
                        )
                        .await;
                        let _ = keep_alive.take();

                        if let Some(done) = done {
                            done.send(()).unwrap()
                        }
                    }
                    .instrument(related_span!(
                        transfer_origin,
                        Level::INFO,
                        "oplog_background_transfer",
                        agent_id = %owned_agent_id.agent_id,
                        // A string in both arms: the same field name typed
                        // differently per call site conflicts in a typed store.
                        from = %format!("layer-{source}"),
                        last_transferred_idx = %last_transferred_idx,
                    ))
                    .await;
                }
            }
        }
    }

    pub async fn try_archive(this: &Arc<dyn Oplog>) -> Option<bool> {
        let this = downcast_oplog::<MultiLayerOplog>(this)?;
        Some(Self::archive(this, false).await)
    }

    pub async fn try_archive_blocking(this: &Arc<dyn Oplog>) -> Option<bool> {
        let this = downcast_oplog::<MultiLayerOplog>(this)?;
        Some(Self::archive(this, true).await)
    }

    async fn archive(this: Arc<Self>, blocking: bool) -> bool {
        let (done_tx, done_rx) = if blocking {
            let (done_tx, done_rx) = tokio::sync::oneshot::channel();
            (Some(done_tx), Some(done_rx))
        } else {
            (None, None)
        };
        let result = if this.primary.length().await > 0 {
            // Unreported automatic commits must remain in primary storage until the next
            // explicit commit returns them to the status reducer.
            let last_transferred_idx = this.last_reported_commit_index.get();
            if last_transferred_idx == OplogIndex::NONE {
                return true;
            }
            this.transfer
                .send(TransferFromPrimary {
                    last_transferred_idx,
                    keep_alive: Some(this.clone()),
                    done: done_tx,
                    transfer_origin: TraceOrigin::capture_current(),
                })
                .expect("Failed to enqueue transfer of primary oplog entries");

            // Retry if additions beyond the reported prefix still need archiving.
            this.lower.len().get() > 1
                || this.primary.current_oplog_index().await > last_transferred_idx
        } else {
            let mut n = 0;
            let first_non_empty = loop {
                let length = this.lower[n].length().await;
                if length > 0 {
                    break Some(n);
                } else if n < this.lower.len().get() - 2 {
                    // skipping the last layer as there is nowhere to transfer to from there
                    n += 1;
                } else {
                    break None;
                }
            };

            if let Some(first_non_empty) = first_non_empty {
                // transferring the whole non-empty lower layer to the next layer
                this.transfer
                    .send(TransferFromLower {
                        source: first_non_empty,
                        last_transferred_idx: this.lower[first_non_empty]
                            .current_oplog_index()
                            .await,
                        keep_alive: Some(this.clone()),
                        done: done_tx,
                        drain: false,
                        transfer_origin: TraceOrigin::capture_current(),
                    })
                    .expect("Failed to enqueue transfer of primary oplog entries");

                // If there are more layers to transfer from, return true
                first_non_empty < this.lower.len().get() - 2
            } else {
                // Fully archived, and no transfer was enqueued to wait for
                return false;
            }
        };

        if let Some(done_rx) = done_rx {
            done_rx
                .await
                .expect("Failed to wait for the archiving to finish");
        }

        result
    }
}

impl Drop for MultiLayerOplog {
    fn drop(&mut self) {
        self.multi_layer_oplog_service
            .unregister_transfer(&self.owned_agent_id.agent_id, &self.transfer_fiber);
        if let Some(close_fn) = self.close_fn.get_mut().unwrap().take() {
            close_fn();
        }
        self.multi_layer_oplog_service
            .abort_transfer_in_drop(&self.transfer_fiber);
    }
}

impl Debug for MultiLayerOplog {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MultiLayerOplog")
            .field("agent_id", &self.owned_agent_id)
            .finish()
    }
}

#[async_trait]
impl Oplog for MultiLayerOplog {
    fn retire(&self) {
        self.retired.store(true, Ordering::Release);
        self.multi_layer_oplog_service
            .unregister_transfer(&self.owned_agent_id.agent_id, &self.transfer_fiber);
        self.multi_layer_oplog_service
            .abort_transfer_in_drop(&self.transfer_fiber);
    }

    fn is_retired(&self) -> bool {
        self.retired.load(Ordering::Acquire) || self.primary.is_retired()
    }

    fn closed(&self) -> OplogCloseCompletion {
        MultiLayerOplogService::transfer_closed(&self.transfer_fiber)
    }

    fn task_owner(&self) -> Option<&super::WorkerTasks> {
        self.primary.task_owner()
    }

    async fn stop_and_wait(&self) -> Result<(), String> {
        let tasks_result = if let Some(tasks) = self.task_owner() {
            tasks.stop_and_wait().await
        } else {
            Ok(())
        };
        self.retire();
        let result = self.closed().await;
        let primary_result = self.primary.stop_and_wait().await;
        tasks_result.and(result).and(primary_result)
    }

    fn enqueue_add(&self, entry: OplogEntry) -> OplogAddReceipt {
        self.primary.enqueue_add(entry)
    }

    async fn add_durable_stream_batch(
        &self,
        make_batch: DurableStreamBatchBuilder,
    ) -> Result<Vec<(OplogIndex, OplogEntry)>, String> {
        self.primary.add_durable_stream_batch(make_batch).await
    }

    async fn drop_prefix(&self, last_dropped_id: OplogIndex) -> u64 {
        let dropped_entries = self.primary.drop_prefix(last_dropped_id).await;
        self.last_transfer_point.max(last_dropped_id);
        dropped_entries
    }

    async fn commit(&self, level: CommitLevel) -> BTreeMap<OplogIndex, OplogEntry> {
        let result = self.primary.commit(level).await;

        if let Some(index) = result.keys().next_back() {
            self.last_reported_commit_index.max(*index);
        }
        let last_committed_idx = self.last_reported_commit_index.get();
        let last_transferred_idx = self.last_transfer_point.get();
        let count = u64::from(last_committed_idx).saturating_sub(u64::from(last_transferred_idx));
        if count >= self.multi_layer_oplog_service.entry_count_limit {
            debug!(
                "Enqueuing transfer of {count} oplog entries from the primary oplog to the next layer up to {last_committed_idx}"
            );
            let _ = self.transfer.send(TransferFromPrimary {
                last_transferred_idx: last_committed_idx,
                keep_alive: None,
                done: None,
                transfer_origin: TraceOrigin::capture_current(),
            });
            self.last_transfer_point.max(last_committed_idx);
        }
        result
    }

    async fn current_oplog_index(&self) -> OplogIndex {
        self.primary.current_oplog_index().await
    }

    async fn raw_durable_stream_session_status(
        &self,
        session_key: &golem_common::model::durable_stream::StreamSessionKey,
    ) -> super::RawDurableStreamSessionStatus {
        self.primary
            .raw_durable_stream_session_status(session_key)
            .await
    }

    async fn last_added_non_hint_entry(&self) -> Option<OplogIndex> {
        self.primary.last_added_non_hint_entry().await
    }

    async fn wait_for_replicas(&self, replicas: u8, timeout: Duration) -> bool {
        self.primary.wait_for_replicas(replicas, timeout).await
    }

    async fn read_exact(&self, idx: OplogIndex, n: u64) -> BTreeMap<OplogIndex, OplogEntry> {
        let mut read = fail_stop(OplogRead::new(idx, n));

        if let Some((start, count)) = read.next_range() {
            let entries = self.primary.read_source(start, count).await;
            fail_stop(read.add_source(OplogReadSource::Primary, entries));
        }

        for (level, layer) in self.lower.iter().enumerate() {
            if let Some((start, count)) = read.next_range() {
                let entries = layer.read_source(start, count).await;
                fail_stop(read.add_source(OplogReadSource::Archive(level), entries));
            } else {
                break;
            }
        }

        fail_stop(read.finish())
    }

    async fn length(&self) -> u64 {
        let mut total_length = self.primary.length().await;
        for layer in &self.lower {
            total_length += layer.length().await;
        }
        total_length
    }

    async fn upload_raw_payload(&self, data: Vec<u8>) -> Result<RawOplogPayload, String> {
        self.primary.upload_raw_payload(data).await
    }

    async fn download_raw_payload(
        &self,
        payload_id: PayloadId,
        md5_hash: Vec<u8>,
    ) -> Result<Vec<u8>, String> {
        self.primary
            .download_raw_payload(payload_id, md5_hash)
            .await
    }

    fn enqueue_add_pair(
        &self,
        start: OplogEntry,
        make_second: Box<dyn FnOnce(OplogIndex) -> OplogEntry + Send>,
    ) -> super::OplogAddPairReceipt {
        self.primary.enqueue_add_pair(start, make_second)
    }

    async fn add_start_with_reserved_raw_payload(
        &self,
        serialized_request: Vec<u8>,
        build_start: ReservedRawStartBuilder,
    ) -> Result<OrderedOplogStart, String> {
        self.primary
            .add_start_with_reserved_raw_payload(serialized_request, build_start)
            .await
    }

    async fn add_start_with_indexed_reserved_raw_payload(
        &self,
        build_request: IndexedReservedStartBuilder,
    ) -> Result<OrderedOplogStart, String> {
        self.primary
            .add_start_with_indexed_reserved_raw_payload(build_request)
            .await
    }

    fn inner(&self) -> Option<Arc<dyn Oplog>> {
        Some(self.primary.clone())
    }
}

#[derive(Debug)]
pub enum BackgroundTransferMessage {
    TransferFromPrimary {
        last_transferred_idx: OplogIndex,
        keep_alive: Option<Arc<dyn Oplog>>,
        done: Option<Sender<()>>,
        transfer_origin: TraceOrigin,
    },
    TransferFromLower {
        source: usize,
        last_transferred_idx: OplogIndex,
        keep_alive: Option<Arc<dyn Oplog>>,
        done: Option<Sender<()>>,
        drain: bool,
        transfer_origin: TraceOrigin,
    },
}

#[async_trait]
trait BackgroundTransfer {
    async fn read_source(&self) -> Vec<(OplogIndex, OplogEntry)>;
    async fn append_target(&self, entries: &[(OplogIndex, OplogEntry)]);
    async fn verify_target(&self, entries: &[(OplogIndex, OplogEntry)]);
    async fn drop_source_prefix(&self, last_dropped_id: OplogIndex);

    async fn run(&self) {
        let entries = self.read_source().await;
        match entries.last() {
            Some(last_entry) => {
                let last_dropped_id = last_entry.0;
                self.append_target(&entries).await;
                self.verify_target(&entries).await;
                self.drop_source_prefix(last_dropped_id).await;
            }
            None => {
                warn!("No entries to transfer from the primary oplog");
            }
        }
    }
}

/// Wraps an open oplog archive to track the number of items written and automatically
/// scheduling transfers to lower levels when the limit is reached
#[derive(Debug)]
pub struct WrappedOplogArchive {
    layer: usize,
    archive: Arc<dyn OplogArchive + Send + Sync>,
    entry_count: AtomicU64,
    transfer: UnboundedSender<BackgroundTransferMessage>,
    entry_count_limit: u64,
}

impl WrappedOplogArchive {
    pub async fn new(
        layer: usize,
        archive: Arc<dyn OplogArchive + Send + Sync>,
        transfer: UnboundedSender<BackgroundTransferMessage>,
        entry_count_limit: u64,
    ) -> Self {
        let initial_entry_count = archive.length().await;
        Self {
            layer,
            archive,
            entry_count: AtomicU64::new(initial_entry_count),
            transfer,
            entry_count_limit,
        }
    }

    pub fn new_fresh(
        layer: usize,
        archive: Arc<dyn OplogArchive + Send + Sync>,
        transfer: UnboundedSender<BackgroundTransferMessage>,
        entry_count_limit: u64,
    ) -> Self {
        Self {
            layer,
            archive,
            entry_count: AtomicU64::new(0),
            transfer,
            entry_count_limit,
        }
    }
}

#[async_trait]
impl OplogArchive for WrappedOplogArchive {
    async fn read_source(&self, idx: OplogIndex, n: u64) -> BTreeMap<OplogIndex, OplogEntry> {
        self.archive.read_source(idx, n).await
    }

    async fn append(&self, chunk: &[(OplogIndex, OplogEntry)]) -> u64 {
        if !chunk.is_empty() {
            let last_idx = chunk.last().unwrap().0;
            let bytes = self.archive.append(chunk).await;
            let old_count = self.entry_count.fetch_add(1, Ordering::AcqRel); // Note: the whole chunk is stored as one entry, so incrementing only by one
            let count = old_count + 1;
            if count >= self.entry_count_limit {
                debug!(
                    "Enqueuing transfer of oplog entries from the oplog layer {} to the next layer up to {last_idx}",
                    self.layer
                );
                let _ = self.transfer.send(TransferFromLower {
                    source: self.layer,
                    last_transferred_idx: last_idx,
                    keep_alive: None,
                    done: None,
                    drain: false,
                    transfer_origin: TraceOrigin::capture_current(),
                });
                // Resetting the counter, otherwise it would trigger additional transfers until the background process finishes
                self.entry_count.store(0, Ordering::Release);
            }
            bytes
        } else {
            0
        }
    }

    async fn verify_persisted(&self, entries: &[(OplogIndex, OplogEntry)]) {
        self.archive.verify_persisted(entries).await
    }

    async fn current_oplog_index(&self) -> OplogIndex {
        self.archive.current_oplog_index().await
    }

    async fn drop_prefix(&self, last_dropped_id: OplogIndex) -> u64 {
        let dropped_entries = self.archive.drop_prefix(last_dropped_id).await;
        let new_length = self.archive.length().await;
        let old_entry_count = self.entry_count.load(Ordering::Acquire);
        let new_entry_count = min(new_length, old_entry_count);
        self.entry_count.store(new_entry_count, Ordering::Release);
        dropped_entries
    }

    async fn length(&self) -> u64 {
        self.archive.length().await
    }

    async fn get_last_index(&self) -> OplogIndex {
        self.archive.get_last_index().await
    }
}

struct BackgroundTransferFromPrimary {
    owned_agent_id: OwnedAgentId,
    agent_mode: AgentMode,
    last_transferred_idx: OplogIndex,
    multi_layer_oplog_service: MultiLayerOplogService,
    primary: Arc<dyn Oplog>,
    lower: NEVec<Arc<dyn OplogArchive + Send + Sync>>,
}

impl BackgroundTransferFromPrimary {
    pub fn new(
        owned_agent_id: OwnedAgentId,
        agent_mode: AgentMode,
        last_transferred_idx: OplogIndex,
        multi_layer_oplog_service: MultiLayerOplogService,
        primary: Arc<dyn Oplog>,
        lower: NEVec<Arc<dyn OplogArchive + Send + Sync>>,
    ) -> Self {
        Self {
            owned_agent_id,
            agent_mode,
            last_transferred_idx,
            multi_layer_oplog_service,
            primary,
            lower,
        }
    }
}

#[async_trait]
impl BackgroundTransfer for BackgroundTransferFromPrimary {
    async fn read_source(&self) -> Vec<(OplogIndex, OplogEntry)> {
        let entries = self
            .multi_layer_oplog_service
            .primary
            .read_source(
                &self.owned_agent_id,
                self.agent_mode,
                OplogIndex::INITIAL,
                self.last_transferred_idx.as_u64(),
            )
            .await;
        fail_stop(validate_transfer_source(
            entries,
            self.last_transferred_idx,
            OplogReadSource::Primary,
        ))
    }

    async fn append_target(&self, entries: &[(OplogIndex, OplogEntry)]) {
        let _ = self.lower.first().append(entries).await;
    }

    async fn verify_target(&self, entries: &[(OplogIndex, OplogEntry)]) {
        self.lower.first().verify_persisted(entries).await
    }

    async fn drop_source_prefix(&self, last_dropped_id: OplogIndex) {
        self.primary.drop_prefix(last_dropped_id).await;
    }
}

struct BackgroundTransferBetweenLowers {
    last_transferred_idx: OplogIndex,
    source_layer: Arc<dyn OplogArchive + Send + Sync>,
    target_layer: Arc<dyn OplogArchive + Send + Sync>,
}

impl BackgroundTransferBetweenLowers {
    pub fn new(
        source: usize,
        last_transferred_idx: OplogIndex,
        lower: NEVec<Arc<dyn OplogArchive + Send + Sync>>,
    ) -> Self {
        let source_layer = lower[source].clone();
        let target_layer = lower[source + 1].clone();

        Self {
            last_transferred_idx,
            source_layer,
            target_layer,
        }
    }
}

pub(crate) async fn transfer_between_lower_layers(
    source: usize,
    last_transferred_idx: OplogIndex,
    lower: NEVec<Arc<dyn OplogArchive + Send + Sync>>,
) {
    BackgroundTransferBetweenLowers::new(source, last_transferred_idx, lower)
        .run()
        .await
}

#[async_trait]
impl BackgroundTransfer for BackgroundTransferBetweenLowers {
    async fn read_source(&self) -> Vec<(OplogIndex, OplogEntry)> {
        let entries = self
            .source_layer
            .read_source(OplogIndex::INITIAL, self.last_transferred_idx.as_u64())
            .await;
        fail_stop(validate_transfer_source(
            entries,
            self.last_transferred_idx,
            OplogReadSource::Other("transfer source archive"),
        ))
    }

    async fn append_target(&self, entries: &[(OplogIndex, OplogEntry)]) {
        let _ = self.target_layer.append(entries).await;
    }

    async fn verify_target(&self, entries: &[(OplogIndex, OplogEntry)]) {
        self.target_layer.verify_persisted(entries).await
    }

    async fn drop_source_prefix(&self, last_dropped_id: OplogIndex) {
        self.source_layer.drop_prefix(last_dropped_id).await;
    }
}

fn validate_transfer_source(
    entries: BTreeMap<OplogIndex, OplogEntry>,
    expected_end: OplogIndex,
    source: OplogReadSource,
) -> Result<Vec<(OplogIndex, OplogEntry)>, OplogReadError> {
    let Some((&start, _)) = entries.first_key_value() else {
        return Ok(Vec::new());
    };
    let (&end, _) = entries.last_key_value().unwrap();
    if end != expected_end || end.as_u64() - start.as_u64() + 1 != entries.len() as u64 {
        return Err(OplogReadError::corruption(
            source,
            format!(
                "transfer source returned {} entries in range [{start}..={end}], expected a contiguous suffix ending at {expected_end}",
                entries.len()
            ),
        ));
    }
    Ok(entries.into_iter().collect())
}

#[cfg(test)]
mod transfer_lifecycle_tests {
    use super::*;
    use crate::services::oplog::compressed::CompressedOplogArchiveService;
    use crate::services::oplog::primary::PrimaryOplogService;
    use crate::services::oplog::tests::{
        default_execution_status, default_last_known_status, make_agent_metadata,
    };
    use crate::storage::indexed::memory::InMemoryIndexedStorage;
    use golem_common::model::oplog::{AgentError, OplogErrorKind};
    use golem_common::model::{RetryConfig, Timestamp};
    use golem_service_base::storage::blob::memory::InMemoryBlobStorage;
    use nonempty_collections::nev;
    use proptest::prelude::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use test_r::test;

    test_r::enable!();

    #[test]
    async fn observer_open_preserves_cursor_when_primary_grows() {
        observer_open_after_primary_growth(0).await;
    }

    #[test]
    async fn observer_open_uses_deep_archive_watermark_when_primary_grows() {
        observer_open_after_primary_growth(3).await;
    }

    async fn observer_open_after_primary_growth(archived: u64) {
        let indexed = Arc::new(InMemoryIndexedStorage::new());
        let blob = Arc::new(InMemoryBlobStorage::new());
        let writer_service = PrimaryOplogService::new(
            indexed.clone(),
            blob.clone(),
            100,
            100,
            100,
            RetryConfig::default(),
        )
        .await;
        let observer_service = Arc::new(
            PrimaryOplogService::new(indexed.clone(), blob, 100, 100, 100, RetryConfig::default())
                .await,
        );
        let first: Arc<dyn OplogArchiveService> = Arc::new(CompressedOplogArchiveService::new(
            indexed.clone(),
            1,
            RetryConfig::default(),
        ));
        let deepest: Arc<dyn OplogArchiveService> = Arc::new(CompressedOplogArchiveService::new(
            indexed,
            2,
            RetryConfig::default(),
        ));
        let account = AccountId::new();
        let environment = EnvironmentId::new();
        let agent = AgentId {
            component_id: ComponentId::new(),
            agent_id: "observer".into(),
        };
        let owned = OwnedAgentId::new(environment, &agent);
        let metadata = make_agent_metadata(agent.clone(), account, environment);
        let writer = writer_service
            .open(
                &mut writer_service.lock_lifecycle(&agent).await,
                &owned,
                AgentMode::Durable,
                None,
                metadata.clone(),
                default_last_known_status(),
                default_execution_status(AgentMode::Durable),
            )
            .await;
        let entries = (1..=archived + 10)
            .map(|index| {
                OplogEntry::Error {
                    timestamp: Timestamp::now_utc(),
                    entity_parent_start_index: None,
                    kind: OplogErrorKind::Invocation,
                    error: AgentError::Unknown(index.to_string()),
                    retry_from: OplogIndex::NONE,
                    inside_atomic_region: false,
                    retry_policy_state: None,
                }
                .rounded()
            })
            .collect::<Vec<_>>();
        let captured = archived + 2;
        for entry in &entries[..captured as usize] {
            writer.add(entry.clone()).await;
        }
        writer.commit(CommitLevel::Always).await;
        let deep_archive = deepest.open(&owned, AgentMode::Durable).await;
        if archived > 0 {
            let prefix = entries[..archived as usize]
                .iter()
                .enumerate()
                .map(|(index, entry)| (OplogIndex::from_u64(index as u64 + 1), entry.clone()))
                .collect::<Vec<_>>();
            deep_archive.append(&prefix).await;
            writer.drop_prefix(OplogIndex::from_u64(archived)).await;
        }
        let observer = observer_service
            .open(
                &mut observer_service.lock_lifecycle(&agent).await,
                &owned,
                AgentMode::Durable,
                None,
                metadata,
                default_last_known_status(),
                default_execution_status(AgentMode::Durable),
            )
            .await;
        assert_eq!(
            observer.current_oplog_index().await,
            OplogIndex::from_u64(captured)
        );
        for entry in &entries[captured as usize..] {
            writer.add(entry.clone()).await;
        }
        writer.commit(CommitLevel::Always).await;
        assert_eq!(observer.length().await, 10);

        let service = MultiLayerOplogService::new(observer_service, nev![first, deepest], 100, 100);
        let observer = MultiLayerOplog::new(
            owned,
            AgentMode::Durable,
            account,
            observer,
            service,
            Box::new(|| {}),
        )
        .await;
        let layered = downcast_oplog::<MultiLayerOplog>(&observer).unwrap();
        assert_eq!(
            layered.last_transfer_point.get(),
            OplogIndex::from_u64(archived)
        );
        assert_eq!(
            observer.current_oplog_index().await,
            OplogIndex::from_u64(captured)
        );
        for (index, entry) in entries[..captured as usize].iter().enumerate() {
            assert_eq!(
                &observer.read(OplogIndex::from_u64(index as u64 + 1)).await,
                entry
            );
        }
        assert_eq!(
            writer.current_oplog_index().await,
            OplogIndex::from_u64(archived + 10)
        );
        assert_eq!(writer.length().await, 10);
        assert_eq!(
            deep_archive.get_last_index().await,
            OplogIndex::from_u64(archived)
        );
    }

    #[derive(Debug, Clone)]
    enum Operation {
        Construct,
        Delete,
        Yield,
    }

    fn operations() -> impl Strategy<Value = Vec<Operation>> {
        prop::collection::vec(
            prop_oneof![
                Just(Operation::Construct),
                Just(Operation::Delete),
                Just(Operation::Yield),
            ],
            1..32,
        )
    }

    async fn cancel_transfer(transfer_fiber: &TransferFiber) {
        MultiLayerOplogService::cancel_transfer(transfer_fiber)
            .await
            .unwrap();
    }

    proptest! {
        /// A transfer registered before its task is constructed must remain inert when deletion wins
        /// the race. This is the lifecycle fence used by both durable and ephemeral oplogs.
        #[test]
        fn deleted_transfer_never_starts(operations in operations()) {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();

            runtime.block_on(async move {
                let transfer_fiber = new_transfer_fiber();
                let started = Arc::new(AtomicUsize::new(0));
                let mut write_gate = None;
                let mut constructed = false;
                let mut deleted = false;

                for operation in operations {
                    match operation {
                        Operation::Construct if !constructed => {
                            let (start_tx, start_rx) = tokio::sync::oneshot::channel();
                            let (write_tx, write_rx) = tokio::sync::oneshot::channel();
                            let started = started.clone();
                            let transfer = tokio::spawn(async move {
                                if start_rx.await.is_ok() && write_rx.await.is_ok() {
                                    started.fetch_add(1, Ordering::SeqCst);
                                }
                            });

                            MultiLayerOplogService::start_transfer(
                                &transfer_fiber,
                                start_tx,
                                transfer,
                            )
                            .await;
                            write_gate = Some(write_tx);
                            constructed = true;
                        }
                        Operation::Delete if !deleted => {
                            cancel_transfer(&transfer_fiber).await;
                            deleted = true;
                        }
                        Operation::Yield => tokio::task::yield_now().await,
                        _ => {}
                    }
                }

                if let Some(write_gate) = write_gate {
                    if deleted {
                        drop(write_gate);
                        prop_assert_eq!(started.load(Ordering::SeqCst), 0);
                    } else {
                        let _ = write_gate.send(());
                        let transfer = {
                            transfer_fiber
                                .lock()
                                .unwrap()
                                .closed
                                .clone()
                        };
                        if let Some(transfer) = transfer {
                            transfer.await.unwrap();
                        }
                        prop_assert_eq!(started.load(Ordering::SeqCst), 1);
                    }
                }

                Ok(())
            })
            .unwrap();
        }
    }
}
