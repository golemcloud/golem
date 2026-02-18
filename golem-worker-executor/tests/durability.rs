// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
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
use axum::extract::Query;
use axum::response::Response;
use axum::routing::get;
use axum::{BoxError, Router};
use bytes::Bytes;
use futures::{stream, StreamExt};
use golem_common::model::oplog::{OplogIndex, PublicOplogEntry, PublicSnapshotData};
use golem_common::{agent_id, data_value};
use golem_test_framework::dsl::TestDsl;
use golem_wasm::Value;
use golem_worker_executor::services::golem_config::SnapshotPolicy;
use golem_worker_executor_test_utils::{
    start, start_with_snapshot_policy, LastUniqueId, TestContext, WorkerExecutorTestDependencies,
};
use http::StatusCode;
use pretty_assertions::assert_eq;
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;
use test_r::{inherit_test_dep, test};
use tokio::sync::Mutex;
use tracing::Instrument;

inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(LastUniqueId);
inherit_test_dep!(Tracing);

#[test]
#[tracing::instrument]
async fn custom_durability_1(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let response = Arc::new(AtomicU32::new(0));
    let response_clone = response.clone();

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
        .component(
            &context.default_environment_id,
            "golem_it_host_api_tests_release",
        )
        .name("golem-it:host-api-tests")
        .store()
        .await?;
    let agent_id = agent_id!("custom-durability", "custom-durability-1");
    let mut env = HashMap::new();
    env.insert("PORT".to_string(), host_http_port.to_string());

    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), env, vec![])
        .await?;

    let result1 = executor
        .invoke_and_await_agent(&component.id, &agent_id, "callback", data_value!("a"))
        .await?;

    executor.check_oplog_is_queryable(&worker_id).await?;

    drop(executor);
    let executor = start(deps, &context).await?;

    let result2 = executor
        .invoke_and_await_agent(&component.id, &agent_id, "callback", data_value!("b"))
        .await?;

    executor.check_oplog_is_queryable(&worker_id).await?;

    drop(executor);
    http_server.abort();

    assert_eq!(
        result1.into_return_value(),
        Some(Value::String("0-a".to_string()))
    );
    assert_eq!(
        result2.into_return_value(),
        Some(Value::String("1-b".to_string()))
    );
    Ok(())
}

#[test]
#[tracing::instrument]
async fn lazy_pollable(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await.unwrap();

    let host_http_port = listener.local_addr().unwrap().port();

    #[derive(Deserialize)]
    struct QueryParams {
        idx: u32,
    }

    let (signal_tx, signal_rx) = tokio::sync::mpsc::unbounded_channel();
    let signal_rx = Arc::new(Mutex::new(signal_rx));

    let http_server = tokio::spawn(
        async move {
            let route = Router::new().route(
                "/fetch",
                get(move |query: Query<QueryParams>| async move {
                    let idx = query.idx;
                    tracing::info!("fetch called with: {}", idx);

                    let stream = stream::iter(0..3).then(move |i| {
                        let signal_rx = signal_rx.clone();
                        async move {
                            tracing::info!("fetch awaiting signal");
                            signal_rx.lock().await.recv().await;
                            let fragment_str = format!("chunk-{idx}-{i}\n");
                            tracing::info!("emitting response fragment: {fragment_str}");
                            let fragment = Bytes::from(fragment_str);
                            Ok::<Bytes, BoxError>(fragment)
                        }
                    });

                    Response::builder()
                        .status(StatusCode::OK)
                        .body(axum::body::Body::from_stream(stream))
                        .unwrap()
                }),
            );

            axum::serve(listener, route).await.unwrap();
        }
        .in_current_span(),
    );

    let component = executor
        .component(
            &context.default_environment_id,
            "golem_it_host_api_tests_release",
        )
        .name("golem-it:host-api-tests")
        .store()
        .await?;
    let agent_id = agent_id!("custom-durability", "lazy-pollable-1");
    let mut env = HashMap::new();
    env.insert("PORT".to_string(), host_http_port.to_string());

    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), env, vec![])
        .await?;

    signal_tx.send(()).unwrap();

    executor
        .invoke_and_await_agent(
            &component.id,
            &agent_id,
            "lazy_pollable_init",
            data_value!(),
        )
        .await?;

    let s1 = executor
        .invoke_and_await_agent(
            &component.id,
            &agent_id,
            "lazy_pollable_test",
            data_value!(1u32),
        )
        .await?;

    signal_tx.send(()).unwrap();

    let s2 = executor
        .invoke_and_await_agent(
            &component.id,
            &agent_id,
            "lazy_pollable_test",
            data_value!(2u32),
        )
        .await?;

    signal_tx.send(()).unwrap();

    let s3 = executor
        .invoke_and_await_agent(
            &component.id,
            &agent_id,
            "lazy_pollable_test",
            data_value!(3u32),
        )
        .await?;

    signal_tx.send(()).unwrap();

    drop(executor);
    let executor = start(deps, &context).await?;

    signal_tx.send(()).unwrap();

    let s4 = executor
        .invoke_and_await_agent(
            &component.id,
            &agent_id,
            "lazy_pollable_test",
            data_value!(3u32),
        )
        .await?;

    executor.check_oplog_is_queryable(&worker_id).await?;
    http_server.abort();

    assert_eq!(
        s1.into_return_value(),
        Some(Value::String("chunk-1-0\n".to_string()))
    );
    assert_eq!(
        s2.into_return_value(),
        Some(Value::String("chunk-1-1\n".to_string()))
    );
    assert_eq!(
        s3.into_return_value(),
        Some(Value::String("chunk-1-2\n".to_string()))
    );
    assert_eq!(
        s4.into_return_value(),
        Some(Value::String("chunk-3-0\n".to_string()))
    );
    Ok(())
}

const SNAPSHOT_TEST_INVOCATIONS: usize = 10;

#[test]
#[tracing::instrument]
async fn automatic_snapshot_disabled(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start_with_snapshot_policy(deps, &context, SnapshotPolicy::Disabled).await?;

    let component = executor
        .component(&context.default_environment_id, "it_agent_counters_release")
        .name("it:agent-counters")
        .store()
        .await?;
    let agent_id = agent_id!("snapshot-counter", "disabled");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    for _ in 0..SNAPSHOT_TEST_INVOCATIONS {
        executor
            .invoke_and_await_agent(&component.id, &agent_id, "increment", data_value!())
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
        .component(&context.default_environment_id, "it_agent_counters_release")
        .name("it:agent-counters")
        .store()
        .await?;
    let agent_id = agent_id!("snapshot-counter", "every-2nd");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    for _ in 0..SNAPSHOT_TEST_INVOCATIONS {
        executor
            .invoke_and_await_agent(&component.id, &agent_id, "increment", data_value!())
            .await?;
    }

    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    let snapshot_count = oplog
        .iter()
        .filter(|entry| matches!(&entry.entry, PublicOplogEntry::Snapshot(_)))
        .count();

    drop(executor);

    assert_eq!(
        snapshot_count,
        SNAPSHOT_TEST_INVOCATIONS / 2,
        "Expected a snapshot every 2 invocations"
    );
    Ok(())
}

#[test]
#[tracing::instrument]
async fn automatic_snapshot_periodic(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
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
        .component(&context.default_environment_id, "it_agent_counters_release")
        .name("it:agent-counters")
        .store()
        .await?;
    let agent_id = agent_id!("snapshot-counter", "periodic");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    for _ in 0..SNAPSHOT_TEST_INVOCATIONS {
        executor
            .invoke_and_await_agent(&component.id, &agent_id, "increment", data_value!())
            .await?;
        tokio::time::sleep(Duration::from_millis(500)).await;
    }

    tokio::time::sleep(Duration::from_secs(3)).await;

    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    let snapshot_count = oplog
        .iter()
        .filter(|entry| matches!(&entry.entry, PublicOplogEntry::Snapshot(_)))
        .count();

    drop(executor);

    assert!(
        snapshot_count >= 1,
        "Expected at least 1 snapshot with periodic policy (every 2s over ~5s of invocations), got {snapshot_count}"
    );
    assert!(
        snapshot_count <= SNAPSHOT_TEST_INVOCATIONS,
        "Expected at most {SNAPSHOT_TEST_INVOCATIONS} snapshots, got {snapshot_count}"
    );
    Ok(())
}

#[test]
#[tracing::instrument]
async fn snapshot_based_recovery(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
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
        .component(&context.default_environment_id, "it_agent_counters_release")
        .name("it:agent-counters")
        .store()
        .await?;
    let agent_id = agent_id!("snapshot-counter", "recovery");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    for _ in 0..5 {
        executor
            .invoke_and_await_agent(&component.id, &agent_id, "increment", data_value!())
            .await?;
    }

    let result_before = executor
        .invoke_and_await_agent(&component.id, &agent_id, "get", data_value!())
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
        SnapshotPolicy::EveryNInvocation { count: 1 },
    )
    .await?;

    let result_after = executor
        .invoke_and_await_agent(&component.id, &agent_id, "get", data_value!())
        .await?;

    assert_eq!(
        result_before, result_after,
        "Worker state should be preserved across restart via snapshot recovery"
    );

    let was_recovered = executor
        .invoke_and_await_agent(
            &component.id,
            &agent_id,
            "was_recovered_from_snapshot",
            data_value!(),
        )
        .await?;

    assert_eq!(
        was_recovered.into_return_value(),
        Some(Value::Bool(true)),
        "Worker should have been recovered from snapshot, not replayed from scratch"
    );

    let increment_after = executor
        .invoke_and_await_agent(&component.id, &agent_id, "increment", data_value!())
        .await?;

    assert_eq!(
        increment_after.into_return_value(),
        Some(Value::U32(6)),
        "Counter should continue from 6 after snapshot recovery"
    );

    drop(executor);
    Ok(())
}

#[test]
#[tracing::instrument]
async fn snapshot_based_recovery_preserves_state_across_multiple_restarts(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
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
        .component(&context.default_environment_id, "it_agent_counters_release")
        .name("it:agent-counters")
        .store()
        .await?;
    let agent_id = agent_id!("snapshot-counter", "multi-restart");
    let _worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    for _ in 0..3 {
        executor
            .invoke_and_await_agent(&component.id, &agent_id, "increment", data_value!())
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
            .invoke_and_await_agent(&component.id, &agent_id, "increment", data_value!())
            .await?;
    }

    drop(executor);
    let executor = start_with_snapshot_policy(
        deps,
        &context,
        SnapshotPolicy::EveryNInvocation { count: 1 },
    )
    .await?;

    let result = executor
        .invoke_and_await_agent(&component.id, &agent_id, "get", data_value!())
        .await?;

    assert_eq!(
        result.into_return_value(),
        Some(Value::U32(6)),
        "Counter should be 6 after two rounds of 3 increments across restarts"
    );

    let was_recovered = executor
        .invoke_and_await_agent(
            &component.id,
            &agent_id,
            "was_recovered_from_snapshot",
            data_value!(),
        )
        .await?;

    assert_eq!(
        was_recovered.into_return_value(),
        Some(Value::Bool(true)),
        "Worker should have been recovered from snapshot after multiple restarts"
    );

    drop(executor);
    Ok(())
}

#[test]
#[tracing::instrument]
async fn ts_default_json_snapshot_recovery(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component(
            &context.default_environment_id,
            "golem_it_constructor_parameter_echo",
        )
        .name("golem-it:constructor-parameter-echo")
        .store()
        .await?;
    let agent_id = agent_id!("snapshot-counter-agent", "ts-recovery");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    for _ in 0..5 {
        executor
            .invoke_and_await_agent(&component.id, &agent_id, "increment", data_value!())
            .await?;
    }

    let result_before = executor
        .invoke_and_await_agent(&component.id, &agent_id, "get", data_value!())
        .await?;

    assert_eq!(
        result_before.clone().into_return_value(),
        Some(Value::F64(5.0)),
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
                    (1.0..=5.0).contains(&count),
                    "Snapshot {i} count should be between 1 and 5, got {count}"
                );
            }
            PublicSnapshotData::Raw(raw) => {
                panic!(
                    "Expected JSON snapshot but got Raw with mime_type '{}'",
                    raw.mime_type
                );
            }
        }
    }

    drop(executor);
    let executor = start(deps, &context).await?;

    let result_after = executor
        .invoke_and_await_agent(&component.id, &agent_id, "get", data_value!())
        .await?;

    assert_eq!(
        result_before, result_after,
        "TS agent state should be preserved across restart via default JSON snapshot recovery"
    );

    let increment_after = executor
        .invoke_and_await_agent(&component.id, &agent_id, "increment", data_value!())
        .await?;

    assert_eq!(
        increment_after.into_return_value(),
        Some(Value::F64(6.0)),
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
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);

    let executor = start(deps, &context).await?;

    let component = executor
        .component(
            &context.default_environment_id,
            "golem_it_constructor_parameter_echo",
        )
        .name("golem-it:constructor-parameter-echo")
        .store()
        .await?;
    let agent_id = agent_id!("snapshot-counter-agent", "ts-multi-restart");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    for _ in 0..3 {
        executor
            .invoke_and_await_agent(&component.id, &agent_id, "increment", data_value!())
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
            .invoke_and_await_agent(&component.id, &agent_id, "increment", data_value!())
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
                    (1.0..=6.0).contains(&count),
                    "Snapshot {i} count should be between 1 and 6, got {count}"
                );
            }
            PublicSnapshotData::Raw(raw) => {
                panic!(
                    "Expected JSON snapshot but got Raw with mime_type '{}'",
                    raw.mime_type
                );
            }
        }
    }

    drop(executor);
    let executor = start(deps, &context).await?;

    let result = executor
        .invoke_and_await_agent(&component.id, &agent_id, "get", data_value!())
        .await?;

    assert_eq!(
        result.into_return_value(),
        Some(Value::F64(6.0)),
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
        .component(&context.default_environment_id, "it_agent_counters_release")
        .name("it:agent-counters")
        .store()
        .await?;
    let agent_id = agent_id!("json-snapshot-counter", "rust-recovery");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    for _ in 0..5 {
        executor
            .invoke_and_await_agent(&component.id, &agent_id, "increment", data_value!())
            .await?;
    }

    let result_before = executor
        .invoke_and_await_agent(&component.id, &agent_id, "get", data_value!())
        .await?;

    assert_eq!(
        result_before.clone().into_return_value(),
        Some(Value::U32(5)),
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
                    (1..=5).contains(&count),
                    "Snapshot {i} count should be between 1 and 5, got {count}"
                );
            }
            PublicSnapshotData::Raw(raw) => {
                panic!(
                    "Expected JSON snapshot but got Raw with mime_type '{}'",
                    raw.mime_type
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

    let result_after = executor
        .invoke_and_await_agent(&component.id, &agent_id, "get", data_value!())
        .await?;

    assert_eq!(
        result_before, result_after,
        "Rust agent state should be preserved across restart via default JSON snapshot recovery"
    );

    let increment_after = executor
        .invoke_and_await_agent(&component.id, &agent_id, "increment", data_value!())
        .await?;

    assert_eq!(
        increment_after.into_return_value(),
        Some(Value::U32(6)),
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
        .component(&context.default_environment_id, "it_agent_counters_release")
        .name("it:agent-counters")
        .store()
        .await?;
    let agent_id = agent_id!("json-snapshot-counter", "rust-multi-restart");
    let _worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    for _ in 0..3 {
        executor
            .invoke_and_await_agent(&component.id, &agent_id, "increment", data_value!())
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
            .invoke_and_await_agent(&component.id, &agent_id, "increment", data_value!())
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
                    (1..=6).contains(&count),
                    "Snapshot {i} count should be between 1 and 6, got {count}"
                );
            }
            PublicSnapshotData::Raw(raw) => {
                panic!(
                    "Expected JSON snapshot but got Raw with mime_type '{}'",
                    raw.mime_type
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

    let result = executor
        .invoke_and_await_agent(&component.id, &agent_id, "get", data_value!())
        .await?;

    assert_eq!(
        result.into_return_value(),
        Some(Value::U32(6)),
        "Counter should be 6 after two rounds of 3 increments across restarts"
    );

    drop(executor);
    Ok(())
}
