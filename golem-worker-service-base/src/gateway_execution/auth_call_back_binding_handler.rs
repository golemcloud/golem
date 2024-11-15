use openidconnect::core::{CoreIdTokenClaims, CoreTokenResponse};
use golem_common::SafeDisplay;
use crate::gateway_binding::{HttpRequestDetails};
use crate::gateway_execution::gateway_session::GatewaySessionStore;
use crate::gateway_security::SecuritySchemeInternal;

pub trait AuthCallBackBindingHandler<Namespace> {
    async fn handle_auth_call_back(
        &self,
        namespace: &Namespace,
        http_request_details: &HttpRequestDetails,
        security_scheme_internal: &SecuritySchemeInternal,
        session: GatewaySessionStore,

    ) -> Result<AuthorisationSuccess, AuthorisationError>;
}

pub struct AuthorisationSuccess {
    pub token_response: CoreTokenResponse,
    pub token_claims: CoreIdTokenClaims,
}

pub enum AuthorisationError {
    InvalidSession,
    AccessTokenNotFound,
    IdTokenNotFound,
    ConflictingState, // Possible CSRF attack
    FailedCodeExchange,
    NonceNotFound,
    SessionUpdateError(String),
}

impl SafeDisplay for AuthorisationError {
    fn to_safe_string(&self) -> String {
        match self {
            AuthorisationError::InvalidSession => "The session is no longer valid.".to_string(),
            AuthorisationError::AccessTokenNotFound => "Unable to continue with authorisation".to_string(),
            AuthorisationError::IdTokenNotFound => "Unable to continue with authentication.".to_string(),
            AuthorisationError::ConflictingState => "Suspicious login attempt".to_string(),
            AuthorisationError::FailedCodeExchange => "Failed to complete the authentication process. Please try again.".to_string(),
            AuthorisationError::NonceNotFound => "Suspicious authorisation attempt. Failed checks.".to_string(),
            AuthorisationError::SessionUpdateError(err) => format!("An error occurred while updating the session. Error details: {}", err),
        }
    }
}