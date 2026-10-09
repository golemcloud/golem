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
use crate::oplog_blob_archive::new_s3_test;
use golem_common::config::DbSqliteConfig;
use golem_common::model::agent::ParsedAgentId;
use golem_common::model::component::ComponentDto;
use golem_common::model::oplog::{OplogIndex, PublicOplogEntry};
use golem_common::{agent_id, data_value};
use golem_service_base::db::sqlite::SqlitePool;
use golem_service_base::storage::blob::BlobStorage;
use golem_service_base::storage::blob::memory::InMemoryBlobStorage;
use golem_service_base::storage::blob::sqlite::SqliteBlobStorage;
use golem_test_framework::dsl::TestDsl;
use golem_worker_executor::metrics::storage::{
    STORAGE_BYTES_WRITTEN_TOTAL, STORAGE_OBJECTS_DELETED_TOTAL, STORAGE_OBJECTS_WRITTEN_TOTAL,
    STORAGE_TYPE_BLOB_STORE,
};
use golem_worker_executor::services::blob_store::DefaultBlobStoreService;
use golem_worker_executor_test_utils::{
    LastUniqueId, PrecompiledComponent, TestContext, TestExecutorOverrides, TestWorkerExecutor,
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

    for (root, operation, result) in
        probe_the_root_container_names(&executor, &component, &agent_id, "destination").await?
    {
        let error = result.expect_err("a namespace root is not a container");
        assert!(
            error.to_ascii_lowercase().contains("invalid"),
            "unexpected error for {operation}({root:?}): {error}"
        );
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

/// A guest writes an object below another object, and an object at the path of a container that
/// it created. The blob storage of the test executor keeps the two at one path, so each write
/// passes the first time and no write gives an error that the executor retries.
#[test]
#[tracing::instrument]
async fn blobstore_writes_an_object_below_an_object_and_over_a_container_without_retrying(
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
    let agent_id = agent_id!("BlobStore", "object-below-object");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    for container in ["objects", "objects/made"] {
        let created = executor
            .invoke_and_await_agent(
                &component,
                &agent_id,
                "blobstore_probe",
                data_value!("create-container", container, "", "", ""),
            )
            .await?
            .into_typed::<Result<(), String>>()?;
        assert_eq!(created, Ok(()), "create-container({container})");
    }

    let objects = [
        ("upper", b"upper".to_vec()),
        ("upper/lower", b"lower".to_vec()),
        ("made", b"made".to_vec()),
    ];
    for (object, data) in &objects {
        let written = executor
            .invoke_and_await_agent(
                &component,
                &agent_id,
                "write_data_result",
                data_value!("objects", *object, data.clone()),
            )
            .await?
            .into_typed::<Result<(), String>>()?;
        assert_eq!(written, Ok(()), "write-data({object})");
    }

    for (object, data) in objects {
        let read = executor
            .invoke_and_await_agent(
                &component,
                &agent_id,
                "get_data",
                data_value!("objects", object),
            )
            .await?
            .into_typed::<Vec<u8>>()?;
        assert_eq!(read, data, "get-data({object})");
    }

    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    assert!(
        oplog
            .iter()
            .all(|entry| !matches!(entry.entry, PublicOplogEntry::Error(_))),
        "the writes must pass without a retry: {oplog:?}"
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
    // The in-memory backend gives no metadata for a root path, as the S3 backend does, so a read
    // that finds no container is not hidden.
    check_the_root_container_names(
        last_unique_id,
        deps,
        host_api_tests,
        Arc::new(InMemoryBlobStorage::new()),
    )
    .await
}

/// The root container names give the same permanent error on the SQLite backend.
#[test]
#[tracing::instrument]
async fn blobstore_gives_an_error_for_a_container_name_at_the_root_on_sqlite(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let root = tempfile::TempDir::new()?;
    let pool = SqlitePool::configured(&DbSqliteConfig {
        database: root
            .path()
            .join("blob_storage.db")
            .to_string_lossy()
            .into_owned(),
        max_connections: 4,
        foreign_keys: false,
    })
    .await?;
    check_the_root_container_names(
        last_unique_id,
        deps,
        host_api_tests,
        Arc::new(SqliteBlobStorage::new(pool).await?),
    )
    .await
}

/// The root container names give the same permanent error on the S3 backend, over RustFS.
#[test]
#[tracing::instrument]
async fn blobstore_gives_an_error_for_a_container_name_at_the_root_on_s3(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    // The test keeps the RustFS container of the storage until it ends.
    let s3 = new_s3_test();
    check_the_root_container_names(
        last_unique_id,
        deps,
        host_api_tests,
        Arc::new(s3.s3_storage().await),
    )
    .await
}

/// Drives each host function that takes a container name with each root name, through the blob
/// store service of production over `storage`, and requires the permanent error of a name.
async fn check_the_root_container_names(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    host_api_tests: &PrecompiledComponent,
    storage: Arc<dyn BlobStorage + Send + Sync>,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start_with_overrides(
        deps,
        &context,
        TestExecutorOverrides {
            wrap_blob_store_service: Some(Arc::new(move |_| {
                Arc::new(DefaultBlobStoreService::new(storage.clone()))
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

    let results =
        probe_the_root_container_names(&executor, &component, &agent_id, &container_name).await?;

    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    assert!(
        oplog
            .iter()
            .all(|entry| !matches!(entry.entry, PublicOplogEntry::Error(_))),
        "permanent root-container errors must not produce retry entries: {oplog:?}"
    );
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

/// Gives the answer of each host function of `wasi:blobstore/blobstore` that takes a container
/// name to each spelling of the root name: as the container of the first four probes, and then as
/// the source and the destination container of a copy and of a move whose other end is
/// `container`.
async fn probe_the_root_container_names(
    executor: &TestWorkerExecutor,
    component: &ComponentDto,
    agent_id: &ParsedAgentId,
    container: &str,
) -> anyhow::Result<Vec<(&'static str, &'static str, Result<(), String>)>> {
    let mut results = Vec::new();
    for root in ["", ".", "./", "././"] {
        for (operation, source, destination) in [
            ("create-container", root, root),
            ("get-container", root, root),
            ("delete-container", root, root),
            ("container-exists", root, root),
            ("copy-object", root, container),
            ("copy-object", container, root),
            ("move-object", root, container),
            ("move-object", container, root),
        ] {
            let result = executor
                .invoke_and_await_agent(
                    component,
                    agent_id,
                    "blobstore_probe",
                    data_value!(operation, source, "object", destination, "object"),
                )
                .await?
                .into_typed::<Result<(), String>>()?;
            results.push((root, operation, result));
        }
    }
    Ok(results)
}
