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
use golem_common::base_model::component::ComponentDto;
use golem_common::model::OwnedAgentId;
use golem_common::model::agent::ParsedAgentId;
use golem_common::model::component::{AgentFilePermissions, CanonicalFilePath};
use golem_common::model::oplog::{OplogIndex, PublicOplogEntry, PublicOplogEntryWithIndex};
use golem_common::model::worker::{RevertToOplogIndex, RevertWorkerTarget};
use golem_common::schema::SchemaValue;
use golem_common::schema::schema_value::ResultValuePayload;
use golem_common::{agent_id, data_value};
use golem_test_framework::dsl::TestDsl;
use golem_test_framework::model::IFSEntry;
use golem_worker_executor::metrics::storage::{
    STORAGE_BYTES_DELETED_TOTAL, STORAGE_BYTES_WRITTEN_TOTAL, STORAGE_TYPE_FILESYSTEM,
};
use golem_worker_executor::worker::EvictionClass;
use golem_worker_executor_test_utils::{
    LastUniqueId, PrecompiledComponent, TestContext, TestWorkerExecutor,
    WorkerExecutorTestDependencies, start_with_agent_storage_quota,
    start_with_executor_storage_pool,
};
use std::path::PathBuf;
use test_r::{inherit_test_dep, test, timeout};

inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(LastUniqueId);
inherit_test_dep!(Tracing);
inherit_test_dep!(
    #[tagged_as("host_api_tests")]
    PrecompiledComponent
);
inherit_test_dep!(
    #[tagged_as("initial_file_system")]
    PrecompiledComponent
);

fn filesystem_storage_deltas(entries: &[PublicOplogEntryWithIndex]) -> Vec<i64> {
    entries
        .iter()
        .filter_map(|entry| match &entry.entry {
            PublicOplogEntry::FilesystemStorageUsageUpdate(params) => Some(params.delta),
            _ => None,
        })
        .collect()
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
    let worker_id = executor
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

    assert_eq!(
        filesystem_storage_deltas(&executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?),
        vec![11]
    );

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
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;
    let account_id = context.account_id.to_string();
    let environment_id = context.default_environment_id.to_string();
    let written_before = STORAGE_BYTES_WRITTEN_TOTAL
        .with_label_values(&[STORAGE_TYPE_FILESYSTEM, &account_id, &environment_id])
        .get();

    let result = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "write_file",
            data_value!("/testfile.txt", "hello world"),
        )
        .await;

    assert_specific_storage_quota_failure(&result, "write_file");
    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    assert_eq!(
        filesystem_storage_deltas(&oplog),
        Vec::<i64>::new(),
        "failed quota reservation must not persist any storage update"
    );
    let written_after = STORAGE_BYTES_WRITTEN_TOTAL
        .with_label_values(&[STORAGE_TYPE_FILESYSTEM, &account_id, &environment_id])
        .get();
    assert_eq!(
        written_after, written_before,
        "failed quota reservation must not record written-byte metrics"
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
    let worker_id = executor
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
    assert_specific_storage_quota_failure(&first, "first permanent quota failure");

    let second = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "write_file",
            data_value!("/testfile2.txt", "hello world 2"),
        )
        .await;
    assert_specific_storage_quota_failure(&second, "second permanent quota failure");
    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    assert!(filesystem_storage_deltas(&oplog).is_empty());

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
    let worker_id = executor
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
        SchemaValue::Result(ResultValuePayload::Ok {
            value: Some(Box::new(SchemaValue::String("hi world!!".to_string())))
        })
    );

    assert_eq!(
        filesystem_storage_deltas(&executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?),
        vec![11, -11, 10]
    );

    Ok(())
}

/// `std::fs::write` truncates before rewriting. Both physical transitions must
/// be recorded while the final canonical total remains unchanged.
#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn agent_quota_truncate_and_rewrite_has_no_net_double_charge(
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
    let worker_id = executor
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

    assert_eq!(
        filesystem_storage_deltas(&executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?),
        vec![11, -11, 11]
    );

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
    let worker_id = executor
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

    assert_specific_storage_quota_failure(&result, "stream_to_file");

    executor.check_oplog_is_queryable(&worker_id).await?;

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
    let worker_id = executor
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

    executor.check_oplog_is_queryable(&worker_id).await?;

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
    let failing_worker_id = executor
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
    let failed_oplog = executor
        .get_oplog(&failing_worker_id, OplogIndex::INITIAL)
        .await?;
    assert_eq!(
        filesystem_storage_deltas(&failed_oplog),
        Vec::<i64>::new(),
        "failed stream mutation must not persist any storage update"
    );

    // A second worker should still be able to allocate and write if the first
    // worker's failed attempt released pool permits.
    let succeeding_agent_id = agent_id!("FileSystem", "stream-write-failed-no-leak-b-1");
    let succeeding_worker_id = executor
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

    executor
        .check_oplog_is_queryable(&failing_worker_id)
        .await?;
    executor
        .check_oplog_is_queryable(&succeeding_worker_id)
        .await?;

    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn executor_pool_reuses_existing_rounding_capacity_for_same_agent(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start_with_executor_storage_pool(deps, &context, 1024).await?;
    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;
    let agent = agent_id!("FileSystem", "same-agent-rounded-capacity");
    let worker = executor.start_agent(&component.id, agent.clone()).await?;

    executor
        .invoke_and_await_agent(
            &component,
            &agent,
            "write_file",
            data_value!("/first.bin", "1234"),
        )
        .await?;
    executor
        .invoke_and_await_agent(
            &component,
            &agent,
            "write_file",
            data_value!("/second.bin", "5678"),
        )
        .await?;
    let oplog = executor.get_oplog(&worker, OplogIndex::INITIAL).await?;
    assert_eq!(
        filesystem_storage_deltas(&oplog),
        vec![4, 4],
        "both writes must commit while sharing one rounded host permit"
    );
    assert!(
        !oplog
            .iter()
            .any(|entry| matches!(entry.entry, PublicOplogEntry::Error(_))),
        "reusing existing rounded capacity must not require a storage-failure restart"
    );
    assert_eq!(
        executor
            .worker_eviction_class(&OwnedAgentId::new(context.default_environment_id, &worker,))
            .await,
        Some(EvictionClass::LoadedIdle)
    );
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn nonblocking_stream_flush_settles_storage_before_descriptor_mutation(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start_with_agent_storage_quota(deps, &context, 8).await?;
    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;
    let agent = agent_id!("FileSystem", "flush-before-descriptor-mutation");
    let worker = executor.start_agent(&component.id, agent.clone()).await?;

    executor
        .invoke_and_await_agent(
            &component,
            &agent,
            "stream_flush_then_set_size",
            data_value!("/flush-then-resize.bin", 8u64, 4u64),
        )
        .await?;

    assert_file_len(&executor, &component, &agent, "/flush-then-resize.bin", 4).await?;
    assert_eq!(
        filesystem_storage_deltas(&executor.get_oplog(&worker, OplogIndex::INITIAL).await?),
        vec![8, -4],
        "flush must settle the stream reservation before set_size releases storage"
    );
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn pending_stream_write_completes_before_descriptor_mutation(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start_with_agent_storage_quota(deps, &context, 8).await?;
    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;
    let agent = agent_id!("FileSystem", "pending-write-before-descriptor-mutation");
    let worker = executor.start_agent(&component.id, agent.clone()).await?;

    executor
        .invoke_and_await_agent(
            &component,
            &agent,
            "stream_write_then_set_size",
            data_value!("/write-then-resize.bin", 8u64, 4u64),
        )
        .await?;

    assert_file_len(&executor, &component, &agent, "/write-then-resize.bin", 4).await?;
    assert_eq!(
        filesystem_storage_deltas(&executor.get_oplog(&worker, OplogIndex::INITIAL).await?),
        vec![8, -4],
        "descriptor mutation must complete and settle the pending stream write first"
    );
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn filesystem_stream_accounting_survives_descriptor_drop(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start_with_agent_storage_quota(deps, &context, 8).await?;
    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;
    let agent = agent_id!("FileSystem", "stream-after-descriptor-drop");
    let worker = executor.start_agent(&component.id, agent.clone()).await?;

    executor
        .invoke_and_await_agent(
            &component,
            &agent,
            "stream_after_descriptor_drop",
            data_value!("/descriptor-dropped.bin", 8u64),
        )
        .await?;

    assert_eq!(
        filesystem_storage_deltas(&executor.get_oplog(&worker, OplogIndex::INITIAL).await?),
        vec![8],
        "stream reconciliation must not depend on the descriptor resource remaining open"
    );
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn filesystem_stream_accounting_survives_file_rename(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start_with_agent_storage_quota(deps, &context, 8).await?;
    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;
    let agent = agent_id!("FileSystem", "stream-after-rename");
    let worker = executor.start_agent(&component.id, agent.clone()).await?;

    executor
        .invoke_and_await_agent(
            &component,
            &agent,
            "stream_after_rename",
            data_value!("/before-rename.bin", "/after-rename.bin", 8u64),
        )
        .await?;

    assert_file_len(&executor, &component, &agent, "/after-rename.bin", 8).await?;
    assert_eq!(
        filesystem_storage_deltas(&executor.get_oplog(&worker, OplogIndex::INITIAL).await?),
        vec![8],
        "stream reconciliation must follow the open file across a rename"
    );
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn renamed_open_stream_uses_the_destination_mutation_lock(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start_with_agent_storage_quota(deps, &context, 8).await?;
    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;
    let agent = agent_id!("FileSystem", "stream-write-after-rename");
    let worker = executor.start_agent(&component.id, agent.clone()).await?;

    executor
        .invoke_and_await_agent(
            &component,
            &agent,
            "stream_write_after_rename_then_set_size",
            data_value!("/before.bin", "/after.bin", 8u64, 4u64),
        )
        .await?;

    assert_file_len(&executor, &component, &agent, "/after.bin", 4).await?;
    assert_eq!(
        filesystem_storage_deltas(&executor.get_oplog(&worker, OplogIndex::INITIAL).await?),
        vec![8, -4]
    );
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn renamed_descriptor_keeps_its_file_lock_after_old_path_reuse(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start_with_agent_storage_quota(deps, &context, 8).await?;
    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;
    let agent = agent_id!("FileSystem", "renamed-descriptor-path-reuse");
    let worker = executor.start_agent(&component.id, agent.clone()).await?;

    executor
        .invoke_and_await_agent(
            &component,
            &agent,
            "stream_after_rename_path_reuse",
            data_value!("/a.bin", "/b.bin", "/c.bin", 8u64, 4u64),
        )
        .await?;

    assert_file_len(&executor, &component, &agent, "/b.bin", 4).await?;
    assert_eq!(
        filesystem_storage_deltas(&executor.get_oplog(&worker, OplogIndex::INITIAL).await?),
        vec![8, -4]
    );
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn rename_releases_replaced_file_storage(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start_with_agent_storage_quota(deps, &context, 12).await?;
    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;
    let agent = agent_id!("FileSystem", "rename-replaces-file");
    let worker = executor.start_agent(&component.id, agent.clone()).await?;

    executor
        .invoke_and_await_agent(
            &component,
            &agent,
            "write_file",
            data_value!("/source.bin", "1234"),
        )
        .await?;
    executor
        .invoke_and_await_agent(
            &component,
            &agent,
            "write_file",
            data_value!("/destination.bin", "12345678"),
        )
        .await?;
    executor
        .invoke_and_await_agent(
            &component,
            &agent,
            "rename_file",
            data_value!("/source.bin", "/destination.bin"),
        )
        .await?;

    assert_file_len(&executor, &component, &agent, "/destination.bin", 4).await?;
    assert_eq!(
        filesystem_storage_deltas(&executor.get_oplog(&worker, OplogIndex::INITIAL).await?),
        vec![4, 8, -8]
    );
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn rename_does_not_release_a_replaced_hardlink_target(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start_with_agent_storage_quota(deps, &context, 6).await?;
    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;
    let agent = agent_id!("FileSystem", "rename-replaces-hardlink");
    let worker = executor.start_agent(&component.id, agent.clone()).await?;

    for (path, contents) in [("/original.bin", "12"), ("/replacement.bin", "1234")] {
        executor
            .invoke_and_await_agent(
                &component,
                &agent,
                "write_file",
                data_value!(path, contents),
            )
            .await?;
    }
    executor
        .invoke_and_await_agent(
            &component,
            &agent,
            "create_link",
            data_value!("/original.bin", "/linked.bin"),
        )
        .await?;
    executor
        .invoke_and_await_agent(
            &component,
            &agent,
            "rename_file",
            data_value!("/replacement.bin", "/linked.bin"),
        )
        .await?;

    assert_file_len(&executor, &component, &agent, "/original.bin", 2).await?;
    assert_file_len(&executor, &component, &agent, "/linked.bin", 4).await?;
    assert_eq!(
        filesystem_storage_deltas(&executor.get_oplog(&worker, OplogIndex::INITIAL).await?),
        vec![2, 4]
    );
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn pending_stream_is_reconciled_before_invocation_completion(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start_with_agent_storage_quota(deps, &context, 8).await?;
    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;
    let agent = agent_id!("FileSystem", "pending-stream-at-invocation-end");
    let worker = executor.start_agent(&component.id, agent.clone()).await?;

    executor
        .invoke_and_await_agent(
            &component,
            &agent,
            "leaked_stream_to_file",
            data_value!("/pending-at-end.bin", 8u64),
        )
        .await?;

    assert_eq!(
        filesystem_storage_deltas(&executor.get_oplog(&worker, OplogIndex::INITIAL).await?),
        vec![8],
        "invocation completion must persist every pending stream reservation"
    );
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
    let worker_id = executor
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

    assert_eq!(
        filesystem_storage_deltas(&executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?),
        vec![1024]
    );

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
    let worker_id = executor
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

    executor.check_oplog_is_queryable(&worker_id).await?;

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
    let worker_id = executor
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

    assert_specific_storage_quota_failure(&result, "blocking stream and flush");

    executor.check_oplog_is_queryable(&worker_id).await?;

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
    let worker_id = executor
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

    executor.check_oplog_is_queryable(&worker_id).await?;

    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn agent_quota_consecutive_blocking_stream_writes_are_cumulative(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start_with_agent_storage_quota(deps, &context, 12).await?;
    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;
    let agent_id = agent_id!("FileSystem", "consecutive-blocking-stream-writes");
    let worker = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    let result = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "consecutive_blocking_stream_writes",
            data_value!("/stream.bin", 8u64, 8u64),
        )
        .await;

    assert!(
        result.is_err(),
        "the second write must fail when cumulative stream growth exceeds quota"
    );
    let error = format!("{result:?}");
    assert!(
        error.contains("AgentExceededFilesystemStorageLimit"),
        "expected cumulative quota failure, got: {error}"
    );
    let oplog = executor.get_oplog(&worker, OplogIndex::INITIAL).await?;
    assert_eq!(filesystem_storage_deltas(&oplog), vec![8]);
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
    let worker_id = executor
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
    assert_specific_storage_quota_failure(&result, "set_size growth");

    executor.check_oplog_is_queryable(&worker_id).await?;

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
    let worker_id = executor
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

    executor.check_oplog_is_queryable(&worker_id).await?;

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
    let worker_id = executor
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
    assert_specific_storage_quota_failure(&result, "positioned write");

    executor.check_oplog_is_queryable(&worker_id).await?;

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
    let worker_id = executor
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
        SchemaValue::Result(ResultValuePayload::Ok {
            value: Some(Box::new(SchemaValue::String("hello world".to_string())))
        })
    );

    executor.check_oplog_is_queryable(&worker_id).await?;

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
    let worker_id = executor
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

    assert_eq!(
        filesystem_storage_deltas(&executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?),
        vec![11]
    );

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
    let worker_id = executor
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

    assert_specific_storage_quota_failure(&result, "stream growth after direct write");

    executor.check_oplog_is_queryable(&worker_id).await?;

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
        SchemaValue::Result(ResultValuePayload::Ok {
            value: Some(Box::new(SchemaValue::String(
                "unique-content-2".to_string()
            )))
        })
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
    assert_specific_storage_quota_failure(
        &result,
        "11-byte write after restart with 27 of 32 bytes already used",
    );

    executor.check_oplog_is_queryable(&worker).await?;

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
    let worker_id = executor
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

    executor.check_oplog_is_queryable(&worker_id).await?;

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
    let worker_id = executor
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

    executor.check_oplog_is_queryable(&worker_id).await?;

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
        SchemaValue::Result(ResultValuePayload::Ok {
            value: Some(Box::new(SchemaValue::String(content_2kb)))
        })
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
        SchemaValue::Result(ResultValuePayload::Ok {
            value: Some(Box::new(SchemaValue::String(content_file3)))
        })
    );

    executor.check_oplog_is_queryable(&worker).await?;

    Ok(())
}

/// When the executor storage pool is full, a worker restart pre-acquires
/// storage via the blocking path which evicts the oldest idle worker, freeing
/// its permits so the restarting worker can proceed.
///
/// Flow (2 KB pool, each worker has a 2 KB file):
/// 1. Worker A writes 2 KB, then is interrupted, releasing its permits.
/// 2. Worker B writes 2 KB, then is interrupted.
/// 3. Worker A restarts and holds the full pool while idle.
/// 4. Worker B restarts, forcing eviction of Worker A before it can acquire 2 KB.
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
    let executor = start_with_executor_storage_pool(deps, &context, 2 * 1024).await?;

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
    // Pool has 2 KB free → blocking acquire_storage acquires 2 permits.
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
        SchemaValue::Result(ResultValuePayload::Ok {
            value: Some(Box::new(SchemaValue::String(content_a.clone())))
        })
    );

    // Step 4: Re-invoke Worker B. Restarts with filesystem_storage_requirement = 2 KB.
    // Pool is empty (Worker A holds 2 KB) — not enough for B's 2 KB requirement.
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
        SchemaValue::Result(ResultValuePayload::Ok {
            value: Some(Box::new(SchemaValue::String(content_b.clone())))
        })
    );

    assert_ne!(
        executor
            .worker_eviction_class(&OwnedAgentId::new(
                context.default_environment_id,
                &worker_a,
            ))
            .await,
        Some(EvictionClass::LoadedIdle),
        "Worker A must no longer be resident after freeing the full pool for Worker B"
    );

    executor.check_oplog_is_queryable(&worker_a).await?;
    executor.check_oplog_is_queryable(&worker_b).await?;

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
    let zero_storage_agent = agent_id!("FileSystem", "exec-pool-zero-storage-1");

    let zero_storage_worker = executor
        .start_agent(&component.id, zero_storage_agent.clone())
        .await?;
    executor
        .invoke_and_await_agent(
            &component,
            &zero_storage_agent,
            "file_len",
            data_value!("/missing.txt"),
        )
        .await?;
    assert_eq!(
        executor
            .worker_eviction_class(&OwnedAgentId::new(
                context.default_environment_id,
                &zero_storage_worker,
            ))
            .await,
        Some(EvictionClass::LoadedIdle)
    );

    // 2 KB content (> 1 KB so each write consumes 2 permits, not just 1).
    let content_a = "A".repeat(2048);
    let content_b = "B".repeat(2048);

    // Worker A writes 2 KB and goes idle — pool exhausted (2/2 permits held).
    let worker_a = executor.start_agent(&component.id, agent_a.clone()).await?;
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
    let worker_b = executor.start_agent(&component.id, agent_b.clone()).await?;
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
        SchemaValue::Result(ResultValuePayload::Ok {
            value: Some(Box::new(SchemaValue::String(content_b)))
        })
    );

    assert_eq!(
        executor
            .worker_eviction_class(&OwnedAgentId::new(
                context.default_environment_id,
                &zero_storage_worker,
            ))
            .await,
        Some(EvictionClass::LoadedIdle),
        "filesystem pressure must not evict a worker that holds no filesystem permits"
    );
    assert_ne!(
        executor
            .worker_eviction_class(&OwnedAgentId::new(
                context.default_environment_id,
                &worker_a,
            ))
            .await,
        Some(EvictionClass::LoadedIdle),
        "filesystem pressure must evict the worker holding the exhausted pool"
    );

    executor.check_oplog_is_queryable(&worker_a).await?;
    executor.check_oplog_is_queryable(&worker_b).await?;

    Ok(())
}

/// Verifies that eviction only acquires the gap — i.e. only evicts as many
/// idle workers as needed to cover the missing portion of the requested bytes,
/// not the full amount.
///
/// Pool = 6 KB. Setup:
///   Worker A writes 1025 bytes → 2 permits → pool: 4 KB free.
///   Worker B writes 1 KB → pool: 3 KB free.
///   Worker C writes 3 KB → pool: 0 KB free. C goes idle.
///
/// C then tries to write an extra 2 KB. Pool is exhausted → NodeOutOfFilesystemStorage.
/// `desired_extra_filesystem_storage = 2 KB`. On `ReacquirePermits` restart:
///   - Old C RunningWorker drops → 3 KB returned to pool (pool: 3 KB free).
///   - `acquire_bytes = filesystem_storage_requirement(3KB) + desired_extra(2KB) = 5 KB`.
///   - Pool has 3 KB → gap = 2 KB → eviction targets 2 KB → evicts idle Worker A
///     (holding 2 permits) → pool: 3 + 2 = 5 KB free.
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
    let content_a = "A".repeat(1025); // 1025 bytes → 2 permits
    let content_b = "B".repeat(1024); // 1 KB → 1 permit
    let content_c1 = "C".repeat(3 * 1024); // 3 KB → 3 permits
    let content_c2 = "D".repeat(2 * 1024); // 2 KB → the extra write that fails

    // Worker A: write 1025 bytes → 2 permits → pool: 4 KB free.
    let worker_a = executor.start_agent(&component.id, agent_a.clone()).await?;
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
    let worker_c = executor.start_agent(&component.id, agent_c.clone()).await?;
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
    // acquire_bytes = 3+2 = 5 KB → gap = 2 KB → evicts idle Worker A (2 permits).
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

    // Verify Worker B was NOT evicted, proving eviction stopped after freeing
    // only the 2 KB gap from A rather than evicting B's 1 KB as well.
    assert_eq!(
        executor
            .worker_eviction_class(&OwnedAgentId::new(
                context.default_environment_id,
                &worker_b,
            ))
            .await,
        Some(EvictionClass::LoadedIdle),
        "Worker B must remain resident after only Worker A's 2 permits are evicted"
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
        SchemaValue::Result(ResultValuePayload::Ok {
            value: Some(Box::new(SchemaValue::String(content_b)))
        })
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
        SchemaValue::Result(ResultValuePayload::Ok {
            value: Some(Box::new(SchemaValue::String(content_c1)))
        })
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
        SchemaValue::Result(ResultValuePayload::Ok {
            value: Some(Box::new(SchemaValue::String(content_c2)))
        })
    );

    executor.check_oplog_is_queryable(&worker_a).await?;
    executor.check_oplog_is_queryable(&worker_b).await?;
    executor.check_oplog_is_queryable(&worker_c).await?;

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
    let worker_b = executor.start_agent(&component.id, agent_b.clone()).await?;
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
        SchemaValue::Result(ResultValuePayload::Ok {
            value: Some(Box::new(SchemaValue::String("content-b-file1".to_string())))
        })
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
        SchemaValue::Result(ResultValuePayload::Ok {
            value: Some(Box::new(SchemaValue::String("content-a-file2".to_string())))
        })
    );

    executor.check_oplog_is_queryable(&worker_a).await?;
    executor.check_oplog_is_queryable(&worker_b).await?;

    Ok(())
}

/// `blocking_splice` from an input stream to a file-backed output stream must
/// be subject to storage quota.  After writing an 11-byte file that nearly
/// exhausts the 20-byte quota, splicing those 11 bytes into a second file
/// (total 22 bytes) must fail with `AgentExceededFilesystemStorageLimit`.
#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn agent_quota_blocking_splice_exceeding_limit_fails(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    // 20-byte quota — "hello world" (11 bytes) fits once but not twice.
    let executor = start_with_agent_storage_quota(deps, &context, 20).await?;

    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;

    let agent_id = agent_id!("FileSystem", "blocking-splice-quota-1");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    // Write the source file — consumes 11 of 20 bytes.
    executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "write_file",
            data_value!("/src.txt", "hello world"),
        )
        .await?;

    // Splice source into a new destination — needs 11 more bytes (total 22),
    // which exceeds the 20-byte quota.
    let result = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "blocking_splice",
            data_value!("/src.txt", "/dst.txt"),
        )
        .await;

    assert_specific_storage_quota_failure(&result, "blocking_splice");

    executor.check_oplog_is_queryable(&worker_id).await?;

    Ok(())
}

/// A successful file-backed `blocking_splice` must complete without re-entering
/// the path mutation lock and must commit exactly the observed destination growth.
#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn agent_quota_blocking_splice_within_limit_completes(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start_with_agent_storage_quota(deps, &context, 22).await?;
    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;
    let agent_id = agent_id!("FileSystem", "blocking-splice-within-limit");
    let worker = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "write_file",
            data_value!("/src.txt", "hello world"),
        )
        .await?;
    let result = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "blocking_splice",
            data_value!("/src.txt", "/dst.txt"),
        )
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow::anyhow!("expected blocking_splice return value"))?;

    assert_eq!(
        result,
        SchemaValue::Result(ResultValuePayload::Ok {
            value: Some(Box::new(SchemaValue::U64(11)))
        })
    );
    let oplog = executor.get_oplog(&worker, OplogIndex::INITIAL).await?;
    assert_eq!(filesystem_storage_deltas(&oplog), vec![11, 11]);
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn blocking_splice_reserves_the_available_input_not_the_requested_limit(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start_with_agent_storage_quota(deps, &context, 2).await?;
    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;
    let agent = agent_id!("FileSystem", "blocking-splice-request-limit");
    let worker = executor.start_agent(&component.id, agent.clone()).await?;

    executor
        .invoke_and_await_agent(
            &component,
            &agent,
            "write_file",
            data_value!("/one-byte-source.bin", "x"),
        )
        .await?;
    let result = executor
        .invoke_and_await_agent(
            &component,
            &agent,
            "blocking_splice_len",
            data_value!(
                "/one-byte-source.bin",
                "/one-byte-destination.bin",
                u64::MAX
            ),
        )
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow::anyhow!("expected blocking_splice_len return value"))?;

    assert_eq!(
        result,
        SchemaValue::Result(ResultValuePayload::Ok {
            value: Some(Box::new(SchemaValue::U64(1)))
        })
    );
    assert_eq!(
        filesystem_storage_deltas(&executor.get_oplog(&worker, OplogIndex::INITIAL).await?),
        vec![1, 1]
    );
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn splice_reserves_the_available_input_not_the_requested_limit(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start_with_agent_storage_quota(deps, &context, 2).await?;
    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;
    let agent = agent_id!("FileSystem", "splice-request-limit");
    let worker = executor.start_agent(&component.id, agent.clone()).await?;

    executor
        .invoke_and_await_agent(
            &component,
            &agent,
            "write_file",
            data_value!("/one-byte-source.bin", "x"),
        )
        .await?;
    let result = executor
        .invoke_and_await_agent(
            &component,
            &agent,
            "splice_len",
            data_value!(
                "/one-byte-source.bin",
                "/one-byte-destination.bin",
                u64::MAX
            ),
        )
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow::anyhow!("expected splice_len return value"))?;

    assert_eq!(
        result,
        SchemaValue::Result(ResultValuePayload::Ok {
            value: Some(Box::new(SchemaValue::U64(1)))
        })
    );
    assert_eq!(
        filesystem_storage_deltas(&executor.get_oplog(&worker, OplogIndex::INITIAL).await?),
        vec![1, 1]
    );
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn positioned_splice_accounts_for_the_sparse_hole(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start_with_agent_storage_quota(deps, &context, 12).await?;
    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;
    let agent = agent_id!("FileSystem", "positioned-splice-hole");
    let worker = executor.start_agent(&component.id, agent.clone()).await?;

    executor
        .invoke_and_await_agent(
            &component,
            &agent,
            "write_file",
            data_value!("/four-byte-source.bin", "1234"),
        )
        .await?;
    let result = executor
        .invoke_and_await_agent(
            &component,
            &agent,
            "blocking_splice_at",
            data_value!("/four-byte-source.bin", "/sparse.bin", 4u64, u64::MAX),
        )
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow::anyhow!("expected blocking_splice_at return value"))?;

    assert_eq!(
        result,
        SchemaValue::Result(ResultValuePayload::Ok {
            value: Some(Box::new(SchemaValue::U64(4)))
        })
    );
    assert_file_len(&executor, &component, &agent, "/sparse.bin", 8).await?;
    assert_eq!(
        filesystem_storage_deltas(&executor.get_oplog(&worker, OplogIndex::INITIAL).await?),
        vec![4, 8]
    );
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn executor_pool_overlapping_p2_streams_use_aggregate_rounding(
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
    let agent = agent_id!("FileSystem", "overlapping-p2-streams");
    let worker = executor.start_agent(&component.id, agent.clone()).await?;

    executor
        .invoke_and_await_agent(
            &component,
            &agent,
            "write_file",
            data_value!("/base.bin", "x".repeat(500)),
        )
        .await?;
    executor
        .invoke_and_await_agent(
            &component,
            &agent,
            "overlapping_stream_writes",
            data_value!("/first.bin", "/second.bin", 600u64),
        )
        .await?;

    assert_file_len(&executor, &component, &agent, "/first.bin", 600).await?;
    assert_file_len(&executor, &component, &agent, "/second.bin", 600).await?;
    assert_eq!(
        filesystem_storage_deltas(&executor.get_oplog(&worker, OplogIndex::INITIAL).await?),
        vec![500, 600, 600]
    );
    Ok(())
}

/// Same as `agent_quota_blocking_splice_exceeding_limit_fails` but exercises
/// the non-blocking `output-stream.splice` path.
#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn agent_quota_splice_exceeding_limit_fails(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start_with_agent_storage_quota(deps, &context, 20).await?;

    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;

    let agent_id = agent_id!("FileSystem", "splice-quota-1");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "write_file",
            data_value!("/src.txt", "hello world"),
        )
        .await?;

    let result = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "splice",
            data_value!("/src.txt", "/dst.txt"),
        )
        .await;

    assert_specific_storage_quota_failure(&result, "splice");

    executor.check_oplog_is_queryable(&worker_id).await?;

    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn provisioned_read_write_file_counts_toward_agent_quota(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start_with_agent_storage_quota(deps, &context, 3).await?;
    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .with_files(
            "FileSystem",
            &[IFSEntry {
                source_path: PathBuf::from("initial-file-system/files/baz.txt"),
                target_path: CanonicalFilePath::from_abs_str("/bar/baz.txt").unwrap(),
                permissions: AgentFilePermissions::ReadWrite,
            }],
        )
        .store()
        .await?;

    let result = executor
        .start_agent(
            &component.id,
            agent_id!("FileSystem", "provisioned-file-over-quota"),
        )
        .await;

    let error = result.expect_err(
        "4-byte provisioned read-write file must prevent creation under a 3-byte quota",
    );
    let message = error.to_string();
    assert!(
        message.contains(
            "Provisioned read-write files require 4 bytes, exceeding the per-agent disk limit of 3 bytes"
        ),
        "unexpected worker creation error: {message}"
    );
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn executor_pool_shares_read_only_initial_file_across_agents(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start_with_executor_storage_pool(deps, &context, 1024).await?;
    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .with_files(
            "FileSystem",
            &[IFSEntry {
                source_path: PathBuf::from("initial-file-system/files/baz.txt"),
                target_path: CanonicalFilePath::from_abs_str("/shared.txt").unwrap(),
                permissions: AgentFilePermissions::ReadOnly,
            }],
        )
        .store()
        .await?;
    let agent_a = agent_id!("FileSystem", "shared-read-only-a");
    let agent_b = agent_id!("FileSystem", "shared-read-only-b");
    let worker_a = executor.start_agent(&component.id, agent_a.clone()).await?;
    let _worker_b = executor.start_agent(&component.id, agent_b.clone()).await?;

    for agent_id in [&agent_a, &agent_b] {
        let result = executor
            .invoke_and_await_agent(
                &component,
                agent_id,
                "read_file",
                data_value!("/shared.txt"),
            )
            .await?
            .into_return_value()
            .ok_or_else(|| anyhow::anyhow!("expected read_file return value"))?;
        assert_eq!(
            result,
            SchemaValue::Result(ResultValuePayload::Ok {
                value: Some(Box::new(SchemaValue::String("baz\n".to_string())))
            })
        );
    }

    assert_eq!(
        executor
            .worker_eviction_class(&OwnedAgentId::new(
                context.default_environment_id,
                &worker_a,
            ))
            .await,
        Some(EvictionClass::LoadedIdle),
        "the second agent must share the cached backing rather than evicting the first"
    );
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn provisioned_read_write_baseline_survives_restart_without_duplication(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start_with_agent_storage_quota(deps, &context, 8).await?;
    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .with_files(
            "FileSystem",
            &[IFSEntry {
                source_path: PathBuf::from("initial-file-system/files/baz.txt"),
                target_path: CanonicalFilePath::from_abs_str("/bar/baz.txt").unwrap(),
                permissions: AgentFilePermissions::ReadWrite,
            }],
        )
        .store()
        .await?;
    let agent = agent_id!("FileSystem", "provisioned-file-restart");
    let account_id = context.account_id.to_string();
    let environment_id = context.default_environment_id.to_string();
    let written_before = STORAGE_BYTES_WRITTEN_TOTAL
        .with_label_values(&[STORAGE_TYPE_FILESYSTEM, &account_id, &environment_id])
        .get();
    let worker = executor.start_agent(&component.id, agent.clone()).await?;
    let written_after_creation = STORAGE_BYTES_WRITTEN_TOTAL
        .with_label_values(&[STORAGE_TYPE_FILESYSTEM, &account_id, &environment_id])
        .get();
    assert_eq!(
        written_after_creation - written_before,
        4.0,
        "fresh creation must record the provisioned read-write baseline once"
    );

    executor
        .invoke_and_await_agent(
            &component,
            &agent,
            "pwrite_file",
            data_value!("/bar/baz.txt", 4u64, "5678"),
        )
        .await?;
    let before_restart = executor.get_oplog(&worker, OplogIndex::INITIAL).await?;
    assert_eq!(
        filesystem_storage_deltas(&before_restart),
        vec![4],
        "oplog must contain exactly the committed growth after the provisioned baseline"
    );
    executor.interrupt(&worker).await?;

    let len = executor
        .invoke_and_await_agent(&component, &agent, "file_len", data_value!("/bar/baz.txt"))
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow::anyhow!("expected file_len return value"))?;
    assert_eq!(
        len,
        SchemaValue::Result(ResultValuePayload::Ok {
            value: Some(Box::new(SchemaValue::U64(8)))
        })
    );
    let after_restart = executor.get_oplog(&worker, OplogIndex::INITIAL).await?;
    assert_eq!(
        filesystem_storage_deltas(&after_restart),
        vec![4],
        "restart must not emit a duplicate provisioned-file seed"
    );
    let written_after_restart = STORAGE_BYTES_WRITTEN_TOTAL
        .with_label_values(&[STORAGE_TYPE_FILESYSTEM, &account_id, &environment_id])
        .get();
    assert_eq!(
        written_after_restart - written_before,
        8.0,
        "restart must not record the four-byte provisioned baseline again"
    );

    let over_quota = executor
        .invoke_and_await_agent(
            &component,
            &agent,
            "pwrite_file",
            data_value!("/bar/baz.txt", 8u64, "x"),
        )
        .await;
    assert!(
        over_quota.is_err(),
        "restart must recover the 4-byte provisioned seed plus 4-byte growth exactly once"
    );
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn replayed_file_release_does_not_duplicate_storage_accounting(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start_with_agent_storage_quota(deps, &context, 8).await?;
    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;
    let agent = agent_id!("FileSystem", "release-replay-accounting");
    let worker = executor.start_agent(&component.id, agent.clone()).await?;

    executor
        .invoke_and_await_agent(
            &component,
            &agent,
            "write_file",
            data_value!("/retained.bin", "1234"),
        )
        .await?;
    executor
        .invoke_and_await_agent(
            &component,
            &agent,
            "write_file",
            data_value!("/released.bin", "5678"),
        )
        .await?;
    executor
        .invoke_and_await_agent(
            &component,
            &agent,
            "delete_file",
            data_value!("/released.bin"),
        )
        .await?;
    let before_replay = executor.get_oplog(&worker, OplogIndex::INITIAL).await?;
    assert_eq!(filesystem_storage_deltas(&before_replay), vec![4, 4, -4]);
    let after_delete = before_replay
        .last()
        .expect("worker oplog must not be empty")
        .oplog_index;
    let _ = executor
        .invoke_and_await_agent(&component, &agent, "file_len", data_value!("/released.bin"))
        .await?;
    executor
        .revert(
            &worker,
            RevertWorkerTarget::RevertToOplogIndex(RevertToOplogIndex {
                last_oplog_index: after_delete,
            }),
        )
        .await?;
    let over_quota = executor
        .invoke_and_await_agent(
            &component,
            &agent,
            "write_file",
            data_value!("/after-replay.bin", "abcde"),
        )
        .await;
    assert_specific_storage_quota_failure(&over_quota, "post-replay five-byte growth");

    assert_eq!(
        filesystem_storage_deltas(&executor.get_oplog(&worker, OplogIndex::INITIAL).await?),
        vec![4, 4, -4],
        "replay must not append or apply the historical release a second time"
    );
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn p2_filesystem_mutations_use_exact_canonical_quota_accounting(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start_with_agent_storage_quota(deps, &context, 10).await?;
    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;
    let agent = agent_id!("FileSystem", "p2-quota-mutations");
    let worker = executor.start_agent(&component.id, agent.clone()).await?;
    let path = "/quota-mutations.bin";
    let account_id = context.account_id.to_string();
    let environment_id = context.default_environment_id.to_string();
    let written_before = STORAGE_BYTES_WRITTEN_TOTAL
        .with_label_values(&[STORAGE_TYPE_FILESYSTEM, &account_id, &environment_id])
        .get();
    let deleted_before = STORAGE_BYTES_DELETED_TOTAL
        .with_label_values(&[STORAGE_TYPE_FILESYSTEM, &account_id, &environment_id])
        .get();

    executor
        .invoke_and_await_agent(&component, &agent, "write_file", data_value!(path, "1234"))
        .await?;
    assert_file_len(&executor, &component, &agent, path, 4).await?;
    executor
        .invoke_and_await_agent(&component, &agent, "append_file", data_value!(path, "567"))
        .await?;
    assert_file_len(&executor, &component, &agent, path, 7).await?;
    executor
        .invoke_and_await_agent(
            &component,
            &agent,
            "set_file_size",
            data_value!(path, 10u64),
        )
        .await?;
    assert_file_len(&executor, &component, &agent, path, 10).await?;
    executor
        .invoke_and_await_agent(&component, &agent, "set_file_size", data_value!(path, 6u64))
        .await?;
    assert_file_len(&executor, &component, &agent, path, 6).await?;
    executor
        .invoke_and_await_agent(&component, &agent, "truncate_file", data_value!(path))
        .await?;
    assert_file_len(&executor, &component, &agent, path, 0).await?;
    executor
        .invoke_and_await_agent(
            &component,
            &agent,
            "append_file",
            data_value!(path, "0123456789"),
        )
        .await?;
    assert_file_len(&executor, &component, &agent, path, 10).await?;
    executor
        .invoke_and_await_agent(&component, &agent, "delete_file", data_value!(path))
        .await?;
    executor
        .invoke_and_await_agent(
            &component,
            &agent,
            "write_file",
            data_value!("/replacement.bin", "abcdefghij"),
        )
        .await?;
    assert_file_len(&executor, &component, &agent, "/replacement.bin", 10).await?;
    let oplog = executor.get_oplog(&worker, OplogIndex::INITIAL).await?;
    assert_eq!(
        filesystem_storage_deltas(&oplog),
        vec![4, 3, 3, -4, -6, 10, -10, 10],
        "P2 committed deltas must match each observed file-size transition"
    );
    let written_after = STORAGE_BYTES_WRITTEN_TOTAL
        .with_label_values(&[STORAGE_TYPE_FILESYSTEM, &account_id, &environment_id])
        .get();
    let deleted_after = STORAGE_BYTES_DELETED_TOTAL
        .with_label_values(&[STORAGE_TYPE_FILESYSTEM, &account_id, &environment_id])
        .get();
    assert_eq!(written_after - written_before, 30.0);
    assert_eq!(deleted_after - deleted_before, 20.0);
    let over_quota = executor
        .invoke_and_await_agent(
            &component,
            &agent,
            "pwrite_file",
            data_value!("/replacement.bin", 10u64, "x"),
        )
        .await;
    assert_specific_storage_quota_failure(&over_quota, "P2 one-byte growth");
    Ok(())
}

async fn assert_file_len(
    executor: &TestWorkerExecutor,
    component: &ComponentDto,
    agent: &ParsedAgentId,
    path: &str,
    expected: u64,
) -> anyhow::Result<()> {
    let result = executor
        .invoke_and_await_agent(component, agent, "file_len", data_value!(path))
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow::anyhow!("expected file_len return value"))?;
    assert_eq!(
        result,
        SchemaValue::Result(ResultValuePayload::Ok {
            value: Some(Box::new(SchemaValue::U64(expected)))
        })
    );
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn p3_filesystem_mutations_use_exact_canonical_quota_accounting(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("initial_file_system")] initial_file_system: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start_with_agent_storage_quota(deps, &context, 10).await?;
    let component = executor
        .component_dep(&context.default_environment_id, initial_file_system)
        .store()
        .await?;
    let agent_id = agent_id!("P3FileSystem", "p3-quota-mutations");
    let worker = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;
    let account_id = context.account_id.to_string();
    let environment_id = context.default_environment_id.to_string();
    let written_before = STORAGE_BYTES_WRITTEN_TOTAL
        .with_label_values(&[STORAGE_TYPE_FILESYSTEM, &account_id, &environment_id])
        .get();
    let deleted_before = STORAGE_BYTES_DELETED_TOTAL
        .with_label_values(&[STORAGE_TYPE_FILESYSTEM, &account_id, &environment_id])
        .get();

    let result = executor
        .invoke_and_await_agent(&component, &agent_id, "mutation_sizes", data_value!())
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow::anyhow!("expected mutation_sizes return value"))?;

    assert_eq!(
        result,
        SchemaValue::List {
            elements: vec![4, 7, 10, 6, 0, 10, 0, 10]
                .into_iter()
                .map(SchemaValue::U64)
                .collect()
        }
    );
    let oplog = executor.get_oplog(&worker, OplogIndex::INITIAL).await?;
    assert_eq!(
        filesystem_storage_deltas(&oplog),
        vec![4, 3, 3, -4, -6, 10, -10, 10],
        "P3 committed deltas must match each observed file-size transition"
    );
    let written_after = STORAGE_BYTES_WRITTEN_TOTAL
        .with_label_values(&[STORAGE_TYPE_FILESYSTEM, &account_id, &environment_id])
        .get();
    let deleted_after = STORAGE_BYTES_DELETED_TOTAL
        .with_label_values(&[STORAGE_TYPE_FILESYSTEM, &account_id, &environment_id])
        .get();
    assert_eq!(written_after - written_before, 30.0);
    assert_eq!(deleted_after - deleted_before, 20.0);
    let over_quota = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "grow_replacement_beyond_quota",
            data_value!(),
        )
        .await;
    assert_specific_storage_quota_failure(&over_quota, "P3 one-byte growth");

    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn p3_mutation_waits_for_pending_p2_stream_accounting(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("initial_file_system")] initial_file_system: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start_with_agent_storage_quota(deps, &context, 8).await?;
    let component = executor
        .component_dep(&context.default_environment_id, initial_file_system)
        .store()
        .await?;
    let agent = agent_id!("P3FileSystem", "p2-stream-before-p3-mutation");
    let worker = executor.start_agent(&component.id, agent.clone()).await?;

    let result = executor
        .invoke_and_await_agent(
            &component,
            &agent,
            "p2_stream_then_p3_set_size",
            data_value!("mixed-preview.bin", 8u64, 4u64),
        )
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow::anyhow!("expected mixed-preview size"))?;

    assert_eq!(result, SchemaValue::U64(4));
    assert_eq!(
        filesystem_storage_deltas(&executor.get_oplog(&worker, OplogIndex::INITIAL).await?),
        vec![8, -4],
        "P3 must settle the P2 stream before measuring its mutation"
    );
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn p3_rename_releases_replaced_file_storage(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("initial_file_system")] initial_file_system: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start_with_agent_storage_quota(deps, &context, 12).await?;
    let component = executor
        .component_dep(&context.default_environment_id, initial_file_system)
        .store()
        .await?;
    let agent = agent_id!("P3FileSystem", "p3-rename-replaces-file");
    let worker = executor.start_agent(&component.id, agent.clone()).await?;

    let result = executor
        .invoke_and_await_agent(
            &component,
            &agent,
            "replace_file",
            data_value!("source.bin", "destination.bin", 4u64, 8u64),
        )
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow::anyhow!("expected P3 replacement size"))?;

    assert_eq!(result, SchemaValue::U64(4));
    assert_eq!(
        filesystem_storage_deltas(&executor.get_oplog(&worker, OplogIndex::INITIAL).await?),
        vec![4, 8, -8]
    );
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn p3_chunked_stream_commits_one_storage_transition(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("initial_file_system")] initial_file_system: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start_with_agent_storage_quota(deps, &context, 16).await?;
    let component = executor
        .component_dep(&context.default_environment_id, initial_file_system)
        .store()
        .await?;
    let agent = agent_id!("P3FileSystem", "p3-chunked-storage-accounting");
    let worker = executor.start_agent(&component.id, agent.clone()).await?;

    let result = executor
        .invoke_and_await_agent(
            &component,
            &agent,
            "write_chunks",
            data_value!("chunked.bin", 4u64, 4u64),
        )
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow::anyhow!("expected write_chunks return value"))?;
    assert_eq!(result, SchemaValue::U64(16));
    assert_eq!(
        filesystem_storage_deltas(&executor.get_oplog(&worker, OplogIndex::INITIAL).await?),
        vec![16],
        "one P3 filesystem stream must commit one canonical storage transition"
    );
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn executor_pool_overlapping_p3_streams_use_aggregate_rounding(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("initial_file_system")] initial_file_system: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start_with_executor_storage_pool(deps, &context, 2 * 1024).await?;
    let component = executor
        .component_dep(&context.default_environment_id, initial_file_system)
        .store()
        .await?;
    let agent = agent_id!("P3FileSystem", "overlapping-p3-streams");
    let worker = executor.start_agent(&component.id, agent.clone()).await?;

    executor
        .invoke_and_await_agent(
            &component,
            &agent,
            "write_bytes",
            data_value!("base.bin", 500u64),
        )
        .await?;
    let sizes = executor
        .invoke_and_await_agent(
            &component,
            &agent,
            "overlapping_stream_writes",
            data_value!("first.bin", "second.bin", 600u64),
        )
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow::anyhow!("expected overlapping_stream_writes return value"))?;

    assert_eq!(
        sizes,
        SchemaValue::List {
            elements: vec![SchemaValue::U64(600), SchemaValue::U64(600)]
        }
    );
    assert_eq!(
        filesystem_storage_deltas(&executor.get_oplog(&worker, OplogIndex::INITIAL).await?),
        vec![500, 600, 600]
    );
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn p3_chunked_stream_commits_written_prefix_when_later_chunk_exceeds_quota(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("initial_file_system")] initial_file_system: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start_with_agent_storage_quota(deps, &context, 6).await?;
    let component = executor
        .component_dep(&context.default_environment_id, initial_file_system)
        .store()
        .await?;
    let agent = agent_id!("P3FileSystem", "p3-partial-chunk-accounting");
    let worker = executor.start_agent(&component.id, agent.clone()).await?;

    let result = executor
        .invoke_and_await_agent(
            &component,
            &agent,
            "write_chunks",
            data_value!("partial.bin", 4u64, 2u64),
        )
        .await;
    assert_specific_storage_quota_failure(&result, "P3 second stream chunk");
    assert_eq!(
        filesystem_storage_deltas(&executor.get_oplog(&worker, OplogIndex::INITIAL).await?),
        vec![4],
        "the written prefix must remain accounted when a later chunk is rejected"
    );
    Ok(())
}

fn assert_specific_storage_quota_failure<T: std::fmt::Debug>(
    result: &anyhow::Result<T>,
    operation: &str,
) {
    assert!(result.is_err(), "{operation} must exceed the exact quota");
    let error = format!("{result:?}");
    assert!(
        error.contains("AgentExceededFilesystemStorageLimit"),
        "expected storage quota failure for {operation}, got: {error}"
    );
}
