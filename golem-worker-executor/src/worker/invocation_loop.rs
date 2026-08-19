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
use crate::services::agent_filesystem::AgentFilesystem;
use crate::services::agent_resource_billing::AgentResourceBilling;
use crate::services::golem_config::SnapshotPolicy;
use crate::services::oplog::{CommitLevel, EphemeralOplog, OplogOps};
use crate::services::{HasOplog, HasShardService, HasWorker};
use crate::worker::invocation::{
    InvocationMode, InvokeResult, invoke_observed_and_traced, lower_invocation,
};
use crate::worker::status_checkpointer;
use crate::worker::{
    CreateWorkerInstanceError, FinalWorkerState, PendingWorkerInterrupt, QueuedWorkerInvocation,
    RetryDecision, RunningWorker, Worker, WorkerCommand, WorkerInterruptState, WorkerTrace,
};
use crate::workerctx::{PublicWorkerIo, UpdateManagement, WorkerCtx};
use anyhow::anyhow;
use async_lock::Mutex;
use drop_stream::DropStream;
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
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;
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
        ConcurrentAgentPermitState<crate::services::active_workers::ConcurrentAgentPermit>,
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
        instance: Instance,
        store: Mutex<Store<Ctx>>,
        filesystem: Box<AgentFilesystem>,
    },
    /// Instance creation was interrupted by a recoverable condition, such as fuel or filesystem
    /// quota exhaustion. The worker metadata remains valid and queued work must be preserved.
    Interrupted(InterruptKind),
    /// Instance creation failed; the worker was already stopped with the startup failure.
    Failed,
}

async fn settle_reconstructed_filesystem(
    filesystem: &AgentFilesystem,
) -> Result<(), WorkerExecutorError> {
    filesystem
        .settle_reconstruction()
        .await
        .map_err(|error| WorkerExecutorError::runtime(error.to_string()))?;
    debug!("Agent filesystem reconstruction settled");
    Ok(())
}

impl<Ctx: WorkerCtx> InvocationLoop<Ctx> {
    async fn pending_interrupt(&self) -> Option<(InterruptKind, RetryDecision)> {
        take_pending_interrupt(&self.interrupt_signal)
            .await
            .map(|interrupt| (interrupt.kind, interrupt.retry_decision()))
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

        'outer: loop {
            self.release_terminal_interrupt().await;
            if let Err(error) = self.parent.shard_service().check_worker(&agent_id) {
                debug!(%agent_id, "Worker generation not started because its shard is not assigned");
                self.parent.complete_startup(self.start_attempt, Err(error));
                self.release_concurrent_agent_permit();
                self.stop_unloaded(None).await;
                break;
            }
            self.acquire_concurrent_agent_permit().await;
            let (instance, store, filesystem) = match self.create_instance().await {
                CreateInstanceResult::Created {
                    instance,
                    store,
                    filesystem,
                } => (instance, store, *filesystem),
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
                                self.parent.complete_startup(
                                    self.start_attempt,
                                    Err(WorkerExecutorError::Interrupted { kind }),
                                );
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
            let resource_billing = store.lock().await.data().durable_ctx().resource_billing();
            if let Err(error) = resource_billing.open(&filesystem.runtime()).await {
                let error = WorkerExecutorError::runtime(format!(
                    "Failed to open worker resource billing window: {error}"
                ));
                self.parent
                    .complete_startup(self.start_attempt, Err(error.clone()));
                self.release_concurrent_agent_permit();
                filesystem.seal();
                drop(store);
                let cleanup_result = filesystem.close_and_delete().await;
                match cleanup_result {
                    Ok(()) => self.stop_unloaded(Some(error)).await,
                    Err(cleanup) => {
                        self.stop_cleanup_failed(WorkerExecutorError::runtime(format!(
                            "{error}; {cleanup}"
                        )))
                        .await;
                    }
                }
                break;
            }

            let (mut final_decision, recovery_failure) = match self
                .recover_instance_state(&instance, &store, &filesystem)
                .await
            {
                Ok(decision) => (decision, None),
                Err(error) => {
                    self.parent
                        .complete_startup(self.start_attempt, Err(error.clone()));
                    (Some(RetryDecision::None), Some(error))
                }
            };
            let mut final_interrupt = None;
            let mut cleanup_ephemeral_worker = false;

            if recovery_failure.is_none()
                && let Some((kind, decision)) = self.pending_interrupt().await
            {
                debug!(
                    %agent_id,
                    ?decision,
                    "Invocation queue loop interrupted after recovery"
                );
                if !matches!(kind, InterruptKind::Restart | InterruptKind::Jump) {
                    final_interrupt = Some(kind);
                }
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
                let resource_billing = store.lock().await.data().durable_ctx().resource_billing();
                let mut inner_loop = InnerInvocationLoop {
                    receiver: &mut self.receiver,
                    active: self.active.clone(),
                    owned_agent_id: self.owned_agent_id.clone(),
                    parent: self.parent.clone(),
                    waiting_for_command: self.waiting_for_command.clone(),
                    interrupt_signal: self.interrupt_signal.clone(),
                    instance: &instance,
                    store: &store,
                    filesystem: &filesystem,
                    resource_billing,
                    invocations_since_snapshot: 0,
                    idle_snapshot_task: None,
                    permit_state: &mut self.permit_state,
                    idle_since_millis: self.idle_since_millis.clone(),
                    resume_replay_pending: self.resume_replay_pending.clone(),
                    worker_trace: self.worker_trace.clone(),
                    deferred_wakeups: &mut deferred_wakeups,
                };

                let result = inner_loop.run().await;
                final_decision = result.retry_decision;
                final_interrupt = result.final_interrupt;
                if let Some((kind, decision)) = self.pending_interrupt().await {
                    if !matches!(kind, InterruptKind::Restart | InterruptKind::Jump) {
                        final_interrupt = Some(kind);
                    }
                    final_decision = Some(decision);
                }
                cleanup_ephemeral_worker = result.cleanup_ephemeral_worker;
            }

            let resource_close_error = self
                .close_resource_window_and_release(&store, &filesystem)
                .await
                .err();
            if let Some(error) = resource_close_error.as_ref() {
                warn!(error = %error, "Resource-window close failed; reconstructing or unloading the worker");
            }
            final_decision =
                decision_after_resource_close(final_decision, resource_close_error.is_some());

            self.suspend_worker(&store).await;

            if let Some(kind) = final_interrupt {
                self.record_retry_interrupt_failure(&store, kind).await;
            }

            let mut runtime = Some((instance, store, filesystem));

            match final_decision {
                None | Some(RetryDecision::None) => {
                    let cleanup_error = Self::close_runtime(&mut runtime).await;
                    debug!(
                        %agent_id,
                        "Invocation queue loop notifying parent about being stopped"
                    );
                    self.stop_closed(
                        cleanup_error,
                        recovery_failure.or(resource_close_error).or_else(|| {
                            cleanup_ephemeral_worker.then(super::inactive_ephemeral_agent_error)
                        }),
                    )
                    .await;
                    if cleanup_ephemeral_worker {
                        self.parent.remove_from_active_workers().await;
                        self.archive_ephemeral_oplog();
                    }
                    break;
                }
                Some(RetryDecision::TryStop(ts)) => {
                    if ts < *self.parent.last_resume_request.lock().await {
                        if let Some(error) = Self::close_runtime(&mut runtime).await {
                            self.stop_cleanup_failed(error).await;
                            break;
                        }
                        debug!(
                            %agent_id,
                            "Suspend request ignored because there was a resume request since it"
                        );
                        continue;
                    } else {
                        let cleanup_error = Self::close_runtime(&mut runtime).await;
                        debug!(
                            %agent_id,
                            "Invocation queue loop notifying parent about being stopped"
                        );
                        self.stop_closed(cleanup_error, resource_close_error).await;
                        break;
                    }
                }
                Some(RetryDecision::Immediate) => {
                    if let Some(error) = Self::close_runtime(&mut runtime).await {
                        self.stop_cleanup_failed(error).await;
                        break;
                    }
                    debug!(%agent_id, "Invocation queue loop triggering restart immediately");
                    continue;
                }
                Some(RetryDecision::Delayed(delay)) => {
                    debug_assert!(resource_close_error.is_none());
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
                                if let Some(error) = Self::close_runtime(&mut runtime).await {
                                    self.stop_cleanup_failed(error).await;
                                    break 'outer;
                                }
                                debug!(%agent_id, "Invocation queue loop restarting after delay");
                                continue 'outer;
                            }
                            command = self.receiver.recv() => {
                                let command = match command {
                                    Some(command) => command,
                                    None => {
                                        let cleanup_error = Self::close_runtime(&mut runtime).await;
                                        debug!(%agent_id, "Invocation queue loop command channel closed during delayed retry");
                                        self.stop_closed(cleanup_error, None).await;
                                        break 'outer;
                                    }
                                };

                                if let Some((kind, decision)) = self.pending_interrupt().await {
                                    debug!(%agent_id, ?decision, "Invocation queue loop interrupted during delayed retry");
                                    if !matches!(kind, InterruptKind::Restart | InterruptKind::Jump) {
                                        let store = &runtime.as_ref().expect("runtime must be open").1;
                                        self.record_retry_interrupt_failure(store, kind).await;
                                    }
                                    match decision {
                                        RetryDecision::Immediate => {
                                            if let Some(error) = Self::close_runtime(&mut runtime).await {
                                                self.stop_cleanup_failed(error).await;
                                                break 'outer;
                                            }
                                            Self::defer_wakeup(&mut deferred_wakeups, command);
                                            continue 'outer;
                                        }
                                        RetryDecision::None => {
                                            let cleanup_error = Self::close_runtime(&mut runtime).await;
                                            self.stop_closed(cleanup_error, None).await;
                                            break 'outer;
                                        }
                                        RetryDecision::TryStop(timestamp) => {
                                            let cleanup_error = Self::close_runtime(&mut runtime).await;
                                            if timestamp < *self.parent.last_resume_request.lock().await {
                                                if let Some(error) = cleanup_error {
                                                    self.stop_cleanup_failed(error).await;
                                                    break 'outer;
                                                }
                                                Self::defer_wakeup(&mut deferred_wakeups, command);
                                                continue 'outer;
                                            }
                                            self.stop_closed(cleanup_error, None).await;
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
                                        if let Some(error) = Self::close_runtime(&mut runtime).await {
                                            self.stop_cleanup_failed(error).await;
                                            break 'outer;
                                        }
                                        Self::defer_wakeup(&mut deferred_wakeups, WorkerCommand::WorkAvailable);
                                        continue 'outer;
                                    }
                                    WorkerCommand::ResumeReplay => {
                                        debug!(%agent_id, "Invocation queue loop woke up for resume replay during delayed retry");
                                        if let Some(error) = Self::close_runtime(&mut runtime).await {
                                            self.stop_cleanup_failed(error).await;
                                            break 'outer;
                                        }
                                        Self::defer_wakeup(&mut deferred_wakeups, WorkerCommand::ResumeReplay);
                                        continue 'outer;
                                    }
                                }
                            }
                        }
                    }
                }
                Some(RetryDecision::ReacquirePermits) => {
                    if let Some(error) = Self::close_runtime(&mut runtime).await {
                        self.stop_cleanup_failed(error).await;
                        break;
                    }
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
        self.parent.filesystem_limit_interrupt.lock().await.take();
        self.interrupt_signal.lock().await.release_terminal_claim();
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
            self.permit_state.install(permit);
        }
    }

    fn release_concurrent_agent_permit(&mut self) {
        self.permit_state.release();
    }

    async fn close_resource_window_and_release(
        &mut self,
        store: &Mutex<Store<Ctx>>,
        filesystem: &AgentFilesystem,
    ) -> Result<(), WorkerExecutorError> {
        filesystem.seal();
        let resource_billing = store.lock().await.data().durable_ctx().resource_billing();
        if resource_billing.is_active() {
            self.permit_state
                .close_then_release(async {
                    resource_billing
                        .close(&filesystem.runtime())
                        .await
                        .map_err(|error| {
                            WorkerExecutorError::runtime(format!(
                                "Failed to close worker resource billing window: {error}"
                            ))
                        })
                })
                .await
        } else {
            self.release_concurrent_agent_permit();
            Ok(())
        }
    }

    async fn stop_unloaded(&self, startup_failure: Option<WorkerExecutorError>) {
        self.parent.complete_startup(
            self.start_attempt,
            Err(startup_failure.clone().unwrap_or_else(|| {
                WorkerExecutorError::unknown("Worker stopped before startup completed")
            })),
        );
        let pending_failure = startup_failure.clone();
        self.parent
            .stop_internal(
                true,
                pending_failure,
                FinalWorkerState::Unloaded { startup_failure },
            )
            .await;
    }

    async fn stop_cleanup_failed(&self, error: WorkerExecutorError) {
        self.parent
            .complete_startup(self.start_attempt, Err(error.clone()));
        let pending_failure = error.clone();
        self.parent
            .stop_internal(
                true,
                Some(pending_failure),
                FinalWorkerState::CleanupFailed(error),
            )
            .await;
    }

    async fn stop_closed(
        &self,
        cleanup_error: Option<WorkerExecutorError>,
        startup_failure: Option<WorkerExecutorError>,
    ) {
        match cleanup_error {
            Some(error) => self.stop_cleanup_failed(error).await,
            None => self.stop_unloaded(startup_failure).await,
        }
    }

    async fn close_runtime(
        runtime: &mut Option<(Instance, Mutex<Store<Ctx>>, AgentFilesystem)>,
    ) -> Option<WorkerExecutorError> {
        let (_instance, store, filesystem) = runtime.take()?;
        let resource_billing = store.lock().await.data().durable_ctx().resource_billing();
        debug_assert!(!resource_billing.is_active());
        drop(store);
        filesystem.close_and_delete().await.err().map(|error| {
            error!(error = %error, "Failed to delete agent runtime filesystem");
            WorkerExecutorError::runtime(error.to_string())
        })
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
        };

        if !already_deferred {
            deferred_wakeups.push_back(command);
        }
    }

    /// Create the worker instance and publish an event about it
    async fn create_instance(&self) -> CreateInstanceResult<Ctx> {
        async {
            debug!("Creating the worker instance");
            match RunningWorker::create_instance(self.parent.clone()).await {
                Ok((instance, store, filesystem)) => CreateInstanceResult::Created {
                    instance,
                    store,
                    filesystem: Box::new(filesystem),
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
                        .stop_internal(true, Some(err), final_state)
                        .await;
                    CreateInstanceResult::Failed
                }
            }
        }
        .instrument(agent_phase_span!(self, "create_instance"))
        .await
    }

    /// Prepares the instance for running by recovering its persisted state
    ///
    /// In case of failure to recover the state, it returns the retry decision to be used.
    async fn recover_instance_state(
        &self,
        instance: &Instance,
        store: &Mutex<Store<Ctx>>,
        filesystem: &AgentFilesystem,
    ) -> Result<Option<RetryDecision>, WorkerExecutorError> {
        async {
            debug!("Preparing the worker instance");
            let mut store = store.lock().await;

            store.data().set_suspended();

            let span = span!(
                Level::INFO,
                "invocation",
                agent_id = %self.owned_agent_id.agent_id,
                agent_type = %self.worker_trace.agent_type,
            );
            let prepare_result =
                Ctx::prepare_instance(&self.owned_agent_id.agent_id, instance, &mut *store)
                    .instrument(span)
                    .await;

            match prepare_result {
                Ok(decision) => {
                    if decision.is_none() {
                        settle_reconstructed_filesystem(filesystem).await?;
                    }
                    debug!("Recovery decision from prepare_instance: {decision:?}");
                    Ok(decision)
                }
                Err(err) => {
                    warn!("Failed to start the worker: {err}");
                    store.data().set_suspended();
                    Err(err)
                }
            }
        }
        .instrument(agent_phase_span!(self, "recover_instance_state"))
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

struct InnerInvocationLoop<'a, Ctx: WorkerCtx> {
    receiver: &'a mut UnboundedReceiver<WorkerCommand>,
    active: Arc<RwLock<VecDeque<QueuedWorkerInvocation>>>,
    owned_agent_id: OwnedAgentId,
    parent: Arc<Worker<Ctx>>, // parent must not be dropped until the invocation_loop is running
    waiting_for_command: Arc<AtomicBool>,
    interrupt_signal: Arc<Mutex<WorkerInterruptState>>,
    instance: &'a Instance,
    store: &'a Mutex<Store<Ctx>>,
    filesystem: &'a AgentFilesystem,
    resource_billing: AgentResourceBilling,
    invocations_since_snapshot: u64,
    idle_snapshot_task: Option<JoinHandle<()>>,
    /// Mutable reference to the concurrent-agent permit held by the outer
    /// `InvocationLoop`. Set to `None` when entering idle (releasing the
    /// permit back to the semaphore pool) and re-acquired on wake.
    permit_state:
        &'a mut ConcurrentAgentPermitState<crate::services::active_workers::ConcurrentAgentPermit>,
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
        let mut cleanup_ephemeral_worker = false;

        // Entering idle: release the concurrent-agent permit so other agents
        // from the same account can start without evicting this one.
        self.check_no_active_tail_work_on_idle().await;
        if let Err(error) = self.release_concurrent_agent_permit().await {
            error!(error = %error, "Failed to close worker resource billing window");
            return InnerInvocationLoopResult {
                retry_decision: Some(RetryDecision::Immediate),
                final_interrupt: None,
                cleanup_ephemeral_worker: false,
            };
        }
        mark_idle(&self.idle_since_millis);
        self.waiting_for_command.store(true, Ordering::Release);
        while let Some(cmd) = self.next_wakeup_or_initial().await {
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
                            break self.interrupt(interrupt).await;
                        }

                        let message = self.active.write().await.pop_front();

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
            };
            match outcome {
                CommandOutcome::BreakOuterLoop => {
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
            cleanup_ephemeral_worker,
        }
    }

    async fn next_wakeup_or_initial(&mut self) -> Option<WorkerCommand> {
        match self.deferred_wakeups.pop_front() {
            Some(command) => Some(command),
            None => self.next_wakeup().await,
        }
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
            let runtime = self.filesystem.runtime();
            let result = self
                .permit_state
                .close_then_release(async {
                    self.resource_billing
                        .close(&runtime)
                        .await
                        .map_err(|error| WorkerExecutorError::runtime(error.to_string()))
                })
                .await;
            return result;
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
            self.resource_billing
                .open(&self.filesystem.runtime())
                .await
                .map_err(|error| WorkerExecutorError::runtime(error.to_string()))?;
            self.permit_state.install(permit);
        }
        Ok(())
    }

    async fn next_wakeup(&mut self) -> Option<WorkerCommand> {
        let mut idle_snapshot_task = self.idle_snapshot_task.take();

        let wakeup = if let Some(task) = idle_snapshot_task.as_mut() {
            tokio::select! {
                cmd = self.receiver.recv() => {
                    task.abort();
                    cmd
                }
                result = &mut *task => {
                    if let Err(err) = result {
                        if !err.is_cancelled() {
                            warn!(agent_id = %self.owned_agent_id.agent_id, "Idle snapshot timer failed: {err}");
                        }
                        return self.receiver.recv().await;
                    }

                    match self.receiver.try_recv() {
                        Ok(cmd) => Some(cmd),
                        Err(tokio::sync::mpsc::error::TryRecvError::Empty) => Some(WorkerCommand::WorkAvailable),
                        Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => None,
                    }
                }
            }
        } else {
            self.receiver.recv().await
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

            let resume_replay_result =
                match Ctx::resume_replay(&mut *store, self.instance, true).await {
                    Ok(None) => settle_reconstructed_filesystem(self.filesystem)
                        .await
                        .map(|()| None),
                    other => other,
                };

            match resume_replay_result {
                Ok(None) => CommandOutcome::Continue,
                Ok(Some(decision)) => CommandOutcome::BreakInnerLoop(decision),
                Err(err) => {
                    warn!("Failed to resume replay: {err}");
                    store.data().set_suspended();

                    self.parent
                        .stop_internal(
                            true,
                            Some(err.clone()),
                            FinalWorkerState::Unloaded {
                                startup_failure: Some(err),
                            },
                        )
                        .await;
                    CommandOutcome::BreakOuterLoop
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

pub(super) struct ConcurrentAgentPermitState<T> {
    permit: Option<T>,
    held: Arc<AtomicBool>,
}

impl<T> ConcurrentAgentPermitState<T> {
    pub(super) fn new(permit: Option<T>, held: Arc<AtomicBool>) -> Self {
        held.store(permit.is_some(), Ordering::Release);
        Self { permit, held }
    }

    fn is_some(&self) -> bool {
        self.permit.is_some()
    }

    fn is_none(&self) -> bool {
        self.permit.is_none()
    }

    fn install(&mut self, permit: T) {
        debug_assert!(self.permit.is_none());
        self.permit = Some(permit);
        self.held.store(true, Ordering::Release);
    }

    fn release(&mut self) {
        if let Some(permit) = self.permit.take() {
            drop(permit);
            self.held.store(false, Ordering::Release);
        } else {
            debug_assert!(!self.held.load(Ordering::Acquire));
        }
    }

    async fn close_then_release<E>(
        &mut self,
        close: impl Future<Output = Result<(), E>>,
    ) -> Result<(), E> {
        let result = close.await;
        self.release();
        result
    }
}

fn decision_after_resource_close(
    decision: Option<RetryDecision>,
    close_failed: bool,
) -> Option<RetryDecision> {
    if close_failed
        && matches!(
            decision,
            Some(RetryDecision::Immediate | RetryDecision::Delayed(_))
        )
    {
        Some(RetryDecision::Immediate)
    } else {
        decision
    }
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
        let result = self
            .invoke_agent_with_context(invocation_context, idempotency_key, invocation)
            .await;

        match result {
            Ok(InvokeResult::Succeeded {
                result: invocation_result,
                consumed_fuel,
            }) => {
                let mut interrupt_state = self.parent.interrupt_signal.lock().await;
                if let Some(interrupt) = interrupt_state.claim_pending_terminal() {
                    drop(interrupt_state);
                    self.agent_invocation_failed(
                        &display_name,
                        Ok(InvokeResult::Interrupted {
                            consumed_fuel,
                            interrupt_kind: interrupt.kind,
                        }),
                    )
                    .await
                } else {
                    drop(interrupt_state);
                    self.agent_invocation_finished(
                        display_name,
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
                self.agent_invocation_failed(&display_name, result).await
            }
            _ => self.agent_invocation_failed(&display_name, result).await,
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

            let invocation_for_lowering = invocation.clone();
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
            Ok(()) => successful_agent_invocation_outcome(
                self.parent.agent_mode(),
                self.store.data().component_metadata().metadata.is_agent(),
                kind,
            ),
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
                failed_agent_invocation_outcome(self.parent.agent_mode(), RetryDecision::None)
            }
        }
    }

    /// The logic handling an agent invocation that did not succeed.
    async fn agent_invocation_failed(
        &mut self,
        full_function_name: &str,
        result: Result<InvokeResult, WorkerExecutorError>,
    ) -> CommandOutcome {
        let trap_type = match result {
            Ok(invoke_result) => invoke_result.as_trap_type::<Ctx>(),
            Err(error) => Some(TrapType::from_error::<Ctx>(
                &anyhow!(error),
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
                                    self.parent
                                        .add_and_commit_oplog(OplogEntry::snapshot(
                                            payload,
                                            snapshot.mime_type,
                                            active_cards,
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
    BreakOuterLoop,
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
    cleanup_ephemeral_worker: bool,
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
        CommandOutcome, ConcurrentAgentPermitState, PeriodicSnapshotAction,
        decision_after_resource_close, failed_agent_invocation_outcome,
        periodic_snapshot_failure_outcome, snapshot_action_at, snapshot_baseline_timestamp,
        successful_agent_invocation_outcome,
    };
    use crate::worker::RetryDecision;
    use crate::worker::invocation::InvokeResult;
    use golem_common::model::AgentInvocationKind;
    use golem_common::model::agent::AgentMode;
    use golem_common::model::oplog::AgentError;
    use golem_common::model::{OplogIndex, Timestamp};
    use golem_service_base::error::worker_executor::WorkerExecutorError;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::time::Duration;
    use test_r::test;

    struct TestPermit {
        held: Arc<AtomicBool>,
        drops: Arc<AtomicUsize>,
        held_while_dropping: Arc<AtomicBool>,
    }

    impl Drop for TestPermit {
        fn drop(&mut self) {
            self.held_while_dropping
                .store(self.held.load(Ordering::Acquire), Ordering::Release);
            self.drops.fetch_add(1, Ordering::AcqRel);
        }
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
    fn close_failure_skips_delayed_sleep_and_reconstructs_immediately() {
        assert_eq!(
            decision_after_resource_close(
                Some(RetryDecision::Delayed(Duration::from_secs(30))),
                true,
            ),
            Some(RetryDecision::Immediate)
        );
        assert_eq!(
            decision_after_resource_close(Some(RetryDecision::Immediate), true),
            Some(RetryDecision::Immediate)
        );
        let timestamp = Timestamp::from(1_000);
        assert_eq!(
            decision_after_resource_close(Some(RetryDecision::TryStop(timestamp)), true),
            Some(RetryDecision::TryStop(timestamp))
        );
    }

    #[test]
    async fn close_failure_still_releases_the_permit() {
        let held = Arc::new(AtomicBool::new(false));
        let drops = Arc::new(AtomicUsize::new(0));
        let mut state = ConcurrentAgentPermitState::new(None, held.clone());
        state.install(TestPermit {
            held: held.clone(),
            drops: drops.clone(),
            held_while_dropping: Arc::new(AtomicBool::new(false)),
        });

        let result = state
            .close_then_release(async { Err::<(), _>("authoritative usage failed") })
            .await;

        assert_eq!(result, Err("authoritative usage failed"));
        assert!(state.is_none());
        assert!(!held.load(Ordering::Acquire));
        assert_eq!(drops.load(Ordering::Acquire), 1);
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
}
