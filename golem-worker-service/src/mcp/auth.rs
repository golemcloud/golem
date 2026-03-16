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

use crate::custom_api::oidc::IdentityProvider;
use crate::mcp::McpCapabilityLookup;
use golem_common::base_model::domain_registration::Domain;
use golem_service_base::custom_api::SecuritySchemeDetails;
use openidconnect::{AuthorizationCode, CsrfToken, Nonce, Scope};
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
    identity_provider: Arc<dyn IdentityProvider>,
}

impl McpBearerAuth {
    pub fn new(
        mcp_capability_lookup: Arc<dyn McpCapabilityLookup>,
        identity_provider: Arc<dyn IdentityProvider>,
    ) -> Self {
        Self {
            mcp_capability_lookup,
            identity_provider,
        }
    }
}

impl<E: Endpoint> Middleware<E> for McpBearerAuth {
    type Output = McpBearerAuthEndpoint<E>;

    fn transform(&self, ep: E) -> Self::Output {
        McpBearerAuthEndpoint {
            inner: ep,
            mcp_capability_lookup: self.mcp_capability_lookup.clone(),
            identity_provider: self.identity_provider.clone(),
        }
    }
}

pub struct McpBearerAuthEndpoint<E> {
    inner: E,
    mcp_capability_lookup: Arc<dyn McpCapabilityLookup>,
    identity_provider: Arc<dyn IdentityProvider>,
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

            if let Err(err) = self
                .identity_provider
                .validate_bearer_token(&scheme, &token)
                .await
            {
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

/// In-memory state for the OAuth proxy flow.
/// Tracks pending authorization requests and proxy codes that can be exchanged for tokens.
pub struct OAuthProxyState {
    /// Keyed by the `state` parameter sent to the external provider.
    /// Stores the MCP client's original redirect_uri and state.
    pending_requests: RwLock<HashMap<String, PendingAuthRequest>>,
    /// Keyed by the proxy authorization code that Golem issues to the MCP client.
    /// Stores the tokens obtained from the external provider.
    proxy_codes: RwLock<HashMap<String, ProxyCodeEntry>>,
}

struct PendingAuthRequest {
    client_redirect_uri: String,
    client_state: Option<String>,
}

struct ProxyCodeEntry {
    id_token: String,
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

fn resolve_base_url(scheme: &SecuritySchemeDetails) -> String {
    let redirect_base = scheme.redirect_url.url();
    format!("{}://{}", redirect_base.scheme(), redirect_base.authority())
}

/// Poem endpoint handler for `/.well-known/oauth-authorization-server` (RFC 8414).
///
/// Returns OAuth authorization server metadata pointing to Golem's own OAuth proxy
/// endpoints so MCP clients like Claude Desktop and mcp-remote can complete the
/// OAuth flow without needing DCR support from the provider.
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
            let base = resolve_base_url(&scheme);

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
            let base = resolve_base_url(&scheme);

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
/// Stores the MCP client's redirect_uri/state in memory, then redirects the user to the
/// external provider's authorization endpoint via `IdentityProvider::get_authorization_url`.
pub async fn oauth_authorize(
    req: &Request,
    mcp_capability_lookup: &dyn McpCapabilityLookup,
    identity_provider: &dyn IdentityProvider,
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
    let scope = get_param("scope");

    // Generate a unique state for the external provider request
    let proxy_csrf = CsrfToken::new(Uuid::new_v4().to_string());
    let proxy_nonce = Nonce::new_random();

    state.pending_requests.write().await.insert(
        proxy_csrf.secret().clone(),
        PendingAuthRequest {
            client_redirect_uri,
            client_state,
        },
    );

    let scopes: Vec<Scope> = scope
        .map(|s| {
            s.split_whitespace()
                .map(|s| Scope::new(s.to_string()))
                .collect()
        })
        .unwrap_or_else(|| scheme.scopes.clone());

    match identity_provider
        .get_authorization_url(&scheme, scopes, proxy_csrf, proxy_nonce)
        .await
    {
        Ok(auth_url) => Response::builder()
            .status(http::StatusCode::FOUND)
            .header("Location", auth_url.url.to_string())
            .body(()),
        Err(err) => {
            tracing::error!("Failed to get authorization URL: {err}");
            Response::builder()
                .status(http::StatusCode::INTERNAL_SERVER_ERROR)
                .body("Failed to initiate authorization")
        }
    }
}

/// `GET /oauth/callback` — OAuth proxy callback endpoint.
///
/// The external provider redirects here with `?code=...&state=...`.
/// Uses `IdentityProvider::exchange_code_for_raw_id_token` to exchange the code,
/// generates a proxy authorization code, and redirects to the MCP client.
pub async fn oauth_callback(
    req: &Request,
    mcp_capability_lookup: &dyn McpCapabilityLookup,
    identity_provider: &dyn IdentityProvider,
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

    let raw_tokens = match identity_provider
        .exchange_code_for_raw_id_token(&scheme, &AuthorizationCode::new(provider_code))
        .await
    {
        Ok(t) => t,
        Err(err) => {
            tracing::error!("Token exchange failed: {err}");
            return Response::builder()
                .status(http::StatusCode::INTERNAL_SERVER_ERROR)
                .body("Token exchange failed");
        }
    };

    // Generate a proxy authorization code for the MCP client
    let proxy_code = Uuid::new_v4().to_string();

    proxy_state.proxy_codes.write().await.insert(
        proxy_code.clone(),
        ProxyCodeEntry {
            id_token: raw_tokens.id_token,
            refresh_token: raw_tokens.refresh_token,
            expires_in: raw_tokens.expires_in,
            token_type: raw_tokens.token_type,
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
/// The MCP client exchanges Golem's proxy authorization code for the stored tokens.
/// Takes an owned `Request` so we can consume the POST form body.
pub async fn oauth_token(mut req: Request, proxy_state: &OAuthProxyState) -> Response {
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
                .body(
                    serde_json::json!({"error": "invalid_request", "error_description": "Missing code parameter"})
                        .to_string(),
                );
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
        None => Response::builder()
            .status(http::StatusCode::BAD_REQUEST)
            .header("Content-Type", "application/json")
            .body(
                serde_json::json!({"error": "invalid_grant", "error_description": "Unknown or expired authorization code"})
                    .to_string(),
            ),
    }
}
