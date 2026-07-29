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

//! Preview-neutral policy for durable outgoing HTTP.
//!
//! The WASI P2 (`durable_host::http`) and P3 (`durable_host::p3::http`) host
//! implementations differ heavily in *mechanics* (P2 swaps stream resources in
//! the resource table, P3 splices body frames inside a spawned task), but they
//! must agree on *policy*: which methods are idempotent, when a transparent
//! retry is allowed at all, which Golem-managed headers are injected into an
//! outgoing request, and how a Range-based response-body resume response is
//! interpreted. This module is the single home for those decisions; both
//! preview trees call into it instead of maintaining parallel copies.

use crate::durable_host::DurableWorkerCtx;
use crate::durable_host::durability::DurableExecutionState;
use crate::workerctx::WorkerCtx;
use golem_common::model::invocation_context::SpanId;
use golem_common::model::oplog::types::SerializableHttpMethod;
use golem_common::model::oplog::{OplogIndex, PersistenceLevel};
use golem_service_base::error::worker_executor::WorkerExecutorError;
use golem_service_base::headers::TraceContextHeaders;
use http::{HeaderName, HeaderValue};
use wasmtime_wasi_http::FieldMap;

/// Returns true if the HTTP method is idempotent per RFC 9110 §9.2.2: the safe
/// methods (GET, HEAD, OPTIONS, TRACE) plus PUT and DELETE. Idempotent methods
/// are safe to transparently re-send even when the worker does not assume
/// idempotence for remote writes.
pub(crate) fn is_idempotent_http_method(method: &SerializableHttpMethod) -> bool {
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

/// Whether a request may be transparently re-sent: either the worker-level
/// `assume_idempotence` override is on, or the method itself is idempotent.
pub(crate) fn is_http_request_idempotent(
    assume_idempotence: bool,
    method: &SerializableHttpMethod,
) -> bool {
    assume_idempotence || is_idempotent_http_method(method)
}

/// Reasons why the worker state forbids any transparent HTTP retry
/// (inline resume, get-time resend, or background retry).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HttpRetryDisallowedReason {
    /// Worker is in replay mode (not live).
    NotLive,
    /// Worker is in snapshotting mode.
    Snapshotting,
    /// Persistence level is PersistNothing — no oplog data to reconstruct from.
    PersistNothing,
    /// Worker is inside a user-defined atomic region; a failure must escalate
    /// to trap+replay so the whole region re-executes.
    InAtomicRegion,
    /// The request method is not idempotent and `assume_idempotence` is false.
    NotIdempotent,
}

/// Checks the worker-state conditions common to every transparent HTTP retry
/// mechanism, on both previews. Method idempotence is checked separately (see
/// [`http_transparent_retry_allowed`]) because P2's inline-retry eligibility
/// interleaves preview-specific body-state checks before it.
pub(crate) fn http_worker_state_allows_retry(
    exec_state: &DurableExecutionState,
    in_atomic_region: bool,
) -> Result<(), HttpRetryDisallowedReason> {
    if !exec_state.is_live {
        return Err(HttpRetryDisallowedReason::NotLive);
    }
    if exec_state.snapshotting_mode.is_some() {
        return Err(HttpRetryDisallowedReason::Snapshotting);
    }
    if exec_state.persistence_level == PersistenceLevel::PersistNothing {
        return Err(HttpRetryDisallowedReason::PersistNothing);
    }
    if in_atomic_region {
        return Err(HttpRetryDisallowedReason::InAtomicRegion);
    }
    Ok(())
}

/// The full preview-neutral gate: worker state plus method idempotence.
pub(crate) fn http_transparent_retry_allowed(
    exec_state: &DurableExecutionState,
    in_atomic_region: bool,
    method: &SerializableHttpMethod,
) -> Result<(), HttpRetryDisallowedReason> {
    http_worker_state_allows_retry(exec_state, in_atomic_region)?;
    if !is_http_request_idempotent(exec_state.assume_idempotence, method) {
        return Err(HttpRetryDisallowedReason::NotIdempotent);
    }
    Ok(())
}

/// Parses the start byte position from a Content-Range header value.
///
/// Expected format: `bytes <start>-<end>/<total>` or `bytes <start>-<end>/*`
/// Returns the start position if successfully parsed.
pub(crate) fn parse_content_range_start(value: &str) -> Option<u64> {
    let rest = value.strip_prefix("bytes ")?;
    let dash_pos = rest.find('-')?;
    rest[..dash_pos].parse::<u64>().ok()
}

/// Returns true if the guest set its own `Range` header on the request.
/// Response-body resume is not supported for such requests, because composing
/// resume-range semantics on top of the guest's own range is not supported.
pub(crate) fn has_guest_range_header<'a>(mut header_names: impl Iterator<Item = &'a str>) -> bool {
    header_names.any(|name| name.eq_ignore_ascii_case("range"))
}

/// The extra headers of a Range-based response-body resume request: a
/// `Range: bytes=<delivered>-` header when a body prefix was already delivered
/// to the guest, and nothing otherwise.
pub(crate) fn resume_range_headers(delivered: u64) -> Vec<(String, String)> {
    if delivered > 0 {
        vec![("range".to_string(), format!("bytes={delivered}-"))]
    } else {
        Vec::new()
    }
}

/// How to proceed with the response received for a Range-based response-body
/// resume request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ResumeResponseAction {
    /// 206 Partial Content with a Content-Range starting exactly at the
    /// delivered byte count: continue the guest-facing stream with the new
    /// body as-is.
    Resume,
    /// The server ignored the range and re-sent the full response with the
    /// original status: skip the already-delivered prefix (count-only, no
    /// content verification) and continue from there. The delivered count may
    /// be zero, in which case there is nothing to skip.
    SkipPrefix,
    /// 416 Range Not Satisfiable: the resource changed since the original
    /// response, so the already-delivered prefix cannot be continued.
    /// Deterministic for this request — must not be retry-routed.
    RangeNotSatisfiable,
    /// Anything else (mismatched or missing Content-Range on a 206, or an
    /// unexpected status): give up on resuming and fall back to the caller's
    /// regular failure handling.
    Fallback,
}

/// Classifies the response of a resume request.
///
/// The checks are ordered 206, then 416, then original-status match: a 416 to
/// a Range request is always a range refusal, even when the original response
/// happened to have status 416.
pub(crate) fn classify_resume_response(
    status: u16,
    content_range_start: Option<u64>,
    delivered: u64,
    original_status: Option<u16>,
) -> ResumeResponseAction {
    if status == 206 {
        if content_range_start == Some(delivered) {
            ResumeResponseAction::Resume
        } else {
            ResumeResponseAction::Fallback
        }
    } else if status == 416 {
        ResumeResponseAction::RangeNotSatisfiable
    } else if original_status == Some(status) {
        ResumeResponseAction::SkipPrefix
    } else {
        ResumeResponseAction::Fallback
    }
}

/// Computes the Golem-managed headers to inject into an outgoing HTTP request:
/// the trace-context headers of the request's invocation span (when
/// `forward_trace_context_headers` is enabled) and an `idempotency-key`
/// derived from the request's own begin oplog index (when
/// `set_outgoing_http_idempotency_key` is enabled and the guest did not set
/// the header itself). The begin index is stable across live execution and
/// replay, so a retried send reuses the same key.
pub(crate) fn golem_managed_http_headers<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    span_id: &SpanId,
    begin_index: OplogIndex,
    guest_has_idempotency_key: bool,
) -> Result<Vec<(String, String)>, WorkerExecutorError> {
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
    if ctx.state.set_outgoing_http_idempotency_key && !guest_has_idempotency_key {
        let idempotency_key = ctx.derive_idempotency_key(begin_index);
        headers.push(("idempotency-key".to_string(), idempotency_key.to_string()));
    }
    Ok(headers)
}

/// Applies the Golem-managed headers to a request's header map, replacing any
/// existing values for the same names. The guest constructed the request with
/// immutable headers, so they are briefly remarked mutable around the
/// injection.
pub(crate) fn apply_managed_http_headers(
    headers: &mut FieldMap,
    field_size_limit: usize,
    managed: &[(String, String)],
) -> Result<(), String> {
    if managed.is_empty() {
        return Ok(());
    }
    headers.set_mutable(field_size_limit);
    let mut result = Ok(());
    for (name, value) in managed {
        let header_name = match HeaderName::try_from(name.as_str()) {
            Ok(name) => name,
            Err(err) => {
                result = Err(format!("invalid injected header name {name}: {err}"));
                break;
            }
        };
        let header_value = match HeaderValue::try_from(value.as_str()) {
            Ok(value) => value,
            Err(err) => {
                result = Err(format!("invalid injected header value for {name}: {err}"));
                break;
            }
        };
        let _ = headers.remove_all(header_name.clone());
        if let Err(err) = headers.append(header_name, header_value) {
            result = Err(format!(
                "failed to inject header {name} into outgoing HTTP request: {err:?}"
            ));
            break;
        }
    }
    headers.set_immutable();
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_r::test;

    #[test]
    fn idempotent_methods_follow_rfc_9110() {
        assert!(is_idempotent_http_method(&SerializableHttpMethod::Get));
        assert!(is_idempotent_http_method(&SerializableHttpMethod::Head));
        assert!(is_idempotent_http_method(&SerializableHttpMethod::Put));
        assert!(is_idempotent_http_method(&SerializableHttpMethod::Delete));
        assert!(is_idempotent_http_method(&SerializableHttpMethod::Options));
        assert!(is_idempotent_http_method(&SerializableHttpMethod::Trace));
        assert!(!is_idempotent_http_method(&SerializableHttpMethod::Post));
        assert!(!is_idempotent_http_method(&SerializableHttpMethod::Patch));
        assert!(!is_idempotent_http_method(&SerializableHttpMethod::Connect));
        assert!(!is_idempotent_http_method(&SerializableHttpMethod::Other(
            "CUSTOM".to_string()
        )));
    }

    #[test]
    fn assume_idempotence_overrides_method() {
        assert!(is_http_request_idempotent(
            true,
            &SerializableHttpMethod::Post
        ));
        assert!(!is_http_request_idempotent(
            false,
            &SerializableHttpMethod::Post
        ));
    }

    fn live_exec_state() -> DurableExecutionState {
        DurableExecutionState {
            is_live: true,
            persistence_level: PersistenceLevel::Smart,
            snapshotting_mode: None,
            assume_idempotence: false,
            max_in_function_retry_delay: std::time::Duration::from_secs(1),
        }
    }

    #[test]
    fn worker_state_gate() {
        assert_eq!(
            http_worker_state_allows_retry(&live_exec_state(), false),
            Ok(())
        );
        assert_eq!(
            http_worker_state_allows_retry(
                &DurableExecutionState {
                    is_live: false,
                    ..live_exec_state()
                },
                false
            ),
            Err(HttpRetryDisallowedReason::NotLive)
        );
        assert_eq!(
            http_worker_state_allows_retry(
                &DurableExecutionState {
                    snapshotting_mode: Some(PersistenceLevel::Smart),
                    ..live_exec_state()
                },
                false
            ),
            Err(HttpRetryDisallowedReason::Snapshotting)
        );
        assert_eq!(
            http_worker_state_allows_retry(
                &DurableExecutionState {
                    persistence_level: PersistenceLevel::PersistNothing,
                    ..live_exec_state()
                },
                false
            ),
            Err(HttpRetryDisallowedReason::PersistNothing)
        );
        assert_eq!(
            http_worker_state_allows_retry(&live_exec_state(), true),
            Err(HttpRetryDisallowedReason::InAtomicRegion)
        );
    }

    #[test]
    fn full_gate_checks_idempotence() {
        assert_eq!(
            http_transparent_retry_allowed(
                &live_exec_state(),
                false,
                &SerializableHttpMethod::Post
            ),
            Err(HttpRetryDisallowedReason::NotIdempotent)
        );
        assert_eq!(
            http_transparent_retry_allowed(&live_exec_state(), false, &SerializableHttpMethod::Get),
            Ok(())
        );
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

    #[test]
    fn guest_range_header_detection() {
        assert!(has_guest_range_header(["Range"].into_iter()));
        assert!(has_guest_range_header(
            ["content-type", "RANGE"].into_iter()
        ));
        assert!(!has_guest_range_header(["content-range"].into_iter()));
        assert!(!has_guest_range_header(std::iter::empty()));
    }

    #[test]
    fn resume_range_header_only_after_delivery() {
        assert_eq!(
            resume_range_headers(100),
            vec![("range".to_string(), "bytes=100-".to_string())]
        );
        assert!(resume_range_headers(0).is_empty());
    }

    #[test]
    fn resume_response_classification() {
        // 206 with matching Content-Range start
        assert_eq!(
            classify_resume_response(206, Some(100), 100, Some(200)),
            ResumeResponseAction::Resume
        );
        // 206 with mismatching or missing Content-Range
        assert_eq!(
            classify_resume_response(206, Some(50), 100, Some(200)),
            ResumeResponseAction::Fallback
        );
        assert_eq!(
            classify_resume_response(206, None, 100, Some(200)),
            ResumeResponseAction::Fallback
        );
        // full response with the original status
        assert_eq!(
            classify_resume_response(200, None, 100, Some(200)),
            ResumeResponseAction::SkipPrefix
        );
        assert_eq!(
            classify_resume_response(200, None, 0, Some(200)),
            ResumeResponseAction::SkipPrefix
        );
        // 416 wins over an original 416 status
        assert_eq!(
            classify_resume_response(416, None, 100, Some(416)),
            ResumeResponseAction::RangeNotSatisfiable
        );
        assert_eq!(
            classify_resume_response(416, None, 100, Some(200)),
            ResumeResponseAction::RangeNotSatisfiable
        );
        // status change
        assert_eq!(
            classify_resume_response(500, None, 100, Some(200)),
            ResumeResponseAction::Fallback
        );
        assert_eq!(
            classify_resume_response(200, None, 100, None),
            ResumeResponseAction::Fallback
        );
    }
}
