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

//! Reverting a Go agent's invocations rolls its state back: the oplog is the
//! source of truth for Go state too, so undoing recorded invocations rebuilds the
//! earlier value.

use crate::Tracing;
use golem_common::model::worker::{RevertLastInvocations, RevertWorkerTarget};
use golem_common::{agent_id, data_value};
use golem_test_framework::dsl::TestDsl;
use golem_worker_executor_test_utils::{
    LastUniqueId, PrecompiledComponent, TestContext, WorkerExecutorTestDependencies, start,
};
use std::collections::HashMap;
use test_r::{inherit_test_dep, test, timeout};

inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(LastUniqueId);
inherit_test_dep!(Tracing);
inherit_test_dep!(
    #[tagged_as("agent_sdk_go")]
    PrecompiledComponent
);

/// After three recorded increments the counter reads 6; reverting the last two
/// invocations (the `add` and the `value` read) rebuilds the state as of the
/// first increment.
#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn go_revert_last_invocations(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("agent_sdk_go")] agent_sdk_go: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component_dep(&context.default_environment_id, agent_sdk_go)
        .store()
        .await?;
    let agent_id = agent_id!("CounterAgent", "go-revert-1");
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), HashMap::new(), Vec::new())
        .await?;

    executor
        .invoke_and_await_agent(&component, &agent_id, "increment", data_value!())
        .await?;
    executor
        .invoke_and_await_agent(&component, &agent_id, "add", data_value!(5i64))
        .await?;
    let before = executor
        .invoke_and_await_agent(&component, &agent_id, "value", data_value!())
        .await?
        .into_typed::<i64>()?;

    // Undo the `value` read and the `add`, leaving only the first increment.
    executor
        .revert(
            &worker_id,
            RevertWorkerTarget::RevertLastInvocations(RevertLastInvocations {
                number_of_invocations: 2,
            }),
        )
        .await?;

    let after = executor
        .invoke_and_await_agent(&component, &agent_id, "value", data_value!())
        .await?
        .into_typed::<i64>()?;

    executor.check_oplog_is_queryable(&worker_id).await?;
    drop(executor);

    assert_eq!(before, 6);
    assert_eq!(after, 1);
    Ok(())
}
