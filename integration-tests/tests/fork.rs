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

use crate::Tracing;
use axum::extract::Path;
use axum::routing::get;
use axum::{Json, Router};
use golem_common::model::oplog::OplogIndex;
use golem_common::model::public_oplog::PublicOplogEntry;
use golem_common::model::{IdempotencyKey, WorkerId, WorkerStatus};
use golem_test_framework::config::{EnvBasedTestDependencies, TestDependencies};
use golem_test_framework::dsl::TestDslUnsafe;
use golem_wasm_rpc::{IntoValueAndType, Record, Value};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use test_r::{flaky, inherit_test_dep, test, timeout};
use tracing::{info, Instrument};
use uuid::Uuid;

inherit_test_dep!(Tracing);
inherit_test_dep!(EnvBasedTestDependencies);

#[test]
#[tracing::instrument]
#[timeout(120000)]
async fn fork_interrupted_worker(deps: &EnvBasedTestDependencies, _tracing: &Tracing) {
    let admin = deps.admin().await;
    let response = Arc::new(Mutex::new("initial".to_string()));
    let host_http_port = 8586;

    let source_worker_name = Uuid::new_v4().to_string();

    let http_server = run_http_server(&response, host_http_port);

    let component_id = admin.component("http-client-2").store().await;
    let mut env = HashMap::new();
    env.insert("PORT".to_string(), host_http_port.to_string());

    let worker_id = admin
        .start_worker_with(
            &component_id,
            source_worker_name.as_str(),
            vec![],
            env,
            vec![],
        )
        .await;

    let target_worker_name = Uuid::new_v4().to_string();

    let target_worker_id = WorkerId {
        component_id: component_id.clone(),
        worker_name: target_worker_name,
    };

    admin.log_output(&worker_id).await;

    admin
        .invoke(
            &worker_id,
            "golem:it/api.{start-polling}",
            vec!["first".into_value_and_type()],
        )
        .await
        .unwrap();

    admin
        .wait_for_status(&worker_id, WorkerStatus::Running, Duration::from_secs(10))
        .await;

    admin.interrupt(&worker_id).await;

    let oplog = admin.get_oplog(&worker_id, OplogIndex::INITIAL).await;

    let last_index = OplogIndex::from_u64(oplog.len() as u64);

    admin
        .fork_worker(&worker_id, &target_worker_id, last_index)
        .await;

    {
        let mut response = response.lock().unwrap();
        *response = "first".to_string();
    }

    admin
        .wait_for_status(
            &target_worker_id,
            WorkerStatus::Idle,
            Duration::from_secs(10),
        )
        .await;

    let result = admin
        .search_oplog(&target_worker_id, "Received first")
        .await;

    http_server.abort();

    assert_eq!(result.len(), 1);
}

#[test]
#[tracing::instrument]
#[timeout(120000)]
async fn fork_running_worker_1(deps: &EnvBasedTestDependencies, _tracing: &Tracing) {
    let admin = deps.admin().await;

    let component_id = admin.component("shopping-cart").store().await;

    let source_worker_name = Uuid::new_v4().to_string();

    let source_worker_id = WorkerId {
        component_id: component_id.clone(),
        worker_name: source_worker_name,
    };

    let _ = admin
        .invoke_and_await(
            &source_worker_id,
            "golem:it/api.{initialize-cart}",
            vec!["test-user-1".into_value_and_type()],
        )
        .await;

    let _ = admin
        .invoke_and_await(
            &source_worker_id,
            "golem:it/api.{add-item}",
            vec![Record(vec![
                ("product-id", "G1002".into_value_and_type()),
                ("name", "Mud Golem".into_value_and_type()),
                ("price", 11.0f32.into_value_and_type()),
                ("quantity", 10u32.into_value_and_type()),
            ])
            .into_value_and_type()],
        )
        .await;

    let target_worker_name = Uuid::new_v4().to_string();

    let target_worker_id = WorkerId {
        component_id: component_id.clone(),
        worker_name: target_worker_name,
    };

    let source_oplog = admin
        .get_oplog(&source_worker_id, OplogIndex::INITIAL)
        .await;

    let oplog_index_of_function_invoked: OplogIndex = OplogIndex::from_u64(3);

    let log_record = source_oplog
        .get(u64::from(oplog_index_of_function_invoked) as usize - 1)
        .expect("Expect at least one entry in source oplog");

    assert!(matches!(
        &log_record.entry,
        PublicOplogEntry::ExportedFunctionInvoked(_)
    ));

    let _ = admin
        .fork_worker(
            &source_worker_id,
            &target_worker_id,
            oplog_index_of_function_invoked,
        )
        .await;

    admin
        .wait_for_status(
            &target_worker_id,
            WorkerStatus::Idle,
            Duration::from_secs(10),
        )
        .await;

    let total_cart_initialisation = admin
        .search_oplog(&target_worker_id, "initialize-cart AND NOT pending")
        .await;

    // Since the fork point was before the completion, it re-intitialises making the total initialisation
    // records 2 along with the new log in target worker.
    assert_eq!(total_cart_initialisation.len(), 2);
}

#[test]
#[ignore]
#[tracing::instrument]
#[flaky(5)]
#[timeout(120000)]
async fn fork_running_worker_2(deps: &EnvBasedTestDependencies, _tracing: &Tracing) {
    let admin = deps.admin().await;
    let response = Arc::new(Mutex::new("initial".to_string()));
    let host_http_port = 8587;
    let http_server = run_http_server(&response, host_http_port);

    let component_id = admin.component("http-client-2").store().await;
    let mut env = HashMap::new();
    env.insert("PORT".to_string(), host_http_port.to_string());

    let source_worker_name = Uuid::new_v4().to_string();
    let source_worker_id = admin
        .start_worker_with(
            &component_id,
            source_worker_name.as_str(),
            vec![],
            env,
            vec![],
        )
        .await;

    let target_worker_name = Uuid::new_v4().to_string();

    let target_worker_id = WorkerId {
        component_id: component_id.clone(),
        worker_name: target_worker_name,
    };

    admin.log_output(&source_worker_id).await;

    admin
        .invoke(
            &source_worker_id,
            "golem:it/api.{start-polling}",
            vec!["first".into_value_and_type()],
        )
        .await
        .unwrap();

    admin
        .wait_for_status(
            &source_worker_id,
            WorkerStatus::Running,
            Duration::from_secs(10),
        )
        .await;

    let oplog = admin
        .get_oplog(&source_worker_id, OplogIndex::INITIAL)
        .await;

    let last_index = OplogIndex::from_u64(oplog.len() as u64);

    admin
        .fork_worker(&source_worker_id, &target_worker_id, last_index)
        .await;

    {
        let mut response = response.lock().unwrap();
        *response = "first".to_string();
    }

    admin
        .wait_for_status(
            &target_worker_id,
            WorkerStatus::Idle,
            Duration::from_secs(20),
        )
        .await;

    admin
        .wait_for_status(
            &source_worker_id,
            WorkerStatus::Idle,
            Duration::from_secs(20),
        )
        .await;

    let target_result = admin
        .search_oplog(&target_worker_id, "Received first")
        .await;
    let source_result = admin
        .search_oplog(&source_worker_id, "Received first")
        .await;

    http_server.abort();

    assert_eq!(target_result.len(), 1);
    assert_eq!(source_result.len(), 1);
}

#[test]
#[tracing::instrument]
#[timeout(120000)]
async fn fork_idle_worker(deps: &EnvBasedTestDependencies, _tracing: &Tracing) {
    let admin = deps.admin().await;
    let component_id = admin.component("shopping-cart").store().await;

    let source_worker_name = Uuid::new_v4().to_string();

    let source_worker_id = WorkerId {
        component_id: component_id.clone(),
        worker_name: source_worker_name,
    };

    let _ = admin
        .invoke_and_await(
            &source_worker_id,
            "golem:it/api.{initialize-cart}",
            vec!["test-user-1".into_value_and_type()],
        )
        .await;

    let _ = admin
        .invoke_and_await(
            &source_worker_id,
            "golem:it/api.{add-item}",
            vec![Record(vec![
                ("product-id", "G1001".into_value_and_type()),
                ("name", "Golem Cloud Subscription 1y".into_value_and_type()),
                ("price", 999999.0f32.into_value_and_type()),
                ("quantity", 1u32.into_value_and_type()),
            ])
            .into_value_and_type()],
        )
        .await;

    let _ = admin
        .invoke_and_await(
            &source_worker_id,
            "golem:it/api.{add-item}",
            vec![Record(vec![
                ("product-id", "G1002".into_value_and_type()),
                ("name", "Mud Golem".into_value_and_type()),
                ("price", 11.0f32.into_value_and_type()),
                ("quantity", 10u32.into_value_and_type()),
            ])
            .into_value_and_type()],
        )
        .await;

    let target_worker_name = Uuid::new_v4().to_string();

    let target_worker_id = WorkerId {
        component_id: component_id.clone(),
        worker_name: target_worker_name,
    };

    let source_oplog = admin
        .get_oplog(&source_worker_id, OplogIndex::INITIAL)
        .await;

    let log_record = source_oplog
        .last()
        .expect("Expect at least one entry in source oplog");

    assert!(matches!(
        &log_record.entry,
        PublicOplogEntry::ExportedFunctionCompleted(_)
    ));

    let _ = admin
        .fork_worker(
            &source_worker_id,
            &target_worker_id,
            OplogIndex::from_u64(source_oplog.len() as u64),
        )
        .await;

    //Invoking G1002 again in forked worker
    let _ = admin
        .invoke_and_await(
            &target_worker_id,
            "golem:it/api.{add-item}",
            vec![Record(vec![
                ("product-id", "G1002".into_value_and_type()),
                ("name", "Mud Golem".into_value_and_type()),
                ("price", 11.0f32.into_value_and_type()),
                ("quantity", 10u32.into_value_and_type()),
            ])
            .into_value_and_type()],
        )
        .await;

    let _ = admin
        .invoke_and_await(
            &target_worker_id,
            "golem:it/api.{update-item-quantity}",
            vec!["G1002".into_value_and_type(), 20u32.into_value_and_type()],
        )
        .await;

    let original_contents = admin
        .invoke_and_await(
            &source_worker_id,
            "golem:it/api.{get-cart-contents}",
            vec![],
        )
        .await
        .unwrap();

    let forked_contents = admin
        .invoke_and_await(
            &target_worker_id,
            "golem:it/api.{get-cart-contents}",
            vec![],
        )
        .await
        .unwrap();

    let result1 = admin
        .search_oplog(&target_worker_id, "G1002 AND NOT pending")
        .await;
    let result2 = admin
        .search_oplog(&target_worker_id, "G1001 AND NOT pending")
        .await;

    assert_eq!(result1.len(), 7); //  three invocations for G1002 and three log messages and the final get-cart-contents invocation
    assert_eq!(result2.len(), 3); //  one invocation and one log for G1001 which was in the original source oplog and the final get-cart-contents invocation

    assert_eq!(
        original_contents,
        vec![Value::List(vec![
            Record(vec![
                ("product-id", "G1001".into_value_and_type()),
                ("name", "Golem Cloud Subscription 1y".into_value_and_type()),
                ("price", 999999.0f32.into_value_and_type()),
                ("quantity", 1u32.into_value_and_type()),
            ])
            .into_value_and_type()
            .value,
            Record(vec![
                ("product-id", "G1002".into_value_and_type()),
                ("name", "Mud Golem".into_value_and_type()),
                ("price", 11.0f32.into_value_and_type()),
                ("quantity", 10u32.into_value_and_type()),
            ])
            .into_value_and_type()
            .value,
        ])]
    );
    assert_eq!(
        forked_contents,
        vec![Value::List(vec![
            Record(vec![
                ("product-id", "G1001".into_value_and_type()),
                ("name", "Golem Cloud Subscription 1y".into_value_and_type()),
                ("price", 999999.0f32.into_value_and_type()),
                ("quantity", 1u32.into_value_and_type()),
            ])
            .into_value_and_type()
            .value,
            Record(vec![
                ("product-id", "G1002".into_value_and_type()),
                ("name", "Mud Golem".into_value_and_type()),
                ("price", 11.0f32.into_value_and_type()),
                ("quantity", 20u32.into_value_and_type()), // Updated quantity
            ])
            .into_value_and_type()
            .value,
            Record(vec![
                ("product-id", "G1002".into_value_and_type()),
                ("name", "Mud Golem".into_value_and_type()),
                ("price", 11.0f32.into_value_and_type()),
                ("quantity", 20u32.into_value_and_type()), // Added quantity
            ])
            .into_value_and_type()
            .value
        ])]
    )
}

#[test]
#[tracing::instrument]
#[timeout(120000)]
async fn fork_worker_when_target_already_exists(
    deps: &EnvBasedTestDependencies,
    _tracing: &Tracing,
) {
    let admin = deps.admin().await;
    let component_id = admin.component("shopping-cart").store().await;

    let source_worker_name = Uuid::new_v4().to_string();

    let source_worker_id = WorkerId {
        component_id: component_id.clone(),
        worker_name: source_worker_name,
    };

    let _ = admin
        .invoke_and_await(
            &source_worker_id,
            "golem:it/api.{initialize-cart}",
            vec!["test-user-1".into_value_and_type()],
        )
        .await;

    let second_call_oplogs = admin
        .search_oplog(&source_worker_id, "initialize-cart")
        .await;

    let index = second_call_oplogs
        .last()
        .expect("Expect at least one entry for the product id G1001")
        .oplog_index;

    let error = golem_test_framework::dsl::TestDsl::fork_worker(
        &admin,
        &source_worker_id,
        &source_worker_id,
        index,
    )
    .await
    .unwrap_err()
    .to_string();

    assert!(error.contains("WorkerAlreadyExists"));
}

#[test]
#[tracing::instrument]
#[timeout(120000)]
async fn fork_worker_with_invalid_oplog_index_cut_off(
    deps: &EnvBasedTestDependencies,
    _tracing: &Tracing,
) {
    let admin = deps.admin().await;
    let component_id = admin.component("shopping-cart").store().await;

    let source_worker_name = Uuid::new_v4().to_string();

    let source_worker_id = WorkerId {
        component_id: component_id.clone(),
        worker_name: source_worker_name,
    };

    let _ = admin
        .invoke_and_await(
            &source_worker_id,
            "golem:it/api.{initialize-cart}",
            vec!["test-user-1".into_value_and_type()],
        )
        .await;

    let target_worker_name = Uuid::new_v4().to_string();

    let target_worker_id = WorkerId {
        component_id: component_id.clone(),
        worker_name: target_worker_name,
    };

    let error = golem_test_framework::dsl::TestDsl::fork_worker(
        &admin,
        &source_worker_id,
        &target_worker_id,
        OplogIndex::INITIAL,
    )
    .await
    .unwrap_err()
    .to_string();

    assert!(error.contains("oplog_index_cut_off must be at least 2"));
}

#[test]
#[tracing::instrument]
#[timeout(120000)]
async fn fork_invalid_worker(deps: &EnvBasedTestDependencies, _tracing: &Tracing) {
    let admin = deps.admin().await;
    let component_id = admin.component("shopping-cart").store().await;

    let source_worker_name = Uuid::new_v4().to_string();

    let source_worker_id = WorkerId {
        component_id: component_id.clone(),
        worker_name: source_worker_name,
    };

    let target_worker_id = WorkerId {
        component_id: component_id.clone(),
        worker_name: "forked-buz".to_string(),
    };

    let error = golem_test_framework::dsl::TestDsl::fork_worker(
        &admin,
        &source_worker_id,
        &target_worker_id,
        OplogIndex::from_u64(14),
    )
    .await
    .unwrap_err()
    .to_string();

    assert!(error.contains(&format!("Worker not found: {source_worker_id}")));
}

// Divergence possibility is mainly respect to environment variables referring to worker-ids.
// Fork shouldn't change the original environment variable values of the source worker
// stored in oplog until cut off
#[test]
#[tracing::instrument]
#[timeout(120000)]
async fn fork_worker_ensures_zero_divergence_until_cut_off(
    deps: &EnvBasedTestDependencies,
    _tracing: &Tracing,
) {
    let admin = deps.admin().await;
    let component_id = admin.component("environment-service").store().await;

    let source_worker_name = Uuid::new_v4().to_string();

    let source_worker_id = WorkerId {
        component_id: component_id.clone(),
        worker_name: source_worker_name.clone(),
    };

    let _ = admin
        .invoke_and_await(&source_worker_id, "golem:it/api.{get-environment}", vec![])
        .await
        .unwrap();

    // The worker name is foo
    let expected = Value::Result(Ok(Some(Box::new(Value::List(vec![
        Value::Tuple(vec![
            Value::String("GOLEM_AGENT_ID".to_string()),
            Value::String(source_worker_name.clone()),
        ]),
        Value::Tuple(vec![
            Value::String("GOLEM_WORKER_NAME".to_string()),
            Value::String(source_worker_name),
        ]),
        Value::Tuple(vec![
            Value::String("GOLEM_COMPONENT_ID".to_string()),
            Value::String(format!("{component_id}")),
        ]),
        Value::Tuple(vec![
            Value::String("GOLEM_COMPONENT_VERSION".to_string()),
            Value::String("0".to_string()),
        ]),
    ])))));

    let target_worker_name = Uuid::new_v4().to_string();

    let target_worker_id = WorkerId {
        component_id: component_id.clone(),
        worker_name: target_worker_name,
    };

    let oplog = admin
        .get_oplog(&source_worker_id, OplogIndex::INITIAL)
        .await;

    // We fork the worker post the completion and see if oplog corresponding to environment value
    // has the same value as foo. As far as the fork cut off point is post the completion, there
    // shouldn't be any divergence for worker information even if forked worker name
    // is different from the source worker name
    let _ = admin
        .fork_worker(
            &source_worker_id,
            &target_worker_id,
            OplogIndex::from_u64(oplog.len() as u64),
        )
        .await;

    let entry = oplog.last().unwrap().clone();

    match entry.entry {
        PublicOplogEntry::ExportedFunctionCompleted(parameters) => {
            assert_eq!(parameters.response.map(|vat| vat.value), Some(expected));
        }
        _ => panic!("Expected ExportedFunctionCompleted"),
    };
}

fn run_http_server(
    response: &Arc<Mutex<String>>,
    host_http_port: u16,
) -> tokio::task::JoinHandle<()> {
    let response_clone = response.clone();

    tokio::spawn(
        async move {
            let route = Router::new().route(
                "/poll",
                get(move || async move {
                    let body = response_clone.lock().unwrap();
                    body.clone()
                }),
            );

            let listener = tokio::net::TcpListener::bind(
                format!("0.0.0.0:{host_http_port}")
                    .parse::<SocketAddr>()
                    .unwrap(),
            )
            .await
            .unwrap();
            axum::serve(listener, route).await.unwrap();
        }
        .in_current_span(),
    )
}

#[test]
#[tracing::instrument]
#[timeout(120000)]
async fn fork_self(deps: &EnvBasedTestDependencies, _tracing: &Tracing) {
    let admin = deps.admin().await;
    let component_id = admin.component("golem-rust-tests").store().await;

    let (port_tx, port_rx) = tokio::sync::oneshot::channel::<u16>();
    let http_server = tokio::spawn(
        async move {
            let route = Router::new()
                .route(
                    "/fork-test/step1/:name/:input",
                    get(move |args: Path<(String, String)>| async move {
                        Json(format!("{}-{}", args.0 .0, args.0 .1))
                    }),
                )
                .route(
                    "/fork-test/step2/:name/:fork/:input",
                    get(move |args: Path<(String, String, String)>| async move {
                        Json(format!("{}-{}-{}", args.0 .0, args.0 .1, args.0 .2))
                    }),
                );

            let listener =
                tokio::net::TcpListener::bind("0.0.0.0:0".parse::<SocketAddr>().unwrap())
                    .await
                    .unwrap();

            port_tx.send(listener.local_addr().unwrap().port()).unwrap();
            axum::serve(listener, route).await.unwrap();
        }
        .in_current_span(),
    );

    let port = port_rx.await.unwrap();
    let mut env = HashMap::new();
    env.insert("PORT".to_string(), port.to_string());

    info!("Using environment: {:?}", env);

    let worker_id = admin
        .start_worker_with(&component_id, "source-worker", vec![], env, vec![])
        .await;

    let _ = admin.log_output(&worker_id).await;

    let idempotency_key = IdempotencyKey::fresh();
    let source_result = admin
        .invoke_and_await_with_key(
            &worker_id,
            &idempotency_key,
            "golem:it/api.{fork-test}",
            vec!["hello".into_value_and_type()],
        )
        .await
        .unwrap();

    let target_worker_id = WorkerId {
        component_id: component_id.clone(),
        worker_name: "forked-worker".to_string(),
    };
    let target_result = admin
        .invoke_and_await_with_key(
            &target_worker_id,
            &idempotency_key,
            "golem:it/api.{fork-test}",
            vec!["hello".into_value_and_type()],
        )
        .await
        .unwrap();

    http_server.abort();

    assert_eq!(
        source_result,
        vec![Value::String(
            "source-worker-hello::source-worker-original-hello".to_string()
        )]
    );
    assert_eq!(
        target_result,
        vec![Value::String(
            "source-worker-hello::forked-worker-forked-hello".to_string()
        )]
    );
}
