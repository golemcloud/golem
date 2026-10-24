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

use super::drop_events::AccessDropEventDrainGuard;
use super::*;
use crate::durable_host::LiveAuthorizationPermit;
use crate::durable_host::durability::GuestImportIdentity;
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

mod access_start;
mod cancel;
mod claims;
mod completion_replay;
mod execution;
mod start;
mod terminal;
pub(crate) use access_start::access_invocation_id;
pub(in crate::durable_host::concurrent) use access_start::guest_import_invocation_id;
pub(crate) use access_start::import_invocation_id;
pub(crate) use access_start::reserve_invocation_id_before_await;
pub(crate) use access_start::resolve_current_observational_owner;
pub(crate) use access_start::resolve_observational_owner;
pub(crate) use claims::admission_refusal_before_start;
pub(crate) use claims::finish_prepared_access_to_live;
pub(crate) use claims::finish_prepared_access_to_live_at;
use claims::publish_incomplete_replay_tail_access;
pub(in crate::durable_host::concurrent) use claims::publish_incomplete_replay_tail_at;
pub(crate) use claims::reproduce_incomplete_start_refusal;
pub(in crate::durable_host::concurrent) use completion_replay::classify_replay_resolution;
use completion_replay::expect_cancelled_resolution;
pub(in crate::durable_host::concurrent) use execution::ambient_trap_context;
#[cfg(test)]
use terminal::encode_response;
pub(crate) use terminal::end_durable_function_access;
pub(crate) use terminal::finish_span_in_memory;
#[cfg(test)]
use terminal::response_has_bounded_encoding_stack;

/// Compile-time policy describing what happens when a [`DurableCallSession`] is dropped without being
/// explicitly finished or cancelled.
pub trait DropPolicy: Send + 'static {
    /// Store-local calls may settle by cancellation when their tool body permanently traps.
    /// Independently supervised calls retain their own terminal selection.
    const CANCEL_ON_TOOL_TRAP: bool = false;

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

/// The state machine for one durable host call.
///
/// Created by [`DurableCallSession::start`], which eagerly appends the call's `Start` (live) or
/// claims it from the oplog (replay). The call is finished with [`DurableCallSession::complete`]
/// (live) / [`DurableCallSession::replay`] (replay), or cancelled with
/// [`DurableCallSession::cancel`]. All terminal methods consume the session so a call cannot be
/// finished twice; an unfinished drop runs the `P` drop policy.
pub struct DurableCallSession<Pair: HostPayloadPair, P: DropPolicy> {
    pub(super) start_idx: OplogIndex,
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
    pub(super) tool_trap_disposal: Arc<AtomicBool>,
    /// An accepted entity Start whose body has not transferred to an independent supervisor.
    pub(super) entity_admission: bool,
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
    prepared: BegunCall<Pair, P, Ctx>,
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

/// The identity of an accessor call, captured in the call that makes it before its first
/// `await`: the custom invocation that observes it and the invocation id its records carry.
struct AccessCallIdentity {
    custom_invocation_scope: Option<CustomInvocationScope>,
    invocation_id: Option<DurableInvocationId>,
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

/// The first phase of a durable call: admitted, with its initiation-time scope captured, before
/// its host-call `Start` is claimed or appended. [`Self::resolve_against_history`] claims the
/// recorded `Start` or continues live at the replay tail; the accessor, context and direct
/// (Store-holding) start paths all produce and resolve this type.
pub(crate) struct BegunCall<Pair: HostPayloadPair, P: DropPolicy, Ctx: WorkerCtx> {
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
    /// The agent authority a direct call captured at admission or at the replay tail.
    agent_auth_ctx: Option<AuthCtx>,
    _phantom: PhantomData<(Pair, P)>,
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
/// A guest import call that makes durable calls reserves an identity in guest call order (see
/// [`crate::durable_host::durability::GuestImportNumbering`]). Replay claims a call's `Start`s by
/// that id. Calls observed inside a custom durable invocation record none: replay
/// never claims them.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) enum InvocationIdSource {
    /// The id of the guest import the call's accessor belongs to. A call made by a task that is
    /// not a guest import fails: such a task must say where its id comes from.
    #[default]
    GuestImport,
    /// The id of the guest import that issued this call, looked up by that import with
    /// [`access_invocation_id`] or [`import_invocation_id`] and passed to the task that makes the
    /// call. `None` means the import is not identified by an invocation id.
    Reserved(Option<DurableInvocationId>),
    /// The call is made by a host task that no single guest import issued. It records no id, and
    /// replay claims it in oplog order.
    HostTask,
}

impl InvocationIdSource {
    /// The id passed from the issuing guest import, or `None` for any other source.
    pub(crate) fn reserved(self) -> Option<DurableInvocationId> {
        match self {
            Self::Reserved(invocation_id) => invocation_id,
            Self::GuestImport | Self::HostTask => None,
        }
    }
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

#[cfg(test)]
mod scope_replay_recovery_tests;

struct ExecutedAccessStart<Pair: HostPayloadPair, P: DropPolicy> {
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

/// Owns the accepted Start across both sides of the metadata oneshot handoff.
struct AcceptedAccessStart<P: DropPolicy> {
    result: Option<(OplogIndex, PendingUpload)>,
    call: Option<DroppedCall>,
    marker: Arc<AtomicBool>,
    sink: Option<UnboundedSender<DropEvent>>,
    cleanup_sink: Option<UnboundedSender<DropEvent>>,
    teardown: TeardownProbe,
    _policy: PhantomData<P>,
}

impl<P: DropPolicy> AcceptedAccessStart<P> {
    fn take(mut self) -> (OplogIndex, PendingUpload) {
        self.call = None;
        self.result.take().expect("accepted Start result")
    }
}

impl<P: DropPolicy> Drop for AcceptedAccessStart<P> {
    fn drop(&mut self) {
        if let Some(call) = self.call.take() {
            if (P::CANCEL_ON_TOOL_TRAP || call.entity_admission)
                && self.marker.load(Ordering::Acquire)
            {
                Cancellable::unfinished_drop(call, self.cleanup_sink.as_ref());
            } else if !self.teardown.is_tearing_down() {
                P::unfinished_drop(call, self.sink.as_ref());
            }
        }
    }
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
pub(crate) enum ResolvedCall<Pair: HostPayloadPair, P: DropPolicy, Ctx: WorkerCtx> {
    Replay(DurableCallSession<Pair, P>),
    Live(BegunCall<Pair, P, Ctx>),
}

fn is_write_side_effect_for_access(function_type: &DurableFunctionType) -> bool {
    matches!(
        function_type,
        DurableFunctionType::WriteLocal | DurableFunctionType::WriteRemote
    )
}

struct ContextRetryHost<'a, Ctx: WorkerCtx> {
    task: TaskRetryContext<Ctx>,
    execution_state: &'a (dyn Fn() -> DurableExecutionState + Send + Sync),
}

pub(super) struct ScopedRetryHost<'a, H> {
    inner: &'a mut H,
    execution_scope: &'a CallExecutionScope,
}

impl<Pair: HostPayloadPair, P: DropPolicy> DurableCallSession<Pair, P> {
    pub fn is_live(&self) -> bool {
        self.is_live
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

    pub fn start_index(&self) -> OplogIndex {
        self.start_idx
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
}

/// The recorded response payload a replayed delivery decodes.
#[derive(Debug)]
pub(super) enum ReplayedPayload {
    /// A successful `End`'s response (which must be present; see
    /// [`completion_replay::decode_completed_response`]).
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

type EncodedResponse<Resp> = Box<(Box<Resp>, Vec<u8>)>;

#[cfg(test)]
mod response_encoding_tests;

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

#[cfg(test)]
mod tests;
