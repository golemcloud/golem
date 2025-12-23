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

use crate::repo::model::token::TokenRecord;
use async_trait::async_trait;
use conditional_trait_gen::trait_gen;
use golem_service_base::db::postgres::PostgresPool;
use golem_service_base::db::sqlite::SqlitePool;
use golem_service_base::db::{Pool, PoolApi};
use golem_service_base::repo::{RepoResult, ResultExt};
use indoc::indoc;
use tracing::{Instrument, Span, info_span};
use uuid::Uuid;

#[async_trait]
pub trait TokenRepo: Send + Sync {
    async fn create(&self, token: TokenRecord) -> RepoResult<Option<TokenRecord>>;

    async fn get_by_id(&self, token_id: Uuid) -> RepoResult<Option<TokenRecord>>;

    async fn get_by_secret(&self, secret: &str) -> RepoResult<Option<TokenRecord>>;

    async fn get_by_account(&self, account_id: Uuid) -> RepoResult<Vec<TokenRecord>>;

    async fn delete(&self, token_id: Uuid) -> RepoResult<()>;
}

pub struct LoggedTokenRepo<Repo: TokenRepo> {
    repo: Repo,
}

static SPAN_NAME: &str = "token repository";

impl<Repo: TokenRepo> LoggedTokenRepo<Repo> {
    pub fn new(repo: Repo) -> Self {
        Self { repo }
    }

    fn span_id(token_id: Uuid) -> Span {
        info_span!(SPAN_NAME, token_id=%token_id)
    }

    fn span_account(account_id: Uuid) -> Span {
        info_span!(SPAN_NAME, account_id = %account_id)
    }
}

#[async_trait]
impl<Repo: TokenRepo> TokenRepo for LoggedTokenRepo<Repo> {
    async fn create(&self, token: TokenRecord) -> RepoResult<Option<TokenRecord>> {
        let span = Self::span_id(token.token_id);
        self.repo.create(token).instrument(span).await
    }

    async fn get_by_id(&self, token_id: Uuid) -> RepoResult<Option<TokenRecord>> {
        self.repo
            .get_by_id(token_id)
            .instrument(Self::span_id(token_id))
            .await
    }

    async fn get_by_secret(&self, secret: &str) -> RepoResult<Option<TokenRecord>> {
        self.repo
            .get_by_secret(secret)
            .instrument(info_span!(SPAN_NAME))
            .await
    }

    async fn get_by_account(&self, account_id: Uuid) -> RepoResult<Vec<TokenRecord>> {
        self.repo
            .get_by_account(account_id)
            .instrument(Self::span_account(account_id))
            .await
    }

    async fn delete(&self, token_id: Uuid) -> RepoResult<()> {
        self.repo
            .delete(token_id)
            .instrument(Self::span_id(token_id))
            .await
    }
}

pub struct DbTokenRepo<DBP: Pool> {
    db_pool: DBP,
}

static METRICS_SVC_NAME: &str = "token";

impl<DBP: Pool> DbTokenRepo<DBP> {
    pub fn new(db_pool: DBP) -> Self {
        Self { db_pool }
    }

    pub fn logged(db_pool: DBP) -> LoggedTokenRepo<Self>
    where
        Self: TokenRepo,
    {
        LoggedTokenRepo::new(Self::new(db_pool))
    }

    fn with_ro(&self, api_name: &'static str) -> DBP::LabelledApi {
        self.db_pool.with_ro(METRICS_SVC_NAME, api_name)
    }

    fn with_rw(&self, api_name: &'static str) -> DBP::LabelledApi {
        self.db_pool.with_rw(METRICS_SVC_NAME, api_name)
    }
}

#[trait_gen(PostgresPool -> PostgresPool, SqlitePool)]
#[async_trait]
impl TokenRepo for DbTokenRepo<PostgresPool> {
    async fn create(&self, token: TokenRecord) -> RepoResult<Option<TokenRecord>> {
        self.with_rw("create")
            .fetch_one_as(
                sqlx::query_as(indoc! { r#"
                    INSERT INTO tokens (token_id, account_id, secret, created_at, expires_at)
                    VALUES ($1, $2, $3, $4, $5)
                    RETURNING token_id, account_id, secret, created_at, expires_at
                "#})
                .bind(token.token_id)
                .bind(token.account_id)
                .bind(token.secret)
                .bind(token.created_at)
                .bind(token.expires_at),
            )
            .await
            .none_on_unique_violation()
    }

    async fn get_by_id(&self, token_id: Uuid) -> RepoResult<Option<TokenRecord>> {
        self.with_ro("get_by_id")
            .fetch_optional_as(
                sqlx::query_as(indoc! { r#"
                    SELECT t.token_id, t.secret, t.account_id, t.created_at, t.expires_at
                    FROM accounts a
                    JOIN tokens t
                        ON t.account_id = a.account_id
                    WHERE
                        t.token_id = $1
                        AND a.deleted_at IS NULL
                "#})
                .bind(token_id),
            )
            .await
    }

    async fn get_by_secret(&self, secret: &str) -> RepoResult<Option<TokenRecord>> {
        self.with_ro("get_by_secret")
            .fetch_optional_as(
                sqlx::query_as(indoc! { r#"
                    SELECT t.token_id, t.secret, t.account_id, t.created_at, t.expires_at
                    FROM accounts a
                    JOIN tokens t
                        ON t.account_id = a.account_id
                    WHERE
                        t.secret = $1
                        AND a.deleted_at IS NULL
                "#})
                .bind(secret),
            )
            .await
    }

    async fn get_by_account(&self, account_id: Uuid) -> RepoResult<Vec<TokenRecord>> {
        self.with_ro("get_by_account")
            .fetch_all_as(
                sqlx::query_as(indoc! { r#"
                    SELECT t.token_id, t.secret, t.account_id, t.created_at, t.expires_at
                    FROM accounts a
                    JOIN tokens t
                        ON t.account_id = a.account_id
                    WHERE
                        a.account_id = $1
                        AND a.deleted_at IS NULL
                    ORDER BY token_id
                "#})
                .bind(account_id),
            )
            .await
    }

    async fn delete(&self, token_id: Uuid) -> RepoResult<()> {
        self.with_rw("delete")
            .execute(
                sqlx::query(indoc! { r#"
                    DELETE FROM tokens WHERE token_id = $1
                "#})
                .bind(token_id),
            )
            .await?;

        Ok(())
    }
}
