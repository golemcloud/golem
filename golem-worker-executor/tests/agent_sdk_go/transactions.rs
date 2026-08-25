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

//! The Go SDK's saga helpers: a committed transaction runs only its forward
//! steps, while a failing one rolls back — running the compensation of every step
//! that had already succeeded. The agent records each step and compensation as it
//! runs, so the recorded order is the assertion.

use crate::Tracing;
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

/// All steps succeed: the transaction commits and no compensation runs.
#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn go_transaction_commits(
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
    let agent_id = agent_id!("SagaAgent", "go-saga-ok");
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), HashMap::new(), Vec::new())
        .await?;

    let outcome = executor
        .invoke_and_await_agent(&component, &agent_id, "run", data_value!(false))
        .await?
        .into_typed::<String>()?;
    let recorded = executor
        .invoke_and_await_agent(&component, &agent_id, "log", data_value!())
        .await?
        .into_typed::<Vec<String>>()?;

    executor.check_oplog_is_queryable(&worker_id).await?;
    drop(executor);

    assert_eq!(outcome, "committed");
    assert_eq!(recorded, vec!["charge".to_string(), "ship".to_string()]);
    Ok(())
}

/// A later step fails: the transaction rolls back, running the compensation of
/// the step that had already succeeded (charge → refund). The failed step is not
/// compensated, since it never took effect.
#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn go_transaction_rolls_back_with_compensation(
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
    let agent_id = agent_id!("SagaAgent", "go-saga-fail");
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), HashMap::new(), Vec::new())
        .await?;

    let outcome = executor
        .invoke_and_await_agent(&component, &agent_id, "run", data_value!(true))
        .await?
        .into_typed::<String>()?;
    let recorded = executor
        .invoke_and_await_agent(&component, &agent_id, "log", data_value!())
        .await?
        .into_typed::<Vec<String>>()?;

    executor.check_oplog_is_queryable(&worker_id).await?;
    drop(executor);

    assert_eq!(outcome, "rolled-back");
    assert_eq!(
        recorded,
        vec![
            "charge".to_string(),
            "ship".to_string(),
            "refund".to_string()
        ]
    );
    Ok(())
}
