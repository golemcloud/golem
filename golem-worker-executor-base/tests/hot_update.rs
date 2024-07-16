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

use crate::common;
use assert2::check;
use golem_test_framework::dsl::TestDslUnsafe;
use golem_wasm_rpc::Value;
use log::info;
use std::time::Duration;
use tokio::spawn;

#[tokio::test]
#[tracing::instrument]
async fn auto_update_on_running() {
    for i in 1..10 {
        let context = common::TestContext::new();
        let executor = common::start(&context).await.unwrap();

        let component_id = executor.store_unique_component("update-test-v1").await;
        let worker_id = executor
            .start_worker(&component_id, "auto_update_on_running")
            .await;
        let _ = executor.log_output(&worker_id).await;

        let target_version = executor
            .update_component(&component_id, "update-test-v2")
            .await;
        info!("Updated component to version {target_version}");

        let executor_clone = executor.clone();
        let worker_id_clone = worker_id.clone();
        let fiber = spawn(async move {
            executor_clone
                .invoke_and_await(
                    &worker_id_clone,
                    "golem:component/api.{f1}",
                    vec![Value::U64(1000)],
                )
                .await
                .unwrap()
        });

        tokio::time::sleep(Duration::from_secs(10)).await;
        executor
            .auto_update_worker(&worker_id, target_version)
            .await;

        tokio::time::sleep(Duration::from_secs(2)).await;
        let _ = executor.log_output(&worker_id).await;

        let result = fiber.await.unwrap();
        info!("result: {:?}", result);
        let metadata = executor.get_worker_metadata(&worker_id).await.unwrap();

        // Expectation: f1 is interrupted in the middle to update the worker, so it get restarted
        // and eventually finishes with 150. The update is marked as a success.
        check!(result[0] == Value::U64(150));
        check!(metadata.last_known_status.component_version == target_version);
        check!(metadata.last_known_status.pending_updates.is_empty());
        check!(metadata.last_known_status.successful_updates.len() == 1);
        check!(metadata.last_known_status.failed_updates.is_empty());
    }
}

// #[tokio::test]
// #[tracing::instrument]
// async fn auto_update_on_idle() {
//     let context = common::TestContext::new();
//     let executor = common::start(&context).await.unwrap();
//
//     let component_id = executor.store_unique_component("update-test-v1").await;
//     let worker_id = executor
//         .start_worker(&component_id, "auto_update_on_idle")
//         .await;
//     let _ = executor.log_output(&worker_id).await;
//
//     let target_version = executor
//         .update_component(&component_id, "update-test-v2")
//         .await;
//     info!("Updated component to version {target_version}");
//
//     executor
//         .auto_update_worker(&worker_id, target_version)
//         .await;
//
//     let result = executor
//         .invoke_and_await(&worker_id, "golem:component/api.{f2}", vec![])
//         .await
//         .unwrap();
//
//     info!("result: {:?}", result);
//     let metadata = executor.get_worker_metadata(&worker_id).await.unwrap();
//
//     // Expectation: the worker has no history so the update succeeds and then calling f2 returns
//     // the current state which is 0
//     check!(result[0] == Value::U64(0));
//     check!(metadata.last_known_status.component_version == target_version);
//     check!(metadata.last_known_status.pending_updates.is_empty());
//     check!(metadata.last_known_status.failed_updates.is_empty());
//     check!(metadata.last_known_status.successful_updates.len() == 1);
// }
//
// #[tokio::test]
// #[tracing::instrument]
// async fn failing_auto_update_on_idle() {
//     let context = common::TestContext::new();
//     let executor = common::start(&context).await.unwrap();
//
//     let component_id = executor.store_unique_component("update-test-v1").await;
//     let worker_id = executor
//         .start_worker(&component_id, "failing_auto_update_on_idle")
//         .await;
//     let _ = executor.log_output(&worker_id).await;
//
//     let target_version = executor
//         .update_component(&component_id, "update-test-v2")
//         .await;
//     info!("Updated component to version {target_version}");
//
//     let _ = executor
//         .invoke_and_await(&worker_id, "golem:component/api.{f1}", vec![Value::U64(0)])
//         .await
//         .unwrap();
//
//     executor
//         .auto_update_worker(&worker_id, target_version)
//         .await;
//
//     let result = executor
//         .invoke_and_await(&worker_id, "golem:component/api.{f2}", vec![])
//         .await
//         .unwrap();
//
//     info!("result: {:?}", result);
//     let metadata = executor.get_worker_metadata(&worker_id).await.unwrap();
//
//     // Expectation: we finish executing f1 which returns with 300. Then we try updating, but the
//     // updated f1 would return 150 which we detect as a divergence and fail the update. After this
//     // f2's original version is executed which returns random u64.
//     check!(result[0] != Value::U64(150));
//     check!(result[0] != Value::U64(300));
//     check!(metadata.last_known_status.component_version == 0);
//     check!(metadata.last_known_status.pending_updates.is_empty());
//     check!(metadata.last_known_status.failed_updates.len() == 1);
//     check!(metadata.last_known_status.successful_updates.is_empty());
// }
//
// #[tokio::test]
// #[tracing::instrument]
// async fn auto_update_on_idle_with_non_diverging_history() {
//     let context = common::TestContext::new();
//     let executor = common::start(&context).await.unwrap();
//
//     let component_id = executor.store_unique_component("update-test-v1").await;
//     let worker_id = executor
//         .start_worker(
//             &component_id,
//             "auto_update_on_idle_with_non_diverging_history",
//         )
//         .await;
//     let _ = executor.log_output(&worker_id).await;
//
//     let target_version = executor
//         .update_component(&component_id, "update-test-v2")
//         .await;
//     info!("Updated component to version {target_version}");
//
//     let _ = executor
//         .invoke_and_await(&worker_id, "golem:component/api.{f3}", vec![])
//         .await
//         .unwrap();
//     let _ = executor
//         .invoke_and_await(&worker_id, "golem:component/api.{f3}", vec![])
//         .await
//         .unwrap();
//
//     executor
//         .auto_update_worker(&worker_id, target_version)
//         .await;
//
//     let result = executor
//         .invoke_and_await(&worker_id, "golem:component/api.{f4}", vec![])
//         .await
//         .unwrap();
//
//     info!("result: {:?}", result);
//     let metadata = executor.get_worker_metadata(&worker_id).await.unwrap();
//
//     // Expectation: the f3 function is not changing between the versions, so we can safely
//     // update the component and call f4 which only exists in the new version.
//     // the current state which is 0
//     check!(result[0] == Value::U64(11));
//     check!(metadata.last_known_status.component_version == target_version);
//     check!(metadata.last_known_status.pending_updates.is_empty());
//     check!(metadata.last_known_status.failed_updates.is_empty());
//     check!(metadata.last_known_status.successful_updates.len() == 1);
// }
//
// #[tokio::test]
// #[tracing::instrument]
// async fn failing_auto_update_on_running() {
//     let context = common::TestContext::new();
//     let executor = common::start(&context).await.unwrap();
//
//     let component_id = executor.store_unique_component("update-test-v1").await;
//     let worker_id = executor
//         .start_worker(&component_id, "failing_auto_update_on_running")
//         .await;
//     let _ = executor.log_output(&worker_id).await;
//
//     let target_version = executor
//         .update_component(&component_id, "update-test-v2")
//         .await;
//     info!("Updated component to version {target_version}");
//
//     let _ = executor
//         .invoke_and_await(&worker_id, "golem:component/api.{f2}", vec![])
//         .await
//         .unwrap();
//
//     let executor_clone = executor.clone();
//     let worker_id_clone = worker_id.clone();
//     let fiber = spawn(async move {
//         executor_clone
//             .invoke_and_await(
//                 &worker_id_clone,
//                 "golem:component/api.{f1}",
//                 vec![Value::U64(1000)],
//             )
//             .await
//             .unwrap()
//     });
//
//     tokio::time::sleep(Duration::from_secs(10)).await;
//     executor
//         .auto_update_worker(&worker_id, target_version)
//         .await;
//
//     tokio::time::sleep(Duration::from_secs(2)).await;
//     let _ = executor.log_output(&worker_id).await;
//
//     let result = fiber.await.unwrap();
//     info!("result: {:?}", result);
//     let metadata = executor.get_worker_metadata(&worker_id).await.unwrap();
//
//     // Expectation: f1 is interrupted in the middle to update the worker, so it get restarted
//     // and tries to get updated, but it fails because f2 was previously executed and it is
//     // diverging from the new version. The update is marked as a failure and the invocation continues
//     // with the original version, resulting in 300.
//     check!(result[0] == Value::U64(300));
//     check!(metadata.last_known_status.component_version == 0);
//     check!(metadata.last_known_status.pending_updates.is_empty());
//     check!(metadata.last_known_status.successful_updates.is_empty());
//     check!(metadata.last_known_status.failed_updates.len() == 1);
// }
//
// #[tokio::test]
// #[tracing::instrument]
// async fn manual_update_on_idle() {
//     let context = common::TestContext::new();
//     let executor = common::start(&context).await.unwrap();
//
//     let component_id = executor.store_unique_component("update-test-v2").await;
//     let worker_id = executor
//         .start_worker(&component_id, "manual_update_on_idle")
//         .await;
//     let _ = executor.log_output(&worker_id).await;
//
//     let target_version = executor
//         .update_component(&component_id, "update-test-v3")
//         .await;
//     info!("Updated component to version {target_version}");
//
//     let _ = executor
//         .invoke_and_await(&worker_id, "golem:component/api.{f1}", vec![Value::U64(0)])
//         .await
//         .unwrap();
//
//     let before_update = executor
//         .invoke_and_await(&worker_id, "golem:component/api.{f2}", vec![])
//         .await
//         .unwrap();
//
//     executor
//         .manual_update_worker(&worker_id, target_version)
//         .await;
//
//     let after_update = executor
//         .invoke_and_await(&worker_id, "golem:component/api.{get}", vec![])
//         .await
//         .unwrap();
//
//     let metadata = executor.get_worker_metadata(&worker_id).await.unwrap();
//
//     // Explanation: we can call 'get' on the updated component that does not exist in previous
//     // versions, and it returns the previous global state which has been transferred to it
//     // using the v2 component's 'save' function through the v3 component's load function.
//
//     check!(before_update == after_update);
//     check!(metadata.last_known_status.component_version == target_version);
//     check!(metadata.last_known_status.pending_updates.is_empty());
//     check!(metadata.last_known_status.failed_updates.is_empty());
//     check!(metadata.last_known_status.successful_updates.len() == 1);
// }
//
// #[tokio::test]
// #[tracing::instrument]
// async fn manual_update_on_idle_without_save_snapshot() {
//     let context = common::TestContext::new();
//     let executor = common::start(&context).await.unwrap();
//
//     let component_id = executor.store_unique_component("update-test-v1").await;
//     let worker_id = executor
//         .start_worker(&component_id, "manual_update_on_idle_without_save_snapshot")
//         .await;
//     let _ = executor.log_output(&worker_id).await;
//
//     let target_version = executor
//         .update_component(&component_id, "update-test-v3")
//         .await;
//     info!("Updated component to version {target_version}");
//
//     let _ = executor
//         .invoke_and_await(&worker_id, "golem:component/api.{f1}", vec![Value::U64(0)])
//         .await
//         .unwrap();
//
//     executor
//         .manual_update_worker(&worker_id, target_version)
//         .await;
//
//     let result = executor
//         .invoke_and_await(&worker_id, "golem:component/api.{f3}", vec![])
//         .await
//         .unwrap();
//
//     let metadata = executor.get_worker_metadata(&worker_id).await.unwrap();
//
//     // Explanation: We are trying to update v1 to v3 using snapshots, but v1 does not
//     // export a save function, so the update attempt fails and the worker continues running
//     // the original version which we can invoke.
//
//     check!(result == vec![Value::U64(3)]);
//     check!(metadata.last_known_status.component_version == 0);
//     check!(metadata.last_known_status.pending_updates.is_empty());
//     check!(metadata.last_known_status.failed_updates.len() == 1);
//     check!(metadata.last_known_status.successful_updates.is_empty());
// }

#[tokio::test]
#[tracing::instrument]
async fn auto_update_on_running_followed_by_manual() {
    for _ in 1..10 {
        let context = common::TestContext::new();
        let executor = common::start(&context).await.unwrap();

        let component_id = executor.store_unique_component("update-test-v1").await;
        let worker_id = executor
            .start_worker(&component_id, "auto_update_on_running_followed_by_manual")
            .await;
        let _ = executor.log_output(&worker_id).await;

        let target_version1 = executor
            .update_component(&component_id, "update-test-v2")
            .await;
        info!("Updated component to version {target_version1}");

        let target_version2 = executor
            .update_component(&component_id, "update-test-v3")
            .await;
        info!("Updated component to version {target_version2}");

        let executor_clone = executor.clone();
        let worker_id_clone = worker_id.clone();
        let fiber = spawn(async move {
            executor_clone
                .invoke_and_await(
                    &worker_id_clone,
                    "golem:component/api.{f1}",
                    vec![Value::U64(1000)],
                )
                .await
                .unwrap()
        });

        tokio::time::sleep(Duration::from_secs(10)).await;
        executor
            .auto_update_worker(&worker_id, target_version1)
            .await;
        executor
            .manual_update_worker(&worker_id, target_version2)
            .await;

        tokio::time::sleep(Duration::from_secs(2)).await;
        let _ = executor.log_output(&worker_id).await;

        let result1 = fiber.await.unwrap();
        info!("result1: {:?}", result1);

        let result2 = executor
            .invoke_and_await(&worker_id, "golem:component/api.{get}", vec![])
            .await
            .unwrap();
        info!("result2: {:?}", result2);

        let metadata = executor.get_worker_metadata(&worker_id).await.unwrap();

        // Expectation: f1 is interrupted in the middle to update the worker, so it get restarted
        // and eventually finishes with 150. The update is marked as a success, but immediately
        // it gets updated again to v3 on which we can call the previously non existent 'get'
        // function to get the same state that was generated by 'v2'.
        check!(result1[0] == Value::U64(150));
        check!(result2[0] == Value::U64(150));
        check!(metadata.last_known_status.component_version == target_version2);
        check!(metadata.last_known_status.pending_updates.is_empty());
        check!(metadata.last_known_status.successful_updates.len() == 2);
        check!(metadata.last_known_status.failed_updates.is_empty());
    }
}

#[tokio::test]
#[tracing::instrument]
async fn manual_update_on_idle_with_failing_load() {
    for _ in 1..50 {
        let context = common::TestContext::new();
        let executor = common::start(&context).await.unwrap();

        let component_id = executor.store_unique_component("update-test-v2").await;
        let worker_id = executor
            .start_worker(&component_id, "manual_update_on_idle_with_failing_load")
            .await;
        let _ = executor.log_output(&worker_id).await;

        let target_version = executor
            .update_component(&component_id, "update-test-v4")
            .await;
        info!("Updated component to version {target_version}");

        let _ = executor
            .invoke_and_await(&worker_id, "golem:component/api.{f1}", vec![Value::U64(0)])
            .await
            .unwrap();

        executor
            .manual_update_worker(&worker_id, target_version)
            .await;

        let result = executor
            .invoke_and_await(&worker_id, "golem:component/api.{f3}", vec![])
            .await
            .unwrap();

        let metadata = executor.get_worker_metadata(&worker_id).await.unwrap();

        // Explanation: We try to update v2 to v4, but v4's load function always fails. So
        // the component must stay on v2, on which we can invoke f3.

        check!(result == vec![Value::U64(3)]);
        check!(metadata.last_known_status.component_version == 0);
        check!(metadata.last_known_status.pending_updates.is_empty());
        check!(metadata.last_known_status.failed_updates.len() == 1);
        check!(metadata.last_known_status.successful_updates.is_empty());
    }
}
