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

use std::fmt::Display;
use std::ops::Deref;
use std::result::Result;
use std::sync::Arc;

use async_trait::async_trait;
use golem_common::model::ComponentId;
use sqlx::{Database, Pool, Row};
use uuid::Uuid;

use crate::repo::RepoError;
use golem_service_base::model::*;

#[derive(sqlx::FromRow, Debug, Clone)]
pub struct ComponentRecord {
    pub namespace: String,
    pub component_id: Uuid,
    pub name: String,
    pub size: i32,
    pub version: i64,
    pub user_component: String,
    pub protected_component: String,
    pub protector_version: Option<i64>,
    pub metadata: Vec<u8>,
}

impl TryFrom<ComponentRecord> for Component {
    type Error = String;
    fn try_from(value: ComponentRecord) -> Result<Self, Self::Error> {
        let metadata: ComponentMetadata = record_metadata_serde::deserialize(&value.metadata)?;
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
        Ok(Component {
            component_name: ComponentName(value.name),
            component_size: value.size as u64,
            metadata,
            versioned_component_id,
            user_component_id,
            protected_component_id,
        })
    }
}

impl From<ComponentRecord> for VersionedComponentId {
    fn from(value: ComponentRecord) -> Self {
        VersionedComponentId {
            component_id: ComponentId(value.component_id),
            version: value.version as u64,
        }
    }
}

impl ComponentRecord {
    pub fn new<Namespace: Display>(
        namespace: Namespace,
        component: Component,
    ) -> Result<Self, String> {
        let metadata = record_metadata_serde::serialize(&component.metadata)?;
        Ok(Self {
            namespace: namespace.to_string(),
            component_id: component.versioned_component_id.component_id.0,
            name: component.component_name.0,
            size: component.component_size as i32,
            version: component.versioned_component_id.version as i64,
            user_component: component.versioned_component_id.slug(),
            protected_component: component.protected_component_id.slug(),
            protector_version: None,
            metadata: metadata.into(),
        })
    }
}

#[async_trait]
pub trait ComponentRepo {
    async fn upsert(&self, component: &ComponentRecord) -> Result<(), RepoError>;

    async fn get(
        &self,
        namespace: &str,
        component_id: &Uuid,
    ) -> Result<Vec<ComponentRecord>, RepoError>;

    async fn get_all(&self, namespace: &str) -> Result<Vec<ComponentRecord>, RepoError>;

    async fn get_latest_version(
        &self,
        namespace: &str,
        component_id: &Uuid,
    ) -> Result<Option<ComponentRecord>, RepoError>;

    async fn get_by_version(
        &self,
        namespace: &str,
        component_id: &Uuid,
        version: u64,
    ) -> Result<Option<ComponentRecord>, RepoError>;

    async fn get_by_name(
        &self,
        namespace: &str,
        name: &str,
    ) -> Result<Vec<ComponentRecord>, RepoError>;

    async fn get_ids_by_name(&self, namespace: &str, name: &str) -> Result<Vec<Uuid>, RepoError>;

    async fn get_namespaces(&self, component_id: &Uuid) -> Result<Vec<String>, RepoError>;

    async fn delete(&self, namespace: &str, component_id: &Uuid) -> Result<(), RepoError>;
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
        let mut transaction = self.db_pool.begin().await?;

        let result = sqlx::query(
            "SELECT count(component_id) as count FROM components WHERE namespace != $1 AND component_id = $2",
        )
        .bind(component.namespace.clone())
        .bind(component.component_id)
        .fetch_one(&mut *transaction)
        .await?;

        let count: i64 = result.get("count");

        if count > 0 {
            return Err(RepoError::Internal("Component id not unique".to_string()));
        }

        sqlx::query(
            r#"
              INSERT INTO components
                (namespace, component_id, name)
              VALUES
                ($1, $2, $3)
              ON CONFLICT (namespace, name) DO NOTHING
               "#,
        )
        .bind(component.namespace.clone())
        .bind(component.component_id)
        .bind(component.name.clone())
        .execute(&mut *transaction)
        .await?;

        sqlx::query(
            r#"
              INSERT INTO component_versions
                (component_id, version, size, user_component, protected_component, protector_version, metadata)
              VALUES
                ($1, $2, $3, $4, $5, $6, $7)
              ON CONFLICT (component_id, version) DO UPDATE
              SET size = $3,
                  user_component = $4,
                  protected_component = $5,
                  protector_version = $6,
                  metadata = $7
               "#,
        )
            .bind(component.component_id)
            .bind(component.version)
            .bind(component.size)
            .bind(component.user_component.clone())
            .bind(component.protected_component.clone())
            .bind(component.protector_version)
            .bind(component.metadata.clone())
            .execute(&mut *transaction)
            .await?;

        transaction.commit().await?;

        Ok(())
    }

    async fn get(
        &self,
        namespace: &str,
        component_id: &Uuid,
    ) -> Result<Vec<ComponentRecord>, RepoError> {
        sqlx::query_as::<_, ComponentRecord>(
            "SELECT components.namespace, components.name, component_versions.* FROM components JOIN component_versions ON components.component_id = component_versions.component_id WHERE components.namespace = $1 AND component_versions.component_id = $2"
        )
            .bind(namespace)
            .bind(component_id)
            .fetch_all(self.db_pool.deref())
            .await
            .map_err(|e| e.into())
    }

    async fn get_all(&self, namespace: &str) -> Result<Vec<ComponentRecord>, RepoError> {
        sqlx::query_as::<_, ComponentRecord>("SELECT components.namespace, components.name, component_versions.* FROM components JOIN component_versions ON components.component_id = component_versions.component_id WHERE components.namespace = $1")
            .bind(namespace)
            .fetch_all(self.db_pool.deref())
            .await
            .map_err(|e| e.into())
    }

    async fn get_latest_version(
        &self,
        namespace: &str,
        component_id: &Uuid,
    ) -> Result<Option<ComponentRecord>, RepoError> {
        sqlx::query_as::<_, ComponentRecord>(
            "SELECT components.namespace, components.name, component_versions.* FROM components JOIN component_versions ON components.component_id = component_versions.component_id WHERE components.namespace = $1 AND component_versions.component_id = $2  ORDER BY version DESC LIMIT 1",
        ).bind(namespace)
            .bind(component_id)
            .fetch_optional(self.db_pool.deref())
            .await
            .map_err(|e| e.into())
    }

    async fn get_by_version(
        &self,
        namespace: &str,
        component_id: &Uuid,
        version: u64,
    ) -> Result<Option<ComponentRecord>, RepoError> {
        sqlx::query_as::<_, ComponentRecord>(
            "SELECT components.namespace, components.name, component_versions.* FROM components JOIN component_versions ON components.component_id = component_versions.component_id WHERE components.namespace = $1 AND component_versions.component_id = $2 AND component_versions.version = $3",
        )
            .bind(namespace)
            .bind(component_id)
            .bind(version as i64)
            .fetch_optional(self.db_pool.deref())
            .await
            .map_err(|e| e.into())
    }

    async fn get_by_name(
        &self,
        namespace: &str,
        name: &str,
    ) -> Result<Vec<ComponentRecord>, RepoError> {
        sqlx::query_as::<_, ComponentRecord>(
            "SELECT components.namespace, components.name, component_versions.* FROM components JOIN component_versions ON components.component_id = component_versions.component_id WHERE components.namespace = $1 AND components.name = $2",
        )
            .bind(namespace)
            .bind(name)
            .fetch_all(self.db_pool.deref())
            .await
            .map_err(|e| e.into())
    }

    async fn get_ids_by_name(&self, namespace: &str, name: &str) -> Result<Vec<Uuid>, RepoError> {
        let result = sqlx::query(
            "SELECT distinct component_id as component_id FROM components WHERE namespace = $1 AND name = $2"
        )
            .bind(namespace)
            .bind(name)
            .fetch_all(self.db_pool.deref())
            .await?;

        Ok(result.into_iter().map(|x| x.get("component_id")).collect())
    }

    async fn get_namespaces(&self, component_id: &Uuid) -> Result<Vec<String>, RepoError> {
        let result = sqlx::query("SELECT namespace FROM components WHERE component_id = $1")
            .bind(component_id)
            .fetch_all(self.db_pool.deref())
            .await?;

        Ok(result.into_iter().map(|x| x.get("namespace")).collect())
    }

    async fn delete(&self, namespace: &str, component_id: &Uuid) -> Result<(), RepoError> {
        let mut transaction = self.db_pool.begin().await?;
        sqlx::query("DELETE FROM component_versions WHERE component_id IN (SELECT component_id FROM components WHERE namespace = $1 AND component_id = $2)")
            .bind(namespace)
            .bind(component_id)
            .execute(&mut *transaction)
            .await?;

        sqlx::query("DELETE FROM components WHERE namespace = $1 AND component_id = $2")
            .bind(namespace)
            .bind(component_id)
            .execute(&mut *transaction)
            .await?;

        transaction.commit().await?;
        Ok(())
    }
}

#[async_trait]
impl ComponentRepo for DbComponentRepo<sqlx::Postgres> {
    async fn upsert(&self, component: &ComponentRecord) -> Result<(), RepoError> {
        let mut transaction = self.db_pool.begin().await?;

        let result = sqlx::query(
            "SELECT count(component_id) as count FROM components WHERE namespace != $1 AND component_id = $2",
        )
        .bind(component.namespace.clone())
        .bind(component.component_id)
        .fetch_one(&mut *transaction)
        .await?;

        let count: i64 = result.get("count");

        if count > 0 {
            return Err(RepoError::Internal("Component id not unique".to_string()));
        }

        sqlx::query(
            r#"
              INSERT INTO components
                (namespace, component_id, name)
              VALUES
                ($1, $2, $3)
              ON CONFLICT (namespace, name) DO NOTHING
               "#,
        )
        .bind(component.namespace.clone())
        .bind(component.component_id)
        .bind(component.name.clone())
        .execute(&mut *transaction)
        .await?;

        sqlx::query(
            r#"
              INSERT INTO component_versions
                (component_id, version, size, user_component, protected_component, protector_version, metadata)
              VALUES
                ($1, $2, $3, $4, $5, $6, $7)
              ON CONFLICT (component_id, version) DO UPDATE
              SET size = $3,
                  user_component = $4,
                  protected_component = $5,
                  protector_version = $6,
                  metadata = $7
               "#,
        )
            .bind(component.component_id)
            .bind(component.version)
            .bind(component.size)
            .bind(component.user_component.clone())
            .bind(component.protected_component.clone())
            .bind(component.protector_version)
            .bind(component.metadata.clone())
            .execute(&mut *transaction)
            .await?;

        transaction.commit().await?;

        Ok(())
    }

    async fn get(
        &self,
        namespace: &str,
        component_id: &Uuid,
    ) -> Result<Vec<ComponentRecord>, RepoError> {
        sqlx::query_as::<_, ComponentRecord>(
            "SELECT components.namespace, components.name, component_versions.* FROM components JOIN component_versions ON components.component_id = component_versions.component_id WHERE components.namespace = $1 AND component_versions.component_id = $2"
        )
            .bind(namespace)
            .bind(component_id)
            .fetch_all(self.db_pool.deref())
            .await
            .map_err(|e| e.into())
    }

    async fn get_all(&self, namespace: &str) -> Result<Vec<ComponentRecord>, RepoError> {
        sqlx::query_as::<_, ComponentRecord>("SELECT components.namespace, components.name, component_versions.* FROM components JOIN component_versions ON components.component_id = component_versions.component_id WHERE components.namespace = $1")
            .bind(namespace)
            .fetch_all(self.db_pool.deref())
            .await
            .map_err(|e| e.into())
    }

    async fn get_latest_version(
        &self,
        namespace: &str,
        component_id: &Uuid,
    ) -> Result<Option<ComponentRecord>, RepoError> {
        sqlx::query_as::<_, ComponentRecord>(
            "SELECT components.namespace, components.name, component_versions.* FROM components JOIN component_versions ON components.component_id = component_versions.component_id WHERE components.namespace = $1 AND component_versions.component_id = $2  ORDER BY version DESC LIMIT 1",
        ).bind(namespace)
            .bind(component_id)
            .fetch_optional(self.db_pool.deref())
            .await
            .map_err(|e| e.into())
    }

    async fn get_by_version(
        &self,
        namespace: &str,
        component_id: &Uuid,
        version: u64,
    ) -> Result<Option<ComponentRecord>, RepoError> {
        sqlx::query_as::<_, ComponentRecord>(
            "SELECT components.namespace, components.name, component_versions.* FROM components JOIN component_versions ON components.component_id = component_versions.component_id WHERE components.namespace = $1 AND component_versions.component_id = $2 AND component_versions.version = $3",
        )
            .bind(namespace)
            .bind(component_id)
            .bind(version as i64)
            .fetch_optional(self.db_pool.deref())
            .await
            .map_err(|e| e.into())
    }

    async fn get_by_name(
        &self,
        namespace: &str,
        name: &str,
    ) -> Result<Vec<ComponentRecord>, RepoError> {
        sqlx::query_as::<_, ComponentRecord>(
            "SELECT components.namespace, components.name, component_versions.* FROM components JOIN component_versions ON components.component_id = component_versions.component_id WHERE components.namespace = $1 AND components.name = $2",
        )
            .bind(namespace)
            .bind(name)
            .fetch_all(self.db_pool.deref())
            .await
            .map_err(|e| e.into())
    }

    async fn get_ids_by_name(&self, namespace: &str, name: &str) -> Result<Vec<Uuid>, RepoError> {
        let result = sqlx::query(
            "SELECT distinct component_id as component_id FROM components WHERE namespace = $1 AND name = $2"
        )
            .bind(namespace)
            .bind(name)
            .fetch_all(self.db_pool.deref())
            .await?;

        Ok(result.into_iter().map(|x| x.get("component_id")).collect())
    }

    async fn get_namespaces(&self, component_id: &Uuid) -> Result<Vec<String>, RepoError> {
        let result = sqlx::query("SELECT namespace FROM components WHERE component_id = $1")
            .bind(component_id)
            .fetch_all(self.db_pool.deref())
            .await?;

        Ok(result.into_iter().map(|x| x.get("namespace")).collect())
    }

    async fn delete(&self, namespace: &str, component_id: &Uuid) -> Result<(), RepoError> {
        let mut transaction = self.db_pool.begin().await?;
        sqlx::query("DELETE FROM component_versions WHERE component_id IN (SELECT component_id FROM components WHERE namespace = $1 AND component_id = $2)")
            .bind(namespace)
            .bind(component_id)
            .execute(&mut *transaction)
            .await?;

        sqlx::query("DELETE FROM components WHERE namespace = $1 AND component_id = $2")
            .bind(namespace)
            .bind(component_id)
            .execute(&mut *transaction)
            .await?;

        transaction.commit().await?;
        Ok(())
    }
}

pub mod record_metadata_serde {
    use bincode::{Decode, Encode};
    use bytes::Bytes;
    use golem_common::serialization::serialize_with_version;
    pub const SERIALIZATION_VERSION_V1: u8 = 1u8;

    pub fn serialize<T: Encode>(routes: &T) -> Result<Bytes, String> {
        serialize_with_version(routes, SERIALIZATION_VERSION_V1)
    }

    pub fn deserialize<T: Decode>(bytes: &[u8]) -> Result<T, String> {
        let (version, data) = bytes.split_at(1);

        match version[0] {
            SERIALIZATION_VERSION_V1 => {
                let (routes, _) = bincode::decode_from_slice(data, bincode::config::standard())
                    .map_err(|e| format!("Failed to deserialize value: {e}"))?;

                Ok(routes)
            }
            _ => Err("Unsupported serialization version".to_string()),
        }
    }
}
