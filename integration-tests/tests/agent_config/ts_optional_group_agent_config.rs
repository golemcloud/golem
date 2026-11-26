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
use golem_client::api::AgentError;
use golem_common::model::agent::AgentConfigEntryDto;
use golem_common::schema::SchemaValue;
use golem_common::{agent_key, data_value};
use golem_test_framework::config::{EnvBasedTestDependencies, TestDependencies};
use golem_test_framework::dsl::{TestDsl, TestDslExtended};
use pretty_assertions::assert_eq;
use pretty_assertions::assert_matches;
use serde_json::json;
use std::collections::HashMap;
use std::sync::Arc;
use test_r::{inherit_test_dep, test, timeout};

inherit_test_dep!(Tracing);
inherit_test_dep!(EnvBasedTestDependencies);
inherit_test_dep!(
    #[tagged_as("ts")]
    Arc<dyn TestContext>
);

#[test]
#[tracing::instrument]
#[timeout("8m")]
async fn optional_group_present_with_all_fields(
    deps: &EnvBasedTestDependencies,
    #[tagged_as("ts")] ctx: &Arc<dyn TestContext>,
) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let (_, env) = user.app_and_env().await?;

    let component = user
        .component(&env.id, ctx.test_component_file())
        .name(ctx.test_component_name())
        .store()
        .await?;

    let agent_id = agent_key!("OptionalGroupConfigAgent", "test-agent");
    user.start_agent_with(
        &component.id,
        agent_id.clone(),
        HashMap::new(),
        vec![
            AgentConfigEntryDto {
                path: vec!["required".to_string()],
                value: json!("hello").into(),
            },
            AgentConfigEntryDto {
                path: vec![ctx.case_config_path_segment("optional-group")],
                value: json!({ "a": 42, "b": "world" }).into(),
            },
        ],
    )
    .await?;

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

    let_assert!(SchemaValue::String(config) = response);
    let parsed: serde_json::Value = serde_json::from_str(&config)?;

    assert_eq!(
        parsed,
        json!({
            "required": "hello",
            "optionalGroup": {
                "a": 42,
                "b": "world"
            }
        })
    );

    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("8m")]
async fn optional_group_present_with_required_field_only(
    deps: &EnvBasedTestDependencies,
    #[tagged_as("ts")] ctx: &Arc<dyn TestContext>,
) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let (_, env) = user.app_and_env().await?;

    let component = user
        .component(&env.id, ctx.test_component_file())
        .name(ctx.test_component_name())
        .store()
        .await?;

    let agent_id = agent_key!("OptionalGroupConfigAgent", "test-agent");
    user.start_agent_with(
        &component.id,
        agent_id.clone(),
        HashMap::new(),
        vec![
            AgentConfigEntryDto {
                path: vec!["required".to_string()],
                value: json!("hello").into(),
            },
            AgentConfigEntryDto {
                path: vec![ctx.case_config_path_segment("optional-group")],
                value: json!({ "a": 42 }).into(),
            },
        ],
    )
    .await?;

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

    let_assert!(SchemaValue::String(config) = response);
    let parsed: serde_json::Value = serde_json::from_str(&config)?;

    assert_eq!(
        parsed,
        json!({
            "required": "hello",
            "optionalGroup": {
                "a": 42
            }
        })
    );

    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("8m")]
async fn all_optional_group_is_absent_when_not_provided(
    deps: &EnvBasedTestDependencies,
    #[tagged_as("ts")] ctx: &Arc<dyn TestContext>,
) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let (_, env) = user.app_and_env().await?;

    let component = user
        .component(&env.id, ctx.test_component_file())
        .name(ctx.test_component_name())
        .store()
        .await?;

    let agent_id = agent_key!("AllOptionalGroupConfigAgent", "test-agent");
    user.start_agent_with(&component.id, agent_id.clone(), HashMap::new(), vec![])
        .await?;

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

    let_assert!(SchemaValue::String(config) = response);
    let parsed: serde_json::Value = serde_json::from_str(&config)?;

    assert_eq!(parsed, json!({}));

    Ok(())
}

// An optional group with only optional children is present with the provided child.
#[test]
#[tracing::instrument]
#[timeout("8m")]
async fn all_optional_group_present_when_child_provided(
    deps: &EnvBasedTestDependencies,
    #[tagged_as("ts")] ctx: &Arc<dyn TestContext>,
) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let (_, env) = user.app_and_env().await?;

    let component = user
        .component(&env.id, ctx.test_component_file())
        .name(ctx.test_component_name())
        .store()
        .await?;

    let agent_id = agent_key!("AllOptionalGroupConfigAgent", "test-agent");
    user.start_agent_with(
        &component.id,
        agent_id.clone(),
        HashMap::new(),
        vec![AgentConfigEntryDto {
            path: vec![ctx.case_config_path_segment("all-optional-group")],
            value: json!({ "x": 7 }).into(),
        }],
    )
    .await?;

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

    let_assert!(SchemaValue::String(config) = response);
    let parsed: serde_json::Value = serde_json::from_str(&config)?;

    assert_eq!(parsed, json!({ "allOptionalGroup": { "x": 7 } }));

    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("8m")]
async fn optional_group_rejects_a_record_missing_a_required_nested_field(
    deps: &EnvBasedTestDependencies,
    #[tagged_as("ts")] ctx: &Arc<dyn TestContext>,
) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let (_, env) = user.app_and_env().await?;

    let component = user
        .component(&env.id, ctx.test_component_file())
        .name(ctx.test_component_name())
        .store()
        .await?;

    let agent_id = agent_key!("NestedRequiredGroupConfigAgent", "test-agent");
    let result = user
        .try_start_agent_with(
            &component.id,
            agent_id.clone(),
            HashMap::new(),
            vec![AgentConfigEntryDto {
                path: vec![ctx.case_config_path_segment("outer")],
                value: json!({ "required": "hello" }).into(),
            }],
        )
        .await?;

    assert_matches!(
        result,
        Err(golem_client::Error::Item(AgentError::Error400(_)))
    );

    Ok(())
}

// An optional group with a required nested object: present when all required fields are provided.
#[test]
#[tracing::instrument]
#[timeout("8m")]
async fn optional_group_present_when_required_nested_child_provided(
    deps: &EnvBasedTestDependencies,
    #[tagged_as("ts")] ctx: &Arc<dyn TestContext>,
) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let (_, env) = user.app_and_env().await?;

    let component = user
        .component(&env.id, ctx.test_component_file())
        .name(ctx.test_component_name())
        .store()
        .await?;

    let agent_id = agent_key!("NestedRequiredGroupConfigAgent", "test-agent");
    user.start_agent_with(
        &component.id,
        agent_id.clone(),
        HashMap::new(),
        vec![AgentConfigEntryDto {
            path: vec![ctx.case_config_path_segment("outer")],
            value: json!({ "required": "hello", "inner": { "a": 99 } }).into(),
        }],
    )
    .await?;

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

    let_assert!(SchemaValue::String(config) = response);
    let parsed: serde_json::Value = serde_json::from_str(&config)?;

    assert_eq!(
        parsed,
        json!({
            "outer": {
                "required": "hello",
                "inner": { "a": 99 }
            }
        })
    );

    Ok(())
}
