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
    start_with_invocation_limits, start_with_table_limit, LastUniqueId, PrecompiledComponent,
    TestContext, WorkerExecutorTestDependencies,
};
use std::collections::HashMap;
use test_r::{inherit_test_dep, test, timeout};

inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(LastUniqueId);
inherit_test_dep!(Tracing);
inherit_test_dep!(
    #[tagged_as("agent_counters")]
    PrecompiledComponent
);
inherit_test_dep!(
    #[tagged_as("http_tests")]
    PrecompiledComponent
);
inherit_test_dep!(
    #[tagged_as("agent_rpc_rust")]
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
/// fail because `create_worker` calls `start_if_needed` and waits for the
/// `WorkerLoaded` event synchronously. Wasmtime calls `table_growing` during
/// component instantiation, which triggers the limit check and propagates the
/// error back to `start_agent`.
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
    // Limit is 100, component table is 275 — fails during synchronous instantiation.
    let executor = start_with_table_limit(deps, &context, 100).await?;

    let component = executor
        .component_dep(&context.default_environment_id, agent_counters)
        .store()
        .await?;

    let agent_id = agent_id!("Counter", "table-limit-exceeded-1");

    // create_worker calls start_if_needed and waits for WorkerLoaded, so the
    // table_growing limit error propagates synchronously here.
    let create_result = executor.start_agent(&component.id, agent_id.clone()).await;

    assert!(
        create_result.is_err(),
        "expected start_agent to fail with ExceededTableLimit"
    );
    let err_str = format!("{create_result:?}");
    assert!(
        err_str.contains("function table") || err_str.contains("ExceededTableLimit"),
        "expected ExceededTableLimit error when starting agent, got: {err_str}"
    );

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

/// Sets the per-invocation HTTP call limit to 0 so that any outgoing HTTP
/// request from the component immediately traps with
/// `WorkerExceededHttpCallLimit`. The `http_tests` component's `run` function
/// makes exactly one HTTP call; with limit 0 it should fail before the call
/// reaches the network.
#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn http_call_limit_exceeded_traps_invocation(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("http_tests")] http_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    use axum::routing::post;
    use axum::Router;
    use tokio::spawn;
    use tracing::Instrument;

    let context = TestContext::new(last_unique_id);
    // Limit is 0: the very first HTTP call in any invocation must trap.
    let executor = start_with_invocation_limits(deps, &context, 0, u64::MAX).await?;

    let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await.unwrap();
    let host_http_port = listener.local_addr().unwrap().port();

    // Spin up a minimal HTTP server. The worker should trap before it reaches
    // the server, but we still bind one so the PORT env var resolves correctly.
    spawn(
        async move {
            axum::serve(listener, Router::new().route("/", post(|| async { "ok" })))
                .await
                .unwrap();
        }
        .in_current_span(),
    );

    let component = executor
        .component_dep(&context.default_environment_id, http_tests)
        .store()
        .await?;

    let mut env = HashMap::new();
    env.insert("PORT".to_string(), host_http_port.to_string());

    let agent_id = agent_id!("HttpClient");
    executor
        .start_agent_with(
            &component.id,
            agent_id.clone(),
            env,
            HashMap::new(),
            Vec::new(),
        )
        .await?;

    // The invocation must fail because the HTTP limit is 0.
    let result = executor
        .invoke_and_await_agent(&component, &agent_id, "run", data_value!())
        .await;

    assert!(
        result.is_err(),
        "expected invocation to fail due to HTTP call limit, but it succeeded"
    );
    let err_str = format!("{result:?}");
    assert!(
        err_str.contains("HTTP call limit") || err_str.contains("ExceededHttpCallLimit"),
        "expected ExceededHttpCallLimit error, got: {err_str}"
    );

    Ok(())
}

/// Sets the per-invocation RPC call limit to 0 so that any outgoing RPC call
/// from the component immediately traps with `WorkerExceededRpcCallLimit`.
/// The `agent_rpc_rust` component's `add_and_get` function makes an RPC call
/// to a counter worker; with limit 0 it should trap before the call is made.
#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn rpc_call_limit_exceeded_traps_invocation(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("agent_rpc_rust")] agent_rpc_rust: &PrecompiledComponent,
    #[tagged_as("agent_counters")] agent_counters: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    // Limit is 0: the very first RPC call in any invocation must trap.
    let executor = start_with_invocation_limits(deps, &context, u64::MAX, 0).await?;

    // Store the counter component that will be the RPC target.
    executor
        .component_dep(&context.default_environment_id, agent_counters)
        .store()
        .await?;

    let rpc_component = executor
        .component_dep(&context.default_environment_id, agent_rpc_rust)
        .store()
        .await?;

    let caller_id = agent_id!("RustParent", "rpc_limit_test");
    executor
        .start_agent(&rpc_component.id, caller_id.clone())
        .await?;

    // The invocation must fail because the RPC limit is 0.
    // `spawn_child` is a function that makes an RPC call to create a child worker.
    let result = executor
        .invoke_and_await_agent(
            &rpc_component,
            &caller_id,
            "spawn_child",
            data_value!("payload"),
        )
        .await;

    assert!(
        result.is_err(),
        "expected invocation to fail due to RPC call limit, but it succeeded"
    );
    let err_str = format!("{result:?}");
    assert!(
        err_str.contains("RPC call limit") || err_str.contains("ExceededRpcCallLimit"),
        "expected ExceededRpcCallLimit error, got: {err_str}"
    );

    Ok(())
}
