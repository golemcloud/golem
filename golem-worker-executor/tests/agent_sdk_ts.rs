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
use golem_api_grpc::proto::golem::worker::{LogEvent, log_event};
use golem_common::model::retry_policy::{NamedRetryPolicy, Predicate, PredicateValue, RetryPolicy};
use golem_common::model::{AgentStatus, RetryConfig};
use golem_common::{agent_id, data_value};
use golem_test_framework::dsl::{TestDsl, drain_connection};
use golem_worker_executor_test_utils::{
    LastUniqueId, PrecompiledComponent, TestContext, TestExecutorOverrides,
    WorkerExecutorTestDependencies, start, start_with_overrides,
};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use test_r::{inherit_test_dep, test, timeout};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
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
        .into_typed::<bool>()?;

    assert!(result);
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
        .into_typed::<bool>()?;

    assert!(result);
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

// ===========================================================================
// CheckoutAgentV2 / chaos-backend reproducers
//
// The four tests below mirror the four shell scripts in the user's
// `~/projects/craft/golem/scripts/` directory:
//
//   s1-payment-failure.sh   → ts_v2_s1_payment_failure_then_reset
//   s2-shipment-hang.sh     → ts_v2_s2_shipment_hangs_then_reset
//   s3-process-crash.sh     → ts_v2_s3_process_crash_mid_workflow
//   s4-high-chaos.sh        → ts_v2_s4_sustained_70_percent_chaos
//
// They share:
//   - a `CheckoutAgentV2` agent that does four sequential POSTs
//     (`/inventory/reserve`, `/payment/charge`, `/shipment/create`,
//     `/email/send`) with 10s `AbortController` deadlines and JSON bodies,
//   - a Rust port of `chaos-backend/server.ts` (the same four routes plus
//     per-endpoint `failureRate`/`latencyMs`/`hang` chaos config that the
//     test harness can mutate at runtime), and
//   - a manifest `http-5xx-retry` policy keyed on `status-code in
//     {500,502,503,504}` matching the user's `golem.yaml`
//     `retryPolicyDefaults`.
// ===========================================================================

#[derive(Clone, Default)]
struct EndpointChaos {
    failure_rate: f64,
    latency_ms: u64,
    hang: bool,
}

#[derive(Clone, Copy)]
enum Endpoint {
    Inventory,
    Payment,
    Shipment,
    Email,
}

/// Per-endpoint *request-arrival* counters. Bumped at the very top of
/// each business handler — BEFORE chaos is applied — so they include
/// hung, failed, and successful requests alike. Used to detect the
/// tight replay/retry loops the user observed (where the chaos log
/// fills with thousands of hits even though the agent never makes
/// durable progress).
#[derive(Default)]
struct AttemptCounts {
    inventory: AtomicUsize,
    payment: AtomicUsize,
    shipment: AtomicUsize,
    email: AtomicUsize,
}

#[derive(Default)]
struct ChaosState {
    inventory: EndpointChaos,
    payment: EndpointChaos,
    shipment: EndpointChaos,
    email: EndpointChaos,
}

/// Per-endpoint count of *successful* (chaos-passed, body-consumed,
/// 200-responded) calls, mirroring the original node.js chaos-backend's
/// `log[name]` length used by the tests' assertions.
#[derive(Default)]
struct LogState {
    inventory: usize,
    payment: usize,
    shipment: usize,
    email: usize,
}

/// Inner shared state of the embedded chaos-backend. Cloned into both
/// the test-facing `ChaosBackend` handle and the axum router so config
/// mutations are visible to in-flight handler invocations.
#[derive(Clone)]
struct ChaosInner {
    attempts: Arc<AttemptCounts>,
    chaos: Arc<Mutex<ChaosState>>,
    log: Arc<Mutex<LogState>>,
}

impl ChaosInner {
    fn read_chaos(&self, ep: Endpoint) -> EndpointChaos {
        let g = self.chaos.lock().unwrap();
        match ep {
            Endpoint::Inventory => g.inventory.clone(),
            Endpoint::Payment => g.payment.clone(),
            Endpoint::Shipment => g.shipment.clone(),
            Endpoint::Email => g.email.clone(),
        }
    }

    fn bump_attempt(&self, ep: Endpoint) {
        let counter = match ep {
            Endpoint::Inventory => &self.attempts.inventory,
            Endpoint::Payment => &self.attempts.payment,
            Endpoint::Shipment => &self.attempts.shipment,
            Endpoint::Email => &self.attempts.email,
        };
        counter.fetch_add(1, Ordering::SeqCst);
    }

    fn bump_log(&self, ep: Endpoint) {
        let mut g = self.log.lock().unwrap();
        let slot = match ep {
            Endpoint::Inventory => &mut g.inventory,
            Endpoint::Payment => &mut g.payment,
            Endpoint::Shipment => &mut g.shipment,
            Endpoint::Email => &mut g.email,
        };
        *slot += 1;
    }
}

/// Embedded in-process chaos-backend handle returned by
/// `start_chaos_backend`. Holds:
///   - shared chaos config that handlers consult on every request,
///   - per-endpoint attempt counters,
///   - per-endpoint successful-call counters,
///   - an `Arc<ServerShutdown>` that aborts the axum server when the
///     last `ChaosBackend` clone is dropped (matching the kill-on-drop
///     semantics of the previous `tokio::process::Child` handle).
#[derive(Clone)]
struct ChaosBackend {
    inner: ChaosInner,
    _shutdown: Arc<ServerShutdown>,
}

struct ServerShutdown {
    abort: tokio::task::AbortHandle,
}

impl Drop for ServerShutdown {
    fn drop(&mut self) {
        self.abort.abort();
    }
}

impl ChaosBackend {
    fn attempts_snapshot(&self) -> (usize, usize, usize, usize) {
        let a = &self.inner.attempts;
        (
            a.inventory.load(Ordering::SeqCst),
            a.payment.load(Ordering::SeqCst),
            a.shipment.load(Ordering::SeqCst),
            a.email.load(Ordering::SeqCst),
        )
    }

    fn set(&self, ep: Endpoint, c: EndpointChaos) {
        let mut g = self.inner.chaos.lock().unwrap();
        match ep {
            Endpoint::Inventory => g.inventory = c,
            Endpoint::Payment => g.payment = c,
            Endpoint::Shipment => g.shipment = c,
            Endpoint::Email => g.email = c,
        }
    }

    async fn set_inventory(&self, c: EndpointChaos) {
        self.set(Endpoint::Inventory, c);
    }
    async fn set_payment(&self, c: EndpointChaos) {
        self.set(Endpoint::Payment, c);
    }
    async fn set_shipment(&self, c: EndpointChaos) {
        self.set(Endpoint::Shipment, c);
    }
    async fn set_email(&self, c: EndpointChaos) {
        self.set(Endpoint::Email, c);
    }

    /// Mirrors the node.js `/_chaos/reset` route: clears all chaos
    /// config across every endpoint. Like the original it does NOT clear
    /// the `log` counters — successful-call counts accumulate for the
    /// life of the backend.
    async fn reset(&self) {
        *self.inner.chaos.lock().unwrap() = ChaosState::default();
    }

    /// Returns `(inventory, payment, shipment, email)` *successful* call
    /// counts as observed by the chaos-backend (chaos-passed, body
    /// consumed, 200 responded). For total wire-level attempt counts
    /// (including hung/failed requests) use `attempts_snapshot`.
    async fn snapshot(&self) -> (usize, usize, usize, usize) {
        let g = self.inner.log.lock().unwrap();
        (g.inventory, g.payment, g.shipment, g.email)
    }
}

fn rand_id8() -> String {
    format!("{:08x}", rand::random::<u32>())
}

/// Per-endpoint business handler. Mirrors the node.js chaos-backend's
/// `applyChaos` semantics exactly:
///   - bump the wire-level attempt counter first (matches the previous
///     TCP-relay sniffing that incremented before the request reached
///     the server),
///   - apply latency,
///   - if `hang`, never respond and never consume the request body,
///   - if a synthetic failure fires, return 500 *without* consuming the
///     request body (matches the node.js `send(res, 500, ...); return false`
///     wire shape that the previous TCP relay was preserving),
///   - otherwise read the body, bump the success counter, return a 200
///     response shaped like the original handler.
async fn business_handler(
    state: ChaosInner,
    ep: Endpoint,
    req: axum::http::Request<axum::body::Body>,
) -> axum::response::Response {
    state.bump_attempt(ep);

    let cfg = state.read_chaos(ep);

    if cfg.latency_ms > 0 {
        tokio::time::sleep(Duration::from_millis(cfg.latency_ms)).await;
    }

    if cfg.hang {
        // Never respond — mirrors node.js handler returning false from
        // `applyChaos` and not calling `res.end()`.
        std::future::pending::<()>().await;
        unreachable!();
    }

    if cfg.failure_rate > 0.0 && rand::random::<f64>() < cfg.failure_rate {
        return chaos_failure_response();
    }

    let body_bytes = match axum::body::to_bytes(req.into_body(), usize::MAX).await {
        Ok(b) => b,
        Err(_) => return chaos_failure_response(),
    };
    let body_json: serde_json::Value = serde_json::from_slice(&body_bytes)
        .unwrap_or(serde_json::Value::Object(Default::default()));

    state.bump_log(ep);

    let order_id = body_json
        .get("orderId")
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    let id = rand_id8();
    let resp_body = match ep {
        Endpoint::Inventory => serde_json::json!({
            "reservationId": format!("res_{id}"),
            "orderId": order_id,
        }),
        Endpoint::Payment => {
            let amount = body_json
                .get("amount")
                .cloned()
                .unwrap_or(serde_json::Value::Null);
            let currency = body_json
                .get("currency")
                .cloned()
                .unwrap_or_else(|| serde_json::Value::String("USD".to_string()));
            serde_json::json!({
                "paymentId": format!("pay_{id}"),
                "orderId": order_id,
                "amount": amount,
                "currency": currency,
            })
        }
        Endpoint::Shipment => {
            let tracking = format!(
                "1Z{:012X}",
                rand::random::<u64>() & 0x0000_FFFF_FFFF_FFFFu64
            );
            serde_json::json!({
                "shipmentId": format!("shp_{id}"),
                "trackingNumber": tracking,
                "orderId": order_id,
            })
        }
        Endpoint::Email => {
            let to = body_json
                .get("to")
                .cloned()
                .unwrap_or(serde_json::Value::Null);
            serde_json::json!({
                "messageId": format!("msg_{id}"),
                "to": to,
            })
        }
    };

    axum::response::Response::builder()
        .status(200)
        .header("content-type", "application/json")
        .body(axum::body::Body::from(resp_body.to_string()))
        .unwrap()
}

fn chaos_failure_response() -> axum::response::Response {
    axum::response::Response::builder()
        .status(500)
        .header("content-type", "application/json")
        .body(axum::body::Body::from(
            r#"{"error":"chaos: synthetic failure"}"#,
        ))
        .unwrap()
}

/// Spawn an embedded in-process axum chaos-backend on an ephemeral port.
///
/// Returns `(port, ChaosBackend)`. The caller hands `port` to the agent
/// under test (so it talks to a real HTTP server at the wire level) and
/// uses the `ChaosBackend` handle to mutate chaos config (`set_*` /
/// `reset`) and read counts (`snapshot` / `attempts_snapshot`). The
/// embedded server is aborted when the last `ChaosBackend` clone is
/// dropped, mirroring the kill-on-drop semantics of the previous
/// `npx tsx server.ts` child process.
async fn start_chaos_backend() -> (u16, ChaosBackend) {
    let inner = ChaosInner {
        attempts: Arc::new(AttemptCounts::default()),
        chaos: Arc::new(Mutex::new(ChaosState::default())),
        log: Arc::new(Mutex::new(LogState::default())),
    };

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();

    let make_route = |ep: Endpoint| {
        let s = inner.clone();
        move |req: axum::http::Request<axum::body::Body>| {
            let s = s.clone();
            async move { business_handler(s, ep, req).await }
        }
    };

    let app = Router::new()
        .route("/inventory/reserve", post(make_route(Endpoint::Inventory)))
        .route("/payment/charge", post(make_route(Endpoint::Payment)))
        .route("/shipment/create", post(make_route(Endpoint::Shipment)))
        .route("/email/send", post(make_route(Endpoint::Email)));

    let handle = tokio::spawn(
        async move {
            let _ = axum::serve(listener, app).await;
        }
        .in_current_span(),
    );
    let abort = handle.abort_handle();

    tracing::info!(port, "embedded chaos-backend ready");

    let backend = ChaosBackend {
        inner,
        _shutdown: Arc::new(ServerShutdown { abort }),
    };
    (port, backend)
}

/// Common executor overrides for the four V2 scenarios:
/// `manifest-5xx-retry` policy with a 2s periodic delay (matching the
/// user's `golem.yaml`), `max_retries: 10000` (also matching the user's
/// `countBox` budget). Worker-level trap retry is left at its default
/// because the user did not customize it.
fn v2_overrides() -> TestExecutorOverrides {
    TestExecutorOverrides {
        retry_policies: Some(vec![manifest_http_5xx_retry_policy(
            "manifest-5xx-retry",
            // Matches the `periodic: "2s"` in the user's `golem.yaml`
            // `retryPolicyDefaults.local.http-5xx-retry`.
            Duration::from_secs(2),
        )]),
        ..Default::default()
    }
}

/// Args for `CheckoutAgentV2.checkout(host, port, customerEmail, amount,
/// address, sku, qty)`. Centralized so the four tests stay in lockstep.
fn checkout_args(host: &str, port: u16) -> golem_common::schema::TypedSchemaValue {
    data_value!(
        host,
        port as f64,
        "alice@example.com",
        42.0_f64,
        "1 Main St",
        "sku-1",
        1.0_f64,
    )
}

/// ---------------------------------------------------------------------------
/// Scenario 1 — 100% payment failure (then chaos reset).
///
/// Mirrors `~/projects/craft/golem/scripts/s1-payment-failure.sh`:
///   1. Set `/payment/charge` `failureRate = 1.0`.
///   2. Trigger `CheckoutAgentV2.checkout` (fire-and-forget).
///   3. Hold the failure for 5 seconds, then reset chaos.
///   4. Wait for the agent to reach `Idle`.
///   5. Assert each endpoint succeeded exactly once (final counts =
///      inventory=1, payment=1, shipment=1, email=1).
///
/// What the user observes: V2 fails on the first /payment/charge POST,
/// the host inline status-code retry only fires once, the next 500 escapes
/// to the guest which throws, the default trap-retry exhausts (3 attempts)
/// and the agent goes to `Failed` with payment=0.
/// ---------------------------------------------------------------------------
#[test]
#[tracing::instrument]
#[timeout("3m")]
async fn ts_v2_s1_payment_failure_then_reset(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("agent_sdk_ts")] agent_sdk_ts: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start_with_overrides(deps, &context, v2_overrides()).await?;

    let (port, backend) = start_chaos_backend().await;

    let component = executor
        .component_dep(&context.default_environment_id, agent_sdk_ts)
        .store()
        .await?;

    let agent_id = agent_id!("CheckoutAgentV2", "ord-s1-v2");
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), HashMap::new(), Vec::new())
        .await?;

    // 1. Configure 100% payment failure.
    backend
        .set_payment(EndpointChaos {
            failure_rate: 1.0,
            ..Default::default()
        })
        .await;

    // 2. Spawn the chaos reset on a background task so the invocation runs
    //    concurrently with the chaos manipulation, exactly like the shell
    //    script's fire-and-forget invoke + sleep + reset sequence.
    let backend_for_reset = backend.clone();
    let reset_handle = tokio::spawn(
        async move {
            tokio::time::sleep(Duration::from_secs(5)).await;
            tracing::info!("S1: 5s elapsed, resetting chaos so /payment recovers");
            backend_for_reset.reset().await;
        }
        .in_current_span(),
    );

    // 3. Trigger the checkout (we use invoke-and-await rather than
    //    fire-and-forget+poll because both end states are equivalent here:
    //    the user's polling loop just waits for the workflow to finish).
    let result = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "checkout",
            checkout_args("127.0.0.1", port),
        )
        .await?
        .into_typed::<bool>()?;

    let _ = reset_handle.await;

    let (inv, pay, ship, mail) = backend.snapshot().await;
    eprintln!("S1 final counts: inventory={inv} payment={pay} shipment={ship} email={mail}");

    assert!(
        result,
        "S1: agent must complete successfully once /payment recovers"
    );
    assert_eq!(
        inv, 1,
        "S1: inventory must succeed exactly once, observed {inv}"
    );
    assert_eq!(
        pay, 1,
        "S1: payment must succeed exactly once, observed {pay}"
    );
    assert_eq!(
        ship, 1,
        "S1: shipment must succeed exactly once, observed {ship}"
    );
    assert_eq!(
        mail, 1,
        "S1: email must succeed exactly once, observed {mail}"
    );

    let _ = worker_id;
    Ok(())
}

/// ---------------------------------------------------------------------------
/// Scenario 2 — `/shipment` hangs forever, then chaos reset.
///
/// Mirrors `~/projects/craft/golem/scripts/s2-shipment-hang.sh`:
///   1. Set `/shipment/create` `hang = true`.
///   2. Trigger `CheckoutAgentV2.checkout`.
///   3. Wait 15 seconds — long enough for the agent's 10s
///      `AbortController` deadline to fire at least once.
///   4. Reset chaos so subsequent `/shipment` calls succeed.
///   5. Wait for the agent to reach `Idle`.
///   6. Assert each endpoint succeeded exactly once and that the live
///      `/shipment` request count is bounded — the user observed
///      thousands of in-flight shipment calls in a tight replay loop.
/// ---------------------------------------------------------------------------
#[test]
#[tracing::instrument]
#[timeout("3m")]
async fn ts_v2_s2_shipment_hangs_then_reset(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("agent_sdk_ts")] agent_sdk_ts: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start_with_overrides(deps, &context, v2_overrides()).await?;

    let (port, backend) = start_chaos_backend().await;

    let component = executor
        .component_dep(&context.default_environment_id, agent_sdk_ts)
        .store()
        .await?;

    let agent_id = agent_id!("CheckoutAgentV2", "ord-s2-v2");
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), HashMap::new(), Vec::new())
        .await?;

    backend
        .set_shipment(EndpointChaos {
            hang: true,
            ..Default::default()
        })
        .await;

    let backend_for_reset = backend.clone();
    let reset_handle = tokio::spawn(
        async move {
            tokio::time::sleep(Duration::from_secs(15)).await;
            tracing::info!("S2: 15s elapsed, resetting chaos so /shipment recovers");
            backend_for_reset.reset().await;
        }
        .in_current_span(),
    );

    let result = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "checkout",
            checkout_args("127.0.0.1", port),
        )
        .await?
        .into_typed::<bool>()?;

    let _ = reset_handle.await;

    let (inv, pay, ship, mail) = backend.snapshot().await;
    let (inv_a, pay_a, ship_a, mail_a) = backend.attempts_snapshot();
    eprintln!(
        "S2 final counts: inventory={inv} payment={pay} shipment={ship} email={mail}; \
         attempts: inventory={inv_a} payment={pay_a} shipment={ship_a} email={mail_a}"
    );

    assert!(
        result,
        "S2: agent must complete successfully once /shipment recovers"
    );
    assert_eq!(
        inv, 1,
        "S2: inventory must succeed exactly once, observed {inv}"
    );
    assert_eq!(
        pay, 1,
        "S2: payment must succeed exactly once, observed {pay}"
    );
    assert_eq!(
        ship, 1,
        "S2: shipment must succeed exactly once, observed {ship}"
    );
    assert_eq!(
        mail, 1,
        "S2: email must succeed exactly once, observed {mail}"
    );

    // Hard upper bound on the *number of /shipment attempts*: the user
    // observed hundreds-to-thousands of in-flight POSTs because every
    // trap-replay re-issues the hung request. With the bug fixed, each
    // failed attempt should be charged against a small bounded retry
    // budget (default trap retry: ~5 attempts) plus the one successful
    // attempt after chaos resets — so 32 is a generous ceiling that any
    // working executor will stay well under and any tight-loop
    // regression will easily exceed.
    assert!(
        ship_a <= 32,
        "S2: /shipment attempt count must be bounded (tight retry loop bug), observed {ship_a}"
    );

    // Regression: the /shipment retries happen inside `atomically(...)`, so
    // every persisted `OplogEntry::Error.retry_from` must point at the
    // active `BeginAtomicRegion` index (BAR) — never at a per-attempt
    // `BeginRemoteWrite` index. If `retry_from` were keyed off the unstable
    // BeginRemoteWrite index, the retry budget would silently reset on
    // every replay and the named retry policy could never exhaust, which is
    // exactly the unbounded-loop bug that the now-removed
    // `lookup_retry_state_with_replay_aggregation` fallback used to mask.
    let oplog = executor
        .get_oplog(&worker_id, golem_common::model::oplog::OplogIndex::INITIAL)
        .await?;
    let begin_atomic_region_indices: std::collections::HashSet<_> = oplog
        .iter()
        .filter_map(|e| match &e.entry {
            golem_common::model::oplog::PublicOplogEntry::BeginAtomicRegion(_) => {
                Some(e.oplog_index)
            }
            _ => None,
        })
        .collect();
    // Scope-`Start` entries (e.g. the batched-write scope opened around
    // `atomically(...)`) carry no `request` payload and a synthetic
    // `<scope:batched-write>` function name, optionally with a claim-safety
    // discriminator suffix (`<scope:batched-write:...>`).
    let begin_remote_write_indices: std::collections::HashSet<_> = oplog
        .iter()
        .filter_map(|e| match &e.entry {
            golem_common::model::oplog::PublicOplogEntry::Start(params)
                if params.request.is_none()
                    && (params.function_name == "<scope:batched-write>"
                        || params.function_name.starts_with("<scope:batched-write:")) =>
            {
                Some(e.oplog_index)
            }
            _ => None,
        })
        .collect();
    let mut error_entries_checked = 0usize;
    for e in &oplog {
        if let golem_common::model::oplog::PublicOplogEntry::Error(params) = &e.entry {
            assert!(
                !begin_remote_write_indices.contains(&params.retry_from),
                "S2: Error.retry_from must NOT point to a BeginRemoteWrite index \
                 (would mean unstable per-attempt retry identity); \
                 retry_from={:?}, error={:?}",
                params.retry_from,
                params.error,
            );
            assert!(
                begin_atomic_region_indices.contains(&params.retry_from),
                "S2: Error.retry_from must point to the active atomic region's \
                 BeginAtomicRegion index; retry_from={:?}, error={:?}, BAR indices={:?}",
                params.retry_from,
                params.error,
                begin_atomic_region_indices,
            );
            error_entries_checked += 1;
        }
    }
    assert!(
        error_entries_checked > 0,
        "S2: expected at least one persisted Error entry from the shipment retries, found none"
    );

    Ok(())
}

/// ---------------------------------------------------------------------------
/// Scenario 3 — process crash mid-workflow.
///
/// Mirrors `~/projects/craft/golem/scripts/s3-process-crash.sh`:
///   1. Set `/email/send` `latencyMs = 15000` so the agent is still in the
///      middle of the email POST when we crash it.
///   2. Trigger `CheckoutAgentV2.checkout`.
///   3. Wait 5 seconds (so inventory/payment/shipment have completed).
///   4. `simulated_crash` to abruptly kill the worker.
///   5. Reset chaos so the email step does not re-wait 15 seconds on replay.
///   6. Wait for the agent to reach `Idle`.
///   7. Assert each endpoint was called live exactly once — Golem replays
///      the oplog, so /inventory, /payment, /shipment must NOT be re-issued,
///      and the user's observed "thousands of /email calls" replay loop
///      must not occur.
/// ---------------------------------------------------------------------------
#[test]
#[tracing::instrument]
#[timeout("3m")]
async fn ts_v2_s3_process_crash_mid_workflow(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("agent_sdk_ts")] agent_sdk_ts: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start_with_overrides(deps, &context, v2_overrides()).await?;

    let (port, backend) = start_chaos_backend().await;

    let component = executor
        .component_dep(&context.default_environment_id, agent_sdk_ts)
        .store()
        .await?;

    let agent_id = agent_id!("CheckoutAgentV2", "ord-s3-v2");
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), HashMap::new(), Vec::new())
        .await?;

    // 15s artificial latency on /email/send so the agent is still in the
    // middle of the email POST when we crash.
    backend
        .set_email(EndpointChaos {
            latency_ms: 15_000,
            ..Default::default()
        })
        .await;

    // Fire-and-forget: invoke the checkout, then crash the worker mid-flight.
    executor
        .invoke_agent(
            &component,
            &agent_id,
            "checkout",
            checkout_args("127.0.0.1", port),
        )
        .await?;

    tokio::time::sleep(Duration::from_secs(5)).await;

    let (inv_pre, pay_pre, ship_pre, mail_pre) = backend.snapshot().await;
    eprintln!(
        "S3 pre-crash counts: inventory={inv_pre} payment={pay_pre} shipment={ship_pre} email={mail_pre}"
    );

    tracing::info!("S3: simulating crash mid-workflow");
    executor.simulated_crash(&worker_id).await?;

    // Reset chaos so the email step does not re-wait 15 seconds on replay.
    backend.reset().await;

    // Wait for the agent to settle.
    let metadata = executor
        .wait_for_statuses(
            &worker_id,
            &[AgentStatus::Idle, AgentStatus::Failed],
            Duration::from_secs(60),
        )
        .await?;

    let (inv, pay, ship, mail) = backend.snapshot().await;
    let (inv_a, pay_a, ship_a, mail_a) = backend.attempts_snapshot();
    eprintln!(
        "S3 final counts: inventory={inv} payment={pay} shipment={ship} email={mail} \
         status={:?}; attempts: inventory={inv_a} payment={pay_a} shipment={ship_a} email={mail_a}",
        metadata.status
    );

    assert_eq!(
        metadata.status,
        AgentStatus::Idle,
        "S3: agent must reach Idle (recovered via oplog replay), got {:?}",
        metadata.status
    );
    assert_eq!(
        inv, 1,
        "S3: /inventory must NOT be re-executed after crash, observed {inv}"
    );
    assert_eq!(
        pay, 1,
        "S3: /payment must NOT be re-executed after crash, observed {pay}"
    );
    assert_eq!(
        ship, 1,
        "S3: /shipment must NOT be re-executed after crash, observed {ship}"
    );
    assert_eq!(
        mail, 1,
        "S3: /email must succeed exactly once after replay (no tight retry loop), observed {mail}"
    );

    // Hard upper bound on /email attempts: the user observed thousands
    // of in-flight POSTs to /email after a mid-call crash because every
    // replay re-issued the in-flight request. With the bug fixed, the
    // total /email attempt count must stay bounded.
    assert!(
        mail_a <= 32,
        "S3: /email attempt count must be bounded (tight retry loop bug), observed {mail_a}"
    );

    Ok(())
}

/// ---------------------------------------------------------------------------
/// Scenario 4 — sustained 70% failure rate on all four endpoints.
///
/// Mirrors `~/projects/craft/golem/scripts/s4-high-chaos.sh`:
///   1. Set `failureRate = 0.7` on every endpoint.
///   2. Trigger `CheckoutAgentV2.checkout` (chaos stays on the whole time).
///   3. Wait for the agent to reach `Idle`.
///   4. Assert each endpoint succeeded exactly once.
///
/// What the user observes: V2 typically fails on the very first /inventory
/// POST. The host status-code retry fires once, the next 500 reaches user
/// code which throws, the default trap-retry policy gives up after 3
/// attempts and the agent ends in `Failed` with all four counts at 0.
/// ---------------------------------------------------------------------------
#[test]
#[tracing::instrument]
#[timeout("3m")]
async fn ts_v2_s4_sustained_70_percent_chaos(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("agent_sdk_ts")] agent_sdk_ts: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start_with_overrides(deps, &context, v2_overrides()).await?;

    let (port, backend) = start_chaos_backend().await;

    let component = executor
        .component_dep(&context.default_environment_id, agent_sdk_ts)
        .store()
        .await?;

    let agent_id = agent_id!("CheckoutAgentV2", "ord-s4-v2");
    let _worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), HashMap::new(), Vec::new())
        .await?;

    // 70% failure rate on all four endpoints — sustained for the full
    // duration of the workflow.
    let chaos = EndpointChaos {
        failure_rate: 0.7,
        ..Default::default()
    };
    backend.set_inventory(chaos.clone()).await;
    backend.set_payment(chaos.clone()).await;
    backend.set_shipment(chaos.clone()).await;
    backend.set_email(chaos).await;

    let result = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "checkout",
            checkout_args("127.0.0.1", port),
        )
        .await?
        .into_typed::<bool>()?;

    let (inv, pay, ship, mail) = backend.snapshot().await;
    eprintln!("S4 final counts: inventory={inv} payment={pay} shipment={ship} email={mail}");

    assert!(
        result,
        "S4: agent must complete successfully despite 70% failure rate"
    );
    assert_eq!(
        inv, 1,
        "S4: inventory must succeed exactly once, observed {inv}"
    );
    assert_eq!(
        pay, 1,
        "S4: payment must succeed exactly once, observed {pay}"
    );
    assert_eq!(
        ship, 1,
        "S4: shipment must succeed exactly once, observed {ship}"
    );
    assert_eq!(
        mail, 1,
        "S4: email must succeed exactly once, observed {mail}"
    );

    Ok(())
}
