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

/// Call-owned facts available to a cancellation recorder when a live persisted handle is dropped.
///
/// `Drop` cannot use wasmtime's `Accessor`, cannot borrow worker state, and cannot `.await`; any
/// production recorder must receive everything it needs from the handle itself and do the async
/// oplog/state work later.
#[derive(Debug, Clone)]
pub struct DroppedCall {
    pub(super) start_idx: OplogIndex,
    pub(super) begin_index: OplogIndex,
    pub(super) function_type: DurableFunctionType,
    pub(super) request_upload: PendingUpload,
    /// The dropped call's atomic-region ownership lease, shared with every other holder (the
    /// originating handle, terminal guards). Released — store-free and idempotently — once the
    /// call's terminal is recorded.
    pub(super) atomic_lease: Option<Arc<AtomicRegionLease>>,
    /// The dropped call's own trap classification, captured from its execution scope at drop time.
    /// A cancellation-drain failure (deferred request upload / terminal recorder join) traps with
    /// this context so the retry grouping belongs to the dropped call, not to whichever later host
    /// call happens to drive the drain.
    pub(super) trap_context: DurableCallTrapContext,
    /// Keeps the dropped call counted as an in-flight live host call until the drop event is
    /// actually processed (its `Cancelled`/terminal recorded at a drain point). Without this, a
    /// handle dropped between a drain and a subsequent in-flight check (e.g. the
    /// `set_oplog_persistence_level` boundary guard) would release its permit before its terminal
    /// entry is recorded, letting the terminal land on the far side of a positional replay
    /// boundary. `None` for call sites that only use the snapshot locally while the handle (and
    /// its own permit) is still alive.
    pub(super) live_call_permit: Option<LiveCallPermit>,
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

    /// The atomic region currently owning the dropped call, read through its lease.
    pub fn atomic_region(&self) -> Option<OplogIndex> {
        self.atomic_lease.as_ref().and_then(|lease| lease.owner())
    }

    /// Releases the dropped call's atomic-region membership (idempotent, store-free).
    pub(super) fn release_atomic_lease(&self) {
        if let Some(lease) = &self.atomic_lease {
            lease.release();
        }
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
        self.release_atomic_lease();
        ctx.end_durable_function(&self.function_type, self.begin_index, false)
            .await?;
        Ok(())
    }

    pub(super) async fn append_cancelled_with_oplog(
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

/// Deferred work emitted from `Drop` impls that cannot touch the worker store themselves.
///
/// Each worker owns a `dropped_call_events` channel (see `PrivateDurableWorkerState`); `Drop`
/// impls ([`CallHandle`], [`AccessTerminalGuard`], resource wrappers) enqueue these events, and
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
    /// A terminal append was handed to an owned task; wait for it before releasing the call's
    /// atomic-region lease. A join failure traps with the dropped call's own `trap_context`
    /// rather than ambient state.
    CleanupAfterTerminal {
        atomic_lease: Option<Arc<AtomicRegionLease>>,
        function_type: DurableFunctionType,
        durable_begin_index: OplogIndex,
        terminal: Option<tokio::task::JoinHandle<Result<(), WorkerExecutorError>>>,
        trap_context: DurableCallTrapContext,
        /// Keeps the call counted as in flight until this event is fully consumed (the owned
        /// terminal task joined and the durable-function scope closed), so a positional boundary
        /// (e.g. a persistence-level change) cannot be placed before the delayed terminal append.
        /// Never read — held purely for its `Drop` effect on the shared counter.
        live_call_permit: Option<LiveCallPermit>,
    },
    /// A deferred guest-delivery token ([`CompletionDelivery`]) was dropped while still armed:
    /// its call's terminal `End` is already recorded and the durable scope already closed, so the
    /// only remaining work is joining the owned marker (or trailing ordered-append) task while
    /// keeping the call counted as in flight — no scope close and no atomic-region cleanup.
    AwaitDiscardMarker {
        terminal: Option<tokio::task::JoinHandle<Result<(), WorkerExecutorError>>>,
        trap_context: DurableCallTrapContext,
        /// Held so invocation settlement waits until the marker append is joined; released when
        /// the event is finished or dropped. Never read.
        live_call_permit: Option<LiveCallPermit>,
    },
    /// A guest-cancelled accessor future may leave a caller-managed durable scope with no code path
    /// back into the resource's `drop`. Close that parent scope from the next safe store-access
    /// window. The close is idempotent because the resource may be dropped before this event drains.
    CloseDurableScope {
        function_type: DurableFunctionType,
        begin_index: OplogIndex,
        span_id: Option<SpanId>,
    },
    /// An invocation-context span whose owning resource was dropped from a synchronous host
    /// context (e.g. a p3 HTTP response dropped before its body was consumed). Finish it from the
    /// next drain point. When `durable` (the span was replayed from a legacy positional
    /// `StartSpan` entry), live writes the `FinishSpan` there and replay consumes it at the same
    /// point, since drain points are deterministic replay points; otherwise the span is derived
    /// (no span oplog entries exist) and is finished in memory only.
    FinishSpan { span_id: SpanId, durable: bool },
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
/// Drains the worker's currently queued dropped-call events and records their durable effects
/// (cancellable drops as `Cancelled`, deferred terminal joins, scope closes, span finishes).
///
/// Called from the next safe worker-access window after the events were enqueued — the start of
/// every `&mut ctx` durable call ([`CallHandle::begin`]). The helper deliberately drains only
/// currently available events; callers decide where to wait for more work.
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
        DropEvent::CleanupAfterTerminal {
            atomic_lease,
            function_type,
            durable_begin_index,
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
            if let Some(lease) = &atomic_lease {
                lease.release();
            }
            ctx.end_durable_function(&function_type, durable_begin_index, false)
                .await
                .map_err(|err| TerminalCallError::new(err, trap_context))?;
        }
        DropEvent::AwaitDiscardMarker {
            terminal,
            trap_context,
            // Bound (not `_`) so the permit is released only at the end of this arm, after the
            // marker append is joined.
            live_call_permit: _live_call_permit,
        } => {
            if let Some(terminal) = terminal {
                let joined = terminal.await.map_err(|err| {
                    WorkerExecutorError::runtime(format!(
                        "completion-discarded marker recorder task failed: {err}"
                    ))
                });
                match joined {
                    Ok(Ok(())) => {}
                    Ok(Err(err)) | Err(err) => {
                        return Err(TerminalCallError::new(err, trap_context));
                    }
                }
            }
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
        DropEvent::FinishSpan { span_id, durable } => {
            if durable {
                ctx.finish_span(&span_id)
                    .await
                    .map_err(|err| TerminalCallError::new(err, ambient_trap_context(ctx)))?;
            } else {
                finish_span_in_memory(ctx, &span_id)
                    .map_err(|err| TerminalCallError::new(err, ambient_trap_context(ctx)))?;
            }
        }
    }
    Ok(())
}

/// Accessor-window variant of [`drain_queued_dropped_call_events`]. It drains the queue from a short
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
                        // The `Cancelled` entry is durable and the lease release is synchronous
                        // (store-free), so neutralize the event before the remaining fallible
                        // work: a torn drain from here re-queues only a permit-holding no-op
                        // instead of re-appending a second `Cancelled`.
                        call.release_atomic_lease();
                        let live_call_permit = call.live_call_permit.take();
                        drain.replace_current(DropEvent::AwaitDiscardMarker {
                            terminal: None,
                            trap_context: context,
                            live_call_permit,
                        });
                        if let Err(err) = end_durable_function_access(
                            store,
                            get_ctx,
                            function_type,
                            durable_begin_index,
                            false,
                        )
                        .await
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
                atomic_lease,
                function_type,
                durable_begin_index,
                terminal,
                trap_context,
                // Left in place: the permit stays owned by the event, so it survives a torn drain
                // (the event is re-queued with the permit) and is released when the event is
                // finished or dropped.
                live_call_permit: _,
            } => {
                let atomic_lease = atomic_lease.clone();
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
                    if let Some(lease) = &atomic_lease {
                        lease.release();
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
            DropEvent::AwaitDiscardMarker {
                terminal,
                trap_context,
                // Left in place: the permit stays owned by the event, so it survives a torn drain
                // (the event is re-queued with the permit) and is released when the event is
                // finished or dropped.
                live_call_permit: _,
            } => {
                let trap_context = *trap_context;
                if let Some(handle) = terminal {
                    match handle.await.map_err(|err| {
                        WorkerExecutorError::runtime(format!(
                            "completion-discarded marker recorder task failed: {err}"
                        ))
                    }) {
                        Ok(Ok(())) => {
                            *terminal = None;
                            recorded += 1;
                        }
                        Ok(Err(err)) | Err(err) => {
                            if first_error.is_none() {
                                first_error = Some(TerminalCallError::new(err, trap_context));
                            }
                        }
                    }
                } else {
                    recorded += 1;
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
            DropEvent::FinishSpan { span_id, durable } => {
                let span_id = span_id.clone();
                let durable = *durable;
                let finish_result = if durable {
                    finish_span_access(store, get_ctx, &span_id).await
                } else {
                    store.with(|mut access| {
                        let ctx = get_ctx(access.data_mut());
                        finish_span_in_memory(ctx, &span_id)
                    })
                };
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
        }
        drain.finish_current();
    }
    if let Some(err) = first_error {
        return Err(err);
    }
    drain.disarm();
    Ok(recorded)
}
