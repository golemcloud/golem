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

use crate::config::AuthServiceConfig;
use anyhow::anyhow;
use async_trait::async_trait;
use golem_common::cache::{BackgroundEvictionMode, Cache, FullCacheEvictionMode, SimpleCache};
use golem_common::model::account::AccountId;
use golem_common::model::auth::TokenSecret;
use golem_common::model::environment::EnvironmentId;
use golem_common::{SafeDisplay, error_forwarding};
use golem_service_base::clients::registry::{RegistryService, RegistryServiceError};
use golem_service_base::model::auth::AuthCtx;
use golem_service_base::model::auth::AuthorizationError;
use std::sync::Arc;
use tokio::sync::RwLock;

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
    async fn invalidate_tokens_for_account(&self, _account_id: AccountId) {}
    async fn invalidate_environment_auth(
        &self,
        _environment_id: EnvironmentId,
        _grantee_account_id: AccountId,
    ) {
    }
    async fn clear_all_caches(&self) {}
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

pub struct RemoteAuthService {
    client: Arc<dyn RegistryService>,
    auth_ctx_cache: Cache<TokenSecret, (), AuthCtx, AuthCtxCacheError>,
    invalidation: RwLock<u64>,
}

impl RemoteAuthService {
    pub fn new(client: Arc<dyn RegistryService>, config: &AuthServiceConfig) -> Self {
        Self {
            client,
            invalidation: RwLock::new(0),
            auth_ctx_cache: Cache::new(
                Some(config.auth_ctx_cache_max_capacity),
                FullCacheEvictionMode::LeastRecentlyUsed(1),
                BackgroundEvictionMode::OlderThan {
                    ttl: config.auth_ctx_cache_ttl,
                    period: config.auth_ctx_cache_eviction_period,
                },
                "token_secret_to_auth_ctx",
            ),
        }
    }
}

#[async_trait]
#[golem_async_trait::region_simple]
impl AuthService for RemoteAuthService {
    async fn invalidate_tokens_for_account(&self, account_id: AccountId) {
        let mut generation = self.invalidation.write().await;
        *generation += 1;
        let entries: Vec<TokenSecret> = self
            .auth_ctx_cache
            .iter()
            .await
            .into_iter()
            .filter(|(_, auth_ctx)| {
                auth_ctx.access_account_id() == account_id
                    || auth_ctx.actor_account_id() == account_id
            })
            .map(|(key, _)| key)
            .collect();
        for key in entries {
            self.auth_ctx_cache.remove(&key).await;
        }
    }

    async fn invalidate_environment_auth(
        &self,
        _environment_id: EnvironmentId,
        grantee_account_id: AccountId,
    ) {
        self.invalidate_tokens_for_account(grantee_account_id).await;
    }

    async fn clear_all_caches(&self) {
        let mut generation = self.invalidation.write().await;
        *generation += 1;
        let auth_keys = self.auth_ctx_cache.keys().await;
        for key in auth_keys {
            self.auth_ctx_cache.remove(&key).await;
        }
    }

    async fn authenticate_token(&self, token: TokenSecret) -> Result<AuthCtx, AuthServiceError> {
        let generation = {
            let guard = self.invalidation.read().await;
            if let Some(auth) = self.auth_ctx_cache.try_get(&token).await {
                return Ok(self
                    .auth_ctx_cache
                    .get_or_insert_simple(&token, async move || Ok(auth))
                    .await?);
            }
            *guard
        };
        let auth = self
            .client
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
            })?;
        let guard = self.invalidation.read().await;
        // A lookup started before invalidation may finish, but must not refill the cache.
        if *guard != generation {
            return Ok(auth);
        }
        Ok(self
            .auth_ctx_cache
            .get_or_insert_simple(&token, async move || Ok(auth))
            .await?)
    }
}
