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
use golem_common::model::oplog::{OplogIndex, PublicOplogEntry};
use golem_common::{agent_id, data_value};
use golem_test_framework::dsl::TestDsl;
use golem_wasm::Value;
use golem_worker_executor_test_utils::{
    start, LastUniqueId, PrecompiledComponent, TestContext, WorkerExecutorTestDependencies,
};
use test_r::{inherit_test_dep, test, timeout};

inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(LastUniqueId);
inherit_test_dep!(
    #[tagged_as("host_api_tests")]
    PrecompiledComponent
);
inherit_test_dep!(Tracing);

#[test]
#[tracing::instrument]
async fn set_retry_policy_is_persisted_to_oplog(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;
    let parsed_agent_id = agent_id!("GolemHostApi", "set-retry-oplog");
    let agent_id = executor
        .start_agent(&component.id, parsed_agent_id.clone())
        .await?;

    executor
        .invoke_and_await_agent(
            &component,
            &parsed_agent_id,
            "set_simple_count_retry_policy",
            data_value!("test-policy".to_string(), 10u32, 5u32),
        )
        .await?;

    let result = executor
        .invoke_and_await_agent(
            &component,
            &parsed_agent_id,
            "has_retry_policy",
            data_value!("test-policy".to_string()),
        )
        .await?;
    let has_it = result.into_return_value().unwrap();
    assert_eq!(has_it, Value::Bool(true));

    let oplog = executor.get_oplog(&agent_id, OplogIndex::INITIAL).await?;
    let has_set = oplog
        .iter()
        .any(|e| matches!(&e.entry, PublicOplogEntry::SetRetryPolicy(_)));
    assert!(has_set, "Expected SetRetryPolicy oplog entry");

    let result = executor
        .invoke_and_await_agent(
            &component,
            &parsed_agent_id,
            "get_retry_policy_count",
            data_value!(),
        )
        .await?;
    let count = result.into_return_value().unwrap();
    match &count {
        Value::U64(n) => assert!(*n >= 1, "Expected at least 1 retry policy, got {n}"),
        other => panic!("Expected U64 return value, got {other:?}"),
    }

    executor.check_oplog_is_queryable(&agent_id).await?;

    drop(executor);
    Ok(())
}

#[test]
#[tracing::instrument]
async fn remove_retry_policy_is_persisted_to_oplog(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;
    let parsed_agent_id = agent_id!("GolemHostApi", "remove-retry-oplog");
    let agent_id = executor
        .start_agent(&component.id, parsed_agent_id.clone())
        .await?;

    executor
        .invoke_and_await_agent(
            &component,
            &parsed_agent_id,
            "set_simple_count_retry_policy",
            data_value!("removable".to_string(), 5u32, 3u32),
        )
        .await?;

    let result = executor
        .invoke_and_await_agent(
            &component,
            &parsed_agent_id,
            "has_retry_policy",
            data_value!("removable".to_string()),
        )
        .await?;
    assert_eq!(result.into_return_value().unwrap(), Value::Bool(true));

    executor
        .invoke_and_await_agent(
            &component,
            &parsed_agent_id,
            "remove_named_retry_policy",
            data_value!("removable".to_string()),
        )
        .await?;

    let result = executor
        .invoke_and_await_agent(
            &component,
            &parsed_agent_id,
            "has_retry_policy",
            data_value!("removable".to_string()),
        )
        .await?;
    assert_eq!(result.into_return_value().unwrap(), Value::Bool(false));

    let oplog = executor.get_oplog(&agent_id, OplogIndex::INITIAL).await?;
    let has_remove = oplog
        .iter()
        .any(|e| matches!(&e.entry, PublicOplogEntry::RemoveRetryPolicy(_)));
    assert!(has_remove, "Expected RemoveRetryPolicy oplog entry");

    executor.check_oplog_is_queryable(&agent_id).await?;

    drop(executor);
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn retry_policy_survives_restart(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;
    let parsed_agent_id = agent_id!("GolemHostApi", "restart-retry");

    let agent_id = executor
        .start_agent(&component.id, parsed_agent_id.clone())
        .await?;

    executor
        .invoke_and_await_agent(
            &component,
            &parsed_agent_id,
            "set_simple_count_retry_policy",
            data_value!("persistent".to_string(), 10u32, 7u32),
        )
        .await?;

    let result = executor
        .invoke_and_await_agent(
            &component,
            &parsed_agent_id,
            "has_retry_policy",
            data_value!("persistent".to_string()),
        )
        .await?;
    assert_eq!(result.into_return_value().unwrap(), Value::Bool(true));

    drop(executor);

    // Restart executor with the same context — component store persists
    let executor = start(deps, &context).await?;

    let result = executor
        .invoke_and_await_agent(
            &component,
            &parsed_agent_id,
            "has_retry_policy",
            data_value!("persistent".to_string()),
        )
        .await?;
    assert_eq!(
        result.into_return_value().unwrap(),
        Value::Bool(true),
        "Retry policy should survive restart via oplog replay"
    );

    let result = executor
        .invoke_and_await_agent(
            &component,
            &parsed_agent_id,
            "list_retry_policy_names",
            data_value!(),
        )
        .await?;
    let names = result.into_return_value().unwrap();
    match &names {
        Value::List(items) => {
            let name_strings: Vec<&str> = items
                .iter()
                .filter_map(|v| match v {
                    Value::String(s) => Some(s.as_str()),
                    _ => None,
                })
                .collect();
            assert!(
                name_strings.contains(&"persistent"),
                "Expected 'persistent' in policy names, got {name_strings:?}"
            );
        }
        other => panic!("Expected List return value, got {other:?}"),
    }

    executor.check_oplog_is_queryable(&agent_id).await?;

    drop(executor);
    Ok(())
}

#[test]
#[tracing::instrument]
async fn get_missing_retry_policy_returns_false(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;
    let parsed_agent_id = agent_id!("GolemHostApi", "missing-retry");
    let agent_id = executor
        .start_agent(&component.id, parsed_agent_id.clone())
        .await?;

    let result = executor
        .invoke_and_await_agent(
            &component,
            &parsed_agent_id,
            "has_retry_policy",
            data_value!("nonexistent".to_string()),
        )
        .await?;
    assert_eq!(result.into_return_value().unwrap(), Value::Bool(false));

    executor.check_oplog_is_queryable(&agent_id).await?;

    drop(executor);
    Ok(())
}

#[test]
#[tracing::instrument]
async fn list_retry_policy_names_returns_all(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;
    let parsed_agent_id = agent_id!("GolemHostApi", "list-retry-names");
    let agent_id = executor
        .start_agent(&component.id, parsed_agent_id.clone())
        .await?;

    for (name, priority, max_retries) in [("zebra", 1u32, 1u32), ("alpha", 2, 2), ("middle", 3, 3)]
    {
        executor
            .invoke_and_await_agent(
                &component,
                &parsed_agent_id,
                "set_simple_count_retry_policy",
                data_value!(name.to_string(), priority, max_retries),
            )
            .await?;
    }

    let result = executor
        .invoke_and_await_agent(
            &component,
            &parsed_agent_id,
            "list_retry_policy_names",
            data_value!(),
        )
        .await?;
    let names = result.into_return_value().unwrap();
    match &names {
        Value::List(items) => {
            let name_strings: Vec<&str> = items
                .iter()
                .filter_map(|v| match v {
                    Value::String(s) => Some(s.as_str()),
                    _ => None,
                })
                .collect();
            assert!(
                name_strings.contains(&"zebra"),
                "Expected 'zebra' in {name_strings:?}"
            );
            assert!(
                name_strings.contains(&"alpha"),
                "Expected 'alpha' in {name_strings:?}"
            );
            assert!(
                name_strings.contains(&"middle"),
                "Expected 'middle' in {name_strings:?}"
            );
        }
        other => panic!("Expected List return value, got {other:?}"),
    }

    executor.check_oplog_is_queryable(&agent_id).await?;

    drop(executor);
    Ok(())
}
