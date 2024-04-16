use std::ops::Deref;
use std::result::Result;
use std::sync::Arc;

use async_trait::async_trait;
use golem_common::model::ProjectId;
use golem_common::model::TemplateId;
use sqlx::{Database, Pool, Row};
use uuid::Uuid;

use crate::repo::RepoError;
use golem_service_base::model::*;

#[derive(sqlx::FromRow, Debug, Clone)]
pub struct TemplateRecord {
    pub template_id: Uuid,
    pub name: String,
    pub size: i32,
    pub version: i64,
    pub user_template: String,
    pub protected_template: String,
    pub protector_version: Option<i64>,
    pub metadata: String,
    pub project_id: Uuid,
}

impl TryFrom<TemplateRecord> for crate::model::Template {
    type Error = String;

    fn try_from(value: TemplateRecord) -> Result<Self, Self::Error> {
        let metadata: TemplateMetadata = serde_json::from_str(&value.metadata)
            .map_err(|e| format!("Invalid Template Metadata: {}", e))?;
        let versioned_template_id: VersionedTemplateId = VersionedTemplateId {
            template_id: TemplateId(value.template_id),
            version: value.version as u64,
        };
        let protected_template_id: ProtectedTemplateId = ProtectedTemplateId {
            versioned_template_id: versioned_template_id.clone(),
        };
        let user_template_id: UserTemplateId = UserTemplateId {
            versioned_template_id: versioned_template_id.clone(),
        };
        Ok(crate::model::Template {
            template_name: TemplateName(value.name),
            template_size: value.size,
            project_id: ProjectId(value.project_id),
            metadata,
            versioned_template_id,
            user_template_id,
            protected_template_id,
        })
    }
}

impl From<crate::model::Template> for TemplateRecord {
    fn from(value: crate::model::Template) -> Self {
        Self {
            template_id: value.versioned_template_id.template_id.0,
            name: value.template_name.0,
            size: value.template_size,
            version: value.versioned_template_id.version as i64,
            user_template: value.versioned_template_id.slug(),
            protected_template: value.protected_template_id.slug(),
            protector_version: None,
            metadata: serde_json::to_string(&value.metadata).unwrap(),
            project_id: value.project_id.0,
        }
    }
}

#[async_trait]
pub trait TemplateRepo {
    async fn upsert(&self, template: &TemplateRecord) -> Result<(), RepoError>;

    async fn get(&self, template_id: &Uuid) -> Result<Vec<TemplateRecord>, RepoError>;

    async fn get_latest_version(
        &self,
        template_id: &Uuid,
    ) -> Result<Option<TemplateRecord>, RepoError>;

    async fn get_by_version(
        &self,
        template_id: &Uuid,
        version: u64,
    ) -> Result<Option<TemplateRecord>, RepoError>;

    async fn get_by_project(&self, project_id: &Uuid) -> Result<Vec<TemplateRecord>, RepoError>;

    async fn get_by_project_and_name(
        &self,
        project_id: &Uuid,
        name: &str,
    ) -> Result<Vec<TemplateRecord>, RepoError>;

    async fn get_count_by_projects(&self, project_ids: Vec<Uuid>) -> Result<u64, RepoError>;

    async fn get_size_by_projects(&self, project_ids: Vec<Uuid>) -> Result<u64, RepoError>;

    async fn delete(&self, template_id: &Uuid) -> Result<(), RepoError>;
}

pub struct DbTemplateRepo<DB: Database> {
    db_pool: Arc<Pool<DB>>,
}

impl<DB: Database> DbTemplateRepo<DB> {
    pub fn new(db_pool: Arc<Pool<DB>>) -> Self {
        Self { db_pool }
    }
}

#[async_trait]
impl TemplateRepo for DbTemplateRepo<sqlx::Postgres> {
    async fn upsert(&self, template: &TemplateRecord) -> Result<(), RepoError> {
        sqlx::query(
            r#"
              INSERT INTO templates
                (template_id, version, project_id, name, size, user_template, protected_template, protector_version, metadata)
              VALUES
                ($1, $2, $3, $4, $5, $6, $7, $8, $9::jsonb)
              ON CONFLICT (template_id, version) DO UPDATE
              SET project_id = $3,
                  name = $4,
                  size = $5,
                  user_template = $6,
                  protected_template = $7,
                  protector_version = $8,
                  metadata = $9::jsonb
            "#,
        )
            .bind(template.template_id)
            .bind(template.version)
            .bind(template.project_id)
            .bind(template.name.clone())
            .bind(template.size)
            .bind(template.user_template.clone())
            .bind(template.protected_template.clone())
            .bind(template.protector_version)
            .bind(template.metadata.clone())
            .execute(self.db_pool.deref())
            .await?;

        Ok(())
    }

    async fn get(&self, template_id: &Uuid) -> Result<Vec<TemplateRecord>, RepoError> {
        sqlx::query_as::<_, TemplateRecord>("SELECT template_id, version, project_id, name, size, user_template, protected_template, protector_version, jsonb_pretty(templates.metadata) AS metadata  FROM templates WHERE template_id = $1")
            .bind(template_id)
            .fetch_all(self.db_pool.deref())
            .await
            .map_err(|e| e.into())
    }

    async fn get_latest_version(
        &self,
        template_id: &Uuid,
    ) -> Result<Option<TemplateRecord>, RepoError> {
        sqlx::query_as::<_, TemplateRecord>(
            "SELECT template_id, version, project_id, name, size, user_template, protected_template, protector_version, jsonb_pretty(templates.metadata) AS metadata FROM templates WHERE template_id = $1 ORDER BY version DESC LIMIT 1",
        )
        .bind(template_id)
        .fetch_optional(self.db_pool.deref())
        .await
        .map_err(|e| e.into())
    }

    async fn get_by_version(
        &self,
        template_id: &Uuid,
        version: u64,
    ) -> Result<Option<TemplateRecord>, RepoError> {
        sqlx::query_as::<_, TemplateRecord>(
            "SELECT template_id, version, project_id, name, size, user_template, protected_template, protector_version, jsonb_pretty(templates.metadata) AS metadata  FROM templates WHERE template_id = $1 AND version = $2",
        )
        .bind(template_id)
        .bind(version as i64)
        .fetch_optional(self.db_pool.deref())
        .await
        .map_err(|e| e.into())
    }

    async fn get_by_project(&self, project_id: &Uuid) -> Result<Vec<TemplateRecord>, RepoError> {
        sqlx::query_as::<_, TemplateRecord>("SELECT template_id, version, project_id, name, size, user_template, protected_template, protector_version, jsonb_pretty(templates.metadata) AS metadata  FROM templates WHERE project_id = $1")
            .bind(project_id)
            .fetch_all(self.db_pool.deref())
            .await
            .map_err(|e| e.into())
    }

    async fn get_by_project_and_name(
        &self,
        project_id: &Uuid,
        name: &str,
    ) -> Result<Vec<TemplateRecord>, RepoError> {
        sqlx::query_as::<_, TemplateRecord>(
            "SELECT template_id, version, project_id, name, size, user_template, protected_template, protector_version, jsonb_pretty(templates.metadata) AS metadata FROM templates WHERE project_id = $1 AND name = $2",
        )
        .bind(project_id)
        .bind(name)
        .fetch_all(self.db_pool.deref())
        .await
        .map_err(|e| e.into())
    }

    async fn get_count_by_projects(&self, project_ids: Vec<Uuid>) -> Result<u64, RepoError> {
        if project_ids.is_empty() {
            Ok(0)
        } else {
            let params = (1..=project_ids.len())
                .map(|i| format!("${}", i))
                .collect::<Vec<_>>()
                .join(", ");
            let query_str = format!(
                r#"
               SELECT count(distinct template_id) AS template_count
               FROM templates
               WHERE project_id IN ( { } )
               "#,
                params
            );

            let mut query = sqlx::query(&query_str);
            for id in project_ids {
                query = query.bind(id);
            }

            let result = query.fetch_one(self.db_pool.deref()).await?;

            let count: i64 = result.get("template_count");
            Ok(count as u64)
        }
    }

    async fn get_size_by_projects(&self, project_ids: Vec<Uuid>) -> Result<u64, RepoError> {
        if project_ids.is_empty() {
            Ok(0)
        } else {
            let params = (1..=project_ids.len())
                .map(|i| format!("${}", i))
                .collect::<Vec<_>>()
                .join(", ");
            let query_str = format!(
                r#"
               SELECT sum(size) AS template_size
               FROM templates
               WHERE project_id IN ( { } )
               "#,
                params
            );

            let mut query = sqlx::query(&query_str);
            for id in project_ids {
                query = query.bind(id);
            }

            let result = query.fetch_one(self.db_pool.deref()).await?;

            let size: i64 = result.try_get("template_size").unwrap_or(0);
            Ok(size as u64)
        }
    }

    async fn delete(&self, template_id: &Uuid) -> Result<(), RepoError> {
        sqlx::query("DELETE FROM templates WHERE template_id = $1")
            .bind(template_id)
            .execute(self.db_pool.deref())
            .await?;
        Ok(())
    }
}

#[async_trait]
impl TemplateRepo for DbTemplateRepo<sqlx::Sqlite> {
    async fn upsert(&self, template: &TemplateRecord) -> Result<(), RepoError> {
        sqlx::query(
            r#"
              INSERT INTO templates
                (template_id, version, project_id, name, size, user_template, protected_template, protector_version, metadata)
              VALUES
                ($1, $2, $3, $4, $5, $6, $7, $8, $9::jsonb)
              ON CONFLICT (template_id, version) DO UPDATE
              SET project_id = $3,
                  name = $4,
                  size = $5,
                  user_template = $6,
                  protected_template = $7,
                  protector_version = $8,
                  metadata = $9::jsonb
            "#,
        )
            .bind(template.template_id)
            .bind(template.version)
            .bind(template.project_id)
            .bind(template.name.clone())
            .bind(template.size)
            .bind(template.user_template.clone())
            .bind(template.protected_template.clone())
            .bind(template.protector_version)
            .bind(template.metadata.clone())
            .execute(self.db_pool.deref())
            .await?;

        Ok(())
    }

    async fn get(&self, template_id: &Uuid) -> Result<Vec<TemplateRecord>, RepoError> {
        sqlx::query_as::<_, TemplateRecord>("SELECT template_id, version, project_id, name, size, user_template, protected_template, protector_version, jsonb_pretty(templates.metadata) AS metadata  FROM templates WHERE template_id = $1")
            .bind(template_id)
            .fetch_all(self.db_pool.deref())
            .await
            .map_err(|e| e.into())
    }

    async fn get_latest_version(
        &self,
        template_id: &Uuid,
    ) -> Result<Option<TemplateRecord>, RepoError> {
        sqlx::query_as::<_, TemplateRecord>(
            "SELECT template_id, version, project_id, name, size, user_template, protected_template, protector_version, jsonb_pretty(templates.metadata) AS metadata FROM templates WHERE template_id = $1 ORDER BY version DESC LIMIT 1",
        )
            .bind(template_id)
            .fetch_optional(self.db_pool.deref())
            .await
            .map_err(|e| e.into())
    }

    async fn get_by_version(
        &self,
        template_id: &Uuid,
        version: u64,
    ) -> Result<Option<TemplateRecord>, RepoError> {
        sqlx::query_as::<_, TemplateRecord>(
            "SELECT template_id, version, project_id, name, size, user_template, protected_template, protector_version, jsonb_pretty(templates.metadata) AS metadata  FROM templates WHERE template_id = $1 AND version = $2",
        )
            .bind(template_id)
            .bind(version as i64)
            .fetch_optional(self.db_pool.deref())
            .await
            .map_err(|e| e.into())
    }

    async fn get_by_project(&self, project_id: &Uuid) -> Result<Vec<TemplateRecord>, RepoError> {
        sqlx::query_as::<_, TemplateRecord>("SELECT template_id, version, project_id, name, size, user_template, protected_template, protector_version, jsonb_pretty(templates.metadata) AS metadata  FROM templates WHERE project_id = $1")
            .bind(project_id)
            .fetch_all(self.db_pool.deref())
            .await
            .map_err(|e| e.into())
    }

    async fn get_by_project_and_name(
        &self,
        project_id: &Uuid,
        name: &str,
    ) -> Result<Vec<TemplateRecord>, RepoError> {
        sqlx::query_as::<_, TemplateRecord>(
            "SELECT template_id, version, project_id, name, size, user_template, protected_template, protector_version, jsonb_pretty(templates.metadata) AS metadata FROM templates WHERE project_id = $1 AND name = $2",
        )
            .bind(project_id)
            .bind(name)
            .fetch_all(self.db_pool.deref())
            .await
            .map_err(|e| e.into())
    }

    async fn get_count_by_projects(&self, project_ids: Vec<Uuid>) -> Result<u64, RepoError> {
        if project_ids.is_empty() {
            Ok(0)
        } else {
            let params = (1..=project_ids.len())
                .map(|i| format!("${}", i))
                .collect::<Vec<_>>()
                .join(", ");
            let query_str = format!(
                r#"
               SELECT count(distinct template_id) AS template_count
               FROM templates
               WHERE project_id IN ( { } )
               "#,
                params
            );

            let mut query = sqlx::query(&query_str);
            for id in project_ids {
                query = query.bind(id);
            }

            let result = query.fetch_one(self.db_pool.deref()).await?;

            let count: i64 = result.get("template_count");
            Ok(count as u64)
        }
    }

    async fn get_size_by_projects(&self, project_ids: Vec<Uuid>) -> Result<u64, RepoError> {
        if project_ids.is_empty() {
            Ok(0)
        } else {
            let params = (1..=project_ids.len())
                .map(|i| format!("${}", i))
                .collect::<Vec<_>>()
                .join(", ");
            let query_str = format!(
                r#"
               SELECT sum(size) AS template_size
               FROM templates
               WHERE project_id IN ( { } )
               "#,
                params
            );

            let mut query = sqlx::query(&query_str);
            for id in project_ids {
                query = query.bind(id);
            }

            let result = query.fetch_one(self.db_pool.deref()).await?;

            let size: i64 = result.try_get("template_size").unwrap_or(0);
            Ok(size as u64)
        }
    }

    async fn delete(&self, template_id: &Uuid) -> Result<(), RepoError> {
        sqlx::query("DELETE FROM templates WHERE template_id = $1")
            .bind(template_id)
            .execute(self.db_pool.deref())
            .await?;
        Ok(())
    }
}
