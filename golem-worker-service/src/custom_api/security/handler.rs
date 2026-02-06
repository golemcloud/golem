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

use super::IdentityProvider;
use super::model::AuthorizationUrl;
use super::session_store::SessionStore;
use crate::custom_api::error::RequestHandlerError;
use crate::custom_api::model::{OidcSession, RichRequest};
use crate::custom_api::route_resolver::ResolvedRouteEntry;
use crate::custom_api::security::model::SessionId;
use crate::custom_api::{ResponseBody, RouteExecutionResult};
use cookie::Cookie;
use golem_service_base::custom_api::SecuritySchemeDetails;
use http::StatusCode;
use openidconnect::{AuthorizationCode, OAuth2TokenResponse};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tracing::debug;
use uuid::Uuid;

const GOLEM_SESSION_ID_COOKIE_NAME: &str = "golem_session_id";

pub struct OidcHandler {
    session_store: Arc<dyn SessionStore>,
    identity_provider: Arc<dyn IdentityProvider>,
}

impl OidcHandler {
    pub fn new(
        session_store: Arc<dyn SessionStore>,
        identity_provider: Arc<dyn IdentityProvider>,
    ) -> Self {
        Self {
            session_store,
            identity_provider,
        }
    }

    pub async fn handle_oidc_callback_behaviour(
        &self,
        request: &mut RichRequest,
        scheme: &Arc<SecuritySchemeDetails>,
    ) -> Result<RouteExecutionResult, RequestHandlerError> {
        let code = request.get_single_param("code")?;
        let state = request.get_single_param("state")?;

        let pending_login = self
            .session_store
            .take_pending_oidc_login(state)
            .await?
            .ok_or(RequestHandlerError::UnknownOidcState)?;

        let client = self.identity_provider.get_client(scheme).await?;

        let nonce = pending_login.nonce.clone();

        let token_response = self
            .identity_provider
            .exchange_code_for_tokens(&client, &AuthorizationCode::new(code.to_string()))
            .await
            .map_err(|err| {
                tracing::warn!("OIDC token exchange failed: {err}");
                RequestHandlerError::OidcTokenExchangeFailed
            })?;

        let id_token_verifier = self.identity_provider.get_id_token_verifier(&client);
        let id_token_claims =
            self.identity_provider
                .get_claims(&id_token_verifier, &token_response, &nonce)?;

        let session = OidcSession {
            subject: id_token_claims.subject().to_string(),
            issuer: id_token_claims.issuer().to_string(),

            email: id_token_claims.email().map(|v| v.to_string()),
            name: id_token_claims
                .name()
                .and_then(|v| v.get(None))
                .map(|v| v.to_string()),
            email_verified: id_token_claims.email_verified(),
            given_name: id_token_claims
                .given_name()
                .and_then(|v| v.get(None))
                .map(|v| v.to_string()),
            family_name: id_token_claims
                .family_name()
                .and_then(|v| v.get(None))
                .map(|v| v.to_string()),
            picture: id_token_claims
                .picture()
                .and_then(|v| v.get(None))
                .map(|v| v.to_string()),
            preferred_username: id_token_claims.preferred_username().map(|v| v.to_string()),

            claims: id_token_claims.clone(),
            scopes: HashSet::from_iter(token_response.scopes().cloned().unwrap_or_default()),
            expires_at: id_token_claims.expiration(),
        };

        let session_id = SessionId(Uuid::now_v7());

        self.session_store
            .store_authenticated_session(&session_id, session)
            .await?;

        let cookie = Cookie::build((GOLEM_SESSION_ID_COOKIE_NAME, session_id.0.to_string()))
            .path("/")
            .http_only(true)
            .secure(true)
            .same_site(cookie::SameSite::Lax)
            .build();

        let mut headers = HashMap::new();
        headers.insert(http::header::SET_COOKIE, cookie.to_string());
        headers.insert(http::header::LOCATION, pending_login.original_uri.clone());

        Ok(RouteExecutionResult {
            status: StatusCode::FOUND,
            headers,
            body: ResponseBody::NoBody,
        })
    }

    pub async fn apply_oidc_incoming_middleware(
        &self,
        request: &mut RichRequest,
        resolved_route: &ResolvedRouteEntry,
    ) -> Result<Option<RouteExecutionResult>, RequestHandlerError> {
        debug!("Begin executing OidcSecurityMiddleware");

        let Some(security_scheme) = resolved_route.route.security_scheme.as_ref() else {
            return Ok(None);
        };

        let session_id = if let Some(s) = request.cookie(GOLEM_SESSION_ID_COOKIE_NAME)
            && let Ok(parsed) = Uuid::parse_str(s)
        {
            SessionId(parsed)
        } else {
            // missing or invalid session_id -> restart flow
            let execution_result =
                start_oidc_flow_for_route(security_scheme, self.identity_provider.clone()).await?;
            return Ok(Some(execution_result));
        };

        let session_opt = self
            .session_store
            .get_authenticated_session(&session_id)
            .await?;

        let Some(session) = session_opt else {
            // session information missing, restart flow
            let auth_url =
                start_oidc_flow_for_route(security_scheme, self.identity_provider.clone()).await?;
            return Ok(Some(auth_url));
        };

        request.set_authenticated_session(session);

        Ok(None)
    }
}

async fn start_oidc_flow_for_route(
    security_scheme: &SecuritySchemeDetails,
    identity_provider: Arc<dyn IdentityProvider>,
) -> Result<RouteExecutionResult, RequestHandlerError> {
    let client = identity_provider.get_client(security_scheme).await?;
    let auth_url = identity_provider.get_authorization_url(
        &client,
        security_scheme.scopes.clone(),
        None,
        None,
    );

    Ok(start_oidc_flow(auth_url))
}

fn start_oidc_flow(auth_url: AuthorizationUrl) -> RouteExecutionResult {
    let mut headers = std::collections::HashMap::new();
    headers.insert(http::header::LOCATION, auth_url.url.to_string());
    RouteExecutionResult {
        status: http::StatusCode::FOUND,
        headers,
        body: ResponseBody::NoBody,
    }
}
