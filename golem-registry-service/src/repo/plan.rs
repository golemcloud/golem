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

use crate::repo::model::account_usage::UsageType;
use crate::repo::model::plan::PlanRecord;
use async_trait::async_trait;
use conditional_trait_gen::trait_gen;
use futures::future::BoxFuture;
use futures::{FutureExt, StreamExt, TryStreamExt, stream};
use golem_service_base::db::postgres::PostgresPool;
use golem_service_base::db::sqlite::SqlitePool;
use golem_service_base::db::{LabelledPoolApi, LabelledPoolTransaction, Pool};
use golem_service_base::repo::RepoResult;
use indoc::indoc;
use sqlx::{Database, Row};
use std::collections::BTreeMap;
use tracing::{Instrument, Span, info_span};
use uuid::Uuid;

#[async_trait]
pub trait PlanRepo: Send + Sync {
    async fn create_or_update(&self, plan: PlanRecord) -> RepoResult<()>;

    async fn get_by_id(&self, plan_id: &Uuid) -> RepoResult<Option<PlanRecord>>;

    async fn list(&self) -> RepoResult<Vec<PlanRecord>>;
}

pub struct LoggedPlanRepo<Repo: PlanRepo> {
    repo: Repo,
}

static SPAN_NAME: &str = "plan repository";

impl<Repo: PlanRepo> LoggedPlanRepo<Repo> {
    pub fn new(repo: Repo) -> Self {
        Self { repo }
    }

    pub fn span_id(plan_id: &Uuid) -> Span {
        info_span!(SPAN_NAME, plan_id=%plan_id)
    }
}

#[async_trait]
impl<Repo: PlanRepo> PlanRepo for LoggedPlanRepo<Repo> {
    async fn create_or_update(&self, plan: PlanRecord) -> RepoResult<()> {
        let span = Self::span_id(&plan.plan_id);
        self.repo.create_or_update(plan).instrument(span).await
    }

    async fn get_by_id(&self, plan_id: &Uuid) -> RepoResult<Option<PlanRecord>> {
        self.repo
            .get_by_id(plan_id)
            .instrument(Self::span_id(plan_id))
            .await
    }

    async fn list(&self) -> RepoResult<Vec<PlanRecord>> {
        self.repo.list().await
    }
}

pub struct DbPlanRepo<DBP: Pool> {
    db_pool: DBP,
}

static METRICS_SVC_NAME: &str = "plan";

impl<DBP: Pool> DbPlanRepo<DBP> {
    pub fn new(db_pool: DBP) -> Self {
        Self { db_pool }
    }

    pub fn logged(db_pool: DBP) -> LoggedPlanRepo<Self>
    where
        Self: PlanRepo,
    {
        LoggedPlanRepo::new(Self::new(db_pool))
    }

    fn with_ro(&self, api_name: &'static str) -> DBP::LabelledApi {
        self.db_pool.with_ro(METRICS_SVC_NAME, api_name)
    }

    async fn with_tx<R, F>(&self, api_name: &'static str, f: F) -> RepoResult<R>
    where
        R: Send,
        F: for<'f> FnOnce(
                &'f mut <DBP::LabelledApi as LabelledPoolApi>::LabelledTransaction,
            ) -> BoxFuture<'f, RepoResult<R>>
            + Send,
    {
        self.db_pool.with_tx(METRICS_SVC_NAME, api_name, f).await
    }
}

#[trait_gen(PostgresPool -> PostgresPool, SqlitePool)]
#[async_trait]
impl PlanRepo for DbPlanRepo<PostgresPool> {
    async fn create_or_update(&self, plan: PlanRecord) -> RepoResult<()> {
        self.with_tx("create_or_update", |tx| {
            async move {
                tx.execute(
                    sqlx::query(indoc! { r#"
                        INSERT INTO plans (plan_id, name) VALUES ($1, $2)
                        ON CONFLICT (plan_id) DO UPDATE SET name = $2
                    "#})
                    .bind(plan.plan_id)
                    .bind(plan.name),
                )
                .await?;

                tx.execute(
                    sqlx::query(indoc! { r#"
                        DELETE FROM plan_usage_limits WHERE plan_id = $1;
                    "#})
                    .bind(plan.plan_id),
                )
                .await?;

                for (usage_type, limit) in plan.limits {
                    if let Some(limit) = limit {
                        Self::insert_limit(tx, &plan.plan_id, usage_type, limit).await?;
                    }
                }

                Ok(())
            }
            .boxed()
        })
        .await
    }

    async fn get_by_id(&self, plan_id: &Uuid) -> RepoResult<Option<PlanRecord>> {
        let plan: Option<PlanRecord> = self
            .with_ro("get_by_id")
            .fetch_optional_as(
                sqlx::query_as(indoc! { r#"
                    SELECT plan_id, name FROM plans WHERE plan_id = $1
                "# })
                .bind(plan_id),
            )
            .await?;

        match plan {
            Some(plan) => Ok(Some(self.with_limits(plan).await?)),
            None => Ok(None),
        }
    }

    async fn list(&self) -> RepoResult<Vec<PlanRecord>> {
        let plans = self
            .with_ro("list")
            .fetch_all_as(sqlx::query_as(indoc! { r#"
                SELECT plan_id, name FROM plans
            "# }))
            .await?;

        stream::iter(plans)
            .then(|plan| self.with_limits(plan))
            .try_collect()
            .await
    }
}

#[async_trait]
trait PlanInternalRepo: PlanRepo {
    type Db: Database;
    type Tx: LabelledPoolTransaction;

    async fn get_limits(&self, plan_id: &Uuid) -> RepoResult<BTreeMap<UsageType, Option<i64>>>;

    async fn with_limits(&self, mut plan: PlanRecord) -> RepoResult<PlanRecord> {
        plan.limits = self.get_limits(&plan.plan_id).await?;
        Ok(plan.with_limit_placeholders())
    }

    async fn insert_limit(
        tx: &mut Self::Tx,
        plan_id: &Uuid,
        usage_type: UsageType,
        limit: i64,
    ) -> RepoResult<()>;
}

#[trait_gen(PostgresPool -> PostgresPool, SqlitePool)]
#[async_trait]
impl PlanInternalRepo for DbPlanRepo<PostgresPool> {
    type Db = <PostgresPool as Pool>::Db;
    type Tx = <<PostgresPool as Pool>::LabelledApi as LabelledPoolApi>::LabelledTransaction;

    async fn get_limits(&self, plan_id: &Uuid) -> RepoResult<BTreeMap<UsageType, Option<i64>>> {
        let rows = self
            .with_ro("get_limits")
            .fetch_all(
                sqlx::query(indoc! { r#"
                    SELECT usage_type, value FROM plan_usage_limits WHERE plan_id = $1
                "# })
                .bind(plan_id),
            )
            .await?;

        let mut limits = BTreeMap::new();
        for row in rows {
            limits.insert(row.try_get("usage_type")?, Some(row.try_get("value")?));
        }

        Ok(limits)
    }

    async fn insert_limit(
        tx: &mut Self::Tx,
        plan_id: &Uuid,
        usage_type: UsageType,
        limit: i64,
    ) -> RepoResult<()> {
        tx.execute(
            sqlx::query(indoc! { r#"
                INSERT INTO plan_usage_limits (plan_id, usage_type, value)
                VALUES ($1, $2, $3)
            "#})
            .bind(plan_id)
            .bind(usage_type)
            .bind(limit),
        )
        .await?;

        Ok(())
    }
}
