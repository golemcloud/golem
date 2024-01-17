use crate::common;
use std::path::Path;
use std::time::Duration;

use assert2::check;

use serde_json::Value;

#[tokio::test]
async fn interruption() {
    let mut executor = common::start().await.unwrap();

    let template_id = executor.store_template(Path::new(
        "../test-templates/interruption.wasm",
    ));
    let worker_id = executor.start_worker(&template_id, "interruption-1").await;

    let mut executor_clone = executor.async_clone().await;
    let worker_id_clone = worker_id.clone();
    let fiber = tokio::spawn(async move {
        executor_clone
            .invoke_and_await(&worker_id_clone, "run", vec![])
            .await
    });

    tokio::time::sleep(Duration::from_secs(2)).await;

    let _ = executor.interrupt(&worker_id).await;
    let result = fiber.await.unwrap();

    drop(executor);

    println!(
        "result: {:?}",
        result.as_ref().map_err(|err| err.to_string())
    );
    check!(result.is_err());
    check!(result
        .err()
        .unwrap()
        .to_string()
        .contains("Interrupted via the Golem API"));
}

#[tokio::test]
async fn simulated_crash() {
    let mut executor = common::start().await.unwrap();

    let template_id = executor.store_template(Path::new(
        "../test-templates/interruption.wasm",
    ));
    let worker_id = executor
        .start_worker(&template_id, "simulated-crash-1")
        .await;

    let mut rx = executor.capture_output(&worker_id).await;

    let mut executor_clone = executor.async_clone().await;
    let worker_id_clone = worker_id.clone();
    let fiber = tokio::spawn(async move {
        let start_time = tokio::time::Instant::now();
        let invoke_result = executor_clone
            .invoke_and_await(&worker_id_clone, "run", vec![])
            .await;
        let elapsed = start_time.elapsed();
        (invoke_result, elapsed)
    });

    tokio::time::sleep(Duration::from_secs(5)).await;

    let _ = executor.simulated_crash(&worker_id).await;
    let (result, elapsed) = fiber.await.unwrap();

    let mut events = vec![];
    rx.recv_many(&mut events, 100).await;
    drop(executor);

    println!(
        "result: {:?}",
        result.as_ref().map_err(|err| err.to_string())
    );
    check!(result.is_ok());
    check!(result == Ok(vec![common::val_string("done")]));
    check!(events == vec![common::stdout_event("Starting interruption test\n"),]);
    check!(elapsed.as_secs() < 13);
}

#[tokio::test]
async fn shopping_cart_example() {
    let mut executor = common::start().await.unwrap();

    let template_id = executor.store_template(Path::new(
        "../test-templates/shopping-cart.wasm",
    ));
    let worker_id = executor.start_worker(&template_id, "shopping-cart-1").await;

    let _ = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api/initialize-cart",
            vec![common::val_string("test-user-1")],
        )
        .await;

    let _ = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api/add-item",
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
            "golem:it/api/add-item",
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
            "golem:it/api/add-item",
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
            "golem:it/api/update-item-quantity",
            vec![common::val_string("G1002"), common::val_u32(20)],
        )
        .await;

    let contents = executor
        .invoke_and_await(&worker_id, "golem:it/api/get-cart-contents", vec![])
        .await;

    let _ = executor
        .invoke_and_await(&worker_id, "golem:it/api/checkout", vec![])
        .await;

    drop(executor);

    assert!(
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
async fn stdio_cc() {
    let mut executor = common::start().await.unwrap();

    let template_id =
        executor.store_template(Path::new("../test-templates/stdio-cc.wasm"));
    let worker_id = executor.start_worker(&template_id, "stdio-cc-1").await;

    let result = executor
        .invoke_and_await_stdio(&worker_id, "run", Value::Number(1234.into()))
        .await;

    drop(executor);

    assert!(result == Ok(Value::Number(2468.into())))
}
