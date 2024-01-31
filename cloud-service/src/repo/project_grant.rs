use std::ops::Deref;
use std::result::Result;
use std::sync::Arc;

use async_trait::async_trait;
use cloud_common::model::{ProjectGrantId, ProjectPolicyId};
use golem_common::model::ProjectId;
use sqlx::{Database, Pool};
use uuid::Uuid;

use crate::model::{ProjectGrant, ProjectGrantData};
use crate::repo::RepoError;

#[derive(sqlx::FromRow, Debug, Clone)]
pub struct ProjectGrantRecord {
    pub project_grant_id: Uuid,
    pub grantor_project_id: Uuid,
    pub grantee_account_id: String,
    pub project_policy_id: Uuid,
}

impl From<ProjectGrantRecord> for ProjectGrant {
    fn from(value: ProjectGrantRecord) -> Self {
        ProjectGrant {
            id: ProjectGrantId(value.project_grant_id),
            data: ProjectGrantData {
                grantor_project_id: ProjectId(value.grantor_project_id),
                grantee_account_id: value.grantee_account_id.as_str().into(),
                project_policy_id: ProjectPolicyId(value.project_policy_id),
            },
        }
    }
}

impl From<ProjectGrant> for ProjectGrantRecord {
    fn from(value: ProjectGrant) -> Self {
        Self {
            project_grant_id: value.id.0,
            grantor_project_id: value.data.grantor_project_id.0,
            grantee_account_id: value.data.grantee_account_id.value,
            project_policy_id: value.data.project_policy_id.0,
        }
    }
}

#[async_trait]
pub trait ProjectGrantRepo {
    async fn create(&self, project_grant: &ProjectGrantRecord) -> Result<(), RepoError>;

    async fn get(&self, project_grant_id: &Uuid) -> Result<Option<ProjectGrantRecord>, RepoError>;

    async fn get_by_account(
        &self,
        grantee_account_id: &str,
    ) -> Result<Vec<ProjectGrantRecord>, RepoError>;

    async fn get_by_project(&self, project_id: &Uuid)
        -> Result<Vec<ProjectGrantRecord>, RepoError>;

    async fn delete(&self, project_grant_id: &Uuid) -> Result<(), RepoError>;
}

pub struct DbProjectGrantRepo<DB: Database> {
    db_pool: Arc<Pool<DB>>,
}

impl<DB: Database> DbProjectGrantRepo<DB> {
    pub fn new(db_pool: Arc<Pool<DB>>) -> Self {
        Self { db_pool }
    }
}

#[async_trait]
impl ProjectGrantRepo for DbProjectGrantRepo<sqlx::Postgres> {
    async fn create(&self, project_grant: &ProjectGrantRecord) -> Result<(), RepoError> {
        sqlx::query(
            r#"
              INSERT INTO project_grants
                (project_grant_id, grantor_project_id, project_policy_id, grantee_account_id)
              VALUES
                ($1, $2, $3, $4)
            "#,
        )
        .bind(project_grant.project_grant_id)
        .bind(project_grant.grantor_project_id)
        .bind(project_grant.project_policy_id)
        .bind(project_grant.grantee_account_id.clone())
        .execute(self.db_pool.deref())
        .await?;

        Ok(())
    }

    async fn get(&self, project_grant_id: &Uuid) -> Result<Option<ProjectGrantRecord>, RepoError> {
        sqlx::query_as::<_, ProjectGrantRecord>(
            "SELECT * FROM project_grants WHERE project_grant_id = $1",
        )
        .bind(project_grant_id)
        .fetch_optional(self.db_pool.deref())
        .await
        .map_err(|e| e.into())
    }

    async fn get_by_project(
        &self,
        project_id: &Uuid,
    ) -> Result<Vec<ProjectGrantRecord>, RepoError> {
        sqlx::query_as::<_, ProjectGrantRecord>(
            "SELECT * FROM project_grants WHERE grantor_project_id = $1",
        )
        .bind(project_id)
        .fetch_all(self.db_pool.deref())
        .await
        .map_err(|e| e.into())
    }

    async fn get_by_account(
        &self,
        grantee_account_id: &str,
    ) -> Result<Vec<ProjectGrantRecord>, RepoError> {
        sqlx::query_as::<_, ProjectGrantRecord>(
            "SELECT * FROM project_grants WHERE grantee_account_id = $1",
        )
        .bind(grantee_account_id)
        .fetch_all(self.db_pool.deref())
        .await
        .map_err(|e| e.into())
    }

    async fn delete(&self, project_grant_id: &Uuid) -> Result<(), RepoError> {
        sqlx::query("DELETE FROM project_grants WHERE project_grant_id = $1")
            .bind(project_grant_id)
            .execute(self.db_pool.deref())
            .await?;
        Ok(())
    }
}

#[async_trait]
impl ProjectGrantRepo for DbProjectGrantRepo<sqlx::Sqlite> {
    async fn create(&self, project_grant: &ProjectGrantRecord) -> Result<(), RepoError> {
        sqlx::query(
            r#"
              INSERT INTO project_grants
                (project_grant_id, grantor_project_id, project_policy_id, grantee_account_id)
              VALUES
                ($1, $2, $3, $4)
            "#,
        )
        .bind(project_grant.project_grant_id)
        .bind(project_grant.grantor_project_id)
        .bind(project_grant.project_policy_id)
        .bind(project_grant.grantee_account_id.clone())
        .execute(self.db_pool.deref())
        .await?;

        Ok(())
    }

    async fn get(&self, project_grant_id: &Uuid) -> Result<Option<ProjectGrantRecord>, RepoError> {
        sqlx::query_as::<_, ProjectGrantRecord>(
            "SELECT * FROM project_grants WHERE project_grant_id = $1",
        )
        .bind(project_grant_id)
        .fetch_optional(self.db_pool.deref())
        .await
        .map_err(|e| e.into())
    }

    async fn get_by_project(
        &self,
        project_id: &Uuid,
    ) -> Result<Vec<ProjectGrantRecord>, RepoError> {
        sqlx::query_as::<_, ProjectGrantRecord>(
            "SELECT * FROM project_grants WHERE grantor_project_id = $1",
        )
        .bind(project_id)
        .fetch_all(self.db_pool.deref())
        .await
        .map_err(|e| e.into())
    }

    async fn get_by_account(
        &self,
        grantee_account_id: &str,
    ) -> Result<Vec<ProjectGrantRecord>, RepoError> {
        sqlx::query_as::<_, ProjectGrantRecord>(
            "SELECT * FROM project_grants WHERE grantee_account_id = $1",
        )
        .bind(grantee_account_id)
        .fetch_all(self.db_pool.deref())
        .await
        .map_err(|e| e.into())
    }

    async fn delete(&self, project_grant_id: &Uuid) -> Result<(), RepoError> {
        sqlx::query("DELETE FROM project_grants WHERE project_grant_id = $1")
            .bind(project_grant_id)
            .execute(self.db_pool.deref())
            .await?;
        Ok(())
    }
}
