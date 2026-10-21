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

/// One unit read from the upstream response body by the durable task.
pub(in crate::durable_host::p3::http) enum HttpBodyFrame {
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

/// Reads the next meaningful frame from the upstream body, skipping empty data
/// frames so an empty frame is never persisted/delivered as a body chunk.
pub(in crate::durable_host::p3::http) async fn read_http_body_frame(
    body: &mut UnsyncBoxBody<Bytes, ErrorCode>,
) -> HttpBodyFrame {
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

/// Why the already-delivered prefix of a re-sent full response could not be skipped.
pub(in crate::durable_host::p3::http) enum SkipBodyPrefixError {
    /// The fresh body ended before the prefix was fully skipped: the resource
    /// shrank since the original response, so the delivered bytes cannot be
    /// continued.
    BodyTooShort,
    /// The fresh body's prefix differs from the bytes already delivered to the
    /// guest: the re-sent response is a different response (a nondeterministic
    /// producer, or a resource that changed), so continuing it would splice two
    /// responses together.
    Mismatch,
    /// Reading the fresh body failed while skipping.
    Read(ErrorCode),
}

/// Reads and discards the first `prefix` bytes of a fresh body, verifying that
/// they are exactly the bytes already delivered to the guest (whose digest the
/// consume-body task accumulates in `delivered`), and returns the remainder of
/// a data frame that straddled the prefix boundary, if any.
pub(in crate::durable_host::p3::http) async fn skip_body_prefix(
    body: &mut UnsyncBoxBody<Bytes, ErrorCode>,
    prefix: u64,
    delivered: &blake3::Hasher,
) -> Result<Option<Bytes>, SkipBodyPrefixError> {
    let mut remaining = prefix;
    let mut fresh = blake3::Hasher::new();
    let mut leftover = None;
    while remaining > 0 {
        match read_http_body_frame(body).await {
            HttpBodyFrame::Data(mut data) => {
                if (data.len() as u64) > remaining {
                    leftover = Some(data.split_off(remaining as usize));
                }
                remaining -= data.len() as u64;
                fresh.update(&data);
            }
            HttpBodyFrame::End(_) => return Err(SkipBodyPrefixError::BodyTooShort),
            HttpBodyFrame::Error(code) => return Err(SkipBodyPrefixError::Read(code)),
            HttpBodyFrame::Cancelled => {
                unreachable!("read_http_body_frame never produces Cancelled")
            }
        }
    }
    if fresh.finalize() != delivered.finalize() {
        return Err(SkipBodyPrefixError::Mismatch);
    }
    Ok(leftover)
}

/// How a replayed response whose body the guest already partly observed continues after a
/// restart.
pub(in crate::durable_host::p3::http) enum ResumeOutcome {
    RecoveryRequired(WorkerExecutorError),
    /// The fresh response continues the delivered prefix; stream its body from here on.
    Resumed {
        body: UnsyncBoxBody<Bytes, ErrorCode>,
        io_guard: AbortOnDropIoTask,
        /// The remainder of a data frame that straddled the prefix boundary.
        leftover: Option<Bytes>,
    },
    /// The resend failed on conversion or on the network; classified for worker-level retry
    /// like any live body failure.
    Failed(ErrorCode),
    /// The delivered prefix cannot be continued: the request cannot be resent, the provider
    /// neither honours the range nor reproduces the response, or the response changed. The body
    /// stream ends with an explicit incomplete-response error after the bytes the guest already
    /// observed; it is deterministic for this history, never retry-routed.
    Interrupted(String),
}

/// Continues a response body whose `delivered` byte prefix the guest observed before a
/// restart, by resending the recorded request with a `Range` header. A `206` continuing at the
/// delivered offset resumes directly; a full response with the original status is accepted
/// only when its prefix is byte-identical to the delivered bytes. A request that carries a
/// guest-set `Range` header is resent unchanged (composing a resume range with the guest's own
/// range is not supported), and any successful response is accepted only through the same
/// prefix verification. Everything else is an interrupted response: silently combining two
/// different responses is never an option.
pub(in crate::durable_host::p3::http) async fn resume_recorded_response<
    Ctx: WorkerCtx,
    U: 'static,
>(
    accessor: &Accessor<U, DurableP3<Ctx>>,
    resend: &P3HttpSendRebuild,
    delivered_bytes: u64,
    delivered: &blake3::Hasher,
) -> ResumeOutcome {
    let guest_range = has_guest_range_header(resend.request.headers.keys().map(String::as_str));
    let range_headers = if guest_range {
        Vec::new()
    } else {
        resume_range_headers(delivered_bytes)
    };
    match resend_recorded_request::<Ctx, U>(accessor, resend, &range_headers).await {
        ResendOutcome::RecoveryRequired(error) => ResumeOutcome::RecoveryRequired(error),
        ResendOutcome::Refused(reason) => ResumeOutcome::Interrupted(reason),
        ResendOutcome::Failed(code) => ResumeOutcome::Failed(code),
        ResendOutcome::Sent { response, io_guard } if guest_range => {
            let status = response.status().as_u16();
            if !(200..300).contains(&status) {
                return ResumeOutcome::Interrupted(format!(
                    "the re-sent request with the guest's own Range header returned status \
                     {status}, which cannot continue the delivered prefix"
                ));
            }
            let mut body = response.into_body();
            let skipped = skip_body_prefix(&mut body, delivered_bytes, delivered).await;
            if let Some(error) = io_guard.1.error() {
                return ResumeOutcome::RecoveryRequired(error);
            }
            match skipped {
                Ok(leftover) => ResumeOutcome::Resumed {
                    body,
                    io_guard,
                    leftover,
                },
                Err(SkipBodyPrefixError::BodyTooShort) => ResumeOutcome::Interrupted(
                    "the re-sent response body is shorter than the bytes already delivered to \
                     the guest"
                        .to_string(),
                ),
                Err(SkipBodyPrefixError::Mismatch) => ResumeOutcome::Interrupted(
                    "the re-sent response body differs from the bytes already delivered to the \
                     guest"
                        .to_string(),
                ),
                Err(SkipBodyPrefixError::Read(code)) => ResumeOutcome::Failed(code),
            }
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
                ResumeResponseAction::Resume => ResumeOutcome::Resumed {
                    body: response.into_body(),
                    io_guard,
                    leftover: None,
                },
                ResumeResponseAction::RangeNotSatisfiable => ResumeOutcome::Interrupted(
                    "the server returned 416 Range Not Satisfiable for the delivered prefix"
                        .to_string(),
                ),
                ResumeResponseAction::SkipPrefix => {
                    let mut body = response.into_body();
                    let skipped = skip_body_prefix(&mut body, delivered_bytes, delivered).await;
                    if let Some(error) = io_guard.1.error() {
                        return ResumeOutcome::RecoveryRequired(error);
                    }
                    match skipped {
                        Ok(leftover) => ResumeOutcome::Resumed {
                            body,
                            io_guard,
                            leftover,
                        },
                        Err(SkipBodyPrefixError::BodyTooShort) => ResumeOutcome::Interrupted(
                            "the re-sent response body is shorter than the bytes already \
                             delivered to the guest"
                                .to_string(),
                        ),
                        Err(SkipBodyPrefixError::Mismatch) => ResumeOutcome::Interrupted(
                            "the re-sent response body differs from the bytes already \
                             delivered to the guest"
                                .to_string(),
                        ),
                        Err(SkipBodyPrefixError::Read(code)) => ResumeOutcome::Failed(code),
                    }
                }
                ResumeResponseAction::Fallback => ResumeOutcome::Interrupted(format!(
                    "the re-sent request returned status {status} (recorded status {}, \
                     content-range start {content_range_start:?}), which cannot continue the \
                     delivered prefix",
                    resend.recorded_status
                )),
            }
        }
    }
}
