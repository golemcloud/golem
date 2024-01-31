use std::ops::Deref;
use std::sync::Arc;

use async_trait::async_trait;
use cloud_common::model::TokenId;
use golem_common::model::AccountId;
use sqlx::{Database, Pool};
use uuid::Uuid;

use crate::model::{Token, UnsafeToken};
use crate::repo::RepoError;

#[derive(sqlx::FromRow, Debug, Clone)]
pub struct TokenRecord {
    pub id: Uuid,
    pub secret: Uuid,
    pub account_id: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub expires_at: chrono::DateTime<chrono::Utc>,
}

impl From<TokenRecord> for Token {
    fn from(value: TokenRecord) -> Self {
        Self {
            id: TokenId(value.id),
            account_id: AccountId::from(value.account_id.as_str()),
            created_at: value.created_at,
            expires_at: value.expires_at,
        }
    }
}

impl From<UnsafeToken> for TokenRecord {
    fn from(value: UnsafeToken) -> Self {
        Self {
            id: value.data.id.0,
            secret: value.secret.value,
            account_id: value.data.account_id.value.to_string(),
            created_at: value.data.created_at,
            expires_at: value.data.expires_at,
        }
    }
}

#[async_trait]
pub trait TokenRepo {
    async fn create(&self, token: &TokenRecord) -> Result<(), RepoError>;

    async fn get(&self, token_id: &Uuid) -> Result<Option<TokenRecord>, RepoError>;

    async fn get_by_secret(&self, secret: &Uuid) -> Result<Option<TokenRecord>, RepoError>;

    async fn get_by_account(&self, account_id: &str) -> Result<Vec<TokenRecord>, RepoError>;

    async fn delete(&self, token_id: &Uuid) -> Result<(), RepoError>;
}

pub struct DbTokenRepo<DB: Database> {
    db_pool: Arc<Pool<DB>>,
}

impl<DB: Database> DbTokenRepo<DB> {
    pub fn new(db_pool: Arc<Pool<DB>>) -> Self {
        Self { db_pool }
    }
}

#[async_trait]
impl TokenRepo for DbTokenRepo<sqlx::Postgres> {
    async fn create(&self, token: &TokenRecord) -> Result<(), RepoError> {
        sqlx::query(
            r#"
              INSERT INTO tokens
                (id, account_id, secret, created_at, expires_at)
              VALUES
                ($1, $2, $3, $4, $5)
            "#,
        )
        .bind(token.id)
        .bind(token.account_id.as_str())
        .bind(token.secret)
        .bind(token.created_at)
        .bind(token.expires_at)
        .execute(self.db_pool.deref())
        .await?;

        Ok(())
    }

    async fn get(&self, token_id: &Uuid) -> Result<Option<TokenRecord>, RepoError> {
        sqlx::query_as::<_, TokenRecord>("SELECT id, account_id, secret, created_at::timestamptz, expires_at::timestamptz FROM tokens WHERE id = $1")
            .bind(token_id)
            .fetch_optional(self.db_pool.deref())
            .await
            .map_err(|e| e.into())
    }

    async fn get_by_secret(&self, secret: &Uuid) -> Result<Option<TokenRecord>, RepoError> {
        sqlx::query_as::<_, TokenRecord>("SELECT id, account_id, secret, created_at::timestamptz, expires_at::timestamptz FROM tokens WHERE secret = $1")
            .bind(secret)
            .fetch_optional(self.db_pool.deref())
            .await
            .map_err(|e| e.into())
    }

    async fn get_by_account(&self, account_id: &str) -> Result<Vec<TokenRecord>, RepoError> {
        sqlx::query_as::<_, TokenRecord>("SELECT  id, account_id, secret, created_at::timestamptz, expires_at::timestamptz FROM tokens WHERE account_id = $1")
            .bind(account_id)
            .fetch_all(self.db_pool.deref())
            .await
            .map_err(|e| e.into())
    }

    async fn delete(&self, token_id: &Uuid) -> Result<(), RepoError> {
        sqlx::query("DELETE FROM tokens WHERE id = $1")
            .bind(token_id)
            .execute(self.db_pool.deref())
            .await?;
        Ok(())
    }
}

#[async_trait]
impl TokenRepo for DbTokenRepo<sqlx::Sqlite> {
    async fn create(&self, token: &TokenRecord) -> Result<(), RepoError> {
        sqlx::query(
            r#"
              INSERT INTO tokens
                (id, account_id, secret, created_at, expires_at)
              VALUES
                ($1, $2, $3, $4, $5)
            "#,
        )
        .bind(token.id)
        .bind(token.account_id.as_str())
        .bind(token.secret)
        .bind(token.created_at)
        .bind(token.expires_at)
        .execute(self.db_pool.deref())
        .await?;

        Ok(())
    }

    async fn get(&self, token_id: &Uuid) -> Result<Option<TokenRecord>, RepoError> {
        sqlx::query_as::<_, TokenRecord>(
            "SELECT id, account_id, secret, created_at, expires_at FROM tokens WHERE id = $1",
        )
        .bind(token_id)
        .fetch_optional(self.db_pool.deref())
        .await
        .map_err(|e| e.into())
    }

    async fn get_by_secret(&self, secret: &Uuid) -> Result<Option<TokenRecord>, RepoError> {
        sqlx::query_as::<_, TokenRecord>(
            "SELECT id, account_id, secret, created_at, expires_at FROM tokens WHERE secret = $1",
        )
        .bind(secret)
        .fetch_optional(self.db_pool.deref())
        .await
        .map_err(|e| e.into())
    }

    async fn get_by_account(&self, account_id: &str) -> Result<Vec<TokenRecord>, RepoError> {
        sqlx::query_as::<_, TokenRecord>("SELECT  id, account_id, secret, created_at, expires_at FROM tokens WHERE account_id = $1")
            .bind(account_id)
            .fetch_all(self.db_pool.deref())
            .await
            .map_err(|e| e.into())
    }

    async fn delete(&self, token_id: &Uuid) -> Result<(), RepoError> {
        sqlx::query("DELETE FROM tokens WHERE id = $1")
            .bind(token_id)
            .execute(self.db_pool.deref())
            .await?;
        Ok(())
    }
}
