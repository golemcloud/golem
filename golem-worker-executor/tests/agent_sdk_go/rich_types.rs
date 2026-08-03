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

//! Composite value types round-trip through the invocation wire for the Go SDK:
//! a list + optional argument in, and a list out.

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

#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn go_rich_types_round_trip(
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
    let agent_id = agent_id!("RichAgent", "go-rich-1");
    executor
        .start_agent_with(&component.id, agent_id.clone(), HashMap::new(), Vec::new())
        .await?;

    // list + Some(option) in.
    let described = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "describe",
            data_value!(vec!["a".to_string(), "b".to_string()], Some("hi".to_string())),
        )
        .await?
        .into_typed::<String>()?;
    assert_eq!(described, "tags=a,b note=hi");

    // empty list + None option.
    let empty = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "describe",
            data_value!(Vec::<String>::new(), Option::<String>::None),
        )
        .await?
        .into_typed::<String>()?;
    assert_eq!(empty, "tags= note=none");

    // list out.
    let repeated = executor
        .invoke_and_await_agent(&component, &agent_id, "repeat", data_value!("x".to_string(), 3i64))
        .await?
        .into_typed::<Vec<String>>()?;
    assert_eq!(repeated, vec!["x".to_string(), "x".to_string(), "x".to_string()]);

    Ok(())
}
