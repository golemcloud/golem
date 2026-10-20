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
impl<Pair: HostPayloadPair, P: DropPolicy> DurableCallSession<Pair, P> {
    /// Begins a durable call.
    ///
    /// Observes the function call, applies admission and the read-only side-effect guard, and
    /// drains pending replay events. Then, (live) upload the request and append the eager host-call
    /// `Start`, or (replay) claim the next host-call `Start` and register a resolver receiver for it.
    pub(crate) async fn start<Ctx: WorkerCtx>(
        ctx: &mut DurableWorkerCtx<Ctx>,
        request: Pair::Req,
        function_type: DurableFunctionType,
    ) -> Result<Self, WorkerExecutorError> {
        Self::start_with_options(ctx, request, function_type, AccessClaimOptions::default()).await
    }

    pub(crate) async fn start_with_options<Ctx: WorkerCtx>(
        ctx: &mut DurableWorkerCtx<Ctx>,
        request: Pair::Req,
        function_type: DurableFunctionType,
        options: AccessClaimOptions,
    ) -> Result<Self, WorkerExecutorError> {
        Self::start_with_options_and_authorize(ctx, request, function_type, options, async |_| {
            Ok(None)
        })
        .await
    }

    /// Resolves replay and scope readiness before authorizing a new immutable Start.
    pub(crate) async fn start_with_options_and_authorize<
        Ctx: WorkerCtx,
        E: From<WorkerExecutorError>,
    >(
        ctx: &mut DurableWorkerCtx<Ctx>,
        request: Pair::Req,
        function_type: DurableFunctionType,
        mut options: AccessClaimOptions,
        authorize: impl AsyncFnOnce(&mut DurableWorkerCtx<Ctx>) -> Result<Option<String>, E>,
    ) -> Result<Self, E> {
        let mut begun = Self::begin_inner(ctx, function_type, false).await?;
        let started_replaying = !begun.is_live();
        let _readiness_permit =
            started_replaying.then(|| LiveCallPermit::new(ctx.state.live_host_call_counter()));
        let parent_start_index = options
            .parent_start_index
            .or(ctx.entity_parent_start_index());
        begun.execution_scope.parent_start_index = parent_start_index;
        begun.retry = begun
            .retry
            .with_parent_start_index(parent_start_index)
            .with_incomplete_repair_authorized_by_scope(
                options.authorizes_incomplete_repair(ctx.entity_parent_start_index()),
            );
        match begun
            .resolve_with_recovery(ctx, &mut options.scope_replay_recovery)
            .await?
        {
            ResolvedCall::Live(begun) => {
                if !begun.retry.durable_execution_state().snapshotting_mode {
                    options
                        .scope_replay_recovery
                        .await_new_call_readiness::<Pair>()
                        .await?;
                }
                let denial = authorize(ctx).await?;
                let request: HostRequest = request.into();
                let mut session = begun
                    .start_live_with_host_request_index(ctx, denial, move |_| Ok((request, None)))
                    .await?;
                session.scope_replay_recovery = options.scope_replay_recovery.into();
                Ok(session)
            }
            ResolvedCall::Replay(mut session) => {
                session.scope_replay_recovery = options.scope_replay_recovery.into();
                Ok(session)
            }
        }
    }

    pub(crate) async fn start_with_agent_authority<Ctx: WorkerCtx>(
        ctx: &mut DurableWorkerCtx<Ctx>,
        request: Pair::Req,
        function_type: DurableFunctionType,
    ) -> Result<Self, WorkerExecutorError> {
        Self::start_inner(ctx, request, function_type, true).await
    }

    pub(crate) async fn start_with_agent_authority_capture<Ctx: WorkerCtx, T>(
        ctx: &mut DurableWorkerCtx<Ctx>,
        request: Pair::Req,
        function_type: DurableFunctionType,
        mut capture: impl FnMut(&mut DurableWorkerCtx<Ctx>) -> T,
    ) -> Result<(Self, Option<T>), WorkerExecutorError> {
        let (begun, mut captured) =
            Self::begin_with_agent_authority_capture(ctx, function_type, &mut capture).await?;
        let handle = match begun.resolve(ctx).await? {
            ResolvedCall::Live(begun) => {
                if captured.is_none() {
                    captured = Some(ctx.with_agent_authority_at_boundary(capture).await?);
                }
                begun.start_live(ctx, request).await?
            }
            ResolvedCall::Replay(handle) => handle,
        };
        Ok((handle, captured))
    }

    async fn start_inner<Ctx: WorkerCtx>(
        ctx: &mut DurableWorkerCtx<Ctx>,
        request: Pair::Req,
        function_type: DurableFunctionType,
        requires_agent_authority: bool,
    ) -> Result<Self, WorkerExecutorError> {
        let begun = Self::begin_inner(ctx, function_type, requires_agent_authority).await?;
        match begun.resolve(ctx).await? {
            ResolvedCall::Live(begun) => begun.start_live(ctx, request).await,
            ResolvedCall::Replay(handle) => Ok(handle),
        }
    }

    pub(crate) async fn start_prepared_context<T, D, Ctx, F>(
        store: &(impl DurableAccess<T, D> + ?Sized),
        get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
        prepared: PreparedContextStart<Pair, P, Ctx>,
        build_request: F,
    ) -> Result<Self, WorkerExecutorError>
    where
        T: 'static,
        D: HasData + ?Sized,
        Ctx: WorkerCtx,
        F: AsyncFnOnce(AccessStartContext) -> Result<Pair::Req, WorkerExecutorError>,
    {
        process_pending_replay_events_access(store, get_ctx).await?;
        Self::continue_prepared_context(
            store,
            get_ctx,
            prepared,
            async |context, _| build_request(context).await,
            None,
            None,
            None::<fn(&mut DurableWorkerCtx<Ctx>)>,
        )
        .await
    }

    pub(super) async fn continue_prepared_context<T, D, Ctx, R, C, F, E>(
        store: &(impl DurableAccess<T, D> + ?Sized),
        get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
        prepared: PreparedContextStart<Pair, P, Ctx>,
        build_request: F,
        build_span: Option<BuildSpanCallback>,
        span_observer: Option<Arc<dyn CallSpanObserver>>,
        capture: Option<C>,
    ) -> Result<Self, E>
    where
        T: 'static,
        D: HasData + ?Sized,
        Ctx: WorkerCtx,
        C: FnMut(&mut DurableWorkerCtx<Ctx>) -> R,
        F: AsyncFnOnce(AccessStartContext, Option<R>) -> Result<Pair::Req, E>,
        E: From<WorkerExecutorError>,
    {
        let PreparedContextStart { prepared } = prepared;
        let host_internal_calls = prepared.claim_options.host_internal_calls;
        if let Err(err) = drain_dropped_call_events_context(store, get_ctx).await {
            return Err(err.source.into());
        }

        let executed = Self::execute_access_start(
            store,
            get_ctx,
            prepared,
            build_request,
            build_span,
            span_observer,
            capture,
        )
        .await?;
        let mut session = store.with(|mut access| {
            let ctx = get_ctx(access.data_mut());
            Self::finish_access_start(ctx, executed)
        })?;
        session.host_internal_calls = host_internal_calls;
        Ok(session)
    }

    pub(super) fn prepare_access_start<Ctx: WorkerCtx>(
        ctx: &mut DurableWorkerCtx<Ctx>,
        function_type: DurableFunctionType,
        claim_options: AccessClaimOptions,
        custom_invocation_scope: Option<CustomInvocationScope>,
        requires_agent_authority: bool,
        invocation_id: Option<DurableInvocationId>,
    ) -> Result<PreparedAccessStart<Pair, P, Ctx>, WorkerExecutorError> {
        let durable_execution_state = InFunctionRetryHost::durable_execution_state(ctx);
        if claim_options.require_recorded_start
            && (durable_execution_state.is_live || durable_execution_state.snapshotting_mode)
        {
            return Err(WorkerExecutorError::unexpected_oplog_entry(
                format!("recorded {} Start", Pair::HOST_FUNCTION_NAME),
                "historical call has no recorded Start",
            ));
        }
        DurabilityHost::observe_function_call(ctx, Pair::INTERFACE, Pair::FUNCTION);
        if !is_accessor_supported_function_type(&function_type) {
            return Err(WorkerExecutorError::runtime(format!(
                "p3 accessor durable call path supports only ReadLocal/WriteLocal/ReadRemote/WriteRemote, got {function_type:?}"
            )));
        }
        if is_write_side_effect_for_access(&function_type)
            && let Err(GolemSpecificWasmTrap::WorkerReadOnlyViolation {
                method,
                host_function,
            }) = DurableWorkerCtx::check_read_only_allows(ctx, Pair::FQFN)
        {
            return Err(WorkerExecutorError::ReadOnlyViolation {
                method,
                host_function,
            });
        }

        let parent_start_index = claim_options
            .parent_start_index
            .or(ctx.entity_parent_start_index());
        let parent_start_index = if let Some(owner) = &claim_options.host_internal_owner {
            if owner.execution_scope.entity_parent_start_index != ctx.entity_parent_start_index() {
                return Err(WorkerExecutorError::runtime(
                    "host-internal call belongs to a different entity Store",
                ));
            }
            Some(owner.start_index)
        } else {
            parent_start_index
        };
        let observational_owner = resolve_observational_owner(
            ctx,
            claim_options.observational_owner,
            custom_invocation_scope,
        )?;
        let execution_scope = BegunCallExecutionScope {
            entity_parent_start_index: ctx.entity_parent_start_index(),
            parent_start_index,
            observational_owner,
            invocation_id,
        };
        let is_live = durable_execution_state.is_live;
        let unpersisted = durable_execution_state.snapshotting_mode;
        let retry =
            InFunctionRetryController::new(function_type, durable_execution_state, Pair::FQFN)
                .with_parent_start_index(parent_start_index)
                .with_incomplete_repair_authorized_by_scope(
                    claim_options.authorizes_incomplete_repair(ctx.entity_parent_start_index()),
                );
        let cleanup_sink = ctx.state.dropped_call_event_sender();
        let live_host_calls = ctx.state.live_host_call_counter();
        Ok(PreparedAccessStart {
            is_live,
            unpersisted,
            requires_agent_authority,
            oplog: ctx.state.oplog.clone(),
            public_state: ctx.public_state.clone(),
            primary_runtime: ctx.runtime == OwnerRuntime::Agent,
            replaying_incomplete_entity: ctx
                .entity_invocation_scope()
                .is_some_and(|scope| scope.mode() == InvocationExecutionMode::ReplayingIncomplete),
            tool_entity: matches!(
                ctx.runtime,
                OwnerRuntime::Entity(AgentEntity::Tool(_) | AgentEntity::ToolMiddleware(_))
            ),
            tool_operation: ctx.entity_tool_operation(),
            admission_boundary: None,
            local_live_tail: ctx.state.local_live_tail(),
            replay_state: ctx.state.replay_state.clone(),
            linear_memory: ctx.linear_memory.clone(),
            execution_scope,
            retry,
            drop_sink: P::production_drop_sink(ctx.state.dropped_call_event_sender()),
            cleanup_sink,
            claim_options,
            live_call_permit: if is_live {
                Some(LiveCallPermit::new(live_host_calls.clone()))
            } else {
                None
            },
            live_host_calls,
            _phantom: PhantomData,
        })
    }

    pub(super) async fn execute_access_start<T, D, Ctx, R, C, F, E>(
        store: &(impl DurableAccess<T, D> + ?Sized),
        get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
        mut prepared: PreparedAccessStart<Pair, P, Ctx>,
        build_request: F,
        build_span: Option<BuildSpanCallback>,
        span_observer: Option<Arc<dyn CallSpanObserver>>,
        mut capture: Option<C>,
    ) -> Result<ExecutedAccessStart<Pair, P>, E>
    where
        T: 'static,
        D: HasData + ?Sized,
        Ctx: WorkerCtx,
        C: FnMut(&mut DurableWorkerCtx<Ctx>) -> R,
        F: AsyncFnOnce(AccessStartContext, Option<R>) -> Result<Pair::Req, E>,
        E: From<WorkerExecutorError>,
    {
        // Match borrowed admission: capture live authority before appending the Start.
        // A replaying call captures nothing until claim continuation goes live.
        let mut captured = if let Some(capture) = &mut capture {
            crate::durable_host::call_coordinator::capture_live_agent_authority_access(
                store,
                get_ctx,
                |ctx| (DurableWorkerCtx::agent_auth_ctx(ctx), capture(ctx)),
            )
            .await?
        } else {
            None
        };
        if capture.is_none() && prepared.is_live && !prepared.unpersisted {
            let _boundary_guard =
                lock_synchronized_card_event_boundary_access(store, get_ctx).await?;
        }
        let mut live_call_permit = prepared.live_call_permit.take();
        let admission_hook = if prepared.is_live {
            None
        } else {
            store.with(|mut access| get_ctx(access.data_mut()).replay_admission_hook())
        };
        if let Some(hook) = &admission_hook {
            hook.before_replay_access_start(Pair::FQFN, ReplayAdmissionStage::BeforeScope)
                .await;
        }
        if let Some(hook) = &admission_hook {
            hook.before_replay_access_start(Pair::FQFN, ReplayAdmissionStage::AfterScope)
                .await;
        }

        let replay_state = prepared.replay_state.clone();
        let linear_memory = prepared.linear_memory.clone();
        let transition_role = if prepared.primary_runtime {
            ReplayToLiveRole::PrimaryAgent
        } else {
            ReplayToLiveRole::NonPrimary
        };
        let replaying_incomplete_entity = prepared.replaying_incomplete_entity;
        let tool_entity = prepared.tool_entity;
        let tool_operation = prepared.tool_operation.clone();
        let public_state = prepared.public_state.clone();
        let store_continued_live = prepared.store_continued_live();
        let execution_scope = prepared.execution_scope;
        let mut retry = prepared.retry;
        let mut is_live = prepared.is_live;

        // Claim before building the request. Concurrent replay may have reached the shared live
        // tail while this Store was waiting to run; observing that transition in the claim's
        // cursor transaction lets an incomplete entity continuation build with live semantics.
        // Missing claims while the cursor remains replaying are still strict divergence.
        let replay = if !is_live {
            loop {
                let claim =
                    Self::replay_start_claim(&prepared.claim_options, &execution_scope, &retry)?;
                let observer = span_observer.clone().map(|observer| {
                    let activity = store.with(|mut access| {
                        get_ctx(access.data_mut()).tail_work_tracker().activity()
                    });
                    (observer, Arc::new(activity))
                });
                let claiming = prepared.replay_state.clone();
                let admission = tool_operation
                    .clone()
                    .map(|operation| (operation, Pair::FQFN.to_string()));
                let owned_claim = claim.clone();
                let recorded_start_required = prepared.claim_options.require_recorded_start
                    || (!prepared.replaying_incomplete_entity && !prepared.primary_runtime);
                let outcome = claims::owned_access_claim(
                    store,
                    get_ctx,
                    recorded_start_required,
                    async move {
                        claiming
                            .claim_start_for_store_with_admission(
                                owned_claim,
                                store_continued_live,
                                move |index, entry| {
                                    if let Some((observer, _activity)) = &observer {
                                        observer.clone().started(index, entry, true);
                                    }
                                },
                                admission,
                            )
                            .await
                    },
                )
                .await?;
                match outcome {
                    ReplayStartClaimOutcome::Claimed { handle, .. } => break Some(handle),
                    outcome @ (ReplayStartClaimOutcome::ReplayEnded
                    | ReplayStartClaimOutcome::DeletedRegion
                    | ReplayStartClaimOutcome::StoreAlreadyLive) => {
                        if prepared.claim_options.require_recorded_start {
                            return Err(WorkerExecutorError::unexpected_oplog_entry(
                                format!("recorded {} Start", Pair::HOST_FUNCTION_NAME),
                                "completed parent has no recorded host-internal child",
                            )
                            .into());
                        }
                        let primary_replay_tail = prepared.primary_runtime
                            && matches!(outcome, ReplayStartClaimOutcome::ReplayEnded);
                        if !prepared.replaying_incomplete_entity && !primary_replay_tail {
                            return Err(WorkerExecutorError::unexpected_oplog_entry(
                                format!("recorded {} Start", Pair::HOST_FUNCTION_NAME),
                                format!(
                                    "replay continuation at {} is valid only for an incomplete entity",
                                    prepared.replay_state.last_replayed_index()
                                ),
                            ).into());
                        }

                        prepare_access_replay_continuation::<Pair, _, _, _>(
                            &mut prepared.claim_options.scope_replay_recovery,
                            &prepared.replay_state,
                            &mut live_call_permit,
                            prepared.live_host_calls.clone(),
                            prepared.unpersisted,
                            store,
                            get_ctx,
                        )
                        .await?;

                        tracing::debug!(
                            function = Pair::FQFN,
                            replay_ended = matches!(outcome, ReplayStartClaimOutcome::ReplayEnded),
                            primary_replay_tail,
                            "Accessor durable call continued live after replay"
                        );
                        let pending = if matches!(outcome, ReplayStartClaimOutcome::ReplayEnded) {
                            match crate::durable_host::begin_replay_to_live(
                                replaying_incomplete_entity,
                                tool_entity,
                                tool_operation.clone(),
                                &public_state,
                                &linear_memory,
                                &replay_state,
                                transition_role,
                                prepared.local_live_tail.clone(),
                            )
                            .await?
                            {
                                BeginReplayToLive::ReplayResumed => continue,
                                BeginReplayToLive::Pending(pending) => pending,
                            }
                        } else {
                            crate::durable_host::begin_local_live_continuation(
                                replaying_incomplete_entity,
                                tool_entity,
                                tool_operation.clone(),
                                &public_state,
                                &linear_memory,
                                transition_role,
                                prepared.local_live_tail.clone(),
                                &prepared.replay_state,
                                prepared.replay_state.replay_target(),
                            )
                            .await?
                        };
                        let boundary = admission_refusal_before_start(&claim, Pair::FQFN)?;
                        finish_prepared_access_to_live_at(
                            pending,
                            prepared.primary_runtime,
                            store,
                            get_ctx,
                            Some(boundary),
                        )
                        .await
                        .and_then(FinishReplayToLive::require_live)?;
                        let previous = retry.durable_execution_state();
                        retry = InFunctionRetryController::new(
                            retry.function_type().clone(),
                            DurableExecutionState {
                                is_live: true,
                                snapshotting_mode: previous.snapshotting_mode,
                                max_in_function_retry_delay: previous.max_in_function_retry_delay,
                            },
                            Pair::FQFN,
                        )
                        .with_parent_start_index(execution_scope.parent_start_index)
                        .with_incomplete_repair_authorized_by_scope(
                            retry.incomplete_repair_authorized_by_scope(),
                        );
                        is_live = true;
                        if live_call_permit.is_none() {
                            live_call_permit =
                                Some(LiveCallPermit::new(prepared.live_host_calls.clone()));
                        }
                        break None;
                    }
                }
            }
        } else {
            None
        };
        if replay.is_some()
            && let Some(hook) = &admission_hook
        {
            hook.before_replay_access_start(Pair::FQFN, ReplayAdmissionStage::AfterStart)
                .await;
        }

        if let Some(capture) = &mut capture
            && captured.is_none()
            && is_live
        {
            captured = if prepared.unpersisted
                && store.with(|mut access| get_ctx(access.data_mut()).state.is_replay())
            {
                // Snapshot loading uses reconstructed authority, not current card state.
                Some(store.with(|mut access| {
                    let ctx = get_ctx(access.data_mut());
                    (DurableWorkerCtx::agent_auth_ctx(ctx), capture(ctx))
                }))
            } else {
                crate::durable_host::call_coordinator::capture_live_agent_authority_access(
                    store,
                    get_ctx,
                    |ctx| (DurableWorkerCtx::agent_auth_ctx(ctx), capture(ctx)),
                )
                .await
                .and_then(|captured| {
                    captured.ok_or_else(|| {
                        WorkerExecutorError::runtime(
                            "authority-bearing call did not switch to live execution",
                        )
                    })
                })
                .map(Some)?
            };
        }
        let (agent_auth_ctx, captured) = match captured {
            Some((auth, captured)) => (
                prepared.requires_agent_authority.then_some(auth),
                Some(captured),
            ),
            None => (None, None),
        };

        if is_live && !prepared.unpersisted {
            prepared
                .claim_options
                .scope_replay_recovery
                .await_new_call_readiness::<Pair>()
                .await?;
        }

        if let Some(check) = prepared.claim_options.permission_check.take() {
            let decision = if is_live
                && !(prepared.unpersisted
                    && store.with(|mut access| !get_ctx(access.data_mut()).state.is_live()))
            {
                Some(match check.targets {
                    Ok(targets) => {
                        authorize_live_permissions_at_serialized_access(store, get_ctx, &targets)
                            .await?
                            .map_err(|error| error.to_safe_string())
                    }
                    Err(denial) => Err(denial),
                })
            } else {
                None
            };
            prepared.claim_options.denial = decision
                .as_ref()
                .and_then(|result| result.as_ref().err())
                .cloned();
            let _ = check.decision.send(decision);
        }

        // Build after permission admission and before Start, including on replay for positional
        // side entries. Snapshot loading retains its reconstructed authority behavior above.
        let request = build_request(
            AccessStartContext {
                is_live,
                is_denied: prepared.claim_options.denial.is_some(),
            },
            captured,
        )
        .await?;

        if is_live
            && !prepared.unpersisted
            && let Some(hook) =
                store.with(|mut access| get_ctx(access.data_mut()).replay_admission_hook())
        {
            hook.before_replay_access_start(Pair::FQFN, ReplayAdmissionStage::BeforeLiveStart)
                .await;
        }

        if is_live {
            if prepared.unpersisted {
                let start_idx = prepared.oplog.current_oplog_index().await;
                Ok(ExecutedAccessStart {
                    begin_index: start_idx,
                    start_idx,
                    persisted: false,
                    requires_agent_authority: prepared.requires_agent_authority,
                    agent_auth_ctx,
                    request_upload: PendingUpload::already_durable(),
                    replay: None,
                    execution_scope: execution_scope.finish(start_idx),
                    retry,
                    scope_replay_recovery: prepared.claim_options.scope_replay_recovery,
                    drop_sink: prepared.drop_sink,
                    cleanup_sink: prepared.cleanup_sink,
                    live_call_permit,
                    _phantom: PhantomData,
                })
            } else {
                if !store.with(|mut access| get_ctx(access.data_mut()).state.is_live()) {
                    return Err(WorkerExecutorError::runtime(
                        "live accessor start returned to replay during boundary synchronization",
                    )
                    .into());
                }
                let request: HostRequest = request.into();
                let function_type = retry.function_type().clone();
                let parent_start_index = execution_scope.parent_start_index;
                let observational_owner = execution_scope.observational_owner;
                let invocation_id = execution_scope.invocation_id;
                let denial = prepared.claim_options.denial.clone();
                let span_started = Arc::new(std::sync::Mutex::new((OplogIndex::INITIAL, None)));
                let built_span = span_started.clone();
                let span_activity = span_observer.as_ref().map(|_| {
                    store.with(|mut access| {
                        get_ctx(access.data_mut()).tail_work_tracker().activity()
                    })
                });
                let (owner, worker, inputs) = store.with(|mut access| {
                    let ctx = get_ctx(access.data_mut());
                    (
                        ctx.state.owner_wallet.clone(),
                        ctx.public_state.worker().clone(),
                        ctx.authority_scope_inputs(),
                    )
                });
                let work = owner.synchronize(worker.clone(), inputs.clone(), false, true);
                let oplog = prepared.oplog.clone();
                let tasks = worker.tasks.clone();
                let (marker, teardown, initiation_context) = store.with(|mut access| {
                    let ctx = get_ctx(access.data_mut());
                    (
                        ctx.tool_trap_disposal.clone(),
                        TeardownProbe::new(
                            ctx.public_state.worker().shutdown_token(),
                            ctx.stream_runtime_teardown_probe(),
                        ),
                        ambient_trap_context(ctx),
                    )
                });
                let initiation_permit = live_call_permit.clone();
                let initiation_scope = execution_scope.clone();
                let initiation_type = retry.function_type().clone();
                let sink = prepared.drop_sink.clone();
                let cleanup_sink = prepared.cleanup_sink.clone();
                let (send, receive) = tokio::sync::oneshot::channel();
                let (settled, settlement) = tokio::sync::oneshot::channel();
                let initiation = tasks.spawn_metadata(async move {
                    let result = async {
                        let mut observation = work
                            .await?
                            .expect("unrestricted synchronization returns a snapshot");
                        let boundary = loop {
                            let boundary = owner.lock_boundary().await?;
                            if let Some(application) = owner.pending_replay_finalization() {
                                drop(boundary);
                                application.result.clone().await?;
                                observation = owner
                                    .synchronize(worker.clone(), inputs.clone(), false, true)
                                    .await?
                                    .expect("unrestricted synchronization returns a snapshot");
                                continue;
                            }
                            if observation.generation
                                == worker
                                    .published_authority_generation()
                                    .load(std::sync::atomic::Ordering::Acquire)
                                && Arc::ptr_eq(&observation.wallet, &owner.snapshot())
                            {
                                break boundary;
                            }
                            drop(boundary);
                            observation = owner
                                .synchronize(worker.clone(), inputs.clone(), false, true)
                                .await?
                                .expect("unrestricted synchronization returns a snapshot");
                        };
                        let result = oplog
                            .add_start_with_indexed_reserved_payload(
                                move |start_index| {
                                    let started = build_span
                                        .map(|builder| builder(start_index).map(Box::new))
                                        .transpose()
                                        .map_err(|error| error.to_string())?;
                                    *built_span.lock().unwrap() = (start_index, started);
                                    Ok(request)
                                },
                                move |request_payload| {
                                    let (start_index, span_started) = {
                                        let mut built = span_started.lock().unwrap();
                                        (built.0, built.1.take())
                                    };
                                    let entry = OplogEntry::Start {
                                        timestamp: Timestamp::now_utc(),
                                        parent_start_index,
                                        function_name: Pair::HOST_FUNCTION_NAME,
                                        invocation_id,
                                        observational_owner,
                                        request: Some(request_payload),
                                        durable_function_type: function_type,
                                        span_started,
                                        denial,
                                    };
                                    // All fallible preparation is complete. The actor pushes this entry
                                    // without yielding; cleanup enqueued here follows the current job.
                                    if let Some(observer) = span_observer {
                                        observer.started(start_index, &entry, false);
                                    }
                                    drop(span_activity);
                                    entry
                                },
                            )
                            .await
                            .map_err(start_write_error);
                        drop(boundary);
                        result
                    }
                    .await;
                    let result =
                        result.map(|(start_idx, request_upload)| AcceptedAccessStart::<P> {
                            call: Some(DroppedCall {
                                start_idx,
                                begin_index: start_idx,
                                function_type: initiation_type,
                                request_upload: request_upload.clone(),
                                entity_admission: Pair::HOST_FUNCTION_NAME
                                    == golem_common::model::oplog::host_functions::GolemEntityInvoke::HOST_FUNCTION_NAME,
                                span_finished: None,
                                span_observer: None,
                                trap_context: DurableCallTrapContext {
                                    retry_from: initiation_scope
                                        .finish(start_idx)
                                        .trap_retry_point(),
                                },
                                live_call_permit: initiation_permit,
                            }),
                            result: Some((start_idx, request_upload)),
                            marker,
                            sink,
                            cleanup_sink,
                            teardown,
                            _policy: PhantomData,
                        });
                    let completion = result.as_ref().map(|_| ()).map_err(Clone::clone);
                    let _ = send.send(result);
                    let _ = settled.send(completion);
                });
                drop(initiation.map_err(WorkerExecutorError::runtime)?);
                let mut cleanup = prepared.cleanup_sink.clone().map(|sink| {
                    AccessDropEventDrainGuard::new(
                        sink,
                        vec![DropEvent::AwaitCompletionMarker {
                            receipt: Some(MarkerReceipt::pending(settlement)),
                            trap_context: initiation_context,
                            live_call_permit: live_call_permit.clone(),
                        }],
                    )
                });
                let accepted = receive
                    .await
                    .map_err(|_| WorkerExecutorError::Interrupted {
                        kind: InterruptKind::Jump,
                    })?;
                if let Some(cleanup) = &mut cleanup {
                    cleanup.disarm();
                }
                let (start_idx, request_upload) = accepted?.take();
                Ok(ExecutedAccessStart {
                    begin_index: start_idx,
                    start_idx,
                    persisted: true,
                    requires_agent_authority: prepared.requires_agent_authority,
                    agent_auth_ctx,
                    request_upload,
                    replay: None,
                    execution_scope: execution_scope.finish(start_idx),
                    retry,
                    scope_replay_recovery: prepared.claim_options.scope_replay_recovery,
                    drop_sink: prepared.drop_sink,
                    cleanup_sink: prepared.cleanup_sink,
                    live_call_permit,
                    _phantom: PhantomData,
                })
            }
        } else {
            let replay = replay.expect("a replaying accessor start must claim a recorded Start");
            let start_idx = replay.start_idx();
            Ok(ExecutedAccessStart {
                begin_index: start_idx,
                start_idx,
                persisted: false,
                requires_agent_authority: prepared.requires_agent_authority,
                agent_auth_ctx,
                request_upload: PendingUpload::already_durable(),
                replay: Some(replay),
                execution_scope: execution_scope.finish(start_idx),
                retry,
                scope_replay_recovery: prepared.claim_options.scope_replay_recovery,
                drop_sink: prepared.drop_sink,
                cleanup_sink: prepared.cleanup_sink,
                live_call_permit: None,
                _phantom: PhantomData,
            })
        }
    }

    pub(super) fn finish_access_start<Ctx: WorkerCtx>(
        ctx: &mut DurableWorkerCtx<Ctx>,
        executed: ExecutedAccessStart<Pair, P>,
    ) -> Result<Self, WorkerExecutorError> {
        let is_live = executed.replay.is_none();
        Ok(DurableCallSession {
            start_idx: executed.start_idx,
            boundary: DurableCallBoundary::from_begin_index(executed.begin_index),
            is_live,
            persisted: executed.persisted,
            request_upload: executed.request_upload,
            replay: executed.replay,
            finished: false,
            cancellation_span_id: None,
            span_observer: None,
            parked_undelivered_replay: false,
            host_internal_calls: false,
            execution_scope: executed.execution_scope,
            retry: executed.retry,
            scope_replay_recovery: executed.scope_replay_recovery.into(),
            executor_shutdown: ctx.public_state.worker().shutdown_token(),
            runtime_teardown: ctx.stream_runtime_teardown_probe(),
            tool_trap_disposal: ctx.tool_trap_disposal.clone(),
            entity_admission: Pair::HOST_FUNCTION_NAME
                == golem_common::model::oplog::host_functions::GolemEntityInvoke::HOST_FUNCTION_NAME,
            requires_agent_authority: executed.requires_agent_authority,
            agent_auth_ctx: executed.agent_auth_ctx,
            drop_sink: executed.drop_sink,
            cleanup_sink: executed.cleanup_sink,
            // Taken at prepare time (or at the replay→live switch), before this call's first
            // oplog append, so the in-flight count never dips to zero mid-call.
            live_call_permit: executed.live_call_permit,
            _phantom: PhantomData,
        })
    }

    /// First phase of a durable call: run `begin_durable_function` (read-only guard, pending replay
    /// events) and capture the retry state, *without* yet writing
    /// the host-call `Start` or claiming it on replay.
    ///
    /// This is the explicit two-phase entry point for the rare "two-step" calls whose request
    /// payload depends on the begin index (e.g. an RPC scheduled invocation embeds an
    /// idempotency key derived from it). Such calls cannot use [`Self::start`] because the request
    /// is not yet known at admission. The common case stays on [`Self::start`], which is
    /// just `begin` + `resolve`, followed by `start_live` for a fresh call.
    pub(crate) async fn begin<Ctx: WorkerCtx>(
        ctx: &mut DurableWorkerCtx<Ctx>,
        function_type: DurableFunctionType,
    ) -> Result<BegunCall<Pair, P>, WorkerExecutorError> {
        Self::begin_inner(ctx, function_type, false).await
    }

    pub(crate) async fn begin_with_agent_authority<Ctx: WorkerCtx>(
        ctx: &mut DurableWorkerCtx<Ctx>,
        function_type: DurableFunctionType,
    ) -> Result<BegunCall<Pair, P>, WorkerExecutorError> {
        Self::begin_inner(ctx, function_type, true).await
    }

    pub(crate) async fn begin_with_agent_authority_capture<Ctx: WorkerCtx, T>(
        ctx: &mut DurableWorkerCtx<Ctx>,
        function_type: DurableFunctionType,
        capture: impl FnMut(&mut DurableWorkerCtx<Ctx>) -> T,
    ) -> Result<(BegunCall<Pair, P>, Option<T>), WorkerExecutorError> {
        let custom_invocation_scope =
            wasmtime::component::current_guest_task_context::<CustomInvocationContext>()
                .map_err(|error| WorkerExecutorError::runtime(error.to_string()))?
                .as_deref()
                .and_then(CustomInvocationContext::current);
        let observational_owner = resolve_observational_owner(ctx, None, custom_invocation_scope)?;
        let invocation_id = ctx.direct_call_invocation_id(&function_type)?;
        drain_queued_dropped_call_events(ctx)
            .await
            .map_err(|err| err.source)?;
        DurabilityHost::observe_function_call(ctx, Pair::INTERFACE, Pair::FUNCTION);
        let (boundary, captured) = DurableCallCoordinator::new(ctx)
            .admit_with_agent_authority_capture(
                DurableCallAdmission::new(&function_type, Pair::FQFN)
                    .with_invocation_id(invocation_id),
                capture,
            )
            .await?;
        Ok((
            Self::finish_begin(
                ctx,
                function_type,
                boundary,
                false,
                None,
                observational_owner,
                invocation_id,
            ),
            captured,
        ))
    }

    async fn begin_inner<Ctx: WorkerCtx>(
        ctx: &mut DurableWorkerCtx<Ctx>,
        function_type: DurableFunctionType,
        requires_agent_authority: bool,
    ) -> Result<BegunCall<Pair, P>, WorkerExecutorError> {
        // Capture and validate the host task's immutable initiation-time context before the first
        // await. Calls already in flight retain this owner after the guest leaves the lexical scope.
        let custom_invocation_scope =
            wasmtime::component::current_guest_task_context::<CustomInvocationContext>()
                .map_err(|error| WorkerExecutorError::runtime(error.to_string()))?
                .as_deref()
                .and_then(CustomInvocationContext::current);
        let observational_owner = resolve_observational_owner(ctx, None, custom_invocation_scope)?;
        // A direct call holds the Store, so it reserves its invocation id in guest call order.
        let invocation_id = ctx.direct_call_invocation_id(&function_type)?;
        drain_queued_dropped_call_events(ctx)
            .await
            .map_err(|err| err.source)?;
        DurabilityHost::observe_function_call(ctx, Pair::INTERFACE, Pair::FUNCTION);

        // Read-only guard, pending replay events and durable-scope recovery all happen inside the
        // durable-call coordinator.
        let admission =
            DurableCallAdmission::new(&function_type, Pair::FQFN).with_invocation_id(invocation_id);
        let (boundary, agent_auth_ctx) = if requires_agent_authority {
            let (boundary, auth_ctx) = DurableCallCoordinator::new(ctx)
                .admit_with_agent_authority(admission)
                .await?;
            (boundary, auth_ctx)
        } else {
            (
                DurableCallCoordinator::new(ctx).admit(admission).await?,
                None,
            )
        };
        Ok(Self::finish_begin(
            ctx,
            function_type,
            boundary,
            requires_agent_authority,
            agent_auth_ctx,
            observational_owner,
            invocation_id,
        ))
    }

    fn finish_begin<Ctx: WorkerCtx>(
        ctx: &DurableWorkerCtx<Ctx>,
        function_type: DurableFunctionType,
        boundary: DurableCallBoundary,
        requires_agent_authority: bool,
        agent_auth_ctx: Option<AuthCtx>,
        observational_owner: Option<OplogIndex>,
        invocation_id: Option<DurableInvocationId>,
    ) -> BegunCall<Pair, P> {
        let durable_execution_state = InFunctionRetryHost::durable_execution_state(ctx);
        let execution_scope = BegunCallExecutionScope {
            entity_parent_start_index: ctx.entity_parent_start_index(),
            parent_start_index: ctx.entity_parent_start_index(),
            observational_owner,
            invocation_id,
        };
        let retry =
            InFunctionRetryController::new(function_type, durable_execution_state, Pair::FQFN)
                .with_parent_start_index(execution_scope.parent_start_index)
                .with_incomplete_repair_authorized_by_scope(
                    execution_scope.entity_parent_start_index.is_some(),
                );

        BegunCall {
            boundary,
            execution_scope,
            retry,
            requires_agent_authority,
            agent_auth_ctx,
            drop_sink: P::production_drop_sink(ctx.state.dropped_call_event_sender()),
            cleanup_sink: ctx.state.dropped_call_event_sender(),
            _phantom: PhantomData,
        }
    }
}

impl<Pair: HostPayloadPair, P: DropPolicy> BegunCall<Pair, P> {
    pub(super) fn is_live(&self) -> bool {
        self.retry.durable_execution_state().is_live
    }

    pub fn agent_auth_ctx(&self) -> &AuthCtx {
        self.agent_auth_ctx
            .as_ref()
            .expect("agent authority context requested from a call without live agent authority")
    }

    /// The index returned by `begin_durable_function` — see [`DurableCallSession::begin_index`]. Available
    /// before the request is finalised, so a two-step call can derive its request payload from it.
    pub fn begin_index(&self) -> OplogIndex {
        self.boundary.begin_index()
    }

    /// Second phase on the live path: upload the request and append the eager host-call `Start`
    /// (or, while snapshotting, persist nothing).
    pub(crate) async fn start_live<Ctx: WorkerCtx>(
        self,
        ctx: &mut DurableWorkerCtx<Ctx>,
        request: Pair::Req,
    ) -> Result<DurableCallSession<Pair, P>, WorkerExecutorError> {
        let request: HostRequest = request.into();
        self.start_live_with_host_request_index(ctx, None, move |_| Ok((request, None)))
            .await
    }

    /// Records a call that the host denied before it ran: a `Start` that holds the denial and an
    /// `End` with the pair's denied response. The operation does not run. Recovery of the `Start`
    /// alone completes it with the same response (see [`HostPayloadPair::denied_response`]).
    pub(crate) async fn start_live_denied<Ctx: WorkerCtx>(
        self,
        ctx: &mut DurableWorkerCtx<Ctx>,
        request: Pair::Req,
        denial: String,
    ) -> anyhow::Result<Pair::Resp>
    where
        Pair: DeniablePair,
    {
        let response = Pair::denied(&request, &denial);
        let request: HostRequest = request.into();
        let handle = self
            .start_live_with_host_request_index(ctx, Some(denial), move |_| Ok((request, None)))
            .await?;
        Ok(handle.complete(ctx, response).await?)
    }

    /// [`Self::start_live_denied`] for a call that opens a span: the `Start` records
    /// `span_started` and the `End` closes the span as denied.
    pub(crate) async fn start_live_denied_with_span<Ctx: WorkerCtx>(
        self,
        ctx: &mut DurableWorkerCtx<Ctx>,
        request: Pair::Req,
        denial: String,
        span_started: golem_common::model::oplog::SpanStarted,
    ) -> anyhow::Result<Pair::Resp>
    where
        Pair: DeniablePair,
    {
        let response = Pair::denied(&request, &denial);
        let request: HostRequest = request.into();
        let span_finished = golem_common::model::oplog::SpanFinished {
            span_id: span_started.span_id.clone(),
            finished_at: Timestamp::now_utc(),
            outcome: golem_common::model::oplog::SpanOutcome::Denied,
        };
        let handle = self
            .start_live_with_host_request_index(ctx, Some(denial), move |_| {
                Ok((request, Some(span_started)))
            })
            .await?;
        Ok(handle
            .complete_with_span(ctx, response, span_finished)
            .await?)
    }

    pub(crate) async fn start_live_with_span<Ctx: WorkerCtx>(
        self,
        ctx: &mut DurableWorkerCtx<Ctx>,
        request: Pair::Req,
        span_started: golem_common::model::oplog::SpanStarted,
    ) -> Result<DurableCallSession<Pair, P>, WorkerExecutorError> {
        let request: HostRequest = request.into();
        self.start_live_with_host_request_index(ctx, None, move |_| {
            Ok((request, Some(span_started)))
        })
        .await
    }

    /// Second phase on the live path for requests whose durable identity depends on the exact
    /// host-call `Start` index. The builder runs inside the oplog's serialized writer step after
    /// that index is assigned and before the `Start` is appended, and records the call's span
    /// metadata on the appended `Start`.
    pub(crate) async fn start_live_with_index_and_span<Ctx: WorkerCtx>(
        self,
        ctx: &mut DurableWorkerCtx<Ctx>,
        build: impl FnOnce(
            OplogIndex,
        ) -> Result<
            (Pair::Req, golem_common::model::oplog::SpanStarted),
            WorkerExecutorError,
        > + Send
        + 'static,
    ) -> Result<DurableCallSession<Pair, P>, WorkerExecutorError>
    where
        Pair::Req: Send + 'static,
    {
        self.start_live_with_host_request_index(ctx, None, move |start_index| {
            build(start_index).map(|(request, span)| (request.into(), Some(span)))
        })
        .await
    }

    async fn start_live_with_host_request_index<Ctx: WorkerCtx>(
        self,
        ctx: &mut DurableWorkerCtx<Ctx>,
        denial: Option<String>,
        build_request: impl FnOnce(
            OplogIndex,
        ) -> Result<
            (HostRequest, Option<golem_common::model::oplog::SpanStarted>),
            WorkerExecutorError,
        > + Send
        + 'static,
    ) -> Result<DurableCallSession<Pair, P>, WorkerExecutorError> {
        debug_assert!(self.is_live(), "start_live() called on a replay handle");
        let snapshotting = self.retry.durable_execution_state().snapshotting_mode;
        // The parent is captured explicitly at initiation, never from temporally-open siblings.
        let parent_start_index = self.execution_scope.parent_start_index;
        let (start_idx, persisted, request_upload) = if snapshotting {
            // Snapshotting persists no durable call records, but the request builder still runs so
            // callers can derive the same in-memory identity shape as a persisted call.
            let oplog = ctx.state.oplog.clone();
            let start_idx = oplog.current_oplog_index().await;
            let _ = build_request(start_idx)?;
            (start_idx, false, PendingUpload::already_durable())
        } else {
            let function_type = self.retry.function_type().clone();
            let oplog = ctx.state.oplog.clone();
            let observational_owner = self.execution_scope.observational_owner;
            let invocation_id = self.execution_scope.invocation_id;
            let span_started = std::sync::Arc::new(std::sync::Mutex::new(None));
            let built_span = span_started.clone();
            // Reserve the request payload and append the `Start` in one guarded step: a big request
            // blob's upload is *begun* but not awaited, so the `Start` is appended in initiation
            // order before the (potentially slow) upload finishes. `add_start_with_reserved_payload`
            // forbids — at compile time — any `.await` between reserving the payload and appending
            // the `Start`, which is what keeps concurrent calls' `Start` entries in initiation
            // order. The returned upload is awaited before this call's `End` / `Cancelled`.
            let (idx, request_upload) = oplog
                .add_start_with_indexed_reserved_payload(
                    move |start_index| {
                        let (request, span) =
                            build_request(start_index).map_err(|error| error.to_string())?;
                        *built_span.lock().unwrap() = span.map(Box::new);
                        Ok(request)
                    },
                    move |request_payload| OplogEntry::Start {
                        timestamp: Timestamp::now_utc(),
                        parent_start_index,
                        function_name: Pair::HOST_FUNCTION_NAME,
                        invocation_id,
                        observational_owner,
                        request: Some(request_payload),
                        durable_function_type: function_type,
                        span_started: span_started.lock().unwrap().take(),
                        denial,
                    },
                )
                .await
                .map_err(start_write_error)?;
            (idx, true, request_upload)
        };
        let execution_scope = self.execution_scope.finish(start_idx);
        Ok(DurableCallSession {
            start_idx,
            boundary: self.boundary,
            is_live: true,
            persisted,
            request_upload,
            replay: None,
            finished: false,
            cancellation_span_id: None,
            span_observer: None,
            parked_undelivered_replay: false,
            host_internal_calls: false,
            execution_scope,
            retry: self.retry,
            scope_replay_recovery: ScopeReplayRecovery::default().into(),
            executor_shutdown: ctx.public_state.worker().shutdown_token(),
            runtime_teardown: ctx.stream_runtime_teardown_probe(),
            tool_trap_disposal: ctx.tool_trap_disposal.clone(),
            entity_admission: false,
            requires_agent_authority: self.requires_agent_authority,
            agent_auth_ctx: self.agent_auth_ctx,
            drop_sink: self.drop_sink,
            cleanup_sink: self.cleanup_sink,
            live_call_permit: Some(LiveCallPermit::new(ctx.state.live_host_call_counter())),
            _phantom: PhantomData,
        })
    }

    /// Claims recorded admission or publishes live continuation before live-only preparation.
    pub(crate) async fn resolve<Ctx: WorkerCtx>(
        self,
        ctx: &mut DurableWorkerCtx<Ctx>,
    ) -> Result<ResolvedCall<Pair, P>, WorkerExecutorError> {
        self.resolve_with_recovery(ctx, &mut ScopeReplayRecovery::Default)
            .await
    }

    async fn resolve_with_recovery<Ctx: WorkerCtx>(
        mut self,
        ctx: &mut DurableWorkerCtx<Ctx>,
        recovery: &mut ScopeReplayRecovery,
    ) -> Result<ResolvedCall<Pair, P>, WorkerExecutorError> {
        if self.is_live() {
            return Ok(ResolvedCall::Live(self));
        }
        loop {
            let store_continued_live = ctx.state.store_continued_live();
            let claim = self.replay_claim();
            let outcome = ctx
                .state
                .replay_state
                .claim_start_for_store_with_admission(
                    claim.clone(),
                    store_continued_live,
                    |_, _| {},
                    ctx.entity_tool_operation()
                        .map(|operation| (operation, Pair::FQFN.to_string())),
                )
                .await?;
            match outcome {
                ReplayStartClaimOutcome::Claimed { handle, .. } => {
                    return Ok(ResolvedCall::Replay(self.finish_replay(ctx, handle)));
                }
                outcome @ (ReplayStartClaimOutcome::ReplayEnded
                | ReplayStartClaimOutcome::DeletedRegion
                | ReplayStartClaimOutcome::StoreAlreadyLive) => {
                    let _readiness_permit = LiveCallPermit::new(ctx.state.live_host_call_counter());
                    let readiness = recovery.await_new_call_readiness::<Pair>().await;
                    if let Err(error) = readiness {
                        ctx.state.replay_state.abandon_replay_for_failure().await?;
                        ctx.state
                            .authoritative_replay_failure
                            .store(true, Ordering::Release);
                        return Err(error);
                    }
                    if !ctx
                        .continue_live_at_replay_tail_with_refusal(
                            matches!(outcome, ReplayStartClaimOutcome::ReplayEnded),
                            format!("recorded {} Start", Pair::HOST_FUNCTION_NAME),
                            Some(admission_refusal_before_start(&claim, Pair::FQFN)?),
                        )
                        .await?
                    {
                        continue;
                    }
                    let previous = self.retry.durable_execution_state();
                    let incomplete_repair_authorized_by_scope =
                        self.retry.incomplete_repair_authorized_by_scope();
                    self.retry = InFunctionRetryController::new(
                        self.retry.function_type().clone(),
                        DurableExecutionState {
                            is_live: true,
                            snapshotting_mode: previous.snapshotting_mode,
                            max_in_function_retry_delay: previous.max_in_function_retry_delay,
                        },
                        Pair::FQFN,
                    )
                    .with_parent_start_index(self.execution_scope.parent_start_index)
                    .with_incomplete_repair_authorized_by_scope(
                        incomplete_repair_authorized_by_scope,
                    );
                    if self.requires_agent_authority {
                        self.agent_auth_ctx = ctx.capture_agent_auth_ctx_at_boundary().await?;
                        if self.agent_auth_ctx.is_none() {
                            return Err(WorkerExecutorError::runtime(
                                "replay-tail authority-bearing call did not switch to live execution",
                            ));
                        }
                    }
                    return Ok(ResolvedCall::Live(self));
                }
            }
        }
    }

    pub(super) fn replay_claim(&self) -> StartClaim {
        let claim = match self.execution_scope.parent_start_index {
            Some(parent_start_index) => StartClaim::owned(
                &Pair::HOST_FUNCTION_NAME,
                self.retry.function_type(),
                parent_start_index,
            ),
            None => StartClaim::unowned(&Pair::HOST_FUNCTION_NAME, self.retry.function_type()),
        };
        claim
            .with_observational_owner(self.execution_scope.observational_owner)
            .with_invocation_id(self.execution_scope.invocation_id)
    }

    fn finish_replay<Ctx: WorkerCtx>(
        self,
        ctx: &mut DurableWorkerCtx<Ctx>,
        replay: ReplayCallHandle,
    ) -> DurableCallSession<Pair, P> {
        let start_idx = replay.start_idx();
        let execution_scope = self.execution_scope.finish(start_idx);
        DurableCallSession {
            start_idx,
            boundary: self.boundary,
            is_live: false,
            persisted: false,
            request_upload: PendingUpload::already_durable(),
            replay: Some(replay),
            finished: false,
            cancellation_span_id: None,
            span_observer: None,
            parked_undelivered_replay: false,
            host_internal_calls: false,
            execution_scope,
            retry: self.retry,
            scope_replay_recovery: ScopeReplayRecovery::default().into(),
            executor_shutdown: ctx.public_state.worker().shutdown_token(),
            runtime_teardown: ctx.stream_runtime_teardown_probe(),
            tool_trap_disposal: ctx.tool_trap_disposal.clone(),
            entity_admission: false,
            requires_agent_authority: self.requires_agent_authority,
            agent_auth_ctx: self.agent_auth_ctx,
            drop_sink: self.drop_sink,
            cleanup_sink: self.cleanup_sink,
            live_call_permit: None,
            _phantom: PhantomData,
        }
    }
}

/// A durable call's `Start` that could not be written: a refusal keeps its type, so the call traps
/// as the lost shard it is; anything else is the payload that could not be built.
fn start_write_error(err: crate::services::oplog::OplogError) -> WorkerExecutorError {
    match err {
        fence @ crate::services::oplog::OplogError::Fenced(_) => fence.into(),
        err => WorkerExecutorError::runtime(format!(
            "failed to serialize and store durable call request: {err}"
        )),
    }
}
