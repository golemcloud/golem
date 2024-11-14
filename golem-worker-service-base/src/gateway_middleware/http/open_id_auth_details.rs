use std::sync::Arc;
use crate::gateway_identity_provider::{GolemIdentityProviderMetadata, IdentityProvider, SecurityScheme};

#[derive(Debug, Clone, PartialEq)]
pub struct OpenIdProviderDetails {
    security_scheme_name: SecurityScheme,
    provider_metadata: GolemIdentityProviderMetadata
}

#[derive(Debug, Clone)]
pub struct OpenIdProviderDetailsWithClient {
    pub security_scheme_name: SecurityScheme,
    pub provider_metadata: GolemIdentityProviderMetadata,
    pub identity_provider: Arc<dyn IdentityProvider + Send + Sync>
}
