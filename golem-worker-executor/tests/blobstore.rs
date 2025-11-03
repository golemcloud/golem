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

use test_r::{inherit_test_dep, test};

use crate::common::{start, TestContext};
use crate::{LastUniqueId, Tracing, WorkerExecutorTestDependencies};
use assert2::check;
use golem_test_framework::dsl::TestDsl;
use golem_wasm::{IntoValueAndType, Value};

inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(LastUniqueId);
inherit_test_dep!(Tracing);

#[test]
#[tracing::instrument]
async fn blobstore_exists_return_true_if_the_container_was_created(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component(&context.default_environment_id, "blob-store-service")
        .store()
        .await?;
    let worker_name = "blob-store-service-1";
    let worker_id = executor.start_worker(&component.id, worker_name).await?;

    let _ = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{create-container}",
            vec![format!("{}-{worker_name}-container", component.id).into_value_and_type()],
        )
        .await?
        .unwrap();

    let result = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{container-exists}",
            vec![format!("{}-{worker_name}-container", component.id).into_value_and_type()],
        )
        .await?
        .unwrap();

    executor.check_oplog_is_queryable(&worker_id).await?;

    drop(executor);

    check!(result == vec![Value::Bool(true)]);

    Ok(())
}

#[test]
#[tracing::instrument]
async fn blobstore_exists_return_false_if_the_container_was_not_created(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component(&context.default_environment_id, "blob-store-service")
        .store()
        .await?;
    let worker_name = "blob-store-service-1";
    let worker_id = executor.start_worker(&component.id, worker_name).await?;

    let result = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{container-exists}",
            vec![format!("{}-{worker_name}-container", component.id).into_value_and_type()],
        )
        .await?
        .unwrap();

    executor.check_oplog_is_queryable(&worker_id).await?;

    drop(executor);

    check!(result == vec![Value::Bool(false)]);
    Ok(())
}
