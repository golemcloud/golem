use crate::clients::token_header;
use crate::model::GolemError;
use async_trait::async_trait;
use golem_client::login::Login;
use golem_client::model::{OAuth2Data, Token, TokenSecret, UnsafeToken};
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
        Ok(self
            .login
            .current_token(&token_header(&manual_token))
            .await?)
    }

    async fn start_oauth2(&self) -> Result<OAuth2Data, GolemError> {
        info!("Start OAuth2 workflow");
        Ok(self.login.start_o_auth2().await?)
    }

    async fn complete_oauth2(&self, session: String) -> Result<UnsafeToken, GolemError> {
        info!("Complete OAuth2 workflow");
        Ok(self.login.complete_o_auth2(session).await?)
    }
}
