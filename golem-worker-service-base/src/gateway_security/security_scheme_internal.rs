use crate::gateway_security::{IdentityProvider, SecuritySchemeWithProviderMetadata};
use std::sync::Arc;

// Just an internal data structure to hold the security scheme and the identity provider together
// for interoperation
#[derive(Debug, Clone, PartialEq)]
pub struct SecuritySchemeInternal {
    pub security_scheme: SecuritySchemeWithProviderMetadata,
    pub identity_provider: Arc<dyn IdentityProvider + Send + Sync>,
}
