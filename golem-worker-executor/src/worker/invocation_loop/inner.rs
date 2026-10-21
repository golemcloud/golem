use super::*;

impl SelectedWork {
    pub(super) fn next(
        queue: &mut VecDeque<QueuedWorkerInvocation>,
        status: Arc<AgentStatusRecord>,
        constructor_key: Option<&IdempotencyKey>,
        filesystem_turn_used: &mut bool,
    ) -> Self {
        let filesystem_request = matches!(
            queue.front(),
            Some(
                QueuedWorkerInvocation::ReadFile { .. }
                    | QueuedWorkerInvocation::GetFileSystemNode { .. }
            )
        );
        // Creation reserves the constructor before ordinary invocation admission. A started
        // constructor completes during replay instead of remaining in the pending queue.
        let needs_initialization = filesystem_request
            && constructor_key.is_some_and(|key| {
                status
                    .pending_invocations
                    .first()
                    .is_some_and(|pending| pending.has_idempotency_key(key))
            });
        let pending_work =
            !status.pending_updates.is_empty() || !status.pending_invocations.is_empty();
        let yield_filesystem_turn = filesystem_request && *filesystem_turn_used && pending_work;
        if !needs_initialization
            && !yield_filesystem_turn
            && let Some(invocation) = queue.pop_front()
        {
            if filesystem_request {
                *filesystem_turn_used = true;
            }
            return Self::Resident(invocation);
        }
        *filesystem_turn_used = false;
        if !status.pending_updates.is_empty() {
            return Self::ApplyPendingUpdate;
        }
        if let Some(pending) = status.pending_invocations.first() {
            return Self::Durable(pending.clone());
        }
        Self::Idle(status)
    }
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
        let mut unloaded_interrupt = None;
        let mut cleanup_ephemeral_worker = false;
        let mut recovery_failure = None;
        let mut cleanup_failure = None;
        let mut filesystem_limit_update = None;

        // Entering idle: release the concurrent-agent permit so other agents
        // from the same account can start without evicting this one.
        self.check_no_active_tail_work_on_idle().await;
        if let Err(error) = self.release_concurrent_agent_permit().await {
            error!(error = %error, "Failed to close worker resource billing window");
            return InnerInvocationLoopResult {
                retry_decision: Some(RetryDecision::None),
                final_interrupt: None,
                unload_request: None,
                unloaded_interrupt: None,
                cleanup_ephemeral_worker: false,
                recovery_failure: None,
                cleanup_failure: Some(error),
                filesystem_limit_update: None,
            };
        }
        if !self
            .parent
            .complete_startup_success(self.start_attempt)
            .await
        {
            return InnerInvocationLoopResult {
                retry_decision: Some(RetryDecision::None),
                final_interrupt: None,
                unload_request: None,
                unloaded_interrupt: None,
                cleanup_ephemeral_worker: false,
                recovery_failure: None,
                cleanup_failure: None,
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
            // Wake notifications can outlive the work already drained by an earlier wake.
            if matches!(
                cmd,
                WorkerCommand::WorkAvailable | WorkerCommand::InternalStatusChanged
            ) && !self.pending_work_requires_permit().await
            {
                continue;
            }

            // Waking from idle: re-acquire the concurrent-agent permit before
            // processing any commands.
            self.waiting_for_command.store(false, Ordering::Release);
            match self.acquire_concurrent_agent_permit().await {
                Ok(PermitAcquisition::Acquired(())) => {}
                Ok(PermitAcquisition::Interrupted(interrupt)) => {
                    unloaded_interrupt = Some(interrupt);
                    self.deferred_wakeups.push_front(cmd);
                    break;
                }
                Ok(PermitAcquisition::CommandChannelClosed) => break,
                Err(error) => {
                    error!(error = %error, "Failed to open worker resource billing window");
                    cleanup_failure = Some(error);
                    final_decision = Some(RetryDecision::None);
                    break;
                }
            }
            let outcome = match cmd {
                WorkerCommand::WorkAvailable | WorkerCommand::InternalStatusChanged => {
                    loop {
                        if let Some(interrupt) = take_pending_interrupt(&self.interrupts).await {
                            unloaded_interrupt = Some(interrupt.clone());
                            break self.interrupt(interrupt).await;
                        }

                        let result = match self.select_next_work().await {
                            Ok(SelectedWork::Resident(message)) => {
                                self.internal_invocation(message).await
                            }
                            Ok(SelectedWork::Durable(pending)) => {
                                self.execute_selected_durable(pending).await
                            }
                            Ok(SelectedWork::ApplyPendingUpdate) => {
                                CommandOutcome::BreakInnerLoop(RetryDecision::Immediate)
                            }
                            Ok(SelectedWork::Idle(status)) => {
                                self.handle_idle_status(&status).await
                            }
                            Err(error) => self.unreadable_oplog_outcome(&error).await,
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
                CommandOutcome::BreakInnerLoopForRecovery { decision, error } => {
                    final_decision = Some(decision);
                    recovery_failure = Some(error);
                    break;
                }
                CommandOutcome::BreakInnerLoopAndArchiveEphemeralOplog(decision, failure) => {
                    final_decision = Some(decision);
                    cleanup_ephemeral_worker = true;
                    recovery_failure = failure;
                    break;
                }
                CommandOutcome::Continue | CommandOutcome::WaitForWakeup => {}
            }

            // Returning to idle: release the concurrent-agent permit.
            self.check_no_active_tail_work_on_idle().await;
            if let Err(error) = self.release_concurrent_agent_permit().await {
                error!(error = %error, "Failed to close worker resource billing window");
                cleanup_failure = Some(error);
                final_decision = Some(RetryDecision::None);
                break;
            }
            mark_idle(&self.idle_since_millis);
            self.waiting_for_command.store(true, Ordering::Release);
        }
        self.abort_idle_snapshot_task();
        self.waiting_for_command.store(false, Ordering::Release);

        let result = InnerInvocationLoopResult {
            retry_decision: final_decision,
            final_interrupt: None,
            unload_request: None,
            unloaded_interrupt: None,
            cleanup_ephemeral_worker,
            recovery_failure,
            cleanup_failure,
            filesystem_limit_update,
        }
        .with_unloaded_interrupt(unloaded_interrupt);
        debug!(final_decision = ?result.retry_decision, "Invocation queue loop finished");
        result
    }

    pub(super) async fn pending_work_requires_permit(&self) -> bool {
        if !self.active.read().await.is_empty()
            || self.interrupts.has_interrupt().await
            || self.parent.last_known_status_detached()
        {
            return true;
        }

        let status = self.parent.get_last_known_status().await;
        !status.pending_updates.is_empty()
            || !status.pending_invocations.is_empty()
            || !matches!(
                self.periodic_snapshot_action(&status),
                PeriodicSnapshotAction::NotNeeded
            )
    }

    pub(super) async fn next_wakeup_or_initial(&mut self) -> ResidentWakeup {
        if filesystem_activity(self.filesystem).has_terminal_failure() {
            return ResidentWakeup::FilesystemTerminalFailure;
        }
        match self.deferred_wakeups.pop_front() {
            Some(command) => ResidentWakeup::Command(command),
            None => self.next_wakeup().await,
        }
    }

    /// Filesystem commands alternate with pending work at invocation boundaries after
    /// initialization. Selecting a durable reference does not consume it. Work is chosen only from
    /// a status that is a fold of the oplog: the error of a rebuild that fails is returned instead.
    pub(super) async fn select_next_work(&mut self) -> Result<SelectedWork, WorkerExecutorError> {
        let status = self.parent.get_repaired_status().await?;
        let mut queue = self.active.write().await;
        queue.retain(|invocation| !invocation.is_abandoned());
        if let Err(error) = Worker::<Ctx>::ensure_not_failed(
            &self.parent.deps,
            &self.parent.owned_agent_id,
            self.parent.agent_mode(),
            &status,
        )
        .await
        {
            for invocation in queue.drain(..) {
                invocation.fail(&error);
            }
        }

        let constructor_key = self
            .parent
            .parsed_agent_id
            .as_ref()
            .map(|_| self.parent.initialization_idempotency_key());
        Ok(SelectedWork::next(
            &mut queue,
            status,
            constructor_key.as_ref(),
            &mut self.filesystem_turn_used,
        ))
    }

    /// What the loop does when the oplog does not give what the next work needs: the status
    /// could not be rebuilt, for example after a commit whose receipt its caller already took, or
    /// the selected pending invocation could not be hydrated. The failure is recorded like a
    /// failed recovery: an oplog whose storage did not answer recovers after a delay and keeps its
    /// pending work, and one that does not read for good stops the agent with the cause.
    pub(super) async fn unreadable_oplog_outcome(
        &self,
        error: &WorkerExecutorError,
    ) -> CommandOutcome {
        self.parent.record_recovery_failure(error).await;
        infrastructure_failure_outcome(self.parent.retired_for_lost_shard(), error, || {
            self.parent.next_infrastructure_recovery_delay()
        })
    }

    /// Checks — before publishing `waiting_for_command = true`, which makes the
    /// worker eligible for idle eviction — that no Golem-spawned store task is
    /// still active. Every guest call drains its tail work before returning
    /// (either the tasks finished or they are parked at a safe park point
    /// awaiting future guest action), so an active task here means the activity
    /// accounting regressed and eviction could race live durable work. Flags
    /// the violation instead of silently entering idle.
    pub(super) async fn check_no_active_tail_work_on_idle(&self) {
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
}

impl<Ctx: WorkerCtx> InnerInvocationLoop<'_, Ctx> {
    pub(super) async fn next_wakeup(&mut self) -> ResidentWakeup {
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
}

impl<Ctx: WorkerCtx> InnerInvocationLoop<'_, Ctx> {
    /// Hydrates and executes the durable reference chosen by `select_next_work`. The pending entry
    /// remains authoritative until the existing invocation-start transition consumes it.
    pub(super) async fn execute_selected_durable(
        &mut self,
        pending_invocation: PendingInvocationRef,
    ) -> CommandOutcome {
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
            .hydrate_pending_invocation(&pending_invocation)
            .await
        {
            Ok(invocation) => invocation,
            Err(error) => {
                warn!(
                    agent_id = %self.owned_agent_id.agent_id,
                    "Failed to hydrate pending invocation from oplog: {error}"
                );
                return self.unreadable_oplog_outcome(&error).await;
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
                filesystem: self.filesystem,
                filesystem_snapshot_slot: self.filesystem_snapshot_slot,
                uses_streams: false,
            };
            invocation
                .external_invocation(timestamped_invocation, pending_invocation.oplog_index)
                .await
        }
        .instrument(pickup_span)
        .await;

        match outcome {
            CommandOutcome::Continue => {
                if self.on_external_invocation_completed().await {
                    return CommandOutcome::Continue;
                }
                // Fairness: after completing one external durable
                // invocation, yield to the scheduler so other same-account
                // agents get a chance to run. The worker will self-wake
                // and re-acquire its permit through the FIFO queue if
                // more durable work remains.
                let status = self.parent.get_last_known_status().await;
                if !status.pending_invocations.is_empty() {
                    // More durable work remains — self-wake so we return
                    // to the outer loop, release the permit (entering
                    // idle), and re-enter through the scheduler queue.
                    return CommandOutcome::WaitForWakeup;
                }
                // Revisit commands that arrived during execution before entering idle.
                CommandOutcome::Continue
            }
            other => other,
        }
    }
}

impl<Ctx: WorkerCtx> InnerInvocationLoop<'_, Ctx> {
    /// Resumes an interrupted replay process
    ///
    /// Returns `CommandOutcome` if this fails and the invocation loop should be stopped.
    /// Otherwise, it returns the new retry decision to be used by the outer invocation loop.
    pub(super) async fn resume_replay(&self) -> CommandOutcome {
        async {
            if !matches!(
                monthly_resource_admission(&self.parent.resource_entry, self.parent.agent_mode()),
                MonthlyResourceAdmission::Admit
            ) {
                return CommandOutcome::Continue;
            }
            let mut store = self.store.lock().await;

            let resume_replay_result = Ctx::resume_replay(&mut *store, self.instance, true).await;

            match resume_replay_result {
                Ok(None) => CommandOutcome::Continue,
                Ok(Some(decision)) => CommandOutcome::BreakInnerLoop(decision),
                Err(WorkerExecutorError::Interrupted {
                    kind: InterruptKind::Jump,
                }) => {
                    store.data().set_suspended();
                    CommandOutcome::BreakInnerLoop(RetryDecision::Immediate)
                }
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
    pub(super) async fn internal_invocation(
        &mut self,
        message: QueuedWorkerInvocation,
    ) -> CommandOutcome {
        let snapshot = matches!(message, QueuedWorkerInvocation::SaveSnapshot);
        let mut store = self.store.lock().await;
        let store = store.deref_mut();

        let mut invocation = Invocation {
            owned_agent_id: self.owned_agent_id.clone(),
            parent: self.parent.clone(),
            instance: self.instance,
            store,
            filesystem: self.filesystem,
            filesystem_snapshot_slot: self.filesystem_snapshot_slot,
            uses_streams: false,
        };
        if !snapshot {
            return invocation.process(message).await;
        }
        let (outcome, ended) = with_end(invocation.process(message), Timestamp::now_utc).await;
        *self.last_periodic_attempt = Some(ended);
        outcome
    }

    /// Performs an interrupt request
    pub(super) async fn interrupt(&self, interrupt: PendingWorkerInterrupt) -> CommandOutcome {
        CommandOutcome::BreakInnerLoop(interrupt.retry_decision())
    }
}

pub(super) async fn wait_for_resident_wakeup(
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
