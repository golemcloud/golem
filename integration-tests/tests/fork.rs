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

use axum::routing::get;
use axum::Router;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use test_r::{flaky, inherit_test_dep, test, timeout};

use crate::Tracing;
use golem_common::model::oplog::OplogIndex;
use golem_common::model::public_oplog::PublicOplogEntry;
use golem_common::model::{WorkerId, WorkerStatus};

use golem_test_framework::config::EnvBasedTestDependencies;
use golem_test_framework::dsl::TestDslUnsafe;

use golem_wasm_rpc::Value;
use uuid::Uuid;

inherit_test_dep!(Tracing);
inherit_test_dep!(EnvBasedTestDependencies);

#[test]
#[tracing::instrument]
#[timeout(120000)]
async fn fork_interrupted_worker(deps: &EnvBasedTestDependencies, _tracing: &Tracing) {
    let response = Arc::new(Mutex::new("initial".to_string()));
    let host_http_port = 8586;

    let source_worker_name = Uuid::new_v4().to_string();

    let http_server = run_http_server(&response, host_http_port);

    let component_id = deps.store_component("http-client-2").await;
    let mut env = HashMap::new();
    env.insert("PORT".to_string(), host_http_port.to_string());

    let worker_id = deps
        .start_worker_with(&component_id, source_worker_name.as_str(), vec![], env)
        .await;

    let target_worker_name = Uuid::new_v4().to_string();

    let target_worker_id = WorkerId {
        component_id: component_id.clone(),
        worker_name: target_worker_name,
    };

    deps.log_output(&worker_id).await;

    deps.invoke(
        &worker_id,
        "golem:it/api.{start-polling}",
        vec![Value::String("first".to_string())],
    )
    .await
    .unwrap();

    deps.wait_for_status(&worker_id, WorkerStatus::Running, Duration::from_secs(10))
        .await;

    deps.interrupt(&worker_id).await;

    let oplog = deps.get_oplog(&worker_id, OplogIndex::INITIAL).await;

    let last_index = OplogIndex::from_u64(oplog.len() as u64);

    deps.fork_worker(&worker_id, &target_worker_id, last_index)
        .await;

    {
        let mut response = response.lock().unwrap();
        *response = "first".to_string();
    }

    deps.wait_for_status(
        &target_worker_id,
        WorkerStatus::Idle,
        Duration::from_secs(10),
    )
    .await;

    let result = deps.search_oplog(&target_worker_id, "Received first").await;

    http_server.abort();

    assert_eq!(result.len(), 1);
}

#[test]
#[tracing::instrument]
#[timeout(120000)]
async fn fork_running_worker_1(deps: &EnvBasedTestDependencies, _tracing: &Tracing) {
    let component_id = deps.store_component("shopping-cart").await;

    let source_worker_name = Uuid::new_v4().to_string();

    let source_worker_id = WorkerId {
        component_id: component_id.clone(),
        worker_name: source_worker_name,
    };

    let _ = deps
        .invoke_and_await(
            &source_worker_id,
            "golem:it/api.{initialize-cart}",
            vec![Value::String("test-user-1".to_string())],
        )
        .await;

    let _ = deps
        .invoke_and_await(
            &source_worker_id,
            "golem:it/api.{add-item}",
            vec![Value::Record(vec![
                Value::String("G1002".to_string()),
                Value::String("Mud Golem".to_string()),
                Value::F32(11.0),
                Value::U32(10),
            ])],
        )
        .await;

    let target_worker_name = Uuid::new_v4().to_string();

    let target_worker_id = WorkerId {
        component_id: component_id.clone(),
        worker_name: target_worker_name,
    };

    let source_oplog = deps.get_oplog(&source_worker_id, OplogIndex::INITIAL).await;

    let oplog_index_of_function_invoked: OplogIndex = OplogIndex::from_u64(3);

    let log_record = source_oplog
        .get(u64::from(oplog_index_of_function_invoked) as usize - 1)
        .expect("Expect at least one entry in source oplog");

    assert!(matches!(
        log_record,
        PublicOplogEntry::ExportedFunctionInvoked(_)
    ));

    let _ = deps
        .fork_worker(
            &source_worker_id,
            &target_worker_id,
            oplog_index_of_function_invoked,
        )
        .await;

    deps.wait_for_status(
        &target_worker_id,
        WorkerStatus::Idle,
        Duration::from_secs(10),
    )
    .await;

    let total_cart_initialisation = deps
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
    let response = Arc::new(Mutex::new("initial".to_string()));
    let host_http_port = 8587;
    let http_server = run_http_server(&response, host_http_port);

    let component_id = deps.store_component("http-client-2").await;
    let mut env = HashMap::new();
    env.insert("PORT".to_string(), host_http_port.to_string());

    let source_worker_name = Uuid::new_v4().to_string();
    let source_worker_id = deps
        .start_worker_with(&component_id, source_worker_name.as_str(), vec![], env)
        .await;

    let target_worker_name = Uuid::new_v4().to_string();

    let target_worker_id = WorkerId {
        component_id: component_id.clone(),
        worker_name: target_worker_name,
    };

    deps.log_output(&source_worker_id).await;

    deps.invoke(
        &source_worker_id,
        "golem:it/api.{start-polling}",
        vec![Value::String("first".to_string())],
    )
    .await
    .unwrap();

    deps.wait_for_status(
        &source_worker_id,
        WorkerStatus::Running,
        Duration::from_secs(10),
    )
    .await;

    let oplog = deps.get_oplog(&source_worker_id, OplogIndex::INITIAL).await;

    let last_index = OplogIndex::from_u64(oplog.len() as u64);

    deps.fork_worker(&source_worker_id, &target_worker_id, last_index)
        .await;

    {
        let mut response = response.lock().unwrap();
        *response = "first".to_string();
    }

    deps.wait_for_status(
        &target_worker_id,
        WorkerStatus::Idle,
        Duration::from_secs(20),
    )
    .await;

    deps.wait_for_status(
        &source_worker_id,
        WorkerStatus::Idle,
        Duration::from_secs(20),
    )
    .await;

    let target_result = deps.search_oplog(&target_worker_id, "Received first").await;
    let source_result = deps.search_oplog(&source_worker_id, "Received first").await;

    http_server.abort();

    assert_eq!(target_result.len(), 1);
    assert_eq!(source_result.len(), 1);
}

#[test]
#[tracing::instrument]
#[timeout(120000)]
async fn fork_idle_worker(deps: &EnvBasedTestDependencies, _tracing: &Tracing) {
    let component_id = deps.store_component("shopping-cart").await;

    let source_worker_name = Uuid::new_v4().to_string();

    let source_worker_id = WorkerId {
        component_id: component_id.clone(),
        worker_name: source_worker_name,
    };

    let _ = deps
        .invoke_and_await(
            &source_worker_id,
            "golem:it/api.{initialize-cart}",
            vec![Value::String("test-user-1".to_string())],
        )
        .await;

    let _ = deps
        .invoke_and_await(
            &source_worker_id,
            "golem:it/api.{add-item}",
            vec![Value::Record(vec![
                Value::String("G1001".to_string()),
                Value::String("Golem Cloud Subscription 1y".to_string()),
                Value::F32(999999.0),
                Value::U32(1),
            ])],
        )
        .await;

    let _ = deps
        .invoke_and_await(
            &source_worker_id,
            "golem:it/api.{add-item}",
            vec![Value::Record(vec![
                Value::String("G1002".to_string()),
                Value::String("Mud Golem".to_string()),
                Value::F32(11.0),
                Value::U32(10),
            ])],
        )
        .await;

    let target_worker_name = Uuid::new_v4().to_string();

    let target_worker_id = WorkerId {
        component_id: component_id.clone(),
        worker_name: target_worker_name,
    };

    let source_oplog = deps.get_oplog(&source_worker_id, OplogIndex::INITIAL).await;

    let oplog_index_of_function_completed_g1001 = OplogIndex::from_u64(11);

    // Minus 1 as oplog index starts from 1
    let log_record = source_oplog
        .get(u64::from(oplog_index_of_function_completed_g1001) as usize - 1)
        .expect("Expect at least one entry in source oplog");

    assert!(matches!(
        log_record,
        PublicOplogEntry::ExportedFunctionCompleted(_)
    ));

    let _ = deps
        .fork_worker(
            &source_worker_id,
            &target_worker_id,
            oplog_index_of_function_completed_g1001,
        )
        .await;

    //Invoking G1002 again in forked worker
    let _ = deps
        .invoke_and_await(
            &target_worker_id,
            "golem:it/api.{add-item}",
            vec![Value::Record(vec![
                Value::String("G1002".to_string()),
                Value::String("Mud Golem".to_string()),
                Value::F32(11.0),
                Value::U32(10),
            ])],
        )
        .await;

    let _ = deps
        .invoke_and_await(
            &target_worker_id,
            "golem:it/api.{update-item-quantity}",
            vec![Value::String("G1002".to_string()), Value::U32(20)],
        )
        .await;

    let result1 = deps
        .search_oplog(&target_worker_id, "G1002 AND NOT pending")
        .await;
    let result2 = deps
        .search_oplog(&target_worker_id, "G1001 AND NOT pending")
        .await;

    assert_eq!(result1.len(), 4); //  two invocations for G1002 and two log messages preceded
    assert_eq!(result2.len(), 2); //  two invocations for G1001 which was in the original source oplog
}

#[test]
#[tracing::instrument]
#[timeout(120000)]
async fn fork_worker_when_target_already_exists(
    deps: &EnvBasedTestDependencies,
    _tracing: &Tracing,
) {
    let component_id = deps.store_component("shopping-cart").await;

    let source_worker_name = Uuid::new_v4().to_string();

    let source_worker_id = WorkerId {
        component_id: component_id.clone(),
        worker_name: source_worker_name,
    };

    let _ = deps
        .invoke_and_await(
            &source_worker_id,
            "golem:it/api.{initialize-cart}",
            vec![Value::String("test-user-1".to_string())],
        )
        .await;

    let second_call_oplogs = deps
        .search_oplog(&source_worker_id, "initialize-cart")
        .await;

    let index = second_call_oplogs
        .last()
        .expect("Expect at least one entry for the product id G1001")
        .oplog_index;

    let error = golem_test_framework::dsl::TestDsl::fork_worker(
        deps,
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
    let component_id = deps.store_component("shopping-cart").await;

    let source_worker_name = Uuid::new_v4().to_string();

    let source_worker_id = WorkerId {
        component_id: component_id.clone(),
        worker_name: source_worker_name,
    };

    let _ = deps
        .invoke_and_await(
            &source_worker_id,
            "golem:it/api.{initialize-cart}",
            vec![Value::String("test-user-1".to_string())],
        )
        .await;

    let target_worker_name = Uuid::new_v4().to_string();

    let target_worker_id = WorkerId {
        component_id: component_id.clone(),
        worker_name: target_worker_name,
    };

    let error = golem_test_framework::dsl::TestDsl::fork_worker(
        deps,
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
    let component_id = deps.store_component("shopping-cart").await;

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
        deps,
        &source_worker_id,
        &target_worker_id,
        OplogIndex::from_u64(14),
    )
    .await
    .unwrap_err()
    .to_string();

    assert!(error.contains("WorkerNotFound"));
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
    let component_id = deps.store_component("environment-service").await;

    let source_worker_name = Uuid::new_v4().to_string();

    let source_worker_id = WorkerId {
        component_id: component_id.clone(),
        worker_name: source_worker_name.clone(),
    };

    let _ = deps
        .invoke_and_await(&source_worker_id, "golem:it/api.{get-environment}", vec![])
        .await
        .unwrap();

    // The worker name is foo
    let expected = Value::Tuple(vec![Value::Result(Ok(Some(Box::new(Value::List(vec![
        Value::Tuple(vec![
            Value::String("GOLEM_WORKER_NAME".to_string()),
            Value::String(source_worker_name),
        ]),
        Value::Tuple(vec![
            Value::String("GOLEM_COMPONENT_ID".to_string()),
            Value::String(format!("{}", component_id)),
        ]),
        Value::Tuple(vec![
            Value::String("GOLEM_COMPONENT_VERSION".to_string()),
            Value::String("0".to_string()),
        ]),
    ])))))]);

    let target_worker_name = Uuid::new_v4().to_string();

    let target_worker_id = WorkerId {
        component_id: component_id.clone(),
        worker_name: target_worker_name,
    };

    // We fork the worker post the completion and see if oplog corresponding to environment value
    // has the same value as foo. As far as the fork cut off point is post the completion, there
    // shouldn't be any divergence for worker information even if forked worker name
    // is different from the source worker name
    let _ = deps
        .fork_worker(
            &source_worker_id,
            &target_worker_id,
            OplogIndex::from_u64(7),
        )
        .await;

    let result = deps
        .get_oplog(&target_worker_id, OplogIndex::from_u64(7))
        .await;

    let entry = result.last().unwrap().clone();

    match entry {
        PublicOplogEntry::ExportedFunctionCompleted(parameters) => {
            assert_eq!(parameters.response.value, expected);
        }
        _ => panic!("Expected ExportedFunctionCompleted"),
    };
}

fn run_http_server(
    response: &Arc<Mutex<String>>,
    host_http_port: u16,
) -> tokio::task::JoinHandle<()> {
    let response_clone = response.clone();

    tokio::spawn(async move {
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
    })
}
