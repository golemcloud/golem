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

use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::post;
use axum::Router;
use bytes::Bytes;
use golem_client::api::RegistryServiceClient;
use golem_common::model::auth::EnvironmentRole;
use golem_common::model::base64::Base64;
use golem_common::model::component::ComponentId;
use golem_common::model::component::{
    PluginInstallation, PluginInstallationAction, PluginPriority, PluginUninstallation,
};
use golem_common::model::environment_plugin_grant::EnvironmentPluginGrantCreation;
use golem_common::model::plugin_registration::{
    OplogProcessorPluginSpec, PluginRegistrationCreation, PluginSpecDto,
};
use golem_common::model::{AgentStatus, OplogIndex, ScanCursor};
use golem_common::{agent_id, data_value};
use golem_test_framework::config::{EnvBasedTestDependencies, TestDependencies};
use golem_test_framework::dsl::{TestDsl, TestDslExtended};
use std::collections::{BTreeMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use test_r::{inherit_test_dep, test};
use tokio::task::{JoinHandle, JoinSet};
use tracing::Instrument;

inherit_test_dep!(EnvBasedTestDependencies);

// ============================================================================
// Helpers
// ============================================================================

/// Per-batch delivery record from the oplog processor test component.
#[derive(Clone, Debug, serde::Deserialize)]
struct BatchCallback {
    #[allow(dead_code)]
    source_worker_id: String,
    #[allow(dead_code)]
    account_id: String,
    #[allow(dead_code)]
    component_id: String,
    first_entry_index: u64,
    entry_count: u64,
    invocations: Vec<InvocationRecord>,
}

#[derive(Clone, Debug, serde::Deserialize)]
struct InvocationRecord {
    oplog_index: u64,
    fn_name: String,
}

/// Extract function names from batches, sorted by oplog_index.
fn extract_function_names(batches: &[BatchCallback]) -> Vec<String> {
    let mut all: Vec<_> = batches.iter().flat_map(|b| b.invocations.iter()).collect();
    all.sort_by_key(|i| i.oplog_index);
    all.into_iter().map(|i| i.fn_name.clone()).collect()
}

/// Assert that all invocation oplog indices across batches are unique (no duplicate deliveries).
fn assert_unique_oplog_indices(batches: &[BatchCallback]) {
    let indices: Vec<u64> = batches
        .iter()
        .flat_map(|b| b.invocations.iter().map(|i| i.oplog_index))
        .collect();
    let unique: HashSet<u64> = indices.iter().copied().collect();
    assert_eq!(
        indices.len(),
        unique.len(),
        "Duplicate oplog indices found: {:?}",
        indices
    );
}

/// Assert that the function names match the expected list, sorted by oplog_index.
fn assert_function_names(batches: &[BatchCallback], expected_fns: &[&str]) {
    let actual_fns = extract_function_names(batches);
    let expected: Vec<String> = expected_fns.iter().map(|s| s.to_string()).collect();
    assert_eq!(actual_fns, expected, "Function names mismatch");
}

/// Count total invocations across all batches.
fn invocation_count(batches: &[BatchCallback]) -> usize {
    batches.iter().map(|b| b.invocations.len()).sum()
}

/// Assert that batch entry ranges don't overlap (no duplicate entry delivery).
#[allow(dead_code)]
fn assert_no_overlapping_batches(batches: &[BatchCallback]) {
    let mut ranges: Vec<(u64, u64)> = batches
        .iter()
        .map(|b| (b.first_entry_index, b.first_entry_index + b.entry_count - 1))
        .collect();
    ranges.sort_by_key(|r| r.0);
    for pair in ranges.windows(2) {
        assert!(
            pair[0].1 < pair[1].0,
            "Overlapping batch ranges: {:?} and {:?}",
            pair[0],
            pair[1]
        );
    }
}

/// Starts an HTTP callback server and returns (callback_url, received_batches, server_handle).
async fn start_callback_server() -> (String, Arc<Mutex<Vec<BatchCallback>>>, JoinHandle<()>) {
    let received_batches: Arc<Mutex<Vec<BatchCallback>>> = Arc::new(Mutex::new(Vec::new()));
    let received_clone = received_batches.clone();

    let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await.unwrap();
    let host_http_port = listener.local_addr().unwrap().port();

    let http_server = tokio::spawn(async move {
        let route = Router::new().route(
            "/callback",
            post(move |body: Bytes| {
                async move {
                    let body_str = String::from_utf8(body.to_vec()).unwrap();
                    let batch: BatchCallback = serde_json::from_str(&body_str).unwrap();
                    let mut batches = received_clone.lock().unwrap();
                    batches.push(batch);
                    "ok"
                }
                .in_current_span()
            }),
        );

        axum::serve(listener, route).await.unwrap();
    });

    let callback_url = format!("http://localhost:{host_http_port}/callback");
    (callback_url, received_batches, http_server)
}

/// Starts an HTTP callback server that can be gated — when `enabled` is false it returns 503.
/// Returns (callback_url, received_batches, enabled_flag, server_handle).
#[allow(dead_code)]
async fn start_callback_server_gated() -> (
    String,
    Arc<Mutex<Vec<BatchCallback>>>,
    Arc<AtomicBool>,
    JoinHandle<()>,
) {
    let received_batches: Arc<Mutex<Vec<BatchCallback>>> = Arc::new(Mutex::new(Vec::new()));
    let received_clone = received_batches.clone();
    let enabled = Arc::new(AtomicBool::new(false));
    let enabled_clone = enabled.clone();

    let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await.unwrap();
    let host_http_port = listener.local_addr().unwrap().port();

    let http_server = tokio::spawn(async move {
        let route = Router::new().route(
            "/callback",
            post(move |body: Bytes| {
                let received = received_clone.clone();
                let enabled = enabled_clone.clone();
                async move {
                    if !enabled.load(Ordering::Relaxed) {
                        return StatusCode::SERVICE_UNAVAILABLE.into_response();
                    }
                    let body_str = String::from_utf8(body.to_vec()).unwrap();
                    let batch: BatchCallback = serde_json::from_str(&body_str).unwrap();
                    let mut batches = received.lock().unwrap();
                    batches.push(batch);
                    "ok".into_response()
                }
                .in_current_span()
            }),
        );

        axum::serve(listener, route).await.unwrap();
    });

    let callback_url = format!("http://localhost:{host_http_port}/callback");
    (callback_url, received_batches, enabled, http_server)
}

/// Finds all workers running on a given plugin component via worker enumeration.
/// Returns their WorkerIds so they can be crashed individually.
async fn find_plugin_workers(
    user: &(impl TestDsl + Send + Sync + ?Sized),
    plugin_component_id: &ComponentId,
) -> Vec<golem_common::model::AgentId> {
    let mut all_workers = Vec::new();
    let mut cursor = ScanCursor {
        cursor: 0,
        layer: 0,
    };
    loop {
        let (next_cursor, workers) = user
            .get_workers_metadata(plugin_component_id, None, cursor, 50, true)
            .await
            .unwrap_or_else(|e| {
                tracing::warn!("Failed to enumerate plugin workers: {e}");
                (None, Vec::new())
            });
        for w in &workers {
            all_workers.push(w.agent_id.clone());
        }
        match next_cursor {
            Some(c) if !c.is_finished() => cursor = c,
            _ => break,
        }
    }
    all_workers
}

/// Crashes the user worker and all discovered plugin workers via simulated_crash.
async fn crash_user_and_plugin_workers(
    user: &(impl TestDsl + Send + Sync + ?Sized),
    user_worker_id: &golem_common::model::AgentId,
    plugin_component_id: &ComponentId,
) {
    // Discover plugin workers first (before crashing anything)
    let plugin_workers = find_plugin_workers(user, plugin_component_id).await;
    tracing::info!(
        "Crashing user worker {user_worker_id} and {} plugin worker(s): {:?}",
        plugin_workers.len(),
        plugin_workers,
    );

    // Crash the user worker
    if let Err(e) = user.simulated_crash(user_worker_id).await {
        tracing::warn!("simulated_crash failed for user worker {user_worker_id}: {e}");
    }

    // Crash all plugin workers
    for wid in &plugin_workers {
        if let Err(e) = user.simulated_crash(wid).await {
            tracing::warn!("simulated_crash failed for plugin worker {wid}: {e}");
        }
    }
}

/// Waits for at least `expected_count` invocations (across batches) within `timeout`.
/// Returns the collected batches. Panics on timeout.
async fn wait_for_invocations(
    received: &Arc<Mutex<Vec<BatchCallback>>>,
    expected_count: usize,
    timeout: Duration,
) -> Vec<BatchCallback> {
    let start = tokio::time::Instant::now();
    loop {
        let batches = received.lock().unwrap().clone();
        let count = invocation_count(&batches);
        if count >= expected_count {
            return batches;
        }
        if start.elapsed() > timeout {
            panic!(
                "Timed out waiting for oplog processor callbacks (got {} of {} expected invocations, {} batches: {:?})",
                count,
                expected_count,
                batches.len(),
                batches,
            );
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

// ============================================================================
// E1 + E2: Basic delivery + oplog verification
// ============================================================================

#[test]
#[tracing::instrument]
async fn oplog_processor(deps: &EnvBasedTestDependencies) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let client = user.registry_service_client().await;
    let (_, env) = user.app_and_env().await?;

    let (callback_url, received_batches, _http_server) = start_callback_server().await;

    let plugin_component = user
        .component(&env.id, "oplog_processor_release")
        .store()
        .await?;

    let oplog_processor_plugin = client
        .create_plugin(
            &user.account_id.0,
            &PluginRegistrationCreation {
                name: "oplog-processor-1".to_string(),
                version: "v1".to_string(),
                description: "A test".to_string(),
                icon: Base64(Vec::new()),
                homepage: "none".to_string(),
                spec: PluginSpecDto::OplogProcessor(OplogProcessorPluginSpec {
                    component_id: plugin_component.id,
                    component_revision: plugin_component.revision,
                }),
            },
        )
        .await?;

    let oplog_processor_plugin_grant = client
        .create_environment_plugin_grant(
            &env.id.0,
            &EnvironmentPluginGrantCreation {
                plugin_registration_id: oplog_processor_plugin.id,
            },
        )
        .await?;

    let mut plugin_params = BTreeMap::new();
    plugin_params.insert("callback-url".to_string(), callback_url.clone());

    let component = user
        .component(&env.id, "it_agent_counters_release")
        .name("it:agent-counters")
        .with_parametrized_plugin(&oplog_processor_plugin_grant.id, 0, plugin_params)
        .store()
        .await?;

    let repo_id = agent_id!("Repository", "worker1");
    let worker_id = user.start_agent(&component.id, repo_id.clone()).await?;

    user.invoke_and_await_agent(
        &component,
        &repo_id,
        "add",
        data_value!("G1000", "Golem T-Shirt M"),
    )
    .await?;

    user.invoke_and_await_agent(
        &component,
        &repo_id,
        "add",
        data_value!("G1001", "Golem Cloud Subscription 1y"),
    )
    .await?;

    user.invoke_and_await_agent(
        &component,
        &repo_id,
        "add",
        data_value!("G1002", "Mud Golem"),
    )
    .await?;

    // E1: Wait for callbacks and verify function names + unique oplog indices
    let batches = wait_for_invocations(&received_batches, 4, Duration::from_secs(60)).await;
    assert_function_names(&batches, &["agent-initialization", "add", "add", "add"]);
    assert_unique_oplog_indices(&batches);

    // E2: Verify oplog entries exist for the worker
    let oplog = user.get_oplog(&worker_id, OplogIndex::from_u64(0)).await?;
    assert!(!oplog.is_empty(), "Worker oplog should not be empty");

    Ok(())
}

// ============================================================================
// Existing test: different env after unregistering
// ============================================================================

#[test]
#[tracing::instrument]
async fn oplog_processor_in_different_env_after_unregistering(
    deps: &EnvBasedTestDependencies,
) -> anyhow::Result<()> {
    let user_1 = deps.user().await?;
    let (_, env_1) = user_1.app_and_env().await?;

    let user_2 = deps.user().await?;
    let client_2 = user_2.registry_service_client().await;

    user_1
        .share_environment(&env_1.id, &user_2.account_id, &[EnvironmentRole::Admin])
        .await?;

    let (callback_url, received_batches, _http_server) = start_callback_server().await;

    let plugin_component = user_2
        .component(&env_1.id, "oplog_processor_release")
        .store()
        .await?;

    let oplog_processor_plugin = client_2
        .create_plugin(
            &user_2.account_id.0,
            &PluginRegistrationCreation {
                name: "oplog-processor-1".to_string(),
                version: "v1".to_string(),
                description: "A test".to_string(),
                icon: Base64(Vec::new()),
                homepage: "none".to_string(),
                spec: PluginSpecDto::OplogProcessor(OplogProcessorPluginSpec {
                    component_id: plugin_component.id,
                    component_revision: plugin_component.revision,
                }),
            },
        )
        .await?;

    let oplog_processor_plugin_grant = client_2
        .create_environment_plugin_grant(
            &env_1.id.0,
            &EnvironmentPluginGrantCreation {
                plugin_registration_id: oplog_processor_plugin.id,
            },
        )
        .await?;

    let mut plugin_params = BTreeMap::new();
    plugin_params.insert("callback-url".to_string(), callback_url.clone());

    let component = user_1
        .component(&env_1.id, "it_agent_counters_release")
        .name("it:agent-counters")
        .with_parametrized_plugin(&oplog_processor_plugin_grant.id, 0, plugin_params)
        .store()
        .await?;

    client_2
        .delete_environment_plugin_grant(&oplog_processor_plugin_grant.id.0)
        .await?;
    client_2.delete_plugin(&oplog_processor_plugin.id.0).await?;

    let repo_id = agent_id!("Repository", "worker1");
    let _worker_id = user_1.start_agent(&component.id, repo_id.clone()).await?;

    user_1
        .invoke_and_await_agent(
            &component,
            &repo_id,
            "add",
            data_value!("G1000", "Golem T-Shirt M"),
        )
        .await?;

    user_1
        .invoke_and_await_agent(
            &component,
            &repo_id,
            "add",
            data_value!("G1001", "Golem Cloud Subscription 1y"),
        )
        .await?;

    user_1
        .invoke_and_await_agent(
            &component,
            &repo_id,
            "add",
            data_value!("G1002", "Mud Golem"),
        )
        .await?;

    let batches = wait_for_invocations(&received_batches, 4, Duration::from_secs(60)).await;
    assert_function_names(&batches, &["agent-initialization", "add", "add", "add"]);
    assert_unique_oplog_indices(&batches);

    Ok(())
}

// ============================================================================
// E3: Executor crash after confirmed flush — no entry loss or duplicates
// ============================================================================

#[test]
#[tracing::instrument]
async fn oplog_processor_crash_after_confirmed_flush(
    deps: &EnvBasedTestDependencies,
) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let client = user.registry_service_client().await;
    let (_, env) = user.app_and_env().await?;

    let (callback_url, received_batches, _http_server) = start_callback_server().await;

    let plugin_component = user
        .component(&env.id, "oplog_processor_release")
        .store()
        .await?;
    let oplog_processor_plugin = client
        .create_plugin(
            &user.account_id.0,
            &PluginRegistrationCreation {
                name: "oplog-processor-1".to_string(),
                version: "v1".to_string(),
                description: "A test".to_string(),
                icon: Base64(Vec::new()),
                homepage: "none".to_string(),
                spec: PluginSpecDto::OplogProcessor(OplogProcessorPluginSpec {
                    component_id: plugin_component.id,
                    component_revision: plugin_component.revision,
                }),
            },
        )
        .await?;
    let oplog_processor_plugin_grant = client
        .create_environment_plugin_grant(
            &env.id.0,
            &EnvironmentPluginGrantCreation {
                plugin_registration_id: oplog_processor_plugin.id,
            },
        )
        .await?;

    let mut plugin_params = BTreeMap::new();
    plugin_params.insert("callback-url".to_string(), callback_url.clone());

    let component = user
        .component(&env.id, "it_agent_counters_release")
        .name("it:agent-counters")
        .with_parametrized_plugin(&oplog_processor_plugin_grant.id, 0, plugin_params)
        .store()
        .await?;

    let repo_id = agent_id!("Repository", "worker1");
    let worker_id = user.start_agent(&component.id, repo_id.clone()).await?;

    user.invoke_and_await_agent(
        &component,
        &repo_id,
        "add",
        data_value!("G1000", "Golem T-Shirt M"),
    )
    .await?;
    user.invoke_and_await_agent(
        &component,
        &repo_id,
        "add",
        data_value!("G1001", "Golem Cloud Subscription 1y"),
    )
    .await?;
    user.invoke_and_await_agent(
        &component,
        &repo_id,
        "add",
        data_value!("G1002", "Mud Golem"),
    )
    .await?;

    // Wait for confirmed flush — callbacks received means the batch was delivered
    let _ = wait_for_invocations(&received_batches, 4, Duration::from_secs(60)).await;
    // Small buffer to let any async confirmation complete
    tokio::time::sleep(Duration::from_secs(2)).await;

    // Crash both user worker and oplog processor plugin worker via simulated_crash
    crash_user_and_plugin_workers(&user, &worker_id, &plugin_component.id).await;
    user.wait_for_statuses(
        &worker_id,
        &[AgentStatus::Idle, AgentStatus::Running],
        Duration::from_secs(60),
    )
    .await?;

    // Invoke once more after recovery
    user.invoke_and_await_agent(
        &component,
        &repo_id,
        "add",
        data_value!("G1003", "Post-crash item"),
    )
    .await?;

    let batches = wait_for_invocations(&received_batches, 5, Duration::from_secs(60)).await;
    assert_function_names(
        &batches,
        &["agent-initialization", "add", "add", "add", "add"],
    );
    assert_unique_oplog_indices(&batches);

    Ok(())
}

// ============================================================================
// E4: Stress test — crash recovery delivers all completed invocations
// ============================================================================
//
// This test verifies the oplog processor plugin's delivery guarantees under
// crash conditions.
//
// Tests that the oplog processor delivers every persisted oplog entry exactly
// once, even when worker crashes cause in-memory buffers to be lost.
//
// The ForwardingOplog buffers entries in memory and flushes on commit-count
// threshold (plugin_max_commit_count) or a timer (plugin_max_elapsed_time).
//
// Strategy:
//  1. Set up worker with oplog processor plugin using default thresholds.
//     Do an initial invocation so the worker is established and the plugin
//     has delivered at least one batch (agent-initialization).
//  2. Restart executor(s) with very high flush thresholds (effectively
//     disabling automatic flushing). This ensures entries accumulate in
//     the ForwardingOplog's in-memory buffer during crash rounds.
//  3. Run CRASH_ROUNDS rounds: each round does INVOCATIONS_PER_ROUND
//     synchronous invoke_and_await calls (entries persist in oplog but
//     buffer in memory without flushing), then crashes both user and
//     plugin workers (losing the in-memory buffer).
//  4. After all crash rounds, restart executor(s) with default thresholds
//     again. Do final invocations to trigger activity.
//  5. Assert: no duplicate oplog indices, and every completed add in
//     the oplog has exactly one callback delivery.
//
// This test verifies that recovery replays previously-persisted oplog
// entries, so no callbacks are lost even when in-memory buffers are
// discarded by crashes.

#[test]
#[tracing::instrument]
async fn oplog_processor_crash_stress(deps: &EnvBasedTestDependencies) -> anyhow::Result<()> {
    const CRASH_ROUNDS: usize = 5;
    const INVOCATIONS_PER_ROUND: usize = 2;

    let user = deps.user().await?;
    let client = user.registry_service_client().await;
    let (_, env) = user.app_and_env().await?;

    let (callback_url, received_batches, _http_server) = start_callback_server().await;

    let plugin_component = user
        .component(&env.id, "oplog_processor_release")
        .store()
        .await?;
    let oplog_processor_plugin = client
        .create_plugin(
            &user.account_id.0,
            &PluginRegistrationCreation {
                name: "oplog-processor-stress".to_string(),
                version: "v1".to_string(),
                description: "Stress test plugin".to_string(),
                icon: Base64(Vec::new()),
                homepage: "none".to_string(),
                spec: PluginSpecDto::OplogProcessor(OplogProcessorPluginSpec {
                    component_id: plugin_component.id,
                    component_revision: plugin_component.revision,
                }),
            },
        )
        .await?;
    let oplog_processor_plugin_grant = client
        .create_environment_plugin_grant(
            &env.id.0,
            &EnvironmentPluginGrantCreation {
                plugin_registration_id: oplog_processor_plugin.id,
            },
        )
        .await?;

    let mut plugin_params = BTreeMap::new();
    plugin_params.insert("callback-url".to_string(), callback_url.clone());

    let component = user
        .component(&env.id, "it_agent_counters_release")
        .name("it:agent-counters")
        .with_parametrized_plugin(&oplog_processor_plugin_grant.id, 0, plugin_params)
        .store()
        .await?;

    let repo_id = agent_id!("Repository", "worker1");
    let worker_id = user.start_agent(&component.id, repo_id.clone()).await?;

    // Phase 1: Initial invocations with default thresholds — establishes the
    // worker and generates oplog entries that will be flushed to the plugin.
    for i in 0..3 {
        user.invoke_and_await_agent(
            &component,
            &repo_id,
            "add",
            data_value!(format!("INIT{i}"), format!("Initial Item {i}")),
        )
        .await?;
    }

    // Give the plugin worker time to process the initial batches
    tokio::time::sleep(Duration::from_secs(10)).await;

    // Phase 2: Restart executor(s) with very high flush thresholds to disable
    // automatic flushing. Entries will accumulate in ForwardingOplog's in-memory
    // buffer and never be sent to the plugin worker.
    tracing::info!("Restarting executor(s) with high flush thresholds...");
    let cluster = deps.worker_executor_cluster();
    unsafe {
        std::env::set_var("GOLEM__OPLOG__PLUGIN_MAX_COMMIT_COUNT", "100000");
        std::env::set_var("GOLEM__OPLOG__PLUGIN_MAX_ELAPSED_TIME", "3600s");
    }
    cluster.kill_all().await;
    cluster.restart_all().await;

    // Wait for the worker to recover after executor restart
    user.wait_for_statuses(
        &worker_id,
        &[AgentStatus::Idle, AgentStatus::Running],
        Duration::from_secs(60),
    )
    .await?;

    // Phase 3: Crash rounds — synchronous invocations (guaranteed to complete
    // and persist in the oplog), but entries buffer in memory without flushing.
    // Crashing both user and plugin workers loses the in-memory buffer.
    for round in 0..CRASH_ROUNDS {
        for i in 0..INVOCATIONS_PER_ROUND {
            let item_id = format!("R{round}I{i}");
            let item_name = format!("Round {round} Item {i}");
            user.invoke_and_await_agent(
                &component,
                &repo_id,
                "add",
                data_value!(item_id, item_name),
            )
            .await?;
        }

        tracing::info!("Chaos round {round}: crashing workers immediately after {INVOCATIONS_PER_ROUND} synchronous invocations...");
        crash_user_and_plugin_workers(&user, &worker_id, &plugin_component.id).await;

        // Wait for recovery before next round
        user.wait_for_statuses(
            &worker_id,
            &[AgentStatus::Idle, AgentStatus::Running],
            Duration::from_secs(60),
        )
        .await?;
    }

    // Phase 4: Restart executor(s) with default thresholds so that recovery
    // can replay missed entries and new invocations flush promptly.
    tracing::info!("Restarting executor(s) with default flush thresholds...");
    unsafe {
        std::env::remove_var("GOLEM__OPLOG__PLUGIN_MAX_COMMIT_COUNT");
        std::env::remove_var("GOLEM__OPLOG__PLUGIN_MAX_ELAPSED_TIME");
    }
    cluster.kill_all().await;
    cluster.restart_all().await;

    // Wait for the worker to recover after executor restart
    user.wait_for_statuses(
        &worker_id,
        &[AgentStatus::Idle, AgentStatus::Running],
        Duration::from_secs(60),
    )
    .await?;

    // Final synchronous invocations to trigger activity and flush
    for i in 0..5 {
        user.invoke_and_await_agent(
            &component,
            &repo_id,
            "add",
            data_value!(format!("FINAL{i}"), format!("Final Item {i}")),
        )
        .await?;
    }

    // Phase 5: Wait for all callbacks to arrive.
    // Expected: agent-initialization (1) + initial adds (3) + crash-round adds
    // (CRASH_ROUNDS * INVOCATIONS_PER_ROUND) + final adds (5).
    let expected_total_adds = 3 + (CRASH_ROUNDS * INVOCATIONS_PER_ROUND) + 5;
    let expected_total_callbacks = 1 + expected_total_adds; // +1 for agent-initialization

    let batches = match tokio::time::timeout(Duration::from_secs(90), async {
        loop {
            let b = received_batches.lock().unwrap().clone();
            if invocation_count(&b) >= expected_total_callbacks {
                return b;
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    })
    .await
    {
        Ok(b) => b,
        Err(_) => {
            // Timed out — collect whatever we have
            received_batches.lock().unwrap().clone()
        }
    };

    let fn_names = extract_function_names(&batches);
    let add_count = fn_names.iter().filter(|f| *f == "add").count();
    let unknown_count = fn_names.iter().filter(|f| *f == "unknown").count();
    let init_count = fn_names
        .iter()
        .filter(|f| *f == "agent-initialization")
        .count();

    // Count completed adds in the oplog via InvocationFinished entries
    let oplog = user.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    let completed_invocations_in_oplog = oplog
        .iter()
        .filter(|entry| format!("{:?}", entry.entry).contains("InvocationFinished"))
        .count();
    // Subtract 1 for agent-initialization's InvocationFinished
    let completed_adds_in_oplog = completed_invocations_in_oplog.saturating_sub(1);

    // "unknown" callbacks come from AgentInvocationFinished entries whose
    // matching AgentInvocationStarted was in a prior batch sent to a
    // different plugin worker instance (e.g. after shard reassignment /
    // locality recovery). These are still valid add invocations; they
    // just couldn't be labelled.
    let effective_add_count = add_count + unknown_count;

    tracing::info!(
        "After {CRASH_ROUNDS} crash rounds: received {} invocations \
         ({add_count} adds, {unknown_count} unknown, {init_count} inits). \
         Oplog has {completed_adds_in_oplog} completed adds (expected {expected_total_adds}).",
        invocation_count(&batches),
    );

    // No duplicate oplog indices ever
    assert_unique_oplog_indices(&batches);

    // All synchronous invocations must appear in the oplog
    assert_eq!(
        completed_adds_in_oplog, expected_total_adds,
        "All synchronous invocations must appear in the oplog"
    );

    // Every completed add must have exactly one callback delivery.
    // Some may be labelled "unknown" when a batch boundary splits
    // AgentInvocationStarted / AgentInvocationFinished and locality
    // recovery migrated to a new plugin worker in between.
    assert_eq!(
        effective_add_count, completed_adds_in_oplog,
        "Oplog processor must deliver exactly one callback per completed invocation. \
         Worker completed {completed_adds_in_oplog} adds but oplog processor delivered \
         {effective_add_count} callbacks ({add_count} add + {unknown_count} unknown)."
    );

    Ok(())
}

// ============================================================================
// E5: No duplicates after crash (documents desired exactly-once behavior)
// ============================================================================

#[test]
#[tracing::instrument]
async fn oplog_processor_no_duplicates_after_crash(
    deps: &EnvBasedTestDependencies,
) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let client = user.registry_service_client().await;
    let (_, env) = user.app_and_env().await?;

    let (callback_url, received_batches, _http_server) = start_callback_server().await;

    let plugin_component = user
        .component(&env.id, "oplog_processor_release")
        .store()
        .await?;
    let oplog_processor_plugin = client
        .create_plugin(
            &user.account_id.0,
            &PluginRegistrationCreation {
                name: "oplog-processor-1".to_string(),
                version: "v1".to_string(),
                description: "A test".to_string(),
                icon: Base64(Vec::new()),
                homepage: "none".to_string(),
                spec: PluginSpecDto::OplogProcessor(OplogProcessorPluginSpec {
                    component_id: plugin_component.id,
                    component_revision: plugin_component.revision,
                }),
            },
        )
        .await?;
    let oplog_processor_plugin_grant = client
        .create_environment_plugin_grant(
            &env.id.0,
            &EnvironmentPluginGrantCreation {
                plugin_registration_id: oplog_processor_plugin.id,
            },
        )
        .await?;

    let mut plugin_params = BTreeMap::new();
    plugin_params.insert("callback-url".to_string(), callback_url.clone());

    let component = user
        .component(&env.id, "it_agent_counters_release")
        .name("it:agent-counters")
        .with_parametrized_plugin(&oplog_processor_plugin_grant.id, 0, plugin_params)
        .store()
        .await?;

    let repo_id = agent_id!("Repository", "worker1");
    let worker_id = user.start_agent(&component.id, repo_id.clone()).await?;

    user.invoke_and_await_agent(
        &component,
        &repo_id,
        "add",
        data_value!("G1000", "Golem T-Shirt M"),
    )
    .await?;
    user.invoke_and_await_agent(
        &component,
        &repo_id,
        "add",
        data_value!("G1001", "Golem Cloud Subscription 1y"),
    )
    .await?;
    user.invoke_and_await_agent(
        &component,
        &repo_id,
        "add",
        data_value!("G1002", "Mud Golem"),
    )
    .await?;

    // Wait for flush to complete
    let _ = wait_for_invocations(&received_batches, 4, Duration::from_secs(60)).await;
    tokio::time::sleep(Duration::from_secs(10)).await;

    // Crash both user worker and oplog processor plugin worker via simulated_crash
    crash_user_and_plugin_workers(&user, &worker_id, &plugin_component.id).await;
    user.wait_for_statuses(
        &worker_id,
        &[AgentStatus::Idle, AgentStatus::Running],
        Duration::from_secs(60),
    )
    .await?;

    user.invoke_and_await_agent(
        &component,
        &repo_id,
        "add",
        data_value!("G1003", "Another Item"),
    )
    .await?;

    let batches = wait_for_invocations(&received_batches, 5, Duration::from_secs(60)).await;
    assert_function_names(
        &batches,
        &["agent-initialization", "add", "add", "add", "add"],
    );
    // With exactly-once semantics, there should be no duplicate oplog indices.
    // May fail with current best-effort delivery — documents desired behavior.
    assert_unique_oplog_indices(&batches);

    Ok(())
}

// ============================================================================
// E6: Multiple plugins — independent tracking
// ============================================================================

#[test]
#[tracing::instrument]
async fn oplog_processor_multiple_plugins_independent(
    deps: &EnvBasedTestDependencies,
) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let client = user.registry_service_client().await;
    let (_, env) = user.app_and_env().await?;

    let (callback_url_a, received_a, _server_a) = start_callback_server().await;
    let (callback_url_b, received_b, _server_b) = start_callback_server().await;

    let plugin_component = user
        .component(&env.id, "oplog_processor_release")
        .store()
        .await?;

    // Register plugin A
    let plugin_a = client
        .create_plugin(
            &user.account_id.0,
            &PluginRegistrationCreation {
                name: "oplog-processor-a".to_string(),
                version: "v1".to_string(),
                description: "Plugin A".to_string(),
                icon: Base64(Vec::new()),
                homepage: "none".to_string(),
                spec: PluginSpecDto::OplogProcessor(OplogProcessorPluginSpec {
                    component_id: plugin_component.id,
                    component_revision: plugin_component.revision,
                }),
            },
        )
        .await?;
    let grant_a = client
        .create_environment_plugin_grant(
            &env.id.0,
            &EnvironmentPluginGrantCreation {
                plugin_registration_id: plugin_a.id,
            },
        )
        .await?;

    // Register plugin B
    let plugin_b = client
        .create_plugin(
            &user.account_id.0,
            &PluginRegistrationCreation {
                name: "oplog-processor-b".to_string(),
                version: "v1".to_string(),
                description: "Plugin B".to_string(),
                icon: Base64(Vec::new()),
                homepage: "none".to_string(),
                spec: PluginSpecDto::OplogProcessor(OplogProcessorPluginSpec {
                    component_id: plugin_component.id,
                    component_revision: plugin_component.revision,
                }),
            },
        )
        .await?;
    let grant_b = client
        .create_environment_plugin_grant(
            &env.id.0,
            &EnvironmentPluginGrantCreation {
                plugin_registration_id: plugin_b.id,
            },
        )
        .await?;

    let mut params_a = BTreeMap::new();
    params_a.insert("callback-url".to_string(), callback_url_a);
    let mut params_b = BTreeMap::new();
    params_b.insert("callback-url".to_string(), callback_url_b);

    let component = user
        .component(&env.id, "it_agent_counters_release")
        .name("it:agent-counters")
        .with_parametrized_plugin(&grant_a.id, 0, params_a)
        .with_parametrized_plugin(&grant_b.id, 1, params_b)
        .store()
        .await?;

    let repo_id = agent_id!("Repository", "worker1");
    let _worker_id = user.start_agent(&component.id, repo_id.clone()).await?;

    user.invoke_and_await_agent(
        &component,
        &repo_id,
        "add",
        data_value!("G1000", "Golem T-Shirt M"),
    )
    .await?;
    user.invoke_and_await_agent(
        &component,
        &repo_id,
        "add",
        data_value!("G1001", "Golem Cloud Subscription 1y"),
    )
    .await?;
    user.invoke_and_await_agent(
        &component,
        &repo_id,
        "add",
        data_value!("G1002", "Mud Golem"),
    )
    .await?;

    let batches_a = wait_for_invocations(&received_a, 4, Duration::from_secs(60)).await;
    let batches_b = wait_for_invocations(&received_b, 4, Duration::from_secs(60)).await;

    assert_function_names(&batches_a, &["agent-initialization", "add", "add", "add"]);
    assert_function_names(&batches_b, &["agent-initialization", "add", "add", "add"]);
    assert_unique_oplog_indices(&batches_a);
    assert_unique_oplog_indices(&batches_b);

    Ok(())
}

// ============================================================================
// E7: Partial plugin failure — working plugin still receives entries
// ============================================================================

#[test]
#[tracing::instrument]
async fn oplog_processor_partial_plugin_failure(
    deps: &EnvBasedTestDependencies,
) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let client = user.registry_service_client().await;
    let (_, env) = user.app_and_env().await?;

    // Plugin A: working callback
    let (callback_url_a, received_a, _server_a) = start_callback_server().await;

    let plugin_component = user
        .component(&env.id, "oplog_processor_release")
        .store()
        .await?;

    let plugin_a = client
        .create_plugin(
            &user.account_id.0,
            &PluginRegistrationCreation {
                name: "oplog-processor-good".to_string(),
                version: "v1".to_string(),
                description: "Working plugin".to_string(),
                icon: Base64(Vec::new()),
                homepage: "none".to_string(),
                spec: PluginSpecDto::OplogProcessor(OplogProcessorPluginSpec {
                    component_id: plugin_component.id,
                    component_revision: plugin_component.revision,
                }),
            },
        )
        .await?;
    let grant_a = client
        .create_environment_plugin_grant(
            &env.id.0,
            &EnvironmentPluginGrantCreation {
                plugin_registration_id: plugin_a.id,
            },
        )
        .await?;

    // Plugin B: unreachable callback URL — will fail fast on connection refused
    let plugin_b = client
        .create_plugin(
            &user.account_id.0,
            &PluginRegistrationCreation {
                name: "oplog-processor-bad".to_string(),
                version: "v1".to_string(),
                description: "Failing plugin".to_string(),
                icon: Base64(Vec::new()),
                homepage: "none".to_string(),
                spec: PluginSpecDto::OplogProcessor(OplogProcessorPluginSpec {
                    component_id: plugin_component.id,
                    component_revision: plugin_component.revision,
                }),
            },
        )
        .await?;
    let grant_b = client
        .create_environment_plugin_grant(
            &env.id.0,
            &EnvironmentPluginGrantCreation {
                plugin_registration_id: plugin_b.id,
            },
        )
        .await?;

    let mut params_a = BTreeMap::new();
    params_a.insert("callback-url".to_string(), callback_url_a);
    let mut params_b = BTreeMap::new();
    params_b.insert(
        "callback-url".to_string(),
        "http://127.0.0.1:1/fail".to_string(),
    );

    let component = user
        .component(&env.id, "it_agent_counters_release")
        .name("it:agent-counters")
        .with_parametrized_plugin(&grant_a.id, 0, params_a)
        .with_parametrized_plugin(&grant_b.id, 1, params_b)
        .store()
        .await?;

    let repo_id = agent_id!("Repository", "worker1");
    let worker_id = user.start_agent(&component.id, repo_id.clone()).await?;

    user.invoke_and_await_agent(
        &component,
        &repo_id,
        "add",
        data_value!("G1000", "Golem T-Shirt M"),
    )
    .await?;
    user.invoke_and_await_agent(
        &component,
        &repo_id,
        "add",
        data_value!("G1001", "Golem Cloud Subscription 1y"),
    )
    .await?;
    user.invoke_and_await_agent(
        &component,
        &repo_id,
        "add",
        data_value!("G1002", "Mud Golem"),
    )
    .await?;

    // Plugin A must receive exactly the expected entries with no duplicates.
    // Current bug: re-buffer on partial failure (plugin B fails) causes plugin A
    // to receive duplicate entries.
    let batches_a = wait_for_invocations(&received_a, 4, Duration::from_secs(60)).await;
    assert_function_names(&batches_a, &["agent-initialization", "add", "add", "add"]);
    assert_unique_oplog_indices(&batches_a);

    // Worker should still be running despite plugin B's failure
    let metadata = user.get_worker_metadata(&worker_id).await?;
    assert_ne!(
        metadata.status,
        AgentStatus::Failed,
        "Worker should not have failed due to plugin failure"
    );

    Ok(())
}

// ============================================================================
// E8: Plugin activation mid-stream
// ============================================================================

#[test]
#[tracing::instrument]
async fn oplog_processor_activation_mid_stream(
    deps: &EnvBasedTestDependencies,
) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let client = user.registry_service_client().await;
    let (_, env) = user.app_and_env().await?;

    let (callback_url, received_batches, _http_server) = start_callback_server().await;

    // Create component WITHOUT plugin
    let component = user
        .component(&env.id, "it_agent_counters_release")
        .name("it:agent-counters")
        .store()
        .await?;

    let repo_id = agent_id!("Repository", "worker1");
    let worker_id = user.start_agent(&component.id, repo_id.clone()).await?;

    // Invoke twice BEFORE plugin is activated
    user.invoke_and_await_agent(
        &component,
        &repo_id,
        "add",
        data_value!("G1000", "Golem T-Shirt M"),
    )
    .await?;
    user.invoke_and_await_agent(
        &component,
        &repo_id,
        "add",
        data_value!("G1001", "Golem Cloud Subscription 1y"),
    )
    .await?;

    // Now register plugin and add it to the component
    let plugin_component = user
        .component(&env.id, "oplog_processor_release")
        .store()
        .await?;
    let oplog_processor_plugin = client
        .create_plugin(
            &user.account_id.0,
            &PluginRegistrationCreation {
                name: "oplog-processor-1".to_string(),
                version: "v1".to_string(),
                description: "A test".to_string(),
                icon: Base64(Vec::new()),
                homepage: "none".to_string(),
                spec: PluginSpecDto::OplogProcessor(OplogProcessorPluginSpec {
                    component_id: plugin_component.id,
                    component_revision: plugin_component.revision,
                }),
            },
        )
        .await?;
    let oplog_processor_plugin_grant = client
        .create_environment_plugin_grant(
            &env.id.0,
            &EnvironmentPluginGrantCreation {
                plugin_registration_id: oplog_processor_plugin.id,
            },
        )
        .await?;

    let mut plugin_params = BTreeMap::new();
    plugin_params.insert("callback-url".to_string(), callback_url.clone());

    let latest = user.get_latest_component_revision(&component.id).await?;
    let updated_component = user
        .update_component_with(
            &component.id,
            latest.revision,
            None,
            Vec::new(),
            Vec::new(),
            None,
            None,
            None,
            vec![PluginInstallationAction::Install(PluginInstallation {
                environment_plugin_grant_id: oplog_processor_plugin_grant.id,
                priority: PluginPriority(0),
                parameters: plugin_params,
            })],
        )
        .await?;

    user.auto_update_worker(&worker_id, updated_component.revision, false)
        .await?;
    user.wait_for_status(&worker_id, AgentStatus::Idle, Duration::from_secs(30))
        .await?;

    // Invoke twice AFTER plugin is activated
    user.invoke_and_await_agent(
        &updated_component,
        &repo_id,
        "add",
        data_value!("G1002", "Mud Golem"),
    )
    .await?;
    user.invoke_and_await_agent(
        &updated_component,
        &repo_id,
        "add",
        data_value!("G1003", "Another Item"),
    )
    .await?;

    // Plugin should only receive post-activation entries, NOT the 2 pre-activation adds.
    let batches = wait_for_invocations(&received_batches, 2, Duration::from_secs(60)).await;
    let fn_names = extract_function_names(&batches);
    assert_eq!(
        fn_names.iter().filter(|f| f.as_str() == "add").count(),
        2,
        "Expected exactly 2 post-activation add callbacks (no pre-activation leakage), got: {:?}",
        fn_names
    );
    assert_unique_oplog_indices(&batches);

    Ok(())
}

// ============================================================================
// E9: Plugin deactivation — no delivery after uninstall
// ============================================================================

#[test]
#[tracing::instrument]
async fn oplog_processor_deactivation(deps: &EnvBasedTestDependencies) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let client = user.registry_service_client().await;
    let (_, env) = user.app_and_env().await?;

    let (callback_url, received_batches, _http_server) = start_callback_server().await;

    let plugin_component = user
        .component(&env.id, "oplog_processor_release")
        .store()
        .await?;
    let oplog_processor_plugin = client
        .create_plugin(
            &user.account_id.0,
            &PluginRegistrationCreation {
                name: "oplog-processor-1".to_string(),
                version: "v1".to_string(),
                description: "A test".to_string(),
                icon: Base64(Vec::new()),
                homepage: "none".to_string(),
                spec: PluginSpecDto::OplogProcessor(OplogProcessorPluginSpec {
                    component_id: plugin_component.id,
                    component_revision: plugin_component.revision,
                }),
            },
        )
        .await?;
    let oplog_processor_plugin_grant = client
        .create_environment_plugin_grant(
            &env.id.0,
            &EnvironmentPluginGrantCreation {
                plugin_registration_id: oplog_processor_plugin.id,
            },
        )
        .await?;

    let mut plugin_params = BTreeMap::new();
    plugin_params.insert("callback-url".to_string(), callback_url.clone());

    let component = user
        .component(&env.id, "it_agent_counters_release")
        .name("it:agent-counters")
        .with_parametrized_plugin(&oplog_processor_plugin_grant.id, 0, plugin_params)
        .store()
        .await?;

    let repo_id = agent_id!("Repository", "worker1");
    let worker_id = user.start_agent(&component.id, repo_id.clone()).await?;

    user.invoke_and_await_agent(
        &component,
        &repo_id,
        "add",
        data_value!("G1000", "Golem T-Shirt M"),
    )
    .await?;
    user.invoke_and_await_agent(
        &component,
        &repo_id,
        "add",
        data_value!("G1001", "Golem Cloud Subscription 1y"),
    )
    .await?;
    user.invoke_and_await_agent(
        &component,
        &repo_id,
        "add",
        data_value!("G1002", "Mud Golem"),
    )
    .await?;

    // Wait for initial delivery
    let batches = wait_for_invocations(&received_batches, 4, Duration::from_secs(60)).await;
    assert_function_names(&batches, &["agent-initialization", "add", "add", "add"]);
    let pre_deactivation_count = invocation_count(&batches);

    // Uninstall the plugin via component update
    let latest = user.get_latest_component_revision(&component.id).await?;
    let updated_component = user
        .update_component_with(
            &component.id,
            latest.revision,
            None,
            Vec::new(),
            Vec::new(),
            None,
            None,
            None,
            vec![PluginInstallationAction::Uninstall(PluginUninstallation {
                environment_plugin_grant_id: oplog_processor_plugin_grant.id,
            })],
        )
        .await?;

    user.auto_update_worker(&worker_id, updated_component.revision, false)
        .await?;
    user.wait_for_status(&worker_id, AgentStatus::Idle, Duration::from_secs(30))
        .await?;

    // Invoke after deactivation
    user.invoke_and_await_agent(
        &updated_component,
        &repo_id,
        "add",
        data_value!("G1003", "Post-deactivation item 1"),
    )
    .await?;
    user.invoke_and_await_agent(
        &updated_component,
        &repo_id,
        "add",
        data_value!("G1004", "Post-deactivation item 2"),
    )
    .await?;

    // Wait and verify no new callbacks arrived
    tokio::time::sleep(Duration::from_secs(15)).await;
    let final_batches = received_batches.lock().unwrap().clone();
    let final_count = invocation_count(&final_batches);
    assert_eq!(
        final_count,
        pre_deactivation_count,
        "No new callbacks should arrive after plugin deactivation, got {final_count} vs {pre_deactivation_count}"
    );

    Ok(())
}

// ============================================================================
// E10: Idle worker — timer flush delivers small batches
// ============================================================================

#[test]
#[tracing::instrument]
async fn oplog_processor_idle_worker_timer_flush(
    deps: &EnvBasedTestDependencies,
) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let client = user.registry_service_client().await;
    let (_, env) = user.app_and_env().await?;

    let (callback_url, received_batches, _http_server) = start_callback_server().await;

    let plugin_component = user
        .component(&env.id, "oplog_processor_release")
        .store()
        .await?;
    let oplog_processor_plugin = client
        .create_plugin(
            &user.account_id.0,
            &PluginRegistrationCreation {
                name: "oplog-processor-1".to_string(),
                version: "v1".to_string(),
                description: "A test".to_string(),
                icon: Base64(Vec::new()),
                homepage: "none".to_string(),
                spec: PluginSpecDto::OplogProcessor(OplogProcessorPluginSpec {
                    component_id: plugin_component.id,
                    component_revision: plugin_component.revision,
                }),
            },
        )
        .await?;
    let oplog_processor_plugin_grant = client
        .create_environment_plugin_grant(
            &env.id.0,
            &EnvironmentPluginGrantCreation {
                plugin_registration_id: oplog_processor_plugin.id,
            },
        )
        .await?;

    let mut plugin_params = BTreeMap::new();
    plugin_params.insert("callback-url".to_string(), callback_url.clone());

    let component = user
        .component(&env.id, "it_agent_counters_release")
        .name("it:agent-counters")
        .with_parametrized_plugin(&oplog_processor_plugin_grant.id, 0, plugin_params)
        .store()
        .await?;

    let repo_id = agent_id!("Repository", "worker1");
    let _worker_id = user.start_agent(&component.id, repo_id.clone()).await?;

    // Just one invocation (+ init) — small batch, must be delivered by timer (5s)
    // not by commit-count threshold (MAX_COMMIT_COUNT=3).
    let t0 = tokio::time::Instant::now();
    user.invoke_and_await_agent(
        &component,
        &repo_id,
        "add",
        data_value!("G1000", "Golem T-Shirt M"),
    )
    .await?;
    let invoke_done = t0.elapsed();

    let batches = wait_for_invocations(&received_batches, 2, Duration::from_secs(60)).await;
    let callback_arrived = t0.elapsed();
    let fn_names = extract_function_names(&batches);
    assert!(fn_names.contains(&"agent-initialization".to_string()));
    assert!(fn_names.contains(&"add".to_string()));
    assert_unique_oplog_indices(&batches);

    // If the timer flush is working, callbacks should arrive ~5s after invocation,
    // not immediately. Log the timing for observability.
    tracing::info!(
        "E10: invoke completed in {:?}, callbacks arrived in {:?} (timer should add ~5s)",
        invoke_done,
        callback_arrived,
    );

    Ok(())
}

// ============================================================================
// E11: Rapid invocations — no loss under burst
// ============================================================================

#[test]
#[tracing::instrument]
async fn oplog_processor_rapid_invocations(deps: &EnvBasedTestDependencies) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let client = user.registry_service_client().await;
    let (_, env) = user.app_and_env().await?;

    let (callback_url, received_batches, _http_server) = start_callback_server().await;

    let plugin_component = user
        .component(&env.id, "oplog_processor_release")
        .store()
        .await?;
    let oplog_processor_plugin = client
        .create_plugin(
            &user.account_id.0,
            &PluginRegistrationCreation {
                name: "oplog-processor-1".to_string(),
                version: "v1".to_string(),
                description: "A test".to_string(),
                icon: Base64(Vec::new()),
                homepage: "none".to_string(),
                spec: PluginSpecDto::OplogProcessor(OplogProcessorPluginSpec {
                    component_id: plugin_component.id,
                    component_revision: plugin_component.revision,
                }),
            },
        )
        .await?;
    let oplog_processor_plugin_grant = client
        .create_environment_plugin_grant(
            &env.id.0,
            &EnvironmentPluginGrantCreation {
                plugin_registration_id: oplog_processor_plugin.id,
            },
        )
        .await?;

    let mut plugin_params = BTreeMap::new();
    plugin_params.insert("callback-url".to_string(), callback_url.clone());

    let component = user
        .component(&env.id, "it_agent_counters_release")
        .name("it:agent-counters")
        .with_parametrized_plugin(&oplog_processor_plugin_grant.id, 0, plugin_params)
        .store()
        .await?;

    let repo_id = agent_id!("Repository", "worker1");
    let _worker_id = user.start_agent(&component.id, repo_id.clone()).await?;

    // Rapid-fire 50 parallel invocations to stress the oplog processor
    let n = 50usize;
    let component = Arc::new(component);
    let mut tasks = JoinSet::new();
    for i in 0..n {
        let user = user.clone();
        let component = component.clone();
        let repo_id = repo_id.clone();
        tasks.spawn(async move {
            user.invoke_and_await_agent(
                &component,
                &repo_id,
                "add",
                data_value!(format!("G{i}"), format!("Item {i}")),
            )
            .await
        });
    }
    while let Some(r) = tasks.join_next().await {
        r.unwrap()?;
    }

    // init + n adds
    let batches = wait_for_invocations(&received_batches, 1 + n, Duration::from_secs(120)).await;

    let fn_names = extract_function_names(&batches);
    assert!(
        fn_names
            .iter()
            .filter(|f| f.as_str() == "agent-initialization")
            .count()
            >= 1,
        "Expected at least one agent-initialization callback"
    );
    assert_eq!(
        fn_names.iter().filter(|f| f.as_str() == "add").count(),
        n,
        "Expected exactly {n} add callbacks"
    );
    assert_unique_oplog_indices(&batches);

    Ok(())
}

// ============================================================================
// E12: No resend after confirmed delivery
// ============================================================================

#[test]
#[tracing::instrument]
async fn oplog_processor_no_resend_after_confirmed(
    deps: &EnvBasedTestDependencies,
) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let client = user.registry_service_client().await;
    let (_, env) = user.app_and_env().await?;

    let (callback_url, received_batches, _http_server) = start_callback_server().await;

    let plugin_component = user
        .component(&env.id, "oplog_processor_release")
        .store()
        .await?;
    let oplog_processor_plugin = client
        .create_plugin(
            &user.account_id.0,
            &PluginRegistrationCreation {
                name: "oplog-processor-1".to_string(),
                version: "v1".to_string(),
                description: "A test".to_string(),
                icon: Base64(Vec::new()),
                homepage: "none".to_string(),
                spec: PluginSpecDto::OplogProcessor(OplogProcessorPluginSpec {
                    component_id: plugin_component.id,
                    component_revision: plugin_component.revision,
                }),
            },
        )
        .await?;
    let oplog_processor_plugin_grant = client
        .create_environment_plugin_grant(
            &env.id.0,
            &EnvironmentPluginGrantCreation {
                plugin_registration_id: oplog_processor_plugin.id,
            },
        )
        .await?;

    let mut plugin_params = BTreeMap::new();
    plugin_params.insert("callback-url".to_string(), callback_url.clone());

    let component = user
        .component(&env.id, "it_agent_counters_release")
        .name("it:agent-counters")
        .with_parametrized_plugin(&oplog_processor_plugin_grant.id, 0, plugin_params)
        .store()
        .await?;

    let repo_id = agent_id!("Repository", "worker1");
    let worker_id = user.start_agent(&component.id, repo_id.clone()).await?;

    user.invoke_and_await_agent(
        &component,
        &repo_id,
        "add",
        data_value!("G1000", "Golem T-Shirt M"),
    )
    .await?;
    user.invoke_and_await_agent(
        &component,
        &repo_id,
        "add",
        data_value!("G1001", "Golem Cloud Subscription 1y"),
    )
    .await?;
    user.invoke_and_await_agent(
        &component,
        &repo_id,
        "add",
        data_value!("G1002", "Mud Golem"),
    )
    .await?;

    // Wait for delivery to complete
    let _ = wait_for_invocations(&received_batches, 4, Duration::from_secs(60)).await;
    // Extra sleep to ensure flush + any confirmation is done
    tokio::time::sleep(Duration::from_secs(5)).await;

    // Crash both user worker and oplog processor plugin worker via simulated_crash
    crash_user_and_plugin_workers(&user, &worker_id, &plugin_component.id).await;
    user.wait_for_statuses(
        &worker_id,
        &[AgentStatus::Idle, AgentStatus::Running],
        Duration::from_secs(60),
    )
    .await?;

    // Wait to see if any resends happen
    tokio::time::sleep(Duration::from_secs(15)).await;
    let final_batches = received_batches.lock().unwrap().clone();
    let final_count = invocation_count(&final_batches);

    // With exactly-once semantics (checkpoint-based confirmation), count must remain 4.
    // Current bug: no checkpoint, so crash recovery re-delivers already-confirmed entries.
    assert_eq!(
        final_count, 4,
        "No resend after confirmed delivery — got {final_count} invocations instead of 4"
    );

    Ok(())
}
