use openidconnect::core::{CoreClient};

#[derive(Clone)]
pub struct OpenIdClient {
    pub client: CoreClient,
}

impl OpenIdClient {
    pub fn new(client: CoreClient) -> Self {
        OpenIdClient { client }
    }
}

