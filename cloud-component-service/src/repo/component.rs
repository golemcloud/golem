use std::ops::Deref;
use std::result::Result;
use std::sync::Arc;

use async_trait::async_trait;
use golem_common::model::ComponentId;
use golem_common::model::ProjectId;
use sqlx::{Database, Pool, Row};
use uuid::Uuid;

use crate::repo::RepoError;
use golem_service_base::model::*;

#[derive(sqlx::FromRow, Debug, Clone)]
pub struct ComponentRecord {
    pub component_id: Uuid,
    pub name: String,
    pub size: i32,
    pub version: i64,
    pub user_component: String,
    pub protected_component: String,
    pub protector_version: Option<i64>,
    pub metadata: String,
    pub project_id: Uuid,
}

impl TryFrom<ComponentRecord> for crate::model::Component {
    type Error = String;

    fn try_from(value: ComponentRecord) -> Result<Self, Self::Error> {
        let metadata: ComponentMetadata = serde_json::from_str(&value.metadata)
            .map_err(|e| format!("Invalid Component Metadata: {}", e))?;
        let versioned_component_id: VersionedComponentId = VersionedComponentId {
            component_id: ComponentId(value.component_id),
            version: value.version as u64,
        };
        let protected_component_id: ProtectedComponentId = ProtectedComponentId {
            versioned_component_id: versioned_component_id.clone(),
        };
        let user_component_id: UserComponentId = UserComponentId {
            versioned_component_id: versioned_component_id.clone(),
        };
        Ok(crate::model::Component {
            component_name: ComponentName(value.name),
            component_size: value.size as u64,
            project_id: ProjectId(value.project_id),
            metadata,
            versioned_component_id,
            user_component_id,
            protected_component_id,
        })
    }
}

impl From<crate::model::Component> for ComponentRecord {
    fn from(value: crate::model::Component) -> Self {
        Self {
            component_id: value.versioned_component_id.component_id.0,
            name: value.component_name.0,
            size: value.component_size as i32,
            version: value.versioned_component_id.version as i64,
            user_component: value.versioned_component_id.slug(),
            protected_component: value.protected_component_id.slug(),
            protector_version: None,
            metadata: serde_json::to_string(&value.metadata).unwrap(),
            project_id: value.project_id.0,
        }
    }
}

#[async_trait]
pub trait ComponentRepo {
    async fn upsert(&self, component: &ComponentRecord) -> Result<(), RepoError>;

    async fn get(&self, component_id: &Uuid) -> Result<Vec<ComponentRecord>, RepoError>;

    async fn get_latest_version(
        &self,
        component_id: &Uuid,
    ) -> Result<Option<ComponentRecord>, RepoError>;

    async fn get_by_version(
        &self,
        component_id: &Uuid,
        version: u64,
    ) -> Result<Option<ComponentRecord>, RepoError>;

    async fn get_by_project(&self, project_id: &Uuid) -> Result<Vec<ComponentRecord>, RepoError>;

    async fn get_by_project_and_name(
        &self,
        project_id: &Uuid,
        name: &str,
    ) -> Result<Vec<ComponentRecord>, RepoError>;

    async fn get_count_by_projects(&self, project_ids: Vec<Uuid>) -> Result<u64, RepoError>;

    async fn get_size_by_projects(&self, project_ids: Vec<Uuid>) -> Result<u64, RepoError>;

    async fn delete(&self, component_id: &Uuid) -> Result<(), RepoError>;
}

pub struct DbComponentRepo<DB: Database> {
    db_pool: Arc<Pool<DB>>,
}

impl<DB: Database> DbComponentRepo<DB> {
    pub fn new(db_pool: Arc<Pool<DB>>) -> Self {
        Self { db_pool }
    }
}

#[async_trait]
impl ComponentRepo for DbComponentRepo<sqlx::Postgres> {
    async fn upsert(&self, component: &ComponentRecord) -> Result<(), RepoError> {
        sqlx::query(
            r#"
              INSERT INTO components
                (component_id, version, project_id, name, size, user_component, protected_component, protector_version, metadata)
              VALUES
                ($1, $2, $3, $4, $5, $6, $7, $8, $9::jsonb)
              ON CONFLICT (component_id, version) DO UPDATE
              SET project_id = $3,
                  name = $4,
                  size = $5,
                  user_component = $6,
                  protected_component = $7,
                  protector_version = $8,
                  metadata = $9::jsonb
            "#,
        )
            .bind(component.component_id)
            .bind(component.version)
            .bind(component.project_id)
            .bind(component.name.clone())
            .bind(component.size)
            .bind(component.user_component.clone())
            .bind(component.protected_component.clone())
            .bind(component.protector_version)
            .bind(component.metadata.clone())
            .execute(self.db_pool.deref())
            .await?;

        Ok(())
    }

    async fn get(&self, component_id: &Uuid) -> Result<Vec<ComponentRecord>, RepoError> {
        sqlx::query_as::<_, ComponentRecord>("SELECT component_id, version, project_id, name, size, user_component, protected_component, protector_version, jsonb_pretty(components.metadata) AS metadata  FROM components WHERE component_id = $1")
            .bind(component_id)
            .fetch_all(self.db_pool.deref())
            .await
            .map_err(|e| e.into())
    }

    async fn get_latest_version(
        &self,
        component_id: &Uuid,
    ) -> Result<Option<ComponentRecord>, RepoError> {
        sqlx::query_as::<_, ComponentRecord>(
            "SELECT component_id, version, project_id, name, size, user_component, protected_component, protector_version, jsonb_pretty(components.metadata) AS metadata FROM components WHERE component_id = $1 ORDER BY version DESC LIMIT 1",
        )
            .bind(component_id)
            .fetch_optional(self.db_pool.deref())
            .await
            .map_err(|e| e.into())
    }

    async fn get_by_version(
        &self,
        component_id: &Uuid,
        version: u64,
    ) -> Result<Option<ComponentRecord>, RepoError> {
        sqlx::query_as::<_, ComponentRecord>(
            "SELECT component_id, version, project_id, name, size, user_component, protected_component, protector_version, jsonb_pretty(components.metadata) AS metadata  FROM components WHERE component_id = $1 AND version = $2",
        )
            .bind(component_id)
            .bind(version as i64)
            .fetch_optional(self.db_pool.deref())
            .await
            .map_err(|e| e.into())
    }

    async fn get_by_project(&self, project_id: &Uuid) -> Result<Vec<ComponentRecord>, RepoError> {
        sqlx::query_as::<_, ComponentRecord>("SELECT component_id, version, project_id, name, size, user_component, protected_component, protector_version, jsonb_pretty(components.metadata) AS metadata  FROM components WHERE project_id = $1")
            .bind(project_id)
            .fetch_all(self.db_pool.deref())
            .await
            .map_err(|e| e.into())
    }

    async fn get_by_project_and_name(
        &self,
        project_id: &Uuid,
        name: &str,
    ) -> Result<Vec<ComponentRecord>, RepoError> {
        sqlx::query_as::<_, ComponentRecord>(
            "SELECT component_id, version, project_id, name, size, user_component, protected_component, protector_version, jsonb_pretty(components.metadata) AS metadata FROM components WHERE project_id = $1 AND name = $2",
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
               SELECT count(distinct component_id) AS component_count
               FROM components
               WHERE project_id IN ( { } )
               "#,
                params
            );

            let mut query = sqlx::query(&query_str);
            for id in project_ids {
                query = query.bind(id);
            }

            let result = query.fetch_one(self.db_pool.deref()).await?;

            let count: i64 = result.get("component_count");
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
               SELECT sum(size) AS component_size
               FROM components
               WHERE project_id IN ( { } )
               "#,
                params
            );

            let mut query = sqlx::query(&query_str);
            for id in project_ids {
                query = query.bind(id);
            }

            let result = query.fetch_one(self.db_pool.deref()).await?;

            let size: i64 = result.try_get("component_size").unwrap_or(0);
            Ok(size as u64)
        }
    }

    async fn delete(&self, component_id: &Uuid) -> Result<(), RepoError> {
        sqlx::query("DELETE FROM components WHERE component_id = $1")
            .bind(component_id)
            .execute(self.db_pool.deref())
            .await?;
        Ok(())
    }
}

#[async_trait]
impl ComponentRepo for DbComponentRepo<sqlx::Sqlite> {
    async fn upsert(&self, component: &ComponentRecord) -> Result<(), RepoError> {
        sqlx::query(
            r#"
              INSERT INTO components
                (component_id, version, project_id, name, size, user_component, protected_component, protector_version, metadata)
              VALUES
                ($1, $2, $3, $4, $5, $6, $7, $8, $9::jsonb)
              ON CONFLICT (component_id, version) DO UPDATE
              SET project_id = $3,
                  name = $4,
                  size = $5,
                  user_component = $6,
                  protected_component = $7,
                  protector_version = $8,
                  metadata = $9::jsonb
            "#,
        )
            .bind(component.component_id)
            .bind(component.version)
            .bind(component.project_id)
            .bind(component.name.clone())
            .bind(component.size)
            .bind(component.user_component.clone())
            .bind(component.protected_component.clone())
            .bind(component.protector_version)
            .bind(component.metadata.clone())
            .execute(self.db_pool.deref())
            .await?;

        Ok(())
    }

    async fn get(&self, component_id: &Uuid) -> Result<Vec<ComponentRecord>, RepoError> {
        sqlx::query_as::<_, ComponentRecord>("SELECT component_id, version, project_id, name, size, user_component, protected_component, protector_version, jsonb_pretty(components.metadata) AS metadata  FROM components WHERE component_id = $1")
            .bind(component_id)
            .fetch_all(self.db_pool.deref())
            .await
            .map_err(|e| e.into())
    }

    async fn get_latest_version(
        &self,
        component_id: &Uuid,
    ) -> Result<Option<ComponentRecord>, RepoError> {
        sqlx::query_as::<_, ComponentRecord>(
            "SELECT component_id, version, project_id, name, size, user_component, protected_component, protector_version, jsonb_pretty(components.metadata) AS metadata FROM components WHERE component_id = $1 ORDER BY version DESC LIMIT 1",
        )
            .bind(component_id)
            .fetch_optional(self.db_pool.deref())
            .await
            .map_err(|e| e.into())
    }

    async fn get_by_version(
        &self,
        component_id: &Uuid,
        version: u64,
    ) -> Result<Option<ComponentRecord>, RepoError> {
        sqlx::query_as::<_, ComponentRecord>(
            "SELECT component_id, version, project_id, name, size, user_component, protected_component, protector_version, jsonb_pretty(components.metadata) AS metadata  FROM components WHERE component_id = $1 AND version = $2",
        )
            .bind(component_id)
            .bind(version as i64)
            .fetch_optional(self.db_pool.deref())
            .await
            .map_err(|e| e.into())
    }

    async fn get_by_project(&self, project_id: &Uuid) -> Result<Vec<ComponentRecord>, RepoError> {
        sqlx::query_as::<_, ComponentRecord>("SELECT component_id, version, project_id, name, size, user_component, protected_component, protector_version, jsonb_pretty(components.metadata) AS metadata  FROM components WHERE project_id = $1")
            .bind(project_id)
            .fetch_all(self.db_pool.deref())
            .await
            .map_err(|e| e.into())
    }

    async fn get_by_project_and_name(
        &self,
        project_id: &Uuid,
        name: &str,
    ) -> Result<Vec<ComponentRecord>, RepoError> {
        sqlx::query_as::<_, ComponentRecord>(
            "SELECT component_id, version, project_id, name, size, user_component, protected_component, protector_version, jsonb_pretty(components.metadata) AS metadata FROM components WHERE project_id = $1 AND name = $2",
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
               SELECT count(distinct component_id) AS component_count
               FROM components
               WHERE project_id IN ( { } )
               "#,
                params
            );

            let mut query = sqlx::query(&query_str);
            for id in project_ids {
                query = query.bind(id);
            }

            let result = query.fetch_one(self.db_pool.deref()).await?;

            let count: i64 = result.get("component_count");
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
               SELECT sum(size) AS component_size
               FROM components
               WHERE project_id IN ( { } )
               "#,
                params
            );

            let mut query = sqlx::query(&query_str);
            for id in project_ids {
                query = query.bind(id);
            }

            let result = query.fetch_one(self.db_pool.deref()).await?;

            let size: i64 = result.try_get("component_size").unwrap_or(0);
            Ok(size as u64)
        }
    }

    async fn delete(&self, component_id: &Uuid) -> Result<(), RepoError> {
        sqlx::query("DELETE FROM components WHERE component_id = $1")
            .bind(component_id)
            .execute(self.db_pool.deref())
            .await?;
        Ok(())
    }
}
