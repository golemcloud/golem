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

use assert2::check;
use golem_wasm_rpc::Value;
use log::info;
use test_r::{inherit_test_dep, test};

use crate::common::{start, TestContext};
use crate::{LastUniqueId, Tracing, WorkerExecutorTestDependencies};
use golem_common::model::oplog::OplogIndex;
use golem_common::model::public_oplog::{ExportedFunctionInvokedParameters, PublicOplogEntry};
use golem_common::model::{IdempotencyKey, WorkerId};
use golem_test_framework::dsl::TestDslUnsafe;

inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(LastUniqueId);
inherit_test_dep!(Tracing);

#[test]
#[tracing::instrument]
async fn get_oplog_1(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await.unwrap();

    let component_id = executor.store_component("runtime-service").await;

    let worker_id = WorkerId {
        component_id,
        worker_name: "getoplog1".to_string(),
    };

    let idempotency_key1 = IdempotencyKey::fresh();
    let idempotency_key2 = IdempotencyKey::fresh();

    let _ = executor
        .invoke_and_await(
            worker_id.clone(),
            "golem:it/api.{generate-idempotency-keys}",
            vec![],
        )
        .await
        .unwrap();
    let _ = executor
        .invoke_and_await_with_key(
            worker_id.clone(),
            &idempotency_key1,
            "golem:it/api.{generate-idempotency-keys}",
            vec![],
        )
        .await
        .unwrap();
    let _ = executor
        .invoke_and_await_with_key(
            worker_id.clone(),
            &idempotency_key2,
            "golem:it/api.{generate-idempotency-keys}",
            vec![],
        )
        .await
        .unwrap();

    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await;

    drop(executor);

    // Whether there is an "enqueued invocation" entry or just directly started invocation
    // depends on timing
    assert!(oplog.len() >= 12 && oplog.len() <= 14);
    assert!(matches!(oplog[0], PublicOplogEntry::Create(_)));
    assert_eq!(
        oplog
            .iter()
            .filter(
                |entry| matches!(entry, PublicOplogEntry::ExportedFunctionInvoked(
        ExportedFunctionInvokedParameters { function_name, .. }
    ) if function_name == "golem:it/api.{generate-idempotency-keys}")
            )
            .count(),
        3
    );
}

#[test]
#[tracing::instrument]
async fn search_oplog_1(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await.unwrap();

    let component_id = executor.store_component("shopping-cart").await;

    let worker_id = WorkerId {
        component_id,
        worker_name: "searchoplog1".to_string(),
    };

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

    let _ = executor
        .invoke_and_await(&worker_id, "golem:it/api.{get-cart-contents}", vec![])
        .await;

    let _ = executor
        .invoke_and_await(&worker_id, "golem:it/api.{checkout}", vec![])
        .await;

    let _oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await;

    let result1 = executor.search_oplog(&worker_id, "G1002").await;

    let result2 = executor.search_oplog(&worker_id, "imported-function").await;

    let result3 = executor
        .search_oplog(&worker_id, "product-id:G1001 OR product-id:G1000")
        .await;

    drop(executor);

    // println!("oplog\n{:#?}", oplog);
    // println!("result1\n{:#?}", result1);
    // println!("result2\n{:#?}", result2);
    // println!("result3\n{:#?}", result3);

    assert_eq!(result1.len(), 4); // two invocations and two log messages
    assert_eq!(result2.len(), 2); // get_preopened_directories, get_random_bytes
    assert_eq!(result3.len(), 2); // two invocations
}

#[test]
#[tracing::instrument]
async fn get_oplog_with_api_changing_updates(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await.unwrap();

    let component_id = executor.store_unique_component("update-test-v1").await;
    let worker_id = executor
        .start_worker(&component_id, "get_oplog_with_api_changing_updates")
        .await;

    let target_version = executor
        .update_component(&component_id, "update-test-v2")
        .await;
    info!("Updated component to version {target_version}");

    let _ = executor
        .invoke_and_await(&worker_id, "golem:component/api.{f3}", vec![])
        .await
        .unwrap();
    let _ = executor
        .invoke_and_await(&worker_id, "golem:component/api.{f3}", vec![])
        .await
        .unwrap();

    executor
        .auto_update_worker(&worker_id, target_version)
        .await;

    let result = executor
        .invoke_and_await(&worker_id, "golem:component/api.{f4}", vec![])
        .await
        .unwrap();

    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await;

    // there might be a pending invocation entry before the update entry. Filter it out to make the test more robust
    let oplog = oplog
        .into_iter()
        .filter(|entry| !matches!(entry, PublicOplogEntry::PendingWorkerInvocation(_)))
        .collect::<Vec<_>>();

    check!(result[0] == Value::U64(11));
    assert_eq!(oplog.len(), 13);
}

#[test]
#[tracing::instrument]
async fn get_oplog_starting_with_updated_component(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await.unwrap();

    let component_id = executor.store_unique_component("update-test-v1").await;
    let target_version = executor
        .update_component(&component_id, "update-test-v2")
        .await;
    info!("Updated component to version {target_version}");

    let worker_id = executor
        .start_worker(&component_id, "get_oplog_starting_with_updated_component")
        .await;
    let result = executor
        .invoke_and_await(&worker_id, "golem:component/api.{f4}", vec![])
        .await
        .unwrap();

    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await;

    check!(result[0] == Value::U64(11));
    assert_eq!(oplog.len(), 3);
    println!("oplog length\n{:#?}", oplog.len());
    println!("oplog\n{:#?}", oplog);
}
