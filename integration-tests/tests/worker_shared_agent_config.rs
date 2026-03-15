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
use golem_client::api::{AgentError, RegistryServiceClient, WorkerError};
use golem_client::model::AgentSecretCreation;
use golem_common::model::agent_secret::AgentSecretUpdate;
use golem_common::model::deployment::DeploymentAgentSecretDefault;
use golem_common::model::optional_field_update::OptionalFieldUpdate;
use golem_common::{agent_id, data_value};
use golem_test_framework::config::{EnvBasedTestDependencies, TestDependencies};
use golem_test_framework::dsl::{TestDsl, TestDslExtended};
use golem_wasm::analysis::analysed_type;
use golem_wasm::Value;
use pretty_assertions::assert_eq;
use pretty_assertions::assert_matches;
use serde_json::json;
use test_r::{inherit_test_dep, test, timeout};

inherit_test_dep!(Tracing);
inherit_test_dep!(EnvBasedTestDependencies);

#[test]
#[tracing::instrument]
#[timeout("4m")]
async fn agent_reads_secret_created_from_default(
    deps: &EnvBasedTestDependencies,
) -> anyhow::Result<()> {
    let user = deps.user().await?.with_auto_deploy(false);
    let (_, env) = user.app_and_env().await?;

    let component = user
        .component(&env.id, "golem_it_agent_sdk_ts")
        .name("golem-it:agent-sdk-ts")
        .store()
        .await?;

    user.deploy_environment_with(
        env.id,
        vec![DeploymentAgentSecretDefault {
            path: vec!["secret".into()],
            secret_value: json!("foo"),
        }],
    )
    .await?;

    let agent_id = agent_id!("SharedConfigAgent", "test-agent");

    user.start_agent(&component.id, agent_id.clone()).await?;

    let response = user
        .invoke_and_await_agent(&component, &agent_id, "echoLocalConfig", data_value!())
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected return value"))?;

    let_assert!(Value::String(config) = response);

    let parsed: serde_json::Value = serde_json::from_str(&config)?;

    assert_eq!(
        parsed,
        json!({
            "secret": "foo"
        })
    );

    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("4m")]
async fn agent_reads_secret_updated_from_default(
    deps: &EnvBasedTestDependencies,
) -> anyhow::Result<()> {
    let user = deps.user().await?.with_auto_deploy(false);
    let (_, env) = user.app_and_env().await?;
    let client = deps.registry_service().client(&user.token).await;

    let component = user
        .component(&env.id, "golem_it_agent_sdk_ts")
        .name("golem-it:agent-sdk-ts")
        .store()
        .await?;

    client
        .create_agent_secret(
            &env.id.0,
            &AgentSecretCreation {
                path: vec!["secret".into()],
                secret_type: analysed_type::str(),
                secret_value: None,
            },
        )
        .await?;

    user.deploy_environment_with(
        env.id,
        vec![DeploymentAgentSecretDefault {
            path: vec!["secret".into()],
            secret_value: json!("foo"),
        }],
    )
    .await?;

    let agent_id = agent_id!("SharedConfigAgent", "test-agent");

    user.start_agent(&component.id, agent_id.clone()).await?;

    let response = user
        .invoke_and_await_agent(&component, &agent_id, "echoLocalConfig", data_value!())
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected return value"))?;

    let_assert!(Value::String(config) = response);

    let parsed: serde_json::Value = serde_json::from_str(&config)?;

    assert_eq!(
        parsed,
        json!({
            "secret": "foo"
        })
    );

    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("4m")]
async fn agent_reads_updated_environment_secret(
    deps: &EnvBasedTestDependencies,
) -> anyhow::Result<()> {
    let user = deps.user().await?.with_auto_deploy(false);
    let (_, env) = user.app_and_env().await?;
    let client = deps.registry_service().client(&user.token).await;

    let component = user
        .component(&env.id, "golem_it_agent_sdk_ts")
        .name("golem-it:agent-sdk-ts")
        .store()
        .await?;

    let secret_path = vec!["secret".to_string()];

    user.deploy_environment_with(
        env.id,
        vec![DeploymentAgentSecretDefault {
            path: secret_path.clone(),
            secret_value: json!("foo"),
        }],
    )
    .await?;

    let agent_id = agent_id!("SharedConfigAgent", "test-agent");

    user.start_agent(&component.id, agent_id.clone()).await?;

    let response = user
        .invoke_and_await_agent(&component, &agent_id, "echoLocalConfig", data_value!())
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected return value"))?;

    let_assert!(Value::String(config) = response);
    let parsed: serde_json::Value = serde_json::from_str(&config)?;

    assert_eq!(parsed, json!({"secret": "foo"}));

    let secrets = client.get_environment_agent_secrets(&env.id.0).await?;
    let secret = secrets
        .values
        .iter()
        .find(|sec| sec.path == secret_path)
        .unwrap();

    client
        .update_agent_secret(
            &secret.id.0,
            &AgentSecretUpdate {
                current_revision: secret.revision,
                secret_value: OptionalFieldUpdate::Set(json!("bar")),
            },
        )
        .await?;

    let response = user
        .invoke_and_await_agent(&component, &agent_id, "echoLocalConfig", data_value!())
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected return value"))?;

    let_assert!(Value::String(config) = response);
    let parsed: serde_json::Value = serde_json::from_str(&config)?;

    assert_eq!(parsed, json!({"secret": "bar"}));

    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("4m")]
async fn agent_fails_on_missing_environment_secret_value(
    deps: &EnvBasedTestDependencies,
) -> anyhow::Result<()> {
    let user = deps.user().await?.with_auto_deploy(false);
    let (_, env) = user.app_and_env().await?;

    let component = user
        .component(&env.id, "golem_it_agent_sdk_ts")
        .name("golem-it:agent-sdk-ts")
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
async fn agent_fails_on_deleted_environment_secret(
    deps: &EnvBasedTestDependencies,
) -> anyhow::Result<()> {
    let user = deps.user().await?.with_auto_deploy(false);
    let (_, env) = user.app_and_env().await?;
    let client = deps.registry_service().client(&user.token).await;

    let component = user
        .component(&env.id, "golem_it_agent_sdk_ts")
        .name("golem-it:agent-sdk-ts")
        .store()
        .await?;

    let secret_path = vec!["secret".to_string()];

    user.deploy_environment_with(
        env.id,
        vec![DeploymentAgentSecretDefault {
            path: secret_path.clone(),
            secret_value: json!("foo"),
        }],
    )
    .await?;

    let agent_id = agent_id!("SharedConfigAgent", "test-agent");

    user.start_agent(&component.id, agent_id.clone()).await?;

    let response = user
        .invoke_and_await_agent(&component, &agent_id, "echoLocalConfig", data_value!())
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected return value"))?;

    let_assert!(Value::String(config) = response);
    let parsed: serde_json::Value = serde_json::from_str(&config)?;
    assert_eq!(parsed, json!({"secret": "foo"}));

    let secrets = client.get_environment_agent_secrets(&env.id.0).await?;
    let secret = secrets
        .values
        .iter()
        .find(|sec| sec.path == secret_path)
        .unwrap();

    client
        .delete_agent_secret(&secret.id.0, secret.revision.into())
        .await?;

    let response = user
        .invoke_and_await_agent(&component, &agent_id, "echoLocalConfig", data_value!())
        .await;

    let Err(error) = response else {
        panic!("expected failed request")
    };

    let downcasted: golem_client::Error<AgentError> = error.downcast().unwrap();

    assert_matches!(
        downcasted,
        golem_client::Error::Item(AgentError::Error500(_))
    );

    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("4m")]
async fn agent_reads_recreated_environment_secret(
    deps: &EnvBasedTestDependencies,
) -> anyhow::Result<()> {
    let user = deps.user().await?.with_auto_deploy(false);
    let (_, env) = user.app_and_env().await?;
    let client = deps.registry_service().client(&user.token).await;

    let component = user
        .component(&env.id, "golem_it_agent_sdk_ts")
        .name("golem-it:agent-sdk-ts")
        .store()
        .await?;

    let secret_path = vec!["secret".to_string()];

    user.deploy_environment_with(
        env.id,
        vec![DeploymentAgentSecretDefault {
            path: secret_path.clone(),
            secret_value: json!("foo"),
        }],
    )
    .await?;

    let agent_id = agent_id!("SharedConfigAgent", "test-agent");

    user.start_agent(&component.id, agent_id.clone()).await?;

    let secrets = client.get_environment_agent_secrets(&env.id.0).await?;
    let secret = secrets
        .values
        .iter()
        .find(|sec| sec.path == secret_path)
        .unwrap();

    client
        .delete_agent_secret(&secret.id.0, secret.revision.into())
        .await?;

    client
        .create_agent_secret(
            &env.id.0,
            &AgentSecretCreation {
                path: secret_path.clone(),
                secret_type: analysed_type::str(),
                secret_value: Some(json!("bar")),
            },
        )
        .await?;

    let response = user
        .invoke_and_await_agent(&component, &agent_id, "echoLocalConfig", data_value!())
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected return value"))?;

    let_assert!(Value::String(config) = response);
    let parsed: serde_json::Value = serde_json::from_str(&config)?;

    assert_eq!(
        parsed,
        json!({
            "secret": "bar"
        })
    );

    Ok(())
}
