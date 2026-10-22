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

use super::*;

impl<Ctx: WorkerCtx> DurableWorkerCtx<Ctx> {
    fn cleanup_custom_durability_state(&mut self) {
        let resources: Vec<_> = self
            .state
            .custom_invocation_scopes
            .drain()
            .map(|(_, scope)| scope.resource_rep)
            .collect();
        for resource_rep in resources {
            let _ = self
                .table()
                .delete(Resource::<durability::LiveCustomDurableInvocation>::new_own(resource_rep));
        }
        self.state.active_custom_invocations.clear();
    }

    /// The retry of an error trap during a speculative replay: the fixed decision of the trap
    /// when it retries, as for an out-of-memory. Any other error, and a trap that is not an
    /// error, gives `None`.
    pub(crate) fn speculative_retry(trap_type: &TrapType) -> Option<RetryDecision> {
        match trap_type {
            TrapType::Error { .. } => Self::fixed_decision_for_trap_type(trap_type)
                .filter(|decision| *decision != RetryDecision::None),
            TrapType::Interrupt(_) | TrapType::Exit => None,
        }
    }

    /// The retry of a start whose load of an application snapshot ended with `result`: the fixed
    /// decision of an interrupt, and the retry of an error trap that every invocation retries, as
    /// [`Self::speculative_retry`] gives it. Any other result gives `None`.
    pub(crate) fn load_retry(result: &InvokeResult) -> Option<RetryDecision> {
        match result {
            InvokeResult::Interrupted { interrupt_kind, .. } => {
                Self::fixed_decision_for_trap_type(&TrapType::Interrupt(*interrupt_kind))
            }
            result => result
                .as_trap_type::<Ctx>()
                .as_ref()
                .and_then(Self::speculative_retry),
        }
    }

    /// Returns the deterministic, policy-independent recovery decision for a
    /// trap type — i.e. the cases where the answer does not depend on retry
    /// state or any retry policy. For trap-error variants whose decision is
    /// driven by named retry policies (`Unknown` and `TransientError`), this returns `None` and
    /// the caller falls through to policy-based resolution.
    pub(crate) fn fixed_decision_for_trap_type(trap_type: &TrapType) -> Option<RetryDecision> {
        match trap_type {
            TrapType::Interrupt(InterruptKind::EntityCancellation) => {
                unreachable!("entity cancellation must settle at its invocation boundary")
            }
            TrapType::Interrupt(InterruptKind::Interrupt(_)) => Some(RetryDecision::None),
            TrapType::Interrupt(InterruptKind::Suspend(ts)) => Some(RetryDecision::TryStop(*ts)),
            TrapType::Interrupt(InterruptKind::Restart) => Some(RetryDecision::Immediate),
            TrapType::Interrupt(InterruptKind::Jump) => Some(RetryDecision::Immediate),
            // Never retried here: a retry in place would reopen the oplog with the same stale
            // epoch. The worker service resumes the agent on the shard's owner.
            TrapType::Interrupt(InterruptKind::ShardLost) => Some(RetryDecision::None),
            TrapType::Exit => Some(RetryDecision::None),
            TrapType::Error {
                error: AgentError::OutOfMemory,
                ..
            } => Some(RetryDecision::ReacquirePermits),
            TrapType::Error {
                error: AgentError::InvalidRequest(_),
                ..
            } => Some(RetryDecision::None),
            TrapType::Error {
                error: AgentError::PermissionDenied(_),
                ..
            } => Some(RetryDecision::None),
            TrapType::Error {
                error: AgentError::StackOverflow,
                ..
            } => Some(RetryDecision::None),
            TrapType::Error {
                error: AgentError::ExceededMemoryLimit,
                ..
            } => Some(RetryDecision::None),
            TrapType::Error {
                error: AgentError::ExceededTableLimit,
                ..
            } => Some(RetryDecision::None),
            TrapType::Error {
                error: AgentError::AgentTerminatedByQuota(_),
                ..
            } => Some(RetryDecision::None),
            TrapType::Error {
                error:
                    AgentError::EphemeralSleepTooLong(_)
                    | AgentError::EphemeralFuelExhausted(_)
                    | AgentError::EphemeralCannotSuspend(_),
                ..
            } => Some(RetryDecision::None),
            TrapType::Error {
                error: AgentError::ReadOnlyViolation(_),
                ..
            } => Some(RetryDecision::None),
            TrapType::Error {
                error: AgentError::InternalError(_),
                ..
            } => Some(RetryDecision::None),
            TrapType::Error {
                error: AgentError::ExceededHttpCallLimit,
                ..
            } => Some(RetryDecision::None),
            TrapType::Error {
                error: AgentError::ExceededRpcCallLimit,
                ..
            } => Some(RetryDecision::None),
            TrapType::Error {
                error: AgentError::PermanentError(_),
                ..
            } => Some(RetryDecision::None),
            TrapType::Error {
                error: AgentError::DeterministicTrap(_),
                ..
            } => Some(RetryDecision::None),
            TrapType::Error {
                error: AgentError::Unknown(_) | AgentError::TransientError(_),
                ..
            } => None,
        }
    }

    fn semantic_trap_type_name(error: &AgentError) -> &'static str {
        match error {
            AgentError::OutOfMemory => "out-of-memory",
            AgentError::InvalidRequest(_) => "invalid-request",
            AgentError::StackOverflow => "stack-overflow",
            AgentError::ExceededMemoryLimit => "exceeded-memory-limit",
            AgentError::ExceededTableLimit => "exceeded-table-limit",
            AgentError::ExceededHttpCallLimit => "exceeded-http-call-limit",
            AgentError::ExceededRpcCallLimit => "exceeded-rpc-call-limit",
            AgentError::InternalError(_) => "internal-error",
            AgentError::DeterministicTrap(_) => "deterministic-trap",
            AgentError::PermanentError(_) => "permanent-error",
            AgentError::Unknown(_) => "unknown",
            AgentError::TransientError(_) => "transient-error",
            AgentError::AgentTerminatedByQuota(_) => "agent-terminated-by-quota",
            AgentError::EphemeralSleepTooLong(_) => "ephemeral-sleep-too-long",
            AgentError::EphemeralFuelExhausted(_) => "ephemeral-fuel-exhausted",
            AgentError::EphemeralCannotSuspend(_) => "ephemeral-cannot-suspend",
            AgentError::ReadOnlyViolation(_) => "read-only-violation",
            AgentError::PermissionDenied(_) => "permission-denied",
        }
    }

    async fn get_recovery_decision_on_trap_with_semantic(
        &mut self,
        retry_state_with_current_attempt: &HashMap<OplogIndex, RetryPolicyState>,
        trap_type: &TrapType,
        full_function_name: &str,
    ) -> (RetryDecision, Option<RetryPolicyState>) {
        // Cases whose decision does not depend on retry policy at all
        // (Interrupt, Exit, deterministic AgentError variants like
        // OutOfMemory, InvalidRequest, …). Returns `None` when policy
        // resolution is required.
        if let Some(decision) = Self::fixed_decision_for_trap_type(trap_type) {
            return (decision, None);
        }

        // Only Error variants whose decision is policy-driven reach this point
        // (Unknown and TransientError).
        let TrapType::Error {
            error,
            retry_from,
            semantic_trap_retry_override,
            ..
        } = trap_type
        else {
            // Should be unreachable: `fixed_decision_for_trap_type` returns
            // `Some(...)` for every non-Error trap variant. Treat as "give up"
            // defensively.
            return (RetryDecision::None, None);
        };

        // (B) — host-originated trap carrying an already-resolved verdict.
        // The host call resolved the named policy with full properties (e.g.
        // HTTP `status-code`) before escalating to trap+replay; honour that
        // exact verdict so the inline path and the trap path stay
        // semantically equivalent.
        if let Some(override_) = semantic_trap_retry_override {
            let decision = match &override_.verdict {
                crate::durable_host::durability::SemanticTrapRetryVerdict::Retry(delay) => {
                    debug!(
                        retry_policy = %override_.policy_name,
                        retry_path = "trap",
                        retry_policy_source = "host-override",
                        retry_decision = "retry",
                        delay_ms = delay.as_millis() as u64,
                        trap = ?trap_type,
                        "Semantic trap retry: delaying (override carried from host call)"
                    );
                    RetryDecision::Delayed(*delay)
                }
                crate::durable_host::durability::SemanticTrapRetryVerdict::GiveUp => {
                    debug!(
                        retry_policy = %override_.policy_name,
                        retry_path = "trap",
                        retry_policy_source = "host-override",
                        retry_decision = "give-up",
                        trap = ?trap_type,
                        "Semantic trap retry: exhausted (override carried from host call)"
                    );
                    RetryDecision::None
                }
            };
            return (decision, Some(override_.retry_policy_state.clone()));
        }

        // (A) — guest-originated trap, or host-originated trap whose
        // `try_trigger_retry` did not produce an override (e.g. eval-error
        // fallthrough). Build a `RetryContext::trap` and resolve through the
        // named retry policies. The synthesized default-from-config has
        // `Predicate::True` so resolution is guaranteed to find a match for
        // any properties this branch produces.
        let named_retry_policies = self.named_retry_policies().await;

        let mut properties = RetryContext::trap(
            Self::semantic_trap_type_name(error),
            Some(full_function_name),
        );
        self.state.enrich_retry_properties(&mut properties);

        // Status-code-keyed user policies are deliberately skipped here (no
        // `status-code` is present in trap context). The synthesized default
        // policy then provides the fallback.
        let named_policy = match golem_common::model::NamedRetryPolicy::resolve_applicable_treating_missing_properties_as_no_match(
            &named_retry_policies,
            &properties,
        ) {
            Ok(Some(named_policy)) => named_policy,
            Ok(None) => {
                warn!(
                    trap = ?trap_type,
                    "No named retry policy matched the trap context (including the synthesized default); giving up"
                );
                return (RetryDecision::None, None);
            }
            Err(error) => {
                warn!(
                    ?error,
                    trap = ?trap_type,
                    "Failed resolving semantic trap retry policy; giving up"
                );
                return (RetryDecision::None, None);
            }
        };

        let current_state = retry_state_with_current_attempt.get(retry_from).cloned();
        let total_attempts_with_current = current_state
            .as_ref()
            .map(|s| s.retry_count())
            .unwrap_or_default();
        let total_attempts_before_current = total_attempts_with_current.saturating_sub(1);

        match evaluate_named_policy_step_resetting_on_invalid_state(
            named_policy,
            &properties,
            current_state.as_ref(),
        ) {
            Ok((new_state, RetryVerdict::Retry(delay))) => {
                debug!(
                    retry_policy = %named_policy.name,
                    retry_path = "trap",
                    retry_policy_source = "worker-local",
                    retry_decision = "retry",
                    delay_ms = delay.as_millis() as u64,
                    attempt = total_attempts_before_current + 1,
                    trap = ?trap_type,
                    "Semantic trap retry: delaying"
                );
                (RetryDecision::Delayed(delay), Some(new_state))
            }
            Ok((new_state, RetryVerdict::GiveUp)) => {
                debug!(
                    retry_policy = %named_policy.name,
                    retry_path = "trap",
                    retry_policy_source = "worker-local",
                    retry_decision = "give-up",
                    attempt = total_attempts_before_current + 1,
                    trap = ?trap_type,
                    "Semantic trap retry: exhausted"
                );
                (RetryDecision::None, Some(new_state.exhausted()))
            }
            Ok((_new_state, RetryVerdict::Error(error))) => {
                warn!(
                    retry_policy = %named_policy.name,
                    ?error,
                    retry_path = "trap",
                    fallback_reason = "eval-error",
                    trap = ?trap_type,
                    "Semantic trap retry policy evaluation returned an error verdict; giving up"
                );
                (RetryDecision::None, None)
            }
            Err(error) => {
                warn!(
                    retry_policy = %named_policy.name,
                    ?error,
                    retry_path = "trap",
                    fallback_reason = "eval-error",
                    trap = ?trap_type,
                    "Failed evaluating semantic trap retry policy; giving up"
                );
                (RetryDecision::None, None)
            }
        }
    }

    /// Returns `Ok(())` if the host is in normal strictness mode, or if the host is in read-only
    /// strictness but the call site has not been restricted (this function is a no-op in normal
    /// mode).
    ///
    /// Returns `Err(GolemSpecificWasmTrap::WorkerReadOnlyViolation)` if the host is in read-only
    /// strictness mode. The error carries the agent method name and the host function name, so
    /// the trap can later be converted to a typed `AgentError::ReadOnlyViolation`.
    ///
    /// Call sites should invoke this at the very top of any host function that introduces a
    /// remote side effect (outgoing HTTP, RPC) before any durability machinery runs.
    pub fn check_read_only_allows(&self, host_function: &str) -> Result<(), GolemSpecificWasmTrap> {
        if self.state.invocation_strictness == InvocationStrictness::ReadOnly {
            let method = self.state.read_only_method_name.clone().unwrap_or_default();
            Err(GolemSpecificWasmTrap::WorkerReadOnlyViolation {
                method,
                host_function: host_function.to_string(),
            })
        } else {
            Ok(())
        }
    }

    /// Returns the current invocation strictness mode.
    pub fn invocation_strictness(&self) -> InvocationStrictness {
        self.state.invocation_strictness
    }

    pub(crate) fn enter_operator_authorized_oplog_processor_invocation(
        &mut self,
    ) -> OperatorAuthorizedOplogProcessorInvocationGuard {
        OperatorAuthorizedOplogProcessorInvocationGuard::enter(
            &self.state.operator_authorized_oplog_processor_invocation,
        )
    }

    pub(crate) fn operator_authorizes_current_invocation(&self) -> bool {
        self.state
            .operator_authorized_oplog_processor_invocation
            .load(Ordering::Acquire)
    }
}

#[async_trait]
impl<Ctx: WorkerCtx> InvocationManagement for DurableWorkerCtx<Ctx> {
    async fn set_current_idempotency_key(&mut self, key: IdempotencyKey) {
        self.state.set_current_idempotency_key(key)
    }

    async fn get_current_idempotency_key(&self) -> Option<IdempotencyKey> {
        self.state.get_current_idempotency_key()
    }

    async fn set_current_invocation_context(
        &mut self,
        invocation_context: InvocationContextStack,
    ) -> Result<(), WorkerExecutorError> {
        let invocation_context = invocation_context
            .limit_depth(self.state.config.limits.max_invocation_context_stack_depth);
        let (invocation_context, current_span_id) =
            InvocationContext::from_stack(invocation_context)
                .map_err(WorkerExecutorError::runtime)?;

        self.state.invocation_context.switch_to(invocation_context);
        self.state.current_span_id = current_span_id;

        Ok(())
    }

    async fn get_current_invocation_context(&self) -> InvocationContextStack {
        self.state
            .invocation_context
            .get_stack(&self.state.current_span_id)
            .unwrap()
            .limit_depth(self.state.config.limits.max_invocation_context_stack_depth)
    }

    fn is_live(&self) -> bool {
        self.state.is_live()
    }

    fn is_replay(&self) -> bool {
        self.state.is_replay()
    }
}

#[async_trait]
impl<Ctx: WorkerCtx> StatusManagement for DurableWorkerCtx<Ctx> {
    fn check_interrupt(&self) -> Option<InterruptKind> {
        {
            let execution_status = self.execution_status.read().unwrap();
            if let ExecutionStatus::Interrupting { interrupt_kind, .. } = &*execution_status {
                return Some(*interrupt_kind);
            }
        }
        if self
            .owner_execution
            .tool_operations()
            .interruptible_owner_failure()
            .is_some()
        {
            return Some(InterruptKind::Interrupt(Timestamp::now_utc()));
        }
        if self
            .entity_cancellation
            .as_ref()
            .is_some_and(tokio_util::sync::CancellationToken::is_cancelled)
        {
            return Some(InterruptKind::EntityCancellation);
        }
        // An exceeded invocation or tail-work deadline surfaces as a synthetic interrupt so work
        // traps at the next epoch check. The corresponding invocation boundary converts the
        // unwind into the appropriate timeout failure.
        if self
            .state
            .invocation_deadline_exceeded
            .load(Ordering::Acquire)
            || self
                .state
                .tail_work_deadline_exceeded
                .load(Ordering::Acquire)
        {
            return Some(InterruptKind::Interrupt(Timestamp::now_utc()));
        }
        None
    }

    fn set_suspended(&self) {
        let mut execution_status = self.execution_status.write().unwrap();
        let current_execution_status = execution_status.clone();
        match current_execution_status {
            ExecutionStatus::Running { agent_mode, .. } => {
                *execution_status = ExecutionStatus::Suspended {
                    agent_mode,
                    timestamp: Timestamp::now_utc(),
                };
            }
            ExecutionStatus::Suspended { .. } => {}
            ExecutionStatus::Interrupting {
                agent_mode,
                await_interruption,
                ..
            } => {
                *execution_status = ExecutionStatus::Suspended {
                    agent_mode,
                    timestamp: Timestamp::now_utc(),
                };
                await_interruption.send(()).ok();
            }
            ExecutionStatus::Loading { agent_mode, .. } => {
                *execution_status = ExecutionStatus::Suspended {
                    agent_mode,
                    timestamp: Timestamp::now_utc(),
                };
            }
        };
    }

    fn set_running(&self) {
        let mut execution_status = self.execution_status.write().unwrap();
        let current_execution_status = execution_status.clone();
        match current_execution_status {
            ExecutionStatus::Running { .. } => {}
            ExecutionStatus::Suspended { agent_mode, .. } => {
                let (tx, _) = tokio::sync::broadcast::channel(128);
                let interrupt_signal = Arc::new(tx);
                *execution_status = ExecutionStatus::Running {
                    agent_mode,
                    timestamp: Timestamp::now_utc(),
                    interrupt_signal,
                };
            }
            ExecutionStatus::Interrupting { .. } => {}
            ExecutionStatus::Loading {
                agent_mode,
                interrupt_signal,
                ..
            } => {
                *execution_status = ExecutionStatus::Running {
                    agent_mode,
                    timestamp: Timestamp::now_utc(),
                    interrupt_signal,
                };
            }
        }
    }
}

impl<Ctx: WorkerCtx> DurableWorkerCtx<Ctx> {
    /// Ends an invocation whose failure, or the status after it, the oplog refused to record.
    async fn refused_terminal_entry(
        &mut self,
        error: &OplogError,
        trap_type: &TrapType,
        full_function_name: &str,
    ) -> InvocationFailureDecision {
        match terminal_entry_refusal(error) {
            // The storage did not answer: the entry can be stored, but the status after it does
            // not read. The invocation stays pending, and the agent recovers from its oplog when
            // the storage answers again.
            TerminalEntryRefusal::RecoveryRequired(error) => {
                InvocationFailureDecision::Infrastructure(error)
            }
            // The oplog failed for good, so the invocation fails here with the trap, and the
            // agent stops with the error, which reaches the other waiting callers.
            TerminalEntryRefusal::OplogFailed(error) => {
                if let Some(idempotency_key) = self.state.get_current_idempotency_key() {
                    self.public_state
                        .worker()
                        .store_invocation_failure(&idempotency_key, trap_type)
                        .await;
                    self.public_state.event_service().emit_invocation_finished(
                        full_function_name,
                        &idempotency_key,
                        self.is_live(),
                    );
                }
                InvocationFailureDecision::Infrastructure(error)
            }
            // Refused by a fence, the shard is lost: no failure is published for its invocation,
            // which the shard's new owner runs.
            TerminalEntryRefusal::Unpublished => {
                InvocationFailureDecision::Retry(RetryDecision::None)
            }
        }
    }
}

#[async_trait]
impl<Ctx: WorkerCtx> InvocationHooks for DurableWorkerCtx<Ctx> {
    async fn on_agent_invocation_started(
        &mut self,
        hydrated: crate::worker::HydratedInvocation,
    ) -> Result<(), WorkerExecutorError> {
        let crate::worker::HydratedInvocation {
            invocation,
            payload,
        } = hydrated;
        if !self.state.durability_is_suppressed() {
            let stack = self.get_current_invocation_context().await;

            let scope_card = match &invocation {
                AgentInvocation::AgentMethod { scope_card, .. }
                | AgentInvocation::ExternalTool { scope_card, .. } => scope_card.clone(),
                _ => None,
            };
            let scope_interest = scope_card.as_ref().map(|scope| {
                self.state
                    .owner_wallet
                    .interest
                    .contribute(None, &scope.root_card_ids)
            });
            let scope_root_cards = if let Some(scope_card) = &scope_card {
                crate::services::card::validate_scope_card(
                    self.state.card_service.as_ref(),
                    scope_card,
                )
                .await?
            } else {
                Vec::new()
            };
            self.install_invocation_scope_card(
                scope_card.clone(),
                scope_root_cards,
                scope_interest,
            )
            .await;

            let input = match &invocation {
                AgentInvocation::AgentInitialization { input, .. }
                | AgentInvocation::AgentMethod { input, .. } => Some(input),
                AgentInvocation::ExternalTool { input, .. } => Some(input.value()),
                _ => None,
            };
            if let Some(input) = input
                && !self.secret_holds_allowed_for_value(input).await?
            {
                self.clear_invocation_scope_card().await;
                return Err(WorkerExecutorError::permission_denied("permission denied"));
            }

            let (idempotency_key, _, _) = invocation.into_parts();
            let _boundary = self.lock_synchronized_card_event_boundary().await?;
            let wallet = self.state.owner_wallet.snapshot();
            let start_index = self
                .public_state
                .worker()
                .oplog()
                .add_agent_invocation_started_from_pending(
                    idempotency_key,
                    payload,
                    stack,
                    InvocationWalletPin {
                        wallet_token: WalletVersionToken {
                            wallet_id_hash: wallet.wallet_id_hash,
                            generation: wallet.generation,
                        },
                        pinned_card_ids: wallet.cards.keys().copied().collect(),
                        scope_card_id: scope_card.map(|card| card.scope_card_id),
                    },
                )
                .await
                .map_err(|err| match err.into_refusal() {
                    Ok(refusal) => self.public_state.worker().refused_by(refusal),
                    Err(err) => panic!(
                        "could not encode agent invocation on {}: {err}",
                        self.agent_id()
                    ),
                })?;
            self.primary_invocation_start_index = Some(start_index);

            self.public_state
                .worker()
                .commit_oplog_and_update_state(CommitLevel::Always)
                .await?;
        }
        Ok(())
    }

    async fn on_agent_invocation_finished(&mut self) {
        self.clear_invocation_scope_card().await;
    }

    async fn on_invocation_failure(
        &mut self,
        full_function_name: &str,
        trap_type: &TrapType,
    ) -> InvocationFailureDecision {
        let current_idempotency_key = self.get_current_idempotency_key().await;

        // Deliberately above the dropped-call drain: that drain appends `Cancelled` entries, and a
        // lost shard's oplog belongs to another executor now. Like a durable `Restart` or `Jump`, a
        // `ShardLost` trap records nothing - not the drain, not an entry, not a status change - and
        // the shard's new owner replays the invocation and records its outcome itself.
        if let TrapType::Interrupt(kind) = trap_type
            && self
                .public_state
                .worker()
                .retire_if_shard_lost(&WorkerExecutorError::Interrupted { kind: *kind })
        {
            return InvocationFailureDecision::Retry(RetryDecision::None);
        }

        let terminal_trap = ephemeral_terminal_trap(
            self.agent_mode(),
            trap_type,
            self.invocation_anchor()
                .unwrap_or(self.state.effective_retry_point()),
        );
        let trap_type = terminal_trap.as_ref().unwrap_or(trap_type);

        // The abandoned runtime is torn down structurally by the owner loop. Do not perform the
        // ordinary semantic dropped-call close, which could await replay scopes or append
        // terminals into the suffix the accepted cut is about to erase.
        if matches!(trap_type, TrapType::Interrupt(InterruptKind::Jump)) {
            return InvocationFailureDecision::Retry(RetryDecision::Immediate);
        }

        if self.state.is_live()
            && !self.state.snapshotting_mode
            && let Err(err) = concurrent::drain_queued_dropped_call_events(self).await
        {
            error!(
                error = %err,
                "Failed to drain dropped durable calls before the invocation failure entry"
            );
            // A `Cancelled` refused by a fence latched during the drain gives the agent up, so
            // the stop that follows drops this generation instead of leaving it cached to be
            // restarted in place.
            if self.public_state.worker().retire_if_shard_lost(&err.source) {
                return InvocationFailureDecision::Retry(RetryDecision::None);
            }
            // A `Cancelled` the oplog refused: the agent recovers, or stops with the error.
            return match err.source {
                error @ (WorkerExecutorError::RecoveryRequired { .. }
                | WorkerExecutorError::OplogFailed { .. }) => {
                    InvocationFailureDecision::Infrastructure(error)
                }
                _ => InvocationFailureDecision::Retry(RetryDecision::None),
            };
        }

        if let TrapType::Interrupt(kind) = trap_type {
            self.public_state
                .worker()
                .acknowledge_invocation_interrupt(*kind)
                .await;
        }
        let retry_from = self
            .invocation_anchor()
            .unwrap_or(self.state.effective_retry_point());
        let monthly_trap = self
            .public_state
            .worker()
            .monthly_failure(trap_type, retry_from);
        let trap_type = monthly_trap.as_ref().unwrap_or(trap_type);
        let suspend_trap = ephemeral_suspend_trap(self.agent_mode(), trap_type, retry_from);
        let trap_type = suspend_trap.as_ref().unwrap_or(trap_type);

        if let TrapType::Error { error, .. } = trap_type {
            match error {
                AgentError::EphemeralSleepTooLong(_) => {
                    record_non_suspending_failure("sleep-too-long")
                }
                AgentError::EphemeralFuelExhausted(_) => {
                    record_non_suspending_failure("fuel-exhausted")
                }
                AgentError::EphemeralCannotSuspend(_) => {
                    record_non_suspending_failure("cannot-suspend")
                }
                _ => {}
            }
        }

        let latest_status_before = self.public_state.worker().get_last_known_status().await;
        let (decision, retry_policy_state) = self
            .get_recovery_decision_on_trap_with_semantic(
                &latest_status_before.current_retry_state,
                trap_type,
                full_function_name,
            )
            .await;

        let terminal_decision = ephemeral_terminal_decision(self.agent_mode(), decision.clone());
        let retry_policy_state = if terminal_decision != decision {
            Some(
                retry_policy_state
                    .unwrap_or(RetryPolicyState::Terminal)
                    .exhausted(),
            )
        } else {
            retry_policy_state
        };
        let decision = terminal_decision;

        let entity_parent_start_index = self.entity_parent_start_index();
        let permission_denial_persisted = if let (
            Some(idempotency_key),
            TrapType::Error {
                error: AgentError::PermissionDenied(error),
                retry_from,
                ..
            },
        ) = (&current_idempotency_key, trap_type)
        {
            let denial_persisted = self
                .state
                .oplog
                .add_pair(
                    OplogEntry::cancel_pending_invocation(idempotency_key.clone()),
                    Box::new({
                        let error = error.clone();
                        let retry_policy_state = retry_policy_state.clone();
                        let retry_from = *retry_from;
                        move |_| {
                            OplogEntry::error(
                                entity_parent_start_index,
                                OplogErrorKind::Invocation,
                                AgentError::PermissionDenied(error),
                                retry_from,
                                retry_policy_state,
                            )
                        }
                    }),
                )
                .await;
            if let Err(error) = denial_persisted {
                if let OplogError::Fenced(fence) = error {
                    // The shard moved while this failure was being recorded. The fence retires the
                    // agent exactly as the `ShardLost` arm above does: nothing further may be
                    // written to an oplog that belongs to another executor now.
                    self.public_state
                        .worker()
                        .refused_by(crate::services::oplog::OplogRefusal::Fenced(fence));
                    return InvocationFailureDecision::Retry(RetryDecision::None);
                }
                if matches!(error, OplogError::Payload(_) | OplogError::Maintenance(_)) {
                    panic!("oplog write: {error}");
                }
                return self
                    .refused_terminal_entry(&error, trap_type, full_function_name)
                    .await;
            }
            // Refused, like the add above.
            if let Err(error) = self
                .public_state
                .worker()
                .commit_oplog_and_update_state(CommitLevel::Always)
                .await
            {
                return self
                    .refused_terminal_entry(&error, trap_type, full_function_name)
                    .await;
            }
            true
        } else {
            false
        };

        let oplog_entry = match trap_type {
            TrapType::Interrupt(InterruptKind::EntityCancellation) => {
                unreachable!("entity cancellation must settle at its invocation boundary")
            }
            TrapType::Interrupt(InterruptKind::Interrupt(_)) => Some(OplogEntry::interrupted()),
            TrapType::Interrupt(InterruptKind::Suspend(_)) => Some(OplogEntry::suspend()),
            TrapType::Interrupt(InterruptKind::Jump) => None,
            TrapType::Interrupt(InterruptKind::Restart) => None,
            // The oplog is the new owner's; a stale writer must leave no trace in it.
            TrapType::Interrupt(InterruptKind::ShardLost) => None,
            TrapType::Exit => Some(OplogEntry::exited()),
            TrapType::Error {
                error: AgentError::PermissionDenied(_),
                ..
            } if permission_denial_persisted => None,
            TrapType::Error { .. } if trap_type.is_invocation_rejection() => {
                current_idempotency_key.map(OplogEntry::cancel_pending_invocation)
            }
            TrapType::Error {
                error, retry_from, ..
            } => Some(OplogEntry::error(
                entity_parent_start_index,
                OplogErrorKind::Invocation,
                error.clone(),
                *retry_from,
                retry_policy_state,
            )),
        };

        if let Some(entry) = oplog_entry
            && let Err(error) = self.public_state.worker().add_and_commit_oplog(entry).await
        {
            return self
                .refused_terminal_entry(&error, trap_type, full_function_name)
                .await;
        };

        let latest_status = self.public_state.worker().get_last_known_status().await;

        let giving_up = trap_type.is_invocation_rejection()
            || matches!(
                latest_status.status,
                AgentStatus::Interrupted | AgentStatus::Exited
            )
            || decision == RetryDecision::None;

        if giving_up {
            // Terminal worker failures also fail queued invocations. A rejected request completes
            // only its own idempotency key so later queued work remains available.
            if let Some(idempotency_key) = self.state.get_current_idempotency_key() {
                self.public_state
                    .worker()
                    .store_invocation_failure(&idempotency_key, trap_type)
                    .await;

                self.public_state.event_service().emit_invocation_finished(
                    full_function_name,
                    &idempotency_key,
                    self.is_live()
                        || self
                            .state
                            .authoritative_replay_failure
                            .load(Ordering::Acquire),
                );
            }
        }

        debug!(
            "Recovery decision for {trap_type:?} with {:?} retries: {:?}",
            latest_status_before.current_retry_state, decision
        );

        InvocationFailureDecision::Retry(decision)
    }

    fn enter_read_only_mode(&mut self, method_name: String) {
        if self.state.invocation_strictness == InvocationStrictness::ReadOnly {
            warn!(
                "enter_read_only_mode called while already in read-only mode (current method: {:?}, new method: {})",
                self.state.read_only_method_name, method_name
            );
        }
        self.state.invocation_strictness = InvocationStrictness::ReadOnly;
        self.state.read_only_method_name = Some(method_name);
    }

    fn exit_read_only_mode(&mut self) {
        match self.state.invocation_strictness {
            InvocationStrictness::ReadOnly => {
                self.state.invocation_strictness = InvocationStrictness::Normal;
                self.state.read_only_method_name = None;
            }
            InvocationStrictness::Normal => {
                warn!(
                    "exit_read_only_mode called without a matching enter_read_only_mode; \
                     invocation strictness left as Normal"
                );
            }
        }
    }

    async fn on_agent_invocation_success(
        &mut self,
        full_function_name: &str,
        consumed_fuel: u64,
        output: &mut AgentInvocationOutput,
    ) -> Result<(), WorkerExecutorError> {
        let mut is_live = self.state.is_live();
        if is_live && !self.state.snapshotting_mode {
            tool::durability::finish_producer_disposals(self).await?;
        }
        if !self.state.active_custom_invocations.is_empty()
            || !self.state.custom_invocation_scopes.is_empty()
        {
            self.cleanup_custom_durability_state();
            return Err(WorkerExecutorError::runtime(
                "agent invocation returned while a custom durable invocation ownership scope was still open",
            ));
        }

        if !is_live {
            // Mirror the live-path drain: events enqueued from synchronous drops during replay
            // (e.g. cleanup for a p3 HTTP response dropped unconsumed) must settle
            // their durable operations before the
            // `AgentInvocationFinished` entry is read.
            concurrent::drain_queued_dropped_call_events(self)
                .await
                .map_err(|err| err.source)?;

            let response = self
                .state
                .replay_state
                .get_oplog_entry_agent_invocation_finished()
                .await?;
            if let Some(recorded_result) = response {
                if !recorded_result.replay_equivalent(&output.result) {
                    return Err(WorkerExecutorError::unexpected_oplog_entry(
                        format!(
                            "{full_function_name} => {:?}",
                            recorded_result.redacted_debug()
                        ),
                        format!(
                            "{full_function_name} => {:?}",
                            output.result.redacted_debug()
                        ),
                    ));
                }
                debug!("Function {full_function_name} finished");
                return Ok(());
            }

            // An incomplete historical invocation can finish after a non-primary entity repair
            // has moved the shared cursor into Settling. Complete the primary transition before
            // publishing this newly repaired invocation as a live success.
            self.switch_to_live().await?;
            is_live = true;
            tool::durability::finish_producer_disposals(self).await?;
        }

        if is_live && !self.state.durability_is_suppressed() {
            if self.state.entity_execution_mode.is_none() {
                self.state
                    .replay_state
                    .release_retained_starts_at_live_invocation_end()
                    .await?;
            }
            let component_revision = output.component_revision.ok_or_else(|| {
                WorkerExecutorError::runtime(
                    "component_revision missing in AgentInvocationOutput during replay",
                )
            })?;

            // Classify the just-completed invocation up front so we can
            // bump the read-only cache epoch on successful mutating
            // completion. For non-AgentMethod results
            // (initialization, manual update, snapshot
            // load/save, oplog processing) we always invalidate — these
            // are all state-changing. For AgentMethod results we ask the
            // worker whether the method is `read_only`.
            let invalidates_read_only_cache = match &output.result {
                AgentInvocationResult::AgentMethod { .. } => self
                    .public_state
                    .worker()
                    .agent_method_invalidates_read_only_cache(full_function_name),
                AgentInvocationResult::AgentInitialization
                | AgentInvocationResult::ManualUpdate
                | AgentInvocationResult::LoadSnapshot { .. }
                | AgentInvocationResult::SaveSnapshot { .. }
                | AgentInvocationResult::ProcessOplogEntries { .. }
                | AgentInvocationResult::ExternalTool { .. } => true,
            };

            // Only `AgentMethod` results need the method name persisted so the
            // public oplog renderer can resolve the correct output schema.
            let method_name = match &output.result {
                AgentInvocationResult::AgentMethod { .. } => Some(full_function_name.to_string()),
                _ => None,
            };

            let finished_index = self
                .public_state
                .worker()
                .oplog()
                .add_agent_invocation_finished(
                    &output.result,
                    method_name,
                    consumed_fuel,
                    component_revision,
                )
                .await
                .map_err(|err| match err.into_refusal() {
                    Ok(refusal) => self.public_state.worker().refused_by(refusal),
                    Err(err) => {
                        panic!("could not encode function result for {full_function_name}: {err}")
                    }
                })?;

            let commit_level = match self.public_state.worker().agent_mode() {
                AgentMode::Durable => CommitLevel::Always,
                AgentMode::Ephemeral => CommitLevel::Deferred,
            };
            self.public_state
                .worker()
                .commit_oplog_before_status_update(commit_level)
                .await?;

            // Bump the read-only cache epoch after the
            // `AgentInvocationFinished` entry is committed, but *before*
            // we publish `InvocationCompleted` to waiters via
            // `store_invocation_success`. Ordering matters: any client
            // that observes the completion event must also see an
            // invalidated cache, otherwise it could read a stale cached
            // result for the now-mutated state.
            if invalidates_read_only_cache {
                self.public_state.worker().bump_read_only_cache_epoch();
            }

            // Capture the agent's oplog index right after
            // `AgentInvocationFinished` was committed, together with the
            // worker's per-instance fingerprint, so the response carries
            // an unambiguous identification of the agent state it was
            // produced from.
            output.oplog_index = Some(finished_index);
            output.agent_fingerprint = Some(
                self.public_state
                    .worker()
                    .get_initial_worker_metadata()
                    .fingerprint,
            );

            if let Some(idempotency_key) = self.state.get_current_idempotency_key() {
                self.public_state
                    .worker()
                    .store_invocation_success(&idempotency_key, output.clone())
                    .await;

                self.public_state.event_service().emit_invocation_finished(
                    full_function_name,
                    &idempotency_key,
                    is_live,
                );
            }
        }
        debug!("Function {full_function_name} finished");

        Ok(())
    }

    async fn get_current_retry_point(&self) -> OplogIndex {
        self.state.effective_retry_point()
    }
}

#[async_trait]
impl<Ctx: WorkerCtx> InvocationContextManagement for DurableWorkerCtx<Ctx> {
    fn remove_span(&mut self, span_id: &SpanId) -> Result<(), WorkerExecutorError> {
        if &self.state.current_span_id == span_id {
            // Walk up to the parent if it still exists in the invocation context;
            // otherwise fall back to the root.
            let parent_id = self
                .state
                .invocation_context
                .get(span_id)
                .ok()
                .and_then(|span| span.parent().map(|p| p.span_id().clone()));

            self.state.current_span_id = parent_id
                .filter(|id| self.state.invocation_context.get(id).is_ok())
                .unwrap_or_else(|| self.state.invocation_context.root.span_id().clone());
        }
        let _ = self
            .state
            .invocation_context
            .finish_span(span_id)
            .map_err(WorkerExecutorError::runtime);
        Ok(())
    }

    fn clone_as_inherited_stack(&self, current_span_id: &SpanId) -> InvocationContextStack {
        self.state
            .invocation_context
            .clone_as_inherited_stack(current_span_id)
            .limit_depth(self.state.config.limits.max_invocation_context_stack_depth)
    }
}

impl OperatorAuthorizedOplogProcessorInvocationGuard {
    pub(in crate::durable_host) fn enter(active: &Arc<AtomicBool>) -> Self {
        active
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .expect("operator-authorized oplog-processor invocation mode cannot be nested");
        Self {
            active: active.clone(),
        }
    }
}

impl Drop for OperatorAuthorizedOplogProcessorInvocationGuard {
    fn drop(&mut self) {
        let was_active = self.active.swap(false, Ordering::AcqRel);
        debug_assert!(
            was_active,
            "operator-authorized oplog-processor invocation mode was not active"
        );
    }
}

pub(crate) fn terminal_entry_refusal(
    error: &crate::services::oplog::OplogError,
) -> TerminalEntryRefusal {
    match error.agent_failure() {
        Some(error @ WorkerExecutorError::RecoveryRequired { .. }) => {
            TerminalEntryRefusal::RecoveryRequired(error)
        }
        Some(error) => TerminalEntryRefusal::OplogFailed(error),
        None => TerminalEntryRefusal::Unpublished,
    }
}

pub(in crate::durable_host) fn ephemeral_terminal_trap(
    mode: AgentMode,
    trap_type: &TrapType,
    retry_from: OplogIndex,
) -> Option<TrapType> {
    match (mode, trap_type) {
        (
            AgentMode::Ephemeral,
            TrapType::Interrupt(kind @ (InterruptKind::Jump | InterruptKind::Restart)),
        ) => Some(TrapType::Error {
            error: AgentError::InternalError(format!(
                "ephemeral agent cannot be restarted in process ({kind:?})"
            )),
            retry_from,
            semantic_trap_retry_override: None,
        }),
        _ => None,
    }
}

pub(crate) fn ephemeral_suspend_trap(
    mode: AgentMode,
    trap_type: &TrapType,
    retry_from: OplogIndex,
) -> Option<TrapType> {
    match (mode, trap_type) {
        (AgentMode::Ephemeral, TrapType::Interrupt(InterruptKind::Suspend(_))) => {
            Some(TrapType::Error {
                error: AgentError::EphemeralCannotSuspend(EphemeralCannotSuspendError {
                    reason: "suspended while an invocation was active".to_string(),
                }),
                retry_from,
                semantic_trap_retry_override: None,
            })
        }
        _ => None,
    }
}

pub(in crate::durable_host) fn ephemeral_terminal_decision(
    mode: AgentMode,
    decision: RetryDecision,
) -> RetryDecision {
    match (mode, decision) {
        (
            AgentMode::Ephemeral,
            RetryDecision::Immediate | RetryDecision::Delayed(_) | RetryDecision::ReacquirePermits,
        ) => RetryDecision::None,
        (_, decision) => decision,
    }
}
