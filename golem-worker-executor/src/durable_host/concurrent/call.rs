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

pub(super) fn ambient_trap_context<Ctx: WorkerCtx>(
    ctx: &DurableWorkerCtx<Ctx>,
) -> DurableCallTrapContext {
    DurableCallTrapContext {
        retry_from: InFunctionRetryHost::current_retry_point(ctx),
        in_atomic_region: InFunctionRetryHost::in_atomic_region(ctx),
    }
}

/// Compile-time policy describing what happens when a [`CallHandle`] is dropped without being
/// explicitly finished or cancelled.
pub trait DropPolicy {
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

impl DropPolicy for Cancellable {
    fn production_drop_sink(
        sink: Option<UnboundedSender<DropEvent>>,
    ) -> Option<UnboundedSender<DropEvent>> {
        sink
    }

    fn unfinished_drop(call: DroppedCall, sink: Option<&UnboundedSender<DropEvent>>) {
        if let Some(sink) = sink {
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
        // No terminal will ever be recorded for this call (its committed `Start` is left
        // incomplete for replay), so its atomic-region membership ends here. The release is
        // store-free, so it is safe directly from the drop path.
        call.release_atomic_lease();
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

/// A handle to one durable host call.
///
/// Created by [`CallHandle::start`], which eagerly appends the call's `Start` (live) or claims it
/// from the oplog (replay). The call is finished with [`CallHandle::complete`] (live) /
/// [`CallHandle::replay`] (replay), or cancelled with [`CallHandle::cancel`]. All terminal methods
/// consume the handle so a call cannot be finished twice; an unfinished drop runs the `P` drop
/// policy.
pub struct CallHandle<Pair: HostPayloadPair, P: DropPolicy> {
    pub(super) start_idx: OplogIndex,
    /// The index returned by `begin_durable_function` / `begin_function`. For a non-idempotent
    /// `WriteRemote` (or a `WriteRemoteBatched(None)`) this is the **durable scope** `Start` that
    /// must be closed via `end_durable_function`; for every other function type it is just the
    /// pre-call index and `end_durable_function` only uses it to commit at the right boundary.
    pub(super) begin_index: OplogIndex,
    pub(super) is_live: bool,
    /// `true` when a `Start` entry was actually appended. It is `false` while snapshotting (where
    /// nothing is persisted) and for replay handles.
    pub(super) persisted: bool,
    /// Tracks the (possibly deferred) blob upload of this call's request payload, started when the
    /// `Start` was reserved. Awaited before the matching `End` / `Cancelled` is appended so an
    /// upload failure surfaces at the call site rather than only at the leaf oplog's commit barrier.
    /// `PendingUpload::already_durable()` (a no-op) for replay handles, snapshotting, and inline
    /// requests.
    pub(super) request_upload: PendingUpload,
    /// Replay-side resolver receiver; `Some` only for replay handles.
    pub(super) replay: Option<ReplayCallHandle>,
    pub(super) finished: bool,
    /// `true` when this replay handle parked on a recorded terminal that was never delivered to
    /// the guest — a `Cancelled { partial: None }` or an `End` marked `CompletionDiscarded` —
    /// waiting for the deterministic guest to drop it at the same point it did live. Makes that
    /// drop an expected state (debug-logged, scope close enqueued) rather than an anomaly.
    pub(super) parked_undelivered_replay: bool,
    /// Initiation-time execution metadata owned by this call, including its atomic-region
    /// ownership lease (see [`CallExecutionScope::atomic_lease`]). Later phases still mirror
    /// selected fields into `PrivateDurableWorkerState` for compatibility, but the call-owned copy
    /// is the source we can move retry/atomic decisions onto as the Accessor reshape proceeds.
    pub(super) execution_scope: CallExecutionScope,
    /// In-function retry decision logic. Also the home of the call's `DurableFunctionType` and
    /// captured `DurableExecutionState`.
    pub(super) retry: InFunctionRetryController,
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
    pub(super) fn new(counter: Arc<AtomicUsize>) -> Self {
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
    /// The durable scope this host-call `Start` will be nested under, if any. This is derived from
    /// the call's own function type / begin index, never from temporally-open sibling scopes.
    pub(super) parent_start_index: Option<OplogIndex>,
    /// Atomic region active when the durable call was initiated, captured so the call's membership
    /// lease is registered against the region it was actually started in.
    pub(super) atomic_region: Option<OplogIndex>,
    /// Persistence level active when the call was initiated. Kept with the call so p3 Accessor
    /// windows can snapshot all execution facts before async work resumes elsewhere.
    #[allow(dead_code)]
    pub(super) persistence_level: PersistenceLevel,
}

impl BegunCallExecutionScope {
    pub(super) fn finish(
        self,
        start_idx: OplogIndex,
        atomic_lease: Option<Arc<AtomicRegionLease>>,
    ) -> CallExecutionScope {
        CallExecutionScope {
            retry_from: self.parent_start_index.unwrap_or(start_idx),
            durable_scope: self.parent_start_index,
            atomic_lease,
            persistence_level: self.persistence_level,
        }
    }
}

#[derive(Debug, Clone)]
pub(super) struct CallExecutionScope {
    /// The retry point owned by this in-flight call: the enclosing durable scope `Start` if present,
    /// otherwise the host-call `Start` itself.
    pub(super) retry_from: OplogIndex,
    /// The enclosing durable scope, if this call belongs to one.
    #[allow(dead_code)]
    pub(super) durable_scope: Option<OplogIndex>,
    /// The call's atomic-region ownership lease, registered when the call was initiated inside an
    /// open atomic region. The lease's *current* owner — not the initiation-time region — drives
    /// trap/retry classification: region close transfers pending members to the enclosing region
    /// or detaches them, and a detached call must group at its own `retry_from` instead of
    /// retrying into the committed region.
    pub(super) atomic_lease: Option<Arc<AtomicRegionLease>>,
    /// Persistence level active when this call was initiated.
    #[allow(dead_code)]
    pub(super) persistence_level: PersistenceLevel,
}

impl CallExecutionScope {
    /// The atomic region *currently* owning this call, if any (read through the lease, so it
    /// reflects transfers and detachments performed by region close).
    pub(super) fn atomic_region(&self) -> Option<OplogIndex> {
        self.atomic_lease.as_ref().and_then(|lease| lease.owner())
    }

    /// Releases the call's atomic-region membership (idempotent, store-free).
    fn release_atomic_lease(&self) {
        if let Some(lease) = &self.atomic_lease {
            lease.release();
        }
    }

    /// The retry point to attach to a trap raised by this call: the call's *currently owning*
    /// atomic region (whole region retried from its begin index) if it still has one, otherwise
    /// its enclosing durable scope `Start` or its own `Start`. This mirrors
    /// [`ScopedRetryHost::retry_point`] so a hard (non-semantic) trap groups exactly like an
    /// inline/semantic retry would.
    fn trap_retry_point(&self) -> OplogIndex {
        self.atomic_region().unwrap_or(self.retry_from)
    }
}

/// Builds an *unregistered* atomic-region lease: it preserves the call's initiation-time region
/// for trap/retry classification (matching the immutable capture used before leases existed) but
/// is not a member of any region registry, so it never transfers or detaches on region close.
/// Used for replay and snapshotting handles, which do not participate in the live in-flight
/// member guard.
pub(super) fn unregistered_atomic_lease(
    atomic_region: Option<OplogIndex>,
    repairable_when_incomplete: bool,
) -> Option<Arc<AtomicRegionLease>> {
    atomic_region.map(|begin_index| {
        Arc::new(AtomicRegionLease::new(
            begin_index,
            repairable_when_incomplete,
        ))
    })
}

struct PreparedAccessStart<Pair: HostPayloadPair, P: DropPolicy, Ctx: WorkerCtx> {
    is_live: bool,
    snapshotting: bool,
    oplog: Arc<dyn Oplog>,
    public_state: PublicDurableWorkerState<Ctx>,
    replay_state: crate::durable_host::replay_state::ReplayState,
    execution_scope: BegunCallExecutionScope,
    retry: InFunctionRetryController,
    /// The registered atomic-region membership lease for a live persisted call initiated inside an
    /// open atomic region; `None` otherwise.
    atomic_lease: Option<Arc<AtomicRegionLease>>,
    drop_sink: Option<UnboundedSender<DropEvent>>,
    cleanup_sink: Option<UnboundedSender<DropEvent>>,
    claim_options: AccessClaimOptions,
    /// The worker's in-flight live host call counter, kept so `execute_access_start` can take a
    /// permit when a replayed scope switches to live mid-start.
    live_host_calls: Arc<AtomicUsize>,
    /// Taken at prepare time — synchronously with worker state, *before* any oplog entry of this
    /// call is appended — so an in-flight check (e.g. the `set_oplog_persistence_level` boundary
    /// guard) can never observe zero calls between this call's `Start` append and its handle
    /// construction in `finish_access_start`. `Some` for live calls, `None` on replay.
    live_call_permit: Option<LiveCallPermit>,
    _phantom: PhantomData<(Pair, P)>,
}

/// Options that make the durable records of a concurrent accessor call claim-safe on replay.
///
/// Accessor host calls run concurrently, so their oplog `Start` entries are appended in
/// scheduling order, which replay does not reproduce. Calls whose identity (function name +
/// durable function type) is shared by concurrent siblings need extra identity to be paired with
/// their own records instead of a sibling's:
///
/// - `scope_discriminator` is appended to the batched-write scope `Start`'s synthetic function
///   name (`<scope:batched-write:DISCRIMINATOR>`), so the replayed call claims exactly its own
///   scope. It must be a deterministic function of state that is identical on the live and replay
///   paths (e.g. a hash of the recorded request, or the `Start` index of an already-claimed
///   related call). Replay matches the discriminated name exactly — there is no plain-name
///   fallback (P3 deploys on a clean database, so no oplog predates the discriminators).
/// - `request_identity` is the [`HostRequest`] value the live path persists in the call's
///   `Start` entry; when set, the replay claim also requires the recorded request payload to
///   match it (by value, never by serialized bytes — payloads can contain `HashMap`s whose byte
///   order is process-random), disambiguating concurrent top-level calls that differ only in
///   their request.
#[derive(Default)]
pub(crate) struct AccessClaimOptions {
    pub(crate) scope_discriminator: Option<String>,
    pub(crate) request_identity: Option<HostRequest>,
}

struct ExecutedAccessStart<Pair: HostPayloadPair, P: DropPolicy> {
    begin_index: OplogIndex,
    start_idx: OplogIndex,
    persisted: bool,
    request_upload: PendingUpload,
    replay: Option<ReplayCallHandle>,
    execution_scope: CallExecutionScope,
    retry: InFunctionRetryController,
    opened_scope: Option<AccessOpenedScope>,
    drop_sink: Option<UnboundedSender<DropEvent>>,
    cleanup_sink: Option<UnboundedSender<DropEvent>>,
    /// The in-flight permit taken at prepare time (or when a replayed scope switched to live),
    /// handed on to the [`CallHandle`] so the call stays counted continuously from before its
    /// first oplog append until its terminal is recorded.
    live_call_permit: Option<LiveCallPermit>,
    _phantom: PhantomData<(Pair, P)>,
}

struct AccessOpenedScope {
    begin_index: OplogIndex,
    replay_handle: Option<ReplayCallHandle>,
    switched_to_live: bool,
}

struct AccessStartCleanup {
    /// The lease to release when the start failed before a handle was constructed. Release is
    /// idempotent and store-free.
    atomic_lease: Option<Arc<AtomicRegionLease>>,
}

/// Context handed to the request builder of [`CallHandle::start_access_with`], available after the
/// durable scope (if any) has been opened but before the host-call `Start` is written or claimed.
pub struct AccessStartContext {
    /// The begin index of the call, mirroring `begin_durable_function`: the durable-scope `Start`
    /// index when the call opens a scope, otherwise the pre-call oplog index. Stable across
    /// live/replay for scope-opening calls; approximately stable otherwise (see
    /// [`CallHandle::start_access_with`]).
    pub begin_index: OplogIndex,
    /// Whether the call executes live (the built request will be persisted) or replays (the built
    /// request is discarded; the builder still runs for its positional replay side effects).
    pub is_live: bool,
}

/// Releases a just-registered atomic-region lease when the accessor start path is torn (the
/// future dropped between registration and handle construction). Release is store-free, so the
/// `Drop` impl performs it directly.
struct AccessStartAtomicGuard {
    atomic_lease: Option<Arc<AtomicRegionLease>>,
}

impl AccessStartAtomicGuard {
    fn new(atomic_lease: Option<Arc<AtomicRegionLease>>) -> Self {
        Self { atomic_lease }
    }

    fn disarm(&mut self) {
        self.atomic_lease = None;
    }
}

impl Drop for AccessStartAtomicGuard {
    fn drop(&mut self) {
        if let Some(lease) = self.atomic_lease.take() {
            lease.release();
        }
    }
}

fn is_write_side_effect_for_access(function_type: &DurableFunctionType) -> bool {
    matches!(
        function_type,
        DurableFunctionType::WriteRemote
            | DurableFunctionType::WriteRemoteBatched(_)
            | DurableFunctionType::WriteRemoteTransaction(_)
    )
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
    fn in_atomic_region(&self) -> bool {
        self.execution_scope.atomic_region().is_some()
    }

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
        let mut state = self.inner.durable_execution_state();
        state.persistence_level = self.execution_scope.persistence_level;
        state
    }

    fn atomic_region_has_side_effects_for(&self, begin_index: OplogIndex) -> bool {
        self.inner.atomic_region_has_side_effects_for(begin_index)
    }

    fn retry_context_atomic_region_had_side_effects(&self) -> bool {
        // Membership-precise: classify against the region *currently owning* this call (via its
        // lease — the initiation region, a parent it was transferred to on nested close, or none
        // once detached). A call started outside any atomic region never writes
        // `inside_atomic_region = true`, even if a sibling later opened one.
        self.execution_scope
            .atomic_region()
            .is_some_and(|begin_index| self.inner.atomic_region_has_side_effects_for(begin_index))
    }

    async fn append_retry_error_entry(
        &mut self,
        retry_from: OplogIndex,
        inside_atomic_region: bool,
        retry_policy_state: Option<golem_common::model::RetryPolicyState>,
    ) {
        self.inner
            .append_retry_error_entry(retry_from, inside_atomic_region, retry_policy_state)
            .await;
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

    async fn persist_durable_function_invocation(
        &self,
        function_name: HostFunctionName,
        request: &HostRequest,
        response: &HostResponse,
        function_type: DurableFunctionType,
    ) {
        self.inner
            .persist_durable_function_invocation(function_name, request, response, function_type)
            .await;
    }

    async fn read_persisted_durable_function_invocation(
        &mut self,
    ) -> Result<
        crate::durable_host::durability::PersistedDurableFunctionInvocation,
        WorkerExecutorError,
    > {
        self.inner
            .read_persisted_durable_function_invocation()
            .await
    }

    async fn try_trigger_retry(
        &mut self,
        failure: Error,
        properties: RetryProperties,
    ) -> anyhow::Result<()> {
        try_trigger_host_trap_retry(self, failure, properties).await
    }

    fn mark_atomic_region_side_effect(&mut self) {
        self.inner.mark_atomic_region_side_effect();
    }

    fn create_interrupt_signal(&self) -> Pin<Box<dyn Future<Output = InterruptKind> + Send>> {
        self.inner.create_interrupt_signal()
    }

    fn check_read_only_allows(&self, host_function: &str) -> Result<(), GolemSpecificWasmTrap> {
        self.inner.check_read_only_allows(host_function)
    }
}

impl<Pair: HostPayloadPair, P: DropPolicy> CallHandle<Pair, P> {
    /// Begins a durable call.
    ///
    /// Observes the function call, then runs `begin_durable_function` — which applies the read-only
    /// side-effect guard, drains pending replay events, and (for a non-idempotent `WriteRemote` /
    /// `WriteRemoteBatched(None)`) opens the durable scope and runs the replay-side "operation was
    /// not completed" recovery. Then, (live) upload the request and append the eager host-call
    /// `Start`, or (replay) claim the next host-call `Start` and register a resolver receiver for it.
    ///
    /// Reusing `begin_durable_function`/`end_durable_function` (rather than re-deriving scope logic
    /// here) keeps the scope semantics consistent by construction: the same scope `Start`/`End`,
    /// the same `parent_start_index` nesting, the same commit/checkpoint boundaries.
    pub async fn start<Ctx: WorkerCtx>(
        ctx: &mut DurableWorkerCtx<Ctx>,
        request: Pair::Req,
        function_type: DurableFunctionType,
    ) -> Result<Self, WorkerExecutorError> {
        let begun = Self::begin(ctx, function_type).await?;
        if begun.is_live() {
            begun.start_live(ctx, request).await
        } else {
            begun.start_replay(ctx).await
        }
    }

    /// Accessor-window entry point for p3 host functions. The method uses short synchronous store
    /// windows only for worker-state snapshots and in-memory bookkeeping; oplog/replay awaits run on
    /// owned handles outside those windows.
    ///
    /// The accessor path supports read calls and remote writes. Scope-opening writes open their
    /// durable scope through owned oplog/replay handles and only re-enter the store for in-memory
    /// bookkeeping.
    pub async fn start_access<T, D, Ctx>(
        store: &Accessor<T, D>,
        get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
        request: Pair::Req,
        function_type: DurableFunctionType,
    ) -> Result<Self, WorkerExecutorError>
    where
        T: 'static,
        D: HasData + ?Sized,
        Ctx: WorkerCtx,
    {
        Self::start_access_with(store, get_ctx, function_type, async move |_| Ok(request)).await
    }

    /// Like [`Self::start_access`], but the request payload is built by `build_request` *between*
    /// the durable-scope open and the host-call `Start` write/claim. This is the accessor
    /// counterpart of the two-step [`Self::begin`] + [`BegunCall::start_live`] flow: it exists for
    /// calls whose persisted request depends on the begin index (e.g. a derived idempotency key) or
    /// that must interleave other positional oplog entries (e.g. a `StartSpan`) between the scope
    /// `Start` and the host-call `Start`.
    ///
    /// The builder runs on **both** the live and the replay path (so positional side entries it
    /// replays, like `StartSpan`, are consumed in the same order they were written), but its
    /// returned request is only persisted on the live path. It receives an [`AccessStartContext`]
    /// with the begin index — the durable-scope `Start` index when the call opens a scope, or the
    /// pre-call oplog index otherwise, mirroring `begin_durable_function` — and the liveness flag.
    /// For non-scope-opening calls the pre-call index is not perfectly stable between a live run
    /// and an incomplete-replay re-execution under concurrent siblings; callers deriving identity
    /// from it accept the same tradeoff as the RPC `async-invoke-and-await` path.
    ///
    /// A builder error aborts the call like any other start failure: the atomic-region
    /// registration is cleaned up and any already-written durable-scope `Start` is left incomplete,
    /// to be recovered by scope recovery on the next replay.
    pub async fn start_access_with<T, D, Ctx, F>(
        store: &Accessor<T, D>,
        get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
        function_type: DurableFunctionType,
        build_request: F,
    ) -> Result<Self, WorkerExecutorError>
    where
        T: 'static,
        D: HasData + ?Sized,
        Ctx: WorkerCtx,
        F: AsyncFnOnce(AccessStartContext) -> Result<Pair::Req, WorkerExecutorError>,
    {
        Self::start_access_with_options(
            store,
            get_ctx,
            function_type,
            AccessClaimOptions::default(),
            build_request,
        )
        .await
    }

    /// [`Self::start_access_with`] with explicit [`AccessClaimOptions`], for calls whose durable
    /// records need extra identity to be claim-safe under concurrent siblings on replay.
    pub(crate) async fn start_access_with_options<T, D, Ctx, F>(
        store: &Accessor<T, D>,
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
        if !is_accessor_supported_function_type(&function_type) {
            return Err(WorkerExecutorError::runtime(format!(
                "p3 accessor durable call path currently supports only ReadLocal/ReadRemote/WriteRemote/WriteRemoteBatched, got {function_type:?}"
            )));
        }
        let prepared = store.with(|mut access| {
            let ctx = get_ctx(access.data_mut());
            Self::prepare_access_start(ctx, function_type, claim_options)
        })?;
        let mut start_guard = AccessStartAtomicGuard::new(prepared.atomic_lease.clone());

        if let Err(err) = drain_dropped_call_events_access(store, get_ctx).await {
            return Err(err.source);
        }

        match Self::execute_access_start(prepared, build_request).await {
            Ok(executed) => {
                process_pending_replay_events_access(store, get_ctx).await?;
                let result = store.with(|mut access| {
                    let ctx = get_ctx(access.data_mut());
                    Self::finish_access_start(ctx, executed)
                });
                if result.is_ok() {
                    start_guard.disarm();
                }
                let mut handle = result?;
                handle
                    .supersede_prior_completion_delivery(store, get_ctx)
                    .await?;
                Ok(handle)
            }
            Err((err, cleanup)) => {
                store.with(|mut access| {
                    let ctx = get_ctx(access.data_mut());
                    Self::cleanup_access_start(ctx, cleanup);
                });
                start_guard.disarm();
                Err(err)
            }
        }
    }

    /// Supersedes any completion-delivery observer still armed on this accessor's host subtask
    /// once this call owns a persisted live `Start`.
    ///
    /// Such an observer belongs to an *earlier* durable call in the same host function whose
    /// response has already been consumed internally by host code (see
    /// [`CompletionDelivery::deliver_at_accessor_terminal`]): the guest can no longer discard
    /// that completion individually — only the whole host call's terminal — and replay
    /// re-executes the host code past its `End` deterministically, re-consuming the response
    /// internally. So no `CompletionDiscarded` marker is needed, and the observer's
    /// [`LiveCallPermit`] must not keep blocking suspension (e.g. for the whole duration of a
    /// subsequent suspendable wait such as `monotonic-clock.wait-for`). Clearing the observer
    /// drops its guard, which suppresses the token and releases the permit; the follow-up drain
    /// joins any ordered append the superseded token parked in the drain queue, so its oplog
    /// ordering and settlement accounting survive.
    ///
    /// This handoff is only performed once *this* call is a persisted live barrier (never for
    /// replay or snapshotting handles): until then the prior observer must stay armed so a guest
    /// cancellation landing before this call's `Start` still records its `CompletionDiscarded`
    /// marker and parks replay at the prior call.
    ///
    /// Invariant required of host functions: after this supersession, a failure of this call
    /// must either arm a newer observer (via its own completion) or escape as a
    /// trap/cancellation — host code must not swallow the failure and return a successful outer
    /// result derived from the superseded completion with no observer armed.
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
        if let Err(error) = store.clear_terminal_observer() {
            self.abandon_for_trap();
            return Err(WorkerExecutorError::runtime(format!(
                "failed to supersede prior accessor completion observer: {error}"
            )));
        }
        if let Err(err) = drain_dropped_call_events_access(store, get_ctx).await {
            self.abandon_for_trap();
            return Err(err.source);
        }
        Ok(())
    }

    fn prepare_access_start<Ctx: WorkerCtx>(
        ctx: &mut DurableWorkerCtx<Ctx>,
        function_type: DurableFunctionType,
        claim_options: AccessClaimOptions,
    ) -> Result<PreparedAccessStart<Pair, P, Ctx>, WorkerExecutorError> {
        DurabilityHost::observe_function_call(ctx, Pair::INTERFACE, Pair::FUNCTION);
        if !is_accessor_supported_function_type(&function_type) {
            return Err(WorkerExecutorError::runtime(format!(
                "p3 accessor durable call path currently supports only ReadLocal/ReadRemote/WriteRemote/WriteRemoteBatched, got {function_type:?}"
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

        let durable_execution_state = InFunctionRetryHost::durable_execution_state(ctx);
        let atomic_region = ctx
            .state
            .active_atomic_regions
            .last()
            .map(|region| region.begin_index);

        let parent_start_index = ctx
            .state
            .child_parent_start_index(&function_type, OplogIndex::INITIAL);
        let execution_scope = BegunCallExecutionScope {
            parent_start_index,
            atomic_region,
            persistence_level: ctx.state.persistence_level,
        };
        let is_live = durable_execution_state.is_live;
        let snapshotting = durable_execution_state.snapshotting_mode.is_some();
        let retry =
            InFunctionRetryController::new(function_type, durable_execution_state, Pair::FQFN);
        // A live persisted call initiated inside an open atomic region joins the region's member
        // registry: its lease starts owned by that region and follows the region's close
        // transitions (transfer to the enclosing region, or detachment at the outermost close).
        let atomic_lease = if is_live && !snapshotting {
            match atomic_region {
                Some(begin_index) => Some(
                    ctx.state
                        .register_atomic_region_call(
                            begin_index,
                            retry.can_reexecute_on_incomplete_replay(),
                        )
                        .ok_or_else(|| {
                            WorkerExecutorError::runtime(format!(
                                "durable call started in atomic region {begin_index}, but the region is not open"
                            ))
                        })?,
                ),
                None => None,
            }
        } else {
            None
        };
        let cleanup_sink = ctx.state.dropped_call_event_sender();
        let live_host_calls = ctx.state.live_host_call_counter();
        Ok(PreparedAccessStart {
            is_live,
            snapshotting,
            oplog: ctx.state.oplog.clone(),
            public_state: ctx.public_state.clone(),
            replay_state: ctx.state.replay_state.clone(),
            execution_scope,
            retry,
            atomic_lease,
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
    /// live call inside a persist-nothing zone still appends its `Start`/`End`, exactly like the
    /// legacy P2 path (`persist_durable_function_invocation`), because the PersistNothing contract
    /// is enforced elsewhere: `PrimaryOplog::commit` suppresses non-`Always` commits while the
    /// zone is open (so the call's own `DurableOnly` commits flush nothing), zone contents that do
    /// reach storage (via the zone-closing `Always` commit) are observability-only, and the replay
    /// cursor skips whole persist-nothing zones without claiming the entries inside them. The
    /// replay branch below guards against ever *replaying* a durable call inside a PersistNothing
    /// block, mirroring `read_persisted_durable_function_invocation`.
    async fn execute_access_start<Ctx: WorkerCtx, F>(
        mut prepared: PreparedAccessStart<Pair, P, Ctx>,
        build_request: F,
    ) -> Result<ExecutedAccessStart<Pair, P>, (WorkerExecutorError, AccessStartCleanup)>
    where
        F: AsyncFnOnce(AccessStartContext) -> Result<Pair::Req, WorkerExecutorError>,
    {
        let mut live_call_permit = prepared.live_call_permit.take();
        let starts_scope = opens_accessor_scope(
            prepared.retry.function_type(),
            prepared.retry.durable_execution_state().assume_idempotence,
            prepared.snapshotting,
        );
        let scope_start = if starts_scope {
            Some(Self::execute_access_scope_start(&prepared).await?)
        } else {
            None
        };

        let mut execution_scope = prepared.execution_scope;
        let mut retry = prepared.retry;
        let mut is_live = prepared.is_live;
        if let Some(scope_start) = &scope_start {
            execution_scope.parent_start_index = Some(scope_start.begin_index);
            if scope_start.switched_to_live {
                let previous = retry.durable_execution_state();
                retry = InFunctionRetryController::new(
                    retry.function_type().clone(),
                    DurableExecutionState {
                        is_live: true,
                        persistence_level: previous.persistence_level,
                        snapshotting_mode: previous.snapshotting_mode,
                        assume_idempotence: previous.assume_idempotence,
                        max_in_function_retry_delay: previous.max_in_function_retry_delay,
                    },
                    Pair::FQFN,
                );
                is_live = true;
                if live_call_permit.is_none() {
                    live_call_permit = Some(LiveCallPermit::new(prepared.live_host_calls.clone()));
                }
            }
        }

        // Build the request between the scope open and the host-call `Start` write/claim. The
        // builder's begin index mirrors `begin_durable_function`: the scope `Start` index when a
        // scope was opened, otherwise the pre-call index (last added / last replayed non-hint
        // entry). It runs on the replay path too so any positional side entries it wrote live are
        // consumed here in the same order.
        let builder_begin_index = match &scope_start {
            Some(scope) => scope.begin_index,
            None => {
                if is_live {
                    prepared.oplog.current_oplog_index().await
                } else {
                    prepared.replay_state.last_replayed_non_hint_index()
                }
            }
        };
        let request = build_request(AccessStartContext {
            begin_index: builder_begin_index,
            is_live,
        })
        .await
        .map_err(|err| {
            (
                err,
                AccessStartCleanup {
                    atomic_lease: prepared.atomic_lease.clone(),
                },
            )
        })?;

        if is_live {
            if prepared.snapshotting {
                let start_idx = prepared.oplog.current_oplog_index().await;
                let atomic_lease = unregistered_atomic_lease(
                    execution_scope.atomic_region,
                    retry.can_reexecute_on_incomplete_replay(),
                );
                Ok(ExecutedAccessStart {
                    begin_index: scope_start
                        .as_ref()
                        .map(|scope| scope.begin_index)
                        .unwrap_or(start_idx),
                    start_idx,
                    persisted: false,
                    request_upload: PendingUpload::already_durable(),
                    replay: None,
                    execution_scope: execution_scope.finish(start_idx, atomic_lease),
                    retry,
                    opened_scope: scope_start,
                    drop_sink: prepared.drop_sink,
                    cleanup_sink: prepared.cleanup_sink,
                    live_call_permit,
                    _phantom: PhantomData,
                })
            } else {
                let request: HostRequest = request.into();
                let function_type = retry.function_type().clone();
                let parent_start_index = execution_scope.parent_start_index;
                let (start_idx, request_upload) = prepared
                    .oplog
                    .add_start_with_reserved_payload(request, move |request_payload| {
                        OplogEntry::Start {
                            timestamp: Timestamp::now_utc(),
                            parent_start_index,
                            function_name: Pair::HOST_FUNCTION_NAME,
                            request: Some(request_payload),
                            durable_function_type: function_type,
                        }
                    })
                    .await
                    .map_err(|err| {
                        (
                            WorkerExecutorError::runtime(format!(
                                "failed to serialize and store durable call request: {err}"
                            )),
                            AccessStartCleanup {
                                atomic_lease: prepared.atomic_lease.clone(),
                            },
                        )
                    })?;
                // The registered lease from prepare time when this call started live inside an
                // open region; a replayed scope that switched to live mid-start falls back to an
                // unregistered lease preserving its initiation-time classification.
                let atomic_lease = prepared.atomic_lease.clone().or_else(|| {
                    unregistered_atomic_lease(
                        execution_scope.atomic_region,
                        retry.can_reexecute_on_incomplete_replay(),
                    )
                });
                Ok(ExecutedAccessStart {
                    begin_index: scope_start
                        .as_ref()
                        .map(|scope| scope.begin_index)
                        .unwrap_or(start_idx),
                    start_idx,
                    persisted: true,
                    request_upload,
                    replay: None,
                    execution_scope: execution_scope.finish(start_idx, atomic_lease),
                    retry,
                    opened_scope: scope_start,
                    drop_sink: prepared.drop_sink,
                    cleanup_sink: prepared.cleanup_sink,
                    live_call_permit,
                    _phantom: PhantomData,
                })
            }
        } else {
            if retry.durable_execution_state().persistence_level == PersistenceLevel::PersistNothing
            {
                return Err((
                    WorkerExecutorError::runtime(
                        "Trying to replay a durable invocation in a PersistNothing block",
                    ),
                    AccessStartCleanup {
                        atomic_lease: prepared.atomic_lease.clone(),
                    },
                ));
            }
            // A call owned by another durable record (its own opened scope, or the parent
            // encoded in the function type) is claimed by identity: accessor host calls run
            // concurrently, so owned `Start`s (e.g. per-chunk children of overlapping
            // consume-body scopes, or call `Start`s under sibling batched-write scopes) are not
            // appended in deterministic guest-initiation order and cannot be claimed
            // positionally. When the caller supplied a request identity, the claim additionally
            // requires the recorded request payload to match, disambiguating concurrent calls
            // whose durable identity is otherwise shared (e.g. parallel P3 HTTP sends).
            let claim_result = match (
                execution_scope.parent_start_index,
                prepared.claim_options.request_identity.as_ref(),
            ) {
                (Some(parent_start_index), Some(expected_request)) => {
                    prepared
                        .replay_state
                        .claim_owned_concurrent_start_matching_request(
                            &Pair::HOST_FUNCTION_NAME,
                            retry.function_type(),
                            parent_start_index,
                            expected_request,
                        )
                        .await
                }
                (Some(parent_start_index), None) => {
                    prepared
                        .replay_state
                        .claim_owned_concurrent_start(
                            &Pair::HOST_FUNCTION_NAME,
                            retry.function_type(),
                            parent_start_index,
                        )
                        .await
                }
                (None, Some(expected_request)) => {
                    prepared
                        .replay_state
                        .claim_concurrent_start_matching_request(
                            &Pair::HOST_FUNCTION_NAME,
                            retry.function_type(),
                            expected_request,
                        )
                        .await
                }
                (None, None) => {
                    prepared
                        .replay_state
                        .claim_concurrent_start(&Pair::HOST_FUNCTION_NAME, retry.function_type())
                        .await
                }
            };
            let replay = claim_result.map_err(|err| {
                (
                    err,
                    AccessStartCleanup {
                        atomic_lease: prepared.atomic_lease.clone(),
                    },
                )
            })?;
            let start_idx = replay.start_idx();
            // Replay handles never participate in the live in-flight member guard, but keep
            // their initiation-time region for trap/retry classification.
            let atomic_lease = unregistered_atomic_lease(
                execution_scope.atomic_region,
                retry.can_reexecute_on_incomplete_replay(),
            );
            Ok(ExecutedAccessStart {
                begin_index: scope_start
                    .as_ref()
                    .map(|scope| scope.begin_index)
                    .unwrap_or(start_idx),
                start_idx,
                persisted: false,
                request_upload: PendingUpload::already_durable(),
                replay: Some(replay),
                execution_scope: execution_scope.finish(start_idx, atomic_lease),
                retry,
                opened_scope: scope_start,
                drop_sink: prepared.drop_sink,
                cleanup_sink: prepared.cleanup_sink,
                live_call_permit: None,
                _phantom: PhantomData,
            })
        }
    }

    async fn execute_access_scope_start<Ctx: WorkerCtx>(
        prepared: &PreparedAccessStart<Pair, P, Ctx>,
    ) -> Result<AccessOpenedScope, (WorkerExecutorError, AccessStartCleanup)> {
        let function_type = prepared.retry.function_type().clone();
        // A caller-supplied discriminator makes the synthetic scope name unique among concurrent
        // siblings, so the replay claim below pairs the call with exactly its own recorded scope
        // (and with it the correct incomplete-scope detection). Without one, concurrent scopes of
        // the same durable function type are interchangeable at claim time.
        let scope_name = match &prepared.claim_options.scope_discriminator {
            Some(discriminator) => {
                HostFunctionName::Custom(format!("<scope:batched-write:{discriminator}>"))
            }
            None => HostFunctionName::Custom("<scope:batched-write>".to_string()),
        };
        if prepared.is_live {
            let entry = OplogEntry::Start {
                timestamp: Timestamp::now_utc(),
                parent_start_index: None,
                function_name: scope_name,
                request: None,
                durable_function_type: function_type,
            };
            let begin_index = prepared
                .public_state
                .worker()
                .add_and_commit_oplog(entry)
                .await;
            Ok(AccessOpenedScope {
                begin_index,
                replay_handle: None,
                switched_to_live: false,
            })
        } else {
            // Replay requires exactly the name the live path recorded (including the
            // discriminator suffix). There is no plain-name fallback: a discriminated claim must
            // never steal a plain sibling scope, and P3 deploys on a clean database so every
            // replayed oplog was recorded with the discriminators already in place.
            let (begin_index, replay_handle) = prepared
                .replay_state
                .claim_scope_start(&scope_name, &function_type)
                .await
                .map_err(|err| {
                    (
                        err,
                        AccessStartCleanup {
                            atomic_lease: prepared.atomic_lease.clone(),
                        },
                    )
                })?;

            if function_type == DurableFunctionType::WriteRemote
                && !prepared.retry.durable_execution_state().assume_idempotence
            {
                if prepared
                    .replay_state
                    .lookup_oplog_entry(begin_index, OplogEntry::is_end_remote_write)
                    .await
                    .is_none()
                {
                    prepared.replay_state.switch_to_live().await;
                    return Err((
                        WorkerExecutorError::runtime(
                            "Non-idempotent remote write operation was not completed, cannot retry",
                        ),
                        AccessStartCleanup {
                            atomic_lease: prepared.atomic_lease.clone(),
                        },
                    ));
                }

                Ok(AccessOpenedScope {
                    begin_index,
                    replay_handle: Some(replay_handle),
                    switched_to_live: false,
                })
            } else if matches!(function_type, DurableFunctionType::WriteRemoteBatched(None)) {
                let lookup_result = prepared
                    .replay_state
                    .lookup_oplog_entry_with_condition_and_state(
                        begin_index,
                        OplogEntry::is_end_remote_write_s::<ScopeScanState>,
                        OplogEntry::no_concurrent_side_effect,
                        ScopeScanState::new(
                            begin_index,
                            prepared.execution_scope.persistence_level,
                        ),
                        OplogEntry::track_scope_membership,
                    )
                    .await;

                match lookup_result {
                    OplogEntryLookupResult::Found { .. } => Ok(AccessOpenedScope {
                        begin_index,
                        replay_handle: Some(replay_handle),
                        switched_to_live: false,
                    }),
                    OplogEntryLookupResult::NotFound {
                        violates_for_all: false,
                    } if prepared.retry.durable_execution_state().assume_idempotence => {
                        prepared.replay_state.switch_to_live().await;
                        let deleted_region = OplogRegion {
                            start: begin_index.next(),
                            end: prepared.replay_state.replay_target().next(),
                        };
                        prepared
                            .public_state
                            .worker()
                            .add_and_commit_oplog(OplogEntry::jump(deleted_region))
                            .await;
                        prepared
                            .public_state
                            .worker()
                            .reattach_worker_status()
                            .await;
                        Ok(AccessOpenedScope {
                            begin_index,
                            replay_handle: None,
                            switched_to_live: true,
                        })
                    }
                    OplogEntryLookupResult::NotFound { .. } => {
                        prepared.replay_state.switch_to_live().await;
                        Err((
                            WorkerExecutorError::runtime(
                                "Non-idempotent remote write operation was not completed, cannot retry",
                            ),
                            AccessStartCleanup {
                                atomic_lease: prepared.atomic_lease.clone(),
                            },
                        ))
                    }
                }
            } else {
                Ok(AccessOpenedScope {
                    begin_index,
                    replay_handle: Some(replay_handle),
                    switched_to_live: false,
                })
            }
        }
    }

    fn finish_access_start<Ctx: WorkerCtx>(
        ctx: &mut DurableWorkerCtx<Ctx>,
        executed: ExecutedAccessStart<Pair, P>,
    ) -> Result<Self, WorkerExecutorError> {
        if let Some(scope) = executed.opened_scope {
            let kind = if matches!(
                executed.retry.function_type(),
                DurableFunctionType::WriteRemoteBatched(None)
            ) {
                DurableScopeKind::BatchedWrite
            } else {
                DurableScopeKind::NonIdempotentWrite
            };
            ctx.state
                .push_durable_scope(scope.begin_index, kind, scope.replay_handle);
            ctx.state.current_retry_point = scope.begin_index;
        }
        let is_live = executed.replay.is_none();
        Ok(CallHandle {
            start_idx: executed.start_idx,
            begin_index: executed.begin_index,
            is_live,
            persisted: executed.persisted,
            request_upload: executed.request_upload,
            replay: executed.replay,
            finished: false,
            parked_undelivered_replay: false,
            execution_scope: executed.execution_scope,
            retry: executed.retry,
            drop_sink: executed.drop_sink,
            cleanup_sink: executed.cleanup_sink,
            // Taken at prepare time (or at the replay→live switch), before this call's first
            // oplog append, so the in-flight count never dips to zero mid-call.
            live_call_permit: executed.live_call_permit,
            _phantom: PhantomData,
        })
    }

    fn cleanup_access_start<Ctx: WorkerCtx>(
        _ctx: &mut DurableWorkerCtx<Ctx>,
        cleanup: AccessStartCleanup,
    ) {
        if let Some(lease) = cleanup.atomic_lease {
            lease.release();
        }
    }

    /// First phase of a durable call: run `begin_durable_function` (read-only guard, pending replay
    /// events, durable-scope open + recovery) and capture the retry state, *without* yet writing
    /// the host-call `Start` or claiming it on replay.
    ///
    /// This is the explicit two-phase entry point for the rare "two-step" calls whose request
    /// payload depends on the durable-scope begin index (e.g. an RPC scheduled invocation embeds an
    /// idempotency key derived from it). Such calls cannot use [`Self::start`] because the request
    /// is not yet known when the scope is opened. The common case stays on [`Self::start`], which is
    /// just `begin` + `start_live`/`start_replay`.
    pub async fn begin<Ctx: WorkerCtx>(
        ctx: &mut DurableWorkerCtx<Ctx>,
        function_type: DurableFunctionType,
    ) -> Result<BegunCall<Pair, P>, WorkerExecutorError> {
        drain_queued_dropped_call_events(ctx)
            .await
            .map_err(|err| err.source)?;
        DurabilityHost::observe_function_call(ctx, Pair::INTERFACE, Pair::FUNCTION);

        // Read-only guard, pending replay events and durable-scope open all happen inside
        // `begin_durable_function`.
        let begin_index = ctx
            .begin_durable_function(&function_type, Pair::FQFN)
            .await?;
        let durable_execution_state = InFunctionRetryHost::durable_execution_state(ctx);
        let execution_scope = BegunCallExecutionScope {
            parent_start_index: ctx
                .state
                .child_parent_start_index(&function_type, begin_index),
            atomic_region: ctx
                .state
                .active_atomic_regions
                .last()
                .map(|region| region.begin_index),
            persistence_level: ctx.state.persistence_level,
        };
        let retry =
            InFunctionRetryController::new(function_type, durable_execution_state, Pair::FQFN);

        Ok(BegunCall {
            begin_index,
            execution_scope,
            retry,
            drop_sink: P::production_drop_sink(ctx.state.dropped_call_event_sender()),
            cleanup_sink: ctx.state.dropped_call_event_sender(),
            _phantom: PhantomData,
        })
    }

    pub fn is_live(&self) -> bool {
        self.is_live
    }

    pub fn start_index(&self) -> OplogIndex {
        self.start_idx
    }

    /// The index returned by `begin_durable_function`: the durable scope `Start` for a
    /// non-idempotent `WriteRemote` / `WriteRemoteBatched(None)`, or the pre-call index otherwise.
    /// Used by call sites that derive a stable identifier from that index (e.g. the idempotency-key
    /// derivation).
    pub fn begin_index(&self) -> OplogIndex {
        self.begin_index
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

    /// The call-owned trap classification for a hard error escaping this call: its own retry point
    /// and atomic-region membership, derived purely from the call's execution scope (the region it
    /// was *initiated* in), never from ambient worker state. Being pure, it needs no `ctx` and is
    /// stable across the call's body, so it can be built at the trap egress point or snapshotted
    /// earlier when the handle is moved before egress (see `io::poll`).
    pub(crate) fn trap_context(&self) -> DurableCallTrapContext {
        DurableCallTrapContext {
            retry_from: self.execution_scope.trap_retry_point(),
            in_atomic_region: self.execution_scope.atomic_region().is_some(),
        }
    }

    /// Abandons this call for a hard trap and wraps the escaping error with this call's
    /// [`DurableCallTrapContext`], so `TrapType::from_error` groups the failure against the call's
    /// own scope. Use this at every `TrapType::Error` egress after the call has started; see
    /// [`Self::abandon_for_trap`] for the non-error control-flow cases.
    pub fn trap(&mut self, err: impl Into<anyhow::Error>) -> anyhow::Error {
        let err = err.into();
        // Invariant lock: a marked host-call trap must never be a deterministic wasm trap. The
        // marker carries no call-owned atomic-region side-effect bit (`TrapType::from_error` sources
        // that from ambient state), which is only sound because the side-effect bit is consulted
        // solely for `AgentError::DeterministicTrap` and a host error is never one. Guest wasm traps
        // are classified on the invocation path without a `CallHandle`, so they never reach here.
        debug_assert!(
            err.root_cause().downcast_ref::<wasmtime::Trap>().is_none(),
            "CallHandle::trap must not wrap a deterministic wasm trap (root cause was a wasmtime::Trap)"
        );
        let context = self.trap_context();
        self.abandon_for_trap();
        mark_durable_call_trap_context(err, context)
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
        let outcome = {
            let mut retry_host = ScopedRetryHost::new(ctx, &self.execution_scope);
            self.retry
                .try_trigger_retry(&mut retry_host, result, classify)
                .await
        };
        outcome.map_err(|err| self.trap(err))
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
        let outcome = {
            let mut retry_host = ScopedRetryHost::new(ctx, &self.execution_scope);
            self.retry
                .try_trigger_retry_or_loop(&mut retry_host, result, classify)
                .await
        };
        outcome.map_err(|err| self.trap(err))
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
    /// retry state of this call's own retry point (its atomic region or enclosing durable scope
    /// `Start`, matching [`ScopedRetryHost::retry_point`]). `properties` are enriched with the
    /// worker-local context (`agent-type`, `is-idempotent`) before resolution.
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
            ctx.state.enrich_retry_properties(&mut properties);
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
            .get_non_detached_last_known_status()
            .await
            .current_retry_state
            .get(&retry_point)
            .cloned();

        let mut retry_host = TaskRetryContext {
            retry_point,
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
        };

        let failure = Error::new(ClassifiedHostError {
            kind: HostFailureKind::Transient,
            message,
        });
        try_trigger_host_trap_retry(&mut retry_host, failure, properties)
            .await
            .map_err(|err| self.trap(err))
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
        debug_assert!(
            self.retry.can_reexecute_on_incomplete_replay(),
            "CallHandle::run is only valid for re-executable calls (reads / idempotent writes); \
             use start/complete/replay explicitly for non-idempotent / batched / transaction writes"
        );
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
    pub async fn complete<Ctx: WorkerCtx>(
        mut self,
        ctx: &mut DurableWorkerCtx<Ctx>,
        response: Pair::Resp,
    ) -> Result<Pair::Resp, TerminalCallError> {
        let context = self.trap_context();
        if let Err(err) = drain_queued_dropped_call_events(ctx).await {
            self.abandon_for_trap();
            return Err(err);
        }
        self.complete_impl(ctx, response)
            .await
            .map_err(|source| TerminalCallError::new(source, context))
    }

    async fn complete_impl<Ctx: WorkerCtx>(
        mut self,
        ctx: &mut DurableWorkerCtx<Ctx>,
        response: Pair::Resp,
    ) -> Result<Pair::Resp, WorkerExecutorError> {
        debug_assert!(self.is_live, "complete() called on a replay handle");
        // This is the call's legitimate terminal; mark it finished up front so that a failure of the
        // downstream commit / scope close (`?` below) does not drop the handle "unfinished" and trip
        // the drop policy. The host-call `End` is what makes the call durable, not these follow-ups.
        self.finished = true;
        if self.persisted {
            let oplog = ctx.state.oplog.clone();
            let host_response: HostResponse = response.clone().into();
            let end = match prepare_end_entry(
                &oplog,
                &self.request_upload,
                self.start_idx,
                &host_response,
            )
            .await
            {
                Ok(end) => end,
                Err(err) => {
                    self.execution_scope.release_atomic_lease();
                    return Err(err);
                }
            };
            if let Some(begin_index) = self.execution_scope.atomic_region()
                && !ctx
                    .state
                    .mark_atomic_region_has_side_effects_for(begin_index)
            {
                self.execution_scope.release_atomic_lease();
                return Err(WorkerExecutorError::runtime(format!(
                    "durable call {} completed after its atomic region {begin_index} was closed",
                    self.start_idx
                )));
            }
            oplog.add(end).await;
            self.execution_scope.release_atomic_lease();
            // Close the durable scope (if one was opened), commit at the right boundary, and run the
            // mid-invocation checkpoint, all via `end_durable_function`.
            ctx.end_durable_function(self.retry.function_type(), self.begin_index, false)
                .await?;
        }
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
            .complete_access_impl(store, get_ctx, response, None)
            .await
            .map_err(|source| TerminalCallError::new(source, context))?;
        // Non-deferred boundary: no fallible guest-facing work follows the host return, so hand
        // the token to Wasmtime's terminal observer synchronously (no tear window exists between
        // the impl returning and this call) — the observer settles it when the guest actually
        // consumes (or discards) the lowered result.
        delivery.deliver_at_accessor_terminal(store);
        Ok(response)
    }

    /// Like [`Self::complete_access`], but for call sites whose response crosses one more
    /// fallible/cancellable boundary after the durable terminal (a second-stage channel send, a
    /// span finish before the host method returns, a wire conversion). Returns the response
    /// together with an armed [`CompletionDelivery`] token; the caller must consume the token at
    /// the real guest-facing boundary (`delivered` / `suppress` / `discarded`), and a torn future
    /// in between records the `CompletionDiscarded` marker via the token's drop.
    ///
    /// `post_end_entry` (e.g. the call's durable `FinishSpan`) is appended by the same owned task
    /// as the terminal `End`, so it is recorded even when this future is torn mid-completion:
    /// replay can then rely on the entry unconditionally following the `End` — the recorded
    /// discard marker (a hint entry) always sorts after both. Callers passing it must *not*
    /// append the entry themselves and only perform its in-memory effect.
    pub async fn complete_access_deferred<T, D, Ctx>(
        mut self,
        store: &Accessor<T, D>,
        get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
        response: Pair::Resp,
        post_end_entry: Option<OplogEntry>,
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
        self.complete_access_impl(store, get_ctx, response, post_end_entry)
            .await
            .map_err(|source| TerminalCallError::new(source, context))
    }

    async fn complete_access_impl<T, D, Ctx>(
        mut self,
        store: &Accessor<T, D>,
        get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
        response: Pair::Resp,
        post_end_entry: Option<OplogEntry>,
    ) -> Result<(Pair::Resp, CompletionDelivery), WorkerExecutorError>
    where
        T: 'static,
        D: HasData + ?Sized,
        Ctx: WorkerCtx,
    {
        self.ensure_accessor_terminal_supported("complete_access")?;
        if !self.is_live {
            return Err(WorkerExecutorError::runtime(
                "complete_access called on a replay handle; use replay_access first",
            ));
        }
        let function_type = self.retry.function_type().clone();
        let begin_index = self.begin_index;
        {
            let (oplog, replay_state) = store.with(|mut access| {
                let ctx = get_ctx(access.data_mut());
                (ctx.state.oplog.clone(), ctx.state.replay_state.clone())
            });
            let trap_context = self.trap_context();
            let mut guard = AccessTerminalGuard::<P>::new(
                DroppedCall {
                    start_idx: self.start_idx,
                    begin_index: self.begin_index,
                    function_type: self.retry.function_type().clone(),
                    request_upload: self.request_upload.clone(),
                    atomic_lease: self.execution_scope.atomic_lease.clone(),
                    trap_context,
                    live_call_permit: self.live_call_permit.clone(),
                },
                self.drop_sink.clone(),
                self.cleanup_sink.clone(),
            );
            self.finished = true;
            let persist_result: Result<(), WorkerExecutorError> = if self.persisted {
                Self::persist_access_terminal(
                    oplog,
                    replay_state,
                    &mut guard,
                    self.start_idx,
                    &response,
                    post_end_entry,
                )
                .await
            } else {
                Ok(())
            };

            // Read the current owner through the lease *after* the terminal is persisted, so a
            // region close that transferred this call mid-flight marks side effects against the
            // region that now owns it.
            let atomic_region = self.execution_scope.atomic_region();
            let finish_result = store.with(|mut access| {
                let ctx = get_ctx(access.data_mut());
                let mut result = Ok(());
                if let Some(begin_index) = atomic_region
                    && persist_result.is_ok()
                    && !ctx.state.mark_atomic_region_has_side_effects_for(begin_index)
                {
                        result = Err(WorkerExecutorError::runtime(format!(
                            "durable call {} completed after its atomic region {begin_index} was closed",
                            self.start_idx
                        )));
                }
                result
            });
            guard.release_atomic_lease();

            if let Err(err) = persist_result {
                guard.disarm();
                return Err(err);
            }
            // From here on the `End` is persisted, but these error returns are *observed* by the
            // caller (the worker traps): the completion was not silently discarded by the guest,
            // so the marker must not be recorded — while the terminal join / permit release must
            // still reach the drain through the guard's drop event.
            if let Err(err) = finish_result {
                guard.suppress_discard_marker();
                return Err(err);
            }
            if let Err(err) =
                end_durable_function_access(store, get_ctx, function_type, begin_index, false).await
            {
                guard.suppress_discard_marker();
                return Err(err);
            }
            Ok((response, guard.take_completion_delivery()))
        }
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
        replay_state: ReplayState,
        guard: &mut AccessTerminalGuard<P>,
        start_idx: OplogIndex,
        response: &Pair::Resp,
        post_end_entry: Option<OplogEntry>,
    ) -> Result<(), WorkerExecutorError> {
        let host_response: HostResponse = response.clone().into();
        let end = prepare_end_entry(
            &oplog,
            &guard
                .call()
                .expect("terminal guard is armed")
                .request_upload,
            start_idx,
            &host_response,
        )
        .await?;
        let terminal_oplog = oplog.clone();
        let terminal = tokio::spawn(async move {
            terminal_oplog.add(end).await;
            // A deferred-delivery call's mandatory post-`End` entry (e.g. its durable
            // `FinishSpan`) is appended by the same owned task: it is recorded even when the
            // completing future is torn right after the `End`, so replay can rely on it
            // unconditionally following the `End` (any discard marker chains after this task).
            if let Some(entry) = post_end_entry {
                terminal_oplog.add(entry).await;
            }
            Ok(())
        });
        // Arm the discard marker together with the owned `End` append: from this point a torn
        // completion future means the guest discarded a persisted successful completion (a tear
        // *during* `wait_terminal` still counts — the owned task appends the `End` regardless).
        guard.cleanup_after_terminal(
            terminal,
            Some(DiscardMarker {
                start_idx,
                oplog,
                replay_state,
            }),
        );
        guard.wait_terminal().await
    }

    /// Replays a call: drive the cursor until the call resolves, decode its response, then close the
    /// durable scope / commit via `end_durable_function`.
    ///
    /// If replay reaches the end of the oplog without ever seeing the matching `End`/`Cancelled` —
    /// a lone committed host-call `Start`, now possible for any write because `Start` is eager —
    /// the call is returned as [`CallReplayOutcome::Incomplete`] (for function types that are safe
    /// to re-execute) so the caller can re-run the side effect and `complete` the existing `Start`.
    /// For non-idempotent / batched / transaction writes re-execution is unsafe, so the same hard
    /// error as before is returned: those are protected by the surrounding durable-scope recovery
    /// run in [`Self::start`] (`begin_durable_function`), not by silent re-execution here.
    pub async fn replay<Ctx: WorkerCtx>(
        mut self,
        ctx: &mut DurableWorkerCtx<Ctx>,
    ) -> Result<CallReplayOutcome<Pair, P>, WorkerExecutorError> {
        let replay = self
            .replay
            .take()
            .expect("replay() called on a live handle");
        let outcome = ctx
            .state
            .replay_state
            .await_resolution_outcome(replay)
            .await?;
        match outcome {
            ResolutionOutcome::Resolved(Resolution::Completed { response, .. }) => {
                // Terminal: mark finished up front so a decode / scope-close failure below does not
                // drop the (replay) handle as "unfinished".
                self.finished = true;
                let oplog = ctx.state.oplog.clone();
                let response = decode_completed_response::<Pair>(&oplog, response).await?;
                ctx.end_durable_function(self.retry.function_type(), self.begin_index, false)
                    .await?;
                Ok(CallReplayOutcome::Replayed(response))
            }
            ResolutionOutcome::Resolved(Resolution::Cancelled {
                cancelled_idx,
                partial,
            }) => {
                self.finished = true;
                if let Some(payload) = partial {
                    let oplog = ctx.state.oplog.clone();
                    let response = download_and_decode_response::<Pair>(
                        &oplog,
                        payload,
                        "Cancelled partial payload",
                    )
                    .await?;
                    ctx.end_durable_function(self.retry.function_type(), self.begin_index, false)
                        .await?;
                    Ok(CallReplayOutcome::Replayed(response))
                } else {
                    Err(WorkerExecutorError::unexpected_oplog_entry(
                        "End or Cancelled { partial: Some(..) }",
                        format!("Cancelled without partial at {cancelled_idx}"),
                    ))
                }
            }
            ResolutionOutcome::Resolved(Resolution::CompletedButDiscarded {
                end_idx,
                marker_idx,
                ..
            }) => {
                // Discarded completions are recorded only on the accessor completion path, and
                // calls replay through the same path they recorded on — a marker resolving on
                // this non-accessor path means the oplog does not match this code path.
                self.finished = true;
                Err(WorkerExecutorError::unexpected_oplog_entry(
                    "End delivered to the guest",
                    format!(
                        "End at {end_idx} marked CompletionDiscarded at {marker_idx} for a non-accessor durable call"
                    ),
                ))
            }
            ResolutionOutcome::Incomplete => {
                self.prepare_incomplete_live_repair(
                    Ctx::ALLOW_LIVE_REPAIR_OF_INCOMPLETE_DURABLE_CALLS,
                    || ctx.state.live_host_call_counter(),
                )?;
                Ok(CallReplayOutcome::Incomplete(self))
            }
        }
    }

    /// Accessor-window replay for non-scope-opening p3 durable calls.
    pub async fn replay_access<T, D, Ctx>(
        mut self,
        store: &Accessor<T, D>,
        get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
    ) -> Result<CallReplayOutcome<Pair, P>, WorkerExecutorError>
    where
        T: 'static,
        D: HasData + ?Sized,
        Ctx: WorkerCtx,
    {
        self.ensure_accessor_terminal_supported("replay_access")?;
        let function_type = self.retry.function_type().clone();
        let begin_index = self.begin_index;
        let (replay_state, oplog) = store.with(|mut access| {
            let ctx = get_ctx(access.data_mut());
            (ctx.state.replay_state.clone(), ctx.state.oplog.clone())
        });
        let replay = self
            .replay
            .take()
            .expect("replay_access() called on a live handle");
        let outcome = replay_state.await_resolution_outcome(replay).await?;
        match outcome {
            ResolutionOutcome::Resolved(Resolution::Completed { response, .. }) => {
                self.finished = true;
                let response = decode_completed_response::<Pair>(&oplog, response).await?;
                end_durable_function_access(store, get_ctx, function_type, begin_index, false)
                    .await?;
                Ok(CallReplayOutcome::Replayed(response))
            }
            ResolutionOutcome::Resolved(Resolution::Cancelled {
                cancelled_idx,
                partial,
            }) => {
                if let Some(payload) = partial {
                    self.finished = true;
                    let response = download_and_decode_response::<Pair>(
                        &oplog,
                        payload,
                        "Cancelled partial payload",
                    )
                    .await?;
                    end_durable_function_access(store, get_ctx, function_type, begin_index, false)
                        .await?;
                    Ok(CallReplayOutcome::Replayed(response))
                } else {
                    // `Cancelled` with no partial result: in the recorded run this call never
                    // returned a value to the guest — its future was dropped mid-flight (e.g. the
                    // loser of a guest `race`/`select!`). Mirror that exactly: never complete, so
                    // the deterministic guest drops this future at the same point it did live.
                    // Resolving to an error here instead would race the guest's own drop — the
                    // resolver delivers the recorded terminal as soon as the cursor crosses it,
                    // which may be before the winning branch has been polled and had a chance to
                    // drop the loser.
                    //
                    // Durable cleanup (closing the durable scope for scope-opening calls) is
                    // deferred to this handle's `Drop`, which enqueues the idempotent
                    // `CloseDurableScope` event. Awaiting `end_durable_function_access` *here*,
                    // before the park, would open a cancellation window: for scope-opening calls
                    // it takes the scope's replay handle before its first await, so the guest
                    // dropping this future mid-close would strand the open scope (with `finished`
                    // already set, `Drop` would not clean it up either).
                    tracing::debug!(
                        "durable call cancelled without partial at {cancelled_idx} during replay; \
                         parking until the guest drops it"
                    );
                    self.parked_undelivered_replay = true;
                    std::future::pending::<()>().await;
                    unreachable!("std::future::pending never completes")
                }
            }
            ResolutionOutcome::Resolved(Resolution::CompletedButDiscarded {
                end_idx,
                marker_idx,
                ..
            }) => {
                // The recorded run persisted a successful `End`, but the guest dropped the
                // completion future before the response was delivered (the `CompletionDiscarded`
                // marker at `marker_idx` records this). Mirror live exactly, like the
                // cancelled-without-partial park above: never complete, so the deterministic
                // guest drops this future at the same point it did live; durable cleanup is
                // deferred to this handle's `Drop`.
                tracing::debug!(
                    "durable call completed at {end_idx} but its completion was discarded by the \
                     guest (marker at {marker_idx}); parking until the guest drops it"
                );
                self.parked_undelivered_replay = true;
                std::future::pending::<()>().await;
                unreachable!("std::future::pending never completes")
            }
            ResolutionOutcome::Incomplete => {
                self.prepare_incomplete_live_repair(
                    Ctx::ALLOW_LIVE_REPAIR_OF_INCOMPLETE_DURABLE_CALLS,
                    || {
                        store.with(|mut access| {
                            get_ctx(access.data_mut()).state.live_host_call_counter()
                        })
                    },
                )?;
                Ok(CallReplayOutcome::Incomplete(self))
            }
        }
    }

    /// Like [`Self::replay_access`], but for deferred-delivery call sites (see
    /// [`Self::complete_access_deferred`]): a replayed terminal is returned together with a
    /// [`CompletionDelivery`] token instead of parking here when the recorded run discarded the
    /// completion. For a recorded `CompletionDiscarded` marker this decodes the persisted
    /// response and closes the durable scope — mirroring exactly what live did before its token
    /// was armed — and the *caller* parks at its own delivery boundary after performing its
    /// deterministic post-`End` continuation (e.g. consuming a positional `FinishSpan`).
    pub async fn replay_access_deferred<T, D, Ctx>(
        mut self,
        store: &Accessor<T, D>,
        get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
    ) -> Result<DeferredCallReplayOutcome<Pair, P>, WorkerExecutorError>
    where
        T: 'static,
        D: HasData + ?Sized,
        Ctx: WorkerCtx,
    {
        self.ensure_accessor_terminal_supported("replay_access_deferred")?;
        let function_type = self.retry.function_type().clone();
        let begin_index = self.begin_index;
        let (replay_state, oplog) = store.with(|mut access| {
            let ctx = get_ctx(access.data_mut());
            (ctx.state.replay_state.clone(), ctx.state.oplog.clone())
        });
        let replay = self
            .replay
            .take()
            .expect("replay_access_deferred() called on a live handle");
        let outcome = replay_state.await_resolution_outcome(replay).await?;
        match outcome {
            ResolutionOutcome::Resolved(Resolution::Completed { response, .. }) => {
                self.finished = true;
                let response = decode_completed_response::<Pair>(&oplog, response).await?;
                end_durable_function_access(store, get_ctx, function_type, begin_index, false)
                    .await?;
                Ok(DeferredCallReplayOutcome::Replayed(
                    response,
                    CompletionDelivery::replay_delivered(),
                ))
            }
            ResolutionOutcome::Resolved(Resolution::CompletedButDiscarded {
                end_idx,
                marker_idx,
                response,
            }) => {
                // The recorded run persisted a successful `End` but the guest discarded the
                // completion before final delivery (marker at `marker_idx`). Decode the persisted
                // response and close the durable scope — the exact state live was in when its
                // delivery token was armed — and let the caller run its deterministic post-`End`
                // continuation before parking at the delivery boundary.
                tracing::debug!(
                    "durable call completed at {end_idx} but its completion was discarded by the \
                     guest (marker at {marker_idx}); replaying its post-End continuation before \
                     parking at the delivery boundary"
                );
                self.finished = true;
                let response = decode_completed_response::<Pair>(&oplog, response).await?;
                end_durable_function_access(store, get_ctx, function_type, begin_index, false)
                    .await?;
                Ok(DeferredCallReplayOutcome::Replayed(
                    response,
                    CompletionDelivery::replay_discarded(),
                ))
            }
            ResolutionOutcome::Resolved(Resolution::Cancelled {
                cancelled_idx,
                partial,
            }) => {
                if let Some(payload) = partial {
                    self.finished = true;
                    let response = download_and_decode_response::<Pair>(
                        &oplog,
                        payload,
                        "Cancelled partial payload",
                    )
                    .await?;
                    end_durable_function_access(store, get_ctx, function_type, begin_index, false)
                        .await?;
                    Ok(DeferredCallReplayOutcome::Replayed(
                        response,
                        CompletionDelivery::replay_delivered(),
                    ))
                } else {
                    // Cancelled with no partial: in the recorded run this call never returned a
                    // value to the guest. Mirror exactly — park here so the deterministic guest
                    // drops this future at the same point it did live; durable cleanup is
                    // deferred to this handle's `Drop`. See [`Self::replay_access`] for why the
                    // scope close must not happen before the park.
                    tracing::debug!(
                        "durable call cancelled without partial at {cancelled_idx} during replay; \
                         parking until the guest drops it"
                    );
                    self.parked_undelivered_replay = true;
                    std::future::pending::<()>().await;
                    unreachable!("std::future::pending never completes")
                }
            }
            ResolutionOutcome::Incomplete => {
                self.prepare_incomplete_live_repair(
                    Ctx::ALLOW_LIVE_REPAIR_OF_INCOMPLETE_DURABLE_CALLS,
                    || {
                        store.with(|mut access| {
                            get_ctx(access.data_mut()).state.live_host_call_counter()
                        })
                    },
                )?;
                Ok(DeferredCallReplayOutcome::Incomplete(self))
            }
        }
    }

    /// Replays a **non-re-executable** call (batched / transaction writes), where an incomplete
    /// `Start` cannot be safely re-run. For these function types [`Self::replay`] never yields
    /// [`CallReplayOutcome::Incomplete`] — it hard-errors first — so this optional combinator
    /// collapses the outcome to the replayed response, removing the dead `Incomplete` arm from the
    /// call sites. Re-executable sites must use [`Self::run`] or the explicit [`Self::replay`] match
    /// instead.
    pub async fn replay_expecting_completion<Ctx: WorkerCtx>(
        self,
        ctx: &mut DurableWorkerCtx<Ctx>,
    ) -> Result<Pair::Resp, WorkerExecutorError> {
        debug_assert!(
            !self.retry.can_reexecute_on_incomplete_replay(),
            "replay_expecting_completion is only valid for non-re-executable calls (batched / \
             transaction writes); re-executable calls must use run() or replay() explicitly"
        );
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
                let trap_context = self.trap_context();
                let dropped_call = DroppedCall {
                    start_idx: self.start_idx,
                    begin_index: self.begin_index,
                    function_type: self.retry.function_type().clone(),
                    request_upload: self.request_upload.clone(),
                    atomic_lease: self.execution_scope.atomic_lease.clone(),
                    trap_context,
                    live_call_permit: self.live_call_permit.clone(),
                };
                // As in `complete`: surface a deferred request-upload failure at the call site before
                // recording the `Cancelled` that references the request. A no-op when the request was
                // inline or eagerly uploaded.
                if let Err(err) = dropped_call.wait_request_upload().await {
                    dropped_call.release_atomic_lease();
                    return Err(err);
                }
                let partial_payload = match partial {
                    Some(partial) => {
                        let host_response: HostResponse = partial.into();
                        match oplog.upload_payload(&host_response).await.map_err(|err| {
                            WorkerExecutorError::runtime(format!(
                                "failed to serialize and store partial durable call response: {err}"
                            ))
                        }) {
                            Ok(payload) => Some(payload),
                            Err(err) => {
                                dropped_call.release_atomic_lease();
                                return Err(err);
                            }
                        }
                    }
                    None => None,
                };
                dropped_call
                    .append_cancelled_with_oplog(oplog, partial_payload)
                    .await?;
                dropped_call.release_atomic_lease();
                ctx.end_durable_function(self.retry.function_type(), self.begin_index, false)
                    .await?;
            }
        } else {
            let replay = self
                .replay
                .take()
                .expect("cancel() in replay called on a live handle");
            let resolution = ctx.state.replay_state.await_resolution(replay).await?;
            expect_cancelled_resolution(&resolution)?;
            ctx.end_durable_function(self.retry.function_type(), self.begin_index, false)
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
        let begin_index = self.begin_index;
        if self.is_live {
            if self.persisted {
                let oplog = store.with(|mut access| get_ctx(access.data_mut()).state.oplog.clone());
                let trap_context = self.trap_context();
                let mut guard = AccessTerminalGuard::<P>::new(
                    DroppedCall {
                        start_idx: self.start_idx,
                        begin_index: self.begin_index,
                        function_type: self.retry.function_type().clone(),
                        request_upload: self.request_upload.clone(),
                        atomic_lease: self.execution_scope.atomic_lease.clone(),
                        trap_context,
                        live_call_permit: self.live_call_permit.clone(),
                    },
                    self.drop_sink.clone(),
                    self.cleanup_sink.clone(),
                );
                let result = async {
                    guard.call().expect("terminal guard is armed").wait_request_upload().await?;
                    let partial_payload = match partial {
                        Some(partial) => {
                            let host_response: HostResponse = partial.into();
                            Some(oplog.upload_payload(&host_response).await.map_err(|err| {
                                WorkerExecutorError::runtime(format!(
                                    "failed to serialize and store partial durable call response: {err}"
                                ))
                            })?)
                        }
                        None => None,
                    };
                    let call = guard.call().expect("terminal guard is armed").clone();
                    let terminal = tokio::spawn(async move {
                        call.append_cancelled_with_oplog(oplog, partial_payload)
                            .await
                    });
                    // Cancellation terminal: never a discarded completion, no marker.
                    guard.cleanup_after_terminal(terminal, None);
                    guard.wait_terminal().await
                }
                .await;
                guard.release_atomic_lease();
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
                "{operation} currently supports only ReadLocal/ReadRemote/WriteRemoteBatched durable calls, got {:?}",
                self.retry.function_type()
            )))
        }
    }

    /// Handles a [`ResolutionOutcome::Incomplete`] resolution, shared by every replay variant.
    ///
    /// Either refuses — debug sessions (`allow_live_repair == false`) must never re-execute side
    /// effects, and non-re-executable function types (non-idempotent / batched / transaction
    /// writes) could duplicate an external side effect — or switches this handle to live
    /// completion of the existing, committed `Start`: the caller re-runs the side effect and
    /// `complete`s, appending the missing `End`. A failure during re-execution stays grouped at
    /// this call's own retry point via the call-owned `execution_scope` (and the semantic-trap
    /// error marker).
    ///
    /// On refusal the handle is marked finished (the outcome is terminal); on success it is live,
    /// persisted, and holds a fresh in-flight permit obtained from `get_counter` (a closure so
    /// accessor callers can bound their `store.with` window to the refusal-free path).
    fn prepare_incomplete_live_repair(
        &mut self,
        allow_live_repair: bool,
        get_counter: impl FnOnce() -> Arc<AtomicUsize>,
    ) -> Result<(), WorkerExecutorError> {
        if !allow_live_repair {
            self.finished = true;
            return Err(WorkerExecutorError::invalid_request(format!(
                "the replay target lies inside an in-flight durable call (Start at {} has no End/Cancelled before the replay target); live re-execution of incomplete durable calls is disabled in debug sessions",
                self.start_idx
            )));
        }
        if !self.retry.can_reexecute_on_incomplete_replay() {
            // Reaching here means the surrounding scope recovery did not already resolve the
            // incomplete call; fail hard, as before.
            self.finished = true;
            return Err(WorkerExecutorError::unexpected_oplog_entry(
                "End or Cancelled",
                format!(
                    "incomplete non-idempotent durable call Start at {} cannot be safely re-executed",
                    self.start_idx
                ),
            ));
        }
        self.is_live = true;
        self.persisted = true;
        self.live_call_permit = Some(LiveCallPermit::new(get_counter()));
        Ok(())
    }
}

/// Downloads a recorded response payload and decodes it into the call's typed response,
/// preserving the canonical error classification: a failed download is a runtime error, a type
/// mismatch is an unexpected-oplog-entry error against the call's fully qualified function name.
async fn download_and_decode_response<Pair: HostPayloadPair>(
    oplog: &Arc<dyn Oplog>,
    payload: OplogPayload<HostResponse>,
    payload_kind: &str,
) -> Result<Pair::Resp, WorkerExecutorError> {
    let host_response = oplog.download_payload(payload).await.map_err(|err| {
        WorkerExecutorError::runtime(format!("{payload_kind} cannot be downloaded: {err}"))
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
) -> Result<Pair::Resp, WorkerExecutorError> {
    let payload = response.ok_or_else(|| {
        WorkerExecutorError::unexpected_oplog_entry(
            "End { response: Some(..) }",
            "End { response: None }".to_string(),
        )
    })?;
    download_and_decode_response::<Pair>(oplog, payload, "End payload").await
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

/// Store-free persistence preparation shared by the direct ([`CallHandle::complete`]) and
/// accessor ([`CallHandle::persist_access_terminal`]) completion paths: waits for the (possibly
/// deferred) request upload, uploads the response payload, and constructs the terminal `End`
/// entry — without appending it, touching atomic-region state, or closing scopes, which the two
/// paths deliberately order differently (direct appends inline; accessor hands the append to an
/// owned task via its terminal guard).
async fn prepare_end_entry(
    oplog: &Arc<dyn Oplog>,
    request_upload: &PendingUpload,
    start_idx: OplogIndex,
    host_response: &HostResponse,
) -> Result<OplogEntry, WorkerExecutorError> {
    // Surface a deferred request-upload failure here, at the call site, before recording the
    // `End` that references the request. The leaf oplog's commit barrier is the backstop, but
    // awaiting here turns an upload failure into a graceful error instead of a commit-time
    // panic. A no-op when the request was inline or eagerly uploaded.
    request_upload.wait().await.map_err(|err| {
        WorkerExecutorError::runtime(format!(
            "failed to serialize and store durable call request: {err}"
        ))
    })?;
    let response_payload = oplog.upload_payload(host_response).await.map_err(|err| {
        WorkerExecutorError::runtime(format!(
            "failed to serialize and store durable call response: {err}"
        ))
    })?;
    Ok(OplogEntry::End {
        timestamp: Timestamp::now_utc(),
        start_index: start_idx,
        response: Some(response_payload),
        forced_commit: false,
    })
}

pub(crate) async fn end_durable_function_access<T, D, Ctx>(
    store: &Accessor<T, D>,
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
    let (opens_scope, is_live, replay_handle, replay_state, oplog, public_state) =
        store.with(|mut access| {
            let ctx = get_ctx(access.data_mut());
            let opens_scope = ctx.state.opens_durable_scope(&function_type);
            let is_live = ctx.state.is_live();
            let replay_handle = if opens_scope && !is_live {
                ctx.state.take_durable_scope_replay_handle(begin_index)
            } else {
                None
            };
            (
                opens_scope,
                is_live,
                replay_handle,
                ctx.state.replay_state.clone(),
                ctx.state.oplog.clone(),
                ctx.public_state.clone(),
            )
        });

    if opens_scope {
        if is_live {
            oplog
                .add(OplogEntry::End {
                    timestamp: Timestamp::now_utc(),
                    start_index: begin_index,
                    response: None,
                    forced_commit: true,
                })
                .await;
        } else if let Some(handle) = replay_handle {
            match replay_state.await_resolution_outcome(handle).await? {
                ResolutionOutcome::Resolved(Resolution::Completed { .. }) => {}
                ResolutionOutcome::Resolved(Resolution::Cancelled { .. }) => {
                    return Err(WorkerExecutorError::unexpected_oplog_entry(
                        format!("End {{ start_index: {begin_index} }}"),
                        format!("Cancelled {{ start_index: {begin_index} }}"),
                    ));
                }
                ResolutionOutcome::Resolved(Resolution::CompletedButDiscarded {
                    end_idx,
                    marker_idx,
                    ..
                }) => {
                    // Discarded completions are recorded only for accessor completion futures; a
                    // marker referencing a durable scope `Start` means the oplog does not match
                    // this code path.
                    return Err(WorkerExecutorError::unexpected_oplog_entry(
                        format!("End {{ start_index: {begin_index} }}"),
                        format!(
                            "End at {end_idx} marked CompletionDiscarded at {marker_idx} for a durable scope"
                        ),
                    ));
                }
                ResolutionOutcome::Incomplete => {
                    oplog
                        .add(OplogEntry::End {
                            timestamp: Timestamp::now_utc(),
                            start_index: begin_index,
                            response: None,
                            forced_commit: true,
                        })
                        .await;
                }
            }
        }

        store.with(|mut access| {
            let ctx = get_ctx(access.data_mut());
            ctx.state.remove_durable_scope(begin_index)
        })?;
    }

    if function_type == DurableFunctionType::WriteRemote
        || matches!(function_type, DurableFunctionType::WriteRemoteBatched(_))
        || matches!(
            function_type,
            DurableFunctionType::WriteRemoteTransaction(_)
        )
        || forced_commit
    {
        public_state
            .worker()
            .commit_oplog_and_update_state(CommitLevel::DurableOnly)
            .await;
        if let Some(min_exposed_marker) = store.with(|mut access| {
            let ctx = get_ctx(access.data_mut());
            if ctx.state.at_clean_checkpoint_boundary() {
                Some(ctx.state.min_exposed_marker)
            } else {
                None
            }
        }) {
            public_state
                .worker()
                .checkpoint_status_mid_invocation(min_exposed_marker)
                .await;
        }
    }

    Ok(())
}

pub(crate) async fn end_durable_function_access_if_open<T, D, Ctx>(
    store: &Accessor<T, D>,
    get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
    function_type: DurableFunctionType,
    begin_index: OplogIndex,
    forced_commit: bool,
) -> Result<bool, WorkerExecutorError>
where
    T: 'static,
    D: HasData + ?Sized,
    Ctx: WorkerCtx,
{
    let is_open = store.with(|mut access| {
        let ctx = get_ctx(access.data_mut());
        ctx.state.is_durable_scope_open(begin_index)
    });
    if is_open {
        end_durable_function_access(store, get_ctx, function_type, begin_index, forced_commit)
            .await?;
        Ok(true)
    } else {
        Ok(false)
    }
}

/// Finishes a span in the in-memory invocation context only, without writing or consuming any
/// oplog entry: pops the current-span pointer if it points at this span, then marks the span
/// finished. This is the non-durable half of [`finish_span_access`], used directly for spans
/// whose identity is derived from durable records (no `StartSpan`/`FinishSpan` entries exist).
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

pub(crate) async fn finish_span_access<T, D, Ctx>(
    store: &Accessor<T, D>,
    get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
    span_id: &SpanId,
) -> Result<(), WorkerExecutorError>
where
    T: 'static,
    D: HasData + ?Sized,
    Ctx: WorkerCtx,
{
    let (is_live, worker, replay_state) = store.with(|mut access| {
        let ctx = get_ctx(access.data_mut());
        (
            ctx.state.is_live(),
            ctx.public_state.worker(),
            ctx.state.replay_state.clone(),
        )
    });

    if is_live {
        worker
            .add_to_oplog(OplogEntry::finish_span(span_id.clone()))
            .await;
    } else {
        crate::get_oplog_entry_owned!(replay_state, OplogEntry::FinishSpan)?;
    }

    store.with(|mut access| {
        let ctx = get_ctx(access.data_mut());
        finish_span_in_memory(ctx, span_id)
    })
}

fn is_accessor_supported_function_type(function_type: &DurableFunctionType) -> bool {
    matches!(
        function_type,
        DurableFunctionType::ReadLocal
            | DurableFunctionType::ReadRemote
            | DurableFunctionType::WriteRemote
            | DurableFunctionType::WriteRemoteBatched(_)
    )
}

fn opens_accessor_scope(
    function_type: &DurableFunctionType,
    assume_idempotence: bool,
    snapshotting: bool,
) -> bool {
    !snapshotting
        && ((*function_type == DurableFunctionType::WriteRemote && !assume_idempotence)
            || matches!(function_type, DurableFunctionType::WriteRemoteBatched(None)))
}

fn opens_replay_durable_scope(
    function_type: &DurableFunctionType,
    assume_idempotence: bool,
) -> bool {
    (*function_type == DurableFunctionType::WriteRemote && !assume_idempotence)
        || matches!(
            *function_type,
            DurableFunctionType::WriteRemoteBatched(None)
        )
}

fn is_accessor_terminal_supported_function_type(function_type: &DurableFunctionType) -> bool {
    is_accessor_supported_function_type(function_type)
        || matches!(function_type, DurableFunctionType::WriteRemote)
}

async fn process_pending_replay_events_access<T, D, Ctx>(
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
    let replay_events = replay_state.take_new_replay_events();
    for event in replay_events {
        match event {
            crate::durable_host::replay_state::ReplayEvent::ForkReplayed { new_phantom_id } => {
                store.with(|mut access| {
                    let ctx = get_ctx(access.data_mut());
                    ctx.state.current_phantom_id = Some(new_phantom_id);
                });
            }
            crate::durable_host::replay_state::ReplayEvent::UpdateReplayed { new_revision } => {
                tracing::debug!(
                    "Updating worker state to component metadata revision {new_revision}"
                );
                update_state_to_new_component_revision_access(store, get_ctx, new_revision).await?;
            }
            crate::durable_host::replay_state::ReplayEvent::CardInstalled { card } => {
                store.with(|mut access| {
                    let ctx = get_ctx(access.data_mut());
                    let card_id = card.card_id();
                    tracing::debug!(card_id = %card_id, "Applying replayed card installation");
                    ctx.state.agent_wallet_cards.insert(card_id, card);
                    ctx.rederive_agent_effective_surface_from_wallet();
                });
            }
            crate::durable_host::replay_state::ReplayEvent::CardRevoked { card_id }
            | crate::durable_host::replay_state::ReplayEvent::CardExpired { card_id } => {
                store.with(|mut access| {
                    let ctx = get_ctx(access.data_mut());
                    tracing::debug!(card_id = %card_id, "Applying replayed card removal");
                    if ctx.state.agent_wallet_cards.remove(&card_id).is_some() {
                        ctx.rederive_agent_effective_surface_from_wallet();
                    }
                });
            }
            crate::durable_host::replay_state::ReplayEvent::ReplayFinished => {
                tracing::debug!("Replaying oplog finished");
                finalize_pending_automatic_update_access(store, get_ctx).await?;
            }
        }
    }
    Ok(())
}

struct AccessRevisionUpdateInputs {
    component_service: Arc<dyn ComponentService>,
    file_loader: Arc<FileLoader>,
    owned_agent_id: golem_common::model::OwnedAgentId,
    agent_id: Option<ParsedAgentId>,
    initial_agent_config: Vec<golem_common::model::worker::TypedAgentConfigEntry>,
    worker_dir: PathBuf,
    current_revision: ComponentRevision,
}

type AccessRevisionUpdateAgentState = (
    HashMap<Vec<String>, golem_common::schema::TypedSchemaValue>,
    golem_common::model::card::EffectiveSurface,
    BTreeMap<golem_common::model::card::CardId, golem_common::model::card::StoredCard>,
);

struct AccessRevisionUpdate {
    metadata: Component,
    agent_state: Option<AccessRevisionUpdateAgentState>,
    files: HashMap<PathBuf, IFSWorkerFile>,
}

async fn finalize_pending_automatic_update_access<T, D, Ctx>(
    store: &Accessor<T, D>,
    get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
) -> Result<(), WorkerExecutorError>
where
    T: 'static,
    D: HasData + ?Sized,
    Ctx: WorkerCtx,
{
    let pending_update = store.with(|mut access| {
        let ctx = get_ctx(access.data_mut());
        let pending_update = ctx
            .state
            .pending_update
            .try_lock()
            .map_err(|_| {
                WorkerExecutorError::runtime(
                    "p3 accessor durable call path cannot inspect pending component update state",
                )
            })?
            .take();
        Ok::<_, WorkerExecutorError>(pending_update)
    });

    let pending_update = if let Some(pending_update) = pending_update? {
        pending_update
    } else {
        return Ok(());
    };

    match pending_update.description {
        UpdateDescription::Automatic { target_revision } => {
            tracing::debug!("Finalizing pending automatic update");
            if let Err(error) =
                update_state_to_new_component_revision_access(store, get_ctx, target_revision).await
            {
                let stringified_error = format!("Applying worker update failed: {error}");
                record_worker_update_failed_access(
                    store,
                    get_ctx,
                    target_revision,
                    stringified_error,
                )
                .await?;
                return Err(error);
            }

            let (component_size, active_plugins) = store.with(|mut access| {
                let ctx = get_ctx(access.data_mut());
                (
                    ctx.state.component_metadata.component_size,
                    HashSet::from_iter({
                        ctx.agent_type_provision_config()
                            .map(|c| c.plugins.as_slice())
                            .unwrap_or_default()
                            .iter()
                            .map(|installation| installation.environment_plugin_grant_id)
                    }),
                )
            });
            record_worker_update_succeeded_access(
                store,
                get_ctx,
                target_revision,
                component_size,
                active_plugins,
            )
            .await?;
            tracing::debug!("Finalizing automatic update to revision {target_revision}");
            Ok(())
        }
        _ => Err(WorkerExecutorError::runtime(
            "pending replay event finalization expected an automatic update description",
        )),
    }
}

async fn update_state_to_new_component_revision_access<T, D, Ctx>(
    store: &Accessor<T, D>,
    get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
    new_revision: ComponentRevision,
) -> Result<(), WorkerExecutorError>
where
    T: 'static,
    D: HasData + ?Sized,
    Ctx: WorkerCtx,
{
    let inputs = store.with(|mut access| {
        let ctx = get_ctx(access.data_mut());
        AccessRevisionUpdateInputs {
            component_service: ctx.state.component_service.clone(),
            file_loader: ctx.state.file_loader.clone(),
            owned_agent_id: ctx.owned_agent_id.clone(),
            agent_id: ctx.state.agent_id.clone(),
            initial_agent_config: ctx.state.initial_agent_config.clone(),
            worker_dir: ctx.worker_dir.path().to_path_buf(),
            current_revision: ctx.state.component_metadata.revision,
        }
    });

    if new_revision <= inputs.current_revision {
        tracing::debug!("Update {new_revision} was already applied, skipping");
        return Ok(());
    }

    let update = prepare_revision_update_access(store, get_ctx, &inputs, new_revision).await?;
    store.with(|mut access| {
        let ctx = get_ctx(access.data_mut());
        apply_revision_update_access(ctx, update);
    });
    Ok(())
}

async fn prepare_revision_update_access<T, D, Ctx>(
    store: &Accessor<T, D>,
    get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
    inputs: &AccessRevisionUpdateInputs,
    new_revision: ComponentRevision,
) -> Result<AccessRevisionUpdate, WorkerExecutorError>
where
    T: 'static,
    D: HasData + ?Sized,
    Ctx: WorkerCtx,
{
    let metadata = inputs
        .component_service
        .get_metadata(inputs.owned_agent_id.component_id(), Some(new_revision))
        .await?;

    let provision_config = inputs.agent_id.as_ref().and_then(|agent_id| {
        metadata
            .metadata
            .agent_type_provision_configs()
            .get(&agent_id.agent_type)
            .cloned()
    });

    let agent_state = if let Some(agent_id) = &inputs.agent_id {
        let agent_type = metadata
            .metadata
            .find_agent_type_by_name_ref(&agent_id.agent_type)
            .ok_or_else(|| {
                WorkerExecutorError::invalid_request(format!(
                    "Agent type {} not found in updated agent metadata",
                    agent_id.agent_type
                ))
            })?;

        let updated_agent_config = effective_agent_config(
            inputs.initial_agent_config.clone(),
            provision_config
                .as_ref()
                .map(|c| c.config.clone())
                .unwrap_or_default(),
        )?;
        validate_agent_config(&updated_agent_config, agent_type)?;

        let initial_card =
            super::super::agent_initial_card_from_component_metadata(&metadata, agent_id)?;
        let initial_wallet_cards = BTreeMap::from([(initial_card.card_id(), initial_card)]);
        let context = super::super::agent_monomorphization_context(
            &metadata,
            &inputs.owned_agent_id,
            agent_id,
        );
        let effective_surface = golem_common::model::card::agent_effective_surface_from_wallet(
            &context,
            initial_wallet_cards.values(),
        );

        Some((
            updated_agent_config,
            effective_surface,
            initial_wallet_cards,
        ))
    } else {
        None
    };

    let mut files = take_initial_files_access(store, get_ctx)?;
    let update_result = super::super::update_filesystem(
        &mut files,
        &inputs.file_loader,
        inputs.owned_agent_id.environment_id,
        &inputs.worker_dir,
        provision_config
            .as_ref()
            .map(|c| c.files.as_slice())
            .unwrap_or_default(),
    )
    .await;

    if let Err(error) = update_result {
        restore_initial_files_access(store, get_ctx, files)?;
        return Err(error);
    }

    Ok(AccessRevisionUpdate {
        metadata,
        agent_state,
        files,
    })
}

fn take_initial_files_access<T, D, Ctx>(
    store: &Accessor<T, D>,
    get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
) -> Result<HashMap<PathBuf, IFSWorkerFile>, WorkerExecutorError>
where
    T: 'static,
    D: HasData + ?Sized,
    Ctx: WorkerCtx,
{
    store.with(|mut access| {
        let ctx = get_ctx(access.data_mut());
        let mut files = ctx.state.files.try_write().map_err(|_| {
            WorkerExecutorError::runtime(
                "p3 accessor durable call path cannot acquire initial-files lock",
            )
        })?;
        Ok(std::mem::take(&mut *files))
    })
}

fn restore_initial_files_access<T, D, Ctx>(
    store: &Accessor<T, D>,
    get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
    restored: HashMap<PathBuf, IFSWorkerFile>,
) -> Result<(), WorkerExecutorError>
where
    T: 'static,
    D: HasData + ?Sized,
    Ctx: WorkerCtx,
{
    store.with(|mut access| {
        let ctx = get_ctx(access.data_mut());
        let mut files = ctx.state.files.try_write().map_err(|_| {
            WorkerExecutorError::runtime(
                "p3 accessor durable call path cannot restore initial-files state",
            )
        })?;
        *files = restored;
        Ok(())
    })
}

fn apply_revision_update_access<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    update: AccessRevisionUpdate,
) {
    let read_only_paths = super::super::compute_read_only_paths(&update.files);
    {
        let mut files = ctx
            .state
            .files
            .try_write()
            .expect("initial-files state was taken by this update path");
        *files = update.files;
    }
    {
        let mut read_only = ctx.state.read_only_paths.write().unwrap();
        *read_only = read_only_paths;
    }

    if let Some((agent_config, effective_surface, initial_wallet_cards)) = update.agent_state {
        ctx.state.agent_config = agent_config;
        ctx.state.cached_agent_config_retry_policies = None;
        ctx.state.agent_effective_surface = effective_surface;
        ctx.state.agent_wallet_cards = initial_wallet_cards;
    }
    ctx.state.component_metadata = update.metadata;
}

async fn record_worker_update_failed_access<T, D, Ctx>(
    store: &Accessor<T, D>,
    get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
    target_revision: ComponentRevision,
    details: String,
) -> Result<(), WorkerExecutorError>
where
    T: 'static,
    D: HasData + ?Sized,
    Ctx: WorkerCtx,
{
    let public_state = store.with(|mut access| get_ctx(access.data_mut()).public_state.clone());
    public_state
        .worker()
        .add_and_commit_oplog(OplogEntry::failed_update(
            target_revision,
            Some(details.clone()),
        ))
        .await;
    tracing::warn!(
        "Worker failed to update to {}: {}, update attempt aborted",
        target_revision,
        details
    );
    Ok(())
}

async fn record_worker_update_succeeded_access<T, D, Ctx>(
    store: &Accessor<T, D>,
    get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
    target_revision: ComponentRevision,
    component_size: u64,
    active_plugins: HashSet<
        golem_common::base_model::environment_plugin_grant::EnvironmentPluginGrantId,
    >,
) -> Result<(), WorkerExecutorError>
where
    T: 'static,
    D: HasData + ?Sized,
    Ctx: WorkerCtx,
{
    tracing::info!("Worker update to {} finished successfully", target_revision);
    let public_state = store.with(|mut access| get_ctx(access.data_mut()).public_state.clone());
    public_state
        .worker()
        .add_and_commit_oplog(OplogEntry::successful_update(
            target_revision,
            component_size,
            active_plugins,
        ))
        .await;
    Ok(())
}

/// The first phase of a two-phase durable call, produced by [`CallHandle::begin`]. The durable
/// scope is already open and the begin index is known; the host-call `Start` has not yet been
/// written (live) nor claimed (replay). Finalised into a [`CallHandle`] with [`Self::start_live`]
/// (after the request has been built) or [`Self::start_replay`].
pub struct BegunCall<Pair: HostPayloadPair, P: DropPolicy> {
    begin_index: OplogIndex,
    execution_scope: BegunCallExecutionScope,
    retry: InFunctionRetryController,
    drop_sink: Option<UnboundedSender<DropEvent>>,
    cleanup_sink: Option<UnboundedSender<DropEvent>>,
    _phantom: PhantomData<(Pair, P)>,
}

impl<Pair: HostPayloadPair, P: DropPolicy> BegunCall<Pair, P> {
    pub fn is_live(&self) -> bool {
        self.retry.durable_execution_state().is_live
    }

    /// The index returned by `begin_durable_function` — see [`CallHandle::begin_index`]. Available
    /// before the request is finalised, so a two-step call can derive its request payload from it.
    pub fn begin_index(&self) -> OplogIndex {
        self.begin_index
    }

    /// Second phase on the live path: upload the request and append the eager host-call `Start`
    /// (or, while snapshotting, persist nothing).
    pub async fn start_live<Ctx: WorkerCtx>(
        self,
        ctx: &mut DurableWorkerCtx<Ctx>,
        request: Pair::Req,
    ) -> Result<CallHandle<Pair, P>, WorkerExecutorError> {
        debug_assert!(self.is_live(), "start_live() called on a replay handle");
        let snapshotting = self
            .retry
            .durable_execution_state()
            .snapshotting_mode
            .is_some();
        // The host-call `Start` nests inside the enclosing durable scope captured at initiation
        // (its own opened scope, or the scope encoded in the function type), derived explicitly —
        // never from the set of temporally-open sibling scopes. `None` for a top-level unscoped call.
        let parent_start_index = self.execution_scope.parent_start_index;
        let (start_idx, persisted, request_upload) = if snapshotting {
            // Snapshotting mode persists nothing.
            let oplog = ctx.state.oplog.clone();
            (
                oplog.current_oplog_index().await,
                false,
                PendingUpload::already_durable(),
            )
        } else {
            let request: HostRequest = request.into();
            let function_type = self.retry.function_type().clone();
            let oplog = ctx.state.oplog.clone();
            // Reserve the request payload and append the `Start` in one guarded step: a big request
            // blob's upload is *begun* but not awaited, so the `Start` is appended in initiation
            // order before the (potentially slow) upload finishes. `add_start_with_reserved_payload`
            // forbids — at compile time — any `.await` between reserving the payload and appending
            // the `Start`, which is what keeps concurrent calls' `Start` entries in initiation
            // order. The returned upload is awaited before this call's `End` / `Cancelled`.
            let (idx, request_upload) = oplog
                .add_start_with_reserved_payload(request, move |request_payload| {
                    OplogEntry::Start {
                        timestamp: Timestamp::now_utc(),
                        parent_start_index,
                        function_name: Pair::HOST_FUNCTION_NAME,
                        request: Some(request_payload),
                        durable_function_type: function_type,
                    }
                })
                .await
                .map_err(|err| {
                    WorkerExecutorError::runtime(format!(
                        "failed to serialize and store durable call request: {err}"
                    ))
                })?;
            (idx, true, request_upload)
        };
        let atomic_lease = if persisted {
            if let Some(begin_index) = self.execution_scope.atomic_region {
                let lease = ctx.state.register_atomic_region_call(
                    begin_index,
                    self.retry.can_reexecute_on_incomplete_replay(),
                );
                if lease.is_none() {
                    return Err(WorkerExecutorError::runtime(format!(
                        "durable call {start_idx} started in atomic region {begin_index}, but the region is not open"
                    )));
                }
                lease
            } else {
                None
            }
        } else {
            // Snapshotting persists nothing; keep the initiation-time region for trap/retry
            // classification without joining the live in-flight member guard.
            unregistered_atomic_lease(
                self.execution_scope.atomic_region,
                self.retry.can_reexecute_on_incomplete_replay(),
            )
        };
        let execution_scope = self.execution_scope.finish(start_idx, atomic_lease);
        Ok(CallHandle {
            start_idx,
            begin_index: self.begin_index,
            is_live: true,
            persisted,
            request_upload,
            replay: None,
            finished: false,
            parked_undelivered_replay: false,
            execution_scope,
            retry: self.retry,
            drop_sink: self.drop_sink,
            cleanup_sink: self.cleanup_sink,
            live_call_permit: Some(LiveCallPermit::new(ctx.state.live_host_call_counter())),
            _phantom: PhantomData,
        })
    }

    /// Second phase on the replay path: claim the next host-call `Start` from the oplog and register
    /// a resolver receiver for it.
    pub async fn start_replay<Ctx: WorkerCtx>(
        self,
        ctx: &mut DurableWorkerCtx<Ctx>,
    ) -> Result<CallHandle<Pair, P>, WorkerExecutorError> {
        debug_assert!(!self.is_live(), "start_replay() called on a live handle");
        // Defensive guard, mirroring `read_persisted_durable_function_invocation`.
        if self.retry.durable_execution_state().persistence_level
            == PersistenceLevel::PersistNothing
        {
            return Err(WorkerExecutorError::runtime(
                "Trying to replay a durable invocation in a PersistNothing block",
            ));
        }
        let replay = ctx
            .state
            .replay_state
            .claim_concurrent_start(&Pair::HOST_FUNCTION_NAME, self.retry.function_type())
            .await?;
        let start_idx = replay.start_idx();
        // Replay handles never participate in the live in-flight member guard, but keep their
        // initiation-time region for trap/retry classification.
        let atomic_lease = unregistered_atomic_lease(
            self.execution_scope.atomic_region,
            self.retry.can_reexecute_on_incomplete_replay(),
        );
        let execution_scope = self.execution_scope.finish(start_idx, atomic_lease);
        Ok(CallHandle {
            start_idx,
            begin_index: self.begin_index,
            is_live: false,
            persisted: false,
            request_upload: PendingUpload::already_durable(),
            replay: Some(replay),
            finished: false,
            parked_undelivered_replay: false,
            execution_scope,
            retry: self.retry,
            drop_sink: self.drop_sink,
            cleanup_sink: self.cleanup_sink,
            live_call_permit: None,
            _phantom: PhantomData,
        })
    }
}

impl<Pair: HostPayloadPair, P: DropPolicy> Drop for CallHandle<Pair, P> {
    fn drop(&mut self) {
        if self.finished {
            return;
        }
        if self.is_live {
            if self.persisted {
                // A live call dropped without finish/cancel: run the compile-time drop policy.
                let trap_context = self.trap_context();
                P::unfinished_drop(
                    DroppedCall {
                        start_idx: self.start_idx,
                        begin_index: self.begin_index,
                        function_type: self.retry.function_type().clone(),
                        request_upload: self.request_upload.clone(),
                        atomic_lease: self.execution_scope.atomic_lease.clone(),
                        trap_context,
                        live_call_permit: self.live_call_permit.take(),
                    },
                    self.drop_sink.as_ref(),
                );
            }
            // Not persisted (snapshotting): there is nothing on disk to reconcile.
        } else {
            if opens_replay_durable_scope(
                self.retry.function_type(),
                self.retry.durable_execution_state().assume_idempotence,
            ) && let Some(sink) = &self.drop_sink
            {
                let _ = sink.send(DropEvent::CloseDurableScope {
                    function_type: self.retry.function_type().clone(),
                    begin_index: self.begin_index,
                    span_id: None,
                });
            }
            if self.parked_undelivered_replay {
                // Expected: the recorded terminal was never delivered to the guest live
                // (`Cancelled { partial: None }` or an `End` marked `CompletionDiscarded`) and
                // the deterministic guest dropped this future at the same point it did live. Any
                // scope close was enqueued above.
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
