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

use crate::mcp::McpCapabilityLookup;
use golem_common::base_model::domain_registration::Domain;
use golem_service_base::custom_api::SecuritySchemeDetails;
use openidconnect::Nonce;
use openidconnect::core::{CoreClient, CoreProviderMetadata};
use poem::http;
use poem::{Endpoint, IntoResponse, Middleware, Request, Response, Result};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use uuid::Uuid;

/// Poem middleware that validates Bearer JWT tokens on MCP requests.
///
/// On each request:
/// 1. Extracts the `Host` header to determine the domain
/// 2. Looks up the `CompiledMcp` for that domain to get the security scheme
/// 3. If a security scheme is configured, validates the `Authorization: Bearer <jwt>` header
/// 4. If no security scheme, passes through without auth
pub struct McpBearerAuth {
    mcp_capability_lookup: Arc<dyn McpCapabilityLookup>,
}

impl McpBearerAuth {
    pub fn new(mcp_capability_lookup: Arc<dyn McpCapabilityLookup>) -> Self {
        Self {
            mcp_capability_lookup,
        }
    }
}

impl<E: Endpoint> Middleware<E> for McpBearerAuth {
    type Output = McpBearerAuthEndpoint<E>;

    fn transform(&self, ep: E) -> Self::Output {
        McpBearerAuthEndpoint {
            inner: ep,
            mcp_capability_lookup: self.mcp_capability_lookup.clone(),
        }
    }
}

pub struct McpBearerAuthEndpoint<E> {
    inner: E,
    mcp_capability_lookup: Arc<dyn McpCapabilityLookup>,
}

impl<E: Endpoint> Endpoint for McpBearerAuthEndpoint<E> {
    type Output = Response;

    async fn call(&self, req: Request) -> Result<Self::Output> {
        // Skip auth for well-known discovery endpoints and OAuth proxy endpoints —
        // they must be publicly accessible so MCP clients can complete the OAuth flow.
        let path = req.uri().path();
        if path.starts_with("/.well-known/") || path.starts_with("/mcp/oauth/") {
            return self.inner.call(req).await.map(|resp| resp.into_response());
        }

        // Only check auth on session-creating requests (no mcp-session-id header).
        // Once a session is established, subsequent requests are trusted via the session.
        // This avoids a gRPC call to the registry on every MCP message.
        let has_session = req.headers().contains_key("mcp-session-id");

        if has_session {
            return self.inner.call(req).await.map(|resp| resp.into_response());
        }

        let host = req
            .headers()
            .get("host")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string());

        let security_scheme = if let Some(host) = &host {
            let domain = Domain(host.clone());
            match self.mcp_capability_lookup.get(&domain).await {
                Ok(compiled_mcp) => compiled_mcp.security_scheme,
                Err(_) => None,
            }
        } else {
            None
        };

        if let Some(scheme) = security_scheme {
            let auth_header = req
                .headers()
                .get("authorization")
                .and_then(|v| v.to_str().ok())
                .map(|s| s.to_string());

            let token = match auth_header {
                Some(header) if header.starts_with("Bearer ") => header[7..].to_string(),
                _ => {
                    tracing::warn!("MCP request missing or invalid Authorization header");
                    return Ok(unauthorized_response(host.as_deref()));
                }
            };

            if let Err(err) = validate_bearer_token(&token, &scheme).await {
                tracing::warn!("MCP Bearer token validation failed: {err}");
                return Ok(unauthorized_response(host.as_deref()));
            }
        }

        self.inner.call(req).await.map(|resp| resp.into_response())
    }
}

fn unauthorized_response(host: Option<&str>) -> Response {
    let resource_metadata_url = if let Some(host) = host {
        format!("http://{host}/.well-known/oauth-protected-resource")
    } else {
        "/.well-known/oauth-protected-resource".to_string()
    };

    Response::builder()
        .status(http::StatusCode::UNAUTHORIZED)
        .header(
            "WWW-Authenticate",
            format!("Bearer realm=\"mcp\", resource_metadata=\"{resource_metadata_url}\""),
        )
        .body(())
}

async fn validate_bearer_token(
    token: &str,
    scheme: &SecuritySchemeDetails,
) -> std::result::Result<(), BearerValidationError> {
    let issuer_url = scheme.provider_type.issuer_url();

    let http_client = openidconnect::reqwest::Client::new();

    let provider_metadata = CoreProviderMetadata::discover_async(issuer_url, &http_client)
        .await
        .map_err(|err| BearerValidationError::ProviderDiscoveryFailed(err.to_string()))?;

    let client = CoreClient::from_provider_metadata(
        provider_metadata,
        scheme.client_id.clone(),
        Some(scheme.client_secret.clone()),
    );

    let verifier = client.id_token_verifier();

    // Parse the raw JWT string as a CoreIdToken
    let id_token: openidconnect::core::CoreIdToken =
        serde_json::from_value(serde_json::Value::String(token.to_string()))
            .map_err(|err| BearerValidationError::InvalidToken(err.to_string()))?;

    // Verify the token's signature and claims.
    // We use a nonce that accepts anything since we didn't initiate this OAuth flow —
    // the MCP client did the OAuth dance and we're just validating the resulting token.
    let _claims = id_token
        .into_claims(&verifier, noop_nonce_verifier)
        .map_err(|err| BearerValidationError::TokenVerificationFailed(err.to_string()))?;

    Ok(())
}

/// A nonce verifier function that accepts any nonce.
/// Used for Bearer token validation where we didn't initiate the OAuth flow.
fn noop_nonce_verifier(_nonce: Option<&Nonce>) -> std::result::Result<(), String> {
    Ok(())
}

#[derive(Debug, thiserror::Error)]
enum BearerValidationError {
    #[error("Failed to discover OIDC provider: {0}")]
    ProviderDiscoveryFailed(String),
    #[error("Invalid token format: {0}")]
    InvalidToken(String),
    #[error("Token verification failed: {0}")]
    TokenVerificationFailed(String),
}

/// In-memory state for the OAuth proxy flow.
/// Tracks pending authorization requests and proxy codes that can be exchanged for tokens.
pub struct OAuthProxyState {
    /// Keyed by the `state` parameter sent to the external provider.
    /// Stores the MCP client's original redirect_uri, state, and PKCE code_challenge.
    pending_requests: RwLock<HashMap<String, PendingAuthRequest>>,
    /// Keyed by the proxy authorization code that Golem issues to the MCP client.
    /// Stores the tokens obtained from the external provider.
    proxy_codes: RwLock<HashMap<String, ProxyCodeEntry>>,
}

struct PendingAuthRequest {
    client_redirect_uri: String,
    client_state: Option<String>,
    _code_challenge: Option<String>,
    _code_challenge_method: Option<String>,
}

struct ProxyCodeEntry {
    id_token: String,
    _access_token: Option<String>,
    refresh_token: Option<String>,
    expires_in: Option<u64>,
    token_type: String,
}

impl OAuthProxyState {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            pending_requests: RwLock::new(HashMap::new()),
            proxy_codes: RwLock::new(HashMap::new()),
        })
    }
}

/// Poem endpoint handler for `/.well-known/oauth-authorization-server` (RFC 8414).
///
/// Returns OAuth authorization server metadata pointing to Golem's own OAuth proxy
/// endpoints (`/authorize`, `/token`, `/oauth/register`) so MCP clients like Claude Desktop
/// and mcp-remote can complete the OAuth flow without needing DCR support from the provider.
pub async fn authorization_server_metadata(
    req: &Request,
    mcp_capability_lookup: &dyn McpCapabilityLookup,
) -> Response {
    let host = req
        .headers()
        .get("host")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("localhost");

    let domain = Domain(host.to_string());

    let security_scheme = match mcp_capability_lookup.get(&domain).await {
        Ok(compiled_mcp) => compiled_mcp.security_scheme,
        Err(_) => None,
    };

    match security_scheme {
        Some(scheme) => {
            let scopes: Vec<String> = scheme.scopes.iter().map(|s| (**s).clone()).collect();

            let redirect_base = scheme.redirect_url.url();
            let base = format!("{}://{}", redirect_base.scheme(), redirect_base.authority());

            let metadata = serde_json::json!({
                "issuer": &base,
                "authorization_endpoint": format!("{base}/mcp/oauth/authorize"),
                "token_endpoint": format!("{base}/mcp/oauth/token"),
                "registration_endpoint": format!("{base}/mcp/oauth/register"),
                "scopes_supported": scopes,
                "response_types_supported": ["code"],
                "code_challenge_methods_supported": ["S256"],
                "grant_types_supported": ["authorization_code", "refresh_token"],
                "token_endpoint_auth_methods_supported": ["client_secret_post", "none"],
            });

            Response::builder()
                .status(http::StatusCode::OK)
                .header("Content-Type", "application/json")
                .body(metadata.to_string())
        }
        None => Response::builder()
            .status(http::StatusCode::NOT_FOUND)
            .body("No security scheme configured for this domain"),
    }
}

/// Poem endpoint handler for `/.well-known/oauth-protected-resource` (RFC 9728).
///
/// Returns JSON metadata pointing MCP clients to Golem's own authorization server
/// so they can perform the OAuth dance to obtain a token.
pub async fn protected_resource_metadata(
    req: &Request,
    mcp_capability_lookup: &dyn McpCapabilityLookup,
) -> Response {
    let host = req
        .headers()
        .get("host")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("localhost");

    let domain = Domain(host.to_string());

    let security_scheme = match mcp_capability_lookup.get(&domain).await {
        Ok(compiled_mcp) => compiled_mcp.security_scheme,
        Err(_) => None,
    };

    match security_scheme {
        Some(scheme) => {
            let scopes: Vec<String> = scheme.scopes.iter().map(|s| (**s).clone()).collect();

            let redirect_base = scheme.redirect_url.url();
            let base = format!("{}://{}", redirect_base.scheme(), redirect_base.authority());

            let metadata = serde_json::json!({
                "resource": format!("{base}/mcp"),
                "authorization_servers": [&base],
                "scopes_supported": scopes,
            });

            Response::builder()
                .status(http::StatusCode::OK)
                .header("Content-Type", "application/json")
                .body(metadata.to_string())
        }
        None => Response::builder()
            .status(http::StatusCode::NOT_FOUND)
            .body("No security scheme configured for this domain"),
    }
}

/// `POST /oauth/register` — fake Dynamic Client Registration.
///
/// MCP clients (mcp-remote, Claude Desktop) expect DCR support. Since providers like Google
/// don't offer it, we return the pre-configured client_id from the security scheme.
pub async fn oauth_register(
    req: &Request,
    mcp_capability_lookup: &dyn McpCapabilityLookup,
) -> Response {
    let host = req
        .headers()
        .get("host")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("localhost");

    let domain = Domain(host.to_string());

    let security_scheme = match mcp_capability_lookup.get(&domain).await {
        Ok(compiled_mcp) => compiled_mcp.security_scheme,
        Err(_) => None,
    };

    match security_scheme {
        Some(scheme) => {
            let response = serde_json::json!({
                "client_id": scheme.client_id.as_str(),
                "client_secret": scheme.client_secret.secret(),
                "client_id_issued_at": 0,
                "client_secret_expires_at": 0,
                "redirect_uris": [],
                "grant_types": ["authorization_code", "refresh_token"],
                "response_types": ["code"],
                "token_endpoint_auth_method": "client_secret_post",
            });

            Response::builder()
                .status(http::StatusCode::CREATED)
                .header("Content-Type", "application/json")
                .body(response.to_string())
        }
        None => Response::builder()
            .status(http::StatusCode::NOT_FOUND)
            .body("No security scheme configured for this domain"),
    }
}

/// `GET /authorize` — OAuth proxy authorization endpoint.
///
/// Stores the MCP client's redirect_uri/state/code_challenge in memory, then redirects the
/// user to the external provider's (e.g. Google's) authorization endpoint with Golem's own
/// callback URL so we can intercept the authorization code.
pub async fn oauth_authorize(
    req: &Request,
    mcp_capability_lookup: &dyn McpCapabilityLookup,
    state: &OAuthProxyState,
) -> Response {
    let host = req
        .headers()
        .get("host")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("localhost");

    let domain = Domain(host.to_string());

    let security_scheme = match mcp_capability_lookup.get(&domain).await {
        Ok(compiled_mcp) => compiled_mcp.security_scheme,
        Err(_) => None,
    };

    let scheme = match security_scheme {
        Some(s) => s,
        None => {
            return Response::builder()
                .status(http::StatusCode::NOT_FOUND)
                .body("No security scheme configured for this domain");
        }
    };

    let query = req.uri().query().unwrap_or("");
    let params: Vec<(String, String)> = url::form_urlencoded::parse(query.as_bytes())
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();

    let get_param = |name: &str| -> Option<String> {
        params
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.clone())
    };

    let client_redirect_uri = match get_param("redirect_uri") {
        Some(uri) => uri,
        None => {
            return Response::builder()
                .status(http::StatusCode::BAD_REQUEST)
                .body("Missing redirect_uri parameter");
        }
    };

    let client_state = get_param("state");
    let code_challenge = get_param("code_challenge");
    let code_challenge_method = get_param("code_challenge_method");
    let scope = get_param("scope");

    // Generate a unique state for the external provider request so we can correlate the callback
    let proxy_state = Uuid::new_v4().to_string();

    state.pending_requests.write().await.insert(
        proxy_state.clone(),
        PendingAuthRequest {
            client_redirect_uri,
            client_state,
            _code_challenge: code_challenge,
            _code_challenge_method: code_challenge_method,
        },
    );

    // Discover the provider's authorization endpoint
    let issuer_url = scheme.provider_type.issuer_url();
    let http_client = openidconnect::reqwest::Client::new();
    let provider_metadata =
        match CoreProviderMetadata::discover_async(issuer_url, &http_client).await {
            Ok(m) => m,
            Err(err) => {
                tracing::error!("Failed to discover OIDC provider for /authorize: {err}");
                return Response::builder()
                    .status(http::StatusCode::INTERNAL_SERVER_ERROR)
                    .body("Failed to discover authorization server");
            }
        };

    let auth_endpoint = provider_metadata.authorization_endpoint().as_str();

    let golem_callback_url = scheme.redirect_url.url().to_string();

    let scopes_str = scope.unwrap_or_else(|| {
        scheme
            .scopes
            .iter()
            .map(|s| (**s).clone())
            .collect::<Vec<_>>()
            .join(" ")
    });

    let redirect_url = format!(
        "{auth_endpoint}?response_type=code&client_id={}&redirect_uri={}&state={}&scope={}&access_type=offline",
        urlencoding::encode(scheme.client_id.as_str()),
        urlencoding::encode(&golem_callback_url),
        urlencoding::encode(&proxy_state),
        urlencoding::encode(&scopes_str),
    );

    Response::builder()
        .status(http::StatusCode::FOUND)
        .header("Location", redirect_url)
        .body(())
}

/// `GET /oauth/callback` — OAuth proxy callback endpoint.
///
/// The external provider (e.g. Google) redirects here with `?code=...&state=...`.
/// Golem exchanges the authorization code for tokens at the provider's token endpoint,
/// generates its own proxy authorization code, stores the tokens, then redirects to the
/// MCP client's original redirect_uri with the proxy code and original state.
pub async fn oauth_callback(
    req: &Request,
    mcp_capability_lookup: &dyn McpCapabilityLookup,
    proxy_state: &OAuthProxyState,
) -> Response {
    let host = req
        .headers()
        .get("host")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("localhost");

    let domain = Domain(host.to_string());

    let query = req.uri().query().unwrap_or("");
    let params: Vec<(String, String)> = url::form_urlencoded::parse(query.as_bytes())
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();

    let get_param = |name: &str| -> Option<String> {
        params
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.clone())
    };

    let provider_code = match get_param("code") {
        Some(c) => c,
        None => {
            return Response::builder()
                .status(http::StatusCode::BAD_REQUEST)
                .body("Missing code parameter from provider");
        }
    };

    let state_param = match get_param("state") {
        Some(s) => s,
        None => {
            return Response::builder()
                .status(http::StatusCode::BAD_REQUEST)
                .body("Missing state parameter from provider");
        }
    };

    // Look up the pending request by the state we sent to the provider
    let pending = proxy_state
        .pending_requests
        .write()
        .await
        .remove(&state_param);
    let pending = match pending {
        Some(p) => p,
        None => {
            return Response::builder()
                .status(http::StatusCode::BAD_REQUEST)
                .body("Unknown or expired state parameter");
        }
    };

    let security_scheme = match mcp_capability_lookup.get(&domain).await {
        Ok(compiled_mcp) => compiled_mcp.security_scheme,
        Err(_) => None,
    };

    let scheme = match security_scheme {
        Some(s) => s,
        None => {
            return Response::builder()
                .status(http::StatusCode::NOT_FOUND)
                .body("No security scheme configured for this domain");
        }
    };

    // Exchange the provider's authorization code for tokens
    let issuer_url = scheme.provider_type.issuer_url();
    let http_client = openidconnect::reqwest::Client::new();
    let provider_metadata =
        match CoreProviderMetadata::discover_async(issuer_url, &http_client).await {
            Ok(m) => m,
            Err(err) => {
                tracing::error!("Failed to discover OIDC provider for /oauth/callback: {err}");
                return Response::builder()
                    .status(http::StatusCode::INTERNAL_SERVER_ERROR)
                    .body("Failed to discover authorization server");
            }
        };

    let token_endpoint = match provider_metadata.token_endpoint() {
        Some(url) => url.as_str().to_string(),
        None => {
            return Response::builder()
                .status(http::StatusCode::INTERNAL_SERVER_ERROR)
                .body("Provider has no token endpoint");
        }
    };

    let golem_callback_url = scheme.redirect_url.url().to_string();

    let http_post_client = openidconnect::reqwest::Client::new();
    let token_response = http_post_client
        .post(&token_endpoint)
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", &provider_code),
            ("redirect_uri", &golem_callback_url),
            ("client_id", scheme.client_id.as_str()),
            ("client_secret", scheme.client_secret.secret()),
        ])
        .send()
        .await;

    let token_response: openidconnect::reqwest::Response = match token_response {
        Ok(resp) => resp,
        Err(err) => {
            tracing::error!("Token exchange request failed: {err}");
            return Response::builder()
                .status(http::StatusCode::INTERNAL_SERVER_ERROR)
                .body("Token exchange failed");
        }
    };

    let token_body: serde_json::Value = match token_response.json().await {
        Ok(v) => v,
        Err(err) => {
            tracing::error!("Failed to parse token response: {err}");
            return Response::builder()
                .status(http::StatusCode::INTERNAL_SERVER_ERROR)
                .body("Failed to parse token response");
        }
    };

    let id_token = match token_body.get("id_token").and_then(|v| v.as_str()) {
        Some(t) => t.to_string(),
        None => {
            // If no id_token, try access_token as fallback
            match token_body.get("access_token").and_then(|v| v.as_str()) {
                Some(t) => t.to_string(),
                None => {
                    tracing::error!("Token response has no id_token or access_token: {token_body}");
                    return Response::builder()
                        .status(http::StatusCode::INTERNAL_SERVER_ERROR)
                        .body("No token in provider response");
                }
            }
        }
    };

    let access_token = token_body
        .get("access_token")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let refresh_token = token_body
        .get("refresh_token")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let expires_in = token_body.get("expires_in").and_then(|v| v.as_u64());
    let token_type = token_body
        .get("token_type")
        .and_then(|v| v.as_str())
        .unwrap_or("Bearer")
        .to_string();

    // Generate a proxy authorization code for the MCP client
    let proxy_code = Uuid::new_v4().to_string();

    proxy_state.proxy_codes.write().await.insert(
        proxy_code.clone(),
        ProxyCodeEntry {
            id_token,
            _access_token: access_token,
            refresh_token,
            expires_in,
            token_type,
        },
    );

    // Redirect to the MCP client's original redirect_uri with the proxy code
    let mut redirect_url = format!(
        "{}?code={}",
        pending.client_redirect_uri,
        urlencoding::encode(&proxy_code),
    );

    if let Some(client_state) = &pending.client_state {
        redirect_url.push_str(&format!("&state={}", urlencoding::encode(client_state)));
    }

    Response::builder()
        .status(http::StatusCode::FOUND)
        .header("Location", redirect_url)
        .body(())
}

/// `POST /token` — OAuth proxy token endpoint.
///
/// The MCP client exchanges Golem's proxy authorization code for the stored tokens
/// (id_token from the external provider). This completes the OAuth flow from the
/// MCP client's perspective.
///
/// Takes an owned `Request` so we can consume the POST form body.
pub async fn oauth_token(mut req: Request, proxy_state: &OAuthProxyState) -> Response {
    // Read the form body — MCP clients POST application/x-www-form-urlencoded
    let body_bytes = match req.take_body().into_bytes().await {
        Ok(b) => b,
        Err(_) => bytes::Bytes::new(),
    };

    let params: Vec<(String, String)> = url::form_urlencoded::parse(&body_bytes)
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();

    let get_param = |name: &str| -> Option<String> {
        params
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.clone())
    };

    let code = match get_param("code") {
        Some(c) => c,
        None => {
            return Response::builder()
                .status(http::StatusCode::BAD_REQUEST)
                .header("Content-Type", "application/json")
                .body(serde_json::json!({"error": "invalid_request", "error_description": "Missing code parameter"}).to_string());
        }
    };

    let entry = proxy_state.proxy_codes.write().await.remove(&code);
    match entry {
        Some(entry) => {
            let mut response = serde_json::json!({
                "access_token": entry.id_token,
                "token_type": entry.token_type,
            });

            if let Some(expires_in) = entry.expires_in {
                response["expires_in"] = serde_json::json!(expires_in);
            }
            if let Some(refresh_token) = &entry.refresh_token {
                response["refresh_token"] = serde_json::json!(refresh_token);
            }

            Response::builder()
                .status(http::StatusCode::OK)
                .header("Content-Type", "application/json")
                .body(response.to_string())
        }
        None => {
            Response::builder()
                .status(http::StatusCode::BAD_REQUEST)
                .header("Content-Type", "application/json")
                .body(serde_json::json!({"error": "invalid_grant", "error_description": "Unknown or expired authorization code"}).to_string())
        }
    }
}
