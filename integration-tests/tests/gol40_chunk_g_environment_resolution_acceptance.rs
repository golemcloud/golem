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
use golem_common::model::agent::{AgentTypeName, ParsedAgentId};
use golem_common::model::agent_secret::{AgentSecretPath, CanonicalAgentSecretPath};
use golem_common::model::component::ComponentDto;
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
use golem_common::model::worker::{AgentConfigEntryDto, AgentMetadataDto};
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

struct ChunkGEnvironmentFixture {
    user: TestUserContext<EnvBasedTestDependencies>,
    publisher_environment: Environment,
    e1: ChunkGConsumer,
    e2: ChunkGConsumer,
}

struct ChunkGConsumer {
    environment: Environment,
    component: ComponentDto,
    parsed_agent_id: ParsedAgentId,
    agent_id: golem_common::model::AgentId,
}

#[derive(Debug, PartialEq, Eq)]
struct EnvironmentEvidence {
    marker: String,
    secret: String,
    reserved: bool,
}

#[test]
#[timeout("12m")]
#[tracing::instrument]
async fn env_config_1_interleaved_e0_release_uses_each_reconstructed_callers_environment(
    deps: &EnvBasedTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let fixture = chunk_g_environment_fixture(deps, 8, 8).await?;
    assert_ne!(fixture.publisher_environment.id, fixture.e1.environment.id);
    assert_ne!(fixture.publisher_environment.id, fixture.e2.environment.id);
    assert_ne!(fixture.e1.environment.id, fixture.e2.environment.id);
    let before_e1 = identity(&fixture.user, &fixture.e1).await?;
    let before_e2 = identity(&fixture.user, &fixture.e2).await?;

    let (e1_first, e2_first) = tokio::join!(
        invoke_probe(&fixture.user, &fixture.e1, 1, 0, 0),
        invoke_probe(&fixture.user, &fixture.e2, 1, 0, 0),
    );
    assert_eq!(
        e1_first?,
        EnvironmentEvidence {
            marker: "e1-creation".to_string(),
            secret: "secret-e1".to_string(),
            reserved: true,
        }
    );
    assert_eq!(
        e2_first?,
        EnvironmentEvidence {
            marker: "e2-creation".to_string(),
            secret: "secret-e2".to_string(),
            reserved: true,
        }
    );

    fixture.user.simulated_crash(&fixture.e1.agent_id).await?;
    fixture.user.simulated_crash(&fixture.e2.agent_id).await?;

    let (e2_second, e1_second) = tokio::join!(
        invoke_probe(&fixture.user, &fixture.e2, 1, 0, 0),
        invoke_probe(&fixture.user, &fixture.e1, 1, 0, 0),
    );
    assert_eq!(
        e2_second?,
        EnvironmentEvidence {
            marker: "e2-creation".to_string(),
            secret: "secret-e2".to_string(),
            reserved: true,
        }
    );
    assert_eq!(
        e1_second?,
        EnvironmentEvidence {
            marker: "e1-creation".to_string(),
            secret: "secret-e1".to_string(),
            reserved: true,
        }
    );

    assert_identity_survived_reconstruction(&fixture.user, &fixture.e1, &before_e1).await?;
    assert_identity_survived_reconstruction(&fixture.user, &fixture.e2, &before_e2).await?;
    Ok(())
}

#[test]
#[timeout("12m")]
#[tracing::instrument]
async fn env_quota_1_named_capacity_is_selected_by_reconstructed_callers_environment(
    deps: &EnvBasedTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let fixture = chunk_g_environment_fixture(deps, 2, 4).await?;
    let before_e1 = identity(&fixture.user, &fixture.e1).await?;
    let before_e2 = identity(&fixture.user, &fixture.e2).await?;

    let (e1_full, e2_partial) = tokio::join!(
        invoke_probe(&fixture.user, &fixture.e1, 2, 2, 2),
        invoke_probe(&fixture.user, &fixture.e2, 3, 3, 3),
    );
    assert!(e1_full?.reserved);
    assert!(e2_partial?.reserved);

    fixture.user.simulated_crash(&fixture.e1.agent_id).await?;
    fixture.user.simulated_crash(&fixture.e2.agent_id).await?;

    let (e2_last, e1_exhausted) = tokio::join!(
        invoke_probe(&fixture.user, &fixture.e2, 1, 1, 1),
        invoke_probe(&fixture.user, &fixture.e1, 1, 1, 1),
    );
    assert!(
        e2_last?.reserved,
        "E1 exhaustion contaminated E2 quota state"
    );
    assert!(
        !e1_exhausted?.reserved,
        "E1 capacity was not durably exhausted"
    );
    assert!(
        !invoke_probe(&fixture.user, &fixture.e2, 1, 1, 1)
            .await?
            .reserved,
        "E2 committed capacity was not tracked independently"
    );

    assert_identity_survived_reconstruction(&fixture.user, &fixture.e1, &before_e1).await?;
    assert_identity_survived_reconstruction(&fixture.user, &fixture.e2, &before_e2).await?;
    Ok(())
}

async fn identity(
    user: &TestUserContext<EnvBasedTestDependencies>,
    consumer: &ChunkGConsumer,
) -> anyhow::Result<AgentMetadataDto> {
    let metadata = user.get_worker_metadata(&consumer.agent_id).await?;
    assert_eq!(metadata.environment_id, consumer.environment.id);
    assert_eq!(metadata.agent_id, consumer.agent_id);
    Ok(metadata)
}

async fn assert_identity_survived_reconstruction(
    user: &TestUserContext<EnvBasedTestDependencies>,
    consumer: &ChunkGConsumer,
    before: &AgentMetadataDto,
) -> anyhow::Result<()> {
    let after = identity(user, consumer).await?;
    assert_eq!(after.fingerprint, before.fingerprint);
    assert!(
        after.last_oplog_index > before.last_oplog_index,
        "tool effects did not durably advance the calling owner's oplog"
    );
    Ok(())
}

async fn chunk_g_environment_fixture(
    deps: &EnvBasedTestDependencies,
    e1_capacity: u64,
    e2_capacity: u64,
) -> anyhow::Result<ChunkGEnvironmentFixture> {
    let publisher = deps.user().await?.with_auto_deploy(false);
    let publisher_client = publisher.registry_service_client().await;
    let (_, publisher_environment) = publisher.app_and_env().await?;
    publisher
        .component(
            &publisher_environment.id,
            "golem_it_tool_streaming_rust_provider_release",
        )
        .name("golem-it:gol40-chunk-g-environment-provider")
        .store()
        .await?;
    let tool_name = ToolName::try_from(TOOL).map_err(anyhow::Error::msg)?;
    let publisher_plan = publisher_client
        .get_environment_deployment_plan(&publisher_environment.id.0)
        .await?;
    let mut publisher_hash_input = publisher_plan.to_diffable();
    publisher_hash_input
        .published_tools
        .insert(TOOL.to_string());
    publisher_client
        .deploy_environment(
            &publisher_environment.id.0,
            &deployment_creation(
                &publisher_plan,
                "gol40-chunk-g-publisher",
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

    let user = deps.user().await?.with_auto_deploy(false);
    let (application, e1_environment) = user.app_and_env().await?;
    let e2_environment = user.env(&application.id).await?;
    let client = user.registry_service_client().await;
    let e1_grant = grant(&client, &publisher, &e1_environment, &release).await?;
    let e2_grant = grant(&client, &publisher, &e2_environment, &release).await?;
    let e1 = prepare_consumer(
        deps,
        &user,
        e1_environment,
        &e1_grant,
        "e1",
        "secret-e1",
        e1_capacity,
    )
    .await?;
    let e2 = prepare_consumer(
        deps,
        &user,
        e2_environment,
        &e2_grant,
        "e2",
        "secret-e2",
        e2_capacity,
    )
    .await?;
    Ok(ChunkGEnvironmentFixture {
        user,
        publisher_environment,
        e1,
        e2,
    })
}

async fn grant(
    client: &(impl RegistryServiceClient + Sync),
    publisher: &TestUserContext<EnvBasedTestDependencies>,
    environment: &Environment,
    release: &golem_common::model::tool_release::ToolRelease,
) -> anyhow::Result<EnvironmentToolGrantWithDetails> {
    Ok(client
        .create_environment_tool_grant(
            &environment.id.0,
            &EnvironmentToolGrantCreation {
                release: ToolReleaseReference::ByCoordinates(ToolReleaseByCoordinates {
                    account: publisher.account_email.clone(),
                    name: release.name.clone(),
                    version: release.version.clone(),
                }),
                automatic: false,
            },
        )
        .await?)
}

async fn prepare_consumer(
    deps: &EnvBasedTestDependencies,
    user: &TestUserContext<EnvBasedTestDependencies>,
    environment: Environment,
    release: &EnvironmentToolGrantWithDetails,
    label: &str,
    secret: &str,
    capacity: u64,
) -> anyhow::Result<ChunkGConsumer> {
    let client = deps.registry_service().client(&user.token).await;
    let component = user
        .component(
            &environment.id,
            "golem_it_tool_streaming_rust_caller_release",
        )
        .name(&format!("golem-it:gol40-chunk-g-caller-{label}"))
        .with_agent_config(
            CALLER,
            vec![AgentConfigEntryDto {
                path: vec!["marker".to_string()],
                value: json!(format!("{label}-component-default")).into(),
            }],
        )
        .store()
        .await?;
    client
        .create_agent_secret(
            &environment.id.0,
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
            &environment.id.0,
            &ResourceDefinitionCreation {
                name: ResourceName(RESOURCE.to_string()),
                limit: ResourceLimit::Capacity(ResourceCapacityLimit { value: capacity }),
                enforcement_action: EnforcementAction::Reject,
                unit: "operation".to_string(),
                units: "operations".to_string(),
            },
        )
        .await?;
    let plan = client
        .get_environment_deployment_plan(&environment.id.0)
        .await?;
    let request = remote_tool(release);
    let mut hash_input: DiffDeployment = plan.to_diffable();
    hash_input
        .remote_tools
        .insert(TOOL.to_string(), remote_hash_input(release).into());
    client
        .deploy_environment(
            &environment.id.0,
            &deployment_creation(
                &plan,
                &format!("gol40-chunk-g-consumer-{label}"),
                hash_input.hash()?,
                Vec::new(),
                vec![request],
            ),
        )
        .await?;
    let parsed_agent_id = agent_id!(CALLER, &format!("gol40-chunk-g-{label}"));
    let agent_id = user
        .start_agent_with(
            &component.id,
            parsed_agent_id.clone(),
            HashMap::new(),
            vec![AgentConfigEntryDto {
                path: vec!["marker".to_string()],
                value: json!(format!("{label}-creation")).into(),
            }],
        )
        .await?;
    Ok(ChunkGConsumer {
        environment,
        component,
        parsed_agent_id,
        agent_id,
    })
}

async fn invoke_probe(
    user: &TestUserContext<EnvBasedTestDependencies>,
    consumer: &ChunkGConsumer,
    expected_use: u64,
    amount: u64,
    commit_amount: u64,
) -> anyhow::Result<EnvironmentEvidence> {
    let value = user
        .invoke_and_await_agent(
            &consumer.component,
            &consumer.parsed_agent_id,
            "observe_owner_environment",
            data_value!(expected_use, amount, commit_amount),
        )
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow::anyhow!("environment probe returned no value"))?;
    let SchemaValue::Record { fields } = value else {
        anyhow::bail!("unexpected environment evidence: {value:?}");
    };
    let [
        SchemaValue::String(marker),
        SchemaValue::String(secret),
        SchemaValue::Bool(reserved),
    ] = fields.as_slice()
    else {
        anyhow::bail!("unexpected environment evidence fields: {fields:?}");
    };
    Ok(EnvironmentEvidence {
        marker: marker.clone(),
        secret: secret.clone(),
        reserved: *reserved,
    })
}

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
