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
use crate::custom_api::oidc::model::{McpPendingAuth, McpProxyCodeEntry};
use crate::custom_api::oidc::session_store::SessionStore;
use crate::mcp::McpCapabilityLookup;
use golem_common::base_model::domain_registration::Domain;
use golem_common::cache::{BackgroundEvictionMode, Cache, FullCacheEvictionMode, SimpleCache};
use golem_service_base::custom_api::SecuritySchemeDetails;
use golem_service_base::mcp::CompiledMcp;
use openidconnect::{AuthorizationCode, CsrfToken, Nonce, Scope};
use poem::http;
use poem::{Endpoint, IntoResponse, Middleware, Request, Response, Result, Route};
use sha2::{Digest, Sha256};
use std::sync::Arc;
use std::time::Duration;
use uuid::Uuid;

/// The expected path suffix for the MCP OAuth callback.
/// The security scheme's `redirect_url` MUST have this path for the proxy flow to work,
/// because this is where the external provider (e.g. Google) redirects after authentication.
const MCP_OAUTH_CALLBACK_PATH: &str = "/mcp/oauth/callback";

/// How long a successfully validated token is cached before re-validation.
const TOKEN_CACHE_TTL: Duration = Duration::from_secs(60);

/// How often the background task scans for expired cache entries.
const TOKEN_CACHE_EVICTION_PERIOD: Duration = Duration::from_secs(30);

/// Hash of a Bearer token, used as a cache key to avoid re-validating
/// the same token against the identity provider on every request.
type TokenHash = [u8; 32];

/// Per-instance cache of successfully validated tokens.
/// Each instance maintains its own cache independently; a cache miss
/// simply triggers a fresh validation against the identity provider.
/// Uses `Cache<TokenHash, (), (), String>` — the value `()` means we only
/// care about presence (token was validated), not about storing data.
type ValidatedTokenCache = Cache<TokenHash, (), (), String>;

/// Poem middleware that validates Bearer JWT tokens on MCP requests.
///
/// Per the MCP spec (2025-06-18), authorization MUST be included in every HTTP
/// request, even within an established session. To avoid hitting the identity
/// provider on every message, validated tokens are cached locally with a short TTL.
pub struct McpBearerAuth {
    mcp_capability_lookup: Arc<dyn McpCapabilityLookup>,
    identity_provider: Arc<dyn IdentityProvider>,
    validated_tokens: ValidatedTokenCache,
}

impl McpBearerAuth {
    pub fn new(
        mcp_capability_lookup: Arc<dyn McpCapabilityLookup>,
        identity_provider: Arc<dyn IdentityProvider>,
    ) -> Self {
        Self {
            mcp_capability_lookup,
            identity_provider,
            validated_tokens: Cache::new(
                None,
                FullCacheEvictionMode::None,
                BackgroundEvictionMode::OlderThan {
                    ttl: TOKEN_CACHE_TTL,
                    period: TOKEN_CACHE_EVICTION_PERIOD,
                },
                "mcp_validated_tokens",
            ),
        }
    }

    fn token_hash(
        token: &str,
        deployment: &CompiledMcp,
        scheme: &SecuritySchemeDetails,
    ) -> TokenHash {
        Self::hash_parts(&[
            token.to_string(),
            deployment.domain.0.clone(),
            deployment.environment_id.to_string(),
            deployment.deployment_revision.to_string(),
            scheme.id.to_string(),
            scheme.name.to_string(),
            format!("{:?}", scheme.provider_type),
            scheme.client_id.as_str().to_string(),
            scheme.client_secret.secret().clone(),
            scheme.redirect_url.as_str().to_string(),
            format!("{:?}", scheme.scopes),
        ])
    }

    fn hash_parts(parts: &[String]) -> TokenHash {
        let mut hasher = Sha256::new();
        for part in parts {
            hasher.update((part.len() as u64).to_le_bytes());
            hasher.update(part.as_bytes());
        }
        hasher.finalize().into()
    }
}

impl<E: Endpoint> Middleware<E> for McpBearerAuth {
    type Output = McpBearerAuthEndpoint<E>;

    fn transform(&self, ep: E) -> Self::Output {
        McpBearerAuthEndpoint {
            inner: ep,
            mcp_capability_lookup: self.mcp_capability_lookup.clone(),
            identity_provider: self.identity_provider.clone(),
            validated_tokens: self.validated_tokens.clone(),
        }
    }
}

pub struct McpBearerAuthEndpoint<E> {
    inner: E,
    mcp_capability_lookup: Arc<dyn McpCapabilityLookup>,
    identity_provider: Arc<dyn IdentityProvider>,
    validated_tokens: ValidatedTokenCache,
}

impl<E: Endpoint> Endpoint for McpBearerAuthEndpoint<E> {
    type Output = Response;

    async fn call(&self, mut req: Request) -> Result<Self::Output> {
        let host = resolve_effective_host(req.headers());
        let Some(host_name) = &host else {
            return Ok(Response::builder()
                .status(http::StatusCode::BAD_REQUEST)
                .finish());
        };
        let compiled = match self
            .mcp_capability_lookup
            .get(&Domain(host_name.clone()))
            .await
        {
            Ok(compiled) => Arc::new(compiled),
            Err(_) => {
                return Ok(Response::builder()
                    .status(http::StatusCode::SERVICE_UNAVAILABLE)
                    .finish());
            }
        };
        if compiled.security_scheme_name.is_some() && compiled.security_scheme.is_none() {
            return Ok(Response::builder()
                .status(http::StatusCode::SERVICE_UNAVAILABLE)
                .finish());
        }

        if let Some(scheme) = &compiled.security_scheme {
            let auth_header = req
                .headers()
                .get("authorization")
                .and_then(|v| v.to_str().ok())
                .map(|s| s.to_string());

            let token = match auth_header {
                Some(header) if header.starts_with("Bearer ") => header[7..].to_string(),
                _ => {
                    tracing::warn!("MCP request missing or invalid Authorization header");
                    return Ok(unauthorized_response(host.as_deref(), scheme));
                }
            };

            let hash = McpBearerAuth::token_hash(&token, &compiled, scheme);
            let identity_provider = self.identity_provider.clone();

            let result = self
                .validated_tokens
                .get_or_insert_simple(&hash, async || {
                    identity_provider
                        .validate_bearer_token(scheme, &token)
                        .await
                        .map_err(|err| err.to_string())
                })
                .await;

            if let Err(err) = result {
                tracing::warn!(error = %err, "MCP Bearer token validation failed");
                return Ok(unauthorized_response(host.as_deref(), scheme));
            }
        }

        req.extensions_mut().insert(compiled);
        self.inner.call(req).await.map(|resp| resp.into_response())
    }
}

fn unauthorized_response(host: Option<&str>, scheme: &SecuritySchemeDetails) -> Response {
    let proto = scheme.redirect_url.url().scheme();
    let resource_metadata_url = if let Some(host) = host {
        format!("{proto}://{host}/.well-known/oauth-protected-resource")
    } else {
        "/.well-known/oauth-protected-resource".to_string()
    };

    Response::builder()
        .status(http::StatusCode::UNAUTHORIZED)
        .header(
            "WWW-Authenticate",
            format!("Bearer realm=\"mcp\", resource_metadata=\"{resource_metadata_url}\""),
        )
        .header("Content-Type", "application/json")
        .body(
            serde_json::json!({
                "error": "unauthorized",
                "error_description": "Bearer token required"
            })
            .to_string(),
        )
}

/// Resolves the effective host from the request, respecting reverse proxies.
/// Priority: `X-Forwarded-Host` → `Host` header.
pub fn resolve_effective_host(headers: &http::HeaderMap) -> Option<String> {
    // 1. X-Forwarded-Host (de facto standard set by reverse proxies)
    //    May be comma-separated; take the first (client-side) value.
    if let Some(host) = headers
        .get("x-forwarded-host")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(',').next())
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        return Some(host.to_owned());
    }

    // 2. Host header (required in HTTP/1.1, always present in HTTP/2)
    headers
        .get(http::header::HOST)
        .and_then(|v| v.to_str().ok())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_owned())
}

/// Builds the externally-visible origin for the resource from the request's
/// effective host combined with the scheme from the redirect URL configuration
/// (or `X-Forwarded-Proto` when set by a trusted proxy).
fn resolve_resource_origin(req: &Request, scheme: &SecuritySchemeDetails) -> String {
    let host = resolve_effective_host(req.headers()).unwrap_or_else(|| "localhost".to_owned());

    let proto = req
        .headers()
        .get("x-forwarded-proto")
        .and_then(|v| v.to_str().ok())
        .filter(|p| *p == "http" || *p == "https")
        .unwrap_or_else(|| scheme.redirect_url.url().scheme());

    format!("{proto}://{host}")
}

/// Validates that the security scheme's `redirect_url` ends with the expected
/// MCP OAuth callback path. If it doesn't, the external provider would redirect
/// to the wrong place and the proxy flow would silently break.
fn validate_redirect_url(scheme: &SecuritySchemeDetails) -> std::result::Result<(), Response> {
    let path = scheme.redirect_url.url().path();
    if path != MCP_OAUTH_CALLBACK_PATH {
        tracing::error!(
            "Security scheme redirect_url path is '{}' but must be '{}' for MCP OAuth proxy",
            path,
            MCP_OAUTH_CALLBACK_PATH,
        );
        Err(Response::builder()
            .status(http::StatusCode::INTERNAL_SERVER_ERROR)
            .body("Security scheme redirect_url is not configured for MCP OAuth proxy"))
    } else {
        Ok(())
    }
}

pub async fn authorization_server_metadata(
    req: &Request,
    mcp_capability_lookup: &dyn McpCapabilityLookup,
) -> Response {
    let host = resolve_effective_host(req.headers()).unwrap_or_else(|| "localhost".to_owned());

    let domain = Domain(host.clone());

    let security_scheme = match mcp_capability_lookup.get(&domain).await {
        Ok(compiled_mcp) => compiled_mcp.security_scheme,
        Err(_) => None,
    };

    match security_scheme {
        Some(scheme) => {
            let scopes: Vec<String> = scheme.scopes.iter().map(|s| (**s).clone()).collect();
            let base = resolve_resource_origin(req, &scheme);

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

/// Returns JSON metadata pointing MCP clients to Golem's own authorization server
/// so they can perform the OAuth dance to obtain a token.
pub async fn protected_resource_metadata(
    req: &Request,
    mcp_capability_lookup: &dyn McpCapabilityLookup,
) -> Response {
    let host = resolve_effective_host(req.headers()).unwrap_or_else(|| "localhost".to_owned());

    let domain = Domain(host.clone());

    let security_scheme = match mcp_capability_lookup.get(&domain).await {
        Ok(compiled_mcp) => compiled_mcp.security_scheme,
        Err(_) => None,
    };

    match security_scheme {
        Some(scheme) => {
            let scopes: Vec<String> = scheme.scopes.iter().map(|s| (**s).clone()).collect();
            let resource_base = resolve_resource_origin(req, &scheme);

            let metadata = serde_json::json!({
                "resource": format!("{resource_base}/mcp"),
                "authorization_servers": [&resource_base],
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

/// `POST /oauth/register` — Dynamic Client Registration (RFC 7591).
///
/// Returns the pre-configured `client_id` and `client_secret` from the security scheme,
/// allowing MCP clients to discover credentials without manual configuration.
/// Echoes back the client's requested `redirect_uris` so tools like `mcp-remote`
/// can find their localhost callback URI in the response.
pub async fn oauth_register(
    mut req: Request,
    mcp_capability_lookup: &dyn McpCapabilityLookup,
) -> Response {
    let host = resolve_effective_host(req.headers()).unwrap_or_else(|| "localhost".to_owned());

    let domain = Domain(host);

    // Parse the client's registration request to extract redirect_uris
    let client_redirect_uris: serde_json::Value = match req.take_body().into_bytes().await {
        Ok(bytes) => serde_json::from_slice::<serde_json::Value>(&bytes)
            .ok()
            .and_then(|v| v.get("redirect_uris").cloned())
            .unwrap_or(serde_json::json!([])),
        Err(_) => serde_json::json!([]),
    };

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
                "redirect_uris": client_redirect_uris,
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
/// Stores the MCP client's redirect_uri/state in the session store, then redirects the user
/// to the external provider's authorization endpoint via `IdentityProvider::get_authorization_url`.
pub async fn oauth_authorize(
    req: &Request,
    mcp_capability_lookup: &dyn McpCapabilityLookup,
    identity_provider: &dyn IdentityProvider,
    session_store: &dyn SessionStore,
) -> Response {
    let host = resolve_effective_host(req.headers()).unwrap_or_else(|| "localhost".to_owned());

    let domain = Domain(host.clone());

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

    if let Err(resp) = validate_redirect_url(&scheme) {
        return resp;
    }

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

    // RFC 8707: MCP clients MUST include the `resource` parameter to bind tokens
    // to the intended MCP server. We accept it here for spec compliance; audience
    // validation against this value is a future enhancement (see validate_bearer_token).
    if let Some(resource) = get_param("resource") {
        tracing::debug!(
            resource,
            "MCP OAuth authorize: client provided resource parameter"
        );
    }

    // Generate a unique state for the external provider request
    let proxy_csrf = CsrfToken::new(Uuid::new_v4().to_string());
    let proxy_nonce = Nonce::new_random();

    if let Err(err) = session_store
        .store_mcp_pending_auth(
            proxy_csrf.secret(),
            McpPendingAuth {
                client_redirect_uri,
                client_state,
            },
        )
        .await
    {
        tracing::error!("Failed to store MCP pending auth: {err}");
        return Response::builder()
            .status(http::StatusCode::INTERNAL_SERVER_ERROR)
            .body("Failed to store pending authorization");
    }

    // Filter out "openid" — the openidconnect library's AuthenticationFlow::AuthorizationCode
    // adds it automatically, so including it here would duplicate it in the URL.
    let scopes: Vec<Scope> = scope
        .map(|s| {
            s.split_whitespace()
                .filter(|s| *s != "openid")
                .map(|s| Scope::new(s.to_string()))
                .collect()
        })
        .unwrap_or_else(|| {
            scheme
                .scopes
                .iter()
                .filter(|s| s.as_str() != "openid")
                .cloned()
                .collect()
        });

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
    session_store: &dyn SessionStore,
) -> Response {
    let host = resolve_effective_host(req.headers()).unwrap_or_else(|| "localhost".to_owned());

    let domain = Domain(host.clone());

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

    let pending = match session_store.take_mcp_pending_auth(&state_param).await {
        Ok(Some(p)) => p,
        Ok(None) => {
            return Response::builder()
                .status(http::StatusCode::BAD_REQUEST)
                .body("Unknown or expired state parameter");
        }
        Err(err) => {
            tracing::error!("Failed to retrieve MCP pending auth: {err}");
            return Response::builder()
                .status(http::StatusCode::INTERNAL_SERVER_ERROR)
                .body("Failed to retrieve pending authorization");
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

    if let Err(err) = session_store
        .store_mcp_proxy_code(
            &proxy_code,
            McpProxyCodeEntry {
                id_token: raw_tokens.id_token,
                refresh_token: raw_tokens.refresh_token,
                expires_in: raw_tokens.expires_in,
                token_type: raw_tokens.token_type,
            },
        )
        .await
    {
        tracing::error!("Failed to store MCP proxy code: {err}");
        return Response::builder()
            .status(http::StatusCode::INTERNAL_SERVER_ERROR)
            .body("Failed to store proxy authorization code");
    }

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
pub async fn oauth_token(mut req: Request, session_store: &dyn SessionStore) -> Response {
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

    let entry = match session_store.take_mcp_proxy_code(&code).await {
        Ok(e) => e,
        Err(err) => {
            tracing::error!("Failed to retrieve MCP proxy code: {err}");
            return Response::builder()
                .status(http::StatusCode::INTERNAL_SERVER_ERROR)
                .header("Content-Type", "application/json")
                .body(
                    serde_json::json!({"error": "server_error", "error_description": "Internal error"})
                        .to_string(),
                );
        }
    };

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

/// Builds the Poem `Route` with all MCP OAuth proxy endpoints wired up.
///
/// Keeps the route setup out of `lib.rs` and co-located with the handler functions.
pub fn oauth_proxy_routes(
    mcp_capability_lookup: Arc<dyn McpCapabilityLookup>,
    identity_provider: Arc<dyn IdentityProvider>,
    session_store: Arc<dyn SessionStore>,
) -> Route {
    let lookup_metadata = mcp_capability_lookup.clone();
    let lookup_authz = mcp_capability_lookup.clone();
    let lookup_register = mcp_capability_lookup.clone();
    let lookup_authorize = mcp_capability_lookup.clone();
    let lookup_callback = mcp_capability_lookup;

    let idp_authorize = identity_provider.clone();
    let idp_callback = identity_provider;

    let store_authorize = session_store.clone();
    let store_callback = session_store.clone();
    let store_token = session_store;

    Route::new()
        .at(
            "/.well-known/oauth-protected-resource",
            poem::endpoint::make(move |req: Request| {
                let lookup = lookup_metadata.clone();
                async move { protected_resource_metadata(&req, lookup.as_ref()).await }
            }),
        )
        .at(
            "/.well-known/oauth-authorization-server",
            poem::endpoint::make(move |req: Request| {
                let lookup = lookup_authz.clone();
                async move { authorization_server_metadata(&req, lookup.as_ref()).await }
            }),
        )
        .at(
            "/mcp/oauth/register",
            poem::endpoint::make(move |req: Request| {
                let lookup = lookup_register.clone();
                async move { oauth_register(req, lookup.as_ref()).await }
            }),
        )
        .at(
            "/mcp/oauth/authorize",
            poem::endpoint::make(move |req: Request| {
                let lookup = lookup_authorize.clone();
                let idp = idp_authorize.clone();
                let store = store_authorize.clone();
                async move {
                    oauth_authorize(&req, lookup.as_ref(), idp.as_ref(), store.as_ref()).await
                }
            }),
        )
        .at(
            MCP_OAUTH_CALLBACK_PATH,
            poem::endpoint::make(move |req: Request| {
                let lookup = lookup_callback.clone();
                let idp = idp_callback.clone();
                let store = store_callback.clone();
                async move {
                    oauth_callback(&req, lookup.as_ref(), idp.as_ref(), store.as_ref()).await
                }
            }),
        )
        .at(
            "/mcp/oauth/token",
            poem::endpoint::make(move |req: Request| {
                let store = store_token.clone();
                async move { oauth_token(req, store.as_ref()).await }
            }),
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mcp::mcp_capabilities_lookup::McpCapabilitiesLookupError;
    use test_r::test;

    struct Lookup(Option<CompiledMcp>);

    #[async_trait::async_trait]
    impl McpCapabilityLookup for Lookup {
        async fn get(
            &self,
            domain: &Domain,
        ) -> std::result::Result<CompiledMcp, McpCapabilitiesLookupError> {
            self.0
                .clone()
                .ok_or_else(|| McpCapabilitiesLookupError::UnknownSite(domain.clone()))
        }
        async fn invalidate(&self, _: &Domain) {}
        async fn invalidate_all(&self) {}
    }

    fn deployment() -> CompiledMcp {
        use golem_common::model::{
            account::{AccountEmail, AccountId},
            application::ApplicationName,
            deployment::DeploymentRevision,
            environment::{EnvironmentId, EnvironmentName},
        };
        CompiledMcp {
            account_id: AccountId::new(),
            account_email: AccountEmail::new("owner@example.test"),
            application_name: ApplicationName("app".into()),
            environment_id: EnvironmentId::new(),
            environment_name: EnvironmentName("env".into()),
            deployment_revision: DeploymentRevision::INITIAL,
            domain: Domain("example.test".into()),
            security_scheme_name: None,
            security_scheme: None,
            registered_agent_types: vec![],
            tools: vec![],
        }
    }

    #[test]
    async fn unavailable_or_unresolved_security_cannot_reach_mcp_endpoint() {
        let mut unresolved = deployment();
        unresolved.security_scheme_name = Some(
            golem_common::model::security_scheme::SecuritySchemeName("oauth".into()),
        );
        for compiled in [None, Some(unresolved)] {
            let middleware = McpBearerAuth::new(
                Arc::new(Lookup(compiled)),
                Arc::new(crate::custom_api::oidc::DefaultIdentityProvider),
            );
            let endpoint =
                middleware.transform(poem::endpoint::make_sync(|_: Request| -> Response {
                    panic!("unauthorized request reached the MCP endpoint")
                }));
            let response = endpoint
                .call(Request::builder().header("host", "example.test").finish())
                .await
                .unwrap();
            assert_eq!(response.status(), http::StatusCode::SERVICE_UNAVAILABLE);
        }
    }

    #[test]
    async fn public_domain_passes_exact_compiled_deployment_to_endpoint() {
        let compiled = deployment();
        let expected_environment = compiled.environment_id;
        let middleware = McpBearerAuth::new(
            Arc::new(Lookup(Some(compiled))),
            Arc::new(crate::custom_api::oidc::DefaultIdentityProvider),
        );
        let endpoint = middleware.transform(poem::endpoint::make_sync(move |req: Request| {
            let compiled = req.extensions().get::<Arc<CompiledMcp>>().unwrap();
            assert_eq!(compiled.environment_id, expected_environment);
            Response::builder()
                .status(http::StatusCode::NO_CONTENT)
                .finish()
        }));
        assert_eq!(
            endpoint
                .call(Request::builder().header("host", "example.test").finish())
                .await
                .unwrap()
                .status(),
            http::StatusCode::NO_CONTENT
        );
    }

    #[test]
    fn token_cache_key_isolated_by_domain_and_scheme() {
        let base = vec![
            "token".to_string(),
            "one.example".to_string(),
            "scheme-a".to_string(),
        ];
        let other_domain = vec![
            "token".to_string(),
            "two.example".to_string(),
            "scheme-a".to_string(),
        ];
        let updated_scheme = vec![
            "token".to_string(),
            "one.example".to_string(),
            "scheme-b".to_string(),
        ];

        assert_ne!(
            McpBearerAuth::hash_parts(&base),
            McpBearerAuth::hash_parts(&other_domain)
        );
        assert_ne!(
            McpBearerAuth::hash_parts(&base),
            McpBearerAuth::hash_parts(&updated_scheme)
        );
    }

    #[test]
    fn token_cache_key_uses_unambiguous_field_framing() {
        let left = vec!["ab".to_string(), "c".to_string()];
        let right = vec!["a".to_string(), "bc".to_string()];

        assert_ne!(
            McpBearerAuth::hash_parts(&left),
            McpBearerAuth::hash_parts(&right)
        );
    }
}
