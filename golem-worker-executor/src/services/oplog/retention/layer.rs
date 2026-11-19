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

//! The oplog layer that keeps reads at or after the retained start.
//!
//! The layer sits directly on the base oplog service, so every read through the oplog stack goes
//! through it, and it is the only layer that reads the retention record. The layers below it move
//! entries between storage tiers with their own handles and do not see the retained start.

use super::{OplogRetention, read_oplog_retention, remove_oplog_retention};
use crate::model::ExecutionStatus;
use crate::services::oplog::{
    CommitLevel, DurableStreamBatchBuilder, IndexedReservedStartBuilder, Oplog,
    OplogAddPairReceipt, OplogAddReceipt, OplogCloseCompletion, OplogError, OplogLifecycleGuard,
    OplogService, OrderedOplogStart, RawDurableStreamSessionStatus, RawOplogPayloadDownloadError,
    ReservedRawStartBuilder, ScanPage, StagePublication, StreamSessionIndexService,
    UnreadableOplog, WorkerTasks,
};
use crate::storage::keyvalue::KeyValueStorage;
use async_trait::async_trait;
use golem_common::model::agent::AgentMode;
use golem_common::model::component::ComponentId;
use golem_common::model::durable_stream::StreamSessionKey;
use golem_common::model::environment::EnvironmentId;
use golem_common::model::oplog::{OplogEntry, OplogIndex, PayloadId, RawOplogPayload};
use golem_common::model::{
    AgentId, AgentMetadata, AgentStatusRecord, OwnedAgentId, ScanCursor, ShardEpoch,
};
use golem_common::read_only_lock;
use golem_service_base::error::worker_executor::WorkerExecutorError;
use std::collections::BTreeMap;
use std::fmt::{Debug, Formatter};
use std::sync::Arc;
use std::time::Duration;

/// An open oplog that refuses reads below its retained start.
///
/// The handle keeps the retention it was opened with. Nothing moves the start of an open oplog.
pub struct RetentionOplog {
    inner: Arc<dyn Oplog>,
    agent_id: AgentId,
    retention: OplogRetention,
}

impl RetentionOplog {
    pub fn new(inner: Arc<dyn Oplog>, agent_id: AgentId, retention: OplogRetention) -> Self {
        Self {
            inner,
            agent_id,
            retention,
        }
    }

    fn checked(&self, oplog_index: OplogIndex, n: u64) -> Result<(), UnreadableOplog> {
        self.retention
            .check_read(oplog_index, n)
            .map_err(|error| UnreadableOplog::new(&self.agent_id, error))
    }

    fn classified(&self, unreadable: UnreadableOplog) -> UnreadableOplog {
        UnreadableOplog::new(&self.agent_id, self.retention.classify(unreadable.error))
    }
}

impl Debug for RetentionOplog {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RetentionOplog")
            .field("agent_id", &self.agent_id)
            .field("retention", &self.retention)
            .finish()
    }
}

#[async_trait]
impl Oplog for RetentionOplog {
    fn retire(&self) {
        self.inner.retire();
    }

    fn is_retired(&self) -> bool {
        self.inner.is_retired()
    }

    fn closed(&self) -> OplogCloseCompletion {
        self.inner.closed()
    }

    fn task_owner(&self) -> Option<&WorkerTasks> {
        self.inner.task_owner()
    }

    fn enqueue_add(&self, entry: OplogEntry) -> OplogAddReceipt {
        self.inner.enqueue_add(entry)
    }

    async fn add_durable_stream_batch(
        &self,
        make_batch: DurableStreamBatchBuilder,
    ) -> Result<Vec<(OplogIndex, OplogEntry)>, OplogError> {
        self.inner.add_durable_stream_batch(make_batch).await
    }

    async fn drop_prefix(&self, last_dropped_id: OplogIndex) -> u64 {
        self.inner.drop_prefix(last_dropped_id).await
    }

    async fn try_drop_prefix(&self, last_dropped_id: OplogIndex) -> Result<u64, String> {
        self.inner.try_drop_prefix(last_dropped_id).await
    }

    async fn commit(
        &self,
        level: CommitLevel,
    ) -> Result<BTreeMap<OplogIndex, OplogEntry>, OplogError> {
        self.inner.commit(level).await
    }

    async fn current_oplog_index(&self) -> OplogIndex {
        self.inner.current_oplog_index().await
    }

    async fn try_current_oplog_index(&self) -> Result<OplogIndex, String> {
        self.inner.try_current_oplog_index().await
    }

    async fn raw_durable_stream_session_status(
        &self,
        session_key: &StreamSessionKey,
    ) -> RawDurableStreamSessionStatus {
        self.inner
            .raw_durable_stream_session_status(session_key)
            .await
    }

    async fn last_added_non_hint_entry(&self) -> Option<OplogIndex> {
        self.inner.last_added_non_hint_entry().await
    }

    async fn wait_for_replicas(&self, replicas: u8, timeout: Duration) -> Result<bool, OplogError> {
        self.inner.wait_for_replicas(replicas, timeout).await
    }

    async fn read_exact(
        &self,
        oplog_index: OplogIndex,
        n: u64,
    ) -> Result<BTreeMap<OplogIndex, OplogEntry>, UnreadableOplog> {
        self.checked(oplog_index, n)?;
        self.inner
            .read_exact(oplog_index, n)
            .await
            .map_err(|error| self.classified(error))
    }

    async fn read_source(
        &self,
        oplog_index: OplogIndex,
        n: u64,
    ) -> Result<BTreeMap<OplogIndex, OplogEntry>, UnreadableOplog> {
        self.checked(oplog_index, n)?;
        self.inner
            .read_source(oplog_index, n)
            .await
            .map_err(|error| self.classified(error))
    }

    async fn try_read_source(
        &self,
        oplog_index: OplogIndex,
        n: u64,
    ) -> Result<BTreeMap<OplogIndex, OplogEntry>, String> {
        self.checked(oplog_index, n)
            .map_err(|error| error.to_string())?;
        self.inner.try_read_source(oplog_index, n).await
    }

    async fn length(&self) -> Result<u64, UnreadableOplog> {
        Ok(self
            .retention
            .retained_length(self.inner.current_oplog_index().await))
    }

    async fn try_length(&self) -> Result<u64, String> {
        Ok(self
            .retention
            .retained_length(self.inner.try_current_oplog_index().await?))
    }

    async fn upload_raw_payload(&self, data: Vec<u8>) -> Result<RawOplogPayload, String> {
        self.inner.upload_raw_payload(data).await
    }

    async fn download_raw_payload(
        &self,
        payload_id: PayloadId,
        md5_hash: Vec<u8>,
    ) -> Result<Vec<u8>, String> {
        self.inner.download_raw_payload(payload_id, md5_hash).await
    }

    async fn download_raw_payload_classified(
        &self,
        payload_id: PayloadId,
        md5_hash: Vec<u8>,
    ) -> Result<Vec<u8>, RawOplogPayloadDownloadError> {
        self.inner
            .download_raw_payload_classified(payload_id, md5_hash)
            .await
    }

    async fn add_start_with_reserved_raw_payload(
        &self,
        serialized_request: Vec<u8>,
        build_start: ReservedRawStartBuilder,
    ) -> Result<OrderedOplogStart, OplogError> {
        self.inner
            .add_start_with_reserved_raw_payload(serialized_request, build_start)
            .await
    }

    async fn add_start_with_indexed_reserved_raw_payload(
        &self,
        build_request: IndexedReservedStartBuilder,
    ) -> Result<OrderedOplogStart, OplogError> {
        self.inner
            .add_start_with_indexed_reserved_raw_payload(build_request)
            .await
    }

    fn enqueue_add_pair(
        &self,
        start: OplogEntry,
        make_second: Box<dyn FnOnce(OplogIndex) -> OplogEntry + Send>,
    ) -> OplogAddPairReceipt {
        self.inner.enqueue_add_pair(start, make_second)
    }

    fn first_retained_index(&self) -> OplogIndex {
        self.retention.first_index()
    }

    fn inner(&self) -> Option<Arc<dyn Oplog>> {
        Some(self.inner.clone())
    }
}

/// The [`OplogService`] that reads the retention record of each agent oplog and keeps every read
/// at or after the retained start. A new oplog keeps every entry. Deleting an oplog removes its
/// record first, so a re-created agent never starts with the start of the deleted one.
pub struct RetentionOplogService {
    inner: Arc<dyn OplogService>,
    key_value_storage: Arc<dyn KeyValueStorage + Send + Sync>,
}

impl RetentionOplogService {
    pub fn new(
        inner: Arc<dyn OplogService>,
        key_value_storage: Arc<dyn KeyValueStorage + Send + Sync>,
    ) -> Self {
        Self {
            inner,
            key_value_storage,
        }
    }

    async fn retention(
        &self,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
    ) -> Result<OplogRetention, UnreadableOplog> {
        read_oplog_retention(
            self.key_value_storage.as_ref(),
            &owned_agent_id.agent_id,
            agent_mode,
        )
        .await
    }

    /// Reads through `read` with the retention rules. A read of the `Create` entry alone needs
    /// the retained start only when it fails, so the usual identity read costs no record lookup.
    async fn read_retained<F, Fut>(
        &self,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
        idx: OplogIndex,
        n: u64,
        read: F,
    ) -> Result<BTreeMap<OplogIndex, OplogEntry>, UnreadableOplog>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<BTreeMap<OplogIndex, OplogEntry>, UnreadableOplog>>,
    {
        let agent_id = &owned_agent_id.agent_id;
        if !(idx == OplogIndex::INITIAL && n == 1) {
            self.retention(owned_agent_id, agent_mode)
                .await?
                .check_read(idx, n)
                .map_err(|error| UnreadableOplog::new(agent_id, error))?;
        }
        match read().await {
            Ok(entries) => Ok(entries),
            Err(unreadable) => {
                let retention = self.retention(owned_agent_id, agent_mode).await?;
                Err(UnreadableOplog::new(
                    agent_id,
                    retention.classify(unreadable.error),
                ))
            }
        }
    }
}

impl Debug for RetentionOplogService {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RetentionOplogService").finish()
    }
}

#[async_trait]
impl OplogService for RetentionOplogService {
    async fn lock_lifecycle(&self, agent_id: &AgentId) -> OplogLifecycleGuard {
        self.inner.lock_lifecycle(agent_id).await
    }

    fn set_stream_session_index(&self, index: Arc<StreamSessionIndexService>) {
        self.inner.set_stream_session_index(index);
    }

    fn stream_session_index(&self) -> Option<Arc<StreamSessionIndexService>> {
        self.inner.stream_session_index()
    }

    async fn create_staged(
        &self,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
        stage_id: uuid::Uuid,
        initial_worker_metadata: AgentMetadata,
    ) -> Result<Arc<dyn Oplog>, String> {
        self.inner
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
        self.inner
            .staged_exists(owned_agent_id, agent_mode, stage_id)
            .await
    }

    async fn publish_staged(
        &self,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
        publication: StagePublication,
        expected_last_index: OplogIndex,
    ) -> Result<bool, String> {
        self.inner
            .publish_staged(owned_agent_id, agent_mode, publication, expected_last_index)
            .await
    }

    async fn discard_staged(
        &self,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
        stage_id: uuid::Uuid,
    ) -> Result<(), String> {
        self.inner
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
        shard_epoch: Option<ShardEpoch>,
    ) -> Result<Arc<dyn Oplog>, WorkerExecutorError> {
        let inner = self
            .inner
            .create(
                lifecycle,
                owned_agent_id,
                agent_mode,
                initial_entry,
                initial_worker_metadata,
                last_known_status,
                execution_status,
                shard_epoch,
            )
            .await?;
        Ok(Arc::new(RetentionOplog::new(
            inner,
            owned_agent_id.agent_id.clone(),
            OplogRetention::FULL,
        )))
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
        shard_epoch: Option<ShardEpoch>,
    ) -> Result<Arc<dyn Oplog>, WorkerExecutorError> {
        let inner = self
            .inner
            .create_fresh(
                lifecycle,
                owned_agent_id,
                agent_mode,
                initial_entry,
                initial_worker_metadata,
                last_known_status,
                execution_status,
                shard_epoch,
            )
            .await?;
        Ok(Arc::new(RetentionOplog::new(
            inner,
            owned_agent_id.agent_id.clone(),
            OplogRetention::FULL,
        )))
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
        shard_epoch: Option<ShardEpoch>,
    ) -> Result<Arc<dyn Oplog>, WorkerExecutorError> {
        let retention = self.retention(owned_agent_id, agent_mode).await?;
        let inner = self
            .inner
            .open(
                lifecycle,
                owned_agent_id,
                agent_mode,
                last_oplog_index,
                initial_worker_metadata,
                last_known_status,
                execution_status,
                shard_epoch,
            )
            .await?;
        Ok(Arc::new(RetentionOplog::new(
            inner,
            owned_agent_id.agent_id.clone(),
            retention,
        )))
    }

    async fn get_last_index(
        &self,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
    ) -> Result<OplogIndex, UnreadableOplog> {
        self.inner.get_last_index(owned_agent_id, agent_mode).await
    }

    async fn try_get_last_index(
        &self,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
    ) -> Result<OplogIndex, String> {
        self.inner
            .try_get_last_index(owned_agent_id, agent_mode)
            .await
    }

    async fn first_retained_index(
        &self,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
    ) -> Result<OplogIndex, UnreadableOplog> {
        Ok(self
            .retention(owned_agent_id, agent_mode)
            .await?
            .first_index())
    }

    async fn assert_owning_epoch(
        &self,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
        expected_epoch: ShardEpoch,
    ) -> Result<(), OplogError> {
        self.inner
            .assert_owning_epoch(owned_agent_id, agent_mode, expected_epoch)
            .await
    }

    async fn delete(
        &self,
        lifecycle: &mut OplogLifecycleGuard,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
        expected_epoch: Option<ShardEpoch>,
    ) -> Result<(), OplogError> {
        remove_oplog_retention(
            self.key_value_storage.as_ref(),
            &owned_agent_id.agent_id,
            agent_mode,
        )
        .await
        .map_err(OplogError::Maintenance)?;
        self.inner
            .delete(lifecycle, owned_agent_id, agent_mode, expected_epoch)
            .await
    }

    async fn read_exact(
        &self,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
        idx: OplogIndex,
        n: u64,
    ) -> Result<BTreeMap<OplogIndex, OplogEntry>, UnreadableOplog> {
        self.read_retained(owned_agent_id, agent_mode, idx, n, || {
            self.inner.read_exact(owned_agent_id, agent_mode, idx, n)
        })
        .await
    }

    async fn read_source(
        &self,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
        idx: OplogIndex,
        n: u64,
    ) -> Result<BTreeMap<OplogIndex, OplogEntry>, UnreadableOplog> {
        self.read_retained(owned_agent_id, agent_mode, idx, n, || {
            self.inner.read_source(owned_agent_id, agent_mode, idx, n)
        })
        .await
    }

    async fn exists(
        &self,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
    ) -> Result<bool, UnreadableOplog> {
        self.inner.exists(owned_agent_id, agent_mode).await
    }

    async fn try_exists(
        &self,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
    ) -> Result<bool, String> {
        self.inner.try_exists(owned_agent_id, agent_mode).await
    }

    async fn scan_for_component(
        &self,
        environment_id: &EnvironmentId,
        component_id: &ComponentId,
        modes: Option<AgentMode>,
        cursor: ScanCursor,
        count: u64,
    ) -> Result<ScanPage, WorkerExecutorError> {
        self.inner
            .scan_for_component(environment_id, component_id, modes, cursor, count)
            .await
    }

    async fn upload_raw_payload(
        &self,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
        data: Vec<u8>,
    ) -> Result<RawOplogPayload, String> {
        self.inner
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
        self.inner
            .download_raw_payload(owned_agent_id, agent_mode, payload_id, md5_hash)
            .await
    }

    async fn download_raw_payload_classified(
        &self,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
        payload_id: PayloadId,
        md5_hash: Vec<u8>,
    ) -> Result<Vec<u8>, RawOplogPayloadDownloadError> {
        self.inner
            .download_raw_payload_classified(owned_agent_id, agent_mode, payload_id, md5_hash)
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::oplog::OplogReadError;
    use crate::services::oplog::PrimaryOplogService;
    use crate::services::oplog::retention::advance_oplog_retention;
    use crate::storage::indexed::memory::InMemoryIndexedStorage;
    use crate::storage::keyvalue::memory::InMemoryKeyValueStorage;
    use golem_common::model::account::{AccountEmail, AccountId};
    use golem_common::model::agent::OwnerKind;
    use golem_common::model::oplog::{CreateRequest, CreateRequestKind};
    use golem_common::model::regions::OplogRegion;
    use golem_common::model::{AgentFingerprint, Timestamp};
    use golem_service_base::storage::blob::memory::InMemoryBlobStorage;
    use std::sync::RwLock;
    use test_r::test;

    struct Fixture {
        service: RetentionOplogService,
        key_value_storage: Arc<dyn KeyValueStorage + Send + Sync>,
        owned_agent_id: OwnedAgentId,
    }

    fn idx(value: u64) -> OplogIndex {
        OplogIndex::from_u64(value)
    }

    async fn fixture() -> Fixture {
        let key_value_storage: Arc<dyn KeyValueStorage + Send + Sync> =
            Arc::new(InMemoryKeyValueStorage::new());
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
        let agent_id = AgentId {
            component_id: ComponentId::new(),
            agent_id: "retention".to_string(),
        };
        Fixture {
            service: RetentionOplogService::new(Arc::new(primary), key_value_storage.clone()),
            key_value_storage,
            owned_agent_id: OwnedAgentId::new(EnvironmentId::new(), &agent_id),
        }
    }

    impl Fixture {
        async fn open(&self) -> Arc<dyn Oplog> {
            let agent_id = self.owned_agent_id.agent_id.clone();
            let metadata = AgentMetadata {
                component_id: agent_id.component_id,
                agent_type_name: None,
                canonical_key: None,
                agent_id: agent_id.clone(),
                owner_kind: OwnerKind::ComponentAgent,
                env: vec![],
                environment_id: self.owned_agent_id.environment_id,
                created_by: AccountId::new(),
                created_by_email: AccountEmail::new("test@golem"),
                config: Vec::new(),
                created_at: Timestamp::now_utc(),
                parent: None,
                last_known_status: AgentStatusRecord::default(),
                original_phantom_id: None,
                fingerprint: AgentFingerprint::new(),
                agent_mode: AgentMode::Durable,
                create_request: CreateRequest::new(
                    CreateRequestKind::GetOrCreate,
                    None,
                    golem_common::model::agent::Principal::anonymous(),
                    &golem_common::model::invocation_context::InvocationContextStack::fresh_rounded(
                    ),
                ),
            };
            self.service
                .open(
                    &mut self.service.lock_lifecycle(&agent_id).await,
                    &self.owned_agent_id,
                    AgentMode::Durable,
                    None,
                    metadata,
                    read_only_lock::arc_swap::ReadOnlyView::new(Arc::new(
                        arc_swap::ArcSwap::from_pointee(AgentStatusRecord::default()),
                    )),
                    read_only_lock::std::ReadOnlyLock::new(Arc::new(RwLock::new(
                        ExecutionStatus::Suspended {
                            agent_mode: AgentMode::Durable,
                            timestamp: Timestamp::now_utc(),
                        },
                    ))),
                    None,
                )
                .await
                .unwrap()
        }

        /// An oplog with the entries 1 to `last`, each a jump that names its own index.
        async fn with_entries(&self, last: u64) {
            let oplog = self.open().await;
            for index in 1..=last {
                oplog.add(entry(index)).await.unwrap();
            }
            oplog.commit(CommitLevel::Always).await.unwrap();
        }

        async fn advance(&self, to: u64) -> OplogRetention {
            advance_oplog_retention(
                self.key_value_storage.as_ref(),
                &self.owned_agent_id.agent_id,
                AgentMode::Durable,
                idx(to),
            )
            .await
            .unwrap()
        }

        async fn service_read(
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

    /// An entry with a fixed timestamp, because the oplog stores timestamps in milliseconds.
    fn entry(index: u64) -> OplogEntry {
        OplogEntry::Jump {
            timestamp: golem_common::model::Timestamp::from(index),
            entity_parent_start_index: None,
            jump: OplogRegion {
                start: idx(index),
                end: idx(index),
            },
        }
    }

    fn expired(start: u64) -> OplogReadError {
        OplogReadError::Expired {
            retained_start: idx(start),
        }
    }

    #[test]
    async fn the_retained_start_survives_a_reopen_and_never_moves_back() {
        let fixture = fixture().await;
        fixture.with_entries(10).await;
        assert_eq!(fixture.open().await.first_retained_index(), idx(1));

        assert_eq!(fixture.advance(5).await.first_index(), idx(5));
        assert_eq!(fixture.advance(3).await.first_index(), idx(5));

        assert_eq!(fixture.open().await.first_retained_index(), idx(5));
        assert_eq!(
            fixture
                .service
                .first_retained_index(&fixture.owned_agent_id, AgentMode::Durable)
                .await
                .unwrap(),
            idx(5)
        );
    }

    #[test]
    async fn the_length_counts_the_retained_entries_and_the_last_index_stays() {
        let fixture = fixture().await;
        fixture.with_entries(10).await;
        fixture.advance(4).await;

        let oplog = fixture.open().await;
        assert_eq!(oplog.length().await.unwrap(), 7);
        assert_eq!(oplog.current_oplog_index().await, idx(10));
        assert_eq!(
            fixture
                .service
                .get_last_index(&fixture.owned_agent_id, AgentMode::Durable)
                .await
                .unwrap(),
            idx(10)
        );
    }

    #[test]
    async fn a_read_below_the_start_is_expired_and_the_create_entry_stays_readable() {
        let fixture = fixture().await;
        fixture.with_entries(10).await;
        fixture.advance(4).await;
        let oplog = fixture.open().await;

        assert_eq!(
            oplog.read_exact(idx(3), 2).await.unwrap_err().error,
            expired(4)
        );
        assert_eq!(
            oplog.read_source(idx(2), 5).await.unwrap_err().error,
            expired(4)
        );
        assert_eq!(fixture.service_read(1, 4).await.unwrap_err(), expired(4));
        assert_eq!(
            oplog
                .read_exact(idx(4), 7)
                .await
                .unwrap()
                .into_iter()
                .collect::<Vec<_>>(),
            (4..=10)
                .map(|index| (idx(index), entry(index)))
                .collect::<Vec<_>>()
        );
        assert_eq!(
            oplog.read(idx(1)).await.unwrap(),
            entry(1),
            "the entry at index 1 is kept below the start"
        );
        assert_eq!(
            fixture.service_read(1, 1).await.unwrap().get(&idx(1)),
            Some(&entry(1))
        );
    }

    #[test]
    async fn only_a_gap_below_the_start_is_expired() {
        let fixture = fixture().await;
        fixture.with_entries(10).await;
        fixture.open().await.drop_prefix(idx(5)).await;
        fixture.advance(4).await;

        let gap = fixture.service_read(4, 3).await.unwrap_err();
        assert!(
            matches!(gap, OplogReadError::Gap { start, .. } if start == idx(4)),
            "{gap:?}"
        );

        fixture.advance(6).await;
        assert_eq!(fixture.service_read(1, 1).await.unwrap_err(), expired(6));
    }

    #[test]
    async fn deleting_the_oplog_removes_its_retained_start() {
        let fixture = fixture().await;
        fixture.with_entries(3).await;
        fixture.advance(2).await;

        fixture
            .service
            .delete(
                &mut fixture
                    .service
                    .lock_lifecycle(&fixture.owned_agent_id.agent_id)
                    .await,
                &fixture.owned_agent_id,
                AgentMode::Durable,
                None,
            )
            .await
            .unwrap();

        assert_eq!(
            fixture
                .service
                .first_retained_index(&fixture.owned_agent_id, AgentMode::Durable)
                .await
                .unwrap(),
            idx(1)
        );
    }
}
