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

use crate::durable_host::concurrent::{
    AccessStartContext, CallHandle, CallReplayOutcome, Cancellable, DropEvent, DropPolicy,
    LeaveIncompleteOnDrop, NotCancellable, finish_span_access, start_span_access,
};
use crate::durable_host::durability::{
    DurabilityHost, DurableCallTrapContext, mark_durable_call_trap_context,
};
use crate::durable_host::http::types::classify_serializable_http_error_code;
use crate::durable_host::p3::{
    DurableP3, DurableP3View, durable_worker_ctx, observe_function_call,
    observe_function_call_store, wasi_http_view,
};
use crate::workerctx::WorkerCtx;
use anyhow::Context as _;
use bytes::Bytes;
use futures::future::{Either, select};
use golem_common::model::RetryContext;
use golem_common::model::invocation_context::{AttributeValue, SpanId};
use golem_common::model::oplog::host_functions::{
    P3HttpClientConsumeBody, P3HttpClientConsumeBodyChunk, P3HttpClientRequestBodyTransmission,
    P3HttpClientSend,
};
use golem_common::model::oplog::payload::types::{
    SerializableDnsErrorPayload, SerializableFieldSizePayload, SerializableHttpErrorCode,
    SerializableHttpMethod, SerializableP3HttpBodyChunk, SerializableP3HttpClientSend,
    SerializableP3HttpClientSendResult, SerializableP3HttpConsumeBodyResult,
    SerializableP3HttpRequestOptions, SerializableP3HttpScheme, SerializableResponseHeaders,
    SerializableTlsAlertReceivedPayload,
};
use golem_common::model::oplog::{
    DurableFunctionType, HostRequestNoInput, HostRequestP3HttpClientSend,
    HostResponseP3HttpClientConsumeBodyChunk, HostResponseP3HttpClientConsumeBodyResult,
    HostResponseP3HttpClientRequestBodyTransmission, HostResponseP3HttpClientSendResult,
    OplogIndex,
};
use golem_service_base::error::worker_executor::WorkerExecutorError;
use golem_service_base::headers::TraceContextHeaders;
use http::{HeaderMap, HeaderName, HeaderValue};
use http_body_util::BodyExt as _;
use http_body_util::Empty;
use http_body_util::combinators::UnsyncBoxBody;
use std::collections::HashMap;
use std::marker::PhantomData;
use std::pin::Pin;
use std::task::{Context, Poll};
use tokio::sync::{mpsc, oneshot};
use tracing::debug;
use wasmtime::component::{
    Access, Accessor, AccessorTask, Destination, FutureConsumer, FutureProducer, FutureReader,
    Resource, Source, StreamProducer, StreamReader, StreamResult,
};
use wasmtime::{AsContextMut, StoreContextMut};
use wasmtime_wasi::TrappableError;
use wasmtime_wasi_http::FieldMap;
use wasmtime_wasi_http::p3::bindings::clocks::monotonic_clock::Duration;
use wasmtime_wasi_http::p3::bindings::http::types::{
    ErrorCode, FieldName, FieldValue, Fields, Headers, Method, Request, RequestOptions, Response,
    Scheme, StatusCode, Trailers,
};
use wasmtime_wasi_http::p3::bindings::http::{client, types};
use wasmtime_wasi_http::p3::{HostBodyStreamProducer, WasiHttp, WasiHttpView};

type HttpError = TrappableError<ErrorCode>;
type HeaderError = TrappableError<types::HeaderError>;
type RequestOptionsError = TrappableError<types::RequestOptionsError>;

type HttpResult<T> = Result<T, HttpError>;
type HeaderResult<T> = Result<T, HeaderError>;
type RequestOptionsResult<T> = Result<T, RequestOptionsError>;

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

async fn send_with_durability<Ctx, U, P>(
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
    // it. Inside the two-phase start (see `CallHandle::start_access_with`) an
    // `outgoing-http-request` invocation-context span is started (durably:
    // `StartSpan` live, consumed on replay) between the durable scope `Start`
    // and the host-call `Start`, exactly like P2 starts it after
    // `begin_durable_function`. The span id is smuggled out for the injection
    // and completion paths below.
    //
    // The Golem-managed headers (`traceparent`/`tracestate` and the derived
    // `idempotency-key`) are injected into the request *resource* only, after
    // the host-call `Start` is written/claimed: like on P2 they are not part of
    // the recorded head, because they are deterministic functions of recorded
    // state — the trace context of the replayed span (same span id) and the
    // idempotency key derived from the call's own `Start` index, which is
    // stable across live execution and replay.
    let mut send_span: Option<SpanId> = None;
    let send_span_out = &mut send_span;
    let mut serialized_request: Option<SerializableP3HttpClientSend> = None;
    let serialized_request_out = &mut serialized_request;
    let mut handle = CallHandle::<P3HttpClientSend, P>::start_access_with(
        store,
        durable_worker_ctx::<Ctx, U>,
        function_type,
        async |_start_context: AccessStartContext| {
            let request =
                serialize_request::<Ctx, U>(store, borrow_resource(&req)).map_err(|err| {
                    WorkerExecutorError::runtime(format!(
                        "failed to serialize outgoing p3 HTTP request: {err}"
                    ))
                })?;
            let span_id = start_span_access(
                store,
                durable_worker_ctx::<Ctx, U>,
                &outgoing_http_request_span_attributes(&request),
            )
            .await?;
            *send_span_out = Some(span_id);
            *serialized_request_out = Some(request.clone());
            Ok(HostRequestP3HttpClientSend { request })
        },
    )
    .await
    .map_err(HttpError::trap)?;

    let span_id = send_span.expect("p3 HTTP send request builder did not run");
    let serialized_request = serialized_request.expect("p3 HTTP send request builder did not run");
    let send_start_index = handle.start_index();

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
                consume_replayed_request::<Ctx, U>(store, req).await?;
                let recorded_status = match &response.result {
                    SerializableP3HttpClientSendResult::Success(headers) => headers.status,
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
                            &span_id,
                            send_start_index,
                            &serialized_request.headers,
                        )
                        .map_err(HttpError::trap)?;
                        // The span stays open until the response body
                        // completes; hand it to the replayed response so the
                        // consume-body / drop paths consume the recorded
                        // `FinishSpan` at the same point it was written live.
                        register_open_response::<Ctx, U>(
                            store,
                            &response,
                            OpenP3HttpResponseState {
                                span_id,
                                method: serialized_request.method.to_string(),
                                uri: outgoing_http_request_uri(&serialized_request),
                                rebuild: Some(P3HttpSendRebuild {
                                    request: serialized_request,
                                    injected_headers,
                                    recorded_status,
                                }),
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
                        // after the `End`; consume its `FinishSpan` here.
                        finish_span_access(store, durable_worker_ctx::<Ctx, U>, &span_id)
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
    let (retry_method, retry_uri) = {
        let request_head = serialize_request::<Ctx, U>(store, borrow_resource(&req))?;
        let injected = golem_outgoing_http_headers::<Ctx, U>(
            store,
            &span_id,
            handle.start_index(),
            &request_head.headers,
        )
        .map_err(HttpError::trap)?;
        apply_headers_to_request_resource::<Ctx, U>(store, &req, &injected)
            .map_err(HttpError::trap)?;
        (
            request_head.method.to_string(),
            outgoing_http_request_uri(&request_head),
        )
    };

    let http_store = store.with_getter::<WasiHttp>(wasi_http_view::<Ctx, U>);
    let interrupt = store.with(|mut access| {
        durable_worker_ctx::<Ctx, U>(access.data_mut()).create_interrupt_signal()
    });
    let send = <WasiHttp as client::HostWithStore<U>>::send(&http_store, req);
    let send_result = match select(Box::pin(send), interrupt).await {
        Either::Left((result, _)) => result,
        Either::Right((interrupt_kind, _)) => {
            let error: anyhow::Error = interrupt_kind.into();
            Err(HttpError::trap(wasmtime::Error::from_anyhow(error)))
        }
    };

    match send_result {
        Ok(response) => {
            let result =
                SerializableP3HttpClientSendResult::Success(serialize_response_headers::<Ctx, U>(
                    store,
                    borrow_resource(&response),
                )?);
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
            // the P2 `end_http_request` span lifecycle. No rebuild info is
            // attached: the response carries the real network body.
            register_open_response::<Ctx, U>(
                store,
                &response,
                OpenP3HttpResponseState {
                    span_id,
                    method: retry_method.clone(),
                    uri: retry_uri.clone(),
                    rebuild: None,
                },
            );
            // Spawns the demand-gated transmission recorder at a
            // deterministic point (right after the send `End`, mirrored by the
            // replay arm above): if the guest reads the transmission future,
            // the result is recorded here; otherwise no entries are written.
            start_transmission_recording::<Ctx, U>(store, pending_transmission);
            Ok(response)
        }
        Err(error) => {
            if let Some(error_code) = error.downcast_ref() {
                let serialized_error = serialize_error_code(error_code);

                // Worker-level retry classification, mirroring the P2
                // outgoing-handler path: a transient transport/protocol failure
                // raises a retry trap here (the worker goes to `Retrying` per
                // its retry policy and re-executes the send from the abandoned
                // `Start` on replay) instead of surfacing as a guest-visible
                // error value. Permanent failures — and transient ones whose
                // retry budget is exhausted — fall through and are recorded and
                // returned to the guest, which is also what a recorded error
                // replays as.
                let for_retry: Result<(), &ErrorCode> = Err(error_code);
                handle
                    .try_trigger_retry_access(
                        store,
                        durable_worker_ctx::<Ctx, U>,
                        &for_retry,
                        |code| classify_serializable_http_error_code(&serialize_error_code(code)),
                        RetryContext::http(&retry_method, &retry_uri),
                    )
                    .await
                    .map_err(|err| HttpError::trap(wasmtime::Error::from_anyhow(err)))?;

                let result = SerializableP3HttpClientSendResult::HttpError(serialized_error);
                handle
                    .complete_access(
                        store,
                        durable_worker_ctx::<Ctx, U>,
                        HostResponseP3HttpClientSendResult { result },
                    )
                    .await
                    .map_err(HttpError::trap)?;
                finish_span_access(store, durable_worker_ctx::<Ctx, U>, &span_id)
                    .await
                    .map_err(HttpError::trap)?;
                // Spawns the demand-gated transmission recorder at a
                // deterministic point (right after the send `End` +
                // `FinishSpan`, mirrored by the replay arm above). The inner
                // send consumed the request even on failure, so a demanded
                // transmission result — `Ok(())` from the dropped I/O wiring,
                // or a deterministic body-validation error — still resolves.
                start_transmission_recording::<Ctx, U>(store, pending_transmission);
                Err(error)
            } else {
                // Trap path: the invocation is torn down and retried from the
                // oplog, so the span is not finished here (no `FinishSpan` is
                // recorded after an incomplete `Start`).
                Err(HttpError::trap(wasmtime::Error::from_anyhow(
                    handle.trap(error),
                )))
            }
        }
    }
}

/// Host-side state of an open p3 HTTP response created by the durable
/// `client::send`, keyed by the response resource rep in
/// `open_p3_http_responses`. Taken over by the durable consume-body task in
/// `consume_body`, or cleaned up by the response `drop` when the body was
/// never consumed.
pub(crate) struct OpenP3HttpResponseState {
    /// The `outgoing-http-request` invocation span of the send that produced
    /// this response. Finished when the response body completes (the durable
    /// consume-body terminal) or via a deferred [`DropEvent::FinishSpan`] when
    /// the response is dropped unconsumed.
    pub(crate) span_id: SpanId,
    /// Request method, for retry properties of body-transfer failures.
    pub(crate) method: String,
    /// Request URI, for retry properties of body-transfer failures.
    pub(crate) uri: String,
    /// Present iff the response was replayed from recorded headers (its body is
    /// an empty placeholder): how to re-issue the recorded request when the
    /// durable consume-body scope turns out to be incomplete and must
    /// re-execute live.
    pub(crate) rebuild: Option<P3HttpSendRebuild>,
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
pub(crate) struct P3HttpSendRebuild {
    request: SerializableP3HttpClientSend,
    injected_headers: Vec<(String, String)>,
    /// The recorded response status, used only to log divergence of the fresh
    /// response's status; the recorded head stays authoritative for the guest.
    recorded_status: u16,
}

/// Associates the open-response state (span, retry properties, optional
/// rebuild info) with the response resource created by (a live or replayed)
/// `client::send`.
fn register_open_response<Ctx: WorkerCtx, U: Send>(
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
fn recorded_head_declares_body(request: &SerializableP3HttpClientSend) -> bool {
    let content_length_declared = request.headers.get("content-length").is_some_and(|values| {
        values.iter().any(|value| {
            std::str::from_utf8(value)
                .ok()
                .and_then(|value| value.trim().parse::<u64>().ok())
                != Some(0)
        })
    });
    let transfer_encoding_declared = request
        .headers
        .get("transfer-encoding")
        .is_some_and(|values| !values.is_empty());
    content_length_declared || transfer_encoding_declared
}

/// Aborts the spawned I/O task of a re-issued request when dropped, bounding
/// its lifetime to the consume-body task that reads the rebuilt body
/// (mirroring the abort-on-drop handle the built-in `WasiHttp::send` attaches
/// to live response bodies).
struct AbortOnDropIoTask(tokio::task::JoinHandle<()>);

impl Drop for AbortOnDropIoTask {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// Result of attempting to re-issue a recorded send for an incomplete
/// consume-body scope.
enum RebuildOutcome {
    /// The request was re-issued: stream the fresh response body. The recorded
    /// response head stays authoritative for the guest — the rebuild only
    /// supplies a replacement body stream.
    Rebuilt {
        body: UnsyncBoxBody<Bytes, ErrorCode>,
        io_guard: AbortOnDropIoTask,
    },
    /// The re-issue failed on conversion or on the network: surfaced as a
    /// body-transfer error and classified for worker-level retry like any live
    /// body failure.
    Failed(ErrorCode),
    /// The recorded head declares a request body, which is not recorded in the
    /// oplog and cannot be reconstructed: fail the body transfer loud with a
    /// permanent error (never retry-routed — a retry would hit the same
    /// refusal forever).
    Refused(String),
}

/// Reconstructs the p3 request resource-equivalent from the recorded head:
/// method, scheme, authority, path, the guest-set headers plus the re-derived
/// Golem-managed headers (replacing same-name guest values, as the live
/// injection does), the recorded per-request timeout options, and an empty
/// body.
fn build_rebuilt_request(
    rebuild: &P3HttpSendRebuild,
) -> Result<wasmtime_wasi_http::p3::Request, String> {
    let method = deserialize_http_method(&rebuild.request.method)?;
    let scheme = rebuild
        .request
        .scheme
        .as_ref()
        .map(deserialize_uri_scheme)
        .transpose()?;
    let authority = rebuild
        .request
        .authority
        .as_deref()
        .map(|authority| {
            http::uri::Authority::try_from(authority)
                .map_err(|err| format!("invalid recorded request authority {authority}: {err}"))
        })
        .transpose()?;
    let path_with_query = rebuild
        .request
        .path_with_query
        .as_deref()
        .map(|path| {
            http::uri::PathAndQuery::try_from(path)
                .map_err(|err| format!("invalid recorded request path {path}: {err}"))
        })
        .transpose()?;

    let mut headers = HeaderMap::new();
    for (name, values) in &rebuild.request.headers {
        let name = HeaderName::try_from(name.as_str())
            .map_err(|err| format!("invalid recorded request header name {name}: {err}"))?;
        for value in values {
            let value = HeaderValue::try_from(value.clone())
                .map_err(|err| format!("invalid recorded request header value: {err}"))?;
            headers.append(name.clone(), value);
        }
    }
    for (name, value) in &rebuild.injected_headers {
        let name = HeaderName::try_from(name.as_str())
            .map_err(|err| format!("invalid injected header name {name}: {err}"))?;
        let value = HeaderValue::try_from(value.as_str())
            .map_err(|err| format!("invalid injected header value for {name}: {err}"))?;
        headers.remove(&name);
        headers.append(name, value);
    }

    let options = rebuild.request.options.as_ref().map(|options| {
        std::sync::Arc::new(wasmtime_wasi_http::p3::RequestOptions {
            connect_timeout: options
                .connect_timeout_nanos
                .map(std::time::Duration::from_nanos),
            first_byte_timeout: options
                .first_byte_timeout_nanos
                .map(std::time::Duration::from_nanos),
            between_bytes_timeout: options
                .between_bytes_timeout_nanos
                .map(std::time::Duration::from_nanos),
        })
    });

    let (request, _transmission) = wasmtime_wasi_http::p3::Request::new(
        method,
        scheme,
        authority,
        path_with_query,
        FieldMap::new_immutable(headers),
        options,
        Empty::<Bytes>::new()
            .map_err(|never| match never {})
            .boxed_unsync(),
    );
    Ok(request)
}

fn deserialize_http_method(method: &SerializableHttpMethod) -> Result<http::Method, String> {
    match method {
        SerializableHttpMethod::Get => Ok(http::Method::GET),
        SerializableHttpMethod::Post => Ok(http::Method::POST),
        SerializableHttpMethod::Put => Ok(http::Method::PUT),
        SerializableHttpMethod::Delete => Ok(http::Method::DELETE),
        SerializableHttpMethod::Head => Ok(http::Method::HEAD),
        SerializableHttpMethod::Connect => Ok(http::Method::CONNECT),
        SerializableHttpMethod::Options => Ok(http::Method::OPTIONS),
        SerializableHttpMethod::Trace => Ok(http::Method::TRACE),
        SerializableHttpMethod::Patch => Ok(http::Method::PATCH),
        SerializableHttpMethod::Other(other) => http::Method::from_bytes(other.as_bytes())
            .map_err(|err| format!("invalid recorded HTTP method {other}: {err}")),
    }
}

fn deserialize_uri_scheme(scheme: &SerializableP3HttpScheme) -> Result<http::uri::Scheme, String> {
    match scheme {
        SerializableP3HttpScheme::Http => Ok(http::uri::Scheme::HTTP),
        SerializableP3HttpScheme::Https => Ok(http::uri::Scheme::HTTPS),
        SerializableP3HttpScheme::Other(other) => other
            .parse()
            .map_err(|err| format!("invalid recorded request scheme {other}: {err}")),
    }
}

/// Re-issues a recorded send whose durable consume-body scope must re-execute
/// live after a restart — the P3 counterpart of P2's
/// `rebuild_request_after_replay`.
///
/// The re-issue is *recovery* of the already-recorded send, not a new
/// guest-visible call: it writes no oplog entries, does not count against HTTP
/// call limits, and starts no new span. It reuses the built-in request
/// conversion (host-header injection, default scheme) and the same connection
/// pool as the original live send. The fresh response head is discarded — the
/// recorded head, already delivered to the guest, stays authoritative; only
/// the body stream is taken.
async fn reissue_recorded_request<Ctx: WorkerCtx, U: 'static>(
    accessor: &Accessor<U, DurableP3<Ctx>>,
    rebuild: P3HttpSendRebuild,
) -> RebuildOutcome {
    if recorded_head_declares_body(&rebuild.request) {
        return RebuildOutcome::Refused(
            "cannot rebuild the in-flight p3 HTTP send after a restart: the request had a body, \
             which is not recorded in the oplog"
                .to_string(),
        );
    }

    let request = match build_rebuilt_request(&rebuild) {
        Ok(request) => request,
        Err(message) => return RebuildOutcome::Failed(ErrorCode::InternalError(Some(message))),
    };

    let converted = accessor.with(|mut access| {
        let pool = {
            let ctx = durable_worker_ctx::<Ctx, U>(access.data_mut());
            ctx.wasi_http.connection_pool.clone()
        };
        let converted = request.into_http_with_getter(
            access.as_context_mut(),
            async { Ok(()) },
            wasi_http_view::<Ctx, U>,
        );
        (pool, converted)
    });
    let (pool, converted) = converted;
    let (http_request, options) = match converted {
        Ok(converted) => converted,
        Err(error) => {
            return match error.downcast_ref() {
                Some(code) => RebuildOutcome::Failed(code.clone()),
                None => RebuildOutcome::Failed(ErrorCode::InternalError(Some(format!(
                    "failed to convert the rebuilt p3 HTTP request: {error:?}"
                )))),
            };
        }
    };
    let options = options.as_deref().copied();

    let sent = match pool {
        Some(pool) => pool.pooled_send_request_p3(http_request, options).await,
        None => match wasmtime_wasi_http::p3::default_send_request(http_request, options).await {
            Ok((response, io)) => Ok((
                response.map(http_body_util::BodyExt::boxed_unsync),
                Box::new(io) as Box<dyn std::future::Future<Output = Result<(), ErrorCode>> + Send>,
            )),
            Err(error) => Err(error),
        },
    };
    match sent {
        Ok((response, io)) => {
            if response.status().as_u16() != rebuild.recorded_status {
                debug!(
                    recorded_status = rebuild.recorded_status,
                    fresh_status = %response.status(),
                    "re-issued p3 HTTP request returned a different status than the recorded \
                     response; the recorded head stays authoritative"
                );
            }
            let body = response.into_body();
            let io = Box::into_pin(io);
            let io_task = tokio::task::spawn(async move {
                let result = io.await;
                debug!(?result, "re-issued p3 HTTP request I/O future finished");
            });
            RebuildOutcome::Rebuilt {
                body,
                io_guard: AbortOnDropIoTask(io_task),
            }
        }
        Err(code) => RebuildOutcome::Failed(code),
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
struct HttpTransmissionResultForwarder(Option<oneshot::Sender<Result<(), ErrorCode>>>);

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
struct HttpTransmissionFutureProducer {
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

fn serialize_transmission_result(
    result: &Result<(), ErrorCode>,
) -> Result<(), SerializableHttpErrorCode> {
    result.as_ref().map(|_| ()).map_err(serialize_error_code)
}

fn deserialize_transmission_result(
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
fn start_transmission_recording<Ctx, U>(
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
fn fail_transmission_task(
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
struct HttpRequestBodyTransmissionTask<Ctx> {
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
struct HttpRequestTransmissionPassthroughTask<Ctx> {
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
fn detach_request_transmission_passthrough<Ctx: WorkerCtx, U: Send + 'static>(
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
fn outgoing_http_request_uri(request: &SerializableP3HttpClientSend) -> String {
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
fn outgoing_http_request_span_attributes(
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
fn golem_outgoing_http_headers<Ctx: WorkerCtx, U: Send>(
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
fn apply_headers_to_request_resource<Ctx: WorkerCtx, U: Send>(
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

fn is_idempotent_http_method(method: &SerializableHttpMethod) -> bool {
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

fn borrow_resource<T: 'static>(resource: &Resource<T>) -> Resource<T> {
    Resource::new_borrow(resource.rep())
}

/// Consume a request resource on the replay path, mirroring what the live
/// `WasiHttp::send` does to the request minus the network send.
///
/// The live path deletes the request from the table, converts it with
/// `into_http` (wiring the outgoing body stream and its content-length
/// validation), and drives the body in the background while returning the
/// response head. On replay we never call `send`, so we reproduce the request
/// side here: delete the request, convert it, and spawn a task that drains the
/// body. This
/// * deletes the request resource (matching live), so it does not leak;
/// * reads the guest's outgoing body stream so a guest streaming a body larger
///   than the channel buffer does not block on a reader that never reads;
/// * resolves the guest-held request-body transmission future with the same
///   deterministic result as the live path (e.g. an `HttpRequestBodySize`
///   error for a content-length mismatch), because that validation lives in
///   `into_http`/`GuestBody`, not in the network send.
///
/// The drain runs in a spawned task rather than inline: live `WasiHttp::send`
/// polls its body-I/O future once and, if it is still pending, spawns a task to
/// finish it instead of blocking the response (`p3/host/handler.rs`). Draining
/// inline here would deadlock a guest that awaits the recorded response before
/// finishing its request-body upload.
///
/// The drain's `ErrorCode` is not the `client::send` result — the recorded
/// response head is the authoritative `client::send` outcome — but it feeds the
/// guest-held request-body transmission chain. Live `WasiHttp::send` wires the
/// transmission future to its request I/O result; on replay we wire it to the
/// drain result via a `oneshot` channel, which flows into the `raw_rx` channel
/// of the request's [`PendingHttpRequestBodyTransmission`] wiring.
///
/// The drain result is normally *not* what the guest sees: the guest-facing
/// transmission future resolves from the **recorded** `body-transmission`
/// terminal (see [`HttpRequestBodyTransmissionTask`]), so a live
/// mid-body network failure replays exactly instead of as `Ok(())`. The
/// drain-derived value is the documented best-effort fallback used only when
/// the recorded terminal is missing (the original run crashed after the send
/// `End` but before the upload result was observed): the incomplete
/// `body-transmission` `Start` re-executes against the drain result — which
/// still surfaces deterministic outgoing-body failures such as a
/// `content-length` mismatch or a guest trailers future resolving to an
/// `ErrorCode` (see
/// `request_body_transmission_result_depends_on_unrecorded_body_read`).
async fn consume_replayed_request<Ctx: WorkerCtx, U: Send + 'static>(
    store: &Accessor<U, DurableP3<Ctx>>,
    req: Resource<Request>,
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
    store.spawn(ReplayRequestBodyDrain::<Ctx>::new(body, drain_result_tx));
    Ok(())
}

/// Drives an outgoing request body to completion, discarding each frame as it
/// is read, and returns its terminal result. Frames are dropped one at a time
/// (rather than accumulated with `collect`) so draining a large replayed upload
/// does not buffer the whole body in memory; the bytes are not needed because
/// the recorded response head is already authoritative.
async fn drain_request_body(mut body: UnsyncBoxBody<Bytes, ErrorCode>) -> Result<(), ErrorCode> {
    while let Some(frame) = body.frame().await {
        frame?;
    }
    Ok(())
}

/// Background task that drains a replayed request's outgoing body to completion
/// (no network) and reports the drain result to the request transmission
/// future. See [`consume_replayed_request`] for why this runs off the `send`
/// return path and how its result is wired back to the guest.
struct ReplayRequestBodyDrain<Ctx> {
    body: UnsyncBoxBody<Bytes, ErrorCode>,
    result_tx: oneshot::Sender<Result<(), ErrorCode>>,
    _phantom: PhantomData<fn() -> Ctx>,
}

impl<Ctx> ReplayRequestBodyDrain<Ctx> {
    fn new(
        body: UnsyncBoxBody<Bytes, ErrorCode>,
        result_tx: oneshot::Sender<Result<(), ErrorCode>>,
    ) -> Self {
        Self {
            body,
            result_tx,
            _phantom: PhantomData,
        }
    }
}

impl<Ctx, U> AccessorTask<U, DurableP3<Ctx>> for ReplayRequestBodyDrain<Ctx>
where
    Ctx: WorkerCtx,
    U: 'static,
{
    async fn run(self, _accessor: &Accessor<U, DurableP3<Ctx>>) -> wasmtime::Result<()> {
        let _ = self.result_tx.send(drain_request_body(self.body).await);
        Ok(())
    }
}

fn serialize_request<Ctx: WorkerCtx, U: Send>(
    store: &Accessor<U, DurableP3<Ctx>>,
    req: Resource<Request>,
) -> HttpResult<SerializableP3HttpClientSend> {
    let http_store = store.with_getter::<WasiHttp>(wasi_http_view::<Ctx, U>);
    http_store.with(|mut access| -> HttpResult<SerializableP3HttpClientSend> {
        let mut view = access.get();
        let method = serialize_method(
            types::HostRequest::get_method(&mut view, borrow_resource(&req))
                .map_err(HttpError::trap)?,
        );
        let scheme = types::HostRequest::get_scheme(&mut view, borrow_resource(&req))
            .map_err(HttpError::trap)?
            .map(serialize_scheme);
        let authority = types::HostRequest::get_authority(&mut view, borrow_resource(&req))
            .map_err(HttpError::trap)?;
        let path_with_query =
            types::HostRequest::get_path_with_query(&mut view, borrow_resource(&req))
                .map_err(HttpError::trap)?;
        let headers_resource = types::HostRequest::get_headers(&mut view, borrow_resource(&req))
            .map_err(HttpError::trap)?;
        let headers = copy_fields(&mut view, headers_resource)?;
        let options = match types::HostRequest::get_options(&mut view, borrow_resource(&req))
            .map_err(HttpError::trap)?
        {
            Some(options) => {
                let serialized = SerializableP3HttpRequestOptions {
                    connect_timeout_nanos: types::HostRequestOptions::get_connect_timeout(
                        &mut view,
                        borrow_resource(&options),
                    )
                    .map_err(HttpError::trap)?,
                    first_byte_timeout_nanos: types::HostRequestOptions::get_first_byte_timeout(
                        &mut view,
                        borrow_resource(&options),
                    )
                    .map_err(HttpError::trap)?,
                    between_bytes_timeout_nanos:
                        types::HostRequestOptions::get_between_bytes_timeout(
                            &mut view,
                            borrow_resource(&options),
                        )
                        .map_err(HttpError::trap)?,
                };
                types::HostRequestOptions::drop(&mut view, options).map_err(HttpError::trap)?;
                Some(serialized)
            }
            None => None,
        };

        Ok(SerializableP3HttpClientSend {
            method,
            scheme,
            authority,
            path_with_query,
            headers,
            options,
        })
    })
}

fn serialize_response_headers<Ctx: WorkerCtx, U: Send>(
    store: &Accessor<U, DurableP3<Ctx>>,
    response: Resource<Response>,
) -> HttpResult<SerializableResponseHeaders> {
    let http_store = store.with_getter::<WasiHttp>(wasi_http_view::<Ctx, U>);
    http_store.with(|mut access| -> HttpResult<SerializableResponseHeaders> {
        let mut view = access.get();
        let status = types::HostResponse::get_status_code(&mut view, borrow_resource(&response))
            .map_err(HttpError::trap)?;
        let headers_resource =
            types::HostResponse::get_headers(&mut view, response).map_err(HttpError::trap)?;
        let headers = copy_fields(&mut view, headers_resource)?;
        Ok(SerializableResponseHeaders { status, headers })
    })
}

fn copy_fields(
    view: &mut wasmtime_wasi_http::p3::WasiHttpCtxView<'_>,
    fields: Resource<Fields>,
) -> HttpResult<HashMap<String, Vec<Vec<u8>>>> {
    let entries =
        types::HostFields::copy_all(view, borrow_resource(&fields)).map_err(HttpError::trap)?;
    types::HostFields::drop(view, fields).map_err(HttpError::trap)?;
    let mut headers = HashMap::new();
    for (name, value) in entries {
        headers.entry(name).or_insert_with(Vec::new).push(value);
    }
    Ok(headers)
}

fn replay_send_response<Ctx: WorkerCtx, U: Send>(
    store: &Accessor<U, DurableP3<Ctx>>,
    result: SerializableP3HttpClientSendResult,
) -> HttpResult<Resource<Response>> {
    match result {
        SerializableP3HttpClientSendResult::Success(headers) => {
            response_from_recorded_headers::<Ctx, U>(store, headers)
        }
        SerializableP3HttpClientSendResult::HttpError(error) => {
            Err(deserialize_error_code(error).into())
        }
    }
}

fn response_from_recorded_headers<Ctx: WorkerCtx, U: Send>(
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

fn serialize_method(method: Method) -> SerializableHttpMethod {
    match method {
        Method::Get => SerializableHttpMethod::Get,
        Method::Post => SerializableHttpMethod::Post,
        Method::Put => SerializableHttpMethod::Put,
        Method::Delete => SerializableHttpMethod::Delete,
        Method::Head => SerializableHttpMethod::Head,
        Method::Connect => SerializableHttpMethod::Connect,
        Method::Options => SerializableHttpMethod::Options,
        Method::Trace => SerializableHttpMethod::Trace,
        Method::Patch => SerializableHttpMethod::Patch,
        Method::Other(method) => SerializableHttpMethod::Other(method),
    }
}

fn serialize_scheme(scheme: Scheme) -> SerializableP3HttpScheme {
    match scheme {
        Scheme::Http => SerializableP3HttpScheme::Http,
        Scheme::Https => SerializableP3HttpScheme::Https,
        Scheme::Other(scheme) => SerializableP3HttpScheme::Other(scheme),
    }
}

fn serialize_error_code(error: &ErrorCode) -> SerializableHttpErrorCode {
    match error {
        ErrorCode::DnsTimeout => SerializableHttpErrorCode::DnsTimeout,
        ErrorCode::DnsError(payload) => {
            SerializableHttpErrorCode::DnsError(serialize_dns_error_payload(payload))
        }
        ErrorCode::DestinationNotFound => SerializableHttpErrorCode::DestinationNotFound,
        ErrorCode::DestinationUnavailable => SerializableHttpErrorCode::DestinationUnavailable,
        ErrorCode::DestinationIpProhibited => SerializableHttpErrorCode::DestinationIpProhibited,
        ErrorCode::DestinationIpUnroutable => SerializableHttpErrorCode::DestinationIpUnroutable,
        ErrorCode::ConnectionRefused => SerializableHttpErrorCode::ConnectionRefused,
        ErrorCode::ConnectionTerminated => SerializableHttpErrorCode::ConnectionTerminated,
        ErrorCode::ConnectionTimeout => SerializableHttpErrorCode::ConnectionTimeout,
        ErrorCode::ConnectionReadTimeout => SerializableHttpErrorCode::ConnectionReadTimeout,
        ErrorCode::ConnectionWriteTimeout => SerializableHttpErrorCode::ConnectionWriteTimeout,
        ErrorCode::ConnectionLimitReached => SerializableHttpErrorCode::ConnectionLimitReached,
        ErrorCode::TlsProtocolError => SerializableHttpErrorCode::TlsProtocolError,
        ErrorCode::TlsCertificateError => SerializableHttpErrorCode::TlsCertificateError,
        ErrorCode::TlsAlertReceived(payload) => SerializableHttpErrorCode::TlsAlertReceived(
            serialize_tls_alert_received_payload(payload),
        ),
        ErrorCode::HttpRequestDenied => SerializableHttpErrorCode::HttpRequestDenied,
        ErrorCode::HttpRequestLengthRequired => {
            SerializableHttpErrorCode::HttpRequestLengthRequired
        }
        ErrorCode::HttpRequestBodySize(payload) => {
            SerializableHttpErrorCode::HttpRequestBodySize(*payload)
        }
        ErrorCode::HttpRequestMethodInvalid => SerializableHttpErrorCode::HttpRequestMethodInvalid,
        ErrorCode::HttpRequestUriInvalid => SerializableHttpErrorCode::HttpRequestUriInvalid,
        ErrorCode::HttpRequestUriTooLong => SerializableHttpErrorCode::HttpRequestUriTooLong,
        ErrorCode::HttpRequestHeaderSectionSize(payload) => {
            SerializableHttpErrorCode::HttpRequestHeaderSectionSize(*payload)
        }
        ErrorCode::HttpRequestHeaderSize(payload) => {
            SerializableHttpErrorCode::HttpRequestHeaderSize(
                payload.as_ref().map(serialize_field_size_payload),
            )
        }
        ErrorCode::HttpRequestTrailerSectionSize(payload) => {
            SerializableHttpErrorCode::HttpRequestTrailerSectionSize(*payload)
        }
        ErrorCode::HttpRequestTrailerSize(payload) => {
            SerializableHttpErrorCode::HttpRequestTrailerSize(serialize_field_size_payload(payload))
        }
        ErrorCode::HttpResponseIncomplete => SerializableHttpErrorCode::HttpResponseIncomplete,
        ErrorCode::HttpResponseHeaderSectionSize(payload) => {
            SerializableHttpErrorCode::HttpResponseHeaderSectionSize(*payload)
        }
        ErrorCode::HttpResponseHeaderSize(payload) => {
            SerializableHttpErrorCode::HttpResponseHeaderSize(serialize_field_size_payload(payload))
        }
        ErrorCode::HttpResponseBodySize(payload) => {
            SerializableHttpErrorCode::HttpResponseBodySize(*payload)
        }
        ErrorCode::HttpResponseTrailerSectionSize(payload) => {
            SerializableHttpErrorCode::HttpResponseTrailerSectionSize(*payload)
        }
        ErrorCode::HttpResponseTrailerSize(payload) => {
            SerializableHttpErrorCode::HttpResponseTrailerSize(serialize_field_size_payload(
                payload,
            ))
        }
        ErrorCode::HttpResponseTransferCoding(payload) => {
            SerializableHttpErrorCode::HttpResponseTransferCoding(payload.clone())
        }
        ErrorCode::HttpResponseContentCoding(payload) => {
            SerializableHttpErrorCode::HttpResponseContentCoding(payload.clone())
        }
        ErrorCode::HttpResponseTimeout => SerializableHttpErrorCode::HttpResponseTimeout,
        ErrorCode::HttpUpgradeFailed => SerializableHttpErrorCode::HttpUpgradeFailed,
        ErrorCode::HttpProtocolError => SerializableHttpErrorCode::HttpProtocolError,
        ErrorCode::LoopDetected => SerializableHttpErrorCode::LoopDetected,
        ErrorCode::ConfigurationError => SerializableHttpErrorCode::ConfigurationError,
        ErrorCode::InternalError(payload) => {
            SerializableHttpErrorCode::InternalError(payload.clone())
        }
    }
}

fn deserialize_error_code(error: SerializableHttpErrorCode) -> ErrorCode {
    match error {
        SerializableHttpErrorCode::DnsTimeout => ErrorCode::DnsTimeout,
        SerializableHttpErrorCode::DnsError(payload) => {
            ErrorCode::DnsError(deserialize_dns_error_payload(payload))
        }
        SerializableHttpErrorCode::DestinationNotFound => ErrorCode::DestinationNotFound,
        SerializableHttpErrorCode::DestinationUnavailable => ErrorCode::DestinationUnavailable,
        SerializableHttpErrorCode::DestinationIpProhibited => ErrorCode::DestinationIpProhibited,
        SerializableHttpErrorCode::DestinationIpUnroutable => ErrorCode::DestinationIpUnroutable,
        SerializableHttpErrorCode::ConnectionRefused => ErrorCode::ConnectionRefused,
        SerializableHttpErrorCode::ConnectionTerminated => ErrorCode::ConnectionTerminated,
        SerializableHttpErrorCode::ConnectionTimeout => ErrorCode::ConnectionTimeout,
        SerializableHttpErrorCode::ConnectionReadTimeout => ErrorCode::ConnectionReadTimeout,
        SerializableHttpErrorCode::ConnectionWriteTimeout => ErrorCode::ConnectionWriteTimeout,
        SerializableHttpErrorCode::ConnectionLimitReached => ErrorCode::ConnectionLimitReached,
        SerializableHttpErrorCode::TlsProtocolError => ErrorCode::TlsProtocolError,
        SerializableHttpErrorCode::TlsCertificateError => ErrorCode::TlsCertificateError,
        SerializableHttpErrorCode::TlsAlertReceived(payload) => {
            ErrorCode::TlsAlertReceived(deserialize_tls_alert_received_payload(payload))
        }
        SerializableHttpErrorCode::HttpRequestDenied => ErrorCode::HttpRequestDenied,
        SerializableHttpErrorCode::HttpRequestLengthRequired => {
            ErrorCode::HttpRequestLengthRequired
        }
        SerializableHttpErrorCode::HttpRequestBodySize(payload) => {
            ErrorCode::HttpRequestBodySize(payload)
        }
        SerializableHttpErrorCode::HttpRequestMethodInvalid => ErrorCode::HttpRequestMethodInvalid,
        SerializableHttpErrorCode::HttpRequestUriInvalid => ErrorCode::HttpRequestUriInvalid,
        SerializableHttpErrorCode::HttpRequestUriTooLong => ErrorCode::HttpRequestUriTooLong,
        SerializableHttpErrorCode::HttpRequestHeaderSectionSize(payload) => {
            ErrorCode::HttpRequestHeaderSectionSize(payload)
        }
        SerializableHttpErrorCode::HttpRequestHeaderSize(payload) => {
            ErrorCode::HttpRequestHeaderSize(payload.map(deserialize_field_size_payload))
        }
        SerializableHttpErrorCode::HttpRequestTrailerSectionSize(payload) => {
            ErrorCode::HttpRequestTrailerSectionSize(payload)
        }
        SerializableHttpErrorCode::HttpRequestTrailerSize(payload) => {
            ErrorCode::HttpRequestTrailerSize(deserialize_field_size_payload(payload))
        }
        SerializableHttpErrorCode::HttpResponseIncomplete => ErrorCode::HttpResponseIncomplete,
        SerializableHttpErrorCode::HttpResponseHeaderSectionSize(payload) => {
            ErrorCode::HttpResponseHeaderSectionSize(payload)
        }
        SerializableHttpErrorCode::HttpResponseHeaderSize(payload) => {
            ErrorCode::HttpResponseHeaderSize(deserialize_field_size_payload(payload))
        }
        SerializableHttpErrorCode::HttpResponseBodySize(payload) => {
            ErrorCode::HttpResponseBodySize(payload)
        }
        SerializableHttpErrorCode::HttpResponseTrailerSectionSize(payload) => {
            ErrorCode::HttpResponseTrailerSectionSize(payload)
        }
        SerializableHttpErrorCode::HttpResponseTrailerSize(payload) => {
            ErrorCode::HttpResponseTrailerSize(deserialize_field_size_payload(payload))
        }
        SerializableHttpErrorCode::HttpResponseTransferCoding(payload) => {
            ErrorCode::HttpResponseTransferCoding(payload)
        }
        SerializableHttpErrorCode::HttpResponseContentCoding(payload) => {
            ErrorCode::HttpResponseContentCoding(payload)
        }
        SerializableHttpErrorCode::HttpResponseTimeout => ErrorCode::HttpResponseTimeout,
        SerializableHttpErrorCode::HttpUpgradeFailed => ErrorCode::HttpUpgradeFailed,
        SerializableHttpErrorCode::HttpProtocolError => ErrorCode::HttpProtocolError,
        SerializableHttpErrorCode::LoopDetected => ErrorCode::LoopDetected,
        SerializableHttpErrorCode::ConfigurationError => ErrorCode::ConfigurationError,
        SerializableHttpErrorCode::InternalError(payload) => ErrorCode::InternalError(payload),
    }
}

fn serialize_dns_error_payload(payload: &types::DnsErrorPayload) -> SerializableDnsErrorPayload {
    SerializableDnsErrorPayload {
        rcode: payload.rcode.clone(),
        info_code: payload.info_code,
    }
}

fn deserialize_dns_error_payload(payload: SerializableDnsErrorPayload) -> types::DnsErrorPayload {
    types::DnsErrorPayload {
        rcode: payload.rcode,
        info_code: payload.info_code,
    }
}

fn serialize_tls_alert_received_payload(
    payload: &types::TlsAlertReceivedPayload,
) -> SerializableTlsAlertReceivedPayload {
    SerializableTlsAlertReceivedPayload {
        alert_id: payload.alert_id,
        alert_message: payload.alert_message.clone(),
    }
}

fn deserialize_tls_alert_received_payload(
    payload: SerializableTlsAlertReceivedPayload,
) -> types::TlsAlertReceivedPayload {
    types::TlsAlertReceivedPayload {
        alert_id: payload.alert_id,
        alert_message: payload.alert_message,
    }
}

fn serialize_field_size_payload(payload: &types::FieldSizePayload) -> SerializableFieldSizePayload {
    SerializableFieldSizePayload {
        field_name: payload.field_name.clone(),
        field_size: payload.field_size,
    }
}

fn deserialize_field_size_payload(
    payload: SerializableFieldSizePayload,
) -> types::FieldSizePayload {
    types::FieldSizePayload {
        field_name: payload.field_name,
        field_size: payload.field_size,
    }
}

impl<Ctx: WorkerCtx> types::Host for DurableP3View<'_, Ctx> {
    fn convert_error_code(&mut self, error: HttpError) -> wasmtime::Result<ErrorCode> {
        observe_function_call(&*self.0, "http::types", "convert-error-code");
        types::Host::convert_error_code(&mut WasiHttpView::http(self.0), error)
    }

    fn convert_header_error(&mut self, error: HeaderError) -> wasmtime::Result<types::HeaderError> {
        observe_function_call(&*self.0, "http::types", "convert-header-error");
        types::Host::convert_header_error(&mut WasiHttpView::http(self.0), error)
    }

    fn convert_request_options_error(
        &mut self,
        error: RequestOptionsError,
    ) -> wasmtime::Result<types::RequestOptionsError> {
        observe_function_call(&*self.0, "http::types", "convert-request-options-error");
        types::Host::convert_request_options_error(&mut WasiHttpView::http(self.0), error)
    }
}

impl<Ctx: WorkerCtx> types::HostFields for DurableP3View<'_, Ctx> {
    fn new(&mut self) -> wasmtime::Result<Resource<Fields>> {
        observe_function_call(&*self.0, "http::types::fields", "new");
        types::HostFields::new(&mut WasiHttpView::http(self.0))
    }

    fn from_list(
        &mut self,
        entries: Vec<(FieldName, FieldValue)>,
    ) -> HeaderResult<Resource<Fields>> {
        observe_function_call(&*self.0, "http::types::fields", "from-list");
        types::HostFields::from_list(&mut WasiHttpView::http(self.0), entries)
    }

    fn get(
        &mut self,
        fields: Resource<Fields>,
        name: FieldName,
    ) -> wasmtime::Result<Vec<FieldValue>> {
        observe_function_call(&*self.0, "http::types::fields", "get");
        types::HostFields::get(&mut WasiHttpView::http(self.0), fields, name)
    }

    fn has(&mut self, fields: Resource<Fields>, name: FieldName) -> wasmtime::Result<bool> {
        observe_function_call(&*self.0, "http::types::fields", "has");
        types::HostFields::has(&mut WasiHttpView::http(self.0), fields, name)
    }

    fn set(
        &mut self,
        fields: Resource<Fields>,
        name: FieldName,
        value: Vec<FieldValue>,
    ) -> HeaderResult<()> {
        observe_function_call(&*self.0, "http::types::fields", "set");
        types::HostFields::set(&mut WasiHttpView::http(self.0), fields, name, value)
    }

    fn delete(&mut self, fields: Resource<Fields>, name: FieldName) -> HeaderResult<()> {
        observe_function_call(&*self.0, "http::types::fields", "delete");
        types::HostFields::delete(&mut WasiHttpView::http(self.0), fields, name)
    }

    fn get_and_delete(
        &mut self,
        fields: Resource<Fields>,
        name: FieldName,
    ) -> HeaderResult<Vec<FieldValue>> {
        observe_function_call(&*self.0, "http::types::fields", "get-and-delete");
        types::HostFields::get_and_delete(&mut WasiHttpView::http(self.0), fields, name)
    }

    fn append(
        &mut self,
        fields: Resource<Fields>,
        name: FieldName,
        value: FieldValue,
    ) -> HeaderResult<()> {
        observe_function_call(&*self.0, "http::types::fields", "append");
        types::HostFields::append(&mut WasiHttpView::http(self.0), fields, name, value)
    }

    fn copy_all(
        &mut self,
        fields: Resource<Fields>,
    ) -> wasmtime::Result<Vec<(FieldName, FieldValue)>> {
        observe_function_call(&*self.0, "http::types::fields", "copy-all");
        types::HostFields::copy_all(&mut WasiHttpView::http(self.0), fields)
    }

    fn clone(&mut self, fields: Resource<Fields>) -> wasmtime::Result<Resource<Fields>> {
        observe_function_call(&*self.0, "http::types::fields", "clone");
        types::HostFields::clone(&mut WasiHttpView::http(self.0), fields)
    }

    fn drop(&mut self, fields: Resource<Fields>) -> wasmtime::Result<()> {
        observe_function_call(&*self.0, "http::types::fields", "drop");
        types::HostFields::drop(&mut WasiHttpView::http(self.0), fields)
    }
}

impl<Ctx: WorkerCtx> types::HostRequest for DurableP3View<'_, Ctx> {
    fn get_method(&mut self, req: Resource<Request>) -> wasmtime::Result<Method> {
        observe_function_call(&*self.0, "http::types::request", "get-method");
        types::HostRequest::get_method(&mut WasiHttpView::http(self.0), req)
    }

    fn set_method(
        &mut self,
        req: Resource<Request>,
        method: Method,
    ) -> wasmtime::Result<Result<(), ()>> {
        observe_function_call(&*self.0, "http::types::request", "set-method");
        types::HostRequest::set_method(&mut WasiHttpView::http(self.0), req, method)
    }

    fn get_path_with_query(&mut self, req: Resource<Request>) -> wasmtime::Result<Option<String>> {
        observe_function_call(&*self.0, "http::types::request", "get-path-with-query");
        types::HostRequest::get_path_with_query(&mut WasiHttpView::http(self.0), req)
    }

    fn set_path_with_query(
        &mut self,
        req: Resource<Request>,
        path_with_query: Option<String>,
    ) -> wasmtime::Result<Result<(), ()>> {
        observe_function_call(&*self.0, "http::types::request", "set-path-with-query");
        types::HostRequest::set_path_with_query(
            &mut WasiHttpView::http(self.0),
            req,
            path_with_query,
        )
    }

    fn get_scheme(&mut self, req: Resource<Request>) -> wasmtime::Result<Option<Scheme>> {
        observe_function_call(&*self.0, "http::types::request", "get-scheme");
        types::HostRequest::get_scheme(&mut WasiHttpView::http(self.0), req)
    }

    fn set_scheme(
        &mut self,
        req: Resource<Request>,
        scheme: Option<Scheme>,
    ) -> wasmtime::Result<Result<(), ()>> {
        observe_function_call(&*self.0, "http::types::request", "set-scheme");
        types::HostRequest::set_scheme(&mut WasiHttpView::http(self.0), req, scheme)
    }

    fn get_authority(&mut self, req: Resource<Request>) -> wasmtime::Result<Option<String>> {
        observe_function_call(&*self.0, "http::types::request", "get-authority");
        types::HostRequest::get_authority(&mut WasiHttpView::http(self.0), req)
    }

    fn set_authority(
        &mut self,
        req: Resource<Request>,
        authority: Option<String>,
    ) -> wasmtime::Result<Result<(), ()>> {
        observe_function_call(&*self.0, "http::types::request", "set-authority");
        types::HostRequest::set_authority(&mut WasiHttpView::http(self.0), req, authority)
    }

    fn get_options(
        &mut self,
        req: Resource<Request>,
    ) -> wasmtime::Result<Option<Resource<RequestOptions>>> {
        observe_function_call(&*self.0, "http::types::request", "get-options");
        types::HostRequest::get_options(&mut WasiHttpView::http(self.0), req)
    }

    fn get_headers(&mut self, req: Resource<Request>) -> wasmtime::Result<Resource<Headers>> {
        observe_function_call(&*self.0, "http::types::request", "get-headers");
        types::HostRequest::get_headers(&mut WasiHttpView::http(self.0), req)
    }
}

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

impl<Ctx: WorkerCtx> types::HostRequestOptions for DurableP3View<'_, Ctx> {
    fn new(&mut self) -> wasmtime::Result<Resource<RequestOptions>> {
        observe_function_call(&*self.0, "http::types::request-options", "new");
        types::HostRequestOptions::new(&mut WasiHttpView::http(self.0))
    }

    fn get_connect_timeout(
        &mut self,
        opts: Resource<RequestOptions>,
    ) -> wasmtime::Result<Option<Duration>> {
        observe_function_call(
            &*self.0,
            "http::types::request-options",
            "get-connect-timeout",
        );
        types::HostRequestOptions::get_connect_timeout(&mut WasiHttpView::http(self.0), opts)
    }

    fn set_connect_timeout(
        &mut self,
        opts: Resource<RequestOptions>,
        duration: Option<Duration>,
    ) -> RequestOptionsResult<()> {
        observe_function_call(
            &*self.0,
            "http::types::request-options",
            "set-connect-timeout",
        );
        types::HostRequestOptions::set_connect_timeout(
            &mut WasiHttpView::http(self.0),
            opts,
            duration,
        )
    }

    fn get_first_byte_timeout(
        &mut self,
        opts: Resource<RequestOptions>,
    ) -> wasmtime::Result<Option<Duration>> {
        observe_function_call(
            &*self.0,
            "http::types::request-options",
            "get-first-byte-timeout",
        );
        types::HostRequestOptions::get_first_byte_timeout(&mut WasiHttpView::http(self.0), opts)
    }

    fn set_first_byte_timeout(
        &mut self,
        opts: Resource<RequestOptions>,
        duration: Option<Duration>,
    ) -> RequestOptionsResult<()> {
        observe_function_call(
            &*self.0,
            "http::types::request-options",
            "set-first-byte-timeout",
        );
        types::HostRequestOptions::set_first_byte_timeout(
            &mut WasiHttpView::http(self.0),
            opts,
            duration,
        )
    }

    fn get_between_bytes_timeout(
        &mut self,
        opts: Resource<RequestOptions>,
    ) -> wasmtime::Result<Option<Duration>> {
        observe_function_call(
            &*self.0,
            "http::types::request-options",
            "get-between-bytes-timeout",
        );
        types::HostRequestOptions::get_between_bytes_timeout(&mut WasiHttpView::http(self.0), opts)
    }

    fn set_between_bytes_timeout(
        &mut self,
        opts: Resource<RequestOptions>,
        duration: Option<Duration>,
    ) -> RequestOptionsResult<()> {
        observe_function_call(
            &*self.0,
            "http::types::request-options",
            "set-between-bytes-timeout",
        );
        types::HostRequestOptions::set_between_bytes_timeout(
            &mut WasiHttpView::http(self.0),
            opts,
            duration,
        )
    }

    fn clone(
        &mut self,
        opts: Resource<RequestOptions>,
    ) -> wasmtime::Result<Resource<RequestOptions>> {
        observe_function_call(&*self.0, "http::types::request-options", "clone");
        types::HostRequestOptions::clone(&mut WasiHttpView::http(self.0), opts)
    }

    fn drop(&mut self, opts: Resource<RequestOptions>) -> wasmtime::Result<()> {
        observe_function_call(&*self.0, "http::types::request-options", "drop");
        types::HostRequestOptions::drop(&mut WasiHttpView::http(self.0), opts)
    }
}

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
type HttpTrailersOutcome = Result<Option<HeaderMap>, ErrorCode>;

/// A demand from the body stream producer to the durable [`HttpConsumeBodyTask`].
enum HttpBodyDemand {
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
enum HttpBodyChunkReply {
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
enum HttpTrailersResolution {
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
struct DurableHttpBodyProducer {
    demand_tx: mpsc::UnboundedSender<HttpBodyDemand>,
    pending: Option<PendingHttpBodyRead>,
    pending_cancel: Option<oneshot::Receiver<()>>,
    finished: bool,
}

struct PendingHttpBodyRead {
    reply: oneshot::Receiver<HttpBodyChunkReply>,
    cancel: Option<oneshot::Sender<()>>,
    cancel_ack: Option<oneshot::Receiver<()>>,
    cancelling: bool,
}

impl DurableHttpBodyProducer {
    fn new(demand_tx: mpsc::UnboundedSender<HttpBodyDemand>) -> Self {
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
                if self.demand_tx.send(HttpBodyDemand::Cancel(tx)).is_err() {
                    self.finished = true;
                    return Poll::Ready(Ok(StreamResult::Cancelled));
                }
                self.pending_cancel = Some(rx);
                continue;
            }

            let (reply_tx, reply_rx) = oneshot::channel();
            let (cancel_tx, cancel_rx) = oneshot::channel();
            let (cancel_ack_tx, cancel_ack_rx) = oneshot::channel();
            if self
                .demand_tx
                .send(HttpBodyDemand::Read {
                    reply: reply_tx,
                    cancel: cancel_rx,
                    cancel_ack: cancel_ack_tx,
                })
                .is_err()
            {
                self.finished = true;
                return Poll::Ready(Err(wasmtime::Error::msg(
                    "consume-body durable task is gone",
                )));
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
struct HttpTrailersFutureProducer<Ctx, U> {
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

fn serialize_consume_body_result(
    result: &Result<Option<HeaderMap>, ErrorCode>,
) -> SerializableP3HttpConsumeBodyResult {
    match result {
        Ok(trailers) => {
            SerializableP3HttpConsumeBodyResult::Trailers(trailers.as_ref().map(serialize_headers))
        }
        Err(error) => SerializableP3HttpConsumeBodyResult::HttpError(serialize_error_code(error)),
    }
}

fn deserialize_consume_body_result(
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
fn fail_consume_body_task(
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

fn serialize_headers(headers: &HeaderMap) -> HashMap<String, Vec<Vec<u8>>> {
    let mut serialized: HashMap<String, Vec<Vec<u8>>> = HashMap::new();
    for (name, value) in headers.iter() {
        serialized
            .entry(name.as_str().to_string())
            .or_default()
            .push(value.as_bytes().to_vec());
    }
    serialized
}

fn deserialize_headers(headers: HashMap<String, Vec<Vec<u8>>>) -> HeaderMap {
    let mut header_map = HeaderMap::new();
    for (name, values) in headers {
        let Ok(name) = HeaderName::try_from(name) else {
            continue;
        };
        for value in values {
            if let Ok(value) = HeaderValue::try_from(value) {
                header_map.append(name.clone(), value);
            }
        }
    }
    header_map
}

/// One unit read from the upstream response body by the durable task.
enum HttpBodyFrame {
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
enum ProducedChunk {
    /// A non-empty body chunk to hand to the guest.
    Data(Bytes),
    /// The recorded stream's terminal: there are no more chunks to deliver.
    End,
    /// A pending guest read was cancelled; finalize durability without
    /// delivering EOF to the guest-facing stream.
    Cancelled,
}

/// Reads the next meaningful frame from the upstream body, skipping empty data
/// frames so an empty frame is never persisted/delivered as a body chunk.
async fn read_http_body_frame(body: &mut UnsyncBoxBody<Bytes, ErrorCode>) -> HttpBodyFrame {
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
struct HttpConsumeBodyTask<Ctx> {
    body: UnsyncBoxBody<Bytes, ErrorCode>,
    demand_rx: mpsc::UnboundedReceiver<HttpBodyDemand>,
    trailers_tx: oneshot::Sender<HttpTrailersResolution>,
    /// Open-response state of the send that produced this response (its
    /// `outgoing-http-request` span, retry properties, and — for a replayed
    /// response — the send rebuild info), taken over from the response
    /// resource in `consume_body`. The span is finished (durably) right after
    /// the parent terminal, mirroring the P2 `end_http_request` span
    /// lifecycle. `None` for responses that did not come from `client::send`.
    response_state: Option<OpenP3HttpResponseState>,
    _phantom: PhantomData<fn() -> Ctx>,
}

impl<Ctx> HttpConsumeBodyTask<Ctx> {
    fn new(
        body: UnsyncBoxBody<Bytes, ErrorCode>,
        demand_rx: mpsc::UnboundedReceiver<HttpBodyDemand>,
        trailers_tx: oneshot::Sender<HttpTrailersResolution>,
        response_state: Option<OpenP3HttpResponseState>,
    ) -> Self {
        Self {
            body,
            demand_rx,
            trailers_tx,
            response_state,
            _phantom: PhantomData,
        }
    }
}

impl<Ctx, U> AccessorTask<U, DurableP3<Ctx>> for HttpConsumeBodyTask<Ctx>
where
    Ctx: WorkerCtx,
    U: 'static,
{
    async fn run(self, accessor: &Accessor<U, DurableP3<Ctx>>) -> wasmtime::Result<()> {
        let HttpConsumeBodyTask {
            mut body,
            mut demand_rx,
            trailers_tx,
            response_state,
            ..
        } = self;

        let (response_span, retry_properties, mut rebuild) = match response_state {
            Some(state) => (
                Some(state.span_id),
                Some(RetryContext::http(&state.method, &state.uri)),
                state.rebuild,
            ),
            None => (None, None, None),
        };
        // Keeps the re-issued request's I/O task alive while its body is read;
        // dropped (aborting the task) when this task finishes. Never read —
        // it exists only for its drop timing.
        let mut _rebuild_io_guard: Option<AbortOnDropIoTask> = None;
        // Set when the rebuild was refused (request body not reconstructable):
        // the resulting terminal error must not be routed through worker-level
        // retry, because a retry would replay into the same refusal forever.
        let mut rebuild_refused = false;

        // Open the parent batched scope. Children nest under its begin index.
        let mut parent = match CallHandle::<P3HttpClientConsumeBody, Cancellable>::start_access(
            accessor,
            durable_worker_ctx::<Ctx, U>,
            HostRequestNoInput {},
            DurableFunctionType::WriteRemoteBatched(None),
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
            let (demand, cancel_rx, read_cancel_ack) = match demand_rx.recv().await {
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
            let produced = if !child.is_live() {
                match child
                    .replay_access(accessor, durable_worker_ctx::<Ctx, U>)
                    .await
                {
                    Ok(CallReplayOutcome::Replayed(response)) => match response.chunk {
                        SerializableP3HttpBodyChunk::Data(bytes) => {
                            ProducedChunk::Data(Bytes::from(bytes))
                        }
                        SerializableP3HttpBodyChunk::End => ProducedChunk::End,
                        SerializableP3HttpBodyChunk::Cancelled => {
                            if let Some(cancel_rx) = cancel_rx {
                                let _ = cancel_rx.await;
                            }
                            cancel_ack = read_cancel_ack;
                            terminal = Ok(None);
                            ProducedChunk::Cancelled
                        }
                    },
                    Ok(CallReplayOutcome::Incomplete(mut child)) => {
                        // A batched (`WriteRemoteBatched(Some(..))`) child is not
                        // re-executable: `replay_access` hard-errors on an
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
                let frame = if let Some(pending_rebuild) = rebuild.take() {
                    // First live read of a replayed response's placeholder body:
                    // the durable consume-body scope turned out to be incomplete
                    // (the original run was interrupted mid-body-stream, so the
                    // scope claim jumped to live), and the placeholder carries no
                    // data. Re-issue the recorded request now and stream the
                    // fresh body instead. This only fires on a real guest demand:
                    // a dropped stream or a cleanly replaying scope never
                    // re-issues.
                    match reissue_recorded_request::<Ctx, U>(accessor, pending_rebuild).await {
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
                            rebuild_refused = true;
                            HttpBodyFrame::Error(ErrorCode::InternalError(Some(message)))
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
                };

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
                // recorded terminal replays as. A refused rebuild is never
                // retry-routed: its replay would hit the same refusal again.
                if let HttpBodyFrame::Error(error_code) = &frame
                    && !rebuild_refused
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

                if let Err(error) = child
                    .complete_access(
                        accessor,
                        durable_worker_ctx::<Ctx, U>,
                        HostResponseP3HttpClientConsumeBodyChunk { chunk },
                    )
                    .await
                {
                    // The child `Start` is already persisted but its `End` failed:
                    // the recorded chunk history is now incomplete. Fail the task
                    // loud rather than papering over it with a normal terminal and a
                    // completed parent marker, which would commit a malformed oplog.
                    // `complete_access` already finished the child handle without
                    // recording a `Cancelled` and its `TerminalCallError` carries the
                    // child scope's trap context, so preserve that error; we only need
                    // to abandon the still-open parent so it is not dropped unfinished
                    // (which would wrongly record a parent `Cancelled`).
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

                match frame {
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
                }
            };

            // Deliver the produced item to the guest-facing stream. This is the
            // single point where chunks reach the guest, identically live and on
            // replay, so the count/order of delivered chunks always matches the
            // count/order of persisted children.
            match produced {
                ProducedChunk::Data(bytes) => {
                    if demand.send(HttpBodyChunkReply::Data(bytes)).is_err() {
                        // The chunk was persisted but the producer vanished
                        // before it could be delivered. The recorded stream
                        // would diverge on replay (where the chunk *would* be
                        // delivered), so fail loud instead of finalizing the
                        // parent with a clean terminal over an undelivered chunk.
                        let trap_context = parent.trap_context();
                        parent.abandon_for_trap();
                        return fail_consume_body_task(
                            trailers_tx,
                            wasmtime::Error::from_anyhow(mark_durable_call_trap_context(
                                anyhow::Error::msg(
                                    "consume-body produced a body chunk without a pending demand",
                                ),
                                trap_context,
                            )),
                            Some(trap_context),
                        );
                    }
                }
                ProducedChunk::End => {
                    let (ack_tx, ack_rx) = oneshot::channel();
                    if demand.send(HttpBodyChunkReply::End { ack: ack_tx }).is_ok() {
                        // Wait for the producer to observe the terminal (report
                        // EOF to the guest) before resolving trailers / finalizing
                        // the parent, so trailers never surface before the body
                        // stream's terminal is observed.
                        let _ = ack_rx.await;
                    }
                    break;
                }
                ProducedChunk::Cancelled => {
                    let _ = demand.send(HttpBodyChunkReply::Cancelled);
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
        // Capture the parent scope's trap context first (it is a pure function of
        // the scope and survives the handle being consumed below) so every
        // finalize failure can tag the guest-facing trailers trap for correct
        // retry grouping.
        let parent_trap_context = parent.trap_context();
        let outcome = if parent.is_live() {
            match parent
                .complete_access(
                    accessor,
                    durable_worker_ctx::<Ctx, U>,
                    HostResponseP3HttpClientConsumeBodyResult {
                        result: serialize_consume_body_result(&terminal),
                    },
                )
                .await
            {
                Ok(response) => deserialize_consume_body_result(response.result),
                // `complete_access` consumed and finished the parent without
                // recording a `Cancelled`; its `TerminalCallError` carries the
                // parent scope's trap context, so preserve it.
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
                .replay_access(accessor, durable_worker_ctx::<Ctx, U>)
                .await
            {
                Ok(CallReplayOutcome::Replayed(response)) => {
                    deserialize_consume_body_result(response.result)
                }
                Ok(CallReplayOutcome::Incomplete(parent)) => {
                    match parent
                        .complete_access(
                            accessor,
                            durable_worker_ctx::<Ctx, U>,
                            HostResponseP3HttpClientConsumeBodyResult {
                                result: serialize_consume_body_result(&terminal),
                            },
                        )
                        .await
                    {
                        Ok(response) => deserialize_consume_body_result(response.result),
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
        // (live: append `FinishSpan`; replay: consume it) before resolving the
        // guest-facing trailers, so the entry's position is stable relative to
        // the parent terminal on both paths.
        if let Some(span_id) = response_span
            && let Err(error) =
                finish_span_access(accessor, durable_worker_ctx::<Ctx, U>, &span_id).await
        {
            return fail_consume_body_task(
                trailers_tx,
                wasmtime::Error::from_anyhow(mark_durable_call_trap_context(
                    anyhow::Error::from(error),
                    parent_trap_context,
                )),
                Some(parent_trap_context),
            );
        }

        let _ = trailers_tx.send(HttpTrailersResolution::Outcome(outcome));
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

        let (demand_tx, demand_rx) = mpsc::unbounded_channel();
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

        store.spawn(HttpConsumeBodyTask::<Ctx>::new(
            body,
            demand_rx,
            trailers_tx,
            response_state,
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
        // so the durable finish is deferred to the next drop-event drain point
        // (a deterministic replay point), mirroring P2's `end_http_request` on
        // response drop.
        {
            let mut store_ctx = store.as_context_mut();
            let ctx = durable_worker_ctx::<Ctx, U>(store_ctx.data_mut());
            if let Some(state) = ctx.state.open_p3_http_responses.remove(&res.rep())
                && let Some(sink) = ctx.state.dropped_call_event_sender()
            {
                let _ = sink.send(DropEvent::FinishSpan {
                    span_id: state.span_id,
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
    use core::pin::Pin;
    use core::task::{Context, Poll};
    use http_body_util::Full;
    use std::future::Future;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use test_r::{test, timeout};
    use wasmtime::{AsContextMut, Engine, Store};
    use wasmtime_wasi::ResourceTable;
    use wasmtime_wasi_http::p3::{WasiHttpCtxView, WasiHttpHooks};
    use wasmtime_wasi_http::{FieldMap, WasiHttpCtx};

    #[derive(Default)]
    struct TestHttpCtx {
        table: ResourceTable,
        ctx: WasiHttpCtx,
        hooks: TestHttpHooks,
    }

    #[derive(Default)]
    struct TestHttpHooks;

    impl WasiHttpHooks for TestHttpHooks {}

    impl WasiHttpView for TestHttpCtx {
        fn http(&mut self) -> WasiHttpCtxView<'_> {
            WasiHttpCtxView {
                hooks: &mut self.hooks,
                table: &mut self.table,
                ctx: &mut self.ctx,
            }
        }
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
                bytes: [b'x'],
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

    fn short_content_length_request() -> (
        wasmtime_wasi_http::p3::Request,
        impl Future<Output = Result<(), ErrorCode>> + Send + 'static,
    ) {
        let mut headers = HeaderMap::new();
        headers.insert(http::header::CONTENT_LENGTH, HeaderValue::from_static("4"));

        wasmtime_wasi_http::p3::Request::new(
            http::Method::POST,
            Some(http::uri::Scheme::HTTP),
            Some(http::uri::Authority::from_static("example.com")),
            Some(http::uri::PathAndQuery::from_static("/upload")),
            FieldMap::new_immutable(headers),
            None,
            Full::new(Bytes::from_static(b"x"))
                .map_err(|never| match never {})
                .boxed_unsync(),
        )
    }

    /// A request whose outgoing body deterministically fails with an
    /// `ErrorCode`, carrying no `content-length` header. Without a content-length
    /// validation wrapper, this error can only reach the guest through the
    /// transmission future, not through `into_http`'s content-length channel.
    fn erroring_body_request_without_content_length() -> (
        wasmtime_wasi_http::p3::Request,
        impl Future<Output = Result<(), ErrorCode>> + Send + 'static,
    ) {
        let body = http_body_util::StreamBody::new(futures::stream::once(async {
            Err::<http_body::Frame<Bytes>, ErrorCode>(ErrorCode::HttpProtocolError)
        }))
        .boxed_unsync();

        wasmtime_wasi_http::p3::Request::new(
            http::Method::POST,
            Some(http::uri::Scheme::HTTP),
            Some(http::uri::Authority::from_static("example.com")),
            Some(http::uri::PathAndQuery::from_static("/upload")),
            FieldMap::new_immutable(HeaderMap::new()),
            None,
            body,
        )
    }

    /// Every `SerializableHttpErrorCode` variant, each carrying a distinct
    /// payload so a mismatched arm between `serialize_error_code` and
    /// `deserialize_error_code` (or a dropped payload field) is detected.
    fn all_serializable_error_codes() -> Vec<SerializableHttpErrorCode> {
        use SerializableHttpErrorCode::*;
        vec![
            DnsTimeout,
            DnsError(SerializableDnsErrorPayload {
                rcode: Some("NXDOMAIN".to_string()),
                info_code: Some(3),
            }),
            DestinationNotFound,
            DestinationUnavailable,
            DestinationIpProhibited,
            DestinationIpUnroutable,
            ConnectionRefused,
            ConnectionTerminated,
            ConnectionTimeout,
            ConnectionReadTimeout,
            ConnectionWriteTimeout,
            ConnectionLimitReached,
            TlsProtocolError,
            TlsCertificateError,
            TlsAlertReceived(SerializableTlsAlertReceivedPayload {
                alert_id: Some(42),
                alert_message: Some("handshake failure".to_string()),
            }),
            HttpRequestDenied,
            HttpRequestLengthRequired,
            HttpRequestBodySize(Some(1024)),
            HttpRequestMethodInvalid,
            HttpRequestUriInvalid,
            HttpRequestUriTooLong,
            HttpRequestHeaderSectionSize(Some(8192)),
            HttpRequestHeaderSize(Some(SerializableFieldSizePayload {
                field_name: Some("authorization".to_string()),
                field_size: Some(64),
            })),
            HttpRequestTrailerSectionSize(Some(256)),
            HttpRequestTrailerSize(SerializableFieldSizePayload {
                field_name: Some("x-checksum".to_string()),
                field_size: Some(32),
            }),
            HttpResponseIncomplete,
            HttpResponseHeaderSectionSize(Some(4096)),
            HttpResponseHeaderSize(SerializableFieldSizePayload {
                field_name: Some("content-type".to_string()),
                field_size: Some(16),
            }),
            HttpResponseBodySize(Some(2048)),
            HttpResponseTrailerSectionSize(Some(128)),
            HttpResponseTrailerSize(SerializableFieldSizePayload {
                field_name: Some("x-trailer".to_string()),
                field_size: Some(8),
            }),
            HttpResponseTransferCoding(Some("chunked".to_string())),
            HttpResponseContentCoding(Some("gzip".to_string())),
            HttpResponseTimeout,
            HttpUpgradeFailed,
            HttpProtocolError,
            LoopDetected,
            ConfigurationError,
            InternalError(Some("boom".to_string())),
        ]
    }

    /// `deserialize_error_code` and `serialize_error_code` must be inverses so a
    /// transport `ErrorCode` recorded on the live path replays back to the guest
    /// unchanged. The roundtrip goes through the live p3 `ErrorCode` and back.
    #[test]
    fn error_code_conversion_roundtrips_through_p3() {
        for serializable in all_serializable_error_codes() {
            let roundtripped = serialize_error_code(&deserialize_error_code(serializable.clone()));
            assert_eq!(roundtripped, serializable);
        }
    }

    fn rebuild_head(
        method: SerializableHttpMethod,
        headers: HashMap<String, Vec<Vec<u8>>>,
    ) -> SerializableP3HttpClientSend {
        SerializableP3HttpClientSend {
            method,
            scheme: Some(SerializableP3HttpScheme::Http),
            authority: Some("localhost:1234".to_string()),
            path_with_query: Some("/stream?x=1".to_string()),
            headers,
            options: None,
        }
    }

    /// The rebuild body gate: only a head that positively declares a request
    /// body (`content-length` > 0 or unparseable, or any `transfer-encoding`)
    /// refuses the re-issue; absent or zero `content-length` allows it.
    #[test]
    fn recorded_head_body_detection() {
        let no_body = rebuild_head(SerializableHttpMethod::Get, HashMap::new());
        assert!(!recorded_head_declares_body(&no_body));

        let zero_length = rebuild_head(
            SerializableHttpMethod::Get,
            HashMap::from([("content-length".to_string(), vec![b"0".to_vec()])]),
        );
        assert!(!recorded_head_declares_body(&zero_length));

        let with_length = rebuild_head(
            SerializableHttpMethod::Post,
            HashMap::from([("content-length".to_string(), vec![b"42".to_vec()])]),
        );
        assert!(recorded_head_declares_body(&with_length));

        let unparseable_length = rebuild_head(
            SerializableHttpMethod::Post,
            HashMap::from([("content-length".to_string(), vec![b"not-a-number".to_vec()])]),
        );
        assert!(recorded_head_declares_body(&unparseable_length));

        let chunked = rebuild_head(
            SerializableHttpMethod::Post,
            HashMap::from([("transfer-encoding".to_string(), vec![b"chunked".to_vec()])]),
        );
        assert!(recorded_head_declares_body(&chunked));
    }

    /// The rebuilt request must carry the recorded head exactly — method,
    /// scheme, authority, path, guest headers — plus the re-derived
    /// Golem-managed headers replacing same-name guest values (as the live
    /// injection does), the recorded per-request timeout options, and an empty
    /// body.
    #[test]
    fn rebuilt_request_matches_recorded_head() {
        let mut head = rebuild_head(
            SerializableHttpMethod::Get,
            HashMap::from([
                ("x-test".to_string(), vec![b"guest".to_vec()]),
                ("idempotency-key".to_string(), vec![b"stale".to_vec()]),
            ]),
        );
        head.options = Some(SerializableP3HttpRequestOptions {
            connect_timeout_nanos: Some(1_000_000_000),
            first_byte_timeout_nanos: None,
            between_bytes_timeout_nanos: Some(2_000_000_000),
        });
        let rebuild = P3HttpSendRebuild {
            request: head,
            injected_headers: vec![
                ("idempotency-key".to_string(), "derived-key".to_string()),
                ("traceparent".to_string(), "00-abc-def-01".to_string()),
            ],
            recorded_status: 200,
        };

        let request = build_rebuilt_request(&rebuild).expect("rebuild request should build");

        assert_eq!(request.method, http::Method::GET);
        assert_eq!(request.scheme, Some(http::uri::Scheme::HTTP));
        assert_eq!(
            request.authority.as_ref().map(|a| a.as_str()),
            Some("localhost:1234")
        );
        assert_eq!(
            request.path_with_query.as_ref().map(|p| p.as_str()),
            Some("/stream?x=1")
        );

        let headers = &request.headers;
        assert_eq!(headers.get("x-test").unwrap(), "guest");
        // The injected value replaces the guest-set one.
        let idempotency_values: Vec<_> = headers.get_all("idempotency-key").iter().collect();
        assert_eq!(idempotency_values, vec!["derived-key"]);
        assert_eq!(headers.get("traceparent").unwrap(), "00-abc-def-01");

        let options = request.options.expect("recorded options should be carried");
        assert_eq!(
            options.connect_timeout,
            Some(std::time::Duration::from_secs(1))
        );
        assert_eq!(options.first_byte_timeout, None);
        assert_eq!(
            options.between_bytes_timeout,
            Some(std::time::Duration::from_secs(2))
        );
    }

    /// Response/trailer headers must replay with the same names, values, and
    /// per-name multiplicity. Header names are lower-cased by `http::HeaderName`,
    /// so the inputs here are already lower-case to make the roundtrip exact.
    #[test]
    fn headers_conversion_roundtrips() {
        let mut headers: HashMap<String, Vec<Vec<u8>>> = HashMap::new();
        headers.insert("content-type".to_string(), vec![b"text/plain".to_vec()]);
        headers.insert(
            "set-cookie".to_string(),
            vec![b"a=1".to_vec(), b"b=2".to_vec()],
        );
        let roundtripped = serialize_headers(&deserialize_headers(headers.clone()));
        assert_eq!(roundtripped, headers);
    }

    /// The `consume-body` terminal (clean trailers, absent trailers, or a body
    /// `ErrorCode`) must replay unchanged.
    #[test]
    fn consume_body_result_conversion_roundtrips() {
        let cases = vec![
            SerializableP3HttpConsumeBodyResult::Trailers(None),
            SerializableP3HttpConsumeBodyResult::Trailers(Some(HashMap::from([(
                "x-trailer".to_string(),
                vec![b"value".to_vec()],
            )]))),
            SerializableP3HttpConsumeBodyResult::HttpError(
                SerializableHttpErrorCode::ConnectionRefused,
            ),
        ];
        for result in cases {
            let roundtripped =
                serialize_consume_body_result(&deserialize_consume_body_result(result.clone()));
            assert_eq!(roundtripped, result);
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
}
