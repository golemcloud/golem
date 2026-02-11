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

use super::identity_provider_metadata::GolemIdentityProviderMetadata;
use super::model::AuthorizationUrl;
use super::open_id_client::OpenIdClient;
use async_trait::async_trait;
use golem_common::IntoAnyhow;
use golem_common::model::security_scheme::Provider;
use golem_service_base::custom_api::SecuritySchemeDetails;
use openidconnect::core::{
    CoreClient, CoreIdTokenClaims, CoreIdTokenVerifier, CoreProviderMetadata, CoreResponseType,
    CoreTokenResponse,
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

impl IntoAnyhow for IdentityProviderError {
    fn into_anyhow(self) -> anyhow::Error {
        anyhow::Error::from(self).context("IdentityProviderError")
    }
}

#[async_trait]
pub trait IdentityProvider: Send + Sync {
    // exchange code for token + get claims
    async fn exchange_code_for_scopes_and_claims(
        &self,
        security_scheme: &SecuritySchemeDetails,
        code: &AuthorizationCode,
        nonce: &Nonce,
    ) -> Result<(Vec<Scope>, CoreIdTokenClaims), IdentityProviderError>;

    async fn get_authorization_url(
        &self,
        security_scheme: &SecuritySchemeDetails,
        scopes: Vec<Scope>,
        state: CsrfToken,
        nonce: Nonce,
    ) -> Result<AuthorizationUrl, IdentityProviderError>;
}

pub struct DefaultIdentityProvider;

impl DefaultIdentityProvider {
    async fn get_provider_metadata(
        &self,
        provider: &Provider,
    ) -> Result<GolemIdentityProviderMetadata, IdentityProviderError> {
        let provider_metadata = CoreProviderMetadata::discover_async(
            provider.issuer_url(),
            openidconnect::reqwest::async_http_client,
        )
        .await
        .map_err(|err| IdentityProviderError::FailedToDiscoverProviderMetadata(err.to_string()))?;

        Ok(provider_metadata)
    }

    async fn exchange_code_for_tokens(
        &self,
        client: &OpenIdClient,
        code: &AuthorizationCode,
    ) -> Result<CoreTokenResponse, IdentityProviderError> {
        let token_response = client
            .client
            .exchange_code(code.clone())
            .request_async(openidconnect::reqwest::async_http_client)
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
}
