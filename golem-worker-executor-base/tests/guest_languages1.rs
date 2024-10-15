// Copyright 2024 Golem Cloud
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

use test_r::{inherit_test_dep, test};

use assert2::{assert, check};
use std::collections::HashMap;
use std::net::SocketAddr;

use chrono::Datelike;
use golem_test_framework::dsl::{log_event_to_string, TestDslUnsafe};
use golem_wasm_rpc::Value;
use http_02::{Response, StatusCode};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tonic::transport::Body;
use warp::Filter;

use crate::common::{start, TestContext};
use crate::{LastUniqueId, Tracing, WorkerExecutorTestDependencies};

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

    let component_id = executor.store_component("zig-3").await;
    let worker_id = executor.start_worker(&component_id, "zig-3").await;

    let _ = executor
        .invoke_and_await(&worker_id, "golem:it/api.{add}", vec![Value::U64(10)])
        .await
        .unwrap();
    let _ = executor
        .invoke_and_await(&worker_id, "golem:it/api.{add}", vec![Value::U64(11)])
        .await
        .unwrap();
    let result = executor
        .invoke_and_await(&worker_id, "golem:it/api.{get}", vec![])
        .await
        .unwrap();

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

    let component_id = executor.store_component("tinygo-wasi").await;
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
            vec![Value::String("Hello Go-lem".to_string())],
        )
        .await
        .unwrap();

    tokio::time::sleep(Duration::from_secs(5)).await;
    let mut events = vec![];
    rx.recv_many(&mut events, 100).await;

    drop(executor);

    assert!(events.len() >= 4);

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
    let http_host_port = context.host_http_port();
    let http_server = tokio::spawn(async move {
        let route = warp::path("post-example")
            .and(warp::post())
            .and(warp::header::optional::<String>("X-Test"))
            .and(warp::body::bytes())
            .map(move |header: Option<String>, body: bytes::Bytes| {
                let body_str = String::from_utf8(body.to_vec()).unwrap();
                {
                    let mut capture = captured_body_clone.lock().unwrap();
                    *capture = Some(body_str.clone());
                    println!("captured body: {}", body_str);
                }
                Response::builder()
                    .status(StatusCode::OK)
                    .body(Body::from(format!(
                        "{{ \"percentage\" : 0.25, \"message\": \"response message {}\" }}",
                        header.unwrap_or("no X-Test header".to_string()),
                    )))
                    .unwrap()
            });

        warp::serve(route)
            .run(
                format!("0.0.0.0:{}", http_host_port)
                    .parse::<SocketAddr>()
                    .unwrap(),
            )
            .await;
    });

    let component_id = executor.store_component("tinygo-wasi-http").await;
    let mut env = HashMap::new();
    env.insert("PORT".to_string(), context.host_http_port().to_string());

    let worker_id = executor
        .start_worker_with(&component_id, "tinygo-wasi-http-1", vec![], env)
        .await;

    let result = executor
        .invoke_and_await(
            &worker_id,
            "example1",
            vec![Value::String("hello tinygo!".to_string())],
        )
        .await
        .unwrap();

    let captured_body = captured_body.lock().unwrap().clone().unwrap();

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

    let component_id = executor.store_component("grain-1").await;
    let worker_id = executor.start_worker(&component_id, "grain-1").await;

    let mut rx = executor.capture_output(&worker_id).await;

    let _result = executor
        .invoke_and_await(&worker_id, "wasi:cli/run@0.2.0.{run}", vec![])
        .await
        .unwrap();

    tokio::time::sleep(Duration::from_secs(5)).await;
    let mut events = vec![];
    rx.recv_many(&mut events, 100).await;

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

#[test]
#[tracing::instrument]
async fn java_example_1(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await.unwrap();

    let component_id = executor.store_component("java-1").await;
    let worker_id = executor.start_worker(&component_id, "java-1").await;

    let mut rx = executor.capture_output(&worker_id).await;

    let result = executor
        .invoke_and_await(
            &worker_id,
            "run-example1",
            vec![Value::String("Hello Golem!".to_string())],
        )
        .await
        .unwrap();

    tokio::time::sleep(Duration::from_secs(5)).await;
    let mut events = vec![];
    rx.recv_many(&mut events, 100).await;

    drop(executor);

    let first_line = log_event_to_string(&events[1]);

    check!(first_line == "Hello world, input is Hello Golem!\n".to_string());
    check!(result == vec![Value::U32("Hello Golem!".len() as u32)]);
}

#[test]
#[tracing::instrument]
async fn java_shopping_cart(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await.unwrap();

    let component_id = executor.store_component("java-2").await;
    let worker_id = executor.start_worker(&component_id, "java-2").await;

    let _ = executor
        .invoke_and_await(
            &worker_id,
            "initialize-cart",
            vec![Value::String("test-user-1".to_string())],
        )
        .await;

    let _ = executor
        .invoke_and_await(
            &worker_id,
            "add-item",
            vec![Value::Record(vec![
                Value::String("G1000".to_string()),
                Value::String("Golem T-Shirt M".to_string()),
                Value::F32(100.0),
                Value::U32(5),
            ])],
        )
        .await;

    let _ = executor
        .invoke_and_await(
            &worker_id,
            "add-item",
            vec![Value::Record(vec![
                Value::String("G1001".to_string()),
                Value::String("Golem Cloud Subscription 1y".to_string()),
                Value::F32(999999.0),
                Value::U32(1),
            ])],
        )
        .await;

    let _ = executor
        .invoke_and_await(
            &worker_id,
            "add-item",
            vec![Value::Record(vec![
                Value::String("G1002".to_string()),
                Value::String("Mud Golem".to_string()),
                Value::F32(11.0),
                Value::U32(10),
            ])],
        )
        .await;

    let _ = executor
        .invoke_and_await(
            &worker_id,
            "update-item-quantity",
            vec![Value::String("G1002".to_string()), Value::U32(20)],
        )
        .await;

    let contents = executor
        .invoke_and_await(&worker_id, "get-cart-contents", vec![])
        .await;

    let _ = executor
        .invoke_and_await(&worker_id, "checkout", vec![])
        .await;

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

    let component_id = executor.store_component("c-1").await;
    let worker_id = executor.start_worker(&component_id, "c-1").await;

    let mut rx = executor.capture_output(&worker_id).await;

    let result = executor
        .invoke_and_await(&worker_id, "run", vec![])
        .await
        .unwrap();

    tokio::time::sleep(Duration::from_secs(5)).await;
    let mut events = vec![];
    rx.recv_many(&mut events, 100).await;

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

    let component_id = executor.store_component("c-1").await;
    let worker_id = executor.start_worker(&component_id, "c-2").await;

    let mut rx = executor.capture_output(&worker_id).await;

    let _ = executor
        .invoke_and_await(
            &worker_id,
            "print",
            vec![Value::String("Hello C!".to_string())],
        )
        .await
        .unwrap();

    tokio::time::sleep(Duration::from_secs(5)).await;
    let mut events = vec![];
    rx.recv_many(&mut events, 100).await;

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

    let component_id = executor.store_component("large-initial-memory").await;
    let worker_id = executor
        .start_worker(&component_id, "large-initial-memory")
        .await;

    executor.log_output(&worker_id).await;

    let result = executor
        .invoke_and_await(&worker_id, "run", vec![])
        .await
        .unwrap();

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

    let component_id = executor.store_component("large-dynamic-memory").await;
    let worker_id = executor
        .start_worker(&component_id, "large-dynamic-memory")
        .await;

    executor.log_output(&worker_id).await;

    let result = executor
        .invoke_and_await(&worker_id, "run", vec![])
        .await
        .unwrap();

    drop(executor);

    check!(result == vec![Value::U64(0)]);
}
