use super::{
    CachedToolDeployment, EnvironmentStateService, GrpcEnvironmentStateService,
    ToolActivationOutcome, ToolActivationSnapshot, ToolDiscoveryCache, ToolDiscoveryError,
    ToolDiscoverySnapshot, get_accessible_tool_from_snapshot, get_accessible_tools_from_snapshot,
    get_tool_activation_from_deployment,
};
use golem_common::model::account::{AccountEmail, AccountId};
use golem_common::model::agent::{
    AgentFileContentHash, AgentTypeName, RegisteredAgentType, ResolvedAgentType,
};
use golem_common::model::agent_secret::{
    AgentSecretId, AgentSecretRevision, CanonicalAgentSecretPath,
};
use golem_common::model::application::{ApplicationId, ApplicationName};
use golem_common::model::auth::TokenSecret;
use golem_common::model::component::{
    AgentFilePath, AgentFilePermissions, ComponentId, ComponentName, ComponentRevision,
    InitialAgentFile,
};
use golem_common::model::deployment::DeploymentRevision;
use golem_common::model::domain_registration::Domain;
use golem_common::model::entity::{
    EntityActivationPolicy, EntityActivationSource, EntityInvocationPlanLayer, ExecutableTarget,
    FilesystemCapability,
};
use golem_common::model::environment::EnvironmentId;
use golem_common::model::json::NormalizedJsonValue;
use golem_common::model::quota::{ResourceDefinition, ResourceDefinitionId, ResourceName};
use golem_common::model::tool::{
    CompiledToolBinding, ConfigKeyScope, HostToolId, RegisteredTool, SecretKeyScope,
    ToolBindingInput, ToolBindingOwner, ToolDeploymentState, ToolFilesystemAccess, ToolName,
    ToolProvisionConfig, ToolSource,
};
use golem_common::model::tool_middleware::CompiledToolMiddlewareChain;
use golem_common::model::tool_middleware::{
    CompiledToolMiddlewareOccurrence, RegisteredToolMiddleware, ToolMiddlewareInstallation,
    ToolMiddlewareName, ToolMiddlewareSource,
};
use golem_common::schema::tool::{
    CommandNode, CommandTree, Doc, Globals, MonomorphicToolMiddlewareScope, Tool, ToolMiddleware,
    ToolMiddlewareScope,
};
use golem_common::schema::{SchemaGraph, SchemaValue, TypedSchemaValue};
use golem_service_base::clients::registry::{
    RegistryInvalidationHandler, RegistryService, RegistryServiceError, ResourceUsageUpdate,
};
use golem_service_base::custom_api::CompiledRoutes;
use golem_service_base::mcp::CompiledMcp;
use golem_service_base::model::agent_secret::AgentSecret;
use golem_service_base::model::auth::AuthCtx;
use golem_service_base::model::component::Component;
use golem_service_base::model::environment::EnvironmentState;
use golem_service_base::model::{AccountResourceLimits, ResourceLimits};
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::Arc;
use std::time::Duration;
use test_r::{test, timeout};

#[test]
fn mcp_binding_scopes_intersect_environment_and_agent_authority() {
    use golem_common::model::agent_config::CanonicalAgentConfigPath;
    use golem_common::model::tool::{ConfigKeyScope, ToolBindingInput};
    let config = |names: &[&str]| {
        ConfigKeyScope::Keys(
            names
                .iter()
                .map(|name| CanonicalAgentConfigPath(vec![(*name).to_string()]))
                .collect(),
        )
    };
    let secrets = |names: &[&str]| {
        SecretKeyScope::Keys(
            names
                .iter()
                .map(|name| CanonicalAgentSecretPath(vec![(*name).to_string()]))
                .collect(),
        )
    };
    let environment = ToolBindingInput {
        config_keys_readable: config(&["shared", "environment"]),
        secret_keys_readable: secrets(&["shared", "environment"]),
        secret_keys_revealable: secrets(&["shared", "unreadable"]),
        ..Default::default()
    };
    let agent = ToolBindingInput {
        config_keys_readable: config(&["shared", "agent"]),
        secret_keys_readable: secrets(&["shared", "agent"]),
        secret_keys_revealable: secrets(&["agent", "unreadable"]),
        ..Default::default()
    };
    assert_eq!(
        super::mcp_binding_scopes(Some(&environment), Some(&agent)),
        (config(&["shared"]), secrets(&["shared"]), secrets(&[]),)
    );
    let middleware_only = ToolBindingInput {
        middleware: Some(vec![]),
        ..Default::default()
    };
    for owner in [None, Some(&middleware_only)] {
        assert_eq!(
            super::mcp_binding_scopes(Some(&environment), owner),
            (
                config(&["shared", "environment"]),
                secrets(&["shared", "environment"]),
                secrets(&["shared"]),
            )
        );
    }
    assert_eq!(
        super::mcp_binding_scopes(None, Some(&agent)),
        (
            config(&["shared", "agent"]),
            secrets(&["shared", "agent"]),
            secrets(&["agent"]),
        )
    );
    assert_eq!(
        super::mcp_binding_scopes(None, None),
        (config(&[]), secrets(&[]), secrets(&[]))
    );
}

fn registered_tool(name: &str, deployment_revision: DeploymentRevision) -> RegisteredTool {
    RegisteredTool {
        deployment_revision,
        release_id: None,
        definition: Tool {
            version: "1.0.0".to_string(),
            commands: CommandTree {
                nodes: vec![CommandNode {
                    name: name.to_string(),
                    aliases: Vec::new(),
                    doc: Doc::default(),
                    globals: Globals::default(),
                    subcommands: Vec::new(),
                    body: None,
                }],
            },
            schema: SchemaGraph::empty(),
        },
        provision: ToolProvisionConfig::default(),
        source: ToolSource::Component {
            component_id: ComponentId::new(),
            component_revision: ComponentRevision::try_from(1_u64).unwrap(),
            component_name: ComponentName(format!("tools:{name}")),
        },
        owner_account_id: AccountId::new(),
        owner_account_email: AccountEmail::new("owner@example.com"),
        metadata_version: "0.1.0".to_string(),
        metadata_digest: Default::default(),
        component_bindings: BTreeMap::new(),
    }
}

fn binding(
    agent_type: &AgentTypeName,
    tool_name: &ToolName,
    registered_tool: &RegisteredTool,
) -> CompiledToolBinding {
    CompiledToolBinding {
        deployment_revision: registered_tool.deployment_revision,
        release_id: registered_tool.release_id,
        owner: agent_owner(agent_type),
        tool_name: tool_name.clone(),
        version: registered_tool.definition.version.clone(),
        metadata_version: registered_tool.metadata_version.clone(),
        metadata_digest: registered_tool.metadata_digest,
        account_id: registered_tool.owner_account_id,
        account_email: registered_tool.owner_account_email.clone(),
        parameters: NormalizedJsonValue::new(serde_json::json!({})),
        config_keys_readable: Default::default(),
        secret_keys_readable: SecretKeyScope::All,
        secret_keys_revealable: SecretKeyScope::All,
        filesystem_access: ToolFilesystemAccess::Unset,
        source: registered_tool.source.clone(),
    }
}

fn deployment_state() -> (ToolDeploymentState, AgentTypeName, AgentTypeName) {
    let deployment_revision = DeploymentRevision::try_from(1_u64).unwrap();
    let agent_a = AgentTypeName("AgentA".to_string());
    let agent_b = AgentTypeName("AgentB".to_string());
    let alpha_name = ToolName::try_from("alpha").unwrap();
    let beta_name = ToolName::try_from("beta").unwrap();
    let unbound_name = ToolName::try_from("unbound").unwrap();
    let alpha = registered_tool(alpha_name.as_str(), deployment_revision);
    let beta = registered_tool(beta_name.as_str(), deployment_revision);
    let unbound = registered_tool(unbound_name.as_str(), deployment_revision);

    (
        ToolDeploymentState {
            deployment_revision,
            registered_tools: BTreeMap::from([
                (alpha_name.clone(), alpha.clone()),
                (beta_name.clone(), beta.clone()),
                (unbound_name, unbound),
            ]),
            tool_bindings: BTreeMap::from([
                (
                    agent_owner(&agent_a),
                    BTreeMap::from([
                        (alpha_name.clone(), binding(&agent_a, &alpha_name, &alpha)),
                        (beta_name.clone(), binding(&agent_a, &beta_name, &beta)),
                    ]),
                ),
                (
                    agent_owner(&agent_b),
                    BTreeMap::from([(beta_name.clone(), binding(&agent_b, &beta_name, &beta))]),
                ),
            ]),
            mcp_imports: Vec::new(),
            tool_middleware_configuration: Default::default(),
            registered_tool_middlewares: BTreeMap::new(),
            tool_middleware_chains: BTreeMap::new(),
        },
        agent_a,
        agent_b,
    )
}

fn agent_owner(agent_type_name: &AgentTypeName) -> ToolBindingOwner {
    ToolBindingOwner::AgentType {
        agent_type_name: agent_type_name.clone(),
    }
}

fn ready_activation(
    deployment: &ToolDeploymentState,
    agent_type: &AgentTypeName,
    tool_name: &ToolName,
) -> ToolActivationSnapshot {
    match get_tool_activation_from_deployment(Some(deployment), &agent_owner(agent_type), tool_name)
        .unwrap()
    {
        ToolActivationOutcome::Ready(activation) => *activation,
        outcome => panic!("expected ready activation, got {outcome:?}"),
    }
}

fn empty_deployment(revision: DeploymentRevision) -> ToolDeploymentState {
    ToolDeploymentState {
        deployment_revision: revision,
        registered_tools: BTreeMap::new(),
        tool_bindings: BTreeMap::new(),
        mcp_imports: Vec::new(),
        tool_middleware_configuration: Default::default(),
        registered_tool_middlewares: BTreeMap::new(),
        tool_middleware_chains: BTreeMap::new(),
    }
}

fn monomorphic_middleware(
    deployment_revision: DeploymentRevision,
    expected: Tool,
    presented_name: &str,
) -> (
    ToolMiddlewareName,
    RegisteredToolMiddleware,
    ToolMiddlewareInstallation,
) {
    let name = ToolMiddlewareName::try_from("project").unwrap();
    let mut presented = expected.clone();
    presented.commands.nodes[0].name = presented_name.to_string();
    let registration = RegisteredToolMiddleware {
        deployment_revision,
        release_id: None,
        definition: ToolMiddleware {
            name: name.to_string(),
            version: "1.0.0".to_string(),
            aliases: Vec::new(),
            doc: Doc::default(),
            parameter_schema: SchemaGraph::empty(),
            scope: ToolMiddlewareScope::Monomorphic(Box::new(MonomorphicToolMiddlewareScope {
                presented,
                expected: Some(expected),
            })),
        },
        provision: ToolProvisionConfig::default(),
        source: ToolMiddlewareSource::Component {
            component_id: ComponentId::new(),
            component_revision: ComponentRevision::INITIAL,
            component_name: ComponentName("middleware:project".to_string()),
        },
        owner_account_id: AccountId::new(),
        owner_account_email: AccountEmail::new("middleware@example.com"),
        metadata_version: "0.1.0".to_string(),
        metadata_digest: Default::default(),
    };
    let installation = ToolMiddlewareInstallation {
        name: name.clone(),
        version: Some("1.0.0".to_string()),
        parameters: NormalizedJsonValue::new(serde_json::json!({})),
        account: Some(AccountEmail::new("middleware@example.com")),
        secret_keys_readable: None,
        secret_keys_revealable: None,
        filesystem_access: ToolFilesystemAccess::Denied,
    };
    (name, registration, installation)
}

#[test]
fn dynamic_admission_compiles_projected_metadata_and_rejects_changed_upstream_contract() {
    use golem_common::model::agent_config::CanonicalAgentConfigPath;
    use golem_common::model::component_metadata::ComponentMetadata;
    use golem_common::model::environment::EnvironmentName;
    use golem_common::model::mcp_import::McpImportSource;
    use golem_mcp_import::tool::{Limits, ProjectedTool};

    let revision = DeploymentRevision::INITIAL;
    let agent = AgentTypeName("Agent".to_string());
    let owner = agent_owner(&agent);
    let projected = ProjectedTool::new(
        &serde_json::json!({
            "name": "search",
            "description": "Search the genuine upstream catalog",
            "inputSchema": {
                "type": "object",
                "properties": { "query": { "type": "string", "description": "search text" } },
                "required": ["query"],
                "additionalProperties": false
            },
            "outputSchema": { "type": "string" }
        }),
        "search",
        Limits::default(),
    )
    .unwrap();
    let tool_name = ToolName::try_from("search").unwrap();
    let (middleware_name, middleware, installation) =
        monomorphic_middleware(revision, projected.definition.clone(), "presented-search");
    let config_key = CanonicalAgentConfigPath(vec!["allowed".to_string()]);
    let secret_key = CanonicalAgentSecretPath(vec!["allowed".to_string()]);
    let mut deployment = empty_deployment(revision);
    deployment
        .registered_tool_middlewares
        .insert(middleware_name, middleware);
    deployment
        .tool_middleware_configuration
        .environment_bindings
        .insert(
            tool_name.clone(),
            ToolBindingInput {
                config_keys_readable: ConfigKeyScope::All,
                secret_keys_readable: SecretKeyScope::All,
                secret_keys_revealable: SecretKeyScope::All,
                middleware: Some(vec![installation]),
                ..Default::default()
            },
        );
    deployment
        .tool_middleware_configuration
        .agent_bindings
        .insert(
            agent.clone(),
            BTreeMap::from([(
                tool_name.clone(),
                ToolBindingInput {
                    config_keys_readable: ConfigKeyScope::Keys([config_key].into()),
                    secret_keys_readable: SecretKeyScope::Keys([secret_key.clone()].into()),
                    secret_keys_revealable: SecretKeyScope::Keys([secret_key].into()),
                    ..Default::default()
                },
            )]),
        );
    let component = Component {
        id: ComponentId::new(),
        revision: ComponentRevision::INITIAL,
        environment_id: EnvironmentId::new(),
        component_name: ComponentName("agent".to_string()),
        hash: Default::default(),
        application_id: ApplicationId::new(),
        account_id: AccountId::new(),
        account_email: AccountEmail::new("owner@example.com"),
        application_name: ApplicationName("app".to_string()),
        environment_name: EnvironmentName::try_from("test").unwrap(),
        component_size: 0,
        metadata: ComponentMetadata::default(),
        created_at: chrono::Utc::now(),
        wasm_hash: Default::default(),
        object_store_key: String::new(),
    };
    let source = McpImportSource {
        environment_id: component.environment_id,
        deployment_revision: revision,
        import_index: 0,
        upstream_tool_name: String::new(),
    };

    let activation = super::tool_activation_from_mcp(
        source.clone(),
        "2025-06-18".to_string(),
        projected.clone(),
        &deployment,
        &component,
        &owner,
    )
    .unwrap();
    assert_eq!(
        activation.registered_tool().definition,
        projected.definition
    );
    let chain = activation.middleware_chain().unwrap();
    assert_eq!(chain.effective_definition.name(), Some("presented-search"));
    assert_eq!(chain.occurrences.len(), 1);
    let occurrence = &chain.occurrences[0];
    assert_eq!(
        occurrence.config_keys_readable,
        activation.binding().config_keys_readable
    );
    assert_eq!(
        occurrence.secret_keys_readable,
        activation.binding().secret_keys_readable
    );
    assert_eq!(
        occurrence.secret_keys_revealable,
        activation.binding().secret_keys_revealable
    );

    let changed = ProjectedTool::new(
        &serde_json::json!({
            "name": "search",
            "inputSchema": {
                "type": "object",
                "properties": { "query": { "type": "integer" } },
                "required": ["query"],
                "additionalProperties": false
            },
            "outputSchema": { "type": "string" }
        }),
        "search",
        Limits::default(),
    )
    .unwrap();
    let error = super::tool_activation_from_mcp(
        source,
        "2025-06-18".to_string(),
        changed,
        &deployment,
        &component,
        &owner,
    )
    .unwrap_err();
    assert!(matches!(
        error,
        ToolDiscoveryError::InconsistentSnapshot { .. }
    ));
    assert!(
        error
            .to_string()
            .contains("middleware for dynamic tool 'search' is incompatible")
    );
}

#[test]
fn accepted_tool_activation_snapshot_round_trip_preserves_binding_identity() {
    let (mut deployment, agent, _) = deployment_state();
    let tool_name = ToolName::try_from("alpha").unwrap();
    let middleware_name = ToolMiddlewareName::try_from("audit").unwrap();
    let middleware = RegisteredToolMiddleware {
        deployment_revision: deployment.deployment_revision,
        release_id: None,
        definition: ToolMiddleware {
            name: middleware_name.to_string(),
            version: "1.0.0".to_string(),
            aliases: Vec::new(),
            doc: Doc::default(),
            scope: ToolMiddlewareScope::Universal,
            parameter_schema: SchemaGraph::empty(),
        },
        provision: ToolProvisionConfig::default(),
        source: ToolMiddlewareSource::Component {
            component_id: ComponentId::new(),
            component_revision: ComponentRevision::try_from(1_u64).unwrap(),
            component_name: ComponentName("middleware:audit".to_string()),
        },
        owner_account_id: AccountId::new(),
        owner_account_email: AccountEmail::new("middleware@example.com"),
        metadata_version: "0.1.0".to_string(),
        metadata_digest: Default::default(),
    };
    let middleware_parameter_schema = middleware.definition.parameter_schema.clone();
    let effective_definition = deployment.registered_tools[&tool_name].definition.clone();
    let owner = agent_owner(&agent);
    deployment
        .registered_tool_middlewares
        .insert(middleware_name, middleware.clone());
    deployment.tool_middleware_chains.insert(
        owner.clone(),
        BTreeMap::from([(
            tool_name.clone(),
            CompiledToolMiddlewareChain {
                deployment_revision: deployment.deployment_revision,
                owner,
                tool_name: tool_name.clone(),
                effective_definition: effective_definition.clone(),
                occurrences: vec![CompiledToolMiddlewareOccurrence {
                    middleware,
                    parameters: TypedSchemaValue::new(
                        middleware_parameter_schema,
                        SchemaValue::Record { fields: Vec::new() },
                    ),
                    provision: ToolProvisionConfig::default(),
                    config_keys_readable: Default::default(),
                    secret_keys_readable: SecretKeyScope::All,
                    secret_keys_revealable: SecretKeyScope::All,
                    filesystem_access: ToolFilesystemAccess::Unset,
                    expected_definition: None,
                    presented_definition: None,
                    next_effective_definition: effective_definition,
                    compatibility: None,
                }],
            },
        )]),
    );
    let activation = ready_activation(&deployment, &agent, &tool_name);
    let expected_revision = activation.binding().deployment_revision;
    let expected_metadata_digest = activation.binding().metadata_digest;

    let encoded = golem_common::serialization::serialize(&activation).unwrap();
    let decoded: ToolActivationSnapshot =
        golem_common::serialization::deserialize(&encoded).unwrap();

    assert_eq!(decoded, activation);
    assert_eq!(decoded.binding().deployment_revision, expected_revision);
    assert_eq!(decoded.binding().metadata_digest, expected_metadata_digest);
    assert_eq!(decoded.middleware_chain().unwrap().occurrences.len(), 1);

    let invalid_policy_is_rejected = |deployment: &ToolDeploymentState| {
        assert!(matches!(
            get_tool_activation_from_deployment(Some(deployment), &agent_owner(&agent), &tool_name),
            Err(ToolDiscoveryError::InconsistentSnapshot { .. })
        ));
    };

    let mut excessive_readable = deployment.clone();
    let binding = excessive_readable
        .tool_bindings
        .get_mut(&agent_owner(&agent))
        .unwrap()
        .get_mut(&tool_name)
        .unwrap();
    binding.secret_keys_readable = SecretKeyScope::Keys(Default::default());
    binding.secret_keys_revealable = SecretKeyScope::Keys(Default::default());
    invalid_policy_is_rejected(&excessive_readable);

    let mut excessive_revealable = deployment.clone();
    excessive_revealable
        .tool_bindings
        .get_mut(&agent_owner(&agent))
        .unwrap()
        .get_mut(&tool_name)
        .unwrap()
        .secret_keys_revealable = SecretKeyScope::Keys(Default::default());
    invalid_policy_is_rejected(&excessive_revealable);

    let mut revealable_outside_readable = deployment;
    revealable_outside_readable
        .tool_middleware_chains
        .get_mut(&agent_owner(&agent))
        .unwrap()
        .get_mut(&tool_name)
        .unwrap()
        .occurrences[0]
        .secret_keys_readable = SecretKeyScope::Keys(Default::default());
    invalid_policy_is_rejected(&revealable_outside_readable);
}

#[test]
fn accessible_tools_join_bindings_and_registrations_in_name_order() {
    let (deployment, agent_a, agent_b) = deployment_state();
    let alpha = ToolName::try_from("alpha").unwrap();
    let beta = ToolName::try_from("beta").unwrap();
    let expected_alpha_component = match &deployment.registered_tools[&alpha].source {
        ToolSource::Component { component_id, .. } => *component_id,
        ToolSource::Host { .. } => panic!("test fixture must be component-backed"),
    };
    let snapshot = ToolDiscoverySnapshot::from(deployment);

    let agent_a_tools =
        get_accessible_tools_from_snapshot(Some(&snapshot), &agent_owner(&agent_a)).unwrap();
    let agent_b_tools =
        get_accessible_tools_from_snapshot(Some(&snapshot), &agent_owner(&agent_b)).unwrap();

    assert_eq!(
        agent_a_tools
            .iter()
            .map(|tool| tool.definition.name().unwrap())
            .collect::<Vec<_>>(),
        vec!["alpha", "beta"]
    );
    assert_eq!(agent_a_tools[0].implemented_by, expected_alpha_component);
    assert_eq!(agent_a_tools[0].lookup_name, "alpha");
    assert_eq!(agent_a_tools[1].lookup_name, "beta");
    assert_eq!(
        agent_b_tools
            .iter()
            .map(|tool| tool.definition.name().unwrap())
            .collect::<Vec<_>>(),
        vec!["beta"]
    );
    assert_eq!(agent_b_tools[0].lookup_name, "beta");
    let beta_for_agent_a =
        get_accessible_tool_from_snapshot(Some(&snapshot), &agent_owner(&agent_a), &beta)
            .unwrap()
            .unwrap();
    let beta_for_agent_b =
        get_accessible_tool_from_snapshot(Some(&snapshot), &agent_owner(&agent_b), &beta)
            .unwrap()
            .unwrap();
    assert!(Arc::ptr_eq(&agent_a_tools[1], &beta_for_agent_a));
    assert_eq!(beta_for_agent_a.as_ref(), beta_for_agent_b.as_ref());
}

#[test]
fn component_baseline_binding_is_independent_from_agent_bindings() {
    let (mut deployment, agent_a, _) = deployment_state();
    let alpha = ToolName::try_from("alpha").unwrap();
    let beta = ToolName::try_from("beta").unwrap();
    let component_id = ComponentId::new();
    let mut baseline_binding = deployment.tool_bindings[&agent_owner(&agent_a)][&alpha].clone();
    baseline_binding.owner = ToolBindingOwner::ComponentBaseline { component_id };
    deployment.tool_bindings.insert(
        baseline_binding.owner.clone(),
        BTreeMap::from([(alpha.clone(), baseline_binding)]),
    );
    let snapshot = ToolDiscoverySnapshot::from(deployment.clone());
    let baseline_owner = ToolBindingOwner::ComponentBaseline { component_id };

    assert!(
        get_accessible_tool_from_snapshot(Some(&snapshot), &baseline_owner, &alpha)
            .unwrap()
            .is_some()
    );
    assert!(
        get_accessible_tool_from_snapshot(Some(&snapshot), &baseline_owner, &beta)
            .unwrap()
            .is_none()
    );
    assert!(matches!(
        get_tool_activation_from_deployment(Some(&deployment), &baseline_owner, &beta).unwrap(),
        ToolActivationOutcome::NotBound
    ));
    assert_eq!(
        get_accessible_tools_from_snapshot(Some(&snapshot), &agent_owner(&agent_a))
            .unwrap()
            .len(),
        2
    );
}

#[test]
fn discovery_projects_effective_definition_per_agent_binding() {
    let (mut deployment, agent_a, agent_b) = deployment_state();
    let beta = ToolName::try_from("beta").unwrap();
    let mut presented_a = deployment.registered_tools[&beta].definition.clone();
    presented_a.commands.nodes[0].name = "adapter-a".to_string();
    let mut presented_b = presented_a.clone();
    presented_b.commands.nodes[0].name = "adapter-b".to_string();
    deployment.tool_middleware_chains = BTreeMap::from([
        (
            agent_owner(&agent_a),
            BTreeMap::from([(
                beta.clone(),
                CompiledToolMiddlewareChain {
                    deployment_revision: deployment.deployment_revision,
                    owner: agent_owner(&agent_a),
                    tool_name: beta.clone(),
                    effective_definition: presented_a,
                    occurrences: Vec::new(),
                },
            )]),
        ),
        (
            agent_owner(&agent_b),
            BTreeMap::from([(
                beta.clone(),
                CompiledToolMiddlewareChain {
                    deployment_revision: deployment.deployment_revision,
                    owner: agent_owner(&agent_b),
                    tool_name: beta.clone(),
                    effective_definition: presented_b,
                    occurrences: Vec::new(),
                },
            )]),
        ),
    ]);

    let snapshot = ToolDiscoverySnapshot::from(deployment);
    let for_a = get_accessible_tool_from_snapshot(Some(&snapshot), &agent_owner(&agent_a), &beta)
        .unwrap()
        .unwrap();
    let for_b = get_accessible_tool_from_snapshot(Some(&snapshot), &agent_owner(&agent_b), &beta)
        .unwrap()
        .unwrap();
    assert_eq!(for_a.lookup_name, "beta");
    assert_eq!(for_b.lookup_name, "beta");
    assert_eq!(for_a.definition.name(), Some("adapter-a"));
    assert_eq!(for_b.definition.name(), Some("adapter-b"));
    assert_eq!(for_a.implemented_by, for_b.implemented_by);
}

#[test]
fn accessible_tool_requires_a_binding_for_the_agent() {
    let (deployment, agent_a, agent_b) = deployment_state();
    let alpha = ToolName::try_from("alpha").unwrap();
    let unbound = ToolName::try_from("unbound").unwrap();
    let snapshot = ToolDiscoverySnapshot::from(deployment);

    assert!(
        get_accessible_tool_from_snapshot(Some(&snapshot), &agent_owner(&agent_a), &alpha)
            .unwrap()
            .is_some()
    );
    assert!(
        get_accessible_tool_from_snapshot(Some(&snapshot), &agent_owner(&agent_b), &alpha)
            .unwrap()
            .is_none()
    );
    assert!(
        get_accessible_tool_from_snapshot(Some(&snapshot), &agent_owner(&agent_a), &unbound)
            .unwrap()
            .is_none()
    );
}

#[test]
fn unknown_valid_tool_name_does_not_change_accessible_set() {
    let (deployment, agent_a, _) = deployment_state();
    let unknown = ToolName::try_from("unknown").unwrap();
    let snapshot = ToolDiscoverySnapshot::from(deployment);
    let before =
        get_accessible_tools_from_snapshot(Some(&snapshot), &agent_owner(&agent_a)).unwrap();

    assert!(
        get_accessible_tool_from_snapshot(Some(&snapshot), &agent_owner(&agent_a), &unknown)
            .unwrap()
            .is_none()
    );
    assert_eq!(
        get_accessible_tools_from_snapshot(Some(&snapshot), &agent_owner(&agent_a)).unwrap(),
        before
    );
}

#[test]
fn missing_deployment_or_agent_bindings_are_empty() {
    let (deployment, _, _) = deployment_state();
    let missing_agent = agent_owner(&AgentTypeName("MissingAgent".to_string()));
    let alpha = ToolName::try_from("alpha").unwrap();
    let snapshot = ToolDiscoverySnapshot::from(deployment);

    assert!(
        get_accessible_tools_from_snapshot(None, &missing_agent)
            .unwrap()
            .is_empty()
    );
    assert!(
        get_accessible_tools_from_snapshot(Some(&snapshot), &missing_agent)
            .unwrap()
            .is_empty()
    );
    assert!(
        get_accessible_tool_from_snapshot(None, &missing_agent, &alpha)
            .unwrap()
            .is_none()
    );
}

#[test]
fn dangling_binding_is_a_permanent_integrity_error() {
    let (mut deployment, agent_a, _) = deployment_state();
    let beta = ToolName::try_from("beta").unwrap();
    deployment.registered_tools.remove(&beta);
    let snapshot = ToolDiscoverySnapshot::from(deployment);

    let list_error =
        get_accessible_tools_from_snapshot(Some(&snapshot), &agent_owner(&agent_a)).unwrap_err();
    let get_error =
        get_accessible_tool_from_snapshot(Some(&snapshot), &agent_owner(&agent_a), &beta)
            .unwrap_err();

    let expected_message = format!(
        "Inconsistent tool deployment snapshot: binding for {:?} references missing tool 'beta'",
        agent_owner(&agent_a)
    );
    assert_eq!(list_error.to_string(), expected_message);
    assert_eq!(get_error.to_string(), expected_message);
    assert!(matches!(
        list_error,
        ToolDiscoveryError::InconsistentSnapshot { .. }
    ));
    assert!(matches!(
        get_error,
        ToolDiscoveryError::InconsistentSnapshot { .. }
    ));
}

#[test]
fn component_dispatch_uses_one_pinned_consumer_snapshot() {
    let (mut deployment, agent_a, _) = deployment_state();
    let alpha = ToolName::try_from("alpha").unwrap();

    let activation = ready_activation(&deployment, &agent_a, &alpha);
    let registered = activation.registered_tool().clone();
    let binding = activation.binding().clone();
    let expected_executable = match &registered.source {
        ToolSource::Component {
            component_id,
            component_revision,
            ..
        } => ExecutableTarget::new(*component_id, *component_revision),
        ToolSource::Host { .. } => panic!("test fixture must be component-backed"),
    };
    deployment.registered_tools.clear();
    deployment.tool_bindings.clear();

    let plan = activation.runtime_plan().unwrap();
    let EntityInvocationPlanLayer::Tool { activation: entity } = plan.layer(0).unwrap() else {
        panic!("component source must produce a component tool leaf")
    };
    assert_eq!(entity.executable_opt(), Some(&expected_executable));
    assert_eq!(entity.deployment_revision(), registered.deployment_revision);
    assert_eq!(entity.filesystem(), FilesystemCapability::Incapable);
    assert_eq!(
        entity.policy(),
        &EntityActivationPolicy::Tool {
            provision: registered.provision,
            binding: Box::new(binding),
            mcp_import: None,
        }
    );
}

#[test]
fn host_dispatch_preserves_exact_handler_and_consumer_policy() {
    let (mut deployment, agent_a, _) = deployment_state();
    let alpha = ToolName::try_from("alpha").unwrap();
    let host_tool_id = HostToolId::try_from("native-search".to_string()).unwrap();
    let implementation_version = "2026.08.28".to_string();
    let registered = deployment.registered_tools.get_mut(&alpha).unwrap();
    registered.source = ToolSource::Host {
        host_tool_id: host_tool_id.clone(),
        implementation_version: implementation_version.clone(),
    };
    registered.provision.env.insert(
        "CONSUMER_CONFIGURATION".to_string(),
        "preserved".to_string(),
    );
    let binding = deployment
        .tool_bindings
        .get_mut(&agent_owner(&agent_a))
        .unwrap()
        .get_mut(&alpha)
        .unwrap();
    binding.source = registered.source.clone();
    binding.parameters = NormalizedJsonValue::new(serde_json::json!({
        "consumer": "parameters"
    }));
    binding.filesystem_access = ToolFilesystemAccess::Allowed;
    let expected_provision = registered.provision.clone();
    let expected_binding = binding.clone();
    let expected_revision = deployment.deployment_revision;

    let activation = ready_activation(&deployment, &agent_a, &alpha);
    let plan = activation.runtime_plan().unwrap();
    let EntityInvocationPlanLayer::Tool {
        activation: planned_leaf,
    } = plan.layer(0).unwrap()
    else {
        panic!("direct host tool plan must contain a native leaf")
    };
    assert!(matches!(
        planned_leaf.source(),
        EntityActivationSource::Host { .. }
    ));
    let EntityActivationSource::Host {
        host_tool_id: actual_host_tool_id,
        implementation_version: actual_implementation_version,
    } = planned_leaf.source()
    else {
        panic!("host source must produce a native tool leaf")
    };
    assert_eq!(actual_host_tool_id, &host_tool_id);
    assert_eq!(actual_implementation_version, &implementation_version);
    assert_eq!(planned_leaf.deployment_revision(), expected_revision);
    assert_eq!(planned_leaf.filesystem(), FilesystemCapability::Capable);
    assert_eq!(
        planned_leaf.policy(),
        &EntityActivationPolicy::Tool {
            provision: expected_provision,
            binding: Box::new(expected_binding),
            mcp_import: None,
        }
    );
}

#[test]
fn activation_lookup_uses_explicit_filesystem_verdict() {
    let (mut deployment, agent_a, _) = deployment_state();
    let alpha = ToolName::try_from("alpha").unwrap();
    deployment
        .tool_bindings
        .get_mut(&agent_owner(&agent_a))
        .unwrap()
        .get_mut(&alpha)
        .unwrap()
        .filesystem_access = ToolFilesystemAccess::Allowed;

    let activation = ready_activation(&deployment, &agent_a, &alpha);

    assert_eq!(activation.filesystem(), FilesystemCapability::Capable);
}

#[test]
fn activation_lookup_distinguishes_not_registered_from_not_bound() {
    let (deployment, agent_a, _) = deployment_state();
    let unbound = ToolName::try_from("unbound").unwrap();
    let missing = ToolName::try_from("missing").unwrap();

    assert_eq!(
        get_tool_activation_from_deployment(Some(&deployment), &agent_owner(&agent_a), &unbound)
            .unwrap(),
        ToolActivationOutcome::NotBound
    );
    assert_eq!(
        get_tool_activation_from_deployment(Some(&deployment), &agent_owner(&agent_a), &missing)
            .unwrap(),
        ToolActivationOutcome::NotRegistered
    );
    assert_eq!(
        get_tool_activation_from_deployment(None, &agent_owner(&agent_a), &unbound).unwrap(),
        ToolActivationOutcome::NotRegistered
    );
}

#[test]
fn activation_lookup_rejects_files_with_explicit_filesystem_denial() {
    let (mut deployment, agent_a, _) = deployment_state();
    let alpha = ToolName::try_from("alpha").unwrap();
    deployment
        .tool_bindings
        .get_mut(&agent_owner(&agent_a))
        .unwrap()
        .get_mut(&alpha)
        .unwrap()
        .filesystem_access = ToolFilesystemAccess::Denied;
    deployment
        .registered_tools
        .get_mut(&alpha)
        .unwrap()
        .provision
        .files
        .push(InitialAgentFile {
            content_hash: AgentFileContentHash(golem_common::model::diff::Hash::empty()),
            path: AgentFilePath::from_rel_str("fixture").unwrap(),
            permissions: AgentFilePermissions::ReadOnly,
            size: 0,
        });

    let result =
        get_tool_activation_from_deployment(Some(&deployment), &agent_owner(&agent_a), &alpha);

    assert!(matches!(
        result,
        Err(ToolDiscoveryError::InconsistentSnapshot { .. })
    ));
}

#[test]
fn activation_lookup_rejects_cross_revision_pairs() {
    let (mut deployment, agent_a, _) = deployment_state();
    let alpha = ToolName::try_from("alpha").unwrap();
    deployment
        .tool_bindings
        .get_mut(&agent_owner(&agent_a))
        .unwrap()
        .get_mut(&alpha)
        .unwrap()
        .deployment_revision = DeploymentRevision::try_from(2_u64).unwrap();

    let error =
        get_tool_activation_from_deployment(Some(&deployment), &agent_owner(&agent_a), &alpha)
            .unwrap_err();

    assert!(matches!(
        error,
        ToolDiscoveryError::InconsistentSnapshot { .. }
    ));
}

#[test]
fn activation_lookup_rejects_binding_owner_mismatch() {
    let (mut deployment, agent_a, agent_b) = deployment_state();
    let alpha = ToolName::try_from("alpha").unwrap();
    deployment
        .tool_bindings
        .get_mut(&agent_owner(&agent_a))
        .unwrap()
        .get_mut(&alpha)
        .unwrap()
        .owner = agent_owner(&agent_b);

    assert!(matches!(
        get_tool_activation_from_deployment(Some(&deployment), &agent_owner(&agent_a), &alpha),
        Err(ToolDiscoveryError::InconsistentSnapshot { .. })
    ));
}

#[test]
fn activation_lookup_rejects_mismatched_release_identity() {
    let (mut deployment, agent_a, _) = deployment_state();
    let alpha = ToolName::try_from("alpha").unwrap();
    deployment
        .tool_bindings
        .get_mut(&agent_owner(&agent_a))
        .unwrap()
        .get_mut(&alpha)
        .unwrap()
        .release_id = Some(golem_common::model::tool_release::ToolReleaseId::new());

    assert!(matches!(
        get_tool_activation_from_deployment(Some(&deployment), &agent_owner(&agent_a), &alpha),
        Err(ToolDiscoveryError::InconsistentSnapshot { .. })
    ));
}

#[test]
fn activation_lookup_rejects_mismatched_metadata_digest() {
    let (mut deployment, agent_a, _) = deployment_state();
    let alpha = ToolName::try_from("alpha").unwrap();
    let registered = &deployment.registered_tools[&alpha];
    let mismatched_digest = golem_common::model::tool_release::tool_metadata_digest(
        "other-metadata-version",
        &registered.definition,
    )
    .unwrap();
    deployment
        .tool_bindings
        .get_mut(&agent_owner(&agent_a))
        .unwrap()
        .get_mut(&alpha)
        .unwrap()
        .metadata_digest = mismatched_digest;

    assert!(matches!(
        get_tool_activation_from_deployment(Some(&deployment), &agent_owner(&agent_a), &alpha),
        Err(ToolDiscoveryError::InconsistentSnapshot { .. })
    ));
}

#[test]
fn activation_lookup_rejects_registration_under_the_wrong_name() {
    let (mut deployment, agent_a, _) = deployment_state();
    let alpha = ToolName::try_from("alpha").unwrap();
    deployment
        .registered_tools
        .get_mut(&alpha)
        .unwrap()
        .definition
        .commands
        .nodes[0]
        .name = "other".to_string();

    let error =
        get_tool_activation_from_deployment(Some(&deployment), &agent_owner(&agent_a), &alpha)
            .unwrap_err();

    assert!(matches!(
        error,
        ToolDiscoveryError::InconsistentSnapshot { .. }
    ));
}

#[test]
fn single_lookup_does_not_scan_unrelated_dangling_bindings() {
    let (mut deployment, agent_a, _) = deployment_state();
    let alpha = ToolName::try_from("alpha").unwrap();
    let beta = ToolName::try_from("beta").unwrap();
    deployment.registered_tools.remove(&beta);
    let snapshot = ToolDiscoverySnapshot::from(deployment);

    assert!(
        get_accessible_tool_from_snapshot(Some(&snapshot), &agent_owner(&agent_a), &alpha)
            .unwrap()
            .is_some()
    );
}

#[test]
#[timeout("30s")]
async fn tool_discovery_invalidation_cannot_be_undone_by_an_in_flight_fill() {
    let cache = Arc::new(ToolDiscoveryCache::new(
        8,
        Duration::from_secs(60),
        Duration::from_secs(60),
    ));
    let environment_id = golem_common::model::environment::EnvironmentId::new();
    let key = (
        environment_id,
        ComponentId::new(),
        ComponentRevision::try_from(1_u64).unwrap(),
    );
    let stale_snapshot = Arc::new(CachedToolDeployment::from(deployment_state().0));
    let fresh_snapshot = Arc::new(CachedToolDeployment::from(deployment_state().0));
    let lookup_started = Arc::new(tokio::sync::Notify::new());
    let release_lookup = Arc::new(tokio::sync::Notify::new());

    let lookup = tokio::spawn({
        let cache = cache.clone();
        let stale_snapshot = stale_snapshot.clone();
        let lookup_started = lookup_started.clone();
        let release_lookup = release_lookup.clone();
        async move {
            cache
                .get_or_insert(&key, move || async move {
                    lookup_started.notify_one();
                    release_lookup.notified().await;
                    Ok(Some(stale_snapshot))
                })
                .await
                .unwrap()
                .unwrap()
        }
    });
    lookup_started.notified().await;

    let mut invalidation = Box::pin(cache.invalidate_environment(environment_id));
    assert!(futures::poll!(invalidation.as_mut()).is_pending());
    assert!(cache.invalidation_guard.try_read().is_err());

    release_lookup.notify_one();
    let loaded_stale_snapshot = lookup.await.unwrap();
    assert!(Arc::ptr_eq(&loaded_stale_snapshot, &stale_snapshot));
    invalidation.await;

    let loaded_fresh_snapshot = cache
        .get_or_insert(&key, {
            let fresh_snapshot = fresh_snapshot.clone();
            move || async move { Ok(Some(fresh_snapshot)) }
        })
        .await
        .unwrap()
        .unwrap();
    assert!(Arc::ptr_eq(&loaded_fresh_snapshot, &fresh_snapshot));
}

#[test]
#[timeout("30s")]
async fn cancelled_tool_discovery_lookup_does_not_wedge_invalidation() {
    let cache = Arc::new(ToolDiscoveryCache::new(
        8,
        Duration::from_secs(60),
        Duration::from_secs(60),
    ));
    let environment_id = golem_common::model::environment::EnvironmentId::new();
    let key = (
        environment_id,
        ComponentId::new(),
        ComponentRevision::try_from(1_u64).unwrap(),
    );
    let stale_snapshot = Arc::new(CachedToolDeployment::from(deployment_state().0));
    let fresh_snapshot = Arc::new(CachedToolDeployment::from(deployment_state().0));
    let lookup_started = Arc::new(tokio::sync::Notify::new());
    let release_lookup = Arc::new(tokio::sync::Notify::new());

    let lookup = tokio::spawn({
        let cache = cache.clone();
        let lookup_started = lookup_started.clone();
        let release_lookup = release_lookup.clone();
        async move {
            cache
                .get_or_insert(&key, move || async move {
                    lookup_started.notify_one();
                    release_lookup.notified().await;
                    Ok(Some(stale_snapshot))
                })
                .await
        }
    });
    lookup_started.notified().await;
    lookup.abort();
    let cancellation = match lookup.await {
        Err(error) => error,
        Ok(_) => panic!("aborted lookup completed successfully"),
    };
    assert!(cancellation.is_cancelled());

    let invalidation_started = Arc::new(tokio::sync::Notify::new());
    let invalidation = tokio::spawn({
        let cache = cache.clone();
        let invalidation_started = invalidation_started.clone();
        async move {
            invalidation_started.notify_one();
            cache.invalidate_environment(environment_id).await;
        }
    });
    invalidation_started.notified().await;
    for _ in 0..100 {
        if cache.invalidation_guard.try_read().is_err() {
            break;
        }
        tokio::task::yield_now().await;
    }
    assert!(cache.invalidation_guard.try_read().is_err());

    release_lookup.notify_one();
    invalidation.await.unwrap();

    let loaded_fresh_snapshot = cache
        .get_or_insert(&key, {
            let fresh_snapshot = fresh_snapshot.clone();
            move || async move { Ok(Some(fresh_snapshot)) }
        })
        .await
        .unwrap()
        .unwrap();
    assert!(Arc::ptr_eq(&loaded_fresh_snapshot, &fresh_snapshot));
}

#[test]
#[timeout("30s")]
async fn tool_discovery_cache_retains_background_ttl_eviction() {
    let cache = ToolDiscoveryCache::new(8, Duration::from_millis(20), Duration::from_millis(5));
    let key = (
        golem_common::model::environment::EnvironmentId::new(),
        ComponentId::new(),
        ComponentRevision::try_from(1_u64).unwrap(),
    );
    let stale_snapshot = Arc::new(CachedToolDeployment::from(deployment_state().0));
    let fresh_snapshot = Arc::new(CachedToolDeployment::from(deployment_state().0));

    let loaded_stale_snapshot = cache
        .get_or_insert(&key, {
            let stale_snapshot = stale_snapshot.clone();
            move || async move { Ok(Some(stale_snapshot)) }
        })
        .await
        .unwrap()
        .unwrap();
    assert!(Arc::ptr_eq(&loaded_stale_snapshot, &stale_snapshot));

    tokio::time::sleep(Duration::from_millis(100)).await;

    let loaded_fresh_snapshot = cache
        .get_or_insert(&key, {
            let fresh_snapshot = fresh_snapshot.clone();
            move || async move { Ok(Some(fresh_snapshot)) }
        })
        .await
        .unwrap()
        .unwrap();
    assert!(Arc::ptr_eq(&loaded_fresh_snapshot, &fresh_snapshot));
}

type ExactKey = (EnvironmentId, DeploymentRevision);
type LiveKey = (EnvironmentId, ComponentId, ComponentRevision);
type DeploymentResponse = Result<Option<ToolDeploymentState>, RegistryServiceError>;

#[derive(Default)]
struct MockRegistryService {
    exact: std::sync::Mutex<HashMap<ExactKey, VecDeque<DeploymentResponse>>>,
    live: std::sync::Mutex<HashMap<LiveKey, Option<ToolDeploymentState>>>,
    exact_calls: std::sync::Mutex<Vec<ExactKey>>,
    live_calls: std::sync::Mutex<Vec<LiveKey>>,
    exact_gate: std::sync::Mutex<Option<(Arc<tokio::sync::Notify>, Arc<tokio::sync::Notify>)>>,
}

impl MockRegistryService {
    fn service(self: &Arc<Self>) -> Arc<GrpcEnvironmentStateService> {
        Arc::new(GrpcEnvironmentStateService::new(
            self.clone(),
            32,
            Duration::from_secs(60),
            Duration::from_secs(60),
        ))
    }

    fn script_exact(&self, key: ExactKey, responses: impl IntoIterator<Item = DeploymentResponse>) {
        self.exact
            .lock()
            .unwrap()
            .insert(key, responses.into_iter().collect());
    }
}

#[async_trait::async_trait]
impl RegistryService for MockRegistryService {
    async fn resolve_mcp_import(
        &self,
        _: &golem_common::model::mcp_import::McpImportSource,
        _: &AuthCtx,
        _: bool,
    ) -> Result<golem_service_base::model::mcp_import::McpImportObservation, RegistryServiceError>
    {
        panic!("unexpected MCP discovery")
    }
    async fn get_mcp_runtime_credential(
        &self,
        _: &golem_common::model::mcp_import::McpImportSource,
        _: &AuthCtx,
    ) -> Result<golem_service_base::clients::registry::McpRuntimeCredential, RegistryServiceError>
    {
        panic!("unexpected MCP credential request")
    }
    async fn report_mcp_resource_unauthorized(
        &self,
        _: &golem_common::model::mcp_import::McpImportSource,
        _: &AuthCtx,
        _: Option<uuid::Uuid>,
    ) -> Result<(), RegistryServiceError> {
        panic!("unexpected MCP feedback")
    }
    async fn authenticate_token(&self, _: &TokenSecret) -> Result<AuthCtx, RegistryServiceError> {
        unimplemented!()
    }
    async fn get_resource_limits(
        &self,
        _: AccountId,
    ) -> Result<ResourceLimits, RegistryServiceError> {
        unimplemented!()
    }
    async fn update_worker_connection_limit(
        &self,
        _: AccountId,
        _: &golem_common::model::AgentId,
        _: bool,
    ) -> Result<(), RegistryServiceError> {
        unimplemented!()
    }
    async fn batch_update_resource_usage(
        &self,
        _: HashMap<AccountId, ResourceUsageUpdate>,
    ) -> Result<AccountResourceLimits, RegistryServiceError> {
        unimplemented!()
    }
    async fn download_component(
        &self,
        _: ComponentId,
        _: ComponentRevision,
    ) -> Result<Vec<u8>, RegistryServiceError> {
        unimplemented!()
    }
    async fn get_component_metadata(
        &self,
        _: ComponentId,
        _: ComponentRevision,
    ) -> Result<Component, RegistryServiceError> {
        unimplemented!()
    }
    async fn get_deployed_component_metadata(
        &self,
        _: ComponentId,
    ) -> Result<Component, RegistryServiceError> {
        unimplemented!()
    }
    async fn get_all_deployed_component_revisions(
        &self,
        _: ComponentId,
    ) -> Result<Vec<Component>, RegistryServiceError> {
        unimplemented!()
    }
    async fn resolve_component(
        &self,
        _: AccountId,
        _: ApplicationId,
        _: EnvironmentId,
        _: &str,
    ) -> Result<Component, RegistryServiceError> {
        unimplemented!()
    }
    async fn get_all_agent_types(
        &self,
        _: EnvironmentId,
        _: ComponentId,
        _: ComponentRevision,
    ) -> Result<Vec<RegisteredAgentType>, RegistryServiceError> {
        unimplemented!()
    }
    async fn get_agent_type(
        &self,
        _: EnvironmentId,
        _: ComponentId,
        _: ComponentRevision,
        _: &AgentTypeName,
    ) -> Result<RegisteredAgentType, RegistryServiceError> {
        unimplemented!()
    }
    async fn get_tool_deployment_state(
        &self,
        environment_id: EnvironmentId,
        component_id: ComponentId,
        component_revision: ComponentRevision,
    ) -> Result<Option<ToolDeploymentState>, RegistryServiceError> {
        let key = (environment_id, component_id, component_revision);
        self.live_calls.lock().unwrap().push(key);
        Ok(self.live.lock().unwrap().get(&key).cloned().flatten())
    }
    async fn get_tool_deployment_state_at_revision(
        &self,
        environment_id: EnvironmentId,
        deployment_revision: DeploymentRevision,
    ) -> Result<Option<ToolDeploymentState>, RegistryServiceError> {
        let key = (environment_id, deployment_revision);
        self.exact_calls.lock().unwrap().push(key);
        let gate = self.exact_gate.lock().unwrap().clone();
        if let Some((started, release)) = gate {
            started.notify_one();
            release.notified().await;
        }
        self.exact
            .lock()
            .unwrap()
            .get_mut(&key)
            .and_then(VecDeque::pop_front)
            .unwrap_or_else(|| panic!("unexpected exact lookup: {key:?}"))
    }
    async fn resolve_agent_type_by_names(
        &self,
        _: &ApplicationName,
        _: &golem_common::model::environment::EnvironmentName,
        _: &AgentTypeName,
        _: Option<DeploymentRevision>,
        _: Option<&str>,
        _: &AuthCtx,
    ) -> Result<ResolvedAgentType, RegistryServiceError> {
        unimplemented!()
    }
    async fn get_active_routes_for_domain(
        &self,
        _: &Domain,
    ) -> Result<CompiledRoutes, RegistryServiceError> {
        unimplemented!()
    }
    async fn get_active_compiled_mcps_for_domain(
        &self,
        _: &Domain,
    ) -> Result<CompiledMcp, RegistryServiceError> {
        unimplemented!()
    }
    async fn get_current_environment_state(
        &self,
        _: EnvironmentId,
    ) -> Result<EnvironmentState, RegistryServiceError> {
        unimplemented!()
    }
    async fn get_agent_secret_revision(
        &self,
        _: EnvironmentId,
        _: AgentSecretId,
        _: CanonicalAgentSecretPath,
        _: AgentSecretRevision,
    ) -> Result<Option<AgentSecret>, RegistryServiceError> {
        unimplemented!()
    }
    async fn get_resource_definition_by_id(
        &self,
        _: ResourceDefinitionId,
    ) -> Result<ResourceDefinition, RegistryServiceError> {
        unimplemented!()
    }
    async fn get_resource_definition_by_name(
        &self,
        _: EnvironmentId,
        _: ResourceName,
    ) -> Result<ResourceDefinition, RegistryServiceError> {
        unimplemented!()
    }
    async fn subscribe_registry_invalidations(
        &self,
        _: Option<u64>,
    ) -> Result<
        std::pin::Pin<
            Box<
                dyn futures::Stream<
                        Item = Result<
                            golem_common::model::agent::RegistryInvalidationEvent,
                            RegistryServiceError,
                        >,
                    > + Send,
            >,
        >,
        RegistryServiceError,
    > {
        unimplemented!()
    }
    async fn run_registry_invalidation_event_subscriber(
        &self,
        _: &'static str,
        _: Option<tokio_util::sync::CancellationToken>,
        _: Arc<dyn RegistryInvalidationHandler>,
    ) {
        unimplemented!()
    }
}

#[test]
#[timeout("30s")]
async fn exact_revision_service_keys_results_and_error_classification() {
    let registry = Arc::new(MockRegistryService::default());
    let env_cedar = EnvironmentId::new();
    let env_orchid = EnvironmentId::new();
    let revision_17 = DeploymentRevision::try_from(17_u64).unwrap();
    let revision_43 = DeploymentRevision::try_from(43_u64).unwrap();
    let revision_89 = DeploymentRevision::try_from(89_u64).unwrap();
    registry.script_exact(
        (env_cedar, revision_17),
        [Ok(Some(empty_deployment(revision_17)))],
    );
    registry.script_exact(
        (env_orchid, revision_17),
        [Ok(Some(empty_deployment(revision_17)))],
    );
    registry.script_exact(
        (env_cedar, revision_43),
        [Ok(Some(empty_deployment(revision_43)))],
    );
    registry.script_exact((env_orchid, revision_43), [Ok(None)]);
    registry.script_exact(
        (env_cedar, revision_89),
        [
            Err(RegistryServiceError::InternalServerError(
                "temporary".to_string(),
            )),
            Ok(Some(empty_deployment(revision_89))),
        ],
    );
    let service = registry.service();

    for (environment, revision) in [
        (env_cedar, revision_17),
        (env_orchid, revision_17),
        (env_cedar, revision_43),
    ] {
        let state = service
            .get_tool_deployment_state_at_revision(environment, revision)
            .await
            .unwrap();
        assert_eq!(state.deployment_revision, revision);
        assert!(
            state.registered_tools.is_empty(),
            "valid empty state must be returned"
        );
    }
    assert!(matches!(
        service
            .get_tool_deployment_state_at_revision(env_orchid, revision_43)
            .await,
        Err(ToolDiscoveryError::MissingDeploymentRevision {
            environment_id,
            deployment_revision
        }) if environment_id == env_orchid && deployment_revision == revision_43
    ));
    assert!(matches!(
        service
            .get_tool_deployment_state_at_revision(env_cedar, revision_89)
            .await,
        Err(ToolDiscoveryError::Retrieval(_))
    ));
    assert_eq!(
        service
            .get_tool_deployment_state_at_revision(env_cedar, revision_89)
            .await
            .unwrap()
            .deployment_revision,
        revision_89
    );
    assert_eq!(
        *registry.exact_calls.lock().unwrap(),
        vec![
            (env_cedar, revision_17),
            (env_orchid, revision_17),
            (env_cedar, revision_43),
            (env_orchid, revision_43),
            (env_cedar, revision_89),
            (env_cedar, revision_89),
        ]
    );
}

#[test]
#[timeout("30s")]
async fn exact_revision_service_rejects_a_response_for_another_revision() {
    let registry = Arc::new(MockRegistryService::default());
    let environment = EnvironmentId::new();
    let requested_revision = DeploymentRevision::try_from(17_u64).unwrap();
    let returned_revision = DeploymentRevision::try_from(43_u64).unwrap();
    registry.script_exact(
        (environment, requested_revision),
        [
            Ok(Some(empty_deployment(returned_revision))),
            Ok(Some(empty_deployment(requested_revision))),
        ],
    );
    let service = registry.service();

    let result = service
        .get_tool_deployment_state_at_revision(environment, requested_revision)
        .await;

    assert!(
        matches!(result, Err(ToolDiscoveryError::InconsistentSnapshot { .. })),
        "an exact-revision lookup must not accept state for a different revision: {result:?}"
    );
    assert_eq!(
        service
            .get_tool_deployment_state_at_revision(environment, requested_revision)
            .await
            .unwrap()
            .deployment_revision,
        requested_revision
    );
    assert_eq!(registry.exact_calls.lock().unwrap().len(), 2);
}

#[test]
#[timeout("30s")]
async fn exact_revision_service_coalesces_and_survives_initial_caller_cancellation() {
    let registry = Arc::new(MockRegistryService::default());
    let environment = EnvironmentId::new();
    let revision = DeploymentRevision::try_from(137_u64).unwrap();
    registry.script_exact(
        (environment, revision),
        [Ok(Some(empty_deployment(revision)))],
    );
    let started = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    *registry.exact_gate.lock().unwrap() = Some((started.clone(), release.clone()));
    let service = registry.service();

    let initial = tokio::spawn({
        let service = service.clone();
        async move {
            service
                .get_tool_deployment_state_at_revision(environment, revision)
                .await
        }
    });
    started.notified().await;
    let followers = (0..6)
        .map(|_| {
            let service = service.clone();
            tokio::spawn(async move {
                service
                    .get_tool_deployment_state_at_revision(environment, revision)
                    .await
                    .unwrap()
            })
        })
        .collect::<Vec<_>>();
    tokio::task::yield_now().await;
    initial.abort();
    assert!(initial.await.unwrap_err().is_cancelled());
    release.notify_one();
    let states = futures::future::join_all(followers)
        .await
        .into_iter()
        .map(Result::unwrap)
        .collect::<Vec<_>>();

    assert_eq!(
        registry.exact_calls.lock().unwrap().as_slice(),
        &[(environment, revision)]
    );
    assert!(
        states
            .iter()
            .all(|state| state.deployment_revision == revision)
    );
    assert!(
        states
            .windows(2)
            .all(|states| Arc::ptr_eq(&states[0], &states[1]))
    );
}

#[test]
#[timeout("30s")]
async fn invalidation_refreshes_live_state_but_preserves_exact_state() {
    let registry = Arc::new(MockRegistryService::default());
    let environment = EnvironmentId::new();
    let component = ComponentId::new();
    let component_revision = ComponentRevision::try_from(23_u64).unwrap();
    let revision_n = DeploymentRevision::try_from(211_u64).unwrap();
    let revision_next = DeploymentRevision::try_from(223_u64).unwrap();
    let live_key = (environment, component, component_revision);
    registry
        .live
        .lock()
        .unwrap()
        .insert(live_key, Some(empty_deployment(revision_n)));
    registry.script_exact(
        (environment, revision_n),
        [Ok(Some(empty_deployment(revision_n)))],
    );
    let service = registry.service();

    assert_eq!(
        service
            .get_live_tool_deployment_state(environment, component, component_revision)
            .await
            .unwrap()
            .unwrap()
            .deployment_revision,
        revision_n
    );
    let exact_n = service
        .get_tool_deployment_state_at_revision(environment, revision_n)
        .await
        .unwrap();
    registry
        .live
        .lock()
        .unwrap()
        .insert(live_key, Some(empty_deployment(revision_next)));
    service.invalidate_environment(environment).await;
    assert_eq!(
        service
            .get_live_tool_deployment_state(environment, component, component_revision)
            .await
            .unwrap()
            .unwrap()
            .deployment_revision,
        revision_next
    );
    service.invalidate_all().await;
    let exact_after_both_invalidations = service
        .get_tool_deployment_state_at_revision(environment, revision_n)
        .await
        .unwrap();
    assert!(Arc::ptr_eq(&exact_n, &exact_after_both_invalidations));
    assert_eq!(
        registry.live_calls.lock().unwrap().as_slice(),
        &[live_key, live_key]
    );
    assert_eq!(
        registry.exact_calls.lock().unwrap().as_slice(),
        &[(environment, revision_n)]
    );

    registry.script_exact(
        (environment, revision_n),
        [Ok(Some(empty_deployment(revision_n)))],
    );
    let cold_service = registry.service();
    assert_eq!(
        cold_service
            .get_tool_deployment_state_at_revision(environment, revision_n)
            .await
            .unwrap()
            .deployment_revision,
        revision_n
    );
    assert_eq!(registry.exact_calls.lock().unwrap().len(), 2);
}
