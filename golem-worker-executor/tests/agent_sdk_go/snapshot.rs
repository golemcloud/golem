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
/// IGNORED until CI builds Go components with golem's Go toolchain fork
/// (`tmp/go-runtime-sampler-wasip1.patch`: goroutine scheduling-latency sampling
/// disabled on wasip1). Why it is needed: Go's `casgstatus` reads the clock on
/// every 8th transition of a goroutine, counted over the goroutine's whole life.
/// Snapshot recovery skips history and the save/load hooks run outside the
/// recording, so the long-lived event-loop goroutine's counter can never match
/// the one the recording was made with, and a `monotonic_clock::now` lands in a
/// different invocation: replay diverges ~15% on go1.25.5, ~80% on go1.27.1. The
/// sampler feeds two metrics no runtime decision reads; with it off this test is
/// 20/20 on go1.27.1. The executor half (the skipped bootstrap prefix re-seeding
/// the runtime's PRNG) is fixed separately (GOL-611). Measured with
/// `diagnostics.rs`; write-up in `tmp/snapshot-divergence-explainer.html`.
#[test]
#[ignore = "needs golem's go toolchain fork (tmp/go-runtime-sampler-wasip1.patch); replay diverges ~80% on the stock go1.27.1 fork CI still uses"]
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

    assert_eq!(
        value, 10,
        "unexported counter must survive snapshot recovery"
    );
    Ok(())
}
