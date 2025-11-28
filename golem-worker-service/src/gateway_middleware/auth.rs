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

use super::{MiddlewareError, MiddlewareSuccess};
use crate::gateway_execution::auth_call_back_binding_handler::AuthorisationError;
use crate::gateway_execution::gateway_session_store::{
    DataKey, DataValue, GatewaySessionError, GatewaySessionStore, SessionId,
};
use crate::gateway_execution::request::RichRequest;
use crate::gateway_security::{IdentityProvider, OpenIdClient};
use golem_common::SafeDisplay;
use golem_service_base::custom_api::SecuritySchemeDetails;
use http::StatusCode;
use openidconnect::core::{CoreIdToken, CoreIdTokenClaims, CoreIdTokenVerifier};
use openidconnect::{ClaimsVerificationError, Nonce};
use std::str::FromStr;
use std::sync::Arc;
use tracing::{debug, error};

pub async fn apply_http_auth(
    security_scheme: &SecuritySchemeDetails,
    input: &RichRequest,
    session_store: &Arc<dyn GatewaySessionStore>,
    identity_provider: &Arc<dyn IdentityProvider>,
) -> Result<MiddlewareSuccess, MiddlewareError> {
    let open_id_client = identity_provider
        .get_client(security_scheme)
        .await
        .map_err(|err| {
            MiddlewareError::Unauthorized(AuthorisationError::IdentityProviderError(err))
        })?;

    let identity_token_verifier = open_id_client.id_token_verifier();

    let cookie_values = input.get_cookie_values();

    let id_token = cookie_values.get("id_token");
    let state = cookie_values.get("session_id");

    if let (Some(id_token), Some(state)) = (id_token, state) {
        get_session_details_or_redirect(
            state,
            identity_token_verifier,
            id_token,
            session_store,
            input,
            identity_provider,
            &open_id_client,
            security_scheme,
        )
        .await
    } else {
        redirect(
            session_store,
            input,
            identity_provider,
            &open_id_client,
            security_scheme,
        )
        .await
    }
}

async fn get_session_details_or_redirect(
    state_from_request: &str,
    identity_token_verifier: CoreIdTokenVerifier<'_>,
    id_token: &str,
    session_store: &Arc<dyn GatewaySessionStore>,
    input: &RichRequest,
    identity_provider: &Arc<dyn IdentityProvider>,
    open_id_client: &OpenIdClient,
    security_scheme: &SecuritySchemeDetails,
) -> Result<MiddlewareSuccess, MiddlewareError> {
    let session_id = SessionId(state_from_request.to_string());

    let nonce_from_session = session_store.get(&session_id, &DataKey::nonce()).await;

    match nonce_from_session {
        Ok(nonce) => {
            let id_token = CoreIdToken::from_str(id_token).map_err(|err| {
                debug!(
                    "Failed to parse id token for session {}: {}",
                    err, session_id.0
                );
                MiddlewareError::Unauthorized(AuthorisationError::InvalidToken)
            })?;

            get_claims(
                &nonce,
                id_token,
                identity_token_verifier,
                &session_id,
                session_store,
                input,
                identity_provider,
                open_id_client,
                security_scheme,
            )
            .await
        }
        Err(GatewaySessionError::MissingValue { .. }) => {
            redirect(
                session_store,
                input,
                identity_provider,
                open_id_client,
                security_scheme,
            )
            .await
        }
        Err(err) => {
            debug!(
                "Failed to get nonce from session store: {:?} for session {}",
                err, session_id.0
            );
            Err(MiddlewareError::Unauthorized(
                AuthorisationError::SessionError(err),
            ))
        }
    }
}

async fn get_claims(
    nonce: &DataValue,
    id_token: CoreIdToken,
    identity_token_verifier: CoreIdTokenVerifier<'_>,
    session_id: &SessionId,
    session_store: &Arc<dyn GatewaySessionStore>,
    input: &RichRequest,
    identity_provider: &Arc<dyn IdentityProvider>,
    open_id_client: &OpenIdClient,
    security_scheme: &SecuritySchemeDetails,
) -> Result<MiddlewareSuccess, MiddlewareError> {
    if let Some(nonce) = nonce.as_string() {
        let token_claims_result: Result<&CoreIdTokenClaims, ClaimsVerificationError> =
            id_token.claims(&identity_token_verifier, &Nonce::new(nonce));

        match token_claims_result {
            Ok(claims) => {
                store_claims_in_session_store(session_id, claims, session_store).await?;

                Ok(MiddlewareSuccess::PassThrough {
                    session_id: Some(session_id.clone()),
                })
            }
            Err(ClaimsVerificationError::Expired(_)) => {
                redirect(
                    session_store,
                    input,
                    identity_provider,
                    open_id_client,
                    security_scheme,
                )
                .await
            }
            Err(claims_verification_error) => {
                error!("Invalid token for session {}", claims_verification_error);

                Err(MiddlewareError::Unauthorized(
                    AuthorisationError::InvalidToken,
                ))
            }
        }
    } else {
        Err(MiddlewareError::Unauthorized(
            AuthorisationError::InvalidNonce,
        ))
    }
}

async fn redirect(
    session_store: &Arc<dyn GatewaySessionStore>,
    input: &RichRequest,
    identity_provider: &Arc<dyn IdentityProvider>,
    client: &OpenIdClient,
    security_scheme: &SecuritySchemeDetails,
) -> Result<MiddlewareSuccess, MiddlewareError> {
    let redirect_uri = input
        .underlying
        .uri()
        .path_and_query()
        .ok_or(MiddlewareError::InternalError(
            "Failed to get redirect uri".to_string(),
        ))?
        .to_string();

    let authorization =
        identity_provider.get_authorization_url(client, security_scheme.scopes.clone(), None, None);

    let state = authorization.csrf_state.secret();

    let session_id = SessionId(state.clone());
    let nonce_data_key = DataKey::nonce();
    let nonce_data_value = DataValue(serde_json::Value::String(
        authorization.nonce.secret().clone(),
    ));

    let redirect_url_data_key = DataKey::redirect_url();

    let redirect_url_data_value = DataValue(serde_json::Value::String(redirect_uri));

    session_store
        .insert(session_id.clone(), nonce_data_key, nonce_data_value)
        .await
        .map_err(|err| MiddlewareError::Unauthorized(AuthorisationError::SessionError(err)))?;
    session_store
        .insert(session_id, redirect_url_data_key, redirect_url_data_value)
        .await
        .map_err(|err| MiddlewareError::Unauthorized(AuthorisationError::SessionError(err)))?;

    let response = poem::Response::builder();
    let result = response
        .header("Location", authorization.url.to_string())
        .status(StatusCode::FOUND)
        .body(());

    Ok(MiddlewareSuccess::Redirect(result))
}

async fn store_claims_in_session_store(
    session_id: &SessionId,
    claims: &CoreIdTokenClaims,
    session_store: &Arc<dyn GatewaySessionStore>,
) -> Result<(), MiddlewareError> {
    let claims_data_key = DataKey::claims();
    let json = serde_json::to_value(claims)
        .map_err(|err| MiddlewareError::InternalError(err.to_string()))?;

    let claims_data_value = DataValue(json);

    session_store
        .insert(session_id.clone(), claims_data_key, claims_data_value)
        .await
        .map_err(|err| MiddlewareError::InternalError(err.to_safe_string()))
}
