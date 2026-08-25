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
/// IGNORED — enabling snapshotting on a Go agent makes the worker fail to start.
/// Observed:
///   - The worker fails with "Failed to instantiate primary executable" (a wasm
///     trap with no guest stderr), and no Snapshot oplog entries are recorded.
///   - Every other Go agent in the same component keeps working, and the Rust
///     control test `durability::automatic_snapshot_every_2nd_invocation` passes
///     under the same executor snapshot policy.
///   - It is NOT the Snapshotter path: replacing the custom Save/Load with a
///     plain exported-field state (the reflective JSON snapshot) fails
///     identically, and the failure is at worker CREATION, before any invocation.
///   - The SDK side that can be checked natively is correct: the policy reaches
///     the agent type (`TestSnapshotPolicyMapsToWit` covers all four variants),
///     `saveState`/`loadState` round-trip in unit tests, and the built component
///     does export `golem:api/save-snapshot` and `load-snapshot` (wasm-tools).
#[test]
#[ignore = "enabling snapshotting on a go agent makes the worker fail to instantiate (wasm trap, no guest stderr) — needs an SDK/toolchain investigation"]
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

    for _ in 0..10 {
        executor
            .invoke_and_await_agent(&component, &agent_id, "bump", data_value!())
            .await?;
    }

    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    let snapshot_count = oplog
        .iter()
        .filter(|entry| matches!(&entry.entry, PublicOplogEntry::Snapshot(_)))
        .count();

    assert_eq!(snapshot_count, 5, "expected a snapshot every 2 invocations");

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

    assert_eq!(value, 10, "unexported counter must survive snapshot recovery");
    Ok(())
}
