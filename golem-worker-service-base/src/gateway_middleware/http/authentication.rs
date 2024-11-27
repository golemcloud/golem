use crate::gateway_binding::HttpRequestDetails;
use crate::gateway_execution::gateway_session::GatewaySessionStore;
use crate::gateway_middleware::{MiddlewareInError, MiddlewareSuccess};
use crate::gateway_security::{IdentityProviderResolver, SecuritySchemeWithProviderMetadata};
use golem_common::SafeDisplay;
use openidconnect::Scope;
use std::sync::Arc;

#[derive(Debug, Clone, PartialEq)]
pub struct HttpAuthenticationMiddleware {
    pub security_scheme: SecuritySchemeWithProviderMetadata,
}

impl HttpAuthenticationMiddleware {
    pub fn get_scopes(&self) -> Vec<Scope> {
        self.security_scheme.security_scheme.scopes()
    }

    pub async fn apply_http_auth(
        &self,
        input: &HttpRequestDetails,
        session_store: &GatewaySessionStore,
        identity_provider_resolver: &Arc<dyn IdentityProviderResolver + Send + Sync>,
    ) -> Result<MiddlewareSuccess, MiddlewareInError> {
        let identity_provider = identity_provider_resolver
            .resolve(&self.security_scheme.security_scheme.provider_type());

        let client = identity_provider
            .get_client(&self.security_scheme)
            .map_err(|err| MiddlewareInError::Unauthorized(err.to_safe_string()))?;

        let identity_token_verifier = identity_provider.get_id_token_verifier(&client);

        let open_id_client = identity_provider
            .get_client(&self.security_scheme)
            .map_err(|err| MiddlewareInError::Unauthorized(err.to_safe_string()))?;

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
                &identity_provider,
                &open_id_client,
                self,
            )
            .await
        } else {
            internal::redirect(
                session_store,
                input,
                &identity_provider,
                &open_id_client,
                self,
            )
            .await
        }
    }
}

mod internal {
    use crate::gateway_binding::HttpRequestDetails;
    use crate::gateway_execution::gateway_session::{
        DataKey, DataValue, GatewaySessionStore, SessionId,
    };
    use crate::gateway_middleware::middleware_in::MiddlewareSuccess;
    use crate::gateway_middleware::{HttpAuthenticationMiddleware, MiddlewareInError};
    use crate::gateway_security::{IdentityProvider, OpenIdClient};
    use http::StatusCode;
    use openidconnect::core::{CoreIdToken, CoreIdTokenClaims, CoreIdTokenVerifier};
    use openidconnect::{ClaimsVerificationError, Nonce};
    use std::str::FromStr;
    use std::sync::Arc;

    pub(crate) async fn get_session_details_or_redirect<'a>(
        state_from_request: &str,
        identity_token_verifier: CoreIdTokenVerifier<'a>,
        id_token: &str,
        session_store: &GatewaySessionStore,
        input: &HttpRequestDetails,
        identity_provider: &Arc<dyn IdentityProvider + Send + Sync>,
        open_id_client: &OpenIdClient,
        http_authentication_details: &HttpAuthenticationMiddleware,
    ) -> Result<MiddlewareSuccess, MiddlewareInError> {
        let session_id = SessionId(state_from_request.to_string());

        let session_data = session_store
            .0
            .get(&session_id)
            .await
            .map_err(MiddlewareInError::InternalServerError)?;

        if let Some(session_data) = session_data {
            let nonce = session_data.value.get(&DataKey::nonce());

            let id_token = CoreIdToken::from_str(id_token)
                .map_err(|err| MiddlewareInError::Unauthorized(err.to_string()))?;

            get_claims(
                nonce,
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
        } else {
            redirect(
                session_store,
                input,
                identity_provider,
                open_id_client,
                http_authentication_details,
            )
            .await
        }
    }
    pub(crate) async fn get_claims<'a>(
        nonce: Option<&DataValue>,
        id_token: CoreIdToken,
        identity_token_verifier: CoreIdTokenVerifier<'a>,
        session_id: &SessionId,
        session_store: &GatewaySessionStore,
        input: &HttpRequestDetails,
        identity_provider: &Arc<dyn IdentityProvider + Send + Sync>,
        open_id_client: &OpenIdClient,
        http_authentication_details: &HttpAuthenticationMiddleware,
    ) -> Result<MiddlewareSuccess, MiddlewareInError> {
        if let Some(nonce) = nonce.and_then(|data_value| data_value.as_string()) {
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
                Err(_) => Err(MiddlewareInError::Unauthorized("Invalid token".to_string())),
            }
        } else {
            Err(MiddlewareInError::Unauthorized("Invalid nonce".to_string()))
        }
    }

    pub(crate) async fn redirect(
        session_store: &GatewaySessionStore,
        input: &HttpRequestDetails,
        identity_provider: &Arc<dyn IdentityProvider + Send + Sync>,
        client: &OpenIdClient,
        http_authorizer: &HttpAuthenticationMiddleware,
    ) -> Result<MiddlewareSuccess, MiddlewareInError> {
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
            .0
            .insert(session_id.clone(), nonce_data_key, nonce_data_value)
            .await
            .map_err(MiddlewareInError::InternalServerError)?;
        session_store
            .0
            .insert(session_id, redirect_url_data_key, redirect_url_data_value)
            .await
            .map_err(MiddlewareInError::InternalServerError)?;

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
    ) -> Result<(), MiddlewareInError> {
        let claims_data_key = DataKey::claims();
        let json = serde_json::to_value(claims)
            .map_err(|err| err.to_string())
            .unwrap();

        let claims_data_value = DataValue(json);

        session_store
            .0
            .insert(session_id.clone(), claims_data_key, claims_data_value)
            .await
            .map_err(MiddlewareInError::InternalServerError)
    }
}
