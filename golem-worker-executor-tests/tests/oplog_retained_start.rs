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
use crate::hot_update::wait_for_snapshot_after;

use futures::{StreamExt, TryStreamExt};
use golem_common::model::agent::ParsedAgentKey;
use golem_common::model::component::ComponentDto;
use golem_common::model::oplog::public_oplog_entry::AgentInvocationStartedParams;
use golem_common::model::oplog::{
    OplogIndex, PublicAgentInvocation, PublicOplogEntry, PublicOplogEntryWithIndex,
};
use golem_common::model::{AgentStatus, IdempotencyKey, InvocationId, OwnedAgentId};
use golem_common::{agent_id, data_value};
use golem_test_framework::dsl::TestDsl;
use golem_worker_executor::services::golem_config::SnapshotPolicy;
use golem_worker_executor_test_utils::{
    LastUniqueId, PrecompiledComponent, TestContext, TestWorkerExecutor,
    WorkerExecutorTestDependencies, force_oplog_retained_start,
    remove_cached_status_and_result_index, start_with_snapshot_policy,
};
use pretty_assertions::assert_eq;
use test_r::{inherit_test_dep, test, timeout};

inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(LastUniqueId);
inherit_test_dep!(
    #[tagged_as("agent_counters")]
    PrecompiledComponent
);
inherit_test_dep!(Tracing);

/// Calls `increment` on the agent `times` times, one call after the other.
async fn increment(
    executor: &TestWorkerExecutor,
    component: &ComponentDto,
    agent: &ParsedAgentKey,
    times: usize,
) -> anyhow::Result<()> {
    futures::stream::iter(0..times)
        .then(|_| executor.invoke_and_await_agent(component, agent, "increment", data_value!()))
        .try_for_each(|_| async { Ok(()) })
        .await
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

/// The invocations of `increment` that the entries start, and the invocations that they finish.
fn increments(entries: &[PublicOplogEntryWithIndex]) -> (Vec<InvocationId>, Vec<InvocationId>) {
    let started = entries
        .iter()
        .filter_map(|entry| match &entry.entry {
            PublicOplogEntry::AgentInvocationStarted(AgentInvocationStartedParams {
                invocation: PublicAgentInvocation::AgentMethodInvocation(method),
                ..
            }) if method.method_name == "increment" => Some(method.invocation_id),
            _ => None,
        })
        .collect();
    let finished = entries
        .iter()
        .filter_map(|entry| match &entry.entry {
            PublicOplogEntry::AgentInvocationFinished(finished)
                if finished.method_name.as_deref() == Some("increment") =>
            {
                Some(finished.invocation_id)
            }
            _ => None,
        })
        .collect();
    (started, finished)
}

/// The index of the first durable call of the agent: the start of the bootstrap range of its
/// snapshots.
fn first_start(entries: &[PublicOplogEntryWithIndex]) -> anyhow::Result<OplogIndex> {
    entries
        .iter()
        .find(|entry| matches!(&entry.entry, PublicOplogEntry::Start(_)))
        .map(|entry| entry.oplog_index)
        .ok_or_else(|| anyhow::anyhow!("the agent made no durable call"))
}

/// An agent whose oplog starts at the bootstrap range of its snapshot recovers from the snapshot
/// after a real restart. The cached status and the invocation result index are removed before
/// the restart, so the executor answers the status and the repeated call by idempotency key from
/// the oplog after the start. The agent answers pages from the start with their original indexes,
/// the `Create` entry alone, and search. A page before the start is refused as expired with the
/// start. The repeated call by key runs nothing: each call of `increment` has one start and one
/// finish.
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

    increment(&executor, &component, &agent, 3).await?;
    let snapshot = wait_for_snapshot_after(&executor, &agent_id, OplogIndex::NONE).await?;
    let retained_start = first_start(&executor.get_oplog(&agent_id, OplogIndex::INITIAL).await?)?;
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
    let fingerprint = executor.get_worker_metadata(&agent_id).await?.fingerprint;
    drop(executor);

    force_oplog_retained_start(deps, &context, &owned_agent_id, retained_start).await?;
    remove_cached_status_and_result_index(deps, &context, &owned_agent_id, fingerprint).await?;
    let executor = start_with_snapshot_policy(deps, &context, policy).await?;

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
    let (started, finished) = increments(&page);
    assert_eq!(
        (started.clone(), finished.clone()),
        increments(&retained),
        "the repeated call by key started no invocation"
    );
    assert_eq!(started, finished, "each started increment finished once");

    let create = executor
        .get_oplog_entry(
            &agent_id,
            context.default_environment_id,
            OplogIndex::INITIAL,
            None,
        )
        .await?;
    assert_eq!(create, history[0], "the `Create` entry is served alone");

    let expired = executor
        .get_oplog(&agent_id, OplogIndex::INITIAL)
        .await
        .expect_err("a page before the start is refused");
    let expired = format!("{expired:#}");
    assert!(
        expired.contains("OplogExpired") && expired.contains(&format!("{retained_start}")),
        "{expired}"
    );
    let expired = executor
        .get_oplog_entry(
            &agent_id,
            context.default_environment_id,
            retained_start.previous(),
            None,
        )
        .await
        .expect_err("a single entry before the start is refused");
    assert!(
        format!("{expired:#}").contains("OplogExpired"),
        "{expired:#}"
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

/// Runs an agent with `policy`, moves the retained start of its oplog to the index that
/// `retained_start` picks from its history, and restarts the executor. The recovery has no usable
/// restore source, so it fails with the expired error that names the start. The oplog is kept:
/// every retained entry is unchanged, and the only new entries are the pending call that started
/// the recovery and the errors that recovery records. The error history of the agent reads from
/// the start and reports the failure.
async fn a_recovery_without_a_usable_restore_source_keeps_the_oplog(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    agent_counters: &PrecompiledComponent,
    policy: SnapshotPolicy,
    name: &str,
    retained_start: impl FnOnce(&[PublicOplogEntryWithIndex]) -> anyhow::Result<OplogIndex>,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start_with_snapshot_policy(deps, &context, policy.clone()).await?;
    let component = executor
        .component_dep(&context.default_environment_id, agent_counters)
        .store()
        .await?;
    let agent = agent_id!("JsonSnapshotCounter", name);
    let agent_id = executor.start_agent(&component.id, agent.clone()).await?;
    let owned_agent_id = OwnedAgentId::new(context.default_environment_id, &agent_id);
    increment(&executor, &component, &agent, 3).await?;
    if !matches!(policy, SnapshotPolicy::Disabled) {
        wait_for_snapshot_after(&executor, &agent_id, OplogIndex::NONE).await?;
    }
    let history = executor.get_oplog(&agent_id, OplogIndex::INITIAL).await?;
    drop(executor);

    let retained_start = retained_start(&history)?;
    force_oplog_retained_start(deps, &context, &owned_agent_id, retained_start).await?;
    let executor = start_with_snapshot_policy(deps, &context, policy).await?;

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
    let metadata = executor.get_worker_metadata(&agent_id).await?;
    assert_eq!(metadata.status, AgentStatus::Failed);
    let last_error = metadata.last_error.unwrap_or_default();
    assert!(
        last_error.contains(&format!("starts at index {retained_start}")),
        "{last_error}"
    );
    Ok(())
}

/// No snapshot is at or after the retained start.
#[test]
#[timeout("4m")]
#[tracing::instrument]
async fn a_recovery_without_a_snapshot_fails_and_keeps_the_oplog(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_counters")] agent_counters: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    a_recovery_without_a_usable_restore_source_keeps_the_oplog(
        last_unique_id,
        deps,
        agent_counters,
        SnapshotPolicy::Disabled,
        "no-snapshot",
        |_| Ok(OplogIndex::from_u64(3)),
    )
    .await
}

/// A snapshot is after the retained start, but the start is inside its bootstrap range, so the
/// snapshot is not usable.
#[test]
#[timeout("4m")]
#[tracing::instrument]
async fn a_recovery_with_an_unusable_snapshot_fails_and_keeps_the_oplog(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_counters")] agent_counters: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    a_recovery_without_a_usable_restore_source_keeps_the_oplog(
        last_unique_id,
        deps,
        agent_counters,
        SnapshotPolicy::EveryNInvocation { count: 2 },
        "unusable-snapshot",
        |history| {
            let retained_start = first_start(history)?.next();
            anyhow::ensure!(
                history
                    .iter()
                    .any(|entry| entry.oplog_index > retained_start
                        && matches!(entry.entry, PublicOplogEntry::Snapshot(_))),
                "no snapshot after the start {retained_start}"
            );
            Ok(retained_start)
        },
    )
    .await
}
