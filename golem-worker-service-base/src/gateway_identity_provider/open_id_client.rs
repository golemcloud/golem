use openidconnect::core::{CoreClient, CoreIdTokenClaims, CoreIdTokenVerifier, CoreTokenResponse};
use openidconnect::Nonce;
use crate::gateway_identity_provider::identity_provider::IdentityProviderError;

#[derive(Clone, Debug)]
pub struct OpenIdClient {
    pub client: CoreClient,
}

impl OpenIdClient {
    pub fn new(client: CoreClient) -> Self {
        OpenIdClient { client }
    }
}

