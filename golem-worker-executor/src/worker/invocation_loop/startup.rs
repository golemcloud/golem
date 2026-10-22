use super::*;

impl<Ctx: WorkerCtx> InvocationLoop<Ctx> {
    /// Stops a generation whose shard was lost, found by an interrupt or by its oplog refusing a
    /// lifecycle entry: the waiters are told to look for the shard's new owner. Both have already
    /// recorded the retirement - `interrupt_and_retire` before it sends `ShardLost`, the write
    /// helper with the fence that refused it.
    pub(super) async fn stop_startup_retired(&self) {
        self.stop_unloaded(None, PendingLiveInvocationDisposition::Fail)
            .await;
    }

    /// Handles an interrupt that came before the instance of the start ran: the kind that
    /// `create_instance` gave, or the interrupt that waits. It records the same lifecycle oplog
    /// entry that the invocation failure path records (`Suspend` or `Interrupted`), then parks,
    /// stops or restarts the start. The concurrent-agent permit is released first.
    pub(super) async fn interrupted_before_instance(
        &mut self,
        kind: InterruptKind,
        pending_interrupt: Option<PendingWorkerInterrupt>,
    ) -> StartupStep {
        self.release_concurrent_agent_permit();
        let kind = pending_interrupt
            .as_ref()
            .map(|interrupt| interrupt.kind)
            .unwrap_or(kind);
        if self.parent.initial_worker_metadata.owner_kind == OwnerKind::EphemeralExternalTool {
            // Core initialization has already entered the executable Store. Losing it
            // is terminal for an external owner, just as losing its invocation body is.
            if self
                .parent
                .add_and_commit_oplog(OplogEntry::interrupted())
                .await
                .is_err()
            {
                // The shard has a new owner. Give the agent up without archiving:
                // the archive would move an oplog that is no longer this executor's.
                self.stop_startup_retired().await;
                return StartupStep::Stop;
            }
            self.stop_unloaded(
                Some(crate::worker::inactive_ephemeral_agent_error()),
                PendingLiveInvocationDisposition::Fail,
            )
            .await;
            self.archive_ephemeral_oplog();
            return StartupStep::Stop;
        }
        // Interrupted while instantiating: record the same lifecycle oplog entry the
        // invocation failure path would (`Suspend`/`Interrupted`), then park or
        // restart. There is no store to run `on_invocation_failure` on, but no
        // invocation was running either — the status marker is all that is needed.
        match kind {
            InterruptKind::EntityCancellation => {
                unreachable!("entity cancellation cannot interrupt the owner")
            }
            InterruptKind::Restart | InterruptKind::Jump => {
                self.parent.acknowledge_startup_interruption(kind).await;
                debug!("Instantiation interrupted for restart, retrying");
                StartupStep::Retry
            }
            InterruptKind::Suspend(ts) => {
                if self
                    .parent
                    .add_and_commit_oplog(OplogEntry::suspend())
                    .await
                    .is_err()
                {
                    self.stop_startup_retired().await;
                    return StartupStep::Stop;
                }
                if ts < *self.parent.last_resume_request.lock().await {
                    debug!(
                        "Suspend during instantiation ignored because there was a resume request since it"
                    );
                    StartupStep::Retry
                } else {
                    self.parent.complete_startup(self.start_attempt, Ok(()));
                    self.stop_unloaded(
                        None,
                        resident_work_disposition(
                            pending_interrupt
                                .map(|interrupt| interrupt.unload_request.reason)
                                .unwrap_or(UnloadReason::Suspend),
                        ),
                    )
                    .await;
                    StartupStep::Stop
                }
            }
            InterruptKind::Interrupt(_) => {
                if self
                    .parent
                    .add_and_commit_oplog(OplogEntry::interrupted())
                    .await
                    .is_err()
                {
                    self.stop_startup_retired().await;
                    return StartupStep::Stop;
                }
                self.parent.complete_startup(
                    self.start_attempt,
                    Err(WorkerExecutorError::Interrupted { kind }),
                );
                self.stop_unloaded(None, PendingLiveInvocationDisposition::Fail)
                    .await;
                StartupStep::Stop
            }
            InterruptKind::ShardLost => {
                // Nothing is written: the oplog belongs to the shard's new owner
                // now. Whoever was waiting for this start is told to look there.
                self.stop_startup_retired().await;
                StartupStep::Stop
            }
        }
    }
}

impl<Ctx: WorkerCtx> InvocationLoop<Ctx> {
    /// Create the worker instance and publish an event about it
    pub(super) async fn create_instance(
        &mut self,
        mut permit: crate::services::active_agents::ConcurrentAgentPermit,
    ) -> CreateInstanceResult<Ctx> {
        let phase = agent_phase_span!(self, "create_instance");
        async {
            debug!("Creating the worker instance");
            if let crate::worker::WorkerInstance::Running(running) =
                self.parent.instance.lock().await.deletion_runtime_mut()
            {
                if running.owner_generation_started {
                    *self.parent.release_scope.lock().unwrap() = Default::default();
                }
                running.owner_generation_started = false;
            }
            self.permit_state.release_scope = self.parent.release_scope();
            permit.observe_release(&self.permit_state.release_scope);
            self.parent
                .linear_memory_grant()
                .lock()
                .unwrap()
                .observe_release(&self.permit_state.release_scope);
            self.parent.unload_cleanup.lock().unwrap().take();
            match monthly_resource_admission(&self.parent.resource_entry, self.parent.agent_mode())
            {
                MonthlyResourceAdmission::Admit => {}
                MonthlyResourceAdmission::Unavailable => {
                    self.stop_unloaded(
                        Some(MonthlyResourceCapacityError::unavailable_error()),
                        PendingLiveInvocationDisposition::Fail,
                    )
                    .await;
                    return CreateInstanceResult::Failed;
                }
                MonthlyResourceAdmission::Suspend => {
                    return CreateInstanceResult::Interrupted(InterruptKind::Suspend(
                        Timestamp::now_utc(),
                    ));
                }
                MonthlyResourceAdmission::FailInvocation(exhaustion) => {
                    let error = monthly_resource_exhausted_invocation_error(
                        &self.parent.config(),
                        exhaustion,
                    );
                    self.stop_unloaded(Some(error), PendingLiveInvocationDisposition::Fail)
                        .await;
                    return CreateInstanceResult::Failed;
                }
            }
            // The start future lives on the heap to keep the invocation-loop stack small.
            match Box::pin(RunningWorker::create_instance(
                self.parent.clone(),
                permit,
                self.start_attempt,
                &self.filesystem_snapshot_slot,
            ))
            .await
            {
                Ok((agent, window, recovery_decision)) => CreateInstanceResult::Created {
                    agent: Box::new(agent),
                    window,
                    recovery_decision,
                },
                // Instance creation was interrupted by a recoverable condition. The worker exists
                // and its metadata and `Create` oplog entry are already persisted, so the caller
                // parks or restarts the worker without exposing an unprepared runtime.
                Err(CreateWorkerInstanceError::ReacquireMemory) => {
                    CreateInstanceResult::ReacquireMemory
                }
                Err(CreateWorkerInstanceError::Failed {
                    error: WorkerExecutorError::Interrupted { kind },
                    filesystem_cleanup_failure: None,
                }) => {
                    debug!("Worker instantiation interrupted: {kind:?}");
                    CreateInstanceResult::Interrupted(kind)
                }
                Err(CreateWorkerInstanceError::Failed {
                    error: err,
                    filesystem_cleanup_failure: None,
                }) if self.parent.agent_mode() == AgentMode::Durable
                    && is_recovery_required_error(&err) =>
                {
                    self.parent.record_recovery_failure(&err).await;
                    CreateInstanceResult::RecoveryRequired(err)
                }
                Err(CreateWorkerInstanceError::Failed {
                    error: err,
                    filesystem_cleanup_failure,
                }) => {
                    warn!("Failed to start the worker: {err}");
                    let err = if matches!(
                        err,
                        WorkerExecutorError::PreviousInvocationFailed { .. }
                            | WorkerExecutorError::PreviousInvocationExited
                    ) {
                        err
                    } else {
                        self.parent.record_recovery_failure(&err).await;
                        err
                    };
                    self.parent
                        .complete_startup(self.start_attempt, Err(err.clone()));
                    let final_state = if let Some(failure) = filesystem_cleanup_failure {
                        let (cleanup, sender) =
                            UnloadCleanup::new(Some(self.permit_state.release_scope.clone()));
                        let _ = sender.send(Err(failure));
                        *self.parent.unload_cleanup.lock().unwrap() = Some(cleanup);
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
        .instrument(phase)
        .await
    }
}
