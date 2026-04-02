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

use crate::services::component::ComponentService;
use crate::services::golem_config::RpcAuthCacheConfig;
use crate::services::rpc::RpcError;
use async_trait::async_trait;
use golem_common::cache::{BackgroundEvictionMode, Cache, FullCacheEvictionMode, SimpleCache};
use golem_common::model::account::AccountId;
use golem_common::model::component::ComponentId;
use golem_common::model::environment::EnvironmentId;
use golem_service_base::clients::registry::{RegistryService, RegistryServiceError};
use golem_service_base::model::auth::{AuthCtx, AuthDetailsForEnvironment, EnvironmentAction};
use std::sync::Arc;

/// Service that encapsulates environment-level authorization checks for local RPC calls.
///
/// Uses a two-tier strategy:
/// - Fast path: if caller is the environment owner (Component.account_id match), allow immediately.
/// - Slow path: fetch AuthDetailsForEnvironment from registry-service and run the standard
///   AuthCtx::authorize_environment_action check. Results are cached with a configurable TTL.
#[async_trait]
pub trait RpcEnvironmentAuthService: Send + Sync {
    /// Check whether `caller_account_id` is allowed to perform `action` on the environment
    /// that owns the component identified by `component_id` and `environment_id`.
    ///
    /// Returns `Ok(())` if allowed, or `Err(RpcError::Denied { .. })` if not.
    async fn check(
        &self,
        caller_account_id: AccountId,
        component_id: ComponentId,
        environment_id: EnvironmentId,
        action: EnvironmentAction,
    ) -> Result<(), RpcError>;
}

#[derive(Clone)]
enum AuthDetailsCacheError {
    NotFound,
    Error,
}

pub struct DefaultRpcEnvironmentAuthService {
    component_service: Arc<dyn ComponentService>,
    registry_service: Arc<dyn RegistryService>,
    // Cache keyed by (environment_id, caller_account_id)
    auth_details_cache:
        Cache<(EnvironmentId, AccountId), (), AuthDetailsForEnvironment, AuthDetailsCacheError>,
}

impl DefaultRpcEnvironmentAuthService {
    pub fn new(
        component_service: Arc<dyn ComponentService>,
        registry_service: Arc<dyn RegistryService>,
        config: &RpcAuthCacheConfig,
    ) -> Self {
        Self {
            component_service,
            registry_service,
            auth_details_cache: Cache::new(
                Some(config.cache_capacity),
                FullCacheEvictionMode::LeastRecentlyUsed(1),
                BackgroundEvictionMode::OlderThan {
                    ttl: config.cache_ttl,
                    period: config.cache_eviction_interval,
                },
                "rpc_environment_auth_details",
            ),
        }
    }

    async fn get_auth_details(
        &self,
        environment_id: EnvironmentId,
        caller_account_id: AccountId,
    ) -> Result<Option<AuthDetailsForEnvironment>, RpcError> {
        let registry_service = self.registry_service.clone();
        let auth_ctx = AuthCtx::impersonated_user(caller_account_id);

        let result = self
            .auth_details_cache
            .get_or_insert_simple(&(environment_id, caller_account_id), move || {
                Box::pin(async move {
                    registry_service
                        .get_auth_details_for_environment(environment_id, false, &auth_ctx)
                        .await
                        .map_err(|e| match e {
                            RegistryServiceError::NotFound(_) => AuthDetailsCacheError::NotFound,
                            e => {
                                tracing::warn!(
                                    "Failed to get auth details for environment {environment_id}: {e}"
                                );
                                AuthDetailsCacheError::Error
                            }
                        })
                })
            })
            .await
            .map(Some)
            .or_else(|e| match e {
                AuthDetailsCacheError::NotFound => Ok(None),
                AuthDetailsCacheError::Error => Err(RpcError::RemoteInternalError {
                    details: "Failed to retrieve environment auth details".to_string(),
                }),
            })?;

        Ok(result)
    }
}

#[async_trait]
impl RpcEnvironmentAuthService for DefaultRpcEnvironmentAuthService {
    async fn check(
        &self,
        caller_account_id: AccountId,
        component_id: ComponentId,
        environment_id: EnvironmentId,
        action: EnvironmentAction,
    ) -> Result<(), RpcError> {
        // Fast path: look up component owner from cached metadata
        let component = self
            .component_service
            .get_metadata(component_id, None)
            .await
            .map_err(|e| RpcError::RemoteInternalError {
                details: format!("Failed to retrieve component metadata: {e}"),
            })?;

        if caller_account_id == component.account_id {
            // Caller is the environment owner — allow immediately with no network call
            return Ok(());
        }

        // Slow path: fetch auth details from registry-service (cached)
        let auth_details = self
            .get_auth_details(environment_id, caller_account_id)
            .await?
            .ok_or_else(|| RpcError::Denied {
                details: format!("The environment action {action} is not allowed"),
            })?;

        let auth_ctx = AuthCtx::impersonated_user(caller_account_id);
        auth_ctx
            .authorize_environment_action(
                auth_details.account_id_owning_environment,
                &auth_details.environment_roles_from_shares,
                action,
            )
            .map_err(|e| RpcError::Denied {
                details: e.to_string(),
            })
    }
}

/// A no-op implementation of `RpcEnvironmentAuthService` that always permits all calls.
/// For use in test environments where authorization is not exercised.
pub struct NoOpRpcEnvironmentAuthService;

#[async_trait]
impl RpcEnvironmentAuthService for NoOpRpcEnvironmentAuthService {
    async fn check(
        &self,
        _caller_account_id: AccountId,
        _component_id: ComponentId,
        _environment_id: EnvironmentId,
        _action: EnvironmentAction,
    ) -> Result<(), RpcError> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use golem_common::model::auth::EnvironmentRole;
    use golem_common::model::component::{ComponentId, ComponentRevision};
    use golem_service_base::clients::registry::RegistryServiceError;
    use golem_service_base::error::worker_executor::WorkerExecutorError;
    use golem_service_base::model::component::Component;
    use std::collections::BTreeSet;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use test_r::test;

    // --- Mock ComponentService ---

    struct MockComponentService {
        owner_account_id: AccountId,
    }

    #[async_trait]
    impl ComponentService for MockComponentService {
        async fn get(
            &self,
            _engine: &wasmtime::Engine,
            _component_id: ComponentId,
            _component_revision: ComponentRevision,
        ) -> Result<(wasmtime::component::Component, Component), WorkerExecutorError> {
            unimplemented!("not needed for auth tests")
        }

        async fn get_metadata(
            &self,
            _component_id: ComponentId,
            _forced_revision: Option<ComponentRevision>,
        ) -> Result<Component, WorkerExecutorError> {
            Ok(make_component(self.owner_account_id))
        }

        async fn resolve_component(
            &self,
            _component_reference: String,
            _resolving_environment: golem_common::model::environment::EnvironmentId,
            _resolving_application: golem_common::model::application::ApplicationId,
            _resolving_account: AccountId,
        ) -> Result<Option<ComponentId>, WorkerExecutorError> {
            unimplemented!("not needed for auth tests")
        }

        async fn all_cached_metadata(&self) -> Vec<Component> {
            vec![]
        }
    }

    // --- Mock RegistryService ---

    struct MockRegistryService {
        auth_details: Option<AuthDetailsForEnvironment>,
        call_count: Arc<AtomicUsize>,
    }

    impl MockRegistryService {
        fn new(auth_details: Option<AuthDetailsForEnvironment>) -> Self {
            Self {
                auth_details,
                call_count: Arc::new(AtomicUsize::new(0)),
            }
        }

        fn call_count(&self) -> Arc<AtomicUsize> {
            self.call_count.clone()
        }
    }

    #[async_trait]
    impl RegistryService for MockRegistryService {
        async fn authenticate_token(
            &self,
            _token: &golem_common::model::auth::TokenSecret,
        ) -> Result<AuthCtx, RegistryServiceError> {
            unimplemented!()
        }

        async fn get_auth_details_for_environment(
            &self,
            _environment_id: EnvironmentId,
            _include_deleted: bool,
            _auth_ctx: &AuthCtx,
        ) -> Result<AuthDetailsForEnvironment, RegistryServiceError> {
            self.call_count.fetch_add(1, Ordering::SeqCst);
            self.auth_details
                .clone()
                .ok_or_else(|| RegistryServiceError::NotFound("not found".to_string()))
        }

        async fn get_resource_limits(
            &self,
            _account_id: AccountId,
        ) -> Result<golem_service_base::model::ResourceLimits, RegistryServiceError> {
            unimplemented!()
        }

        async fn update_worker_limit(
            &self,
            _account_id: AccountId,
            _agent_id: &golem_common::model::AgentId,
            _added: bool,
        ) -> Result<(), RegistryServiceError> {
            unimplemented!()
        }

        async fn update_worker_connection_limit(
            &self,
            _account_id: AccountId,
            _agent_id: &golem_common::model::AgentId,
            _added: bool,
        ) -> Result<(), RegistryServiceError> {
            unimplemented!()
        }

        async fn batch_update_resource_usage(
            &self,
            _updates: std::collections::HashMap<
                AccountId,
                golem_service_base::clients::registry::ResourceUsageUpdate,
            >,
        ) -> Result<golem_service_base::model::AccountResourceLimits, RegistryServiceError>
        {
            unimplemented!()
        }

        async fn download_component(
            &self,
            _component_id: ComponentId,
            _component_revision: ComponentRevision,
        ) -> Result<Vec<u8>, RegistryServiceError> {
            unimplemented!()
        }

        async fn get_component_metadata(
            &self,
            _component_id: ComponentId,
            _component_revision: ComponentRevision,
        ) -> Result<Component, RegistryServiceError> {
            unimplemented!()
        }

        async fn get_deployed_component_metadata(
            &self,
            _component_id: ComponentId,
        ) -> Result<Component, RegistryServiceError> {
            unimplemented!()
        }

        async fn get_all_deployed_component_revisions(
            &self,
            _component_id: ComponentId,
        ) -> Result<Vec<Component>, RegistryServiceError> {
            unimplemented!()
        }

        async fn resolve_component(
            &self,
            _resolving_account_id: AccountId,
            _resolving_application_id: golem_common::model::application::ApplicationId,
            _resolving_environment_id: EnvironmentId,
            _component_slug: &str,
        ) -> Result<Component, RegistryServiceError> {
            unimplemented!()
        }

        async fn get_all_agent_types(
            &self,
            _environment_id: EnvironmentId,
            _component_id: ComponentId,
            _component_revision: ComponentRevision,
        ) -> Result<Vec<golem_common::model::agent::RegisteredAgentType>, RegistryServiceError>
        {
            unimplemented!()
        }

        async fn get_agent_type(
            &self,
            _environment_id: EnvironmentId,
            _component_id: ComponentId,
            _component_revision: ComponentRevision,
            _name: &golem_common::model::agent::AgentTypeName,
        ) -> Result<golem_common::model::agent::RegisteredAgentType, RegistryServiceError> {
            unimplemented!()
        }

        async fn resolve_agent_type_by_names(
            &self,
            _app_name: &golem_common::model::application::ApplicationName,
            _environment_name: &golem_common::model::environment::EnvironmentName,
            _agent_type_name: &golem_common::model::agent::AgentTypeName,
            _deployment_revision: Option<golem_common::model::deployment::DeploymentRevision>,
            _owner_account_email: Option<&str>,
            _auth_ctx: &AuthCtx,
        ) -> Result<golem_common::model::agent::ResolvedAgentType, RegistryServiceError> {
            unimplemented!()
        }

        async fn get_active_routes_for_domain(
            &self,
            _domain: &golem_common::model::domain_registration::Domain,
        ) -> Result<golem_service_base::custom_api::CompiledRoutes, RegistryServiceError> {
            unimplemented!()
        }

        async fn get_active_compiled_mcps_for_domain(
            &self,
            _domain: &golem_common::model::domain_registration::Domain,
        ) -> Result<golem_service_base::mcp::CompiledMcp, RegistryServiceError> {
            unimplemented!()
        }

        async fn get_current_environment_state(
            &self,
            _environment_id: EnvironmentId,
        ) -> Result<golem_service_base::model::environment::EnvironmentState, RegistryServiceError>
        {
            unimplemented!()
        }

        async fn get_resource_definition_by_id(
            &self,
            _resource_definition_id: golem_common::model::resource_definition::ResourceDefinitionId,
        ) -> Result<
            golem_common::model::resource_definition::ResourceDefinition,
            RegistryServiceError,
        > {
            unimplemented!()
        }

        async fn get_resource_definition_by_name(
            &self,
            _environment_id: EnvironmentId,
            _resource_name: golem_common::model::resource_definition::ResourceName,
        ) -> Result<
            golem_common::model::resource_definition::ResourceDefinition,
            RegistryServiceError,
        > {
            unimplemented!()
        }

        async fn subscribe_registry_invalidations(
            &self,
            _last_seen_event_id: Option<u64>,
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
            _handler: Arc<dyn golem_service_base::clients::registry::RegistryInvalidationHandler>,
        ) {
            unimplemented!()
        }
    }

    // --- Helpers ---

    fn make_account_id() -> AccountId {
        AccountId::new()
    }

    fn make_environment_id() -> EnvironmentId {
        EnvironmentId::new()
    }

    fn make_component_id() -> ComponentId {
        ComponentId::new()
    }

    fn make_component(owner: AccountId) -> Component {
        use golem_common::model::application::ApplicationId;
        use golem_common::model::component::{ComponentName, ComponentRevision};
        use golem_common::model::component_metadata::ComponentMetadata;
        use golem_common::model::diff::Hash;
        use golem_service_base::model::component::Component;

        Component {
            id: make_component_id(),
            revision: ComponentRevision::INITIAL,
            environment_id: make_environment_id(),
            component_name: ComponentName("test".to_string()),
            hash: Hash::empty(),
            application_id: ApplicationId::new(),
            account_id: owner,
            component_size: 0,
            metadata: ComponentMetadata::from_parts(vec![], vec![], None, None, vec![]),
            created_at: chrono::Utc::now(),
            files: vec![],
            installed_plugins: vec![],
            env: std::collections::BTreeMap::new(),
            config_vars: std::collections::BTreeMap::new(),
            agent_config: vec![],
            wasm_hash: Hash::empty(),
            object_store_key: String::new(),
        }
    }

    fn make_auth_details(
        owner: AccountId,
        roles: impl IntoIterator<Item = EnvironmentRole>,
    ) -> AuthDetailsForEnvironment {
        AuthDetailsForEnvironment {
            account_id_owning_environment: owner,
            environment_roles_from_shares: BTreeSet::from_iter(roles),
        }
    }

    fn make_service(
        owner: AccountId,
        registry: Arc<dyn RegistryService>,
    ) -> DefaultRpcEnvironmentAuthService {
        DefaultRpcEnvironmentAuthService::new(
            Arc::new(MockComponentService {
                owner_account_id: owner,
            }),
            registry,
            &RpcAuthCacheConfig::default(),
        )
    }

    // --- Tests ---

    #[test]
    async fn owner_fast_path_is_allowed() {
        let owner = make_account_id();
        // Registry will never be called on the fast path
        let registry = Arc::new(MockRegistryService::new(None));
        let call_count = registry.call_count();
        let svc = make_service(owner, registry);

        let result = svc
            .check(
                owner,
                make_component_id(),
                make_environment_id(),
                EnvironmentAction::UpdateWorker,
            )
            .await;

        assert!(result.is_ok(), "owner should be allowed: {result:?}");
        assert_eq!(
            call_count.load(Ordering::SeqCst),
            0,
            "registry should not be called for owner"
        );
    }

    #[test]
    async fn non_owner_with_deployer_role_is_allowed_for_update_worker() {
        let owner = make_account_id();
        let caller = make_account_id();
        let env_id = make_environment_id();
        let auth_details = make_auth_details(owner, [EnvironmentRole::Deployer]);
        let registry = Arc::new(MockRegistryService::new(Some(auth_details)));
        let svc = make_service(owner, registry);

        let result = svc
            .check(
                caller,
                make_component_id(),
                env_id,
                EnvironmentAction::UpdateWorker,
            )
            .await;

        assert!(
            result.is_ok(),
            "deployer should be allowed for UpdateWorker: {result:?}"
        );
    }

    #[test]
    async fn non_owner_with_no_shares_is_denied() {
        let owner = make_account_id();
        let caller = make_account_id();
        let env_id = make_environment_id();
        // No auth details found in registry (simulates no shares)
        let registry = Arc::new(MockRegistryService::new(None));
        let svc = make_service(owner, registry);

        let result = svc
            .check(
                caller,
                make_component_id(),
                env_id,
                EnvironmentAction::UpdateWorker,
            )
            .await;

        assert!(
            matches!(result, Err(RpcError::Denied { .. })),
            "non-owner with no shares should be denied: {result:?}"
        );
    }

    #[test]
    async fn non_owner_with_viewer_role_is_denied_for_update_worker() {
        let owner = make_account_id();
        let caller = make_account_id();
        let env_id = make_environment_id();
        // Viewer role is not sufficient for UpdateWorker
        let auth_details = make_auth_details(owner, [EnvironmentRole::Viewer]);
        let registry = Arc::new(MockRegistryService::new(Some(auth_details)));
        let svc = make_service(owner, registry);

        let result = svc
            .check(
                caller,
                make_component_id(),
                env_id,
                EnvironmentAction::UpdateWorker,
            )
            .await;

        assert!(
            matches!(result, Err(RpcError::Denied { .. })),
            "viewer should not be allowed for UpdateWorker: {result:?}"
        );
    }

    #[test]
    async fn create_worker_action_requires_appropriate_role() {
        let owner = make_account_id();
        let caller = make_account_id();
        let env_id = make_environment_id();
        // Viewer is sufficient for CreateWorker per the permission model
        let auth_details = make_auth_details(owner, [EnvironmentRole::Viewer]);
        let registry = Arc::new(MockRegistryService::new(Some(auth_details)));
        let svc = make_service(owner, registry);

        let result = svc
            .check(
                caller,
                make_component_id(),
                env_id,
                EnvironmentAction::CreateWorker,
            )
            .await;

        assert!(
            result.is_ok(),
            "viewer should be allowed for CreateWorker: {result:?}"
        );
    }

    #[test]
    async fn cache_prevents_duplicate_registry_calls() {
        let owner = make_account_id();
        let caller = make_account_id();
        let env_id = make_environment_id();
        let component_id = make_component_id();
        let auth_details = make_auth_details(owner, [EnvironmentRole::Deployer]);
        let registry = Arc::new(MockRegistryService::new(Some(auth_details)));
        let call_count = registry.call_count();
        let svc = make_service(owner, registry);

        // Two calls for the same (env, caller) pair
        let _ = svc
            .check(
                caller,
                component_id,
                env_id,
                EnvironmentAction::UpdateWorker,
            )
            .await;
        let _ = svc
            .check(
                caller,
                component_id,
                env_id,
                EnvironmentAction::UpdateWorker,
            )
            .await;

        assert_eq!(
            call_count.load(Ordering::SeqCst),
            1,
            "registry should be called only once due to caching"
        );
    }
}
