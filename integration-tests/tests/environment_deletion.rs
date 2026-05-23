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

use anyhow::anyhow;
use axum::Router;
use axum::routing::post;
use golem_client::api::RegistryServiceClient;
use golem_common::model::{AgentStatus, PromiseId};
use golem_common::{agent_id, data_value};
use golem_test_framework::config::{EnvBasedTestDependencies, TestDependencies};
use golem_test_framework::dsl::{TestDsl, TestDslExtended};
use golem_wasm::FromValue;
use pretty_assertions::assert_matches;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use test_r::{inherit_test_dep, test, timeout};
use tracing::Instrument;

inherit_test_dep!(Tracing);
inherit_test_dep!(EnvBasedTestDependencies);

/// Verify that deleting an environment stops a self-scheduling agent.
///
/// The agent sends an HTTP POST to a local test server on every `tick`, then
/// schedules itself 500 ms into the future.  Once the environment is deleted
/// the scheduler can no longer activate the agent (component metadata fetch
/// returns NotFound for a deleted environment) and the ping counter stops
/// advancing.
#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn environment_deletion_stops_self_scheduling_agent(
    deps: &EnvBasedTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let registry_client = user.registry_service_client().await;
    let (_, env) = user.app_and_env().await?;

    let ping_count = Arc::new(AtomicU64::new(0));
    let ping_count_clone = ping_count.clone();

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0".parse::<SocketAddr>().unwrap())
        .await
        .unwrap();
    let port = listener.local_addr().unwrap().port();

    let http_server = tokio::spawn(
        async move {
            let route = Router::new().route(
                "/ping",
                post(move || {
                    let counter = ping_count_clone.clone();
                    async move {
                        counter.fetch_add(1, Ordering::SeqCst);
                        "ok"
                    }
                }),
            );
            axum::serve(listener, route).await.unwrap();
        }
        .in_current_span(),
    );

    let component = user
        .component(&env.id, "golem_it_agent_rpc_rust_release")
        .name("golem-it:agent-rpc-rust")
        .store()
        .await?;

    let agent_name = "http-polling-self-scheduler-test";
    let parsed_agent_id = agent_id!("HttpPollingSelfScheduler", agent_name);
    user.start_agent(&component.id, parsed_agent_id.clone())
        .await?;

    user.invoke_agent(
        &component,
        &parsed_agent_id,
        "tick",
        golem_common::data_value!("127.0.0.1", port as u16),
    )
    .await?;

    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        if ping_count.load(Ordering::SeqCst) >= 3 {
            break;
        }
        if tokio::time::Instant::now() > deadline {
            http_server.abort();
            anyhow::bail!(
                "timed out waiting for the agent loop to start (got {} pings)",
                ping_count.load(Ordering::SeqCst)
            );
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }

    registry_client
        .delete_environment(&env.id.0, env.revision.into())
        .await?;

    tokio::time::sleep(Duration::from_secs(2)).await;
    let count_after_delete = ping_count.load(Ordering::SeqCst);

    tokio::time::sleep(Duration::from_secs(3)).await;
    let count_stable = ping_count.load(Ordering::SeqCst);

    http_server.abort();

    assert_eq!(
        count_after_delete, count_stable,
        "agent kept pinging after environment deletion ({count_after_delete} → {count_stable})"
    );

    Ok(())
}

/// Invoking a method on an already-running agent whose environment has been
/// deleted should return a 404 error.
#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn invoke_existing_agent_in_deleted_environment_fails(
    deps: &EnvBasedTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let registry_client = user.registry_service_client().await;
    let (_, env) = user.app_and_env().await?;

    let component = user
        .component(&env.id, "it_agent_counters_release")
        .name("it:agent-counters")
        .store()
        .await?;

    let parsed_agent_id = agent_id!("Counter", "env-delete-existing");

    user.invoke_and_await_agent(&component, &parsed_agent_id, "increment", data_value!())
        .await?;

    registry_client
        .delete_environment(&env.id.0, env.revision.into())
        .await?;

    // Allow the invalidation event to propagate to the executor.
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;

    let error = user
        .invoke_and_await_agent(&component, &parsed_agent_id, "increment", data_value!())
        .await
        .unwrap_err();

    let downcasted = error
        .downcast_ref::<golem_client::Error<golem_client::api::AgentError>>()
        .unwrap();

    assert_matches!(
        downcasted,
        golem_client::Error::Item(golem_client::api::AgentError::Error404(_))
    );

    Ok(())
}

/// Invoking a method on a non-existing agent whose environment has been deleted
/// should also return a 404.
#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn invoke_new_agent_in_deleted_environment_fails(
    deps: &EnvBasedTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let registry_client = user.registry_service_client().await;
    let (_, env) = user.app_and_env().await?;

    let component = user
        .component(&env.id, "it_agent_counters_release")
        .name("it:agent-counters")
        .store()
        .await?;

    registry_client
        .delete_environment(&env.id.0, env.revision.into())
        .await?;

    // Allow the invalidation event to propagate to the executor.
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;

    let error = user
        .invoke_and_await_agent(
            &component,
            &agent_id!("Counter", "env-delete-new"),
            "increment",
            data_value!(),
        )
        .await
        .unwrap_err();

    let downcasted = error
        .downcast_ref::<golem_client::Error<golem_client::api::AgentError>>()
        .unwrap();

    assert_matches!(
        downcasted,
        golem_client::Error::Item(golem_client::api::AgentError::Error404(_))
    );

    Ok(())
}

/// Completing a promise whose owning agent lives in a deleted environment should
/// not panic the executor.  This is a regression test: the panic occurs because
/// `WorkerService::get` inside the promise-completion path calls `get_metadata`
/// which now returns `Err` (ComponentNotFound) for a deleted environment, and
/// the existing code uses `unwrap_or_else(|err| panic!(...))`.
///
/// The test intentionally does NOT assert on the promise-completion result —
/// it just verifies that the executor process survives the call.
#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn complete_promise_in_deleted_environment_does_not_panic(
    deps: &EnvBasedTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let registry_client = user.registry_service_client().await;
    let (_, env) = user.app_and_env().await?;

    let component = user
        .component(&env.id, "golem_it_agent_promise")
        .name("golem-it:agent-promise")
        .store()
        .await?;

    let agent_id = agent_id!("PromiseAgent", "promise-env-delete-test");
    let worker = user.start_agent(&component.id, agent_id.clone()).await?;

    let result = user
        .invoke_and_await_agent(&component, &agent_id, "getPromise", data_value!())
        .await?;

    let promise_id_vat = result
        .into_return_value_and_type()
        .ok_or_else(|| anyhow!("expected promise id return value"))?;
    let promise_id =
        PromiseId::from_value(promise_id_vat.value.clone()).map_err(|e| anyhow!("{e}"))?;

    // Suspend the agent on the promise.
    user
        .invoke_agent(&component, &agent_id, "awaitPromise", data_value!(promise_id_vat))
        .await?;

    user.wait_for_status(&worker, AgentStatus::Suspended, Duration::from_secs(10))
        .await?;

    // Delete the environment while the agent is suspended.
    registry_client
        .delete_environment(&env.id.0, env.revision.into())
        .await?;

    // Completing the promise must not panic the executor.
    // We ignore the result — the interesting property is that we reach this
    // line without the executor crashing.
    let error = user.complete_promise(&promise_id, b"ignored".to_vec()).await.unwrap_err();

    let downcasted = error
        .downcast_ref::<golem_client::Error<golem_client::api::WorkerError>>()
        .unwrap();

    assert_matches!(
        downcasted,
        golem_client::Error::Item(golem_client::api::WorkerError::Error404(_))
    );

    Ok(())
}
