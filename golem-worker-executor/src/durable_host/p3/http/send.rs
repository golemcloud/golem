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

use super::rebuild::AbortOnDropIoTask;
use super::rebuild::P3HttpSendRebuild;
use super::serialization::{
    serialize_error_code, serialize_method, serialize_request, serialize_response_headers,
};
use super::*;
use crate::durable_host::concurrent::{
    AccessClaimOptions, AccessStartContext, CallHandle, CallReplayOutcome, Cancellable, DropPolicy,
    LeaveIncompleteOnDrop, finish_span_access, finish_span_in_memory,
    try_replay_recorded_start_span_access,
};
use crate::durable_host::durability::{
    AsyncRetryDecision, ClassifiedHostError, DurabilityHost, HostFailureKind, InFunctionRetryHost,
    InFunctionRetryState, TaskRetryContext, try_trigger_host_trap_retry,
};
use crate::durable_host::http::types::classify_serializable_http_error_code;
use crate::durable_host::p3::{DurableP3, DurableP3View, durable_worker_ctx, wasi_http_view};
use crate::services::HasWorker;
use crate::workerctx::WorkerCtx;
use anyhow::Context as _;
use bytes::Bytes;
use futures::future::{Either, select};
use golem_common::model::invocation_context::{AttributeValue, SpanId};
use golem_common::model::oplog::host_functions::P3HttpClientSend;
use golem_common::model::oplog::payload::types::{
    SerializableHttpMethod, SerializableP3HttpClientSend, SerializableP3HttpClientSendResult,
    SerializableP3HttpScheme,
};
use golem_common::model::oplog::{
    DurableFunctionType, HostRequest, HostRequestP3HttpClientSend,
    HostResponseP3HttpClientSendResult, OplogIndex,
};
use golem_common::model::{
    NamedRetryPolicy, OwnedAgentId, PredicateValue, RetryContext, RetryProperties,
};
use golem_service_base::error::worker_executor::WorkerExecutorError;
use golem_service_base::headers::TraceContextHeaders;
use http::{HeaderMap, HeaderName, HeaderValue};
use http_body_util::BodyExt as _;
use http_body_util::combinators::UnsyncBoxBody;
use std::collections::HashMap;
use std::future::Future;
use std::marker::PhantomData;
use std::num::NonZeroU64;
use std::task::{Context, Poll, Waker};
use tokio::sync::oneshot;
use tracing::debug;
use uuid::Uuid;
use wasmtime::AsContextMut;
use wasmtime::component::{Accessor, Resource};
use wasmtime_wasi_http::P3PooledConnection;
use wasmtime_wasi_http::p3::WasiHttp;
use wasmtime_wasi_http::p3::bindings::http::types::{ErrorCode, Request, Response};
use wasmtime_wasi_http::p3::bindings::http::{client, types};

impl<Ctx: WorkerCtx> client::Host for DurableP3View<'_, Ctx> {}

impl<U: Send + 'static, Ctx: WorkerCtx> client::HostWithStore<U> for DurableP3<Ctx> {
    async fn send(
        store: &Accessor<U, Self>,
        req: Resource<Request>,
    ) -> HttpResult<Resource<Response>> {
        let method = {
            let http_store = store.with_getter::<WasiHttp>(wasi_http_view::<Ctx, U>);
            http_store.with(|mut access| {
                let mut view = access.get();
                types::HostRequest::get_method(&mut view, borrow_resource(&req))
                    .map_err(HttpError::trap)
            })?
        };

        if is_idempotent_http_method(&serialize_method(method)) {
            send_with_durability::<Ctx, U, LeaveIncompleteOnDrop>(
                store,
                req,
                DurableFunctionType::WriteRemote,
            )
            .await
        } else {
            send_with_durability::<Ctx, U, Cancellable>(
                store,
                req,
                DurableFunctionType::WriteRemoteBatched(None),
            )
            .await
        }
    }
}

pub(super) async fn send_with_durability<Ctx, U, P>(
    store: &Accessor<U, DurableP3<Ctx>>,
    req: Resource<Request>,
    function_type: DurableFunctionType,
) -> HttpResult<Resource<Response>>
where
    Ctx: WorkerCtx,
    U: Send + 'static,
    P: DropPolicy,
{
    // Detach the request's body-transmission wiring (installed by the durable
    // `request::new`) *before* anything else: the inner `WasiHttp::send` deletes
    // the request from the resource table at the start of the call, and reps are
    // reused after deletion, so leaving the entry keyed by this rep until the
    // send resolves would let a concurrently created request clobber it. The
    // wiring is held locally through the arms below; the durable recording of
    // the transmission result starts after the send terminal is recorded (see
    // `start_transmission_recording`). On the trap paths it is simply dropped —
    // the invocation is torn down and the guest future is never polled again.
    //
    // `None` here means the request resource was not created through the durable
    // `request::new` wrapper (e.g. a forwarded incoming request); such a request
    // has no guest-held transmission future wired through us, so there is
    // nothing to record — deterministically on both live and replay paths.
    let pending_transmission = store.with(|mut access| {
        let ctx = durable_worker_ctx::<Ctx, U>(access.data_mut());
        ctx.state
            .pending_p3_http_request_transmissions
            .remove(&req.rep())
    });

    // Per-invocation HTTP call limit and monthly account-level HTTP call quota,
    // mirroring the P2 `http::outgoing_handler::handle` path. Both checks
    // no-op during replay and run before any durability machinery so that a
    // denied call writes no oplog entry.
    store.with(|mut access| {
        let ctx = durable_worker_ctx::<Ctx, U>(access.data_mut());
        ctx.state
            .check_and_increment_http_call_count()
            .map_err(|trap| HttpError::trap(wasmtime::Error::from(trap)))?;
        ctx.record_monthly_http_call()
            .map_err(|err| HttpError::trap(wasmtime::Error::from_anyhow(err)))
    })?;

    // The request head is serialized (and recorded) exactly as the guest built
    // it. The send's `outgoing-http-request` invocation-context span is
    // *derived*: its span id is a deterministic function of the send's own
    // host-call `Start` index, so no separate `StartSpan`/`FinishSpan` entries
    // are written or consumed — positional span entries are unsound under
    // concurrent sends (G35). Oplogs recorded by older executors still carry a
    // positional `StartSpan` between the durable scope `Start` and the
    // host-call `Start`; the request builder consumes it from that position
    // when present and the recorded span id is used instead (legacy mode).
    //
    // The Golem-managed headers (`traceparent`/`tracestate` and the derived
    // `idempotency-key`) are injected into the request *resource* only, after
    // the host-call `Start` is written/claimed: like on P2 they are not part of
    // the recorded head, because they are deterministic functions of recorded
    // state — the trace context of the send span (same derived/recorded span
    // id) and the idempotency key derived from the call's own `Start` index,
    // which is stable across live execution and replay.
    let mut legacy_send_span: Option<SpanId> = None;
    let legacy_send_span_out = &mut legacy_send_span;
    let serialized_request =
        serialize_request::<Ctx, U>(store, borrow_resource(&req)).map_err(|err| {
            HttpError::trap(WorkerExecutorError::runtime(format!(
                "failed to serialize outgoing p3 HTTP request: {err}"
            )))
        })?;
    // Concurrent sends share the durable identity of their records (same function name and
    // durable function type), so both the send's batched-write scope and its host-call `Start`
    // are made claim-safe by the request itself: the scope name carries a canonical digest of
    // the request head, and the `Start` claim matches the recorded request payload by value.
    // Sends with equal requests remain interchangeable and are claimed in oplog order.
    let host_request = HostRequestP3HttpClientSend {
        request: serialized_request.clone(),
    };
    let scope_discriminator =
        p3_send_request_discriminator(&serialized_request).map_err(|err| {
            HttpError::trap(WorkerExecutorError::runtime(format!(
                "failed to compute outgoing p3 HTTP request discriminator: {err}"
            )))
        })?;
    let claim_options = AccessClaimOptions {
        scope_discriminator: Some(format!("req:{scope_discriminator}")),
        request_identity: Some(HostRequest::from(host_request.clone())),
    };
    let mut handle = CallHandle::<P3HttpClientSend, P>::start_access_with_options(
        store,
        durable_worker_ctx::<Ctx, U>,
        function_type,
        claim_options,
        async |start_context: AccessStartContext| {
            if !start_context.is_live {
                *legacy_send_span_out =
                    try_replay_recorded_start_span_access(store, durable_worker_ctx::<Ctx, U>)
                        .await?;
            }
            Ok(host_request)
        },
    )
    .await
    .map_err(HttpError::trap)?;
    let send_start_index = handle.start_index();

    let span = match legacy_send_span {
        Some(span_id) => P3HttpSendSpan {
            span_id,
            legacy_durable: true,
        },
        None => store
            .with(|mut access| {
                let ctx = durable_worker_ctx::<Ctx, U>(access.data_mut());
                let span_id = derive_p3_send_span_id(&ctx.state.owned_agent_id, send_start_index);
                let parent = ctx.state.current_span_id.clone();
                let span = ctx
                    .state
                    .invocation_context
                    .start_span(&parent, Some(span_id.clone()))
                    .map_err(WorkerExecutorError::runtime)?;
                for (name, value) in outgoing_http_request_span_attributes(&serialized_request) {
                    span.set_attribute(name, value);
                }
                Ok::<_, WorkerExecutorError>(P3HttpSendSpan {
                    span_id,
                    legacy_durable: false,
                })
            })
            .map_err(HttpError::trap)?,
    };

    if !handle.is_live() {
        match handle
            .replay_access(store, durable_worker_ctx::<Ctx, U>)
            .await
            .map_err(HttpError::trap)?
        {
            CallReplayOutcome::Replayed(response) => {
                // The live path consumes the request inside `WasiHttp::send`.
                // On replay we never call `send`, so consume it here (delete
                // it, drain its outgoing body) before returning the recorded
                // response — otherwise the request leaks and a guest
                // streaming a body or awaiting its transmission future hangs.
                // A recorded-request-body result additionally lets the drain
                // complete an interrupted frame recording (self-heal); legacy
                // results keep the plain drain-and-discard.
                let recorded_request_body = match &response.result {
                    SerializableP3HttpClientSendResult::SuccessWithRecordedRequestBody {
                        recording_complete_at_end,
                        ..
                    } => Some(ReplayedRequestBodyRecording {
                        send_start_index,
                        recording_complete_at_end: *recording_complete_at_end,
                    }),
                    _ => None,
                };
                let has_recorded_request_body = recorded_request_body.is_some();
                consume_replayed_request::<Ctx, U>(store, req, recorded_request_body).await?;
                let recorded_status = match &response.result {
                    SerializableP3HttpClientSendResult::Success(headers) => headers.status,
                    SerializableP3HttpClientSendResult::SuccessWithRecordedRequestBody {
                        headers,
                        ..
                    } => headers.status,
                    SerializableP3HttpClientSendResult::HttpError(_) => 0,
                };
                let result = replay_send_response::<Ctx, U>(store, response.result);
                match result {
                    Ok(response) => {
                        // The replayed response carries an empty placeholder body:
                        // the recorded chunks are delivered by the durable
                        // consume-body scope on replay. If that scope turns out to
                        // be incomplete (the original run was interrupted
                        // mid-body-stream), its live re-execution must re-issue the
                        // recorded request to obtain a real body — capture
                        // everything needed for that here. The Golem-managed
                        // headers are re-derived from recorded state (same span,
                        // same `Start` index), so the re-issued request carries the
                        // same trace context and idempotency key as the original.
                        let injected_headers = golem_outgoing_http_headers::<Ctx, U>(
                            store,
                            &span.span_id,
                            send_start_index,
                            &serialized_request.headers,
                        )
                        .map_err(HttpError::trap)?;
                        // The span stays open until the response body
                        // completes; hand it to the replayed response so the
                        // consume-body / drop paths finish it at the same
                        // point as the live path.
                        register_open_response::<Ctx, U>(
                            store,
                            &response,
                            OpenP3HttpResponseState {
                                span,
                                method: serialized_request.method.to_string(),
                                uri: outgoing_http_request_uri(&serialized_request),
                                is_idempotent: effective_method_idempotence::<Ctx, U>(
                                    store,
                                    &serialized_request.method,
                                ),
                                resend: Some(P3HttpSendRebuild {
                                    request: serialized_request,
                                    injected_headers,
                                    recorded_status,
                                    recorded_request_body: has_recorded_request_body
                                        .then_some(send_start_index),
                                    durable_body: None,
                                }),
                                body_is_placeholder: true,
                            },
                        );
                        // Spawns the demand-gated transmission recorder
                        // at the same point as the live path, so a demanded
                        // recording's `Start` is claimed where it was appended.
                        start_transmission_recording::<Ctx, U>(store, pending_transmission);
                        return Ok(response);
                    }
                    Err(error) => {
                        // A recorded send error closed the span live right
                        // after the `End`; finish it here at the same point.
                        finish_p3_send_span::<Ctx, U>(store, &span)
                            .await
                            .map_err(HttpError::trap)?;
                        // Spawns the demand-gated transmission recorder
                        // at the same point as the live path, so a demanded
                        // recording's `Start` is claimed where it was appended.
                        start_transmission_recording::<Ctx, U>(store, pending_transmission);
                        return Err(error);
                    }
                }
            }
            CallReplayOutcome::Incomplete(live_handle) => handle = live_handle,
        }
    }

    // Inject the Golem-managed headers into the request resource before the
    // live send. This runs both for a fresh live call and for an
    // incomplete-replay re-execution, and derives identical values in both
    // cases: the trace context comes from the (recorded, replayed) span and
    // the idempotency key from the call's own `Start` index, which the replay
    // claim returns unchanged.
    let (retry_method, retry_uri, injected_headers) = {
        let request_head = serialize_request::<Ctx, U>(store, borrow_resource(&req))?;
        let injected = golem_outgoing_http_headers::<Ctx, U>(
            store,
            &span.span_id,
            handle.start_index(),
            &request_head.headers,
        )
        .map_err(HttpError::trap)?;
        apply_headers_to_request_resource::<Ctx, U>(store, &req, &injected)
            .map_err(HttpError::trap)?;
        (
            request_head.method.to_string(),
            outgoing_http_request_uri(&request_head),
            injected,
        )
    };

    // Every live send goes through the physical-send machinery below, so the
    // request body is recorded as durable frames for every persisted send —
    // independent of whether in-function retry is eligible.
    // `inline_retry_eligible` gates only the retry decisions inside the
    // attempt loop; with it false the first attempt's outcome is final and
    // flows through the common post-loop completion.
    let inline_retry_eligible =
        inline_retry_eligible_for_method::<Ctx, U>(store, &serialized_request.method);

    let converted =
        match convert_physical_send_request::<Ctx, U>(store, req, handle.start_index()).await {
            Ok(physical) => physical,
            Err(PhysicalSendConversionError::Trap(error)) => {
                return Err(HttpError::trap(wasmtime::Error::from_anyhow(
                    handle.trap(error),
                )));
            }
            Err(PhysicalSendConversionError::HttpError(error)) => {
                let _ = error
                    .final_transmission_tx
                    .send(Err(error.error_code.clone()));
                let error_code = error.error_code;
                let serialized_error = serialize_error_code(&error_code);
                let result = SerializableP3HttpClientSendResult::HttpError(serialized_error);
                handle
                    .complete_access(
                        store,
                        durable_worker_ctx::<Ctx, U>,
                        HostResponseP3HttpClientSendResult { result },
                    )
                    .await
                    .map_err(HttpError::trap)?;
                finish_p3_send_span::<Ctx, U>(store, &span)
                    .await
                    .map_err(HttpError::trap)?;
                start_transmission_recording::<Ctx, U>(store, pending_transmission);
                return Err(error_code.into());
            }
        };
    let PhysicalSend::Replayable(physical) = converted;
    let mut retry_state = InFunctionRetryState::new();
    let mut retry_task_ctx = make_p3_http_retry_task_context::<Ctx, U>(
        store,
        handle.start_index(),
        RetryContext::http(&retry_method, &retry_uri),
    )
    .await;

    let send_result = loop {
        let interrupt = store.with(|mut access| {
            durable_worker_ctx::<Ctx, U>(access.data_mut()).create_interrupt_signal()
        });
        let send = physical.attempt();
        let attempt_result = match select(Box::pin(send), interrupt).await {
            Either::Left((result, _)) => result,
            Either::Right((interrupt_kind, _)) => {
                let error: anyhow::Error = interrupt_kind.into();
                return Err(HttpError::trap(wasmtime::Error::from_anyhow(error)));
            }
        };

        match attempt_result {
            Ok((response, io, pooled_connection)) => {
                if inline_retry_eligible
                    && let Some(policy) = matching_status_retry_policy(
                        store,
                        &mut retry_task_ctx,
                        &retry_method,
                        &retry_uri,
                        response.status().as_u16(),
                        &serialized_request.method,
                    )
                    .await
                {
                    let properties = http_retry_properties(
                        store,
                        &retry_method,
                        &retry_uri,
                        Some(response.status().as_u16()),
                        "http-status",
                        &serialized_request.method,
                    );
                    retry_task_ctx.retry_properties = properties.clone();
                    match retry_state
                        .decide_retry_for_named_policy(
                            &mut retry_task_ctx,
                            "http-status-retry",
                            &properties,
                            &policy,
                        )
                        .await
                    {
                        AsyncRetryDecision::RetryAfterDelay(delay) => {
                            poison_p3_pooled_connection(&pooled_connection);
                            drop(response);
                            drop(io);
                            physical.body.abandon_active_live_view();
                            match physical.body.drain_to_terminal().await {
                                DurableRequestBodyDrainOutcome::Replayable => {
                                    tokio::time::sleep(delay).await;
                                    continue;
                                }
                                // The body cannot be resent (guest body error
                                // or a frame-recording failure): the inline
                                // retry is refused and the failure surfaces
                                // through the common post-loop handling.
                                DurableRequestBodyDrainOutcome::NotReplayable => {
                                    break Err(ErrorCode::InternalError(Some(
                                        "request body could not be replayed in-function"
                                            .to_string(),
                                    )));
                                }
                            }
                        }
                        AsyncRetryDecision::FallBackToTrap => {
                            let status = response.status().as_u16();
                            let properties = http_retry_properties(
                                store,
                                &retry_method,
                                &retry_uri,
                                Some(status),
                                "http-status",
                                &serialized_request.method,
                            );
                            let failure = anyhow::Error::new(ClassifiedHostError {
                                kind: HostFailureKind::Transient,
                                message: format!(
                                    "HTTP status {status} matched retry policy but exceeded the in-function retry delay threshold"
                                ),
                            });
                            if let Err(err) = try_trigger_host_trap_retry(
                                &mut retry_task_ctx,
                                failure,
                                properties,
                            )
                            .await
                            {
                                poison_p3_pooled_connection(&pooled_connection);
                                return Err(HttpError::trap(wasmtime::Error::from_anyhow(
                                    handle.trap(err),
                                )));
                            }
                            break Ok((response, io));
                        }
                        AsyncRetryDecision::Exhausted => {
                            break Ok((response, io));
                        }
                    }
                } else {
                    break Ok((response, io));
                }
            }
            Err(error_code)
                if inline_retry_eligible
                    && classify_serializable_http_error_code(&serialize_error_code(
                        &error_code,
                    )) == HostFailureKind::Transient =>
            {
                let properties = http_retry_properties(
                    store,
                    &retry_method,
                    &retry_uri,
                    None,
                    "transient",
                    &serialized_request.method,
                );
                retry_task_ctx.retry_properties = properties.clone();
                // A transient failure after request-body frames were already
                // pulled (i.e. the attempt was writing the body to the
                // network) is charged as a distinct request-body-write retry
                // phase; a failure before any body frame was consumed keeps
                // the plain in-task label.
                let retry_label = if physical.body.frames_consumed() {
                    "request-body-write"
                } else {
                    "in-task"
                };
                match retry_state
                    .decide_retry_with_properties(&mut retry_task_ctx, retry_label, &properties)
                    .await
                {
                    AsyncRetryDecision::RetryAfterDelay(delay)
                        if physical.body.can_replay_after_send_failure() =>
                    {
                        tokio::time::sleep(delay).await;
                        continue;
                    }
                    AsyncRetryDecision::RetryAfterDelay(_) => {
                        break Err(error_code);
                    }
                    AsyncRetryDecision::FallBackToTrap | AsyncRetryDecision::Exhausted => {
                        break Err(error_code);
                    }
                }
            }
            Err(error_code) => break Err(error_code),
        }
    };

    match send_result {
        Ok((http_response, io)) => {
            let response_status = http_response.status().as_u16();
            let send_start_index = handle.start_index();
            let response = response_resource_from_http::<Ctx, U>(
                store,
                http_response,
                io,
                physical.final_transmission_tx,
            )?;
            let headers = serialize_response_headers::<Ctx, U>(store, borrow_resource(&response))?;
            // When the request body was recorded as durable frames, the
            // recorded result says so, letting replay self-heal an
            // interrupted recording and letting a restart rebuild resend the
            // body from the oplog. `recording_complete_at_end` is a scan
            // shortcut only: frame appends run concurrently with the send, so
            // the recording may still complete after this point (its terminal
            // frame just lands later in the oplog).
            let result = if physical.body.recording_enabled() {
                SerializableP3HttpClientSendResult::SuccessWithRecordedRequestBody {
                    headers,
                    recording_complete_at_end: physical.body.recording_complete(),
                }
            } else {
                SerializableP3HttpClientSendResult::Success(headers)
            };
            handle
                .complete_access(
                    store,
                    durable_worker_ctx::<Ctx, U>,
                    HostResponseP3HttpClientSendResult { result },
                )
                .await
                .map_err(HttpError::trap)?;
            // The span stays open until the response body completes (the
            // durable consume-body terminal or an unconsumed drop), mirroring
            // the P2 `end_http_request` span lifecycle. The resend descriptor
            // lets a failed live body read resume via a ranged re-send; the
            // response body itself is the real network body (no placeholder).
            register_open_response::<Ctx, U>(
                store,
                &response,
                OpenP3HttpResponseState {
                    span,
                    method: retry_method.clone(),
                    uri: retry_uri.clone(),
                    is_idempotent: effective_method_idempotence::<Ctx, U>(
                        store,
                        &serialized_request.method,
                    ),
                    resend: Some(P3HttpSendRebuild {
                        request: serialized_request,
                        injected_headers,
                        recorded_status: response_status,
                        recorded_request_body: physical
                            .body
                            .recording_enabled()
                            .then_some(send_start_index),
                        durable_body: Some(physical.body.clone()),
                    }),
                    body_is_placeholder: false,
                },
            );
            // Spawns the demand-gated transmission recorder at a
            // deterministic point (right after the send `End`, mirrored by the
            // replay arm above): if the guest reads the transmission future,
            // the result is recorded here; otherwise no entries are written.
            start_transmission_recording::<Ctx, U>(store, pending_transmission);
            Ok(response)
        }
        Err(error_code) => {
            let _ = physical.final_transmission_tx.send(Err(error_code.clone()));
            let serialized_error = serialize_error_code(&error_code);

            // Worker-level retry classification, mirroring the P2
            // outgoing-handler path: a transient transport/protocol failure
            // raises a retry trap here (the worker goes to `Retrying` per
            // its retry policy and re-executes the send from the abandoned
            // `Start` on replay) instead of surfacing as a guest-visible
            // error value. Permanent failures — and transient ones whose
            // retry budget is exhausted — fall through and are recorded and
            // returned to the guest, which is also what a recorded error
            // replays as.
            if classify_serializable_http_error_code(&serialize_error_code(&error_code))
                == HostFailureKind::Transient
            {
                let properties = http_retry_properties(
                    store,
                    &retry_method,
                    &retry_uri,
                    None,
                    "transient",
                    &serialized_request.method,
                );
                let mut retry_task_ctx = make_p3_http_retry_task_context::<Ctx, U>(
                    store,
                    handle.start_index(),
                    properties.clone(),
                )
                .await;
                let failure = anyhow::Error::new(ClassifiedHostError {
                    kind: HostFailureKind::Transient,
                    message: error_code.to_string(),
                });
                try_trigger_host_trap_retry(&mut retry_task_ctx, failure, properties)
                    .await
                    .map_err(|err| {
                        HttpError::trap(wasmtime::Error::from_anyhow(handle.trap(err)))
                    })?;
            }

            let result = SerializableP3HttpClientSendResult::HttpError(serialized_error);
            handle
                .complete_access(
                    store,
                    durable_worker_ctx::<Ctx, U>,
                    HostResponseP3HttpClientSendResult { result },
                )
                .await
                .map_err(HttpError::trap)?;
            finish_p3_send_span::<Ctx, U>(store, &span)
                .await
                .map_err(HttpError::trap)?;
            // Spawns the demand-gated transmission recorder at a
            // deterministic point (right after the send `End` and the
            // span finish, mirrored by the replay arm above). The inner
            // send consumed the request even on failure, so a demanded
            // transmission result — `Ok(())` from the dropped I/O wiring,
            // or a deterministic body-validation error — still resolves.
            start_transmission_recording::<Ctx, U>(store, pending_transmission);
            Err(error_code.into())
        }
    }
}

pub(super) struct PhysicalSendHttpError {
    error_code: ErrorCode,
    final_transmission_tx: oneshot::Sender<Result<(), ErrorCode>>,
}

pub(super) enum PhysicalSendConversionError {
    HttpError(PhysicalSendHttpError),
    Trap(anyhow::Error),
}

pub(super) enum PhysicalSend<Ctx: WorkerCtx> {
    Replayable(PhysicalSendRequest<Ctx>),
}

pub(super) fn poison_p3_pooled_connection(connection: &Option<P3PooledConnection>) {
    if let Some(connection) = connection {
        connection.poison();
    }
}

pub(super) struct PhysicalSendRequest<Ctx: WorkerCtx> {
    pool: Option<wasmtime_wasi_http::HttpConnectionPool>,
    method: http::Method,
    uri: http::Uri,
    version: http::Version,
    headers: HeaderMap,
    body: DurableRequestBody,
    options: Option<wasmtime_wasi_http::p3::RequestOptions>,
    final_transmission_tx: oneshot::Sender<Result<(), ErrorCode>>,
    _phantom: PhantomData<fn() -> Ctx>,
}

impl<Ctx: WorkerCtx> PhysicalSendRequest<Ctx> {
    async fn attempt(
        &self,
    ) -> Result<
        (
            http::Response<UnsyncBoxBody<Bytes, ErrorCode>>,
            Box<dyn Future<Output = Result<(), ErrorCode>> + Send>,
            Option<P3PooledConnection>,
        ),
        ErrorCode,
    > {
        let body = self.body.replayer().boxed_unsync();
        let mut request = http::Request::builder()
            .method(self.method.clone())
            .uri(self.uri.clone())
            .version(self.version)
            .body(body)
            .map_err(|err| ErrorCode::InternalError(Some(err.to_string())))?;
        *request.headers_mut() = self.headers.clone();

        match &self.pool {
            Some(pool) => pool
                .pooled_send_request_p3(request, self.options)
                .await
                .map(|(response, io, connection)| (response, io, Some(connection))),
            None => match wasmtime_wasi_http::p3::default_send_request(request, self.options).await
            {
                Ok((response, io)) => Ok((
                    response.map(http_body_util::BodyExt::boxed_unsync),
                    Box::new(io) as Box<dyn Future<Output = Result<(), ErrorCode>> + Send>,
                    None,
                )),
                Err(error) => Err(error),
            },
        }
    }
}

pub(super) async fn convert_physical_send_request<Ctx, U>(
    store: &Accessor<U, DurableP3<Ctx>>,
    req: Resource<Request>,
    send_start_index: OplogIndex,
) -> Result<PhysicalSend<Ctx>, PhysicalSendConversionError>
where
    Ctx: WorkerCtx,
    U: Send + 'static,
{
    let (final_transmission_tx, final_transmission_rx) = oneshot::channel();
    let (pool, oplog, recording_enabled) = store.with(|mut access| {
        let ctx = durable_worker_ctx::<Ctx, U>(access.data_mut());
        (
            ctx.wasi_http.connection_pool.clone(),
            ctx.state.oplog.clone(),
            ctx.state.snapshotting_mode.is_none()
                && ctx.state.persistence_level
                    != golem_common::model::oplog::PersistenceLevel::PersistNothing,
        )
    });
    let http_store = store.with_getter::<WasiHttp>(wasi_http_view::<Ctx, U>);
    let converted = http_store.with(|mut access| {
        let request = access
            .get()
            .table
            .delete(req)
            .context("failed to delete p3 HTTP request from table")
            .map_err(wasmtime::Error::from_anyhow)
            .map_err(HttpError::trap)?;
        let converted = request.into_http_with_getter(
            access.as_context_mut(),
            async move { final_transmission_rx.await.unwrap_or(Ok(())) },
            wasi_http_view::<Ctx, U>,
        )?;
        HttpResult::Ok(converted)
    });

    let (http_request, options) = match converted {
        Ok(converted) => converted,
        Err(error) => {
            if let Some(error_code) = error.downcast_ref().cloned() {
                return Err(PhysicalSendConversionError::HttpError(
                    PhysicalSendHttpError {
                        error_code,
                        final_transmission_tx,
                    },
                ));
            }
            return Err(PhysicalSendConversionError::Trap(anyhow::anyhow!(
                "failed to convert p3 HTTP request: {error:?}"
            )));
        }
    };

    let options = options.as_deref().copied();
    let (parts, body) = http_request.into_parts();
    Ok(PhysicalSend::Replayable(PhysicalSendRequest {
        pool,
        method: parts.method,
        uri: parts.uri,
        version: parts.version,
        headers: parts.headers,
        body: DurableRequestBody::new(body, oplog, send_start_index, recording_enabled),
        options,
        final_transmission_tx,
        _phantom: PhantomData,
    }))
}

pub(super) fn response_resource_from_http<Ctx, U>(
    store: &Accessor<U, DurableP3<Ctx>>,
    response: http::Response<UnsyncBoxBody<Bytes, ErrorCode>>,
    io: Box<dyn Future<Output = Result<(), ErrorCode>> + Send>,
    final_transmission_tx: oneshot::Sender<Result<(), ErrorCode>>,
) -> HttpResult<Resource<Response>>
where
    Ctx: WorkerCtx,
    U: Send + 'static,
{
    let (parts, body) = response.into_parts();
    let mut io = Box::into_pin(io);
    let body = match io.as_mut().poll(&mut Context::from_waker(Waker::noop())) {
        Poll::Ready(result) => {
            let _ = final_transmission_tx.send(result.clone());
            body
        }
        Poll::Pending => {
            let (io_result_tx, io_result_rx) = oneshot::channel();
            let io_task = tokio::task::spawn(async move {
                let result = io.await;
                debug!(?result, "p3 HTTP send I/O future finished");
                let _ = io_result_tx.send(result);
            });
            let io_guard = AbortOnDropIoTask(io_task);
            tokio::task::spawn(async move {
                let result = io_result_rx.await.unwrap_or(Ok(()));
                let _ = final_transmission_tx.send(result);
            });
            BodyWithState {
                body,
                _state: io_guard,
            }
            .boxed_unsync()
        }
    };
    let response = http::Response::from_parts(parts, body);
    let (response, response_io) = wasmtime_wasi_http::p3::Response::from_http(response);
    tokio::task::spawn(async move {
        let result = response_io.await;
        debug!(?result, "p3 HTTP response body I/O future finished");
    });
    let http_store = store.with_getter::<WasiHttp>(wasi_http_view::<Ctx, U>);
    http_store.with(|mut access| {
        access
            .get()
            .table
            .push(response)
            .context("failed to push p3 HTTP response to table")
            .map_err(wasmtime::Error::from_anyhow)
            .map_err(HttpError::trap)
    })
}

pub(super) async fn make_p3_http_retry_task_context<Ctx, U>(
    store: &Accessor<U, DurableP3<Ctx>>,
    retry_point: OplogIndex,
    mut retry_properties: RetryProperties,
) -> TaskRetryContext<Ctx>
where
    Ctx: WorkerCtx,
    U: Send + 'static,
{
    let (
        environment_state_service,
        environment_id,
        default_retry_policy,
        agent_config_retry_policies,
        runtime_retry_policy_mutations,
        max_in_function_retry_delay,
        worker,
    ) = store.with(|mut access| {
        let ctx = durable_worker_ctx::<Ctx, U>(access.data_mut());
        ctx.state.enrich_retry_properties(&mut retry_properties);
        (
            ctx.state.environment_state_service.clone(),
            ctx.state.owned_agent_id.environment_id,
            NamedRetryPolicy::default_from_config(&ctx.state.config.retry),
            ctx.state.agent_config_retry_policies(),
            ctx.state.runtime_retry_policy_mutations.clone(),
            ctx.state.config.max_in_function_retry_delay,
            ctx.public_state.worker(),
        )
    });
    let current_retry_policy_state = worker
        .get_non_detached_last_known_status()
        .await
        .current_retry_state
        .get(&retry_point)
        .cloned();

    TaskRetryContext {
        retry_point,
        environment_state_service,
        environment_id,
        default_retry_policy,
        agent_config_retry_policies,
        runtime_retry_policy_mutations,
        max_in_function_retry_delay,
        current_retry_policy_state,
        retry_properties,
        worker,
    }
}

pub(super) fn http_retry_properties<Ctx, U>(
    store: &Accessor<U, DurableP3<Ctx>>,
    method: &str,
    uri: &str,
    status_code: Option<u16>,
    error_type: &str,
    serialized_method: &SerializableHttpMethod,
) -> RetryProperties
where
    Ctx: WorkerCtx,
    U: Send + 'static,
{
    let mut properties = RetryContext::http_with_response(method, uri, status_code, error_type);
    let effective_idempotence = effective_method_idempotence::<Ctx, U>(store, serialized_method);
    apply_method_idempotence(&mut properties, effective_idempotence);
    properties
}

/// Effective idempotence of a request for retry decisions: the worker-level
/// `assume_idempotence` override, or an idempotent HTTP method.
pub(super) fn effective_method_idempotence<Ctx, U>(
    store: &Accessor<U, DurableP3<Ctx>>,
    serialized_method: &SerializableHttpMethod,
) -> bool
where
    Ctx: WorkerCtx,
    U: Send + 'static,
{
    store.with(|mut access| {
        let ctx = durable_worker_ctx::<Ctx, U>(access.data_mut());
        ctx.state.assume_idempotence || is_idempotent_http_method(serialized_method)
    })
}

pub(super) fn apply_method_idempotence(properties: &mut RetryProperties, is_idempotent: bool) {
    properties.set("is-idempotent", PredicateValue::Boolean(is_idempotent));
}

pub(super) fn inline_retry_eligible_for_method<Ctx, U>(
    store: &Accessor<U, DurableP3<Ctx>>,
    method: &SerializableHttpMethod,
) -> bool
where
    Ctx: WorkerCtx,
    U: Send + 'static,
{
    store.with(|mut access| {
        let ctx = durable_worker_ctx::<Ctx, U>(access.data_mut());
        ctx.state.is_live()
            && ctx.state.snapshotting_mode.is_none()
            && ctx.state.persistence_level
                != golem_common::model::oplog::PersistenceLevel::PersistNothing
            && ctx.state.active_atomic_regions.is_empty()
            && (ctx.state.assume_idempotence || is_idempotent_http_method(method))
    })
}

pub(super) async fn matching_status_retry_policy<Ctx, U>(
    store: &Accessor<U, DurableP3<Ctx>>,
    retry_ctx: &mut TaskRetryContext<Ctx>,
    method: &str,
    uri: &str,
    status_code: u16,
    serialized_method: &SerializableHttpMethod,
) -> Option<NamedRetryPolicy>
where
    Ctx: WorkerCtx,
    U: Send + 'static,
{
    let properties = http_retry_properties(
        store,
        method,
        uri,
        Some(status_code),
        "http-status",
        serialized_method,
    );
    let status_policies: Vec<NamedRetryPolicy> = retry_ctx
        .named_retry_policies()
        .await
        .into_iter()
        .filter(|policy| {
            policy.predicate.references_property("status-code")
                || policy.policy.references_property("status-code")
        })
        .collect();
    match NamedRetryPolicy::resolve_applicable_treating_missing_properties_as_no_match(
        &status_policies,
        &properties,
    ) {
        Ok(policy) => policy.cloned(),
        Err(error) => {
            tracing::warn!(?error, "Failed resolving p3 HTTP status retry policy");
            None
        }
    }
}

/// Host-side state of an open p3 HTTP response created by the durable
/// `client::send`, keyed by the response resource rep in
/// `open_p3_http_responses`. Taken over by the durable consume-body task in
/// `consume_body`, or cleaned up by the response `drop` when the body was
/// never consumed.
#[derive(Clone)]
pub(crate) struct P3HttpSendSpan {
    pub(crate) span_id: SpanId,
    pub(super) legacy_durable: bool,
}

/// Deterministically derives the span id of a p3 send's
/// `outgoing-http-request` span from the send's own host-call `Start` index,
/// which is stable across live execution and replay. Uses UUIDv5 (like
/// `IdempotencyKey::derived`) folded to 64 bits, so re-derived trace-context
/// headers are stable across executor versions too.
pub(super) fn derive_p3_send_span_id(
    owned_agent_id: &OwnedAgentId,
    start_index: OplogIndex,
) -> SpanId {
    let name = format!("{owned_agent_id}/outgoing-http-request/{start_index}");
    let (hi, lo) = Uuid::new_v5(&Uuid::NAMESPACE_OID, name.as_bytes()).as_u64_pair();
    let n = NonZeroU64::new(hi ^ lo).unwrap_or(NonZeroU64::new(u64::MAX).unwrap());
    SpanId(n)
}

/// A deterministic digest of a P3 send's serialized request head, used to discriminate the
/// send's batched-write scope name among concurrent sends. Every field is length-prefixed and
/// the headers are folded in sorted by name, so the digest does not depend on the `HashMap`
/// iteration order (which is process-random and therefore differs between the recording run and
/// a post-restart replay).
pub(super) fn p3_send_request_discriminator(
    request: &SerializableP3HttpClientSend,
) -> Result<String, String> {
    fn update(hasher: &mut blake3::Hasher, bytes: &[u8]) {
        hasher.update(&(bytes.len() as u64).to_le_bytes());
        hasher.update(bytes);
    }
    let mut hasher = blake3::Hasher::new();
    update(
        &mut hasher,
        &golem_common::serialization::serialize(&request.method)?,
    );
    update(
        &mut hasher,
        &golem_common::serialization::serialize(&request.scheme)?,
    );
    update(
        &mut hasher,
        &golem_common::serialization::serialize(&request.authority)?,
    );
    update(
        &mut hasher,
        &golem_common::serialization::serialize(&request.path_with_query)?,
    );
    let mut headers: Vec<(&String, &Vec<Vec<u8>>)> = request.headers.iter().collect();
    headers.sort_by_key(|(name, _)| name.as_str());
    for (name, values) in headers {
        update(&mut hasher, name.as_bytes());
        update(
            &mut hasher,
            &golem_common::serialization::serialize(values)?,
        );
    }
    update(
        &mut hasher,
        &golem_common::serialization::serialize(&request.options)?,
    );
    Ok(hasher.finalize().to_hex().to_string())
}

/// Finishes the send's `outgoing-http-request` span: in-memory only for
/// derived spans (no span oplog entries exist), durably (positional
/// `FinishSpan`) for spans replayed from legacy oplogs.
pub(super) async fn finish_p3_send_span<Ctx: WorkerCtx, U: 'static>(
    store: &Accessor<U, DurableP3<Ctx>>,
    span: &P3HttpSendSpan,
) -> Result<(), WorkerExecutorError> {
    if span.legacy_durable {
        finish_span_access(store, durable_worker_ctx::<Ctx, U>, &span.span_id).await
    } else {
        store.with(|mut access| {
            let ctx = durable_worker_ctx::<Ctx, U>(access.data_mut());
            finish_span_in_memory(ctx, &span.span_id)
        })
    }
}

/// Everything needed to re-issue a recorded p3 `client::send` after a restart
/// (the P3 counterpart of P2's `rebuild_request_after_replay`): the request
/// head + options recorded in the send's `Start` payload (reconstructed
/// deterministically from the guest-rebuilt request resource during replay)
/// and the Golem-managed headers (`traceparent`/`tracestate`,
/// `idempotency-key`) re-derived from recorded state — the replayed span and
/// the send's own `Start` index — so the re-issued request is byte-identical
/// to the original in every Golem-controlled aspect.
///
/// The request *body* is not recorded in the oplog, so only body-less requests
/// can be re-issued; see [`recorded_head_declares_body`].
pub(super) fn outgoing_http_request_uri(request: &SerializableP3HttpClientSend) -> String {
    let scheme = match request
        .scheme
        .as_ref()
        .unwrap_or(&SerializableP3HttpScheme::Https)
    {
        SerializableP3HttpScheme::Http => "http",
        SerializableP3HttpScheme::Https | SerializableP3HttpScheme::Other(_) => "https",
    };
    format!(
        "{}://{}{}",
        scheme,
        request.authority.as_deref().unwrap_or(""),
        request.path_with_query.as_deref().unwrap_or("")
    )
}

/// Span attributes for the `outgoing-http-request` invocation-context span,
/// mirroring the P2 `http::outgoing_handler::handle` span shape.
pub(super) fn outgoing_http_request_span_attributes(
    request: &SerializableP3HttpClientSend,
) -> Vec<(String, AttributeValue)> {
    let uri = outgoing_http_request_uri(request);
    vec![
        (
            "name".to_string(),
            AttributeValue::String("outgoing-http-request".to_string()),
        ),
        ("request.uri".to_string(), AttributeValue::String(uri)),
        (
            "request.method".to_string(),
            AttributeValue::String(request.method.to_string()),
        ),
    ]
}

/// Computes the Golem-managed headers to inject into an outgoing p3 HTTP
/// request, mirroring P2 (`http/outgoing_http.rs`): the trace-context headers
/// of the request's invocation span (when `forward_trace_context_headers` is
/// enabled) and an `idempotency-key` derived from the send's own host-call
/// `Start` index (when `set_outgoing_http_idempotency_key` is enabled and the
/// guest did not set the header itself). The `Start` index is stable across
/// live execution and replay, so a retried send reuses the same key.
/// `guest_headers` is the serialized request head used to detect a
/// guest-provided idempotency key.
pub(super) fn golem_outgoing_http_headers<Ctx: WorkerCtx, U: Send>(
    store: &Accessor<U, DurableP3<Ctx>>,
    span_id: &SpanId,
    start_index: OplogIndex,
    guest_headers: &HashMap<String, Vec<Vec<u8>>>,
) -> Result<Vec<(String, String)>, WorkerExecutorError> {
    store.with(|mut access| {
        let ctx = durable_worker_ctx::<Ctx, U>(access.data_mut());
        let mut headers = Vec::new();
        if ctx.state.forward_trace_context_headers {
            let invocation_context =
                ctx.state
                    .invocation_context
                    .get_stack(span_id)
                    .map_err(|err| {
                        WorkerExecutorError::runtime(format!(
                            "span {span_id} missing from the invocation context while injecting trace context headers: {err}"
                        ))
                    })?;
            let trace_context_headers =
                TraceContextHeaders::from_invocation_context(invocation_context);
            headers.extend(trace_context_headers.to_raw_headers_map());
        }
        if ctx.state.set_outgoing_http_idempotency_key
            && !guest_headers.contains_key("idempotency-key")
        {
            let idempotency_key = ctx.derive_idempotency_key(start_index);
            headers.push(("idempotency-key".to_string(), idempotency_key.to_string()));
        }
        Ok(headers)
    })
}

/// Applies the injected headers to the actual request resource (replacing any
/// existing values for the same names) so the network request carries them.
/// The guest constructed the request with immutable headers, so they are
/// briefly remarked mutable, mirroring the P2 injection.
pub(super) fn apply_headers_to_request_resource<Ctx: WorkerCtx, U: Send>(
    store: &Accessor<U, DurableP3<Ctx>>,
    req: &Resource<Request>,
    headers: &[(String, String)],
) -> Result<(), WorkerExecutorError> {
    if headers.is_empty() {
        return Ok(());
    }
    let http_store = store.with_getter::<WasiHttp>(wasi_http_view::<Ctx, U>);
    http_store.with(|mut access| {
        let view = access.get();
        let field_size_limit = view.ctx.field_size_limit;
        let request = view
            .table
            .get_mut(&Resource::<wasmtime_wasi_http::p3::Request>::new_borrow(
                req.rep(),
            ))
            .map_err(|err| {
                WorkerExecutorError::runtime(format!(
                    "failed to get outgoing p3 HTTP request from table: {err}"
                ))
            })?;
        request.headers.set_mutable(field_size_limit);
        let mut result = Ok(());
        for (name, value) in headers {
            let header_name = match HeaderName::try_from(name.as_str()) {
                Ok(name) => name,
                Err(err) => {
                    result = Err(WorkerExecutorError::runtime(format!(
                        "invalid injected header name {name}: {err}"
                    )));
                    break;
                }
            };
            let header_value = match HeaderValue::try_from(value.as_str()) {
                Ok(value) => value,
                Err(err) => {
                    result = Err(WorkerExecutorError::runtime(format!(
                        "invalid injected header value for {name}: {err}"
                    )));
                    break;
                }
            };
            let _ = request.headers.remove_all(header_name.clone());
            if let Err(err) = request.headers.append(header_name, header_value) {
                result = Err(WorkerExecutorError::runtime(format!(
                    "failed to inject header {name} into outgoing p3 HTTP request: {err:?}"
                )));
                break;
            }
        }
        request.headers.set_immutable();
        result
    })
}

pub(super) fn is_idempotent_http_method(method: &SerializableHttpMethod) -> bool {
    matches!(
        method,
        SerializableHttpMethod::Get
            | SerializableHttpMethod::Head
            | SerializableHttpMethod::Put
            | SerializableHttpMethod::Delete
            | SerializableHttpMethod::Options
            | SerializableHttpMethod::Trace
    )
}
