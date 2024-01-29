use std::ops::Deref;
use std::result::Result;
use std::sync::Arc;

use async_trait::async_trait;
use golem_common::model::TemplateId;
use sqlx::{Database, Pool};
use uuid::Uuid;

use crate::repo::RepoError;
use golem_cloud_server_base::model::*;

#[derive(sqlx::FromRow, Debug, Clone)]
pub struct TemplateRecord {
    pub template_id: Uuid,
    pub name: String,
    pub size: i32,
    pub version: i32,
    pub user_template: String,
    pub protected_template: String,
    pub protector_version: Option<i32>,
    pub metadata: String,
}

impl From<TemplateRecord> for Template {
    fn from(value: TemplateRecord) -> Self {
        let metadata: TemplateMetadata = serde_json::from_str(&value.metadata).unwrap();
        let versioned_template_id: VersionedTemplateId = VersionedTemplateId {
            template_id: TemplateId(value.template_id),
            version: value.version,
        };
        let protected_template_id: ProtectedTemplateId = ProtectedTemplateId {
            versioned_template_id: versioned_template_id.clone(),
        };
        let user_template_id: UserTemplateId = UserTemplateId {
            versioned_template_id: versioned_template_id.clone(),
        };
        Template {
            template_name: TemplateName(value.name),
            template_size: value.size,
            metadata,
            versioned_template_id,
            user_template_id,
            protected_template_id,
        }
    }
}

impl From<Template> for TemplateRecord {
    fn from(value: Template) -> Self {
        Self {
            template_id: value.versioned_template_id.template_id.0,
            name: value.template_name.0,
            size: value.template_size,
            version: value.versioned_template_id.version,
            user_template: value.versioned_template_id.slug(),
            protected_template: value.protected_template_id.slug(),
            protector_version: None,
            metadata: serde_json::to_string(&value.metadata).unwrap(),
        }
    }
}

#[async_trait]
pub trait TemplateRepo {
    async fn upsert(&self, template: &TemplateRecord) -> Result<(), RepoError>;

    async fn get(&self, template_id: &Uuid) -> Result<Vec<TemplateRecord>, RepoError>;

    async fn get_all(&self) -> Result<Vec<TemplateRecord>, RepoError>;

    async fn get_latest_version(
        &self,
        template_id: &Uuid,
    ) -> Result<Option<TemplateRecord>, RepoError>;

    async fn get_by_version(
        &self,
        template_id: &Uuid,
        version: i32,
    ) -> Result<Option<TemplateRecord>, RepoError>;

    async fn get_by_name(&self, name: &str) -> Result<Vec<TemplateRecord>, RepoError>;

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
impl TemplateRepo for DbTemplateRepo<sqlx::Sqlite> {
    async fn upsert(&self, template: &TemplateRecord) -> Result<(), RepoError> {
        sqlx::query(
            r#"
              INSERT INTO templates
                (template_id, version, name, size, user_template, protected_template, protector_version, metadata)
              VALUES
                ($1, $2, $3, $4, $5, $6, $7, $8::jsonb)
              ON CONFLICT (template_id, version) DO UPDATE
              SET name = $3,
                  size = $4,
                  user_template = $5,
                  protected_template = $6,
                  protector_version = $7,
                  metadata = $8::jsonb
            "#,
        )
            .bind(template.template_id)
            .bind(template.version)
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
        sqlx::query_as::<_, TemplateRecord>("SELECT template_id, version, name, size, user_template, protected_template, protector_version,  CAST(metadata AS TEXT) AS metadata  FROM templates WHERE template_id = $1")
            .bind(template_id)
            .fetch_all(self.db_pool.deref())
            .await
            .map_err(|e| e.into())
    }

    async fn get_all(&self) -> Result<Vec<TemplateRecord>, RepoError> {
        sqlx::query_as::<_, TemplateRecord>("SELECT template_id, version, name, size, user_template, protected_template, protector_version,  CAST(metadata AS TEXT) AS metadata  FROM templates")
            .fetch_all(self.db_pool.deref())
            .await
            .map_err(|e| e.into())
    }

    async fn get_latest_version(
        &self,
        template_id: &Uuid,
    ) -> Result<Option<TemplateRecord>, RepoError> {
        sqlx::query_as::<_, TemplateRecord>(
            "SELECT template_id, version, name, size, user_template, protected_template, protector_version,  CAST(metadata AS TEXT) AS metadata FROM templates WHERE template_id = $1 ORDER BY version DESC LIMIT 1",
        )
            .bind(template_id)
            .fetch_optional(self.db_pool.deref())
            .await
            .map_err(|e| e.into())
    }

    async fn get_by_version(
        &self,
        template_id: &Uuid,
        version: i32,
    ) -> Result<Option<TemplateRecord>, RepoError> {
        sqlx::query_as::<_, TemplateRecord>(
            "SELECT template_id, version, name, size, user_template, protected_template, protector_version,  CAST(metadata AS TEXT) AS metadata  FROM templates WHERE template_id = $1 AND version = $2",
        )
            .bind(template_id)
            .bind(version)
            .fetch_optional(self.db_pool.deref())
            .await
            .map_err(|e| e.into())
    }

    async fn get_by_name(&self, name: &str) -> Result<Vec<TemplateRecord>, RepoError> {
        sqlx::query_as::<_, TemplateRecord>(
            "SELECT template_id, version, name, size, user_template, protected_template, protector_version,  CAST(metadata AS TEXT) AS metadata FROM templates WHERE name = $1",
        )
            .bind(name)
            .fetch_all(self.db_pool.deref())
            .await
            .map_err(|e| e.into())
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
impl TemplateRepo for DbTemplateRepo<sqlx::Postgres> {
    async fn upsert(&self, template: &TemplateRecord) -> Result<(), RepoError> {
        sqlx::query(
            r#"
              INSERT INTO templates
                (template_id, version, name, size, user_template, protected_template, protector_version, metadata)
              VALUES
                ($1, $2, $3, $4, $5, $6, $7, $8:jsonb)
              ON CONFLICT (template_id, version) DO UPDATE
              SET name = $3,
                  size = $4,
                  user_template = $5,
                  protected_template = $6,
                  protector_version = $7,
                  metadata = $8::jsonb
            "#,
        )
            .bind(template.template_id)
            .bind(template.version)
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


    async fn get_all(&self) -> Result<Vec<TemplateRecord>, RepoError> {
        sqlx::query_as::<_, TemplateRecord>("SELECT template_id, version, project_id, name, size, user_template, protected_template, protector_version, jsonb_pretty(templates.metadata) AS metadata  FROM templates")
            .fetch_all(self.db_pool.deref())
            .await
            .map_err(|e| e.into())
    }

    async fn get(&self, template_id: &Uuid) -> Result<Vec<TemplateRecord>, RepoError> {
        sqlx::query_as::<_, TemplateRecord>("SELECT template_id, version, project_id, name, size, user_template, protected_template, protector_version, jsonb_pretty(templates.metadata) AS metadata  FROM templates WHERE template_id = $1")
            .bind(template_id)
            .fetch_all(self.db_pool.deref())
            .await
            .map_err(|e| e.into())
    }

    async fn get_by_name(
        &self,
        name: &str,
    ) -> Result<Vec<TemplateRecord>, RepoError> {
        sqlx::query_as::<_, TemplateRecord>(
            "SELECT template_id, version, project_id, name, size, user_template, protected_template, protector_version, jsonb_pretty(templates.metadata) AS metadata FROM templates WHERE name = $1",
        )
            .bind(name)
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
        version: i32,
    ) -> Result<Option<TemplateRecord>, RepoError> {
        sqlx::query_as::<_, TemplateRecord>(
            "SELECT template_id, version, project_id, name, size, user_template, protected_template, protector_version, jsonb_pretty(templates.metadata) AS metadata  FROM templates WHERE template_id = $1 AND version = $2",
        )
            .bind(template_id)
            .bind(version)
            .fetch_optional(self.db_pool.deref())
            .await
            .map_err(|e| e.into())
    }

    async fn delete(&self, template_id: &Uuid) -> Result<(), RepoError> {
        sqlx::query("DELETE FROM templates WHERE template_id = $1")
            .bind(template_id)
            .execute(self.db_pool.deref())
            .await?;
        Ok(())
    }
}