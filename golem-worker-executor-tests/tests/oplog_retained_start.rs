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
use golem_common::model::agent::{ParsedAgentKey, RevertLastInvocations, RevertWorkerTarget};
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

fn entries_from(
    entries: &[PublicOplogEntryWithIndex],
    start: OplogIndex,
) -> Vec<PublicOplogEntryWithIndex> {
    entries
        .iter()
        .filter(|entry| entry.oplog_index >= start)
        .cloned()
        .collect()
}

/// Whether `text` names `retained_start` as the start of an expired oplog: the expired text of
/// the agent error, or the `retained_start` field of the gRPC error. A start with more digits,
/// such as 40 for 4, does not match.
fn names_expired_start(text: &str, retained_start: OplogIndex) -> bool {
    [
        format!("expired: it starts at index {retained_start}"),
        format!("retained_start: {}", retained_start.as_u64()),
    ]
    .iter()
    .any(|needle| {
        text.match_indices(needle.as_str())
            .any(|(at, _)| !text[at + needle.len()..].starts_with(|c: char| c.is_ascii_digit()))
    })
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
/// after a real restart. The test removes the cached status and the invocation result index
/// before the restart, so the executor answers the status and the repeated call by idempotency
/// key from the oplog after the start. The agent answers pages from the start with their original
/// indexes, the `Create` entry alone, and search. The executor refuses a page before the start as
/// expired with the start. The repeated call by key runs nothing: each call of `increment` has one
/// start and one finish. The executor refuses a fork of the agent as expired. A revert of more
/// invocations than the oplog keeps stops at the start. A revert of the last invocation reads the
/// whole history, so the executor refuses it as expired. An agent whose oplog starts at index 1
/// reverts its last call in the same executor.
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
    let retained = entries_from(&history, retained_start);
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
        expired.contains("OplogExpired") && names_expired_start(&expired, retained_start),
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
    let expired = format!("{expired:#}");
    assert!(
        expired.contains("OplogExpired") && names_expired_start(&expired, retained_start),
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

    let last = page
        .last()
        .map(|entry| entry.oplog_index)
        .unwrap_or(retained_start);
    let refused = executor
        .fork_worker(&agent_id, "retained-start-fork", last)
        .await
        .expect_err("a fork copies the history from index 1");
    let refused = format!("{refused:#}");
    assert!(names_expired_start(&refused, retained_start), "{refused}");

    let too_far = executor
        .revert(
            &agent_id,
            RevertWorkerTarget::RevertLastInvocations(RevertLastInvocations {
                number_of_invocations: 100,
            }),
        )
        .await
        .expect_err("the revert walk stops at the start");
    let too_far = format!("{too_far:#}");
    assert!(
        too_far.contains("Could not find 100 invocations to revert"),
        "{too_far}"
    );
    let refused = executor
        .revert(
            &agent_id,
            RevertWorkerTarget::RevertLastInvocations(RevertLastInvocations {
                number_of_invocations: 1,
            }),
        )
        .await
        .expect_err("the revert reads the whole history from index 1");
    let refused = format!("{refused:#}");
    assert!(names_expired_start(&refused, retained_start), "{refused}");
    let value = executor
        .invoke_and_await_agent(&component, &agent, "get", data_value!())
        .await?
        .into_typed::<u32>()?;
    assert_eq!(value, 4, "the refused revert changed nothing");

    // An agent whose oplog starts at index 1 reverts its last call in the same executor.
    let control = agent_id!("FailingCounter", "retained-start-control");
    let control_id = executor.start_agent(&component.id, control.clone()).await?;
    executor
        .invoke_and_await_agent(&component, &control, "add", data_value!(5u64))
        .await?;
    executor
        .invoke_and_await_agent(&component, &control, "add", data_value!(7u64))
        .await?;
    executor
        .revert(
            &control_id,
            RevertWorkerTarget::RevertLastInvocations(RevertLastInvocations {
                number_of_invocations: 1,
            }),
        )
        .await?;
    let reverted = executor.get_oplog(&control_id, OplogIndex::INITIAL).await?;
    assert!(
        matches!(
            reverted.last().map(|entry| &entry.entry),
            Some(PublicOplogEntry::Revert(_))
        ),
        "{:?}",
        reverted.last()
    );
    let total = executor
        .invoke_and_await_agent(&component, &control, "get", data_value!())
        .await?
        .into_typed::<u64>()?;
    assert_eq!(total, 5, "the revert removed the last `add`");
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
    assert!(names_expired_start(&failure, retained_start), "{failure}");

    let create = executor
        .get_oplog_entry(
            &agent_id,
            context.default_environment_id,
            OplogIndex::INITIAL,
            None,
        )
        .await?;
    assert_eq!(create, history[0], "the `Create` entry is kept");
    let kept = executor.get_oplog(&agent_id, retained_start).await?;
    let retained = entries_from(&history, retained_start);
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
        names_expired_start(&last_error, retained_start),
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
