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
use crate::durable_host::replay_state::{ReplayStartClaimOutcome, StartClaim};
use crate::durable_host::tail_work::{TailActivity, TailWorkTracker};
use futures::FutureExt;
use futures::future::{BoxFuture, Shared};
use golem_common::model::entity::OwnerRuntime;
use golem_common::model::oplog::host_functions::P3HttpSpanCleanup;
use golem_common::model::oplog::{HostRequestP3HttpSpanCleanup, HostResponseGolemApiUnit};

/// Store-independent inputs captured before a synchronous HTTP resource/terminal drop.
#[derive(Clone, Debug)]
pub(crate) struct HttpSpanCleanupRecorder {
    oplog: Arc<dyn Oplog>,
    replay: ReplayState,
    live: bool,
    persisted: bool,
    parent: Option<OplogIndex>,
    owner: Option<OplogIndex>,
    send_start_index: OplogIndex,
    sink: UnboundedSender<DropEvent>,
    tail_work: TailWorkTracker,
}

impl HttpSpanCleanupRecorder {
    pub(crate) fn new<Ctx: WorkerCtx>(
        ctx: &DurableWorkerCtx<Ctx>,
        send_start_index: OplogIndex,
        owner: Option<OplogIndex>,
        persisted: bool,
    ) -> Self {
        Self {
            oplog: ctx.state.oplog.clone(),
            replay: ctx.state.replay_state.clone(),
            live: ctx.is_live(),
            persisted: persisted && !ctx.state.snapshotting_mode,
            parent: ctx
                .child_parent_start_index(&DurableFunctionType::ReadLocal, OplogIndex::INITIAL),
            owner,
            send_start_index,
            sink: ctx
                .state
                .dropped_call_event_sender()
                .expect("worker cleanup queue"),
            tail_work: ctx.tail_work_tracker(),
        }
    }

    pub(crate) fn for_start(
        mut self,
        send_start_index: OplogIndex,
        owner: Option<OplogIndex>,
        live: bool,
    ) -> Self {
        self.send_start_index = send_start_index;
        self.owner = owner;
        self.live = live;
        self
    }

    #[must_use]
    pub(crate) fn record(
        self,
        finished: golem_common::model::oplog::SpanFinished,
    ) -> Option<HttpSpanCleanupReplay> {
        let progress = if !self.persisted {
            futures::future::ready(Ok(HttpSpanCleanupProgress::Complete))
                .boxed()
                .shared()
        } else if self.live {
            self.append(&finished, None)
        } else {
            let recorder = self.clone();
            let span_id = finished.span_id.clone();
            async move { recorder.replay(span_id).await }
                .boxed()
                .shared()
        };
        let driver = (!self.live && self.persisted).then(|| HttpSpanCleanupReplay {
            progress: progress.clone(),
            activity: self.tail_work.activity(),
        });
        let sink = self.sink.clone();
        let _ = sink.send(DropEvent::FinishP3HttpSpan {
            cleanup: Box::new(HttpSpanCleanup {
                recorder: self,
                finished,
                progress,
            }),
        });
        driver
    }

    fn append(
        &self,
        finished: &golem_common::model::oplog::SpanFinished,
        existing: Option<(OplogIndex, Timestamp)>,
    ) -> Shared<BoxFuture<'static, Result<HttpSpanCleanupProgress, WorkerExecutorError>>> {
        let mut finished = finished.clone();
        if let Some((_, timestamp)) = existing {
            finished.finished_at = timestamp;
        }
        let timestamp = finished.finished_at;
        let end = move |start_index| OplogEntry::End {
            timestamp,
            start_index,
            response: Some(OplogPayload::Inline(Box::new(
                HostResponseGolemApiUnit { result: Ok(()) }.into(),
            ))),
            forced_commit: false,
            span_finished: Some(finished),
            span_attributes: None,
        };
        if let Some((start_index, _)) = existing {
            let receipt = self.oplog.enqueue_add(end(start_index));
            async move {
                receipt.await;
                Ok(HttpSpanCleanupProgress::Complete)
            }
            .boxed()
            .shared()
        } else {
            let start = OplogEntry::Start {
                timestamp,
                parent_start_index: self.parent,
                function_name: P3HttpSpanCleanup::HOST_FUNCTION_NAME,
                invocation_id: None,
                observational_owner: self.owner,
                request: Some(OplogPayload::Inline(Box::new(
                    HostRequestP3HttpSpanCleanup {
                        send_start_index: self.send_start_index,
                    }
                    .into(),
                ))),
                durable_function_type: DurableFunctionType::ReadLocal,
                span_started: None,
            };
            let receipt = self.oplog.enqueue_add_pair(start, Box::new(end));
            async move {
                receipt.await;
                Ok(HttpSpanCleanupProgress::Complete)
            }
            .boxed()
            .shared()
        }
    }

    async fn replay(
        &self,
        span_id: SpanId,
    ) -> Result<HttpSpanCleanupProgress, WorkerExecutorError> {
        // The HTTP continuation may be cancelled before decoding its recorded result. Learn
        // whether that call's terminal already owns the close before claiming a cleanup call.
        // This read belongs to the retained cleanup future, so either its driver or a Store-
        // holding host drain can finish it without depending on the cancelled HTTP waiter.
        if let Some(
            OplogEntry::End {
                span_finished: Some(finished),
                ..
            }
            | OplogEntry::Cancelled {
                span_finished: Some(finished),
                ..
            },
        ) = self
            .replay
            .visible_terminal_entry(self.send_start_index)
            .await
            && finished.span_id == span_id
        {
            return Ok(HttpSpanCleanupProgress::Complete);
        }
        let request = HostRequestP3HttpSpanCleanup {
            send_start_index: self.send_start_index,
        }
        .into();
        let claim = match self.parent {
            Some(parent) => StartClaim::owned_matching_request(
                &P3HttpSpanCleanup::HOST_FUNCTION_NAME,
                &DurableFunctionType::ReadLocal,
                parent,
                &request,
            ),
            None => StartClaim::unowned_matching_request(
                &P3HttpSpanCleanup::HOST_FUNCTION_NAME,
                &DurableFunctionType::ReadLocal,
                &request,
            ),
        }
        .with_observational_owner(self.owner);
        let (handle, timestamp) = match self.replay.claim_start_or_replay_end(claim).await? {
            ReplayStartClaimOutcome::Claimed { handle, entry } => (handle, entry.timestamp()),
            ReplayStartClaimOutcome::ReplayEnded => {
                return Ok(HttpSpanCleanupProgress::ContinueLive {
                    existing: None,
                    replay_ended: true,
                });
            }
            ReplayStartClaimOutcome::DeletedRegion => {
                return Ok(HttpSpanCleanupProgress::ContinueLive {
                    existing: None,
                    replay_ended: false,
                });
            }
        };
        let start_index = handle.start_idx();
        match self.replay.await_resolution_outcome(handle).await? {
            ResolutionOutcome::Incomplete => Ok(HttpSpanCleanupProgress::ContinueLive {
                existing: Some((start_index, timestamp)),
                replay_ended: true,
            }),
            ResolutionOutcome::Resolved(Resolution::Completed {
                end_idx,
                response: Some(response),
                delivery_marker: None,
                ..
            }) => {
                let response = self
                    .oplog
                    .download_payload(response)
                    .await
                    .map_err(WorkerExecutorError::runtime)?;
                let expected: HostResponse = HostResponseGolemApiUnit { result: Ok(()) }.into();
                let end = self.oplog.read(end_idx).await;
                if response != expected
                    || !matches!(&end, OplogEntry::End { span_finished: Some(finished), .. } if finished.span_id == span_id)
                {
                    return Err(WorkerExecutorError::unexpected_oplog_entry(
                        "successful HTTP span cleanup with matching close",
                        format!("{end:?}"),
                    ));
                }
                Ok(HttpSpanCleanupProgress::Complete)
            }
            other => Err(WorkerExecutorError::unexpected_oplog_entry(
                "HTTP span cleanup End without guest delivery markers",
                format!("{other:?}"),
            )),
        }
    }
}

#[derive(Clone, Debug)]
enum HttpSpanCleanupProgress {
    Complete,
    ContinueLive {
        existing: Option<(OplogIndex, Timestamp)>,
        replay_ended: bool,
    },
}

#[derive(Debug)]
pub struct HttpSpanCleanup {
    recorder: HttpSpanCleanupRecorder,
    finished: golem_common::model::oplog::SpanFinished,
    // A cancelled drain drops only its clone; the queue retains the same claim/receipt future.
    progress: Shared<BoxFuture<'static, Result<HttpSpanCleanupProgress, WorkerExecutorError>>>,
}

/// Claims replay cleanup at the same guest action that enqueued the live pair. No Store access
/// is needed: a concurrent direct drain can poll the same future while holding the Store.
pub(crate) struct HttpSpanCleanupReplay {
    progress: Shared<BoxFuture<'static, Result<HttpSpanCleanupProgress, WorkerExecutorError>>>,
    activity: TailActivity,
}

impl<T: Send + 'static, D: HasData + ?Sized> wasmtime::component::AccessorTask<T, D>
    for HttpSpanCleanupReplay
{
    async fn run(self, _store: &Accessor<T, D>) -> wasmtime::Result<()> {
        // The queue retains both errors and live-continuation work for the ordinary drain.
        let _ = self.progress.await;
        drop(self.activity);
        Ok(())
    }
}

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
    pub(super) span_finished: Option<golem_common::model::oplog::SpanFinished>,
    pub(super) span_observer: Option<Arc<dyn CallSpanObserver>>,
    /// Shared signal for process-equivalent executor teardown. An unfinished call dropped after
    /// this signal is intentionally left incomplete for replay rather than treated as guest
    /// cancellation or a host-call programming error.
    pub(super) executor_shutdown: tokio_util::sync::CancellationToken,
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

    pub fn begin_index(&self) -> OplogIndex {
        self.begin_index
    }

    pub fn function_type(&self) -> &DurableFunctionType {
        &self.function_type
    }

    pub fn request_upload(&self) -> &PendingUpload {
        &self.request_upload
    }

    pub(super) fn is_executor_shutting_down(&self) -> bool {
        self.executor_shutdown.is_cancelled()
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
            span_finished: self.span_finished.clone(),
        };
        self.notify_span_closed();
        oplog.add(cancelled).await;
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
        /// cannot be placed before the delayed terminal append.
        /// Never read — held purely for its `Drop` effect on the shared counter.
        live_call_permit: Option<LiveCallPermit>,
    },
    /// A deferred guest-delivery token ([`CompletionDelivery`]) was dropped while still armed:
    /// its call's terminal `End` is already recorded and the durable scope already closed, so the
    /// only remaining work is joining the owned marker (or trailing ordered-append) task while
    /// keeping the call counted as in flight — no scope close and no atomic-region cleanup.
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
    /// A guest-cancelled accessor future may leave a caller-managed durable scope with no code path
    /// back into the resource's `drop`. Close that parent scope from the next safe store-access
    /// window. The close is idempotent because the resource may be dropped before this event drains.
    CloseDurableScope {
        function_type: DurableFunctionType,
        begin_index: OplogIndex,
        span_finished: Option<golem_common::model::oplog::SpanFinished>,
    },
    /// An invocation-context span whose owning resource was dropped from a synchronous host
    /// context (e.g. a p3 HTTP response dropped before its body was consumed). Finish it from the
    /// next drain point.
    FinishSpan { span_id: SpanId },
    /// Guest abandonment of a p3 HTTP send/response. The original remote send remains incomplete
    /// (pre-End) or completed (post-End); this separate local operation records the captured
    /// abandonment time and closes only its span.
    FinishP3HttpSpan { cleanup: Box<HttpSpanCleanup> },
    /// A guest dropped a durable readable stream endpoint synchronously. Wasmtime cannot await
    /// the durable consumer intent or source cancellation from the resource destructor, so the
    /// next safe worker-access window performs both before invocation progress can overtake them.
    CancelDroppedDurableInput {
        cancellation: Box<DroppedDurableInput>,
    },
}

/// Removes synchronously completed marker events while retaining every other event in order.
/// Returns false if any marker remains unresolved (including a cached receipt failure).
pub(crate) fn settle_completed_marker_events(events: &mut VecDeque<DropEvent>) -> bool {
    let mut retained = VecDeque::with_capacity(events.len());
    let mut unresolved_marker = false;
    while let Some(mut event) = events.pop_front() {
        let settled = match &mut event {
            DropEvent::AwaitCompletionMarker { receipt: None, .. } => true,
            DropEvent::AwaitCompletionMarker {
                receipt: Some(receipt),
                ..
            } => receipt.try_succeeded(),
            _ => false,
        };
        if !settled {
            unresolved_marker |= matches!(event, DropEvent::AwaitCompletionMarker { .. });
            retained.push_back(event);
        }
    }
    *events = retained;
    !unresolved_marker
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
        DropEvent::CloseDurableScope {
            function_type,
            begin_index,
            span_finished,
        } => {
            if ctx.state.is_durable_scope_open(begin_index) {
                if let Some(span_finished) = span_finished {
                    ctx.end_durable_function_with_span(
                        &function_type,
                        begin_index,
                        false,
                        span_finished.clone(),
                    )
                    .await
                    .map_err(|err| TerminalCallError::new(err, ambient_trap_context(ctx)))?;
                    finish_span_in_memory(ctx, &span_finished.span_id)
                        .map_err(|err| TerminalCallError::new(err, ambient_trap_context(ctx)))?;
                } else {
                    ctx.end_durable_function(&function_type, begin_index, false)
                        .await
                        .map_err(|err| TerminalCallError::new(err, ambient_trap_context(ctx)))?;
                }
            }
        }
        DropEvent::FinishSpan { span_id } => {
            finish_span_in_memory(ctx, &span_id)
                .map_err(|err| TerminalCallError::new(err, ambient_trap_context(ctx)))?;
        }
        DropEvent::FinishP3HttpSpan { mut cleanup } => {
            while let HttpSpanCleanupProgress::ContinueLive {
                existing,
                replay_ended,
            } = cleanup
                .progress
                .clone()
                .await
                .map_err(|err| TerminalCallError::new(err, ambient_trap_context(ctx)))?
            {
                if !ctx
                    .continue_live_at_replay_tail(replay_ended, "HTTP span cleanup".to_string())
                    .await
                    .map_err(|err| TerminalCallError::new(err, ambient_trap_context(ctx)))?
                {
                    return Err(TerminalCallError::new(
                        WorkerExecutorError::runtime(
                            "replay target grew while HTTP span cleanup was settling",
                        ),
                        ambient_trap_context(ctx),
                    ));
                }
                cleanup.progress = cleanup.recorder.append(&cleanup.finished, existing);
            }
            finish_span_in_memory(ctx, &cleanup.finished.span_id)
                .map_err(|err| TerminalCallError::new(err, ambient_trap_context(ctx)))?;
        }
        DropEvent::CancelDroppedDurableInput { cancellation } => {
            let result = async {
                if !ctx.is_live() {
                    if cancellation
                        .is_recorded()
                        .await
                        .map_err(WorkerExecutorError::runtime)?
                    {
                        return Ok(());
                    }
                    if ctx.rejects_live_continuation_at_replay_tail() {
                        return Err(WorkerExecutorError::unexpected_oplog_entry(
                            "durable stream guest-drop cancellation",
                            "no recorded cancellation during completed entity replay",
                        ));
                    }
                    loop {
                        ctx.state.replay_state.await_natural_tail_end(None).await?;
                        match ctx
                            .prepare_live_continuation_at_replay_tail(
                                true,
                                "durable stream guest-drop cancellation".to_string(),
                            )
                            .await?
                        {
                            BeginReplayToLive::ReplayResumed => continue,
                            BeginReplayToLive::Pending(pending) => {
                                ctx.finish_switch_to_live(pending).await?.require_live()?;
                                break;
                            }
                        }
                    }
                }
                cancellation
                    .cancel()
                    .await
                    .map_err(WorkerExecutorError::runtime)
            }
            .await;
            result.map_err(|error| TerminalCallError::new(error, ambient_trap_context(ctx)))?;
        }
    }
    Ok(())
}

pub(crate) async fn cancel_dropped_durable_input_access<T, D, Ctx>(
    store: &Accessor<T, D>,
    get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
    cancellation: &DroppedDurableInput,
) -> Result<(), WorkerExecutorError>
where
    T: 'static,
    D: HasData + ?Sized,
    Ctx: WorkerCtx,
{
    let (live, rejected, replay, activity) = store.with(|mut access| {
        let ctx = get_ctx(access.data_mut());
        (
            ctx.is_live(),
            ctx.rejects_live_continuation_at_replay_tail(),
            ctx.state.replay_state.clone(),
            ctx.tail_work_tracker().activity(),
        )
    });
    if !live {
        if cancellation
            .is_recorded()
            .await
            .map_err(WorkerExecutorError::runtime)?
        {
            return Ok(());
        }
        if rejected {
            return Err(WorkerExecutorError::unexpected_oplog_entry(
                "durable stream guest-drop cancellation",
                "no recorded cancellation during completed entity replay",
            ));
        }
        loop {
            replay.await_natural_tail_end(Some(&activity)).await?;
            let (transition, primary) = store.with(|mut access| {
                let ctx = get_ctx(access.data_mut());
                (
                    ctx.prepare_live_continuation_at_replay_tail(
                        true,
                        "durable stream guest-drop cancellation".to_string(),
                    ),
                    ctx.runtime == OwnerRuntime::Agent,
                )
            });
            match transition.await? {
                BeginReplayToLive::ReplayResumed => continue,
                BeginReplayToLive::Pending(pending) => {
                    finish_prepared_access_to_live(pending, primary, store, get_ctx)
                        .await?
                        .require_live()?;
                    break;
                }
            }
        }
    }
    cancellation
        .cancel()
        .await
        .map_err(WorkerExecutorError::runtime)
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
                        drain.replace_current(DropEvent::AwaitCompletionMarker {
                            receipt: None,
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
            DropEvent::CloseDurableScope {
                function_type,
                begin_index,
                span_finished,
            } => {
                let function_type = function_type.clone();
                let begin_index = *begin_index;
                let span_finished = span_finished.clone();
                match end_durable_function_access_if_open_with_span(
                    store,
                    get_ctx,
                    function_type,
                    begin_index,
                    false,
                    span_finished.clone(),
                )
                .await
                {
                    Ok(true) => {
                        if let Some(span_finished) = span_finished
                            && let Err(err) = store.with(|mut access| {
                                finish_span_in_memory(
                                    get_ctx(access.data_mut()),
                                    &span_finished.span_id,
                                )
                            })
                            && first_error.is_none()
                        {
                            first_error = Some(TerminalCallError::new(
                                err,
                                store.with(|mut access| {
                                    ambient_trap_context(get_ctx(access.data_mut()))
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
                    while let HttpSpanCleanupProgress::ContinueLive {
                        existing,
                        replay_ended,
                    } = cleanup.progress.clone().await?
                    {
                        let (transition, primary) = store.with(|mut access| {
                            let ctx = get_ctx(access.data_mut());
                            (
                                ctx.prepare_live_continuation_at_replay_tail(
                                    replay_ended,
                                    "HTTP span cleanup".to_string(),
                                ),
                                ctx.runtime == OwnerRuntime::Agent,
                            )
                        });
                        match transition.await? {
                            BeginReplayToLive::ReplayResumed => {
                                return Err(WorkerExecutorError::runtime(
                                    "replay target grew while HTTP span cleanup was settling",
                                ));
                            }
                            BeginReplayToLive::Pending(pending) => {
                                finish_prepared_access_to_live(pending, primary, store, get_ctx)
                                    .await?
                                    .require_live()?;
                            }
                        }
                        // Submission and replacement have no await between them: a torn drain
                        // retains the receipt, never the instruction to append again.
                        cleanup.progress = cleanup.recorder.append(&cleanup.finished, existing);
                    }
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
        OplogEntry::NoOp {
            timestamp: Timestamp::now_utc(),
            entity_parent_start_index: None,
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
        )
        .await
        .unwrap();
        let (sink, receiver) = tokio::sync::mpsc::unbounded_channel();
        (
            HttpSpanCleanupRecorder {
                oplog,
                replay,
                live,
                persisted,
                parent: None,
                owner: None,
                send_start_index: OplogIndex::from_u64(17),
                sink,
                tail_work: TailWorkTracker::new(),
            },
            receiver,
        )
    }

    #[test]
    #[test_r::timeout("30s")]
    async fn http_cleanup_replay_respects_the_recorded_send_close_owner() {
        for terminal_kind in ["closing-end", "cancelled", "open-end"] {
            let oplog = Arc::new(InMemoryOplog::new());
            oplog.add(noop()).await;
            let send_index = oplog.add(OplogEntry::Start {
                timestamp: Timestamp::now_utc(),
                parent_start_index: None,
                function_name: golem_common::model::oplog::host_functions::P3HttpClientSend::HOST_FUNCTION_NAME,
                invocation_id: None,
                observational_owner: None,
                request: Some(OplogPayload::Inline(Box::new(golem_common::model::oplog::HostRequestNoInput {}.into()))),
                durable_function_type: DurableFunctionType::WriteRemote,
                span_started: None,
            }).await;
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
                        HostResponseGolemApiUnit { result: Ok(()) }.into(),
                    ))),
                    forced_commit: false,
                    span_finished: (terminal_kind == "closing-end").then(|| close.clone()),
                    span_attributes: None,
                }
            };
            oplog.add(terminal).await;
            if terminal_kind == "open-end" {
                let (recorder, mut events) = cleanup_recorder(oplog.clone(), true, true).await;
                assert!(
                    recorder
                        .for_start(send_index, None, true)
                        .record(close.clone())
                        .is_none()
                );
                let DropEvent::FinishP3HttpSpan { cleanup } = events.recv().await.unwrap() else {
                    panic!("cleanup event");
                };
                assert!(matches!(
                    cleanup.progress.await.unwrap(),
                    HttpSpanCleanupProgress::Complete
                ));
            }
            let before = oplog
                .read_exact(OplogIndex::INITIAL, oplog.length().await)
                .await;
            let (recorder, mut events) = cleanup_recorder(oplog.clone(), false, true).await;
            let replay = recorder.replay.clone();
            let ReplayStartClaimOutcome::Claimed { handle: send, .. } = replay.claim_start_or_replay_end(StartClaim::unowned(
                &golem_common::model::oplog::host_functions::P3HttpClientSend::HOST_FUNCTION_NAME,
                &DurableFunctionType::WriteRemote,
            )).await.unwrap() else {
                panic!("recorded send Start");
            };
            let driver = recorder
                .for_start(send_index, None, false)
                .record(close)
                .unwrap();
            // A cancelled driver must not lose the metadata read or cleanup claim: the direct
            // drain owns the same future and can run it without the HTTP continuation.
            drop(driver);
            let DropEvent::FinishP3HttpSpan { cleanup } = events.recv().await.unwrap() else {
                panic!("cleanup event");
            };
            assert!(matches!(
                cleanup.progress.await.unwrap(),
                HttpSpanCleanupProgress::Complete
            ));
            assert!(matches!(
                replay.await_resolution_outcome(send).await.unwrap(),
                ResolutionOutcome::Resolved(_)
            ));
            assert_eq!(
                oplog
                    .read_exact(OplogIndex::INITIAL, oplog.length().await)
                    .await,
                before
            );
            assert!(events.try_recv().is_err());
        }
    }

    #[test]
    #[test_r::timeout("30s")]
    async fn http_cleanup_receipt_survives_cancelled_drain_and_keeps_drop_time() {
        for cancel_before_actor_finishes in [true, false] {
            let (reached, mut reached_rx) = tokio::sync::mpsc::unbounded_channel();
            let gate = Arc::new(tokio::sync::Semaphore::new(0));
            let oplog = Arc::new(InMemoryOplog::with_end_gate(reached, gate.clone()));
            oplog.add(noop()).await;
            let (mut recorder, mut receiver) = cleanup_recorder(oplog.clone(), true, true).await;
            recorder.parent = Some(OplogIndex::from_u64(8));
            recorder.owner = Some(OplogIndex::from_u64(11));
            let sink = recorder.sink.clone();
            let closed = finished();
            assert!(recorder.record(closed.clone()).is_none());
            reached_rx.recv().await.unwrap();
            assert_eq!(
                oplog.length().await,
                1,
                "pair must not be partially visible"
            );
            let event = receiver.try_recv().unwrap();
            let mut drain = AccessDropEventDrainGuard::new(sink, vec![event]);
            assert!(drain.start_next());
            let DropEvent::FinishP3HttpSpan { cleanup } = drain.current_mut() else {
                panic!("expected cleanup");
            };
            if cancel_before_actor_finishes {
                assert!(futures::poll!(cleanup.progress.clone()).is_pending());
            } else {
                gate.add_permits(1);
                assert!(matches!(
                    cleanup.progress.clone().await.unwrap(),
                    HttpSpanCleanupProgress::Complete
                ));
            }
            drop(drain);
            if cancel_before_actor_finishes {
                gate.add_permits(1);
            }
            // This later writer also proves the submitted job progresses without a cleanup waiter.
            let later = oplog.add(noop()).await;
            let DropEvent::FinishP3HttpSpan { cleanup } = receiver.try_recv().unwrap() else {
                panic!("cancelled drain must retain cleanup");
            };
            assert!(matches!(
                cleanup.progress.clone().await.unwrap(),
                HttpSpanCleanupProgress::Complete
            ));
            assert!(receiver.try_recv().is_err());
            assert_eq!(later, OplogIndex::from_u64(4));
            let start = oplog.read(OplogIndex::from_u64(2)).await;
            assert!(matches!(start, OplogEntry::Start {
                timestamp, parent_start_index: Some(parent), observational_owner: Some(owner),
                function_name, durable_function_type: DurableFunctionType::ReadLocal, ..
            } if timestamp == closed.finished_at && parent == OplogIndex::from_u64(8)
                && owner == OplogIndex::from_u64(11) && function_name == P3HttpSpanCleanup::HOST_FUNCTION_NAME));
            let end = oplog.read(OplogIndex::from_u64(3)).await;
            assert!(
                matches!(end, OplogEntry::End { start_index, span_finished: Some(span), .. }
                if start_index == OplogIndex::from_u64(2) && span == closed)
            );
        }
    }

    #[test]
    #[test_r::timeout("30s")]
    async fn http_cleanup_replay_claim_survives_cancelled_drain() {
        for cancel_after_claim in [false, true] {
            let oplog = Arc::new(InMemoryOplog::new());
            oplog.add(noop()).await;
            let closed = finished();
            oplog
                .add(OplogEntry::Start {
                    timestamp: closed.finished_at,
                    parent_start_index: None,
                    observational_owner: None,
                    function_name: P3HttpSpanCleanup::HOST_FUNCTION_NAME,
                    invocation_id: None,
                    request: Some(OplogPayload::Inline(Box::new(
                        HostRequestP3HttpSpanCleanup {
                            send_start_index: OplogIndex::from_u64(17),
                        }
                        .into(),
                    ))),
                    durable_function_type: DurableFunctionType::ReadLocal,
                    span_started: None,
                })
                .await;
            oplog.add(noop()).await;
            oplog
                .add(OplogEntry::End {
                    timestamp: closed.finished_at,
                    start_index: OplogIndex::from_u64(2),
                    response: Some(OplogPayload::Inline(Box::new(
                        HostResponseGolemApiUnit { result: Ok(()) }.into(),
                    ))),
                    forced_commit: false,
                    span_finished: Some(closed.clone()),
                    span_attributes: None,
                })
                .await;
            let (recorder, mut receiver) = cleanup_recorder(oplog.clone(), false, true).await;
            let replay = recorder.replay.clone();
            let sink = recorder.sink.clone();
            let driver = recorder.record(closed).unwrap();
            let mut drain =
                AccessDropEventDrainGuard::new(sink, vec![receiver.try_recv().unwrap()]);
            assert!(drain.start_next());
            if cancel_after_claim {
                let DropEvent::FinishP3HttpSpan { cleanup } = drain.current_mut() else {
                    panic!("expected cleanup")
                };
                while replay.last_replayed_index() < OplogIndex::from_u64(2) {
                    assert!(futures::poll!(cleanup.progress.clone()).is_pending());
                    tokio::task::yield_now().await;
                }
            }
            drop(drain);
            let DropEvent::FinishP3HttpSpan { cleanup } = receiver.try_recv().unwrap() else {
                panic!("expected retained cleanup")
            };
            while replay.last_replayed_index() < OplogIndex::from_u64(2) {
                assert!(futures::poll!(cleanup.progress.clone()).is_pending());
                tokio::task::yield_now().await;
            }
            let (index, entry) = replay.get_oplog_entry().await.unwrap();
            assert_eq!(index, OplogIndex::from_u64(3));
            assert!(matches!(entry, OplogEntry::NoOp { .. }));
            assert!(matches!(
                cleanup.progress.clone().await.unwrap(),
                HttpSpanCleanupProgress::Complete
            ));
            assert_eq!(
                oplog.length().await,
                4,
                "replay must not append another cleanup"
            );
            drop(driver);
        }
    }

    #[test]
    #[test_r::timeout("30s")]
    async fn http_cleanup_driver_unblocks_an_existing_call_without_a_new_drain() {
        let oplog = Arc::new(InMemoryOplog::new());
        oplog.add(noop()).await;
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
            })
            .await;
        let closed = finished();
        let (live, mut live_events) = cleanup_recorder(oplog.clone(), true, true).await;
        assert!(live.record(closed.clone()).is_none());
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
            .await;
        let (recorder, mut events) = cleanup_recorder(oplog.clone(), false, true).await;
        let replay = recorder.replay.clone();
        let ReplayStartClaimOutcome::Claimed { handle, .. } = replay
            .claim_start_or_replay_end(StartClaim::unowned(
                &other_name,
                &DurableFunctionType::ReadRemote,
            ))
            .await
            .unwrap()
        else {
            panic!("expected existing call Start");
        };
        let driver = recorder.record(closed).unwrap();
        // Drive exactly the Store task's Store-independent work. The cleanup queue remains untouched.
        let driving = tokio::spawn(async move {
            let result = driver.progress.await;
            drop(driver.activity);
            result
        });
        let result = replay.await_resolution_outcome(handle).await.unwrap();
        assert!(
            matches!(result, ResolutionOutcome::Resolved(Resolution::Completed { end_idx, .. }) if end_idx == other_end)
        );
        assert!(matches!(
            driving.await.unwrap().unwrap(),
            HttpSpanCleanupProgress::Complete
        ));
        let DropEvent::FinishP3HttpSpan { cleanup } = events.try_recv().unwrap() else {
            panic!("cleanup must remain joinable")
        };
        assert!(matches!(
            cleanup.progress.await.unwrap(),
            HttpSpanCleanupProgress::Complete
        ));
        assert_eq!(oplog.length().await, 5);
    }

    #[test]
    #[test_r::timeout("30s")]
    async fn http_cleanup_incomplete_repair_preserves_start_and_drop_timestamp() {
        let oplog = Arc::new(InMemoryOplog::new());
        oplog.add(noop()).await;
        let recorded = finished();
        let index = oplog
            .add(OplogEntry::Start {
                timestamp: recorded.finished_at,
                parent_start_index: None,
                observational_owner: None,
                function_name: P3HttpSpanCleanup::HOST_FUNCTION_NAME,
                invocation_id: None,
                request: Some(OplogPayload::Inline(Box::new(
                    HostRequestP3HttpSpanCleanup {
                        send_start_index: OplogIndex::from_u64(17),
                    }
                    .into(),
                ))),
                durable_function_type: DurableFunctionType::ReadLocal,
                span_started: None,
            })
            .await;
        let (recorder, mut events) = cleanup_recorder(oplog.clone(), false, true).await;
        let replay = recorder.replay.clone();
        let mut closed = recorded.clone();
        closed.finished_at = "2026-04-05T10:11:12Z".parse().unwrap();
        let driver = recorder.record(closed).unwrap();
        let DropEvent::FinishP3HttpSpan { cleanup } = events.try_recv().unwrap() else {
            panic!("expected cleanup")
        };
        let HttpSpanCleanupProgress::ContinueLive {
            existing,
            replay_ended,
        } = cleanup.progress.clone().await.unwrap()
        else {
            panic!("incomplete cleanup must require live transition")
        };
        assert_eq!(existing, Some((index, recorded.finished_at)));
        assert!(replay_ended);
        assert_eq!(oplog.length().await, 2, "no append before live transition");
        let memory = crate::services::linear_memory::LinearMemoryTracker::new(
            2,
            2,
            golem_common::model::agent::AgentMode::Durable,
            true,
            Arc::new(crate::services::resource_limits::AtomicResourceEntry::new(
                0, 10, 0, 0, 0,
            )),
            Arc::new(std::sync::Mutex::new(
                crate::services::active_agents::MemoryGrant::inert(2),
            )),
            std::time::Instant::now(),
        );
        replay
            .switch_to_live(&memory, ReplayToLiveRole::PrimaryAgent)
            .await
            .unwrap();
        assert!(matches!(
            cleanup
                .recorder
                .append(&cleanup.finished, existing)
                .await
                .unwrap(),
            HttpSpanCleanupProgress::Complete
        ));
        let end = oplog.read(OplogIndex::from_u64(3)).await;
        assert!(
            matches!(end, OplogEntry::End { start_index, span_finished: Some(span), .. }
            if start_index == index && span == recorded)
        );
        assert_eq!(oplog.length().await, 3);
        drop(driver);
    }

    #[test]
    async fn http_cleanup_snapshot_provenance_suppresses_live_and_replay_work() {
        for live in [true, false] {
            let oplog = Arc::new(InMemoryOplog::new());
            oplog.add(noop()).await;
            let (recorder, mut receiver) = cleanup_recorder(oplog.clone(), live, false).await;
            assert!(recorder.record(finished()).is_none());
            let DropEvent::FinishP3HttpSpan { cleanup } = receiver.try_recv().unwrap() else {
                panic!("expected cleanup")
            };
            assert!(matches!(
                cleanup.progress.clone().await.unwrap(),
                HttpSpanCleanupProgress::Complete
            ));
            assert_eq!(oplog.length().await, 1);
        }
    }

    fn marker(receipt: MarkerReceipt) -> DropEvent {
        DropEvent::AwaitCompletionMarker {
            receipt: Some(receipt),
            trap_context: DurableCallTrapContext {
                retry_from: OplogIndex::INITIAL,
                in_atomic_region: false,
            },
            live_call_permit: None,
        }
    }

    #[test]
    fn successful_markers_release_only_their_own_live_permits() {
        let counter = Arc::new(AtomicUsize::new(0));
        let _rpc = LiveCallPermit::new(counter.clone());
        let unsafe_call = LiveCallPermit::new(counter.clone());
        let (sender, receiver) = tokio::sync::oneshot::channel();
        let mut event = marker(MarkerReceipt::pending(receiver));
        if let DropEvent::AwaitCompletionMarker {
            live_call_permit, ..
        } = &mut event
        {
            *live_call_permit = Some(LiveCallPermit::new(counter.clone()));
        }
        let mut events = VecDeque::from([event]);
        assert!(!settle_completed_marker_events(&mut events));
        assert_eq!(counter.load(Ordering::Acquire), 3);
        sender.send(Ok(())).unwrap();
        assert!(settle_completed_marker_events(&mut events));
        assert_eq!(counter.load(Ordering::Acquire), 2);
        assert!(
            !crate::durable_host::PrivateDurableWorkerState::suspend_admissible(
                counter.load(Ordering::Acquire),
                1,
                false,
                false,
            )
        );
        drop(unsafe_call);
        assert!(
            crate::durable_host::PrivateDurableWorkerState::suspend_admissible(
                counter.load(Ordering::Acquire),
                1,
                false,
                false,
            )
        );

        events.push_back(DropEvent::AwaitCompletionMarker {
            receipt: None,
            trap_context: DurableCallTrapContext {
                retry_from: OplogIndex::INITIAL,
                in_atomic_region: false,
            },
            live_call_permit: Some(LiveCallPermit::new(counter.clone())),
        });
        assert!(settle_completed_marker_events(&mut events));
        assert_eq!(counter.load(Ordering::Acquire), 1);
    }

    #[test]
    fn settlement_scans_past_other_events_and_preserves_retained_order() {
        let span1 = SpanId::generate();
        let span2 = SpanId::generate();
        let span3 = SpanId::generate();
        let (success_sender, success_receiver) = tokio::sync::oneshot::channel();
        success_sender.send(Ok(())).unwrap();
        let (failure_sender, failure_receiver) = tokio::sync::oneshot::channel();
        failure_sender
            .send(Err(WorkerExecutorError::runtime("failed marker")))
            .unwrap();
        let mut events = VecDeque::from([
            DropEvent::FinishSpan {
                span_id: span1.clone(),
            },
            marker(MarkerReceipt::pending(success_receiver)),
            DropEvent::FinishSpan {
                span_id: span2.clone(),
            },
            marker(MarkerReceipt::pending(failure_receiver)),
            DropEvent::FinishSpan {
                span_id: span3.clone(),
            },
        ]);

        assert!(!settle_completed_marker_events(&mut events));
        assert_eq!(events.len(), 4);
        assert!(
            matches!(events.pop_front(), Some(DropEvent::FinishSpan { span_id, .. }) if span_id == span1)
        );
        assert!(
            matches!(events.pop_front(), Some(DropEvent::FinishSpan { span_id, .. }) if span_id == span2)
        );
        assert!(matches!(
            events.pop_front(),
            Some(DropEvent::AwaitCompletionMarker {
                live_call_permit: None,
                ..
            })
        ));
        assert!(
            matches!(events.pop_front(), Some(DropEvent::FinishSpan { span_id, .. }) if span_id == span3)
        );
    }
}
