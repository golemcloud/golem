use crate::gateway_binding::HttpRequestDetails;
use crate::gateway_execution::auth_call_back_binding_handler::AuthorisationError;
use crate::gateway_execution::gateway_session::GatewaySessionStore;
use crate::gateway_middleware::{MiddlewareError, MiddlewareSuccess};
use crate::gateway_security::{IdentityProvider, SecuritySchemeWithProviderMetadata};
use openidconnect::Scope;
use std::sync::Arc;

#[derive(Debug, Clone, PartialEq)]
pub struct HttpAuthenticationMiddleware {
    pub security_scheme_with_metadata: SecuritySchemeWithProviderMetadata,
}

impl HttpAuthenticationMiddleware {
    pub fn get_scopes(&self) -> Vec<Scope> {
        self.security_scheme_with_metadata.security_scheme.scopes()
    }

    pub async fn apply_http_auth(
        &self,
        input: &HttpRequestDetails,
        session_store: &GatewaySessionStore,
        identity_provider: &Arc<dyn IdentityProvider + Send + Sync>,
    ) -> Result<MiddlewareSuccess, MiddlewareError> {
        let open_id_client = identity_provider
            .get_client(&self.security_scheme_with_metadata.security_scheme)
            .await
            .map_err(|err| {
                MiddlewareError::Unauthorized(AuthorisationError::IdentityProviderError(err))
            })?;

        let identity_token_verifier = open_id_client.id_token_verifier();

        let cookie_values = input.get_cookie_values();

        let id_token = cookie_values.get("id_token");
        let state = cookie_values.get("session_id");

        if let (Some(id_token), Some(state)) = (id_token, state) {
            internal::get_session_details_or_redirect(
                state,
                identity_token_verifier,
                id_token,
                session_store,
                input,
                identity_provider,
                &open_id_client,
                self,
            )
            .await
        } else {
            internal::redirect(
                session_store,
                input,
                identity_provider,
                &open_id_client,
                self,
            )
            .await
        }
    }
}

mod internal {
    use crate::gateway_binding::HttpRequestDetails;
    use crate::gateway_execution::auth_call_back_binding_handler::AuthorisationError;
    use crate::gateway_execution::gateway_session::{
        DataKey, DataValue, GatewaySessionError, GatewaySessionStore, SessionId,
    };
    use crate::gateway_middleware::http::middleware_error::MiddlewareSuccess;
    use crate::gateway_middleware::{HttpAuthenticationMiddleware, MiddlewareError};
    use crate::gateway_security::{IdentityProvider, OpenIdClient};
    use golem_common::SafeDisplay;
    use http::StatusCode;
    use openidconnect::core::{CoreIdToken, CoreIdTokenClaims, CoreIdTokenVerifier};
    use openidconnect::{ClaimsVerificationError, Nonce};
    use std::str::FromStr;
    use std::sync::Arc;
    use tracing::{debug, error};

    pub(crate) async fn get_session_details_or_redirect(
        state_from_request: &str,
        identity_token_verifier: CoreIdTokenVerifier<'_>,
        id_token: &str,
        session_store: &GatewaySessionStore,
        input: &HttpRequestDetails,
        identity_provider: &Arc<dyn IdentityProvider + Send + Sync>,
        open_id_client: &OpenIdClient,
        http_authentication_details: &HttpAuthenticationMiddleware,
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
                    http_authentication_details,
                )
                .await
            }
            Err(GatewaySessionError::MissingValue { .. }) => {
                redirect(
                    session_store,
                    input,
                    identity_provider,
                    open_id_client,
                    http_authentication_details,
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
    pub(crate) async fn get_claims(
        nonce: &DataValue,
        id_token: CoreIdToken,
        identity_token_verifier: CoreIdTokenVerifier<'_>,
        session_id: &SessionId,
        session_store: &GatewaySessionStore,
        input: &HttpRequestDetails,
        identity_provider: &Arc<dyn IdentityProvider + Send + Sync>,
        open_id_client: &OpenIdClient,
        http_authentication_details: &HttpAuthenticationMiddleware,
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
                        http_authentication_details,
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

    pub(crate) async fn redirect(
        session_store: &GatewaySessionStore,
        input: &HttpRequestDetails,
        identity_provider: &Arc<dyn IdentityProvider + Send + Sync>,
        client: &OpenIdClient,
        http_authorizer: &HttpAuthenticationMiddleware,
    ) -> Result<MiddlewareSuccess, MiddlewareError> {
        let redirect_uri = input.get_api_input_path();

        let authorization = identity_provider.get_authorization_url(
            client,
            http_authorizer.get_scopes(),
            None,
            None,
        );

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

    pub(crate) async fn store_claims_in_session_store(
        session_id: &SessionId,
        claims: &CoreIdTokenClaims,
        session_store: &GatewaySessionStore,
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
}
