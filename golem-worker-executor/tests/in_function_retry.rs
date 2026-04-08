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
use golem_common::model::RetryConfig;
use golem_common::model::oplog::{OplogIndex, PublicOplogEntry};
use golem_common::{agent_id, data_value};
use golem_test_framework::dsl::TestDsl;
use golem_wasm::Value;
use golem_worker_executor_test_utils::{
    FailingBlobStoreService, FailingKeyValueService, FailingRpc, LastUniqueId,
    PrecompiledComponent, TestContext, TestExecutorOverrides, WorkerExecutorTestDependencies,
    start_with_overrides,
};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use test_r::{inherit_test_dep, test};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::spawn;
use tracing::Instrument;

inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(LastUniqueId);
inherit_test_dep!(
    #[tagged_as("host_api_tests")]
    PrecompiledComponent
);
inherit_test_dep!(
    #[tagged_as("http_tests")]
    PrecompiledComponent
);
inherit_test_dep!(
    #[tagged_as("agent_rpc_rust")]
    PrecompiledComponent
);
inherit_test_dep!(Tracing);

/// Helper: count oplog Error entries whose error message contains the given substring.
async fn count_oplog_errors_containing(
    executor: &impl TestDsl,
    worker_id: &golem_common::model::AgentId,
    substring: &str,
) -> anyhow::Result<usize> {
    let oplog = executor.get_oplog(worker_id, OplogIndex::INITIAL).await?;
    Ok(oplog
        .iter()
        .filter(|e| {
            if let PublicOplogEntry::Error(params) = &e.entry {
                params.error.contains(substring)
            } else {
                false
            }
        })
        .count())
}

#[test]
#[tracing::instrument]
async fn keyvalue_get_retries_inline_on_transient_failure(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);

    let overrides = TestExecutorOverrides {
        configure: Some(Arc::new(|config| {
            config.retry = RetryConfig {
                max_attempts: 5,
                min_delay: Duration::from_millis(1),
                max_delay: Duration::from_millis(1),
                multiplier: 1.0,
                max_jitter_factor: None,
            };
            config.max_in_function_retry_delay = Duration::from_secs(1);
        })),
        wrap_key_value_service: Some(Arc::new(|inner| {
            Arc::new(FailingKeyValueService::new(inner, 2))
        })),
        ..Default::default()
    };

    let executor = start_with_overrides(deps, &context, overrides).await?;

    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;
    let agent_id = agent_id!("KeyValue", "in-function-retry-get-1");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    // Seed the key-value store (set does not fail)
    executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "set",
            data_value!(
                format!("{}-in-function-retry-get-1-bucket", component.id),
                "retry-key",
                vec![1u8, 2u8, 3u8]
            ),
        )
        .await?;

    // Get should succeed after 2 in-function retries
    let result = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "get",
            data_value!(
                format!("{}-in-function-retry-get-1-bucket", component.id),
                "retry-key"
            ),
        )
        .await?
        .into_return_value()
        .expect("Expected a return value");

    assert_eq!(
        result,
        Value::Option(Some(Box::new(Value::List(vec![
            Value::U8(1),
            Value::U8(2),
            Value::U8(3),
        ]))))
    );

    // Verify oplog contains 2 in-function retry error entries
    let retry_count =
        count_oplog_errors_containing(&executor, &worker_id, "in-function retry").await?;
    assert_eq!(
        retry_count, 2,
        "Expected 2 in-function retry error entries in oplog"
    );

    Ok(())
}

#[test]
#[tracing::instrument]
async fn keyvalue_set_retries_inline_when_idempotent(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);

    let overrides = TestExecutorOverrides {
        configure: Some(Arc::new(|config| {
            config.retry = RetryConfig {
                max_attempts: 5,
                min_delay: Duration::from_millis(1),
                max_delay: Duration::from_millis(1),
                multiplier: 1.0,
                max_jitter_factor: None,
            };
            config.max_in_function_retry_delay = Duration::from_secs(1);
        })),
        wrap_key_value_service: Some(Arc::new(|inner| {
            Arc::new(FailingKeyValueService::with_set_failures(inner, 2))
        })),
        ..Default::default()
    };

    let executor = start_with_overrides(deps, &context, overrides).await?;

    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;
    let agent_id = agent_id!("KeyValue", "in-function-retry-set-1");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    // set should succeed after 2 in-function retries (assume_idempotence defaults to true)
    executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "set",
            data_value!(
                format!("{}-in-function-retry-set-1-bucket", component.id),
                "retry-key",
                vec![10u8, 20u8, 30u8]
            ),
        )
        .await?;

    // Verify the data was actually written by reading it back
    let result = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "get",
            data_value!(
                format!("{}-in-function-retry-set-1-bucket", component.id),
                "retry-key"
            ),
        )
        .await?
        .into_return_value()
        .expect("Expected a return value");

    assert_eq!(
        result,
        Value::Option(Some(Box::new(Value::List(vec![
            Value::U8(10),
            Value::U8(20),
            Value::U8(30),
        ]))))
    );

    // Verify oplog contains 2 in-function retry error entries from the set call
    let retry_count =
        count_oplog_errors_containing(&executor, &worker_id, "in-function retry").await?;
    assert_eq!(
        retry_count, 2,
        "Expected 2 in-function retry error entries in oplog"
    );

    Ok(())
}

#[test]
#[tracing::instrument]
async fn blobstore_get_data_retries_inline_on_transient_failure(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);

    let overrides = TestExecutorOverrides {
        configure: Some(Arc::new(|config| {
            config.retry = RetryConfig {
                max_attempts: 5,
                min_delay: Duration::from_millis(1),
                max_delay: Duration::from_millis(1),
                multiplier: 1.0,
                max_jitter_factor: None,
            };
            config.max_in_function_retry_delay = Duration::from_secs(1);
        })),
        wrap_blob_store_service: Some(Arc::new(|inner| {
            Arc::new(FailingBlobStoreService::new(inner, 2))
        })),
        ..Default::default()
    };

    let executor = start_with_overrides(deps, &context, overrides).await?;

    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;
    let agent_id = agent_id!("BlobStore", "in-function-retry-get-data-1");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    let container_name = format!("{}-in-function-retry-get-data-1-container", component.id);

    // Create a container
    executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "create_container",
            data_value!(container_name.clone()),
        )
        .await?;

    // Write data to the container (write_data does not fail)
    executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "write_data",
            data_value!(
                container_name.clone(),
                "test-object",
                vec![10u8, 20u8, 30u8]
            ),
        )
        .await?;

    // Read data back — get_data will fail 2 times then succeed
    let result = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "get_data",
            data_value!(container_name.clone(), "test-object"),
        )
        .await?
        .into_return_value()
        .expect("Expected a return value");

    assert_eq!(
        result,
        Value::List(vec![Value::U8(10), Value::U8(20), Value::U8(30),])
    );

    // Verify oplog contains 2 in-function retry error entries
    let retry_count =
        count_oplog_errors_containing(&executor, &worker_id, "in-function retry").await?;
    assert_eq!(
        retry_count, 2,
        "Expected 2 in-function retry error entries in oplog"
    );

    Ok(())
}

#[test]
#[tracing::instrument]
async fn in_function_retry_falls_back_to_trap_when_delay_exceeds_threshold(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);

    let overrides = TestExecutorOverrides {
        configure: Some(Arc::new(|config| {
            config.retry = RetryConfig {
                max_attempts: 5,
                min_delay: Duration::from_millis(100),
                max_delay: Duration::from_millis(100),
                multiplier: 1.0,
                max_jitter_factor: None,
            };
            config.max_in_function_retry_delay = Duration::from_millis(1);
        })),
        wrap_key_value_service: Some(Arc::new(|inner| {
            Arc::new(FailingKeyValueService::new(inner, 1))
        })),
        ..Default::default()
    };

    let executor = start_with_overrides(deps, &context, overrides).await?;

    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;
    let agent_id = agent_id!("KeyValue", "in-function-retry-trap-fallback-1");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    // Seed data
    executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "set",
            data_value!(
                format!("{}-in-function-retry-trap-fallback-1-bucket", component.id),
                "retry-key",
                vec![7u8, 8u8, 9u8]
            ),
        )
        .await?;

    // Get should eventually succeed via trap+replay (not inline retry)
    let result = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "get",
            data_value!(
                format!("{}-in-function-retry-trap-fallback-1-bucket", component.id),
                "retry-key"
            ),
        )
        .await?
        .into_return_value()
        .expect("Expected a return value");

    assert_eq!(
        result,
        Value::Option(Some(Box::new(Value::List(vec![
            Value::U8(7),
            Value::U8(8),
            Value::U8(9),
        ]))))
    );

    // Verify NO in-function retry error entries in oplog
    // (because the retry fell back to trap+replay, not inline retry)
    let retry_count =
        count_oplog_errors_containing(&executor, &worker_id, "in-function retry").await?;
    assert_eq!(
        retry_count, 0,
        "Expected 0 in-function retry error entries in oplog (should have fallen back to trap)"
    );

    Ok(())
}

#[test]
#[tracing::instrument]
async fn in_function_retry_transitions_from_inline_to_trap_based(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);

    // Delay formula: get_delay(attempts) = min_delay * multiplier^(attempts.saturating_sub(1)),
    // capped at max_delay.
    //   get_delay(0) = 5ms, get_delay(1) = 5ms, get_delay(2) = 50ms
    //
    // Actual flow with decide_retry (total_attempts = retry_count + oplog_retry_count):
    // - failure #1: total_attempts=0, delay=5ms → inline retry (retry_count becomes 1,
    //   oplog gets 1 in-function retry error entry with retry_from=15)
    // - failure #2: total_attempts=2 (retry_count=1 + oplog_retry_count=1), delay=50ms
    //   → exceeds 20ms threshold → FallBackToTrap
    // - after trap+replay: retry_count resets to 0 but current_retry_point stays at 15,
    //   so oplog_retry_count_for(15) = 2. total_attempts=0+2=2, delay=50ms → still exceeds
    //   threshold → FallBackToTrap again (budget is shared!)
    // - after second trap+replay: same logic → FallBackToTrap once more (failure #3 consumed
    //   by this trap cycle), then success
    //
    // Result: 1 "in-function retry" oplog error entry (only the first retry was inline).
    let overrides = TestExecutorOverrides {
        configure: Some(Arc::new(|config| {
            config.retry = RetryConfig {
                max_attempts: 10,
                min_delay: Duration::from_millis(5),
                max_delay: Duration::from_millis(500),
                multiplier: 10.0,
                max_jitter_factor: None,
            };
            config.max_in_function_retry_delay = Duration::from_millis(20);
        })),
        wrap_key_value_service: Some(Arc::new(|inner| {
            Arc::new(FailingKeyValueService::new(inner, 3))
        })),
        ..Default::default()
    };

    let executor = start_with_overrides(deps, &context, overrides).await?;

    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;
    let agent_id = agent_id!("KeyValue", "in-function-retry-mixed-1");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    // Seed data
    executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "set",
            data_value!(
                format!("{}-in-function-retry-mixed-1-bucket", component.id),
                "retry-key",
                vec![4u8, 5u8, 6u8]
            ),
        )
        .await?;

    // get fails 3 times: 2 inline retries then trap+replay recovery
    let result = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "get",
            data_value!(
                format!("{}-in-function-retry-mixed-1-bucket", component.id),
                "retry-key"
            ),
        )
        .await?
        .into_return_value()
        .expect("Expected a return value");

    assert_eq!(
        result,
        Value::Option(Some(Box::new(Value::List(vec![
            Value::U8(4),
            Value::U8(5),
            Value::U8(6),
        ]))))
    );

    // Verify oplog has exactly 1 in-function retry error entry (the first inline retry).
    // The subsequent retries all exceeded the delay threshold so they went through trap+replay,
    // proving that the retry budget is shared: after replay, the oplog_retry_count from
    // previous attempts is still consulted via the stable current_retry_point.
    let retry_count =
        count_oplog_errors_containing(&executor, &worker_id, "in-function retry").await?;
    assert_eq!(
        retry_count, 1,
        "Expected 1 in-function retry error entry (only the first retry was inline; \
         subsequent retries shared the budget and exceeded the delay threshold)"
    );

    Ok(())
}

/// Starts a raw TCP server that drops the first `fail_count` connections (producing
/// ConnectionTerminated errors), then serves a valid HTTP 200 response on subsequent
/// connections.
///
/// On the success path the server reads the full HTTP request before responding.
/// This avoids a race in hyper's HTTP/1 client dispatcher where a response that
/// arrives before `send_request` registers the callback is rejected with
/// `Canceled(UnexpectedMessage)`, causing spurious extra retries on busy CI
/// machines.
///
/// Returns `(port, connection_counter)`.
async fn start_failing_http_server(fail_count: usize) -> (u16, Arc<AtomicUsize>) {
    let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let counter = Arc::new(AtomicUsize::new(0));
    let counter_clone = counter.clone();

    spawn(
        async move {
            loop {
                let (mut stream, _) = match listener.accept().await {
                    Ok(conn) => conn,
                    Err(_) => break,
                };
                let n = counter_clone.fetch_add(1, Ordering::SeqCst);
                if n < fail_count {
                    // Immediately close the connection — produces ConnectionTerminated
                    drop(stream);
                } else {
                    // Read the full HTTP request before responding to avoid a
                    // hyper dispatcher race (see doc comment above).
                    let mut data = Vec::new();
                    let mut buf = [0u8; 4096];
                    loop {
                        match stream.read(&mut buf).await {
                            Ok(0) => break,
                            Ok(n) => data.extend_from_slice(&buf[..n]),
                            Err(_) => break,
                        }
                        let data_str = String::from_utf8_lossy(&data);
                        if let Some(header_end) = data_str.find("\r\n\r\n") {
                            let headers = &data_str[..header_end];
                            if let Some(cl_line) = headers
                                .lines()
                                .find(|l| l.to_lowercase().starts_with("content-length:"))
                            {
                                let cl: usize = cl_line
                                    .split(':')
                                    .nth(1)
                                    .unwrap()
                                    .trim()
                                    .parse()
                                    .unwrap_or(0);
                                let body_start = header_end + 4;
                                if data.len() >= body_start + cl {
                                    break;
                                }
                            } else {
                                break;
                            }
                        }
                    }

                    let body = "response is test-header test-body";
                    let response = format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body,
                    );
                    let _ = stream.write_all(response.as_bytes()).await;
                    let _ = stream.shutdown().await;
                }
            }
        }
        .in_current_span(),
    );

    (port, counter)
}

#[test]
#[tracing::instrument]
async fn http_zone1_inline_retry_on_transient_connection_failure(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("http_tests")] http_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);

    let overrides = TestExecutorOverrides {
        configure: Some(Arc::new(|config| {
            config.retry = RetryConfig {
                max_attempts: 5,
                min_delay: Duration::from_millis(1),
                max_delay: Duration::from_millis(5),
                multiplier: 1.0,
                max_jitter_factor: None,
            };
            config.max_in_function_retry_delay = Duration::from_secs(1);
        })),
        ..Default::default()
    };

    let executor = start_with_overrides(deps, &context, overrides).await?;

    let (port, connection_counter) = start_failing_http_server(2).await;

    let component = executor
        .component_dep(&context.default_environment_id, http_tests)
        .store()
        .await?;
    let mut env = HashMap::new();
    env.insert("PORT".to_string(), port.to_string());

    let agent_id = agent_id!("HttpClient");
    let worker_id = executor
        .start_agent_with(
            &component.id,
            agent_id.clone(),
            env,
            HashMap::new(),
            Vec::new(),
        )
        .await?;

    let result = executor
        .invoke_and_await_agent(&component, &agent_id, "run", data_value!())
        .await?;

    assert_eq!(result, data_value!("200 response is test-header test-body"));

    // Server received 2 failed + 1 successful = 3 total connections
    let total_connections = connection_counter.load(Ordering::SeqCst);
    assert_eq!(
        total_connections, 3,
        "Expected 3 total connections (2 dropped + 1 successful)"
    );

    // Verify oplog contains 2 in-function retry error entries
    let retry_count =
        count_oplog_errors_containing(&executor, &worker_id, "in-function retry").await?;
    assert_eq!(
        retry_count, 2,
        "Expected 2 in-function retry error entries in oplog"
    );

    Ok(())
}

#[test]
#[tracing::instrument]
async fn http_zone1_falls_back_to_trap_when_delay_exceeds_threshold(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("http_tests")] http_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);

    let overrides = TestExecutorOverrides {
        configure: Some(Arc::new(|config| {
            config.retry = RetryConfig {
                max_attempts: 5,
                min_delay: Duration::from_millis(100),
                max_delay: Duration::from_millis(100),
                multiplier: 1.0,
                max_jitter_factor: None,
            };
            config.max_in_function_retry_delay = Duration::from_millis(1);
        })),
        ..Default::default()
    };

    let executor = start_with_overrides(deps, &context, overrides).await?;

    let (port, _connection_counter) = start_failing_http_server(1).await;

    let component = executor
        .component_dep(&context.default_environment_id, http_tests)
        .store()
        .await?;
    let mut env = HashMap::new();
    env.insert("PORT".to_string(), port.to_string());

    let agent_id = agent_id!("HttpClient");
    let worker_id = executor
        .start_agent_with(
            &component.id,
            agent_id.clone(),
            env,
            HashMap::new(),
            Vec::new(),
        )
        .await?;

    // Call should eventually succeed via trap+replay (not inline retry)
    let result = executor
        .invoke_and_await_agent(&component, &agent_id, "run", data_value!())
        .await?;

    assert_eq!(result, data_value!("200 response is test-header test-body"));

    // Verify NO in-function retry error entries in oplog
    let retry_count =
        count_oplog_errors_containing(&executor, &worker_id, "in-function retry").await?;
    assert_eq!(
        retry_count, 0,
        "Expected 0 in-function retry error entries in oplog (should have fallen back to trap)"
    );

    Ok(())
}

#[test]
#[tracing::instrument]
async fn async_rpc_inline_retry_on_transient_remote_error(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] agent_rpc_rust: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);

    let overrides = TestExecutorOverrides {
        configure: Some(Arc::new(|config| {
            config.retry = RetryConfig {
                max_attempts: 5,
                min_delay: Duration::from_millis(1),
                max_delay: Duration::from_millis(5),
                multiplier: 1.0,
                max_jitter_factor: None,
            };
            config.max_in_function_retry_delay = Duration::from_secs(1);
        })),
        wrap_rpc: Some(Arc::new(|inner| Arc::new(FailingRpc::new(inner, 2)))),
        ..Default::default()
    };

    let executor = start_with_overrides(deps, &context, overrides).await?;

    let component = executor
        .component_dep(&context.default_environment_id, agent_rpc_rust)
        .store()
        .await?;

    let agent_id = agent_id!("RpcCaller", "in-function-retry-rpc-1");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    // test1 creates 3 RpcCounter agents and calls inc_by/get_value via RPC.
    // The first 2 invoke_and_await calls will fail with RemoteInternalError
    // (transient), then succeed via inline retry.
    let result = executor
        .invoke_and_await_agent(&component, &agent_id, "test1", data_value!())
        .await?;

    let result_value = result
        .into_return_value()
        .expect("Expected a single return value");

    // test1 returns [(name3, 3), (name2, 3), (name1, 3)] — counter values
    // The exact values depend on how many RPC calls succeed. The important
    // thing is the call completed successfully despite transient failures.
    assert!(
        matches!(result_value, Value::List(ref items) if !items.is_empty()),
        "Expected a non-empty list result from test1, got: {result_value:?}"
    );

    // Verify oplog contains in-function retry error entries
    let retry_count =
        count_oplog_errors_containing(&executor, &worker_id, "in-function retry").await?;
    assert!(
        retry_count > 0,
        "Expected at least 1 in-function retry error entry in oplog, got {retry_count}"
    );

    Ok(())
}

/// Starts a raw TCP server for large-body POST tests. On the first `fail_count`
/// connections, accepts the connection, reads HTTP headers (allowing the client
/// to start sending the body), then drops the connection mid-stream. On subsequent
/// connections, reads the full body and responds with HTTP 200 echoing the body size.
/// Returns `(port, connection_counter)`.
async fn start_body_dropping_http_server(fail_count: usize) -> (u16, Arc<AtomicUsize>) {
    let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let counter = Arc::new(AtomicUsize::new(0));
    let counter_clone = counter.clone();

    spawn(
        async move {
            loop {
                let (mut stream, _) = match listener.accept().await {
                    Ok(conn) => conn,
                    Err(_) => break,
                };
                let n = counter_clone.fetch_add(1, Ordering::SeqCst);
                if n < fail_count {
                    // Read a small amount (HTTP headers) then drop,
                    // forcing the client's body write to fail.
                    let mut buf = [0u8; 512];
                    let _ = stream.read(&mut buf).await;
                    drop(stream);
                } else {
                    // Read the full request (headers + body), then respond
                    let mut data = Vec::new();
                    let mut buf = [0u8; 8192];
                    loop {
                        match stream.read(&mut buf).await {
                            Ok(0) => break,
                            Ok(n) => data.extend_from_slice(&buf[..n]),
                            Err(_) => break,
                        }
                        // Check if we've received the end of the HTTP body.
                        // For simplicity, look for Content-Length and verify.
                        let data_str = String::from_utf8_lossy(&data);
                        if let Some(header_end) = data_str.find("\r\n\r\n") {
                            let headers = &data_str[..header_end];
                            if let Some(cl_line) = headers
                                .lines()
                                .find(|l| l.to_lowercase().starts_with("content-length:"))
                            {
                                let cl: usize = cl_line
                                    .split(':')
                                    .nth(1)
                                    .unwrap()
                                    .trim()
                                    .parse()
                                    .unwrap_or(0);
                                let body_start = header_end + 4;
                                if data.len() >= body_start + cl {
                                    break;
                                }
                            } else if headers
                                .lines()
                                .any(|l| l.to_lowercase().contains("transfer-encoding: chunked"))
                            {
                                // Chunked request bodies end with a final zero-size chunk and a
                                // blank line. Accept optional trailers by checking for terminal
                                // "\r\n\r\n" in the body section.
                                let body_data = &data_str[header_end + 4..];
                                if body_data.ends_with("\r\n\r\n") {
                                    break;
                                }
                            }
                        }
                    }
                    let body = format!("received {} bytes", data.len());
                    let response = format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body,
                    );
                    let _ = stream.write_all(response.as_bytes()).await;
                    let _ = stream.shutdown().await;
                }
            }
        }
        .in_current_span(),
    );

    (port, counter)
}

/// Starts a raw TCP server that responds to both GET and POST requests.
/// The first `fail_count` connections are dropped immediately.
/// Subsequent connections get a valid HTTP 200 response.
/// Returns `(port, connection_counter)`.
async fn start_failing_http_server_any_method(fail_count: usize) -> (u16, Arc<AtomicUsize>) {
    let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let counter = Arc::new(AtomicUsize::new(0));
    let counter_clone = counter.clone();

    spawn(
        async move {
            loop {
                let (mut stream, _) = match listener.accept().await {
                    Ok(conn) => conn,
                    Err(_) => break,
                };
                let n = counter_clone.fetch_add(1, Ordering::SeqCst);
                if n < fail_count {
                    drop(stream);
                } else {
                    // Read the full request
                    let mut data = Vec::new();
                    let mut buf = [0u8; 4096];
                    loop {
                        match stream.read(&mut buf).await {
                            Ok(0) => break,
                            Ok(n) => data.extend_from_slice(&buf[..n]),
                            Err(_) => break,
                        }
                        let data_str = String::from_utf8_lossy(&data);
                        if let Some(header_end) = data_str.find("\r\n\r\n") {
                            let headers = &data_str[..header_end];
                            // For GET requests (no body), we can respond immediately
                            if headers.starts_with("GET ") {
                                break;
                            }
                            // For POST, check Content-Length or Transfer-Encoding
                            if let Some(cl_line) = headers
                                .lines()
                                .find(|l| l.to_lowercase().starts_with("content-length:"))
                            {
                                let cl: usize = cl_line
                                    .split(':')
                                    .nth(1)
                                    .unwrap()
                                    .trim()
                                    .parse()
                                    .unwrap_or(0);
                                let body_start = header_end + 4;
                                if data.len() >= body_start + cl {
                                    break;
                                }
                            } else if headers
                                .lines()
                                .any(|l| l.to_lowercase().contains("transfer-encoding: chunked"))
                            {
                                // Chunked encoding: a chunked message always ends
                                // with "0\r\n" (final chunk) + optional trailers
                                // + "\r\n" (blank line). So the body is complete
                                // when it ends with "\r\n\r\n" (either "0\r\n\r\n"
                                // for no trailers, or "trailer: val\r\n\r\n").
                                let body_data = &data_str[header_end + 4..];
                                if body_data.ends_with("\r\n\r\n") {
                                    break;
                                }
                            } else {
                                // No Content-Length or chunked encoding, assume no body
                                break;
                            }
                        }
                    }
                    let body = "response ok";
                    let response = format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body,
                    );
                    let _ = stream.write_all(response.as_bytes()).await;
                    let _ = stream.shutdown().await;
                }
            }
        }
        .in_current_span(),
    );

    (port, counter)
}

#[test]
#[tracing::instrument]
async fn http_output_stream_inline_retry_on_body_write_failure(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("http_tests")] http_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);

    let overrides = TestExecutorOverrides {
        configure: Some(Arc::new(|config| {
            config.retry = RetryConfig {
                max_attempts: 5,
                min_delay: Duration::from_millis(1),
                max_delay: Duration::from_millis(5),
                multiplier: 1.0,
                max_jitter_factor: None,
            };
            config.max_in_function_retry_delay = Duration::from_secs(1);
        })),
        ..Default::default()
    };

    let executor = start_with_overrides(deps, &context, overrides).await?;

    // Server that accepts connection, reads partial data, then drops — triggers body write error
    let (port, connection_counter) = start_body_dropping_http_server(2).await;

    let component = executor
        .component_dep(&context.default_environment_id, http_tests)
        .store()
        .await?;
    let mut env = HashMap::new();
    env.insert("PORT".to_string(), port.to_string());

    let agent_id = agent_id!("HttpClient4");
    let worker_id = executor
        .start_agent_with(
            &component.id,
            agent_id.clone(),
            env,
            HashMap::new(),
            Vec::new(),
        )
        .await?;

    // post_large_body writes 256KB in 4 chunks. The first 2 connections will fail
    // mid-body-write, triggering output stream inline retry.
    let result = executor
        .invoke_and_await_agent(&component, &agent_id, "post_large_body", data_value!())
        .await?;

    let result_value = result.into_return_value().expect("Expected a return value");

    // The response should be "200 received <N> bytes" — verify it succeeded
    assert!(
        matches!(&result_value, Value::String(s) if s.starts_with("200 ")),
        "Expected a successful 200 response, got: {result_value:?}"
    );

    // Server received 2 failed + 1 successful = 3 total connections
    let total_connections = connection_counter.load(Ordering::SeqCst);
    assert_eq!(
        total_connections, 3,
        "Expected 3 total connections (2 dropped + 1 successful)"
    );

    // Verify oplog contains in-function retry error entries.
    // The exact count depends on which stream operation triggers the error
    // (write or flush), but there should be at least 1 retry per failed connection.
    let retry_count =
        count_oplog_errors_containing(&executor, &worker_id, "in-function retry").await?;
    assert!(
        retry_count > 0,
        "Expected at least 1 in-function retry error entry in oplog, got {retry_count}"
    );

    Ok(())
}

#[test]
#[tracing::instrument]
async fn http_post_fails_permanently_when_idempotence_disabled(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("http_tests")] http_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);

    let overrides = TestExecutorOverrides {
        configure: Some(Arc::new(|config| {
            config.retry = RetryConfig {
                max_attempts: 5,
                min_delay: Duration::from_millis(1),
                max_delay: Duration::from_millis(5),
                multiplier: 1.0,
                max_jitter_factor: None,
            };
            config.max_in_function_retry_delay = Duration::from_secs(1);
        })),
        ..Default::default()
    };

    let executor = start_with_overrides(deps, &context, overrides).await?;

    // 1 connection will be dropped; POST with assume_idempotence=false should NOT retry inline
    let (port, _connection_counter) = start_failing_http_server_any_method(1).await;

    let component = executor
        .component_dep(&context.default_environment_id, http_tests)
        .store()
        .await?;
    let mut env = HashMap::new();
    env.insert("PORT".to_string(), port.to_string());

    let agent_id = agent_id!("HttpClient4");
    let _worker_id = executor
        .start_agent_with(
            &component.id,
            agent_id.clone(),
            env,
            HashMap::new(),
            Vec::new(),
        )
        .await?;

    // post_non_idempotent sets assume_idempotence=false and uses POST.
    // POST is not idempotent, so inline retry should NOT happen.
    // Trap+replay also cannot recover because the non-idempotent remote write
    // was not completed — replay detects this and fails permanently with
    // "Non-idempotent remote write operation was not completed, cannot retry".
    let result = executor
        .invoke_and_await_agent(&component, &agent_id, "post_non_idempotent", data_value!())
        .await;

    assert!(
        result.is_err(),
        "Expected the invocation to fail permanently, but it succeeded: {result:?}"
    );
    let err_msg = format!("{}", result.unwrap_err());
    assert!(
        err_msg.contains("cannot retry"),
        "Expected error about non-idempotent write not being retryable, got: {err_msg}"
    );

    Ok(())
}

#[test]
#[tracing::instrument]
async fn http_get_retried_inline_even_when_idempotence_disabled(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("http_tests")] http_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);

    let overrides = TestExecutorOverrides {
        configure: Some(Arc::new(|config| {
            config.retry = RetryConfig {
                max_attempts: 5,
                min_delay: Duration::from_millis(1),
                max_delay: Duration::from_millis(5),
                multiplier: 1.0,
                max_jitter_factor: None,
            };
            config.max_in_function_retry_delay = Duration::from_secs(1);
        })),
        ..Default::default()
    };

    let executor = start_with_overrides(deps, &context, overrides).await?;

    // 2 connections will be dropped; GET is inherently idempotent so inline retry should work
    let (port, connection_counter) = start_failing_http_server_any_method(2).await;

    let component = executor
        .component_dep(&context.default_environment_id, http_tests)
        .store()
        .await?;
    let mut env = HashMap::new();
    env.insert("PORT".to_string(), port.to_string());

    let agent_id = agent_id!("HttpClient4");
    let worker_id = executor
        .start_agent_with(
            &component.id,
            agent_id.clone(),
            env,
            HashMap::new(),
            Vec::new(),
        )
        .await?;

    // get_idempotent sets assume_idempotence=false and uses GET.
    // GET is inherently idempotent, so inline retry SHOULD still happen.
    let result = executor
        .invoke_and_await_agent(&component, &agent_id, "get_idempotent", data_value!())
        .await?;

    let result_value = result.into_return_value().expect("Expected a return value");

    assert!(
        matches!(&result_value, Value::String(s) if s.starts_with("200 ")),
        "Expected a successful 200 response, got: {result_value:?}"
    );

    // Server received 2 failed + 1 successful = 3 total connections
    let total_connections = connection_counter.load(Ordering::SeqCst);
    assert_eq!(
        total_connections, 3,
        "Expected 3 total connections (2 dropped + 1 successful)"
    );

    // Verify oplog contains in-function retry error entries (GET is idempotent)
    let retry_count =
        count_oplog_errors_containing(&executor, &worker_id, "in-function retry").await?;
    assert_eq!(
        retry_count, 2,
        "Expected 2 in-function retry error entries in oplog (GET is inherently idempotent)"
    );

    Ok(())
}

/// Starts a TCP server that sends partial responses, then supports Range-based resume.
/// First `fail_count` connections: sends `initial_status` headers + `prefix_len` bytes then drops.
/// Subsequent connections: if `resume_supports_range` is true and a Range header is present,
/// responds 206 with remaining bytes; otherwise responds with `resume_status` and the full body.
/// The body is `body_size` bytes of sequential values (i % 256).
/// Returns `(port, connection_counter)`.
async fn start_partial_response_http_server(
    fail_count: usize,
    prefix_len: usize,
    body_size: usize,
    initial_status: u16,
    resume_status: u16,
    resume_supports_range: bool,
) -> (u16, Arc<AtomicUsize>, Arc<AtomicUsize>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let counter = Arc::new(AtomicUsize::new(0));
    let counter_clone = counter.clone();
    let range_counter = Arc::new(AtomicUsize::new(0));
    let range_counter_clone = range_counter.clone();

    // Generate the full body (deterministic pattern)
    let full_body: Vec<u8> = (0..body_size).map(|i| (i % 256) as u8).collect();

    spawn(
        async move {
            loop {
                let (mut stream, _) = match listener.accept().await {
                    Ok(conn) => conn,
                    Err(_) => break,
                };
                let n = counter_clone.fetch_add(1, Ordering::SeqCst);
                let full_body = full_body.clone();

                if n < fail_count {
                    // Read request headers first to make failure timing deterministic.
                    let mut req_buf = [0u8; 4096];
                    let mut req_header_data = Vec::new();
                    loop {
                        match stream.read(&mut req_buf).await {
                            Ok(0) => break,
                            Ok(n) => {
                                req_header_data.extend_from_slice(&req_buf[..n]);
                                if req_header_data.windows(4).any(|w| w == b"\r\n\r\n") {
                                    break;
                                }
                            }
                            Err(_) => break,
                        }
                    }

                    // Send headers + partial body, then drop
                    let initial_reason = match initial_status {
                        200 => "OK",
                        201 => "Created",
                        _ => panic!("unsupported initial status: {initial_status}"),
                    };
                    let headers = format!(
                        "HTTP/1.1 {} {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        initial_status,
                        initial_reason,
                        body_size,
                    );
                    let _ = stream.write_all(headers.as_bytes()).await;
                    let _ = stream.write_all(&full_body[..prefix_len]).await;
                    let _ = stream.flush().await;
                    // Wait for the client to receive the partial data before dropping
                    tokio::time::sleep(Duration::from_millis(200)).await;
                    drop(stream);
                } else {
                    // Read request headers to check for Range
                    let mut buf = [0u8; 4096];
                    let mut header_data = Vec::new();
                    loop {
                        match stream.read(&mut buf).await {
                            Ok(0) => break,
                            Ok(n) => {
                                header_data.extend_from_slice(&buf[..n]);
                                if header_data.windows(4).any(|w| w == b"\r\n\r\n") {
                                    break;
                                }
                            }
                            Err(_) => break,
                        }
                    }
                    let header_str = String::from_utf8_lossy(&header_data);

                    // Parse Range header
                    let range_start = header_str.lines().find_map(|line| {
                        if line.to_lowercase().starts_with("range:") {
                            // Parse "Range: bytes=N-"
                            let val = line.split(':').nth(1)?.trim();
                            let rest = val.strip_prefix("bytes=")?;
                            let dash_pos = rest.find('-')?;
                            rest[..dash_pos].parse::<usize>().ok()
                        } else {
                            None
                        }
                    });

                    if range_start.is_some() {
                        range_counter_clone.fetch_add(1, Ordering::SeqCst);
                    }

                    if resume_supports_range && let Some(start) = range_start {
                        if start <= body_size {
                            // 206 Partial Content
                            let remaining = &full_body[start..];
                            let content_range =
                                format!("bytes {}-{}/{}", start, body_size - 1, body_size);
                            let response = format!(
                                "HTTP/1.1 206 Partial Content\r\nContent-Range: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                                content_range,
                                remaining.len(),
                            );
                            let _ = stream.write_all(response.as_bytes()).await;
                            let _ = stream.write_all(remaining).await;
                        } else {
                            // Invalid range
                            let response = "HTTP/1.1 416 Range Not Satisfiable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
                            let _ = stream.write_all(response.as_bytes()).await;
                        }
                    } else {
                        // Full body response (for response-body resumption
                        // matching-status skip path)
                        let resume_reason = match resume_status {
                            200 => "OK",
                            201 => "Created",
                            _ => panic!("unsupported resume status: {resume_status}"),
                        };
                        let response = format!(
                            "HTTP/1.1 {} {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                            resume_status,
                            resume_reason,
                            body_size,
                        );
                        let _ = stream.write_all(response.as_bytes()).await;
                        let _ = stream.write_all(&full_body).await;
                    }
                    let _ = stream.shutdown().await;
                }
            }
        }
        .in_current_span(),
    );

    (port, counter, range_counter)
}

/// Decodes an HTTP chunked transfer-encoded body into raw bytes.
fn decode_chunked_body(data: &[u8]) -> Vec<u8> {
    let mut result = Vec::new();
    let mut pos = 0;
    while pos < data.len() {
        // Find the end of the chunk size line
        let crlf = data[pos..]
            .windows(2)
            .position(|w| w == b"\r\n")
            .map(|p| pos + p);
        let crlf = match crlf {
            Some(p) => p,
            None => break,
        };
        let size_str = String::from_utf8_lossy(&data[pos..crlf]);
        let chunk_size = match usize::from_str_radix(size_str.trim(), 16) {
            Ok(s) => s,
            Err(_) => break,
        };
        if chunk_size == 0 {
            break; // Terminal chunk
        }
        let chunk_start = crlf + 2;
        let chunk_end = chunk_start + chunk_size;
        if chunk_end > data.len() {
            // Incomplete chunk — take what we have
            result.extend_from_slice(&data[chunk_start..]);
            break;
        }
        result.extend_from_slice(&data[chunk_start..chunk_end]);
        pos = chunk_end + 2; // Skip trailing \r\n after chunk data
    }
    result
}

/// Starts a TCP server for testing write_zeroes body reconstruction.
/// First `fail_count` connections: reads some data then drops (simulates body write failure).
/// Subsequent connections: reads full request body and responds with a validation summary.
/// Returns `(port, connection_counter)`.
async fn start_write_zeroes_validation_server(fail_count: usize) -> (u16, Arc<AtomicUsize>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let counter = Arc::new(AtomicUsize::new(0));
    let counter_clone = counter.clone();

    spawn(
        async move {
            loop {
                let (mut stream, _) = match listener.accept().await {
                    Ok(conn) => conn,
                    Err(_) => break,
                };
                let n = counter_clone.fetch_add(1, Ordering::SeqCst);

                if n < fail_count {
                    // Read a small amount then drop
                    let mut buf = [0u8; 512];
                    let _ = stream.read(&mut buf).await;
                    drop(stream);
                } else {
                    // Read the full request, handling both content-length and
                    // chunked transfer encoding (used by streaming bodies).
                    let mut data = Vec::new();
                    let mut buf = [0u8; 8192];
                    loop {
                        match stream.read(&mut buf).await {
                            Ok(0) => break,
                            Ok(n) => data.extend_from_slice(&buf[..n]),
                            Err(_) => break,
                        }
                        let data_str = String::from_utf8_lossy(&data);
                        if let Some(header_end) = data_str.find("\r\n\r\n") {
                            let headers = &data_str[..header_end];
                            if let Some(cl_line) = headers
                                .lines()
                                .find(|l| l.to_lowercase().starts_with("content-length:"))
                            {
                                let cl: usize = cl_line
                                    .split(':')
                                    .nth(1)
                                    .unwrap()
                                    .trim()
                                    .parse()
                                    .unwrap_or(0);
                                let body_start = header_end + 4;
                                if data.len() >= body_start + cl {
                                    break;
                                }
                            }
                            // Check for chunked transfer encoding terminator
                            if headers
                                .lines()
                                .any(|l| l.to_lowercase().contains("transfer-encoding: chunked"))
                            {
                                // Chunked encoding ends with "0\r\n\r\n"
                                if data.ends_with(b"0\r\n\r\n") {
                                    break;
                                }
                            }
                        }
                    }

                    // Extract body — decode chunked encoding if needed
                    let header_end_pos = String::from_utf8_lossy(&data)
                        .find("\r\n\r\n")
                        .map(|p| p + 4)
                        .unwrap_or(data.len());
                    let headers_str = String::from_utf8_lossy(&data[..header_end_pos]);
                    let is_chunked = headers_str
                        .to_lowercase()
                        .contains("transfer-encoding: chunked");
                    let raw_body = &data[header_end_pos..];
                    let request_body: Vec<u8> = if is_chunked {
                        decode_chunked_body(raw_body)
                    } else {
                        raw_body.to_vec()
                    };
                    let request_body = &request_body[..];

                    // Validate: "HEAD" + 1024 zeroes + 1024 * 0xAB
                    let expected_len = 4 + 1024 + 1024;
                    let valid = request_body.len() == expected_len
                        && &request_body[..4] == b"HEAD"
                        && request_body[4..4 + 1024].iter().all(|&b| b == 0)
                        && request_body[4 + 1024..].iter().all(|&b| b == 0xAB);

                    let body = if valid {
                        format!("body-ok len={}", request_body.len())
                    } else {
                        format!("body-bad len={}", request_body.len())
                    };

                    let response = format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body,
                    );
                    let _ = stream.write_all(response.as_bytes()).await;
                    let _ = stream.shutdown().await;
                }
            }
        }
        .in_current_span(),
    );

    (port, counter)
}

#[test]
#[tracing::instrument]
async fn http_resuming_response_body_inline_retry_on_body_read_failure(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("http_tests")] http_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);

    let overrides = TestExecutorOverrides {
        configure: Some(Arc::new(|config| {
            config.retry = RetryConfig {
                max_attempts: 5,
                min_delay: Duration::from_millis(1),
                max_delay: Duration::from_millis(5),
                multiplier: 1.0,
                max_jitter_factor: None,
            };
            config.max_in_function_retry_delay = Duration::from_secs(1);
        })),
        ..Default::default()
    };

    let executor = start_with_overrides(deps, &context, overrides).await?;

    // Server sends 1024-byte body. First connection sends 256 bytes then drops.
    // Second connection checks for Range header and responds with 206 + remaining bytes.
    let (port, connection_counter, range_counter) =
        start_partial_response_http_server(1, 256, 1024, 200, 200, true).await;

    let component = executor
        .component_dep(&context.default_environment_id, http_tests)
        .store()
        .await?;
    let mut env = HashMap::new();
    env.insert("PORT".to_string(), port.to_string());

    let agent_id = agent_id!("HttpClient4");
    let worker_id = executor
        .start_agent_with(
            &component.id,
            agent_id.clone(),
            env,
            HashMap::new(),
            Vec::new(),
        )
        .await?;

    // get_and_read_body_chunked reads in 256-byte chunks, triggering
    // response-body resumption on the partial response drop.
    let result = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "get_and_read_body_chunked",
            data_value!(),
        )
        .await?;

    let result_value = result.into_return_value().expect("Expected a return value");

    // Verify the response contains the full body (1024 bytes of sequential pattern)
    assert!(
        matches!(&result_value, Value::String(s) if s.starts_with("200 ")),
        "Expected a successful 200 response, got: {result_value:?}"
    );

    // Server received 1 partial + 1 successful = 2 total connections
    let total_connections = connection_counter.load(Ordering::SeqCst);
    assert_eq!(
        total_connections, 2,
        "Expected 2 total connections (1 partial + 1 resumed)"
    );

    let range_requests = range_counter.load(Ordering::SeqCst);
    assert!(
        range_requests > 0,
        "Expected at least 1 range request from response-body resumption retry, got {range_requests}"
    );

    // Verify oplog contains in-function retry error entries for
    // response-body resumption.
    let retry_count =
        count_oplog_errors_containing(&executor, &worker_id, "in-function retry").await?;
    assert!(
        retry_count > 0,
        "Expected at least 1 in-function retry error entry in oplog for response-body resumption, got {retry_count}"
    );

    Ok(())
}

#[test]
#[tracing::instrument]
async fn http_resuming_response_body_inline_retry_accepts_matching_non_partial_success_status(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("http_tests")] http_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);

    let overrides = TestExecutorOverrides {
        configure: Some(Arc::new(|config| {
            config.retry = RetryConfig {
                max_attempts: 5,
                min_delay: Duration::from_millis(1),
                max_delay: Duration::from_millis(5),
                multiplier: 1.0,
                max_jitter_factor: None,
            };
            config.max_in_function_retry_delay = Duration::from_secs(1);
        })),
        ..Default::default()
    };

    let executor = start_with_overrides(deps, &context, overrides).await?;

    // Server sends a 201 response body, drops mid-stream, then ignores the retry Range
    // header and resends the full body with the same 201 status.
    let (port, connection_counter, range_counter) =
        start_partial_response_http_server(1, 256, 1024, 201, 201, false).await;

    let component = executor
        .component_dep(&context.default_environment_id, http_tests)
        .store()
        .await?;
    let mut env = HashMap::new();
    env.insert("PORT".to_string(), port.to_string());

    let agent_id = agent_id!("HttpClient4");
    let worker_id = executor
        .start_agent_with(
            &component.id,
            agent_id.clone(),
            env,
            HashMap::new(),
            Vec::new(),
        )
        .await?;

    let result = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "get_and_read_body_chunked",
            data_value!(),
        )
        .await?;

    let result_value = result.into_return_value().expect("Expected a return value");

    assert!(
        matches!(&result_value, Value::String(s) if s.starts_with("201 ")),
        "Expected a successful 201 response, got: {result_value:?}"
    );

    let total_connections = connection_counter.load(Ordering::SeqCst);
    assert_eq!(
        total_connections, 2,
        "Expected 2 total connections (1 partial + 1 resumed)"
    );

    let range_requests = range_counter.load(Ordering::SeqCst);
    assert!(
        range_requests > 0,
        "Expected at least 1 range request from response-body resumption retry, got {range_requests}"
    );

    let retry_count =
        count_oplog_errors_containing(&executor, &worker_id, "in-function retry").await?;
    assert!(
        retry_count > 0,
        "Expected at least 1 in-function retry error entry in oplog for response-body resumption, got {retry_count}"
    );

    Ok(())
}

#[test]
#[tracing::instrument]
async fn http_write_zeroes_body_reconstruction(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("http_tests")] http_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);

    let overrides = TestExecutorOverrides {
        configure: Some(Arc::new(|config| {
            config.retry = RetryConfig {
                max_attempts: 5,
                min_delay: Duration::from_millis(1),
                max_delay: Duration::from_millis(5),
                multiplier: 1.0,
                max_jitter_factor: None,
            };
            config.max_in_function_retry_delay = Duration::from_secs(1);
        })),
        ..Default::default()
    };

    let executor = start_with_overrides(deps, &context, overrides).await?;

    // Server validates the body contains HEAD + zeroes + 0xAB bytes.
    // First connection drops after reading partial data.
    let (port, connection_counter) = start_write_zeroes_validation_server(1).await;

    let component = executor
        .component_dep(&context.default_environment_id, http_tests)
        .store()
        .await?;
    let mut env = HashMap::new();
    env.insert("PORT".to_string(), port.to_string());

    let agent_id = agent_id!("HttpClient4");
    let worker_id = executor
        .start_agent_with(
            &component.id,
            agent_id.clone(),
            env,
            HashMap::new(),
            Vec::new(),
        )
        .await?;

    let result = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "post_with_write_zeroes",
            data_value!(),
        )
        .await?;

    let result_value = result.into_return_value().expect("Expected a return value");

    // Server should validate the body and return "200 body-ok len=2052"
    assert_eq!(
        result_value,
        Value::String("200 body-ok len=2052".to_string()),
        "Expected server to validate reconstructed body (HEAD + 1024 zeroes + 1024 * 0xAB)"
    );

    // Server received 1 failed + 1 successful = 2 total connections
    let total_connections = connection_counter.load(Ordering::SeqCst);
    assert_eq!(
        total_connections, 2,
        "Expected 2 total connections (1 dropped + 1 successful)"
    );

    // Verify oplog contains in-function retry error entries
    let retry_count =
        count_oplog_errors_containing(&executor, &worker_id, "in-function retry").await?;
    assert!(
        retry_count > 0,
        "Expected at least 1 in-function retry error entry in oplog, got {retry_count}"
    );

    Ok(())
}

#[test]
#[tracing::instrument]
async fn http_no_output_stream_retry_when_subscribe_used(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("http_tests")] http_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);

    let overrides = TestExecutorOverrides {
        configure: Some(Arc::new(|config| {
            config.retry = RetryConfig {
                max_attempts: 5,
                min_delay: Duration::from_millis(1),
                max_delay: Duration::from_millis(5),
                multiplier: 1.0,
                max_jitter_factor: None,
            };
            config.max_in_function_retry_delay = Duration::from_secs(1);
        })),
        ..Default::default()
    };

    let executor = start_with_overrides(deps, &context, overrides).await?;

    // Server drops first connection (triggers body write failure)
    let (port, connection_counter) = start_body_dropping_http_server(1).await;

    let component = executor
        .component_dep(&context.default_environment_id, http_tests)
        .store()
        .await?;
    let mut env = HashMap::new();
    env.insert("PORT".to_string(), port.to_string());

    let agent_id = agent_id!("HttpClient4");
    let worker_id = executor
        .start_agent_with(
            &component.id,
            agent_id.clone(),
            env,
            HashMap::new(),
            Vec::new(),
        )
        .await?;

    // post_with_subscribe calls subscribe() on the output stream before writing,
    // which sets output_stream_subscribed=true and disqualifies inline retry.
    let result = executor
        .invoke_and_await_agent(&component, &agent_id, "post_with_subscribe", data_value!())
        .await?;

    let result_value = result.into_return_value().expect("Expected a return value");

    assert!(
        matches!(&result_value, Value::String(s) if s.starts_with("200 ")),
        "Expected eventual success via trap+replay, got: {result_value:?}"
    );

    // Verify the server saw at least 2 connections (1 failed + 1 replay success)
    let total_connections = connection_counter.load(Ordering::SeqCst);
    assert!(
        total_connections >= 2,
        "Expected at least 2 connections (1 dropped + 1 replay), got {total_connections}"
    );

    // Verify NO in-function retry error entries in oplog
    // (inline retry should have been disqualified by output_stream_subscribed)
    let retry_count =
        count_oplog_errors_containing(&executor, &worker_id, "in-function retry").await?;
    assert_eq!(
        retry_count, 0,
        "Expected 0 in-function retry error entries (subscribe disqualifies inline retry)"
    );

    Ok(())
}

#[test]
#[tracing::instrument]
async fn http_no_retry_when_trailers_present(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("http_tests")] http_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);

    let overrides = TestExecutorOverrides {
        configure: Some(Arc::new(|config| {
            config.retry = RetryConfig {
                max_attempts: 5,
                min_delay: Duration::from_millis(1),
                max_delay: Duration::from_millis(5),
                multiplier: 1.0,
                max_jitter_factor: None,
            };
            config.max_in_function_retry_delay = Duration::from_secs(1);
        })),
        ..Default::default()
    };

    let executor = start_with_overrides(deps, &context, overrides).await?;

    // Drop first connection — POST with trailers should NOT trigger inline retry
    let (port, connection_counter) = start_failing_http_server_any_method(1).await;

    let component = executor
        .component_dep(&context.default_environment_id, http_tests)
        .store()
        .await?;
    let mut env = HashMap::new();
    env.insert("PORT".to_string(), port.to_string());

    let agent_id = agent_id!("HttpClient4");
    let worker_id = executor
        .start_agent_with(
            &component.id,
            agent_id.clone(),
            env,
            HashMap::new(),
            Vec::new(),
        )
        .await?;

    // post_with_trailers finishes the outgoing body with trailers,
    // which sets has_outgoing_trailers=true and disqualifies inline retry.
    let result = executor
        .invoke_and_await_agent(&component, &agent_id, "post_with_trailers", data_value!())
        .await?;

    let result_value = result.into_return_value().expect("Expected a return value");

    assert!(
        matches!(&result_value, Value::String(s) if s.starts_with("200 ")),
        "Expected eventual success via trap+replay, got: {result_value:?}"
    );

    // Verify the server saw at least 2 connections (1 failed + 1 replay success)
    let total_connections = connection_counter.load(Ordering::SeqCst);
    assert!(
        total_connections >= 2,
        "Expected at least 2 connections (1 dropped + 1 replay), got {total_connections}"
    );

    // Verify NO in-function retry error entries in oplog
    let retry_count =
        count_oplog_errors_containing(&executor, &worker_id, "in-function retry").await?;
    assert_eq!(
        retry_count, 0,
        "Expected 0 in-function retry error entries (trailers disqualify inline retry)"
    );

    Ok(())
}

#[test]
#[tracing::instrument]
async fn http_no_resuming_response_body_retry_when_body_skip_used(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("http_tests")] http_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);

    let overrides = TestExecutorOverrides {
        configure: Some(Arc::new(|config| {
            config.retry = RetryConfig {
                max_attempts: 5,
                min_delay: Duration::from_millis(1),
                max_delay: Duration::from_millis(5),
                multiplier: 1.0,
                max_jitter_factor: None,
            };
            config.max_in_function_retry_delay = Duration::from_secs(1);
        })),
        ..Default::default()
    };

    let executor = start_with_overrides(deps, &context, overrides).await?;

    // Server sends 2048-byte body. First connection sends 1024 bytes then drops.
    // The guest reads first 256, skips 256, then tries to read more — which will fail.
    // Response-body resumption should be disqualified because blocking_skip was
    // used.
    let (port, connection_counter, range_counter) =
        start_partial_response_http_server(1, 1024, 2048, 200, 200, true).await;

    let component = executor
        .component_dep(&context.default_environment_id, http_tests)
        .store()
        .await?;
    let mut env = HashMap::new();
    env.insert("PORT".to_string(), port.to_string());

    let agent_id = agent_id!("HttpClient4");
    let _worker_id = executor
        .start_agent_with(
            &component.id,
            agent_id.clone(),
            env,
            HashMap::new(),
            Vec::new(),
        )
        .await?;

    // get_with_body_skip reads 256 bytes, skips 256, then reads remaining.
    // The skip sets had_body_skip=true, disqualifying response-body resumption.
    let result = executor
        .invoke_and_await_agent(&component, &agent_id, "get_with_body_skip", data_value!())
        .await?;

    let result_value = result.into_return_value().expect("Expected a return value");

    // Should eventually succeed (via trap+replay, not response-body-resumption
    // inline retry)
    assert!(
        matches!(&result_value, Value::String(s) if s.starts_with("200 ")),
        "Expected eventual success, got: {result_value:?}"
    );

    // Verify the server saw at least 2 connections (1 partial + 1 replay success)
    let total_connections = connection_counter.load(Ordering::SeqCst);
    assert!(
        total_connections >= 2,
        "Expected at least 2 connections (1 partial + 1 replay), got {total_connections}"
    );

    // Response-body resumption should be disqualified by had_body_skip, so the
    // recovery request must not use a Range header.
    let range_requests = range_counter.load(Ordering::SeqCst);
    assert_eq!(
        range_requests, 0,
        "Expected 0 range requests (body skip must disqualify response-body resumption), got {range_requests}"
    );

    Ok(())
}
