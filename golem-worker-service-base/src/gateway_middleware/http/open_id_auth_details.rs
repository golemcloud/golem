use std::sync::Arc;
use crate::gateway_security::{GolemIdentityProviderMetadata, IdentityProvider, SecurityScheme};

#[derive(Debug, Clone, PartialEq)]
    pub struct AuthCallBackDetails {
    security_scheme_name: SecurityScheme,
    provider_metadata: GolemIdentityProviderMetadata
}

#[derive(Debug, Clone)]
pub struct AuthCallBackDetailsInternal {
    pub security_scheme_name: SecurityScheme,
    pub provider_metadata: GolemIdentityProviderMetadata,
    pub identity_provider: Arc<dyn IdentityProvider + Send + Sync>
}
