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
    start_failing_http_server, start_failing_http_server_any_method,
    start_status_code_retry_http_server,
};

#[test]
#[tracing::instrument]
async fn http_status_retry_policy_retries_matching_status(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("http_tests")] http_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start_with_overrides(deps, &context, Default::default()).await?;

    let (port, counter, idempotency_keys) = start_status_code_retry_http_server(3).await;

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
            "post_with_status_retry_policy",
            data_value!(),
        )
        .await?;

    assert_eq!(result.into_typed::<String>()?, "200 status-retry-ok");
    assert_eq!(counter.load(Ordering::SeqCst), 4);
    {
        let idempotency_keys = idempotency_keys.lock().unwrap();
        assert_eq!(idempotency_keys.len(), 4);
        let first_key = idempotency_keys[0]
            .as_ref()
            .expect("initial HTTP request must have idempotency-key");
        assert!(
            idempotency_keys
                .iter()
                .all(|key| key.as_ref() == Some(first_key)),
            "status-code retries must preserve the original idempotency-key"
        );
    }

    executor.check_oplog_is_queryable(&worker_id).await?;

    Ok(())
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
        .start_agent_with(&component.id, agent_id.clone(), env, Vec::new())
        .await?;

    let result = executor
        .invoke_and_await_agent(&component, &agent_id, "run", data_value!())
        .await?;

    assert_eq!(
        result.into_typed::<String>()?,
        "200 response is test-header test-body"
    );

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
        .start_agent_with(&component.id, agent_id.clone(), env, Vec::new())
        .await?;

    // Call should eventually succeed via trap+replay (not inline retry)
    let result = executor
        .invoke_and_await_agent(&component, &agent_id, "run", data_value!())
        .await?;

    assert_eq!(
        result.into_typed::<String>()?,
        "200 response is test-header test-body"
    );

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
        .start_agent_with(&component.id, agent_id.clone(), env, Vec::new())
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
        .start_agent_with(&component.id, agent_id.clone(), env, Vec::new())
        .await?;

    // get_idempotent sets assume_idempotence=false and uses GET.
    // GET is inherently idempotent, so inline retry SHOULD still happen.
    let result = executor
        .invoke_and_await_agent(&component, &agent_id, "get_idempotent", data_value!())
        .await?;

    let result_value = result.into_typed::<String>()?;

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

    // Verify oplog contains in-function retry error entries (GET is idempotent)
    let retry_count =
        count_oplog_errors_containing(&executor, &worker_id, "in-function retry").await?;
    assert_eq!(
        retry_count, 2,
        "Expected 2 in-function retry error entries in oplog (GET is inherently idempotent)"
    );

    Ok(())
}
