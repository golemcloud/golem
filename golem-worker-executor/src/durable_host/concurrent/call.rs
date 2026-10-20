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
use crate::durable_host::LiveAuthorizationPermit;
use crate::durable_host::durability::reserve_invocation_id;
use crate::durable_host::replay_state::{ReplayStartClaimOutcome, StartClaim};
use crate::services::shutdown::join_or_park_at_runtime_shutdown;
use crate::workerctx::ReplayAdmissionStage;
use futures::future::BoxFuture;
use golem_common::SafeDisplay;
use golem_common::model::card::PermissionTarget;
use golem_common::model::entity::{AgentEntity, InvocationExecutionMode, OwnerRuntime};
use golem_common::model::oplog::DurableInvocationId;
use golem_common::model::oplog::payload::types::SerializableToolAdmissionRefusal;
use golem_service_base::model::auth::AuthCtx;
use std::sync::atomic::AtomicBool;

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

pub(super) fn ambient_trap_context<Ctx: WorkerCtx>(
    ctx: &DurableWorkerCtx<Ctx>,
) -> DurableCallTrapContext {
    DurableCallTrapContext {
        retry_from: InFunctionRetryHost::current_retry_point(ctx),
    }
}

/// Compile-time policy describing what happens when a [`DurableCallSession`] is dropped without being
/// explicitly finished or cancelled.
pub trait DropPolicy: Send + 'static {
    fn unfinished_drop(call: DroppedCall, sink: Option<&UnboundedSender<DropEvent>>);

    fn production_drop_sink(
        sink: Option<UnboundedSender<DropEvent>>,
    ) -> Option<UnboundedSender<DropEvent>> {
        let _ = sink;
        None
    }
}

/// Drop policy for calls that may legitimately be cancelled (dropped from a `select!`, etc.).
pub struct Cancellable;

/// Drop policy for idempotent calls whose unfinished live execution should be retried from the
/// committed `Start` during replay instead of recorded as a terminal cancellation.
pub struct LeaveIncompleteOnDrop;

/// Drop policy for calls that must always be finished or explicitly cancelled. Dropping one
/// unfinished is a bug (default-deny).
pub struct NotCancellable;

/// Host-owned span bookkeeping. Notifications do not select call outcomes or affect replay
/// matching; they transfer the tracing ownership of records selected by the ordinary call path.
pub(crate) trait CallSpanObserver: std::fmt::Debug + Send + Sync {
    fn started(self: Arc<Self>, index: OplogIndex, entry: &OplogEntry, replay: bool);
    fn closed(&self, finished: &golem_common::model::oplog::SpanFinished);
}

impl DropPolicy for Cancellable {
    fn production_drop_sink(
        sink: Option<UnboundedSender<DropEvent>>,
    ) -> Option<UnboundedSender<DropEvent>> {
        sink
    }

    fn unfinished_drop(call: DroppedCall, sink: Option<&UnboundedSender<DropEvent>>) {
        if let Some(sink) = sink {
            call.notify_span_closed();
            let _ = sink.send(DropEvent::UnfinishedCancellable { call });
        } else {
            // Production always attaches the worker's dropped-call event sink; a missing sink
            // means a test fixture without one, and the drop is only logged.
            let start_idx = call.start_idx;
            tracing::warn!(
                "durable call {start_idx} dropped unfinished with no drop-event sink attached; no Cancelled entry will be recorded"
            );
        }
    }
}

impl DropPolicy for LeaveIncompleteOnDrop {
    fn production_drop_sink(
        sink: Option<UnboundedSender<DropEvent>>,
    ) -> Option<UnboundedSender<DropEvent>> {
        sink
    }

    fn unfinished_drop(call: DroppedCall, _sink: Option<&UnboundedSender<DropEvent>>) {
        tracing::debug!(
            start_idx = %call.start_idx(),
            "durable call dropped unfinished; leaving committed Start incomplete for replay"
        );
    }
}

impl DropPolicy for NotCancellable {
    fn unfinished_drop(call: DroppedCall, sink: Option<&UnboundedSender<DropEvent>>) {
        if let Some(sink) = sink {
            let _ = sink.send(DropEvent::UnfinishedNotCancellable { call });
        } else if cfg!(debug_assertions) && !std::thread::panicking() {
            let start_idx = call.start_idx;
            panic!("non-cancellable durable call {start_idx} dropped without finish/cancel");
        } else {
            let start_idx = call.start_idx;
            tracing::error!(
                "non-cancellable durable call {start_idx} dropped without finish/cancel"
            );
        }
    }
}

/// The state machine for one durable host call.
///
/// Created by [`DurableCallSession::start`], which eagerly appends the call's `Start` (live) or
/// claims it from the oplog (replay). The call is finished with [`DurableCallSession::complete`]
/// (live) / [`DurableCallSession::replay`] (replay), or cancelled with
/// [`DurableCallSession::cancel`]. All terminal methods consume the session so a call cannot be
/// finished twice; an unfinished drop runs the `P` drop policy.
pub struct DurableCallSession<Pair: HostPayloadPair, P: DropPolicy> {
    pub(super) start_idx: OplogIndex,
    /// Carries the [`DurableCallBoundary`] admitted for this session.
    pub(super) boundary: DurableCallBoundary,
    pub(super) is_live: bool,
    /// `true` when a `Start` entry was actually appended. It is `false` during unpersisted
    /// execution and for replay handles.
    pub(super) persisted: bool,
    /// Tracks the (possibly deferred) blob upload of this call's request payload, started when the
    /// `Start` was reserved. Awaited before the matching `End` / `Cancelled` is appended so an
    /// upload failure surfaces at the call site rather than only at the leaf oplog's commit barrier.
    /// `PendingUpload::already_durable()` (a no-op) for replay handles, unpersisted execution, and
    /// inline requests.
    pub(super) request_upload: PendingUpload,
    /// Replay-side resolver receiver; `Some` only for replay handles.
    pub(super) replay: Option<ReplayCallHandle>,
    pub(super) finished: bool,
    /// Span closed by a guest cancellation of this call, including a dropped host future.
    pub(super) cancellation_span_id: Option<SpanId>,
    pub(super) span_observer: Option<Arc<dyn CallSpanObserver>>,
    /// `true` when this replay handle parked on a recorded terminal that was never delivered to
    /// the guest — a `Cancelled { partial: None }` or an `End` marked `CompletionDiscarded` —
    /// waiting for the deterministic guest to drop it at the same point it did live. Makes that
    /// drop an expected state (debug-logged) rather than an anomaly.
    pub(super) parked_undelivered_replay: bool,
    /// Whether the call was started with [`AccessClaimOptions::host_internal_calls`], which
    /// [`Self::invoke_host_internal_access`] requires of the owner of a host-internal call.
    pub(super) host_internal_calls: bool,
    /// Initiation-time execution metadata owned by this call.
    pub(super) execution_scope: CallExecutionScope,
    /// In-function retry decision logic. Also the home of the call's `DurableFunctionType` and
    /// captured `DurableExecutionState`.
    pub(super) retry: InFunctionRetryController,
    /// The readiness future is Send but need not be Sync; sessions can still be shared by reference.
    pub(super) scope_replay_recovery: std::sync::Mutex<ScopeReplayRecovery>,
    /// Shared signal for process-equivalent executor teardown. An unfinished call dropped after
    /// this signal is intentionally left incomplete for replay rather than treated as guest
    /// cancellation or a host-call programming error.
    pub(super) executor_shutdown: tokio_util::sync::CancellationToken,
    pub(super) runtime_teardown: Arc<dyn Fn() -> bool + Send + Sync>,
    /// Whether switching this call to live execution requires a recovered, synchronized agent
    /// permission-card authority boundary.
    pub(super) requires_agent_authority: bool,
    /// The authority snapshot captured at the same boundary that admitted live execution.
    /// Replay handles acquire it only if an incomplete call switches back to live execution.
    pub(super) agent_auth_ctx: Option<AuthCtx>,
    /// Policy-controlled sink for unfinished drops, from [`DropPolicy::production_drop_sink`]:
    /// the worker's dropped-call event sender for `Cancellable`/`LeaveIncompleteOnDrop`, `None`
    /// for `NotCancellable`. Unit tests attach their own sink to observe drop events.
    pub(super) drop_sink: Option<UnboundedSender<DropEvent>>,
    /// Policy-independent sink for `CleanupAfterTerminal` events: a torn terminal (the completion
    /// future dropped while the spawned terminal append is still pending) must hand its join +
    /// in-flight permit to the next drain even for policies whose `production_drop_sink` is `None`
    /// (e.g. `NotCancellable`).
    pub(super) cleanup_sink: Option<UnboundedSender<DropEvent>>,
    /// Counts this handle as an in-flight live host call while present.
    pub(super) live_call_permit: Option<LiveCallPermit>,
    pub(super) _phantom: PhantomData<(Pair, P)>,
}

#[derive(Debug)]
pub struct LiveCallPermit(Arc<AtomicUsize>);

impl LiveCallPermit {
    pub(crate) fn new(counter: Arc<AtomicUsize>) -> Self {
        counter.fetch_add(1, Ordering::AcqRel);
        Self(counter)
    }
}

impl Clone for LiveCallPermit {
    /// Cloning takes an additional permit on the same counter, so the call stays counted as
    /// in-flight until *every* holder (the handle itself, a queued drop event, a terminal guard)
    /// has been dropped.
    fn clone(&self) -> Self {
        Self::new(self.0.clone())
    }
}

impl Drop for LiveCallPermit {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

#[derive(Debug, Clone)]
pub(super) struct BegunCallExecutionScope {
    pub(super) entity_parent_start_index: Option<OplogIndex>,
    /// Explicit parent, or the calling Store's entity parent, captured at initiation.
    pub(super) parent_start_index: Option<OplogIndex>,
    /// Custom durable invocation whose physical execution this call observes. Captured once at
    /// initiation so asynchronous continuations never depend on ambient task context.
    pub(super) observational_owner: Option<OplogIndex>,
    /// Invocation id of the guest call, recorded in every `Start` of the call; see
    /// [`InvocationIdSource`].
    pub(super) invocation_id: Option<DurableInvocationId>,
}

impl BegunCallExecutionScope {
    pub(super) fn finish(self, start_idx: OplogIndex) -> CallExecutionScope {
        CallExecutionScope {
            entity_parent_start_index: self.entity_parent_start_index,
            retry_from: self
                .observational_owner
                .or(self.parent_start_index)
                .unwrap_or(start_idx),
            durable_scope: self.parent_start_index,
            observational_owner: self.observational_owner,
            invocation_id: self.invocation_id,
        }
    }
}

#[derive(Debug, Clone)]
pub(super) struct CallExecutionScope {
    pub(super) entity_parent_start_index: Option<OplogIndex>,
    /// The retry point owned by this in-flight call: the enclosing durable scope `Start` if present,
    /// otherwise the host-call `Start` itself.
    pub(super) retry_from: OplogIndex,
    /// The enclosing durable scope, if this call belongs to one.
    #[allow(dead_code)]
    pub(super) durable_scope: Option<OplogIndex>,
    /// Custom durable invocation that owns this observational call, if any.
    pub(super) observational_owner: Option<OplogIndex>,
    /// Invocation id of the guest call; see [`BegunCallExecutionScope::invocation_id`].
    pub(super) invocation_id: Option<DurableInvocationId>,
}

impl CallExecutionScope {
    /// The retry point to attach to a trap raised by this call: the owning custom invocation for
    /// an observational call, otherwise its enclosing durable scope `Start` or its own `Start`. This mirrors
    /// [`ScopedRetryHost::retry_point`] so a hard (non-semantic) trap groups exactly like an
    /// inline/semantic retry would.
    pub(super) fn trap_retry_point(&self) -> OplogIndex {
        self.observational_owner.unwrap_or(self.retry_from)
    }
}

pub(crate) struct PreparedContextStart<Pair: HostPayloadPair, P: DropPolicy, Ctx: WorkerCtx> {
    prepared: PreparedAccessStart<Pair, P, Ctx>,
}

/// The request of an accessor call's `Start`.
enum AccessRequest<Req, T: 'static, D: HasData + ?Sized + 'static, E = WorkerExecutorError> {
    /// A request known when the call is made.
    Ready(Req),
    /// A request built between the durable-scope open and the `Start` write or claim; see
    /// [`DurableCallSession::start_access_with`].
    Built(AccessRequestBuilder<Req, T, D, E>),
}

type AccessRequestBuilder<Req, T, D, E> = Box<
    dyn for<'a> FnOnce(AccessStartContext, &'a Accessor<T, D>) -> BoxFuture<'a, Result<Req, E>>
        + Send,
>;

impl<Req, T: 'static, D: HasData + ?Sized + 'static, E> AccessRequest<Req, T, D, E> {
    async fn build(self, context: AccessStartContext, store: &Accessor<T, D>) -> Result<Req, E> {
        match self {
            Self::Ready(request) => Ok(request),
            Self::Built(build) => build(context, store).await,
        }
    }
}

/// The identity of an accessor call, captured in the call that makes it before its first
/// `await`: the custom invocation that observes it and the invocation id its records carry.
struct AccessCallIdentity {
    custom_invocation_scope: Option<CustomInvocationScope>,
    invocation_id: Option<DurableInvocationId>,
}

impl AccessCallIdentity {
    fn capture<T, D, Ctx>(
        store: &(impl DurableAccess<T, D> + ?Sized),
        get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
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
            function_type,
            claim_options,
            custom_invocation_scope.is_some(),
        )?;
        Ok(Self {
            custom_invocation_scope,
            invocation_id,
        })
    }
}

/// How far the guest's call took an owned start before it handed the start to its task.
enum OwnedStartStage<Pair: HostPayloadPair, P: DropPolicy, Ctx: WorkerCtx> {
    /// A live call, prepared in the guest's call.
    Prepared(Box<PreparedContextStart<Pair, P, Ctx>>),
    /// A replayed call. Its task processes the pending replay events before it prepares the call.
    Identified {
        identity: AccessCallIdentity,
        function_type: DurableFunctionType,
        claim_options: Box<AccessClaimOptions>,
    },
}

/// The owned store task that starts an accessor call made by the guest; see
/// [`DurableCallSession::start_access_with_options`]. Its tail-work activity keeps invocation
/// settlement waiting until the `Start` is recorded and, when the guest dropped the call, its
/// session is dropped.
struct OwnedAccessStart<
    Pair: HostPayloadPair,
    P: DropPolicy,
    T: 'static,
    D: HasData + ?Sized + 'static,
    Ctx: WorkerCtx,
    E,
> {
    stage: OwnedStartStage<Pair, P, Ctx>,
    get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
    request: AccessRequest<Pair::Req, T, D, E>,
    build_span: Option<BuildSpanCallback>,
    span_observer: Option<Arc<dyn CallSpanObserver>>,
    started: oneshot::Sender<Result<DurableCallSession<Pair, P>, E>>,
    _activity: crate::durable_host::tail_work::TailActivity,
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

/// The owned store task of [`DurableCallSession::claim_replay_access_with_options`] for a claim
/// made by the guest. Like [`OwnedAccessStart`], it always claims the recorded `Start`.
struct OwnedReplayClaim<Pair: HostPayloadPair, P: DropPolicy, T: 'static, Ctx: WorkerCtx> {
    identity: AccessCallIdentity,
    function_type: DurableFunctionType,
    claim_options: AccessClaimOptions,
    get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
    claimed: oneshot::Sender<
        Result<ReplayAccessStartOutcome<DurableCallSession<Pair, P>>, WorkerExecutorError>,
    >,
    _activity: crate::durable_host::tail_work::TailActivity,
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
    let result = tokio::select! {
        result = start => result,
        interruption = interrupt => {
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

struct PreparedAccessStart<Pair: HostPayloadPair, P: DropPolicy, Ctx: WorkerCtx> {
    is_live: bool,
    unpersisted: bool,
    requires_agent_authority: bool,
    oplog: Arc<dyn Oplog>,
    public_state: PublicDurableWorkerState<Ctx>,
    primary_runtime: bool,
    replaying_incomplete_entity: bool,
    tool_entity: bool,
    tool_operation: Option<crate::durable_host::tool::operation::OwnerToolOperation>,
    admission_boundary: Option<SerializableToolAdmissionRefusal>,
    local_live_tail: Arc<AtomicBool>,
    replay_state: crate::durable_host::replay_state::ReplayState,
    linear_memory: crate::services::linear_memory::LinearMemoryTracker,
    execution_scope: BegunCallExecutionScope,
    retry: InFunctionRetryController,
    drop_sink: Option<UnboundedSender<DropEvent>>,
    cleanup_sink: Option<UnboundedSender<DropEvent>>,
    claim_options: AccessClaimOptions,
    /// The worker's in-flight live host call counter, kept so `execute_access_start` can take a
    /// permit when a replayed scope switches to live mid-start.
    live_host_calls: Arc<AtomicUsize>,
    /// Taken at prepare time — synchronously with worker state, *before* any oplog entry of this
    /// call is appended — so boundary checks can never observe zero calls between this call's
    /// `Start` append and its handle construction in `finish_access_start`. `Some` for live calls,
    /// `None` on replay.
    live_call_permit: Option<LiveCallPermit>,
    _phantom: PhantomData<(Pair, P)>,
}

impl<Pair: HostPayloadPair, P: DropPolicy, Ctx: WorkerCtx> PreparedAccessStart<Pair, P, Ctx> {
    /// Whether this Store itself already continued live locally: an incomplete entity that
    /// switched to live while the shared cursor may still replay other owners' records. Such a
    /// Store still claims while unclaimed retained `Start`s exist (see
    /// [`ReplayState::claim_start_for_store`]).
    fn store_continued_live(&self) -> bool {
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

async fn publish_incomplete_replay_tail_access<T, D, Ctx>(
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

async fn prepare_access_replay_continuation<Pair, T, D, Ctx>(
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

pub(super) async fn publish_incomplete_replay_tail_at<Ctx: WorkerCtx>(
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

/// Options of a concurrent accessor call that shape its durable records and their replay claim.
///
/// Accessor host calls run concurrently, so their oplog `Start` entries are appended in
/// scheduling order, which replay does not reproduce. Replay pairs each call with its own records
/// by the invocation id the records carry (see [`InvocationIdSource`]).
#[derive(Default)]
pub(crate) struct AccessClaimOptions {
    /// The function of the `Start` a tool invocation records instead of its accepted entity
    /// invocation `Start` when it rejects the call before dispatch. Replay claims either one, as a
    /// historical reconstruction claim.
    pub(crate) tool_rejection_function_name: Option<HostFunctionName>,
    /// Explicit durable parent. When set, it overrides the calling Store's entity parent.
    pub(crate) parent_start_index: Option<OplogIndex>,
    /// Ownership propagated from a resource created by an earlier observational call. When absent,
    /// initiation captures the current custom-invocation task context.
    pub(crate) observational_owner: Option<OplogIndex>,
    pub(crate) scope_replay_recovery: ScopeReplayRecovery,
    /// Set by a durable scope owner that re-executes its effect call-locally (see
    /// [`ScopeReplayRecovery::Reexecute`]) on the children it records inside that scope: a child
    /// whose `Start` is recorded without a terminal is then completed live under that `Start`.
    pub(crate) incomplete_repair_authorized_by_scope: bool,
    pub(crate) host_internal_owner: Option<HostInternalCallOwner>,
    pub(crate) require_recorded_start: bool,
    /// The call makes host-internal calls under its own `Start` (see
    /// [`DurableCallSession::invoke_host_internal_access`]). When the call's recorded terminal is
    /// a `Cancelled` without a partial result, the replay claim drains their records, because a
    /// replayed call parks at that terminal and never makes them again.
    pub(crate) host_internal_calls: bool,
    /// The invocation id this call records in its `Start`s and claims them by on replay.
    pub(crate) invocation_id: InvocationIdSource,
    /// The denial a live call records in its `Start` (see [`OplogEntry::Start`]). A call that
    /// claims a recorded `Start` ignores it and uses the recorded denial.
    pub(crate) denial: Option<String>,
    pub(crate) permission_check: Option<AccessPermissionCheck>,
}

/// Authorization owned by call initiation, evaluated only after a claim resolves to new work.
pub(crate) struct AccessPermissionCheck {
    targets: Result<Vec<PermissionTarget>, String>,
    decision: oneshot::Sender<Option<Result<LiveAuthorizationPermit, String>>>,
}

impl AccessPermissionCheck {
    pub(crate) fn new(
        targets: Result<Vec<PermissionTarget>, String>,
    ) -> (
        Self,
        oneshot::Receiver<Option<Result<LiveAuthorizationPermit, String>>>,
    ) {
        let (decision, receive) = oneshot::channel();
        (Self { targets, decision }, receive)
    }
}

impl AccessClaimOptions {
    fn authorizes_incomplete_repair(&self, entity_parent: Option<OplogIndex>) -> bool {
        // Entity lineage records ancestry, not a protocol parent. An explicit owner must
        // decide repair even when its index happens to equal the entity's Start.
        self.incomplete_repair_authorized_by_scope
            || (self.parent_start_index.is_none()
                && self.host_internal_owner.is_none()
                && entity_parent.is_some())
    }
}

/// Where an accessor call gets the invocation id it records in its `Start`s.
///
/// A guest call that reaches the durable call path through a concurrent guest import records the
/// invocation id of that import (see [`reserve_invocation_id`]), and replay claims its `Start`s
/// by that id. Calls observed inside a custom durable invocation record none: replay never claims
/// them.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) enum InvocationIdSource {
    /// The call reserves the id of its guest import when it starts. This is correct only when the
    /// import has made no `await` that lets the guest make another import call first.
    #[default]
    AtStart,
    /// The id reserved earlier by the guest import that issued this call, with
    /// [`reserve_access_invocation_id`] or [`reserve_import_invocation_id`]. `None` means the
    /// import is not identified by an invocation id.
    Reserved(Option<DurableInvocationId>),
}

impl InvocationIdSource {
    /// The reserved invocation id, or `None` when the call reserves its id at start.
    pub(crate) fn reserved(self) -> Option<DurableInvocationId> {
        match self {
            Self::AtStart => None,
            Self::Reserved(invocation_id) => invocation_id,
        }
    }
}

/// Reserves the invocation id of the concurrent guest import that `store` belongs to. Host
/// functions call it before their first `await` when they `await` before starting their durable
/// call, or when a task they spawn makes the durable call.
pub(crate) fn reserve_access_invocation_id<T, D, Ctx>(
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
    reserve_guest_import_invocation_id(store, get_ctx, observed_by_custom_invocation)
        .map(InvocationIdSource::Reserved)
}

/// Reserves the invocation id of the concurrent guest import that `store` belongs to, for a host
/// function that `await`s before it starts its first durable call. The durable calls the import
/// starts later use this id.
pub(crate) fn reserve_invocation_id_before_await<T, D, Ctx>(
    store: &Accessor<T, D>,
    get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
) -> Result<(), WorkerExecutorError>
where
    T: 'static,
    D: HasData + ?Sized,
    Ctx: WorkerCtx,
{
    reserve_access_invocation_id(store, get_ctx).map(|_| ())
}

/// Reserves the invocation id of a synchronous guest import, which runs to completion inside the
/// guest's call and so needs no import order check. A task the import spawns passes the result to
/// its durable call.
pub(crate) fn reserve_import_invocation_id<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    observational_owner: Option<OplogIndex>,
) -> Result<InvocationIdSource, WorkerExecutorError> {
    if observational_owner.is_some() || ctx.state.snapshotting_mode {
        return Ok(InvocationIdSource::Reserved(None));
    }
    let Some(anchor) = ctx.invocation_anchor() else {
        return Ok(InvocationIdSource::Reserved(None));
    };
    let invocation_id = reserve_invocation_id(ctx, None, anchor)?;
    ctx.record_deferred_import(invocation_id);
    Ok(InvocationIdSource::Reserved(Some(invocation_id)))
}

pub(super) fn reserve_guest_import_invocation_id<T, D, Ctx>(
    store: &Accessor<T, D>,
    get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
    observed_by_custom_invocation: bool,
) -> Result<Option<DurableInvocationId>, WorkerExecutorError>
where
    T: 'static,
    D: HasData + ?Sized,
    Ctx: WorkerCtx,
{
    if observed_by_custom_invocation || !store.has_guest_visible_subtask() {
        return Ok(None);
    }
    let activity = store.runtime_activity();
    store.with(|mut access| {
        let ctx = get_ctx(access.data_mut());
        if ctx.state.snapshotting_mode {
            return Ok(None);
        }
        let Some(anchor) = ctx.invocation_anchor() else {
            return Ok(None);
        };
        reserve_invocation_id(ctx, activity, anchor).map(Some)
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
    _function_type: &DurableFunctionType,
    claim_options: &AccessClaimOptions,
    observed_by_custom_invocation: bool,
) -> Result<Option<DurableInvocationId>, WorkerExecutorError>
where
    T: 'static,
    D: HasData + ?Sized,
    Ctx: WorkerCtx,
{
    if let InvocationIdSource::Reserved(invocation_id) = claim_options.invocation_id {
        return Ok(invocation_id);
    }
    if let Some(owner) = &claim_options.host_internal_owner {
        return Ok(owner.execution_scope.invocation_id);
    }
    store.reserve_call_invocation_id(
        get_ctx,
        observed_by_custom_invocation || claim_options.observational_owner.is_some(),
    )
}

/// An internal call belongs to the outer call, not to a guest completion. Keeping the outer
/// session alive covers cancellation even after the internal result has been consumed.
pub(crate) struct HostInternalCallOwner {
    start_index: OplogIndex,
    execution_scope: CallExecutionScope,
    declared: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum AccessResultDestination {
    Guest,
    Host,
}

type ScopeReplayReadiness = futures::future::Shared<
    Pin<Box<dyn Future<Output = Result<ScopeReplayDecision, WorkerExecutorError>> + Send>>,
>;

#[derive(Clone, Copy)]
pub(crate) enum ScopeReplayDecision {
    Reexecute,
    CompleteWithLoss,
}

#[derive(Clone, Default)]
pub(crate) enum ScopeReplayRecovery {
    #[default]
    Default,
    /// The owner completes the original call with typed protocol loss, without a new effect.
    CompleteWithLoss,
    Reexecute {
        readiness: ScopeReplayReadiness,
        loss_may_start_child: bool,
    },
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

    pub(super) fn for_child(&self) -> Self {
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

    fn allows_new_call(&self) -> bool {
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

    async fn await_readiness(&mut self) -> Result<(), WorkerExecutorError> {
        match self {
            Self::Reexecute { readiness, .. } => readiness.clone().await.map(|_| ()),
            Self::Default | Self::CompleteWithLoss => Ok(()),
        }
    }

    async fn await_new_call_readiness<Pair: HostPayloadPair>(
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

#[cfg(test)]
mod scope_replay_recovery_tests {
    use super::{
        AccessClaimOptions, CallExecutionScope, HostInternalCallOwner, ScopeReplayRecovery,
    };
    use golem_common::model::oplog::OplogIndex;
    use test_r::test;

    #[test]
    fn entity_lineage_does_not_authorize_explicit_protocol_children() {
        let entity = OplogIndex::from_u64(17);
        let mut options = AccessClaimOptions::default();
        assert!(options.authorizes_incomplete_repair(Some(entity)));
        assert!(!options.authorizes_incomplete_repair(None));
        options.parent_start_index = Some(entity);
        assert!(!options.authorizes_incomplete_repair(Some(entity)));
        options.incomplete_repair_authorized_by_scope = true;
        assert!(options.authorizes_incomplete_repair(Some(entity)));

        options.incomplete_repair_authorized_by_scope = false;
        options.parent_start_index = None;
        options.host_internal_owner = Some(HostInternalCallOwner {
            start_index: entity,
            execution_scope: CallExecutionScope {
                entity_parent_start_index: Some(entity),
                retry_from: entity,
                durable_scope: Some(entity),
                observational_owner: None,
                invocation_id: None,
            },
            declared: false,
        });
        assert!(!options.authorizes_incomplete_repair(Some(entity)));
        options.incomplete_repair_authorized_by_scope = true;
        assert!(options.authorizes_incomplete_repair(Some(entity)));
    }

    #[test]
    fn reexecute_policy_authorizes_existing_incomplete_scope() {
        let recovery = ScopeReplayRecovery::reexecute_when(async { Ok(()) });

        assert!(recovery.allows_new_call());
        assert!(ScopeReplayRecovery::Default.allows_new_call());
    }

    #[test]
    fn typed_loss_requires_an_existing_start() {
        assert!(!ScopeReplayRecovery::CompleteWithLoss.allows_new_call());
    }

    fn replaying_call(
        recovery: ScopeReplayRecovery,
        child_authorized: Option<bool>,
    ) -> super::DurableCallSession<
        golem_common::model::oplog::host_functions::MonotonicClockNow,
        super::LeaveIncompleteOnDrop,
    > {
        use super::*;
        let index = OplogIndex::from_u64(5);
        DurableCallSession::<
            golem_common::model::oplog::host_functions::MonotonicClockNow,
            LeaveIncompleteOnDrop,
        > {
            start_idx: index,
            boundary: DurableCallBoundary::from_begin_index(index),
            is_live: false,
            persisted: false,
            request_upload: PendingUpload::already_durable(),
            replay: None,
            finished: false,
            cancellation_span_id: None,
            span_observer: None,
            parked_undelivered_replay: false,
            host_internal_calls: false,
            execution_scope: CallExecutionScope {
                retry_from: index,
                durable_scope: None,
                observational_owner: None,
                entity_parent_start_index: None,
                invocation_id: None,
            },
            retry: InFunctionRetryController::new(
                DurableFunctionType::ReadLocal,
                DurableExecutionState {
                    is_live: false,
                    snapshotting_mode: false,
                    max_in_function_retry_delay: std::time::Duration::ZERO,
                },
                "test:monotonic_clock::now",
            )
            .with_parent_start_index(child_authorized.map(|_| OplogIndex::from_u64(2)))
            .with_incomplete_repair_authorized_by_scope(child_authorized.unwrap_or(false)),
            scope_replay_recovery: recovery.into(),
            executor_shutdown: tokio_util::sync::CancellationToken::new(),
            runtime_teardown: Arc::new(|| false),
            requires_agent_authority: false,
            agent_auth_ctx: None,
            drop_sink: None,
            cleanup_sink: None,
            live_call_permit: None,
            _phantom: PhantomData,
        }
    }

    #[test]
    async fn incomplete_repair_readiness_blocks_snapshot_boundary() {
        use super::*;
        let counter = Arc::new(AtomicUsize::new(0));
        let (ready, readiness) = tokio::sync::oneshot::channel();
        let mut call = replaying_call(
            ScopeReplayRecovery::reexecute_when(async { readiness.await.unwrap() }),
            None,
        );
        let mut repair = Box::pin(call.prepare_incomplete_live_repair(|| counter.clone()));
        assert!(
            std::future::poll_fn(|cx| std::task::Poll::Ready(repair.as_mut().poll(cx)))
                .await
                .is_pending()
        );
        assert!(
            !crate::durable_host::PrivateDurableWorkerState::clean_checkpoint_boundary(
                false,
                counter.load(Ordering::Acquire) > 0,
                false,
            ),
            "repair waiting for readiness must block snapshot/checkpoint admission"
        );
        ready
            .send(Err(WorkerExecutorError::recovery_required(
                "recording unavailable",
            )))
            .unwrap();
        assert!(matches!(
            repair.await,
            Err(WorkerExecutorError::RecoveryRequired { .. })
        ));
        assert!(!call.is_live());
        assert!(call.finished);
        assert_eq!(counter.load(Ordering::Acquire), 0);
    }

    #[test]
    async fn demand_children_share_the_parent_readiness_decision() {
        use super::*;
        use golem_common::model::oplog::host_functions::MonotonicClockNow;
        for unavailable in [false, true] {
            let (ready, readiness) = tokio::sync::oneshot::channel();
            let mut parent = replaying_call(
                ScopeReplayRecovery::reexecute_when(async { readiness.await.unwrap() }),
                None,
            );
            let mut child = replaying_call(parent.child_replay_recovery(), Some(true));
            let mut new_child = parent.child_replay_recovery();
            let counter = Arc::new(AtomicUsize::new(0));
            let mut repair = Box::pin(child.prepare_incomplete_live_repair(|| counter.clone()));
            let mut begin = Box::pin(new_child.await_new_call_readiness::<MonotonicClockNow>());
            assert!(futures::poll!(&mut repair).is_pending());
            assert!(futures::poll!(&mut begin).is_pending());
            assert!(!parent.is_live());
            ready
                .send(if unavailable {
                    Err(WorkerExecutorError::recovery_required(
                        "payload unavailable",
                    ))
                } else {
                    Ok(())
                })
                .unwrap();
            assert_eq!(repair.await.is_err(), unavailable);
            assert_eq!(begin.await.is_err(), unavailable);
            assert_eq!(child.is_live(), !unavailable);
            assert_eq!(
                parent
                    .prepare_incomplete_live_repair(|| counter.clone())
                    .await
                    .is_err(),
                unavailable
            );
            assert_eq!(parent.is_live(), !unavailable);
            parent.abandon_for_trap();
            child.abandon_for_trap();
        }
    }

    #[test]
    async fn deferred_typed_loss_allows_only_the_existing_parent_and_its_children() {
        use super::*;
        use golem_common::model::oplog::host_functions::MonotonicClockNow;
        let mut recovery =
            ScopeReplayRecovery::decide_when(async { Ok(ScopeReplayDecision::CompleteWithLoss) });
        let mut child = recovery.for_child();
        child
            .await_new_call_readiness::<MonotonicClockNow>()
            .await
            .unwrap();
        recovery.await_readiness().await.unwrap();
        assert!(
            recovery
                .await_new_call_readiness::<MonotonicClockNow>()
                .await
                .unwrap_err()
                .to_string()
                .contains("without a recorded Start")
        );
    }

    #[test]
    async fn typed_owner_loss_requires_child_authorization() {
        use super::*;
        let counter = Arc::new(AtomicUsize::new(0));
        let mut unauthorized = replaying_call(ScopeReplayRecovery::CompleteWithLoss, Some(false));
        let error = unauthorized
            .prepare_incomplete_live_repair(|| counter.clone())
            .await
            .unwrap_err();
        assert!(error.to_string().contains("no owner authorization"));
        assert!(!unauthorized.is_live());
        assert_eq!(counter.load(Ordering::Acquire), 0);

        let mut authorized = replaying_call(ScopeReplayRecovery::CompleteWithLoss, Some(true));
        authorized
            .prepare_incomplete_live_repair(|| counter.clone())
            .await
            .unwrap();
        assert!(authorized.is_live());
        assert_eq!(authorized.begin_index(), OplogIndex::from_u64(5));
        authorized.abandon_for_trap();
    }
}

struct ExecutedAccessStart<Pair: HostPayloadPair, P: DropPolicy> {
    begin_index: OplogIndex,
    start_idx: OplogIndex,
    persisted: bool,
    requires_agent_authority: bool,
    agent_auth_ctx: Option<AuthCtx>,
    request_upload: PendingUpload,
    replay: Option<ReplayCallHandle>,
    execution_scope: CallExecutionScope,
    retry: InFunctionRetryController,
    scope_replay_recovery: ScopeReplayRecovery,
    drop_sink: Option<UnboundedSender<DropEvent>>,
    cleanup_sink: Option<UnboundedSender<DropEvent>>,
    /// The in-flight permit taken at prepare time (or when a replayed scope switched to live),
    /// handed on to the [`DurableCallSession`] so the call stays counted continuously from before its
    /// first oplog append until its terminal is recorded.
    live_call_permit: Option<LiveCallPermit>,
    _phantom: PhantomData<(Pair, P)>,
}

/// Context handed to the request builder of [`DurableCallSession::start_access_with`], before the
/// host-call `Start` is written.
pub(crate) struct AccessStartContext {
    /// Whether the call executes live (the built request will be persisted) or replays (the built
    /// request is discarded; the builder still runs for its positional replay side effects).
    pub(crate) is_live: bool,
    pub(crate) is_denied: bool,
}

pub(crate) enum ReplayAccessStartOutcome<H> {
    Claimed(H),
    ReplayEnded,
}

/// Builds a call's span metadata from the assigned durable `Start` index during the oplog's
/// serialized writer step.
pub(crate) type BuildSpanCallback = Box<
    dyn FnOnce(OplogIndex) -> Result<golem_common::model::oplog::SpanStarted, WorkerExecutorError>
        + Send,
>;

#[allow(clippy::large_enum_variant)]
pub(crate) enum ResolvedCall<Pair: HostPayloadPair, P: DropPolicy> {
    Replay(DurableCallSession<Pair, P>),
    Live(BegunCall<Pair, P>),
}

fn is_write_side_effect_for_access(function_type: &DurableFunctionType) -> bool {
    matches!(
        function_type,
        DurableFunctionType::WriteLocal | DurableFunctionType::WriteRemote
    )
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

struct ContextRetryHost<'a, Ctx: WorkerCtx> {
    task: TaskRetryContext<Ctx>,
    execution_state: &'a (dyn Fn() -> DurableExecutionState + Send + Sync),
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

pub(super) struct ScopedRetryHost<'a, H> {
    inner: &'a mut H,
    execution_scope: &'a CallExecutionScope,
}

impl<'a, H> ScopedRetryHost<'a, H> {
    pub(super) fn new(inner: &'a mut H, execution_scope: &'a CallExecutionScope) -> Self {
        Self {
            inner,
            execution_scope,
        }
    }

    fn retry_point(&self) -> OplogIndex {
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
    ) -> Result<OplogIndex, WorkerExecutorError> {
        self.inner
            .begin_durable_function(function_type, host_function)
            .await
    }

    async fn begin_durable_function_with_span(
        &mut self,
        function_type: &DurableFunctionType,
        host_function: &str,
        span_started: golem_common::model::oplog::SpanStarted,
    ) -> Result<(OplogIndex, golem_common::model::oplog::SpanStarted), WorkerExecutorError> {
        self.inner
            .begin_durable_function_with_span(function_type, host_function, span_started)
            .await
    }

    async fn end_durable_function(
        &mut self,
        function_type: &DurableFunctionType,
        begin_index: OplogIndex,
        forced_commit: bool,
    ) -> Result<(), WorkerExecutorError> {
        self.inner
            .end_durable_function(function_type, begin_index, forced_commit)
            .await
    }

    async fn end_durable_function_with_span(
        &mut self,
        function_type: &DurableFunctionType,
        begin_index: OplogIndex,
        forced_commit: bool,
        span_finished: golem_common::model::oplog::SpanFinished,
    ) -> Result<(), WorkerExecutorError> {
        self.inner
            .end_durable_function_with_span(
                function_type,
                begin_index,
                forced_commit,
                span_finished,
            )
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

    /// Starts and drives a re-executable value call through live execution, completed replay, or
    /// incomplete replay repair. This is the default entry point for callers that do not need the
    /// begin index or session-owned retry/authority state while performing the live action.
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
        let identity = AccessCallIdentity::capture(store, get_ctx, &function_type, &claim_options)?;
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

    /// Drops a session whose owned start finished after the guest dropped the call. A live
    /// session runs its drop policy. A replayed session is dropped at the point the guest
    /// dropped the call in the recorded run, which is expected.
    fn discard_after_guest_drop(mut self) {
        if !self.is_live {
            self.parked_undelivered_replay = true;
        }
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
        let identity = AccessCallIdentity::capture(store, get_ctx, &function_type, &claim_options)?;
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
        let identity = AccessCallIdentity::capture(store, get_ctx, &function_type, &claim_options)?;
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

    async fn continue_prepared_context<T, D, Ctx, R, C, F, E>(
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
        let identity = AccessCallIdentity::capture(store, get_ctx, &function_type, &claim_options)?;
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

    /// The claim of [`Self::claim_replay_access_with_options`] once the call's identity is
    /// captured.
    async fn claim_replay_identified<T, D, Ctx>(
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
    async fn supersede_prior_completion_delivery<T, D, Ctx>(
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

    fn prepare_access_start<Ctx: WorkerCtx>(
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
    fn replay_start_claim(
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

    async fn execute_access_start<T, D, Ctx, R, C, F, E>(
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
                let outcome = prepared
                    .replay_state
                    .claim_start_for_store_with_admission(
                        claim.clone(),
                        store_continued_live,
                        move |index, entry| {
                            if let Some((observer, _activity)) = &observer {
                                observer.clone().started(index, entry, true);
                            }
                        },
                        tool_operation
                            .clone()
                            .map(|operation| (operation, Pair::FQFN.to_string())),
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
                let (send, receive) = tokio::sync::oneshot::channel();
                let _ = tasks.spawn_metadata(async move {
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
                    let _ = send.send(result);
                });
                let (start_idx, request_upload) = receive
                    .await
                    .map_err(|_| WorkerExecutorError::Interrupted {
                        kind: InterruptKind::Jump,
                    })
                    .and_then(|result| result)?;
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

    fn finish_access_start<Ctx: WorkerCtx>(
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

    pub fn is_live(&self) -> bool {
        self.is_live
    }

    pub fn start_index(&self) -> OplogIndex {
        self.start_idx
    }

    /// The invocation id recorded in the `Start`s of this call.
    pub(crate) fn invocation_id(&self) -> Option<DurableInvocationId> {
        self.execution_scope.invocation_id
    }

    pub(crate) fn is_persisted(&self) -> bool {
        self.persisted || !self.is_live
    }

    pub(crate) fn take_historical_reconstruction(&mut self) -> Option<HistoricalReconstruction> {
        self.replay
            .as_mut()
            .and_then(ReplayCallHandle::take_historical_reconstruction)
    }

    pub async fn recorded_request<Ctx: WorkerCtx>(
        &self,
        ctx: &DurableWorkerCtx<Ctx>,
    ) -> Result<Pair::Req, WorkerExecutorError>
    where
        Pair::Req: TryFrom<HostRequest, Error = String>,
    {
        let entry = ctx.state.oplog.read(self.start_idx).await?;
        let OplogEntry::Start {
            request: Some(request),
            ..
        } = entry
        else {
            return Err(WorkerExecutorError::unexpected_oplog_entry(
                format!("Start with request at {}", self.start_idx),
                format!("{entry:?}"),
            ));
        };
        let request = ctx
            .state
            .oplog
            .download_payload(request)
            .await
            .map_err(|error| {
                WorkerExecutorError::runtime(format!(
                    "failed to load durable call request at {}: {error}",
                    self.start_idx
                ))
            })?;
        Pair::Req::try_from(request).map_err(|actual| {
            WorkerExecutorError::unexpected_oplog_entry(Pair::FQFN.to_string(), actual)
        })
    }

    /// Returns tracing metadata carried by the claimed `Start`. This is deliberately loaded only
    /// after the ordinary call claim has selected the operation; tracing metadata never
    /// participates in request matching or cursor routing.
    pub async fn recorded_span_started<Ctx: WorkerCtx>(
        &self,
        ctx: &DurableWorkerCtx<Ctx>,
    ) -> Result<Option<golem_common::model::oplog::SpanStarted>, WorkerExecutorError> {
        match ctx.state.oplog.read(self.start_idx).await? {
            OplogEntry::Start { span_started, .. } => Ok(span_started.map(|span| *span)),
            entry => Err(WorkerExecutorError::unexpected_oplog_entry(
                format!("Start at {}", self.start_idx),
                format!("{entry:?}"),
            )),
        }
    }

    pub(crate) async fn recorded_span_started_access<T, D, Ctx>(
        &self,
        store: &Accessor<T, D>,
        get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
    ) -> Result<Option<golem_common::model::oplog::SpanStarted>, WorkerExecutorError>
    where
        T: 'static,
        D: HasData + ?Sized,
        Ctx: WorkerCtx,
    {
        let oplog = store.with(|mut access| get_ctx(access.data_mut()).state.oplog.clone());
        match oplog.read(self.start_idx).await? {
            OplogEntry::Start { span_started, .. } => Ok(span_started.map(|span| *span)),
            entry => Err(WorkerExecutorError::unexpected_oplog_entry(
                format!("Start at {}", self.start_idx),
                format!("{entry:?}"),
            )),
        }
    }

    /// The boundary index returned by `begin_durable_function`.
    /// Used by call sites that derive a stable identifier from that index (e.g. the idempotency-key
    /// derivation).
    pub fn begin_index(&self) -> OplogIndex {
        self.boundary.begin_index()
    }

    pub(crate) fn require_incomplete_repair_readiness(
        &mut self,
        readiness: impl Future<Output = Result<(), WorkerExecutorError>> + Send + 'static,
    ) {
        *self.scope_replay_recovery.get_mut().unwrap() =
            ScopeReplayRecovery::reexecute_when(readiness);
    }

    pub(super) fn child_replay_recovery(&self) -> ScopeReplayRecovery {
        self.scope_replay_recovery.lock().unwrap().for_child()
    }

    pub fn agent_auth_ctx(&self) -> &AuthCtx {
        self.agent_auth_ctx
            .as_ref()
            .expect("agent authority context requested from a call without live agent authority")
    }

    pub fn take_agent_auth_ctx(&mut self) -> AuthCtx {
        self.agent_auth_ctx
            .take()
            .expect("agent authority context requested from a call without live agent authority")
    }

    pub(crate) fn observational_owner(&self) -> Option<OplogIndex> {
        self.execution_scope.observational_owner
    }

    pub(crate) fn is_observational(&self) -> bool {
        self.observational_owner().is_some()
    }

    pub(crate) fn close_span_on_cancellation(&mut self, span_id: SpanId) {
        self.cancellation_span_id = Some(span_id);
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

    /// Snapshots everything a cancellation recorder / terminal guard needs from this live
    /// persisted call into a self-contained [`DroppedCall`]. `Drop` cannot await, use an
    /// `Accessor`, or borrow worker state, so the snapshot must carry every call-owned fact
    /// needed for the deferred oplog/state work. `live_call_permit` is passed explicitly
    /// because ownership differs by site: terminal paths clone it (the handle stays alive with
    /// its own permit until the terminal completes), while `Drop` moves it into the drop event
    /// so the call stays counted as in-flight until the event is drained.
    /// Snapshot of the two teardown signals (process-equivalent executor shutdown and the
    /// owning runtime's store teardown) for guards and delivery tokens that may outlive the
    /// session.
    fn teardown_probe(&self) -> TeardownProbe {
        TeardownProbe::new(
            self.executor_shutdown.clone(),
            self.runtime_teardown.clone(),
        )
    }

    fn dropped_call_snapshot(&self, live_call_permit: Option<LiveCallPermit>) -> DroppedCall {
        DroppedCall {
            start_idx: self.start_idx,
            begin_index: self.begin_index(),
            function_type: self.retry.function_type().clone(),
            request_upload: self.request_upload.clone(),
            trap_context: self.trap_context(),
            live_call_permit,
            span_observer: self.span_observer.clone(),
            span_finished: self.cancellation_span_id.as_ref().map(|span_id| {
                golem_common::model::oplog::SpanFinished {
                    span_id: span_id.clone(),
                    finished_at: Timestamp::now_utc(),
                    outcome: golem_common::model::oplog::SpanOutcome::Cancelled,
                }
            }),
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
    /// inside the crate for calls that derive identity from the begin index or own custom retry
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
        let call = Self::start_access(store, get_ctx, request, function_type).await?;
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

    /// Completes a live call: upload the response, append the matching host-call `End`, then close
    /// the durable scope / commit / checkpoint via `end_durable_function`.
    ///
    /// A terminal failure is returned as a [`TerminalCallError`] carrying this call's own
    /// [`DurableCallTrapContext`], so the post-trap retry grouping stays owned by the call rather
    /// than falling back to ambient worker state a sibling could have clobbered once durable calls
    /// overlap.
    pub(crate) async fn complete<Ctx: WorkerCtx>(
        self,
        ctx: &mut impl DurableContext<Ctx>,
        response: Pair::Resp,
    ) -> Result<Pair::Resp, TerminalCallError> {
        self.complete_drained(ctx, response, None).await
    }

    async fn complete_drained<Ctx: WorkerCtx>(
        mut self,
        ctx: &mut impl DurableContext<Ctx>,
        response: Pair::Resp,
        span_finished: Option<golem_common::model::oplog::SpanFinished>,
    ) -> Result<Pair::Resp, TerminalCallError> {
        let context = self.trap_context();
        if let Err(err) = ctx.drain().await {
            self.abandon_for_trap();
            return Err(err);
        }
        self.complete_context(ctx, response, span_finished, None, false)
            .await
            .map_err(|source| TerminalCallError::new(source, context))
    }

    pub async fn complete_with_span<Ctx: WorkerCtx>(
        mut self,
        ctx: &mut DurableWorkerCtx<Ctx>,
        response: Pair::Resp,
        span_finished: golem_common::model::oplog::SpanFinished,
    ) -> Result<Pair::Resp, TerminalCallError> {
        let context = self.trap_context();
        if let Err(err) = drain_queued_dropped_call_events(ctx).await {
            self.abandon_for_trap();
            return Err(err);
        }
        self.complete_impl(ctx, response, Some(span_finished), None, false)
            .await
            .map_err(|source| TerminalCallError::new(source, context))
    }

    pub async fn complete_with_span_attributes<Ctx: WorkerCtx>(
        mut self,
        ctx: &mut DurableWorkerCtx<Ctx>,
        response: Pair::Resp,
        span_attributes: golem_common::model::oplog::SpanAttributes,
    ) -> Result<Pair::Resp, TerminalCallError> {
        let context = self.trap_context();
        if let Err(err) = drain_queued_dropped_call_events(ctx).await {
            self.abandon_for_trap();
            return Err(err);
        }
        self.complete_impl(ctx, response, None, Some(span_attributes), false)
            .await
            .map_err(|source| TerminalCallError::new(source, context))
    }

    async fn complete_impl<Ctx: WorkerCtx>(
        self,
        ctx: &mut DurableWorkerCtx<Ctx>,
        response: Pair::Resp,
        span_finished: Option<golem_common::model::oplog::SpanFinished>,
        span_attributes: Option<golem_common::model::oplog::SpanAttributes>,
        forced_commit: bool,
    ) -> Result<Pair::Resp, WorkerExecutorError> {
        self.complete_context(ctx, response, span_finished, span_attributes, forced_commit)
            .await
    }

    /// Records a direct host-return terminal without a completion delivery token or marker.
    pub(crate) async fn complete_context<Ctx: WorkerCtx>(
        mut self,
        access: &mut impl DurableContext<Ctx>,
        response: Pair::Resp,
        span_finished: Option<golem_common::model::oplog::SpanFinished>,
        span_attributes: Option<golem_common::model::oplog::SpanAttributes>,
        forced_commit: bool,
    ) -> Result<Pair::Resp, WorkerExecutorError> {
        debug_assert!(self.is_live, "complete() called on a replay handle");
        // This is the call's legitimate terminal; mark it finished up front so that a failure of the
        // downstream commit / scope close (`?` below) does not drop the handle "unfinished" and trip
        // the drop policy. The host-call `End` is what makes the call durable, not these follow-ups.
        self.finished = true;
        let response = if self.persisted {
            let oplog = access.with_ctx(|ctx| ctx.state.oplog.clone());
            let (response, end) = prepare_end_entry::<Pair>(
                &oplog,
                &self.request_upload,
                self.start_idx,
                response,
                span_finished,
                span_attributes,
            )
            .await?;
            oplog.add(end).await?;
            access
                .finish_scope_with_commit(
                    self.retry.function_type().clone(),
                    self.begin_index(),
                    forced_commit,
                )
                .await?;
            response
        } else {
            response
        };
        Ok(response)
    }

    /// Accessor-window completion for non-scope-opening p3 durable calls.
    ///
    /// As with [`Self::complete`], a terminal failure carries this call's own
    /// [`DurableCallTrapContext`] via [`TerminalCallError`].
    pub async fn complete_access<T, D, Ctx>(
        mut self,
        store: &Accessor<T, D>,
        get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
        response: Pair::Resp,
    ) -> Result<Pair::Resp, TerminalCallError>
    where
        T: 'static,
        D: HasData + ?Sized,
        Ctx: WorkerCtx,
    {
        let context = self.trap_context();
        if let Err(err) = drain_dropped_call_events_access(store, get_ctx).await {
            self.abandon_for_trap();
            return Err(err);
        }
        let (response, delivery) = self
            .complete_access_impl(store, get_ctx, response)
            .await
            .map_err(|source| TerminalCallError::new(source, context))?;
        // Non-deferred boundary: no fallible guest-facing work follows the host return, so hand
        // the token to Wasmtime's terminal observer synchronously (no tear window exists between
        // the impl returning and this call) — the observer settles it when the guest actually
        // consumes (or discards) the lowered result.
        delivery
            .deliver_at_accessor_terminal(store)
            .await
            .map_err(|source| TerminalCallError::new(source, context))?;
        Ok(response)
    }

    /// Completes at the ordinary accessor boundary with a span transition on the terminal.
    pub(crate) async fn complete_access_with_span<T, D, Ctx>(
        self,
        store: &Accessor<T, D>,
        get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
        response: Pair::Resp,
        span_finished: golem_common::model::oplog::SpanFinished,
    ) -> Result<Pair::Resp, TerminalCallError>
    where
        T: 'static,
        D: HasData + ?Sized,
        Ctx: WorkerCtx,
    {
        let context = self.trap_context();
        let (response, delivery) = self
            .complete_access_deferred_with_span(store, get_ctx, response, span_finished)
            .await?;
        delivery
            .deliver_at_accessor_terminal(store)
            .await
            .map_err(|source| TerminalCallError::new(source, context))?;
        Ok(response)
    }

    /// Like [`Self::complete_access`], but for call sites whose response crosses one more
    /// fallible/cancellable boundary after the durable terminal (a second-stage channel send, a
    /// span finish before the host method returns, a wire conversion). Returns the response
    /// together with an armed [`CompletionDelivery`] token; the caller must consume the token at
    /// the real guest-facing boundary (`delivered` / `suppress` / `discarded`), and a torn future
    /// in between records the `CompletionDiscarded` marker via the token's drop.
    pub async fn complete_access_deferred<T, D, Ctx>(
        self,
        store: &Accessor<T, D>,
        get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
        response: Pair::Resp,
    ) -> Result<(Pair::Resp, CompletionDelivery), TerminalCallError>
    where
        T: 'static,
        D: HasData + ?Sized,
        Ctx: WorkerCtx,
    {
        self.complete_deferred(&mut AccessorContext::new(store, get_ctx), response)
            .await
    }

    /// Completes a call without publishing its response to the caller. Native callers prepare
    /// the delivery token and consume it at the Rust return, with no intervening await.
    pub(crate) async fn complete_deferred<Ctx: WorkerCtx>(
        self,
        access: &mut impl DurableContext<Ctx>,
        response: Pair::Resp,
    ) -> Result<(Pair::Resp, CompletionDelivery), TerminalCallError> {
        self.complete_deferred_drained(access, response, None).await
    }

    async fn complete_deferred_drained<Ctx: WorkerCtx>(
        mut self,
        access: &mut impl DurableContext<Ctx>,
        response: Pair::Resp,
        span_finished: Option<golem_common::model::oplog::SpanFinished>,
    ) -> Result<(Pair::Resp, CompletionDelivery), TerminalCallError> {
        let context = self.trap_context();
        if let Err(err) = access.drain().await {
            self.abandon_for_trap();
            return Err(err);
        }
        let (response, mut guard) = self
            .complete_terminal(
                access,
                response,
                span_finished,
                AccessResultDestination::Guest,
            )
            .await
            .map_err(|source| TerminalCallError::new(source, context))?;
        Ok((response, guard.take_completion_delivery()))
    }

    /// Prepares the End before calling `accept`. The caller must hold its local effect lock
    /// while reserving the unpublished effect and invoking `enqueue(reservation)`. Returning
    /// without invoking `enqueue` rejects the candidate. Rejection returns the
    /// unfinished call and response, so the caller can select another terminal or retry.
    /// Publication owns the reservation until the End append and DurableOnly commit succeed;
    /// it must be infallible after acceptance.
    /// `failed` must synchronously fence observers before returning the owner cleanup future.
    /// The owned coordinator resolves the terminal receipt before polling that future, so
    /// cleanup may join the receipt without depending on itself. Runtime replacement and
    /// worker retirement retain the coordinator until cleanup completes.
    /// Replay uses the ordinary deferred replay interface; this callback is live-only.
    pub async fn complete_access_deferred_with_effect<T, D, Ctx, R, A, F, E, EF>(
        self,
        store: &Accessor<T, D>,
        get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
        response: Pair::Resp,
        accept: A,
        publish: F,
        failed: E,
    ) -> Result<Result<(Pair::Resp, CompletionDelivery), (Self, Pair::Resp)>, TerminalCallError>
    where
        T: 'static,
        D: HasData + ?Sized,
        Ctx: WorkerCtx,
        R: Send + 'static,
        A: FnOnce(&Pair::Resp, Box<dyn FnOnce(R) + '_>),
        F: FnOnce(R, OplogIndex) + Send + 'static,
        E: FnOnce(WorkerExecutorError) -> EF + Send + 'static,
        EF: Future<Output = ()> + Send + 'static,
    {
        self.complete_deferred_with_effect(
            &mut AccessorContext::new(store, get_ctx),
            response,
            accept,
            publish,
            failed,
        )
        .await
    }

    /// Shared commit-before-publication protocol; see [`Self::complete_access_deferred_with_effect`].
    pub(crate) async fn complete_deferred_with_effect<Ctx, R, A, F, E, EF>(
        mut self,
        access: &mut impl DurableContext<Ctx>,
        response: Pair::Resp,
        accept: A,
        publish: F,
        failed: E,
    ) -> Result<Result<(Pair::Resp, CompletionDelivery), (Self, Pair::Resp)>, TerminalCallError>
    where
        Ctx: WorkerCtx,
        R: Send + 'static,
        A: FnOnce(&Pair::Resp, Box<dyn FnOnce(R) + '_>),
        F: FnOnce(R, OplogIndex) + Send + 'static,
        E: FnOnce(WorkerExecutorError) -> EF + Send + 'static,
        EF: Future<Output = ()> + Send + 'static,
    {
        let context = self.trap_context();
        if let Err(err) = access.drain().await {
            self.abandon_for_trap();
            return Err(err);
        }
        self.ensure_accessor_terminal_supported("complete_access_deferred_with_effect")
            .map_err(|err| TerminalCallError::new(err, context))?;
        if !self.is_live || !self.persisted {
            return Err(TerminalCallError::new(
                WorkerExecutorError::runtime(
                    "local effect completion requires a persisted live call",
                ),
                context,
            ));
        }
        let (oplog, recorder, worker) = access.with_ctx(|ctx| {
            (
                ctx.state.oplog.clone(),
                ctx.state.completion_marker_recorder(),
                ctx.public_state.worker(),
            )
        });
        let prepared = prepare_end_entry::<Pair>(
            &oplog,
            &self.request_upload,
            self.start_idx,
            response,
            None,
            None,
        )
        .await;
        let (response, end) = match prepared {
            Ok(prepared) => prepared,
            Err(err) => {
                self.abandon_for_trap();
                return Err(TerminalCallError::new(err, context));
            }
        };
        let Some(recording) = worker.runtime_append_tasks().register() else {
            self.abandon_for_trap();
            return Err(TerminalCallError::new(
                WorkerExecutorError::Interrupted {
                    kind: InterruptKind::Restart,
                },
                context,
            ));
        };
        let tasks = worker.tasks.clone();
        let coordinator = prepare_access_terminal_effect(
            &tasks,
            recording,
            async move {
                worker
                    .commit_oplog_and_update_state(CommitLevel::DurableOnly)
                    .await?;
                Ok(())
            },
            publish,
            failed,
        );
        let (handoff, terminal) = match coordinator {
            Ok(coordinator) => coordinator,
            Err(err) => {
                self.abandon_for_trap();
                return Err(TerminalCallError::new(err, context));
            }
        };
        let mut guard = AccessTerminalGuard::<P>::new(
            self.dropped_call_snapshot(self.live_call_permit.clone()),
            self.drop_sink.clone(),
            self.cleanup_sink.clone(),
            self.teardown_probe(),
        );
        let accepted = accept_access_terminal_effect(&response, &oplog, end, accept);
        let (reservation, append) = match accepted {
            Some(accepted) => accepted,
            None => {
                guard.disarm();
                return Ok(Err((self, response)));
            }
        };
        // Admission preceded acceptance; the coordinator cannot be cancelled by retirement.
        self.finished = true;
        assert!(handoff.send((reservation, append)).is_ok());
        guard.cleanup_after_terminal(
            terminal,
            Some(CompletionMarkerRecord {
                start_idx: self.start_idx,
                recorder,
            }),
        );
        if let Err(err) = guard.wait_terminal().await {
            guard.disarm();
            return Err(TerminalCallError::new(err, context));
        }
        self.finish_terminal_scope(access, &mut guard)
            .await
            .map_err(|err| TerminalCallError::new(err, context))?;
        Ok(Ok((response, guard.take_completion_delivery())))
    }

    pub async fn complete_access_deferred_with_span<T, D, Ctx>(
        mut self,
        store: &Accessor<T, D>,
        get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
        response: Pair::Resp,
        span_finished: golem_common::model::oplog::SpanFinished,
    ) -> Result<(Pair::Resp, CompletionDelivery), TerminalCallError>
    where
        T: 'static,
        D: HasData + ?Sized,
        Ctx: WorkerCtx,
    {
        let context = self.trap_context();
        if let Err(err) = drain_dropped_call_events_access(store, get_ctx).await {
            self.abandon_for_trap();
            return Err(err);
        }
        self.complete_access_impl_with_span(store, get_ctx, response, Some(span_finished))
            .await
            .map_err(|source| TerminalCallError::new(source, context))
    }

    async fn complete_access_impl<T, D, Ctx>(
        self,
        store: &Accessor<T, D>,
        get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
        response: Pair::Resp,
    ) -> Result<(Pair::Resp, CompletionDelivery), WorkerExecutorError>
    where
        T: 'static,
        D: HasData + ?Sized,
        Ctx: WorkerCtx,
    {
        self.complete_access_impl_with_span(store, get_ctx, response, None)
            .await
    }

    async fn complete_access_impl_with_span<T, D, Ctx>(
        self,
        store: &Accessor<T, D>,
        get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
        response: Pair::Resp,
        span_finished: Option<golem_common::model::oplog::SpanFinished>,
    ) -> Result<(Pair::Resp, CompletionDelivery), WorkerExecutorError>
    where
        T: 'static,
        D: HasData + ?Sized,
        Ctx: WorkerCtx,
    {
        let (response, mut guard) = self
            .complete_access_terminal(
                store,
                get_ctx,
                response,
                span_finished,
                AccessResultDestination::Guest,
            )
            .await?;
        Ok((response, guard.take_completion_delivery()))
    }

    async fn complete_access_terminal<T, D, Ctx>(
        self,
        store: &Accessor<T, D>,
        get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
        response: Pair::Resp,
        span_finished: Option<golem_common::model::oplog::SpanFinished>,
        destination: AccessResultDestination,
    ) -> Result<(Pair::Resp, AccessTerminalGuard<P>), WorkerExecutorError>
    where
        T: 'static,
        D: HasData + ?Sized,
        Ctx: WorkerCtx,
    {
        self.complete_terminal(
            &mut AccessorContext::new(store, get_ctx),
            response,
            span_finished,
            destination,
        )
        .await
    }

    async fn complete_terminal<Ctx: WorkerCtx>(
        mut self,
        access: &mut impl DurableContext<Ctx>,
        response: Pair::Resp,
        span_finished: Option<golem_common::model::oplog::SpanFinished>,
        destination: AccessResultDestination,
    ) -> Result<(Pair::Resp, AccessTerminalGuard<P>), WorkerExecutorError> {
        self.ensure_accessor_terminal_supported("complete_access")?;
        if !self.is_live {
            return Err(WorkerExecutorError::runtime(
                "complete_access called on a replay handle; use replay_access first",
            ));
        }
        {
            let (oplog, completion_marker_recorder) = access.with_ctx(|ctx| {
                (
                    ctx.state.oplog.clone(),
                    (destination == AccessResultDestination::Guest)
                        .then(|| ctx.state.completion_marker_recorder()),
                )
            });
            let mut guard = AccessTerminalGuard::<P>::new(
                self.dropped_call_snapshot(self.live_call_permit.clone()),
                self.drop_sink.clone(),
                self.cleanup_sink.clone(),
                self.teardown_probe(),
            );
            self.finished = true;
            let persist_result: Result<Pair::Resp, WorkerExecutorError> = if self.persisted {
                Self::persist_access_terminal(
                    oplog,
                    completion_marker_recorder,
                    &mut guard,
                    self.start_idx,
                    response,
                    span_finished,
                )
                .await
            } else {
                Ok(response)
            };

            let response = match persist_result {
                Ok(response) => response,
                Err(err) => {
                    guard.disarm();
                    return Err(err);
                }
            };
            self.finish_terminal_scope(access, &mut guard).await?;
            Ok((response, guard))
        }
    }

    async fn finish_terminal_scope<Ctx: WorkerCtx>(
        &mut self,
        access: &mut impl DurableContext<Ctx>,
        guard: &mut AccessTerminalGuard<P>,
    ) -> Result<(), WorkerExecutorError> {
        let function_type = self.retry.function_type().clone();
        let begin_index = self.begin_index();
        let result = access.finish_scope(function_type, begin_index).await;
        if result.is_err() {
            // The caller observes this failure; it is not a guest-discarded completion.
            guard.suppress_discard_marker();
        }
        result
    }

    /// Persistence stage of [`Self::complete_access_impl`]: wait for the (possibly deferred)
    /// request upload, upload the response payload, hand the terminal `End` append to an owned
    /// task via [`AccessTerminalGuard::cleanup_after_terminal`], and join it before returning.
    ///
    /// Deliberately store-free — it sees only the oplog and the armed guard, never the `Accessor`
    /// — so nothing on this path can release the live-call permit or emit the guard's cleanup
    /// event before the terminal entry is appended: the guard still owns both when this returns,
    /// and they are only released downstream (`disarm` after `end_durable_function_access`), or
    /// handed to the drain queue if the completion future is torn mid-await. The focused ordering
    /// test `access_terminal_end_is_appended_before_cleanup_and_permit_release` drives this exact
    /// function against a gated oplog to keep that invariant observable.
    pub(super) async fn persist_access_terminal(
        oplog: Arc<dyn Oplog>,
        completion_marker_recorder: Option<CompletionMarkerRecorder>,
        guard: &mut AccessTerminalGuard<P>,
        start_idx: OplogIndex,
        response: Pair::Resp,
        span_finished: Option<golem_common::model::oplog::SpanFinished>,
    ) -> Result<Pair::Resp, WorkerExecutorError> {
        let (response, end) = prepare_end_entry::<Pair>(
            &oplog,
            &guard
                .call()
                .expect("terminal guard is armed")
                .request_upload,
            start_idx,
            response,
            span_finished,
            None,
        )
        .await?;
        // Reserve the terminal synchronously in oplog order before arming the terminal guard. A
        // tear may queue a completion marker immediately, and that marker must never overtake an
        // unpolled spawned append task.
        if let OplogEntry::End {
            span_finished: Some(finished),
            ..
        } = &end
            && let Some(observer) = &guard.call().expect("terminal guard is armed").span_observer
        {
            observer.closed(finished);
        }
        let end_append = oplog.enqueue_add(end);
        // Arm the discard marker together with the owned `End` append: from this point a torn
        // completion future means the guest discarded a persisted successful completion (a tear
        // *during* `wait_terminal` still counts — the owned task appends the `End` regardless).
        guard.cleanup_after_terminal(
            tokio::spawn(async move {
                end_append
                    .await
                    .map(|_| ())
                    .map_err(WorkerExecutorError::from)
            }),
            completion_marker_recorder.map(|recorder| CompletionMarkerRecord {
                start_idx,
                recorder,
            }),
        );
        guard.wait_terminal().await?;
        Ok(response)
    }

    /// Replays a call: drive the cursor until the call resolves, decode its response, then close the
    /// durable scope / commit via `end_durable_function`.
    ///
    /// If replay reaches the end of the oplog without ever seeing the matching `End`/`Cancelled` —
    /// a lone committed host-call `Start`, now possible for any write because `Start` is eager —
    /// the call is returned as [`CallReplayOutcome::Incomplete`] (for function types that are safe
    /// to re-execute) so the caller can re-run the side effect and `complete` the existing `Start`.
    /// A child call requires explicit repair authorization from its protocol owner.
    pub async fn replay<Ctx: WorkerCtx>(
        self,
        ctx: &mut DurableWorkerCtx<Ctx>,
    ) -> Result<CallReplayOutcome<Pair, P>, WorkerExecutorError> {
        self.replay_context(ctx).await
    }

    async fn reproduce_refusal_for_claimed_start<Ctx: WorkerCtx>(
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

    /// Replays a direct host-return call. Markerless Ends return immediately; delivery and
    /// discard markers remain divergence errors, not deferred guest completion boundaries.
    pub(crate) async fn replay_context<Ctx: WorkerCtx>(
        mut self,
        access: &mut impl DurableContext<Ctx>,
    ) -> Result<CallReplayOutcome<Pair, P>, WorkerExecutorError> {
        self.reproduce_refusal_for_claimed_start(access).await?;
        let replay = self
            .replay
            .take()
            .expect("replay() called on a live handle");
        let (replay_state, oplog) = access.with_ctx(|ctx| {
            if let Some(hook) = ctx.replay_admission_hook() {
                hook.before_direct_replay_wait(Pair::FQFN, self.start_idx);
            }
            (ctx.state.replay_state.clone(), ctx.state.oplog.clone())
        });
        let outcome = replay_state.await_resolution_outcome(replay).await?;
        match classify_replay_resolution(outcome) {
            ReplayedResolution::Delivered(payload, disposition) => {
                if let ReplayDeliveryDisposition::AtMarker(marker_idx) = disposition {
                    self.finished = true;
                    return Err(WorkerExecutorError::unexpected_oplog_entry(
                        "a completion-delivery marker for an accessor durable call",
                        format!(
                            "CompletionDelivered at {marker_idx} references non-accessor durable call Start at {}",
                            self.start_idx
                        ),
                    ));
                }
                // `Immediate` and `AtReplayTail` both deliver at the host return: the direct
                // path's guest task is synchronously blocked inside the host call, so delivery
                // coincides with the host return and has no pre-delivery divergence window.
                //
                // Terminal: mark finished up front so a decode / scope-close failure below does not
                // drop the (replay) handle as "unfinished".
                self.finished = true;
                let response = decode_replayed_payload::<Pair>(
                    &oplog,
                    payload,
                    self.execution_scope.trap_retry_point(),
                )
                .await?;
                access
                    .finish_scope(self.retry.function_type().clone(), self.begin_index())
                    .await?;
                Ok(CallReplayOutcome::Replayed(response))
            }
            ReplayedResolution::Undelivered(terminal) => {
                // Replay divergence: undelivered terminals cannot legally occur on the direct
                // path (see [`UndeliveredTerminal::direct_divergence_error`]).
                self.finished = true;
                Err(terminal.direct_divergence_error())
            }
            ReplayedResolution::Incomplete => {
                self.prepare_incomplete_live_repair_at(access).await?;
                if let Err(error) = access.publish_direct_replay_tail(self.start_idx).await {
                    self.finished = true;
                    return Err(error);
                }
                if let Some((response, span_finished)) =
                    self.recorded_denied_response(access).await?
                {
                    let response = self
                        .complete_drained(access, response, span_finished)
                        .await
                        .map_err(|error| error.source)?;
                    return Ok(CallReplayOutcome::Replayed(response));
                }
                if self.requires_agent_authority {
                    match access.agent_auth_ctx().await {
                        Ok(auth_ctx) => self.agent_auth_ctx = Some(auth_ctx),
                        Err(error) => {
                            self.abandon_for_trap();
                            return Err(error);
                        }
                    }
                }
                Ok(CallReplayOutcome::Incomplete(self))
            }
        }
    }

    /// Accessor-window replay for non-scope-opening p3 durable calls.
    pub async fn replay_access<T, D, Ctx>(
        self,
        store: &Accessor<T, D>,
        get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
    ) -> Result<CallReplayOutcome<Pair, P>, WorkerExecutorError>
    where
        T: 'static,
        D: HasData + ?Sized,
        Ctx: WorkerCtx,
    {
        self.replay_access_to(store, get_ctx, AccessResultDestination::Guest, false)
            .await
    }

    async fn replay_access_to<T, D, Ctx>(
        mut self,
        store: &Accessor<T, D>,
        get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
        destination: AccessResultDestination,
        require_completed: bool,
    ) -> Result<CallReplayOutcome<Pair, P>, WorkerExecutorError>
    where
        T: 'static,
        D: HasData + ?Sized,
        Ctx: WorkerCtx,
    {
        self.ensure_accessor_terminal_supported("replay_access")?;
        self.reproduce_refusal_for_claimed_start(&mut AccessorContext::new(store, get_ctx))
            .await?;
        let function_type = self.retry.function_type().clone();
        let begin_index = self.begin_index();
        let (replay_state, oplog, completion_marker_recorder) = store.with(|mut access| {
            let ctx = get_ctx(access.data_mut());
            (
                ctx.state.replay_state.clone(),
                ctx.state.oplog.clone(),
                ctx.state.completion_marker_recorder(),
            )
        });
        let replay = self
            .replay
            .take()
            .expect("replay_access() called on a live handle");
        let outcome = replay_state.await_resolution_outcome(replay).await?;
        if require_completed
            && !matches!(
                outcome,
                ResolutionOutcome::Resolved(Resolution::Completed { .. })
            )
        {
            self.finished = true;
            return Err(WorkerExecutorError::unexpected_oplog_entry(
                "completed host-internal child",
                "completed parent has an unfinished host-internal child",
            ));
        }
        if destination == AccessResultDestination::Host
            && matches!(
                outcome,
                ResolutionOutcome::Resolved(
                    Resolution::Completed {
                        delivery_marker: Some(_),
                        ..
                    } | Resolution::CompletedButDiscarded { .. }
                )
            )
        {
            self.finished = true;
            return Err(WorkerExecutorError::unexpected_oplog_entry(
                "host-internal terminal without a guest delivery marker",
                format!("delivery marker for host-internal Start {}", self.start_idx),
            ));
        }
        match classify_replay_resolution(outcome) {
            ReplayedResolution::Delivered(payload, disposition) => {
                self.finished = true;
                let response = decode_replayed_payload::<Pair>(
                    &oplog,
                    payload,
                    self.execution_scope.trap_retry_point(),
                )
                .await?;
                end_durable_function_access(store, get_ctx, function_type, begin_index, false)
                    .await?;
                if destination == AccessResultDestination::Guest {
                    CompletionDelivery::replay_delivered(
                        disposition,
                        self.start_idx,
                        completion_marker_recorder,
                        self.trap_context(),
                        self.cleanup_sink.clone(),
                        self.teardown_probe(),
                    )
                    .deliver_at_accessor_terminal(store)
                    .await?;
                }
                Ok(CallReplayOutcome::Replayed(response))
            }
            ReplayedResolution::Undelivered(terminal) => {
                // The recorded run never delivered this terminal to the guest (either the call
                // was cancelled mid-flight without a partial result, or its completion was
                // discarded before delivery). Mirror that exactly: never complete, so the
                // deterministic guest drops this future at the same point it did live. Resolving
                // to an error here instead would race the guest's own drop — the resolver
                // delivers the recorded terminal as soon as the cursor crosses it, which may be
                // before the winning branch has been polled and had a chance to drop the loser.
                //
                // Durable cleanup (closing the durable scope for scope-opening calls) is
                // deferred to this handle's `Drop`, which enqueues the idempotent
                // `CloseDurableScope` event. Awaiting `end_durable_function_access` *here*,
                // before the park, would open a cancellation window: for scope-opening calls
                // it takes the scope's replay handle before its first await, so the guest
                // dropping this future mid-close would strand the open scope (with `finished`
                // already set, `Drop` would not clean it up either).
                terminal.log_parking();
                self.parked_undelivered_replay = true;
                // Release the terminal before suspending: a discarded completion carries the
                // persisted response payload, which must not be kept alive for as long as the
                // guest keeps this parked future around.
                drop(terminal);
                park_undelivered_forever().await
            }
            ReplayedResolution::Incomplete => {
                self.prepare_incomplete_live_repair_at(&mut AccessorContext::new(store, get_ctx))
                    .await?;
                let transition = publish_incomplete_replay_tail_at(
                    &mut AccessorContext::new(store, get_ctx),
                    Some(SerializableToolAdmissionRefusal::AtRecordedOplogEntry {
                        index: self.start_idx.into(),
                    }),
                )
                .await;
                if let Err(error) = transition.and_then(FinishReplayToLive::require_live) {
                    self.finished = true;
                    return Err(error);
                }
                let synchronization = if self.requires_agent_authority {
                    match crate::durable_host::call_coordinator::capture_live_agent_authority_access(store, get_ctx, |ctx| DurableWorkerCtx::agent_auth_ctx(ctx)).await {
                        Ok(Some(auth_ctx)) => {
                            self.agent_auth_ctx = Some(auth_ctx);
                            Ok(())
                        }
                        Ok(None) => Err(WorkerExecutorError::runtime("incomplete durable call did not switch to live execution")),
                        Err(error) => Err(error),
                    }
                } else {
                    Ok(())
                };
                if let Err(error) = synchronization {
                    self.abandon_for_trap();
                    return Err(error);
                }
                let denied = self
                    .recorded_denied_response(&mut AccessorContext::new(store, get_ctx))
                    .await?;
                if let Some((response, span_finished)) = denied {
                    if destination != AccessResultDestination::Guest {
                        self.abandon_for_trap();
                        return Err(WorkerExecutorError::unexpected_oplog_entry(
                            "host-internal Start without a denial",
                            format!("denied host-internal Start {}", self.start_idx),
                        ));
                    }
                    let response = match span_finished {
                        Some(span_finished) => {
                            self.complete_access_with_span(store, get_ctx, response, span_finished)
                                .await
                        }
                        None => self.complete_access(store, get_ctx, response).await,
                    }
                    .map_err(|error| error.source)?;
                    return Ok(CallReplayOutcome::Replayed(response));
                }
                Ok(CallReplayOutcome::Incomplete(self))
            }
        }
    }

    /// Replay terminal handling for entity-body reconstruction. Unlike the guest-future accessor
    /// path, a recorded cancellation is returned to the reconstruction coordinator instead of
    /// parking forever: the coordinator owns the transient entity task and aborts it at that
    /// durable boundary. Completed responses are decoded normally and incomplete Starts become
    /// live-repair handles under the original Start index.
    pub async fn replay_reconstruction_access<T, D, Ctx>(
        mut self,
        store: &Accessor<T, D>,
        get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
    ) -> Result<ReconstructionReplayOutcome<Pair, P>, WorkerExecutorError>
    where
        T: 'static,
        D: HasData + ?Sized,
        Ctx: WorkerCtx,
    {
        self.ensure_accessor_terminal_supported("replay_reconstruction_access")?;
        let function_type = self.retry.function_type().clone();
        let begin_index = self.begin_index();
        let (replay_state, oplog, completion_marker_recorder) = store.with(|mut access| {
            let ctx = get_ctx(access.data_mut());
            (
                ctx.state.replay_state.clone(),
                ctx.state.oplog.clone(),
                ctx.state.completion_marker_recorder(),
            )
        });
        let replay = self
            .replay
            .take()
            .expect("replay_reconstruction_access() called on a live handle");
        let outcome = replay_state.await_resolution_outcome(replay).await?;
        match classify_replay_resolution(outcome) {
            ReplayedResolution::Delivered(payload, disposition) => {
                self.finished = true;
                let cancelled = matches!(payload, ReplayedPayload::CancelledPartial(_));
                let response = decode_replayed_payload::<Pair>(
                    &oplog,
                    payload,
                    self.execution_scope.trap_retry_point(),
                )
                .await?;
                end_durable_function_access(store, get_ctx, function_type, begin_index, false)
                    .await?;
                CompletionDelivery::replay_delivered(
                    disposition,
                    self.start_idx,
                    completion_marker_recorder,
                    self.trap_context(),
                    self.cleanup_sink.clone(),
                    self.teardown_probe(),
                )
                .deliver_at_accessor_terminal(store)
                .await?;
                if cancelled {
                    Ok(ReconstructionReplayOutcome::Cancelled(response))
                } else {
                    Ok(ReconstructionReplayOutcome::Replayed(response))
                }
            }
            ReplayedResolution::Undelivered(UndeliveredTerminal::CancelledWithoutPartial {
                cancelled_idx,
            }) => {
                self.finished = true;
                end_durable_function_access(store, get_ctx, function_type, begin_index, false)
                    .await?;
                Err(WorkerExecutorError::unexpected_oplog_entry(
                    "entity cancellation with partial terminal provenance",
                    format!("Cancelled without partial at {cancelled_idx}"),
                ))
            }
            ReplayedResolution::Undelivered(terminal) => {
                self.finished = true;
                Err(terminal.direct_divergence_error())
            }
            ReplayedResolution::Incomplete => {
                self.prepare_incomplete_live_repair_at(&mut AccessorContext::new(store, get_ctx))
                    .await?;
                let transition = match publish_incomplete_replay_tail_access(
                    store,
                    get_ctx,
                    SerializableToolAdmissionRefusal::AtRecordedOplogEntry {
                        index: self.start_idx.into(),
                    },
                )
                .await
                {
                    Ok(transition) => transition,
                    Err(error) => {
                        self.finished = true;
                        return Err(error);
                    }
                };
                match transition {
                    FinishReplayToLive::Cancelled => {
                        Ok(ReconstructionReplayOutcome::LiveAdmissionCancelled(self))
                    }
                    FinishReplayToLive::Live | FinishReplayToLive::AttachmentAdmissionRejected => {
                        Ok(ReconstructionReplayOutcome::Incomplete(self))
                    }
                }
            }
        }
    }

    pub(in crate::durable_host) async fn resolve_completed_reconstruction(
        mut self,
        replay_state: ReplayState,
        oplog: Arc<dyn Oplog>,
        completion_marker_recorder: CompletionMarkerRecorder,
    ) -> Result<(Pair::Resp, ResolvedReconstructionTerminal), WorkerExecutorError> {
        self.ensure_accessor_terminal_supported("resolve_completed_reconstruction")?;
        let function_type = self.retry.function_type().clone();
        let begin_index = self.begin_index();
        let replay = self
            .replay
            .take()
            .expect("resolve_completed_reconstruction() called on a live handle");
        let outcome = replay_state.await_resolution_outcome(replay).await?;
        match classify_replay_resolution(outcome) {
            ReplayedResolution::Delivered(payload, disposition) => {
                self.finished = true;
                let cancelled = matches!(payload, ReplayedPayload::CancelledPartial(_));
                let response = decode_replayed_payload::<Pair>(
                    &oplog,
                    payload,
                    self.execution_scope.trap_retry_point(),
                )
                .await?;
                let delivery = CompletionDelivery::replay_delivered(
                    disposition,
                    self.start_idx,
                    completion_marker_recorder,
                    self.trap_context(),
                    self.cleanup_sink.clone(),
                    self.teardown_probe(),
                );
                Ok((
                    response,
                    ResolvedReconstructionTerminal {
                        function_type,
                        begin_index,
                        delivery,
                        cancelled,
                    },
                ))
            }
            ReplayedResolution::Undelivered(terminal) => {
                self.finished = true;
                Err(terminal.direct_divergence_error())
            }
            ReplayedResolution::Incomplete => {
                self.finished = true;
                Err(WorkerExecutorError::unexpected_oplog_entry(
                    "completed entity invocation terminal",
                    format!(
                        "end of replay after entity Start at {}; the completed reconstruction was unresolved",
                        self.start_idx
                    ),
                ))
            }
        }
    }

    /// Like [`Self::replay_access`], but for deferred-delivery call sites (see
    /// [`Self::complete_access_deferred`]): a replayed terminal is returned together with a
    /// [`CompletionDelivery`] token instead of parking here when the recorded run discarded the
    /// completion. For a recorded `CompletionDiscarded` marker this decodes the persisted
    /// response and closes the durable scope — mirroring exactly what live did before its token
    /// was armed — and the *caller* parks at its own delivery boundary after performing its
    /// deterministic post-`End` continuation (e.g. restoring resource state).
    pub async fn replay_access_deferred<T, D, Ctx>(
        self,
        store: &Accessor<T, D>,
        get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
    ) -> Result<DeferredCallReplayOutcome<Pair, P>, WorkerExecutorError>
    where
        T: 'static,
        D: HasData + ?Sized,
        Ctx: WorkerCtx,
    {
        self.replay_deferred(&mut AccessorContext::new(store, get_ctx))
            .await
    }

    /// Resolves the recorded terminal while retaining the actual caller-delivery boundary.
    /// Callers map errors with the session's trap context, as on the accessor path.
    pub(crate) async fn replay_deferred<Ctx: WorkerCtx>(
        mut self,
        access: &mut impl DurableContext<Ctx>,
    ) -> Result<DeferredCallReplayOutcome<Pair, P>, WorkerExecutorError> {
        self.ensure_accessor_terminal_supported("replay_access_deferred")?;
        self.reproduce_refusal_for_claimed_start(access).await?;
        let function_type = self.retry.function_type().clone();
        let begin_index = self.begin_index();
        let (replay_state, oplog, completion_marker_recorder) = access.with_ctx(|ctx| {
            (
                ctx.state.replay_state.clone(),
                ctx.state.oplog.clone(),
                ctx.state.completion_marker_recorder(),
            )
        });
        let replay = self
            .replay
            .take()
            .expect("replay_access_deferred() called on a live handle");
        let outcome = replay_state.await_resolution_outcome(replay).await?;
        match classify_replay_resolution(outcome) {
            ReplayedResolution::Delivered(payload, disposition) => {
                self.finished = true;
                let response = decode_replayed_payload::<Pair>(
                    &oplog,
                    payload,
                    self.execution_scope.trap_retry_point(),
                )
                .await?;
                access.finish_scope(function_type, begin_index).await?;
                // The delivery token is constructed only after the fallible decode / scope close
                // succeeded: a token dropped on the error path would log a spurious
                // unconsumed-token warning.
                Ok(DeferredCallReplayOutcome::Replayed(
                    response,
                    CompletionDelivery::replay_delivered(
                        disposition,
                        self.start_idx,
                        completion_marker_recorder,
                        self.trap_context(),
                        self.cleanup_sink.clone(),
                        self.teardown_probe(),
                    ),
                ))
            }
            ReplayedResolution::Undelivered(UndeliveredTerminal::CompletionDiscarded {
                end_idx,
                marker_idx,
                response,
            }) => {
                // The recorded run persisted a successful `End` but the guest discarded the
                // completion before final delivery (marker at `marker_idx`). Decode the
                // persisted response and close the durable scope — the exact state live was in
                // when its delivery token was armed — and let the caller run its deterministic
                // post-`End` continuation before parking at the delivery boundary.
                tracing::debug!(
                    "durable call completed at {end_idx} but its completion was discarded \
                     by the guest (marker at {marker_idx}); replaying its post-End \
                     continuation before parking at the delivery boundary"
                );
                self.finished = true;
                let response = decode_replayed_payload::<Pair>(
                    &oplog,
                    ReplayedPayload::Completed(response),
                    self.execution_scope.trap_retry_point(),
                )
                .await?;
                access.finish_scope(function_type, begin_index).await?;
                // As above, the token is constructed only after the fallible operations.
                Ok(DeferredCallReplayOutcome::Replayed(
                    response,
                    CompletionDelivery::replay_discarded(),
                ))
            }
            ReplayedResolution::Undelivered(
                terminal @ UndeliveredTerminal::CancelledWithoutPartial { .. },
            ) => {
                // Cancelled with no partial: in the recorded run this call never returned a
                // value to the guest. Mirror exactly — park here so the deterministic guest
                // drops this future at the same point it did live; durable cleanup is
                // deferred to this handle's `Drop`. See [`Self::replay_access`] for why the
                // scope close must not happen before the park.
                terminal.log_parking();
                self.parked_undelivered_replay = true;
                park_undelivered_forever().await
            }
            ReplayedResolution::Incomplete => {
                self.prepare_incomplete_live_repair_at(access).await?;
                let transition = publish_incomplete_replay_tail_at(
                    access,
                    Some(SerializableToolAdmissionRefusal::AtRecordedOplogEntry {
                        index: self.start_idx.into(),
                    }),
                )
                .await;
                if let Err(error) = transition.and_then(FinishReplayToLive::require_live) {
                    self.finished = true;
                    return Err(error);
                }
                let synchronization = if self.requires_agent_authority {
                    match access.agent_auth_ctx().await {
                        Ok(auth_ctx) => {
                            self.agent_auth_ctx = Some(auth_ctx);
                            Ok(())
                        }
                        Err(error) => Err(error),
                    }
                } else {
                    Ok(())
                };
                if let Err(error) = synchronization {
                    self.abandon_for_trap();
                    return Err(error);
                }
                if let Some((response, span_finished)) =
                    self.recorded_denied_response(access).await?
                {
                    let (response, delivery) = self
                        .complete_deferred_drained(access, response, span_finished)
                        .await
                        .map_err(|error| error.source)?;
                    return Ok(DeferredCallReplayOutcome::Replayed(response, delivery));
                }
                Ok(DeferredCallReplayOutcome::Incomplete(self))
            }
        }
    }

    /// Promotes an incomplete parent after its protocol-authorized child reached live execution.
    /// Publishing the child's replay tail does not change the parent's session-local state.
    pub(super) async fn resume_incomplete_scope_access<T, D, Ctx>(
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

    /// Finalizes a response that has already been produced by adapter-specific logic, converging
    /// live completion, completed replay, and incomplete-replay repair in one transition.
    pub(crate) async fn finish_access<T, D, Ctx>(
        self,
        store: &Accessor<T, D>,
        get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
        response: Pair::Resp,
    ) -> Result<Pair::Resp, TerminalCallError>
    where
        T: 'static,
        D: HasData + ?Sized,
        Ctx: WorkerCtx,
    {
        let context = self.trap_context();
        if self.is_live() {
            self.complete_access(store, get_ctx, response).await
        } else {
            match self
                .replay_access(store, get_ctx)
                .await
                .map_err(|source| TerminalCallError::new(source, context))?
            {
                CallReplayOutcome::Replayed(recorded) => Ok(recorded),
                CallReplayOutcome::Incomplete(call) => {
                    call.complete_access(store, get_ctx, response).await
                }
            }
        }
    }

    /// Deferred-delivery form of [`Self::finish_access`]. The returned token always describes the
    /// recorded delivery state, regardless of whether this invocation completed the item live,
    /// repaired an incomplete `Start`, or replayed an existing terminal. The fallback response
    /// is built only after incomplete-call readiness has selected the owner's recovery outcome.
    pub(crate) async fn finish_access_deferred<T, D, Ctx>(
        self,
        store: &Accessor<T, D>,
        get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
        response: impl FnOnce() -> Pair::Resp,
    ) -> Result<(Pair::Resp, CompletionDelivery), TerminalCallError>
    where
        T: 'static,
        D: HasData + ?Sized,
        Ctx: WorkerCtx,
    {
        let context = self.trap_context();
        if self.is_live() {
            self.complete_access_deferred(store, get_ctx, response())
                .await
        } else {
            match self
                .replay_access_deferred(store, get_ctx)
                .await
                .map_err(|source| TerminalCallError::new(source, context))?
            {
                DeferredCallReplayOutcome::Replayed(recorded, delivery) => Ok((recorded, delivery)),
                DeferredCallReplayOutcome::Incomplete(call) => {
                    call.complete_access_deferred(store, get_ctx, response())
                        .await
                }
            }
        }
    }

    pub(crate) async fn finish_access_deferred_with_span<T, D, Ctx>(
        self,
        store: &Accessor<T, D>,
        get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
        response: impl FnOnce() -> (Pair::Resp, golem_common::model::oplog::SpanFinished),
    ) -> Result<(Pair::Resp, CompletionDelivery), TerminalCallError>
    where
        T: 'static,
        D: HasData + ?Sized,
        Ctx: WorkerCtx,
    {
        let context = self.trap_context();
        if self.is_live() {
            let (response, span_finished) = response();
            self.complete_access_deferred_with_span(store, get_ctx, response, span_finished)
                .await
        } else {
            match self
                .replay_access_deferred(store, get_ctx)
                .await
                .map_err(|source| TerminalCallError::new(source, context))?
            {
                DeferredCallReplayOutcome::Replayed(recorded, delivery) => Ok((recorded, delivery)),
                DeferredCallReplayOutcome::Incomplete(call) => {
                    let (response, span_finished) = response();
                    call.complete_access_deferred_with_span(store, get_ctx, response, span_finished)
                        .await
                }
            }
        }
    }

    /// Replays a call whose protocol requires a recorded terminal rather than live repair.
    pub async fn replay_expecting_completion<Ctx: WorkerCtx>(
        self,
        ctx: &mut DurableWorkerCtx<Ctx>,
    ) -> Result<Pair::Resp, WorkerExecutorError> {
        match self.replay(ctx).await? {
            CallReplayOutcome::Replayed(response) => Ok(response),
            CallReplayOutcome::Incomplete(mut handle) => {
                handle.abandon_for_trap();
                Err(WorkerExecutorError::unexpected_oplog_entry(
                    "End or Cancelled",
                    "an incomplete non-re-executable durable call".to_string(),
                ))
            }
        }
    }

    /// Replays a recorded response or completes the original incomplete call with a typed
    /// failure. This grants no permission to repeat the external effect.
    pub async fn replay_or_persist_failure<Ctx: WorkerCtx>(
        mut self,
        ctx: &mut DurableWorkerCtx<Ctx>,
        failure: Pair::Resp,
    ) -> Result<Pair::Resp, WorkerExecutorError> {
        self.reproduce_refusal_for_claimed_start(ctx).await?;
        let replay = self
            .replay
            .take()
            .expect("response-only repair requires replay");
        let outcome = ctx
            .state
            .replay_state
            .await_resolution_outcome(replay)
            .await?;
        match classify_replay_resolution(outcome) {
            ReplayedResolution::Delivered(payload, disposition) => {
                self.finished = true;
                if matches!(disposition, ReplayDeliveryDisposition::AtMarker(_)) {
                    return Err(WorkerExecutorError::unexpected_oplog_entry(
                        "a direct host-return terminal",
                        "a completion-delivery marker".to_string(),
                    ));
                }
                let response = decode_replayed_payload::<Pair>(
                    &ctx.state.oplog,
                    payload,
                    self.execution_scope.trap_retry_point(),
                )
                .await?;
                ctx.finish_scope(self.retry.function_type().clone(), self.begin_index())
                    .await?;
                Ok(response)
            }
            ReplayedResolution::Undelivered(terminal) => {
                self.finished = true;
                Err(terminal.direct_divergence_error())
            }
            ReplayedResolution::Incomplete => {
                self.prepare_incomplete_live_repair_at(ctx).await?;
                if let Err(error) = ctx.publish_direct_replay_tail(self.start_idx).await {
                    self.finished = true;
                    return Err(error);
                }
                if let Some((response, span_finished)) = self.recorded_denied_response(ctx).await? {
                    return self
                        .complete_context(ctx, response, span_finished, None, true)
                        .await;
                }
                self.complete_context(ctx, failure, None, None, true).await
            }
        }
    }

    /// Cancels a call.
    ///
    /// Live: append a `Cancelled` entry. Replay: expect the call to resolve as `Cancelled`. This
    /// call's own retry grouping stays with its call-owned `execution_scope` (and the semantic-trap
    /// error marker); the cancel path no longer touches the ambient/global retry-point fallback.
    pub async fn cancel<Ctx: WorkerCtx>(
        mut self,
        ctx: &mut DurableWorkerCtx<Ctx>,
        partial: Option<Pair::Resp>,
    ) -> Result<(), TerminalCallError> {
        let context = self.trap_context();
        if let Err(err) = drain_queued_dropped_call_events(ctx).await {
            self.abandon_for_trap();
            return Err(err);
        }
        self.cancel_impl(ctx, partial)
            .await
            .map_err(|source| TerminalCallError::new(source, context))
    }

    async fn cancel_impl<Ctx: WorkerCtx>(
        mut self,
        ctx: &mut DurableWorkerCtx<Ctx>,
        partial: Option<Pair::Resp>,
    ) -> Result<(), WorkerExecutorError> {
        // Terminal: mark finished up front so a fallible step below does not drop the handle as
        // "unfinished".
        self.finished = true;
        if self.is_live {
            if self.persisted {
                let oplog = ctx.state.oplog.clone();
                let dropped_call = self.dropped_call_snapshot(self.live_call_permit.clone());
                // As in `complete`: surface a deferred request-upload failure at the call site before
                // recording the `Cancelled` that references the request. A no-op when the request was
                // inline or eagerly uploaded.
                dropped_call.wait_request_upload().await?;
                let partial_payload = upload_partial_response(&oplog, partial).await?;
                dropped_call
                    .append_cancelled_with_oplog(oplog, partial_payload)
                    .await?;
                DurableCallCoordinator::new(ctx)
                    .finish(self.retry.function_type(), self.boundary, false, None)
                    .await?;
            }
        } else {
            let replay = self
                .replay
                .take()
                .expect("cancel() in replay called on a live handle");
            let resolution = ctx.state.replay_state.await_resolution(replay).await?;
            expect_cancelled_resolution(&resolution)?;
            DurableCallCoordinator::new(ctx)
                .finish(self.retry.function_type(), self.boundary, false, None)
                .await?;
        }
        Ok(())
    }

    /// Accessor-window cancellation for p3 durable calls.
    pub async fn cancel_access<T, D, Ctx>(
        mut self,
        store: &Accessor<T, D>,
        get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
        partial: Option<Pair::Resp>,
    ) -> Result<(), TerminalCallError>
    where
        T: 'static,
        D: HasData + ?Sized,
        Ctx: WorkerCtx,
    {
        let context = self.trap_context();
        if let Err(err) = drain_dropped_call_events_access(store, get_ctx).await {
            self.abandon_for_trap();
            return Err(err);
        }
        self.cancel_access_impl(store, get_ctx, partial)
            .await
            .map_err(|source| TerminalCallError::new(source, context))
    }

    async fn cancel_access_impl<T, D, Ctx>(
        mut self,
        store: &Accessor<T, D>,
        get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
        partial: Option<Pair::Resp>,
    ) -> Result<(), WorkerExecutorError>
    where
        T: 'static,
        D: HasData + ?Sized,
        Ctx: WorkerCtx,
    {
        self.ensure_accessor_terminal_supported("cancel_access")?;
        self.finished = true;
        let function_type = self.retry.function_type().clone();
        let begin_index = self.begin_index();
        if self.is_live {
            if self.persisted {
                let oplog = store.with(|mut access| get_ctx(access.data_mut()).state.oplog.clone());
                let mut guard = AccessTerminalGuard::<P>::new(
                    self.dropped_call_snapshot(self.live_call_permit.clone()),
                    self.drop_sink.clone(),
                    self.cleanup_sink.clone(),
                    self.teardown_probe(),
                );
                let result = async {
                    guard
                        .call()
                        .expect("terminal guard is armed")
                        .wait_request_upload()
                        .await?;
                    let partial_payload = upload_partial_response(&oplog, partial).await?;
                    let call = guard.call().expect("terminal guard is armed").clone();
                    let cancelled = OplogEntry::Cancelled {
                        timestamp: Timestamp::now_utc(),
                        start_index: call.start_idx,
                        partial: partial_payload,
                        span_finished: call.span_finished.clone(),
                    };
                    call.notify_span_closed();
                    // Reserve the cancellation in oplog order before spawning its waiter. A cut
                    // accepted immediately afterward can then choose a horizon that includes it.
                    let append = oplog.enqueue_add(cancelled);
                    let terminal = tokio::spawn(async move {
                        append.await?;
                        Ok(())
                    });
                    // Cancellation terminal: never a discarded completion, no marker.
                    guard.cleanup_after_terminal(terminal, None);
                    guard.wait_terminal().await
                }
                .await;
                if let Err(err) = result {
                    guard.disarm();
                    return Err(err);
                }
                end_durable_function_access(store, get_ctx, function_type, begin_index, false)
                    .await?;
                guard.disarm();
            }
        } else {
            let replay_state =
                store.with(|mut access| get_ctx(access.data_mut()).state.replay_state.clone());
            let replay = self
                .replay
                .take()
                .expect("cancel_access() in replay called on a live handle");
            let resolution = replay_state.await_resolution(replay).await?;
            expect_cancelled_resolution(&resolution)?;
            end_durable_function_access(store, get_ctx, function_type, begin_index, false).await?;
        }
        Ok(())
    }

    fn ensure_accessor_terminal_supported(
        &self,
        operation: &str,
    ) -> Result<(), WorkerExecutorError> {
        if is_accessor_terminal_supported_function_type(self.retry.function_type()) {
            Ok(())
        } else {
            Err(WorkerExecutorError::runtime(format!(
                "{operation} supports only ReadLocal/WriteLocal/ReadRemote/WriteRemote durable calls, got {:?}",
                self.retry.function_type()
            )))
        }
    }

    async fn prepare_incomplete_live_repair_at<Ctx: WorkerCtx>(
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

    /// The response of an incomplete call whose `Start` records a denial: the host denied the
    /// call before it ran, so the repair completes the `Start` with the denied response and does
    /// not run the operation. A `Start` that opens a span gets the span transition that closes it
    /// as denied, as the live denial does. `None` when the `Start` records no denial.
    async fn recorded_denied_response<Ctx: WorkerCtx>(
        &mut self,
        access: &mut impl DurableContext<Ctx>,
    ) -> Result<
        Option<(Pair::Resp, Option<golem_common::model::oplog::SpanFinished>)>,
        WorkerExecutorError,
    > {
        let oplog = access.with_ctx(|ctx| ctx.state.oplog.clone());
        let start_idx = self.start_idx;
        let result = async {
            let (request, denial, span_started) = match oplog.read(start_idx).await? {
                OplogEntry::Start { denial: None, .. } => return Ok(None),
                OplogEntry::Start {
                    request: Some(request),
                    denial: Some(denial),
                    span_started,
                    ..
                } => (request, denial, span_started),
                entry => {
                    return Err(WorkerExecutorError::unexpected_oplog_entry(
                        format!("denied {} Start with a request", Pair::FQFN),
                        format!("{entry:?}"),
                    ));
                }
            };
            let request = oplog.download_payload(request).await.map_err(|error| {
                WorkerExecutorError::runtime(format!(
                    "failed to load the request of the denied {} call at {}: {error}",
                    Pair::FQFN,
                    start_idx
                ))
            })?;
            let request = Pair::Req::try_from(request).map_err(|actual| {
                WorkerExecutorError::unexpected_oplog_entry(Pair::FQFN.to_string(), actual)
            })?;
            let span_finished =
                span_started.map(|started| golem_common::model::oplog::SpanFinished {
                    span_id: started.span_id,
                    finished_at: Timestamp::now_utc(),
                    outcome: golem_common::model::oplog::SpanOutcome::Denied,
                });
            Pair::denied_response(&request, &denial)
                .map(|response| Some((response, span_finished)))
                .ok_or_else(|| {
                    WorkerExecutorError::unexpected_oplog_entry(
                        format!("{} Start without a denial", Pair::FQFN),
                        format!("denial at {}: {denial}", start_idx),
                    )
                })
        }
        .await;
        if result.is_err() {
            self.abandon_for_trap();
        }
        result
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
    async fn prepare_incomplete_live_repair(
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

/// Parks a replaying call forever, after the caller marked its replay as undelivered: the
/// recorded run never delivered this terminal to the guest, which will therefore drop this
/// future at the same point it did live. This future never completes; the return type exists
/// only so a `match` arm can produce whatever type its siblings do.
async fn park_undelivered_forever<T>() -> T {
    let never: std::convert::Infallible = std::future::pending().await;
    match never {}
}

/// The recorded response payload a replayed delivery decodes.
#[derive(Debug)]
pub(super) enum ReplayedPayload {
    /// A successful `End`'s response (which must be present; see
    /// [`decode_completed_response`]).
    Completed(Option<OplogPayload<HostResponse>>),
    /// A `Cancelled` entry's partial response.
    CancelledPartial(OplogPayload<HostResponse>),
}

/// A recorded terminal that never reached the guest in the recorded run. What to do with it is
/// each replay path's policy: the direct path treats it as replay divergence (see
/// [`Self::direct_divergence_error`]), the accessor path parks so the deterministic guest drops
/// the future at the same point it did live, and the deferred-accessor path re-delivers a
/// discarded completion to the caller's deterministic post-`End` continuation.
#[derive(Debug)]
pub(super) enum UndeliveredTerminal {
    /// `Cancelled { partial: None }`: the call's future was dropped mid-flight (e.g. the loser
    /// of a guest `race`/`select!`) without ever returning a value.
    CancelledWithoutPartial { cancelled_idx: OplogIndex },
    /// A successful `End` (at `end_idx`) whose completion the guest discarded before delivery
    /// (recorded by the `CompletionDiscarded` marker at `marker_idx`), carrying the persisted
    /// response the deferred-delivery replay path decodes and re-delivers.
    CompletionDiscarded {
        end_idx: OplogIndex,
        marker_idx: OplogIndex,
        response: Option<OplogPayload<HostResponse>>,
    },
}

impl UndeliveredTerminal {
    fn log_parking(&self) {
        match self {
            UndeliveredTerminal::CancelledWithoutPartial { cancelled_idx } => {
                tracing::debug!(
                    "durable call cancelled without partial at {cancelled_idx} during replay; \
                     parking until the guest drops it"
                );
            }
            UndeliveredTerminal::CompletionDiscarded {
                end_idx,
                marker_idx,
                ..
            } => {
                tracing::debug!(
                    "durable call completed at {end_idx} but its completion was discarded by the \
                     guest (marker at {marker_idx}); parking until the guest drops it"
                );
            }
        }
    }

    /// The replay-divergence error the direct (p2) path reports for an undelivered terminal.
    /// The direct path is serialized by the store borrow and delivers at the resolution point,
    /// so a terminal that was never delivered to the guest cannot legally occur on it: an
    /// undelivered cancellation means the future was dropped mid-flight (accessor-only), and
    /// discarded completions are recorded only on the accessor completion path — calls replay
    /// through the same path they recorded on.
    pub(super) fn direct_divergence_error(&self) -> WorkerExecutorError {
        match self {
            UndeliveredTerminal::CancelledWithoutPartial { cancelled_idx } => {
                WorkerExecutorError::unexpected_oplog_entry(
                    "End or Cancelled { partial: Some(..) }",
                    format!("Cancelled without partial at {cancelled_idx}"),
                )
            }
            UndeliveredTerminal::CompletionDiscarded {
                end_idx,
                marker_idx,
                ..
            } => WorkerExecutorError::unexpected_oplog_entry(
                "End delivered to the guest",
                format!(
                    "End at {end_idx} marked CompletionDiscarded at {marker_idx} for a non-accessor durable call"
                ),
            ),
        }
    }
}

/// Mode-independent classification of a replayed durable-call resolution outcome, as produced by
/// [`classify_replay_resolution`]. This is the single transition model shared by the direct,
/// accessor, and deferred-accessor replay paths; the classification is total, and each path
/// expresses its own policy for the [`Self::Undelivered`] case by matching at the call site (see
/// [`UndeliveredTerminal`]).
#[derive(Debug)]
pub(super) enum ReplayedResolution {
    /// The recorded run delivered (or, for a markerless completed `End`, must still deliver)
    /// this payload to the guest: decode it, close the durable scope, run its deterministic
    /// post-`End` continuation, then gate the guest handoff according to the
    /// [`ReplayDeliveryDisposition`].
    Delivered(ReplayedPayload, ReplayDeliveryDisposition),
    /// The recorded terminal never reached the guest.
    Undelivered(UndeliveredTerminal),
    /// The call's `Start` was committed but its terminal never was: candidate for live repair
    /// (re-execution) when the function type allows it.
    Incomplete,
}

/// Pure classification of a replayed durable-call resolution outcome, shared by the direct,
/// accessor, and deferred-accessor replay paths; the per-path policies (divergence vs park vs
/// deferred re-delivery, store access, scope close, park bookkeeping) stay in the respective
/// methods.
pub(super) fn classify_replay_resolution(outcome: ResolutionOutcome) -> ReplayedResolution {
    match outcome {
        ResolutionOutcome::Incomplete => ReplayedResolution::Incomplete,
        ResolutionOutcome::Resolved(Resolution::Completed {
            response,
            delivery_marker,
            ..
        }) => ReplayedResolution::Delivered(
            ReplayedPayload::Completed(response),
            match delivery_marker {
                Some(marker_idx) => ReplayDeliveryDisposition::AtMarker(marker_idx),
                // A completed `End` without a delivery marker: the recorded run crashed after
                // the `End` became durable but before the completion crossed to the guest.
                None => ReplayDeliveryDisposition::AtReplayTail,
            },
        ),
        ResolutionOutcome::Resolved(Resolution::Cancelled {
            cancelled_idx,
            partial,
        }) => match partial {
            Some(payload) => ReplayedResolution::Delivered(
                ReplayedPayload::CancelledPartial(payload),
                // A cancellation's partial result is delivered at the guest-initiated (and
                // therefore deterministic) cancellation point: never marker-bearing, no gating.
                ReplayDeliveryDisposition::Immediate,
            ),
            // `Cancelled` with no partial result: in the recorded run this call never returned a
            // value to the guest — its future was dropped mid-flight.
            None => ReplayedResolution::Undelivered(UndeliveredTerminal::CancelledWithoutPartial {
                cancelled_idx,
            }),
        },
        // The recorded run persisted a successful `End`, but a `CompletionDiscarded` marker
        // records that the guest dropped the completion future before the response was
        // delivered.
        ResolutionOutcome::Resolved(Resolution::CompletedButDiscarded {
            end_idx,
            marker_idx,
            response,
        }) => ReplayedResolution::Undelivered(UndeliveredTerminal::CompletionDiscarded {
            end_idx,
            marker_idx,
            response,
        }),
    }
}

/// Serializes and uploads a cancellation's optional partial response payload, shared by the
/// direct and accessor cancellation paths. The caller is responsible for site-specific failure
/// cleanup through its terminal guard.
async fn upload_partial_response<Resp: Into<HostResponse> + Send + 'static>(
    oplog: &Arc<dyn Oplog>,
    partial: Option<Resp>,
) -> Result<Option<OplogPayload<HostResponse>>, WorkerExecutorError> {
    match partial {
        Some(partial) => {
            let partial = Box::new(partial);
            let prepared =
                join_or_park_at_runtime_shutdown(tokio::task::spawn_blocking(move || {
                    let host_response: HostResponse = (*partial).into();
                    let bytes = golem_common::serialization::serialize(&host_response)?;
                    Ok::<_, String>(Box::new((bytes, Arc::new(host_response))))
                }))
                .await
                .map_err(|err| {
                    WorkerExecutorError::runtime(format!(
                        "partial durable call response encoding task failed: {err}"
                    ))
                })?
                .map_err(|err| {
                    WorkerExecutorError::runtime(format!(
                        "failed to serialize partial durable call response: {err}"
                    ))
                })?;
            let (bytes, cached) = *prepared;
            let raw_payload = oplog.upload_raw_payload(bytes).await.map_err(|err| {
                WorkerExecutorError::runtime(format!(
                    "failed to store partial durable call response: {err}"
                ))
            })?;
            Ok(Some(raw_payload.into_payload_with_cache(cached).map_err(
                |err| {
                    WorkerExecutorError::runtime(format!(
                        "failed to prepare partial durable call response payload: {err}"
                    ))
                },
            )?))
        }
        None => Ok(None),
    }
}

/// Decodes a replayed delivery's recorded payload into the call's typed response.
async fn decode_replayed_payload<Pair: HostPayloadPair>(
    oplog: &Arc<dyn Oplog>,
    payload: ReplayedPayload,
    retry_from: OplogIndex,
) -> Result<Pair::Resp, WorkerExecutorError> {
    match payload {
        ReplayedPayload::Completed(response) => {
            decode_completed_response::<Pair>(oplog, response, retry_from).await
        }
        ReplayedPayload::CancelledPartial(payload) => {
            download_and_decode_response::<Pair>(
                oplog,
                payload,
                "Cancelled partial payload",
                retry_from,
            )
            .await
        }
    }
}

/// Downloads a recorded response payload and decodes it into the call's typed response,
/// preserving the canonical error classification: backend unavailability requires reconstruction,
/// corrupt or missing data and type mismatches are unexpected oplog entries.
async fn download_and_decode_response<Pair: HostPayloadPair>(
    oplog: &Arc<dyn Oplog>,
    payload: OplogPayload<HostResponse>,
    payload_kind: &str,
    retry_from: OplogIndex,
) -> Result<Pair::Resp, WorkerExecutorError> {
    let host_response = oplog
        .download_payload_classified(payload)
        .await
        .map_err(|error| match error {
            crate::services::oplog::OplogPayloadDownloadError::Backend(error) => {
                WorkerExecutorError::recovery_required_from(
                    format!("{payload_kind} cannot be downloaded: {error}"),
                    retry_from,
                )
            }
            crate::services::oplog::OplogPayloadDownloadError::Corrupt(error) => {
                WorkerExecutorError::unexpected_oplog_entry(
                    "valid durable call response payload",
                    format!("{payload_kind} is corrupt: {error}"),
                )
            }
        })?;
    host_response
        .try_into()
        .map_err(|err| WorkerExecutorError::unexpected_oplog_entry(Pair::FQFN, err))
}

/// Decodes a replayed successful completion's response: requires the recorded `End` to carry a
/// response payload, then downloads and decodes it via [`download_and_decode_response`].
async fn decode_completed_response<Pair: HostPayloadPair>(
    oplog: &Arc<dyn Oplog>,
    response: Option<OplogPayload<HostResponse>>,
    retry_from: OplogIndex,
) -> Result<Pair::Resp, WorkerExecutorError> {
    let payload = response.ok_or_else(|| {
        WorkerExecutorError::unexpected_oplog_entry(
            "End { response: Some(..) }",
            "End { response: None }".to_string(),
        )
    })?;
    download_and_decode_response::<Pair>(oplog, payload, "End payload", retry_from).await
}

/// Validates a replay-side cancellation: the recorded resolution must be `Cancelled`. A recorded
/// completion (delivered or discarded) means the replayed guest cancelled a call that the
/// recorded run completed — a replay divergence.
fn expect_cancelled_resolution(resolution: &Resolution) -> Result<(), WorkerExecutorError> {
    match resolution {
        Resolution::Completed { end_idx, .. }
        | Resolution::CompletedButDiscarded { end_idx, .. } => Err(
            WorkerExecutorError::unexpected_oplog_entry("Cancelled", format!("End at {end_idx}")),
        ),
        Resolution::Cancelled { .. } => Ok(()),
    }
}

/// Store-free persistence preparation shared by the direct ([`DurableCallSession::complete`]) and
/// accessor ([`DurableCallSession::persist_access_terminal`]) completion paths: waits for the
/// (possibly deferred) request upload, encodes and uploads the response payload, and constructs
/// the terminal `End` entry — without appending it or closing
/// scopes, which the two paths deliberately order differently (direct appends inline; accessor
/// hands the append to an owned task via its terminal guard).
async fn prepare_end_entry<Pair: HostPayloadPair>(
    oplog: &Arc<dyn Oplog>,
    request_upload: &PendingUpload,
    start_idx: OplogIndex,
    response: Pair::Resp,
    span_finished: Option<golem_common::model::oplog::SpanFinished>,
    span_attributes: Option<golem_common::model::oplog::SpanAttributes>,
) -> Result<(Pair::Resp, OplogEntry), WorkerExecutorError> {
    // Surface a deferred request-upload failure here, at the call site, before recording the
    // `End` that references the request. The leaf oplog's commit barrier is the backstop, but
    // awaiting here turns an upload failure into a graceful error instead of a commit-time
    // panic. A no-op when the request was inline or eagerly uploaded.
    request_upload.wait().await.map_err(|err| {
        WorkerExecutorError::runtime(format!(
            "failed to serialize and store durable call request: {err}"
        ))
    })?;

    // Flat responses have input-independent encoding depth and need no thread-pool handoff.
    // Recursive or unaudited response families still use a fresh stack: this can be called inside
    // a Wasmtime fiber, whose stack is independent of the native runtime thread's stack.
    let response = Box::new(response);
    let prepared = if response_has_bounded_encoding_stack::<Pair::Resp>() {
        encode_response(response)
    } else {
        join_or_park_at_runtime_shutdown(tokio::task::spawn_blocking(move || {
            encode_response(response)
        }))
        .await
        .map_err(|err| {
            WorkerExecutorError::runtime(format!(
                "durable call response encoding task failed: {err}"
            ))
        })?
    }
    .map_err(|err| {
        WorkerExecutorError::runtime(format!("failed to serialize durable call response: {err}"))
    })?;
    let (response, bytes) = *prepared;
    let raw_payload = oplog.upload_raw_payload(bytes).await.map_err(|err| {
        WorkerExecutorError::runtime(format!("failed to store durable call response: {err}"))
    })?;
    let response_payload = raw_payload.into_payload().map_err(|err| {
        WorkerExecutorError::runtime(format!(
            "failed to prepare durable call response payload: {err}"
        ))
    })?;
    Ok((
        *response,
        OplogEntry::End {
            timestamp: Timestamp::now_utc(),
            start_index: start_idx,
            response: Some(response_payload),
            forced_commit: false,
            span_finished,
            span_attributes,
        },
    ))
}

fn response_has_bounded_encoding_stack<Resp: 'static>() -> bool {
    use golem_common::model::oplog::payload::*;
    use std::any::TypeId;

    // Only audited scalar/flat collection families are admitted. In particular, a small value of
    // a recursive schema-bearing type is not safe to classify by its current size or variant.
    [
        TypeId::of::<HostResponseBlobStoreUnit>(),
        TypeId::of::<HostResponseBlobStoreGetData>(),
        TypeId::of::<HostResponseBlobStoreListObjects>(),
        TypeId::of::<HostResponseBlobStoreContains>(),
        TypeId::of::<HostResponseBlobStoreTimestamp>(),
        TypeId::of::<HostResponseBlobStoreOptionalTimestamp>(),
        TypeId::of::<HostResponseMonotonicClockTimestamp>(),
        TypeId::of::<HostResponseWallClock>(),
        TypeId::of::<HostResponseGolemAgentWebhookUrl>(),
        TypeId::of::<HostResponseGolemApiIdempotencyKey>(),
        TypeId::of::<HostResponseGolemApiPromiseCompletion>(),
        TypeId::of::<HostResponseGolemApiPromiseResult>(),
        TypeId::of::<HostResponseGolemApiUnit>(),
        TypeId::of::<HostResponseGolemRpcUnit>(),
        TypeId::of::<HostResponseKVGet>(),
        TypeId::of::<HostResponseKVGetMany>(),
        TypeId::of::<HostResponseKVDelete>(),
        TypeId::of::<HostResponseKVKeys>(),
        TypeId::of::<HostResponseKVUnit>(),
        TypeId::of::<HostResponsePollReady>(),
        TypeId::of::<HostResponsePollResult>(),
        TypeId::of::<HostResponseRandomBytes>(),
        TypeId::of::<HostResponseRandomU64>(),
        TypeId::of::<HostResponseRandomSeed>(),
        TypeId::of::<HostResponseP3MonotonicClockUnit>(),
        TypeId::of::<HostResponseStreamCheckWrite>(),
        TypeId::of::<HostResponseStreamSkip>(),
        TypeId::of::<HostResponseQuotaTokenAcquired>(),
        TypeId::of::<HostResponseQuotaCommitResult>(),
        TypeId::of::<HostResponsePermissionCardDerived>(),
        TypeId::of::<HostResponsePermissionCardLiveness>(),
        TypeId::of::<HostResponsePermissionCardDecision>(),
        TypeId::of::<HostResponsePermissionCardTransferComplete>(),
        TypeId::of::<HostResponsePermissionWallet>(),
        TypeId::of::<HostResponsePermissionWalletVersion>(),
        TypeId::of::<HostResponseP3KeyvalueIncomingValueStream>(),
        TypeId::of::<HostResponseP3BlobstoreIncomingValueStream>(),
        TypeId::of::<HostResponseP3HttpClientConsumeBodyChunk>(),
        TypeId::of::<HostResponseP3SocketsTcpReceiveChunk>(),
        TypeId::of::<HostResponseGolemApiCard>(),
        TypeId::of::<HostResponseGolemApiOplogEntries>(),
        TypeId::of::<HostResponseConfigGetResponse>(),
        TypeId::of::<HostResponseConfigGetAllResponse>(),
        TypeId::of::<HostResponseCliEnvironmentGetEnvironment>(),
    ]
    .contains(&TypeId::of::<Resp>())
}

type EncodedResponse<Resp> = Box<(Box<Resp>, Vec<u8>)>;

// Keep the large HostResponse enum's construction/serialization frame out of the async caller.
// Box the input and output so the blocking-task envelope also has a bounded stack footprint.
#[inline(never)]
#[allow(
    clippy::boxed_local,
    reason = "the caller must not move the generic response onto its small Wasmtime fiber stack"
)]
fn encode_response<Resp: Into<HostResponse> + TryFrom<HostResponse, Error = String>>(
    response: Box<Resp>,
) -> Result<EncodedResponse<Resp>, String> {
    let host_response: HostResponse = (*response).into();
    let bytes = golem_common::serialization::serialize(&host_response)?;
    let response = Box::new(Resp::try_from(host_response)?);
    Ok(Box::new((response, bytes)))
}

#[cfg(test)]
mod response_encoding_tests {
    use super::*;
    use golem_common::model::oplog::payload::{
        HostResponseGolemRpcInvokeAndAwait, HostResponseGolemToolTools, HostResponseRandomBytes,
    };
    use test_r::test;

    #[test]
    fn response_encoding_preserves_payload() {
        let response = HostResponseRandomBytes {
            bytes: vec![1, 2, 3, 4],
        };
        let expected: HostResponse = response.clone().into();
        let (returned, bytes) = *encode_response(Box::new(response.clone())).unwrap();
        assert_eq!(*returned, response);
        assert_eq!(
            bytes,
            golem_common::serialization::serialize(&expected).unwrap()
        );
        assert_eq!(
            golem_common::serialization::deserialize::<HostResponse>(&bytes).unwrap(),
            expected
        );
        assert!(response_has_bounded_encoding_stack::<HostResponseRandomBytes>());
        assert!(!response_has_bounded_encoding_stack::<
            HostResponseGolemRpcInvokeAndAwait,
        >());
        assert!(!response_has_bounded_encoding_stack::<
            HostResponseGolemToolTools,
        >());
    }

    #[test]
    async fn response_encoding_on_wasmtime_fiber() -> anyhow::Result<()> {
        let config = golem_common::wasmtime_config::create_wasmtime_config_without_fs_cache();
        let engine = wasmtime::Engine::new(&config)?;
        let module = wasmtime::Module::new(
            &engine,
            r#"(module
                (import "host" "encode" (func $encode))
                (func $nested (param $depth i32) (result i32)
                    local.get $depth
                    if (result i32)
                        local.get $depth
                        i32.const 1
                        i32.sub
                        call $nested
                        i32.const 1
                        i32.add
                    else
                        call $encode
                        i32.const 0
                    end)
                (func (export "run") (result i32)
                    i32.const 512
                    call $nested))"#,
        )?;
        let mut linker = wasmtime::Linker::new(&engine);
        linker.func_wrap("host", "encode", || -> wasmtime::Result<()> {
            for length in [0, 4, 64 * 1024] {
                let response = HostResponseRandomBytes {
                    bytes: vec![42; length],
                };
                let (returned, bytes) = *encode_response(Box::new(response))
                    .map_err(wasmtime::Error::msg)?;
                assert_eq!(returned.bytes.len(), length);
                assert!(!bytes.is_empty());
                assert!(matches!(golem_common::serialization::deserialize::<HostResponse>(&bytes).unwrap(), HostResponse::RandomBytes(value) if value.bytes == returned.bytes));
            }
            Ok(())
        })?;
        let mut store = wasmtime::Store::new(&engine, ());
        store.set_fuel(u64::MAX)?;
        store.set_epoch_deadline(u64::MAX);
        let instance = linker.instantiate_async(&mut store, &module).await?;
        let run = instance.get_typed_func::<(), i32>(&mut store, "run")?;
        assert_eq!(run.call_async(&mut store, ()).await?, 512);
        Ok(())
    }
}

impl ResolvedReconstructionTerminal {
    pub(in crate::durable_host) async fn finish_access<T, D, Ctx>(
        self,
        store: &Accessor<T, D>,
        get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
    ) -> Result<bool, WorkerExecutorError>
    where
        T: 'static,
        D: HasData + ?Sized,
        Ctx: WorkerCtx,
    {
        let cancelled = self.cancelled();
        end_durable_function_access(store, get_ctx, self.function_type, self.begin_index, false)
            .await?;
        self.delivery.deliver_at_accessor_terminal(store).await?;
        Ok(cancelled)
    }
}

pub(crate) async fn end_durable_function_access<T, D, Ctx>(
    store: &(impl DurableAccess<T, D> + ?Sized),
    get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
    function_type: DurableFunctionType,
    begin_index: OplogIndex,
    forced_commit: bool,
) -> Result<(), WorkerExecutorError>
where
    T: 'static,
    D: HasData + ?Sized,
    Ctx: WorkerCtx,
{
    end_durable_function_access_with_span(
        store,
        get_ctx,
        function_type,
        begin_index,
        forced_commit,
        None,
    )
    .await
}

pub(crate) async fn end_durable_function_access_with_span<T, D, Ctx>(
    store: &(impl DurableAccess<T, D> + ?Sized),
    get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
    function_type: DurableFunctionType,
    _begin_index: OplogIndex,
    forced_commit: bool,
    _span_finished: Option<golem_common::model::oplog::SpanFinished>,
) -> Result<(), WorkerExecutorError>
where
    T: 'static,
    D: HasData + ?Sized,
    Ctx: WorkerCtx,
{
    let (snapshotting_mode, public_state) = store.with(|mut access| {
        let ctx = get_ctx(access.data_mut());
        (ctx.state.snapshotting_mode, ctx.public_state.clone())
    });

    if snapshotting_mode {
        return Ok(());
    }

    if function_type == DurableFunctionType::WriteRemote || forced_commit {
        public_state
            .worker()
            .commit_oplog_and_update_state(CommitLevel::DurableOnly)
            .await?;
        if store.with(|mut access| {
            let ctx = get_ctx(access.data_mut());
            ctx.state.at_clean_checkpoint_boundary()
        }) {
            public_state
                .worker()
                .checkpoint_status_mid_invocation()
                .await;
        }
    }

    Ok(())
}

/// Finishes a span in the in-memory invocation context only, without writing or consuming any
/// oplog entry: pops the current-span pointer if it points at this span, then marks the span
/// finished. The owning durable operation records the span transition separately.
pub(crate) fn finish_span_in_memory<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    span_id: &SpanId,
) -> Result<(), WorkerExecutorError> {
    if &ctx.state.current_span_id == span_id {
        let span = ctx.state.invocation_context.get(span_id).map_err(|err| {
            WorkerExecutorError::runtime(format!(
                "span {span_id} missing during finish_span replay: {err}"
            ))
        })?;
        ctx.state.current_span_id = span
            .parent()
            .map(|p| p.span_id().clone())
            .unwrap_or_else(|| ctx.state.invocation_context.root.span_id().clone());
    }
    let _ = ctx
        .state
        .invocation_context
        .finish_span(span_id)
        .map_err(WorkerExecutorError::runtime);
    Ok(())
}

fn is_accessor_supported_function_type(function_type: &DurableFunctionType) -> bool {
    matches!(
        function_type,
        DurableFunctionType::ReadLocal
            | DurableFunctionType::WriteLocal
            | DurableFunctionType::ReadRemote
            | DurableFunctionType::WriteRemote
    )
}

fn is_accessor_terminal_supported_function_type(function_type: &DurableFunctionType) -> bool {
    is_accessor_supported_function_type(function_type)
        || matches!(function_type, DurableFunctionType::WriteRemote)
}

/// The first phase of a two-phase durable call, produced by [`DurableCallSession::begin`]. The
/// boundary index is known; the host-call `Start` has not yet been
/// written or claimed. Resolve replay before preparing a live request with [`Self::resolve`].
pub struct BegunCall<Pair: HostPayloadPair, P: DropPolicy> {
    boundary: DurableCallBoundary,
    execution_scope: BegunCallExecutionScope,
    retry: InFunctionRetryController,
    requires_agent_authority: bool,
    agent_auth_ctx: Option<AuthCtx>,
    drop_sink: Option<UnboundedSender<DropEvent>>,
    cleanup_sink: Option<UnboundedSender<DropEvent>>,
    _phantom: PhantomData<(Pair, P)>,
}

impl<Pair: HostPayloadPair, P: DropPolicy> BegunCall<Pair, P> {
    fn is_live(&self) -> bool {
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
                        ctx.state.authoritative_replay_failure = true;
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

    fn replay_claim(&self) -> StartClaim {
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
            requires_agent_authority: self.requires_agent_authority,
            agent_auth_ctx: self.agent_auth_ctx,
            drop_sink: self.drop_sink,
            cleanup_sink: self.cleanup_sink,
            live_call_permit: None,
            _phantom: PhantomData,
        }
    }
}

impl<Pair: HostPayloadPair, P: DropPolicy> Drop for DurableCallSession<Pair, P> {
    fn drop(&mut self) {
        if self.finished {
            return;
        }
        if self.teardown_probe().is_tearing_down() {
            tracing::debug!(
                start_idx = %self.start_idx,
                "durable call abandoned during executor shutdown"
            );
            return;
        }
        if self.is_live {
            if self.persisted {
                let live_call_permit = self.live_call_permit.take();
                // A live call dropped without finish/cancel: run the compile-time drop policy.
                P::unfinished_drop(
                    self.dropped_call_snapshot(live_call_permit),
                    self.drop_sink.as_ref(),
                );
            }
            // Not persisted: there is nothing on disk to reconcile.
        } else {
            if self.parked_undelivered_replay {
                // Expected: the recorded terminal was never delivered to the guest live
                // (`Cancelled { partial: None }` or an `End` marked `CompletionDiscarded`) and
                // the deterministic guest dropped this future at the same point it did live.
                tracing::debug!(
                    "parked undelivered durable call replay handle for Start {} dropped by the guest",
                    self.start_idx
                );
            } else {
                // A replay handle must never enqueue a live cancellation; just note the anomaly.
                tracing::warn!(
                    "replay durable call handle for Start {} dropped without finishing",
                    self.start_idx
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_r::test;

    #[test]
    #[test_r::timeout("10s")]
    async fn direct_claim_with_a_wrong_invocation_id_does_not_consume_the_start() {
        use super::super::tests::InMemoryOplog;
        use golem_common::model::oplog::HostRequestGolemRpcResource;
        use golem_common::model::oplog::host_functions::GolemRpcWasmRpcDrop;

        let recorded_invocation_id = DurableInvocationId::new(OplogIndex::from_u64(1), 7);
        for parent_start_index in [None, Some(OplogIndex::from_u64(19))] {
            let request = HostRequestGolemRpcResource {
                creation_index: OplogIndex::from_u64(7),
            };
            let oplog = Arc::new(InMemoryOplog::new());
            oplog
                .add(OplogEntry::RecoverySucceeded {
                    timestamp: Timestamp::now_utc(),
                })
                .await
                .unwrap();
            let start_index = oplog
                .add(OplogEntry::Start {
                    timestamp: Timestamp::now_utc(),
                    parent_start_index,
                    function_name: GolemRpcWasmRpcDrop::HOST_FUNCTION_NAME,
                    invocation_id: Some(recorded_invocation_id),
                    observational_owner: None,
                    request: Some(OplogPayload::Inline(Box::new(request.clone().into()))),
                    durable_function_type: DurableFunctionType::ReadLocal,
                    span_started: None,
                    denial: None,
                })
                .await
                .unwrap();
            oplog
                .add(OplogEntry::End {
                    timestamp: Timestamp::now_utc(),
                    start_index,
                    response: None,
                    forced_commit: false,
                    span_finished: None,
                    span_attributes: None,
                })
                .await
                .unwrap();
            let replay_state = ReplayState::new_for_owner(
                golem_common::model::OwnedAgentId {
                    environment_id: golem_common::model::environment::EnvironmentId::new(),
                    agent_id: golem_common::model::AgentId {
                        component_id: golem_common::model::component::ComponentId::new(),
                        agent_id: "direct-claim-test".to_string(),
                    },
                },
                oplog.clone(),
                golem_common::model::regions::DeletedRegions::default(),
                None,
                crate::durable_host::tool::operation::OwnerToolOperations::new(),
                oplog.current_oplog_index().await,
                None,
            )
            .await
            .unwrap();
            let begun = |invocation_id| BegunCall::<GolemRpcWasmRpcDrop, NotCancellable> {
                boundary: DurableCallBoundary::from_begin_index(OplogIndex::from_u64(1)),
                execution_scope: BegunCallExecutionScope {
                    entity_parent_start_index: parent_start_index,
                    parent_start_index,
                    observational_owner: None,
                    invocation_id: Some(invocation_id),
                },
                retry: InFunctionRetryController::new(
                    DurableFunctionType::ReadLocal,
                    DurableExecutionState {
                        is_live: false,
                        snapshotting_mode: false,
                        max_in_function_retry_delay: std::time::Duration::ZERO,
                    },
                    GolemRpcWasmRpcDrop::FQFN,
                ),
                requires_agent_authority: false,
                agent_auth_ctx: None,
                drop_sink: None,
                cleanup_sink: None,
                _phantom: PhantomData,
            };
            let wrong = replay_state
                .claim_start_or_replay_end(
                    begun(DurableInvocationId::new(OplogIndex::from_u64(1), 8)).replay_claim(),
                )
                .await;
            // A top-level Start with another invocation id belongs to a concurrent sibling call,
            // so the claim retains it and goes live; under an unknown parent it is divergence.
            if parent_start_index.is_none() {
                assert!(matches!(wrong, Ok(ReplayStartClaimOutcome::ReplayEnded)));
            } else {
                assert!(wrong.is_err());
            }
            let claimed = replay_state
                .claim_start_or_replay_end(begun(recorded_invocation_id).replay_claim())
                .await
                .expect("a wrong invocation id must leave the recorded operation available");
            let ReplayStartClaimOutcome::Claimed { handle, .. } = claimed else {
                panic!("expected the matching Start to remain available");
            };
            assert_eq!(handle.start_idx(), start_index);
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
