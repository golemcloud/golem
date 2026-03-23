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

//! Transparent HTTP request retry without worker restart.
//!
//! When an outgoing HTTP request fails with a transient error and the retry budget
//! allows more attempts, this module reconstructs the original request from oplog
//! metadata and re-sends it in-place — without trapping, without creating a new WASM
//! instance, and without replaying the oplog.
//!
//! # Zones
//!
//! - **Zone 1**: Retry at `FutureIncomingResponse::get()` — the response hasn't arrived yet,
//!   or arrived with an error. The outgoing body is fully finished.
//! - **Zone 2**: Retry during response body reading — the response was partially consumed.
//!   Requires re-sending the request and verifying the response prefix matches.

use crate::durable_host::durability::{DurableExecutionState, HostFailureKind};
use crate::durable_host::http::types::classify_http_error_code;
use crate::durable_host::HttpRequestState;
use crate::services::oplog::{Oplog, OplogOps};
use bytes::{BufMut, Bytes, BytesMut};
use golem_common::model::oplog::payload::HostPayloadPair;
use golem_common::model::oplog::types::SerializableHttpMethod;
use golem_common::model::oplog::{
    DurableFunctionType, HostRequestHttpRequest, HostResponse, OplogEntry, OplogIndex,
    PersistenceLevel,
};
use golem_common::model::RetryConfig;
use http::{HeaderName, HeaderValue};
use http_body_util::BodyExt;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;
use tracing::Instrument;
use wasmtime_wasi::{InputStream, OutputStream};
use wasmtime_wasi_http::bindings::http::types as wasi_http_types;
use wasmtime_wasi_http::body::{HostIncomingBody, HostOutgoingBody, HyperOutgoingBody, StreamContext};
use wasmtime_wasi_http::types::{
    default_send_request_with_pool, FutureIncomingResponseHandle, HostFutureIncomingResponse,
    OutgoingRequestConfig,
};
use wasmtime_wasi_http::HttpConnectionPool;

/// Reasons why an HTTP request is not eligible for transparent inline retry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InlineRetryIneligible {
    /// Worker is in replay mode (not live).
    NotLive,
    /// Worker is in snapshotting mode.
    Snapshotting,
    /// Persistence level is PersistNothing — no oplog data to reconstruct from.
    PersistNothing,
    /// The outgoing body used splice/blocking_splice, so bytes can't be reconstructed.
    UnreconstructableBody,
    /// The outgoing body included trailers, which are not persisted.
    HasOutgoingTrailers,
    /// The outgoing body is not yet finished (Zone 1 only).
    BodyNotFinished,
    /// The request method is not idempotent and assume_idempotence is false.
    NotIdempotent,
    /// The response body used skip/blocking_skip (Zone 2 only).
    HadBodySkip,
    /// The output stream had subscribe() called, so pollable may be stale after replacement.
    OutputStreamSubscribed,
}

/// Which retry zone is being attempted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetryZone {
    /// Retry at FutureIncomingResponse::get() — response not yet consumed.
    Zone1,
    /// Retry during response body reading — partial body already consumed.
    Zone2,
}

/// Checks whether the given HTTP request is eligible for transparent inline retry.
///
/// Returns `Ok(())` if eligible, or `Err(reason)` explaining why not.
pub fn is_http_inline_retry_eligible(
    exec_state: &DurableExecutionState,
    request_state: &HttpRequestState,
    zone: RetryZone,
) -> Result<(), InlineRetryIneligible> {
    if !exec_state.is_live {
        return Err(InlineRetryIneligible::NotLive);
    }

    if exec_state.snapshotting_mode.is_some() {
        return Err(InlineRetryIneligible::Snapshotting);
    }

    if exec_state.persistence_level == PersistenceLevel::PersistNothing {
        return Err(InlineRetryIneligible::PersistNothing);
    }

    if request_state.has_unreconstructable_body {
        return Err(InlineRetryIneligible::UnreconstructableBody);
    }

    if request_state.output_stream_subscribed {
        return Err(InlineRetryIneligible::OutputStreamSubscribed);
    }

    if zone == RetryZone::Zone2 && request_state.had_body_skip {
        return Err(InlineRetryIneligible::HadBodySkip);
    }

    // Idempotency check
    if !exec_state.assume_idempotence && !is_method_idempotent(&request_state.request.method) {
        return Err(InlineRetryIneligible::NotIdempotent);
    }

    Ok(())
}

/// Returns true if the HTTP method is inherently idempotent (safe to retry
/// even when `assume_idempotence` is false).
fn is_method_idempotent(method: &SerializableHttpMethod) -> bool {
    matches!(
        method,
        SerializableHttpMethod::Get
            | SerializableHttpMethod::Head
            | SerializableHttpMethod::Put
            | SerializableHttpMethod::Delete
            | SerializableHttpMethod::Options
    )
}

/// Reconstructs the outgoing request body by scanning oplog entries in
/// `[begin_index..current_oplog_index]` for body write entries belonging
/// to this request's batch.
///
/// Reads `StreamWriteWithBytes` responses from the oplog and concatenates
/// all successfully written byte chunks to reconstruct the full body.
pub async fn reconstruct_outgoing_body(
    oplog: &Arc<dyn Oplog>,
    begin_index: OplogIndex,
) -> Result<Bytes, anyhow::Error> {
    let current_idx = oplog.current_oplog_index().await;

    if current_idx <= begin_index {
        return Ok(Bytes::new());
    }

    let n: u64 = Into::<u64>::into(current_idx) - Into::<u64>::into(begin_index);
    let entries = oplog.read_many(begin_index, n).await;
    let mut body = BytesMut::new();

    let write_fn_name =
        golem_common::model::oplog::host_functions::HttpTypesOutgoingBodyStreamWrite::HOST_FUNCTION_NAME;
    let write_zeroes_fn_name =
        golem_common::model::oplog::host_functions::HttpTypesOutgoingBodyStreamWriteZeroes::HOST_FUNCTION_NAME;

    for (_idx, entry) in &entries {
        if let OplogEntry::HostCall {
            function_name,
            response,
            durable_function_type: DurableFunctionType::WriteRemoteBatched(Some(batch_begin)),
            ..
        } = entry
        {
            if batch_begin != &begin_index {
                continue;
            }

            if *function_name == write_fn_name || *function_name == write_zeroes_fn_name {
                let response_value =
                    oplog.download_payload(response.clone()).await.map_err(|err| {
                        anyhow::anyhow!("failed to download outgoing body chunk payload: {err}")
                    })?;

                if let HostResponse::StreamWriteWithBytes(payload) = response_value {
                    if let Ok(data) = &payload.result {
                        body.put_slice(data);
                    }
                }
            }
        }
    }

    Ok(body.freeze())
}

/// Reads all successfully received incoming body chunks from the oplog for
/// a given request batch. Used by Zone 2 to determine how many bytes the guest
/// has already consumed.
///
/// Returns the flattened bytes of all successful chunks.
pub async fn read_incoming_body_chunks(
    oplog: &Arc<dyn Oplog>,
    begin_index: OplogIndex,
) -> Result<Bytes, anyhow::Error> {
    let current_idx = oplog.current_oplog_index().await;

    if current_idx <= begin_index {
        return Ok(Bytes::new());
    }

    let n: u64 = Into::<u64>::into(current_idx) - Into::<u64>::into(begin_index);
    let entries = oplog.read_many(begin_index, n).await;
    let mut chunks = BytesMut::new();

    let read_fn_name =
        golem_common::model::oplog::host_functions::HttpTypesIncomingBodyStreamRead::HOST_FUNCTION_NAME;
    let blocking_read_fn_name =
        golem_common::model::oplog::host_functions::HttpTypesIncomingBodyStreamBlockingRead::HOST_FUNCTION_NAME;

    for (_idx, entry) in &entries {
        if let OplogEntry::HostCall {
            function_name,
            response,
            durable_function_type: DurableFunctionType::WriteRemoteBatched(Some(batch_begin)),
            ..
        } = entry
        {
            if batch_begin != &begin_index {
                continue;
            }

            if *function_name == read_fn_name || *function_name == blocking_read_fn_name {
                let response_value =
                    oplog.download_payload(response.clone()).await.map_err(|err| {
                        anyhow::anyhow!("failed to download incoming body chunk payload: {err}")
                    })?;

                if let HostResponse::StreamChunk(payload) = response_value {
                    if let Ok(data) = &payload.result {
                        chunks.put_slice(data);
                    }
                }
            }
        }
    }

    Ok(chunks.freeze())
}

/// Builds a `hyper::Request` from the stored HTTP request metadata and
/// reconstructed body bytes.
///
/// The request exactly reproduces the original: same URI, method, headers,
/// and body content. Headers are `Vec<(String, Vec<u8>)>` preserving
/// duplicates and byte-level fidelity.
///
/// For Zone 2 resumption, `extra_headers` can include a `Range` header.
pub fn reconstruct_http_request(
    request: &HostRequestHttpRequest,
    body: HyperOutgoingBody,
    extra_headers: &[(String, String)],
) -> Result<hyper::Request<HyperOutgoingBody>, anyhow::Error> {
    let method = http::Method::try_from(&request.method)?;
    let uri: hyper::Uri = request.uri.parse().map_err(|e| {
        anyhow::anyhow!("failed to parse stored URI '{}': {e}", request.uri)
    })?;

    let mut builder = hyper::Request::builder().method(method).uri(uri);

    // Replay stored headers exactly
    for (name, value) in &request.headers {
        let header_name = HeaderName::from_str(name)
            .map_err(|e| anyhow::anyhow!("invalid stored header name '{name}': {e}"))?;
        let header_value = HeaderValue::from_str(value)
            .map_err(|e| anyhow::anyhow!("invalid stored header value for '{name}': {e}"))?;
        builder = builder.header(header_name, header_value);
    }

    // Add any extra headers (e.g., Range for Zone 2)
    for (name, value) in extra_headers {
        let header_name = HeaderName::from_str(name)
            .map_err(|e| anyhow::anyhow!("invalid extra header name '{name}': {e}"))?;
        let header_value = HeaderValue::from_str(value)
            .map_err(|e| anyhow::anyhow!("invalid extra header value for '{name}': {e}"))?;
        builder = builder.header(header_name, header_value);
    }

    builder
        .body(body)
        .map_err(|e| anyhow::anyhow!("failed to build reconstructed HTTP request: {e}"))
}

/// Convenience wrapper for `reconstruct_http_request` that takes raw body
/// bytes and wraps them in a `Full<Bytes>` body.
pub fn reconstruct_http_request_full(
    request: &HostRequestHttpRequest,
    body_bytes: Bytes,
    extra_headers: &[(String, String)],
) -> Result<hyper::Request<HyperOutgoingBody>, anyhow::Error> {
    let body: HyperOutgoingBody = http_body_util::Full::new(body_bytes)
        .map_err(|_| unreachable!("Infallible error"))
        .boxed_unsync();
    reconstruct_http_request(request, body, extra_headers)
}

/// Sends a reconstructed HTTP request using the connection pool.
///
/// This bypasses the normal `WasiHttpView::send_request` path and directly
/// calls `default_send_request_with_pool`, producing a `HostFutureIncomingResponse`
/// that can be used to replace the failed one in-place.
pub fn send_reconstructed_request(
    request: hyper::Request<HyperOutgoingBody>,
    config: OutgoingRequestConfig,
    connection_pool: Option<HttpConnectionPool>,
) -> HostFutureIncomingResponse {
    default_send_request_with_pool(request, config, None, connection_pool)
}

/// Verifies that the first `expected_prefix.len()` bytes from `actual_data`
/// match the previously consumed body bytes.
///
/// This is used by Zone 2 to confirm that the re-sent request produced the
/// same response body prefix before resuming streaming to the guest.
///
/// The comparison is flattened — chunk boundaries do not need to match,
/// only the byte content.
///
/// Returns `true` if the prefix matches exactly, `false` otherwise.
pub fn verify_body_prefix(expected_prefix: &[u8], actual_data: &[u8]) -> bool {
    if actual_data.len() < expected_prefix.len() {
        return false;
    }
    actual_data[..expected_prefix.len()] == *expected_prefix
}

/// Result of rebuilding a streaming HTTP request for output stream retry.
///
/// Contains the new resources that should replace the old ones in the
/// ResourceTable via `get_mut()` in-place replacement.
pub struct RebuiltStreamingRequest {
    /// The new `HostFutureIncomingResponse` to replace the old one.
    pub future: HostFutureIncomingResponse,
    /// The new `HostOutgoingBody` to replace the old one.
    pub outgoing_body: HostOutgoingBody,
    /// The new output stream (boxed) to replace the old one.
    pub output_stream: Box<dyn OutputStream>,
}

/// Rebuilds an HTTP request as a streaming request for output stream retry.
///
/// This reconstructs all prior body bytes from the oplog, creates a fresh
/// outgoing body+stream pair, writes the prior bytes into the new stream,
/// and sends the request. The caller receives a `RebuiltStreamingRequest`
/// whose fields can replace the guest's existing resource table entries.
///
/// Unlike `send_reconstructed_request()` (which sends the body as a complete
/// `Full<Bytes>`), this creates a streaming body so the guest can continue
/// writing additional data after the retry.
pub async fn rebuild_streaming_request(
    oplog: &Arc<dyn Oplog>,
    request_state: &HttpRequestState,
    config: OutgoingRequestConfig,
    connection_pool: Option<HttpConnectionPool>,
) -> Result<RebuiltStreamingRequest, anyhow::Error> {
    // 1. Reconstruct all prior successful body bytes from oplog
    let prior_bytes = reconstruct_outgoing_body(oplog, request_state.begin_index).await?;

    // 2. Create a fresh outgoing body with a streaming body pair
    let (mut new_outgoing_body, hyper_body) =
        HostOutgoingBody::new(StreamContext::Request, None, 1, 1024 * 1024);

    // 3. Take the output stream from the new body
    let mut new_stream = new_outgoing_body
        .take_output_stream()
        .ok_or_else(|| anyhow::anyhow!("failed to take output stream from new outgoing body"))?;

    // 4. Write all prior body bytes into the new stream using raw OutputStream
    if !prior_bytes.is_empty() {
        // Write in chunks respecting the stream's write budget
        let mut offset = 0;
        while offset < prior_bytes.len() {
            let budget = new_stream
                .check_write()
                .map_err(|e| anyhow::anyhow!("check_write failed during body replay: {e}"))?;

            if budget == 0 {
                // Stream is full, need to flush/wait — use ready() to wait for capacity
                new_stream
                    .ready()
                    .await;
                continue;
            }

            let end = std::cmp::min(offset + budget, prior_bytes.len());
            let chunk = Bytes::copy_from_slice(&prior_bytes[offset..end]);
            new_stream
                .write(chunk)
                .map_err(|e| anyhow::anyhow!("write failed during body replay: {e}"))?;
            offset = end;
        }
    }

    // 5. Build the HTTP request with the streaming body
    let reconstructed = reconstruct_http_request(&request_state.request, hyper_body, &[])?;

    // 6. Send the request (the body is streaming — hyper will read from the channel
    //    as the guest continues writing)
    let new_future = default_send_request_with_pool(
        reconstructed,
        config,
        None,
        connection_pool,
    );

    Ok(RebuiltStreamingRequest {
        future: new_future,
        outgoing_body: new_outgoing_body,
        output_stream: new_stream,
    })
}

/// Spawns a retry-aware HTTP request task that wraps an existing pending request.
///
/// The returned handle awaits the original request first. If it succeeds or
/// fails with a permanent error, the result is returned directly. If it fails
/// with a transient `ErrorCode`, the task enters `in_task_retry_loop` to
/// reconstruct the request from the oplog and retry transparently.
///
/// This function is the HTTP equivalent of `spawn_rpc_task_with_retry` in
/// `wasm_rpc/mod.rs`.
pub fn spawn_http_request_with_retry(
    original_handle: FutureIncomingResponseHandle,
    request: HostRequestHttpRequest,
    config: OutgoingRequestConfig,
    connection_pool: Option<HttpConnectionPool>,
    oplog: Arc<dyn crate::services::oplog::Oplog>,
    retry_config: RetryConfig,
    max_delay: Duration,
    begin_index: OplogIndex,
) -> FutureIncomingResponseHandle {
    // Capture config fields individually since OutgoingRequestConfig is not Clone
    let use_tls = config.use_tls;
    let connect_timeout = config.connect_timeout;
    let first_byte_timeout = config.first_byte_timeout;
    let between_bytes_timeout = config.between_bytes_timeout;

    wasmtime_wasi::runtime::spawn(
        async move {
            // Await the original (first attempt) request
            let first_result = original_handle.await;

            match first_result {
                // Wasmtime-level trap: propagate immediately
                Err(trap) => Err(trap),

                // Successful response: return it
                Ok(Ok(resp)) => Ok(Ok(resp)),

                // Permanent HTTP error: return as-is
                Ok(Err(ref code))
                    if classify_http_error_code(code) == HostFailureKind::Permanent =>
                {
                    first_result
                }

                // Transient HTTP error: enter retry loop
                Ok(Err(_initial_error)) => {
                    let result = crate::durable_host::durability::in_task_retry_loop(
                        retry_config,
                        max_delay,
                        oplog.clone(),
                        begin_index,
                        0,
                        classify_http_error_code,
                        || {
                            let oplog = oplog.clone();
                            let request = request.clone();
                            let connection_pool = connection_pool.clone();
                            async move {
                                // Reconstruct body from oplog
                                let body_bytes = reconstruct_outgoing_body(&oplog, begin_index)
                                    .await
                                    .map_err(|e| {
                                        wasi_http_types::ErrorCode::InternalError(Some(format!(
                                            "body reconstruction failed: {e}"
                                        )))
                                    })?;

                                // Build the request
                                let http_request =
                                    reconstruct_http_request_full(&request, body_bytes, &[])
                                        .map_err(|e| {
                                            wasi_http_types::ErrorCode::InternalError(Some(
                                                format!("request reconstruction failed: {e}"),
                                            ))
                                        })?;

                                let retry_config = OutgoingRequestConfig {
                                    use_tls,
                                    connect_timeout,
                                    first_byte_timeout,
                                    between_bytes_timeout,
                                };

                                // Send and await the response
                                let mut future_resp = default_send_request_with_pool(
                                    http_request,
                                    retry_config,
                                    None,
                                    connection_pool,
                                );

                                // Wait for the response to be ready
                                use wasmtime_wasi::Pollable;
                                future_resp.ready().await;

                                // Extract the result
                                match future_resp.unwrap_ready() {
                                    Ok(result) => result,
                                    Err(trap) => Err(wasi_http_types::ErrorCode::InternalError(
                                        Some(format!("request failed with trap: {trap}")),
                                    )),
                                }
                            }
                        },
                    )
                    .await;

                    Ok(result)
                }
            }
        }
        .in_current_span(),
    )
}

/// Attempts a single inline retry cycle for an outgoing body stream write failure.
///
/// This is called when write/flush/write_zeroes fails with a transient error.
/// It reconstructs the request from the oplog, replays all prior body bytes into
/// a new streaming body, sends the request, and swaps the resources in-place.
///
/// Returns `Ok(true)` if retry succeeded (resources swapped, caller should re-attempt the write),
/// `Ok(false)` if retry is not eligible,
/// `Err` if retry failed fatally.
pub async fn try_output_stream_inline_retry<Ctx: crate::workerctx::WorkerCtx>(
    ctx: &mut crate::durable_host::DurableWorkerCtx<Ctx>,
    stream_rep: u32,
) -> Result<bool, anyhow::Error> {
    use crate::durable_host::DurabilityHost;
    use crate::services::HasOplog;
    use wasmtime::component::Resource;
    use wasmtime_wasi_http::bindings::http::types::FutureIncomingResponse;
    use wasmtime_wasi_http::bindings::http::types::OutgoingBody;
    use wasmtime_wasi_http::body::HostOutgoingBody as HostOutgoingBodyType;
    use wasmtime_wasi_http::types::HostFutureIncomingResponse;

    // 1. Find the request handle and state
    let request_handle = match ctx.state.find_request_handle_by_output_stream(stream_rep) {
        Some(h) => h,
        None => return Ok(false),
    };

    let request_state = match ctx.state.open_http_requests.get(&request_handle) {
        Some(s) => s.clone(),
        None => return Ok(false),
    };

    // 2. Check eligibility
    let exec_state = ctx.durable_execution_state();
    if is_http_inline_retry_eligible(&exec_state, &request_state, RetryZone::Zone1).is_err() {
        return Ok(false);
    }

    // 3. Get connection pool and oplog
    let oplog = ctx.public_state.oplog();
    let connection_pool = ctx.wasi_http.connection_pool.clone();
    let config = request_state.outgoing_request_config();

    // 4. Rebuild the streaming request
    let rebuilt = rebuild_streaming_request(&oplog, &request_state, config, connection_pool).await?;

    // 5. Swap resources in the resource table
    // Swap the FutureIncomingResponse — re-wrap with background retry if the
    // original request had it, so that transient errors at get() are still handled.
    let new_future = if request_state.has_background_retry {
        if let HostFutureIncomingResponse::Pending(handle) = rebuilt.future {
            let retry_handle = spawn_http_request_with_retry(
                handle,
                request_state.request.clone(),
                request_state.outgoing_request_config(),
                ctx.wasi_http.connection_pool.clone(),
                oplog.clone(),
                ctx.retry_config(),
                exec_state.max_in_function_retry_delay,
                request_state.begin_index,
            );
            HostFutureIncomingResponse::pending(retry_handle)
        } else {
            rebuilt.future
        }
    } else {
        rebuilt.future
    };
    let future_res: &mut HostFutureIncomingResponse = ctx
        .table()
        .get_mut(&Resource::<FutureIncomingResponse>::new_borrow(request_handle))?;
    *future_res = new_future;

    // Swap the OutgoingBody if we have a rep
    if let Some(body_rep) = request_state.outgoing_body_rep {
        let body_entry: &mut HostOutgoingBodyType = ctx
            .table()
            .get_mut(&Resource::<OutgoingBody>::new_borrow(body_rep))?;
        *body_entry = rebuilt.outgoing_body;
    }

    // Swap the OutputStream
    use wasmtime_wasi::p2::bindings::io::streams::OutputStream as WasiOutputStream;

    let stream_entry: &mut wasmtime_wasi::DynOutputStream = ctx
        .table()
        .get_mut(&Resource::<WasiOutputStream>::new_borrow(stream_rep))?;
    *stream_entry = rebuilt.output_stream;

    tracing::debug!(
        request_handle = request_handle,
        stream_rep = stream_rep,
        "Output stream inline retry: successfully rebuilt and swapped resources"
    );

    Ok(true)
}

/// Attempts Zone 2 inline retry for a response body stream read failure.
///
/// When reading response body bytes fails with a transient error, this function:
/// 1. Checks Zone 2 eligibility (no prior skip, etc.)
/// 2. Calculates bytes already delivered to the guest from oplog
/// 3. Reconstructs the outgoing request with a Range header
/// 4. Sends the request and handles 206/200/416 responses
/// 5. Swaps the InputStream to the new response's body stream
///
/// Returns `Ok(true)` if retry succeeded (stream swapped, caller should re-attempt read),
/// `Ok(false)` if retry is not eligible or conditions not met,
/// `Err` with a StreamError if content mismatch detected.
pub async fn try_zone2_inline_retry<Ctx: crate::workerctx::WorkerCtx>(
    ctx: &mut crate::durable_host::DurableWorkerCtx<Ctx>,
    stream_handle: u32,
) -> Result<bool, anyhow::Error> {
    use crate::durable_host::DurabilityHost;
    use crate::services::HasOplog;
    use wasmtime::component::Resource;
    use wasmtime_wasi::p2::bindings::io::streams::InputStream as WasiInputStream;
    use wasmtime_wasi_http::bindings::http::types::IncomingBody as WasiIncomingBody;

    // 1. Find the request state via the stream handle.
    //    The stream rep IS the request tracking handle for incoming body streams.
    let request_state = match ctx.state.open_http_requests.get(&stream_handle) {
        Some(s) => s.clone(),
        None => return Ok(false),
    };

    // Zone 2 requires a tracked IncomingBody handle to properly swap body+stream
    let body_handle = match request_state.body_handle {
        Some(h) => h,
        None => return Ok(false),
    };

    // 2. Check Zone 2 eligibility
    let exec_state = ctx.durable_execution_state();
    if is_http_inline_retry_eligible(&exec_state, &request_state, RetryZone::Zone2).is_err() {
        return Ok(false);
    }

    // 3. Calculate bytes already delivered to the guest from the oplog
    let oplog = ctx.public_state.oplog();
    let already_consumed = read_incoming_body_chunks(&oplog, request_state.begin_index).await?;
    let consumed_len = already_consumed.len();

    // 4. Reconstruct the outgoing request body from the oplog
    let body_bytes = reconstruct_outgoing_body(&oplog, request_state.begin_index).await?;

    // 5. Build the request, adding a Range header if bytes were already consumed.
    //    If the original request already has a Range header, Zone 2 is not supported
    //    because composing Range headers correctly is complex.
    let has_range = request_state
        .request
        .headers
        .iter()
        .any(|(k, _)| k.eq_ignore_ascii_case("range"));
    if has_range {
        return Ok(false);
    }
    let extra_headers = if consumed_len > 0 {
        vec![("range".to_string(), format!("bytes={consumed_len}-"))]
    } else {
        vec![]
    };
    let http_request =
        reconstruct_http_request_full(&request_state.request, body_bytes, &extra_headers)?;

    // 6. Send the reconstructed request
    let config = request_state.outgoing_request_config();
    let connection_pool = ctx.wasi_http.connection_pool.clone();
    let mut future_resp = default_send_request_with_pool(http_request, config, None, connection_pool);

    use wasmtime_wasi::Pollable;
    future_resp.ready().await;

    let response = match future_resp.unwrap_ready() {
        Ok(Ok(resp)) => resp,
        Ok(Err(_error_code)) => return Ok(false),
        Err(_trap) => return Ok(false),
    };

    let status = response.resp.status().as_u16();
    let between_bytes_timeout = response.between_bytes_timeout;

    // 7. Handle response status
    match status {
        206 => {
            // Partial Content — verify Content-Range header matches consumed_len
            let content_range = response
                .resp
                .headers()
                .get("content-range")
                .and_then(|v| v.to_str().ok())
                .map(|s| s.to_owned());

            let range_start = content_range.as_deref().and_then(parse_content_range_start);

            match range_start {
                Some(start) if start == consumed_len as u64 => {
                    // Range matches — swap body+stream
                    let (_parts, body) = response.resp.into_parts();
                    let new_body =
                        HostIncomingBody::new(body, between_bytes_timeout, usize::MAX);

                    // Swap IncomingBody at body_handle, then take stream from it
                    let body_entry: &mut HostIncomingBody = ctx
                        .table()
                        .get_mut(&Resource::<WasiIncomingBody>::new_borrow(body_handle))?;
                    *body_entry = new_body;
                    let new_stream = body_entry.take_stream().ok_or_else(|| {
                        anyhow::anyhow!("HTTP retry failed: could not take stream from new body")
                    })?;

                    // Swap the InputStream in the resource table
                    let stream_entry: &mut wasmtime_wasi::DynInputStream = ctx
                        .table()
                        .get_mut(&Resource::<WasiInputStream>::new_borrow(stream_handle))?;
                    *stream_entry = new_stream;

                    tracing::debug!(
                        stream_handle = stream_handle,
                        consumed_len = consumed_len,
                        "Zone 2 inline retry: 206 Partial Content, body+stream swapped"
                    );
                    Ok(true)
                }
                _ => {
                    // Content-Range doesn't match or missing — fall back
                    tracing::debug!(
                        stream_handle = stream_handle,
                        content_range = ?content_range,
                        consumed_len = consumed_len,
                        "Zone 2 inline retry: 206 Content-Range mismatch, falling back"
                    );
                    Ok(false)
                }
            }
        }
        200 if consumed_len == 0 => {
            // Full response with nothing consumed yet — swap body+stream directly
            let (_parts, body) = response.resp.into_parts();
            let new_body = HostIncomingBody::new(body, between_bytes_timeout, usize::MAX);

            let body_entry: &mut HostIncomingBody = ctx
                .table()
                .get_mut(&Resource::<WasiIncomingBody>::new_borrow(body_handle))?;
            *body_entry = new_body;
            let new_stream = body_entry.take_stream().ok_or_else(|| {
                anyhow::anyhow!("HTTP retry failed: could not take stream from new body")
            })?;

            let stream_entry: &mut wasmtime_wasi::DynInputStream = ctx
                .table()
                .get_mut(&Resource::<WasiInputStream>::new_borrow(stream_handle))?;
            *stream_entry = new_stream;

            tracing::debug!(
                stream_handle = stream_handle,
                "Zone 2 inline retry: 200 OK (no bytes consumed), body+stream swapped"
            );
            Ok(true)
        }
        200 => {
            // Full response — need to verify consumed prefix then swap
            let (_parts, body) = response.resp.into_parts();
            let new_body = HostIncomingBody::new(body, between_bytes_timeout, usize::MAX);

            // Swap IncomingBody at body_handle first
            let body_entry: &mut HostIncomingBody = ctx
                .table()
                .get_mut(&Resource::<WasiIncomingBody>::new_borrow(body_handle))?;
            *body_entry = new_body;
            let mut new_stream = body_entry.take_stream().ok_or_else(|| {
                anyhow::anyhow!("HTTP retry failed: could not take stream from new body")
            })?;

            // Read and verify consumed_len bytes from the new stream
            let mut verified = 0usize;
            while verified < consumed_len {
                // Wait for data to be available
                new_stream.ready().await;

                let remaining = consumed_len - verified;
                let chunk = new_stream.read(remaining).map_err(|e| match e {
                    wasmtime_wasi::StreamError::Closed => anyhow::anyhow!(
                        "HTTP retry failed: response shorter than previously consumed bytes"
                    ),
                    wasmtime_wasi::StreamError::LastOperationFailed(e) => anyhow::anyhow!(
                        "HTTP retry failed: error reading prefix for verification: {e}"
                    ),
                    wasmtime_wasi::StreamError::Trap(e) => anyhow::anyhow!(
                        "HTTP retry failed: trap reading prefix for verification: {e}"
                    ),
                })?;

                if chunk.is_empty() {
                    // No data yet, will retry after ready()
                    continue;
                }

                let to_check = chunk.len().min(remaining);
                if chunk[..to_check] != already_consumed[verified..verified + to_check] {
                    return Err(anyhow::anyhow!(
                        "HTTP retry failed: response content changed between attempts"
                    ));
                }
                verified += to_check;
            }

            // Prefix verified — swap the stream (which now has the remaining body)
            let stream_entry: &mut wasmtime_wasi::DynInputStream = ctx
                .table()
                .get_mut(&Resource::<WasiInputStream>::new_borrow(stream_handle))?;
            *stream_entry = new_stream;

            tracing::debug!(
                stream_handle = stream_handle,
                consumed_len = consumed_len,
                "Zone 2 inline retry: 200 OK with prefix verification, body+stream swapped"
            );
            Ok(true)
        }
        416 => {
            // Range Not Satisfiable — content changed
            Err(anyhow::anyhow!(
                "HTTP retry failed: server returned 416 Range Not Satisfiable"
            ))
        }
        _ => {
            // Unexpected status — don't retry
            tracing::debug!(
                stream_handle = stream_handle,
                status = status,
                "Zone 2 inline retry: unexpected status code, falling back"
            );
            Ok(false)
        }
    }
}

/// Parses the start byte position from a Content-Range header value.
///
/// Expected format: `bytes <start>-<end>/<total>` or `bytes <start>-<end>/*`
/// Returns the start position if successfully parsed.
fn parse_content_range_start(value: &str) -> Option<u64> {
    let rest = value.strip_prefix("bytes ")?;
    let dash_pos = rest.find('-')?;
    rest[..dash_pos].parse::<u64>().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_r::test;

    #[test]
    fn test_is_method_idempotent() {
        assert!(is_method_idempotent(&SerializableHttpMethod::Get));
        assert!(is_method_idempotent(&SerializableHttpMethod::Head));
        assert!(is_method_idempotent(&SerializableHttpMethod::Put));
        assert!(is_method_idempotent(&SerializableHttpMethod::Delete));
        assert!(is_method_idempotent(&SerializableHttpMethod::Options));
        assert!(!is_method_idempotent(&SerializableHttpMethod::Post));
        assert!(!is_method_idempotent(&SerializableHttpMethod::Patch));
        assert!(!is_method_idempotent(&SerializableHttpMethod::Connect));
        assert!(!is_method_idempotent(&SerializableHttpMethod::Trace));
        assert!(!is_method_idempotent(
            &SerializableHttpMethod::Other("CUSTOM".to_string())
        ));
    }

    #[test]
    fn test_verify_body_prefix_exact_match() {
        let expected = b"hello world";
        let actual = b"hello world and more";
        assert!(verify_body_prefix(expected, actual));
    }

    #[test]
    fn test_verify_body_prefix_mismatch() {
        let expected = b"hello world";
        let actual = b"hello worlX and more";
        assert!(!verify_body_prefix(expected, actual));
    }

    #[test]
    fn test_verify_body_prefix_too_short() {
        let expected = b"hello world";
        let actual = b"hello";
        assert!(!verify_body_prefix(expected, actual));
    }

    #[test]
    fn test_verify_body_prefix_empty() {
        assert!(verify_body_prefix(b"", b"anything"));
        assert!(verify_body_prefix(b"", b""));
    }

    #[test]
    fn test_parse_content_range_start_standard() {
        assert_eq!(
            parse_content_range_start("bytes 1024-2047/4096"),
            Some(1024)
        );
    }

    #[test]
    fn test_parse_content_range_start_unknown_total() {
        assert_eq!(parse_content_range_start("bytes 512-1023/*"), Some(512));
    }

    #[test]
    fn test_parse_content_range_start_zero() {
        assert_eq!(
            parse_content_range_start("bytes 0-999/1000"),
            Some(0)
        );
    }

    #[test]
    fn test_parse_content_range_start_invalid() {
        assert_eq!(parse_content_range_start("invalid"), None);
        assert_eq!(parse_content_range_start("bytes abc-def/ghi"), None);
        assert_eq!(parse_content_range_start(""), None);
    }
}
