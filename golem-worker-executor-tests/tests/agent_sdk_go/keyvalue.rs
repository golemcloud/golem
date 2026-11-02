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

//! The Go SDK's keyvalue wrapper against the real host store: values round-trip,
//! deletion is observable, and a write survives a crash by replaying from the
//! oplog. Mirrors `keyvalue::readwrite_get_returns_the_value_that_was_set`.

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

/// A value written through the SDK wrapper is read back, listed, and deleted;
/// reading an absent key yields the wrapper's "not found" answer (empty string).
#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn go_keyvalue_round_trip(
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
    let agent_id = agent_id!("KvAgent", "go-kv-1");
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), HashMap::new(), Vec::new())
        .await?;

    // The bucket is namespaced by component so parallel tests cannot collide.
    let bucket = format!("{}-go-kv-1", component.id);

    executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "set",
            data_value!(bucket.clone(), "alpha", "first"),
        )
        .await?;
    executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "set",
            data_value!(bucket.clone(), "beta", "second"),
        )
        .await?;

    let stored = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "get",
            data_value!(bucket.clone(), "alpha"),
        )
        .await?
        .into_typed::<String>()?;
    let exists = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "exists",
            data_value!(bucket.clone(), "alpha"),
        )
        .await?
        .into_typed::<bool>()?;
    let keys = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "keys",
            data_value!(bucket.clone(), ""),
        )
        .await?
        .into_typed::<Vec<String>>()?;

    executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "delete",
            data_value!(bucket.clone(), "alpha"),
        )
        .await?;

    let after_delete = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "get",
            data_value!(bucket.clone(), "alpha"),
        )
        .await?
        .into_typed::<String>()?;
    let missing = executor
        .invoke_and_await_agent(&component, &agent_id, "get", data_value!(bucket, "nope"))
        .await?
        .into_typed::<String>()?;

    executor.check_oplog_is_queryable(&worker_id).await?;
    drop(executor);

    assert_eq!(stored, "first");
    assert!(exists);
    assert_eq!(keys, vec!["alpha".to_string(), "beta".to_string()]);
    assert_eq!(after_delete, "");
    assert_eq!(missing, "");
    Ok(())
}

/// A key written before a crash is still readable afterwards: the recorded write
/// replays from the oplog rather than being lost or re-executed.
#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn go_keyvalue_survives_crash(
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
    let agent_id = agent_id!("KvAgent", "go-kv-2");
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), HashMap::new(), Vec::new())
        .await?;

    let bucket = format!("{}-go-kv-2", component.id);

    executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "set",
            data_value!(bucket.clone(), "key", "value"),
        )
        .await?;

    executor.simulated_crash(&worker_id).await?;

    let after_crash = executor
        .invoke_and_await_agent(&component, &agent_id, "get", data_value!(bucket, "key"))
        .await?
        .into_typed::<String>()?;

    executor.check_oplog_is_queryable(&worker_id).await?;
    drop(executor);

    assert_eq!(after_crash, "value");
    Ok(())
}
