use crate::common;
use std::collections::HashMap;
use std::env;
use std::io::Write;
use std::net::SocketAddr;
use std::os::unix::fs::FileExt;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use assert2::check;
use http_02::{Response, StatusCode};

use golem_api_grpc::proto::golem::worker::{worker_execution_error, LogEvent, TemplateParseFailed};
use golem_api_grpc::proto::golem::workerexecutor::CompletePromiseRequest;
use golem_common::model::{AccountId, InvocationKey, PromiseId, WorkerId, WorkerStatus};
use golem_worker_executor_base::error::GolemError;
use serde_json::Value;

use tokio::time::sleep;
use tonic::transport::Body;
use tracing::debug;
use warp::Filter;
use wasmtime_wasi::preview2::spawn;

#[tokio::test]
async fn interruption() {
    let context = common::TestContext::new();
    let mut executor = common::start(&context).await.unwrap();

    let template_id = executor.store_template(Path::new("../test-templates/interruption.wasm"));
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
    let context = common::TestContext::new();
    let mut executor = common::start(&context).await.unwrap();

    let template_id = executor.store_template(Path::new("../test-templates/interruption.wasm"));
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
    let context = common::TestContext::new();
    let mut executor = common::start(&context).await.unwrap();

    let template_id = executor.store_template(Path::new("../test-templates/shopping-cart.wasm"));
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
    let context = common::TestContext::new();
    let mut executor = common::start(&context).await.unwrap();

    let template_id = executor.store_template(Path::new("../test-templates/stdio-cc.wasm"));
    let worker_id = executor.start_worker(&template_id, "stdio-cc-1").await;

    let result = executor
        .invoke_and_await_stdio(&worker_id, "run", Value::Number(1234.into()))
        .await;

    drop(executor);

    assert!(result == Ok(Value::Number(2468.into())))
}

#[tokio::test]
async fn dynamic_instance_creation() {
    let context = common::TestContext::new();
    let mut executor = common::start(&context).await.unwrap();

    let template_id =
        executor.store_template(Path::new("../test-templates/environment-service.wasm"));
    let worker_id = WorkerId {
        template_id: template_id.clone(),
        worker_name: "dynamic-instance-creation-1".to_string(),
    };

    let args = executor
        .invoke_and_await(&worker_id, "golem:it/api/get-arguments", vec![])
        .await
        .unwrap();
    let env = executor
        .invoke_and_await(&worker_id, "golem:it/api/get-environment", vec![])
        .await
        .unwrap();

    drop(executor);

    check!(args == vec![common::val_result(Ok(common::val_list(vec![])))]);
    check!(
        env == vec![common::val_result(Ok(common::val_list(vec![
            common::val_pair(
                common::val_string("GOLEM_WORKER_NAME"),
                common::val_string("dynamic-instance-creation-1")
            ),
            common::val_pair(
                common::val_string("GOLEM_TEMPLATE_ID"),
                common::val_string(&format!("{}", template_id))
            ),
            common::val_pair(
                common::val_string("GOLEM_TEMPLATE_VERSION"),
                common::val_string("0")
            ),
        ])))]
    );
}

#[tokio::test]
async fn promise() {
    let context = common::TestContext::new();
    let mut executor = common::start(&context).await.unwrap();

    let template_id = executor.store_template(Path::new("../test-templates/promise.wasm"));
    let worker_id = executor.start_worker(&template_id, "promise-1").await;

    let mut executor_clone = executor.async_clone().await;
    let worker_id_clone = worker_id.clone();
    let fiber = tokio::spawn(async move {
        executor_clone
            .invoke_and_await(&worker_id_clone, "run", vec![])
            .await
            .unwrap()
    });

    sleep(Duration::from_secs(10)).await;

    executor
        .client
        .complete_promise(CompletePromiseRequest {
            promise_id: Some(
                PromiseId {
                    worker_id: worker_id.clone(),
                    oplog_idx: 1,
                }
                .into(),
            ),
            data: vec![42],
        })
        .await
        .unwrap();

    let result = fiber.await.unwrap();

    drop(executor);

    check!(result == vec![common::val_list(vec![common::val_u8(42)])]);
}

#[tokio::test]
async fn get_self_uri() {
    let context = common::TestContext::new();
    let mut executor = common::start(&context).await.unwrap();

    let template_id = executor.store_template(Path::new("../test-templates/runtime-service.wasm"));
    let worker_id = executor
        .start_worker(&template_id, "runtime-service-1")
        .await;

    let result = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api/get-self-uri",
            vec![common::val_string("function-name")],
        )
        .await
        .unwrap();

    drop(executor);

    check!(
        result
            == vec![common::val_string(&format!(
                "worker://{template_id}/runtime-service-1/function-name"
            ))]
    );
}

#[tokio::test]
async fn invoking_with_same_invocation_key_is_idempotent() {
    let context = common::TestContext::new();
    let mut executor = common::start(&context).await.unwrap();

    let template_id = executor.store_template(Path::new("../test-templates/shopping-cart.wasm"));
    let worker_id = executor.start_worker(&template_id, "shopping-cart-2").await;

    let invocation_key = executor.get_invocation_key(&worker_id).await;
    let _result = executor
        .invoke_and_await_with_key(
            &worker_id,
            &invocation_key,
            "golem:it/api/add-item",
            vec![common::val_record(vec![
                common::val_string("G1000"),
                common::val_string("Golem T-Shirt M"),
                common::val_float32(100.0),
                common::val_u32(5),
            ])],
        )
        .await
        .unwrap();

    let _result2 = executor
        .invoke_and_await_with_key(
            &worker_id,
            &invocation_key,
            "golem:it/api/add-item",
            vec![common::val_record(vec![
                common::val_string("G1000"),
                common::val_string("Golem T-Shirt M"),
                common::val_float32(100.0),
                common::val_u32(5),
            ])],
        )
        .await
        .unwrap();

    let contents = executor
        .invoke_and_await(&worker_id, "golem:it/api/get-cart-contents", vec![])
        .await
        .unwrap();

    drop(executor);

    check!(
        contents
            == vec![common::val_list(vec![common::val_record(vec![
                common::val_string("G1000"),
                common::val_string("Golem T-Shirt M"),
                common::val_float32(100.0),
                common::val_u32(5),
            ])])]
    );
}

#[tokio::test]
async fn invoking_with_invalid_invocation_key_is_failure() {
    let context = common::TestContext::new();
    let mut executor = common::start(&context).await.unwrap();

    let template_id = executor.store_template(Path::new("../test-templates/shopping-cart.wasm"));
    let worker_id = executor.start_worker(&template_id, "shopping-cart-3").await;

    let invocation_key = InvocationKey {
        value: "bad-invocation-key".to_string(),
    };
    let result = executor
        .invoke_and_await_with_key(
            &worker_id,
            &invocation_key,
            "golem:it/api/add-item",
            vec![common::val_record(vec![
                common::val_string("G1000"),
                common::val_string("Golem T-Shirt M"),
                common::val_float32(100.0),
                common::val_u32(5),
            ])],
        )
        .await;

    drop(executor);

    check!(result.is_err());
}

#[tokio::test]
async fn invoking_with_same_invocation_key_is_idempotent_after_restart() {
    let context = common::TestContext::new();
    let mut executor = common::start(&context).await.unwrap();

    let template_id = executor.store_template(Path::new("../test-templates/shopping-cart.wasm"));
    let worker_id = executor.start_worker(&template_id, "shopping-cart-4").await;

    let invocation_key = executor.get_invocation_key(&worker_id).await;
    let _result = executor
        .invoke_and_await_with_key(
            &worker_id,
            &invocation_key,
            "golem:it/api/add-item",
            vec![common::val_record(vec![
                common::val_string("G1000"),
                common::val_string("Golem T-Shirt M"),
                common::val_float32(100.0),
                common::val_u32(5),
            ])],
        )
        .await
        .unwrap();

    drop(executor);
    let mut executor = common::start(&context).await.unwrap();

    let _result2 = executor
        .invoke_and_await_with_key(
            &worker_id,
            &invocation_key,
            "golem:it/api/add-item",
            vec![common::val_record(vec![
                common::val_string("G1000"),
                common::val_string("Golem T-Shirt M"),
                common::val_float32(100.0),
                common::val_u32(5),
            ])],
        )
        .await
        .unwrap();

    let contents = executor
        .invoke_and_await(&worker_id, "golem:it/api/get-cart-contents", vec![])
        .await
        .unwrap();

    drop(executor);

    check!(
        contents
            == vec![common::val_list(vec![common::val_record(vec![
                common::val_string("G1000"),
                common::val_string("Golem T-Shirt M"),
                common::val_float32(100.0),
                common::val_u32(5),
            ])])]
    );
}

#[tokio::test]
async fn optional_parameters() {
    let context = common::TestContext::new();
    let mut executor = common::start(&context).await.unwrap();

    let template_id = executor.store_template(Path::new("../test-templates/option-service.wasm"));
    let worker_id = executor
        .start_worker(&template_id, "optional-service-1")
        .await;

    let echo_some = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api/echo",
            vec![common::val_option(Some(common::val_string("Hello")))],
        )
        .await
        .unwrap();

    let echo_none = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api/echo",
            vec![common::val_option(None)],
        )
        .await
        .unwrap();

    let todo_some = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api/todo",
            vec![common::val_record(vec![
                common::val_string("todo"),
                common::val_option(Some(common::val_string("description"))),
            ])],
        )
        .await
        .unwrap();

    let todo_none = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api/todo",
            vec![common::val_record(vec![
                common::val_string("todo"),
                common::val_option(Some(common::val_string("description"))),
            ])],
        )
        .await
        .unwrap();

    drop(executor);

    check!(echo_some == vec![common::val_option(Some(common::val_string("Hello")))]);
    check!(echo_none == vec![common::val_option(None)]);
    check!(todo_some == vec![common::val_string("todo")]);
    check!(todo_none == vec![common::val_string("todo")]);
}

#[tokio::test]
async fn flags_parameters() {
    let context = common::TestContext::new();
    let mut executor = common::start(&context).await.unwrap();

    let template_id = executor.store_template(Path::new("../test-templates/flags-service.wasm"));
    let worker_id = executor.start_worker(&template_id, "flags-service-1").await;

    let create_task = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api/create-task",
            vec![common::val_record(vec![
                common::val_string("t1"),
                common::val_flags(4, &[0, 1]),
            ])],
        )
        .await
        .unwrap();

    let get_tasks = executor
        .invoke_and_await(&worker_id, "golem:it/api/get-tasks", vec![])
        .await
        .unwrap();

    drop(executor);

    check!(
        create_task
            == vec![common::val_record(vec![
                common::val_string("t1"),
                common::val_flags(4, &[0, 1, 2])
            ])]
    );
    check!(
        get_tasks
            == vec![common::val_list(vec![
                common::val_record(vec![common::val_string("t1"), common::val_flags(4, &[0])]),
                common::val_record(vec![
                    common::val_string("t2"),
                    common::val_flags(4, &[0, 2, 3])
                ])
            ])]
    );
}

#[tokio::test]
async fn variants_with_no_payloads() {
    let context = common::TestContext::new();
    let mut executor = common::start(&context).await.unwrap();

    let template_id = executor.store_template(Path::new("../test-templates/variant-service.wasm"));
    let worker_id = executor
        .start_worker(&template_id, "variant-service-1")
        .await;

    let result = executor
        .invoke_and_await(&worker_id, "golem:it/api/bid", vec![])
        .await;

    drop(executor);

    check!(result.is_ok());
}

#[tokio::test]
async fn delete_instance() {
    let context = common::TestContext::new();
    let mut executor = common::start(&context).await.unwrap();

    let template_id = executor.store_template(Path::new("../test-templates/option-service.wasm"));
    let worker_id = executor
        .start_worker(&template_id, "delete-instance-1")
        .await;

    let _ = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api/echo",
            vec![common::val_option(Some(common::val_string("Hello")))],
        )
        .await
        .unwrap();

    let metadata1 = executor.get_worker_metadata(&worker_id).await;
    executor.delete_worker(&worker_id).await;
    let metadata2 = executor.get_worker_metadata(&worker_id).await;

    check!(metadata1.is_some());
    check!(metadata2.is_none());
}

#[tokio::test]
async fn error_handling_when_worker_is_invoked_with_fewer_than_expected_parameters() {
    let context = common::TestContext::new();
    let mut executor = common::start(&context).await.unwrap();

    let template_id = executor.store_template(Path::new("../test-templates/option-service.wasm"));
    let worker_id = executor
        .start_worker(&template_id, "fewer-than-expected-parameters-1")
        .await;

    let failure = executor
        .invoke_and_await(&worker_id, "golem:it/api/echo", vec![])
        .await;
    drop(executor);
    check!(failure.is_err());
}

#[tokio::test]
async fn error_handling_when_worker_is_invoked_with_more_than_expected_parameters() {
    let context = common::TestContext::new();
    let mut executor = common::start(&context).await.unwrap();

    let template_id = executor.store_template(Path::new("../test-templates/option-service.wasm"));
    let worker_id = executor
        .start_worker(&template_id, "more-than-expected-parameters-1")
        .await;

    let failure = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api/echo",
            vec![
                common::val_option(Some(common::val_string("Hello"))),
                common::val_string("extra parameter"),
            ],
        )
        .await;
    drop(executor);

    check!(failure.is_err());
}

#[tokio::test]
async fn get_instance_metadata() {
    let context = common::TestContext::new();
    let mut executor = common::start(&context).await.unwrap();

    let template_id = executor.store_template(Path::new("../test-templates/clock-service.wasm"));
    let worker_id = executor
        .start_worker(&template_id, "get-instance-metadata-1")
        .await;

    let worker_id_clone = worker_id.clone();
    let mut executor_clone = executor.async_clone().await;
    let fiber = tokio::spawn(async move {
        executor_clone
            .invoke_and_await(
                &worker_id_clone,
                "golem:it/api/sleep",
                vec![common::val_u64(10)],
            )
            .await
    });

    sleep(Duration::from_secs(5)).await;

    let metadata1 = executor.get_worker_metadata(&worker_id).await.unwrap();
    let _ = fiber.await;

    sleep(Duration::from_secs(2)).await;

    let metadata2 = executor.get_worker_metadata(&worker_id).await.unwrap();

    drop(executor);

    check!(
        metadata1.last_known_status.status == WorkerStatus::Suspended || // it is sleeping - whether it is suspended or not is the server's decision
        metadata1.last_known_status.status == WorkerStatus::Running
    );
    check!(metadata2.last_known_status.status == WorkerStatus::Idle);
    check!(metadata1.worker_id.template_version == 0);
    check!(metadata1.worker_id.worker_id == worker_id);
    check!(
        metadata1.account_id
            == AccountId {
                value: "test-account".to_string()
            }
    );
}

#[tokio::test]
async fn create_invoke_delete_create_invoke() {
    let context = common::TestContext::new();
    let mut executor = common::start(&context).await.unwrap();

    let template_id = executor.store_template(Path::new("../test-templates/shopping-cart.wasm"));
    let worker_id = executor
        .start_worker(&template_id, "create-invoke-delete-create-invoke-1")
        .await;

    let r1 = executor
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

    executor.delete_worker(&worker_id).await;

    let worker_id = executor
        .start_worker(&template_id, "create-invoke-delete-create-invoke-1") // same name as before
        .await;

    let r2 = executor
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

    drop(executor);

    check!(r1.is_ok());
    check!(r2.is_ok());
}

#[tokio::test]
async fn recovering_an_old_instance_after_updating_a_template() {
    let context = common::TestContext::new();
    let mut executor = common::start(&context).await.unwrap();

    let template_id = executor.store_template(Path::new("../test-templates/shopping-cart.wasm"));
    let worker_id = executor
        .start_worker(
            &template_id,
            "recovering-an-old-instance-after-updating-a-template-1",
        )
        .await;

    let r1 = executor
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
        .await
        .unwrap();

    // Updating the template with an incompatible new version
    let new_version = executor.update_template(
        &template_id,
        Path::new("../test-templates/option-service.wasm"),
    );

    // Creating a new worker of the updated template and call it
    let worker_id2 = executor
        .start_worker_versioned(
            &template_id,
            new_version,
            "recovering-an-old-instance-after-updating-a-template-2",
        )
        .await;

    let r2 = executor
        .invoke_and_await(
            &worker_id2,
            "golem:it/api/echo",
            vec![common::val_option(Some(common::val_string("Hello")))],
        )
        .await
        .unwrap();

    // Restarting the server to force worker recovery
    drop(executor);
    let mut executor = common::start(&context).await.unwrap();

    // Call the first worker again to check if it is still working
    let r3 = executor
        .invoke_and_await(&worker_id, "golem:it/api/get-cart-contents", vec![])
        .await
        .unwrap();

    drop(executor);

    check!(r1 == vec![]);
    check!(r2 == vec![common::val_option(Some(common::val_string("Hello")))]);
    check!(
        r3 == vec![common::val_list(vec![common::val_record(vec![
            common::val_string("G1000"),
            common::val_string("Golem T-Shirt M"),
            common::val_float32(100.0),
            common::val_u32(5),
        ])])]
    );
}

#[tokio::test]
async fn recreating_an_instance_after_it_got_deleted_with_a_different_version() {
    let context = common::TestContext::new();
    let mut executor = common::start(&context).await.unwrap();

    let template_id = executor.store_template(Path::new("../test-templates/shopping-cart.wasm"));
    let worker_id = executor
        .start_worker(
            &template_id,
            "recreating-an-instance-after-it-got-deleted-with-a-different-version-1",
        )
        .await;

    let r1 = executor
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
        .await
        .unwrap();

    // Updating the template with an incompatible new version
    let new_version = executor.update_template(
        &template_id,
        Path::new("../test-templates/option-service.wasm"),
    );

    // Deleting the first instance
    executor.delete_worker(&worker_id).await;

    // Create a new instance with the same name and call it the first instance again to check if it is still working
    let worker_id = executor
        .start_worker_versioned(
            &template_id,
            new_version,
            "recovering-an-old-instance-after-updating-a-template-1",
        )
        .await;

    let r2 = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api/echo",
            vec![common::val_option(Some(common::val_string("Hello")))],
        )
        .await
        .unwrap();

    drop(executor);

    check!(r1 == vec![]);
    check!(r2 == vec![common::val_option(Some(common::val_string("Hello")))]);
}

#[tokio::test]
async fn trying_to_use_an_old_wasm_provides_good_error_message() {
    let context = common::TestContext::new();
    // case: WASM is an old version, rejected by protector

    let mut executor = common::start(&context).await.unwrap();

    let template_id =
        executor.store_template_unverified(Path::new("../test-templates/old-component.wasm"));
    let result = executor
        .try_start_worker(&template_id, "old-component-1")
        .await;

    check!(result.is_err());
    check!(
        result.err().unwrap()
            == worker_execution_error::Error::TemplateParseFailed(TemplateParseFailed {
                template_id: Some(template_id.into()),
                template_version: 0,
                reason: "failed to parse WebAssembly module".to_string()
            })
    );
}

#[tokio::test]
async fn trying_to_use_a_wasm_that_wasmtime_cannot_load_provides_good_error_message() {
    let context = common::TestContext::new();
    // case: WASM can be parsed but wasmtime does not support it
    let mut executor = common::start(&context).await.unwrap();
    let template_id = executor.store_template(Path::new("../test-templates/write-stdout.wasm"));

    let cwd = env::current_dir().expect("Failed to get current directory");
    debug!("Current directory: {cwd:?}");
    let target_dir = cwd.join(Path::new("data/templates"));
    let template_path = target_dir.join(Path::new(&format!("{template_id}-0.wasm")));

    {
        let mut file = std::fs::File::options()
            .write(true)
            .truncate(false)
            .open(&template_path)
            .expect("Failed to open template file");
        file.write_at(&[1, 2, 3, 4], 0)
            .expect("Failed to write to template file");
        file.flush().expect("Failed to flush template file");
    }

    let result = executor.try_start_worker(&template_id, "bad-wasm-1").await;

    check!(result.is_err());
    check!(
        result.err().unwrap()
            == worker_execution_error::Error::TemplateParseFailed(TemplateParseFailed {
                template_id: Some(template_id.into()),
                template_version: 0,
                reason: "failed to parse WebAssembly module".to_string()
            })
    );
}

#[tokio::test]
async fn trying_to_use_a_wasm_that_wasmtime_cannot_load_provides_good_error_message_after_recovery()
{
    let context = common::TestContext::new();
    let mut executor = common::start(&context).await.unwrap();
    let template_id = executor.store_template(Path::new("../test-templates/write-stdout.wasm"));

    let worker_id = executor
        .try_start_worker(&template_id, "bad-wasm-2")
        .await
        .unwrap();

    // worker is idle. if we restart the server it won't get recovered
    drop(executor);
    let mut executor = common::start(&context).await.unwrap();

    // corrupting the uploaded WASM
    let cwd = env::current_dir().expect("Failed to get current directory");
    debug!("Current directory: {cwd:?}");
    let target_dir = cwd.join(Path::new("data/templates"));
    let template_path = target_dir.join(Path::new(&format!("{template_id}-0.wasm")));
    let compiled_template_path = target_dir.join(Path::new(&format!("{template_id}-0.cwasm")));

    {
        debug!("Corrupting {:?}", template_path);
        let mut file = std::fs::File::options()
            .write(true)
            .truncate(false)
            .open(&template_path)
            .expect("Failed to open template file");
        file.write_at(&[1, 2, 3, 4], 0)
            .expect("Failed to write to template file");
        file.flush().expect("Failed to flush template file");

        debug!("Deleting {:?}", compiled_template_path);
        std::fs::remove_file(&compiled_template_path).expect("Failed to delete compiled template");
    }

    // trying to invoke the previously created worker
    let result = executor.invoke_and_await(&worker_id, "run", vec![]).await;

    check!(result.is_err());
    check!(
        result.err().unwrap()
            == GolemError::TemplateParseFailed {
                template_id,
                template_version: 0,
                reason: "failed to parse WebAssembly module".to_string()
            }
    );
}

#[tokio::test]
async fn long_running_poll_loop_works_as_expected() {
    let context = common::TestContext::new();
    let mut executor = common::start(&context).await.unwrap();

    let response = Arc::new(Mutex::new("initial".to_string()));
    let response_clone = response.clone();
    let host_http_port = context.host_http_port();

    let http_server = tokio::spawn(async move {
        let route = warp::path::path("poll").and(warp::get()).map(move || {
            let body = response_clone.lock().unwrap();
            Response::builder()
                .status(StatusCode::OK)
                .body(Body::from(body.clone()))
                .unwrap()
        });

        warp::serve(route)
            .run(
                format!("0.0.0.0:{}", host_http_port)
                    .parse::<SocketAddr>()
                    .unwrap(),
            )
            .await;
    });

    let template_id = executor.store_template(Path::new("../test-templates/http-client-2.wasm"));
    let mut env = HashMap::new();
    env.insert("PORT".to_string(), host_http_port.to_string());

    let worker_id = executor
        .try_start_worker_versioned(&template_id, 0, "poll-loop-template-0", vec![], env)
        .await
        .unwrap();

    executor.log_output(&worker_id).await;

    executor
        .invoke(
            &worker_id,
            "golem:it/api/start-polling",
            vec![common::val_string("first")],
        )
        .await
        .unwrap();

    sleep(Duration::from_secs(2)).await;

    let status1 = executor.get_worker_metadata(&worker_id).await.unwrap();

    {
        let mut response = response.lock().unwrap();
        *response = "first".to_string();
    }

    sleep(Duration::from_secs(2)).await;
    let status2 = executor.get_worker_metadata(&worker_id).await.unwrap();

    drop(executor);
    http_server.abort();

    check!(status1.last_known_status.status == WorkerStatus::Running);
    check!(status2.last_known_status.status == WorkerStatus::Idle);
}

#[tokio::test]
async fn long_running_poll_loop_interrupting_and_resuming_by_second_invocation() {
    let context = common::TestContext::new();
    let mut executor = common::start(&context).await.unwrap();

    let response = Arc::new(Mutex::new("initial".to_string()));
    let response_clone = response.clone();
    let host_http_port = context.host_http_port();

    let http_server = tokio::spawn(async move {
        let route = warp::path::path("poll").and(warp::get()).map(move || {
            let body = response_clone.lock().unwrap();
            Response::builder()
                .status(StatusCode::OK)
                .body(Body::from(body.clone()))
                .unwrap()
        });

        warp::serve(route)
            .run(
                format!("0.0.0.0:{}", host_http_port)
                    .parse::<SocketAddr>()
                    .unwrap(),
            )
            .await;
    });

    let template_id = executor.store_template(Path::new("../test-templates/http-client-2.wasm"));
    let mut env = HashMap::new();
    env.insert("PORT".to_string(), host_http_port.to_string());
    let worker_id = executor
        .try_start_worker_versioned(&template_id, 0, "poll-loop-template-1", vec![], env)
        .await
        .unwrap();

    executor.log_output(&worker_id).await;

    executor
        .invoke(
            &worker_id,
            "golem:it/api/start-polling",
            vec![common::val_string("first")],
        )
        .await
        .unwrap();

    sleep(Duration::from_secs(2)).await;
    let status1 = executor.get_worker_metadata(&worker_id).await.unwrap();

    sleep(Duration::from_secs(4)).await;
    executor.interrupt(&worker_id).await;

    sleep(Duration::from_secs(2)).await;
    let status2 = executor.get_worker_metadata(&worker_id).await.unwrap();

    let mut executor_clone = executor.async_clone().await;
    let worker_id_clone = worker_id.clone();
    let fiber = spawn(async move {
        // Invoke blocks until the invocation starts
        executor_clone
            .invoke(
                &worker_id_clone,
                "golem:it/api/start-polling",
                vec![common::val_string("second")],
            )
            .await
            .unwrap();
    });

    sleep(Duration::from_secs(2)).await;
    let status3 = executor.get_worker_metadata(&worker_id).await.unwrap();

    {
        let mut response = response.lock().unwrap();
        *response = "first".to_string();
    }

    fiber.await;

    sleep(Duration::from_secs(3)).await;
    let status4 = executor.get_worker_metadata(&worker_id).await.unwrap();

    {
        let mut response = response.lock().unwrap();
        *response = "second".to_string();
    }

    sleep(Duration::from_secs(2)).await;
    let status5 = executor.get_worker_metadata(&worker_id).await.unwrap();

    drop(executor);
    http_server.abort();

    check!(status1.last_known_status.status == WorkerStatus::Running); // first running
    check!(status2.last_known_status.status == WorkerStatus::Interrupted); // first interrupted
    check!(status3.last_known_status.status == WorkerStatus::Running); // first resumed
    check!(status4.last_known_status.status == WorkerStatus::Running); // second running
    check!(status5.last_known_status.status == WorkerStatus::Idle); // second finished
}

#[tokio::test]
async fn long_running_poll_loop_connection_breaks_on_interrupt() {
    let context = common::TestContext::new();
    let mut executor = common::start(&context).await.unwrap();

    let response = Arc::new(Mutex::new("initial".to_string()));
    let response_clone = response.clone();
    let host_http_port = context.host_http_port();

    let http_server = tokio::spawn(async move {
        let route = warp::path::path("poll").and(warp::get()).map(move || {
            let body = response_clone.lock().unwrap();
            Response::builder()
                .status(StatusCode::OK)
                .body(Body::from(body.clone()))
                .unwrap()
        });

        warp::serve(route)
            .run(
                format!("0.0.0.0:{}", host_http_port)
                    .parse::<SocketAddr>()
                    .unwrap(),
            )
            .await;
    });

    let template_id = executor.store_template(Path::new("../test-templates/http-client-2.wasm"));
    let mut env = HashMap::new();
    env.insert("PORT".to_string(), host_http_port.to_string());
    let worker_id = executor
        .try_start_worker_versioned(&template_id, 0, "poll-loop-template-2", vec![], env)
        .await
        .unwrap();

    let rx = executor.capture_output_with_termination(&worker_id).await;

    executor
        .invoke(
            &worker_id,
            "golem:it/api/start-polling",
            vec![common::val_string("first")],
        )
        .await
        .unwrap();

    sleep(Duration::from_secs(2)).await;

    executor.interrupt(&worker_id).await;

    let events = common::drain_connection(rx).await;

    drop(executor);
    http_server.abort();

    check!(events.contains(&Some(common::stdout_event("Calling the poll endpoint\n"))));
    check!(events.contains(&Some(common::stdout_event("Received initial\n"))));
}

#[tokio::test]
async fn long_running_poll_loop_connection_retry_does_not_resume_interrupted_worker() {
    let context = common::TestContext::new();
    let mut executor = common::start(&context).await.unwrap();

    let response = Arc::new(Mutex::new("initial".to_string()));
    let response_clone = response.clone();
    let host_http_port = context.host_http_port();

    let http_server = tokio::spawn(async move {
        let route = warp::path::path("poll").and(warp::get()).map(move || {
            let body = response_clone.lock().unwrap();
            Response::builder()
                .status(StatusCode::OK)
                .body(Body::from(body.clone()))
                .unwrap()
        });

        warp::serve(route)
            .run(
                format!("0.0.0.0:{}", host_http_port)
                    .parse::<SocketAddr>()
                    .unwrap(),
            )
            .await;
    });

    let template_id = executor.store_template(Path::new("../test-templates/http-client-2.wasm"));
    let mut env = HashMap::new();
    env.insert("PORT".to_string(), host_http_port.to_string());

    let worker_id = executor
        .try_start_worker_versioned(&template_id, 0, "poll-loop-template-3", vec![], env)
        .await
        .unwrap();

    let rx = executor.capture_output_with_termination(&worker_id).await;

    executor
        .invoke(
            &worker_id,
            "golem:it/api/start-polling",
            vec![common::val_string("first")],
        )
        .await
        .unwrap();

    sleep(Duration::from_secs(2)).await;

    executor.interrupt(&worker_id).await;

    let _ = common::drain_connection(rx).await;
    let status1 = executor.get_worker_metadata(&worker_id).await.unwrap();

    let _rx = executor.capture_output_with_termination(&worker_id).await;
    sleep(Duration::from_secs(2)).await;
    let status2 = executor.get_worker_metadata(&worker_id).await.unwrap();

    drop(executor);
    http_server.abort();

    check!(status1.last_known_status.status == WorkerStatus::Interrupted);
    check!(status2.last_known_status.status == WorkerStatus::Interrupted);
}

#[tokio::test]
async fn long_running_poll_loop_connection_can_be_restored_after_resume() {
    let context = common::TestContext::new();
    let mut executor = common::start(&context).await.unwrap();

    let response = Arc::new(Mutex::new("initial".to_string()));
    let response_clone = response.clone();
    let host_http_port = context.host_http_port();

    let http_server = tokio::spawn(async move {
        let route = warp::path::path("poll").and(warp::get()).map(move || {
            let body = response_clone.lock().unwrap();
            Response::builder()
                .status(StatusCode::OK)
                .body(Body::from(body.clone()))
                .unwrap()
        });

        warp::serve(route)
            .run(
                format!("0.0.0.0:{}", host_http_port)
                    .parse::<SocketAddr>()
                    .unwrap(),
            )
            .await;
    });

    let template_id = executor.store_template(Path::new("../test-templates/http-client-2.wasm"));
    let mut env = HashMap::new();
    env.insert("PORT".to_string(), host_http_port.to_string());

    let worker_id = executor
        .try_start_worker_versioned(&template_id, 0, "poll-loop-template-4", vec![], env)
        .await
        .unwrap();

    let rx = executor.capture_output_with_termination(&worker_id).await;

    executor
        .invoke(
            &worker_id,
            "golem:it/api/start-polling",
            vec![common::val_string("first")],
        )
        .await
        .unwrap();

    sleep(Duration::from_secs(2)).await;
    let status1 = executor.get_worker_metadata(&worker_id).await.unwrap();

    executor.interrupt(&worker_id).await;

    let mut events = common::drain_connection(rx).await;
    let status2 = executor.get_worker_metadata(&worker_id).await.unwrap();

    executor.resume(&worker_id).await;
    sleep(Duration::from_secs(2)).await;

    let mut rx = executor.capture_output_with_termination(&worker_id).await;
    let status3 = executor.get_worker_metadata(&worker_id).await.unwrap();

    {
        let mut response = response.lock().unwrap();
        *response = "first".to_string();
    }

    sleep(Duration::from_secs(4)).await;

    rx.recv_many(&mut events, 100).await;

    let status4 = executor.get_worker_metadata(&worker_id).await.unwrap();

    drop(executor);
    http_server.abort();

    let events: Vec<LogEvent> = events.into_iter().flatten().collect();

    check!(status1.last_known_status.status == WorkerStatus::Running);
    check!(status2.last_known_status.status == WorkerStatus::Interrupted);
    check!(status3.last_known_status.status == WorkerStatus::Running);
    check!(status4.last_known_status.status == WorkerStatus::Idle);
    check!(events.contains(&common::stdout_event("Calling the poll endpoint\n")));
    check!(events.contains(&common::stdout_event("Received initial\n")));
    check!(events.contains(&common::stdout_event("Poll loop finished\n")));
}

#[tokio::test]
async fn long_running_poll_loop_worker_can_be_deleted_after_interrupt() {
    let context = common::TestContext::new();
    let mut executor = common::start(&context).await.unwrap();

    let response = Arc::new(Mutex::new("initial".to_string()));
    let response_clone = response.clone();
    let host_http_port = context.host_http_port();

    let http_server = tokio::spawn(async move {
        let route = warp::path::path("poll").and(warp::get()).map(move || {
            let body = response_clone.lock().unwrap();
            Response::builder()
                .status(StatusCode::OK)
                .body(Body::from(body.clone()))
                .unwrap()
        });

        warp::serve(route)
            .run(
                format!("0.0.0.0:{}", host_http_port)
                    .parse::<SocketAddr>()
                    .unwrap(),
            )
            .await;
    });

    let template_id = executor.store_template(Path::new("../test-templates/http-client-2.wasm"));
    let mut env = HashMap::new();
    env.insert("PORT".to_string(), host_http_port.to_string());

    let worker_id = executor
        .try_start_worker_versioned(&template_id, 0, "poll-loop-template-5", vec![], env)
        .await
        .unwrap();

    let rx = executor.capture_output_with_termination(&worker_id).await;

    executor
        .invoke(
            &worker_id,
            "golem:it/api/start-polling",
            vec![common::val_string("first")],
        )
        .await
        .unwrap();

    sleep(Duration::from_secs(2)).await;

    executor.interrupt(&worker_id).await;

    let _ = common::drain_connection(rx).await;

    executor.delete_worker(&worker_id).await;
    let metadata = executor.get_worker_metadata(&worker_id).await;

    drop(executor);
    http_server.abort();

    check!(metadata.is_none());
}
