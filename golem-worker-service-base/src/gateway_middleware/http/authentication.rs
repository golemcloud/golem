use crate::gateway_binding::HttpRequestDetails;
use crate::gateway_execution::gateway_session::{DataKey, GatewaySessionStore, SessionId};
use crate::gateway_middleware::{MiddlewareInError, MiddlewareSuccess};
use crate::gateway_security::SecuritySchemeWithProviderMetadata;
use openidconnect::core::{CoreIdToken, CoreIdTokenClaims};
use openidconnect::{ClaimsVerificationError, Nonce, Scope};
use std::str::FromStr;

#[derive(Debug, Clone, PartialEq)]
pub struct HttpAuthorizer {
    pub security_scheme: SecuritySchemeWithProviderMetadata,
}

impl HttpAuthorizer {
    pub fn get_scopes(&self) -> Vec<Scope> {
        self.security_scheme.security_scheme.scopes()
    }

    pub async fn apply_http_auth(
        &self,
        input: &HttpRequestDetails,
        session_store: &GatewaySessionStore,
    ) -> Result<MiddlewareSuccess<poem::Response>, MiddlewareInError> {
        let identity_provider = &self.security_scheme.identity_provider();

        let open_id_client = identity_provider
            .get_client(&self.security_scheme)
            .map_err(|err| MiddlewareInError::Unauthorized(err.to_string()))?;

        let id_token = input.get_id_token_from_cookie();

        let state = input.get_auth_state_from_cookie();

        if let (Some(id_token), Some(state)) = (id_token, state) {
            let id_token = CoreIdToken::from_str(&id_token)
                .map_err(|err| MiddlewareInError::Unauthorized(err.to_string()))?;

            let nonce = session_store
                .0
                .get(SessionId(state.clone()), DataKey::nonce())
                .await
                .map_err(MiddlewareInError::InternalServerError)?;

            if let Some(nonce) = nonce.and_then(|x| x.as_string()) {
                let result: Result<&CoreIdTokenClaims, ClaimsVerificationError> =
                    id_token.claims(&open_id_client.id_token_verifier(), &Nonce::new(nonce));

                match result {
                    Ok(claims) => {
                        internal::store_claims_in_session_store(
                            &SessionId(state.clone()),
                            claims,
                            session_store,
                        )
                        .await?;
                        Ok(MiddlewareSuccess::PassThrough)
                    }
                    Err(ClaimsVerificationError::Expired(_)) => {
                        internal::redirect(
                            session_store,
                            input,
                            identity_provider,
                            &open_id_client,
                            self,
                        )
                        .await
                    }
                    Err(_) => Err(MiddlewareInError::Unauthorized("Invalid token".to_string())),
                }
            } else {
                Err(MiddlewareInError::Unauthorized("Invalid nonce".to_string()))
            }
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
    use crate::gateway_execution::gateway_session::{
        DataKey, DataValue, GatewaySessionStore, SessionId,
    };
    use crate::gateway_middleware::middleware_in::MiddlewareSuccess;
    use crate::gateway_middleware::{HttpAuthorizer, MiddlewareInError};
    use crate::gateway_security::{IdentityProvider, OpenIdClient};
    use http::StatusCode;
    use openidconnect::core::CoreIdTokenClaims;
    use std::sync::Arc;

    pub(crate) async fn redirect(
        session_store: &GatewaySessionStore,
        input: &HttpRequestDetails,
        identity_provider: &Arc<dyn IdentityProvider + Send + Sync>,
        client: &OpenIdClient,
        http_authorizer: &HttpAuthorizer,
    ) -> Result<MiddlewareSuccess<poem::Response>, MiddlewareInError> {
        let redirect_uri = input.get_uri();

        let authorization =
            identity_provider.get_authorization_url(client, http_authorizer.get_scopes());

        let state = authorization.csrf_state.secret();

        // TODO Handle session-id
        let session_id = SessionId(state.clone());
        let nonce_data_key = DataKey::nonce();
        let nonce_data_value = DataValue(serde_json::Value::String(
            authorization.nonce.secret().clone(),
        ));

        let redirect_url_data_key = DataKey::redirect_uri();

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
        let claims_data_key = DataKey("claims".to_string());
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
