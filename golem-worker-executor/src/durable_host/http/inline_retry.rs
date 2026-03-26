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
//! # Phases
//!
//! - **Awaiting Response**: Retry at `FutureIncomingResponse::get()` — the response
//!   hasn't arrived yet, or arrived with an error. The outgoing body is fully
//!   finished.
//! - **Resuming Response Body**: Retry during response body reading — the response
//!   was partially consumed. Requires re-sending the request and verifying the
//!   response prefix matches.

use crate::durable_host::HttpRequestState;
use crate::durable_host::durability::{
    AsyncRetryDecision, DurabilityHost, DurableExecutionState, HostFailureKind,
    InFunctionRetryHost, InFunctionRetryState,
};
use crate::durable_host::http::types::classify_http_error_code;
use crate::services::oplog::{Oplog, OplogOps};
use crate::services::{HasOplog, HasWorker};
use bytes::Bytes;
use golem_common::model::RetryConfig;
use golem_common::model::oplog::payload::HostPayloadPair;
use golem_common::model::oplog::types::SerializableHttpMethod;
use golem_common::model::oplog::{
    DurableFunctionType, HostRequestHttpRequest, HostResponse, OplogEntry, OplogIndex,
    PersistenceLevel,
};
use golem_common::model::{NamedRetryPolicy, PredicateValue, RetryProperties};
use http::{HeaderName, HeaderValue};
use http_body_util::BodyExt;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;
use tracing::Instrument;
use wasmtime_wasi::OutputStream;
use wasmtime_wasi_http::HttpConnectionPool;
use wasmtime_wasi_http::bindings::http::types as wasi_http_types;
use wasmtime_wasi_http::body::{
    HostIncomingBody, HostOutgoingBody, HyperOutgoingBody, StreamContext,
};
use wasmtime_wasi_http::types::{
    FutureIncomingResponseHandle, HostFutureIncomingResponse, IncomingResponse,
    OutgoingRequestConfig, default_send_request_with_pool,
};

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
    /// The outgoing body is not yet finished (awaiting-response phase only).
    BodyNotFinished,
    /// The request method is not idempotent and assume_idempotence is false.
    NotIdempotent,
    /// The response body used skip/blocking_skip (response-body resumption only).
    HadBodySkip,
    /// The output stream had subscribe() called, so pollable may be stale after replacement.
    OutputStreamSubscribed,
}

/// Which inline retry phase is being attempted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InlineRetryPhase {
    /// Retry at FutureIncomingResponse::get() — response not yet consumed.
    AwaitingResponse,
    /// Retry during response body reading — partial body already consumed.
    ResumingResponseBody,
    /// Retry during outgoing body stream writing — body is still being written.
    WritingRequestBody,
}

/// Checks whether the given HTTP request is eligible for transparent inline retry.
///
/// Returns `Ok(())` if eligible, or `Err(reason)` explaining why not.
pub(crate) fn is_http_inline_retry_eligible(
    exec_state: &DurableExecutionState,
    request_state: &HttpRequestState,
    zone: InlineRetryPhase,
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

    if request_state.retry.has_unreconstructable_body {
        return Err(InlineRetryIneligible::UnreconstructableBody);
    }

    // The output_stream_subscribed guard prevents stale pollables after resource
    // swap. The flag is only set when the guest explicitly calls subscribe() on
    // the output stream — wasmtime's internal blocking_write_and_flush goes
    // directly to the underlying DynOutputStream and does NOT call our
    // HostOutputStream::subscribe, so the flag won't be spuriously set by
    // blocking write operations. If the guest did call subscribe(), the Pollable
    // it holds would go stale after we swap the OutputStream resource, so we
    // must reject retry in all phases including WritingRequestBody.
    if request_state.retry.output_stream_subscribed {
        return Err(InlineRetryIneligible::OutputStreamSubscribed);
    }

    if request_state.retry.has_outgoing_trailers {
        return Err(InlineRetryIneligible::HasOutgoingTrailers);
    }

    if zone == InlineRetryPhase::AwaitingResponse
        && request_state.output_stream_rep.is_some()
        && !request_state.retry.body_finished
    {
        return Err(InlineRetryIneligible::BodyNotFinished);
    }

    // WritingRequestBody does not require body_finished — the body is still
    // being written, which is the whole point of this retry path.

    if zone == InlineRetryPhase::ResumingResponseBody && request_state.retry.had_body_skip {
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

/// A chunk of outgoing body data reconstructed from the oplog.
///
/// This avoids materializing the entire body in memory — callers can
/// stream chunks into a hyper body pipe incrementally.
#[derive(Debug, Clone)]
pub enum BodyChunk {
    /// Actual data bytes written via `write`.
    Data(Bytes),
    /// A run of zeroes written via `write_zeroes`.
    Zeroes(u64),
}

/// Reconstructs the outgoing request body by scanning oplog entries in
/// `[begin_index..current_oplog_index]` for body write entries belonging
/// to this request's batch.
///
/// Returns a `Vec<BodyChunk>` instead of a contiguous `Bytes` buffer so
/// that callers can stream the chunks into a hyper body pipe without
/// allocating the full body in memory at once.
pub async fn reconstruct_outgoing_body_chunks(
    oplog: &Arc<dyn Oplog>,
    begin_index: OplogIndex,
) -> Result<Vec<BodyChunk>, anyhow::Error> {
    reconstruct_outgoing_body_chunks_after(oplog, begin_index, None).await
}

async fn reconstruct_outgoing_body_chunks_after(
    oplog: &Arc<dyn Oplog>,
    begin_index: OplogIndex,
    after_index: Option<OplogIndex>,
) -> Result<Vec<BodyChunk>, anyhow::Error> {
    let current_idx = oplog.current_oplog_index().await;

    if current_idx < begin_index {
        return Ok(Vec::new());
    }

    // current_oplog_index() returns the index OF the last written entry,
    // so we need +1 to include it in the scan range.
    let n: u64 = Into::<u64>::into(current_idx) - Into::<u64>::into(begin_index) + 1;
    let entries = oplog.read_many(begin_index, n).await;
    let mut chunks = Vec::new();

    let write_fn_name =
        golem_common::model::oplog::host_functions::HttpTypesOutgoingBodyStreamWrite::HOST_FUNCTION_NAME;
    let write_zeroes_fn_name =
        golem_common::model::oplog::host_functions::HttpTypesOutgoingBodyStreamWriteZeroes::HOST_FUNCTION_NAME;

    for (idx, entry) in &entries {
        if after_index.is_some_and(|after| *idx <= after) {
            continue;
        }

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

            if *function_name == write_fn_name {
                let response_value =
                    oplog
                        .download_payload(response.clone())
                        .await
                        .map_err(|err| {
                            anyhow::anyhow!("failed to download outgoing body chunk payload: {err}")
                        })?;

                if let HostResponse::StreamWriteWithBytes(payload) = response_value
                    && let Ok(data) = &payload.result
                    && !data.is_empty()
                {
                    chunks.push(BodyChunk::Data(Bytes::from(data.clone())));
                }
            } else if *function_name == write_zeroes_fn_name {
                let response_value =
                    oplog
                        .download_payload(response.clone())
                        .await
                        .map_err(|err| {
                            anyhow::anyhow!("failed to download outgoing body chunk payload: {err}")
                        })?;

                if let HostResponse::StreamWriteZeroes(payload) = response_value
                    && let Ok(len) = &payload.result
                    && *len > 0
                {
                    chunks.push(BodyChunk::Zeroes(*len));
                }
            }
        }
    }

    Ok(chunks)
}

async fn find_last_retry_error_index(
    oplog: &Arc<dyn Oplog>,
    begin_index: OplogIndex,
) -> Result<Option<OplogIndex>, anyhow::Error> {
    let current_idx = oplog.current_oplog_index().await;

    if current_idx < begin_index {
        return Ok(None);
    }

    let n: u64 = Into::<u64>::into(current_idx) - Into::<u64>::into(begin_index) + 1;
    let entries = oplog.read_many(begin_index, n).await;

    let mut last_retry_error_idx = None;
    for (idx, entry) in entries {
        if let OplogEntry::Error { retry_from, .. } = entry
            && retry_from == begin_index
        {
            last_retry_error_idx = Some(idx);
        }
    }

    Ok(last_retry_error_idx)
}

/// Counts the total number of incoming body bytes successfully delivered to
/// the guest, as recorded in the oplog. Used during response-body resumption to
/// determine how many
/// bytes to skip or verify without materializing the full body.
pub async fn count_incoming_body_bytes(
    oplog: &Arc<dyn Oplog>,
    begin_index: OplogIndex,
) -> Result<u64, anyhow::Error> {
    let current_idx = oplog.current_oplog_index().await;

    if current_idx < begin_index {
        return Ok(0);
    }

    // current_oplog_index() returns the index OF the last written entry,
    // so we need +1 to include it in the scan range.
    let n: u64 = Into::<u64>::into(current_idx) - Into::<u64>::into(begin_index) + 1;
    let entries = oplog.read_many(begin_index, n).await;
    let mut total: u64 = 0;

    let read_fn_name =
        golem_common::model::oplog::host_functions::HttpTypesIncomingBodyStreamRead::HOST_FUNCTION_NAME;
    let blocking_read_fn_name =
        golem_common::model::oplog::host_functions::HttpTypesIncomingBodyStreamBlockingRead::HOST_FUNCTION_NAME;

    for entry in entries.values() {
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
                    oplog
                        .download_payload(response.clone())
                        .await
                        .map_err(|err| {
                            anyhow::anyhow!("failed to download incoming body chunk payload: {err}")
                        })?;

                if let HostResponse::StreamChunk(payload) = response_value
                    && let Ok(data) = &payload.result
                {
                    total += data.len() as u64;
                }
            }
        }
    }

    Ok(total)
}

/// Builds a `hyper::Request` from the stored HTTP request metadata and
/// reconstructed body bytes.
///
/// The request exactly reproduces the original: same URI, method, headers,
/// and body content. Headers are `Vec<(String, Vec<u8>)>` preserving
/// duplicates and byte-level fidelity.
///
/// For response-body resumption, `extra_headers` can include a `Range` header.
pub fn reconstruct_http_request(
    request: &HostRequestHttpRequest,
    body: HyperOutgoingBody,
    extra_headers: &[(String, String)],
) -> Result<hyper::Request<HyperOutgoingBody>, anyhow::Error> {
    let method = http::Method::try_from(&request.method)?;
    let uri: hyper::Uri = request
        .uri
        .parse()
        .map_err(|e| anyhow::anyhow!("failed to parse stored URI '{}': {e}", request.uri))?;

    let mut builder = hyper::Request::builder().method(method).uri(uri);

    // Replay stored headers exactly
    for (name, value) in &request.headers {
        let header_name = HeaderName::from_str(name)
            .map_err(|e| anyhow::anyhow!("invalid stored header name '{name}': {e}"))?;
        let header_value = HeaderValue::from_str(value)
            .map_err(|e| anyhow::anyhow!("invalid stored header value for '{name}': {e}"))?;
        builder = builder.header(header_name, header_value);
    }

    // Add any extra headers (e.g., Range for response-body resumption)
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

/// Converts a `Vec<BodyChunk>` into a `HyperOutgoingBody` for use with hyper.
///
/// Each chunk is emitted as a separate `Frame::data` item in a stream,
/// so the full body is never materialized as a single contiguous buffer.
/// Zeroes chunks are emitted in 64 KiB slices to cap per-frame allocation.
pub fn body_chunks_to_hyper_body(chunks: Vec<BodyChunk>) -> HyperOutgoingBody {
    use futures::stream;
    use http_body_util::StreamBody;

    const ZERO_BUF_SIZE: usize = 64 * 1024;

    let frames = chunks.into_iter().flat_map(move |chunk| match chunk {
        BodyChunk::Data(data) => {
            vec![hyper::body::Frame::data(data)]
        }
        BodyChunk::Zeroes(total) => {
            let mut frames = Vec::new();
            let mut remaining = total as usize;
            while remaining > 0 {
                let len = remaining.min(ZERO_BUF_SIZE);
                frames.push(hyper::body::Frame::data(Bytes::from(vec![0u8; len])));
                remaining -= len;
            }
            frames
        }
    });

    let body_stream = stream::iter(frames.map(Ok::<_, wasi_http_types::ErrorCode>));
    let stream_body: StreamBody<_> = StreamBody::new(body_stream);
    stream_body.boxed_unsync()
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

/// Sends an HTTP request with interrupt-aware retry on transient errors.
///
/// On transient `ErrorCode`, computes backoff delay via `get_delay`, sleeps with
/// interrupt awareness, and retries. Returns `Ok(Some(response))` on success,
/// `Ok(None)` when retries are exhausted or the delay exceeds `max_delay` (caller
/// should fall back), or `Err` if an interrupt occurs during sleep.
///
/// This is the in-context counterpart of `in_task_retry_loop` (which runs in
/// spawned background tasks without interrupt awareness).
async fn send_with_interrupt_aware_retries<Ctx: crate::workerctx::WorkerCtx>(
    ctx: &mut crate::durable_host::DurableWorkerCtx<Ctx>,
    request_state: &HttpRequestState,
    body_chunks: &[BodyChunk],
    extra_headers: &[(String, String)],
    retry_function_name: Option<&'static str>,
) -> Result<Option<IncomingResponse>, anyhow::Error> {
    let mut retry_state = retry_function_name.map(|_| InFunctionRetryState::new());
    let reconstructed_body_len: u64 = body_chunks
        .iter()
        .map(|chunk| match chunk {
            BodyChunk::Data(data) => data.len() as u64,
            BodyChunk::Zeroes(len) => *len,
        })
        .sum();
    let request_has_content_length = request_state
        .request
        .headers
        .iter()
        .any(|(name, _)| name.eq_ignore_ascii_case("content-length"));

    loop {
        let hyper_body = body_chunks_to_hyper_body(body_chunks.to_vec());
        let mut merged_extra_headers = extra_headers.to_vec();
        let extra_has_content_length = merged_extra_headers
            .iter()
            .any(|(name, _)| name.eq_ignore_ascii_case("content-length"));
        if !request_has_content_length && !extra_has_content_length {
            merged_extra_headers.push((
                "content-length".to_string(),
                reconstructed_body_len.to_string(),
            ));
        }

        let http_request =
            reconstruct_http_request(&request_state.request, hyper_body, &merged_extra_headers)?;
        let config = request_state.outgoing_request_config();

        // Force a fresh transport for each inline retry attempt. Reusing pooled
        // connections after mid-body failures can keep retrying a poisoned
        // socket and repeatedly hit read timeouts.
        let mut future_resp = default_send_request_with_pool(http_request, config, None, None);

        use wasmtime_wasi::Pollable;
        future_resp.ready().await;

        match future_resp.unwrap_ready() {
            Ok(Ok(resp)) => return Ok(Some(resp)),
            Err(_trap) => return Ok(None),
            Ok(Err(ref error_code))
                if classify_http_error_code(error_code) == HostFailureKind::Permanent =>
            {
                return Ok(None);
            }
            Ok(Err(error_code)) => {
                if let (Some(retry_state), Some(function_name)) =
                    (retry_state.as_mut(), retry_function_name)
                {
                    let retry_properties = golem_common::model::RetryContext::http_with_response(
                        &request_state.request.method.to_string(),
                        &request_state.request.uri,
                        None,
                        "transient",
                    );
                    match retry_state.decide_retry_with_properties(ctx, function_name, &retry_properties).await {
                        AsyncRetryDecision::RetryAfterDelay(delay) => {
                            // Interrupt-aware sleep
                            let interrupt = ctx.create_interrupt_signal();
                            let sleep = tokio::time::sleep(delay);
                            tokio::pin!(sleep);

                            match futures::future::select(sleep, interrupt).await {
                                futures::future::Either::Left(_) => {
                                    tracing::debug!(
                                        retry_count = retry_state.retry_count(),
                                        ?delay,
                                        ?error_code,
                                        "Resuming response body inline retry: transient send error, retrying"
                                    );
                                }
                                futures::future::Either::Right((interrupt_kind, _)) => {
                                    return Err(anyhow::Error::from(interrupt_kind));
                                }
                            }
                        }
                        AsyncRetryDecision::Exhausted | AsyncRetryDecision::FallBackToTrap => {
                            return Ok(None);
                        }
                    }
                } else {
                    return Ok(None);
                }
            }
        }
    }
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

/// Writes a sequence of `BodyChunk`s into an `OutputStream`, respecting
/// backpressure via `check_write` / `ready`.
///
/// Data chunks are written in budget-sized slices. Zeroes chunks are written
/// using small temporary buffers (up to 64 KiB at a time) to avoid
/// materializing huge allocations.
async fn replay_body_chunks(
    stream: &mut Box<dyn OutputStream>,
    chunks: &[BodyChunk],
) -> Result<(), anyhow::Error> {
    for chunk in chunks {
        match chunk {
            BodyChunk::Data(data) => {
                let mut offset = 0;
                while offset < data.len() {
                    let budget = stream.check_write().map_err(|e| {
                        anyhow::anyhow!("check_write failed during body replay: {e}")
                    })?;

                    if budget == 0 {
                        stream.ready().await;
                        continue;
                    }

                    let end = std::cmp::min(offset + budget, data.len());
                    let slice = Bytes::copy_from_slice(&data[offset..end]);
                    stream
                        .write(slice)
                        .map_err(|e| anyhow::anyhow!("write failed during body replay: {e}"))?;
                    offset = end;
                }
            }
            BodyChunk::Zeroes(total) => {
                const ZERO_BUF_SIZE: usize = 64 * 1024;
                let mut remaining = *total as usize;
                while remaining > 0 {
                    let budget = stream.check_write().map_err(|e| {
                        anyhow::anyhow!("check_write failed during zeroes replay: {e}")
                    })?;

                    if budget == 0 {
                        stream.ready().await;
                        continue;
                    }

                    let write_len = remaining.min(budget).min(ZERO_BUF_SIZE);
                    let zeros = Bytes::from(vec![0u8; write_len]);
                    stream
                        .write(zeros)
                        .map_err(|e| anyhow::anyhow!("write failed during zeroes replay: {e}"))?;
                    remaining -= write_len;
                }
            }
        }
    }
    Ok(())
}

/// Rebuilds an HTTP request as a streaming request for output stream retry.
///
/// This reconstructs all prior body chunks from the oplog, creates a fresh
/// outgoing body+stream pair, streams the prior chunks into the new stream,
/// and sends the request. The caller receives a `RebuiltStreamingRequest`
/// whose fields can replace the guest's existing resource table entries.
///
/// Unlike `send_reconstructed_request()` (which sends the body as a complete
/// `Full<Bytes>`), this creates a streaming body so the guest can continue
/// writing additional data after the retry. Chunks are replayed lazily to
/// avoid materializing the full body in memory.
pub(crate) async fn rebuild_streaming_request(
    oplog: &Arc<dyn Oplog>,
    request_state: &HttpRequestState,
    config: OutgoingRequestConfig,
    connection_pool: Option<HttpConnectionPool>,
) -> Result<RebuiltStreamingRequest, anyhow::Error> {
    // 1. Reconstruct body chunks from oplog (lazy representation)
    let body_chunks = reconstruct_outgoing_body_chunks(oplog, request_state.begin_index).await?;

    // 2. Create a fresh outgoing body with a streaming body pair
    let (mut new_outgoing_body, hyper_body) =
        HostOutgoingBody::new(StreamContext::Request, None, 1, 1024 * 1024);

    // 3. Take the output stream from the new body
    let mut new_stream = new_outgoing_body
        .take_output_stream()
        .ok_or_else(|| anyhow::anyhow!("failed to take output stream from new outgoing body"))?;

    // 4. Build the HTTP request with the streaming body
    let reconstructed = reconstruct_http_request(&request_state.request, hyper_body, &[])?;

    // 5. Send the request BEFORE writing prior bytes. Hyper starts consuming
    //    the pipe in the background once dispatched, preventing deadlock when
    //    prior bytes exceed hyper's internal pipe capacity (~1MB).
    let new_future = default_send_request_with_pool(reconstructed, config, None, connection_pool);

    // 6. Stream all prior body chunks into the new stream using raw OutputStream.
    //    Now that hyper is actively consuming, the pipe won't fill up permanently.
    replay_body_chunks(&mut new_stream, &body_chunks).await?;

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
pub fn spawn_http_request_with_retry<Ctx: crate::workerctx::WorkerCtx>(
    original_handle: FutureIncomingResponseHandle,
    request: HostRequestHttpRequest,
    config: OutgoingRequestConfig,
    _connection_pool: Option<HttpConnectionPool>,
    worker: Arc<crate::worker::Worker<Ctx>>,
    named_retry_policies: Vec<NamedRetryPolicy>,
    retry_properties: RetryProperties,
    max_delay: Duration,
    begin_index: OplogIndex,
    execution_status: Arc<std::sync::RwLock<crate::model::ExecutionStatus>>,
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
                Ok(Err(initial_error)) => {
                    let oplog = worker.oplog();
                    let current_retry_policy_state = worker
                        .get_non_detached_last_known_status()
                        .await
                        .current_retry_state
                        .get(&begin_index)
                        .cloned();
                    let mut task_ctx = crate::durable_host::durability::TaskRetryContext {
                        retry_point: begin_index,
                        named_retry_policies,
                        max_in_function_retry_delay: max_delay,
                        current_retry_policy_state,
                        retry_properties,
                        worker,
                    };

                    // Account for the initial transient failure of the original request
                    // so retry metrics/oplog entries include it as part of in-function
                    // retry budgeting (matching host-call retry semantics).
                    let mut initial_retry_state = InFunctionRetryState::new();
                    let mut initial_retry_properties = task_ctx.retry_properties.clone();
                    initial_retry_properties
                        .set("error-type", PredicateValue::Text("transient".to_string()));
                    match initial_retry_state
                        .decide_retry_with_properties(
                            &mut task_ctx,
                            "in-task",
                            &initial_retry_properties,
                        )
                        .await
                    {
                        AsyncRetryDecision::RetryAfterDelay(delay) => {
                            tokio::time::sleep(delay).await;
                        }
                        AsyncRetryDecision::Exhausted | AsyncRetryDecision::FallBackToTrap => {
                            return Ok(Err(initial_error));
                        }
                    }

                    let result = crate::durable_host::durability::in_task_retry_loop(
                        task_ctx,
                        classify_http_error_code,
                        || {
                            let oplog = oplog.clone();
                            let request = request.clone();
                            async move {
                                // Reconstruct body chunks from oplog
                                let body_chunks =
                                    reconstruct_outgoing_body_chunks(&oplog, begin_index)
                                        .await
                                        .map_err(|e| {
                                            wasi_http_types::ErrorCode::InternalError(Some(
                                                format!("body reconstruction failed: {e}"),
                                            ))
                                        })?;

                                // Build the request with a streaming body
                                let hyper_body = body_chunks_to_hyper_body(body_chunks);
                                let http_request =
                                    reconstruct_http_request(&request, hyper_body, &[]).map_err(
                                        |e| {
                                            wasi_http_types::ErrorCode::InternalError(Some(
                                                format!("request reconstruction failed: {e}"),
                                            ))
                                        },
                                    )?;

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
                                    // Force a fresh connection for each in-task retry attempt.
                                    // Reusing pooled connections after mid-body failures can
                                    // keep retrying a poisoned transport and lead to repeated
                                    // read timeouts.
                                    None,
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
                        || {
                            execution_status
                                .read()
                                .unwrap()
                                .create_await_interrupt_signal()
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

    // 2. Check eligibility — use WritingRequestBody (body is still being written)
    let exec_state = ctx.durable_execution_state();
    if is_http_inline_retry_eligible(
        &exec_state,
        &request_state,
        InlineRetryPhase::WritingRequestBody,
    )
    .is_err()
    {
        return Ok(false);
    }

    // 3. Check retry budget — decide_retry handles delay calculation, oplog error
    //    entry writing, and metric recording. The oplog-based retry count provides
    //    persistence across individual write calls within the same request.
    let mut retry_state = InFunctionRetryState::new();
    let retry_properties = golem_common::model::RetryContext::http_with_response(
        &request_state.request.method.to_string(),
        &request_state.request.uri,
        None,
        "transient",
    );
    let decision = retry_state.decide_retry_with_properties(ctx, "output-stream-write", &retry_properties).await;

    match decision {
        AsyncRetryDecision::RetryAfterDelay(delay) => {
            // Interrupt-aware sleep before rebuilding
            let interrupt = ctx.create_interrupt_signal();
            let sleep = tokio::time::sleep(delay);
            tokio::pin!(sleep);

            match futures::future::select(sleep, interrupt).await {
                futures::future::Either::Left(_) => {
                    // Sleep completed, proceed with rebuild below
                }
                futures::future::Either::Right((interrupt_kind, _)) => {
                    return Err(anyhow::Error::from(interrupt_kind));
                }
            }
        }
        AsyncRetryDecision::Exhausted | AsyncRetryDecision::FallBackToTrap => {
            return Ok(false);
        }
    }

    // 4. Get oplog
    let oplog = ctx.public_state.oplog();
    let config = request_state.outgoing_request_config();

    // 5. Rebuild the streaming request
    let rebuilt = rebuild_streaming_request(&oplog, &request_state, config, None).await?;

    // 6. Swap resources in the resource table
    // Swap the FutureIncomingResponse — re-wrap with background retry if the
    // original request had it, so that transient errors at get() are still handled.
    let new_future = if request_state.retry.has_background_retry {
        if let HostFutureIncomingResponse::Pending(handle) = rebuilt.future {
            let named_retry_policies = ctx.state.named_retry_policies().to_vec();
            let retry_properties = golem_common::model::RetryContext::http(
                &request_state.request.method.to_string(),
                &request_state.request.uri,
            );
            let retry_handle = spawn_http_request_with_retry(
                handle,
                request_state.request.clone(),
                request_state.outgoing_request_config(),
                ctx.wasi_http.connection_pool.clone(),
                ctx.public_state.worker(),
                named_retry_policies,
                retry_properties,
                exec_state.max_in_function_retry_delay,
                request_state.begin_index,
                ctx.execution_status.clone(),
            );
            HostFutureIncomingResponse::pending(retry_handle)
        } else {
            rebuilt.future
        }
    } else {
        rebuilt.future
    };
    let future_res: &mut HostFutureIncomingResponse = ctx.table().get_mut(&Resource::<
        FutureIncomingResponse,
    >::new_borrow(
        request_handle
    ))?;
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

    let stream_entry: &mut wasmtime_wasi::DynOutputStream =
        ctx.table()
            .get_mut(&Resource::<WasiOutputStream>::new_borrow(stream_rep))?;
    *stream_entry = rebuilt.output_stream;

    Ok(true)
}

/// Attempts response-body resumption inline retry for a response body stream read failure.
///
/// When reading response body bytes fails with a transient error, this function:
/// 1. Checks response-body resumption eligibility (no prior skip, etc.)
/// 2. Calculates bytes already delivered to the guest from oplog
/// 3. Reconstructs the outgoing request with a Range header
/// 4. Sends the request and handles 206, 416, or a full-body response that
///    preserves the original status code seen by the guest
/// 5. Swaps the InputStream to the new response's body stream
///
/// Returns `Ok(true)` if retry succeeded (stream swapped, caller should re-attempt read),
/// `Ok(false)` if retry is not eligible or conditions not met,
/// `Err` with a StreamError if content mismatch detected.
pub async fn try_resuming_response_body_inline_retry<Ctx: crate::workerctx::WorkerCtx>(
    ctx: &mut crate::durable_host::DurableWorkerCtx<Ctx>,
    stream_handle: u32,
) -> Result<bool, anyhow::Error> {
    use wasmtime::component::Resource;
    use wasmtime_wasi::p2::bindings::io::streams::InputStream as WasiInputStream;
    use wasmtime_wasi_http::bindings::http::types::IncomingBody as WasiIncomingBody;

    // 1. Find the request state via the stream handle.
    //    The stream rep IS the request tracking handle for incoming body streams.
    let request_state = match ctx.state.open_http_requests.get(&stream_handle) {
        Some(s) => s.clone(),
        None => return Ok(false),
    };

    // Response-body resumption requires a tracked IncomingBody handle to properly
    // swap body+stream.
    let body_handle = match request_state.body_handle {
        Some(h) => h,
        None => return Ok(false),
    };

    // 2. Check response-body resumption eligibility
    let exec_state = ctx.durable_execution_state();
    if is_http_inline_retry_eligible(
        &exec_state,
        &request_state,
        InlineRetryPhase::ResumingResponseBody,
    )
    .is_err()
    {
        return Ok(false);
    }

    // 3. Count bytes already delivered to the guest from the oplog
    let oplog = ctx.public_state.oplog();
    let consumed_len = count_incoming_body_bytes(&oplog, request_state.begin_index).await?;

    // 4. Reconstruct the outgoing request body chunks from the oplog
    let body_chunks = reconstruct_outgoing_body_chunks(&oplog, request_state.begin_index).await?;

    // 5. Build the request, adding a Range header if bytes were already consumed.
    //    If the original request already has a Range header, response-body
    //    resumption is not supported because composing Range headers correctly is
    //    complex.
    let has_range = request_state
        .request
        .headers
        .iter()
        .any(|(k, _)| k.eq_ignore_ascii_case("range"));
    if has_range {
        return Ok(false);
    }

    // Record and budget this response-body resumption as an in-function retry
    // attempt
    // so oplog/error accounting reflects that we recovered from a transient read
    // failure by reconstructing and resuming the request.
    let mut retry_state = InFunctionRetryState::new();
    let retry_properties = golem_common::model::RetryContext::http_with_response(
        &request_state.request.method.to_string(),
        &request_state.request.uri,
        None,
        "transient",
    );
    match retry_state
        .decide_retry_with_properties(ctx, "http-zone2-read", &retry_properties)
        .await
    {
        AsyncRetryDecision::RetryAfterDelay(delay) => {
            let interrupt = ctx.create_interrupt_signal();
            let sleep = tokio::time::sleep(delay);
            tokio::pin!(sleep);

            match futures::future::select(sleep, interrupt).await {
                futures::future::Either::Left(_) => {}
                futures::future::Either::Right((interrupt_kind, _)) => {
                    return Err(anyhow::Error::from(interrupt_kind));
                }
            }
        }
        AsyncRetryDecision::Exhausted | AsyncRetryDecision::FallBackToTrap => {
            return Ok(false);
        }
    }

    let extra_headers = if consumed_len > 0 {
        vec![("range".to_string(), format!("bytes={consumed_len}-"))]
    } else {
        vec![]
    };
    // 6. Send the reconstructed request (with interrupt-aware retries)
    let response = match send_with_interrupt_aware_retries(
        ctx,
        &request_state,
        &body_chunks,
        &extra_headers,
        Some("http-resume-response-body-send"),
    )
    .await?
    {
        Some(resp) => resp,
        None => return Ok(false),
    };

    let status = response.resp.status().as_u16();
    let original_status = request_state.response_status;
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
                Some(start) if start == consumed_len => {
                    // Range matches — swap body+stream
                    let (_parts, body) = response.resp.into_parts();
                    let new_body = HostIncomingBody::new(body, between_bytes_timeout, usize::MAX);

                    // Swap IncomingBody at body_handle, then take stream from it
                    let body_entry: &mut HostIncomingBody =
                        ctx.table()
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
                        "Resuming response body inline retry: 206 Partial Content, body+stream swapped"
                    );
                    Ok(true)
                }
                _ => {
                    // Content-Range doesn't match or missing — fall back
                    tracing::debug!(
                        stream_handle = stream_handle,
                        content_range = ?content_range,
                        consumed_len = consumed_len,
                        "Resuming response body inline retry: 206 Content-Range mismatch, falling back"
                    );
                    Ok(false)
                }
            }
        }
        _ if original_status == Some(status) && consumed_len == 0 => {
            // Full response with nothing consumed yet — swap body+stream directly
            let (_parts, body) = response.resp.into_parts();
            let new_body = HostIncomingBody::new(body, between_bytes_timeout, usize::MAX);

            let body_entry: &mut HostIncomingBody =
                ctx.table()
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
                status = status,
                "Resuming response body inline retry: matching full response (no bytes consumed), body+stream swapped"
            );
            Ok(true)
        }
        _ if original_status == Some(status) => {
            // Full response — skip consumed_len bytes then swap.
            // We only count bytes (no content verification) because materializing
            // the previously consumed data would require the same OOM-prone
            // allocation we are trying to avoid.
            let (_parts, body) = response.resp.into_parts();
            let new_body = HostIncomingBody::new(body, between_bytes_timeout, usize::MAX);

            // Swap IncomingBody at body_handle first
            let body_entry: &mut HostIncomingBody =
                ctx.table()
                    .get_mut(&Resource::<WasiIncomingBody>::new_borrow(body_handle))?;
            *body_entry = new_body;
            let mut new_stream = body_entry.take_stream().ok_or_else(|| {
                anyhow::anyhow!("HTTP retry failed: could not take stream from new body")
            })?;

            // Skip consumed_len bytes from the new stream (read and discard)
            let mut skipped = 0u64;
            while skipped < consumed_len {
                // Wait for data to be available
                new_stream.ready().await;

                let remaining = (consumed_len - skipped) as usize;
                let chunk = new_stream.read(remaining).map_err(|e| match e {
                    wasmtime_wasi::StreamError::Closed => anyhow::anyhow!(
                        "HTTP retry failed: response shorter than previously consumed bytes"
                    ),
                    wasmtime_wasi::StreamError::LastOperationFailed(e) => {
                        anyhow::anyhow!("HTTP retry failed: error reading prefix for skip: {e}")
                    }
                    wasmtime_wasi::StreamError::Trap(e) => {
                        anyhow::anyhow!("HTTP retry failed: trap reading prefix for skip: {e}")
                    }
                })?;

                if chunk.is_empty() {
                    // No data yet, will retry after ready()
                    continue;
                }

                skipped += chunk.len() as u64;
            }

            // Prefix skipped — swap the stream (which now has the remaining body)
            let stream_entry: &mut wasmtime_wasi::DynInputStream = ctx
                .table()
                .get_mut(&Resource::<WasiInputStream>::new_borrow(stream_handle))?;
            *stream_entry = new_stream;

            tracing::debug!(
                stream_handle = stream_handle,
                status = status,
                consumed_len = consumed_len,
                "Resuming response body inline retry: matching full response with prefix skip, body+stream swapped"
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
            // Unexpected or status-changing response — don't retry
            tracing::debug!(
                stream_handle = stream_handle,
                status = status,
                original_status = original_status,
                "Resuming response body inline retry: retried status mismatch or unsupported status, falling back"
            );
            Ok(false)
        }
    }
}

/// Attempts awaiting-response inline retry from `FutureIncomingResponse::get()`
/// after a transient response error.
///
pub(crate) async fn try_awaiting_response_inline_retry<Ctx: crate::workerctx::WorkerCtx>(
    ctx: &mut crate::durable_host::DurableWorkerCtx<Ctx>,
    request_state: &HttpRequestState,
) -> Result<Option<IncomingResponse>, anyhow::Error> {
    let exec_state = ctx.durable_execution_state();
    let mut eligibility_state = request_state.clone();
    // AwaitingResponse get()-time resend does not swap output stream resources, so an
    // output-stream subscribe() pollable cannot go stale here.
    eligibility_state.retry.output_stream_subscribed = false;

    if is_http_inline_retry_eligible(
        &exec_state,
        &eligibility_state,
        InlineRetryPhase::AwaitingResponse,
    )
    .is_err()
    {
        return Ok(None);
    }

    let oplog = ctx.public_state.oplog();
    let last_retry_error_idx =
        find_last_retry_error_index(&oplog, request_state.begin_index).await?;

    let mut body_chunks = reconstruct_outgoing_body_chunks_after(
        &oplog,
        request_state.begin_index,
        last_retry_error_idx,
    )
    .await?;
    if body_chunks.is_empty() {
        body_chunks = reconstruct_outgoing_body_chunks(&oplog, request_state.begin_index).await?;
    }
    send_with_interrupt_aware_retries(ctx, request_state, &body_chunks, &[], None).await
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
        assert!(!is_method_idempotent(&SerializableHttpMethod::Other(
            "CUSTOM".to_string()
        )));
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
        assert_eq!(parse_content_range_start("bytes 0-999/1000"), Some(0));
    }

    #[test]
    fn test_parse_content_range_start_invalid() {
        assert_eq!(parse_content_range_start("invalid"), None);
        assert_eq!(parse_content_range_start("bytes abc-def/ghi"), None);
        assert_eq!(parse_content_range_start(""), None);
    }

    fn make_exec_state() -> DurableExecutionState {
        DurableExecutionState {
            is_live: true,
            persistence_level: PersistenceLevel::PersistRemoteSideEffects,
            snapshotting_mode: None,
            assume_idempotence: true,
            max_in_function_retry_delay: Duration::from_secs(1),
        }
    }

    fn make_request_state() -> HttpRequestState {
        use crate::durable_host::HttpRequestCloseOwner;
        use crate::durable_host::HttpRetryEligibility;
        use golem_common::model::invocation_context::SpanId;

        HttpRequestState {
            close_owner: HttpRequestCloseOwner::FutureIncomingResponseDrop,
            begin_index: OplogIndex::INITIAL,
            request: HostRequestHttpRequest {
                uri: "http://localhost:8080/".to_string(),
                method: SerializableHttpMethod::Get,
                headers: std::collections::HashMap::new(),
            },
            span_id: SpanId::generate(),
            body_handle: None,
            response_status: Some(200),
            outgoing_body_rep: None,
            output_stream_rep: None,
            use_tls: false,
            connect_timeout: Duration::from_secs(5),
            first_byte_timeout: Duration::from_secs(5),
            between_bytes_timeout: Duration::from_secs(5),
            retry: HttpRetryEligibility {
                body_finished: true,
                ..Default::default()
            },
        }
    }

    #[test]
    fn test_eligible_baseline() {
        let exec = make_exec_state();
        let req = make_request_state();
        assert!(
            is_http_inline_retry_eligible(&exec, &req, InlineRetryPhase::AwaitingResponse).is_ok()
        );
        assert!(
            is_http_inline_retry_eligible(&exec, &req, InlineRetryPhase::ResumingResponseBody)
                .is_ok()
        );
    }

    #[test]
    fn test_unreconstructable_body_disqualifies() {
        let exec = make_exec_state();
        let mut req = make_request_state();
        req.retry.has_unreconstructable_body = true;
        assert_eq!(
            is_http_inline_retry_eligible(&exec, &req, InlineRetryPhase::AwaitingResponse),
            Err(InlineRetryIneligible::UnreconstructableBody)
        );
    }

    #[test]
    fn test_output_stream_subscribed_disqualifies_zone1_and_zone2() {
        let exec = make_exec_state();
        let mut req = make_request_state();
        req.retry.output_stream_subscribed = true;
        assert_eq!(
            is_http_inline_retry_eligible(&exec, &req, InlineRetryPhase::AwaitingResponse),
            Err(InlineRetryIneligible::OutputStreamSubscribed)
        );
        assert_eq!(
            is_http_inline_retry_eligible(&exec, &req, InlineRetryPhase::ResumingResponseBody),
            Err(InlineRetryIneligible::OutputStreamSubscribed)
        );
        assert_eq!(
            is_http_inline_retry_eligible(&exec, &req, InlineRetryPhase::WritingRequestBody),
            Err(InlineRetryIneligible::OutputStreamSubscribed)
        );
    }

    #[test]
    fn test_has_outgoing_trailers_disqualifies() {
        let exec = make_exec_state();
        let mut req = make_request_state();
        req.retry.has_outgoing_trailers = true;
        assert_eq!(
            is_http_inline_retry_eligible(&exec, &req, InlineRetryPhase::AwaitingResponse),
            Err(InlineRetryIneligible::HasOutgoingTrailers)
        );
    }

    #[test]
    fn test_had_body_skip_disqualifies_resuming_response_body_only() {
        let exec = make_exec_state();
        let mut req = make_request_state();
        req.retry.had_body_skip = true;
        // AwaitingResponse should still be eligible
        assert!(
            is_http_inline_retry_eligible(&exec, &req, InlineRetryPhase::AwaitingResponse).is_ok()
        );
        // ResumingResponseBody should be disqualified
        assert_eq!(
            is_http_inline_retry_eligible(&exec, &req, InlineRetryPhase::ResumingResponseBody),
            Err(InlineRetryIneligible::HadBodySkip)
        );
    }

    #[test]
    fn test_body_not_finished_disqualifies_awaiting_response_only() {
        let exec = make_exec_state();
        let mut req = make_request_state();
        req.output_stream_rep = Some(1);
        req.retry.body_finished = false;
        req.output_stream_rep = Some(1);
        // AwaitingResponse should be disqualified
        assert_eq!(
            is_http_inline_retry_eligible(&exec, &req, InlineRetryPhase::AwaitingResponse),
            Err(InlineRetryIneligible::BodyNotFinished)
        );
        // ResumingResponseBody should still be eligible
        assert!(
            is_http_inline_retry_eligible(&exec, &req, InlineRetryPhase::ResumingResponseBody)
                .is_ok()
        );
    }

    #[test]
    fn test_no_output_stream_eligible_even_if_body_not_finished() {
        let exec = make_exec_state();
        let mut req = make_request_state();
        req.retry.body_finished = false;
        // output_stream_rep is None (no stream opened), so body_finished is irrelevant
        assert!(
            is_http_inline_retry_eligible(&exec, &req, InlineRetryPhase::AwaitingResponse).is_ok()
        );
        assert!(
            is_http_inline_retry_eligible(&exec, &req, InlineRetryPhase::ResumingResponseBody)
                .is_ok()
        );
    }

    #[test]
    fn test_non_idempotent_without_assume_idempotence_disqualifies() {
        let mut exec = make_exec_state();
        exec.assume_idempotence = false;
        let mut req = make_request_state();
        req.request.method = SerializableHttpMethod::Post;
        assert_eq!(
            is_http_inline_retry_eligible(&exec, &req, InlineRetryPhase::AwaitingResponse),
            Err(InlineRetryIneligible::NotIdempotent)
        );
    }

    #[test]
    fn test_idempotent_method_eligible_without_assume_idempotence() {
        let mut exec = make_exec_state();
        exec.assume_idempotence = false;
        let req = make_request_state(); // GET is idempotent
        assert!(
            is_http_inline_retry_eligible(&exec, &req, InlineRetryPhase::AwaitingResponse).is_ok()
        );
    }

    #[test]
    fn test_not_live_disqualifies() {
        let mut exec = make_exec_state();
        exec.is_live = false;
        let req = make_request_state();
        assert_eq!(
            is_http_inline_retry_eligible(&exec, &req, InlineRetryPhase::AwaitingResponse),
            Err(InlineRetryIneligible::NotLive)
        );
    }

    #[test]
    fn test_persist_nothing_disqualifies() {
        let mut exec = make_exec_state();
        exec.persistence_level = PersistenceLevel::PersistNothing;
        let req = make_request_state();
        assert_eq!(
            is_http_inline_retry_eligible(&exec, &req, InlineRetryPhase::AwaitingResponse),
            Err(InlineRetryIneligible::PersistNothing)
        );
    }
}
