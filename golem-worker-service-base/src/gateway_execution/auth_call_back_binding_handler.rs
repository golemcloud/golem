use crate::gateway_binding::HttpRequestDetails;
use crate::gateway_execution::gateway_session::{
    DataKey, DataValue, GatewaySessionStore, SessionId,
};
use crate::gateway_security::{IdentityProviderError, SecuritySchemeWithProviderMetadata};
use async_trait::async_trait;
use golem_common::SafeDisplay;
use openidconnect::core::{CoreIdTokenClaims, CoreTokenResponse};
use openidconnect::{AuthorizationCode, Nonce, OAuth2TokenResponse};

pub type AuthCallBackResult = Result<AuthorisationSuccess, AuthorisationError>;

#[async_trait]
pub trait AuthCallBackBindingHandler {
    async fn handle_auth_call_back(
        &self,
        http_request_details: &HttpRequestDetails,
        security_scheme_internal: &SecuritySchemeWithProviderMetadata,
        session: &GatewaySessionStore,
    ) -> AuthCallBackResult;
}

pub struct AuthorisationSuccess {
    pub token_response: CoreTokenResponse,
    pub token_claims: CoreIdTokenClaims,
}

pub enum AuthorisationError {
    CodeNotFound,
    InvalidCode,
    StateNotFound,
    InvalidState,
    InvalidSession,
    MissingParametersInSession,
    AccessTokenNotFound,
    IdTokenNotFound,
    ConflictingState, // Possible CSRF attack
    NonceNotFound,
    FailedCodeExchange(IdentityProviderError),
    ClaimFetchError(IdentityProviderError),
    IdentityProviderError(IdentityProviderError),
    SessionUpdateError(String),
}

// Only SafeDisplay is allowed for AuthorisationError
impl SafeDisplay for AuthorisationError {
    fn to_safe_string(&self) -> String {
        match self {
            AuthorisationError::CodeNotFound => "The authorisation code is missing.".to_string(),
            AuthorisationError::InvalidCode => "The authorisation code is invalid.".to_string(),
            AuthorisationError::StateNotFound => {
                "Missing parameters from identity provider".to_string()
            }
            AuthorisationError::InvalidState => {
                "Invalid parameters from identity provider.".to_string()
            }
            AuthorisationError::InvalidSession => "The session is no longer valid.".to_string(),
            AuthorisationError::MissingParametersInSession => "Session failures".to_string(),
            AuthorisationError::ClaimFetchError(err) => {
                format!("Failed to fetch claims. Error details: {}", err)
            }
            AuthorisationError::IdentityProviderError(err) => {
                format!("Identity provider error: {}", err)
            }
            AuthorisationError::AccessTokenNotFound => {
                "Unable to continue with authorisation".to_string()
            }
            AuthorisationError::IdTokenNotFound => {
                "Unable to continue with authentication.".to_string()
            }
            AuthorisationError::ConflictingState => "Suspicious login attempt".to_string(),
            AuthorisationError::FailedCodeExchange(err) => {
                format!("Failed to exchange code for tokens. Error details: {}", err)
            }
            AuthorisationError::NonceNotFound => {
                "Suspicious authorisation attempt. Failed checks.".to_string()
            }
            AuthorisationError::SessionUpdateError(err) => format!(
                "An error occurred while updating the session. Error details: {}",
                err
            ),
        }
    }
}

pub struct DefaultAuthCallBack;

#[async_trait]
impl AuthCallBackBindingHandler for DefaultAuthCallBack {
    async fn handle_auth_call_back(
        &self,
        http_request_details: &HttpRequestDetails,
        security_scheme_internal: &SecuritySchemeWithProviderMetadata,
        session_store: &GatewaySessionStore,
    ) -> Result<AuthorisationSuccess, AuthorisationError> {
        let query_params = &http_request_details.request_path_values;

        let code_value = query_params
            .get("code")
            .ok_or(AuthorisationError::CodeNotFound)?;

        let code = code_value.as_str().ok_or(AuthorisationError::InvalidCode)?;

        let authorisation_code = AuthorizationCode::new(code.to_string());

        let state_value = query_params
            .get("state")
            .ok_or(AuthorisationError::StateNotFound)?;

        let state_str = state_value
            .as_str()
            .ok_or(AuthorisationError::InvalidState)?;

        let obtained_state = state_str.to_string();

        let session_params = session_store
            .0
            .get_params(SessionId(obtained_state.to_string()))
            .await
            .map_err(|_| AuthorisationError::MissingParametersInSession)?
            .ok_or(AuthorisationError::InvalidSession)?;

        let nonce = session_params
            .get(&DataKey("nonce".to_string()))
            .ok_or(AuthorisationError::MissingParametersInSession)?
            .as_string()
            .ok_or(AuthorisationError::NonceNotFound)?;

        let open_id_client = security_scheme_internal
            .identity_provider()
            .get_client(&security_scheme_internal)
            .map_err(|err| AuthorisationError::IdentityProviderError(err))?;

        let token_response = security_scheme_internal
            .identity_provider()
            .exchange_code_for_tokens(&open_id_client, &authorisation_code)
            .await
            .map_err(|err| AuthorisationError::FailedCodeExchange(err))?;

        let claims = security_scheme_internal
            .identity_provider()
            .get_claims(
                &open_id_client,
                token_response.clone(),
                &Nonce::new(nonce.clone()),
            )
            .map_err(|err| AuthorisationError::ClaimFetchError(err))?;

        let _ = session_store
            .0
            .insert(
                SessionId(obtained_state.to_string()),
                DataKey("claims".to_string()),
                DataValue(serde_json::to_value(claims.clone()).unwrap()), // TODO;
            )
            .await
            .map_err(|err| AuthorisationError::SessionUpdateError(err.to_string()))?;

        let access_token = token_response.access_token().secret().clone();

        // access token in session store
        let _ = session_store
            .0
            .insert(
                SessionId(obtained_state.to_string()),
                DataKey("access_token".to_string()),
                DataValue(serde_json::Value::String(access_token)),
            )
            .await
            .map_err(|err| AuthorisationError::SessionUpdateError(err.to_string()))?;

        Ok(AuthorisationSuccess {
            token_response,
            token_claims: claims,
        })
    }
}
