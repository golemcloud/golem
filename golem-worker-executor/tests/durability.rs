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
use crate::filesystem_snapshots::{InvocationShape, invocation_shape};
use axum::Router;
use axum::extract::Query;
use axum::routing::{any, get};
use golem_api_grpc::proto::golem::worker::LogEvent;
use golem_common::model::account::AccountId;
use golem_common::model::agent::{GolemUserPrincipal, Principal};
use golem_common::model::oplog::{
    OplogIndex, PublicAgentInvocation, PublicOplogEntry, PublicOplogEntryWithIndex,
    PublicSnapshotData,
};
use golem_common::model::worker::AgentConfigEntryDto;
use golem_common::model::{AgentEvent, AgentId, AgentStatus, OwnedAgentId};
use golem_common::{agent_id, data_value, phantom_agent_id};
use golem_test_framework::dsl::TestDsl;
use golem_worker_executor::services::golem_config::SnapshotPolicy;
use golem_worker_executor_test_utils::{
    LastUniqueId, PrecompiledComponent, TestContext, WorkerExecutorTestDependencies, start,
    start_with_snapshot_policy,
};
use pretty_assertions::assert_eq;
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::Duration;
use test_r::{inherit_test_dep, test, timeout};
use tokio::sync::mpsc::UnboundedReceiver;
use tracing::Instrument;

inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(LastUniqueId);
inherit_test_dep!(
    #[tagged_as("host_api_tests")]
    PrecompiledComponent
);
inherit_test_dep!(
    #[tagged_as("agent_counters")]
    PrecompiledComponent
);
inherit_test_dep!(
    #[tagged_as("constructor_parameter_echo")]
    PrecompiledComponent
);
inherit_test_dep!(
    #[tagged_as("agent_sdk_rust")]
    PrecompiledComponent
);
inherit_test_dep!(Tracing);

fn total_memory_growth(entries: &[PublicOplogEntryWithIndex]) -> u64 {
    entries
        .iter()
        .filter_map(|entry| match &entry.entry {
            PublicOplogEntry::GrowMemory(params) => Some(params.delta),
            _ => None,
        })
        .sum()
}

/// Expects the snapshot to be loaded and kept during recovery. Must be called after the first
/// invocation on the recovered worker completed: the events are read up to that invocation's
/// `InvocationFinished`, so a snapshot that was loaded but abandoned afterwards because its replayed
/// tail diverged fails the assertion too.
pub(crate) async fn assert_snapshot_recovery_loaded(events: &mut UnboundedReceiver<LogEvent>) {
    tokio::time::timeout(Duration::from_secs(10), async {
        let mut loaded = false;
        while let Some(event) = events.recv().await {
            match AgentEvent::try_from(event) {
                Ok(AgentEvent::SnapshotRecoverySucceeded { .. }) => loaded = true,
                Ok(AgentEvent::SnapshotRecoveryFailed {
                    snapshot_index,
                    error,
                    ..
                }) => {
                    panic!("Snapshot recovery from {snapshot_index} failed: {error}");
                }
                Ok(AgentEvent::InvocationFinished { .. }) => {
                    assert!(
                        loaded,
                        "Invocation finished without a snapshot recovery event"
                    );
                    return;
                }
                _ => {}
            }
        }
        panic!("Worker event stream ended before snapshot recovery event");
    })
    .await
    .expect("Timed out waiting for snapshot recovery event");
}

/// Expects the recovery to reject the snapshot at one index with `expected_error` and then to
/// load the snapshot at another index. Gives the rejected index and the loaded index.
async fn snapshot_recovery_fell_back(
    events: &mut UnboundedReceiver<LogEvent>,
    expected_error: &str,
) -> (OplogIndex, OplogIndex) {
    tokio::time::timeout(Duration::from_secs(10), async {
        use futures::StreamExt as _;
        // The stream pulls one event at a time, and `next` stops at the first success, so it
        // consumes the events up to that success and no more.
        let fell_back = futures::stream::poll_fn(|context| events.poll_recv(context))
            .scan(None, |rejected: &mut Option<OplogIndex>, event| {
                let found = match AgentEvent::try_from(event) {
                    Ok(AgentEvent::SnapshotRecoveryFailed {
                        snapshot_index,
                        error,
                        ..
                    }) => {
                        assert!(
                            rejected.is_none(),
                            "Snapshot recovery from {snapshot_index} failed after an earlier failure: {error}"
                        );
                        assert!(
                            error.contains(expected_error),
                            "Snapshot recovery failed with unexpected error: {error}"
                        );
                        *rejected = Some(snapshot_index);
                        None
                    }
                    Ok(AgentEvent::SnapshotRecoverySucceeded { snapshot_index, .. }) => {
                        let rejected = rejected.unwrap_or_else(|| {
                            panic!(
                                "Snapshot recovery from {snapshot_index} succeeded before a failure"
                            )
                        });
                        Some((rejected, snapshot_index))
                    }
                    _ => None,
                };
                std::future::ready(Some(found))
            })
            .filter_map(std::future::ready);
        std::pin::pin!(fell_back)
            .next()
            .await
            .unwrap_or_else(|| {
                panic!("Worker event stream ended before the snapshot recovery events")
            })
    })
    .await
    .expect("Timed out waiting for the snapshot recovery events")
}

pub(crate) async fn assert_snapshot_recovery_failed(
    events: &mut UnboundedReceiver<LogEvent>,
    expected_error: &str,
) {
    tokio::time::timeout(Duration::from_secs(10), async {
        while let Some(event) = events.recv().await {
            match AgentEvent::try_from(event) {
                Ok(AgentEvent::SnapshotRecoveryFailed { error, .. }) => {
                    assert!(
                        error.contains(expected_error),
                        "Snapshot recovery failed with unexpected error: {error}"
                    );
                    return;
                }
                Ok(AgentEvent::SnapshotRecoverySucceeded { snapshot_index, .. }) => {
                    panic!("Snapshot recovery from {snapshot_index} unexpectedly succeeded");
                }
                _ => {}
            }
        }
        panic!("Worker event stream ended before snapshot recovery event");
    })
    .await
    .expect("Timed out waiting for snapshot recovery event");
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SnapshotLoadProbeStatus {
    value: u64,
    loaded_value: Option<u64>,
    origin: String,
    mode: String,
    principal: String,
    agent_type: String,
    phantom_id: Option<String>,
    config_marker: String,
    read_bytes: usize,
    constructor_calls_at_restore: u32,
    constructor_calls_now: u32,
    load_calls_now: u32,
}

#[test]
#[tracing::instrument]
async fn custom_durability_1(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let response = Arc::new(AtomicU32::new(0));
    let response_clone = response.clone();
    let request_history = Arc::new(Mutex::new(Vec::new()));
    let request_history_in_server = request_history.clone();

    let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await.unwrap();

    let host_http_port = listener.local_addr().unwrap().port();

    #[derive(Deserialize)]
    struct QueryParams {
        payload: String,
    }

    let http_server = tokio::spawn(
        async move {
            let route = Router::new().route(
                "/callback",
                get(move |query: Query<QueryParams>| async move {
                    request_history_in_server
                        .lock()
                        .unwrap()
                        .push(query.payload.clone());
                    let result = format!(
                        "{}-{}",
                        response_clone.fetch_add(1, Ordering::AcqRel),
                        query.payload
                    );
                    tracing::info!("responding to callback: {result}");
                    result
                }),
            );

            axum::serve(listener, route).await.unwrap();
        }
        .in_current_span(),
    );

    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;
    let agent_id = agent_id!("CustomDurability", "custom-durability-1");
    let mut env = HashMap::new();
    env.insert("PORT".to_string(), host_http_port.to_string());

    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), env, Vec::new())
        .await?;

    let result1 = executor
        .invoke_and_await_agent(&component, &agent_id, "callback", data_value!("a"))
        .await?;

    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    let logical_starts: Vec<_> = oplog
        .iter()
        .filter(|entry| {
            matches!(&entry.entry, PublicOplogEntry::Start(params) if params.function_name == "golem-it::test-callback")
        })
        .collect();
    assert_eq!(
        logical_starts.len(),
        1,
        "one custom logical call is recorded"
    );
    let logical_start = logical_starts[0].oplog_index;
    assert!(oplog.iter().any(
        |entry| matches!(&entry.entry, PublicOplogEntry::End(params) if params.start_index == logical_start)
    ));
    let observational_starts: Vec<_> = oplog
        .iter()
        .filter(|entry| {
            matches!(
                &entry.entry,
                PublicOplogEntry::Start(params)
                    if params.observational_owner == Some(logical_start)
            )
        })
        .collect();
    assert!(
        !observational_starts.is_empty(),
        "host calls owned by the custom invocation are recorded as ordinary observational calls"
    );
    assert!(
        oplog.iter().any(|entry| {
            matches!(
                &entry.entry,
                PublicOplogEntry::Start(params)
                    if params.function_name == "golem::api::get_self_metadata"
                        && params.observational_owner == Some(logical_start)
            )
        }),
        "every host effect initiated by the live custom invocation is observational; starts: {:#?}",
        oplog
            .iter()
            .filter_map(|entry| match &entry.entry {
                PublicOplogEntry::Start(params) => Some((
                    entry.oplog_index,
                    &params.function_name,
                    params.observational_owner,
                )),
                _ => None,
            })
            .collect::<Vec<_>>()
    );
    executor.check_oplog_is_queryable(&worker_id).await?;

    drop(executor);
    let executor = start(deps, &context).await?;

    let result2 = executor
        .invoke_and_await_agent(&component, &agent_id, "callback", data_value!("b"))
        .await?;

    executor.check_oplog_is_queryable(&worker_id).await?;

    drop(executor);
    http_server.abort();

    assert_eq!(result1.into_typed::<String>()?, "0-a");
    assert_eq!(result2.into_typed::<String>()?, "1-b");
    assert_eq!(&*request_history.lock().unwrap(), &["a", "b"]);
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn custom_durability_counter_ids_handle_nested_and_concurrent_no_input_calls(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;
    let response = Arc::new(AtomicU32::new(0));
    let history = Arc::new(Mutex::new(Vec::<String>::new()));
    let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await?;
    let port = listener.local_addr()?.port();

    #[derive(Deserialize)]
    struct Params {
        payload: String,
    }

    let server_response = response.clone();
    let server_history = history.clone();
    let server = tokio::spawn(
        async move {
            let slow_recorded = Arc::new(tokio::sync::Notify::new());
            let fast_counter_assigned = Arc::new(tokio::sync::Notify::new());
            let route = Router::new().route(
                "/callback",
                any(move |Query(params): Query<Params>| {
                    let response = server_response.clone();
                    let history = server_history.clone();
                    let slow_recorded = slow_recorded.clone();
                    let fast_counter_assigned = fast_counter_assigned.clone();
                    async move {
                        // Record slow first, but assign fast's response counter before slow's.
                        if params.payload == "fast" {
                            slow_recorded.notified().await;
                        }
                        history.lock().unwrap().push(params.payload.clone());
                        if params.payload == "slow" {
                            slow_recorded.notify_one();
                            fast_counter_assigned.notified().await;
                        }
                        let result = format!(
                            "{}-{}",
                            response.fetch_add(1, Ordering::AcqRel),
                            params.payload
                        );
                        if params.payload == "fast" {
                            fast_counter_assigned.notify_one();
                        }
                        result
                    }
                }),
            );
            axum::serve(listener, route).await.unwrap();
        }
        .in_current_span(),
    );

    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;
    let agent_id = agent_id!("CustomDurability", "counter-invocation-ids");
    let mut env = HashMap::new();
    env.insert("PORT".to_string(), port.to_string());
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), env, Vec::new())
        .await?;

    let nested = executor
        .invoke_and_await_agent(&component, &agent_id, "nested_no_input", data_value!())
        .await?
        .into_typed::<Vec<String>>()?;
    assert_eq!(nested, vec!["0-nested", "1-root-sibling"]);

    let concurrent = executor
        .invoke_and_await_agent(&component, &agent_id, "concurrent_no_input", data_value!())
        .await?
        .into_typed::<Vec<String>>()?;

    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    let concurrent_anchor = oplog
        .iter()
        .find_map(|entry| match &entry.entry {
            PublicOplogEntry::AgentInvocationStarted(params)
                if matches!(
                    &params.invocation,
                    PublicAgentInvocation::AgentMethodInvocation(invocation)
                        if invocation.method_name == "concurrent_no_input"
                ) =>
            {
                Some(entry.oplog_index)
            }
            _ => None,
        })
        .expect("concurrent agent invocation");
    let mut concurrent_starts: Vec<_> = oplog
        .iter()
        .filter_map(|entry| match &entry.entry {
            PublicOplogEntry::Start(params)
                if params.function_name == "golem-it::test-generator"
                    && params
                        .invocation_id
                        .is_some_and(|id| id.anchor == concurrent_anchor) =>
            {
                Some((entry.oplog_index, params))
            }
            _ => None,
        })
        .collect();
    concurrent_starts.sort_by_key(|(_, params)| params.invocation_id.unwrap().ordinal);
    assert_eq!(
        concurrent_starts.len(),
        2,
        "both concurrent custom invocations must share the agent invocation anchor"
    );
    for (ordinal, (start_index, params)) in concurrent_starts.iter().enumerate() {
        assert_eq!(
            params.parent_start_index, None,
            "concurrent calls are siblings"
        );
        assert_eq!(params.invocation_id.unwrap().ordinal, ordinal as u64);
        for function_name in ["http::client::send", "http::types::response::consume-body"] {
            let effects: Vec<_> = oplog
                .iter()
                .filter_map(|entry| match &entry.entry {
                    PublicOplogEntry::Start(effect)
                        if effect.function_name == function_name
                            && effect.observational_owner == Some(*start_index) =>
                    {
                        Some(effect)
                    }
                    _ => None,
                })
                .collect();
            assert_eq!(
                effects.len(),
                1,
                "each sibling owns its own {function_name}"
            );
            assert!(effects.iter().all(|effect| effect.invocation_id.is_none()));
        }
    }
    assert_eq!(
        concurrent,
        vec!["3-slow", "2-fast"],
        "the slow call starts first but completes after its identical sibling"
    );
    let custom_starts: Vec<_> = oplog
        .iter()
        .filter_map(|entry| match &entry.entry {
            PublicOplogEntry::Start(params)
                if params.function_name == "golem-it::test-generator"
                    || params.function_name == "golem-it::test-generator-parent" =>
            {
                Some((
                    entry.oplog_index,
                    params.parent_start_index,
                    params.invocation_id,
                ))
            }
            _ => None,
        })
        .collect();
    assert_eq!(custom_starts.len(), 5);
    assert!(custom_starts.iter().all(|(_, _, id)| id.is_some()));
    let unique_ids: std::collections::HashSet<_> = custom_starts
        .iter()
        .map(|(_, _, id)| id.expect("custom Start invocation ID"))
        .collect();
    assert_eq!(unique_ids.len(), custom_starts.len());
    let outer_start = custom_starts
        .iter()
        .find(|(_, parent, _)| parent.is_none())
        .expect("outer custom Start")
        .0;
    assert!(
        custom_starts
            .iter()
            .any(|(_, parent, _)| *parent == Some(outer_start)),
        "the nested generator has the outer custom invocation as its call-tree parent"
    );

    assert_eq!(
        &*history.lock().unwrap(),
        &["nested", "root-sibling", "slow", "fast"]
    );
    executor.simulated_crash(&worker_id).await?;
    let after_recovery = executor
        .invoke_and_await_agent(&component, &agent_id, "callback", data_value!("after"))
        .await?
        .into_typed::<String>()?;
    assert_eq!(after_recovery, "4-after");
    assert_eq!(
        &*history.lock().unwrap(),
        &["nested", "root-sibling", "slow", "fast", "after"],
        "replay must not re-execute nested or concurrent generator effects"
    );

    server.abort();
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn custom_durability_cancelled_finish_drops_result_in_its_scope(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use anyhow::Context as _;
    for fallible in [true, false] {
        let context = TestContext::new(last_unique_id);
        let executor = start(deps, &context).await?;
        let history = Arc::new(Mutex::new(Vec::<String>::new()));
        let cancel = Arc::new(tokio::sync::Semaphore::new(0));
        let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await?;
        let port = listener.local_addr()?.port();
        #[derive(Deserialize)]
        struct Params {
            payload: String,
        }
        let route_history = history.clone();
        let route_cancel = cancel.clone();
        let routes = Router::new()
            .route(
                "/cancel",
                any(move || {
                    let cancel = route_cancel.clone();
                    async move {
                        cancel.acquire().await.unwrap().forget();
                        "cancelled"
                    }
                }),
            )
            .route(
                "/callback",
                any(move |Query(params): Query<Params>| {
                    let history = route_history.clone();
                    async move {
                        history.lock().unwrap().push(params.payload.clone());
                        params.payload
                    }
                }),
            );
        let server = tokio::spawn(
            async move { axum::serve(listener, routes).await.unwrap() }.in_current_span(),
        );
        let component = executor
            .component_dep(&context.default_environment_id, host_api_tests)
            .store()
            .await?;
        let agent_id = agent_id!("CustomDurability", format!("cancel-result-{fallible}"));
        let worker_id = executor
            .start_agent_with(
                &component.id,
                agent_id.clone(),
                HashMap::from([("PORT".to_string(), port.to_string())]),
                Vec::new(),
            )
            .await?;
        executor
            .invoke_and_await_agent(&component, &agent_id, "warmup_environment", data_value!())
            .await?;
        let mut events = executor.capture_output(&worker_id).await?;
        let (accepted, release) = executor.gate_next_oplog_function_end(
            &worker_id,
            golem_common::model::oplog::host_functions::HostFunctionName::Custom(
                "golem-it::cancel-result".to_string(),
            ),
            false,
        );
        let invocation = tokio::spawn({
            let executor = executor.clone();
            let component = component.clone();
            let agent_id = agent_id.clone();
            async move {
                executor
                    .invoke_and_await_agent(
                        &component,
                        &agent_id,
                        "cancel_pending_result",
                        data_value!(fallible),
                    )
                    .await
            }
            .in_current_span()
        });
        tokio::time::timeout(Duration::from_secs(20), accepted)
            .await
            .context("waiting for custom End acceptance")??;
        // End is accepted, but finish cannot deliver before its receipt is released.
        cancel.add_permits(1);
        // Logging does not drain oplog appends. The destructor's HTTP call does, so its
        // effect must wait until the accepted finish receipt is released below.
        tokio::time::timeout(Duration::from_secs(20), async {
            while let Some(event) = events.recv().await {
                if matches!(AgentEvent::try_from(event), Ok(AgentEvent::Log { message, .. })
                    if message.ends_with("dropping undelivered custom result"))
                {
                    return;
                }
            }
            panic!("event stream ended before result destruction");
        })
        .await
        .context("waiting for undelivered result destruction")?;
        release.send(()).unwrap();
        invocation.await??;
        let entries = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
        let custom_start = entries
            .iter()
            .find_map(|entry| match &entry.entry {
                PublicOplogEntry::Start(params)
                    if params.function_name == "golem-it::cancel-result" =>
                {
                    Some(entry.oplog_index)
                }
                _ => None,
            })
            .expect("custom operation began");
        assert_eq!(entries.iter().filter(|entry| matches!(&entry.entry,
            PublicOplogEntry::Start(params) if params.function_name == "http::client::send"
                && params.observational_owner == Some(custom_start) && params.invocation_id.is_none()
        )).count(), 1, "undelivered result destructor retains its custom scope");
        assert_eq!(
            entries
                .iter()
                .filter(|entry| matches!(&entry.entry,
                    PublicOplogEntry::End(params) if params.start_index == custom_start
                ))
                .count(),
            1
        );
        assert!(entries.iter().any(|entry| matches!(&entry.entry,
            PublicOplogEntry::CompletionDiscarded(params) if params.start_index == custom_start
        )));
        executor.simulated_crash(&worker_id).await?;
        assert_eq!(
            executor
                .invoke_and_await_agent(&component, &agent_id, "callback", data_value!("fresh"))
                .await?
                .into_typed::<String>()?,
            "fresh"
        );
        assert_eq!(
            &*history.lock().unwrap(),
            &["result-drop", "after-cancel", "fresh"],
            "reconstruction neither repeats the destructor effect nor loses the later root call"
        );
        server.abort();
    }
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn custom_durability_recovers_with_one_completed_and_one_suspended_sibling(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;
    let history = Arc::new(Mutex::new(Vec::<String>::new()));
    let second_arrived = Arc::new(tokio::sync::Notify::new());
    let checkpoint = Arc::new(tokio::sync::Notify::new());
    let resume = Arc::new(tokio::sync::Semaphore::new(0));
    let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await?;
    let port = listener.local_addr()?.port();

    #[derive(Deserialize)]
    struct Params {
        payload: String,
    }
    let route_history = history.clone();
    let callback_resume = resume.clone();
    let release_resume = resume.clone();
    let route_checkpoint = checkpoint.clone();
    let route = Router::new()
        .route(
            "/callback",
            any(move |Query(params): Query<Params>| {
                let history = route_history.clone();
                let arrived = second_arrived.clone();
                let resume = callback_resume.clone();
                async move {
                    history.lock().unwrap().push(params.payload.clone());
                    match params.payload.as_str() {
                        "first" => arrived.notified().await,
                        "second" => {
                            arrived.notify_one();
                            let _ = resume.acquire().await;
                        }
                        _ => {}
                    }
                    params.payload
                }
            }),
        )
        .route(
            "/release",
            any(move || {
                let checkpoint = route_checkpoint.clone();
                let resume = release_resume.clone();
                async move {
                    // The guest sends this only after finishing the first custom operation.
                    checkpoint.notify_one();
                    let _ = resume.acquire().await;
                    "released"
                }
            }),
        );
    let server =
        tokio::spawn(async move { axum::serve(listener, route).await.unwrap() }.in_current_span());
    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;
    let agent_id = agent_id!("CustomDurability", "partial-concurrent-recovery");
    let worker_id = executor
        .start_agent_with(
            &component.id,
            agent_id.clone(),
            HashMap::from([("PORT".to_string(), port.to_string())]),
            Vec::new(),
        )
        .await?;
    executor
        .invoke_and_await_agent(&component, &agent_id, "warmup_environment", data_value!())
        .await?;
    let invocation = tokio::spawn({
        let executor = executor.clone();
        let component = component.clone();
        let agent_id = agent_id.clone();
        async move {
            executor
                .invoke_and_await_agent(
                    &component,
                    &agent_id,
                    "concurrent_first_finishes",
                    data_value!(),
                )
                .await
        }
        .in_current_span()
    });
    tokio::time::timeout(Duration::from_secs(20), checkpoint.notified()).await?;
    executor.commit_oplog(&worker_id).await?;
    let before = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    let anchor = before
        .iter()
        .find_map(|entry| match &entry.entry {
            PublicOplogEntry::AgentInvocationStarted(params)
                if matches!(&params.invocation, PublicAgentInvocation::AgentMethodInvocation(call)
                if call.method_name == "concurrent_first_finishes") =>
            {
                Some(entry.oplog_index)
            }
            _ => None,
        })
        .expect("concurrent invocation anchor");
    let roots = |entries: &[PublicOplogEntryWithIndex]| {
        let mut roots = entries
            .iter()
            .filter_map(|entry| match &entry.entry {
                PublicOplogEntry::Start(params)
                    if params.function_name == "golem-it::test-generator"
                        && params.invocation_id.is_some_and(|id| id.anchor == anchor) =>
                {
                    assert_eq!(params.parent_start_index, None);
                    Some((entry.oplog_index, params.invocation_id.unwrap()))
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        roots.sort_by_key(|(_, id)| id.ordinal);
        roots
    };
    let siblings = roots(&before);
    assert_eq!(siblings.len(), 2);
    assert_eq!(siblings[0].1.ordinal, 0);
    assert_eq!(siblings[1].1.ordinal, 1);
    let ends = |entries: &[PublicOplogEntryWithIndex], start| {
        entries
            .iter()
            .filter(|entry| {
                matches!(&entry.entry,
                    PublicOplogEntry::End(params) if params.start_index == start
                )
            })
            .count()
    };
    assert_eq!(ends(&before, siblings[0].0), 1);
    assert_eq!(ends(&before, siblings[1].0), 0);

    let release_send = before
        .iter()
        .find_map(|entry| match &entry.entry {
            PublicOplogEntry::Start(params)
                if params.function_name == "http::client::send"
                    && params.invocation_id.is_some_and(|id| id.anchor == anchor) =>
            {
                Some((entry.oplog_index, params))
            }
            _ => None,
        })
        .expect("release request has started");
    assert_eq!(release_send.1.observational_owner, None);
    assert_eq!(release_send.1.invocation_id.unwrap().ordinal, 2);
    let release_scope = release_send
        .1
        .parent_start_index
        .expect("POST send belongs to a batched-write scope");
    assert!(before.iter().any(|entry| {
        entry.oplog_index == release_scope
            && matches!(&entry.entry, PublicOplogEntry::Start(params)
                if params.function_name == "<scope:batched-write>"
                    && params.parent_start_index.is_none()
                    && params.observational_owner.is_none()
                    && params.invocation_id == release_send.1.invocation_id)
    }));

    executor.simulated_crash(&worker_id).await?;
    // Closing the gate also releases requests from the discarded Store, if still connected.
    resume.close();
    assert_eq!(
        invocation.await??.into_typed::<Vec<String>>()?,
        vec!["first", "second"]
    );
    let effects = history.lock().unwrap().clone();
    assert_eq!(effects.iter().filter(|p| *p == "first").count(), 1);
    assert_eq!(effects.iter().filter(|p| *p == "second").count(), 2);
    let recovered = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    assert_eq!(
        roots(&recovered),
        siblings,
        "recovery retains both logical identities"
    );
    for (start, _) in &siblings {
        assert_eq!(ends(&recovered, *start), 1);
        assert!(recovered.iter().any(|entry| matches!(&entry.entry,
            PublicOplogEntry::Start(params) if params.function_name == "http::client::send"
                && params.observational_owner == Some(*start) && params.invocation_id.is_none()
        )));
    }
    let jumps: Vec<_> = recovered
        .iter()
        .filter_map(|entry| match &entry.entry {
            PublicOplogEntry::Jump(params) => Some(&params.jump),
            _ => None,
        })
        .collect();
    assert!(jumps.iter().any(|jump| jump.contains(release_send.0)));
    assert!(!jumps.iter().any(|jump| jump.contains(release_scope)));
    let root_sends: Vec<_> = recovered
        .iter()
        .filter(|entry| !jumps.iter().any(|jump| jump.contains(entry.oplog_index)))
        .filter_map(|entry| match &entry.entry {
            PublicOplogEntry::Start(params)
                if params.function_name == "http::client::send"
                    && params.invocation_id.is_some_and(|id| id.anchor == anchor) =>
            {
                Some((entry.oplog_index, params))
            }
            _ => None,
        })
        .collect();
    assert_eq!(
        root_sends.len(),
        1,
        "exactly one release request remains in effective history"
    );
    assert_ne!(root_sends[0].0, release_send.0);
    assert_eq!(root_sends[0].1.observational_owner, None);
    assert_eq!(root_sends[0].1.parent_start_index, Some(release_scope));
    assert_eq!(root_sends[0].1.invocation_id, release_send.1.invocation_id);
    assert_eq!(ends(&recovered, root_sends[0].0), 1);
    server.abort();
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn custom_durability_scopes_follow_poll_and_drop_execution(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;
    let history = Arc::new(Mutex::new(Vec::<String>::new()));
    let second_arrived = Arc::new(tokio::sync::Notify::new());
    let release_second = Arc::new(tokio::sync::Notify::new());
    let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await?;
    let port = listener.local_addr()?.port();

    #[derive(Deserialize)]
    struct Params {
        payload: String,
    }
    let route_history = history.clone();
    let release_history = history.clone();
    let release = release_second.clone();
    let route = Router::new()
        .route(
            "/callback",
            any(move |Query(params): Query<Params>| {
                let history = route_history.clone();
                let arrived = second_arrived.clone();
                let release = release_second.clone();
                async move {
                    history.lock().unwrap().push(params.payload.clone());
                    match params.payload.as_str() {
                        "first" => arrived.notified().await,
                        "second" => {
                            arrived.notify_one();
                            release.notified().await;
                        }
                        _ => {}
                    }
                    params.payload
                }
            }),
        )
        .route(
            "/release",
            any(move || {
                let release = release.clone();
                let history = release_history.clone();
                async move {
                    history.lock().unwrap().push("release".to_string());
                    release.notify_one();
                    "released"
                }
            }),
        );
    let server =
        tokio::spawn(async move { axum::serve(listener, route).await.unwrap() }.in_current_span());
    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;
    let agent_id = agent_id!("CustomDurability", "poll-and-drop-scopes");
    let worker_id = executor
        .start_agent_with(
            &component.id,
            agent_id.clone(),
            HashMap::from([("PORT".to_string(), port.to_string())]),
            Vec::new(),
        )
        .await?;
    // The standard library's first environment read is a root durable call, too.
    executor
        .invoke_and_await_agent(&component, &agent_id, "warmup_environment", data_value!())
        .await?;
    for (method, expected) in [
        ("concurrent_first_finishes", vec!["first", "second"]),
        ("concurrent_nested", vec!["nested", "nested"]),
    ] {
        let result = executor
            .invoke_and_await_agent(&component, &agent_id, method, data_value!())
            .await?
            .into_typed::<Vec<String>>()?;
        assert_eq!(result, expected);
    }
    let before = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    let starts_before: Vec<_> = before
        .iter()
        .filter_map(|entry| match &entry.entry {
            PublicOplogEntry::Start(params) if params.function_name.starts_with("golem-it::") => {
                Some(entry.oplog_index)
            }
            _ => None,
        })
        .collect();
    executor.simulated_crash(&worker_id).await?;
    let after = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "callback",
            data_value!("after-restart"),
        )
        .await?
        .into_typed::<String>()?;
    assert_eq!(after, "after-restart");
    assert_eq!(
        history.lock().unwrap().len(),
        6,
        "completed replay performs no HTTP effects"
    );
    let recovered = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    let recovered_starts: Vec<_> = recovered
        .iter()
        .filter_map(|entry| match &entry.entry {
            PublicOplogEntry::Start(params)
                if params.function_name.starts_with("golem-it::")
                    && params.function_name != "golem-it::test-callback" =>
            {
                Some(entry.oplog_index)
            }
            _ => None,
        })
        .collect();
    assert_eq!(
        recovered_starts, starts_before,
        "completed replay appends no custom Starts"
    );

    let lifecycle = executor
        .invoke_and_await_agent(&component, &agent_id, "scope_lifecycle", data_value!())
        .await?
        .into_typed::<Vec<String>>()?;
    assert_eq!(
        lifecycle,
        vec!["prepared", "sibling", "ready-drop", "fresh"]
    );
    let mut effects = history.lock().unwrap().clone();
    effects.sort();
    assert_eq!(
        effects,
        [
            "after-restart",
            "cancel-drop",
            "first",
            "fresh",
            "nested",
            "nested",
            "ready-drop",
            "release",
            "second",
            "sibling"
        ]
    );
    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    let anchor_for = |method: &str| {
        oplog
            .iter()
            .find_map(|entry| match &entry.entry {
                PublicOplogEntry::AgentInvocationStarted(params)
                    if matches!(&params.invocation,
            PublicAgentInvocation::AgentMethodInvocation(call) if call.method_name == method) =>
                {
                    Some(entry.oplog_index)
                }
                _ => None,
            })
            .expect("agent invocation anchor")
    };
    let roots_for = |anchor| {
        let mut roots = oplog
            .iter()
            .filter_map(|entry| match &entry.entry {
                PublicOplogEntry::Start(params)
                    if params.function_name.starts_with("golem-it::")
                        && params.invocation_id.is_some_and(|id| id.anchor == anchor) =>
                {
                    Some((entry.oplog_index, params))
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        roots.sort_by_key(|(_, params)| params.invocation_id.unwrap().ordinal);
        roots
    };
    let adverse = roots_for(anchor_for("concurrent_first_finishes"));
    let nested = roots_for(anchor_for("concurrent_nested"));
    for roots in [&adverse, &nested] {
        assert_eq!(roots.len(), 2);
        for (ordinal, (_, params)) in roots.iter().enumerate() {
            assert_eq!(params.parent_start_index, None);
            assert_eq!(params.invocation_id.unwrap().ordinal, ordinal as u64);
        }
    }
    let end_of = |start| {
        oplog.iter().find_map(|entry| match &entry.entry {
            PublicOplogEntry::End(params) if params.start_index == start => Some(entry.oplog_index),
            _ => None,
        })
    };
    assert!(
        end_of(adverse[0].0).unwrap() < end_of(adverse[1].0).unwrap(),
        "first-begun operation finishes while second is still held"
    );
    for (start, _) in &adverse {
        for name in ["http::client::send", "http::types::response::consume-body"] {
            assert_eq!(oplog.iter().filter(|entry| matches!(&entry.entry,
                PublicOplogEntry::Start(params) if params.function_name == name && params.observational_owner == Some(*start) && params.invocation_id.is_none()
            )).count(), 1);
        }
    }
    let release_sends: Vec<_> = oplog
        .iter()
        .filter_map(|entry| match &entry.entry {
            PublicOplogEntry::Start(params)
                if params.function_name == "http::client::send"
                    && params
                        .invocation_id
                        .is_some_and(|id| id.anchor == anchor_for("concurrent_first_finishes")) =>
            {
                Some((entry.oplog_index, params))
            }
            _ => None,
        })
        .collect();
    assert_eq!(release_sends.len(), 1);
    assert_eq!(release_sends[0].1.observational_owner, None);
    assert!(end_of(adverse[0].0).unwrap() < release_sends[0].0);
    assert!(release_sends[0].0 < end_of(adverse[1].0).unwrap());
    for (root, _) in &nested {
        let children: Vec<_> = oplog
            .iter()
            .filter_map(|entry| match &entry.entry {
                PublicOplogEntry::Start(params)
                    if params.function_name == "golem-it::test-generator"
                        && params.parent_start_index == Some(*root) =>
                {
                    Some((entry.oplog_index, params))
                }
                _ => None,
            })
            .collect();
        assert_eq!(children.len(), 1);
        assert_eq!(children[0].1.invocation_id.unwrap().anchor, *root);
        assert_eq!(children[0].1.invocation_id.unwrap().ordinal, 0);
        for name in ["http::client::send", "http::types::response::consume-body"] {
            assert_eq!(oplog.iter().filter(|entry| matches!(&entry.entry,
                PublicOplogEntry::Start(params) if params.function_name == name && params.observational_owner == Some(children[0].0) && params.invocation_id.is_none()
            )).count(), 1);
        }
    }
    let lifecycle_roots = roots_for(anchor_for("scope_lifecycle"));
    assert_eq!(
        lifecycle_roots
            .iter()
            .map(|(_, params)| params.function_name.as_str())
            .collect::<Vec<_>>(),
        [
            "golem-it::prepared-parent",
            "golem-it::abandoned",
            "golem-it::drop-sibling",
            "golem-it::completed-body",
            "golem-it::test-generator"
        ]
    );
    for (ordinal, (_, params)) in lifecycle_roots.iter().enumerate() {
        assert_eq!(params.parent_start_index, None);
        assert_eq!(params.invocation_id.unwrap().ordinal, ordinal as u64);
    }
    for (name, parent, finished) in [
        ("prepared", lifecycle_roots[0].0, true),
        ("abandoned-child", lifecycle_roots[1].0, false),
        ("sync-child", lifecycle_roots[2].0, true),
    ] {
        let child = oplog
            .iter()
            .find_map(|entry| match &entry.entry {
                PublicOplogEntry::Start(params)
                    if params.function_name == format!("golem-it::{name}") =>
                {
                    Some((entry.oplog_index, params))
                }
                _ => None,
            })
            .unwrap();
        assert_eq!(child.1.parent_start_index, Some(parent));
        assert_eq!(child.1.invocation_id.unwrap().anchor, parent);
        assert_eq!(child.1.invocation_id.unwrap().ordinal, 0);
        assert_eq!(end_of(child.0).is_some(), finished);
        if name == "abandoned-child" {
            assert_eq!(oplog.iter().filter(|entry| matches!(&entry.entry,
                PublicOplogEntry::Start(params) if params.function_name == "http::client::send" && params.observational_owner == Some(child.0)
            )).count(), 1, "cancelled body destructor executes in the child's scope");
        }
    }
    assert!(
        end_of(lifecycle_roots[1].0).is_none(),
        "unfinished drop writes no End"
    );
    assert!(!oplog.iter().any(|entry| matches!(&entry.entry,
        PublicOplogEntry::Start(params) if params.function_name == "golem-it::unpolled"
    )));
    let completed = lifecycle_roots[3].0;
    let destructor_effect = oplog
        .iter()
        .find_map(|entry| match &entry.entry {
            PublicOplogEntry::Start(params)
                if params.function_name == "http::client::send"
                    && params.observational_owner == Some(completed) =>
            {
                Some(entry.oplog_index)
            }
            _ => None,
        })
        .expect("completed body destructor retains its custom scope");
    assert!(destructor_effect < end_of(completed).unwrap());
    for mode in ["empty", "drop"] {
        executor
            .invoke_and_await_agent(&component, &agent_id, "context_probe", data_value!(mode))
            .await?;
    }
    let probes = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    let empty_start = probes
        .iter()
        .find_map(|entry| match &entry.entry {
            PublicOplogEntry::Start(params) if params.function_name == "context-probe-empty" => {
                Some((entry.oplog_index, params.invocation_id.unwrap().anchor))
            }
            _ => None,
        })
        .unwrap();
    let empty_effects: Vec<_> = probes
        .iter()
        .filter_map(|entry| match &entry.entry {
            PublicOplogEntry::Start(params)
                if params.function_name == "http::client::send"
                    && (params
                        .invocation_id
                        .is_some_and(|id| id.anchor == empty_start.1)
                        || params.observational_owner == Some(empty_start.0)) =>
            {
                Some(params)
            }
            _ => None,
        })
        .collect();
    assert_eq!(empty_effects.len(), 2);
    assert_eq!(
        empty_effects[0].observational_owner, None,
        "installing empty clears a nonempty context"
    );
    assert_eq!(
        empty_effects[1].observational_owner,
        Some(empty_start.0),
        "reinstalling the returned snapshot restores attribution"
    );
    let probe_start = probes
        .iter()
        .find_map(|entry| match &entry.entry {
            PublicOplogEntry::Start(params) if params.function_name == "context-probe" => {
                Some(entry.oplog_index)
            }
            _ => None,
        })
        .unwrap();
    assert!(
        probes.iter().any(|entry| matches!(&entry.entry,
            PublicOplogEntry::End(params) if params.start_index == probe_start
        )),
        "dropping a snapshot does not close the live invocation"
    );
    assert!(probes.iter().any(|entry| matches!(&entry.entry,
        PublicOplogEntry::Start(params) if params.function_name == "http::client::send" && params.observational_owner == Some(probe_start)
    )));
    for (name, mode, message) in [
        ("stale-context", "stale", "is no longer active"),
        ("stale-ancestor", "ancestor", "is no longer active"),
        (
            "cross-invocation-context",
            "cross",
            "belongs to another invocation",
        ),
    ] {
        let probe_agent = agent_id!("CustomDurability", name);
        executor
            .start_agent_with(
                &component.id,
                probe_agent.clone(),
                HashMap::new(),
                Vec::new(),
            )
            .await?;
        if mode == "cross" {
            executor
                .invoke_and_await_agent(
                    &component,
                    &probe_agent,
                    "context_probe",
                    data_value!("save"),
                )
                .await?;
        }
        let error = executor
            .invoke_and_await_agent(&component, &probe_agent, "context_probe", data_value!(mode))
            .await
            .expect_err("invalid snapshot installation must trap");
        assert!(format!("{error:?}").contains(message), "{error:?}");
    }
    server.abort();
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn custom_durability_crash_mid_live_invocation_reexecutes_whole_body(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;
    let history = Arc::new(Mutex::new(Vec::<String>::new()));
    let hold_first_callback = Arc::new(AtomicBool::new(true));
    let callback_arrived = Arc::new(tokio::sync::Notify::new());
    let release_callback = Arc::new(tokio::sync::Notify::new());
    let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await?;
    let port = listener.local_addr()?.port();

    #[derive(Deserialize)]
    struct Params {
        payload: String,
    }

    let route_history = history.clone();
    let route_arrived = callback_arrived.clone();
    let route_release = release_callback.clone();
    let route_hold = hold_first_callback.clone();
    let server = tokio::spawn(
        async move {
            let probe_history = route_history.clone();
            let route = Router::new()
                .route(
                    "/probe",
                    get(move |Query(params): Query<Params>| {
                        probe_history
                            .lock()
                            .unwrap()
                            .push(format!("probe:{}", params.payload));
                        async { "probe" }
                    }),
                )
                .route(
                    "/callback",
                    get(move |Query(params): Query<Params>| {
                        let history = route_history.clone();
                        let arrived = route_arrived.clone();
                        let release = route_release.clone();
                        let hold = route_hold.clone();
                        async move {
                            history
                                .lock()
                                .unwrap()
                                .push(format!("callback:{}", params.payload));
                            if hold.swap(false, Ordering::SeqCst) {
                                arrived.notify_one();
                                release.notified().await;
                            }
                            format!("done-{}", params.payload)
                        }
                    }),
                );
            axum::serve(listener, route).await.unwrap();
        }
        .in_current_span(),
    );

    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;
    let agent_id = agent_id!("CustomDurability", "custom-durability-mid-block");
    let mut env = HashMap::new();
    env.insert("PORT".to_string(), port.to_string());
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), env, Vec::new())
        .await?;
    let invocation = tokio::spawn(
        {
            let executor = executor.clone();
            let component = component.clone();
            let agent_id = agent_id.clone();
            async move {
                executor
                    .invoke_and_await_agent(
                        &component,
                        &agent_id,
                        "callback_after_probe",
                        data_value!("x"),
                    )
                    .await
            }
        }
        .in_current_span(),
    );
    tokio::time::timeout(Duration::from_secs(20), callback_arrived.notified()).await?;
    executor.simulated_crash(&worker_id).await?;
    release_callback.notify_waiters();
    let result = invocation.await??;
    assert_eq!(result.into_typed::<String>()?, "done-x");
    assert_eq!(
        &*history.lock().unwrap(),
        &["probe:x", "callback:x", "probe:x", "callback:x"]
    );

    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    let logical_starts: Vec<_> = oplog
        .iter()
        .filter(|entry| {
            matches!(&entry.entry, PublicOplogEntry::Start(params) if params.function_name == "golem-it::test-callback")
        })
        .collect();
    assert_eq!(
        logical_starts.len(),
        1,
        "whole-block recovery keeps one logical custom invocation"
    );
    let logical_start = logical_starts[0].oplog_index;
    assert_eq!(
        oplog
            .iter()
            .filter(|entry| {
                matches!(&entry.entry, PublicOplogEntry::End(params) if params.start_index == logical_start)
            })
            .count(),
        1,
        "the recovered custom invocation has one logical terminal"
    );
    let observational_starts: Vec<_> = oplog
        .iter()
        .filter(|entry| {
            matches!(
                &entry.entry,
                PublicOplogEntry::Start(params)
                    if params.observational_owner == Some(logical_start)
            )
        })
        .collect();
    assert!(
        observational_starts.len() >= 2,
        "whole-block recovery keeps physical host calls observable as ordinary calls"
    );

    let before_replay = history.lock().unwrap().len();
    drop(executor);
    let executor = start(deps, &context).await?;
    executor.check_oplog_is_queryable(&worker_id).await?;
    assert_eq!(
        history.lock().unwrap().len(),
        before_replay,
        "completed replay touches neither endpoint"
    );
    server.abort();
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn immediate_recovery_does_not_duplicate_instantiation_memory_growth(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_counters")] agent_counters: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;
    let component = executor
        .component_dep(&context.default_environment_id, agent_counters)
        .store()
        .await?;
    let agent_id = agent_id!("InstantiationGrowthCounter", "memory-growth-recovery");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;
    let owned_agent_id = OwnedAgentId::new(context.default_environment_id, &worker_id);
    let initial_bytes = executor.worker_memory_requirement(&owned_agent_id).await?;

    executor
        .invoke_and_await_agent(&component, &agent_id, "increment", data_value!())
        .await?;
    let before = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    let growth_before = total_memory_growth(&before);
    assert!(
        growth_before > 0,
        "fixture must produce instantiation-time memory growth"
    );
    let canonical_bytes = executor.worker_memory_requirement(&owned_agent_id).await?;
    assert!(
        canonical_bytes > initial_bytes,
        "fixture must grow linear memory after instantiation: initial={initial_bytes}, canonical={canonical_bytes}"
    );

    let executor_clone = executor.clone();
    let component_clone = component.clone();
    let agent_id_clone = agent_id.clone();
    let invocation = tokio::spawn(
        async move {
            executor_clone
                .invoke_and_await_agent(
                    &component_clone,
                    &agent_id_clone,
                    "delayed_increment",
                    data_value!(5_000u64),
                )
                .await
        }
        .in_current_span(),
    );

    executor
        .wait_for_status(&worker_id, AgentStatus::Running, Duration::from_secs(10))
        .await?;
    executor.simulated_crash(&worker_id).await?;

    let recovered_result = invocation.await??;
    assert_eq!(
        recovered_result.into_typed::<u32>()?,
        2,
        "the interrupted invocation must complete after recovery without duplicating state"
    );
    let post_recovery_result = executor
        .invoke_and_await_agent(&component, &agent_id, "increment", data_value!())
        .await?;
    assert_eq!(
        post_recovery_result.into_typed::<u32>()?,
        3,
        "the recovered worker must accept subsequent invocations"
    );

    let after = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    let growth_after = total_memory_growth(&after);
    assert_eq!(
        growth_after, growth_before,
        "immediate recovery must not persist instantiation growth again"
    );
    assert_eq!(
        executor.worker_memory_requirement(&owned_agent_id).await?,
        canonical_bytes,
        "re-instantiation must preserve the canonical memory requirement"
    );

    Ok(())
}

const SNAPSHOT_TEST_INVOCATIONS: usize = 10;

#[test]
#[tracing::instrument]
async fn automatic_snapshot_disabled(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_counters")] agent_counters: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start_with_snapshot_policy(deps, &context, SnapshotPolicy::Disabled).await?;

    let component = executor
        .component_dep(&context.default_environment_id, agent_counters)
        .store()
        .await?;
    let agent_id = agent_id!("JsonSnapshotCounter", "disabled");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    for _ in 0..SNAPSHOT_TEST_INVOCATIONS {
        executor
            .invoke_and_await_agent(&component, &agent_id, "increment", data_value!())
            .await?;
    }

    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    let snapshot_count = oplog
        .iter()
        .filter(|entry| matches!(&entry.entry, PublicOplogEntry::Snapshot(_)))
        .count();

    drop(executor);

    assert_eq!(
        snapshot_count, 0,
        "Expected no snapshots with disabled policy"
    );
    Ok(())
}

#[test]
#[tracing::instrument]
async fn automatic_snapshot_every_2nd_invocation(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_counters")] agent_counters: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start_with_snapshot_policy(
        deps,
        &context,
        SnapshotPolicy::EveryNInvocation { count: 2 },
    )
    .await?;

    let component = executor
        .component_dep(&context.default_environment_id, agent_counters)
        .store()
        .await?;
    let agent_id = agent_id!("JsonSnapshotCounter", "every-2nd");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    // Construction counts as an invocation; align subsequent snapshots with even increments.
    let initial = executor
        .invoke_and_await_agent(&component, &agent_id, "get", data_value!())
        .await?;
    assert_eq!(initial.into_typed::<u32>()?, 0);

    for _ in 0..SNAPSHOT_TEST_INVOCATIONS {
        executor
            .invoke_and_await_agent(&component, &agent_id, "increment", data_value!())
            .await?;
    }

    let expected_snapshot_count = 1 + SNAPSHOT_TEST_INVOCATIONS / 2;
    let oplog = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
            let snapshot_count = oplog
                .iter()
                .filter(|entry| matches!(&entry.entry, PublicOplogEntry::Snapshot(_)))
                .count();
            if snapshot_count >= expected_snapshot_count {
                return Ok::<_, anyhow::Error>(oplog);
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .map_err(|_| anyhow::anyhow!("automatic snapshots were not persisted in time"))??;
    assert!(
        oplog
            .iter()
            .take_while(|entry| !matches!(&entry.entry, PublicOplogEntry::Snapshot(_)))
            .any(|entry| matches!(
                &entry.entry,
                PublicOplogEntry::Start(params) if params.function_name == "monotonic_clock::now"
            )),
        "core initializer must record a clock call before the snapshot"
    );
    let snapshot_count = oplog
        .iter()
        .filter(|entry| matches!(&entry.entry, PublicOplogEntry::Snapshot(_)))
        .count();

    assert_eq!(
        snapshot_count, expected_snapshot_count,
        "Expected a snapshot every 2 invocations"
    );

    let tail = executor
        .invoke_and_await_agent(&component, &agent_id, "increment", data_value!())
        .await?;
    assert_eq!(tail.into_typed::<u32>()?, 11);
    let oplog_with_tail = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    assert_eq!(
        oplog_with_tail
            .iter()
            .rposition(|entry| matches!(&entry.entry, PublicOplogEntry::Snapshot(_))),
        oplog
            .iter()
            .rposition(|entry| matches!(&entry.entry, PublicOplogEntry::Snapshot(_))),
        "The final increment must remain outside the last snapshot"
    );

    drop(executor);
    let executor = start_with_snapshot_policy(
        deps,
        &context,
        SnapshotPolicy::EveryNInvocation { count: 2 },
    )
    .await?;
    let mut events = executor.capture_output(&worker_id).await?;

    let result_after_restart = executor
        .invoke_and_await_agent(&component, &agent_id, "get", data_value!())
        .await?;
    assert_snapshot_recovery_loaded(&mut events).await;

    assert_eq!(
        result_after_restart.into_typed::<u32>()?,
        11,
        "Counter should include the increment replayed after the automatic snapshot"
    );

    drop(executor);
    Ok(())
}

#[test]
#[tracing::instrument]
async fn automatic_snapshot_periodic(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_counters")] agent_counters: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start_with_snapshot_policy(
        deps,
        &context,
        SnapshotPolicy::Periodic {
            period: Duration::from_secs(2),
        },
    )
    .await?;

    let component = executor
        .component_dep(&context.default_environment_id, agent_counters)
        .store()
        .await?;
    let agent_id = agent_id!("JsonSnapshotCounter", "periodic");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    for _ in 0..SNAPSHOT_TEST_INVOCATIONS {
        executor
            .invoke_and_await_agent(&component, &agent_id, "increment", data_value!())
            .await?;
        tokio::time::sleep(Duration::from_millis(500)).await;
    }

    tokio::time::sleep(Duration::from_secs(3)).await;

    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    let snapshot_count = oplog
        .iter()
        .filter(|entry| matches!(&entry.entry, PublicOplogEntry::Snapshot(_)))
        .count();

    assert!(
        snapshot_count >= 1,
        "Expected at least 1 snapshot with periodic policy (every 2s over ~5s of invocations), got {snapshot_count}"
    );
    assert!(
        snapshot_count <= SNAPSHOT_TEST_INVOCATIONS,
        "Expected at most {SNAPSHOT_TEST_INVOCATIONS} snapshots, got {snapshot_count}"
    );

    drop(executor);
    let executor = start_with_snapshot_policy(
        deps,
        &context,
        SnapshotPolicy::Periodic {
            period: Duration::from_secs(2),
        },
    )
    .await?;
    let mut events = executor.capture_output(&worker_id).await?;

    let result_after_restart = executor
        .invoke_and_await_agent(&component, &agent_id, "get", data_value!())
        .await?;
    assert_snapshot_recovery_loaded(&mut events).await;

    assert_eq!(
        result_after_restart.into_typed::<u32>()?,
        SNAPSHOT_TEST_INVOCATIONS as u32,
        "Counter should be restored from the automatic snapshot after restart"
    );

    drop(executor);
    Ok(())
}

#[test]
#[tracing::instrument]
async fn snapshot_based_recovery(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_counters")] agent_counters: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start_with_snapshot_policy(
        deps,
        &context,
        SnapshotPolicy::EveryNInvocation { count: 3 },
    )
    .await?;

    let component = executor
        .component_dep(&context.default_environment_id, agent_counters)
        .store()
        .await?;
    let agent_id = agent_id!("SnapshotCounter", "recovery");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    for _ in 0..105 {
        executor
            .invoke_and_await_agent(&component, &agent_id, "increment", data_value!())
            .await?;
    }

    let result_before = executor
        .invoke_and_await_agent(&component, &agent_id, "get", data_value!())
        .await?;

    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    let snapshot_count = oplog
        .iter()
        .filter(|entry| matches!(&entry.entry, PublicOplogEntry::Snapshot(_)))
        .count();
    assert!(
        snapshot_count >= 1,
        "Expected at least one snapshot before restart, got {snapshot_count}"
    );

    drop(executor);
    let executor = start_with_snapshot_policy(
        deps,
        &context,
        SnapshotPolicy::EveryNInvocation { count: 3 },
    )
    .await?;
    let mut events = executor.capture_output(&worker_id).await?;

    let result_after = executor
        .invoke_and_await_agent(&component, &agent_id, "get", data_value!())
        .await?;
    assert_snapshot_recovery_loaded(&mut events).await;

    assert_eq!(
        result_before, result_after,
        "Worker state should be preserved across restart via snapshot recovery"
    );

    let increment_after = executor
        .invoke_and_await_agent(&component, &agent_id, "increment", data_value!())
        .await?;

    assert_eq!(
        increment_after.into_typed::<u32>()?,
        106,
        "Counter should continue from 106 after snapshot recovery"
    );

    drop(executor);
    Ok(())
}

#[test]
#[timeout("120s")]
#[tracing::instrument]
async fn snapshot_load_restores_without_initialization_and_replays_only_the_suffix(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_sdk_rust")] agent_sdk_rust: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let config_marker = format!("snapshot-config-{}", context.redis_prefix());
    let executor = start_with_snapshot_policy(
        deps,
        &context,
        SnapshotPolicy::EveryNInvocation { count: 4 },
    )
    .await?;

    let component = executor
        .component_dep(&context.default_environment_id, agent_sdk_rust)
        .with_agent_config(
            "SnapshotLoadProbe",
            vec![AgentConfigEntryDto {
                path: vec!["marker".to_string()],
                value: serde_json::Value::String(config_marker.clone()).into(),
            }],
        )
        .store()
        .await?;
    let phantom_id = uuid::Uuid::new_v4();
    let agent_id = phantom_agent_id!("SnapshotLoadProbe", phantom_id, "read");
    let worker_id = AgentId {
        component_id: component.id,
        agent_id: agent_id.to_string(),
    };
    let principal = Principal::GolemUser(GolemUserPrincipal {
        account_id: AccountId::new(),
    });
    let principal_account_id = match &principal {
        Principal::GolemUser(principal) => principal.account_id.to_string(),
        _ => unreachable!(),
    };

    for expected in 1..=4 {
        let result = executor
            .invoke_and_await_agent_as_principal(
                &component,
                &agent_id,
                principal.clone(),
                "increment",
                data_value!(),
            )
            .await?;
        assert_eq!(result.into_typed::<u64>()?, expected);
    }

    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    let latest_snapshot = oplog
        .iter()
        .filter_map(|entry| match &entry.entry {
            PublicOplogEntry::Snapshot(params) => Some(&params.data),
            _ => None,
        })
        .next_back()
        .expect("the third invocation should create a snapshot");
    let PublicSnapshotData::Raw(snapshot) = latest_snapshot else {
        panic!("custom Rust snapshots should use the binary envelope");
    };
    assert_eq!(snapshot.mime_type, "application/octet-stream");
    assert_eq!(
        snapshot.data.first(),
        Some(&2),
        "expected binary envelope v2"
    );
    let principal_length = u32::from_be_bytes(
        snapshot.data[1..5]
            .try_into()
            .expect("binary snapshot should contain the principal length"),
    ) as usize;
    let state_offset = 5 + principal_length;
    assert!(
        String::from_utf8_lossy(&snapshot.data[5..state_offset]).contains(&principal_account_id),
        "snapshot envelope should retain the initialization principal"
    );
    assert_eq!(
        &snapshot.data[state_offset..],
        3u64.to_be_bytes().as_slice(),
        "the latest snapshot should contain the state after invocation three"
    );
    let oplog_before_recovery = executor.oplog_max_index(&worker_id).await?;

    drop(executor);
    let executor = start_with_snapshot_policy(
        deps,
        &context,
        SnapshotPolicy::EveryNInvocation { count: 4 },
    )
    .await?;
    let mut events = executor.capture_output(&worker_id).await?;

    executor.resume(&worker_id, false).await?;
    executor
        .wait_for_status(&worker_id, AgentStatus::Idle, Duration::from_secs(10))
        .await?;
    assert_eq!(
        executor.oplog_max_index(&worker_id).await?,
        oplog_before_recovery,
        "snapshot loading and suffix replay must not write to the oplog"
    );

    let status = executor
        .invoke_and_await_agent_as_principal(
            &component,
            &agent_id,
            principal,
            "status",
            data_value!(),
        )
        .await?
        .into_typed::<String>()?;
    assert_snapshot_recovery_loaded(&mut events).await;
    let status: SnapshotLoadProbeStatus = serde_json::from_str(&status)?;

    assert_eq!(status.value, 4, "the invocation suffix should be replayed");
    assert_eq!(status.loaded_value, Some(3));
    assert_eq!(status.origin, "restored");
    assert_eq!(status.mode, "read");
    assert_eq!(status.agent_type, "SnapshotLoadProbe");
    let expected_phantom_id = phantom_id.to_string();
    assert_eq!(
        status.phantom_id.as_deref(),
        Some(expected_phantom_id.as_str())
    );
    assert_eq!(status.config_marker, config_marker);
    assert_eq!(status.read_bytes, 4, "local reads should remain available");
    assert_eq!(status.constructor_calls_at_restore, 0);
    assert_eq!(status.constructor_calls_now, 0);
    assert_eq!(status.load_calls_now, 1);
    assert!(
        status.principal.contains(&principal_account_id),
        "restore context should contain the initialization principal"
    );

    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    let recovery_entries: Vec<_> = oplog
        .iter()
        .filter(|entry| entry.oplog_index > oplog_before_recovery)
        .collect();
    assert!(
        recovery_entries
            .iter()
            .all(|entry| !matches!(entry.entry, PublicOplogEntry::Start(_))),
        "snapshot loading and suffix replay must not add durable host-call entries"
    );
    assert!(
        recovery_entries.iter().all(|entry| !matches!(
            &entry.entry,
            PublicOplogEntry::AgentInvocationStarted(params)
                if matches!(params.invocation, PublicAgentInvocation::LoadSnapshot(_))
        )),
        "snapshot loading must not be recorded as an agent invocation"
    );

    drop(executor);
    Ok(())
}

#[test]
#[timeout("120s")]
#[tracing::instrument]
async fn snapshot_load_rejects_write_http_and_rpc_and_falls_back_to_the_older_snapshot(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_sdk_rust")] agent_sdk_rust: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let config_marker = format!("snapshot-config-{}", context.redis_prefix());
    let executor = start_with_snapshot_policy(
        deps,
        &context,
        SnapshotPolicy::EveryNInvocation { count: 1 },
    )
    .await?;
    let component = executor
        .component_dep(&context.default_environment_id, agent_sdk_rust)
        .with_agent_config(
            "SnapshotLoadProbe",
            vec![AgentConfigEntryDto {
                path: vec!["marker".to_string()],
                value: serde_json::Value::String(config_marker.clone()).into(),
            }],
        )
        .store()
        .await?;
    let principal = Principal::GolemUser(GolemUserPrincipal {
        account_id: AccountId::new(),
    });
    let mut probes = Vec::new();

    for mode in ["write", "http", "rpc"] {
        let agent_id = phantom_agent_id!("SnapshotLoadProbe", uuid::Uuid::new_v4(), mode);
        let worker_id = AgentId {
            component_id: component.id,
            agent_id: agent_id.to_string(),
        };
        for expected in 1..=2 {
            let result = executor
                .invoke_and_await_agent_as_principal(
                    &component,
                    &agent_id,
                    principal.clone(),
                    "increment",
                    data_value!(),
                )
                .await?;
            assert_eq!(result.into_typed::<u64>()?, expected);
        }
        let last_finished = executor
            .get_oplog(&worker_id, OplogIndex::INITIAL)
            .await?
            .iter()
            .rev()
            .find(|entry| matches!(entry.entry, PublicOplogEntry::AgentInvocationFinished(_)))
            .expect("the second increment has completed")
            .oplog_index;
        // Automatic snapshot creation follows publication of the invocation result.
        let snapshots = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let snapshots = executor
                    .get_oplog(&worker_id, OplogIndex::INITIAL)
                    .await?
                    .iter()
                    .filter(|entry| matches!(entry.entry, PublicOplogEntry::Snapshot(_)))
                    .map(|entry| entry.oplog_index)
                    .collect::<Vec<_>>();
                if snapshots.last().is_some_and(|index| *index > last_finished) {
                    break Ok::<_, anyhow::Error>(snapshots);
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("the second increment should commit its automatic snapshot")?;
        let [.., older, newest] = snapshots[..] else {
            panic!(
                "{mode} probe should have an older loadable snapshot before the failing latest snapshot"
            );
        };
        let oplog_before_recovery = executor.oplog_max_index(&worker_id).await?;
        probes.push((
            mode,
            agent_id,
            worker_id,
            oplog_before_recovery,
            (newest, older),
        ));
    }

    drop(executor);
    let executor = start_with_snapshot_policy(
        deps,
        &context,
        SnapshotPolicy::EveryNInvocation { count: 1 },
    )
    .await?;

    for (mode, agent_id, worker_id, oplog_before_recovery, (newest, older)) in probes {
        let mut events = executor.capture_output(&worker_id).await?;
        executor.resume(&worker_id, false).await?;
        let recovery = snapshot_recovery_fell_back(
            &mut events,
            "Read-only agent method attempted a side effect",
        )
        .await;
        assert_eq!(
            recovery,
            (newest, older),
            "{mode}: only the failing latest snapshot is rejected, and the start loads the older one"
        );
        executor
            .wait_for_status(&worker_id, AgentStatus::Idle, Duration::from_secs(10))
            .await?;
        assert_eq!(
            executor.oplog_max_index(&worker_id).await?,
            oplog_before_recovery,
            "{mode}: the rejected snapshot and the replay after the older one must not write to the oplog"
        );

        let status = executor
            .invoke_and_await_agent_as_principal(
                &component,
                &agent_id,
                principal.clone(),
                "status",
                data_value!(),
            )
            .await?
            .into_typed::<String>()?;
        let status: SnapshotLoadProbeStatus = serde_json::from_str(&status)?;

        assert_eq!(
            status.value, 2,
            "{mode}: the older snapshot and the replay after it restore state"
        );
        assert_eq!(
            status.loaded_value,
            Some(1),
            "{mode}: the older snapshot was loaded"
        );
        assert_eq!(status.origin, "restored", "{mode}: snapshot restore");
        assert_eq!(status.mode, mode);
        assert_eq!(status.config_marker, config_marker);
        assert_eq!(
            status.read_bytes, 4,
            "{mode}: the load of the older snapshot read its bytes"
        );
        assert_eq!(
            status.constructor_calls_now, 0,
            "{mode}: the start from the older snapshot ran no constructor"
        );
        assert_eq!(
            status.load_calls_now, 1,
            "{mode}: the failed load state was discarded, and the older snapshot loaded once"
        );
    }

    drop(executor);
    Ok(())
}

#[test]
#[tracing::instrument]
async fn snapshot_based_recovery_preserves_state_across_multiple_restarts(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_counters")] agent_counters: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);

    let executor = start_with_snapshot_policy(
        deps,
        &context,
        SnapshotPolicy::EveryNInvocation { count: 1 },
    )
    .await?;

    let component = executor
        .component_dep(&context.default_environment_id, agent_counters)
        .store()
        .await?;
    let agent_id = agent_id!("SnapshotCounter", "multi-restart");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    for _ in 0..105 {
        executor
            .invoke_and_await_agent(&component, &agent_id, "increment", data_value!())
            .await?;
    }

    drop(executor);
    let executor = start_with_snapshot_policy(
        deps,
        &context,
        SnapshotPolicy::EveryNInvocation { count: 1 },
    )
    .await?;

    for _ in 0..3 {
        executor
            .invoke_and_await_agent(&component, &agent_id, "increment", data_value!())
            .await?;
    }

    drop(executor);
    let executor = start_with_snapshot_policy(
        deps,
        &context,
        SnapshotPolicy::EveryNInvocation { count: 1 },
    )
    .await?;
    let mut events = executor.capture_output(&worker_id).await?;

    let result = executor
        .invoke_and_await_agent(&component, &agent_id, "get", data_value!())
        .await?;
    assert_snapshot_recovery_loaded(&mut events).await;

    assert_eq!(
        result.into_typed::<u32>()?,
        108,
        "Counter should be 108 after 105 increments, restart, then 3 more increments"
    );

    drop(executor);
    Ok(())
}

#[test]
#[tracing::instrument]
async fn ts_default_json_snapshot_recovery(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("constructor_parameter_echo")] constructor_parameter_echo: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component_dep(&context.default_environment_id, constructor_parameter_echo)
        .store()
        .await?;
    let agent_id = agent_id!("SnapshotCounterAgent", "ts-recovery");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    for _ in 0..5 {
        executor
            .invoke_and_await_agent(&component, &agent_id, "increment", data_value!())
            .await?;
    }

    let result_before = executor
        .invoke_and_await_agent(&component, &agent_id, "get", data_value!())
        .await?;

    assert_eq!(
        result_before.clone().into_typed::<f64>()?,
        5.0,
        "Counter should be 5 after 5 increments"
    );

    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    let snapshots: Vec<_> = oplog
        .iter()
        .filter_map(|entry| match &entry.entry {
            PublicOplogEntry::Snapshot(params) => Some(params.clone()),
            _ => None,
        })
        .collect();
    assert!(
        !snapshots.is_empty(),
        "Expected at least one snapshot before restart, got 0"
    );

    for (i, snapshot) in snapshots.iter().enumerate() {
        match &snapshot.data {
            PublicSnapshotData::Json(json_data) => {
                let state = json_data
                    .data
                    .get("state")
                    .unwrap_or_else(|| panic!("Snapshot {i} JSON missing 'state' field"));
                assert!(
                    state.get("count").is_some(),
                    "Snapshot {i} JSON state missing 'count' field"
                );
                let count = state["count"].as_f64().unwrap_or_else(|| {
                    panic!("Snapshot {i} 'count' is not a number: {:?}", state["count"])
                });
                assert!(
                    (0.0..=5.0).contains(&count),
                    "Snapshot {i} count should be between 0 and 5, got {count}"
                );
            }
            other => {
                panic!(
                    "Expected JSON snapshot but got {:?}",
                    std::mem::discriminant(other)
                );
            }
        }
    }

    drop(executor);
    let executor = start(deps, &context).await?;
    let mut events = executor.capture_output(&worker_id).await?;

    let result_after = executor
        .invoke_and_await_agent(&component, &agent_id, "get", data_value!())
        .await?;
    assert_snapshot_recovery_loaded(&mut events).await;

    assert_eq!(
        result_before, result_after,
        "TS agent state should be preserved across restart via default JSON snapshot recovery"
    );

    let increment_after = executor
        .invoke_and_await_agent(&component, &agent_id, "increment", data_value!())
        .await?;

    assert_eq!(
        increment_after.into_typed::<f64>()?,
        6.0,
        "Counter should continue from 6 after snapshot recovery"
    );

    executor.check_oplog_is_queryable(&worker_id).await?;

    drop(executor);
    Ok(())
}

#[test]
#[tracing::instrument]
async fn ts_default_json_snapshot_recovery_across_multiple_restarts(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("constructor_parameter_echo")] constructor_parameter_echo: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);

    let executor = start(deps, &context).await?;

    let component = executor
        .component_dep(&context.default_environment_id, constructor_parameter_echo)
        .store()
        .await?;
    let agent_id = agent_id!("SnapshotCounterAgent", "ts-multi-restart");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    for _ in 0..3 {
        executor
            .invoke_and_await_agent(&component, &agent_id, "increment", data_value!())
            .await?;
    }

    let oplog1 = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    let snapshots1: Vec<_> = oplog1
        .iter()
        .filter_map(|entry| match &entry.entry {
            PublicOplogEntry::Snapshot(params) => Some(params.clone()),
            _ => None,
        })
        .collect();
    assert!(
        !snapshots1.is_empty(),
        "Expected at least one snapshot after first round of increments"
    );

    drop(executor);
    let executor = start(deps, &context).await?;

    for _ in 0..3 {
        executor
            .invoke_and_await_agent(&component, &agent_id, "increment", data_value!())
            .await?;
    }

    let oplog2 = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    let snapshots2: Vec<_> = oplog2
        .iter()
        .filter_map(|entry| match &entry.entry {
            PublicOplogEntry::Snapshot(params) => Some(params.clone()),
            _ => None,
        })
        .collect();
    assert!(
        snapshots2.len() > snapshots1.len(),
        "Expected more snapshots after second round of increments"
    );

    for (i, snapshot) in snapshots2.iter().enumerate() {
        match &snapshot.data {
            PublicSnapshotData::Json(json_data) => {
                let state = json_data
                    .data
                    .get("state")
                    .unwrap_or_else(|| panic!("Snapshot {i} JSON missing 'state' field"));
                assert!(
                    state.get("count").is_some(),
                    "Snapshot {i} JSON state missing 'count' field"
                );
                let count = state["count"].as_f64().unwrap_or_else(|| {
                    panic!("Snapshot {i} 'count' is not a number: {:?}", state["count"])
                });
                assert!(
                    (0.0..=6.0).contains(&count),
                    "Snapshot {i} count should be between 0 and 6, got {count}"
                );
            }
            other => {
                panic!(
                    "Expected JSON snapshot but got {:?}",
                    std::mem::discriminant(other)
                );
            }
        }
    }

    drop(executor);
    let executor = start(deps, &context).await?;
    let mut events = executor.capture_output(&worker_id).await?;

    let result = executor
        .invoke_and_await_agent(&component, &agent_id, "get", data_value!())
        .await?;
    assert_snapshot_recovery_loaded(&mut events).await;

    assert_eq!(
        result.into_typed::<f64>()?,
        6.0,
        "Counter should be 6 after two rounds of 3 increments across restarts"
    );

    drop(executor);
    Ok(())
}

#[test]
#[tracing::instrument]
async fn rust_default_json_snapshot_recovery(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_counters")] agent_counters: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start_with_snapshot_policy(
        deps,
        &context,
        SnapshotPolicy::EveryNInvocation { count: 1 },
    )
    .await?;

    let component = executor
        .component_dep(&context.default_environment_id, agent_counters)
        .store()
        .await?;
    let agent_id = agent_id!("JsonSnapshotCounter", "rust-recovery");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    for _ in 0..5 {
        executor
            .invoke_and_await_agent(&component, &agent_id, "increment", data_value!())
            .await?;
    }

    let result_before = executor
        .invoke_and_await_agent(&component, &agent_id, "get", data_value!())
        .await?;

    assert_eq!(
        result_before.clone().into_typed::<u32>()?,
        5,
        "Counter should be 5 after 5 increments"
    );

    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    let snapshots: Vec<_> = oplog
        .iter()
        .filter_map(|entry| match &entry.entry {
            PublicOplogEntry::Snapshot(params) => Some(params.clone()),
            _ => None,
        })
        .collect();
    assert!(
        !snapshots.is_empty(),
        "Expected at least one snapshot before restart, got 0"
    );

    for (i, snapshot) in snapshots.iter().enumerate() {
        match &snapshot.data {
            PublicSnapshotData::Json(json_data) => {
                let state = json_data
                    .data
                    .get("state")
                    .unwrap_or_else(|| panic!("Snapshot {i} JSON missing 'state' field"));
                assert!(
                    state.get("count").is_some(),
                    "Snapshot {i} JSON state missing 'count' field"
                );
                let count = state["count"].as_u64().unwrap_or_else(|| {
                    panic!("Snapshot {i} 'count' is not a number: {:?}", state["count"])
                });
                assert!(
                    (0..=5).contains(&count),
                    "Snapshot {i} count should be between 0 and 5, got {count}"
                );
            }
            other => {
                panic!(
                    "Expected JSON snapshot but got {:?}",
                    std::mem::discriminant(other)
                );
            }
        }
    }

    drop(executor);
    let executor = start_with_snapshot_policy(
        deps,
        &context,
        SnapshotPolicy::EveryNInvocation { count: 1 },
    )
    .await?;
    let mut events = executor.capture_output(&worker_id).await?;

    let result_after = executor
        .invoke_and_await_agent(&component, &agent_id, "get", data_value!())
        .await?;
    assert_snapshot_recovery_loaded(&mut events).await;

    assert_eq!(
        result_before, result_after,
        "Rust agent state should be preserved across restart via default JSON snapshot recovery"
    );

    let increment_after = executor
        .invoke_and_await_agent(&component, &agent_id, "increment", data_value!())
        .await?;

    assert_eq!(
        increment_after.into_typed::<u32>()?,
        6,
        "Counter should continue from 6 after snapshot recovery"
    );

    executor.check_oplog_is_queryable(&worker_id).await?;

    drop(executor);
    Ok(())
}

#[test]
#[tracing::instrument]
async fn rust_default_json_snapshot_recovery_across_multiple_restarts(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_counters")] agent_counters: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);

    let executor = start_with_snapshot_policy(
        deps,
        &context,
        SnapshotPolicy::EveryNInvocation { count: 1 },
    )
    .await?;

    let component = executor
        .component_dep(&context.default_environment_id, agent_counters)
        .store()
        .await?;
    let agent_id = agent_id!("JsonSnapshotCounter", "rust-multi-restart");
    let _worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    for _ in 0..3 {
        executor
            .invoke_and_await_agent(&component, &agent_id, "increment", data_value!())
            .await?;
    }

    let oplog1 = executor.get_oplog(&_worker_id, OplogIndex::INITIAL).await?;
    let snapshots1: Vec<_> = oplog1
        .iter()
        .filter_map(|entry| match &entry.entry {
            PublicOplogEntry::Snapshot(params) => Some(params.clone()),
            _ => None,
        })
        .collect();
    assert!(
        !snapshots1.is_empty(),
        "Expected at least one snapshot after first round of increments"
    );

    drop(executor);
    let executor = start_with_snapshot_policy(
        deps,
        &context,
        SnapshotPolicy::EveryNInvocation { count: 1 },
    )
    .await?;

    for _ in 0..3 {
        executor
            .invoke_and_await_agent(&component, &agent_id, "increment", data_value!())
            .await?;
    }

    let oplog2 = executor.get_oplog(&_worker_id, OplogIndex::INITIAL).await?;
    let snapshots2: Vec<_> = oplog2
        .iter()
        .filter_map(|entry| match &entry.entry {
            PublicOplogEntry::Snapshot(params) => Some(params.clone()),
            _ => None,
        })
        .collect();
    assert!(
        snapshots2.len() > snapshots1.len(),
        "Expected more snapshots after second round of increments"
    );

    for (i, snapshot) in snapshots2.iter().enumerate() {
        match &snapshot.data {
            PublicSnapshotData::Json(json_data) => {
                let state = json_data
                    .data
                    .get("state")
                    .unwrap_or_else(|| panic!("Snapshot {i} JSON missing 'state' field"));
                assert!(
                    state.get("count").is_some(),
                    "Snapshot {i} JSON state missing 'count' field"
                );
                let count = state["count"].as_u64().unwrap_or_else(|| {
                    panic!("Snapshot {i} 'count' is not a number: {:?}", state["count"])
                });
                assert!(
                    (0..=6).contains(&count),
                    "Snapshot {i} count should be between 0 and 6, got {count}"
                );
            }
            other => {
                panic!(
                    "Expected JSON snapshot but got {:?}",
                    std::mem::discriminant(other)
                );
            }
        }
    }

    drop(executor);
    let executor = start_with_snapshot_policy(
        deps,
        &context,
        SnapshotPolicy::EveryNInvocation { count: 1 },
    )
    .await?;
    let mut events = executor.capture_output(&_worker_id).await?;

    let result = executor
        .invoke_and_await_agent(&component, &agent_id, "get", data_value!())
        .await?;
    assert_snapshot_recovery_loaded(&mut events).await;

    assert_eq!(
        result.into_typed::<u32>()?,
        6,
        "Counter should be 6 after two rounds of 3 increments across restarts (rust)"
    );

    drop(executor);
    Ok(())
}

/// On an executor without filesystem snapshots, `SqliteSnapshotAgent` takes no periodic snapshot:
/// its constructor writes the file of its file-backed database, so the tree differs from its
/// initial files, and a start from a record without a name would not get the file back. The
/// restart replays the whole oplog, which rebuilds the databases.
#[test]
#[tracing::instrument]
async fn ts_sqlite_file_database_without_filesystem_snapshots_takes_no_snapshot_and_replays(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("constructor_parameter_echo")] constructor_parameter_echo: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use futures::StreamExt as _;

    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component_dep(&context.default_environment_id, constructor_parameter_echo)
        .store()
        .await?;
    let agent_id = agent_id!("SqliteSnapshotAgent", "sqlite-no-filesystem-snapshots");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    executor
        .invoke_and_await_agent(&component, &agent_id, "addItem", data_value!("apple"))
        .await?;
    executor
        .invoke_and_await_agent(&component, &agent_id, "addLog", data_value!("started"))
        .await?;
    executor
        .invoke_and_await_agent(&component, &agent_id, "setLabel", data_value!("after-init"))
        .await?;
    let state_before = executor
        .invoke_and_await_agent(&component, &agent_id, "getState", data_value!())
        .await?;
    // The snapshot after an invocation, and its check, run after the invocation returned to the
    // caller, so the test waits for the checks.
    tokio::time::timeout(
        Duration::from_secs(30),
        std::pin::pin!(
            futures::stream::repeat(())
                .then(|()| tokio::time::sleep(Duration::from_millis(50)))
                .filter(|()| std::future::ready(executor.filesystem_captures("changed") >= 2))
        )
        .next(),
    )
    .await
    .map_err(|_| anyhow::anyhow!("fewer than two checks found a changed tree"))?;

    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    let snapshots: Vec<_> = oplog
        .iter()
        .filter(|entry| matches!(&entry.entry, PublicOplogEntry::Snapshot(_)))
        .map(|entry| entry.oplog_index)
        .collect();

    drop(executor);
    let executor = start(deps, &context).await?;
    let mut events = executor.capture_output(&worker_id).await?;

    let state_after = executor
        .invoke_and_await_agent(&component, &agent_id, "getState", data_value!())
        .await?;
    let recoveries: Vec<String> = tokio::time::timeout(
        Duration::from_secs(10),
        futures::stream::poll_fn(|context| events.poll_recv(context))
            .filter_map(|event| std::future::ready(AgentEvent::try_from(event).ok()))
            .take_while(|event| {
                std::future::ready(!matches!(event, AgentEvent::InvocationFinished { .. }))
            })
            .filter_map(|event| {
                std::future::ready(match event {
                    AgentEvent::SnapshotRecoveryFailed { snapshot_index, .. } => {
                        Some(format!("failed at {snapshot_index}"))
                    }
                    AgentEvent::SnapshotRecoverySucceeded { snapshot_index, .. } => {
                        Some(format!("loaded at {snapshot_index}"))
                    }
                    _ => None,
                })
            })
            .collect(),
    )
    .await?;

    assert!(
        snapshots.is_empty(),
        "a periodic snapshot was taken after the database file existed: {snapshots:?}"
    );
    assert_eq!(state_before, state_after);
    assert!(recoveries.is_empty(), "{recoveries:?}");
    executor.check_oplog_is_queryable(&worker_id).await?;
    assert_eq!(
        invocation_shape(&executor.stored_oplog(&worker_id).await),
        InvocationShape::settled()
    );
    drop(executor);
    Ok(())
}

/// The snapshot entries in the oplog of `agent_id`.
async fn snapshot_count(
    executor: &golem_worker_executor_test_utils::TestWorkerExecutor,
    agent_id: &AgentId,
) -> anyhow::Result<usize> {
    Ok(executor
        .get_oplog(agent_id, OplogIndex::INITIAL)
        .await?
        .iter()
        .filter(|entry| matches!(&entry.entry, PublicOplogEntry::Snapshot(_)))
        .count())
}

/// With an executor policy that takes a snapshot after each invocation, an ephemeral agent with
/// snapshot hooks takes none, and a durable agent of the same component still takes snapshots.
/// The executor keeps no filesystem snapshots, so each snapshot checks the tree of its agent after
/// the save hook: a check count of zero shows that no snapshot of the ephemeral agent ran.
#[test]
#[tracing::instrument]
async fn an_ephemeral_agent_takes_no_snapshot_and_a_durable_agent_does(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_counters")] agent_counters: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start_with_snapshot_policy(
        deps,
        &context,
        SnapshotPolicy::EveryNInvocation { count: 1 },
    )
    .await?;
    let component = executor
        .component_dep(&context.default_environment_id, agent_counters)
        .store()
        .await?;
    let checks = || {
        ["initial_files", "changed", "captured", "unchanged"]
            .map(|outcome| executor.filesystem_captures(outcome))
            .into_iter()
            .sum::<u64>()
    };
    let ephemeral = agent_id!("EphemeralSnapshotCounter", "no-snapshot");
    executor
        .start_agent(&component.id, ephemeral.clone())
        .await?;

    executor
        .invoke_and_await_agent(&component, &ephemeral, "increment", data_value!())
        .await?;
    executor
        .invoke_and_await_agent(&component, &ephemeral, "increment", data_value!())
        .await?;
    // The snapshot after an invocation runs after the invocation returned to the caller.
    tokio::time::sleep(Duration::from_secs(2)).await;
    let ephemeral_checks = checks();
    let durable = agent_id!("SnapshotCounter", "snapshots");
    let durable_id = executor.start_agent(&component.id, durable.clone()).await?;
    executor
        .invoke_and_await_agent(&component, &durable, "increment", data_value!())
        .await?;
    executor
        .invoke_and_await_agent(&component, &durable, "increment", data_value!())
        .await?;
    tokio::time::sleep(Duration::from_secs(2)).await;

    assert_eq!(ephemeral_checks, 0);
    assert!(snapshot_count(&executor, &durable_id).await? >= 1);
    assert!(checks() >= 1);
    drop(executor);
    Ok(())
}

#[test]
#[tracing::instrument]
async fn monotonic_clock_now_replay_parity(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;
    let agent_id = agent_id!("MonotonicClockState", "monotonic-replay-parity-1");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    // Live: record a single `monotonic_clock::now()` reading. This goes through the
    // concurrent-replay path (eager `Start` + resolver-matched `End`) and stores the value in
    // the agent's state.
    let recorded_live = executor
        .invoke_and_await_agent(&component, &agent_id, "record_now", data_value!())
        .await?
        .into_typed::<u64>()?;

    executor.check_oplog_is_queryable(&worker_id).await?;

    // Restart the executor so the worker is recovered by replaying its oplog. During replay
    // `monotonic_clock::now()` must yield the recorded value, so the rebuilt agent state must equal
    // the live reading.
    drop(executor);
    let executor = start(deps, &context).await?;

    let recorded_after_replay = executor
        .invoke_and_await_agent(&component, &agent_id, "get_recorded", data_value!())
        .await?
        .into_typed::<u64>()?;

    executor.check_oplog_is_queryable(&worker_id).await?;
    drop(executor);

    assert_eq!(
        recorded_live, recorded_after_replay,
        "monotonic_clock::now() must replay to the same value recorded live"
    );
    Ok(())
}
