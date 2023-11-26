use async_trait::async_trait;
use chrono::{DateTime, Utc};
use golem_client::apis::configuration::Configuration;
use golem_client::apis::token_api::{
    v2_accounts_account_id_tokens_get, v2_accounts_account_id_tokens_post,
    v2_accounts_account_id_tokens_token_id_delete, v2_accounts_account_id_tokens_token_id_get,
};
use golem_client::models;
use golem_client::models::{CreateTokenDto, UnsafeToken};
use tracing::info;

use crate::model::{AccountId, GolemError, TokenId};

#[async_trait]
pub trait TokenClient {
    async fn get_all(&self, account_id: &AccountId) -> Result<Vec<models::Token>, GolemError>;
    async fn get(&self, account_id: &AccountId, id: TokenId) -> Result<models::Token, GolemError>;
    async fn post(
        &self,
        account_id: &AccountId,
        expires_at: DateTime<Utc>,
    ) -> Result<UnsafeToken, GolemError>;
    async fn delete(&self, account_id: &AccountId, id: TokenId) -> Result<(), GolemError>;
}

pub struct TokenClientLive {
    pub configuration: Configuration,
}

#[async_trait]
impl TokenClient for TokenClientLive {
    async fn get_all(&self, account_id: &AccountId) -> Result<Vec<models::Token>, GolemError> {
        info!("Getting all tokens for used: {account_id}");
        Ok(v2_accounts_account_id_tokens_get(&self.configuration, &account_id.id).await?)
    }

    async fn get(&self, account_id: &AccountId, id: TokenId) -> Result<models::Token, GolemError> {
        info!("Getting derails for token: {id}");

        Ok(v2_accounts_account_id_tokens_token_id_get(
            &self.configuration,
            &account_id.id,
            &id.0.to_string(),
        )
        .await?)
    }

    async fn post(
        &self,
        account_id: &AccountId,
        expires_at: DateTime<Utc>,
    ) -> Result<UnsafeToken, GolemError> {
        info!("Creating token");

        Ok(v2_accounts_account_id_tokens_post(
            &self.configuration,
            &account_id.id,
            CreateTokenDto {
                expires_at: expires_at.to_string(),
            },
        )
        .await?)
    }

    async fn delete(&self, account_id: &AccountId, id: TokenId) -> Result<(), GolemError> {
        info!("Deleting token: {id}");

        let _ = v2_accounts_account_id_tokens_token_id_delete(
            &self.configuration,
            &account_id.id,
            &id.0.to_string(),
        )
        .await?;
        Ok(())
    }
}
