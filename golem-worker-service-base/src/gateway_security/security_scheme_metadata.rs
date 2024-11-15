use crate::gateway_security::{GolemIdentityProviderMetadata, SecurityScheme};

#[derive(Debug, Clone, PartialEq)]
    pub struct SecuritySchemeWithProviderMetadata {
    pub security_scheme: SecurityScheme,
    pub provider_metadata: GolemIdentityProviderMetadata
}
