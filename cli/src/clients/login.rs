use golem_client::login::Login;
use golem_client::model::{TokenSecret, Token, OAuth2Data, UnsafeToken};
use async_trait::async_trait;
use crate::clients::token_header;
use crate::model::GolemError;
use tracing::info;

#[async_trait]
pub trait LoginClient {
    async fn token_details(&self, manual_token: TokenSecret) -> Result<Token, GolemError>;

    async fn start_oauth2(&self) -> Result<OAuth2Data, GolemError>;

    async fn complete_oauth2(&self, session: String) -> Result<UnsafeToken, GolemError>;
}

pub struct LoginClientLive<L: Login + Send + Sync> {
    pub login: L,
}

#[async_trait]
impl<L: Login + Send + Sync> LoginClient for LoginClientLive<L> {
    async fn token_details(&self, manual_token: TokenSecret) -> Result<Token, GolemError> {
        info!("Getting token info");
        Ok(self.login.current_token(&token_header(&manual_token)).await.map_err(|e| e.to_login_endpoint_error())?)
    }

    async fn start_oauth2(&self) -> Result<OAuth2Data, GolemError> {
        info!("Start OAuth2 workflow");
        Ok(self.login.start_o_auth2().await.map_err(|e| e.to_login_endpoint_error())?)
    }

    async fn complete_oauth2(&self, session: String) -> Result<UnsafeToken, GolemError> {
        info!("Complete OAuth2 workflow");
        Ok(self.login.complete_o_auth2(session).await.map_err(|e| e.to_login_endpoint_error())?)
    }
}