use async_trait::async_trait;
use chrono::{DateTime, Utc};
use golem_client::model;
use golem_client::model::{CreateTokenDTO, UnsafeToken};
use golem_client::token::Token;
use tracing::info;
use crate::clients::CloudAuthentication;
use crate::model::{AccountId, GolemError, TokenId};

#[async_trait]
pub trait TokenClient {
    async fn get_all(&self, account_id: &AccountId, auth: &CloudAuthentication) -> Result<Vec<model::Token>, GolemError>;
    async fn get(&self, account_id: &AccountId, id: TokenId, auth: &CloudAuthentication) -> Result<model::Token, GolemError>;
    async fn post(&self, account_id: &AccountId, expires_at: DateTime<Utc>, auth: &CloudAuthentication) -> Result<UnsafeToken, GolemError>;
    async fn delete(&self, account_id: &AccountId, id: TokenId, auth: &CloudAuthentication) -> Result<(), GolemError>;

}

pub struct TokenClientLive<C: Token + Send + Sync> {
    pub client: C
}

#[async_trait]
impl<C: Token + Send + Sync> TokenClient for TokenClientLive<C> {
    async fn get_all(&self, account_id: &AccountId, auth: &CloudAuthentication) -> Result<Vec<model::Token>, GolemError> {
        info!("Getting all tokens for used: {account_id}");
        Ok(self.client.get_tokens(&account_id.id, &auth.header()).await?)
    }

    async fn get(&self, account_id: &AccountId, id: TokenId, auth: &CloudAuthentication) -> Result<model::Token, GolemError> {
        info!("Getting derails for token: {id}");
        Ok(self.client.get_token(&account_id.id, id.into(), &auth.header()).await?)
    }

    async fn post(&self, account_id: &AccountId, expires_at: DateTime<Utc>, auth: &CloudAuthentication) -> Result<UnsafeToken, GolemError> {
        info!("Creating token");
        Ok(self.client.post_token(&account_id.id, CreateTokenDTO{expires_at}, &auth.header()).await?)
    }

    async fn delete(&self, account_id: &AccountId, id: TokenId, auth: &CloudAuthentication) -> Result<(), GolemError> {
        info!("Deleting token: {id}");

        Ok(self.client.delete_token(&account_id.id, id.into(), &auth.header()).await?)
    }
}