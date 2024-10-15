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

use test_r::{inherit_test_dep, test};

use crate::common::{start, TestContext, TestWorkerExecutor};
use crate::{LastUniqueId, Tracing, WorkerExecutorTestDependencies};
use futures_util::FutureExt;
use golem_common::model::{WorkerId, WorkerStatus};
use golem_common::serialization::{deserialize, serialize};
use golem_test_framework::config::TestDependencies;
use golem_test_framework::dsl::TestDslUnsafe;
use redis::AsyncCommands;
use std::collections::BTreeMap;
use std::path::Path;
use tracing::info;

inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(LastUniqueId);
inherit_test_dep!(Tracing);

#[test]
#[tracing::instrument]
async fn recover_shopping_cart_example(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await.unwrap();

    let worker_id =
        restore_from_recovery_golden_file(&executor, "shopping_cart_example", "shopping-cart")
            .await;

    executor.interrupt(&worker_id).await;
    executor.resume(&worker_id).await;

    let (metadata, _) = executor
        .get_worker_metadata(&worker_id)
        .await
        .expect("Failed to get metadata");

    assert_eq!(metadata.last_known_status.status, WorkerStatus::Idle);
}

async fn restore_from_recovery_golden_file(
    executor: &TestWorkerExecutor,
    name: &str,
    component_name: &str,
) -> WorkerId {
    let worker_id_path =
        Path::new("tests/goldenfiles").join(format!("worker_recovery_{name}.worker_id.bin"));
    let oplog_path =
        Path::new("tests/goldenfiles").join(format!("worker_recovery_{name}.oplog.bin"));

    let worker_id = tokio::fs::read(&worker_id_path).await.unwrap();
    let worker_id: WorkerId = deserialize(&worker_id).unwrap();

    let entries = tokio::fs::read(&oplog_path).await.unwrap();
    let entries: Vec<BTreeMap<String, BTreeMap<String, Vec<u8>>>> = deserialize(&entries).unwrap();

    executor
        .store_component_with_id(component_name, &worker_id.component_id)
        .await;

    let mut redis = executor.redis().get_async_connection(0).await;

    let oplog_key = &format!(
        "{}worker:oplog:{}",
        executor.redis().prefix(),
        worker_id.to_redis_key()
    );

    info!("Using redis key {oplog_key}");

    let before: Vec<BTreeMap<String, BTreeMap<String, Vec<u8>>>> =
        redis.xrange_all(&oplog_key).await.unwrap();

    assert_eq!(before, vec![]);

    for entry in entries.clone() {
        for (key, value) in entry {
            let value = value.into_iter().collect::<Vec<_>>();
            let _: () = redis.xadd(&oplog_key, key, &value).await.unwrap();
        }
    }

    let entries2: Vec<BTreeMap<String, BTreeMap<String, Vec<u8>>>> =
        redis.xrange_all(&oplog_key).await.unwrap();

    assert_eq!(entries, entries2);

    worker_id
}

/// Saves the oplog and worker ID if the environment variable UPDATE_GOLDENFILES is "1". This
/// should be called at the end of some test cases performing various operations on a worker.
/// If the UPDATE_GOLDENFILES environment variable is not set, it does nothing. The generated
/// files should be verified (if they can be recovered) in separate test cases.
pub async fn save_recovery_golden_file(
    executor: &TestWorkerExecutor,
    name: &str,
    worker_id: &WorkerId,
) {
    if std::env::var("UPDATE_GOLDENFILES") == Ok("1".to_string()) {
        info!(
            "Saving golden file for worker recovery test case {name}, using worker id {worker_id}"
        );
        let mut redis = executor.redis().get_async_connection(0).await;
        let entries: Vec<BTreeMap<String, BTreeMap<String, Vec<u8>>>> = redis
            .xrange_all(&format!(
                "{}worker:oplog:{}",
                executor.redis().prefix(),
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
