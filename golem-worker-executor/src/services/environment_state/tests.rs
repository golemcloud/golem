use super::{
    CachedToolDeployment, EnvironmentStateService, GrpcEnvironmentStateService,
    ToolActivationOutcome, ToolActivationSnapshot, ToolDiscoveryCache, ToolDiscoveryError,
    ToolDiscoverySnapshot, ToolDispatchTarget, get_accessible_tool_from_snapshot,
    get_accessible_tools_from_snapshot, get_tool_activation_from_deployment,
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
use golem_common::model::entity::{EntityActivationPolicy, ExecutableTarget, FilesystemCapability};
use golem_common::model::environment::EnvironmentId;
use golem_common::model::json::NormalizedJsonValue;
use golem_common::model::quota::{ResourceDefinition, ResourceDefinitionId, ResourceName};
use golem_common::model::tool::{
    CompiledToolBinding, HostToolId, RegisteredTool, SecretKeyScope, ToolDeploymentState,
    ToolFilesystemAccess, ToolName, ToolProvisionConfig, ToolSource,
};
use golem_common::schema::SchemaGraph;
use golem_common::schema::tool::{CommandNode, CommandTree, Doc, Globals, Tool};
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
        agent_type_name: agent_type.clone(),
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
            agent_tool_bindings: BTreeMap::from([
                (
                    agent_a.clone(),
                    BTreeMap::from([
                        (alpha_name.clone(), binding(&agent_a, &alpha_name, &alpha)),
                        (beta_name.clone(), binding(&agent_a, &beta_name, &beta)),
                    ]),
                ),
                (
                    agent_b.clone(),
                    BTreeMap::from([(beta_name.clone(), binding(&agent_b, &beta_name, &beta))]),
                ),
            ]),
            mcp_imports: Vec::new(),
        },
        agent_a,
        agent_b,
    )
}

fn ready_activation(
    deployment: &ToolDeploymentState,
    agent_type: &AgentTypeName,
    tool_name: &ToolName,
) -> ToolActivationSnapshot {
    match get_tool_activation_from_deployment(Some(deployment), agent_type, tool_name).unwrap() {
        ToolActivationOutcome::Ready(activation) => *activation,
        outcome => panic!("expected ready activation, got {outcome:?}"),
    }
}

fn empty_deployment(revision: DeploymentRevision) -> ToolDeploymentState {
    ToolDeploymentState {
        deployment_revision: revision,
        registered_tools: BTreeMap::new(),
        agent_tool_bindings: BTreeMap::new(),
        mcp_imports: Vec::new(),
    }
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

    let agent_a_tools = get_accessible_tools_from_snapshot(Some(&snapshot), &agent_a).unwrap();
    let agent_b_tools = get_accessible_tools_from_snapshot(Some(&snapshot), &agent_b).unwrap();

    assert_eq!(
        agent_a_tools
            .iter()
            .map(|tool| tool.definition.name().unwrap())
            .collect::<Vec<_>>(),
        vec!["alpha", "beta"]
    );
    assert_eq!(agent_a_tools[0].implemented_by, expected_alpha_component);
    assert_eq!(
        agent_b_tools
            .iter()
            .map(|tool| tool.definition.name().unwrap())
            .collect::<Vec<_>>(),
        vec!["beta"]
    );
    let beta_for_agent_a = get_accessible_tool_from_snapshot(Some(&snapshot), &agent_a, &beta)
        .unwrap()
        .unwrap();
    let beta_for_agent_b = get_accessible_tool_from_snapshot(Some(&snapshot), &agent_b, &beta)
        .unwrap()
        .unwrap();
    assert!(Arc::ptr_eq(&agent_a_tools[1], &beta_for_agent_a));
    assert!(Arc::ptr_eq(&beta_for_agent_a, &beta_for_agent_b));
}

#[test]
fn accessible_tool_requires_a_binding_for_the_agent() {
    let (deployment, agent_a, agent_b) = deployment_state();
    let alpha = ToolName::try_from("alpha").unwrap();
    let unbound = ToolName::try_from("unbound").unwrap();
    let snapshot = ToolDiscoverySnapshot::from(deployment);

    assert!(
        get_accessible_tool_from_snapshot(Some(&snapshot), &agent_a, &alpha)
            .unwrap()
            .is_some()
    );
    assert!(
        get_accessible_tool_from_snapshot(Some(&snapshot), &agent_b, &alpha)
            .unwrap()
            .is_none()
    );
    assert!(
        get_accessible_tool_from_snapshot(Some(&snapshot), &agent_a, &unbound)
            .unwrap()
            .is_none()
    );
}

#[test]
fn unknown_valid_tool_name_does_not_change_accessible_set() {
    let (deployment, agent_a, _) = deployment_state();
    let unknown = ToolName::try_from("unknown").unwrap();
    let snapshot = ToolDiscoverySnapshot::from(deployment);
    let before = get_accessible_tools_from_snapshot(Some(&snapshot), &agent_a).unwrap();

    assert!(
        get_accessible_tool_from_snapshot(Some(&snapshot), &agent_a, &unknown)
            .unwrap()
            .is_none()
    );
    assert_eq!(
        get_accessible_tools_from_snapshot(Some(&snapshot), &agent_a).unwrap(),
        before
    );
}

#[test]
fn missing_deployment_or_agent_bindings_are_empty() {
    let (deployment, _, _) = deployment_state();
    let missing_agent = AgentTypeName("MissingAgent".to_string());
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

    let list_error = get_accessible_tools_from_snapshot(Some(&snapshot), &agent_a).unwrap_err();
    let get_error =
        get_accessible_tool_from_snapshot(Some(&snapshot), &agent_a, &beta).unwrap_err();

    let expected_message = concat!(
        "Inconsistent tool deployment snapshot: binding for agent type ",
        "'AgentA' references missing tool 'beta'"
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
    deployment.agent_tool_bindings.clear();

    let ToolDispatchTarget::Component(entity) = activation.into_dispatch_target().unwrap() else {
        panic!("component source must dispatch through component activation")
    };
    assert_eq!(entity.executable_opt(), Some(&expected_executable));
    assert_eq!(entity.deployment_revision(), registered.deployment_revision);
    assert_eq!(entity.filesystem(), FilesystemCapability::Incapable);
    assert_eq!(
        entity.policy(),
        &EntityActivationPolicy::Tool {
            provision: registered.provision,
            binding: Box::new(binding),
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
        .agent_tool_bindings
        .get_mut(&agent_a)
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
    let ToolDispatchTarget::Host {
        host_tool_id: actual_host_tool_id,
        implementation_version: actual_implementation_version,
        deployment_revision,
        provision,
        binding,
        filesystem,
    } = activation.into_dispatch_target().unwrap()
    else {
        panic!("host source must dispatch directly without a component activation")
    };

    assert_eq!(actual_host_tool_id, host_tool_id);
    assert_eq!(actual_implementation_version, implementation_version);
    assert_eq!(deployment_revision, expected_revision);
    assert_eq!(provision, expected_provision);
    assert_eq!(*binding, expected_binding);
    assert_eq!(filesystem, FilesystemCapability::Capable);
}

#[test]
fn activation_lookup_uses_explicit_filesystem_verdict() {
    let (mut deployment, agent_a, _) = deployment_state();
    let alpha = ToolName::try_from("alpha").unwrap();
    deployment
        .agent_tool_bindings
        .get_mut(&agent_a)
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
        get_tool_activation_from_deployment(Some(&deployment), &agent_a, &unbound).unwrap(),
        ToolActivationOutcome::NotBound
    );
    assert_eq!(
        get_tool_activation_from_deployment(Some(&deployment), &agent_a, &missing).unwrap(),
        ToolActivationOutcome::NotRegistered
    );
    assert_eq!(
        get_tool_activation_from_deployment(None, &agent_a, &unbound).unwrap(),
        ToolActivationOutcome::NotRegistered
    );
}

#[test]
fn activation_lookup_rejects_files_with_explicit_filesystem_denial() {
    let (mut deployment, agent_a, _) = deployment_state();
    let alpha = ToolName::try_from("alpha").unwrap();
    deployment
        .agent_tool_bindings
        .get_mut(&agent_a)
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

    let result = get_tool_activation_from_deployment(Some(&deployment), &agent_a, &alpha);

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
        .agent_tool_bindings
        .get_mut(&agent_a)
        .unwrap()
        .get_mut(&alpha)
        .unwrap()
        .deployment_revision = DeploymentRevision::try_from(2_u64).unwrap();

    let error =
        get_tool_activation_from_deployment(Some(&deployment), &agent_a, &alpha).unwrap_err();

    assert!(matches!(
        error,
        ToolDiscoveryError::InconsistentSnapshot { .. }
    ));
}

#[test]
fn activation_lookup_rejects_mismatched_release_identity() {
    let (mut deployment, agent_a, _) = deployment_state();
    let alpha = ToolName::try_from("alpha").unwrap();
    deployment
        .agent_tool_bindings
        .get_mut(&agent_a)
        .unwrap()
        .get_mut(&alpha)
        .unwrap()
        .release_id = Some(golem_common::model::tool_release::ToolReleaseId::new());

    assert!(matches!(
        get_tool_activation_from_deployment(Some(&deployment), &agent_a, &alpha),
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
        .agent_tool_bindings
        .get_mut(&agent_a)
        .unwrap()
        .get_mut(&alpha)
        .unwrap()
        .metadata_digest = mismatched_digest;

    assert!(matches!(
        get_tool_activation_from_deployment(Some(&deployment), &agent_a, &alpha),
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
        get_tool_activation_from_deployment(Some(&deployment), &agent_a, &alpha).unwrap_err();

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
        get_accessible_tool_from_snapshot(Some(&snapshot), &agent_a, &alpha)
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
    let loaded_stale_snapshot = lookup.await.unwrap();
    assert!(Arc::ptr_eq(&loaded_stale_snapshot, &stale_snapshot));
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

#[derive(Default)]
struct MockRegistryService {
    exact: std::sync::Mutex<
        HashMap<ExactKey, VecDeque<Result<Option<ToolDeploymentState>, RegistryServiceError>>>,
    >,
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

    fn script_exact(
        &self,
        key: ExactKey,
        responses: impl IntoIterator<Item = Result<Option<ToolDeploymentState>, RegistryServiceError>>,
    ) {
        self.exact
            .lock()
            .unwrap()
            .insert(key, responses.into_iter().collect());
    }
}

#[async_trait::async_trait]
impl RegistryService for MockRegistryService {
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
