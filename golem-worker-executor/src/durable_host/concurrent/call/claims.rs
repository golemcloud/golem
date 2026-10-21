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
pub(crate) fn admission_refusal_before_start(
    claim: &StartClaim,
    initiating_function: &str,
) -> Result<SerializableToolAdmissionRefusal, WorkerExecutorError> {
    Ok(SerializableToolAdmissionRefusal::BeforeStart {
        initiating_function: initiating_function.to_string(),
        claim: desert_rust::serialize(claim, Vec::new()).map_err(|error| {
            WorkerExecutorError::runtime(format!("failed to encode admission claim: {error}"))
        })?,
    })
}

pub(crate) async fn reproduce_incomplete_start_refusal(
    operation: Option<&crate::durable_host::tool::operation::OwnerToolOperation>,
    replay: &ReplayState,
    start_index: OplogIndex,
) -> Result<(), WorkerExecutorError> {
    let Some(operation) = operation else {
        return Ok(());
    };
    let Some((expected, _)) = operation.expected_admission_refusal() else {
        return Ok(());
    };
    if (expected
        == SerializableToolAdmissionRefusal::AtRecordedOplogEntry {
            index: start_index.into(),
        }
        || operation.observed_admission_refusal().is_some())
        && replay
            .retire_incomplete_admission_claim(start_index)
            .await?
    {
        return Err(operation.reproduce_admission_refusal(expected).await);
    }
    Ok(())
}

impl<Pair: HostPayloadPair, P: DropPolicy, Ctx: WorkerCtx> PreparedAccessStart<Pair, P, Ctx> {
    /// Whether this Store itself already continued live locally: an incomplete entity that
    /// switched to live while the shared cursor may still replay other owners' records. Such a
    /// Store still claims while unclaimed retained `Start`s exist (see
    /// [`ReplayState::claim_start_for_store`]).
    pub(super) fn store_continued_live(&self) -> bool {
        self.replaying_incomplete_entity && self.local_live_tail.load(Ordering::Acquire)
    }

    fn replay_to_live_role(&self) -> ReplayToLiveRole {
        if self.primary_runtime {
            ReplayToLiveRole::PrimaryAgent
        } else {
            ReplayToLiveRole::NonPrimary
        }
    }

    fn begin_switch_to_live(
        &self,
    ) -> impl Future<Output = Result<BeginReplayToLive, WorkerExecutorError>> + Send + 'static {
        let replaying_incomplete_entity = self.replaying_incomplete_entity;
        let tool_entity = self.tool_entity;
        let tool_operation = self.tool_operation.clone();
        let public_state = self.public_state.clone();
        let linear_memory = self.linear_memory.clone();
        let replay_state = self.replay_state.clone();
        let role = self.replay_to_live_role();
        let local_live_tail = self.local_live_tail.clone();
        async move {
            crate::durable_host::begin_replay_to_live(
                replaying_incomplete_entity,
                tool_entity,
                tool_operation,
                &public_state,
                &linear_memory,
                &replay_state,
                role,
                local_live_tail,
            )
            .await
        }
    }

    async fn switch_to_live(&mut self) -> Result<(), WorkerExecutorError> {
        let pending = match self.begin_switch_to_live().await? {
            BeginReplayToLive::ReplayResumed => {
                return Err(WorkerExecutorError::runtime(
                    "replay target grew while a durable call was settling",
                ));
            }
            BeginReplayToLive::Pending(pending) => pending,
        };
        pending
            .finish_at(self.admission_boundary.clone())
            .await?
            .require_live()
    }
}

pub(super) async fn publish_incomplete_replay_tail_access<T, D, Ctx>(
    store: &(impl DurableAccess<T, D> + ?Sized),
    get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
    boundary: SerializableToolAdmissionRefusal,
) -> Result<FinishReplayToLive, WorkerExecutorError>
where
    T: 'static,
    D: HasData + ?Sized,
    Ctx: WorkerCtx,
{
    publish_incomplete_replay_tail_at(&mut AccessorContext::new(store, get_ctx), Some(boundary))
        .await
}

pub(super) async fn prepare_access_replay_continuation<Pair, T, D, Ctx>(
    recovery: &mut ScopeReplayRecovery,
    replay_state: &ReplayState,
    live_call_permit: &mut Option<LiveCallPermit>,
    counter: Arc<AtomicUsize>,
    unpersisted: bool,
    store: &(impl DurableAccess<T, D> + ?Sized),
    get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
) -> Result<(), WorkerExecutorError>
where
    Pair: HostPayloadPair,
    T: 'static,
    D: HasData + ?Sized,
    Ctx: WorkerCtx,
{
    if unpersisted {
        return Ok(());
    }
    if live_call_permit.is_none() {
        *live_call_permit = Some(LiveCallPermit::new(counter));
    }
    let readiness = recovery.await_new_call_readiness::<Pair>().await;
    if let Err(error) = readiness {
        replay_state.abandon_replay_for_failure().await?;
        store.with(|mut access| {
            get_ctx(access.data_mut())
                .state
                .authoritative_replay_failure = true
        });
        return Err(error);
    }
    Ok(())
}

pub(in crate::durable_host::concurrent) async fn publish_incomplete_replay_tail_at<
    Ctx: WorkerCtx,
>(
    access: &mut impl DurableContext<Ctx>,
    boundary: Option<SerializableToolAdmissionRefusal>,
) -> Result<FinishReplayToLive, WorkerExecutorError> {
    let (
        replay_state,
        linear_memory,
        role,
        local_live_tail,
        replaying_incomplete_entity,
        tool_entity,
        tool_operation,
        public_state,
    ) = access.with_ctx(|ctx| {
        (
            ctx.state.replay_state.clone(),
            ctx.linear_memory.clone(),
            if ctx.runtime == OwnerRuntime::Agent {
                ReplayToLiveRole::PrimaryAgent
            } else {
                ReplayToLiveRole::NonPrimary
            },
            ctx.state.local_live_tail(),
            ctx.state.entity_execution_mode == Some(InvocationExecutionMode::ReplayingIncomplete),
            matches!(
                ctx.runtime,
                OwnerRuntime::Entity(AgentEntity::Tool(_) | AgentEntity::ToolMiddleware(_))
            ),
            ctx.entity_tool_operation(),
            ctx.public_state.clone(),
        )
    });
    let pending = match crate::durable_host::begin_replay_to_live(
        replaying_incomplete_entity,
        tool_entity,
        tool_operation,
        &public_state,
        &linear_memory,
        &replay_state,
        role,
        local_live_tail,
    )
    .await?
    {
        BeginReplayToLive::ReplayResumed => {
            return Err(WorkerExecutorError::runtime(
                "replay target grew while an incomplete durable call was settling",
            ));
        }
        BeginReplayToLive::Pending(pending) => pending,
    };
    let outcome = pending.finish_at(boundary).await?;
    if outcome == FinishReplayToLive::Live && role == ReplayToLiveRole::PrimaryAgent {
        access.process_replay_events().await?;
    }
    Ok(outcome)
}

pub(crate) async fn finish_prepared_access_to_live<T, D, Ctx>(
    pending: PendingReplayToLive,
    primary_runtime: bool,
    store: &(impl DurableAccess<T, D> + ?Sized),
    get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
) -> Result<FinishReplayToLive, WorkerExecutorError>
where
    T: 'static,
    D: HasData + ?Sized,
    Ctx: WorkerCtx,
{
    finish_prepared_access_to_live_at(pending, primary_runtime, store, get_ctx, None).await
}

pub(crate) async fn finish_prepared_access_to_live_at<T, D, Ctx>(
    pending: PendingReplayToLive,
    primary_runtime: bool,
    store: &(impl DurableAccess<T, D> + ?Sized),
    get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
    boundary: Option<SerializableToolAdmissionRefusal>,
) -> Result<FinishReplayToLive, WorkerExecutorError>
where
    T: 'static,
    D: HasData + ?Sized,
    Ctx: WorkerCtx,
{
    let outcome = pending.finish_at(boundary).await?;
    if outcome == FinishReplayToLive::Live && primary_runtime {
        process_pending_replay_events_access(store, get_ctx).await?;
    }
    Ok(outcome)
}

impl ScopeReplayRecovery {
    pub(crate) fn reexecute_when(
        readiness: impl Future<Output = Result<(), WorkerExecutorError>> + Send + 'static,
    ) -> Self {
        Self::decide_when(async move { readiness.await.map(|()| ScopeReplayDecision::Reexecute) })
    }

    pub(crate) fn decide_when(
        readiness: impl Future<Output = Result<ScopeReplayDecision, WorkerExecutorError>>
        + Send
        + 'static,
    ) -> Self {
        Self::Reexecute {
            readiness: futures::FutureExt::shared(futures::FutureExt::boxed(readiness)),
            loss_may_start_child: false,
        }
    }

    pub(in crate::durable_host::concurrent) fn for_child(&self) -> Self {
        match self {
            // The existing parent authorizes a new terminal child, not a new parent effect.
            Self::CompleteWithLoss => Self::reexecute_when(async { Ok(()) }),
            Self::Reexecute { readiness, .. } => Self::Reexecute {
                readiness: readiness.clone(),
                loss_may_start_child: true,
            },
            recovery => recovery.clone(),
        }
    }

    pub(super) fn allows_new_call(&self) -> bool {
        match self {
            Self::Default => true,
            Self::CompleteWithLoss => false,
            // Readiness is checked before a live Start or repair of an incomplete call.
            Self::Reexecute {
                readiness,
                loss_may_start_child,
            } => {
                *loss_may_start_child
                    || !matches!(
                        readiness.peek(),
                        Some(Ok(ScopeReplayDecision::CompleteWithLoss))
                    )
            }
        }
    }

    pub(super) async fn await_readiness(&mut self) -> Result<(), WorkerExecutorError> {
        match self {
            Self::Reexecute { readiness, .. } => readiness.clone().await.map(|_| ()),
            Self::Default | Self::CompleteWithLoss => Ok(()),
        }
    }

    pub(super) async fn await_new_call_readiness<Pair: HostPayloadPair>(
        &mut self,
    ) -> Result<(), WorkerExecutorError> {
        self.await_readiness().await?;
        if self.allows_new_call() {
            Ok(())
        } else {
            Err(WorkerExecutorError::runtime(format!(
                "{} protocol owner forbids recovery without a recorded Start",
                Pair::FQFN
            )))
        }
    }
}

impl<Pair: HostPayloadPair, P: DropPolicy> DurableCallSession<Pair, P> {
    /// The claim of [`Self::claim_replay_access_with_options`] once the call's identity is
    /// captured.
    pub(super) async fn claim_replay_identified<T, D, Ctx>(
        store: &Accessor<T, D>,
        get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
        identity: AccessCallIdentity,
        function_type: DurableFunctionType,
        claim_options: AccessClaimOptions,
    ) -> Result<ReplayAccessStartOutcome<Self>, WorkerExecutorError>
    where
        T: 'static,
        D: HasData + ?Sized,
        Ctx: WorkerCtx,
    {
        let primary_live = store.with(|mut access| {
            let ctx = get_ctx(access.data_mut());
            ctx.state.durable_call_is_live() && ctx.runtime == OwnerRuntime::Agent
        });
        if primary_live {
            return Ok(ReplayAccessStartOutcome::ReplayEnded);
        }
        process_pending_replay_events_access(store, get_ctx).await?;
        let mut prepared = store.with(|mut access| {
            let ctx = get_ctx(access.data_mut());
            Self::prepare_access_start(
                ctx,
                function_type,
                claim_options,
                identity.custom_invocation_scope,
                false,
                identity.invocation_id,
            )
        })?;
        let claim = Self::replay_start_claim(
            &prepared.claim_options,
            &prepared.execution_scope,
            &prepared.retry,
        )?;
        prepared.admission_boundary = Some(admission_refusal_before_start(&claim, Pair::FQFN)?);
        if prepared.is_live {
            prepare_access_replay_continuation::<Pair, _, _, _>(
                &mut prepared.claim_options.scope_replay_recovery,
                &prepared.replay_state,
                &mut prepared.live_call_permit,
                prepared.live_host_calls.clone(),
                prepared.unpersisted,
                store,
                get_ctx,
            )
            .await?;
            prepared.switch_to_live().await?;
            return if prepared.replaying_incomplete_entity || prepared.primary_runtime {
                Ok(ReplayAccessStartOutcome::ReplayEnded)
            } else {
                Err(WorkerExecutorError::unexpected_oplog_entry(
                    format!("recorded {} Start", Pair::HOST_FUNCTION_NAME),
                    "replay ended before a completed-entity reconstruction claim",
                ))
            };
        }
        if let Err(err) = drain_dropped_call_events_access(store, get_ctx).await {
            return Err(err.source);
        }

        let replay_state = prepared.replay_state.clone();
        let replaying_incomplete_entity = prepared.replaying_incomplete_entity;
        let tool_entity = prepared.tool_entity;
        let tool_operation = prepared.tool_operation.clone();
        let public_state = prepared.public_state.clone();
        let linear_memory = prepared.linear_memory.clone();
        let store_continued_live = prepared.store_continued_live();
        let execution_scope = prepared.execution_scope;
        let retry = prepared.retry;
        match replay_state
            .claim_start_for_store_with_admission(
                claim,
                store_continued_live,
                |_, _| {},
                tool_operation
                    .clone()
                    .map(|operation| (operation, Pair::FQFN.to_string())),
            )
            .await?
        {
            ReplayStartClaimOutcome::Claimed { handle, .. } => {
                let start_idx = handle.start_idx();
                let executed = ExecutedAccessStart {
                    begin_index: start_idx,
                    start_idx,
                    persisted: false,
                    requires_agent_authority: false,
                    agent_auth_ctx: None,
                    request_upload: PendingUpload::already_durable(),
                    replay: Some(handle),
                    execution_scope: execution_scope.finish(start_idx),
                    retry,
                    scope_replay_recovery: prepared.claim_options.scope_replay_recovery,
                    drop_sink: prepared.drop_sink,
                    cleanup_sink: prepared.cleanup_sink,
                    live_call_permit: None,
                    _phantom: PhantomData,
                };
                let handle = store.with(|mut access| {
                    let ctx = get_ctx(access.data_mut());
                    Self::finish_access_start(ctx, executed)
                })?;
                Ok(ReplayAccessStartOutcome::Claimed(handle))
            }
            outcome @ (ReplayStartClaimOutcome::ReplayEnded
            | ReplayStartClaimOutcome::DeletedRegion
            | ReplayStartClaimOutcome::StoreAlreadyLive) => {
                let primary_replay_tail = prepared.primary_runtime
                    && matches!(outcome, ReplayStartClaimOutcome::ReplayEnded);
                if !prepared.replaying_incomplete_entity && !primary_replay_tail {
                    return Err(WorkerExecutorError::unexpected_oplog_entry(
                        format!("recorded {} Start", Pair::HOST_FUNCTION_NAME),
                        format!(
                            "replay continuation at {} is valid only for the primary agent replay tail or an incomplete entity",
                            prepared.replay_state.last_replayed_index()
                        ),
                    ));
                }
                let transition_role = if prepared.primary_runtime {
                    ReplayToLiveRole::PrimaryAgent
                } else {
                    ReplayToLiveRole::NonPrimary
                };
                prepare_access_replay_continuation::<Pair, _, _, _>(
                    &mut prepared.claim_options.scope_replay_recovery,
                    &prepared.replay_state,
                    &mut prepared.live_call_permit,
                    prepared.live_host_calls.clone(),
                    prepared.unpersisted,
                    store,
                    get_ctx,
                )
                .await?;
                let pending = if matches!(outcome, ReplayStartClaimOutcome::ReplayEnded) {
                    match crate::durable_host::begin_replay_to_live(
                        replaying_incomplete_entity,
                        tool_entity,
                        tool_operation,
                        &public_state,
                        &linear_memory,
                        &replay_state,
                        transition_role,
                        prepared.local_live_tail.clone(),
                    )
                    .await?
                    {
                        BeginReplayToLive::ReplayResumed => {
                            return Err(WorkerExecutorError::runtime(
                                "replay target grew while a reconstructed durable call was settling",
                            ));
                        }
                        BeginReplayToLive::Pending(pending) => pending,
                    }
                } else {
                    crate::durable_host::begin_local_live_continuation(
                        replaying_incomplete_entity,
                        tool_entity,
                        tool_operation,
                        &public_state,
                        &linear_memory,
                        transition_role,
                        prepared.local_live_tail.clone(),
                        &prepared.replay_state,
                        prepared.replay_state.replay_target(),
                    )
                    .await?
                };
                finish_prepared_access_to_live_at(
                    pending,
                    prepared.primary_runtime,
                    store,
                    get_ctx,
                    prepared.admission_boundary.clone(),
                )
                .await?
                .require_live()?;
                Ok(ReplayAccessStartOutcome::ReplayEnded)
            }
        }
    }

    /// Enforces that no completion-delivery observer is still armed on this accessor's host
    /// subtask once this call owns a persisted live `Start`.
    ///
    /// An armed observer here would belong to an *earlier* durable call in the same host
    /// function whose response host code consumed internally before issuing this call. That
    /// pattern is forbidden: [`CompletionDelivery::deliver_at_accessor_terminal`] must be the
    /// tail operation of its host function. The invariant is what lets replay withhold a
    /// markerless completed `End` until the recorded tail exhausts — a completion consumed
    /// host-internally would legitimize durable tail entries that depend on an unmarked
    /// delivery, silently re-opening the crash window that tail-gating closes. So instead of
    /// tolerating the handoff, finding an armed observer is a hard error.
    ///
    /// Clearing the observer drops its guard, which suppresses the token (no marker) and
    /// releases its [`LiveCallPermit`]; the follow-up drain joins anything the suppressed token
    /// parked in the drain queue before the error escapes, so settlement accounting stays
    /// consistent while the worker traps.
    ///
    /// The check only runs once *this* call is a persisted live barrier (never for replay or
    /// snapshotting handles): until then a prior observer legitimately stays armed so a guest
    /// cancellation landing before this call's `Start` still records its `CompletionDiscarded`
    /// marker and parks replay at the prior call.
    pub(super) async fn supersede_prior_completion_delivery<T, D, Ctx>(
        &mut self,
        store: &Accessor<T, D>,
        get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
    ) -> Result<(), WorkerExecutorError>
    where
        T: 'static,
        D: HasData + ?Sized,
        Ctx: WorkerCtx,
    {
        if !(self.is_live && self.persisted) {
            return Ok(());
        }
        let cleared = match store.clear_terminal_observer() {
            Ok(cleared) => cleared,
            Err(error) => {
                self.abandon_for_trap();
                return Err(WorkerExecutorError::runtime(format!(
                    "failed to inspect the accessor completion observer: {error}"
                )));
            }
        };
        if cleared {
            // Drain first so the suppressed observer's parked bookkeeping settles before the
            // error escapes.
            if let Err(err) = drain_dropped_call_events_access(store, get_ctx).await {
                self.abandon_for_trap();
                return Err(err.source);
            }
            self.abandon_for_trap();
            return Err(WorkerExecutorError::runtime(format!(
                "a completion-delivery observer of an earlier durable call was still armed when \
                 durable call {} started on the same host subtask: \
                 deliver_at_accessor_terminal must be the tail operation of its host function",
                self.start_idx
            )));
        }
        Ok(())
    }

    /// Persistence-suppression model: only **snapshotting** is handled here (the live
    /// `persisted: false` branch). `PersistenceLevel::PersistNothing` deliberately is *not* — a
    /// live call inside a persist-nothing zone still appends its `Start`/`End`, exactly like a
    /// direct call, because the PersistNothing contract
    /// is enforced elsewhere: `PrimaryOplog::commit` suppresses non-`Always` commits while the
    /// zone is open (so the call's own `DurableOnly` commits flush nothing), zone contents that do
    /// reach storage (via the zone-closing `Always` commit) are observability-only, and the replay
    /// cursor skips whole persist-nothing zones without claiming the entries inside them. The
    /// replay branch below guards against ever *replaying* a durable call inside a PersistNothing
    /// block.
    pub(super) fn replay_start_claim(
        claim_options: &AccessClaimOptions,
        execution_scope: &BegunCallExecutionScope,
        retry: &InFunctionRetryController,
    ) -> Result<StartClaim, WorkerExecutorError> {
        let claim = match (
            execution_scope.parent_start_index,
            claim_options.tool_rejection_function_name.as_ref(),
        ) {
            (Some(parent_start_index), Some(rejected_function_name)) => {
                StartClaim::tool_invocation(
                    &Pair::HOST_FUNCTION_NAME,
                    rejected_function_name,
                    retry.function_type(),
                    parent_start_index,
                )
            }
            (None, Some(_)) => {
                return Err(WorkerExecutorError::runtime(
                    "tool invocation replay claim requires an owner Start index",
                ));
            }
            (Some(parent_start_index), None) => StartClaim::owned(
                &Pair::HOST_FUNCTION_NAME,
                retry.function_type(),
                parent_start_index,
            ),
            (None, None) => StartClaim::unowned(&Pair::HOST_FUNCTION_NAME, retry.function_type()),
        };
        Ok(claim
            .with_observational_owner(execution_scope.observational_owner)
            .with_invocation_id(execution_scope.invocation_id)
            .with_host_internal_calls(claim_options.host_internal_calls))
    }

    pub(super) async fn reproduce_refusal_for_claimed_start<Ctx: WorkerCtx>(
        &mut self,
        access: &mut impl DurableContext<Ctx>,
    ) -> Result<(), WorkerExecutorError> {
        let (operation, replay) =
            access.with_ctx(|ctx| (ctx.entity_tool_operation(), ctx.state.replay_state.clone()));
        if let Err(error) =
            reproduce_incomplete_start_refusal(operation.as_ref(), &replay, self.start_idx).await
        {
            self.finished = true;
            return Err(error);
        }
        Ok(())
    }

    /// Promotes an incomplete parent after its protocol-authorized child reached live execution.
    /// Publishing the child's replay tail does not change the parent's session-local state.
    pub(in crate::durable_host::concurrent) async fn resume_incomplete_scope_access<T, D, Ctx>(
        &mut self,
        store: &Accessor<T, D>,
        get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
    ) -> Result<(), WorkerExecutorError>
    where
        T: 'static,
        D: HasData + ?Sized,
        Ctx: WorkerCtx,
    {
        let replay_state =
            store.with(|mut access| get_ctx(access.data_mut()).state.replay_state.clone());
        let replay = self
            .replay
            .take()
            .expect("incomplete scope repair requires a replaying parent");
        let result = async {
            let outcome = replay_state.await_resolution_outcome(replay).await?;
            if !matches!(
                classify_replay_resolution(outcome),
                ReplayedResolution::Incomplete
            ) {
                return Err(WorkerExecutorError::unexpected_oplog_entry(
                    "an incomplete parent for a live demand-stream child",
                    format!("completed scope at {}", self.start_idx),
                ));
            }
            self.prepare_incomplete_live_repair_at(&mut AccessorContext::new(store, get_ctx))
                .await?;
            publish_incomplete_replay_tail_at(
                &mut AccessorContext::new(store, get_ctx),
                Some(SerializableToolAdmissionRefusal::AtRecordedOplogEntry {
                    index: self.start_idx.into(),
                }),
            )
            .await?
            .require_live()?;
            Ok(())
        }
        .await;
        if result.is_err() {
            self.abandon_for_trap();
        }
        result
    }

    pub(super) async fn prepare_incomplete_live_repair_at<Ctx: WorkerCtx>(
        &mut self,
        access: &mut impl DurableContext<Ctx>,
    ) -> Result<(), WorkerExecutorError> {
        let counter = access.with_ctx(|ctx| ctx.state.live_host_call_counter());
        if let Err(error) = self.prepare_incomplete_live_repair(|| counter).await {
            let replay = access.with_ctx(|ctx| ctx.state.replay_state.clone());
            replay.abandon_replay_for_failure().await?;
            access.with_ctx(|ctx| ctx.state.authoritative_replay_failure = true);
            return Err(error);
        }
        Ok(())
    }

    /// Handles a [`ResolutionOutcome::Incomplete`] resolution, shared by every replay variant.
    ///
    /// Refuses child repair without authorization from its protocol owner.
    /// Otherwise, switches this handle to live
    /// completion of the existing, committed `Start`: the caller re-runs the side effect and
    /// `complete`s, appending the missing `End`. A failure during re-execution stays grouped at
    /// this call's own retry point via the call-owned `execution_scope` (and the semantic-trap
    /// error marker).
    ///
    /// On refusal the handle is marked finished (the outcome is terminal); on success it is live,
    /// persisted, and holds a fresh in-flight permit obtained from `get_counter` (a closure so
    /// accessor callers can bound their `store.with` window to the refusal-free path).
    pub(super) async fn prepare_incomplete_live_repair(
        &mut self,
        get_counter: impl FnOnce() -> Arc<AtomicUsize>,
    ) -> Result<(), WorkerExecutorError> {
        let mut recovery = std::mem::take(self.scope_replay_recovery.get_mut().unwrap());
        if !self.retry.can_reexecute_on_incomplete_replay() {
            self.finished = true;
            return Err(WorkerExecutorError::unexpected_oplog_entry(
                "End or Cancelled",
                format!(
                    "invariant: {} child at Start {} has no owner authorization for incomplete repair",
                    Pair::FQFN,
                    self.start_idx
                ),
            ));
        }
        self.live_call_permit = Some(LiveCallPermit::new(get_counter()));
        if let Err(error) = recovery.await_readiness().await {
            self.finished = true;
            self.live_call_permit = None;
            return Err(error);
        }
        self.is_live = true;
        self.persisted = true;
        Ok(())
    }
}
