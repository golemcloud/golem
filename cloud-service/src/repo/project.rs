use std::ops::Deref;
use std::result::Result;
use std::sync::Arc;

use async_trait::async_trait;
use golem_common::model::ProjectId;
use sqlx::{Database, Pool, Row};
use uuid::Uuid;

use crate::model::{Project, ProjectData, ProjectType};
use crate::repo::RepoError;

#[derive(sqlx::FromRow, Debug, Clone)]
pub struct ProjectRecord {
    pub project_id: Uuid,
    pub name: String,
    pub owner_account_id: String,
    pub description: String,
    pub is_default: bool,
}

impl From<ProjectRecord> for Project {
    fn from(value: ProjectRecord) -> Self {
        let project_type = if value.is_default {
            ProjectType::Default
        } else {
            ProjectType::NonDefault
        };
        Project {
            project_id: ProjectId(value.project_id),
            project_data: ProjectData {
                name: value.name,
                owner_account_id: value.owner_account_id.as_str().into(),
                description: value.description,
                default_environment_id: "default".to_string(),
                project_type,
            },
        }
    }
}

impl From<Project> for ProjectRecord {
    fn from(value: Project) -> Self {
        Self {
            project_id: value.project_id.0,
            name: value.project_data.name,
            owner_account_id: value.project_data.owner_account_id.value,
            description: value.project_data.description,
            is_default: value.project_data.project_type == ProjectType::Default,
        }
    }
}

#[async_trait]
pub trait ProjectRepo {
    async fn create(&self, project: &ProjectRecord) -> Result<(), RepoError>;

    async fn get(&self, project_id: &Uuid) -> Result<Option<ProjectRecord>, RepoError>;

    async fn get_own(&self, account_id: &str) -> Result<Vec<ProjectRecord>, RepoError>;

    async fn get_own_count(&self, account_id: &str) -> Result<u64, RepoError>;

    async fn get_own_default(&self, account_id: &str) -> Result<Option<ProjectRecord>, RepoError>;

    async fn get_all(&self) -> Result<Vec<ProjectRecord>, RepoError>;

    async fn delete(&self, project_id: &Uuid) -> Result<(), RepoError>;
}

pub struct DbProjectRepo<DB: Database> {
    db_pool: Arc<Pool<DB>>,
}

impl<DB: Database> DbProjectRepo<DB> {
    pub fn new(db_pool: Arc<Pool<DB>>) -> Self {
        Self { db_pool }
    }
}

#[async_trait]
impl ProjectRepo for DbProjectRepo<sqlx::Postgres> {
    async fn create(&self, project: &ProjectRecord) -> Result<(), RepoError> {
        let mut tx = self.db_pool.begin().await?;

        sqlx::query(
            r#"
              INSERT INTO projects
                (project_id, name, description)
              VALUES
                ($1, $2, $3)
            "#,
        )
        .bind(project.project_id)
        .bind(project.name.clone())
        .bind(project.description.clone())
        .execute(&mut *tx)
        .await?;

        sqlx::query(
            r#"
              INSERT INTO project_account
                (project_id, owner_account_id, is_default)
              VALUES
                ($1, $2, $3)
            "#,
        )
        .bind(project.project_id)
        .bind(project.owner_account_id.clone())
        .bind(project.is_default)
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;

        Ok(())
    }

    async fn get(&self, project_id: &Uuid) -> Result<Option<ProjectRecord>, RepoError> {
        sqlx::query_as::<_, ProjectRecord>(
            r#"
               SELECT * FROM project_account pa JOIN projects p ON pa.project_id = p.project_id
               WHERE p.project_id = $1
               "#,
        )
        .bind(project_id)
        .fetch_optional(self.db_pool.deref())
        .await
        .map_err(|e| e.into())
    }

    async fn get_own(&self, account_id: &str) -> Result<Vec<ProjectRecord>, RepoError> {
        sqlx::query_as::<_, ProjectRecord>(
            r#"
               SELECT * FROM project_account pa JOIN projects p ON pa.project_id = p.project_id
               WHERE pa.owner_account_id = $1
               "#,
        )
        .bind(account_id)
        .fetch_all(self.db_pool.deref())
        .await
        .map_err(|e| e.into())
    }

    async fn get_own_count(&self, account_id: &str) -> Result<u64, RepoError> {
        let result = sqlx::query(
            r#"
               SELECT count(distinct p.project_id) AS project_count
               FROM project_account pa JOIN projects p ON pa.project_id = p.project_id
               WHERE pa.owner_account_id = $1
               "#,
        )
        .bind(account_id)
        .fetch_one(self.db_pool.deref())
        .await?;

        let count: i64 = result.get("project_count");
        Ok(count as u64)
    }

    async fn get_own_default(&self, account_id: &str) -> Result<Option<ProjectRecord>, RepoError> {
        sqlx::query_as::<_, ProjectRecord>(
            r#"
               SELECT * FROM project_account pa JOIN projects p ON pa.project_id = p.project_id
               WHERE pa.owner_account_id = $1 AND pa.is_default = true
               "#,
        )
        .bind(account_id)
        .fetch_optional(self.db_pool.deref())
        .await
        .map_err(|e| e.into())
    }

    async fn get_all(&self) -> Result<Vec<ProjectRecord>, RepoError> {
        sqlx::query_as::<_, ProjectRecord>(
            "SELECT * FROM project_account pa JOIN projects p ON pa.project_id = p.project_id",
        )
        .fetch_all(self.db_pool.deref())
        .await
        .map_err(|e| e.into())
    }

    async fn delete(&self, project_id: &Uuid) -> Result<(), RepoError> {
        let mut tx = self.db_pool.begin().await?;

        sqlx::query("DELETE FROM project_account WHERE project_id = $1")
            .bind(project_id)
            .execute(&mut *tx)
            .await?;

        sqlx::query("DELETE FROM project_grants WHERE grantor_project_id = $1")
            .bind(project_id)
            .execute(&mut *tx)
            .await?;

        sqlx::query("DELETE FROM projects WHERE project_id = $1")
            .bind(project_id)
            .execute(&mut *tx)
            .await?;

        tx.commit().await?;

        Ok(())
    }
}

#[async_trait]
impl ProjectRepo for DbProjectRepo<sqlx::Sqlite> {
    async fn create(&self, project: &ProjectRecord) -> Result<(), RepoError> {
        let mut tx = self.db_pool.begin().await?;

        sqlx::query(
            r#"
              INSERT INTO projects
                (project_id, name, description)
              VALUES
                ($1, $2, $3)
            "#,
        )
        .bind(project.project_id)
        .bind(project.name.clone())
        .bind(project.description.clone())
        .execute(&mut *tx)
        .await?;

        sqlx::query(
            r#"
              INSERT INTO project_account
                (project_id, owner_account_id, is_default)
              VALUES
                ($1, $2, $3)
            "#,
        )
        .bind(project.project_id)
        .bind(project.owner_account_id.clone())
        .bind(project.is_default)
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;

        Ok(())
    }

    async fn get(&self, project_id: &Uuid) -> Result<Option<ProjectRecord>, RepoError> {
        sqlx::query_as::<_, ProjectRecord>(
            r#"
               SELECT * FROM project_account pa JOIN projects p ON pa.project_id = p.project_id
               WHERE p.project_id = $1
               "#,
        )
        .bind(project_id)
        .fetch_optional(self.db_pool.deref())
        .await
        .map_err(|e| e.into())
    }

    async fn get_own(&self, account_id: &str) -> Result<Vec<ProjectRecord>, RepoError> {
        sqlx::query_as::<_, ProjectRecord>(
            r#"
               SELECT * FROM project_account pa JOIN projects p ON pa.project_id = p.project_id
               WHERE pa.owner_account_id = $1
               "#,
        )
        .bind(account_id)
        .fetch_all(self.db_pool.deref())
        .await
        .map_err(|e| e.into())
    }

    async fn get_own_count(&self, account_id: &str) -> Result<u64, RepoError> {
        let result = sqlx::query(
            r#"
               SELECT count(distinct p.project_id) AS project_count
               FROM project_account pa JOIN projects p ON pa.project_id = p.project_id
               WHERE pa.owner_account_id = $1
               "#,
        )
        .bind(account_id)
        .fetch_one(self.db_pool.deref())
        .await?;

        let count: i64 = result.get("project_count");
        Ok(count as u64)
    }

    async fn get_own_default(&self, account_id: &str) -> Result<Option<ProjectRecord>, RepoError> {
        sqlx::query_as::<_, ProjectRecord>(
            r#"
               SELECT * FROM project_account pa JOIN projects p ON pa.project_id = p.project_id
               WHERE pa.owner_account_id = $1 AND pa.is_default = true
               "#,
        )
        .bind(account_id)
        .fetch_optional(self.db_pool.deref())
        .await
        .map_err(|e| e.into())
    }

    async fn get_all(&self) -> Result<Vec<ProjectRecord>, RepoError> {
        sqlx::query_as::<_, ProjectRecord>(
            "SELECT * FROM project_account pa JOIN projects p ON pa.project_id = p.project_id",
        )
        .fetch_all(self.db_pool.deref())
        .await
        .map_err(|e| e.into())
    }

    async fn delete(&self, project_id: &Uuid) -> Result<(), RepoError> {
        let mut tx = self.db_pool.begin().await?;

        sqlx::query("DELETE FROM project_account WHERE project_id = $1")
            .bind(project_id)
            .execute(&mut *tx)
            .await?;

        sqlx::query("DELETE FROM project_grants WHERE grantor_project_id = $1")
            .bind(project_id)
            .execute(&mut *tx)
            .await?;

        sqlx::query("DELETE FROM projects WHERE project_id = $1")
            .bind(project_id)
            .execute(&mut *tx)
            .await?;

        tx.commit().await?;

        Ok(())
    }
}
