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

use anyhow::anyhow;
use axum::Router;
use axum::routing::post;
use golem_client::api::RegistryServiceClient;
use golem_common::model::{AgentStatus, PromiseId};
use golem_common::tracing::{TracingConfig, init_tracing_with_default_debug_env_filter};
use golem_common::{agent_id, data_value};
use golem_test_framework::config::{
    EnvBasedTestDependencies, EnvBasedTestDependenciesConfig, TestDependencies,
};
use golem_test_framework::dsl::{TestDsl, TestDslExtended};
use golem_wasm::FromValue;
use pretty_assertions::assert_matches;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use test_r::{test, test_dep, timeout};
use tracing::Instrument;
use tracing::debug;
use tracing::info;

test_r::enable!();

#[derive(Debug)]
pub struct Tracing;

impl Tracing {
    pub fn init() -> Self {
        #[cfg(unix)]
        unsafe {
            backtrace_on_stack_overflow::enable()
        };
        init_tracing_with_default_debug_env_filter(
            &TracingConfig::test_pretty_without_time("integration-tests").with_env_overrides(),
        );
        Self
    }
}

#[test_dep]
pub fn tracing() -> Tracing {
    Tracing::init()
}

#[test_dep]
pub async fn create_deps(_tracing: &Tracing) -> EnvBasedTestDependencies {
    let deps = EnvBasedTestDependencies::new(EnvBasedTestDependenciesConfig {
        worker_executor_cluster_size: 1,
        oplog_archive_interval: Some(Duration::from_secs(2)),
        ..EnvBasedTestDependenciesConfig::new()
    })
    .await
    .expect("Failed constructing test dependencies");

    deps.redis_monitor().assert_valid();

    deps
}

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

/// Regression test for executor panic when the oplog archiver fires for a
/// worker whose environment has been deleted.
///
/// Panic path (stack frames from production trace):
///   SchedulerServiceDefault background loop
///     → ScheduledAction::ArchiveOplog handler
///     → SchedulerWorkerAccess::open_oplog
///     → Worker::get_or_create_suspended
///     → Worker::new
///     → DefaultWorkerService::get
///     → component_service.get_metadata(revision)   ← returns ComponentNotFound
///     → unwrap_or_else(|err| panic!(...))           ← BOOM
///
/// The archiver is scheduled when a worker transitions to Idle/Failed/Exited.
/// With GOLEM__OPLOG__ARCHIVE_INTERVAL=2s (set in the test framework env) the
/// archive action fires within a few seconds after the worker goes idle.
/// Deleting the environment before that window expires reproduces the panic.
#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn oplog_archive_in_deleted_environment_does_not_panic(
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

    // Invoke a method so the worker completes and transitions to Idle, which
    // schedules an ArchiveOplog action 2 s from now (archive_interval=2s in tests).
    user.invoke_and_await_agent(
        &component,
        &agent_id!("Counter", "archive-panic-test"),
        "increment",
        data_value!(),
    )
    .await?;

    // Delete the environment while the ArchiveOplog action is pending.
    registry_client
        .delete_environment(&env.id.0, env.revision.into())
        .await?;

    debug!("deleted environment {}", env.id);

    // Wait long enough for the archive action to fire (archive_interval=2s +
    // scheduler process_interval≤2s → up to 4s; 8s is a comfortable margin).
    tokio::time::sleep(Duration::from_secs(8)).await;

    // Verify the executor is still alive
    assert!(deps.worker_executor_cluster().is_running().await);

    // Verify the executor is still alive by making an unrelated call on a
    // fresh environment. If the scheduler task panicked the executor would
    // be unresponsive. Only having a single executor guarantees we are hitting the instance.
    let liveness_user = deps.user().await?;
    let (_, liveness_env) = liveness_user.app_and_env().await?;

    let liveness_component = liveness_user
        .component(&liveness_env.id, "it_agent_counters_release")
        .name("it:agent-counters")
        .store()
        .await?;

    liveness_user
        .invoke_and_await_agent(
            &liveness_component,
            &agent_id!("Counter", "liveness-check"),
            "increment",
            data_value!(),
        )
        .await
        .map_err(|e| anyhow::anyhow!(
            "executor is no longer responsive after ArchiveOplog fired on deleted environment: {e}"
        ))?;

    Ok(())
}

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

#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn complete_promise_in_deleted_environment_results_in_404(
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
    user.invoke_agent(
        &component,
        &agent_id,
        "awaitPromise",
        data_value!(promise_id_vat),
    )
    .await?;

    user.wait_for_status(&worker, AgentStatus::Suspended, Duration::from_secs(10))
        .await?;

    // Delete the environment while the agent is suspended.
    registry_client
        .delete_environment(&env.id.0, env.revision.into())
        .await?;

    info!("Completing pending promise");

    let error = user
        .complete_promise(&promise_id, b"ignored".to_vec())
        .await
        .unwrap_err();

    let downcasted = error
        .downcast_ref::<golem_client::Error<golem_client::api::WorkerError>>()
        .unwrap();

    assert_matches!(
        downcasted,
        golem_client::Error::Item(golem_client::api::WorkerError::Error404(_))
    );

    Ok(())
}
