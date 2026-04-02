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

use crate::services::golem_config::RpcAuthCacheConfig;
use crate::services::rpc::RpcError;
use async_trait::async_trait;
use golem_common::cache::{BackgroundEvictionMode, Cache, FullCacheEvictionMode, SimpleCache};
use golem_common::model::account::AccountId;
use golem_common::model::environment::EnvironmentId;
use golem_service_base::clients::registry::{RegistryService, RegistryServiceError};
use golem_service_base::model::auth::{AuthCtx, AuthDetailsForEnvironment, EnvironmentAction};
use std::sync::Arc;

/// Service that encapsulates environment-level authorization checks for local RPC calls.
///
/// Uses a two-tier strategy:
/// - Fast path: if the caller owns the target environment, allow immediately with no extra network
///   call. The environment owner is obtained from the first `AuthDetailsForEnvironment` fetch and
///   cached keyed by `EnvironmentId` alone (owner never changes for a given env).
/// - Slow path: fetch `AuthDetailsForEnvironment` from registry-service and run the standard
///   `AuthCtx::authorize_environment_action` check. Results are cached keyed by
///   `(EnvironmentId, AccountId)` with a configurable TTL.
#[async_trait]
pub trait RpcEnvironmentAuthService: Send + Sync {
    /// Check whether `caller_account_id` is allowed to perform `action` on `environment_id`.
    ///
    /// Returns `Ok(())` if allowed, or `Err(RpcError::Denied { .. })` if not.
    async fn check(
        &self,
        caller_account_id: AccountId,
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
    registry_service: Arc<dyn RegistryService>,
    /// Cache of the environment owner, keyed by `EnvironmentId`.
    /// Populated on first auth-details fetch; the owner never changes for a given environment.
    env_owner_cache: Cache<EnvironmentId, (), AccountId, AuthDetailsCacheError>,
    /// Cache of full auth details keyed by `(EnvironmentId, caller_account_id)`.
    auth_details_cache:
        Cache<(EnvironmentId, AccountId), (), AuthDetailsForEnvironment, AuthDetailsCacheError>,
}

impl DefaultRpcEnvironmentAuthService {
    pub fn new(registry_service: Arc<dyn RegistryService>, config: &RpcAuthCacheConfig) -> Self {
        Self {
            registry_service,
            env_owner_cache: Cache::new(
                Some(config.cache_capacity),
                FullCacheEvictionMode::LeastRecentlyUsed(1),
                BackgroundEvictionMode::OlderThan {
                    ttl: config.cache_ttl,
                    period: config.cache_eviction_interval,
                },
                "rpc_environment_owner",
            ),
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

        self.auth_details_cache
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
            })
    }

    /// Returns the cached owner of `environment_id`, fetching it on first call.
    /// Populates the owner cache as a side-effect of the first auth-details fetch.
    async fn get_env_owner(
        &self,
        environment_id: EnvironmentId,
        caller_account_id: AccountId,
    ) -> Result<Option<AccountId>, RpcError> {
        // Fetch auth details (cached per (env_id, caller)); the response carries the env owner.
        let auth_details = self
            .get_auth_details(environment_id, caller_account_id)
            .await?;

        // Populate the per-env owner cache so subsequent callers with different account IDs can
        // skip the auth-details fetch entirely on the fast path.
        if let Some(ref details) = auth_details {
            let owner = details.account_id_owning_environment;
            let _ = self
                .env_owner_cache
                .get_or_insert_simple(&environment_id, || {
                    Box::pin(async move { Ok::<_, AuthDetailsCacheError>(owner) })
                })
                .await;
        }

        Ok(auth_details.map(|d| d.account_id_owning_environment))
    }
}

#[async_trait]
impl RpcEnvironmentAuthService for DefaultRpcEnvironmentAuthService {
    async fn check(
        &self,
        caller_account_id: AccountId,
        environment_id: EnvironmentId,
        action: EnvironmentAction,
    ) -> Result<(), RpcError> {
        // Fast path: if the caller owns the target environment, allow immediately.
        // We use account_id_owning_environment (from AuthDetailsForEnvironment) — not
        // component.account_id — because the component deployer and the env owner can differ.
        let env_owner = self
            .get_env_owner(environment_id, caller_account_id)
            .await?
            .ok_or_else(|| RpcError::Denied {
                details: format!("The environment action {action} is not allowed"),
            })?;

        if caller_account_id == env_owner {
            return Ok(());
        }

        // Slow path: auth details already cached by get_env_owner above; re-fetch from cache.
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
    use golem_service_base::model::component::Component;
    use std::collections::BTreeSet;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use test_r::test;

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

    fn make_auth_details(
        owner: AccountId,
        roles: impl IntoIterator<Item = EnvironmentRole>,
    ) -> AuthDetailsForEnvironment {
        AuthDetailsForEnvironment {
            account_id_owning_environment: owner,
            environment_roles_from_shares: BTreeSet::from_iter(roles),
        }
    }

    fn make_service(registry: Arc<dyn RegistryService>) -> DefaultRpcEnvironmentAuthService {
        DefaultRpcEnvironmentAuthService::new(registry, &RpcAuthCacheConfig::default())
    }

    // --- Tests ---

    #[test]
    async fn owner_fast_path_is_allowed() {
        let owner = make_account_id();
        let env_id = make_environment_id();
        // Registry returns auth_details with owner == caller — fast path triggers after first fetch.
        let auth_details = make_auth_details(owner, []);
        let registry = Arc::new(MockRegistryService::new(Some(auth_details)));
        let call_count = registry.call_count();
        let svc = make_service(registry);

        let result = svc
            .check(owner, env_id, EnvironmentAction::UpdateWorker)
            .await;

        assert!(result.is_ok(), "owner should be allowed: {result:?}");
        // Registry is called once to populate the owner cache; subsequent calls skip it.
        assert_eq!(
            call_count.load(Ordering::SeqCst),
            1,
            "registry should be called once to populate owner cache"
        );
    }

    #[test]
    async fn owner_fast_path_skips_registry_on_second_call() {
        let owner = make_account_id();
        let env_id = make_environment_id();
        let auth_details = make_auth_details(owner, []);
        let registry = Arc::new(MockRegistryService::new(Some(auth_details)));
        let call_count = registry.call_count();
        let svc = make_service(registry);

        // First call populates the owner cache.
        let _ = svc
            .check(owner, env_id, EnvironmentAction::UpdateWorker)
            .await;
        // Second call: owner cache hit, no registry call.
        let result = svc
            .check(owner, env_id, EnvironmentAction::UpdateWorker)
            .await;

        assert!(result.is_ok(), "owner should be allowed: {result:?}");
        assert_eq!(
            call_count.load(Ordering::SeqCst),
            1,
            "registry should not be called again after owner cache is warm"
        );
    }

    #[test]
    async fn non_owner_with_deployer_role_is_allowed_for_update_worker() {
        let owner = make_account_id();
        let caller = make_account_id();
        let env_id = make_environment_id();
        let auth_details = make_auth_details(owner, [EnvironmentRole::Deployer]);
        let registry = Arc::new(MockRegistryService::new(Some(auth_details)));
        let svc = make_service(registry);

        let result = svc
            .check(caller, env_id, EnvironmentAction::UpdateWorker)
            .await;

        assert!(
            result.is_ok(),
            "deployer should be allowed for UpdateWorker: {result:?}"
        );
    }

    #[test]
    async fn non_owner_with_no_shares_is_denied() {
        let caller = make_account_id();
        let env_id = make_environment_id();
        // Registry returns NotFound — no auth details, no shares
        let registry = Arc::new(MockRegistryService::new(None));
        let svc = make_service(registry);

        let result = svc
            .check(caller, env_id, EnvironmentAction::UpdateWorker)
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
        let svc = make_service(registry);

        let result = svc
            .check(caller, env_id, EnvironmentAction::UpdateWorker)
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
        let svc = make_service(registry);

        let result = svc
            .check(caller, env_id, EnvironmentAction::CreateWorker)
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
        let auth_details = make_auth_details(owner, [EnvironmentRole::Deployer]);
        let registry = Arc::new(MockRegistryService::new(Some(auth_details)));
        let call_count = registry.call_count();
        let svc = make_service(registry);

        // Two calls for the same (env, caller) pair
        let _ = svc
            .check(caller, env_id, EnvironmentAction::UpdateWorker)
            .await;
        let _ = svc
            .check(caller, env_id, EnvironmentAction::UpdateWorker)
            .await;

        assert_eq!(
            call_count.load(Ordering::SeqCst),
            1,
            "registry should be called only once due to caching"
        );
    }
}
