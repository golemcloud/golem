use crate::gateway_security::default_provider::DefaultIdentityProvider;
use crate::gateway_security::identity_provider::{
    AuthorizationUrl, IdentityProvider, IdentityProviderError,
};
use crate::gateway_security::open_id_client::OpenIdClient;
use crate::gateway_security::{GolemIdentityProviderMetadata, SecuritySchemeWithProviderMetadata};
use async_trait::async_trait;
use openidconnect::core::{CoreIdTokenClaims, CoreTokenResponse};
use openidconnect::{AuthorizationCode, IssuerUrl, Nonce, Scope};

pub struct GoogleIdentityProvider {
    default_provider: DefaultIdentityProvider,
}

#[async_trait]
impl IdentityProvider for GoogleIdentityProvider {
    async fn get_provider_metadata(
        &self,
        issuer_url: &IssuerUrl,
    ) -> Result<GolemIdentityProviderMetadata, IdentityProviderError> {
        self.default_provider
            .get_provider_metadata(issuer_url)
            .await
    }

    async fn exchange_code_for_tokens(
        &self,
        client: &OpenIdClient,
        code: &AuthorizationCode,
    ) -> Result<CoreTokenResponse, IdentityProviderError> {
        self.default_provider
            .exchange_code_for_tokens(client, code)
            .await
    }

    fn get_client(
        &self,
        security_scheme: &SecuritySchemeWithProviderMetadata,
    ) -> Result<OpenIdClient, IdentityProviderError> {
        self.default_provider.get_client(security_scheme)
    }

    fn get_claims(
        &self,
        client: &OpenIdClient,
        core_token_response: CoreTokenResponse,
        nonce: &Nonce,
    ) -> Result<CoreIdTokenClaims, IdentityProviderError> {
        self.default_provider
            .get_claims(client, core_token_response, nonce)
    }

    fn get_authorization_url(
        &self,
        client: &OpenIdClient,
        scopes: Vec<Scope>,
    ) -> Result<AuthorizationUrl, IdentityProviderError> {
        self.default_provider.get_authorization_url(client, scopes)
    }
}
