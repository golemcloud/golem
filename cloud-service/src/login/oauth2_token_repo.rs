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

use crate::model::{OAuth2Provider, OAuth2Token};
use async_trait::async_trait;
use conditional_trait_gen::trait_gen;
use golem_common::model::AccountId;
use golem_common::model::TokenId;
use golem_service_base::db::Pool;
use golem_service_base::repo::RepoError;
use std::result::Result;
use std::str::FromStr;
use uuid::Uuid;

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
pub trait OAuth2TokenRepo: Send + Sync {
    async fn upsert(&self, token: &OAuth2TokenRecord) -> Result<(), RepoError>;

    async fn get(
        &self,
        provider: &str,
        external_id: &str,
    ) -> Result<Option<OAuth2TokenRecord>, RepoError>;

    async fn get_by_token_id(&self, token_id: &Uuid) -> Result<Vec<OAuth2TokenRecord>, RepoError>;

    async fn clean_token_id(&self, provider: &str, external_id: &str) -> Result<(), RepoError>;
}

pub struct DbOAuth2TokenRepo<DB: Pool> {
    db_pool: DB,
}

impl<DB: Pool> DbOAuth2TokenRepo<DB> {
    pub fn new(db_pool: DB) -> Self {
        Self { db_pool }
    }
}

#[trait_gen(golem_service_base::db::postgres::PostgresPool -> golem_service_base::db::postgres::PostgresPool, golem_service_base::db::sqlite::SqlitePool
)]
#[async_trait]
impl OAuth2TokenRepo for DbOAuth2TokenRepo<golem_service_base::db::postgres::PostgresPool> {
    async fn upsert(&self, token: &OAuth2TokenRecord) -> Result<(), RepoError> {
        let query = sqlx::query(
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
        .bind(token.token_id);

        self.db_pool
            .with_rw("oauth2_token", "upsert")
            .execute(query)
            .await?;

        Ok(())
    }

    async fn get(
        &self,
        provider: &str,
        external_id: &str,
    ) -> Result<Option<OAuth2TokenRecord>, RepoError> {
        let query = sqlx::query_as::<_, OAuth2TokenRecord>(
            "SELECT * FROM oauth2_tokens WHERE provider = $1 AND external_id = $2",
        )
        .bind(provider)
        .bind(external_id);

        self.db_pool
            .with_ro("oauth2_token", "get")
            .fetch_optional_as(query)
            .await
    }

    async fn get_by_token_id(&self, token_id: &Uuid) -> Result<Vec<OAuth2TokenRecord>, RepoError> {
        let query = sqlx::query_as::<_, OAuth2TokenRecord>(
            "SELECT * FROM oauth2_tokens WHERE token_id = $1",
        )
        .bind(token_id);

        self.db_pool
            .with_ro("oauth2_token", "get_by_token_id")
            .fetch_all(query)
            .await
    }

    async fn clean_token_id(&self, provider: &str, external_id: &str) -> Result<(), RepoError> {
        let query = sqlx::query(
            "UPDATE oauth2_tokens SET token_id = NULL WHERE provider = $1 AND external_id = $2",
        )
        .bind(provider)
        .bind(external_id);

        self.db_pool
            .with_rw("oauth2_token", "clean_token_id")
            .execute(query)
            .await?;
        Ok(())
    }
}
