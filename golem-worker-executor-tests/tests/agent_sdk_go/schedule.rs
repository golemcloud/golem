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
//! call later without the caller waiting, and `Invocation.Cancel` withdraws one
//! that has not started yet and reports `false` for one already cancelled.

use crate::Tracing;
use golem_common::model::account::AccountId;
use golem_common::model::caller::Caller;
use golem_common::{agent_key, data_value};
use golem_test_framework::dsl::TestDsl;
use golem_worker_executor_test_utils::{
    LastUniqueId, PrecompiledComponent, TestContext, WorkerExecutorTestDependencies, callers, start,
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
    let scheduler_id = agent_key!("SchedulerAgent", "go-sched-1");
    let counter_id = agent_key!("CounterAgent", "go-sched-target-1");
    let scheduler = executor
        .start_agent_with(
            &component.id,
            scheduler_id.clone(),
            HashMap::new(),
            Vec::new(),
        )
        .await?;

    let account = callers::account(AccountId::new());
    executor
        .invoke_and_await_agent_as_caller(
            &component,
            &scheduler_id,
            account.clone(),
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
    assert_eq!(value, 1, "the scheduled increment should have run");

    // The counter recorded the account that started `bump` as the original caller and the
    // scheduler as the immediate caller, for the schedule and for the run.
    let counter = executor.keyed_agent_id(component.environment_id, &counter_id)?;
    let expected = Caller {
        immediate: callers::agent_in(
            scheduler.agent_id,
            scheduler_id.agent_type.clone(),
            executor.get_worker_metadata(&scheduler).await?.fingerprint,
            component.environment_id,
        )
        .immediate,
        ..account
    };
    let recorded = executor
        .recorded_callers(&counter.agent_id)
        .await
        .into_iter()
        .filter(|(name, _)| name.ends_with("increment"))
        .collect::<Vec<_>>();
    assert_eq!(
        recorded
            .iter()
            .map(|(name, _)| name.as_str())
            .collect::<Vec<_>>(),
        ["pending increment", "increment"]
    );
    for (name, recorded) in recorded {
        assert_eq!(recorded, expected, "{name}");
    }
    drop(executor);
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
    let scheduler_id = agent_key!("SchedulerAgent", "go-sched-2");
    let counter_id = agent_key!("CounterAgent", "go-sched-target-2");
    executor
        .start_agent_with(
            &component.id,
            scheduler_id.clone(),
            HashMap::new(),
            Vec::new(),
        )
        .await?;

    // The first cancellation withdraws the invocation; a repeated one finds it
    // already cancelled.
    let cancelled = executor
        .invoke_and_await_agent(
            &component,
            &scheduler_id,
            "bump-cancelled",
            data_value!("go-sched-target-2", 500i64),
        )
        .await?
        .into_typed::<Vec<bool>>()?;
    assert_eq!(cancelled, vec![true, false]);

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
