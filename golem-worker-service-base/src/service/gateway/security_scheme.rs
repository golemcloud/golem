// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use crate::gateway_security::{
    IdentityProvider, IdentityProviderError, SecurityScheme, SecuritySchemeIdentifier,
    SecuritySchemeWithProviderMetadata,
};
use crate::repo::security_scheme::{SecuritySchemeRecord, SecuritySchemeRepo};
use async_trait::async_trait;
use golem_common::cache::{BackgroundEvictionMode, Cache, FullCacheEvictionMode, SimpleCache};
use golem_common::SafeDisplay;
use std::fmt::Display;
use std::hash::Hash;
use std::sync::Arc;
use tracing::info;

// The controller phase can decide whether the developer of API deployment
// has create-security role in Namespace, before calling this service
#[async_trait]
pub trait SecuritySchemeService<Namespace> {
    async fn get(
        &self,
        security_scheme_name: &SecuritySchemeIdentifier,
        namespace: &Namespace,
    ) -> Result<SecuritySchemeWithProviderMetadata, SecuritySchemeServiceError>;

    async fn create(
        &self,
        namespace: &Namespace,
        security_scheme: &SecurityScheme,
    ) -> Result<SecuritySchemeWithProviderMetadata, SecuritySchemeServiceError>;
}

#[derive(Debug, Clone)]
pub enum SecuritySchemeServiceError {
    IdentityProviderError(IdentityProviderError),
    InternalError(String),
    NotFound(SecuritySchemeIdentifier),
}

// For satisfying thiserror::Error
// https://github.com/golemcloud/golem/issues/1071
impl Display for SecuritySchemeServiceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.to_safe_string())
    }
}

impl SafeDisplay for SecuritySchemeServiceError {
    fn to_safe_string(&self) -> String {
        match self {
            SecuritySchemeServiceError::IdentityProviderError(err) => {
                format!("IdentityProviderError: {}", err.to_safe_string())
            }
            SecuritySchemeServiceError::InternalError(err) => {
                format!("InternalError: {}", err)
            }
            SecuritySchemeServiceError::NotFound(identifier) => {
                format!("SecurityScheme not found: {}", identifier)
            }
        }
    }
}

pub type SecuritySchemeCache<N> = Cache<
    (N, SecuritySchemeIdentifier),
    (),
    SecuritySchemeWithProviderMetadata,
    SecuritySchemeServiceError,
>;
pub struct DefaultSecuritySchemeService<Namespace> {
    cache: SecuritySchemeCache<Namespace>,
    repo: Arc<dyn SecuritySchemeRepo + Sync + Send>,
    identity_provider: Arc<dyn IdentityProvider + Sync + Send>,
}

impl<Namespace: Send + Sync + Clone + Hash + Eq + 'static> DefaultSecuritySchemeService<Namespace> {
    pub fn new(
        repo: Arc<dyn SecuritySchemeRepo + Sync + Send>,
        identity_provider: Arc<dyn IdentityProvider + Sync + Send>,
    ) -> Self {
        DefaultSecuritySchemeService {
            cache: Cache::new(
                Some(1024),
                FullCacheEvictionMode::None,
                BackgroundEvictionMode::None,
                "security_scheme",
            ),
            repo,
            identity_provider,
        }
    }
}

#[async_trait]
impl<Namespace: Display + Clone + Hash + Eq + PartialEq + Send + Sync + 'static>
    SecuritySchemeService<Namespace> for DefaultSecuritySchemeService<Namespace>
{
    async fn get(
        &self,
        security_scheme_identifier: &SecuritySchemeIdentifier,
        namespace: &Namespace,
    ) -> Result<SecuritySchemeWithProviderMetadata, SecuritySchemeServiceError> {
        self.cache
            .get_or_insert_simple(
                &(namespace.clone(), security_scheme_identifier.clone()),
                || {
                    let security_scheme_identifier = security_scheme_identifier.clone();
                    let repo = self.repo.clone();
                    Box::pin(async move {
                        let result = repo
                            .get(&security_scheme_identifier.to_string())
                            .await
                            .map_err(|err| {
                                SecuritySchemeServiceError::InternalError(err.to_string())
                            })?;

                        match result {
                            Some(v) => SecuritySchemeWithProviderMetadata::try_from(v)
                                .map_err(SecuritySchemeServiceError::InternalError),
                            None => Err(SecuritySchemeServiceError::NotFound(
                                security_scheme_identifier.clone(),
                            )),
                        }
                    })
                },
            )
            .await
    }

    async fn create(
        &self,
        namespace: &Namespace,
        security_scheme: &SecurityScheme,
    ) -> Result<SecuritySchemeWithProviderMetadata, SecuritySchemeServiceError> {
        let identity_provider = &self.identity_provider;

        let provider_metadata = identity_provider
            .get_provider_metadata(&security_scheme.provider_type())
            .await;

        match provider_metadata {
            Ok(provider_metadata) => {
                info!("Provider metadata: {:?}", provider_metadata);

                let security_scheme_with_provider_metadata = SecuritySchemeWithProviderMetadata {
                    security_scheme: security_scheme.clone(),
                    provider_metadata,
                };

                let record = SecuritySchemeRecord::from_security_scheme_metadata(
                    namespace,
                    &security_scheme_with_provider_metadata,
                )
                .map_err(SecuritySchemeServiceError::InternalError)?;

                self.repo.create(&record).await.map_err(|err| {
                    SecuritySchemeServiceError::InternalError(err.to_safe_string())
                })?;

                let result = self
                    .get(&security_scheme.scheme_identifier(), namespace)
                    .await?;

                info!("Security scheme created: {:?}", result);

                Ok(security_scheme_with_provider_metadata)
            }
            Err(err) => Err(SecuritySchemeServiceError::IdentityProviderError(err)),
        }
    }
}
