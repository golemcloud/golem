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

use crate::model::{ReadFileResult, TrapType};
use crate::sandbox_filesystem::{SandboxFilesystem, SandboxFilesystemAdapter};
use crate::services::agent_filesystem::{
    LimitTransition, ResidentFilesystem, ResidentFilesystemActivity, SealedFilesystem,
    drain_sealed_filesystem, filesystem_activity, seal, set_limits,
};
use crate::services::golem_config::SnapshotPolicy;
use crate::services::oplog::{CommitLevel, EphemeralOplog, OplogOps};
use crate::services::resource_usage_metering::{ResourceUsageMeteringWindow, close_window};
use crate::services::{HasActiveAgents, HasOplog, HasShardService, HasWorker};
use crate::worker::invocation::{
    InvocationMode, InvokeResult, invoke_observed_and_traced, lower_invocation,
};
use crate::worker::status_checkpointer;
use crate::worker::{
    CreateWorkerInstanceError, FinalWorkerState, PendingLiveInvocationDisposition,
    PendingWorkerInterrupt, QueuedWorkerInvocation, RetryDecision, RunningAgent,
    RunningAgentRuntime, RunningWorker, UnloadReason, UnloadRequest, Worker, WorkerCommand,
    WorkerInterruptState, WorkerRunningAgent, WorkerTrace,
};
use crate::workerctx::{PublicWorkerIo, UpdateManagement, WorkerCtx};
use async_lock::Mutex;
use drop_stream::DropStream;
use futures::FutureExt;
use futures::channel::oneshot;
use futures::channel::oneshot::Sender;
use golem_common::model::agent::{AgentMode, ParsedAgentId};
use golem_common::model::component::{CanonicalFilePath, ComponentRevision};
use golem_common::model::oplog::{AgentError, OplogEntry};
use golem_common::model::{
    AgentId, AgentInvocation, AgentInvocationKind, AgentInvocationOutput, AgentInvocationResult,
    IdempotencyKey, OwnedAgentId, TimestampedAgentInvocation,
};
use golem_common::model::{
    AgentStatusRecord, OplogIndex, Timestamp,
    invocation_context::{AttributeValue, InvocationContextStack},
};
use golem_common::retries::get_delay;
use golem_service_base::error::worker_executor::{InterruptKind, WorkerExecutorError};
use golem_service_base::model::GetFileSystemNodeResult;

use golem_common::model::agent::structural_format::format_structural_typed;
use golem_common::related_span;
use golem_common::tracing::TraceOrigin;
use std::collections::VecDeque;
use std::future::Future;
use std::ops::DerefMut;
use std::panic::AssertUnwindSafe;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use tokio::sync::mpsc::UnboundedReceiver;
use tokio::task::JoinHandle;
use tracing::{Instrument, Level, debug, error, span, warn};
use uuid::Uuid;
use wasmtime::Store;
use wasmtime::component::Instance;

/// Span for one bounded phase of a worker's lifecycle.
///
/// The loop itself has no span (see `TraceOrigin`), so each phase span carries the
/// agent fields itself and links back to the worker's startup, keeping one worker's
/// phases navigable from each other.
///
/// Requires `owned_agent_id` and `worker_trace` fields on `$this`.
macro_rules! agent_phase_span {
    ($this:expr, $name:expr) => {
        related_span!(
            $this.worker_trace.startup_origin,
            Level::INFO,
            $name,
            agent_id = %$this.owned_agent_id.agent_id,
            agent_type = %$this.worker_trace.agent_type,
        )
    };
}

/// Context of a running worker's invocation loop
pub struct InvocationLoop<Ctx: WorkerCtx> {
    pub receiver: UnboundedReceiver<WorkerCommand>,
    pub active: Arc<RwLock<VecDeque<QueuedWorkerInvocation>>>,
    pub owned_agent_id: OwnedAgentId,
    pub parent: Arc<Worker<Ctx>>, // parent must not be dropped until the invocation_loop is running
    pub waiting_for_command: Arc<AtomicBool>,
    pub interrupt_signal: Arc<Mutex<WorkerInterruptState>>,
    pub oom_retry_count: u32,
    /// Concurrent-agent permit owned by this invocation loop task. Released
    /// (set to `None`) when the agent goes idle, re-acquired when it wakes up.
    /// Only actively running agents hold a permit. Normal stops close the command
    /// channel and await cooperative loop exit; this field's drop is only a fallback
    /// for task cancellation or panic.
    pub(super) permit_state:
        ConcurrentAgentPermitState<crate::services::active_agents::ConcurrentAgentPermit>,
    pub(super) filesystem_activity: Arc<StdMutex<Option<ResidentFilesystemActivity>>>,
    pub(super) unload_request: Arc<StdMutex<Option<UnloadRequest>>>,
    pub idle_since_millis: Arc<AtomicU64>,
    /// `ResumeReplay` is not represented in the internal queue, so we track it
    /// explicitly to avoid evicting a worker that is blocked waking up for it.
    pub resume_replay_pending: Arc<AtomicBool>,
    pub start_attempt: Uuid,
    /// What this worker's phase spans link back to, and the fields they carry.
    pub worker_trace: WorkerTrace,
}

impl<Ctx: WorkerCtx> Drop for InvocationLoop<Ctx> {
    fn drop(&mut self) {
        self.permit_state.release();
    }
}

/// Outcome of creating the worker instance for one iteration of the invocation loop.
enum CreateInstanceResult<Ctx: WorkerCtx> {
    Created {
        agent: Box<WorkerRunningAgent<Ctx>>,
        window: ResourceUsageMeteringWindow,
        recovery_decision: Option<RetryDecision>,
    },
    /// Instance creation was interrupted by a recoverable condition, such as fuel or filesystem
    /// quota exhaustion. The worker metadata remains valid and queued work must be preserved.
    Interrupted(InterruptKind),
    /// Instance creation failed; the worker was already stopped with the startup failure.
    Failed,
}

struct ResidentAgentOwnership<Runtime, Adapter: SandboxFilesystemAdapter = SandboxFilesystem> {
    runtime: Runtime,
    filesystem: ResidentFilesystem<Adapter>,
}

struct SealedAgentOwnership<Runtime, Adapter: SandboxFilesystemAdapter = SandboxFilesystem> {
    runtime: Runtime,
    filesystem: SealedFilesystem<Adapter>,
}

enum FilesystemLimitUpdateOutcome {
    Resident(ResidentFilesystem),
    MustUnload {
        filesystem: SealedFilesystem,
        failure: Option<WorkerExecutorError>,
        suspend: bool,
    },
}

struct PendingFilesystemLimitUpdate {
    allocated_bytes: u64,
    senders: Vec<Sender<Result<(), WorkerExecutorError>>>,
}

enum ResidentWakeup {
    Command(WorkerCommand),
    FilesystemTerminalFailure,
    CommandChannelClosed,
}

impl PendingFilesystemLimitUpdate {
    fn push(&mut self, allocated_bytes: u64, sender: Sender<Result<(), WorkerExecutorError>>) {
        self.allocated_bytes = allocated_bytes;
        self.senders.push(sender);
    }

    fn complete(self, result: Result<(), WorkerExecutorError>) {
        for sender in self.senders {
            let _ = sender.send(result.clone());
        }
    }
}

fn coalesce_filesystem_limit_update(
    allocated_bytes: u64,
    sender: Sender<Result<(), WorkerExecutorError>>,
    receiver: &mut UnboundedReceiver<WorkerCommand>,
    deferred_wakeups: &mut VecDeque<WorkerCommand>,
) -> PendingFilesystemLimitUpdate {
    let mut update = PendingFilesystemLimitUpdate {
        allocated_bytes,
        senders: vec![sender],
    };
    let pending_wakeups = std::mem::take(deferred_wakeups);
    let mut absorb = |command| match command {
        WorkerCommand::UpdateFilesystemLimit {
            allocated_bytes,
            sender,
        } => update.push(allocated_bytes, sender),
        command => deferred_wakeups.push_back(command),
    };
    for command in pending_wakeups {
        absorb(command);
    }
    while let Ok(command) = receiver.try_recv() {
        absorb(command);
    }
    update
}

impl<Ctx: WorkerCtx> InvocationLoop<Ctx> {
    async fn pending_interrupt(&self) -> Option<PendingWorkerInterrupt> {
        take_pending_interrupt(&self.interrupt_signal).await
    }

    /// Runs the invocation loop of a running worker, responsible for processing incoming
    /// invocation and update commands one by one.
    ///
    /// The outer invocation loop consists of the following steps:
    ///
    /// - Creating the worker instance
    /// - Recovering the worker state
    /// - Processing incoming commands in the inner invocation loop
    /// - Suspending the worker
    /// - Process the retry decision
    pub async fn run(&mut self) {
        let agent_id = self.owned_agent_id.agent_id.clone();
        let mut deferred_wakeups = VecDeque::new();
        let worker = Arc::downgrade(&self.parent);
        let _filesystem_limit_registration = self
            .parent
            .resource_entry
            .clone()
            .register_agent_filesystem_limit_target(
                self.owned_agent_id.clone(),
                move |allocated_bytes| {
                    let worker = worker.clone();
                    Box::pin(async move {
                        match worker.upgrade() {
                            Some(worker) => {
                                worker
                                    .request_agent_filesystem_limit_update(allocated_bytes)
                                    .await
                            }
                            None => Ok(()),
                        }
                    })
                },
            );

        'outer: loop {
            self.release_terminal_interrupt().await;
            // ADMISSION (CP-0 ruling E5): gates the start of a generation, so
            // fencing refuses new generations and never interrupts a running one.
            if let Err(error) = self.parent.shard_service().check_admission(&agent_id) {
                debug!(%agent_id, "Worker generation not started because its shard is not assigned");
                self.parent.complete_startup(self.start_attempt, Err(error));
                self.release_concurrent_agent_permit();
                self.stop_unloaded(None).await;
                break;
            }
            self.acquire_concurrent_agent_permit().await;
            let permit = self
                .permit_state
                .take_permit()
                .expect("startup must hold a concurrent-agent permit");
            let entity_generation = self
                .parent
                .active_agents()
                .try_get_active_agent(&self.owned_agent_id)
                .await
                .map(|active_agent| {
                    let generation = active_agent.entity_fence_generation();
                    (active_agent, generation)
                });
            let (mut agent, window, recovery_decision) = match self.create_instance(permit).await {
                CreateInstanceResult::Created {
                    agent,
                    window,
                    recovery_decision,
                } => (*agent, window, recovery_decision),
                CreateInstanceResult::Interrupted(kind) => {
                    self.release_concurrent_agent_permit();
                    let pending_interrupt = take_pending_interrupt(&self.interrupt_signal).await;
                    let kind = pending_interrupt
                        .map(|interrupt| interrupt.kind)
                        .unwrap_or(kind);
                    // Interrupted while instantiating: record the same lifecycle oplog entry the
                    // invocation failure path would (`Suspend`/`Interrupted`), then park or
                    // restart. There is no store to run `on_invocation_failure` on, but no
                    // invocation was running either — the status marker is all that is needed.
                    match kind {
                        InterruptKind::Restart | InterruptKind::Jump => {
                            debug!("Instantiation interrupted for restart, retrying");
                            continue;
                        }
                        InterruptKind::Suspend(ts) => {
                            self.parent
                                .add_and_commit_oplog(OplogEntry::suspend())
                                .await;
                            if ts < *self.parent.last_resume_request.lock().await {
                                debug!(
                                    "Suspend during instantiation ignored because there was a resume request since it"
                                );
                                continue;
                            } else {
                                self.parent.complete_startup(self.start_attempt, Ok(()));
                                self.stop_unloaded(None).await;
                                break;
                            }
                        }
                        InterruptKind::Interrupt(_) => {
                            self.parent
                                .add_and_commit_oplog(OplogEntry::interrupted())
                                .await;
                            self.parent.complete_startup(
                                self.start_attempt,
                                Err(WorkerExecutorError::Interrupted { kind }),
                            );
                            self.stop_unloaded(None).await;
                            break;
                        }
                    }
                }
                CreateInstanceResult::Failed => {
                    // early return, can't retry a failed instance creation
                    self.release_concurrent_agent_permit();
                    break;
                }
            };
            self.permit_state.install_window(window);
            *self.filesystem_activity.lock().unwrap() =
                Some(filesystem_activity(&agent.filesystem));
            if let Some((active_agent, generation)) = entity_generation {
                let interrupt_state = self.interrupt_signal.lock().await;
                if !interrupt_state.has_interrupt() {
                    active_agent.reopen_entity_admission_if_generation(generation);
                }
            }
            let mut final_decision = recovery_decision;
            let mut recovery_failure = None;
            let mut final_interrupt = None;
            let mut final_unload_request = None;
            let mut cleanup_ephemeral_worker = false;

            if recovery_failure.is_none()
                && let Some(interrupt) = self.pending_interrupt().await
            {
                let kind = interrupt.kind;
                let decision = interrupt.retry_decision();
                debug!(
                    %agent_id,
                    ?decision,
                    "Invocation queue loop interrupted after recovery"
                );
                if !matches!(kind, InterruptKind::Restart | InterruptKind::Jump) {
                    final_interrupt = Some(kind);
                }
                final_unload_request = Some(interrupt.unload_request);
                final_decision = Some(decision);
            }

            if final_decision.is_none()
                && !self
                    .parent
                    .complete_startup_success(self.start_attempt)
                    .await
            {
                final_decision = Some(RetryDecision::None);
            }

            if final_decision.is_none() {
                'resident: loop {
                    let mut inner_loop = InnerInvocationLoop {
                        receiver: &mut self.receiver,
                        active: self.active.clone(),
                        owned_agent_id: self.owned_agent_id.clone(),
                        parent: self.parent.clone(),
                        waiting_for_command: self.waiting_for_command.clone(),
                        interrupt_signal: self.interrupt_signal.clone(),
                        instance: &agent.runtime.instance,
                        store: &agent.runtime.store,
                        filesystem: &agent.filesystem,
                        invocations_since_snapshot: 0,
                        idle_snapshot_task: None,
                        permit_state: &mut self.permit_state,
                        idle_since_millis: self.idle_since_millis.clone(),
                        resume_replay_pending: self.resume_replay_pending.clone(),
                        worker_trace: self.worker_trace.clone(),
                        deferred_wakeups: &mut deferred_wakeups,
                    };

                    let result = inner_loop.run().await;
                    if let Some(update) = result.filesystem_limit_update {
                        let RunningAgent {
                            runtime,
                            filesystem,
                        } = agent;
                        let RunningAgentRuntime { instance, store } = runtime;
                        let limit_update = match self
                            .parent
                            .active_agents()
                            .agent_filesystems()
                            .resolved_limits(update.allocated_bytes)
                        {
                            Ok(limits) => match set_limits(filesystem, limits).await {
                                Ok(LimitTransition::Resident(filesystem)) => {
                                    FilesystemLimitUpdateOutcome::Resident(filesystem)
                                }
                                Ok(LimitTransition::MustUnload(filesystem)) => {
                                    FilesystemLimitUpdateOutcome::MustUnload {
                                        filesystem,
                                        failure: None,
                                        suspend: true,
                                    }
                                }
                                Err(failure) => FilesystemLimitUpdateOutcome::MustUnload {
                                    filesystem: failure.filesystem,
                                    failure: Some(WorkerExecutorError::runtime(
                                        failure.source.to_string(),
                                    )),
                                    suspend: false,
                                },
                            },
                            Err(error) => FilesystemLimitUpdateOutcome::MustUnload {
                                filesystem: seal(filesystem),
                                failure: Some(WorkerExecutorError::runtime(error.to_string())),
                                suspend: false,
                            },
                        };
                        match limit_update {
                            FilesystemLimitUpdateOutcome::Resident(filesystem) => {
                                agent = RunningAgent {
                                    runtime: RunningAgentRuntime { instance, store },
                                    filesystem,
                                };
                                *self.filesystem_activity.lock().unwrap() =
                                    Some(filesystem_activity(&agent.filesystem));
                                update.complete(Ok(()));
                                continue 'resident;
                            }
                            FilesystemLimitUpdateOutcome::MustUnload {
                                filesystem,
                                failure,
                                suspend,
                            } => {
                                store
                                    .lock()
                                    .await
                                    .data()
                                    .durable_ctx()
                                    .begin_stream_runtime_teardown();
                                if let Some(active_agent) = self
                                    .parent
                                    .active_agents()
                                    .try_get_active_agent(&self.owned_agent_id)
                                    .await
                                {
                                    active_agent.fence_entity_bodies();
                                }
                                let cleanup_failure = finish_filesystem_limit_unload(
                                    suspend,
                                    unload_sealed_agent_ownership(
                                        SealedAgentOwnership {
                                            runtime: RunningAgentRuntime { instance, store },
                                            filesystem,
                                        },
                                        UnloadReason::FilesystemLimit,
                                        resource_usage_close_deadline(),
                                        &mut self.permit_state,
                                        &self.filesystem_activity,
                                    ),
                                    || async {
                                        self.parent
                                            .add_and_commit_oplog(OplogEntry::suspend())
                                            .await;
                                    },
                                )
                                .await;
                                let response = match (&failure, &cleanup_failure) {
                                    (None, None) => Ok(()),
                                    (Some(failure), None) => Err(failure.clone()),
                                    (None, Some(cleanup)) => Err(cleanup.clone()),
                                    (Some(failure), Some(cleanup)) => {
                                        Err(WorkerExecutorError::runtime(format!(
                                            "{failure}; {cleanup}"
                                        )))
                                    }
                                };
                                update.complete(response);
                                match cleanup_failure {
                                    Some(cleanup) => {
                                        let cleanup = failure.map_or(cleanup.clone(), |failure| {
                                            WorkerExecutorError::runtime(format!(
                                                "{failure}; {cleanup}"
                                            ))
                                        });
                                        self.stop_cleanup_failed(cleanup).await;
                                    }
                                    None => self.stop_unloaded(failure).await,
                                }
                                break 'outer;
                            }
                        }
                    }
                    final_decision = result.retry_decision;
                    final_interrupt = result.final_interrupt;
                    final_unload_request = result.unload_request;
                    recovery_failure = result.recovery_failure;
                    if let Some(interrupt) = self.pending_interrupt().await {
                        let kind = interrupt.kind;
                        let decision = interrupt.retry_decision();
                        if !matches!(kind, InterruptKind::Restart | InterruptKind::Jump) {
                            final_interrupt = Some(kind);
                        }
                        final_unload_request = Some(interrupt.unload_request);
                        final_decision = Some(decision);
                    }
                    cleanup_ephemeral_worker = result.cleanup_ephemeral_worker;
                    break 'resident;
                }
            }

            let retry_was_live = {
                let store = agent.runtime.store.lock().await;
                store.data().durable_ctx().begin_stream_runtime_teardown();
                store.data().is_live()
            };
            self.suspend_worker(&agent.runtime.store).await;

            if let Some(kind) = final_interrupt {
                self.record_retry_interrupt_failure(&agent.runtime.store, kind)
                    .await;
            }

            let unload_request = self
                .unload_request
                .lock()
                .unwrap()
                .take()
                .or(final_unload_request)
                .unwrap_or_else(|| {
                    let reason = if recovery_failure.is_some() {
                        UnloadReason::Failure
                    } else {
                        match final_decision {
                            Some(RetryDecision::TryStop(_)) => UnloadReason::Suspend,
                            Some(RetryDecision::ReacquirePermits) => UnloadReason::OutOfMemory,
                            Some(RetryDecision::Immediate | RetryDecision::Delayed(_)) => {
                                UnloadReason::Failure
                            }
                            None | Some(RetryDecision::None) => UnloadReason::ExplicitStop,
                        }
                    };
                    UnloadRequest::ordinary(reason)
                });
            if let Some(active_agent) = self
                .parent
                .active_agents()
                .try_get_active_agent(&self.owned_agent_id)
                .await
            {
                active_agent.fence_entity_bodies();
            }
            if let Some(error) = Self::unload_running_agent(
                agent,
                unload_request.reason,
                unload_request.deadline,
                &mut self.permit_state,
                &self.filesystem_activity,
            )
            .await
            {
                self.stop_cleanup_failed(error).await;
                break;
            }

            match final_decision {
                None | Some(RetryDecision::None) => {
                    debug!(
                        %agent_id,
                        "Invocation queue loop notifying parent about being stopped"
                    );
                    self.stop_closed(
                        None,
                        recovery_failure.or_else(|| {
                            cleanup_ephemeral_worker.then(super::inactive_ephemeral_agent_error)
                        }),
                    )
                    .await;
                    if cleanup_ephemeral_worker {
                        self.parent.remove_from_active_agents().await;
                        self.archive_ephemeral_oplog();
                    }
                    break;
                }
                Some(RetryDecision::TryStop(ts)) => {
                    if ts < *self.parent.last_resume_request.lock().await {
                        debug!(
                            %agent_id,
                            "Suspend request ignored because there was a resume request since it"
                        );
                        continue;
                    } else {
                        debug!(
                            %agent_id,
                            "Invocation queue loop notifying parent about being stopped"
                        );
                        self.stop_closed(None, None).await;
                        break;
                    }
                }
                Some(RetryDecision::Immediate) => {
                    debug!(%agent_id, "Invocation queue loop triggering restart immediately");
                    continue;
                }
                Some(RetryDecision::Delayed(delay)) => {
                    debug!(
                        %agent_id,
                        ?delay,
                        "Invocation queue loop sleeping for a delayed restart"
                    );
                    let sleep = tokio::time::sleep(delay);
                    tokio::pin!(sleep);
                    loop {
                        tokio::select! {
                            _ = &mut sleep => {
                                debug!(%agent_id, "Invocation queue loop restarting after delay");
                                continue 'outer;
                            }
                            command = self.receiver.recv() => {
                                let command = match command {
                                    Some(command) => command,
                                    None => {
                                        debug!(%agent_id, "Invocation queue loop command channel closed during delayed retry");
                                        self.stop_closed(None, None).await;
                                        break 'outer;
                                    }
                                };

                                if let Some(interrupt) = self.pending_interrupt().await {
                                    let kind = interrupt.kind;
                                    let decision = interrupt.retry_decision();
                                    debug!(%agent_id, ?decision, "Invocation queue loop interrupted during delayed retry");
                                    if !matches!(kind, InterruptKind::Restart | InterruptKind::Jump) {
                                        let current_idempotency_key = self
                                            .parent
                                            .get_non_detached_last_known_status()
                                            .await
                                            .current_idempotency_key;
                                        match kind {
                                            InterruptKind::Suspend(_) => {
                                                self.parent.add_and_commit_oplog(OplogEntry::suspend()).await;
                                            }
                                            InterruptKind::Interrupt(_) => {
                                                self.parent.add_and_commit_oplog(OplogEntry::interrupted()).await;
                                            }
                                            InterruptKind::Restart | InterruptKind::Jump => {}
                                        }
                                        if matches!(kind, InterruptKind::Interrupt(_))
                                            && let Some(key) = current_idempotency_key
                                        {
                                            self.parent
                                                .store_invocation_failure(
                                                    &key,
                                                    &TrapType::Interrupt(kind),
                                                )
                                                .await;
                                            self.parent.event_service().emit_invocation_finished(
                                                "interrupted during retry",
                                                &key,
                                                retry_was_live,
                                            );
                                        }
                                    }
                                    match decision {
                                        RetryDecision::Immediate => {
                                            Self::defer_wakeup(&mut deferred_wakeups, command);
                                            continue 'outer;
                                        }
                                        RetryDecision::None => {
                                            self.stop_closed(None, None).await;
                                            break 'outer;
                                        }
                                        RetryDecision::TryStop(timestamp) => {
                                            if timestamp < *self.parent.last_resume_request.lock().await {
                                                Self::defer_wakeup(&mut deferred_wakeups, command);
                                                continue 'outer;
                                            }
                                            self.stop_closed(None, None).await;
                                            break 'outer;
                                        }
                                        RetryDecision::Delayed(_) | RetryDecision::ReacquirePermits => {
                                            unreachable!("queued interrupts do not delay or reacquire permits")
                                        }
                                    }
                                }

                                match command {
                                    WorkerCommand::InternalStatusChanged => {
                                        debug!("Invocation queue loop ignored internal status change during delayed retry");
                                        continue;
                                    }
                                    WorkerCommand::WorkAvailable => {
                                        debug!(%agent_id, "Invocation queue loop woke up during delayed retry");
                                        Self::defer_wakeup(&mut deferred_wakeups, WorkerCommand::WorkAvailable);
                                        continue 'outer;
                                    }
                                    WorkerCommand::ResumeReplay => {
                                        debug!(%agent_id, "Invocation queue loop woke up for resume replay during delayed retry");
                                        Self::defer_wakeup(&mut deferred_wakeups, WorkerCommand::ResumeReplay);
                                        continue 'outer;
                                    }
                                    WorkerCommand::UpdateFilesystemLimit { sender, .. } => {
                                        let _ = sender.send(Ok(()));
                                    }
                                }
                            }
                        }
                    }
                }
                Some(RetryDecision::ReacquirePermits) => {
                    let delay = get_delay(self.parent.oom_retry_config(), self.oom_retry_count);
                    debug!(
                        %agent_id,
                        ?delay,
                        "Invocation queue loop dropping memory permits and triggering restart"
                    );
                    let pending_startup_attempt = self.parent.pending_startup_attempt();
                    if let Err(error) = Worker::restart_on_oom(
                        self.parent.clone(),
                        true,
                        delay,
                        self.oom_retry_count + 1,
                        pending_startup_attempt,
                    )
                    .await
                    {
                        warn!("Failed to restart worker after releasing memory permits: {error}");
                    }
                    break;
                }
            }
        }
        self.release_terminal_interrupt().await;
    }

    async fn release_terminal_interrupt(&self) {
        self.interrupt_signal
            .lock()
            .await
            .reset_terminal_for_new_generation();
    }

    async fn acquire_concurrent_agent_permit(&mut self) {
        if self.permit_state.is_none() {
            let agent_id = self.owned_agent_id.agent_id();
            let permit = self
                .parent
                .registered_concurrent_account
                .acquire(agent_id)
                .instrument(agent_phase_span!(self, "acquire_concurrent_agent_permit"))
                .await;
            self.permit_state.install_tracked(permit);
        }
    }

    fn release_concurrent_agent_permit(&mut self) {
        self.permit_state.release();
    }

    async fn stop_unloaded(&self, startup_failure: Option<WorkerExecutorError>) {
        self.parent.complete_startup(
            self.start_attempt,
            Err(startup_failure.clone().unwrap_or_else(|| {
                WorkerExecutorError::unknown("Worker stopped before startup completed")
            })),
        );
        if let Some(active_agent) = self
            .parent
            .active_agents()
            .try_get_active_agent(&self.owned_agent_id)
            .await
        {
            active_agent.fence_entity_bodies();
        }
        self.parent
            .stop_internal(
                true,
                startup_failure.clone(),
                UnloadRequest::ordinary(UnloadReason::ExplicitStop),
                FinalWorkerState::Unloaded { startup_failure },
                PendingLiveInvocationDisposition::Fail,
            )
            .await;
    }

    async fn stop_cleanup_failed(&self, error: WorkerExecutorError) {
        self.parent
            .complete_startup(self.start_attempt, Err(error.clone()));
        if let Some(active_agent) = self
            .parent
            .active_agents()
            .try_get_active_agent(&self.owned_agent_id)
            .await
        {
            active_agent.fence_entity_bodies();
        }
        let pending_failure = error.clone();
        self.parent
            .stop_internal(
                true,
                Some(pending_failure),
                UnloadRequest::ordinary(UnloadReason::Failure),
                FinalWorkerState::CleanupFailed(error),
                PendingLiveInvocationDisposition::Fail,
            )
            .await;
    }

    async fn stop_closed(
        &self,
        cleanup_error: Option<WorkerExecutorError>,
        startup_failure: Option<WorkerExecutorError>,
    ) {
        publish_unload_outcome(
            cleanup_error,
            startup_failure,
            |error| async move { self.stop_cleanup_failed(error).await },
            |startup_failure| async move { self.stop_unloaded(startup_failure).await },
        )
        .await;
    }

    fn unload_running_agent<Runtime, Adapter>(
        agent: RunningAgent<Runtime, Adapter>,
        reason: UnloadReason,
        deadline: Instant,
        permit_state: &mut ConcurrentAgentPermitState<
            crate::services::active_agents::ConcurrentAgentPermit,
        >,
        filesystem_activity: &Arc<StdMutex<Option<ResidentFilesystemActivity>>>,
    ) -> UnloadObserver
    where
        Runtime: Send + 'static,
        Adapter: SandboxFilesystemAdapter,
    {
        let RunningAgent {
            runtime,
            filesystem,
        } = agent;
        unload_resident_agent_ownership(
            ResidentAgentOwnership {
                runtime,
                filesystem,
            },
            reason,
            deadline,
            permit_state,
            filesystem_activity,
        )
    }

    fn archive_ephemeral_oplog(&self) {
        let oplog = self.parent.oplog.clone();
        tokio::spawn(async move {
            let _ = EphemeralOplog::try_archive_background(&oplog).await;
        });
    }

    async fn record_retry_interrupt_failure(&self, store: &Mutex<Store<Ctx>>, kind: InterruptKind) {
        store
            .lock()
            .await
            .data_mut()
            .on_invocation_failure("interrupted during retry", &TrapType::Interrupt(kind))
            .await;
    }

    fn defer_wakeup(deferred_wakeups: &mut VecDeque<WorkerCommand>, command: WorkerCommand) {
        let already_deferred = match command {
            WorkerCommand::WorkAvailable => deferred_wakeups
                .iter()
                .any(|command| matches!(command, WorkerCommand::WorkAvailable)),
            WorkerCommand::ResumeReplay => deferred_wakeups
                .iter()
                .any(|command| matches!(command, WorkerCommand::ResumeReplay)),
            WorkerCommand::InternalStatusChanged => true,
            WorkerCommand::UpdateFilesystemLimit { .. } => false,
        };

        if !already_deferred {
            deferred_wakeups.push_back(command);
        }
    }

    /// Create the worker instance and publish an event about it
    async fn create_instance(
        &self,
        permit: crate::services::active_agents::ConcurrentAgentPermit,
    ) -> CreateInstanceResult<Ctx> {
        async {
            debug!("Creating the worker instance");
            match RunningWorker::create_instance(self.parent.clone(), permit).await {
                Ok((agent, window, recovery_decision)) => CreateInstanceResult::Created {
                    agent: Box::new(agent),
                    window,
                    recovery_decision,
                },
                // Instance creation was interrupted by a recoverable condition. The worker exists
                // and its metadata and `Create` oplog entry are already persisted, so the caller
                // parks or restarts the worker without exposing an unprepared runtime.
                Err(CreateWorkerInstanceError {
                    error: WorkerExecutorError::Interrupted { kind },
                    filesystem_cleanup_failed: false,
                }) => {
                    debug!("Worker instantiation interrupted: {kind:?}");
                    CreateInstanceResult::Interrupted(kind)
                }
                Err(CreateWorkerInstanceError {
                    error: err,
                    filesystem_cleanup_failed,
                }) => {
                    warn!("Failed to start the worker: {err}");
                    self.parent
                        .complete_startup(self.start_attempt, Err(err.clone()));
                    let final_state = if filesystem_cleanup_failed {
                        FinalWorkerState::CleanupFailed(err.clone())
                    } else {
                        FinalWorkerState::Unloaded {
                            startup_failure: Some(err.clone()),
                        }
                    };
                    self.parent
                        .stop_internal(
                            true,
                            Some(err),
                            UnloadRequest::ordinary(UnloadReason::Failure),
                            final_state,
                            PendingLiveInvocationDisposition::Fail,
                        )
                        .await;
                    CreateInstanceResult::Failed
                }
            }
        }
        .instrument(agent_phase_span!(self, "create_instance"))
        .await
    }

    /// Suspends the worker after the invocation loop exited
    async fn suspend_worker(&self, store: &Mutex<Store<Ctx>>) {
        async {
            // Marking the worker as suspended
            store.lock().await.data().set_suspended();

            // Making sure all pending commits are flushed
            // Make sure all pending commits are done
            let worker = store.lock().await.data().get_public_state().worker();
            worker
                .commit_oplog_and_update_state(CommitLevel::Always)
                .await;

            // The worker is going idle; persist its cached status synchronously now instead of leaving
            // it for the next background sweep, so reads of an idle worker see an up-to-date blob.
            worker.force_flush_status().await;

            // Idle is a structurally clean boundary (the invocation loop has exited and committed, so
            // no jumpable region is open): write a throttled clean status checkpoint so a later
            // jump-induced recompute can fold forward from here.
            worker
                .checkpoint_status(status_checkpointer::CheckpointReason::Idle)
                .await;
        }
        .instrument(agent_phase_span!(self, "suspend_worker"))
        .await
    }
}

async fn publish_unload_outcome<T>(
    cleanup_error: Option<WorkerExecutorError>,
    startup_failure: Option<WorkerExecutorError>,
    cleanup_failed: impl AsyncFnOnce(WorkerExecutorError) -> T,
    unloaded: impl AsyncFnOnce(Option<WorkerExecutorError>) -> T,
) -> T {
    match cleanup_error {
        Some(error) => cleanup_failed(error).await,
        None => unloaded(startup_failure).await,
    }
}

async fn finish_filesystem_limit_unload(
    suspend: bool,
    unload: impl Future<Output = Option<WorkerExecutorError>>,
    record_suspend: impl AsyncFnOnce(),
) -> Option<WorkerExecutorError> {
    let cleanup_failure = unload.await;
    if suspend && cleanup_failure.is_none() {
        record_suspend().await;
    }
    cleanup_failure
}

fn unload_resident_agent_ownership<Runtime, Adapter>(
    ownership: ResidentAgentOwnership<Runtime, Adapter>,
    reason: UnloadReason,
    deadline: Instant,
    permit_state: &mut ConcurrentAgentPermitState<
        crate::services::active_agents::ConcurrentAgentPermit,
    >,
    filesystem_activity: &Arc<StdMutex<Option<ResidentFilesystemActivity>>>,
) -> UnloadObserver
where
    Runtime: Send + 'static,
    Adapter: SandboxFilesystemAdapter,
{
    let ResidentAgentOwnership {
        runtime,
        filesystem,
    } = ownership;
    unload_sealed_agent_ownership(
        SealedAgentOwnership {
            runtime,
            filesystem: seal(filesystem),
        },
        reason,
        deadline,
        permit_state,
        filesystem_activity,
    )
}

fn unload_sealed_agent_ownership<Runtime, Adapter>(
    ownership: SealedAgentOwnership<Runtime, Adapter>,
    reason: UnloadReason,
    deadline: Instant,
    permit_state: &mut ConcurrentAgentPermitState<
        crate::services::active_agents::ConcurrentAgentPermit,
    >,
    filesystem_activity: &Arc<StdMutex<Option<ResidentFilesystemActivity>>>,
) -> UnloadObserver
where
    Runtime: Send + 'static,
    Adapter: SandboxFilesystemAdapter,
{
    let window = permit_state.take_window();
    let permit = permit_state.take_permit();
    let permit_held = Arc::clone(&permit_state.held);
    filesystem_activity.lock().unwrap().take();
    spawn_module_owned_unload_continuation(move |completion| async move {
        let SealedAgentOwnership {
            runtime,
            filesystem,
        } = ownership;
        debug!(?reason, "Unloading resident agent");
        drop(runtime);

        let deadline_sleep = tokio::time::sleep_until(tokio::time::Instant::from_std(deadline));
        tokio::pin!(deadline_sleep);
        let (drained_before_deadline, close_error) = {
            let drain = drain_sealed_filesystem(&filesystem);
            tokio::pin!(drain);
            let drained_before_deadline = tokio::select! {
                _ = &mut drain => true,
                _ = &mut deadline_sleep => false,
            };
            let close_error = match window {
                Some(window) => close_window(window, deadline)
                    .await
                    .err()
                    .map(|error| WorkerExecutorError::runtime(error.to_string())),
                None => {
                    drop(permit);
                    None
                }
            };
            permit_held.store(false, Ordering::Release);

            if !drained_before_deadline {
                completion.complete(Some(combine_unload_errors(
                    unload_deadline_error(reason),
                    close_error.clone(),
                )));
                drain.await;
            }
            (drained_before_deadline, close_error)
        };

        if !drained_before_deadline {
            if let Some(error) = delete_unloaded_filesystem(filesystem).await {
                error!(?reason, error = %error, "Agent cleanup failed after unload deadline");
            }
            return;
        }

        if let Some(error) = close_error {
            completion.complete(Some(error));
            if let Some(error) = delete_unloaded_filesystem(filesystem).await {
                error!(?reason, error = %error, "Agent cleanup failed after lifecycle completion");
            }
            return;
        }

        let deletion = delete_unloaded_filesystem(filesystem);
        tokio::pin!(deletion);
        tokio::select! {
            result = &mut deletion => {
                completion.complete(result);
            }
            _ = &mut deadline_sleep => {
                completion.complete(Some(unload_deadline_error(reason)));
                if let Some(error) = deletion.await {
                    error!(?reason, error = %error, "Agent cleanup failed after unload deadline");
                }
            }
        };
    })
}

fn combine_unload_errors(
    primary: WorkerExecutorError,
    secondary: Option<WorkerExecutorError>,
) -> WorkerExecutorError {
    secondary.map_or(primary.clone(), |secondary| {
        WorkerExecutorError::runtime(format!("{primary}; {secondary}"))
    })
}

fn unload_deadline_error(reason: UnloadReason) -> WorkerExecutorError {
    WorkerExecutorError::runtime(format!("agent unload deadline reached for {reason:?}"))
}

async fn delete_unloaded_filesystem<Adapter: SandboxFilesystemAdapter>(
    filesystem: SealedFilesystem<Adapter>,
) -> Option<WorkerExecutorError> {
    crate::services::agent_filesystem::delete(filesystem)
        .await
        .err()
        .map(|error| {
            error!(error = %error.source, "Failed to delete agent runtime filesystem");
            WorkerExecutorError::runtime(error.source.to_string())
        })
}

async fn catch_invocation_loop_panic<T>(
    future: impl Future<Output = T>,
) -> Result<T, WorkerExecutorError> {
    AssertUnwindSafe(future)
        .catch_unwind()
        .await
        .map_err(|panic| {
            let message = panic
                .downcast_ref::<&str>()
                .copied()
                .or_else(|| panic.downcast_ref::<String>().map(String::as_str))
                .unwrap_or("unknown panic");
            WorkerExecutorError::runtime(format!("invocation loop panicked: {message}"))
        })
}

pub(super) async fn run_invocation_loop_task<T>(
    future: impl Future<Output = T>,
    on_panic: impl AsyncFnOnce(WorkerExecutorError),
) {
    if let Err(error) = catch_invocation_loop_panic(future).await {
        on_panic(error).await;
    }
}

struct InnerInvocationLoop<'a, Ctx: WorkerCtx> {
    receiver: &'a mut UnboundedReceiver<WorkerCommand>,
    active: Arc<RwLock<VecDeque<QueuedWorkerInvocation>>>,
    owned_agent_id: OwnedAgentId,
    parent: Arc<Worker<Ctx>>, // parent must not be dropped until the invocation_loop is running
    waiting_for_command: Arc<AtomicBool>,
    interrupt_signal: Arc<Mutex<WorkerInterruptState>>,
    instance: &'a Instance,
    store: &'a Mutex<Store<Ctx>>,
    filesystem: &'a ResidentFilesystem,
    invocations_since_snapshot: u64,
    idle_snapshot_task: Option<JoinHandle<()>>,
    /// Mutable reference to the concurrent-agent permit held by the outer
    /// `InvocationLoop`. Set to `None` when entering idle (releasing the
    /// permit back to the semaphore pool) and re-acquired on wake.
    permit_state:
        &'a mut ConcurrentAgentPermitState<crate::services::active_agents::ConcurrentAgentPermit>,
    idle_since_millis: Arc<AtomicU64>,
    resume_replay_pending: Arc<AtomicBool>,
    deferred_wakeups: &'a mut VecDeque<WorkerCommand>,
    /// What this worker's phase spans link back to, and the fields they carry.
    worker_trace: WorkerTrace,
}

impl<Ctx: WorkerCtx> InnerInvocationLoop<'_, Ctx> {
    /// The inner invocation loop started when the worker instance state is fully restored
    /// and the worker is ready to take invocations.
    ///
    /// This loop exits when the unbounded message queue owned by the RunningWorker is dropped,
    /// or when an error occurs in one of the command handlers.
    ///
    /// The inner loop only runs if the retry decision coming from `recover_instance_state` is `None`,
    /// meaning there were no errors during the instance preparation. The inner loop can override this
    /// decision in the following way:
    /// - If it returns `RetryDecision::None`, it means it is not possible to retry the outer loop and the whole invocation loop should be stopped.
    /// - Otherwise it returns either `None` if there were no errors, otherwise the retry decision coming from the
    ///   underlying retry logic.
    ///
    /// The outer loop should either break or use the returned retry decision after the inner loop quits.
    pub async fn run(&mut self) -> InnerInvocationLoopResult {
        let agent_id = self.owned_agent_id.agent_id.clone();
        debug!(%agent_id, "Invocation queue loop started");

        let mut final_decision = None;
        let mut final_interrupt = None;
        let mut unload_request = None;
        let mut cleanup_ephemeral_worker = false;
        let mut recovery_failure = None;
        let mut filesystem_limit_update = None;

        // Entering idle: release the concurrent-agent permit so other agents
        // from the same account can start without evicting this one.
        self.check_no_active_tail_work_on_idle().await;
        if let Err(error) = self.release_concurrent_agent_permit().await {
            error!(error = %error, "Failed to close worker resource billing window");
            return InnerInvocationLoopResult {
                retry_decision: Some(RetryDecision::Immediate),
                final_interrupt: None,
                unload_request: None,
                cleanup_ephemeral_worker: false,
                recovery_failure: None,
                filesystem_limit_update: None,
            };
        }
        mark_idle(&self.idle_since_millis);
        self.waiting_for_command.store(true, Ordering::Release);
        loop {
            let cmd = match self.next_wakeup_or_initial().await {
                ResidentWakeup::Command(cmd) => cmd,
                ResidentWakeup::FilesystemTerminalFailure => {
                    debug!(
                        %agent_id,
                        "Resident filesystem generation reported a terminal failure"
                    );
                    final_decision = Some(RetryDecision::Immediate);
                    break;
                }
                ResidentWakeup::CommandChannelClosed => break,
            };
            let cmd = match cmd {
                WorkerCommand::UpdateFilesystemLimit {
                    allocated_bytes,
                    sender,
                } => {
                    filesystem_limit_update = Some(coalesce_filesystem_limit_update(
                        allocated_bytes,
                        sender,
                        self.receiver,
                        self.deferred_wakeups,
                    ));
                    break;
                }
                command => command,
            };
            if matches!(cmd, WorkerCommand::InternalStatusChanged)
                && !self.internal_status_change_requires_permit().await
            {
                continue;
            }

            // Waking from idle: re-acquire the concurrent-agent permit before
            // processing any commands.
            self.waiting_for_command.store(false, Ordering::Release);
            if let Err(error) = self.acquire_concurrent_agent_permit().await {
                error!(error = %error, "Failed to open worker resource billing window");
                final_decision = Some(RetryDecision::Immediate);
                break;
            }
            let outcome = match cmd {
                WorkerCommand::WorkAvailable | WorkerCommand::InternalStatusChanged => {
                    loop {
                        if let Some(interrupt) =
                            take_pending_interrupt(&self.interrupt_signal).await
                        {
                            if interrupt.is_terminal() {
                                final_interrupt = Some(interrupt.kind);
                            }
                            unload_request = Some(interrupt.unload_request);
                            break self.interrupt(interrupt).await;
                        }

                        let message = self.pop_ready_internal_invocation().await;

                        let result = if let Some(message) = message {
                            self.internal_invocation(message).await
                        } else {
                            // Queue is empty, use last_known_status for pending updates and invocations.
                            // This may inject a snapshot as the next action, so stay in the drain loop
                            // when immediate follow-up work was scheduled.
                            self.drain_pending_from_status().await
                        };

                        match result {
                            CommandOutcome::Continue => {
                                // Continue draining the queue
                                continue;
                            }
                            CommandOutcome::WaitForWakeup => {
                                break CommandOutcome::Continue;
                            }
                            other => {
                                // Break out of the drain loop and handle the outcome
                                break other;
                            }
                        }
                    }
                }
                WorkerCommand::ResumeReplay => {
                    self.resume_replay_pending.store(false, Ordering::Release);
                    self.resume_replay().await
                }
                WorkerCommand::UpdateFilesystemLimit { .. } => unreachable!(),
            };
            match outcome {
                CommandOutcome::BreakOuterLoop(error) => {
                    recovery_failure = error;
                    final_decision = Some(RetryDecision::None);
                    break;
                }
                CommandOutcome::BreakInnerLoop(decision) => {
                    final_decision = Some(decision);
                    break;
                }
                CommandOutcome::BreakInnerLoopAndArchiveEphemeralOplog(decision) => {
                    final_decision = Some(decision);
                    cleanup_ephemeral_worker = true;
                    break;
                }
                CommandOutcome::Continue | CommandOutcome::WaitForWakeup => {}
            }

            // Returning to idle: release the concurrent-agent permit.
            self.check_no_active_tail_work_on_idle().await;
            if let Err(error) = self.release_concurrent_agent_permit().await {
                error!(error = %error, "Failed to close worker resource billing window");
                final_decision = Some(RetryDecision::Immediate);
                break;
            }
            mark_idle(&self.idle_since_millis);
            self.waiting_for_command.store(true, Ordering::Release);
        }
        self.abort_idle_snapshot_task();
        self.waiting_for_command.store(false, Ordering::Release);

        debug!(final_decision = ?final_decision, "Invocation queue loop finished");

        InnerInvocationLoopResult {
            retry_decision: final_decision,
            final_interrupt,
            unload_request,
            cleanup_ephemeral_worker,
            recovery_failure,
            filesystem_limit_update,
        }
    }

    async fn internal_status_change_requires_permit(&self) -> bool {
        if !self.active.read().await.is_empty()
            || self.interrupt_signal.lock().await.has_interrupt()
        {
            return true;
        }

        let status = self.parent.get_non_detached_last_known_status().await;
        !status.pending_updates.is_empty()
            || !status.pending_invocations.is_empty()
            || !matches!(
                self.periodic_snapshot_action(&status),
                PeriodicSnapshotAction::NotNeeded
            )
    }

    async fn next_wakeup_or_initial(&mut self) -> ResidentWakeup {
        if filesystem_activity(self.filesystem).has_terminal_failure() {
            return ResidentWakeup::FilesystemTerminalFailure;
        }
        match self.deferred_wakeups.pop_front() {
            Some(command) => ResidentWakeup::Command(command),
            None => self.next_wakeup().await,
        }
    }

    async fn pop_ready_internal_invocation(&self) -> Option<QueuedWorkerInvocation> {
        self.active.write().await.pop_front()
    }

    /// Checks — before publishing `waiting_for_command = true`, which makes the
    /// worker eligible for idle eviction — that no Golem-spawned store task is
    /// still active. Every guest call drains its tail work before returning
    /// (either the tasks finished or they are parked at a safe park point
    /// awaiting future guest action), so an active task here means the activity
    /// accounting regressed and eviction could race live durable work. Flags
    /// the violation instead of silently entering idle.
    async fn check_no_active_tail_work_on_idle(&self) {
        let active = self
            .store
            .lock()
            .await
            .data()
            .durable_ctx()
            .tail_work_tracker()
            .active_count();
        if active != 0 {
            error!(
                "Worker entering idle with {active} active store-spawned task(s); \
                 tail work must settle before an invocation completes"
            );
            debug_assert!(
                active == 0,
                "worker entered idle with {active} active store-spawned task(s)"
            );
        }
    }

    /// Release the concurrent-agent permit back to the semaphore pool.
    /// Called when the agent enters idle state. No-op if already released.
    async fn release_concurrent_agent_permit(&mut self) -> Result<(), WorkerExecutorError> {
        if self.permit_state.is_some() {
            debug!(agent_id = %self.owned_agent_id.agent_id, "Releasing concurrent-agent permit (entering idle)");
            if let Some(window) = self.permit_state.take_window() {
                let result = close_window(window, resource_usage_close_deadline())
                    .await
                    .map(|_| ())
                    .map_err(|error| WorkerExecutorError::runtime(error.to_string()));
                self.permit_state.mark_released();
                return result;
            }
            self.permit_state.release();
        }
        Ok(())
    }

    /// Re-acquire the concurrent-agent permit from the scheduler.
    /// Called when the agent wakes from idle to process a command.
    /// The scheduler ensures FIFO ordering within the account so that a worker
    /// that just finished goes to the back of the queue.
    async fn acquire_concurrent_agent_permit(&mut self) -> Result<(), WorkerExecutorError> {
        if self.permit_state.is_none() {
            let span = agent_phase_span!(self, "acquire_concurrent_agent_permit");
            let agent_id = self.owned_agent_id.agent_id();
            let registered_concurrent_account = self.parent.registered_concurrent_account.clone();
            let permit = async {
                debug!("Re-acquiring concurrent-agent permit (waking from idle)");
                registered_concurrent_account.acquire(agent_id).await
            }
            .instrument(span)
            .await;
            let permit = self.permit_state.track(permit);
            match crate::services::agent_filesystem::open_resource_usage_window(
                self.filesystem,
                permit,
            )
            .await
            {
                Ok(window) => self.permit_state.install_window(window),
                Err(error) => {
                    self.permit_state.mark_released();
                    return Err(WorkerExecutorError::runtime(error.to_string()));
                }
            }
        }
        Ok(())
    }

    async fn next_wakeup(&mut self) -> ResidentWakeup {
        let mut idle_snapshot_task = self.idle_snapshot_task.take();
        let activity = filesystem_activity(self.filesystem);

        let wakeup = if let Some(task) = idle_snapshot_task.as_mut() {
            tokio::select! {
                wakeup = wait_for_resident_wakeup(self.receiver, &activity) => {
                    task.abort();
                    wakeup
                }
                result = &mut *task => {
                    if let Err(err) = result {
                        if !err.is_cancelled() {
                            warn!(agent_id = %self.owned_agent_id.agent_id, "Idle snapshot timer failed: {err}");
                        }
                        return wait_for_resident_wakeup(self.receiver, &activity).await;
                    }

                    if activity.has_terminal_failure() {
                        ResidentWakeup::FilesystemTerminalFailure
                    } else {
                        match self.receiver.try_recv() {
                            Ok(cmd) => ResidentWakeup::Command(cmd),
                            Err(tokio::sync::mpsc::error::TryRecvError::Empty) => {
                                ResidentWakeup::Command(WorkerCommand::WorkAvailable)
                            }
                            Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => {
                                ResidentWakeup::CommandChannelClosed
                            }
                        }
                    }
                }
            }
        } else {
            wait_for_resident_wakeup(self.receiver, &activity).await
        };

        self.idle_snapshot_task = None;
        wakeup
    }

    fn abort_idle_snapshot_task(&mut self) {
        if let Some(task) = self.idle_snapshot_task.take() {
            task.abort();
        }
    }

    /// When the main queue becomes empty, process items from last_known_status:
    /// first pending_updates, then pending_invocations
    async fn drain_pending_from_status(&mut self) -> CommandOutcome {
        loop {
            let status = self.parent.get_non_detached_last_known_status().await;

            // First, try to process a pending update
            if status.pending_updates.front().is_some() {
                // if the update made it to pending_updates (instead of pending invocations), it is ready
                // to be processed on next restart. So just restart here and let the recovery logic take over
                break CommandOutcome::BreakInnerLoop(RetryDecision::Immediate);
            }

            // Then, try to process a pending invocation
            if let Some(pending_invocation) = status.pending_invocations.first() {
                let idempotency_key = pending_invocation.idempotency_key();
                let origin = match idempotency_key {
                    Some(idempotency_key) => self
                        .parent
                        .external_invocation_origins
                        .read()
                        .await
                        .get(idempotency_key)
                        .cloned(),
                    None => None,
                };

                // An invocation with no recorded origin was enqueued in an earlier
                // process, so there is nothing in-process to relate it to.
                let origin = origin.unwrap_or_else(TraceOrigin::none);

                // The status record only stores a lightweight reference to the pending invocation;
                // hydrate the full invocation (including its payload) from the oplog before running.
                let timestamped_invocation = match self
                    .parent
                    .hydrate_pending_invocation(pending_invocation)
                    .await
                {
                    Ok(invocation) => invocation,
                    Err(error) => {
                        warn!(
                            agent_id = %self.owned_agent_id.agent_id,
                            "Failed to hydrate pending invocation from oplog: {error}"
                        );
                        break CommandOutcome::BreakInnerLoop(RetryDecision::Immediate);
                    }
                };

                // The span for picking work off the queue and running it: the root
                // of its own trace, linked back to whatever enqueued the work.
                // `otel.kind = consumer` is what the OpenTelemetry messaging
                // conventions prescribe for processing work a producer handed off.
                let pickup_span = related_span!(
                    origin,
                    Level::INFO,
                    "invocation_queue_pickup",
                    agent_id = %self.owned_agent_id.agent_id,
                    agent_type = %self.worker_trace.agent_type,
                    // The root of the execution's own trace, so it has to say which
                    // invocation it is: the link points back at the producer, but
                    // this key is what a search can join the two traces on. Left
                    // unset rather than empty when there is none, so a search for
                    // one key cannot collide with every keyless pickup.
                    idempotency_key = tracing::field::Empty,
                    otel.kind = "consumer",
                );

                if let Some(idempotency_key) = idempotency_key {
                    pickup_span.record("idempotency_key", tracing::field::display(idempotency_key));
                }

                let outcome = async {
                    let mut store = self.store.lock().await;
                    let mut invocation = Invocation {
                        owned_agent_id: self.owned_agent_id.clone(),
                        parent: self.parent.clone(),
                        instance: self.instance,
                        store: store.deref_mut(),
                    };
                    invocation.external_invocation(timestamped_invocation).await
                }
                .instrument(pickup_span)
                .await;

                match outcome {
                    CommandOutcome::Continue => {
                        if self.on_external_invocation_completed().await {
                            break CommandOutcome::Continue;
                        }
                        // Fairness: after completing one external durable
                        // invocation, yield to the scheduler so other same-account
                        // agents get a chance to run. The worker will self-wake
                        // and re-acquire its permit through the FIFO queue if
                        // more durable work remains.
                        let status = self.parent.get_non_detached_last_known_status().await;
                        if !status.pending_invocations.is_empty() {
                            // More durable work remains — self-wake so we return
                            // to the outer loop, release the permit (entering
                            // idle), and re-enter through the scheduler queue.
                            break CommandOutcome::WaitForWakeup;
                        }
                        continue;
                    }
                    other => break other,
                }
            }

            match self.periodic_snapshot_action(&status) {
                PeriodicSnapshotAction::DueNow => {
                    self.inject_snapshot_as_next_action().await;
                    break CommandOutcome::Continue;
                }
                PeriodicSnapshotAction::Wait(delay) => {
                    self.schedule_idle_snapshot(delay);
                    break CommandOutcome::WaitForWakeup;
                }
                PeriodicSnapshotAction::NotNeeded => {}
            }

            break CommandOutcome::WaitForWakeup;
        }
    }

    async fn on_external_invocation_completed(&mut self) -> bool {
        self.invocations_since_snapshot += 1;
        match self.parent.snapshot_policy() {
            SnapshotPolicy::EveryNInvocation { count } => {
                if self.invocations_since_snapshot >= *count as u64 {
                    self.invocations_since_snapshot = 0;
                    self.inject_snapshot_as_next_action().await;
                    true
                } else {
                    false
                }
            }
            SnapshotPolicy::Periodic { .. } => {
                let status = self.parent.get_non_detached_last_known_status().await;
                if matches!(
                    self.periodic_snapshot_action(&status),
                    PeriodicSnapshotAction::DueNow
                ) {
                    self.inject_snapshot_as_next_action().await;
                    true
                } else {
                    false
                }
            }
            SnapshotPolicy::Disabled => false,
        }
    }

    fn periodic_snapshot_action(&self, status: &AgentStatusRecord) -> PeriodicSnapshotAction {
        let SnapshotPolicy::Periodic { period } = self.parent.snapshot_policy() else {
            return PeriodicSnapshotAction::NotNeeded;
        };

        let created_at = self.parent.get_initial_worker_metadata().created_at;
        let last_snapshot_timestamp =
            snapshot_baseline_timestamp(status.last_automatic_snapshot_timestamp, created_at);

        snapshot_action_at(last_snapshot_timestamp, *period, Timestamp::now_utc())
    }

    fn schedule_idle_snapshot(&mut self, delay: Duration) {
        self.abort_idle_snapshot_task();
        self.idle_snapshot_task = Some(tokio::spawn(async move {
            tokio::time::sleep(delay).await;
        }));
    }

    async fn inject_snapshot_as_next_action(&self) {
        self.active
            .write()
            .await
            .push_front(QueuedWorkerInvocation::SaveSnapshot);
    }

    /// Resumes an interrupted replay process
    ///
    /// Returns `CommandOutcome` if this fails and the invocation loop should be stopped.
    /// Otherwise, it returns the new retry decision to be used by the outer invocation loop.
    async fn resume_replay(&self) -> CommandOutcome {
        async {
            let mut store = self.store.lock().await;

            let resume_replay_result = Ctx::resume_replay(&mut *store, self.instance, true).await;

            match resume_replay_result {
                Ok(None) => CommandOutcome::Continue,
                Ok(Some(decision)) => CommandOutcome::BreakInnerLoop(decision),
                Err(err) => {
                    warn!("Failed to resume replay: {err}");
                    store.data().set_suspended();
                    CommandOutcome::BreakOuterLoop(Some(err))
                }
            }
        }
        .instrument(agent_phase_span!(self, "resume_replay"))
        .await
    }

    /// Performs a queued invocation on the worker
    ///
    /// The queued invocations internal invocations that we use for
    /// concurrency control.
    async fn internal_invocation(&mut self, message: QueuedWorkerInvocation) -> CommandOutcome {
        let mut store = self.store.lock().await;
        let store = store.deref_mut();

        let mut invocation = Invocation {
            owned_agent_id: self.owned_agent_id.clone(),
            parent: self.parent.clone(),
            instance: self.instance,
            store,
        };
        invocation.process(message).await
    }

    /// Performs an interrupt request
    async fn interrupt(&self, interrupt: PendingWorkerInterrupt) -> CommandOutcome {
        CommandOutcome::BreakInnerLoop(interrupt.retry_decision())
    }
}

async fn wait_for_resident_wakeup(
    receiver: &mut UnboundedReceiver<WorkerCommand>,
    activity: &ResidentFilesystemActivity,
) -> ResidentWakeup {
    tokio::select! {
        command = receiver.recv() => command.map_or(
            ResidentWakeup::CommandChannelClosed,
            ResidentWakeup::Command,
        ),
        () = activity.wait_for_terminal_failure() => ResidentWakeup::FilesystemTerminalFailure,
    }
}

#[cfg(test)]
async fn close_usage_before_delete<E, Close, Delete, DeleteFuture>(
    close: Close,
    mark_permit_released: impl FnOnce(),
    delete: Delete,
) -> (Option<E>, Option<E>)
where
    Close: Future<Output = Option<E>>,
    Delete: FnOnce() -> DeleteFuture,
    DeleteFuture: Future<Output = Option<E>>,
{
    let close_error = close.await;
    mark_permit_released();
    let delete_error = delete().await;
    (close_error, delete_error)
}

struct UnloadObserver {
    receiver: tokio::sync::oneshot::Receiver<Option<WorkerExecutorError>>,
}

#[derive(Clone)]
struct UnloadCompletion {
    sender: Arc<StdMutex<Option<tokio::sync::oneshot::Sender<Option<WorkerExecutorError>>>>>,
}

impl UnloadCompletion {
    fn complete(&self, result: Option<WorkerExecutorError>) -> bool {
        let Some(sender) = self.sender.lock().unwrap().take() else {
            return false;
        };
        let _ = sender.send(result);
        true
    }

    fn is_pending(&self) -> bool {
        self.sender.lock().unwrap().is_some()
    }
}

impl Future for UnloadObserver {
    type Output = Option<WorkerExecutorError>;

    fn poll(
        mut self: std::pin::Pin<&mut Self>,
        context: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        match std::pin::Pin::new(&mut self.receiver).poll(context) {
            std::task::Poll::Ready(Ok(result)) => std::task::Poll::Ready(result),
            std::task::Poll::Ready(Err(_)) => std::task::Poll::Ready(Some(
                WorkerExecutorError::runtime("module-owned agent unload task stopped unexpectedly"),
            )),
            std::task::Poll::Pending => std::task::Poll::Pending,
        }
    }
}

#[cfg(test)]
fn spawn_module_owned_unload(
    task: impl Future<Output = Option<WorkerExecutorError>> + Send + 'static,
) -> UnloadObserver {
    spawn_module_owned_unload_continuation(move |completion| async move {
        completion.complete(task.await);
    })
}

fn spawn_module_owned_unload_continuation<Task, TaskFuture>(task: Task) -> UnloadObserver
where
    Task: FnOnce(UnloadCompletion) -> TaskFuture + Send + 'static,
    TaskFuture: Future<Output = ()> + Send + 'static,
{
    let (sender, receiver) = tokio::sync::oneshot::channel();
    let completion = UnloadCompletion {
        sender: Arc::new(StdMutex::new(Some(sender))),
    };
    let completion_after_task = completion.clone();
    tokio::spawn(async move {
        let result = std::panic::AssertUnwindSafe(async move { task(completion).await })
            .catch_unwind()
            .await;
        if result.is_err() {
            let error = WorkerExecutorError::runtime("module-owned agent unload task panicked");
            error!(error = %error, "Module-owned agent unload task panicked");
            completion_after_task.complete(Some(error));
        } else if completion_after_task.is_pending() {
            completion_after_task.complete(Some(WorkerExecutorError::runtime(
                "module-owned agent unload task stopped without lifecycle completion",
            )));
        }
    });
    UnloadObserver { receiver }
}

pub(super) struct ConcurrentAgentPermitState<T> {
    permit: Option<T>,
    window: Option<ResourceUsageMeteringWindow>,
    held: Arc<AtomicBool>,
}

impl<T> ConcurrentAgentPermitState<T> {
    pub(super) fn new(permit: Option<T>, held: Arc<AtomicBool>) -> Self {
        held.store(permit.is_some(), Ordering::Release);
        Self {
            permit,
            window: None,
            held,
        }
    }

    fn is_some(&self) -> bool {
        self.permit.is_some() || self.window.is_some()
    }

    fn is_none(&self) -> bool {
        self.permit.is_none() && self.window.is_none()
    }

    fn install(&mut self, permit: T) {
        debug_assert!(self.permit.is_none());
        self.permit = Some(permit);
        self.held.store(true, Ordering::Release);
    }

    fn take_permit(&mut self) -> Option<T> {
        self.permit.take()
    }

    fn install_window(&mut self, window: ResourceUsageMeteringWindow) {
        debug_assert!(self.permit.is_none());
        debug_assert!(self.window.is_none());
        self.window = Some(window);
        self.held.store(true, Ordering::Release);
    }

    fn take_window(&mut self) -> Option<ResourceUsageMeteringWindow> {
        self.window.take()
    }

    fn mark_released(&self) {
        self.held.store(false, Ordering::Release);
    }

    fn release(&mut self) {
        if let Some(permit) = self.permit.take() {
            drop(permit);
            self.held.store(false, Ordering::Release);
        } else if let Some(window) = self.window.take() {
            drop(window);
        }
    }
}

impl ConcurrentAgentPermitState<crate::services::active_agents::ConcurrentAgentPermit> {
    fn track(
        &self,
        permit: crate::services::active_agents::ConcurrentAgentPermit,
    ) -> crate::services::active_agents::ConcurrentAgentPermit {
        permit.track_held(Arc::clone(&self.held))
    }

    fn install_tracked(&mut self, permit: crate::services::active_agents::ConcurrentAgentPermit) {
        self.install(permit.track_held(Arc::clone(&self.held)));
    }
}

fn resource_usage_close_deadline() -> Instant {
    Instant::now() + Duration::from_secs(30)
}

fn mark_idle(idle_since_millis: &AtomicU64) {
    let now = Timestamp::now_utc().to_millis();
    let _ = idle_since_millis.fetch_update(Ordering::Release, Ordering::Acquire, |previous| {
        Some(now.max(previous.saturating_add(1)))
    });
}

async fn take_pending_interrupt(
    signal: &Mutex<WorkerInterruptState>,
) -> Option<PendingWorkerInterrupt> {
    signal.lock().await.take()
}

/// Context for performing one `QueuedWorkerInvocation`
///
/// The most important part is that unlike the `InnerInvocationLoop`, it holds a locked
/// mutable reference to the instance `Store`. The instance mutex is held for the whole duration
/// of performing an invocation.
struct Invocation<'a, Ctx: WorkerCtx> {
    owned_agent_id: OwnedAgentId,
    parent: Arc<Worker<Ctx>>, // parent must not be dropped until the invocation_loop is running
    instance: &'a Instance,
    store: &'a mut Store<Ctx>,
}

impl<Ctx: WorkerCtx> Invocation<'_, Ctx> {
    /// Process a queued worker invocation
    async fn process(&mut self, message: QueuedWorkerInvocation) -> CommandOutcome {
        match message {
            QueuedWorkerInvocation::GetFileSystemNode { path, sender } => {
                self.get_file_system_node(path, sender).await;
                CommandOutcome::Continue
            }
            QueuedWorkerInvocation::GetWalletCards { sender } => {
                let wallet = self
                    .store
                    .data_mut()
                    .durable_ctx_mut()
                    .active_agent_wallet_cards_snapshot()
                    .await;
                let _ = sender.send(wallet);
                CommandOutcome::Continue
            }
            QueuedWorkerInvocation::ReadFile { path, sender } => {
                self.read_file(path, sender).await;
                CommandOutcome::Continue
            }
            QueuedWorkerInvocation::AwaitReadyToProcessCommands { sender } => {
                let _ = sender.send(Ok(()));
                CommandOutcome::Continue
            }
            QueuedWorkerInvocation::SaveSnapshot => self.save_snapshot().await,
        }
    }

    /// Process an external queued worker invocation - this is either an exported function invocation
    /// or a manual update request (which involves invoking the exported save-snapshot functions, so
    /// it is a special case of the exported function invocation).
    async fn external_invocation(&mut self, inner: TimestampedAgentInvocation) -> CommandOutcome {
        match inner.invocation {
            AgentInvocation::ManualUpdate { target_revision } => {
                self.manual_update(target_revision).await
            }
            invocation => {
                if let Some(idempotency_key) = invocation.idempotency_key() {
                    let has_result = {
                        let invocation_results = self.parent.invocation_results.read().await;
                        invocation_results.contains_key(idempotency_key)
                    };
                    if !has_result {
                        self.invoke_agent(invocation).await
                    } else {
                        debug!(
                            "Skipping enqueued invocation with idempotency key {idempotency_key} as it already has a result"
                        );
                        if let Err(error) =
                            self.parent.cancel_invocation(idempotency_key.clone()).await
                        {
                            warn!(
                                agent_id = %self.owned_agent_id.agent_id,
                                "Failed to remove completed invocation from the pending queue: {error}"
                            );
                            return CommandOutcome::BreakInnerLoop(RetryDecision::Immediate);
                        }
                        CommandOutcome::Continue
                    }
                } else {
                    self.invoke_agent(invocation).await
                }
            }
        }
    }

    /// Invokes an agent function on the worker
    async fn invoke_agent(&mut self, invocation: AgentInvocation) -> CommandOutcome {
        let display_name = invocation.display_name();
        let invocation_context = invocation.invocation_context();
        let idempotency_key = invocation
            .idempotency_key()
            .cloned()
            .unwrap_or_else(IdempotencyKey::fresh);

        let span = span!(
            Level::INFO,
            "invocation",
            agent_id = %self.owned_agent_id.agent_id,
            agent_type = self.parent.agent_type_label(),
            %idempotency_key,
            function = display_name
        );

        self.invoke_agent_inner(invocation_context, idempotency_key, invocation)
            .instrument(span)
            .await
    }

    /// Invokes an agent function on the worker
    ///
    /// The inner implementation of `invoke_agent` to be instrumented with a span.
    async fn invoke_agent_inner(
        &mut self,
        invocation_context: InvocationContextStack,
        idempotency_key: IdempotencyKey,
        invocation: AgentInvocation,
    ) -> CommandOutcome {
        let kind = invocation.kind();
        let display_name = invocation.display_name();
        let invocation_idempotency_key = idempotency_key.clone();
        let result = self
            .invoke_agent_with_context(invocation_context, idempotency_key, invocation)
            .await;

        match result {
            Ok(InvokeResult::Succeeded {
                result: mut invocation_result,
                consumed_fuel,
            }) => {
                let mut interrupt_state = self.parent.interrupt_signal.lock().await;
                if let Some(interrupt) = interrupt_state.claim_pending_terminal() {
                    drop(interrupt_state);
                    self.agent_invocation_failed(
                        &display_name,
                        &invocation_idempotency_key,
                        Ok(InvokeResult::Interrupted {
                            consumed_fuel,
                            interrupt_kind: interrupt.kind,
                        }),
                    )
                    .await
                } else {
                    drop(interrupt_state);
                    if let AgentInvocationResult::AgentMethod { output } = &mut invocation_result {
                        let component = self.store.data().component_metadata();
                        let Some(agent_type) =
                            self.parent.parsed_agent_id.as_ref().and_then(|parsed| {
                                component
                                    .metadata
                                    .find_agent_type_by_name_ref(&parsed.agent_type)
                            })
                        else {
                            return self
                                .agent_invocation_failed(
                                    &display_name,
                                    &invocation_idempotency_key,
                                    Err(WorkerExecutorError::runtime(
                                        "durable invocation result schema is unavailable",
                                    )),
                                )
                                .await;
                        };
                        let Some(method) = agent_type
                            .methods
                            .iter()
                            .find(|method| method.name == display_name)
                        else {
                            return self
                                .agent_invocation_failed(
                                    &display_name,
                                    &invocation_idempotency_key,
                                    Err(WorkerExecutorError::runtime(
                                        "durable invocation result method schema is unavailable",
                                    )),
                                )
                                .await;
                        };
                        let graph = agent_type.schema.clone();
                        let root =
                            method.output_schema.schema().cloned().unwrap_or_else(|| {
                                golem_common::schema::SchemaType::tuple(Vec::new())
                            });
                        let component_revision = component.revision;
                        let parent = self.parent.clone();
                        let idempotency_key = invocation_idempotency_key.clone();
                        let result_value = output.clone();
                        match self
                            .store
                            .run_concurrent(async move |_accessor| {
                                parent
                                    .materialize_durable_streaming_result(
                                        &idempotency_key,
                                        result_value,
                                        &graph,
                                        &root,
                                        component_revision,
                                    )
                                    .await
                            })
                            .await
                        {
                            Ok(Ok(materialized)) => *output = materialized,
                            Ok(Err(error)) => {
                                return self
                                    .agent_invocation_failed(
                                        &display_name,
                                        &invocation_idempotency_key,
                                        Err(error),
                                    )
                                    .await;
                            }
                            Err(error) => {
                                return self
                                    .agent_invocation_failed(
                                        &display_name,
                                        &invocation_idempotency_key,
                                        Err(WorkerExecutorError::runtime(error.to_string())),
                                    )
                                    .await;
                            }
                        }
                    }
                    self.agent_invocation_finished(
                        display_name,
                        &invocation_idempotency_key,
                        invocation_result,
                        consumed_fuel,
                        kind,
                    )
                    .await
                }
            }
            result @ Ok(InvokeResult::Interrupted { interrupt_kind, .. }) => {
                if !matches!(interrupt_kind, InterruptKind::Restart | InterruptKind::Jump) {
                    self.parent
                        .interrupt_signal
                        .lock()
                        .await
                        .claim_pending_terminal();
                }
                self.agent_invocation_failed(&display_name, &invocation_idempotency_key, result)
                    .await
            }
            _ => {
                self.agent_invocation_failed(&display_name, &invocation_idempotency_key, result)
                    .await
            }
        }
    }

    /// Sets the necessary contextual information on the worker and performs the actual
    /// invocation.
    async fn invoke_agent_with_context(
        &mut self,
        mut invocation_context: InvocationContextStack,
        idempotency_key: IdempotencyKey,
        invocation: AgentInvocation,
    ) -> Result<InvokeResult, WorkerExecutorError> {
        let (lowered, local_span_ids, inherited_span_ids) = async {
            self.store
                .data_mut()
                .set_current_idempotency_key(idempotency_key.clone())
                .await;

            let component_metadata = self.store.data().component_metadata().metadata.clone();

            Self::extend_invocation_context(
                &mut invocation_context,
                &idempotency_key,
                &invocation,
                &self.owned_agent_id.agent_id(),
                &self.parent.parsed_agent_id,
            );

            let (local_span_ids, inherited_span_ids) = invocation_context.span_ids();
            self.store
                .data_mut()
                .set_current_invocation_context(invocation_context)
                .await?;

            if let Some(idempotency_key) = self.store.data().get_current_idempotency_key().await {
                self.store
                    .data()
                    .get_public_state()
                    .worker()
                    .store_invocation_resuming(&idempotency_key)
                    .await;
            }

            let invocation_for_lowering = self
                .parent
                .rehydrate_durable_streaming_invocation(invocation.clone())
                .await?;
            let lowered = lower_invocation(
                invocation_for_lowering,
                &component_metadata,
                self.parent.parsed_agent_id.as_ref(),
            )?;

            Ok::<_, WorkerExecutorError>((lowered, local_span_ids, inherited_span_ids))
        }
        .instrument(span!(Level::INFO, "prepare_invocation_context"))
        .await?;

        let result = invoke_observed_and_traced(
            lowered,
            self.store,
            self.instance,
            InvocationMode::Live(invocation),
        )
        .await;

        // We are removing the spans introduced by the invocation. Not calling `finish_span` here,
        // as it would add FinishSpan oplog entries without corresponding StartSpan ones. Instead,
        // the oplog processor should assume that spans implicitly created by AgentInvocationStarted
        // are finished at AgentInvocationFinished.
        for span_id in local_span_ids {
            self.store.data_mut().remove_span(&span_id)?;
        }
        for span_id in inherited_span_ids {
            self.store.data_mut().remove_span(&span_id)?;
        }

        result
    }

    /// The logic handling a successfully finished agent invocation
    ///
    /// Successful here means that the invocation function returned with
    /// `InvokeResult::Succeeded`. As the returned values get further processing,
    /// the whole invocation can still fail during that.
    async fn agent_invocation_finished(
        &mut self,
        full_function_name: String,
        idempotency_key: &IdempotencyKey,
        invocation_result: AgentInvocationResult,
        consumed_fuel: u64,
        kind: AgentInvocationKind,
    ) -> CommandOutcome {
        let component_revision = self.store.data().component_metadata().revision;
        let mut output = AgentInvocationOutput {
            result: invocation_result,
            consumed_fuel: Some(consumed_fuel),
            invocation_status: None,
            component_revision: Some(component_revision),
            agent_id: None,
            idempotency_key: None,
            oplog_index: None,
            agent_fingerprint: None,
        };
        match self
            .store
            .data_mut()
            .on_agent_invocation_success(&full_function_name, consumed_fuel, &mut output)
            .await
        {
            Ok(()) => {
                if let Err(error) = self
                    .parent
                    .complete_durable_streaming_session(idempotency_key)
                    .await
                {
                    tracing::error!(%error, "Failed to complete durable streaming session");
                    return failed_agent_invocation_outcome(
                        self.parent.agent_mode(),
                        RetryDecision::Immediate,
                    );
                }
                successful_agent_invocation_outcome(
                    self.parent.agent_mode(),
                    self.store.data().component_metadata().metadata.is_agent(),
                    kind,
                )
            }
            Err(error) => {
                self.store
                    .data_mut()
                    .on_invocation_failure(
                        &full_function_name,
                        &TrapType::Error {
                            error: AgentError::InternalError(error.to_string()),
                            retry_from: OplogIndex::INITIAL,
                            in_atomic_region: false,
                            atomic_region_had_side_effects: false,
                            semantic_trap_retry_override: None,
                        },
                    )
                    .await;
                let _ = self
                    .parent
                    .fail_durable_streaming_session(idempotency_key, error.to_string())
                    .await;
                failed_agent_invocation_outcome(self.parent.agent_mode(), RetryDecision::None)
            }
        }
    }

    /// The logic handling an agent invocation that did not succeed.
    async fn agent_invocation_failed(
        &mut self,
        full_function_name: &str,
        idempotency_key: &IdempotencyKey,
        result: Result<InvokeResult, WorkerExecutorError>,
    ) -> CommandOutcome {
        self.store
            .data()
            .durable_ctx()
            .begin_stream_runtime_teardown();
        let details = format!("{result:?}");
        let trap_type = match result {
            Ok(invoke_result) => invoke_result.as_trap_type::<Ctx>(),
            Err(error) => Some(TrapType::from_worker_executor_error::<Ctx>(
                error,
                OplogIndex::INITIAL,
                false,
                false,
                self.parent.agent_mode(),
            )),
        };
        let decision = match trap_type {
            Some(trap_type) => {
                self.store
                    .data_mut()
                    .on_invocation_failure(full_function_name, &trap_type)
                    .await
            }
            None => RetryDecision::None,
        };

        if decision == RetryDecision::None {
            let _ = self
                .parent
                .fail_durable_streaming_session(idempotency_key, details)
                .await;
        }

        failed_agent_invocation_outcome(self.parent.agent_mode(), decision)
    }

    /// Try to perform the save-snapshot step of a manual update on the worker
    async fn manual_update(&mut self, target_revision: ComponentRevision) -> CommandOutcome {
        let span = span!(
            Level::INFO,
            "manual_update",
            agent_id = %self.owned_agent_id.agent_id,
            target_revision = %target_revision,
            agent_type = self.parent
                .parsed_agent_id
                .as_ref()
                .map(|id| id.agent_type.to_string())
                .unwrap_or_else(|| "-".to_string()),
        );

        self.manual_update_inner(target_revision)
            .instrument(span)
            .await
    }

    /// The inner implementation of the manual update command
    async fn manual_update_inner(&mut self, target_revision: ComponentRevision) -> CommandOutcome {
        // The saved snapshot becomes the replay cut point of the snapshot-based update: after the
        // update, replay starts from the snapshot and skips everything before it. No durable call
        // or scope may span that cut, so refuse the update while any is still open.
        if let Some(blocker) = self.store.data().snapshot_boundary_blocker() {
            return self
                .fail_update(
                    target_revision,
                    format!("cannot take a snapshot for the update: {blocker}"),
                )
                .await;
        }

        let idempotency_key = {
            let ctx = self.store.data_mut();
            let idempotency_key = IdempotencyKey::fresh();
            ctx.set_current_idempotency_key(idempotency_key.clone())
                .await;
            idempotency_key
        };
        let component_metadata = self.store.data().component_metadata().metadata.clone();

        let save_snapshot_invocation = AgentInvocation::SaveSnapshot { idempotency_key };
        let lowered = match lower_invocation(
            save_snapshot_invocation,
            &component_metadata,
            self.parent.parsed_agent_id.as_ref(),
        ) {
            Ok(lowered) => lowered,
            Err(err) => {
                warn!("Failed to lower save-snapshot invocation: {err}");
                return self
                    .fail_update(
                        target_revision,
                        format!("failed to lower save-snapshot invocation: {err}"),
                    )
                    .await;
            }
        };

        let invocation_context = InvocationContextStack::fresh();
        let (local_span_ids, inherited_span_ids) = invocation_context.span_ids();
        if let Err(err) = self
            .store
            .data_mut()
            .set_current_invocation_context(invocation_context)
            .await
        {
            warn!("Failed to install invocation context for manual update save-snapshot: {err}");
            return self
                .fail_update(
                    target_revision,
                    format!("failed to install invocation context for save-snapshot: {err}"),
                )
                .await;
        }

        self.store
            .data_mut()
            .durable_ctx_mut()
            .begin_call_snapshotting_function();
        let result =
            invoke_observed_and_traced(lowered, self.store, self.instance, InvocationMode::Replay)
                .await;
        self.store
            .data_mut()
            .durable_ctx_mut()
            .end_call_snapshotting_function_if_active();

        for span_id in local_span_ids {
            let _ = self.store.data_mut().remove_span(&span_id);
        }
        for span_id in inherited_span_ids {
            let _ = self.store.data_mut().remove_span(&span_id);
        }

        match result {
            Ok(InvokeResult::Succeeded {
                result: AgentInvocationResult::SaveSnapshot { snapshot },
                ..
            }) => {
                match self
                    .store
                    .data()
                    .get_public_state()
                    .oplog()
                    .create_snapshot_based_update_description(
                        target_revision,
                        snapshot.data,
                        snapshot.mime_type,
                    )
                    .await
                {
                    Ok(update_description) => {
                        // Enqueue the update
                        self.parent.enqueue_update(update_description).await;

                        // Reactivate the worker
                        CommandOutcome::BreakInnerLoop(RetryDecision::Immediate)
                        // Stop processing the queue to avoid race conditions
                    }
                    Err(error) => {
                        self.fail_update(
                            target_revision,
                            format!("failed to store the snapshot for manual update: {error}"),
                        )
                        .await
                    }
                }
            }
            Ok(InvokeResult::Succeeded { .. }) => {
                self.fail_update(
                    target_revision,
                    "failed to get a snapshot for manual update: invalid snapshot result"
                        .to_string(),
                )
                .await
            }
            Ok(InvokeResult::Failed { error, .. }) => {
                let stderr = self
                    .store
                    .data()
                    .get_public_state()
                    .event_service()
                    .get_last_invocation_errors();
                let error = error.to_string(&stderr);
                self.fail_update(
                    target_revision,
                    format!("failed to get a snapshot for manual update: {error}"),
                )
                .await
            }
            Ok(InvokeResult::Exited { .. }) => {
                self.fail_update(
                    target_revision,
                    "failed to get a snapshot for manual update: it called exit".to_string(),
                )
                .await
            }
            Ok(InvokeResult::Interrupted { interrupt_kind, .. }) => {
                self.fail_update(
                    target_revision,
                    format!("failed to get a snapshot for manual update: {interrupt_kind:?}"),
                )
                .await
            }
            Err(error) => {
                self.fail_update(
                    target_revision,
                    format!("failed to get a snapshot for manual update: {error:?}"),
                )
                .await
            }
        }
    }

    /// Performs a directory listing command on the worker's file system
    ///
    /// These are threaded through the invocation loop to make sure they are not accessing the file system concurrently with invocations
    /// that may modify them.
    async fn get_file_system_node(
        &self,
        path: CanonicalFilePath,
        sender: Sender<Result<GetFileSystemNodeResult, WorkerExecutorError>>,
    ) {
        let _filesystem_access = match self
            .store
            .data()
            .durable_ctx()
            .acquire_owner_filesystem_inspection()
            .await
        {
            Ok(access) => access,
            Err(error) => {
                let _ = sender.send(Err(error));
                return;
            }
        };
        let result = self.store.data().get_file_system_node(&path).await;
        let _ = sender.send(result);
    }

    /// Performs a read file command on the worker's file system
    ///
    /// These are threaded through the invocation loop to make sure they are not accessing the file system concurrently with invocations
    /// that may modify them.
    async fn read_file(
        &self,
        path: CanonicalFilePath,
        sender: Sender<Result<ReadFileResult, WorkerExecutorError>>,
    ) {
        let _filesystem_access = match self
            .store
            .data()
            .durable_ctx()
            .acquire_owner_filesystem_inspection()
            .await
        {
            Ok(access) => access,
            Err(error) => {
                let _ = sender.send(Err(error));
                return;
            }
        };
        let result = self.store.data().read_file(&path).await;
        match result {
            Ok(ReadFileResult::Ok(stream)) => {
                // special case. We need to wait until the stream is consumed to avoid corruption
                //
                // This will delay processing of the next invocation and is quite unfortunate.
                // A possible improvement would be to check whether we are on a copy-on-write filesystem
                // if yes, we can make a cheap copy of the file here and serve the read from that copy.

                let (latch, latch_receiver) = oneshot::channel();
                let drop_stream = DropStream::new(stream, || latch.send(()).unwrap());
                let _ = sender.send(Ok(ReadFileResult::Ok(Box::pin(drop_stream))));
                latch_receiver.await.unwrap();
            }
            other => {
                let _ = sender.send(other);
            }
        };
    }

    /// Records an attempted worker update as failed
    async fn fail_update(
        &self,
        target_revision: ComponentRevision,
        error: String,
    ) -> CommandOutcome {
        self.store
            .data()
            .on_worker_update_failed(target_revision, Some(error))
            .await;
        CommandOutcome::Continue
    }

    /// Extends the invocation context with a new span containing information about the invocation
    fn extend_invocation_context(
        invocation_context: &mut InvocationContextStack,
        idempotency_key: &IdempotencyKey,
        invocation: &AgentInvocation,
        agent_id: &AgentId,
        parsed_agent_id: &Option<ParsedAgentId>,
    ) {
        let invocation_span = invocation_context.spans.first().start_span(None);
        invocation_span.set_attribute(
            "name".to_string(),
            AttributeValue::String("invoke-exported-function".to_string()),
        );
        invocation_span.set_attribute(
            "idempotency_key".to_string(),
            AttributeValue::String(idempotency_key.to_string()),
        );
        invocation_span.set_attribute(
            "function_name".to_string(),
            AttributeValue::String(invocation.display_name()),
        );
        invocation_span.set_attribute(
            "invocation_kind".to_string(),
            AttributeValue::String(format!("{:?}", invocation.kind())),
        );
        invocation_span.set_attribute(
            "agent_id".to_string(),
            AttributeValue::String(agent_id.to_string()),
        );
        if let Some(parsed_agent_id) = parsed_agent_id {
            invocation_span.set_attribute(
                "agent_type".to_string(),
                AttributeValue::String(parsed_agent_id.agent_type.to_string()),
            );
            invocation_span.set_attribute(
                "agent_parameters".to_string(),
                AttributeValue::String(
                    format_structural_typed(&parsed_agent_id.parameters)
                        .unwrap_or_else(|err| format!("Cannot render: {}", err)),
                ),
            )
        }
        invocation_context.push(invocation_span);
    }

    async fn save_snapshot(&mut self) -> CommandOutcome {
        // A committed snapshot is a replay cut point (snapshot-based recovery skips everything
        // before it), so no durable call or scope may span it. Skip this periodic snapshot when
        // the worker is not at a safe boundary; the next scheduled snapshot will retry.
        if let Some(blocker) = self.store.data().snapshot_boundary_blocker() {
            warn!("Skipping periodic snapshot: {blocker}");
            return CommandOutcome::Continue;
        }

        let idempotency_key = IdempotencyKey::fresh();
        self.store
            .data_mut()
            .set_current_idempotency_key(idempotency_key.clone())
            .await;
        let component_metadata = self.store.data().component_metadata().metadata.clone();

        let save_snapshot_invocation = AgentInvocation::SaveSnapshot { idempotency_key };
        let lowered = match lower_invocation(
            save_snapshot_invocation,
            &component_metadata,
            self.parent.parsed_agent_id.as_ref(),
        ) {
            Ok(lowered) => lowered,
            Err(err) => {
                warn!("Failed to lower save-snapshot invocation: {err}");
                return CommandOutcome::Continue;
            }
        };

        let invocation_context = InvocationContextStack::fresh();
        let (local_span_ids, inherited_span_ids) = invocation_context.span_ids();
        if let Err(err) = self
            .store
            .data_mut()
            .set_current_invocation_context(invocation_context)
            .await
        {
            warn!("Failed to install invocation context for periodic save-snapshot: {err}");
            return CommandOutcome::Continue;
        }

        self.store
            .data_mut()
            .durable_ctx_mut()
            .begin_call_snapshotting_function();
        let result =
            invoke_observed_and_traced(lowered, self.store, self.instance, InvocationMode::Replay)
                .await;
        self.store
            .data_mut()
            .durable_ctx_mut()
            .end_call_snapshotting_function_if_active();

        for span_id in local_span_ids {
            let _ = self.store.data_mut().remove_span(&span_id);
        }
        for span_id in inherited_span_ids {
            let _ = self.store.data_mut().remove_span(&span_id);
        }

        if let Some(outcome) = periodic_snapshot_failure_outcome(&result) {
            match &result {
                Ok(InvokeResult::Failed { .. }) => {
                    warn!(
                        "Periodic snapshot save function failed; restarting worker to recover the store"
                    );
                }
                Err(err) => {
                    warn!(
                        "Periodic snapshot save invocation error: {err}; restarting worker to recover the store"
                    );
                }
                _ => unreachable!(),
            }

            return outcome;
        }

        match result {
            Ok(InvokeResult::Succeeded {
                result: AgentInvocationResult::SaveSnapshot { snapshot },
                ..
            }) => {
                let serialized = golem_common::serialization::serialize(&snapshot.data);
                match serialized {
                    Ok(serialized_bytes) => {
                        match self.parent.oplog.upload_raw_payload(serialized_bytes).await {
                            Ok(raw_payload) => match raw_payload.into_payload::<Vec<u8>>() {
                                Ok(payload) => {
                                    let active_cards = self
                                        .store
                                        .data()
                                        .durable_ctx()
                                        .agent_wallet_cards_snapshot();
                                    let wallet_generation =
                                        self.store.data().durable_ctx().wallet_generation();
                                    self.parent
                                        .add_and_commit_oplog(OplogEntry::snapshot(
                                            payload,
                                            snapshot.mime_type,
                                            active_cards,
                                            wallet_generation,
                                        ))
                                        .await;
                                    debug!("Periodic snapshot saved successfully");

                                    // A snapshot is committed between invocations, so no jumpable
                                    // region is open: a clean boundary to checkpoint the status,
                                    // aligning the checkpoint with the snapshot index.
                                    self.parent
                                        .checkpoint_status(
                                            status_checkpointer::CheckpointReason::Snapshot,
                                        )
                                        .await;
                                }
                                Err(err) => {
                                    warn!("Failed to convert snapshot payload: {err}");
                                }
                            },
                            Err(err) => {
                                warn!("Failed to upload periodic snapshot payload: {err}");
                            }
                        }
                    }
                    Err(err) => {
                        warn!("Failed to serialize snapshot data: {err}");
                    }
                }
                CommandOutcome::Continue
            }
            Ok(InvokeResult::Succeeded { .. }) => {
                warn!("Periodic snapshot returned unexpected result format");
                CommandOutcome::Continue
            }
            Ok(InvokeResult::Exited { .. }) => {
                warn!("Worker exited during periodic snapshot save");
                CommandOutcome::BreakInnerLoop(RetryDecision::None)
            }
            Ok(InvokeResult::Interrupted { .. }) => {
                warn!("Worker interrupted during periodic snapshot save");
                CommandOutcome::BreakInnerLoop(RetryDecision::None)
            }
            Ok(InvokeResult::Failed { .. }) | Err(_) => unreachable!(),
        }
    }
}

/// Outcome of processing a single command within the inner invocation loop
#[derive(Debug, PartialEq, Eq)]
enum CommandOutcome {
    /// Break from both the inner and outer loops, there is no way to retry anything
    BreakOuterLoop(Option<WorkerExecutorError>),
    /// Break from the inner loop, setting the retry decision for the outer loop
    BreakInnerLoop(RetryDecision),
    /// Break from the inner loop and archive the stopped ephemeral worker's oplog.
    BreakInnerLoopAndArchiveEphemeralOplog(RetryDecision),
    /// Continue processing in the inner loop
    Continue,
    /// Stop draining for now and wait for the next command or idle timer wakeup
    WaitForWakeup,
}

struct InnerInvocationLoopResult {
    retry_decision: Option<RetryDecision>,
    final_interrupt: Option<InterruptKind>,
    unload_request: Option<UnloadRequest>,
    cleanup_ephemeral_worker: bool,
    recovery_failure: Option<WorkerExecutorError>,
    filesystem_limit_update: Option<PendingFilesystemLimitUpdate>,
}

fn successful_agent_invocation_outcome(
    agent_mode: AgentMode,
    is_agent_component: bool,
    kind: AgentInvocationKind,
) -> CommandOutcome {
    if should_cleanup_terminal_ephemeral_invocation(agent_mode, is_agent_component, kind) {
        CommandOutcome::BreakInnerLoopAndArchiveEphemeralOplog(RetryDecision::None)
    } else {
        CommandOutcome::Continue
    }
}

fn failed_agent_invocation_outcome(
    agent_mode: AgentMode,
    decision: RetryDecision,
) -> CommandOutcome {
    if agent_mode == AgentMode::Ephemeral && decision == RetryDecision::None {
        CommandOutcome::BreakInnerLoopAndArchiveEphemeralOplog(decision)
    } else {
        CommandOutcome::BreakInnerLoop(decision)
    }
}

fn should_cleanup_terminal_ephemeral_invocation(
    agent_mode: AgentMode,
    is_agent_component: bool,
    kind: AgentInvocationKind,
) -> bool {
    agent_mode == AgentMode::Ephemeral
        && !(is_agent_component && kind == AgentInvocationKind::AgentInitialization)
}

#[derive(Debug, PartialEq, Eq)]
enum PeriodicSnapshotAction {
    NotNeeded,
    DueNow,
    Wait(Duration),
}

fn periodic_snapshot_failure_outcome(
    result: &Result<InvokeResult, WorkerExecutorError>,
) -> Option<CommandOutcome> {
    match result {
        Ok(InvokeResult::Failed { .. }) | Err(_) => {
            Some(CommandOutcome::BreakInnerLoop(RetryDecision::Immediate))
        }
        _ => None,
    }
}

fn snapshot_baseline_timestamp(
    last_snapshot_timestamp: Option<Timestamp>,
    created_at: Timestamp,
) -> Timestamp {
    last_snapshot_timestamp.unwrap_or(created_at)
}

fn snapshot_action_at(
    last_snapshot_timestamp: Timestamp,
    period: Duration,
    now: Timestamp,
) -> PeriodicSnapshotAction {
    let period_millis = period.as_millis();
    if period_millis == 0 {
        return PeriodicSnapshotAction::DueNow;
    }

    let now = now.to_millis() as u128;
    let due_at = last_snapshot_timestamp.to_millis() as u128 + period_millis;

    if now >= due_at {
        PeriodicSnapshotAction::DueNow
    } else {
        PeriodicSnapshotAction::Wait(Duration::from_millis((due_at - now) as u64))
    }
}

#[cfg(test)]
mod tests {
    use super::{
        CommandOutcome, ConcurrentAgentPermitState, InvocationLoop, PeriodicSnapshotAction,
        ResidentAgentOwnership, ResidentWakeup, catch_invocation_loop_panic,
        close_usage_before_delete, coalesce_filesystem_limit_update,
        failed_agent_invocation_outcome, finish_filesystem_limit_unload,
        periodic_snapshot_failure_outcome, publish_unload_outcome, run_invocation_loop_task,
        snapshot_action_at, snapshot_baseline_timestamp, spawn_module_owned_unload,
        successful_agent_invocation_outcome, unload_resident_agent_ownership,
        wait_for_resident_wakeup,
    };
    use crate::sandbox_filesystem::ScriptedSandboxFilesystem;
    use crate::services::active_agents::stop_loaded_idle_if_eligible;
    use crate::services::agent_filesystem::{
        AccessError, FilesystemStorageError, FlushLevel, Follow, OpenNode, PathTarget, Target,
        attributes, billing_metered_resident_with_open_node_for_unload_test, close, delete,
        filesystem_activity, flush, metered_resident_with_open_node_for_unload_test,
        resident_for_unload_test, seal,
    };
    use crate::services::resource_usage_metering::close_window;
    use crate::worker::invocation::InvokeResult;
    use crate::worker::{
        EvictionClass, FilesystemPressureEligibility, FinalWorkerState,
        PendingLiveInvocationDisposition, RetryDecision, RunningAgent, StoppingWorker,
        UnloadReason, WorkerCommand, WorkerInstance, complete_stopping_worker,
    };
    use crate::workerctx::default::Context;
    use golem_common::model::AgentInvocationKind;
    use golem_common::model::agent::AgentMode;
    use golem_common::model::oplog::AgentError;
    use golem_common::model::{OplogIndex, Timestamp};
    use golem_service_base::error::worker_executor::WorkerExecutorError;
    use std::collections::VecDeque;
    use std::path::Path;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};
    use test_r::{test, timeout};

    struct TestPermit {
        held: Arc<AtomicBool>,
        drops: Arc<AtomicUsize>,
        held_while_dropping: Arc<AtomicBool>,
    }

    struct TestStoreOwner {
        node: Option<OpenNode>,
        dropped: Arc<AtomicBool>,
    }

    #[test]
    #[timeout("5s")]
    async fn loaded_idle_wait_wakes_for_a_terminal_filesystem_failure() {
        let (filesystem, control, window, generation_handle, node) =
            metered_resident_with_open_node_for_unload_test().await;
        let activity = filesystem_activity(&filesystem);
        let (_commands, mut receiver) = tokio::sync::mpsc::unbounded_channel();
        let waiting =
            tokio::spawn(async move { wait_for_resident_wakeup(&mut receiver, &activity).await });
        control.push_flush(Err(FilesystemStorageError::io(
            "detached terminal flush",
            Path::new("<idle-wakeup-test>"),
            std::io::ErrorKind::Other.into(),
        )));

        assert!(matches!(
            flush(&generation_handle, &node, FlushLevel::Data)
                .unwrap()
                .await,
            Err(crate::services::agent_filesystem::Error::RuntimeInvalidated)
        ));
        assert!(matches!(
            waiting.await.unwrap(),
            ResidentWakeup::FilesystemTerminalFailure
        ));

        control.push_observe_allocation(Err(FilesystemStorageError::verification(
            "observe unsupported allocation during idle wakeup cleanup",
            Path::new("<idle-wakeup-test>"),
        )));
        close_window(window, Instant::now() + Duration::from_secs(1))
            .await
            .unwrap();
        control.push_close(Ok(()));
        close(node).await.unwrap();
        control.push_delete_and_verify(Ok(()));
        delete(seal(filesystem)).await.unwrap();
    }

    impl Drop for TestStoreOwner {
        fn drop(&mut self) {
            self.dropped.store(true, Ordering::Release);
            drop(self.node.take());
        }
    }

    impl Drop for TestPermit {
        fn drop(&mut self) {
            self.held_while_dropping
                .store(self.held.load(Ordering::Acquire), Ordering::Release);
            self.drops.fetch_add(1, Ordering::AcqRel);
        }
    }

    #[test]
    async fn idle_filesystem_limit_downgrade_upgrade_commands_coalesce_without_lost_work() {
        let (commands, mut receiver) = tokio::sync::mpsc::unbounded_channel();
        let (downgrade_sender, downgrade_result) = futures::channel::oneshot::channel();
        let (upgrade_sender, upgrade_result) = futures::channel::oneshot::channel();
        commands.send(WorkerCommand::WorkAvailable).unwrap();
        commands
            .send(WorkerCommand::UpdateFilesystemLimit {
                allocated_bytes: 1024 * 1024,
                sender: upgrade_sender,
            })
            .unwrap();
        let mut deferred_wakeups = VecDeque::new();

        let update = coalesce_filesystem_limit_update(
            4096,
            downgrade_sender,
            &mut receiver,
            &mut deferred_wakeups,
        );

        assert_eq!(update.allocated_bytes, 1024 * 1024);
        assert_eq!(update.senders.len(), 2);
        for sender in update.senders {
            let _ = sender.send(Ok(()));
        }
        assert!(downgrade_result.await.unwrap().is_ok());
        assert!(upgrade_result.await.unwrap().is_ok());
        assert!(receiver.try_recv().is_err());
        assert_eq!(deferred_wakeups.len(), 1);
        assert!(matches!(
            deferred_wakeups.pop_front(),
            Some(WorkerCommand::WorkAvailable)
        ));
    }

    #[test]
    async fn idle_over_limit_update_publishes_suspend_only_after_cleanup() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let cleanup_events = Arc::clone(&events);
        let suspend_events = Arc::clone(&events);
        let (release_cleanup, cleanup_released) = tokio::sync::oneshot::channel();

        let completion = tokio::spawn(finish_filesystem_limit_unload(
            true,
            async move {
                let _ = cleanup_released.await;
                cleanup_events.lock().unwrap().push("cleanup");
                None
            },
            move || async move {
                suspend_events.lock().unwrap().push("suspend");
            },
        ));
        tokio::task::yield_now().await;
        assert!(events.lock().unwrap().is_empty());

        release_cleanup.send(()).unwrap();
        assert!(completion.await.unwrap().is_none());
        assert_eq!(*events.lock().unwrap(), ["cleanup", "suspend"]);
    }

    fn complete_stopping(final_state: FinalWorkerState) -> WorkerInstance {
        let (instance, notify) = complete_stopping_worker(
            StoppingWorker {
                notify: golem_common::one_shot::OneShotEvent::new(),
                final_state: FinalWorkerState::Unloaded {
                    startup_failure: None,
                },
                pending_live_invocations: PendingLiveInvocationDisposition::Preserve,
            },
            final_state,
        );
        notify.set();
        instance
    }

    #[test]
    async fn dropped_unload_observer_does_not_cancel_module_owned_cleanup() {
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = tokio::sync::oneshot::channel();
        let completed = Arc::new(AtomicBool::new(false));
        let completed_in_task = Arc::clone(&completed);

        let observer = spawn_module_owned_unload(async move {
            let _ = started_tx.send(());
            let _ = release_rx.await;
            completed_in_task.store(true, Ordering::Release);
            None
        });
        started_rx.await.unwrap();
        drop(observer);
        release_tx.send(()).unwrap();

        tokio::time::timeout(Duration::from_secs(1), async {
            while !completed.load(Ordering::Acquire) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    }

    #[test]
    async fn unload_task_panic_is_reported_by_the_observer() {
        let error = spawn_module_owned_unload(async move {
            panic!("injected unload panic");
        })
        .await
        .expect("panic must become a cleanup error");

        assert!(error.to_string().contains("unload task panicked"));
    }

    #[test]
    async fn unload_observer_completes_only_after_module_owned_cleanup() {
        let (release_tx, release_rx) = tokio::sync::oneshot::channel();
        let observer = spawn_module_owned_unload(async move {
            let _ = release_rx.await;
            None
        });
        tokio::pin!(observer);

        assert!(
            tokio::time::timeout(Duration::from_millis(20), &mut observer)
                .await
                .is_err()
        );
        release_tx.send(()).unwrap();
        assert!(observer.await.is_none());
    }

    #[test]
    async fn resource_window_and_permit_close_before_native_deletion() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let close_events = Arc::clone(&events);
        let release_events = Arc::clone(&events);
        let delete_events = Arc::clone(&events);

        let result = close_usage_before_delete(
            async move {
                close_events.lock().unwrap().push("window-closed");
                None::<()>
            },
            move || release_events.lock().unwrap().push("permit-released"),
            move || async move {
                delete_events.lock().unwrap().push("filesystem-deleted");
                None::<()>
            },
        )
        .await;

        assert_eq!(result, (None, None));
        assert_eq!(
            *events.lock().unwrap(),
            ["window-closed", "permit-released", "filesystem-deleted"]
        );
    }

    #[test]
    async fn invocation_loop_panic_is_returned_to_owned_cleanup_boundary() {
        let error = catch_invocation_loop_panic(async {
            panic!("injected invocation-loop panic");
        })
        .await
        .expect_err("panic must become a lifecycle error");

        assert!(
            error
                .to_string()
                .contains("invocation loop panicked: injected invocation-loop panic")
        );
    }

    #[test]
    async fn invocation_loop_task_handles_panic_before_resident_creation() {
        let handled = Arc::new(Mutex::new(None));
        let handled_by_task = Arc::clone(&handled);

        run_invocation_loop_task(
            async { panic!("injected startup panic") },
            move |error| async move {
                *handled_by_task.lock().unwrap() = Some(error);
            },
        )
        .await;

        assert!(
            handled
                .lock()
                .unwrap()
                .as_ref()
                .is_some_and(|error| error.to_string().contains("injected startup panic"))
        );
    }

    #[test]
    #[timeout("5s")]
    async fn invocation_loop_panic_with_resident_owner_uses_filesystem_drop_cleanup() {
        let (filesystem, control) = resident_for_unload_test().await;
        control.push_delete_and_verify(Ok(()));
        let deletion = control.block("delete_and_verify");
        let handled = Arc::new(AtomicBool::new(false));
        let handled_by_task = Arc::clone(&handled);

        run_invocation_loop_task(
            async move {
                let _filesystem = filesystem;
                panic!("injected resident panic");
            },
            move |_error| async move {
                handled_by_task.store(true, Ordering::Release);
            },
        )
        .await;

        deletion.wait_started().await;
        assert!(handled.load(Ordering::Acquire));
        deletion.release();
    }

    #[test]
    #[timeout("5s")]
    async fn concrete_unload_running_agent_publishes_only_after_verified_deletion() {
        let (filesystem, control, window, generation_handle, node) =
            metered_resident_with_open_node_for_unload_test().await;
        control.push_close(Ok(()));
        let close = control.block("close");
        control.push_delete_and_verify(Ok(()));
        let deletion = control.block("delete_and_verify");
        let held = Arc::new(AtomicBool::new(false));
        let mut permit_state = ConcurrentAgentPermitState::new(None, Arc::clone(&held));
        permit_state.install_window(window);
        let activity = Arc::new(Mutex::new(Some(filesystem_activity(&filesystem))));
        let store_dropped = Arc::new(AtomicBool::new(false));

        let observer = InvocationLoop::<Context>::unload_running_agent(
            RunningAgent {
                runtime: TestStoreOwner {
                    node: Some(node),
                    dropped: Arc::clone(&store_dropped),
                },
                filesystem,
            },
            UnloadReason::ExplicitStop,
            Instant::now() + Duration::from_secs(1),
            &mut permit_state,
            &activity,
        );
        let publication = tokio::spawn(async move {
            let cleanup_error = observer.await;
            publish_unload_outcome(
                cleanup_error,
                None,
                |error| async move { complete_stopping(FinalWorkerState::CleanupFailed(error)) },
                |startup_failure| async move {
                    complete_stopping(FinalWorkerState::Unloaded { startup_failure })
                },
            )
            .await
        });

        let late_target = PathTarget::at_root(&generation_handle, "late-dispatch").unwrap();
        assert!(matches!(
            attributes(&generation_handle, Target::Path(&late_target, Follow::Yes)),
            Err(AccessError::Revoked)
        ));
        assert!(activity.lock().unwrap().is_none());
        close.wait_started().await;
        assert!(store_dropped.load(Ordering::Acquire));
        assert!(held.load(Ordering::Acquire));
        close.release();

        deletion.wait_started().await;
        assert!(!held.load(Ordering::Acquire));
        tokio::task::yield_now().await;
        assert!(!publication.is_finished());
        deletion.release();
        let instance = publication.await.unwrap();
        assert!(matches!(instance, WorkerInstance::Unloaded { .. }));
    }

    #[test]
    #[timeout("5s")]
    async fn concrete_unload_deletion_failure_publishes_cleanup_failed() {
        let (filesystem, control, window, _generation_handle, node) =
            metered_resident_with_open_node_for_unload_test().await;
        control.push_close(Ok(()));
        control.push_delete_and_verify(Err(FilesystemStorageError::verification(
            "injected verified deletion failure",
            Path::new("<unload-test>"),
        )));
        let held = Arc::new(AtomicBool::new(false));
        let mut permit_state = ConcurrentAgentPermitState::new(None, Arc::clone(&held));
        permit_state.install_window(window);
        let activity = Arc::new(Mutex::new(Some(filesystem_activity(&filesystem))));

        let cleanup_error = InvocationLoop::<Context>::unload_running_agent(
            RunningAgent {
                runtime: TestStoreOwner {
                    node: Some(node),
                    dropped: Arc::new(AtomicBool::new(false)),
                },
                filesystem,
            },
            UnloadReason::ExplicitStop,
            Instant::now() + Duration::from_secs(1),
            &mut permit_state,
            &activity,
        )
        .await
        .expect("verified deletion failure must fail unload");
        assert!(!held.load(Ordering::Acquire));

        let instance = publish_unload_outcome(
            Some(cleanup_error),
            None,
            |error| async move { complete_stopping(FinalWorkerState::CleanupFailed(error)) },
            |startup_failure| async move {
                complete_stopping(FinalWorkerState::Unloaded { startup_failure })
            },
        )
        .await;
        assert!(matches!(instance, WorkerInstance::CleanupFailed(_)));
    }

    #[test]
    #[timeout("5s")]
    async fn dropped_production_unload_observer_does_not_cancel_owned_cleanup() {
        let (filesystem, control, window, _generation_handle, node) =
            metered_resident_with_open_node_for_unload_test().await;
        control.push_close(Ok(()));
        let close = control.block("close");
        control.push_delete_and_verify(Ok(()));
        let deletion = control.block("delete_and_verify");
        let held = Arc::new(AtomicBool::new(false));
        let mut permit_state = ConcurrentAgentPermitState::new(None, Arc::clone(&held));
        permit_state.install_window(window);
        let activity = Arc::new(Mutex::new(Some(filesystem_activity(&filesystem))));

        let observer = unload_resident_agent_ownership::<_, ScriptedSandboxFilesystem>(
            ResidentAgentOwnership {
                runtime: TestStoreOwner {
                    node: Some(node),
                    dropped: Arc::new(AtomicBool::new(false)),
                },
                filesystem,
            },
            UnloadReason::ExplicitStop,
            Instant::now() + Duration::from_secs(1),
            &mut permit_state,
            &activity,
        );
        drop(observer);

        close.wait_started().await;
        close.release();
        deletion.wait_started().await;
        assert!(!held.load(Ordering::Acquire));
        deletion.release();
    }

    #[test]
    #[timeout("5s")]
    async fn production_unload_starts_final_observation_after_native_close_drains() {
        let (filesystem, control, window, _generation_handle, node) =
            billing_metered_resident_with_open_node_for_unload_test().await;
        control.push_close(Ok(()));
        let close = control.block("close");
        control.push_observe_allocation(Ok(crate::sandbox_filesystem::FilesystemAllocation {
            allocated_bytes: 100,
            filesystem_objects: 1,
        }));
        let final_observation = control.block("observe_allocation");
        control.push_delete_and_verify(Ok(()));
        let held = Arc::new(AtomicBool::new(false));
        let mut permit_state = ConcurrentAgentPermitState::new(None, Arc::clone(&held));
        permit_state.install_window(window);
        let activity = Arc::new(Mutex::new(Some(filesystem_activity(&filesystem))));

        let observer = unload_resident_agent_ownership::<_, ScriptedSandboxFilesystem>(
            ResidentAgentOwnership {
                runtime: TestStoreOwner {
                    node: Some(node),
                    dropped: Arc::new(AtomicBool::new(false)),
                },
                filesystem,
            },
            UnloadReason::ExplicitStop,
            Instant::now() + Duration::from_secs(1),
            &mut permit_state,
            &activity,
        );

        close.wait_started().await;
        assert!(
            tokio::time::timeout(Duration::from_millis(20), final_observation.wait_started())
                .await
                .is_err()
        );
        close.release();
        final_observation.wait_started().await;
        final_observation.release();

        assert!(observer.await.is_none());
        assert!(!held.load(Ordering::Acquire));
    }

    #[test]
    #[timeout("5s")]
    async fn concrete_unload_deadline_publishes_cleanup_failed_and_cleanup_continues() {
        let (filesystem, control, window, _generation_handle, node) =
            metered_resident_with_open_node_for_unload_test().await;
        control.push_close(Ok(()));
        let close = control.block("close");
        control.push_delete_and_verify(Ok(()));
        let deletion = control.block("delete_and_verify");
        let held = Arc::new(AtomicBool::new(false));
        let mut permit_state = ConcurrentAgentPermitState::new(None, Arc::clone(&held));
        permit_state.install_window(window);
        let activity = Arc::new(Mutex::new(Some(filesystem_activity(&filesystem))));

        let observer = InvocationLoop::<Context>::unload_running_agent(
            RunningAgent {
                runtime: TestStoreOwner {
                    node: Some(node),
                    dropped: Arc::new(AtomicBool::new(false)),
                },
                filesystem,
            },
            UnloadReason::ExplicitStop,
            Instant::now() + Duration::from_millis(20),
            &mut permit_state,
            &activity,
        );

        close.wait_started().await;
        let cleanup_error = tokio::time::timeout(Duration::from_millis(200), observer)
            .await
            .expect("deadline must resolve the lifecycle observer")
            .expect("deadline must report cleanup failure");
        assert!(cleanup_error.to_string().contains("unload deadline"));
        assert!(!held.load(Ordering::Acquire));

        let instance = publish_unload_outcome(
            Some(cleanup_error),
            None,
            |error| async move { complete_stopping(FinalWorkerState::CleanupFailed(error)) },
            |startup_failure| async move {
                complete_stopping(FinalWorkerState::Unloaded { startup_failure })
            },
        )
        .await;
        assert!(matches!(instance, WorkerInstance::CleanupFailed(_)));
        assert!(!matches!(instance, WorkerInstance::Stopping(_)));
        assert!(
            !control
                .calls()
                .iter()
                .any(|call| call.starts_with("delete_and_verify("))
        );

        close.release();
        deletion.wait_started().await;
        deletion.release();
        deletion.wait_completed().await;
    }

    #[test]
    #[timeout("5s")]
    async fn filesystem_pressure_stop_seam_reaches_concrete_unload_with_exact_deadline() {
        let (filesystem, control, window, _generation_handle, node) =
            metered_resident_with_open_node_for_unload_test().await;
        control.push_close(Ok(()));
        let close = control.block("close");
        control.push_delete_and_verify(Ok(()));
        let deletion = control.block("delete_and_verify");
        let held = Arc::new(AtomicBool::new(false));
        let mut permit_state = ConcurrentAgentPermitState::new(None, Arc::clone(&held));
        permit_state.install_window(window);
        let activity = Arc::new(Mutex::new(Some(filesystem_activity(&filesystem))));
        let deadline = Instant::now() + Duration::from_secs(1);
        let eligibility = FilesystemPressureEligibility {
            idle_since: 11,
            last_effect_completion: 7,
        };

        let pressure_stop = tokio::spawn(stop_loaded_idle_if_eligible(
            eligibility,
            crate::worker::UnloadRequest::new(UnloadReason::FilesystemPressure, deadline),
            move |target_class,
                  expected_eligibility,
                  unload_request: crate::worker::UnloadRequest| async move {
                assert_eq!(target_class, EvictionClass::LoadedIdle);
                assert_eq!(expected_eligibility, Some(eligibility));
                assert_eq!(unload_request.reason, UnloadReason::FilesystemPressure);
                assert_eq!(unload_request.deadline, deadline);
                InvocationLoop::<Context>::unload_running_agent(
                    RunningAgent {
                        runtime: TestStoreOwner {
                            node: Some(node),
                            dropped: Arc::new(AtomicBool::new(false)),
                        },
                        filesystem,
                    },
                    unload_request.reason,
                    unload_request.deadline,
                    &mut permit_state,
                    &activity,
                )
                .await
            },
        ));

        close.wait_started().await;
        assert!(held.load(Ordering::Acquire));
        close.release();
        deletion.wait_started().await;
        assert!(!held.load(Ordering::Acquire));
        deletion.release();
        assert!(pressure_stop.await.unwrap().is_none());
    }

    #[test]
    fn permit_state_stays_conservative_through_release_and_reacquisition() {
        let held = Arc::new(AtomicBool::new(false));
        let drops = Arc::new(AtomicUsize::new(0));
        let held_while_dropping = Arc::new(AtomicBool::new(false));
        let mut state = ConcurrentAgentPermitState::new(None, held.clone());

        state.install(TestPermit {
            held: held.clone(),
            drops: drops.clone(),
            held_while_dropping: held_while_dropping.clone(),
        });
        assert!(held.load(Ordering::Acquire));
        assert!(state.is_some());

        state.release();
        assert!(state.is_none());
        assert!(!held.load(Ordering::Acquire));
        assert!(held_while_dropping.load(Ordering::Acquire));
        assert_eq!(drops.load(Ordering::Acquire), 1);

        state.install(TestPermit {
            held: held.clone(),
            drops: drops.clone(),
            held_while_dropping,
        });
        assert!(held.load(Ordering::Acquire));
        state.release();
        assert_eq!(drops.load(Ordering::Acquire), 2);
    }

    #[test]
    fn periodic_snapshot_uses_creation_time_until_the_first_snapshot() {
        let created_at = Timestamp::from(1_000);

        let baseline = snapshot_baseline_timestamp(None, created_at);

        assert_eq!(baseline, created_at);
    }

    #[test]
    fn periodic_snapshot_is_due_once_the_period_elapsed() {
        let last_snapshot = Timestamp::from(1_000);
        let now = Timestamp::from(6_000);

        let action = snapshot_action_at(last_snapshot, Duration::from_secs(5), now);

        assert_eq!(action, PeriodicSnapshotAction::DueNow);
    }

    #[test]
    fn periodic_snapshot_waits_for_the_remaining_idle_time() {
        let last_snapshot = Timestamp::from(1_000);
        let now = Timestamp::from(4_250);

        let action = snapshot_action_at(last_snapshot, Duration::from_secs(5), now);

        assert_eq!(
            action,
            PeriodicSnapshotAction::Wait(Duration::from_millis(1_750))
        );
    }

    #[test]
    fn periodic_snapshot_failed_invocation_triggers_immediate_recovery() {
        let result = Ok(InvokeResult::Failed {
            consumed_fuel: 0,
            error: AgentError::InternalError("boom".to_string()),
            retry_from: OplogIndex::INITIAL,
            in_atomic_region: false,
            atomic_region_had_side_effects: false,
            semantic_trap_retry_override: None,
        });

        assert_eq!(
            periodic_snapshot_failure_outcome(&result),
            Some(CommandOutcome::BreakInnerLoop(RetryDecision::Immediate))
        );
    }

    #[test]
    fn periodic_snapshot_invocation_error_triggers_immediate_recovery() {
        let result = Err(WorkerExecutorError::runtime("boom"));

        assert_eq!(
            periodic_snapshot_failure_outcome(&result),
            Some(CommandOutcome::BreakInnerLoop(RetryDecision::Immediate))
        );
    }

    #[test]
    fn ephemeral_non_initialization_invocation_requests_archive_drain() {
        assert_eq!(
            successful_agent_invocation_outcome(
                AgentMode::Ephemeral,
                true,
                AgentInvocationKind::AgentMethod
            ),
            CommandOutcome::BreakInnerLoopAndArchiveEphemeralOplog(RetryDecision::None)
        );
    }

    #[test]
    fn ephemeral_agent_initialization_does_not_request_active_worker_cleanup() {
        assert_eq!(
            successful_agent_invocation_outcome(
                AgentMode::Ephemeral,
                true,
                AgentInvocationKind::AgentInitialization
            ),
            CommandOutcome::Continue
        );
    }

    #[test]
    fn durable_invocation_does_not_request_active_worker_cleanup() {
        assert_eq!(
            successful_agent_invocation_outcome(
                AgentMode::Durable,
                true,
                AgentInvocationKind::AgentMethod
            ),
            CommandOutcome::Continue
        );
    }

    #[test]
    fn terminal_ephemeral_failure_requests_archive_drain() {
        assert_eq!(
            failed_agent_invocation_outcome(AgentMode::Ephemeral, RetryDecision::None),
            CommandOutcome::BreakInnerLoopAndArchiveEphemeralOplog(RetryDecision::None)
        );
    }

    #[test]
    fn retryable_ephemeral_failure_does_not_request_archive_drain() {
        assert_eq!(
            failed_agent_invocation_outcome(AgentMode::Ephemeral, RetryDecision::Immediate),
            CommandOutcome::BreakInnerLoop(RetryDecision::Immediate)
        );
    }

    #[test]
    fn terminal_durable_failure_does_not_request_archive_drain() {
        assert_eq!(
            failed_agent_invocation_outcome(AgentMode::Durable, RetryDecision::None),
            CommandOutcome::BreakInnerLoop(RetryDecision::None)
        );
    }

    #[test]
    fn durable_live_streaming_invocation_always_reconstructs_the_store() {
        assert_eq!(
            successful_agent_invocation_outcome(
                AgentMode::Durable,
                true,
                AgentInvocationKind::AgentMethod
            ),
            CommandOutcome::Continue
        );
    }

    #[test]
    fn ephemeral_live_streaming_invocation_archives_its_ephemeral_oplog() {
        assert_eq!(
            successful_agent_invocation_outcome(
                AgentMode::Ephemeral,
                true,
                AgentInvocationKind::AgentMethod
            ),
            CommandOutcome::BreakInnerLoopAndArchiveEphemeralOplog(RetryDecision::None)
        );
    }
}
