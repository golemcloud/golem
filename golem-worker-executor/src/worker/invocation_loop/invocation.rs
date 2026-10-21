use super::*;

impl<Ctx: WorkerCtx> InvocationLoop<Ctx> {
    pub(super) async fn record_retry_interrupt_failure(
        &self,
        store: &Mutex<Store<Ctx>>,
        kind: InterruptKind,
    ) {
        // The invocation callback may have committed its outcome while this stop was being
        // established. Only an invocation still running or retrying needs another failure record.
        let status = self.parent.get_last_known_status().await;
        if !matches!(status.status, AgentStatus::Running | AgentStatus::Retrying) {
            return;
        }
        store
            .lock()
            .await
            .data_mut()
            .on_invocation_failure("interrupted during retry", &TrapType::Interrupt(kind))
            .await;
    }
}

impl<Ctx: WorkerCtx> Invocation<'_, Ctx> {
    /// Process a queued worker invocation
    pub(super) async fn process(&mut self, message: QueuedWorkerInvocation) -> CommandOutcome {
        match message {
            QueuedWorkerInvocation::GetFileSystemNode { path, sender, .. } => {
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
            QueuedWorkerInvocation::ReadFile {
                path,
                selection,
                sender,
                ..
            } => {
                self.read_file(path, selection, sender).await;
                CommandOutcome::Continue
            }
            QueuedWorkerInvocation::AwaitReadyToProcessCommands { sender } => {
                let _ = sender.send(Ok(()));
                CommandOutcome::Continue
            }
            QueuedWorkerInvocation::SaveSnapshot => self.take_guest_snapshot().await,
        }
    }

    /// Process an external queued worker invocation - this is either an exported function invocation
    /// or a manual update request (which involves invoking the exported save-snapshot functions, so
    /// it is a special case of the exported function invocation).
    pub(super) async fn external_invocation(
        &mut self,
        inner: HydratedInvocation,
        update_attempt_index: OplogIndex,
    ) -> CommandOutcome {
        // Rechecked here as well as where the invocation was taken: hydrating it and waiting for
        // the store both leave room for the owner to start retiring in between.
        if self.parent.owner_retirement_requested.is_cancelled() {
            return CommandOutcome::BreakInnerLoop(RetryDecision::None);
        }
        match inner.invocation {
            AgentInvocation::ManualUpdate { target_revision } => {
                self.manual_update(target_revision, update_attempt_index)
                    .await
            }
            _ => {
                if let Some(idempotency_key) = inner.invocation.idempotency_key() {
                    let has_result = matches!(
                        self.parent.lookup_invocation_result(idempotency_key).await,
                        LookupResult::Complete(_) | LookupResult::Interrupted
                    );
                    if !has_result {
                        self.invoke_agent(inner).await
                    } else {
                        debug!(
                            "Skipping enqueued invocation with idempotency key {idempotency_key} as it already has a result"
                        );
                        // Retirement waits for this loop; stop awaiting an operation that may
                        // itself be waiting for retirement's stop to finish.
                        match self
                            .parent
                            .owner_retirement_requested
                            .run_until_cancelled(
                                self.parent.cancel_invocation(idempotency_key.clone()),
                            )
                            .await
                        {
                            Some(Ok(())) => CommandOutcome::Continue,
                            None => CommandOutcome::BreakInnerLoop(RetryDecision::None),
                            Some(Err(error)) if self.parent.retire_if_shard_lost(&error) => {
                                CommandOutcome::BreakInnerLoop(RetryDecision::None)
                            }
                            Some(Err(error)) => {
                                warn!(
                                    agent_id = %self.owned_agent_id.agent_id,
                                    %error,
                                    "Failed to remove completed invocation from the pending queue"
                                );
                                CommandOutcome::BreakInnerLoop(RetryDecision::Immediate)
                            }
                        }
                    }
                } else {
                    self.invoke_agent(inner).await
                }
            }
        }
    }

    /// Invokes an agent function on the worker
    pub(super) async fn invoke_agent(&mut self, invocation: HydratedInvocation) -> CommandOutcome {
        let display_name = invocation.invocation.display_name();
        let invocation_context = invocation.invocation.invocation_context();
        let idempotency_key = invocation
            .invocation
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

        let outcome = self
            .invoke_agent_inner(invocation_context, idempotency_key, invocation)
            .instrument(span)
            .await;
        self.store.data().set_suspended();
        outcome
    }

    /// Invokes an agent function on the worker
    ///
    /// The inner implementation of `invoke_agent` to be instrumented with a span.
    pub(super) async fn invoke_agent_inner(
        &mut self,
        invocation_context: InvocationContextStack,
        idempotency_key: IdempotencyKey,
        invocation: HydratedInvocation,
    ) -> CommandOutcome {
        let kind = invocation.invocation.kind();
        let display_name = invocation.invocation.display_name();
        let invocation_idempotency_key = idempotency_key.clone();
        self.uses_streams = invocation_uses_streams(
            &invocation.invocation,
            &self.store.data().component_metadata().metadata,
            self.parent.parsed_agent_id.as_ref(),
        );
        let admission =
            monthly_resource_admission(&self.parent.resource_entry, self.parent.agent_mode());
        if admission != MonthlyResourceAdmission::Admit {
            self.store
                .data_mut()
                .set_current_idempotency_key(idempotency_key.clone())
                .await;
        }
        match admission {
            MonthlyResourceAdmission::Admit => {}
            MonthlyResourceAdmission::Unavailable => {
                return self
                    .agent_invocation_failed(
                        &display_name,
                        &invocation_idempotency_key,
                        Err(MonthlyResourceCapacityError::unavailable_error()),
                    )
                    .await;
            }
            MonthlyResourceAdmission::Suspend => {
                return self
                    .agent_invocation_failed(
                        &display_name,
                        &invocation_idempotency_key,
                        Ok(InvokeResult::Interrupted {
                            consumed_fuel: 0,
                            interrupt_kind: InterruptKind::Suspend(Timestamp::now_utc()),
                        }),
                    )
                    .await;
            }
            MonthlyResourceAdmission::FailInvocation(exhaustion) => {
                return self
                    .agent_invocation_failed(
                        &display_name,
                        &invocation_idempotency_key,
                        Err(monthly_resource_exhausted_invocation_error(
                            &self.parent.config(),
                            exhaustion,
                        )),
                    )
                    .await;
            }
        }
        let result = self
            .invoke_agent_with_context(invocation_context, idempotency_key, invocation)
            .await;

        match result {
            Ok(InvokeResult::Succeeded {
                result: invocation_result,
                consumed_fuel,
            }) => {
                if let Some(interrupt) = self.parent.interrupt_signal.claim_pending_terminal().await
                {
                    interrupt.establishment.wait().await;
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
                    self.agent_invocation_finished(
                        display_name,
                        &invocation_idempotency_key,
                        *invocation_result,
                        consumed_fuel,
                        kind,
                    )
                    .await
                }
            }
            _ => {
                self.agent_invocation_failed(&display_name, &invocation_idempotency_key, result)
                    .await
            }
        }
    }

    /// Sets the necessary contextual information on the worker and performs the actual
    /// invocation.
    pub(super) async fn invoke_agent_with_context(
        &mut self,
        mut invocation_context: InvocationContextStack,
        idempotency_key: IdempotencyKey,
        invocation: HydratedInvocation,
    ) -> Result<InvokeResult, WorkerExecutorError> {
        let (lowered, local_span_ids, inherited_span_ids) = async {
            self.store
                .data_mut()
                .set_current_idempotency_key(idempotency_key.clone())
                .await;

            Self::extend_invocation_context(
                &mut invocation_context,
                &idempotency_key,
                &invocation.invocation,
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

            let invocation_for_lowering = if self.uses_streams {
                self.parent
                    .rehydrate_durable_streaming_invocation(invocation.invocation.clone())
                    .await?
            } else {
                invocation.invocation.clone()
            };
            let lowered = lower_invocation(
                invocation_for_lowering,
                &self.store.data().component_metadata().metadata,
                self.parent.parsed_agent_id.as_ref(),
            )?;

            Ok::<_, WorkerExecutorError>((lowered, local_span_ids, inherited_span_ids))
        }
        .instrument(span!(Level::INFO, "prepare_invocation_context"))
        .await?;

        let display_name = lowered.display_name.clone();
        let result = invoke_observed_and_traced(
            lowered,
            self.store,
            self.instance,
            InvocationMode::Live(invocation),
        )
        .await;
        let result = if self.uses_streams {
            materialize_streaming_result(self.store, result, &display_name, &idempotency_key).await
        } else {
            result
        };

        // Invocation-owned spans are closed by AgentInvocationFinished in the
        // oplog processor; removing their resident context records no transition.
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
    pub(super) async fn agent_invocation_finished(
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
                if self.uses_streams {
                    let parent = self.parent.clone();
                    let idempotency_key = idempotency_key.clone();
                    // Host stream operations can retain session locks across a pending poll.
                    // Keep driving the Store while protocol-only completion acquires them.
                    let result = self
                        .store
                        .run_concurrent(async move |_accessor| {
                            parent
                                .complete_durable_streaming_session(&idempotency_key)
                                .await
                        })
                        .await;
                    if let Err(error) = result
                        .map_err(|error| WorkerExecutorError::runtime(error.to_string()))
                        .and_then(|result| result)
                    {
                        tracing::error!(%error, "Failed to complete durable streaming session");
                        // An in-place retry would reopen the oplog at an epoch this executor no
                        // longer holds, so a lost shard gives the agent up instead.
                        if self.parent.retire_if_shard_lost(&error) {
                            self.store
                                .data_mut()
                                .on_invocation_failure(
                                    &full_function_name,
                                    &TrapType::Interrupt(InterruptKind::ShardLost),
                                )
                                .await;
                            return CommandOutcome::BreakInnerLoop(RetryDecision::None);
                        }
                        self.parent
                            .durable_stream_producer
                            .changed()
                            .notify_waiters();
                        return failed_agent_invocation_outcome(
                            self.parent.agent_mode(),
                            RetryDecision::Immediate,
                            None,
                        );
                    }
                }
                self.parent.reset_infrastructure_recovery_backoff();
                successful_agent_invocation_outcome(
                    self.parent.agent_mode(),
                    self.store.data().component_metadata().metadata.is_agent(),
                    kind,
                )
            }
            Err(WorkerExecutorError::Interrupted { kind }) => {
                let decision = match self
                    .store
                    .data_mut()
                    .on_invocation_failure(&full_function_name, &TrapType::Interrupt(kind))
                    .await
                {
                    InvocationFailureDecision::Retry(decision) => decision,
                    InvocationFailureDecision::Infrastructure(error) => {
                        return self.invocation_failure_recovery(&error);
                    }
                };
                // A lost shard writes nothing more: a terminal streaming-session failure would end
                // an invocation the shard's new owner resumes.
                if matches!(kind, InterruptKind::ShardLost) {
                    return CommandOutcome::BreakInnerLoop(RetryDecision::None);
                }
                if self.uses_streams
                    && !matches!(kind, InterruptKind::Jump)
                    && let Err(error) = self
                        .parent
                        .fail_durable_streaming_session(idempotency_key, kind.to_string())
                        .await
                {
                    if self.parent.retire_if_shard_lost(&error) {
                        return CommandOutcome::BreakInnerLoop(RetryDecision::None);
                    }
                    self.parent
                        .durable_stream_producer
                        .changed()
                        .notify_waiters();
                }
                failed_agent_invocation_outcome(self.parent.agent_mode(), decision, None)
            }
            // Intercepted before the arm below, which would flatten it into an
            // `AgentError::InternalError` and append an `Error` entry to the very oplog that
            // just refused the write.
            Err(WorkerExecutorError::OplogFenced { .. }) => {
                self.store
                    .data_mut()
                    .on_invocation_failure(
                        &full_function_name,
                        &TrapType::Interrupt(InterruptKind::ShardLost),
                    )
                    .await;
                CommandOutcome::BreakInnerLoop(RetryDecision::None)
            }
            // The oplog refused the end of the invocation: the agent recovers, or stops with the
            // error, instead of recording an `Error` entry in the oplog that refused it.
            Err(
                error @ (WorkerExecutorError::RecoveryRequired { .. }
                | WorkerExecutorError::OplogFailed { .. }),
            ) => {
                self.parent.record_recovery_failure(&error).await;
                self.invocation_failure_recovery(&error)
            }
            Err(
                error @ WorkerExecutorError::InvocationFailed {
                    error:
                        AgentError::EphemeralCannotSuspend(_) | AgentError::EphemeralFuelExhausted(_),
                    ..
                },
            ) => {
                self.agent_invocation_failed(&full_function_name, idempotency_key, Err(error))
                    .await
            }
            Err(error) => {
                // A refused commit is the `OplogFenced` arm above; this is the hook's own failure.
                let trap_type = TrapType::Error {
                    error: AgentError::InternalError(error.to_string()),
                    retry_from: OplogIndex::INITIAL,
                    semantic_trap_retry_override: None,
                };
                if let InvocationFailureDecision::Infrastructure(error) = self
                    .store
                    .data_mut()
                    .on_invocation_failure(&full_function_name, &trap_type)
                    .await
                {
                    return self.invocation_failure_recovery(&error);
                }
                if self.uses_streams
                    && let Err(error) = self
                        .parent
                        .fail_durable_streaming_session(idempotency_key, error.to_string())
                        .await
                {
                    if self.parent.retire_if_shard_lost(&error) {
                        return CommandOutcome::BreakInnerLoop(RetryDecision::None);
                    }
                    self.parent
                        .durable_stream_producer
                        .changed()
                        .notify_waiters();
                }
                failed_agent_invocation_outcome(
                    self.parent.agent_mode(),
                    RetryDecision::None,
                    Some(&error),
                )
            }
        }
    }

    /// What the loop does when the oplog storage did not answer while the failure of an invocation
    /// was recorded: the agent recovers after a delay, and the invocation stays pending.
    pub(super) fn invocation_failure_recovery(
        &self,
        error: &WorkerExecutorError,
    ) -> CommandOutcome {
        infrastructure_failure_outcome(self.parent.retired_for_lost_shard(), error, || {
            self.parent.next_infrastructure_recovery_delay()
        })
    }

    /// The logic handling an agent invocation that did not succeed.
    pub(super) async fn agent_invocation_failed(
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
        let oplog_failure = result.as_ref().err().cloned();
        let owner_failure = self
            .store
            .data()
            .durable_ctx()
            .selected_tool_owner_failure();
        debug!(
            owner_id = %self.parent.owned_agent_id(),
            owner_failure = ?owner_failure,
            "Classifying failed invocation after tool owner arbitration"
        );
        if let Some(error) = crate::durable_host::selected_recovery_error(
            owner_failure.as_ref(),
            self.parent.agent_mode(),
            &result,
        ) {
            self.parent.record_recovery_failure(error).await;
            return infrastructure_failure_outcome(
                self.parent.retired_for_lost_shard(),
                error,
                || self.parent.next_infrastructure_recovery_delay(),
            );
        }
        let trap_type = match owner_failure {
            Some(OwnerFailureWinner::Trap(trap)) => Some(trap),
            Some(OwnerFailureWinner::Lifecycle(kind)) => Some(TrapType::Interrupt(kind)),
            Some(OwnerFailureWinner::Infrastructure(error)) => {
                Some(TrapType::from_worker_executor_error::<Ctx>(
                    error,
                    OplogIndex::INITIAL,
                    self.parent.agent_mode(),
                ))
            }
            None => match result {
                Ok(invoke_result) => invoke_result.as_trap_type::<Ctx>(),
                Err(error) => Some(TrapType::from_worker_executor_error::<Ctx>(
                    error,
                    OplogIndex::INITIAL,
                    self.parent.agent_mode(),
                )),
            },
        };
        let shard_lost = matches!(
            trap_type,
            Some(TrapType::Interrupt(InterruptKind::ShardLost))
        );
        let decision = match trap_type {
            Some(trap_type) => match self
                .store
                .data_mut()
                .on_invocation_failure(full_function_name, &trap_type)
                .await
            {
                InvocationFailureDecision::Retry(decision) => decision,
                InvocationFailureDecision::Infrastructure(error) => {
                    return self.invocation_failure_recovery(&error);
                }
            },
            None => RetryDecision::None,
        };
        // A lost shard writes nothing more: a terminal streaming-session failure would end an
        // invocation the shard's new owner resumes.
        if shard_lost {
            return CommandOutcome::BreakInnerLoop(RetryDecision::None);
        }

        if self.uses_streams
            && decision == RetryDecision::None
            && let Err(error) = self
                .parent
                .fail_durable_streaming_session(idempotency_key, details)
                .await
        {
            if self.parent.retire_if_shard_lost(&error) {
                return CommandOutcome::BreakInnerLoop(RetryDecision::None);
            }
            self.parent
                .durable_stream_producer
                .changed()
                .notify_waiters();
        }

        failed_agent_invocation_outcome(self.parent.agent_mode(), decision, oplog_failure.as_ref())
    }
}

impl<Ctx: WorkerCtx> Invocation<'_, Ctx> {
    /// Extends the invocation context with a new span containing information about the invocation
    pub(super) fn extend_invocation_context(
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
}

pub(super) fn should_terminally_interrupt_ephemeral(
    owner_kind: OwnerKind,
    agent_mode: AgentMode,
    decision: Option<&RetryDecision>,
) -> bool {
    match decision {
        Some(
            RetryDecision::Immediate | RetryDecision::Delayed(_) | RetryDecision::ReacquirePermits,
        ) => agent_mode == AgentMode::Ephemeral,
        Some(RetryDecision::TryStop(_)) => owner_kind == OwnerKind::EphemeralExternalTool,
        None | Some(RetryDecision::None) => false,
    }
}

pub(super) fn successful_agent_invocation_outcome(
    agent_mode: AgentMode,
    is_agent_component: bool,
    kind: AgentInvocationKind,
) -> CommandOutcome {
    if should_cleanup_terminal_ephemeral_invocation(agent_mode, is_agent_component, kind) {
        CommandOutcome::BreakInnerLoopAndArchiveEphemeralOplog(RetryDecision::None, None)
    } else {
        CommandOutcome::Continue
    }
}

/// What the loop does after an invocation that did not succeed. A terminal ephemeral failure
/// archives the oplog; when the oplog of the agent failed (`oplog_failure`), the waiters get that
/// error with its cause.
pub(super) fn failed_agent_invocation_outcome(
    agent_mode: AgentMode,
    decision: RetryDecision,
    oplog_failure: Option<&WorkerExecutorError>,
) -> CommandOutcome {
    if agent_mode == AgentMode::Ephemeral && decision == RetryDecision::None {
        CommandOutcome::BreakInnerLoopAndArchiveEphemeralOplog(
            decision,
            oplog_failure
                .filter(|error| matches!(error, WorkerExecutorError::OplogFailed { .. }))
                .cloned(),
        )
    } else {
        CommandOutcome::BreakInnerLoop(decision)
    }
}

/// What the loop does after an infrastructure failure of an invocation. A lost shard stops the
/// agent: its waiters are sent to the new owner. An oplog that failed for good stops the agent too,
/// because a reconstruction would fail again, and the stop gives the waiters that error, with its
/// cause. Any other failure reconstructs the agent after `delay`.
pub(super) fn infrastructure_failure_outcome(
    retired_for_lost_shard: bool,
    error: &WorkerExecutorError,
    delay: impl FnOnce() -> Duration,
) -> CommandOutcome {
    if retired_for_lost_shard {
        CommandOutcome::BreakInnerLoop(RetryDecision::None)
    } else if matches!(error, WorkerExecutorError::OplogFailed { .. }) {
        CommandOutcome::BreakOuterLoop(Some(error.clone()))
    } else {
        CommandOutcome::BreakInnerLoopForRecovery {
            decision: RetryDecision::Delayed(delay()),
            error: error.clone(),
        }
    }
}

pub(super) fn is_recovery_required_error(error: &WorkerExecutorError) -> bool {
    match error {
        WorkerExecutorError::RecoveryRequired { .. } => true,
        WorkerExecutorError::FailedToResumeAgent { reason, .. } => {
            is_recovery_required_error(reason)
        }
        _ => false,
    }
}

pub(super) fn should_cleanup_terminal_ephemeral_invocation(
    agent_mode: AgentMode,
    is_agent_component: bool,
    kind: AgentInvocationKind,
) -> bool {
    agent_mode == AgentMode::Ephemeral
        && !(is_agent_component && kind == AgentInvocationKind::AgentInitialization)
}
