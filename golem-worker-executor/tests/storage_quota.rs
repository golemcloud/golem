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

// ── Per-agent (plan) quota ────────────────────────────────────────────────────

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
