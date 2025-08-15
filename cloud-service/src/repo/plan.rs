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

use crate::model::{Plan, PlanData};
use async_trait::async_trait;
use conditional_trait_gen::trait_gen;
use golem_common::model::PlanId;
use golem_service_base::db::Pool;
use golem_service_base::repo::RepoError;
use std::result::Result;
use uuid::Uuid;

#[derive(sqlx::FromRow, Debug, Clone)]
pub struct PlanRecord {
    pub plan_id: Uuid,
    pub project_limit: i32,
    pub component_limit: i32,
    pub worker_limit: i32,
    pub storage_limit: i32,
    pub monthly_gas_limit: i64,
    pub monthly_upload_limit: i32,
}

impl From<PlanRecord> for Plan {
    fn from(value: PlanRecord) -> Self {
        Plan {
            plan_id: PlanId(value.plan_id),
            plan_data: PlanData {
                project_limit: value.project_limit,
                component_limit: value.component_limit,
                worker_limit: value.worker_limit,
                storage_limit: value.storage_limit,
                monthly_gas_limit: value.monthly_gas_limit,
                monthly_upload_limit: value.monthly_upload_limit,
            },
        }
    }
}

impl From<Plan> for PlanRecord {
    fn from(value: Plan) -> Self {
        Self {
            plan_id: value.plan_id.0,
            project_limit: value.plan_data.project_limit,
            component_limit: value.plan_data.component_limit,
            worker_limit: value.plan_data.worker_limit,
            storage_limit: value.plan_data.storage_limit,
            monthly_gas_limit: value.plan_data.monthly_gas_limit,
            monthly_upload_limit: value.plan_data.monthly_upload_limit,
        }
    }
}

#[async_trait]
pub trait PlanRepo: Send + Sync {
    async fn create(&self, plan: &PlanRecord) -> Result<(), RepoError>;

    async fn update(&self, plan: &PlanRecord) -> Result<(), RepoError>;

    async fn get(&self, plan_id: &Uuid) -> Result<Option<PlanRecord>, RepoError>;

    async fn get_all(&self) -> Result<Vec<PlanRecord>, RepoError>;

    async fn delete(&self, plan_id: &Uuid) -> Result<(), RepoError>;
}

pub struct DbPlanRepo<DB: Pool> {
    db_pool: DB,
}

impl<DB: Pool> DbPlanRepo<DB> {
    pub fn new(db_pool: DB) -> Self {
        Self { db_pool }
    }
}

#[trait_gen(golem_service_base::db::postgres::PostgresPool -> golem_service_base::db::postgres::PostgresPool, golem_service_base::db::sqlite::SqlitePool
)]
#[async_trait]
impl PlanRepo for DbPlanRepo<golem_service_base::db::postgres::PostgresPool> {
    async fn create(&self, plan: &PlanRecord) -> Result<(), RepoError> {
        let query = sqlx::query(r#"
              INSERT INTO plans
                (plan_id, project_limit, component_limit, worker_limit, storage_limit, monthly_gas_limit, monthly_upload_limit)
              VALUES
                ($1, $2, $3, $4, $5, $6, $7)
            "#)
            .bind(plan.plan_id)
            .bind(plan.project_limit)
            .bind(plan.component_limit)
            .bind(plan.worker_limit)
            .bind(plan.storage_limit)
            .bind(plan.monthly_gas_limit)
            .bind(plan.monthly_upload_limit);

        self.db_pool
            .with_rw("plan", "create")
            .execute(query)
            .await?;

        Ok(())
    }

    async fn update(&self, plan: &PlanRecord) -> Result<(), RepoError> {
        let query = sqlx::query(r#"
              INSERT INTO plans
                (plan_id, project_limit, component_limit, worker_limit, storage_limit, monthly_gas_limit, monthly_upload_limit)
              VALUES
                ($1, $2, $3, $4, $5, $6, $7)
              ON CONFLICT (plan_id) DO UPDATE
              SET project_limit = $2,
                  component_limit = $3,
                  worker_limit = $4,
                  storage_limit = $5,
                  monthly_gas_limit = $6,
                  monthly_upload_limit = $7
            "#)
            .bind(plan.plan_id)
            .bind(plan.project_limit)
            .bind(plan.component_limit)
            .bind(plan.worker_limit)
            .bind(plan.storage_limit)
            .bind(plan.monthly_gas_limit)
            .bind(plan.monthly_upload_limit);

        self.db_pool
            .with_rw("plan", "update")
            .execute(query)
            .await?;

        Ok(())
    }

    async fn get(&self, plan_id: &Uuid) -> Result<Option<PlanRecord>, RepoError> {
        let query =
            sqlx::query_as::<_, PlanRecord>("SELECT * FROM plans WHERE plan_id = $1").bind(plan_id);

        self.db_pool
            .with_ro("plan", "get")
            .fetch_optional_as(query)
            .await
    }

    async fn get_all(&self) -> Result<Vec<PlanRecord>, RepoError> {
        let query = sqlx::query_as::<_, PlanRecord>("SELECT * FROM plans");

        self.db_pool
            .with_ro("plan", "get_all")
            .fetch_all(query)
            .await
    }

    async fn delete(&self, plan_id: &Uuid) -> Result<(), RepoError> {
        let query = sqlx::query("DELETE FROM plans WHERE plan_id = $1").bind(plan_id);

        self.db_pool
            .with_rw("plan", "delete")
            .execute(query)
            .await?;

        Ok(())
    }
}
