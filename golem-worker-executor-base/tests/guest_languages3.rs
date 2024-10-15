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

use crate::common::{start, TestContext};
use crate::{LastUniqueId, Tracing, WorkerExecutorTestDependencies};
use assert2::{check, let_assert};
use chrono::Datelike;
use golem_test_framework::dsl::{events_to_lines, log_event_to_string, TestDslUnsafe};
use golem_wasm_rpc::Value;
use std::collections::HashMap;
use std::time::Duration;

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

    let component_id = executor.store_component("js-1").await;
    let worker_id = executor.start_worker(&component_id, "js-1").await;

    let mut rx = executor.capture_output(&worker_id).await;

    let start = chrono::Utc::now().timestamp_millis() as u64;

    let result = executor
        .invoke_and_await(
            &worker_id,
            "hello",
            vec![Value::String("JavaScript component".to_string())],
        )
        .await
        .unwrap();

    let end = chrono::Utc::now().timestamp_millis() as u64;

    tokio::time::sleep(Duration::from_secs(5)).await;
    let mut events = vec![];
    rx.recv_many(&mut events, 100).await;

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

    let component_id = executor.store_component("js-2").await;
    let worker_id = executor.start_worker(&component_id, "js-2").await;

    let _ = executor
        .invoke_and_await(&worker_id, "golem:it/api.{add}", vec![Value::U64(5)])
        .await
        .unwrap();

    let _ = executor
        .invoke_and_await(&worker_id, "golem:it/api.{add}", vec![Value::U64(6)])
        .await
        .unwrap();

    let result = executor
        .invoke_and_await(&worker_id, "golem:it/api.{get}", vec![])
        .await
        .unwrap();

    drop(executor);

    check!(result == vec![Value::U64(11)]);
}

#[test]
#[tracing::instrument]
async fn csharp_example_1(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await.unwrap();

    let component_id = executor.store_component("csharp-1").await;
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

    tokio::time::sleep(Duration::from_secs(5)).await;
    let lines = events_to_lines(&mut rx).await;

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
