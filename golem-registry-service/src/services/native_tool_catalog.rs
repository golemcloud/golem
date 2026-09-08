// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.

use crate::services::tool_release::ToolReleaseService;
use golem_common::model::account::AccountEmail;
use golem_common::model::deployment::DeploymentPlanAmbientToolEntry;
use golem_common::model::tool::{ToolBindingInput, ToolProvisionConfig, ToolSource};
use golem_common::model::tool_release::ToolRelease;
use golem_common::model::tool_release::{
    SystemToolAvailability, SystemToolReleaseProvision, ToolReleaseOrigin,
};
use std::collections::BTreeSet;
use std::sync::{Arc, RwLock};

/// Exact registry-visible half of a native implementation registration.
#[derive(Clone)]
pub struct NativeToolDescriptor {
    pub definition: golem_native_tool::NativeToolDefinition,
    pub release_name: golem_common::model::tool::ToolName,
    pub provision: ToolProvisionConfig,
    pub environment_binding: ToolBindingInput,
}

/// Native definitions compiled into the production registry binary.
pub fn compiled_native_tools() -> Vec<NativeToolDescriptor> {
    Vec::new()
}

/// Registry-side source of truth for native descriptors. It deliberately has no dependency on
/// executor registration; production starts with an empty inventory and tests/products inject it.
#[derive(Clone, Default)]
pub struct NativeToolCatalog {
    entries: Arc<RwLock<Vec<AmbientToolDeployment>>>,
}

#[derive(Clone)]
pub struct AmbientToolDeployment {
    pub release: ToolRelease,
    pub owner_account_email: AccountEmail,
    pub provision: ToolProvisionConfig,
    pub environment_binding: ToolBindingInput,
}

impl NativeToolCatalog {
    pub fn active(&self) -> Vec<AmbientToolDeployment> {
        self.entries
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    pub fn plan_entries(&self) -> Vec<DeploymentPlanAmbientToolEntry> {
        self.active()
            .into_iter()
            .map(|ambient| DeploymentPlanAmbientToolEntry {
                release_id: ambient.release.id,
                name: ambient.release.name,
                version: ambient.release.version,
                source_digest: golem_common::model::tool_release::tool_source_digest(
                    &ambient.release.source,
                ),
                owner_account_id: ambient.release.owner_account_id,
                owner_account_email: ambient.owner_account_email,
                metadata_version: ambient.release.metadata_version,
                metadata_digest: ambient.release.metadata_digest,
                definition: ambient.release.definition,
                provision: ambient.provision,
                environment_binding: ambient.environment_binding,
            })
            .collect()
    }

    pub async fn provision(
        &self,
        descriptors: Vec<NativeToolDescriptor>,
        owner_email: AccountEmail,
        releases: &ToolReleaseService,
    ) -> anyhow::Result<()> {
        let mut names = BTreeSet::new();
        let mut active = Vec::with_capacity(descriptors.len());
        for descriptor in descriptors {
            descriptor
                .definition
                .validate()
                .map_err(anyhow::Error::msg)?;
            if !names.insert(descriptor.release_name.clone()) {
                anyhow::bail!(
                    "native catalog has multiple active versions for {}",
                    descriptor.release_name
                );
            }
            let release = SystemToolReleaseProvision {
                name: descriptor.release_name,
                version: descriptor.definition.tool.version.clone(),
                source: ToolSource::Host {
                    host_tool_id: golem_common::model::tool::HostToolId::try_from(
                        descriptor.definition.id.clone(),
                    )
                    .map_err(anyhow::Error::msg)?,
                    implementation_version: descriptor.definition.implementation_version,
                },
                definition: descriptor.definition.tool,
                metadata_version: descriptor.definition.metadata_version,
                availability: SystemToolAvailability::Ambient,
            };
            let release = releases.provision_system_release(release).await?;
            if release.origin != ToolReleaseOrigin::ProtectedSystem {
                anyhow::bail!("native catalog release was not provisioned as protected system");
            }
            active.push(AmbientToolDeployment {
                release,
                owner_account_email: owner_email.clone(),
                provision: descriptor.provision,
                environment_binding: descriptor.environment_binding,
            });
        }
        *self
            .entries
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = active;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bootstrap::Services;
    use crate::config::{ComponentCompilationConfig, LoginConfig, RegistryServiceConfig};
    use golem_common::config::{DbConfig, DbSqliteConfig};
    use golem_common::model::Empty;
    use golem_common::model::account::AccountId;
    use golem_common::model::agent::extraction::extract_component_metadata_from_bytes;
    use golem_common::model::application::{ApplicationCreation, ApplicationName};
    use golem_common::model::component::{
        AgentTypeInitialPermissions, AgentTypeProvisionConfigCreation, ComponentCreation,
        ComponentName,
    };
    use golem_common::model::deployment::{DeploymentCreation, DeploymentVersion};
    use golem_common::model::diff::{Hash, Hashable};
    use golem_common::model::environment::{EnvironmentCreation, EnvironmentName};
    use golem_common::model::json::NormalizedJsonValue;
    use golem_common::model::tool::{ConfigKeyScope, HostToolId, ToolName};
    use golem_common::model::tool_release::{
        ToolReleaseById, ToolReleaseId, ToolReleaseLifecycle, ToolReleaseOrigin,
        ToolReleaseReference,
    };
    use golem_common::schema::SchemaGraph;
    use golem_common::schema::tool::{CommandNode, CommandTree, Tool};
    use golem_service_base::config::BlobStorageConfig;
    use golem_service_base::model::auth::AuthCtx;
    use std::collections::BTreeMap;
    use test_r::{test, timeout};
    use tokio::task::JoinSet;

    #[test]
    fn plan_exposes_ambient_identity_metadata_and_defaults() {
        let account_id = AccountId::new();
        let release_id = ToolReleaseId::new();
        let name = ToolName::try_from("native-test").unwrap();
        let source = ToolSource::Host {
            host_tool_id: HostToolId::try_from("native-test".to_string()).unwrap(),
            implementation_version: "1".to_string(),
        };
        let now = chrono::Utc::now();
        let release = ToolRelease {
            id: release_id,
            owner_account_id: account_id,
            name: name.clone(),
            version: "1".to_string(),
            source: source.clone(),
            definition: Tool {
                version: "1".to_string(),
                commands: CommandTree { nodes: vec![] },
                schema: SchemaGraph::empty(),
            },
            metadata_version: "0.1.0".to_string(),
            metadata_digest: Hash::new(blake3::hash(b"metadata")),
            immutable: true,
            lifecycle: ToolReleaseLifecycle::Published,
            origin: ToolReleaseOrigin::ProtectedSystem,
            system_availability: Some(SystemToolAvailability::Ambient),
            created_at: now,
            created_by: account_id,
            state_changed_at: now,
            state_changed_by: account_id,
        };
        let environment_binding = ToolBindingInput::default();
        let owner_account_email = AccountEmail::new("system@golem.cloud");
        let catalog = NativeToolCatalog::default();
        *catalog.entries.write().unwrap() = vec![AmbientToolDeployment {
            release: release.clone(),
            owner_account_email: owner_account_email.clone(),
            provision: ToolProvisionConfig::default(),
            environment_binding: environment_binding.clone(),
        }];
        let entry = &catalog.plan_entries()[0];
        assert_eq!(entry.release_id, release_id);
        assert_eq!(entry.environment_binding, environment_binding);
    }

    #[test]
    #[timeout("120s")]
    async fn ambient_catalog_is_compiled_into_first_deployment_and_stale_plan_is_rejected() {
        let temp_dir = tempfile::tempdir().unwrap();
        let config = RegistryServiceConfig {
            db: DbConfig::Sqlite(DbSqliteConfig {
                database: temp_dir
                    .path()
                    .join("registry.db")
                    .to_string_lossy()
                    .into_owned(),
                max_connections: 4,
                foreign_keys: true,
            }),
            login: LoginConfig::Disabled(Empty {}),
            blob_storage: BlobStorageConfig::default_in_memory(),
            component_compilation: ComponentCompilationConfig::Disabled(Empty {}),
            ..Default::default()
        };
        let owner = config.initial_accounts["builtin_tool_owner"].clone();
        let consumer = config.initial_accounts["root"].id;
        let mut join_set = JoinSet::new();
        let services = Services::new(&config, &mut join_set).await.unwrap();
        let auth = AuthCtx::system();
        let user_auth = services
            .auth_service
            .authenticate_token(config.initial_accounts["root"].token.clone().unwrap())
            .await
            .unwrap();
        let tool_name = ToolName::try_from("ambient-test").unwrap();

        provision_test_native(&services, &owner.email, &tool_name, "1.0.0").await;
        let app = services
            .application_service
            .create(
                consumer,
                ApplicationCreation {
                    name: ApplicationName("ambient-consumer".into()),
                },
                &auth,
            )
            .await
            .unwrap();
        let env = services
            .environment_service
            .create(
                app.id,
                EnvironmentCreation {
                    name: EnvironmentName("empty".into()),
                    compatibility_check: false,
                    version_check: false,
                    security_overrides: false,
                },
                &auth,
            )
            .await
            .unwrap();

        let caller_wasm = std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../test-components/tool-streaming/golem-temp/agents/",
            "golem_it_tool_streaming_rust_caller_release.wasm"
        ))
        .expect("build the tool-streaming caller before running this test");
        let metadata = extract_component_metadata_from_bytes(&caller_wasm, true, true)
            .await
            .unwrap();
        let caller_agent = metadata
            .agent_types
            .iter()
            .find(|agent| agent.type_name.0 == "ToolStreamingCaller")
            .cloned()
            .expect("the real caller component must export ToolStreamingCaller");
        let caller_agent_name = caller_agent.type_name.clone();
        services
            .component_write_service
            .create(
                env.id,
                ComponentCreation {
                    component_name: ComponentName("golem-it:tool-streaming-rust-caller".into()),
                    agent_types: vec![caller_agent],
                    agent_type_provision_configs: BTreeMap::from([(
                        caller_agent_name.clone(),
                        AgentTypeProvisionConfigCreation {
                            initial_permissions: AgentTypeInitialPermissions::default(),
                            env: BTreeMap::new(),
                            config: vec![],
                            plugin_installations: vec![],
                            files: BTreeMap::new(),
                        },
                    )]),
                    tools: vec![],
                    tool_deployment_configs: BTreeMap::new(),
                },
                caller_wasm,
                None,
                &user_auth,
            )
            .await
            .unwrap();

        let stale_plan = services
            .deployment_service
            .get_current_deployment_plan(env.id, &auth)
            .await
            .unwrap();
        assert_eq!(stale_plan.components.len(), 1);
        assert_eq!(stale_plan.ambient_tools.len(), 1);
        assert_eq!(
            stale_plan.ambient_tools[0]
                .environment_binding
                .config_keys_readable,
            ConfigKeyScope::Keys(BTreeSet::new())
        );
        let stale_request = ambient_deployment_request(
            &stale_plan,
            std::slice::from_ref(&caller_agent_name),
            &BTreeMap::new(),
        );

        provision_test_native(&services, &owner.email, &tool_name, "2.0.0").await;
        let stale = services
            .deployment_write_service
            .create_deployment(
                env.id,
                DeploymentCreation {
                    current_revision: stale_plan.current_revision,
                    expected_deployment_hash: stale_request.0,
                    version: DeploymentVersion("stale".into()),
                    publish_tools: vec![],
                    remote_tools: stale_request.1,
                    agent_secret_defaults: vec![],
                    quota_resource_defaults: vec![],
                    retry_policy_defaults: vec![],
                    replace_incompatible_agent_secrets: false,
                },
                &auth,
            )
            .await;
        assert!(
            stale.is_err(),
            "a plan from an older ambient catalog must be rejected"
        );

        let plan = services
            .deployment_service
            .get_current_deployment_plan(env.id, &auth)
            .await
            .unwrap();
        let empty_scope = ConfigKeyScope::Keys(BTreeSet::new());
        let agent_overrides = BTreeMap::from([(
            caller_agent_name.clone(),
            ToolBindingInput {
                config_keys_readable: empty_scope.clone(),
                ..Default::default()
            },
        )]);
        let request = ambient_deployment_request(
            &plan,
            std::slice::from_ref(&caller_agent_name),
            &agent_overrides,
        );
        let mut remote_tools = request.1;
        remote_tools[0].provision.config =
            NormalizedJsonValue::new(serde_json::json!({"spoofed": true}));
        remote_tools[0].provision.env = BTreeMap::from([("SPOOFED".into(), "yes".into())]);
        remote_tools[0].environment_binding = Some(ToolBindingInput {
            version: Some("spoofed-version".into()),
            parameters: NormalizedJsonValue::new(serde_json::json!({"spoofed": true})),
            account: Some(AccountEmail::new("spoofed@example.com")),
            ..Default::default()
        });
        let deployed = services
            .deployment_write_service
            .create_deployment(
                env.id,
                DeploymentCreation {
                    current_revision: plan.current_revision,
                    expected_deployment_hash: request.0,
                    version: DeploymentVersion("first".into()),
                    publish_tools: vec![],
                    remote_tools,
                    agent_secret_defaults: vec![],
                    quota_resource_defaults: vec![],
                    retry_policy_defaults: vec![],
                    replace_incompatible_agent_secrets: false,
                },
                &auth,
            )
            .await
            .unwrap();
        let summary = services
            .deployment_service
            .get_deployment_summary(env.id, deployed.revision, &auth)
            .await
            .unwrap();
        assert_eq!(deployed.deployment_hash, request.0);
        assert_eq!(summary.components.len(), 1);
        assert_eq!(summary.remote_tools.len(), 1);
        assert_eq!(summary.remote_tools[0].name, tool_name);
        let state = services
            .deployment_service
            .get_current_tool_deployment_state(env.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(state.registered_tools.len(), 1);
        let registered = &state.registered_tools[&tool_name];
        assert_eq!(registered.provision, ToolProvisionConfig::default());
        assert_eq!(registered.owner_account_email, owner.email);
        let binding = &state.agent_tool_bindings[&caller_agent_name][&tool_name];
        assert_eq!(binding.config_keys_readable, empty_scope);
        assert_eq!(
            binding.parameters,
            NormalizedJsonValue::new(serde_json::json!({}))
        );
        assert_eq!(binding.account_email, registered.owner_account_email);
        assert_eq!(binding.source, registered.source);
        assert!(
            services
                .environment_tool_grant_service
                .list_in_environment(env.id, &auth)
                .await
                .unwrap()
                .is_empty(),
            "ambient tools must not create grants"
        );
    }

    async fn provision_test_native(
        services: &Services,
        owner_email: &AccountEmail,
        name: &ToolName,
        version: &str,
    ) {
        let definition = Tool {
            version: version.into(),
            commands: CommandTree {
                nodes: vec![CommandNode {
                    name: name.to_string(),
                    aliases: vec![],
                    doc: Default::default(),
                    globals: Default::default(),
                    subcommands: vec![],
                    body: None,
                }],
            },
            schema: SchemaGraph::empty(),
        };
        services
            .native_tool_catalog
            .provision(
                vec![NativeToolDescriptor {
                    definition: golem_native_tool::NativeToolDefinition::new(
                        "ambient-test",
                        version,
                        definition,
                    )
                    .unwrap(),
                    release_name: name.clone(),
                    provision: ToolProvisionConfig::default(),
                    environment_binding: ToolBindingInput {
                        config_keys_readable: ConfigKeyScope::Keys(BTreeSet::new()),
                        ..Default::default()
                    },
                }],
                owner_email.clone(),
                &services.tool_release_service,
            )
            .await
            .unwrap();
    }

    fn ambient_deployment_request(
        plan: &golem_common::model::deployment::DeploymentPlan,
        agent_types: &[golem_common::model::agent::AgentTypeName],
        agent_overrides: &BTreeMap<golem_common::model::agent::AgentTypeName, ToolBindingInput>,
    ) -> (Hash, Vec<golem_common::model::tool::RemoteToolDeployment>) {
        let mut target = plan.to_diffable();
        let remote_tools = plan
            .ambient_tools
            .iter()
            .map(|ambient| {
                target.remote_tools.insert(
                    ambient.name.to_string(),
                    ambient
                        .to_diffable(agent_types.iter().cloned(), agent_overrides)
                        .into(),
                );
                golem_common::model::tool::RemoteToolDeployment {
                    name: ambient.name.clone(),
                    release: ToolReleaseReference::ById(ToolReleaseById {
                        release_id: ambient.release_id,
                    }),
                    provision: ambient.provision.clone(),
                    environment_binding: Some(ambient.environment_binding.clone()),
                    agent_bindings: agent_overrides.clone(),
                }
            })
            .collect();
        (target.hash().unwrap(), remote_tools)
    }
}
