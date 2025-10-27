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

use crate::common::{start, TestContext, TestWorkerExecutor};
use crate::{LastUniqueId, Tracing, WorkerExecutorTestDependencies};
use golem_common::model::{WorkerId, WorkerStatus};
use golem_common::serialization::{deserialize, serialize};
use golem_test_framework::config::{TestDependencies, TestDependenciesDsl};
use golem_test_framework::dsl::TestDslUnsafe;
use redis::AsyncCommands;
use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;
use test_r::{inherit_test_dep, test};
use tracing::info;

inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(LastUniqueId);
inherit_test_dep!(Tracing);

#[test]
#[tracing::instrument]
#[ignore] // TODO: 1.3 breaks worker recovery compatibility. to be regenerated once 1.3 is final
async fn recover_shopping_cart_example(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context)
        .await
        .unwrap()
        .into_admin_with_unique_project()
        .await;

    let worker_id = restore_from_recovery_golden_file(
        &executor,
        &context,
        "shopping_cart_example",
        &["shopping-cart"],
    )
    .await;

    executor.interrupt(&worker_id).await;
    executor.resume(&worker_id, false).await;

    let status = wait_for_worker_recovery(&executor, &worker_id).await;
    assert_eq!(status, WorkerStatus::Idle);
}

#[test]
#[tracing::instrument]
#[ignore] // TODO: 1.3 breaks worker recovery compatibility. to be regenerated once 1.3 is final
async fn recover_shopping_cart_resource_example(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context)
        .await
        .unwrap()
        .into_admin_with_unique_project()
        .await;

    let worker_id = restore_from_recovery_golden_file(
        &executor,
        &context,
        "shopping_cart_resource_example",
        &["shopping-cart-resource"],
    )
    .await;

    executor.interrupt(&worker_id).await;
    executor.resume(&worker_id, false).await;

    let status = wait_for_worker_recovery(&executor, &worker_id).await;
    assert_eq!(status, WorkerStatus::Idle);
}

#[test]
#[tracing::instrument]
#[ignore] // TODO: 1.3 breaks worker recovery compatibility. to be regenerated once 1.3 is final
async fn recover_environment_example(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context)
        .await
        .unwrap()
        .into_admin_with_unique_project()
        .await;

    let worker_id = restore_from_recovery_golden_file(
        &executor,
        &context,
        "environment_example",
        &["environment-service"],
    )
    .await;

    executor.interrupt(&worker_id).await;
    executor.resume(&worker_id, false).await;

    let status = wait_for_worker_recovery(&executor, &worker_id).await;
    assert_eq!(status, WorkerStatus::Idle);
}

#[test]
#[tracing::instrument]
#[ignore] // TODO: 1.3 breaks worker recovery compatibility. to be regenerated once 1.3 is final
async fn recover_read_stdin(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context)
        .await
        .unwrap()
        .into_admin_with_unique_project()
        .await;

    let worker_id =
        restore_from_recovery_golden_file(&executor, &context, "read_stdin_fails", &["read-stdin"])
            .await;

    executor.interrupt(&worker_id).await;
    let _ = golem_test_framework::dsl::TestDsl::resume(&executor, &worker_id, false).await; // this fails but we don't mind

    let status = wait_for_worker_recovery(&executor, &worker_id).await;
    assert_eq!(status, WorkerStatus::Failed);
}

#[test]
#[tracing::instrument]
#[ignore] // TODO: 1.3 breaks worker recovery compatibility. to be regenerated once 1.3 is final
async fn recover_jump(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context)
        .await
        .unwrap()
        .into_admin_with_unique_project()
        .await;

    let worker_id =
        restore_from_recovery_golden_file(&executor, &context, "jump", &["runtime-service"]).await;

    executor.interrupt(&worker_id).await;
    executor.resume(&worker_id, false).await;

    let status = wait_for_worker_recovery(&executor, &worker_id).await;
    assert_eq!(status, WorkerStatus::Idle);
}

#[test]
#[tracing::instrument]
#[ignore] // TODO: 1.3 breaks worker recovery compatibility. to be regenerated once 1.3 is final
async fn recover_js_example_1(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context)
        .await
        .unwrap()
        .into_admin_with_unique_project()
        .await;

    let worker_id =
        restore_from_recovery_golden_file(&executor, &context, "js_example_1", &["js-1"]).await;

    executor.interrupt(&worker_id).await;
    executor.resume(&worker_id, false).await;

    let status = wait_for_worker_recovery(&executor, &worker_id).await;
    assert_eq!(status, WorkerStatus::Idle);
}

#[test]
#[tracing::instrument]
#[ignore] // TODO: 1.3 breaks worker recovery compatibility. to be regenerated once 1.3 is final
async fn recover_auto_update_on_running(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context)
        .await
        .unwrap()
        .into_admin_with_unique_project()
        .await;

    let worker_id = restore_from_recovery_golden_file(
        &executor,
        &context,
        "auto_update_on_running",
        &["update-test-v1", "update-test-v2"],
    )
    .await;

    executor.interrupt(&worker_id).await;
    executor.resume(&worker_id, false).await;

    let status = wait_for_worker_recovery(&executor, &worker_id).await;
    assert_eq!(status, WorkerStatus::Idle);
}

#[test]
#[tracing::instrument]
#[ignore] // TODO: 1.3 breaks worker recovery compatibility. to be regenerated once 1.3 is final
async fn recover_counter_resource_test_2(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context)
        .await
        .unwrap()
        .into_admin_with_unique_project()
        .await;

    let caller_worker_id = restore_from_recovery_golden_file(
        &executor,
        &context,
        "counter_resource_test_2_caller",
        &["caller_composed"],
    )
    .await;
    let counter_worker_id = restore_from_recovery_golden_file(
        &executor,
        &context,
        "counter_resource_test_2_counter",
        &["counters"],
    )
    .await;

    executor.interrupt(&caller_worker_id).await;
    executor.interrupt(&counter_worker_id).await;
    executor.resume(&caller_worker_id, false).await;

    let status = wait_for_worker_recovery(&executor, &caller_worker_id).await;
    assert_eq!(status, WorkerStatus::Idle);
}

async fn restore_from_recovery_golden_file(
    executor: &TestDependenciesDsl<TestWorkerExecutor>,
    context: &TestContext,
    name: &str,
    component_names: &[&str],
) -> WorkerId {
    let worker_id_path =
        Path::new("tests/goldenfiles").join(format!("worker_recovery_{name}.worker_id.bin"));
    let oplog_path =
        Path::new("tests/goldenfiles").join(format!("worker_recovery_{name}.oplog.bin"));

    let worker_id = tokio::fs::read(&worker_id_path).await.unwrap();
    let worker_id: WorkerId = deserialize(&worker_id).unwrap();

    let entries = tokio::fs::read(&oplog_path).await.unwrap();
    let entries: Vec<BTreeMap<String, BTreeMap<String, Vec<u8>>>> = deserialize(&entries).unwrap();

    for (idx, component_name) in component_names.iter().enumerate() {
        if idx == 0 {
            executor
                .store_component_with_id(component_name, &worker_id.component_id)
                .await;
        } else {
            executor
                .update_component(&worker_id.component_id, component_name)
                .await;
        }
    }

    let mut redis = executor.deps.redis().get_async_connection(0).await;

    let oplog_key = &format!(
        "{}worker:oplog:{}",
        context.redis_prefix(),
        worker_id.to_redis_key()
    );

    info!("Using redis key {oplog_key}");

    let before: Vec<BTreeMap<String, BTreeMap<String, Vec<u8>>>> =
        redis.xrange_all(oplog_key).await.unwrap();

    assert_eq!(before, vec![]);

    for entry in entries.clone() {
        for (key, value) in entry {
            let value = value.into_iter().collect::<Vec<_>>();
            let _: () = redis.xadd(oplog_key, key, &value).await.unwrap();
        }
    }

    let entries2: Vec<BTreeMap<String, BTreeMap<String, Vec<u8>>>> =
        redis.xrange_all(oplog_key).await.unwrap();

    assert_eq!(entries, entries2);

    worker_id
}

/// Saves the oplog and worker ID if the environment variable UPDATE_GOLDENFILES is "1". This
/// should be called at the end of some test cases performing various operations on a worker.
/// If the UPDATE_GOLDENFILES environment variable is not set, it does nothing. The generated
/// files should be verified (if they can be recovered) in separate test cases.
pub async fn save_recovery_golden_file(
    executor: &TestWorkerExecutor,
    context: &TestContext,
    name: &str,
    worker_id: &WorkerId,
) {
    if std::env::var("UPDATE_GOLDENFILES") == Ok("1".to_string()) {
        info!(
            "Saving golden file for worker recovery test case {name}, using worker id {worker_id}"
        );
        let mut redis = executor.redis().get_async_connection(0).await;
        let entries: Vec<BTreeMap<String, BTreeMap<String, Vec<u8>>>> = redis
            .xrange_all(format!(
                "{}worker:oplog:{}",
                context.redis_prefix(),
                worker_id.to_redis_key()
            ))
            .await
            .unwrap();

        let oplog_path =
            Path::new("tests/goldenfiles").join(format!("worker_recovery_{name}.oplog.bin"));
        let worker_id_path =
            Path::new("tests/goldenfiles").join(format!("worker_recovery_{name}.worker_id.bin"));

        let encoded_oplog = serialize(&entries).unwrap();
        let encoded_worker_id = serialize(&worker_id).unwrap();

        tokio::fs::write(&oplog_path, encoded_oplog).await.unwrap();
        tokio::fs::write(&worker_id_path, encoded_worker_id)
            .await
            .unwrap();
    }
}

async fn wait_for_worker_recovery(
    executor: &TestDependenciesDsl<TestWorkerExecutor>,
    worker_id: &WorkerId,
) -> WorkerStatus {
    loop {
        let (metadata, _) = executor
            .get_worker_metadata(worker_id)
            .await
            .expect("Failed to get metadata");

        if metadata.last_known_status.pending_invocations.is_empty()
            && (metadata.last_known_status.status == WorkerStatus::Idle
                || metadata.last_known_status.status == WorkerStatus::Failed)
        {
            break metadata.last_known_status.status;
        }

        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}
