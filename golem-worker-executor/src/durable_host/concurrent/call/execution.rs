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
pub(in crate::durable_host::concurrent) fn ambient_trap_context<Ctx: WorkerCtx>(
    ctx: &DurableWorkerCtx<Ctx>,
) -> DurableCallTrapContext {
    DurableCallTrapContext {
        retry_from: InFunctionRetryHost::current_retry_point(ctx),
    }
}

#[async_trait]
impl<Ctx: WorkerCtx> InFunctionRetryHost for ContextRetryHost<'_, Ctx> {
    fn current_retry_point(&self) -> OplogIndex {
        self.task.current_retry_point()
    }

    async fn named_retry_policies(&mut self) -> Vec<golem_common::model::NamedRetryPolicy> {
        self.task.named_retry_policies().await
    }

    async fn current_retry_state_for(
        &self,
        retry_from: OplogIndex,
    ) -> Option<golem_common::model::RetryPolicyState> {
        self.task
            .worker
            .get_last_known_status()
            .await
            .current_retry_state
            .get(&retry_from)
            .cloned()
    }

    fn durable_execution_state(&self) -> DurableExecutionState {
        (self.execution_state)()
    }

    async fn append_retry_error_entry(
        &mut self,
        retry_from: OplogIndex,
        retry_policy_state: Option<golem_common::model::RetryPolicyState>,
    ) -> Result<(), crate::services::oplog::OplogError> {
        if self.durable_execution_state().snapshotting_mode {
            return Ok(());
        }
        self.task
            .append_retry_error_entry(retry_from, retry_policy_state)
            .await
    }
}

impl<'a, H> ScopedRetryHost<'a, H> {
    pub(in crate::durable_host::concurrent) fn new(
        inner: &'a mut H,
        execution_scope: &'a CallExecutionScope,
    ) -> Self {
        Self {
            inner,
            execution_scope,
        }
    }

    pub(super) fn retry_point(&self) -> OplogIndex {
        self.execution_scope.trap_retry_point()
    }
}

#[async_trait]
impl<H: InFunctionRetryHost + Send + Sync> InFunctionRetryHost for ScopedRetryHost<'_, H> {
    fn current_retry_point(&self) -> OplogIndex {
        self.retry_point()
    }

    async fn named_retry_policies(&mut self) -> Vec<golem_common::model::NamedRetryPolicy> {
        self.inner.named_retry_policies().await
    }

    async fn current_retry_state_for(
        &self,
        retry_from: OplogIndex,
    ) -> Option<golem_common::model::RetryPolicyState> {
        self.inner.current_retry_state_for(retry_from).await
    }

    fn durable_execution_state(&self) -> DurableExecutionState {
        self.inner.durable_execution_state()
    }

    async fn append_retry_error_entry(
        &mut self,
        retry_from: OplogIndex,
        retry_policy_state: Option<golem_common::model::RetryPolicyState>,
    ) -> Result<(), crate::services::oplog::OplogError> {
        self.inner
            .append_retry_error_entry(retry_from, retry_policy_state)
            .await
    }
}

#[async_trait]
impl<H: DurabilityHost + Send + Sync> DurabilityHost for ScopedRetryHost<'_, H> {
    fn observe_function_call(&self, interface: &str, function: &str) {
        self.inner.observe_function_call(interface, function);
    }

    async fn begin_durable_function(
        &mut self,
        function_type: &DurableFunctionType,
        host_function: &str,
    ) -> Result<(), WorkerExecutorError> {
        self.inner
            .begin_durable_function(function_type, host_function)
            .await
    }

    async fn end_durable_function(
        &mut self,
        function_type: &DurableFunctionType,
        forced_commit: bool,
    ) -> Result<(), WorkerExecutorError> {
        self.inner
            .end_durable_function(function_type, forced_commit)
            .await
    }

    async fn try_trigger_retry(
        &mut self,
        failure: Error,
        properties: RetryProperties,
    ) -> anyhow::Result<()> {
        try_trigger_host_trap_retry(self, failure, properties).await
    }

    fn create_interrupt_signal(&self) -> Pin<Box<dyn Future<Output = InterruptKind> + Send>> {
        self.inner.create_interrupt_signal()
    }

    fn check_read_only_allows(&self, host_function: &str) -> Result<(), GolemSpecificWasmTrap> {
        self.inner.check_read_only_allows(host_function)
    }
}

impl<Pair: HostPayloadPair, P: DropPolicy> DurableCallSession<Pair, P> {
    /// Starts and drives a re-executable value call through live execution, completed replay, or
    /// incomplete replay repair. This is the default entry point for callers that do not need the
    /// `Start` index or session-owned retry/authority state while performing the live action.
    pub(crate) async fn invoke<Ctx, A, E>(
        ctx: &mut DurableWorkerCtx<Ctx>,
        request: Pair::Req,
        function_type: DurableFunctionType,
        live_action: A,
    ) -> Result<Pair::Resp, E>
    where
        Ctx: WorkerCtx,
        E: DurableCallTrapError,
        A: AsyncFnOnce(&mut DurableWorkerCtx<Ctx>) -> Result<Pair::Resp, E>,
    {
        Self::start(ctx, request, function_type)
            .await?
            .run(ctx, live_action)
            .await
    }

    /// Low-level abandon: marks the call as finished without writing anything to the oplog, leaving
    /// its host-call `Start` incomplete on disk. This is the terminal used when a host call traps
    /// (fall-back to oplog replay) or is interrupted: a trap is **not** a cancellation, so it must
    /// never write a `Cancelled`. The incomplete `Start` is resolved on the next replay/retry (see
    /// [`CallReplayOutcome::Incomplete`]).
    ///
    /// This does **not** attach a [`DurableCallTrapContext`]. Every *hard error* that escapes as a
    /// `TrapType::Error` must instead go through [`Self::trap`], so the post-trap retry grouping is
    /// owned by this call's scope rather than ambient worker state a sibling call could clobber once
    /// durable calls overlap. Raw `abandon_for_trap` is only for non-`TrapType::Error` control flow
    /// (interrupts / sleep-suspend), where `TrapType::from_error` ignores the marker anyway, and for
    /// tests.
    pub(crate) fn abandon_for_trap(&mut self) {
        self.finished = true;
    }

    /// The call-owned trap classification for a hard error escaping this call: its own retry point,
    /// derived purely from the call's execution scope, never from ambient worker state.
    /// Being pure, it needs no `ctx` and is
    /// stable across the call's body, so it can be built at the trap egress point or snapshotted
    /// earlier when the handle is moved before egress (see `io::poll`).
    pub(crate) fn trap_context(&self) -> DurableCallTrapContext {
        DurableCallTrapContext {
            retry_from: self.execution_scope.trap_retry_point(),
        }
    }

    /// Abandons this call for a hard trap and wraps the escaping error with this call's
    /// [`DurableCallTrapContext`], so `TrapType::from_error` groups the failure against the call's
    /// own scope. Use this at every `TrapType::Error` egress after the call has started; see
    /// [`Self::abandon_for_trap`] for the non-error control-flow cases.
    pub fn trap(&mut self, err: impl Into<anyhow::Error>) -> anyhow::Error {
        let err = err.into();
        // Guest wasm traps are classified on the invocation path without a
        // `DurableCallSession`, so they never reach here.
        debug_assert!(
            err.root_cause().downcast_ref::<wasmtime::Trap>().is_none(),
            "DurableCallSession::trap must not wrap a deterministic wasm trap (root cause was a wasmtime::Trap)"
        );
        let context = self.trap_context();
        self.abandon_for_trap();
        mark_durable_call_trap_context(err, context)
    }

    pub fn trap_semantic_retry_override(
        &mut self,
        payload: SemanticTrapRetryOverride,
        kind: HostFailureKind,
        message: impl Into<String>,
    ) -> anyhow::Error {
        self.trap(semantic_trap_retry_override_error(payload, kind, message))
    }

    /// Retry wrapper around [`InFunctionRetryController::try_trigger_retry`]. On the `Err` branch
    /// (a trap is being raised to trigger an oplog-level retry) it automatically
    /// [`abandon_for_trap`](Self::abandon_for_trap)s, so `?`-style call sites stay correct without
    /// hitting the `NotCancellable` unfinished-drop guard.
    pub async fn try_trigger_retry<Ok, Err: Display>(
        &mut self,
        ctx: &mut (impl DurabilityHost + Send + Sync),
        result: &Result<Ok, Err>,
        classify: impl Fn(&Err) -> HostFailureKind,
    ) -> anyhow::Result<()> {
        self.try_trigger_retry_with_properties(ctx, result, classify, RetryProperties::new())
            .await
    }

    pub async fn try_trigger_retry_with_properties<Ok, Err: Display>(
        &mut self,
        ctx: &mut (impl DurabilityHost + Send + Sync),
        result: &Result<Ok, Err>,
        classify: impl Fn(&Err) -> HostFailureKind,
        properties: RetryProperties,
    ) -> anyhow::Result<()> {
        let outcome = {
            let mut retry_host = ScopedRetryHost::new(ctx, &self.execution_scope);
            self.retry
                .try_trigger_retry_with_properties(&mut retry_host, result, classify, properties)
                .await
        };
        outcome.map_err(|err| self.trap(err))
    }

    pub async fn try_trigger_retry_or_loop<Ok, Err: Display>(
        &mut self,
        ctx: &mut (impl DurabilityHost + Send + Sync),
        result: &Result<Ok, Err>,
        classify: impl Fn(&Err) -> HostFailureKind,
    ) -> anyhow::Result<InternalRetryResult> {
        self.try_trigger_retry_or_loop_with_properties(
            ctx,
            result,
            classify,
            RetryProperties::new(),
        )
        .await
    }

    pub async fn try_trigger_retry_or_loop_with_properties<Ok, Err: Display>(
        &mut self,
        ctx: &mut (impl DurabilityHost + Send + Sync),
        result: &Result<Ok, Err>,
        classify: impl Fn(&Err) -> HostFailureKind,
        properties: RetryProperties,
    ) -> anyhow::Result<InternalRetryResult> {
        let outcome = {
            let mut retry_host = ScopedRetryHost::new(ctx, &self.execution_scope);
            self.retry
                .try_trigger_retry_or_loop_with_properties(
                    &mut retry_host,
                    result,
                    classify,
                    properties,
                )
                .await
        };
        outcome.map_err(|err| self.trap(err))
    }

    /// Accessor-path counterpart of [`Self::try_trigger_retry_with_properties`] for p3 host
    /// wrappers: classifies a host error *value* that is about to be recorded and returned to the
    /// guest and, when it is transient and the worker's retry policy allows another attempt,
    /// raises a retry trap (`Err`) instead — routing the failure through the worker-level retry
    /// machinery (worker goes to `Retrying`) rather than letting the guest observe it. On the
    /// `Err` branch the handle is [`trap`](Self::trap)ped (abandoned with the call's trap
    /// context), so `?`-style call sites stay correct; its host-call `Start` is left incomplete
    /// and the call re-executes on the retry's replay. `Ok(())` means the error is permanent or
    /// the retry budget is exhausted: the caller must persist the failed result and return it to
    /// the guest, exactly as before.
    ///
    /// Only meaningful on the live path (recorded failures replay deterministically to the guest
    /// without re-classification, mirroring the `&mut self` path).
    ///
    /// The accessor cannot hold the store across the async policy resolution, so the retry
    /// decision runs against a [`TaskRetryContext`] snapshot taken in one short store window: the
    /// same policy tiers as `PrivateDurableWorkerState::named_retry_policies` and the current
    /// retry state of this call's own retry point (its call or enclosing durable scope
    /// `Start`, matching [`ScopedRetryHost::retry_point`]). `properties` are enriched with the
    /// worker-local context (`agent-type`) before resolution.
    pub async fn try_trigger_retry_access<T, D, Ctx, Ok, Err>(
        &mut self,
        store: &Accessor<T, D>,
        get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
        result: &Result<Ok, Err>,
        classify: impl Fn(&Err) -> HostFailureKind,
        properties: RetryProperties,
    ) -> anyhow::Result<()>
    where
        T: 'static,
        D: HasData + ?Sized,
        Ctx: WorkerCtx,
        Err: Display,
    {
        self.try_trigger_retry_context(store, get_ctx, result, classify, properties)
            .await
    }

    pub(crate) async fn try_trigger_retry_context<T, D, Ctx, Ok, Err>(
        &mut self,
        store: &(impl DurableAccess<T, D> + ?Sized),
        get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
        result: &Result<Ok, Err>,
        classify: impl Fn(&Err) -> HostFailureKind,
        properties: RetryProperties,
    ) -> anyhow::Result<()>
    where
        T: 'static,
        D: HasData + ?Sized,
        Ctx: WorkerCtx,
        Err: Display,
    {
        debug_assert!(
            self.is_live(),
            "try_trigger_retry_access is only valid on the live path"
        );
        let Err(err) = result else {
            return Ok(());
        };
        if classify(err) != HostFailureKind::Transient {
            return Ok(());
        }

        let message = err.to_string();
        let mut properties = properties;
        properties.set(
            "error-type",
            golem_common::model::PredicateValue::Text("transient".to_string()),
        );

        store.with(|mut access| {
            get_ctx(access.data_mut())
                .state
                .enrich_retry_properties(&mut properties)
        });
        let mut retry_host = self
            .retry_context_snapshot(store, get_ctx, &mut properties)
            .await;
        let failure = Error::new(ClassifiedHostError {
            kind: HostFailureKind::Transient,
            message,
        });
        try_trigger_host_trap_retry(&mut retry_host, failure, properties)
            .await
            .map_err(|err| self.trap(err))
    }

    pub(crate) async fn try_trigger_retry_or_loop_context<T, D, Ctx, Ok, Err>(
        &mut self,
        store: &(impl DurableAccess<T, D> + ?Sized),
        get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
        result: &Result<Ok, Err>,
        classify: impl Fn(&Err) -> HostFailureKind,
    ) -> anyhow::Result<InternalRetryResult>
    where
        T: 'static,
        D: HasData + ?Sized,
        Ctx: WorkerCtx,
        Err: Display,
    {
        debug_assert!(self.is_live(), "internal retries require a live call");
        let Err(err) = result else {
            return Ok(InternalRetryResult::Persist);
        };
        let kind = classify(err);
        if kind == HostFailureKind::Permanent {
            return Ok(InternalRetryResult::Persist);
        }
        let mut properties = RetryProperties::new();
        let task = self
            .retry_context_snapshot(store, get_ctx, &mut properties)
            .await;
        let execution_state =
            || store.with(|mut access| get_ctx(access.data_mut()).durable_execution_state());
        let mut context_host = ContextRetryHost {
            task,
            execution_state: &execution_state,
        };
        let mut retry_host = ScopedRetryHost::new(&mut context_host, &self.execution_scope);
        let outcome = self
            .retry
            .try_trigger_retry_or_loop_with_capabilities(
                &mut retry_host,
                result,
                |_| kind,
                properties,
                |_| store.with(|mut access| get_ctx(access.data_mut()).create_interrupt_signal()),
                |host, failure, properties| {
                    Box::pin(try_trigger_host_trap_retry(host, failure, properties))
                },
            )
            .await;
        outcome.map_err(|err| self.trap(err))
    }

    async fn retry_context_snapshot<T, D, Ctx>(
        &self,
        store: &(impl DurableAccess<T, D> + ?Sized),
        get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
        properties: &mut RetryProperties,
    ) -> TaskRetryContext<Ctx>
    where
        T: 'static,
        D: HasData + ?Sized,
        Ctx: WorkerCtx,
    {
        let retry_point = self.execution_scope.trap_retry_point();
        let (
            environment_state_service,
            environment_id,
            default_retry_policy,
            agent_config_retry_policies,
            runtime_retry_policy_mutations,
            worker,
        ) = store.with(|mut access| {
            let ctx = get_ctx(access.data_mut());
            (
                ctx.state.environment_state_service.clone(),
                ctx.state.owned_agent_id.environment_id,
                golem_common::model::NamedRetryPolicy::default_from_config(&ctx.state.config.retry),
                ctx.state.agent_config_retry_policies(),
                ctx.state.runtime_retry_policy_mutations.clone(),
                ctx.public_state.worker(),
            )
        });

        let current_retry_policy_state = worker
            .get_last_known_status()
            .await
            .current_retry_state
            .get(&retry_point)
            .cloned();

        TaskRetryContext {
            retry_point,
            entity_parent_start_index: self.execution_scope.entity_parent_start_index,
            environment_state_service,
            environment_id,
            default_retry_policy,
            agent_config_retry_policies,
            runtime_retry_policy_mutations,
            max_in_function_retry_delay: self
                .retry
                .durable_execution_state()
                .max_in_function_retry_delay,
            current_retry_policy_state,
            retry_properties: properties.clone(),
            worker,
        }
    }

    /// Drives the full live / replay / incomplete-replay flow for a **re-executable** durable call
    /// (reads and idempotent writes), inverting control: the actual side effect is supplied as
    /// `live_action` and run only when needed. It runs on the live path, and again only if replay
    /// finds the eager `Start` committed without its `End` (re-executing to complete that `Start`);
    /// when the call replays from a committed `End`, `live_action` is not run at all.
    ///
    /// On a `live_action` error the handle is abandoned for trap (its `Start` left incomplete, never
    /// a `Cancelled`) and the error is returned, so the propagating `?` cannot trip the
    /// unfinished-drop policy. `E` is the call site's native error (e.g. `wasmtime::Error`); it only
    /// needs to absorb a `WorkerExecutorError` from the durability machinery.
    ///
    /// This is the optional Shape-A combinator that removes the live/replay/incomplete boilerplate.
    /// Sites with retry loops or bespoke control flow keep the explicit `start` / `complete` /
    /// `replay` form.
    pub async fn run<Ctx, A, E>(
        self,
        ctx: &mut DurableWorkerCtx<Ctx>,
        live_action: A,
    ) -> Result<Pair::Resp, E>
    where
        Ctx: WorkerCtx,
        E: DurableCallTrapError,
        A: AsyncFnOnce(&mut DurableWorkerCtx<Ctx>) -> Result<Pair::Resp, E>,
    {
        if self.is_live() {
            self.run_live_action(ctx, live_action).await
        } else {
            match self.replay(ctx).await? {
                CallReplayOutcome::Replayed(response) => Ok(response),
                CallReplayOutcome::Incomplete(handle) => {
                    handle.run_live_action(ctx, live_action).await
                }
            }
        }
    }

    /// Starts and drives an accessor value call. Low-level session construction stays available
    /// inside the crate for calls that derive identity from the `Start` index or own custom retry
    /// state; ordinary p3 values use this driver. The live action executes without holding a store
    /// window. Incomplete replay repairs the existing `Start` only for re-executable calls;
    /// [`Self::replay_access`] rejects it for non-re-executable calls before returning an outcome.
    pub(crate) async fn invoke_access<T, D, Ctx, A, E>(
        store: &Accessor<T, D>,
        get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
        request: Pair::Req,
        function_type: DurableFunctionType,
        live_action: A,
    ) -> Result<Pair::Resp, E>
    where
        T: 'static,
        D: HasData + ?Sized,
        Ctx: WorkerCtx,
        E: DurableCallTrapError,
        A: AsyncFnOnce() -> Result<Pair::Resp, E>,
    {
        Self::invoke_access_with_options(
            store,
            get_ctx,
            request,
            function_type,
            AccessClaimOptions::default(),
            live_action,
        )
        .await
    }

    /// [`Self::invoke_access`] with explicit claim options.
    pub(crate) async fn invoke_access_with_options<T, D, Ctx, A, E>(
        store: &Accessor<T, D>,
        get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
        request: Pair::Req,
        function_type: DurableFunctionType,
        options: AccessClaimOptions,
        live_action: A,
    ) -> Result<Pair::Resp, E>
    where
        T: 'static,
        D: HasData + ?Sized + 'static,
        Ctx: WorkerCtx,
        E: DurableCallTrapError,
        A: AsyncFnOnce() -> Result<Pair::Resp, E>,
    {
        let call = Self::start_access_with_options(store, get_ctx, function_type, options, request)
            .await?;
        if call.is_live() {
            call.run_live_action_access(store, get_ctx, live_action)
                .await
        } else {
            match call.replay_access(store, get_ctx).await? {
                CallReplayOutcome::Replayed(response) => Ok(response),
                CallReplayOutcome::Incomplete(call) => {
                    call.run_live_action_access(store, get_ctx, live_action)
                        .await
                }
            }
        }
    }

    /// [`Self::invoke_access`] for a call that the host can deny before it runs. A new call
    /// authorizes `targets` after its claim resolves and before its immutable `Start`.
    /// A live denied call records the denial in its
    /// `Start` and completes with [`DeniablePair::denied`] without running `live_action`. A
    /// replayed call uses the recorded decision, also when it repairs an incomplete `Start`.
    pub(crate) async fn invoke_deniable_access<T, D, Ctx, A, E>(
        store: &Accessor<T, D>,
        get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
        request: Pair::Req,
        function_type: DurableFunctionType,
        targets: Result<Vec<PermissionTarget>, String>,
        live_action: A,
    ) -> Result<Pair::Resp, E>
    where
        Pair: DeniablePair,
        Pair::Req: Clone,
        T: 'static,
        D: HasData + ?Sized + 'static,
        Ctx: WorkerCtx,
        E: DurableCallTrapError,
        A: AsyncFnOnce() -> Result<Pair::Resp, E>,
    {
        let (call, decision) = Self::start_authorized_access(
            store,
            get_ctx,
            function_type,
            AccessClaimOptions::default(),
            request.clone(),
            targets,
        )
        .await?;
        let denied = decision
            .as_ref()
            .and_then(|decision| decision.as_ref().err())
            .map(|denial| Pair::denied(&request, denial));
        if call.is_live() {
            match denied {
                Some(response) => call
                    .complete_access(store, get_ctx, response)
                    .await
                    .map_err(|error| E::from_durable_call_trap(error.into_marked_anyhow())),
                None => {
                    call.run_live_action_access(store, get_ctx, live_action)
                        .await
                }
            }
        } else {
            match call.replay_access(store, get_ctx).await? {
                CallReplayOutcome::Replayed(response) => Ok(response),
                CallReplayOutcome::Incomplete(call) => {
                    call.run_live_action_access(store, get_ctx, live_action)
                        .await
                }
            }
        }
    }

    pub(crate) fn host_internal_owner(&self) -> HostInternalCallOwner {
        HostInternalCallOwner {
            start_index: self.start_idx,
            execution_scope: self.execution_scope.clone(),
            declared: self.host_internal_calls,
        }
    }

    /// Records a cancellable child whose result is consumed by the host. Its End is sufficient
    /// for replay: there is no guest delivery token, terminal observer, or replay-tail gate.
    /// The outer session must remain alive until the host continuation finishes.
    pub(crate) async fn invoke_host_internal_access<T, D, Ctx, A, E>(
        store: &Accessor<T, D>,
        get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
        owner: HostInternalCallOwner,
        completed_parent: bool,
        request: Pair::Req,
        function_type: DurableFunctionType,
        live_action: A,
    ) -> Result<Pair::Resp, E>
    where
        T: 'static,
        D: HasData + ?Sized,
        Ctx: WorkerCtx,
        E: DurableCallTrapError,
        A: AsyncFnOnce() -> Result<Pair::Resp, E>,
    {
        // The replay claim of a cancelled owner drains its host-internal calls only when the
        // owner declared them; an undeclared one would be left unclaimed on replay.
        if !owner.declared {
            return Err(WorkerExecutorError::runtime(format!(
                "{} is a host-internal call of a call started without host_internal_calls",
                Pair::FQFN
            ))
            .into());
        }
        let hook = store.with(|mut access| get_ctx(access.data_mut()).replay_admission_hook());
        if let Some(hook) = &hook {
            hook.before_replay_access_start(Pair::FQFN, ReplayAdmissionStage::BeforeDeferredStart)
                .await;
        }
        if completed_parent
            && store.with(|mut access| get_ctx(access.data_mut()).state.durable_call_is_live())
        {
            return Err(WorkerExecutorError::unexpected_oplog_entry(
                format!("recorded {} Start", Pair::HOST_FUNCTION_NAME),
                "completed parent has no recorded host-internal child",
            )
            .into());
        }
        let options = AccessClaimOptions {
            observational_owner: owner.execution_scope.observational_owner,
            host_internal_owner: Some(owner),
            require_recorded_start: completed_parent,
            incomplete_repair_authorized_by_scope: !completed_parent,
            ..Default::default()
        };
        let mut call =
            Self::start_access_with_options(store, get_ctx, function_type, options, request)
                .await?;
        if let Some(hook) = &hook {
            hook.before_replay_access_start(Pair::FQFN, ReplayAdmissionStage::AfterDeferredStart)
                .await;
        }
        if !call.is_live() {
            match call
                .replay_access_to(
                    store,
                    get_ctx,
                    AccessResultDestination::Host,
                    completed_parent,
                )
                .await?
            {
                CallReplayOutcome::Replayed(response) => return Ok(response),
                CallReplayOutcome::Incomplete(incomplete) => {
                    call = incomplete;
                }
            }
        }
        let response = match live_action().await {
            Ok(response) => response,
            Err(error) => return Err(E::from_durable_call_trap(call.trap(error))),
        };
        if let Err(error) = drain_dropped_call_events_access(store, get_ctx).await {
            call.abandon_for_trap();
            return Err(E::from_durable_call_trap(error.into_marked_anyhow()));
        }
        let context = call.trap_context();
        let (response, mut guard) = call
            .complete_access_terminal(
                store,
                get_ctx,
                response,
                None,
                AccessResultDestination::Host,
            )
            .await
            .map_err(|source| {
                E::from_durable_call_trap(
                    TerminalCallError::new(source, context).into_marked_anyhow(),
                )
            })?;
        guard.disarm();
        if let Some(hook) = &hook {
            hook.before_replay_access_start(
                Pair::FQFN,
                ReplayAdmissionStage::AfterHostInternalTerminal,
            )
            .await;
        }
        Ok(response)
    }

    pub(crate) async fn run_live_action_access<T, D, Ctx, A, E>(
        mut self,
        store: &Accessor<T, D>,
        get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
        live_action: A,
    ) -> Result<Pair::Resp, E>
    where
        T: 'static,
        D: HasData + ?Sized,
        Ctx: WorkerCtx,
        E: DurableCallTrapError,
        A: AsyncFnOnce() -> Result<Pair::Resp, E>,
    {
        match live_action().await {
            Ok(response) => self
                .complete_access(store, get_ctx, response)
                .await
                .map_err(|error| E::from_durable_call_trap(error.into_marked_anyhow())),
            Err(error) => Err(E::from_durable_call_trap(self.trap(error))),
        }
    }

    /// Runs `live_action` and either completes the call with its response or, on error, abandons the
    /// call for trap and propagates the error wrapped with this call's [`DurableCallTrapContext`].
    /// Shared by both [`Self::run`] paths.
    async fn run_live_action<Ctx, A, E>(
        mut self,
        ctx: &mut DurableWorkerCtx<Ctx>,
        live_action: A,
    ) -> Result<Pair::Resp, E>
    where
        Ctx: WorkerCtx,
        E: DurableCallTrapError,
        A: AsyncFnOnce(&mut DurableWorkerCtx<Ctx>) -> Result<Pair::Resp, E>,
    {
        match live_action(ctx).await {
            Ok(response) => self
                .complete(ctx, response)
                .await
                .map_err(|err| E::from_durable_call_trap(err.into_marked_anyhow())),
            Err(err) => Err(E::from_durable_call_trap(self.trap(err))),
        }
    }
}
