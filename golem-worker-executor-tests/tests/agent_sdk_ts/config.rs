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

use crate::Tracing;
use golem_common::model::agent::AgentConfigEntryDto;
use golem_common::model::oplog::{OplogIndex, PublicOplogEntry};
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
    #[tagged_as("agent_sdk_ts")]
    PrecompiledComponent
);

#[test]
#[timeout("120s")]
#[tracing::instrument]
async fn static_ts_config_defaults_and_creation_overrides_survive_reconstruction(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_sdk_ts")] agent_sdk_ts: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let corpus: serde_json::Value = serde_json::from_str(include_str!(
        "../../../test-data/config-conformance/v1.json"
    ))?;
    let reference = &corpus["reference"];
    let agent_type = reference["agentType"].as_str().unwrap();
    let entries = |layer: &str| {
        reference[layer]
            .as_object()
            .unwrap()
            .iter()
            .map(|(path, value)| AgentConfigEntryDto {
                path: vec![path.clone()],
                value: value.clone().into(),
            })
            .collect()
    };
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;
    let component = executor
        .component_dep(&context.default_environment_id, agent_sdk_ts)
        .with_agent_config(agent_type, entries("deployment"))
        .store()
        .await?;
    let agent_id = agent_id!(agent_type, "captured-defaults");
    let worker_id = executor
        .start_agent_with(
            &component.id,
            agent_id.clone(),
            HashMap::new(),
            entries("creation"),
        )
        .await?;
    let deployed_agent_id = agent_id!(agent_type, "deployment-defaults");
    let reads = [
        (&deployed_agent_id, &reference["expectedDeploymentRead"]),
        (&agent_id, &reference["expectedRead"]),
    ];
    for (id, expected) in reads {
        for method in ["read", "constructorRead"] {
            let actual = executor
                .invoke_and_await_agent(&component, id, method, data_value!())
                .await?
                .into_typed::<String>()?;
            assert_eq!(
                &serde_json::from_str::<serde_json::Value>(&actual)?,
                expected
            );
        }
    }

    executor.shutdown_and_wait_for_invocation_loops().await?;
    drop(executor);
    let executor = start(deps, &context).await?;
    for (id, expected) in reads {
        for method in ["read", "constructorRead"] {
            let actual = executor
                .invoke_and_await_agent(&component, id, method, data_value!())
                .await?
                .into_typed::<String>()?;
            assert_eq!(
                &serde_json::from_str::<serde_json::Value>(&actual)?,
                expected
            );
        }
    }
    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    for entry in &oplog {
        if let PublicOplogEntry::Start(_) = &entry.entry {
            assert!(
                oplog.iter().any(|terminal| matches!(
                    &terminal.entry,
                    PublicOplogEntry::End(end) if end.start_index == entry.oplog_index
                )),
                "config call at {} must have its recorded result",
                entry.oplog_index
            );
        }
    }
    assert_eq!(
        oplog
            .iter()
            .filter(|entry| matches!(&entry.entry, PublicOplogEntry::AgentInvocationFinished(_)))
            .count(),
        5,
    );
    Ok(())
}
