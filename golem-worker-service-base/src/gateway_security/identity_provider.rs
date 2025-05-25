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

use crate::gateway_security::open_id_client::OpenIdClient;
use crate::gateway_security::{GolemIdentityProviderMetadata, Provider, SecurityScheme};
use async_trait::async_trait;
use golem_common::SafeDisplay;
use openidconnect::core::{CoreIdTokenClaims, CoreIdTokenVerifier, CoreTokenResponse};
use openidconnect::{AuthorizationCode, CsrfToken, Nonce, Scope};
use std::fmt::{Display, Formatter};
use url::Url;

// A high level abstraction of an identity-provider, that expose
// necessary functionalities that gets called at various points in gateway security integration.
#[async_trait]
pub trait IdentityProvider {
    // Fetches the provider metadata from the issuer url, and this must be called
    // during the registration of the security scheme with golem.
    // The security scheme regisration stores the provider metadata, along with the security scheme
    // in the security scheme store of Golem
    async fn get_provider_metadata(
        &self,
        provider: &Provider,
    ) -> Result<GolemIdentityProviderMetadata, IdentityProviderError>;

    // Exchange of Code token happens during the auth_call_back phase of the OpenID workflow
    // In other words, this gets called only during the execution of static binding backing auth_call_back endpoint.
    async fn exchange_code_for_tokens(
        &self,
        client: &OpenIdClient,
        code: &AuthorizationCode,
    ) -> Result<CoreTokenResponse, IdentityProviderError>;

    // A client can be created given provider-metadata at any phase of the security workflow in API Gateway.
    // It can be created to create the authorisation URL to redirect user to the provider's login page
    // Or It can be created before exchange of token during the execution of static binding backing auth_call_back endpoint.
    async fn get_client(
        &self,
        security_scheme: &SecurityScheme,
    ) -> Result<OpenIdClient, IdentityProviderError>;

    // Get IDToken verifier
    // For the most part, this is an internal detail to openidconnect, however,
    // to test verifying claims using our own key pairs, this can be exposed
    fn get_id_token_verifier<'a>(&self, client: &'a OpenIdClient) -> CoreIdTokenVerifier<'a>;

    // Claims are fetched from the ID token, and this gets called during the execution of static binding backing auth_call_back endpoint.
    // If needed this can be called just before serving the protected route, to fetch the claims from the ID token as a middleware
    // and feed it to the protected route handler through Rib. In any case, claims needs to be stored in a session
    // as the OAuth2 workflow in OpenID gets initiated by the gateway and not the client user-agent.
    fn get_claims(
        &self,
        client: &CoreIdTokenVerifier,
        core_token_response: CoreTokenResponse,
        nonce: &Nonce,
    ) -> Result<CoreIdTokenClaims, IdentityProviderError>;

    // This gets called during the redirect to the provider's login page,
    // and this is the first step in the OAuth2 workflow in serving a protected route.
    fn get_authorization_url(
        &self,
        client: &OpenIdClient,
        scopes: Vec<Scope>,
        state: Option<CsrfToken>,
        nonce: Option<Nonce>,
    ) -> AuthorizationUrl;
}

pub struct AuthorizationUrl {
    pub url: Url,
    pub csrf_state: CsrfToken,
    pub nonce: Nonce,
}

#[derive(Debug, Clone)]
pub enum IdentityProviderError {
    ClientInitError(String),
    InvalidIssuerUrl(String),
    FailedToDiscoverProviderMetadata(String),
    FailedToExchangeCodeForTokens(String),
    IdTokenVerificationError(String),
}

// To satisfy thiserror
// https://github.com/golemcloud/golem/issues/1071
impl Display for IdentityProviderError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.to_safe_string())
    }
}

impl SafeDisplay for IdentityProviderError {
    fn to_safe_string(&self) -> String {
        match self {
            IdentityProviderError::ClientInitError(err) => format!("ClientInitError: {}", err),
            IdentityProviderError::InvalidIssuerUrl(err) => format!("InvalidIssuerUrl: {}", err),
            IdentityProviderError::FailedToDiscoverProviderMetadata(err) => {
                format!("FailedToDiscoverProviderMetadata: {}", err)
            }
            IdentityProviderError::FailedToExchangeCodeForTokens(err) => {
                format!("FailedToExchangeCodeForTokens: {}", err)
            }
            IdentityProviderError::IdTokenVerificationError(err) => {
                format!("IdTokenVerificationError: {}", err)
            }
        }
    }
}
