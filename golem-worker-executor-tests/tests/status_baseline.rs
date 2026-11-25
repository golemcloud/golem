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

//! The durable status baseline: an archive of the oplog keeps it, a restart folds the status from
//! it, and a re-created agent never reads the baseline of the deleted one.

use crate::Tracing;

use crate::filesystem_snapshots::eventually;
use anyhow::Context;
use futures::{StreamExt, TryStreamExt};
use golem_common::model::agent::ParsedAgentKey;
use golem_common::model::component::ComponentDto;
use golem_common::model::{AgentStatus, OwnedAgentId, ShardAssignment, ShardId};
use golem_common::{agent_key, data_value};
use golem_test_framework::dsl::TestDsl;
use golem_worker_executor::services::golem_config::{AgentStatusCheckpointConfig, OplogConfig};
use golem_worker_executor::storage::scheduler::SchedulerStorage;
use golem_worker_executor::storage::scheduler::sqlite::SqliteSchedulerStorage;
use golem_worker_executor_test_utils::{
    LastUniqueId, PrecompiledComponent, TestContext, TestExecutorOverrides, TestWorkerExecutor,
    WorkerExecutorTestDependencies, agent_oplog_length, archived_oplog_chunks,
    scheduler_sqlite_storage_config, start_with_overrides, truncate_archived_oplog_chunk,
};
use pretty_assertions::assert_eq;
use std::sync::Arc;
use std::time::Duration;
use test_r::{inherit_test_dep, test, timeout};

inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(LastUniqueId);
inherit_test_dep!(
    #[tagged_as("agent_counters")]
    PrecompiledComponent
);
inherit_test_dep!(Tracing);

/// The compressed archive level the agents' oplogs move to; with no blob level they stay there.
const ARCHIVE_LEVEL: usize = 1;

/// Archives the oplogs of idle agents within a second, into the compressed level only, and writes
/// a status baseline at each idle boundary.
fn overrides() -> TestExecutorOverrides {
    TestExecutorOverrides {
        configure: Some(Arc::new(|config| {
            config.oplog = OplogConfig {
                indexed_storage_layers: 2,
                blob_storage_layers: 0,
                archive_interval: Duration::from_secs(1),
                entry_count_limit: 5,
                ..Default::default()
            };
            config.agent_status_checkpoint = AgentStatusCheckpointConfig {
                enabled: true,
                min_oplog_delta: 1,
            };
        })),
        ..Default::default()
    }
}

async fn start(
    deps: &WorkerExecutorTestDependencies,
    context: &TestContext,
) -> anyhow::Result<TestWorkerExecutor> {
    start_with_overrides(deps, context, overrides()).await
}

/// Waits until the primary oplog of the agent is empty and no archive action is due or leased,
/// so the whole history is in the archive and every archive has completed.
async fn wait_until_archived(
    deps: &WorkerExecutorTestDependencies,
    context: &TestContext,
    owned_agent_id: &OwnedAgentId,
) -> anyhow::Result<()> {
    let scheduler =
        SqliteSchedulerStorage::configured(&scheduler_sqlite_storage_config(deps, context))
            .await
            .map_err(anyhow::Error::msg)?;
    let assignment = ShardAssignment::unexpiring(1, [ShardId::new(0)]);
    eventually(Duration::from_secs(60), || async {
        let archived = agent_oplog_length(deps, context, owned_agent_id).await? == 0
            && scheduler
                .count_due(jiff::Timestamp::MAX, &assignment)
                .await?
                == 0;
        Ok(archived.then_some(()))
    })
    .await
    .with_context(|| format!("the oplog of {owned_agent_id} was not archived"))
}

/// Stops the worker once it settles after its last invocation, and retires it. The worker
/// writes its status baseline at this idle boundary, when its invocation loop exits.
async fn stop_when_idle(
    executor: &TestWorkerExecutor,
    owned_agent_id: &OwnedAgentId,
) -> anyhow::Result<()> {
    eventually(Duration::from_secs(10), || async {
        Ok(executor
            .stop_worker_if_idle(owned_agent_id)
            .await?
            .then_some(()))
    })
    .await?;
    executor.retire_unloaded_worker(owned_agent_id).await
}

/// Invokes `increment` on `agent` `count` times, one after the other.
async fn increment(
    executor: &TestWorkerExecutor,
    component: &ComponentDto,
    agent: &ParsedAgentKey,
    count: usize,
) -> anyhow::Result<()> {
    futures::stream::iter(0..count)
        .map(Ok)
        .try_for_each(|_| async {
            executor
                .invoke_and_await_agent(component, agent, "increment", data_value!())
                .await
                .map(|_| ())
        })
        .await
}

/// A completed archive keeps the status baseline. After a real restart with no cached status, the
/// status fold starts from the baseline and never reads the oplog below it. The fold does not read
/// an archived chunk before the baseline that does not decode. A full fold of the oplog fails on
/// that chunk.
///
/// The test reads the status through the status fold and does not ask for the metadata of the
/// agent. The invocation result index is not part of the baseline: a cold load of the agent
/// rebuilds it from index 1, so that load reads the chunk that does not decode.
#[test]
#[timeout("4m")]
#[tracing::instrument]
async fn a_restart_after_a_completed_archive_folds_the_status_from_the_baseline(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_counters")] agent_counters: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;
    let component = executor
        .component_dep(&context.default_environment_id, agent_counters)
        .store()
        .await?;
    let agent = agent_key!("Counter", "archived-baseline");
    let owned_agent_id = executor.start_agent(&component.id, agent.clone()).await?;
    let agent_id = owned_agent_id.agent_id;

    // Three rounds, each archived on its own, leave three chunks: the first holds `Create`. The
    // stop after the second round writes a status baseline before the last archive.
    increment(&executor, &component, &agent, 2).await?;
    wait_until_archived(deps, &context, &owned_agent_id).await?;
    increment(&executor, &component, &agent, 2).await?;
    wait_until_archived(deps, &context, &owned_agent_id).await?;
    stop_when_idle(&executor, &owned_agent_id).await?;
    let before_last_archive = executor.status_checkpoint(&agent_id).await?;
    increment(&executor, &component, &agent, 2).await?;
    wait_until_archived(deps, &context, &owned_agent_id).await?;
    let after_last_archive = executor.status_checkpoint(&agent_id).await?;
    stop_when_idle(&executor, &owned_agent_id).await?;
    let baseline = executor
        .status_checkpoint(&agent_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("the agent wrote no status baseline"))?;
    executor.remove_cached_status(&agent_id).await?;
    executor.shutdown_and_wait_for_invocation_loops().await?;
    drop(executor);

    let chunks = archived_oplog_chunks(deps, &context, &owned_agent_id, ARCHIVE_LEVEL).await?;
    anyhow::ensure!(
        chunks.len() >= 3,
        "expected three archived chunks, got {}",
        chunks.len()
    );
    let (middle_last_index, _) = chunks[chunks.len() - 2];
    anyhow::ensure!(
        middle_last_index < u64::from(baseline.oplog_idx),
        "the baseline at {} is not after the middle chunk, which ends at {middle_last_index}",
        baseline.oplog_idx
    );
    truncate_archived_oplog_chunk(
        deps,
        &context,
        &owned_agent_id,
        ARCHIVE_LEVEL,
        chunks.len() - 2,
    )
    .await?;

    let executor = start(deps, &context).await?;
    let status = executor
        .folded_status(&agent_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("the status fold gave no status"))?;
    let last_oplog_index = executor.last_oplog_index(&agent_id).await?;

    assert!(before_last_archive.is_some());
    assert_eq!(after_last_archive, before_last_archive);
    assert_eq!(status.status, AgentStatus::Idle);
    assert!(last_oplog_index >= baseline.oplog_idx);
    assert_eq!(status.oplog_idx, last_oplog_index);
    assert_eq!(executor.status_checkpoint(&agent_id).await?, Some(baseline));
    Ok(())
}

/// The deletion of an agent removes its status baseline, and the re-created agent of the same
/// name folds its status from its own oplog and baseline only.
#[test]
#[timeout("4m")]
#[tracing::instrument]
async fn a_re_created_agent_never_reads_the_baseline_of_the_deleted_one(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_counters")] agent_counters: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;
    let component = executor
        .component_dep(&context.default_environment_id, agent_counters)
        .store()
        .await?;
    let agent = agent_key!("Counter", "re-created-baseline");
    let owned_agent_id = executor.start_agent(&component.id, agent.clone()).await?;
    let agent_id = owned_agent_id.agent_id;
    increment(&executor, &component, &agent, 3).await?;
    stop_when_idle(&executor, &owned_agent_id).await?;
    let deleted = executor.agent_fingerprint(&agent_id).await?;
    let deleted_baseline = executor
        .status_checkpoint_of(&agent_id, deleted)
        .await?
        .ok_or_else(|| anyhow::anyhow!("the agent wrote no status baseline"))?;

    executor.delete_worker(&owned_agent_id).await?;
    let removed = executor.status_checkpoint_of(&agent_id, deleted).await?;

    executor.start_agent(&component.id, agent.clone()).await?;
    let first = executor
        .invoke_and_await_agent(&component, &agent, "increment", data_value!())
        .await?
        .into_typed::<u32>()?;
    let re_created = executor.agent_fingerprint(&agent_id).await?;
    stop_when_idle(&executor, &owned_agent_id).await?;
    executor.remove_cached_status(&agent_id).await?;
    executor.shutdown_and_wait_for_invocation_loops().await?;
    drop(executor);

    let executor = start(deps, &context).await?;
    let metadata = executor.get_worker_metadata(&owned_agent_id).await?;
    let second = executor
        .invoke_and_await_agent(&component, &agent, "increment", data_value!())
        .await?
        .into_typed::<u32>()?;
    let baseline = executor.status_checkpoint(&agent_id).await?;

    assert_eq!(removed, None);
    assert_ne!(re_created, deleted);
    assert_eq!((first, second), (1, 2));
    assert_eq!(metadata.status, AgentStatus::Idle);
    assert_eq!(
        deleted_baseline.export_fork_admissions.owner_fingerprint,
        Some(deleted)
    );
    assert_eq!(
        baseline.and_then(|baseline| baseline.export_fork_admissions.owner_fingerprint),
        Some(re_created)
    );
    assert_eq!(
        executor.status_checkpoint_of(&agent_id, deleted).await?,
        None
    );
    Ok(())
}
