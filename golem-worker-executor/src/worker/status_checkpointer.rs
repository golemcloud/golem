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

//! Persistence of the *clean* cached [`AgentStatusRecord`] checkpoint.
//!
//! The live cached status blob (see [`crate::worker::status_flusher`]) is written in the background
//! and is allowed to advance into replay/deleted oplog regions. That makes it useless as a fold
//! baseline after a jump: a replay-time `Jump` deletes the region the cached status' own oplog index
//! sits in, so the status recompute can no longer fold forward and has to re-read the whole oplog
//! from index 1.
//!
//! [`StatusCheckpointer`] maintains a *separate* copy of the status — the checkpoint — written only
//! at structurally clean boundaries where no jumpable region is open: at snapshot save and at
//! throttled idle boundaries. Because it is never advanced into an open region, it always holds a
//! baseline that predates any later jump, so the recompute can fold forward from it
//! (see [`crate::worker::status::calculate_last_known_status_with_checkpoint`]).
//!
//! The checkpoint is best-effort: a failed (or stale, or missing) checkpoint only means a future
//! recompute reads a few more oplog entries, never incorrect state — the oplog remains the single
//! source of truth.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use tokio::sync::Mutex;
use tracing::debug;

use golem_common::model::{AgentStatusRecord, OwnedAgentId};

use crate::services::worker::WorkerService;

/// Why a checkpoint is being written. Snapshot-aligned checkpoints bypass the idle throttle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckpointReason {
    /// Written right after a snapshot oplog entry was committed (a natural clean boundary).
    Snapshot,
    /// Written when the worker goes idle. Throttled by the configured min oplog-index delta.
    Idle,
    /// Written *during* a long-running invocation at a structurally clean boundary (no open
    /// rollback region) and below the outstanding `get_oplog_index` marker watermark. Throttled
    /// exactly like [`CheckpointReason::Idle`].
    MidInvocation,
}

impl CheckpointReason {
    fn as_str(&self) -> &'static str {
        match self {
            CheckpointReason::Snapshot => "snapshot",
            CheckpointReason::Idle => "idle",
            CheckpointReason::MidInvocation => "mid_invocation",
        }
    }
}

/// Baseline state guarded by the checkpoint lock.
struct CheckpointState {
    /// The last status successfully written to the checkpoint, used both as the delta baseline for
    /// the next write and as the throttle reference (its `oplog_idx`). `None` until the first
    /// successful write of this worker incarnation, forcing a full reconcile write.
    last_written: Option<AgentStatusRecord>,
}

/// Per-worker coordinator that writes the clean status checkpoint at clean boundaries.
pub struct StatusCheckpointer {
    owned_agent_id: OwnedAgentId,
    /// Ephemeral workers never persist any cached status; every operation is a no-op for them.
    is_ephemeral: bool,
    /// When `false`, no checkpoint is ever written (recompute falls back to a full from-scratch
    /// fold).
    enabled: bool,
    /// Minimum number of oplog entries appended since the last checkpoint before a throttled
    /// (idle) checkpoint is written. Snapshot checkpoints ignore this.
    min_oplog_delta: u64,

    worker_service: Arc<dyn WorkerService>,

    /// Set once the owning worker starts deleting. After this, no checkpoint is written, so an
    /// in-flight write cannot resurrect the checkpoint after `remove_cached_status` deletes it.
    delete_started: AtomicBool,

    /// Serializes checkpoint writes and guards the persisted baseline.
    state: Mutex<CheckpointState>,
}

impl StatusCheckpointer {
    pub fn new(
        owned_agent_id: OwnedAgentId,
        is_ephemeral: bool,
        enabled: bool,
        min_oplog_delta: u64,
        worker_service: Arc<dyn WorkerService>,
    ) -> Self {
        Self {
            owned_agent_id,
            is_ephemeral,
            enabled,
            min_oplog_delta,
            worker_service,
            delete_started: AtomicBool::new(false),
            state: Mutex::new(CheckpointState { last_written: None }),
        }
    }

    /// Prevents any future checkpoint write from resurrecting the checkpoint after it is deleted.
    ///
    /// Mirrors [`crate::worker::status_flusher::AgentStatusFlusher::begin_delete`]: setting the flag
    /// alone is not enough, because a [`Self::maybe_checkpoint`] call that already passed the
    /// early-out may be mid-write while holding the state lock and would complete after the delete.
    /// Acquiring the state lock here is a barrier — once held, no checkpoint write is in progress,
    /// and every subsequent call observes `delete_started == true` and bails before writing. Must be
    /// called (and awaited) before `WorkerService::remove`/`remove_cached_status`.
    pub async fn begin_delete(&self) {
        self.delete_started.store(true, Ordering::Release);
        // Barrier: blocks until any in-flight checkpoint write completes.
        let _state = self.state.lock().await;
    }

    /// Writes a clean status checkpoint from `status` if eligible.
    ///
    /// `status` must be a current, non-detached status taken at a structurally clean boundary (no
    /// jumpable region open). Eligibility:
    /// - never for ephemeral workers or when disabled;
    /// - always for [`CheckpointReason::Snapshot`] (bypasses the throttle, aligning the checkpoint
    ///   with the snapshot index);
    /// - for [`CheckpointReason::Idle`]: when no checkpoint exists yet, when the existing checkpoint
    ///   is no longer a usable fold baseline (its index now sits in a skipped/deleted region — e.g.
    ///   a logical revert appended a `Revert` marker and recorded a deleted region covering it), or
    ///   when at least `min_oplog_delta` entries were appended since the last checkpoint.
    ///
    /// Never written once [`Self::begin_delete`] has run, so an in-flight write cannot resurrect the
    /// checkpoint after it is deleted.
    ///
    /// Best-effort: a write failure is logged and metered, the baseline is left unchanged, and the
    /// worker continues. The oplog remains the source of truth.
    pub async fn maybe_checkpoint(&self, status: &AgentStatusRecord, reason: CheckpointReason) {
        if self.is_ephemeral || !self.enabled || self.delete_started.load(Ordering::Acquire) {
            return;
        }

        let mut state = self.state.lock().await;

        // Re-check after taking the lock: `begin_delete` may have set the flag while we waited for
        // it (it takes this same lock as a barrier), so once we hold the lock the flag is final.
        if self.delete_started.load(Ordering::Acquire) {
            return;
        }

        let should_write = match (reason, &state.last_written) {
            (CheckpointReason::Snapshot, _) => true,
            (CheckpointReason::Idle | CheckpointReason::MidInvocation, None) => true,
            (CheckpointReason::Idle | CheckpointReason::MidInvocation, Some(previous)) => {
                let current_idx = status.oplog_idx.as_u64();
                let previous_idx = previous.oplog_idx.as_u64();
                // The previous checkpoint is unusable as a fold baseline once a region deletes its
                // index. Reverts are *logical* (a `Revert` marker is appended and a deleted region
                // recorded), so they leave `current_idx > previous_idx` with a small delta and the
                // throttle below would skip the refresh — leaving only a checkpoint that a later
                // recompute must discard, falling back to a full from-scratch fold. Force a refresh
                // in that case. (`current_idx < previous_idx` is kept as a defensive belt; the
                // append-only oplog does not actually move the tip backwards.)
                let previous_unusable = status
                    .skipped_regions
                    .is_in_deleted_region(previous.oplog_idx)
                    || status
                        .deleted_regions
                        .is_in_deleted_region(previous.oplog_idx);
                previous_unusable
                    || current_idx < previous_idx
                    || current_idx.saturating_sub(previous_idx) >= self.min_oplog_delta
            }
        };

        if !should_write {
            return;
        }

        match self
            .worker_service
            .write_status_checkpoint(
                &self.owned_agent_id,
                state.last_written.as_ref(),
                status.clone(),
            )
            .await
        {
            Ok(written) => {
                debug!(
                    agent_id = %self.owned_agent_id,
                    reason = reason.as_str(),
                    oplog_idx = %written.oplog_idx,
                    "Wrote clean agent status checkpoint"
                );
                state.last_written = Some(written);
                crate::metrics::workers::record_agent_status_checkpoint_write(reason.as_str());
            }
            Err(err) => {
                debug!(
                    agent_id = %self.owned_agent_id,
                    reason = reason.as_str(),
                    "Failed to write clean agent status checkpoint (will retry at next boundary): {err}"
                );
                crate::metrics::workers::record_agent_status_checkpoint_write_failed(
                    reason.as_str(),
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use golem_common::model::agent::AgentMode;
    use golem_common::model::component::ComponentId;
    use golem_common::model::environment::EnvironmentId;
    use golem_common::model::oplog::OplogIndex;
    use golem_common::model::regions::OplogRegion;
    use golem_common::model::{AgentId, AgentStatus, AgentStatusRecord};
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
        async fn get(&self, _owned_agent_id: &OwnedAgentId) -> Option<GetWorkerMetadataResult> {
            None
        }

        async fn get_running_workers_in_shards(&self) -> Vec<GetWorkerMetadataResult> {
            Vec::new()
        }

        async fn remove(&self, _owned_agent_id: &OwnedAgentId) {}

        async fn remove_cached_status(&self, _owned_agent_id: &OwnedAgentId) {}

        async fn get_agent_mode(&self, _owned_agent_id: &OwnedAgentId) -> Option<AgentMode> {
            Some(AgentMode::Durable)
        }

        async fn write_cached_status(
            &self,
            _owned_agent_id: &OwnedAgentId,
            _previous_status: Option<&AgentStatusRecord>,
            status_value: AgentStatusRecord,
        ) -> Result<AgentStatusRecord, String> {
            Ok(status_value)
        }

        async fn read_status_checkpoint(
            &self,
            _owned_agent_id: &OwnedAgentId,
            _agent_mode: AgentMode,
        ) -> Option<AgentStatusRecord> {
            None
        }

        async fn write_status_checkpoint(
            &self,
            _owned_agent_id: &OwnedAgentId,
            _previous_checkpoint: Option<&AgentStatusRecord>,
            checkpoint: AgentStatusRecord,
        ) -> Result<AgentStatusRecord, String> {
            self.writes.lock().unwrap().push(checkpoint.oplog_idx);
            Ok(checkpoint)
        }

        async fn set_assignment_tracking(
            &self,
            _owned_agent_id: &OwnedAgentId,
            _status_value: &AgentStatusRecord,
        ) {
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
        StatusCheckpointer::new(owned_agent_id(), false, true, min_delta, service)
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

    #[test]
    async fn disabled_and_ephemeral_never_write() {
        let service = Arc::new(RecordingWorkerService::default());

        let disabled = StatusCheckpointer::new(owned_agent_id(), false, false, 0, service.clone());
        disabled
            .maybe_checkpoint(&status_at(10), CheckpointReason::Snapshot)
            .await;

        let ephemeral = StatusCheckpointer::new(owned_agent_id(), true, true, 0, service.clone());
        ephemeral
            .maybe_checkpoint(&status_at(10), CheckpointReason::Snapshot)
            .await;

        assert!(service.writes.lock().unwrap().is_empty());
    }
}
