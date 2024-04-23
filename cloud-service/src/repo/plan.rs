use std::ops::Deref;
use std::result::Result;
use std::sync::Arc;

use async_trait::async_trait;
use cloud_common::model::PlanId;
use sqlx::{Database, Pool};
use uuid::Uuid;

use crate::model::{Plan, PlanData};
use crate::repo::RepoError;

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
pub trait PlanRepo {
    async fn create(&self, plan: &PlanRecord) -> Result<(), RepoError>;

    async fn update(&self, plan: &PlanRecord) -> Result<(), RepoError>;

    async fn get(&self, plan_id: &Uuid) -> Result<Option<PlanRecord>, RepoError>;

    async fn get_all(&self) -> Result<Vec<PlanRecord>, RepoError>;

    async fn delete(&self, plan_id: &Uuid) -> Result<(), RepoError>;
}

pub struct DbPlanRepo<DB: Database> {
    db_pool: Arc<Pool<DB>>,
}

impl<DB: Database> DbPlanRepo<DB> {
    pub fn new(db_pool: Arc<Pool<DB>>) -> Self {
        Self { db_pool }
    }
}

#[async_trait]
impl PlanRepo for DbPlanRepo<sqlx::Postgres> {
    async fn create(&self, plan: &PlanRecord) -> Result<(), RepoError> {
        sqlx::query(r#"
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
            .bind(plan.monthly_upload_limit)
            .execute(self.db_pool.deref())
            .await?;

        Ok(())
    }

    async fn update(&self, plan: &PlanRecord) -> Result<(), RepoError> {
        sqlx::query(r#"
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
            .bind(plan.monthly_upload_limit)
            .execute(self.db_pool.deref())
            .await?;

        Ok(())
    }

    async fn get(&self, plan_id: &Uuid) -> Result<Option<PlanRecord>, RepoError> {
        sqlx::query_as::<_, PlanRecord>("SELECT * FROM plans WHERE plan_id = $1")
            .bind(plan_id)
            .fetch_optional(self.db_pool.deref())
            .await
            .map_err(|e| e.into())
    }

    async fn get_all(&self) -> Result<Vec<PlanRecord>, RepoError> {
        sqlx::query_as::<_, PlanRecord>("SELECT * FROM plans")
            .fetch_all(self.db_pool.deref())
            .await
            .map_err(|e| e.into())
    }

    async fn delete(&self, plan_id: &Uuid) -> Result<(), RepoError> {
        sqlx::query("DELETE FROM plans WHERE plan_id = $1")
            .bind(plan_id)
            .execute(self.db_pool.deref())
            .await?;
        Ok(())
    }
}

#[async_trait]
impl PlanRepo for DbPlanRepo<sqlx::Sqlite> {
    async fn create(&self, plan: &PlanRecord) -> Result<(), RepoError> {
        sqlx::query(r#"
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
            .bind(plan.monthly_upload_limit)
            .execute(self.db_pool.deref())
            .await?;

        Ok(())
    }

    async fn update(&self, plan: &PlanRecord) -> Result<(), RepoError> {
        sqlx::query(r#"
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
            .bind(plan.monthly_upload_limit)
            .execute(self.db_pool.deref())
            .await?;

        Ok(())
    }

    async fn get(&self, plan_id: &Uuid) -> Result<Option<PlanRecord>, RepoError> {
        sqlx::query_as::<_, PlanRecord>("SELECT * FROM plans WHERE plan_id = $1")
            .bind(plan_id)
            .fetch_optional(self.db_pool.deref())
            .await
            .map_err(|e| e.into())
    }

    async fn get_all(&self) -> Result<Vec<PlanRecord>, RepoError> {
        sqlx::query_as::<_, PlanRecord>("SELECT * FROM plans")
            .fetch_all(self.db_pool.deref())
            .await
            .map_err(|e| e.into())
    }

    async fn delete(&self, plan_id: &Uuid) -> Result<(), RepoError> {
        sqlx::query("DELETE FROM plans WHERE plan_id = $1")
            .bind(plan_id)
            .execute(self.db_pool.deref())
            .await?;
        Ok(())
    }
}
