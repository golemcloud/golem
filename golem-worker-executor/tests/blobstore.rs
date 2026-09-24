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
use golem_service_base::config::BlobStorageConfig;
use golem_test_framework::dsl::TestDsl;
use golem_worker_executor::metrics::storage::{
    STORAGE_BYTES_WRITTEN_TOTAL, STORAGE_OBJECTS_DELETED_TOTAL, STORAGE_OBJECTS_WRITTEN_TOTAL,
    STORAGE_TYPE_BLOB_STORE,
};
use golem_worker_executor_test_utils::{
    LastUniqueId, PrecompiledComponent, TestContext, TestExecutorOverrides,
    WorkerExecutorTestDependencies, start, start_with_overrides,
};
use pretty_assertions::assert_eq;
use std::sync::Arc;
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
        .into_typed::<bool>()?;

    executor.check_oplog_is_queryable(&worker_id).await?;

    drop(executor);

    assert!(result);

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
        .into_typed::<bool>()?;

    executor.check_oplog_is_queryable(&worker_id).await?;

    drop(executor);

    assert!(!result);
    Ok(())
}

#[test]
#[tracing::instrument]
async fn blobstore_rejects_root_container_names_without_retrying(
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
    let agent_id = agent_id!("BlobStore", "root-container-contract");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    for root in ["", ".", "./", "././"] {
        for (operation, source, destination) in [
            ("create-container", root, "destination"),
            ("get-container", root, "destination"),
            ("delete-container", root, "destination"),
            ("container-exists", root, "destination"),
            ("copy-object", root, "destination"),
            ("copy-object", "source", root),
            ("move-object", root, "destination"),
            ("move-object", "source", root),
        ] {
            let result = executor
                .invoke_and_await_agent(
                    &component,
                    &agent_id,
                    "blobstore_probe",
                    data_value!(operation, source, "object", destination, "object"),
                )
                .await?
                .into_typed::<Result<(), String>>()?;
            let error = result.expect_err("a namespace root is not a container");
            assert!(
                error.to_ascii_lowercase().contains("invalid"),
                "unexpected error for {operation}({root:?}): {error}"
            );
        }
    }

    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    assert!(
        oplog
            .iter()
            .all(|entry| !matches!(entry.entry, PublicOplogEntry::Error(_))),
        "permanent root-container errors must not produce retry entries: {oplog:?}"
    );

    Ok(())
}

#[test]
#[tracing::instrument]
async fn blobstore_rejects_root_object_names_without_hiding_the_container(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start_with_overrides(
        deps,
        &context,
        TestExecutorOverrides {
            configure: Some(Arc::new(|config| {
                config.blob_storage = BlobStorageConfig::default_in_memory();
            })),
            ..Default::default()
        },
    )
    .await?;
    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;
    let agent_id = agent_id!("BlobStore", "root-object-contract");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    for (index, root) in ["", ".", "./", "././"].into_iter().enumerate() {
        let container = format!("root-object-container-{index}");
        executor
            .invoke_and_await_agent(
                &component,
                &agent_id,
                "create_container",
                data_value!(container.clone()),
            )
            .await?;
        executor
            .invoke_and_await_agent(
                &component,
                &agent_id,
                "write_data",
                data_value!(container.clone(), "object", b"kept".to_vec()),
            )
            .await?;

        for (operation, object, objects) in [
            ("write-data", root, Vec::<String>::new()),
            ("delete-object", root, Vec::<String>::new()),
            (
                "delete-objects",
                "object",
                vec!["object".to_string(), root.to_string()],
            ),
            ("has-object", root, Vec::<String>::new()),
            ("object-info", root, Vec::<String>::new()),
        ] {
            let result = executor
                .invoke_and_await_agent(
                    &component,
                    &agent_id,
                    "container_probe",
                    data_value!(
                        operation,
                        container.clone(),
                        object,
                        objects,
                        b"data".to_vec()
                    ),
                )
                .await?
                .into_typed::<Result<(), String>>()?;
            let error = result.expect_err("a namespace root is not an object");
            assert!(
                error.to_ascii_lowercase().contains("invalid"),
                "unexpected error for {operation}({root:?}): {error}"
            );
        }

        let kept = executor
            .invoke_and_await_agent(
                &component,
                &agent_id,
                "get_data",
                data_value!(container.clone(), "object"),
            )
            .await?
            .into_typed::<Vec<u8>>()?;
        assert_eq!(kept, b"kept", "a rejected batch deleted a valid object");

        for (operation, source, destination) in [
            ("copy-object", root, "object"),
            ("copy-object", "object", root),
            ("move-object", root, "object"),
            ("move-object", "object", root),
        ] {
            let result = executor
                .invoke_and_await_agent(
                    &component,
                    &agent_id,
                    "blobstore_probe",
                    data_value!(
                        operation,
                        container.clone(),
                        source,
                        container.clone(),
                        destination
                    ),
                )
                .await?
                .into_typed::<Result<(), String>>()?;
            let error = result.expect_err("a namespace root is not an object");
            assert!(
                error.to_ascii_lowercase().contains("invalid"),
                "unexpected error for {operation}({root:?}): {error}"
            );
        }

        let read = executor
            .invoke_and_await_agent(
                &component,
                &agent_id,
                "container_probe",
                data_value!(
                    "get-data",
                    container.clone(),
                    root,
                    Vec::<String>::new(),
                    Vec::<u8>::new()
                ),
            )
            .await?
            .into_typed::<Result<(), String>>()?;
        let error = read.expect_err("the root read must keep the missing-object result");
        assert!(
            error.to_ascii_lowercase().contains("not found"),
            "unexpected root read error for {root:?}: {error}"
        );

        let container_exists = executor
            .invoke_and_await_agent(
                &component,
                &agent_id,
                "container_exists",
                data_value!(container),
            )
            .await?
            .into_typed::<bool>()?;
        assert!(
            container_exists,
            "root write hid its container for {root:?}"
        );
    }

    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    assert!(
        oplog
            .iter()
            .all(|entry| !matches!(entry.entry, PublicOplogEntry::Error(_))),
        "permanent root-object errors must not produce retry entries: {oplog:?}"
    );

    Ok(())
}

#[test]
#[tracing::instrument]
async fn blobstore_missing_copy_and_move_return_without_retrying(
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
    let agent_id = agent_id!("BlobStore", "missing-copy-contract");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    for operation in ["copy-object", "move-object"] {
        let result = executor
            .invoke_and_await_agent(
                &component,
                &agent_id,
                "blobstore_probe",
                data_value!(operation, "source", "missing", "destination", "object"),
            )
            .await?
            .into_typed::<Result<(), String>>()?;
        let error = result.expect_err("the source blob does not exist");
        assert!(
            error.to_ascii_lowercase().contains("not found"),
            "unexpected error for {operation}: {error}"
        );
    }

    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    assert!(
        oplog
            .iter()
            .all(|entry| !matches!(entry.entry, PublicOplogEntry::Error(_))),
        "permanent missing-source errors must not produce retry entries: {oplog:?}"
    );

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
