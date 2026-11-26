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
use golem_service_base::service::type_id_resolution::InvalidationGenerations;
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
    auth_ctx_cache: Cache<TokenSecret, (), FetchedAuthCtx, (u64, AuthCtxCacheError)>,
    generations: Arc<InvalidationGenerations<AccountId>>,
}

#[derive(Clone)]
struct FetchedAuthCtx {
    generation: u64,
    auth: AuthCtx,
}

impl RemoteAuthService {
    pub fn new(client: Arc<dyn RegistryService>, config: &AuthServiceConfig) -> Self {
        Self {
            client,
            generations: Arc::default(),
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
        let generation = self.generations.invalidate(account_id);
        for (key, fetched) in self.auth_ctx_cache.iter().await {
            if fetched.auth.access_account_id() == account_id
                || fetched.auth.actor_account_id() == account_id
            {
                self.auth_ctx_cache
                    .remove_if_cached(&key, |cached| cached.generation < generation)
                    .await;
            }
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
        let generation = self.generations.invalidate_all();
        for (key, _) in self.auth_ctx_cache.iter().await {
            self.auth_ctx_cache
                .remove_if_cached(&key, |cached| cached.generation < generation)
                .await;
        }
    }

    async fn authenticate_token(&self, token: TokenSecret) -> Result<AuthCtx, AuthServiceError> {
        let request_generation = self.generations.current();
        loop {
            let client = self.client.clone();
            let generations = self.generations.clone();
            let lookup_token = token.clone();
            let fetched = self
                .auth_ctx_cache
                .get_or_insert_simple_spawned(&token, move || async move {
                    let generation = generations.current();
                    let auth = client
                        .authenticate_token(&lookup_token)
                        .await
                        .map_err(|e| {
                            let error = match e {
                                RegistryServiceError::CouldNotAuthenticate(_) => {
                                    AuthCtxCacheError::CouldNotAuthenticate
                                }
                                e => {
                                    tracing::warn!("Authenticating user token failed: {e}");
                                    AuthCtxCacheError::Error
                                }
                            };
                            (generation, error)
                        })?;
                    Ok(FetchedAuthCtx { generation, auth })
                })
                .await;
            let fetched = match fetched {
                Ok(fetched) => fetched,
                // Errors have no account identity. Retry an older shared error after any
                // invalidation that preceded this request, without discarding unrelated successes.
                Err((generation, _)) if generation < request_generation => continue,
                Err((_, error)) => return Err(error.into()),
            };
            if self
                .generations
                .is_current(fetched.auth.access_account_id(), fetched.generation)
                && self
                    .generations
                    .is_current(fetched.auth.actor_account_id(), fetched.generation)
            {
                return Ok(fetched.auth);
            }
            self.auth_ctx_cache
                .remove_if_cached(&token, |cached| cached.generation == fetched.generation)
                .await;
        }
    }
}
