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

use super::identity_provider_metadata::GolemIdentityProviderMetadata;
use super::model::AuthorizationUrl;
use super::open_id_client::OpenIdClient;
use async_trait::async_trait;
use golem_common::IntoAnyhow;
use golem_common::model::security_scheme::Provider;
use golem_service_base::custom_api::SecuritySchemeDetails;
use openidconnect::core::{
    CoreClient, CoreIdToken, CoreIdTokenClaims, CoreIdTokenVerifier, CoreProviderMetadata,
    CoreResponseType, CoreTokenResponse,
};
use openidconnect::{
    AuthenticationFlow, AuthorizationCode, CsrfToken, Nonce, OAuth2TokenResponse, Scope,
};
use tracing::debug;

#[derive(Debug, thiserror::Error)]
pub enum IdentityProviderError {
    #[error("Failed to initialize client: {0}")]
    ClientInitError(String),
    #[error("Invalid issuer URL: {0}")]
    InvalidIssuerUrl(String),
    #[error("Failed to discover provider metadata: {0}")]
    FailedToDiscoverProviderMetadata(String),
    #[error("Failed to exchange code for tokens: {0}")]
    FailedToExchangeCodeForTokens(String),
    #[error("ID token verification error: {0}")]
    IdTokenVerificationError(String),
    #[error("ID token exchange for code failed")]
    OidcTokenExchangeFailed,
}

pub struct RawTokenResponse {
    pub id_token: String,
    pub access_token: Option<String>,
    pub refresh_token: Option<String>,
    pub expires_in: Option<u64>,
    pub token_type: String,
}

impl IntoAnyhow for IdentityProviderError {
    fn into_anyhow(self) -> anyhow::Error {
        anyhow::Error::from(self).context("IdentityProviderError")
    }
}

#[async_trait]
pub trait IdentityProvider: Send + Sync {
    /// Exchanges an authorization code for parsed claims and scopes.
    ///
    /// Used by the **HTTP API gateway** OIDC flow where Golem is the relying party:
    /// Golem initiated the OAuth flow itself (with its own nonce), so it can verify
    /// the nonce and parse the id_token into structured claims (subject, email, etc.)
    /// to build an authenticated session.
    async fn exchange_code_for_scopes_and_claims(
        &self,
        security_scheme: &SecuritySchemeDetails,
        code: &AuthorizationCode,
        nonce: &Nonce,
    ) -> Result<(Vec<Scope>, CoreIdTokenClaims), IdentityProviderError>;

    /// Builds the authorization URL that redirects the user to the identity provider.
    ///
    /// Used by both flows:
    /// - **HTTP**: redirects users to Google when they hit a protected route
    /// - **MCP**: redirects users to Google when the MCP client initiates OAuth
    async fn get_authorization_url(
        &self,
        security_scheme: &SecuritySchemeDetails,
        scopes: Vec<Scope>,
        state: CsrfToken,
        nonce: Nonce,
    ) -> Result<AuthorizationUrl, IdentityProviderError>;

    /// Exchanges an authorization code for tokens, returning the raw JWT strings.
    ///
    /// Used by the **MCP OAuth proxy** flow where Golem acts as an intermediary:
    /// the MCP client (Claude Desktop, mcp-remote) cannot talk to Google directly
    /// (no DCR support), so Golem proxies the entire OAuth dance. After exchanging
    /// the code with Google, Golem doesn't need to parse the claims — it just needs
    /// the raw id_token string to pass back to the MCP client as a Bearer token.
    ///
    /// Key differences from `exchange_code_for_scopes_and_claims`:
    /// - Returns raw JWT strings instead of parsed `CoreIdTokenClaims`
    /// - Does not verify a nonce (Golem generated the nonce internally just to
    ///   satisfy the OIDC library; the MCP client never sees it)
    /// - The MCP client will later present this token as `Authorization: Bearer <jwt>`
    ///   and `validate_bearer_token` will verify it at that point
    async fn exchange_code_for_raw_id_token(
        &self,
        security_scheme: &SecuritySchemeDetails,
        code: &AuthorizationCode,
    ) -> Result<RawTokenResponse, IdentityProviderError>;

    /// Validates a Bearer JWT token against the provider's JWKS.
    ///
    /// Used by the **MCP Bearer auth middleware** to verify tokens on incoming requests.
    /// The token may have been obtained through the OAuth proxy flow above, or
    /// directly by the client through their own OAuth flow — either way, we validate
    /// the signature and claims against the provider's published keys.
    ///
    /// Uses a no-op nonce verifier because we didn't necessarily initiate the OAuth
    /// flow that produced this token.
    async fn validate_bearer_token(
        &self,
        security_scheme: &SecuritySchemeDetails,
        token: &str,
    ) -> Result<(), IdentityProviderError>;
}

pub struct DefaultIdentityProvider;

impl DefaultIdentityProvider {
    async fn get_provider_metadata(
        &self,
        provider: &Provider,
    ) -> Result<GolemIdentityProviderMetadata, IdentityProviderError> {
        let http_client = openidconnect::reqwest::Client::new();
        let issuer_url = provider
            .issuer_url()
            .map_err(IdentityProviderError::FailedToDiscoverProviderMetadata)?;
        let provider_metadata =
            CoreProviderMetadata::discover_async(issuer_url, &http_client)
                .await
                .map_err(|err| {
                    IdentityProviderError::FailedToDiscoverProviderMetadata(err.to_string())
                })?;

        Ok(provider_metadata)
    }

    async fn exchange_code_for_tokens(
        &self,
        client: &OpenIdClient,
        code: &AuthorizationCode,
    ) -> Result<CoreTokenResponse, IdentityProviderError> {
        let http_client = openidconnect::reqwest::Client::new();
        let token_response: CoreTokenResponse = client
            .client
            .exchange_code(code.clone())
            .map_err(|err| IdentityProviderError::FailedToExchangeCodeForTokens(err.to_string()))?
            .request_async(&http_client)
            .await
            .map_err(|err| IdentityProviderError::FailedToExchangeCodeForTokens(err.to_string()))?;

        Ok(token_response)
    }

    async fn get_client(
        &self,
        security_scheme: &SecuritySchemeDetails,
    ) -> Result<OpenIdClient, IdentityProviderError> {
        debug!(
            "Creating identity provider client for {}",
            security_scheme.id
        );

        let provider_metadata = self
            .get_provider_metadata(&security_scheme.provider_type)
            .await?;

        let client = CoreClient::from_provider_metadata(
            provider_metadata,
            security_scheme.client_id.clone(),
            Some(security_scheme.client_secret.clone()),
        )
        .set_redirect_uri(security_scheme.redirect_url.clone());

        Ok(OpenIdClient { client })
    }

    fn get_id_token_verifier<'a>(&self, client: &'a OpenIdClient) -> CoreIdTokenVerifier<'a> {
        client.client.id_token_verifier()
    }

    fn get_claims(
        &self,
        id_token_verifier: &CoreIdTokenVerifier,
        core_token_response: &CoreTokenResponse,
        nonce: &Nonce,
    ) -> Result<CoreIdTokenClaims, IdentityProviderError> {
        let id_token_claims: &CoreIdTokenClaims = core_token_response
            .extra_fields()
            .id_token()
            .ok_or(IdentityProviderError::IdTokenVerificationError(
                "Failed to get ID token".to_string(),
            ))?
            .claims(id_token_verifier, nonce)
            .map_err(|err| IdentityProviderError::IdTokenVerificationError(err.to_string()))?;

        Ok(id_token_claims.clone())
    }
}

fn noop_nonce_verifier(_nonce: Option<&Nonce>) -> Result<(), String> {
    Ok(())
}

#[async_trait]
impl IdentityProvider for DefaultIdentityProvider {
    async fn exchange_code_for_scopes_and_claims(
        &self,
        security_scheme: &SecuritySchemeDetails,
        code: &AuthorizationCode,
        nonce: &Nonce,
    ) -> Result<(Vec<Scope>, CoreIdTokenClaims), IdentityProviderError> {
        let client = self.get_client(security_scheme).await?;

        let token_response = self
            .exchange_code_for_tokens(&client, code)
            .await
            .map_err(|err| {
                tracing::warn!("OIDC token exchange failed: {err}");
                IdentityProviderError::OidcTokenExchangeFailed
            })?;

        let id_token_scopes = token_response.scopes().cloned().unwrap_or_default();

        let id_token_verifier = self.get_id_token_verifier(&client);
        let id_token_claims = self.get_claims(&id_token_verifier, &token_response, nonce)?;

        Ok((id_token_scopes, id_token_claims))
    }

    async fn get_authorization_url(
        &self,
        security_scheme: &SecuritySchemeDetails,
        scopes: Vec<Scope>,
        state: CsrfToken,
        nonce: Nonce,
    ) -> Result<AuthorizationUrl, IdentityProviderError> {
        let client = self.get_client(security_scheme).await?;

        let builder = client.client.authorize_url(
            AuthenticationFlow::<CoreResponseType>::AuthorizationCode,
            || state,
            || nonce,
        );

        let builder = scopes
            .iter()
            .fold(builder, |builder, scope| builder.add_scope(scope.clone()));

        let (auth_url, csrf_state, nonce) = builder.url();

        Ok(AuthorizationUrl {
            url: auth_url,
            csrf_state,
            nonce,
        })
    }

    async fn exchange_code_for_raw_id_token(
        &self,
        security_scheme: &SecuritySchemeDetails,
        code: &AuthorizationCode,
    ) -> Result<RawTokenResponse, IdentityProviderError> {
        let client = self.get_client(security_scheme).await?;

        let token_response = self
            .exchange_code_for_tokens(&client, code)
            .await
            .map_err(|err| {
                tracing::warn!("OIDC token exchange failed: {err}");
                IdentityProviderError::OidcTokenExchangeFailed
            })?;

        let id_token_string = match token_response.extra_fields().id_token() {
            Some(id_token) => id_token.to_string(),
            None => token_response.access_token().secret().clone(),
        };

        let access_token = Some(token_response.access_token().secret().clone());
        let refresh_token = token_response.refresh_token().map(|t| t.secret().clone());
        let expires_in = token_response.expires_in().map(|d| d.as_secs());
        let token_type = token_response.token_type().as_ref().to_string();

        Ok(RawTokenResponse {
            id_token: id_token_string,
            access_token,
            refresh_token,
            expires_in,
            token_type,
        })
    }

    async fn validate_bearer_token(
        &self,
        security_scheme: &SecuritySchemeDetails,
        token: &str,
    ) -> Result<(), IdentityProviderError> {
        let provider_metadata = self
            .get_provider_metadata(&security_scheme.provider_type)
            .await?;

        let client = CoreClient::from_provider_metadata(
            provider_metadata,
            security_scheme.client_id.clone(),
            Some(security_scheme.client_secret.clone()),
        );

        let verifier = client.id_token_verifier();

        let id_token: CoreIdToken = serde_json::from_value(serde_json::Value::String(
            token.to_string(),
        ))
        .map_err(|err| {
            IdentityProviderError::IdTokenVerificationError(format!("Failed to parse token: {err}"))
        })?;

        let _claims = id_token
            .into_claims(&verifier, noop_nonce_verifier)
            .map_err(|err| IdentityProviderError::IdTokenVerificationError(err.to_string()))?;

        Ok(())
    }
}
