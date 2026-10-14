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

//! An archived oplog chunk that does not decode fails its agent, and nothing else: the executor
//! starts on the data, the other agents run, and the chunk stays for an operator to inspect.

use crate::Tracing;

use golem_common::model::{AgentId, AgentStatus, OwnedAgentId};
use golem_common::{agent_id, data_value};
use golem_test_framework::dsl::TestDsl;
use golem_worker_executor::services::golem_config::OplogConfig;
use golem_worker_executor_test_utils::{
    LastUniqueId, PrecompiledComponent, TestContext, WorkerExecutorTestDependencies,
    agent_oplog_length, archived_oplog_chunks, start_with_oplog_config,
    truncate_archived_oplog_chunk,
};
use pretty_assertions::assert_eq;
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

/// Archives the oplogs of idle agents within a second, into the compressed level only.
fn archiving_config() -> OplogConfig {
    OplogConfig {
        indexed_storage_layers: 2,
        blob_storage_layers: 0,
        archive_interval: Duration::from_secs(1),
        entry_count_limit: 5,
        ..Default::default()
    }
}

/// Asserts that the invocation failed with the cause of the unreadable oplog: the agent, the
/// archive layer and the decode error.
fn assert_fails_with_the_decode_error<T: std::fmt::Debug>(
    result: anyhow::Result<T>,
    agent_id: &AgentId,
) {
    let error = result.expect_err("an agent whose oplog does not read must not run");
    let error = format!("{error:#}");
    assert!(
        error.contains(&agent_id.to_string())
            && error.contains("archive layer 1")
            && error.contains("failed to decode compressed oplog chunk")
            && error.contains("Input ended unexpectedly"),
        "the failure does not name the agent, the archive layer and the decode error: {error}"
    );
}

/// Waits until the primary oplog of the agent is empty, so its whole history, `Create` included,
/// is in the archive.
async fn wait_until_archived(
    deps: &WorkerExecutorTestDependencies,
    context: &TestContext,
    owned_agent_id: &OwnedAgentId,
) -> anyhow::Result<()> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    while agent_oplog_length(deps, context, owned_agent_id).await? > 0 {
        anyhow::ensure!(
            tokio::time::Instant::now() < deadline,
            "the oplog of {owned_agent_id} was not archived"
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    Ok(())
}

#[test]
#[timeout("4m")]
#[tracing::instrument]
async fn an_undecodable_archive_chunk_fails_only_its_agent(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_counters")] agent_counters: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start_with_oplog_config(deps, &context, Some(archiving_config())).await?;
    let component = executor
        .component_dep(&context.default_environment_id, agent_counters)
        .store()
        .await?;

    let unreadable = agent_id!("Counter", "unreadable-archive");
    let healthy = agent_id!("Counter", "healthy-archive");
    let unreadable_id = executor
        .start_agent(&component.id, unreadable.clone())
        .await?;
    let healthy_id = executor.start_agent(&component.id, healthy.clone()).await?;
    for agent in [&unreadable, &healthy] {
        for _ in 0..3 {
            executor
                .invoke_and_await_agent(&component, agent, "increment", data_value!())
                .await?;
        }
    }
    let unreadable_owned = OwnedAgentId::new(context.default_environment_id, &unreadable_id);
    let healthy_owned = OwnedAgentId::new(context.default_environment_id, &healthy_id);
    wait_until_archived(deps, &context, &unreadable_owned).await?;
    wait_until_archived(deps, &context, &healthy_owned).await?;
    drop(executor);

    truncate_archived_oplog_chunk(deps, &context, &unreadable_owned, ARCHIVE_LEVEL, 0).await?;
    let corrupt = archived_oplog_chunks(deps, &context, &unreadable_owned, ARCHIVE_LEVEL).await?;

    // The executor starts on the data, and recovers every agent it can read.
    let executor = start_with_oplog_config(deps, &context, Some(archiving_config())).await?;

    assert_fails_with_the_decode_error(
        executor
            .invoke_and_await_agent(&component, &unreadable, "increment", data_value!())
            .await,
        &unreadable_id,
    );

    assert_eq!(
        executor
            .invoke_and_await_agent(&component, &healthy, "increment", data_value!())
            .await?
            .into_typed::<u32>()?,
        4
    );
    assert_eq!(
        executor.get_worker_metadata(&healthy_id).await?.status,
        AgentStatus::Idle
    );

    // Nothing deleted or rewrote the unreadable history.
    drop(executor);
    assert_eq!(
        archived_oplog_chunks(deps, &context, &unreadable_owned, ARCHIVE_LEVEL).await?,
        corrupt
    );
    Ok(())
}

/// The `Create` of the agent reads, and a later archived chunk does not: the agent is found, its
/// start replays into the chunk and fails with the cause, and the agent is not run.
#[test]
#[timeout("4m")]
#[tracing::instrument]
async fn an_undecodable_later_archive_chunk_fails_only_its_agent(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_counters")] agent_counters: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start_with_oplog_config(deps, &context, Some(archiving_config())).await?;
    let component = executor
        .component_dep(&context.default_environment_id, agent_counters)
        .store()
        .await?;

    let unreadable = agent_id!("Counter", "unreadable-later-chunk");
    let healthy = agent_id!("Counter", "healthy-later-chunk");
    let unreadable_id = executor
        .start_agent(&component.id, unreadable.clone())
        .await?;
    let healthy_id = executor.start_agent(&component.id, healthy.clone()).await?;
    let unreadable_owned = OwnedAgentId::new(context.default_environment_id, &unreadable_id);
    let healthy_owned = OwnedAgentId::new(context.default_environment_id, &healthy_id);
    // Two rounds of invocations, each archived on its own, leave two chunks per agent.
    for _ in 0..2 {
        for agent in [&unreadable, &healthy] {
            for _ in 0..2 {
                executor
                    .invoke_and_await_agent(&component, agent, "increment", data_value!())
                    .await?;
            }
        }
        wait_until_archived(deps, &context, &unreadable_owned).await?;
        wait_until_archived(deps, &context, &healthy_owned).await?;
    }
    drop(executor);

    let chunks = archived_oplog_chunks(deps, &context, &unreadable_owned, ARCHIVE_LEVEL).await?;
    anyhow::ensure!(
        chunks.len() >= 2,
        "expected two archived chunks, got {}",
        chunks.len()
    );
    truncate_archived_oplog_chunk(
        deps,
        &context,
        &unreadable_owned,
        ARCHIVE_LEVEL,
        chunks.len() - 1,
    )
    .await?;
    let corrupt = archived_oplog_chunks(deps, &context, &unreadable_owned, ARCHIVE_LEVEL).await?;

    let executor = start_with_oplog_config(deps, &context, Some(archiving_config())).await?;

    assert_fails_with_the_decode_error(
        executor
            .invoke_and_await_agent(&component, &unreadable, "increment", data_value!())
            .await,
        &unreadable_id,
    );

    assert_eq!(
        executor
            .invoke_and_await_agent(&component, &healthy, "increment", data_value!())
            .await?
            .into_typed::<u32>()?,
        5
    );

    drop(executor);
    assert_eq!(
        archived_oplog_chunks(deps, &context, &unreadable_owned, ARCHIVE_LEVEL).await?,
        corrupt
    );
    Ok(())
}
