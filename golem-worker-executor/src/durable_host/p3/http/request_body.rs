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

use super::serialization::{
    deserialize_error_code, deserialize_headers, serialize_error_code, serialize_headers,
};
use crate::durable_host::concurrent::{CallHandle, CallReplayOutcome, LeaveIncompleteOnDrop};
use crate::durable_host::durability::{DurableCallTrapContext, mark_durable_call_trap_context};
use crate::durable_host::p3::{
    DurableP3, durable_worker_ctx, observe_function_call_store, wasi_http_view,
};
use crate::services::oplog::{Oplog, OplogOps};
use crate::workerctx::WorkerCtx;
use bytes::Bytes;
use golem_common::model::oplog::host_functions::P3HttpClientRequestBodyTransmission;
use golem_common::model::oplog::payload::types::{
    SerializableHttpErrorCode, SerializableP3HttpRequestBodyFrame,
};
use golem_common::model::oplog::{
    DurableFunctionType, HostRequest, HostRequestNoInput, HostRequestP3HttpClientRequestBodyFrame,
    HostResponseP3HttpClientRequestBodyTransmission, HostStreamKind, OplogEntry, OplogIndex,
};
use golem_common::serialization::serialize;
use http_body::Body as HttpBody;
use http_body::Frame;
use http_body::SizeHint;
use http_body_util::BodyExt as _;
use http_body_util::combinators::UnsyncBoxBody;
use std::future::Future;
use std::marker::PhantomData;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};
use tokio::sync::oneshot;
use wasmtime::component::{
    Access, Accessor, AccessorTask, FutureConsumer, FutureProducer, FutureReader, Resource, Source,
    StreamReader,
};
use wasmtime::{AsContextMut, StoreContextMut};
use wasmtime_wasi_http::p3::WasiHttp;
use wasmtime_wasi_http::p3::bindings::http::types;
use wasmtime_wasi_http::p3::bindings::http::types::{
    ErrorCode, Headers, Request, RequestOptions, Trailers,
};

/// Upper bound on the number of request-body frame recordings that may be in
/// flight (spawned but not yet appended to the oplog) at the same time. When
/// the window is full, pulling the next frame from the guest body waits until
/// a recording lands, so oplog backpressure propagates to the request-body
/// write side and the in-memory footprint of unrecorded frames stays bounded.
const REQUEST_BODY_RECORDING_WINDOW: usize = 4;

/// Durable recorder and resend source for the outgoing request body of a p3
/// `client::send`.
///
/// The live guest body is pulled only on demand: by the active attempt's
/// [`DurableRequestBodyView`] while the request is written to the network, or
/// by [`Self::drain_to_terminal`] when a status-code retry must consume the
/// rest of the body before resending. Every pulled frame is persisted as a
/// `HostStreamFrame` hint entry (kind [`HostStreamKind::P3HttpRequestBody`])
/// attached to the send's `Start` index, through a bounded in-flight window.
/// Only the oplog index of each recorded frame is kept in memory — never the
/// bytes — so resending an arbitrarily large body needs bounded memory. A
/// terminal frame (`End` or `Error`) is always recorded, including for
/// bodiless sends. While persistence is off (`PersistNothing`) or a snapshot
/// is being taken, recording is skipped: the body still streams, but only an
/// unpulled body can be resent.
#[derive(Clone)]
pub(super) struct DurableRequestBody {
    oplog: Arc<dyn Oplog>,
    parent_start_index: OplogIndex,
    recording_enabled: bool,
    state: Arc<Mutex<DurableRequestBodyState>>,
}

struct DurableRequestBodyState {
    inner: Pin<Box<UnsyncBoxBody<Bytes, ErrorCode>>>,
    /// One slot per pulled data/trailers frame, in pull order. Terminal frames
    /// get no slot: the in-memory `terminal` drives the views instead.
    slots: Vec<RecordedFrameSlot>,
    /// Byte offset of the next data frame (cumulative pulled data length).
    next_offset: u64,
    /// Number of data/trailers frames pulled from the live guest body,
    /// independent of whether they were recorded. Distinguishes a genuinely
    /// empty body (replayable even without a recording) from a pulled body
    /// whose frames were not recorded (not replayable).
    pulled_frames: usize,
    /// Frame recordings spawned but not yet appended to the oplog.
    pending_recordings: usize,
    terminal: Option<RequestBodyTerminal>,
    /// First frame-recording failure; refuses resends and fails views.
    recording_failed: Option<String>,
    live_polled: bool,
    active_live_view: bool,
    wakers: Vec<Waker>,
}

struct RecordedFrameSlot {
    /// Data length of the frame (0 for trailers), for size hints.
    data_len: u64,
    recording: FrameRecording,
}

enum FrameRecording {
    /// The recording task is spawned; the oplog index is not known yet.
    InFlight,
    /// The frame's `HostStreamFrame` entry lives at this oplog index.
    Recorded(OplogIndex),
}

enum RequestBodyTerminal {
    End,
    Error(ErrorCode),
}

/// Outcome of [`DurableRequestBody::drain_to_terminal`].
pub(super) enum DurableRequestBodyDrainOutcome {
    /// The whole body (up to a clean end) is recorded: a resend can replay it.
    Replayable,
    /// The guest body failed, a frame recording failed, or the pulled frames
    /// were not recorded: resending is refused.
    NotReplayable,
}

impl DurableRequestBodyState {
    fn wake_all(&mut self) {
        for waker in self.wakers.drain(..) {
            waker.wake();
        }
    }

    fn park(&mut self, cx: &Context<'_>) {
        let waker = cx.waker();
        if !self.wakers.iter().any(|existing| existing.will_wake(waker)) {
            self.wakers.push(waker.clone());
        }
    }

    fn is_unpulled(&self) -> bool {
        !self.live_polled && self.slots.is_empty() && self.terminal.is_none()
    }
}

impl DurableRequestBody {
    pub(super) fn new(
        body: UnsyncBoxBody<Bytes, ErrorCode>,
        oplog: Arc<dyn Oplog>,
        parent_start_index: OplogIndex,
        recording_enabled: bool,
    ) -> Self {
        Self {
            oplog,
            parent_start_index,
            recording_enabled,
            state: Arc::new(Mutex::new(DurableRequestBodyState {
                inner: Box::pin(body),
                slots: Vec::new(),
                next_offset: 0,
                pulled_frames: 0,
                pending_recordings: 0,
                terminal: None,
                recording_failed: None,
                live_polled: false,
                active_live_view: false,
                wakers: Vec::new(),
            })),
        }
    }

    /// A body for one send attempt: serves the already-recorded frames from
    /// the oplog first, then claims the live guest body and continues pulling
    /// (recording as it goes). At most one view may be live at a time.
    pub(super) fn replayer(&self) -> DurableRequestBodyView {
        DurableRequestBodyView {
            shared: self.clone(),
            pos: 0,
            live_claimed: false,
            pending_load: None,
        }
    }

    /// Whether a failed send attempt may be retried by resending this body:
    /// nothing was pulled yet (a fresh body), or the body ended cleanly with
    /// every frame recorded for replay. A guest body error, a recording
    /// failure, or unrecorded pulled frames refuse the resend.
    pub(super) fn can_replay_after_send_failure(&self) -> bool {
        let state = self.lock_state();
        if state.recording_failed.is_some() {
            return false;
        }
        match &state.terminal {
            Some(RequestBodyTerminal::Error(_)) => false,
            Some(RequestBodyTerminal::End) => self.recording_enabled || state.pulled_frames == 0,
            None => state.is_unpulled(),
        }
    }

    /// Whether any data or trailers frames were already pulled from the live
    /// guest body — i.e. a failed attempt had started writing the request
    /// body to the network. Terminal frames alone (a bodiless send) do not
    /// count.
    pub(super) fn frames_consumed(&self) -> bool {
        self.lock_state().pulled_frames > 0
    }

    /// Whether pulled frames are being persisted as oplog entries (i.e. not in
    /// `PersistNothing` or snapshotting mode).
    pub(super) fn recording_enabled(&self) -> bool {
        self.recording_enabled
    }

    /// Whether the recording is complete *right now*: the body reached a
    /// terminal, every spawned frame append (including the terminal frame's)
    /// has landed in the oplog, and none failed. Appends run concurrently with
    /// the send, so a `false` here is not final — the recording may still
    /// complete later; a replayed drain then finds the terminal by scanning.
    pub(super) fn recording_complete(&self) -> bool {
        let state = self.lock_state();
        self.recording_enabled
            && state.terminal.is_some()
            && state.pending_recordings == 0
            && state.recording_failed.is_none()
    }

    /// Force-releases the live claim of an abandoned attempt's view (the
    /// attempt's request/connection may be dropped asynchronously), so a drain
    /// or a subsequent view can pull the body.
    pub(super) fn abandon_active_live_view(&self) {
        let mut state = self.lock_state();
        state.active_live_view = false;
        state.wake_all();
    }

    /// Drain mode for a status-code retry: pulls the rest of the guest body
    /// without an attempt demanding it, recording every frame, and resolves
    /// once the terminal is known.
    pub(super) async fn drain_to_terminal(&self) -> DurableRequestBodyDrainOutcome {
        futures::future::poll_fn(|cx| self.poll_drain(cx)).await
    }

    fn poll_drain(&self, cx: &mut Context<'_>) -> Poll<DurableRequestBodyDrainOutcome> {
        let mut state = self.lock_state();
        loop {
            if state.recording_failed.is_some() {
                return Poll::Ready(DurableRequestBodyDrainOutcome::NotReplayable);
            }
            match &state.terminal {
                Some(RequestBodyTerminal::End) => {
                    return Poll::Ready(if self.recording_enabled || state.pulled_frames == 0 {
                        DurableRequestBodyDrainOutcome::Replayable
                    } else {
                        DurableRequestBodyDrainOutcome::NotReplayable
                    });
                }
                Some(RequestBodyTerminal::Error(_)) => {
                    return Poll::Ready(DurableRequestBodyDrainOutcome::NotReplayable);
                }
                None => {}
            }
            if state.active_live_view {
                state.park(cx);
                return Poll::Pending;
            }
            if self.recording_enabled && state.pending_recordings >= REQUEST_BODY_RECORDING_WINDOW {
                state.park(cx);
                return Poll::Pending;
            }
            match state.inner.as_mut().poll_frame(cx) {
                Poll::Ready(Some(Ok(frame))) => {
                    state.live_polled = true;
                    let _ = self.accept_live_frame(&mut state, frame);
                }
                Poll::Ready(Some(Err(error))) => {
                    state.live_polled = true;
                    self.record_terminal(&mut state, RequestBodyTerminal::Error(error));
                }
                Poll::Ready(None) => {
                    state.live_polled = true;
                    self.record_terminal(&mut state, RequestBodyTerminal::End);
                }
                Poll::Pending => return Poll::Pending,
            }
        }
    }

    /// Bookkeeping for one frame pulled from the live guest body: records it
    /// (when recording is enabled) and returns it for delivery. Frames that
    /// are neither data nor trailers are dropped (`None`).
    fn accept_live_frame(
        &self,
        state: &mut DurableRequestBodyState,
        frame: Frame<Bytes>,
    ) -> Option<Frame<Bytes>> {
        match frame.into_data().map_err(Frame::into_trailers) {
            Ok(data) => {
                state.pulled_frames += 1;
                if self.recording_enabled {
                    self.spawn_frame_recording(
                        state,
                        Some(data.len() as u64),
                        SerializableP3HttpRequestBodyFrame::Data {
                            offset: state.next_offset,
                            bytes: data.to_vec(),
                        },
                    );
                }
                state.next_offset = state.next_offset.saturating_add(data.len() as u64);
                Some(Frame::data(data))
            }
            Err(Ok(trailers)) => {
                state.pulled_frames += 1;
                if self.recording_enabled {
                    self.spawn_frame_recording(
                        state,
                        Some(0),
                        SerializableP3HttpRequestBodyFrame::Trailers(Some(serialize_headers(
                            &trailers,
                        ))),
                    );
                }
                Some(Frame::trailers(trailers))
            }
            Err(Err(_)) => None,
        }
    }

    fn record_terminal(&self, state: &mut DurableRequestBodyState, terminal: RequestBodyTerminal) {
        if self.recording_enabled {
            let frame = match &terminal {
                RequestBodyTerminal::End => SerializableP3HttpRequestBodyFrame::End,
                RequestBodyTerminal::Error(error) => {
                    SerializableP3HttpRequestBodyFrame::Error(serialize_error_code(error))
                }
            };
            self.spawn_frame_recording(state, None, frame);
        }
        state.terminal = Some(terminal);
        state.wake_all();
    }

    /// Spawns the oplog append of one frame. With `slot_data_len` set, a
    /// replayable slot is pushed for the frame; terminal frames pass `None`.
    fn spawn_frame_recording(
        &self,
        state: &mut DurableRequestBodyState,
        slot_data_len: Option<u64>,
        frame: SerializableP3HttpRequestBodyFrame,
    ) {
        let slot = slot_data_len.map(|data_len| {
            state.slots.push(RecordedFrameSlot {
                data_len,
                recording: FrameRecording::InFlight,
            });
            state.slots.len() - 1
        });
        state.pending_recordings += 1;
        let oplog = self.oplog.clone();
        let parent_start_index = self.parent_start_index;
        let shared = self.state.clone();
        tokio::task::spawn(async move {
            let result = record_frame_entry(oplog, parent_start_index, frame).await;
            let mut state = shared
                .lock()
                .expect("p3 durable request body mutex poisoned");
            state.pending_recordings -= 1;
            match result {
                Ok(index) => {
                    if let Some(slot) = slot {
                        state.slots[slot].recording = FrameRecording::Recorded(index);
                    }
                }
                Err(error) => {
                    if state.recording_failed.is_none() {
                        state.recording_failed = Some(error);
                    }
                }
            }
            state.wake_all();
        });
    }

    #[cfg(test)]
    fn has_pending_recordings(&self) -> bool {
        self.lock_state().pending_recordings > 0
    }

    fn lock_state(&self) -> std::sync::MutexGuard<'_, DurableRequestBodyState> {
        self.state
            .lock()
            .expect("p3 durable request body mutex poisoned")
    }
}

/// Serializes one request-body frame and appends its `HostStreamFrame` hint
/// entry, returning the assigned index. The payload reference is built without
/// an in-memory cache: reading it back goes through the oplog (inline bytes or
/// blob storage), which is what keeps resends bounded-memory.
pub(super) async fn record_frame_entry(
    oplog: Arc<dyn Oplog>,
    parent_start_index: OplogIndex,
    frame: SerializableP3HttpRequestBodyFrame,
) -> Result<OplogIndex, String> {
    let request = HostRequest::from(HostRequestP3HttpClientRequestBodyFrame { frame });
    let bytes = serialize(&request)?;
    let raw = oplog.upload_raw_payload(bytes).await?;
    let payload = raw.into_payload::<HostRequest>()?;
    Ok(oplog
        .add(OplogEntry::host_stream_frame(
            parent_start_index,
            HostStreamKind::P3HttpRequestBody,
            payload,
        ))
        .await)
}

/// Loads one recorded data/trailers frame back from its `HostStreamFrame`
/// entry. Uses `read_many`, which merges not-yet-committed buffered entries,
/// so a resend within the same session can replay frames that have not been
/// committed yet.
async fn load_recorded_frame(
    oplog: Arc<dyn Oplog>,
    index: OplogIndex,
) -> Result<Frame<Bytes>, ErrorCode> {
    let internal = |message: String| ErrorCode::InternalError(Some(message));
    let entry = oplog
        .read_many(index, 1)
        .await
        .remove(&index)
        .ok_or_else(|| internal(format!("recorded request-body frame missing at {index}")))?;
    let OplogEntry::HostStreamFrame { payload, .. } = entry else {
        return Err(internal(format!(
            "oplog entry at {index} is not a recorded request-body frame"
        )));
    };
    let request = oplog
        .download_payload::<HostRequest>(payload)
        .await
        .map_err(internal)?;
    let HostRequest::P3HttpClientRequestBodyFrame(frame) = request else {
        return Err(internal(format!(
            "oplog entry at {index} carries an unexpected payload type"
        )));
    };
    match frame.frame {
        SerializableP3HttpRequestBodyFrame::Data { bytes, .. } => {
            Ok(Frame::data(Bytes::from(bytes)))
        }
        SerializableP3HttpRequestBodyFrame::Trailers(trailers) => Ok(Frame::trailers(
            deserialize_headers(trailers.unwrap_or_default()),
        )),
        SerializableP3HttpRequestBodyFrame::End | SerializableP3HttpRequestBodyFrame::Error(_) => {
            Err(internal(format!(
                "recorded request-body frame at {index} is a terminal frame"
            )))
        }
    }
}

/// Summary of the `HostStreamFrame` request-body recording found in the oplog
/// for one send, merged by offset across crash/re-exec generations.
pub(super) struct RecordedRequestBodyScan {
    /// Contiguously covered byte length from offset 0. Data frames from
    /// different generations may duplicate or overlap; coverage stops at the
    /// first gap.
    pub(super) covered_len: u64,
    /// Every recorded data frame, sorted by `(offset, len)`. Only the oplog
    /// index of each frame is kept — never the bytes.
    pub(super) data_frames: Vec<RecordedDataFrame>,
    /// The entry of the first recorded trailers frame, if any. Generations may
    /// record the trailers more than once; the guest re-produces the same
    /// trailers deterministically, so any one of them is authoritative.
    pub(super) trailers_index: Option<OplogIndex>,
    /// The recorded terminal frame, if any: the recording is complete and
    /// needs no continuation. If generations disagree (which a deterministic
    /// guest never produces), the error terminal wins.
    pub(super) terminal: Option<RecordedRequestBodyTerminal>,
}

/// Location and extent of one recorded request-body data frame.
pub(super) struct RecordedDataFrame {
    pub(super) offset: u64,
    pub(super) len: u64,
    pub(super) index: OplogIndex,
}

/// The recorded terminal frame of a request-body recording.
pub(super) enum RecordedRequestBodyTerminal {
    End,
    Error(SerializableHttpErrorCode),
}

impl RecordedRequestBodyScan {
    pub(super) fn trailers_recorded(&self) -> bool {
        self.trailers_index.is_some()
    }

    pub(super) fn terminal_recorded(&self) -> bool {
        self.terminal.is_some()
    }
}

/// Scans the oplog for the request-body frames recorded for the send whose
/// `Start` entry is at `parent_start_index`, from right after that `Start` to
/// the current physical end of the oplog.
///
/// This is a direct scan, not a `ReplayState` lookup: frames are hint entries
/// that the replay cursor auto-skips, so by the time a replayed send result is
/// available the cursor is already past them. Frames recorded for *other*
/// sends — including abandoned `Start`s from crashed generations — are skipped
/// by the `parent_start_index` key. Each matching frame's payload is
/// downloaded one at a time and dropped right after inspection, so scanning an
/// arbitrarily large recorded body needs bounded memory (plus one
/// `(offset, len)` pair per data frame).
pub(super) async fn scan_recorded_request_body_frames(
    oplog: Arc<dyn Oplog>,
    parent_start_index: OplogIndex,
) -> Result<RecordedRequestBodyScan, String> {
    const SCAN_CHUNK: u64 = 1024;
    let scan_end = oplog.current_oplog_index().await;
    let mut data_frames: Vec<RecordedDataFrame> = Vec::new();
    let mut trailers_index: Option<OplogIndex> = None;
    let mut terminal: Option<RecordedRequestBodyTerminal> = None;
    let mut next = parent_start_index.next();
    while next <= scan_end {
        let entries = oplog.read_many(next, SCAN_CHUNK).await;
        if entries.is_empty() {
            break;
        }
        for (index, entry) in &entries {
            let OplogEntry::HostStreamFrame {
                parent_start_index: frame_parent,
                kind,
                payload,
                ..
            } = entry
            else {
                continue;
            };
            if *frame_parent != parent_start_index || *kind != HostStreamKind::P3HttpRequestBody {
                continue;
            }
            let request = oplog
                .download_payload::<HostRequest>(payload.clone())
                .await
                .map_err(|err| {
                    format!("failed to read recorded request-body frame at {index}: {err}")
                })?;
            let HostRequest::P3HttpClientRequestBodyFrame(frame) = request else {
                return Err(format!(
                    "oplog entry at {index} carries an unexpected payload type"
                ));
            };
            match frame.frame {
                SerializableP3HttpRequestBodyFrame::Data { offset, bytes } => {
                    data_frames.push(RecordedDataFrame {
                        offset,
                        len: bytes.len() as u64,
                        index: *index,
                    });
                }
                SerializableP3HttpRequestBodyFrame::Trailers(_) => {
                    trailers_index.get_or_insert(*index);
                }
                SerializableP3HttpRequestBodyFrame::End => {
                    if terminal.is_none() {
                        terminal = Some(RecordedRequestBodyTerminal::End);
                    }
                }
                SerializableP3HttpRequestBodyFrame::Error(error) => {
                    terminal = Some(RecordedRequestBodyTerminal::Error(error));
                }
            }
        }
        next = next.range_end(entries.len() as u64).next();
    }
    data_frames.sort_unstable_by_key(|frame| (frame.offset, frame.len));
    let mut covered_len = 0u64;
    for frame in &data_frames {
        if frame.offset > covered_len {
            break;
        }
        covered_len = covered_len.max(frame.offset.saturating_add(frame.len));
    }
    Ok(RecordedRequestBodyScan {
        covered_len,
        data_frames,
        trailers_index,
        terminal,
    })
}

/// A resend body streamed entirely from a completed oplog recording: replays
/// the contiguous data coverage (splitting overlapped frames from merged
/// crash/re-exec generations), then the recorded trailers, then ends. Frame
/// payloads are loaded from the oplog one at a time and dropped after
/// delivery, so resending an arbitrarily large recorded body needs bounded
/// memory. The caller must have verified that the recorded terminal is a clean
/// `End`.
pub(super) fn recorded_request_body_replay(
    oplog: Arc<dyn Oplog>,
    scan: &RecordedRequestBodyScan,
) -> UnsyncBoxBody<Bytes, ErrorCode> {
    struct ReplayState {
        oplog: Arc<dyn Oplog>,
        /// `(frame index, bytes to skip at its start)` per planned load.
        plan: std::collections::VecDeque<(OplogIndex, usize)>,
        trailers: Option<OplogIndex>,
    }

    let mut plan = std::collections::VecDeque::new();
    let mut pos = 0u64;
    for frame in &scan.data_frames {
        let end = frame.offset.saturating_add(frame.len);
        if end <= pos {
            continue;
        }
        if frame.offset > pos {
            break;
        }
        plan.push_back((frame.index, (pos - frame.offset) as usize));
        pos = end;
    }
    let state = ReplayState {
        oplog,
        plan,
        trailers: scan.trailers_index,
    };
    let stream = futures::stream::try_unfold(state, |mut state| async move {
        if let Some((index, skip)) = state.plan.pop_front() {
            let frame = load_recorded_frame(state.oplog.clone(), index).await?;
            let data = frame.into_data().map_err(|_| {
                ErrorCode::InternalError(Some(format!(
                    "recorded request-body frame at {index} is no longer a data frame"
                )))
            })?;
            return Ok(Some((Frame::data(data.slice(skip..)), state)));
        }
        if let Some(index) = state.trailers.take() {
            let frame = load_recorded_frame(state.oplog.clone(), index).await?;
            return Ok(Some((frame, state)));
        }
        Ok(None)
    });
    http_body_util::StreamBody::new(stream).boxed_unsync()
}

/// One attempt's request body handed to the HTTP client: replays the recorded
/// frame prefix from the oplog, then continues with the live guest body.
pub(super) struct DurableRequestBodyView {
    shared: DurableRequestBody,
    pos: usize,
    live_claimed: bool,
    pending_load: Option<Pin<Box<dyn Future<Output = Result<Frame<Bytes>, ErrorCode>> + Send>>>,
}

impl Drop for DurableRequestBodyView {
    fn drop(&mut self) {
        if self.live_claimed {
            let mut state = self.shared.lock_state();
            state.active_live_view = false;
            state.wake_all();
        }
    }
}

impl HttpBody for DurableRequestBodyView {
    type Data = Bytes;
    type Error = ErrorCode;

    fn poll_frame(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        let this = self.get_mut();
        loop {
            if let Some(load) = this.pending_load.as_mut() {
                let frame = std::task::ready!(load.as_mut().poll(cx));
                this.pending_load = None;
                this.pos += 1;
                return Poll::Ready(Some(frame));
            }
            let shared = this.shared.clone();
            let mut state = shared.lock_state();
            if let Some(message) = &state.recording_failed {
                return Poll::Ready(Some(Err(ErrorCode::InternalError(Some(message.clone())))));
            }
            if this.pos < state.slots.len() {
                match &state.slots[this.pos].recording {
                    FrameRecording::Recorded(index) => {
                        this.pending_load =
                            Some(Box::pin(load_recorded_frame(shared.oplog.clone(), *index)));
                        drop(state);
                        continue;
                    }
                    FrameRecording::InFlight => {
                        state.park(cx);
                        return Poll::Pending;
                    }
                }
            }
            match &state.terminal {
                Some(RequestBodyTerminal::Error(error)) => {
                    return Poll::Ready(Some(Err(error.clone())));
                }
                Some(RequestBodyTerminal::End) => return Poll::Ready(None),
                None => {}
            }
            if !this.live_claimed {
                assert!(
                    !state.active_live_view,
                    "only one p3 durable request body live view may be active"
                );
                state.active_live_view = true;
                this.live_claimed = true;
            }
            if shared.recording_enabled && state.pending_recordings >= REQUEST_BODY_RECORDING_WINDOW
            {
                state.park(cx);
                return Poll::Pending;
            }
            match state.inner.as_mut().poll_frame(cx) {
                Poll::Ready(Some(Ok(frame))) => {
                    state.live_polled = true;
                    match shared.accept_live_frame(&mut state, frame) {
                        Some(frame) => {
                            this.pos = state.slots.len();
                            return Poll::Ready(Some(Ok(frame)));
                        }
                        None => {
                            drop(state);
                            continue;
                        }
                    }
                }
                Poll::Ready(Some(Err(error))) => {
                    state.live_polled = true;
                    shared.record_terminal(&mut state, RequestBodyTerminal::Error(error.clone()));
                    return Poll::Ready(Some(Err(error)));
                }
                Poll::Ready(None) => {
                    state.live_polled = true;
                    shared.record_terminal(&mut state, RequestBodyTerminal::End);
                    return Poll::Ready(None);
                }
                Poll::Pending => return Poll::Pending,
            }
        }
    }

    fn is_end_stream(&self) -> bool {
        if self.pending_load.is_some() {
            return false;
        }
        let state = self.shared.lock_state();
        self.pos >= state.slots.len() && matches!(state.terminal, Some(RequestBodyTerminal::End))
    }

    fn size_hint(&self) -> SizeHint {
        let state = self.shared.lock_state();
        let start = self.pos.min(state.slots.len());
        let prefix_len: u64 = state.slots[start..].iter().map(|slot| slot.data_len).sum();
        let inner = state.inner.size_hint();
        let mut hint = SizeHint::new();
        hint.set_lower(prefix_len.saturating_add(inner.lower()));
        if let Some(upper) = inner.upper() {
            hint.set_upper(prefix_len.saturating_add(upper));
        }
        hint
    }
}

pub(super) struct BodyWithState<B, T> {
    pub(super) body: B,
    pub(super) _state: T,
}

impl<B, T> HttpBody for BodyWithState<B, T>
where
    B: HttpBody + Unpin,
    T: Unpin,
{
    type Data = B::Data;
    type Error = B::Error;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        Pin::new(&mut self.body).poll_frame(cx)
    }

    fn is_end_stream(&self) -> bool {
        self.body.is_end_stream()
    }

    fn size_hint(&self) -> SizeHint {
        self.body.size_hint()
    }
}

/// Resolution delivered to the guest-facing request-body transmission future
/// (the `FutureReader<Result<(), ErrorCode>>` returned by the durable
/// `request::new`) once the transmission outcome is known.
pub(crate) enum HttpTransmissionResolution {
    /// The transmission terminal: recorded (live), replayed, or — for a request
    /// consumed without a `client::send` — the deterministic passthrough value.
    Outcome(Result<(), ErrorCode>),
    /// A durability failure: the transmission future traps with this message,
    /// tagged with the failing call scope's trap context.
    Trap {
        message: String,
        trap_context: DurableCallTrapContext,
    },
}

/// Durable wiring of a p3 outgoing request's body transmission future,
/// interposed by the durable `request::new` between the built-in
/// `WasiHttp` future and the guest.
///
/// `raw_rx` carries the *raw* transmission result produced by the built-in
/// machinery (the request-body I/O result of a live send, a deterministic
/// body-validation error, the guest-supplied `consume-body` future's value, or
/// `Ok(())` when the wiring is dropped — mirroring the built-in
/// sender-dropped-means-success rule). `resolution_tx` resolves the guest-held
/// future. `demand_rx` fires when the guest actually polls the transmission
/// future ([`HttpTransmissionFutureProducer`] sends it on the first real
/// read); the durable recording is gated on it so a guest that never observes
/// the transmission result writes no oplog entries — see
/// [`HttpRequestBodyTransmissionTask`].
///
/// Registered in `pending_p3_http_request_transmissions` keyed by the request
/// resource rep, and detached by the host call that consumes the request:
/// `client::send` records/replays the result durably
/// ([`HttpRequestBodyTransmissionTask`]), while a guest-side
/// `consume-body`/`drop` forwards the deterministic raw value with no
/// recording ([`HttpRequestTransmissionPassthroughTask`]).
pub(crate) struct PendingHttpRequestBodyTransmission {
    raw_rx: oneshot::Receiver<Result<(), ErrorCode>>,
    resolution_tx: oneshot::Sender<HttpTransmissionResolution>,
    demand_rx: oneshot::Receiver<()>,
}

/// Host-side consumer forwarding the built-in transmission future's value into
/// the plain `raw_rx` channel of [`PendingHttpRequestBodyTransmission`]
/// (mirroring the built-in `BodyResultConsumer`), so the durable tasks can
/// await it without store access.
pub(super) struct HttpTransmissionResultForwarder(Option<oneshot::Sender<Result<(), ErrorCode>>>);

impl<U> FutureConsumer<U> for HttpTransmissionResultForwarder
where
    U: 'static,
{
    type Item = Result<(), ErrorCode>;

    fn poll_consume(
        mut self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        store: StoreContextMut<U>,
        mut src: Source<'_, Self::Item>,
        _finish: bool,
    ) -> Poll<wasmtime::Result<()>> {
        let mut result = None;
        src.read(store, &mut result)?;
        let result = result
            .ok_or_else(|| wasmtime::Error::msg("transmission result value missing from source"))?;
        let tx = self.0.take().ok_or_else(|| {
            wasmtime::Error::msg("transmission result forwarder polled after completion")
        })?;
        let _ = tx.send(result);
        Poll::Ready(Ok(()))
    }
}

/// Guest-facing request-body transmission `FutureReader` producer, mirroring
/// [`HttpTrailersFutureProducer`]: awaits the resolution from the durable (or
/// passthrough) task and delivers it to the guest.
///
/// The first *real* read (a poll that is not an immediate cancellation) sends
/// a one-shot demand: the durable recording task gates its oplog entries on
/// it, so entries exist iff the guest observed the transmission result — a
/// deterministic function of guest behaviour, identical live and on replay.
pub(super) struct HttpTransmissionFutureProducer {
    rx: oneshot::Receiver<HttpTransmissionResolution>,
    demand_tx: Option<oneshot::Sender<()>>,
}

impl<U> FutureProducer<U> for HttpTransmissionFutureProducer
where
    U: 'static,
{
    type Item = Result<(), ErrorCode>;

    fn poll_produce(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        _store: StoreContextMut<U>,
        finish: bool,
    ) -> Poll<wasmtime::Result<Option<Self::Item>>> {
        let this = self.get_mut();
        if !finish && let Some(demand_tx) = this.demand_tx.take() {
            // The send fails silently when the owning task is a
            // non-recording passthrough (which drops `demand_rx`) or is gone.
            let _ = demand_tx.send(());
        }
        match Pin::new(&mut this.rx).poll(cx) {
            Poll::Pending if finish => Poll::Ready(Ok(None)),
            Poll::Pending => Poll::Pending,
            Poll::Ready(Ok(HttpTransmissionResolution::Outcome(result))) => {
                Poll::Ready(Ok(Some(result)))
            }
            // A durability failure occurred before the terminal was recorded: the
            // transmission future must trap (carrying the failing call scope's
            // trap context) rather than resolve to a normal error value that
            // would mask it.
            Poll::Ready(Ok(HttpTransmissionResolution::Trap {
                message,
                trap_context,
            })) => Poll::Ready(Err(wasmtime::Error::from_anyhow(
                mark_durable_call_trap_context(anyhow::Error::msg(message), trap_context),
            ))),
            // The channel closed without a resolution: the owning task was
            // dropped before sending. The normal paths always send a resolution
            // first, so this is a durability failure and must trap.
            Poll::Ready(Err(_)) => Poll::Ready(Err(wasmtime::Error::msg(
                "request-body transmission task dropped before resolving",
            ))),
        }
    }
}

pub(super) fn serialize_transmission_result(
    result: &Result<(), ErrorCode>,
) -> Result<(), SerializableHttpErrorCode> {
    result.as_ref().map(|_| ()).map_err(serialize_error_code)
}

pub(super) fn deserialize_transmission_result(
    result: Result<(), SerializableHttpErrorCode>,
) -> Result<(), ErrorCode> {
    result.map_err(deserialize_error_code)
}

/// Starts the (demand-gated) durable recording of a sent request's body
/// transmission result (G8): spawns [`HttpRequestBodyTransmissionTask`] for
/// the request's transmission wiring. The spawn happens at this deterministic
/// point — right after the send terminal and its span entries — so that when
/// the guest demands the result, the task's `Start` append/claim lands at a
/// stable position relative to the send's own entries. `None` means the
/// request carried no durable transmission wiring (not created via the
/// durable `request::new`); nothing is recorded then, identically on both
/// paths.
pub(super) fn start_transmission_recording<Ctx, U>(
    store: &Accessor<U, DurableP3<Ctx>>,
    pending: Option<PendingHttpRequestBodyTransmission>,
) where
    Ctx: WorkerCtx,
    U: Send + 'static,
{
    if let Some(pending) = pending {
        store.spawn(HttpRequestBodyTransmissionTask::<Ctx>::new(pending));
    }
}

/// Fail the durable transmission task loudly on a durability-machinery error,
/// mirroring [`fail_consume_body_task`]: the guest-facing transmission future
/// is resolved with a [`HttpTransmissionResolution::Trap`] carrying the failing
/// call scope's trap context, and the task itself returns `Err` (surfaced as a
/// trap by the runtime).
pub(super) fn fail_transmission_task(
    resolution_tx: oneshot::Sender<HttpTransmissionResolution>,
    error: wasmtime::Error,
    trap_context: DurableCallTrapContext,
) -> wasmtime::Result<()> {
    let _ = resolution_tx.send(HttpTransmissionResolution::Trap {
        message: "request-body transmission durable persistence failed".to_string(),
        trap_context,
    });
    Err(error)
}

/// Durable recorder for a sent request's body transmission result.
///
/// The recording is **demand-gated**: the task first parks on `demand_rx` (a
/// plain oneshot fired by the guest's first real poll of the transmission
/// future) and touches no durable machinery until then. Oplog entries
/// therefore exist iff the guest observed the transmission result — a
/// deterministic function of guest behaviour, identical live and on replay
/// (a demanding guest re-demands during replay, so the recorded `Start` is
/// always claimed). This gating is what keeps a fire-and-forget send safe:
/// `run_concurrent` does not drain spawned tasks (G25/T28), and a task left
/// parked on replay-cursor machinery when the invocation's event loop exits
/// would strand the fair cursor lock and deadlock the worker. Parked on the
/// plain demand channel, an undemanded task is inert.
///
/// On demand — live: awaits the raw transmission result (the send's
/// request-body I/O outcome, fed through [`HttpTransmissionResultForwarder`];
/// a closed channel means the wiring was dropped, which the built-in
/// machinery treats as success), records it as the `body-transmission`
/// `Start`/`End`, and resolves the guest future with the recorded value.
/// Replay: claims the recorded `Start` and resolves the guest future from the
/// recorded `End`; an incomplete `Start` re-executes against the raw channel,
/// which on replay carries [`consume_replayed_request`]'s drain-derived
/// result (the documented best-effort fallback for a run that crashed before
/// observing the real outcome).
///
/// A guest awaiting the demanded resolution keeps the invocation (and thus
/// the store's event loop) alive until the `End` is recorded, so a demanded
/// transmission's entries land before `AgentInvocationFinished`. A guest that
/// demands, *cancels* the read, and immediately finishes the invocation can
/// still leave this task parked on the replay cursor past `run_concurrent`
/// exit — that residual exposure is shared with the other spawned durable
/// tasks and is resolved by the T28 invocation-end drain.
pub(super) struct HttpRequestBodyTransmissionTask<Ctx> {
    raw_rx: oneshot::Receiver<Result<(), ErrorCode>>,
    resolution_tx: oneshot::Sender<HttpTransmissionResolution>,
    demand_rx: oneshot::Receiver<()>,
    _phantom: PhantomData<fn() -> Ctx>,
}

impl<Ctx> HttpRequestBodyTransmissionTask<Ctx> {
    fn new(pending: PendingHttpRequestBodyTransmission) -> Self {
        Self {
            raw_rx: pending.raw_rx,
            resolution_tx: pending.resolution_tx,
            demand_rx: pending.demand_rx,
            _phantom: PhantomData,
        }
    }
}

impl<Ctx, U> AccessorTask<U, DurableP3<Ctx>> for HttpRequestBodyTransmissionTask<Ctx>
where
    Ctx: WorkerCtx,
    U: 'static,
{
    async fn run(self, accessor: &Accessor<U, DurableP3<Ctx>>) -> wasmtime::Result<()> {
        let Self {
            raw_rx,
            resolution_tx,
            demand_rx,
            ..
        } = self;

        // Gate all durable work on the guest's demand. A closed channel means
        // the guest dropped the transmission future without ever reading it:
        // nothing to record, nothing to resolve.
        if demand_rx.await.is_err() {
            return Ok(());
        }

        // `ReadRemote` + `LeaveIncompleteOnDrop`: the call only *observes* the
        // upload outcome (the send itself is the write), so an incomplete
        // `Start` (crash before the upload result was recorded) safely
        // re-executes on replay instead of failing the worker.
        let mut handle = match CallHandle::<
            P3HttpClientRequestBodyTransmission,
            LeaveIncompleteOnDrop,
        >::start_access(
            accessor,
            durable_worker_ctx::<Ctx, U>,
            HostRequestNoInput {},
            DurableFunctionType::ReadRemote,
        )
        .await
        {
            Ok(handle) => handle,
            // No handle exists yet, so there is no call scope to tag the trap
            // with; drop the resolution sender so a still-polling guest traps
            // loudly on the closed channel.
            Err(error) => {
                drop(resolution_tx);
                return Err(wasmtime::Error::from(error));
            }
        };

        if !handle.is_live() {
            let trap_context = handle.trap_context();
            match handle
                .replay_access(accessor, durable_worker_ctx::<Ctx, U>)
                .await
            {
                Ok(CallReplayOutcome::Replayed(response)) => {
                    let _ = resolution_tx.send(HttpTransmissionResolution::Outcome(
                        deserialize_transmission_result(response.result),
                    ));
                    return Ok(());
                }
                Ok(CallReplayOutcome::Incomplete(live_handle)) => {
                    handle = live_handle;
                }
                Err(error) => {
                    return fail_transmission_task(
                        resolution_tx,
                        wasmtime::Error::from_anyhow(mark_durable_call_trap_context(
                            anyhow::Error::from(error),
                            trap_context,
                        )),
                        trap_context,
                    );
                }
            }
        }

        // Live (or incomplete-replay re-execution): await the raw result and
        // record it before resolving the guest future, so the guest never
        // observes a transmission outcome that is not yet durable.
        let raw = raw_rx.await.unwrap_or(Ok(()));
        let trap_context = handle.trap_context();
        match handle
            .complete_access(
                accessor,
                durable_worker_ctx::<Ctx, U>,
                HostResponseP3HttpClientRequestBodyTransmission {
                    result: serialize_transmission_result(&raw),
                },
            )
            .await
        {
            Ok(response) => {
                let _ = resolution_tx.send(HttpTransmissionResolution::Outcome(
                    deserialize_transmission_result(response.result),
                ));
                Ok(())
            }
            Err(error) => {
                fail_transmission_task(resolution_tx, wasmtime::Error::from(error), trap_context)
            }
        }
    }
}

/// Forwards the deterministic raw transmission result to the guest future for
/// a request consumed *without* a `client::send`: a guest-side `consume-body`
/// (the value comes from the guest-supplied future) or a request `drop`
/// (`Ok(())`, matching the built-in sender-dropped rule). No durable entries
/// are written — the value is a pure function of replayed guest behaviour.
pub(super) struct HttpRequestTransmissionPassthroughTask<Ctx> {
    raw_rx: oneshot::Receiver<Result<(), ErrorCode>>,
    resolution_tx: oneshot::Sender<HttpTransmissionResolution>,
    _phantom: PhantomData<fn() -> Ctx>,
}

impl<Ctx> HttpRequestTransmissionPassthroughTask<Ctx> {
    fn new(pending: PendingHttpRequestBodyTransmission) -> Self {
        Self {
            raw_rx: pending.raw_rx,
            resolution_tx: pending.resolution_tx,
            _phantom: PhantomData,
        }
    }
}

impl<Ctx, U> AccessorTask<U, DurableP3<Ctx>> for HttpRequestTransmissionPassthroughTask<Ctx>
where
    Ctx: WorkerCtx,
    U: 'static,
{
    async fn run(self, _accessor: &Accessor<U, DurableP3<Ctx>>) -> wasmtime::Result<()> {
        let result = self.raw_rx.await.unwrap_or(Ok(()));
        let _ = self
            .resolution_tx
            .send(HttpTransmissionResolution::Outcome(result));
        Ok(())
    }
}

/// Detaches the transmission wiring of a request consumed without a
/// `client::send` (guest-side `consume-body` or `drop`) and spawns the
/// non-durable passthrough forwarder for it. Must run in the same host call
/// that deletes the request resource, before the delete, so a reused rep can
/// never alias a stale entry.
pub(super) fn detach_request_transmission_passthrough<Ctx: WorkerCtx, U: Send + 'static>(
    store: &mut Access<U, DurableP3<Ctx>>,
    request_rep: u32,
) {
    let pending = {
        let mut store_ctx = store.as_context_mut();
        let ctx = durable_worker_ctx::<Ctx, U>(store_ctx.data_mut());
        ctx.state
            .pending_p3_http_request_transmissions
            .remove(&request_rep)
    };
    if let Some(pending) = pending {
        store.spawn(HttpRequestTransmissionPassthroughTask::<Ctx>::new(pending));
    }
}

/// Renders the request URI of a serialized outgoing p3 HTTP request the same
/// way the P2 `http::outgoing_handler::handle` path does, for span attributes
/// and retry properties.
impl<U: Send + 'static, Ctx: WorkerCtx> types::HostRequestWithStore<U> for DurableP3<Ctx> {
    fn new(
        mut store: Access<U, Self>,
        headers: Resource<Headers>,
        contents: Option<StreamReader<u8>>,
        trailers: FutureReader<Result<Option<Resource<Trailers>>, ErrorCode>>,
        options: Option<Resource<RequestOptions>>,
    ) -> wasmtime::Result<(Resource<Request>, FutureReader<Result<(), ErrorCode>>)> {
        observe_function_call_store::<Ctx, U>(
            store.as_context_mut().data_mut(),
            "http::types::request",
            "new",
        );
        let (req, inner_transmission) = {
            let http_store =
                Access::<U, WasiHttp>::new(store.as_context_mut(), wasi_http_view::<Ctx, U>);
            <WasiHttp as types::HostRequestWithStore<U>>::new(
                http_store, headers, contents, trailers, options,
            )?
        };

        // Interpose on the request-body transmission future: the guest gets our
        // own `FutureReader` and the built-in one is piped into a plain channel.
        // The host call that later consumes the request decides how the guest
        // future resolves: `client::send` records/replays the (otherwise
        // non-deterministic) result durably, while a guest-side
        // `consume-body`/`drop` forwards the deterministic raw value as-is.
        let (raw_tx, raw_rx) = oneshot::channel();
        inner_transmission.pipe(
            store.as_context_mut(),
            HttpTransmissionResultForwarder(Some(raw_tx)),
        )?;
        let (resolution_tx, resolution_rx) = oneshot::channel();
        let (demand_tx, demand_rx) = oneshot::channel();
        let transmission = FutureReader::new(
            &mut store,
            HttpTransmissionFutureProducer {
                rx: resolution_rx,
                demand_tx: Some(demand_tx),
            },
        )?;

        // Register the wiring only after every fallible construction succeeded
        // (a failure above traps and tears the store down, so no partial state
        // survives).
        {
            let mut store_ctx = store.as_context_mut();
            let ctx = durable_worker_ctx::<Ctx, U>(store_ctx.data_mut());
            ctx.state.pending_p3_http_request_transmissions.insert(
                req.rep(),
                PendingHttpRequestBodyTransmission {
                    raw_rx,
                    resolution_tx,
                    demand_rx,
                },
            );
        }
        Ok((req, transmission))
    }

    fn consume_body(
        mut store: Access<U, Self>,
        req: Resource<Request>,
        fut: FutureReader<Result<(), ErrorCode>>,
    ) -> wasmtime::Result<(
        StreamReader<u8>,
        FutureReader<Result<Option<Resource<Trailers>>, ErrorCode>>,
    )> {
        observe_function_call_store::<Ctx, U>(
            store.as_context_mut().data_mut(),
            "http::types::request",
            "consume-body",
        );
        detach_request_transmission_passthrough::<Ctx, U>(&mut store, req.rep());
        let store = Access::<U, WasiHttp>::new(store.as_context_mut(), wasi_http_view::<Ctx, U>);
        <WasiHttp as types::HostRequestWithStore<U>>::consume_body(store, req, fut)
    }

    fn drop(mut store: Access<U, Self>, req: Resource<Request>) -> wasmtime::Result<()> {
        observe_function_call_store::<Ctx, U>(
            store.as_context_mut().data_mut(),
            "http::types::request",
            "drop",
        );
        detach_request_transmission_passthrough::<Ctx, U>(&mut store, req.rep());
        let store = Access::<U, WasiHttp>::new(store.as_context_mut(), wasi_http_view::<Ctx, U>);
        <WasiHttp as types::HostRequestWithStore<U>>::drop(store, req)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::durable_host::durability::DurableCallTrapContext;
    use crate::durable_host::p3::http::test_support::*;
    use golem_common::model::oplog::OplogIndex;
    use http::HeaderMap;
    use std::time::Duration;
    use test_r::{test, timeout};
    use wasmtime::{AsContextMut, Engine, Store};

    async fn wait_for_recordings(body: &DurableRequestBody) {
        while body.has_pending_recordings() {
            tokio::task::yield_now().await;
        }
    }

    /// Splits recorded frames into (data frames sorted by offset, trailers
    /// count, end count, error count). Frame recordings are appended by
    /// concurrent tasks, so their oplog order is not the pull order; the
    /// offset-keyed frame shape exists precisely so ordering does not matter.
    fn partition_frames(
        frames: Vec<SerializableP3HttpRequestBodyFrame>,
    ) -> (Vec<(u64, Vec<u8>)>, usize, usize, usize) {
        let mut data = Vec::new();
        let mut trailers = 0;
        let mut ends = 0;
        let mut errors = 0;
        for frame in frames {
            match frame {
                SerializableP3HttpRequestBodyFrame::Data { offset, bytes } => {
                    data.push((offset, bytes))
                }
                SerializableP3HttpRequestBodyFrame::Trailers(_) => trailers += 1,
                SerializableP3HttpRequestBodyFrame::End => ends += 1,
                SerializableP3HttpRequestBodyFrame::Error(_) => errors += 1,
            }
        }
        data.sort_by_key(|(offset, _)| *offset);
        (data, trailers, ends, errors)
    }

    /// Collects a view's frames into (data bytes, trailers), asserting a clean
    /// end of stream.
    async fn collect_view(mut view: DurableRequestBodyView) -> (Vec<u8>, Option<HeaderMap>) {
        let mut data = Vec::new();
        let mut trailers = None;
        while let Some(frame) = view.frame().await {
            let frame = frame.expect("view must not fail");
            match frame.into_data().map_err(Frame::into_trailers) {
                Ok(bytes) => data.extend_from_slice(&bytes),
                Err(Ok(t)) => trailers = Some(t),
                Err(Err(_)) => panic!("unexpected frame kind"),
            }
        }
        (data, trailers)
    }

    /// A live pull must record every data/trailers frame plus the `End`
    /// terminal as offset-keyed `HostStreamFrame` entries, and a subsequent
    /// view must replay the identical body purely from the oplog.
    #[test]
    #[timeout("10s")]
    async fn durable_request_body_records_and_replays_from_oplog() {
        let oplog = FrameTestOplog::new();
        let mut trailer_map = HeaderMap::new();
        trailer_map.insert("x-checksum", "abc123".parse().unwrap());
        let body = frame_body(
            vec![
                Frame::data(Bytes::from_static(b"hello")),
                Frame::data(Bytes::from_static(b" world")),
                Frame::trailers(trailer_map.clone()),
            ],
            None,
        );
        let durable = DurableRequestBody::new(body, oplog.clone(), OplogIndex::INITIAL, true);

        let (data, trailers) = collect_view(durable.replayer()).await;
        assert_eq!(data, b"hello world");
        assert_eq!(trailers, Some(trailer_map.clone()));
        wait_for_recordings(&durable).await;

        let frames = oplog.recorded_frames_for(OplogIndex::INITIAL);
        assert_eq!(frames.len(), 4);
        let (data_frames, trailer_count, end_count, error_count) = partition_frames(frames);
        assert_eq!(
            data_frames,
            vec![(0, b"hello".to_vec()), (5, b" world".to_vec())]
        );
        assert_eq!(trailer_count, 1);
        assert_eq!(end_count, 1);
        assert_eq!(error_count, 0);

        assert!(durable.can_replay_after_send_failure());
        let (data, trailers) = collect_view(durable.replayer()).await;
        assert_eq!(data, b"hello world");
        assert_eq!(trailers, Some(trailer_map));
    }

    /// A bodiless send must still record the `End` terminal frame, so that
    /// "recording present" is unambiguous, and must stay replayable.
    #[test]
    #[timeout("10s")]
    async fn durable_request_body_records_end_terminal_for_bodiless_send() {
        let oplog = FrameTestOplog::new();
        let body = frame_body(vec![], None);
        let durable = DurableRequestBody::new(body, oplog.clone(), OplogIndex::INITIAL, true);

        assert!(matches!(
            durable.drain_to_terminal().await,
            DurableRequestBodyDrainOutcome::Replayable
        ));
        wait_for_recordings(&durable).await;

        let frames = oplog.recorded_frames_for(OplogIndex::INITIAL);
        assert_eq!(frames.len(), 1);
        assert!(matches!(
            &frames[0],
            SerializableP3HttpRequestBodyFrame::End
        ));
        assert!(durable.can_replay_after_send_failure());

        let (data, trailers) = collect_view(durable.replayer()).await;
        assert!(data.is_empty());
        assert!(trailers.is_none());
    }

    /// `frames_consumed` distinguishes the request-body-write phase: false for
    /// a fresh body and for a bodiless send (terminal only), true once a data
    /// frame was pulled from the live guest body.
    #[test]
    #[timeout("10s")]
    async fn durable_request_body_frames_consumed_tracks_body_write_phase() {
        let oplog = FrameTestOplog::new();
        let body = frame_body(
            vec![
                Frame::data(Bytes::from_static(b"first")),
                Frame::data(Bytes::from_static(b"second")),
            ],
            None,
        );
        let durable = DurableRequestBody::new(body, oplog.clone(), OplogIndex::INITIAL, true);
        assert!(!durable.frames_consumed());

        let mut view = durable.replayer();
        let first = view.frame().await.unwrap().unwrap();
        assert_eq!(first.into_data().unwrap(), Bytes::from_static(b"first"));
        assert!(durable.frames_consumed());
        drop(view);

        let bodiless_oplog = FrameTestOplog::new();
        let bodiless = DurableRequestBody::new(
            frame_body(Vec::new(), None),
            bodiless_oplog,
            OplogIndex::INITIAL,
            true,
        );
        assert!(matches!(
            bodiless.drain_to_terminal().await,
            DurableRequestBodyDrainOutcome::Replayable
        ));
        assert!(!bodiless.frames_consumed());
    }

    /// The demand-to-drain transition: after an attempt's view pulled a prefix
    /// of the body, `drain_to_terminal` must pull and record the rest and
    /// report the body replayable.
    #[test]
    #[timeout("10s")]
    async fn durable_request_body_drains_rest_after_partial_view_pull() {
        let oplog = FrameTestOplog::new();
        let body = frame_body(
            vec![
                Frame::data(Bytes::from_static(b"first")),
                Frame::data(Bytes::from_static(b"second")),
            ],
            None,
        );
        let durable = DurableRequestBody::new(body, oplog.clone(), OplogIndex::INITIAL, true);

        let mut view = durable.replayer();
        let first = view.frame().await.unwrap().unwrap();
        assert_eq!(first.into_data().unwrap(), Bytes::from_static(b"first"));
        drop(view);

        assert!(matches!(
            durable.drain_to_terminal().await,
            DurableRequestBodyDrainOutcome::Replayable
        ));
        wait_for_recordings(&durable).await;

        let frames = oplog.recorded_frames_for(OplogIndex::INITIAL);
        assert_eq!(frames.len(), 3);
        let (data_frames, trailer_count, end_count, error_count) = partition_frames(frames);
        assert_eq!(
            data_frames,
            vec![(0, b"first".to_vec()), (5, b"second".to_vec())]
        );
        assert_eq!(trailer_count, 0);
        assert_eq!(end_count, 1);
        assert_eq!(error_count, 0);

        let (data, _) = collect_view(durable.replayer()).await;
        assert_eq!(data, b"firstsecond");
    }

    /// The bounded in-flight window: with recordings stuck (oplog uploads
    /// gated), pulling stalls after `REQUEST_BODY_RECORDING_WINDOW` frames and
    /// resumes as recordings land.
    #[test]
    #[timeout("10s")]
    async fn durable_request_body_bounds_in_flight_recordings() {
        let oplog = FrameTestOplog::gated();
        let total_frames = REQUEST_BODY_RECORDING_WINDOW + 2;
        let body = frame_body(
            (0..total_frames)
                .map(|i| Frame::data(Bytes::from(vec![i as u8; 3])))
                .collect(),
            None,
        );
        let durable = DurableRequestBody::new(body, oplog.clone(), OplogIndex::INITIAL, true);

        let mut view = durable.replayer();
        for _ in 0..REQUEST_BODY_RECORDING_WINDOW {
            view.frame().await.unwrap().unwrap();
        }
        assert!(
            tokio::time::timeout(Duration::from_millis(50), view.frame())
                .await
                .is_err(),
            "pulling past the recording window must stall while recordings are in flight"
        );

        oplog.release_uploads(1);
        view.frame()
            .await
            .expect("a landed recording must unblock the next pull")
            .unwrap();

        oplog.release_uploads(total_frames * 2);
        while view.frame().await.is_some() {}
        wait_for_recordings(&durable).await;

        let frames = oplog.recorded_frames_for(OplogIndex::INITIAL);
        assert_eq!(frames.len(), total_frames + 1);
        let (data_frames, _, end_count, error_count) = partition_frames(frames);
        assert_eq!(data_frames.len(), total_frames);
        assert_eq!(end_count, 1);
        assert_eq!(error_count, 0);
    }

    /// A guest body error must be recorded as the `Error` terminal frame and
    /// must refuse both the drain outcome and a resend.
    #[test]
    #[timeout("10s")]
    async fn durable_request_body_error_terminal_refuses_resend() {
        let oplog = FrameTestOplog::new();
        let body = frame_body(
            vec![Frame::data(Bytes::from_static(b"partial"))],
            Some(ErrorCode::InternalError(Some(
                "guest body failed".to_string(),
            ))),
        );
        let durable = DurableRequestBody::new(body, oplog.clone(), OplogIndex::INITIAL, true);

        let mut view = durable.replayer();
        view.frame().await.unwrap().unwrap();
        let error = view.frame().await.unwrap().unwrap_err();
        assert!(matches!(error, ErrorCode::InternalError(_)));
        drop(view);

        assert!(!durable.can_replay_after_send_failure());
        assert!(matches!(
            durable.drain_to_terminal().await,
            DurableRequestBodyDrainOutcome::NotReplayable
        ));
        wait_for_recordings(&durable).await;

        let frames = oplog.recorded_frames_for(OplogIndex::INITIAL);
        assert_eq!(frames.len(), 2);
        let (data_frames, _, end_count, error_count) = partition_frames(frames);
        assert_eq!(data_frames, vec![(0, b"partial".to_vec())]);
        assert_eq!(end_count, 0);
        assert_eq!(error_count, 1);
    }

    /// With recording disabled (`PersistNothing` / snapshotting), the body
    /// must still stream but write no oplog entries, and a pulled non-empty
    /// body must not claim to be replayable.
    #[test]
    #[timeout("10s")]
    async fn durable_request_body_skips_recording_when_disabled() {
        let oplog = FrameTestOplog::new();
        let body = frame_body(vec![Frame::data(Bytes::from_static(b"data"))], None);
        let durable = DurableRequestBody::new(body, oplog.clone(), OplogIndex::INITIAL, false);

        assert!(durable.can_replay_after_send_failure());
        let (data, _) = collect_view(durable.replayer()).await;
        assert_eq!(data, b"data");

        assert!(oplog.recorded_frames_for(OplogIndex::INITIAL).is_empty());
        assert!(!durable.can_replay_after_send_failure());
        assert!(matches!(
            durable.drain_to_terminal().await,
            DurableRequestBodyDrainOutcome::NotReplayable
        ));
    }

    /// With recording disabled, an unpulled (bodiless) body stays replayable:
    /// resending it sends the same empty body.
    #[test]
    #[timeout("10s")]
    async fn durable_request_body_disabled_recording_keeps_empty_body_replayable() {
        let oplog = FrameTestOplog::new();
        let body = frame_body(vec![], None);
        let durable = DurableRequestBody::new(body, oplog.clone(), OplogIndex::INITIAL, false);

        assert!(matches!(
            durable.drain_to_terminal().await,
            DurableRequestBodyDrainOutcome::Replayable
        ));
        assert!(durable.can_replay_after_send_failure());
        assert!(oplog.recorded_frames_for(OplogIndex::INITIAL).is_empty());
    }

    /// The recorded `body-transmission` terminal must replay unchanged: the
    /// serializable transmission result round-trips through the live p3
    /// `ErrorCode` for every error variant (and the success case).
    #[test]
    fn transmission_result_conversion_roundtrips() {
        let mut cases: Vec<Result<(), SerializableHttpErrorCode>> = vec![Ok(())];
        cases.extend(all_serializable_error_codes().into_iter().map(Err));
        for case in cases {
            let roundtripped =
                serialize_transmission_result(&deserialize_transmission_result(case.clone()));
            assert_eq!(roundtripped, case);
        }
    }

    /// The guest-facing transmission future must deliver the resolved outcome
    /// (here: the recorded/replayed transmission error) and stay pending until
    /// the owning task resolves it. Its first real poll must fire the one-shot
    /// demand that gates the durable recording, exactly once.
    #[test]
    fn transmission_future_producer_delivers_outcome_and_demands_once() {
        let engine = Engine::default();
        let mut store = Store::new(&engine, TestHttpCtx::default());
        let (tx, rx) = oneshot::channel();
        let (demand_tx, mut demand_rx) = oneshot::channel();
        let mut producer = HttpTransmissionFutureProducer {
            rx,
            demand_tx: Some(demand_tx),
        };
        let mut cx = Context::from_waker(std::task::Waker::noop());

        assert!(matches!(
            Pin::new(&mut producer).poll_produce(&mut cx, store.as_context_mut(), false),
            Poll::Pending
        ));
        assert!(
            demand_rx.try_recv().is_ok(),
            "the first real poll must fire the recording demand"
        );

        assert!(
            tx.send(HttpTransmissionResolution::Outcome(Err(
                ErrorCode::ConnectionTerminated
            )))
            .is_ok()
        );
        let produced = Pin::new(&mut producer).poll_produce(&mut cx, store.as_context_mut(), false);
        assert!(
            matches!(
                produced,
                Poll::Ready(Ok(Some(Err(ErrorCode::ConnectionTerminated))))
            ),
            "producer must deliver the resolved transmission outcome"
        );
        assert!(
            producer.demand_tx.is_none(),
            "the demand must fire exactly once"
        );
    }

    /// A cancellation-only poll (`finish == true` while pending) must not fire
    /// the recording demand: a guest that never really reads the transmission
    /// future must leave no durable trace.
    #[test]
    fn transmission_future_producer_does_not_demand_on_cancellation() {
        let engine = Engine::default();
        let mut store = Store::new(&engine, TestHttpCtx::default());
        let (_tx, rx) = oneshot::channel();
        let (demand_tx, mut demand_rx) = oneshot::channel::<()>();
        let mut producer = HttpTransmissionFutureProducer {
            rx,
            demand_tx: Some(demand_tx),
        };
        let mut cx = Context::from_waker(std::task::Waker::noop());

        let produced = Pin::new(&mut producer).poll_produce(&mut cx, store.as_context_mut(), true);
        assert!(matches!(produced, Poll::Ready(Ok(None))));
        assert!(
            matches!(
                demand_rx.try_recv(),
                Err(oneshot::error::TryRecvError::Empty)
            ),
            "a cancellation-only poll must not fire the recording demand"
        );
        assert!(producer.demand_tx.is_some());
    }

    /// A durability failure (a `Trap` resolution, or the resolution channel
    /// closing without a resolution) must trap the guest-facing transmission
    /// future rather than resolve it to a normal value that would mask the
    /// failure.
    #[test]
    fn transmission_future_producer_traps_on_durability_failure() {
        let engine = Engine::default();
        let mut store = Store::new(&engine, TestHttpCtx::default());
        let mut cx = Context::from_waker(std::task::Waker::noop());

        let (trap_tx, trap_rx) = oneshot::channel();
        let mut trap_producer = HttpTransmissionFutureProducer {
            rx: trap_rx,
            demand_tx: None,
        };
        assert!(
            trap_tx
                .send(HttpTransmissionResolution::Trap {
                    message: "request-body transmission durable persistence failed".to_string(),
                    trap_context: DurableCallTrapContext {
                        retry_from: OplogIndex::INITIAL,
                        in_atomic_region: false,
                    },
                })
                .is_ok()
        );
        assert!(
            matches!(
                Pin::new(&mut trap_producer).poll_produce(&mut cx, store.as_context_mut(), false),
                Poll::Ready(Err(_))
            ),
            "a Trap resolution must trap the transmission future"
        );

        let (closed_tx, closed_rx) = oneshot::channel::<HttpTransmissionResolution>();
        drop(closed_tx);
        let mut closed_producer = HttpTransmissionFutureProducer {
            rx: closed_rx,
            demand_tx: None,
        };
        assert!(
            matches!(
                Pin::new(&mut closed_producer).poll_produce(&mut cx, store.as_context_mut(), false),
                Poll::Ready(Err(_))
            ),
            "a resolution channel closed without a resolution must trap the transmission future"
        );
    }

    /// The interposition installed by the durable `request::new` pipes the
    /// built-in transmission `FutureReader` into a plain channel via
    /// [`HttpTransmissionResultForwarder`]; the piped value must arrive on the
    /// raw channel so the durable/passthrough tasks can await it without store
    /// access. The pipe transfer only runs while the store's event loop is
    /// driven, so the receive happens inside `run_concurrent`.
    #[test]
    #[timeout("10s")]
    async fn transmission_result_forwarder_forwards_piped_value() {
        let mut config = wasmtime::Config::new();
        config.concurrency_support(true);
        let engine = Engine::new(&config).unwrap();
        let mut store = Store::new(&engine, TestHttpCtx::default());

        let (raw_tx, raw_rx) = oneshot::channel();
        let raw = store
            .run_concurrent(
                async move |accessor| -> wasmtime::Result<Result<(), ErrorCode>> {
                    accessor.with(|mut store| -> wasmtime::Result<()> {
                        let reader = FutureReader::new(&mut store, async {
                            Ok::<Result<(), ErrorCode>, wasmtime::Error>(Err(
                                ErrorCode::ConnectionTerminated,
                            ))
                        })?;
                        reader.pipe(&mut store, HttpTransmissionResultForwarder(Some(raw_tx)))
                    })?;
                    Ok(raw_rx.await.unwrap_or(Ok(())))
                },
            )
            .await
            .unwrap()
            .unwrap();

        assert!(
            matches!(raw, Err(ErrorCode::ConnectionTerminated)),
            "the piped transmission result must arrive on the raw channel, got {raw:?}"
        );
    }

    /// The rebuild resend body streams the recorded coverage back from the
    /// oplog: overlapping frames from merged crash/re-exec generations are
    /// split at the already-delivered boundary (never duplicating bytes), the
    /// recorded trailers follow the data, and the body ends cleanly.
    #[test]
    #[timeout("10s")]
    async fn recorded_request_body_replay_splits_overlapping_generations() {
        const PARENT: OplogIndex = OplogIndex::NONE;
        let oplog = FrameTestOplog::new();
        for frame in [
            SerializableP3HttpRequestBodyFrame::Data {
                offset: 3,
                bytes: b"lo!".to_vec(),
            },
            SerializableP3HttpRequestBodyFrame::Data {
                offset: 0,
                bytes: b"hel".to_vec(),
            },
            SerializableP3HttpRequestBodyFrame::Data {
                offset: 0,
                bytes: b"hell".to_vec(),
            },
            SerializableP3HttpRequestBodyFrame::Trailers(Some(
                [("x-check".to_string(), vec![b"sum".to_vec()])]
                    .into_iter()
                    .collect(),
            )),
            SerializableP3HttpRequestBodyFrame::End,
        ] {
            record_frame_entry(oplog.clone(), PARENT, frame)
                .await
                .expect("failed to record a test frame");
        }

        let scan = scan_recorded_request_body_frames(oplog.clone(), PARENT)
            .await
            .expect("scan failed");
        assert_eq!(scan.covered_len, 6);
        assert!(matches!(
            scan.terminal,
            Some(RecordedRequestBodyTerminal::End)
        ));

        let collected = recorded_request_body_replay(oplog, &scan)
            .collect()
            .await
            .expect("replay body failed");
        let trailers = collected.trailers().cloned();
        assert_eq!(collected.to_bytes().as_ref(), b"hello!");
        let trailers = trailers.expect("recorded trailers must be re-emitted");
        assert_eq!(
            trailers.get("x-check").map(|value| value.as_bytes()),
            Some(b"sum".as_slice())
        );
    }

    /// A bodiless recording (terminal frame only) replays as an empty body
    /// without trailers.
    #[test]
    #[timeout("10s")]
    async fn recorded_request_body_replay_of_bodiless_recording_is_empty() {
        const PARENT: OplogIndex = OplogIndex::NONE;
        let oplog = FrameTestOplog::new();
        record_frame_entry(
            oplog.clone(),
            PARENT,
            SerializableP3HttpRequestBodyFrame::End,
        )
        .await
        .expect("failed to record a test frame");

        let scan = scan_recorded_request_body_frames(oplog.clone(), PARENT)
            .await
            .expect("scan failed");
        assert_eq!(scan.covered_len, 0);
        assert!(matches!(
            scan.terminal,
            Some(RecordedRequestBodyTerminal::End)
        ));

        let collected = recorded_request_body_replay(oplog, &scan)
            .collect()
            .await
            .expect("replay body failed");
        assert!(collected.trailers().is_none());
        assert!(collected.to_bytes().is_empty());
    }
}
