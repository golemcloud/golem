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
use openidconnect::core::{CoreClient, CoreProviderMetadata};
use openidconnect::Nonce;
use poem::http;
use poem::{Endpoint, IntoResponse, Middleware, Request, Response, Result};
use std::sync::Arc;

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
                    return Ok(unauthorized_response(&scheme));
                }
            };

            if let Err(err) = validate_bearer_token(&token, &scheme).await {
                tracing::warn!("MCP Bearer token validation failed: {err}");
                return Ok(unauthorized_response(&scheme));
            }
        }

        self.inner
            .call(req)
            .await
            .map(|resp| resp.into_response())
    }
}

fn unauthorized_response(scheme: &SecuritySchemeDetails) -> Response {
    let issuer_url = scheme.provider_type.issuer_url();

    Response::builder()
        .status(http::StatusCode::UNAUTHORIZED)
        .header(
            "WWW-Authenticate",
            format!(
                "Bearer realm=\"mcp\", resource_metadata=\"{}/.well-known/oauth-protected-resource\"",
                issuer_url.as_str().trim_end_matches('/')
            ),
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
    let id_token: openidconnect::core::CoreIdToken = serde_json::from_value(
        serde_json::Value::String(token.to_string()),
    )
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
