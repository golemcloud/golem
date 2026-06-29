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
use golem_common::{agent_id, data_value};
use golem_test_framework::dsl::TestDsl;
use golem_worker_executor_test_utils::{
    LastUniqueId, PrecompiledComponent, TestContext, TestExecutorOverrides,
    WorkerExecutorTestDependencies, start_with_overrides,
};
use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;
use test_r::{inherit_test_dep, test};

inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(LastUniqueId);
inherit_test_dep!(
    #[tagged_as("http_tests")]
    PrecompiledComponent
);
inherit_test_dep!(Tracing);

use super::count_oplog_errors_containing;
use super::http_servers::{
    start_body_dropping_http_server, start_body_retry_then_response_retry_http_server,
    start_failing_http_server_any_method, start_partial_response_http_server,
    start_write_zeroes_validation_server,
};

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
        .start_agent_with(&component.id, agent_id.clone(), env, Vec::new())
        .await?;

    // post_large_body writes 256KB in 4 chunks. The first 2 connections will fail
    // mid-body-write, triggering output stream inline retry.
    let result = executor
        .invoke_and_await_agent(&component, &agent_id, "post_large_body", data_value!())
        .await?;

    let result_value = result.into_typed::<String>()?;

    // The response should be "200 received <N> bytes" — verify it succeeded
    assert!(
        result_value.starts_with("200 "),
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
async fn http_awaiting_response_retry_resends_full_body_after_output_stream_retry(
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
    let (port, body_lengths) = start_body_retry_then_response_retry_http_server().await;

    let component = executor
        .component_dep(&context.default_environment_id, http_tests)
        .store()
        .await?;
    let mut env = HashMap::new();
    env.insert("PORT".to_string(), port.to_string());

    let agent_id = agent_id!("HttpClient4");
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), env, Vec::new())
        .await?;

    let result = executor
        .invoke_and_await_agent(&component, &agent_id, "post_large_body", data_value!())
        .await?;
    let result_value = result.into_typed::<String>()?;

    const FULL_BODY_LEN: usize = 4 * 64 * 1024;
    assert_eq!(
        result_value,
        format!("200 received {FULL_BODY_LEN} body bytes")
    );

    {
        let body_lengths = body_lengths.lock().unwrap();
        assert!(
            body_lengths.len() >= 3,
            "Expected at least three requests, got body lengths {body_lengths:?}"
        );
        assert!(
            body_lengths[0] >= 64 * 1024,
            "First attempt must receive at least one body chunk before output-stream retry, got {body_lengths:?}"
        );
        assert_eq!(
            body_lengths[1], FULL_BODY_LEN,
            "Output-stream retry should rebuild and send the full body"
        );
        assert_eq!(
            body_lengths[2], FULL_BODY_LEN,
            "Awaiting-response retry must resend the full body, not only the suffix after the previous retry error"
        );
    }

    let retry_count =
        count_oplog_errors_containing(&executor, &worker_id, "in-function retry").await?;
    assert!(
        retry_count > 0,
        "Expected at least one in-function retry error entry in oplog, got {retry_count}"
    );

    Ok(())
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
        .start_agent_with(&component.id, agent_id.clone(), env, Vec::new())
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

    let result_value = result.into_typed::<String>()?;

    // Verify the response contains the full body (1024 bytes of sequential pattern)
    assert!(
        result_value.starts_with("200 "),
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
        .start_agent_with(&component.id, agent_id.clone(), env, Vec::new())
        .await?;

    let result = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "get_and_read_body_chunked",
            data_value!(),
        )
        .await?;

    let result_value = result.into_typed::<String>()?;

    assert!(
        result_value.starts_with("201 "),
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
        .start_agent_with(&component.id, agent_id.clone(), env, Vec::new())
        .await?;

    let result = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "post_with_write_zeroes",
            data_value!(),
        )
        .await?;

    let result_value = result.into_typed::<String>()?;

    // Server should validate the body and return "200 body-ok len=2052"
    assert_eq!(
        result_value, "200 body-ok len=2052",
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
        .start_agent_with(&component.id, agent_id.clone(), env, Vec::new())
        .await?;

    // post_with_subscribe calls subscribe() on the output stream before writing,
    // which sets output_stream_subscribed=true and disqualifies inline retry.
    let result = executor
        .invoke_and_await_agent(&component, &agent_id, "post_with_subscribe", data_value!())
        .await?;

    let result_value = result.into_typed::<String>()?;

    assert!(
        result_value.starts_with("200 "),
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
        .start_agent_with(&component.id, agent_id.clone(), env, Vec::new())
        .await?;

    // post_with_trailers finishes the outgoing body with trailers,
    // which sets has_outgoing_trailers=true and disqualifies inline retry.
    let result = executor
        .invoke_and_await_agent(&component, &agent_id, "post_with_trailers", data_value!())
        .await?;

    let result_value = result.into_typed::<String>()?;

    assert!(
        result_value.starts_with("200 "),
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
        .start_agent_with(&component.id, agent_id.clone(), env, Vec::new())
        .await?;

    // get_with_body_skip reads 256 bytes, skips 256, then reads remaining.
    // The skip sets had_body_skip=true, disqualifying response-body resumption.
    let result = executor
        .invoke_and_await_agent(&component, &agent_id, "get_with_body_skip", data_value!())
        .await?;

    let result_value = result.into_typed::<String>()?;

    // Should eventually succeed (via trap+replay, not response-body-resumption
    // inline retry)
    assert!(
        result_value.starts_with("200 "),
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
