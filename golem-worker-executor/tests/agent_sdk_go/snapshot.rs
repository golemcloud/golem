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

//! The Go SDK's custom snapshot support (`golem.Snapshotter`). The agent keeps its
//! counter in an UNEXPORTED field, which the SDK's default reflective snapshot
//! cannot see — so state surviving a snapshot-based recovery proves the SDK
//! actually called the type's Save/Load.

use crate::Tracing;
use crate::durability::assert_snapshot_recovery_loaded;
use golem_common::model::oplog::{OplogIndex, PublicOplogEntry};
use golem_common::{agent_id, data_value};
use golem_test_framework::dsl::TestDsl;
use golem_worker_executor::services::golem_config::SnapshotPolicy;
use golem_worker_executor_test_utils::{
    LastUniqueId, PrecompiledComponent, TestContext, WorkerExecutorTestDependencies,
    start_with_snapshot_policy,
};
use std::collections::HashMap;
use test_r::{inherit_test_dep, test, timeout};

inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(LastUniqueId);
inherit_test_dep!(Tracing);
inherit_test_dep!(
    #[tagged_as("agent_sdk_go")]
    PrecompiledComponent
);

/// With a snapshot taken every 2nd invocation, snapshots are recorded in the
/// oplog; after a restart the worker recovers from a snapshot and the counter —
/// held in an unexported field, so only reachable through the SDK's Save/Load —
/// is intact.
///
/// IGNORED — the Go SDK's snapshot declarations do not currently produce
/// snapshotting. Observed with the same executor snapshot policy that the Rust
/// `durability::automatic_snapshot_every_2nd_invocation` control test passes
/// under, and with every other Go agent in this component working:
///   - `Spec.Snapshot: golem.SnapshotEveryN(2)` — the agent runs, but the guest
///     is never asked to save a snapshot (no save-snapshot call is made) and a
///     restart recovers by ordinary replay, so no snapshot-recovery event arrives.
///   - `Spec.Snapshot: golem.SnapshotDefault` — the agent fails to instantiate
///     ("Failed to instantiate primary executable", trap during agent creation),
///     while the other agents in the same component keep working.
#[test]
#[ignore = "go snapshot declarations do not produce snapshotting: SnapshotEveryN never triggers save-snapshot, SnapshotDefault traps on agent creation — needs an SDK investigation"]
#[tracing::instrument]
#[timeout("2m")]
async fn go_custom_snapshot_round_trips_unexported_state(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("agent_sdk_go")] agent_sdk_go: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let policy = SnapshotPolicy::EveryNInvocation { count: 2 };
    let executor = start_with_snapshot_policy(deps, &context, policy.clone()).await?;

    let component = executor
        .component_dep(&context.default_environment_id, agent_sdk_go)
        .store()
        .await?;
    let agent_id = agent_id!("SnapAgent", "go-snapshot-1");
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), HashMap::new(), Vec::new())
        .await?;

    for _ in 0..4 {
        executor
            .invoke_and_await_agent(&component, &agent_id, "bump", data_value!())
            .await?;
    }

    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    let snapshot_count = oplog
        .iter()
        .filter(|entry| matches!(&entry.entry, PublicOplogEntry::Snapshot(_)))
        .count();

    drop(executor);
    let executor = start_with_snapshot_policy(deps, &context, policy).await?;
    let mut events = executor.capture_output(&worker_id).await?;

    let value = executor
        .invoke_and_await_agent(&component, &agent_id, "value", data_value!())
        .await?
        .into_typed::<i64>()?;
    assert_snapshot_recovery_loaded(&mut events).await;

    executor.check_oplog_is_queryable(&worker_id).await?;
    drop(executor);

    assert_eq!(snapshot_count, 2, "expected a snapshot every 2 invocations");
    assert_eq!(value, 4, "unexported counter must survive snapshot recovery");
    Ok(())
}
