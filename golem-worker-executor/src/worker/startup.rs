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

use crate::worker::*;

impl<Ctx: WorkerCtx> Worker<Ctx> {
    pub async fn start_if_needed(
        this: Arc<Worker<Ctx>>,
    ) -> Result<Option<Uuid>, WorkerExecutorError> {
        Self::start_if_needed_internal(this, 0, None).await
    }
    pub(in crate::worker) async fn start_if_needed_internal(
        this: Arc<Worker<Ctx>>,
        oom_retry_count: u32,
        existing_start_attempt: Option<Uuid>,
    ) -> Result<Option<Uuid>, WorkerExecutorError> {
        {
            *this.last_resume_request.lock().await = Timestamp::now_utc();
        }

        let mut instance_guard = this.lock_non_stopping_worker().await;
        if let Some(error) = this.monthly_cleanup_failure() {
            return Err(error);
        }
        instance_guard.ensure_not_deleting()?;
        // A handle kept past a retired generation must not start it again: the start would take
        // permits, could append `Resumed` to an oplog the new owner now writes, and a failure in
        // it would publish, by agent id, to the waiters of the generation that replaced this one.
        if this.owner_retirement_requested.is_cancelled() {
            return Err(this.retirement_error());
        }
        match &*instance_guard {
            WorkerInstance::Unloaded {
                startup_failure: Some(err),
            } if this.agent_mode() == AgentMode::Ephemeral => {
                crate::metrics::ephemeral::record_inactive_invocation_failure();
                Err(err.clone())
            }
            WorkerInstance::Unloaded { .. } => {
                let start_attempt =
                    existing_start_attempt.or_else(|| this.startup_attempt.pending());
                if start_attempt.is_none() {
                    this.clear_unavailable_periodic();
                }
                let memory_requirement = match this.memory_requirement().await {
                    Ok(memory_requirement) => memory_requirement,
                    Err(error) => {
                        *instance_guard = WorkerInstance::Unloaded {
                            startup_failure: Some(error.clone()),
                        };
                        this.fail_pending_invocations(error.clone()).await;
                        drop(instance_guard);
                        if let Some(start_attempt) = start_attempt {
                            this.complete_startup(start_attempt, Err(error.clone()));
                        }
                        return Err(error);
                    }
                };
                let status = this.state_actor.status().await;
                if this.agent_mode() == AgentMode::Durable
                    && status.status == AgentStatus::Interrupted
                    && status.current_idempotency_key.is_some()
                {
                    this.add_and_commit_oplog_internal(
                        &instance_guard,
                        OplogEntry::resumed(),
                        None,
                    )
                    .await?;
                }
                let start_attempt = this.startup_attempt.begin(start_attempt);
                this.mark_as_loading(start_attempt);
                crate::metrics::workers::inc_worker_waiting_for_memory();
                *instance_guard = WorkerInstance::WaitingForPermit(WaitingWorker::new(
                    this.clone(),
                    memory_requirement,
                    oom_retry_count,
                    start_attempt,
                ));
                Ok(Some(start_attempt))
            }
            WorkerInstance::CleanupFailed(error) => Err(error.clone()),
            WorkerInstance::WaitingForPermit(waiting) => Ok(Some(waiting.start_attempt)),
            WorkerInstance::Running(_) => this.startup_attempt.current(),
            WorkerInstance::Deleting(_) | WorkerInstance::StoppedForDeletion => Err(
                WorkerExecutorError::invalid_request("Worker is being deleted"),
            ),
            WorkerInstance::Unresolved | WorkerInstance::Initializing(_) => Err(
                WorkerExecutorError::runtime("Worker initialization has not completed"),
            ),
            WorkerInstance::Stopping(_) => panic!("impossible"),
        }
    }
    pub(in crate::worker) fn mark_as_loading(&self, start_attempt: Uuid) {
        self.startup_attempt.begin(Some(start_attempt));
        self.monthly_stop.lock().unwrap().take();
        *self.release_scope.lock().unwrap() = Default::default();
        let mut execution_status = self.execution_status.write().unwrap();
        *execution_status = ExecutionStatus::loading(execution_status.agent_mode());
    }
    pub(in crate::worker) fn publish_startup_result(
        &self,
        start_attempt: Uuid,
        result: Result<(), WorkerExecutorError>,
    ) {
        // A start of an agent retired for a lost shard is never a success, and however it failed,
        // its waiters are told to retry on the shard's new owner: a stop that reports "stopped
        // before startup completed", or the recovery error the lost shard caused, would hand them
        // a failure they surface instead of retrying.
        let result = if self.retired_for_lost_shard() {
            Err(WorkerExecutorError::ShardingNotReady)
        } else {
            result
        };
        if !self.startup_attempt.complete(start_attempt, &result) {
            return;
        }
        self.publish_completed_startup_result(start_attempt, result);
    }
    pub(in crate::worker) fn publish_completed_startup_result(
        &self,
        start_attempt: Uuid,
        result: Result<(), WorkerExecutorError>,
    ) {
        self.events().publish(Event::WorkerLoaded {
            agent_id: self.agent_id(),
            start_attempt,
            result,
        });
    }
    pub(crate) fn complete_startup(
        &self,
        start_attempt: Uuid,
        result: Result<(), WorkerExecutorError>,
    ) {
        self.publish_startup_result(start_attempt, result);
    }
    pub(crate) async fn complete_startup_success(&self, start_attempt: Uuid) -> bool {
        let instance_guard = self.instance.lock().await;
        let active_attempt = match &*instance_guard {
            WorkerInstance::Running(running) => Some(running.start_attempt),
            _ => None,
        };
        let is_active = active_attempt == Some(start_attempt);
        let mut result = if self.owner_retirement_requested.is_cancelled() {
            Err(self.retirement_error())
        } else if is_active {
            Ok(())
        } else {
            Err(WorkerExecutorError::unknown(
                "Worker stopped before startup completed",
            ))
        };
        // The success marker is skipped for a retiring owner: for a lost shard its oplog belongs
        // to the new owner, which clears the recovery error itself when it starts the agent.
        if result.is_ok()
            && self.get_last_known_status().await.last_error_kind == Some(OplogErrorKind::Recovery)
        {
            // Refused, the start is not a success: the caller stops it.
            result = self
                .add_and_commit_oplog_internal(
                    &instance_guard,
                    OplogEntry::recovery_succeeded(),
                    None,
                )
                .await
                .map(|_| ())
                .map_err(WorkerExecutorError::from);
        }
        // Mapped after the awaits above, so a shard lost during them is what the waiters hear.
        if self.retired_for_lost_shard() {
            result = Err(WorkerExecutorError::ShardingNotReady);
        }

        let completed = match &result {
            Ok(()) => self
                .startup_attempt
                .complete_success_if_active(start_attempt, active_attempt),
            Err(_) => self.startup_attempt.complete(start_attempt, &result),
        };

        let started = result.is_ok();
        if completed {
            self.publish_completed_startup_result(start_attempt, result);
        }
        drop(instance_guard);
        started
    }
    pub(crate) fn pending_startup_attempt(&self) -> Option<Uuid> {
        self.startup_attempt.pending()
    }
    pub(in crate::worker) async fn start_waiting_worker(
        this: Arc<Worker<Ctx>>,
        memory_grant: MemoryGrant,
        component_charge: WorkerComponentCharge,
        concurrent_agent_permit: crate::services::active_agents::ConcurrentAgentPermit,
        oom_retry_count: u32,
        start_attempt: Uuid,
        worker_trace: WorkerTrace,
    ) {
        let mut instance_guard = this.instance.lock().await;
        match &*instance_guard {
            WorkerInstance::WaitingForPermit(waiting_worker)
                if waiting_worker.start_attempt == start_attempt =>
            {
                this.interrupt_signal
                    .reset_terminal_for_new_generation()
                    .await;
                let running = RunningWorker::new(
                    this.owned_agent_id.clone(),
                    this.queue.clone(),
                    this.clone(),
                    memory_grant,
                    component_charge,
                    concurrent_agent_permit,
                    oom_retry_count,
                    start_attempt,
                    worker_trace,
                )
                .await;
                crate::metrics::workers::dec_worker_waiting_for_memory();
                match running {
                    Ok(running) => {
                        crate::metrics::workers::inc_worker_memory_resident();
                        *instance_guard = WorkerInstance::Running(running);
                    }
                    Err(error) => {
                        this.complete_startup(start_attempt, Err(error.clone()));
                        *instance_guard = WorkerInstance::Unloaded {
                            startup_failure: Some(error),
                        };
                    }
                }
            }
            _ => {
                debug!("worker was not waiting for permit anymore, not starting");
                // The worker is not becoming resident: dropping `memory_grant`
                // here returns its reservation to the gate.
            }
        }
    }
}

impl RunningWorker {
    pub async fn new<Ctx: WorkerCtx>(
        owned_agent_id: OwnedAgentId,
        queue: Arc<RwLock<VecDeque<QueuedWorkerInvocation>>>,
        parent: Arc<Worker<Ctx>>,
        memory_grant: MemoryGrant,
        component_charge: WorkerComponentCharge,
        concurrent_agent_permit: crate::services::active_agents::ConcurrentAgentPermit,
        oom_retry_count: u32,
        start_attempt: Uuid,
        worker_trace: WorkerTrace,
    ) -> Result<Self, WorkerExecutorError> {
        let completion = tasks::TaskScope::default();
        completion
            .bind(&parent.tasks)
            .map_err(WorkerExecutorError::invalid_request)?;
        let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
        sender.send(WorkerCommand::WorkAvailable).unwrap();

        let active_clone = queue.clone();
        let owned_agent_id_clone = owned_agent_id.clone();
        let waiting_for_command = Arc::new(AtomicBool::new(false));
        let waiting_for_command_clone = waiting_for_command.clone();
        let concurrent_agent_permit_held = Arc::new(AtomicBool::new(true));
        let concurrent_agent_permit_held_clone = Arc::clone(&concurrent_agent_permit_held);
        let filesystem_activity = Arc::new(StdMutex::new(None));
        let filesystem_activity_clone = Arc::clone(&filesystem_activity);
        let filesystem_snapshot_slot = filesystem_snapshots::SnapshotSlot::default();
        let filesystem_snapshot_slot_clone = filesystem_snapshot_slot.clone();
        let unload_request = Arc::new(StdMutex::new(None));
        let unload_request_clone = Arc::clone(&unload_request);
        let idle_since_millis = Arc::new(AtomicU64::new(0));
        let idle_since_millis_clone = Arc::clone(&idle_since_millis);
        let interrupts = parent.interrupt_signal.clone();
        let interrupts_clone = interrupts.clone();
        let resume_replay_pending = Arc::new(AtomicBool::new(false));
        let resume_replay_pending_clone = resume_replay_pending.clone();
        let memory_grant = Arc::new(StdMutex::new(memory_grant));
        let memory_grant_registration =
            LinearMemoryGrantRegistration::new(parent.clone(), memory_grant);

        let panic_parent = Arc::clone(&parent);
        let shutdown_parent = Arc::clone(&parent);
        let invocation_loops = parent.active_agents().invocation_loops();
        let invocation_loop_task = async move {
            RunningWorker::invocation_loop(
                receiver,
                active_clone,
                owned_agent_id_clone,
                parent,
                waiting_for_command_clone,
                interrupts_clone,
                oom_retry_count,
                concurrent_agent_permit,
                concurrent_agent_permit_held_clone,
                filesystem_activity_clone,
                filesystem_snapshot_slot_clone,
                unload_request_clone,
                idle_since_millis_clone,
                resume_replay_pending_clone,
                start_attempt,
                worker_trace,
            )
            .await;
            drop((memory_grant_registration, component_charge));
        };
        let handle = invocation_loops.spawn(
            async move {
                // Join-only ownership: interruption must finish the loop's commits and unload,
                // not cancel its future. Release after both the loop and panic cleanup finish.
                let _completion = completion;
                run_invocation_loop_task(
                    invocation_loop_task,
                    move |error: WorkerExecutorError| async move {
                        panic_parent.complete_startup(start_attempt, Err(error.clone()));
                        panic_parent
                            .stop_internal(
                                true,
                                Some(error.clone()),
                                UnloadRequest::ordinary(UnloadReason::Panic),
                                FinalWorkerState::CleanupFailed(error),
                                PendingLiveInvocationDisposition::Fail,
                            )
                            .await;
                    },
                )
                .await;
            },
            move || {
                let shutdown = shutdown_parent.durable_stream_producer.shutdown();
                Box::pin(async move {
                    let _ = shutdown.await;
                })
            },
        );

        Ok(RunningWorker {
            handle: Some(handle),
            sender,
            queue,
            waiting_for_command,
            concurrent_agent_permit_held,
            filesystem_activity,
            filesystem_snapshot_slot,
            unload_request,
            idle_since_millis,
            interrupts,
            resume_replay_pending,
            start_attempt,
            owner_generation_started: false,
        })
    }
    pub fn stop(mut self, unload_request: UnloadRequest) -> JoinHandle<()> {
        *self.unload_request.lock().unwrap() = Some(unload_request);
        self.handle.take().unwrap()
    }
    pub(in crate::worker) async fn invocation_loop<Ctx: WorkerCtx>(
        receiver: UnboundedReceiver<WorkerCommand>,
        active: Arc<RwLock<VecDeque<QueuedWorkerInvocation>>>,
        owned_agent_id: OwnedAgentId,
        parent: Arc<Worker<Ctx>>, // parent must not be dropped until the invocation_loop is running
        waiting_for_command: Arc<AtomicBool>,
        interrupts: Arc<Interrupts>,
        oom_retry_count: u32,
        concurrent_agent_permit: crate::services::active_agents::ConcurrentAgentPermit,
        concurrent_agent_permit_held: Arc<AtomicBool>,
        filesystem_activity: Arc<StdMutex<Option<ResidentFilesystemActivity>>>,
        filesystem_snapshot_slot: filesystem_snapshots::SnapshotSlot,
        unload_request: Arc<StdMutex<Option<UnloadRequest>>>,
        idle_since_millis: Arc<AtomicU64>,
        resume_replay_pending: Arc<AtomicBool>,
        start_attempt: Uuid,
        worker_trace: WorkerTrace,
    ) {
        let release_scope = parent.release_scope();
        let mut invocation_loop = InvocationLoop {
            receiver,
            active,
            owned_agent_id,
            parent,
            waiting_for_command,
            interrupts,
            oom_retry_count,
            permit_state: ConcurrentAgentPermitState::new(
                Some(concurrent_agent_permit.track_held(Arc::clone(&concurrent_agent_permit_held))),
                concurrent_agent_permit_held,
            )
            .with_release_scope(release_scope),
            filesystem_activity,
            filesystem_snapshot_slot,
            last_periodic_attempt: None,
            unload_request,
            idle_since_millis,
            resume_replay_pending,
            start_attempt,
            worker_trace,
        };
        invocation_loop.run().await;
    }
}

impl StartupAttemptTracker {
    pub(in crate::worker) fn begin(&self, existing: Option<Uuid>) -> Uuid {
        let mut state = self.state.lock().unwrap();
        let attempt = existing.or(state.pending).unwrap_or_else(Uuid::new_v4);
        state.pending = Some(attempt);
        state.failure = None;
        attempt
    }

    pub(in crate::worker) fn pending(&self) -> Option<Uuid> {
        self.state.lock().unwrap().pending
    }

    pub(in crate::worker) fn current(&self) -> Result<Option<Uuid>, WorkerExecutorError> {
        let state = self.state.lock().unwrap();
        match (state.pending, state.failure.as_ref()) {
            (Some(attempt), _) => Ok(Some(attempt)),
            (None, Some(error)) => Err(error.clone()),
            (None, None) => Ok(None),
        }
    }

    pub(in crate::worker) fn complete(
        &self,
        attempt: Uuid,
        result: &Result<(), WorkerExecutorError>,
    ) -> bool {
        let mut state = self.state.lock().unwrap();
        if state.pending != Some(attempt) {
            return false;
        }
        state.failure = result.as_ref().err().cloned();
        state.pending = None;
        true
    }

    pub(in crate::worker) fn complete_success_if_active(
        &self,
        attempt: Uuid,
        active_attempt: Option<Uuid>,
    ) -> bool {
        if active_attempt != Some(attempt) {
            return false;
        }
        self.complete(attempt, &Ok(()))
    }
}

impl WaitingWorker {
    pub fn new<Ctx: WorkerCtx>(
        parent: Arc<Worker<Ctx>>,
        memory_requirement: u64,
        oom_retry_count: u32,
        start_attempt: Uuid,
    ) -> Self {
        let worker_trace = parent.trace(TraceOrigin::capture_current());

        let release_scope = parent.release_scope();
        let handle = tokio::task::spawn(async move {
            let agent_id = parent.owned_agent_id.agent_id();
            let registered_concurrent_account = parent.registered_concurrent_account.clone();

            // The start confirms the filesystem snapshot of its record before it takes any
            // permit, so a wait for an upload holds no slot and no memory.
            parent
                .confirm_filesystem_snapshot_before_start(start_attempt)
                .await;

            // Determine the component's compiled-module size before acquiring
            // the per-account concurrency slot (and before reserving memory),
            // so the worker's memory and its module are admitted together (the
            // module is reserved first, then the memory admission accounts for
            // it). The module is charged once per resident component and shared
            // by all its workers.
            //
            // Charges the pending-update target revision when one is queued
            // (matching what create_instance loads); only a non-existent
            // target falls back to the current revision, and transient
            // resolution failures are retried rather than wedging the worker
            // in WaitingForPermit or under-reserving against the old revision.
            //
            // This resolution is read-only and holds no permits, so it is done
            // before acquiring the concurrent-agent permit: its retry loop must
            // not hold one of the account's active-agent slots while the worker
            // is not yet running, otherwise a single worker whose target
            // metadata is transiently unavailable could block unrelated workers
            // of the same account from starting.
            // Not spanned: this retries on a 500ms delay and logs once per attempt,
            // so a span covering the whole call would accumulate two events a second
            // for as long as resolution keeps failing, and never close to export
            // them. The retry events stay in the logs, and how long the wait took
            // is recorded as a metric rather than a span.
            let phase_start = std::time::Instant::now();
            let requirement = parent.startup_component_charge_requirement().await;
            parent
                .startup_linear_memory_bytes
                .store(requirement.startup_linear_memory_bytes, Ordering::Release);
            let memory_requirement =
                memory_requirement.max(requirement.reserved_linear_memory_bytes);
            crate::metrics::workers::record_worker_admission_wait(
                AdmissionPhase::ResolveComponentCharge,
                phase_start.elapsed(),
            );

            let phase_start = std::time::Instant::now();
            let mut concurrent_agent_permit = registered_concurrent_account
                .acquire(agent_id.clone())
                .instrument(related_span!(
                    worker_trace.startup_origin,
                    Level::INFO,
                    "acquire_concurrent_agent_slot",
                    %agent_id,
                    agent_type = %worker_trace.agent_type
                ))
                .await;
            concurrent_agent_permit.observe_release(&release_scope);
            crate::metrics::workers::record_worker_admission_wait(
                AdmissionPhase::ConcurrencySlot,
                phase_start.elapsed(),
            );

            // `memory_grant` and `component_charge` own their reservations
            // from here on: held as locals until the worker becomes resident
            // (when they move into the RunningWorker) or this task ends/aborts
            // (when dropping them returns the reservations to the gate). This
            // is what makes a start cancelled mid-flight — e.g. the worker
            // being deleted while still waiting for its remaining permits —
            // release rather than leak its grant and module charge.
            //
            // Admission is not gated while waiting for a per-account
            // concurrency slot above; otherwise one account could exhaust the
            // memory headroom with workers that are not allowed to run yet.
            let phase_start = std::time::Instant::now();
            // Not spanned, for the same reason as the charge resolution above:
            // `acquire_memory` retries on the same 500ms delay and logs once per
            // attempt. Its duration is recorded as a metric instead.
            let (mut memory_grant, component_charge) = parent
                .active_agents()
                .acquire_with_component_charge(
                    memory_requirement,
                    requirement.component_id,
                    requirement.component_revision,
                    requirement.module_bytes,
                )
                .await;
            memory_grant.observe_release(&release_scope);
            crate::metrics::workers::record_worker_admission_wait(
                AdmissionPhase::Memory,
                phase_start.elapsed(),
            );
            debug!("Attempting to start worker after acquiring enough permits");
            Worker::start_waiting_worker(
                parent,
                memory_grant,
                component_charge,
                concurrent_agent_permit,
                oom_retry_count,
                start_attempt,
                worker_trace,
            )
            .await;
            // If we do not start the worker here we will drop the permits here, which will release them to the host.
        });

        WaitingWorker {
            handle: Some(handle),
            start_attempt,
        }
    }
}

impl Drop for WaitingWorker {
    fn drop(&mut self) {
        if let Some(handle) = self.handle.take() {
            handle.abort();
        }
    }
}
