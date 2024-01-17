use assert2::assert;
use serde_json::{Map, Value};

use std::path::Path;

use crate::common;

#[tokio::test]
async fn zig_example_1() {
    let mut executor = common::start().await.unwrap();

    let template_id =
        executor.store_template(Path::new("../test-templates/zig-1.wasm"));
    let worker_id = executor.start_worker(&template_id, "zig-1").await;

    let result = executor
        .invoke_and_await_stdio(
            &worker_id,
            "wasi:cli/run@0.2.0-rc-2023-11-10/run",
            Value::Number(1234.into()),
        )
        .await;

    drop(executor);

    assert!(result == Ok(Value::Number(2468.into())))
}

#[tokio::test]
async fn zig_example_2() {
    let mut executor = common::start().await.unwrap();

    let template_id =
        executor.store_template(Path::new("../test-templates/zig-2.wasm"));
    let worker_id = executor.start_worker(&template_id, "zig-2").await;
    let rx = executor.capture_output(&worker_id).await;

    let _ = executor
        .invoke_and_await_stdio_eventloop(
            &worker_id,
            "wasi:cli/run@0.2.0-rc-2023-11-10/run",
            Value::Object(Map::from_iter([(
                "add".to_string(),
                Value::Number(10.into()),
            )])),
        )
        .await
        .expect("invoke_and_await_stdio_eventloop 1");
    let _ = executor
        .invoke_and_await_stdio_eventloop(
            &worker_id,
            "wasi:cli/run@0.2.0-rc-2023-11-10/run",
            Value::Object(Map::from_iter([(
                "add".to_string(),
                Value::Number(1.into()),
            )])),
        )
        .await
        .expect("invoke_and_await_stdio_eventloop 2");
    let response = executor
        .invoke_and_await_stdio_eventloop(
            &worker_id,
            "wasi:cli/run@0.2.0-rc-2023-11-10/run",
            Value::Object(Map::from_iter([(
                "get".to_string(),
                Value::Object(Map::new()),
            )])),
        )
        .await;

    drop(executor);
    drop(rx);

    assert!(response == Ok(Value::Number(11.into())))
}
