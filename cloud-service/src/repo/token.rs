// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use crate::model::{Token, UnsafeToken};
use async_trait::async_trait;
use conditional_trait_gen::{trait_gen, when};
use golem_common::model::AccountId;
use golem_common::model::TokenId;
use golem_service_base::db::Pool;
use golem_service_base::repo::RepoError;
use uuid::Uuid;

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
pub trait TokenRepo: Send + Sync {
    async fn create(&self, token: &TokenRecord) -> Result<(), RepoError>;

    async fn get(&self, token_id: &Uuid) -> Result<Option<TokenRecord>, RepoError>;

    async fn get_by_secret(&self, secret: &Uuid) -> Result<Option<TokenRecord>, RepoError>;

    async fn get_by_account(&self, account_id: &str) -> Result<Vec<TokenRecord>, RepoError>;

    async fn delete(&self, token_id: &Uuid) -> Result<(), RepoError>;
}

pub struct DbTokenRepo<DB: Pool> {
    db_pool: DB,
}

impl<DB: Pool> DbTokenRepo<DB> {
    pub fn new(db_pool: DB) -> Self {
        Self { db_pool }
    }
}

#[trait_gen(golem_service_base::db::postgres::PostgresPool -> golem_service_base::db::postgres::PostgresPool, golem_service_base::db::sqlite::SqlitePool
)]
#[async_trait]
impl TokenRepo for DbTokenRepo<golem_service_base::db::postgres::PostgresPool> {
    async fn create(&self, token: &TokenRecord) -> Result<(), RepoError> {
        let query = sqlx::query(
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
        .bind(token.expires_at);

        self.db_pool
            .with_rw("tokens", "create")
            .execute(query)
            .await?;

        Ok(())
    }

    #[when(golem_service_base::db::postgres::PostgresPool -> get)]
    async fn get_postgres(&self, token_id: &Uuid) -> Result<Option<TokenRecord>, RepoError> {
        let query = sqlx::query_as::<_, TokenRecord>("SELECT id, account_id, secret, created_at::timestamptz, expires_at::timestamptz FROM tokens WHERE id = $1")
            .bind(token_id);

        self.db_pool
            .with_ro("tokens", "get")
            .fetch_optional_as(query)
            .await
    }

    #[when(golem_service_base::db::sqlite::SqlitePool -> get)]
    async fn get_sqlite(&self, token_id: &Uuid) -> Result<Option<TokenRecord>, RepoError> {
        let query = sqlx::query_as::<_, TokenRecord>(
            "SELECT id, account_id, secret, created_at, expires_at FROM tokens WHERE id = $1",
        )
        .bind(token_id);

        self.db_pool
            .with_ro("tokens", "get")
            .fetch_optional_as(query)
            .await
    }

    #[when(golem_service_base::db::postgres::PostgresPool -> get_by_secret)]
    async fn get_by_secret_postgres(
        &self,
        secret: &Uuid,
    ) -> Result<Option<TokenRecord>, RepoError> {
        let query = sqlx::query_as::<_, TokenRecord>("SELECT id, account_id, secret, created_at::timestamptz, expires_at::timestamptz FROM tokens WHERE secret = $1")
            .bind(secret);

        self.db_pool
            .with_ro("tokens", "get_by_secret")
            .fetch_optional_as(query)
            .await
    }

    #[when(golem_service_base::db::sqlite::SqlitePool -> get_by_secret)]
    async fn get_by_secret_sqlite(&self, secret: &Uuid) -> Result<Option<TokenRecord>, RepoError> {
        let query = sqlx::query_as::<_, TokenRecord>(
            "SELECT id, account_id, secret, created_at, expires_at FROM tokens WHERE secret = $1",
        )
        .bind(secret);

        self.db_pool
            .with_ro("tokens", "get_by_secret")
            .fetch_optional_as(query)
            .await
    }

    #[when(golem_service_base::db::postgres::PostgresPool -> get_by_account)]
    async fn get_by_account_postgres(
        &self,
        account_id: &str,
    ) -> Result<Vec<TokenRecord>, RepoError> {
        let query = sqlx::query_as::<_, TokenRecord>("SELECT  id, account_id, secret, created_at::timestamptz, expires_at::timestamptz FROM tokens WHERE account_id = $1")
            .bind(account_id);

        self.db_pool
            .with_ro("tokens", "get_by_account")
            .fetch_all(query)
            .await
    }

    #[when(golem_service_base::db::sqlite::SqlitePool -> get_by_account)]
    async fn get_by_account_sqlite(&self, account_id: &str) -> Result<Vec<TokenRecord>, RepoError> {
        let query = sqlx::query_as::<_, TokenRecord>("SELECT  id, account_id, secret, created_at, expires_at FROM tokens WHERE account_id = $1")
            .bind(account_id);

        self.db_pool
            .with_ro("tokens", "get_by_account")
            .fetch_all(query)
            .await
    }

    async fn delete(&self, token_id: &Uuid) -> Result<(), RepoError> {
        let query = sqlx::query("DELETE FROM tokens WHERE id = $1").bind(token_id);

        self.db_pool
            .with_rw("tokens", "delete")
            .execute(query)
            .await?;

        Ok(())
    }
}
