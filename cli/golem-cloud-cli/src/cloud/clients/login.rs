use crate::cloud::clients::errors::CloudGolemError;
use async_trait::async_trait;
use golem_cloud_client::api::{LoginClient as HttpClient, LoginOauth2WebFlowPollError};
use golem_cloud_client::model::{Token, TokenSecret, UnsafeToken, WebFlowAuthorizeUrlResponse};
use golem_cloud_client::{Context, Security};
use tracing::info;

#[async_trait]
pub trait LoginClient {
    async fn token_details(&self, manual_token: TokenSecret) -> Result<Token, CloudGolemError>;

    async fn start_oauth2(&self) -> Result<WebFlowAuthorizeUrlResponse, CloudGolemError>;

    async fn complete_oauth2(&self, state: String) -> Result<UnsafeToken, CloudGolemError>;
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

        Ok(client.current_login_token().await?)
    }

    async fn start_oauth2(&self) -> Result<WebFlowAuthorizeUrlResponse, CloudGolemError> {
        info!("Start OAuth2 workflow");
        Ok(self
            .client
            .oauth_2_web_flow_start("github", Some("https://golem.cloud"))
            .await?)
    }

    async fn complete_oauth2(&self, state: String) -> Result<UnsafeToken, CloudGolemError> {
        use tokio::time::{sleep, Duration};

        info!("Complete OAuth2 workflow");
        let mut attempts = 0;
        let max_attempts = 30;
        let delay = Duration::from_secs(1);

        loop {
            let status = self.client.oauth_2_web_flow_poll(&state).await;
            match status {
                Ok(token) => return Ok(token),
                Err(e) => match e {
                    golem_cloud_client::Error::Item(LoginOauth2WebFlowPollError::Error202(_)) => {
                        attempts += 1;
                        if attempts >= max_attempts {
                            return Err(CloudGolemError("OAuth2 workflow timeout".to_string()));
                        }

                        sleep(delay).await;
                    }
                    _ => {
                        return Err(e.into());
                    }
                },
            }
        }
    }
}
