use crate::gateway_security::{IdentityProvider};
use std::sync::Arc;
use crate::gateway_middleware::SecuritySchemeWithProviderMetadata;

// Just an internal data structure to hold the security scheme and the identity provider together
// for interoperation
#[derive(Debug, Clone)]
pub struct SecuritySchemeInternal {
    pub security_scheme: SecuritySchemeWithProviderMetadata,
    pub identity_provider: Arc<dyn IdentityProvider + Send + Sync>,
}
