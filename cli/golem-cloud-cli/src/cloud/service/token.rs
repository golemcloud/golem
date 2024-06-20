use crate::cloud::clients::token::TokenClient;
use crate::cloud::model::text::{TokenVecView, UnsafeTokenView};
use crate::cloud::model::TokenId;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use golem_cli::cloud::AccountId;
use golem_cli::model::{GolemError, GolemResult};

#[async_trait]
pub trait TokenService {
    async fn list(&self, account_id: Option<AccountId>) -> Result<GolemResult, GolemError>;
    async fn add(
        &self,
        expires_at: DateTime<Utc>,
        account_id: Option<AccountId>,
    ) -> Result<GolemResult, GolemError>;
    async fn delete(
        &self,
        token_id: TokenId,
        account_id: Option<AccountId>,
    ) -> Result<GolemResult, GolemError>;
}

pub struct TokenServiceLive {
    pub account_id: AccountId,
    pub client: Box<dyn TokenClient + Send + Sync>,
}

#[async_trait]
impl TokenService for TokenServiceLive {
    async fn list(&self, account_id: Option<AccountId>) -> Result<GolemResult, GolemError> {
        let account_id = account_id.as_ref().unwrap_or(&self.account_id);
        let token = self.client.get_all(account_id).await?;
        Ok(GolemResult::Ok(Box::new(TokenVecView(token))))
    }

    async fn add(
        &self,
        expires_at: DateTime<Utc>,
        account_id: Option<AccountId>,
    ) -> Result<GolemResult, GolemError> {
        let account_id = account_id.as_ref().unwrap_or(&self.account_id);
        let token = self.client.post(account_id, expires_at).await?;
        Ok(GolemResult::Ok(Box::new(UnsafeTokenView(token))))
    }

    async fn delete(
        &self,
        token_id: TokenId,
        account_id: Option<AccountId>,
    ) -> Result<GolemResult, GolemError> {
        let account_id = account_id.as_ref().unwrap_or(&self.account_id);
        self.client.delete(account_id, token_id).await?;
        Ok(GolemResult::Str("Deleted".to_string()))
    }
}
