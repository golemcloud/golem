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

pub(crate) struct TestRegistry {
    revisions: Vec<Component>,
    pub(crate) deployed: Mutex<Option<Component>>,
    next_lookup: Mutex<Option<Arc<LookupGate>>>,
}

struct LookupGate {
    started: Semaphore,
    release: Semaphore,
}

impl LookupGate {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            started: Semaphore::new(0),
            release: Semaphore::new(0),
        })
    }
}

impl TestRegistry {
    pub(crate) fn new(revisions: Vec<Component>, deployed: Option<Component>) -> Self {
        Self {
            revisions,
            deployed: Mutex::new(deployed),
            next_lookup: Mutex::new(None),
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
        created_at: chrono::Utc::now(),
        wasm_hash: golem_common::model::diff::Hash::empty(),
        object_store_key: "test".into(),
    }
}

#[async_trait]
impl RegistryService for TestRegistry {
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
        let gate = self.next_lookup.lock().unwrap().take();
        if let Some(gate) = gate {
            gate.started.add_permits(1);
            gate.release.acquire().await.unwrap().forget();
        }
        result
    }
    async fn get_all_deployed_component_revisions(
        &self,
        _: ComponentId,
    ) -> Result<Vec<Component>, RegistryServiceError> {
        Ok(self.revisions.clone())
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
        _: &ApplicationName,
        _: &EnvironmentName,
        _: &AgentTypeName,
        _: Option<DeploymentRevision>,
        _: Option<&str>,
        _: &AuthCtx,
    ) -> Result<ResolvedAgentType, RegistryServiceError> {
        unimplemented!()
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
