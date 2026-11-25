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
/// The handle keeps the retention that the service read when it opened the oplog. Nothing moves
/// the start of an open oplog. The handle keeps the [`AgentId`] for the error of a refused read.
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
        read_retained(
            self.retention,
            &self.agent_id,
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
            &self.agent_id,
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
            .check_agent_read(&self.agent_id, oplog_index, n)
            .map_err(|error| error.to_string())?;
        // The inner read gives the part of the range that its layer holds, so it reports no gap.
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

    fn retention(&self) -> OplogRetention {
        self.retention
    }

    fn inner(&self) -> Option<Arc<dyn Oplog>> {
        Some(self.inner.clone())
    }
}

/// The [`OplogService`] that reads the retention record of each agent oplog and keeps every read
/// at or after the retained start. A new oplog keeps every entry. The delete removes the record
/// only after the fenced delete of the oplog succeeds. When the fence refuses the delete, or the
/// delete fails, the record stays, and the oplog that stays keeps its start.
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
            owned_agent_id.agent_id,
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
            owned_agent_id.agent_id,
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
            owned_agent_id.agent_id,
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
mod tests {
    use super::*;
    use crate::services::oplog::OplogReadError;
    use crate::services::oplog::retention::test_oplog::{RetainedOplog, expired, idx, jump};
    use test_r::test;

    #[test]
    async fn the_retained_start_survives_a_reopen_and_never_moves_back() {
        let oplog = RetainedOplog::new().await;
        oplog.with_jumps(10).await;
        assert_eq!(oplog.open().await.retention(), OplogRetention::FULL);

        assert_eq!(oplog.advance(5).await.start(), idx(5));
        assert_eq!(oplog.advance(3).await.start(), idx(5));

        assert_eq!(oplog.open().await.retention().start(), idx(5));
        assert_eq!(oplog.start().await, idx(5));
    }

    /// Each agent mode has its own start. An ephemeral start survives a reopen of the ephemeral
    /// oplog next to the durable start of the same agent.
    #[test]
    async fn each_agent_mode_keeps_its_own_start_through_a_reopen() {
        let oplog = RetainedOplog::new().await;
        assert_eq!(
            oplog.advance_in(AgentMode::Ephemeral, 6).await.start(),
            idx(6)
        );
        assert_eq!(
            oplog.advance_in(AgentMode::Durable, 4).await.start(),
            idx(4)
        );

        assert_eq!(
            oplog
                .open_in(AgentMode::Ephemeral)
                .await
                .retention()
                .start(),
            idx(6)
        );
        assert_eq!(
            oplog.open_in(AgentMode::Durable).await.retention().start(),
            idx(4)
        );
        let stored = |agent_mode| oplog.service.retention(&oplog.owned_agent_id, agent_mode);
        assert_eq!(stored(AgentMode::Ephemeral).await.unwrap().start(), idx(6));
        assert_eq!(stored(AgentMode::Durable).await.unwrap().start(), idx(4));
    }

    #[test]
    async fn the_length_counts_the_retained_entries_and_the_last_index_stays() {
        let oplog = RetainedOplog::new().await;
        oplog.with_jumps(10).await;
        // The entries below the start are gone, so the length of the stored entries is not the
        // last index.
        oplog.open().await.drop_prefix(idx(3)).await;
        oplog.advance(4).await;

        let handle = oplog.open().await;
        assert_eq!(handle.length().await.unwrap(), 7);
        assert_eq!(handle.try_length().await.unwrap(), 7);
        assert_eq!(handle.current_oplog_index().await, idx(10));
        assert_eq!(
            oplog
                .service
                .get_last_index(&oplog.owned_agent_id, AgentMode::Durable)
                .await
                .unwrap(),
            idx(10)
        );
    }

    #[test]
    async fn a_read_below_the_start_is_expired_and_the_create_entry_stays_readable() {
        let oplog = RetainedOplog::new().await;
        oplog.with_jumps(10).await;
        oplog.advance(4).await;
        let handle = oplog.open().await;

        assert_eq!(
            handle.read_exact(idx(3), 2).await.unwrap_err().error,
            expired(4)
        );
        assert_eq!(
            handle.read_source(idx(2), 5).await.unwrap_err().error,
            expired(4)
        );
        let refused = handle.try_read_source(idx(3), 2).await.unwrap_err();
        assert!(refused.contains(&expired(4).to_string()), "{refused}");
        assert_eq!(oplog.read(1, 4).await.unwrap_err(), expired(4));
        assert_eq!(oplog.read(3, 1).await.unwrap_err(), expired(4));
        assert_eq!(
            handle
                .read_exact(idx(4), 7)
                .await
                .unwrap()
                .into_iter()
                .collect::<Vec<_>>(),
            (4..=10)
                .map(|index| (idx(index), jump(index)))
                .collect::<Vec<_>>()
        );
        assert_eq!(
            handle
                .try_read_source(idx(4), 1)
                .await
                .unwrap()
                .get(&idx(4)),
            Some(&jump(4))
        );
        assert_eq!(
            handle.read(idx(1)).await.unwrap(),
            jump(1),
            "the entry at index 1 is kept below the start"
        );
        assert_eq!(oplog.read(1, 1).await.unwrap().get(&idx(1)), Some(&jump(1)));
    }

    #[test]
    async fn only_a_gap_that_ends_below_the_start_is_expired() {
        let oplog = RetainedOplog::new().await;
        oplog.with_jumps(10).await;
        oplog.open().await.drop_prefix(idx(5)).await;
        let gap = oplog.read(1, 1).await.unwrap_err();
        assert!(
            matches!(gap, OplogReadError::Gap { start, .. } if start == idx(1)),
            "with the start at index 1 a missing `Create` entry stays a gap: {gap:?}"
        );
        oplog.advance(4).await;

        let gap = oplog.read(4, 3).await.unwrap_err();
        assert!(
            matches!(gap, OplogReadError::Gap { start, end } if start == idx(4) && end == idx(5)),
            "{gap:?}"
        );
        let gap = oplog
            .open()
            .await
            .read_exact(idx(4), 3)
            .await
            .unwrap_err()
            .error;
        assert!(matches!(gap, OplogReadError::Gap { .. }), "{gap:?}");

        oplog.advance(6).await;
        assert_eq!(oplog.read(1, 1).await.unwrap_err(), expired(6));
        assert_eq!(
            oplog.open().await.read(idx(1)).await.unwrap_err().error,
            expired(6)
        );
    }

    #[test]
    async fn a_refused_delete_keeps_the_retained_start_and_a_delete_removes_it() {
        let oplog = RetainedOplog::new().await;
        let handle = oplog.create(jump(1), ShardEpoch(5)).await;
        handle.add_and_commit(jump(2)).await.unwrap();
        drop(handle);
        oplog.advance(2).await;

        let refused = oplog.delete(Some(ShardEpoch(4))).await;
        assert!(matches!(refused, Err(OplogError::Fenced(_))), "{refused:?}");
        assert_eq!(
            oplog.start().await,
            idx(2),
            "a refused delete keeps the start"
        );
        assert!(
            oplog
                .service
                .exists(&oplog.owned_agent_id, AgentMode::Durable)
                .await
                .unwrap()
        );

        oplog.delete(Some(ShardEpoch(5))).await.unwrap();
        assert_eq!(oplog.start().await, OplogIndex::INITIAL);
        assert!(
            !oplog
                .service
                .exists(&oplog.owned_agent_id, AgentMode::Durable)
                .await
                .unwrap()
        );
    }

    /// A failed read of the `Create` entry alone needs the record to classify its error. When the
    /// read of the record fails too, the error of the read stays, because it is the cause.
    #[test]
    async fn a_failed_record_read_keeps_the_error_of_the_read() {
        use crate::storage::keyvalue::KeyValueStorageError;
        use crate::storage::keyvalue::fault_injecting::{
            FaultInjectingKeyValueStorage, KeyValueStorageFaults,
        };
        use crate::storage::keyvalue::memory::InMemoryKeyValueStorage;

        let faults = KeyValueStorageFaults::default();
        let oplog =
            RetainedOplog::with_key_value_storage(Arc::new(FaultInjectingKeyValueStorage::new(
                Arc::new(InMemoryKeyValueStorage::new()),
                faults.clone(),
            )))
            .await;
        oplog.with_jumps(10).await;
        oplog.open().await.drop_prefix(idx(5)).await;
        let calls = faults.calls("read_retention");
        faults.fail(
            "read_retention",
            1,
            KeyValueStorageError::Other("unavailable".to_string()),
        );

        let gap = oplog.read(1, 1).await.unwrap_err();
        assert!(
            matches!(gap, OplogReadError::Gap { start, .. } if start == idx(1)),
            "{gap:?}"
        );
        assert_eq!(faults.calls("read_retention"), calls + 1);
    }

    /// A delete whose removal of the retention record fails reports the failure, so the caller
    /// does not take the delete as complete while the record keeps the old start.
    #[test]
    async fn a_failed_removal_of_the_retention_record_fails_the_delete() {
        use crate::storage::keyvalue::KeyValueStorageError;
        use crate::storage::keyvalue::fault_injecting::{
            FaultInjectingKeyValueStorage, KeyValueStorageFaults,
        };
        use crate::storage::keyvalue::memory::InMemoryKeyValueStorage;

        let faults = KeyValueStorageFaults::default();
        let oplog =
            RetainedOplog::with_key_value_storage(Arc::new(FaultInjectingKeyValueStorage::new(
                Arc::new(InMemoryKeyValueStorage::new()),
                faults.clone(),
            )))
            .await;
        let handle = oplog.create(jump(1), ShardEpoch(5)).await;
        handle.add_and_commit(jump(2)).await.unwrap();
        drop(handle);
        oplog.advance(2).await;
        faults.fail(
            "remove_retention",
            1,
            KeyValueStorageError::Other("unavailable".to_string()),
        );

        let failed = oplog.delete(Some(ShardEpoch(5))).await;
        assert!(
            matches!(failed, Err(OplogError::Maintenance(_))),
            "{failed:?}"
        );
        assert_eq!(faults.calls("remove_retention"), 1);
        assert_eq!(oplog.start().await, idx(2), "the record keeps the start");
    }

    /// The retained range read of the handle and of the service reads the retained entries from
    /// the retained start to the last index, and reads nothing when the last index is below the
    /// start. The deletion scan and the consumer probe read through it.
    #[test]
    async fn a_retained_range_read_reads_only_the_retained_entries() {
        use crate::services::oplog::{OplogOps, OplogServiceOps};

        let oplog = RetainedOplog::new().await;
        oplog.with_jumps(6).await;
        oplog.advance(4).await;
        let keys = |entries: BTreeMap<OplogIndex, OplogEntry>| {
            entries
                .into_keys()
                .map(|index| index.as_u64())
                .collect::<Vec<_>>()
        };
        let handle = oplog.open().await;
        let service = |last: u64| {
            oplog
                .service
                .read_retained_range(&oplog.owned_agent_id, AgentMode::Durable, idx(last))
        };

        assert_eq!(
            keys(handle.read_retained_range(idx(6)).await.unwrap()),
            vec![4, 5, 6]
        );
        assert_eq!(
            keys(handle.read_retained_range(idx(4)).await.unwrap()),
            vec![4]
        );
        assert_eq!(
            keys(handle.read_retained_range(idx(3)).await.unwrap()),
            Vec::<u64>::new()
        );
        assert_eq!(keys(service(6).await.unwrap()), vec![4, 5, 6]);
        assert_eq!(keys(service(4).await.unwrap()), vec![4]);
        assert_eq!(keys(service(3).await.unwrap()), Vec::<u64>::new());
    }

    /// Every call of an open oplog that the layer does not check reaches the inner oplog with the
    /// same arguments, and its answer comes back unchanged.
    #[test]
    async fn the_retained_oplog_forwards_every_other_call_to_the_inner_oplog() {
        use crate::services::oplog::retention::test_oplog::{RecordingOplog, at};

        let inner = Arc::new(RecordingOplog::default());
        let handle = RetentionOplog::new(inner.clone(), AgentId::from_parts(7, 1), at(4));
        let payload_id = PayloadId(uuid::Uuid::from_u128(9));

        assert!(!handle.is_retired());
        handle.retire();
        assert!(inner.is_retired());
        assert!(handle.is_retired());
        assert!(std::ptr::eq(handle.task_owner().unwrap(), &inner.tasks));
        assert_eq!(
            handle
                .add_durable_stream_batch(Box::new(|_| Vec::new()))
                .await
                .unwrap(),
            vec![(idx(21), jump(21))]
        );
        assert_eq!(handle.try_drop_prefix(idx(6)).await, Ok(6));
        assert_eq!(
            handle.commit(CommitLevel::Always).await.unwrap(),
            BTreeMap::from([(idx(8), jump(8))])
        );
        assert_eq!(handle.try_current_oplog_index().await, Ok(idx(12)));
        assert_eq!(handle.last_added_non_hint_entry().await, Some(idx(11)));
        assert!(
            handle
                .wait_for_replicas(2, Duration::from_secs(3))
                .await
                .unwrap()
        );
        assert!(
            !handle
                .wait_for_replicas(1, Duration::from_secs(3))
                .await
                .unwrap()
        );
        assert_eq!(
            handle
                .download_raw_payload(payload_id.clone(), vec![7, 8])
                .await,
            Ok(vec![7, 8])
        );
        assert_eq!(
            handle
                .download_raw_payload_classified(payload_id.clone(), vec![8, 7])
                .await
                .unwrap(),
            vec![8, 7]
        );
        assert!(Arc::ptr_eq(
            &handle.inner().unwrap(),
            &(inner.clone() as Arc<dyn Oplog>)
        ));

        assert_eq!(
            inner.take_calls(),
            vec![
                "retire".to_string(),
                "add_durable_stream_batch(0)".to_string(),
                "try_drop_prefix(6)".to_string(),
                "commit(Always)".to_string(),
                "try_current_oplog_index".to_string(),
                "last_added_non_hint_entry".to_string(),
                "wait_for_replicas(2, 3s)".to_string(),
                "wait_for_replicas(1, 3s)".to_string(),
                format!("download_raw_payload({payload_id:?}, [7, 8])"),
                format!("download_raw_payload_classified({payload_id:?}, [8, 7])"),
            ]
        );
    }

    /// Every call of the service that the layer does not check reaches the inner service with
    /// the same arguments, and its answer comes back unchanged. A read through the layer reads
    /// the inner service after the check of the retained start, and a read below the start does
    /// not reach the inner service.
    #[test]
    async fn the_retention_service_forwards_every_other_call_to_the_inner_service() {
        use crate::services::oplog::StagePublication;
        use crate::services::oplog::retention::advance_oplog_retention;
        use crate::services::oplog::retention::test_oplog::RecordingOplogService;
        use crate::services::stream_session_index::StreamSessionIndexService;
        use crate::storage::keyvalue::memory::InMemoryKeyValueStorage;

        let owner = OwnedAgentId::new(EnvironmentId::new(), &AgentId::from_parts(7, 1));
        let other = OwnedAgentId::new(EnvironmentId::new(), &AgentId::from_parts(7, 2));
        let inner = Arc::new(RecordingOplogService::new(owner.clone()));
        let key_value_storage: Arc<dyn KeyValueStorage + Send + Sync> =
            Arc::new(InMemoryKeyValueStorage::new());
        let service = RetentionOplogService::new(inner.clone(), key_value_storage.clone());
        advance_oplog_retention(
            key_value_storage.as_ref(),
            &owner,
            AgentMode::Durable,
            idx(4),
        )
        .await
        .unwrap();
        let odd_stage = uuid::Uuid::from_u128(3);
        let even_stage = uuid::Uuid::from_u128(4);
        let payload_id = PayloadId(uuid::Uuid::from_u128(9));
        let type_id = TypeId::try_from(3u32).unwrap();
        let durable = AgentMode::Durable;
        let ephemeral = AgentMode::Ephemeral;

        let index = Arc::new(StreamSessionIndexService::new(
            key_value_storage.clone(),
            Arc::downgrade(&(inner.clone() as Arc<dyn OplogService>)),
        ));
        service.set_stream_session_index(index.clone());
        assert!(Arc::ptr_eq(&inner.stream_session_index().unwrap(), &index));
        assert!(Arc::ptr_eq(
            &service.stream_session_index().unwrap(),
            &index
        ));
        assert_eq!(
            service.staged_exists(&owner, durable, odd_stage).await,
            Ok(true)
        );
        assert_eq!(
            service.staged_exists(&owner, durable, even_stage).await,
            Ok(false)
        );
        assert_eq!(
            service
                .publish_staged(
                    &owner,
                    durable,
                    StagePublication::for_tests(odd_stage),
                    idx(6)
                )
                .await,
            Ok(true)
        );
        assert_eq!(
            service
                .publish_staged(
                    &owner,
                    ephemeral,
                    StagePublication::for_tests(even_stage),
                    idx(3)
                )
                .await,
            Ok(false)
        );
        assert_eq!(
            service.discard_staged(&owner, durable, odd_stage).await,
            Err(format!("no stage {odd_stage}"))
        );
        assert_eq!(
            service.try_get_last_index(&owner, durable).await,
            Ok(idx(13))
        );
        service
            .assert_owning_epoch(&owner, durable, ShardEpoch(5))
            .await
            .unwrap();
        let fenced = service
            .assert_owning_epoch(&owner, durable, ShardEpoch(4))
            .await;
        assert!(
            matches!(&fenced, Err(OplogError::Fenced(fence)) if fence.expected_epoch == ShardEpoch(4)),
            "{fenced:?}"
        );
        assert_eq!(
            service
                .read_source(&owner, durable, idx(5), 2)
                .await
                .unwrap(),
            BTreeMap::from([(idx(5), jump(5)), (idx(6), jump(6))])
        );
        assert_eq!(
            service
                .read_source(&owner, durable, idx(2), 3)
                .await
                .unwrap_err()
                .error,
            expired(4)
        );
        assert_eq!(service.try_exists(&owner, durable).await, Ok(true));
        assert_eq!(service.try_exists(&owner, ephemeral).await, Ok(false));
        assert_eq!(
            service
                .scan_for_type(
                    &other.environment_id,
                    type_id,
                    Some(durable),
                    ScanCursor::new("from".to_string()),
                    17
                )
                .await
                .unwrap(),
            ScanPage {
                cursor: ScanCursor::new("after 3".to_string()),
                agent_ids: vec![owner.clone()],
                examined_keys: 17,
            }
        );
        assert_eq!(
            service
                .download_raw_payload(&owner, durable, payload_id.clone(), vec![7, 8])
                .await,
            Ok(vec![7, 8])
        );
        assert_eq!(
            service
                .download_raw_payload_classified(&other, ephemeral, payload_id.clone(), vec![8, 7])
                .await
                .unwrap(),
            vec![8, 7]
        );

        assert_eq!(
            inner.take_calls(),
            vec![
                "set_stream_session_index".to_string(),
                format!("staged_exists({owner}, Durable, {odd_stage})"),
                format!("staged_exists({owner}, Durable, {even_stage})"),
                format!("publish_staged({owner}, Durable, {odd_stage}, 6)"),
                format!("publish_staged({owner}, Ephemeral, {even_stage}, 3)"),
                format!("discard_staged({owner}, Durable, {odd_stage})"),
                format!("try_get_last_index({owner}, Durable)"),
                format!("assert_owning_epoch({owner}, Durable, {:?})", ShardEpoch(5)),
                format!("assert_owning_epoch({owner}, Durable, {:?})", ShardEpoch(4)),
                format!("read_source({owner}, Durable, 5, 2)"),
                format!("try_exists({owner}, Durable)"),
                format!("try_exists({owner}, Ephemeral)"),
                format!(
                    "scan_for_type({:?}, 3, Some(Durable), {:?}, 17)",
                    other.environment_id,
                    ScanCursor::new("from".to_string())
                ),
                format!("download_raw_payload({owner}, Durable, {payload_id:?}, [7, 8])"),
                format!(
                    "download_raw_payload_classified({other}, Ephemeral, {payload_id:?}, [8, 7])"
                ),
            ]
        );
    }
}
