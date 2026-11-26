use super::*;
use golem_common::model::AgentId;
use golem_common::model::account::{AccountEmail, AccountId};
use golem_common::model::agent::{
    AgentTypeName, RegisteredAgentType, RegistryInvalidationEvent, ResolvedAgentType,
};
use golem_common::model::application::{ApplicationId, ApplicationName};
use golem_common::model::auth::TokenSecret;
use golem_common::model::component::ComponentName;
use golem_common::model::component_metadata::ComponentMetadata;
use golem_common::model::deployment::DeploymentRevision;
use golem_common::model::environment::EnvironmentName;
use golem_common::model::mcp_import::McpImportSource;
use golem_common::model::quota::{ResourceDefinition, ResourceDefinitionId, ResourceName};
use golem_service_base::clients::registry::{
    McpRuntimeCredential, RegistryInvalidationHandler, ResourceUsageUpdate,
};
use golem_service_base::custom_api::CompiledRoutes;
use golem_service_base::mcp::CompiledMcp;
use golem_service_base::model::auth::AuthCtx;
use golem_service_base::model::environment::EnvironmentState;
use golem_service_base::model::mcp_import::McpImportObservation;
use golem_service_base::model::{AccountResourceLimits, ResourceLimits};
use std::collections::HashMap;
use std::pin::Pin;
use std::sync::Mutex;
use test_r::test;
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;

/// A tool deployment of `component` that registers `definition` as `tool_name` and binds it to
/// each of `owners`.
pub(crate) fn tool_deployment_state(
    component: &Component,
    tool_name: &str,
    definition: golem_common::schema::tool::Tool,
    owners: impl IntoIterator<Item = golem_common::model::tool::ToolBindingOwner>,
) -> golem_common::model::tool::ToolDeploymentState {
    use golem_common::model::deployment::DeploymentRevision;
    use golem_common::model::tool::{
        CompiledToolBinding, RegisteredTool, SecretKeyScope, ToolDeploymentState, ToolName,
        ToolProvisionConfig, ToolRegistration, ToolSource,
    };
    let tool_name = ToolName::try_from(tool_name).unwrap();
    let registration = ToolRegistration {
        deployment_revision: DeploymentRevision::INITIAL,
        release_id: None,
        definition: definition.clone(),
        provision: ToolProvisionConfig::default(),
        component_bindings: std::collections::BTreeMap::new(),
        source: ToolSource::Component {
            component_id: component.id,
            component_revision: component.revision,
            component_name: component.component_name.clone(),
        },
        owner_account_id: component.account_id,
        owner_account_email: component.account_email.clone(),
        metadata_version: "0.1.0".into(),
        metadata_digest: Default::default(),
    };
    let tool_bindings = owners
        .into_iter()
        .map(|owner| {
            let binding = CompiledToolBinding {
                deployment_revision: DeploymentRevision::INITIAL,
                release_id: None,
                owner: owner.clone(),
                tool_name: tool_name.clone(),
                version: definition.version.clone(),
                metadata_version: registration.metadata_version.clone(),
                metadata_digest: Default::default(),
                account_id: component.account_id,
                account_email: component.account_email.clone(),
                parameters: golem_common::base_model::json::NormalizedJsonValue::new(
                    serde_json::json!({}),
                ),
                config_keys_readable: Default::default(),
                secret_keys_readable: SecretKeyScope::All,
                secret_keys_revealable: SecretKeyScope::All,
                filesystem_access: Default::default(),
                source: registration.source.clone(),
                shell: false,
            };
            (
                owner,
                std::collections::BTreeMap::from([(tool_name.clone(), binding)]),
            )
        })
        .collect();
    ToolDeploymentState {
        deployment_revision: DeploymentRevision::INITIAL,
        registered_tools: std::collections::BTreeMap::from([(
            tool_name,
            RegisteredTool {
                type_id: golem_common::model::type_id::TypeId::try_from(1_u32).unwrap(),
                registration,
            },
        )]),
        tool_bindings,
        mcp_imports: Vec::new(),
        registered_tool_middlewares: std::collections::BTreeMap::new(),
        tool_middleware_chains: std::collections::BTreeMap::new(),
        tool_middleware_configuration: Default::default(),
    }
}

pub(crate) struct TestRegistry {
    revisions: Vec<Component>,
    pub(crate) deployed: Mutex<Option<Component>>,
    pub(crate) deployed_agent_type: Mutex<Option<RegisteredAgentType>>,
    pub(crate) next_lookup: Mutex<Option<Arc<LookupGate>>>,
    pub(crate) deployed_tools: Mutex<Option<golem_common::model::tool::ToolDeploymentState>>,
    pub(crate) tool_state_reads: std::sync::atomic::AtomicUsize,
    pub(crate) auth_contexts: Mutex<HashMap<TokenSecret, AuthCtx>>,
    pub(crate) auth_reads: std::sync::atomic::AtomicUsize,
    pub(crate) resolution_reads: std::sync::atomic::AtomicUsize,
}

/// Holds the next deployed lookup after it read the registry state, until the test releases it.
pub(crate) struct LookupGate {
    pub(crate) started: Semaphore,
    pub(crate) release: Semaphore,
}

impl LookupGate {
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self {
            started: Semaphore::new(0),
            release: Semaphore::new(0),
        })
    }
}

impl TestRegistry {
    async fn pass_lookup_gate(&self) {
        let gate = self.next_lookup.lock().unwrap().take();
        if let Some(gate) = gate {
            gate.started.add_permits(1);
            gate.release.acquire().await.unwrap().forget();
        }
    }

    pub(crate) fn new(revisions: Vec<Component>, deployed: Option<Component>) -> Self {
        Self {
            revisions,
            deployed: Mutex::new(deployed),
            deployed_agent_type: Mutex::new(None),
            next_lookup: Mutex::new(None),
            deployed_tools: Mutex::new(None),
            tool_state_reads: std::sync::atomic::AtomicUsize::new(0),
            auth_contexts: Mutex::new(HashMap::new()),
            auth_reads: std::sync::atomic::AtomicUsize::new(0),
            resolution_reads: std::sync::atomic::AtomicUsize::new(0),
        }
    }
}

pub(crate) fn component() -> Component {
    Component {
        id: ComponentId::new(),
        revision: ComponentRevision::INITIAL,
        environment_id: EnvironmentId::new(),
        component_name: ComponentName("cache-test".into()),
        hash: golem_common::model::diff::Hash::empty(),
        application_id: ApplicationId::new(),
        account_id: AccountId::new(),
        account_email: AccountEmail::new("test@example.com"),
        application_name: ApplicationName("cache-test".into()),
        environment_name: EnvironmentName::try_from("test").unwrap(),
        component_size: 0,
        metadata: ComponentMetadata::default(),
        created_at: jiff::Timestamp::now(),
        wasm_hash: golem_common::model::diff::Hash::empty(),
        package_metadata: None,
        object_store_key: "test".into(),
    }
}

#[async_trait]
impl RegistryService for TestRegistry {
    async fn get_tool_deployment_state(
        &self,
        _: EnvironmentId,
        _: ComponentId,
        _: ComponentRevision,
    ) -> Result<Option<golem_common::model::tool::ToolDeploymentState>, RegistryServiceError> {
        self.tool_state_reads.fetch_add(1, Ordering::SeqCst);
        Ok(self.deployed_tools.lock().unwrap().clone())
    }
    async fn get_component_metadata(
        &self,
        id: ComponentId,
        revision: ComponentRevision,
    ) -> Result<Component, RegistryServiceError> {
        self.revisions
            .iter()
            .find(|c| c.id == id && c.revision == revision)
            .cloned()
            .ok_or_else(|| RegistryServiceError::NotFound("component".into()))
    }
    async fn get_deployed_component_metadata(
        &self,
        _: ComponentId,
    ) -> Result<Component, RegistryServiceError> {
        let result = self
            .deployed
            .lock()
            .unwrap()
            .clone()
            .ok_or_else(|| RegistryServiceError::NotFound("component".into()));
        self.pass_lookup_gate().await;
        result
    }
    async fn get_deployed_agent_type(
        &self,
        _: EnvironmentId,
        agent_type_name: &AgentTypeName,
    ) -> Result<Option<RegisteredAgentType>, RegistryServiceError> {
        let result = self
            .deployed_agent_type
            .lock()
            .unwrap()
            .clone()
            .filter(|registered| registered.agent_type.type_name == *agent_type_name);
        self.pass_lookup_gate().await;
        Ok(result)
    }
    async fn get_all_deployed_component_revisions(
        &self,
        _: ComponentId,
    ) -> Result<Vec<Component>, RegistryServiceError> {
        Ok(self.revisions.clone())
    }
    async fn authenticate_token(
        &self,
        token: &TokenSecret,
    ) -> Result<AuthCtx, RegistryServiceError> {
        self.auth_reads
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let auth = self.auth_contexts.lock().unwrap().get(token).cloned();
        self.pass_lookup_gate().await;
        auth.ok_or_else(|| RegistryServiceError::CouldNotAuthenticate("unknown token".into()))
    }
    async fn get_resource_limits(
        &self,
        _: AccountId,
        _: golem_service_base::clients::registry::ResourceUsageMetering,
    ) -> Result<ResourceLimits, RegistryServiceError> {
        unimplemented!()
    }
    async fn update_worker_connection_limit(
        &self,
        _: AccountId,
        _: &AgentId,
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
    async fn get_mcp_runtime_credential(
        &self,
        _: &McpImportSource,
        _: &AuthCtx,
    ) -> Result<McpRuntimeCredential, RegistryServiceError> {
        unimplemented!()
    }
    async fn report_mcp_resource_unauthorized(
        &self,
        _: &McpImportSource,
        _: &AuthCtx,
        _: Option<uuid::Uuid>,
    ) -> Result<(), RegistryServiceError> {
        unimplemented!()
    }
    async fn resolve_mcp_import(
        &self,
        _: &McpImportSource,
        _: &AuthCtx,
        _: bool,
    ) -> Result<McpImportObservation, RegistryServiceError> {
        unimplemented!()
    }
    async fn resolve_agent_type_by_names(
        &self,
        app: &ApplicationName,
        env: &EnvironmentName,
        agent: &AgentTypeName,
        revision: Option<DeploymentRevision>,
        _: Option<&str>,
        auth: &AuthCtx,
    ) -> Result<ResolvedAgentType, RegistryServiceError> {
        use golem_common::model::card::owner::{AgentOwnerLeafPattern, AgentOwnerPattern};
        use golem_common::model::card::{
            AgentResourcePattern, AgentVerb, ClassPermissionTarget, PermissionTarget,
        };
        self.resolution_reads
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let registered = self.deployed_agent_type.lock().unwrap().clone().unwrap();
        let authorization = auth
            .authorize_permission(&PermissionTarget::Agent(ClassPermissionTarget {
                owner: AgentOwnerPattern::Agent {
                    account: registered.implemented_by.account_email.clone(),
                    application: app.clone(),
                    environment: env.clone(),
                    component: ComponentName(registered.implemented_by.component_name.clone()),
                    agent: AgentOwnerLeafPattern::AgentTypeWildcard(agent.clone()),
                },
                verb: Some(AgentVerb::View),
                resource: AgentResourcePattern::Empty,
            }))
            .map_err(|_| RegistryServiceError::NotFound(agent.0.clone()));
        // Hold denied responses too, so tests can exercise permission grants in flight.
        if let Err(error) = authorization {
            self.pass_lookup_gate().await;
            return Err(error);
        }
        let resolved = ResolvedAgentType {
            registered_agent_type: registered,
            environment_id: self
                .deployed
                .lock()
                .unwrap()
                .as_ref()
                .unwrap()
                .environment_id,
            deployment_revision: revision.unwrap_or(DeploymentRevision::INITIAL),
            current_deployment_revision: revision
                .is_none()
                .then_some(golem_common::model::deployment::CurrentDeploymentRevision::INITIAL),
        };
        self.pass_lookup_gate().await;
        Ok(resolved)
    }
    async fn get_active_routes_for_domain(
        &self,
        _: &golem_common::model::domain_registration::Domain,
    ) -> Result<CompiledRoutes, RegistryServiceError> {
        unimplemented!()
    }
    async fn get_active_compiled_mcps_for_domain(
        &self,
        _: &golem_common::model::domain_registration::Domain,
    ) -> Result<CompiledMcp, RegistryServiceError> {
        unimplemented!()
    }
    async fn get_current_environment_state(
        &self,
        _: EnvironmentId,
    ) -> Result<EnvironmentState, RegistryServiceError> {
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
        Pin<
            Box<
                dyn futures::Stream<Item = Result<RegistryInvalidationEvent, RegistryServiceError>>
                    + Send,
            >,
        >,
        RegistryServiceError,
    > {
        unimplemented!()
    }
    async fn run_registry_invalidation_event_subscriber(
        &self,
        _: &'static str,
        _: Option<CancellationToken>,
        _: Arc<dyn RegistryInvalidationHandler>,
    ) {
        unimplemented!()
    }
}

#[test]
async fn current_metadata_is_independent_of_cached_revisions() {
    let deployed = component();
    let mut newer = deployed.clone();
    newer.revision = ComponentRevision::new(7).unwrap();
    let registry = Arc::new(TestRegistry::new(
        vec![deployed.clone(), newer.clone()],
        Some(deployed.clone()),
    ));
    let service = RemoteComponentService::new(registry.clone(), &ComponentServiceConfig::default());

    service
        .get_revision(newer.id, newer.revision)
        .await
        .unwrap();
    service.get_all_revisions(deployed.id).await.unwrap();
    assert_eq!(
        service
            .get_current_by_id(deployed.id)
            .await
            .unwrap()
            .revision,
        deployed.revision
    );

    *registry.deployed.lock().unwrap() = Some(newer.clone());
    assert_eq!(
        service
            .get_current_by_id_uncached(deployed.id)
            .await
            .unwrap()
            .revision,
        newer.revision
    );
    assert_eq!(
        service
            .get_current_by_id(deployed.id)
            .await
            .unwrap()
            .revision,
        deployed.revision
    );

    *registry.deployed.lock().unwrap() = None;
    assert!(matches!(
        service.get_current_by_id_uncached(deployed.id).await,
        Err(ComponentServiceError::ComponentNotFound)
    ));
    // A cold deployed lookup must not fall back to historical revision metadata.
    let cold_service = RemoteComponentService::new(registry, &ComponentServiceConfig::default());
    cold_service
        .get_revision(newer.id, newer.revision)
        .await
        .unwrap();
    assert!(matches!(
        cold_service.get_current_by_id(deployed.id).await,
        Err(ComponentServiceError::ComponentNotFound)
    ));
}

#[test]
async fn tool_deployment_state_is_cached_until_invalidation() {
    let a = component();
    let registry = Arc::new(TestRegistry::new(vec![a.clone()], Some(a.clone())));
    let service = RemoteComponentService::new(registry.clone(), &ComponentServiceConfig::default());
    let state = |tool: &str| {
        tool_deployment_state(
            &a,
            tool,
            golem_common::schema::tool::Tool {
                version: "1.0.0".into(),
                requires_filesystem: false,
                http_mount: None,
                schema: golem_common::schema::SchemaGraph::empty(),
                commands: golem_common::schema::tool::CommandTree { nodes: vec![] },
            },
            Vec::new(),
        )
    };
    let read = async |revision: ComponentRevision| {
        service
            .get_tool_deployment_state(a.environment_id, a.id, revision)
            .await
            .unwrap()
            .map(|state| {
                state
                    .registered_tools
                    .keys()
                    .map(|t| t.to_string())
                    .collect::<Vec<_>>()
            })
    };
    let reads = || registry.tool_state_reads.load(Ordering::SeqCst);

    *registry.deployed_tools.lock().unwrap() = Some(state("first"));
    assert_eq!(read(a.revision).await, Some(vec!["first".to_string()]));
    *registry.deployed_tools.lock().unwrap() = Some(state("second"));
    assert_eq!(read(a.revision).await, Some(vec!["first".to_string()]));
    assert_eq!(reads(), 1);

    // Another revision of the same component is a separate entry.
    let other_revision = ComponentRevision::new(7).unwrap();
    assert_eq!(read(other_revision).await, Some(vec!["second".to_string()]));
    assert_eq!(reads(), 2);

    service.invalidate_current().await;
    assert_eq!(read(a.revision).await, Some(vec!["second".to_string()]));
    assert_eq!(reads(), 3);

    // A target without an active tool deployment is cached and invalidated the same way.
    *registry.deployed_tools.lock().unwrap() = None;
    service.invalidate_current().await;
    assert_eq!(read(a.revision).await, None);
    *registry.deployed_tools.lock().unwrap() = Some(state("third"));
    assert_eq!(read(a.revision).await, None);
    assert_eq!(reads(), 4);
    service.invalidate_current().await;
    assert_eq!(read(a.revision).await, Some(vec!["third".to_string()]));
}

#[test]
async fn invalidation_refreshes_deployments_rollbacks_and_removal() {
    let a = component();
    let mut b = a.clone();
    b.revision = ComponentRevision::new(4).unwrap();
    let registry = Arc::new(TestRegistry::new(
        vec![a.clone(), b.clone()],
        Some(a.clone()),
    ));
    let service = RemoteComponentService::new(registry.clone(), &ComponentServiceConfig::default());
    assert_eq!(
        service.get_current_by_id(a.id).await.unwrap().revision,
        a.revision
    );
    for deployed in [b.clone(), a.clone()] {
        *registry.deployed.lock().unwrap() = Some(deployed.clone());
        service.invalidate_current().await;
        assert_eq!(
            service.get_current_by_id(a.id).await.unwrap().revision,
            deployed.revision
        );
        assert_eq!(
            service
                .get_revision(b.id, b.revision)
                .await
                .unwrap()
                .revision,
            b.revision
        );
    }
    *registry.deployed.lock().unwrap() = None;
    service.invalidate_current().await;
    assert!(matches!(
        service.get_current_by_id(a.id).await,
        Err(ComponentServiceError::ComponentNotFound)
    ));
    *registry.deployed.lock().unwrap() = Some(b.clone());
    assert_eq!(
        service.get_current_by_id(a.id).await.unwrap().revision,
        b.revision
    );
}

#[test]
#[test_r::timeout("10s")]
async fn invalidation_fences_pending_successes_and_errors() {
    for (old_exists, ttl) in [
        (true, Duration::from_secs(60)),
        (false, Duration::from_secs(60)),
        (true, Duration::ZERO),
        (false, Duration::ZERO),
    ] {
        let a = component();
        let mut b = a.clone();
        b.revision = ComponentRevision::new(3).unwrap();
        let registry = Arc::new(TestRegistry::new(
            vec![a.clone(), b.clone()],
            old_exists.then(|| a.clone()),
        ));
        let gate = LookupGate::new();
        *registry.next_lookup.lock().unwrap() = Some(gate.clone());
        let service = Arc::new(RemoteComponentService::new(
            registry.clone(),
            &ComponentServiceConfig {
                current_component_cache_ttl: ttl,
                ..Default::default()
            },
        ));
        let caller = tokio::spawn({
            let service = service.clone();
            async move { service.get_current_by_id(a.id).await }
        });
        gate.started.acquire().await.unwrap().forget();
        *registry.deployed.lock().unwrap() = Some(b.clone());
        service.invalidate_current().await;
        assert_eq!(
            service.get_current_by_id(a.id).await.unwrap().revision,
            b.revision
        );
        gate.release.add_permits(1);
        assert_eq!(caller.await.unwrap().unwrap().revision, b.revision);
        assert_eq!(
            service.get_current_by_id(a.id).await.unwrap().revision,
            b.revision
        );
    }
}

#[test]
#[test_r::timeout("10s")]
async fn current_lookup_survives_a_burst_of_deployment_invalidations() {
    let a = component();
    let registry = Arc::new(TestRegistry::new(vec![a.clone()], Some(a.clone())));
    let service = Arc::new(RemoteComponentService::new(
        registry.clone(),
        &ComponentServiceConfig::default(),
    ));
    let mut gate = LookupGate::new();
    *registry.next_lookup.lock().unwrap() = Some(gate.clone());
    let mut caller = tokio::spawn({
        let service = service.clone();
        async move { service.get_current_by_id(a.id).await }
    });
    for _ in 0..4 {
        tokio::select! {
            started = gate.started.acquire() => started.unwrap().forget(),
            result = &mut caller => panic!("lookup stopped during deployment invalidation: {result:?}"),
        }
        service.invalidate_current().await;
        let next = LookupGate::new();
        *registry.next_lookup.lock().unwrap() = Some(next.clone());
        gate.release.add_permits(1);
        gate = next;
    }
    gate.started.acquire().await.unwrap().forget();
    let mut latest = a.clone();
    latest.revision = ComponentRevision::new(3).unwrap();
    *registry.deployed.lock().unwrap() = Some(latest.clone());
    service.invalidate_current().await;
    gate.release.add_permits(1);
    assert_eq!(caller.await.unwrap().unwrap().revision, latest.revision);
}

#[test]
fn current_lookup_invalidation_retries_have_a_time_bound() {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .start_paused(true)
        .build()
        .unwrap()
        .block_on(async {
            let a = component();
            let registry = Arc::new(TestRegistry::new(vec![a.clone()], Some(a.clone())));
            let gate = LookupGate::new();
            *registry.next_lookup.lock().unwrap() = Some(gate.clone());
            let service = Arc::new(RemoteComponentService::new(
                registry,
                &ComponentServiceConfig::default(),
            ));
            let caller = tokio::spawn({
                let service = service.clone();
                async move { service.get_current_by_id(a.id).await }
            });
            gate.started.acquire().await.unwrap().forget();
            service.invalidate_current().await;
            tokio::time::advance(Duration::from_secs(31)).await;
            gate.release.add_permits(1);
            assert!(matches!(
                caller.await.unwrap(),
                Err(ComponentServiceError::InternalError(_))
            ));
        });
}

#[test]
#[test_r::timeout("10s")]
async fn cancelled_caller_does_not_strand_current_lookup() {
    let a = component();
    let registry = Arc::new(TestRegistry::new(vec![a.clone()], Some(a.clone())));
    let gate = LookupGate::new();
    *registry.next_lookup.lock().unwrap() = Some(gate.clone());
    let service = Arc::new(RemoteComponentService::new(
        registry,
        &ComponentServiceConfig::default(),
    ));
    let caller = tokio::spawn({
        let service = service.clone();
        async move { service.get_current_by_id(a.id).await }
    });
    gate.started.acquire().await.unwrap().forget();
    caller.abort();
    assert!(caller.await.unwrap_err().is_cancelled());
    gate.release.add_permits(1);
    assert_eq!(
        service.get_current_by_id(a.id).await.unwrap().revision,
        a.revision
    );
}

#[test]
fn absolute_age_expires_despite_continuous_reads() {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .start_paused(true)
        .build()
        .unwrap()
        .block_on(async {
            let a = component();
            let mut b = a.clone();
            b.revision = ComponentRevision::new(5).unwrap();
            let registry = Arc::new(TestRegistry::new(
                vec![a.clone(), b.clone()],
                Some(a.clone()),
            ));
            let config = ComponentServiceConfig {
                current_component_cache_ttl: Duration::from_secs(10),
                ..Default::default()
            };
            let service = Arc::new(RemoteComponentService::new(registry.clone(), &config));
            assert_eq!(
                service.get_current_by_id(a.id).await.unwrap().revision,
                a.revision
            );
            *registry.deployed.lock().unwrap() = Some(b.clone());
            for _ in 0..9 {
                tokio::time::advance(Duration::from_secs(1)).await;
                assert_eq!(
                    service.get_current_by_id(a.id).await.unwrap().revision,
                    a.revision
                );
            }
            tokio::time::advance(Duration::from_secs(1)).await;
            assert!(service.get_current_by_id_in_cache(a.id).await.is_none());
            assert_eq!(
                service.get_current_by_id(a.id).await.unwrap().revision,
                b.revision
            );
            service.invalidate_current().await;
            *registry.deployed.lock().unwrap() = Some(a.clone());
            let gate = LookupGate::new();
            *registry.next_lookup.lock().unwrap() = Some(gate.clone());
            let caller = tokio::spawn({
                let service = service.clone();
                async move { service.get_current_by_id(a.id).await }
            });
            gate.started.acquire().await.unwrap().forget();
            *registry.deployed.lock().unwrap() = Some(b.clone());
            tokio::time::advance(Duration::from_secs(10)).await;
            gate.release.add_permits(1);
            assert_eq!(caller.await.unwrap().unwrap().revision, b.revision);
        });
}

#[test]
async fn zero_age_or_capacity_disables_current_cache() {
    for config in [
        ComponentServiceConfig {
            current_component_cache_ttl: Duration::ZERO,
            ..Default::default()
        },
        ComponentServiceConfig {
            component_cache_max_capacity: 0,
            ..Default::default()
        },
    ] {
        let a = component();
        let mut b = a.clone();
        b.revision = ComponentRevision::new(2).unwrap();
        let registry = Arc::new(TestRegistry::new(
            vec![a.clone(), b.clone()],
            Some(a.clone()),
        ));
        let service = RemoteComponentService::new(registry.clone(), &config);
        assert_eq!(
            service.get_current_by_id(a.id).await.unwrap().revision,
            a.revision
        );
        *registry.deployed.lock().unwrap() = Some(b.clone());
        assert_eq!(
            service.get_current_by_id(a.id).await.unwrap().revision,
            b.revision
        );
    }
}

#[test]
#[test_r::timeout("30s")]
async fn cancelled_authentication_does_not_block_other_account_invalidation() {
    use crate::config::AuthServiceConfig;
    use crate::service::auth::{AuthService, RemoteAuthService};

    let registry = Arc::new(TestRegistry::new(vec![], None));
    let account = AccountId::new();
    let other_account = AccountId::new();
    let token = TokenSecret::new();
    let other_token = TokenSecret::new();
    for (token, account_id, email) in [
        (token.clone(), account, "cancelled@example.com"),
        (other_token.clone(), other_account, "other@example.com"),
    ] {
        registry.auth_contexts.lock().unwrap().insert(
            token,
            AuthCtx::User(golem_service_base::model::auth::UserAuthCtx {
                account_id,
                account_email: AccountEmail::new(email),
                account_plan_id: golem_common::model::plan::PlanId::new(),
                account_roles: Default::default(),
                effective_surface: Default::default(),
                delegation_surface: None,
            }),
        );
    }
    let service = Arc::new(RemoteAuthService::new(
        registry.clone(),
        &AuthServiceConfig::default(),
    ));
    service
        .authenticate_token(other_token.clone())
        .await
        .unwrap();

    let gate = LookupGate::new();
    *registry.next_lookup.lock().unwrap() = Some(gate.clone());
    let owner = tokio::spawn({
        let service = service.clone();
        let token = token.clone();
        async move { service.authenticate_token(token).await }
    });
    gate.started.acquire().await.unwrap().forget();
    owner.abort();
    assert!(owner.await.unwrap_err().is_cancelled());

    // A retry of the cancelled request must not prevent unrelated revocation.
    let retry = service.authenticate_token(token);
    tokio::pin!(retry);
    assert!(futures::poll!(&mut retry).is_pending());
    registry.auth_contexts.lock().unwrap().remove(&other_token);
    tokio::time::timeout(
        Duration::from_secs(2),
        service.invalidate_tokens_for_account(other_account),
    )
    .await
    .expect("cancelled authentication blocked invalidation for another account");
    assert!(service.authenticate_token(other_token).await.is_err());
    gate.release.add_permits(1);
    assert!(retry.await.is_ok());
}
