// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use crate::gateway_security::open_id_client::OpenIdClient;
use crate::gateway_security::*;
use async_trait::async_trait;
use openidconnect::core::{
    CoreClient, CoreIdTokenClaims, CoreIdTokenVerifier, CoreProviderMetadata, CoreResponseType,
    CoreTokenResponse,
};
use openidconnect::{AuthenticationFlow, AuthorizationCode, CsrfToken, Nonce, Scope};
use tracing::debug;

// All providers can reuse DefaultIdentityProvider if provided internally
pub struct DefaultIdentityProvider;

#[async_trait]
impl IdentityProvider for DefaultIdentityProvider {
    // To be called during API definition registration to then store them in the database
    async fn get_provider_metadata(
        &self,
        provider: &Provider,
    ) -> Result<GolemIdentityProviderMetadata, IdentityProviderError> {
        let issue_url = provider.issue_url().map_err(|err| {
            IdentityProviderError::FailedToDiscoverProviderMetadata(err.to_string())
        })?;

        let provide_metadata = CoreProviderMetadata::discover_async(
            issue_url,
            openidconnect::reqwest::async_http_client,
        )
        .await
        .map_err(|err| IdentityProviderError::FailedToDiscoverProviderMetadata(err.to_string()))?;

        Ok(provide_metadata)
    }

    // To be called during call_back authentication URL which is a injected URL
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
        security_scheme: &SecurityScheme,
    ) -> Result<OpenIdClient, IdentityProviderError> {
        debug!(
            "Creating identity provider client for {}",
            security_scheme.scheme_identifier()
        );

        let provider_metadata = self
            .get_provider_metadata(&security_scheme.provider_type())
            .await?;

        let client = CoreClient::from_provider_metadata(
            provider_metadata,
            security_scheme.client_id().clone(),
            Some(security_scheme.client_secret().clone()),
        )
        .set_redirect_uri(security_scheme.redirect_url());

        Ok(OpenIdClient { client })
    }

    fn get_id_token_verifier<'a>(&self, client: &'a OpenIdClient) -> CoreIdTokenVerifier<'a> {
        client.client.id_token_verifier()
    }

    fn get_claims(
        &self,
        id_token_verifier: &CoreIdTokenVerifier,
        core_token_response: CoreTokenResponse,
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

    fn get_authorization_url(
        &self,
        client: &OpenIdClient,
        scopes: Vec<Scope>,
        state: Option<CsrfToken>,
        nonce: Option<Nonce>,
    ) -> AuthorizationUrl {
        let state = || state.unwrap_or_else(CsrfToken::new_random);
        let nonce = || nonce.unwrap_or_else(Nonce::new_random);

        let builder = client.client.authorize_url(
            AuthenticationFlow::<CoreResponseType>::AuthorizationCode,
            state,
            nonce,
        );

        let builder = scopes
            .iter()
            .fold(builder, |builder, scope| builder.add_scope(scope.clone()));

        let (auth_url, csrf_state, nonce) = builder.url();

        AuthorizationUrl {
            url: auth_url,
            csrf_state,
            nonce,
        }
    }
}
