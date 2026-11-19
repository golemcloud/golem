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

//! An agent oplog whose retained start is after index 1. The start is written to the storage
//! that the executor reads at startup while no executor runs, and every check runs after a real
//! restart.

use crate::Tracing;

use golem_common::model::oplog::{OplogIndex, PublicOplogEntry, PublicOplogEntryWithIndex};
use golem_common::model::{AgentId, AgentStatus, IdempotencyKey, OwnedAgentId};
use golem_common::{agent_id, data_value};
use golem_test_framework::dsl::TestDsl;
use golem_worker_executor::services::golem_config::SnapshotPolicy;
use golem_worker_executor_test_utils::{
    LastUniqueId, PrecompiledComponent, TestContext, TestWorkerExecutor,
    WorkerExecutorTestDependencies, force_oplog_retained_start, start_with_snapshot_policy,
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

/// The index of the last snapshot in the oplog of the agent, after waiting for one.
async fn last_snapshot_index(
    executor: &TestWorkerExecutor,
    agent_id: &AgentId,
) -> anyhow::Result<OplogIndex> {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let snapshot = executor
                .get_oplog(agent_id, OplogIndex::INITIAL)
                .await?
                .into_iter()
                .rev()
                .find(|entry| matches!(&entry.entry, PublicOplogEntry::Snapshot(_)));
            if let Some(snapshot) = snapshot {
                return Ok::<_, anyhow::Error>(snapshot.oplog_index);
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .map_err(|_| anyhow::anyhow!("no snapshot was persisted in time"))?
}

fn from(
    entries: &[PublicOplogEntryWithIndex],
    start: OplogIndex,
) -> Vec<PublicOplogEntryWithIndex> {
    entries
        .iter()
        .filter(|entry| entry.oplog_index >= start)
        .cloned()
        .collect()
}

/// An agent whose oplog starts at the bootstrap range of its snapshot recovers from the snapshot
/// after a real restart. The bootstrap range is the first durable calls of the agent, so the start
/// is the first `Start` entry; history before it is not served until a side record keeps the
/// bootstrap range. The agent answers its status, a repeated call by idempotency key, pages from the start with their
/// original indexes, and search. A page before the start is refused as expired with the start.
#[test]
#[timeout("4m")]
#[tracing::instrument]
async fn an_agent_whose_oplog_starts_at_its_snapshot_recovers_and_answers_reads(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_counters")] agent_counters: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let policy = SnapshotPolicy::EveryNInvocation { count: 2 };
    let executor = start_with_snapshot_policy(deps, &context, policy.clone()).await?;
    let component = executor
        .component_dep(&context.default_environment_id, agent_counters)
        .store()
        .await?;
    let agent = agent_id!("JsonSnapshotCounter", "retained-start");
    let agent_id = executor.start_agent(&component.id, agent.clone()).await?;
    let owned_agent_id = OwnedAgentId::new(context.default_environment_id, &agent_id);

    for _ in 0..3 {
        executor
            .invoke_and_await_agent(&component, &agent, "increment", data_value!())
            .await?;
    }
    let snapshot = last_snapshot_index(&executor, &agent_id).await?;
    let retained_start = executor
        .get_oplog(&agent_id, OplogIndex::INITIAL)
        .await?
        .into_iter()
        .find(|entry| matches!(&entry.entry, PublicOplogEntry::Start(_)))
        .map(|entry| entry.oplog_index)
        .ok_or_else(|| anyhow::anyhow!("the agent made no durable call"))?;
    assert!(
        OplogIndex::INITIAL < retained_start && retained_start < snapshot,
        "start {retained_start}, snapshot {snapshot}"
    );
    let key = IdempotencyKey::fresh();
    let keyed = executor
        .invoke_and_await_agent_with_key(&component, &agent, &key, "increment", data_value!())
        .await?
        .into_typed::<u32>()?;
    assert_eq!(keyed, 4);
    let history = executor.get_oplog(&agent_id, OplogIndex::INITIAL).await?;
    drop(executor);

    force_oplog_retained_start(deps, &context, &owned_agent_id, retained_start).await?;
    let executor = start_with_snapshot_policy(deps, &context, policy).await?;

    let value = executor
        .invoke_and_await_agent(&component, &agent, "get", data_value!())
        .await?
        .into_typed::<u32>()?;
    assert_eq!(value, 4);
    assert_eq!(
        executor.get_worker_metadata(&agent_id).await?.status,
        AgentStatus::Idle
    );

    let repeated = executor
        .invoke_and_await_agent_with_key(&component, &agent, &key, "increment", data_value!())
        .await?
        .into_typed::<u32>()?;
    assert_eq!(
        repeated, 4,
        "the call by key answers from its stored result"
    );
    let value = executor
        .invoke_and_await_agent(&component, &agent, "get", data_value!())
        .await?
        .into_typed::<u32>()?;
    assert_eq!(value, 4, "the repeated call by key ran no second increment");

    let page = executor.get_oplog(&agent_id, retained_start).await?;
    assert_eq!(
        page.first().map(|entry| entry.oplog_index),
        Some(retained_start)
    );
    let retained = from(&history, retained_start);
    assert_eq!(
        page[..retained.len()],
        retained[..],
        "the entries keep their original indexes"
    );

    let expired = executor
        .get_oplog(&agent_id, OplogIndex::INITIAL)
        .await
        .expect_err("a page before the start is refused");
    let expired = format!("{expired:#}");
    assert!(
        expired.contains("OplogExpired") && expired.contains(&format!("{retained_start}")),
        "{expired}"
    );

    let found = executor.search_oplog(&agent_id, "increment").await?;
    assert!(!found.is_empty());
    assert!(
        found
            .iter()
            .all(|entry| entry.oplog_index >= retained_start),
        "{found:?}"
    );
    Ok(())
}

/// When no restore source of an agent is usable, because no snapshot is at or after the retained
/// start, recovery fails with the expired error that names the start. The oplog is kept: every
/// retained entry is unchanged, and the only new entries are the pending call that started the
/// recovery and the errors that recovery records.
#[test]
#[timeout("4m")]
#[tracing::instrument]
async fn a_recovery_without_a_usable_restore_source_fails_and_keeps_the_oplog(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_counters")] agent_counters: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start_with_snapshot_policy(deps, &context, SnapshotPolicy::Disabled).await?;
    let component = executor
        .component_dep(&context.default_environment_id, agent_counters)
        .store()
        .await?;
    let agent = agent_id!("JsonSnapshotCounter", "no-restore-source");
    let agent_id = executor.start_agent(&component.id, agent.clone()).await?;
    let owned_agent_id = OwnedAgentId::new(context.default_environment_id, &agent_id);
    for _ in 0..3 {
        executor
            .invoke_and_await_agent(&component, &agent, "increment", data_value!())
            .await?;
    }
    let history = executor.get_oplog(&agent_id, OplogIndex::INITIAL).await?;
    drop(executor);

    let retained_start = OplogIndex::from_u64(3);
    force_oplog_retained_start(deps, &context, &owned_agent_id, retained_start).await?;
    let executor = start_with_snapshot_policy(deps, &context, SnapshotPolicy::Disabled).await?;

    let failure = executor
        .invoke_and_await_agent(&component, &agent, "get", data_value!())
        .await
        .expect_err("the recovery has no usable restore source");
    let failure = format!("{failure:#}");
    assert!(
        failure.contains(&format!("starts at index {retained_start}")),
        "{failure}"
    );

    let kept = executor.get_oplog(&agent_id, retained_start).await?;
    let retained = from(&history, retained_start);
    assert_eq!(kept[..retained.len()], retained[..]);
    assert!(
        kept[retained.len()..].iter().all(|entry| matches!(
            entry.entry,
            PublicOplogEntry::Error(_) | PublicOplogEntry::PendingAgentInvocation(_)
        )),
        "{:?}",
        &kept[retained.len()..]
    );
    Ok(())
}
