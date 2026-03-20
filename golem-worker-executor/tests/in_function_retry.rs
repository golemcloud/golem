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
    FailingBlobStoreService, FailingKeyValueService, FailingRpc,
    LastUniqueId, PrecompiledComponent, TestContext, TestExecutorOverrides,
    WorkerExecutorTestDependencies,start_with_overrides,
};
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
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
/// connections. Returns `(port, connection_counter)`.
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
        wrap_rpc: Some(Arc::new(|inner| {
            Arc::new(FailingRpc::new(inner, 2))
        })),
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
                            // For POST, check Content-Length
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
                                // No Content-Length, assume no body
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

    let result_value = result
        .into_return_value()
        .expect("Expected a return value");

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
async fn http_post_not_retried_inline_when_idempotence_disabled(
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
    let worker_id = executor
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
    // It should still eventually succeed via trap+replay.
    let result = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "post_non_idempotent",
            data_value!(),
        )
        .await?;

    let result_value = result
        .into_return_value()
        .expect("Expected a return value");

    assert!(
        matches!(&result_value, Value::String(s) if s.starts_with("200 ")),
        "Expected a successful 200 response, got: {result_value:?}"
    );

    // Verify NO in-function retry error entries in oplog
    let retry_count =
        count_oplog_errors_containing(&executor, &worker_id, "in-function retry").await?;
    assert_eq!(
        retry_count, 0,
        "Expected 0 in-function retry error entries (POST should not be retried inline \
         when assume_idempotence=false)"
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

    let result_value = result
        .into_return_value()
        .expect("Expected a return value");

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
