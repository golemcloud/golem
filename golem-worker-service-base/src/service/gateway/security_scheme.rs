use crate::gateway_security::{
    IdentityProvider, IdentityProviderError, SecurityScheme, SecuritySchemeIdentifier,
    SecuritySchemeWithProviderMetadata,
};
use async_trait::async_trait;
use golem_common::cache::{Cache, SimpleCache};
use std::fmt::Display;
use std::hash::Hash;
use std::sync::Arc;

// The controller phase can decide whether the developer of API deployment
// has create-security role in Namespace, before calling this service
#[async_trait]
pub trait SecuritySchemeService<Namespace> {
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
    identity_provider: Arc<dyn IdentityProvider + Send + Sync>,
    cache: SecuritySchemeCache<Namespace>,
}

impl<Namespace> DefaultSecuritySchemeService<Namespace> {
    pub fn new(
        identity_provider: Arc<dyn IdentityProvider + Send + Sync>,
        cache: SecuritySchemeCache<Namespace>,
    ) -> Self {
        DefaultSecuritySchemeService {
            identity_provider,
            cache,
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
            .get(&(namespace, security_scheme_identifier.clone()))
            .await;

        Ok(result)
    }

    async fn create(
        &self,
        namespace: &Namespace,
        security_scheme: &SecurityScheme,
    ) -> Result<SecuritySchemeWithProviderMetadata, SecuritySchemeServiceError> {
        let provider_metadata = self
            .identity_provider
            .get_provider_metadata(&security_scheme.issue_url())
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
                    .get_or_insert_simple(&(namespace, security_scheme.scheme_identifier()), || {
                        Box::pin(async move { Ok(security_scheme_with_provider_metadata) })
                    })
                    .await?;

                Ok(result)
            }
            Err(err) => Err(SecuritySchemeServiceError::IdentityProviderError(err)),
        }
    }
}
