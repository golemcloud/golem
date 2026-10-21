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

use super::*;
use async_trait::async_trait;
use golem_common::model::agent::AgentMode;
use golem_common::model::component::ComponentId;
use golem_common::model::environment::EnvironmentId;
use golem_common::model::oplog::OplogIndex;
use golem_common::model::regions::OplogRegion;
use golem_common::model::{AgentId, AgentStatus, AgentStatusRecord};
use golem_service_base::error::worker_executor::WorkerExecutorError;
use std::sync::Mutex as StdMutex;
use test_r::test;
use uuid::Uuid;

use crate::services::worker::{GetWorkerMetadataResult, WorkerService};

/// Records every checkpoint write so tests can assert the throttle behaviour.
#[derive(Default)]
struct RecordingWorkerService {
    writes: StdMutex<Vec<OplogIndex>>,
}

#[async_trait]
impl WorkerService for RecordingWorkerService {
    async fn lookup_durable_stream_public_binding(
        &self,
        _owned_agent_id: &OwnedAgentId,
        _agent_mode: AgentMode,
        _fingerprint: golem_common::model::AgentFingerprint,
        _public_session_id: &str,
    ) -> Result<
        Option<golem_common::model::DurableStreamPublicBinding>,
        crate::durable_host::durable_stream::SessionError,
    > {
        unimplemented!()
    }

    async fn get(
        &self,
        _owned_agent_id: &OwnedAgentId,
    ) -> Result<Option<GetWorkerMetadataResult>, WorkerExecutorError> {
        Ok(None)
    }

    async fn lookup_durable_stream_session(
        &self,
        _owned_agent_id: &OwnedAgentId,
        _agent_mode: AgentMode,
        _fingerprint: golem_common::model::AgentFingerprint,
        _status: &AgentStatusRecord,
        _key: &golem_common::model::IdempotencyKey,
    ) -> Result<
        Option<golem_common::model::DurableStreamSessionStatus>,
        crate::durable_host::durable_stream::SessionError,
    > {
        unimplemented!()
    }

    async fn get_running_workers_in_shards(
        &self,
        _on_stale: crate::services::worker::OnStale<'_>,
    ) -> Result<Vec<crate::services::worker::GetWorkerMetadataResult>, WorkerExecutorError> {
        Ok(Default::default())
    }

    async fn remove(
        &self,
        _lifecycle: &mut crate::services::oplog::OplogLifecycleGuard,
        _owned_agent_id: &OwnedAgentId,
        _agent_mode: AgentMode,
        _fingerprint: AgentFingerprint,
        _expected_epoch: Option<golem_common::model::ShardEpoch>,
        _after_oplog_delete: &(dyn Fn(AgentFingerprint) + Send + Sync),
    ) -> Result<(), WorkerExecutorError> {
        Ok(())
    }

    async fn remove_cached_status(
        &self,
        _owned_agent_id: &OwnedAgentId,
        _fingerprint: AgentFingerprint,
    ) -> Result<(), WorkerExecutorError> {
        Ok(())
    }

    async fn resolve_agent_identity(
        &self,
        _owned_agent_id: &OwnedAgentId,
    ) -> Result<Option<crate::services::worker::ResolvedAgentIdentity>, WorkerExecutorError> {
        Ok(None)
    }

    async fn write_cached_status(
        &self,
        _owned_agent_id: &OwnedAgentId,
        _fingerprint: AgentFingerprint,
        _previous_status: Option<&AgentStatusRecord>,
        status_value: AgentStatusRecord,
    ) -> Result<AgentStatusRecord, WorkerExecutorError> {
        Ok(status_value)
    }

    async fn read_status_checkpoint(
        &self,
        _owned_agent_id: &OwnedAgentId,
        _fingerprint: AgentFingerprint,
        _agent_mode: AgentMode,
    ) -> Result<Option<AgentStatusRecord>, WorkerExecutorError> {
        Ok(None)
    }

    async fn write_status_checkpoint(
        &self,
        _owned_agent_id: &OwnedAgentId,
        _fingerprint: AgentFingerprint,
        _previous_checkpoint: Option<&AgentStatusRecord>,
        checkpoint: AgentStatusRecord,
    ) -> Result<AgentStatusRecord, WorkerExecutorError> {
        self.writes.lock().unwrap().push(checkpoint.oplog_idx);
        Ok(checkpoint)
    }

    async fn set_assignment_tracking(
        &self,
        _owned_agent_id: &OwnedAgentId,
        _fingerprint: AgentFingerprint,
        _status_value: &AgentStatusRecord,
    ) -> Result<(), String> {
        Ok(())
    }

    async fn remove_if_stale(
        &self,
        _owned_agent_id: &OwnedAgentId,
        _fingerprint: golem_common::model::AgentFingerprint,
        _on_stale: crate::services::worker::OnStale<'_>,
    ) -> Result<bool, WorkerExecutorError> {
        Ok(false)
    }
}

fn owned_agent_id() -> OwnedAgentId {
    OwnedAgentId::new(
        EnvironmentId::new(),
        &AgentId {
            component_id: ComponentId(Uuid::new_v4()),
            agent_id: "test".to_string(),
        },
    )
}

fn status_at(oplog_idx: u64) -> AgentStatusRecord {
    AgentStatusRecord {
        status: AgentStatus::Idle,
        oplog_idx: OplogIndex::from_u64(oplog_idx),
        ..AgentStatusRecord::default()
    }
}

fn checkpointer(service: Arc<RecordingWorkerService>, min_delta: u64) -> StatusCheckpointer {
    checkpointer_losing(service, min_delta, tokio::sync::watch::channel(None).1)
}

/// A checkpointer whose lost shard `lost_shard` reports.
fn checkpointer_losing(
    service: Arc<RecordingWorkerService>,
    min_delta: u64,
    lost_shard: super::super::LostShard,
) -> StatusCheckpointer {
    StatusCheckpointer::new(
        owned_agent_id(),
        AgentFingerprint(Uuid::nil()),
        false,
        true,
        min_delta,
        service,
        lost_shard,
    )
}

#[test]
async fn a_worker_service_that_stores_no_rejections_gives_no_rejected_snapshots() {
    let rejected = RecordingWorkerService::default()
        .get_rejected_periodic_snapshots(&owned_agent_id(), AgentFingerprint(Uuid::nil()))
        .await
        .unwrap();

    assert!(rejected.is_empty());
}

#[test]
async fn idle_first_checkpoint_is_always_written() {
    let service = Arc::new(RecordingWorkerService::default());
    let cp = checkpointer(service.clone(), 100);

    cp.maybe_checkpoint(&status_at(10), CheckpointReason::Idle)
        .await;

    assert_eq!(
        *service.writes.lock().unwrap(),
        vec![OplogIndex::from_u64(10)]
    );
}

#[test]
async fn idle_throttles_until_min_delta() {
    let service = Arc::new(RecordingWorkerService::default());
    let cp = checkpointer(service.clone(), 100);

    cp.maybe_checkpoint(&status_at(10), CheckpointReason::Idle)
        .await; // first -> written (10)
    cp.maybe_checkpoint(&status_at(50), CheckpointReason::Idle)
        .await; // +40 < 100 -> skipped
    cp.maybe_checkpoint(&status_at(110), CheckpointReason::Idle)
        .await; // +100 >= 100 -> written (110)

    assert_eq!(
        *service.writes.lock().unwrap(),
        vec![OplogIndex::from_u64(10), OplogIndex::from_u64(110)]
    );
}

#[test]
async fn mid_invocation_first_checkpoint_is_always_written() {
    let service = Arc::new(RecordingWorkerService::default());
    let cp = checkpointer(service.clone(), 100);

    cp.maybe_checkpoint(&status_at(10), CheckpointReason::MidInvocation)
        .await;

    assert_eq!(
        *service.writes.lock().unwrap(),
        vec![OplogIndex::from_u64(10)]
    );
}

#[test]
async fn mid_invocation_throttles_like_idle() {
    let service = Arc::new(RecordingWorkerService::default());
    let cp = checkpointer(service.clone(), 100);

    cp.maybe_checkpoint(&status_at(10), CheckpointReason::MidInvocation)
        .await; // first -> written (10)
    cp.maybe_checkpoint(&status_at(50), CheckpointReason::MidInvocation)
        .await; // +40 < 100 -> skipped
    cp.maybe_checkpoint(&status_at(110), CheckpointReason::MidInvocation)
        .await; // +100 >= 100 -> written (110)

    assert_eq!(
        *service.writes.lock().unwrap(),
        vec![OplogIndex::from_u64(10), OplogIndex::from_u64(110)]
    );
}

#[test]
async fn mid_invocation_and_idle_share_the_same_throttle_baseline() {
    let service = Arc::new(RecordingWorkerService::default());
    let cp = checkpointer(service.clone(), 100);

    // A mid-invocation checkpoint and a subsequent idle checkpoint use the same persisted
    // baseline, so the idle one is throttled relative to the mid-invocation write.
    cp.maybe_checkpoint(&status_at(10), CheckpointReason::MidInvocation)
        .await; // written (10)
    cp.maybe_checkpoint(&status_at(40), CheckpointReason::Idle)
        .await; // +30 < 100 -> skipped
    cp.maybe_checkpoint(&status_at(150), CheckpointReason::Idle)
        .await; // +140 >= 100 -> written (150)

    assert_eq!(
        *service.writes.lock().unwrap(),
        vec![OplogIndex::from_u64(10), OplogIndex::from_u64(150)]
    );
}

#[test]
async fn snapshot_bypasses_throttle() {
    let service = Arc::new(RecordingWorkerService::default());
    let cp = checkpointer(service.clone(), 100);

    cp.maybe_checkpoint(&status_at(10), CheckpointReason::Idle)
        .await; // written (10)
    cp.maybe_checkpoint(&status_at(20), CheckpointReason::Snapshot)
        .await; // +10 but snapshot -> written (20)

    assert_eq!(
        *service.writes.lock().unwrap(),
        vec![OplogIndex::from_u64(10), OplogIndex::from_u64(20)]
    );
}

#[test]
async fn idle_refreshes_when_oplog_moved_behind_checkpoint() {
    let service = Arc::new(RecordingWorkerService::default());
    let cp = checkpointer(service.clone(), 100);

    cp.maybe_checkpoint(&status_at(500), CheckpointReason::Idle)
        .await; // written (500)
    // A revert truncated the oplog: the new tip is behind the stale checkpoint -> refresh.
    cp.maybe_checkpoint(&status_at(200), CheckpointReason::Idle)
        .await;

    assert_eq!(
        *service.writes.lock().unwrap(),
        vec![OplogIndex::from_u64(500), OplogIndex::from_u64(200)]
    );
}

#[test]
async fn idle_refreshes_when_previous_checkpoint_in_deleted_region() {
    let service = Arc::new(RecordingWorkerService::default());
    // High throttle so a small index delta alone would never trigger a refresh.
    let cp = checkpointer(service.clone(), 1000);

    cp.maybe_checkpoint(&status_at(100), CheckpointReason::Idle)
        .await; // written (100)

    // A logical revert appends a `Revert` marker (tip advances to 121) and records a deleted
    // region [80, 120] that covers the previous checkpoint at 100. The +21 delta is below the
    // 1000 throttle, but the previous checkpoint is now an unusable fold baseline, so we must
    // refresh it rather than leave only a checkpoint a later recompute would have to discard.
    let mut after_revert = status_at(121);
    after_revert.skipped_regions.add(OplogRegion {
        start: OplogIndex::from_u64(80),
        end: OplogIndex::from_u64(120),
    });
    cp.maybe_checkpoint(&after_revert, CheckpointReason::Idle)
        .await;

    assert_eq!(
        *service.writes.lock().unwrap(),
        vec![OplogIndex::from_u64(100), OplogIndex::from_u64(121)]
    );
}

#[test]
async fn delete_started_prevents_checkpoint() {
    let service = Arc::new(RecordingWorkerService::default());
    let cp = checkpointer(service.clone(), 0);

    cp.begin_delete().await;
    // Even a snapshot checkpoint (which otherwise bypasses every throttle) must not write after
    // deletion has started, so it cannot resurrect a deleted checkpoint.
    cp.maybe_checkpoint(&status_at(10), CheckpointReason::Snapshot)
        .await;

    assert!(service.writes.lock().unwrap().is_empty());
}

// A given-up generation's checkpoint belongs to the shard's new owner: no reason, not even a
// snapshot, may write one after the give-up.
#[test]
async fn a_given_up_checkpointer_writes_no_checkpoint() {
    let service = Arc::new(RecordingWorkerService::default());
    let (lose, lost) = tokio::sync::watch::channel(None);
    let cp = checkpointer_losing(service.clone(), 0, lost);

    lose.send_replace(Some(super::super::RetirementReason::ShardRevoked));
    cp.maybe_checkpoint(&status_at(10), CheckpointReason::Snapshot)
        .await;
    cp.maybe_checkpoint(&status_at(20), CheckpointReason::Idle)
        .await;
    cp.maybe_checkpoint(&status_at(30), CheckpointReason::MidInvocation)
        .await;

    assert!(
        service.writes.lock().unwrap().is_empty(),
        "a given-up generation wrote a status checkpoint"
    );
}

#[test]
async fn disabled_and_ephemeral_never_write() {
    let service = Arc::new(RecordingWorkerService::default());

    let disabled = StatusCheckpointer::new(
        owned_agent_id(),
        AgentFingerprint(Uuid::nil()),
        false,
        false,
        0,
        service.clone(),
        tokio::sync::watch::channel(None).1,
    );
    disabled
        .maybe_checkpoint(&status_at(10), CheckpointReason::Snapshot)
        .await;

    let ephemeral = StatusCheckpointer::new(
        owned_agent_id(),
        AgentFingerprint(Uuid::nil()),
        true,
        true,
        0,
        service.clone(),
        tokio::sync::watch::channel(None).1,
    );
    ephemeral
        .maybe_checkpoint(&status_at(10), CheckpointReason::Snapshot)
        .await;

    assert!(service.writes.lock().unwrap().is_empty());
}
