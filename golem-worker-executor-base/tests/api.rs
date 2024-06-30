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

use golem_api_grpc::proto::golem::worker::{
    worker_execution_error, ComponentParseFailed, LogEvent,
};
use golem_api_grpc::proto::golem::workerexecutor::CompletePromiseRequest;
use golem_common::model::{
    AccountId, ComponentId, FilterComparator, IdempotencyKey, PromiseId, ScanCursor,
    StringFilterComparator, WorkerFilter, WorkerId, WorkerMetadata, WorkerStatus,
};
use golem_wasm_rpc::Value;

use crate::common::{start, TestContext, TestWorkerExecutor};
use golem_common::model::oplog::OplogIndex;
use golem_test_framework::config::TestDependencies;
use golem_test_framework::dsl::{
    drain_connection, is_worker_execution_error, stdout_event, worker_error_message, TestDsl,
};
use tokio::time::sleep;
use tonic::transport::Body;
use tracing::debug;
use warp::Filter;
use wasmtime_wasi::runtime::spawn;

#[tokio::test]
#[tracing::instrument]
async fn interruption() {
    let context = TestContext::new();
    let executor = start(&context).await.unwrap();

    let component_id = executor.store_component("interruption").await;
    let worker_id = executor.start_worker(&component_id, "interruption-1").await;

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

    let component_id = executor.store_component("interruption").await;
    let worker_id = executor
        .start_worker(&component_id, "simulated-crash-1")
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
    check!(result == Ok(vec![Value::String("done".to_string())]));
    check!(events == vec![stdout_event("Starting interruption test\n"),]);
    check!(elapsed.as_secs() < 13);
}

#[tokio::test]
#[tracing::instrument]
async fn shopping_cart_example() {
    let context = TestContext::new();
    let executor = start(&context).await.unwrap();

    let component_id = executor.store_component("shopping-cart").await;
    let worker_id = executor
        .start_worker(&component_id, "shopping-cart-1")
        .await;

    let _ = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{initialize-cart}",
            vec![Value::String("test-user-1".to_string())],
        )
        .await;

    let _ = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{add-item}",
            vec![Value::Record(vec![
                Value::String("G1000".to_string()),
                Value::String("Golem T-Shirt M".to_string()),
                Value::F32(100.0),
                Value::U32(5),
            ])],
        )
        .await;

    let _ = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{add-item}",
            vec![Value::Record(vec![
                Value::String("G1001".to_string()),
                Value::String("Golem Cloud Subscription 1y".to_string()),
                Value::F32(999999.0),
                Value::U32(1),
            ])],
        )
        .await;

    let _ = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{add-item}",
            vec![Value::Record(vec![
                Value::String("G1002".to_string()),
                Value::String("Mud Golem".to_string()),
                Value::F32(11.0),
                Value::U32(10),
            ])],
        )
        .await;

    let _ = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{update-item-quantity}",
            vec![Value::String("G1002".to_string()), Value::U32(20)],
        )
        .await;

    let contents = executor
        .invoke_and_await(&worker_id, "golem:it/api.{get-cart-contents}", vec![])
        .await;

    let _ = executor
        .invoke_and_await(&worker_id, "golem:it/api.{checkout}", vec![])
        .await;

    drop(executor);

    assert!(
        contents
            == Ok(vec![Value::List(vec![
                Value::Record(vec![
                    Value::String("G1000".to_string()),
                    Value::String("Golem T-Shirt M".to_string()),
                    Value::F32(100.0),
                    Value::U32(5),
                ]),
                Value::Record(vec![
                    Value::String("G1001".to_string()),
                    Value::String("Golem Cloud Subscription 1y".to_string()),
                    Value::F32(999999.0),
                    Value::U32(1),
                ]),
                Value::Record(vec![
                    Value::String("G1002".to_string()),
                    Value::String("Mud Golem".to_string()),
                    Value::F32(11.0),
                    Value::U32(20),
                ]),
            ])])
    )
}

#[tokio::test]
#[tracing::instrument]
async fn stdio_cc() {
    let context = TestContext::new();
    let executor = start(&context).await.unwrap();

    let component_id = executor.store_component("stdio-cc").await;
    let worker_id = executor.start_worker(&component_id, "stdio-cc-1").await;

    let result = executor
        .invoke_and_await_stdio(&worker_id, "run", serde_json::Value::Number(1234.into()))
        .await;

    drop(executor);

    assert!(result == Ok(serde_json::Value::Number(2468.into())))
}

#[tokio::test]
#[tracing::instrument]
async fn dynamic_worker_creation() {
    let context = TestContext::new();
    let executor = start(&context).await.unwrap();

    let component_id = executor.store_component("environment-service").await;
    let worker_id = WorkerId {
        component_id: component_id.clone(),
        worker_name: "dynamic-worker-creation-1".to_string(),
    };

    let args = executor
        .invoke_and_await(&worker_id, "golem:it/api.{get-arguments}", vec![])
        .await
        .unwrap();
    let env = executor
        .invoke_and_await(&worker_id, "golem:it/api.{get-environment}", vec![])
        .await
        .unwrap();

    drop(executor);

    check!(args == vec![Value::Result(Ok(Some(Box::new(Value::List(vec![])))))]);
    check!(
        env == vec![Value::Result(Ok(Some(Box::new(Value::List(vec![
            Value::Tuple(vec![
                Value::String("GOLEM_WORKER_NAME".to_string()),
                Value::String("dynamic-worker-creation-1".to_string())
            ]),
            Value::Tuple(vec![
                Value::String("GOLEM_COMPONENT_ID".to_string()),
                Value::String(format!("{}", component_id))
            ]),
            Value::Tuple(vec![
                Value::String("GOLEM_COMPONENT_VERSION".to_string()),
                Value::String("0".to_string())
            ]),
        ])))))]
    );
}

#[tokio::test]
#[tracing::instrument]
async fn promise() {
    let context = TestContext::new();
    let executor = start(&context).await.unwrap();

    let component_id = executor.store_component("promise").await;
    let worker_id = executor.start_worker(&component_id, "promise-1").await;

    let executor_clone = executor.clone();
    let worker_id_clone = worker_id.clone();
    let fiber = tokio::spawn(async move {
        executor_clone
            .invoke_and_await(&worker_id_clone, "run", vec![])
            .await
    });

    sleep(Duration::from_secs(10)).await;

    executor
        .client()
        .await
        .complete_promise(CompletePromiseRequest {
            promise_id: Some(
                PromiseId {
                    worker_id: worker_id.clone(),
                    oplog_idx: OplogIndex::from_u64(3),
                }
                .into(),
            ),
            data: vec![42],
            account_id: Some(
                AccountId {
                    value: "test-account".to_string(),
                }
                .into(),
            ),
        })
        .await
        .unwrap();

    let result = fiber.await.unwrap();

    drop(executor);

    check!(result == Ok(vec![Value::List(vec![Value::U8(42)])]));
}

#[tokio::test]
#[tracing::instrument]
async fn get_self_uri() {
    let context = TestContext::new();
    let executor = start(&context).await.unwrap();

    let component_id = executor.store_component("runtime-service").await;
    let worker_id = executor
        .start_worker(&component_id, "runtime-service-1")
        .await;

    let result = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{get-self-uri}",
            vec![Value::String("function-name".to_string())],
        )
        .await
        .unwrap();

    drop(executor);

    check!(
        result
            == vec![Value::String(format!(
                "worker://{component_id}/runtime-service-1/function-name"
            ))]
    );
}

#[tokio::test]
#[tracing::instrument]
async fn get_workers_from_worker() {
    let context = TestContext::new();
    let mut executor = start(&context).await.unwrap();

    let component_id = executor.store_component("runtime-service").await;

    let worker_id1 = executor
        .start_worker(&component_id, "runtime-service-1")
        .await;

    let worker_id2 = executor
        .start_worker(&component_id, "runtime-service-2")
        .await;

    async fn get_check(
        worker_id: &WorkerId,
        name_filter: Option<String>,
        expected_count: usize,
        executor: &mut TestWorkerExecutor,
    ) {
        let component_id_val = {
            let (high, low) = worker_id.component_id.0.as_u64_pair();
            Value::Record(vec![Value::Record(vec![Value::U64(high), Value::U64(low)])])
        };

        let filter_val = name_filter.map(|name| {
            Value::Record(vec![Value::List(vec![Value::Record(vec![Value::List(
                vec![Value::Variant {
                    case_idx: 0,
                    case_value: Some(Box::new(Value::Record(vec![
                        Value::Enum(0),
                        Value::String(name.clone()),
                    ]))),
                }],
            )])])])
        });

        let result = executor
            .invoke_and_await(
                worker_id,
                "golem:it/api.{get-workers}",
                vec![
                    component_id_val,
                    Value::Option(filter_val.map(Box::new)),
                    Value::Bool(true),
                ],
            )
            .await
            .unwrap();

        println!("result: {:?}", result.clone());

        match result.first() {
            Some(Value::List(list)) => {
                check!(list.len() == expected_count);
            }
            _ => {
                check!(false);
            }
        }
    }

    get_check(&worker_id1, None, 2, &mut executor).await;
    get_check(
        &worker_id2,
        Some("runtime-service-1".to_string()),
        1,
        &mut executor,
    )
    .await;

    drop(executor);
}

#[tokio::test]
#[tracing::instrument]
async fn invoking_with_same_idempotency_key_is_idempotent() {
    let context = TestContext::new();
    let executor = start(&context).await.unwrap();

    let component_id = executor.store_component("shopping-cart").await;
    let worker_id = executor
        .start_worker(&component_id, "shopping-cart-2")
        .await;

    let idempotency_key = IdempotencyKey::fresh();
    let _result = executor
        .invoke_and_await_with_key(
            &worker_id,
            &idempotency_key,
            "golem:it/api.{add-item}",
            vec![Value::Record(vec![
                Value::String("G1000".to_string()),
                Value::String("Golem T-Shirt M".to_string()),
                Value::F32(100.0),
                Value::U32(5),
            ])],
        )
        .await
        .unwrap();

    let _result2 = executor
        .invoke_and_await_with_key(
            &worker_id,
            &idempotency_key,
            "golem:it/api.{add-item}",
            vec![Value::Record(vec![
                Value::String("G1000".to_string()),
                Value::String("Golem T-Shirt M".to_string()),
                Value::F32(100.0),
                Value::U32(5),
            ])],
        )
        .await
        .unwrap();

    let contents = executor
        .invoke_and_await(&worker_id, "golem:it/api.{get-cart-contents}", vec![])
        .await
        .unwrap();

    drop(executor);

    check!(
        contents
            == vec![Value::List(vec![Value::Record(vec![
                Value::String("G1000".to_string()),
                Value::String("Golem T-Shirt M".to_string()),
                Value::F32(100.0),
                Value::U32(5),
            ])])]
    );
}

#[tokio::test]
#[tracing::instrument]
async fn invoking_with_same_idempotency_key_is_idempotent_after_restart() {
    let context = TestContext::new();
    let executor = start(&context).await.unwrap();

    let component_id = executor.store_component("shopping-cart").await;
    let worker_id = executor
        .start_worker(&component_id, "shopping-cart-4")
        .await;

    let idempotency_key = IdempotencyKey::fresh();
    let _result = executor
        .invoke_and_await_with_key(
            &worker_id,
            &idempotency_key,
            "golem:it/api.{add-item}",
            vec![Value::Record(vec![
                Value::String("G1000".to_string()),
                Value::String("Golem T-Shirt M".to_string()),
                Value::F32(100.0),
                Value::U32(5),
            ])],
        )
        .await
        .unwrap();

    drop(executor);
    let executor = start(&context).await.unwrap();

    let _result2 = executor
        .invoke_and_await_with_key(
            &worker_id,
            &idempotency_key,
            "golem:it/api.{add-item}",
            vec![Value::Record(vec![
                Value::String("G1000".to_string()),
                Value::String("Golem T-Shirt M".to_string()),
                Value::F32(100.0),
                Value::U32(5),
            ])],
        )
        .await
        .unwrap();

    let contents = executor
        .invoke_and_await(&worker_id, "golem:it/api.{get-cart-contents}", vec![])
        .await
        .unwrap();

    drop(executor);

    check!(
        contents
            == vec![Value::List(vec![Value::Record(vec![
                Value::String("G1000".to_string()),
                Value::String("Golem T-Shirt M".to_string()),
                Value::F32(100.0),
                Value::U32(5),
            ])])]
    );
}

#[tokio::test]
#[tracing::instrument]
async fn optional_parameters() {
    let context = TestContext::new();
    let executor = start(&context).await.unwrap();

    let component_id = executor.store_component("option-service").await;
    let worker_id = executor
        .start_worker(&component_id, "optional-service-1")
        .await;

    let echo_some = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{echo}",
            vec![Value::Option(Some(Box::new(Value::String(
                "Hello".to_string(),
            ))))],
        )
        .await
        .unwrap();

    let echo_none = executor
        .invoke_and_await(&worker_id, "golem:it/api.{echo}", vec![Value::Option(None)])
        .await
        .unwrap();

    let todo_some = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{todo}",
            vec![Value::Record(vec![
                Value::String("todo".to_string()),
                Value::Option(Some(Box::new(Value::String("description".to_string())))),
            ])],
        )
        .await
        .unwrap();

    let todo_none = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{todo}",
            vec![Value::Record(vec![
                Value::String("todo".to_string()),
                Value::Option(Some(Box::new(Value::String("description".to_string())))),
            ])],
        )
        .await
        .unwrap();

    drop(executor);

    check!(
        echo_some
            == vec![Value::Option(Some(Box::new(Value::String(
                "Hello".to_string()
            ))))]
    );
    check!(echo_none == vec![Value::Option(None)]);
    check!(todo_some == vec![Value::String("todo".to_string())]);
    check!(todo_none == vec![Value::String("todo".to_string())]);
}

#[tokio::test]
#[tracing::instrument]
async fn flags_parameters() {
    let context = TestContext::new();
    let executor = start(&context).await.unwrap();

    let component_id = executor.store_component("flags-service").await;
    let worker_id = executor
        .start_worker(&component_id, "flags-service-1")
        .await;

    let create_task = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{create-task}",
            vec![Value::Record(vec![
                Value::String("t1".to_string()),
                Value::Flags(vec![true, true, false, false]),
            ])],
        )
        .await
        .unwrap();

    let get_tasks = executor
        .invoke_and_await(&worker_id, "golem:it/api.{get-tasks}", vec![])
        .await
        .unwrap();

    drop(executor);

    check!(
        create_task
            == vec![Value::Record(vec![
                Value::String("t1".to_string()),
                Value::Flags(vec![true, true, true, false])
            ])]
    );
    check!(
        get_tasks
            == vec![Value::List(vec![
                Value::Record(vec![
                    Value::String("t1".to_string()),
                    Value::Flags(vec![true, false, false, false])
                ]),
                Value::Record(vec![
                    Value::String("t2".to_string()),
                    Value::Flags(vec![true, false, true, true])
                ])
            ])]
    );
}

#[tokio::test]
#[tracing::instrument]
async fn variants_with_no_payloads() {
    let context = TestContext::new();
    let executor = start(&context).await.unwrap();

    let component_id = executor.store_component("variant-service").await;
    let worker_id = executor
        .start_worker(&component_id, "variant-service-1")
        .await;

    let result = executor
        .invoke_and_await(&worker_id, "golem:it/api.{bid}", vec![])
        .await;

    drop(executor);

    check!(result.is_ok());
}

#[tokio::test]
#[tracing::instrument]
async fn delete_worker() {
    let context = TestContext::new();
    let executor = start(&context).await.unwrap();

    let component_id = executor.store_component("option-service").await;
    let worker_id = executor
        .start_worker(&component_id, "delete-worker-1")
        .await;

    let _ = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{echo}",
            vec![Value::Option(Some(Box::new(Value::String(
                "Hello".to_string(),
            ))))],
        )
        .await
        .unwrap();

    let metadata1 = executor.get_worker_metadata(&worker_id).await;

    let (cursor1, values1) = executor
        .get_workers_metadata(
            &worker_id.component_id,
            Some(WorkerFilter::new_name(
                StringFilterComparator::Equal,
                worker_id.worker_name.clone(),
            )),
            ScanCursor::default(),
            10,
            true,
        )
        .await;

    executor.delete_worker(&worker_id).await;

    let metadata2 = executor.get_worker_metadata(&worker_id).await;

    check!(values1.len() == 1);
    check!(cursor1.is_none());
    check!(metadata1.is_some());
    check!(metadata2.is_none());
}

#[tokio::test]
#[tracing::instrument]
async fn get_workers() {
    async fn get_check(
        component_id: &ComponentId,
        filter: Option<WorkerFilter>,
        expected_count: usize,
        executor: &mut TestWorkerExecutor,
    ) -> Vec<WorkerMetadata> {
        let (cursor, values) = executor
            .get_workers_metadata(component_id, filter, ScanCursor::default(), 20, true)
            .await;

        check!(values.len() == expected_count);
        check!(cursor.is_none());

        values
    }

    let context = TestContext::new();
    let mut executor = start(&context).await.unwrap();

    let component_id = executor.store_component("option-service").await;

    let workers_count = 10;
    let mut worker_ids = vec![];

    for i in 0..workers_count {
        let worker_id = executor
            .start_worker(&component_id, &format!("test-worker-{}", i))
            .await;

        worker_ids.push(worker_id);
    }

    for worker_id in worker_ids.clone() {
        let _ = executor
            .invoke_and_await(
                &worker_id,
                "golem:it/api.{echo}",
                vec![Value::Option(Some(Box::new(Value::String(
                    "Hello".to_string(),
                ))))],
            )
            .await
            .unwrap();

        get_check(
            &component_id,
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
        &component_id,
        Some(WorkerFilter::new_name(
            StringFilterComparator::Like,
            "test".to_string(),
        )),
        workers_count,
        &mut executor,
    )
    .await;

    get_check(
        &component_id,
        Some(
            WorkerFilter::new_name(StringFilterComparator::Like, "test".to_string())
                .and(
                    WorkerFilter::new_status(FilterComparator::Equal, WorkerStatus::Idle).or(
                        WorkerFilter::new_status(FilterComparator::Equal, WorkerStatus::Running),
                    ),
                )
                .and(WorkerFilter::new_version(FilterComparator::Equal, 0)),
        ),
        workers_count,
        &mut executor,
    )
    .await;

    get_check(
        &component_id,
        Some(WorkerFilter::new_name(StringFilterComparator::Like, "test".to_string()).not()),
        0,
        &mut executor,
    )
    .await;

    get_check(&component_id, None, workers_count, &mut executor).await;

    let (cursor1, values1) = executor
        .get_workers_metadata(
            &component_id,
            None,
            ScanCursor::default(),
            (workers_count / 2) as u64,
            true,
        )
        .await;

    check!(cursor1.is_some());
    check!(values1.len() >= workers_count / 2);

    let (cursor2, values2) = executor
        .get_workers_metadata(
            &component_id,
            None,
            cursor1.unwrap(),
            (workers_count - values1.len()) as u64,
            true,
        )
        .await;

    check!(values2.len() == workers_count - values1.len());

    if let Some(cursor2) = cursor2 {
        let (_, values3) = executor
            .get_workers_metadata(&component_id, None, cursor2, workers_count as u64, true)
            .await;
        check!(values3.len() == 0);
    }

    for worker_id in worker_ids {
        executor.delete_worker(&worker_id).await;
    }

    get_check(&component_id, None, 0, &mut executor).await;
}

#[tokio::test]
#[tracing::instrument]
async fn error_handling_when_worker_is_invoked_with_fewer_than_expected_parameters() {
    let context = TestContext::new();
    let executor = start(&context).await.unwrap();

    let component_id = executor.store_component("option-service").await;
    let worker_id = executor
        .start_worker(&component_id, "fewer-than-expected-parameters-1")
        .await;

    let failure = executor
        .invoke_and_await(&worker_id, "golem:it/api.{echo}", vec![])
        .await;
    drop(executor);
    check!(failure.is_err());
}

#[tokio::test]
#[tracing::instrument]
async fn error_handling_when_worker_is_invoked_with_more_than_expected_parameters() {
    let context = TestContext::new();
    let executor = start(&context).await.unwrap();

    let component_id = executor.store_component("option-service").await;
    let worker_id = executor
        .start_worker(&component_id, "more-than-expected-parameters-1")
        .await;

    let failure = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{echo}",
            vec![
                Value::Option(Some(Box::new(Value::String("Hello".to_string())))),
                Value::String("extra parameter".to_string()),
            ],
        )
        .await;
    drop(executor);

    check!(failure.is_err());
}

#[tokio::test]
#[tracing::instrument]
async fn get_worker_metadata() {
    let context = TestContext::new();
    let executor = start(&context).await.unwrap();

    let component_id = executor.store_component("clock-service").await;
    let worker_id = executor
        .start_worker(&component_id, "get-worker-metadata-1")
        .await;

    let worker_id_clone = worker_id.clone();
    let executor_clone = executor.clone();
    let fiber = tokio::spawn(async move {
        executor_clone
            .invoke_and_await(
                &worker_id_clone,
                "golem:it/api.{sleep}",
                vec![Value::U64(10)],
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
    check!(metadata1.last_known_status.component_version == 0);
    check!(metadata1.worker_id == worker_id);
    check!(
        metadata1.account_id
            == AccountId {
                value: "test-account".to_string()
            }
    );
    check!(metadata2.last_known_status.component_size == 60782);
    check!(metadata2.last_known_status.total_linear_memory_size == 1114112);
}

#[tokio::test]
#[tracing::instrument]
async fn create_invoke_delete_create_invoke() {
    let context = TestContext::new();
    let executor = start(&context).await.unwrap();

    let component_id = executor.store_component("shopping-cart").await;
    let worker_id = executor
        .start_worker(&component_id, "create-invoke-delete-create-invoke-1")
        .await;

    let r1 = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{add-item}",
            vec![Value::Record(vec![
                Value::String("G1000".to_string()),
                Value::String("Golem T-Shirt M".to_string()),
                Value::F32(100.0),
                Value::U32(5),
            ])],
        )
        .await;

    executor.delete_worker(&worker_id).await;

    let worker_id = executor
        .start_worker(&component_id, "create-invoke-delete-create-invoke-1") // same name as before
        .await;

    let r2 = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{add-item}",
            vec![Value::Record(vec![
                Value::String("G1000".to_string()),
                Value::String("Golem T-Shirt M".to_string()),
                Value::F32(100.0),
                Value::U32(5),
            ])],
        )
        .await;

    drop(executor);

    check!(r1.is_ok());
    check!(r2.is_ok());
}

#[tokio::test]
#[tracing::instrument]
async fn recovering_an_old_worker_after_updating_a_component() {
    let context = TestContext::new();
    let executor = start(&context).await.unwrap();

    let component_id = executor.store_unique_component("shopping-cart").await;
    let worker_id = executor
        .start_worker(
            &component_id,
            "recovering-an-old-worker-after-updating-a-component-1",
        )
        .await;

    let r1 = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{add-item}",
            vec![Value::Record(vec![
                Value::String("G1000".to_string()),
                Value::String("Golem T-Shirt M".to_string()),
                Value::F32(100.0),
                Value::U32(5),
            ])],
        )
        .await
        .unwrap();

    // Updating the component with an incompatible new version
    executor
        .update_component(&component_id, "option-service")
        .await;

    // Creating a new worker of the updated component and call it
    let worker_id2 = executor
        .start_worker(
            &component_id,
            "recovering-an-old-worker-after-updating-a-component-2",
        )
        .await;

    let r2 = executor
        .invoke_and_await(
            &worker_id2,
            "golem:it/api.{echo}",
            vec![Value::Option(Some(Box::new(Value::String(
                "Hello".to_string(),
            ))))],
        )
        .await
        .unwrap();

    // Restarting the server to force worker recovery
    drop(executor);
    let executor = start(&context).await.unwrap();

    // Call the first worker again to check if it is still working
    let r3 = executor
        .invoke_and_await(&worker_id, "golem:it/api.{get-cart-contents}", vec![])
        .await
        .unwrap();

    drop(executor);

    check!(r1 == vec![]);
    check!(
        r2 == vec![Value::Option(Some(Box::new(Value::String(
            "Hello".to_string()
        ))))]
    );
    check!(
        r3 == vec![Value::List(vec![Value::Record(vec![
            Value::String("G1000".to_string()),
            Value::String("Golem T-Shirt M".to_string()),
            Value::F32(100.0),
            Value::U32(5),
        ])])]
    );
}

#[tokio::test]
#[tracing::instrument]
async fn recreating_a_worker_after_it_got_deleted_with_a_different_version() {
    let context = TestContext::new();
    let executor = start(&context).await.unwrap();

    let component_id = executor.store_unique_component("shopping-cart").await;
    let worker_id = executor
        .start_worker(
            &component_id,
            "recreating-an-worker-after-it-got-deleted-with-a-different-version-1",
        )
        .await;

    let r1 = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{add-item}",
            vec![Value::Record(vec![
                Value::String("G1000".to_string()),
                Value::String("Golem T-Shirt M".to_string()),
                Value::F32(100.0),
                Value::U32(5),
            ])],
        )
        .await
        .unwrap();

    // Updating the component with an incompatible new version
    executor
        .update_component(&component_id, "option-service")
        .await;

    // Deleting the first instance
    executor.delete_worker(&worker_id).await;

    // Create a new instance with the same name and call it the first instance again to check if it is still working
    let worker_id = executor
        .start_worker(
            &component_id,
            "recovering-an-old-worker-after-updating-a-component-1",
        )
        .await;

    let r2 = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{echo}",
            vec![Value::Option(Some(Box::new(Value::String(
                "Hello".to_string(),
            ))))],
        )
        .await
        .unwrap();

    drop(executor);

    check!(r1 == vec![]);
    check!(
        r2 == vec![Value::Option(Some(Box::new(Value::String(
            "Hello".to_string()
        ))))]
    );
}

#[tokio::test]
#[tracing::instrument]
async fn trying_to_use_an_old_wasm_provides_good_error_message() {
    let context = TestContext::new();
    // case: WASM is an old version, rejected by protector

    let executor = start(&context).await.unwrap();

    let component_id = executor.store_component_unverified("old-component").await;
    let result = executor
        .try_start_worker(&component_id, "old-component-1")
        .await;

    check!(result.is_err());
    check!(is_worker_execution_error(
        &result.err().unwrap(),
        &worker_execution_error::Error::ComponentParseFailed(ComponentParseFailed {
            component_id: Some(component_id.into()),
            component_version: 0,
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
    let component_id = executor.store_component("write-stdout").await;

    let cwd = env::current_dir().expect("Failed to get current directory");
    debug!("Current directory: {cwd:?}");
    let target_dir = cwd.join(Path::new("data/components"));
    let component_path = target_dir.join(Path::new(&format!("{component_id}-0.wasm")));

    {
        let mut file = std::fs::File::options()
            .write(true)
            .truncate(false)
            .open(&component_path)
            .expect("Failed to open component file");
        file.write_at(&[1, 2, 3, 4], 0)
            .expect("Failed to write to component file");
        file.flush().expect("Failed to flush component file");
    }

    let result = executor.try_start_worker(&component_id, "bad-wasm-1").await;

    check!(result.is_err());
    check!(is_worker_execution_error(
        &result.err().unwrap(),
        &worker_execution_error::Error::ComponentParseFailed(ComponentParseFailed {
            component_id: Some(component_id.into()),
            component_version: 0,
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
    let component_id = executor.store_component("write-stdout").await;

    let worker_id = executor
        .try_start_worker(&component_id, "bad-wasm-2")
        .await
        .unwrap();

    // worker is idle. if we restart the server it won't get recovered
    drop(executor);
    let executor = start(&context).await.unwrap();

    // corrupting the uploaded WASM
    let cwd = env::current_dir().expect("Failed to get current directory");
    debug!("Current directory: {cwd:?}");
    let target_dir = cwd.join(Path::new("data/components"));
    let component_path = target_dir.join(Path::new(&format!("{component_id}-0.wasm")));
    let compiled_component_path = cwd.join(Path::new(&format!(
        "data/compilation_cache/{component_id}/0.cwasm"
    )));

    {
        debug!("Corrupting {:?}", component_path);
        let mut file = std::fs::File::options()
            .write(true)
            .truncate(false)
            .open(&component_path)
            .expect("Failed to open component file");
        file.write_at(&[1, 2, 3, 4], 0)
            .expect("Failed to write to component file");
        file.flush().expect("Failed to flush component file");

        debug!("Deleting {:?}", compiled_component_path);
        std::fs::remove_file(&compiled_component_path)
            .expect("Failed to delete compiled component");
    }

    // trying to invoke the previously created worker
    let result = executor.invoke_and_await(&worker_id, "run", vec![]).await;

    check!(result.is_err());
    check!(is_worker_execution_error(
        &result.err().unwrap(),
        &worker_execution_error::Error::ComponentParseFailed(ComponentParseFailed {
            component_id: Some(component_id.into()),
            component_version: 0,
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

    let component_id = executor.store_component("http-client-2").await;
    let mut env = HashMap::new();
    env.insert("PORT".to_string(), host_http_port.to_string());

    let worker_id = executor
        .start_worker_with(&component_id, "poll-loop-component-0", vec![], env)
        .await;

    executor.log_output(&worker_id).await;

    executor
        .invoke(
            &worker_id,
            "golem:it/api.{start-polling}",
            vec![Value::String("first".to_string())],
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

    let component_id = executor.store_component("http-client-2").await;
    let mut env = HashMap::new();
    env.insert("PORT".to_string(), host_http_port.to_string());
    let worker_id = executor
        .start_worker_with(&component_id, "poll-loop-component-1", vec![], env)
        .await;

    executor.log_output(&worker_id).await;

    executor
        .invoke(
            &worker_id,
            "golem:it/api.{start-polling}",
            vec![Value::String("first".to_string())],
        )
        .await
        .unwrap();

    sleep(Duration::from_secs(2)).await;
    let status1 = executor.get_worker_metadata(&worker_id).await.unwrap();
    let values1 = executor
        .get_running_workers_metadata(
            &worker_id.component_id,
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
    let values2 = executor
        .get_running_workers_metadata(
            &worker_id.component_id,
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
                "golem:it/api.{start-polling}",
                vec![Value::String("second".to_string())],
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
    check!(!values1.is_empty());
    // first running
    check!(status2.last_known_status.status == WorkerStatus::Interrupted);
    check!(values2.is_empty());
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

    let component_id = executor.store_component("http-client-2").await;
    let mut env = HashMap::new();
    env.insert("PORT".to_string(), host_http_port.to_string());
    let worker_id = executor
        .start_worker_with(&component_id, "poll-loop-component-2", vec![], env)
        .await;

    let rx = executor.capture_output_with_termination(&worker_id).await;

    executor
        .invoke(
            &worker_id,
            "golem:it/api.{start-polling}",
            vec![Value::String("first".to_string())],
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

    let component_id = executor.store_component("http-client-2").await;
    let mut env = HashMap::new();
    env.insert("PORT".to_string(), host_http_port.to_string());

    let worker_id = executor
        .start_worker_with(&component_id, "poll-loop-component-3", vec![], env)
        .await;

    let rx = executor.capture_output_with_termination(&worker_id).await;

    executor
        .invoke(
            &worker_id,
            "golem:it/api.{start-polling}",
            vec![Value::String("first".to_string())],
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

    let component_id = executor.store_component("http-client-2").await;
    let mut env = HashMap::new();
    env.insert("PORT".to_string(), host_http_port.to_string());

    let worker_id = executor
        .start_worker_with(&component_id, "poll-loop-component-4", vec![], env)
        .await;

    let rx = executor.capture_output_with_termination(&worker_id).await;

    executor
        .invoke(
            &worker_id,
            "golem:it/api.{start-polling}",
            vec![Value::String("first".to_string())],
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

    let component_id = executor.store_component("http-client-2").await;
    let mut env = HashMap::new();
    env.insert("PORT".to_string(), host_http_port.to_string());

    let worker_id = executor
        .start_worker_with(&component_id, "poll-loop-component-5", vec![], env)
        .await;

    let rx = executor.capture_output_with_termination(&worker_id).await;

    executor
        .invoke(
            &worker_id,
            "golem:it/api.{start-polling}",
            vec![Value::String("first".to_string())],
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

    let component_id = executor.store_component("shopping-cart-resource").await;
    let worker_id = executor
        .start_worker(&component_id, "shopping-cart-resource-1")
        .await;

    let cart = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{[constructor]cart}",
            vec![Value::String("test-user-1".to_string())],
        )
        .await
        .unwrap();
    println!("cart: {:?}", cart);

    let _ = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{[method]cart.add-item}",
            vec![
                cart[0].clone(),
                Value::Record(vec![
                    Value::String("G1000".to_string()),
                    Value::String("Golem T-Shirt M".to_string()),
                    Value::F32(100.0),
                    Value::U32(5),
                ]),
            ],
        )
        .await;

    let _ = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{[method]cart.add-item}",
            vec![
                cart[0].clone(),
                Value::Record(vec![
                    Value::String("G1001".to_string()),
                    Value::String("Golem Cloud Subscription 1y".to_string()),
                    Value::F32(999999.0),
                    Value::U32(1),
                ]),
            ],
        )
        .await;

    let _ = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{[method]cart.add-item}",
            vec![
                cart[0].clone(),
                Value::Record(vec![
                    Value::String("G1002".to_string()),
                    Value::String("Mud Golem".to_string()),
                    Value::F32(11.0),
                    Value::U32(10),
                ]),
            ],
        )
        .await;

    let _ = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{[method]cart.update-item-quantity}",
            vec![
                cart[0].clone(),
                Value::String("G1002".to_string()),
                Value::U32(20),
            ],
        )
        .await;

    let contents = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{[method]cart.get-cart-contents}",
            vec![cart[0].clone()],
        )
        .await;

    let _ = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{[method]cart.checkout}",
            vec![cart[0].clone()],
        )
        .await;

    drop(executor);

    assert!(
        contents
            == Ok(vec![Value::List(vec![
                Value::Record(vec![
                    Value::String("G1000".to_string()),
                    Value::String("Golem T-Shirt M".to_string()),
                    Value::F32(100.0),
                    Value::U32(5),
                ]),
                Value::Record(vec![
                    Value::String("G1001".to_string()),
                    Value::String("Golem Cloud Subscription 1y".to_string()),
                    Value::F32(999999.0),
                    Value::U32(1),
                ]),
                Value::Record(vec![
                    Value::String("G1002".to_string()),
                    Value::String("Mud Golem".to_string()),
                    Value::F32(11.0),
                    Value::U32(20),
                ]),
            ])])
    )
}

#[tokio::test]
#[tracing::instrument]
async fn counter_resource_test_1() {
    let context = TestContext::new();
    let executor = start(&context).await.unwrap();

    let component_id = executor.store_component("counters").await;
    let worker_id = executor.start_worker(&component_id, "counters-1").await;
    executor.log_output(&worker_id).await;

    let counter1 = executor
        .invoke_and_await(
            &worker_id,
            "rpc:counters/api.{[constructor]counter}",
            vec![Value::String("counter1".to_string())],
        )
        .await
        .unwrap();
    let _ = executor
        .invoke_and_await(
            &worker_id,
            "rpc:counters/api.{[method]counter.inc-by}",
            vec![counter1[0].clone(), Value::U64(5)],
        )
        .await;

    let result1 = executor
        .invoke_and_await(
            &worker_id,
            "rpc:counters/api.{[method]counter.get-value}",
            vec![counter1[0].clone()],
        )
        .await;

    let _ = executor
        .invoke_and_await(
            &worker_id,
            "rpc:counters/api.{[drop]counter}",
            vec![counter1[0].clone()],
        )
        .await;

    let result2 = executor
        .invoke_and_await(&worker_id, "rpc:counters/api.{get-all-dropped}", vec![])
        .await;

    drop(executor);

    check!(result1 == Ok(vec![Value::U64(5)]));
    check!(
        result2
            == Ok(vec![Value::List(vec![Value::Tuple(vec![
                Value::String("counter1".to_string()),
                Value::U64(5)
            ])])])
    );
}

#[tokio::test]
#[tracing::instrument]
async fn counter_resource_test_2() {
    let context = TestContext::new();
    let executor = start(&context).await.unwrap();

    let component_id = executor.store_component("counters").await;
    let worker_id = executor.start_worker(&component_id, "counters-2").await;
    executor.log_output(&worker_id).await;

    let _ = executor
        .invoke_and_await(
            &worker_id,
            "rpc:counters/api.{counter(\"counter1\").inc-by}",
            vec![Value::U64(5)],
        )
        .await;

    let _ = executor
        .invoke_and_await(
            &worker_id,
            "rpc:counters/api.{counter(\"counter2\").inc-by}",
            vec![Value::U64(1)],
        )
        .await;
    let _ = executor
        .invoke_and_await(
            &worker_id,
            "rpc:counters/api.{counter(\"counter2\").inc-by}",
            vec![Value::U64(2)],
        )
        .await;

    let result1 = executor
        .invoke_and_await(
            &worker_id,
            "rpc:counters/api.{counter(\"counter1\").get-value}",
            vec![],
        )
        .await;
    let result2 = executor
        .invoke_and_await(
            &worker_id,
            "rpc:counters/api.{counter(\"counter2\").get-value}",
            vec![],
        )
        .await;

    let _ = executor
        .invoke_and_await(
            &worker_id,
            "rpc:counters/api.{counter(\"counter1\").drop}",
            vec![],
        )
        .await;
    let _ = executor
        .invoke_and_await(
            &worker_id,
            "rpc:counters/api.{counter(\"counter2\").drop}",
            vec![],
        )
        .await;

    let result3 = executor
        .invoke_and_await(&worker_id, "rpc:counters/api.{get-all-dropped}", vec![])
        .await;

    drop(executor);

    check!(result1 == Ok(vec![Value::U64(5)]));
    check!(result2 == Ok(vec![Value::U64(3)]));
    check!(
        result3
            == Ok(vec![Value::List(vec![
                Value::Tuple(vec![Value::String("counter1".to_string()), Value::U64(5)]),
                Value::Tuple(vec![Value::String("counter2".to_string()), Value::U64(3)])
            ])])
    );
}

#[tokio::test]
#[tracing::instrument]
async fn reconstruct_interrupted_state() {
    let context = TestContext::new();
    let executor = start(&context).await.unwrap();

    let component_id = executor.store_component("interruption").await;
    let worker_id = executor.start_worker(&component_id, "interruption-1").await;

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

#[tokio::test]
#[tracing::instrument]
async fn invocation_queue_is_persistent() {
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

    let component_id = executor.store_component("http-client-2").await;
    let mut env = HashMap::new();
    env.insert("PORT".to_string(), host_http_port.to_string());

    let worker_id = executor
        .start_worker_with(&component_id, "invocation-queue-is-persistent", vec![], env)
        .await;

    executor.log_output(&worker_id).await;

    executor
        .invoke(
            &worker_id,
            "golem:it/api.{start-polling}",
            vec![Value::String("done".to_string())],
        )
        .await
        .unwrap();

    sleep(Duration::from_secs(2)).await;

    executor
        .invoke(&worker_id, "golem:it/api.{increment}", vec![])
        .await
        .unwrap();
    executor
        .invoke(&worker_id, "golem:it/api.{increment}", vec![])
        .await
        .unwrap();
    executor
        .invoke(&worker_id, "golem:it/api.{increment}", vec![])
        .await
        .unwrap();

    executor.interrupt(&worker_id).await;

    sleep(Duration::from_secs(2)).await;

    drop(executor);
    let executor = start(&context).await.unwrap();

    executor
        .invoke(&worker_id, "golem:it/api.{increment}", vec![])
        .await
        .unwrap();

    executor.log_output(&worker_id).await;

    sleep(Duration::from_secs(2)).await;

    {
        let mut response = response.lock().unwrap();
        *response = "done".to_string();
    }

    let result = executor
        .invoke_and_await(&worker_id, "golem:it/api.{get-count}", vec![])
        .await
        .unwrap();

    drop(executor);
    http_server.abort();

    check!(result == vec![Value::U64(4)]);
}
