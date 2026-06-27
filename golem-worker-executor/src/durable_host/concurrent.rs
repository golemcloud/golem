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

//! Concurrent-replay core for durable host calls.
//!
//! A durable host call is identified by the [`OplogIndex`] of its `Start` entry. While live,
//! the call eagerly appends a `Start` (capturing its request) and later an `End` (its response)
//! or a `Cancelled`. During replay the [`ConcurrentReplayResolver`] matches each completed
//! `End`/`Cancelled` back to the awaiting [`CallHandle`] via a [`ReplayableOneshot`], so the two
//! halves of a call no longer have to be adjacent in the oplog — which is what lets us track
//! async, parallel host functions.
//!
//! Every durable host call runs through this path via [`CallHandle`]. Because the ported host
//! methods still take `&mut self`, two calls cannot truly overlap yet, so the resolver's
//! out-of-order behaviour is proven by synthetic unit tests rather than a concurrent runtime test.

use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::marker::PhantomData;
use std::pin::Pin;
use std::sync::Arc;

use anyhow::Error;
use async_trait::async_trait;
use golem_common::model::invocation_context::SpanId;
use golem_common::model::oplog::{
    DurableFunctionType, HostPayloadPair, HostRequest, HostResponse, OplogEntry, OplogIndex,
    OplogPayload, PersistenceLevel, ScopeScanState, host_functions::HostFunctionName,
};
use golem_common::model::regions::OplogRegion;
use golem_common::model::{RetryProperties, Timestamp};
use golem_service_base::error::worker_executor::{
    GolemSpecificWasmTrap, InterruptKind, WorkerExecutorError,
};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tokio::sync::oneshot;
use wasmtime::component::{Accessor, HasData};

use crate::durable_host::durability::{
    DurabilityHost, DurableCallTrapContext, DurableCallTrapError, DurableExecutionState,
    HostFailureKind, InFunctionRetryController, InFunctionRetryHost, InternalRetryResult,
    TerminalCallError, mark_durable_call_trap_context, try_trigger_host_trap_retry,
};
use crate::durable_host::replay_state::OplogEntryLookupResult;
use crate::durable_host::{DurableScopeKind, DurableWorkerCtx, PublicDurableWorkerState};
use crate::services::HasWorker;
use crate::services::oplog::{CommitLevel, Oplog, OplogOps, PendingUpload};
use crate::workerctx::{InvocationContextManagement, WorkerCtx};
use std::fmt::Display;

fn ambient_trap_context<Ctx: WorkerCtx>(ctx: &DurableWorkerCtx<Ctx>) -> DurableCallTrapContext {
    DurableCallTrapContext {
        retry_from: InFunctionRetryHost::current_retry_point(ctx),
        in_atomic_region: InFunctionRetryHost::in_atomic_region(ctx),
    }
}

/// Replayable single-shot channel used to deliver a call's [`Resolution`] from the replay cursor
/// to the awaiting [`CallHandle`].
///
/// `tokio::sync::oneshot` already supports send-before-await, which is all this currently needs.
/// The only "resolve happened before the awaiter registered" case is handled by the resolver's
/// `buffered` map, not by the channel. This is kept behind a type alias so it can later be swapped
/// for a dedicated replayable primitive.
pub type ReplayableOneshot<T> = oneshot::Sender<T>;
pub type ReplayableOneshotReceiver<T> = oneshot::Receiver<T>;

/// The outcome of a durable call as observed while replaying the oplog.
///
/// The entry index is carried purely for validation and diagnostics.
#[derive(Debug, Clone)]
pub enum Resolution {
    /// The call completed successfully via an `End` entry.
    Completed {
        end_idx: OplogIndex,
        response: Option<OplogPayload<HostResponse>>,
        #[expect(
            dead_code,
            reason = "preserved for the concurrent-durability replay model"
        )]
        forced_commit: bool,
    },
    /// The call was cancelled (dropped before completion) via a `Cancelled` entry.
    Cancelled {
        cancelled_idx: OplogIndex,
        partial: Option<OplogPayload<HostResponse>>,
    },
}

/// The outcome of driving the replay cursor for a durable call.
///
/// With eager `Start` every durable call writes its `Start` before the side effect, so a forced
/// commit elsewhere can make a lone `Start` durable before its `End`. When replay reaches the end
/// of the oplog without ever seeing the matching `End`/`Cancelled`, the call is reported as
/// [`ResolutionOutcome::Incomplete`] so the caller can re-execute it live and complete the existing
/// `Start`, instead of failing the whole replay.
#[derive(Debug)]
pub enum ResolutionOutcome {
    /// The call's `End`/`Cancelled` was observed during replay.
    Resolved(Resolution),
    /// Replay reached the end of the oplog (now live) without the call's `End`/`Cancelled`.
    Incomplete,
}

/// The result of [`CallHandle::replay`].
pub enum CallReplayOutcome<Pair: HostPayloadPair, P: DropPolicy> {
    /// The call's `End` was replayed and decoded into its response.
    Replayed(Pair::Resp),
    /// The call's `Start` was committed but its `End` never was. The returned handle has been
    /// switched to live completion of that existing `Start`: the caller must re-run the side effect
    /// and call [`CallHandle::complete`] (which appends the missing `End`). Only produced for
    /// function types that are safe to re-execute.
    Incomplete(CallHandle<Pair, P>),
}

/// Matches replayed `End`/`Cancelled` entries back to the [`CallHandle`]s awaiting them, keyed by
/// the `OplogIndex` of the call's `Start`.
///
/// Lives inside the replay state behind its lock. It is fed **only** from the committed-consume
/// hook (see [`crate::durable_host::replay_state::ReplayState`]); speculative cursor reads that
/// roll back must never reach it.
#[derive(Debug, Default)]
pub struct ConcurrentReplayResolver {
    /// Awaiters that have registered but whose resolution has not been observed yet.
    pending: HashMap<OplogIndex, ReplayableOneshot<ResolutionOutcome>>,
    /// Resolutions observed before their awaiter registered. While durable host calls are
    /// serialized this is always empty (the await-resolution guard guarantees a call's `Start` is
    /// claimed before its `End`/`Cancelled` is consumed); it exists for the resolver's own unit
    /// tests and for once host calls can genuinely overlap and that order is no longer guaranteed.
    buffered: HashMap<OplogIndex, ResolutionOutcome>,
}

impl ConcurrentReplayResolver {
    /// Registers an awaiter for the call started at `start_idx` and returns the receiver it should
    /// await on. If the resolution was already observed (buffered), the returned receiver is
    /// pre-resolved.
    pub fn register(
        &mut self,
        start_idx: OplogIndex,
    ) -> ReplayableOneshotReceiver<ResolutionOutcome> {
        let (tx, rx) = oneshot::channel();
        if let Some(resolution) = self.buffered.remove(&start_idx) {
            let _ = tx.send(resolution);
        } else {
            // A `Start` index is claimed (and thus registered) exactly once: claiming advances the
            // positional cursor past that `Start`. A second registration for the same index would
            // mean two awaiters for one call, silently dropping the first.
            debug_assert!(
                !self.pending.contains_key(&start_idx),
                "duplicate awaiter registered for Start at {start_idx}"
            );
            self.pending.insert(start_idx, tx);
        }
        rx
    }

    /// Resolves a registered awaiter, or buffers the resolution if none is registered yet.
    ///
    /// Test-only seam exercising the buffered (resolve-before-register) branch directly. The
    /// production replay path uses [`Self::resolve_if_pending`] instead, so that resolutions for
    /// calls nobody is awaiting are dropped rather than accumulating in `buffered`.
    #[cfg(test)]
    pub fn resolve(&mut self, start_idx: OplogIndex, resolution: Resolution) {
        let outcome = ResolutionOutcome::Resolved(resolution);
        if let Some(tx) = self.pending.remove(&start_idx) {
            let _ = tx.send(outcome);
        } else {
            self.buffered.insert(start_idx, outcome);
        }
    }

    /// Resolves a registered awaiter if (and only if) one exists, returning whether it did.
    ///
    /// This is the only entry point used by the committed-consume replay hook: an `End`/`Cancelled`
    /// for a call nobody is awaiting — e.g. the guest-facing manual durability pair written by
    /// `persist_durable_function_invocation`, which is consumed through the same cursor but never
    /// registers an awaiter — is silently ignored rather than buffered forever.
    pub fn resolve_if_pending(&mut self, start_idx: OplogIndex, resolution: Resolution) -> bool {
        if let Some(tx) = self.pending.remove(&start_idx) {
            let _ = tx.send(ResolutionOutcome::Resolved(resolution));
            true
        } else {
            false
        }
    }

    /// Resolves every still-registered awaiter as [`ResolutionOutcome::Incomplete`].
    ///
    /// Called when replay reaches the end of the oplog ([`crate::durable_host::replay_state::ReplayState::switch_to_live`]):
    /// any call whose `Start` was committed but whose `End`/`Cancelled` never was is, by definition,
    /// incomplete. Waking the awaiters here (rather than relying on each to notice end-of-replay
    /// itself) is what lets a call that is *suspended* waiting for the cursor to advance — because a
    /// concurrently-replaying sibling call owns the cursor head — make progress once replay finishes
    /// instead of hanging forever.
    pub fn fail_all_pending_incomplete(&mut self) {
        for (_start_idx, tx) in self.pending.drain() {
            let _ = tx.send(ResolutionOutcome::Incomplete);
        }
    }

    /// Removes a registered awaiter without resolving it. Used when a claimed call turns out to be
    /// incomplete on replay (its `Start` is committed but its `End` never was): the awaiter is
    /// switched to live completion, so its pending registration must not linger in the resolver.
    pub fn unregister(&mut self, start_idx: OplogIndex) {
        self.pending.remove(&start_idx);
    }

    /// Returns whether an awaiter is currently registered for `start_idx`.
    ///
    /// The replay cursor uses this to decide which `End`/`Cancelled` entries are *awaited
    /// terminals* it may auto-drain (and route back to their awaiter) versus the ones it must leave
    /// for their own positional consumer: scope `End`s, unclaimed `Start`s, and deterministic
    /// markers.
    pub fn is_pending(&self, start_idx: OplogIndex) -> bool {
        self.pending.contains_key(&start_idx)
    }
}

/// Replay-side state for a single in-flight call: the `Start` index it claimed and the receiver
/// that will deliver its [`Resolution`].
#[derive(Debug)]
pub struct ReplayCallHandle {
    start_idx: OplogIndex,
    receiver: ReplayableOneshotReceiver<ResolutionOutcome>,
}

impl ReplayCallHandle {
    pub fn new(
        start_idx: OplogIndex,
        receiver: ReplayableOneshotReceiver<ResolutionOutcome>,
    ) -> Self {
        Self {
            start_idx,
            receiver,
        }
    }

    pub fn start_idx(&self) -> OplogIndex {
        self.start_idx
    }

    /// Consumes the handle into its parts (used by the replay-state driver).
    pub fn into_parts(self) -> (OplogIndex, ReplayableOneshotReceiver<ResolutionOutcome>) {
        (self.start_idx, self.receiver)
    }
}

/// Call-owned facts available to a cancellation recorder when a live persisted handle is dropped.
///
/// `Drop` cannot use wasmtime's `Accessor`, cannot borrow worker state, and cannot `.await`; any
/// production recorder must receive everything it needs from the handle itself and do the async
/// oplog/state work later.
#[derive(Debug, Clone)]
pub struct DroppedCall {
    start_idx: OplogIndex,
    begin_index: OplogIndex,
    function_type: DurableFunctionType,
    request_upload: PendingUpload,
    atomic_region_registration: Option<OplogIndex>,
    /// The dropped call's own trap classification, captured from its execution scope at drop time.
    /// A cancellation-drain failure (deferred request upload / terminal recorder join) traps with
    /// this context so the retry grouping belongs to the dropped call, not to whichever later host
    /// call happens to drive the drain.
    trap_context: DurableCallTrapContext,
}

impl DroppedCall {
    pub fn start_idx(&self) -> OplogIndex {
        self.start_idx
    }

    pub fn begin_index(&self) -> OplogIndex {
        self.begin_index
    }

    pub fn function_type(&self) -> &DurableFunctionType {
        &self.function_type
    }

    pub fn request_upload(&self) -> &PendingUpload {
        &self.request_upload
    }

    pub fn atomic_region_registration(&self) -> Option<OplogIndex> {
        self.atomic_region_registration
    }

    pub fn trap_context(&self) -> DurableCallTrapContext {
        self.trap_context
    }

    async fn wait_request_upload(&self) -> Result<(), WorkerExecutorError> {
        self.request_upload.wait().await.map_err(|err| {
            WorkerExecutorError::runtime(format!(
                "failed to serialize and store durable call request: {err}"
            ))
        })
    }

    async fn append_cancelled<Ctx: WorkerCtx>(
        self,
        ctx: &mut DurableWorkerCtx<Ctx>,
        partial: Option<OplogPayload<HostResponse>>,
    ) -> Result<(), WorkerExecutorError> {
        self.append_cancelled_with_oplog(ctx.state.oplog.clone(), partial)
            .await?;
        if let Some(begin_index) = self.atomic_region_registration {
            ctx.state.unregister_atomic_region_call(begin_index);
        }
        ctx.end_durable_function(&self.function_type, self.begin_index, false)
            .await?;
        Ok(())
    }

    async fn append_cancelled_with_oplog(
        &self,
        oplog: std::sync::Arc<dyn crate::services::oplog::Oplog>,
        partial: Option<OplogPayload<HostResponse>>,
    ) -> Result<(), WorkerExecutorError> {
        let cancelled = OplogEntry::Cancelled {
            timestamp: Timestamp::now_utc(),
            start_index: self.start_idx,
            partial,
        };
        oplog.add(cancelled).await;
        Ok(())
    }
}

/// Event emitted when a [`CallHandle`] is dropped without being finished or cancelled.
///
/// There is no recorder actor in production yet, so an unfinished drop only logs. The drop policy
/// is exercised in unit tests by attaching a sink that records these events.
#[derive(Debug)]
pub enum DropEvent {
    /// A `Cancellable` handle was dropped unfinished; the next drain records `Cancelled` from this
    /// call-owned snapshot and closes the matching durable-function scope.
    UnfinishedCancellable { call: DroppedCall },
    /// A `NotCancellable` handle was dropped unfinished; this is a programming error.
    UnfinishedNotCancellable { call: DroppedCall },
    /// A terminal was already being recorded when the future was dropped; only in-memory atomic
    /// membership cleanup is needed, not another durable terminal entry.
    CleanupAtomicRegion { begin_index: OplogIndex },
    /// A terminal append was handed to an owned task; wait for it before in-memory cleanup. A join
    /// failure traps with the dropped call's own `trap_context` rather than ambient state.
    CleanupAfterTerminal {
        atomic_region_registration: Option<OplogIndex>,
        function_type: DurableFunctionType,
        durable_begin_index: OplogIndex,
        terminal: Option<tokio::task::JoinHandle<Result<(), WorkerExecutorError>>>,
        trap_context: DurableCallTrapContext,
    },
    /// A guest-cancelled accessor future may leave a caller-managed durable scope with no code path
    /// back into the resource's `drop`. Close that parent scope from the next safe store-access
    /// window. The close is idempotent because the resource may be dropped before this event drains.
    CloseDurableScope {
        function_type: DurableFunctionType,
        begin_index: OplogIndex,
        span_id: Option<SpanId>,
    },
}

struct AccessDropEventDrainGuard {
    sink: UnboundedSender<DropEvent>,
    pending: VecDeque<DropEvent>,
    current: Option<DropEvent>,
    disarmed: bool,
}

impl AccessDropEventDrainGuard {
    fn new(sink: UnboundedSender<DropEvent>, events: Vec<DropEvent>) -> Self {
        Self {
            sink,
            pending: events.into(),
            current: None,
            disarmed: false,
        }
    }

    fn start_next(&mut self) -> bool {
        debug_assert!(self.current.is_none());
        self.current = self.pending.pop_front();
        self.current.is_some()
    }

    fn current_mut(&mut self) -> &mut DropEvent {
        self.current
            .as_mut()
            .expect("access dropped-call drain has an active event")
    }

    fn replace_current(&mut self, event: DropEvent) {
        self.current = Some(event);
    }

    fn finish_current(&mut self) {
        self.current = None;
    }

    fn disarm(&mut self) {
        self.disarmed = true;
    }
}

impl Drop for AccessDropEventDrainGuard {
    fn drop(&mut self) {
        if self.disarmed {
            return;
        }
        if let Some(event) = self.current.take() {
            let _ = self.sink.send(event);
        }
        while let Some(event) = self.pending.pop_front() {
            let _ = self.sink.send(event);
        }
    }
}

enum AccessTerminalGuardState {
    BeforeTerminal {
        call: DroppedCall,
    },
    CleanupAfterTerminal {
        atomic_region_registration: Option<OplogIndex>,
        function_type: DurableFunctionType,
        durable_begin_index: OplogIndex,
        terminal: Option<tokio::task::JoinHandle<Result<(), WorkerExecutorError>>>,
        trap_context: DurableCallTrapContext,
    },
    Disarmed,
}

struct AccessTerminalGuard<P: DropPolicy> {
    state: AccessTerminalGuardState,
    sink: Option<UnboundedSender<DropEvent>>,
    _phantom: PhantomData<P>,
}

impl<P: DropPolicy> AccessTerminalGuard<P> {
    fn new(call: DroppedCall, sink: Option<UnboundedSender<DropEvent>>) -> Self {
        Self {
            state: AccessTerminalGuardState::BeforeTerminal { call },
            sink,
            _phantom: PhantomData,
        }
    }

    fn atomic_region_registration(&self) -> Option<OplogIndex> {
        match &self.state {
            AccessTerminalGuardState::BeforeTerminal { call } => call.atomic_region_registration(),
            AccessTerminalGuardState::CleanupAfterTerminal {
                atomic_region_registration,
                ..
            } => *atomic_region_registration,
            AccessTerminalGuardState::Disarmed => None,
        }
    }

    fn call(&self) -> Option<&DroppedCall> {
        match &self.state {
            AccessTerminalGuardState::BeforeTerminal { call } => Some(call),
            _ => None,
        }
    }

    fn cleanup_after_terminal(
        &mut self,
        terminal: tokio::task::JoinHandle<Result<(), WorkerExecutorError>>,
    ) {
        let call = self
            .call()
            .expect("cleanup_after_terminal called before the terminal is armed");
        let atomic_region_registration = call.atomic_region_registration();
        let function_type = call.function_type().clone();
        let durable_begin_index = call.begin_index();
        let trap_context = call.trap_context();
        self.state = AccessTerminalGuardState::CleanupAfterTerminal {
            atomic_region_registration,
            function_type,
            durable_begin_index,
            terminal: Some(terminal),
            trap_context,
        };
    }

    async fn wait_terminal(&mut self) -> Result<(), WorkerExecutorError> {
        if let AccessTerminalGuardState::CleanupAfterTerminal { terminal, .. } = &mut self.state {
            if let Some(handle) = terminal {
                handle.await.map_err(|err| {
                    WorkerExecutorError::runtime(format!(
                        "durable call terminal recorder task failed: {err}"
                    ))
                })??;
            }
            *terminal = None;
        }
        Ok(())
    }

    fn disarm(&mut self) {
        self.state = AccessTerminalGuardState::Disarmed;
    }
}

impl<P: DropPolicy> Drop for AccessTerminalGuard<P> {
    fn drop(&mut self) {
        match std::mem::replace(&mut self.state, AccessTerminalGuardState::Disarmed) {
            AccessTerminalGuardState::BeforeTerminal { call } => {
                P::unfinished_drop(call, self.sink.as_ref());
            }
            AccessTerminalGuardState::CleanupAfterTerminal {
                atomic_region_registration,
                function_type,
                durable_begin_index,
                terminal,
                trap_context,
            } => {
                if let Some(sink) = &self.sink {
                    let _ = sink.send(DropEvent::CleanupAfterTerminal {
                        atomic_region_registration,
                        function_type,
                        durable_begin_index,
                        terminal,
                        trap_context,
                    });
                }
            }
            AccessTerminalGuardState::Disarmed => {}
        }
    }
}

/// Drains currently queued dropped-call events and records cancellable drops as `Cancelled`.
///
/// A future p3 wrapper can enqueue from `Drop` and call this from the next safe worker-access window.
/// The helper deliberately drains only currently available events; callers decide where to wait for
/// more work.
#[expect(
    dead_code,
    reason = "p3 cancellable host wrappers will drain this queue once they are wired"
)]
pub async fn drain_dropped_call_events<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    receiver: &mut UnboundedReceiver<DropEvent>,
) -> Result<usize, TerminalCallError> {
    let mut recorded = 0;
    while let Ok(event) = receiver.try_recv() {
        record_dropped_call_event(ctx, event).await?;
        recorded += 1;
    }
    Ok(recorded)
}

pub async fn drain_queued_dropped_call_events<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
) -> Result<usize, TerminalCallError> {
    let mut recorded = 0;
    while let Ok(event) = ctx.state.dropped_call_events.1.try_recv() {
        record_dropped_call_event(ctx, event).await?;
        recorded += 1;
    }
    Ok(recorded)
}

async fn record_dropped_call_event<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    event: DropEvent,
) -> Result<(), TerminalCallError> {
    match event {
        DropEvent::UnfinishedCancellable { call } => {
            let context = call.trap_context();
            call.wait_request_upload()
                .await
                .map_err(|err| TerminalCallError::new(err, context))?;
            call.append_cancelled(ctx, None)
                .await
                .map_err(|err| TerminalCallError::new(err, context))?;
        }
        DropEvent::UnfinishedNotCancellable { call } => {
            return Err(TerminalCallError::new(
                WorkerExecutorError::runtime(format!(
                    "non-cancellable durable call {} dropped without finish/cancel",
                    call.start_idx()
                )),
                call.trap_context(),
            ));
        }
        DropEvent::CleanupAtomicRegion { begin_index } => {
            ctx.state.unregister_atomic_region_call(begin_index);
        }
        DropEvent::CleanupAfterTerminal {
            atomic_region_registration,
            function_type,
            durable_begin_index,
            terminal,
            trap_context,
        } => {
            if let Some(terminal) = terminal {
                let joined = terminal.await.map_err(|err| {
                    WorkerExecutorError::runtime(format!(
                        "durable call terminal recorder task failed: {err}"
                    ))
                });
                match joined {
                    Ok(Ok(())) => {}
                    Ok(Err(err)) | Err(err) => {
                        return Err(TerminalCallError::new(err, trap_context));
                    }
                }
            }
            if let Some(begin_index) = atomic_region_registration {
                ctx.state.unregister_atomic_region_call(begin_index);
            }
            ctx.end_durable_function(&function_type, durable_begin_index, false)
                .await
                .map_err(|err| TerminalCallError::new(err, trap_context))?;
        }
        DropEvent::CloseDurableScope {
            function_type,
            begin_index,
            span_id,
        } => {
            if ctx.state.is_durable_scope_open(begin_index) {
                ctx.end_durable_function(&function_type, begin_index, false)
                    .await
                    .map_err(|err| TerminalCallError::new(err, ambient_trap_context(ctx)))?;
                if let Some(span_id) = span_id {
                    ctx.finish_span(&span_id)
                        .await
                        .map_err(|err| TerminalCallError::new(err, ambient_trap_context(ctx)))?;
                }
            }
        }
    }
    Ok(())
}

/// Accessor-window variant of [`drain_dropped_call_events`]. It drains the queue from a short
/// worker-state window, records `Cancelled` entries using owned oplog handles outside the window,
/// then re-enters only to unregister atomic-region membership.
pub async fn drain_dropped_call_events_access<T, D, Ctx>(
    store: &Accessor<T, D>,
    get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
) -> Result<usize, TerminalCallError>
where
    T: 'static,
    D: HasData + ?Sized,
    Ctx: WorkerCtx,
{
    let (oplog, sink, events) = store.with(|mut access| {
        let ctx = get_ctx(access.data_mut());
        (
            ctx.state.oplog.clone(),
            ctx.state
                .dropped_call_event_sender()
                .expect("dropped-call event sender is always available"),
            ctx.state.take_dropped_call_events(),
        )
    });
    let mut drain = AccessDropEventDrainGuard::new(sink, events);
    let mut recorded = 0;
    let mut first_error = None;
    while drain.start_next() {
        match drain.current_mut() {
            DropEvent::UnfinishedCancellable { call } => {
                let begin_index = call.atomic_region_registration();
                let context = call.trap_context();
                let function_type = call.function_type().clone();
                let durable_begin_index = call.begin_index();
                let result = async {
                    call.wait_request_upload().await?;
                    call.append_cancelled_with_oplog(oplog.clone(), None).await
                }
                .await;
                match result {
                    Ok(()) => {
                        if let Some(begin_index) = begin_index {
                            drain.replace_current(DropEvent::CleanupAtomicRegion { begin_index });
                            store.with(|mut access| {
                                let ctx = get_ctx(access.data_mut());
                                ctx.state.unregister_atomic_region_call(begin_index);
                            });
                        }
                        if let Err(err) = end_durable_function_access(
                            store,
                            get_ctx,
                            function_type,
                            durable_begin_index,
                            false,
                        )
                        .await
                        {
                            if first_error.is_none() {
                                first_error = Some(TerminalCallError::new(err, context));
                            }
                        }
                        recorded += 1;
                    }
                    Err(err) => {
                        if first_error.is_none() {
                            first_error = Some(TerminalCallError::new(err, context));
                        }
                    }
                }
            }
            DropEvent::UnfinishedNotCancellable { call } => {
                let context = call.trap_context();
                let start_idx = call.start_idx();
                first_error.get_or_insert_with(|| {
                    TerminalCallError::new(
                        WorkerExecutorError::runtime(format!(
                            "non-cancellable durable call {start_idx} dropped without finish/cancel"
                        )),
                        context,
                    )
                });
            }
            DropEvent::CleanupAtomicRegion { begin_index } => {
                let begin_index = *begin_index;
                store.with(|mut access| {
                    let ctx = get_ctx(access.data_mut());
                    ctx.state.unregister_atomic_region_call(begin_index);
                });
                recorded += 1;
            }
            DropEvent::CleanupAfterTerminal {
                atomic_region_registration,
                function_type,
                durable_begin_index,
                terminal,
                trap_context,
            } => {
                let atomic_region_registration = *atomic_region_registration;
                let function_type = function_type.clone();
                let durable_begin_index = *durable_begin_index;
                let trap_context = *trap_context;
                let mut terminal_recorded = true;
                if let Some(handle) = terminal {
                    match handle.await.map_err(|err| {
                        WorkerExecutorError::runtime(format!(
                            "durable call terminal recorder task failed: {err}"
                        ))
                    }) {
                        Ok(Ok(())) => {
                            *terminal = None;
                        }
                        Ok(Err(err)) | Err(err) => {
                            if first_error.is_none() {
                                first_error = Some(TerminalCallError::new(err, trap_context));
                            }
                            terminal_recorded = false;
                        }
                    }
                }
                if terminal_recorded {
                    if let Some(begin_index) = atomic_region_registration {
                        store.with(|mut access| {
                            let ctx = get_ctx(access.data_mut());
                            ctx.state.unregister_atomic_region_call(begin_index);
                        });
                    }
                    if let Err(err) = end_durable_function_access(
                        store,
                        get_ctx,
                        function_type,
                        durable_begin_index,
                        false,
                    )
                    .await
                    {
                        if first_error.is_none() {
                            first_error = Some(TerminalCallError::new(err, trap_context));
                        }
                    } else {
                        recorded += 1;
                    }
                }
            }
            DropEvent::CloseDurableScope {
                function_type,
                begin_index,
                span_id,
            } => {
                let function_type = function_type.clone();
                let begin_index = *begin_index;
                let span_id = span_id.clone();
                match end_durable_function_access_if_open(
                    store,
                    get_ctx,
                    function_type,
                    begin_index,
                    false,
                )
                .await
                {
                    Ok(true) => {
                        if let Some(span_id) = span_id
                            && let Err(err) = finish_span_access(store, get_ctx, &span_id).await
                            && first_error.is_none()
                        {
                            first_error = Some(TerminalCallError::new(
                                err,
                                store.with(|mut access| {
                                    let ctx = get_ctx(access.data_mut());
                                    ambient_trap_context(ctx)
                                }),
                            ));
                        }
                        recorded += 1;
                    }
                    Ok(false) => {}
                    Err(err) => {
                        if first_error.is_none() {
                            first_error = Some(TerminalCallError::new(
                                err,
                                store.with(|mut access| {
                                    let ctx = get_ctx(access.data_mut());
                                    ambient_trap_context(ctx)
                                }),
                            ));
                        }
                    }
                }
            }
        }
        drain.finish_current();
    }
    if let Some(err) = first_error {
        return Err(err);
    }
    drain.disarm();
    Ok(recorded)
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
            let start_idx = call.start_idx;
            tracing::warn!(
                "durable call {start_idx} dropped unfinished; no production cancellation recorder yet"
            );
        }
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
    start_idx: OplogIndex,
    /// The index returned by `begin_durable_function` / `begin_function`. For a non-idempotent
    /// `WriteRemote` (or a `WriteRemoteBatched(None)`) this is the **durable scope** `Start` that
    /// must be closed via `end_durable_function`; for every other function type it is just the
    /// pre-call index and `end_durable_function` only uses it to commit at the right boundary.
    begin_index: OplogIndex,
    is_live: bool,
    /// `true` when a `Start` entry was actually appended. It is `false` while snapshotting (where
    /// nothing is persisted) and for replay handles.
    persisted: bool,
    /// Tracks the (possibly deferred) blob upload of this call's request payload, started when the
    /// `Start` was reserved. Awaited before the matching `End` / `Cancelled` is appended so an
    /// upload failure surfaces at the call site rather than only at the leaf oplog's commit barrier.
    /// `PendingUpload::already_durable()` (a no-op) for replay handles, snapshotting, and inline
    /// requests.
    request_upload: PendingUpload,
    /// Replay-side resolver receiver; `Some` only for replay handles.
    replay: Option<ReplayCallHandle>,
    finished: bool,
    /// Initiation-time execution metadata owned by this call. Later phases still mirror selected
    /// fields into `PrivateDurableWorkerState` for compatibility, but the call-owned copy is the
    /// source we can move retry/atomic decisions onto as the Accessor reshape proceeds.
    execution_scope: CallExecutionScope,
    /// Atomic region membership registered while this live call is in flight. `Drop` deliberately
    /// does not clear it because drop-only cancellation cannot access the worker store; successful
    /// `complete`/`cancel` terminals clear it explicitly.
    atomic_region_registration: Option<OplogIndex>,
    /// In-function retry decision logic. Also the home of the call's `DurableFunctionType` and
    /// captured `DurableExecutionState`.
    retry: InFunctionRetryController,
    /// Optional sink used by unit tests to observe unfinished-drop behaviour. `None` in production.
    drop_sink: Option<UnboundedSender<DropEvent>>,
    _phantom: PhantomData<(Pair, P)>,
}

#[derive(Debug, Clone)]
struct BegunCallExecutionScope {
    /// The durable scope this host-call `Start` will be nested under, if any. This is derived from
    /// the call's own function type / begin index, never from temporally-open sibling scopes.
    parent_start_index: Option<OplogIndex>,
    /// Atomic region active when the durable call was initiated. This is captured now so later
    /// completion/error handling can move from completion-time state to initiation-time membership.
    #[allow(dead_code)]
    atomic_region: Option<OplogIndex>,
    /// Persistence level active when the call was initiated. Kept with the call so p3 Accessor
    /// windows can snapshot all execution facts before async work resumes elsewhere.
    #[allow(dead_code)]
    persistence_level: PersistenceLevel,
}

impl BegunCallExecutionScope {
    fn finish(self, start_idx: OplogIndex) -> CallExecutionScope {
        CallExecutionScope {
            retry_from: self.parent_start_index.unwrap_or(start_idx),
            durable_scope: self.parent_start_index,
            atomic_region: self.atomic_region,
            persistence_level: self.persistence_level,
        }
    }
}

#[derive(Debug, Clone)]
struct CallExecutionScope {
    /// The retry point owned by this in-flight call: the enclosing durable scope `Start` if present,
    /// otherwise the host-call `Start` itself.
    retry_from: OplogIndex,
    /// The enclosing durable scope, if this call belongs to one.
    #[allow(dead_code)]
    durable_scope: Option<OplogIndex>,
    /// Atomic region active when this call was initiated.
    #[allow(dead_code)]
    atomic_region: Option<OplogIndex>,
    /// Persistence level active when this call was initiated.
    #[allow(dead_code)]
    persistence_level: PersistenceLevel,
}

impl CallExecutionScope {
    fn atomic_region(&self) -> Option<OplogIndex> {
        self.atomic_region
    }

    /// The retry point to attach to a trap raised by this call: the call's own atomic region (whole
    /// region retried from its begin index) if it was initiated inside one, otherwise its enclosing
    /// durable scope `Start` or its own `Start`. This mirrors [`ScopedRetryHost::retry_point`] so a
    /// hard (non-semantic) trap groups exactly like an inline/semantic retry would.
    fn trap_retry_point(&self) -> OplogIndex {
        self.atomic_region.unwrap_or(self.retry_from)
    }
}

struct PreparedAccessStart<Pair: HostPayloadPair, P: DropPolicy, Ctx: WorkerCtx> {
    is_live: bool,
    snapshotting: bool,
    oplog: Arc<dyn Oplog>,
    public_state: PublicDurableWorkerState<Ctx>,
    replay_state: crate::durable_host::replay_state::ReplayState,
    execution_scope: BegunCallExecutionScope,
    retry: InFunctionRetryController,
    atomic_region_registration: Option<OplogIndex>,
    drop_sink: Option<UnboundedSender<DropEvent>>,
    cleanup_sink: Option<UnboundedSender<DropEvent>>,
    _phantom: PhantomData<(Pair, P)>,
}

struct ExecutedAccessStart<Pair: HostPayloadPair, P: DropPolicy> {
    begin_index: OplogIndex,
    start_idx: OplogIndex,
    persisted: bool,
    request_upload: PendingUpload,
    replay: Option<ReplayCallHandle>,
    execution_scope: CallExecutionScope,
    retry: InFunctionRetryController,
    atomic_region_registration: Option<OplogIndex>,
    opened_scope: Option<AccessOpenedScope>,
    drop_sink: Option<UnboundedSender<DropEvent>>,
    _phantom: PhantomData<(Pair, P)>,
}

struct AccessOpenedScope {
    begin_index: OplogIndex,
    replay_handle: Option<ReplayCallHandle>,
    switched_to_live: bool,
}

struct AccessStartCleanup {
    atomic_region_registration: Option<OplogIndex>,
}

struct AccessStartAtomicGuard {
    atomic_region_registration: Option<OplogIndex>,
    sink: Option<UnboundedSender<DropEvent>>,
}

impl AccessStartAtomicGuard {
    fn new(
        atomic_region_registration: Option<OplogIndex>,
        sink: Option<UnboundedSender<DropEvent>>,
    ) -> Self {
        Self {
            atomic_region_registration,
            sink,
        }
    }

    fn disarm(&mut self) {
        self.atomic_region_registration = None;
    }
}

impl Drop for AccessStartAtomicGuard {
    fn drop(&mut self) {
        if let Some(begin_index) = self.atomic_region_registration.take()
            && let Some(sink) = &self.sink
        {
            let _ = sink.send(DropEvent::CleanupAtomicRegion { begin_index });
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

struct ScopedRetryHost<'a, H> {
    inner: &'a mut H,
    execution_scope: &'a CallExecutionScope,
}

impl<'a, H> ScopedRetryHost<'a, H> {
    fn new(inner: &'a mut H, execution_scope: &'a CallExecutionScope) -> Self {
        Self {
            inner,
            execution_scope,
        }
    }

    fn retry_point(&self) -> OplogIndex {
        self.execution_scope
            .atomic_region
            .unwrap_or(self.execution_scope.retry_from)
    }
}

#[async_trait]
impl<H: InFunctionRetryHost + Send + Sync> InFunctionRetryHost for ScopedRetryHost<'_, H> {
    fn in_atomic_region(&self) -> bool {
        self.execution_scope.atomic_region.is_some()
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
        // Membership/initiation-precise: classify against the region this call was *initiated* in,
        // not whatever region happens to be outermost at retry time. A call started outside any
        // atomic region never writes `inside_atomic_region = true`, even if a sibling later opened
        // one.
        self.execution_scope
            .atomic_region
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
        if !is_accessor_supported_function_type(&function_type) {
            return Err(WorkerExecutorError::runtime(format!(
                "p3 accessor durable call path currently supports only ReadLocal/ReadRemote/WriteRemote/WriteRemoteBatched, got {function_type:?}"
            )));
        }
        let prepared = store.with(|mut access| {
            let ctx = get_ctx(access.data_mut());
            Self::prepare_access_start(ctx, function_type)
        })?;
        let mut start_guard = AccessStartAtomicGuard::new(
            prepared.atomic_region_registration,
            prepared.cleanup_sink.clone(),
        );

        if let Err(err) = drain_dropped_call_events_access(store, get_ctx).await {
            return Err(err.source);
        }

        match Self::execute_access_start(prepared, request).await {
            Ok(executed) => {
                if let Err(err) = process_pending_replay_events_access(store, get_ctx).await {
                    return Err(err);
                }
                let result = store.with(|mut access| {
                    let ctx = get_ctx(access.data_mut());
                    Self::finish_access_start(ctx, executed)
                });
                if result.is_ok() {
                    start_guard.disarm();
                }
                result
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

    fn prepare_access_start<Ctx: WorkerCtx>(
        ctx: &mut DurableWorkerCtx<Ctx>,
        function_type: DurableFunctionType,
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
        if durable_execution_state.is_live
            && durable_execution_state.snapshotting_mode.is_none()
            && let Some(begin_index) = atomic_region
            && !ctx.state.register_atomic_region_call(begin_index)
        {
            return Err(WorkerExecutorError::runtime(format!(
                "durable call started in atomic region {begin_index}, but the region is not open"
            )));
        }

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
        let cleanup_sink = ctx.state.dropped_call_event_sender();
        Ok(PreparedAccessStart {
            is_live,
            snapshotting,
            oplog: ctx.state.oplog.clone(),
            public_state: ctx.public_state.clone(),
            replay_state: ctx.state.replay_state.clone(),
            execution_scope,
            retry,
            atomic_region_registration: if is_live && !snapshotting {
                atomic_region
            } else {
                None
            },
            drop_sink: P::production_drop_sink(ctx.state.dropped_call_event_sender()),
            cleanup_sink,
            _phantom: PhantomData,
        })
    }

    async fn execute_access_start<Ctx: WorkerCtx>(
        prepared: PreparedAccessStart<Pair, P, Ctx>,
        request: Pair::Req,
    ) -> Result<ExecutedAccessStart<Pair, P>, (WorkerExecutorError, AccessStartCleanup)> {
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
            }
        }

        if is_live {
            if prepared.snapshotting {
                let start_idx = prepared.oplog.current_oplog_index().await;
                Ok(ExecutedAccessStart {
                    begin_index: scope_start
                        .as_ref()
                        .map(|scope| scope.begin_index)
                        .unwrap_or(start_idx),
                    start_idx,
                    persisted: false,
                    request_upload: PendingUpload::already_durable(),
                    replay: None,
                    execution_scope: execution_scope.finish(start_idx),
                    retry,
                    atomic_region_registration: None,
                    opened_scope: scope_start,
                    drop_sink: prepared.drop_sink,
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
                                atomic_region_registration: prepared.atomic_region_registration,
                            },
                        )
                    })?;
                Ok(ExecutedAccessStart {
                    begin_index: scope_start
                        .as_ref()
                        .map(|scope| scope.begin_index)
                        .unwrap_or(start_idx),
                    start_idx,
                    persisted: true,
                    request_upload,
                    replay: None,
                    execution_scope: execution_scope.finish(start_idx),
                    retry,
                    atomic_region_registration: prepared.atomic_region_registration,
                    opened_scope: scope_start,
                    drop_sink: prepared.drop_sink,
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
                        atomic_region_registration: prepared.atomic_region_registration,
                    },
                ));
            }
            let replay = prepared
                .replay_state
                .claim_concurrent_start(&Pair::HOST_FUNCTION_NAME, retry.function_type())
                .await
                .map_err(|err| {
                    (
                        err,
                        AccessStartCleanup {
                            atomic_region_registration: prepared.atomic_region_registration,
                        },
                    )
                })?;
            let start_idx = replay.start_idx();
            Ok(ExecutedAccessStart {
                begin_index: scope_start
                    .as_ref()
                    .map(|scope| scope.begin_index)
                    .unwrap_or(start_idx),
                start_idx,
                persisted: false,
                request_upload: PendingUpload::already_durable(),
                replay: Some(replay),
                execution_scope: execution_scope.finish(start_idx),
                retry,
                atomic_region_registration: None,
                opened_scope: scope_start,
                drop_sink: prepared.drop_sink,
                _phantom: PhantomData,
            })
        }
    }

    async fn execute_access_scope_start<Ctx: WorkerCtx>(
        prepared: &PreparedAccessStart<Pair, P, Ctx>,
    ) -> Result<AccessOpenedScope, (WorkerExecutorError, AccessStartCleanup)> {
        let function_type = prepared.retry.function_type().clone();
        if prepared.is_live {
            let entry = OplogEntry::Start {
                timestamp: Timestamp::now_utc(),
                parent_start_index: None,
                function_name: HostFunctionName::Custom("<scope:batched-write>".to_string()),
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
            let scope_name = HostFunctionName::Custom("<scope:batched-write>".to_string());
            let (begin_index, replay_handle) = prepared
                .replay_state
                .claim_scope_start(&scope_name, &function_type)
                .await
                .map_err(|err| {
                    (
                        err,
                        AccessStartCleanup {
                            atomic_region_registration: prepared.atomic_region_registration,
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
                            atomic_region_registration: prepared.atomic_region_registration,
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
                                atomic_region_registration: prepared.atomic_region_registration,
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
        Ok(CallHandle {
            start_idx: executed.start_idx,
            begin_index: executed.begin_index,
            is_live: executed.replay.is_none(),
            persisted: executed.persisted,
            request_upload: executed.request_upload,
            replay: executed.replay,
            finished: false,
            execution_scope: executed.execution_scope,
            atomic_region_registration: executed.atomic_region_registration,
            retry: executed.retry,
            drop_sink: executed.drop_sink,
            _phantom: PhantomData,
        })
    }

    fn cleanup_access_start<Ctx: WorkerCtx>(
        ctx: &mut DurableWorkerCtx<Ctx>,
        cleanup: AccessStartCleanup,
    ) {
        if let Some(begin_index) = cleanup.atomic_region_registration {
            ctx.state.unregister_atomic_region_call(begin_index);
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
            // Surface a deferred request-upload failure here, at the call site, before recording the
            // `End` that references the request. The leaf oplog's commit barrier is the backstop, but
            // awaiting here turns an upload failure into a graceful error instead of a commit-time
            // panic. A no-op when the request was inline or eagerly uploaded.
            let oplog = ctx.state.oplog.clone();
            if let Err(err) = self.request_upload.wait().await.map_err(|err| {
                WorkerExecutorError::runtime(format!(
                    "failed to serialize and store durable call request: {err}"
                ))
            }) {
                if let Some(begin_index) = self.atomic_region_registration.take() {
                    ctx.state.unregister_atomic_region_call(begin_index);
                }
                return Err(err);
            }
            let host_response: HostResponse = response.clone().into();
            let response_payload = match oplog.upload_payload(&host_response).await.map_err(|err| {
                WorkerExecutorError::runtime(format!(
                    "failed to serialize and store durable call response: {err}"
                ))
            }) {
                Ok(payload) => payload,
                Err(err) => {
                    if let Some(begin_index) = self.atomic_region_registration.take() {
                        ctx.state.unregister_atomic_region_call(begin_index);
                    }
                    return Err(err);
                }
            };
            if let Some(begin_index) = self.execution_scope.atomic_region() {
                if !ctx
                    .state
                    .mark_atomic_region_has_side_effects_for(begin_index)
                {
                    if let Some(begin_index) = self.atomic_region_registration.take() {
                        ctx.state.unregister_atomic_region_call(begin_index);
                    }
                    return Err(WorkerExecutorError::runtime(format!(
                        "durable call {} completed after its atomic region {begin_index} was closed",
                        self.start_idx
                    )));
                }
            }
            let end = OplogEntry::End {
                timestamp: Timestamp::now_utc(),
                start_index: self.start_idx,
                response: Some(response_payload),
                forced_commit: false,
            };
            oplog.add(end).await;
            if let Some(begin_index) = self.atomic_region_registration.take() {
                ctx.state.unregister_atomic_region_call(begin_index);
            }
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
        self.complete_access_impl(store, get_ctx, response)
            .await
            .map_err(|source| TerminalCallError::new(source, context))
    }

    async fn complete_access_impl<T, D, Ctx>(
        mut self,
        store: &Accessor<T, D>,
        get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
        response: Pair::Resp,
    ) -> Result<Pair::Resp, WorkerExecutorError>
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
            let oplog = store.with(|mut access| get_ctx(access.data_mut()).state.oplog.clone());
            let trap_context = self.trap_context();
            let mut guard = AccessTerminalGuard::<P>::new(
                DroppedCall {
                    start_idx: self.start_idx,
                    begin_index: self.begin_index,
                    function_type: self.retry.function_type().clone(),
                    request_upload: self.request_upload.clone(),
                    atomic_region_registration: self.atomic_region_registration.take(),
                    trap_context,
                },
                self.drop_sink.clone(),
            );
            self.finished = true;
            let persist_result: Result<(), WorkerExecutorError> = if self.persisted {
                async {
                    guard
                        .call()
                        .expect("terminal guard is armed")
                        .wait_request_upload()
                        .await
                        .map_err(|err| {
                            WorkerExecutorError::runtime(format!(
                                "failed to serialize and store durable call request: {err}"
                            ))
                        })?;
                    let host_response: HostResponse = response.clone().into();
                    let response_payload =
                        oplog.upload_payload(&host_response).await.map_err(|err| {
                            WorkerExecutorError::runtime(format!(
                                "failed to serialize and store durable call response: {err}"
                            ))
                        })?;
                    let end = OplogEntry::End {
                        timestamp: Timestamp::now_utc(),
                        start_index: self.start_idx,
                        response: Some(response_payload),
                        forced_commit: false,
                    };
                    let terminal_oplog = oplog.clone();
                    let terminal = tokio::spawn(async move {
                        terminal_oplog.add(end).await;
                        Ok(())
                    });
                    guard.cleanup_after_terminal(terminal);
                    guard.wait_terminal().await?;
                    Ok(())
                }
                .await
            } else {
                Ok(())
            };

            let atomic_region = self.execution_scope.atomic_region();
            let registration = guard.atomic_region_registration();
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
                if let Some(begin_index) = registration {
                    ctx.state.unregister_atomic_region_call(begin_index);
                }
                result
            });

            if let Err(err) = persist_result {
                guard.disarm();
                return Err(err);
            }
            finish_result?;
            end_durable_function_access(store, get_ctx, function_type, begin_index, false).await?;
            guard.disarm();
            Ok(response)
        }
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
        let start_idx = self.start_idx;
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
                let payload = response.ok_or_else(|| {
                    WorkerExecutorError::unexpected_oplog_entry(
                        "End { response: Some(..) }",
                        "End { response: None }".to_string(),
                    )
                })?;
                let oplog = ctx.state.oplog.clone();
                let host_response = oplog.download_payload(payload).await.map_err(|err| {
                    WorkerExecutorError::runtime(format!("End payload cannot be downloaded: {err}"))
                })?;
                let response: Pair::Resp = host_response
                    .try_into()
                    .map_err(|err| WorkerExecutorError::unexpected_oplog_entry(Pair::FQFN, err))?;
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
                    let host_response = oplog.download_payload(payload).await.map_err(|err| {
                        WorkerExecutorError::runtime(format!(
                            "Cancelled partial payload cannot be downloaded: {err}"
                        ))
                    })?;
                    let response: Pair::Resp = host_response.try_into().map_err(|err| {
                        WorkerExecutorError::unexpected_oplog_entry(Pair::FQFN, err)
                    })?;
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
            ResolutionOutcome::Incomplete => {
                if self.retry.can_reexecute_on_incomplete_replay() {
                    // Switch the handle to live completion of the existing, committed `Start`: the
                    // caller re-runs the side effect and `complete`s, appending the missing `End`.
                    // A failure during re-execution stays grouped at this call's own retry point via
                    // the call-owned `execution_scope` (and the semantic-trap error marker).
                    self.is_live = true;
                    self.persisted = true;
                    Ok(CallReplayOutcome::Incomplete(self))
                } else {
                    // Re-executing a non-idempotent / batched / transaction write could duplicate an
                    // external side effect. Reaching here means the surrounding scope recovery did
                    // not already resolve it; fail hard, as before.
                    self.finished = true;
                    Err(WorkerExecutorError::unexpected_oplog_entry(
                        "End or Cancelled",
                        format!(
                            "incomplete non-idempotent durable call Start at {start_idx} cannot be safely re-executed"
                        ),
                    ))
                }
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
        let start_idx = self.start_idx;
        let outcome = replay_state.await_resolution_outcome(replay).await?;
        match outcome {
            ResolutionOutcome::Resolved(Resolution::Completed { response, .. }) => {
                self.finished = true;
                let payload = response.ok_or_else(|| {
                    WorkerExecutorError::unexpected_oplog_entry(
                        "End { response: Some(..) }",
                        "End { response: None }".to_string(),
                    )
                })?;
                let host_response = oplog.download_payload(payload).await.map_err(|err| {
                    WorkerExecutorError::runtime(format!("End payload cannot be downloaded: {err}"))
                })?;
                let response: Pair::Resp = host_response
                    .try_into()
                    .map_err(|err| WorkerExecutorError::unexpected_oplog_entry(Pair::FQFN, err))?;
                end_durable_function_access(store, get_ctx, function_type, begin_index, false)
                    .await?;
                Ok(CallReplayOutcome::Replayed(response))
            }
            ResolutionOutcome::Resolved(Resolution::Cancelled {
                cancelled_idx,
                partial,
            }) => {
                self.finished = true;
                if let Some(payload) = partial {
                    let host_response = oplog.download_payload(payload).await.map_err(|err| {
                        WorkerExecutorError::runtime(format!(
                            "Cancelled partial payload cannot be downloaded: {err}"
                        ))
                    })?;
                    let response: Pair::Resp = host_response.try_into().map_err(|err| {
                        WorkerExecutorError::unexpected_oplog_entry(Pair::FQFN, err)
                    })?;
                    end_durable_function_access(store, get_ctx, function_type, begin_index, false)
                        .await?;
                    Ok(CallReplayOutcome::Replayed(response))
                } else {
                    Err(WorkerExecutorError::unexpected_oplog_entry(
                        "End or Cancelled { partial: Some(..) }",
                        format!("Cancelled without partial at {cancelled_idx}"),
                    ))
                }
            }
            ResolutionOutcome::Incomplete => {
                if self.retry.can_reexecute_on_incomplete_replay() {
                    self.is_live = true;
                    self.persisted = true;
                    Ok(CallReplayOutcome::Incomplete(self))
                } else {
                    self.finished = true;
                    Err(WorkerExecutorError::unexpected_oplog_entry(
                        "End or Cancelled",
                        format!(
                            "incomplete non-idempotent durable call Start at {start_idx} cannot be safely re-executed"
                        ),
                    ))
                }
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
                    atomic_region_registration: self.atomic_region_registration.take(),
                    trap_context,
                };
                // As in `complete`: surface a deferred request-upload failure at the call site before
                // recording the `Cancelled` that references the request. A no-op when the request was
                // inline or eagerly uploaded.
                if let Err(err) = dropped_call.wait_request_upload().await {
                    if let Some(begin_index) = dropped_call.atomic_region_registration() {
                        ctx.state.unregister_atomic_region_call(begin_index);
                    }
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
                                if let Some(begin_index) = dropped_call.atomic_region_registration()
                                {
                                    ctx.state.unregister_atomic_region_call(begin_index);
                                }
                                return Err(err);
                            }
                        }
                    }
                    None => None,
                };
                dropped_call
                    .append_cancelled_with_oplog(oplog, partial_payload)
                    .await?;
                if let Some(begin_index) = dropped_call.atomic_region_registration() {
                    ctx.state.unregister_atomic_region_call(begin_index);
                }
                ctx.end_durable_function(self.retry.function_type(), self.begin_index, false)
                    .await?;
            }
        } else {
            let replay = self
                .replay
                .take()
                .expect("cancel() in replay called on a live handle");
            let resolution = ctx.state.replay_state.await_resolution(replay).await?;
            if let Resolution::Completed { end_idx, .. } = resolution {
                return Err(WorkerExecutorError::unexpected_oplog_entry(
                    "Cancelled",
                    format!("End at {end_idx}"),
                ));
            }
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
                        atomic_region_registration: self.atomic_region_registration.take(),
                        trap_context,
                    },
                    self.drop_sink.clone(),
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
                    guard.cleanup_after_terminal(terminal);
                    guard.wait_terminal().await
                }
                .await;
                let registration = guard.atomic_region_registration();
                store.with(|mut access| {
                    let ctx = get_ctx(access.data_mut());
                    if let Some(begin_index) = registration {
                        ctx.state.unregister_atomic_region_call(begin_index);
                    }
                });
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
            if let Resolution::Completed { end_idx, .. } = resolution {
                return Err(WorkerExecutorError::unexpected_oplog_entry(
                    "Cancelled",
                    format!("End at {end_idx}"),
                ));
            }
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

async fn finish_span_access<T, D, Ctx>(
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
        crate::get_oplog_entry!(replay_state, OplogEntry::FinishSpan)?;
    }

    store.with(|mut access| {
        let ctx = get_ctx(access.data_mut());
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
    let replay_events = replay_state.take_new_replay_events().await;
    for event in replay_events {
        match event {
            crate::durable_host::replay_state::ReplayEvent::ForkReplayed { new_phantom_id } => {
                store.with(|mut access| {
                    let ctx = get_ctx(access.data_mut());
                    ctx.state.current_phantom_id = Some(new_phantom_id);
                });
            }
            crate::durable_host::replay_state::ReplayEvent::UpdateReplayed { .. } => {
                return Err(WorkerExecutorError::runtime(
                    "p3 accessor durable call path cannot yet apply replayed component updates",
                ));
            }
            crate::durable_host::replay_state::ReplayEvent::ReplayFinished => {
                let has_pending_update = store.with(|mut access| {
                    let ctx = get_ctx(access.data_mut());
                    ctx.state
                        .pending_update
                        .try_lock()
                        .map(|pending| pending.is_some())
                        .map_err(|_| {
                            WorkerExecutorError::runtime(
                                "p3 accessor durable call path cannot inspect pending component update state",
                            )
                        })
                })?;
                if has_pending_update {
                    return Err(WorkerExecutorError::runtime(
                        "p3 accessor durable call path cannot yet finalize replayed component updates",
                    ));
                }
            }
        }
    }
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
        let execution_scope = self.execution_scope.finish(start_idx);
        let atomic_region_registration = if persisted {
            if let Some(begin_index) = execution_scope.atomic_region() {
                if !ctx.state.register_atomic_region_call(begin_index) {
                    return Err(WorkerExecutorError::runtime(format!(
                        "durable call {start_idx} started in atomic region {begin_index}, but the region is not open"
                    )));
                }
                Some(begin_index)
            } else {
                None
            }
        } else {
            None
        };
        Ok(CallHandle {
            start_idx,
            begin_index: self.begin_index,
            is_live: true,
            persisted,
            request_upload,
            replay: None,
            finished: false,
            execution_scope,
            atomic_region_registration,
            retry: self.retry,
            drop_sink: self.drop_sink,
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
        let execution_scope = self.execution_scope.finish(start_idx);
        Ok(CallHandle {
            start_idx,
            begin_index: self.begin_index,
            is_live: false,
            persisted: false,
            request_upload: PendingUpload::already_durable(),
            replay: Some(replay),
            finished: false,
            execution_scope,
            atomic_region_registration: None,
            retry: self.retry,
            drop_sink: self.drop_sink,
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
                        atomic_region_registration: self.atomic_region_registration,
                        trap_context,
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
            // A replay handle must never enqueue a live cancellation; just note the anomaly.
            tracing::warn!(
                "replay durable call handle for Start {} dropped without finishing",
                self.start_idx
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use golem_common::model::oplog::host_functions;
    use golem_common::model::{NamedRetryPolicy, Predicate, RetryPolicy, RetryPolicyState};
    use std::time::Duration;
    use test_r::test;
    use tokio::sync::mpsc;

    fn idx(n: u64) -> OplogIndex {
        OplogIndex::from_u64(n)
    }

    fn durable_execution_state() -> DurableExecutionState {
        DurableExecutionState {
            is_live: true,
            persistence_level: PersistenceLevel::Smart,
            snapshotting_mode: None,
            assume_idempotence: true,
            max_in_function_retry_delay: Duration::from_secs(20),
        }
    }

    fn completed(end_idx: u64) -> Resolution {
        Resolution::Completed {
            end_idx: idx(end_idx),
            response: None,
            forced_commit: false,
        }
    }

    // ---- ConcurrentReplayResolver ----

    #[test]
    fn resolver_out_of_order_completion() {
        // [S1, S2, E2, E1]: claim both, then resolve E2 before E1.
        let mut resolver = ConcurrentReplayResolver::default();
        let mut rx1 = resolver.register(idx(1));
        let mut rx2 = resolver.register(idx(2));
        assert!(rx1.try_recv().is_err());
        assert!(rx2.try_recv().is_err());

        assert!(resolver.resolve_if_pending(idx(2), completed(3)));
        match rx2.try_recv() {
            Ok(ResolutionOutcome::Resolved(Resolution::Completed { end_idx, .. })) => {
                assert_eq!(end_idx, idx(3))
            }
            other => panic!("unexpected resolution for h2: {other:?}"),
        }
        assert!(rx1.try_recv().is_err());

        assert!(resolver.resolve_if_pending(idx(1), completed(4)));
        match rx1.try_recv() {
            Ok(ResolutionOutcome::Resolved(Resolution::Completed { end_idx, .. })) => {
                assert_eq!(end_idx, idx(4))
            }
            other => panic!("unexpected resolution for h1: {other:?}"),
        }
    }

    #[test]
    fn resolver_cancelled() {
        // [S1, Cancelled1]
        let mut resolver = ConcurrentReplayResolver::default();
        let mut rx = resolver.register(idx(1));
        assert!(resolver.resolve_if_pending(
            idx(1),
            Resolution::Cancelled {
                cancelled_idx: idx(2),
                partial: None,
            },
        ));
        match rx.try_recv() {
            Ok(ResolutionOutcome::Resolved(Resolution::Cancelled { cancelled_idx, .. })) => {
                assert_eq!(cancelled_idx, idx(2))
            }
            other => panic!("unexpected resolution: {other:?}"),
        }
    }

    #[test]
    fn resolver_resolve_before_register_buffers() {
        let mut resolver = ConcurrentReplayResolver::default();
        resolver.resolve(idx(1), completed(2));
        assert!(!resolver.is_pending(idx(1)));
        let mut rx = resolver.register(idx(1));
        match rx.try_recv() {
            Ok(ResolutionOutcome::Resolved(Resolution::Completed { end_idx, .. })) => {
                assert_eq!(end_idx, idx(2))
            }
            other => panic!("expected pre-resolved receiver, got {other:?}"),
        }
    }

    #[test]
    fn resolver_missing_pending_is_dropped_not_buffered() {
        let mut resolver = ConcurrentReplayResolver::default();
        // No registration: resolve_if_pending must not buffer (this is the unregistered-End case,
        // e.g. the guest-facing manual durability pair).
        assert!(!resolver.resolve_if_pending(idx(1), completed(2)));
        let mut rx = resolver.register(idx(1));
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn resolver_fail_all_pending_incomplete_wakes_everyone() {
        // End-of-replay must wake every still-suspended awaiter as Incomplete, not leave them
        // hanging. A resolved awaiter is already gone and is unaffected.
        let mut resolver = ConcurrentReplayResolver::default();
        let mut rx1 = resolver.register(idx(1));
        let mut rx2 = resolver.register(idx(2));
        assert!(resolver.resolve_if_pending(idx(1), completed(3)));

        resolver.fail_all_pending_incomplete();

        match rx1.try_recv() {
            Ok(ResolutionOutcome::Resolved(Resolution::Completed { end_idx, .. })) => {
                assert_eq!(end_idx, idx(3))
            }
            other => panic!("rx1 should already be resolved: {other:?}"),
        }
        match rx2.try_recv() {
            Ok(ResolutionOutcome::Incomplete) => {}
            other => panic!("rx2 should be Incomplete: {other:?}"),
        }
        assert!(!resolver.is_pending(idx(2)));
    }

    #[test]
    fn resolver_duplicate_resolution_is_ignored() {
        let mut resolver = ConcurrentReplayResolver::default();
        let mut rx = resolver.register(idx(1));
        assert!(resolver.resolve_if_pending(idx(1), completed(2)));
        // Second resolution: no longer pending.
        assert!(!resolver.resolve_if_pending(idx(1), completed(3)));
        match rx.try_recv() {
            Ok(ResolutionOutcome::Resolved(Resolution::Completed { end_idx, .. })) => {
                assert_eq!(end_idx, idx(2))
            }
            other => panic!("unexpected resolution: {other:?}"),
        }
    }

    // ---- CallHandle drop policy ----

    fn live_unfinished_handle<P: DropPolicy>(
        start_idx: OplogIndex,
        sink: mpsc::UnboundedSender<DropEvent>,
    ) -> CallHandle<host_functions::MonotonicClockNow, P> {
        live_unfinished_handle_with_atomic_region(start_idx, None, sink)
    }

    fn live_unfinished_handle_with_atomic_region<P: DropPolicy>(
        start_idx: OplogIndex,
        atomic_region_registration: Option<OplogIndex>,
        sink: mpsc::UnboundedSender<DropEvent>,
    ) -> CallHandle<host_functions::MonotonicClockNow, P> {
        use crate::durable_host::durability::DurableExecutionState;
        use std::time::Duration;
        let durable_execution_state = DurableExecutionState {
            is_live: true,
            persistence_level: PersistenceLevel::Smart,
            snapshotting_mode: None,
            assume_idempotence: false,
            max_in_function_retry_delay: Duration::ZERO,
        };
        CallHandle {
            start_idx,
            begin_index: start_idx,
            is_live: true,
            persisted: true,
            request_upload: PendingUpload::already_durable(),
            replay: None,
            finished: false,
            execution_scope: CallExecutionScope {
                retry_from: start_idx,
                durable_scope: None,
                atomic_region: None,
                persistence_level: PersistenceLevel::Smart,
            },
            atomic_region_registration,
            retry: InFunctionRetryController::new(
                DurableFunctionType::ReadLocal,
                durable_execution_state,
                "test:monotonic_clock::now",
            ),
            drop_sink: Some(sink),
            _phantom: PhantomData,
        }
    }

    /// A synthetic, already-finished handle carrying an arbitrary [`CallExecutionScope`]. Used by the
    /// Seam-2 terminal-failure tests to drive `trap_context()` (the call-owned classification a
    /// terminal-step failure attaches) for a call initiated in a specific scope, without standing up
    /// a full `DurableWorkerCtx`. `finished` is set so dropping the handle is a no-op (no drop
    /// event), and no `drop_sink` is attached. `start_idx`/`begin_index` are set from
    /// `scope.retry_from` only so the struct is well-formed; the tests read nothing but the scope.
    fn synthetic_finished_handle_with_scope<P: DropPolicy>(
        scope: CallExecutionScope,
    ) -> CallHandle<host_functions::MonotonicClockNow, P> {
        use crate::durable_host::durability::DurableExecutionState;
        use std::time::Duration;
        let durable_execution_state = DurableExecutionState {
            is_live: true,
            persistence_level: PersistenceLevel::Smart,
            snapshotting_mode: None,
            assume_idempotence: false,
            max_in_function_retry_delay: Duration::ZERO,
        };
        let start_idx = scope.retry_from;
        let atomic_region_registration = scope.atomic_region;
        CallHandle {
            start_idx,
            begin_index: start_idx,
            is_live: true,
            persisted: true,
            request_upload: PendingUpload::already_durable(),
            replay: None,
            finished: true,
            execution_scope: scope,
            atomic_region_registration,
            retry: InFunctionRetryController::new(
                DurableFunctionType::ReadLocal,
                durable_execution_state,
                "test:monotonic_clock::now",
            ),
            drop_sink: None,
            _phantom: PhantomData,
        }
    }

    #[test]
    fn drop_cancellable_unfinished_enqueues_exactly_one_cancelled() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        {
            let _handle = live_unfinished_handle::<Cancellable>(idx(5), tx);
        }
        match rx.try_recv() {
            Ok(DropEvent::UnfinishedCancellable { call }) => {
                assert_eq!(call.start_idx(), idx(5));
                assert_eq!(call.atomic_region_registration(), None);
            }
            other => panic!("expected one UnfinishedCancellable, got {other:?}"),
        }
        assert!(rx.try_recv().is_err(), "expected exactly one drop event");
    }

    #[test]
    fn drop_cancellable_unfinished_carries_call_owned_cancellation_state() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        {
            let _handle =
                live_unfinished_handle_with_atomic_region::<Cancellable>(idx(5), Some(idx(2)), tx);
        }
        match rx.try_recv() {
            Ok(DropEvent::UnfinishedCancellable { call }) => {
                assert_eq!(call.start_idx(), idx(5));
                assert_eq!(call.atomic_region_registration(), Some(idx(2)));
            }
            other => panic!("expected one UnfinishedCancellable, got {other:?}"),
        }
    }

    #[test]
    fn drop_not_cancellable_unfinished_signals_policy_violation() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        {
            let _handle = live_unfinished_handle::<NotCancellable>(idx(7), tx);
        }
        match rx.try_recv() {
            Ok(DropEvent::UnfinishedNotCancellable { call }) => {
                assert_eq!(call.start_idx(), idx(7))
            }
            other => panic!("expected UnfinishedNotCancellable, got {other:?}"),
        }
    }

    #[test]
    fn drop_after_finish_is_noop() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        {
            let mut handle = live_unfinished_handle::<Cancellable>(idx(9), tx);
            handle.finished = true;
        }
        assert!(
            rx.try_recv().is_err(),
            "finished handle must not emit a drop event"
        );
    }

    #[test]
    fn seam2_unfinished_cancellable_drops_emit_independent_call_snapshots() {
        let (tx, mut rx) = mpsc::unbounded_channel();

        let handle_a = live_unfinished_handle_with_atomic_region::<Cancellable>(
            idx(5),
            Some(idx(2)),
            tx.clone(),
        );
        let handle_b =
            live_unfinished_handle_with_atomic_region::<Cancellable>(idx(9), Some(idx(3)), tx);

        drop(handle_a);
        drop(handle_b);

        match rx.try_recv() {
            Ok(DropEvent::UnfinishedCancellable { call }) => {
                assert_eq!(call.start_idx(), idx(5));
                assert_eq!(call.atomic_region_registration(), Some(idx(2)));
            }
            other => panic!("expected first cancellable drop snapshot, got {other:?}"),
        }
        match rx.try_recv() {
            Ok(DropEvent::UnfinishedCancellable { call }) => {
                assert_eq!(call.start_idx(), idx(9));
                assert_eq!(call.atomic_region_registration(), Some(idx(3)));
            }
            other => panic!("expected second cancellable drop snapshot, got {other:?}"),
        }
        assert!(rx.try_recv().is_err(), "expected exactly two drop events");
    }

    #[test]
    fn abandon_for_trap_suppresses_unfinished_drop() {
        // A trap-abandoned NotCancellable handle must NOT trip the unfinished-drop policy: the
        // host-call Start is intentionally left incomplete for replay/retry, not cancelled.
        let (tx, mut rx) = mpsc::unbounded_channel();
        {
            let mut handle = live_unfinished_handle::<NotCancellable>(idx(11), tx);
            handle.abandon_for_trap();
        }
        assert!(
            rx.try_recv().is_err(),
            "abandon_for_trap must not emit a drop event"
        );
    }

    // ---- call-scoped retry host ----

    struct RetryHostProbe {
        current_retry_point: OplogIndex,
        appended_retry_from: Vec<OplogIndex>,
        appended_inside_atomic_region: Vec<bool>,
        /// Atomic region begin indices the inner host reports as having recorded side effects.
        regions_with_side_effects: Vec<OplogIndex>,
        /// Ambient outermost-region side-effect state, used by the unscoped fallback path.
        outermost_has_side_effects: bool,
    }

    impl RetryHostProbe {
        fn new(current_retry_point: OplogIndex) -> Self {
            Self {
                current_retry_point,
                appended_retry_from: Vec::new(),
                appended_inside_atomic_region: Vec::new(),
                regions_with_side_effects: Vec::new(),
                outermost_has_side_effects: false,
            }
        }
    }

    #[async_trait]
    impl InFunctionRetryHost for RetryHostProbe {
        fn in_atomic_region(&self) -> bool {
            false
        }

        fn current_retry_point(&self) -> OplogIndex {
            self.current_retry_point
        }

        async fn named_retry_policies(&mut self) -> Vec<golem_common::model::NamedRetryPolicy> {
            vec![NamedRetryPolicy {
                name: "test".to_string(),
                priority: 0,
                predicate: Predicate::True,
                policy: RetryPolicy::CountBox {
                    max_retries: 1,
                    inner: Box::new(RetryPolicy::Periodic(Duration::from_millis(1))),
                },
            }]
        }

        async fn current_retry_state_for(
            &self,
            retry_from: OplogIndex,
        ) -> Option<golem_common::model::RetryPolicyState> {
            if retry_from == self.current_retry_point {
                Some(RetryPolicyState::CountBox {
                    attempts: 1,
                    inner: Box::new(RetryPolicyState::Counter(1)),
                })
            } else {
                None
            }
        }

        fn durable_execution_state(&self) -> DurableExecutionState {
            durable_execution_state()
        }

        fn atomic_region_has_side_effects_for(&self, begin_index: OplogIndex) -> bool {
            self.regions_with_side_effects.contains(&begin_index)
        }

        fn retry_context_atomic_region_had_side_effects(&self) -> bool {
            self.outermost_has_side_effects
        }

        async fn append_retry_error_entry(
            &mut self,
            retry_from: OplogIndex,
            inside_atomic_region: bool,
            _retry_policy_state: Option<golem_common::model::RetryPolicyState>,
        ) {
            self.appended_retry_from.push(retry_from);
            self.appended_inside_atomic_region
                .push(inside_atomic_region);
        }
    }

    #[test]
    fn scoped_retry_host_uses_call_retry_point_not_inner_current() {
        let mut inner = RetryHostProbe::new(idx(99));
        let scope = CallExecutionScope {
            retry_from: idx(42),
            durable_scope: Some(idx(40)),
            atomic_region: None,
            persistence_level: PersistenceLevel::PersistRemoteSideEffects,
        };

        let retry_host = ScopedRetryHost::new(&mut inner, &scope);

        assert!(!retry_host.in_atomic_region());
        assert_eq!(retry_host.current_retry_point(), idx(42));
        assert_eq!(
            retry_host.durable_execution_state().persistence_level,
            PersistenceLevel::PersistRemoteSideEffects
        );
    }

    #[test]
    fn scoped_retry_host_uses_call_atomic_region_as_retry_point() {
        let mut inner = RetryHostProbe::new(idx(99));
        let scope = CallExecutionScope {
            retry_from: idx(42),
            durable_scope: Some(idx(40)),
            atomic_region: Some(idx(7)),
            persistence_level: PersistenceLevel::Smart,
        };

        let retry_host = ScopedRetryHost::new(&mut inner, &scope);

        assert!(retry_host.in_atomic_region());
        assert_eq!(retry_host.current_retry_point(), idx(7));
    }

    #[test]
    async fn scoped_retry_host_trap_retry_uses_call_retry_point() {
        let mut inner = RetryHostProbe::new(idx(99));
        let scope = CallExecutionScope {
            retry_from: idx(42),
            durable_scope: Some(idx(40)),
            atomic_region: None,
            persistence_level: PersistenceLevel::Smart,
        };
        let mut retry_host = ScopedRetryHost::new(&mut inner, &scope);

        let outcome = try_trigger_host_trap_retry(
            &mut retry_host,
            Error::msg("transient failure"),
            RetryProperties::new(),
        )
        .await;

        assert!(
            outcome.is_err(),
            "empty state at the call-owned retry point should allow one retry; using the inner/global retry point would be exhausted"
        );
    }

    #[test]
    async fn seam2_overlapping_semantic_traps_carry_independent_retry_points() {
        let mut inner = RetryHostProbe::new(idx(99));
        let scope_a = CallExecutionScope {
            retry_from: idx(42),
            durable_scope: Some(idx(40)),
            atomic_region: None,
            persistence_level: PersistenceLevel::Smart,
        };
        let scope_b = CallExecutionScope {
            retry_from: idx(77),
            durable_scope: Some(idx(70)),
            atomic_region: None,
            persistence_level: PersistenceLevel::Smart,
        };

        let error_a = {
            let mut retry_host = ScopedRetryHost::new(&mut inner, &scope_a);
            try_trigger_host_trap_retry(
                &mut retry_host,
                Error::msg("transient failure a"),
                RetryProperties::new(),
            )
            .await
            .expect_err("call A should trap for oplog-level retry")
        };

        let error_b = {
            let mut retry_host = ScopedRetryHost::new(&mut inner, &scope_b);
            try_trigger_host_trap_retry(
                &mut retry_host,
                Error::msg("transient failure b"),
                RetryProperties::new(),
            )
            .await
            .expect_err("call B should trap for oplog-level retry")
        };

        let override_a =
            crate::durable_host::durability::find_semantic_trap_retry_override(&error_a)
                .expect("call A trap must carry a semantic retry override");
        let override_b =
            crate::durable_host::durability::find_semantic_trap_retry_override(&error_b)
                .expect("call B trap must carry a semantic retry override");

        assert_eq!(override_a.retry_from, idx(42));
        assert_eq!(override_b.retry_from, idx(77));
    }

    #[test]
    async fn seam2_overlapping_atomic_region_traps_use_initiation_membership() {
        let mut inner = RetryHostProbe::new(idx(99));
        let scope_a = CallExecutionScope {
            retry_from: idx(42),
            durable_scope: Some(idx(40)),
            atomic_region: Some(idx(7)),
            persistence_level: PersistenceLevel::Smart,
        };
        let scope_b = CallExecutionScope {
            retry_from: idx(77),
            durable_scope: Some(idx(70)),
            atomic_region: Some(idx(8)),
            persistence_level: PersistenceLevel::Smart,
        };

        let error_a = {
            let mut retry_host = ScopedRetryHost::new(&mut inner, &scope_a);
            try_trigger_host_trap_retry(
                &mut retry_host,
                Error::msg("transient failure a"),
                RetryProperties::new(),
            )
            .await
            .expect_err("call A should trap for oplog-level retry")
        };

        let error_b = {
            let mut retry_host = ScopedRetryHost::new(&mut inner, &scope_b);
            try_trigger_host_trap_retry(
                &mut retry_host,
                Error::msg("transient failure b"),
                RetryProperties::new(),
            )
            .await
            .expect_err("call B should trap for oplog-level retry")
        };

        let override_a =
            crate::durable_host::durability::find_semantic_trap_retry_override(&error_a)
                .expect("call A trap must carry a semantic retry override");
        let override_b =
            crate::durable_host::durability::find_semantic_trap_retry_override(&error_b)
                .expect("call B trap must carry a semantic retry override");

        assert_eq!(override_a.retry_from, idx(7));
        assert_eq!(override_b.retry_from, idx(8));
    }

    // ---- Seam-2 terminal-step failures ----

    /// Classifies a terminal-step failure the way the invocation loop does, with a deliberately
    /// *hostile* ambient fallback (a retry point and atomic-region membership belonging to no real
    /// call). A call-owned [`DurableCallTrapContext`] marker carried by the error must override both
    /// fallbacks; if the terminal path lost the marker, the classifier would silently adopt these
    /// ambient values instead — the exact §5.7.3 misclassification we are guarding against.
    fn classify_with_hostile_ambient(error: &anyhow::Error) -> crate::model::TrapType {
        crate::model::TrapType::from_error::<crate::workerctx::default::Context>(
            error,
            idx(99),
            true,
            false,
            golem_common::model::agent::AgentMode::Durable,
        )
    }

    #[test]
    fn seam2_terminal_failure_carries_call_owned_trap_context() {
        // A failure escaping a *terminal* durable-call step (`complete` / `complete_access` /
        // `cancel` / the dropped-call drain) is wrapped in a `TerminalCallError` built from the
        // call's own execution scope. Classifying it must group the retry against the call's own
        // scope and use the call's own atomic-region membership, never the ambient worker state.
        let scope = CallExecutionScope {
            retry_from: idx(42),
            durable_scope: Some(idx(40)),
            atomic_region: None,
            persistence_level: PersistenceLevel::Smart,
        };
        let handle = synthetic_finished_handle_with_scope::<Cancellable>(scope);

        let error: anyhow::Error = TerminalCallError::new(
            WorkerExecutorError::runtime("terminal step failure"),
            handle.trap_context(),
        )
        .into();

        match classify_with_hostile_ambient(&error) {
            crate::model::TrapType::Error {
                retry_from,
                in_atomic_region,
                ..
            } => {
                // Call-owned (idx 42, non-atomic) wins over hostile ambient (idx 99, atomic).
                assert_eq!(retry_from, idx(42));
                assert!(!in_atomic_region);
            }
            other => panic!("expected TrapType::Error, got {other:?}"),
        }
    }

    #[test]
    fn seam2_overlapping_terminal_failures_carry_independent_trap_contexts() {
        // Two durable calls overlap in flight and share the same ambient worker state. Each one's
        // terminal-step failure must still be classified against its OWN retry point and
        // atomic-region membership. Before §5.7.3 these escaped unmarked and fell back to that shared
        // ambient state, so an overlapping sibling could clobber the grouping.
        let scope_a = CallExecutionScope {
            retry_from: idx(42),
            durable_scope: Some(idx(40)),
            atomic_region: Some(idx(7)),
            persistence_level: PersistenceLevel::Smart,
        };
        let scope_b = CallExecutionScope {
            retry_from: idx(77),
            durable_scope: Some(idx(70)),
            atomic_region: None,
            persistence_level: PersistenceLevel::Smart,
        };
        let handle_a = synthetic_finished_handle_with_scope::<Cancellable>(scope_a);
        let handle_b = synthetic_finished_handle_with_scope::<Cancellable>(scope_b);

        let error_a: anyhow::Error =
            TerminalCallError::new(WorkerExecutorError::runtime("a"), handle_a.trap_context())
                .into();
        let error_b: anyhow::Error =
            TerminalCallError::new(WorkerExecutorError::runtime("b"), handle_b.trap_context())
                .into();

        // Both classified against the SAME hostile ambient state, yet each keeps its own grouping.
        match classify_with_hostile_ambient(&error_a) {
            crate::model::TrapType::Error {
                retry_from,
                in_atomic_region,
                ..
            } => {
                // Call A was initiated inside an atomic region: the whole region retries from its
                // begin index (idx 7) and membership is recorded.
                assert_eq!(retry_from, idx(7));
                assert!(in_atomic_region);
            }
            other => panic!("expected TrapType::Error for call A, got {other:?}"),
        }
        match classify_with_hostile_ambient(&error_b) {
            crate::model::TrapType::Error {
                retry_from,
                in_atomic_region,
                ..
            } => {
                // Call B was not in an atomic region: it retries from its own scope (idx 77).
                assert_eq!(retry_from, idx(77));
                assert!(!in_atomic_region);
            }
            other => panic!("expected TrapType::Error for call B, got {other:?}"),
        }
    }

    #[test]
    fn seam2_dropped_call_drain_failure_uses_dropped_call_trap_context() {
        // A cancellation-drain failure (deferred request upload / terminal recorder join) is driven
        // by whichever later host call happens to run the drain, but it must be classified with the
        // *dropped* call's captured context, not the drainer's ambient state. `DroppedCall::trap_context`
        // carries that captured classification; the drain wraps failures with it via
        // `TerminalCallError`.
        // An atomic dropped call: its own context (idx 3, atomic) must win over the hostile ambient.
        let dropped_atomic = DroppedCall {
            start_idx: idx(5),
            begin_index: idx(4),
            function_type: DurableFunctionType::ReadRemote,
            request_upload: PendingUpload::already_durable(),
            atomic_region_registration: Some(idx(3)),
            trap_context: DurableCallTrapContext {
                retry_from: idx(3),
                in_atomic_region: true,
            },
        };
        // A non-atomic dropped call: its membership (false) must win over the hostile ambient's
        // `in_atomic_region = true`, so the membership assertion is independent of the retry point.
        let dropped_non_atomic = DroppedCall {
            start_idx: idx(8),
            begin_index: idx(8),
            function_type: DurableFunctionType::ReadRemote,
            request_upload: PendingUpload::already_durable(),
            atomic_region_registration: None,
            trap_context: DurableCallTrapContext {
                retry_from: idx(8),
                in_atomic_region: false,
            },
        };

        for (dropped, expected_retry, expected_atomic) in [
            (dropped_atomic, idx(3), true),
            (dropped_non_atomic, idx(8), false),
        ] {
            let error: anyhow::Error = TerminalCallError::new(
                WorkerExecutorError::runtime("cancellation drain failed"),
                dropped.trap_context(),
            )
            .into();

            // `classify_with_hostile_ambient` supplies idx(99)/atomic-membership-true as the
            // drainer's ambient state; the dropped call's own context must win.
            match classify_with_hostile_ambient(&error) {
                crate::model::TrapType::Error {
                    retry_from,
                    in_atomic_region,
                    ..
                } => {
                    assert_eq!(retry_from, expected_retry);
                    assert_eq!(in_atomic_region, expected_atomic);
                }
                other => panic!("expected TrapType::Error, got {other:?}"),
            }
        }
    }

    // ---- call-owned execution scope ----

    #[test]
    fn begun_execution_scope_uses_parent_scope_as_retry_from() {
        let begun = BegunCallExecutionScope {
            parent_start_index: Some(idx(10)),
            atomic_region: Some(idx(2)),
            persistence_level: PersistenceLevel::PersistNothing,
        };

        let scope = begun.finish(idx(11));

        assert_eq!(scope.retry_from, idx(10));
        assert_eq!(scope.durable_scope, Some(idx(10)));
        assert_eq!(scope.atomic_region, Some(idx(2)));
        assert_eq!(scope.persistence_level, PersistenceLevel::PersistNothing);
    }

    #[test]
    fn begun_execution_scope_uses_call_start_as_retry_from_when_unscoped() {
        let begun = BegunCallExecutionScope {
            parent_start_index: None,
            atomic_region: None,
            persistence_level: PersistenceLevel::Smart,
        };

        let scope = begun.finish(idx(12));

        assert_eq!(scope.retry_from, idx(12));
        assert_eq!(scope.durable_scope, None);
        assert_eq!(scope.atomic_region, None);
        assert_eq!(scope.persistence_level, PersistenceLevel::Smart);
    }

    #[test]
    fn call_execution_scope_owns_call_retry_point() {
        let scope = CallExecutionScope {
            retry_from: idx(42),
            durable_scope: Some(idx(40)),
            atomic_region: None,
            persistence_level: PersistenceLevel::Smart,
        };

        assert_eq!(scope.retry_from, idx(42));
    }

    // ---- function-type re-execution policy ----

    #[test]
    fn can_reexecute_matches_internal_retry_eligibility() {
        use crate::durable_host::durability::{DurableExecutionState, InFunctionRetryController};

        fn controller(
            ft: DurableFunctionType,
            assume_idempotence: bool,
        ) -> InFunctionRetryController {
            InFunctionRetryController::new(
                ft,
                DurableExecutionState {
                    is_live: true,
                    persistence_level: PersistenceLevel::Smart,
                    snapshotting_mode: None,
                    assume_idempotence,
                    max_in_function_retry_delay: Duration::ZERO,
                },
                "test:fn",
            )
        }

        // Reads and local/idempotent writes are safe to re-execute on an incomplete Start.
        assert!(
            controller(DurableFunctionType::ReadLocal, false).can_reexecute_on_incomplete_replay()
        );
        assert!(
            controller(DurableFunctionType::ReadRemote, false).can_reexecute_on_incomplete_replay()
        );
        assert!(
            controller(DurableFunctionType::WriteLocal, false).can_reexecute_on_incomplete_replay()
        );
        assert!(
            controller(DurableFunctionType::WriteRemote, true).can_reexecute_on_incomplete_replay()
        );

        // Non-idempotent / batched / transaction writes are not — neither the `None` (scope-opening)
        // nor the `Some(begin_index)` (in-scope host call) variants. The `Some(..)` variants are the
        // ones a migrated batched/transaction host call carries, so a lone committed host-call
        // `Start` for them must hard-error on incomplete replay rather than re-execute and risk
        // duplicating an external write (`CallHandle::replay` Incomplete arm).
        assert!(
            !controller(DurableFunctionType::WriteRemote, false)
                .can_reexecute_on_incomplete_replay()
        );
        assert!(
            !controller(DurableFunctionType::WriteRemoteBatched(None), true)
                .can_reexecute_on_incomplete_replay()
        );
        assert!(
            !controller(DurableFunctionType::WriteRemoteBatched(Some(idx(7))), true)
                .can_reexecute_on_incomplete_replay()
        );
        assert!(
            !controller(DurableFunctionType::WriteRemoteTransaction(None), true)
                .can_reexecute_on_incomplete_replay()
        );
        assert!(
            !controller(
                DurableFunctionType::WriteRemoteTransaction(Some(idx(9))),
                true
            )
            .can_reexecute_on_incomplete_replay()
        );
    }
}
