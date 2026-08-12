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

//! Cross-agent RPC for the Go SDK: a caller agent invokes a durable ledger agent
//! synchronously (Call) and asynchronously (CallAsync + Future.Get), and the
//! ledger's accumulating state confirms the calls reached a real target.

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

/// Synchronous Call accumulates on the target (same region → same durable ledger
/// instance), and CallAsync + Future.Get works and routes by region.
#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn go_rpc_sync_and_async_calls(
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
    let agent_id = agent_id!("RpcAgent", "go-rpc-1");
    executor
        .start_agent_with(&component.id, agent_id.clone(), HashMap::new(), Vec::new())
        .await?;

    // Two synchronous calls to the same region accumulate on that ledger instance.
    let r1 = executor
        .invoke_and_await_agent(&component, &agent_id, "call", data_value!("eu", 10i64))
        .await?
        .into_typed::<i64>()?;
    assert_eq!(r1, 10);

    let r2 = executor
        .invoke_and_await_agent(&component, &agent_id, "call", data_value!("eu", 5i64))
        .await?
        .into_typed::<i64>()?;
    assert_eq!(r2, 15);

    // An async call to a different region hits a fresh ledger instance.
    let r3 = executor
        .invoke_and_await_agent(&component, &agent_id, "async", data_value!("us", 7i64))
        .await?
        .into_typed::<i64>()?;
    assert_eq!(r3, 7);

    Ok(())
}
