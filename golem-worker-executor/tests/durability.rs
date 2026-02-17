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
use golem_common::model::oplog::{OplogIndex, PublicOplogEntry};
use golem_common::model::WorkerId;
use golem_common::{agent_id, data_value};
use golem_test_framework::dsl::TestDsl;
use golem_wasm::analysis::{AnalysedResourceId, AnalysedResourceMode, AnalysedType, TypeHandle};
use golem_wasm::{IntoValueAndType, Value, ValueAndType};
use golem_worker_executor::services::golem_config::SnapshotPolicy;
use golem_worker_executor_test_utils::{
    start, start_with_snapshot_policy, LastUniqueId, TestContext, WorkerExecutorTestDependencies,
};
use http::StatusCode;
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
        .store()
        .await?;
    let worker_id = WorkerId {
        component_id: component.id,
        worker_name: "snapshot-counter(\"disabled\")".to_string(),
    };

    for _ in 0..SNAPSHOT_TEST_INVOCATIONS {
        executor
            .invoke_and_await(
                &worker_id,
                "it:agent-counters/snapshot-counter.{increment}",
                vec![],
            )
            .await??;
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
        .store()
        .await?;
    let worker_id = WorkerId {
        component_id: component.id,
        worker_name: "snapshot-counter(\"every-2nd\")".to_string(),
    };

    for _ in 0..SNAPSHOT_TEST_INVOCATIONS {
        executor
            .invoke_and_await(
                &worker_id,
                "it:agent-counters/snapshot-counter.{increment}",
                vec![],
            )
            .await??;
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
        .store()
        .await?;
    let worker_id = WorkerId {
        component_id: component.id,
        worker_name: "snapshot-counter(\"periodic\")".to_string(),
    };

    for _ in 0..SNAPSHOT_TEST_INVOCATIONS {
        executor
            .invoke_and_await(
                &worker_id,
                "it:agent-counters/snapshot-counter.{increment}",
                vec![],
            )
            .await??;
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
