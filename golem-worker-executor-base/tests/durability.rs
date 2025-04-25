// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use axum::extract::Query;
use axum::response::Response;
use axum::routing::get;
use axum::{BoxError, Router};
use bytes::Bytes;
use futures_util::{stream, StreamExt};
use golem_wasm_rpc::{IntoValueAndType, Value};
use http::StatusCode;
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use test_r::{inherit_test_dep, test};
use tokio::sync::Mutex;
use tracing::Instrument;

use crate::common::{start, TestContext};
use crate::{LastUniqueId, Tracing, WorkerExecutorTestDependencies};
use golem_test_framework::dsl::TestDslUnsafe;

inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(LastUniqueId);
inherit_test_dep!(Tracing);

#[test]
#[tracing::instrument]
async fn custom_durability_1(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await.unwrap();

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

    let component_id = executor.component("custom_durability").store().await;
    let mut env = HashMap::new();
    env.insert("PORT".to_string(), host_http_port.to_string());

    let worker_id = executor
        .start_worker_with(&component_id, "custom-durability-1", vec![], env)
        .await;

    let result1 = executor
        .invoke_and_await(
            &worker_id,
            "golem:it-exports/golem-it-api.{callback}",
            vec!["a".into_value_and_type()],
        )
        .await
        .unwrap();

    executor.check_oplog_is_queryable(&worker_id).await;

    drop(executor);

    let executor = start(deps, &context).await.unwrap();

    let result2 = executor
        .invoke_and_await(
            &worker_id,
            "golem:it-exports/golem-it-api.{callback}",
            vec!["b".into_value_and_type()],
        )
        .await
        .unwrap();

    executor.check_oplog_is_queryable(&worker_id).await;

    drop(executor);
    http_server.abort();

    assert_eq!(result1, vec![Value::String("0-a".to_string())]);
    assert_eq!(result2, vec![Value::String("1-b".to_string())]);
}

#[test]
#[tracing::instrument]
async fn lazy_pollable(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await.unwrap();

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
                            let fragment_str = format!("chunk-{}-{}\n", idx, i);
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

    let component_id = executor.component("custom_durability").store().await;
    let mut env = HashMap::new();
    env.insert("PORT".to_string(), host_http_port.to_string());

    let worker_id = executor
        .start_worker_with(&component_id, "custom-durability-1", vec![], env)
        .await;

    signal_tx.send(()).unwrap();

    let s1 = executor
        .invoke_and_await(
            &worker_id,
            "golem:it-exports/golem-it-api.{lazy-pollable-test().test}",
            vec![1u32.into_value_and_type()],
        )
        .await
        .unwrap();

    signal_tx.send(()).unwrap();

    let s2 = executor
        .invoke_and_await(
            &worker_id,
            "golem:it-exports/golem-it-api.{lazy-pollable-test().test}",
            vec![2u32.into_value_and_type()],
        )
        .await
        .unwrap();

    signal_tx.send(()).unwrap();

    let s3 = executor
        .invoke_and_await(
            &worker_id,
            "golem:it-exports/golem-it-api.{lazy-pollable-test().test}",
            vec![3u32.into_value_and_type()],
        )
        .await
        .unwrap();

    signal_tx.send(()).unwrap();

    drop(executor);
    let executor = start(deps, &context).await.unwrap();

    signal_tx.send(()).unwrap();

    let s4 = executor
        .invoke_and_await(
            &worker_id,
            "golem:it-exports/golem-it-api.{lazy-pollable-test().test}",
            vec![3u32.into_value_and_type()],
        )
        .await
        .unwrap();

    executor.check_oplog_is_queryable(&worker_id).await;
    http_server.abort();

    assert_eq!(s1, vec![Value::String("chunk-1-0\n".to_string())]);
    assert_eq!(s2, vec![Value::String("chunk-1-1\n".to_string())]);
    assert_eq!(s3, vec![Value::String("chunk-1-2\n".to_string())]);
    assert_eq!(s4, vec![Value::String("chunk-3-0\n".to_string())]);
}
