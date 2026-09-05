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

use golem_client::api::{
    RegistryServiceClient, RegistryServiceDeployEnvironmentError,
    RegistryServiceGetToolReleaseError, RegistryServiceRollbackEnvironmentError,
};
use golem_client::model::AgentSecretCreation;
use golem_client::model::DeploymentCreation;
use golem_common::model::agent::AgentTypeName;
use golem_common::model::agent_secret::{AgentSecretPath, CanonicalAgentSecretPath};
use golem_common::model::component::{
    AgentTypeProvisionConfigUpdate, ComponentCreation, ComponentName, ComponentUpdate,
    ToolDeploymentConfigCreation, ToolDeploymentConfigUpdate, ToolProvisionConfigCreation,
};
use golem_common::model::deployment::{
    DeploymentAgentSecretDefault, DeploymentPlan, DeploymentRollback, DeploymentVersion,
};
use golem_common::model::diff::{
    EffectiveToolBinding, RemoteToolDeployment as DiffRemoteToolDeployment,
};
use golem_common::model::diff::{Hash, Hashable};
use golem_common::model::domain_registration::{Domain, DomainRegistrationCreation};
use golem_common::model::environment::EnvironmentCurrentDeploymentView;
use golem_common::model::environment::EnvironmentUpdate;
use golem_common::model::environment_tool_grant::{
    EnvironmentToolGrantCreation, EnvironmentToolGrantDeletion, EnvironmentToolGrantWithDetails,
};
use golem_common::model::http_api_deployment::{
    HttpApiDeploymentAgentOptions, HttpApiDeploymentCreation,
};
use golem_common::model::json::NormalizedJsonValue;
use golem_common::model::optional_field_update::OptionalFieldUpdate;
use golem_common::model::tool::{
    RemoteToolDeployment, SecretKeyScope, ToolBindingInput, ToolFilesystemAccess, ToolName,
    ToolProvisionConfig,
};
use golem_common::model::tool_release::{
    ToolReleaseByCoordinates, ToolReleaseById, ToolReleaseLifecycle, ToolReleaseReference,
};
use golem_common::schema::tool::{
    CommandBody, CommandNode, CommandTree, Doc, Globals, Positionals, Tool,
};
use golem_common::schema::validation::is_equivalent_cross_graph;
use golem_common::schema::{ExternalSchemaValue, SchemaGraph, SchemaType, SchemaValue};
use golem_common::{agent_id, data_value};
use golem_test_framework::config::{EnvBasedTestDependencies, TestDependencies};
use golem_test_framework::dsl::{TestDsl, TestDslExtended};
use pretty_assertions::{assert_eq, assert_matches, assert_ne};
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};
use test_r::{inherit_test_dep, test, timeout};
use tokio::fs::File;

inherit_test_dep!(EnvBasedTestDependencies);

fn cross_account_tool(version: &str) -> Tool {
    Tool {
        version: version.to_string(),
        commands: CommandTree {
            nodes: vec![CommandNode {
                name: "search".to_string(),
                aliases: Vec::new(),
                doc: Doc::default(),
                globals: Globals::default(),
                subcommands: Vec::new(),
                body: Some(CommandBody {
                    positionals: Positionals::default(),
                    options: Vec::new(),
                    flags: Vec::new(),
                    constraints: Vec::new(),
                    stdin: None,
                    stdout: None,
                    result: None,
                    errors: Vec::new(),
                    annotations: None,
                }),
            }],
        },
        schema: SchemaGraph::empty(),
    }
}

fn publisher_tool_config() -> ToolDeploymentConfigCreation {
    ToolDeploymentConfigCreation {
        provision: ToolProvisionConfigCreation {
            config: NormalizedJsonValue::new(json!({})),
            env: BTreeMap::new(),
            plugin_installations: Vec::new(),
            files: BTreeMap::new(),
        },
        environment_binding: None,
        agent_bindings: BTreeMap::new(),
    }
}

fn consumer_secret_scope() -> SecretKeyScope {
    SecretKeyScope::Keys(BTreeSet::from([CanonicalAgentSecretPath(vec![
        "search".to_string(),
        "token".to_string(),
    ])]))
}

fn remote_tool_request(
    release: ToolReleaseReference,
    bind_to_host_api: bool,
) -> RemoteToolDeployment {
    let agent_bindings = if bind_to_host_api {
        BTreeMap::from([(
            AgentTypeName("GolemHostApi".to_string()),
            ToolBindingInput {
                version: None,
                parameters: NormalizedJsonValue::new(json!({"index": "consumer-documents"})),
                account: None,
                secret_keys_readable: consumer_secret_scope(),
                secret_keys_revealable: consumer_secret_scope(),
            },
        )])
    } else {
        BTreeMap::new()
    };
    RemoteToolDeployment {
        name: ToolName::try_from("search").unwrap(),
        release,
        provision: ToolProvisionConfig {
            config: NormalizedJsonValue::new(json!({"tenant": "consumer"})),
            env: BTreeMap::from([("LOG_LEVEL".to_string(), "debug".to_string())]),
            plugins: Vec::new(),
            files: Vec::new(),
        },
        environment_binding: None,
        agent_bindings,
    }
}

fn remote_tool_hash_input(
    grant: &EnvironmentToolGrantWithDetails,
    bind_to_host_api: bool,
) -> DiffRemoteToolDeployment {
    let bindings = if bind_to_host_api {
        BTreeMap::from([(
            AgentTypeName("GolemHostApi".to_string()),
            EffectiveToolBinding {
                parameters: NormalizedJsonValue::new(json!({
                    "index": "consumer-documents"
                })),
                secret_keys_readable: consumer_secret_scope(),
                secret_keys_revealable: consumer_secret_scope(),
                filesystem_access: ToolFilesystemAccess::Unset,
            },
        )])
    } else {
        BTreeMap::new()
    };
    DiffRemoteToolDeployment {
        release_id: grant.release.id,
        version: grant.release.version.clone(),
        source_digest: grant.release.source_digest,
        owner_account_id: grant.release_owner.id,
        owner_account_email: grant.release_owner.email.clone(),
        metadata_version: grant.release.metadata_version.clone(),
        metadata_digest: grant.release.metadata_digest,
        provision: ToolProvisionConfig {
            config: NormalizedJsonValue::new(json!({"tenant": "consumer"})),
            env: BTreeMap::from([("LOG_LEVEL".to_string(), "debug".to_string())]),
            plugins: Vec::new(),
            files: Vec::new(),
        },
        bindings,
    }
}

fn deployment_creation(
    plan: &DeploymentPlan,
    version: &str,
    expected_deployment_hash: Hash,
    publish_tools: Vec<ToolName>,
    remote_tools: Vec<RemoteToolDeployment>,
) -> DeploymentCreation {
    DeploymentCreation {
        current_revision: plan.current_revision,
        expected_deployment_hash,
        version: DeploymentVersion(version.to_string()),
        agent_secret_defaults: Vec::new(),
        quota_resource_defaults: Vec::new(),
        retry_policy_defaults: Vec::new(),
        publish_tools,
        remote_tools,
        replace_incompatible_agent_secrets: false,
    }
}

fn external(value: SchemaValue) -> ExternalSchemaValue {
    ExternalSchemaValue::try_from(value).unwrap()
}

fn assert_secret_type_is_string(secret_type: &SchemaGraph) {
    let expected = SchemaGraph::anonymous(SchemaType::string());
    assert!(is_equivalent_cross_graph(
        secret_type,
        &secret_type.root,
        &expected,
        &expected.root,
    ));
}

#[test]
#[tracing::instrument]
async fn deploy_environment(deps: &EnvBasedTestDependencies) -> anyhow::Result<()> {
    let user = deps.user().await?.with_auto_deploy(false);
    let client = deps.registry_service().client(&user.token).await;
    let (_, env) = user.app_and_env().await?;

    user.component(&env.id, "it_agent_counters_release")
        .name("it:agent-counters")
        .store()
        .await?;

    let plan = client.get_environment_deployment_plan(&env.id.0).await?;

    let deployment = client
        .deploy_environment(
            &env.id.0,
            &DeploymentCreation {
                current_revision: None,
                expected_deployment_hash: plan.deployment_hash,
                version: DeploymentVersion("0.0.1".to_string()),
                publish_tools: Vec::new(),
                remote_tools: Vec::new(),
                agent_secret_defaults: Vec::new(),
                quota_resource_defaults: Vec::new(),
                retry_policy_defaults: Vec::new(),
                replace_incompatible_agent_secrets: false,
            },
        )
        .await?;

    // plan hash and actual hash are the same
    assert_eq!(deployment.deployment_hash, plan.deployment_hash);

    // Can get hash and current revision from environment
    {
        let fetched_environment = client.get_environment(&env.id.0).await?;
        let Some(current_deployment) = fetched_environment.current_deployment else {
            panic!("expected current_deployment to be Some");
        };
        assert_eq!(current_deployment.deployment_revision, deployment.revision);
        assert_eq!(
            current_deployment.deployment_hash,
            deployment.deployment_hash
        );
    }

    // Summary of the deployed deployment is the same as the original plan
    {
        let fetched_deployment = client
            .get_deployment_summary(&env.id.0, deployment.revision.into())
            .await?;
        assert_eq!(fetched_deployment.deployment_hash, plan.deployment_hash);
        assert_eq!(fetched_deployment.components, plan.components);
    }

    Ok(())
}

#[test]
#[tracing::instrument]
async fn deploy_rejects_reset_secret_override_when_compatibility_check_enabled(
    deps: &EnvBasedTestDependencies,
) -> anyhow::Result<()> {
    let user = deps.user().await?.with_auto_deploy(false);
    let client = deps.registry_service().client(&user.token).await;
    let (_, env) = user.app_and_env().await?;

    let env = client
        .update_environment(
            &env.id.0,
            &EnvironmentUpdate {
                current_revision: env.revision,
                name: None,
                compatibility_check: Some(true),
                version_check: None,
                security_overrides: None,
            },
        )
        .await?;

    user.component(&env.id, "it_agent_counters_release")
        .name("it:agent-counters")
        .store()
        .await?;

    let plan = client.get_environment_deployment_plan(&env.id.0).await?;

    let result = client
        .deploy_environment(
            &env.id.0,
            &DeploymentCreation {
                current_revision: None,
                expected_deployment_hash: plan.deployment_hash,
                version: DeploymentVersion("0.0.1".to_string()),
                publish_tools: Vec::new(),
                remote_tools: Vec::new(),
                agent_secret_defaults: Vec::new(),
                quota_resource_defaults: Vec::new(),
                retry_policy_defaults: Vec::new(),
                replace_incompatible_agent_secrets: true,
            },
        )
        .await;

    assert!(matches!(
        result,
        Err(golem_client::Error::Item(
            RegistryServiceDeployEnvironmentError::Error400(_)
        ))
    ));

    Ok(())
}

#[test]
#[tracing::instrument]
async fn deploy_allows_reset_secret_override_when_compatibility_check_disabled(
    deps: &EnvBasedTestDependencies,
) -> anyhow::Result<()> {
    let user = deps.user().await?.with_auto_deploy(false);
    let client = deps.registry_service().client(&user.token).await;
    let (_, env) = user.app_and_env().await?;

    let env = client
        .update_environment(
            &env.id.0,
            &EnvironmentUpdate {
                current_revision: env.revision,
                name: None,
                compatibility_check: Some(false),
                version_check: None,
                security_overrides: None,
            },
        )
        .await?;

    user.component(&env.id, "it_agent_counters_release")
        .name("it:agent-counters")
        .store()
        .await?;

    let plan = client.get_environment_deployment_plan(&env.id.0).await?;

    let deployment = client
        .deploy_environment(
            &env.id.0,
            &DeploymentCreation {
                current_revision: None,
                expected_deployment_hash: plan.deployment_hash,
                version: DeploymentVersion("0.0.1".to_string()),
                publish_tools: Vec::new(),
                remote_tools: Vec::new(),
                agent_secret_defaults: Vec::new(),
                quota_resource_defaults: Vec::new(),
                retry_policy_defaults: Vec::new(),
                replace_incompatible_agent_secrets: true,
            },
        )
        .await?;

    assert_eq!(deployment.deployment_hash, plan.deployment_hash);

    Ok(())
}

#[test]
#[tracing::instrument]
async fn fail_with_409_on_hash_mismatch(deps: &EnvBasedTestDependencies) -> anyhow::Result<()> {
    let user = deps.user().await?.with_auto_deploy(false);
    let client = deps.registry_service().client(&user.token).await;
    let (_, env) = user.app_and_env().await?;

    user.component(&env.id, "it_agent_counters_release")
        .name("it:agent-counters")
        .store()
        .await?;

    {
        let result = client
            .deploy_environment(
                &env.id.0,
                &DeploymentCreation {
                    current_revision: None,
                    expected_deployment_hash: Hash::empty(),
                    version: DeploymentVersion("0.0.1".to_string()),
                    publish_tools: Vec::new(),
                    remote_tools: Vec::new(),
                    agent_secret_defaults: Vec::new(),
                    quota_resource_defaults: Vec::new(),
                    retry_policy_defaults: Vec::new(),
                    replace_incompatible_agent_secrets: false,
                },
            )
            .await;

        assert_matches!(
            result,
            Err(golem_client::Error::Item(
                RegistryServiceDeployEnvironmentError::Error409(_)
            ))
        );
    }

    Ok(())
}

#[test]
#[tracing::instrument]
async fn get_component_version_from_previous_deployment(
    deps: &EnvBasedTestDependencies,
) -> anyhow::Result<()> {
    let user = deps.user().await?.with_auto_deploy(false);
    let client = deps.registry_service().client(&user.token).await;
    let (_, env) = user.app_and_env().await?;

    let component = user
        .component(&env.id, "it_agent_counters_release")
        .name("it:agent-counters")
        .store()
        .await?;

    let plan_1 = client.get_environment_deployment_plan(&env.id.0).await?;

    let deployment_1 = client
        .deploy_environment(
            &env.id.0,
            &DeploymentCreation {
                current_revision: None,
                expected_deployment_hash: plan_1.deployment_hash,
                version: DeploymentVersion("0.0.1".to_string()),
                publish_tools: Vec::new(),
                remote_tools: Vec::new(),
                agent_secret_defaults: Vec::new(),
                quota_resource_defaults: Vec::new(),
                retry_policy_defaults: Vec::new(),
                replace_incompatible_agent_secrets: false,
            },
        )
        .await?;

    let updated_component = client
        .update_component(
            &component.id.0,
            &ComponentUpdate {
                current_revision: component.revision,
                agent_types: None,
                agent_type_provision_config_updates: Some(BTreeMap::from([(
                    AgentTypeName("Counter".to_string()),
                    AgentTypeProvisionConfigUpdate {
                        env: Some(BTreeMap::from_iter(vec![(
                            "ENV_VAR".to_string(),
                            "ENV_VAR_VALUE".to_string(),
                        )])),
                        ..Default::default()
                    },
                )])),
                tools: None,
                tool_deployment_config_updates: None,
                allow_incompatible_config: false,
            },
            None::<Vec<u8>>,
            None::<Vec<u8>>,
        )
        .await?;

    let plan_2 = client.get_environment_deployment_plan(&env.id.0).await?;

    let deployment_2 = client
        .deploy_environment(
            &env.id.0,
            &DeploymentCreation {
                current_revision: Some(deployment_1.current_revision),
                expected_deployment_hash: plan_2.deployment_hash,
                version: DeploymentVersion("0.0.2".to_string()),
                publish_tools: Vec::new(),
                remote_tools: Vec::new(),
                agent_secret_defaults: Vec::new(),
                quota_resource_defaults: Vec::new(),
                retry_policy_defaults: Vec::new(),
                replace_incompatible_agent_secrets: false,
            },
        )
        .await?;

    {
        let fetched_component = client
            .get_deployment_component(
                &env.id.0,
                deployment_1.revision.into(),
                &component.component_name.0,
            )
            .await?;
        assert_eq!(fetched_component, component);
    }

    {
        let fetched_component = client
            .get_deployment_component(
                &env.id.0,
                deployment_2.revision.into(),
                &component.component_name.0,
            )
            .await?;
        assert_eq!(fetched_component, updated_component);
    }

    Ok(())
}

#[test]
#[tracing::instrument]
async fn full_deployment(deps: &EnvBasedTestDependencies) -> anyhow::Result<()> {
    let user = deps.user().await?.with_auto_deploy(false);
    let client = deps.registry_service().client(&user.token).await;
    let (_, env) = user.app_and_env().await?;

    // needs to be static as it's used for hash calculation
    let domain = Domain("full_deployment_test.golem.cloud".to_string());

    client
        .create_domain_registration(
            &env.id.0,
            &DomainRegistrationCreation {
                domain: domain.clone(),
            },
        )
        .await?;

    let component = user
        .component(&env.id, "golem_it_agent_sdk_ts")
        .name("golem-it:agent-sdk-ts")
        .store()
        .await?;

    let http_api_deployment_creation = HttpApiDeploymentCreation {
        domain: domain.clone(),
        agents: BTreeMap::from_iter([(
            AgentTypeName("HttpAgent".to_string()),
            HttpApiDeploymentAgentOptions::default(),
        )]),
        webhooks_prefix: HttpApiDeploymentCreation::default_webhooks_prefix(),
        openapi_endpoint_prefix: HttpApiDeploymentCreation::default_openapi_endpoint_prefix(),
    };

    let http_api_deployment = client
        .create_http_api_deployment(&env.id.0, &http_api_deployment_creation)
        .await?;

    let plan = client.get_environment_deployment_plan(&env.id.0).await?;

    // Verify plan structure without comparing exact hashes
    assert_eq!(plan.current_revision, None);
    assert_eq!(plan.components.len(), 1);
    assert_eq!(
        plan.components[0].name,
        ComponentName("golem-it:agent-sdk-ts".to_string())
    );
    assert_eq!(plan.components[0].id, component.id);
    assert_eq!(plan.components[0].revision, component.revision);
    assert_eq!(plan.http_api_deployments.len(), 1);
    assert_eq!(plan.http_api_deployments[0].id, http_api_deployment.id);
    assert_eq!(
        plan.http_api_deployments[0].revision,
        http_api_deployment.revision
    );
    assert_eq!(plan.http_api_deployments[0].domain, domain);

    let deployment = client
        .deploy_environment(
            &env.id.0,
            &DeploymentCreation {
                current_revision: None,
                expected_deployment_hash: plan.deployment_hash,
                version: DeploymentVersion("0.0.1".to_string()),
                publish_tools: Vec::new(),
                remote_tools: Vec::new(),
                agent_secret_defaults: Vec::new(),
                quota_resource_defaults: Vec::new(),
                retry_policy_defaults: Vec::new(),
                replace_incompatible_agent_secrets: false,
            },
        )
        .await?;
    assert_eq!(deployment.deployment_hash, plan.deployment_hash);

    {
        let fetched = client
            .get_deployment_summary(&env.id.0, deployment.revision.into())
            .await?;

        assert_eq!(fetched.deployment_hash, plan.deployment_hash);
        assert_eq!(fetched.components, plan.components);
        assert_eq!(fetched.http_api_deployments, plan.http_api_deployments);
    }

    Ok(())
}

#[test]
#[tracing::instrument]
async fn rollback(deps: &EnvBasedTestDependencies) -> anyhow::Result<()> {
    let user = deps.user().await?.with_auto_deploy(false);
    let client = deps.registry_service().client(&user.token).await;
    let (_, env) = user.app_and_env().await?;

    user.component(&env.id, "golem_it_agent_rpc")
        .name("golem-it:agent-rpc")
        .store()
        .await?;

    let deployment_1 = user.deploy_environment(env.id).await?;

    user.component(&env.id, "it_agent_counters_release")
        .name("it:agent-counters")
        .store()
        .await?;

    let deployment_2 = user.deploy_environment(env.id).await?;

    assert_ne!(deployment_2.revision, deployment_1.revision);
    assert_ne!(deployment_2.deployment_hash, deployment_1.deployment_hash);

    // noop rollback
    {
        let result = client
            .rollback_environment(
                &env.id.0,
                &DeploymentRollback {
                    current_revision: deployment_2.current_revision,
                    deployment_revision: deployment_2.revision,
                },
            )
            .await;

        assert_matches!(
            result,
            Err(golem_client::Error::Item(
                RegistryServiceRollbackEnvironmentError::Error409(_)
            ))
        );
    }

    {
        let env = client.get_environment(&env.id.0).await?;
        assert_eq!(
            env.current_deployment,
            Some(EnvironmentCurrentDeploymentView {
                revision: deployment_2.current_revision,
                deployment_revision: deployment_2.revision,
                deployment_version: deployment_2.version,
                deployment_hash: deployment_2.deployment_hash
            })
        )
    };

    // actual rollback
    let rollback_result = client
        .rollback_environment(
            &env.id.0,
            &DeploymentRollback {
                current_revision: deployment_2.current_revision,
                deployment_revision: deployment_1.revision,
            },
        )
        .await?;

    let expected_revision = deployment_2.current_revision.next()?;

    assert_eq!(rollback_result.current_revision, expected_revision);
    assert_eq!(rollback_result.revision, deployment_1.revision);
    assert_eq!(
        rollback_result.deployment_hash,
        deployment_1.deployment_hash
    );
    assert_eq!(rollback_result.version, deployment_1.version);

    {
        let env = client.get_environment(&env.id.0).await?;
        assert_eq!(
            env.current_deployment,
            Some(EnvironmentCurrentDeploymentView {
                revision: expected_revision,
                deployment_revision: deployment_1.revision,
                deployment_version: deployment_1.version,
                deployment_hash: deployment_1.deployment_hash
            })
        )
    };

    Ok(())
}

#[test]
#[tracing::instrument]
async fn filter_deployments_by_version(deps: &EnvBasedTestDependencies) -> anyhow::Result<()> {
    let user = deps.user().await?.with_auto_deploy(false);
    let client = deps.registry_service().client(&user.token).await;
    let (_, env) = user.app_and_env().await?;

    let component = user
        .component(&env.id, "it_agent_counters_release")
        .name("it:agent-counters")
        .store()
        .await?;

    let deployment_1 = user.deploy_environment(env.id).await?;

    client
        .update_component(
            &component.id.0,
            &ComponentUpdate {
                current_revision: component.revision,
                agent_types: None,
                agent_type_provision_config_updates: Some(BTreeMap::from([(
                    AgentTypeName("Counter".to_string()),
                    AgentTypeProvisionConfigUpdate {
                        env: Some(BTreeMap::from_iter(vec![(
                            "ENV_VAR".to_string(),
                            "ENV_VAR_VALUE".to_string(),
                        )])),
                        ..Default::default()
                    },
                )])),
                tools: None,
                tool_deployment_config_updates: None,
                allow_incompatible_config: false,
            },
            None::<Vec<u8>>,
            None::<Vec<u8>>,
        )
        .await?;

    let deployment_2 = user.deploy_environment(env.id).await?;

    {
        let deployments = client.list_deployments(&env.id.0, None).await?;
        assert_eq!(
            deployments.values,
            vec![deployment_1.clone().into(), deployment_2.clone().into()]
        )
    }

    {
        let deployments = client
            .list_deployments(&env.id.0, Some(&deployment_2.version.0))
            .await?;
        assert_eq!(deployments.values, vec![deployment_2.clone().into()])
    }

    Ok(())
}

#[test]
#[tracing::instrument]
async fn deploy_creates_missing_secret_from_default(
    deps: &EnvBasedTestDependencies,
) -> anyhow::Result<()> {
    let user = deps.user().await?.with_auto_deploy(false);
    let client = deps.registry_service().client(&user.token).await;
    let (_, env) = user.app_and_env().await?;

    user.component(&env.id, "golem_it_agent_sdk_ts")
        .name("golem-it:agent-sdk-ts")
        .store()
        .await?;

    let plan = client.get_environment_deployment_plan(&env.id.0).await?;

    let secret_path = vec!["secret".into()];

    client
        .deploy_environment(
            &env.id.0,
            &DeploymentCreation {
                current_revision: None,
                expected_deployment_hash: plan.deployment_hash,
                version: DeploymentVersion("0.0.1".to_string()),
                publish_tools: Vec::new(),
                remote_tools: Vec::new(),
                agent_secret_defaults: vec![DeploymentAgentSecretDefault {
                    path: AgentSecretPath(secret_path.clone()),
                    secret_value: json!("foo"),
                }],
                quota_resource_defaults: Vec::new(),
                retry_policy_defaults: Vec::new(),
                replace_incompatible_agent_secrets: false,
            },
        )
        .await?;

    let secrets = client.list_environment_agent_secrets(&env.id.0).await?;

    assert_eq!(secrets.values.len(), 4);
    let secret = secrets
        .values
        .iter()
        .find(|sec| sec.path.0 == secret_path)
        .unwrap();

    assert_eq!(secret.path.0, secret_path);
    assert_secret_type_is_string(&secret.secret_type);
    assert_eq!(
        secret
            .secret_value
            .as_ref()
            .map(ExternalSchemaValue::as_inner),
        Some(&SchemaValue::String("foo".to_string()))
    );

    Ok(())
}

#[test]
#[tracing::instrument]
async fn deploy_ignores_default_if_secret_already_exists(
    deps: &EnvBasedTestDependencies,
) -> anyhow::Result<()> {
    let user = deps.user().await?.with_auto_deploy(false);
    let client = deps.registry_service().client(&user.token).await;
    let (_, env) = user.app_and_env().await?;

    let secret_path = vec!["secret".into()];

    client
        .create_agent_secret(
            &env.id.0,
            &AgentSecretCreation {
                path: AgentSecretPath(secret_path.clone()),
                secret_type: SchemaGraph::anonymous(SchemaType::string()),
                secret_value: Some(external(SchemaValue::String("bar".to_string()))),
            },
        )
        .await?;

    user.component(&env.id, "golem_it_agent_sdk_ts")
        .name("golem-it:agent-sdk-ts")
        .store()
        .await?;

    let plan = client.get_environment_deployment_plan(&env.id.0).await?;

    client
        .deploy_environment(
            &env.id.0,
            &DeploymentCreation {
                current_revision: None,
                expected_deployment_hash: plan.deployment_hash,
                version: DeploymentVersion("0.0.1".to_string()),
                publish_tools: Vec::new(),
                remote_tools: Vec::new(),
                agent_secret_defaults: vec![DeploymentAgentSecretDefault {
                    path: AgentSecretPath(secret_path.clone()),
                    secret_value: json!("foo"),
                }],
                quota_resource_defaults: Vec::new(),
                retry_policy_defaults: Vec::new(),
                replace_incompatible_agent_secrets: false,
            },
        )
        .await?;

    let secrets = client.list_environment_agent_secrets(&env.id.0).await?;

    assert_eq!(secrets.values.len(), 4);
    let secret = secrets
        .values
        .iter()
        .find(|sec| sec.path.0 == secret_path)
        .unwrap();

    assert_eq!(secret.path.0, secret_path);
    assert_secret_type_is_string(&secret.secret_type);

    // Existing value must be preserved
    assert_eq!(
        secret
            .secret_value
            .as_ref()
            .map(ExternalSchemaValue::as_inner),
        Some(&SchemaValue::String("bar".to_string()))
    );

    Ok(())
}

#[test]
#[tracing::instrument]
async fn deploy_uses_default_if_secret_already_exists_with_no_value(
    deps: &EnvBasedTestDependencies,
) -> anyhow::Result<()> {
    let user = deps.user().await?.with_auto_deploy(false);
    let client = deps.registry_service().client(&user.token).await;
    let (_, env) = user.app_and_env().await?;

    let secret_path = vec!["secret".into()];

    client
        .create_agent_secret(
            &env.id.0,
            &AgentSecretCreation {
                path: AgentSecretPath(secret_path.clone()),
                secret_type: SchemaGraph::anonymous(SchemaType::string()),
                secret_value: None,
            },
        )
        .await?;

    user.component(&env.id, "golem_it_agent_sdk_ts")
        .name("golem-it:agent-sdk-ts")
        .store()
        .await?;

    let plan = client.get_environment_deployment_plan(&env.id.0).await?;

    client
        .deploy_environment(
            &env.id.0,
            &DeploymentCreation {
                current_revision: None,
                expected_deployment_hash: plan.deployment_hash,
                version: DeploymentVersion("0.0.1".to_string()),
                publish_tools: Vec::new(),
                remote_tools: Vec::new(),
                agent_secret_defaults: vec![DeploymentAgentSecretDefault {
                    path: AgentSecretPath(secret_path.clone()),
                    secret_value: json!("foo"),
                }],
                quota_resource_defaults: Vec::new(),
                retry_policy_defaults: Vec::new(),
                replace_incompatible_agent_secrets: false,
            },
        )
        .await?;

    let secrets = client.list_environment_agent_secrets(&env.id.0).await?;

    assert_eq!(secrets.values.len(), 4);
    let secret = secrets
        .values
        .iter()
        .find(|sec| sec.path.0 == secret_path)
        .unwrap();

    assert_eq!(secret.path.0, secret_path);
    assert_secret_type_is_string(&secret.secret_type);

    assert_eq!(
        secret
            .secret_value
            .as_ref()
            .map(ExternalSchemaValue::as_inner),
        Some(&SchemaValue::String("foo".to_string()))
    );

    Ok(())
}

#[test]
#[tracing::instrument]
async fn deploy_fails_if_existing_secret_type_mismatches_default(
    deps: &EnvBasedTestDependencies,
) -> anyhow::Result<()> {
    let user = deps.user().await?.with_auto_deploy(false);
    let client = deps.registry_service().client(&user.token).await;
    let (_, env) = user.app_and_env().await?;

    let secret_path = vec!["secret".into()];

    client
        .create_agent_secret(
            &env.id.0,
            &AgentSecretCreation {
                path: AgentSecretPath(secret_path.clone()),
                secret_type: SchemaGraph::anonymous(SchemaType::bool()),
                secret_value: Some(external(SchemaValue::Bool(false))),
            },
        )
        .await?;

    user.component(&env.id, "golem_it_agent_sdk_ts")
        .name("golem-it:agent-sdk-ts")
        .store()
        .await?;

    let plan = client.get_environment_deployment_plan(&env.id.0).await?;

    let result = client
        .deploy_environment(
            &env.id.0,
            &DeploymentCreation {
                current_revision: None,
                expected_deployment_hash: plan.deployment_hash,
                version: DeploymentVersion("0.0.1".to_string()),
                publish_tools: Vec::new(),
                remote_tools: Vec::new(),
                agent_secret_defaults: vec![DeploymentAgentSecretDefault {
                    path: AgentSecretPath(secret_path.clone()),
                    secret_value: json!("abc"),
                }],
                quota_resource_defaults: Vec::new(),
                retry_policy_defaults: Vec::new(),
                replace_incompatible_agent_secrets: false,
            },
        )
        .await;

    assert_matches!(
        result,
        Err(golem_client::Error::Item(
            RegistryServiceDeployEnvironmentError::Error400(_)
        ))
    );

    Ok(())
}

#[test]
#[tracing::instrument]
async fn deploy_fails_if_secret_default_mismatches_component(
    deps: &EnvBasedTestDependencies,
) -> anyhow::Result<()> {
    let user = deps.user().await?.with_auto_deploy(false);
    let client = deps.registry_service().client(&user.token).await;
    let (_, env) = user.app_and_env().await?;

    let secret_path = vec!["secret".into()];

    user.component(&env.id, "golem_it_agent_sdk_ts")
        .name("golem-it:agent-sdk-ts")
        .store()
        .await?;

    let plan = client.get_environment_deployment_plan(&env.id.0).await?;

    let result = client
        .deploy_environment(
            &env.id.0,
            &DeploymentCreation {
                current_revision: None,
                expected_deployment_hash: plan.deployment_hash,
                version: DeploymentVersion("0.0.1".to_string()),
                publish_tools: Vec::new(),
                remote_tools: Vec::new(),
                agent_secret_defaults: vec![DeploymentAgentSecretDefault {
                    path: AgentSecretPath(secret_path.clone()),
                    secret_value: json!(false),
                }],
                quota_resource_defaults: Vec::new(),
                retry_policy_defaults: Vec::new(),
                replace_incompatible_agent_secrets: false,
            },
        )
        .await;

    assert_matches!(
        result,
        Err(golem_client::Error::Item(
            RegistryServiceDeployEnvironmentError::Error400(_)
        ))
    );

    Ok(())
}

#[test]
#[timeout("12m")]
#[tracing::instrument]
async fn cross_account_tool_release_lifecycle_reaches_snapshot_activation(
    deps: &EnvBasedTestDependencies,
) -> anyhow::Result<()> {
    let publisher = deps.user().await?.with_auto_deploy(false);
    let publisher_client = publisher.registry_service_client().await;
    let (_, publisher_env) = publisher.app_and_env().await?;
    let consumer = deps.user().await?.with_auto_deploy(false);
    let consumer_client = consumer.registry_service_client().await;
    let (consumer_app, consumer_env) = consumer.app_and_env().await?;
    let other_consumer_env = consumer.env(&consumer_app.id).await?;
    let tool_name = ToolName::try_from("search").unwrap();

    let publisher_component = publisher_client
        .create_component(
            &publisher_env.id.0,
            &ComponentCreation {
                component_name: ComponentName::try_from("publisher-tools:search")
                    .map_err(anyhow::Error::msg)?,
                agent_types: Vec::new(),
                agent_type_provision_configs: BTreeMap::new(),
                tools: vec![cross_account_tool("1.2.0")],
                tool_deployment_configs: BTreeMap::from([(
                    tool_name.clone(),
                    publisher_tool_config(),
                )]),
            },
            File::open(
                deps.component_directory()
                    .join("it_agent_counters_release.wasm"),
            )
            .await?,
            None::<File>,
        )
        .await?;

    let publisher_plan = publisher_client
        .get_environment_deployment_plan(&publisher_env.id.0)
        .await?;
    let mut publisher_hash_input = publisher_plan.to_diffable();
    publisher_hash_input
        .published_tools
        .insert(tool_name.to_string());
    publisher_client
        .deploy_environment(
            &publisher_env.id.0,
            &deployment_creation(
                &publisher_plan,
                "publisher-1.2.0",
                publisher_hash_input.hash()?,
                vec![tool_name.clone()],
                Vec::new(),
            ),
        )
        .await?;

    let release_v12 = publisher_client
        .list_account_tool_releases(&publisher.account_id.0)
        .await?
        .values
        .into_iter()
        .find(|release| release.name == tool_name && release.version == "1.2.0")
        .expect("publisher deployment must create search@1.2.0");
    assert_eq!(release_v12.lifecycle, ToolReleaseLifecycle::Published);

    let published_source_deletion = publisher_client
        .delete_component(
            &publisher_component.id.0,
            publisher_component.revision.into(),
        )
        .await;
    assert!(
        published_source_deletion.is_err(),
        "a component revision referenced by a tool release and deployment snapshot must not be deleted"
    );

    assert_matches!(
        consumer_client.get_tool_release(&release_v12.id.0).await,
        Err(golem_client::Error::Item(
            RegistryServiceGetToolReleaseError::Error404(_)
        ))
    );

    let release_v12_coordinates = ToolReleaseReference::ByCoordinates(ToolReleaseByCoordinates {
        account: publisher.account_email.clone(),
        name: tool_name.clone(),
        version: "1.2.0".to_string(),
    });
    let grant_v12 = consumer_client
        .create_environment_tool_grant(
            &consumer_env.id.0,
            &EnvironmentToolGrantCreation {
                release: release_v12_coordinates,
                automatic: false,
            },
        )
        .await?;
    assert_eq!(grant_v12.release.id, release_v12.id);
    assert!(
        consumer_client
            .list_environment_tool_grants(&other_consumer_env.id.0)
            .await?
            .values
            .is_empty(),
        "a grant must not leak to another environment in the same account"
    );

    let other_plan = consumer_client
        .get_environment_deployment_plan(&other_consumer_env.id.0)
        .await?;
    let other_remote_hash_input = remote_tool_hash_input(&grant_v12, false);
    let mut other_hash_input = other_plan.to_diffable();
    other_hash_input.remote_tools.insert(
        tool_name.to_string(),
        other_remote_hash_input.clone().into(),
    );
    let ungranted_deploy = consumer_client
        .deploy_environment(
            &other_consumer_env.id.0,
            &deployment_creation(
                &other_plan,
                "ungranted",
                other_hash_input.hash()?,
                Vec::new(),
                vec![remote_tool_request(
                    ToolReleaseReference::ById(ToolReleaseById {
                        release_id: release_v12.id,
                    }),
                    false,
                )],
            ),
        )
        .await;
    assert_matches!(
        ungranted_deploy,
        Err(golem_client::Error::Item(
            RegistryServiceDeployEnvironmentError::Error400(_)
        ))
    );

    let consumer_component = consumer
        .component(&consumer_env.id, "golem_it_host_api_tests_release")
        .name("consumer-tools:host-api")
        .unique()
        .store()
        .await?;
    let consumer_plan_v12 = consumer_client
        .get_environment_deployment_plan(&consumer_env.id.0)
        .await?;
    let remote_v12_hash_input = remote_tool_hash_input(&grant_v12, true);
    let remote_v12_hash = remote_v12_hash_input.hash()?;
    let mut consumer_hash_input_v12 = consumer_plan_v12.to_diffable();
    consumer_hash_input_v12
        .remote_tools
        .insert(tool_name.to_string(), remote_v12_hash_input.into());
    let consumer_deployment_v12 = consumer_client
        .deploy_environment(
            &consumer_env.id.0,
            &deployment_creation(
                &consumer_plan_v12,
                "consumer-1.2.0",
                consumer_hash_input_v12.hash()?,
                Vec::new(),
                vec![remote_tool_request(
                    ToolReleaseReference::ById(ToolReleaseById {
                        release_id: release_v12.id,
                    }),
                    true,
                )],
            ),
        )
        .await?;
    let consumer_summary_v12 = consumer_client
        .get_deployment_summary(&consumer_env.id.0, consumer_deployment_v12.revision.into())
        .await?;
    assert_eq!(consumer_summary_v12.remote_tools.len(), 1);
    assert_eq!(consumer_summary_v12.remote_tools[0].hash, remote_v12_hash);

    let initial_agent = agent_id!("GolemHostApi", "remote-search-1-2");
    consumer
        .start_agent(&consumer_component.id, initial_agent.clone())
        .await?;
    let initial_result = consumer
        .invoke_and_await_agent(
            &consumer_component,
            &initial_agent,
            "tool_rpc_invoke_and_await_result",
            data_value!(tool_name.as_str(), Vec::<String>::new(), String::new()),
        )
        .await?
        .into_typed::<Result<(), String>>()?;
    assert!(
        initial_result.as_ref().is_err_and(|error| {
            error.contains("RemoteInternalError")
                && error.contains("sidecar invocation backend")
                && !error.contains("Denied")
                && !error.contains("NotFound")
        }),
        "a granted remote release must reach the admitted component dispatch boundary: {initial_result:?}"
    );

    let publisher_component_v13 = publisher_client
        .update_component(
            &publisher_component.id.0,
            &ComponentUpdate {
                current_revision: publisher_component.revision,
                agent_types: None,
                agent_type_provision_config_updates: None,
                tools: Some(vec![cross_account_tool("1.3.0")]),
                tool_deployment_config_updates: Some(BTreeMap::from([(
                    tool_name.clone(),
                    ToolDeploymentConfigUpdate {
                        provision: None,
                        environment_binding: OptionalFieldUpdate::NoChange,
                        agent_bindings: None,
                    },
                )])),
                allow_incompatible_config: false,
            },
            None::<File>,
            None::<File>,
        )
        .await?;
    assert_ne!(
        publisher_component_v13.revision,
        publisher_component.revision
    );
    let publisher_plan_v13 = publisher_client
        .get_environment_deployment_plan(&publisher_env.id.0)
        .await?;
    let mut publisher_hash_input_v13 = publisher_plan_v13.to_diffable();
    publisher_hash_input_v13
        .published_tools
        .insert(tool_name.to_string());
    publisher_client
        .deploy_environment(
            &publisher_env.id.0,
            &deployment_creation(
                &publisher_plan_v13,
                "publisher-1.3.0",
                publisher_hash_input_v13.hash()?,
                vec![tool_name.clone()],
                Vec::new(),
            ),
        )
        .await?;
    let release_v13 = publisher_client
        .list_account_tool_releases(&publisher.account_id.0)
        .await?
        .values
        .into_iter()
        .find(|release| release.name == tool_name && release.version == "1.3.0")
        .expect("publisher deployment must create search@1.3.0");

    let current_consumer_environment = consumer_client.get_environment(&consumer_env.id.0).await?;
    let current_consumer_deployment = current_consumer_environment
        .current_deployment
        .expect("consumer environment must retain its current deployment");
    assert_eq!(
        current_consumer_deployment.deployment_revision,
        consumer_deployment_v12.revision
    );
    let still_pinned_summary = consumer_client
        .get_deployment_summary(
            &consumer_env.id.0,
            current_consumer_deployment.deployment_revision.into(),
        )
        .await?;
    assert_eq!(still_pinned_summary.remote_tools.len(), 1);
    assert_eq!(still_pinned_summary.remote_tools[0].hash, remote_v12_hash);

    let grant_v13 = consumer_client
        .create_environment_tool_grant(
            &consumer_env.id.0,
            &EnvironmentToolGrantCreation {
                release: ToolReleaseReference::ById(ToolReleaseById {
                    release_id: release_v13.id,
                }),
                automatic: false,
            },
        )
        .await?;
    let consumer_plan_v13 = consumer_client
        .get_environment_deployment_plan(&consumer_env.id.0)
        .await?;
    let remote_v13_hash_input = remote_tool_hash_input(&grant_v13, true);
    let remote_v13_hash = remote_v13_hash_input.hash()?;
    let mut consumer_hash_input_v13 = consumer_plan_v13.to_diffable();
    consumer_hash_input_v13
        .remote_tools
        .insert(tool_name.to_string(), remote_v13_hash_input.into());
    let consumer_deployment_v13 = consumer_client
        .deploy_environment(
            &consumer_env.id.0,
            &deployment_creation(
                &consumer_plan_v13,
                "consumer-1.3.0",
                consumer_hash_input_v13.hash()?,
                Vec::new(),
                vec![remote_tool_request(
                    ToolReleaseReference::ById(ToolReleaseById {
                        release_id: release_v13.id,
                    }),
                    true,
                )],
            ),
        )
        .await?;
    let consumer_summary_v13 = consumer_client
        .get_deployment_summary(&consumer_env.id.0, consumer_deployment_v13.revision.into())
        .await?;
    assert_eq!(consumer_summary_v13.remote_tools.len(), 1);
    assert_eq!(consumer_summary_v13.remote_tools[0].hash, remote_v13_hash);
    assert_ne!(remote_v13_hash, remote_v12_hash);

    consumer_client
        .delete_environment_tool_grant(
            &grant_v13.grant.id.0,
            &EnvironmentToolGrantDeletion { automatic: false },
        )
        .await?;
    let revoked_agent = agent_id!("GolemHostApi", "remote-search-revoked");
    consumer
        .start_agent(&consumer_component.id, revoked_agent.clone())
        .await?;
    let revoked_result = consumer
        .invoke_and_await_agent(
            &consumer_component,
            &revoked_agent,
            "tool_rpc_invoke_and_await_result",
            data_value!(tool_name.as_str(), Vec::<String>::new(), String::new()),
        )
        .await?
        .into_typed::<Result<(), String>>()?;
    assert!(
        revoked_result.as_ref().is_err_and(|error| {
            error.contains("RemoteInternalError")
                && error.contains("sidecar invocation backend")
                && !error.contains("Denied")
                && !error.contains("NotFound")
        }),
        "grant revocation must not invalidate the current deployment snapshot: {revoked_result:?}"
    );

    consumer_client
        .restore_environment_tool_grant(&grant_v13.grant.id.0)
        .await?;
    let de_published = publisher_client
        .de_publish_tool_release(&release_v13.id.0)
        .await?;
    assert_eq!(de_published.lifecycle, ToolReleaseLifecycle::DePublished);
    let grants_after_depublication = consumer_client
        .list_environment_tool_grants(&consumer_env.id.0)
        .await?
        .values;
    assert_eq!(grants_after_depublication.len(), 1);
    assert_eq!(grants_after_depublication[0].release.id, release_v12.id);

    let de_published_agent = agent_id!("GolemHostApi", "remote-search-de-published");
    consumer
        .start_agent(&consumer_component.id, de_published_agent.clone())
        .await?;
    let de_published_result = consumer
        .invoke_and_await_agent(
            &consumer_component,
            &de_published_agent,
            "tool_rpc_invoke_and_await_result",
            data_value!(tool_name.as_str(), Vec::<String>::new(), String::new()),
        )
        .await?
        .into_typed::<Result<(), String>>()?;
    assert!(
        de_published_result.as_ref().is_err_and(|error| {
            error.contains("RemoteInternalError")
                && error.contains("sidecar invocation backend")
                && !error.contains("Denied")
                && !error.contains("NotFound")
        }),
        "de-publication must not invalidate the current deployment snapshot: {de_published_result:?}"
    );

    let restored_release = publisher_client
        .restore_tool_release(&release_v13.id.0)
        .await?;
    assert_eq!(restored_release.lifecycle, ToolReleaseLifecycle::Published);
    let grants_after_release_restore = consumer_client
        .list_environment_tool_grants(&consumer_env.id.0)
        .await?
        .values;
    assert_eq!(grants_after_release_restore.len(), 2);
    assert!(
        grants_after_release_restore
            .iter()
            .any(|grant| grant.release.id == release_v12.id)
    );
    assert!(
        grants_after_release_restore
            .iter()
            .any(|grant| grant.release.id == release_v13.id),
        "restoring a release must make its preserved grants active again"
    );

    Ok(())
}
