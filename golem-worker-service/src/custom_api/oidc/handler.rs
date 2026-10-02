// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
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

use super::model::{
    AUTHORIZATION_CODE_LIFETIME, AuthorizationCode as GolemAuthorizationCode,
    BEARER_TOKEN_LIFETIME, BearerToken, PENDING_PKCE_LOGIN_LIFETIME, PendingOidcLogin,
    PendingPkceLogin, PkceAuthorizationCode, PkceBearerCredential, PkceBinding,
};
use super::pkce::{
    AuthorizationRequest, MAX_ENCODED_REQUEST_LENGTH, ProtocolError, ProtocolErrorCode,
    ProtocolErrorResponse, TokenRequest, TokenResponse, bearer_authentication_failure,
};
use super::session_store::SessionStore;
use super::{IdentityProvider, OIDC_SESSION_EXPIRY};
use crate::custom_api::error::RequestHandlerError;
use crate::custom_api::model::OidcSession;
use crate::custom_api::oidc::model::SessionId;
use crate::custom_api::route_resolver::ResolvedRouteEntry;
use crate::custom_api::{
    ResponseBody, RichRequest, RichRouteSecurity, RichSecuritySchemeRouteSecurity,
    RouteExecutionResult,
};
use anyhow::anyhow;
use chrono::Utc;
use cookie::Cookie;
use golem_service_base::custom_api::SecuritySchemeDetails;
use http::{HeaderMap, HeaderValue, StatusCode};
use openidconnect::{AuthorizationCode, CsrfToken, Nonce};
use std::collections::HashSet;
use std::str::FromStr;
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
        resolved_route: &ResolvedRouteEntry,
        scheme: &Arc<SecuritySchemeDetails>,
    ) -> Result<RouteExecutionResult, RequestHandlerError> {
        if matches!(
            scheme.login,
            golem_common::model::security_scheme::SecuritySchemeLogin::AuthorizationCodePkce(_)
        ) {
            self.handle_pkce_callback(request, resolved_route, scheme)
                .await
        } else {
            self.handle_cookie_callback(request, scheme).await
        }
    }

    async fn handle_cookie_callback(
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

        let session = oidc_session(id_token_scopes, id_token_claims)?;

        let session_id = SessionId(Uuid::now_v7());

        self.session_store
            .store_authenticated_session(&session_id, session)
            .await?;

        let is_https = scheme.redirect_url.url().scheme() == "https";

        let cookie = Cookie::build((GOLEM_SESSION_ID_COOKIE_NAME, session_id.0.to_string()))
            .path("/")
            .http_only(true)
            .secure(is_https)
            .same_site(cookie::SameSite::Lax)
            .max_age(cookie::time::Duration::seconds(
                OIDC_SESSION_EXPIRY.num_seconds(),
            ))
            .build();

        let mut headers = HeaderMap::new();
        headers.insert(
            http::header::SET_COOKIE,
            HeaderValue::from_str(&cookie.to_string()).map_err(anyhow::Error::from)?,
        );
        headers.insert(
            http::header::LOCATION,
            HeaderValue::from_str(&pending_login.original_uri).map_err(anyhow::Error::from)?,
        );

        Ok(RouteExecutionResult {
            status: StatusCode::FOUND,
            headers,
            body: ResponseBody::NoBody,
        })
    }

    pub async fn handle_pkce_authorization(
        &self,
        request: &RichRequest,
        resolved_route: &ResolvedRouteEntry,
        scheme: &Arc<SecuritySchemeDetails>,
    ) -> Result<RouteExecutionResult, RequestHandlerError> {
        let golem_common::model::security_scheme::SecuritySchemeLogin::AuthorizationCodePkce(
            config,
        ) = &scheme.login
        else {
            return Err(RequestHandlerError::invariant_violated(
                "PKCE authorization route has a non-PKCE security scheme",
            ));
        };
        let authorization_request = match AuthorizationRequest::parse(
            request.underlying.uri().query().unwrap_or_default(),
            &config.redirect_uris,
        ) {
            Ok(request) => request,
            Err(error) => return protocol_error(StatusCode::BAD_REQUEST, error.response()),
        };
        let upstream_state = CsrfToken::new_random();
        let upstream_nonce = Nonce::new_random();
        let expires_at = Utc::now()
            .checked_add_signed(PENDING_PKCE_LOGIN_LIFETIME)
            .ok_or_else(|| anyhow!("Failed to compute PKCE login expiry"))?;
        let scopes = provider_scopes(scheme);
        let authorization_url = self
            .identity_provider
            .get_authorization_url(scheme, scopes, upstream_state, upstream_nonce)
            .await?;
        self.session_store
            .store_pending_pkce_login(
                authorization_url.csrf_state.secret(),
                PendingPkceLogin {
                    binding: pkce_binding(resolved_route, scheme),
                    redirect_uri: authorization_request.redirect_uri,
                    frontend_state: authorization_request.state,
                    code_challenge: authorization_request.code_challenge,
                    upstream_nonce: authorization_url.nonce,
                    expires_at,
                },
            )
            .await?;
        redirect(authorization_url.url.as_str())
    }

    async fn handle_pkce_callback(
        &self,
        request: &RichRequest,
        resolved_route: &ResolvedRouteEntry,
        scheme: &Arc<SecuritySchemeDetails>,
    ) -> Result<RouteExecutionResult, RequestHandlerError> {
        let state = match single_callback_parameter(request, "state") {
            Ok(value) => value,
            Err(response) => return Ok(response),
        };
        let Some(pending_login) = self.session_store.take_pending_pkce_login(state).await? else {
            return protocol_error(
                StatusCode::BAD_REQUEST,
                invalid_request("authorization transaction is invalid or expired"),
            );
        };
        if pending_login.binding != pkce_binding(resolved_route, scheme) {
            return protocol_error(
                StatusCode::BAD_REQUEST,
                invalid_request("authorization transaction is invalid or expired"),
            );
        }

        if let Some(errors) = request.query_params().get("error") {
            if !matches!(errors.as_slice(), [error] if !error.is_empty())
                || request.query_params().contains_key("code")
            {
                return protocol_error(
                    StatusCode::BAD_REQUEST,
                    invalid_request("authorization callback is invalid"),
                );
            }
            let authorization_request = AuthorizationRequest {
                redirect_uri: pending_login.redirect_uri,
                state: pending_login.frontend_state,
                code_challenge: pending_login.code_challenge,
            };
            return redirect(
                super::pkce::upstream_denial_redirect(&authorization_request).as_str(),
            );
        }

        let code = match single_callback_parameter(request, "code") {
            Ok(value) => value,
            Err(response) => return Ok(response),
        };
        let (scopes, claims) = self
            .identity_provider
            .exchange_code_for_scopes_and_claims(
                scheme,
                &AuthorizationCode::new(code.to_string()),
                &pending_login.upstream_nonce,
            )
            .await
            .map_err(|error| {
                tracing::warn!("OIDC token exchange failed: {error}");
                RequestHandlerError::OidcTokenExchangeFailed
            })?;
        let principal = oidc_session(scopes, claims)?;
        let expires_at = Utc::now()
            .checked_add_signed(AUTHORIZATION_CODE_LIFETIME)
            .ok_or_else(|| anyhow!("Failed to compute authorization code expiry"))?;
        let authorization_code = GolemAuthorizationCode::generate();
        self.session_store
            .store_authorization_code(
                &authorization_code,
                PkceAuthorizationCode {
                    binding: pending_login.binding,
                    principal,
                    redirect_uri: pending_login.redirect_uri.clone(),
                    code_challenge: pending_login.code_challenge,
                    expires_at,
                },
            )
            .await?;

        let mut frontend_redirect = url::Url::parse(&pending_login.redirect_uri)
            .map_err(|error| anyhow!("Stored frontend redirect URI is invalid: {error}"))?;
        frontend_redirect
            .query_pairs_mut()
            .append_pair("code", authorization_code.secret())
            .append_pair("state", &pending_login.frontend_state);
        redirect(frontend_redirect.as_str())
    }

    pub async fn handle_pkce_token(
        &self,
        request: &mut RichRequest,
        resolved_route: &ResolvedRouteEntry,
        scheme: &Arc<SecuritySchemeDetails>,
    ) -> Result<RouteExecutionResult, RequestHandlerError> {
        if !matches!(
            scheme.login,
            golem_common::model::security_scheme::SecuritySchemeLogin::AuthorizationCodePkce(_)
        ) {
            return Err(RequestHandlerError::invariant_violated(
                "PKCE token route has a non-PKCE security scheme",
            ));
        }
        if !has_form_content_type(request) {
            return token_protocol_error(
                StatusCode::BAD_REQUEST,
                invalid_request("content type must be application/x-www-form-urlencoded"),
            );
        }
        let body = match request
            .underlying
            .take_body()
            .into_bytes_limit(MAX_ENCODED_REQUEST_LENGTH)
            .await
        {
            Ok(body) => body,
            Err(poem::error::ReadBodyError::PayloadTooLarge) => {
                return token_protocol_error(
                    StatusCode::BAD_REQUEST,
                    ProtocolError::RequestTooLarge.response(),
                );
            }
            Err(error) => return Err(anyhow::Error::from(error).into()),
        };
        let token_request = match TokenRequest::parse(&body) {
            Ok(request) => request,
            Err(error) => return token_protocol_error(StatusCode::BAD_REQUEST, error.response()),
        };

        let Some(authorization_code) = self
            .session_store
            .take_authorization_code(&token_request.code)
            .await?
        else {
            return invalid_grant();
        };
        let expected_binding = pkce_binding(resolved_route, scheme);
        if authorization_code.binding != expected_binding
            || authorization_code.redirect_uri != token_request.redirect_uri
            || !token_request.verifies_challenge(&authorization_code.code_challenge)
        {
            return invalid_grant();
        }

        let expires_at = Utc::now()
            .checked_add_signed(BEARER_TOKEN_LIFETIME)
            .ok_or_else(|| anyhow!("Failed to compute bearer token expiry"))?;
        let bearer_token = BearerToken::generate();
        self.session_store
            .store_bearer_credential(
                &bearer_token,
                PkceBearerCredential {
                    binding: authorization_code.binding,
                    principal: authorization_code.principal,
                    expires_at,
                },
            )
            .await?;

        let response = TokenResponse::bearer(
            bearer_token.secret().to_string(),
            BEARER_TOKEN_LIFETIME.num_seconds() as u64,
        );
        let mut headers = token_cache_headers();
        headers.insert(
            http::header::CONTENT_TYPE,
            HeaderValue::from_static("application/json"),
        );
        Ok(RouteExecutionResult {
            status: StatusCode::OK,
            headers,
            body: ResponseBody::PoemBody {
                body: poem::Body::from_json(&response)
                    .expect("token response serialization cannot fail"),
                content_type: None,
            },
        })
    }

    pub async fn apply_oidc_incoming_middleware(
        &self,
        request: &mut RichRequest,
        resolved_route: &ResolvedRouteEntry,
    ) -> Result<Option<RouteExecutionResult>, RequestHandlerError> {
        debug!("Begin executing OidcSecurityMiddleware");

        let RichRouteSecurity::SecurityScheme(RichSecuritySchemeRouteSecurity { security_scheme }) =
            &resolved_route.route.security
        else {
            return Ok(None);
        };

        if matches!(
            security_scheme.login,
            golem_common::model::security_scheme::SecuritySchemeLogin::AuthorizationCodePkce(_)
        ) {
            return self
                .apply_pkce_bearer_middleware(request, resolved_route, security_scheme)
                .await;
        }

        let session_id = if let Some(s) = request.cookie(GOLEM_SESSION_ID_COOKIE_NAME)
            && let Ok(parsed) = Uuid::parse_str(s)
        {
            SessionId(parsed)
        } else {
            // missing or invalid session_id -> restart flow
            let execution_result = self
                .start_oidc_flow_for_route(request, resolved_route, security_scheme)
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
                .start_oidc_flow_for_route(request, resolved_route, security_scheme)
                .await?;
            return Ok(Some(auth_url));
        };

        request.set_authenticated_session(session);

        Ok(None)
    }

    async fn apply_pkce_bearer_middleware(
        &self,
        request: &mut RichRequest,
        resolved_route: &ResolvedRouteEntry,
        scheme: &SecuritySchemeDetails,
    ) -> Result<Option<RouteExecutionResult>, RequestHandlerError> {
        let Some(token) = bearer_token(request) else {
            return Ok(Some(bearer_authentication_failure()));
        };
        let Some(credential) = self
            .session_store
            .get_bearer_credential(token.secret(), &pkce_binding(resolved_route, scheme))
            .await?
        else {
            return Ok(Some(bearer_authentication_failure()));
        };
        if credential.principal.is_expired() {
            return Ok(Some(bearer_authentication_failure()));
        }
        request.set_authenticated_session(credential.principal);
        Ok(None)
    }

    async fn start_oidc_flow_for_route(
        &self,
        request: &RichRequest,
        resolved_route: &ResolvedRouteEntry,
        security_scheme: &SecuritySchemeDetails,
    ) -> Result<RouteExecutionResult, RequestHandlerError> {
        let state = CsrfToken::new_random();
        let nonce = Nonce::new_random();

        let pending_login = PendingOidcLogin {
            scheme_id: security_scheme.id,
            nonce: nonce.clone(),
            original_uri: format!(
                "{}://{}{}",
                resolved_route.public_scheme,
                resolved_route.public_authority,
                request.underlying.uri().path_and_query().ok_or_else(|| {
                    RequestHandlerError::invariant_violated("Selected request has no path")
                })?
            ),
        };

        self.session_store
            .store_pending_oidc_login(state.secret(), pending_login)
            .await?;

        // Filter out "openid" — AuthenticationFlow::AuthorizationCode adds it automatically
        let scopes = provider_scopes(security_scheme);

        debug!("getting auth url for oidc");

        let auth_url = self
            .identity_provider
            .get_authorization_url(security_scheme, scopes, state, nonce)
            .await?;

        let mut headers = HeaderMap::new();
        headers.insert(
            http::header::LOCATION,
            HeaderValue::from_str(auth_url.url.as_str()).map_err(anyhow::Error::from)?,
        );
        Ok(RouteExecutionResult {
            status: http::StatusCode::FOUND,
            headers,
            body: ResponseBody::NoBody,
        })
    }
}

fn provider_scopes(security_scheme: &SecuritySchemeDetails) -> Vec<openidconnect::Scope> {
    security_scheme
        .scopes
        .iter()
        .filter(|scope| scope.as_str() != "openid")
        .cloned()
        .collect()
}

fn pkce_binding(
    resolved_route: &ResolvedRouteEntry,
    scheme: &SecuritySchemeDetails,
) -> PkceBinding {
    let api_origin = url::Url::parse(&format!(
        "{}://{}",
        resolved_route.public_scheme, resolved_route.public_authority
    ))
    .expect("resolved public origin must be a valid URL")
    .origin()
    .ascii_serialization();
    PkceBinding {
        security_scheme_id: scheme.id,
        security_scheme_revision: scheme.revision,
        environment_id: resolved_route.route.environment_id,
        api_origin,
    }
}

fn oidc_session(
    scopes: Vec<openidconnect::Scope>,
    claims: openidconnect::core::CoreIdTokenClaims,
) -> Result<OidcSession, RequestHandlerError> {
    let expires_at = Utc::now()
        .checked_add_signed(OIDC_SESSION_EXPIRY)
        .ok_or_else(|| anyhow!("Failed to compute expiry"))?;
    Ok(OidcSession {
        subject: claims.subject().to_string(),
        issuer: claims.issuer().to_string(),
        email: claims.email().map(|value| value.to_string()),
        name: claims
            .name()
            .and_then(|value| value.get(None))
            .map(|value| value.to_string()),
        email_verified: claims.email_verified(),
        given_name: claims
            .given_name()
            .and_then(|value| value.get(None))
            .map(|value| value.to_string()),
        family_name: claims
            .family_name()
            .and_then(|value| value.get(None))
            .map(|value| value.to_string()),
        picture: claims
            .picture()
            .and_then(|value| value.get(None))
            .map(|value| value.to_string()),
        preferred_username: claims.preferred_username().map(|value| value.to_string()),
        claims,
        scopes: HashSet::from_iter(scopes),
        expires_at,
    })
}

fn single_callback_parameter<'a>(
    request: &'a RichRequest,
    name: &'static str,
) -> Result<&'a str, RouteExecutionResult> {
    match request.query_params().get(name).map(Vec::as_slice) {
        Some([value]) if !value.is_empty() => Ok(value),
        _ => Err(protocol_error_unchecked(
            StatusCode::BAD_REQUEST,
            invalid_request("authorization callback is invalid"),
        )),
    }
}

fn invalid_request(error_description: &'static str) -> ProtocolErrorResponse {
    ProtocolErrorResponse {
        error: ProtocolErrorCode::InvalidRequest,
        error_description,
    }
}

fn invalid_grant() -> Result<RouteExecutionResult, RequestHandlerError> {
    token_protocol_error(
        StatusCode::BAD_REQUEST,
        ProtocolErrorResponse {
            error: ProtocolErrorCode::InvalidGrant,
            error_description: "authorization grant is invalid",
        },
    )
}

fn protocol_error(
    status: StatusCode,
    error: ProtocolErrorResponse,
) -> Result<RouteExecutionResult, RequestHandlerError> {
    Ok(protocol_error_unchecked(status, error))
}

fn protocol_error_unchecked(
    status: StatusCode,
    error: ProtocolErrorResponse,
) -> RouteExecutionResult {
    RouteExecutionResult {
        status,
        headers: HeaderMap::new(),
        body: ResponseBody::PoemBody {
            body: poem::Body::from_json(&error).expect("protocol error serialization cannot fail"),
            content_type: Some("application/json"),
        },
    }
}

fn token_protocol_error(
    status: StatusCode,
    error: ProtocolErrorResponse,
) -> Result<RouteExecutionResult, RequestHandlerError> {
    let mut response = protocol_error_unchecked(status, error);
    response.headers.extend(token_cache_headers());
    Ok(response)
}

fn token_cache_headers() -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(
        http::header::CACHE_CONTROL,
        HeaderValue::from_static("no-store"),
    );
    headers.insert(http::header::PRAGMA, HeaderValue::from_static("no-cache"));
    headers
}

fn has_form_content_type(request: &RichRequest) -> bool {
    let mut values = request.headers().get_all(http::header::CONTENT_TYPE).iter();
    let Some(value) = values.next() else {
        return false;
    };
    values.next().is_none()
        && value.to_str().is_ok_and(|value| {
            value.split(';').next().is_some_and(|media_type| {
                media_type
                    .trim()
                    .eq_ignore_ascii_case("application/x-www-form-urlencoded")
            })
        })
}

fn bearer_token(request: &RichRequest) -> Option<BearerToken> {
    let mut values = request
        .headers()
        .get_all(http::header::AUTHORIZATION)
        .iter();
    let value = values.next()?.to_str().ok()?;
    if values.next().is_some() {
        return None;
    }
    let (scheme, token) = value.split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("Bearer")
        || token.is_empty()
        || token.len() > 512
        || token.bytes().any(|byte| byte.is_ascii_whitespace())
    {
        return None;
    }
    BearerToken::from_str(token).ok()
}

fn redirect(location: &str) -> Result<RouteExecutionResult, RequestHandlerError> {
    let mut headers = HeaderMap::new();
    headers.insert(
        http::header::LOCATION,
        HeaderValue::from_str(location).map_err(anyhow::Error::from)?,
    );
    Ok(RouteExecutionResult {
        status: StatusCode::FOUND,
        headers,
        body: ResponseBody::NoBody,
    })
}
