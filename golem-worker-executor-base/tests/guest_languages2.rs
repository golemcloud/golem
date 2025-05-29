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

use test_r::{inherit_test_dep, test, timeout};

use crate::common::{start, TestContext};
use crate::{LastUniqueId, Tracing, WorkerExecutorTestDependencies};
use assert2::{check, let_assert};
use chrono::Datelike;
use golem_test_framework::dsl::{events_to_lines, TestDslUnsafe};
use golem_wasm_rpc::{IntoValueAndType, Value};
use std::time::{Duration, Instant};

inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(LastUniqueId);
inherit_test_dep!(Tracing);

#[test]
#[tracing::instrument]
#[timeout(300_000)]
async fn javascript_example_3(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await.unwrap();

    let component_id = executor.component("js-3").store().await;
    let worker_id = executor.start_worker(&component_id, "js-3").await;

    let result_fetch_get = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{fetch-get}",
            vec!["https://google.com".into_value_and_type()],
        )
        .await
        .unwrap();

    executor.check_oplog_is_queryable(&worker_id).await;

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

    let component_id = executor.component("js-4").store().await;
    let worker_id = executor.start_worker(&component_id, "js-4").await;

    let result = executor
        .invoke_and_await(&worker_id, "golem:it/api.{create-promise}", vec![])
        .await
        .unwrap();

    executor.check_oplog_is_queryable(&worker_id).await;

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

    let component_id = executor.component("python-1").store().await;
    let worker_id = executor.start_worker(&component_id, "python-1").await;

    let _ = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{add}",
            vec![3u64.into_value_and_type()],
        )
        .await
        .unwrap();

    let _ = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{add}",
            vec![8u64.into_value_and_type()],
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
async fn swift_example_1(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await.unwrap();

    let component_id = executor.component("swift-1").store().await;
    let worker_id = executor.start_worker(&component_id, "swift-1").await;

    let mut rx = executor.capture_output(&worker_id).await;

    let _ = executor
        .invoke_and_await(&worker_id, "wasi:cli/run@0.2.0.{run}", vec![])
        .await
        .unwrap();

    let mut lines = Vec::new();
    let start = Instant::now();

    while lines.len() < 2 && start.elapsed() < Duration::from_secs(5) {
        lines.extend(events_to_lines(&mut rx).await);
    }

    executor.check_oplog_is_queryable(&worker_id).await;

    drop(executor);

    let now = chrono::Local::now();
    let year = now.year();

    check!(lines[0] == "Hello world!".to_string());
    check!(lines[1] == year.to_string());
}
