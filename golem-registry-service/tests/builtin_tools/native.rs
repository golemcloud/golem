use golem_common::config::{DbConfig, DbSqliteConfig};
use golem_common::model::Empty;
use golem_common::model::account::{AccountEmail, AccountSummary};
use golem_common::model::agent::extraction::extract_component_metadata_from_bytes;
use golem_common::model::application::{ApplicationCreation, ApplicationName};
use golem_common::model::component::{
    AgentTypeInitialPermissions, AgentTypeProvisionConfigCreation, ComponentCreation, ComponentName,
};
use golem_common::model::deployment::{DeploymentCreation, DeploymentVersion};
use golem_common::model::diff::{Hash, Hashable};
use golem_common::model::environment::{EnvironmentCreation, EnvironmentName};
use golem_common::model::json::NormalizedJsonValue;
use golem_common::model::tool::{ConfigKeyScope, ToolBindingInput, ToolName, ToolProvisionConfig};
use golem_common::model::tool_release::{ToolReleaseById, ToolReleaseReference};
use golem_common::schema::SchemaGraph;
use golem_common::schema::tool::{CommandNode, CommandTree, Tool};
use golem_registry_service::bootstrap::Services;
use golem_registry_service::config::{
    ComponentCompilationConfig, LoginConfig, PrecreatedAccount, RegistryServiceConfig,
};
use golem_registry_service::services::deployment::DeploymentWriteError;
use golem_registry_service::services::native_tool_catalog::NativeToolDescriptor;
use golem_service_base::config::BlobStorageConfig;
use golem_service_base::model::auth::AuthCtx;
use std::collections::{BTreeMap, BTreeSet};
use test_r::{test, timeout};
use tokio::task::JoinSet;

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

    provision_test_native(&services, &owner, &tool_name, "1.0.0").await;
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
        "/../test-components/",
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

    provision_test_native(&services, &owner, &tool_name, "2.0.0").await;
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
    let error = services
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
        .unwrap_err();
    assert!(matches!(
        error,
        DeploymentWriteError::AmbientToolConflict(_)
    ));

    let canonical_request = ambient_deployment_request(
        &plan,
        std::slice::from_ref(&caller_agent_name),
        &agent_overrides,
    );
    let mut duplicate_remote_tools = canonical_request.1.clone();
    let mut conflicting_duplicate = duplicate_remote_tools[0].clone();
    conflicting_duplicate.provision.config =
        NormalizedJsonValue::new(serde_json::json!({"spoofed": true}));
    duplicate_remote_tools.push(conflicting_duplicate);
    let error = services
        .deployment_write_service
        .create_deployment(
            env.id,
            DeploymentCreation {
                current_revision: plan.current_revision,
                expected_deployment_hash: canonical_request.0,
                version: DeploymentVersion("duplicate".into()),
                publish_tools: vec![],
                remote_tools: duplicate_remote_tools,
                agent_secret_defaults: vec![],
                quota_resource_defaults: vec![],
                retry_policy_defaults: vec![],
                replace_incompatible_agent_secrets: false,
            },
            &auth,
        )
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        DeploymentWriteError::DuplicateRemoteToolName(name) if name == tool_name
    ));

    services
        .deployment_write_service
        .create_deployment(
            env.id,
            DeploymentCreation {
                current_revision: plan.current_revision,
                expected_deployment_hash: canonical_request.0,
                version: DeploymentVersion("first".into()),
                publish_tools: vec![],
                remote_tools: canonical_request.1,
                agent_secret_defaults: vec![],
                quota_resource_defaults: vec![],
                retry_policy_defaults: vec![],
                replace_incompatible_agent_secrets: false,
            },
            &auth,
        )
        .await
        .unwrap();
}

async fn provision_test_native(
    services: &Services,
    owner: &PrecreatedAccount,
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
            AccountSummary {
                id: owner.id,
                name: owner.name.clone(),
                email: owner.email.clone(),
            },
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
