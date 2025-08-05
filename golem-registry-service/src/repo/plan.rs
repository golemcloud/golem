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
use golem_service_base::db::postgres::PostgresPool;
use golem_service_base::db::sqlite::SqlitePool;
use golem_service_base::db::{Pool, PoolApi};
use golem_service_base::repo;
use golem_service_base::repo::RepoError;
use indoc::indoc;
use tracing::{Instrument, Span, info_span};
use uuid::Uuid;

#[async_trait]
pub trait PlanRepository: Send + Sync {
    async fn create(&self, plan: PlanRecord) -> Result<PlanRecord, RepoError>;
}

pub struct LoggedPlanRepository<Repo: PlanRepository> {
    repo: Repo,
}

static SPAN_NAME: &str = "plan repository";

impl<Repo: PlanRepository> LoggedPlanRepository<Repo> {
    pub fn new(repo: Repo) -> Self {
        Self { repo }
    }

    pub fn span(plan_id: &Uuid) -> Span {
        info_span!(SPAN_NAME, plan_id=%plan_id)
    }
}

#[async_trait]
impl<Repo: PlanRepository> PlanRepository for LoggedPlanRepository<Repo> {
    async fn create(&self, plan: PlanRecord) -> repo::Result<PlanRecord> {
        let span = Self::span(&plan.plan_id);
        self.repo.create(plan).instrument(span).await
    }
}

pub struct DbPlanRepository<DBP: Pool> {
    db_pool: DBP,
}

static METRICS_SVC_NAME: &str = "plan";

impl<DBP: Pool> DbPlanRepository<DBP> {
    pub fn new(db_pool: DBP) -> Self {
        Self { db_pool }
    }

    pub fn logged(db_pool: DBP) -> LoggedPlanRepository<Self>
    where
        Self: PlanRepository,
    {
        LoggedPlanRepository::new(Self::new(db_pool))
    }

    fn with_rw(&self, api_name: &'static str) -> DBP::LabelledApi {
        self.db_pool.with_rw(METRICS_SVC_NAME, api_name)
    }
}

#[trait_gen(PostgresPool -> PostgresPool, SqlitePool)]
#[async_trait]
impl PlanRepository for DbPlanRepository<PostgresPool> {
    async fn create(&self, plan: PlanRecord) -> repo::Result<PlanRecord> {
        self.with_rw("create")
            .fetch_one_as(
                sqlx::query_as(indoc! {r#"
                    INSERT INTO plans (plan_id, name)
                    VALUES ($1, $2)
                    RETURNING plan_id, name
                "#})
                .bind(plan.plan_id)
                .bind(plan.name),
            )
            .await
    }
}
