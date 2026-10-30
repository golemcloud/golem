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
use crate::durable_host::replay_to_live::store_is_live;
use crate::durable_host::tail_work::TailActivity;
use futures::FutureExt;
use futures::future::{BoxFuture, Shared};
use golem_common::model::entity::{InvocationExecutionMode, OwnerRuntime};

/// Store-independent inputs captured before a synchronous HTTP resource/terminal drop.
#[derive(Clone, Debug)]
pub(crate) struct HttpSpanCleanupRecorder {
    oplog: Arc<dyn Oplog>,
    replay: ReplayState,
    entity_execution_mode: Option<InvocationExecutionMode>,
    local_live_tail: Arc<std::sync::atomic::AtomicBool>,
    persisted: bool,
    parent: Option<OplogIndex>,
    send_start_index: OplogIndex,
    sink: UnboundedSender<DropEvent>,
}

impl HttpSpanCleanupRecorder {
    pub(crate) fn new<Ctx: WorkerCtx>(
        ctx: &DurableWorkerCtx<Ctx>,
        send_start_index: OplogIndex,
        persisted: bool,
    ) -> Self {
        Self {
            oplog: ctx.state.oplog.clone(),
            replay: ctx.state.replay_state.clone(),
            entity_execution_mode: ctx.state.entity_execution_mode,
            local_live_tail: ctx.state.local_live_tail(),
            persisted: persisted && !ctx.state.snapshotting_mode,
            parent: ctx.entity_parent_start_index(),
            send_start_index,
            sink: ctx
                .state
                .dropped_call_event_sender()
                .expect("worker cleanup queue"),
        }
    }

    pub(crate) fn for_start(mut self, send_start_index: OplogIndex) -> Self {
        self.send_start_index = send_start_index;
        self
    }

    pub(crate) fn record(self, finished: golem_common::model::oplog::SpanFinished) {
        // Sample this Store at abandonment, then retain that decision across delayed drains.
        let live = store_is_live(
            self.entity_execution_mode,
            self.local_live_tail
                .load(std::sync::atomic::Ordering::Acquire),
            self.replay.is_live_published(),
        );
        let recorder = self.clone();
        let closed = finished.clone();
        let progress = async move {
            if live && recorder.persisted {
                if let Some(
                    OplogEntry::End {
                        span_finished: Some(terminal),
                        ..
                    }
                    | OplogEntry::Cancelled {
                        span_finished: Some(terminal),
                        ..
                    },
                ) = recorder
                    .replay
                    .visible_terminal_entry(recorder.send_start_index)
                    .await?
                    && terminal.span_id == closed.span_id
                {
                    return Ok(());
                }
                recorder
                    .oplog
                    .enqueue_add(OplogEntry::SpanClosed {
                        timestamp: closed.finished_at,
                        entity_parent_start_index: recorder.parent,
                        resource_start_index: Some(recorder.send_start_index),
                        span_finished: closed,
                    })
                    .await?;
            }
            Ok(())
        }
        .boxed()
        .shared();
        let sink = self.sink.clone();
        let _ = sink.send(DropEvent::FinishP3HttpSpan {
            cleanup: Box::new(HttpSpanCleanup { finished, progress }),
        });
    }
}

#[derive(Debug)]
pub struct HttpSpanCleanup {
    finished: golem_common::model::oplog::SpanFinished,
    // A cancelled drain drops only its clone; the queue retains the metadata read and receipt.
    pub(crate) progress: Shared<BoxFuture<'static, Result<(), WorkerExecutorError>>>,
}

/// Call-owned facts available to a cancellation recorder when a live persisted handle is dropped.
///
/// `Drop` cannot use wasmtime's `Accessor`, cannot borrow worker state, and cannot `.await`; any
/// production recorder must receive everything it needs from the handle itself and do the async
/// oplog/state work later.
#[derive(Debug, Clone)]
pub struct DroppedCall {
    pub(super) start_idx: OplogIndex,
    pub(super) function_type: DurableFunctionType,
    pub(super) request_upload: PendingUpload,
    pub(super) entity_admission: bool,
    pub(super) span_finished: Option<golem_common::model::oplog::SpanFinished>,
    pub(super) span_observer: Option<Arc<dyn CallSpanObserver>>,
    /// The dropped call's own trap classification, captured from its execution scope at drop time.
    /// A cancellation-drain failure (deferred request upload / terminal recorder join) traps with
    /// this context so the retry grouping belongs to the dropped call, not to whichever later host
    /// call happens to drive the drain.
    pub(super) trap_context: DurableCallTrapContext,
    /// Keeps the dropped call counted as an in-flight live host call until the drop event is
    /// actually processed (its `Cancelled`/terminal recorded at a drain point). Without this, a
    /// handle dropped between a drain and a subsequent boundary check would release its permit
    /// before its terminal
    /// entry is recorded, letting the terminal land on the far side of a positional replay
    /// boundary. `None` for call sites that only use the snapshot locally while the handle (and
    /// its own permit) is still alive.
    pub(super) live_call_permit: Option<LiveCallPermit>,
}

impl DroppedCall {
    pub(super) fn notify_span_closed(&self) {
        if let (Some(observer), Some(finished)) = (&self.span_observer, &self.span_finished) {
            observer.closed(finished);
        }
    }

    pub fn start_idx(&self) -> OplogIndex {
        self.start_idx
    }

    pub fn function_type(&self) -> &DurableFunctionType {
        &self.function_type
    }

    pub fn request_upload(&self) -> &PendingUpload {
        &self.request_upload
    }

    pub fn trap_context(&self) -> DurableCallTrapContext {
        self.trap_context
    }

    pub(super) async fn wait_request_upload(&self) -> Result<(), WorkerExecutorError> {
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
        ctx.end_durable_function(&self.function_type, false).await?;
        Ok(())
    }

    pub(super) async fn append_cancelled_with_oplog(
        &self,
        oplog: std::sync::Arc<dyn crate::services::oplog::Oplog>,
        partial: Option<OplogPayload<HostResponse>>,
    ) -> Result<(), WorkerExecutorError> {
        let partial = if self.entity_admission && partial.is_none() {
            Some(OplogPayload::Inline(Box::new(
                crate::durable_host::entity::cancelled_tool_terminal(
                    golem_common::model::oplog::payload::types::SerializableEntityBodyExecution::Skipped,
                )
                .await?
                .into(),
            )))
        } else {
            partial
        };
        let span_finished = if self.entity_admission && self.span_finished.is_none() {
            let entry = oplog.read(self.start_idx).await?;
            let OplogEntry::Start {
                span_started: Some(span),
                ..
            } = entry
            else {
                return Err(WorkerExecutorError::runtime(
                    "accepted entity admission has no opening span",
                ));
            };
            Some(golem_common::model::oplog::SpanFinished {
                span_id: span.span_id,
                finished_at: Timestamp::now_utc(),
                outcome: golem_common::model::oplog::SpanOutcome::Cancelled,
            })
        } else {
            self.span_finished.clone()
        };
        let cancelled = OplogEntry::Cancelled {
            timestamp: Timestamp::now_utc(),
            start_index: self.start_idx,
            partial,
            span_finished,
        };
        self.notify_span_closed();
        oplog.add(cancelled).await?;
        Ok(())
    }
}

/// Deferred work emitted from `Drop` impls that cannot touch the worker store themselves.
///
/// Each worker owns a `dropped_call_events` channel (see `PrivateDurableWorkerState`); `Drop`
/// impls ([`DurableCallSession`], [`AccessTerminalGuard`], resource wrappers) enqueue these events, and
/// they are drained from the next safe worker-access window: [`drain_queued_dropped_call_events`]
/// at the start of every `&mut ctx` durable call and [`drain_dropped_call_events_access`] on the
/// accessor path (call start and terminals). The drain records durable effects (`Cancelled`
/// entries, scope closes, span finishes) via [`record_dropped_call_event`] or the accessor-window
/// equivalent. Unit tests attach their own sink to observe the enqueued events directly.
#[derive(Debug)]
pub enum DropEvent {
    /// A `Cancellable` handle was dropped unfinished; the next drain records `Cancelled` from this
    /// call-owned snapshot and closes the matching durable-function scope.
    UnfinishedCancellable { call: DroppedCall },
    /// A `NotCancellable` handle was dropped unfinished; this is a programming error.
    UnfinishedNotCancellable { call: DroppedCall },
    /// A terminal append was handed to an owned task; wait for it before closing the call's
    /// durable scope. A join failure traps with the dropped call's own `trap_context`
    /// rather than ambient state.
    CleanupAfterTerminal {
        function_type: DurableFunctionType,
        terminal: Option<OrderedAppend>,
        trap_context: DurableCallTrapContext,
        /// Keeps the call counted as in flight until this event is fully consumed (the owned
        /// terminal task joined and the durable-function scope closed), so a positional boundary
        /// cannot be placed before the delayed terminal append.
        /// Never read — held purely for its `Drop` effect on the shared counter.
        live_call_permit: Option<LiveCallPermit>,
    },
    /// A deferred guest-delivery token ([`CompletionDelivery`]) was dropped while still armed:
    /// its call's terminal `End` is already recorded and the durable scope already closed, so the
    /// only remaining work is joining the owned marker (or trailing ordered-append) task while
    /// keeping the call counted as in flight — no scope close.
    /// Also joins accessor initiation abandoned by a disposed Store. The accepted metadata task
    /// retains ownership of its Start through the result handoff and queues cancellation separately.
    AwaitCompletionMarker {
        receipt: Option<MarkerReceipt>,
        trap_context: DurableCallTrapContext,
        /// Held so invocation settlement waits until the marker append is joined; released when
        /// the event is finished or dropped. Never read.
        live_call_permit: Option<LiveCallPermit>,
    },
    /// Settles an owned custom-invocation `begin` task before invocation progress can overtake it.
    /// Guest cancellation appends a matching `Cancelled`; whole-invocation failure only waits for
    /// a possible Start and leaves it incomplete for retry. Both release the parent's initiation
    /// guard after the owned task finishes.
    SettleCustomInvocationBegin {
        lifecycle: Arc<CustomBeginLifecycle>,
        terminal: Option<tokio::task::JoinHandle<Result<Option<OplogIndex>, WorkerExecutorError>>>,
    },
    /// An invocation-context span whose owning resource was dropped from a synchronous host
    /// context (e.g. a p3 HTTP response dropped before its body was consumed). Finish it from the
    /// next drain point.
    FinishSpan { span_id: SpanId },
    /// Guest abandonment of a p3 HTTP send/response. Live cleanup records a span hint at the
    /// captured abandonment time; replay only releases resident state. Neither changes the
    /// remote send's terminal.
    FinishP3HttpSpan { cleanup: Box<HttpSpanCleanup> },
    /// A guest dropped a durable readable stream endpoint synchronously. Wasmtime cannot await
    /// the durable consumer intent or source cancellation from the resource destructor. Live
    /// drains forward cancellation; replay drains transfer passive waits to the runtime owner.
    CancelDroppedDurableInput {
        cancellation: Box<DroppedDurableInput>,
    },
    /// A deferred input disposal failed admission or replay validation. Surface the failure
    /// through the Store without aborting unrelated native reconstruction tasks.
    DurableInputDisposalFailed {
        error: WorkerExecutorError,
        trap_context: DurableCallTrapContext,
    },
    /// A semantic producer disposal retains its commit receipt or resumes admission at the
    /// replay tail from a safe context window. Rust resource destruction never records it.
    DisposeToolProducer {
        disposal: Box<crate::durable_host::tool::durability::ProducerDisposal>,
        trap_context: DurableCallTrapContext,
    },
}

pub(super) struct AccessDropEventDrainGuard {
    sink: UnboundedSender<DropEvent>,
    pending: VecDeque<DropEvent>,
    current: Option<DropEvent>,
    disarmed: bool,
}

impl AccessDropEventDrainGuard {
    pub(super) fn new(sink: UnboundedSender<DropEvent>, events: Vec<DropEvent>) -> Self {
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

    pub(super) fn disarm(&mut self) {
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
/// Drains the worker's currently queued dropped-call events and records their durable effects
/// (cancellable drops as `Cancelled`, deferred terminal joins, scope closes, span finishes).
///
/// Called from the next safe worker-access window after the events were enqueued — the start of
/// every `&mut ctx` durable call ([`DurableCallSession::begin`]). The helper deliberately drains only
/// currently available events; callers decide where to wait for more work.
pub async fn drain_queued_dropped_call_events<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
) -> Result<usize, TerminalCallError> {
    let mut recorded = 0;
    while let Some(event) = ctx.state.take_next_dropped_call_event() {
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
        DropEvent::CleanupAfterTerminal {
            function_type,
            terminal,
            trap_context,
            // Bound (not `_`) so the permit is released only at the end of this arm, after the
            // terminal join and scope close.
            live_call_permit: _live_call_permit,
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
            ctx.end_durable_function(&function_type, false)
                .await
                .map_err(|err| TerminalCallError::new(err, trap_context))?;
        }
        DropEvent::AwaitCompletionMarker {
            mut receipt,
            trap_context,
            // Bound (not `_`) so the permit is released only at the end of this arm, after the
            // marker append is joined.
            live_call_permit: _live_call_permit,
        } => {
            if let Some(receipt) = &mut receipt {
                await_marker_receipt(receipt)
                    .await
                    .map_err(|err| TerminalCallError::new(err, trap_context))?;
            }
        }
        DropEvent::SettleCustomInvocationBegin {
            lifecycle,
            terminal,
        } => {
            let cancelled_start = if let Some(terminal) = terminal {
                terminal
                    .await
                    .map_err(|err| {
                        TerminalCallError::new(
                            WorkerExecutorError::runtime(format!(
                                "custom invocation cancellation recorder task failed: {err}"
                            )),
                            ambient_trap_context(ctx),
                        )
                    })?
                    .map_err(|err| TerminalCallError::new(err, ambient_trap_context(ctx)))?
            } else {
                None
            };
            if let Some(start_index) = cancelled_start {
                ctx.state.active_custom_invocations.remove(&start_index);
            }
            lifecycle.release_initiation();
        }
        DropEvent::FinishSpan { span_id } => {
            finish_span_in_memory(ctx, &span_id)
                .map_err(|err| TerminalCallError::new(err, ambient_trap_context(ctx)))?;
        }
        DropEvent::FinishP3HttpSpan { cleanup } => {
            let mut drain = AccessDropEventDrainGuard::new(
                ctx.state
                    .dropped_call_event_sender()
                    .expect("worker cleanup queue"),
                vec![DropEvent::FinishP3HttpSpan { cleanup }],
            );
            drain.start_next();
            let DropEvent::FinishP3HttpSpan { cleanup } = drain.current_mut() else {
                unreachable!("HTTP cleanup guard contains the HTTP cleanup event");
            };
            let result = async {
                cleanup.progress.clone().await?;
                finish_span_in_memory(ctx, &cleanup.finished.span_id)
            }
            .await;
            drain.finish_current();
            drain.disarm();
            result.map_err(|err| TerminalCallError::new(err, ambient_trap_context(ctx)))?;
        }
        DropEvent::DisposeToolProducer {
            mut disposal,
            trap_context,
        } => {
            disposal
                .finish(ctx)
                .await
                .map_err(|error| TerminalCallError::new(error, trap_context))?;
        }
        DropEvent::CancelDroppedDurableInput { cancellation } => {
            let result = async {
                if cancellation.defer_if_replaying(ctx)? {
                    return Ok(());
                }
                ctx.process_pending_replay_events().await?;
                cancellation
                    .cancel()
                    .await
                    .map_err(|error| error.into_worker_executor_error(WorkerExecutorError::runtime))
            }
            .await;
            result.map_err(|error| TerminalCallError::new(error, ambient_trap_context(ctx)))?;
        }
        DropEvent::DurableInputDisposalFailed {
            error,
            trap_context,
        } => return Err(TerminalCallError::new(error, trap_context)),
    }
    Ok(())
}

pub(crate) async fn cancel_dropped_durable_input_access<T, D, Ctx>(
    store: &(impl DurableAccess<T, D> + ?Sized),
    get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
    cancellation: &DroppedDurableInput,
) -> Result<(), WorkerExecutorError>
where
    T: 'static,
    D: HasData + ?Sized,
    Ctx: WorkerCtx,
{
    if store.with(|mut access| cancellation.defer_if_replaying(get_ctx(access.data_mut())))? {
        return Ok(());
    }
    process_pending_replay_events_access(store, get_ctx).await?;
    cancellation
        .cancel()
        .await
        .map_err(|error| error.into_worker_executor_error(WorkerExecutorError::runtime))
}

/// Waits for the natural end of replay and switches the Store to live there, so a guest drop
/// whose cancellation the recorded run never persisted can record it live.
pub(crate) async fn continue_live_at_natural_tail_end_access<T, D, Ctx>(
    store: &(impl DurableAccess<T, D> + ?Sized),
    get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
    replay: &ReplayState,
    activity: &TailActivity,
    operation: &str,
    boundary: Option<golem_common::model::oplog::payload::types::SerializableToolAdmissionRefusal>,
) -> Result<(), WorkerExecutorError>
where
    T: 'static,
    D: HasData + ?Sized,
    Ctx: WorkerCtx,
{
    loop {
        replay.await_natural_tail_end(Some(activity)).await?;
        let (transition, primary) = store.with(|mut access| {
            let ctx = get_ctx(access.data_mut());
            (
                ctx.prepare_live_continuation_at_replay_tail(true, operation.to_string()),
                ctx.runtime == OwnerRuntime::Agent,
            )
        });
        match transition.await? {
            BeginReplayToLive::ReplayResumed => continue,
            BeginReplayToLive::Pending(pending) => {
                finish_prepared_access_to_live_at(
                    pending,
                    primary,
                    store,
                    get_ctx,
                    boundary.clone(),
                )
                .await?
                .require_live()?;
                return Ok(());
            }
        }
    }
}

/// Accessor-window variant of [`drain_queued_dropped_call_events`]. It drains the queue from a short
/// worker-state window, records `Cancelled` entries using owned oplog handles outside the window,
/// then closes the durable-function scope.
pub async fn drain_dropped_call_events_access<T, D, Ctx>(
    store: &Accessor<T, D>,
    get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
) -> Result<usize, TerminalCallError>
where
    T: 'static,
    D: HasData + ?Sized,
    Ctx: WorkerCtx,
{
    drain_dropped_call_events_context(store, get_ctx).await
}

pub(in crate::durable_host) async fn drain_dropped_call_events_context<T, D, Ctx>(
    store: &(impl DurableAccess<T, D> + ?Sized),
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
                let context = call.trap_context();
                let function_type = call.function_type().clone();
                let result = async {
                    call.wait_request_upload().await?;
                    call.append_cancelled_with_oplog(oplog.clone(), None).await
                }
                .await;
                match result {
                    Ok(()) => {
                        // The `Cancelled` entry is durable, so neutralize the event before the remaining fallible
                        // work: a torn drain from here re-queues only a permit-holding no-op
                        // instead of re-appending a second `Cancelled`.
                        let live_call_permit = call.live_call_permit.take();
                        drain.replace_current(DropEvent::AwaitCompletionMarker {
                            receipt: None,
                            trap_context: context,
                            live_call_permit,
                        });
                        if let Err(err) =
                            end_durable_function_access(store, get_ctx, function_type, false).await
                            && first_error.is_none()
                        {
                            first_error = Some(TerminalCallError::new(err, context));
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
            DropEvent::CleanupAfterTerminal {
                function_type,
                terminal,
                trap_context,
                // Left in place: the permit stays owned by the event, so it survives a torn drain
                // (the event is re-queued with the permit) and is released when the event is
                // finished or dropped.
                live_call_permit: _,
            } => {
                let function_type = function_type.clone();
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
                    if let Err(err) =
                        end_durable_function_access(store, get_ctx, function_type, false).await
                    {
                        if first_error.is_none() {
                            first_error = Some(TerminalCallError::new(err, trap_context));
                        }
                    } else {
                        recorded += 1;
                    }
                }
            }
            DropEvent::AwaitCompletionMarker {
                receipt,
                trap_context,
                // Left in place: the permit stays owned by the event, so it survives a torn drain
                // (the event is re-queued with the permit) and is released when the event is
                // finished or dropped.
                live_call_permit: _,
            } => {
                let trap_context = *trap_context;
                if let Some(pending) = receipt {
                    match await_marker_receipt(pending).await {
                        Ok(()) => {
                            *receipt = None;
                            recorded += 1;
                        }
                        Err(err) => {
                            if first_error.is_none() {
                                first_error = Some(TerminalCallError::new(err, trap_context));
                            }
                        }
                    }
                } else {
                    recorded += 1;
                }
            }
            DropEvent::SettleCustomInvocationBegin {
                lifecycle,
                terminal,
            } => {
                let mut cancelled_start = None;
                if let Some(handle) = terminal {
                    match handle.await.map_err(|err| {
                        WorkerExecutorError::runtime(format!(
                            "custom invocation cancellation recorder task failed: {err}"
                        ))
                    }) {
                        Ok(Ok(start_index)) => {
                            *terminal = None;
                            cancelled_start = start_index;
                            recorded += 1;
                        }
                        Ok(Err(err)) | Err(err) => {
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
                if terminal.is_none() {
                    if let Some(start_index) = cancelled_start {
                        store.with(|mut access| {
                            get_ctx(access.data_mut())
                                .state
                                .active_custom_invocations
                                .remove(&start_index);
                        });
                    }
                    lifecycle.release_initiation();
                }
            }
            DropEvent::FinishSpan { span_id } => {
                let span_id = span_id.clone();
                let finish_result = store.with(|mut access| {
                    let ctx = get_ctx(access.data_mut());
                    finish_span_in_memory(ctx, &span_id)
                });
                if let Err(err) = finish_result {
                    if first_error.is_none() {
                        first_error = Some(TerminalCallError::new(
                            err,
                            store.with(|mut access| {
                                let ctx = get_ctx(access.data_mut());
                                ambient_trap_context(ctx)
                            }),
                        ));
                    }
                } else {
                    recorded += 1;
                }
            }
            DropEvent::FinishP3HttpSpan { cleanup } => {
                let result = async {
                    cleanup.progress.clone().await?;
                    store.with(|mut access| {
                        finish_span_in_memory(get_ctx(access.data_mut()), &cleanup.finished.span_id)
                    })
                }
                .await;
                if let Err(error) = result {
                    if first_error.is_none() {
                        first_error = Some(TerminalCallError::new(
                            error,
                            store.with(|mut access| {
                                ambient_trap_context(get_ctx(access.data_mut()))
                            }),
                        ));
                    }
                } else {
                    recorded += 1;
                }
            }
            DropEvent::DisposeToolProducer {
                disposal,
                trap_context,
            } => {
                let mut access = AccessorContext::new(store, get_ctx);
                if let Err(error) = disposal.finish(&mut access).await {
                    if first_error.is_none() {
                        first_error = Some(TerminalCallError::new(error, *trap_context));
                    }
                } else {
                    recorded += 1;
                }
            }
            DropEvent::CancelDroppedDurableInput { cancellation } => {
                if let Err(error) =
                    cancel_dropped_durable_input_access(store, get_ctx, cancellation).await
                {
                    if first_error.is_none() {
                        first_error = Some(TerminalCallError::new(
                            error,
                            store.with(|mut access| {
                                let ctx = get_ctx(access.data_mut());
                                ambient_trap_context(ctx)
                            }),
                        ));
                    }
                } else {
                    recorded += 1;
                }
            }
            DropEvent::DurableInputDisposalFailed {
                error,
                trap_context,
            } => {
                if first_error.is_none() {
                    first_error = Some(TerminalCallError::new(error.clone(), *trap_context));
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

#[cfg(test)]
pub(super) mod tests {
    use super::super::tests::InMemoryOplog;
    use super::*;
    use golem_common::model::component::ComponentId;
    use golem_common::model::environment::EnvironmentId;
    use golem_common::model::oplog::{SpanFinished, SpanOutcome};
    use golem_common::model::regions::DeletedRegions;
    use golem_common::model::{AgentId, OwnedAgentId};
    use test_r::test;

    fn noop() -> OplogEntry {
        OplogEntry::RemoveRetryPolicy {
            timestamp: Timestamp::now_utc(),
            entity_parent_start_index: None,
            name: "test-policy".to_string(),
        }
    }

    fn finished() -> SpanFinished {
        SpanFinished {
            span_id: SpanId::generate(),
            finished_at: "2025-03-12T13:14:15.123Z".parse().unwrap(),
            outcome: SpanOutcome::Abandoned,
        }
    }

    pub(crate) async fn cleanup_recorder(
        oplog: Arc<dyn Oplog>,
        live: bool,
        persisted: bool,
    ) -> (
        HttpSpanCleanupRecorder,
        tokio::sync::mpsc::UnboundedReceiver<DropEvent>,
    ) {
        let replay = ReplayState::new_for_owner(
            OwnedAgentId {
                environment_id: EnvironmentId::new(),
                agent_id: AgentId {
                    component_id: ComponentId::new(),
                    agent_id: "cleanup-test".to_string(),
                },
            },
            oplog.clone(),
            DeletedRegions::default(),
            None,
            crate::durable_host::tool::operation::OwnerToolOperations::new(),
            oplog.current_oplog_index().await,
            None,
        )
        .await
        .unwrap();
        cleanup_recorder_for_store(
            oplog,
            replay,
            Some(InvocationExecutionMode::Live),
            Arc::new(std::sync::atomic::AtomicBool::new(live)),
            persisted,
        )
    }

    pub(crate) fn cleanup_recorder_for_store(
        oplog: Arc<dyn Oplog>,
        replay: ReplayState,
        entity_execution_mode: Option<InvocationExecutionMode>,
        local_live_tail: Arc<std::sync::atomic::AtomicBool>,
        persisted: bool,
    ) -> (
        HttpSpanCleanupRecorder,
        tokio::sync::mpsc::UnboundedReceiver<DropEvent>,
    ) {
        let (sink, receiver) = tokio::sync::mpsc::unbounded_channel();
        (
            HttpSpanCleanupRecorder {
                oplog,
                replay,
                entity_execution_mode,
                local_live_tail,
                persisted,
                parent: None,
                send_start_index: OplogIndex::from_u64(17),
                sink,
            },
            receiver,
        )
    }

    #[test]
    #[test_r::timeout("30s")]
    async fn http_cleanup_respects_the_recorded_send_close_owner() {
        for terminal_kind in ["closing-end", "cancelled", "open-end"] {
            let oplog = Arc::new(InMemoryOplog::new());
            oplog.add(noop()).await.unwrap();
            let send_index = oplog.add(OplogEntry::Start {
                timestamp: Timestamp::now_utc(),
                parent_start_index: None,
                function_name: golem_common::model::oplog::host_functions::P3HttpClientSend::HOST_FUNCTION_NAME,
                invocation_id: None,
                observational_owner: None,
                request: Some(OplogPayload::Inline(Box::new(golem_common::model::oplog::HostRequestNoInput {}.into()))),
                durable_function_type: DurableFunctionType::WriteRemote,
                span_started: None,
                denial: None,
            }).await.unwrap();
            let close = finished();
            let terminal = if terminal_kind == "cancelled" {
                OplogEntry::Cancelled {
                    timestamp: Timestamp::now_utc(),
                    start_index: send_index,
                    partial: None,
                    span_finished: Some(close.clone()),
                }
            } else {
                OplogEntry::End {
                    timestamp: Timestamp::now_utc(),
                    start_index: send_index,
                    response: Some(OplogPayload::Inline(Box::new(
                        golem_common::model::oplog::HostResponseGolemApiUnit { result: Ok(()) }
                            .into(),
                    ))),
                    forced_commit: false,
                    span_finished: (terminal_kind == "closing-end").then(|| close.clone()),
                    span_attributes: None,
                }
            };
            oplog.add(terminal).await.unwrap();
            for live in [true, false] {
                let (recorder, mut events) = cleanup_recorder(oplog.clone(), live, true).await;
                let before = oplog.length().await.unwrap();
                recorder.for_start(send_index).record(close.clone());
                let DropEvent::FinishP3HttpSpan { cleanup } = events.recv().await.unwrap() else {
                    panic!("cleanup event");
                };
                cleanup.progress.await.unwrap();
                assert_eq!(
                    oplog.length().await.unwrap(),
                    before + u64::from(live && terminal_kind == "open-end")
                );
                assert!(events.try_recv().is_err());
            }
        }
    }

    #[test]
    #[test_r::timeout("30s")]
    async fn http_cleanup_freezes_store_liveness_when_queued() {
        for live_at_drop in [true, false] {
            let oplog = Arc::new(InMemoryOplog::new());
            oplog.add(noop()).await.unwrap();
            let (recorder, mut events) = cleanup_recorder(oplog.clone(), live_at_drop, true).await;
            let local_live_tail = recorder.local_live_tail.clone();
            recorder.record(finished());
            local_live_tail.store(!live_at_drop, std::sync::atomic::Ordering::Release);
            let DropEvent::FinishP3HttpSpan { cleanup } = events.try_recv().unwrap() else {
                panic!("cleanup event");
            };
            cleanup.progress.await.unwrap();
            assert_eq!(oplog.length().await.unwrap(), 1 + u64::from(live_at_drop));
        }
    }

    #[test]
    #[test_r::timeout("30s")]
    async fn http_cleanup_receipt_survives_cancelled_drain_and_keeps_drop_time() {
        for cancel_before_actor_finishes in [true, false] {
            let (reached, mut reached_rx) = tokio::sync::mpsc::unbounded_channel();
            let gate = Arc::new(tokio::sync::Semaphore::new(0));
            let oplog = Arc::new(InMemoryOplog::with_end_gate(reached, gate.clone()));
            oplog.add(noop()).await.unwrap();
            let (mut recorder, mut receiver) = cleanup_recorder(oplog.clone(), true, true).await;
            // Hold the ordered writer before the hint. Polling cleanup must submit its append
            // and park on that receipt, not merely leave an unstarted future in the queue.
            let preceding = oplog.enqueue_add(OplogEntry::End {
                timestamp: Timestamp::now_utc(),
                start_index: OplogIndex::INITIAL,
                response: None,
                forced_commit: false,
                span_finished: None,
                span_attributes: None,
            });
            reached_rx.recv().await.unwrap();
            recorder.parent = Some(OplogIndex::from_u64(8));
            let sink = recorder.sink.clone();
            let closed = finished();
            recorder.record(closed.clone());
            let event = receiver.try_recv().unwrap();
            let mut drain = AccessDropEventDrainGuard::new(sink, vec![event]);
            assert!(drain.start_next());
            let DropEvent::FinishP3HttpSpan { cleanup } = drain.current_mut() else {
                panic!("expected cleanup");
            };
            assert!(futures::poll!(cleanup.progress.clone()).is_pending());
            assert_eq!(oplog.length().await.unwrap(), 1);
            if !cancel_before_actor_finishes {
                gate.add_permits(1);
                cleanup.progress.clone().await.unwrap();
            }
            drop(drain);
            if cancel_before_actor_finishes {
                gate.add_permits(1);
            }
            preceding.await.unwrap();
            let DropEvent::FinishP3HttpSpan { cleanup } = receiver.try_recv().unwrap() else {
                panic!("cancelled drain must retain cleanup");
            };
            cleanup.progress.clone().await.unwrap();
            cleanup.progress.await.unwrap();
            assert!(receiver.try_recv().is_err());
            assert_eq!(oplog.length().await.unwrap(), 3);
            let hint = oplog.read(OplogIndex::from_u64(3)).await.unwrap();
            assert!(
                matches!(hint, OplogEntry::SpanClosed { timestamp, entity_parent_start_index: Some(parent), resource_start_index: Some(start_index), span_finished }
                if start_index == OplogIndex::from_u64(17) && parent == OplogIndex::from_u64(8) && timestamp == closed.finished_at && span_finished == closed)
            );
        }
    }

    #[test]
    #[test_r::timeout("30s")]
    async fn http_cleanup_hint_does_not_block_an_existing_call_without_a_drop() {
        let oplog = Arc::new(InMemoryOplog::new());
        oplog.add(noop()).await.unwrap();
        let other_name = HostFunctionName::Custom("other-read".to_string());
        let other_index = oplog
            .add(OplogEntry::Start {
                timestamp: Timestamp::now_utc(),
                parent_start_index: None,
                observational_owner: None,
                function_name: other_name.clone(),
                invocation_id: None,
                request: Some(OplogPayload::Inline(Box::new(
                    golem_common::model::oplog::HostRequestNoInput {}.into(),
                ))),
                durable_function_type: DurableFunctionType::ReadRemote,
                span_started: None,
                denial: None,
            })
            .await
            .unwrap();
        let closed = finished();
        let (live, mut live_events) = cleanup_recorder(oplog.clone(), true, true).await;
        live.record(closed.clone());
        let DropEvent::FinishP3HttpSpan { cleanup } = live_events.try_recv().unwrap() else {
            panic!("expected cleanup")
        };
        cleanup.progress.await.unwrap();
        let other_end = oplog
            .add(OplogEntry::End {
                timestamp: Timestamp::now_utc(),
                start_index: other_index,
                response: None,
                forced_commit: false,
                span_finished: None,
                span_attributes: None,
            })
            .await
            .unwrap();
        let (recorder, _events) = cleanup_recorder(oplog.clone(), false, true).await;
        let replay = recorder.replay.clone();
        let crate::durable_host::replay_state::ReplayStartClaimOutcome::Claimed { handle, .. } =
            replay
                .claim_start_or_replay_end(crate::durable_host::replay_state::StartClaim::unowned(
                    &other_name,
                    &DurableFunctionType::ReadRemote,
                ))
                .await
                .unwrap()
        else {
            panic!("expected existing call Start");
        };
        let result = replay.await_resolution_outcome(handle).await.unwrap();
        assert!(
            matches!(result, ResolutionOutcome::Resolved(Resolution::Completed { end_idx, .. }) if end_idx == other_end)
        );
        assert_eq!(oplog.length().await.unwrap(), 4);
    }

    #[test]
    async fn http_cleanup_snapshot_provenance_suppresses_live_and_replay_work() {
        for live in [true, false] {
            let oplog = Arc::new(InMemoryOplog::new());
            oplog.add(noop()).await.unwrap();
            let (recorder, mut receiver) = cleanup_recorder(oplog.clone(), live, false).await;
            recorder.record(finished());
            let DropEvent::FinishP3HttpSpan { cleanup } = receiver.try_recv().unwrap() else {
                panic!("expected cleanup")
            };
            cleanup.progress.await.unwrap();
            assert_eq!(oplog.length().await.unwrap(), 1);
        }
    }
}
