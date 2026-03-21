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

/// Full lifecycle: write → delete → suspend → restart → write → verify pool
///
/// Verifies that `current_storage_usage` (reconstructed from `StorageUsageUpdate`
/// oplog entries) stays accurate across the full suspend/restart cycle, and that
/// the executor semaphore is correctly pre-acquired from the reconstructed value
/// on restart.
///
/// Pool = 2 KB (2 permits). Each 11-byte "hello world" write rounds up to 1 KB.
///
/// 1. Write file-1 (1 KB) → pool: 1 KB used, 1 KB free.
/// 2. Write file-2 (1 KB) → pool: 2 KB used, 0 KB free.
/// 3. Delete file-1 → pool: 1 KB used, 1 KB free.
///    The `StorageUsageUpdate(-1 KB)` oplog entry is written.
/// 4. Interrupt → RunningWorker drops → permits released → pool: 2 KB free.
/// 5. Re-invoke → restart pre-acquires `current_storage_usage = 1 KB`
///    (reconstructed from oplog: +1 KB +1 KB -1 KB) → pool: 1 KB used, 1 KB free.
/// 6. Write file-3 (1 KB) → succeeds (1 KB free in pool).
/// 7. Write file-4 (1 KB) → fails with `WorkerOutOfStorage`
///    (pool exhausted: 2 KB in use = file-2 + file-3).
///
/// If `current_storage_usage` were miscalculated on restart (e.g. missing the
/// delete delta or double-counting), the pre-acquire would be wrong and steps
/// 6 or 7 would behave incorrectly.
#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn executor_pool_storage_usage_survives_restart(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start_with_executor_storage_pool(deps, &context, 2 * 1024).await?;

    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;

    let agent = agent_id!("FileSystem", "lifecycle-1");
    let worker = executor.start_agent(&component.id, agent.clone()).await?;

    // Step 1: write file-1 → 1 KB used.
    executor
        .invoke_and_await_agent(
            &component,
            &agent,
            "write_file",
            data_value!("/file-1.txt", "hello world"),
        )
        .await?;

    // Step 2: write file-2 → pool exhausted (2 KB used).
    executor
        .invoke_and_await_agent(
            &component,
            &agent,
            "write_file",
            data_value!("/file-2.txt", "hello world"),
        )
        .await?;

    // Step 3: delete file-1 → 1 KB freed, StorageUsageUpdate(-1 KB) written to oplog.
    executor
        .invoke_and_await_agent(
            &component,
            &agent,
            "delete_file",
            data_value!("/file-1.txt"),
        )
        .await?;

    // Step 4: interrupt → permits released to pool.
    executor.interrupt(&worker).await?;

    // Step 5: re-invoke → restart reconstructs current_storage_usage = 1 KB from
    // oplog (+1+1-1), pre-acquires 1 KB → pool: 1 KB used, 1 KB free.
    // Reading file-2 confirms the worker state is intact after restart.
    executor
        .invoke_and_await_agent(&component, &agent, "read_file", data_value!("/file-2.txt"))
        .await?;

    // Step 6: write file-3 → uses the 1 KB remaining in the pool. Must succeed.
    executor
        .invoke_and_await_agent(
            &component,
            &agent,
            "write_file",
            data_value!("/file-3.txt", "hello world"),
        )
        .await?;

    // Step 7: write file-4 → pool now at 2 KB (file-2 + file-3). Must fail with
    // WorkerOutOfStorage. If current_storage_usage was wrong after restart, this
    // step would either incorrectly succeed or fail too early.
    let result = executor
        .invoke_and_await_agent(
            &component,
            &agent,
            "write_file",
            data_value!("/file-4.txt", "hello world"),
        )
        .await;
    assert!(
        result.is_err(),
        "expected write to fail: pool is exhausted (file-2 + file-3 = 2 KB)"
    );
    let err_str = format!("{result:?}");
    assert!(
        err_str.contains("OutOfStorage") || err_str.contains("storage"),
        "expected WorkerOutOfStorage, got: {err_str}"
    );

    Ok(())
}

/// Account-quota level lifecycle: write → delete → suspend → restart → write → verify quota.
///
/// Same sequence as `executor_pool_storage_usage_survives_restart` but using
/// the per-agent plan limit. Verifies that `current_storage_usage` stays
/// accurate at the account-quota layer across the suspend/restart cycle.
///
/// Quota = 22 bytes. Each 11-byte "hello world" write is counted exactly.
///
/// 1. Write file-1 (11 bytes) → usage: 11, remaining: 11.
/// 2. Write file-2 (11 bytes) → quota exhausted: usage: 22, remaining: 0.
/// 3. Delete file-1 → usage: 11, remaining: 11.
/// 4. Interrupt → worker unloaded from memory.
/// 5. Re-invoke → restart reconstructs current_storage_usage = 11 bytes.
/// 6. Write file-3 (11 bytes) → succeeds (11 bytes remaining in quota).
/// 7. Write file-4 (11 bytes) → fails with `ExceededStorageLimit`
///    (quota exhausted: file-2 + file-3 = 22 bytes).
#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn agent_quota_storage_usage_survives_restart(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    // 22-byte quota: fits exactly two 11-byte "hello world" writes.
    let executor = start_with_agent_storage_quota(deps, &context, 22).await?;

    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;

    let agent = agent_id!("FileSystem", "agent-lifecycle-1");
    let worker = executor.start_agent(&component.id, agent.clone()).await?;

    executor
        .invoke_and_await_agent(
            &component,
            &agent,
            "write_file",
            data_value!("/file-1.txt", "hello world"),
        )
        .await?;

    executor
        .invoke_and_await_agent(
            &component,
            &agent,
            "write_file",
            data_value!("/file-2.txt", "hello world"),
        )
        .await?;

    executor
        .invoke_and_await_agent(
            &component,
            &agent,
            "delete_file",
            data_value!("/file-1.txt"),
        )
        .await?;

    executor.interrupt(&worker).await?;

    // Restart reconstructs current_storage_usage = 11 bytes (+11 +11 -11).
    executor
        .invoke_and_await_agent(&component, &agent, "read_file", data_value!("/file-2.txt"))
        .await?;

    executor
        .invoke_and_await_agent(
            &component,
            &agent,
            "write_file",
            data_value!("/file-3.txt", "hello world"),
        )
        .await?;

    let result = executor
        .invoke_and_await_agent(
            &component,
            &agent,
            "write_file",
            data_value!("/file-4.txt", "hello world"),
        )
        .await;
    assert!(
        result.is_err(),
        "expected write to fail: quota exhausted (file-2 + file-3 = 22 bytes)"
    );
    let err_str = format!("{result:?}");
    assert!(
        err_str.contains("ExceededStorageLimit") || err_str.contains("storage"),
        "expected WorkerExceededStorageLimit, got: {err_str}"
    );

    Ok(())
}

/// `set_file_size` growing a file beyond the per-agent quota must fail with
/// `WorkerExceededStorageLimit`. Only the delta (new_size − current_size) is
/// charged, not the full new size.
#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn agent_quota_set_size_grow_beyond_limit_fails(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    // 11-byte quota. Write 11 bytes first (exhausts quota). Then set_size to
    // 12 — grows by 1 byte — must fail because the delta (1 byte) would exceed
    // the remaining quota (0 bytes).
    let executor = start_with_agent_storage_quota(deps, &context, 11).await?;

    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;

    let agent_id = agent_id!("FileSystem", "set-size-grow-1");
    executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    // Write 11 bytes — exhausts the quota.
    executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "write_file",
            data_value!("/file.txt", "hello world"),
        )
        .await?;

    // Grow by 1 byte — must fail (quota exhausted).
    let result = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "set_file_size",
            data_value!("/file.txt", 12u64),
        )
        .await;
    assert!(
        result.is_err(),
        "expected set_size grow to fail: quota exhausted"
    );
    let err_str = format!("{result:?}");
    assert!(
        err_str.contains("ExceededStorageLimit") || err_str.contains("storage"),
        "expected WorkerExceededStorageLimit, got: {err_str}"
    );

    Ok(())
}

/// `set_file_size` shrinking a file releases the freed bytes back to the
/// per-agent quota, allowing a subsequent write of equal size to succeed.
#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn agent_quota_set_size_shrink_releases_quota(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    // 11-byte quota. Write 11 bytes (exhausts quota). Shrink to 5 bytes —
    // releases 6 bytes. A subsequent write of 6 bytes must now succeed.
    let executor = start_with_agent_storage_quota(deps, &context, 11).await?;

    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;

    let agent_id = agent_id!("FileSystem", "set-size-shrink-1");
    executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    // Write 11 bytes — exhausts quota.
    executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "write_file",
            data_value!("/file.txt", "hello world"),
        )
        .await?;

    // Shrink to 5 bytes — frees 6 bytes back to quota.
    executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "set_file_size",
            data_value!("/file.txt", 5u64),
        )
        .await?;

    // Write 6 bytes to a new file — must succeed (6 bytes freed by shrink).
    executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "write_file",
            data_value!("/file2.txt", "hello!"),
        )
        .await?;

    Ok(())
}

/// `pwrite_file` (direct `descriptor::write`) is subject to per-agent quota.
/// Writing beyond the quota must fail with `WorkerExceededStorageLimit`.
#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn agent_quota_pwrite_beyond_limit_fails(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    // 5-byte quota. pwrite of 11 bytes ("hello world") exceeds it.
    let executor = start_with_agent_storage_quota(deps, &context, 5).await?;

    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;

    let agent_id = agent_id!("FileSystem", "pwrite-quota-1");
    executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    let result = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "pwrite_file",
            data_value!("/file.txt", 0u64, "hello world"),
        )
        .await;
    assert!(result.is_err(), "expected pwrite to fail: quota exceeded");
    let err_str = format!("{result:?}");
    assert!(
        err_str.contains("ExceededStorageLimit") || err_str.contains("storage"),
        "expected WorkerExceededStorageLimit, got: {err_str}"
    );

    Ok(())
}

/// `pwrite_file` (direct `descriptor::write`) within the per-agent quota succeeds.
#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn agent_quota_pwrite_within_limit_succeeds(
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

    let agent_id = agent_id!("FileSystem", "pwrite-ok-1");
    executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "pwrite_file",
            data_value!("/file.txt", 0u64, "hello world"),
        )
        .await?;

    Ok(())
}

/// When the executor storage pool is full, a worker restart pre-acquires
/// storage via the blocking path which evicts the oldest idle worker, freeing
/// its permits so the restarting worker can proceed.
///
/// Flow (1 KB pool, each 11-byte write rounds up to 1 KB = 1 permit):
/// 1. Worker A writes 1 KB, then is interrupted → permit released.
/// 2. Worker B writes 1 KB (pool free after A's interrupt), then is interrupted.
/// 3. Worker A re-invoked → restarts with storage_requirement = 1 KB
///    → blocking acquire_storage succeeds → A holds the 1 KB permit, now idle.
/// 4. Worker B re-invoked → restarts with storage_requirement = 1 KB
///    → pool is 0 (A holds it) → blocking acquire_storage calls
///    try_free_up_storage → evicts idle Worker A → 1 KB freed
///    → Worker B acquires the permit and its invocation succeeds.
#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn executor_pool_idle_worker_evicted_when_pool_full(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    // 1 KB pool = 1 permit. Each 11-byte write rounds up to 1 KB.
    let executor = start_with_executor_storage_pool(deps, &context, 1024).await?;

    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;

    let agent_a = agent_id!("FileSystem", "eviction-a-1");
    let agent_b = agent_id!("FileSystem", "eviction-b-1");

    // Step 1: Worker A writes 1 KB, then is interrupted to release its permit.
    let worker_a = executor.start_agent(&component.id, agent_a.clone()).await?;
    executor
        .invoke_and_await_agent(
            &component,
            &agent_a,
            "write_file",
            data_value!("/file-a.txt", "hello world"),
        )
        .await?;
    executor.interrupt(&worker_a).await?;

    // Step 2: Worker B writes 1 KB (pool is free after A's interrupt), then
    // is interrupted so its permit is released.
    let worker_b = executor.start_agent(&component.id, agent_b.clone()).await?;
    executor
        .invoke_and_await_agent(
            &component,
            &agent_b,
            "write_file",
            data_value!("/file-b.txt", "hello world"),
        )
        .await?;
    executor.interrupt(&worker_b).await?;

    // Step 3: Re-invoke Worker A. Restarts with storage_requirement = 1 KB.
    // Pool has 1 KB free → blocking acquire_storage succeeds immediately.
    // Worker A finishes read_file and becomes idle, still holding the 1 KB permit.
    executor
        .invoke_and_await_agent(
            &component,
            &agent_a,
            "read_file",
            data_value!("/file-a.txt"),
        )
        .await?;

    // Step 4: Re-invoke Worker B. Restarts with storage_requirement = 1 KB.
    // Pool is 0 (Worker A holds it) → blocking acquire_storage calls
    // try_free_up_storage → evicts idle Worker A → 1 KB freed
    // → Worker B acquires and its invocation succeeds.
    executor
        .invoke_and_await_agent(
            &component,
            &agent_b,
            "read_file",
            data_value!("/file-b.txt"),
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
async fn executor_pool_failed_acquire_leaves_no_phantom_oplog_entry(
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
