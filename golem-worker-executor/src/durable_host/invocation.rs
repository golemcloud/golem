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
use golem_common::schema::SchemaValue;

/// Reads the recorded terminal of the invocation the loop replays from its open invocation
/// anchor, while the background work of the invocation replays.
pub(crate) struct InvocationTerminalReader {
    replay_state: ReplayState,
    invocation_id: golem_common::model::InvocationId,
    runtime: Option<Arc<crate::worker::suspension::RuntimeStore>>,
}

impl InvocationTerminalReader {
    /// The recorded terminal of the invocation, or `None` at the replay tail.
    ///
    /// The read runs cursor operations and oplog reads outside the Store's runtime. Only while
    /// it waits for cursor progress, which only the owner's runtime makes, can the driver of
    /// the Store report a deadlock.
    pub(crate) async fn read(
        &self,
    ) -> Result<Option<(OplogIndex, OplogEntry)>, WorkerExecutorError> {
        let outside = self
            .runtime
            .as_ref()
            .map(|runtime| runtime.root_waits_outside());
        self.replay_state
            .get_oplog_entry_agent_invocation_terminal_in_open_anchor(&self.invocation_id, || {
                outside.as_ref().map(|outside| outside.parked())
            })
            .await
    }
}

impl<Ctx: WorkerCtx> DurableWorkerCtx<Ctx> {
    /// Opens the invocation anchor at `anchor` for the invocation `invocation_id` the loop
    /// replays, which records its result at the export's return and drains its background work
    /// after it. The anchor reads the invocation's return signal.
    pub(crate) async fn open_invocation_anchor(
        &mut self,
        anchor: OplogIndex,
        invocation_id: golem_common::model::InvocationId,
    ) -> Result<(), WorkerExecutorError> {
        let export_returned = self
            .attribution
            .export_returned(attribution::InvocationKey::Invocation(invocation_id));
        self.state
            .replay_state
            .open_invocation_anchor(anchor, export_returned)
            .await?;
        self.state.open_invocation_anchor = Some(anchor);
        Ok(())
    }

    /// The reader of the recorded terminal of the invocation the loop replays, when the Store
    /// replays and the invocation has an open invocation anchor.
    pub(crate) fn invocation_terminal_reader(&self) -> Option<InvocationTerminalReader> {
        if self.state.is_live() || self.state.open_invocation_anchor.is_none() {
            return None;
        }
        Some(InvocationTerminalReader {
            replay_state: self.state.replay_state.clone(),
            invocation_id: self.state.get_current_idempotency_key()?,
            runtime: self.runtime_suspension.clone(),
        })
    }

    /// Keeps the terminal that replay consumed when the export of the invocation returned, for
    /// the result reader of the invocation.
    pub(crate) fn stash_replayed_invocation_terminal(
        &mut self,
        terminal: Result<(OplogIndex, OplogEntry), WorkerExecutorError>,
    ) {
        self.state.replayed_invocation_terminal = Some(terminal);
    }

    /// Applies the authority cut-off of `invocation` once its export returned: its scope card
    /// stops bounding the calls it makes from then on, and stays installed, hidden, for the calls
    /// it made before. The Store's call hook applies it before every host call, and the
    /// invocation loop when the export call's future resolves. Returns whether the cut-off
    /// applies.
    pub(crate) fn apply_return_cut_off(&mut self, invocation: attribution::InvocationKey) -> bool {
        if invocation == attribution::InvocationKey::AgentLevel
            || !self.attribution.export_returned_is_marked(invocation)
        {
            return false;
        }
        if let Some(entry) = self.state.invocations.own_mut(Some(invocation))
            && !entry.authority.final_outcome_reached()
        {
            entry.authority.reach_final_outcome();
            self.rederive_agent_effective_surface_from_wallet();
        }
        true
    }

    /// Called from the Store's call hook before every host call: applies the cut-off of
    /// `invocation`, the invocation the call is attributed to, and returns whether the call is
    /// made before that cut-off. Such a call keeps the invocation's scope card while its own host
    /// code continues after the cut-off.
    pub(crate) fn host_call_made_before_cut_off(
        &mut self,
        invocation: Option<attribution::InvocationKey>,
    ) -> bool {
        invocation.is_some_and(|invocation| {
            invocation != attribution::InvocationKey::AgentLevel
                && !self.apply_return_cut_off(invocation)
        })
    }

    /// Whether replay stashed the terminal of the invocation, or the failure of reading it, for
    /// the result reader of the invocation.
    pub(crate) fn has_replayed_invocation_terminal(&self) -> bool {
        self.state.replayed_invocation_terminal.is_some()
    }

    /// Takes the terminal that replay stashed for the result reader of the invocation.
    pub(crate) fn take_replayed_invocation_terminal(
        &mut self,
    ) -> Option<Result<(OplogIndex, OplogEntry), WorkerExecutorError>> {
        self.state.replayed_invocation_terminal.take()
    }

    /// Marks that the loop's invocation recorded its result at the export's return. A later
    /// failure of the invocation's background work is then an ordinary trap of the agent: it
    /// neither replaces nor publishes the recorded result.
    pub(crate) fn mark_result_recorded_at_return(&mut self) {
        self.state.result_recorded_at_return = true;
    }

    /// Whether the loop's invocation recorded its result at the export's return.
    pub(crate) fn result_recorded_at_return(&self) -> bool {
        self.state.result_recorded_at_return
    }

    /// Ends an invocation whose result was recorded at the export's return, after its
    /// background work drained, with `consumed_fuel` the fuel of the drain.
    ///
    /// A replayed invocation closes its invocation anchor. A recorded `AgentInvocationSettled`
    /// ends it. At the replay tail the Store switches to live and ends it as a live invocation
    /// does: the producers are disposed, the open custom durable scopes are checked, the
    /// retained `Start`s are released, and `AgentInvocationSettled` is appended and committed at
    /// the level of `AgentInvocationFinished`. A live invocation of a durable agent whose next
    /// work is a due queued invocation leaves the commit to the `AgentInvocationStarted` of that
    /// invocation ([`crate::worker::Worker::defer_settlement_commit`]).
    pub(crate) async fn on_agent_invocation_settled(
        &mut self,
        consumed_fuel: u64,
    ) -> Result<(), WorkerExecutorError> {
        self.state.replayed_invocation_terminal = None;
        let invocation_id = self.state.get_current_idempotency_key().ok_or_else(|| {
            WorkerExecutorError::runtime("agent invocation settled without a current invocation")
        })?;
        let invocation = self.invocation_key();
        let mut live_from_start = true;
        if let Some(anchor) = self.state.open_invocation_anchor.take() {
            live_from_start = false;
            match self
                .state
                .replay_state
                .close_invocation_anchor(anchor, &invocation_id)
                .await?
            {
                replay_state::SettledAt::Recorded(_) => {
                    self.state.close_loop_invocation();
                    return self.close_custom_durability_state(invocation);
                }
                replay_state::SettledAt::ReplayTail => {
                    if !self.state.is_live() {
                        self.switch_to_live().await?;
                    }
                }
            }
        } else if !self.state.is_live() {
            return Err(WorkerExecutorError::runtime(
                "agent invocation settled in replay without an open invocation anchor",
            ));
        }

        tool::durability::finish_producer_disposals(self).await?;
        self.close_custom_durability_state(invocation)?;
        if matches!(self.state.store_replay_state, StoreReplayState::Primary) {
            self.state
                .replay_state
                .release_retained_starts_at_live_invocation_end()
                .await?;
        }
        let worker = self.public_state.worker();
        worker
            .oplog()
            .add_agent_invocation_settled(invocation_id, consumed_fuel)
            .await
            .map_err(|error| match error.into_refusal() {
                Ok(refusal) => worker.refused_by(refusal),
                Err(error) => error,
            })?;
        self.state.close_loop_invocation();
        let commit_level = match worker.agent_mode() {
            AgentMode::Durable => CommitLevel::Always,
            AgentMode::Ephemeral => CommitLevel::Deferred,
        };
        if commit_level == CommitLevel::Always
            && live_from_start
            && matches!(self.state.store_replay_state, StoreReplayState::Primary)
            && worker.next_work_is_a_due_invocation()
        {
            worker.defer_settlement_commit();
            return Ok(());
        }
        worker
            .commit_oplog_before_status_update(commit_level)
            .await?;
        Ok(())
    }

    /// Fails when `invocation` still has an open custom durable invocation or ownership scope,
    /// after closing them.
    fn close_custom_durability_state(
        &mut self,
        invocation: attribution::InvocationKey,
    ) -> Result<(), WorkerExecutorError> {
        if self.has_open_custom_durability_state(invocation) {
            self.cleanup_custom_durability_state(invocation);
            return Err(WorkerExecutorError::runtime(
                "agent invocation returned while a custom durable invocation ownership scope was still open",
            ));
        }
        Ok(())
    }

    /// Settles a returned method error, including session work that must keep driving the Store.
    pub(crate) async fn on_agent_method_failure(
        store: &mut impl AsContextMut<Data = Ctx>,
        full_function_name: &str,
        error: AgentError,
        uses_streams: bool,
        recorded_at: ResultRecordedAt,
    ) -> Result<(), crate::worker::MethodFailureSettlementError> {
        use crate::worker::MethodFailureSettlementError;

        let mut store = store.as_context_mut();
        // A failure read from replay, also one that replay consumed at the export's return before
        // the Store switched to live, was settled by the run that recorded it.
        let replayed = store
            .data()
            .durable_ctx()
            .has_replayed_invocation_terminal();
        store
            .data_mut()
            .record_agent_method_failure(full_function_name, error.clone(), recorded_at)
            .await
            .map_err(MethodFailureSettlementError::Invocation)?;
        if replayed || !store.data().durable_ctx().is_live() {
            return Ok(());
        }

        let worker = store.data().durable_ctx().public_state.worker();
        if uses_streams && recorded_at != ResultRecordedAt::ExportReturn {
            let key = store
                .data()
                .durable_ctx()
                .state
                .get_current_idempotency_key()
                .expect("a settled method failure has an invocation key");
            let session_worker = worker.clone();
            store
                .as_context_mut()
                .run_concurrent(async move |_accessor| {
                    session_worker
                        .fail_durable_streaming_session(&key, error.message().to_owned())
                        .await
                })
                .await
                .map_err(|error| {
                    MethodFailureSettlementError::Recovery(WorkerExecutorError::runtime(
                        error.to_string(),
                    ))
                })?
                .map_err(MethodFailureSettlementError::Recovery)?;
        }
        worker.reset_infrastructure_recovery_backoff();
        Ok(())
    }

    /// Whether `invocation` still has an open custom durable invocation or ownership scope. A
    /// record whose owner's entry is gone belongs to the invocation the loop runs.
    fn has_open_custom_durability_state(&self, invocation: attribution::InvocationKey) -> bool {
        let invocations = &self.state.invocations;
        self.state
            .active_custom_invocations
            .values()
            .any(|active| invocations.key(Some(active.owner)) == invocation)
            || self
                .state
                .custom_invocation_scopes
                .values()
                .any(|scope| invocations.key(Some(scope.owner)) == invocation)
    }

    /// Closes the custom durable invocations and ownership scopes of `invocation`.
    fn cleanup_custom_durability_state(&mut self, invocation: attribution::InvocationKey) {
        let invocations = &self.state.invocations;
        let mut resources = Vec::new();
        self.state.custom_invocation_scopes.retain(|_, scope| {
            let owned = invocations.key(Some(scope.owner)) == invocation;
            if owned {
                resources.push(scope.resource_rep);
            }
            !owned
        });
        self.state
            .active_custom_invocations
            .retain(|_, active| invocations.key(Some(active.owner)) != invocation);
        for resource_rep in resources {
            let _ = self
                .table()
                .delete(Resource::<durability::LiveCustomDurableInvocation>::new_own(resource_rep));
        }
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
        let named_retry_policies = self.state.guest_trap_retry_policies().await;

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
        let state = self.state.invocation_state();
        if state.strictness == InvocationStrictness::ReadOnly {
            let method = state.read_only_method_name.clone().unwrap_or_default();
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
        self.state.invocation_state().strictness
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

    /// Whether a completed invocation with `result` can change the state of the agent. Every
    /// result except an `AgentMethod` one does: initialization, manual update, snapshot load and
    /// save, oplog processing and external tools. An agent method does unless it is `read_only`.
    fn invocation_changes_state(
        &self,
        full_function_name: &str,
        result: &AgentInvocationResult,
    ) -> bool {
        match result {
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
        }
    }
}

impl<Ctx: WorkerCtx> InvocationManagement for DurableWorkerCtx<Ctx> {
    async fn set_current_idempotency_key(&mut self, key: golem_common::model::InvocationId) {
        self.state.set_current_idempotency_key(key)
    }

    async fn get_current_idempotency_key(&self) -> Option<golem_common::model::InvocationId> {
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

        let state = self.state.loop_invocation_state_mut();
        state.invocation_context.switch_to(invocation_context);
        state.current_span_id = current_span_id;

        Ok(())
    }

    async fn get_current_invocation_context(&self) -> InvocationContextStack {
        self.state
            .invocation_state()
            .invocation_context
            .get_stack(&self.state.invocation_state().current_span_id)
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
    /// Closes the invocation the loop runs at its terminal when the terminal is not recorded at
    /// the export's return, as `on_agent_invocation_settled` does at `AgentInvocationSettled`: a
    /// parked store task then cannot resume under the finished invocation and record after its
    /// terminal, for example while a crash-interrupted invocation completes its durable Stream
    /// Session.
    fn close_loop_invocation_at_settled_terminal(&mut self, at_return: bool) {
        if !at_return {
            self.state.close_loop_invocation();
        }
    }

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
                if let Some(idempotency_key) = self
                    .state
                    .get_current_idempotency_key()
                    .filter(|_| !self.state.result_recorded_at_return)
                {
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

impl<Ctx: WorkerCtx> InvocationHooks for DurableWorkerCtx<Ctx> {
    async fn on_agent_invocation_started(
        &mut self,
        hydrated: crate::worker::HydratedInvocation,
        input: Option<&SchemaValue>,
    ) -> Result<bool, WorkerExecutorError> {
        let crate::worker::HydratedInvocation {
            invocation,
            operation,
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

            if let Some(input) = input
                && !self.secret_holds_allowed_for_value(input).await?
            {
                self.clear_invocation_scope_card().await;
                return Err(WorkerExecutorError::permission_denied("permission denied"));
            }

            let selected_preparation = self
                .public_state
                .worker()
                .selected_method_preparation(&invocation, input, &self.state.component_metadata)
                .await?;
            let (idempotency_key, _, _) = invocation.into_parts();
            let _boundary = self.lock_synchronized_card_event_boundary().await?;
            let wallet = self.state.owner_wallet.snapshot();
            let start_index = self
                .public_state
                .worker()
                .start_pending_invocation(
                    idempotency_key,
                    operation,
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
                    selected_preparation,
                )
                .await?;
            let Some(start_index) = start_index else {
                return Ok(false);
            };
            self.state.set_loop_invocation_start_index(start_index);
        }
        Ok(true)
    }

    async fn on_agent_invocation_finished(&mut self) {
        self.state
            .loop_invocation_state_mut()
            .authority
            .reach_final_outcome();
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

        // A failure of the next invocation before its start (admission, lowering, scope card)
        // decides with the retry state that the previous invocation's deferred
        // `AgentInvocationSettled` clears, so that entry is committed and folded first.
        if let Err(error) = self
            .public_state
            .worker()
            .commit_deferred_settlement()
            .await
        {
            return self
                .refused_terminal_entry(&error, trap_type, full_function_name)
                .await;
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

        let after_result = self.state.result_recorded_at_return;
        let retry_policy_state =
            drain_give_up_retry_state(after_result, &decision, retry_policy_state);
        let FailureAttribution {
            failed_invocation,
            rejection,
        } = failure_attribution(current_idempotency_key, trap_type, after_result);

        let entity_parent_start_index = self.entity_parent_start_index();
        // A rejected invocation is recorded as its cancellation followed by the rejection error,
        // so the status fold keeps the rejection as its result even when it never started.
        let rejection_persisted = if let (
            Some(idempotency_key),
            TrapType::Error {
                error, retry_from, ..
            },
        ) = (&failed_invocation, trap_type)
            && rejection
        {
            let rejection_recorded = self
                .state
                .oplog
                .add_pair(
                    OplogEntry::cancel_pending_invocation(*idempotency_key),
                    Box::new({
                        let error = error.clone();
                        let retry_policy_state = retry_policy_state.clone();
                        let retry_from = *retry_from;
                        let invocation_id = *idempotency_key;
                        move |_| {
                            OplogEntry::error(
                                entity_parent_start_index,
                                OplogErrorKind::Invocation,
                                Some(invocation_id),
                                error,
                                retry_from,
                                retry_policy_state,
                                None,
                                None,
                            )
                        }
                    }),
                )
                .await;
            if let Err(error) = rejection_recorded {
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
            TrapType::Error { .. } if rejection_persisted => None,
            TrapType::Error { .. } if rejection => {
                failed_invocation.map(OplogEntry::cancel_pending_invocation)
            }
            TrapType::Error {
                error, retry_from, ..
            } => Some(OplogEntry::error(
                entity_parent_start_index,
                OplogErrorKind::Invocation,
                failed_invocation,
                error.clone(),
                *retry_from,
                retry_policy_state,
                None,
                None,
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

        let giving_up = rejection
            || matches!(
                latest_status.status,
                AgentStatus::Interrupted | AgentStatus::Exited
            )
            || decision == RetryDecision::None;

        if giving_up {
            self.state
                .loop_invocation_state_mut()
                .authority
                .reach_final_outcome();
            self.rederive_agent_effective_surface_from_wallet();
            self.clear_invocation_scope_card().await;
            // Terminal worker failures also fail queued invocations. A rejected request completes
            // only its own idempotency key so later queued work remains available. An invocation
            // that recorded its result keeps it.
            if after_result {
                self.public_state
                    .worker()
                    .store_queued_invocation_failures(current_idempotency_key.as_ref(), trap_type)
                    .await;
            } else if let Some(idempotency_key) = self.state.get_current_idempotency_key() {
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

        if rejection_persisted {
            InvocationFailureDecision::Rejected
        } else {
            InvocationFailureDecision::Retry(decision)
        }
    }

    async fn record_agent_method_failure(
        &mut self,
        full_function_name: &str,
        error: AgentError,
        recorded_at: ResultRecordedAt,
    ) -> Result<(), WorkerExecutorError> {
        if self.state.durability_is_suppressed() {
            return Err(WorkerExecutorError::unexpected_oplog_entry(
                OplogIndex::NONE,
                "recorded agent method failure",
                "method failure while durability is suppressed",
            ));
        }
        // An invocation that records its result at the export's return checks its custom durable
        // scopes, disposes its producers and releases its retained `Start`s when its background
        // work drained (`on_agent_invocation_settled`).
        let at_return = recorded_at == ResultRecordedAt::ExportReturn;
        let invocation = self.invocation_key();
        if !at_return {
            self.close_custom_durability_state(invocation)?;
        }
        let anchor = self.invocation_anchor().ok_or_else(|| {
            WorkerExecutorError::runtime("method failure has no invocation start")
        })?;
        let key = self
            .state
            .get_current_idempotency_key()
            .ok_or_else(|| WorkerExecutorError::runtime("method failure has no idempotency key"))?;
        // As for a success, a terminal that replay consumed at the export's return is the
        // recorded failure even when background work switched the Store to live since.
        let replayed_terminal = self.state.replayed_invocation_terminal.take();
        let is_live = self.state.is_live();
        if !is_live || replayed_terminal.is_some() {
            if !is_live {
                concurrent::drain_queued_dropped_call_events(self)
                    .await
                    .map_err(|error| error.source)?;
            }
            let parent = self.entity_parent_start_index();
            let recorded = match replayed_terminal {
                Some(terminal) => Some(ReplayState::agent_method_failure(
                    terminal?, anchor, &error, parent,
                )?),
                None => {
                    self.state
                        .replay_state
                        .get_oplog_entry_agent_method_failure(&key, anchor, &error, parent)
                        .await?
                }
            };
            if let Some(finished_index) = recorded {
                if self
                    .public_state
                    .worker()
                    .agent_method_invalidates_read_only_cache(full_function_name)
                {
                    self.state.last_state_change = Some(finished_index);
                }
                self.close_loop_invocation_at_settled_terminal(at_return);
                return Ok(());
            }
            // No terminal exists at the natural tail: complete this interrupted invocation live.
            self.switch_to_live().await?;
        }

        // Both a fresh invocation and an incomplete replay now follow the same live path.
        if !at_return {
            tool::durability::finish_producer_disposals(self).await?;
        }
        if !at_return && matches!(self.state.store_replay_state, StoreReplayState::Primary) {
            self.state
                .replay_state
                .release_retained_starts_at_live_invocation_end()
                .await?;
        }
        let worker = self.public_state.worker();
        let finished_index = worker
            .add_to_oplog(OplogEntry::agent_invocation_failed(
                self.entity_parent_start_index(),
                key,
                error.clone(),
                anchor,
                at_return,
            ))
            .await?;
        let commit_level = match worker.agent_mode() {
            AgentMode::Durable => CommitLevel::Always,
            AgentMode::Ephemeral => CommitLevel::Deferred,
        };
        worker
            .commit_oplog_before_status_update(commit_level)
            .await?;
        self.close_loop_invocation_at_settled_terminal(at_return);
        // Mutations preceding a returned error are retained. Invalidate before publishing
        // the failure so a following reader cannot reuse the pre-invocation state.
        if worker.agent_method_invalidates_read_only_cache(full_function_name) {
            worker.bump_read_only_cache_epoch();
            self.state.last_state_change = Some(finished_index);
        }
        worker
            .store_method_failure(&key, error, finished_index)
            .await;
        self.public_state
            .event_service()
            .emit_invocation_finished(full_function_name, &key, true);
        Ok(())
    }

    fn enter_read_only_mode(&mut self, method_name: String) {
        let state = self.state.loop_invocation_state_mut();
        if state.strictness == InvocationStrictness::ReadOnly {
            warn!(
                "enter_read_only_mode called while already in read-only mode (current method: {:?}, new method: {})",
                state.read_only_method_name, method_name
            );
        }
        state.strictness = InvocationStrictness::ReadOnly;
        state.read_only_method_name = Some(method_name);
    }

    fn exit_read_only_mode(&mut self) {
        let state = self.state.loop_invocation_state_mut();
        match state.strictness {
            InvocationStrictness::ReadOnly => {
                state.strictness = InvocationStrictness::Normal;
                state.read_only_method_name = None;
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
        recorded_at: ResultRecordedAt,
    ) -> Result<(), WorkerExecutorError> {
        // An invocation that records its result at the export's return disposes its producers,
        // checks its custom durable scopes and releases its retained `Start`s when its background
        // work drained (`on_agent_invocation_settled`).
        let at_return = recorded_at == ResultRecordedAt::ExportReturn;
        let mut is_live = self.state.is_live();
        if is_live && !self.state.snapshotting_mode && !at_return {
            tool::durability::finish_producer_disposals(self).await?;
        }
        let invocation = self.invocation_key();
        if !at_return {
            self.close_custom_durability_state(invocation)?;
        }

        let invocation_id = self.state.get_current_idempotency_key().ok_or_else(|| {
            WorkerExecutorError::runtime("agent invocation finished without a current invocation")
        })?;

        // A terminal that replay consumed at the export's return is this invocation's recorded
        // result even when the Store switched to live since: background work of the invocation
        // can continue live at the replay tail before the result is recorded.
        let replayed_terminal = self.state.replayed_invocation_terminal.take();
        if !is_live || replayed_terminal.is_some() {
            // Mirror the live-path drain: events enqueued from synchronous drops during replay
            // (e.g. cleanup for a p3 HTTP response dropped unconsumed) must settle
            // their durable operations before the
            // `AgentInvocationFinished` entry is read.
            if !is_live {
                concurrent::drain_queued_dropped_call_events(self)
                    .await
                    .map_err(|err| err.source)?;
            }

            let response = match replayed_terminal {
                Some(terminal) => Some(
                    self.state
                        .replay_state
                        .agent_invocation_finished(terminal?)
                        .await?,
                ),
                None => {
                    self.state
                        .replay_state
                        .get_oplog_entry_agent_invocation_finished(&invocation_id)
                        .await?
                }
            };
            if let Some((finished_index, recorded_result)) = response {
                let worker = self.public_state.worker();
                let refers_to_return = if let OplogEntry::AgentInvocationFinished {
                    result: golem_common::model::oplog::FinishedResult::Returned(index),
                    ..
                } = worker.oplog().read(finished_index).await?
                {
                    let producer = worker.durable_stream_producer().await?;
                    let session_key = producer.qualify_session(
                        &golem_common::model::durable_stream::StreamRegistrationInvocation::Local(
                            invocation_id,
                        ),
                    );
                    let status = worker
                        .oplog()
                        .raw_durable_stream_session_status(&session_key)
                        .await;
                    let expected = status
                        .status
                        .map_err(WorkerExecutorError::runtime)?
                        .and_then(|status| status.invocation_result);
                    if expected != Some(index) {
                        return Err(WorkerExecutorError::unexpected_oplog_entry(
                            finished_index,
                            format!("FinishedResult::Returned({expected:?})"),
                            format!("FinishedResult::Returned({index})"),
                        ));
                    }
                    true
                } else {
                    false
                };
                if !refers_to_return && !recorded_result.replay_equivalent(&output.result) {
                    return Err(WorkerExecutorError::unexpected_oplog_entry(
                        finished_index,
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
                if self.invocation_changes_state(full_function_name, &recorded_result) {
                    self.state.last_state_change = Some(finished_index);
                }
                self.close_loop_invocation_at_settled_terminal(at_return);
                debug!("Function {full_function_name} finished");
                return Ok(());
            }

            // An incomplete historical invocation can finish after a non-primary entity repair
            // has moved the shared cursor into Settling. Complete the primary transition before
            // publishing this newly repaired invocation as a live success.
            self.switch_to_live().await?;
            is_live = true;
            if !at_return {
                tool::durability::finish_producer_disposals(self).await?;
            }
        }

        if is_live && !self.state.durability_is_suppressed() {
            if !at_return && matches!(self.state.store_replay_state, StoreReplayState::Primary) {
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
            // completion.
            let invalidates_read_only_cache =
                self.invocation_changes_state(full_function_name, &output.result);

            // Only `AgentMethod` results need the method name persisted so the
            // public oplog renderer can resolve the correct output schema.
            let method_name = match &output.result {
                AgentInvocationResult::AgentMethod { .. } => Some(full_function_name.to_string()),
                _ => None,
            };

            let returned = self
                .public_state
                .worker()
                .get_repaired_status()
                .await?
                .running
                .as_ref()
                .filter(|running| running.key == invocation_id)
                .and_then(|running| running.returned);
            let finished_index = self
                .public_state
                .worker()
                .oplog()
                .add_agent_invocation_finished(
                    invocation_id,
                    &output.result,
                    returned,
                    method_name,
                    consumed_fuel,
                    component_revision,
                    at_return,
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
            self.public_state.worker().notify_stream_invocation_result();
            self.close_loop_invocation_at_settled_terminal(at_return);

            // Bump the read-only cache epoch after the
            // `AgentInvocationFinished` entry is committed, but *before*
            // we publish `InvocationCompleted` to waiters via
            // `store_invocation_success`. Ordering matters: any client
            // that observes the completion event must also see an
            // invalidated cache, otherwise it could read a stale cached
            // result for the now-mutated state.
            if invalidates_read_only_cache {
                self.public_state.worker().bump_read_only_cache_epoch();
                self.state.last_state_change = Some(finished_index);
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

impl<Ctx: WorkerCtx> InvocationContextManagement for DurableWorkerCtx<Ctx> {
    fn remove_span(&mut self, span_id: &SpanId) -> Result<(), WorkerExecutorError> {
        if &self.state.invocation_state().current_span_id == span_id {
            // Walk up to the parent if it still exists in the invocation context;
            // otherwise fall back to the root.
            let parent_id = self
                .state
                .invocation_state()
                .invocation_context
                .get(span_id)
                .ok()
                .and_then(|span| span.parent().map(|p| p.span_id().clone()));

            self.state.invocation_state_mut().current_span_id = parent_id
                .filter(|id| {
                    self.state
                        .invocation_state()
                        .invocation_context
                        .get(id)
                        .is_ok()
                })
                .unwrap_or_else(|| {
                    self.state
                        .invocation_state()
                        .invocation_context
                        .root
                        .span_id()
                        .clone()
                });
        }
        let _ = self
            .state
            .invocation_state_mut()
            .invocation_context
            .finish_span(span_id)
            .map_err(WorkerExecutorError::runtime);
        Ok(())
    }

    fn clone_as_inherited_stack(&self, current_span_id: &SpanId) -> InvocationContextStack {
        self.state
            .invocation_state()
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

/// To what an invocation failure is attributed.
#[derive(Debug, PartialEq)]
pub(in crate::durable_host) struct FailureAttribution {
    /// The invocation the failure entry names.
    pub failed_invocation: Option<golem_common::model::InvocationId>,
    /// Whether the failure rejects the invocation: it cancels it with `CancelPendingInvocation`
    /// and completes only its key.
    pub rejection: bool,
}

/// Attributes a failure of the invocation `current`. After the invocation recorded its result
/// (`after_result`), a failure of its background work names no invocation and rejects nothing:
/// it is an ordinary trap of the agent, which the retry policy handles.
pub(in crate::durable_host) fn failure_attribution(
    current: Option<golem_common::model::InvocationId>,
    trap_type: &TrapType,
    after_result: bool,
) -> FailureAttribution {
    FailureAttribution {
        failed_invocation: current.filter(|_| !after_result),
        rejection: trap_type.is_invocation_rejection() && !after_result,
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
            provenance: crate::model::FailureProvenance::Lifecycle,
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
                provenance: crate::model::FailureProvenance::Lifecycle,
            })
        }
        _ => None,
    }
}

/// The retry state recorded with the trap `Error` when the agent decided `decision`. A give-up
/// after the invocation recorded its result records an exhausted state, whatever the policy
/// evaluation gave: the status fold reads that `Error`, which names no invocation, as the
/// background failure of the settling invocation only when its state is exhausted.
pub(in crate::durable_host) fn drain_give_up_retry_state(
    after_result: bool,
    decision: &RetryDecision,
    retry_policy_state: Option<RetryPolicyState>,
) -> Option<RetryPolicyState> {
    match retry_policy_state {
        Some(state)
            if after_result && *decision == RetryDecision::None && !state.is_exhausted() =>
        {
            Some(state.exhausted())
        }
        None if after_result && *decision == RetryDecision::None => {
            Some(RetryPolicyState::Terminal)
        }
        state => state,
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
