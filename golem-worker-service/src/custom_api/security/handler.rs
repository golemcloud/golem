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

use super::model::PendingOidcLogin;
use super::session_store::SessionStore;
use super::{IdentityProvider, OIDC_SESSION_EXPIRY};
use crate::custom_api::error::RequestHandlerError;
use crate::custom_api::model::OidcSession;
use crate::custom_api::route_resolver::ResolvedRouteEntry;
use crate::custom_api::security::model::SessionId;
use crate::custom_api::{ResponseBody, RichRequest, RouteExecutionResult};
use anyhow::anyhow;
use chrono::Utc;
use cookie::Cookie;
use golem_service_base::custom_api::SecuritySchemeDetails;
use http::StatusCode;
use openidconnect::{AuthorizationCode, CsrfToken, Nonce};
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

        if pending_login.scheme_id != scheme.id {
            return Err(RequestHandlerError::OidcSchemeMismatch);
        }

        let (id_token_scopes, id_token_claims) = self
            .identity_provider
            .exchange_code_for_scopes_and_claims(
                scheme,
                &AuthorizationCode::new(code.to_string()),
                &pending_login.nonce,
            )
            .await
            .map_err(|err| {
                tracing::warn!("OIDC token exchange failed: {err}");
                RequestHandlerError::OidcTokenExchangeFailed
            })?;

        let session_expires_at = Utc::now()
            .checked_add_signed(OIDC_SESSION_EXPIRY)
            .ok_or_else(|| anyhow!("Failed to compute expiry"))?;

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
            scopes: HashSet::from_iter(id_token_scopes),
            expires_at: session_expires_at,
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
            .max_age(cookie::time::Duration::seconds(
                OIDC_SESSION_EXPIRY.num_seconds(),
            ))
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
            let execution_result = self
                .start_oidc_flow_for_route(request, security_scheme)
                .await?;
            return Ok(Some(execution_result));
        };

        let session_opt = self
            .session_store
            .get_authenticated_session(&session_id)
            .await?;

        let Some(session) = session_opt else {
            // session information missing, restart flow
            let auth_url = self
                .start_oidc_flow_for_route(request, security_scheme)
                .await?;
            return Ok(Some(auth_url));
        };

        request.set_authenticated_session(session);

        Ok(None)
    }

    async fn start_oidc_flow_for_route(
        &self,
        request: &RichRequest,
        security_scheme: &SecuritySchemeDetails,
    ) -> Result<RouteExecutionResult, RequestHandlerError> {
        let state = CsrfToken::new_random();
        let nonce = Nonce::new_random();

        let pending_login = PendingOidcLogin {
            scheme_id: security_scheme.id,
            nonce: nonce.clone(),
            original_uri: request.underlying.uri().to_string(),
        };

        self.session_store
            .store_pending_oidc_login(state.secret(), pending_login)
            .await?;

        let auth_url = self
            .identity_provider
            .get_authorization_url(
                security_scheme,
                security_scheme.scopes.clone(),
                state,
                nonce,
            )
            .await?;

        let mut headers = std::collections::HashMap::new();
        headers.insert(http::header::LOCATION, auth_url.url.to_string());
        Ok(RouteExecutionResult {
            status: http::StatusCode::FOUND,
            headers,
            body: ResponseBody::NoBody,
        })
    }
}
