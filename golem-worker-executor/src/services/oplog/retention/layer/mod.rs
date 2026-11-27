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
//! The layer sits directly on the base oplog service, and it is the only layer that reads the
//! retention record. Every read through the oplog stack goes through it, except the reads of a
//! staged oplog: `create_staged` gives the handle of the inner service. The layers below it move
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
use golem_common::model::durable_stream::StreamSessionKey;
use golem_common::model::environment::EnvironmentId;
use golem_common::model::oplog::{OplogEntry, OplogIndex, PayloadId, RawOplogPayload};
use golem_common::model::type_id::TypeId;
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
/// The handle keeps the start that it read at open. A later move of the record does not change
/// it. A handle of `create` or `create_fresh` reads no record and starts at index 1.
pub struct RetentionOplog {
    inner: Arc<dyn Oplog>,
    service: Arc<dyn OplogService>,
    owner: OwnedAgentId,
    mode: AgentMode,
    retention: OplogRetention,
}

impl RetentionOplog {
    pub(super) fn new(
        inner: Arc<dyn Oplog>,
        service: Arc<dyn OplogService>,
        owner: OwnedAgentId,
        mode: AgentMode,
        retention: OplogRetention,
    ) -> Self {
        Self {
            inner,
            service,
            owner,
            mode,
            retention,
        }
    }
}

/// Reads `n` entries from `idx` with `read`, by the rules of `retention`: the check of the read
/// comes first, and a failed read goes through [`OplogRetention::classify_unreadable`]. The open
/// handle and the service both read through it.
async fn read_retained<Fut>(
    retention: OplogRetention,
    agent_id: &AgentId,
    idx: OplogIndex,
    n: u64,
    read: Fut,
) -> Result<BTreeMap<OplogIndex, OplogEntry>, UnreadableOplog>
where
    Fut: Future<Output = Result<BTreeMap<OplogIndex, OplogEntry>, UnreadableOplog>>,
{
    retention.check_agent_read(agent_id, idx, n)?;
    read.await
        .map_err(|unreadable| retention.classify_unreadable(unreadable))
}

impl Debug for RetentionOplog {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RetentionOplog")
            .field("owner", &self.owner)
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
        read_retained(
            self.retention,
            &self.owner.agent_id,
            oplog_index,
            n,
            self.inner.read_exact(oplog_index, n),
        )
        .await
    }

    async fn read_source(
        &self,
        oplog_index: OplogIndex,
        n: u64,
    ) -> Result<BTreeMap<OplogIndex, OplogEntry>, UnreadableOplog> {
        read_retained(
            self.retention,
            &self.owner.agent_id,
            oplog_index,
            n,
            self.inner.read_source(oplog_index, n),
        )
        .await
    }

    async fn try_read_source(
        &self,
        oplog_index: OplogIndex,
        n: u64,
    ) -> Result<BTreeMap<OplogIndex, OplogEntry>, String> {
        self.retention
            .check_agent_read(&self.owner.agent_id, oplog_index, n)
            .map_err(|error| error.to_string())?;
        // The inner read gives the part of the range that its layer holds, so it reports no gap.
        self.inner.try_read_source(oplog_index, n).await
    }

    async fn length(&self) -> Result<u64, UnreadableOplog> {
        let length = self.inner.length().await?;
        if self.retention == OplogRetention::FULL {
            return Ok(length);
        }
        // An archive layer can count chunks, not entries. Use the committed tail for the count.
        let last = self
            .service
            .try_get_last_index(&self.owner, self.mode)
            .await?;
        Ok(self.retention.retained_length(last))
    }

    async fn try_length(&self) -> Result<u64, String> {
        let length = self.inner.try_length().await?;
        if self.retention == OplogRetention::FULL {
            return Ok(length);
        }
        let last = self
            .service
            .try_get_last_index(&self.owner, self.mode)
            .await
            .map_err(UnreadableOplog::into_tail_maintenance_error)?;
        Ok(self.retention.retained_length(last))
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

    fn retention(&self) -> OplogRetention {
        self.retention
    }

    fn inner(&self) -> Option<Arc<dyn Oplog>> {
        Some(self.inner.clone())
    }
}

/// The [`OplogService`] that reads the retention record of each agent oplog and keeps every read
/// at or after the retained start. `create` and `create_fresh` read no record, so the handle of a
/// new oplog starts at index 1. `open` and the reads of the service read the record, also a record
/// that an earlier incarnation of the agent left. The delete removes the record only after the
/// delete of the oplog succeeds (fenced for a durable oplog). When the fence refuses the delete, or
/// the delete fails, the record stays, and the oplog that stays keeps its start.
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

    /// Reads `n` entries from `idx` with `read`. A read of the `Create` entry alone and an empty
    /// read need the retained start only when they fail, so the usual identity read costs no
    /// record lookup. Every other read loads the record once, before the read.
    async fn read_with_retention<Fut>(
        &self,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
        idx: OplogIndex,
        n: u64,
        read: Fut,
    ) -> Result<BTreeMap<OplogIndex, OplogEntry>, UnreadableOplog>
    where
        Fut: Future<Output = Result<BTreeMap<OplogIndex, OplogEntry>, UnreadableOplog>>,
    {
        let agent_id = &owned_agent_id.agent_id;
        if OplogRetention::read_needs_start(idx, n) {
            let retention = self.retention(owned_agent_id, agent_mode).await?;
            read_retained(retention, agent_id, idx, n, read).await
        } else {
            match read.await {
                Ok(entries) => Ok(entries),
                // A failed read of the record keeps the error of the read, which is the cause.
                Err(unreadable) => Err(match self.retention(owned_agent_id, agent_mode).await {
                    Ok(retention) => retention.classify_unreadable(unreadable),
                    Err(_) => unreadable,
                }),
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
            self.inner.clone(),
            owned_agent_id.clone(),
            agent_mode,
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
            self.inner.clone(),
            owned_agent_id.clone(),
            agent_mode,
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
            self.inner.clone(),
            owned_agent_id.clone(),
            agent_mode,
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
    ) -> Result<OplogIndex, UnreadableOplog> {
        self.inner
            .try_get_last_index(owned_agent_id, agent_mode)
            .await
    }

    async fn retention(
        &self,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
    ) -> Result<OplogRetention, UnreadableOplog> {
        read_oplog_retention(self.key_value_storage.as_ref(), owned_agent_id, agent_mode).await
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
        self.inner
            .delete(lifecycle, owned_agent_id, agent_mode, expected_epoch)
            .await?;
        remove_oplog_retention(self.key_value_storage.as_ref(), owned_agent_id, agent_mode)
            .await
            .map_err(OplogError::Maintenance)
    }

    async fn read_exact(
        &self,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
        idx: OplogIndex,
        n: u64,
    ) -> Result<BTreeMap<OplogIndex, OplogEntry>, UnreadableOplog> {
        self.read_with_retention(
            owned_agent_id,
            agent_mode,
            idx,
            n,
            self.inner.read_exact(owned_agent_id, agent_mode, idx, n),
        )
        .await
    }

    async fn read_source(
        &self,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
        idx: OplogIndex,
        n: u64,
    ) -> Result<BTreeMap<OplogIndex, OplogEntry>, UnreadableOplog> {
        self.read_with_retention(
            owned_agent_id,
            agent_mode,
            idx,
            n,
            self.inner.read_source(owned_agent_id, agent_mode, idx, n),
        )
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

    async fn scan_for_type(
        &self,
        environment_id: &EnvironmentId,
        type_id: TypeId,
        modes: Option<AgentMode>,
        cursor: ScanCursor,
        count: u64,
    ) -> Result<ScanPage, WorkerExecutorError> {
        self.inner
            .scan_for_type(environment_id, type_id, modes, cursor, count)
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
mod tests;
