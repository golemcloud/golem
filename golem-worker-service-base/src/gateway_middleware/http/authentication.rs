use openidconnect::Scope;
use crate::gateway_security::{SecuritySchemeInternal};

#[derive(Debug, Clone, PartialEq)]
pub struct HttpAuth {
    pub scheme_internal: SecuritySchemeInternal
}

impl HttpAuth {
    pub fn get_scopes(&self) -> Vec<Scope> {
        self.scheme_internal.security_scheme.security_scheme.scopes()
    }
}