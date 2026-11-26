// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
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

//! E20: a failure of a never-live fork that is not a refusal keeps its class. `SnapshotUpdateTest`
//! of agent-updates-v1 runs `suffix_host_error` harmlessly; agent-updates-v2 fails in it by a
//! host error.

use super::wait_for_update_outcomes;
use crate::Tracing;
use anyhow::anyhow;
use golem_common::model::component::ComponentDto;
use golem_common::model::oplog::public_oplog_entry::FailedUpdateParams;
use golem_common::model::oplog::{PublicOplogEntry, UpdateFailureClass};
use golem_common::model::{OplogIndex, OwnedAgentId};
use golem_common::{agent_key, data_value};
use golem_test_framework::dsl::TestDsl;
use golem_worker_executor::services::golem_config::SnapshotPolicy;
use golem_worker_executor_test_utils::{
    LastUniqueId, PrecompiledComponent, TestContext, TestExecutorOverrides, TestWorkerExecutor,
    WorkerExecutorTestDependencies,
};
use pretty_assertions::assert_eq;
use std::sync::Arc;
use test_r::{inherit_test_dep, test, timeout};

inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(LastUniqueId);
inherit_test_dep!(
    #[tagged_as("agent_update_v1")]
    PrecompiledComponent
);
inherit_test_dep!(Tracing);

/// Starts a `SnapshotUpdateTest` source, runs `stable_value` and the suffix method, and returns
/// the source and the index of the suffix's `AgentInvocationFinished`: a cut before any snapshot
/// that follows the suffix. `pre_snapshot_value` returns a different value in agent-updates-v2,
/// so a full replay of it would diverge before the suffix.
async fn source(
    executor: &TestWorkerExecutor,
    component: &ComponentDto,
    suffix: &str,
) -> anyhow::Result<(OwnedAgentId, OplogIndex)> {
    let source_id =
        crate::agent::create_unkeyed_agent(executor, component, &agent_key!("SnapshotUpdateTest"))
            .await?;
    for method in ["stable_value", suffix] {
        crate::agent::invoke_and_await_agent_by_id(
            executor,
            component,
            &source_id,
            None,
            method,
            data_value!(),
        )
        .await?;
    }
    let cut = executor
        .get_oplog(&source_id, OplogIndex::INITIAL)
        .await?
        .iter()
        .rev()
        .find(|entry| matches!(entry.entry, PublicOplogEntry::AgentInvocationFinished(_)))
        .map(|entry| entry.oplog_index)
        .ok_or_else(|| anyhow!("{suffix} did not finish"))?;
    Ok((source_id, cut))
}

/// Forks `source_id` at `cut` with the never-live mark, updates the fork to `target` and returns
/// its one failed update.
async fn failed_update(
    executor: &TestWorkerExecutor,
    source_id: &OwnedAgentId,
    cut: OplogIndex,
    target: golem_common::model::component::ComponentRevision,
) -> anyhow::Result<FailedUpdateParams> {
    let fork = executor.fork_worker_never_live(source_id, cut).await?;
    executor.auto_update_worker(&fork, target, false).await?;
    let outcome = wait_for_update_outcomes(executor, &fork, cut, 1, false)
        .await?
        .remove(0);
    match outcome.entry {
        PublicOplogEntry::FailedUpdate(failed) => Ok(failed),
        other => Err(anyhow!("the update did not fail: {other:?}")),
    }
}

/// E20: a host error in the replayed suffix of a never-live fork fails the update as
/// `Operational`, the class the same failure has without the mark. The classified permanent host
/// failure of E20 is a failing transaction status query, in
/// `a_permanently_failing_transaction_status_query_fails_an_update_as_operational`.
#[test]
#[timeout("4m")]
async fn a_host_error_in_the_suffix_of_a_never_live_fork_is_operational(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_update_v1")] agent_update_v1: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = crate::fork::start_with_local_resume_and(
        deps,
        &context,
        TestExecutorOverrides {
            configure: Some(Arc::new(|config| {
                config.oplog.default_snapshotting = SnapshotPolicy::Disabled;
            })),
            ..Default::default()
        },
    )
    .await?;
    let component = executor
        .component_dep(&context.default_environment_id, agent_update_v1)
        .store()
        .await?;
    let (source_id, cut) = source(&executor, &component, "suffix_host_error").await?;
    let target = executor
        .update_component(&component.id, "it_agent_update_v2_release")
        .await?;
    let failed = failed_update(&executor, &source_id, cut, target.revision).await?;
    assert_eq!(
        failed.failure_class,
        UpdateFailureClass::Operational,
        "{failed:?}"
    );
    Ok(())
}
