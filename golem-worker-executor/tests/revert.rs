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
use golem_common::model::component::ComponentRevision;
use golem_common::model::oplog::PublicOplogEntry;
use golem_common::model::worker::{RevertLastInvocations, RevertToOplogIndex, RevertWorkerTarget};
use golem_common::model::{OplogIndex, WorkerStatus};
use golem_common::{agent_id, data_value};
use golem_test_framework::dsl::{update_counts, TestDsl};
use golem_wasm::Value;
use golem_worker_executor_test_utils::{
    start, LastUniqueId, TestContext, WorkerExecutorTestDependencies,
};
use log::info;
use pretty_assertions::{assert_eq, assert_ne};
use test_r::{inherit_test_dep, test, timeout};

inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(LastUniqueId);
inherit_test_dep!(Tracing);

#[test]
#[tracing::instrument]
#[timeout("4m")]
async fn revert_successful_invocations(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component(
            &context.default_environment_id,
            "golem_it_agent_rpc_rust_release",
        )
        .name("golem-it:agent-rpc-rust")
        .store()
        .await?;

    let agent_id1 = agent_id!("rpc-counter", "counter1");
    let worker_id1 = executor
        .start_agent(&component.id, agent_id1.clone())
        .await?;
    executor.log_output(&worker_id1).await?;

    let agent_id2 = agent_id!("rpc-counter", "counter2");
    let worker_id2 = executor
        .start_agent(&component.id, agent_id2.clone())
        .await?;

    // counter1.inc_by(5)
    executor
        .invoke_and_await_agent(&component, &agent_id1, "inc_by", data_value!(5u64))
        .await?;

    // counter2.inc_by(1)
    executor
        .invoke_and_await_agent(&component, &agent_id2, "inc_by", data_value!(1u64))
        .await?;

    // counter2.inc_by(2)
    executor
        .invoke_and_await_agent(&component, &agent_id2, "inc_by", data_value!(2u64))
        .await?;

    // counter2.get_value() -> 3
    let result2 = executor
        .invoke_and_await_agent(&component, &agent_id2, "get_value", data_value!())
        .await?;

    // Revert last 2 invocations on counter2 (undoes inc_by(2) and get_value)
    executor
        .revert(
            &worker_id2,
            RevertWorkerTarget::RevertLastInvocations(RevertLastInvocations {
                number_of_invocations: 2,
            }),
        )
        .await?;

    // counter1.get_value() -> 5 (unchanged)
    let result1 = executor
        .invoke_and_await_agent(&component, &agent_id1, "get_value", data_value!())
        .await?;

    // counter2.get_value() -> 1 (inc_by(2) was reverted)
    let result2_after = executor
        .invoke_and_await_agent(&component, &agent_id2, "get_value", data_value!())
        .await?;

    executor.check_oplog_is_queryable(&worker_id1).await?;
    executor.check_oplog_is_queryable(&worker_id2).await?;

    let result1_value = result1
        .into_return_value()
        .expect("Expected a return value");
    let result2_value = result2
        .into_return_value()
        .expect("Expected a return value");
    let result2_after_value = result2_after
        .into_return_value()
        .expect("Expected a return value");

    assert_eq!(result1_value, Value::U64(5));
    assert_eq!(result2_value, Value::U64(3));
    assert_eq!(result2_after_value, Value::U64(1));

    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("4m")]
async fn revert_failed_worker(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use golem_common::data_value;

    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component(&context.default_environment_id, "it_agent_counters_release")
        .name("it:agent-counters")
        .store()
        .await?;
    let agent_id = agent_id!("failing-counter", "revert_failed_worker");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    executor
        .invoke_and_await_agent(&component, &agent_id, "add", data_value!(5u64))
        .await?;

    let result2 = executor
        .invoke_and_await_agent(&component, &agent_id, "add", data_value!(50u64))
        .await;

    let result3 = executor
        .invoke_and_await_agent(&component, &agent_id, "get", data_value!())
        .await;

    executor
        .revert(
            &worker_id,
            RevertWorkerTarget::RevertLastInvocations(RevertLastInvocations {
                number_of_invocations: 1,
            }),
        )
        .await?;

    let result4 = executor
        .invoke_and_await_agent(&component, &agent_id, "get", data_value!())
        .await;

    executor.check_oplog_is_queryable(&worker_id).await?;

    assert!(result2.is_err());
    assert!(result3.is_err());
    assert!(
        result4.is_ok(),
        "Expected get after revert to succeed: {:?}",
        result4
    );

    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("4m")]
async fn revert_failed_worker_to_invoke_of_failed_invocation(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use golem_common::data_value;

    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component(&context.default_environment_id, "it_agent_counters_release")
        .name("it:agent-counters")
        .store()
        .await?;
    let agent_id = agent_id!("failing-counter", "revert_failed_worker");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    executor
        .invoke_and_await_agent(&component, &agent_id, "add", data_value!(5u64))
        .await?;

    let result2 = executor
        .invoke_and_await_agent(&component, &agent_id, "add", data_value!(50u64))
        .await;

    let revert_target = {
        let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
        oplog
            .iter()
            .rfind(|op| matches!(op.entry, PublicOplogEntry::AgentInvocationStarted(_)))
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
        .await?;

    let result3 = executor
        .invoke_and_await_agent(&component, &agent_id, "get", data_value!())
        .await;

    executor.check_oplog_is_queryable(&worker_id).await?;

    assert!(result2.is_err());
    {
        let err3 = format!(
            "{}",
            result3.expect_err("Expected get after revert to fail")
        );
        assert!(
            err3.contains("value is too large"),
            "Expected 'value is too large' in error: {err3}"
        );
        assert!(
            !err3.contains("Oplog"),
            "Unexpected 'Oplog' in error: {err3}"
        );
    }

    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("4m")]
async fn revert_auto_update(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use golem_common::data_value;

    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component(
            &context.default_environment_id,
            "it_agent_update_v1_release",
        )
        .name("it:agent-update")
        .unique()
        .store()
        .await?;
    let agent_id = agent_id!("update-test");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;
    executor.log_output(&worker_id).await?;

    // Wait for the worker to finish initialization before reading the oplog
    executor
        .wait_for_status(
            &worker_id,
            WorkerStatus::Idle,
            std::time::Duration::from_secs(10),
        )
        .await?;

    let updated_component = executor
        .update_component(&component.id, "it_agent_update_v2_release")
        .await?;

    info!(
        "Updated component to version {}",
        updated_component.revision
    );

    // Get the oplog index right after initialization (before the update)
    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    let after_init_index = oplog.last().unwrap().oplog_index;

    executor
        .auto_update_worker(&worker_id, updated_component.revision, false)
        .await?;

    let result1 = executor
        .invoke_and_await_agent(&component, &agent_id, "f2", data_value!())
        .await?;

    // Revert to just after initialization (not INITIAL), because agents cannot
    // be invoked if their initialization is reverted.
    executor
        .revert(
            &worker_id,
            RevertWorkerTarget::RevertToOplogIndex(RevertToOplogIndex {
                last_oplog_index: after_init_index,
            }),
        )
        .await?;

    let result2 = executor
        .invoke_and_await_agent(&component, &agent_id, "f2", data_value!())
        .await?;

    info!("result: {result1:?}");
    let metadata = executor.get_worker_metadata(&worker_id).await?;

    executor.check_oplog_is_queryable(&worker_id).await?;

    // Expectation: the worker has no history so the update succeeds and then calling f2 returns
    // the current state which is 0. After the revert to just after init, calling f2 again
    // returns a random number. The traces of the update should be gone.
    assert_eq!(result1, data_value!(0u64));
    assert_ne!(result2, data_value!(0u64));
    assert_eq!(metadata.component_revision, ComponentRevision::INITIAL);
    assert_eq!(update_counts(&metadata), (0, 0, 0));

    Ok(())
}
