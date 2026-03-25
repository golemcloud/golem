// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
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
use crate::repo::registry_change::{
    ChangeEventId, DbRegistryChangeRepo, NewRegistryChangeEvent,
};
use async_trait::async_trait;
use conditional_trait_gen::trait_gen;
use futures::future::BoxFuture;
use golem_service_base::db::postgres::PostgresPool;
use golem_service_base::db::sqlite::SqlitePool;
use golem_service_base::db::{LabelledPoolApi, Pool, PoolApi};
use golem_service_base::repo::{RepoError, RepoResult, ResultExt};
use indoc::indoc;
use std::fmt::Debug;
use tracing::{Instrument, Span, info_span};
use uuid::Uuid;

#[async_trait]
pub trait TokenRepo: Send + Sync {
    async fn create(&self, token: TokenRecord) -> RepoResult<Option<TokenRecord>>;

    async fn get_by_id(&self, token_id: Uuid) -> RepoResult<Option<TokenRecord>>;

    async fn get_by_secret(&self, secret: &str) -> RepoResult<Option<TokenRecord>>;

    async fn get_by_account(&self, account_id: Uuid) -> RepoResult<Vec<TokenRecord>>;

    async fn delete(&self, token_id: Uuid) -> RepoResult<()>;

    async fn delete_and_record_invalidation(
        &self,
        token_id: Uuid,
        account_id: Uuid,
    ) -> RepoResult<Option<ChangeEventId>>;
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

    async fn delete_and_record_invalidation(
        &self,
        token_id: Uuid,
        account_id: Uuid,
    ) -> RepoResult<Option<ChangeEventId>> {
        self.repo
            .delete_and_record_invalidation(token_id, account_id)
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

    async fn with_tx_err<R, E, F>(&self, api_name: &'static str, f: F) -> Result<R, E>
    where
        R: Send,
        E: Debug + Send + From<RepoError>,
        F: for<'f> FnOnce(
                &'f mut <DBP::LabelledApi as LabelledPoolApi>::LabelledTransaction,
            ) -> BoxFuture<'f, Result<R, E>>
            + Send,
    {
        self.db_pool
            .with_tx_err(METRICS_SVC_NAME, api_name, f)
            .await
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

    async fn delete_and_record_invalidation(
        &self,
        token_id: Uuid,
        account_id: Uuid,
    ) -> RepoResult<Option<ChangeEventId>> {
        let result: Option<ChangeEventId> = self
            .with_tx_err("delete_and_record_invalidation", |tx| {
                Box::pin(async move {
                    let delete_result = tx
                        .execute(
                            sqlx::query("DELETE FROM tokens WHERE token_id = $1").bind(token_id),
                        )
                        .await?;

                    if delete_result.rows_affected() == 0 {
                        return Ok::<_, RepoError>(None);
                    }

                    let event = NewRegistryChangeEvent::account_tokens_invalidated(account_id);
                    let event_id =
                        DbRegistryChangeRepo::<PostgresPool>::insert_change_event_in_tx(tx, &event)
                            .await?;
                    Ok(Some(event_id))
                })
            })
            .await?;

        Ok(result)
    }
}
