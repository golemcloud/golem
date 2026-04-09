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
    STORAGE_BYTES_WRITTEN_TOTAL, STORAGE_OBJECTS_DELETED_TOTAL, STORAGE_OBJECTS_WRITTEN_TOTAL,
    STORAGE_TYPE_BLOB_STORE,
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
async fn blobstore_exists_return_true_if_the_container_was_created(
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
    let agent_id = agent_id!("BlobStore", "blob-store-service-1");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    let container_name = format!("{}-blob-store-service-1-container", component.id);

    executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "create_container",
            data_value!(container_name.clone()),
        )
        .await?;

    let result = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "container_exists",
            data_value!(container_name),
        )
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected return value"))?;

    executor.check_oplog_is_queryable(&worker_id).await?;

    drop(executor);

    assert_eq!(result, Value::Bool(true));

    Ok(())
}

#[test]
#[tracing::instrument]
async fn blobstore_exists_return_false_if_the_container_was_not_created(
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
    let agent_id = agent_id!("BlobStore", "blob-store-service-1");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    let result = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "container_exists",
            data_value!(format!("{}-blob-store-service-1-container", component.id)),
        )
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected return value"))?;

    executor.check_oplog_is_queryable(&worker_id).await?;

    drop(executor);

    assert_eq!(result, Value::Bool(false));
    Ok(())
}

#[test]
#[tracing::instrument]
async fn blobstore_write_increments_storage_bytes_written_metric(
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
    let agent_id = agent_id!("BlobStore", "blob-store-metrics-write-1");

    let container_name = format!("{}-metrics-write-container", component.id);
    let data: Vec<u8> = vec![1u8; 128];
    let account_id = context.account_id.to_string();
    let environment_id = context.default_environment_id.to_string();

    let bytes_before = STORAGE_BYTES_WRITTEN_TOTAL
        .with_label_values(&[STORAGE_TYPE_BLOB_STORE, &account_id, &environment_id])
        .get();
    let objects_before = STORAGE_OBJECTS_WRITTEN_TOTAL
        .with_label_values(&[STORAGE_TYPE_BLOB_STORE, &account_id, &environment_id])
        .get();

    executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "create_container",
            data_value!(container_name.clone()),
        )
        .await?;

    executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "write_object",
            data_value!(container_name.clone(), "obj1".to_string(), data.clone()),
        )
        .await?;

    executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "write_object",
            data_value!(container_name, "obj2".to_string(), data.clone()),
        )
        .await?;

    drop(executor);

    assert_eq!(
        STORAGE_BYTES_WRITTEN_TOTAL
            .with_label_values(&[STORAGE_TYPE_BLOB_STORE, &account_id, &environment_id])
            .get(),
        bytes_before + 256.0,
        "bytes_written should increase by 128 + 128 = 256"
    );
    assert_eq!(
        STORAGE_OBJECTS_WRITTEN_TOTAL
            .with_label_values(&[STORAGE_TYPE_BLOB_STORE, &account_id, &environment_id])
            .get(),
        objects_before + 2.0,
        "objects_written should increase by 2"
    );

    Ok(())
}

#[test]
#[tracing::instrument]
async fn blobstore_delete_increments_storage_objects_deleted_metric(
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
    let agent_id = agent_id!("BlobStore", "blob-store-metrics-delete-1");

    let container_name = format!("{}-metrics-delete-container", component.id);
    let data: Vec<u8> = vec![42u8; 64];
    let account_id = context.account_id.to_string();
    let environment_id = context.default_environment_id.to_string();

    executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "create_container",
            data_value!(container_name.clone()),
        )
        .await?;

    executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "write_object",
            data_value!(
                container_name.clone(),
                "obj-to-delete".to_string(),
                data.clone()
            ),
        )
        .await?;

    let objects_deleted_before = STORAGE_OBJECTS_DELETED_TOTAL
        .with_label_values(&[STORAGE_TYPE_BLOB_STORE, &account_id, &environment_id])
        .get();

    executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "delete_object",
            data_value!(container_name.clone(), "obj-to-delete".to_string()),
        )
        .await?;

    drop(executor);

    assert_eq!(
        STORAGE_OBJECTS_DELETED_TOTAL
            .with_label_values(&[STORAGE_TYPE_BLOB_STORE, &account_id, &environment_id])
            .get(),
        objects_deleted_before + 1.0,
        "objects_deleted should increase by 1 after delete_object"
    );

    Ok(())
}
