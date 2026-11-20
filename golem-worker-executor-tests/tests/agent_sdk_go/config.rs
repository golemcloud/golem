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

//! Agent config for the Go SDK: values provided at deploy (a flat key and a
//! nested path) are resolved and read back by a configured agent at runtime.

use crate::Tracing;
use golem_common::model::agent::{AgentConfigEntryDto, AgentTypeName};
use golem_common::model::component::AgentTypeProvisionConfigUpdate;
use golem_common::model::oplog::{OplogIndex, PublicOplogEntry};
use golem_common::{agent_id, data_value};
use golem_test_framework::dsl::TestDsl;
use golem_worker_executor_test_utils::{
    LastUniqueId, PrecompiledComponent, TestContext, WorkerExecutorTestDependencies, start,
};
use std::collections::{BTreeMap, HashMap};
use test_r::{inherit_test_dep, test, timeout};

inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(LastUniqueId);
inherit_test_dep!(Tracing);
inherit_test_dep!(
    #[tagged_as("agent_sdk_go")]
    PrecompiledComponent
);

/// A configured Go agent reads a flat config value ("greeting") and a nested one
/// ("fee"/"cents") set at deploy time. "cents" is declared as an s64, so its
/// deploy-time value is a canonical base-10 string rather than a JSON number.
#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn go_agent_config_read(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("agent_sdk_go")] agent_sdk_go: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component_dep(&context.default_environment_id, agent_sdk_go)
        .with_agent_config(
            "ConfigAgent",
            vec![
                AgentConfigEntryDto {
                    path: vec!["greeting".to_string()],
                    value: serde_json::Value::String("hello".to_string()).into(),
                },
                AgentConfigEntryDto {
                    path: vec!["fee".to_string(), "cents".to_string()],
                    value: serde_json::json!("30").into(),
                },
            ],
        )
        .store()
        .await?;

    let agent_id = agent_id!("ConfigAgent", "go-config-1");
    executor
        .start_agent_with(&component.id, agent_id.clone(), HashMap::new(), Vec::new())
        .await?;

    let greeting = executor
        .invoke_and_await_agent(&component, &agent_id, "greeting", data_value!())
        .await?
        .into_typed::<String>()?;
    assert_eq!(greeting, "hello");

    let cents = executor
        .invoke_and_await_agent(&component, &agent_id, "cents", data_value!())
        .await?
        .into_typed::<i64>()?;
    assert_eq!(cents, 30);

    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn go_static_config_defaults_survive_reconstruction(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("agent_sdk_go")] agent_sdk_go: &PrecompiledComponent,
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
    let mut component = executor
        .component_dep(&context.default_environment_id, agent_sdk_go)
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
    let deployed_worker_id = executor
        .start_agent(&component.id, deployed_agent_id.clone())
        .await?;
    let reads = [
        (&deployed_agent_id, &reference["expectedDeploymentRead"]),
        (&agent_id, &reference["expectedRead"]),
    ];
    for (id, expected) in reads {
        for method in ["read", "constructor-read"] {
            let actual = executor
                .invoke_and_await_agent(&component, id, method, data_value!())
                .await?
                .into_typed::<String>()?;
            assert_eq!(
                &serde_json::from_str::<serde_json::Value>(&actual)?,
                expected,
                "{id}: {method}",
            );
        }
    }

    executor.shutdown_and_wait_for_invocation_loops().await?;
    drop(executor);
    let executor = start(deps, &context).await?;
    for (id, expected) in reads {
        for method in ["read", "constructor-read"] {
            let actual = executor
                .invoke_and_await_agent(&component, id, method, data_value!())
                .await?
                .into_typed::<String>()?;
            assert_eq!(
                &serde_json::from_str::<serde_json::Value>(&actual)?,
                expected,
                "{id}: {method}",
            );
        }
    }
    let mut current_expected = reference["expectedDeploymentRead"].clone();
    for (label, count, config) in [
        (
            "new-revision",
            0,
            Some(vec![AgentConfigEntryDto {
                path: vec!["settings".into()],
                value: serde_json::json!({"label": "new-revision", "count": 0}).into(),
            }]),
        ),
        ("new-revision", 0, None),
        ("code-default", 31, Some(vec![])),
    ] {
        component = executor
            .update_component_with(
                &component.id,
                component.revision,
                None,
                Some(BTreeMap::from([(
                    AgentTypeName(agent_type.into()),
                    AgentTypeProvisionConfigUpdate {
                        config,
                        ..Default::default()
                    },
                )])),
                vec![],
            )
            .await?;
        current_expected["settings"] = serde_json::json!({"label": label, "count": count});
        for ((id, expected), worker) in [
            (&deployed_agent_id, &current_expected),
            (&agent_id, &reference["expectedRead"]),
        ]
        .into_iter()
        .zip([&deployed_worker_id, &worker_id])
        {
            executor
                .auto_update_worker(worker, component.revision, false)
                .await?;
            let actual = executor
                .invoke_and_await_agent(&component, id, "read", data_value!())
                .await?
                .into_typed::<String>()?;
            assert_eq!(
                &serde_json::from_str::<serde_json::Value>(&actual)?,
                expected,
                "{id}: revision {}",
                component.revision,
            );
            assert_eq!(
                executor
                    .get_worker_metadata(worker)
                    .await?
                    .component_revision,
                component.revision,
            );
        }
    }
    executor.shutdown_and_wait_for_invocation_loops().await?;
    drop(executor);
    let executor = start(deps, &context).await?;
    for (id, expected) in [
        (&deployed_agent_id, &current_expected),
        (&agent_id, &reference["expectedRead"]),
    ] {
        let actual = executor
            .invoke_and_await_agent(&component, id, "read", data_value!())
            .await?
            .into_typed::<String>()?;
        assert_eq!(
            &serde_json::from_str::<serde_json::Value>(&actual)?,
            expected
        );
    }
    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    for entry in &oplog {
        if let PublicOplogEntry::Start(_) = &entry.entry {
            assert!(oplog.iter().any(|terminal| matches!(
                &terminal.entry,
                PublicOplogEntry::End(end) if end.start_index == entry.oplog_index
            )));
        }
    }
    assert_eq!(
        oplog
            .iter()
            .filter(|entry| matches!(&entry.entry, PublicOplogEntry::AgentInvocationFinished(_)))
            .count(),
        9,
    );
    Ok(())
}
