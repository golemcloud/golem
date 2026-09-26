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
use golem_common::model::{AgentStatus, OplogIndex, PromiseId};
use golem_common::{agent_id, data_value};
use golem_test_framework::dsl::TestDsl;
use golem_worker_executor_test_utils::{
    LastUniqueId, PrecompiledComponent, TestContext, TestExecutorOverrides,
    WorkerExecutorTestDependencies, start, start_with_overrides,
};
use std::collections::HashMap;
use std::sync::Arc;
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

/// A Go caller that is still waiting on an RPC when the executor's RPC idle
/// window closes is suspended, and must be resumed into the same call once the
/// target answers.
///
/// The target blocks on a promise, so the test decides when the call returns:
/// it waits for the caller to be suspended mid-call, then completes the promise.
/// A Rust caller in the same position is resumed and returns the result.
#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn go_rpc_caller_resumes_after_suspending_mid_call(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("agent_sdk_go")] agent_sdk_go: &PrecompiledComponent,
) -> anyhow::Result<()> {
    rpc_caller_resumes_after_suspending_mid_call(last_unique_id, deps, agent_sdk_go, "await-remote")
        .await
}

/// The same through `CallAsync` + `Future.Get`.
#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn go_async_rpc_caller_resumes_after_suspending_mid_call(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("agent_sdk_go")] agent_sdk_go: &PrecompiledComponent,
) -> anyhow::Result<()> {
    rpc_caller_resumes_after_suspending_mid_call(
        last_unique_id,
        deps,
        agent_sdk_go,
        "await-remote-async",
    )
    .await
}

async fn rpc_caller_resumes_after_suspending_mid_call(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    agent_sdk_go: &PrecompiledComponent,
    method: &'static str,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    // A short RPC idle window, so the caller is suspended within seconds.
    let executor = start_with_overrides(
        deps,
        &context,
        TestExecutorOverrides {
            configure: Some(Arc::new(|config| {
                config.suspend.rpc_suspend_after = Duration::from_secs(2);
                config.suspend.rpc_resume_after = Duration::from_secs(1);
            })),
            ..TestExecutorOverrides::default()
        },
    )
    .await?;

    let component = executor
        .component_dep(&context.default_environment_id, agent_sdk_go)
        .store()
        .await?;

    let target_name = format!("go-rpc-suspend-target-{method}");
    let target_id = agent_id!("PromiseAgent", target_name.clone());
    let target = executor
        .start_agent_with(&component.id, target_id.clone(), HashMap::new(), Vec::new())
        .await?;
    let oplog_idx = executor
        .invoke_and_await_agent(&component, &target_id, "create", data_value!())
        .await?
        .into_typed::<i64>()?;

    let caller_id = agent_id!("RpcAgent", format!("go-rpc-suspend-caller-{method}"));
    let caller = executor
        .start_agent_with(&component.id, caller_id.clone(), HashMap::new(), Vec::new())
        .await?;

    let call = {
        let executor = executor.clone();
        let component = component.clone();
        let caller_id = caller_id.clone();
        tokio::spawn(
            async move {
                executor
                    .invoke_and_await_agent(
                        &component,
                        &caller_id,
                        method,
                        data_value!(target_name, oplog_idx),
                    )
                    .await
            }
            .in_current_span(),
        )
    };

    // The caller is still inside the RPC, so it outlasts the idle window.
    executor
        .wait_for_status(&caller, AgentStatus::Suspended, Duration::from_secs(30))
        .await?;

    executor
        .complete_promise(
            &PromiseId {
                agent_id: target.clone(),
                oplog_idx: OplogIndex::from_u64(oplog_idx as u64),
            },
            b"\"approved\"".to_vec(),
        )
        .await?;

    // Resumption is scheduled rpc_resume_after later; a caller that is never
    // resumed fails here rather than hanging the suite.
    let returned = tokio::time::timeout(Duration::from_secs(45), call)
        .await
        .map_err(|_| {
            anyhow::anyhow!("the Go caller was suspended mid-RPC and never resumed with the result")
        })???
        .into_typed::<String>()?;

    executor.check_oplog_is_queryable(&caller).await?;
    drop(executor);

    assert_eq!(returned, "approved");
    Ok(())
}
