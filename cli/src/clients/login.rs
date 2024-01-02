use async_trait::async_trait;
use golem_client::api::LoginClient as HttpClient;
use golem_client::model::OAuth2Data;
use golem_client::model::Token;
use golem_client::model::TokenSecret;
use golem_client::model::UnsafeToken;
use golem_client::{Context, Security};
use tracing::info;

use crate::model::GolemError;

#[async_trait]
pub trait LoginClient {
    async fn token_details(&self, manual_token: TokenSecret) -> Result<Token, GolemError>;

    async fn start_oauth2(&self) -> Result<OAuth2Data, GolemError>;

    async fn complete_oauth2(&self, session: String) -> Result<UnsafeToken, GolemError>;
}

pub struct LoginClientLive<C: HttpClient + Sync + Send> {
    pub client: C,
    pub context: Context,
}

#[async_trait]
impl<C: HttpClient + Sync + Send> LoginClient for LoginClientLive<C> {
    async fn token_details(&self, manual_token: TokenSecret) -> Result<Token, GolemError> {
        info!("Getting token info");
        let mut context = self.context.clone();
        context.security_token = Security::Bearer(manual_token.value.to_string());

        let client = golem_client::api::LoginClientLive { context };

        Ok(client.v_2_login_token_get().await?)
    }

    async fn start_oauth2(&self) -> Result<OAuth2Data, GolemError> {
        info!("Start OAuth2 workflow");
        Ok(self.client.login_oauth_2_device_start_post().await?)
    }

    async fn complete_oauth2(&self, session: String) -> Result<UnsafeToken, GolemError> {
        info!("Complete OAuth2 workflow");
        Ok(self
            .client
            .login_oauth_2_device_complete_post(&session)
            .await?)
    }
}
