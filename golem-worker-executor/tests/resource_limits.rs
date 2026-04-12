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
use axum::Router;
use axum::routing::get;
use golem_common::model::AgentStatus;
use golem_common::{agent_id, data_value};
use golem_test_framework::dsl::TestDsl;
use golem_worker_executor_test_utils::{
    LastUniqueId, PrecompiledComponent, TestContext, WorkerExecutorTestDependencies,
    start_with_concurrent_agent_limit, start_with_invocation_limits, start_with_table_limit,
};
use std::collections::HashMap;
use std::time::Duration;
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

// ---------------------------------------------------------------------------
// Concurrent agent limit tests
// ---------------------------------------------------------------------------

/// When the concurrent agent limit is not yet reached, agents start immediately
/// without waiting for a permit.
#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn concurrent_agent_limit_not_reached_starts_immediately(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("agent_counters")] agent_counters: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start_with_concurrent_agent_limit(deps, &context, 2).await?;

    let component = executor
        .component_dep(&context.default_environment_id, agent_counters)
        .store()
        .await?;

    let a1 = agent_id!("Counter", "concurrent-limit-ok-1");
    executor.start_agent(&component.id, a1.clone()).await?;

    let a2 = agent_id!("Counter", "concurrent-limit-ok-2");
    executor.start_agent(&component.id, a2.clone()).await?;

    // Both agents should be invocable.
    executor
        .invoke_and_await_agent(&component, &a1, "increment", data_value!())
        .await?;
    executor
        .invoke_and_await_agent(&component, &a2, "increment", data_value!())
        .await?;

    Ok(())
}

/// When the limit is reached with no idle agents, a new agent waits in
/// WaitingForPermit until the running agent finishes and its permit is returned.
///
/// Uses an HTTP-gated server to keep a1 provably Running (not Idle) while a2
/// is in WaitingForPermit, eliminating any timing dependency on agent
/// initialization speed.
#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn concurrent_agent_limit_waits_for_running_agent_to_finish(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("http_tests")] http_tests: &PrecompiledComponent,
    #[tagged_as("agent_counters")] agent_counters: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start_with_concurrent_agent_limit(deps, &context, 1).await?;

    // HTTP server that gates its /poll response behind a Notify.
    // HttpClient2.start_polling polls GET /poll until the body equals "done".
    // By holding the Notify unreleased we keep a1 in the Running state
    // for as long as needed, preventing eviction and holding the only permit.
    let gate = std::sync::Arc::new(tokio::sync::Notify::new());
    let gate_clone = gate.clone();
    let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await?;
    let port = listener.local_addr()?.port();
    let http_server = tokio::spawn(async move {
        let route = Router::new().route(
            "/poll",
            get(move || {
                let gate = gate_clone.clone();
                async move {
                    gate.notified().await;
                    "done".to_string()
                }
            }),
        );
        axum::serve(listener, route).await.unwrap();
    });

    let http_component = executor
        .component_dep(&context.default_environment_id, http_tests)
        .store()
        .await?;

    // Start a1 using the HttpClient2 agent. Pass the HTTP server port via env.
    let a1 = agent_id!("HttpClient2");
    let mut env = HashMap::new();
    env.insert("PORT".to_string(), port.to_string());
    let a1_id = executor
        .start_agent_with(
            &http_component.id,
            a1.clone(),
            env,
            HashMap::new(),
            Vec::new(),
        )
        .await?;

    // Enqueue start_polling (fire-and-forget). a1 immediately starts Running,
    // polling the gated server — it cannot complete or go Idle until we notify.
    executor
        .invoke_agent(&http_component, &a1, "start_polling", data_value!("done"))
        .await?;

    // Confirm a1 is provably Running (holds the only permit, is not evictable).
    executor
        .wait_for_status(&a1_id, AgentStatus::Running, Duration::from_secs(10))
        .await?;

    let counters_component = executor
        .component_dep(&context.default_environment_id, agent_counters)
        .store()
        .await?;

    // Spawn a2's start in background while a1 still holds only permit.
    // It must stay pending until gate opens; otherwise test is not exercising
    // contested path.
    let executor_clone = executor.clone();
    let counters_clone = counters_component.clone();
    let a2 = agent_id!("Counter", "concurrent-wait-http-2");
    let a2_clone = a2.clone();
    let a2_start = tokio::spawn(async move {
        executor_clone
            .start_agent(&counters_clone.id, a2_clone)
            .await
    });

    tokio::time::sleep(Duration::from_millis(250)).await;
    assert!(
        !a2_start.is_finished(),
        "a2 start finished while a1 was still Running; test did not hit permit contention"
    );

    // Release the gate — a1's poll loop returns "done", its invocation
    // completes, and its permit is returned to the semaphore via Drop.
    // This unblocks a2 from WaitingForPermit.
    gate.notify_waiters();

    // Wait for a1 to become Idle (invocation done, permit released).
    executor
        .wait_for_status(&a1_id, AgentStatus::Idle, Duration::from_secs(10))
        .await?;

    // a2 is now unblocked. invoke_and_await_agent will block until a2 has
    // acquired its permit and processed the invocation — this is the proof
    // that the waiting-and-unblocking path works correctly.
    executor
        .invoke_and_await_agent(&counters_component, &a2, "increment", data_value!())
        .await?;

    let a2_id = tokio::time::timeout(Duration::from_secs(10), a2_start)
        .await
        .expect("a2 start should unblock after a1 releases permit")??;
    executor
        .wait_for_status(&a2_id, AgentStatus::Idle, Duration::from_secs(10))
        .await?;

    http_server.abort();
    Ok(())
}

/// Idle agents must release their concurrent-agent permit so new agents can
/// start without evicting the idle ones.
///
/// Setup: limit=1, keep a1 actively Running via an HTTP gate. While a1 is
/// Running (not evictable) and holds the only permit, start a2 in the
/// background — it blocks in WaitingForPermit. Release the gate so a1's
/// invocation completes and a1 goes Idle.
///
/// With the bug (idle agents hold permits): a1 stays idle but still holds the
/// permit. a2 remains blocked in WaitingForPermit because there is nothing to
/// evict (the `try_free_up` callback found a1 was still Running at the time it
/// was called, and now a1 is idle but the callback already returned false). a2
/// never starts, and invoking it times out.
///
/// With the fix (idle agents release permits): a1 goes Idle and immediately
/// drops its permit. a2's `acquire_owned().await` unblocks, a2 starts, and its
/// invocation succeeds within the timeout.
///
/// Crucially, this test does NOT rely on eviction. The only way a2 can start
/// is if a1's permit was returned to the pool by the idle transition, not by
/// `stop_if_idle` eviction.
#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn concurrent_agent_idle_releases_permit(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("http_tests")] http_tests: &PrecompiledComponent,
    #[tagged_as("agent_counters")] agent_counters: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    // limit=1: exactly one concurrent-agent permit.
    let executor = start_with_concurrent_agent_limit(deps, &context, 1).await?;

    // --- HTTP gate: keeps a1 provably Running until we release it. ---
    let gate = std::sync::Arc::new(tokio::sync::Notify::new());
    let gate_clone = gate.clone();
    let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await?;
    let port = listener.local_addr()?.port();
    let http_server = tokio::spawn(async move {
        let route = Router::new().route(
            "/poll",
            get(move || {
                let gate = gate_clone.clone();
                async move {
                    gate.notified().await;
                    "done".to_string()
                }
            }),
        );
        axum::serve(listener, route).await.unwrap();
    });

    let http_component = executor
        .component_dep(&context.default_environment_id, http_tests)
        .store()
        .await?;

    // Start a1 using HttpClient2 (polls GET /poll until body equals "done").
    let a1 = agent_id!("HttpClient2");
    let mut env = HashMap::new();
    env.insert("PORT".to_string(), port.to_string());
    let a1_id = executor
        .start_agent_with(
            &http_component.id,
            a1.clone(),
            env,
            HashMap::new(),
            Vec::new(),
        )
        .await?;

    // Fire-and-forget: a1 starts polling the gated server.
    executor
        .invoke_agent(&http_component, &a1, "start_polling", data_value!("done"))
        .await?;

    // Confirm a1 is Running (holds the only permit, is NOT evictable).
    executor
        .wait_for_status(&a1_id, AgentStatus::Running, Duration::from_secs(10))
        .await?;

    // Spawn a2 in background while a1 still holds only permit. Assert start is
    // still pending before gate opens. That proves later success comes from
    // permit release on idle transition, not lucky late scheduling.
    let counters_component = executor
        .component_dep(&context.default_environment_id, agent_counters)
        .store()
        .await?;
    let executor_clone = executor.clone();
    let counters_clone = counters_component.clone();
    let a2 = agent_id!("Counter", "idle-permit-2");
    let a2_clone = a2.clone();
    let a2_start = tokio::spawn(async move {
        executor_clone
            .start_agent(&counters_clone.id, a2_clone)
            .await
    });

    tokio::time::sleep(Duration::from_millis(250)).await;
    assert!(
        !a2_start.is_finished(),
        "a2 start finished while a1 was still Running; test did not hit idle-release path"
    );

    // Release the gate. a1's poll returns "done", invocation completes, a1 goes Idle.
    // With the fix: Idle transition drops the permit → semaphore notifies a2 → a2 starts.
    // With the bug: a1 stays Idle but holds permit → a2 remains blocked forever.
    gate.notify_waiters();

    // a2 should now be unblocked (fix) or remain stuck (bug).
    // Give it 15 seconds — well beyond what starting a counter agent takes.
    let a2_result = tokio::time::timeout(
        Duration::from_secs(15),
        executor.invoke_and_await_agent(&counters_component, &a2, "increment", data_value!()),
    )
    .await;

    assert!(
        a2_result.is_ok(),
        "a2 should have started after a1 went Idle and released its permit, \
         but it timed out — idle agents are still holding permits"
    );
    a2_result.unwrap()?;

    let a2_id = tokio::time::timeout(Duration::from_secs(10), a2_start)
        .await
        .expect("a2 start should unblock after a1 goes idle and releases permit")??;
    executor
        .wait_for_status(&a2_id, AgentStatus::Idle, Duration::from_secs(10))
        .await?;

    http_server.abort();
    Ok(())
}

// ---------------------------------------------------------------------------
// Per-invocation call limit tests
// ---------------------------------------------------------------------------

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
