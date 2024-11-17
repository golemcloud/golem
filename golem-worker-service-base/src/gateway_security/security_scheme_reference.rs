use crate::gateway_security::{SecuritySchemeIdentifier, SecuritySchemeWithProviderMetadata};
use std::fmt::Display;

#[derive(Debug, Clone, PartialEq)]
pub struct SecuritySchemeReference {
    pub security_scheme_identifier: SecuritySchemeIdentifier,
}

impl From<SecuritySchemeWithProviderMetadata> for SecuritySchemeReference {
    fn from(value: SecuritySchemeWithProviderMetadata) -> Self {
        SecuritySchemeReference {
            security_scheme_identifier: value.security_scheme.scheme_identifier(),
        }
    }
}
