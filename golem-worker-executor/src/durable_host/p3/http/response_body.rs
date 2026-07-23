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

use super::rebuild::resend_recorded_request;
use super::rebuild::{AbortOnDropIoTask, P3HttpSendRebuild};
use super::rebuild::{RebuildOutcome, ResendOutcome, reissue_recorded_request};
use super::serialization::{deserialize_error_code, serialize_error_code};
use super::serialization::{deserialize_headers, serialize_headers};
use super::*;
use crate::durable_host::concurrent::{
    AccessClaimOptions, CallHandle, Cancellable, CompletionDelivery, DeferredCallReplayOutcome,
    DropEvent, NotCancellable,
};
use crate::durable_host::durability::{
    AsyncRetryDecision, DurabilityHost, DurableCallTrapContext, HostFailureKind,
    InFunctionRetryState, TaskRetryContext, mark_durable_call_trap_context,
};
use crate::durable_host::http::inline_retry::parse_content_range_start;
use crate::durable_host::http::types::classify_serializable_http_error_code;
use crate::durable_host::p3::{
    DurableP3, DurableP3View, durable_worker_ctx, observe_function_call,
    observe_function_call_store, wasi_http_view,
};
use crate::durable_host::tail_work::TailActivity;
use crate::workerctx::WorkerCtx;
use bytes::Bytes;
use golem_common::model::RetryContext;
use golem_common::model::oplog::host_functions::{
    P3HttpClientConsumeBody, P3HttpClientConsumeBodyChunk,
};
use golem_common::model::oplog::payload::types::{
    SerializableP3HttpBodyChunk, SerializableP3HttpConsumeBodyResult,
};
use golem_common::model::oplog::{
    DurableFunctionType, HostRequestNoInput, HostResponseP3HttpClientConsumeBodyChunk,
    HostResponseP3HttpClientConsumeBodyResult,
};
use golem_service_base::error::worker_executor::WorkerExecutorError;
use http::HeaderMap;
use http_body_util::BodyExt as _;
use http_body_util::combinators::UnsyncBoxBody;
use std::future::Future;
use std::marker::PhantomData;
use std::pin::Pin;
use std::task::{Context, Poll};
use tokio::sync::{mpsc, oneshot};
use tracing::debug;
use wasmtime::component::{
    Access, Accessor, AccessorTask, Destination, FutureProducer, FutureReader, Resource,
    StreamProducer, StreamReader, StreamResult,
};
use wasmtime::{AsContextMut, StoreContextMut};
use wasmtime_wasi_http::FieldMap;
use wasmtime_wasi_http::p3::bindings::http::types;
use wasmtime_wasi_http::p3::bindings::http::types::{
    ErrorCode, Headers, Response, StatusCode, Trailers,
};
use wasmtime_wasi_http::p3::{HostBodyStreamProducer, WasiHttp, WasiHttpView};

pub(crate) struct OpenP3HttpResponseState {
    /// The `outgoing-http-request` invocation span of the send that produced
    /// this response. Finished when the response body completes (the durable
    /// consume-body terminal) or via a deferred [`DropEvent::FinishSpan`] when
    /// the response is dropped unconsumed.
    pub(crate) span: P3HttpSendSpan,
    /// Request method, for retry properties of body-transfer failures.
    pub(crate) method: String,
    /// Request URI, for retry properties of body-transfer failures.
    pub(crate) uri: String,
    /// Effective idempotence of the request (the worker-level
    /// `assume_idempotence` override, or an idempotent HTTP method), applied
    /// to the retry properties of body-transfer failures.
    pub(super) is_idempotent: bool,
    /// How to re-send the recorded request without a new guest-visible call.
    /// Used to re-issue the request when a replayed placeholder body's
    /// durable consume-body scope turns out to be incomplete and must
    /// re-execute live, and to resume a failed live body read via a ranged
    /// re-send. Populated on both the live and replay send paths.
    pub(crate) resend: Option<P3HttpSendRebuild>,
    /// True iff the response was replayed from recorded headers and carries
    /// an empty placeholder body: the first live body read must re-issue the
    /// recorded request (via `resend`) to obtain a real body.
    pub(crate) body_is_placeholder: bool,
}

/// The send's `outgoing-http-request` invocation-context span together with
/// how it must be finished.
///
/// Spans of sends recorded by the current executor are *derived*: the span id
/// is a deterministic (UUIDv5-based) function of the send's own host-call
/// `Start` index, so no `StartSpan`/`FinishSpan` oplog entries exist for them
/// and finishing is in-memory only. Positional span entries are unsound under
/// concurrent sends — overlapping sends interleave their entries live and
/// consume each other's on replay — which is why the span identity is derived
/// from the (claim-based, order-independent) durable `Start` instead.
///
/// Spans reconstructed from a legacy positional `StartSpan` entry (oplogs
/// written by older executors) keep the legacy durable finish: the matching
/// positional `FinishSpan` is consumed on replay, or appended when the worker
/// has switched to live, exactly as the recording executor would have done.
pub(super) fn register_open_response<Ctx: WorkerCtx, U: Send>(
    store: &Accessor<U, DurableP3<Ctx>>,
    response: &Resource<Response>,
    state: OpenP3HttpResponseState,
) {
    let rep = response.rep();
    store.with(|mut access| {
        let ctx = durable_worker_ctx::<Ctx, U>(access.data_mut());
        ctx.state.open_p3_http_responses.insert(rep, state);
    });
}

/// Whether the recorded request head declares a request body: a positive (or
/// unparseable) `content-length`, or any `transfer-encoding`. The oplog does
/// not record request body bytes, so such a request cannot be faithfully
/// re-issued after a restart. This is best-effort detection from the head
/// only — a streamed upload without `content-length` is indistinguishable
/// from no body and slips through.
impl<Ctx: WorkerCtx> types::HostResponse for DurableP3View<'_, Ctx> {
    fn get_status_code(&mut self, res: Resource<Response>) -> wasmtime::Result<StatusCode> {
        observe_function_call(&*self.0, "http::types::response", "get-status-code");
        types::HostResponse::get_status_code(&mut WasiHttpView::http(self.0), res)
    }

    fn set_status_code(
        &mut self,
        res: Resource<Response>,
        status_code: StatusCode,
    ) -> wasmtime::Result<Result<(), ()>> {
        observe_function_call(&*self.0, "http::types::response", "set-status-code");
        types::HostResponse::set_status_code(&mut WasiHttpView::http(self.0), res, status_code)
    }

    fn get_headers(&mut self, res: Resource<Response>) -> wasmtime::Result<Resource<Headers>> {
        observe_function_call(&*self.0, "http::types::response", "get-headers");
        types::HostResponse::get_headers(&mut WasiHttpView::http(self.0), res)
    }
}

/// Result fed to the guest-facing trailers `FutureReader` once the body closes.
pub(super) type HttpTrailersOutcome = Result<Option<HeaderMap>, ErrorCode>;

/// A demand from the body stream producer to the durable [`HttpConsumeBodyTask`].
pub(super) enum HttpBodyDemand {
    /// Read and durably persist the next body chunk, replying on the channel.
    Read {
        reply: oneshot::Sender<HttpBodyChunkReply>,
        cancel: oneshot::Receiver<()>,
        cancel_ack: oneshot::Sender<()>,
    },
    /// The guest dropped/cancelled the stream with no read in flight. Finalize
    /// the durable consume-body parent without reading more upstream bytes, then
    /// acknowledge so the guest invocation cannot return with an incomplete
    /// parent scope in the oplog.
    Cancel(oneshot::Sender<()>),
}

/// The task's reply to a single producer demand.
pub(super) enum HttpBodyChunkReply {
    /// One non-empty body frame, already persisted to the oplog as a `Data`
    /// child chunk before being handed back for delivery to the guest.
    Data(Bytes),
    /// The body stream reached its terminal (clean EOF, trailers, or a body
    /// error); there are no more bytes to deliver. The producer signals `ack`
    /// immediately before it reports EOF to the guest, so the durable task only
    /// resolves trailers (and finalizes the parent marker) once the terminal has
    /// actually been observed by the guest-facing stream.
    End { ack: oneshot::Sender<()> },
    /// The guest cancelled this pending body read before upstream bytes arrived.
    Cancelled,
    /// A durable failure occurred while persisting/replaying the body; the guest
    /// stream traps with this message, tagged with the failing call scope's trap
    /// context so post-trap retry grouping stays owned by that call.
    Failed {
        message: String,
        trap_context: DurableCallTrapContext,
    },
}

/// Resolution delivered to the guest-facing trailers future once the body closes
/// (or the durable task fails before recording the terminal).
pub(super) enum HttpTrailersResolution {
    /// The body terminal: clean trailers (or a body `ErrorCode`).
    Outcome(HttpTrailersOutcome),
    /// A durability failure: the trailers future traps with this message, tagged
    /// with the failing call scope's trap context.
    Trap {
        message: String,
        trap_context: DurableCallTrapContext,
    },
}

/// Body stream returned to the guest from `consume-body`.
///
/// `consume-body` is a *synchronous* host function but durable persistence is
/// async, so the producer never touches the oplog (or the upstream body)
/// itself. Instead it bridges to the spawned [`HttpConsumeBodyTask`] with a
/// demand/reply protocol: when the guest needs more bytes the producer sends a
/// demand and parks; the task reads (live) or replays (on replay) exactly one
/// body frame, persists/claims it as a child durable call, and replies with the
/// bytes. The whole frame is then handed to the runtime's buffer
/// (`Destination::set_buffer`), which delivers it across however many guest
/// reads and only calls `poll_produce` again once it is fully drained — so
/// exactly one child chunk is produced per real demand, identically live and on
/// replay.
pub(super) struct DurableHttpBodyProducer {
    demand_tx: mpsc::Sender<HttpBodyDemand>,
    pending: Option<PendingHttpBodyRead>,
    pending_cancel: Option<oneshot::Receiver<()>>,
    finished: bool,
}

pub(super) struct PendingHttpBodyRead {
    reply: oneshot::Receiver<HttpBodyChunkReply>,
    cancel: Option<oneshot::Sender<()>>,
    cancel_ack: Option<oneshot::Receiver<()>>,
    cancelling: bool,
}

impl DurableHttpBodyProducer {
    fn new(demand_tx: mpsc::Sender<HttpBodyDemand>) -> Self {
        Self {
            demand_tx,
            pending: None,
            pending_cancel: None,
            finished: false,
        }
    }
}

impl<D> StreamProducer<D> for DurableHttpBodyProducer {
    type Item = u8;
    type Buffer = Bytes;

    fn poll_produce<'a>(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        mut store: StoreContextMut<'a, D>,
        mut dst: Destination<'a, Self::Item, Self::Buffer>,
        finish: bool,
    ) -> Poll<wasmtime::Result<StreamResult>> {
        loop {
            if self.finished {
                return Poll::Ready(Ok(StreamResult::Dropped));
            }

            if let Some(rx) = self.pending_cancel.as_mut() {
                match Pin::new(rx).poll(cx) {
                    Poll::Pending => return Poll::Pending,
                    Poll::Ready(_) => {
                        self.pending_cancel = None;
                        self.finished = true;
                        return Poll::Ready(Ok(StreamResult::Cancelled));
                    }
                }
            }

            if let Some(pending) = self.pending.as_mut() {
                if finish && !pending.cancelling {
                    if let Some(cancel) = pending.cancel.take() {
                        let _ = cancel.send(());
                    }
                    pending.cancelling = true;
                }
                if pending.cancelling
                    && let Some(cancel_ack) = pending.cancel_ack.as_mut()
                {
                    match Pin::new(cancel_ack).poll(cx) {
                        Poll::Ready(_) => {
                            self.pending = None;
                            self.finished = true;
                            return Poll::Ready(Ok(StreamResult::Cancelled));
                        }
                        Poll::Pending => {}
                    }
                }
                match Pin::new(&mut pending.reply).poll(cx) {
                    Poll::Pending => {
                        // A demand is in flight. If `finish` was set above, the
                        // durable task has also been signalled to stop the
                        // upstream read and record a terminal, so this pending
                        // wait is bounded by durable finalization rather than by
                        // the remote peer producing more body bytes.
                        return Poll::Pending;
                    }
                    Poll::Ready(Ok(HttpBodyChunkReply::Data(bytes))) => {
                        self.pending = None;
                        if bytes.is_empty() {
                            continue;
                        }
                        // Hand the whole frame to the runtime; it delivers it
                        // across as many guest reads as needed and only calls
                        // us again once it is drained.
                        dst.set_buffer(bytes);
                        return Poll::Ready(Ok(StreamResult::Completed));
                    }
                    Poll::Ready(Ok(HttpBodyChunkReply::End { ack })) => {
                        let cancelling = pending.cancelling;
                        let cancel_ack = pending.cancel_ack.take();
                        self.pending = None;
                        // Acknowledge the terminal *before* reporting EOF so the
                        // task only resolves trailers after this stream observes
                        // the terminal. A dropped `ack` receiver just means the
                        // task is already gone, which is harmless here.
                        let _ = ack.send(());
                        if cancelling {
                            if let Some(cancel_ack) = cancel_ack {
                                self.pending_cancel = Some(cancel_ack);
                                continue;
                            }
                            self.finished = true;
                            return Poll::Ready(Ok(StreamResult::Cancelled));
                        } else {
                            self.finished = true;
                            return Poll::Ready(Ok(StreamResult::Dropped));
                        }
                    }
                    Poll::Ready(Ok(HttpBodyChunkReply::Cancelled)) => {
                        let cancel_ack = pending.cancel_ack.take();
                        self.pending = None;
                        if let Some(cancel_ack) = cancel_ack {
                            self.pending_cancel = Some(cancel_ack);
                            continue;
                        }
                        self.finished = true;
                        return Poll::Ready(Ok(StreamResult::Cancelled));
                    }
                    Poll::Ready(Ok(HttpBodyChunkReply::Failed {
                        message,
                        trap_context,
                    })) => {
                        self.pending = None;
                        self.finished = true;
                        return Poll::Ready(Err(wasmtime::Error::from_anyhow(
                            mark_durable_call_trap_context(
                                anyhow::Error::msg(message),
                                trap_context,
                            ),
                        )));
                    }
                    Poll::Ready(Err(_)) => {
                        self.finished = true;
                        return Poll::Ready(Err(wasmtime::Error::msg(
                            "consume-body durable task dropped before replying",
                        )));
                    }
                }
            }

            // No demand in flight.
            if dst.remaining(&mut store) == Some(0) {
                // Zero-length read: the guest is probing readiness, not reading.
                // Do not turn this into a durable body read.
                return Poll::Ready(Ok(StreamResult::Completed));
            }
            if finish {
                // The guest is cancelling the stream and we have nothing
                // buffered and no demand in flight. Ask the durable task to
                // finalize the parent scope and wait for that acknowledgement;
                // otherwise a component that drops the body and returns
                // immediately can leave an incomplete consume-body scope in the
                // oplog and fail on replay.
                let (tx, rx) = oneshot::channel();
                match self.demand_tx.try_send(HttpBodyDemand::Cancel(tx)) {
                    Ok(()) => {}
                    Err(mpsc::error::TrySendError::Closed(_)) => {
                        self.finished = true;
                        return Poll::Ready(Ok(StreamResult::Cancelled));
                    }
                    Err(mpsc::error::TrySendError::Full(_)) => {
                        // The producer keeps at most one demand in flight, so the
                        // capacity-1 channel can never be full here.
                        self.finished = true;
                        return Poll::Ready(Err(wasmtime::Error::msg(
                            "consume-body demand channel unexpectedly full",
                        )));
                    }
                }
                self.pending_cancel = Some(rx);
                continue;
            }

            let (reply_tx, reply_rx) = oneshot::channel();
            let (cancel_tx, cancel_rx) = oneshot::channel();
            let (cancel_ack_tx, cancel_ack_rx) = oneshot::channel();
            match self.demand_tx.try_send(HttpBodyDemand::Read {
                reply: reply_tx,
                cancel: cancel_rx,
                cancel_ack: cancel_ack_tx,
            }) {
                Ok(()) => {}
                Err(mpsc::error::TrySendError::Closed(_)) => {
                    self.finished = true;
                    return Poll::Ready(Err(wasmtime::Error::msg(
                        "consume-body durable task is gone",
                    )));
                }
                Err(mpsc::error::TrySendError::Full(_)) => {
                    // The producer keeps at most one demand in flight, so the
                    // capacity-1 channel can never be full here.
                    self.finished = true;
                    return Poll::Ready(Err(wasmtime::Error::msg(
                        "consume-body demand channel unexpectedly full",
                    )));
                }
            }
            self.pending = Some(PendingHttpBodyRead {
                reply: reply_rx,
                cancel: Some(cancel_tx),
                cancel_ack: Some(cancel_ack_rx),
                cancelling: false,
            });
            // Loop to register the receiver's waker (the reply is not ready yet).
        }
    }
}

/// Guest-facing trailers `FutureReader` producer. Awaits the terminal trailers
/// from the durable task and, only when read, materializes a `trailers`
/// resource in the store table.
pub(super) struct HttpTrailersFutureProducer<Ctx, U> {
    rx: oneshot::Receiver<HttpTrailersResolution>,
    _phantom: PhantomData<fn() -> (Ctx, U)>,
}

impl<Ctx, U> HttpTrailersFutureProducer<Ctx, U> {
    fn new(rx: oneshot::Receiver<HttpTrailersResolution>) -> Self {
        Self {
            rx,
            _phantom: PhantomData,
        }
    }
}

impl<Ctx, U> FutureProducer<U> for HttpTrailersFutureProducer<Ctx, U>
where
    Ctx: WorkerCtx,
    U: 'static,
{
    type Item = Result<Option<Resource<Trailers>>, ErrorCode>;

    fn poll_produce(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        mut store: StoreContextMut<U>,
        finish: bool,
    ) -> Poll<wasmtime::Result<Option<Self::Item>>> {
        let this = self.get_mut();
        match Pin::new(&mut this.rx).poll(cx) {
            Poll::Pending if finish => Poll::Ready(Ok(None)),
            Poll::Pending => Poll::Pending,
            Poll::Ready(Ok(HttpTrailersResolution::Outcome(outcome))) => {
                let item = match outcome {
                    Ok(None) => Ok(None),
                    Ok(Some(headers)) => {
                        let view = wasi_http_view::<Ctx, U>(store.data_mut());
                        match view.table.push(FieldMap::new_immutable(headers)) {
                            Ok(resource) => Ok(Some(resource)),
                            Err(err) => {
                                return Poll::Ready(Err(wasmtime::Error::from(err)
                                    .context("failed to push consume-body trailers to table")));
                            }
                        }
                    }
                    Err(error) => Err(error),
                };
                Poll::Ready(Ok(Some(item)))
            }
            // A durability failure occurred before the terminal was recorded: the
            // trailers future must trap (carrying the failing call scope's trap
            // context) rather than resolve to a normal error that would mask it.
            Poll::Ready(Ok(HttpTrailersResolution::Trap {
                message,
                trap_context,
            })) => Poll::Ready(Err(wasmtime::Error::from_anyhow(
                mark_durable_call_trap_context(anyhow::Error::msg(message), trap_context),
            ))),
            // The channel is closed without any resolution: the durable task was
            // aborted before sending. On the normal path the task always sends a
            // resolution before dropping the sender, so a closed channel here is
            // a durability failure and must trap rather than resolve to a normal
            // error that would mask it.
            Poll::Ready(Err(_)) => Poll::Ready(Err(wasmtime::Error::msg(
                "consume-body durable task dropped before resolving trailers",
            ))),
        }
    }
}

pub(super) fn serialize_consume_body_result(
    result: &Result<Option<HeaderMap>, ErrorCode>,
) -> SerializableP3HttpConsumeBodyResult {
    match result {
        Ok(trailers) => {
            SerializableP3HttpConsumeBodyResult::Trailers(trailers.as_ref().map(serialize_headers))
        }
        Err(error) => SerializableP3HttpConsumeBodyResult::HttpError(serialize_error_code(error)),
    }
}

pub(super) fn deserialize_consume_body_result(
    result: SerializableP3HttpConsumeBodyResult,
) -> Result<Option<HeaderMap>, ErrorCode> {
    match result {
        SerializableP3HttpConsumeBodyResult::Trailers(trailers) => {
            Ok(trailers.map(deserialize_headers))
        }
        SerializableP3HttpConsumeBodyResult::HttpError(error) => Err(deserialize_error_code(error)),
    }
}

/// Fail the durable `consume-body` task loudly on a durability-machinery error
/// (an oplog read/write failure), as opposed to a normal HTTP body error.
///
/// A durability failure must not be turned into a normal terminal: doing so
/// would commit a completed parent marker sitting after an incomplete child
/// chunk (a malformed oplog). Instead we return `Err` from the task, which the
/// runtime surfaces as a trap. The parent batched scope is left without a
/// terminal marker (the caller abandons/traps the parent handle so a `Cancelled`
/// is never written), so on replay the worker recovers from the incomplete
/// `Start` rather than observing committed-but-corrupt durable state.
///
/// The `error` must already carry the failing call's [`DurableCallTrapContext`]
/// (via `CallHandle::trap`, a `TerminalCallError`, or `mark_durable_call_trap_context`)
/// so post-trap retry grouping stays owned by that call's scope; this helper does
/// not stringify it for the returned trap.
///
/// The guest-facing trailers future is resolved with a [`HttpTrailersResolution::Trap`]
/// carrying `trap_context` (the failing call scope's context) so it also fails
/// loud — with correct retry grouping — instead of resolving to a normal error
/// that would mask the durability failure. When `trap_context` is `None` (no
/// owning call scope exists yet) the sender is dropped, which still traps the
/// trailers future loudly.
pub(super) fn fail_consume_body_task(
    trailers_tx: oneshot::Sender<HttpTrailersResolution>,
    error: wasmtime::Error,
    trap_context: Option<DurableCallTrapContext>,
) -> wasmtime::Result<()> {
    match trap_context {
        Some(trap_context) => {
            // The detailed cause is preserved in the returned (marked) task error;
            // give the guest-facing trailers trap a clear, stable message rather
            // than re-displaying the trap-context marker carried by `error`.
            let _ = trailers_tx.send(HttpTrailersResolution::Trap {
                message: "consume-body durable persistence failed".to_string(),
                trap_context,
            });
        }
        None => drop(trailers_tx),
    }
    Err(error)
}

/// One unit read from the upstream response body by the durable task.
pub(super) enum HttpBodyFrame {
    /// A non-empty data frame.
    Data(Bytes),
    /// The body closed cleanly, optionally delivering trailers.
    End(Option<HeaderMap>),
    /// The body transfer errored.
    Error(ErrorCode),
    /// The guest cancelled an already-pending body read before upstream bytes
    /// arrived. This is persisted distinctly from EOF so replay can complete the
    /// guest read with cancellation instead of delivering a synthetic terminal.
    Cancelled,
}

/// One item produced by a single iteration of the durable consume-body loop —
/// after the chunk has been persisted (live) or replayed (replay) — to be
/// delivered to the guest-facing body stream.
pub(super) enum ProducedChunk {
    /// A non-empty body chunk to hand to the guest.
    Data(Bytes),
    /// The recorded stream's terminal: there are no more chunks to deliver.
    End,
    /// A pending guest read was cancelled; finalize durability without
    /// delivering EOF to the guest-facing stream.
    Cancelled,
}

/// Outcome of [`transfer_data_chunk`]: whether the persisted `Data` chunk reached the guest.
enum DataChunkTransfer {
    /// The chunk was sent to the guest and counted into `delivered_bytes`.
    Delivered,
    /// The guest abandoned the body reader before delivery — either live (the demand receiver
    /// was gone; the child's `CompletionDiscarded` marker is now durable) or on replay (the
    /// recorded run discarded the chunk; the task parked until the replayed guest dropped its
    /// in-flight read). The body must finalize as abandoned, with a clean `Ok(None)` terminal.
    Abandoned,
}

/// Parks at the delivery boundary of a replay-discarded completion: the recorded run persisted
/// this child but the guest dropped the body reader before it was delivered (the child's marker
/// records the discard). Never re-deliver the recorded completion — wait until the replayed
/// guest drops its in-flight read at the same point it did live, then finalize the body exactly
/// as the recorded run did after its failed delivery.
async fn park_replay_discarded_delivery(
    activity: &TailActivity,
    mut demand: oneshot::Sender<HttpBodyChunkReply>,
) {
    debug!(
        "recorded consume-body completion was discarded before delivery; parking until the \
         replayed guest drops the body reader"
    );
    activity.park(demand.closed()).await;
    drop(demand);
}

/// The guest-facing transfer of one persisted (or replayed) `Data` chunk — the single fallible
/// boundary between the child's durable `End` and the guest actually receiving the bytes. Owns
/// the chunk's deferred-delivery token:
///
/// - a successful send is `delivered` and advances `delivered_bytes` (a later resume's `Range`
///   offset must count only bytes the guest received);
/// - a closed demand receiver — an ordinary abandonment race (e.g. a guest-side timeout won
///   between the child `End` and this send), not corruption — records the child's
///   `CompletionDiscarded` marker and returns only once it is durable, so replay never
///   re-delivers the persisted chunk to a guest that did not receive it live;
/// - a replay-discarded child is never re-sent: the task parks until the replayed guest drops
///   its in-flight read at the same point it did live.
///
/// Fails only when the marker append itself fails; error conversion and parent finalization
/// stay with the caller.
async fn transfer_data_chunk(
    activity: &TailActivity,
    demand: oneshot::Sender<HttpBodyChunkReply>,
    bytes: Bytes,
    delivery: CompletionDelivery,
    delivered_bytes: &mut u64,
) -> Result<DataChunkTransfer, WorkerExecutorError> {
    if delivery.is_replay_discarded() {
        park_replay_discarded_delivery(activity, demand).await;
        return Ok(DataChunkTransfer::Abandoned);
    }
    let chunk_len = bytes.len() as u64;
    if demand.send(HttpBodyChunkReply::Data(bytes)).is_ok() {
        *delivered_bytes += chunk_len;
        delivery.delivered();
        Ok(DataChunkTransfer::Delivered)
    } else {
        debug!(
            "consume-body chunk persisted but the guest dropped the body reader before \
             delivery; finalizing the body as abandoned"
        );
        delivery.discarded().await?;
        Ok(DataChunkTransfer::Abandoned)
    }
}

/// Reads the next meaningful frame from the upstream body, skipping empty data
/// frames so an empty frame is never persisted/delivered as a body chunk.
pub(super) async fn read_http_body_frame(
    body: &mut UnsyncBoxBody<Bytes, ErrorCode>,
) -> HttpBodyFrame {
    loop {
        match body.frame().await {
            Some(Ok(frame)) => match frame.into_data() {
                Ok(data) => {
                    if data.is_empty() {
                        continue;
                    }
                    return HttpBodyFrame::Data(data);
                }
                Err(frame) => match frame.into_trailers() {
                    Ok(trailers) => return HttpBodyFrame::End(Some(trailers)),
                    Err(_) => return HttpBodyFrame::Error(ErrorCode::HttpProtocolError),
                },
            },
            Some(Err(err)) => return HttpBodyFrame::Error(err),
            None => return HttpBodyFrame::End(None),
        }
    }
}

/// Why skipping the already-delivered prefix of a re-sent full response failed.
pub(super) enum SkipBodyPrefixError {
    /// The fresh body ended before the prefix was fully skipped: the resource
    /// shrank since the original response, so the delivered bytes cannot be
    /// continued.
    BodyTooShort,
    /// Reading the fresh body failed while skipping.
    Read(ErrorCode),
}

/// Reads and discards the first `prefix` bytes of a fresh body (count-only, no
/// content verification — P2 parity), returning the remainder of a data frame
/// that straddled the prefix boundary, if any.
pub(super) async fn skip_body_prefix(
    body: &mut UnsyncBoxBody<Bytes, ErrorCode>,
    prefix: u64,
) -> Result<Option<Bytes>, SkipBodyPrefixError> {
    let mut remaining = prefix;
    while remaining > 0 {
        match read_http_body_frame(body).await {
            HttpBodyFrame::Data(mut data) => {
                if (data.len() as u64) > remaining {
                    return Ok(Some(data.split_off(remaining as usize)));
                }
                remaining -= data.len() as u64;
            }
            HttpBodyFrame::End(_) => return Err(SkipBodyPrefixError::BodyTooShort),
            HttpBodyFrame::Error(code) => return Err(SkipBodyPrefixError::Read(code)),
            HttpBodyFrame::Cancelled => {
                unreachable!("read_http_body_frame never produces Cancelled")
            }
        }
    }
    Ok(None)
}

/// Durable driver for a `consume-body` response stream.
///
/// Owns the upstream body and persists it **chunk-by-chunk** under a single
/// `consume-body` batched durable scope (mirroring the P2 incoming-body stream):
///
/// * the parent `P3HttpClientConsumeBody` call opens the batched scope and is
///   finalized last with a marker carrying the trailers / body-error terminal;
/// * every delivered body frame is persisted as a `P3HttpClientConsumeBodyChunk`
///   child (`Data`) before its bytes are handed to the guest;
/// * a final `End` child terminates the recorded stream so replay knows when to
///   stop reading children.
///
/// Each child is produced in response to exactly one producer demand, so on
/// replay the same number of children are read back from the oplog and delivered
/// in the same order — no whole-body buffering, bounded memory.
pub(super) struct HttpConsumeBodyTask<Ctx> {
    body: UnsyncBoxBody<Bytes, ErrorCode>,
    demand_rx: mpsc::Receiver<HttpBodyDemand>,
    trailers_tx: oneshot::Sender<HttpTrailersResolution>,
    /// Open-response state of the send that produced this response (its
    /// `outgoing-http-request` span, retry properties, and — for a replayed
    /// response — the send rebuild info), taken over from the response
    /// resource in `consume_body`. The span is finished (durably) right after
    /// the parent terminal, mirroring the P2 `end_http_request` span
    /// lifecycle. `None` for responses that did not come from `client::send`.
    response_state: Option<OpenP3HttpResponseState>,
    activity: TailActivity,
    _phantom: PhantomData<fn() -> Ctx>,
}

impl<Ctx> HttpConsumeBodyTask<Ctx> {
    fn new(
        body: UnsyncBoxBody<Bytes, ErrorCode>,
        demand_rx: mpsc::Receiver<HttpBodyDemand>,
        trailers_tx: oneshot::Sender<HttpTrailersResolution>,
        response_state: Option<OpenP3HttpResponseState>,
        activity: TailActivity,
    ) -> Self {
        Self {
            body,
            demand_rx,
            trailers_tx,
            response_state,
            activity,
            _phantom: PhantomData,
        }
    }
}

impl<Ctx, U> AccessorTask<U, DurableP3<Ctx>> for HttpConsumeBodyTask<Ctx>
where
    Ctx: WorkerCtx,
    U: Send + 'static,
{
    async fn run(self, accessor: &Accessor<U, DurableP3<Ctx>>) -> wasmtime::Result<()> {
        let HttpConsumeBodyTask {
            mut body,
            mut demand_rx,
            trailers_tx,
            response_state,
            activity,
            ..
        } = self;

        let (response_span, retry_properties, resume_context, resend, mut pending_reissue) =
            match response_state {
                Some(state) => {
                    let mut properties = RetryContext::http(&state.method, &state.uri);
                    apply_method_idempotence(&mut properties, state.is_idempotent);
                    (
                        Some(state.span),
                        Some(properties),
                        Some((state.method, state.uri, state.is_idempotent)),
                        state.resend,
                        state.body_is_placeholder,
                    )
                }
                None => (None, None, None, None, false),
            };
        // Keeps the re-issued request's I/O task alive while its body is read;
        // dropped (aborting the task) when this task finishes. Never read —
        // it exists only for its drop timing.
        let mut _rebuild_io_guard: Option<AbortOnDropIoTask> = None;
        // Set when a terminal error must not be routed through worker-level
        // retry: a refused rebuild (request body not reconstructable — a retry
        // would replay into the same refusal forever) or a content-changed
        // response-body resume (416 / short full response — deterministic for
        // the same request).
        let mut retry_exempt = false;
        // In-function retry budget of the response-body resume path, shared across all resume
        // attempts of this consume-body scope.
        let mut resume_retry_state = InFunctionRetryState::new();
        let mut resume_retry_ctx: Option<TaskRetryContext<Ctx>> = None;
        // Bytes delivered to the guest-facing stream so far (replayed chunks +
        // live frames): the resume offset for `Range` requests.
        let mut delivered_bytes: u64 = 0;

        // Open the parent batched scope. Children nest under its begin index.
        // Concurrently consumed response bodies open scopes with identical durable identity, so
        // the scope name is discriminated by the producing send's own `Start` index — recorded
        // oplog state that is identical on the live and replay paths and, unlike the derived
        // span id (a function of the owning agent's id), survives forking the oplog to another
        // agent. Responses that did not come from `client::send` have no span and keep the plain
        // scope name.
        let mut parent =
            match CallHandle::<P3HttpClientConsumeBody, Cancellable>::start_access_with_options(
                accessor,
                durable_worker_ctx::<Ctx, U>,
                DurableFunctionType::WriteRemoteBatched(None),
                AccessClaimOptions {
                    scope_discriminator: response_span
                        .as_ref()
                        .map(|span| format!("consume-body:{}", span.send_start_index)),
                    request_identity: None,
                },
                async |_| Ok(HostRequestNoInput {}),
            )
            .await
            {
                Ok(parent) => parent,
                // No parent handle exists yet, so there is nothing to abandon; the
                // `WorkerExecutorError` carries no call context but there is no scope
                // to group against either.
                Err(error) => {
                    return fail_consume_body_task(trailers_tx, wasmtime::Error::from(error), None);
                }
            };
        let parent_begin = parent.begin_index();

        // The trailers / body-error terminal, set on the live path; on replay it
        // is taken from the parent marker instead.
        let mut terminal: HttpTrailersOutcome = Ok(None);
        let mut cancel_ack: Option<oneshot::Sender<()>> = None;

        loop {
            // Safe park: waiting for the guest to demand the next body chunk.
            let (demand, cancel_rx, read_cancel_ack) = match activity.park(demand_rx.recv()).await {
                Some(HttpBodyDemand::Read {
                    reply,
                    cancel,
                    cancel_ack,
                }) => {
                    if reply.is_closed() {
                        break;
                    }
                    (reply, Some(cancel), Some(cancel_ack))
                }
                Some(HttpBodyDemand::Cancel(ack)) => {
                    cancel_ack = Some(ack);
                    break;
                }
                None => break,
            };

            let mut child =
                match CallHandle::<P3HttpClientConsumeBodyChunk, NotCancellable>::start_access(
                    accessor,
                    durable_worker_ctx::<Ctx, U>,
                    HostRequestNoInput {},
                    DurableFunctionType::WriteRemoteBatched(Some(parent_begin)),
                )
                .await
                {
                    Ok(child) => child,
                    Err(error) => {
                        // Durable-machinery failure (not an HTTP body error): surface
                        // it to the in-flight guest read and fail the task. No child
                        // `Start` was persisted; `parent.trap` abandons the parent so
                        // it never records a `Cancelled` (a trap is not a
                        // cancellation) and tags the error with the parent scope's
                        // trap context for correct retry grouping.
                        let trap_context = parent.trap_context();
                        let _ = demand.send(HttpBodyChunkReply::Failed {
                            message: error.to_string(),
                            trap_context,
                        });
                        return fail_consume_body_task(
                            trailers_tx,
                            wasmtime::Error::from_anyhow(parent.trap(error)),
                            Some(trap_context),
                        );
                    }
                };

            // Produce the next item: replay the recorded child (replay) or read
            // the upstream body and persist it (live). Delivery to the guest-facing
            // stream happens afterwards, identically on both paths.
            //
            // The demand-channel send below is the chunk's real guest-facing
            // delivery boundary — one more fallible transfer *after* the child's
            // durable `End` — so the child terminal goes through the
            // deferred-delivery API: a closed demand receiver after the persisted
            // `End` records the child's `CompletionDiscarded` marker instead of
            // replay redelivering a chunk the recorded run never handed to the
            // guest.
            let (produced, delivery) = if !child.is_live() {
                match child
                    .replay_access_deferred(accessor, durable_worker_ctx::<Ctx, U>)
                    .await
                {
                    Ok(DeferredCallReplayOutcome::Replayed(response, delivery)) => {
                        let produced = match response.chunk {
                            SerializableP3HttpBodyChunk::Data(bytes) => {
                                // Mirror the live path: once a frame is produced, the
                                // read's cancel plumbing is released (live, the read
                                // future owns it and is dropped when a frame wins the
                                // select). Holding the `cancel_ack` sender across the
                                // delivery boundary would leave a cancelling guest
                                // blocked in `stream.cancel-read` while a
                                // replay-discarded delivery parks on the demand —
                                // a circular wait.
                                drop(cancel_rx);
                                drop(read_cancel_ack);
                                ProducedChunk::Data(Bytes::from(bytes))
                            }
                            SerializableP3HttpBodyChunk::End => {
                                drop(cancel_rx);
                                drop(read_cancel_ack);
                                ProducedChunk::End
                            }
                            SerializableP3HttpBodyChunk::Cancelled => {
                                if let Some(cancel_rx) = cancel_rx {
                                    let _ = cancel_rx.await;
                                }
                                cancel_ack = read_cancel_ack;
                                terminal = Ok(None);
                                ProducedChunk::Cancelled
                            }
                        };
                        (produced, delivery)
                    }
                    Ok(DeferredCallReplayOutcome::Incomplete(mut child)) => {
                        // A batched (`WriteRemoteBatched(Some(..))`) child is not
                        // re-executable: `replay_access_deferred` hard-errors on an
                        // incomplete `Start` rather than returning `Incomplete`,
                        // so this arm is not reachable in normal operation. Treat
                        // it defensively: abandon the live child handle (a trap is
                        // not a cancellation) so it is not dropped unfinished, then
                        // trap the parent.
                        child.abandon_for_trap();
                        let message =
                            "consume-body chunk replay returned an unexpected incomplete child"
                                .to_string();
                        let trap_context = parent.trap_context();
                        let _ = demand.send(HttpBodyChunkReply::Failed {
                            message: message.clone(),
                            trap_context,
                        });
                        return fail_consume_body_task(
                            trailers_tx,
                            wasmtime::Error::from_anyhow(parent.trap(anyhow::Error::msg(message))),
                            Some(trap_context),
                        );
                    }
                    Err(error) => {
                        let trap_context = parent.trap_context();
                        let _ = demand.send(HttpBodyChunkReply::Failed {
                            message: error.to_string(),
                            trap_context,
                        });
                        return fail_consume_body_task(
                            trailers_tx,
                            wasmtime::Error::from_anyhow(parent.trap(error)),
                            Some(trap_context),
                        );
                    }
                }
            } else {
                // Live upstream body reads (including a re-issued request's body) can park
                // indefinitely waiting for network bytes, so they must race the worker's
                // interrupt signal: worker interruption / the invocation deadline can only
                // unwind the event loop cooperatively, from within a parked host future.
                let interrupt = accessor.with(|mut access| {
                    durable_worker_ctx::<Ctx, U>(access.data_mut()).create_interrupt_signal()
                });
                let read_frame = async {
                    if pending_reissue {
                        // First live read of a replayed response's placeholder body:
                        // the durable consume-body scope turned out to be incomplete
                        // (the original run was interrupted mid-body-stream, so the
                        // scope claim jumped to live), and the placeholder carries no
                        // data. Re-issue the recorded request now and stream the
                        // fresh body instead. This only fires on a real guest demand:
                        // a dropped stream or a cleanly replaying scope never
                        // re-issues.
                        pending_reissue = false;
                        match resend.as_ref() {
                            Some(resend) => {
                                match reissue_recorded_request::<Ctx, U>(accessor, resend).await {
                                    RebuildOutcome::Rebuilt {
                                        body: fresh_body,
                                        io_guard,
                                    } => {
                                        body = fresh_body;
                                        _rebuild_io_guard = Some(io_guard);
                                        read_http_body_frame(&mut body).await
                                    }
                                    RebuildOutcome::Failed(code) => HttpBodyFrame::Error(code),
                                    RebuildOutcome::Refused(message) => {
                                        retry_exempt = true;
                                        HttpBodyFrame::Error(ErrorCode::InternalError(Some(
                                            message,
                                        )))
                                    }
                                }
                            }
                            None => {
                                retry_exempt = true;
                                HttpBodyFrame::Error(ErrorCode::InternalError(Some(
                                "cannot rebuild the in-flight p3 HTTP send after a restart: no \
                                 resend information was captured for the replayed response"
                                    .to_string(),
                            )))
                            }
                        }
                    } else if let Some(cancel_rx) = cancel_rx {
                        tokio::select! {
                            _ = cancel_rx => {
                                cancel_ack = read_cancel_ack;
                                HttpBodyFrame::Cancelled
                            }
                            frame = read_http_body_frame(&mut body) => frame,
                        }
                    } else {
                        read_http_body_frame(&mut body).await
                    }
                };
                let mut frame = tokio::select! {
                    frame = read_frame => frame,
                    kind = interrupt => {
                        // An interrupt is not a cancellation and not a hard error:
                        // abandon the open chunk child first, then the parent, so
                        // neither writes a terminal (both `Start`s stay incomplete
                        // and re-execute on resume), and unwind the event loop with
                        // the interrupt kind directly so it classifies as
                        // `TrapType::Interrupt`.
                        child.abandon_for_trap();
                        parent.abandon_for_trap();
                        return Err(wasmtime::Error::from_anyhow(kind.into()));
                    }
                };

                // Inline response-body resume, mirroring the P2
                // `try_resuming_response_body_inline_retry` path: a transient
                // live read failure re-sends the recorded request with a
                // `Range: bytes={delivered}-` header and splices the fresh
                // body into this task, so the guest-facing stream continues
                // seamlessly — no trap, no replay. This runs *before* the
                // worker-level classification below; only an ineligible or
                // budget-exhausted failure falls through to it.
                while let HttpBodyFrame::Error(error_code) = &frame {
                    if retry_exempt {
                        break;
                    }
                    let (Some(resend), Some((method, uri, is_idempotent))) =
                        (resend.as_ref(), resume_context.as_ref())
                    else {
                        break;
                    };
                    if classify_serializable_http_error_code(&serialize_error_code(error_code))
                        != HostFailureKind::Transient
                    {
                        break;
                    }
                    // A request that already carried a guest-set `Range`
                    // header cannot be resumed: composing range semantics on
                    // top of the guest's own range is not supported.
                    if resend
                        .request
                        .headers
                        .keys()
                        .any(|name| name.eq_ignore_ascii_case("range"))
                    {
                        break;
                    }
                    // The same worker-state and idempotence gates as the send's
                    // own inline-retry loop (live, not snapshotting, persistence
                    // on, no atomic region, idempotence predicate).
                    if !inline_retry_eligible_for_method::<Ctx, U>(accessor, &resend.request.method)
                    {
                        break;
                    }

                    let mut properties =
                        RetryContext::http_with_response(method, uri, None, "transient");
                    apply_method_idempotence(&mut properties, *is_idempotent);
                    if resume_retry_ctx.is_none() {
                        resume_retry_ctx = Some(
                            make_p3_http_retry_task_context::<Ctx, U>(
                                accessor,
                                parent_begin,
                                properties.clone(),
                            )
                            .await,
                        );
                    }
                    let retry_ctx = resume_retry_ctx
                        .as_mut()
                        .expect("resume retry context was just created");
                    retry_ctx.retry_properties = properties.clone();
                    match resume_retry_state
                        .decide_retry_with_properties(retry_ctx, "http-zone2-read", &properties)
                        .await
                    {
                        AsyncRetryDecision::RetryAfterDelay(delay) => {
                            tokio::time::sleep(delay).await;
                        }
                        AsyncRetryDecision::FallBackToTrap | AsyncRetryDecision::Exhausted => {
                            break;
                        }
                    }

                    let range_headers = if delivered_bytes > 0 {
                        vec![("range".to_string(), format!("bytes={delivered_bytes}-"))]
                    } else {
                        Vec::new()
                    };
                    match resend_recorded_request::<Ctx, U>(accessor, resend, &range_headers).await
                    {
                        // The recorded request body cannot be reconstructed:
                        // resume is refused, the original failure falls through
                        // to the worker-level retry classification (whose replay
                        // re-issues from a then-complete recording, or fails).
                        ResendOutcome::Refused(reason) => {
                            debug!(%reason, "p3 HTTP response-body resume refused");
                            break;
                        }
                        // The resume send itself failed: charge another resume
                        // attempt against the budget with the fresh error.
                        ResendOutcome::Failed(code) => {
                            frame = HttpBodyFrame::Error(code);
                        }
                        ResendOutcome::Sent { response, io_guard } => {
                            let status = response.status().as_u16();
                            if status == 206 {
                                let content_range_start = response
                                    .headers()
                                    .get("content-range")
                                    .and_then(|value| value.to_str().ok())
                                    .and_then(parse_content_range_start);
                                if content_range_start == Some(delivered_bytes) {
                                    body = response.into_body();
                                    _rebuild_io_guard = Some(io_guard);
                                    frame = read_http_body_frame(&mut body).await;
                                } else {
                                    debug!(
                                        ?content_range_start,
                                        delivered_bytes,
                                        "p3 HTTP response-body resume: 206 Content-Range \
                                         mismatch, falling back"
                                    );
                                    break;
                                }
                            } else if status == 416 {
                                // The server refuses the range: the resource
                                // changed since the original response, so the
                                // already-delivered prefix cannot be continued.
                                // Deterministic for this request — never
                                // retry-routed.
                                retry_exempt = true;
                                frame = HttpBodyFrame::Error(ErrorCode::InternalError(Some(
                                    "response-body resume failed: the server returned 416 Range \
                                     Not Satisfiable"
                                        .to_string(),
                                )));
                                break;
                            } else if status == resend.recorded_status {
                                // The server ignored the range and re-sent the
                                // full response with the original status: skip
                                // the already-delivered prefix (count-only, no
                                // content verification — P2 parity) and continue
                                // from there.
                                let mut fresh_body = response.into_body();
                                match skip_body_prefix(&mut fresh_body, delivered_bytes).await {
                                    Ok(leftover) => {
                                        body = fresh_body;
                                        _rebuild_io_guard = Some(io_guard);
                                        frame = match leftover {
                                            Some(bytes) => HttpBodyFrame::Data(bytes),
                                            None => read_http_body_frame(&mut body).await,
                                        };
                                    }
                                    Err(SkipBodyPrefixError::BodyTooShort) => {
                                        retry_exempt = true;
                                        frame =
                                            HttpBodyFrame::Error(ErrorCode::InternalError(Some(
                                                "response-body resume failed: the re-sent \
                                                 response body is shorter than the bytes already \
                                                 delivered to the guest"
                                                    .to_string(),
                                            )));
                                        break;
                                    }
                                    Err(SkipBodyPrefixError::Read(code)) => {
                                        frame = HttpBodyFrame::Error(code);
                                    }
                                }
                            } else {
                                debug!(
                                    status,
                                    recorded_status = resend.recorded_status,
                                    "p3 HTTP response-body resume: unexpected status, falling back"
                                );
                                break;
                            }
                        }
                    }
                }

                // Worker-level retry classification for live body-transfer
                // errors, mirroring the P2 body-stream read path: a transient
                // error raises a retry trap here — before anything about this
                // frame is persisted or delivered, so the guest never observes
                // a truncated stream — leaving the parent `Start` incomplete.
                // The retry's replay then jumps the scope and re-issues the
                // recorded request (see `reissue_recorded_request`), re-reading
                // the body from a fresh response. Permanent errors — and
                // transient ones whose retry budget is exhausted — fall through
                // and are recorded as the terminal, which is also what a
                // recorded terminal replays as. A retry-exempt failure (refused
                // rebuild, content-changed resume) is never retry-routed: its
                // replay would hit the same deterministic failure again.
                if let HttpBodyFrame::Error(error_code) = &frame
                    && !retry_exempt
                    && let Some(retry_properties) = retry_properties.clone()
                {
                    let for_retry: Result<(), &ErrorCode> = Err(error_code);
                    let trap_context = parent.trap_context();
                    if let Err(error) = parent
                        .try_trigger_retry_access(
                            accessor,
                            durable_worker_ctx::<Ctx, U>,
                            &for_retry,
                            |code| {
                                classify_serializable_http_error_code(&serialize_error_code(code))
                            },
                            retry_properties,
                        )
                        .await
                    {
                        // The retry trap tears the invocation down;
                        // `try_trigger_retry_access` already abandoned the
                        // parent handle. The child `Start` is persisted but the
                        // jumped scope discards it on replay; abandon the handle
                        // so its drop does not record a `Cancelled`. The span is
                        // deliberately not finished (no `FinishSpan` after an
                        // incomplete `Start`) — the retry's replay reconstructs
                        // it.
                        child.abandon_for_trap();
                        let _ = demand.send(HttpBodyChunkReply::Failed {
                            message: error.to_string(),
                            trap_context,
                        });
                        return fail_consume_body_task(
                            trailers_tx,
                            wasmtime::Error::from_anyhow(error),
                            Some(trap_context),
                        );
                    }
                }

                let chunk = match &frame {
                    HttpBodyFrame::Data(bytes) => SerializableP3HttpBodyChunk::Data(bytes.to_vec()),
                    HttpBodyFrame::End(_) | HttpBodyFrame::Error(_) => {
                        SerializableP3HttpBodyChunk::End
                    }
                    HttpBodyFrame::Cancelled => SerializableP3HttpBodyChunk::Cancelled,
                };

                let delivery = match child
                    .complete_access_deferred(
                        accessor,
                        durable_worker_ctx::<Ctx, U>,
                        HostResponseP3HttpClientConsumeBodyChunk { chunk },
                        None,
                    )
                    .await
                {
                    Ok((_, delivery)) => delivery,
                    Err(error) => {
                        // The child `Start` is already persisted but its `End` failed:
                        // the recorded chunk history is now incomplete. Fail the task
                        // loud rather than papering over it with a normal terminal and a
                        // completed parent marker, which would commit a malformed oplog.
                        // `complete_access_deferred` already finished the child handle
                        // without recording a `Cancelled` and its `TerminalCallError`
                        // carries the child scope's trap context, so preserve that error;
                        // we only need to abandon the still-open parent so it is not
                        // dropped unfinished (which would wrongly record a parent
                        // `Cancelled`).
                        let trap_context = parent.trap_context();
                        let _ = demand.send(HttpBodyChunkReply::Failed {
                            message: error.to_string(),
                            trap_context,
                        });
                        parent.abandon_for_trap();
                        return fail_consume_body_task(
                            trailers_tx,
                            wasmtime::Error::from(error),
                            Some(trap_context),
                        );
                    }
                };

                let produced = match frame {
                    HttpBodyFrame::Data(bytes) => ProducedChunk::Data(bytes),
                    HttpBodyFrame::End(trailers) => {
                        terminal = Ok(trailers);
                        ProducedChunk::End
                    }
                    HttpBodyFrame::Error(error) => {
                        terminal = Err(error);
                        ProducedChunk::End
                    }
                    HttpBodyFrame::Cancelled => {
                        terminal = Ok(None);
                        ProducedChunk::Cancelled
                    }
                };
                (produced, delivery)
            };

            // Deliver the produced item to the guest-facing stream. This is the
            // single point where chunks reach the guest, identically live and on
            // replay, so the count/order of delivered chunks always matches the
            // count/order of persisted children. It is also where the child's
            // deferred-delivery token is consumed: a successful send is
            // `delivered`, a closed demand receiver records the child's
            // `CompletionDiscarded` marker, and a replay-discarded child is never
            // re-sent (the task parks until the replayed guest drops the body
            // reader at the same point it did live).
            match produced {
                ProducedChunk::Data(bytes) => {
                    // The transfer boundary itself — replay-discard parking, the
                    // send, delivered-byte accounting, and the token — lives in
                    // `transfer_data_chunk`; only parent finalization stays here.
                    match transfer_data_chunk(
                        &activity,
                        demand,
                        bytes,
                        delivery,
                        &mut delivered_bytes,
                    )
                    .await
                    {
                        Ok(DataChunkTransfer::Delivered) => {}
                        Ok(DataChunkTransfer::Abandoned) => {
                            // The undelivered chunk finalizes the body through the
                            // normal reader-drop path: the parent closes with a
                            // clean terminal, and if replay's guest never demands
                            // the recorded chunk, the whole abandoned scope is
                            // skipped at the invocation boundary.
                            terminal = Ok(None);
                            break;
                        }
                        Err(error) => {
                            let trap_context = parent.trap_context();
                            parent.abandon_for_trap();
                            return fail_consume_body_task(
                                trailers_tx,
                                wasmtime::Error::from_anyhow(mark_durable_call_trap_context(
                                    anyhow::Error::from(error),
                                    trap_context,
                                )),
                                Some(trap_context),
                            );
                        }
                    }
                }
                ProducedChunk::End => {
                    if delivery.is_replay_discarded() {
                        park_replay_discarded_delivery(&activity, demand).await;
                        break;
                    }
                    let (ack_tx, ack_rx) = oneshot::channel();
                    if demand.send(HttpBodyChunkReply::End { ack: ack_tx }).is_ok() {
                        delivery.delivered();
                        // Wait for the producer to observe the terminal (report
                        // EOF to the guest) before resolving trailers / finalizing
                        // the parent, so trailers never surface before the body
                        // stream's terminal is observed.
                        let _ = ack_rx.await;
                    } else {
                        let trap_context = parent.trap_context();
                        if let Err(error) = delivery.discarded().await {
                            parent.abandon_for_trap();
                            return fail_consume_body_task(
                                trailers_tx,
                                wasmtime::Error::from_anyhow(mark_durable_call_trap_context(
                                    anyhow::Error::from(error),
                                    trap_context,
                                )),
                                Some(trap_context),
                            );
                        }
                    }
                    break;
                }
                ProducedChunk::Cancelled => {
                    if delivery.is_replay_discarded() {
                        park_replay_discarded_delivery(&activity, demand).await;
                        break;
                    }
                    if demand.send(HttpBodyChunkReply::Cancelled).is_ok() {
                        delivery.delivered();
                    } else {
                        let trap_context = parent.trap_context();
                        if let Err(error) = delivery.discarded().await {
                            parent.abandon_for_trap();
                            return fail_consume_body_task(
                                trailers_tx,
                                wasmtime::Error::from_anyhow(mark_durable_call_trap_context(
                                    anyhow::Error::from(error),
                                    trap_context,
                                )),
                                Some(trap_context),
                            );
                        }
                    }
                    break;
                }
            }
        }

        // Drop the upstream body so a partially-consumed (or replayed-empty)
        // body closes its network read promptly.
        drop(body);

        // Finalize the parent with the terminal marker. The parent always
        // completes with a marker on the normal path; the `Cancellable` policy
        // exists only for the crash/drop contract (task dropped without
        // finishing), handled by the call handle's drop machinery.
        //
        // The trailers send below is the real guest-facing delivery boundary,
        // so the terminal is recorded through the deferred-delivery API: a
        // closed trailers receiver after the persisted `End` records a
        // `CompletionDiscarded` marker instead of replay redelivering the
        // outcome. The span's durable `FinishSpan` (legacy spans) rides the
        // same owned task as the `End`, preserving the recorded
        // `End → FinishSpan → CompletionDiscarded` order replay consumes
        // positionally.
        //
        // Capture the parent scope's trap context first (it is a pure function of
        // the scope and survives the handle being consumed below) so every
        // finalize failure can tag the guest-facing trailers trap for correct
        // retry grouping.
        let parent_trap_context = parent.trap_context();
        let post_end_entry = response_span
            .as_ref()
            .and_then(|span| span.deferred_finish_entry());
        let (outcome, delivery) = if parent.is_live() {
            match parent
                .complete_access_deferred(
                    accessor,
                    durable_worker_ctx::<Ctx, U>,
                    HostResponseP3HttpClientConsumeBodyResult {
                        result: serialize_consume_body_result(&terminal),
                    },
                    post_end_entry,
                )
                .await
            {
                Ok((response, delivery)) => {
                    (deserialize_consume_body_result(response.result), delivery)
                }
                // `complete_access_deferred` consumed and finished the parent
                // without recording a `Cancelled`; its `TerminalCallError`
                // carries the parent scope's trap context, so preserve it.
                Err(error) => {
                    return fail_consume_body_task(
                        trailers_tx,
                        wasmtime::Error::from(error),
                        Some(parent_trap_context),
                    );
                }
            }
        } else {
            match parent
                .replay_access_deferred(accessor, durable_worker_ctx::<Ctx, U>)
                .await
            {
                Ok(DeferredCallReplayOutcome::Replayed(response, delivery)) => {
                    (deserialize_consume_body_result(response.result), delivery)
                }
                Ok(DeferredCallReplayOutcome::Incomplete(parent)) => {
                    match parent
                        .complete_access_deferred(
                            accessor,
                            durable_worker_ctx::<Ctx, U>,
                            HostResponseP3HttpClientConsumeBodyResult {
                                result: serialize_consume_body_result(&terminal),
                            },
                            post_end_entry,
                        )
                        .await
                    {
                        Ok((response, delivery)) => {
                            (deserialize_consume_body_result(response.result), delivery)
                        }
                        Err(error) => {
                            return fail_consume_body_task(
                                trailers_tx,
                                wasmtime::Error::from(error),
                                Some(parent_trap_context),
                            );
                        }
                    }
                }
                Err(error) => {
                    return fail_consume_body_task(
                        trailers_tx,
                        wasmtime::Error::from_anyhow(mark_durable_call_trap_context(
                            anyhow::Error::from(error),
                            parent_trap_context,
                        )),
                        Some(parent_trap_context),
                    );
                }
            }
        };

        // The response body reached its terminal and the parent marker is
        // committed/replayed: finish the send's `outgoing-http-request` span
        // before resolving the guest-facing trailers. Live armed, the durable
        // positional `FinishSpan` (legacy spans) is already appended by the
        // owned terminal task, so only the synchronous in-memory finish
        // remains; on replay (or an unpersisted live call) the original
        // handling consumes/appends the positional entry, so its position
        // stays stable relative to the parent terminal on both paths.
        if let Some(span) = &response_span {
            let finish_result = if delivery.is_live_armed() {
                finish_p3_send_span_in_memory::<Ctx, U>(accessor, span)
            } else {
                finish_p3_send_span::<Ctx, U>(accessor, span).await
            };
            if let Err(error) = finish_result {
                // The error is observed by the caller (the trailers future
                // traps): not a silent discard, so no marker.
                delivery.suppress();
                return fail_consume_body_task(
                    trailers_tx,
                    wasmtime::Error::from_anyhow(mark_durable_call_trap_context(
                        anyhow::Error::from(error),
                        parent_trap_context,
                    )),
                    Some(parent_trap_context),
                );
            }
        }

        if delivery.is_replay_discarded() {
            // The recorded run persisted the parent terminal but the guest
            // dropped the trailers future before the outcome was delivered
            // (the marker records the discard). Never send the outcome:
            // retain the sender so the guest-facing trailers future stays
            // pending, and park at the delivery boundary until the
            // deterministic guest drops the receiver at the same point it
            // did live.
            let mut trailers_tx = trailers_tx;
            activity.park(trailers_tx.closed()).await;
            drop(trailers_tx);
        } else {
            match trailers_tx.send(HttpTrailersResolution::Outcome(outcome)) {
                Ok(()) => delivery.delivered(),
                Err(_) => {
                    // The guest dropped the trailers future after the terminal
                    // was persisted: the completion is silently discarded, so
                    // record the marker before acknowledging any cancellation
                    // (the task itself holds a `TailActivity`, and `discarded`
                    // hands a torn wait to the drain queue, so the marker stays
                    // settlement-accounted either way).
                    if let Err(error) = delivery.discarded().await {
                        return Err(wasmtime::Error::from_anyhow(
                            mark_durable_call_trap_context(
                                anyhow::Error::from(error),
                                parent_trap_context,
                            ),
                        ));
                    }
                }
            }
        }
        if let Some(ack) = cancel_ack {
            let _ = ack.send(());
        }
        Ok(())
    }
}

impl<U: Send + 'static, Ctx: WorkerCtx> types::HostResponseWithStore<U> for DurableP3<Ctx> {
    fn new(
        mut store: Access<U, Self>,
        headers: Resource<Headers>,
        contents: Option<StreamReader<u8>>,
        trailers: FutureReader<Result<Option<Resource<Trailers>>, ErrorCode>>,
    ) -> wasmtime::Result<(Resource<Response>, FutureReader<Result<(), ErrorCode>>)> {
        observe_function_call_store::<Ctx, U>(
            store.as_context_mut().data_mut(),
            "http::types::response",
            "new",
        );
        let store = Access::<U, WasiHttp>::new(store.as_context_mut(), wasi_http_view::<Ctx, U>);
        <WasiHttp as types::HostResponseWithStore<U>>::new(store, headers, contents, trailers)
    }

    fn consume_body(
        mut store: Access<U, Self>,
        res: Resource<Response>,
        fut: FutureReader<Result<(), ErrorCode>>,
    ) -> wasmtime::Result<(
        StreamReader<u8>,
        FutureReader<Result<Option<Resource<Trailers>>, ErrorCode>>,
    )> {
        // Take ownership of the response's open-response state (if this
        // response came from `client::send`): the durable consume-body task
        // finishes its span when the body reaches its terminal and uses its
        // rebuild info when an incomplete scope must re-execute live. Removing
        // the mapping here also keeps the later `drop` of the response
        // resource from finishing the span a second time.
        let response_state = {
            let mut store_ctx = store.as_context_mut();
            let ctx = durable_worker_ctx::<Ctx, U>(store_ctx.data_mut());
            ctx.state.open_p3_http_responses.remove(&res.rep())
        };

        // Delegate to the built-in implementation to wire `fut` into the body's
        // transmission-result channel and to build the host body stream.
        let (upstream_stream, mut upstream_trailers) = {
            let http_store =
                Access::<U, WasiHttp>::new(store.as_context_mut(), wasi_http_view::<Ctx, U>);
            <WasiHttp as types::HostResponseWithStore<U>>::consume_body(http_store, res, fut)?
        };

        // Recover the host body producer so we can drive and record the body
        // transfer ourselves. Responses obtained from `client.send` (live or
        // replayed) always carry a host-constructed body, so this succeeds.
        let body =
            match upstream_stream.try_into::<HostBodyStreamProducer<U>>(store.as_context_mut()) {
                Ok(mut producer) => {
                    let body = producer.take_body();
                    // Dropping the now-empty producer resolves the upstream
                    // trailers future (`Ok(None)`), which we discard below.
                    drop(producer);
                    body
                }
                Err(stream) => {
                    // Guest-constructed response body (not from `send`): fall back
                    // to the non-durable passthrough. No state was registered for
                    // such responses (`response_state` is `None` here).
                    debug_assert!(response_state.is_none());
                    return Ok((stream, upstream_trailers));
                }
            };

        // We surface trailers through our own future, so discard the built-in
        // trailers future.
        upstream_trailers.close(store.as_context_mut())?;

        // Capacity 1 suffices (and bounds memory as defense in depth): the
        // producer keeps at most one demand in flight at a time.
        let (demand_tx, demand_rx) = mpsc::channel(1);
        let (trailers_tx, trailers_rx) = oneshot::channel();

        // Build both guest-facing handles before spawning the durable task. The
        // task appends the `consume-body` `Start`; the guest cannot poll either
        // handle until this host call returns, so spawning first would risk
        // committing a `Start` with no terminal (orphaned `Start`) if a later
        // handle construction fails.
        let mut stream = StreamReader::new(&mut store, DurableHttpBodyProducer::new(demand_tx))?;
        let trailers = match FutureReader::new(
            &mut store,
            HttpTrailersFutureProducer::<Ctx, U>::new(trailers_rx),
        ) {
            Ok(trailers) => trailers,
            Err(err) => {
                let _ = stream.close(store.as_context_mut());
                return Err(err);
            }
        };

        let activity = {
            let mut store_ctx = store.as_context_mut();
            durable_worker_ctx::<Ctx, U>(store_ctx.data_mut())
                .tail_work_tracker()
                .activity()
        };
        store.spawn(HttpConsumeBodyTask::<Ctx>::new(
            body,
            demand_rx,
            trailers_tx,
            response_state,
            activity,
        ));
        Ok((stream, trailers))
    }

    fn drop(mut store: Access<U, Self>, res: Resource<Response>) -> wasmtime::Result<()> {
        observe_function_call_store::<Ctx, U>(
            store.as_context_mut().data_mut(),
            "http::types::response",
            "drop",
        );

        // A send-created response dropped before its body was consumed still
        // owns its `outgoing-http-request` span. This host call is synchronous,
        // so the finish is deferred to the next drop-event drain point (a
        // deterministic replay point — which matters for legacy-recorded spans
        // whose finish is durable), mirroring P2's `end_http_request` on
        // response drop.
        {
            let mut store_ctx = store.as_context_mut();
            let ctx = durable_worker_ctx::<Ctx, U>(store_ctx.data_mut());
            if let Some(state) = ctx.state.open_p3_http_responses.remove(&res.rep())
                && let Some(sink) = ctx.state.dropped_call_event_sender()
            {
                let _ = sink.send(DropEvent::FinishSpan {
                    span_id: state.span.span_id,
                    durable: state.span.legacy_durable,
                });
            }
        }

        let store = Access::<U, WasiHttp>::new(store.as_context_mut(), wasi_http_view::<Ctx, U>);
        <WasiHttp as types::HostResponseWithStore<U>>::drop(store, res)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::durable_host::p3::http::test_support::FrameTestOplog;
    use crate::durable_host::tail_work::TailWorkTracker;
    use crate::services::oplog::Oplog;
    use golem_common::model::Timestamp;
    use golem_common::model::oplog::host_functions::HostFunctionName;
    use golem_common::model::oplog::{
        HostRequest, HostResponse, OplogEntry, OplogIndex, OplogPayload,
    };
    use test_r::{test, timeout};

    /// Seeds `oplog` with the durable prefix a live consume-body loop leaves behind right before
    /// the guest-facing transfer of its first chunk: the parent consume-body `Start`, the child
    /// chunk `Start`, and the child's persisted `End(Data)`. Returns the child's start index —
    /// the index a `CompletionDiscarded` marker for the chunk must reference.
    async fn seed_persisted_data_child(oplog: &FrameTestOplog, bytes: &[u8]) -> OplogIndex {
        oplog
            .add(OplogEntry::Start {
                timestamp: Timestamp::now_utc(),
                parent_start_index: None,
                function_name: HostFunctionName::P3HttpClientConsumeBody,
                request: Some(OplogPayload::Inline(Box::new(HostRequest::NoInput(
                    HostRequestNoInput {},
                )))),
                durable_function_type: DurableFunctionType::WriteRemoteBatched(None),
            })
            .await;
        let child_start = oplog
            .add(OplogEntry::Start {
                timestamp: Timestamp::now_utc(),
                parent_start_index: Some(OplogIndex::from_u64(1)),
                function_name: HostFunctionName::P3HttpClientConsumeBodyChunk,
                request: Some(OplogPayload::Inline(Box::new(HostRequest::NoInput(
                    HostRequestNoInput {},
                )))),
                durable_function_type: DurableFunctionType::WriteRemoteBatched(Some(
                    OplogIndex::from_u64(1),
                )),
            })
            .await;
        oplog
            .add(OplogEntry::End {
                timestamp: Timestamp::now_utc(),
                start_index: child_start,
                response: Some(OplogPayload::Inline(Box::new(
                    HostResponse::P3HttpClientConsumeBodyChunk(
                        HostResponseP3HttpClientConsumeBodyChunk {
                            chunk: SerializableP3HttpBodyChunk::Data(bytes.to_vec()),
                        },
                    ),
                ))),
                forced_commit: false,
            })
            .await;
        child_start
    }

    /// A successful live transfer must deliver the chunk to the guest, consume the token as
    /// delivered (no marker append), and advance the delivered-byte count by the chunk length.
    #[test]
    #[timeout("10s")]
    async fn transfer_data_chunk_delivers_live_chunk_and_advances_bytes() {
        let oplog = FrameTestOplog::new();
        let child_start = seed_persisted_data_child(&oplog, b"abc").await;
        let seeded_entries = oplog.entry_count();
        let delivery = CompletionDelivery::test_live_armed(oplog.clone(), child_start)
            .await
            .expect("failed to build the live delivery token");
        let tracker = TailWorkTracker::new();
        let activity = tracker.activity();
        let (demand, mut reply) = oneshot::channel();
        let mut delivered_bytes = 0u64;

        let outcome = transfer_data_chunk(
            &activity,
            demand,
            Bytes::from_static(b"abc"),
            delivery,
            &mut delivered_bytes,
        )
        .await
        .expect("live transfer must not fail");

        assert!(matches!(outcome, DataChunkTransfer::Delivered));
        assert_eq!(delivered_bytes, 3);
        match reply.try_recv() {
            Ok(HttpBodyChunkReply::Data(bytes)) => assert_eq!(bytes, Bytes::from_static(b"abc")),
            Ok(_) => panic!("expected the delivered data chunk, got a different reply kind"),
            Err(error) => panic!("expected the delivered data chunk, got no reply: {error}"),
        }
        assert_eq!(
            oplog.entry_count(),
            seeded_entries,
            "a delivered chunk must not append a marker"
        );
    }

    /// The vanished-demand-receiver regression at the unit level: the child's `End(Data)` is
    /// durable but the guest dropped the body reader before the transfer. The helper must report
    /// the body abandoned, must not count the undelivered bytes, and must have the child's
    /// `CompletionDiscarded` marker durable *before* it returns.
    #[test]
    #[timeout("10s")]
    async fn transfer_data_chunk_records_discard_marker_for_closed_live_receiver() {
        let oplog = FrameTestOplog::new();
        let child_start = seed_persisted_data_child(&oplog, b"abc").await;
        let delivery = CompletionDelivery::test_live_armed(oplog.clone(), child_start)
            .await
            .expect("failed to build the live delivery token");
        let tracker = TailWorkTracker::new();
        let activity = tracker.activity();
        let (demand, reply) = oneshot::channel::<HttpBodyChunkReply>();
        drop(reply);
        let mut delivered_bytes = 0u64;

        let outcome = transfer_data_chunk(
            &activity,
            demand,
            Bytes::from_static(b"abc"),
            delivery,
            &mut delivered_bytes,
        )
        .await
        .expect("a discarded transfer must not fail the task");

        assert!(matches!(outcome, DataChunkTransfer::Abandoned));
        assert_eq!(
            delivered_bytes, 0,
            "an undelivered chunk must not advance the delivered-byte count"
        );
        let markers = oplog
            .entries()
            .into_iter()
            .filter(|entry| matches!(entry, OplogEntry::CompletionDiscarded { .. }))
            .collect::<Vec<_>>();
        match markers.as_slice() {
            [OplogEntry::CompletionDiscarded { start_index, .. }] => {
                assert_eq!(
                    *start_index, child_start,
                    "the marker must reference the discarded child's Start"
                );
            }
            other => panic!("expected exactly one CompletionDiscarded marker, got {other:?}"),
        }
    }

    /// Replay of a recorded discarded chunk with a repeated guest demand: the helper must never
    /// re-deliver the recorded bytes — it parks (as inactive tail work) until the replayed guest
    /// drops its in-flight read, then finalizes the body as abandoned without touching the oplog
    /// or the delivered-byte count.
    #[test]
    #[timeout("10s")]
    async fn transfer_data_chunk_parks_replay_discarded_until_reader_drop() {
        let oplog = FrameTestOplog::new();
        let delivery = CompletionDelivery::test_replay_discarded();
        let tracker = TailWorkTracker::new();
        let activity = tracker.activity();
        let (demand, mut reply) = oneshot::channel::<HttpBodyChunkReply>();
        let mut delivered_bytes = 0u64;

        let mut transfer = Box::pin(transfer_data_chunk(
            &activity,
            demand,
            Bytes::from_static(b"abc"),
            delivery,
            &mut delivered_bytes,
        ));
        assert!(
            futures::poll!(transfer.as_mut()).is_pending(),
            "the transfer must park while the replayed guest still holds the body reader"
        );
        assert!(
            !tracker.has_active(),
            "the parked transfer must be a safe park point (inactive tail work)"
        );
        assert!(
            reply.try_recv().is_err(),
            "a replay-discarded chunk must never be re-delivered"
        );

        drop(reply);
        let outcome = transfer
            .await
            .expect("a replay-discarded transfer must not fail the task");
        assert!(matches!(outcome, DataChunkTransfer::Abandoned));
        assert!(
            tracker.has_active(),
            "the task must be counted active again after un-parking"
        );
        assert_eq!(delivered_bytes, 0);
        assert_eq!(
            oplog.entry_count(),
            0,
            "replay must not append anything at the delivery boundary"
        );
    }
}
