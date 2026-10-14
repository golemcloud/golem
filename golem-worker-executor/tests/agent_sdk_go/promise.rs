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

//! The Go SDK's promise wrapper: awaiting a promise durably SUSPENDS the worker
//! (it does not spin or return early), and completing it from outside resumes the
//! agent with the payload. Mirrors `api::promise`.

use crate::Tracing;
use anyhow::anyhow;
use golem_common::model::{AgentStatus, OplogIndex, PromiseId};
use golem_common::{agent_id, data_value};
use golem_test_framework::dsl::TestDsl;
use golem_worker_executor_test_utils::{
    LastUniqueId, PrecompiledComponent, TestContext, WorkerExecutorTestDependencies, start,
};
use std::collections::HashMap;
use std::time::Duration;
use test_r::{inherit_test_dep, test, timeout};
use tracing::Instrument;

inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(LastUniqueId);
inherit_test_dep!(Tracing);
inherit_test_dep!(
    #[tagged_as("agent_sdk_go")]
    PrecompiledComponent
);

/// The agent creates a promise and awaits it in a second invocation; the worker
/// must reach Suspended rather than returning, and completing the promise through
/// the executor API resumes it with the completed payload.
#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn go_promise_suspends_until_completed(
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
    let agent_id = agent_id!("PromiseAgent", "go-promise-1");
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), HashMap::new(), Vec::new())
        .await?;

    // The agent returns the promise's oplog index, which addresses it for completion.
    let oplog_idx = executor
        .invoke_and_await_agent(&component, &agent_id, "create", data_value!())
        .await?
        .into_typed::<i64>()?;

    let executor_clone = executor.clone();
    let component_clone = component.clone();
    let agent_id_clone = agent_id.clone();
    let mut fiber = tokio::spawn(
        async move {
            executor_clone
                .invoke_and_await_agent(
                    &component_clone,
                    &agent_id_clone,
                    "await",
                    data_value!(oplog_idx),
                )
                .await
        }
        .in_current_span(),
    );

    // The await must park the worker rather than complete.
    tokio::select! {
        result = &mut fiber => {
            let invoke_result = result??;
            return Err(anyhow!("await returned instead of suspending: {invoke_result:?}"));
        }
        status = executor.wait_for_status(&worker_id, AgentStatus::Suspended, Duration::from_secs(10)) => {
            status?;
        }
    }

    executor
        .complete_promise(
            &PromiseId {
                agent_id: worker_id.clone(),
                oplog_idx: OplogIndex::from_u64(oplog_idx as u64),
            },
            // The Go wrapper decodes the payload as JSON for a string T.
            b"\"approved\"".to_vec(),
        )
        .await?;

    let awaited = fiber.await??.into_typed::<String>()?;

    executor.check_oplog_is_queryable(&worker_id).await?;
    drop(executor);

    assert_eq!(awaited, "approved");
    Ok(())
}
