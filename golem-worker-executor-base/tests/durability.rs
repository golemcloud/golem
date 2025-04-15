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
use axum::routing::get;
use axum::Router;
use golem_wasm_rpc::IntoValueAndType;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use serde::Deserialize;
use test_r::{inherit_test_dep, test};
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
        payload: String
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
            vec!["a".into_value_and_type()],
        )
        .await
        .unwrap();

    executor.check_oplog_is_queryable(&worker_id).await;

    drop(executor);
    http_server.abort();

    println!("Result 1: {:?}", result1);
    println!("Result 2: {:?}", result2);
}
