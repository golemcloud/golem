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
use axum::routing::get;
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
