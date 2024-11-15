use std::sync::Arc;
use crate::gateway_security::{GolemIdentityProviderMetadata, IdentityProvider, SecurityScheme};

// Just an internal data structure to hold the security scheme and the identity provider together
// for interoperation
#[derive(Debug, Clone)]
pub struct SecuritySchemeInternal {
    pub security_scheme_name: SecurityScheme,
    pub provider_metadata: GolemIdentityProviderMetadata,
    pub identity_provider: Arc<dyn IdentityProvider + Send + Sync>
}
