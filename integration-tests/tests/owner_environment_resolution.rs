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
use golem_client::api::RegistryServiceClient;
use golem_client::model::{AgentSecretCreation, DeploymentCreation};
use golem_common::model::agent::AgentTypeName;
use golem_common::model::agent_secret::{AgentSecretPath, CanonicalAgentSecretPath};
use golem_common::model::deployment::{DeploymentPlan, DeploymentVersion};
use golem_common::model::diff::Hashable;
use golem_common::model::diff::{
    Deployment as DiffDeployment, EffectiveToolBinding,
    RemoteToolDeployment as DiffRemoteToolDeployment, ToolMiddlewareBindingInput,
};
use golem_common::model::environment::Environment;
use golem_common::model::environment_tool_grant::{
    EnvironmentToolGrantCreation, EnvironmentToolGrantWithDetails,
};
use golem_common::model::json::NormalizedJsonValue;
use golem_common::model::quota::{
    EnforcementAction, ResourceCapacityLimit, ResourceDefinitionCreation, ResourceLimit,
    ResourceName,
};
use golem_common::model::tool::{
    ConfigKeyScope, RemoteToolDeployment, SecretKeyScope, ToolBindingInput, ToolName,
    ToolProvisionConfig,
};
use golem_common::model::tool_release::{ToolReleaseByCoordinates, ToolReleaseReference};
use golem_common::model::worker::AgentConfigEntryDto;
use golem_common::schema::{ExternalSchemaValue, SchemaGraph, SchemaType, SchemaValue};
use golem_common::{agent_id, data_value};
use golem_test_framework::config::dsl_impl::TestUserContext;
use golem_test_framework::config::{EnvBasedTestDependencies, TestDependencies};
use golem_test_framework::dsl::{TestDsl, TestDslExtended};
use serde_json::json;
use std::collections::{BTreeMap, HashMap};
use test_r::{inherit_test_dep, test, timeout};

inherit_test_dep!(Tracing);
inherit_test_dep!(EnvBasedTestDependencies);

const TOOL: &str = "environment-probe";
const CALLER: &str = "EnvironmentProbeCaller";
const RESOURCE: &str = "owner-capacity";

fn deployment_creation(
    plan: &DeploymentPlan,
    version: &str,
    expected_deployment_hash: golem_common::model::diff::Hash,
    publish_tools: Vec<ToolName>,
    remote_tools: Vec<RemoteToolDeployment>,
) -> DeploymentCreation {
    DeploymentCreation {
        current_revision: plan.current_revision,
        expected_deployment_hash,
        version: DeploymentVersion(version.to_string()),
        publish_tools,
        publish_tool_middlewares: Vec::new(),
        remote_tool_middlewares: Vec::new(),
        universal_tool_middlewares: Vec::new(),
        environment_tool_middleware_bindings: Default::default(),
        agent_tool_middleware_bindings: Default::default(),
        remote_tools,
        mcp_imports: Vec::new(),
        agent_secret_defaults: Vec::new(),
        quota_resource_defaults: Vec::new(),
        retry_policy_defaults: Vec::new(),
        replace_incompatible_agent_secrets: false,
    }
}

fn caller_binding() -> ToolBindingInput {
    ToolBindingInput {
        config_keys_readable: ConfigKeyScope::All,
        secret_keys_readable: SecretKeyScope::Keys(std::collections::BTreeSet::from([
            CanonicalAgentSecretPath(vec!["secret".to_string()]),
        ])),
        secret_keys_revealable: SecretKeyScope::Keys(std::collections::BTreeSet::from([
            CanonicalAgentSecretPath(vec!["secret".to_string()]),
        ])),
        ..ToolBindingInput::default()
    }
}

fn remote_tool(release: &EnvironmentToolGrantWithDetails) -> RemoteToolDeployment {
    RemoteToolDeployment {
        name: ToolName::try_from(TOOL).unwrap(),
        release: ToolReleaseReference::ByCoordinates(ToolReleaseByCoordinates {
            account: release.release_owner.email.clone(),
            name: release.release.name.clone(),
            version: release.release.version.clone(),
        }),
        provision: ToolProvisionConfig {
            config: NormalizedJsonValue::new(json!({"marker": "publisher"})),
            env: BTreeMap::from([("OWNER_MARKER".to_string(), "publisher".to_string())]),
            plugins: Vec::new(),
            files: Vec::new(),
        },
        environment_binding: None,
        component_bindings: BTreeMap::new(),
        agent_bindings: BTreeMap::from([(AgentTypeName(CALLER.to_string()), caller_binding())]),
    }
}

fn remote_hash_input(release: &EnvironmentToolGrantWithDetails) -> DiffRemoteToolDeployment {
    DiffRemoteToolDeployment {
        release_id: release.release.id,
        version: release.release.version.clone(),
        source_digest: release.release.source_digest,
        owner_account_id: release.release_owner.id,
        owner_account_email: release.release_owner.email.clone(),
        metadata_version: release.release.metadata_version.clone(),
        metadata_digest: release.release.metadata_digest,
        provision: ToolProvisionConfig {
            config: NormalizedJsonValue::new(json!({"marker": "publisher"})),
            env: BTreeMap::from([("OWNER_MARKER".to_string(), "publisher".to_string())]),
            plugins: Vec::new(),
            files: Vec::new(),
        },
        component_bindings: BTreeMap::new(),
        bindings: BTreeMap::from([(
            AgentTypeName(CALLER.to_string()),
            EffectiveToolBinding {
                parameters: NormalizedJsonValue::new(json!({})),
                config_keys_readable: ConfigKeyScope::All,
                secret_keys_readable: caller_binding().secret_keys_readable,
                secret_keys_revealable: caller_binding().secret_keys_revealable,
                filesystem_access: Default::default(),
            },
        )]),
        environment_middleware_binding: None,
        component_middleware_bindings: BTreeMap::new(),
        agent_middleware_bindings: BTreeMap::from([(
            AgentTypeName(CALLER.to_string()),
            ToolMiddlewareBindingInput {
                config_keys_readable: ConfigKeyScope::All,
                secret_keys_readable: caller_binding().secret_keys_readable,
                secret_keys_revealable: caller_binding().secret_keys_revealable,
                middleware: None,
                middleware_merge_mode: None,
            },
        )]),
    }
}

async fn prepare_consumer_environment(
    deps: &EnvBasedTestDependencies,
    user: &TestUserContext<EnvBasedTestDependencies>,
    env: &Environment,
    release: &EnvironmentToolGrantWithDetails,
    marker: &str,
    secret: &str,
    capacity: u64,
) -> anyhow::Result<golem_common::model::component::ComponentDto> {
    let client = deps.registry_service().client(&user.token).await;
    let component = user
        .component(&env.id, "golem_it_tool_streaming_rust_caller_release")
        .name(&format!("golem-it:owner-environment-caller-{marker}"))
        .with_agent_config(
            CALLER,
            vec![AgentConfigEntryDto {
                path: vec!["marker".to_string()],
                value: json!("component-default").into(),
            }],
        )
        .store()
        .await?;

    client
        .create_agent_secret(
            &env.id.0,
            &AgentSecretCreation {
                path: AgentSecretPath(vec!["secret".to_string()]),
                secret_type: SchemaGraph::anonymous(SchemaType::string()),
                secret_value: Some(
                    ExternalSchemaValue::try_from(SchemaValue::String(secret.to_string()))
                        .map_err(anyhow::Error::msg)?,
                ),
            },
        )
        .await?;
    client
        .create_resource(
            &env.id.0,
            &ResourceDefinitionCreation {
                name: ResourceName(RESOURCE.to_string()),
                limit: ResourceLimit::Capacity(ResourceCapacityLimit { value: capacity }),
                enforcement_action: EnforcementAction::Reject,
                unit: "operation".to_string(),
                units: "operations".to_string(),
            },
        )
        .await?;

    let plan = client.get_environment_deployment_plan(&env.id.0).await?;
    let request = remote_tool(release);
    let mut hash_input: DiffDeployment = plan.to_diffable();
    hash_input
        .remote_tools
        .insert(TOOL.to_string(), remote_hash_input(release).into());
    client
        .deploy_environment(
            &env.id.0,
            &deployment_creation(
                &plan,
                &format!("consumer-{marker}"),
                hash_input.hash()?,
                Vec::new(),
                vec![request],
            ),
        )
        .await?;

    Ok(component)
}

fn assert_evidence(value: SchemaValue, marker: &str, secret: &str, reserved: bool) {
    assert_eq!(
        value,
        SchemaValue::Record {
            fields: vec![
                SchemaValue::String(marker.to_string()),
                SchemaValue::String(secret.to_string()),
                SchemaValue::Bool(reserved),
            ],
        }
    );
}

async fn invoke_probe(
    user: &TestUserContext<EnvBasedTestDependencies>,
    component: &golem_common::model::component::ComponentDto,
    agent: &golem_common::model::agent::ParsedAgentId,
    expected_use: u64,
    amount: u64,
    commit_amount: u64,
) -> anyhow::Result<SchemaValue> {
    user.invoke_and_await_agent(
        component,
        agent,
        "observe_owner_environment",
        data_value!(expected_use, amount, commit_amount),
    )
    .await?
    .into_return_value()
    .ok_or_else(|| anyhow::anyhow!("environment probe returned no value"))
}

#[test]
#[timeout("12m")]
#[tracing::instrument]
async fn granted_tool_uses_calling_owner_environment_for_config_secrets_and_named_quota(
    deps: &EnvBasedTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let publisher = deps.user().await?.with_auto_deploy(false);
    let publisher_client = publisher.registry_service_client().await;
    let (_, publisher_env) = publisher.app_and_env().await?;
    publisher
        .component(
            &publisher_env.id,
            "golem_it_tool_streaming_rust_provider_release",
        )
        .name("golem-it:owner-environment-provider")
        .store()
        .await?;

    let tool_name = ToolName::try_from(TOOL).map_err(anyhow::Error::msg)?;
    let publisher_plan = publisher_client
        .get_environment_deployment_plan(&publisher_env.id.0)
        .await?;
    let mut publisher_hash_input = publisher_plan.to_diffable();
    publisher_hash_input
        .published_tools
        .insert(TOOL.to_string());
    publisher_client
        .deploy_environment(
            &publisher_env.id.0,
            &deployment_creation(
                &publisher_plan,
                "publisher",
                publisher_hash_input.hash()?,
                vec![tool_name.clone()],
                Vec::new(),
            ),
        )
        .await?;
    let release = publisher_client
        .list_account_tool_releases(&publisher.account_id.0)
        .await?
        .values
        .into_iter()
        .find(|release| release.name == tool_name)
        .expect("environment probe release is published");

    let consumer = deps.user().await?.with_auto_deploy(false);
    let (consumer_app, env1) = consumer.app_and_env().await?;
    let env2 = consumer.env(&consumer_app.id).await?;
    let consumer_client = consumer.registry_service_client().await;
    let grant1 = consumer_client
        .create_environment_tool_grant(
            &env1.id.0,
            &EnvironmentToolGrantCreation {
                release: ToolReleaseReference::ByCoordinates(ToolReleaseByCoordinates {
                    account: publisher.account_email.clone(),
                    name: tool_name.clone(),
                    version: release.version.clone(),
                }),
                automatic: false,
            },
        )
        .await?;
    let grant2 = consumer_client
        .create_environment_tool_grant(
            &env2.id.0,
            &EnvironmentToolGrantCreation {
                release: ToolReleaseReference::ByCoordinates(ToolReleaseByCoordinates {
                    account: publisher.account_email.clone(),
                    name: tool_name,
                    version: release.version,
                }),
                automatic: false,
            },
        )
        .await?;

    let component1 =
        prepare_consumer_environment(deps, &consumer, &env1, &grant1, "e1", "secret-e1", 2).await?;
    let component2 =
        prepare_consumer_environment(deps, &consumer, &env2, &grant2, "e2", "secret-e2", 4).await?;
    let agent1 = agent_id!(CALLER, "e1-caller");
    let agent2 = agent_id!(CALLER, "e2-caller");
    consumer
        .start_agent_with(
            &component1.id,
            agent1.clone(),
            HashMap::new(),
            vec![AgentConfigEntryDto {
                path: vec!["marker".to_string()],
                value: json!("e1").into(),
            }],
        )
        .await?;
    consumer
        .start_agent_with(
            &component2.id,
            agent2.clone(),
            HashMap::new(),
            vec![AgentConfigEntryDto {
                path: vec!["marker".to_string()],
                value: json!("e2").into(),
            }],
        )
        .await?;

    assert_evidence(
        invoke_probe(&consumer, &component1, &agent1, 2, 2, 2).await?,
        "e1",
        "secret-e1",
        true,
    );
    assert_evidence(
        invoke_probe(&consumer, &component2, &agent2, 3, 3, 3).await?,
        "e2",
        "secret-e2",
        true,
    );
    assert_evidence(
        invoke_probe(&consumer, &component1, &agent1, 1, 1, 1).await?,
        "e1",
        "secret-e1",
        false,
    );
    assert_evidence(
        invoke_probe(&consumer, &component2, &agent2, 1, 1, 1).await?,
        "e2",
        "secret-e2",
        true,
    );
    assert_evidence(
        invoke_probe(&consumer, &component2, &agent2, 1, 1, 1).await?,
        "e2",
        "secret-e2",
        false,
    );

    Ok(())
}
