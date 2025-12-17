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

use crate::repo::model::plan::PlanRecord;
use async_trait::async_trait;
use conditional_trait_gen::trait_gen;
use futures::FutureExt;
use futures::future::BoxFuture;
use golem_service_base::db::postgres::PostgresPool;
use golem_service_base::db::sqlite::SqlitePool;
use golem_service_base::db::{LabelledPoolApi, Pool};
use golem_service_base::repo::RepoResult;
use indoc::indoc;
use tracing::{Instrument, Span, info_span};
use uuid::Uuid;

#[async_trait]
pub trait PlanRepo: Send + Sync {
    async fn create_or_update(&self, plan: PlanRecord) -> RepoResult<()>;

    async fn get_by_id(&self, plan_id: Uuid) -> RepoResult<Option<PlanRecord>>;

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

    pub fn span_id(plan_id: Uuid) -> Span {
        info_span!(SPAN_NAME, plan_id=%plan_id)
    }
}

#[async_trait]
impl<Repo: PlanRepo> PlanRepo for LoggedPlanRepo<Repo> {
    async fn create_or_update(&self, plan: PlanRecord) -> RepoResult<()> {
        let span = Self::span_id(plan.plan_id);
        self.repo.create_or_update(plan).instrument(span).await
    }

    async fn get_by_id(&self, plan_id: Uuid) -> RepoResult<Option<PlanRecord>> {
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
                        INSERT INTO plans (
                            plan_id, name, max_memory_per_worker, total_app_count,
                            total_env_count, total_component_count, total_worker_count, total_worker_connection_count,
                            total_component_storage_bytes, monthly_gas_limit, monthly_component_upload_limit_bytes
                        )
                        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
                        ON CONFLICT (plan_id) DO UPDATE SET
                            name = $2,
                            max_memory_per_worker = $3,
                            total_app_count = $4,
                            total_env_count = $5,
                            total_component_count = $6,
                            total_worker_count = $7,
                            total_worker_connection_count = $8,
                            total_component_storage_bytes = $9,
                            monthly_gas_limit = $10,
                            monthly_component_upload_limit_bytes = $11
                    "#})
                    .bind(plan.plan_id)
                    .bind(plan.name)
                    .bind(plan.max_memory_per_worker)
                    .bind(plan.total_app_count)
                    .bind(plan.total_env_count)
                    .bind(plan.total_component_count)
                    .bind(plan.total_worker_count)
                    .bind(plan.total_worker_connection_count)
                    .bind(plan.total_component_storage_bytes)
                    .bind(plan.monthly_gas_limit)
                    .bind(plan.monthly_component_upload_limit_bytes)
                )
                .await?;

                Ok(())
            }
            .boxed()
        })
        .await
    }

    async fn get_by_id(&self, plan_id: Uuid) -> RepoResult<Option<PlanRecord>> {
        let plan: Option<PlanRecord> = self
            .with_ro("get_by_id")
            .fetch_optional_as(
                sqlx::query_as(indoc! { r#"
                    SELECT
                        plan_id, name, max_memory_per_worker, total_app_count,
                        total_env_count, total_component_count, total_worker_count, total_worker_connection_count,
                        total_component_storage_bytes, monthly_gas_limit, monthly_component_upload_limit_bytes
                    FROM plans
                    WHERE plan_id = $1
                "# })
                .bind(plan_id),
            )
            .await?;

        match plan {
            Some(plan) => Ok(Some(plan)),
            None => Ok(None),
        }
    }

    async fn list(&self) -> RepoResult<Vec<PlanRecord>> {
        let plans = self
            .with_ro("list")
            .fetch_all_as(sqlx::query_as(indoc! { r#"
                SELECT
                    plan_id, name, max_memory_per_worker, total_app_count,
                    total_env_count, total_component_count, total_worker_count, total_worker_connection_count,
                    total_component_storage_bytes, monthly_gas_limit, monthly_component_upload_limit_bytes
                FROM plans
            "# }))
            .await?;

        Ok(plans)
    }
}
