use crate::gateway_security::{GolemIdentityProviderMetadata, SecurityScheme};

// This can exist as part of the middleware to initiate the authorisation workflow
// redirecting user to provider login page, or it can be part of the static binding
// serving the auth_call_back endpoint that's called by the provider after the user logs in.
#[derive(Debug, Clone, PartialEq)]
pub struct SecuritySchemeWithProviderMetadata {
    pub security_scheme: SecurityScheme,
    pub provider_metadata: GolemIdentityProviderMetadata,
}
