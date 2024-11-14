use openidconnect::core::{CoreClient};

#[derive(Clone, Debug)]
pub struct OpenIdClient {
    pub client: CoreClient,
}

impl OpenIdClient {
    pub fn new(client: CoreClient) -> Self {
        OpenIdClient { client }
    }
}

