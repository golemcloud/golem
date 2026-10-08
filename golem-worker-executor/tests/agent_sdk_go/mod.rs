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

//! Runtime tests for the Go SDK, driven through the `agent-sdk-go` guest (built
//! by `test-components/build-components.sh go`). This is the foundational suite —
//! more scenarios (durability/replay, RPC, config, …) build on this wiring.

pub mod atomic_region;
pub mod blobstore;
pub mod config;
pub mod diagnostics;
pub mod durability;
pub mod http;
pub mod keyvalue;
pub mod promise;
pub mod retry;
pub mod revert;
pub mod rich_types;
pub mod rpc;
pub mod schedule;
pub mod snapshot;
pub mod transactions;
pub mod websocket;

use crate::Tracing;
use golem_common::model::AgentStatus;
use golem_common::model::oplog::{LogLevel, OplogIndex, PublicOplogEntry};
use golem_common::{agent_id, data_value};
use golem_test_framework::dsl::TestDsl;
use golem_worker_executor_test_utils::{
    LastUniqueId, PrecompiledComponent, TestContext, WorkerExecutorTestDependencies, start,
};
use std::collections::HashMap;
use std::time::Duration;
use test_r::{inherit_test_dep, test, timeout};

inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(LastUniqueId);
inherit_test_dep!(Tracing);
inherit_test_dep!(
    #[tagged_as("agent_sdk_go")]
    PrecompiledComponent
);

/// A durable Go counter agent registers, dispatches its methods, and keeps state
/// across invocations — the smoke test that proves the agent-sdk-go guest builds
/// and runs under the worker executor.
#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn go_counter_basic_invoke(
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

    let agent_id = agent_id!("CounterAgent", "go-counter-1");
    executor
        .start_agent_with(&component.id, agent_id.clone(), HashMap::new(), Vec::new())
        .await?;

    let v1 = executor
        .invoke_and_await_agent(&component, &agent_id, "increment", data_value!())
        .await?
        .into_typed::<i64>()?;
    assert_eq!(v1, 1);

    let v2 = executor
        .invoke_and_await_agent(&component, &agent_id, "add", data_value!(5i64))
        .await?
        .into_typed::<i64>()?;
    assert_eq!(v2, 6);

    let v3 = executor
        .invoke_and_await_agent(&component, &agent_id, "value", data_value!())
        .await?
        .into_typed::<i64>()?;
    assert_eq!(v3, 6);

    Ok(())
}

/// A panic in a handler traps the component, as in the other SDKs: the agent
/// fails rather than exiting, and the panic is reported on stderr in one entry.
#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn go_handler_panic_fails_the_agent(
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

    let agent_id = agent_id!("CounterAgent", "go-counter-panic-1");
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), HashMap::new(), Vec::new())
        .await?;

    let result = executor
        .invoke_and_await_agent(&component, &agent_id, "fail", data_value!())
        .await;
    let err = format!(
        "{:#}",
        result.expect_err("a panicking handler must not return")
    );
    assert!(err.contains("unreachable"), "unexpected error: {err}");

    executor
        .wait_for_status(&worker_id, AgentStatus::Failed, Duration::from_secs(10))
        .await?;

    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    let stderr: Vec<&str> = oplog
        .iter()
        .filter_map(|e| match &e.entry {
            PublicOplogEntry::Log(log) if log.level == LogLevel::Stderr => {
                Some(log.message.as_str())
            }
            _ => None,
        })
        .collect();
    assert_eq!(stderr.len(), 1, "unexpected stderr: {stderr:?}");
    assert!(
        stderr[0].starts_with("panic: agent method \"fail\" panicked: counter gave up"),
        "unexpected stderr: {}",
        stderr[0]
    );
    Ok(())
}
