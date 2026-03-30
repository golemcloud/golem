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
use golem_common::model::oplog::public_oplog_entry::AgentInvocationFinishedParams;
use golem_common::model::oplog::{OplogIndex, PublicOplogEntry};
use golem_common::{agent_id, data_value};
use golem_test_framework::dsl::TestDsl;
use golem_worker_executor_test_utils::{
    LastUniqueId, PrecompiledComponent, TestContext, WorkerExecutorTestDependencies,
    start_with_fuel_tracking,
};
use test_r::{inherit_test_dep, test, timeout};

inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(LastUniqueId);
inherit_test_dep!(Tracing);
inherit_test_dep!(
    #[tagged_as("host_api_tests")]
    PrecompiledComponent
);

/// Extracts the `consumed_fuel` values from all `AgentInvocationFinished`
/// entries in an oplog, in order.
fn consumed_fuel_from_oplog(
    oplog: &[golem_common::model::oplog::PublicOplogEntryWithIndex],
) -> Vec<i64> {
    oplog
        .iter()
        .filter_map(|e| {
            if let PublicOplogEntry::AgentInvocationFinished(AgentInvocationFinishedParams {
                consumed_fuel,
                ..
            }) = &e.entry
            {
                Some(*consumed_fuel)
            } else {
                None
            }
        })
        .collect()
}

#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn fuel_is_consumed_during_invocation(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    // Verifies that consumed_fuel in the oplog is non-zero after a real
    // invocation — i.e. borrow_fuel was called during epoch ticks and the
    // net consumption was recorded correctly.
    let context = TestContext::new(last_unique_id);
    let executor = start_with_fuel_tracking(deps, &context).await?;

    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;

    let agent_id = agent_id!("Clocks", "fuel-consumed-1");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    // Sleep for 50ms — guaranteed to span multiple epoch ticks (default 10ms each)
    executor
        .invoke_and_await_agent(&component, &agent_id, "sleep_for", data_value!(0.05f64))
        .await?;

    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    let fuel_values = consumed_fuel_from_oplog(&oplog);

    assert!(
        !fuel_values.is_empty(),
        "expected at least one AgentInvocationFinished entry in oplog"
    );
    assert!(
        fuel_values.iter().any(|&f| f > 0),
        "expected consumed_fuel > 0 after a real invocation, got: {fuel_values:?}"
    );

    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn each_invocation_fuel_is_accounted_independently(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    // Verifies that fuel is correctly returned at invocation end and does not
    // leak into the next invocation. Two identical invocations must report
    // approximately equal consumed_fuel (within one fuel_to_borrow unit).
    let context = TestContext::new(last_unique_id);
    let executor = start_with_fuel_tracking(deps, &context).await?;

    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;

    let agent_id = agent_id!("Clocks", "fuel-independent-1");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    // Two identical 50ms invocations
    executor
        .invoke_and_await_agent(&component, &agent_id, "sleep_for", data_value!(0.05f64))
        .await?;
    executor
        .invoke_and_await_agent(&component, &agent_id, "sleep_for", data_value!(0.05f64))
        .await?;

    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    let fuel_values = consumed_fuel_from_oplog(&oplog);

    // Take the last two entries (skipping the constructor invocation which comes first).
    let last_two: Vec<i64> = fuel_values.into_iter().rev().take(2).collect();
    assert!(
        last_two.len() == 2,
        "expected at least 2 AgentInvocationFinished entries in oplog"
    );
    let (second, first) = (last_two[0], last_two[1]);

    assert!(first > 0, "first invocation must consume fuel, got {first}");
    assert!(
        second > 0,
        "second invocation must consume fuel, got {second}"
    );

    // Both identical invocations should have similar fuel consumption.
    // Variance comes from ±1 epoch tick of scheduler jitter (~10,000 units on a
    // ~50,000 base), so 2× is a generous but realistic bound.
    let ratio = (first as f64 / second as f64).max(second as f64 / first as f64);
    assert!(
        ratio < 2.0,
        "fuel consumption for identical invocations should be similar: first={first} second={second} ratio={ratio:.2}"
    );

    Ok(())
}
