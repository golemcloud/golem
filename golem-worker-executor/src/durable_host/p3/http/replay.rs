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

use super::serialization::{deserialize_error_code, serialize_error_code, serialize_headers};
use super::*;
use crate::durable_host::p3::{DurableP3, durable_worker_ctx, wasi_http_view};
use crate::durable_host::tail_work::TailActivity;
use crate::services::oplog::Oplog;
use crate::workerctx::WorkerCtx;
use anyhow::Context as _;
use bytes::Bytes;
use golem_common::model::oplog::payload::types::{
    SerializableP3HttpClientSendResult, SerializableP3HttpRequestBodyFrame,
    SerializableResponseHeaders,
};
use golem_common::model::oplog::{OplogIndex, PersistenceLevel};
use http::{HeaderMap, HeaderName, HeaderValue};
use http_body_util::BodyExt as _;
use http_body_util::Empty;
use http_body_util::combinators::UnsyncBoxBody;
use std::marker::PhantomData;
use std::sync::Arc;
use tokio::sync::oneshot;
use tracing::warn;
use wasmtime::AsContextMut;
use wasmtime::component::{Accessor, AccessorTask, Resource};
use wasmtime_wasi_http::p3::WasiHttp;
use wasmtime_wasi_http::p3::bindings::http::types::{ErrorCode, Request, Response};

/// Identity of a replayed send's recorded request-body frame stream.
pub(super) struct ReplayedRequestBodyRecording {
    /// The send's `Start` index — the `parent_start_index` key of its frames.
    pub(super) send_start_index: OplogIndex,
    /// Whether the recording had already reached its terminal frame when the
    /// send result was recorded. If true the recording is complete and the
    /// drain never needs to append anything.
    pub(super) recording_complete_at_end: bool,
}

pub(super) async fn consume_replayed_request<Ctx: WorkerCtx, U: Send + 'static>(
    store: &Accessor<U, DurableP3<Ctx>>,
    req: Resource<Request>,
    recorded_body: Option<ReplayedRequestBodyRecording>,
) -> HttpResult<()> {
    let (drain_result_tx, drain_result_rx) = oneshot::channel::<Result<(), ErrorCode>>();
    let http_store = store.with_getter::<WasiHttp>(wasi_http_view::<Ctx, U>);
    let body = http_store.with(
        |mut access| -> HttpResult<UnsyncBoxBody<Bytes, ErrorCode>> {
            let request = access
                .get()
                .table
                .delete(req)
                .context("failed to delete replayed p3 HTTP request from table")
                .map_err(wasmtime::Error::from_anyhow)
                .map_err(HttpError::trap)?;
            let (http_request, _options) = request.into_http_with_getter(
                access.as_context_mut(),
                // Resolve the transmission future with the drain's result. If
                // the drain task is dropped before sending (e.g. worker
                // teardown), fall back to `Ok(())`.
                async move { drain_result_rx.await.unwrap_or(Ok(())) },
                wasi_http_view::<Ctx, U>,
            )?;
            Ok(http_request.into_body())
        },
    )?;
    let activity = store.with(|mut access| {
        durable_worker_ctx::<Ctx, U>(access.data_mut())
            .tail_work_tracker()
            .activity()
    });
    store.spawn(ReplayRequestBodyDrain::<Ctx>::new(
        body,
        recorded_body,
        drain_result_tx,
        activity,
    ));
    Ok(())
}

/// Spawns a [`ReplayedRequestLeakGuard`] for the replayed send's request
/// resource. Called by the durable `send` replay path *before* it awaits the
/// recorded resolution; see the guard's documentation for why.
pub(super) fn spawn_replayed_request_leak_guard<Ctx: WorkerCtx, U: Send + 'static>(
    store: &Accessor<U, DurableP3<Ctx>>,
    request_rep: u32,
    disarm_rx: oneshot::Receiver<()>,
) {
    let activity = store.with(|mut access| {
        durable_worker_ctx::<Ctx, U>(access.data_mut())
            .tail_work_tracker()
            .activity()
    });
    store.spawn(ReplayedRequestLeakGuard::<Ctx> {
        request_rep,
        disarm_rx,
        activity,
        _phantom: PhantomData,
    });
}

/// Fallback consumer of a replayed send's request resource.
///
/// The guest may drop the replayed send future at any await point (e.g. it
/// cancels the response future after losing a `race` against another branch,
/// which aborts the host task mid-`replay_access`). At those points the send
/// future still owns the replayed `Resource<Request>`, and a plain drop would
/// leak it in the resource table: the host-side ends of the guest's body
/// streams stay alive but are never consumed, so the guest's pending body /
/// trailers writes never resolve and the guest task can never exit — blocking
/// invocation settlement.
///
/// The guard holds the request's table rep and waits on a disarm channel. The
/// replay path disarms it on every path where request ownership is handed
/// over: normal replay completion (which consumes the request inline) and
/// live re-execution of an incomplete call (where the real `send` takes the
/// request). Only when the send future is dropped without disarming does the
/// guard consume the request itself — deleting it from the table and draining
/// its outgoing body, which resolves the guest's writes just like a completed
/// transmission would. Rep reuse is not a hazard: the leaked entry is deleted
/// by nobody else, so its rep cannot be reassigned before the guard consumes
/// it.
struct ReplayedRequestLeakGuard<Ctx> {
    request_rep: u32,
    disarm_rx: oneshot::Receiver<()>,
    activity: TailActivity,
    _phantom: PhantomData<fn() -> Ctx>,
}

impl<Ctx, U> AccessorTask<U, DurableP3<Ctx>> for ReplayedRequestLeakGuard<Ctx>
where
    Ctx: WorkerCtx,
    U: Send + 'static,
{
    async fn run(self, accessor: &Accessor<U, DurableP3<Ctx>>) -> wasmtime::Result<()> {
        let Self {
            request_rep,
            disarm_rx,
            activity,
            _phantom,
        } = self;
        // This wait stays active (never parked): it resolves at the latest
        // when the guest task exits — the send future is part of the guest's
        // call graph, so guest exit either completes it (disarm sent) or
        // drops it (channel closed) — which strictly precedes the settlement
        // check that consults the tracker.
        let disarmed = disarm_rx.await.is_ok();
        if !disarmed
            && let Err(error) =
                consume_replayed_request::<Ctx, U>(accessor, Resource::new_own(request_rep), None)
                    .await
        {
            warn!(
                ?error,
                "failed to consume a replayed p3 HTTP request dropped by the guest mid-replay"
            );
        }
        drop(activity);
        Ok(())
    }
}

/// Drives an outgoing request body to completion, discarding each frame as it
/// is read, and returns its terminal result. Frames are dropped one at a time
/// (rather than accumulated with `collect`) so draining a large replayed upload
/// does not buffer the whole body in memory; the bytes are not needed because
/// the recorded response head is already authoritative.
pub(super) async fn drain_request_body(
    mut body: UnsyncBoxBody<Bytes, ErrorCode>,
) -> Result<(), ErrorCode> {
    while let Some(frame) = body.frame().await {
        frame?;
    }
    Ok(())
}

/// Background task that drains a replayed request's outgoing body to completion
/// (no network) and reports the drain result to the request transmission
/// future. See [`consume_replayed_request`] for why this runs off the `send`
/// return path and how its result is wired back to the guest.
///
/// For sends with a recorded request body whose recording may be incomplete
/// (the original run crashed while the body was still streaming), the drain
/// additionally self-heals the recording: it matches the guest's re-produced
/// bytes against the recorded coverage and appends the missing frames — see
/// [`drain_replayed_request_body_completing_recording`].
pub(super) struct ReplayRequestBodyDrain<Ctx> {
    body: UnsyncBoxBody<Bytes, ErrorCode>,
    recorded_body: Option<ReplayedRequestBodyRecording>,
    result_tx: oneshot::Sender<Result<(), ErrorCode>>,
    activity: TailActivity,
    _phantom: PhantomData<fn() -> Ctx>,
}

impl<Ctx> ReplayRequestBodyDrain<Ctx> {
    fn new(
        body: UnsyncBoxBody<Bytes, ErrorCode>,
        recorded_body: Option<ReplayedRequestBodyRecording>,
        result_tx: oneshot::Sender<Result<(), ErrorCode>>,
        activity: TailActivity,
    ) -> Self {
        Self {
            body,
            recorded_body,
            result_tx,
            activity,
            _phantom: PhantomData,
        }
    }
}

impl<Ctx, U> AccessorTask<U, DurableP3<Ctx>> for ReplayRequestBodyDrain<Ctx>
where
    Ctx: WorkerCtx,
    U: 'static,
{
    async fn run(self, accessor: &Accessor<U, DurableP3<Ctx>>) -> wasmtime::Result<()> {
        let Self {
            body,
            recorded_body,
            result_tx,
            activity,
            _phantom,
        } = self;
        let result = match recorded_body.filter(|recorded| !recorded.recording_complete_at_end) {
            None => activity.park(drain_request_body(body)).await,
            Some(recorded_body) => {
                let (oplog, recording_enabled) = accessor.with(|mut access| {
                    let ctx = durable_worker_ctx::<Ctx, U>(access.data_mut());
                    (
                        ctx.state.oplog.clone(),
                        ctx.state.snapshotting_mode.is_none()
                            && ctx.state.persistence_level != PersistenceLevel::PersistNothing,
                    )
                });
                if recording_enabled {
                    drain_replayed_request_body_completing_recording(
                        body,
                        oplog,
                        recorded_body.send_start_index,
                        &activity,
                    )
                    .await
                } else {
                    activity.park(drain_request_body(body)).await
                }
            }
        };
        let _ = result_tx.send(result);
        Ok(())
    }
}

/// Drains a replayed request body whose oplog recording may be incomplete,
/// completing the recording as it goes: the recorded frames are summarized
/// (merged by offset — frame recordings are appended by concurrent tasks, so
/// oplog order is not pull order, and crash/re-exec generations may overlap),
/// and if no terminal frame was recorded, the guest's re-produced bytes past
/// the covered prefix are appended as new frames. Bytes inside the covered
/// prefix are discarded by byte count, because the guest's frame boundaries
/// can differ between runs.
///
/// Scan or append failures never fail the drain: the guest's transmission
/// result must not depend on recording health. They only leave the recording
/// incomplete, which a later rebuild detects and refuses.
pub(super) async fn drain_replayed_request_body_completing_recording(
    body: UnsyncBoxBody<Bytes, ErrorCode>,
    oplog: Arc<dyn Oplog>,
    send_start_index: OplogIndex,
    activity: &TailActivity,
) -> Result<(), ErrorCode> {
    let scan = match scan_recorded_request_body_frames(oplog.clone(), send_start_index).await {
        Ok(scan) => scan,
        Err(error) => {
            warn!(
                send_start_index = %send_start_index,
                error = %error,
                "Failed to scan the recorded p3 HTTP request-body frames; draining the replayed request body without completing its recording"
            );
            // Safe park: the fallback drain appends nothing (guest-driven only).
            return activity.park(drain_request_body(body)).await;
        }
    };
    if scan.terminal_recorded() {
        // The recording completed after the send result was recorded (its
        // terminal frame just landed later in the oplog): nothing to append.
        // Safe park: this drain appends nothing (guest-driven only).
        return activity.park(drain_request_body(body)).await;
    }
    drain_past_recorded_prefix(body, oplog, send_start_index, scan, activity).await
}

/// The incomplete-recording drain core: discards the guest's re-produced bytes
/// up to `scan.covered_len` (splitting a frame that straddles the boundary),
/// records everything past it — data at its byte offset, trailers unless
/// already recorded, and always a terminal frame — and returns the guest
/// body's terminal result.
async fn drain_past_recorded_prefix(
    mut body: UnsyncBoxBody<Bytes, ErrorCode>,
    oplog: Arc<dyn Oplog>,
    send_start_index: OplogIndex,
    scan: RecordedRequestBodyScan,
    activity: &TailActivity,
) -> Result<(), ErrorCode> {
    let mut recording = true;
    let mut trailers_recorded = scan.trailers_recorded();
    let mut pos: u64 = 0;
    // Safe park: each frame is guest-(re)produced body data; the appends
    // between frames stay active.
    while let Some(frame) = activity.park(body.frame()).await {
        match frame {
            Ok(frame) => match frame.into_data().map_err(http_body::Frame::into_trailers) {
                Ok(data) => {
                    let end = pos.saturating_add(data.len() as u64);
                    if recording && end > scan.covered_len {
                        let skip = scan.covered_len.saturating_sub(pos);
                        recording = record_frame_or_warn(
                            &oplog,
                            send_start_index,
                            SerializableP3HttpRequestBodyFrame::Data {
                                offset: pos.max(scan.covered_len),
                                bytes: data.slice(skip as usize..).to_vec(),
                            },
                        )
                        .await;
                    }
                    pos = end;
                }
                Err(Ok(trailers)) => {
                    if recording && !trailers_recorded {
                        recording = record_frame_or_warn(
                            &oplog,
                            send_start_index,
                            SerializableP3HttpRequestBodyFrame::Trailers(Some(serialize_headers(
                                &trailers,
                            ))),
                        )
                        .await;
                        trailers_recorded = true;
                    }
                }
                Err(Err(_)) => {}
            },
            Err(error) => {
                if recording {
                    record_frame_or_warn(
                        &oplog,
                        send_start_index,
                        SerializableP3HttpRequestBodyFrame::Error(serialize_error_code(&error)),
                    )
                    .await;
                }
                return Err(error);
            }
        }
    }
    if recording {
        record_frame_or_warn(
            &oplog,
            send_start_index,
            SerializableP3HttpRequestBodyFrame::End,
        )
        .await;
    }
    Ok(())
}

/// Appends one frame to the send's recording, returning whether recording may
/// continue. An append failure stops recording (a later frame would leave a
/// gap that hides the failure) but never fails the drain itself.
async fn record_frame_or_warn(
    oplog: &Arc<dyn Oplog>,
    send_start_index: OplogIndex,
    frame: SerializableP3HttpRequestBodyFrame,
) -> bool {
    match record_frame_entry(oplog.clone(), send_start_index, frame).await {
        Ok(_) => true,
        Err(error) => {
            warn!(
                send_start_index = %send_start_index,
                error = %error,
                "Failed to append a p3 HTTP request-body frame while completing a replayed send's recording; leaving the recording incomplete"
            );
            false
        }
    }
}

pub(super) fn replay_send_response<Ctx: WorkerCtx, U: Send>(
    store: &Accessor<U, DurableP3<Ctx>>,
    result: SerializableP3HttpClientSendResult,
) -> HttpResult<Resource<Response>> {
    match result {
        SerializableP3HttpClientSendResult::SuccessWithRecordedRequestBody { headers, .. } => {
            response_from_recorded_headers::<Ctx, U>(store, headers)
        }
        SerializableP3HttpClientSendResult::HttpError(error) => {
            Err(deserialize_error_code(error).into())
        }
    }
}

pub(super) fn response_from_recorded_headers<Ctx: WorkerCtx, U: Send>(
    store: &Accessor<U, DurableP3<Ctx>>,
    recorded: SerializableResponseHeaders,
) -> HttpResult<Resource<Response>> {
    let status = http::StatusCode::from_u16(recorded.status).map_err(HttpError::trap)?;
    let mut headers = HeaderMap::new();
    for (name, values) in recorded.headers {
        let name = HeaderName::try_from(name).map_err(HttpError::trap)?;
        for value in values {
            headers.append(
                name.clone(),
                HeaderValue::try_from(value).map_err(HttpError::trap)?,
            );
        }
    }

    let mut response = http::Response::new(Empty::<Bytes>::new());
    *response.status_mut() = status;
    *response.headers_mut() = headers;
    let (response, _io) = wasmtime_wasi_http::p3::Response::from_http(response);
    let http_store = store.with_getter::<WasiHttp>(wasi_http_view::<Ctx, U>);
    http_store.with(|mut access| {
        access
            .get()
            .table
            .push(response)
            .context("failed to push replayed p3 HTTP response to table")
            .map_err(wasmtime::Error::from_anyhow)
            .map_err(HttpError::trap)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::durable_host::p3::http::test_support::*;
    use core::pin::Pin;
    use core::task::{Context, Poll};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use test_r::{test, timeout};
    use wasmtime::component::Access;
    use wasmtime::{AsContextMut, Engine, Store};
    use wasmtime_wasi_http::p3::bindings::http::types;
    use wasmtime_wasi_http::p3::{WasiHttp, WasiHttpView};

    fn test_activity() -> TailActivity {
        crate::durable_host::tail_work::TailWorkTracker::new().activity()
    }

    struct TrackedChunk {
        live_chunks: Arc<AtomicUsize>,
        bytes: [u8; 1],
    }

    impl TrackedChunk {
        fn new(live_chunks: Arc<AtomicUsize>) -> Self {
            live_chunks.fetch_add(1, Ordering::SeqCst);
            Self {
                live_chunks,
                bytes: *b"x",
            }
        }
    }

    impl AsRef<[u8]> for TrackedChunk {
        fn as_ref(&self) -> &[u8] {
            &self.bytes
        }
    }

    impl Drop for TrackedChunk {
        fn drop(&mut self) {
            self.live_chunks.fetch_sub(1, Ordering::SeqCst);
        }
    }

    struct RetainDetectingBody {
        remaining: usize,
        live_chunks: Arc<AtomicUsize>,
    }

    impl RetainDetectingBody {
        fn new(chunks: usize) -> Self {
            Self {
                remaining: chunks,
                live_chunks: Arc::new(AtomicUsize::new(0)),
            }
        }
    }

    impl http_body::Body for RetainDetectingBody {
        type Data = Bytes;
        type Error = ErrorCode;

        fn poll_frame(
            mut self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
        ) -> Poll<Option<Result<http_body::Frame<Self::Data>, Self::Error>>> {
            if self.remaining == 0 {
                return Poll::Ready(None);
            }
            if self.live_chunks.load(Ordering::SeqCst) != 0 {
                self.remaining = 0;
                return Poll::Ready(Some(Err(ErrorCode::InternalError(Some(
                    "previous request-body frame was retained until a later poll".to_string(),
                )))));
            }
            self.remaining -= 1;
            Poll::Ready(Some(Ok(http_body::Frame::data(Bytes::from_owner(
                TrackedChunk::new(self.live_chunks.clone()),
            )))))
        }
    }
    /// Replay request-body draining must be a streaming drain: its purpose is to
    /// unblock guest uploads and resolve the request transmission future, not to
    /// retain the whole upload in memory. This body reports an error if a prior
    /// data frame is still live when the next frame is polled; the frame-by-frame
    /// `drain_request_body` passes, while `BodyExt::collect` would retain all
    /// frames until EOF and fail.
    #[test]
    #[timeout("10s")]
    async fn replay_request_body_drain_does_not_retain_frames_until_eof() {
        let body = RetainDetectingBody::new(2).boxed_unsync();
        let result = drain_request_body(body).await;
        assert!(
            matches!(result, Ok(())),
            "replay request drain must discard each frame as it is read; got {result:?}"
        );
    }

    /// Interim regression guard for how `consume_replayed_request` resolves the
    /// guest's request-body transmission future while that result is not yet
    /// recorded in the oplog.
    ///
    /// A naive `HostRequestWithStore::drop` resolves the future to `Ok(())` (a
    /// dropped `result_tx` is treated as success), losing any drain-observed
    /// body error — here a `content-length: 4` header with a 1-byte body, which
    /// content-length validation reports as `HttpRequestBodySize`.
    /// `consume_replayed_request` instead deletes the request and drains its
    /// body via `into_http` (in a spawned task), resolving the transmission
    /// future from the drain result. This pins that drain-derived policy so the
    /// replay path is not regressed back to the `drop` shortcut.
    ///
    /// This is a *replay-local* invariant, not "matches live": whether live
    /// actually surfaced this error depends on whether the network read the
    /// body, which is not recorded (see
    /// `request_body_transmission_result_depends_on_unrecorded_body_read`).
    ///
    /// Note: this exercises a host-backed body (`Body::Host`) as a stand-in.
    /// Driving a real guest body stream (`Body::Guest`) on replay needs a
    /// wasip3 HTTP component harness, which does not exist in-repo yet.
    #[test]
    async fn replay_request_consume_drain_surfaces_deterministic_body_error() {
        let engine = Engine::default();

        // The `drop` shortcut loses the deterministic error — the bug we avoid.
        let (drop_request, drop_transmission) = short_content_length_request();
        let mut drop_store = Store::new(&engine, TestHttpCtx::default());
        let drop_handle = drop_store
            .data_mut()
            .table
            .push(drop_request)
            .expect("request should be pushed to the resource table");
        let drop_access =
            Access::<TestHttpCtx, WasiHttp>::new(drop_store.as_context_mut(), TestHttpCtx::http);
        <WasiHttp as types::HostRequestWithStore<TestHttpCtx>>::drop(drop_access, drop_handle)
            .expect("request drop should succeed");
        let drop_transmission_result = drop_transmission.await;
        assert!(
            matches!(drop_transmission_result, Ok(())),
            "dropping the request loses the deterministic body transmission error, got {drop_transmission_result:?}"
        );

        // The delete + `into_http` + drain sequence used by
        // `consume_replayed_request` preserves the error.
        let (replay_request, replay_transmission) = short_content_length_request();
        let mut replay_store = Store::new(&engine, TestHttpCtx::default());
        let (replay_http_request, _) = replay_request
            .into_http(&mut replay_store, async { Ok(()) })
            .expect("request should convert to an HTTP request");
        let _ = replay_http_request.into_body().collect().await;
        let replay_transmission_result = replay_transmission.await;
        assert!(
            matches!(
                replay_transmission_result,
                Err(ErrorCode::HttpRequestBodySize(_))
            ),
            "replay consume must preserve the live request body transmission error, got {replay_transmission_result:?}"
        );
    }

    /// Documents why the request-body transmission future cannot be replayed
    /// exactly without recording its result: the *same* request shape yields two
    /// different transmission results depending only on whether the outgoing
    /// body is read, and that fact is not in the oplog.
    ///
    /// * Dropped unread — models a live transport failure that occurs before the
    ///   body is read (e.g. connection refused). Content-length validation never
    ///   runs, so the transmission future resolves `Ok(())`.
    /// * Drained to EOF — what the replay path does, and what a live send that
    ///   reads the body does. The short body fails content-length validation, so
    ///   the transmission future resolves `Err(HttpRequestBodySize)`.
    ///
    /// `client::send` records only the response head, not which of these
    /// occurred, so the drain-derived replay result (see
    /// `replay_request_consume_drain_surfaces_deterministic_body_error`) is a
    /// best-effort interim. Recording the transmission result itself is the
    /// follow-up that closes this gap; item #8 stays blocked on it.
    #[test]
    async fn request_body_transmission_result_depends_on_unrecorded_body_read() {
        let engine = Engine::default();

        // Dropped unread: no content-length validation runs, resolves `Ok(())`.
        let (dropped_request, dropped_transmission) = short_content_length_request();
        let mut dropped_store = Store::new(&engine, TestHttpCtx::default());
        let (dropped_http_request, _) = dropped_request
            .into_http(&mut dropped_store, async { Ok(()) })
            .expect("request should convert to an HTTP request");
        drop(dropped_http_request);
        let dropped_result = dropped_transmission.await;
        assert!(
            matches!(dropped_result, Ok(())),
            "dropping the body unread should not surface content-length validation, got {dropped_result:?}"
        );

        // Drained to EOF: the short body fails content-length validation.
        let (drained_request, drained_transmission) = short_content_length_request();
        let mut drained_store = Store::new(&engine, TestHttpCtx::default());
        let (drained_http_request, _) = drained_request
            .into_http(&mut drained_store, async { Ok(()) })
            .expect("request should convert to an HTTP request");
        let _ = drained_http_request.into_body().collect().await;
        let drained_result = drained_transmission.await;
        assert!(
            matches!(drained_result, Err(ErrorCode::HttpRequestBodySize(_))),
            "draining the short body should surface content-length validation, got {drained_result:?}"
        );
    }

    /// The replay request drain must feed deterministic outgoing-body failures
    /// back to the guest's request-body transmission future even when there is
    /// no `content-length` validation wrapper to carry the error. Live `send`
    /// wires the transmission future to the request I/O result, so replay must
    /// wire it to the local drain result (`consume_replayed_request` does this
    /// via a `oneshot`) instead of resolving it with an unconditional `Ok(())`.
    ///
    /// This mirrors the `consume_replayed_request` wiring with a host body that
    /// deterministically errors and no content-length header. A naive
    /// `async { Ok(()) }` transmission future would lose the error.
    #[test]
    async fn replay_request_consume_preserves_body_error_without_content_length() {
        let engine = Engine::default();
        let (request, transmission) = erroring_body_request_without_content_length();
        let mut replay_store = Store::new(&engine, TestHttpCtx::default());

        // Mirror the fixed `consume_replayed_request`: wire the transmission
        // future to the drain result via a oneshot, drain the body, then report
        // the drain result.
        let (drain_result_tx, drain_result_rx) = oneshot::channel::<Result<(), ErrorCode>>();
        let (replay_http_request, _) = request
            .into_http(&mut replay_store, async move {
                drain_result_rx.await.unwrap_or(Ok(()))
            })
            .expect("request should convert to an HTTP request");
        let drain_result = replay_http_request.into_body().collect().await.map(|_| ());
        assert!(
            matches!(drain_result, Err(ErrorCode::HttpProtocolError)),
            "body drain should observe the deterministic error, got {drain_result:?}"
        );
        let _ = drain_result_tx.send(drain_result);

        let replay_transmission_result = transmission.await;
        assert!(
            matches!(
                replay_transmission_result,
                Err(ErrorCode::HttpProtocolError)
            ),
            "replay consume must propagate deterministic body errors to the request body transmission future, got {replay_transmission_result:?}"
        );
    }

    /// The parent index used for the send under test. `NONE` (0) makes the
    /// scan window (`parent.next()` ..= oplog end) cover a fresh test oplog
    /// from its first entry, so tests do not need a placeholder `Start` entry.
    const PARENT: OplogIndex = OplogIndex::NONE;

    async fn record(
        oplog: &Arc<FrameTestOplog>,
        parent: OplogIndex,
        frame: SerializableP3HttpRequestBodyFrame,
    ) {
        record_frame_entry(oplog.clone(), parent, frame)
            .await
            .expect("failed to record a test frame");
    }

    fn data(offset: u64, bytes: &[u8]) -> SerializableP3HttpRequestBodyFrame {
        SerializableP3HttpRequestBodyFrame::Data {
            offset,
            bytes: bytes.to_vec(),
        }
    }

    /// Reconstructs the recorded body bytes from offset-keyed data frames
    /// (overlaps and duplicates allowed, gaps fail the test) and counts the
    /// trailers / `End` / `Error` frames.
    fn summarize_recording(
        frames: Vec<SerializableP3HttpRequestBodyFrame>,
    ) -> (Vec<u8>, usize, usize, usize) {
        let mut data_frames = Vec::new();
        let mut trailers = 0;
        let mut ends = 0;
        let mut errors = 0;
        for frame in frames {
            match frame {
                SerializableP3HttpRequestBodyFrame::Data { offset, bytes } => {
                    data_frames.push((offset, bytes))
                }
                SerializableP3HttpRequestBodyFrame::Trailers(_) => trailers += 1,
                SerializableP3HttpRequestBodyFrame::End => ends += 1,
                SerializableP3HttpRequestBodyFrame::Error(_) => errors += 1,
            }
        }
        data_frames.sort();
        let mut body: Vec<u8> = Vec::new();
        for (offset, bytes) in data_frames {
            let offset = offset as usize;
            assert!(
                offset <= body.len(),
                "gap in the recorded data frames at offset {offset}"
            );
            let overlap = (body.len() - offset).min(bytes.len());
            body.extend_from_slice(&bytes[overlap..]);
        }
        (body, trailers, ends, errors)
    }

    /// Data frames from different crash/re-exec generations may duplicate or
    /// overlap, and their oplog order is append order, not pull order; the
    /// scan must still report the merged contiguous coverage from offset 0.
    #[test]
    #[timeout("10s")]
    async fn recorded_request_body_scan_merges_overlapping_generations() {
        let oplog = FrameTestOplog::new();
        record(&oplog, PARENT, data(3, b"lo")).await;
        record(&oplog, PARENT, data(0, b"hel")).await;
        record(&oplog, PARENT, data(0, b"hell")).await;
        let scan = scan_recorded_request_body_frames(oplog.clone(), PARENT)
            .await
            .expect("scan failed");
        assert_eq!(scan.covered_len, 5);
        assert!(!scan.trailers_recorded());
        assert!(!scan.terminal_recorded());
    }

    /// Coverage is the contiguous prefix from offset 0: a recorded frame past
    /// a gap must not extend it (the gap is exactly what the replay drain has
    /// to fill in).
    #[test]
    #[timeout("10s")]
    async fn recorded_request_body_scan_stops_at_first_gap() {
        let oplog = FrameTestOplog::new();
        record(&oplog, PARENT, data(0, b"ab")).await;
        record(&oplog, PARENT, data(4, b"cd")).await;
        record(
            &oplog,
            PARENT,
            SerializableP3HttpRequestBodyFrame::Trailers(None),
        )
        .await;
        let scan = scan_recorded_request_body_frames(oplog.clone(), PARENT)
            .await
            .expect("scan failed");
        assert_eq!(scan.covered_len, 2);
        assert!(scan.trailers_recorded());
        assert!(!scan.terminal_recorded());
    }

    /// Frames keyed to a different `Start` — e.g. an abandoned send from a
    /// crashed generation — must be invisible to the scan: neither their data
    /// coverage nor their trailers/terminal flags may leak in. The parent-key
    /// filter is the skip-on-read for abandoned starts.
    #[test]
    #[timeout("10s")]
    async fn recorded_request_body_scan_skips_frames_of_other_sends() {
        let foreign = OplogIndex::from_u64(999);
        let oplog = FrameTestOplog::new();
        record(&oplog, PARENT, data(0, b"ab")).await;
        record(&oplog, foreign, data(0, b"zzzzzz")).await;
        record(
            &oplog,
            foreign,
            SerializableP3HttpRequestBodyFrame::Trailers(None),
        )
        .await;
        record(&oplog, foreign, SerializableP3HttpRequestBodyFrame::End).await;
        let scan = scan_recorded_request_body_frames(oplog.clone(), PARENT)
            .await
            .expect("scan failed");
        assert_eq!(scan.covered_len, 2);
        assert!(!scan.trailers_recorded());
        assert!(!scan.terminal_recorded());
    }

    /// An interrupted recording (data prefix, no terminal) is completed by the
    /// drain even when the guest re-produces the body with different frame
    /// boundaries: bytes inside the covered prefix are discarded by byte
    /// count, a frame straddling the boundary is split, and only the missing
    /// suffix plus the terminal is appended.
    #[test]
    #[timeout("10s")]
    async fn replay_drain_completes_interrupted_recording_across_frame_boundaries() {
        let oplog = FrameTestOplog::new();
        record(&oplog, PARENT, data(0, b"hello")).await;
        let body = frame_body(
            vec![
                http_body::Frame::data(Bytes::from_static(b"hel")),
                http_body::Frame::data(Bytes::from_static(b"lo w")),
                http_body::Frame::data(Bytes::from_static(b"orld")),
            ],
            None,
        );
        let result = drain_replayed_request_body_completing_recording(
            body,
            oplog.clone(),
            PARENT,
            &test_activity(),
        )
        .await;
        assert!(matches!(result, Ok(())), "drain failed: {result:?}");
        let (bytes, trailers, ends, errors) =
            summarize_recording(oplog.recorded_frames_for(PARENT));
        assert_eq!(bytes, b"hello world");
        assert_eq!((trailers, ends, errors), (0, 1, 0));
    }

    /// A recording whose terminal frame landed in the oplog (even after the
    /// send result) is complete: the drain must only discard the guest's
    /// re-produced body and append nothing.
    #[test]
    #[timeout("10s")]
    async fn replay_drain_leaves_complete_recording_untouched() {
        let oplog = FrameTestOplog::new();
        record(&oplog, PARENT, data(0, b"abc")).await;
        record(&oplog, PARENT, SerializableP3HttpRequestBodyFrame::End).await;
        let entries_before = oplog.entry_count();
        let body = frame_body(
            vec![http_body::Frame::data(Bytes::from_static(b"abc"))],
            None,
        );
        let result = drain_replayed_request_body_completing_recording(
            body,
            oplog.clone(),
            PARENT,
            &test_activity(),
        )
        .await;
        assert!(matches!(result, Ok(())), "drain failed: {result:?}");
        assert_eq!(oplog.entry_count(), entries_before);
    }

    /// Guest trailers past the covered prefix are recorded when the recording
    /// is missing them.
    #[test]
    #[timeout("10s")]
    async fn replay_drain_records_missing_trailers() {
        let oplog = FrameTestOplog::new();
        record(&oplog, PARENT, data(0, b"abc")).await;
        let mut guest_trailers = HeaderMap::new();
        guest_trailers.insert("x-checksum", HeaderValue::from_static("abc"));
        let body = frame_body(
            vec![
                http_body::Frame::data(Bytes::from_static(b"abc")),
                http_body::Frame::trailers(guest_trailers),
            ],
            None,
        );
        let result = drain_replayed_request_body_completing_recording(
            body,
            oplog.clone(),
            PARENT,
            &test_activity(),
        )
        .await;
        assert!(matches!(result, Ok(())), "drain failed: {result:?}");
        let (bytes, trailers, ends, errors) =
            summarize_recording(oplog.recorded_frames_for(PARENT));
        assert_eq!(bytes, b"abc");
        assert_eq!((trailers, ends, errors), (1, 1, 0));
    }

    /// Trailers that are already part of the recording must not be duplicated
    /// when the guest re-produces them.
    #[test]
    #[timeout("10s")]
    async fn replay_drain_does_not_duplicate_recorded_trailers() {
        let oplog = FrameTestOplog::new();
        record(&oplog, PARENT, data(0, b"abc")).await;
        let mut recorded_trailers = HeaderMap::new();
        recorded_trailers.insert("x-checksum", HeaderValue::from_static("abc"));
        record(
            &oplog,
            PARENT,
            SerializableP3HttpRequestBodyFrame::Trailers(Some(serialize_headers(
                &recorded_trailers,
            ))),
        )
        .await;
        let mut guest_trailers = HeaderMap::new();
        guest_trailers.insert("x-checksum", HeaderValue::from_static("abc"));
        let body = frame_body(
            vec![
                http_body::Frame::data(Bytes::from_static(b"abc")),
                http_body::Frame::trailers(guest_trailers),
            ],
            None,
        );
        let result = drain_replayed_request_body_completing_recording(
            body,
            oplog.clone(),
            PARENT,
            &test_activity(),
        )
        .await;
        assert!(matches!(result, Ok(())), "drain failed: {result:?}");
        let (bytes, trailers, ends, errors) =
            summarize_recording(oplog.recorded_frames_for(PARENT));
        assert_eq!(bytes, b"abc");
        assert_eq!((trailers, ends, errors), (1, 1, 0));
    }

    /// A guest body error during the completing drain is recorded as the
    /// terminal frame AND surfaced as the drain result, so the transmission
    /// future sees the same deterministic error as the plain drain path.
    #[test]
    #[timeout("10s")]
    async fn replay_drain_records_error_terminal_and_returns_the_error() {
        let oplog = FrameTestOplog::new();
        record(&oplog, PARENT, data(0, b"ab")).await;
        let body = frame_body(
            vec![http_body::Frame::data(Bytes::from_static(b"ab"))],
            Some(ErrorCode::HttpProtocolError),
        );
        let result = drain_replayed_request_body_completing_recording(
            body,
            oplog.clone(),
            PARENT,
            &test_activity(),
        )
        .await;
        assert!(
            matches!(result, Err(ErrorCode::HttpProtocolError)),
            "drain must surface the guest body error, got {result:?}"
        );
        let (bytes, trailers, ends, errors) =
            summarize_recording(oplog.recorded_frames_for(PARENT));
        assert_eq!(bytes, b"ab");
        assert_eq!((trailers, ends, errors), (0, 0, 1));
    }

    /// A send with no recorded frames at all (the crash hit before any frame
    /// landed) still gets its recording completed: an empty guest body records
    /// just the `End` terminal.
    #[test]
    #[timeout("10s")]
    async fn replay_drain_records_end_for_empty_unrecorded_body() {
        let oplog = FrameTestOplog::new();
        let body = frame_body(vec![], None);
        let result = drain_replayed_request_body_completing_recording(
            body,
            oplog.clone(),
            PARENT,
            &test_activity(),
        )
        .await;
        assert!(matches!(result, Ok(())), "drain failed: {result:?}");
        let (bytes, trailers, ends, errors) =
            summarize_recording(oplog.recorded_frames_for(PARENT));
        assert!(bytes.is_empty());
        assert_eq!((trailers, ends, errors), (0, 1, 0));
    }
}
