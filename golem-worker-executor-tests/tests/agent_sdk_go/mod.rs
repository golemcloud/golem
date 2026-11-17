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

pub mod blobstore;
pub mod config;
pub mod durability;
pub mod http;
pub mod invocation;
pub mod keyvalue;
pub mod logging;
pub mod permissions;
pub mod promise;
pub mod rdbms;
pub mod retry;
pub mod revert;
pub mod rich_types;
pub mod rpc;
pub mod schedule;
pub mod snapshot;
pub mod transaction_http;
pub mod transactions;
pub mod websocket;

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

/// A handler panic fails its invocation without discarding preceding mutations
/// or preventing a later invocation on the same durable agent.
#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn go_handler_panic_fails_only_its_invocation(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("agent_sdk_go")] agent_sdk_go: &PrecompiledComponent,
) -> anyhow::Result<()> {
    use golem_common::model::IdempotencyKey;

    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component_dep(&context.default_environment_id, agent_sdk_go)
        .store()
        .await?;

    let agent_id = agent_id!("CounterAgent", "go-counter-panic-1");
    let worker = executor
        .start_agent_with(&component.id, agent_id.clone(), HashMap::new(), Vec::new())
        .await?;

    assert_eq!(
        executor
            .invoke_and_await_agent(&component, &agent_id, "declaredError", data_value!())
            .await?
            .into_typed::<Result<i64, String>>()?,
        Err("declared error".to_string())
    );
    let key = IdempotencyKey::fresh();
    let result = executor
        .invoke_and_await_agent_with_key(&component, &agent_id, &key, "fail", data_value!())
        .await;
    let err = format!(
        "{:#}",
        result.expect_err("a panicking handler must not return")
    );
    assert!(err.contains("counter gave up"), "unexpected error: {err}");
    assert!(
        err.contains("fail"),
        "the failure must retain the method name: {err}"
    );
    assert_eq!(
        executor
            .invoke_and_await_agent(&component, &agent_id, "value", data_value!())
            .await?
            .into_typed::<i64>()?,
        7
    );
    assert_eq!(
        executor
            .invoke_and_await_agent(&component, &agent_id, "increment", data_value!())
            .await?
            .into_typed::<i64>()?,
        8
    );
    let before = executor.get_oplog_last_index(&worker).await?;
    executor.release().await?;
    let executor = start(deps, &context).await?;
    let failure = executor
        .invoke_and_await_agent_with_key(&component, &agent_id, &key, "fail", data_value!())
        .await
        .expect_err("cold lookup must retain the handler failure");
    assert!(
        format!("{failure:#}").contains("counter gave up"),
        "{failure:#}"
    );
    assert_eq!(executor.get_oplog_last_index(&worker).await?, before);
    assert_eq!(
        executor
            .invoke_and_await_agent(&component, &agent_id, "value", data_value!())
            .await?
            .into_typed::<i64>()?,
        8
    );
    assert_eq!(
        executor
            .invoke_and_await_agent(&component, &agent_id, "increment", data_value!())
            .await?
            .into_typed::<i64>()?,
        9
    );
    Ok(())
}
