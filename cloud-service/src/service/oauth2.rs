use std::sync::Arc;

use async_trait::async_trait;

use crate::model::{
    EncodedOAuth2Session, OAuth2AccessToken, OAuth2Data, OAuth2Provider, OAuth2Session,
};
use crate::service::oauth2_github_client::{OAuth2GithubClient, OAuth2GithubClientError};
use crate::service::oauth2_session::{OAuth2SessionError, OAuth2SessionService};

#[derive(Debug, thiserror::Error)]
pub enum OAuth2Error {
    #[error("Invalid Session: {0}")]
    InvalidSession(String),
    #[error("Invalid State: {0}")]
    InvalidState(String),
    #[error("Internal error: {0}")]
    Internal(#[from] anyhow::Error),
}

impl From<OAuth2GithubClientError> for OAuth2Error {
    fn from(err: OAuth2GithubClientError) -> Self {
        match err {
            OAuth2GithubClientError::Unexpected(error) => OAuth2Error::Internal(error),
        }
    }
}

impl From<OAuth2SessionError> for OAuth2Error {
    fn from(err: OAuth2SessionError) -> Self {
        match err {
            OAuth2SessionError::Internal(error) => OAuth2Error::Internal(error),
            OAuth2SessionError::InvalidSession(msg) => OAuth2Error::InvalidSession(msg),
        }
    }
}

#[derive(Debug)]
pub struct UrlWithState {
    pub url: String,
    pub state: String,
}

#[async_trait]
pub trait OAuth2Service {
    async fn start_workflow(&self) -> Result<OAuth2Data, OAuth2Error>;
    async fn finish_workflow(
        &self,
        encoded_session: &EncodedOAuth2Session,
    ) -> Result<OAuth2AccessToken, OAuth2Error>;

    async fn get_authorize_url(
        &self,
        provider: OAuth2Provider,
        state: &str,
    ) -> Result<String, OAuth2Error>;

    async fn exchange_code_for_token(
        &self,
        provider: OAuth2Provider,
        code: &str,
        state: &str,
    ) -> Result<String, OAuth2Error>;
}

pub struct OAuth2ServiceDefault {
    client: Arc<dyn OAuth2GithubClient + Send + Sync>,
    session_service: Arc<dyn OAuth2SessionService + Send + Sync>,
}

impl OAuth2ServiceDefault {
    pub fn new(
        client: Arc<dyn OAuth2GithubClient + Send + Sync>,
        session_service: Arc<dyn OAuth2SessionService + Send + Sync>,
    ) -> OAuth2ServiceDefault {
        OAuth2ServiceDefault {
            client,
            session_service,
        }
    }
}

#[async_trait]
impl OAuth2Service for OAuth2ServiceDefault {
    async fn start_workflow(&self) -> Result<OAuth2Data, OAuth2Error> {
        let data = self.client.initiate_device_workflow().await?;
        let now = chrono::Utc::now();
        let session = OAuth2Session {
            device_code: data.device_code,
            interval: data.interval,
            expires_at: now + data.expires_in,
        };
        let encoded_session = self.session_service.encode_session(&session)?;
        Ok(OAuth2Data {
            url: data.verification_uri,
            user_code: data.user_code,
            expires: session.expires_at,
            encoded_session: encoded_session.value,
        })
    }

    async fn finish_workflow(
        &self,
        encoded_session: &EncodedOAuth2Session,
    ) -> Result<OAuth2AccessToken, OAuth2Error> {
        let session = self.session_service.decode_session(encoded_session)?;
        let access_token = self
            .client
            .get_access_token(&session.device_code, session.interval, session.expires_at)
            .await?;
        Ok(OAuth2AccessToken {
            provider: OAuth2Provider::Github,
            access_token,
        })
    }

    async fn get_authorize_url(
        &self,
        provider: OAuth2Provider,
        state: &str,
    ) -> Result<String, OAuth2Error> {
        match provider {
            OAuth2Provider::Github => {
                let url = self.client.get_authorize_url(state).await;
                Ok(url)
            }
        }
    }

    async fn exchange_code_for_token(
        &self,
        provider: OAuth2Provider,
        code: &str,
        state: &str,
    ) -> Result<String, OAuth2Error> {
        match provider {
            OAuth2Provider::Github => Ok(self.client.exchange_code_for_token(code, state).await?),
        }
    }
}

#[derive(Default)]
pub struct OAuth2ServiceNoOp {}

#[async_trait]
impl OAuth2Service for OAuth2ServiceNoOp {
    async fn start_workflow(&self) -> Result<OAuth2Data, OAuth2Error> {
        Ok(OAuth2Data {
            url: String::new(),
            user_code: String::new(),
            expires: chrono::Utc::now(),
            encoded_session: String::new(),
        })
    }

    async fn finish_workflow(
        &self,
        _encoded_session: &EncodedOAuth2Session,
    ) -> Result<OAuth2AccessToken, OAuth2Error> {
        Ok(OAuth2AccessToken {
            provider: OAuth2Provider::Github,
            access_token: String::new(),
        })
    }

    async fn get_authorize_url(
        &self,
        _provider: OAuth2Provider,
        _state: &str,
    ) -> Result<String, OAuth2Error> {
        Ok(String::new())
    }

    async fn exchange_code_for_token(
        &self,
        _provider: OAuth2Provider,
        _code: &str,
        _state: &str,
    ) -> Result<String, OAuth2Error> {
        Ok(String::new())
    }
}
