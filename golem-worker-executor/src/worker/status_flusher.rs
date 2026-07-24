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

//! Background-batched persistence of the cached [`AgentStatusRecord`] blob.
//!
//! Historically the cached status blob was written to the KV store on *every* oplog commit, which
//! for hot, long-running agents produced an enormous volume of read/write traffic. The blob is,
//! however, fully derivable from the oplog (which is the single source of truth): a stale blob only
//! means a cold read has to fold a few more oplog entries forward, never incorrect state.
//!
//! [`AgentStatusFlusher`] decouples the blob write from the commit. On each status change the
//! worker is merely marked *dirty* ([`AgentStatusFlusher::mark_dirty`]); a single per-executor
//! background sweeper ([`AgentStatusFlushQueue`]) coalesces the writes and flushes each dirty
//! worker at most once per configured interval. Lifecycle boundaries (suspend, stop/evict,
//! reattach) force a synchronous flush so the cache is current when it matters most.
//!
//! The `RunningWorkers` recovery index is **not** managed here — it is the authoritative resume
//! index after a crash/reshard and is always updated synchronously on the hot path (see
//! [`crate::services::worker::WorkerService::set_assignment_tracking`]). To avoid a KV round-trip
//! per commit we only touch that index when the tracking predicate actually transitions. This is
//! safe across reshards because the index is keyed by a worker's *shard id*
//! (`hash(agent_id) % number_of_shards`), and `number_of_shards` is a fixed cluster-wide constant:
//! a worker's shard id therefore never changes, so a reshard merely moves ownership of an existing
//! shard-keyed set to another executor (which enumerates it via `get_running_workers_in_shards`),
//! without needing the entry to be re-written.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex, Weak};
use std::time::Duration;

use arc_swap::ArcSwap;
use futures::StreamExt;
use tokio::sync::Mutex;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::{Instrument, Level, debug, error, span};

use golem_common::model::{AgentStatusRecord, OwnedAgentId};

use crate::services::worker::{DefaultWorkerService, WorkerService};

/// Process-wide source of unique flusher ids. Used as the dirty-queue key so that two distinct
/// flusher instances for the *same* agent id (e.g. an old instance still mid-flush during an
/// evict→reload cycle) cannot shadow each other's queue entry. The agent id / fingerprint cannot
/// be used for this: the fingerprint is persisted in the `Create` oplog entry and re-read
/// identically on every reload, so it is stable across an evict→reload of the same logical worker.
static NEXT_QUEUE_ID: AtomicU64 = AtomicU64::new(0);

/// Why a flush is being performed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlushReason {
    /// Periodic flush triggered by the background sweeper.
    Background,
    /// Synchronous flush forced at a lifecycle boundary (suspend / stop / reattach) or because
    /// background flushing is disabled.
    Forced,
}

impl FlushReason {
    fn as_str(&self) -> &'static str {
        match self {
            FlushReason::Background => "background",
            FlushReason::Forced => "forced",
        }
    }
}

/// Baseline state guarded by the flush lock. Held only while a flush is in progress.
struct FlushBaseline {
    /// The last status successfully written to the cache, used to compute minimal deltas. Only
    /// meaningful when `base_known` is `true`.
    last_flushed: AgentStatusRecord,
    /// Whether `last_flushed` actually reflects what is persisted in the KV store. `false` after a
    /// folded cache-hit load or a detach, forcing the next flush to perform a full reconcile write
    /// (`previous = None`) instead of a delta against a baseline that was never persisted.
    base_known: bool,
}

/// Per-worker coordinator that batches cached-status-blob writes in the background.
pub struct AgentStatusFlusher {
    /// Unique id of this flusher instance, used as the dirty-queue key.
    queue_id: u64,
    owned_agent_id: OwnedAgentId,
    /// Ephemeral workers never persist a cached status; every operation is a no-op for them.
    is_ephemeral: bool,
    /// When `false`, blob writes happen synchronously on the hot path (historical behaviour); when
    /// `true`, the hot path only marks the worker dirty and the sweeper performs the write.
    background_enabled: bool,

    worker_service: Arc<dyn WorkerService>,
    queue: Arc<AgentStatusFlushQueue>,

    /// The live in-memory status, shared with the owning `Worker` (its `last_known_status`).
    current_status: Arc<ArcSwap<AgentStatusRecord>>,
    /// Whether the worker's status is currently detached from the oplog, shared with the owning
    /// `Worker`. While detached the in-memory status is not authoritative, so flushing is skipped.
    detached: Arc<AtomicBool>,

    /// Weak handle to self, used to (re-)enqueue into the dirty queue.
    self_weak: Weak<AgentStatusFlusher>,

    /// Serializes flushes and guards the persisted baseline.
    baseline: Mutex<FlushBaseline>,
    /// Whether there are unflushed status changes. Source of truth for the dirty queue.
    dirty: AtomicBool,
    /// Set once the worker starts deleting; prevents a concurrent background flush from resurrecting
    /// the blob after `remove_cached_status` has deleted it.
    delete_started: AtomicBool,
}

impl AgentStatusFlusher {
    pub fn new(
        owned_agent_id: OwnedAgentId,
        is_ephemeral: bool,
        background_enabled: bool,
        worker_service: Arc<dyn WorkerService>,
        queue: Arc<AgentStatusFlushQueue>,
        current_status: Arc<ArcSwap<AgentStatusRecord>>,
        detached: Arc<AtomicBool>,
    ) -> Arc<Self> {
        Arc::new_cyclic(|self_weak| Self {
            queue_id: NEXT_QUEUE_ID.fetch_add(1, Ordering::Relaxed),
            owned_agent_id,
            is_ephemeral,
            background_enabled,
            worker_service,
            queue,
            current_status,
            detached,
            self_weak: self_weak.clone(),
            baseline: Mutex::new(FlushBaseline {
                last_flushed: AgentStatusRecord::default(),
                // The cache (if any) holds whatever checkpoint the worker was loaded from, which the
                // in-memory status has typically been folded past; the first flush must therefore
                // be a full reconcile write.
                base_known: false,
            }),
            dirty: AtomicBool::new(false),
            delete_started: AtomicBool::new(false),
        })
    }

    /// Called from the hot path whenever the in-memory status changed. Updates the recovery index
    /// synchronously (only when the tracking predicate transitions) and then either marks the
    /// worker dirty for the background sweeper or, when background flushing is disabled, flushes the
    /// blob synchronously.
    pub async fn on_status_changed(
        &self,
        previous_status: &AgentStatusRecord,
        new_status: &AgentStatusRecord,
    ) {
        if self.is_ephemeral {
            return;
        }

        // Recovery index: keep it synchronous (it must be timely for crash/reshard recovery), but
        // only touch the KV store when the predicate actually changes, to avoid a round-trip per
        // commit.
        let track_now = DefaultWorkerService::should_track_for_assignment_recovery(new_status);
        let track_before =
            DefaultWorkerService::should_track_for_assignment_recovery(previous_status);
        if track_now != track_before {
            self.worker_service
                .set_assignment_tracking(&self.owned_agent_id, new_status)
                .await;
        }

        // Blob: defer to the sweeper, or write inline if background flushing is off.
        if self.background_enabled {
            self.mark_dirty();
        } else {
            // Disabled mode mirrors the historical "write on every commit" behaviour. A failed
            // write is logged, metered and re-queued inside `flush`; it is not fatal because the
            // blob is fully reconstructable from the oplog (the source of truth), and the
            // background sweeper — spawned regardless of this setting — will retry the re-queued
            // entry.
            let _ = self.flush(FlushReason::Forced).await;
        }
    }

    /// Marks the worker dirty and ensures it is enqueued for the next sweeper tick. The `dirty`
    /// flag is the source of truth; the queue entry is just a wakeup, so we only enqueue on the
    /// clean→dirty transition.
    fn mark_dirty(&self) {
        if self.is_ephemeral || self.delete_started.load(Ordering::Acquire) {
            return;
        }
        if !self.dirty.swap(true, Ordering::AcqRel) {
            self.queue.enqueue(self.queue_id, self.self_weak.clone());
        }
    }

    /// Marks the in-memory status as no longer reflected by any persisted baseline, forcing the
    /// next flush to perform a full reconcile write. Called when the status detaches from the oplog
    /// (e.g. during a revert or snapshot update).
    pub async fn invalidate_baseline(&self) {
        self.baseline.lock().await.base_known = false;
    }

    /// Persists the current in-memory status to the cache (if dirty / forced). Safe to call
    /// concurrently; flushes are serialized by the baseline lock.
    ///
    /// Returns `Err` if the underlying KV write failed (the worker is left dirty and re-queued for
    /// a later retry). A failure is never fatal — the blob is fully reconstructable from the oplog —
    /// so lifecycle callers treat it as best-effort, but the `Result` is surfaced so callers can
    /// log/observe it rather than silently assuming the flush succeeded.
    pub async fn flush(&self, reason: FlushReason) -> Result<(), String> {
        if self.is_ephemeral {
            return Ok(());
        }

        let mut baseline = self.baseline.lock().await;

        // Authoritative early-outs under the lock.
        if self.delete_started.load(Ordering::Acquire) {
            self.dirty.store(false, Ordering::Release);
            return Ok(());
        }
        if self.detached.load(Ordering::Acquire) {
            // The in-memory status is not authoritative while detached; reattach will recompute and
            // force a full flush.
            self.dirty.store(false, Ordering::Release);
            return Ok(());
        }

        // A background sweep can pick up a stale queue entry left behind by an intervening forced
        // flush (forced flushes do not drain the queue). If there is nothing to flush, skip the
        // (otherwise unconditional) snapshot + write. Forced flushes always proceed so lifecycle
        // boundaries are guaranteed to reconcile even when `dirty` was already cleared.
        if reason == FlushReason::Background && !self.dirty.load(Ordering::Acquire) {
            return Ok(());
        }

        // Clear `dirty` BEFORE snapshotting `current_status` so that any update concurrent with the
        // write re-sets `dirty` (and re-enqueues), guaranteeing it is not lost.
        self.dirty.store(false, Ordering::Release);

        let snapshot = self.current_status.load_full().as_ref().clone();
        let previous = if baseline.base_known {
            Some(&baseline.last_flushed)
        } else {
            None
        };

        match self
            .worker_service
            .write_cached_status(&self.owned_agent_id, previous, snapshot)
            .await
        {
            Ok(flushed) => {
                baseline.last_flushed = flushed;
                baseline.base_known = true;
                crate::metrics::workers::record_agent_status_flush(reason.as_str());
                Ok(())
            }
            Err(err) => {
                error!(
                    agent_id = %self.owned_agent_id,
                    reason = reason.as_str(),
                    "Failed to flush cached agent status, will retry: {err}"
                );
                crate::metrics::workers::record_agent_status_flush_failed(reason.as_str());
                // Restore the dirty flag and re-enqueue so the sweeper retries.
                self.dirty.store(true, Ordering::Release);
                if !self.delete_started.load(Ordering::Acquire) {
                    self.queue.enqueue(self.queue_id, self.self_weak.clone());
                }
                Err(err)
            }
        }
    }

    /// Marks the worker as deleting and waits for any in-flight flush to finish, so a concurrent
    /// background flush cannot resurrect the cache after it is deleted. Must be called (and awaited)
    /// before `WorkerService::remove`/`remove_cached_status`.
    ///
    /// Setting `delete_started` alone is not enough: a flush that already passed the
    /// `delete_started` early-out is mid-write while holding the baseline lock, and would complete
    /// its write after the delete removed the blob. Acquiring the baseline lock here acts as a
    /// barrier — once it is held, no flush is in progress, and every subsequent flush observes
    /// `delete_started == true` and bails before writing.
    pub async fn begin_delete(&self) {
        self.delete_started.store(true, Ordering::Release);
        self.dirty.store(false, Ordering::Release);
        // Barrier: blocks until any flush that started before `delete_started` was set completes.
        let _baseline = self.baseline.lock().await;
    }
}

/// Per-executor registry of dirty flushers, drained periodically by a background sweeper.
pub struct AgentStatusFlushQueue {
    dirty: StdMutex<HashMap<u64, Weak<AgentStatusFlusher>>>,
    max_concurrency: usize,
    background_handle: StdMutex<Option<JoinHandle<()>>>,
}

impl AgentStatusFlushQueue {
    /// Creates the queue and spawns the background sweeper. The sweeper stops when the
    /// `shutdown_token` is cancelled or when the queue itself is dropped.
    pub fn new(
        interval: Duration,
        max_concurrency: usize,
        shutdown_token: CancellationToken,
    ) -> Arc<Self> {
        let queue = Arc::new(Self {
            dirty: StdMutex::new(HashMap::new()),
            max_concurrency: max_concurrency.max(1),
            background_handle: StdMutex::new(None),
        });

        let weak = Arc::downgrade(&queue);
        let handle = tokio::spawn(
            async move {
                loop {
                    tokio::select! {
                        _ = shutdown_token.cancelled() => {
                            debug!("Shutdown requested, draining agent status flush queue once before stopping");
                            // Best-effort final drain so a graceful shutdown leaves the cache as
                            // fresh as possible. Not required for correctness (the oplog is the
                            // source of truth and a cold load re-folds), but avoids an avoidable
                            // re-fold on next load of every dirty worker.
                            if let Some(queue) = weak.upgrade() {
                                queue.sweep().await;
                            }
                            break;
                        }
                        _ = tokio::time::sleep(interval) => {}
                    }
                    match weak.upgrade() {
                        Some(queue) => queue.sweep().await,
                        None => break,
                    }
                }
            }
            .instrument(span!(parent: None, Level::INFO, "Agent status flush sweeper")),
        );
        *queue.background_handle.lock().unwrap() = Some(handle);
        queue
    }

    /// Inserts (or replaces) the dirty-queue entry for the given flusher. Keyed by the flusher's
    /// unique `queue_id`, so re-enqueues only ever target this flusher's own slot.
    fn enqueue(&self, queue_id: u64, flusher: Weak<AgentStatusFlusher>) {
        self.dirty.lock().unwrap().insert(queue_id, flusher);
    }

    /// Drains all currently-dirty flushers and flushes them with bounded concurrency. Entries
    /// enqueued while a sweep is running are picked up by the next tick.
    async fn sweep(&self) {
        let entries: Vec<Weak<AgentStatusFlusher>> = {
            let mut dirty = self.dirty.lock().unwrap();
            dirty.drain().map(|(_, weak)| weak).collect()
        };
        if entries.is_empty() {
            return;
        }

        futures::stream::iter(entries)
            .for_each_concurrent(self.max_concurrency, |weak| async move {
                if let Some(flusher) = weak.upgrade() {
                    // Failures are logged, metered and re-queued inside `flush`; nothing more to do
                    // here (the next sweep will retry).
                    let _ = flusher.flush(FlushReason::Background).await;
                }
            })
            .await;
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.dirty.lock().unwrap().len()
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
    use golem_common::model::{AgentId, AgentStatus, AgentStatusRecord};
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
        async fn get(&self, _owned_agent_id: &OwnedAgentId) -> Option<GetWorkerMetadataResult> {
            unimplemented!()
        }
        async fn get_running_workers_in_shards(&self) -> Vec<GetWorkerMetadataResult> {
            unimplemented!()
        }
        async fn remove(&self, _owned_agent_id: &OwnedAgentId) {}
        async fn remove_cached_status(&self, _owned_agent_id: &OwnedAgentId) {}
        async fn get_agent_mode(&self, _owned_agent_id: &OwnedAgentId) -> Option<AgentMode> {
            None
        }
        async fn write_cached_status(
            &self,
            _owned_agent_id: &OwnedAgentId,
            previous_status: Option<&AgentStatusRecord>,
            status_value: AgentStatusRecord,
        ) -> Result<AgentStatusRecord, String> {
            let mut state = self.state.lock().unwrap();
            if state.fail_writes {
                return Err("injected failure".to_string());
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
            Ok(checkpoint)
        }
        async fn set_assignment_tracking(
            &self,
            _owned_agent_id: &OwnedAgentId,
            status_value: &AgentStatusRecord,
        ) {
            self.state
                .lock()
                .unwrap()
                .tracking_calls
                .push(status_value.status);
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
        let current = Arc::new(ArcSwap::from_pointee(status(AgentStatus::Idle, 0)));
        let detached = Arc::new(AtomicBool::new(false));
        let flusher = AgentStatusFlusher::new(
            agent_id(),
            is_ephemeral,
            background_enabled,
            worker_service,
            queue,
            current.clone(),
            detached.clone(),
        );
        (flusher, current, detached)
    }

    fn test_queue() -> Arc<AgentStatusFlushQueue> {
        // Long interval so the background sweeper never fires; tests drive `sweep` manually.
        AgentStatusFlushQueue::new(Duration::from_secs(3600), 16, CancellationToken::new())
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
}
