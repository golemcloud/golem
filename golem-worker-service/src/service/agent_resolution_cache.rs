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
use golem_common::model::agent::{AgentTypeName, ResolvedAgentType};
use golem_common::model::application::ApplicationName;
use golem_common::model::deployment::DeploymentRevision;
use golem_common::model::environment::{EnvironmentId, EnvironmentName};
use golem_service_base::clients::registry::{RegistryService, RegistryServiceError};
use golem_service_base::model::auth::AuthCtx;
use std::sync::Arc;
use std::time::Duration;
use tracing::{debug, info, warn};

#[derive(Debug, Clone, Hash, Eq, PartialEq)]
struct AgentResolutionCacheKey {
    app_name: String,
    env_name: String,
    agent_type_name: String,
    owner_account_email: Option<String>,
}

pub struct AgentResolutionCache {
    cache: Cache<AgentResolutionCacheKey, (), ResolvedAgentType, RegistryServiceError>,
    registry_service: Arc<dyn RegistryService>,
    latest_revisions: scc::HashMap<EnvironmentId, DeploymentRevision>,
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
        Self {
            cache,
            registry_service,
            latest_revisions: scc::HashMap::new(),
        }
    }

    pub async fn resolve(
        &self,
        app_name: &ApplicationName,
        env_name: &EnvironmentName,
        agent_type_name: &AgentTypeName,
        owner_account_email: Option<&str>,
        auth_ctx: &AuthCtx,
    ) -> Result<ResolvedAgentType, RegistryServiceError> {
        let key = AgentResolutionCacheKey {
            app_name: app_name.0.clone(),
            env_name: env_name.0.clone(),
            agent_type_name: agent_type_name.0.clone(),
            owner_account_email: owner_account_email.map(|s| s.to_string()),
        };

        // Check if we have a cached but stale entry and remove it before get_or_insert
        if let Some(resolved) = self.cache.try_get(&key).await {
            if self.is_stale(&resolved) {
                self.cache.remove(&key).await;
            }
        }

        let registry = self.registry_service.clone();
        let app = app_name.clone();
        let env = env_name.clone();
        let agent = agent_type_name.clone();
        let owner = owner_account_email.map(|s| s.to_string());
        let auth = auth_ctx.clone();

        let resolved = self
            .cache
            .get_or_insert_simple(&key, async || {
                registry
                    .resolve_agent_type_by_names(&app, &env, &agent, None, owner.as_deref(), &auth)
                    .await
            })
            .await?;

        // Track the revision for staleness detection
        self.advance_latest_revision(resolved.environment_id, resolved.deployment_revision);

        Ok(resolved)
    }

    fn is_stale(&self, resolved: &ResolvedAgentType) -> bool {
        self.latest_revisions
            .read_sync(&resolved.environment_id, |_, latest_rev| {
                resolved.deployment_revision != *latest_rev
            })
            == Some(true)
    }

    fn advance_latest_revision(
        &self,
        environment_id: EnvironmentId,
        deployment_revision: DeploymentRevision,
    ) {
        let updated = self
            .latest_revisions
            .update_sync(&environment_id, |_, existing| {
                if deployment_revision > *existing {
                    *existing = deployment_revision;
                }
            })
            .is_some();
        if !updated {
            let _ = self
                .latest_revisions
                .insert_sync(environment_id, deployment_revision);
        }
    }

    pub fn update_latest_revision(
        &self,
        environment_id: EnvironmentId,
        deployment_revision: DeploymentRevision,
    ) {
        self.advance_latest_revision(environment_id, deployment_revision);
    }

    pub async fn clear(&self) {
        let keys = self.cache.keys().await;
        for key in keys {
            self.cache.remove(&key).await;
        }
        self.latest_revisions.retain_sync(|_, _| false);
    }
}

pub async fn run_invalidation_subscriber(
    registry_service: Arc<dyn RegistryService>,
    cache: Arc<AgentResolutionCache>,
) {
    use futures::StreamExt;

    let mut last_seen_event_id: Option<u64> = None;
    let mut backoff = Duration::from_millis(100);
    let max_backoff = Duration::from_secs(30);

    loop {
        match registry_service
            .subscribe_deployment_invalidations(last_seen_event_id)
            .await
        {
            Ok(mut stream) => {
                info!("Connected to deployment invalidation stream");
                backoff = Duration::from_millis(100);

                while let Some(result) = stream.next().await {
                    match result {
                        Ok(event) => {
                            if event.cursor_expired {
                                warn!("Deployment invalidation cursor expired, flushing cache");
                                cache.clear().await;
                            } else if let Some(env_id) = event.environment_id {
                                if let Ok(rev) =
                                    DeploymentRevision::new(event.deployment_revision)
                                {
                                    debug!(
                                        environment_id = %env_id,
                                        deployment_revision = event.deployment_revision,
                                        "Received deployment invalidation event"
                                    );
                                    cache.update_latest_revision(env_id, rev);
                                }
                            } else {
                                warn!(
                                    event_id = event.event_id,
                                    "Received invalidation event with no environment_id and cursor_expired=false, ignoring"
                                );
                            }
                            last_seen_event_id = Some(event.event_id);
                        }
                        Err(e) => {
                            warn!("Error receiving invalidation event: {e}, reconnecting");
                            break;
                        }
                    }
                }
            }
            Err(e) => {
                warn!("Failed to connect to invalidation stream: {e}");
            }
        }

        tokio::time::sleep(backoff).await;
        backoff = (backoff * 2).min(max_backoff);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use golem_common::base_model::account::AccountId;
    use golem_common::base_model::application::ApplicationId;
    use golem_common::model::agent::{
        AgentConstructor, AgentMode, AgentType, AgentTypeName, DataSchema, NamedElementSchemas,
        RegisteredAgentType, RegisteredAgentTypeImplementer, ResolvedAgentType, Snapshotting,
    };
    use golem_common::model::application::ApplicationName;
    use golem_common::model::component::{ComponentId, ComponentRevision};
    use golem_common::model::deployment::DeploymentRevision;
    use golem_common::model::environment::{EnvironmentId, EnvironmentName};
    use golem_common::model::Empty;
    use golem_service_base::clients::registry::RegistryServiceError;
    use golem_service_base::model::auth::AuthCtx;
    use std::sync::atomic::{AtomicU64, Ordering};
    use test_r::test;
    use uuid::Uuid;

    fn make_resolved(env_id: EnvironmentId, rev: DeploymentRevision) -> ResolvedAgentType {
        ResolvedAgentType {
            registered_agent_type: RegisteredAgentType {
                agent_type: AgentType {
                    type_name: AgentTypeName("test-agent".to_string()),
                    description: String::new(),
                    source_language: String::new(),
                    constructor: AgentConstructor {
                        name: None,
                        description: String::new(),
                        prompt_hint: None,
                        input_schema: DataSchema::Tuple(NamedElementSchemas {
                            elements: vec![],
                        }),
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
                },
            },
            environment_id: env_id,
            deployment_revision: rev,
        }
    }

    struct MockRegistryService {
        env_id: EnvironmentId,
        revision: AtomicU64,
    }

    impl MockRegistryService {
        fn new(env_id: EnvironmentId) -> Self {
            Self {
                env_id,
                revision: AtomicU64::new(0),
            }
        }

        fn set_revision(&self, rev: u64) {
            self.revision.store(rev, Ordering::SeqCst);
        }
    }

    #[async_trait::async_trait]
    impl RegistryService for MockRegistryService {
        async fn authenticate_token(
            &self,
            _: &golem_common::model::auth::TokenSecret,
        ) -> Result<AuthCtx, RegistryServiceError> {
            unimplemented!()
        }
        async fn get_auth_details_for_environment(
            &self,
            _: EnvironmentId,
            _: bool,
            _: &AuthCtx,
        ) -> Result<golem_service_base::model::auth::AuthDetailsForEnvironment, RegistryServiceError>
        {
            unimplemented!()
        }
        async fn get_resource_limits(
            &self,
            _: AccountId,
        ) -> Result<golem_service_base::model::ResourceLimits, RegistryServiceError> {
            unimplemented!()
        }
        async fn update_worker_limit(
            &self,
            _: AccountId,
            _: &golem_common::base_model::AgentId,
            _: bool,
        ) -> Result<(), RegistryServiceError> {
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
        async fn batch_update_fuel_usage(
            &self,
            _: std::collections::HashMap<AccountId, i64>,
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
        async fn resolve_latest_agent_type_by_names(
            &self,
            _: &AccountId,
            _: &ApplicationName,
            _: &EnvironmentName,
            _: &AgentTypeName,
        ) -> Result<golem_common::model::agent::RegisteredAgentType, RegistryServiceError> {
            unimplemented!()
        }
        async fn resolve_agent_type_at_deployment(
            &self,
            _: &AccountId,
            _: &ApplicationName,
            _: &EnvironmentName,
            _: &AgentTypeName,
            _: DeploymentRevision,
        ) -> Result<golem_common::model::agent::RegisteredAgentType, RegistryServiceError> {
            unimplemented!()
        }
        async fn resolve_agent_type_by_names(
            &self,
            _app_name: &ApplicationName,
            _environment_name: &EnvironmentName,
            _agent_type_name: &AgentTypeName,
            _deployment_revision: Option<DeploymentRevision>,
            _owner_account_email: Option<&str>,
            _auth_ctx: &AuthCtx,
        ) -> Result<ResolvedAgentType, RegistryServiceError> {
            let rev = self.revision.load(Ordering::SeqCst);
            Ok(make_resolved(
                self.env_id,
                DeploymentRevision::new(rev).unwrap_or(DeploymentRevision::INITIAL),
            ))
        }
        async fn get_active_routes_for_domain(
            &self,
            _: &golem_common::base_model::domain_registration::Domain,
        ) -> Result<golem_service_base::custom_api::CompiledRoutes, RegistryServiceError>
        {
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
        async fn subscribe_deployment_invalidations(
            &self,
            _: Option<u64>,
        ) -> Result<
            std::pin::Pin<
                Box<
                    dyn futures::Stream<
                            Item = Result<
                                golem_common::model::agent::DeploymentInvalidationEvent,
                                RegistryServiceError,
                            >,
                        > + Send,
                >,
            >,
            RegistryServiceError,
        > {
            unimplemented!()
        }
    }

    fn make_auth() -> AuthCtx {
        AuthCtx::System
    }

    fn make_cache(registry: Arc<dyn RegistryService>) -> AgentResolutionCache {
        AgentResolutionCache::new(registry, 10, Duration::from_secs(60), Duration::from_secs(60))
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
    }

    #[test]
    async fn test_cache_miss_different_key() {
        let env_id = EnvironmentId(Uuid::new_v4());
        let registry = Arc::new(MockRegistryService::new(env_id));
        let cache = make_cache(registry);

        let app = ApplicationName("app".to_string());
        let env = EnvironmentName("env".to_string());
        let agent = AgentTypeName("agent".to_string());

        let result = cache
            .resolve(&app, &env, &agent, None, &make_auth())
            .await;
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

        let result = cache
            .resolve(&app, &env, &agent, None, &make_auth())
            .await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap().deployment_revision, DeploymentRevision::INITIAL);

        // Simulate invalidation with a newer revision
        let new_rev = DeploymentRevision::new(1).unwrap();
        cache.update_latest_revision(env_id, new_rev);

        // Update mock to return new revision
        registry.set_revision(1);

        // Should re-resolve from registry since cached entry is stale
        let result2 = cache
            .resolve(&app, &env, &agent, None, &make_auth())
            .await;
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

        let result = cache
            .resolve(&app, &env, &agent, None, &make_auth())
            .await;
        assert!(result.is_ok());

        cache.update_latest_revision(env_id, DeploymentRevision::INITIAL);

        let result2 = cache
            .resolve(&app, &env, &agent, None, &make_auth())
            .await;
        assert!(result2.is_ok());
    }

    #[test]
    async fn test_clear_flushes_everything() {
        let env_id = EnvironmentId(Uuid::new_v4());
        let registry = Arc::new(MockRegistryService::new(env_id));
        let cache = make_cache(registry);

        let app = ApplicationName("app".to_string());
        let env = EnvironmentName("env".to_string());
        let agent = AgentTypeName("agent".to_string());

        let _ = cache
            .resolve(&app, &env, &agent, None, &make_auth())
            .await;
        cache.clear().await;

        // After clear, cache should re-resolve (which works fine since mock returns a value)
        let result = cache
            .resolve(&app, &env, &agent, None, &make_auth())
            .await;
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

        let r1 = cache
            .resolve(&app, &env, &agent, None, &make_auth())
            .await;
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
}
