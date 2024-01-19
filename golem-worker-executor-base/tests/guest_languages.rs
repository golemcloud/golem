use assert2::{assert, check};
use serde_json::{Map, Value};
use std::collections::HashMap;
use std::net::SocketAddr;

use chrono::Datelike;
use http::{Response, StatusCode};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tonic::transport::Body;
use warp::Filter;

use crate::common;

#[tokio::test]
async fn zig_example_1() {
    let mut executor = common::start().await.unwrap();

    let template_id = executor.store_template(Path::new("../test-templates/zig-1.wasm"));
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

    let template_id = executor.store_template(Path::new("../test-templates/zig-2.wasm"));
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

#[tokio::test]
async fn tinygo_example() {
    let mut executor = common::start().await.unwrap();

    let template_id = executor.store_template(Path::new("../test-templates/tinygo-wasi.wasm"));
    let worker_id = executor.start_worker(&template_id, "tinygo-wasi-1").await;

    let mut rx = executor.capture_output(&worker_id).await;

    let result = executor
        .invoke_and_await(
            &worker_id,
            "example1",
            vec![common::val_string("Hello Go-lem")],
        )
        .await
        .unwrap();

    tokio::time::sleep(Duration::from_secs(5)).await;
    let mut events = vec![];
    rx.recv_many(&mut events, 100).await;

    drop(executor);

    let first_line = common::log_event_to_string(&events[0]);
    let second_line = common::log_event_to_string(&events[1]);
    let parts: Vec<_> = second_line.split(' ').collect();
    let last_part = parts.last().unwrap().trim();
    let now = chrono::Local::now();
    let year = now.year();

    check!(first_line == "Hello Go-lem\n".to_string());
    check!(second_line.starts_with(&format!("test {year}")));
    check!(result == vec!(common::val_i32(last_part.parse::<i32>().unwrap())));
}

#[tokio::test]
async fn tinygo_http_client() {
    let mut executor = common::start().await.unwrap();

    let captured_body: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    let captured_body_clone = captured_body.clone();
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
            .run("0.0.0.0:9999".parse::<SocketAddr>().unwrap())
            .await;
    });

    let template_id = executor.store_template(Path::new("../test-templates/tinygo-wasi-http.wasm"));
    let worker_id = executor
        .start_worker(&template_id, "tinygo-wasi-http-1")
        .await;

    let result = executor
        .invoke_and_await(
            &worker_id,
            "example1",
            vec![common::val_string("hello tinygo!")],
        )
        .await
        .unwrap();

    let captured_body = captured_body.lock().unwrap().clone().unwrap();

    drop(executor);
    http_server.abort();

    check!(
        result
            == vec![common::val_string(
                "200 percentage: 0.250000, message: response message no X-Test header"
            )]
    );
    check!(
        captured_body
            == "{\"Name\":\"Something\",\"Amount\":42,\"Comments\":[\"Hello\",\"World\"]}"
                .to_string()
    );
}

#[tokio::test]
async fn grain_example_1() {
    let mut executor = common::start().await.unwrap();

    let template_id = executor.store_template(Path::new("../test-templates/grain-1.wasm"));
    let worker_id = executor.start_worker(&template_id, "grain-1").await;

    let mut rx = executor.capture_output(&worker_id).await;

    let _result = executor
        .invoke_and_await(&worker_id, "wasi:cli/run@0.2.0-rc-2023-11-10/run", vec![])
        .await
        .unwrap();

    tokio::time::sleep(Duration::from_secs(5)).await;
    let mut events = vec![];
    rx.recv_many(&mut events, 100).await;

    drop(executor);

    let first_line = common::log_event_to_string(&events[0]);
    let second_line = common::log_event_to_string(&events[1]);
    let third_line = common::log_event_to_string(&events[2]);

    let now = chrono::Local::now();
    let epoch = now.timestamp_nanos_opt().unwrap();
    let hour = 3_600_000_000_000;

    check!(first_line == "hello world".to_string());
    check!(second_line.parse::<i64>().is_ok());
    check!(third_line.parse::<i64>().unwrap() > (epoch - hour));
    check!(third_line.parse::<i64>().unwrap() < (epoch + hour));
}

#[tokio::test]
async fn java_example_1() {
    let mut executor = common::start().await.unwrap();

    let template_id = executor.store_template(Path::new("../test-templates/java-1.wasm"));
    let worker_id = executor.start_worker(&template_id, "java-1").await;

    let mut rx = executor.capture_output(&worker_id).await;

    let result = executor
        .invoke_and_await(
            &worker_id,
            "run-example1",
            vec![common::val_string("Hello Golem!")],
        )
        .await
        .unwrap();

    tokio::time::sleep(Duration::from_secs(5)).await;
    let mut events = vec![];
    rx.recv_many(&mut events, 100).await;

    drop(executor);

    let first_line = common::log_event_to_string(&events[0]);

    check!(first_line == "Hello world, input is Hello Golem!\n".to_string());
    check!(result == vec![common::val_u32("Hello Golem!".len() as u32)]);
}

#[tokio::test]
async fn java_shopping_cart() {
    let mut executor = common::start().await.unwrap();

    let template_id = executor.store_template(Path::new("../test-templates/java-2.wasm"));
    let worker_id = executor.start_worker(&template_id, "java-2").await;

    let _ = executor
        .invoke_and_await(
            &worker_id,
            "initialize-cart",
            vec![common::val_string("test-user-1")],
        )
        .await;

    let _ = executor
        .invoke_and_await(
            &worker_id,
            "add-item",
            vec![common::val_record(vec![
                common::val_string("G1000"),
                common::val_string("Golem T-Shirt M"),
                common::val_float32(100.0),
                common::val_u32(5),
            ])],
        )
        .await;

    let _ = executor
        .invoke_and_await(
            &worker_id,
            "add-item",
            vec![common::val_record(vec![
                common::val_string("G1001"),
                common::val_string("Golem Cloud Subscription 1y"),
                common::val_float32(999999.0),
                common::val_u32(1),
            ])],
        )
        .await;

    let _ = executor
        .invoke_and_await(
            &worker_id,
            "add-item",
            vec![common::val_record(vec![
                common::val_string("G1002"),
                common::val_string("Mud Golem"),
                common::val_float32(11.0),
                common::val_u32(10),
            ])],
        )
        .await;

    let _ = executor
        .invoke_and_await(
            &worker_id,
            "update-item-quantity",
            vec![common::val_string("G1002"), common::val_u32(20)],
        )
        .await;

    let contents = executor
        .invoke_and_await(&worker_id, "get-cart-contents", vec![])
        .await;

    let _ = executor
        .invoke_and_await(&worker_id, "checkout", vec![])
        .await;

    drop(executor);

    std::assert!(
        contents
            == Ok(vec![common::val_list(vec![
                common::val_record(vec![
                    common::val_string("G1000"),
                    common::val_string("Golem T-Shirt M"),
                    common::val_float32(100.0),
                    common::val_u32(5),
                ]),
                common::val_record(vec![
                    common::val_string("G1001"),
                    common::val_string("Golem Cloud Subscription 1y"),
                    common::val_float32(999999.0),
                    common::val_u32(1),
                ]),
                common::val_record(vec![
                    common::val_string("G1002"),
                    common::val_string("Mud Golem"),
                    common::val_float32(11.0),
                    common::val_u32(20),
                ])
            ])])
    )
}

#[tokio::test]
async fn javascript_example_1() {
    let mut executor = common::start().await.unwrap();

    let template_id = executor.store_template(Path::new("../test-templates/js-1.wasm"));
    let worker_id = executor.start_worker(&template_id, "js-1").await;

    let mut rx = executor.capture_output(&worker_id).await;

    let result = executor
        .invoke_and_await(
            &worker_id,
            "hello",
            vec![common::val_string("JavaScript component")],
        )
        .await
        .unwrap();

    tokio::time::sleep(Duration::from_secs(5)).await;
    let mut events = vec![];
    rx.recv_many(&mut events, 100).await;

    drop(executor);

    let first_line = common::log_event_to_string(&events[0]);
    let parts = first_line.split(' ').collect::<Vec<_>>();
    let now = chrono::Local::now();

    check!(parts[0] == "Hello");
    check!(parts[1] == "JavaScript");
    check!(parts[2] == "component!");
    check!(parts[3].parse::<f64>().is_ok());
    check!(parts[4] == "0"); // NOTE: validating that Date.now() is not working
    check!(parts[13] != "0"); // NOTE: validating that directly calling wasi:clocks/wall-clock/now works
    check!(parts[21].to_string() == now.year().to_string());
    check!(result == vec![common::val_string(&first_line)]);
}

#[tokio::test]
async fn javascript_example_2() {
    let mut executor = common::start().await.unwrap();

    let template_id = executor.store_template(Path::new("../test-templates/js-2.wasm"));
    let worker_id = executor.start_worker(&template_id, "js-2").await;

    let _ = executor
        .invoke_and_await(&worker_id, "golem:it/api/add", vec![common::val_u64(5)])
        .await
        .unwrap();

    let _ = executor
        .invoke_and_await(&worker_id, "golem:it/api/add", vec![common::val_u64(6)])
        .await
        .unwrap();

    let result = executor
        .invoke_and_await(&worker_id, "golem:it/api/get", vec![])
        .await
        .unwrap();

    drop(executor);

    check!(result == vec![common::val_u64(11)]);
}

#[tokio::test]
async fn csharp_example_1() {
    let mut executor = common::start().await.unwrap();

    let template_id = executor.store_template(Path::new("../test-templates/csharp-1.wasm"));
    let mut env = HashMap::new();
    env.insert("TEST_ENV".to_string(), "test-value".to_string());
    let worker_id = executor
        .try_start_worker_versioned(
            &template_id,
            0,
            "csharp-1",
            vec!["test-arg".to_string()],
            env,
        )
        .await
        .unwrap();

    let mut rx = executor.capture_output(&worker_id).await;

    let _result = executor
        .invoke_and_await(&worker_id, "wasi:cli/run@0.2.0-rc-2023-11-10/run", vec![])
        .await
        .unwrap();

    tokio::time::sleep(Duration::from_secs(5)).await;
    let lines = common::events_to_lines(&mut rx).await;

    drop(executor);

    let now = chrono::Local::now();
    let year = now.year();

    check!(lines[0] == "Hello, World!".to_string());
    check!(lines[1].parse::<i32>().is_ok());
    check!(lines[2] == year.to_string());
    // NOTE: command line argument access is not working currently in dotnet-wasi
    check!(lines[3] == "".to_string());
    check!(lines.contains(&"TEST_ENV: test-value".to_string()));
    check!(lines.contains(&format!("GOLEM_TEMPLATE_ID: {template_id}")));
    check!(lines.contains(&"GOLEM_WORKER_NAME: csharp-1".to_string()));
    check!(lines.contains(&"GOLEM_TEMPLATE_VERSION: 0".to_string()));
}

#[tokio::test]
async fn c_example_1() {
    let mut executor = common::start().await.unwrap();

    let template_id = executor.store_template(Path::new("../test-templates/c-1.wasm"));
    let worker_id = executor.start_worker(&template_id, "c-1").await;

    let mut rx = executor.capture_output(&worker_id).await;

    let result = executor
        .invoke_and_await(&worker_id, "run", vec![])
        .await
        .unwrap();

    tokio::time::sleep(Duration::from_secs(5)).await;
    let mut events = vec![];
    rx.recv_many(&mut events, 100).await;

    drop(executor);

    let first_line = common::log_event_to_string(&events[0]);

    check!(first_line == "Hello World!\n".to_string());
    check!(result == vec![common::val_i32(100)]);
}

#[tokio::test]
async fn c_example_2() {
    let mut executor = common::start().await.unwrap();

    let template_id = executor.store_template(Path::new("../test-templates/c-1.wasm"));
    let worker_id = executor.start_worker(&template_id, "c-2").await;

    let mut rx = executor.capture_output(&worker_id).await;

    let _ = executor
        .invoke_and_await(&worker_id, "print", vec![common::val_string("Hello C!")])
        .await
        .unwrap();

    tokio::time::sleep(Duration::from_secs(5)).await;
    let mut events = vec![];
    rx.recv_many(&mut events, 100).await;

    drop(executor);

    let first_line = common::log_event_to_string(&events[0]);
    let now = chrono::Local::now();
    let year = now.year();

    check!(first_line == format!("Hello C! {year}"));
}

#[tokio::test]
async fn swift_example_1() {
    let mut executor = common::start().await.unwrap();

    let template_id = executor.store_template(Path::new("../test-templates/swift-1.wasm"));
    let worker_id = executor.start_worker(&template_id, "swift-1").await;

    let mut rx = executor.capture_output(&worker_id).await;

    let _ = executor
        .invoke_and_await(&worker_id, "wasi:cli/run@0.2.0-rc-2023-11-10/run", vec![])
        .await
        .unwrap();

    tokio::time::sleep(Duration::from_secs(5)).await;
    let lines = common::events_to_lines(&mut rx).await;

    drop(executor);

    let now = chrono::Local::now();
    let year = now.year();

    check!(lines[0] == "Hello world!".to_string());
    check!(lines[1] == year.to_string());
}

#[tokio::test]
async fn python_example_1() {
    let mut executor = common::start().await.unwrap();

    let template_id = executor.store_template(Path::new("../test-templates/python-1.wasm"));
    let worker_id = executor.start_worker(&template_id, "python-1").await;

    let _ = executor
        .invoke_and_await(&worker_id, "golem:it/api/add", vec![common::val_u64(3)])
        .await
        .unwrap();

    let _ = executor
        .invoke_and_await(&worker_id, "golem:it/api/add", vec![common::val_u64(8)])
        .await
        .unwrap();

    let result = executor
        .invoke_and_await(&worker_id, "golem:it/api/get", vec![])
        .await
        .unwrap();

    drop(executor);

    check!(result == vec![common::val_u64(11)]);
}
