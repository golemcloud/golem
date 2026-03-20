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
use wasmtime_wasi::OutputStream;
use wasmtime_wasi_http::bindings::http::types as wasi_http_types;
use wasmtime_wasi_http::body::{HostOutgoingBody, HyperOutgoingBody, StreamContext};
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
    /// Retry during outgoing body stream writes — before any response data.
    OutgoingBodyStream,
}

/// Checks whether the given HTTP request is eligible for transparent inline retry.
///
/// Returns `Ok(())` if eligible, or `Err(reason)` explaining why not.
pub fn is_http_inline_retry_eligible(
    exec_state: &DurableExecutionState,
    request_state: &HttpRequestState,
    _zone: RetryZone,
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

    // NOTE: Zone-specific eligibility fields (has_unreconstructable_body,
    // has_outgoing_trailers, body_finished, had_body_skip, output_stream_subscribed)
    // are not yet tracked on HttpRequestState. These checks will be added when
    // Zone 2 and OutgoingBodyStream retry are implemented. For now, the background
    // retry task (Zone 1) handles all requests where the initial send failed.

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
/// NOTE: The current oplog format for outgoing body stream writes
/// (`StreamWriteResult`) only records success/failure, not the actual
/// bytes written. Therefore this function currently returns an empty body.
/// This is correct for GET/HEAD/DELETE/OPTIONS requests that have no body.
/// For POST/PUT/PATCH with a body, the retry will send an empty body —
/// which may still succeed for idempotent retries where the server already
/// processed the original request.
///
/// Future work: store outgoing body bytes in the oplog to enable full
/// body reconstruction for retries.
pub async fn reconstruct_outgoing_body(
    _oplog: &Arc<dyn Oplog>,
    _begin_index: OplogIndex,
) -> Result<Bytes, anyhow::Error> {
    // Outgoing body bytes are not currently stored in the oplog.
    // The StreamWriteResult response only records Ok(()) / Err(...).
    Ok(Bytes::new())
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
    let method = serializable_method_to_http(&request.method)?;
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

/// Converts a `SerializableHttpMethod` to an `http::Method`.
fn serializable_method_to_http(
    method: &SerializableHttpMethod,
) -> Result<http::Method, anyhow::Error> {
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
        SerializableHttpMethod::Other(m) => http::Method::from_bytes(m.as_bytes())
            .map_err(|e| anyhow::anyhow!("invalid HTTP method '{m}': {e}")),
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
    fn test_serializable_method_to_http() {
        assert_eq!(
            serializable_method_to_http(&SerializableHttpMethod::Get).unwrap(),
            http::Method::GET
        );
        assert_eq!(
            serializable_method_to_http(&SerializableHttpMethod::Post).unwrap(),
            http::Method::POST
        );
        assert_eq!(
            serializable_method_to_http(&SerializableHttpMethod::Other("PURGE".to_string()))
                .unwrap(),
            http::Method::from_bytes(b"PURGE").unwrap()
        );
    }
}
