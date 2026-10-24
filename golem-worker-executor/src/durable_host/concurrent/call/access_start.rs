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

impl<Req, T: 'static, D: HasData + ?Sized + 'static, E> AccessRequest<Req, T, D, E> {
    async fn build(self, context: AccessStartContext, store: &Accessor<T, D>) -> Result<Req, E> {
        match self {
            Self::Ready(request) => Ok(request),
            Self::Built(build) => build(context, store).await,
        }
    }
}

impl AccessCallIdentity {
    fn capture<T, D, Ctx>(
        store: &(impl DurableAccess<T, D> + ?Sized),
        get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
        host_function: &str,
        function_type: &DurableFunctionType,
        claim_options: &AccessClaimOptions,
    ) -> Result<Self, WorkerExecutorError>
    where
        T: 'static,
        D: HasData + ?Sized,
        Ctx: WorkerCtx,
    {
        let custom_invocation_scope = store.custom_invocation_scope()?;
        if !is_accessor_supported_function_type(function_type) {
            return Err(WorkerExecutorError::runtime(format!(
                "p3 accessor durable call path supports only ReadLocal/WriteLocal/ReadRemote/WriteRemote, got {function_type:?}"
            )));
        }
        let invocation_id = resolve_access_invocation_id(
            store,
            get_ctx,
            claim_options,
            custom_invocation_scope.is_some(),
        )
        .map_err(|error| {
            WorkerExecutorError::runtime(format!("durable call {host_function}: {error}"))
        })?;
        Ok(Self {
            custom_invocation_scope,
            invocation_id,
        })
    }
}

impl<Pair, P, T, D, Ctx, E> wasmtime::component::AccessorTask<T, D>
    for OwnedAccessStart<Pair, P, T, D, Ctx, E>
where
    Pair: HostPayloadPair,
    P: DropPolicy,
    T: 'static,
    D: HasData + ?Sized + 'static,
    Ctx: WorkerCtx,
    E: From<WorkerExecutorError> + Send + 'static + Into<anyhow::Error>,
{
    async fn run(self, accessor: &Accessor<T, D>) -> wasmtime::Result<()> {
        let Self {
            stage,
            get_ctx,
            request,
            build_span,
            span_observer,
            started,
            _activity,
        } = self;
        let build_request = async move |context, _| request.build(context, accessor).await;
        let start = async {
            match stage {
                OwnedStartStage::Prepared(prepared) => {
                    DurableCallSession::continue_prepared_context(
                        accessor,
                        get_ctx,
                        *prepared,
                        build_request,
                        build_span,
                        span_observer,
                        None::<fn(&mut DurableWorkerCtx<Ctx>)>,
                    )
                    .await
                }
                OwnedStartStage::Identified {
                    identity,
                    function_type,
                    claim_options,
                } => {
                    DurableCallSession::start_identified(
                        accessor,
                        get_ctx,
                        identity,
                        function_type,
                        *claim_options,
                        false,
                        build_request,
                        build_span,
                        span_observer,
                        None::<fn(&mut DurableWorkerCtx<Ctx>)>,
                    )
                    .await
                }
            }
        };
        hand_over_owned_start(accessor, get_ctx, start, started, |session| {
            session.discard_after_guest_drop()
        })
        .await
    }
}

impl<Pair, P, T, D, Ctx> wasmtime::component::AccessorTask<T, D>
    for OwnedReplayClaim<Pair, P, T, Ctx>
where
    Pair: HostPayloadPair,
    P: DropPolicy,
    T: 'static,
    D: HasData + ?Sized,
    Ctx: WorkerCtx,
{
    async fn run(self, accessor: &Accessor<T, D>) -> wasmtime::Result<()> {
        let Self {
            identity,
            function_type,
            claim_options,
            get_ctx,
            claimed,
            _activity,
        } = self;
        let claim = DurableCallSession::claim_replay_identified(
            accessor,
            get_ctx,
            identity,
            function_type,
            claim_options,
        );
        hand_over_owned_start(accessor, get_ctx, claim, claimed, |outcome| {
            if let ReplayAccessStartOutcome::Claimed(session) = outcome {
                session.discard_after_guest_drop();
            }
        })
        .await
    }
}

/// Hands the result of an owned start to the guest's call. The start races the worker's
/// interrupt signal like a park point does, so an interruption unwinds the task even while the
/// start waits. A result the guest no longer waits for goes to `discard`.
async fn hand_over_owned_start<R, T, D, Ctx, E>(
    accessor: &Accessor<T, D>,
    get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
    start: impl Future<Output = Result<R, E>>,
    handed: oneshot::Sender<Result<R, E>>,
    discard: impl FnOnce(R),
) -> wasmtime::Result<()>
where
    T: 'static,
    D: HasData + ?Sized,
    Ctx: WorkerCtx,
    E: From<WorkerExecutorError> + Into<anyhow::Error>,
{
    let interrupt =
        accessor.with(|mut access| get_ctx(access.data_mut()).create_interrupt_signal());
    tokio::pin!(start);
    let result = tokio::select! {
        result = &mut start => result,
        interruption = interrupt => {
            // Classify the selected disposal before dropping the metadata receiver. Its
            // independently owned Start may be accepted after this task unwinds.
            accessor.with(|mut access| {
                let ctx = get_ctx(access.data_mut());
                match interruption {
                    InterruptKind::EntityCancellation => {
                        ctx.tool_trap_disposal.store(true, Ordering::Release);
                    }
                    InterruptKind::Interrupt(_)
                    | InterruptKind::Restart
                    | InterruptKind::Suspend(_)
                    | InterruptKind::Jump
                    | InterruptKind::ShardLost => ctx.begin_stream_runtime_teardown(),
                }
            });
            let error = WorkerExecutorError::from(interruption);
            let _ = handed.send(Err(error.clone().into()));
            return Err(error.into());
        }
    };
    match handed.send(result) {
        Ok(()) => Ok(()),
        Err(Ok(result)) => {
            discard(result);
            Ok(())
        }
        // The guest dropped the call, so nobody else can fail with this error.
        Err(Err(error)) => Err(wasmtime::Error::from_anyhow(error.into())),
    }
}

/// The invocation id of the guest import that `store` belongs to, for a task the import spawns
/// to make its durable calls. A call observed by a custom durable invocation records none.
pub(crate) fn access_invocation_id<T, D, Ctx>(
    store: &Accessor<T, D>,
    get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
) -> Result<InvocationIdSource, WorkerExecutorError>
where
    T: 'static,
    D: HasData + ?Sized,
    Ctx: WorkerCtx,
{
    let observed_by_custom_invocation = store
        .guest_task_context::<CustomInvocationContext>()
        .map_err(|error| WorkerExecutorError::runtime(error.to_string()))?
        .as_deref()
        .and_then(CustomInvocationContext::current)
        .is_some();
    guest_import_invocation_id(store, get_ctx, observed_by_custom_invocation)
        .map(InvocationIdSource::Reserved)
}

/// Reserves the invocation id of the concurrent guest import that `store` belongs to, for a host
/// function that `await`s before it starts its first durable call. An import reserves its ordinal
/// lazily, and only while it is the guest's newest import (see
/// [`crate::durable_host::durability::GuestImportNumbering`]); after an `await` the guest may have
/// made another import call. The durable calls the import starts later use this id.
pub(crate) fn reserve_invocation_id_before_await<T, D, Ctx>(
    store: &Accessor<T, D>,
    get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
) -> Result<(), WorkerExecutorError>
where
    T: 'static,
    D: HasData + ?Sized,
    Ctx: WorkerCtx,
{
    access_invocation_id(store, get_ctx).map(|_| ())
}

/// The invocation id of a Store-holding guest import, for a task the import spawns to make its
/// durable calls.
pub(crate) fn import_invocation_id<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    observational_owner: Option<OplogIndex>,
) -> Result<InvocationIdSource, WorkerExecutorError> {
    if observational_owner.is_some() {
        return Ok(InvocationIdSource::Reserved(None));
    }
    ctx.direct_call_invocation_id()
        .map(InvocationIdSource::Reserved)
}

pub(in crate::durable_host::concurrent) fn guest_import_invocation_id<T, D, Ctx>(
    store: &Accessor<T, D>,
    get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
    observed_by_custom_invocation: bool,
) -> Result<Option<DurableInvocationId>, WorkerExecutorError>
where
    T: 'static,
    D: HasData + ?Sized,
    Ctx: WorkerCtx,
{
    if observed_by_custom_invocation {
        return Ok(None);
    }
    let activity = store.runtime_activity();
    store.with(|mut access| {
        let ctx = get_ctx(access.data_mut());
        if ctx.state.snapshotting_mode {
            return Ok(None);
        }
        ctx.guest_imports
            .reserve_import_identity(activity)
            .map(GuestImportIdentity::invocation_id)
    })
}

/// Resolves the invocation id of an accessor call from its claim options. Runs before the call's
/// first `await`.
///
/// A host-internal child records the id of its owner. Any other call records the id of the
/// guest import that makes it.
fn resolve_access_invocation_id<T, D, Ctx>(
    store: &(impl DurableAccess<T, D> + ?Sized),
    get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
    claim_options: &AccessClaimOptions,
    observed_by_custom_invocation: bool,
) -> Result<Option<DurableInvocationId>, WorkerExecutorError>
where
    T: 'static,
    D: HasData + ?Sized,
    Ctx: WorkerCtx,
{
    match claim_options.invocation_id {
        InvocationIdSource::Reserved(invocation_id) => Ok(invocation_id),
        _ if claim_options.host_internal_owner.is_some() => Ok(claim_options
            .host_internal_owner
            .as_ref()
            .and_then(|owner| owner.execution_scope.invocation_id)),
        InvocationIdSource::HostTask => Ok(None),
        InvocationIdSource::GuestImport => store.call_invocation_id(
            get_ctx,
            observed_by_custom_invocation || claim_options.observational_owner.is_some(),
        ),
    }
}

pub(crate) fn resolve_observational_owner<Ctx: WorkerCtx>(
    ctx: &DurableWorkerCtx<Ctx>,
    explicit_owner: Option<OplogIndex>,
    custom_invocation_scope: Option<CustomInvocationScope>,
) -> Result<Option<OplogIndex>, WorkerExecutorError> {
    match explicit_owner {
        Some(owner) => Ok(Some(owner)),
        None => custom_invocation_scope
            .map(|scope| {
                let registered = ctx
                    .state
                    .custom_invocation_scopes
                    .get(&scope.scope_id)
                    .ok_or_else(|| {
                        WorkerExecutorError::runtime(format!(
                            "custom-operation scope {} is no longer active",
                            scope.scope_id
                        ))
                    })?;
                if registered.owner_start_index != scope.owner_start_index
                    || !ctx
                        .state
                        .active_custom_invocations
                        .contains_key(&scope.owner_start_index)
                {
                    return Err(WorkerExecutorError::runtime(format!(
                        "custom-operation scope {} has stale invocation attribution",
                        scope.scope_id
                    )));
                }
                Ok(scope.owner_start_index)
            })
            .transpose(),
    }
}

pub(crate) fn resolve_current_observational_owner<Ctx: WorkerCtx>(
    ctx: &DurableWorkerCtx<Ctx>,
    custom_invocation_context: Option<&CustomInvocationContext>,
) -> Result<Option<OplogIndex>, WorkerExecutorError> {
    resolve_observational_owner(
        ctx,
        None,
        custom_invocation_context.and_then(CustomInvocationContext::current),
    )
}

impl<Pair: HostPayloadPair, P: DropPolicy> DurableCallSession<Pair, P> {
    /// Accessor-window entry point for p3 host functions. The method uses short synchronous store
    /// windows only for worker-state snapshots and in-memory bookkeeping; oplog/replay awaits run on
    /// owned handles outside those windows.
    ///
    /// The accessor path supports the four durable function types.
    ///
    /// The start of a call made by the guest runs in an owned store task; see
    /// [`Self::start_access_with_options`].
    pub(crate) async fn start_access<T, D, Ctx>(
        store: &Accessor<T, D>,
        get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
        request: Pair::Req,
        function_type: DurableFunctionType,
    ) -> Result<Self, WorkerExecutorError>
    where
        T: 'static,
        D: HasData + ?Sized + 'static,
        Ctx: WorkerCtx,
    {
        Self::start_access_with_options(
            store,
            get_ctx,
            function_type,
            AccessClaimOptions::default(),
            request,
        )
        .await
    }

    /// Like [`Self::start_access`], but the request payload is built by `build_request` before
    /// the host-call `Start` write. This is the accessor
    /// counterpart of the two-step [`Self::begin`] + [`BegunCall::start_live`] flow.
    ///
    /// The builder runs on **both** the live and the replay path, but its
    /// returned request is only persisted on the live path. It receives an [`AccessStartContext`]
    /// with the liveness flag, and the accessor of the task that runs the start, which is not the
    /// accessor of the guest's call when the start is owned (see
    /// [`Self::start_access_with_options`]).
    ///
    /// A builder error aborts the call like any other start failure: any already-written
    /// durable-scope `Start` is left incomplete,
    /// to be recovered by scope recovery on the next replay.
    pub(crate) async fn start_access_with<T, D, Ctx, F>(
        store: &Accessor<T, D>,
        get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
        function_type: DurableFunctionType,
        build_request: F,
    ) -> Result<Self, WorkerExecutorError>
    where
        T: 'static,
        D: HasData + ?Sized + 'static,
        Ctx: WorkerCtx,
        F: for<'a> FnOnce(
                AccessStartContext,
                &'a Accessor<T, D>,
            ) -> BoxFuture<'a, Result<Pair::Req, WorkerExecutorError>>
            + Send
            + 'static,
    {
        let mut handle = Self::start_access_request(
            store,
            get_ctx,
            function_type,
            AccessClaimOptions::default(),
            AccessRequest::Built(Box::new(build_request)),
            None,
            None,
        )
        .await?;
        handle
            .supersede_prior_completion_delivery(store, get_ctx)
            .await?;
        Ok(handle)
    }

    /// [`Self::start_access`] with explicit [`AccessClaimOptions`], for calls whose durable
    /// records need extra identity to be claim-safe under concurrent siblings on replay.
    ///
    /// The guest may cancel a pending call at any point. A start that ran in the guest's call
    /// would stop where the cancellation finds it: before the `Start` is appended or claimed, or
    /// after the `Start` is appended but before the session exists to run its drop policy. What
    /// the oplog holds of the call would then depend on host scheduling, and replay, which
    /// reaches a different point when the guest cancels again, could not reproduce it. So the
    /// call's identity is captured in the guest's call, and a call made by the guest starts in
    /// an owned store task. The task always appends (or claims) the `Start`. When the guest has
    /// dropped the call by then, the task drops the session, and the drop policy records the
    /// call as if the guest had dropped it right after its start. Calls made by host-owned tasks
    /// and host-internal calls start inline: the guest cannot cancel their start.
    pub(crate) async fn start_access_with_options<T, D, Ctx>(
        store: &Accessor<T, D>,
        get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
        function_type: DurableFunctionType,
        claim_options: AccessClaimOptions,
        request: Pair::Req,
    ) -> Result<Self, WorkerExecutorError>
    where
        T: 'static,
        D: HasData + ?Sized + 'static,
        Ctx: WorkerCtx,
    {
        let mut handle = Self::start_access_request(
            store,
            get_ctx,
            function_type,
            claim_options,
            AccessRequest::<_, T, D, WorkerExecutorError>::Ready(request),
            None,
            None,
        )
        .await?;
        handle
            .supersede_prior_completion_delivery(store, get_ctx)
            .await?;
        Ok(handle)
    }

    /// Authorizes a new call before its immutable `Start`. Recorded calls, including incomplete
    /// repairs, keep their recorded decision. Initiation owns the check even if the guest cancels.
    pub(crate) async fn start_authorized_access<T, D, Ctx>(
        store: &Accessor<T, D>,
        get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
        function_type: DurableFunctionType,
        mut options: AccessClaimOptions,
        request: Pair::Req,
        targets: Result<Vec<PermissionTarget>, String>,
    ) -> Result<(Self, Option<Result<LiveAuthorizationPermit, String>>), WorkerExecutorError>
    where
        Pair: DeniablePair,
        T: 'static,
        D: HasData + ?Sized + 'static,
        Ctx: WorkerCtx,
    {
        let (check, receive) = AccessPermissionCheck::new(targets);
        options.permission_check = Some(check);
        let call = Self::start_access_with_options(store, get_ctx, function_type, options, request)
            .await?;
        let decision = receive.await.map_err(|_| {
            WorkerExecutorError::runtime("durable call start lost its authorization decision")
        })?;
        Ok((call, decision))
    }

    /// Starts an accessor call with metadata prepared before its immutable `Start` is appended.
    pub(crate) async fn start_access_with_options_and_span<T, D, Ctx>(
        store: &Accessor<T, D>,
        get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
        function_type: DurableFunctionType,
        claim_options: AccessClaimOptions,
        request: Pair::Req,
        span_started: golem_common::model::oplog::SpanStarted,
    ) -> Result<Self, WorkerExecutorError>
    where
        T: 'static,
        D: HasData + ?Sized + 'static,
        Ctx: WorkerCtx,
    {
        let mut handle = Self::start_access_request(
            store,
            get_ctx,
            function_type,
            claim_options,
            AccessRequest::<_, T, D, WorkerExecutorError>::Ready(request),
            Some(Box::new(move |_| Ok(span_started))),
            None,
        )
        .await?;
        handle
            .supersede_prior_completion_delivery(store, get_ctx)
            .await?;
        Ok(handle)
    }

    /// Starts an accessor call whose opening span identity is derived from the reserved host-call
    /// `Start` index. The span metadata is constructed inside the same serialized oplog step that
    /// appends the immutable `Start`; replay claims the call by its ordinary execution identity
    /// and reads the recorded metadata afterwards.
    pub(crate) async fn start_access_with_options_and_indexed_span<T, D, Ctx, S, F>(
        store: &Accessor<T, D>,
        get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
        function_type: DurableFunctionType,
        claim_options: AccessClaimOptions,
        build_request: F,
        build_span: S,
        span_observer: Arc<dyn CallSpanObserver>,
    ) -> Result<Self, wasmtime::Error>
    where
        T: 'static,
        D: HasData + ?Sized + 'static,
        Ctx: WorkerCtx,
        F: for<'a> FnOnce(
                AccessStartContext,
                &'a Accessor<T, D>,
            ) -> BoxFuture<'a, Result<Pair::Req, wasmtime::Error>>
            + Send
            + 'static,
        S: FnOnce(
                OplogIndex,
            )
                -> Result<golem_common::model::oplog::SpanStarted, WorkerExecutorError>
            + Send
            + 'static,
    {
        let mut handle = Self::start_access_request(
            store,
            get_ctx,
            function_type,
            claim_options,
            AccessRequest::Built(Box::new(build_request)),
            Some(Box::new(build_span)),
            Some(span_observer.clone()),
        )
        .await?;
        // This entry point's caller registers its own early terminal observer after checking
        // for a prior one. Clearing here would destroy the current call's span observer.
        handle.span_observer = Some(span_observer);
        Ok(handle)
    }

    /// The start shared by the accessor entry points. See [`Self::start_access_with_options`]
    /// for when the start is owned.
    async fn start_access_request<T, D, Ctx, E>(
        store: &Accessor<T, D>,
        get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
        function_type: DurableFunctionType,
        claim_options: AccessClaimOptions,
        request: AccessRequest<Pair::Req, T, D, E>,
        build_span: Option<BuildSpanCallback>,
        span_observer: Option<Arc<dyn CallSpanObserver>>,
    ) -> Result<Self, E>
    where
        T: 'static,
        D: HasData + ?Sized + 'static,
        Ctx: WorkerCtx,
        E: From<WorkerExecutorError> + Send + 'static + Into<anyhow::Error>,
    {
        let identity = AccessCallIdentity::capture(
            store,
            get_ctx,
            Pair::FQFN,
            &function_type,
            &claim_options,
        )?;
        if !store.has_guest_visible_subtask() || claim_options.host_internal_owner.is_some() {
            return Self::start_identified(
                store,
                get_ctx,
                identity,
                function_type,
                claim_options,
                false,
                async move |context, _| request.build(context, store).await,
                build_span,
                span_observer,
                None::<fn(&mut DurableWorkerCtx<Ctx>)>,
            )
            .await;
        }
        // A live call is prepared in the guest's call, as an inline start would prepare it
        // before its first `await`: the call counts as in flight from the guest's call on.
        let stage =
            if store.with(|mut access| get_ctx(access.data_mut()).state.durable_call_is_live()) {
                OwnedStartStage::Prepared(store.with(|mut access| {
                    Self::prepare_access_start(
                        get_ctx(access.data_mut()),
                        function_type,
                        claim_options,
                        identity.custom_invocation_scope,
                        false,
                        identity.invocation_id,
                    )
                    .map(|prepared| Box::new(PreparedContextStart { prepared }))
                })?)
            } else {
                OwnedStartStage::Identified {
                    identity,
                    function_type,
                    claim_options: Box::new(claim_options),
                }
            };
        let (started, receiver) = oneshot::channel();
        let activity =
            store.with(|mut access| get_ctx(access.data_mut()).tail_work_tracker().activity());
        store.spawn(OwnedAccessStart {
            stage,
            get_ctx,
            request,
            build_span,
            span_observer,
            started,
            _activity: activity,
        });
        receiver.await.map_err(|_| {
            WorkerExecutorError::runtime("owned durable call start ended without a result")
        })?
    }

    /// Starts a host-owned call through the same access windows as component calls. The owner
    /// drives the returned session independently of its result observer and uses deferred
    /// completion to keep recording separate from delivery. Historical-only jobs set
    /// `require_recorded_start` in `claim_options` before initiation.
    pub(crate) async fn start_context_with_options<T, D, Ctx, F>(
        store: &(impl DurableAccess<T, D> + ?Sized),
        get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
        function_type: DurableFunctionType,
        claim_options: AccessClaimOptions,
        build_request: F,
    ) -> Result<Self, WorkerExecutorError>
    where
        T: 'static,
        D: HasData + ?Sized,
        Ctx: WorkerCtx,
        F: AsyncFnOnce(AccessStartContext) -> Result<Pair::Req, WorkerExecutorError>,
    {
        Self::start_access_with_options_and_span_builder(
            store,
            get_ctx,
            function_type,
            claim_options,
            build_request,
            None,
            None,
            false,
        )
        .await
    }

    /// Starts an authority-bearing call without holding the context across protocol waits.
    /// Authority is captured at admission, or after scope recovery first makes the call live.
    pub(crate) async fn start_context_with_agent_authority<T, D, Ctx>(
        store: &(impl DurableAccess<T, D> + ?Sized),
        get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
        request: Pair::Req,
        function_type: DurableFunctionType,
    ) -> Result<Self, WorkerExecutorError>
    where
        T: 'static,
        D: HasData + ?Sized,
        Ctx: WorkerCtx,
    {
        Self::start_access_with_options_and_span_builder(
            store,
            get_ctx,
            function_type,
            AccessClaimOptions::default(),
            async move |_| Ok(request),
            None,
            None,
            true,
        )
        .await
    }

    async fn start_access_with_options_and_span_builder<T, D, Ctx, F>(
        store: &(impl DurableAccess<T, D> + ?Sized),
        get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
        function_type: DurableFunctionType,
        claim_options: AccessClaimOptions,
        build_request: F,
        build_span: Option<BuildSpanCallback>,
        span_observer: Option<Arc<dyn CallSpanObserver>>,
        requires_agent_authority: bool,
    ) -> Result<Self, WorkerExecutorError>
    where
        T: 'static,
        D: HasData + ?Sized,
        Ctx: WorkerCtx,
        F: AsyncFnOnce(AccessStartContext) -> Result<Pair::Req, WorkerExecutorError>,
    {
        Self::start_context_with_capture_and_span_builder(
            store,
            get_ctx,
            function_type,
            claim_options,
            async move |context, _| build_request(context).await,
            build_span,
            span_observer,
            requires_agent_authority,
            requires_agent_authority.then_some(|_: &mut DurableWorkerCtx<Ctx>| ()),
        )
        .await
    }

    /// Captures an admitted view for the request without refreshing it on incomplete replay.
    /// The request must retain any data needed to repair its own incomplete execution.
    pub(crate) async fn start_context_with_agent_authority_capture<T, D, Ctx, R, C, F>(
        store: &(impl DurableAccess<T, D> + ?Sized),
        get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
        function_type: DurableFunctionType,
        capture: C,
        build_request: F,
    ) -> Result<Self, WorkerExecutorError>
    where
        T: 'static,
        D: HasData + ?Sized,
        Ctx: WorkerCtx,
        C: FnMut(&mut DurableWorkerCtx<Ctx>) -> R,
        F: AsyncFnOnce(AccessStartContext, Option<R>) -> Result<Pair::Req, WorkerExecutorError>,
    {
        Self::start_context_with_capture_and_span_builder(
            store,
            get_ctx,
            function_type,
            AccessClaimOptions::default(),
            build_request,
            None,
            None,
            false,
            Some(capture),
        )
        .await
    }

    async fn start_context_with_capture_and_span_builder<T, D, Ctx, R, C, F>(
        store: &(impl DurableAccess<T, D> + ?Sized),
        get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
        function_type: DurableFunctionType,
        claim_options: AccessClaimOptions,
        build_request: F,
        build_span: Option<BuildSpanCallback>,
        span_observer: Option<Arc<dyn CallSpanObserver>>,
        requires_agent_authority: bool,
        capture: Option<C>,
    ) -> Result<Self, WorkerExecutorError>
    where
        T: 'static,
        D: HasData + ?Sized,
        Ctx: WorkerCtx,
        C: FnMut(&mut DurableWorkerCtx<Ctx>) -> R,
        F: AsyncFnOnce(AccessStartContext, Option<R>) -> Result<Pair::Req, WorkerExecutorError>,
    {
        // Capture the immutable task-context snapshot synchronously, before the first await. The
        // store window of the start validates it against worker state and moves the owner into
        // the call's execution scope.
        let identity = AccessCallIdentity::capture(
            store,
            get_ctx,
            Pair::FQFN,
            &function_type,
            &claim_options,
        )?;
        Self::start_identified(
            store,
            get_ctx,
            identity,
            function_type,
            claim_options,
            requires_agent_authority,
            build_request,
            build_span,
            span_observer,
            capture,
        )
        .await
    }

    /// The start of a call whose identity is already captured.
    async fn start_identified<T, D, Ctx, R, C, F, E>(
        store: &(impl DurableAccess<T, D> + ?Sized),
        get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
        identity: AccessCallIdentity,
        function_type: DurableFunctionType,
        claim_options: AccessClaimOptions,
        requires_agent_authority: bool,
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
        let is_live =
            store.with(|mut access| get_ctx(access.data_mut()).state.durable_call_is_live());
        if !is_live {
            process_pending_replay_events_access(store, get_ctx).await?;
        }
        let prepared = store.with(|mut access| {
            let ctx = get_ctx(access.data_mut());
            Self::prepare_access_start(
                ctx,
                function_type,
                claim_options,
                identity.custom_invocation_scope,
                requires_agent_authority,
                identity.invocation_id,
            )
        })?;
        Self::continue_prepared_context(
            store,
            get_ctx,
            PreparedContextStart { prepared },
            build_request,
            build_span,
            span_observer,
            capture,
        )
        .await
    }

    /// Captures historical call ownership synchronously before an owned job is spawned.
    pub(crate) fn prepare_historical_context<T, D, Ctx>(
        store: &(impl DurableAccess<T, D> + ?Sized),
        get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
        function_type: DurableFunctionType,
        mut claim_options: AccessClaimOptions,
    ) -> Result<PreparedContextStart<Pair, P, Ctx>, WorkerExecutorError>
    where
        T: 'static,
        D: HasData + ?Sized,
        Ctx: WorkerCtx,
    {
        claim_options.require_recorded_start = true;
        Self::prepare_context(store, get_ctx, function_type, claim_options)
    }

    /// Captures the identity of a call synchronously, before its start is handed to an owned
    /// job.
    fn prepare_context<T, D, Ctx>(
        store: &(impl DurableAccess<T, D> + ?Sized),
        get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
        function_type: DurableFunctionType,
        claim_options: AccessClaimOptions,
    ) -> Result<PreparedContextStart<Pair, P, Ctx>, WorkerExecutorError>
    where
        T: 'static,
        D: HasData + ?Sized,
        Ctx: WorkerCtx,
    {
        let identity = AccessCallIdentity::capture(
            store,
            get_ctx,
            Pair::FQFN,
            &function_type,
            &claim_options,
        )?;
        let prepared = store.with(|mut access| {
            Self::prepare_access_start(
                get_ctx(access.data_mut()),
                function_type,
                claim_options,
                identity.custom_invocation_scope,
                false,
                identity.invocation_id,
            )
        })?;
        Ok(PreparedContextStart { prepared })
    }

    /// Claims a replay-only accessor call whose durable request must be reconstructed from the
    /// claimed `Start`. If another Store advances the shared cursor to the live tail first, an
    /// incomplete entity Store is transitioned through the owner protocol and the caller is told
    /// to redispatch through its live request-construction path.
    pub(crate) async fn claim_replay_access_with_options<T, D, Ctx>(
        store: &Accessor<T, D>,
        get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
        function_type: DurableFunctionType,
        claim_options: AccessClaimOptions,
    ) -> Result<ReplayAccessStartOutcome<Self>, WorkerExecutorError>
    where
        T: 'static,
        D: HasData + ?Sized,
        Ctx: WorkerCtx,
    {
        let identity = AccessCallIdentity::capture(
            store,
            get_ctx,
            Pair::FQFN,
            &function_type,
            &claim_options,
        )?;
        // A claim made by the guest runs in an owned task for the reason a start does; see
        // [`Self::start_access_with_options`].
        let outcome = if store.has_guest_visible_subtask() {
            let (claimed, receiver) = oneshot::channel();
            let activity =
                store.with(|mut access| get_ctx(access.data_mut()).tail_work_tracker().activity());
            store.spawn(OwnedReplayClaim {
                identity,
                function_type,
                claim_options,
                get_ctx,
                claimed,
                _activity: activity,
            });
            receiver.await.map_err(|_| {
                WorkerExecutorError::runtime("owned durable call claim ended without a result")
            })??
        } else {
            Self::claim_replay_identified(store, get_ctx, identity, function_type, claim_options)
                .await?
        };
        match outcome {
            ReplayAccessStartOutcome::Claimed(mut handle) => {
                handle
                    .supersede_prior_completion_delivery(store, get_ctx)
                    .await?;
                Ok(ReplayAccessStartOutcome::Claimed(handle))
            }
            ReplayAccessStartOutcome::ReplayEnded => Ok(ReplayAccessStartOutcome::ReplayEnded),
        }
    }
}
