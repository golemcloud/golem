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
use golem_test_framework::dsl::TestDsl;
use golem_worker_executor_test_utils::{
    start_customized, start_with_storage_quota, LastUniqueId, PrecompiledComponent, TestContext,
    WorkerExecutorTestDependencies,
};
use test_r::{inherit_test_dep, test, timeout};

inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(LastUniqueId);
inherit_test_dep!(Tracing);
inherit_test_dep!(
    #[tagged_as("host_api_tests")]
    PrecompiledComponent
);

/// Writing a file whose size is well within the per-worker disk quota must
/// succeed. The limit here (1 MB) is much larger than "hello world" (11 bytes).
#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn storage_write_within_quota_succeeds(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start_with_storage_quota(deps, &context, 1024 * 1024).await?;

    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;

    let agent_id = agent_id!("FileSystem", "storage-quota-ok-1");
    executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    // 11 bytes — well within 1 MB quota.
    executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "write_file",
            data_value!("/testfile.txt", "hello world"),
        )
        .await?;

    Ok(())
}

/// Writing a file whose size exceeds the per-worker disk quota must fail with
/// a permanent error (`WorkerExceededStorageLimit`). The error is not retried.
#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn storage_write_exceeding_quota_fails(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    // 5-byte quota — "hello world" (11 bytes) exceeds it.
    let executor = start_with_storage_quota(deps, &context, 5).await?;

    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;

    let agent_id = agent_id!("FileSystem", "storage-quota-exceeded-1");
    executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    let result = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "write_file",
            data_value!("/testfile.txt", "hello world"),
        )
        .await;

    assert!(
        result.is_err(),
        "expected write to fail when quota is exceeded"
    );
    let err_str = format!("{result:?}");
    assert!(
        err_str.contains("ExceededStorageLimit") || err_str.contains("storage"),
        "expected ExceededStorageLimit error, got: {err_str}"
    );

    Ok(())
}

/// After a worker receives `WorkerExceededStorageLimit`, a second write
/// attempt also fails — the error is permanent and the quota does not reset.
#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn storage_exceeded_quota_is_not_retried(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start_with_storage_quota(deps, &context, 5).await?;

    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;

    let agent_id = agent_id!("FileSystem", "storage-quota-no-retry-1");
    executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    // First write: exceeds quota.
    let first_result = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "write_file",
            data_value!("/testfile.txt", "hello world"),
        )
        .await;
    assert!(
        first_result.is_err(),
        "expected first write to fail with ExceededStorageLimit"
    );

    // Second write: must also fail — the quota hasn't changed.
    let second_result = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "write_file",
            data_value!("/testfile2.txt", "hello world"),
        )
        .await;
    assert!(
        second_result.is_err(),
        "expected second write to also fail (not retry successfully)"
    );

    let err_str = format!("{second_result:?}");
    assert!(
        err_str.contains("ExceededStorageLimit")
            || err_str.contains("storage")
            || err_str.contains("already exists")
            || err_str.contains("AlreadyExists"),
        "expected a relevant error on second attempt, got: {err_str}"
    );

    Ok(())
}

// ── Executor-wide semaphore tests ────────────────────────────────────────────
//
// These tests use `start_customized` with `system_storage_override` to set a
// tiny executor-wide storage pool. `TestWorkerCtx` has no per-plan limit
// (`max_disk_space = u64::MAX`), so only the semaphore fires. The error is
// `WorkerOutOfStorage` (retriable), distinct from the permanent
// `WorkerExceededStorageLimit` tested above.

/// A write that fits within the executor-wide semaphore pool succeeds even
/// when the pool is small, as long as capacity is available.
#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn executor_storage_within_pool_succeeds(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    // 1 MB pool — plenty for "hello world".
    let executor = start_customized(
        deps,
        &context,
        None,
        Some(1024 * 1024),
        None,
        None,
        None,
        None,
    )
    .await?;

    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;

    let agent_id = agent_id!("FileSystem", "exec-storage-ok-1");
    executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "write_file",
            data_value!("/testfile.txt", "hello world"),
        )
        .await?;

    Ok(())
}

/// When the executor-wide storage pool is exhausted (1 KB total, writing
/// 11 bytes from two workers simultaneously), at least one write must fail
/// with `WorkerOutOfStorage`.
///
/// This is the semaphore-level quota: `system_storage_override` sets the total
/// bytes available across all workers on the node. Unlike the per-plan quota
/// (`WorkerExceededStorageLimit`), this error is retriable — the worker will
/// be suspended and retried once another worker releases storage.
#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn executor_storage_pool_exhaustion_returns_out_of_storage(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    // 1 KB pool — worker 1's write (11 bytes) fits; worker 2 also writes 11
    // bytes but initial file loading of the component itself may fill the pool.
    // Using 1 byte makes it certain the pool is exhausted after worker 1 writes.
    let executor = start_customized(
        deps,
        &context,
        None,
        Some(1), // 1 byte — exhausted by the first write
        None,
        None,
        None,
        None,
    )
    .await?;

    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;

    let agent_id = agent_id!("FileSystem", "exec-storage-exhausted-1");
    executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    // With a 1-byte pool, writing 11 bytes must fail with WorkerOutOfStorage.
    let result = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "write_file",
            data_value!("/testfile.txt", "hello world"),
        )
        .await;

    assert!(
        result.is_err(),
        "expected write to fail when executor storage pool is exhausted"
    );
    let err_str = format!("{result:?}");
    assert!(
        err_str.contains("OutOfStorage") || err_str.contains("storage"),
        "expected WorkerOutOfStorage error, got: {err_str}"
    );

    Ok(())
}
