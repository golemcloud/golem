use crate::gateway_security::identity_provider::IdentityProviderError;
use openidconnect::core::{CoreClient, CoreIdTokenClaims, CoreIdTokenVerifier, CoreTokenResponse};
use openidconnect::Nonce;

#[derive(Clone, Debug)]
pub struct OpenIdClient {
    pub client: CoreClient,
}

impl OpenIdClient {
    pub fn new(client: CoreClient) -> Self {
        OpenIdClient { client }
    }

    pub fn id_token_verifier(&self) -> CoreIdTokenVerifier {
        self.client.id_token_verifier()
    }
}
