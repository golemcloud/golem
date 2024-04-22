// Copyright 2024 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use std::ops::Deref;
use std::result::Result;
use std::sync::Arc;

use async_trait::async_trait;
use golem_common::model::ComponentId;
use sqlx::{Database, Pool};
use uuid::Uuid;

use crate::repo::RepoError;
use golem_service_base::model::*;

#[derive(sqlx::FromRow, Debug, Clone)]
pub struct ComponentRecord {
    pub component_id: Uuid,
    pub name: String,
    pub size: i64,
    pub version: i64,
    pub user_component: String,
    pub protected_component: String,
    pub protector_version: Option<i64>,
    pub metadata: String,
}

impl From<ComponentRecord> for Component {
    fn from(value: ComponentRecord) -> Self {
        let metadata: ComponentMetadata = serde_json::from_str(&value.metadata).unwrap();
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
        Component {
            component_name: ComponentName(value.name),
            component_size: value.size as u64,
            metadata,
            versioned_component_id,
            user_component_id,
            protected_component_id,
        }
    }
}

impl From<Component> for ComponentRecord {
    fn from(value: Component) -> Self {
        Self {
            component_id: value.versioned_component_id.component_id.0,
            name: value.component_name.0,
            size: value.component_size as i64,
            version: value.versioned_component_id.version as i64,
            user_component: value.versioned_component_id.slug(),
            protected_component: value.protected_component_id.slug(),
            protector_version: None,
            metadata: serde_json::to_string(&value.metadata).unwrap(),
        }
    }
}

#[async_trait]
pub trait ComponentRepo {
    async fn upsert(&self, component: &ComponentRecord) -> Result<(), RepoError>;

    async fn get(&self, component_id: &Uuid) -> Result<Vec<ComponentRecord>, RepoError>;

    async fn get_all(&self) -> Result<Vec<ComponentRecord>, RepoError>;

    async fn get_latest_version(
        &self,
        component_id: &Uuid,
    ) -> Result<Option<ComponentRecord>, RepoError>;

    async fn get_by_version(
        &self,
        component_id: &Uuid,
        version: u64,
    ) -> Result<Option<ComponentRecord>, RepoError>;

    async fn get_by_name(&self, name: &str) -> Result<Vec<ComponentRecord>, RepoError>;

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
impl ComponentRepo for DbComponentRepo<sqlx::Sqlite> {
    async fn upsert(&self, component: &ComponentRecord) -> Result<(), RepoError> {
        sqlx::query(
            r#"
              INSERT INTO components
                (component_id, version, name, size, user_component, protected_component, protector_version, metadata)
              VALUES
                ($1, $2, $3, $4, $5, $6, $7, $8::jsonb)
              ON CONFLICT (component_id, version) DO UPDATE
              SET name = $3,
                  size = $4,
                  user_component = $5,
                  protected_component = $6,
                  protector_version = $7,
                  metadata = $8::jsonb
               "#,
        )
            .bind(component.component_id)
            .bind(component.version)
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
        sqlx::query_as::<_, ComponentRecord>("SELECT component_id, version, name, size, user_component, protected_component, protector_version,  CAST(metadata AS TEXT) AS metadata  FROM components WHERE component_id = $1")
            .bind(component_id)
            .fetch_all(self.db_pool.deref())
            .await
            .map_err(|e| e.into())
    }

    async fn get_all(&self) -> Result<Vec<ComponentRecord>, RepoError> {
        sqlx::query_as::<_, ComponentRecord>("SELECT component_id, version, name, size, user_component, protected_component, protector_version,  CAST(metadata AS TEXT) AS metadata  FROM components")
            .fetch_all(self.db_pool.deref())
            .await
            .map_err(|e| e.into())
    }

    async fn get_latest_version(
        &self,
        component_id: &Uuid,
    ) -> Result<Option<ComponentRecord>, RepoError> {
        sqlx::query_as::<_, ComponentRecord>(
            "SELECT component_id, version, name, size, user_component, protected_component, protector_version,  CAST(metadata AS TEXT) AS metadata FROM components WHERE component_id = $1 ORDER BY version DESC LIMIT 1",
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
            "SELECT component_id, version, name, size, user_component, protected_component, protector_version,  CAST(metadata AS TEXT) AS metadata  FROM components WHERE component_id = $1 AND version = $2",
        )
            .bind(component_id)
            .bind(version as i64)
            .fetch_optional(self.db_pool.deref())
            .await
            .map_err(|e| e.into())
    }

    async fn get_by_name(&self, name: &str) -> Result<Vec<ComponentRecord>, RepoError> {
        sqlx::query_as::<_, ComponentRecord>(
            "SELECT component_id, version, name, size, user_component, protected_component, protector_version,  CAST(metadata AS TEXT) AS metadata FROM components WHERE name = $1",
        )
            .bind(name)
            .fetch_all(self.db_pool.deref())
            .await
            .map_err(|e| e.into())
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
impl ComponentRepo for DbComponentRepo<sqlx::Postgres> {
    async fn upsert(&self, component: &ComponentRecord) -> Result<(), RepoError> {
        sqlx::query(
            r#"
              INSERT INTO components
                (component_id, version, name, size, user_component, protected_component, protector_version, metadata)
              VALUES
                ($1, $2, $3, $4, $5, $6, $7, $8::jsonb)
              ON CONFLICT (component_id, version) DO UPDATE
              SET name = $3,
                  size = $4,
                  user_component = $5,
                  protected_component = $6,
                  protector_version = $7,
                  metadata = $8::jsonb
            "#,
        )
            .bind(component.component_id)
            .bind(component.version)
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

    async fn get_all(&self) -> Result<Vec<ComponentRecord>, RepoError> {
        sqlx::query_as::<_, ComponentRecord>("SELECT component_id, name, size, version, user_component, protected_component, protector_version, jsonb_pretty(components.metadata) AS metadata  FROM components")
            .fetch_all(self.db_pool.deref())
            .await
            .map_err(|e| e.into())
    }

    async fn get(&self, component_id: &Uuid) -> Result<Vec<ComponentRecord>, RepoError> {
        sqlx::query_as::<_, ComponentRecord>("SELECT component_id, name, size, version, user_component, protected_component, protector_version, jsonb_pretty(components.metadata) AS metadata  FROM components WHERE component_id = $1")
            .bind(component_id)
            .fetch_all(self.db_pool.deref())
            .await
            .map_err(|e| e.into())
    }

    async fn get_by_name(&self, name: &str) -> Result<Vec<ComponentRecord>, RepoError> {
        sqlx::query_as::<_, ComponentRecord>(
            "SELECT component_id, name, size, version, user_component, protected_component, protector_version, jsonb_pretty(components.metadata) AS metadata FROM components WHERE name = $1",
        )
            .bind(name)
            .fetch_all(self.db_pool.deref())
            .await
            .map_err(|e| e.into())
    }

    async fn get_latest_version(
        &self,
        component_id: &Uuid,
    ) -> Result<Option<ComponentRecord>, RepoError> {
        sqlx::query_as::<_, ComponentRecord>(
            "SELECT component_id, name, size, version, user_component, protected_component, protector_version, jsonb_pretty(components.metadata) AS metadata FROM components WHERE component_id = $1 ORDER BY version DESC LIMIT 1",
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
            "SELECT component_id, name, size, version, user_component, protected_component, protector_version, jsonb_pretty(components.metadata) AS metadata  FROM components WHERE component_id = $1 AND version = $2",
        )
            .bind(component_id)
            .bind(version as i64)
            .fetch_optional(self.db_pool.deref())
            .await
            .map_err(|e| e.into())
    }

    async fn delete(&self, component_id: &Uuid) -> Result<(), RepoError> {
        sqlx::query("DELETE FROM components WHERE component_id = $1")
            .bind(component_id)
            .execute(self.db_pool.deref())
            .await?;
        Ok(())
    }
}
