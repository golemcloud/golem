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

//! Recovery of `SnapshotMismatchAgent`, whose custom snapshot load derives a value differently
//! from the live code. Once the agent runs live from such a load, the results it records cannot
//! be reproduced by a replay from an older restore point, and recovery fails with a divergence.

use crate::Tracing;
use golem_common::model::agent::{AgentMetadataDto, ParsedAgentId};
use golem_common::model::component::ComponentDto;
use golem_common::model::oplog::{OplogErrorKind, OplogIndex, PublicOplogEntry};
use golem_common::model::{AgentId, AgentStatus};
use golem_common::{agent_id, data_value};
use golem_test_framework::dsl::TestDsl;
use golem_worker_executor_test_utils::{
    LastUniqueId, PrecompiledComponent, TestContext, TestWorkerExecutor,
    WorkerExecutorTestDependencies, start,
};
use std::time::Duration;
use test_r::{inherit_test_dep, test, timeout};

inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(LastUniqueId);
inherit_test_dep!(Tracing);
inherit_test_dep!(
    #[tagged_as("agent_sdk_ts")]
    PrecompiledComponent
);

const SNAPSHOT_HINT: &str = "a snapshot load that differs from the live state is a common cause";

/// Calls `add` with `len` characters.
async fn add(
    executor: &TestWorkerExecutor,
    component: &ComponentDto,
    agent_id: &ParsedAgentId,
    len: usize,
) -> anyhow::Result<f64> {
    executor
        .invoke_and_await_agent(component, agent_id, "add", data_value!("x".repeat(len)))
        .await?
        .into_typed::<f64>()
}

/// The latest snapshot, when it follows the last finished invocation.
async fn snapshot_at_tail(
    executor: &TestWorkerExecutor,
    worker_id: &AgentId,
) -> anyhow::Result<Option<OplogIndex>> {
    for _ in 0..50 {
        let oplog = executor.get_oplog(worker_id, OplogIndex::INITIAL).await?;
        let last = |snapshot: bool| {
            oplog
                .iter()
                .rfind(|entry| match &entry.entry {
                    PublicOplogEntry::Snapshot(_) => snapshot,
                    PublicOplogEntry::AgentInvocationFinished(_) => !snapshot,
                    _ => false,
                })
                .map(|entry| entry.oplog_index)
        };
        if let (Some(snapshot), Some(finished)) = (last(true), last(false))
            && snapshot > finished
        {
            return Ok(Some(snapshot));
        }
        tokio::time::sleep(Duration::from_millis(40)).await;
    }
    Ok(None)
}

/// Calls `add` until the agent takes `count` new snapshots, and returns the length of the last
/// added string.
async fn add_until_snapshots(
    executor: &TestWorkerExecutor,
    component: &ComponentDto,
    agent_id: &ParsedAgentId,
    worker_id: &AgentId,
    mut len: usize,
    count: usize,
) -> anyhow::Result<usize> {
    let mut seen = snapshot_at_tail(executor, worker_id).await?;
    let mut taken = 0;
    while taken < count {
        len += 1;
        add(executor, component, agent_id, len).await?;
        let latest = snapshot_at_tail(executor, worker_id).await?;
        if latest.is_some() && latest != seen {
            seen = latest;
            taken += 1;
        }
    }
    Ok(len)
}

/// Runs the agent live from a snapshot whose load differs from the live state, then takes two
/// newer snapshots and one more invocation, and restarts the executor. Recovery rejects both
/// newer snapshots and replays from the authoritative baseline of the agent, which diverges at
/// the first result that the live agent computed from the faulty load. Gives the metadata of the
/// failed agent and the oplog indexes of its snapshots.
async fn fail_recovery_after_a_live_restore(
    deps: &WorkerExecutorTestDependencies,
    context: &TestContext,
    executor: TestWorkerExecutor,
    component: &ComponentDto,
    agent_id: &ParsedAgentId,
    worker_id: &AgentId,
    len: usize,
) -> anyhow::Result<(AgentMetadataDto, Vec<OplogIndex>)> {
    let len = add_until_snapshots(&executor, component, agent_id, worker_id, len, 1).await?;
    drop(executor);

    let executor = start(deps, context).await?;
    let len = add_until_snapshots(&executor, component, agent_id, worker_id, len, 2).await?;
    add(&executor, component, agent_id, len + 1).await?;
    drop(executor);

    let executor = start(deps, context).await?;
    let resumed = executor
        .invoke_and_await_agent(component, agent_id, "add", data_value!("y"))
        .await;
    assert!(
        resumed.is_err(),
        "a replay that diverges must not resume the agent: {resumed:?}"
    );
    let metadata = executor.get_worker_metadata(worker_id).await?;
    assert_eq!(metadata.status, AgentStatus::Failed);
    assert_eq!(metadata.last_error_kind, Some(OplogErrorKind::Recovery));
    let snapshots = executor
        .get_oplog(worker_id, OplogIndex::INITIAL)
        .await?
        .iter()
        .filter(|entry| matches!(entry.entry, PublicOplogEntry::Snapshot(_)))
        .map(|entry| entry.oplog_index)
        .collect();
    Ok((metadata, snapshots))
}

#[test]
#[tracing::instrument]
#[timeout("4m")]
async fn ts_full_replay_after_rejected_snapshots_names_them(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("agent_sdk_ts")] agent_sdk_ts: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;
    let component = executor
        .component_dep(&context.default_environment_id, agent_sdk_ts)
        .store()
        .await?;
    let agent_id = agent_id!("SnapshotMismatchAgent", "full-replay");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    // The first snapshot follows `add("x")`; its load sets approx to the length of `["x"]`, 5,
    // so the live agent then returns 5 + 2 = 7 for `add("xx")`, where a replay returns 1 + 2 = 3.
    let (metadata, snapshots) = fail_recovery_after_a_live_restore(
        deps, &context, executor, &component, &agent_id, &worker_id, 0,
    )
    .await?;

    // Recovery rejects the two newest snapshots, the candidates of its start, and then replays
    // without a snapshot.
    let [.., older, newer] = snapshots[..] else {
        panic!("the agent has at least two snapshots: {snapshots:?}");
    };
    assert_eq!(
        metadata.last_error,
        Some(format!(
            "Unexpected oplog entry during replay: expected add => AgentMethod {{ output: F64(7.0) }}, got add => AgentMethod {{ output: F64(3.0) }}; replay started without a snapshot after the snapshots at oplog indexes {older}, {newer} were rejected; {SNAPSHOT_HINT}"
        ))
    );
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("4m")]
async fn ts_divergence_after_a_snapshot_names_the_snapshot(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("agent_sdk_ts")] agent_sdk_ts: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;
    let component = executor
        .component_dep(&context.default_environment_id, agent_sdk_ts)
        .store()
        .await?;
    let agent_id = agent_id!("SnapshotMismatchAgent", "update-baseline");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;
    add(&executor, &component, &agent_id, 1).await?;

    // A snapshot-based manual update makes its snapshot the authoritative baseline: recovery
    // never replays the history before it.
    let updated = executor
        .update_component(&component.id, "golem_it_agent_sdk_ts")
        .await?;
    executor
        .manual_update_worker(&worker_id, updated.revision, false)
        .await?;
    executor
        .wait_for_component_revision(&worker_id, updated.revision, Duration::from_secs(30))
        .await?;
    let baseline = executor
        .get_oplog(&worker_id, OplogIndex::INITIAL)
        .await?
        .iter()
        .find_map(|entry| {
            matches!(entry.entry, PublicOplogEntry::PendingUpdate(_)).then_some(entry.oplog_index)
        })
        .expect("the manual update has a pending-update entry");

    let (metadata, snapshots) = fail_recovery_after_a_live_restore(
        deps, &context, executor, &updated, &agent_id, &worker_id, 1,
    )
    .await?;

    // Recovery rejects the two newest snapshots, whose loads differ from the live state, and
    // then replays from the baseline, which the live agent also ran from.
    let [.., older, newer] = snapshots[..] else {
        panic!("the agent has at least two snapshots: {snapshots:?}");
    };
    let error = metadata.last_error.expect("a failed agent has an error");
    assert!(
        error.starts_with("Unexpected oplog entry during replay: expected add => AgentMethod"),
        "{error}"
    );
    assert!(
        error.ends_with(&format!(
            "; replay started from the snapshot at oplog index {baseline} after the snapshots at oplog indexes {older}, {newer} were rejected; {SNAPSHOT_HINT}"
        )),
        "{error}"
    );
    Ok(())
}
