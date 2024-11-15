use crate::gateway_security::SecuritySchemeInternal;
use openidconnect::Scope;

#[derive(Debug, Clone, PartialEq)]
pub struct HttpAuthorizer {
    pub scheme_internal: SecuritySchemeInternal,
}

impl HttpAuthorizer {
    pub fn get_scopes(&self) -> Vec<Scope> {
        self.scheme_internal
            .security_scheme
            .security_scheme
            .scopes()
    }
}
