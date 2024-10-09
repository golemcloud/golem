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

use crate::common::{start, TestContext};
use golem_common::model::oplog::OplogIndex;
use golem_common::model::public_oplog::{ExportedFunctionInvokedParameters, PublicOplogEntry};
use golem_common::model::{IdempotencyKey, WorkerId};
use golem_test_framework::dsl::TestDslUnsafe;

#[tokio::test]
#[tracing::instrument]
async fn get_oplog_1() {
    let context = TestContext::new();
    let executor = start(&context).await.unwrap();

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

    assert_eq!(oplog.len(), 14);
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
