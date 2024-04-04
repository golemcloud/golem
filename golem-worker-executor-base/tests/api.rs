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
use redis::Commands;

use golem_api_grpc::proto::golem::worker::{worker_execution_error, LogEvent, TemplateParseFailed};
use golem_api_grpc::proto::golem::workerexecutor::CompletePromiseRequest;
use golem_common::model::{
    AccountId, FilterComparator, InvocationKey, PromiseId, StringFilterComparator, TemplateId,
    WorkerFilter, WorkerId, WorkerMetadata, WorkerStatus,
};

use serde_json::Value;

use crate::common::{start, TestContext, TestWorkerExecutor};
use golem_test_framework::config::TestDependencies;
use golem_test_framework::dsl::{
    drain_connection, is_worker_execution_error, stdout_event, val_flags, val_float32, val_list,
    val_option, val_pair, val_record, val_result, val_string, val_u32, val_u64, val_u8,
    worker_error_message, TestDsl,
};
use tokio::time::sleep;
use tonic::transport::Body;
use tracing::debug;
use warp::Filter;
use wasmtime_wasi::preview2::spawn;

#[tokio::test]
#[tracing::instrument]
async fn interruption() {
    let context = TestContext::new();
    let executor = start(&context).await.unwrap();

    let template_id = executor.store_template("interruption").await;
    let worker_id = executor.start_worker(&template_id, "interruption-1").await;

    let executor_clone = executor.clone();
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
        result.as_ref().map_err(worker_error_message)
    );
    check!(result.is_err());
    check!(worker_error_message(&result.err().unwrap()).contains("Interrupted via the Golem API"));
}

#[tokio::test]
#[tracing::instrument]
async fn simulated_crash() {
    let context = TestContext::new();
    let executor = start(&context).await.unwrap();

    let template_id = executor.store_template("interruption").await;
    let worker_id = executor
        .start_worker(&template_id, "simulated-crash-1")
        .await;

    let mut rx = executor.capture_output(&worker_id).await;

    let executor_clone = executor.clone();
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
        result.as_ref().map_err(worker_error_message)
    );
    check!(result.is_ok());
    check!(result == Ok(vec![val_string("done")]));
    check!(events == vec![stdout_event("Starting interruption test\n"),]);
    check!(elapsed.as_secs() < 13);
}

#[tokio::test]
#[tracing::instrument]
async fn shopping_cart_example() {
    let context = TestContext::new();
    let executor = start(&context).await.unwrap();

    let template_id = executor.store_template("shopping-cart").await;
    let worker_id = executor.start_worker(&template_id, "shopping-cart-1").await;

    let _ = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api/initialize-cart",
            vec![val_string("test-user-1")],
        )
        .await;

    let _ = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api/add-item",
            vec![val_record(vec![
                val_string("G1000"),
                val_string("Golem T-Shirt M"),
                val_float32(100.0),
                val_u32(5),
            ])],
        )
        .await;

    let _ = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api/add-item",
            vec![val_record(vec![
                val_string("G1001"),
                val_string("Golem Cloud Subscription 1y"),
                val_float32(999999.0),
                val_u32(1),
            ])],
        )
        .await;

    let _ = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api/add-item",
            vec![val_record(vec![
                val_string("G1002"),
                val_string("Mud Golem"),
                val_float32(11.0),
                val_u32(10),
            ])],
        )
        .await;

    let _ = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api/update-item-quantity",
            vec![val_string("G1002"), val_u32(20)],
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
            == Ok(vec![val_list(vec![
                val_record(vec![
                    val_string("G1000"),
                    val_string("Golem T-Shirt M"),
                    val_float32(100.0),
                    val_u32(5),
                ]),
                val_record(vec![
                    val_string("G1001"),
                    val_string("Golem Cloud Subscription 1y"),
                    val_float32(999999.0),
                    val_u32(1),
                ]),
                val_record(vec![
                    val_string("G1002"),
                    val_string("Mud Golem"),
                    val_float32(11.0),
                    val_u32(20),
                ]),
            ])])
    )
}

#[tokio::test]
#[tracing::instrument]
async fn stdio_cc() {
    let context = TestContext::new();
    let executor = start(&context).await.unwrap();

    let template_id = executor.store_template("stdio-cc").await;
    let worker_id = executor.start_worker(&template_id, "stdio-cc-1").await;

    let result = executor
        .invoke_and_await_stdio(&worker_id, "run", Value::Number(1234.into()))
        .await;

    drop(executor);

    assert!(result == Ok(Value::Number(2468.into())))
}

#[tokio::test]
#[tracing::instrument]
async fn dynamic_instance_creation() {
    let context = TestContext::new();
    let executor = start(&context).await.unwrap();

    let template_id = executor.store_template("environment-service").await;
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

    check!(args == vec![val_result(Ok(val_list(vec![])))]);
    check!(
        env == vec![val_result(Ok(val_list(vec![
            val_pair(
                val_string("GOLEM_WORKER_NAME"),
                val_string("dynamic-instance-creation-1")
            ),
            val_pair(
                val_string("GOLEM_TEMPLATE_ID"),
                val_string(&format!("{}", template_id))
            ),
            val_pair(val_string("GOLEM_TEMPLATE_VERSION"), val_string("0")),
        ])))]
    );
}

#[tokio::test]
#[tracing::instrument]
async fn promise() {
    let context = TestContext::new();
    let executor = start(&context).await.unwrap();

    let template_id = executor.store_template("promise").await;
    let worker_id = executor.start_worker(&template_id, "promise-1").await;

    let executor_clone = executor.clone();
    let worker_id_clone = worker_id.clone();
    let fiber = tokio::spawn(async move {
        executor_clone
            .invoke_and_await(&worker_id_clone, "run", vec![])
            .await
            .unwrap()
    });

    sleep(Duration::from_secs(10)).await;

    executor
        .client()
        .await
        .complete_promise(CompletePromiseRequest {
            promise_id: Some(
                PromiseId {
                    worker_id: worker_id.clone(),
                    oplog_idx: 2,
                }
                .into(),
            ),
            data: vec![42],
        })
        .await
        .unwrap();

    let result = fiber.await.unwrap();

    drop(executor);

    check!(result == vec![val_list(vec![val_u8(42)])]);
}

#[tokio::test]
#[tracing::instrument]
async fn get_self_uri() {
    let context = TestContext::new();
    let executor = start(&context).await.unwrap();

    let template_id = executor.store_template("runtime-service").await;
    let worker_id = executor
        .start_worker(&template_id, "runtime-service-1")
        .await;

    let result = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api/get-self-uri",
            vec![val_string("function-name")],
        )
        .await
        .unwrap();

    drop(executor);

    check!(
        result
            == vec![val_string(&format!(
                "worker://{template_id}/runtime-service-1/function-name"
            ))]
    );
}

#[tokio::test]
#[tracing::instrument]
async fn invoking_with_same_invocation_key_is_idempotent() {
    let context = TestContext::new();
    let executor = start(&context).await.unwrap();

    let template_id = executor.store_template("shopping-cart").await;
    let worker_id = executor.start_worker(&template_id, "shopping-cart-2").await;

    let invocation_key = executor.get_invocation_key(&worker_id).await;
    let _result = executor
        .invoke_and_await_with_key(
            &worker_id,
            &invocation_key,
            "golem:it/api/add-item",
            vec![val_record(vec![
                val_string("G1000"),
                val_string("Golem T-Shirt M"),
                val_float32(100.0),
                val_u32(5),
            ])],
        )
        .await
        .unwrap();

    let _result2 = executor
        .invoke_and_await_with_key(
            &worker_id,
            &invocation_key,
            "golem:it/api/add-item",
            vec![val_record(vec![
                val_string("G1000"),
                val_string("Golem T-Shirt M"),
                val_float32(100.0),
                val_u32(5),
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
            == vec![val_list(vec![val_record(vec![
                val_string("G1000"),
                val_string("Golem T-Shirt M"),
                val_float32(100.0),
                val_u32(5),
            ])])]
    );
}

#[tokio::test]
#[tracing::instrument]
async fn invoking_with_invalid_invocation_key_is_failure() {
    let context = TestContext::new();
    let executor = start(&context).await.unwrap();

    let template_id = executor.store_template("shopping-cart").await;
    let worker_id = executor.start_worker(&template_id, "shopping-cart-3").await;

    let invocation_key = InvocationKey {
        value: "bad-invocation-key".to_string(),
    };
    let result = executor
        .invoke_and_await_with_key(
            &worker_id,
            &invocation_key,
            "golem:it/api/add-item",
            vec![val_record(vec![
                val_string("G1000"),
                val_string("Golem T-Shirt M"),
                val_float32(100.0),
                val_u32(5),
            ])],
        )
        .await;

    drop(executor);

    check!(result.is_err());
}

#[tokio::test]
#[tracing::instrument]
async fn invoking_with_same_invocation_key_is_idempotent_after_restart() {
    let context = TestContext::new();
    let executor = start(&context).await.unwrap();

    let template_id = executor.store_template("shopping-cart").await;
    let worker_id = executor.start_worker(&template_id, "shopping-cart-4").await;

    let invocation_key = executor.get_invocation_key(&worker_id).await;
    let _result = executor
        .invoke_and_await_with_key(
            &worker_id,
            &invocation_key,
            "golem:it/api/add-item",
            vec![val_record(vec![
                val_string("G1000"),
                val_string("Golem T-Shirt M"),
                val_float32(100.0),
                val_u32(5),
            ])],
        )
        .await
        .unwrap();

    drop(executor);
    let executor = start(&context).await.unwrap();

    let _result2 = executor
        .invoke_and_await_with_key(
            &worker_id,
            &invocation_key,
            "golem:it/api/add-item",
            vec![val_record(vec![
                val_string("G1000"),
                val_string("Golem T-Shirt M"),
                val_float32(100.0),
                val_u32(5),
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
            == vec![val_list(vec![val_record(vec![
                val_string("G1000"),
                val_string("Golem T-Shirt M"),
                val_float32(100.0),
                val_u32(5),
            ])])]
    );
}

#[tokio::test]
#[tracing::instrument]
async fn optional_parameters() {
    let context = TestContext::new();
    let executor = start(&context).await.unwrap();

    let template_id = executor.store_template("option-service").await;
    let worker_id = executor
        .start_worker(&template_id, "optional-service-1")
        .await;

    let echo_some = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api/echo",
            vec![val_option(Some(val_string("Hello")))],
        )
        .await
        .unwrap();

    let echo_none = executor
        .invoke_and_await(&worker_id, "golem:it/api/echo", vec![val_option(None)])
        .await
        .unwrap();

    let todo_some = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api/todo",
            vec![val_record(vec![
                val_string("todo"),
                val_option(Some(val_string("description"))),
            ])],
        )
        .await
        .unwrap();

    let todo_none = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api/todo",
            vec![val_record(vec![
                val_string("todo"),
                val_option(Some(val_string("description"))),
            ])],
        )
        .await
        .unwrap();

    drop(executor);

    check!(echo_some == vec![val_option(Some(val_string("Hello")))]);
    check!(echo_none == vec![val_option(None)]);
    check!(todo_some == vec![val_string("todo")]);
    check!(todo_none == vec![val_string("todo")]);
}

#[tokio::test]
#[tracing::instrument]
async fn flags_parameters() {
    let context = TestContext::new();
    let executor = start(&context).await.unwrap();

    let template_id = executor.store_template("flags-service").await;
    let worker_id = executor.start_worker(&template_id, "flags-service-1").await;

    let create_task = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api/create-task",
            vec![val_record(vec![val_string("t1"), val_flags(4, &[0, 1])])],
        )
        .await
        .unwrap();

    let get_tasks = executor
        .invoke_and_await(&worker_id, "golem:it/api/get-tasks", vec![])
        .await
        .unwrap();

    drop(executor);

    check!(create_task == vec![val_record(vec![val_string("t1"), val_flags(4, &[0, 1, 2])])]);
    check!(
        get_tasks
            == vec![val_list(vec![
                val_record(vec![val_string("t1"), val_flags(4, &[0])]),
                val_record(vec![val_string("t2"), val_flags(4, &[0, 2, 3])])
            ])]
    );
}

#[tokio::test]
#[tracing::instrument]
async fn variants_with_no_payloads() {
    let context = TestContext::new();
    let executor = start(&context).await.unwrap();

    let template_id = executor.store_template("variant-service").await;
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
#[tracing::instrument]
async fn delete_instance() {
    let context = TestContext::new();
    let executor = start(&context).await.unwrap();

    let template_id = executor.store_template("option-service").await;
    let worker_id = executor
        .start_worker(&template_id, "delete-instance-1")
        .await;

    let _ = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api/echo",
            vec![val_option(Some(val_string("Hello")))],
        )
        .await
        .unwrap();

    let metadata1 = executor.get_worker_metadata(&worker_id).await;

    let (cursor1, metadatas1) = executor
        .get_worker_metadatas(
            &worker_id.template_id,
            Some(WorkerFilter::new_name(
                StringFilterComparator::Equal,
                worker_id.worker_name.clone(),
            )),
            0,
            10,
            true,
        )
        .await;

    executor.delete_worker(&worker_id).await;

    let metadata2 = executor.get_worker_metadata(&worker_id).await;

    check!(metadatas1.len() == 1);
    check!(cursor1.is_none());
    check!(metadata1.is_some());
    check!(metadata2.is_none());
}

#[tokio::test]
#[tracing::instrument]
async fn get_workers() {
    async fn get_check(
        template_id: &TemplateId,
        filter: Option<WorkerFilter>,
        expected_count: usize,
        executor: &mut TestWorkerExecutor,
    ) -> Vec<WorkerMetadata> {
        let (cursor, metadatas) = executor
            .get_worker_metadatas(template_id, filter, 0, 20, true)
            .await;

        check!(metadatas.len() == expected_count);
        check!(cursor.is_none());

        metadatas
    }

    let context = TestContext::new();
    let mut executor = start(&context).await.unwrap();

    let template_id = executor.store_template("option-service").await;

    let workers_count = 10;
    let mut worker_ids = vec![];

    for i in 0..workers_count {
        let worker_id = executor
            .start_worker(&template_id, &format!("test-instance-{}", i))
            .await;

        worker_ids.push(worker_id);
    }

    for worker_id in worker_ids.clone() {
        let _ = executor
            .invoke_and_await(
                &worker_id,
                "golem:it/api/echo",
                vec![val_option(Some(val_string("Hello")))],
            )
            .await
            .unwrap();

        get_check(
            &template_id,
            Some(WorkerFilter::new_name(
                StringFilterComparator::Equal,
                worker_id.worker_name.clone(),
            )),
            1,
            &mut executor,
        )
        .await;
    }

    get_check(
        &template_id,
        Some(WorkerFilter::new_name(
            StringFilterComparator::Like,
            "test".to_string(),
        )),
        workers_count,
        &mut executor,
    )
    .await;

    get_check(
        &template_id,
        Some(
            WorkerFilter::new_name(StringFilterComparator::Like, "test".to_string())
                .and(
                    WorkerFilter::new_status(WorkerStatus::Idle)
                        .or(WorkerFilter::new_status(WorkerStatus::Running)),
                )
                .and(WorkerFilter::new_version(FilterComparator::Equal, 0)),
        ),
        workers_count,
        &mut executor,
    )
    .await;

    get_check(
        &template_id,
        Some(WorkerFilter::new_name(StringFilterComparator::Like, "test".to_string()).not()),
        0,
        &mut executor,
    )
    .await;

    get_check(&template_id, None, workers_count, &mut executor).await;

    let (cursor1, metadatas1) = executor
        .get_worker_metadatas(&template_id, None, 0, (workers_count / 2) as u64, true)
        .await;

    check!(cursor1.is_some());
    check!(metadatas1.len() == workers_count / 2);

    let (cursor2, metadatas2) = executor
        .get_worker_metadatas(
            &template_id,
            None,
            cursor1.unwrap(),
            (workers_count - metadatas1.len()) as u64,
            true,
        )
        .await;

    check!(metadatas2.len() == workers_count - metadatas1.len());

    if let Some(cursor2) = cursor2 {
        let (_, metadatas3) = executor
            .get_worker_metadatas(&template_id, None, cursor2, workers_count as u64, true)
            .await;
        check!(metadatas3.len() == 0);
    }

    for worker_id in worker_ids {
        executor.delete_worker(&worker_id).await;
    }

    get_check(&template_id, None, 0, &mut executor).await;
}

#[tokio::test]
#[tracing::instrument]
async fn error_handling_when_worker_is_invoked_with_fewer_than_expected_parameters() {
    let context = TestContext::new();
    let executor = start(&context).await.unwrap();

    let template_id = executor.store_template("option-service").await;
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
#[tracing::instrument]
async fn error_handling_when_worker_is_invoked_with_more_than_expected_parameters() {
    let context = TestContext::new();
    let executor = start(&context).await.unwrap();

    let template_id = executor.store_template("option-service").await;
    let worker_id = executor
        .start_worker(&template_id, "more-than-expected-parameters-1")
        .await;

    let failure = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api/echo",
            vec![
                val_option(Some(val_string("Hello"))),
                val_string("extra parameter"),
            ],
        )
        .await;
    drop(executor);

    check!(failure.is_err());
}

#[tokio::test]
#[tracing::instrument]
async fn get_instance_metadata() {
    let context = TestContext::new();
    let executor = start(&context).await.unwrap();

    let template_id = executor.store_template("clock-service").await;
    let worker_id = executor
        .start_worker(&template_id, "get-instance-metadata-1")
        .await;

    let worker_id_clone = worker_id.clone();
    let executor_clone = executor.clone();
    let fiber = tokio::spawn(async move {
        executor_clone
            .invoke_and_await(&worker_id_clone, "golem:it/api/sleep", vec![val_u64(10)])
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
#[tracing::instrument]
async fn create_invoke_delete_create_invoke() {
    let context = TestContext::new();
    let executor = start(&context).await.unwrap();

    let template_id = executor.store_template("shopping-cart").await;
    let worker_id = executor
        .start_worker(&template_id, "create-invoke-delete-create-invoke-1")
        .await;

    let r1 = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api/add-item",
            vec![val_record(vec![
                val_string("G1000"),
                val_string("Golem T-Shirt M"),
                val_float32(100.0),
                val_u32(5),
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
            vec![val_record(vec![
                val_string("G1000"),
                val_string("Golem T-Shirt M"),
                val_float32(100.0),
                val_u32(5),
            ])],
        )
        .await;

    drop(executor);

    check!(r1.is_ok());
    check!(r2.is_ok());
}

#[tokio::test]
#[tracing::instrument]
async fn recovering_an_old_instance_after_updating_a_template() {
    let context = TestContext::new();
    let executor = start(&context).await.unwrap();

    let template_id = executor.store_template("shopping-cart").await;
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
            vec![val_record(vec![
                val_string("G1000"),
                val_string("Golem T-Shirt M"),
                val_float32(100.0),
                val_u32(5),
            ])],
        )
        .await
        .unwrap();

    // Updating the template with an incompatible new version
    executor
        .update_template(&template_id, "option-service")
        .await;

    // Creating a new worker of the updated template and call it
    let worker_id2 = executor
        .start_worker(
            &template_id,
            "recovering-an-old-instance-after-updating-a-template-2",
        )
        .await;

    let r2 = executor
        .invoke_and_await(
            &worker_id2,
            "golem:it/api/echo",
            vec![val_option(Some(val_string("Hello")))],
        )
        .await
        .unwrap();

    // Restarting the server to force worker recovery
    drop(executor);
    let executor = start(&context).await.unwrap();

    // Call the first worker again to check if it is still working
    let r3 = executor
        .invoke_and_await(&worker_id, "golem:it/api/get-cart-contents", vec![])
        .await
        .unwrap();

    drop(executor);

    check!(r1 == vec![]);
    check!(r2 == vec![val_option(Some(val_string("Hello")))]);
    check!(
        r3 == vec![val_list(vec![val_record(vec![
            val_string("G1000"),
            val_string("Golem T-Shirt M"),
            val_float32(100.0),
            val_u32(5),
        ])])]
    );
}

#[tokio::test]
#[tracing::instrument]
async fn recreating_an_instance_after_it_got_deleted_with_a_different_version() {
    let context = TestContext::new();
    let executor = start(&context).await.unwrap();

    let template_id = executor.store_template("shopping-cart").await;
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
            vec![val_record(vec![
                val_string("G1000"),
                val_string("Golem T-Shirt M"),
                val_float32(100.0),
                val_u32(5),
            ])],
        )
        .await
        .unwrap();

    // Updating the template with an incompatible new version
    executor
        .update_template(&template_id, "option-service")
        .await;

    // Deleting the first instance
    executor.delete_worker(&worker_id).await;

    // Create a new instance with the same name and call it the first instance again to check if it is still working
    let worker_id = executor
        .start_worker(
            &template_id,
            "recovering-an-old-instance-after-updating-a-template-1",
        )
        .await;

    let r2 = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api/echo",
            vec![val_option(Some(val_string("Hello")))],
        )
        .await
        .unwrap();

    drop(executor);

    check!(r1 == vec![]);
    check!(r2 == vec![val_option(Some(val_string("Hello")))]);
}

#[tokio::test]
#[tracing::instrument]
async fn trying_to_use_an_old_wasm_provides_good_error_message() {
    let context = TestContext::new();
    // case: WASM is an old version, rejected by protector

    let executor = start(&context).await.unwrap();

    let template_id = executor.store_template_unverified("old-component").await;
    let result = executor
        .try_start_worker(&template_id, "old-component-1")
        .await;

    check!(result.is_err());
    check!(is_worker_execution_error(
        &result.err().unwrap(),
        &worker_execution_error::Error::TemplateParseFailed(TemplateParseFailed {
            template_id: Some(template_id.into()),
            template_version: 0,
            reason: "failed to parse WebAssembly module".to_string()
        })
    ));
}

#[tokio::test]
#[tracing::instrument]
async fn trying_to_use_a_wasm_that_wasmtime_cannot_load_provides_good_error_message() {
    let context = TestContext::new();
    // case: WASM can be parsed but wasmtime does not support it
    let executor = start(&context).await.unwrap();
    let template_id = executor.store_template("write-stdout").await;

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
    check!(is_worker_execution_error(
        &result.err().unwrap(),
        &worker_execution_error::Error::TemplateParseFailed(TemplateParseFailed {
            template_id: Some(template_id.into()),
            template_version: 0,
            reason: "failed to parse WebAssembly module".to_string()
        })
    ));
}

#[tokio::test]
#[tracing::instrument]
async fn trying_to_use_a_wasm_that_wasmtime_cannot_load_provides_good_error_message_after_recovery()
{
    let context = TestContext::new();
    let executor = start(&context).await.unwrap();
    let template_id = executor.store_template("write-stdout").await;

    let worker_id = executor
        .try_start_worker(&template_id, "bad-wasm-2")
        .await
        .unwrap();

    // worker is idle. if we restart the server it won't get recovered
    drop(executor);
    let executor = start(&context).await.unwrap();

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
    check!(is_worker_execution_error(
        &result.err().unwrap(),
        &worker_execution_error::Error::TemplateParseFailed(TemplateParseFailed {
            template_id: Some(template_id.into()),
            template_version: 0,
            reason: "failed to parse WebAssembly module".to_string()
        })
    ));
}

#[tokio::test]
#[tracing::instrument]
async fn long_running_poll_loop_works_as_expected() {
    let context = TestContext::new();
    let executor = start(&context).await.unwrap();

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

    let template_id = executor.store_template("http-client-2").await;
    let mut env = HashMap::new();
    env.insert("PORT".to_string(), host_http_port.to_string());

    let worker_id = executor
        .start_worker_with(&template_id, "poll-loop-template-0", vec![], env)
        .await;

    executor.log_output(&worker_id).await;

    executor
        .invoke(
            &worker_id,
            "golem:it/api/start-polling",
            vec![val_string("first")],
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
#[tracing::instrument]
async fn long_running_poll_loop_interrupting_and_resuming_by_second_invocation() {
    let context = TestContext::new();
    let executor = start(&context).await.unwrap();

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

    let template_id = executor.store_template("http-client-2").await;
    let mut env = HashMap::new();
    env.insert("PORT".to_string(), host_http_port.to_string());
    let worker_id = executor
        .start_worker_with(&template_id, "poll-loop-template-1", vec![], env)
        .await;

    executor.log_output(&worker_id).await;

    executor
        .invoke(
            &worker_id,
            "golem:it/api/start-polling",
            vec![val_string("first")],
        )
        .await
        .unwrap();

    sleep(Duration::from_secs(2)).await;
    let status1 = executor.get_worker_metadata(&worker_id).await.unwrap();
    let metadatas1 = executor
        .get_running_worker_metadatas(
            &worker_id.template_id,
            Some(WorkerFilter::new_name(
                StringFilterComparator::Equal,
                worker_id.worker_name.clone(),
            )),
        )
        .await;

    sleep(Duration::from_secs(4)).await;
    executor.interrupt(&worker_id).await;

    sleep(Duration::from_secs(2)).await;
    let status2 = executor.get_worker_metadata(&worker_id).await.unwrap();
    let metadatas2 = executor
        .get_running_worker_metadatas(
            &worker_id.template_id,
            Some(WorkerFilter::new_name(
                StringFilterComparator::Equal,
                worker_id.worker_name.clone(),
            )),
        )
        .await;

    let executor_clone = executor.clone();
    let worker_id_clone = worker_id.clone();
    let fiber = spawn(async move {
        // Invoke blocks until the invocation starts
        executor_clone
            .invoke(
                &worker_id_clone,
                "golem:it/api/start-polling",
                vec![val_string("second")],
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

    check!(status1.last_known_status.status == WorkerStatus::Running);
    check!(!metadatas1.is_empty());
    // first running
    check!(status2.last_known_status.status == WorkerStatus::Interrupted);
    check!(metadatas2.is_empty());
    // first interrupted
    check!(status3.last_known_status.status == WorkerStatus::Running);
    // first resumed
    check!(status4.last_known_status.status == WorkerStatus::Running);
    // second running
    check!(status5.last_known_status.status == WorkerStatus::Idle); // second finished
}

#[tokio::test]
#[tracing::instrument]
async fn long_running_poll_loop_connection_breaks_on_interrupt() {
    let context = TestContext::new();
    let executor = start(&context).await.unwrap();

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

    let template_id = executor.store_template("http-client-2").await;
    let mut env = HashMap::new();
    env.insert("PORT".to_string(), host_http_port.to_string());
    let worker_id = executor
        .start_worker_with(&template_id, "poll-loop-template-2", vec![], env)
        .await;

    let rx = executor.capture_output_with_termination(&worker_id).await;

    executor
        .invoke(
            &worker_id,
            "golem:it/api/start-polling",
            vec![val_string("first")],
        )
        .await
        .unwrap();

    sleep(Duration::from_secs(2)).await;

    executor.interrupt(&worker_id).await;

    let events = drain_connection(rx).await;

    drop(executor);
    http_server.abort();

    check!(events.contains(&Some(stdout_event("Calling the poll endpoint\n"))));
    check!(events.contains(&Some(stdout_event("Received initial\n"))));
}

#[tokio::test]
#[tracing::instrument]
async fn long_running_poll_loop_connection_retry_does_not_resume_interrupted_worker() {
    let context = TestContext::new();
    let executor = start(&context).await.unwrap();

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

    let template_id = executor.store_template("http-client-2").await;
    let mut env = HashMap::new();
    env.insert("PORT".to_string(), host_http_port.to_string());

    let worker_id = executor
        .start_worker_with(&template_id, "poll-loop-template-3", vec![], env)
        .await;

    let rx = executor.capture_output_with_termination(&worker_id).await;

    executor
        .invoke(
            &worker_id,
            "golem:it/api/start-polling",
            vec![val_string("first")],
        )
        .await
        .unwrap();

    sleep(Duration::from_secs(2)).await;

    executor.interrupt(&worker_id).await;

    let _ = drain_connection(rx).await;
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
#[tracing::instrument]
async fn long_running_poll_loop_connection_can_be_restored_after_resume() {
    let context = TestContext::new();
    let executor = start(&context).await.unwrap();

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

    let template_id = executor.store_template("http-client-2").await;
    let mut env = HashMap::new();
    env.insert("PORT".to_string(), host_http_port.to_string());

    let worker_id = executor
        .start_worker_with(&template_id, "poll-loop-template-4", vec![], env)
        .await;

    let rx = executor.capture_output_with_termination(&worker_id).await;

    executor
        .invoke(
            &worker_id,
            "golem:it/api/start-polling",
            vec![val_string("first")],
        )
        .await
        .unwrap();

    sleep(Duration::from_secs(2)).await;
    let status1 = executor.get_worker_metadata(&worker_id).await.unwrap();

    executor.interrupt(&worker_id).await;

    let mut events = drain_connection(rx).await;
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
    check!(events.contains(&stdout_event("Calling the poll endpoint\n")));
    check!(events.contains(&stdout_event("Received initial\n")));
    check!(events.contains(&stdout_event("Poll loop finished\n")));
}

#[tokio::test]
#[tracing::instrument]
async fn long_running_poll_loop_worker_can_be_deleted_after_interrupt() {
    let context = TestContext::new();
    let executor = start(&context).await.unwrap();

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

    let template_id = executor.store_template("http-client-2").await;
    let mut env = HashMap::new();
    env.insert("PORT".to_string(), host_http_port.to_string());

    let worker_id = executor
        .start_worker_with(&template_id, "poll-loop-template-5", vec![], env)
        .await;

    let rx = executor.capture_output_with_termination(&worker_id).await;

    executor
        .invoke(
            &worker_id,
            "golem:it/api/start-polling",
            vec![val_string("first")],
        )
        .await
        .unwrap();

    sleep(Duration::from_secs(2)).await;

    executor.interrupt(&worker_id).await;

    let _ = drain_connection(rx).await;

    executor.delete_worker(&worker_id).await;
    let metadata = executor.get_worker_metadata(&worker_id).await;

    drop(executor);
    http_server.abort();

    check!(metadata.is_none());
}

#[tokio::test]
#[tracing::instrument]
async fn shopping_cart_resource_example() {
    let context = TestContext::new();
    let executor = start(&context).await.unwrap();

    let template_id = executor.store_template("shopping-cart-resource").await;
    let worker_id = executor
        .start_worker(&template_id, "shopping-cart-resource-1")
        .await;

    let cart = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api/[constructor]cart",
            vec![val_string("test-user-1")],
        )
        .await
        .unwrap();
    println!("cart: {:?}", cart);

    let _ = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api/[method]cart.add-item",
            vec![
                cart[0].clone(),
                val_record(vec![
                    val_string("G1000"),
                    val_string("Golem T-Shirt M"),
                    val_float32(100.0),
                    val_u32(5),
                ]),
            ],
        )
        .await;

    let _ = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api/[method]cart.add-item",
            vec![
                cart[0].clone(),
                val_record(vec![
                    val_string("G1001"),
                    val_string("Golem Cloud Subscription 1y"),
                    val_float32(999999.0),
                    val_u32(1),
                ]),
            ],
        )
        .await;

    let _ = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api/[method]cart.add-item",
            vec![
                cart[0].clone(),
                val_record(vec![
                    val_string("G1002"),
                    val_string("Mud Golem"),
                    val_float32(11.0),
                    val_u32(10),
                ]),
            ],
        )
        .await;

    let _ = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api/[method]cart.update-item-quantity",
            vec![cart[0].clone(), val_string("G1002"), val_u32(20)],
        )
        .await;

    let contents = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api/[method]cart.get-cart-contents",
            vec![cart[0].clone()],
        )
        .await;

    let _ = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api/[method]cart.checkout",
            vec![cart[0].clone()],
        )
        .await;

    drop(executor);

    assert!(
        contents
            == Ok(vec![val_list(vec![
                val_record(vec![
                    val_string("G1000"),
                    val_string("Golem T-Shirt M"),
                    val_float32(100.0),
                    val_u32(5),
                ]),
                val_record(vec![
                    val_string("G1001"),
                    val_string("Golem Cloud Subscription 1y"),
                    val_float32(999999.0),
                    val_u32(1),
                ]),
                val_record(vec![
                    val_string("G1002"),
                    val_string("Mud Golem"),
                    val_float32(11.0),
                    val_u32(20),
                ]),
            ])])
    )
}

#[tokio::test]
#[tracing::instrument]
async fn counter_resource_test_1() {
    let context = TestContext::new();
    let executor = start(&context).await.unwrap();

    let template_id = executor.store_template("counters").await;
    let worker_id = executor.start_worker(&template_id, "counters-1").await;
    executor.log_output(&worker_id).await;

    let counter1 = executor
        .invoke_and_await(
            &worker_id,
            "rpc:counters/api/[constructor]counter",
            vec![val_string("counter1")],
        )
        .await
        .unwrap();
    let _ = executor
        .invoke_and_await(
            &worker_id,
            "rpc:counters/api/[method]counter.inc-by",
            vec![counter1[0].clone(), val_u64(5)],
        )
        .await;

    let result1 = executor
        .invoke_and_await(
            &worker_id,
            "rpc:counters/api/[method]counter.get-value",
            vec![counter1[0].clone()],
        )
        .await;

    let _ = executor
        .invoke_and_await(
            &worker_id,
            "rpc:counters/api/[drop]counter",
            vec![counter1[0].clone()],
        )
        .await;

    let result2 = executor
        .invoke_and_await(&worker_id, "rpc:counters/api/get-all-dropped", vec![])
        .await;

    drop(executor);

    check!(result1 == Ok(vec![val_u64(5)]));
    check!(
        result2
            == Ok(vec![val_list(vec![val_pair(
                val_string("counter1"),
                val_u64(5)
            )])])
    );
}

#[tokio::test]
#[tracing::instrument]
async fn reconstruct_interrupted_state() {
    let context = TestContext::new();
    let executor = start(&context).await.unwrap();

    let template_id = executor.store_template("interruption").await;
    let worker_id = executor.start_worker(&template_id, "interruption-1").await;

    let executor_clone = executor.clone();
    let worker_id_clone = worker_id.clone();
    let fiber = tokio::spawn(async move {
        executor_clone
            .invoke_and_await(&worker_id_clone, "run", vec![])
            .await
    });

    tokio::time::sleep(Duration::from_secs(2)).await;

    let _ = executor.interrupt(&worker_id).await;
    let result = fiber.await.unwrap();

    // Explicitly deleting the status information from Redis to check if it can be
    // reconstructed from Redis

    let mut redis = executor.redis().get_connection(0);
    let _: () = redis
        .del(format!(
            "{}instance:status:{}",
            context.redis_prefix(),
            worker_id.to_redis_key()
        ))
        .unwrap();
    debug!("Deleted status information from Redis");

    let status = executor
        .get_worker_metadata(&worker_id)
        .await
        .unwrap()
        .last_known_status
        .status;

    drop(executor);
    check!(result.is_err());
    check!(worker_error_message(&result.err().unwrap()).contains("Interrupted via the Golem API"));
    check!(status == WorkerStatus::Interrupted);
}
