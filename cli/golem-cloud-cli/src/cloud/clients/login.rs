use crate::cloud::clients::errors::CloudGolemError;
use async_trait::async_trait;
use golem_cloud_client::api::LoginClient as HttpClient;
use golem_cloud_client::model::{OAuth2Data, Token, TokenSecret, UnsafeToken};
use golem_cloud_client::{Context, Security};
use tracing::info;

#[async_trait]
pub trait LoginClient {
    async fn token_details(&self, manual_token: TokenSecret) -> Result<Token, CloudGolemError>;

    async fn start_oauth2(&self) -> Result<OAuth2Data, CloudGolemError>;

    async fn complete_oauth2(&self, session: String) -> Result<UnsafeToken, CloudGolemError>;
}

pub struct LoginClientLive<C: HttpClient + Sync + Send> {
    pub client: C,
    pub context: Context,
}

#[async_trait]
impl<C: HttpClient + Sync + Send> LoginClient for LoginClientLive<C> {
    async fn token_details(&self, manual_token: TokenSecret) -> Result<Token, CloudGolemError> {
        info!("Getting token info");
        let mut context = self.context.clone();
        context.security_token = Security::Bearer(manual_token.value.to_string());

        let client = golem_cloud_client::api::LoginClientLive { context };

        Ok(client.v_2_login_token_get().await?)
    }

    async fn start_oauth2(&self) -> Result<OAuth2Data, CloudGolemError> {
        info!("Start OAuth2 workflow");
        Ok(self.client.login_oauth_2_device_start_post().await?)
    }

    async fn complete_oauth2(&self, session: String) -> Result<UnsafeToken, CloudGolemError> {
        info!("Complete OAuth2 workflow");
        Ok(self
            .client
            .login_oauth_2_device_complete_post(&session)
            .await?)
    }
}
