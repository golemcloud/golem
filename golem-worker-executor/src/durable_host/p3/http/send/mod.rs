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
use super::serialization::{serialize_error_code, serialize_request, serialize_response_headers};
use super::*;
use crate::durable_host::authorization::targets::http_target;
use crate::durable_host::concurrent::{
    AccessClaimOptions, AccessPermissionCheck, CallSpanObserver, Cancellable,
    DeferredCallReplayOutcome, DropPolicy, DurableCallSession, access_invocation_id,
    finish_span_in_memory,
};
use crate::durable_host::durability::{
    AsyncRetryDecision, ClassifiedHostError, DurabilityHost, HostFailureKind, InFunctionRetryHost,
    InFunctionRetryState, TaskRetryContext, semantic_trap_retry_override_error,
    try_trigger_host_trap_retry,
};
use crate::durable_host::http::policy::{
    apply_managed_http_headers, classify_serializable_http_error_code, golem_managed_http_headers,
    http_worker_state_allows_retry,
};
use crate::durable_host::p3::{DurableP3, DurableP3View, durable_worker_ctx, wasi_http_view};
use crate::services::HasWorker;
use crate::workerctx::WorkerCtx;
use anyhow::Context as _;
use bytes::Bytes;
use futures::future::{Either, select};
use golem_common::model::invocation_context::{AttributeValue, InvocationContextSpan, SpanId};
use golem_common::model::oplog::host_functions::P3HttpClientSend;
use golem_common::model::oplog::payload::types::{
    SerializableHttpMethod, SerializableP3HttpClientSend, SerializableP3HttpClientSendResult,
    SerializableP3HttpScheme,
};
use golem_common::model::oplog::{
    AttributeMap, DeniablePair, DurableFunctionType, HostRequestP3HttpClientSend,
    HostResponseP3HttpClientSendResult, OplogEntry, OplogIndex, SpanFinished, SpanKind,
    SpanStarted,
};
use golem_common::model::{NamedRetryPolicy, OwnedAgentId, RetryContext, RetryProperties};
use golem_service_base::error::worker_executor::WorkerExecutorError;
use http::HeaderMap;
use http_body_util::BodyExt as _;
use http_body_util::combinators::UnsyncBoxBody;
use std::collections::HashMap;
use std::future::Future;
use std::marker::PhantomData;
use std::num::NonZeroU64;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};
use tokio::sync::oneshot;
use tracing::debug;
use uuid::Uuid;
use wasmtime::AsContextMut;
use wasmtime::component::{Accessor, Resource, TerminalConsumption};
use wasmtime_wasi_http::P3PooledConnection;
use wasmtime_wasi_http::p3::WasiHttp;
use wasmtime_wasi_http::p3::bindings::http::client;
use wasmtime_wasi_http::p3::bindings::http::types::{ErrorCode, Request, Response};

mod headers;
mod observation;
mod retry;
mod transport;

pub(super) use headers::*;
pub(super) use observation::*;
pub(super) use retry::*;
pub(super) use transport::*;

impl<Ctx: WorkerCtx> client::Host for DurableP3View<'_, Ctx> {}

impl<U: Send + 'static, Ctx: WorkerCtx> client::HostWithStore<U> for DurableP3<Ctx> {
    async fn send(
        store: &Accessor<U, Self>,
        req: Resource<Request>,
    ) -> HttpResult<Resource<Response>> {
        send_with_durability::<Ctx, U, Cancellable>(store, req, DurableFunctionType::WriteRemote)
            .await
    }
}

pub(super) async fn send_with_durability<Ctx, U, P>(
    store: &Accessor<U, DurableP3<Ctx>>,
    mut req: Resource<Request>,
    function_type: DurableFunctionType,
) -> HttpResult<Resource<Response>>
where
    Ctx: WorkerCtx,
    U: Send + 'static,
    P: DropPolicy,
{
    // The body transmission recorded after the send carries the same invocation id.
    let invocation_id =
        access_invocation_id(store, durable_worker_ctx::<Ctx, U>).map_err(HttpError::trap)?;

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
    let mut pending_transmission = store.with(|mut access| {
        let ctx = durable_worker_ctx::<Ctx, U>(access.data_mut());
        ctx.state
            .pending_p3_http_request_transmissions
            .remove(&req.rep())
    });

    // The request head is serialized (and recorded) exactly as the guest built
    // it. The send's `outgoing-http-request` invocation-context span is
    // *derived*: its span id is a deterministic function of the send's own
    // host-call `Start` index, so no separate `StartSpan`/`FinishSpan` entries
    // are written or consumed — positional span entries are unsound under
    // concurrent sends.
    //
    // The Golem-managed headers (`traceparent`/`tracestate` and the derived
    // `idempotency-key`) are injected into the request *resource* only, after
    // the host-call `Start` is written/claimed: like on P2 they are not part of
    // the recorded head, because they are deterministic functions of recorded
    // state — the trace context of the send span (same derived/recorded span
    // id) and the idempotency key derived from the call's own `Start` index,
    // which is stable across live execution and replay.
    let serialized_request =
        serialize_request::<Ctx, U>(store, borrow_resource(&req)).map_err(|err| {
            HttpError::trap(WorkerExecutorError::runtime(format!(
                "failed to serialize outgoing p3 HTTP request: {err}"
            )))
        })?;
    // Concurrent sends are told apart by their invocation id.
    let host_request = HostRequestP3HttpClientSend {
        request: serialized_request.clone(),
    };
    // Initiation evaluates these targets only for a new claim; historical calls use
    // their recorded permission decision, including when today's target is invalid.
    let targets = http_target(&outgoing_http_request_uri(&serialized_request))
        .map(|target| vec![target.permission])
        .map_err(|error| error.to_string());
    let (permission_check, permission_decision) = AccessPermissionCheck::new(targets);
    let claim_options = AccessClaimOptions {
        parent_start_index: None,
        observational_owner: None,
        invocation_id,
        permission_check: Some(permission_check),
        ..Default::default()
    };
    let span_seed = store.with(|mut access| {
        let ctx = durable_worker_ctx::<Ctx, U>(access.data_mut());
        (
            ctx.state.owned_agent_id.clone(),
            ctx.state.invocation_context.trace_id.clone(),
            ctx.state.invocation_context.trace_states.clone(),
            ctx.state.current_span_id.clone(),
        )
    });
    let span_request = serialized_request.clone();
    let persisted = store.with(|mut access| {
        !durable_worker_ctx::<Ctx, U>(access.data_mut())
            .state
            .durability_is_suppressed()
    });
    let cleanup_recorder = store.with(|mut access| {
        crate::durable_host::concurrent::HttpSpanCleanupRecorder::new(
            durable_worker_ctx::<Ctx, U>(access.data_mut()),
            OplogIndex::INITIAL,
            persisted,
        )
    });
    let lifecycle = Arc::new(P3SendSpanLifecycle::new(cleanup_recorder));
    let send_terminal =
        SendTerminalObserver::register(store, lifecycle.clone()).map_err(HttpError::trap)?;
    let span_started_at = golem_common::model::Timestamp::now_utc();
    // Start admission itself can await a replay claim while the guest drops this send.
    // Keep request cleanup owned by the Store until either replay or live send takes it.
    let (disarm_leak_guard_tx, disarm_leak_guard_rx) = oneshot::channel();
    spawn_replayed_request_leak_guard::<Ctx, U>(store, req.rep(), disarm_leak_guard_rx);
    let quota_request = host_request.clone();
    let mut handle =
        DurableCallSession::<P3HttpClientSend, P>::start_access_with_options_and_indexed_span(
            store,
            durable_worker_ctx::<Ctx, U>,
            function_type.clone(),
            claim_options,
            move |start, store| {
                Box::pin(async move {
                    if start.is_live && !start.is_denied {
                        store.with(|mut access| {
                            let ctx = durable_worker_ctx::<Ctx, U>(access.data_mut());
                            ctx.state
                                .check_and_increment_http_call_count()
                                .map_err(wasmtime::Error::from)?;
                            ctx.record_monthly_http_call()
                                .map_err(wasmtime::Error::from_anyhow)
                        })?;
                    }
                    Ok(quota_request)
                })
            },
            move |start_index| {
                let (owned_agent_id, trace_id, trace_states, parent_span_id) = span_seed;
                Ok(SpanStarted {
                    span_id: derive_p3_send_span_id(&owned_agent_id, start_index),
                    trace_id,
                    trace_states,
                    parent_span_id: Some(parent_span_id),
                    links: Vec::new(),
                    started_at: span_started_at,
                    attributes: AttributeMap(
                        outgoing_http_request_span_attributes(&span_request)
                            .into_iter()
                            .collect(),
                    ),
                    kind: SpanKind::Client,
                })
            },
            lifecycle.clone(),
        )
        .await
        .map_err(HttpError::trap)?;
    let send_start_index = handle.start_index();
    let observational_owner = handle.observational_owner();
    let decision = permission_decision.await.map_err(|_| {
        HttpError::trap(WorkerExecutorError::runtime(
            "p3 HTTP send permission decision channel closed",
        ))
    })?;
    let denied = match decision {
        Some(Err(denial)) => Some(P3HttpClientSend::denied(&host_request, &denial)),
        Some(Ok(_)) | None => None,
    };

    let span_started = if handle.is_persisted() {
        lifecycle.started_span().ok_or_else(|| {
            HttpError::trap(WorkerExecutorError::runtime(
                "p3 HTTP send Start has no span_started transition",
            ))
        })?
    } else {
        let ctx = store.with(|mut access| {
            let ctx = durable_worker_ctx::<Ctx, U>(access.data_mut());
            (
                ctx.state.owned_agent_id.clone(),
                ctx.state.invocation_context.trace_id.clone(),
                ctx.state.invocation_context.trace_states.clone(),
                ctx.state.current_span_id.clone(),
            )
        });
        SpanStarted {
            span_id: derive_p3_send_span_id(&ctx.0, send_start_index),
            trace_id: ctx.1,
            trace_states: ctx.2,
            parent_span_id: Some(ctx.3),
            links: Vec::new(),
            started_at: span_started_at,
            attributes: AttributeMap(
                outgoing_http_request_span_attributes(&serialized_request)
                    .into_iter()
                    .collect(),
            ),
            kind: SpanKind::Client,
        }
    };
    if !handle.is_persisted() {
        lifecycle.snapshot_started(send_start_index, span_started.clone());
    }
    let span = store
        .with(|mut access| {
            install_p3_send_span(
                durable_worker_ctx::<Ctx, U>(access.data_mut()),
                &span_started,
            )?;
            Ok::<_, WorkerExecutorError>(P3HttpSendSpan {
                span_id: span_started.span_id.clone(),
                send_start_index,
                persisted: handle.is_persisted(),
                closure_claimed: lifecycle.closure_claimed.clone(),
            })
        })
        .map_err(HttpError::trap)?;
    handle.close_span_on_cancellation(span.span_id.clone());

    let mut restored_body = None;
    let mut restored_transmission = None;
    if !handle.is_live() {
        // Intake cannot wait for send resolution: the guest writer can issue another durable
        // call after its last write, whose retained completion must advance the replay cursor
        // before this send can be classified as incomplete.
        let _ = disarm_leak_guard_tx.send(());
        let replayed_request_body_drain = consume_replayed_request::<Ctx, U>(
            store,
            req,
            Some(ReplayedRequestBodyRecording {
                send_start_index,
                trap_context: handle.trap_context(),
                recording_complete_at_end: false,
            }),
        )
        .await
        .map_err(|error| HttpError::trap(wasmtime::Error::from_anyhow(handle.trap(error))))?;
        if let Some(drain) = &replayed_request_body_drain {
            let progress = drain.progress.clone();
            handle.require_incomplete_repair_readiness(
                async move { progress.recording_ready().await },
            );
        }
        let trap_context = handle.trap_context();
        match handle
            .replay_access_deferred(store, durable_worker_ctx::<Ctx, U>)
            .await
            .map_err(|error| {
                HttpError::trap(wasmtime::Error::from_anyhow(
                    crate::durable_host::durability::mark_durable_call_trap_context(
                        error.into(),
                        trap_context,
                    ),
                ))
            })? {
            DeferredCallReplayOutcome::Replayed(response, delivery) => {
                let recorded_status = match &response.result {
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
                                span: span.clone(),
                                method: serialized_request.method.to_string(),
                                uri: outgoing_http_request_uri(&serialized_request),
                                resend: Some(P3HttpSendRebuild {
                                    request: serialized_request,
                                    injected_headers,
                                    recorded_status,
                                    recorded_request_body: send_start_index,
                                    durable_body: None,
                                    replayed_request_body_drain,
                                }),
                                body_is_placeholder: true,
                                observational_owner,
                            },
                        );
                        // Spawns the demand-gated transmission recorder
                        // at the same point as the live path, so a demanded
                        // recording's `Start` is claimed where it was appended.
                        start_transmission_recording::<Ctx, U>(
                            store,
                            pending_transmission,
                            observational_owner,
                            invocation_id,
                        );
                        if delivery.is_replay_discarded() {
                            // The recorded run persisted this send's `End` but the guest
                            // dropped the send future before it returned. The recorded
                            // continuation is mirrored above (request consumed, response
                            // bookkeeping, recorder spawned); park instead of returning the
                            // response, so the deterministic guest drops this future at the
                            // same point it did live.
                            send_terminal
                                .attach(store, delivery)
                                .await
                                .map_err(HttpError::trap)?;
                            std::future::pending::<()>().await;
                            unreachable!("std::future::pending never completes")
                        }
                        send_terminal
                            .attach(store, delivery)
                            .await
                            .map_err(HttpError::trap)?;
                        return Ok(response);
                    }
                    Err(error) => {
                        // A recorded send error closed the span at its `End`;
                        // apply the same in-memory transition on replay.
                        finish_p3_send_span::<Ctx, U>(store, &span)
                            .await
                            .map_err(HttpError::trap)?;
                        // Spawns the demand-gated transmission recorder
                        // at the same point as the live path, so a demanded
                        // recording's `Start` is claimed where it was appended.
                        start_transmission_recording::<Ctx, U>(
                            store,
                            pending_transmission,
                            observational_owner,
                            invocation_id,
                        );
                        if delivery.is_replay_discarded() {
                            // The recorded run persisted this send's `End` but the guest
                            // dropped the send future before the error was returned. The
                            // recorded continuation is mirrored above; park instead of
                            // returning the error.
                            std::future::pending::<()>().await;
                            unreachable!("std::future::pending never completes")
                        }
                        span.claim_closure();
                        send_terminal
                            .attach(store, delivery)
                            .await
                            .map_err(HttpError::trap)?;
                        return Err(error);
                    }
                }
            }
            DeferredCallReplayOutcome::Incomplete(live_handle) => {
                handle = live_handle;
                let continuation = replayed_request_body_drain
                    .as_ref()
                    .and_then(|drain| drain.progress.take_continuation());
                let mut rebuild = P3HttpSendRebuild {
                    request: serialized_request.clone(),
                    injected_headers: Vec::new(),
                    recorded_status: 0,
                    recorded_request_body: send_start_index,
                    durable_body: None,
                    replayed_request_body_drain,
                };
                let recovery = RequestBodyRecovery::default();
                if recorded_request_body_replayability(
                    store,
                    &mut rebuild,
                    recovery.clone(),
                    continuation.is_some(),
                )
                .await
                .is_err()
                {
                    if let Some(error) = recovery.error() {
                        return Err(HttpError::trap(wasmtime::Error::from_anyhow(
                            handle.trap(error),
                        )));
                    }
                    let error_code = ErrorCode::HttpResponseIncomplete;
                    let (_, delivery) = handle
                        .complete_access_deferred_with_span(
                            store,
                            durable_worker_ctx::<Ctx, U>,
                            HostResponseP3HttpClientSendResult {
                                result: SerializableP3HttpClientSendResult::HttpError(
                                    serialize_error_code(&error_code),
                                ),
                            },
                            p3_send_span_finished(
                                &span,
                                golem_common::model::oplog::SpanOutcome::Failed,
                            ),
                        )
                        .await
                        .map_err(HttpError::trap)?;
                    if let Err(error) = finish_p3_send_span_in_memory::<Ctx, U>(store, &span) {
                        delivery.suppress();
                        return Err(HttpError::trap(error));
                    }
                    start_transmission_recording::<Ctx, U>(
                        store,
                        pending_transmission,
                        observational_owner,
                        invocation_id,
                    );
                    span.claim_closure();
                    send_terminal
                        .attach(store, delivery)
                        .await
                        .map_err(HttpError::trap)?;
                    return Err(error_code.into());
                }
                let oplog = store.with(|mut access| {
                    durable_worker_ctx::<Ctx, U>(access.data_mut())
                        .state
                        .oplog
                        .clone()
                });
                let scan = scan_recorded_request_body_frames(oplog.clone(), send_start_index)
                    .await
                    .map_err(|error| {
                        let error = error
                            .recovery_error()
                            .unwrap_or_else(|| WorkerExecutorError::runtime(error.to_string()));
                        HttpError::trap(wasmtime::Error::from_anyhow(handle.trap(error)))
                    })?;
                let recording_tasks = store.with(|mut access| {
                    durable_worker_ctx::<Ctx, U>(access.data_mut())
                        .public_state
                        .worker()
                        .runtime_append_tasks()
                });
                let continuation = match continuation {
                    Some(continuation) => {
                        restored_transmission = continuation.transmission;
                        continuation.body
                    }
                    None => http_body_util::Empty::new()
                        .map_err(|never| match never {})
                        .boxed_unsync(),
                };
                restored_body = Some(DurableRequestBody::from_recording(
                    continuation,
                    oplog,
                    send_start_index,
                    recording_tasks,
                    scan,
                    recovery,
                ));
                let request = build_rebuilt_request(
                    &rebuild,
                    &[],
                    restored_body.as_ref().unwrap().replayer().boxed_unsync(),
                )
                .map_err(|error| {
                    HttpError::trap(wasmtime::Error::from_anyhow(
                        handle.trap(WorkerExecutorError::runtime(error)),
                    ))
                })?;
                req = store
                    .with_getter::<WasiHttp>(wasi_http_view::<Ctx, U>)
                    .with(|mut access| access.get().table.push(request))
                    .map_err(|error| {
                        HttpError::trap(wasmtime::Error::from_anyhow(handle.trap(error)))
                    })?;
            }
        }
    } else {
        let _ = disarm_leak_guard_tx.send(());
    }

    if let Some(denied) = denied {
        let _ = consume_replayed_request::<Ctx, U>(store, req, None).await?;
        let (response, delivery) = handle
            .complete_access_deferred_with_span(
                store,
                durable_worker_ctx::<Ctx, U>,
                denied,
                p3_send_span_finished(&span, golem_common::model::oplog::SpanOutcome::Denied),
            )
            .await
            .map_err(HttpError::trap)?;
        if delivery.is_live_armed() {
            if let Err(error) = finish_p3_send_span_in_memory::<Ctx, U>(store, &span) {
                delivery.suppress();
                return Err(HttpError::trap(error));
            }
        } else {
            finish_p3_send_span::<Ctx, U>(store, &span)
                .await
                .map_err(HttpError::trap)?;
        }
        start_transmission_recording::<Ctx, U>(
            store,
            pending_transmission,
            observational_owner,
            invocation_id,
        );
        span.claim_closure();
        send_terminal
            .attach(store, delivery)
            .await
            .map_err(HttpError::trap)?;
        return replay_send_response::<Ctx, U>(store, response.result);
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
    let inline_retry_eligible = !handle.is_observational()
        && inline_retry_eligible_for_method::<Ctx, U>(store, &serialized_request.method);

    let recording_enabled = store.with(|mut access| {
        !durable_worker_ctx::<Ctx, U>(access.data_mut())
            .state
            .durability_is_suppressed()
    });
    let converted = match convert_physical_send_request::<Ctx, U>(
        store,
        req,
        handle.start_index(),
        recording_enabled,
        restored_body,
        restored_transmission,
    )
    .await
    {
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
            // The guest-visible error return below is the real delivery boundary: record
            // the terminal through the deferred API so a future torn between the `End` and
            // the return records the discard instead of replay redelivering the error.
            let (_, delivery) = handle
                .complete_access_deferred_with_span(
                    store,
                    durable_worker_ctx::<Ctx, U>,
                    HostResponseP3HttpClientSendResult { result },
                    p3_send_span_finished(&span, golem_common::model::oplog::SpanOutcome::Failed),
                )
                .await
                .map_err(HttpError::trap)?;
            if delivery.is_live_armed() {
                // Only the synchronous in-memory span finish remains; everything up to
                // the return leaves no tear window.
                if let Err(error) = finish_p3_send_span_in_memory::<Ctx, U>(store, &span) {
                    // The error is observed by the caller (the worker traps): not a
                    // silent discard, so no marker.
                    delivery.suppress();
                    return Err(HttpError::trap(error));
                }
                start_transmission_recording::<Ctx, U>(
                    store,
                    pending_transmission,
                    observational_owner,
                    invocation_id,
                );
                // The guest-visible error return below still crosses Wasmtime's lowering
                // and terminal-consumption boundary: hand the token to the terminal
                // observer instead of consuming it here.
                span.claim_closure();
                send_terminal
                    .attach(store, delivery)
                    .await
                    .map_err(HttpError::trap)?;
            } else {
                // Unpersisted live call (snapshotting): the original span handling
                // applies.
                finish_p3_send_span::<Ctx, U>(store, &span)
                    .await
                    .map_err(HttpError::trap)?;
                start_transmission_recording::<Ctx, U>(
                    store,
                    pending_transmission,
                    observational_owner,
                    invocation_id,
                );
                delivery.delivered();
            }
            return Err(error_code.into());
        }
    };
    let PhysicalSend::Replayable(physical) = converted;
    if let Some(pending) = pending_transmission.as_mut() {
        pending.recovery = physical.body.recovery.clone();
    }
    let mut retry_state = InFunctionRetryState::new();
    let mut retry_task_ctx = make_p3_http_retry_task_context::<Ctx, U>(
        store,
        handle.trap_context().retry_from,
        RetryContext::http(&retry_method, &retry_uri),
    )
    .await;

    let mut send_semantic_override = None;
    let send_result = loop {
        let interrupt = store.with(|mut access| {
            durable_worker_ctx::<Ctx, U>(access.data_mut()).create_interrupt_signal()
        });
        let send = physical.attempt();
        let attempt_result = match select(Box::pin(send), interrupt).await {
            Either::Left((result, _)) => result,
            Either::Right((interrupt_kind, _)) => {
                // An interrupt is not a cancellation: abandon the send call so its `Start` stays
                // incomplete on disk (re-executed on resume) instead of letting the handle drop
                // enqueue a spurious `Cancelled` terminal, and propagate the kind directly so it
                // classifies as `TrapType::Interrupt`.
                handle.abandon_for_trap();
                let error: anyhow::Error = interrupt_kind.into();
                return Err(HttpError::trap(wasmtime::Error::from_anyhow(error)));
            }
        };

        if let Some(error) = physical.body.recovery.error() {
            return Err(HttpError::trap(wasmtime::Error::from_anyhow(
                handle.trap(error),
            )));
        }
        match attempt_result {
            Ok((response, io, pooled_connection)) => {
                // Observational calls cannot retry inline, but must still honour a matching
                // status-code policy through the worker's trap-based retry path.
                if handle.is_observational()
                    && store.with(|mut access| {
                        let ctx = durable_worker_ctx::<Ctx, U>(access.data_mut());
                        ctx.state.is_live() && !ctx.state.snapshotting_mode
                    })
                    && matching_status_retry_policy(
                        store,
                        &mut retry_task_ctx,
                        &retry_method,
                        &retry_uri,
                        response.status().as_u16(),
                        &serialized_request.method,
                    )
                    .await
                    .is_some()
                {
                    let status = response.status().as_u16();
                    // The rejected response's connection may hold undrained bytes; keep the
                    // pool from reusing it (same as the inline-retry and p2 status paths).
                    poison_p3_pooled_connection(&pooled_connection);
                    let properties = http_retry_properties(
                        store,
                        &retry_method,
                        &retry_uri,
                        Some(status),
                        "http-status",
                        &serialized_request.method,
                    );
                    let for_retry: Result<(), String> = Err(format!(
                        "HTTP status {status} matched a status-code retry policy in an observational call"
                    ));
                    handle
                        .try_trigger_retry_access(
                            store,
                            durable_worker_ctx::<Ctx, U>,
                            &for_retry,
                            |_| HostFailureKind::Transient,
                            properties,
                        )
                        .await
                        .map_err(|err| HttpError::trap(wasmtime::Error::from_anyhow(err)))?;
                    // The policy gave up (retry budget exhausted): expose the rejected
                    // response to the guest as-is.
                    break Ok((response, io));
                }
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
                        AsyncRetryDecision::FallBackToTrap(semantic_override) => {
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
                            let trap = if let Some(payload) = semantic_override {
                                Some(anyhow::Error::new(
                                    crate::durable_host::durability::SemanticTrapRetryOverrideMarker {
                                        payload,
                                        inner: failure,
                                    },
                                ))
                            } else {
                                try_trigger_host_trap_retry(
                                    &mut retry_task_ctx,
                                    failure,
                                    properties,
                                )
                                .await
                                .err()
                            };
                            if let Some(err) = trap {
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
                    AsyncRetryDecision::FallBackToTrap(semantic_override) => {
                        send_semantic_override = semantic_override;
                        break Err(error_code);
                    }
                    AsyncRetryDecision::Exhausted => {
                        break Err(error_code);
                    }
                }
            }
            Err(error_code) => break Err(error_code),
        }
    };

    // Recording the request body was refused: the send failed because this agent's shard has a
    // new owner, which traps as the lost shard instead of reaching the guest as a request error.
    if send_result.is_err()
        && let Some(fence) = physical.body.recording_fence()
    {
        return Err(HttpError::trap(WorkerExecutorError::from(
            crate::services::oplog::OplogError::Fenced(fence),
        )));
    }

    match send_result {
        Ok((http_response, io)) => {
            let response_status = http_response.status().as_u16();
            let send_start_index = handle.start_index();
            let response = response_resource_from_http::<Ctx, U>(
                store,
                http_response,
                io,
                physical.final_transmission_tx,
                physical.body.recovery.clone(),
            )?;
            let headers = serialize_response_headers::<Ctx, U>(store, borrow_resource(&response))?;
            // When the request body was recorded as durable frames, the
            // recorded result says so, letting replay self-heal an
            // interrupted recording and letting a restart rebuild resend the
            // body from the oplog. `recording_complete_at_end` is a scan
            // shortcut only: frame appends run concurrently with the send, so
            // the recording may still complete after this point (its terminal
            // frame just lands later in the oplog).
            let result = SerializableP3HttpClientSendResult::SuccessWithRecordedRequestBody {
                headers,
                recording_complete_at_end: physical.body.recording_complete(),
            };
            // The guest-visible `Ok(response)` return below is the real delivery boundary:
            // record the terminal through the deferred API so a future torn between the `End`
            // and the return records the discard instead of replay redelivering the response.
            // The span stays open with the response, so there is no post-`End` entry.
            let (_, delivery) = handle
                .complete_access_deferred(
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
                    span: span.clone(),
                    method: retry_method.clone(),
                    uri: retry_uri.clone(),
                    resend: Some(P3HttpSendRebuild {
                        request: serialized_request,
                        injected_headers,
                        recorded_status: response_status,
                        recorded_request_body: send_start_index,
                        durable_body: Some(physical.body.clone()),
                        replayed_request_body_drain: None,
                    }),
                    body_is_placeholder: false,
                    observational_owner,
                },
            );
            // Spawns the demand-gated transmission recorder at a
            // deterministic point (right after the send `End`, mirrored by the
            // replay arm above): if the guest reads the transmission future,
            // the result is recorded here; otherwise no entries are written.
            start_transmission_recording::<Ctx, U>(
                store,
                pending_transmission,
                observational_owner,
                invocation_id,
            );
            // Everything since the `End` was synchronous, so no tear window remains between
            // here and handing the token to Wasmtime's terminal observer, which settles it
            // when the guest actually consumes (or discards) the lowered response.
            send_terminal
                .attach(store, delivery)
                .await
                .map_err(HttpError::trap)?;
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
                if let Some(semantic_override) = send_semantic_override.take() {
                    let failure = semantic_trap_retry_override_error(
                        semantic_override,
                        HostFailureKind::Transient,
                        error_code.to_string(),
                    );
                    return Err(HttpError::trap(wasmtime::Error::from_anyhow(
                        handle.trap(failure),
                    )));
                } else {
                    let mut retry_task_ctx = make_p3_http_retry_task_context::<Ctx, U>(
                        store,
                        handle.trap_context().retry_from,
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
            }

            let result = SerializableP3HttpClientSendResult::HttpError(serialized_error);
            // The guest-visible error return below is the real delivery boundary: record the
            // terminal through the deferred API so a future torn between the `End` and the
            // return records the discard instead of replay redelivering the error.
            let (_, delivery) = handle
                .complete_access_deferred_with_span(
                    store,
                    durable_worker_ctx::<Ctx, U>,
                    HostResponseP3HttpClientSendResult { result },
                    p3_send_span_finished(&span, golem_common::model::oplog::SpanOutcome::Failed),
                )
                .await
                .map_err(HttpError::trap)?;
            if delivery.is_live_armed() {
                // Only the synchronous in-memory span finish remains; everything up to the
                // return leaves no tear window.
                if let Err(error) = finish_p3_send_span_in_memory::<Ctx, U>(store, &span) {
                    // The error is observed by the caller (the worker traps): not a silent
                    // discard, so no marker.
                    delivery.suppress();
                    return Err(HttpError::trap(error));
                }
            } else {
                // Unpersisted live call (snapshotting): the original span handling applies.
                finish_p3_send_span::<Ctx, U>(store, &span)
                    .await
                    .map_err(HttpError::trap)?;
            }
            // Spawns the demand-gated transmission recorder at a
            // deterministic point (right after the send `End` and the
            // span finish, mirrored by the replay arm above). The inner
            // send consumed the request even on failure, so a demanded
            // transmission result — `Ok(())` from the dropped I/O wiring,
            // or a deterministic body-validation error — still resolves.
            start_transmission_recording::<Ctx, U>(
                store,
                pending_transmission,
                observational_owner,
                invocation_id,
            );
            // The guest-visible error return below still crosses Wasmtime's lowering and
            // terminal-consumption boundary: hand the token to the terminal observer instead
            // of consuming it here.
            span.claim_closure();
            send_terminal
                .attach(store, delivery)
                .await
                .map_err(HttpError::trap)?;
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

/// Host-side state of an open p3 HTTP response created by the durable
/// `client::send`, keyed by the response resource rep in
/// `open_p3_http_responses`. Taken over by the durable consume-body task in
/// `consume_body`, or cleaned up by the response `drop` when the body was
/// never consumed.
#[derive(Clone)]
pub(crate) struct P3HttpSendSpan {
    pub(crate) span_id: SpanId,
    pub(crate) persisted: bool,
    /// The send's own host-call `Start` index. Unlike the span id (which is derived from the
    /// owning agent's id and therefore changes when the oplog is forked to another agent), the
    /// `Start` index is part of the recorded oplog itself and survives fork/revert unchanged.
    pub(crate) send_start_index: OplogIndex,
    closure_claimed: Arc<AtomicBool>,
}

struct SendTerminalObserver {
    state: Arc<Mutex<SendTerminalState>>,
    registered: bool,
}

struct P3SendSpanLifecycle {
    state: Mutex<P3SendSpanLifecycleState>,
    closure_claimed: Arc<AtomicBool>,
}

struct P3SendSpanLifecycleState {
    recorder: crate::durable_host::concurrent::HttpSpanCleanupRecorder,
    started: Option<SpanStarted>,
    abandonment: Option<golem_common::model::Timestamp>,
    cleanup_submitted: bool,
}

struct SendTerminalState {
    consumption: Option<TerminalConsumption>,
    delivery: Option<crate::durable_host::concurrent::CompletionDelivery>,
}

#[cfg(test)]
#[path = "observation/tests.rs"]
mod terminal_tests;
