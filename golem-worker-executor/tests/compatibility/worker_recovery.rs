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
use anyhow::anyhow;
use golem_common::model::{WorkerId, WorkerStatus};
use golem_common::serialization::{deserialize, serialize};
use golem_test_framework::dsl::TestDsl;
use golem_worker_executor_test_utils::{
    LastUniqueId, TestContext, TestWorkerExecutor, WorkerExecutorTestDependencies,
};
use redis::AsyncCommands;
use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;
use test_r::inherit_test_dep;
use tracing::info;

inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(LastUniqueId);
inherit_test_dep!(Tracing);

#[allow(dead_code)]
async fn restore_from_recovery_golden_file(
    executor: &TestWorkerExecutor,
    context: &TestContext,
    name: &str,
    component_names: &[&str],
) -> anyhow::Result<WorkerId> {
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
                .store_component_with_id(
                    component_name,
                    &worker_id.component_id,
                    &context.default_environment_id,
                )
                .await?;
        } else {
            executor
                .update_component(&worker_id.component_id, component_name)
                .await?;
        }
    }

    let mut redis = executor.redis().get_async_connection(0).await;

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

    Ok(worker_id)
}

/// Saves the oplog and worker ID if the environment variable UPDATE_GOLDENFILES is "1". This
/// should be called at the end of some test cases performing various operations on a worker.
/// If the UPDATE_GOLDENFILES environment variable is not set, it does nothing. The generated
/// files should be verified (if they can be recovered) in separate test cases.
#[allow(dead_code)]
pub async fn save_recovery_golden_file(
    executor: &TestWorkerExecutor,
    context: &TestContext,
    name: &str,
    worker_id: &WorkerId,
) -> anyhow::Result<()> {
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
            .await?;

        let oplog_path =
            Path::new("tests/goldenfiles").join(format!("worker_recovery_{name}.oplog.bin"));
        let worker_id_path =
            Path::new("tests/goldenfiles").join(format!("worker_recovery_{name}.worker_id.bin"));

        let encoded_oplog = serialize(&entries).map_err(|e| anyhow!(e))?;
        let encoded_worker_id = serialize(&worker_id).map_err(|e| anyhow!(e))?;

        tokio::fs::write(&oplog_path, encoded_oplog).await?;
        tokio::fs::write(&worker_id_path, encoded_worker_id).await?;
    }
    Ok(())
}

#[allow(dead_code)]
async fn wait_for_worker_recovery(
    executor: &TestWorkerExecutor,
    worker_id: &WorkerId,
) -> anyhow::Result<WorkerStatus> {
    loop {
        let metadata = executor.get_worker_metadata(worker_id).await?;

        if metadata.pending_invocation_count == 0
            && (metadata.status == WorkerStatus::Idle || metadata.status == WorkerStatus::Failed)
        {
            break Ok(metadata.status);
        }

        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}
