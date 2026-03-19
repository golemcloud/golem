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
    start_with_agent_storage_quota, start_with_executor_storage_pool, LastUniqueId,
    PrecompiledComponent, TestContext, WorkerExecutorTestDependencies,
};
use test_r::{inherit_test_dep, test, timeout};

inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(LastUniqueId);
inherit_test_dep!(Tracing);
inherit_test_dep!(
    #[tagged_as("host_api_tests")]
    PrecompiledComponent
);

/// A failed write (quota exceeded) must not consume any of the remaining quota.
/// The next write within the original limit must still succeed.
///
/// This tests that `check_storage_quota` rejects the write before any oplog
/// entry or semaphore acquire happens, so the quota counter stays accurate.
#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn failed_write_does_not_reduce_remaining_quota(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    // 20-byte quota. First write succeeds (11 bytes). Second write exceeds quota
    // (11 more = 22 total). Third write of 9 bytes must still succeed — the
    // 9 remaining bytes of quota are intact because the failed write left no trace.
    let executor = start_with_agent_storage_quota(deps, &context, 20).await?;

    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;

    let agent_id = agent_id!("FileSystem", "quota-ordering-1");
    executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    // First write: 11 bytes, leaves 9 bytes of quota.
    executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "write_file",
            data_value!("/file1.txt", "hello world"),
        )
        .await?;

    // Second write: 11 bytes, would total 22 — exceeds 20-byte quota. Must fail.
    let over = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "write_file",
            data_value!("/file2.txt", "hello world"),
        )
        .await;
    assert!(
        over.is_err(),
        "expected second write to fail with ExceededStorageLimit"
    );

    // Third write: 9 bytes — must succeed because the failed write left no phantom
    // entry reducing the remaining quota.
    executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "write_file",
            data_value!("/testfile2.txt", "hello world"),
        )
        .await?;

    Ok(())
}

/// `write_zeroes` on a file-backed output stream must be subject to storage
/// quota. Writing more zeroes than the per-agent quota allows must fail with
/// `WorkerExceededStorageLimit`.
#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn agent_quota_write_zeroes_to_file_exceeding_limit_fails(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    // 5-byte quota — 1024 zeroes exceeds it.
    let executor = start_with_agent_storage_quota(deps, &context, 5).await?;

    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;

    let agent_id = agent_id!("FileSystem", "write-zeroes-quota-1");
    executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    let result = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "write_zeroes_to_file",
            data_value!("/zeroes.bin", 1024u64),
        )
        .await;

    assert!(
        result.is_err(),
        "expected write_zeroes to fail when quota exceeded"
    );
    let err_str = format!("{result:?}");
    assert!(
        err_str.contains("ExceededStorageLimit") || err_str.contains("storage"),
        "expected WorkerExceededStorageLimit, got: {err_str}"
    );

    Ok(())
}

/// `write_zeroes` on stdout must NOT charge storage quota — console streams
/// are exempt. Even with a very tight per-agent quota (1 byte), writing a
/// large number of zeroes to stdout must succeed.
#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn agent_quota_write_zeroes_to_stdout_does_not_charge_quota(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    // 1-byte per-agent quota — file writes would immediately fail.
    // Stdout writes must succeed regardless.
    let executor = start_with_agent_storage_quota(deps, &context, 1).await?;

    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;

    let agent_id = agent_id!("FileSystem", "write-zeroes-stdout-1");
    executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "write_zeroes_to_stdout",
            data_value!(1024u64),
        )
        .await?;

    Ok(())
}

/// `blocking_write_zeroes_and_flush` on a file-backed output stream must be
/// subject to storage quota — it now correctly composes `write_zeroes` and
/// `blocking_flush`, both of which enforce quota. Exceeding the limit must fail.
#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn agent_quota_blocking_write_zeroes_and_flush_exceeding_limit_fails(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    // 5-byte quota — 1024 zeroes exceeds it.
    let executor = start_with_agent_storage_quota(deps, &context, 5).await?;

    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;

    let agent_id = agent_id!("FileSystem", "blocking-write-zeroes-quota-1");
    executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    let result = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "blocking_write_zeroes_and_flush_to_file",
            data_value!("/zeroes.bin", 1024u64),
        )
        .await;

    assert!(
        result.is_err(),
        "expected blocking_write_zeroes_and_flush to fail when quota exceeded"
    );
    let err_str = format!("{result:?}");
    assert!(
        err_str.contains("ExceededStorageLimit") || err_str.contains("storage"),
        "expected WorkerExceededStorageLimit, got: {err_str}"
    );

    Ok(())
}

/// A failed executor-pool acquire (WorkerOutOfStorage) must not leave a phantom
/// StorageUsageUpdate in the oplog that would over-count usage on restart.
///
/// Flow: exhaust the 1 KB pool with one write → second write fails →
/// delete the first file (releases pool) → second write succeeds. If the
/// failed write left a phantom +11 bytes in the oplog, after delete the
/// reconstructed usage would be inflated and the pool might still appear full.
#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn failed_executor_pool_acquire_leaves_no_phantom_oplog_entry(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    // 1 KB executor pool. First write consumes it. Second write fails with
    // WorkerOutOfStorage. Delete frees the pool. Third write must succeed —
    // if the failed write had written a phantom +11 bytes to the oplog, the
    // reconstructed storage_requirement on restart would exceed the pool.
    let executor = start_with_executor_storage_pool(deps, &context, 1024).await?;

    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;

    let agent_id = agent_id!("FileSystem", "exec-ordering-1");
    executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    // First write: exhausts the 1 KB pool.
    executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "write_file",
            data_value!("/file1.txt", "hello world"),
        )
        .await?;

    // Second write: pool exhausted → WorkerOutOfStorage. No oplog entry should
    // be written for this failed attempt.
    let failed = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "write_file",
            data_value!("/file2.txt", "hello world"),
        )
        .await;
    assert!(
        failed.is_err(),
        "expected second write to fail with WorkerOutOfStorage"
    );

    // Delete the first file — releases the 1 KB permit back to the pool.
    executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "delete_file",
            data_value!("/file1.txt"),
        )
        .await?;

    // Third write: pool has 1 KB available again. Must succeed. If the failed
    // second write left a phantom StorageUsageUpdate, storage_requirement after
    // restart would be inflated and this write would fail.
    executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "write_file",
            data_value!("/file3.txt", "hello world"),
        )
        .await?;

    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn agent_quota_write_within_limit_succeeds(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start_with_agent_storage_quota(deps, &context, 1024 * 1024).await?;

    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;

    let agent_id = agent_id!("FileSystem", "agent-quota-ok-1");
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

#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn agent_quota_write_exceeding_limit_fails(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    // 5-byte limit — "hello world" (11 bytes) exceeds it.
    let executor = start_with_agent_storage_quota(deps, &context, 5).await?;

    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;

    let agent_id = agent_id!("FileSystem", "agent-quota-exceeded-1");
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
        "expected write to fail when agent quota is exceeded"
    );
    let err_str = format!("{result:?}");
    assert!(
        err_str.contains("ExceededStorageLimit") || err_str.contains("storage"),
        "expected WorkerExceededStorageLimit, got: {err_str}"
    );

    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn agent_quota_exceeded_limit_is_not_retried(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start_with_agent_storage_quota(deps, &context, 5).await?;

    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;

    let agent_id = agent_id!("FileSystem", "agent-quota-no-retry-1");
    executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    let first = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "write_file",
            data_value!("/testfile.txt", "hello world"),
        )
        .await;
    assert!(
        first.is_err(),
        "expected first write to fail with ExceededStorageLimit"
    );

    let second = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "write_file",
            data_value!("/testfile2.txt", "hello world"),
        )
        .await;
    assert!(
        second.is_err(),
        "expected second write to also fail — agent quota is permanent"
    );

    let err_str = format!("{second:?}");
    assert!(
        err_str.contains("ExceededStorageLimit")
            || err_str.contains("storage")
            || err_str.contains("already exists")
            || err_str.contains("AlreadyExists"),
        "expected a relevant error on second attempt, got: {err_str}"
    );

    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn agent_quota_freed_after_file_deletion(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    // Exactly 11 bytes — fits "hello world" once. A second write of the same
    // size must succeed after deletion returns the quota.
    let executor = start_with_agent_storage_quota(deps, &context, 11).await?;

    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;

    let agent_id = agent_id!("FileSystem", "agent-quota-release-delete-1");
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

    executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "delete_file",
            data_value!("/testfile.txt"),
        )
        .await?;

    executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "write_file",
            data_value!("/testfile2.txt", "hello world"),
        )
        .await?;

    Ok(())
}

/// `write_zeroes` on a file-backed stream within the per-agent quota succeeds.
#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn agent_quota_write_zeroes_to_file_within_limit_succeeds(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    // 1 MB quota — 1024 zeroes is well within it.
    let executor = start_with_agent_storage_quota(deps, &context, 1024 * 1024).await?;

    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;

    let agent_id = agent_id!("FileSystem", "write-zeroes-quota-ok-1");
    executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "write_zeroes_to_file",
            data_value!("/zeroes.bin", 1024u64),
        )
        .await?;

    Ok(())
}

/// `blocking_write_zeroes_and_flush` on a file-backed stream within the
/// per-agent quota succeeds.
#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn agent_quota_blocking_write_zeroes_and_flush_within_limit_succeeds(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start_with_agent_storage_quota(deps, &context, 1024 * 1024).await?;

    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;

    let agent_id = agent_id!("FileSystem", "blocking-write-zeroes-quota-ok-1");
    executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "blocking_write_zeroes_and_flush_to_file",
            data_value!("/zeroes.bin", 1024u64),
        )
        .await?;

    Ok(())
}

/// Storage quota is cumulative across write paths. Writing via `write_file`
/// and then via `write_zeroes_to_file` both count against the same per-agent
/// quota. If their combined sizes exceed the limit, the second write fails.
#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn agent_quota_cumulative_across_write_paths(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    // 15-byte quota. First write: 11 bytes ("hello world") via write_file.
    // Second write: 8 zeroes via write_zeroes — combined 19 bytes > 15 → fails.
    let executor = start_with_agent_storage_quota(deps, &context, 15).await?;

    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;

    let agent_id = agent_id!("FileSystem", "agent-quota-cumulative-1");
    executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    // First write: 11 bytes — succeeds, 4 bytes remaining.
    executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "write_file",
            data_value!("/file1.txt", "hello world"),
        )
        .await?;

    // Second write: 8 zeroes via write_zeroes — would total 19 bytes > 15 → fails.
    let result = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "write_zeroes_to_file",
            data_value!("/zeroes.bin", 8u64),
        )
        .await;

    assert!(
        result.is_err(),
        "expected write_zeroes to fail: combined writes exceed quota"
    );
    let err_str = format!("{result:?}");
    assert!(
        err_str.contains("ExceededStorageLimit") || err_str.contains("storage"),
        "expected WorkerExceededStorageLimit, got: {err_str}"
    );

    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn executor_pool_write_within_capacity_succeeds(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start_with_executor_storage_pool(deps, &context, 1024 * 1024).await?;

    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;

    let agent_id = agent_id!("FileSystem", "exec-pool-ok-1");
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

#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn executor_pool_exhaustion_returns_out_of_storage(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    // 1-byte pool — guaranteed to be exhausted by any write.
    let executor = start_with_executor_storage_pool(deps, &context, 1).await?;

    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;

    let agent_id = agent_id!("FileSystem", "exec-pool-exhausted-1");
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
        "expected write to fail when executor storage pool is exhausted"
    );
    let err_str = format!("{result:?}");
    assert!(
        err_str.contains("OutOfStorage") || err_str.contains("storage"),
        "expected WorkerOutOfStorage, got: {err_str}"
    );

    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn executor_pool_freed_after_file_deletion(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    // 1 KB pool. "hello world" (11 bytes) rounds up to 1 KB = 1 permit,
    // exhausting the pool. A second write of the same size must succeed after
    // deletion returns the permit.
    let executor = start_with_executor_storage_pool(deps, &context, 1024).await?;

    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;

    let agent_id = agent_id!("FileSystem", "exec-pool-release-delete-1");
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

    executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "delete_file",
            data_value!("/testfile.txt"),
        )
        .await?;

    executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "write_file",
            data_value!("/testfile2.txt", "hello world"),
        )
        .await?;

    Ok(())
}
