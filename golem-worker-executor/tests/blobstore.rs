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
use golem_common::{agent_id, data_value};
use golem_service_base::storage::blob::memory::InMemoryBlobStorage;
use golem_test_framework::dsl::TestDsl;
use golem_worker_executor::metrics::storage::{
    STORAGE_BYTES_WRITTEN_TOTAL, STORAGE_OBJECTS_DELETED_TOTAL, STORAGE_OBJECTS_WRITTEN_TOTAL,
    STORAGE_TYPE_BLOB_STORE,
};
use golem_worker_executor::services::blob_store::DefaultBlobStoreService;
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

#[test]
#[tracing::instrument]
async fn blobstore_get_data_outside_the_object_gives_the_guest_an_error(
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
    let agent_id = agent_id!("BlobStore", "blob-store-range-1");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    let container_name = format!("{}-blob-store-range-1-container", component.id);

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
            "write_data",
            data_value!(
                container_name.clone(),
                "range-object",
                vec![10u8, 20u8, 30u8]
            ),
        )
        .await?;

    let end_after_the_object = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "get_data_range_result",
            data_value!(container_name.clone(), "range-object", 1u64, 3u64),
        )
        .await?
        .into_typed::<Result<Vec<u8>, String>>()?;
    assert!(
        end_after_the_object.as_ref().is_err_and(
            |error| error.contains("Invalid input: the byte range 1-3 is not in the blob")
        ),
        "{end_after_the_object:?}"
    );

    let start_after_the_object = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "get_data_range_result",
            data_value!(container_name.clone(), "range-object", 3u64, 5u64),
        )
        .await?
        .into_typed::<Result<Vec<u8>, String>>()?;
    let whole_object = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "get_data_range_result",
            data_value!(container_name, "range-object", 0u64, 2u64),
        )
        .await?
        .into_typed::<Result<Vec<u8>, String>>()?;

    executor.check_oplog_is_queryable(&worker_id).await?;

    drop(executor);

    assert!(
        start_after_the_object.as_ref().is_err_and(
            |error| error.contains("Invalid input: the byte range 3-5 is not in the blob")
        ),
        "{start_after_the_object:?}"
    );
    assert_eq!(whole_object, Ok(vec![10u8, 20u8, 30u8]));

    Ok(())
}

/// A guest picks the container name, and `""`, `"."`, `"./"` and `"././"` are four spellings of
/// one name: the root of the namespace, which names no container. Each host function of
/// `wasi:blobstore/blobstore` that takes a container name gives the guest a permanent error for
/// each of the four, and the executor keeps running.
///
/// `create-container` is the one that stopped the executor. It reads the container back for the
/// time that the guest gets, `create_dir` leaves no directory at the root, so the read found no
/// container. The host read that answer with `Option::unwrap`. The harness of this test unwinds
/// a panic and fails the invocation, while the executor binary is built with `panic = "abort"`,
/// so there one container name of a guest stopped the process and every worker on it.
#[test]
#[tracing::instrument]
async fn blobstore_gives_an_error_for_a_container_name_at_the_root_of_the_namespace(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    // The executor runs the blob store service of production over the in-memory backend, which
    // gives no metadata for a root path, as the S3 backend does. The filesystem backend that
    // the test dependencies hold gives the metadata of the directory of the namespace there, so
    // it hides the read that found no container.
    let executor = start_with_overrides(
        deps,
        &context,
        TestExecutorOverrides {
            wrap_blob_store_service: Some(Arc::new(|_| {
                Arc::new(DefaultBlobStoreService::new(Arc::new(
                    InMemoryBlobStorage::new(),
                )))
            })),
            ..Default::default()
        },
    )
    .await?;

    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;
    let agent_id = agent_id!("BlobStore", "blob-store-root-name-1");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    let container_name = format!("{}-blob-store-root-name-1-container", component.id);
    executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "create_container",
            data_value!(container_name.clone()),
        )
        .await?;

    let mut results = Vec::new();
    for root_name in ["", ".", "./", "././"] {
        // The root name is the container of the first four probes, and then the source
        // container and the destination container of a copy and of a move.
        let probes = [
            ("create-container", root_name, root_name),
            ("get-container", root_name, root_name),
            ("delete-container", root_name, root_name),
            ("container-exists", root_name, root_name),
            ("copy-object", root_name, container_name.as_str()),
            ("copy-object", container_name.as_str(), root_name),
            ("move-object", root_name, container_name.as_str()),
            ("move-object", container_name.as_str(), root_name),
        ];

        for (operation, source_container, destination_container) in probes {
            let result = executor
                .invoke_and_await_agent(
                    &component,
                    &agent_id,
                    "blobstore_probe",
                    data_value!(
                        operation,
                        source_container,
                        "object",
                        destination_container,
                        "object"
                    ),
                )
                .await?
                .into_typed::<Result<(), String>>()?;

            results.push((root_name, operation, result));
        }
    }

    executor.check_oplog_is_queryable(&worker_id).await?;

    drop(executor);

    assert!(
        results.iter().all(|(_, _, result)| result
            .as_ref()
            .is_err_and(|error| error.contains("Invalid input: the blob path has no name in it"))),
        "{results:?}"
    );

    Ok(())
}
