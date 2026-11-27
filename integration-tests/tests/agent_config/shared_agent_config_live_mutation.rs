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
use golem_client::api::{AgentError, RegistryServiceClient};
use golem_client::model::{AgentSecretCreation, AgentSecretUpdate};
use golem_common::model::agent_secret::AgentSecretPath;
use golem_common::model::deployment::DeploymentAgentSecretDefault;
use golem_common::model::oplog::PublicOplogEntry;
use golem_common::model::optional_field_update::OptionalFieldUpdate;
use golem_common::model::{AgentStatus, OplogIndex, PromiseId};
use golem_common::schema::{ExternalSchemaValue, FromSchema, SchemaGraph, SchemaType, SchemaValue};
use golem_common::{agent_key, data_value};
use golem_test_framework::config::{EnvBasedTestDependencies, TestDependencies};
use golem_test_framework::dsl::{TestDsl, TestDslExtended};
use pretty_assertions::assert_eq;
use pretty_assertions::assert_matches;
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;
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
#[timeout("8m")]
async fn agent_reads_updated_environment_secret(
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

    let secret_path = vec!["secret".to_string()];

    user.deploy_environment_with(env.id, |d| {
        d.agent_secret_defaults = vec![
            DeploymentAgentSecretDefault {
                path: AgentSecretPath(secret_path.clone()),
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

    let agent_id = agent_key!("SharedConfigAgent", "test-agent");

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

    let_assert!(SchemaValue::String(config) = response);
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

    let secrets = client.list_environment_agent_secrets(&env.id.0).await?;
    let secret = secrets
        .values
        .iter()
        .find(|sec| sec.path.0 == secret_path)
        .unwrap();

    client
        .update_agent_secret(
            &secret.id.0,
            &AgentSecretUpdate {
                current_revision: secret.revision,
                secret_value: OptionalFieldUpdate::Set(
                    ExternalSchemaValue::try_from(SchemaValue::String("bar".to_string())).unwrap(),
                ),
            },
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
            "secret": "bar",
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
#[timeout("8m")]
async fn repeated_secret_reveal_invocation_replays_pinned_original_value(
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

    let secret_path = vec!["secret".to_string()];

    user.deploy_environment_with(env.id, |d| {
        d.agent_secret_defaults = vec![
            DeploymentAgentSecretDefault {
                path: AgentSecretPath(secret_path.clone()),
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

    let agent_id = agent_key!("SharedConfigAgent", "test-agent-pinned-secret");
    let worker = user.start_agent(&component.id, agent_id.clone()).await?;

    let promise_id_value = user
        .invoke_and_await_agent(
            &component,
            &agent_id,
            ctx.create_replay_gate_method_name(),
            data_value!(),
        )
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected replay gate promise id"))?;
    let promise_id =
        <PromiseId as FromSchema>::from_value(&promise_id_value).map_err(|e| anyhow!("{e}"))?;

    let invocation = {
        let user = user.clone();
        let component = component.clone();
        let agent_id = agent_id.clone();
        let promise_id = promise_id.clone();
        let method_name = ctx
            .reveal_secret_then_await_replay_gate_method_name()
            .to_string();

        tokio::spawn(async move {
            user.invoke_and_await_agent(
                &component,
                &agent_id,
                &method_name,
                data_value!(promise_id),
            )
            .await
        })
    };

    user.wait_for_status(&worker, AgentStatus::Suspended, Duration::from_secs(10))
        .await?;

    let secrets = client.list_environment_agent_secrets(&env.id.0).await?;
    let secret = secrets
        .values
        .iter()
        .find(|sec| sec.path.0 == secret_path)
        .unwrap();

    client
        .update_agent_secret(
            &secret.id.0,
            &AgentSecretUpdate {
                current_revision: secret.revision,
                secret_value: OptionalFieldUpdate::Set(
                    ExternalSchemaValue::try_from(SchemaValue::String("bar".to_string())).unwrap(),
                ),
            },
        )
        .await?;

    user.simulated_crash(&worker).await?;
    user.wait_for_status(&worker, AgentStatus::Suspended, Duration::from_secs(10))
        .await?;
    user.complete_promise(&worker.environment_id, &promise_id, b"resume".to_vec())
        .await?;

    let replayed = invocation
        .await??
        .into_return_value()
        .ok_or_else(|| anyhow!("expected replayed return value"))?;
    assert_eq!(replayed, SchemaValue::String("foo".to_string()));

    let fresh = user
        .invoke_and_await_agent(
            &component,
            &agent_id,
            ctx.agent_method_name(),
            data_value!(),
        )
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected fresh return value"))?;
    let_assert!(SchemaValue::String(config) = fresh);
    let fresh: serde_json::Value = serde_json::from_str(&config)?;
    assert_eq!(fresh["secret"], json!("bar"));

    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("8m")]
async fn held_secret_reveal_after_delete_replays_value_and_version_not_found(
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
    let secret_path = vec!["secret".to_string()];
    user.deploy_environment_with(env.id, |d| {
        d.agent_secret_defaults = vec![
            DeploymentAgentSecretDefault {
                path: AgentSecretPath(secret_path.clone()),
                secret_value: json!("before-delete"),
            },
            DeploymentAgentSecretDefault {
                path: AgentSecretPath(vec![ctx.case_config_path_segment("complex-secret")]),
                secret_value: json!({"foo": "foo", "bar": 1}),
            },
        ];
    })
    .await?;
    let secrets = client.list_environment_agent_secrets(&env.id.0).await?;
    let secret = secrets
        .values
        .iter()
        .find(|secret| secret.path.0 == secret_path)
        .unwrap();
    let agent_id = agent_key!("SharedConfigAgent", "held-secret-delete");
    let worker = user.start_agent(&component.id, agent_id.clone()).await?;
    let mut gates = Vec::new();
    for _ in 0..2 {
        let value = user
            .invoke_and_await_agent(
                &component,
                &agent_id,
                ctx.create_replay_gate_method_name(),
                data_value!(),
            )
            .await?
            .into_return_value()
            .ok_or_else(|| anyhow!("expected replay gate promise id"))?;
        gates.push(PromiseId::from_value(&value).map_err(|error| anyhow!("{error}"))?);
    }
    let invocation = {
        let user = user.clone();
        let component = component.clone();
        let agent_id = agent_id.clone();
        let gates = gates.clone();
        let method = ctx
            .reveal_held_secret_across_replay_gates_method_name()
            .to_string();
        tokio::spawn(async move {
            user.invoke_and_await_agent(
                &component,
                &agent_id,
                &method,
                data_value!(gates[0].clone(), gates[1].clone()),
            )
            .await
        })
    };
    user.wait_for_status(&worker, AgentStatus::Suspended, Duration::from_secs(30))
        .await?;
    client
        .delete_agent_secret(&secret.id.0, secret.revision.into())
        .await?;
    user.complete_promise(&worker.environment_id, &gates[0], b"deleted".to_vec())
        .await?;

    // Wait for the second gate, not a stale Suspended status from the first gate.
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            let entries = user.get_oplog(&worker, OplogIndex::INITIAL).await?;
            let waits = entries.iter().filter(|entry| matches!(&entry.entry,
                PublicOplogEntry::Start(start) if start.function_name == "golem::api::get_promise_result"
            )).count();
            if waits == 2 { return Ok::<_, anyhow::Error>(()); }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }).await??;
    user.wait_for_status(&worker, AgentStatus::Suspended, Duration::from_secs(30))
        .await?;
    user.simulated_crash(&worker).await?;
    user.wait_for_status(&worker, AgentStatus::Suspended, Duration::from_secs(30))
        .await?;
    user.complete_promise(&worker.environment_id, &gates[1], b"replayed".to_vec())
        .await?;
    let replayed = invocation
        .await??
        .into_return_value()
        .ok_or_else(|| anyhow!("expected replayed result"))?;
    let_assert!(SchemaValue::String(replayed) = replayed);
    let replayed: serde_json::Value = serde_json::from_str(&replayed)?;
    assert_eq!(
        replayed,
        json!({
            "before": "before-delete",
            "error": "version-not-found",
            "version": secret.revision.get().to_be_bytes(),
        })
    );
    let entries = user.get_oplog(&worker, OplogIndex::INITIAL).await?;
    assert_eq!(entries.iter().filter(|entry| matches!(&entry.entry,
        PublicOplogEntry::Start(start) if start.function_name == "golem::secrets::reveal::reveal"
    )).count(), 2);
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("8m")]
async fn agent_fails_on_deleted_environment_secret(
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

    let secret_path = vec!["secret".to_string()];

    user.deploy_environment_with(env.id, |d| {
        d.agent_secret_defaults = vec![
            DeploymentAgentSecretDefault {
                path: AgentSecretPath(secret_path.clone()),
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

    let agent_id = agent_key!("SharedConfigAgent", "test-agent");

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

    let_assert!(SchemaValue::String(config) = response);
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

    let secrets = client.list_environment_agent_secrets(&env.id.0).await?;
    let secret = secrets
        .values
        .iter()
        .find(|sec| sec.path.0 == secret_path)
        .unwrap();

    client
        .delete_agent_secret(&secret.id.0, secret.revision.into())
        .await?;

    let response = user
        .invoke_and_await_agent(
            &component,
            &agent_id,
            ctx.agent_method_name(),
            data_value!(),
        )
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
#[timeout("8m")]
async fn agent_reads_recreated_environment_secret(
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

    let secret_path = vec!["secret".to_string()];

    user.deploy_environment_with(env.id, |d| {
        d.agent_secret_defaults = vec![
            DeploymentAgentSecretDefault {
                path: AgentSecretPath(secret_path.clone()),
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

    let agent_id = agent_key!("SharedConfigAgent", "test-agent");

    user.start_agent(&component.id, agent_id.clone()).await?;

    let secrets = client.list_environment_agent_secrets(&env.id.0).await?;
    let secret = secrets
        .values
        .iter()
        .find(|sec| sec.path.0 == secret_path)
        .unwrap();

    client
        .delete_agent_secret(&secret.id.0, secret.revision.into())
        .await?;

    client
        .create_agent_secret(
            &env.id.0,
            &AgentSecretCreation {
                path: AgentSecretPath(secret_path.clone()),
                secret_type: SchemaGraph::anonymous(SchemaType::string()),
                secret_value: Some(
                    ExternalSchemaValue::try_from(SchemaValue::String("bar".to_string())).unwrap(),
                ),
            },
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
            "secret": "bar",
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
#[timeout("8m")]
async fn agent_reads_secret_after_canonicalized_update(
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

    let secret_path = vec!["secret_path".to_string()];

    user.deploy_environment_with(env.id, |d| {
        d.agent_secret_defaults = vec![DeploymentAgentSecretDefault {
            path: AgentSecretPath(secret_path.clone()),
            secret_value: json!("foo"),
        }];
    })
    .await?;

    let agent_id = agent_key!("LocalCasingSharedConfigAgent", "test-agent");

    user.start_agent(&component.id, agent_id.clone()).await?;

    let secrets = client.list_environment_agent_secrets(&env.id.0).await?;
    let secret = secrets
        .values
        .iter()
        .find(|sec| sec.path.0 == vec!["secretPath".to_string()])
        .unwrap();

    client
        .update_agent_secret(
            &secret.id.0,
            &AgentSecretUpdate {
                current_revision: secret.revision,
                secret_value: OptionalFieldUpdate::Set(
                    ExternalSchemaValue::try_from(SchemaValue::String("bar".to_string())).unwrap(),
                ),
            },
        )
        .await?;

    // There is a race here between the updated agent secret becoming visible and the invocation,
    // so poll until we see the new value
    let parsed_config: serde_json::Value;
    loop {
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

        if config.contains("bar") {
            parsed_config = serde_json::from_str(&config)?;
            break;
        }
    }

    assert_eq!(
        parsed_config,
        json!({
            "secretPath": "bar"
        })
    );

    Ok(())
}
