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
use golem_common::{agent_id, data_value};
use golem_test_framework::dsl::TestDsl;
use golem_worker_executor_test_utils::{
    start_with_table_limit, LastUniqueId, PrecompiledComponent, TestContext,
    WorkerExecutorTestDependencies,
};
use test_r::{inherit_test_dep, test, timeout};

inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(LastUniqueId);
inherit_test_dep!(Tracing);
inherit_test_dep!(
    #[tagged_as("agent_counters")]
    PrecompiledComponent
);

/// The `it_agent_counters_release` component has a static function table with
/// 275 entries. Setting the limit well above that (1000) ensures normal
/// invocations are unaffected by the table limit enforcement.
#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn table_within_limit_succeeds(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("agent_counters")] agent_counters: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    // Limit is 1000, component table is 275 — well within the limit.
    let executor = start_with_table_limit(deps, &context, 1000).await?;

    let component = executor
        .component_dep(&context.default_environment_id, agent_counters)
        .store()
        .await?;

    let agent_id = agent_id!("Counter", "table-limit-ok-1");
    executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    // Normal invocation should succeed with a generous table limit.
    executor
        .invoke_and_await_agent(&component, &agent_id, "increment", data_value!())
        .await?;

    Ok(())
}

/// The `it_agent_counters_release` component has a static function table with
/// 275 entries. Setting the limit below that (100) causes worker creation to
/// fail because wasmtime calls `table_growing` during component instantiation,
/// which triggers the limit check before any function is invoked.
///
/// The expected outcome is that `start_agent` (which instantiates the component)
/// returns an error containing the ExceededTableLimit message.
#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn table_exceeding_limit_fails_at_instantiation(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("agent_counters")] agent_counters: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    // Limit is 100, component table is 275 — should fail during instantiation.
    let executor = start_with_table_limit(deps, &context, 100).await?;

    let component = executor
        .component_dep(&context.default_environment_id, agent_counters)
        .store()
        .await?;

    let agent_id = agent_id!("Counter", "table-limit-exceeded-1");

    // start_agent triggers component instantiation, which calls table_growing.
    // With a limit of 100 and a component table of 275, this must fail.
    let create_result = executor.start_agent(&component.id, agent_id.clone()).await;

    match create_result {
        Err(err) => {
            let err_str = format!("{err:?}");
            assert!(
                err_str.contains("function table") || err_str.contains("ExceededTableLimit"),
                "expected ExceededTableLimit error when starting agent, got: {err_str}"
            );
        }
        Ok(_) => {
            // If start_agent succeeded unexpectedly, the invoke should fail.
            // This handles a hypothetical race where instantiation is deferred.
            let invoke_result = executor
                .invoke_and_await_agent(&component, &agent_id, "increment", data_value!())
                .await;
            assert!(
                invoke_result.is_err(),
                "expected invocation to fail with ExceededTableLimit"
            );
            let err_str = format!("{invoke_result:?}");
            assert!(
                err_str.contains("function table") || err_str.contains("ExceededTableLimit"),
                "expected ExceededTableLimit error, got: {err_str}"
            );
        }
    }

    Ok(())
}

/// After a worker fails to start due to ExceededTableLimit, attempting to
/// create the same agent again should also fail (it's not retriable — the
/// component simply cannot be instantiated with this table limit).
#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn table_exceeding_limit_not_retried(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("agent_counters")] agent_counters: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start_with_table_limit(deps, &context, 100).await?;

    let component = executor
        .component_dep(&context.default_environment_id, agent_counters)
        .store()
        .await?;

    let agent_id = agent_id!("Counter", "table-limit-no-retry-1");

    // First attempt: expect failure due to table limit at instantiation.
    let first_result = executor.start_agent(&component.id, agent_id.clone()).await;
    assert!(
        first_result.is_err(),
        "expected first start_agent to fail with ExceededTableLimit"
    );

    // Second attempt with the same agent: also expect failure.
    // The component table limit hasn't changed, so instantiation will fail again.
    let second_result = executor.start_agent(&component.id, agent_id.clone()).await;
    assert!(
        second_result.is_err(),
        "expected second start_agent to also fail (not retry successfully)"
    );

    let err_str = format!("{second_result:?}");
    // The second error may be AgentAlreadyExists or another instantiation failure —
    // either way the worker is not running and the limit is still respected.
    assert!(
        err_str.contains("function table")
            || err_str.contains("ExceededTableLimit")
            || err_str.contains("already exists")
            || err_str.contains("AlreadyExists"),
        "expected a relevant error on second attempt, got: {err_str}"
    );

    Ok(())
}
