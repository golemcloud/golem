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

//! The Go SDK's blobstore wrapper against the real host store: objects round-trip
//! with their metadata, listing reflects writes and deletes.

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

/// Written objects are readable with the right size, listed in the container, and
/// gone after deletion; reading an absent object yields the wrapper's "not found"
/// answer (empty string).
#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn go_blobstore_round_trip(
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
    let agent_id = agent_id!("BlobAgent", "go-blob-1");
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), HashMap::new(), Vec::new())
        .await?;

    // The container is namespaced by component so parallel tests cannot collide.
    let container = format!("{}-go-blob-1", component.id);

    executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "write",
            data_value!(container.clone(), "greeting.txt", "hello"),
        )
        .await?;
    executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "write",
            data_value!(container.clone(), "other.txt", "x"),
        )
        .await?;

    let content = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "read",
            data_value!(container.clone(), "greeting.txt"),
        )
        .await?
        .into_typed::<String>()?;
    let size = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "size",
            data_value!(container.clone(), "greeting.txt"),
        )
        .await?
        .into_typed::<i64>()?;
    let listed = executor
        .invoke_and_await_agent(&component, &agent_id, "list", data_value!(container.clone()))
        .await?
        .into_typed::<Vec<String>>()?;

    executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "delete",
            data_value!(container.clone(), "other.txt"),
        )
        .await?;

    let after_delete = executor
        .invoke_and_await_agent(&component, &agent_id, "list", data_value!(container.clone()))
        .await?
        .into_typed::<Vec<String>>()?;
    let missing = executor
        .invoke_and_await_agent(&component, &agent_id, "read", data_value!(container, "gone"))
        .await?
        .into_typed::<String>()?;

    executor.check_oplog_is_queryable(&worker_id).await?;
    drop(executor);

    assert_eq!(content, "hello");
    assert_eq!(size, 5);
    assert_eq!(
        listed,
        vec!["greeting.txt".to_string(), "other.txt".to_string()]
    );
    assert_eq!(after_delete, vec!["greeting.txt".to_string()]);
    assert_eq!(missing, "");
    Ok(())
}
