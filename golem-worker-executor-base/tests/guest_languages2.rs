use crate::common::{start, TestContext};
use assert2::{check, let_assert};
use chrono::Datelike;
use golem_test_framework::dsl::{events_to_lines, TestDsl};
use golem_wasm_rpc::Value;
use std::time::Duration;

#[tokio::test]
#[tracing::instrument]
async fn javascript_example_3() {
    let context = TestContext::new();
    let executor = start(&context).await.unwrap();

    let component_id = executor.store_component("js-3").await;
    let worker_id = executor.start_worker(&component_id, "js-3").await;

    let timeout_time = 1000;
    // Invoke_and_await will wait for the timeout to be finished.
    let result_set = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{set-timeout}",
            vec![Value::U64(timeout_time)],
        )
        .await
        .unwrap();

    let result_get = executor
        .invoke_and_await(&worker_id, "golem:it/api.{get}", vec![])
        .await
        .unwrap();

    drop(executor);

    let_assert!(Some(Value::U64(start)) = result_set.into_iter().next());
    let_assert!(Some(Value::U64(end)) = result_get.into_iter().next());

    check!(end > start, "End time is not greater than start time");

    let total_time = end - start;

    check!(total_time >= timeout_time);
    check!(total_time < timeout_time + 100);
}

#[tokio::test]
#[tracing::instrument]
async fn javascript_example_4() {
    let context = TestContext::new();
    let executor = start(&context).await.unwrap();

    let component_id = executor.store_component("js-4").await;
    let worker_id = executor.start_worker(&component_id, "js-4").await;

    let result = executor
        .invoke_and_await(&worker_id, "golem:it/api.{create-promise}", vec![])
        .await
        .unwrap();

    drop(executor);

    let_assert!(Some(Value::Record(_)) = result.into_iter().next());
}

#[tokio::test]
#[tracing::instrument]
async fn python_example_1() {
    let context = TestContext::new();
    let executor = start(&context).await.unwrap();

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

#[tokio::test]
#[tracing::instrument]
async fn swift_example_1() {
    let context = TestContext::new();
    let executor = start(&context).await.unwrap();

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
