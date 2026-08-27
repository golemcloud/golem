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

//! Scheduled invocations from a Go guest: `MethodDef.Schedule` runs a cross-agent
//! call later without the caller waiting, and `ScheduledInvocation.Cancel` stops
//! one that has not started yet.

use crate::Tracing;
use golem_common::{agent_id, data_value};
use golem_test_framework::dsl::TestDsl;
use golem_worker_executor_test_utils::{
    LastUniqueId, PrecompiledComponent, TestContext, WorkerExecutorTestDependencies, start,
};
use std::collections::HashMap;
use std::time::Duration;
use test_r::{inherit_test_dep, test, timeout};

inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(LastUniqueId);
inherit_test_dep!(Tracing);
inherit_test_dep!(
    #[tagged_as("agent_sdk_go")]
    PrecompiledComponent
);

/// A scheduled increment runs on its own: the caller returns immediately and the
/// target counter moves once the scheduled time passes.
#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn go_scheduled_invocation_runs(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("agent_sdk_go")] agent_sdk_go: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component_dep(&context.default_environment_id, agent_sdk_go)
        .store()
        .await?;
    let scheduler_id = agent_id!("SchedulerAgent", "go-sched-1");
    let counter_id = agent_id!("CounterAgent", "go-sched-target-1");
    executor
        .start_agent_with(&component.id, scheduler_id.clone(), HashMap::new(), Vec::new())
        .await?;

    executor
        .invoke_and_await_agent(
            &component,
            &scheduler_id,
            "bump",
            data_value!("go-sched-target-1", 500i64),
        )
        .await?;

    // The scheduled call runs on its own schedule, so poll for its effect.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    let mut value = 0;
    while tokio::time::Instant::now() < deadline {
        value = executor
            .invoke_and_await_agent(&component, &counter_id, "value", data_value!())
            .await?
            .into_typed::<i64>()?;
        if value >= 1 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    drop(executor);

    assert_eq!(value, 1, "the scheduled increment should have run");
    Ok(())
}

/// Cancelling a scheduled invocation before its time arrives prevents it: the
/// target counter never moves.
#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn go_scheduled_invocation_can_be_cancelled(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("agent_sdk_go")] agent_sdk_go: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component_dep(&context.default_environment_id, agent_sdk_go)
        .store()
        .await?;
    let scheduler_id = agent_id!("SchedulerAgent", "go-sched-2");
    let counter_id = agent_id!("CounterAgent", "go-sched-target-2");
    executor
        .start_agent_with(&component.id, scheduler_id.clone(), HashMap::new(), Vec::new())
        .await?;

    executor
        .invoke_and_await_agent(
            &component,
            &scheduler_id,
            "bump-cancelled",
            data_value!("go-sched-target-2", 500i64),
        )
        .await?;

    // Well past the scheduled time: the cancelled invocation must not have run.
    tokio::time::sleep(Duration::from_secs(3)).await;
    let value = executor
        .invoke_and_await_agent(&component, &counter_id, "value", data_value!())
        .await?
        .into_typed::<i64>()?;
    drop(executor);

    assert_eq!(value, 0, "the cancelled increment must not have run");
    Ok(())
}
