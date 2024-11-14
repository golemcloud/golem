use async_trait::async_trait;
use openidconnect::{AuthorizationCode, Nonce, Scope};
use openidconnect::core::{CoreIdTokenClaims, CoreTokenResponse};
use crate::gateway_identity_provider::default_provider::DefaultIdentityProvider;
use crate::gateway_identity_provider::identity_provider::{AuthorizationUrl, IdentityProvider, IdentityProviderError};
use crate::gateway_identity_provider::open_id_client::OpenIdClient;
use crate::gateway_identity_provider::security_scheme::SecurityScheme;

pub struct GoogleIdentityProvider {
    default_provider: DefaultIdentityProvider
}

#[async_trait]
impl IdentityProvider for GoogleIdentityProvider {

    async fn get_client(&self, security_scheme: &SecurityScheme) -> Result<OpenIdClient, IdentityProviderError> {
        self.default_provider.get_client(security_scheme).await
    }

    async fn exchange_code_for_tokens(&self, client: &OpenIdClient, code: &AuthorizationCode) -> Result<CoreTokenResponse, IdentityProviderError> {
        self.default_provider.exchange_code_for_tokens(client, code).await
    }

    fn get_claims(&self, client: &OpenIdClient, core_token_response: CoreTokenResponse, nonce: &Nonce) -> Result<CoreIdTokenClaims, IdentityProviderError> {
        self.default_provider.get_claims(client, core_token_response, nonce)
    }

    fn get_authorization_url(&self, client: &OpenIdClient, scopes: Vec<Scope>) -> Result<AuthorizationUrl, IdentityProviderError> {
        self.default_provider.get_authorization_url(client, scopes)
    }
}