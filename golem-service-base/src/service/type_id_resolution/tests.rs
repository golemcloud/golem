use super::*;
use crate::clients::registry::{
    McpRuntimeCredential, RegistryInvalidationHandler, ResolvedTypeId, ResourceUsageUpdate,
};
use crate::custom_api::CompiledRoutes;
use crate::mcp::CompiledMcp;
use crate::model::auth::AuthCtx;
use crate::model::component::Component;
use crate::model::environment::EnvironmentState;
use crate::model::mcp_import::McpImportObservation;
use crate::model::{AccountResourceLimits, ResourceLimits};
use golem_common::model::AgentId;
use golem_common::model::account::{AccountEmail, AccountId};
use golem_common::model::agent::{
    AgentTypeName, RegisteredAgentType, RegistryInvalidationEvent, ResolvedAgentType,
};
use golem_common::model::application::{ApplicationId, ApplicationName};
use golem_common::model::auth::TokenSecret;
use golem_common::model::component::{ComponentId, ComponentRevision};
use golem_common::model::deployment::DeploymentRevision;
use golem_common::model::environment::EnvironmentName;
use golem_common::model::mcp_import::McpImportSource;
use golem_common::model::quota::{ResourceDefinition, ResourceDefinitionId, ResourceName};
use std::collections::HashMap;
use std::pin::Pin;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use test_r::test;
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

#[derive(Default)]
struct ScriptedRegistry {
    entries: Mutex<HashMap<(EnvironmentId, TypeId), ResolvedTypeId>>,
    calls: AtomicUsize,
    fail: AtomicBool,
    gate_next_call: AtomicBool,
    read_done: Notify,
    release: Notify,
}

impl ScriptedRegistry {
    fn set(&self, environment_id: EnvironmentId, type_id: TypeId, entry: Option<ResolvedTypeId>) {
        let mut entries = self.entries.lock().unwrap();
        match entry {
            Some(entry) => entries.insert((environment_id, type_id), entry),
            None => entries.remove(&(environment_id, type_id)),
        };
    }

    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

fn env(n: u128) -> EnvironmentId {
    EnvironmentId(Uuid::from_u128(n))
}

fn type_id(n: u32) -> TypeId {
    TypeId::try_from(n).unwrap()
}

fn implementer(revision: u64) -> RegisteredAgentTypeImplementer {
    RegisteredAgentTypeImplementer {
        component_id: ComponentId(Uuid::from_u128(9)),
        component_revision: ComponentRevision::new(revision).unwrap(),
        component_name: "cart".to_string(),
        account_id: AccountId(Uuid::from_u128(8)),
        account_email: AccountEmail::new("owner@golem"),
    }
}

fn agent_type(
    name: &str,
    implemented_by: Option<RegisteredAgentTypeImplementer>,
) -> ResolvedTypeId {
    ResolvedTypeId {
        kind: TypeIdKind::AgentType,
        name: name.to_string(),
        implemented_by,
    }
}

#[test]
async fn names_stay_cached_and_misses_are_not_cached() {
    let registry = Arc::new(ScriptedRegistry::default());
    let cache = TypeIdResolutionCache::new(registry.clone(), 16);

    assert_eq!(cache.resolve(env(1), type_id(7)).await.unwrap(), None);
    registry.set(env(1), type_id(7), Some(agent_type("Cart", None)));
    let resolved = cache.resolve(env(1), type_id(7)).await.unwrap().unwrap();
    assert_eq!(*resolved, (TypeIdKind::AgentType, "Cart".to_string()));
    assert_eq!(registry.calls(), 2);

    cache.invalidate_environment(env(1)).await;
    registry.set(env(1), type_id(7), None);
    let resolved = cache.resolve(env(1), type_id(7)).await.unwrap().unwrap();
    assert_eq!(resolved.1, "Cart");
    assert_eq!(registry.calls(), 2);

    assert_eq!(cache.resolve(env(2), type_id(7)).await.unwrap(), None);
    assert_eq!(registry.calls(), 3);
}

#[test]
async fn implementers_are_cached_until_their_environment_is_invalidated() {
    let registry = Arc::new(ScriptedRegistry::default());
    let cache = TypeIdResolutionCache::new(registry.clone(), 16);
    registry.set(env(1), type_id(7), Some(agent_type("Cart", None)));
    registry.set(
        env(2),
        type_id(7),
        Some(agent_type("Cart", Some(implementer(1)))),
    );

    assert_eq!(
        cache
            .deployed_implementer(env(1), type_id(7))
            .await
            .unwrap(),
        None
    );
    assert_eq!(
        cache
            .deployed_implementer(env(2), type_id(7))
            .await
            .unwrap(),
        Some(implementer(1))
    );
    assert_eq!(registry.calls(), 2);

    registry.set(
        env(1),
        type_id(7),
        Some(agent_type("Cart", Some(implementer(2)))),
    );
    registry.set(
        env(2),
        type_id(7),
        Some(agent_type("Cart", Some(implementer(3)))),
    );
    assert_eq!(
        cache
            .deployed_implementer(env(1), type_id(7))
            .await
            .unwrap(),
        None
    );
    assert_eq!(registry.calls(), 2);

    cache.invalidate_environment(env(1)).await;
    assert_eq!(
        cache
            .deployed_implementer(env(1), type_id(7))
            .await
            .unwrap(),
        Some(implementer(2))
    );
    assert_eq!(
        cache
            .deployed_implementer(env(2), type_id(7))
            .await
            .unwrap(),
        Some(implementer(1))
    );
    assert_eq!(registry.calls(), 3);
}

#[test]
async fn registry_failures_are_not_cached() {
    let registry = Arc::new(ScriptedRegistry::default());
    let cache = TypeIdResolutionCache::new(registry.clone(), 16);
    registry.set(
        env(1),
        type_id(7),
        Some(agent_type("Cart", Some(implementer(1)))),
    );

    registry.fail.store(true, Ordering::SeqCst);
    assert!(cache.resolve(env(1), type_id(7)).await.is_err());
    assert!(
        cache
            .deployed_implementer(env(1), type_id(7))
            .await
            .is_err()
    );

    registry.fail.store(false, Ordering::SeqCst);
    assert!(cache.resolve(env(1), type_id(7)).await.unwrap().is_some());
    assert_eq!(
        cache
            .deployed_implementer(env(1), type_id(7))
            .await
            .unwrap(),
        Some(implementer(1))
    );
}

#[test]
async fn a_lookup_in_flight_during_invalidation_does_not_keep_the_old_implementer() {
    let registry = Arc::new(ScriptedRegistry::default());
    let cache = TypeIdResolutionCache::new(registry.clone(), 16);
    registry.set(
        env(2),
        type_id(7),
        Some(agent_type("Cart", Some(implementer(3)))),
    );
    assert_eq!(
        cache
            .deployed_implementer(env(2), type_id(7))
            .await
            .unwrap(),
        Some(implementer(3))
    );
    registry.set(
        env(1),
        type_id(7),
        Some(agent_type("Cart", Some(implementer(1)))),
    );
    registry.gate_next_call.store(true, Ordering::SeqCst);

    let in_flight = cache.deployed_implementer(env(1), type_id(7));
    let deploy_and_join = async {
        registry.read_done.notified().await;
        registry.set(
            env(1),
            type_id(7),
            Some(agent_type("Cart", Some(implementer(2)))),
        );
        cache.invalidate_environment(env(1)).await;
        let (joined, ()) = tokio::join!(cache.deployed_implementer(env(1), type_id(7)), async {
            tokio::task::yield_now().await;
            registry.release.notify_one();
        });
        joined
    };
    let (in_flight, joined) = tokio::join!(in_flight, deploy_and_join);

    assert_eq!(joined.unwrap(), Some(implementer(2)));
    assert_eq!(in_flight.unwrap(), Some(implementer(2)));
    assert_eq!(
        cache
            .deployed_implementer(env(1), type_id(7))
            .await
            .unwrap(),
        Some(implementer(2))
    );
}

#[test]
async fn a_cancelled_lookup_does_not_block_later_lookups() {
    let registry = Arc::new(ScriptedRegistry::default());
    let cache = TypeIdResolutionCache::new(registry.clone(), 16);
    registry.set(
        env(1),
        type_id(7),
        Some(agent_type("Cart", Some(implementer(1)))),
    );
    registry.gate_next_call.store(true, Ordering::SeqCst);

    let cancelled = tokio::time::timeout(
        std::time::Duration::from_millis(50),
        cache.deployed_implementer(env(1), type_id(7)),
    )
    .await;
    assert!(cancelled.is_err());
    registry.release.notify_one();

    let later = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        cache.deployed_implementer(env(1), type_id(7)),
    )
    .await
    .expect("a later lookup waits on the cancelled lookup's pending entry");
    assert_eq!(later.unwrap(), Some(implementer(1)));
}

#[async_trait::async_trait]
impl RegistryService for ScriptedRegistry {
    async fn resolve_type_id(
        &self,
        environment_id: EnvironmentId,
        type_id: TypeId,
    ) -> Result<Option<ResolvedTypeId>, RegistryServiceError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self.fail.load(Ordering::SeqCst) {
            return Err(RegistryServiceError::internal_client_error("unavailable"));
        }
        let entry = self
            .entries
            .lock()
            .unwrap()
            .get(&(environment_id, type_id))
            .cloned();
        if self.gate_next_call.swap(false, Ordering::SeqCst) {
            self.read_done.notify_one();
            self.release.notified().await;
        }
        Ok(entry)
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
    async fn authenticate_token(&self, _: &TokenSecret) -> Result<AuthCtx, RegistryServiceError> {
        unimplemented!()
    }
    async fn get_resource_limits(
        &self,
        _: AccountId,
        _: crate::clients::registry::ResourceUsageMetering,
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
