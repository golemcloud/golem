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

fn body_span_outcome(
    cancelled: bool,
    terminal: &HttpTrailersOutcome,
) -> golem_common::model::oplog::SpanOutcome {
    if cancelled {
        golem_common::model::oplog::SpanOutcome::Cancelled
    } else if terminal.is_ok() {
        golem_common::model::oplog::SpanOutcome::Completed
    } else {
        golem_common::model::oplog::SpanOutcome::Failed
    }
}

async fn require_recording_before_response_demand(
    readiness: impl Future<Output = Result<(), WorkerExecutorError>>,
    demand_signal: Arc<HttpBodyDemandSignal>,
    request_body_progress: Option<Arc<ReplayedRequestBodyDrainProgress>>,
) -> Result<(), WorkerExecutorError> {
    let Some(request_body_progress) = request_body_progress else {
        return readiness.await;
    };
    tokio::pin!(readiness);
    let mut response_demanded = demand_signal.observed.load(Ordering::Acquire);
    loop {
        let request_body_waiting = request_body_progress.is_waiting_for_guest_body();
        if response_demanded && request_body_waiting {
            tokio::select! {
                biased;
                result = &mut readiness => return result,
                () = std::future::ready(()) => return Err(WorkerExecutorError::runtime(
                    "response-body demand arrived while replay was waiting for more guest request-body data",
                ))
            }
        }

        if response_demanded {
            tokio::select! {
                biased;
                result = &mut readiness => return result,
                () = request_body_progress.wait_for_change(request_body_waiting) => {}
            }
        } else {
            tokio::select! {
                biased;
                result = &mut readiness => return result,
                () = demand_signal.observed() => response_demanded = true,
                () = request_body_progress.wait_for_change(request_body_waiting) => {}
            }
        }
    }
}

pub(in crate::durable_host::p3::http) fn serialize_consume_body_result(
    result: &Result<Option<HeaderMap>, ErrorCode>,
) -> SerializableP3HttpConsumeBodyResult {
    match result {
        Ok(trailers) => {
            SerializableP3HttpConsumeBodyResult::Trailers(trailers.as_ref().map(serialize_headers))
        }
        Err(error) => SerializableP3HttpConsumeBodyResult::HttpError(serialize_error_code(error)),
    }
}

pub(in crate::durable_host::p3::http) fn deserialize_consume_body_result(
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
/// (via `DurableCallSession::trap`, a `TerminalCallError`, or `mark_durable_call_trap_context`)
/// so post-trap retry grouping stays owned by that call's scope; this helper does
/// not stringify it for the returned trap.
///
/// The guest-facing trailers future is resolved with a [`HttpTrailersResolution::Trap`]
/// carrying `trap_context` (the failing call scope's context) so it also fails
/// loud — with correct retry grouping — instead of resolving to a normal error
/// that would mask the durability failure. When `trap_context` is `None` (no
/// owning call scope exists yet) the sender is dropped, which still traps the
/// trailers future loudly.
pub(in crate::durable_host::p3::http) fn fail_consume_body_task(
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

/// One item produced by a single iteration of the durable consume-body loop —
/// after the chunk has been persisted (live) or replayed (replay) — to be
/// delivered to the guest-facing body stream.
pub(in crate::durable_host::p3::http) enum ProducedChunk {
    /// A non-empty body chunk to hand to the guest.
    Data(Bytes),
    /// The recorded stream's terminal: there are no more chunks to deliver.
    End,
    /// A pending guest read was cancelled; finalize durability without
    /// delivering EOF to the guest-facing stream.
    Cancelled,
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
pub(in crate::durable_host::p3::http) struct HttpConsumeBodyTask<Ctx> {
    body: UnsyncBoxBody<Bytes, ErrorCode>,
    demand_rx: mpsc::Receiver<HttpBodyDemand>,
    demand_signal: Arc<HttpBodyDemandSignal>,
    trailers_tx: oneshot::Sender<HttpTrailersResolution>,
    trailers_delivery: Arc<HttpTrailersDelivery>,
    /// Open-response state of the send that produced this response (its
    /// `outgoing-http-request` span, retry properties, and — for a replayed
    /// response — the send rebuild info), taken over from the response
    /// resource in `consume_body`. The span is finished (durably) right after
    /// the parent terminal, mirroring the P2 `end_http_request` span
    /// lifecycle. `None` for responses that did not come from `client::send`.
    response_state: Option<OpenP3HttpResponseState>,
    /// The invocation id of the `consume-body` guest import that spawned the task.
    invocation_id: InvocationIdSource,
    activity: TailActivity,
    _phantom: PhantomData<fn() -> Ctx>,
}

impl<Ctx> HttpConsumeBodyTask<Ctx> {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn new(
        body: UnsyncBoxBody<Bytes, ErrorCode>,
        demand_rx: mpsc::Receiver<HttpBodyDemand>,
        demand_signal: Arc<HttpBodyDemandSignal>,
        trailers_tx: oneshot::Sender<HttpTrailersResolution>,
        trailers_delivery: Arc<HttpTrailersDelivery>,
        response_state: Option<OpenP3HttpResponseState>,
        invocation_id: InvocationIdSource,
        activity: TailActivity,
    ) -> Self {
        Self {
            body,
            demand_rx,
            demand_signal,
            trailers_tx,
            trailers_delivery,
            response_state,
            invocation_id,
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
            demand_rx,
            demand_signal,
            trailers_tx,
            trailers_delivery,
            mut response_state,
            invocation_id,
            activity,
            ..
        } = self;

        let recording_recovery = RequestBodyRecovery::default();
        let unreconstructable = Arc::new(AtomicBool::new(false));
        let scope_replay_recovery = match response_state.as_mut() {
            Some(state) if state.body_is_placeholder => match state.resend.as_mut() {
                Some(rebuild) => {
                    let request_body_progress = rebuild
                        .replayed_request_body_drain
                        .as_ref()
                        .map(|drain| drain.progress.clone());
                    let readiness = require_recording_before_response_demand(
                        recorded_request_body_replayability(
                            accessor,
                            rebuild,
                            recording_recovery.clone(),
                            false,
                        ),
                        demand_signal.clone(),
                        request_body_progress,
                    );
                    let recovery = recording_recovery.clone();
                    let unreconstructable = unreconstructable.clone();
                    ScopeReplayRecovery::decide_when(async move {
                        match readiness.await {
                            Ok(()) => Ok(ScopeReplayDecision::Reexecute),
                            Err(_) if recovery.error().is_some() => Err(recovery.error().unwrap()),
                            Err(_) => {
                                unreconstructable.store(true, Ordering::Release);
                                Ok(ScopeReplayDecision::CompleteWithLoss)
                            }
                        }
                    })
                }
                // The owner cannot re-issue this response, but still owns completion of its
                // original scope and chunk calls with HttpResponseIncomplete.
                None => {
                    unreconstructable.store(true, Ordering::Release);
                    ScopeReplayRecovery::CompleteWithLoss
                }
            },
            _ => ScopeReplayRecovery::Default,
        };

        let (
            response_span,
            retry_properties,
            resume_context,
            resend,
            mut pending_reissue,
            observational_owner,
        ) = match response_state {
            Some(state) => {
                let properties = RetryContext::http(&state.method, &state.uri);
                (
                    Some(state.span),
                    Some(properties),
                    Some((state.method, state.uri)),
                    state.resend,
                    state.body_is_placeholder,
                    state.observational_owner,
                )
            }
            None => (None, None, None, None, false, None),
        };
        // Keeps re-issued request I/O alive and carries storage failures from its request body.
        let mut _rebuild_io_guard: Option<AbortOnDropIoTask> = None;
        // Set when a terminal error must not be routed through worker-level
        // or inline retry: a refused rebuild
        // (request body not reconstructable — a retry
        // would replay into the same refusal forever) or a content-changed
        // response-body resume (416 / short full response — deterministic for
        // the same request).
        let mut retry_exempt = false;
        // In-function retry budget of the response-body resume path, shared across all resume
        // attempts of this consume-body scope.
        let mut resume_retry_state = InFunctionRetryState::new();
        let mut resume_semantic_override = None;
        let mut resume_retry_ctx: Option<TaskRetryContext<Ctx>> = None;
        // Bytes delivered to the guest-facing stream so far (replayed chunks +
        // live frames): the resume offset for `Range` requests, and their digest, which a
        // re-sent full response must reproduce before its remainder may continue the stream.
        let mut delivered_bytes: u64 = 0;
        let mut delivered_digest = blake3::Hasher::new();

        // Open the parent call. Children nest under its Start index. Concurrently
        // consumed response bodies are told apart by the invocation id of each `consume-body`.
        let mut stream = match DurableDemandStream::<
            P3HttpClientConsumeBody,
            P3HttpClientConsumeBodyChunk,
            HttpBodyDemand,
        >::start(
            accessor,
            durable_worker_ctx::<Ctx, U>,
            demand_rx,
            AccessClaimOptions {
                parent_start_index: None,
                observational_owner,
                scope_replay_recovery,
                invocation_id,
                ..Default::default()
            },
            DemandDeliveryMode::Deferred,
            HostRequestNoInput {},
        )
        .await
        {
            Ok(stream) => stream,
            // No parent handle exists yet, so there is nothing to abandon; the
            // `WorkerExecutorError` carries no call context but there is no scope
            // to group against either.
            Err(error) => {
                if let Some(error) = recording_recovery.error() {
                    return Err(error.into());
                }
                return fail_consume_body_task(trailers_tx, wasmtime::Error::from(error), None);
            }
        };
        let parent_begin = stream.start_index();

        // The trailers / body-error terminal, set on the live path; on replay it
        // is taken from the parent marker instead.
        let mut terminal: HttpTrailersOutcome = Ok(None);
        let mut body_cancelled = false;
        let mut cancel_ack: Option<oneshot::Sender<()>> = None;

        loop {
            // Safe park: waiting for the guest to demand the next body chunk.
            let (demand, cancel_rx, mut read_cancel_ack, guest) =
                match stream.next_demand(&activity).await {
                    Some(HttpBodyDemand::Read {
                        reply,
                        cancel,
                        cancel_ack,
                        guest,
                    }) => {
                        if reply.is_closed() {
                            break;
                        }
                        (reply, Some(cancel), Some(cancel_ack), guest)
                    }
                    Some(HttpBodyDemand::Cancel(ack)) => {
                        cancel_ack = Some(ack);
                        break;
                    }
                    None => break,
                };

            let item = match stream
                .next(
                    accessor,
                    durable_worker_ctx::<Ctx, U>,
                    HostRequestNoInput {},
                )
                .await
            {
                Ok(item) => item,
                Err(error @ WorkerExecutorError::RecoveryRequired { .. }) => {
                    return Err(error.into());
                }
                Err(error) => {
                    // Durable-machinery failure (not an HTTP body error): surface
                    // it to the in-flight guest read and leave the parent incomplete.
                    let trap_context = stream.trap_context();
                    let _ = demand.send(HttpBodyChunkReply::Failed {
                        message: error.to_string(),
                        trap_context,
                    });
                    return fail_consume_body_task(
                        trailers_tx,
                        wasmtime::Error::from_anyhow(stream.trap(error)),
                        Some(trap_context),
                    );
                }
            };

            // Produce the next item: replay the recorded child (replay) or read
            // the upstream body and persist it (live). Delivery to the guest-facing
            // stream happens afterwards, identically on both paths.
            //
            // A child terminal is durable before transfer, but its deferred-delivery token
            // remains owned by the reply until the reader observes it. Dropping an unread reply
            // records a discard; merely handing bytes to the producer is not delivery.
            let (produced, delivery) = match item {
                DurableDemandItem::Replayed { response, delivery } => {
                    let replay_at_marker = delivery.is_replay_at_marker();
                    let replay_discarded = delivery.is_replay_discarded();
                    let produced = match response.chunk {
                        SerializableP3HttpBodyChunk::Data(bytes) => {
                            drop(cancel_rx);
                            // A marker-bearing replay must keep cancel-read pending until the
                            // recorded bytes reach Wasmtime's destination. Markerless and
                            // discarded completions retain the live cancellation race; the
                            // latter must release the ack before parking to avoid a cycle.
                            if !replay_at_marker {
                                drop(read_cancel_ack.take());
                            }
                            ProducedChunk::Data(Bytes::from(bytes))
                        }
                        SerializableP3HttpBodyChunk::End => {
                            drop(cancel_rx);
                            if !replay_at_marker {
                                drop(read_cancel_ack.take());
                            }
                            ProducedChunk::End
                        }
                        SerializableP3HttpBodyChunk::Cancelled => {
                            if replay_discarded {
                                drop(cancel_rx);
                                drop(read_cancel_ack.take());
                            } else {
                                if let Some(cancel_rx) = cancel_rx {
                                    let _ = cancel_rx.await;
                                }
                                if replay_at_marker {
                                    cancel_ack = read_cancel_ack.take();
                                } else {
                                    drop(read_cancel_ack.take());
                                }
                            }
                            terminal = Ok(None);
                            ProducedChunk::Cancelled
                        }
                    };
                    (produced, delivery)
                }
                DurableDemandItem::Live(mut child) => {
                    // Live upstream body reads (including a re-issued request's body) can park
                    // indefinitely waiting for network bytes, so they must race the worker's
                    // interrupt signal: worker interruption / the invocation deadline can only
                    // unwind the event loop cooperatively, from within a parked host future.
                    let interrupt = accessor.with(|mut access| {
                        durable_worker_ctx::<Ctx, U>(access.data_mut()).create_interrupt_signal()
                    });
                    let read_frame = async {
                        let frame = if pending_reissue {
                            // First live read of a replayed response's placeholder body:
                            // the durable consume-body scope was either missing entirely
                            // (the crash happened before its `Start` was committed, so a
                            // synthetic scope was recovered live) or incomplete (the
                            // original run was interrupted mid-body-stream; its recorded
                            // chunks replayed and the scope continues live under its
                            // recorded `Start`), and the placeholder carries no data.
                            // Re-issue the recorded request now and stream the fresh body
                            // instead. This only fires on a real guest demand: a dropped
                            // stream or a cleanly replaying scope never re-issues. Chunks the
                            // guest already observed are never re-delivered: the fresh body
                            // must continue them (`resume_recorded_response`) or the stream
                            // ends with an explicit incomplete-response error.
                            pending_reissue = false;
                            match resend
                                .as_ref()
                                .filter(|_| !unreconstructable.load(Ordering::Acquire))
                            {
                                Some(resend) if delivered_bytes > 0 => {
                                    match resume_recorded_response::<Ctx, U>(
                                        accessor,
                                        resend,
                                        delivered_bytes,
                                        &delivered_digest,
                                    )
                                    .await
                                    {
                                        ResumeOutcome::RecoveryRequired(error) => {
                                            return Err(error);
                                        }
                                        ResumeOutcome::Resumed {
                                            body: fresh_body,
                                            io_guard,
                                            leftover,
                                        } => {
                                            body = fresh_body;
                                            _rebuild_io_guard = Some(io_guard);
                                            match leftover {
                                                Some(bytes) => HttpBodyFrame::Data(bytes),
                                                None => read_http_body_frame(&mut body).await,
                                            }
                                        }
                                        ResumeOutcome::Failed(code) => HttpBodyFrame::Error(code),
                                        ResumeOutcome::Interrupted(reason) => {
                                            tracing::warn!(
                                                delivered_bytes,
                                                %reason,
                                                "p3 HTTP response body cannot continue after a \
                                                 restart; the guest observes an incomplete response"
                                            );
                                            retry_exempt = true;
                                            HttpBodyFrame::Error(ErrorCode::HttpResponseIncomplete)
                                        }
                                    }
                                }
                                Some(resend) => {
                                    match reissue_recorded_request::<Ctx, U>(accessor, resend).await
                                    {
                                        RebuildOutcome::RecoveryRequired(error) => {
                                            return Err(error);
                                        }
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
                                            debug!(%message, "p3 HTTP response rebuild refused");
                                            retry_exempt = true;
                                            HttpBodyFrame::Error(ErrorCode::HttpResponseIncomplete)
                                        }
                                    }
                                }
                                None => {
                                    retry_exempt = true;
                                    HttpBodyFrame::Error(ErrorCode::HttpResponseIncomplete)
                                }
                            }
                        } else {
                            read_http_body_frame(&mut body).await
                        };
                        Ok::<_, WorkerExecutorError>(frame)
                    };
                    let cancelled = async {
                        match cancel_rx {
                            Some(cancel_rx) => {
                                let _ = cancel_rx.await;
                            }
                            None => std::future::pending().await,
                        }
                    };
                    let mut frame = tokio::select! {
                        biased;
                        () = cancelled => {
                            cancel_ack = read_cancel_ack.take();
                            HttpBodyFrame::Cancelled
                        }
                        frame = read_frame => match frame {
                            Ok(frame) => frame,
                            Err(error) => {
                                child.abandon_for_trap();
                                stream.abandon_for_trap();
                                return Err(error.into());
                            }
                        },
                        kind = interrupt => {
                            // An interrupt is not a cancellation and not a hard error:
                            // abandon the open chunk child first, then the parent, so
                            // neither writes a terminal (both `Start`s stay incomplete
                            // and re-execute on resume), and unwind the event loop with
                            // the interrupt kind directly so it classifies as
                            // `TrapType::Interrupt`.
                            child.abandon_for_trap();
                            stream.abandon_for_trap();
                            return Err(wasmtime::Error::from_anyhow(kind.into()));
                        }
                    };
                    // A frame read without cancellation releases a later cancel-read at once.
                    // The frame reaches the guest or is discarded through the child's delivery
                    // token, so cancel-read must not wait for the child's durable terminal.
                    drop(read_cancel_ack.take());

                    if let Some(error) = _rebuild_io_guard
                        .as_ref()
                        .and_then(|guard| guard.1.error())
                        .or_else(|| {
                            resend
                                .as_ref()
                                .and_then(|resend| resend.durable_body.as_ref())
                                .and_then(|body| body.recovery.error())
                        })
                    {
                        child.abandon_for_trap();
                        stream.abandon_for_trap();
                        return Err(error.into());
                    }
                    if matches!(frame, HttpBodyFrame::Cancelled) {
                        // Stop the resend's connection driver before acknowledging cancellation.
                        drop(_rebuild_io_guard.take());
                    }

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
                        let (Some(resend), Some((method, uri))) =
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
                        if has_guest_range_header(resend.request.headers.keys().map(String::as_str))
                        {
                            break;
                        }
                        // The same live/snapshot and observational gates as the send's retry loop.
                        if observational_owner.is_some()
                            || !inline_retry_eligible_for_method::<Ctx, U>(
                                accessor,
                                &resend.request.method,
                            )
                        {
                            break;
                        }

                        let properties =
                            RetryContext::http_with_response(method, uri, None, "transient");
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
                            AsyncRetryDecision::FallBackToTrap(semantic_override) => {
                                resume_semantic_override = semantic_override;
                                break;
                            }
                            AsyncRetryDecision::Exhausted => {
                                break;
                            }
                        }

                        let range_headers = resume_range_headers(delivered_bytes);
                        match resend_recorded_request::<Ctx, U>(accessor, resend, &range_headers)
                            .await
                        {
                            // Storage outages reconstruct with infrastructure backoff. A
                            // recording that cannot reproduce the request reports typed loss;
                            // semantic retry cannot make that recording replayable.
                            ResendOutcome::RecoveryRequired(error) => {
                                child.abandon_for_trap();
                                stream.abandon_for_trap();
                                return Err(error.into());
                            }
                            ResendOutcome::Refused(reason) => {
                                debug!(%reason, "p3 HTTP response-body resume refused");
                                retry_exempt = true;
                                frame = HttpBodyFrame::Error(ErrorCode::HttpResponseIncomplete);
                                break;
                            }
                            // The resume send itself failed: charge another resume
                            // attempt against the budget with the fresh error.
                            ResendOutcome::Failed(code) => {
                                frame = HttpBodyFrame::Error(code);
                            }
                            ResendOutcome::Sent { response, io_guard } => {
                                let status = response.status().as_u16();
                                let content_range_start = response
                                    .headers()
                                    .get("content-range")
                                    .and_then(|value| value.to_str().ok())
                                    .and_then(parse_content_range_start);
                                match classify_resume_response(
                                    status,
                                    content_range_start,
                                    delivered_bytes,
                                    Some(resend.recorded_status),
                                ) {
                                    ResumeResponseAction::Resume => {
                                        body = response.into_body();
                                        _rebuild_io_guard = Some(io_guard);
                                        frame = read_http_body_frame(&mut body).await;
                                    }
                                    ResumeResponseAction::RangeNotSatisfiable => {
                                        // The server refuses the range: the resource
                                        // changed since the original response, so the
                                        // already-delivered prefix cannot be continued.
                                        // Deterministic for this request — never
                                        // retry-routed.
                                        retry_exempt = true;
                                        frame =
                                            HttpBodyFrame::Error(ErrorCode::HttpResponseIncomplete);
                                        break;
                                    }
                                    ResumeResponseAction::SkipPrefix => {
                                        // The server ignored the range and re-sent the
                                        // full response with the original status: skip
                                        // the already-delivered prefix, which must be
                                        // byte-identical to what the guest observed, and
                                        // continue from there.
                                        let mut fresh_body = response.into_body();
                                        let skipped = skip_body_prefix(
                                            &mut fresh_body,
                                            delivered_bytes,
                                            &delivered_digest,
                                        )
                                        .await;
                                        if let Some(error) = io_guard.1.error() {
                                            child.abandon_for_trap();
                                            stream.abandon_for_trap();
                                            return Err(error.into());
                                        }
                                        match skipped {
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
                                                frame = HttpBodyFrame::Error(
                                                    ErrorCode::HttpResponseIncomplete,
                                                );
                                                break;
                                            }
                                            Err(SkipBodyPrefixError::Mismatch) => {
                                                retry_exempt = true;
                                                frame = HttpBodyFrame::Error(
                                                    ErrorCode::HttpResponseIncomplete,
                                                );
                                                break;
                                            }
                                            Err(SkipBodyPrefixError::Read(code)) => {
                                                frame = HttpBodyFrame::Error(code);
                                            }
                                        }
                                    }
                                    ResumeResponseAction::Fallback => {
                                        debug!(
                                            status,
                                            recorded_status = resend.recorded_status,
                                            ?content_range_start,
                                            delivered_bytes,
                                            "p3 HTTP response-body resume: resume response not \
                                         usable, falling back"
                                        );
                                        break;
                                    }
                                }
                            }
                        }
                        if let Some(error) =
                            _rebuild_io_guard.as_ref().and_then(|guard| guard.1.error())
                        {
                            child.abandon_for_trap();
                            stream.abandon_for_trap();
                            return Err(error.into());
                        }
                    }

                    // Worker-level retry classification for live body-transfer
                    // errors, mirroring the P2 body-stream read path: a transient
                    // error raises a retry trap here — before anything about this
                    // frame is persisted or delivered, so the guest never observes
                    // a truncated stream — leaving the parent `Start` incomplete.
                    // Replay repairs the same scope and re-issues the recorded
                    // request (see `reissue_recorded_request`), verifying the prefix
                    // already delivered before continuing. Permanent errors — and
                    // transient ones whose retry budget is exhausted — fall through
                    // and are recorded as the terminal, which is also what a
                    // recorded terminal replays as. A retry-exempt failure (refused
                    // rebuild or content-changed resume) is never retry-routed:
                    // repeating it would fail deterministically.
                    if let HttpBodyFrame::Error(error_code) = &frame
                        && !retry_exempt
                        && let Some(retry_properties) = retry_properties.clone()
                    {
                        let for_retry: Result<(), &ErrorCode> = Err(error_code);
                        let trap_context = stream.trap_context();
                        let trap = if let Some(semantic_override) = resume_semantic_override.take()
                        {
                            let failure = semantic_trap_retry_override_error(
                                semantic_override,
                                HostFailureKind::Transient,
                                error_code.to_string(),
                            );
                            Some(stream.parent_mut().trap(failure))
                        } else {
                            stream
                                .parent_mut()
                                .try_trigger_retry_access(
                                    accessor,
                                    durable_worker_ctx::<Ctx, U>,
                                    &for_retry,
                                    |code| {
                                        classify_serializable_http_error_code(
                                            &serialize_error_code(code),
                                        )
                                    },
                                    retry_properties,
                                )
                                .await
                                .err()
                        };
                        if let Some(error) = trap {
                            // The retry trap tears the invocation down after the
                            // parent handle has been abandoned. The child's original
                            // `Start` stays incomplete for in-place repair; abandon the
                            // handle so its drop does not record a `Cancelled`. The span is
                            // deliberately left open — the retry's replay reconstructs it.
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
                        HttpBodyFrame::Data(bytes) => {
                            SerializableP3HttpBodyChunk::Data(bytes.to_vec())
                        }
                        HttpBodyFrame::End(_) | HttpBodyFrame::Error(_) => {
                            SerializableP3HttpBodyChunk::End
                        }
                        HttpBodyFrame::Cancelled => SerializableP3HttpBodyChunk::Cancelled,
                    };

                    let delivery = match stream
                        .complete_item(
                            accessor,
                            durable_worker_ctx::<Ctx, U>,
                            child,
                            HostResponseP3HttpClientConsumeBodyChunk { chunk },
                        )
                        .await
                    {
                        Ok(delivery) => delivery,
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
                            let trap_context = stream.trap_context();
                            let _ = demand.send(HttpBodyChunkReply::Failed {
                                message: error.to_string(),
                                trap_context,
                            });
                            stream.abandon_for_trap();
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
                }
            };

            // Deliver the produced item to the guest-facing stream. This is the
            // single point where chunks reach the guest, identically live and on
            // replay, so the count/order of delivered chunks always matches the
            // count/order of persisted children. It is also where the child's
            // deferred-delivery token is consumed: reader observation is
            // `delivered`, a dropped reply reserves the child's `CompletionDiscarded`
            // marker, and a replay-discarded child is never re-sent (the task parks
            // until the replayed guest drops the body reader at the same point it
            // did live).
            match produced {
                ProducedChunk::Data(bytes) => {
                    let chunk_len = bytes.len() as u64;
                    delivered_digest.update(&bytes);
                    match deliver_http_body_reply(&activity, demand, delivery, guest, |delivery| {
                        HttpBodyChunkReply::Data { bytes, delivery }
                    })
                    .await
                    {
                        Ok(DemandDelivery::Delivered) => delivered_bytes += chunk_len,
                        Ok(DemandDelivery::Abandoned) => {
                            // The undelivered chunk finalizes the body through the
                            // normal reader-drop path: the parent closes with a
                            // clean terminal, and if replay's guest never demands
                            // the recorded chunk, the whole abandoned scope is
                            // skipped at the invocation boundary.
                            terminal = Ok(None);
                            break;
                        }
                        Err(error) => {
                            let trap_context = stream.trap_context();
                            return fail_consume_body_task(
                                trailers_tx,
                                wasmtime::Error::from_anyhow(stream.trap(error)),
                                Some(trap_context),
                            );
                        }
                    }
                }
                ProducedChunk::End => {
                    match deliver_http_body_reply(&activity, demand, delivery, guest, |delivery| {
                        HttpBodyChunkReply::End { delivery }
                    })
                    .await
                    {
                        Ok(DemandDelivery::Delivered) => {}
                        Ok(DemandDelivery::Abandoned) => {}
                        Err(error) => {
                            let trap_context = stream.trap_context();
                            return fail_consume_body_task(
                                trailers_tx,
                                wasmtime::Error::from_anyhow(stream.trap(error)),
                                Some(trap_context),
                            );
                        }
                    }
                    break;
                }
                ProducedChunk::Cancelled => {
                    body_cancelled = true;
                    match deliver_http_body_reply(&activity, demand, delivery, guest, |delivery| {
                        HttpBodyChunkReply::Cancelled { delivery }
                    })
                    .await
                    {
                        Ok(_) => {}
                        Err(error) => {
                            let trap_context = stream.trap_context();
                            return fail_consume_body_task(
                                trailers_tx,
                                wasmtime::Error::from_anyhow(stream.trap(error)),
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

        // Finalize the parent after every child. The stream owns the live/replay
        // state transition and consumes itself here so it cannot finish earlier.
        // The trailers future's terminal-consumption observer is the real guest-facing delivery
        // boundary, so the terminal is recorded through the deferred-delivery API. Dropping the
        // future after the persisted `End` records a `CompletionDiscarded` marker instead of
        // replay redelivering the outcome.
        //
        // Capture the parent scope's trap context first (it is a pure function of
        // the scope and survives the handle being consumed below) so every
        // finalize failure can tag the guest-facing trailers trap for correct
        // retry grouping.
        let parent_trap_context = stream.trap_context();
        let repairing_terminal = !stream.is_live();
        let response = || {
            let terminal = if repairing_terminal && unreconstructable.load(Ordering::Acquire) {
                Err(ErrorCode::HttpResponseIncomplete)
            } else {
                terminal
            };
            (
                HostResponseP3HttpClientConsumeBodyResult {
                    result: serialize_consume_body_result(&terminal),
                },
                body_span_outcome(body_cancelled, &terminal),
            )
        };
        let close_span = response_span
            .as_ref()
            .is_some_and(P3HttpSendSpan::claim_closure);
        let finish = if close_span {
            let span = response_span.as_ref().expect("checked above");
            stream
                .finish_deferred_with_span(accessor, durable_worker_ctx::<Ctx, U>, || {
                    let (response, outcome) = response();
                    (response, p3_send_span_finished(span, outcome))
                })
                .await
        } else {
            stream
                .finish_deferred(accessor, durable_worker_ctx::<Ctx, U>, || response().0)
                .await
        };
        let (response, mut delivery) = match finish {
            Ok(result) => result,
            Err(error) if matches!(&error.source, WorkerExecutorError::RecoveryRequired { .. }) => {
                return Err(error.source.into());
            }
            Err(error) => {
                return fail_consume_body_task(
                    trailers_tx,
                    wasmtime::Error::from(error),
                    Some(parent_trap_context),
                );
            }
        };
        let outcome = deserialize_consume_body_result(response.result);

        // The response body reached its terminal and the parent marker is
        // committed/replayed: finish the send's `outgoing-http-request` span
        // before resolving the guest-facing trailers.
        if close_span {
            let span = response_span.as_ref().expect("checked above");
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

        // Cancellation is settled once the body and send span are durable. Without a recorded
        // trailers delivery marker, holding its acknowledgement across the delivery wait would
        // prevent the guest from returning (or dropping trailers) and advancing the replay tail.
        // Recorded deliveries retain their ordering; their acknowledgement stays below the gate.
        if !delivery.is_replay_at_marker()
            && let Some(ack) = cancel_ack.take()
        {
            let _ = ack.send(());
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
            let mut trailers_tx = trailers_tx;
            tokio::select! {
                biased;
                _ = trailers_tx.closed() => {},
                result = delivery.prepare_delivery(Some(&activity)) => {
                    result.map_err(|error| {
                        wasmtime::Error::from_anyhow(mark_durable_call_trap_context(
                            anyhow::Error::from(error),
                            parent_trap_context,
                        ))
                    })?;
                }
            }
            trailers_delivery.arm(delivery);
            match trailers_tx.send(HttpTrailersResolution::Outcome(outcome)) {
                Ok(()) => {}
                // The terminal observer has already settled the delivery token if the guest
                // dropped the future, or suppressed it if the future disappeared during a trap.
                Err(HttpTrailersResolution::Outcome(_)) => {}
                Err(HttpTrailersResolution::Trap { .. }) => {
                    unreachable!("the sent trailers resolution is an outcome")
                }
            }
        }
        if let Some(ack) = cancel_ack {
            let _ = ack.send(());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
