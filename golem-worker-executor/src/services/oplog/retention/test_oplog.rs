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

//! An oplog stack with the retention layer over an in-memory primary oplog, for the tests of the
//! layer and of the readers that go by the retained start.

use super::{OplogRetention, RetentionOplogService, advance_oplog_retention};
use crate::model::ExecutionStatus;
use crate::services::oplog::{
    CommitLevel, DurableStreamBatchBuilder, IndexedReservedStartBuilder, Oplog,
    OplogAddPairReceipt, OplogAddReceipt, OplogError, OplogFence, OplogLifecycleGuard,
    OplogReadError, OplogService, OrderedOplogStart, PrimaryOplogService,
    RawOplogPayloadDownloadError, ReservedRawStartBuilder, ScanPage, StagePublication,
    UnreadableOplog, WorkerTasks,
};
use crate::services::stream_session_index::StreamSessionIndexService;
use crate::storage::indexed::memory::InMemoryIndexedStorage;
use crate::storage::keyvalue::KeyValueStorage;
use crate::storage::keyvalue::memory::InMemoryKeyValueStorage;
use async_trait::async_trait;
use futures::{StreamExt, TryStreamExt};
use golem_common::model::account::{AccountEmail, AccountId};
use golem_common::model::agent::{AgentMode, OwnerKind};
use golem_common::model::caller::Caller;
use golem_common::model::component::ComponentId;
use golem_common::model::environment::EnvironmentId;
use golem_common::model::invocation_context::InvocationContextStack;
use golem_common::model::oplog::{
    CreateRequest, CreateRequestKind, OplogEntry, OplogIndex, PayloadId, RawOplogPayload,
};
use golem_common::model::regions::OplogRegion;
use golem_common::model::type_id::TypeId;
use golem_common::model::{
    AgentFingerprint, AgentId, AgentMetadata, AgentStatusRecord, OwnedAgentId, ScanCursor,
    ShardEpoch, Timestamp,
};
use golem_common::read_only_lock;
use golem_service_base::error::worker_executor::WorkerExecutorError;
use golem_service_base::storage::blob::memory::InMemoryBlobStorage;
use std::collections::BTreeMap;
use std::fmt::{Debug, Formatter};
use std::sync::atomic::Ordering;
use std::sync::{Arc, RwLock};
use std::time::Duration;

pub(crate) struct RetainedOplog {
    pub service: Arc<RetentionOplogService>,
    pub owned_agent_id: OwnedAgentId,
    pub component_id: ComponentId,
    key_value_storage: Arc<dyn KeyValueStorage + Send + Sync>,
}

pub(crate) fn idx(value: u64) -> OplogIndex {
    OplogIndex::from_u64(value)
}

/// An entry with a fixed timestamp, because the oplog stores timestamps in milliseconds. The jump
/// names its own index, so that each entry is different.
pub(crate) fn jump(index: u64) -> OplogEntry {
    OplogEntry::Jump {
        timestamp: Timestamp::from(index),
        entity_parent_start_index: None,
        jump: OplogRegion {
            start: idx(index),
            end: idx(index),
        },
    }
}

/// The start of an invocation, as the walks that count invocations see it.
pub(crate) fn invocation_started() -> OplogEntry {
    use golem_common::model::card::{InvocationWalletPin, WalletVersionToken};
    use golem_common::model::{InvocationRequestOperation, StoredAgentInvocationPayload};

    OplogEntry::AgentInvocationStarted {
        timestamp: Timestamp::from(1),
        invocation_id: uuid::Uuid::from_u128(1).into(),
        operation: Box::new(InvocationRequestOperation::SaveSnapshot),
        payload: golem_common::model::oplog::payload::OplogPayload::Inline(Box::new(
            StoredAgentInvocationPayload::SaveSnapshot {
                caller: Caller::anonymous(),
            },
        )),
        trace_id: golem_common::model::invocation_context::TraceId::generate(),
        trace_states: Vec::new(),
        invocation_context: Vec::new(),
        wallet_pin: Box::new(InvocationWalletPin {
            wallet_token: WalletVersionToken {
                wallet_id_hash: [0; 32],
                generation: 0,
            },
            pinned_card_ids: Vec::new(),
            scope_card_id: None,
        }),
    }
}

/// The retention whose start is at `start`.
pub(crate) fn at(start: u64) -> OplogRetention {
    OplogRetention::FULL.advanced_to(idx(start))
}

pub(crate) fn expired(start: u64) -> OplogReadError {
    OplogReadError::Expired {
        retained_start: idx(start),
    }
}

fn last_known_status() -> read_only_lock::arc_swap::ReadOnlyView<AgentStatusRecord> {
    read_only_lock::arc_swap::ReadOnlyView::new(Arc::new(arc_swap::ArcSwap::from_pointee(
        AgentStatusRecord::default(),
    )))
}

fn execution_status() -> read_only_lock::std::ReadOnlyLock<ExecutionStatus> {
    read_only_lock::std::ReadOnlyLock::new(Arc::new(RwLock::new(ExecutionStatus::Suspended {
        agent_mode: AgentMode::Durable,
        timestamp: Timestamp::now_utc(),
    })))
}

impl RetainedOplog {
    pub async fn new() -> Self {
        Self::with_key_value_storage(Arc::new(InMemoryKeyValueStorage::new())).await
    }

    /// The stack with `key_value_storage` as the storage of the retention record.
    pub async fn with_key_value_storage(
        key_value_storage: Arc<dyn KeyValueStorage + Send + Sync>,
    ) -> Self {
        let primary = PrimaryOplogService::new(
            Arc::new(InMemoryIndexedStorage::new()),
            Arc::new(InMemoryBlobStorage::new()),
            1,
            1,
            4096,
            32,
            golem_common::model::RetryConfig::default(),
        )
        .await;
        let agent_id = AgentId::from_parts(7, 1);
        Self {
            service: Arc::new(RetentionOplogService::new(
                Arc::new(primary),
                key_value_storage.clone(),
            )),
            owned_agent_id: OwnedAgentId::new(EnvironmentId::new(), &agent_id),
            component_id: ComponentId::new(),
            key_value_storage,
        }
    }

    pub fn agent_id(&self) -> &AgentId {
        &self.owned_agent_id.agent_id
    }

    fn metadata(&self) -> AgentMetadata {
        AgentMetadata {
            component_id: self.component_id,
            agent_type_name: None,
            canonical_key: None,
            constructor_parameters: None,
            agent_id: *self.agent_id(),
            owner_kind: OwnerKind::ComponentAgent,
            env: vec![],
            environment_id: self.owned_agent_id.environment_id,
            created_by: AccountId::new(),
            created_by_email: AccountEmail::new("test@golem"),
            config: Vec::new(),
            created_at: Timestamp::now_utc(),
            parent: None,
            last_known_status: AgentStatusRecord::default(),
            fingerprint: AgentFingerprint::new(),
            agent_mode: AgentMode::Durable,
            create_request: CreateRequest::new(
                CreateRequestKind::GetOrCreate,
                None,
                Caller::anonymous(),
                &InvocationContextStack::fresh_rounded(),
            ),
        }
    }

    /// The `Create` entry of the agent, in the agent mode `Durable`.
    pub fn create_entry(&self) -> OplogEntry {
        let metadata = self.metadata();
        OplogEntry::create(Box::new(golem_common::model::oplog::CreateParameters {
            agent_id: metadata.agent_id,
            component_id: metadata.component_id,
            agent_type_name: None,
            canonical_key: None,
            constructor_parameters: None,
            owner_kind: metadata.owner_kind,
            agent_mode: AgentMode::Durable,
            component_revision: golem_common::model::component::ComponentRevision::INITIAL,
            env: vec![],
            environment_id: metadata.environment_id,
            created_by: metadata.created_by,
            parent: None,
            component_size: 100,
            initial_total_linear_memory_size: 100,
            initial_active_plugins: Default::default(),
            local_agent_config: vec![],
            instance_id: metadata.fingerprint.0,
            create_request: metadata.create_request,
            never_live: false,
        }))
    }

    /// The key-value storage of the retention record.
    pub fn key_value_storage(&self) -> Arc<dyn KeyValueStorage + Send + Sync> {
        self.key_value_storage.clone()
    }

    pub async fn open(&self) -> Arc<dyn Oplog> {
        self.service
            .open(
                &mut self.service.lock_lifecycle(self.agent_id()).await,
                &self.owned_agent_id,
                AgentMode::Durable,
                None,
                self.metadata(),
                last_known_status(),
                execution_status(),
                None,
            )
            .await
            .unwrap()
    }

    /// A new oplog whose first entry is `first`, with `epoch` as its shard epoch.
    pub async fn create(&self, first: OplogEntry, epoch: ShardEpoch) -> Arc<dyn Oplog> {
        self.service
            .create(
                &mut self.service.lock_lifecycle(self.agent_id()).await,
                &self.owned_agent_id,
                AgentMode::Durable,
                first,
                self.metadata(),
                last_known_status(),
                execution_status(),
                Some(epoch),
            )
            .await
            .unwrap()
    }

    /// Adds `entries` in their order after the last entry, and commits them.
    pub async fn add(&self, entries: impl IntoIterator<Item = OplogEntry>) {
        let oplog = self.open().await;
        futures::stream::iter(entries)
            .then(|entry| oplog.add(entry))
            .try_collect::<Vec<_>>()
            .await
            .unwrap();
        oplog.commit(CommitLevel::Always).await.unwrap();
    }

    /// An oplog with the entries 1 to `last`, each made by [`jump`].
    pub async fn with_jumps(&self, last: u64) {
        self.add((1..=last).map(jump)).await;
    }

    pub async fn advance(&self, to: u64) -> OplogRetention {
        advance_oplog_retention(
            self.key_value_storage.as_ref(),
            &self.owned_agent_id,
            AgentMode::Durable,
            idx(to),
        )
        .await
        .unwrap()
    }

    pub async fn start(&self) -> OplogIndex {
        self.service
            .retention(&self.owned_agent_id, AgentMode::Durable)
            .await
            .unwrap()
            .start()
    }

    pub async fn delete(&self, expected_epoch: Option<ShardEpoch>) -> Result<(), OplogError> {
        self.service
            .delete(
                &mut self.service.lock_lifecycle(self.agent_id()).await,
                &self.owned_agent_id,
                AgentMode::Durable,
                expected_epoch,
            )
            .await
    }

    pub async fn read(
        &self,
        start: u64,
        n: u64,
    ) -> Result<BTreeMap<OplogIndex, OplogEntry>, OplogReadError> {
        self.service
            .read_exact(&self.owned_agent_id, AgentMode::Durable, idx(start), n)
            .await
            .map_err(|unreadable| unreadable.error)
    }
}

/// An open oplog that records each call that reaches it, with its arguments, and answers from the
/// arguments. A test of a wrapper sees from it whether a call reached the inner oplog with the
/// same arguments, and whether the answer came back unchanged. The calls that no test makes are
/// unreachable.
#[derive(Default)]
pub(crate) struct RecordingOplog {
    calls: std::sync::Mutex<Vec<String>>,
    retired: std::sync::atomic::AtomicBool,
    pub tasks: WorkerTasks,
}

impl RecordingOplog {
    fn record(&self, call: String) {
        self.calls.lock().unwrap().push(call);
    }

    /// The calls that reached the oplog since the last take, in their order.
    pub fn take_calls(&self) -> Vec<String> {
        std::mem::take(&mut self.calls.lock().unwrap())
    }
}

impl Debug for RecordingOplog {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RecordingOplog").finish()
    }
}

#[async_trait]
impl Oplog for RecordingOplog {
    fn retire(&self) {
        self.record("retire".to_string());
        self.retired.store(true, Ordering::SeqCst);
    }

    fn is_retired(&self) -> bool {
        self.retired.load(Ordering::SeqCst)
    }

    fn task_owner(&self) -> Option<&WorkerTasks> {
        Some(&self.tasks)
    }

    fn enqueue_add(&self, _entry: OplogEntry) -> OplogAddReceipt {
        unreachable!()
    }

    async fn add_durable_stream_batch(
        &self,
        make_batch: DurableStreamBatchBuilder,
    ) -> Result<Vec<(OplogIndex, OplogEntry)>, OplogError> {
        let records = make_batch(idx(21));
        self.record(format!("add_durable_stream_batch({})", records.len()));
        Ok(vec![(idx(21), jump(21))])
    }

    async fn drop_prefix(&self, _last_dropped_id: OplogIndex) -> u64 {
        unreachable!()
    }

    async fn try_drop_prefix(&self, last_dropped_id: OplogIndex) -> Result<u64, String> {
        self.record(format!("try_drop_prefix({last_dropped_id})"));
        Ok(last_dropped_id.as_u64())
    }

    async fn commit(
        &self,
        level: CommitLevel,
    ) -> Result<BTreeMap<OplogIndex, OplogEntry>, OplogError> {
        self.record(format!("commit({level:?})"));
        Ok(BTreeMap::from([(idx(8), jump(8))]))
    }

    async fn current_oplog_index(&self) -> OplogIndex {
        unreachable!()
    }

    async fn try_current_oplog_index(&self) -> Result<OplogIndex, String> {
        self.record("try_current_oplog_index".to_string());
        Ok(idx(12))
    }

    async fn last_added_non_hint_entry(&self) -> Option<OplogIndex> {
        self.record("last_added_non_hint_entry".to_string());
        Some(idx(11))
    }

    async fn wait_for_replicas(&self, replicas: u8, timeout: Duration) -> Result<bool, OplogError> {
        self.record(format!("wait_for_replicas({replicas}, {timeout:?})"));
        Ok(replicas > 1)
    }

    async fn read_exact(
        &self,
        _oplog_index: OplogIndex,
        _n: u64,
    ) -> Result<BTreeMap<OplogIndex, OplogEntry>, UnreadableOplog> {
        unreachable!()
    }

    async fn length(&self) -> Result<u64, UnreadableOplog> {
        unreachable!()
    }

    async fn upload_raw_payload(&self, _data: Vec<u8>) -> Result<RawOplogPayload, String> {
        unreachable!()
    }

    async fn download_raw_payload(
        &self,
        payload_id: PayloadId,
        md5_hash: Vec<u8>,
    ) -> Result<Vec<u8>, String> {
        self.record(format!(
            "download_raw_payload({payload_id:?}, {md5_hash:?})"
        ));
        Ok(md5_hash)
    }

    async fn download_raw_payload_classified(
        &self,
        payload_id: PayloadId,
        md5_hash: Vec<u8>,
    ) -> Result<Vec<u8>, RawOplogPayloadDownloadError> {
        self.record(format!(
            "download_raw_payload_classified({payload_id:?}, {md5_hash:?})"
        ));
        Ok(md5_hash)
    }

    async fn add_start_with_reserved_raw_payload(
        &self,
        _serialized_request: Vec<u8>,
        _build_start: ReservedRawStartBuilder,
    ) -> Result<OrderedOplogStart, OplogError> {
        unreachable!()
    }

    async fn add_start_with_indexed_reserved_raw_payload(
        &self,
        _build_request: IndexedReservedStartBuilder,
    ) -> Result<OrderedOplogStart, OplogError> {
        unreachable!()
    }

    fn enqueue_add_pair(
        &self,
        _start: OplogEntry,
        _make_second: Box<dyn FnOnce(OplogIndex) -> OplogEntry + Send>,
    ) -> OplogAddPairReceipt {
        unreachable!()
    }
}

/// An oplog service that records each call that reaches it, with its arguments, and answers
/// from the arguments, as [`RecordingOplog`] does for an open oplog. Its oplog of the agent
/// `owner` is at the shard epoch 5.
pub(crate) struct RecordingOplogService {
    calls: std::sync::Mutex<Vec<String>>,
    stream_session_index: std::sync::Mutex<Option<Arc<StreamSessionIndexService>>>,
    owner: OwnedAgentId,
}

impl RecordingOplogService {
    pub fn new(owner: OwnedAgentId) -> Self {
        Self {
            calls: Default::default(),
            stream_session_index: Default::default(),
            owner,
        }
    }

    fn record(&self, call: String) {
        self.calls.lock().unwrap().push(call);
    }

    /// The calls that reached the service since the last take, in their order.
    pub fn take_calls(&self) -> Vec<String> {
        std::mem::take(&mut self.calls.lock().unwrap())
    }
}

impl Debug for RecordingOplogService {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RecordingOplogService").finish()
    }
}

#[async_trait]
impl OplogService for RecordingOplogService {
    async fn lock_lifecycle(&self, _agent_id: &AgentId) -> OplogLifecycleGuard {
        unreachable!()
    }

    fn set_stream_session_index(&self, index: Arc<StreamSessionIndexService>) {
        self.record("set_stream_session_index".to_string());
        *self.stream_session_index.lock().unwrap() = Some(index);
    }

    fn stream_session_index(&self) -> Option<Arc<StreamSessionIndexService>> {
        self.stream_session_index.lock().unwrap().clone()
    }

    async fn staged_exists(
        &self,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
        stage_id: uuid::Uuid,
    ) -> Result<bool, String> {
        self.record(format!(
            "staged_exists({owned_agent_id}, {agent_mode:?}, {stage_id})"
        ));
        Ok(stage_id.as_u128() % 2 == 1)
    }

    async fn publish_staged(
        &self,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
        publication: StagePublication,
        expected_last_index: OplogIndex,
    ) -> Result<bool, String> {
        self.record(format!(
            "publish_staged({owned_agent_id}, {agent_mode:?}, {}, {expected_last_index})",
            publication.stage_id()
        ));
        Ok(expected_last_index > idx(5))
    }

    async fn discard_staged(
        &self,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
        stage_id: uuid::Uuid,
    ) -> Result<(), String> {
        self.record(format!(
            "discard_staged({owned_agent_id}, {agent_mode:?}, {stage_id})"
        ));
        Err(format!("no stage {stage_id}"))
    }

    async fn create(
        &self,
        _lifecycle: &mut OplogLifecycleGuard,
        _owned_agent_id: &OwnedAgentId,
        _agent_mode: AgentMode,
        _initial_entry: OplogEntry,
        _initial_worker_metadata: AgentMetadata,
        _last_known_status: read_only_lock::arc_swap::ReadOnlyView<AgentStatusRecord>,
        _execution_status: read_only_lock::std::ReadOnlyLock<ExecutionStatus>,
        _shard_epoch: Option<ShardEpoch>,
    ) -> Result<Arc<dyn Oplog>, WorkerExecutorError> {
        unreachable!()
    }

    async fn create_fresh(
        &self,
        _lifecycle: &mut OplogLifecycleGuard,
        _owned_agent_id: &OwnedAgentId,
        _agent_mode: AgentMode,
        _initial_entry: OplogEntry,
        _initial_worker_metadata: AgentMetadata,
        _last_known_status: read_only_lock::arc_swap::ReadOnlyView<AgentStatusRecord>,
        _execution_status: read_only_lock::std::ReadOnlyLock<ExecutionStatus>,
        _shard_epoch: Option<ShardEpoch>,
    ) -> Result<Arc<dyn Oplog>, WorkerExecutorError> {
        unreachable!()
    }

    async fn open(
        &self,
        _lifecycle: &mut OplogLifecycleGuard,
        _owned_agent_id: &OwnedAgentId,
        _agent_mode: AgentMode,
        _last_oplog_index: Option<OplogIndex>,
        _initial_worker_metadata: AgentMetadata,
        _last_known_status: read_only_lock::arc_swap::ReadOnlyView<AgentStatusRecord>,
        _execution_status: read_only_lock::std::ReadOnlyLock<ExecutionStatus>,
        _shard_epoch: Option<ShardEpoch>,
    ) -> Result<Arc<dyn Oplog>, WorkerExecutorError> {
        unreachable!()
    }

    async fn get_last_index(
        &self,
        _owned_agent_id: &OwnedAgentId,
        _agent_mode: AgentMode,
    ) -> Result<OplogIndex, UnreadableOplog> {
        unreachable!()
    }

    async fn try_get_last_index(
        &self,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
    ) -> Result<OplogIndex, String> {
        self.record(format!(
            "try_get_last_index({owned_agent_id}, {agent_mode:?})"
        ));
        Ok(idx(13))
    }

    async fn retention(
        &self,
        _owned_agent_id: &OwnedAgentId,
        _agent_mode: AgentMode,
    ) -> Result<OplogRetention, UnreadableOplog> {
        unreachable!()
    }

    async fn delete(
        &self,
        _lifecycle: &mut OplogLifecycleGuard,
        _owned_agent_id: &OwnedAgentId,
        _agent_mode: AgentMode,
        _expected_epoch: Option<ShardEpoch>,
    ) -> Result<(), OplogError> {
        unreachable!()
    }

    async fn assert_owning_epoch(
        &self,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
        expected_epoch: ShardEpoch,
    ) -> Result<(), OplogError> {
        self.record(format!(
            "assert_owning_epoch({owned_agent_id}, {agent_mode:?}, {expected_epoch:?})"
        ));
        if *owned_agent_id == self.owner && expected_epoch == ShardEpoch(5) {
            Ok(())
        } else {
            Err(OplogError::Fenced(OplogFence {
                agent_id: owned_agent_id.agent_id,
                expected_epoch,
                actual_epoch: Some(ShardEpoch(5)),
            }))
        }
    }

    async fn read_exact(
        &self,
        _owned_agent_id: &OwnedAgentId,
        _agent_mode: AgentMode,
        _idx: OplogIndex,
        _n: u64,
    ) -> Result<BTreeMap<OplogIndex, OplogEntry>, UnreadableOplog> {
        unreachable!()
    }

    async fn read_source(
        &self,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
        start: OplogIndex,
        n: u64,
    ) -> Result<BTreeMap<OplogIndex, OplogEntry>, UnreadableOplog> {
        self.record(format!(
            "read_source({owned_agent_id}, {agent_mode:?}, {start}, {n})"
        ));
        Ok((start.as_u64()..start.as_u64() + n)
            .map(|index| (idx(index), jump(index)))
            .collect())
    }

    async fn exists(
        &self,
        _owned_agent_id: &OwnedAgentId,
        _agent_mode: AgentMode,
    ) -> Result<bool, UnreadableOplog> {
        unreachable!()
    }

    async fn try_exists(
        &self,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
    ) -> Result<bool, String> {
        self.record(format!("try_exists({owned_agent_id}, {agent_mode:?})"));
        Ok(agent_mode == AgentMode::Durable)
    }

    async fn scan_for_type(
        &self,
        environment_id: &EnvironmentId,
        type_id: TypeId,
        modes: Option<AgentMode>,
        cursor: ScanCursor,
        count: u64,
    ) -> Result<ScanPage, WorkerExecutorError> {
        self.record(format!(
            "scan_for_type({environment_id:?}, {}, {modes:?}, {cursor:?}, {count})",
            type_id.get()
        ));
        Ok(ScanPage {
            cursor: ScanCursor::new(format!("after {}", type_id.get())),
            agent_ids: vec![self.owner.clone()],
            examined_keys: count,
        })
    }

    async fn upload_raw_payload(
        &self,
        _owned_agent_id: &OwnedAgentId,
        _agent_mode: AgentMode,
        _data: Vec<u8>,
    ) -> Result<RawOplogPayload, String> {
        unreachable!()
    }

    async fn download_raw_payload(
        &self,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
        payload_id: PayloadId,
        md5_hash: Vec<u8>,
    ) -> Result<Vec<u8>, String> {
        self.record(format!(
            "download_raw_payload({owned_agent_id}, {agent_mode:?}, {payload_id:?}, {md5_hash:?})"
        ));
        Ok(md5_hash)
    }

    async fn download_raw_payload_classified(
        &self,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
        payload_id: PayloadId,
        md5_hash: Vec<u8>,
    ) -> Result<Vec<u8>, RawOplogPayloadDownloadError> {
        self.record(format!(
            "download_raw_payload_classified({owned_agent_id}, {agent_mode:?}, {payload_id:?}, {md5_hash:?})"
        ));
        Ok(md5_hash)
    }
}
