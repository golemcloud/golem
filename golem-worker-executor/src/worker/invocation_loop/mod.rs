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

use crate::durable_host::tool::operation::{OwnerFailureWinner, OwnerToolOperations};
use crate::filesystem_snapshot::AgentSnapshots;
use crate::model::{LookupResult, TrapType};
use crate::sandbox_filesystem::{SandboxFilesystem, SandboxFilesystemAdapter};
use crate::services::agent_filesystem::{
    CaptureError, CaptureOutcome, DeleteFailure, InitialFilesCheck, LimitTransition,
    ResidentFilesystem, ResidentFilesystemActivity, SealedFilesystem, TreeMark, WholeCapture,
    capture, capture_whole, check_initial_files, drain_sealed_filesystem, filesystem_activity,
    seal, set_limits,
};
use crate::services::agent_filesystem_snapshots::Confirm;
use crate::services::golem_config::SnapshotPolicy;
use crate::services::oplog::plugin::ForwardingOplog;
use crate::services::oplog::{CommitLevel, EphemeralOplog, OplogError, OplogOps, downcast_oplog};
use crate::services::resource_limits::{
    AtomicResourceEntry, MonthlyResourceCapacityError, MonthlyResourceExhaustion,
};
use crate::services::{
    HasActiveAgents, HasAgentFilesystemSnapshots, HasConfig, HasExtraDeps, HasOplog,
    HasOplogService, HasShardService, HasWorker,
};
use crate::worker::filesystem_snapshots::{
    ConfirmedFilesystemSnapshot, PeriodicFailure, PeriodicResult, PeriodicSnapshotHost,
    SnapshotSlot, UpdateSnapshot, UpdateSnapshotHost, confirm_by, periodic_snapshot,
    update_snapshot,
};
use crate::worker::interrupt::Interrupts;
use crate::worker::invocation::{
    InvocationMode, InvokeResult, LoweredInvocation, invocation_uses_streams,
    invoke_observed_and_traced, lower_invocation, materialize_streaming_result,
};
use crate::worker::monthly_limits::ExecutionWindow;
use crate::worker::status_checkpointer;
use crate::worker::{
    CreateWorkerInstanceError, FinalWorkerState, HydratedInvocation, InvocationFailureDecision,
    PendingLiveInvocationDisposition, PendingWorkerInterrupt, QueuedWorkerInvocation,
    RetryDecision, RunningAgent, RunningAgentRuntime, RunningWorker, UnloadReason, UnloadRequest,
    Worker, WorkerCommand, WorkerDeletionHook, WorkerRunningAgent, WorkerTrace,
};
use crate::workerctx::{PublicWorkerIo, UpdateManagement, WorkerCtx};
use async_lock::Mutex;
use futures::FutureExt;
use futures::channel::oneshot::{self, Sender};
use futures::future::{BoxFuture, Shared};
use golem_common::model::agent::{AgentMode, OwnerKind, ParsedAgentId};
use golem_common::model::component::{CanonicalFilePath, ComponentRevision};
use golem_common::model::filesystem::{FileByteSelection, FileReadError};
use golem_common::model::oplog::{
    AgentError, EphemeralCannotSuspendError, EphemeralFuelExhaustedError, FilesystemSnapshotName,
    OplogEntry,
};
use golem_common::model::{
    AgentId, AgentInvocation, AgentInvocationKind, AgentInvocationOutput, AgentInvocationResult,
    IdempotencyKey, OwnedAgentId,
};
use golem_common::model::{
    AgentStatus, AgentStatusRecord, OplogIndex, PendingInvocationRef, Timestamp,
    invocation_context::{AttributeValue, InvocationContextStack, SpanId},
};
use golem_common::retries::get_delay;
use golem_service_base::error::worker_executor::{InterruptKind, WorkerExecutorError};
use golem_service_base::model::FileReadResponse;
use golem_service_base::model::GetFileSystemNodeResult;

use golem_common::model::agent::structural_format::format_structural_typed;
use golem_common::related_span;
use golem_common::tracing::TraceOrigin;
use std::collections::{HashSet, VecDeque};
use std::future::Future;
use std::ops::DerefMut;
use std::panic::AssertUnwindSafe;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};
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

mod commands;
mod inner;
mod invocation;
mod permits;
mod snapshots;
mod startup;
mod unload;

use commands::apply_delayed_retry_commands_after_unload;
use commands::coalesce_delayed_retry_prefix;
use commands::coalesce_filesystem_limit_update;
#[cfg(test)]
use inner::wait_for_resident_wakeup;
#[cfg(test)]
use invocation::failed_agent_invocation_outcome;
use invocation::infrastructure_failure_outcome;
use invocation::is_recovery_required_error;
use invocation::should_terminally_interrupt_ephemeral;
#[cfg(test)]
use invocation::successful_agent_invocation_outcome;
#[cfg(test)]
use permits::acquire_permit_or_interrupt;
use permits::mark_idle;
use permits::resource_usage_close_deadline;
#[cfg(test)]
use snapshots::measured_capture;
#[cfg(test)]
use snapshots::periodic_failure_outcome;
#[cfg(test)]
use snapshots::periodic_snapshot_failure_outcome;
#[cfg(test)]
use snapshots::snapshot_action_at;
#[cfg(test)]
use snapshots::snapshot_baseline_timestamp;
use snapshots::with_end;
#[cfg(test)]
use unload::catch_invocation_loop_panic;
pub(super) use unload::cleanup_startup_filesystem;
use unload::finish_filesystem_limit_unload;
#[cfg(test)]
use unload::publish_unload_outcome;
pub(super) use unload::run_invocation_loop_task;
#[cfg(test)]
use unload::spawn_module_owned_unload;
#[cfg(test)]
use unload::unload_resident_agent_ownership;
use unload::unload_sealed_agent_ownership;

/// Context of a running worker's invocation loop
pub struct InvocationLoop<Ctx: WorkerCtx> {
    pub receiver: UnboundedReceiver<WorkerCommand>,
    pub active: Arc<tokio::sync::RwLock<VecDeque<QueuedWorkerInvocation>>>,
    pub owned_agent_id: OwnedAgentId,
    pub parent: Arc<Worker<Ctx>>, // parent must not be dropped until the invocation_loop is running
    pub waiting_for_command: Arc<AtomicBool>,
    pub interrupts: Arc<Interrupts>,
    pub oom_retry_count: u32,
    /// Concurrent-agent permit owned by this invocation loop task. Released
    /// (set to `None`) when the agent goes idle, re-acquired when it wakes up.
    /// Only actively running agents hold a permit. Normal stops close the command
    /// channel and await cooperative loop exit; this field's drop is only a fallback
    /// for task cancellation or panic.
    pub(super) permit_state:
        ConcurrentAgentPermitState<crate::services::active_agents::ConcurrentAgentPermit>,
    pub(super) filesystem_activity: Arc<StdMutex<Option<ResidentFilesystemActivity>>>,
    /// The filesystem snapshots of the generation that runs.
    pub(super) filesystem_snapshot_slot: SnapshotSlot,
    /// When the last `SaveSnapshot` of this loop ended, whether it wrote a record or not. Every
    /// `SaveSnapshot` sets it, and only the `Periodic` policy reads it.
    pub(super) last_periodic_attempt: Option<Timestamp>,
    pub(super) unload_request: Arc<StdMutex<Option<UnloadRequest>>>,
    pub idle_since_millis: Arc<AtomicU64>,
    pub start_attempt: Uuid,
    /// What this worker's phase spans link back to, and the fields they carry.
    pub worker_trace: WorkerTrace,
}

impl<Ctx: WorkerCtx> Drop for InvocationLoop<Ctx> {
    fn drop(&mut self) {
        self.permit_state.release();
        self.permit_state.release_scope.seal();
    }
}

/// Outcome of creating the worker instance for one iteration of the invocation loop.
enum CreateInstanceResult<Ctx: WorkerCtx> {
    Created {
        agent: Box<WorkerRunningAgent<Ctx>>,
        window: ExecutionWindow,
        recovery_decision: Option<RetryDecision>,
    },
    /// Instance creation was interrupted by a recoverable condition, such as fuel or filesystem
    /// quota exhaustion. The worker metadata remains valid and queued work must be preserved.
    Interrupted(InterruptKind),
    /// Reconstruction reached a live continuation whose recovery data was temporarily
    /// unavailable. The incomplete runtime and filesystem have already been discarded.
    RecoveryRequired(WorkerExecutorError),
    /// The update head changed before a runtime was created; memory admission must run again.
    ReacquireMemory,
    /// Instance creation failed; the worker was already stopped with the startup failure.
    Failed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum MonthlyResourceAdmission {
    Admit,
    Unavailable,
    Suspend,
    FailInvocation(MonthlyResourceExhaustion),
}

fn monthly_resource_admission_for_capacity(
    capacity: Result<(), MonthlyResourceCapacityError>,
    agent_mode: AgentMode,
) -> MonthlyResourceAdmission {
    match (capacity, agent_mode) {
        (Ok(()), _) => MonthlyResourceAdmission::Admit,
        (Err(MonthlyResourceCapacityError::Unavailable), _) => {
            MonthlyResourceAdmission::Unavailable
        }
        (Err(MonthlyResourceCapacityError::Exhausted(_)), AgentMode::Durable) => {
            MonthlyResourceAdmission::Suspend
        }
        (Err(MonthlyResourceCapacityError::Exhausted(exhaustion)), AgentMode::Ephemeral) => {
            MonthlyResourceAdmission::FailInvocation(exhaustion)
        }
    }
}

pub(crate) fn monthly_resource_admission(
    resource_entry: &AtomicResourceEntry,
    agent_mode: AgentMode,
) -> MonthlyResourceAdmission {
    monthly_resource_admission_for_capacity(
        resource_entry.monthly_resource_capacity(agent_mode),
        agent_mode,
    )
}

pub(super) fn monthly_resource_exhausted_invocation_error(
    config: &crate::services::golem_config::GolemConfig,
    exhaustion: MonthlyResourceExhaustion,
) -> WorkerExecutorError {
    WorkerExecutorError::InvocationFailed {
        error: match exhaustion {
            MonthlyResourceExhaustion::Compute => {
                AgentError::EphemeralFuelExhausted(EphemeralFuelExhaustedError {
                    overdraft_limit: config
                        .limits
                        .fuel_to_borrow
                        .saturating_mul(config.limits.ephemeral_fuel_overdraft_multiplier),
                })
            }
            exhaustion => AgentError::EphemeralCannotSuspend(EphemeralCannotSuspendError {
                reason: exhaustion.reason().to_string(),
            }),
        },
        stderr: String::new(),
    }
}

struct ResidentAgentOwnership<Runtime, Adapter: SandboxFilesystemAdapter = SandboxFilesystem> {
    runtime: Runtime,
    filesystem: ResidentFilesystem<Adapter>,
    operations: Option<Arc<OwnerToolOperations>>,
}

struct SealedAgentOwnership<Runtime, Adapter: SandboxFilesystemAdapter = SandboxFilesystem> {
    runtime: Runtime,
    filesystem: SealedFilesystem<Adapter>,
    operations: Option<Arc<OwnerToolOperations>>,
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

#[derive(Default)]
struct DelayedRetryCommands {
    filesystem_limit_updates: Vec<Sender<Result<(), WorkerExecutorError>>>,
}

enum ResidentWakeup {
    Command(WorkerCommand),
    FilesystemTerminalFailure,
    CommandChannelClosed,
}

/// Takes the exact request, then waits for its independently owned establishment.
async fn take_pending_interrupt(interrupts: &Interrupts) -> Option<PendingWorkerInterrupt> {
    let interrupt = interrupts.take().await;
    if let Some(interrupt) = &interrupt {
        interrupt.establishment.wait().await;
    }
    interrupt
}

enum PermitAcquisition<T> {
    Acquired(T),
    Interrupted(PendingWorkerInterrupt),
    CommandChannelClosed,
}

impl<Ctx: WorkerCtx> InvocationLoop<Ctx> {
    async fn pending_interrupt(&self) -> Option<PendingWorkerInterrupt> {
        take_pending_interrupt(&self.interrupts).await
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

        let mut retry_was_live = false;
        'outer: loop {
            self.parent
                .queue
                .write()
                .await
                .retain(|invocation| !invocation.is_abandoned());
            self.release_terminal_interrupt().await;
            // ADMISSION: gates the start of a generation, so
            // fencing refuses new generations and never interrupts a running one.
            if let Err(error) = self.parent.shard_service().check_admission(&agent_id) {
                debug!(%agent_id, "Worker generation not started because its shard is not assigned");
                self.parent.complete_startup(self.start_attempt, Err(error));
                self.release_concurrent_agent_permit();
                self.stop_unloaded(None, PendingLiveInvocationDisposition::Fail)
                    .await;
                break;
            }
            let retiring = self.parent.owner_retirement_requested.clone();
            if retiring.is_cancelled() {
                self.parent.complete_startup(
                    self.start_attempt,
                    Err(WorkerExecutorError::runtime("Worker owner is retiring")),
                );
                self.release_concurrent_agent_permit();
                self.stop_unloaded(None, PendingLiveInvocationDisposition::Fail)
                    .await;
                break;
            }
            let pending_interrupt = self.pending_interrupt().await;
            if let Some(interrupt) = pending_interrupt {
                if self
                    .handle_unloaded_interrupt(
                        interrupt.clone(),
                        retry_was_live,
                        interrupt.retry_decision(),
                    )
                    .await
                {
                    break;
                }
                continue;
            }
            if self.permit_state.is_none() {
                if let Err(error) = self.parent.join_interrupt_establishment().await {
                    self.parent
                        .complete_startup(self.start_attempt, Err(error.clone()));
                    self.stop_cleanup_failed(error).await;
                    break;
                }
                let parent = self.parent.clone();
                let permit_agent_id = self.owned_agent_id.agent_id().clone();
                let permit = parent
                    .registered_concurrent_account
                    .acquire(permit_agent_id)
                    .instrument(agent_phase_span!(self, "acquire_concurrent_agent_permit"));
                tokio::pin!(permit);
                loop {
                    let pending_interrupt = self.pending_interrupt().await;
                    if let Some(interrupt) = pending_interrupt
                        && self
                            .handle_unloaded_interrupt(
                                interrupt.clone(),
                                retry_was_live,
                                interrupt.retry_decision(),
                            )
                            .await
                    {
                        break 'outer;
                    }
                    tokio::select! {
                        biased;
                        () = retiring.cancelled() => {
                            self.parent.complete_startup(self.start_attempt, Err(WorkerExecutorError::runtime("Worker owner is retiring")));
                            self.stop_unloaded(None, PendingLiveInvocationDisposition::Fail).await;
                            break 'outer;
                        }
                        permit = &mut permit => {
                            self.permit_state.install_tracked(permit);
                            continue 'outer;
                        }
                        command = self.receiver.recv() => {
                            let Some(command) = command else {
                                debug!(%agent_id, "Invocation queue loop command channel closed while awaiting concurrent-agent permit");
                                self.stop_closed(
                                    None,
                                    None,
                                    PendingLiveInvocationDisposition::Fail,
                                )
                                .await;
                                break 'outer;
                            };
                            let pending_interrupt = self.pending_interrupt().await;
                            if let Some(interrupt) = pending_interrupt
                                && self
                                    .handle_unloaded_interrupt(interrupt.clone(), retry_was_live, interrupt.retry_decision())
                                    .await
                            {
                                break 'outer;
                            }
                            match command {
                                WorkerCommand::InternalStatusChanged => {}
                                WorkerCommand::WorkAvailable => {
                                    Self::defer_wakeup(&mut deferred_wakeups, command);
                                }
                                WorkerCommand::UpdateFilesystemLimit { sender, .. } => {
                                    let _ = sender.send(Ok(()));
                                }
                            }
                        }
                    }
                }
            }
            // An interrupt that waits now, such as the terminal interrupt that ended the wait of
            // the start for an upload, is handled as an interrupt of the instantiation.
            if let Some(interrupt) = self.pending_interrupt().await {
                match self
                    .interrupted_before_instance(interrupt.kind, Some(interrupt))
                    .await
                {
                    StartupStep::Retry => continue,
                    StartupStep::Stop => break,
                }
            }
            let permit = self
                .permit_state
                .take_permit()
                .expect("startup must hold a concurrent-agent permit");
            let (mut agent, window, recovery_decision) = match self.create_instance(permit).await {
                CreateInstanceResult::Created {
                    agent,
                    window,
                    recovery_decision,
                } => (*agent, window, recovery_decision),
                CreateInstanceResult::ReacquireMemory => {
                    if let Some(interrupt) = self.pending_interrupt().await {
                        match self
                            .interrupted_before_instance(interrupt.kind, Some(interrupt))
                            .await
                        {
                            StartupStep::Stop => break,
                            StartupStep::Retry => {}
                        }
                    }
                    let pending_startup_attempt = self.parent.pending_startup_attempt();
                    if let Err(error) = Worker::restart_with_memory_admission(
                        self.parent.clone(),
                        true,
                        None,
                        self.oom_retry_count,
                        pending_startup_attempt,
                        UnloadReason::Restart,
                    )
                    .await
                    {
                        warn!("Failed to restart worker after changing update admission: {error}");
                    }
                    break;
                }
                CreateInstanceResult::Interrupted(kind) => {
                    self.release_concurrent_agent_permit();
                    let pending_interrupt = self.pending_interrupt().await;
                    match self
                        .interrupted_before_instance(kind, pending_interrupt)
                        .await
                    {
                        StartupStep::Retry => continue,
                        StartupStep::Stop => break,
                    }
                }
                CreateInstanceResult::RecoveryRequired(_error) => {
                    self.release_concurrent_agent_permit();
                    if self.parent.retired_for_lost_shard() {
                        self.stop_startup_retired().await;
                        break;
                    }
                    let pending_interrupt = self.pending_interrupt().await;
                    if let Some(interrupt) = pending_interrupt {
                        if self
                            .handle_unloaded_interrupt(
                                interrupt.clone(),
                                retry_was_live,
                                interrupt.retry_decision(),
                            )
                            .await
                        {
                            break;
                        }
                        continue;
                    }
                    let delay = self.parent.next_infrastructure_recovery_delay();
                    let sleep = tokio::time::sleep(delay);
                    tokio::pin!(sleep);
                    loop {
                        tokio::select! {
                            _ = &mut sleep => continue 'outer,
                            command = self.receiver.recv() => {
                                let Some(command) = command else {
                                    self.stop_closed(None, None, PendingLiveInvocationDisposition::Fail).await;
                                    break 'outer;
                                };
                                let pending_interrupt = self.pending_interrupt().await;
                                if let Some(interrupt) = pending_interrupt {
                                    if self.handle_unloaded_interrupt(interrupt.clone(), retry_was_live, interrupt.retry_decision()).await {
                                        break 'outer;
                                    }
                                    Self::defer_wakeup(&mut deferred_wakeups, command);
                                    continue 'outer;
                                }
                                match command {
                                    WorkerCommand::WorkAvailable => {
                                        Self::defer_wakeup(&mut deferred_wakeups, command);
                                    }
                                    WorkerCommand::InternalStatusChanged => {}
                                    WorkerCommand::UpdateFilesystemLimit { sender, .. } => {
                                        let _ = sender.send(Ok(()));
                                    }
                                }
                            }
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
            let mut final_decision = recovery_decision;
            let mut recovery_failure = None;
            let mut cleanup_failure = None;
            let mut final_interrupt = None;
            let mut final_unload_request = None;
            let mut unloaded_interrupt = None;
            let mut cleanup_ephemeral_worker = false;

            if let Some(interrupt) = self.pending_interrupt().await {
                let kind = interrupt.kind;
                let decision = interrupt.retry_decision();
                debug!(
                    %agent_id,
                    ?decision,
                    "Invocation queue loop interrupted after recovery"
                );
                if !matches!(kind, InterruptKind::Restart) {
                    final_interrupt = Some(kind);
                }
                final_unload_request = Some(interrupt.unload_request);
                final_decision = Some(decision);
            }

            if final_decision.is_none() {
                'resident: loop {
                    if !self.active.read().await.is_empty() {
                        Self::defer_wakeup(&mut deferred_wakeups, WorkerCommand::WorkAvailable);
                    }
                    let mut inner_loop = InnerInvocationLoop {
                        start_attempt: self.start_attempt,
                        receiver: &mut self.receiver,
                        active: self.active.clone(),
                        owned_agent_id: self.owned_agent_id.clone(),
                        parent: self.parent.clone(),
                        waiting_for_command: self.waiting_for_command.clone(),
                        interrupts: Arc::clone(&self.interrupts),
                        instance: &agent.runtime.instance,
                        store: &agent.runtime.store,
                        filesystem: &agent.filesystem,
                        filesystem_snapshot_slot: &self.filesystem_snapshot_slot,
                        last_periodic_attempt: &mut self.last_periodic_attempt,
                        invocations_since_snapshot: 0,
                        idle_snapshot_task: None,
                        filesystem_turn_used: false,
                        permit_state: &mut self.permit_state,
                        idle_since_millis: self.idle_since_millis.clone(),
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
                                store.mark_primary_execution_stopped();
                                store
                                    .lock()
                                    .await
                                    .data()
                                    .durable_ctx()
                                    .begin_stream_runtime_teardown();
                                if let Some(active_agent) =
                                    self.parent.active_agent_for_this_owner().await
                                {
                                    let owner_failure = failure.clone().map_or_else(
                                        || {
                                            OwnerFailureWinner::Lifecycle(InterruptKind::Interrupt(
                                                Timestamp::now_utc(),
                                            ))
                                        },
                                        OwnerFailureWinner::Infrastructure,
                                    );
                                    active_agent.fence_entity_bodies(owner_failure).await;
                                }
                                // Cleanup runs independently of this wait. Retain its final result
                                // so deletion can still join it if the unload deadline expires.
                                self.end_snapshot_generation();
                                let unloading = unload_sealed_agent_ownership(
                                    SealedAgentOwnership {
                                        runtime: RunningAgentRuntime { instance, store },
                                        filesystem,
                                        operations: Some(
                                            self.parent.owner_execution().tool_operations(),
                                        ),
                                    },
                                    UnloadReason::FilesystemLimit,
                                    resource_usage_close_deadline(),
                                    &mut self.permit_state,
                                    &self.filesystem_activity,
                                    None,
                                );
                                *self.parent.unload_cleanup.lock().unwrap() =
                                    Some(unloading.cleanup.clone());
                                let cleanup_failure =
                                    finish_filesystem_limit_unload(suspend, unloading, || async {
                                        // A refusal records the lost shard, which the stop
                                        // below acts on; there is nothing else to undo.
                                        let _ = self
                                            .parent
                                            .add_and_commit_oplog(OplogEntry::suspend())
                                            .await;
                                    })
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
                                    None => {
                                        self.stop_unloaded(
                                            failure,
                                            PendingLiveInvocationDisposition::Fail,
                                        )
                                        .await
                                    }
                                }
                                break 'outer;
                            }
                        }
                    }
                    final_decision = result.retry_decision;
                    final_interrupt = result.final_interrupt;
                    final_unload_request = result.unload_request;
                    unloaded_interrupt = result.unloaded_interrupt;
                    recovery_failure = result.recovery_failure;
                    cleanup_failure = result.cleanup_failure;
                    // The taken idle demand keeps its cause; later requests remain queued.
                    if unloaded_interrupt.is_none()
                        && let Some(interrupt) = self.pending_interrupt().await
                    {
                        let kind = interrupt.kind;
                        let decision = interrupt.retry_decision();
                        if !matches!(kind, InterruptKind::Restart) {
                            final_interrupt = Some(kind);
                        }
                        final_unload_request = Some(interrupt.unload_request);
                        final_decision = Some(decision);
                    }
                    cleanup_ephemeral_worker = result.cleanup_ephemeral_worker;
                    break 'resident;
                }
            }

            agent.runtime.store.mark_primary_execution_stopped();
            let delayed_retry_commands = matches!(final_decision, Some(RetryDecision::Delayed(_)))
                .then(|| {
                    let prefix_len = self.receiver.len();
                    coalesce_delayed_retry_prefix::<Ctx>(
                        &mut self.receiver,
                        &mut deferred_wakeups,
                        prefix_len,
                    )
                });

            if unloaded_interrupt.is_none()
                && let Some(interrupt) = self.pending_interrupt().await
            {
                let kind = interrupt.kind;
                let decision = interrupt.retry_decision();
                if !matches!(kind, InterruptKind::Restart) {
                    final_interrupt = Some(kind);
                }
                final_unload_request = Some(interrupt.unload_request);
                final_decision = Some(decision);
            }

            let terminal_ephemeral_restart = should_terminally_interrupt_ephemeral(
                self.parent.initial_worker_metadata.owner_kind,
                self.parent.agent_mode(),
                final_decision.as_ref(),
            );
            if terminal_ephemeral_restart {
                // Ephemeral execution cannot reconstruct after losing its Store. Traps normally
                // fail in on_invocation_failure; storage and protocol retries stop here instead.
                final_interrupt = Some(InterruptKind::Interrupt(Timestamp::now_utc()));
            }
            if terminal_ephemeral_restart
                || self.parent.initial_worker_metadata.owner_kind
                    == OwnerKind::EphemeralExternalTool
            {
                final_decision = Some(RetryDecision::None);
                cleanup_ephemeral_worker = true;
            }

            retry_was_live = {
                let store = agent.runtime.store.lock().await;
                store.data().durable_ctx().begin_stream_runtime_teardown();
                store.data().is_live()
            };
            self.suspend_worker(&agent.runtime.store).await;

            if let Some(kind) = final_interrupt
                && (unloaded_interrupt.is_none() || cleanup_ephemeral_worker)
                && !self.parent.state_actor.lifecycle_is_closed()
            {
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
            if (recovery_failure.is_none()
                || final_interrupt.is_some()
                || self.parent.retired_for_lost_shard())
                && let Some(active_agent) = self.parent.active_agent_for_this_owner().await
            {
                let terminal_interrupt = final_interrupt.or_else(|| {
                    if recovery_failure.is_none()
                        && let Some(RetryDecision::TryStop(timestamp)) = final_decision
                    {
                        Some(InterruptKind::Suspend(timestamp))
                    } else {
                        None
                    }
                });
                let owner_failure = self
                    .parent
                    .retired_for_lost_shard()
                    .then_some(InterruptKind::ShardLost)
                    .or(terminal_interrupt)
                    .map(OwnerFailureWinner::Lifecycle);
                match owner_failure {
                    Some(failure) => active_agent.fence_entity_bodies(failure).await,
                    None => active_agent.fence_entity_bodies_for_unload().await,
                }
            }
            // Tests can shorten the deadline and pause filesystem cleanup to exercise late
            // completion; without a hook, the normal unload timing is unchanged.
            let hook = Ctx::worker_deletion_hook(&self.parent.extra_deps());
            let deadline = hook.as_ref().map_or(unload_request.deadline, |hook| {
                hook.unload_deadline(&self.owned_agent_id, unload_request.deadline)
            });
            let unload_hook = hook.map(|hook| (self.owned_agent_id.clone(), hook));
            // An automatic update that restarts the agent in place selects its record when this
            // generation ends; the newest record counts only once its upload confirmed it. The
            // unload deadline moves by the time of the wait, so the wait does not use it up.
            let upload = matches!(final_decision, Some(RetryDecision::Immediate))
                .then(|| self.parent.upload_before_an_update())
                .flatten()
                .and_then(|name| Some((name, self.filesystem_snapshot_slot.generation()?)));
            let deadline = match upload {
                Some((name, mark)) => {
                    let waiting = Instant::now();
                    self.confirm_filesystem_snapshot_before_an_update(
                        name,
                        mark,
                        &mut deferred_wakeups,
                    )
                    .await;
                    deadline + waiting.elapsed()
                }
                None => deadline,
            };
            self.end_snapshot_generation();
            let unloading = Self::unload_running_agent(
                agent,
                Some(self.parent.owner_execution().tool_operations()),
                unload_request.reason,
                deadline,
                &mut self.permit_state,
                &self.filesystem_activity,
                unload_hook,
            );
            // Save final completion before awaiting the bounded result: a timeout below does
            // not stop cleanup, and a later delete must wait for or retry that same cleanup.
            *self.parent.unload_cleanup.lock().unwrap() = Some(match &cleanup_failure {
                Some(error) => unloading.cleanup.with_failure(error.clone()),
                None => unloading.cleanup.clone(),
            });
            let unload_failure = unloading.await;
            if let Some(error) = cleanup_failure.or(unload_failure) {
                self.stop_cleanup_failed(error).await;
                break;
            }
            if self.parent.owner_retirement_requested.is_cancelled()
                && !self.parent.retired_for_lost_shard()
                && let Some(error) = recovery_failure.take()
            {
                self.stop_cleanup_failed(error).await;
                break;
            }

            if let Some(commands) = delayed_retry_commands {
                apply_delayed_retry_commands_after_unload(commands);
            }

            if self.parent.monthly_policy_is_unavailable() {
                final_decision = Some(RetryDecision::None);
            }
            if let Some(interrupt) = unloaded_interrupt
                && !cleanup_ephemeral_worker
            {
                if self.parent.retired_for_lost_shard() {
                    self.stop_startup_retired().await;
                    break;
                }
                // Disposal succeeded. The unloaded path owns this demand's marker, not
                // the completed invocation whose Store has just been destroyed.
                if self
                    .handle_unloaded_interrupt(
                        interrupt,
                        retry_was_live,
                        final_decision.expect("taken idle demand has a retry decision"),
                    )
                    .await
                {
                    break;
                }
                continue;
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
                            cleanup_ephemeral_worker
                                .then(crate::worker::inactive_ephemeral_agent_error)
                        }),
                        PendingLiveInvocationDisposition::Fail,
                    )
                    .await;
                    if cleanup_ephemeral_worker {
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
                        self.stop_closed(
                            None,
                            None,
                            resident_work_disposition(unload_request.reason),
                        )
                        .await;
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
                                        self.stop_closed(None, None, PendingLiveInvocationDisposition::Fail).await;
                                        break 'outer;
                                    }
                                };

                                let pending_interrupt = self.pending_interrupt().await;
                                if let Some(interrupt) = pending_interrupt {
                                    let kind = interrupt.kind;
                                    let decision = interrupt.retry_decision();
                                    debug!(%agent_id, ?decision, "Invocation queue loop interrupted during delayed retry");
                                    if !matches!(kind, InterruptKind::Restart) {
                                        let current_idempotency_key = self
                                            .parent
                                            .get_last_known_status()
                                            .await
                                            .current_idempotency_key
                                            .clone();
                                        let recorded = match kind {
                                            InterruptKind::EntityCancellation => unreachable!("entity cancellation cannot interrupt the owner"),
                                            InterruptKind::Suspend(_) => {
                                                self.parent.add_and_commit_oplog(OplogEntry::suspend()).await.map(|_| ())
                                            }
                                            InterruptKind::Interrupt(_) => {
                                                self.parent.add_and_commit_oplog(OplogEntry::interrupted()).await.map(|_| ())
                                            }
                                            InterruptKind::Restart
                                            | InterruptKind::Jump
                                            | InterruptKind::ShardLost => Ok(()),
                                        };
                                        // Refused, the shard is lost: nothing restarts
                                        // in place.
                                        if recorded.is_err() {
                                            self.stop_startup_retired().await;
                                            break 'outer;
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
                                            self.stop_closed(None, None, PendingLiveInvocationDisposition::Fail).await;
                                            break 'outer;
                                        }
                                        RetryDecision::TryStop(timestamp) => {
                                            if timestamp < *self.parent.last_resume_request.lock().await {
                                                Self::defer_wakeup(&mut deferred_wakeups, command);
                                                continue 'outer;
                                            }
                                            self.stop_closed(None, None, resident_work_disposition(interrupt.unload_request.reason)).await;
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
                                        Self::defer_wakeup(&mut deferred_wakeups, WorkerCommand::WorkAvailable);
                                        if recovery_failure.is_none() {
                                            debug!(%agent_id, "Invocation queue loop woke up during delayed retry");
                                            continue 'outer;
                                        }
                                        debug!(%agent_id, "Invocation queue loop retained work during infrastructure recovery backoff");
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
                    if let Err(error) = Worker::restart_with_memory_admission(
                        self.parent.clone(),
                        true,
                        delay,
                        self.oom_retry_count + 1,
                        pending_startup_attempt,
                        UnloadReason::OutOfMemory,
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

    /// Ends the filesystem snapshots of the generation that runs. A confirmation of that
    /// generation that comes later writes nothing.
    fn end_snapshot_generation(&self) {
        self.filesystem_snapshot_slot.end();
    }

    async fn release_terminal_interrupt(&self) {
        self.interrupts.reset_terminal_for_new_generation().await;
    }

    fn release_concurrent_agent_permit(&mut self) {
        self.permit_state.release();
    }
}

struct InnerInvocationLoop<'a, Ctx: WorkerCtx> {
    start_attempt: Uuid,
    receiver: &'a mut UnboundedReceiver<WorkerCommand>,
    active: Arc<tokio::sync::RwLock<VecDeque<QueuedWorkerInvocation>>>,
    owned_agent_id: OwnedAgentId,
    parent: Arc<Worker<Ctx>>, // parent must not be dropped until the invocation_loop is running
    waiting_for_command: Arc<AtomicBool>,
    interrupts: Arc<Interrupts>,
    instance: &'a Instance,
    store: &'a Mutex<Store<Ctx>>,
    filesystem: &'a ResidentFilesystem,
    filesystem_snapshot_slot: &'a SnapshotSlot,
    /// When the last `SaveSnapshot` ended. Every `SaveSnapshot` sets it, and only the `Periodic`
    /// policy reads it: the next periodic snapshot waits a period from it, whether the last one
    /// wrote a record or not, and however long it took.
    last_periodic_attempt: &'a mut Option<Timestamp>,
    invocations_since_snapshot: u64,
    idle_snapshot_task: Option<JoinHandle<()>>,
    filesystem_turn_used: bool,
    /// Mutable reference to the concurrent-agent permit held by the outer
    /// `InvocationLoop`. Set to `None` when entering idle (releasing the
    /// permit back to the semaphore pool) and re-acquired on wake.
    permit_state:
        &'a mut ConcurrentAgentPermitState<crate::services::active_agents::ConcurrentAgentPermit>,
    idle_since_millis: Arc<AtomicU64>,
    deferred_wakeups: &'a mut VecDeque<WorkerCommand>,
    /// What this worker's phase spans link back to, and the fields they carry.
    worker_trace: WorkerTrace,
}

enum SelectedWork {
    Durable(PendingInvocationRef),
    Resident(QueuedWorkerInvocation),
    ApplyPendingUpdate,
    Idle(Arc<AgentStatusRecord>),
}

struct UnloadObserver {
    receiver: tokio::sync::oneshot::Receiver<Option<WorkerExecutorError>>,
    cleanup: UnloadCleanup,
}

/// Final module-owned cleanup, independent of the bounded lifecycle notification.
#[derive(Clone, Debug)]
pub(super) struct UnloadCleanup {
    completion: Shared<BoxFuture<'static, Result<(), UnloadCleanupFailure>>>,
    release_scope: Option<crate::metrics::resource_release::ReleaseScope>,
}

#[derive(Clone, Debug)]
pub(crate) enum UnloadCleanupFailure {
    Other(WorkerExecutorError),
    Filesystem {
        failure: DeleteFailure,
        close_error: Option<WorkerExecutorError>,
        release_receipt: Option<crate::metrics::resource_release::ReleaseReceipt>,
    },
}

#[derive(Clone)]
struct UnloadCompletion {
    sender: Arc<StdMutex<Option<tokio::sync::oneshot::Sender<Option<WorkerExecutorError>>>>>,
}

pub(super) struct ConcurrentAgentPermitState<T> {
    release_scope: crate::metrics::resource_release::ReleaseScope,
    permit: Option<T>,
    window: Option<ExecutionWindow>,
    held: Arc<AtomicBool>,
}

fn resident_work_disposition(reason: UnloadReason) -> PendingLiveInvocationDisposition {
    if reason == UnloadReason::Suspend {
        PendingLiveInvocationDisposition::Preserve
    } else {
        PendingLiveInvocationDisposition::Fail
    }
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
    filesystem: &'a ResidentFilesystem,
    filesystem_snapshot_slot: &'a SnapshotSlot,
    uses_streams: bool,
}

/// What the start does after an interrupt that came before its instance ran.
enum StartupStep {
    /// The start runs again.
    Retry,
    /// The start ends.
    Stop,
}

/// Outcome of processing a single command within the inner invocation loop
#[derive(Debug, PartialEq, Eq)]
enum CommandOutcome {
    /// Break from both the inner and outer loops, there is no way to retry anything
    BreakOuterLoop(Option<WorkerExecutorError>),
    /// Break from the inner loop, setting the retry decision for the outer loop
    BreakInnerLoop(RetryDecision),
    /// Discard the current runtime and reconstruct the accepted invocation after an
    /// infrastructure-owned recovery failure.
    BreakInnerLoopForRecovery {
        decision: RetryDecision,
        error: WorkerExecutorError,
    },
    /// Break from the inner loop and archive the stopped ephemeral worker's oplog. The error, when
    /// there is one, is the failure that stopped the agent, which its waiters get instead of the
    /// ordinary end of an ephemeral agent.
    BreakInnerLoopAndArchiveEphemeralOplog(RetryDecision, Option<WorkerExecutorError>),
    /// Continue processing in the inner loop
    Continue,
    /// Stop draining for now and wait for the next command or idle timer wakeup
    WaitForWakeup,
}

#[derive(Default)]
struct InnerInvocationLoopResult {
    retry_decision: Option<RetryDecision>,
    final_interrupt: Option<InterruptKind>,
    unload_request: Option<UnloadRequest>,
    // A taken no-body demand still owns its lifecycle marker after physical disposal.
    unloaded_interrupt: Option<PendingWorkerInterrupt>,
    cleanup_ephemeral_worker: bool,
    recovery_failure: Option<WorkerExecutorError>,
    cleanup_failure: Option<WorkerExecutorError>,
    filesystem_limit_update: Option<PendingFilesystemLimitUpdate>,
}

impl InnerInvocationLoopResult {
    fn with_unloaded_interrupt(mut self, interrupt: Option<PendingWorkerInterrupt>) -> Self {
        if let Some(interrupt) = &interrupt {
            self.retry_decision = Some(interrupt.retry_decision());
            self.final_interrupt =
                (!matches!(interrupt.kind, InterruptKind::Restart)).then_some(interrupt.kind);
            self.unload_request = Some(interrupt.unload_request);
        }
        self.unloaded_interrupt = interrupt;
        self
    }
}

#[derive(Debug, PartialEq, Eq)]
enum PeriodicSnapshotAction {
    NotNeeded,
    DueNow,
    Wait(Duration),
}

struct PeriodicHost<'i, 'a, Ctx: WorkerCtx>(&'i mut Invocation<'a, Ctx>);

struct UpdateHost<'i, 'a, Ctx: WorkerCtx> {
    invocation: &'i mut Invocation<'a, Ctx>,
    target_revision: ComponentRevision,
    /// The admission of the update, which its `PendingUpdate` and `FailedUpdate` name.
    update_attempt_index: OplogIndex,
}

#[cfg(test)]
mod tests;
