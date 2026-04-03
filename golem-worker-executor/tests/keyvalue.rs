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
use anyhow::anyhow;
use golem_common::{agent_id, data_value};
use golem_test_framework::dsl::TestDsl;
use golem_wasm::Value;
use golem_worker_executor::metrics::storage::{
    STORAGE_TYPE_KV, STORAGE_BYTES_WRITTEN_TOTAL, STORAGE_OBJECTS_DELETED_TOTAL,
    STORAGE_OBJECTS_WRITTEN_TOTAL,
};
use golem_worker_executor_test_utils::{
    LastUniqueId, PrecompiledComponent, TestContext, WorkerExecutorTestDependencies, start,
};
use pretty_assertions::assert_eq;
use test_r::{inherit_test_dep, test};

inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(LastUniqueId);
inherit_test_dep!(
    #[tagged_as("host_api_tests")]
    PrecompiledComponent
);
inherit_test_dep!(Tracing);

#[test]
#[tracing::instrument]
async fn readwrite_get_returns_the_value_that_was_set(
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
    let agent_id = agent_id!("KeyValue", "key-value-service-1");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "set",
            data_value!(
                format!("{}-key-value-service-1-bucket", component.id),
                "key",
                vec![1u8, 2u8, 3u8]
            ),
        )
        .await?;

    let result = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "get",
            data_value!(
                format!("{}-key-value-service-1-bucket", component.id),
                "key"
            ),
        )
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected return value"))?;

    executor.check_oplog_is_queryable(&worker_id).await?;

    assert_eq!(
        result,
        Value::Option(Some(Box::new(Value::List(vec![
            Value::U8(1),
            Value::U8(2),
            Value::U8(3),
        ]))))
    );

    Ok(())
}

#[test]
#[tracing::instrument]
async fn readwrite_get_fails_if_the_value_was_not_set(
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
    let agent_id = agent_id!("KeyValue", "key-value-service-2");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    let result = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "get",
            data_value!(
                format!("{}-key-value-service-2-bucket", component.id),
                "key"
            ),
        )
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected return value"))?;

    executor.check_oplog_is_queryable(&worker_id).await?;

    assert_eq!(result, Value::Option(None));

    Ok(())
}

#[test]
#[tracing::instrument]
async fn readwrite_set_replaces_the_value_if_it_was_already_set(
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
    let agent_id = agent_id!("KeyValue", "key-value-service-3");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "set",
            data_value!(
                format!("{}-key-value-service-3-bucket", component.id),
                "key",
                vec![1u8, 2u8, 3u8]
            ),
        )
        .await?;

    executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "set",
            data_value!(
                format!("{}-key-value-service-3-bucket", component.id),
                "key",
                vec![4u8, 5u8, 6u8]
            ),
        )
        .await?;

    let result = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "get",
            data_value!(
                format!("{}-key-value-service-3-bucket", component.id),
                "key"
            ),
        )
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected return value"))?;

    executor.check_oplog_is_queryable(&worker_id).await?;

    assert_eq!(
        result,
        Value::Option(Some(Box::new(Value::List(vec![
            Value::U8(4),
            Value::U8(5),
            Value::U8(6),
        ]))))
    );

    Ok(())
}

#[test]
#[tracing::instrument]
async fn readwrite_delete_removes_the_value_if_it_was_already_set(
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
    let agent_id = agent_id!("KeyValue", "key-value-service-4");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "set",
            data_value!(
                format!("{}-key-value-service-4-bucket", component.id),
                "key",
                vec![1u8, 2u8, 3u8]
            ),
        )
        .await?;

    executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "delete",
            data_value!(
                format!("{}-key-value-service-4-bucket", component.id),
                "key"
            ),
        )
        .await?;

    let result = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "get",
            data_value!(
                format!("{}-key-value-service-4-bucket", component.id),
                "key"
            ),
        )
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected return value"))?;

    executor.check_oplog_is_queryable(&worker_id).await?;

    assert_eq!(result, Value::Option(None));

    Ok(())
}

#[test]
#[tracing::instrument]
async fn readwrite_exists_returns_true_if_the_value_was_set(
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
    let agent_id = agent_id!("KeyValue", "key-value-service-5");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "set",
            data_value!(
                format!("{}-key-value-service-5-bucket", component.id),
                "key",
                vec![1u8, 2u8, 3u8]
            ),
        )
        .await?;

    let result = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "exists",
            data_value!(
                format!("{}-key-value-service-5-bucket", component.id),
                "key"
            ),
        )
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected return value"))?;

    executor.check_oplog_is_queryable(&worker_id).await?;

    assert_eq!(result, Value::Bool(true));
    Ok(())
}

#[test]
#[tracing::instrument]
async fn readwrite_exists_returns_false_if_the_value_was_not_set(
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
    let agent_id = agent_id!("KeyValue", "key-value-service-6");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    let result = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "exists",
            data_value!(
                format!("{}-key-value-service-6-bucket", component.id),
                "key"
            ),
        )
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected return value"))?;

    executor.check_oplog_is_queryable(&worker_id).await?;

    assert_eq!(result, Value::Bool(false));

    Ok(())
}

#[test]
#[tracing::instrument]
async fn readwrite_buckets_can_be_shared_between_workers(
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
    let agent_id_1 = agent_id!("KeyValue", "key-value-service-7");
    let worker_id_1 = executor
        .start_agent(&component.id, agent_id_1.clone())
        .await?;
    let agent_id_2 = agent_id!("KeyValue", "key-value-service-8");
    let worker_id_2 = executor
        .start_agent(&component.id, agent_id_2.clone())
        .await?;

    executor
        .invoke_and_await_agent(
            &component,
            &agent_id_1,
            "set",
            data_value!(
                format!("{}-bucket", component.id),
                "key",
                vec![1u8, 2u8, 3u8]
            ),
        )
        .await?;

    let result = executor
        .invoke_and_await_agent(
            &component,
            &agent_id_2,
            "get",
            data_value!(format!("{}-bucket", component.id), "key"),
        )
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected return value"))?;

    executor.check_oplog_is_queryable(&worker_id_1).await?;
    executor.check_oplog_is_queryable(&worker_id_2).await?;

    assert_eq!(
        result,
        Value::Option(Some(Box::new(Value::List(vec![
            Value::U8(1),
            Value::U8(2),
            Value::U8(3),
        ]))))
    );

    Ok(())
}

#[test]
#[tracing::instrument]
async fn batch_get_many_gets_multiple_values(
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
    let agent_id = agent_id!("KeyValue", "key-value-service-9");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    let bucket = format!("{}-key-value-service-9-bucket", component.id);

    executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "set",
            data_value!(bucket.clone(), "key1", vec![1u8, 2u8, 3u8]),
        )
        .await?;

    executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "set",
            data_value!(bucket.clone(), "key2", vec![4u8, 5u8, 6u8]),
        )
        .await?;

    executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "set",
            data_value!(bucket.clone(), "key3", vec![7u8, 8u8, 9u8]),
        )
        .await?;

    let result = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "get_many",
            data_value!(
                bucket,
                vec!["key1".to_string(), "key2".to_string(), "key3".to_string()]
            ),
        )
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected return value"))?;

    executor.check_oplog_is_queryable(&worker_id).await?;

    assert_eq!(
        result,
        Value::Option(Some(Box::new(Value::List(vec![
            Value::List(vec![Value::U8(1), Value::U8(2), Value::U8(3),]),
            Value::List(vec![Value::U8(4), Value::U8(5), Value::U8(6),]),
            Value::List(vec![Value::U8(7), Value::U8(8), Value::U8(9),])
        ]))))
    );

    Ok(())
}

#[test]
#[tracing::instrument]
async fn batch_get_many_fails_if_any_value_was_not_set(
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
    let agent_id = agent_id!("KeyValue", "key-value-service-10");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    let bucket = format!("{}-key-value-service-10-bucket", component.id);

    executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "set",
            data_value!(bucket.clone(), "key1", vec![1u8, 2u8, 3u8]),
        )
        .await?;

    executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "set",
            data_value!(bucket.clone(), "key2", vec![4u8, 5u8, 6u8]),
        )
        .await?;

    let result = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "get_many",
            data_value!(
                bucket,
                vec!["key1".to_string(), "key2".to_string(), "key3".to_string()]
            ),
        )
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected return value"))?;

    executor.check_oplog_is_queryable(&worker_id).await?;
    assert_eq!(result, Value::Option(None));
    Ok(())
}

#[test]
#[tracing::instrument]
async fn batch_set_many_sets_multiple_values(
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
    let agent_id = agent_id!("KeyValue", "key-value-service-11");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    let bucket = format!("{}-key-value-service-11-bucket", component.id);

    executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "set_many",
            data_value!(
                bucket.clone(),
                vec![
                    ("key1".to_string(), vec![1u8, 2u8, 3u8]),
                    ("key2".to_string(), vec![4u8, 5u8, 6u8]),
                    ("key3".to_string(), vec![7u8, 8u8, 9u8]),
                ]
            ),
        )
        .await?;

    let result1 = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "get",
            data_value!(bucket.clone(), "key1"),
        )
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected return value"))?;

    let result2 = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "get",
            data_value!(bucket.clone(), "key2"),
        )
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected return value"))?;

    let result3 = executor
        .invoke_and_await_agent(&component, &agent_id, "get", data_value!(bucket, "key3"))
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected return value"))?;

    executor.check_oplog_is_queryable(&worker_id).await?;

    assert_eq!(
        result1,
        Value::Option(Some(Box::new(Value::List(vec![
            Value::U8(1),
            Value::U8(2),
            Value::U8(3),
        ]))))
    );
    assert_eq!(
        result2,
        Value::Option(Some(Box::new(Value::List(vec![
            Value::U8(4),
            Value::U8(5),
            Value::U8(6),
        ]))))
    );
    assert_eq!(
        result3,
        Value::Option(Some(Box::new(Value::List(vec![
            Value::U8(7),
            Value::U8(8),
            Value::U8(9),
        ]))))
    );

    Ok(())
}

#[test]
#[tracing::instrument]
async fn batch_delete_many_deletes_multiple_values(
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
    let agent_id = agent_id!("KeyValue", "key-value-service-12");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    let bucket = format!("{}-key-value-service-12-bucket", component.id);

    executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "set",
            data_value!(bucket.clone(), "key1", vec![1u8, 2u8, 3u8]),
        )
        .await?;

    executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "set",
            data_value!(bucket.clone(), "key2", vec![4u8, 5u8, 6u8]),
        )
        .await?;

    executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "set",
            data_value!(bucket.clone(), "key3", vec![7u8, 8u8, 9u8]),
        )
        .await?;

    executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "delete_many",
            data_value!(
                bucket.clone(),
                vec!["key1".to_string(), "key2".to_string(), "key3".to_string()]
            ),
        )
        .await?;

    let result1 = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "get",
            data_value!(bucket.clone(), "key1"),
        )
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected return value"))?;

    let result2 = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "get",
            data_value!(bucket.clone(), "key2"),
        )
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected return value"))?;

    let result3 = executor
        .invoke_and_await_agent(&component, &agent_id, "get", data_value!(bucket, "key3"))
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected return value"))?;

    executor.check_oplog_is_queryable(&worker_id).await?;

    assert_eq!(result1, Value::Option(None));
    assert_eq!(result2, Value::Option(None));
    assert_eq!(result3, Value::Option(None));

    Ok(())
}

#[test]
#[tracing::instrument]
async fn batch_get_keys_returns_multiple_keys(
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
    let agent_id = agent_id!("KeyValue", "key-value-service-13");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    let bucket = format!("{}-key-value-service-13-bucket", component.id);

    executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "set",
            data_value!(bucket.clone(), "key1", vec![1u8, 2u8, 3u8]),
        )
        .await?;

    executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "set",
            data_value!(bucket.clone(), "key2", vec![4u8, 5u8, 6u8]),
        )
        .await?;

    executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "set",
            data_value!(bucket.clone(), "key3", vec![7u8, 8u8, 9u8]),
        )
        .await?;

    let result = executor
        .invoke_and_await_agent(&component, &agent_id, "get_keys", data_value!(bucket))
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected return value"))?;

    executor.check_oplog_is_queryable(&worker_id).await?;

    assert_eq!(
        result,
        Value::List(vec![
            Value::String("key1".to_string()),
            Value::String("key2".to_string()),
            Value::String("key3".to_string()),
        ])
    );

    Ok(())
}

#[test]
#[tracing::instrument]
async fn kv_set_increments_storage_bytes_and_objects_written_metrics(
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
    let agent_id = agent_id!("KeyValue", "kv-metrics-set-1");
    let bucket = format!("{}-kv-metrics-set-bucket", component.id);
    let account_id = context.account_id.to_string();
    let environment_id = context.default_environment_id.to_string();

    let bytes_before = STORAGE_BYTES_WRITTEN_TOTAL
        .with_label_values(&[STORAGE_TYPE_KV, &account_id, &environment_id])
        .get();
    let objects_before = STORAGE_OBJECTS_WRITTEN_TOTAL
        .with_label_values(&[STORAGE_TYPE_KV, &account_id, &environment_id])
        .get();

    // set "hello" (5 bytes)
    executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "set",
            data_value!(bucket.clone(), "k1", vec![104u8, 101u8, 108u8, 108u8, 111u8]),
        )
        .await?;

    // set "world!" (6 bytes)
    executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "set",
            data_value!(bucket.clone(), "k2", vec![119u8, 111u8, 114u8, 108u8, 100u8, 33u8]),
        )
        .await?;

    drop(executor);

    assert_eq!(
        STORAGE_BYTES_WRITTEN_TOTAL
            .with_label_values(&[STORAGE_TYPE_KV, &account_id, &environment_id])
            .get(),
        bytes_before + 11.0,
        "bytes_written should increase by 5 + 6 = 11"
    );
    assert_eq!(
        STORAGE_OBJECTS_WRITTEN_TOTAL
            .with_label_values(&[STORAGE_TYPE_KV, &account_id, &environment_id])
            .get(),
        objects_before + 2.0,
        "objects_written should increase by 2"
    );

    Ok(())
}

#[test]
#[tracing::instrument]
async fn kv_delete_increments_storage_objects_deleted_metric(
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
    let agent_id = agent_id!("KeyValue", "kv-metrics-delete-1");
    let bucket = format!("{}-kv-metrics-delete-bucket", component.id);
    let account_id = context.account_id.to_string();
    let environment_id = context.default_environment_id.to_string();

    // write a key first
    executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "set",
            data_value!(bucket.clone(), "to-delete", vec![1u8, 2u8, 3u8]),
        )
        .await?;

    let objects_deleted_before = STORAGE_OBJECTS_DELETED_TOTAL
        .with_label_values(&[STORAGE_TYPE_KV, &account_id, &environment_id])
        .get();

    executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "delete",
            data_value!(bucket.clone(), "to-delete"),
        )
        .await?;

    drop(executor);

    assert_eq!(
        STORAGE_OBJECTS_DELETED_TOTAL
            .with_label_values(&[STORAGE_TYPE_KV, &account_id, &environment_id])
            .get(),
        objects_deleted_before + 1.0,
        "objects_deleted should increase by 1 after delete"
    );

    Ok(())
}
