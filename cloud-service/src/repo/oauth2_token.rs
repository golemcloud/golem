use std::ops::Deref;
use std::result::Result;
use std::str::FromStr;
use std::sync::Arc;

use async_trait::async_trait;
use cloud_common::model::TokenId;
use golem_common::model::AccountId;
use sqlx::{Database, Pool};
use uuid::Uuid;

use crate::model::{OAuth2Provider, OAuth2Token};
use crate::repo::RepoError;

#[derive(sqlx::FromRow, Debug, Clone)]
pub struct OAuth2TokenRecord {
    pub provider: String,
    pub external_id: String,
    pub account_id: String,
    pub token_id: Option<Uuid>,
}

impl TryFrom<OAuth2TokenRecord> for OAuth2Token {
    type Error = String;

    fn try_from(value: OAuth2TokenRecord) -> Result<Self, Self::Error> {
        let provider = OAuth2Provider::from_str(value.provider.as_str())?;
        Ok(Self {
            provider,
            external_id: value.external_id.clone(),
            account_id: AccountId::from(value.account_id.as_str()),
            token_id: value.token_id.map(TokenId),
        })
    }
}

impl From<OAuth2Token> for OAuth2TokenRecord {
    fn from(value: OAuth2Token) -> Self {
        Self {
            provider: value.provider.to_string(),
            external_id: value.external_id.clone(),
            account_id: value.account_id.value.to_string(),
            token_id: value.token_id.map(|t| t.0),
        }
    }
}

#[async_trait]
pub trait OAuth2TokenRepo {
    async fn upsert(&self, token: &OAuth2TokenRecord) -> Result<(), RepoError>;

    async fn get(
        &self,
        provider: &str,
        external_id: &str,
    ) -> Result<Option<OAuth2TokenRecord>, RepoError>;

    async fn get_by_token_id(&self, token_id: &Uuid) -> Result<Vec<OAuth2TokenRecord>, RepoError>;

    async fn clean_token_id(&self, provider: &str, external_id: &str) -> Result<(), RepoError>;
}

pub struct DbOAuth2TokenRepo<DB: Database> {
    db_pool: Arc<Pool<DB>>,
}

impl<DB: Database> DbOAuth2TokenRepo<DB> {
    pub fn new(db_pool: Arc<Pool<DB>>) -> Self {
        Self { db_pool }
    }
}

#[async_trait]
impl OAuth2TokenRepo for DbOAuth2TokenRepo<sqlx::Postgres> {
    async fn upsert(&self, token: &OAuth2TokenRecord) -> Result<(), RepoError> {
        sqlx::query(
            r#"
              INSERT INTO oauth2_tokens
                (provider, external_id, account_id, token_id)
              VALUES
                ($1, $2, $3, $4)
              ON CONFLICT (provider, external_id) DO UPDATE
              SET account_id = $3,
                  token_id = $4
            "#,
        )
        .bind(token.provider.as_str())
        .bind(token.external_id.as_str())
        .bind(token.account_id.as_str())
        .bind(token.token_id)
        .execute(self.db_pool.deref())
        .await?;

        Ok(())
    }

    async fn get(
        &self,
        provider: &str,
        external_id: &str,
    ) -> Result<Option<OAuth2TokenRecord>, RepoError> {
        sqlx::query_as::<_, OAuth2TokenRecord>(
            "SELECT * FROM oauth2_tokens WHERE provider = $1 AND external_id = $2",
        )
        .bind(provider)
        .bind(external_id)
        .fetch_optional(self.db_pool.deref())
        .await
        .map_err(|e| e.into())
    }

    async fn get_by_token_id(&self, token_id: &Uuid) -> Result<Vec<OAuth2TokenRecord>, RepoError> {
        sqlx::query_as::<_, OAuth2TokenRecord>("SELECT * FROM oauth2_tokens WHERE token_id = $1")
            .bind(token_id)
            .fetch_all(self.db_pool.deref())
            .await
            .map_err(|e| e.into())
    }

    async fn clean_token_id(&self, provider: &str, external_id: &str) -> Result<(), RepoError> {
        sqlx::query(
            "UPDATE oauth2_tokens SET token_id = NULL WHERE provider = $1 AND external_id = $2",
        )
        .bind(provider)
        .bind(external_id)
        .execute(self.db_pool.deref())
        .await?;
        Ok(())
    }
}

#[async_trait]
impl OAuth2TokenRepo for DbOAuth2TokenRepo<sqlx::Sqlite> {
    async fn upsert(&self, token: &OAuth2TokenRecord) -> Result<(), RepoError> {
        sqlx::query(
            r#"
              INSERT INTO oauth2_tokens
                (provider, external_id, account_id, token_id)
              VALUES
                ($1, $2, $3, $4)
              ON CONFLICT (provider, external_id) DO UPDATE
              SET account_id = $3,
                  token_id = $4
            "#,
        )
        .bind(token.provider.as_str())
        .bind(token.external_id.as_str())
        .bind(token.account_id.as_str())
        .bind(token.token_id)
        .execute(self.db_pool.deref())
        .await?;

        Ok(())
    }

    async fn get(
        &self,
        provider: &str,
        external_id: &str,
    ) -> Result<Option<OAuth2TokenRecord>, RepoError> {
        sqlx::query_as::<_, OAuth2TokenRecord>(
            "SELECT * FROM oauth2_tokens WHERE provider = $1 AND external_id = $2",
        )
        .bind(provider)
        .bind(external_id)
        .fetch_optional(self.db_pool.deref())
        .await
        .map_err(|e| e.into())
    }

    async fn get_by_token_id(&self, token_id: &Uuid) -> Result<Vec<OAuth2TokenRecord>, RepoError> {
        sqlx::query_as::<_, OAuth2TokenRecord>("SELECT * FROM oauth2_tokens WHERE token_id = $1")
            .bind(token_id)
            .fetch_all(self.db_pool.deref())
            .await
            .map_err(|e| e.into())
    }

    async fn clean_token_id(&self, provider: &str, external_id: &str) -> Result<(), RepoError> {
        sqlx::query(
            "UPDATE oauth2_tokens SET token_id = NULL WHERE provider = $1 AND external_id = $2",
        )
        .bind(provider)
        .bind(external_id)
        .execute(self.db_pool.deref())
        .await?;
        Ok(())
    }
}
