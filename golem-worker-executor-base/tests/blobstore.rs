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
use assert2::check;
use golem_test_framework::dsl::TestDslUnsafe;
use golem_wasm_rpc::Value;

#[tokio::test]
#[tracing::instrument]
async fn blobstore_exists_return_true_if_the_container_was_created() {
    let context = TestContext::new();
    let executor = start(&context).await.unwrap();

    let component_id = executor.store_component("blob-store-service").await;
    let worker_name = "blob-store-service-1";
    let worker_id = executor.start_worker(&component_id, worker_name).await;

    let _ = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{create-container}",
            vec![Value::String(format!(
                "{component_id}-{worker_name}-container"
            ))],
        )
        .await
        .unwrap();

    let result = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{container-exists}",
            vec![Value::String(format!(
                "{component_id}-{worker_name}-container"
            ))],
        )
        .await
        .unwrap();

    drop(executor);

    check!(result == vec![Value::Bool(true)]);
}

#[tokio::test]
#[tracing::instrument]
async fn blobstore_exists_return_false_if_the_container_was_not_created() {
    let context = TestContext::new();
    let executor = start(&context).await.unwrap();

    let component_id = executor.store_component("blob-store-service").await;
    let worker_name = "blob-store-service-1";
    let worker_id = executor.start_worker(&component_id, worker_name).await;

    let result = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{container-exists}",
            vec![Value::String(format!(
                "{component_id}-{worker_name}-container"
            ))],
        )
        .await
        .unwrap();

    drop(executor);

    check!(result == vec![Value::Bool(false)]);
}
