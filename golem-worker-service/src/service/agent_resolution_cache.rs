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

use golem_common::cache::{BackgroundEvictionMode, Cache, FullCacheEvictionMode, SimpleCache};
use golem_common::model::account::AccountId;
use golem_common::model::agent::{AgentTypeName, ResolvedAgentType};
use golem_common::model::application::ApplicationName;
use golem_common::model::deployment::{CurrentDeploymentRevision, DeploymentRevision};
use golem_common::model::environment::{EnvironmentId, EnvironmentName};
use golem_service_base::clients::registry::{RegistryService, RegistryServiceError};
use golem_service_base::model::auth::AuthCtx;
use golem_service_base::service::type_id_resolution::InvalidationGenerations;
use std::sync::Arc;
use std::time::Duration;
use tracing::warn;

// These keys assume token-authenticated User or AdminImpersonation callers with per-account
// permission surfaces. Account permission events invalidate their entries. Agent contexts
// with independently attenuated runtime-card surfaces must not share this cache.
#[derive(Debug, Clone, Hash, Eq, PartialEq)]
struct AgentResolutionCacheKey {
    caller_account_id: AccountId,
    app_name: String,
    env_name: String,
    agent_type_name: String,
    owner_account_email: Option<String>,
}

#[derive(Debug, Clone, Hash, Eq, PartialEq)]
struct PinnedAgentResolutionCacheKey {
    caller_account_id: AccountId,
    app_name: String,
    env_name: String,
    agent_type_name: String,
    deployment_revision: DeploymentRevision,
    owner_account_email: Option<String>,
}

struct CurrentDeploymentRevisions {
    deployment_revision: DeploymentRevision,
    current_deployment_revision: CurrentDeploymentRevision,
}

#[derive(Clone)]
struct FetchedAgentResolution {
    generation: u64,
    resolved: Arc<ResolvedAgentType>,
}

pub struct AgentResolutionCache {
    cache: Cache<AgentResolutionCacheKey, (), FetchedAgentResolution, (u64, RegistryServiceError)>,
    pinned_cache: Cache<
        PinnedAgentResolutionCacheKey,
        (),
        FetchedAgentResolution,
        (u64, RegistryServiceError),
    >,
    registry_service: Arc<dyn RegistryService>,
    current_revisions: scc::HashMap<EnvironmentId, CurrentDeploymentRevisions>,
    generations: Arc<InvalidationGenerations<AccountId>>,
}

impl AgentResolutionCache {
    pub fn new(
        registry_service: Arc<dyn RegistryService>,
        capacity: usize,
        ttl: Duration,
        eviction_period: Duration,
    ) -> Self {
        let cache = Cache::new(
            Some(capacity),
            FullCacheEvictionMode::LeastRecentlyUsed(1),
            BackgroundEvictionMode::OlderThan {
                ttl,
                period: eviction_period,
            },
            "agent_resolution",
        );
        let pinned_cache = Cache::new(
            Some(capacity),
            FullCacheEvictionMode::LeastRecentlyUsed(1),
            BackgroundEvictionMode::None,
            "agent_resolution_pinned",
        );
        Self {
            cache,
            pinned_cache,
            registry_service,
            current_revisions: scc::HashMap::new(),
            generations: Arc::default(),
        }
    }

    pub async fn resolve(
        &self,
        app_name: &ApplicationName,
        env_name: &EnvironmentName,
        agent_type_name: &AgentTypeName,
        owner_account_email: Option<&str>,
        auth_ctx: &AuthCtx,
    ) -> Result<Arc<ResolvedAgentType>, RegistryServiceError> {
        let key = AgentResolutionCacheKey {
            caller_account_id: auth_ctx.access_account_id(),
            app_name: app_name.0.clone(),
            env_name: env_name.0.clone(),
            agent_type_name: agent_type_name.0.clone(),
            owner_account_email: owner_account_email.map(|s| s.to_string()),
        };

        let generation = self.generations.current();
        self.cache
            .remove_if_cached(&key, |cached| self.is_stale(&cached.resolved))
            .await;
        let resolved = loop {
            let registry = self.registry_service.clone();
            let app = app_name.clone();
            let env = env_name.clone();
            let agent = agent_type_name.clone();
            let owner = owner_account_email.map(str::to_owned);
            let auth = auth_ctx.clone();
            let fetched = self
                .cache
                .get_or_insert_simple_spawned(&key, move || async move {
                    let resolved = registry
                        .resolve_agent_type_by_names(
                            &app,
                            &env,
                            &agent,
                            None,
                            owner.as_deref(),
                            &auth,
                        )
                        .await
                        .map(Arc::new)
                        .map_err(|error| (generation, error))?;
                    Ok(FetchedAgentResolution {
                        generation,
                        resolved,
                    })
                })
                .await;
            let fetched = match fetched {
                Ok(fetched) => fetched,
                Err((fetched_generation, error)) => {
                    if self
                        .generations
                        .is_current(key.caller_account_id, fetched_generation)
                        || !self
                            .generations
                            .is_current(key.caller_account_id, generation)
                    {
                        return Err(error);
                    }
                    continue;
                }
            };
            if fetched.resolved.current_deployment_revision.is_some()
                && self.is_stale(&fetched.resolved)
            {
                self.cache
                    .remove_if_cached(&key, |cached| {
                        Arc::ptr_eq(&cached.resolved, &fetched.resolved)
                    })
                    .await;
                continue;
            }
            if self
                .generations
                .is_current(key.caller_account_id, fetched.generation)
            {
                break fetched.resolved;
            }
            self.cache
                .remove_if_cached(&key, |cached| cached.generation == fetched.generation)
                .await;
            // A request whose authentication predates invalidation may finish, but cannot
            // retain its response. A newer request that joined it must fetch with its own auth.
            if !self
                .generations
                .is_current(key.caller_account_id, generation)
            {
                return Ok(fetched.resolved);
            }
        };

        // Track the revision for staleness detection
        if let Some(current_deployment_revision) = resolved.current_deployment_revision {
            self.advance_current_revision(
                resolved.environment_id,
                resolved.deployment_revision,
                current_deployment_revision,
            );
        }

        Ok(resolved)
    }

    pub async fn resolve_pinned(
        &self,
        app_name: &ApplicationName,
        env_name: &EnvironmentName,
        agent_type_name: &AgentTypeName,
        deployment_revision: DeploymentRevision,
        owner_account_email: Option<&str>,
        auth_ctx: &AuthCtx,
    ) -> Result<Arc<ResolvedAgentType>, RegistryServiceError> {
        let key = PinnedAgentResolutionCacheKey {
            caller_account_id: auth_ctx.access_account_id(),
            app_name: app_name.0.clone(),
            env_name: env_name.0.clone(),
            agent_type_name: agent_type_name.0.clone(),
            deployment_revision,
            owner_account_email: owner_account_email.map(|s| s.to_string()),
        };

        let generation = self.generations.current();
        loop {
            let registry = self.registry_service.clone();
            let app = app_name.clone();
            let env = env_name.clone();
            let agent = agent_type_name.clone();
            let owner = owner_account_email.map(str::to_owned);
            let auth = auth_ctx.clone();
            let fetched = self
                .pinned_cache
                .get_or_insert_simple_spawned(&key, move || async move {
                    let resolved = registry
                        .resolve_agent_type_by_names(
                            &app,
                            &env,
                            &agent,
                            Some(deployment_revision),
                            owner.as_deref(),
                            &auth,
                        )
                        .await
                        .map(Arc::new)
                        .map_err(|error| (generation, error))?;
                    Ok(FetchedAgentResolution {
                        generation,
                        resolved,
                    })
                })
                .await;
            let fetched = match fetched {
                Ok(fetched) => fetched,
                Err((fetched_generation, error)) => {
                    if self
                        .generations
                        .is_current(key.caller_account_id, fetched_generation)
                        || !self
                            .generations
                            .is_current(key.caller_account_id, generation)
                    {
                        return Err(error);
                    }
                    continue;
                }
            };
            if self
                .generations
                .is_current(key.caller_account_id, fetched.generation)
            {
                return Ok(fetched.resolved);
            }
            self.pinned_cache
                .remove_if_cached(&key, |cached| cached.generation == fetched.generation)
                .await;
            if !self
                .generations
                .is_current(key.caller_account_id, generation)
            {
                return Ok(fetched.resolved);
            }
        }
    }

    fn is_stale(&self, resolved: &ResolvedAgentType) -> bool {
        let Some(current_deployment_revision) = resolved.current_deployment_revision else {
            warn!(
                environment_id = %resolved.environment_id,
                deployment_revision = resolved.deployment_revision.get(),
                "Resolved current agent type without current_deployment_revision; treating as stale"
            );
            return true;
        };

        self.current_revisions
            .read_sync(&resolved.environment_id, |_, current_rev| {
                current_deployment_revision < current_rev.current_deployment_revision
                    || (current_deployment_revision == current_rev.current_deployment_revision
                        && resolved.deployment_revision != current_rev.deployment_revision)
            })
            == Some(true)
    }

    fn advance_current_revision(
        &self,
        environment_id: EnvironmentId,
        deployment_revision: DeploymentRevision,
        current_deployment_revision: CurrentDeploymentRevision,
    ) {
        let updated = self
            .current_revisions
            .update_sync(&environment_id, |_, existing| {
                if current_deployment_revision > existing.current_deployment_revision {
                    *existing = CurrentDeploymentRevisions {
                        deployment_revision,
                        current_deployment_revision,
                    }
                }
            })
            .is_some();
        if !updated {
            let _ = self.current_revisions.insert_sync(
                environment_id,
                CurrentDeploymentRevisions {
                    deployment_revision,
                    current_deployment_revision,
                },
            );
        }
    }

    pub fn update_current_revision(
        &self,
        environment_id: EnvironmentId,
        deployment_revision: DeploymentRevision,
        current_deployment_revision: CurrentDeploymentRevision,
    ) {
        self.advance_current_revision(
            environment_id,
            deployment_revision,
            current_deployment_revision,
        );
    }

    /// Removes all cache entries for a specific application name.
    /// Also removes corresponding entries from `current_revisions` for the
    /// given environment UUIDs to avoid the stale-but-not-observed path.
    pub async fn invalidate_by_app_name(&self, app_name: &str, environment_ids: &[EnvironmentId]) {
        let keys = self.cache.keys().await;
        for key in keys.into_iter().filter(|k| k.app_name == app_name) {
            self.cache.remove(&key).await;
        }
        for env_id in environment_ids {
            self.current_revisions.remove_sync(env_id);
        }
    }

    /// Removes all cache entries for a specific (app_name, env_name) pair.
    /// Also removes the corresponding `current_revisions` entry for the given
    /// environment UUID.
    pub async fn invalidate_by_env(
        &self,
        app_name: &str,
        env_name: &str,
        environment_id: EnvironmentId,
    ) {
        let keys = self.cache.keys().await;
        for key in keys
            .into_iter()
            .filter(|k| k.app_name == app_name && k.env_name == env_name)
        {
            self.cache.remove(&key).await;
        }
        self.current_revisions.remove_sync(&environment_id);
    }

    /// Invalidates account entries in both caches, including responses still in flight.
    pub async fn invalidate_by_account(&self, account_id: AccountId) {
        let generation = self.generations.invalidate(account_id);
        for (key, _) in self.cache.iter().await {
            if key.caller_account_id == account_id {
                self.cache
                    .remove_if_cached(&key, |cached| cached.generation < generation)
                    .await;
            }
        }
        for (key, _) in self.pinned_cache.iter().await {
            if key.caller_account_id == account_id {
                self.pinned_cache
                    .remove_if_cached(&key, |cached| cached.generation < generation)
                    .await;
            }
        }
    }

    pub async fn clear(&self) {
        let generation = self.generations.invalidate_all();
        for (key, _) in self.cache.iter().await {
            self.cache
                .remove_if_cached(&key, |cached| cached.generation < generation)
                .await;
        }
        for (key, _) in self.pinned_cache.iter().await {
            self.pinned_cache
                .remove_if_cached(&key, |cached| cached.generation < generation)
                .await;
        }
        self.current_revisions.retain_sync(|_, _| false);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use golem_common::base_model::account::AccountId;
    use golem_common::base_model::application::ApplicationId;
    use golem_common::model::agent::{
        AgentMode, AgentTypeName, RegisteredAgentType, RegisteredAgentTypeImplementer,
        ResolvedAgentType, Snapshotting,
    };
    use golem_common::model::application::ApplicationName;
    use golem_common::model::component::{ComponentId, ComponentRevision};
    use golem_common::model::deployment::{CurrentDeploymentRevision, DeploymentRevision};
    use golem_common::model::environment::{EnvironmentId, EnvironmentName};
    use golem_common::model::quota::{ResourceDefinition, ResourceDefinitionId, ResourceName};
    use golem_common::schema::{AgentConstructorSchema, AgentTypeSchema, InputSchema, SchemaGraph};
    use golem_schema::Empty;
    use golem_service_base::clients::registry::RegistryServiceError;
    use golem_service_base::model::auth::AuthCtx;
    use std::sync::atomic::{AtomicU64, Ordering};
    use test_r::test;
    use uuid::Uuid;

    fn make_resolved(env_id: EnvironmentId, rev: DeploymentRevision) -> ResolvedAgentType {
        ResolvedAgentType {
            registered_agent_type: RegisteredAgentType {
                type_id: golem_common::model::type_id::TypeId::try_from(42u32).unwrap(),
                agent_type: AgentTypeSchema {
                    kind: golem_common::schema::agent::AgentTypeKind::Regular,
                    type_name: AgentTypeName("test-agent".to_string()),
                    description: String::new(),
                    source_language: String::new(),
                    schema: SchemaGraph::empty(),
                    constructor: AgentConstructorSchema {
                        name: None,
                        description: String::new(),
                        prompt_hint: None,
                        input_schema: InputSchema::Parameters(vec![]),
                    },
                    methods: vec![],
                    dependencies: vec![],
                    mode: AgentMode::Durable,
                    http_mount: None,
                    snapshotting: Snapshotting::Disabled(Empty {}),
                    config: vec![],
                },
                implemented_by: RegisteredAgentTypeImplementer {
                    component_id: ComponentId(Uuid::nil()),
                    component_revision: ComponentRevision::INITIAL,
                    component_name: "test-component".to_string(),
                    account_id: AccountId(Uuid::nil()),
                    account_email: golem_common::model::account::AccountEmail::new("test@golem"),
                },
            },
            environment_id: env_id,
            deployment_revision: rev,
            current_deployment_revision: Some(
                CurrentDeploymentRevision::new(rev.get())
                    .unwrap_or(CurrentDeploymentRevision::INITIAL),
            ),
        }
    }

    struct MockRegistryService {
        env_id: EnvironmentId,
        environments_by_account: std::collections::HashMap<AccountId, EnvironmentId>,
        revision: AtomicU64,
        current_revision: AtomicU64,
        next_lookup: std::sync::Mutex<Option<Arc<crate::service::component::tests::LookupGate>>>,
    }

    impl MockRegistryService {
        fn new(env_id: EnvironmentId) -> Self {
            Self {
                env_id,
                environments_by_account: std::collections::HashMap::new(),
                revision: AtomicU64::new(0),
                current_revision: AtomicU64::new(0),
                next_lookup: std::sync::Mutex::new(None),
            }
        }

        fn set_revision(&self, rev: u64) {
            self.revision.store(rev, Ordering::SeqCst);
            self.current_revision.store(rev, Ordering::SeqCst);
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
        async fn authenticate_token(
            &self,
            _: &golem_common::model::auth::TokenSecret,
        ) -> Result<AuthCtx, RegistryServiceError> {
            unimplemented!()
        }
        async fn get_resource_limits(
            &self,
            _: AccountId,
            _metering: golem_service_base::clients::registry::ResourceUsageMetering,
        ) -> Result<golem_service_base::model::ResourceLimits, RegistryServiceError> {
            unimplemented!()
        }
        async fn update_worker_connection_limit(
            &self,
            _: AccountId,
            _: &golem_common::base_model::AgentId,
            _: bool,
        ) -> Result<(), RegistryServiceError> {
            unimplemented!()
        }
        async fn batch_update_resource_usage(
            &self,
            _: std::collections::HashMap<
                AccountId,
                golem_service_base::clients::registry::ResourceUsageUpdate,
            >,
        ) -> Result<golem_service_base::model::AccountResourceLimits, RegistryServiceError>
        {
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
        ) -> Result<golem_service_base::model::component::Component, RegistryServiceError> {
            unimplemented!()
        }
        async fn get_deployed_component_metadata(
            &self,
            _: ComponentId,
        ) -> Result<golem_service_base::model::component::Component, RegistryServiceError> {
            unimplemented!()
        }
        async fn get_all_deployed_component_revisions(
            &self,
            _: ComponentId,
        ) -> Result<Vec<golem_service_base::model::component::Component>, RegistryServiceError>
        {
            unimplemented!()
        }
        async fn resolve_component(
            &self,
            _: AccountId,
            _: ApplicationId,
            _: EnvironmentId,
            _: &str,
        ) -> Result<golem_service_base::model::component::Component, RegistryServiceError> {
            unimplemented!()
        }
        async fn get_all_agent_types(
            &self,
            _: EnvironmentId,
            _: ComponentId,
            _: ComponentRevision,
        ) -> Result<Vec<golem_common::model::agent::RegisteredAgentType>, RegistryServiceError>
        {
            unimplemented!()
        }
        async fn get_agent_type(
            &self,
            _: EnvironmentId,
            _: ComponentId,
            _: ComponentRevision,
            _: &AgentTypeName,
        ) -> Result<golem_common::model::agent::RegisteredAgentType, RegistryServiceError> {
            unimplemented!()
        }
        async fn resolve_agent_type_by_names(
            &self,
            _app_name: &ApplicationName,
            _environment_name: &EnvironmentName,
            _agent_type_name: &AgentTypeName,
            _deployment_revision: Option<DeploymentRevision>,
            owner_account_email: Option<&str>,
            auth_ctx: &AuthCtx,
        ) -> Result<ResolvedAgentType, RegistryServiceError> {
            let rev = self.revision.load(Ordering::SeqCst);
            let current_revision = self.current_revision.load(Ordering::SeqCst);
            let env_id = if owner_account_email.is_none() {
                self.environments_by_account
                    .get(&auth_ctx.access_account_id())
                    .copied()
                    .unwrap_or(self.env_id)
            } else {
                self.env_id
            };
            let gate = self.next_lookup.lock().unwrap().take();
            if let Some(gate) = gate {
                gate.started.add_permits(1);
                gate.release.acquire().await.unwrap().forget();
            }
            let mut resolved = make_resolved(
                env_id,
                DeploymentRevision::new(rev).unwrap_or(DeploymentRevision::INITIAL),
            );
            resolved.current_deployment_revision =
                Some(CurrentDeploymentRevision::new(current_revision).unwrap());
            Ok(resolved)
        }
        async fn get_active_routes_for_domain(
            &self,
            _: &golem_common::base_model::domain_registration::Domain,
        ) -> Result<golem_service_base::custom_api::CompiledRoutes, RegistryServiceError> {
            unimplemented!()
        }
        async fn get_active_compiled_mcps_for_domain(
            &self,
            _: &golem_common::base_model::domain_registration::Domain,
        ) -> Result<golem_service_base::mcp::CompiledMcp, RegistryServiceError> {
            unimplemented!()
        }
        async fn get_current_environment_state(
            &self,
            _: EnvironmentId,
        ) -> Result<golem_service_base::model::environment::EnvironmentState, RegistryServiceError>
        {
            unimplemented!()
        }
        async fn get_resource_definition_by_id(
            &self,
            _resource_definition_id: ResourceDefinitionId,
        ) -> Result<ResourceDefinition, RegistryServiceError> {
            unimplemented!()
        }
        async fn get_resource_definition_by_name(
            &self,
            _environment_id: EnvironmentId,
            _resource_name: ResourceName,
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
            _service_name: &'static str,
            _shutdown_token: Option<tokio_util::sync::CancellationToken>,
            _handler: std::sync::Arc<
                dyn golem_service_base::clients::registry::RegistryInvalidationHandler,
            >,
        ) {
            unimplemented!()
        }
    }

    fn make_auth() -> AuthCtx {
        AuthCtx::System
    }

    fn make_cache(registry: Arc<dyn RegistryService>) -> AgentResolutionCache {
        AgentResolutionCache::new(
            registry,
            10,
            Duration::from_secs(60),
            Duration::from_secs(60),
        )
    }

    #[test]
    async fn test_resolution_is_scoped_to_caller_account() {
        let alice_id = AccountId(Uuid::new_v4());
        let bob_id = AccountId(Uuid::new_v4());
        let alice_env = EnvironmentId(Uuid::new_v4());
        let bob_env = EnvironmentId(Uuid::new_v4());
        let mut registry = MockRegistryService::new(EnvironmentId(Uuid::new_v4()));
        registry.environments_by_account = [(alice_id, alice_env), (bob_id, bob_env)].into();
        let cache = make_cache(Arc::new(registry));
        let app = ApplicationName("app".to_string());
        let env = EnvironmentName("env".to_string());
        let agent = AgentTypeName("agent".to_string());

        let admin_id = AccountId(Uuid::new_v4());
        for (account_id, email, expected_env) in [
            (alice_id, "alice@test.com", alice_env),
            (bob_id, "bob@test.com", bob_env),
            (alice_id, "alice@test.com", alice_env),
            (bob_id, "bob@test.com", bob_env),
        ] {
            for auth in [
                AuthCtx::agent_with_effective_surface(
                    account_id,
                    golem_common::model::account::AccountEmail::new(email),
                    Default::default(),
                ),
                AuthCtx::admin_impersonation(
                    admin_id,
                    account_id,
                    golem_common::model::account::AccountEmail::new(email),
                    Default::default(),
                    golem_common::model::plan::PlanId(Uuid::nil()),
                    Default::default(),
                ),
            ] {
                let resolved = cache
                    .resolve(&app, &env, &agent, None, &auth)
                    .await
                    .unwrap();
                assert_eq!(resolved.environment_id, expected_env);
            }
        }
    }

    #[test]
    async fn test_pinned_resolution_is_scoped_to_caller_account() {
        let alice_id = AccountId(Uuid::new_v4());
        let bob_id = AccountId(Uuid::new_v4());
        let alice_env = EnvironmentId(Uuid::new_v4());
        let bob_env = EnvironmentId(Uuid::new_v4());
        let mut registry = MockRegistryService::new(EnvironmentId(Uuid::new_v4()));
        registry.environments_by_account = [(alice_id, alice_env), (bob_id, bob_env)].into();
        let revision = DeploymentRevision::new(5).unwrap();
        registry.set_revision(5);
        let cache = make_cache(Arc::new(registry));
        let app = ApplicationName("app".to_string());
        let env = EnvironmentName("env".to_string());
        let agent = AgentTypeName("agent".to_string());

        let admin_id = AccountId(Uuid::new_v4());
        for (account_id, email, expected_env) in [
            (alice_id, "alice@test.com", alice_env),
            (bob_id, "bob@test.com", bob_env),
            (alice_id, "alice@test.com", alice_env),
            (bob_id, "bob@test.com", bob_env),
        ] {
            for auth in [
                AuthCtx::agent_with_effective_surface(
                    account_id,
                    golem_common::model::account::AccountEmail::new(email),
                    Default::default(),
                ),
                AuthCtx::admin_impersonation(
                    admin_id,
                    account_id,
                    golem_common::model::account::AccountEmail::new(email),
                    Default::default(),
                    golem_common::model::plan::PlanId(Uuid::nil()),
                    Default::default(),
                ),
            ] {
                let resolved = cache
                    .resolve_pinned(&app, &env, &agent, revision, None, &auth)
                    .await
                    .unwrap();
                assert_eq!(resolved.environment_id, expected_env);
                assert_eq!(resolved.deployment_revision, revision);
            }
        }
    }

    #[test]
    async fn test_cache_hit_after_resolve() {
        let env_id = EnvironmentId(Uuid::new_v4());
        let registry = Arc::new(MockRegistryService::new(env_id));
        let cache = make_cache(registry.clone());

        let app = ApplicationName("app".to_string());
        let env = EnvironmentName("env".to_string());
        let agent = AgentTypeName("agent".to_string());

        let result = cache
            .resolve(&app, &env, &agent, Some("owner@test.com"), &make_auth())
            .await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap().environment_id, env_id);

        // Second call should hit cache (mock would return same result either way,
        // but the cache should not call the registry again)
        let result2 = cache
            .resolve(&app, &env, &agent, Some("owner@test.com"), &make_auth())
            .await;
        assert!(result2.is_ok());
        assert_eq!(result2.unwrap().registered_agent_type.type_id.get(), 42);
    }

    #[test]
    async fn test_cache_miss_different_key() {
        let env_id = EnvironmentId(Uuid::new_v4());
        let registry = Arc::new(MockRegistryService::new(env_id));
        let cache = make_cache(registry);

        let app = ApplicationName("app".to_string());
        let env = EnvironmentName("env".to_string());
        let agent = AgentTypeName("agent".to_string());

        let result = cache.resolve(&app, &env, &agent, None, &make_auth()).await;
        assert!(result.is_ok());
    }

    #[test]
    async fn test_invalidation_different_revision() {
        let env_id = EnvironmentId(Uuid::new_v4());
        let registry = Arc::new(MockRegistryService::new(env_id));
        let cache = make_cache(registry.clone());

        let app = ApplicationName("app".to_string());
        let env = EnvironmentName("env".to_string());
        let agent = AgentTypeName("agent".to_string());

        let result = cache.resolve(&app, &env, &agent, None, &make_auth()).await;
        assert!(result.is_ok());
        assert_eq!(
            result.unwrap().deployment_revision,
            DeploymentRevision::INITIAL
        );

        // Simulate invalidation with a newer revision
        let new_rev = DeploymentRevision::new(1).unwrap();
        cache.update_current_revision(
            env_id,
            new_rev,
            CurrentDeploymentRevision::new(new_rev.get()).unwrap(),
        );

        // Update mock to return new revision
        registry.set_revision(1);

        // Should re-resolve from registry since cached entry is stale
        let result2 = cache.resolve(&app, &env, &agent, None, &make_auth()).await;
        assert!(result2.is_ok());
        assert_eq!(result2.unwrap().deployment_revision, new_rev);
    }

    #[test]
    async fn test_no_invalidation_same_revision() {
        let env_id = EnvironmentId(Uuid::new_v4());
        let registry = Arc::new(MockRegistryService::new(env_id));
        let cache = make_cache(registry);

        let app = ApplicationName("app".to_string());
        let env = EnvironmentName("env".to_string());
        let agent = AgentTypeName("agent".to_string());

        let result = cache.resolve(&app, &env, &agent, None, &make_auth()).await;
        assert!(result.is_ok());

        cache.update_current_revision(
            env_id,
            DeploymentRevision::INITIAL,
            CurrentDeploymentRevision::INITIAL,
        );

        let result2 = cache.resolve(&app, &env, &agent, None, &make_auth()).await;
        assert!(result2.is_ok());
    }

    #[test]
    #[test_r::timeout("30s")]
    async fn post_deployment_caller_does_not_join_old_revision() {
        let env_id = EnvironmentId::new();
        let registry = Arc::new(MockRegistryService::new(env_id));
        let cache = make_cache(registry.clone());
        let app = ApplicationName("app".into());
        let env = EnvironmentName("env".into());
        let agent = AgentTypeName("agent".into());
        let auth = AuthCtx::admin_impersonation(
            AccountId::new(),
            AccountId::new(),
            golem_common::model::account::AccountEmail::new("caller@test.com"),
            Default::default(),
            golem_common::model::plan::PlanId::new(),
            Default::default(),
        );
        let gate = crate::service::component::tests::LookupGate::new();
        *registry.next_lookup.lock().unwrap() = Some(gate.clone());
        let old = cache.resolve(&app, &env, &agent, None, &auth);
        tokio::pin!(old);
        assert!(futures::poll!(&mut old).is_pending());
        gate.started.acquire().await.unwrap().forget();

        let new_rev = DeploymentRevision::new(1).unwrap();
        registry.set_revision(1);
        cache.update_current_revision(env_id, new_rev, CurrentDeploymentRevision::new(1).unwrap());
        let fresh = cache.resolve(&app, &env, &agent, None, &auth);
        tokio::pin!(fresh);
        let immediate = futures::poll!(&mut fresh);
        gate.release.add_permits(1);
        old.await.unwrap();
        let result = match immediate {
            std::task::Poll::Ready(result) => result,
            std::task::Poll::Pending => fresh.await,
        }
        .unwrap();
        // Both the pending caller and a later caller must observe the deployment.
        assert_eq!(
            cache
                .resolve(&app, &env, &agent, None, &auth)
                .await
                .unwrap()
                .deployment_revision,
            new_rev
        );
        assert_eq!(
            result.deployment_revision, new_rev,
            "a request started after the deployment update must not reuse its predecessor's revision"
        );
    }

    #[test]
    async fn test_clear_flushes_everything() {
        let env_id = EnvironmentId(Uuid::new_v4());
        let registry = Arc::new(MockRegistryService::new(env_id));
        let cache = make_cache(registry);

        let app = ApplicationName("app".to_string());
        let env = EnvironmentName("env".to_string());
        let agent = AgentTypeName("agent".to_string());

        let _ = cache.resolve(&app, &env, &agent, None, &make_auth()).await;
        cache.clear().await;

        // After clear, cache should re-resolve (which works fine since mock returns a value)
        let result = cache.resolve(&app, &env, &agent, None, &make_auth()).await;
        assert!(result.is_ok());
    }

    #[test]
    async fn test_owner_email_is_part_of_cache_key() {
        let env_id = EnvironmentId(Uuid::new_v4());
        let registry = Arc::new(MockRegistryService::new(env_id));
        let cache = make_cache(registry);

        let app = ApplicationName("app".to_string());
        let env = EnvironmentName("env".to_string());
        let agent = AgentTypeName("agent".to_string());

        let r1 = cache.resolve(&app, &env, &agent, None, &make_auth()).await;
        assert!(r1.is_ok());

        let r2 = cache
            .resolve(&app, &env, &agent, Some("alice@test.com"), &make_auth())
            .await;
        assert!(r2.is_ok());

        let r3 = cache
            .resolve(&app, &env, &agent, Some("bob@test.com"), &make_auth())
            .await;
        assert!(r3.is_ok());
    }

    #[test]
    async fn test_pinned_cache_hit() {
        let env_id = EnvironmentId(Uuid::new_v4());
        let registry = Arc::new(MockRegistryService::new(env_id));
        registry.set_revision(5);
        let cache = make_cache(registry.clone());

        let app = ApplicationName("app".to_string());
        let env = EnvironmentName("env".to_string());
        let agent = AgentTypeName("agent".to_string());
        let rev = DeploymentRevision::new(5).unwrap();

        let r1 = cache
            .resolve_pinned(&app, &env, &agent, rev, None, &make_auth())
            .await;
        assert!(r1.is_ok());
        assert_eq!(r1.unwrap().deployment_revision, rev);

        // Second call should hit the pinned cache
        let r2 = cache
            .resolve_pinned(&app, &env, &agent, rev, None, &make_auth())
            .await;
        assert!(r2.is_ok());
        let r2 = r2.unwrap();
        assert_eq!(r2.deployment_revision, rev);
        assert_eq!(r2.registered_agent_type.type_id.get(), 42);
    }

    #[test]
    #[test_r::timeout("10s")]
    async fn newer_registry_revision_before_event_completes() {
        let env_id = EnvironmentId::new();
        let registry = Arc::new(MockRegistryService::new(env_id));
        let cache = make_cache(registry.clone());
        let app = ApplicationName("app".into());
        let env = EnvironmentName("env".into());
        let auth = make_auth();
        cache
            .resolve(&app, &env, &AgentTypeName("first".into()), None, &auth)
            .await
            .unwrap();

        // The registry can observe a deployment before the asynchronous event arrives.
        // A miss for another agent must accept that newer authoritative response.
        registry.set_revision(1);
        let resolved = tokio::time::timeout(
            Duration::from_secs(1),
            cache.resolve(&app, &env, &AgentTypeName("second".into()), None, &auth),
        )
        .await;

        let resolved = resolved
            .expect("a newer registry response must not retry until its deployment event arrives")
            .unwrap();
        assert_eq!(
            resolved.deployment_revision,
            DeploymentRevision::new(1).unwrap()
        );

        // A delayed older event must not undo the revision learned from the registry.
        cache.update_current_revision(
            env_id,
            DeploymentRevision::INITIAL,
            CurrentDeploymentRevision::INITIAL,
        );
        assert_eq!(
            cache
                .resolve(&app, &env, &AgentTypeName("first".into()), None, &auth)
                .await
                .unwrap()
                .deployment_revision,
            DeploymentRevision::new(1).unwrap()
        );

        // A rollback decreases the deployment revision but increases the current counter.
        registry.set_revision(0);
        registry.current_revision.store(2, Ordering::SeqCst);
        let rollback = cache
            .resolve(&app, &env, &AgentTypeName("third".into()), None, &auth)
            .await
            .unwrap();
        assert_eq!(rollback.deployment_revision, DeploymentRevision::INITIAL);
        assert_eq!(
            rollback.current_deployment_revision,
            Some(CurrentDeploymentRevision::new(2).unwrap())
        );
        assert_eq!(
            cache
                .resolve(&app, &env, &AgentTypeName("first".into()), None, &auth)
                .await
                .unwrap()
                .deployment_revision,
            DeploymentRevision::INITIAL
        );
    }

    #[test]
    async fn test_pinned_cache_different_revisions_are_separate() {
        let env_id = EnvironmentId(Uuid::new_v4());
        let registry = Arc::new(MockRegistryService::new(env_id));
        let cache = make_cache(registry.clone());

        let app = ApplicationName("app".to_string());
        let env = EnvironmentName("env".to_string());
        let agent = AgentTypeName("agent".to_string());

        registry.set_revision(1);
        let rev1 = DeploymentRevision::new(1).unwrap();
        let r1 = cache
            .resolve_pinned(&app, &env, &agent, rev1, None, &make_auth())
            .await;
        assert!(r1.is_ok());
        assert_eq!(r1.unwrap().deployment_revision, rev1);

        registry.set_revision(2);
        let rev2 = DeploymentRevision::new(2).unwrap();
        let r2 = cache
            .resolve_pinned(&app, &env, &agent, rev2, None, &make_auth())
            .await;
        assert!(r2.is_ok());
        assert_eq!(r2.unwrap().deployment_revision, rev2);
    }
}
