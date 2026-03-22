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
use golem_wasm::Value;
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
        err_str.contains("AgentExceededFilesystemStorageLimit") || err_str.contains("storage"),
        "expected AgentExceededFilesystemStorageLimit, got: {err_str}"
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
        "expected first write to fail with AgentExceededFilesystemStorageLimit"
    );

    let second = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "write_file",
            data_value!("/testfile2.txt", "hello world 2"),
        )
        .await;
    assert!(
        second.is_err(),
        "expected second write to also fail — agent quota is permanent"
    );

    let err_str = format!("{second:?}");
    assert!(
        err_str.contains("AgentExceededFilesystemStorageLimit")
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

    // Write a second file with distinct content within the 11-byte quota.
    // "hi world!!" is 10 bytes
    executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "write_file",
            data_value!("/testfile2.txt", "hi world!!"),
        )
        .await?;

    let content = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "read_file",
            data_value!("/testfile2.txt"),
        )
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow::anyhow!("expected return value from read_file"))?;
    assert_eq!(
        content,
        Value::Result(Ok(Some(Box::new(Value::String("hi world!!".to_string())))))
    );

    Ok(())
}

/// Rewriting the same file with the same content via `write_file` should not
/// consume additional quota, because the resulting file size does not grow.
#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn agent_quota_overwrite_same_file_should_not_double_charge(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    // Exactly 11 bytes: enough for one "hello world" payload. Overwrite of the
    // same file with the same payload must also succeed.
    let executor = start_with_agent_storage_quota(deps, &context, 11).await?;

    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;

    let agent_id = agent_id!("FileSystem", "agent-quota-overwrite-same-file-1");
    executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "write_file",
            data_value!("/same.txt", "hello world"),
        )
        .await?;

    executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "write_file",
            data_value!("/same.txt", "hello world"),
        )
        .await?;

    Ok(())
}

/// Stream-based writes on a file-backed output stream must be subject to
/// storage quota. Writing more bytes than the per-agent quota allows must fail with
/// `AgentExceededFilesystemStorageLimit`.
#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn agent_quota_stream_write_exceeding_limit_fails(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    // 5-byte quota — writing 1024 bytes via stream exceeds it.
    let executor = start_with_agent_storage_quota(deps, &context, 5).await?;

    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;

    let agent_id = agent_id!("FileSystem", "stream-write-quota-1");
    executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    let result = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "stream_to_file",
            data_value!("/stream.bin", 1024u64),
        )
        .await;

    assert!(
        result.is_err(),
        "expected stream_to_file to fail when quota exceeded"
    );
    let err_str = format!("{result:?}");
    assert!(
        err_str.contains("AgentExceededFilesystemStorageLimit") || err_str.contains("storage"),
        "expected AgentExceededFilesystemStorageLimit, got: {err_str}"
    );

    Ok(())
}

/// Stream-based writes within the per-agent quota succeed.
#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn agent_quota_stream_write_within_limit_succeeds(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    // 1 MB quota — writing 1024 bytes via stream is within it.
    let executor = start_with_agent_storage_quota(deps, &context, 1024 * 1024).await?;

    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;

    let agent_id = agent_id!("FileSystem", "stream-write-quota-ok-1");
    executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "stream_to_file",
            data_value!("/stream.bin", 1024u64),
        )
        .await?;

    Ok(())
}

/// A failed stream-write attempt that exceeds stream permit (`check_write`)
/// must roll back any pre-reserved executor pool allocation so another worker
/// can still allocate and write.
#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn executor_pool_stream_write_failed_attempt_does_not_leak_pool_permits(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    // 2 MiB executor pool so the failing worker can reserve 2 MiB first.
    let executor = start_with_executor_storage_pool(deps, &context, 2 * 1024 * 1024).await?;

    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;

    let failing_agent_id = agent_id!("FileSystem", "stream-write-failed-no-leak-a-1");
    executor
        .start_agent(&component.id, failing_agent_id.clone())
        .await?;

    let failed = executor
        .invoke_and_await_agent(
            &component,
            &failing_agent_id,
            "stream_to_file",
            data_value!("/big.bin", 2 * 1024 * 1024u64),
        )
        .await;
    assert!(failed.is_err(), "expected oversized stream_to_file to fail");

    // A second worker should still be able to allocate and write if the first
    // worker's failed attempt released pool permits.
    let succeeding_agent_id = agent_id!("FileSystem", "stream-write-failed-no-leak-b-1");
    executor
        .start_agent(&component.id, succeeding_agent_id.clone())
        .await?;

    executor
        .invoke_and_await_agent(
            &component,
            &succeeding_agent_id,
            "stream_to_file",
            data_value!("/small.bin", 1024u64),
        )
        .await?;

    Ok(())
}

/// Overwriting the same file with `stream_to_file` must not double-charge
/// quota when the file does not grow.
#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn agent_quota_stream_overwrite_same_file_should_not_double_charge(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start_with_agent_storage_quota(deps, &context, 1024).await?;

    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;

    let agent_id = agent_id!("FileSystem", "stream-overwrite-same-file-1");
    executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "stream_to_file",
            data_value!("/same-stream.bin", 1024u64),
        )
        .await?;

    executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "stream_to_file",
            data_value!("/same-stream.bin", 1024u64),
        )
        .await?;

    Ok(())
}

/// Stream writes on stdout must NOT charge storage quota — console streams
/// are exempt. Even with a very tight per-agent quota (1 byte), a large
/// stream write to stdout must succeed.
#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn agent_quota_stream_to_stdout_does_not_charge_quota(
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

    let agent_id = agent_id!("FileSystem", "stream-stdout-1");
    executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "stream_to_stdout",
            data_value!(1024u64),
        )
        .await?;

    Ok(())
}

/// `blocking_stream_and_flush_to_file` on a file-backed output stream must be
/// subject to storage quota. Exceeding the limit must fail.
#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn agent_quota_blocking_stream_and_flush_exceeding_limit_fails(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    // 5-byte quota — writing 1024 bytes via stream exceeds it.
    let executor = start_with_agent_storage_quota(deps, &context, 5).await?;

    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;

    let agent_id = agent_id!("FileSystem", "blocking-stream-write-quota-1");
    executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    let result = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "blocking_stream_and_flush_to_file",
            data_value!("/stream.bin", 1024u64),
        )
        .await;

    assert!(
        result.is_err(),
        "expected blocking_stream_and_flush_to_file to fail when quota exceeded"
    );
    let err_str = format!("{result:?}");
    assert!(
        err_str.contains("AgentExceededFilesystemStorageLimit") || err_str.contains("storage"),
        "expected AgentExceededFilesystemStorageLimit, got: {err_str}"
    );

    Ok(())
}

/// `blocking_stream_and_flush_to_file` on a file-backed stream within the
/// per-agent quota succeeds.
#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn agent_quota_blocking_stream_and_flush_within_limit_succeeds(
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

    let agent_id = agent_id!("FileSystem", "blocking-stream-write-quota-ok-1");
    executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "blocking_stream_and_flush_to_file",
            data_value!("/stream.bin", 1024u64),
        )
        .await?;

    Ok(())
}

/// `set_file_size` growing a file beyond the per-agent quota must fail with
/// `AgentExceededFilesystemStorageLimit`. Only the delta (new_size − current_size) is
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
        err_str.contains("AgentExceededFilesystemStorageLimit") || err_str.contains("storage"),
        "expected AgentExceededFilesystemStorageLimit, got: {err_str}"
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
/// Writing beyond the quota must fail with `AgentExceededFilesystemStorageLimit`.
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
        err_str.contains("AgentExceededFilesystemStorageLimit") || err_str.contains("storage"),
        "expected AgentExceededFilesystemStorageLimit, got: {err_str}"
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

    let content = executor
        .invoke_and_await_agent(&component, &agent_id, "read_file", data_value!("/file.txt"))
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow::anyhow!("expected return value from read_file"))?;

    assert_eq!(
        content,
        Value::Result(Ok(Some(Box::new(Value::String("hello world".to_string())))))
    );

    Ok(())
}

/// Overwriting the same byte range via direct `descriptor::write` (`pwrite_file`)
/// must not consume additional quota when file size does not grow.
#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn agent_quota_pwrite_overwrite_same_range_should_not_double_charge(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    // Exactly 11 bytes: enough for one "hello world" payload.
    let executor = start_with_agent_storage_quota(deps, &context, 11).await?;

    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;

    let agent_id = agent_id!("FileSystem", "agent-quota-pwrite-overwrite-1");
    executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "pwrite_file",
            data_value!("/pwrite.txt", 0u64, "hello world"),
        )
        .await?;

    // Same offset and same payload: logical size remains 11 bytes.
    executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "pwrite_file",
            data_value!("/pwrite.txt", 0u64, "hello world"),
        )
        .await?;

    Ok(())
}

/// Storage quota is cumulative across write paths. Writing via `write_file`
/// and then via `stream_to_file` both count against the same per-agent
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
    // Second write: 8 bytes via stream_to_file — combined 19 bytes > 15 → fails.
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

    // Second write: 8 bytes via stream API — would total 19 bytes > 15 → fails.
    let result = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "stream_to_file",
            data_value!("/stream.bin", 8u64),
        )
        .await;

    assert!(
        result.is_err(),
        "expected stream_to_file to fail: combined writes exceed quota"
    );
    let err_str = format!("{result:?}");
    assert!(
        err_str.contains("AgentExceededFilesystemStorageLimit") || err_str.contains("storage"),
        "expected AgentExceededFilesystemStorageLimit, got: {err_str}"
    );

    Ok(())
}

/// Account-quota level lifecycle: write → delete → suspend → restart → write → verify quota.
///
/// Same sequence as `executor_pool_storage_usage_survives_restart` but using
/// the per-agent plan limit. Verifies that `current_filesystem_storage_usage` stays
/// accurate at the account-quota layer across the suspend/restart cycle.
///
/// Quota = 22 bytes. Each 11-byte "hello world" write is counted exactly.
///
/// 1. Write file-1 (11 bytes) → usage: 11, remaining: 11.
/// 2. Write file-2 (11 bytes) → quota exhausted: usage: 22, remaining: 0.
/// 3. Delete file-1 → usage: 11, remaining: 11.
/// 4. Interrupt → worker unloaded from memory.
/// 5. Re-invoke → restart reconstructs current_filesystem_storage_usage = 11 bytes.
/// 6. Write file-3 (11 bytes) → succeeds (11 bytes remaining in quota).
/// 7. Write file-4 (11 bytes) → fails with `AgentExceededFilesystemStorageLimit`
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
    // 32-byte quota: fits exactly two 16-byte writes ("unique-content-N").
    let executor = start_with_agent_storage_quota(deps, &context, 32).await?;

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
            data_value!("/file-1.txt", "unique-content-1"),
        )
        .await?;

    executor
        .invoke_and_await_agent(
            &component,
            &agent,
            "write_file",
            data_value!("/file-2.txt", "unique-content-2"),
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

    // Restart reconstructs current_filesystem_storage_usage = 16 bytes (+16 +16 -16).
    // Verify file-2 has its distinct content — confirms the right file survived.
    // file-1 was deleted; reading file-1 would return an error, not file-2's content.
    let content = executor
        .invoke_and_await_agent(&component, &agent, "read_file", data_value!("/file-2.txt"))
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow::anyhow!("expected return value from read_file"))?;
    assert_eq!(
        content,
        Value::Result(Ok(Some(Box::new(Value::String(
            "unique-content-2".to_string()
        )))))
    );

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
        err_str.contains("AgentExceededFilesystemStorageLimit") || err_str.contains("storage"),
        "expected AgentExceededFilesystemStorageLimit, got: {err_str}"
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

/// A failed executor-pool acquire (`NodeOutOfFilesystemStorage`) must not leave a
/// phantom `FilesystemStorageUsageUpdate` in the oplog.
///
/// `FilesystemStorageUsageUpdate` is written to the oplog only AFTER `acquire_filesystem_space`
/// succeeds. If a transient failure during the `ReacquirePermits` retry loop
/// wrote a phantom entry, `current_filesystem_storage_usage` would be inflated on restart
/// and the pool pre-acquire would consume more than the actual files on disk.
///
/// Pool = 3 KB (no agent quota so only `NodeOutOfFilesystemStorage` can fire).
///
/// 1. Write file1 (1 KB) → usage: 1 KB, pool: 2 KB free.
/// 2. Write file2 (2 KB) → usage: 3 KB, pool: 0 KB.
///    May transiently hit NodeOutOfFilesystemStorage and retry — no phantom must be written.
/// 3. Delete file1 → usage: 2 KB, pool: 1 KB free.
/// 4. Interrupt → RunningWorker drops → pool: 3 KB free.
/// 5. Restart → pre-acquires `current_filesystem_storage_usage = 2 KB` from oplog → pool: 1 KB free.
/// 6. Write file3 (1 KB) → must succeed.
///    If phantom entries inflated usage to > 2 KB, pre-acquire would leave < 1 KB
///    free and file3 would fail.
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
    // 3 KB pool, no per-agent quota. Only NodeOutOfFilesystemStorage (retriable) can fire.
    let executor = start_with_executor_storage_pool(deps, &context, 3 * 1024).await?;

    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;

    let agent_id = agent_id!("FileSystem", "exec-ordering-1");
    let worker = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    let content_1kb = "A".repeat(1024);
    let content_2kb = "B".repeat(2 * 1024);
    let content_file3 = "C".repeat(1024);

    // Step 1: write 1 KB → pool: 2 KB free, usage: 1 KB.
    executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "write_file",
            data_value!("/file1.txt", content_1kb.as_str()),
        )
        .await?;

    // Step 2: write 2 KB → pool: 0 KB, usage: 3 KB.
    executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "write_file",
            data_value!("/file2.txt", content_2kb.as_str()),
        )
        .await?;

    // Step 3: delete file1 → usage drops to 2 KB, pool: 1 KB free.
    executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "delete_file",
            data_value!("/file1.txt"),
        )
        .await?;

    // Step 4: interrupt → RunningWorker drops, pool: 3 KB free.
    executor.interrupt(&worker).await?;

    // Step 5+6: restart pre-acquires 2 KB (file2 only, no phantom from transient
    // failures) → pool: 1 KB free. Write file3 (1 KB) → must succeed.
    executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "write_file",
            data_value!("/file3.txt", content_file3.as_str()),
        )
        .await?;

    // Verify file2 and file3 content
    let read2 = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "read_file",
            data_value!("/file2.txt"),
        )
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow::anyhow!("expected return value"))?;
    assert_eq!(
        read2,
        Value::Result(Ok(Some(Box::new(Value::String(content_2kb)))))
    );

    let read3 = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "read_file",
            data_value!("/file3.txt"),
        )
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow::anyhow!("expected return value"))?;
    assert_eq!(
        read3,
        Value::Result(Ok(Some(Box::new(Value::String(content_file3)))))
    );

    Ok(())
}

/// When the executor storage pool is full, a worker restart pre-acquires
/// storage via the blocking path which evicts the oldest idle worker, freeing
/// its permits so the restarting worker can proceed.
///
/// Flow (1 KB pool, each 11-byte write rounds up to 1 KB = 1 permit):
/// 1. Worker A writes 1 KB, then is interrupted → permit released.
/// 2. Worker B writes 1 KB (pool free after A's interrupt), then is interrupted.
/// 3. Worker A re-invoked → restarts with filesystem_storage_requirement = 1 KB
///    → blocking acquire_storage succeeds → A holds the 1 KB permit, now idle.
/// 4. Worker B re-invoked → restarts with filesystem_storage_requirement = 1 KB
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
    // 4 KB pool = 4 permits. Each worker writes a 2 KB file (2 permits).
    // Using >1 KB content exercises multi-permit eviction — verifies that
    // try_free_up_storage frees enough bytes (not just 1 permit minimum).
    let executor = start_with_executor_storage_pool(deps, &context, 4 * 1024).await?;

    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;

    let agent_a = agent_id!("FileSystem", "eviction-a-1");
    let agent_b = agent_id!("FileSystem", "eviction-b-1");

    // 2 KB content strings (> 1 KB so each write consumes 2 permits).
    let content_a = "A".repeat(2048);
    let content_b = "B".repeat(2048);

    // Step 1: Worker A writes 2 KB, then is interrupted to release its 2 permits.
    let worker_a = executor.start_agent(&component.id, agent_a.clone()).await?;
    executor
        .invoke_and_await_agent(
            &component,
            &agent_a,
            "write_file",
            data_value!("/file-a.txt", content_a.as_str()),
        )
        .await?;
    executor.interrupt(&worker_a).await?;

    // Step 2: Worker B writes 2 KB (pool is free after A's interrupt), then
    // is interrupted so its 2 permits are released.
    let worker_b = executor.start_agent(&component.id, agent_b.clone()).await?;
    executor
        .invoke_and_await_agent(
            &component,
            &agent_b,
            "write_file",
            data_value!("/file-b.txt", content_b.as_str()),
        )
        .await?;
    executor.interrupt(&worker_b).await?;

    // Step 3: Re-invoke Worker A. Restarts with filesystem_storage_requirement = 2 KB.
    // Pool has 4 KB free → blocking acquire_storage acquires 2 permits.
    // Worker A reads file and becomes idle, holding 2 permits.
    let read_a = executor
        .invoke_and_await_agent(
            &component,
            &agent_a,
            "read_file",
            data_value!("/file-a.txt"),
        )
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow::anyhow!("expected return value from read_file"))?;
    assert_eq!(
        read_a,
        Value::Result(Ok(Some(Box::new(Value::String(content_a.clone())))))
    );

    // Step 4: Re-invoke Worker B. Restarts with filesystem_storage_requirement = 2 KB.
    // Pool has 2 KB free (Worker A holds 2) — not enough for B's 2 KB requirement.
    // Blocking acquire_storage calls try_free_up_storage → evicts idle Worker A
    // (freeing 2 permits) → Worker B acquires its 2 permits and reads successfully.
    let read_b = executor
        .invoke_and_await_agent(
            &component,
            &agent_b,
            "read_file",
            data_value!("/file-b.txt"),
        )
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow::anyhow!("expected return value from read_file"))?;
    assert_eq!(
        read_b,
        Value::Result(Ok(Some(Box::new(Value::String(content_b.clone())))))
    );

    Ok(())
}

/// When the executor pool is exhausted by an idle worker, a second worker's
/// first-time write triggers eviction of the idle worker via `desired_extra_filesystem_storage`,
/// freeing enough permits for the write to succeed.
///
/// Worker A writes 2 KB and goes idle (holding 2 permits). Worker B tries to
/// write 2 KB — pool is exhausted → `NodeOutOfFilesystemStorage` → `ReacquirePermits`.
/// `desired_extra_filesystem_storage` is set to 2 KB so the blocking `acquire_storage` in
/// the restart path requests 2 KB from `try_free_up_storage`, which evicts idle
/// Worker A (freeing 2 permits). Worker B then succeeds.
///
/// Uses >1 KB content to verify that `desired_extra_filesystem_storage` correctly drives
/// multi-permit eviction rather than just the 1-permit minimum.
#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn executor_pool_idle_worker_evicted_on_first_write_node_out_of_filesystem_storage(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    // 2 KB pool = 2 permits. Worker A exhausts it with a 2 KB write.
    // Worker B must evict A to write its own 2 KB file.
    let executor = start_with_executor_storage_pool(deps, &context, 2 * 1024).await?;

    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;

    let agent_a = agent_id!("FileSystem", "exec-pool-exhausted-a-1");
    let agent_b = agent_id!("FileSystem", "exec-pool-exhausted-b-1");

    // 2 KB content (> 1 KB so each write consumes 2 permits, not just 1).
    let content_a = "A".repeat(2048);
    let content_b = "B".repeat(2048);

    // Worker A writes 2 KB and goes idle — pool exhausted (2/2 permits held).
    executor.start_agent(&component.id, agent_a.clone()).await?;
    executor
        .invoke_and_await_agent(
            &component,
            &agent_a,
            "write_file",
            data_value!("/file-a.txt", content_a.as_str()),
        )
        .await?;

    // Worker B writes 2 KB. Pool is full → NodeOutOfFilesystemStorage → desired_extra_filesystem_storage
    // set to 2 KB → ReacquirePermits → blocking acquire_storage requests 2 KB
    // → try_free_up_storage evicts idle Worker A (freeing 2 permits)
    // → Worker B acquires 2 permits and its write succeeds.
    executor.start_agent(&component.id, agent_b.clone()).await?;
    executor
        .invoke_and_await_agent(
            &component,
            &agent_b,
            "write_file",
            data_value!("/file-b.txt", content_b.as_str()),
        )
        .await?;

    // Verify Worker B's file has the correct unique content.
    let content = executor
        .invoke_and_await_agent(
            &component,
            &agent_b,
            "read_file",
            data_value!("/file-b.txt"),
        )
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow::anyhow!("expected return value from read_file"))?;
    assert_eq!(
        content,
        Value::Result(Ok(Some(Box::new(Value::String(content_b)))))
    );

    Ok(())
}

/// Verifies that eviction only acquires the gap — i.e. only evicts as many
/// idle workers as needed to cover the missing portion of the requested bytes,
/// not the full amount.
///
/// Pool = 6 KB. Setup:
///   Worker A writes 2 KB → pool: 4 KB free.
///   Worker B writes 1 KB → pool: 3 KB free.
///   Worker C writes 3 KB → pool: 0 KB free. C goes idle.
///
/// C then tries to write an extra 2 KB. Pool is exhausted → NodeOutOfFilesystemStorage.
/// `desired_extra_filesystem_storage = 2 KB`. On `ReacquirePermits` restart:
///   - Old C RunningWorker drops → 3 KB returned to pool (pool: 3 KB free).
///   - `acquire_bytes = filesystem_storage_requirement(3KB) + desired_extra(2KB) = 5 KB`.
///   - Pool has 3 KB → gap = 2 KB → eviction targets 2 KB → evicts idle Worker A
///     (holding 2 KB) → pool: 3 + 2 = 5 KB free.
///   - Acquires 5 KB, releases 2 KB (desired_extra) → pool: 2 KB free.
///   - C holds 3 KB as filesystem_storage_permit (its existing files).
///   - C's pending 2 KB write re-acquires → succeeds.
///   - Worker B (1 KB) is NOT evicted — only the minimum gap was cleared.
///
/// After the test: B and C are both running with correct file contents.
#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn executor_pool_only_gap_evicted_not_full_amount(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    // 6 KB pool = 6 permits.
    let executor = start_with_executor_storage_pool(deps, &context, 6 * 1024).await?;

    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;

    let agent_a = agent_id!("FileSystem", "gap-evict-a-1");
    let agent_b = agent_id!("FileSystem", "gap-evict-b-1");
    let agent_c = agent_id!("FileSystem", "gap-evict-c-1");

    // Content strings sized to consume specific permit counts.
    let content_a = "A".repeat(2 * 1024); // 2 KB → 2 permits
    let content_b = "B".repeat(1024); // 1 KB → 1 permit
    let content_c1 = "C".repeat(3 * 1024); // 3 KB → 3 permits
    let content_c2 = "D".repeat(2 * 1024); // 2 KB → the extra write that fails

    // Worker A: write 2 KB → pool: 4 KB free.
    executor.start_agent(&component.id, agent_a.clone()).await?;
    executor
        .invoke_and_await_agent(
            &component,
            &agent_a,
            "write_file",
            data_value!("/file-a.txt", content_a.as_str()),
        )
        .await?;

    // Worker B: write 1 KB → pool: 3 KB free.
    let worker_b = executor.start_agent(&component.id, agent_b.clone()).await?;
    executor
        .invoke_and_await_agent(
            &component,
            &agent_b,
            "write_file",
            data_value!("/file-b.txt", content_b.as_str()),
        )
        .await?;

    // Worker C: write 3 KB → pool: 0 KB free. C goes idle.
    executor.start_agent(&component.id, agent_c.clone()).await?;
    executor
        .invoke_and_await_agent(
            &component,
            &agent_c,
            "write_file",
            data_value!("/file-c1.txt", content_c1.as_str()),
        )
        .await?;

    // Worker C tries to write 2 KB more. Pool is exhausted →
    // NodeOutOfFilesystemStorage → desired_extra_filesystem_storage = 2 KB → ReacquirePermits.
    // C's old RunningWorker drops returning 3 KB → pool: 3 KB.
    // acquire_bytes = 3+2 = 5 KB → gap = 2 KB → evicts idle Worker A (2 KB).
    // Pool after eviction: 5 KB → acquire 5 KB → release 2 KB → pool: 2 KB free.
    // C's pending 2 KB write re-acquires → succeeds.
    // Worker B (1 KB) must NOT be evicted.
    executor
        .invoke_and_await_agent(
            &component,
            &agent_c,
            "write_file",
            data_value!("/file-c2.txt", content_c2.as_str()),
        )
        .await?;

    // Verify Worker B was NOT evicted — it should still be idle in memory.
    // We check its status is Idle (loaded, not suspended/failed) before reading,
    // proving eviction stopped after freeing only the gap (2 KB from A),
    // not B's 1 KB as well.
    let metadata_b = executor.get_worker_metadata(&worker_b).await?;
    assert_eq!(
        metadata_b.status,
        golem_common::model::AgentStatus::Idle,
        "Worker B must remain Idle (not evicted) — only Worker A's 2 KB should have been evicted"
    );

    // Verify Worker B's file content is intact.
    let read_b = executor
        .invoke_and_await_agent(
            &component,
            &agent_b,
            "read_file",
            data_value!("/file-b.txt"),
        )
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow::anyhow!("expected return value from read_file"))?;
    assert_eq!(
        read_b,
        Value::Result(Ok(Some(Box::new(Value::String(content_b)))))
    );

    // Verify Worker C's first file — confirms prior state survived the eviction/restart.
    let read_c1 = executor
        .invoke_and_await_agent(
            &component,
            &agent_c,
            "read_file",
            data_value!("/file-c1.txt"),
        )
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow::anyhow!("expected return value from read_file"))?;
    assert_eq!(
        read_c1,
        Value::Result(Ok(Some(Box::new(Value::String(content_c1)))))
    );

    // Verify Worker C's second file — the write that triggered eviction.
    let read_c2 = executor
        .invoke_and_await_agent(
            &component,
            &agent_c,
            "read_file",
            data_value!("/file-c2.txt"),
        )
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow::anyhow!("expected return value from read_file"))?;
    assert_eq!(
        read_c2,
        Value::Result(Ok(Some(Box::new(Value::String(content_c2)))))
    );

    Ok(())
}

/// Full lifecycle: write → delete → interrupt → restart → verify pool accounting.
///
/// Verifies that `current_filesystem_storage_usage` is reconstructed correctly from the oplog
/// across an interrupt/restart cycle, and that the executor semaphore reflects the
/// reconstructed value accurately.
///
/// Pool = 2 KB (2 permits). Each 11-byte "hello world" write rounds up to 1 KB.
///
/// Worker A:
///   1. Writes file-1 (1 KB) → pool: 1 KB used.
///   2. Writes file-2 (1 KB) → pool: 2 KB used, exhausted.
///   3. Deletes file-1 → pool: 1 KB used. FilesystemStorageUsageUpdate(-1KB) written to oplog.
///   4. Interrupted → RunningWorker drops → 1 KB permit returned → pool: 1 KB free.
///
/// Worker B (different agent, same pool):
///   5. Writes 1 KB → should succeed because pool has 1 KB free.
///      If A's current_filesystem_storage_usage was wrong (e.g. 2 KB instead of 1 KB), A would
///      have consumed 2 KB on restart pre-acquire, leaving 0 KB free, and B would fail.
///
/// Worker A re-invoked:
///   6. Restart reconstructs current_filesystem_storage_usage = 1 KB from oplog (+1+1-1).
///      Pre-acquires 1 KB from the pool. Pool: 0 KB free.
///      Reads file-2 to confirm durable state is intact.
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
    // 2 KB pool.
    let executor = start_with_executor_storage_pool(deps, &context, 2 * 1024).await?;

    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;

    let agent_a = agent_id!("FileSystem", "lifecycle-a-1");
    let agent_b = agent_id!("FileSystem", "lifecycle-b-1");
    let worker_a = executor.start_agent(&component.id, agent_a.clone()).await?;

    // Step 1: Worker A writes file-1 → 1 KB used, 1 KB free.
    executor
        .invoke_and_await_agent(
            &component,
            &agent_a,
            "write_file",
            data_value!("/file-1.txt", "content-a-file1"),
        )
        .await?;

    // Step 2: Worker A writes file-2 → pool exhausted (2 KB used).
    executor
        .invoke_and_await_agent(
            &component,
            &agent_a,
            "write_file",
            data_value!("/file-2.txt", "content-a-file2"),
        )
        .await?;

    // Step 3: Worker A deletes file-1 → 1 KB freed. FilesystemStorageUsageUpdate(-1 KB) → oplog.
    executor
        .invoke_and_await_agent(
            &component,
            &agent_a,
            "delete_file",
            data_value!("/file-1.txt"),
        )
        .await?;

    // Step 4: Interrupt Worker A → RunningWorker drops → 1 KB permit returned.
    // Pool now: 1 KB free.
    executor.interrupt(&worker_a).await?;

    // Step 5: Worker B writes 1 KB. Pool has 1 KB free → must succeed.
    // If Worker A's current_filesystem_storage_usage was incorrectly reconstructed as 2 KB
    // (missing the delete delta), its restart would pre-acquire 2 KB leaving 0 KB
    // free, and Worker B's write would fail.
    executor.start_agent(&component.id, agent_b.clone()).await?;
    executor
        .invoke_and_await_agent(
            &component,
            &agent_b,
            "write_file",
            data_value!("/file-b.txt", "content-b-file1"),
        )
        .await?;

    // Verify Worker B's file has the correct unique content.
    let content_b = executor
        .invoke_and_await_agent(
            &component,
            &agent_b,
            "read_file",
            data_value!("/file-b.txt"),
        )
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow::anyhow!("expected return value from read_file"))?;
    assert_eq!(
        content_b,
        Value::Result(Ok(Some(Box::new(Value::String(
            "content-b-file1".to_string()
        )))))
    );

    // Step 6: Re-invoke Worker A. Restart reconstructs current_filesystem_storage_usage = 1 KB.
    // Reads file-2 — confirms durable state survived the interrupt with correct content.
    // file-1 was deleted; file-2 must still have its distinct original content.
    let content_a = executor
        .invoke_and_await_agent(
            &component,
            &agent_a,
            "read_file",
            data_value!("/file-2.txt"),
        )
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow::anyhow!("expected return value from read_file"))?;
    assert_eq!(
        content_a,
        Value::Result(Ok(Some(Box::new(Value::String(
            "content-a-file2".to_string()
        )))))
    );

    Ok(())
}
