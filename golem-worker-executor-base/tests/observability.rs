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

use assert2::check;
use axum::routing::post;
use axum::{Json, Router};
use golem_wasm_rpc::{IntoValueAndType, Value};
use http::HeaderMap;
use log::info;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use test_r::{inherit_test_dep, test};
use tracing::Instrument;

use crate::common::{start, TestContext};
use crate::{LastUniqueId, Tracing, WorkerExecutorTestDependencies};
use golem_common::model::component_metadata::{DynamicLinkedInstance, DynamicLinkedWasmRpc};
use golem_common::model::oplog::OplogIndex;
use golem_common::model::public_oplog::{ExportedFunctionInvokedParameters, PublicOplogEntry};
use golem_common::model::{IdempotencyKey, WorkerId};
use golem_test_framework::dsl::TestDslUnsafe;

inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(LastUniqueId);
inherit_test_dep!(Tracing);

#[test]
#[tracing::instrument]
async fn get_oplog_1(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await.unwrap();

    let component_id = executor.component("runtime-service").store().await;

    let worker_id = WorkerId {
        component_id,
        worker_name: "getoplog1".to_string(),
    };

    let idempotency_key1 = IdempotencyKey::fresh();
    let idempotency_key2 = IdempotencyKey::fresh();

    let _ = executor
        .invoke_and_await(
            worker_id.clone(),
            "golem:it/api.{generate-idempotency-keys}",
            vec![],
        )
        .await
        .unwrap();
    let _ = executor
        .invoke_and_await_with_key(
            worker_id.clone(),
            &idempotency_key1,
            "golem:it/api.{generate-idempotency-keys}",
            vec![],
        )
        .await
        .unwrap();
    let _ = executor
        .invoke_and_await_with_key(
            worker_id.clone(),
            &idempotency_key2,
            "golem:it/api.{generate-idempotency-keys}",
            vec![],
        )
        .await
        .unwrap();

    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await;

    drop(executor);

    // Whether there is an "enqueued invocation" entry or just directly started invocation
    // depends on timing
    assert!(oplog.len() >= 12 && oplog.len() <= 14);
    assert!(matches!(oplog[0], PublicOplogEntry::Create(_)));
    assert_eq!(
        oplog
            .iter()
            .filter(
                |entry| matches!(entry, PublicOplogEntry::ExportedFunctionInvoked(
        ExportedFunctionInvokedParameters { function_name, .. }
    ) if function_name == "golem:it/api.{generate-idempotency-keys}")
            )
            .count(),
        3
    );
}

#[test]
#[tracing::instrument]
async fn search_oplog_1(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await.unwrap();

    let component_id = executor.component("shopping-cart").store().await;

    let worker_id = WorkerId {
        component_id,
        worker_name: "searchoplog1".to_string(),
    };

    let _ = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{initialize-cart}",
            vec!["test-user-1".into_value_and_type()],
        )
        .await;

    let _ = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{add-item}",
            vec![vec![
                ("product-id", "G1000".into_value_and_type()),
                ("name", "Golem T-Shirt M".into_value_and_type()),
                ("price", 100.0f32.into_value_and_type()),
                ("quantity", 5u32.into_value_and_type()),
            ]
            .into_value_and_type()],
        )
        .await;

    let _ = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{add-item}",
            vec![vec![
                ("product-id", "G1001".into_value_and_type()),
                ("name", "Golem Cloud Subscription 1y".into_value_and_type()),
                ("price", 999999.0f32.into_value_and_type()),
                ("quantity", 1u32.into_value_and_type()),
            ]
            .into_value_and_type()],
        )
        .await;

    let _ = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{add-item}",
            vec![vec![
                ("product-id", "G1002".into_value_and_type()),
                ("name", "Mud Golem".into_value_and_type()),
                ("price", 11.0f32.into_value_and_type()),
                ("quantity", 10u32.into_value_and_type()),
            ]
            .into_value_and_type()],
        )
        .await;

    let _ = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{update-item-quantity}",
            vec!["G1002".into_value_and_type(), 20u32.into_value_and_type()],
        )
        .await;

    let _ = executor
        .invoke_and_await(&worker_id, "golem:it/api.{get-cart-contents}", vec![])
        .await;

    let _ = executor
        .invoke_and_await(&worker_id, "golem:it/api.{checkout}", vec![])
        .await;

    let _oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await;

    let result1 = executor.search_oplog(&worker_id, "G1002").await;

    let result2 = executor.search_oplog(&worker_id, "imported-function").await;

    let result3 = executor
        .search_oplog(&worker_id, "product-id:G1001 OR product-id:G1000")
        .await;

    drop(executor);

    assert_eq!(result1.len(), 4); // two invocations and two log messages
    assert_eq!(result2.len(), 2); // get_preopened_directories, get_random_bytes
    assert_eq!(result3.len(), 2); // two invocations
}

#[test]
#[tracing::instrument]
async fn get_oplog_with_api_changing_updates(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await.unwrap();

    let component_id = executor.component("update-test-v1").unique().store().await;
    let worker_id = executor
        .start_worker(&component_id, "get_oplog_with_api_changing_updates")
        .await;

    let target_version = executor
        .update_component(&component_id, "update-test-v2")
        .await;
    info!("Updated component to version {target_version}");

    let _ = executor
        .invoke_and_await(&worker_id, "golem:component/api.{f3}", vec![])
        .await
        .unwrap();
    let _ = executor
        .invoke_and_await(&worker_id, "golem:component/api.{f3}", vec![])
        .await
        .unwrap();

    executor
        .auto_update_worker(&worker_id, target_version)
        .await;

    let result = executor
        .invoke_and_await(&worker_id, "golem:component/api.{f4}", vec![])
        .await
        .unwrap();

    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await;

    // there might be a pending invocation entry before the update entry. Filter it out to make the test more robust
    let oplog = oplog
        .into_iter()
        .filter(|entry| !matches!(entry, PublicOplogEntry::PendingWorkerInvocation(_)))
        .collect::<Vec<_>>();

    check!(result[0] == Value::U64(11));
    assert_eq!(oplog.len(), 17);
}

#[test]
#[tracing::instrument]
async fn get_oplog_starting_with_updated_component(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await.unwrap();

    let component_id = executor.component("update-test-v1").unique().store().await;
    let target_version = executor
        .update_component(&component_id, "update-test-v2")
        .await;
    info!("Updated component to version {target_version}");

    let worker_id = executor
        .start_worker(&component_id, "get_oplog_starting_with_updated_component")
        .await;
    let result = executor
        .invoke_and_await(&worker_id, "golem:component/api.{f4}", vec![])
        .await
        .unwrap();

    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await;

    check!(result[0] == Value::U64(11));
    assert_eq!(oplog.len(), 3);
}

#[test]
#[tracing::instrument]
#[allow(clippy::await_holding_lock)]
async fn invocation_context_test(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await.unwrap();

    let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await.unwrap();
    let host_http_port = listener.local_addr().unwrap().port();

    let contexts = Arc::new(Mutex::new(Vec::new()));
    let contexts_clone = contexts.clone();

    let traceparents = Arc::new(Mutex::new(Vec::new()));
    let traceparents_clone = traceparents.clone();

    let http_server = tokio::spawn(
        async move {
            let route = Router::new().route(
                "/invocation-context",
                post(
                    move |headers: HeaderMap, body: Json<serde_json::Value>| async move {
                        contexts_clone.lock().unwrap().push(body.0);
                        traceparents_clone
                            .lock()
                            .unwrap()
                            .push(headers.get("traceparent").cloned());
                        "ok"
                    },
                ),
            );

            axum::serve(listener, route).await.unwrap();
        }
        .in_current_span(),
    );

    let mut env = HashMap::new();
    env.insert("PORT".to_string(), host_http_port.to_string());

    let component_id = executor
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
    let worker_id = executor
        .start_worker_with(&component_id, "w1", vec![], env.clone())
        .await;

    let result = executor
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
    info!("{:#?}", dump);

    http_server.abort();
    drop(executor);

    let traceparents = traceparents.lock().unwrap();

    check!(result.is_ok());
    check!(traceparents.len() == 3);
    check!(traceparents.iter().all(|tp| tp.is_some()));

    check!(
        dump[0]
            .as_object()
            .unwrap()
            .get("spans")
            .unwrap()
            .as_array()
            .unwrap()
            .len()
            == 2
    ); // root, invoke-exported-function
    check!(
        dump[1]
            .as_object()
            .unwrap()
            .get("spans")
            .unwrap()
            .as_array()
            .unwrap()
            .len()
            == 5
    ); // + rpc-connection, rpc-invocation, invoke-exported-function
    check!(
        dump[2]
            .as_object()
            .unwrap()
            .get("spans")
            .unwrap()
            .as_array()
            .unwrap()
            .len()
            == 10
    ); // + custom1, custom2, rpc-connection, rpc-invocation, invoke-exported-function
}
