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

use crate::services::golem_config::DirectInvocationAuthCacheConfig;
use crate::services::rpc::RpcError;
use async_trait::async_trait;
use golem_common::cache::{BackgroundEvictionMode, Cache, FullCacheEvictionMode, SimpleCache};
use golem_common::model::OwnedAgentId;
use golem_common::model::account::{AccountEmail, AccountId};
use golem_common::model::card::owner::{AgentOwnerLeafPattern, AgentOwnerPattern};
use golem_common::model::card::{
    AgentResourcePattern, AgentVerb, ClassPermissionTarget, EffectiveSurface, PermissionTarget,
};
use golem_common::model::component::ComponentId;
use golem_service_base::clients::registry::{RegistryService, RegistryServiceError};
use golem_service_base::model::auth::AuthCtx;
use golem_service_base::model::component::Component;
use std::sync::Arc;

/// The account that owns the environment being accessed.
/// Returned by `DirectInvocationAuthService::check` to make it unambiguous at call sites
/// that this is the environment owner, not the caller.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EnvironmentOwnerAccountId(pub AccountId);

impl From<EnvironmentOwnerAccountId> for AccountId {
    fn from(value: EnvironmentOwnerAccountId) -> Self {
        value.0
    }
}

/// Service that encapsulates agent authorization checks for local RPC calls.
#[async_trait]
pub trait DirectInvocationAuthService: Send + Sync {
    async fn check(
        &self,
        caller_account_id: AccountId,
        caller_account_email: &AccountEmail,
        caller_effective_surface: &EffectiveSurface,
        owned_agent_id: &OwnedAgentId,
        verb: AgentVerb,
        resource: AgentResourcePattern,
    ) -> Result<EnvironmentOwnerAccountId, RpcError>;
}

#[derive(Clone)]
enum AuthDetailsCacheError {
    NotFound,
    Error,
}

pub struct DefaultDirectInvocationAuthService {
    registry_service: Arc<dyn RegistryService>,
    component_cache: Cache<ComponentId, (), Component, AuthDetailsCacheError>,
}

impl DefaultDirectInvocationAuthService {
    pub fn new(
        registry_service: Arc<dyn RegistryService>,
        config: &DirectInvocationAuthCacheConfig,
    ) -> Self {
        Self {
            registry_service,
            component_cache: Cache::new(
                Some(config.cache_capacity),
                FullCacheEvictionMode::LeastRecentlyUsed(1),
                BackgroundEvictionMode::OlderThan {
                    ttl: config.cache_ttl,
                    period: config.cache_eviction_interval,
                },
                "rpc_component_metadata",
            ),
        }
    }

    async fn get_component(
        &self,
        component_id: ComponentId,
    ) -> Result<Option<Component>, RpcError> {
        let registry_service = self.registry_service.clone();

        self.component_cache
            .get_or_insert_simple(&component_id, move || {
                Box::pin(async move {
                    registry_service
                        .get_deployed_component_metadata(component_id)
                        .await
                        .map_err(|e| match e {
                            RegistryServiceError::NotFound(_) => AuthDetailsCacheError::NotFound,
                            e => {
                                tracing::warn!(
                                    "Failed to get component metadata for component {component_id}: {e}"
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
                    details: "Failed to retrieve component metadata".to_string(),
                }),
            })
    }
}

#[async_trait]
impl DirectInvocationAuthService for DefaultDirectInvocationAuthService {
    async fn check(
        &self,
        caller_account_id: AccountId,
        caller_account_email: &AccountEmail,
        caller_effective_surface: &EffectiveSurface,
        owned_agent_id: &OwnedAgentId,
        verb: AgentVerb,
        resource: AgentResourcePattern,
    ) -> Result<EnvironmentOwnerAccountId, RpcError> {
        let component = self
            .get_component(owned_agent_id.component_id())
            .await?
            .ok_or_else(|| RpcError::Denied {
                details: format!("Component {} not found", owned_agent_id.component_id()),
            })?;

        let auth_ctx = AuthCtx::agent_with_effective_surface(
            caller_account_id,
            caller_account_email.clone(),
            caller_effective_surface.clone(),
        );

        auth_ctx
            .authorize_permission(&PermissionTarget::Agent(ClassPermissionTarget {
                owner: AgentOwnerPattern::Agent {
                    account: component.account_email.clone(),
                    application: component.application_name.clone(),
                    environment: component.environment_name.clone(),
                    component: component.component_name.clone(),
                    agent: AgentOwnerLeafPattern::Agent(owned_agent_id.agent_name()),
                },
                verb: Some(verb),
                resource,
            }))
            .map_err(|e| RpcError::Denied {
                details: e.to_string(),
            })?;

        Ok(EnvironmentOwnerAccountId(component.account_id))
    }
}

/// A no-op implementation of `DirectInvocationAuthService` that always permits all calls.
/// For use in test environments where authorization is not exercised.
/// Returns `caller_account_id` as the env owner (no-op context has no real owner).
pub struct NoOpDirectInvocationAuthService;

#[async_trait]
impl DirectInvocationAuthService for NoOpDirectInvocationAuthService {
    async fn check(
        &self,
        caller_account_id: AccountId,
        _caller_account_email: &AccountEmail,
        _caller_effective_surface: &EffectiveSurface,
        _owned_agent_id: &OwnedAgentId,
        _verb: AgentVerb,
        _resource: AgentResourcePattern,
    ) -> Result<EnvironmentOwnerAccountId, RpcError> {
        Ok(EnvironmentOwnerAccountId(caller_account_id))
    }
}
