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
use golem_common::model::{
    AgentStatus, NamedRetryPolicy, Predicate, PredicateValue, RetryConfig, RetryPolicy,
};
use golem_common::{agent_id, data_value, phantom_agent_id};
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
use tracing::Instrument;

inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(LastUniqueId);
inherit_test_dep!(Tracing);
inherit_test_dep!(
    #[tagged_as("http_tests")]
    PrecompiledComponent
);

async fn start_always_failing_http_server() -> u16 {
    let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();

    tokio::spawn(
        async move {
            loop {
                let (stream, _) = match listener.accept().await {
                    Ok(conn) => conn,
                    Err(_) => break,
                };
                drop(stream);
            }
        }
        .in_current_span(),
    );

    port
}

fn time_box_retry_overrides() -> TestExecutorOverrides {
    TestExecutorOverrides {
        configure: Some(Arc::new(|config| {
            config.max_in_function_retry_delay = Duration::from_millis(1);
        })),
        retry_policies: Some(vec![NamedRetryPolicy {
            name: "time-box-recovery".to_string(),
            priority: 100,
            predicate: Predicate::True,
            policy: RetryPolicy::TimeBox {
                limit: Duration::from_millis(500),
                inner: Box::new(RetryPolicy::Periodic(Duration::from_secs(2))),
            },
        }]),
        ..Default::default()
    }
}

fn streaming_time_box_retry_overrides(path: &str) -> TestExecutorOverrides {
    TestExecutorOverrides {
        configure: Some(Arc::new(|config| {
            config.max_in_function_retry_delay = Duration::from_millis(1);
        })),
        retry_policies: Some(vec![
            NamedRetryPolicy {
                name: "streaming-time-box".to_string(),
                priority: 100,
                predicate: Predicate::And(
                    Box::new(Predicate::PropEq {
                        property: "verb".to_string(),
                        value: PredicateValue::Text("GET".to_string()),
                    }),
                    Box::new(Predicate::PropEq {
                        property: "uri-path".to_string(),
                        value: PredicateValue::Text(path.to_string()),
                    }),
                ),
                policy: RetryPolicy::TimeBox {
                    limit: Duration::from_millis(500),
                    inner: Box::new(RetryPolicy::Periodic(Duration::from_secs(2))),
                },
            },
            NamedRetryPolicy {
                name: "catch-all-must-not-match".to_string(),
                priority: 0,
                predicate: Predicate::True,
                policy: RetryPolicy::Never,
            },
        ]),
        ..Default::default()
    }
}

#[derive(Clone, Copy)]
enum StreamingFailure {
    BeforeResponse,
    TruncatedResponse,
}

async fn start_streaming_failure_server(failure: StreamingFailure) -> (u16, Arc<AtomicUsize>) {
    let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let requests = Arc::new(AtomicUsize::new(0));
    let requests_in_server = requests.clone();

    tokio::spawn(
        async move {
            loop {
                let (mut stream, _) = match listener.accept().await {
                    Ok(connection) => connection,
                    Err(_) => break,
                };
                requests_in_server.fetch_add(1, Ordering::SeqCst);
                let truncated = matches!(failure, StreamingFailure::TruncatedResponse);
                tokio::spawn(async move {
                    let mut request = Vec::new();
                    let mut buffer = [0u8; 1024];
                    while !request.windows(4).any(|window| window == b"\r\n\r\n") {
                        let read = stream.read(&mut buffer).await.unwrap_or(0);
                        if read == 0 {
                            return;
                        }
                        request.extend_from_slice(&buffer[..read]);
                    }
                    if truncated {
                        let _ = stream
                            .write_all(
                                b"HTTP/1.1 200 OK\r\ncontent-length: 1024\r\n\
                                  connection: close\r\n\r\npartial",
                            )
                            .await;
                        let _ = stream.flush().await;
                    }
                });
            }
        }
        .in_current_span(),
    );

    (port, requests)
}

async fn assert_streaming_time_box_handoff(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    http_tests: &PrecompiledComponent,
    agent_type: &str,
    method: &'static str,
    path: &str,
    failure: StreamingFailure,
    expect_invocation_error: bool,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor =
        start_with_overrides(deps, &context, streaming_time_box_retry_overrides(path)).await?;
    let (port, requests) = start_streaming_failure_server(failure).await;
    let component = executor
        .component_dep(&context.default_environment_id, http_tests)
        .store()
        .await?;
    let mut env = HashMap::new();
    env.insert("PORT".to_string(), port.to_string());
    let agent_id = agent_id!(agent_type);
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), env, Vec::new())
        .await?;

    let executor_clone = executor.clone();
    let component_clone = component.clone();
    let agent_id_clone = agent_id.clone();
    let invocation = tokio::spawn(
        async move {
            executor_clone
                .invoke_and_await_agent(&component_clone, &agent_id_clone, method, data_value!())
                .await
        }
        .in_current_span(),
    );

    executor
        .wait_for_status(&worker_id, AgentStatus::Retrying, Duration::from_secs(20))
        .await?;
    tokio::time::sleep(Duration::from_millis(750)).await;
    executor.simulated_crash(&worker_id).await?;

    let result = tokio::time::timeout(Duration::from_millis(1_500), invocation).await??;
    assert_eq!(
        result.is_err(),
        expect_invocation_error,
        "unexpected invocation result after the retry policy was exhausted: {result:?}"
    );
    assert!(
        requests.load(Ordering::SeqCst) >= 2,
        "trap recovery must reissue the failed HTTP request"
    );
    executor.check_oplog_is_queryable(&worker_id).await?;
    Ok(())
}

fn delayed_recovery_retry_overrides() -> TestExecutorOverrides {
    TestExecutorOverrides {
        configure: Some(Arc::new(|config| {
            config.retry = RetryConfig {
                max_attempts: 100,
                min_delay: Duration::from_secs(30),
                max_delay: Duration::from_secs(30),
                multiplier: 1.0,
                max_jitter_factor: None,
            };
            config.max_in_function_retry_delay = Duration::from_millis(1);
        })),
        ..Default::default()
    }
}

#[test]
#[tracing::instrument]
#[timeout("90s")]
async fn interrupt_worker_during_delayed_recovery_retry(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("http_tests")] http_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start_with_overrides(deps, &context, delayed_recovery_retry_overrides()).await?;
    let port = start_always_failing_http_server().await;

    let component = executor
        .component_dep(&context.default_environment_id, http_tests)
        .store()
        .await?;
    let mut env = HashMap::new();
    env.insert("PORT".to_string(), port.to_string());

    let agent_id = phantom_agent_id!("HttpClient", uuid::Uuid::new_v4());
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), env, Vec::new())
        .await?;

    let executor_clone = executor.clone();
    let component_clone = component.clone();
    let agent_id_clone = agent_id.clone();
    let invocation = tokio::spawn(
        async move {
            executor_clone
                .invoke_and_await_agent(&component_clone, &agent_id_clone, "run", data_value!())
                .await
        }
        .in_current_span(),
    );

    executor
        .wait_for_status(&worker_id, AgentStatus::Retrying, Duration::from_secs(20))
        .await?;

    tokio::time::timeout(Duration::from_secs(5), executor.interrupt(&worker_id)).await??;

    executor
        .wait_for_status(&worker_id, AgentStatus::Interrupted, Duration::from_secs(5))
        .await?;

    let result = tokio::time::timeout(Duration::from_secs(5), invocation).await??;
    assert!(result.is_err());
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("Interrupted via the Golem API")
    );

    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("90s")]
async fn time_box_elapsed_budget_survives_reconstruction(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("http_tests")] http_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start_with_overrides(deps, &context, time_box_retry_overrides()).await?;
    let port = start_always_failing_http_server().await;
    let component = executor
        .component_dep(&context.default_environment_id, http_tests)
        .store()
        .await?;
    let mut env = HashMap::new();
    env.insert("PORT".to_string(), port.to_string());
    let agent_id = phantom_agent_id!("HttpClient", uuid::Uuid::new_v4());
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), env, Vec::new())
        .await?;

    let executor_clone = executor.clone();
    let component_clone = component.clone();
    let agent_id_clone = agent_id.clone();
    let invocation = tokio::spawn(
        async move {
            executor_clone
                .invoke_and_await_agent(&component_clone, &agent_id_clone, "run", data_value!())
                .await
        }
        .in_current_span(),
    );

    executor
        .wait_for_status(&worker_id, AgentStatus::Retrying, Duration::from_secs(20))
        .await?;
    tokio::time::sleep(Duration::from_millis(750)).await;
    executor.simulated_crash(&worker_id).await?;

    // A reset budget would authorize another attempt after the configured two-second delay.
    // Completing sooner proves reconstruction gave up against the original elapsed budget.
    let result = tokio::time::timeout(Duration::from_millis(1_500), invocation).await??;
    assert!(
        result.is_err(),
        "the failing invocation must exhaust its time box"
    );

    executor.check_oplog_is_queryable(&worker_id).await?;

    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("90s")]
async fn response_body_fallback_preserves_time_box_override(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("http_tests")] http_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    assert_streaming_time_box_handoff(
        last_unique_id,
        deps,
        http_tests,
        "HttpClient2",
        "slow_body_stream",
        "/big-byte-array",
        StreamingFailure::TruncatedResponse,
        false,
    )
    .await
}

#[test]
#[tracing::instrument]
#[timeout("90s")]
async fn p3_transport_fallback_preserves_time_box_override(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("http_tests")] http_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    assert_streaming_time_box_handoff(
        last_unique_id,
        deps,
        http_tests,
        "HttpClient4",
        "get_and_store_full_response",
        "/full-response",
        StreamingFailure::BeforeResponse,
        true,
    )
    .await
}

#[test]
#[tracing::instrument]
#[timeout("90s")]
async fn delete_worker_during_delayed_recovery_retry(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("http_tests")] http_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start_with_overrides(deps, &context, delayed_recovery_retry_overrides()).await?;
    let port = start_always_failing_http_server().await;

    let component = executor
        .component_dep(&context.default_environment_id, http_tests)
        .store()
        .await?;
    let mut env = HashMap::new();
    env.insert("PORT".to_string(), port.to_string());

    let agent_id = phantom_agent_id!("HttpClient", uuid::Uuid::new_v4());
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), env, Vec::new())
        .await?;

    let executor_clone = executor.clone();
    let component_clone = component.clone();
    let agent_id_clone = agent_id.clone();
    let invocation = tokio::spawn(
        async move {
            executor_clone
                .invoke_and_await_agent(&component_clone, &agent_id_clone, "run", data_value!())
                .await
        }
        .in_current_span(),
    );

    executor
        .wait_for_status(&worker_id, AgentStatus::Retrying, Duration::from_secs(20))
        .await?;

    tokio::time::timeout(Duration::from_secs(5), executor.delete_worker(&worker_id)).await??;

    let metadata = executor.get_worker_metadata(&worker_id).await;
    assert!(metadata.is_err());

    let _ = tokio::time::timeout(Duration::from_secs(5), invocation).await??;

    Ok(())
}
