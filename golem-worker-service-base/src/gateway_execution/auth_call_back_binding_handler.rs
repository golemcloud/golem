// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use crate::gateway_execution::gateway_session::{
    DataKey, DataValue, GatewaySessionError, GatewaySessionStore, SessionId,
};
use crate::gateway_security::{
    IdentityProvider, IdentityProviderError, SecuritySchemeWithProviderMetadata,
};
use async_trait::async_trait;
use golem_common::SafeDisplay;
use openidconnect::core::CoreTokenResponse;
use openidconnect::{AuthorizationCode, OAuth2TokenResponse};
use std::collections::HashMap;
use std::sync::Arc;

pub type AuthCallBackResult = Result<AuthorisationSuccess, AuthorisationError>;

#[async_trait]
pub trait AuthCallBackBindingHandler {
    async fn handle_auth_call_back(
        &self,
        query_params: &HashMap<String, String>,
        security_scheme: &SecuritySchemeWithProviderMetadata,
        gateway_session_store: &GatewaySessionStore,
        identity_provider: &Arc<dyn IdentityProvider + Send + Sync>,
    ) -> AuthCallBackResult;
}

pub struct AuthorisationSuccess {
    pub token_response: CoreTokenResponse,
    pub target_path: String,
    pub id_token: Option<String>,
    pub access_token: String,
    pub session: String,
}

#[derive(Debug)]
pub enum AuthorisationError {
    Internal(String),
    CodeNotFound,
    InvalidCode,
    StateNotFound,
    InvalidState,
    InvalidSession,
    InvalidNonce,
    MissingParametersInSession,
    AccessTokenNotFound,
    InvalidToken,
    IdTokenNotFound,
    ConflictingState, // Possible CSRF attack
    NonceNotFound,
    FailedCodeExchange(IdentityProviderError),
    ClaimFetchError(IdentityProviderError),
    IdentityProviderError(IdentityProviderError),
    SessionError(GatewaySessionError),
}

// Only SafeDisplay is allowed for AuthorisationError
impl SafeDisplay for AuthorisationError {
    fn to_safe_string(&self) -> String {
        match self {
            AuthorisationError::Internal(_) => "Failed authentication".to_string(),
            AuthorisationError::InvalidNonce => "Failed authentication".to_string(),
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
                format!(
                    "Failed to fetch claims. Error details: {}",
                    err.to_safe_string()
                )
            }
            AuthorisationError::InvalidToken => "Invalid token".to_string(),
            AuthorisationError::IdentityProviderError(err) => {
                format!("Identity provider error: {}", err.to_safe_string())
            }
            AuthorisationError::AccessTokenNotFound => {
                "Unable to continue with authorisation".to_string()
            }
            AuthorisationError::IdTokenNotFound => {
                "Unable to continue with authentication.".to_string()
            }
            AuthorisationError::ConflictingState => "Suspicious login attempt".to_string(),
            AuthorisationError::FailedCodeExchange(err) => {
                format!(
                    "Failed to exchange code for tokens. Error details: {}",
                    err.to_safe_string()
                )
            }
            AuthorisationError::NonceNotFound => {
                "Suspicious authorisation attempt. Failed checks.".to_string()
            }
            AuthorisationError::SessionError(err) => format!(
                "An error occurred while updating the session. Error details: {}",
                err.to_safe_string()
            ),
        }
    }
}

pub struct DefaultAuthCallBack;

#[async_trait]
impl AuthCallBackBindingHandler for DefaultAuthCallBack {
    async fn handle_auth_call_back(
        &self,
        query_params: &HashMap<String, String>,
        security_scheme_with_metadata: &SecuritySchemeWithProviderMetadata,
        session_store: &GatewaySessionStore,
        identity_provider: &Arc<dyn IdentityProvider + Send + Sync>,
    ) -> Result<AuthorisationSuccess, AuthorisationError> {
        let code = query_params
            .get("code")
            .map(|c| AuthorizationCode::new(c.to_string()));
        let state = query_params.get("state").cloned();

        let authorisation_code = code.ok_or(AuthorisationError::CodeNotFound)?;
        let state = state.ok_or(AuthorisationError::StateNotFound)?;

        let target_path = session_store
            .get(
                &SessionId(state.clone()),
                &DataKey("redirect_url".to_string()),
            )
            .await
            .map_err(AuthorisationError::SessionError)?
            .as_string()
            .ok_or(AuthorisationError::Internal(
                "Invalid redirect url (target url of the protected resource)".to_string(),
            ))?;

        let open_id_client = identity_provider
            .get_client(&security_scheme_with_metadata.security_scheme)
            .await
            .map_err(AuthorisationError::IdentityProviderError)?;

        let token_response = identity_provider
            .exchange_code_for_tokens(&open_id_client, &authorisation_code)
            .await
            .map_err(AuthorisationError::FailedCodeExchange)?;

        let access_token = token_response.access_token().secret().clone();
        let id_token = token_response
            .extra_fields()
            .id_token()
            .map(|x| x.to_string());

        // access token in session store
        let _ = session_store
            .insert(
                SessionId(state.clone()),
                DataKey::access_token(),
                DataValue(serde_json::Value::String(access_token.clone())),
            )
            .await
            .map_err(AuthorisationError::SessionError)?;

        if let Some(id_token) = &id_token {
            // id token in session store
            let _ = session_store
                .insert(
                    SessionId(state.clone()),
                    DataKey::id_token(),
                    DataValue(serde_json::Value::String(id_token.to_string())),
                )
                .await
                .map_err(AuthorisationError::SessionError)?;
        }

        Ok(AuthorisationSuccess {
            token_response,
            target_path,
            id_token,
            access_token,
            session: state,
        })
    }
}
