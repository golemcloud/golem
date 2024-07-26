use crate::cloud::clients::errors::CloudGolemError;
use crate::cloud::model::TokenId;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use golem_cli::cloud::AccountId;
use golem_cloud_client::model::{CreateTokenDto, Token, UnsafeToken};
use tracing::info;

#[async_trait]
pub trait TokenClient {
    async fn get_all(&self, account_id: &AccountId) -> Result<Vec<Token>, CloudGolemError>;
    async fn get(&self, account_id: &AccountId, id: TokenId) -> Result<Token, CloudGolemError>;
    async fn post(
        &self,
        account_id: &AccountId,
        expires_at: DateTime<Utc>,
    ) -> Result<UnsafeToken, CloudGolemError>;
    async fn delete(&self, account_id: &AccountId, id: TokenId) -> Result<(), CloudGolemError>;
}

pub struct TokenClientLive<C: golem_cloud_client::api::TokenClient + Sync + Send> {
    pub client: C,
}

#[async_trait]
impl<C: golem_cloud_client::api::TokenClient + Sync + Send> TokenClient for TokenClientLive<C> {
    async fn get_all(&self, account_id: &AccountId) -> Result<Vec<Token>, CloudGolemError> {
        info!("Getting all tokens for used: {account_id}");
        Ok(self.client.get_tokens(&account_id.id).await?)
    }

    async fn get(&self, account_id: &AccountId, id: TokenId) -> Result<Token, CloudGolemError> {
        info!("Getting derails for token: {id}");

        Ok(self.client.get_token(&account_id.id, &id.0).await?)
    }

    async fn post(
        &self,
        account_id: &AccountId,
        expires_at: DateTime<Utc>,
    ) -> Result<UnsafeToken, CloudGolemError> {
        info!("Creating token");

        Ok(self
            .client
            .create_token(&account_id.id, &CreateTokenDto { expires_at })
            .await?)
    }

    async fn delete(&self, account_id: &AccountId, id: TokenId) -> Result<(), CloudGolemError> {
        info!("Deleting token: {id}");

        let _ = self.client.delete_token(&account_id.id, &id.0).await?;
        Ok(())
    }
}
