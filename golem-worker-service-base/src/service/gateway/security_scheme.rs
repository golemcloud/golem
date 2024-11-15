use crate::gateway_middleware::SecuritySchemeWithProviderMetadata;
use crate::gateway_security::{
    IdentityProvider, IdentityProviderError, SchemeIdentifier, SecurityScheme,
};
use async_trait::async_trait;
use golem_common::cache::{Cache, SimpleCache};
use std::sync::Arc;

#[async_trait]
pub trait SecuritySchemeService<AuthCtx, Namespace> {
    async fn get(
        &self,
        security_scheme_name: &SchemeIdentifier,
        auth_ctx: AuthCtx,
        namespace: Namespace,
    ) -> Option<SecuritySchemeWithProviderMetadata>;
    async fn create(
        &self,
        auth_ctx: AuthCtx,
        namespace: Namespace,
        security_scheme: &SecurityScheme,
    ) -> Result<SecuritySchemeWithProviderMetadata, SecuritySchemeServiceError>;
}

pub enum SecuritySchemeServiceError {
    IdentityProviderError(IdentityProviderError),
    InternalError(String),
}

pub type SecuritySchemeCache<N> = Cache<
    (N, SchemeIdentifier),
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

impl<Namespace, AuthCtx> SecuritySchemeService<Namespace, AuthCtx>
    for DefaultSecuritySchemeService<Namespace>
{
    async fn get(
        &self,
        security_scheme_identifier: &SchemeIdentifier,
        namespace: Namespace,
        _auth_ctx: AuthCtx,
    ) -> Option<SecuritySchemeWithProviderMetadata> {
        // TODO; get_or_insert_simple with Repo
        self.cache
            .get(&(namespace, security_scheme_identifier.clone()))
    }

    async fn create(
        &self,
        namespace: Namespace,
        auth_ctx: AuthCtx,
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
                    .get_or_insert_simple(
                        &(namespace, security_scheme.scheme_identifier()),
                        security_scheme_with_provider_metadata.clone(),
                    )
                    .await?;

                Ok(result)
            }
            Err(err) => Err(SecuritySchemeServiceError::IdentityProviderError(err)),
        }
    }
}
