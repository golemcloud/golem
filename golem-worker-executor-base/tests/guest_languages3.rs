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

use test_r::{inherit_test_dep, test};

use crate::common::{start, TestContext};
use crate::{LastUniqueId, Tracing, WorkerExecutorTestDependencies};
use assert2::{check, let_assert};
use axum::response::IntoResponse;
use axum::routing::post;
use axum::Router;
use bytes::Bytes;
use chrono::Datelike;
use golem_test_framework::dsl::{events_to_lines, log_event_to_string, TestDslUnsafe};
use golem_wasm_rpc::{IntoValueAndType, Value};
use http::HeaderMap;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tracing::{info, Instrument};

inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(LastUniqueId);
inherit_test_dep!(Tracing);

#[test]
#[tracing::instrument]
async fn javascript_example_1(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await.unwrap();

    let component_id = executor.component("js-1").store().await;
    let worker_id = executor.start_worker(&component_id, "js-1").await;

    let mut rx = executor.capture_output(&worker_id).await;

    let start = chrono::Utc::now().timestamp_millis() as u64;

    let result = executor
        .invoke_and_await(
            &worker_id,
            "hello",
            vec!["JavaScript component".into_value_and_type()],
        )
        .await
        .unwrap();

    let end = chrono::Utc::now().timestamp_millis() as u64;

    let mut events = vec![];
    let start_time = Instant::now();
    while events.len() < 2 && start_time.elapsed() < Duration::from_secs(5) {
        if let Some(event) = rx.recv().await {
            events.push(event);
        } else {
            break;
        }
    }

    executor.check_oplog_is_queryable(&worker_id).await;

    drop(executor);

    let_assert!(Some(Value::Record(record_values)) = result.into_iter().next());

    let_assert!(
        [
            Value::F64(_random),
            Value::String(random_uuid),
            Value::U64(js_time),
            Value::U64(wasi_time)
        ] = record_values.as_slice(),
    );

    check!(uuid::Uuid::parse_str(random_uuid).is_ok(), "Invalid UUID");

    // validating that Date.now() is working
    check!(*js_time >= start && *js_time <= end, "Invalid js time");
    // validating that directly calling wasi:clocks/wall-clock/now works
    check!(
        *wasi_time >= start && *wasi_time <= end,
        "Invalid wasi Time"
    );

    let first_line = log_event_to_string(&events[1]);
    let parts = first_line.split(' ').collect::<Vec<_>>();

    check!(parts[0] == "Hello");
    check!(parts[1] == "JavaScript");
    check!(parts[2] == "component!");
}

#[test]
#[tracing::instrument]
async fn javascript_example_2(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await.unwrap();

    let component_id = executor.component("js-2").store().await;
    let worker_id = executor.start_worker(&component_id, "js-2").await;

    let _ = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{add}",
            vec![5u64.into_value_and_type()],
        )
        .await
        .unwrap();

    let _ = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{add}",
            vec![6u64.into_value_and_type()],
        )
        .await
        .unwrap();

    let result = executor
        .invoke_and_await(&worker_id, "golem:it/api.{get}", vec![])
        .await
        .unwrap();

    executor.check_oplog_is_queryable(&worker_id).await;

    drop(executor);

    check!(result == vec![Value::U64(11)]);
}

#[test]
#[tracing::instrument]
#[ignore]
async fn csharp_example_1(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await.unwrap();

    let component_id = executor.component("csharp-1").store().await;
    let mut env = HashMap::new();
    env.insert("TEST_ENV".to_string(), "test-value".to_string());
    let worker_id = executor
        .start_worker_with(&component_id, "csharp-1", vec!["test-arg".to_string()], env)
        .await;

    let mut rx = executor.capture_output(&worker_id).await;

    let _result = executor
        .invoke_and_await(&worker_id, "wasi:cli/run@0.2.0.{run}", vec![])
        .await
        .unwrap();

    let mut lines = Vec::new();
    let start = Instant::now();

    while lines.len() < 4 && start.elapsed() < Duration::from_secs(5) {
        lines.extend(events_to_lines(&mut rx).await);
    }

    executor.check_oplog_is_queryable(&worker_id).await;

    drop(executor);

    let now = chrono::Local::now();
    let year = now.year();

    check!(lines[0] == "Hello, World!".to_string());
    check!(lines[1].parse::<i32>().is_ok());
    check!(lines[2] == year.to_string());
    // NOTE: command line argument access is not working currently in dotnet-wasi
    check!(lines[3] == "".to_string());
    check!(lines.contains(&"TEST_ENV: test-value".to_string()));
    check!(lines.contains(&format!("GOLEM_COMPONENT_ID: {component_id}")));
    check!(lines.contains(&"GOLEM_WORKER_NAME: csharp-1".to_string()));
    check!(lines.contains(&"GOLEM_COMPONENT_VERSION: 0".to_string()));
}

#[test]
#[tracing::instrument]
async fn python_http_client(
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
        let header = headers.get("X-Test").unwrap().to_str().unwrap();
        format!("\"test-response: {header}\"")
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

    let component_id = executor
        .component("golem_it_python_http_client")
        .store()
        .await;
    let mut env = HashMap::new();
    env.insert("PORT".to_string(), host_http_port.to_string());

    let worker_id = executor
        .start_worker_with(&component_id, "python-http-client-1", vec![], env)
        .await;

    let result = executor
        .invoke_and_await(
            &worker_id,
            "golem:it-python-http-client-exports/golem-it-python-http-client-api.{run}",
            vec![],
        )
        .await
        .unwrap();

    let captured_body = captured_body.lock().unwrap().clone().unwrap();

    executor.check_oplog_is_queryable(&worker_id).await;

    drop(executor);
    http_server.abort();

    check!(result == vec![Value::String("test-response: test-header".to_string())]);
    check!(captured_body == "\"test-body\"".to_string());
}
