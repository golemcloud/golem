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
use golem_test_framework::dsl::{events_to_lines, TestDslUnsafe};
use golem_wasm_rpc::Value;
use std::time::Duration;

inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(LastUniqueId);
inherit_test_dep!(Tracing);

#[test]
#[tracing::instrument]
async fn javascript_example_3(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await.unwrap();

    let component_id = executor.store_component("js-3").await;
    let worker_id = executor.start_worker(&component_id, "js-3").await;

    let result_fetch_get = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{fetch-get}",
            vec![Value::String("https://google.com".to_string())],
        )
        .await
        .unwrap();

    drop(executor);

    let_assert!(Some(Value::String(result_body)) = result_fetch_get.into_iter().next());

    assert!(result_body.contains("google.com"));
}

#[test]
#[tracing::instrument]
async fn javascript_example_4(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await.unwrap();

    let component_id = executor.store_component("js-4").await;
    let worker_id = executor.start_worker(&component_id, "js-4").await;

    let result = executor
        .invoke_and_await(&worker_id, "golem:it/api.{create-promise}", vec![])
        .await
        .unwrap();

    drop(executor);

    let_assert!(Some(Value::Record(_)) = result.into_iter().next());
}

#[test]
#[tracing::instrument]
async fn python_example_1(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await.unwrap();

    let component_id = executor.store_component("python-1").await;
    let worker_id = executor.start_worker(&component_id, "python-1").await;

    let _ = executor
        .invoke_and_await(&worker_id, "golem:it/api.{add}", vec![Value::U64(3)])
        .await
        .unwrap();

    let _ = executor
        .invoke_and_await(&worker_id, "golem:it/api.{add}", vec![Value::U64(8)])
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
async fn swift_example_1(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await.unwrap();

    let component_id = executor.store_component("swift-1").await;
    let worker_id = executor.start_worker(&component_id, "swift-1").await;

    let mut rx = executor.capture_output(&worker_id).await;

    let _ = executor
        .invoke_and_await(&worker_id, "wasi:cli/run@0.2.0.{run}", vec![])
        .await
        .unwrap();

    tokio::time::sleep(Duration::from_secs(5)).await;
    let lines = events_to_lines(&mut rx).await;

    drop(executor);

    let now = chrono::Local::now();
    let year = now.year();

    check!(lines[0] == "Hello world!".to_string());
    check!(lines[1] == year.to_string());
}
