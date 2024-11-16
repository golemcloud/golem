use openidconnect::core::{CoreClient, CoreIdTokenVerifier};

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
