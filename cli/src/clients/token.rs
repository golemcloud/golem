use async_trait::async_trait;
use chrono::{DateTime, Utc};
use golem_client::model::CreateTokenDto;
use golem_client::model::Token;
use golem_client::model::UnsafeToken;
use tracing::info;

use crate::model::{AccountId, GolemError, TokenId};

#[async_trait]
pub trait TokenClient {
    async fn get_all(&self, account_id: &AccountId) -> Result<Vec<Token>, GolemError>;
    async fn get(&self, account_id: &AccountId, id: TokenId) -> Result<Token, GolemError>;
    async fn post(
        &self,
        account_id: &AccountId,
        expires_at: DateTime<Utc>,
    ) -> Result<UnsafeToken, GolemError>;
    async fn delete(&self, account_id: &AccountId, id: TokenId) -> Result<(), GolemError>;
}

pub struct TokenClientLive<C: golem_client::api::TokenClient + Sync + Send> {
    pub client: C,
}

#[async_trait]
impl<C: golem_client::api::TokenClient + Sync + Send> TokenClient for TokenClientLive<C> {
    async fn get_all(&self, account_id: &AccountId) -> Result<Vec<Token>, GolemError> {
        info!("Getting all tokens for used: {account_id}");
        Ok(self.client.get(&account_id.id).await?)
    }

    async fn get(&self, account_id: &AccountId, id: TokenId) -> Result<Token, GolemError> {
        info!("Getting derails for token: {id}");

        Ok(self.client.token_id_get(&account_id.id, &id.0).await?)
    }

    async fn post(
        &self,
        account_id: &AccountId,
        expires_at: DateTime<Utc>,
    ) -> Result<UnsafeToken, GolemError> {
        info!("Creating token");

        Ok(self
            .client
            .post(&account_id.id, &CreateTokenDto { expires_at })
            .await?)
    }

    async fn delete(&self, account_id: &AccountId, id: TokenId) -> Result<(), GolemError> {
        info!("Deleting token: {id}");

        let _ = self.client.token_id_delete(&account_id.id, &id.0).await?;
        Ok(())
    }
}
