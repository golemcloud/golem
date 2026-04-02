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
    FailingBlobStoreService, FailingKeyValueService, LastUniqueId, PrecompiledComponent,
    TestContext, TestExecutorOverrides, WorkerExecutorTestDependencies, start_with_overrides,
};
use std::sync::Arc;
use std::time::Duration;
use test_r::{inherit_test_dep, test};

inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(LastUniqueId);
inherit_test_dep!(
    #[tagged_as("host_api_tests")]
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
