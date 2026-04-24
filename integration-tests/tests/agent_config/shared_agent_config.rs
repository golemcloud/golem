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
use golem_client::api::{RegistryServiceClient, WorkerError};
use golem_client::model::AgentSecretCreation;
use golem_common::model::agent_secret::AgentSecretPath;
use golem_common::model::deployment::DeploymentAgentSecretDefault;
use golem_common::{agent_id, data_value};
use golem_test_framework::config::{EnvBasedTestDependencies, TestDependencies};
use golem_test_framework::dsl::{TestDsl, TestDslExtended};
use golem_wasm::Value;
use golem_wasm::analysis::analysed_type;
use pretty_assertions::assert_eq;
use pretty_assertions::assert_matches;
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
async fn agent_reads_secret_created_from_default(
    deps: &EnvBasedTestDependencies,
    #[dimension(lang)] ctx: &Arc<dyn TestContext>,
) -> anyhow::Result<()> {
    let user = deps.user().await?.with_auto_deploy(false);
    let (_, env) = user.app_and_env().await?;

    let component = user
        .component(&env.id, ctx.test_component_file())
        .name(ctx.test_component_name())
        .store()
        .await?;

    user.deploy_environment_with(env.id, |d| {
        d.agent_secret_defaults = vec![
            DeploymentAgentSecretDefault {
                path: AgentSecretPath(vec!["secret".into()]),
                secret_value: json!("foo"),
            },
            DeploymentAgentSecretDefault {
                path: AgentSecretPath(vec![ctx.case_config_path_segment("complex-secret")]),
                secret_value: json!({
                    "foo": "foo",
                    "bar": 1,
                }),
            },
        ];
    })
    .await?;

    let agent_id = agent_id!("SharedConfigAgent", "test-agent");

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

    let_assert!(Value::String(config) = response);

    let parsed: serde_json::Value = serde_json::from_str(&config)?;

    assert_eq!(
        parsed,
        json!({
            "secret": "foo",
            "complexSecret": {
                "foo": "foo",
                "bar": 1,
            }
        })
    );

    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("4m")]
async fn agent_reads_secret_updated_from_default(
    deps: &EnvBasedTestDependencies,
    #[dimension(lang)] ctx: &Arc<dyn TestContext>,
) -> anyhow::Result<()> {
    let user = deps.user().await?.with_auto_deploy(false);
    let (_, env) = user.app_and_env().await?;
    let client = deps.registry_service().client(&user.token).await;

    let component = user
        .component(&env.id, ctx.test_component_file())
        .name(ctx.test_component_name())
        .store()
        .await?;

    client
        .create_agent_secret(
            &env.id.0,
            &AgentSecretCreation {
                path: AgentSecretPath(vec!["secret".into()]),
                secret_type: analysed_type::str(),
                secret_value: None,
            },
        )
        .await?;

    user.deploy_environment_with(env.id, |d| {
        d.agent_secret_defaults = vec![
            DeploymentAgentSecretDefault {
                path: AgentSecretPath(vec!["secret".into()]),
                secret_value: json!("foo"),
            },
            DeploymentAgentSecretDefault {
                path: AgentSecretPath(vec![ctx.case_config_path_segment("complex-secret")]),
                secret_value: json!({
                    "foo": "foo",
                    "bar": 1,
                }),
            },
        ];
    })
    .await?;

    let agent_id = agent_id!("SharedConfigAgent", "test-agent");

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

    let_assert!(Value::String(config) = response);

    let parsed: serde_json::Value = serde_json::from_str(&config)?;

    assert_eq!(
        parsed,
        json!({
            "secret": "foo",
            "complexSecret": {
                "foo": "foo",
                "bar": 1,
            }
        })
    );

    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("4m")]
async fn agent_fails_on_missing_environment_secret_value(
    deps: &EnvBasedTestDependencies,
    #[dimension(lang)] ctx: &Arc<dyn TestContext>,
) -> anyhow::Result<()> {
    let user = deps.user().await?.with_auto_deploy(false);
    let (_, env) = user.app_and_env().await?;

    let component = user
        .component(&env.id, ctx.test_component_file())
        .name(ctx.test_component_name())
        .store()
        .await?;

    user.deploy_environment(env.id).await?;

    let agent_id = agent_id!("SharedConfigAgent", "test-agent");

    let response = user
        .try_start_agent(&component.id, agent_id.clone())
        .await?;

    assert_matches!(
        response,
        Err(golem_client::Error::Item(WorkerError::Error500(_)))
    );

    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("4m")]
async fn agent_reads_secret_with_different_casing(
    deps: &EnvBasedTestDependencies,
    #[dimension(lang)] ctx: &Arc<dyn TestContext>,
) -> anyhow::Result<()> {
    let user = deps.user().await?.with_auto_deploy(false);
    let (_, env) = user.app_and_env().await?;

    let component = user
        .component(&env.id, ctx.test_component_file())
        .name(ctx.test_component_name())
        .store()
        .await?;

    user.deploy_environment_with(env.id, |d| {
        d.agent_secret_defaults = vec![DeploymentAgentSecretDefault {
            path: AgentSecretPath(vec!["secret_path".into()]),
            secret_value: json!("foo"),
        }];
    })
    .await?;

    let agent_id = agent_id!("LocalCasingSharedConfigAgent", "test-agent");

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

    let_assert!(Value::String(config) = response);

    let parsed: serde_json::Value = serde_json::from_str(&config)?;

    assert_eq!(
        parsed,
        json!({
            "secretPath": "foo"
        })
    );

    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("4m")]
async fn agent_reads_secret_with_mixed_case_path(
    deps: &EnvBasedTestDependencies,
    #[dimension(lang)] ctx: &Arc<dyn TestContext>,
) -> anyhow::Result<()> {
    let user = deps.user().await?.with_auto_deploy(false);
    let (_, env) = user.app_and_env().await?;

    let component = user
        .component(&env.id, ctx.test_component_file())
        .name(ctx.test_component_name())
        .store()
        .await?;

    user.deploy_environment_with(env.id, |d| {
        d.agent_secret_defaults = vec![DeploymentAgentSecretDefault {
            path: AgentSecretPath(vec!["SecretPath".into()]),
            secret_value: json!("foo"),
        }];
    })
    .await?;

    let agent_id = agent_id!("LocalCasingSharedConfigAgent", "test-agent");

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

    let_assert!(Value::String(config) = response);

    let parsed: serde_json::Value = serde_json::from_str(&config)?;

    assert_eq!(
        parsed,
        json!({
            "secretPath": "foo"
        })
    );

    Ok(())
}
