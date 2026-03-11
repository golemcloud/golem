// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
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
use anyhow::anyhow;
use assert2::let_assert;
use golem_client::api::WorkerError;
use golem_common::model::agent::AgentTypeName;
use golem_common::model::component::LocalAgentConfigEntry;
use golem_common::model::worker::WorkerCreationLocalAgentConfigEntry;
use golem_common::{agent_id, data_value};
use golem_test_framework::config::{EnvBasedTestDependencies, TestDependencies};
use golem_test_framework::dsl::{TestDsl, TestDslExtended};
use golem_wasm::Value;
use pretty_assertions::assert_eq;
use pretty_assertions::assert_matches;
use serde_json::json;
use std::collections::HashMap;
use test_r::{inherit_test_dep, test, timeout};

inherit_test_dep!(Tracing);
inherit_test_dep!(EnvBasedTestDependencies);

#[test]
#[tracing::instrument]
#[timeout("4m")]
async fn agent_with_only_component_local_agent_config(
    deps: &EnvBasedTestDependencies,
) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let (_, env) = user.app_and_env().await?;

    let component = user
        .component(&env.id, "golem_it_agent_sdk_ts")
        .name("golem-it:agent-sdk-ts")
        .with_local_agent_config(vec![
            LocalAgentConfigEntry {
                agent: AgentTypeName("ConfigAgent".to_string()),
                key: vec!["foo".to_string()],
                value: json!(1),
            },
            LocalAgentConfigEntry {
                agent: AgentTypeName("ConfigAgent".to_string()),
                key: vec!["bar".to_string()],
                value: json!("bar"),
            },
            LocalAgentConfigEntry {
                agent: AgentTypeName("ConfigAgent".to_string()),
                key: vec!["nested".to_string(), "a".to_string()],
                value: json!(true),
            },
            LocalAgentConfigEntry {
                agent: AgentTypeName("ConfigAgent".to_string()),
                key: vec!["nested".to_string(), "b".to_string()],
                value: json!([1, 2]),
            },
            LocalAgentConfigEntry {
                agent: AgentTypeName("ConfigAgent".to_string()),
                key: vec!["aliasedNested".to_string(), "c".to_string()],
                value: json!(3),
            },
        ])
        .store()
        .await?;

    let agent_id = agent_id!("ConfigAgent", "test-agent");
    user.start_agent(&component.id, agent_id.clone()).await?;

    let response = user
        .invoke_and_await_agent(&component, &agent_id, "echoLocalConfig", data_value!())
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected return value"))?;

    let_assert!(Value::String(agent_config) = response);

    let parsed_agent_config: serde_json::Value = serde_json::from_str(&agent_config)?;

    assert_eq!(
        parsed_agent_config,
        json!({
            "foo": 1,
            "bar": "bar",
            "nested": {
                "a": true,
                "b": [1, 2]
            },
            "aliasedNested": {
                "c": 3
            }
        })
    );

    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("4m")]
async fn agent_with_only_worker_local_agent_config(
    deps: &EnvBasedTestDependencies,
) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let (_, env) = user.app_and_env().await?;

    let component = user
        .component(&env.id, "golem_it_agent_sdk_ts")
        .name("golem-it:agent-sdk-ts")
        .store()
        .await?;

    let agent_id = agent_id!("ConfigAgent", "test-agent");
    user.start_agent_with(
        &component.id,
        agent_id.clone(),
        HashMap::new(),
        HashMap::new(),
        vec![
            WorkerCreationLocalAgentConfigEntry {
                key: vec!["foo".to_string()],
                value: json!(1),
            },
            WorkerCreationLocalAgentConfigEntry {
                key: vec!["bar".to_string()],
                value: json!("bar"),
            },
            WorkerCreationLocalAgentConfigEntry {
                key: vec!["nested".to_string(), "a".to_string()],
                value: json!(true),
            },
            WorkerCreationLocalAgentConfigEntry {
                key: vec!["nested".to_string(), "b".to_string()],
                value: json!([1, 2]),
            },
            WorkerCreationLocalAgentConfigEntry {
                key: vec!["aliasedNested".to_string(), "c".to_string()],
                value: json!(3),
            },
        ],
    )
    .await?;

    let response = user
        .invoke_and_await_agent(&component, &agent_id, "echoLocalConfig", data_value!())
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected return value"))?;

    let_assert!(Value::String(agent_config) = response);

    let parsed_agent_config: serde_json::Value = serde_json::from_str(&agent_config)?;

    assert_eq!(
        parsed_agent_config,
        json!({
            "foo": 1,
            "bar": "bar",
            "nested": {
                "a": true,
                "b": [1, 2]
            },
            "aliasedNested": {
                "c": 3
            }
        })
    );

    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("4m")]
async fn agent_with_mixed_local_agent_config(
    deps: &EnvBasedTestDependencies,
) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let (_, env) = user.app_and_env().await?;

    let component = user
        .component(&env.id, "golem_it_agent_sdk_ts")
        .name("golem-it:agent-sdk-ts")
        .with_local_agent_config(vec![
            LocalAgentConfigEntry {
                agent: AgentTypeName("ConfigAgent".to_string()),
                key: vec!["foo".to_string()],
                value: json!(1),
            },
            LocalAgentConfigEntry {
                agent: AgentTypeName("ConfigAgent".to_string()),
                key: vec!["bar".to_string()],
                value: json!("bar"),
            },
        ])
        .store()
        .await?;

    let agent_id = agent_id!("ConfigAgent", "test-agent");
    user.start_agent_with(
        &component.id,
        agent_id.clone(),
        HashMap::new(),
        HashMap::new(),
        vec![
            WorkerCreationLocalAgentConfigEntry {
                key: vec!["foo".to_string()],
                value: json!(2),
            },
            WorkerCreationLocalAgentConfigEntry {
                key: vec!["nested".to_string(), "a".to_string()],
                value: json!(true),
            },
            WorkerCreationLocalAgentConfigEntry {
                key: vec!["nested".to_string(), "b".to_string()],
                value: json!([1, 2]),
            },
            WorkerCreationLocalAgentConfigEntry {
                key: vec!["aliasedNested".to_string(), "c".to_string()],
                value: json!(3),
            },
        ],
    )
    .await?;

    let response = user
        .invoke_and_await_agent(&component, &agent_id, "echoLocalConfig", data_value!())
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected return value"))?;

    let_assert!(Value::String(agent_config) = response);

    let parsed_agent_config: serde_json::Value = serde_json::from_str(&agent_config)?;

    assert_eq!(
        parsed_agent_config,
        json!({
            "foo": 2,
            "bar": "bar",
            "nested": {
                "a": true,
                "b": [1, 2]
            },
            "aliasedNested": {
                "c": 3
            }
        })
    );

    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("4m")]
async fn agent_with_mixed_local_agent_config_update(
    deps: &EnvBasedTestDependencies,
) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let (_, env) = user.app_and_env().await?;

    let component = user
        .component(&env.id, "golem_it_agent_sdk_ts")
        .name("golem-it:agent-sdk-ts")
        .with_local_agent_config(vec![
            LocalAgentConfigEntry {
                agent: AgentTypeName("ConfigAgent".to_string()),
                key: vec!["foo".to_string()],
                value: json!(1),
            },
            LocalAgentConfigEntry {
                agent: AgentTypeName("ConfigAgent".to_string()),
                key: vec!["bar".to_string()],
                value: json!("bar"),
            },
        ])
        .store()
        .await?;

    let agent_id = agent_id!("ConfigAgent", "test-agent");

    let worker_id = user
        .start_agent_with(
            &component.id,
            agent_id.clone(),
            HashMap::new(),
            HashMap::new(),
            vec![
                WorkerCreationLocalAgentConfigEntry {
                    key: vec!["foo".to_string()],
                    value: json!(2),
                },
                WorkerCreationLocalAgentConfigEntry {
                    key: vec!["nested".to_string(), "a".to_string()],
                    value: json!(true),
                },
                WorkerCreationLocalAgentConfigEntry {
                    key: vec!["nested".to_string(), "b".to_string()],
                    value: json!([1, 2]),
                },
                WorkerCreationLocalAgentConfigEntry {
                    key: vec!["aliasedNested".to_string(), "c".to_string()],
                    value: json!(3),
                },
            ],
        )
        .await?;

    let updated_component = user
        .update_component_with(
            &component.id,
            component.revision,
            None,
            Vec::new(),
            Vec::new(),
            None,
            None,
            Some(vec![
                LocalAgentConfigEntry {
                    agent: AgentTypeName("ConfigAgent".to_string()),
                    key: vec!["foo".to_string()],
                    value: json!(3),
                },
                LocalAgentConfigEntry {
                    agent: AgentTypeName("ConfigAgent".to_string()),
                    key: vec!["bar".to_string()],
                    value: json!("baz"),
                },
            ]),
            Vec::new(),
        )
        .await?;

    user.auto_update_worker(&worker_id, updated_component.revision, false)
        .await?;

    let response = user
        .invoke_and_await_agent(&component, &agent_id, "echoLocalConfig", data_value!())
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected return value"))?;

    let_assert!(Value::String(agent_config) = response);

    let parsed_agent_config: serde_json::Value = serde_json::from_str(&agent_config)?;

    assert_eq!(
        parsed_agent_config,
        json!({
            "foo": 2,
            "bar": "baz",
            "nested": {
                "a": true,
                "b": [1, 2]
            },
            "aliasedNested": {
                "c": 3
            }
        })
    );

    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("4m")]
async fn missing_local_agent_config_key(deps: &EnvBasedTestDependencies) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let (_, env) = user.app_and_env().await?;

    let component = user
        .component(&env.id, "golem_it_agent_sdk_ts")
        .name("golem-it:agent-sdk-ts")
        .store()
        .await?;

    let agent_id = agent_id!("ConfigAgent", "test-agent");

    let result = user
        .try_start_agent_with(
            &component.id,
            agent_id.clone(),
            HashMap::new(),
            HashMap::new(),
            vec![
                WorkerCreationLocalAgentConfigEntry {
                    key: vec!["bar".to_string()],
                    value: json!("bar"),
                },
                WorkerCreationLocalAgentConfigEntry {
                    key: vec!["nested".to_string(), "a".to_string()],
                    value: json!(true),
                },
                WorkerCreationLocalAgentConfigEntry {
                    key: vec!["nested".to_string(), "b".to_string()],
                    value: json!([1, 2]),
                },
                WorkerCreationLocalAgentConfigEntry {
                    key: vec!["aliasedNested".to_string(), "c".to_string()],
                    value: json!(3),
                },
            ],
        )
        .await?;

    // TODO: this should be 400 / 409
    assert_matches!(
        result,
        Err(golem_client::Error::Item(WorkerError::Error500(_)))
    );

    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("4m")]
async fn mistyped_local_agent_config_key(deps: &EnvBasedTestDependencies) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let (_, env) = user.app_and_env().await?;

    let component = user
        .component(&env.id, "golem_it_agent_sdk_ts")
        .name("golem-it:agent-sdk-ts")
        .store()
        .await?;

    let agent_id = agent_id!("ConfigAgent", "test-agent");

    let result = user
        .try_start_agent_with(
            &component.id,
            agent_id.clone(),
            HashMap::new(),
            HashMap::new(),
            vec![
                WorkerCreationLocalAgentConfigEntry {
                    key: vec!["foo".to_string()],
                    value: json!("foo"),
                },
                WorkerCreationLocalAgentConfigEntry {
                    key: vec!["bar".to_string()],
                    value: json!("bar"),
                },
                WorkerCreationLocalAgentConfigEntry {
                    key: vec!["nested".to_string(), "a".to_string()],
                    value: json!(true),
                },
                WorkerCreationLocalAgentConfigEntry {
                    key: vec!["nested".to_string(), "b".to_string()],
                    value: json!([1, 2]),
                },
                WorkerCreationLocalAgentConfigEntry {
                    key: vec!["aliasedNested".to_string(), "c".to_string()],
                    value: json!(3),
                },
            ],
        )
        .await?;

    // TODO: this should be 400 / 409
    assert_matches!(
        result,
        Err(golem_client::Error::Item(WorkerError::Error500(_)))
    );

    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("4m")]
async fn optional_local_agent_config_does_not_need_to_be_provided(
    deps: &EnvBasedTestDependencies,
) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let (_, env) = user.app_and_env().await?;

    let component = user
        .component(&env.id, "golem_it_agent_sdk_ts")
        .name("golem-it:agent-sdk-ts")
        .store()
        .await?;

    let agent_id = agent_id!("ConfigAgent", "test-agent");
    user.start_agent_with(
        &component.id,
        agent_id.clone(),
        HashMap::new(),
        HashMap::new(),
        vec![
            WorkerCreationLocalAgentConfigEntry {
                key: vec!["foo".to_string()],
                value: json!(1),
            },
            WorkerCreationLocalAgentConfigEntry {
                key: vec!["bar".to_string()],
                value: json!("bar"),
            },
            WorkerCreationLocalAgentConfigEntry {
                key: vec!["nested".to_string(), "a".to_string()],
                value: json!(true),
            },
            WorkerCreationLocalAgentConfigEntry {
                key: vec!["nested".to_string(), "b".to_string()],
                value: json!([1, 2]),
            },
        ],
    )
    .await?;

    let response = user
        .invoke_and_await_agent(&component, &agent_id, "echoLocalConfig", data_value!())
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected return value"))?;

    let_assert!(Value::String(agent_config) = response);

    let parsed_agent_config: serde_json::Value = serde_json::from_str(&agent_config)?;

    assert_eq!(
        parsed_agent_config,
        json!({
            "foo": 1,
            "bar": "bar",
            "nested": {
                "a": true,
                "b": [1, 2]
            },
            "aliasedNested": { }
        })
    );

    Ok(())
}
