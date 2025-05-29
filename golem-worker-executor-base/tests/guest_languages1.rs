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

use crate::common::{start, TestContext};
use crate::{LastUniqueId, Tracing, WorkerExecutorTestDependencies};
use assert2::{assert, check};
use axum::response::IntoResponse;
use axum::routing::post;
use axum::Router;
use bytes::Bytes;
use chrono::Datelike;
use golem_test_framework::dsl::{log_event_to_string, TestDslUnsafe};
use golem_wasm_rpc::{IntoValueAndType, Value};
use http::HeaderMap;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use test_r::{inherit_test_dep, test};
use tracing::{info, Instrument};

inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(LastUniqueId);
inherit_test_dep!(Tracing);

#[test]
#[tracing::instrument]
async fn zig_example_3(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await.unwrap();

    let component_id = executor.component("zig-3").store().await;
    let worker_id = executor.start_worker(&component_id, "zig-3").await;

    let _ = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{add}",
            vec![10u64.into_value_and_type()],
        )
        .await
        .unwrap();
    let _ = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{add}",
            vec![11u64.into_value_and_type()],
        )
        .await
        .unwrap();
    let result = executor
        .invoke_and_await(&worker_id, "golem:it/api.{get}", vec![])
        .await
        .unwrap();

    executor.check_oplog_is_queryable(&worker_id).await;

    drop(executor);

    assert_eq!(result, vec![Value::U64(21)])
}

#[test]
#[tracing::instrument]
async fn tinygo_example(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await.unwrap();

    let component_id = executor.component("tinygo-wasi").store().await;
    let worker_id = executor
        .start_worker_with(
            &component_id,
            "tinygo-wasi-1",
            vec!["arg-1".to_string(), "arg-2".to_string()],
            HashMap::from([("ENV_VAR_1".to_string(), "ENV_VAR_VALUE_1".to_string())]),
        )
        .await;

    let mut rx = executor.capture_output(&worker_id).await;

    let result = executor
        .invoke_and_await(
            &worker_id,
            "example1",
            vec!["Hello Go-lem".into_value_and_type()],
        )
        .await
        .unwrap();

    let mut events = vec![];
    let start = Instant::now();
    while events.len() < 5 && start.elapsed() < Duration::from_secs(5) {
        if let Some(event) = rx.recv().await {
            events.push(event);
        } else {
            break;
        }
    }

    executor.check_oplog_is_queryable(&worker_id).await;

    drop(executor);

    assert!(events.len() >= 5);

    let first_line = log_event_to_string(&events[1]);
    let second_line = log_event_to_string(&events[2]);
    let parts: Vec<_> = second_line.split(' ').collect();
    let last_part = parts.last().unwrap().trim();
    let now = chrono::Local::now();
    let year = now.year();
    let third_line = log_event_to_string(&events[3]);
    let fourth_line = log_event_to_string(&events[4]);

    check!(first_line == "Hello Go-lem\n".to_string());
    check!(second_line.starts_with(&format!("test {year}")));
    check!(third_line.contains("arg-1 arg-2"));
    check!(fourth_line.contains("ENV_VAR_1=ENV_VAR_VALUE_1"));
    check!(fourth_line.contains("GOLEM_WORKER_NAME=tinygo-wasi-1"));
    check!(fourth_line.contains("GOLEM_COMPONENT_ID="));
    check!(fourth_line.contains("GOLEM_COMPONENT_VERSION=0"));
    check!(result == vec!(Value::S32(last_part.parse::<i32>().unwrap())));
}

#[test]
#[tracing::instrument]
async fn tinygo_http_client(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await.unwrap();

    let captured_body: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    let captured_body_clone = captured_body.clone();

    async fn request_handler(
        captured_body_clone: Arc<Mutex<Option<String>>>,
        headers: HeaderMap,
        body: Bytes,
    ) -> impl IntoResponse {
        let body_str = String::from_utf8(body.to_vec()).unwrap();
        {
            let mut capture = captured_body_clone.lock().unwrap();
            *capture = Some(body_str.clone());
            info!("captured body: {}", body_str);
        }
        let header = headers.get("X-Test");
        format!(
            "{{ \"percentage\" : 0.25, \"message\": \"response message {}\" }}",
            header
                .map(|h| h.to_str().unwrap().to_string())
                .unwrap_or("no X-Test header".to_string()),
        )
    }

    let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await.unwrap();

    let host_http_port = listener.local_addr().unwrap().port();

    let http_server = tokio::spawn(
        async move {
            let route = Router::new().route(
                "/post-example",
                post(|headers, body| request_handler(captured_body_clone, headers, body)),
            );

            axum::serve(listener, route).await.unwrap();
        }
        .in_current_span(),
    );

    let component_id = executor.component("tinygo-wasi-http").store().await;
    let mut env = HashMap::new();
    env.insert("PORT".to_string(), host_http_port.to_string());

    let worker_id = executor
        .start_worker_with(&component_id, "tinygo-wasi-http-1", vec![], env)
        .await;

    let result = executor
        .invoke_and_await(
            &worker_id,
            "example1",
            vec!["hello tinygo!".into_value_and_type()],
        )
        .await
        .unwrap();

    let captured_body = captured_body.lock().unwrap().clone().unwrap();

    executor.check_oplog_is_queryable(&worker_id).await;

    drop(executor);
    http_server.abort();

    check!(
        result
            == vec![Value::String(
                "200 percentage: 0.250000, message: response message no X-Test header".to_string()
            )]
    );
    check!(
        captured_body
            == "{\"Name\":\"Something\",\"Amount\":42,\"Comments\":[\"Hello\",\"World\"]}"
                .to_string()
    );
}

#[test]
#[tracing::instrument]
#[ignore] // Building with the latest Grain compiler fails in "WebAssembly Translation error"
async fn grain_example_1(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await.unwrap();

    let component_id = executor.component("grain-1").store().await;
    let worker_id = executor.start_worker(&component_id, "grain-1").await;

    let mut rx = executor.capture_output(&worker_id).await;

    let _result = executor
        .invoke_and_await(&worker_id, "wasi:cli/run@0.2.0.{run}", vec![])
        .await
        .unwrap();

    tokio::time::sleep(Duration::from_secs(5)).await;
    let mut events = vec![];
    rx.recv_many(&mut events, 100).await;

    executor.check_oplog_is_queryable(&worker_id).await;

    drop(executor);

    let first_line = log_event_to_string(&events[0]);
    let second_line = log_event_to_string(&events[1]);
    let third_line = log_event_to_string(&events[2]);

    let now = chrono::Local::now();
    let epoch = now.timestamp_nanos_opt().unwrap();
    let hour = 3_600_000_000_000;

    check!(first_line == "hello world".to_string());
    check!(second_line.parse::<i64>().is_ok());
    check!(third_line.parse::<i64>().unwrap() > (epoch - hour));
    check!(third_line.parse::<i64>().unwrap() < (epoch + hour));
}

// NOTE: disabled for now:
//   - rebuilt with teavm 0.27.0 and 0.28.0, and both is failing with (previously used _some_ 0.27.0-SNAPSHOT)
//   - with both it is failing with: meth_otr_ExceptionHandling_throwException in !meth_oti_Memory_realloc
#[ignore]
#[test]
#[tracing::instrument]
async fn java_example_1(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await.unwrap();

    let component_id = executor.component("java-1").store().await;
    let worker_id = executor.start_worker(&component_id, "java-1").await;

    let mut rx = executor.capture_output(&worker_id).await;

    let result = executor
        .invoke_and_await(
            &worker_id,
            "run-example1",
            vec!["Hello Golem!".into_value_and_type()],
        )
        .await
        .unwrap();

    let mut events = vec![];
    let start = Instant::now();
    while events.len() < 2 && start.elapsed() < Duration::from_secs(5) {
        if let Some(event) = rx.recv().await {
            events.push(event);
        } else {
            break;
        }
    }

    executor.check_oplog_is_queryable(&worker_id).await;

    drop(executor);

    let first_line = log_event_to_string(&events[1]);

    check!(first_line == "Hello world, input is Hello Golem!\n".to_string());
    check!(result == vec![Value::U32("Hello Golem!".len() as u32)]);
}

// NOTE: disabled for now:
//   - rebuilt with teavm 0.27.0 and 0.28.0, and both is failing with (previously used _some_ 0.27.0-SNAPSHOT)
//   - with both it is failing with: meth_otr_ExceptionHandling_throwException in !meth_oti_Memory_realloc
#[ignore]
#[test]
#[tracing::instrument]
async fn java_shopping_cart(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await.unwrap();

    let component_id = executor.component("java-2").store().await;
    let worker_id = executor.start_worker(&component_id, "java-2").await;

    let _ = executor
        .invoke_and_await(
            &worker_id,
            "initialize-cart",
            vec!["test-user-1".into_value_and_type()],
        )
        .await;

    let _ = executor
        .invoke_and_await(
            &worker_id,
            "add-item",
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
            "add-item",
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
            "add-item",
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
            "update-item-quantity",
            vec!["G1002".into_value_and_type(), 20u32.into_value_and_type()],
        )
        .await;

    let contents = executor
        .invoke_and_await(&worker_id, "get-cart-contents", vec![])
        .await;

    let _ = executor
        .invoke_and_await(&worker_id, "checkout", vec![])
        .await;

    executor.check_oplog_is_queryable(&worker_id).await;

    drop(executor);

    assert_eq!(
        contents,
        Ok(vec![Value::List(vec![
            Value::Record(vec![
                Value::String("G1000".to_string()),
                Value::String("Golem T-Shirt M".to_string()),
                Value::F32(100.0),
                Value::U32(5),
            ]),
            Value::Record(vec![
                Value::String("G1001".to_string()),
                Value::String("Golem Cloud Subscription 1y".to_string()),
                Value::F32(999999.0),
                Value::U32(1),
            ]),
            Value::Record(vec![
                Value::String("G1002".to_string()),
                Value::String("Mud Golem".to_string()),
                Value::F32(11.0),
                Value::U32(20),
            ]),
        ])])
    )
}

#[test]
#[tracing::instrument]
async fn c_example_1(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await.unwrap();

    let component_id = executor.component("c-1").store().await;
    let worker_id = executor.start_worker(&component_id, "c-1").await;

    let mut rx = executor.capture_output(&worker_id).await;

    let result = executor
        .invoke_and_await(&worker_id, "run", vec![])
        .await
        .unwrap();

    let mut events = vec![];
    let start = Instant::now();
    while events.len() < 2 && start.elapsed() < Duration::from_secs(5) {
        if let Some(event) = rx.recv().await {
            events.push(event);
        } else {
            break;
        }
    }

    executor.check_oplog_is_queryable(&worker_id).await;

    drop(executor);

    let first_line = log_event_to_string(&events[1]);

    check!(first_line == "Hello World!\n".to_string());
    check!(result == vec![Value::S32(100)]);
}

#[test]
#[tracing::instrument]
async fn c_example_2(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await.unwrap();

    let component_id = executor.component("c-1").store().await;
    let worker_id = executor.start_worker(&component_id, "c-2").await;

    let mut rx = executor.capture_output(&worker_id).await;

    let _ = executor
        .invoke_and_await(&worker_id, "print", vec!["Hello C!".into_value_and_type()])
        .await
        .unwrap();

    let mut events = vec![];
    let start = Instant::now();
    while events.len() < 2 && start.elapsed() < Duration::from_secs(5) {
        if let Some(event) = rx.recv().await {
            events.push(event);
        } else {
            break;
        }
    }

    executor.check_oplog_is_queryable(&worker_id).await;

    drop(executor);

    let first_line = log_event_to_string(&events[1]);
    let now = chrono::Local::now();
    let year = now.year();

    check!(first_line == format!("Hello C! {year}"));
}

#[test]
#[tracing::instrument]
#[ignore]
async fn c_example_3(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await.unwrap();

    let component_id = executor.component("large-initial-memory").store().await;
    let worker_id = executor
        .start_worker(&component_id, "large-initial-memory")
        .await;

    executor.log_output(&worker_id).await;

    let result = executor
        .invoke_and_await(&worker_id, "run", vec![])
        .await
        .unwrap();

    executor.check_oplog_is_queryable(&worker_id).await;

    drop(executor);

    check!(result == vec![Value::U64(536870912)]);
}

#[test]
#[tracing::instrument]
#[ignore]
async fn c_example_4(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await.unwrap();

    let component_id = executor.component("large-dynamic-memory").store().await;
    let worker_id = executor
        .start_worker(&component_id, "large-dynamic-memory")
        .await;

    executor.log_output(&worker_id).await;

    let result = executor
        .invoke_and_await(&worker_id, "run", vec![])
        .await
        .unwrap();

    executor.check_oplog_is_queryable(&worker_id).await;

    drop(executor);

    check!(result == vec![Value::U64(0)]);
}
