use async_trait::async_trait;
use golem_client::apis::configuration::Configuration;
use golem_client::apis::login_api::{
    login_oauth2_device_complete_post, login_oauth2_device_start_post, v2_login_token_get,
};
use golem_client::models::{OAuth2Data, Token, TokenSecret, UnsafeToken};
use tracing::info;

use crate::model::GolemError;

#[async_trait]
pub trait LoginClient {
    async fn token_details(&self, manual_token: TokenSecret) -> Result<Token, GolemError>;

    async fn start_oauth2(&self) -> Result<OAuth2Data, GolemError>;

    async fn complete_oauth2(&self, session: String) -> Result<UnsafeToken, GolemError>;
}

pub struct LoginClientLive {
    pub configuration: Configuration,
}

#[async_trait]
impl LoginClient for LoginClientLive {
    async fn token_details(&self, manual_token: TokenSecret) -> Result<Token, GolemError> {
        info!("Getting token info");
        let mut config = self.configuration.clone();
        config.bearer_access_token = Some(manual_token.value.to_string());
        Ok(v2_login_token_get(&config).await?)
    }

    async fn start_oauth2(&self) -> Result<OAuth2Data, GolemError> {
        info!("Start OAuth2 workflow");
        Ok(login_oauth2_device_start_post(&self.configuration).await?)
    }

    async fn complete_oauth2(&self, session: String) -> Result<UnsafeToken, GolemError> {
        info!("Complete OAuth2 workflow");
        Ok(login_oauth2_device_complete_post(&self.configuration, &session).await?)
    }
}
