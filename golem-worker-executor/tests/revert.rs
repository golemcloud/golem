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

use crate::common::{start, TestContext};
use crate::{LastUniqueId, Tracing, WorkerExecutorTestDependencies};
use assert2::{check, let_assert};
use golem_common::model::public_oplog::PublicOplogEntry;
use golem_common::model::OplogIndex;
use golem_service_base::model::{RevertLastInvocations, RevertToOplogIndex, RevertWorkerTarget};
use golem_test_framework::config::TestDependencies;
use golem_test_framework::dsl::TestDslUnsafe;
use golem_wasm::analysis::{AnalysedResourceId, AnalysedResourceMode, AnalysedType, TypeHandle};
use golem_wasm::{IntoValue, IntoValueAndType, ValueAndType};
use log::info;
use test_r::{inherit_test_dep, test};

inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(LastUniqueId);
inherit_test_dep!(Tracing);

#[test]
#[tracing::instrument]
async fn revert_successful_invocations(
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

    let component_id = executor.component("counters").store().await;
    let worker_id = executor
        .start_worker(&component_id, "revert_successful_invocations")
        .await;
    executor.log_output(&worker_id).await;

    let counter1 = executor
        .invoke_and_await(
            &worker_id,
            "rpc:counters-exports/api.{[constructor]counter}",
            vec!["counter1".into_value_and_type()],
        )
        .await
        .unwrap();
    let counter2 = executor
        .invoke_and_await(
            &worker_id,
            "rpc:counters-exports/api.{[constructor]counter}",
            vec!["counter2".into_value_and_type()],
        )
        .await
        .unwrap();
    let counter_handle_type = AnalysedType::Handle(TypeHandle {
        name: None,
        owner: None,
        resource_id: AnalysedResourceId(0),
        mode: AnalysedResourceMode::Borrowed,
    });
    let counter_ref1 = ValueAndType::new(counter1[0].clone(), counter_handle_type.clone());
    let counter_ref2 = ValueAndType::new(counter2[0].clone(), counter_handle_type);

    let _ = executor
        .invoke_and_await(
            &worker_id,
            "rpc:counters-exports/api.{[method]counter.inc-by}",
            vec![counter_ref1.clone(), 5u64.into_value_and_type()],
        )
        .await
        .unwrap();

    let _ = executor
        .invoke_and_await(
            &worker_id,
            "rpc:counters-exports/api.{[method]counter.inc-by}",
            vec![counter_ref2.clone(), 1u64.into_value_and_type()],
        )
        .await
        .unwrap();
    let _ = executor
        .invoke_and_await(
            &worker_id,
            "rpc:counters-exports/api.{[method]counter.inc-by}",
            vec![counter_ref2.clone(), 2u64.into_value_and_type()],
        )
        .await
        .unwrap();

    let result1 = executor
        .invoke_and_await(
            &worker_id,
            "rpc:counters-exports/api.{[method]counter.get-value}",
            vec![counter_ref1.clone()],
        )
        .await
        .unwrap();
    let result2 = executor
        .invoke_and_await(
            &worker_id,
            "rpc:counters-exports/api.{[method]counter.get-value}",
            vec![counter_ref2.clone()],
        )
        .await
        .unwrap();

    executor
        .revert(
            &worker_id,
            RevertWorkerTarget::RevertLastInvocations(RevertLastInvocations {
                number_of_invocations: 3,
            }),
        )
        .await;

    let result3 = executor
        .invoke_and_await(
            &worker_id,
            "rpc:counters-exports/api.{[method]counter.get-value}",
            vec![counter_ref1.clone()],
        )
        .await
        .unwrap();
    let result4 = executor
        .invoke_and_await(
            &worker_id,
            "rpc:counters-exports/api.{[method]counter.get-value}",
            vec![counter_ref2.clone()],
        )
        .await
        .unwrap();

    executor.check_oplog_is_queryable(&worker_id).await;

    drop(executor);

    assert_eq!(result1, vec![5u64.into_value()]);
    assert_eq!(result2, vec![3u64.into_value()]);
    assert_eq!(result3, vec![5u64.into_value()]);
    assert_eq!(result4, vec![1u64.into_value()]);
}

#[test]
#[tracing::instrument]
async fn revert_failed_worker(
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

    let component_id = executor.component("failing-component").store().await;
    let worker_id = executor
        .start_worker(&component_id, "revert_failed_worker")
        .await;

    let result1 = executor
        .invoke_and_await(
            &worker_id,
            "golem:component/api.{add}",
            vec![5u64.into_value_and_type()],
        )
        .await;

    let result2 = executor
        .invoke_and_await(
            &worker_id,
            "golem:component/api.{add}",
            vec![50u64.into_value_and_type()],
        )
        .await;

    let result3 = executor
        .invoke_and_await(&worker_id, "golem:component/api.{get}", vec![])
        .await;

    executor
        .revert(
            &worker_id,
            RevertWorkerTarget::RevertLastInvocations(RevertLastInvocations {
                number_of_invocations: 1,
            }),
        )
        .await;

    let result4 = executor
        .invoke_and_await(&worker_id, "golem:component/api.{get}", vec![])
        .await;

    executor.check_oplog_is_queryable(&worker_id).await;

    drop(executor);

    check!(result1.is_ok());
    check!(result2.is_err());
    check!(result3.is_err());
    check!(result4.is_ok());
}

#[test]
#[tracing::instrument]
async fn revert_failed_worker_to_invoke_of_failed_inocation(
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

    let component_id = executor.component("failing-component").store().await;
    let worker_id = executor
        .start_worker(&component_id, "revert_failed_worker")
        .await;

    let result1 = executor
        .invoke_and_await(
            &worker_id,
            "golem:component/api.{add}",
            vec![5u64.into_value_and_type()],
        )
        .await;

    let result2 = executor
        .invoke_and_await(
            &worker_id,
            "golem:component/api.{add}",
            vec![50u64.into_value_and_type()],
        )
        .await;

    let revert_target = {
        let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await;
        tracing::warn!("oplog: {oplog:?}");
        oplog
            .iter()
            .rfind(|op| matches!(op.entry, PublicOplogEntry::ExportedFunctionInvoked(_)))
            .cloned()
            .unwrap()
    };

    executor
        .revert(
            &worker_id,
            RevertWorkerTarget::RevertToOplogIndex(RevertToOplogIndex {
                last_oplog_index: revert_target.oplog_index,
            }),
        )
        .await;

    let result3 = executor
        .invoke_and_await(&worker_id, "golem:component/api.{get}", vec![])
        .await;

    executor.check_oplog_is_queryable(&worker_id).await;

    check!(result1.is_ok());
    check!(result2.is_err());
    {
        let_assert!(Err(golem_api_grpc::proto::golem::worker::v1::worker_error::Error::InternalError(golem_api_grpc::proto::golem::worker::v1::WorkerExecutionError { error: Some(golem_api_grpc::proto::golem::worker::v1::worker_execution_error::Error::InvocationFailed(golem_api_grpc::proto::golem::worker::v1::InvocationFailed { stderr, .. })) })) = result3);
        assert2::assert!(stderr.contains("value is too large"));
        assert2::assert!(!stderr.contains("Oplog"));
    }
}

#[test]
#[tracing::instrument]
async fn revert_auto_update(
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

    let component_id = executor.component("update-test-v1").unique().store().await;
    let worker_id = executor
        .start_worker(&component_id, "revert_auto_update")
        .await;
    let _ = executor.log_output(&worker_id).await;

    let target_version = executor
        .update_component(&component_id, "update-test-v2-11")
        .await;
    info!("Updated component to version {target_version}");

    executor
        .auto_update_worker(&worker_id, target_version)
        .await;

    let result1 = executor
        .invoke_and_await(&worker_id, "golem:component/api.{f2}", vec![])
        .await
        .unwrap();

    executor
        .revert(
            &worker_id,
            RevertWorkerTarget::RevertToOplogIndex(RevertToOplogIndex {
                last_oplog_index: OplogIndex::INITIAL,
            }),
        )
        .await;

    let result2 = executor
        .invoke_and_await(&worker_id, "golem:component/api.{f2}", vec![])
        .await
        .unwrap();

    info!("result: {result1:?}");
    let (metadata, _) = executor.get_worker_metadata(&worker_id).await.unwrap();

    executor.check_oplog_is_queryable(&worker_id).await;

    // Expectation: the worker has no history so the update succeeds and then calling f2 returns
    // the current state which is 0. After the revert, calling f2 again returns a random number.
    // The traces of the update should be gone.
    check!(result1[0] == 0u64.into_value());
    check!(result2[0] != 0u64.into_value());
    check!(metadata.last_known_status.component_version == 0);
    check!(metadata.last_known_status.pending_updates.is_empty());
    check!(metadata.last_known_status.failed_updates.is_empty());
    check!(metadata.last_known_status.successful_updates.is_empty());
}
