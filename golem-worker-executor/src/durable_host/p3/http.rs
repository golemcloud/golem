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
    P3HttpClientConsumeBody, P3HttpClientConsumeBodyChunk, P3HttpClientSend,
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
    HostResponseP3HttpClientSendResult, OplogIndex,
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
use wasmtime::component::{
    Access, Accessor, AccessorTask, Destination, FutureProducer, FutureReader, Resource,
    StreamProducer, StreamReader, StreamResult,
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
            Ok(HostRequestP3HttpClientSend { request })
        },
    )
    .await
    .map_err(HttpError::trap)?;

    let span_id = send_span.expect("p3 HTTP send request builder did not run");

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
                let result = replay_send_response::<Ctx, U>(store, response.result);
                match result {
                    Ok(response) => {
                        // The span stays open until the response body
                        // completes; hand it to the replayed response so the
                        // consume-body / drop paths consume the recorded
                        // `FinishSpan` at the same point it was written live.
                        register_response_span::<Ctx, U>(store, &response, span_id);
                        return Ok(response);
                    }
                    Err(error) => {
                        // A recorded send error closed the span live right
                        // after the `End`; consume its `FinishSpan` here.
                        finish_span_access(store, durable_worker_ctx::<Ctx, U>, &span_id)
                            .await
                            .map_err(HttpError::trap)?;
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
            // the P2 `end_http_request` span lifecycle.
            register_response_span::<Ctx, U>(store, &response, span_id);
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

/// Associates the `outgoing-http-request` span with the response resource
/// created by (a live or replayed) `client::send`. The span is finished when
/// the response body completes: the durable consume-body task takes ownership
/// of it in `consume_body`, or the response `drop` finishes it via a deferred
/// [`DropEvent::FinishSpan`] when the body was never consumed.
fn register_response_span<Ctx: WorkerCtx, U: Send>(
    store: &Accessor<U, DurableP3<Ctx>>,
    response: &Resource<Response>,
    span_id: SpanId,
) {
    let rep = response.rep();
    store.with(|mut access| {
        let ctx = durable_worker_ctx::<Ctx, U>(access.data_mut());
        ctx.state.open_p3_http_response_spans.insert(rep, span_id);
    });
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
/// response head is the authoritative `client::send` outcome — but it *is* the
/// guest-held request-body transmission result. Live `WasiHttp::send` wires the
/// transmission future to its request I/O result; on replay we wire it to the
/// drain result via a `oneshot` channel so a deterministic outgoing-body
/// failure (e.g. a guest trailers future that resolves to an `ErrorCode`, with
/// no `content-length` validation wrapper to surface it) replays to the guest
/// instead of an unconditional `Ok(())`.
///
/// This drain-derived result is a best-effort *interim*: the transmission
/// result is not recorded, and it is genuinely non-deterministic on live (it
/// depends on whether/how far the network read the body, which the recorded
/// `client::send` success/error does not capture). So replay can diverge from a
/// particular live run in either direction — a live transport error that
/// dropped the body unread resolves `Ok(())` whereas the replay drain surfaces
/// a body-validation error, and a non-deterministic mid-body network error
/// cannot be reproduced and replays as `Ok(())`. Recording the transmission
/// result itself is the follow-up that closes this gap; item #8 stays blocked
/// on it (see `request_body_transmission_result_depends_on_unrecorded_body_read`).
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
        let store = Access::<U, WasiHttp>::new(store.as_context_mut(), wasi_http_view::<Ctx, U>);
        <WasiHttp as types::HostRequestWithStore<U>>::new(
            store, headers, contents, trailers, options,
        )
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
        let store = Access::<U, WasiHttp>::new(store.as_context_mut(), wasi_http_view::<Ctx, U>);
        <WasiHttp as types::HostRequestWithStore<U>>::consume_body(store, req, fut)
    }

    fn drop(mut store: Access<U, Self>, req: Resource<Request>) -> wasmtime::Result<()> {
        observe_function_call_store::<Ctx, U>(
            store.as_context_mut().data_mut(),
            "http::types::request",
            "drop",
        );
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

/// A demand from the body stream producer to the durable [`HttpConsumeBodyTask`]
/// for the next body chunk, carrying the channel the task replies on.
type HttpBodyDemand = oneshot::Sender<HttpBodyChunkReply>;

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
    pending: Option<oneshot::Receiver<HttpBodyChunkReply>>,
    finished: bool,
}

impl DurableHttpBodyProducer {
    fn new(demand_tx: mpsc::UnboundedSender<HttpBodyDemand>) -> Self {
        Self {
            demand_tx,
            pending: None,
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

            if let Some(rx) = self.pending.as_mut() {
                match Pin::new(rx).poll(cx) {
                    Poll::Pending => {
                        // A demand is in flight: the task has been asked for
                        // (and will durably persist) exactly one chunk. We must
                        // deliver that chunk to a guest read rather than abandon
                        // it, otherwise the recorded child chunk would have no
                        // matching delivery and replay would diverge. So even
                        // when the guest is trying to cancel (`finish`), wait for
                        // the in-flight chunk instead of returning `Cancelled`.
                        // The demand is only ever issued from a positive-capacity
                        // poll (a deterministic guest read), so the demand/child
                        // sequence is identical on replay; the wait, however, is
                        // only bounded if the upstream eventually produces the
                        // frame (or closes) — a stalled upstream blocks
                        // cancellation, matching P2 blocking-read semantics.
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
                        self.pending = None;
                        self.finished = true;
                        // Acknowledge the terminal *before* reporting EOF so the
                        // task only resolves trailers after this stream observes
                        // the terminal. A dropped `ack` receiver just means the
                        // task is already gone, which is harmless here.
                        let _ = ack.send(());
                        return Poll::Ready(Ok(StreamResult::Dropped));
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
                // The guest is cancelling a read and we have nothing buffered
                // and no demand in flight: report a cancelled (empty) read
                // without starting a new durable body read.
                return Poll::Ready(Ok(StreamResult::Cancelled));
            }

            let (tx, rx) = oneshot::channel();
            if self.demand_tx.send(tx).is_err() {
                self.finished = true;
                return Poll::Ready(Err(wasmtime::Error::msg(
                    "consume-body durable task is gone",
                )));
            }
            self.pending = Some(rx);
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
}

/// One item produced by a single iteration of the durable consume-body loop —
/// after the chunk has been persisted (live) or replayed (replay) — to be
/// delivered to the guest-facing body stream.
enum ProducedChunk {
    /// A non-empty body chunk to hand to the guest.
    Data(Bytes),
    /// The recorded stream's terminal: there are no more chunks to deliver.
    Terminal,
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
    /// The `outgoing-http-request` span of the send that produced this
    /// response, taken over from the response resource in `consume_body`.
    /// Finished (durably) right after the parent terminal, mirroring the P2
    /// `end_http_request` span lifecycle. `None` for responses that did not
    /// come from `client::send`.
    response_span: Option<SpanId>,
    _phantom: PhantomData<fn() -> Ctx>,
}

impl<Ctx> HttpConsumeBodyTask<Ctx> {
    fn new(
        body: UnsyncBoxBody<Bytes, ErrorCode>,
        demand_rx: mpsc::UnboundedReceiver<HttpBodyDemand>,
        trailers_tx: oneshot::Sender<HttpTrailersResolution>,
        response_span: Option<SpanId>,
    ) -> Self {
        Self {
            body,
            demand_rx,
            trailers_tx,
            response_span,
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
            response_span,
            ..
        } = self;

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

        loop {
            let demand = demand_rx.recv().await;

            let child =
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
                        if let Some(reply_tx) = demand {
                            let _ = reply_tx.send(HttpBodyChunkReply::Failed {
                                message: error.to_string(),
                                trap_context,
                            });
                        }
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
                        SerializableP3HttpBodyChunk::End => ProducedChunk::Terminal,
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
                        if let Some(reply_tx) = demand {
                            let _ = reply_tx.send(HttpBodyChunkReply::Failed {
                                message: message.clone(),
                                trap_context,
                            });
                        }
                        return fail_consume_body_task(
                            trailers_tx,
                            wasmtime::Error::from_anyhow(parent.trap(anyhow::Error::msg(message))),
                            Some(trap_context),
                        );
                    }
                    Err(error) => {
                        let trap_context = parent.trap_context();
                        if let Some(reply_tx) = demand {
                            let _ = reply_tx.send(HttpBodyChunkReply::Failed {
                                message: error.to_string(),
                                trap_context,
                            });
                        }
                        return fail_consume_body_task(
                            trailers_tx,
                            wasmtime::Error::from_anyhow(parent.trap(error)),
                            Some(trap_context),
                        );
                    }
                }
            } else {
                // When the producer is already gone (guest dropped the stream) we
                // terminate the recorded stream with an `End` child instead of
                // reading more of the upstream body — and we must not start a new
                // upstream read whose persisted chunk could never be delivered.
                let producer_gone = demand
                    .as_ref()
                    .map(|reply_tx| reply_tx.is_closed())
                    .unwrap_or(true);
                let frame = if producer_gone {
                    HttpBodyFrame::End(None)
                } else {
                    read_http_body_frame(&mut body).await
                };

                let chunk = match &frame {
                    HttpBodyFrame::Data(bytes) => SerializableP3HttpBodyChunk::Data(bytes.to_vec()),
                    HttpBodyFrame::End(_) | HttpBodyFrame::Error(_) => {
                        SerializableP3HttpBodyChunk::End
                    }
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
                    if let Some(reply_tx) = demand {
                        let _ = reply_tx.send(HttpBodyChunkReply::Failed {
                            message: error.to_string(),
                            trap_context,
                        });
                    }
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
                        ProducedChunk::Terminal
                    }
                    HttpBodyFrame::Error(error) => {
                        terminal = Err(error);
                        ProducedChunk::Terminal
                    }
                }
            };

            // Deliver the produced item to the guest-facing stream. This is the
            // single point where chunks reach the guest, identically live and on
            // replay, so the count/order of delivered chunks always matches the
            // count/order of persisted children.
            match produced {
                ProducedChunk::Data(bytes) => match demand {
                    Some(reply_tx) => {
                        if reply_tx.send(HttpBodyChunkReply::Data(bytes)).is_err() {
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
                                        "consume-body persisted a body chunk that could not be \
                                         delivered to the guest stream",
                                    ),
                                    trap_context,
                                )),
                                Some(trap_context),
                            );
                        }
                    }
                    None => {
                        // A `Data` item is only ever produced in response to a
                        // demand, so a missing demand here is a protocol invariant
                        // violation rather than a clean stream end.
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
                },
                ProducedChunk::Terminal => {
                    if let Some(reply_tx) = demand {
                        let (ack_tx, ack_rx) = oneshot::channel();
                        if reply_tx
                            .send(HttpBodyChunkReply::End { ack: ack_tx })
                            .is_ok()
                        {
                            // Wait for the producer to observe the terminal (report
                            // EOF to the guest) before resolving trailers / finalizing
                            // the parent, so trailers never surface before the body
                            // stream's terminal is observed.
                            let _ = ack_rx.await;
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
        // A transient body-transfer error terminal (`Err` in `terminal`) is
        // deliberately *not* routed through worker-level retry here, unlike the
        // P2 `future_trailers::get` path: the send's `End` is already recorded,
        // so a retry would replay the response from its recorded headers (with
        // an empty body — the request is not re-issued) and then re-execute
        // this scope live against that empty body, silently delivering wrong
        // data to the guest. Routing body errors through retry requires the
        // mid-flight send rebuild (re-issuing the recorded request on replay)
        // and must land together with it; until then the error is recorded and
        // surfaced to the guest exactly as a replayed terminal would be.
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
        // Take ownership of the response's `outgoing-http-request` span (if
        // this response came from `client::send`): the durable consume-body
        // task finishes it when the body reaches its terminal. Removing the
        // mapping here also keeps the later `drop` of the response resource
        // from finishing it a second time.
        let response_span = {
            let mut store_ctx = store.as_context_mut();
            let ctx = durable_worker_ctx::<Ctx, U>(store_ctx.data_mut());
            ctx.state.open_p3_http_response_spans.remove(&res.rep())
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
                    // to the non-durable passthrough. No span was registered for
                    // such responses (`response_span` is `None` here).
                    debug_assert!(response_span.is_none());
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
            response_span,
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
            if let Some(span_id) = ctx.state.open_p3_http_response_spans.remove(&res.rep())
                && let Some(sink) = ctx.state.dropped_call_event_sender()
            {
                let _ = sink.send(DropEvent::FinishSpan { span_id });
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
}
