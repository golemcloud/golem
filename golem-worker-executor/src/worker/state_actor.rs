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
//!   a store event loop and must never take the worker lifecycle lock: callers holding that lock
//!   await status jobs (e.g. `Worker::add_and_commit_oplog_internal`), so taking it here would
//!   deadlock. It only performs oplog-actor roundtrips, storage/network IO, and lock-free status
//!   publication.
//! * The **lifecycle queue** runs notification and memory-accounting jobs. Jobs that take the
//!   worker lifecycle-state lock are fire-and-forget. Ordered oplog entries are awaitable but
//!   never take that lock, so a lifecycle-state-lock holder never waits on a lifecycle operation
//!   that needs the same lock. A cancellation-safe status transaction may own guards acquired by
//!   its caller, but the status task never acquires those locks itself.
//!
//! The status task is also the **only writer** of the worker's published status
//! (`last_known_status`, an `ArcSwap`) and its `detached` flag; every other component reads them
//! lock-free.

use super::status::{
    calculate_last_known_status_with_checkpoint, try_fold_status_from,
    update_status_with_new_entries,
};
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
use futures::FutureExt;
use futures::future::{BoxFuture, Shared};
use golem_common::model::account::AccountId;
use golem_common::model::agent::AgentMode;
use golem_common::model::oplog::{OplogEntry, OplogIndex};
use golem_common::model::{
    AgentFingerprint, AgentStatus, AgentStatusRecord, IdempotencyKey, OwnedAgentId,
    ScheduledAction, Timestamp,
};
use golem_service_base::error::worker_executor::{InterruptKind, WorkerExecutorError};
use std::any::Any;
use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use tokio::sync::{Mutex, OwnedMutexGuard, mpsc, oneshot};
use tracing::debug;

/// Handle to the worker-state actor's two job queues. Dropping it requests ordered shutdown.
pub(super) struct WorkerStateActor<Ctx: WorkerCtx> {
    commit: Arc<OwnerCommitController>,
    lifecycle_jobs: mpsc::UnboundedSender<LifecycleJob<Ctx>>,
    notification_queued: Arc<AtomicBool>,
    stop: Arc<WorkerStateActorStop>,
    owned_agent_id: OwnedAgentId,
}

/// Cold shutdown control retained until both actors have actually exited. Oplog generations
/// may register a Weak handle without owning the actors or their oplog through pending jobs.
pub(crate) struct WorkerStateActorStop {
    request: Box<dyn Fn() + Send + Sync>,
    completion: Shared<BoxFuture<'static, Result<(), String>>>,
}

impl WorkerStateActorStop {
    fn new(
        request: impl Fn() + Send + Sync + 'static,
        lifecycle_task: tokio::task::JoinHandle<()>,
        status_jobs: mpsc::UnboundedSender<StatusJob>,
        status_task: tokio::task::JoinHandle<()>,
        finish_status: impl Future<Output = ()> + Send + 'static,
    ) -> Arc<Self> {
        let (done, completion) = oneshot::channel();
        let stop = Arc::new(Self {
            request: Box::new(request),
            completion: async move {
                completion.await.unwrap_or_else(|_| {
                    Err("Worker state actor completion driver ended without a result".into())
                })
            }
            .boxed()
            .shared(),
        });
        tokio::spawn({
            let stop = stop.clone();
            async move {
                let lifecycle_result = lifecycle_task.await;
                // Lifecycle work can submit status jobs. Even a lifecycle panic must let
                // previously accepted status work finish before the status actor exits.
                let _ = status_jobs.send(StatusJob::Stop);
                let status_result = status_task.await;
                finish_status.await;
                let result = match (lifecycle_result, status_result) {
                    (Ok(()), Ok(())) => Ok(()),
                    (Err(error), Ok(())) => Err(format!("Worker lifecycle actor failed: {error}")),
                    (Ok(()), Err(error)) => Err(format!("Worker status actor failed: {error}")),
                    (Err(lifecycle), Err(status)) => Err(format!(
                        "Worker lifecycle actor failed: {lifecycle}; worker status actor failed: {status}"
                    )),
                };
                let _ = done.send(result);
                drop(stop);
            }
        });
        stop
    }

    /// The caller must stop external producers before requesting the FIFO drain.
    pub fn request_stop(&self) {
        (self.request)();
    }

    /// Observes actual completion without initiating shutdown. Errors still mean both tasks exited.
    pub async fn wait(&self) -> Result<(), String> {
        self.completion.clone().await
    }

    /// The caller must not hold the WorkerInstance mutex while joining. A cold oplog lifecycle
    /// guard may remain held. Cancellation does not abandon the shared shutdown driver.
    pub async fn stop_and_wait(&self) -> Result<(), String> {
        self.request_stop();
        self.wait().await
    }
}

/// Owner-scoped handle for serializing oplog commits with status publication.
///
/// The controller communicates with the independently-polled status actor and never acquires the
/// primary Store or the worker lifecycle-state lock. Primary and entity Stores can therefore
/// commit the shared owner oplog while another Store is suspended in a guest call.
pub(crate) struct OwnerCommitController {
    status_jobs: mpsc::UnboundedSender<StatusJob>,
    owned_agent_id: OwnedAgentId,
}

/// A request processed by the status task, which exclusively owns the commit + status-fold
/// transaction. Jobs are processed strictly in enqueue order, giving the same serialization the
/// former `update_state_lock` mutex provided — without lock-ownership handoff to potentially
/// unpollable callers.
enum StatusJob {
    Stop,
    /// Commits the oplog and folds the newly committed entries into the published status.
    /// Replies with the current oplog index after the commit and whether the status changed.
    /// The reply deliberately does not depend on the worker lifecycle lock; if the caller wants the
    /// invocation loop notified about the change, it enqueues a lifecycle job afterwards.
    CommitAndUpdateState {
        level: CommitLevel,
        committed: Option<oneshot::Sender<()>>,
        notify_after_fold: bool,
        done: oneshot::Sender<(OplogIndex, bool)>,
    },
    /// Appends an entry and completes its commit + fold transaction even if the caller is
    /// cancelled. The caller-acquired guards remain owned by this job until the transaction ends.
    AppendAndCommitAttached {
        entry: Box<OplogEntry>,
        _worker_keepalive: Arc<dyn Any + Send + Sync>,
        _instance_guard: OwnedMutexGuard<WorkerInstance>,
        _card_event_boundary_guard: Option<OwnedMutexGuard<()>>,
        done: oneshot::Sender<Result<(), WorkerExecutorError>>,
    },
    AppendInvocationIfVersion {
        entry: Box<OplogEntry>,
        idempotency_key: IdempotencyKey,
        expected_result_generation: u64,
        expected_revert_generation: u64,
        instance_guard: OwnedMutexGuard<WorkerInstance>,
        done: oneshot::Sender<bool>,
    },
    /// Returns the published status after reattaching it when a jump or revert detached it.
    /// Serialization on the status queue prevents observing an in-flight status transition.
    AttachedStatus {
        done: oneshot::Sender<Result<Arc<AgentStatusRecord>, WorkerExecutorError>>,
    },
    /// Returns the published status if it is currently attached to the oplog, `None` if it is
    /// detached. Runs on the status queue so it cannot observe the detached window of an
    /// in-flight commit or reattach transaction. The attached-ness check happens on the actor,
    /// but the caller decides how to react (it asserts): a job whose caller was cancelled must
    /// not be able to panic the actor.
    NonDetachedStatus {
        done: oneshot::Sender<Option<Arc<AgentStatusRecord>>>,
    },
    /// Commits, then — if the status became detached (a jump or revert made it non-foldable) —
    /// recomputes it from the oplog, republishes it, and forces a cache flush.
    Reattach {
        done: oneshot::Sender<()>,
    },
}

/// A request processed by the lifecycle task. Notifications and ordinary growth persistence are
/// fire-and-forget. Ordered oplog entries await a reply but never take the worker's `instance`
/// lock, so it remains safe for store-polled callers.
enum LifecycleJob<Ctx: WorkerCtx> {
    Stop,
    Drain {
        done: oneshot::Sender<()>,
    },
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
    fingerprint: AgentFingerprint,
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
        self.stop.request_stop();
    }
}

impl<Ctx: WorkerCtx> WorkerStateActor<Ctx> {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        deps: All<Ctx>,
        owned_agent_id: OwnedAgentId,
        fingerprint: AgentFingerprint,
        agent_mode: AgentMode,
        created_by: AccountId,
        oplog: Arc<dyn Oplog>,
        last_known_status: Arc<ArcSwap<AgentStatusRecord>>,
        detached: Arc<AtomicBool>,
        metrics_status: Arc<WorkerStatusMetric>,
        status_flusher: Arc<AgentStatusFlusher>,
        published_authority_generation: Arc<AtomicU64>,
        lifecycle: Arc<Mutex<WorkerInstance>>,
    ) -> Self {
        let task_owner = oplog.task_owner().cloned();
        let state = StatusState {
            deps,
            owned_agent_id: owned_agent_id.clone(),
            fingerprint,
            agent_mode,
            created_by,
            oplog,
            last_known_status,
            detached,
            metrics_status,
            status_flusher: status_flusher.clone(),
            published_authority_generation,
        };

        let notification_queued = Arc::new(AtomicBool::new(false));
        let notification_queued_task = notification_queued.clone();
        let (lifecycle_jobs, mut lifecycle_rx) = mpsc::unbounded_channel::<LifecycleJob<Ctx>>();
        let status_lifecycle_jobs = lifecycle_jobs.clone();
        let status_notification_queued = notification_queued.clone();
        let (status_jobs, mut status_rx) = mpsc::unbounded_channel::<StatusJob>();
        let status_task = tokio::spawn(async move {
            while let Some(job) = status_rx.recv().await {
                match job {
                    StatusJob::Stop => break,
                    StatusJob::CommitAndUpdateState {
                        level,
                        committed,
                        notify_after_fold,
                        done,
                    } => {
                        let changed = state.commit_and_update_state(level, committed).await;
                        let index = state.oplog.current_oplog_index().await;
                        if changed && notify_after_fold {
                            queue_status_notification(
                                &status_lifecycle_jobs,
                                &status_notification_queued,
                            );
                        }
                        let _ = done.send((index, changed));
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
                                if state.detached.load(Ordering::Acquire) {
                                    Err(WorkerExecutorError::runtime(
                                        "Committed worker status could not be reconstructed",
                                    ))
                                } else {
                                    Ok(())
                                }
                            },
                            done,
                        )
                        .await;
                    }
                    StatusJob::AppendInvocationIfVersion {
                        entry,
                        idempotency_key,
                        expected_result_generation,
                        expected_revert_generation,
                        instance_guard,
                        done,
                    } => {
                        complete_status_job(
                            async {
                                state.ensure_status_attached().await;
                                let status = state.last_known_status.load();
                                if !can_append_invocation(
                                    &status,
                                    &idempotency_key,
                                    expected_result_generation,
                                    expected_revert_generation,
                                ) {
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
                        let result = if state.detached.load(Ordering::Acquire) {
                            Err(WorkerExecutorError::runtime(
                                "Worker status could not be reconstructed",
                            ))
                        } else {
                            Ok(state.last_known_status.load_full())
                        };
                        let _ = done.send(result);
                    }
                    StatusJob::NonDetachedStatus { done } => {
                        let status = if state.detached.load(Ordering::Acquire) {
                            None
                        } else {
                            Some(state.last_known_status.load_full())
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

        let lifecycle_task = tokio::spawn(async move {
            let mut drains = Vec::new();
            while let Some(job) = lifecycle_rx.recv().await {
                match job {
                    LifecycleJob::Stop => break,
                    LifecycleJob::Drain { done } => {
                        drains.push(done);
                    }
                    LifecycleJob::NotifyStatusChanged => {
                        let lifecycle_guard = lifecycle.lock().await;
                        notification_queued_task.store(false, Ordering::Release);
                        if let WorkerInstance::Running(running) = &*lifecycle_guard {
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
                // Earlier growth jobs can enqueue another pass behind a drain request.
                if lifecycle_rx.is_empty() {
                    for done in drains.drain(..) {
                        let _ = done.send(());
                    }
                }
            }
        });

        let stop = WorkerStateActorStop::new(
            {
                let lifecycle_jobs = lifecycle_jobs.clone();
                move || {
                    let _ = lifecycle_jobs.send(LifecycleJob::Stop);
                }
            },
            lifecycle_task,
            status_jobs.clone(),
            status_task,
            async move { status_flusher.begin_delete().await },
        );
        let actor = Self {
            commit: Arc::new(OwnerCommitController {
                status_jobs,
                owned_agent_id: owned_agent_id.clone(),
            }),
            lifecycle_jobs,
            notification_queued,
            stop,
            owned_agent_id,
        };
        if let Some(task_owner) = task_owner {
            task_owner.register_actor(&actor.stop_handle());
        }
        actor
    }

    pub fn stop_handle(&self) -> Arc<WorkerStateActorStop> {
        self.stop.clone()
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
                notify_after_fold: false,
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
                notify_after_fold: false,
                done,
            })
            .await
    }

    /// Enqueues the complete commit + fold transaction, returning a receipt fulfilled directly
    /// after the commit. The actor retains the fold and queues lifecycle notification afterwards.
    pub fn enqueue_commit_and_update_state_notifying(
        &self,
        level: CommitLevel,
    ) -> oneshot::Receiver<()> {
        let (committed, committed_rx) = oneshot::channel();
        let (done, _done_rx) = oneshot::channel();
        if self
            .commit
            .status_jobs
            .send(StatusJob::CommitAndUpdateState {
                level,
                committed: Some(committed),
                notify_after_fold: true,
                done,
            })
            .is_err()
        {
            panic!(
                "Worker state actor for {} terminated unexpectedly",
                self.owned_agent_id
            );
        }
        committed_rx
    }

    pub async fn append_and_commit_attached(
        &self,
        entry: OplogEntry,
        worker: Arc<Worker<Ctx>>,
        instance_guard: OwnedMutexGuard<WorkerInstance>,
        card_event_boundary_guard: Option<OwnedMutexGuard<()>>,
    ) -> Result<(), WorkerExecutorError> {
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
        idempotency_key: IdempotencyKey,
        expected_result_generation: u64,
        expected_revert_generation: u64,
        instance_guard: OwnedMutexGuard<WorkerInstance>,
    ) -> bool {
        self.commit
            .run_status_job(|done| StatusJob::AppendInvocationIfVersion {
                entry: Box::new(entry),
                idempotency_key,
                expected_result_generation,
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
            .expect("Worker status could not be reconstructed")
    }

    /// Observational reads can race retirement. A closed queue or dropped reply means the
    /// caller must resolve the worker's lifecycle before reading its persisted state.
    pub async fn observe_attached_status(
        &self,
    ) -> Result<Option<Arc<AgentStatusRecord>>, WorkerExecutorError> {
        let (done, response) = oneshot::channel();
        if self
            .commit
            .status_jobs
            .send(StatusJob::AttachedStatus { done })
            .is_err()
        {
            return Ok(None);
        }
        response.await.ok().transpose()
    }

    pub async fn try_attached_status(&self) -> Result<Arc<AgentStatusRecord>, WorkerExecutorError> {
        self.reattach_worker_status().await;
        self.commit
            .run_status_job(|done| StatusJob::NonDetachedStatus { done })
            .await
            .ok_or_else(|| WorkerExecutorError::runtime("Worker status could not be reconstructed"))
    }

    /// Returns the published status, asserting it is attached to the oplog. Serialized behind
    /// any in-flight commit/reattach transactions. The assert lives here on the caller side, so
    /// a job left behind by a cancelled caller cannot panic the actor.
    pub async fn non_detached_status(&self) -> Arc<AgentStatusRecord> {
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
    /// fibers alike, because the worker lifecycle lock is only taken on the lifecycle task.
    pub fn notify_status_changed(&self) {
        queue_status_notification(&self.lifecycle_jobs, &self.notification_queued);
    }

    /// Joins queued lifecycle work and its requeued descendants after execution has stopped.
    /// Must not be awaited while holding the worker instance lock or from a lifecycle job.
    pub async fn drain_lifecycle(&self) -> Result<(), WorkerExecutorError> {
        let (done, result) = oneshot::channel();
        self.lifecycle_jobs
            .send(LifecycleJob::Drain { done })
            .map_err(|_| WorkerExecutorError::runtime("Worker lifecycle actor stopped"))?;
        result
            .await
            .map_err(|_| WorkerExecutorError::runtime("Worker lifecycle drain stopped"))
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
            notify_after_fold: false,
            done,
        })
        .await
    }

    /// Sends a job to the status task and waits for its reply.
    ///
    /// Panics if the task is gone: orderly shutdown drains accepted jobs after external producers
    /// stop, so a missing reply means the actor failed or the caller submitted work after shutdown.
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

fn queue_status_notification<Ctx: WorkerCtx>(
    lifecycle_jobs: &mpsc::UnboundedSender<LifecycleJob<Ctx>>,
    notification_queued: &AtomicBool,
) {
    if !notification_queued.swap(true, Ordering::AcqRel)
        && lifecycle_jobs
            .send(LifecycleJob::NotifyStatusChanged)
            .is_err()
    {
        notification_queued.store(false, Ordering::Release);
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
        // Sample before committing: a later sample could include new, uncommitted appends.
        let appended_through = self.oplog.current_oplog_index().await;
        let mut new_entries = self.oplog.commit(commit_level).await;
        if let Some(committed) = committed {
            let _ = committed.send(());
        }

        let mut authority_change_count = new_entries
            .values()
            .filter(|entry| is_authority_state_entry(entry))
            .count() as u64;

        let changed = if !self.detached.load(Ordering::Acquire) {
            let old_status = self.last_known_status.load_full();
            // A catch-up can already have folded a plugin's buffered direct-commit receipts.
            new_entries.retain(|index, _| *index > old_status.oplog_idx);
            let flushes_buffer =
                self.agent_mode != AgentMode::Ephemeral || commit_level != CommitLevel::DurableOnly;
            let contiguous = match (new_entries.first_key_value(), new_entries.last_key_value()) {
                (Some((first, _)), Some((last, _))) => {
                    *first == old_status.oplog_idx.next()
                        && last.as_u64() - first.as_u64() + 1 == new_entries.len() as u64
                        && (!flushes_buffer || *last >= appended_through)
                }
                _ => !flushes_buffer || appended_through <= old_status.oplog_idx,
            };
            let updated_status = if contiguous {
                update_status_with_new_entries(
                    self.agent_mode,
                    old_status.as_ref().clone(),
                    new_entries,
                    &self.deps.config().retry,
                )
            } else {
                // Threshold flushes and replica waits can commit entries outside this actor.
                // Read committed storage in bounded chunks, including payload hydration, rather
                // than retaining every automatically flushed entry in memory until this commit.
                // The gap may contain authority changes absent from the commit receipt.
                authority_change_count = authority_change_count.max(1);
                if self.agent_mode == AgentMode::Ephemeral && commit_level == CommitLevel::Deferred
                {
                    // A bounded receipt cache may have dropped part of a long invocation. The
                    // completion receipt has already been sent; make the deferred tail readable
                    // from storage before taking this exceptional reconstruction path.
                    self.oplog.commit(CommitLevel::Always).await;
                }
                try_fold_status_from(
                    &self.deps,
                    &self.owned_agent_id,
                    self.agent_mode,
                    old_status.as_ref().clone(),
                )
                .await
            };

            match updated_status {
                // The comparison stays. Skipping the fold when the commit produced no entries
                // would not be equivalent: the fold's `finalize` step prunes oplog-processor
                // checkpoints that are neither active nor in-flight, and it runs whether or not
                // there were entries (see `empty_fold_is_not_the_identity` in `status`).
                Ok(Some(updated_status)) if updated_status != *old_status => {
                    let updated_status = self.update_last_known_status(updated_status).await;

                    self.schedule_oplog_archive_if_needed(&old_status, &updated_status)
                        .await;

                    true
                }
                Ok(Some(_)) => false,
                Ok(None) => {
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
                Err(error) => {
                    tracing::error!(
                        agent_id = %self.owned_agent_id,
                        %error,
                        "Failed to update worker status from newly committed oplog entries; detaching status"
                    );
                    self.detached.store(true, Ordering::Release);
                    self.status_flusher.invalidate_baseline().await;
                    true
                }
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
                self.fingerprint,
                self.agent_mode,
                None,
            )
            .await
            .and_then(|status| {
                status
                    .ok_or_else(|| "worker oplog disappeared while reattaching status".to_string())
            });

            let Ok(worker_status) = worker_status else {
                tracing::error!(
                    agent_id = %self.owned_agent_id,
                    error = %worker_status.unwrap_err(),
                    "Failed to recompute detached worker status"
                );
                return false;
            };

            // Install the recomputed status while still detached, so a concurrent background sweep
            // keeps skipping (the in-memory status is not authoritative until it is installed).
            self.update_last_known_status(worker_status).await;

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
    ///
    /// Returns the published record. The `Arc` is built before the swap and shared with the
    /// follow-up work, so the record itself is never copied here: it is large, and this runs on
    /// every commit that changed the status.
    async fn update_last_known_status(
        &self,
        new_status: AgentStatusRecord,
    ) -> Arc<AgentStatusRecord> {
        let previous_metrics_status = self.metrics_status.status();
        let new_status = Arc::new(new_status);
        let previous_status = self.last_known_status.swap(new_status.clone());
        self.metrics_status
            .update(previous_metrics_status, new_status.status);
        self.status_flusher
            .on_status_changed(&previous_status, &new_status)
            .await;
        new_status
    }

    async fn schedule_oplog_archive_if_needed(
        &self,
        old_status: &AgentStatusRecord,
        new_status: &AgentStatusRecord,
    ) {
        // Teardown drains an ephemeral oplog, and the oplog sweep archives one stranded by a
        // crashed pod, so registering an archive per ephemeral invocation would only cost a
        // scheduler-storage write. With the sweep disabled nothing else covers the crash case, so
        // the registration still happens. See `OplogSweepConfig::enabled`.
        if self.agent_mode == AgentMode::Ephemeral && self.deps.config().oplog.sweep.enabled {
            return;
        }

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

fn can_append_invocation(
    status: &AgentStatusRecord,
    idempotency_key: &IdempotencyKey,
    expected_result_generation: u64,
    expected_revert_generation: u64,
) -> bool {
    status.invocation_results.change_generation() == expected_result_generation
        && status.invocation_results.revert_generation() == expected_revert_generation
        && !status.invocation_results.contains_key(idempotency_key)
        && status.current_idempotency_key.as_ref() != Some(idempotency_key)
        && !status
            .pending_invocations
            .iter()
            .any(|invocation| invocation.has_idempotency_key(idempotency_key))
}

#[cfg(test)]
mod tests {
    use super::{
        LifecycleJob, OwnerCommitController, StatusJob, WorkerStateActor, WorkerStateActorStop,
        can_append_invocation, complete_status_job,
    };
    use crate::workerctx::default::Context;
    use golem_common::model::component::ComponentId;
    use golem_common::model::environment::EnvironmentId;
    use golem_common::model::oplog::OplogIndex;
    use golem_common::model::{
        AgentId, AgentStatusRecord, IdempotencyKey, OwnedAgentId, PendingInvocationRef, Timestamp,
    };
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use test_r::{test, timeout};
    use tokio::sync::{Notify, mpsc, oneshot};

    #[test]
    #[timeout("30s")]
    async fn stop_drains_lifecycle_status_work_and_retains_cancelled_joins() {
        for pause_stage in 0..3 {
            for (panic_lifecycle, panic_status) in
                [(false, false), (true, false), (false, true), (true, true)]
            {
                let entered = Arc::new(Notify::new());
                let release = Arc::new(Notify::new());
                let status_finished = Arc::new(AtomicBool::new(false));
                let lifecycle_finished = Arc::new(AtomicBool::new(false));
                let finalized = Arc::new(AtomicBool::new(false));
                let (status_jobs, mut status_rx) = mpsc::unbounded_channel();
                let (lifecycle_jobs, mut lifecycle_rx) = mpsc::unbounded_channel();
                let status_task = tokio::spawn({
                    let entered = entered.clone();
                    let release = release.clone();
                    let finished = status_finished.clone();
                    async move {
                        // A lifecycle job must be able to submit status work before status Stop.
                        let Some(StatusJob::AttachedStatus { done }) = status_rx.recv().await
                        else {
                            panic!("status actor stopped before lifecycle work finished");
                        };
                        done.send(Ok(Arc::new(AgentStatusRecord::default())))
                            .unwrap();
                        assert!(matches!(status_rx.recv().await, Some(StatusJob::Stop)));
                        if pause_stage == 1 {
                            entered.notify_one();
                            release.notified().await;
                        }
                        finished.store(true, Ordering::Release);
                        assert!(!panic_status, "injected status actor panic");
                    }
                });
                let lifecycle_task = tokio::spawn({
                    let status_jobs = status_jobs.clone();
                    let entered = entered.clone();
                    let release = release.clone();
                    let finished = lifecycle_finished.clone();
                    async move {
                        assert!(matches!(
                            lifecycle_rx.recv().await,
                            Some(LifecycleJob::Stop)
                        ));
                        let (done, response) = oneshot::channel();
                        assert!(status_jobs.send(StatusJob::AttachedStatus { done }).is_ok());
                        response.await.unwrap().unwrap();
                        if pause_stage == 0 {
                            entered.notify_one();
                            release.notified().await;
                        }
                        finished.store(true, Ordering::Release);
                        assert!(!panic_lifecycle, "injected lifecycle actor panic");
                    }
                });
                let stop = WorkerStateActorStop::new(
                    {
                        let lifecycle_jobs = lifecycle_jobs.clone();
                        move || {
                            let _ = lifecycle_jobs.send(LifecycleJob::Stop);
                        }
                    },
                    lifecycle_task,
                    status_jobs.clone(),
                    status_task,
                    {
                        let entered = entered.clone();
                        let release = release.clone();
                        let finalized = finalized.clone();
                        let status_finished = status_finished.clone();
                        let lifecycle_finished = lifecycle_finished.clone();
                        async move {
                            assert!(status_finished.load(Ordering::Acquire));
                            assert!(lifecycle_finished.load(Ordering::Acquire));
                            if pause_stage == 2 {
                                entered.notify_one();
                                release.notified().await;
                            }
                            finalized.store(true, Ordering::Release);
                        }
                    },
                );
                let weak_stop = Arc::downgrade(&stop);
                let owned_agent_id = OwnedAgentId::new(
                    EnvironmentId::new(),
                    &AgentId {
                        component_id: ComponentId::new(),
                        agent_id: "actor-stop".into(),
                    },
                );
                let actor = Arc::new(WorkerStateActor::<Context> {
                    commit: Arc::new(OwnerCommitController {
                        status_jobs,
                        owned_agent_id: owned_agent_id.clone(),
                    }),
                    lifecycle_jobs,
                    notification_queued: Arc::new(AtomicBool::new(false)),
                    stop,
                    owned_agent_id,
                });
                if pause_stage == 2 {
                    // No explicit stop: dropping the old shell must initiate the same drain.
                    drop(actor);
                    entered.notified().await;
                } else {
                    let first = tokio::spawn({
                        let actor = actor.clone();
                        async move { actor.stop_handle().stop_and_wait().await }
                    });
                    entered.notified().await;
                    first.abort();
                    assert!(first.await.unwrap_err().is_cancelled());
                    drop(actor);
                }
                // The completion driver keeps a weak registration upgradeable after shell Drop.
                let stop = weak_stop.upgrade().unwrap();
                let mut retry = Box::pin(stop.stop_and_wait());
                let mut overlapping = Box::pin(stop.wait());
                assert!(futures::poll!(retry.as_mut()).is_pending());
                assert!(futures::poll!(overlapping.as_mut()).is_pending());
                assert!(!finalized.load(Ordering::Acquire));
                release.notify_one();
                let (result, overlapping) = tokio::join!(retry, overlapping);
                assert!(lifecycle_finished.load(Ordering::Acquire));
                assert!(status_finished.load(Ordering::Acquire));
                assert!(finalized.load(Ordering::Acquire));
                assert_eq!(result, overlapping);
                assert_eq!(result, stop.stop_and_wait().await);
                assert_eq!(result.is_err(), panic_lifecycle || panic_status);
                if let Err(error) = result {
                    assert_eq!(
                        error.contains("Worker lifecycle actor failed"),
                        panic_lifecycle
                    );
                    assert_eq!(error.contains("status actor failed"), panic_status);
                }
                drop(stop);
                while weak_stop.upgrade().is_some() {
                    tokio::task::yield_now().await;
                }
            }
        }
    }

    fn pending_invocation(key: IdempotencyKey) -> PendingInvocationRef {
        PendingInvocationRef {
            timestamp: Timestamp::now_utc(),
            oplog_index: OplogIndex::INITIAL,
            idempotency_key: Some(key),
            manual_update_target_revision: None,
        }
    }

    #[test]
    fn invocation_admission_ignores_unrelated_oplog_and_pending_changes() {
        let key = IdempotencyKey::fresh();
        let mut status = AgentStatusRecord {
            oplog_idx: OplogIndex::from_u64(100),
            pending_invocations: vec![pending_invocation(IdempotencyKey::fresh())],
            ..AgentStatusRecord::default()
        };
        let result_generation = status.invocation_results.change_generation();
        let revert_generation = status.invocation_results.revert_generation();

        assert!(can_append_invocation(
            &status,
            &key,
            result_generation,
            revert_generation
        ));

        status.oplog_idx = OplogIndex::from_u64(1_000);
        status
            .pending_invocations
            .push(pending_invocation(IdempotencyKey::fresh()));
        assert!(can_append_invocation(
            &status,
            &key,
            result_generation,
            revert_generation
        ));
    }

    #[test]
    fn invocation_admission_rejects_same_key_or_result_branch_changes() {
        let key = IdempotencyKey::fresh();
        let mut status = AgentStatusRecord::default();
        let result_generation = status.invocation_results.change_generation();
        let revert_generation = status.invocation_results.revert_generation();

        status.pending_invocations = vec![pending_invocation(key.clone())];
        assert!(!can_append_invocation(
            &status,
            &key,
            result_generation,
            revert_generation
        ));

        status.pending_invocations.clear();
        status.current_idempotency_key = Some(key.clone());
        assert!(!can_append_invocation(
            &status,
            &key,
            result_generation,
            revert_generation
        ));

        status.current_idempotency_key = None;
        status
            .invocation_results
            .insert(IdempotencyKey::fresh(), OplogIndex::INITIAL);
        assert!(!can_append_invocation(
            &status,
            &key,
            result_generation,
            revert_generation
        ));

        status
            .invocation_results
            .insert(key.clone(), OplogIndex::from_u64(2));
        let key_result_generation = status.invocation_results.change_generation();
        assert!(!can_append_invocation(
            &status,
            &key,
            key_result_generation,
            revert_generation
        ));

        status.invocation_results.set_revert_generation(1);
        assert!(!can_append_invocation(
            &status,
            &key,
            key_result_generation,
            revert_generation
        ));
    }

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
