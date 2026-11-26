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
//! synchronously (Call) and through a triggered invocation (Trigger +
//! Invocation.AwaitResult), and the
//! ledger's accumulating state confirms the calls reached a real target.

use crate::Tracing;
use golem_common::model::oplog::{PublicAgentInvocation, PublicOplogEntry};
use golem_common::model::{AgentStatus, OplogIndex};
use golem_common::schema::SchemaValue;
use golem_common::{agent_key, data_value};
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
/// instance), and Trigger + Invocation.AwaitResult works and routes by region.
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
    let agent_id = agent_key!("RpcAgent", "go-rpc-1");
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

    // A triggered call to a different region, awaited through its handle, hits a
    // fresh ledger instance.
    let r3 = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "trigger-await",
            data_value!("us", 7i64),
        )
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

/// The same through `Trigger` + `Invocation.AwaitResult`.
#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn go_trigger_await_caller_resumes_after_suspending_mid_call(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("agent_sdk_go")] agent_sdk_go: &PrecompiledComponent,
) -> anyhow::Result<()> {
    rpc_caller_resumes_after_suspending_mid_call(
        last_unique_id,
        deps,
        agent_sdk_go,
        "await-remote-trigger",
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
    let target_id = agent_key!("PromiseAgent", target_name.clone());
    let target = executor
        .start_agent_with(&component.id, target_id.clone(), HashMap::new(), Vec::new())
        .await?;
    let oplog_idx = executor
        .invoke_and_await_agent(&component, &target_id, "create", data_value!())
        .await?
        .into_typed::<i64>()?;

    let caller_id = agent_key!("RpcAgent", format!("go-rpc-suspend-caller-{method}"));
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
            &executor.context.default_environment_id,
            &executor
                .promise_id(&target, OplogIndex::from_u64(oplog_idx as u64))
                .await?,
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

/// Goroutines call several ledgers at once; after a restart the replay serves
/// those calls from the oplog instead of re-issuing them, so a second round
/// leaves every ledger at exactly twice the amount.
#[test]
#[tracing::instrument]
#[timeout("3m")]
async fn go_parallel_rpc_from_goroutines_replays(
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
    let agent_id = agent_key!("RpcAgent", "go-rpc-parallel");
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), HashMap::new(), Vec::new())
        .await?;
    let regions: Vec<String> = (0..8).map(|i| format!("parallel-{i}")).collect();

    let first = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "call-parallel",
            data_value!(regions.clone(), 3i64),
        )
        .await?
        .into_typed::<Vec<i64>>()?;
    assert_eq!(first, vec![3; 8]);
    executor.check_oplog_is_queryable(&worker_id).await?;

    drop(executor);
    let executor = start(deps, &context).await?;

    let second = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "call-parallel",
            data_value!(regions, 3i64),
        )
        .await?
        .into_typed::<Vec<i64>>()?;
    assert_eq!(
        second,
        vec![6; 8],
        "replaying the first round must not record on the ledgers again"
    );
    executor.check_oplog_is_queryable(&worker_id).await?;
    Ok(())
}

/// The Go SDK's invocation handles: a triggered invocation completes and can no
/// longer be cancelled, a handle rebuilt from its ID awaits the same typed
/// result, and an invocation scheduled an hour ahead is cancelled before it
/// starts, so awaiting it fails with the `RemoteCancelled` kind and the counter
/// never sees the scheduled addition.
#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn go_invocation_handles(
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
    let agent_id = agent_key!("RpcAgent", "go-invocation-handles");
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), HashMap::new(), Vec::new())
        .await?;

    let result = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "invocation-handles",
            data_value!("go-invocation-handles-counter"),
        )
        .await?;
    executor.check_oplog_is_queryable(&worker_id).await?;

    let Some(SchemaValue::Record { fields }) = result.value() else {
        panic!("expected the invocation handle report record: {result:?}");
    };
    assert_eq!(
        fields.as_slice(),
        [
            SchemaValue::String("completed".to_string()),
            SchemaValue::Bool(false),
            SchemaValue::S64(3),
            SchemaValue::String("scheduled".to_string()),
            SchemaValue::Bool(true),
            SchemaValue::String("cancelled".to_string()),
            SchemaValue::String("cancelled".to_string()),
            SchemaValue::S64(3),
        ],
        "triggeredStatus, cancelCompleted, rebuiltValue, scheduledStatus, cancelScheduled, \
         cancelledStatus, awaitCancelled, value"
    );

    // The cancelled addition of 100 never ran on the counter.
    let counter_id = agent_key!("CounterAgent", "go-invocation-handles-counter");
    let value = executor
        .invoke_and_await_agent(&component, &counter_id, "value", data_value!())
        .await?
        .into_typed::<i64>()?;
    assert_eq!(value, 3, "only the triggered addition ran");
    Ok(())
}

/// Cancelling an invocation that has already started fails with the
/// `RemoteCannotCancelRunning` kind, and the invocation keeps running: it
/// returns once its promise is completed.
#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn go_invocation_handle_cannot_cancel_running(
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

    let target_id = agent_key!("PromiseAgent", "go-cancel-running-target");
    let target = executor
        .start_agent_with(&component.id, target_id.clone(), HashMap::new(), Vec::new())
        .await?;
    let oplog_idx = executor
        .invoke_and_await_agent(&component, &target_id, "create", data_value!())
        .await?
        .into_typed::<i64>()?;

    let caller_id = agent_key!("RpcAgent", "go-cancel-running-caller");
    let caller = executor
        .start_agent_with(&component.id, caller_id.clone(), HashMap::new(), Vec::new())
        .await?;
    let result = executor
        .invoke_and_await_agent(
            &component,
            &caller_id,
            "cancel-running",
            data_value!("go-cancel-running-target", oplog_idx),
        )
        .await?;
    executor.check_oplog_is_queryable(&caller).await?;

    let Some(SchemaValue::Record { fields }) = result.value() else {
        panic!("expected the cancel-running report record: {result:?}");
    };
    let [
        SchemaValue::String(running_status),
        SchemaValue::String(cancel_running),
    ] = fields.as_slice()
    else {
        panic!("unexpected cancel-running report: {fields:?}");
    };
    assert!(
        running_status == "running" || running_status == "waiting",
        "the await should have started: {running_status}"
    );
    assert_eq!(cancel_running, "cannot cancel running");

    // The refused cancellation left the await running: completing its promise
    // lets it finish.
    executor
        .complete_promise(
            &executor.context.default_environment_id,
            &executor
                .promise_id(&target, OplogIndex::from_u64(oplog_idx as u64))
                .await?,
            b"\"approved\"".to_vec(),
        )
        .await?;
    // The target runs its invocations in order, so this one starts only after
    // the await has finished.
    let completed = executor
        .invoke_and_await_agent(
            &component,
            &target_id,
            "complete",
            data_value!(oplog_idx, "again"),
        )
        .await?
        .into_typed::<bool>()?;
    assert!(!completed, "the promise was already completed");
    executor.check_oplog_is_queryable(&target).await?;
    Ok(())
}

/// The garbage collector releases one handle of an invocation while a second handle built from
/// its ID awaits it. The release neither cancels the invocation nor ends the other await: the
/// await receives the result, and the target runs the invocation once. The guest reads the
/// second handle's status only after the collection, and the promise is completed only after
/// that read, so the collection happens while the await is pending.
#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn go_collected_invocation_handle_does_not_cancel_its_invocation(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("agent_sdk_go")] agent_sdk_go: &PrecompiledComponent,
) -> anyhow::Result<()> {
    const HANDLE_STATUS: &str = "golem::rpc::invocation::status";
    const COLLECTION_TIMEOUT: Duration = Duration::from_secs(30);
    const POLL_INTERVAL: Duration = Duration::from_millis(10);
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component_dep(&context.default_environment_id, agent_sdk_go)
        .store()
        .await?;

    let target_id = agent_key!("PromiseAgent", "go-collected-handle-target");
    let target = executor
        .start_agent_with(&component.id, target_id.clone(), HashMap::new(), Vec::new())
        .await?;
    let oplog_idx = executor
        .invoke_and_await_agent(&component, &target_id, "create", data_value!())
        .await?
        .into_typed::<i64>()?;

    let caller_id = agent_key!("RpcAgent", "go-collected-handle-caller");
    let caller = executor
        .start_agent_with(&component.id, caller_id.clone(), HashMap::new(), Vec::new())
        .await?;
    let collecting = {
        let executor = executor.clone();
        let component = component.clone();
        let caller_id = caller_id.clone();
        tokio::spawn(async move {
            executor
                .invoke_and_await_agent(
                    &component,
                    &caller_id,
                    "collected-handle",
                    data_value!("go-collected-handle-target", oplog_idx),
                )
                .await
        })
    };
    // The guest reads the status only after the first handle is collected.
    tokio::time::timeout(COLLECTION_TIMEOUT, async {
        loop {
            executor.commit_oplog(&caller.agent_id).await?;
            let oplog = executor.get_oplog(&caller, OplogIndex::INITIAL).await?;
            if oplog.iter().any(|entry| {
                matches!(&entry.entry, PublicOplogEntry::Start(start)
                    if start.function_name == HANDLE_STATUS)
            }) {
                return Ok::<_, anyhow::Error>(());
            }
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    })
    .await??;
    executor
        .complete_promise(
            &executor.context.default_environment_id,
            &executor
                .promise_id(&target, OplogIndex::from_u64(oplog_idx as u64))
                .await?,
            b"\"approved\"".to_vec(),
        )
        .await?;
    let result = collecting.await??;
    executor.check_oplog_is_queryable(&caller).await?;

    let Some(SchemaValue::Record { fields }) = result.value() else {
        panic!("expected the collected-handle report record: {result:?}");
    };
    assert_eq!(
        fields.as_slice(),
        [
            SchemaValue::Bool(true),
            SchemaValue::Bool(true),
            SchemaValue::String("approved".to_string()),
            SchemaValue::String("completed".to_string()),
        ],
        "collected, pendingAtCollection, value, finalStatus"
    );
    executor.commit_oplog(&target.agent_id).await?;
    let awaits = executor
        .get_oplog(&target, OplogIndex::INITIAL)
        .await?
        .iter()
        .filter(|entry| {
            matches!(&entry.entry, PublicOplogEntry::AgentInvocationStarted(started)
                if matches!(&started.invocation, PublicAgentInvocation::AgentMethodInvocation(invocation)
                    if invocation.method_name == "await"))
        })
        .count();
    assert_eq!(awaits, 1, "the target ran the awaited invocation once");
    Ok(())
}
