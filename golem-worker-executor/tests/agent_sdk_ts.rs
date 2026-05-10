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
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use golem_api_grpc::proto::golem::worker::{LogEvent, log_event};
use golem_common::model::RetryConfig;
use golem_common::model::retry_policy::{NamedRetryPolicy, Predicate, PredicateValue, RetryPolicy};
use golem_common::{agent_id, data_value};
use golem_test_framework::dsl::{TestDsl, drain_connection};
use golem_wasm::Value;
use golem_worker_executor_test_utils::{
    LastUniqueId, PrecompiledComponent, TestContext, TestExecutorOverrides,
    WorkerExecutorTestDependencies, start, start_with_overrides,
};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use test_r::{inherit_test_dep, test, timeout};
use tokio::net::TcpListener;
use tracing::Instrument;

inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(LastUniqueId);
inherit_test_dep!(Tracing);
inherit_test_dep!(
    #[tagged_as("agent_sdk_ts")]
    PrecompiledComponent
);

#[derive(Clone)]
struct AttemptCounterState {
    counter: Arc<AtomicUsize>,
    fail_count: usize,
}

async fn attempt_handler(State(state): State<AttemptCounterState>) -> axum::response::Response {
    let attempt = state.counter.fetch_add(1, Ordering::SeqCst) + 1;
    if attempt <= state.fail_count {
        axum::response::Response::builder()
            .status(500)
            .body(axum::body::Body::empty())
            .unwrap()
    } else {
        axum::response::Response::builder()
            .status(200)
            .body(axum::body::Body::empty())
            .unwrap()
    }
}

async fn start_attempt_counter_server(fail_count: usize) -> (u16, Arc<AtomicUsize>) {
    let counter = Arc::new(AtomicUsize::new(0));
    let state = AttemptCounterState {
        counter: counter.clone(),
        fail_count,
    };
    let app = Router::new()
        .route("/attempt", get(attempt_handler))
        .route("/attempt-post", post(attempt_post_handler))
        .with_state(state);
    let listener = TcpListener::bind("0.0.0.0:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(
        async move {
            axum::serve(listener, app).await.unwrap();
        }
        .in_current_span(),
    );
    (port, counter)
}

/// POST counterpart of `attempt_handler` that mirrors the user's
/// chaos-backend exactly: on the failure path it returns 500
/// **immediately, without reading the request body**, then closes the
/// response. The success path consumes the body and returns 200.
///
/// This shape matches `applyChaos` in `chaos-backend/server.ts`:
///
///   if (cfg.failureRate && Math.random() < cfg.failureRate) {
///     send(res, 500, { error: "chaos: synthetic failure" });
///     return false;       // body never consumed
///   }
///
/// Returning 500 mid-request without consuming the body is what makes the
/// host inline retry resend path go wrong — on the resent request the
/// receiving HTTP server replies with HTTP 400 (no Content-Type, empty
/// body), the guest sees a non-5xx response, throws, traps, and the
/// default trap policy gives up.
async fn attempt_post_handler(
    State(state): State<AttemptCounterState>,
    request: axum::extract::Request,
) -> axum::response::Response {
    let attempt = state.counter.fetch_add(1, Ordering::SeqCst) + 1;
    tracing::info!(attempt, "attempt-post handler received request");
    if attempt <= state.fail_count {
        // Failure path: respond 500 *without* consuming the request body.
        // Drop the request (and therefore its body) immediately.
        drop(request);
        axum::response::Response::builder()
            .status(500)
            .body(axum::body::Body::empty())
            .unwrap()
    } else {
        // Success path: drain the body, then respond 200.
        let _ = axum::body::to_bytes(request.into_body(), 64 * 1024).await;
        axum::response::Response::builder()
            .status(200)
            .body(axum::body::Body::empty())
            .unwrap()
    }
}

#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn ts_with_retry_policy_retries_on_user_land_error(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("agent_sdk_ts")] agent_sdk_ts: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);

    let overrides = TestExecutorOverrides {
        configure: Some(Arc::new(|config| {
            config.retry = RetryConfig {
                max_attempts: 10,
                min_delay: Duration::from_millis(1),
                max_delay: Duration::from_millis(10),
                multiplier: 1.0,
                max_jitter_factor: None,
            };
        })),
        ..Default::default()
    };
    let executor = start_with_overrides(deps, &context, overrides).await?;

    // Server fails the first 3 requests with HTTP 500, succeeds on the 4th.
    let (port, counter) = start_attempt_counter_server(3).await;

    let component = executor
        .component_dep(&context.default_environment_id, agent_sdk_ts)
        .store()
        .await?;

    let agent_id = agent_id!("RetryTest", "retry-test");
    executor
        .start_agent_with(&component.id, agent_id.clone(), HashMap::new(), Vec::new())
        .await?;

    let result = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "withRetryPolicyTest",
            data_value!("localhost", port as f64),
        )
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow::anyhow!("expected return value"))?;

    assert_eq!(result, Value::Bool(true));
    // The server was called at least fail_count+1 times: once per failure plus the final success.
    // With oplog-replay retries the exact count may be higher, but it must be > 3.
    assert!(counter.load(Ordering::SeqCst) > 3);

    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn ts_http_status_retry_policy_retries_matching_status(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("agent_sdk_ts")] agent_sdk_ts: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start_with_overrides(deps, &context, Default::default()).await?;

    let (port, counter) = start_attempt_counter_server(3).await;

    let component = executor
        .component_dep(&context.default_environment_id, agent_sdk_ts)
        .store()
        .await?;

    let agent_id = agent_id!("RetryTest", "status-retry-test");
    executor
        .start_agent_with(&component.id, agent_id.clone(), HashMap::new(), Vec::new())
        .await?;

    let result = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "withStatusRetryPolicyTest",
            data_value!("localhost", port as f64),
        )
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow::anyhow!("expected return value"))?;

    assert_eq!(result, Value::Bool(true));
    assert_eq!(counter.load(Ordering::SeqCst), 4);

    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn ts_invocation_events_use_method_display_name(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("agent_sdk_ts")] agent_sdk_ts: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    // Server fails the first request with HTTP 500, succeeds on the 2nd.
    let (port, _counter) = start_attempt_counter_server(1).await;

    let component = executor
        .component_dep(&context.default_environment_id, agent_sdk_ts)
        .store()
        .await?;

    let agent_id = agent_id!("RetryTest", "invocation-events-test");
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), HashMap::new(), Vec::new())
        .await?;

    let (rx, abort_capture) = executor.capture_output_with_termination(&worker_id).await?;

    let _ = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "withRetryPolicyTest",
            data_value!("localhost", port as f64),
        )
        .await?;

    // Give the broadcast channel a moment to deliver the trailing InvocationFinished event.
    tokio::time::sleep(Duration::from_millis(500)).await;

    abort_capture.send(()).unwrap();
    let events = drain_connection(rx).await;

    let invocation_started_functions: Vec<String> = events
        .iter()
        .filter_map(|e| match e {
            Some(LogEvent {
                event: Some(log_event::Event::InvocationStarted(inner)),
            }) => Some(inner.function.clone()),
            _ => None,
        })
        .collect();

    let invocation_finished_functions: Vec<String> = events
        .iter()
        .filter_map(|e| match e {
            Some(LogEvent {
                event: Some(log_event::Event::InvocationFinished(inner)),
            }) => Some(inner.function.clone()),
            _ => None,
        })
        .collect();

    tracing::info!(?invocation_started_functions, "captured InvocationStarted");
    tracing::info!(
        ?invocation_finished_functions,
        "captured InvocationFinished"
    );

    assert!(
        invocation_started_functions
            .iter()
            .any(|f| f == "withRetryPolicyTest"),
        "expected an InvocationStarted event with function == \"withRetryPolicyTest\", got {invocation_started_functions:?}"
    );
    assert!(
        invocation_finished_functions
            .iter()
            .any(|f| f == "withRetryPolicyTest"),
        "expected an InvocationFinished event with function == \"withRetryPolicyTest\", got {invocation_finished_functions:?}"
    );
    assert!(
        !invocation_started_functions
            .iter()
            .any(|f| f.contains("golem:agent/guest")),
        "no InvocationStarted should report the raw WIT function name, got {invocation_started_functions:?}"
    );
    assert!(
        !invocation_finished_functions
            .iter()
            .any(|f| f.contains("golem:agent/guest")),
        "no InvocationFinished should report the raw WIT function name, got {invocation_finished_functions:?}"
    );

    Ok(())
}

/// Builds a manifest-style status-code retry policy:
///
///   countBox(maxRetries = 1000, inner = periodic(<delay>))
///   predicate: status-code in {500, 502, 503, 504}
fn manifest_http_5xx_retry_policy(name: &str, delay: Duration) -> NamedRetryPolicy {
    NamedRetryPolicy {
        name: name.to_string(),
        priority: 20,
        predicate: Predicate::PropIn {
            property: "status-code".to_string(),
            values: vec![
                PredicateValue::Integer(500),
                PredicateValue::Integer(502),
                PredicateValue::Integer(503),
                PredicateValue::Integer(504),
            ],
        },
        policy: RetryPolicy::CountBox {
            max_retries: 1000,
            inner: Box::new(RetryPolicy::Periodic(delay)),
        },
    }
}

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
        .into_return_value()
        .ok_or_else(|| anyhow::anyhow!("expected return value"))?;

    assert_eq!(
        result,
        Value::Bool(true),
        "agent must complete successfully"
    );
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
///         HTTP/1.1 500 Internal Server Error
///         Content-Type: application/json
///         Content-Length: 38
///
///         {"error":"chaos: synthetic failure"}
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
) -> (
    u16,
    Arc<AtomicUsize>,
    Arc<AtomicUsize>,
    Arc<AtomicUsize>,
) {
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
        if let Some(rest) = lower.strip_prefix("content-length:") {
            if let Ok(n) = rest.trim().parse::<usize>() {
                return Some(n);
            }
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
    let (port, counter, conn_counter, bad_requests) =
        start_chaos_post_server(fail_count).await;

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
        .into_return_value()
        .ok_or_else(|| anyhow::anyhow!("expected return value"))?;

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

    assert_eq!(
        result,
        Value::Bool(true),
        "agent must complete successfully (host inline status-code retry must transparently re-issue the failing POST request with its JSON body)"
    );

    assert!(
        observed > fail_count,
        "server must observe at least {} POST attempts, observed {observed}",
        fail_count + 1
    );

    Ok(())
}


