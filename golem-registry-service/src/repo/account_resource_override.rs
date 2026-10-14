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

use crate::repo::model::account_resource_override::{
    AccountResourceOverrideDimension, AccountResourceOverrideRecord,
};
use async_trait::async_trait;
use conditional_trait_gen::trait_gen;
use futures::FutureExt;
use futures::future::BoxFuture;
use golem_service_base::db::postgres::PostgresPool;
use golem_service_base::db::sqlite::SqlitePool;
use golem_service_base::db::{LabelledPoolApi, Pool, PoolApi};
use golem_service_base::repo::{NumericU64, RepoResult, SqlDateTime};
use indoc::indoc;
use tracing::{Instrument, Span, info_span};
use uuid::Uuid;

#[async_trait]
pub trait AccountResourceOverrideRepo: Send + Sync {
    async fn get_active_value(
        &self,
        account_id: Uuid,
        dimension: AccountResourceOverrideDimension,
        now: &SqlDateTime,
    ) -> RepoResult<Option<NumericU64>>;

    async fn upsert(&self, record: AccountResourceOverrideRecord) -> RepoResult<()>;

    async fn delete(
        &self,
        account_id: Uuid,
        dimension: AccountResourceOverrideDimension,
    ) -> RepoResult<()>;
}

pub struct LoggedAccountResourceOverrideRepo<Repo: AccountResourceOverrideRepo> {
    repo: Repo,
}

static SPAN_NAME: &str = "account resource override repository";

impl<Repo: AccountResourceOverrideRepo> LoggedAccountResourceOverrideRepo<Repo> {
    pub fn new(repo: Repo) -> Self {
        Self { repo }
    }

    fn span(account_id: Uuid, dimension: AccountResourceOverrideDimension) -> Span {
        info_span!(SPAN_NAME, account_id = %account_id, dimension = dimension.as_str())
    }
}

#[async_trait]
impl<Repo: AccountResourceOverrideRepo> AccountResourceOverrideRepo
    for LoggedAccountResourceOverrideRepo<Repo>
{
    async fn get_active_value(
        &self,
        account_id: Uuid,
        dimension: AccountResourceOverrideDimension,
        now: &SqlDateTime,
    ) -> RepoResult<Option<NumericU64>> {
        self.repo
            .get_active_value(account_id, dimension, now)
            .instrument(Self::span(account_id, dimension))
            .await
    }

    async fn upsert(&self, record: AccountResourceOverrideRecord) -> RepoResult<()> {
        let span = Self::span(record.account_id, record.dimension);
        self.repo.upsert(record).instrument(span).await
    }

    async fn delete(
        &self,
        account_id: Uuid,
        dimension: AccountResourceOverrideDimension,
    ) -> RepoResult<()> {
        self.repo
            .delete(account_id, dimension)
            .instrument(Self::span(account_id, dimension))
            .await
    }
}

pub struct DbAccountResourceOverrideRepo<DBP: Pool> {
    db_pool: DBP,
}

impl<DBP: Pool> DbAccountResourceOverrideRepo<DBP> {
    pub fn new(db_pool: DBP) -> Self {
        Self { db_pool }
    }

    pub fn logged(db_pool: DBP) -> LoggedAccountResourceOverrideRepo<Self>
    where
        Self: AccountResourceOverrideRepo,
    {
        LoggedAccountResourceOverrideRepo::new(Self::new(db_pool))
    }

    fn with_ro(&self, api_name: &'static str) -> DBP::LabelledApi {
        self.db_pool.with_ro("account_resource_override", api_name)
    }

    async fn with_tx<R, F>(&self, api_name: &'static str, f: F) -> RepoResult<R>
    where
        R: Send,
        F: for<'f> FnOnce(
                &'f mut <DBP::LabelledApi as LabelledPoolApi>::LabelledTransaction,
            ) -> BoxFuture<'f, RepoResult<R>>
            + Send,
    {
        self.db_pool
            .with_tx("account_resource_override", api_name, f)
            .await
    }
}

#[trait_gen(PostgresPool -> PostgresPool, SqlitePool)]
#[async_trait]
impl AccountResourceOverrideRepo for DbAccountResourceOverrideRepo<PostgresPool> {
    async fn get_active_value(
        &self,
        account_id: Uuid,
        dimension: AccountResourceOverrideDimension,
        now: &SqlDateTime,
    ) -> RepoResult<Option<NumericU64>> {
        let value: Option<(NumericU64,)> = self
            .with_ro("get_active_value")
            .fetch_optional_as(
                sqlx::query_as(
                    "SELECT override_value FROM account_resource_overrides WHERE account_id = $1 AND dimension = $2 AND (expires_at IS NULL OR expires_at > $3)",
                )
                .bind(account_id)
                .bind(dimension.as_str())
                .bind(now),
            )
            .await?;
        Ok(value.map(|(value,)| value))
    }

    async fn upsert(&self, record: AccountResourceOverrideRecord) -> RepoResult<()> {
        self.with_tx("upsert", |tx| {
            async move {
                tx.execute(
                    sqlx::query(indoc! { r#"
                        INSERT INTO account_resource_overrides (
                            account_id, dimension, override_value, reason, expires_at, created_by, created_at
                        ) VALUES ($1, $2, $3, $4, $5, $6, $7)
                        ON CONFLICT (account_id, dimension) DO UPDATE SET
                            override_value = $3,
                            reason = $4,
                            expires_at = $5,
                            created_by = $6,
                            created_at = $7
                    "# })
                    .bind(record.account_id)
                    .bind(record.dimension.as_str())
                    .bind(record.override_value)
                    .bind(record.reason.as_str())
                    .bind(record.expires_at)
                    .bind(record.created_by)
                    .bind(record.created_at),
                )
                .await?;
                Ok(())
            }
            .boxed()
        })
        .await
    }

    async fn delete(
        &self,
        account_id: Uuid,
        dimension: AccountResourceOverrideDimension,
    ) -> RepoResult<()> {
        self.with_tx("delete", |tx| {
            async move {
                tx.execute(
                    sqlx::query("DELETE FROM account_resource_overrides WHERE account_id = $1 AND dimension = $2")
                        .bind(account_id)
                        .bind(dimension.as_str()),
                )
                .await?;
                Ok(())
            }
            .boxed()
        })
        .await
    }
}
