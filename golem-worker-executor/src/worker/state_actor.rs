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

//! The worker-state actor: owns the worker state that escapes the wasm store's exclusive
//! `&mut WorkerCtx` discipline and is therefore shared with `'static` host futures.
//!
//! Host code reaches this state from three execution contexts that must never block each other
//! through shared lock ownership:
//!
//! * futures polled by wasmtime's store event loop (concurrent p3 durable host calls),
//! * host code running on wasm fibers that suspend while *keeping the store* (async libcalls
//!   such as the `memory.grow` resource limiter), and
//! * independent tokio tasks (the invocation loop, gRPC handlers, background services).
//!
//! While a store-keeping fiber is suspended, the event loop cannot poll any store-polled future
//! (wasmtime's documented store-blocking limitation, wasmtime#11869/#11870). Tokio's fair locks
//! hand ownership to a queued waiter at wake time, *before it is polled*, so a store-polled
//! future that merely queues on a shared lock can become its unpollable owner and wedge the
//! whole store. The actor shape eliminates this class by construction: callers only send jobs
//! over a channel and await oneshot replies, which never make an unpolled caller the owner of
//! anything, and the actor tasks (polled directly by tokio) always make progress.
//!
//! The actor runs **two independent job queues** because their deadlock disciplines differ:
//!
//! * The **status queue** serializes the oplog-commit + status-fold transaction (previously
//!   guarded by the `update_state_lock` mutex). Its task must never await anything completed by
//!   a store event loop and must never take the worker's `instance` lock: callers holding the
//!   instance lock await status jobs (e.g. `Worker::add_and_commit_oplog_internal`), so taking
//!   that lock here would deadlock. It only performs oplog-actor roundtrips, storage/network IO,
//!   and lock-free status publication.
//! * The **lifecycle queue** runs notification and memory-accounting jobs. Jobs that take the
//!   `instance` lock are fire-and-forget. Ordered oplog entries are awaitable but never take that
//!   lock, so an instance-lock holder never waits on a lifecycle operation that needs the same
//!   lock. A cancellation-safe status transaction may own guards acquired by its caller, but the
//!   status task never acquires those locks itself.
//!
//! The status task is also the **only writer** of the worker's published status
//! (`last_known_status`, an `ArcSwap`) and its `detached` flag; every other component reads them
//! lock-free.

use super::status::{calculate_last_known_status_with_checkpoint, update_status_with_new_entries};
use super::status_flusher::{AgentStatusFlusher, FlushReason};
use super::{
    PendingMemoryGrowth, UnloadReason, Worker, WorkerCommand, WorkerInstance, WorkerStatusMetric,
};
use crate::services::linear_memory::LinearMemoryTracker;
use crate::services::oplog::{CommitLevel, Oplog};
use crate::services::{All, HasConfig, HasSchedulerService};
use crate::workerctx::WorkerCtx;
use arc_swap::ArcSwap;
use chrono::Utc;
use golem_common::model::account::AccountId;
use golem_common::model::agent::AgentMode;
use golem_common::model::oplog::{OplogEntry, OplogIndex};
use golem_common::model::{
    AgentStatus, AgentStatusRecord, OwnedAgentId, ScheduledAction, Timestamp,
};
use golem_service_base::error::worker_executor::InterruptKind;
use std::any::Any;
use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use tokio::sync::{Mutex, OwnedMutexGuard, mpsc, oneshot};
use tracing::debug;

/// Handle to the worker-state actor's two job queues. Owned by [`Worker`]; dropping it aborts
/// both tasks.
pub(super) struct WorkerStateActor<Ctx: WorkerCtx> {
    commit: Arc<OwnerCommitController>,
    lifecycle_jobs: mpsc::UnboundedSender<LifecycleJob<Ctx>>,
    notification_queued: Arc<AtomicBool>,
    status_task: tokio::task::JoinHandle<()>,
    lifecycle_task: tokio::task::JoinHandle<()>,
    owned_agent_id: OwnedAgentId,
}

/// Owner-scoped handle for serializing oplog commits with status publication.
///
/// The controller communicates with the independently-polled status actor and never acquires the
/// primary Store or the worker instance lock. Primary and entity Stores can therefore commit the
/// shared owner oplog while another Store is suspended in a guest call.
pub(crate) struct OwnerCommitController {
    status_jobs: mpsc::UnboundedSender<StatusJob>,
    owned_agent_id: OwnedAgentId,
}

/// A request processed by the status task, which exclusively owns the commit + status-fold
/// transaction. Jobs are processed strictly in enqueue order, giving the same serialization the
/// former `update_state_lock` mutex provided — without lock-ownership handoff to potentially
/// unpollable callers.
enum StatusJob {
    /// Commits the oplog and folds the newly committed entries into the published status.
    /// Replies with the current oplog index after the commit and whether the status changed.
    /// The reply deliberately does not depend on the instance lock; if the caller wants the
    /// invocation loop notified about the change, it enqueues a lifecycle job afterwards.
    CommitAndUpdateState {
        level: CommitLevel,
        committed: Option<oneshot::Sender<()>>,
        done: oneshot::Sender<(OplogIndex, bool)>,
    },
    /// Appends an entry and completes its commit + fold transaction even if the caller is
    /// cancelled. The caller-acquired guards remain owned by this job until the transaction ends.
    AppendAndCommitAttached {
        entry: Box<OplogEntry>,
        _worker_keepalive: Arc<dyn Any + Send + Sync>,
        _instance_guard: OwnedMutexGuard<WorkerInstance>,
        _card_event_boundary_guard: OwnedMutexGuard<()>,
        done: oneshot::Sender<()>,
    },
    AppendInvocationIfVersion {
        entry: Box<OplogEntry>,
        expected_oplog_index: OplogIndex,
        expected_revert_generation: u64,
        instance_guard: OwnedMutexGuard<WorkerInstance>,
        done: oneshot::Sender<bool>,
    },
    /// Returns the published status after reattaching it when a jump or revert detached it.
    /// Serialization on the status queue prevents observing an in-flight status transition.
    AttachedStatus {
        done: oneshot::Sender<Arc<AgentStatusRecord>>,
    },
    /// Returns the published status if it is currently attached to the oplog, `None` if it is
    /// detached. Runs on the status queue so it cannot observe the detached window of an
    /// in-flight commit or reattach transaction. The attached-ness check happens on the actor,
    /// but the caller decides how to react (it asserts): a job whose caller was cancelled must
    /// not be able to panic the actor.
    NonDetachedStatus {
        done: oneshot::Sender<Option<AgentStatusRecord>>,
    },
    /// Commits, then — if the status became detached (a jump or revert made it non-foldable) —
    /// recomputes it from the oplog, republishes it, and forces a cache flush.
    Reattach { done: oneshot::Sender<()> },
}

/// A request processed by the lifecycle task. Notifications and ordinary growth persistence are
/// fire-and-forget. Ordered oplog entries await a reply but never take the worker's `instance`
/// lock, so it remains safe for store-polled callers.
enum LifecycleJob<Ctx: WorkerCtx> {
    /// Wakes the invocation loop after a commit changed the published status.
    NotifyStatusChanged,
    /// Records a `GrowMemory` oplog hint after a guest growth has committed.
    GrowMemory {
        worker: Arc<Worker<Ctx>>,
        growth: Arc<PendingMemoryGrowth>,
    },
    OrderedOplogEntry {
        worker: Arc<Worker<Ctx>>,
        entry: Box<OplogEntry>,
        done: oneshot::Sender<()>,
    },
    MemoryLimitExceeded {
        worker: Arc<Worker<Ctx>>,
        memory: LinearMemoryTracker,
    },
}

/// The state exclusively owned by the status task.
struct StatusState<Ctx: WorkerCtx> {
    deps: All<Ctx>,
    owned_agent_id: OwnedAgentId,
    agent_mode: AgentMode,
    created_by: AccountId,
    oplog: Arc<dyn Oplog>,
    /// The published worker status. Written only by this task (and during worker construction,
    /// before the actor exists); read lock-free everywhere else.
    last_known_status: Arc<ArcSwap<AgentStatusRecord>>,
    /// Whether the published status is detached from the oplog (no longer incrementally
    /// foldable). Written only by this task; read lock-free elsewhere.
    detached: Arc<AtomicBool>,
    metrics_status: Arc<WorkerStatusMetric>,
    status_flusher: Arc<AgentStatusFlusher>,
    /// Monotonically publishes committed authority changes after the matching
    /// status fold. Live host-call authorization uses this as its lock-free
    /// invalidation signal.
    published_authority_generation: Arc<AtomicU64>,
}

impl<Ctx: WorkerCtx> Drop for WorkerStateActor<Ctx> {
    fn drop(&mut self) {
        // Status jobs that must outlive a cancelled caller hold a strong `Arc<Worker>`, so the
        // actor cannot be dropped while such a job is queued or running. Lifecycle jobs that need
        // the worker likewise hold a strong `Arc<Worker>`; other lifecycle jobs are harmless to
        // discard when the worker goes away.
        self.status_task.abort();
        self.lifecycle_task.abort();
    }
}

impl<Ctx: WorkerCtx> WorkerStateActor<Ctx> {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        deps: All<Ctx>,
        owned_agent_id: OwnedAgentId,
        agent_mode: AgentMode,
        created_by: AccountId,
        oplog: Arc<dyn Oplog>,
        last_known_status: Arc<ArcSwap<AgentStatusRecord>>,
        detached: Arc<AtomicBool>,
        metrics_status: Arc<WorkerStatusMetric>,
        status_flusher: Arc<AgentStatusFlusher>,
        published_authority_generation: Arc<AtomicU64>,
        instance: Arc<Mutex<WorkerInstance>>,
    ) -> Self {
        let state = StatusState {
            deps,
            owned_agent_id: owned_agent_id.clone(),
            agent_mode,
            created_by,
            oplog,
            last_known_status,
            detached,
            metrics_status,
            status_flusher,
            published_authority_generation,
        };

        let (status_jobs, mut status_rx) = mpsc::unbounded_channel::<StatusJob>();
        let status_task = tokio::spawn(async move {
            while let Some(job) = status_rx.recv().await {
                match job {
                    StatusJob::CommitAndUpdateState {
                        level,
                        committed,
                        done,
                    } => {
                        complete_status_job(
                            async {
                                let changed = state.commit_and_update_state(level, committed).await;
                                let index = state.oplog.current_oplog_index().await;
                                (index, changed)
                            },
                            done,
                        )
                        .await;
                    }
                    StatusJob::AppendAndCommitAttached {
                        entry,
                        _worker_keepalive,
                        _instance_guard,
                        _card_event_boundary_guard,
                        done,
                    } => {
                        complete_status_job(
                            async {
                                state.oplog.add(*entry).await;
                                state
                                    .commit_and_update_state(CommitLevel::Always, None)
                                    .await;
                                state.ensure_status_attached().await;
                            },
                            done,
                        )
                        .await;
                    }
                    StatusJob::AppendInvocationIfVersion {
                        entry,
                        expected_oplog_index,
                        expected_revert_generation,
                        instance_guard,
                        done,
                    } => {
                        complete_status_job(
                            async {
                                state.ensure_status_attached().await;
                                let status = state.last_known_status.load();
                                if status.oplog_idx != expected_oplog_index
                                    || status.invocation_results.revert_generation()
                                        != expected_revert_generation
                                {
                                    return false;
                                }
                                drop(status);
                                state.oplog.add(*entry).await;
                                state
                                    .commit_and_update_state(CommitLevel::Always, None)
                                    .await;
                                if let WorkerInstance::Running(running) = &*instance_guard {
                                    running.sender.send(WorkerCommand::WorkAvailable).unwrap();
                                }
                                true
                            },
                            done,
                        )
                        .await;
                    }
                    StatusJob::AttachedStatus { done } => {
                        if state.detached.load(Ordering::Acquire) {
                            state.reattach().await;
                        }
                        let _ = done.send(state.last_known_status.load_full());
                    }
                    StatusJob::NonDetachedStatus { done } => {
                        let status = if state.detached.load(Ordering::Acquire) {
                            None
                        } else {
                            Some(state.last_known_status.load_full().as_ref().clone())
                        };
                        let _ = done.send(status);
                    }
                    StatusJob::Reattach { done } => {
                        state.reattach().await;
                        let _ = done.send(());
                    }
                }
            }
        });

        let notification_queued = Arc::new(AtomicBool::new(false));
        let notification_queued_task = notification_queued.clone();
        let (lifecycle_jobs, mut lifecycle_rx) = mpsc::unbounded_channel::<LifecycleJob<Ctx>>();
        let lifecycle_task = tokio::spawn(async move {
            while let Some(job) = lifecycle_rx.recv().await {
                match job {
                    LifecycleJob::NotifyStatusChanged => {
                        let instance_guard = instance.lock().await;
                        notification_queued_task.store(false, Ordering::Release);
                        if let WorkerInstance::Running(running) = &*instance_guard {
                            let _ = running.sender.send(WorkerCommand::InternalStatusChanged);
                        }
                    }
                    LifecycleJob::GrowMemory { worker, growth } => {
                        worker.persist_pending_memory_growth(growth).await;
                    }
                    LifecycleJob::OrderedOplogEntry {
                        worker,
                        entry,
                        done,
                    } => {
                        worker.add_and_commit_oplog(*entry).await;
                        let _ = done.send(());
                    }
                    LifecycleJob::MemoryLimitExceeded { worker, memory } => {
                        worker
                            .memory_limit_interrupt_queued
                            .store(false, Ordering::Release);
                        if memory.exceeds_current_limit() {
                            worker
                                .set_interrupting_for(
                                    InterruptKind::Suspend(Timestamp::now_utc()),
                                    UnloadReason::MemoryLimit,
                                )
                                .await;
                        }
                    }
                }
            }
        });

        Self {
            commit: Arc::new(OwnerCommitController {
                status_jobs,
                owned_agent_id: owned_agent_id.clone(),
            }),
            lifecycle_jobs,
            notification_queued,
            status_task,
            lifecycle_task,
            owned_agent_id,
        }
    }

    /// Commits the oplog and folds the new entries into the published status. Returns the
    /// current oplog index after the commit and whether the status changed.
    ///
    /// If the caller's future is dropped while awaiting the reply, the commit still runs to
    /// completion on the status task (the same semantics as the oplog actor's own jobs).
    pub async fn commit_and_update_state(&self, level: CommitLevel) -> (OplogIndex, bool) {
        self.commit
            .run_status_job(|done| StatusJob::CommitAndUpdateState {
                level,
                committed: None,
                done,
            })
            .await
    }

    pub async fn commit_and_update_state_notifying(
        &self,
        level: CommitLevel,
        committed: oneshot::Sender<()>,
    ) -> (OplogIndex, bool) {
        self.commit
            .run_status_job(|done| StatusJob::CommitAndUpdateState {
                level,
                committed: Some(committed),
                done,
            })
            .await
    }

    pub async fn append_and_commit_attached(
        &self,
        entry: OplogEntry,
        worker: Arc<Worker<Ctx>>,
        instance_guard: OwnedMutexGuard<WorkerInstance>,
        card_event_boundary_guard: OwnedMutexGuard<()>,
    ) {
        let worker_keepalive: Arc<dyn Any + Send + Sync> = worker;
        self.commit
            .run_status_job(|done| StatusJob::AppendAndCommitAttached {
                entry: Box::new(entry),
                _worker_keepalive: worker_keepalive,
                _instance_guard: instance_guard,
                _card_event_boundary_guard: card_event_boundary_guard,
                done,
            })
            .await
    }

    pub async fn append_invocation_if_version(
        &self,
        entry: OplogEntry,
        expected_oplog_index: OplogIndex,
        expected_revert_generation: u64,
        instance_guard: OwnedMutexGuard<WorkerInstance>,
    ) -> bool {
        self.commit
            .run_status_job(|done| StatusJob::AppendInvocationIfVersion {
                entry: Box::new(entry),
                expected_oplog_index,
                expected_revert_generation,
                instance_guard,
                done,
            })
            .await
    }

    pub async fn attached_status(&self) -> Arc<AgentStatusRecord> {
        self.commit
            .run_status_job(|done| StatusJob::AttachedStatus { done })
            .await
    }

    /// Returns the published status, asserting it is attached to the oplog. Serialized behind
    /// any in-flight commit/reattach transactions. The assert lives here on the caller side, so
    /// a job left behind by a cancelled caller cannot panic the actor.
    pub async fn non_detached_status(&self) -> AgentStatusRecord {
        self.commit
            .run_status_job(|done| StatusJob::NonDetachedStatus { done })
            .await
            .expect("worker status was unexpectedly detached from the oplog")
    }

    /// Commits and, if the status is detached, recomputes and republishes it (see
    /// [`Worker::reattach_worker_status`]).
    pub async fn reattach_worker_status(&self) {
        self.commit
            .run_status_job(|done| StatusJob::Reattach { done })
            .await
    }

    pub fn owner_commit_controller(&self) -> Arc<OwnerCommitController> {
        self.commit.clone()
    }

    /// Asks the lifecycle task to wake the invocation loop about a status change. Fire and
    /// forget: never blocks, and safe to call from store-polled futures and store-keeping
    /// fibers alike, because the instance lock is only taken on the lifecycle task.
    pub fn notify_status_changed(&self) {
        if !self.notification_queued.swap(true, Ordering::AcqRel)
            && self
                .lifecycle_jobs
                .send(LifecycleJob::NotifyStatusChanged)
                .is_err()
        {
            self.notification_queued.store(false, Ordering::Release);
        }
    }

    /// Asks the lifecycle task to record a committed guest `memory.grow` of `delta` bytes. Fire
    /// and forget: called from the `memory.grow` resource limiter, which runs on a store-keeping
    /// fiber and must not await anything.
    pub fn grow_memory(&self, worker: Arc<Worker<Ctx>>, growth: Arc<PendingMemoryGrowth>) {
        if self
            .lifecycle_jobs
            .send(LifecycleJob::GrowMemory { worker, growth })
            .is_err()
        {
            panic!(
                "Worker state actor for {} terminated unexpectedly",
                self.owned_agent_id
            );
        }
    }

    pub fn memory_limit_exceeded(&self, worker: Arc<Worker<Ctx>>, memory: LinearMemoryTracker) {
        if self
            .lifecycle_jobs
            .send(LifecycleJob::MemoryLimitExceeded { worker, memory })
            .is_err()
        {
            panic!(
                "Worker state actor for {} terminated unexpectedly",
                self.owned_agent_id
            );
        }
    }

    pub fn queue_ordered_oplog_entry(
        &self,
        worker: Arc<Worker<Ctx>>,
        entry: OplogEntry,
    ) -> oneshot::Receiver<()> {
        let (done, done_rx) = oneshot::channel();
        if self
            .lifecycle_jobs
            .send(LifecycleJob::OrderedOplogEntry {
                worker,
                entry: Box::new(entry),
                done,
            })
            .is_err()
        {
            panic!(
                "Worker state actor for {} terminated unexpectedly",
                self.owned_agent_id
            );
        }
        done_rx
    }
}

impl OwnerCommitController {
    pub async fn commit_and_update_state(&self, level: CommitLevel) -> (OplogIndex, bool) {
        self.run_status_job(|done| StatusJob::CommitAndUpdateState {
            level,
            committed: None,
            done,
        })
        .await
    }

    /// Sends a job to the status task and waits for its reply.
    ///
    /// Panics if the task is gone: it is only aborted from `Drop` (when no caller can be in
    /// flight anymore), so a missing reply means the task itself panicked and the worker's
    /// status state is no longer trustworthy.
    async fn run_status_job<R>(&self, make_job: impl FnOnce(oneshot::Sender<R>) -> StatusJob) -> R {
        let (done, done_rx) = oneshot::channel();
        if self.status_jobs.send(make_job(done)).is_err() {
            panic!(
                "Worker state actor for {} terminated unexpectedly",
                self.owned_agent_id
            );
        }
        match done_rx.await {
            Ok(result) => result,
            Err(_) => panic!(
                "Worker state actor for {} dropped a request without replying",
                self.owned_agent_id
            ),
        }
    }
}

async fn complete_status_job<R>(transaction: impl Future<Output = R>, done: oneshot::Sender<R>) {
    let result = transaction.await;
    let _ = done.send(result);
}

impl<Ctx: WorkerCtx> StatusState<Ctx> {
    /// The commit + status-fold transaction. Commits the oplog, then either folds the newly
    /// committed entries into the published status or marks the status detached when it can no
    /// longer be incrementally computed (e.g. after a revert or a snapshot update). Returns
    /// whether the published status (or its detachment) changed.
    async fn commit_and_update_state(
        &self,
        commit_level: CommitLevel,
        committed: Option<oneshot::Sender<()>>,
    ) -> bool {
        let new_entries = self.oplog.commit(commit_level).await;
        if let Some(committed) = committed {
            let _ = committed.send(());
        }

        let authority_change_count = new_entries
            .values()
            .filter(|entry| is_authority_state_entry(entry))
            .count() as u64;

        let changed = if !self.detached.load(Ordering::Acquire) {
            let old_status = self.last_known_status.load_full();

            let updated_status = update_status_with_new_entries(
                self.agent_mode,
                old_status.as_ref().clone(),
                new_entries,
                &self.deps.config().retry,
            );

            if let Some(updated_status) = updated_status {
                if updated_status != *old_status {
                    self.update_last_known_status(updated_status.clone()).await;

                    self.schedule_oplog_archive_if_needed(&old_status, &updated_status)
                        .await;

                    true
                } else {
                    false
                }
            } else {
                // The status can no longer be incrementally computed by adding the new oplog entries, instead a full reload needs to be performed.
                // This can happen during a revert or a snapshot update for example.
                debug!(agent_id = %self.owned_agent_id.agent_id, "Detaching worker_status from oplog");
                self.detached.store(true, Ordering::Release);
                // The in-memory status is no longer authoritative, and after reattach it will be
                // recomputed from scratch, so the persisted baseline can no longer be trusted: the
                // next flush must be a full reconcile write.
                self.status_flusher.invalidate_baseline().await;
                true
            }
        } else {
            false
        };

        // This release-publish is deliberately owned by the cancellation-proof
        // status actor and happens only after the committed entries were folded
        // (or the status was marked detached). A producer cannot report success
        // for a committed card event while authorization still considers the
        // old generation current.
        if authority_change_count != 0 {
            self.published_authority_generation
                .fetch_add(authority_change_count, Ordering::Release);
        }

        changed
    }

    async fn reattach(&self) {
        self.commit_and_update_state(CommitLevel::Always, None)
            .await;

        self.ensure_status_attached().await;
    }

    async fn ensure_status_attached(&self) -> bool {
        if self.detached.load(Ordering::Acquire) {
            debug!(
                agent_id = %self.owned_agent_id.agent_id,
                "Worker status was detached from oplog, recomputing it"
            );

            let worker_status = calculate_last_known_status_with_checkpoint(
                &self.deps,
                &self.owned_agent_id,
                self.agent_mode,
                None,
            )
            .await
            .expect("Failed to recompute worker status for existing worker");

            // Install the recomputed status while still detached, so a concurrent background sweep
            // keeps skipping (the in-memory status is not authoritative until it is installed).
            self.update_last_known_status(worker_status.clone()).await;

            // Now the in-memory status is authoritative again; clear the flag and force a flush.
            // Release ordering pairs with the Acquire loads in the checkpoint/flusher paths: with
            // the lock-free ArcSwap status, this flag is the publication barrier that makes the
            // recomputed status visible before readers start trusting it again.
            self.detached.store(false, Ordering::Release);

            // The status was recomputed from an earlier baseline; persist it synchronously so the
            // cache is immediately usable again. A failure remains best-effort because the oplog
            // is authoritative and the flusher will retry it in the background.
            if let Err(err) = self.status_flusher.flush(FlushReason::Forced).await {
                debug!(
                    agent_id = %self.owned_agent_id.agent_id,
                    "Forced status flush on reattach failed (will retry in background): {err}"
                );
            }

            true
        } else {
            false
        }
    }

    /// Publishes a new status and hands the (previous, new) pair to the flusher, which updates
    /// the `RunningWorkers` recovery index synchronously and either marks the worker dirty for
    /// the background sweeper or writes the blob inline (when background flushing is disabled).
    async fn update_last_known_status(&self, new_status: AgentStatusRecord) {
        let previous_metrics_status = self.metrics_status.status();
        let previous_status = self.last_known_status.swap(Arc::new(new_status.clone()));
        self.metrics_status
            .update(previous_metrics_status, new_status.status);
        self.status_flusher
            .on_status_changed(&previous_status, &new_status)
            .await;
    }

    async fn schedule_oplog_archive_if_needed(
        &self,
        old_status: &AgentStatusRecord,
        new_status: &AgentStatusRecord,
    ) {
        if old_status.status != new_status.status
            && matches!(
                new_status.status,
                AgentStatus::Idle | AgentStatus::Failed | AgentStatus::Exited
            )
        {
            let archive_interval = self.deps.config().oplog.archive_interval;
            let last_oplog_index = new_status.oplog_idx;

            debug!(
                worker_id = %self.owned_agent_id,
                new_status = ?new_status.status,
                "Scheduling ArchiveOplog after status transition"
            );

            self.deps
                .scheduler_service()
                .schedule(
                    Utc::now() + archive_interval,
                    ScheduledAction::ArchiveOplog {
                        account_id: self.created_by,
                        owned_agent_id: self.owned_agent_id.clone(),
                        agent_mode: self.agent_mode,
                        last_oplog_index,
                        next_after: archive_interval,
                    },
                )
                .await;
        }
    }
}

fn is_authority_state_entry(entry: &OplogEntry) -> bool {
    matches!(
        entry,
        OplogEntry::CardEventQueued { .. }
            | OplogEntry::CardInstalled { .. }
            | OplogEntry::CardInstallFailed { .. }
            | OplogEntry::CardRevoked { .. }
            | OplogEntry::CardExpired { .. }
            | OplogEntry::CardDerived { .. }
            | OplogEntry::CardTransferStarted { .. }
            | OplogEntry::CardTransferred { .. }
            | OplogEntry::CardRevokedCascade { .. }
            | OplogEntry::CardTransferConfirmed { .. }
    )
}

#[cfg(test)]
mod tests {
    use super::complete_status_job;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU64, Ordering};
    use test_r::test;
    use tokio::sync::{Notify, oneshot};

    #[test]
    async fn authority_publication_survives_producer_cancellation_before_actor_reply() {
        let generation = Arc::new(AtomicU64::new(0));
        let committed = Arc::new(AtomicU64::new(0));
        let before_reply = Arc::new(Notify::new());
        let release_reply = Arc::new(Notify::new());
        let (done, done_rx) = oneshot::channel();

        let actor = tokio::spawn({
            let generation = generation.clone();
            let committed = committed.clone();
            let before_reply = before_reply.clone();
            let release_reply = release_reply.clone();
            async move {
                complete_status_job(
                    async move {
                        committed.store(1, Ordering::Release);
                        generation.store(1, Ordering::Release);
                        before_reply.notify_one();
                        release_reply.notified().await;
                    },
                    done,
                )
                .await;
            }
        });
        let producer = tokio::spawn(done_rx);

        before_reply.notified().await;
        producer.abort();
        release_reply.notify_one();
        actor
            .await
            .expect("status actor must finish the transaction");

        assert_eq!(committed.load(Ordering::Acquire), 1);
        assert_eq!(generation.load(Ordering::Acquire), 1);
    }
}
