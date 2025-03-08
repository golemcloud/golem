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

use crate::Tracing;
use assert2::check;
use axum::routing::post;
use axum::{Json, Router};
use golem_common::model::component_metadata::{DynamicLinkedInstance, DynamicLinkedWasmRpc};
use golem_test_framework::config::EnvBasedTestDependencies;
use golem_test_framework::dsl::TestDslUnsafe;
use serde_json::Value;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use test_r::{inherit_test_dep, test, timeout};

inherit_test_dep!(Tracing);
inherit_test_dep!(EnvBasedTestDependencies);

#[test]
#[tracing::instrument]
#[timeout(120000)]
async fn invocation_context_test(deps: &EnvBasedTestDependencies, _tracing: &Tracing) {
    let host_http_port = 8588;

    let contexts = Arc::new(Mutex::new(Vec::new()));
    let contexts_clone = contexts.clone();

    let http_server = tokio::spawn(async move {
        let route = Router::new().route(
            "/invocation-context",
            post(move |body: Json<Value>| async move {
                contexts_clone.lock().unwrap().push(body.0);
                "ok"
            }),
        );

        let listener = tokio::net::TcpListener::bind(
            format!("0.0.0.0:{}", host_http_port)
                .parse::<SocketAddr>()
                .unwrap(),
        )
        .await
        .unwrap();
        axum::serve(listener, route).await.unwrap();
    });

    let mut env = HashMap::new();
    env.insert("PORT".to_string(), host_http_port.to_string());

    let component_id = deps
        .component("golem_ictest")
        .with_dynamic_linking(&[(
            "golem:ictest-client/golem-ictest-client",
            DynamicLinkedInstance::WasmRpc(DynamicLinkedWasmRpc {
                target_interface_name: HashMap::from_iter(vec![(
                    "golem-ictest-api".to_string(),
                    "golem:ictest-exports/golem-ictest-api".to_string(),
                )]),
            }),
        )])
        .store()
        .await;
    let worker_id = deps
        .start_worker_with(&component_id, "w1", vec![], env.clone())
        .await;

    let result = deps
        .invoke_and_await(
            &worker_id,
            "golem:ictest-exports/golem-ictest-api.{test1}",
            vec![],
        )
        .await;

    let start = std::time::Instant::now();
    loop {
        let contexts = contexts.lock().unwrap();
        if contexts.len() == 3 {
            break;
        }
        drop(contexts);

        if start.elapsed().as_secs() > 30 {
            check!(false, "Timeout waiting for contexts");
        }
        tokio::time::sleep(std::time::Duration::from_millis(1000)).await;
    }

    let dump: Vec<_> = contexts.lock().unwrap().drain(..).collect();
    println!("{:#?}", dump);

    http_server.abort();

    // TODO: invoke 'test1' through Rib with custom invocation context
    // TODO: assertions

    check!(result.is_ok());
}
