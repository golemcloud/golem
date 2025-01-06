// Copyright 2024-2025 Golem Cloud
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

use crate::common::{start, TestContext, TestWorkerExecutor};
use crate::compatibility::worker_recovery::save_recovery_golden_file;
use crate::{LastUniqueId, Tracing, WorkerExecutorTestDependencies};
use assert2::check;
use axum::routing::get;
use axum::Router;
use golem_api_grpc::proto::golem::worker::v1::{worker_execution_error, ComponentParseFailed};
use golem_api_grpc::proto::golem::workerexecutor::v1::CompletePromiseRequest;
use golem_common::model::oplog::{IndexedResourceKey, OplogIndex, WorkerResourceId};
use golem_common::model::{
    AccountId, ComponentId, FilterComparator, IdempotencyKey, PromiseId, ScanCursor,
    StringFilterComparator, TargetWorkerId, Timestamp, WorkerFilter, WorkerId, WorkerMetadata,
    WorkerResourceDescription, WorkerStatus,
};
use golem_test_framework::config::TestDependencies;
use golem_test_framework::dsl::{
    drain_connection, is_worker_execution_error, stdout_event_matching, stdout_events,
    worker_error_message, TestDslUnsafe,
};
use golem_wasm_rpc::Value;
use redis::Commands;
use std::collections::HashMap;
use std::env;
use std::io::Write;
use std::net::SocketAddr;
use std::os::unix::fs::FileExt;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::time::sleep;
use tracing::{debug, info};
use wasmtime_wasi::runtime::spawn;

inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(LastUniqueId);
inherit_test_dep!(Tracing);

#[test]
#[tracing::instrument]
async fn interruption(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await.unwrap();

    let component_id = executor.store_component("interruption").await;
    let worker_id = executor.start_worker(&component_id, "interruption-1").await;

    let executor_clone = executor.clone();
    let worker_id_clone = worker_id.clone();
    let fiber = tokio::spawn(async move {
        executor_clone
            .invoke_and_await(worker_id_clone, "run", vec![])
            .await
    });

    executor
        .wait_for_status(&worker_id, WorkerStatus::Running, Duration::from_secs(10))
        .await;

    let _ = executor.interrupt(&worker_id).await;
    let result = fiber.await.unwrap();

    drop(executor);

    check!(result.is_err());
    check!(worker_error_message(&result.err().unwrap()).contains("Interrupted via the Golem API"));
}

#[test]
#[tracing::instrument]
async fn simulated_crash(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await.unwrap();

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
            .invoke_and_await(worker_id_clone, "run", vec![])
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
    check!(stdout_events(events.into_iter()) == vec!["Starting interruption test\n"]);
    check!(elapsed.as_secs() < 13);
}

#[test]
#[tracing::instrument]
async fn shopping_cart_example(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
) {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await.unwrap();

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

    save_recovery_golden_file(&executor, &context, "shopping_cart_example", &worker_id).await;
    drop(executor);

    check!(
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
    );
}

#[test]
#[tracing::instrument]
async fn dynamic_worker_creation(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
) {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await.unwrap();

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

fn get_env_result(env: Vec<Value>) -> HashMap<String, String> {
    match env.into_iter().next() {
        Some(Value::Result(Ok(Some(inner)))) => match *inner {
            Value::List(items) => {
                let pairs = items
                    .into_iter()
                    .filter_map(|item| match item {
                        Value::Tuple(values) if values.len() == 2 => {
                            let mut iter = values.into_iter();
                            let key = iter.next();
                            let value = iter.next();
                            match (key, value) {
                                (Some(Value::String(key)), Some(Value::String(value))) => {
                                    Some((key, value))
                                }
                                _ => None,
                            }
                        }
                        _ => None,
                    })
                    .collect::<Vec<(String, String)>>();
                HashMap::from_iter(pairs)
            }
            _ => panic!("Unexpected result value"),
        },
        _ => panic!("Unexpected result value"),
    }
}

#[test]
#[tracing::instrument]
async fn dynamic_worker_creation_without_name(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
) {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await.unwrap();

    let component_id = executor.store_component("environment-service").await;
    let worker_id = TargetWorkerId {
        component_id: component_id.clone(),
        worker_name: None,
    };

    let env1 = executor
        .invoke_and_await(worker_id.clone(), "golem:it/api.{get-environment}", vec![])
        .await
        .unwrap();
    let env2 = executor
        .invoke_and_await(worker_id.clone(), "golem:it/api.{get-environment}", vec![])
        .await
        .unwrap();

    drop(executor);

    let env1 = get_env_result(env1);
    let env2 = get_env_result(env2);

    check!(env1.contains_key("GOLEM_WORKER_NAME"));
    check!(env1.get("GOLEM_COMPONENT_ID") == Some(&component_id.to_string()));
    check!(env1.get("GOLEM_COMPONENT_VERSION") == Some(&"0".to_string()));
    check!(env2.contains_key("GOLEM_WORKER_NAME"));
    check!(env2.get("GOLEM_COMPONENT_ID") == Some(&component_id.to_string()));
    check!(env2.get("GOLEM_COMPONENT_VERSION") == Some(&"0".to_string()));
    check!(env1.get("GOLEM_WORKER_NAME") != env2.get("GOLEM_WORKER_NAME"));
}

#[test]
#[tracing::instrument]
async fn ephemeral_worker_creation_without_name(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
) {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await.unwrap();

    let component_id = executor
        .store_ephemeral_component("environment-service")
        .await;
    let worker_id = TargetWorkerId {
        component_id: component_id.clone(),
        worker_name: None,
    };

    let env1 = executor
        .invoke_and_await(worker_id.clone(), "golem:it/api.{get-environment}", vec![])
        .await
        .unwrap();
    let env2 = executor
        .invoke_and_await(worker_id.clone(), "golem:it/api.{get-environment}", vec![])
        .await
        .unwrap();

    drop(executor);

    let env1 = get_env_result(env1);
    let env2 = get_env_result(env2);

    check!(env1.contains_key("GOLEM_WORKER_NAME"));
    check!(env1.get("GOLEM_COMPONENT_ID") == Some(&component_id.to_string()));
    check!(env1.get("GOLEM_COMPONENT_VERSION") == Some(&"0".to_string()));
    check!(env2.contains_key("GOLEM_WORKER_NAME"));
    check!(env2.get("GOLEM_COMPONENT_ID") == Some(&component_id.to_string()));
    check!(env2.get("GOLEM_COMPONENT_VERSION") == Some(&"0".to_string()));
    check!(env1.get("GOLEM_WORKER_NAME") != env2.get("GOLEM_WORKER_NAME"));
}

#[test]
#[tracing::instrument]
async fn ephemeral_worker_creation_with_name_is_not_persistent(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
) {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await.unwrap();

    let component_id = executor.store_ephemeral_component("counters").await;
    let worker_id = TargetWorkerId {
        component_id: component_id.clone(),
        worker_name: Some("test".to_string()),
    };

    let _ = executor
        .invoke_and_await(
            worker_id.clone(),
            "rpc:counters-exports/api.{inc-global-by}",
            vec![Value::U64(2)],
        )
        .await
        .unwrap();

    let result = executor
        .invoke_and_await(
            worker_id.clone(),
            "rpc:counters-exports/api.{get-global-value}",
            vec![],
        )
        .await
        .unwrap();

    drop(executor);

    check!(result == vec![Value::U64(0)]);
}

#[test]
#[tracing::instrument]
async fn promise(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await.unwrap();

    let component_id = executor.store_component("promise").await;
    let worker_id = executor.start_worker(&component_id, "promise-1").await;

    let executor_clone = executor.clone();
    let worker_id_clone = worker_id.clone();
    let fiber = tokio::spawn(async move {
        executor_clone
            .invoke_and_await(&worker_id_clone, "run", vec![])
            .await
    });

    // While waiting for the promise, the worker gets suspended
    executor
        .wait_for_status(&worker_id, WorkerStatus::Suspended, Duration::from_secs(10))
        .await;

    executor
        .client()
        .await
        .expect("Failed to get client")
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

#[test]
#[tracing::instrument]
async fn get_self_uri(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await.unwrap();

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
                "urn:worker:{component_id}/runtime-service-1/function-name"
            ))]
    );
}

#[test]
#[tracing::instrument]
async fn get_workers_from_worker(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
) {
    let context = TestContext::new(last_unique_id);
    let mut executor = start(deps, &context).await.unwrap();

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
                worker_id.clone(),
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

#[test]
#[tracing::instrument]
async fn get_metadata_from_worker(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
) {
    let context = TestContext::new(last_unique_id);
    let mut executor = start(deps, &context).await.unwrap();

    let component_id = executor.store_component("runtime-service").await;

    let worker_id1 = executor
        .start_worker(&component_id, "runtime-service-1")
        .await;

    let worker_id2 = executor
        .start_worker(&component_id, "runtime-service-2")
        .await;

    fn get_worker_id_val(worker_id: &WorkerId) -> Value {
        let component_id_val = {
            let (high, low) = worker_id.component_id.0.as_u64_pair();
            Value::Record(vec![Value::Record(vec![Value::U64(high), Value::U64(low)])])
        };

        Value::Record(vec![
            component_id_val,
            Value::String(worker_id.worker_name.clone()),
        ])
    }

    async fn get_check(
        worker_id1: &WorkerId,
        worker_id2: &WorkerId,
        executor: &mut TestWorkerExecutor,
    ) {
        let worker_id_val1 = get_worker_id_val(worker_id1);

        let result = executor
            .invoke_and_await(
                worker_id1.clone(),
                "golem:it/api.{get-self-metadata}",
                vec![],
            )
            .await
            .unwrap();

        match result.first() {
            Some(Value::Record(values)) if !values.is_empty() => {
                let id_val = values.first().unwrap();
                check!(worker_id_val1 == *id_val);
            }
            _ => {
                check!(false);
            }
        }

        let worker_id_val2 = get_worker_id_val(worker_id2);

        let result = executor
            .invoke_and_await(
                worker_id1.clone(),
                "golem:it/api.{get-worker-metadata}",
                vec![worker_id_val2.clone()],
            )
            .await
            .unwrap();

        match result.first() {
            Some(Value::Option(value)) if value.is_some() => {
                let result = *value.clone().unwrap();
                match result {
                    Value::Record(values) if !values.is_empty() => {
                        let id_val = values.first().unwrap();
                        check!(worker_id_val2 == *id_val);
                    }
                    _ => {
                        check!(false);
                    }
                }
            }
            _ => {
                check!(false);
            }
        }
    }

    get_check(&worker_id1, &worker_id2, &mut executor).await;
    get_check(&worker_id2, &worker_id1, &mut executor).await;

    drop(executor);
}

#[test]
#[tracing::instrument]
async fn invoking_with_same_idempotency_key_is_idempotent(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
) {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await.unwrap();

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

#[test]
#[tracing::instrument]
async fn invoking_with_same_idempotency_key_is_idempotent_after_restart(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
) {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await.unwrap();

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
    let executor = start(deps, &context).await.unwrap();

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

#[test]
#[tracing::instrument]
async fn optional_parameters(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await.unwrap();

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

#[test]
#[tracing::instrument]
async fn flags_parameters(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await.unwrap();

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

#[test]
#[tracing::instrument]
async fn variants_with_no_payloads(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
) {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await.unwrap();

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

#[test]
#[tracing::instrument]
async fn delete_worker(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await.unwrap();

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

#[test]
#[tracing::instrument]
async fn get_workers(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) {
    async fn get_check(
        component_id: &ComponentId,
        filter: Option<WorkerFilter>,
        expected_count: usize,
        executor: &mut TestWorkerExecutor,
    ) -> Vec<(WorkerMetadata, Option<String>)> {
        let (cursor, values) = executor
            .get_workers_metadata(component_id, filter, ScanCursor::default(), 20, true)
            .await;

        check!(values.len() == expected_count);
        check!(cursor.is_none());

        values
    }

    let context = TestContext::new(last_unique_id);
    let mut executor = start(deps, &context).await.unwrap();

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

#[test]
#[tracing::instrument]
async fn error_handling_when_worker_is_invoked_with_fewer_than_expected_parameters(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
) {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await.unwrap();

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

#[test]
#[tracing::instrument]
async fn error_handling_when_worker_is_invoked_with_more_than_expected_parameters(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
) {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await.unwrap();

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

#[test]
#[tracing::instrument]
async fn get_worker_metadata(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await.unwrap();

    let component_id = executor.store_component("clock-service").await;
    let component_path = executor.component_directory().join("clock-service.wasm");
    let expected_component_size = tokio::fs::metadata(&component_path).await.unwrap().len();

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
                vec![Value::U64(2)],
            )
            .await
    });

    let metadata1 = executor
        .wait_for_statuses(
            &worker_id,
            &[WorkerStatus::Running, WorkerStatus::Suspended],
            Duration::from_secs(5),
        )
        .await;

    let _ = fiber.await;

    let metadata2 = executor
        .wait_for_status(&worker_id, WorkerStatus::Idle, Duration::from_secs(10))
        .await;

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

    check!(metadata2.last_known_status.component_size == expected_component_size);
    check!(metadata2.last_known_status.total_linear_memory_size == 1245184);
}

#[test]
#[tracing::instrument]
async fn create_invoke_delete_create_invoke(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
) {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await.unwrap();

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

#[test]
#[tracing::instrument]
async fn recovering_an_old_worker_after_updating_a_component(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
) {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await.unwrap();

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
    let executor = start(deps, &context).await.unwrap();

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

#[test]
#[tracing::instrument]
async fn recreating_a_worker_after_it_got_deleted_with_a_different_version(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
) {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await.unwrap();

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

#[test]
#[tracing::instrument]
async fn trying_to_use_an_old_wasm_provides_good_error_message(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
) {
    let context = TestContext::new(last_unique_id);
    // case: WASM is an old version, rejected by protector

    let executor = start(deps, &context).await.unwrap();

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

#[test]
#[tracing::instrument]
async fn trying_to_use_a_wasm_that_wasmtime_cannot_load_provides_good_error_message(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
) {
    let context = TestContext::new(last_unique_id);
    // case: WASM can be parsed but wasmtime does not support it
    let executor = start(deps, &context).await.unwrap();
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

#[test]
#[tracing::instrument]
async fn trying_to_use_a_wasm_that_wasmtime_cannot_load_provides_good_error_message_after_recovery(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
) {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await.unwrap();
    let component_id = executor.store_component("write-stdout").await;

    let worker_id = executor
        .try_start_worker(&component_id, "bad-wasm-2")
        .await
        .unwrap();

    // worker is idle. if we restart the server it won't get recovered
    drop(executor);
    let executor = start(deps, &context).await.unwrap();

    // corrupting the uploaded WASM
    let cwd = env::current_dir().expect("Failed to get current directory");
    debug!("Current directory: {cwd:?}");
    let target_dir = cwd.join(Path::new("data/components"));
    let component_path = target_dir.join(Path::new(&format!("{component_id}-0.wasm")));
    let compiled_component_path = cwd.join(Path::new(&format!(
        "data/blobs/compilation_cache/{component_id}/0.cwasm"
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

#[test]
#[tracing::instrument]
async fn long_running_poll_loop_works_as_expected(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
) {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await.unwrap();

    let response = Arc::new(Mutex::new("initial".to_string()));
    let response_clone = response.clone();
    let host_http_port = context.host_http_port();

    let http_server = tokio::spawn(async move {
        let route = Router::new().route(
            "/poll",
            get(move || async move {
                let body = response_clone.lock().unwrap();
                body.clone()
            }),
        );

        let listener = tokio::net::TcpListener::bind(
            format!("0.0.0.0:{}", host_http_port)
                .parse::<SocketAddr>()
                .unwrap(),
        )
        .await
        .unwrap();
        axum::serve(listener, route).await.unwrap();
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

    executor
        .wait_for_status(&worker_id, WorkerStatus::Running, Duration::from_secs(10))
        .await;

    {
        let mut response = response.lock().unwrap();
        *response = "first".to_string();
    }

    executor
        .wait_for_status(&worker_id, WorkerStatus::Idle, Duration::from_secs(10))
        .await;

    drop(executor);
    http_server.abort();
}

#[test]
#[tracing::instrument]
async fn long_running_poll_loop_interrupting_and_resuming_by_second_invocation(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
) {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await.unwrap();

    let response = Arc::new(Mutex::new("initial".to_string()));
    let response_clone = response.clone();
    let host_http_port = context.host_http_port();

    let http_server = tokio::spawn(async move {
        let route = Router::new().route(
            "/poll",
            get(move || async move {
                let body = response_clone.lock().unwrap();
                body.clone()
            }),
        );

        let listener = tokio::net::TcpListener::bind(
            format!("0.0.0.0:{}", host_http_port)
                .parse::<SocketAddr>()
                .unwrap(),
        )
        .await
        .unwrap();
        axum::serve(listener, route).await.unwrap();
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

    executor
        .wait_for_status(&worker_id, WorkerStatus::Running, Duration::from_secs(10))
        .await;
    let values1 = executor
        .get_running_workers_metadata(
            &worker_id.component_id,
            Some(WorkerFilter::new_name(
                StringFilterComparator::Equal,
                worker_id.worker_name.clone(),
            )),
        )
        .await;

    executor.interrupt(&worker_id).await;

    executor
        .wait_for_status(
            &worker_id,
            WorkerStatus::Interrupted,
            Duration::from_secs(5),
        )
        .await;
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

    executor
        .wait_for_status(&worker_id, WorkerStatus::Running, Duration::from_secs(10))
        .await;

    {
        let mut response = response.lock().unwrap();
        *response = "first".to_string();
    }

    fiber.await;

    sleep(Duration::from_secs(1)).await;

    {
        let mut response = response.lock().unwrap();
        *response = "second".to_string();
    }

    executor
        .wait_for_status(&worker_id, WorkerStatus::Idle, Duration::from_secs(10))
        .await;

    drop(executor);
    http_server.abort();

    check!(!values1.is_empty());
    // first running
    check!(values2.is_empty());
    // first interrupted
}

#[test]
#[tracing::instrument]
async fn long_running_poll_loop_connection_breaks_on_interrupt(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
) {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await.unwrap();

    let response = Arc::new(Mutex::new("initial".to_string()));
    let response_clone = response.clone();
    let host_http_port = context.host_http_port();

    let http_server = tokio::spawn(async move {
        let route = Router::new().route(
            "/poll",
            get(move || async move {
                let body = response_clone.lock().unwrap();
                body.clone()
            }),
        );

        let listener = tokio::net::TcpListener::bind(
            format!("0.0.0.0:{}", host_http_port)
                .parse::<SocketAddr>()
                .unwrap(),
        )
        .await
        .unwrap();
        axum::serve(listener, route).await.unwrap();
    });

    let component_id = executor.store_component("http-client-2").await;
    let mut env = HashMap::new();
    env.insert("PORT".to_string(), host_http_port.to_string());
    let worker_id = executor
        .start_worker_with(&component_id, "poll-loop-component-2", vec![], env)
        .await;

    let mut rx = executor.capture_output_with_termination(&worker_id).await;

    executor
        .invoke(
            &worker_id,
            "golem:it/api.{start-polling}",
            vec![Value::String("first".to_string())],
        )
        .await
        .unwrap();

    executor
        .wait_for_status(&worker_id, WorkerStatus::Running, Duration::from_secs(10))
        .await;

    let start = Instant::now();

    let mut found1 = false;
    let mut found2 = false;
    while (!found1 || !found2) && start.elapsed() < Duration::from_secs(5) {
        match rx.recv().await {
            Some(Some(event)) => {
                if stdout_event_matching(&event, "Calling the poll endpoint\n") {
                    found1 = true;
                } else if stdout_event_matching(&event, "Received initial\n") {
                    found2 = true;
                }
            }
            _ => {
                panic!("Did not receive the expected log events");
            }
        }
    }

    executor.interrupt(&worker_id).await;

    let _ = drain_connection(rx).await;

    drop(executor);
    http_server.abort();
}

#[test]
#[tracing::instrument]
async fn long_running_poll_loop_connection_retry_does_not_resume_interrupted_worker(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
) {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await.unwrap();

    let response = Arc::new(Mutex::new("initial".to_string()));
    let response_clone = response.clone();
    let host_http_port = context.host_http_port();

    let http_server = tokio::spawn(async move {
        let route = Router::new().route(
            "/poll",
            get(move || async move {
                let body = response_clone.lock().unwrap();
                body.clone()
            }),
        );

        let listener = tokio::net::TcpListener::bind(
            format!("0.0.0.0:{}", host_http_port)
                .parse::<SocketAddr>()
                .unwrap(),
        )
        .await
        .unwrap();
        axum::serve(listener, route).await.unwrap();
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

    executor
        .wait_for_status(&worker_id, WorkerStatus::Running, Duration::from_secs(10))
        .await;

    executor.interrupt(&worker_id).await;

    let _ = drain_connection(rx).await;
    let (status1, _) = executor.get_worker_metadata(&worker_id).await.unwrap();

    let _rx = executor.capture_output_with_termination(&worker_id).await;
    sleep(Duration::from_secs(2)).await;
    let (status2, _) = executor.get_worker_metadata(&worker_id).await.unwrap();

    drop(executor);
    http_server.abort();

    check!(status1.last_known_status.status == WorkerStatus::Interrupted);
    check!(status2.last_known_status.status == WorkerStatus::Interrupted);
}

#[test]
#[tracing::instrument]
async fn long_running_poll_loop_connection_can_be_restored_after_resume(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
) {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await.unwrap();

    let response = Arc::new(Mutex::new("initial".to_string()));
    let response_clone = response.clone();
    let host_http_port = context.host_http_port();

    let http_server = tokio::spawn(async move {
        let route = Router::new().route(
            "/poll",
            get(move || async move {
                let body = response_clone.lock().unwrap();
                body.clone()
            }),
        );

        let listener = tokio::net::TcpListener::bind(
            format!("0.0.0.0:{}", host_http_port)
                .parse::<SocketAddr>()
                .unwrap(),
        )
        .await
        .unwrap();
        axum::serve(listener, route).await.unwrap();
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

    executor
        .wait_for_status(&worker_id, WorkerStatus::Running, Duration::from_secs(10))
        .await;

    executor.interrupt(&worker_id).await;

    let _ = drain_connection(rx).await;
    let (status2, _) = executor.get_worker_metadata(&worker_id).await.unwrap();

    executor.resume(&worker_id).await;
    executor
        .wait_for_status(&worker_id, WorkerStatus::Running, Duration::from_secs(10))
        .await;
    let mut rx = executor.capture_output_with_termination(&worker_id).await;

    {
        let mut response = response.lock().unwrap();
        *response = "first".to_string();
    }

    let start = Instant::now();

    let mut found1 = false;
    let mut found2 = false;
    let mut found3 = false;
    while (!found1 || !found2 || !found3) && start.elapsed() < Duration::from_secs(5) {
        match rx.recv().await {
            Some(Some(event)) => {
                if stdout_event_matching(&event, "Calling the poll endpoint\n") {
                    found1 = true;
                } else if stdout_event_matching(&event, "Received initial\n") {
                    found2 = true;
                } else if stdout_event_matching(&event, "Poll loop finished\n") {
                    found3 = true;
                }
            }
            _ => {
                panic!("Did not receive the expected log events");
            }
        }
    }

    let (status4, _) = executor.get_worker_metadata(&worker_id).await.unwrap();

    drop(executor);
    http_server.abort();

    check!(status2.last_known_status.status == WorkerStatus::Interrupted);
    check!(status4.last_known_status.status == WorkerStatus::Idle);
}

#[test]
#[tracing::instrument]
async fn long_running_poll_loop_worker_can_be_deleted_after_interrupt(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
) {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await.unwrap();

    let response = Arc::new(Mutex::new("initial".to_string()));
    let response_clone = response.clone();
    let host_http_port = context.host_http_port();

    let http_server = tokio::spawn(async move {
        let route = Router::new().route(
            "/poll",
            get(move || async move {
                let body = response_clone.lock().unwrap();
                body.clone()
            }),
        );

        let listener = tokio::net::TcpListener::bind(
            format!("0.0.0.0:{}", host_http_port)
                .parse::<SocketAddr>()
                .unwrap(),
        )
        .await
        .unwrap();
        axum::serve(listener, route).await.unwrap();
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

    executor
        .wait_for_status(&worker_id, WorkerStatus::Running, Duration::from_secs(10))
        .await;

    executor.interrupt(&worker_id).await;

    let _ = drain_connection(rx).await;

    executor.delete_worker(&worker_id).await;
    let metadata = executor.get_worker_metadata(&worker_id).await;

    drop(executor);
    http_server.abort();

    check!(metadata.is_none());
}

#[test]
#[tracing::instrument]
async fn shopping_cart_resource_example(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
) {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await.unwrap();

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

    assert_eq!(
        contents,
        Ok(vec![Value::List(vec![
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

#[test]
#[tracing::instrument]
async fn counter_resource_test_1(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
) {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await.unwrap();

    let component_id = executor.store_component("counters").await;
    let worker_id = executor.start_worker(&component_id, "counters-1").await;
    executor.log_output(&worker_id).await;

    let counter1 = executor
        .invoke_and_await(
            &worker_id,
            "rpc:counters-exports/api.{[constructor]counter}",
            vec![Value::String("counter1".to_string())],
        )
        .await
        .unwrap();

    let _ = executor
        .invoke_and_await(
            &worker_id,
            "rpc:counters-exports/api.{[method]counter.inc-by}",
            vec![counter1[0].clone(), Value::U64(5)],
        )
        .await;

    let result1 = executor
        .invoke_and_await(
            &worker_id,
            "rpc:counters-exports/api.{[method]counter.get-value}",
            vec![counter1[0].clone()],
        )
        .await;

    let (metadata1, _) = executor.get_worker_metadata(&worker_id).await.unwrap();

    let _ = executor
        .invoke_and_await(
            &worker_id,
            "rpc:counters-exports/api.{[drop]counter}",
            vec![counter1[0].clone()],
        )
        .await;

    let result2 = executor
        .invoke_and_await(
            &worker_id,
            "rpc:counters-exports/api.{get-all-dropped}",
            vec![],
        )
        .await;

    let (metadata2, _) = executor.get_worker_metadata(&worker_id).await.unwrap();

    drop(executor);

    check!(result1 == Ok(vec![Value::U64(5)]));

    check!(
        result2
            == Ok(vec![Value::List(vec![Value::Tuple(vec![
                Value::String("counter1".to_string()),
                Value::U64(5)
            ])])])
    );

    let ts = Timestamp::now_utc();
    let mut resources1 = metadata1
        .last_known_status
        .owned_resources
        .iter()
        .map(|(k, v)| {
            (
                *k,
                WorkerResourceDescription {
                    created_at: ts,
                    ..v.clone()
                },
            )
        })
        .collect::<Vec<_>>();
    resources1.sort_by_key(|(k, _v)| *k);
    check!(
        resources1
            == vec![(
                WorkerResourceId(0),
                WorkerResourceDescription {
                    created_at: ts,
                    indexed_resource_key: None
                }
            ),]
    );

    let resources2 = metadata2
        .last_known_status
        .owned_resources
        .iter()
        .map(|(k, v)| {
            (
                *k,
                WorkerResourceDescription {
                    created_at: ts,
                    ..v.clone()
                },
            )
        })
        .collect::<Vec<_>>();
    check!(resources2 == vec![]);
}

#[test]
#[tracing::instrument]
async fn counter_resource_test_2(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
) {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await.unwrap();

    let component_id = executor.store_component("counters").await;
    let worker_id = executor.start_worker(&component_id, "counters-2").await;
    executor.log_output(&worker_id).await;

    let _ = executor
        .invoke_and_await(
            &worker_id,
            "rpc:counters-exports/api.{counter(\"counter1\").inc-by}",
            vec![Value::U64(5)],
        )
        .await;

    let _ = executor
        .invoke_and_await(
            &worker_id,
            "rpc:counters-exports/api.{counter(\"counter2\").inc-by}",
            vec![Value::U64(1)],
        )
        .await;
    let _ = executor
        .invoke_and_await(
            &worker_id,
            "rpc:counters-exports/api.{counter(\"counter2\").inc-by}",
            vec![Value::U64(2)],
        )
        .await;

    let result1 = executor
        .invoke_and_await(
            &worker_id,
            "rpc:counters-exports/api.{counter(\"counter1\").get-value}",
            vec![],
        )
        .await;
    let result2 = executor
        .invoke_and_await(
            &worker_id,
            "rpc:counters-exports/api.{counter(\"counter2\").get-value}",
            vec![],
        )
        .await;

    let (metadata1, _) = executor.get_worker_metadata(&worker_id).await.unwrap();

    let _ = executor
        .invoke_and_await(
            &worker_id,
            "rpc:counters-exports/api.{counter(\"counter1\").drop}",
            vec![],
        )
        .await;
    let _ = executor
        .invoke_and_await(
            &worker_id,
            "rpc:counters-exports/api.{counter(\"counter2\").drop}",
            vec![],
        )
        .await;

    let result3 = executor
        .invoke_and_await(
            &worker_id,
            "rpc:counters-exports/api.{get-all-dropped}",
            vec![],
        )
        .await;

    let (metadata2, _) = executor.get_worker_metadata(&worker_id).await.unwrap();

    let _oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await;

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

    let ts = Timestamp::now_utc();
    let mut resources1 = metadata1
        .last_known_status
        .owned_resources
        .iter()
        .map(|(k, v)| {
            (
                *k,
                WorkerResourceDescription {
                    created_at: ts,
                    ..v.clone()
                },
            )
        })
        .collect::<Vec<_>>();
    resources1.sort_by_key(|(k, _v)| *k);
    check!(
        resources1
            == vec![
                (
                    WorkerResourceId(0),
                    WorkerResourceDescription {
                        created_at: ts,
                        indexed_resource_key: Some(IndexedResourceKey {
                            resource_name: "counter".to_string(),
                            resource_params: vec!["\"counter1\"".to_string()]
                        })
                    }
                ),
                (
                    WorkerResourceId(1),
                    WorkerResourceDescription {
                        created_at: ts,
                        indexed_resource_key: Some(IndexedResourceKey {
                            resource_name: "counter".to_string(),
                            resource_params: vec!["\"counter2\"".to_string()]
                        })
                    }
                )
            ]
    );

    let resources2 = metadata2
        .last_known_status
        .owned_resources
        .iter()
        .map(|(k, v)| {
            (
                *k,
                WorkerResourceDescription {
                    created_at: ts,
                    ..v.clone()
                },
            )
        })
        .collect::<Vec<_>>();

    check!(resources2 == vec![]);
}

#[test]
#[tracing::instrument]
async fn reconstruct_interrupted_state(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
) {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await.unwrap();

    let component_id = executor.store_component("interruption").await;
    let worker_id = executor.start_worker(&component_id, "interruption-1").await;

    let executor_clone = executor.clone();
    let worker_id_clone = worker_id.clone();
    let fiber = tokio::spawn(async move {
        executor_clone
            .invoke_and_await(&worker_id_clone, "run", vec![])
            .await
    });

    executor
        .wait_for_status(&worker_id, WorkerStatus::Running, Duration::from_secs(10))
        .await;

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
        .0
        .last_known_status
        .status;

    drop(executor);
    check!(result.is_err());
    check!(worker_error_message(&result.err().unwrap()).contains("Interrupted via the Golem API"));
    check!(status == WorkerStatus::Interrupted);
}

#[test]
#[tracing::instrument]
async fn invocation_queue_is_persistent(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
) {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await.unwrap();

    let response = Arc::new(Mutex::new("initial".to_string()));
    let response_clone = response.clone();
    let host_http_port = context.host_http_port();

    let http_server = tokio::spawn(async move {
        let route = Router::new().route(
            "/poll",
            get(move || async move {
                let body = response_clone.lock().unwrap();
                body.clone()
            }),
        );

        let listener = tokio::net::TcpListener::bind(
            format!("0.0.0.0:{}", host_http_port)
                .parse::<SocketAddr>()
                .unwrap(),
        )
        .await
        .unwrap();
        axum::serve(listener, route).await.unwrap();
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

    executor
        .wait_for_status(&worker_id, WorkerStatus::Running, Duration::from_secs(10))
        .await;

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

    executor
        .wait_for_status(
            &worker_id,
            WorkerStatus::Interrupted,
            Duration::from_secs(5),
        )
        .await;

    drop(executor);
    let executor = start(deps, &context).await.unwrap();

    executor
        .invoke(&worker_id, "golem:it/api.{increment}", vec![])
        .await
        .unwrap();

    executor.log_output(&worker_id).await;

    executor
        .wait_for_status(&worker_id, WorkerStatus::Running, Duration::from_secs(10))
        .await;
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

#[test]
#[tracing::instrument]
async fn invoke_with_non_existing_function(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
) {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await.unwrap();

    let component_id = executor.store_component("option-service").await;
    let worker_id = executor
        .start_worker(&component_id, "invoke_with_non_existing_function")
        .await;

    // First we invoke a function that does not exist and expect a failure
    let failure = executor.invoke_and_await(&worker_id, "WRONG", vec![]).await;

    // Then we invoke an existing function, to prove the worker should not be in failed state
    let success = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{echo}",
            vec![Value::Option(Some(Box::new(Value::String(
                "Hello".to_string(),
            ))))],
        )
        .await;

    drop(executor);

    check!(failure.is_err());
    check!(
        success
            == Ok(vec![Value::Option(Some(Box::new(Value::String(
                "Hello".to_string()
            ))))])
    );
}

#[test]
#[tracing::instrument]
async fn stderr_returned_for_failed_component(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
) {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await.unwrap();

    let component_id = executor.store_component("failing-component").await;
    let worker_id = executor
        .start_worker(&component_id, "failing-worker-1")
        .await;

    let result1 = executor
        .invoke_and_await(&worker_id, "golem:component/api.{add}", vec![Value::U64(5)])
        .await;

    let result2 = executor
        .invoke_and_await(
            &worker_id,
            "golem:component/api.{add}",
            vec![Value::U64(50)],
        )
        .await;

    let result3 = executor
        .invoke_and_await(&worker_id, "golem:component/api.{get}", vec![])
        .await;

    let (metadata, last_error) = executor.get_worker_metadata(&worker_id).await.unwrap();

    let (next, all) = executor
        .get_workers_metadata(&component_id, None, ScanCursor::default(), 100, true)
        .await;

    drop(executor);

    info!(
        "result2: {:?}",
        worker_error_message(&result2.clone().err().unwrap())
    );
    info!(
        "result3: {:?}",
        worker_error_message(&result3.clone().err().unwrap())
    );

    check!(result1.is_ok());
    check!(result2.is_err());
    check!(result3.is_err());

    let expected_stderr = "\n\nthread '<unnamed>' panicked at src/lib.rs:29:17:\nvalue is too large\nnote: run with `RUST_BACKTRACE=1` environment variable to display a backtrace\n";

    check!(worker_error_message(&result2.clone().err().unwrap()).ends_with(&expected_stderr));
    check!(worker_error_message(&result3.clone().err().unwrap()).ends_with(&expected_stderr));

    check!(metadata.last_known_status.status == WorkerStatus::Failed);
    check!(last_error.is_some());
    check!(last_error.unwrap().ends_with(&expected_stderr));

    check!(next.is_none());
    check!(all.len() == 1);
    check!(all[0].1.is_some());
    check!(all[0].1.clone().unwrap().ends_with(&expected_stderr));
}
