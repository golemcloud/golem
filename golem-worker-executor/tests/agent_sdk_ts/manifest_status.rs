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

use crate::Tracing;
use axum::Router;
use axum::extract::State;
use axum::routing::{get, post};
use golem_common::model::RetryConfig;
use golem_common::{agent_id, data_value};
use golem_test_framework::dsl::TestDsl;
use golem_worker_executor_test_utils::{
    LastUniqueId, PrecompiledComponent, TestContext, TestExecutorOverrides,
    WorkerExecutorTestDependencies, start_with_overrides,
};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use test_r::{inherit_test_dep, test, timeout};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tracing::Instrument;

use super::attempt_server::start_attempt_counter_server;
use super::manifest_http_5xx_retry_policy;

inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(LastUniqueId);
inherit_test_dep!(Tracing);
inherit_test_dep!(
    #[tagged_as("agent_sdk_ts")]
    PrecompiledComponent
);

/// Reproducer for the manifest-only HTTP 5xx retry path (the "V2" pattern):
/// the guest does plain `fetch + throw on !ok`, with NO `withRetryPolicy`
/// and NO `atomically`.  The retry policy is supplied entirely by the
/// environment state service (mirroring `retryPolicyDefaults` in
/// `golem.yaml`).
///
/// The server returns 500 for the first 3 requests then 200; the executor
/// must transparently re-issue the failing request 4 times until success.
async fn run_manifest_status_retry_test(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    agent_sdk_ts: &PrecompiledComponent,
    delay: Duration,
    fail_count: usize,
    agent_name: &str,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);

    let overrides = TestExecutorOverrides {
        retry_policies: Some(vec![manifest_http_5xx_retry_policy(
            "manifest-5xx-retry",
            delay,
        )]),
        ..Default::default()
    };
    let executor = start_with_overrides(deps, &context, overrides).await?;

    let (port, counter) = start_attempt_counter_server(fail_count).await;

    let component = executor
        .component_dep(&context.default_environment_id, agent_sdk_ts)
        .store()
        .await?;

    let agent_id = agent_id!("RetryTest", agent_name);
    executor
        .start_agent_with(&component.id, agent_id.clone(), HashMap::new(), Vec::new())
        .await?;

    let result = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "manifestStatusRetryTest",
            data_value!("localhost", port as f64),
        )
        .await?
        .into_typed::<bool>()?;

    assert!(result, "agent must complete successfully");
    let observed = counter.load(Ordering::SeqCst);
    assert!(
        observed > fail_count,
        "server must observe at least {} attempts, observed {observed}",
        fail_count + 1
    );

    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn ts_manifest_status_retry_immediate(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("agent_sdk_ts")] agent_sdk_ts: &PrecompiledComponent,
) -> anyhow::Result<()> {
    // Even with a near-zero delay, the manifest-only path must retry
    // transparently when the guest throws on !ok.
    run_manifest_status_retry_test(
        last_unique_id,
        deps,
        agent_sdk_ts,
        Duration::from_millis(1),
        3,
        "manifest-status-retry-immediate",
    )
    .await
}

#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn ts_manifest_status_retry_periodic_short(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("agent_sdk_ts")] agent_sdk_ts: &PrecompiledComponent,
) -> anyhow::Result<()> {
    // 200 ms periodic delay reproduces the V2 failure mode in the user's
    // chaos-backend smoke test (the host matches the policy and schedules
    // a non-zero delay; the in-flight retry must still re-issue the
    // request).
    run_manifest_status_retry_test(
        last_unique_id,
        deps,
        agent_sdk_ts,
        Duration::from_millis(200),
        3,
        "manifest-status-retry-periodic-short",
    )
    .await
}

/// Spawn a raw TCP "chaos backend" that mirrors the user's Node.js
/// `chaos-backend/server.ts` semantics on a single keep-alive connection:
///
///   - Reads the request line + headers (only enough to know how much to
///     respond), but **does NOT consume the request body** when failing.
///   - On the first `fail_count` requests it writes:
///     ```
///     HTTP/1.1 500 Internal Server Error
///     Content-Type: application/json
///     Content-Length: 38
///
///     {"error":"chaos: synthetic failure"}
///     ```
///     and **leaves the connection open** for keep-alive reuse, exactly like
///     Node.js's default HTTP server.
///   - After `fail_count` failures, on the next request it drains the body
///     and replies 200 OK with an empty JSON body.
///
/// Returning 5xx mid-request without consuming the request body is what makes
/// the host inline status-code retry resend path go wrong: Hyper sends the
/// resend on the same poisoned keep-alive connection, the leftover bytes from
/// the un-drained body get prepended to the new request line, the receiving
/// HTTP/1.1 parser (this server, like Node.js) then sees a malformed request
/// and replies HTTP 400 with no body. The guest sees a non-5xx response,
/// throws, traps, and the default trap policy gives up.
async fn start_chaos_post_server(
    fail_count: usize,
) -> (u16, Arc<AtomicUsize>, Arc<AtomicUsize>, Arc<AtomicUsize>) {
    let counter = Arc::new(AtomicUsize::new(0));
    let conn_counter = Arc::new(AtomicUsize::new(0));
    let bad_request_counter = Arc::new(AtomicUsize::new(0));
    let counter_clone = counter.clone();
    let conn_counter_clone = conn_counter.clone();
    let conn_counter_for_return = conn_counter.clone();
    let bad_request_counter_clone = bad_request_counter.clone();
    let bad_request_counter_for_return = bad_request_counter.clone();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();

    tokio::spawn(
        async move {
            loop {
                let (mut socket, _addr) = match listener.accept().await {
                    Ok(s) => s,
                    Err(_) => continue,
                };
                let conn_id = conn_counter_clone.fetch_add(1, Ordering::SeqCst) + 1;
                tracing::warn!(conn_id, "chaos-post server: NEW TCP CONNECTION accepted");
                let counter_inner = counter_clone.clone();
                let bad_request_counter_inner = bad_request_counter_clone.clone();
                tokio::spawn(async move {
                    let mut buf = [0u8; 8192];
                    let mut accumulated: Vec<u8> = Vec::new();
                    let mut requests_on_this_conn = 0usize;

                    loop {
                        // Read more bytes from the socket (or whatever's leftover from
                        // the previous undrained body sits in `accumulated`).
                        let n = match socket.read(&mut buf).await {
                            Ok(0) => return,
                            Ok(n) => n,
                            Err(_) => return,
                        };
                        accumulated.extend_from_slice(&buf[..n]);

                        // Find the end-of-headers marker.
                        let header_end = match find_double_crlf(&accumulated) {
                            Some(idx) => idx,
                            None => continue, // need more bytes for full headers
                        };

                        let attempt = counter_inner.fetch_add(1, Ordering::SeqCst) + 1;
                        requests_on_this_conn += 1;
                        tracing::warn!(
                            conn_id,
                            attempt,
                            requests_on_this_conn,
                            "chaos-post server: parsed request on this conn"
                        );

                        // Validate the request-line begins with "POST " — if it
                        // doesn't, this connection is poisoned (leftover body bytes
                        // fed back into the parser), reply 400 like Node.js does.
                        let valid_request =
                            accumulated.starts_with(b"POST ") || accumulated.starts_with(b"GET ");

                        if !valid_request {
                            let bad = bad_request_counter_inner.fetch_add(1, Ordering::SeqCst) + 1;
                            tracing::warn!(
                                attempt,
                                bad,
                                "chaos-post server received malformed request (poisoned keep-alive connection), replying 400"
                            );
                            let _ = socket
                                .write_all(
                                    b"HTTP/1.1 400 Bad Request\r\n\
                                      Content-Length: 0\r\n\
                                      Connection: close\r\n\r\n",
                                )
                                .await;
                            return;
                        }

                        if attempt <= fail_count {
                            tracing::info!(
                                attempt,
                                "chaos-post server: chaos-injecting 500 (NOT consuming body)"
                            );
                            let body = b"{\"error\":\"chaos: synthetic failure\"}";
                            let resp = format!(
                                "HTTP/1.1 500 Internal Server Error\r\n\
                                 Content-Type: application/json\r\n\
                                 Content-Length: {}\r\n\
                                 \r\n",
                                body.len()
                            );
                            if socket.write_all(resp.as_bytes()).await.is_err() {
                                return;
                            }
                            if socket.write_all(body).await.is_err() {
                                return;
                            }
                            // Discard the consumed headers; *leave* the body bytes
                            // sitting in `accumulated` (that's the bug-trigger
                            // condition — the body was never drained).
                            accumulated.drain(..header_end + 4);
                            // Loop and wait for more bytes (the resend) on the same
                            // keep-alive connection.
                        } else {
                            tracing::info!(attempt, "chaos-post server: success path 200");
                            // Drain the request body before responding.
                            let content_length =
                                content_length(&accumulated[..header_end]).unwrap_or(0);
                            let body_start = header_end + 4;
                            while accumulated.len() < body_start + content_length {
                                let n = match socket.read(&mut buf).await {
                                    Ok(0) => break,
                                    Ok(n) => n,
                                    Err(_) => return,
                                };
                                accumulated.extend_from_slice(&buf[..n]);
                            }
                            let _ = socket
                                .write_all(
                                    b"HTTP/1.1 200 OK\r\n\
                                      Content-Type: application/json\r\n\
                                      Content-Length: 2\r\n\
                                      Connection: close\r\n\r\n\
                                      {}",
                                )
                                .await;
                            return;
                        }
                    }
                });
            }
        }
        .in_current_span(),
    );

    (
        port,
        counter,
        conn_counter_for_return,
        bad_request_counter_for_return,
    )
}

fn find_double_crlf(data: &[u8]) -> Option<usize> {
    data.windows(4).position(|w| w == b"\r\n\r\n")
}

fn content_length(headers_raw: &[u8]) -> Option<usize> {
    let s = std::str::from_utf8(headers_raw).ok()?;
    for line in s.split("\r\n") {
        let lower = line.to_ascii_lowercase();
        if let Some(rest) = lower.strip_prefix("content-length:")
            && let Ok(n) = rest.trim().parse::<usize>()
        {
            return Some(n);
        }
    }
    None
}

/// Reproducer for the "V2" manifest-only HTTP 5xx retry path when the
/// failing request is a POST with a JSON body.
///
/// User code does:
///
///   const res = await fetch(url, {
///     method: 'POST',
///     headers: { 'content-type': 'application/json' },
///     body: JSON.stringify(payload),
///   });
///   if (!res.ok) throw new Error(...);
///
/// with NO `withRetryPolicy`, NO `atomically`. The retry comes
/// entirely from a `manifest-5xx-retry` named policy keyed on
/// `status-code`, which is the same shape the user has in their
/// `golem.yaml` `retryPolicyDefaults`.
///
/// Observed behavior in the user's chaos-backend smoke test:
///   - first POST returns 500 (chaos)
///   - host log: "HTTP status retry matched ... policy: manifest-5xx-retry"
///   - host log: "HTTP request body finished ... exposing response to retry path"
///   - host log: "HTTP response status matched user-defined retry policy;
///                retrying, delay: 2s"
///   - host re-sends the request via `default_send_request_with_pool`
///   - the resent request comes back with HTTP 400 (no Content-Type header,
///     empty body — looks like a wire-protocol-level rejection by the
///     receiving HTTP server, not a chaos-injected response)
///   - guest sees res.status === 400 (not 5xx), throws, traps
///   - default trap retry exhausts (3 attempts) → agent goes to Failed
///
/// Expected behavior: same as the GET variant
/// (`ts_manifest_status_retry_periodic_short`) — the host should
/// transparently re-issue the failing POST request including its
/// `Content-Type` header and JSON body until the server returns 200.
#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn ts_manifest_status_retry_post_with_json_body(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("agent_sdk_ts")] agent_sdk_ts: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);

    let overrides = TestExecutorOverrides {
        retry_policies: Some(vec![manifest_http_5xx_retry_policy(
            "manifest-5xx-retry",
            Duration::from_millis(50),
        )]),
        configure: Some(Arc::new(|config| {
            // Tighten trap retry to 3 attempts with tiny delays so this test
            // fails fast (not via timeout) when the bug is present.
            config.retry = RetryConfig {
                max_attempts: 3,
                min_delay: Duration::from_millis(1),
                max_delay: Duration::from_millis(10),
                multiplier: 1.0,
                max_jitter_factor: None,
            };
        })),
        ..Default::default()
    };
    let executor = start_with_overrides(deps, &context, overrides).await?;

    // Server returns chaos-style 500 (does NOT consume request body, leaves
    // keep-alive connection open) for the first `fail_count` requests, then
    // 200 OK on the next. With the host bug the very first inline retry
    // reuses the poisoned keep-alive connection and the server replies
    // HTTP 400 to the resend (un-drained body bytes prepended to the new
    // request line). 400 is NOT in the 5xx retry policy predicate, so the
    // guest sees a non-5xx response, throws, traps, and only the trap
    // retry budget keeps the invocation alive.
    //
    // We deliberately keep `fail_count` small enough that, in the absence
    // of the bug, the host's inline 5xx retry would settle the invocation
    // in a fraction of a second and the guest would never observe a 4xx.
    // The hard assertion below — `bad_requests == 0` — is what catches the
    // bug regardless of whether the agent eventually succeeds via trap
    // replay; a single 400 surfaced to the guest means the host's resend
    // path poisoned a keep-alive connection.
    let fail_count: usize = 5;
    let (port, counter, conn_counter, bad_requests) = start_chaos_post_server(fail_count).await;

    let component = executor
        .component_dep(&context.default_environment_id, agent_sdk_ts)
        .store()
        .await?;

    let agent_id = agent_id!("RetryTest", "manifest-status-retry-post-json");
    executor
        .start_agent_with(&component.id, agent_id.clone(), HashMap::new(), Vec::new())
        .await?;

    let result = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "manifestStatusRetryPostTest",
            data_value!("localhost", port as f64),
        )
        .await?
        .into_typed::<bool>()?;

    let observed = counter.load(Ordering::SeqCst);
    let observed_conns = conn_counter.load(Ordering::SeqCst);
    let observed_bad = bad_requests.load(Ordering::SeqCst);
    eprintln!(
        "chaos-post server observed: {observed} requests over {observed_conns} TCP connections, {observed_bad} of which were poisoned (400)"
    );

    // Hard assertion: the host MUST NOT surface any 400 Bad Request
    // responses to the guest. Each 400 here means the host inline
    // status-code retry resent on a keep-alive connection that still had
    // un-drained request body bytes from the previous attempt, which the
    // receiving HTTP/1.1 parser then rejected as malformed. This is the
    // exact failure mode the user observed against `chaos-backend`.
    assert_eq!(
        observed_bad, 0,
        "host inline status-code retry resent on a poisoned keep-alive connection: \
         server saw {observed_bad} malformed (400) requests over {observed_conns} \
         TCP connections; this surfaces a non-5xx response to the guest and \
         bypasses the manifest 5xx retry policy"
    );

    assert!(
        result,
        "agent must complete successfully (host inline status-code retry must transparently re-issue the failing POST request with its JSON body)"
    );

    assert!(
        observed > fail_count,
        "server must observe at least {} POST attempts, observed {observed}",
        fail_count + 1
    );

    Ok(())
}

// ---------------------------------------------------------------------------
// V2 reproducer tests
//
// The following three tests reproduce the regressions documented in the V2
// chaos-backend smoke test. They share a common manifest setup: the executor
// is configured with the `manifest-5xx-retry` named policy (matching
// `retryPolicyDefaults: http-5xx-retry` in the user's golem.yaml) and a
// tightened `RetryConfig` so failure paths terminate within the test
// `#[timeout("2m")]` rather than hammering the backend for minutes on end.
//
// The guest methods are plain `await fetch(...) ; throw on !ok` chains with
// NO `withRetryPolicy` and NO `atomically` — the V2 manifest-only shape.
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct StepCounterState {
    counter_a: Arc<AtomicUsize>,
    counter_b: Arc<AtomicUsize>,
    fail_count_a: usize,
    fail_count_b: usize,
}

async fn step_a_handler(State(state): State<StepCounterState>) -> axum::response::Response {
    let attempt = state.counter_a.fetch_add(1, Ordering::SeqCst) + 1;
    let status = if attempt <= state.fail_count_a {
        500
    } else {
        200
    };
    axum::response::Response::builder()
        .status(status)
        .body(axum::body::Body::empty())
        .unwrap()
}

async fn step_b_handler(State(state): State<StepCounterState>) -> axum::response::Response {
    let attempt = state.counter_b.fetch_add(1, Ordering::SeqCst) + 1;
    let status = if attempt <= state.fail_count_b {
        500
    } else {
        200
    };
    axum::response::Response::builder()
        .status(status)
        .body(axum::body::Body::empty())
        .unwrap()
}

async fn start_two_step_get_server(
    fail_count_a: usize,
    fail_count_b: usize,
) -> (u16, Arc<AtomicUsize>, Arc<AtomicUsize>) {
    let counter_a = Arc::new(AtomicUsize::new(0));
    let counter_b = Arc::new(AtomicUsize::new(0));
    let state = StepCounterState {
        counter_a: counter_a.clone(),
        counter_b: counter_b.clone(),
        fail_count_a,
        fail_count_b,
    };
    let app = Router::new()
        .route("/step-a", get(step_a_handler))
        .route("/step-b", get(step_b_handler))
        .with_state(state);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(
        async move {
            axum::serve(listener, app).await.unwrap();
        }
        .in_current_span(),
    );
    (port, counter_a, counter_b)
}

/// V2 reproducer #1: the host's inline status-code retry must re-arm for
/// EVERY outgoing request in the same invocation, not just the first.
///
/// The agent does two sequential GETs to two different routes, each of
/// which fails 3 times before returning 200. With the bug present, the
/// first GET succeeds via inline retry but the second's 5xx escapes to
/// the guest, which throws; the default trap-retry exhausts and the
/// agent ends `Failed`. After the fix, both GETs must succeed
/// transparently.
#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn ts_manifest_status_retry_two_sequential_calls_are_both_re_armed(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("agent_sdk_ts")] agent_sdk_ts: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);

    let overrides = TestExecutorOverrides {
        retry_policies: Some(vec![manifest_http_5xx_retry_policy(
            "manifest-5xx-retry",
            Duration::from_millis(50),
        )]),
        configure: Some(Arc::new(|config| {
            config.retry = RetryConfig {
                max_attempts: 3,
                min_delay: Duration::from_millis(1),
                max_delay: Duration::from_millis(10),
                multiplier: 1.0,
                max_jitter_factor: None,
            };
        })),
        ..Default::default()
    };
    let executor = start_with_overrides(deps, &context, overrides).await?;

    let fail_count: usize = 3;
    let (port, counter_a, counter_b) = start_two_step_get_server(fail_count, fail_count).await;

    let component = executor
        .component_dep(&context.default_environment_id, agent_sdk_ts)
        .store()
        .await?;

    let agent_id = agent_id!("RetryTest", "manifest-status-retry-two-step");
    executor
        .start_agent_with(&component.id, agent_id.clone(), HashMap::new(), Vec::new())
        .await?;

    let result = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "manifestStatusRetryTwoStepGet",
            data_value!("127.0.0.1", port as f64),
        )
        .await?
        .into_typed::<bool>()?;

    let observed_a = counter_a.load(Ordering::SeqCst);
    let observed_b = counter_b.load(Ordering::SeqCst);
    eprintln!(
        "two-step server observed: /step-a {observed_a} requests, /step-b {observed_b} requests"
    );

    assert!(
        result,
        "agent must complete successfully (manifest 5xx retry must re-arm for the second call too)"
    );
    assert_eq!(
        observed_a,
        fail_count + 1,
        "/step-a must be retried inline exactly {} times to reach the success response, observed {observed_a}",
        fail_count + 1
    );
    assert_eq!(
        observed_b,
        fail_count + 1,
        "/step-b must be retried inline exactly {} times to reach the success response, observed {observed_b}",
        fail_count + 1
    );

    Ok(())
}

/// Spawn a raw TCP server with two routes:
///
/// - `GET /ok` → reads the request, replies `HTTP/1.1 200 OK`.
/// - `GET /crash` → reads the request headers, increments the crash counter,
///   then drops the socket without sending any response bytes.
async fn start_ok_then_crash_server() -> (u16, Arc<AtomicUsize>, Arc<AtomicUsize>) {
    let ok_counter = Arc::new(AtomicUsize::new(0));
    let crash_counter = Arc::new(AtomicUsize::new(0));
    let ok_for_task = ok_counter.clone();
    let crash_for_task = crash_counter.clone();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();

    tokio::spawn(
        async move {
            loop {
                let (mut socket, _addr) = match listener.accept().await {
                    Ok(s) => s,
                    Err(_) => continue,
                };
                let ok_inner = ok_for_task.clone();
                let crash_inner = crash_for_task.clone();
                tokio::spawn(async move {
                    let mut buf = [0u8; 8192];
                    let mut accumulated: Vec<u8> = Vec::new();
                    loop {
                        let n = match socket.read(&mut buf).await {
                            Ok(0) => return,
                            Ok(n) => n,
                            Err(_) => return,
                        };
                        accumulated.extend_from_slice(&buf[..n]);
                        if find_double_crlf(&accumulated).is_none() {
                            continue;
                        }
                        let request_line_end =
                            accumulated.iter().position(|&b| b == b'\n').unwrap_or(0);
                        let request_line =
                            std::str::from_utf8(&accumulated[..request_line_end]).unwrap_or("");
                        if request_line.contains("/crash") {
                            crash_inner.fetch_add(1, Ordering::SeqCst);
                            // Drop the socket without writing any response. Hyper
                            // observes a connection-terminated transport error.
                            return;
                        } else if request_line.contains("/ok") {
                            ok_inner.fetch_add(1, Ordering::SeqCst);
                            let _ = socket
                                .write_all(
                                    b"HTTP/1.1 200 OK\r\n\
                                      Content-Length: 0\r\n\
                                      Connection: close\r\n\r\n",
                                )
                                .await;
                            return;
                        } else {
                            let _ = socket
                                .write_all(
                                    b"HTTP/1.1 404 Not Found\r\n\
                                      Content-Length: 0\r\n\
                                      Connection: close\r\n\r\n",
                                )
                                .await;
                            return;
                        }
                    }
                });
            }
        }
        .in_current_span(),
    );

    (port, ok_counter, crash_counter)
}

/// V2 reproducer #3 (S3 — "process-crash mid-call"): a GET to `/ok`
/// succeeds, then a GET to `/crash` accepts the connection and drops the
/// socket without sending any response. Hyper surfaces a transport error
/// (`HttpProtocolError`/`ConnectionTerminated`), classified as transient
/// but NOT matching the `http-5xx-retry` predicate. After the fix, the
/// worker's transport-error trap path must bound the retry count to
/// `RetryConfig::max_attempts` rather than entering the unbounded
/// ~1091-call replay loop the user observed.
#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn ts_manifest_status_retry_ok_then_crash_is_bounded(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("agent_sdk_ts")] agent_sdk_ts: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);

    let overrides = TestExecutorOverrides {
        retry_policies: Some(vec![manifest_http_5xx_retry_policy(
            "manifest-5xx-retry",
            Duration::from_millis(50),
        )]),
        configure: Some(Arc::new(|config| {
            config.retry = RetryConfig {
                max_attempts: 3,
                min_delay: Duration::from_millis(1),
                max_delay: Duration::from_millis(10),
                multiplier: 1.0,
                max_jitter_factor: None,
            };
        })),
        ..Default::default()
    };
    let executor = start_with_overrides(deps, &context, overrides).await?;

    let (port, ok_counter, crash_counter) = start_ok_then_crash_server().await;

    let component = executor
        .component_dep(&context.default_environment_id, agent_sdk_ts)
        .store()
        .await?;

    let agent_id = agent_id!("RetryTest", "manifest-status-retry-ok-then-crash");
    executor
        .start_agent_with(&component.id, agent_id.clone(), HashMap::new(), Vec::new())
        .await?;

    let result = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "manifestStatusRetryOkThenCrash",
            data_value!("127.0.0.1", port as f64),
        )
        .await;

    let observed_ok = ok_counter.load(Ordering::SeqCst);
    let observed_crash = crash_counter.load(Ordering::SeqCst);
    eprintln!(
        "ok-then-crash server observed: /ok {observed_ok} requests, /crash {observed_crash} requests; agent result is_err={}",
        result.is_err()
    );

    assert!(
        result.is_err(),
        "agent must terminate with an error after the host gives up retrying the crashing call"
    );
    assert_eq!(observed_ok, 1, "/ok must be hit live exactly once");
    assert!(
        observed_crash <= 3,
        "/crash request count must be bounded by max_attempts=3, observed {observed_crash}"
    );

    Ok(())
}

/// Spawn an axum server with two POST routes:
///
/// - `POST /ok-post` → returns 200.
/// - `POST /perma-500` → always returns 500.
async fn start_v2_post_server() -> (u16, Arc<AtomicUsize>, Arc<AtomicUsize>) {
    let ok_counter = Arc::new(AtomicUsize::new(0));
    let bad_counter = Arc::new(AtomicUsize::new(0));

    #[derive(Clone)]
    struct V2State {
        ok: Arc<AtomicUsize>,
        bad: Arc<AtomicUsize>,
    }
    let state = V2State {
        ok: ok_counter.clone(),
        bad: bad_counter.clone(),
    };

    let router = Router::new()
        .route(
            "/ok-post",
            post(|State(s): State<V2State>| async move {
                s.ok.fetch_add(1, Ordering::SeqCst);
                axum::response::Response::builder()
                    .status(200)
                    .header("content-type", "application/json")
                    .body(axum::body::Body::from(r#"{"ok":true}"#))
                    .unwrap()
            }),
        )
        .route(
            "/perma-500",
            post(|State(s): State<V2State>| async move {
                s.bad.fetch_add(1, Ordering::SeqCst);
                axum::response::Response::builder()
                    .status(500)
                    .body(axum::body::Body::from("permanent failure"))
                    .unwrap()
            }),
        )
        .with_state(state);

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();

    tokio::spawn(
        async move {
            let _ = axum::serve(listener, router).await;
        }
        .in_current_span(),
    );

    (port, ok_counter, bad_counter)
}

/// V2 reproducer (CheckoutAgentV2 / "ok then forever-500"): the agent does a
/// successful POST followed by a POST that always returns 500. With a
/// manifest `http-5xx-retry` policy whose `maxRetries` is large (1000 in
/// this test setup), the host transparently re-sends the second POST many
/// times before fetch() resolves to user code. After the policy budget is
/// exhausted, fetch returns 500, the agent throws, and the worker's
/// trap-replay path takes over.
///
/// The test asserts that `/perma-500` is hit at most `maxRetries + 1` times
/// in total, regardless of the worker-level `RetryConfig`. If the V2
/// regression were active, the host-side trap-replay loop would re-issue
/// the POST many more times than the policy permits.
#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn ts_manifest_status_retry_v2_ok_then_forever_500_is_bounded(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("agent_sdk_ts")] agent_sdk_ts: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);

    // Manifest policy budget: at most 1000 inline retries on 5xx.
    let manifest_policy_budget: u32 = 1000;
    // Worker-level retry budget kept tiny so the trap-replay path can't
    // multiply the policy budget into a much larger live request count.
    let max_attempts: u32 = 3;

    let overrides = TestExecutorOverrides {
        retry_policies: Some(vec![manifest_http_5xx_retry_policy(
            "manifest-5xx-retry",
            // Tiny periodic delay so 1000 retries finish within the test
            // wall-clock budget.
            Duration::from_millis(2),
        )]),
        configure: Some(Arc::new(move |config| {
            config.retry = RetryConfig {
                max_attempts,
                min_delay: Duration::from_millis(1),
                max_delay: Duration::from_millis(10),
                multiplier: 1.0,
                max_jitter_factor: None,
            };
        })),
        ..Default::default()
    };
    let executor = start_with_overrides(deps, &context, overrides).await?;

    let (port, ok_counter, bad_counter) = start_v2_post_server().await;

    let component = executor
        .component_dep(&context.default_environment_id, agent_sdk_ts)
        .store()
        .await?;

    let agent_id = agent_id!("RetryTest", "manifest-status-retry-v2-perma-500");
    executor
        .start_agent_with(&component.id, agent_id.clone(), HashMap::new(), Vec::new())
        .await?;

    // Per-request guest deadline: 30 seconds. Long enough to never fire in this
    // test (the server responds promptly), so we test pure status-code retry.
    let request_timeout_ms: f64 = 30_000.0;
    let result = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "manifestStatusRetryV2OkThenForever500",
            data_value!("127.0.0.1", port as f64, request_timeout_ms),
        )
        .await;

    let observed_ok = ok_counter.load(Ordering::SeqCst);
    let observed_bad = bad_counter.load(Ordering::SeqCst);
    eprintln!(
        "v2-perma-500 server observed: /ok-post {observed_ok} requests, /perma-500 {observed_bad} requests; agent result is_err={}",
        result.is_err()
    );

    assert!(
        result.is_err(),
        "agent must terminate with an error after the manifest 5xx retry budget is exhausted"
    );
    assert_eq!(
        observed_ok, 1,
        "/ok-post must be hit live exactly once, observed {observed_ok}"
    );
    // Hard upper bound: even allowing for the worker's max_attempts trap-replay
    // multiplication, total /perma-500 requests must be at most
    // (manifest_policy_budget + 1) * max_attempts. If the V2 loop is active,
    // observed_bad will blow well past this.
    let hard_cap = (manifest_policy_budget as usize + 1) * max_attempts as usize;
    assert!(
        observed_bad <= hard_cap,
        "/perma-500 request count must be bounded by manifest+trap-retry budget = {hard_cap}, observed {observed_bad}"
    );

    Ok(())
}
