// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
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

use crate::config::AuthServiceConfig;
use anyhow::anyhow;
use async_trait::async_trait;
use golem_common::cache::{BackgroundEvictionMode, Cache, FullCacheEvictionMode, SimpleCache};
use golem_common::model::auth::TokenSecret;
use golem_common::model::environment::EnvironmentId;
use golem_common::{SafeDisplay, error_forwarding};
use golem_service_base::clients::registry::{RegistryService, RegistryServiceError};
use golem_service_base::model::auth::AuthorizationError;
use golem_service_base::model::auth::{AuthCtx, AuthDetailsForEnvironment, EnvironmentAction};
use std::sync::Arc;

#[derive(Debug, thiserror::Error)]
pub enum AuthServiceError {
    #[error("Could not authenticate user using token")]
    CouldNotAuthenticate,
    #[error(transparent)]
    Unauthorized(#[from] AuthorizationError),
    #[error(transparent)]
    InternalError(#[from] anyhow::Error),
}

impl SafeDisplay for AuthServiceError {
    fn to_safe_string(&self) -> String {
        match self {
            Self::CouldNotAuthenticate => self.to_string(),
            Self::Unauthorized(inner) => inner.to_safe_string(),
            Self::InternalError(_) => "Internal error".to_string(),
        }
    }
}

error_forwarding!(AuthServiceError, RegistryServiceError);

#[async_trait]
pub trait AuthService: Send + Sync {
    async fn authenticate_token(&self, token: TokenSecret) -> Result<AuthCtx, AuthServiceError>;
    async fn authorize_environment_actions(
        &self,
        environment_id: EnvironmentId,
        action: EnvironmentAction,
        auth_ctx: &AuthCtx,
    ) -> Result<AuthDetailsForEnvironment, AuthServiceError>;
}

#[derive(Clone)]
enum AuthCtxCacheError {
    CouldNotAuthenticate,
    Error,
}

impl From<AuthCtxCacheError> for AuthServiceError {
    fn from(value: AuthCtxCacheError) -> Self {
        match value {
            AuthCtxCacheError::CouldNotAuthenticate => AuthServiceError::CouldNotAuthenticate,
            AuthCtxCacheError::Error => {
                AuthServiceError::InternalError(anyhow!("Cached request failed"))
            }
        }
    }
}

#[derive(Clone)]
enum EnvironmentAuthDetailsCacheError {
    NotFound,
    Error,
}

pub struct RemoteAuthService {
    client: Arc<dyn RegistryService>,
    auth_ctx_cache: Cache<TokenSecret, (), AuthCtx, AuthCtxCacheError>,
    environment_auth_details_cache: Cache<
        (EnvironmentId, AuthCtx),
        (),
        AuthDetailsForEnvironment,
        EnvironmentAuthDetailsCacheError,
    >,
}

impl RemoteAuthService {
    pub fn new(client: Arc<dyn RegistryService>, config: &AuthServiceConfig) -> Self {
        Self {
            client,
            auth_ctx_cache: Cache::new(
                Some(config.auth_ctx_cache_max_capacity),
                FullCacheEvictionMode::LeastRecentlyUsed(1),
                BackgroundEvictionMode::OlderThan {
                    ttl: config.auth_ctx_cache_ttl,
                    period: config.auth_ctx_cache_eviction_period,
                },
                "token_secret_to_auth_ctx",
            ),
            environment_auth_details_cache: Cache::new(
                Some(config.environment_auth_details_cache_max_capacity),
                FullCacheEvictionMode::LeastRecentlyUsed(1),
                BackgroundEvictionMode::OlderThan {
                    ttl: config.environment_auth_details_cache_ttl,
                    period: config.environment_auth_details_cache_eviction_period,
                },
                "environment_id_to_auth_details",
            ),
        }
    }

    async fn auth_details_for_environment_id(
        &self,
        environment_id: EnvironmentId,
        auth_ctx: &AuthCtx,
    ) -> Result<Option<AuthDetailsForEnvironment>, AuthServiceError> {
        // environment level auth does not care about impersonation, so downgrade here to avoid cache
        // misses during rpc
        let impersonated_auth = auth_ctx.impersonated();
        let result = self
            .environment_auth_details_cache
            .get_or_insert_simple(
                &(environment_id, impersonated_auth.clone()),
                async move || {
                    self.client
                        .get_auth_details_for_environment(environment_id, false, &impersonated_auth)
                        .await
                        .map_err(|e| match e {
                            RegistryServiceError::NotFound(_) => {
                                EnvironmentAuthDetailsCacheError::NotFound
                            }
                            e => {
                                tracing::warn!("Authenticating user token failed: {e}");
                                EnvironmentAuthDetailsCacheError::Error
                            }
                        })
                },
            )
            .await
            .map(Some)
            .or_else(|e| match e {
                EnvironmentAuthDetailsCacheError::NotFound => Ok(None),
                EnvironmentAuthDetailsCacheError::Error => Err(anyhow!(
                    "Cached get_auth_details_for_environment request failed"
                )),
            })?;

        Ok(result)
    }
}

#[async_trait]
impl AuthService for RemoteAuthService {
    async fn authenticate_token(&self, token: TokenSecret) -> Result<AuthCtx, AuthServiceError> {
        let result = self
            .auth_ctx_cache
            .get_or_insert_simple(&token.clone(), async move || {
                self.client
                    .authenticate_token(&token)
                    .await
                    .map_err(|e| match e {
                        RegistryServiceError::CouldNotAuthenticate(_) => {
                            AuthCtxCacheError::CouldNotAuthenticate
                        }
                        e => {
                            tracing::warn!("Authenticating user token failed: {e}");
                            AuthCtxCacheError::Error
                        }
                    })
            })
            .await?;

        Ok(result)
    }

    async fn authorize_environment_actions(
        &self,
        environment_id: EnvironmentId,
        action: EnvironmentAction,
        auth_ctx: &AuthCtx,
    ) -> Result<AuthDetailsForEnvironment, AuthServiceError> {
        let environment_auth_details = self
            .auth_details_for_environment_id(environment_id, auth_ctx)
            .await?
            .ok_or(AuthServiceError::Unauthorized(
                AuthorizationError::EnvironmentActionNotAllowed(action),
            ))?;

        auth_ctx.authorize_environment_action(
            environment_auth_details.account_id_owning_environment,
            &environment_auth_details.environment_roles_from_shares,
            action,
        )?;

        Ok(environment_auth_details)
    }
}
