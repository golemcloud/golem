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
use crate::span_test_support::{Tracing, get_tracing_dependency as test_r_get_dep_tracing};
use async_trait::async_trait;
use golem_common::model::agent::AgentMode;
use golem_common::model::component::ComponentId;
use golem_common::model::environment::EnvironmentId;
use golem_common::model::oplog::OplogIndex;
use golem_common::model::{AgentId, AgentStatus, AgentStatusRecord};
use golem_service_base::error::worker_executor::WorkerExecutorError;
use test_r::test;
use uuid::Uuid;

use crate::services::worker::{GetWorkerMetadataResult, WorkerService};

struct RecordedWrite {
    previous_was_some: bool,
    oplog_idx: OplogIndex,
}

#[derive(Default)]
struct MockState {
    writes: Vec<RecordedWrite>,
    tracking_calls: Vec<AgentStatus>,
    fail_writes: bool,
    fail_tracking: bool,
}

#[derive(Default)]
struct MockWorkerService {
    state: StdMutex<MockState>,
}

impl MockWorkerService {
    fn arc() -> Arc<Self> {
        Arc::new(Self::default())
    }
    fn set_fail(&self, fail: bool) {
        self.state.lock().unwrap().fail_writes = fail;
    }
    fn set_fail_tracking(&self, fail: bool) {
        self.state.lock().unwrap().fail_tracking = fail;
    }
    fn write_count(&self) -> usize {
        self.state.lock().unwrap().writes.len()
    }
    fn tracking_count(&self) -> usize {
        self.state.lock().unwrap().tracking_calls.len()
    }
    fn previous_flags(&self) -> Vec<bool> {
        self.state
            .lock()
            .unwrap()
            .writes
            .iter()
            .map(|w| w.previous_was_some)
            .collect()
    }
    fn flushed_oplog_idxs(&self) -> Vec<OplogIndex> {
        self.state
            .lock()
            .unwrap()
            .writes
            .iter()
            .map(|w| w.oplog_idx)
            .collect()
    }
}

#[async_trait]
impl WorkerService for MockWorkerService {
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
        unimplemented!()
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
        unimplemented!()
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
        previous_status: Option<&AgentStatusRecord>,
        status_value: AgentStatusRecord,
    ) -> Result<AgentStatusRecord, WorkerExecutorError> {
        let mut state = self.state.lock().unwrap();
        if state.fail_writes {
            return Err(
                golem_service_base::error::worker_executor::WorkerExecutorError::runtime(
                    "injected failure",
                ),
            );
        }
        state.writes.push(RecordedWrite {
            previous_was_some: previous_status.is_some(),
            oplog_idx: status_value.oplog_idx,
        });
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
        Ok(checkpoint)
    }
    async fn set_assignment_tracking(
        &self,
        _owned_agent_id: &OwnedAgentId,
        _fingerprint: AgentFingerprint,
        status_value: &AgentStatusRecord,
    ) -> Result<(), String> {
        let mut state = self.state.lock().unwrap();
        state.tracking_calls.push(status_value.status);
        if state.fail_tracking {
            return Err("injected tracking failure".to_string());
        }
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

fn agent_id() -> OwnedAgentId {
    OwnedAgentId::new(
        EnvironmentId::new(),
        &AgentId {
            component_id: ComponentId(Uuid::new_v4()),
            agent_id: "test".to_string(),
        },
    )
}

fn status(status: AgentStatus, oplog_idx: u64) -> AgentStatusRecord {
    AgentStatusRecord {
        status,
        oplog_idx: OplogIndex::from_u64(oplog_idx),
        ..AgentStatusRecord::default()
    }
}

fn make_flusher(
    is_ephemeral: bool,
    background_enabled: bool,
    worker_service: Arc<dyn WorkerService>,
    queue: Arc<AgentStatusFlushQueue>,
) -> (
    Arc<AgentStatusFlusher>,
    Arc<ArcSwap<AgentStatusRecord>>,
    Arc<AtomicBool>,
) {
    make_flusher_losing(
        is_ephemeral,
        background_enabled,
        worker_service,
        queue,
        tokio::sync::watch::channel(None).1,
    )
}

/// A flusher whose lost shard `lost_shard` reports.
fn make_flusher_losing(
    is_ephemeral: bool,
    background_enabled: bool,
    worker_service: Arc<dyn WorkerService>,
    queue: Arc<AgentStatusFlushQueue>,
    lost_shard: super::super::LostShard,
) -> (
    Arc<AgentStatusFlusher>,
    Arc<ArcSwap<AgentStatusRecord>>,
    Arc<AtomicBool>,
) {
    let current = Arc::new(ArcSwap::from_pointee(status(AgentStatus::Idle, 0)));
    let detached = Arc::new(AtomicBool::new(false));
    let flusher = AgentStatusFlusher::new(
        agent_id(),
        AgentFingerprint(uuid::Uuid::nil()),
        is_ephemeral,
        background_enabled,
        worker_service,
        queue,
        None,
        current.clone(),
        detached.clone(),
        lost_shard,
    );
    (flusher, current, detached)
}

fn test_queue() -> Arc<AgentStatusFlushQueue> {
    // Long interval so the background sweeper never fires; tests drive `sweep` manually.
    AgentStatusFlushQueue::new(Duration::from_secs(3600), 16, CancellationToken::new())
}

/// One span per sweep, not one for the lifetime of the sweeper.
#[test]
async fn sweep_records_one_closed_span_when_it_has_work(tracing: &Tracing) {
    let ws = MockWorkerService::arc();
    let queue = test_queue();
    let (flusher, current, _) = make_flusher(false, true, ws.clone(), queue.clone());

    current.store(Arc::new(status(AgentStatus::Running, 1)));
    flusher.mark_dirty();

    let recorder = crate::span_test_support::record_spans(tracing);
    queue.sweep().await;

    recorder.assert_closed_span("agent_status_flush_sweep");
    recorder.assert_all_closed();
}

/// An idle sweep is still spanned, so that every tick of the sweeper is
/// represented the same way.
#[test]
async fn sweep_records_one_closed_span_when_nothing_is_dirty(tracing: &Tracing) {
    let ws = MockWorkerService::arc();
    let queue = test_queue();
    let (_flusher, _current, _) = make_flusher(false, true, ws.clone(), queue.clone());

    let recorder = crate::span_test_support::record_spans(tracing);
    queue.sweep().await;

    recorder.assert_closed_span("agent_status_flush_sweep");
}

#[test]
async fn first_flush_uses_none_then_baseline() {
    let ws = MockWorkerService::arc();
    let queue = test_queue();
    let (flusher, current, _) = make_flusher(false, true, ws.clone(), queue.clone());

    current.store(Arc::new(status(AgentStatus::Running, 1)));
    flusher.mark_dirty();
    assert_eq!(queue.len(), 1);
    queue.sweep().await;

    current.store(Arc::new(status(AgentStatus::Running, 2)));
    flusher.mark_dirty();
    queue.sweep().await;

    // First write reconciles with `previous = None`; the second uses the persisted baseline.
    assert_eq!(ws.previous_flags(), vec![false, true]);
    assert_eq!(
        ws.flushed_oplog_idxs(),
        vec![OplogIndex::from_u64(1), OplogIndex::from_u64(2)]
    );
    assert!(!flusher.dirty.load(Ordering::Acquire));
    assert_eq!(queue.len(), 0);
}

#[test]
async fn first_flush_uses_persisted_baseline_when_known() {
    let ws = MockWorkerService::arc();
    let queue = test_queue();
    let persisted = status(AgentStatus::Idle, 7);
    let current = Arc::new(ArcSwap::from_pointee(persisted.clone()));
    let detached = Arc::new(AtomicBool::new(false));
    let flusher = AgentStatusFlusher::new(
        agent_id(),
        AgentFingerprint(uuid::Uuid::nil()),
        false,
        true,
        ws.clone(),
        queue.clone(),
        Some(persisted),
        current.clone(),
        detached,
        tokio::sync::watch::channel(None).1,
    );

    current.store(Arc::new(status(AgentStatus::Running, 8)));
    flusher.mark_dirty();
    queue.sweep().await;

    assert_eq!(ws.previous_flags(), vec![true]);
    assert_eq!(ws.flushed_oplog_idxs(), vec![OplogIndex::from_u64(8)]);
}

#[test]
async fn coalesces_multiple_changes_into_one_flush() {
    let ws = MockWorkerService::arc();
    let queue = test_queue();
    let (flusher, current, _) = make_flusher(false, true, ws.clone(), queue.clone());

    // Several status changes between sweeps collapse into a single write of the latest value.
    for idx in 1..=5 {
        current.store(Arc::new(status(AgentStatus::Running, idx)));
        flusher.mark_dirty();
    }
    assert_eq!(queue.len(), 1);

    queue.sweep().await;
    assert_eq!(ws.flushed_oplog_idxs(), vec![OplogIndex::from_u64(5)]);
}

#[test]
async fn mark_dirty_enqueues_once_per_episode() {
    let ws = MockWorkerService::arc();
    let queue = test_queue();
    let (flusher, _current, _) = make_flusher(false, true, ws.clone(), queue.clone());

    flusher.mark_dirty();
    flusher.mark_dirty();
    flusher.mark_dirty();
    assert_eq!(queue.len(), 1);

    queue.sweep().await;
    assert_eq!(ws.write_count(), 1);
    assert!(!flusher.dirty.load(Ordering::Acquire));

    // A second sweep with no new changes does nothing.
    queue.sweep().await;
    assert_eq!(ws.write_count(), 1);
}

#[test]
async fn background_skips_stale_clean_queue_entry() {
    let ws = MockWorkerService::arc();
    let queue = test_queue();
    let (flusher, current, _) = make_flusher(false, true, ws.clone(), queue.clone());

    // A change is marked dirty and enqueued, then a forced flush (e.g. suspend) writes it and
    // clears `dirty` -- but does not drain the queue entry.
    current.store(Arc::new(status(AgentStatus::Running, 1)));
    flusher.mark_dirty();
    assert_eq!(queue.len(), 1);
    let _ = flusher.flush(FlushReason::Forced).await;
    assert_eq!(ws.write_count(), 1);
    assert!(!flusher.dirty.load(Ordering::Acquire));
    assert_eq!(queue.len(), 1); // forced flush left the queue entry behind

    // The next sweep drains the now-clean entry and must NOT issue a redundant write.
    queue.sweep().await;
    assert_eq!(ws.write_count(), 1);
    assert_eq!(queue.len(), 0);
}

#[test]
async fn write_failure_requeues_dirty() {
    let ws = MockWorkerService::arc();
    let queue = test_queue();
    let (flusher, current, _) = make_flusher(false, true, ws.clone(), queue.clone());

    current.store(Arc::new(status(AgentStatus::Running, 1)));
    ws.set_fail(true);
    flusher.mark_dirty();
    queue.sweep().await;

    // Write failed: still dirty and re-enqueued, nothing recorded.
    assert!(flusher.dirty.load(Ordering::Acquire));
    assert_eq!(queue.len(), 1);
    assert_eq!(ws.write_count(), 0);

    ws.set_fail(false);
    queue.sweep().await;
    assert_eq!(ws.write_count(), 1);
    assert!(!flusher.dirty.load(Ordering::Acquire));
    // The recovered write is still a full reconcile (baseline never became known).
    assert_eq!(ws.previous_flags(), vec![false]);
}

#[test]
async fn delete_started_prevents_flush() {
    let ws = MockWorkerService::arc();
    let queue = test_queue();
    let (flusher, current, _) = make_flusher(false, true, ws.clone(), queue.clone());

    current.store(Arc::new(status(AgentStatus::Running, 1)));
    flusher.begin_delete().await;

    flusher.mark_dirty();
    assert_eq!(queue.len(), 0); // mark_dirty is a no-op after begin_delete
    let _ = flusher.flush(FlushReason::Forced).await;
    queue.sweep().await;

    assert_eq!(ws.write_count(), 0);
    assert!(!flusher.dirty.load(Ordering::Acquire));
}

// The blob is the shard's new owner's once this generation is given up: neither a forced flush
// nor a background sweep of a flush queued before the give-up may write it.
#[test]
async fn a_given_up_flusher_never_writes_the_status_blob() {
    let ws = MockWorkerService::arc();
    let queue = test_queue();
    let (lose, lost) = tokio::sync::watch::channel(None);
    let (flusher, current, _) = make_flusher_losing(false, true, ws.clone(), queue.clone(), lost);

    current.store(Arc::new(status(AgentStatus::Running, 1)));
    flusher.mark_dirty();

    lose.send_replace(Some(super::super::RetirementReason::ShardRevoked));
    let _ = flusher.flush(FlushReason::Forced).await;
    current.store(Arc::new(status(AgentStatus::Idle, 2)));
    flusher.mark_dirty();
    queue.sweep().await;

    assert_eq!(
        ws.write_count(),
        0,
        "a given-up generation wrote its status blob"
    );
    assert!(!flusher.dirty.load(Ordering::Acquire));
}

// Nor the recovery-index row: a stale generation dropping it would hide the agent from the new
// owner's crash recovery. Inline flushing is exercised too, as background flushing is off.
#[test]
async fn a_given_up_flusher_leaves_the_recovery_index_alone() {
    let ws = MockWorkerService::arc();
    let queue = test_queue();
    let (lose, lost) = tokio::sync::watch::channel(None);
    let (flusher, _current, _) = make_flusher_losing(false, false, ws.clone(), queue.clone(), lost);

    lose.send_replace(Some(super::super::RetirementReason::ShardRevoked));
    flusher
        .on_status_changed(
            &status(AgentStatus::Idle, 0),
            &status(AgentStatus::Running, 1),
        )
        .await;
    flusher
        .on_status_changed(
            &status(AgentStatus::Running, 1),
            &status(AgentStatus::Idle, 2),
        )
        .await;

    assert_eq!(ws.tracking_count(), 0);
    assert_eq!(ws.write_count(), 0);
}

#[test]
async fn detached_skips_flush() {
    let ws = MockWorkerService::arc();
    let queue = test_queue();
    let (flusher, current, detached) = make_flusher(false, true, ws.clone(), queue.clone());

    current.store(Arc::new(status(AgentStatus::Running, 1)));
    detached.store(true, Ordering::Release);
    flusher.mark_dirty();
    let _ = flusher.flush(FlushReason::Forced).await;

    assert_eq!(ws.write_count(), 0);
    assert!(!flusher.dirty.load(Ordering::Acquire));

    // After reattach (detached cleared), flushing works again.
    detached.store(false, Ordering::Release);
    flusher.mark_dirty();
    queue.sweep().await;
    assert_eq!(ws.write_count(), 1);
}

#[test]
async fn ephemeral_is_noop() {
    let ws = MockWorkerService::arc();
    let queue = test_queue();
    let (flusher, _current, _) = make_flusher(true, true, ws.clone(), queue.clone());

    flusher
        .on_status_changed(
            &status(AgentStatus::Idle, 0),
            &status(AgentStatus::Running, 1),
        )
        .await;
    flusher.mark_dirty();
    let _ = flusher.flush(FlushReason::Forced).await;
    queue.sweep().await;

    assert_eq!(ws.write_count(), 0);
    assert_eq!(ws.tracking_count(), 0);
    assert_eq!(queue.len(), 0);
}

#[test]
async fn assignment_tracking_fires_only_on_transition() {
    let ws = MockWorkerService::arc();
    let queue = test_queue();
    let (flusher, _current, _) = make_flusher(false, true, ws.clone(), queue.clone());

    // Idle -> Running: predicate transitions false -> true, fires.
    flusher
        .on_status_changed(
            &status(AgentStatus::Idle, 0),
            &status(AgentStatus::Running, 1),
        )
        .await;
    // Running -> Running: no transition, does not fire.
    flusher
        .on_status_changed(
            &status(AgentStatus::Running, 1),
            &status(AgentStatus::Running, 2),
        )
        .await;
    // Running -> Idle: transitions true -> false, fires.
    flusher
        .on_status_changed(
            &status(AgentStatus::Running, 2),
            &status(AgentStatus::Idle, 3),
        )
        .await;

    assert_eq!(ws.tracking_count(), 2);
}

#[test]
async fn disabled_background_flushes_inline() {
    let ws = MockWorkerService::arc();
    let queue = test_queue();
    let (flusher, current, _) = make_flusher(false, false, ws.clone(), queue.clone());

    current.store(Arc::new(status(AgentStatus::Running, 1)));
    flusher
        .on_status_changed(
            &status(AgentStatus::Idle, 0),
            &status(AgentStatus::Running, 1),
        )
        .await;

    // Blob written synchronously, nothing left in the queue.
    assert_eq!(ws.write_count(), 1);
    assert_eq!(queue.len(), 0);
    assert!(!flusher.dirty.load(Ordering::Acquire));
}

#[test]
async fn distinct_flushers_have_distinct_queue_ids() {
    let ws = MockWorkerService::arc();
    let queue = test_queue();
    let (a, _, _) = make_flusher(false, true, ws.clone(), queue.clone());
    let (b, _, _) = make_flusher(false, true, ws.clone(), queue.clone());
    assert_ne!(a.queue_id, b.queue_id);
}

/// A failed recovery-index update fails the whole call, and the blob is not written after it.
/// Reporting success would let the caller go on to run a worker that `RunningWorkers` does not
/// list - one that no crash or reshard would resume - while the blob left behind makes it look
/// accounted for.
#[test]
async fn update_cached_status_fails_when_tracking_fails() {
    let service = MockWorkerService::arc();
    service.set_fail_tracking(true);

    let result = service
        .update_cached_status(
            &agent_id(),
            AgentFingerprint(uuid::Uuid::nil()),
            None,
            status(AgentStatus::Running, 1),
        )
        .await;

    assert_eq!(
        result,
        Err(
            golem_service_base::error::worker_executor::WorkerExecutorError::runtime(
                "injected tracking failure"
            )
        )
    );
    assert_eq!(service.tracking_count(), 1);
    assert_eq!(service.write_count(), 0);
}

/// The blob write is reported too: it is the caller's only signal that the status it just
/// computed did not reach storage.
#[test]
async fn update_cached_status_reports_a_failed_status_write() {
    let service = MockWorkerService::arc();
    service.set_fail(true);

    let result = service
        .update_cached_status(
            &agent_id(),
            AgentFingerprint(uuid::Uuid::nil()),
            None,
            status(AgentStatus::Running, 1),
        )
        .await;

    assert!(result.is_err());
    assert_eq!(service.tracking_count(), 1);
    assert_eq!(service.write_count(), 0);
}
