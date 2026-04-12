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

use super::TestContext;
use crate::Tracing;
use anyhow::anyhow;
use assert2::let_assert;
use golem_common::model::worker::AgentConfigEntryDto;
use golem_common::{agent_id, data_value};
use golem_test_framework::config::{EnvBasedTestDependencies, TestDependencies};
use golem_test_framework::dsl::{TestDsl, TestDslExtended};
use golem_wasm::Value;
use pretty_assertions::assert_eq;
use serde_json::json;
use std::sync::Arc;
use test_r::{define_matrix_dimension, inherit_test_dep, test, timeout};

inherit_test_dep!(Tracing);
inherit_test_dep!(EnvBasedTestDependencies);
inherit_test_dep!(
    #[tagged_as("ts")]
    Arc<dyn TestContext>
);
inherit_test_dep!(
    #[tagged_as("rust")]
    Arc<dyn TestContext>
);

define_matrix_dimension!(lang: Arc<dyn TestContext> -> "ts", "rust");

#[test]
#[tracing::instrument]
#[timeout("4m")]
async fn rpc_provided_config_overrides_defaults(
    deps: &EnvBasedTestDependencies,
    #[dimension(lang)] ctx: &Arc<dyn TestContext>,
) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let (_, env) = user.app_and_env().await?;

    let component = user
        .component(&env.id, ctx.test_component_file())
        .name(ctx.test_component_name())
        .with_agent_config("LocalConfigAgent", vec![
            AgentConfigEntryDto { path: vec!["foo".to_string()], value: json!(1).into() },
            AgentConfigEntryDto { path: vec!["bar".to_string()], value: json!("bar").into() },
            AgentConfigEntryDto { path: vec!["nested".to_string(), "a".to_string()], value: json!(true).into() },
            AgentConfigEntryDto { path: vec!["nested".to_string(), "b".to_string()], value: json!([1, 2]).into() },
            AgentConfigEntryDto {
                path: vec![ctx.case_config_path_segment("aliased-nested"), "c".to_string()],
                value: json!(3).into(),
            },
        ])
        .with_agent_config("RpcLocalConfigAgent", vec![
            AgentConfigEntryDto { path: vec!["foo".to_string()], value: json!(2).into() },
        ])
        .store()
        .await?;

    let agent_id = agent_id!("RpcLocalConfigAgent", "test-agent");
    user.start_agent(&component.id, agent_id.clone()).await?;

    let response = user
        .invoke_and_await_agent(
            &component,
            &agent_id,
            ctx.agent_method_name(),
            data_value!(),
        )
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
async fn rpc_can_start_agent_by_providing_config_missing_in_defaults(
    deps: &EnvBasedTestDependencies,
    #[dimension(lang)] ctx: &Arc<dyn TestContext>,
) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let (_, env) = user.app_and_env().await?;

    let component = user
        .component(&env.id, ctx.test_component_file())
        .name(ctx.test_component_name())
        .with_agent_config("LocalConfigAgent", vec![
            AgentConfigEntryDto { path: vec!["bar".to_string()], value: json!("bar").into() },
            AgentConfigEntryDto { path: vec!["nested".to_string(), "a".to_string()], value: json!(true).into() },
            AgentConfigEntryDto { path: vec!["nested".to_string(), "b".to_string()], value: json!([1, 2]).into() },
            AgentConfigEntryDto {
                path: vec![ctx.case_config_path_segment("aliased-nested"), "c".to_string()],
                value: json!(3).into(),
            },
        ])
        .with_agent_config("RpcLocalConfigAgent", vec![
            AgentConfigEntryDto { path: vec!["foo".to_string()], value: json!(2).into() },
        ])
        .store()
        .await?;

    let agent_id = agent_id!("RpcLocalConfigAgent", "test-agent");
    user.start_agent(&component.id, agent_id.clone()).await?;

    let response = user
        .invoke_and_await_agent(
            &component,
            &agent_id,
            ctx.agent_method_name(),
            data_value!(),
        )
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
async fn rpc_does_not_override_values_of_existing_agent(
    deps: &EnvBasedTestDependencies,
    #[dimension(lang)] ctx: &Arc<dyn TestContext>,
) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let (_, env) = user.app_and_env().await?;

    let component = user
        .component(&env.id, ctx.test_component_file())
        .name(ctx.test_component_name())
        .with_agent_config("LocalConfigAgent", vec![
            AgentConfigEntryDto { path: vec!["foo".to_string()], value: json!(1).into() },
            AgentConfigEntryDto { path: vec!["bar".to_string()], value: json!("bar").into() },
            AgentConfigEntryDto { path: vec!["nested".to_string(), "a".to_string()], value: json!(true).into() },
            AgentConfigEntryDto { path: vec!["nested".to_string(), "b".to_string()], value: json!([1, 2]).into() },
            AgentConfigEntryDto {
                path: vec![ctx.case_config_path_segment("aliased-nested"), "c".to_string()],
                value: json!(3).into(),
            },
        ])
        .with_agent_config("RpcLocalConfigAgent", vec![
            AgentConfigEntryDto { path: vec!["foo".to_string()], value: json!(2).into() },
        ])
        .store()
        .await?;

    user.start_agent(&component.id, agent_id!("LocalConfigAgent", "test-agent"))
        .await?;

    let agent_id = agent_id!("RpcLocalConfigAgent", "test-agent");
    user.start_agent(&component.id, agent_id.clone()).await?;

    let response = user
        .invoke_and_await_agent(
            &component,
            &agent_id,
            ctx.agent_method_name(),
            data_value!(),
        )
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
