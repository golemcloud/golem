use crate::gateway_security::{
    IdentityProviderError, SecurityScheme, SecuritySchemeIdentifier,
    SecuritySchemeWithProviderMetadata,
};
use async_trait::async_trait;
use golem_common::cache::{BackgroundEvictionMode, Cache, FullCacheEvictionMode, SimpleCache};
use std::fmt::Display;
use std::hash::Hash;

// The controller phase can decide whether the developer of API deployment
// has create-security role in Namespace, before calling this service
#[async_trait]
pub trait   SecuritySchemeService<Namespace> {
    async fn get(
        &self,
        security_scheme_name: &SecuritySchemeIdentifier,
        namespace: &Namespace,
    ) -> Result<Option<SecuritySchemeWithProviderMetadata>, SecuritySchemeServiceError>;
    async fn create(
        &self,
        namespace: &Namespace,
        security_scheme: &SecurityScheme,
    ) -> Result<SecuritySchemeWithProviderMetadata, SecuritySchemeServiceError>;
}

#[derive(Clone)]
pub enum SecuritySchemeServiceError {
    IdentityProviderError(IdentityProviderError),
    InternalError(String),
}

impl Display for SecuritySchemeServiceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SecuritySchemeServiceError::IdentityProviderError(err) => {
                write!(f, "IdentityProviderError: {}", err)
            }
            SecuritySchemeServiceError::InternalError(err) => {
                write!(f, "InternalError: {}", err)
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
}

impl<Namespace: Send + Sync + Clone + Hash + Eq + 'static> Default
    for DefaultSecuritySchemeService<Namespace>
{
    fn default() -> Self {
        DefaultSecuritySchemeService {
            cache: Cache::new(
                Some(1024),
                FullCacheEvictionMode::None,
                BackgroundEvictionMode::None,
                "security_Scheme",
            ),
        }
    }
}

#[async_trait]
impl<Namespace: Clone + Hash + Eq + PartialEq + Send + Sync + 'static>
    SecuritySchemeService<Namespace> for DefaultSecuritySchemeService<Namespace>
{
    async fn get(
        &self,
        security_scheme_identifier: &SecuritySchemeIdentifier,
        namespace: &Namespace,
    ) -> Result<Option<SecuritySchemeWithProviderMetadata>, SecuritySchemeServiceError> {
        // TODO; get_or_insert_simple with Repo
        let result = self
            .cache
            .get(&(namespace.clone(), security_scheme_identifier.clone()))
            .await;

        Ok(result)
    }

    async fn create(
        &self,
        namespace: &Namespace,
        security_scheme: &SecurityScheme,
    ) -> Result<SecuritySchemeWithProviderMetadata, SecuritySchemeServiceError> {
        let identity_provider = security_scheme.identity_provider();

        let provider_metadata = identity_provider
            .get_provider_metadata(&security_scheme.provider_type())
            .await;

        match provider_metadata {
            Ok(provider_metadata) => {
                let security_scheme_with_provider_metadata = SecuritySchemeWithProviderMetadata {
                    security_scheme: security_scheme.clone(),
                    provider_metadata,
                };
                // TODO: get_or_insert_simple with Repo
                let result = self
                    .cache
                    .get_or_insert_simple(
                        &(namespace.clone(), security_scheme.scheme_identifier()),
                        || Box::pin(async move { Ok(security_scheme_with_provider_metadata) }),
                    )
                    .await?;

                Ok(result)
            }
            Err(err) => Err(SecuritySchemeServiceError::IdentityProviderError(err)),
        }
    }
}
