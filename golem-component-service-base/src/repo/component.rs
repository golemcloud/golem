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
use golem_service_base::model::{
    ComponentMetadata, ComponentName, ProtectedComponentId, UserComponentId, VersionedComponentId,
};
use sqlx::{Database, Pool, Row};
use uuid::Uuid;

use crate::model::Component;
use crate::repo::RepoError;

#[derive(sqlx::FromRow, Debug, Clone)]
pub struct ComponentRecord {
    pub namespace: String,
    pub component_id: Uuid,
    pub name: String,
    pub size: i32,
    pub version: i64,
    pub metadata: Vec<u8>,
}

impl<Namespace> TryFrom<ComponentRecord> for Component<Namespace>
where
    Namespace: Display + TryFrom<String> + Eq + Clone + Send + Sync,
    <Namespace as TryFrom<String>>::Error: Display + Send + Sync + 'static,
{
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
        let namespace = Namespace::try_from(value.namespace).map_err(|e| e.to_string())?;
        Ok(Component {
            namespace,
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

impl<Namespace> TryFrom<Component<Namespace>> for ComponentRecord
where
    Namespace: Display,
{
    type Error = String;

    fn try_from(value: Component<Namespace>) -> Result<Self, Self::Error> {
        let metadata = record_metadata_serde::serialize(&value.metadata)?;
        Ok(Self {
            namespace: value.namespace.to_string(),
            component_id: value.versioned_component_id.component_id.0,
            name: value.component_name.0,
            size: value.component_size as i32,
            version: value.versioned_component_id.version as i64,
            metadata: metadata.into(),
        })
    }
}

#[async_trait]
pub trait ComponentRepo {
    async fn create(&self, component: &ComponentRecord) -> Result<(), RepoError>;

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

    async fn get_id_by_name(&self, namespace: &str, name: &str) -> Result<Option<Uuid>, RepoError>;

    async fn get_namespace(&self, component_id: &Uuid) -> Result<Option<String>, RepoError>;

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
    async fn create(&self, component: &ComponentRecord) -> Result<(), RepoError> {
        let mut transaction = self.db_pool.begin().await?;

        let result = sqlx::query("SELECT namespace FROM components WHERE component_id = $1")
            .bind(component.component_id)
            .fetch_optional(&mut *transaction)
            .await?;

        if let Some(result) = result {
            let namespace: String = result.get("namespace");
            if namespace != component.namespace {
                return Err(RepoError::Internal(
                    "Component namespace invalid".to_string(),
                ));
            }
        } else {
            sqlx::query(
                r#"
                  INSERT INTO components
                    (namespace, component_id, name)
                  VALUES
                    ($1, $2, $3)
                   "#,
            )
            .bind(component.namespace.clone())
            .bind(component.component_id)
            .bind(component.name.clone())
            .execute(&mut *transaction)
            .await?;
        }

        sqlx::query(
            r#"
              INSERT INTO component_versions
                (component_id, version, size, metadata)
              VALUES
                ($1, $2, $3, $4)
               "#,
        )
        .bind(component.component_id)
        .bind(component.version)
        .bind(component.size)
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
            r#"
                SELECT
                    c.namespace AS namespace,
                    c.name AS name,
                    c.component_id AS component_id,
                    cv.version AS version,
                    cv.size AS size,
                    cv.metadata AS metadata
                FROM components c
                    JOIN component_versions cv ON c.component_id = cv.component_id
                WHERE c.namespace = $1 AND c.component_id = $2
                "#,
        )
        .bind(namespace)
        .bind(component_id)
        .fetch_all(self.db_pool.deref())
        .await
        .map_err(|e| e.into())
    }

    async fn get_all(&self, namespace: &str) -> Result<Vec<ComponentRecord>, RepoError> {
        sqlx::query_as::<_, ComponentRecord>(
            r#"
                SELECT
                    c.namespace AS namespace,
                    c.name AS name,
                    c.component_id AS component_id,
                    cv.version AS version,
                    cv.size AS size,
                    cv.metadata AS metadata
                FROM components c
                    JOIN component_versions cv ON c.component_id = cv.component_id
                WHERE c.namespace = $1
                "#,
        )
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
            r#"
                SELECT
                    c.namespace AS namespace,
                    c.name AS name,
                    c.component_id AS component_id,
                    cv.version AS version,
                    cv.size AS size,
                    cv.metadata AS metadata
                FROM components c
                    JOIN component_versions cv ON c.component_id = cv.component_id
                WHERE c.namespace = $1 AND c.component_id = $2
                ORDER BY cv.version DESC LIMIT 1
                "#,
        )
        .bind(namespace)
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
            r#"
                SELECT
                    c.namespace AS namespace,
                    c.name AS name,
                    c.component_id AS component_id,
                    cv.version AS version,
                    cv.size AS size,
                    cv.metadata AS metadata
                FROM components c
                    JOIN component_versions cv ON c.component_id = cv.component_id
                WHERE c.namespace = $1 AND c.component_id = $2 AND cv.version = $3
                "#,
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
            r#"
                SELECT
                    c.namespace AS namespace,
                    c.name AS name,
                    c.component_id AS component_id,
                    cv.version AS version,
                    cv.size AS size,
                    cv.metadata AS metadata
                FROM components c
                    JOIN component_versions cv ON c.component_id = cv.component_id
                WHERE c.namespace = $1 AND c.name = $2
                "#,
        )
        .bind(namespace)
        .bind(name)
        .fetch_all(self.db_pool.deref())
        .await
        .map_err(|e| e.into())
    }

    async fn get_id_by_name(&self, namespace: &str, name: &str) -> Result<Option<Uuid>, RepoError> {
        let result =
            sqlx::query("SELECT component_id FROM components WHERE namespace = $1 AND name = $2")
                .bind(namespace)
                .bind(name)
                .fetch_optional(self.db_pool.deref())
                .await?;

        Ok(result.map(|x| x.get("component_id")))
    }

    async fn get_namespace(&self, component_id: &Uuid) -> Result<Option<String>, RepoError> {
        let result = sqlx::query("SELECT namespace FROM components WHERE component_id = $1")
            .bind(component_id)
            .fetch_optional(self.db_pool.deref())
            .await?;

        Ok(result.map(|x| x.get("namespace")))
    }

    async fn delete(&self, namespace: &str, component_id: &Uuid) -> Result<(), RepoError> {
        let mut transaction = self.db_pool.begin().await?;
        sqlx::query(
            r#"
                DELETE FROM component_versions
                WHERE component_id IN (SELECT component_id FROM components WHERE namespace = $1 AND component_id = $2)
            "#
        )
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
    async fn create(&self, component: &ComponentRecord) -> Result<(), RepoError> {
        let mut transaction = self.db_pool.begin().await?;

        let result = sqlx::query("SELECT namespace FROM components WHERE component_id = $1")
            .bind(component.component_id)
            .fetch_optional(&mut *transaction)
            .await?;

        if let Some(result) = result {
            let namespace: String = result.get("namespace");
            if namespace != component.namespace {
                return Err(RepoError::Internal(
                    "Component namespace invalid".to_string(),
                ));
            }
        } else {
            sqlx::query(
                r#"
                  INSERT INTO components
                    (namespace, component_id, name)
                  VALUES
                    ($1, $2, $3)
                   "#,
            )
            .bind(component.namespace.clone())
            .bind(component.component_id)
            .bind(component.name.clone())
            .execute(&mut *transaction)
            .await?;
        }

        sqlx::query(
            r#"
              INSERT INTO component_versions
                (component_id, version, size, metadata)
              VALUES
                ($1, $2, $3, $4)
               "#,
        )
        .bind(component.component_id)
        .bind(component.version)
        .bind(component.size)
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
            r#"
                SELECT
                    c.namespace AS namespace,
                    c.name AS name,
                    c.component_id AS component_id,
                    cv.version AS version,
                    cv.size AS size,
                    cv.metadata AS metadata
                FROM components c
                    JOIN component_versions cv ON c.component_id = cv.component_id
                WHERE c.namespace = $1 AND c.component_id = $2
                "#,
        )
        .bind(namespace)
        .bind(component_id)
        .fetch_all(self.db_pool.deref())
        .await
        .map_err(|e| e.into())
    }

    async fn get_all(&self, namespace: &str) -> Result<Vec<ComponentRecord>, RepoError> {
        sqlx::query_as::<_, ComponentRecord>(
            r#"
                SELECT
                    c.namespace AS namespace,
                    c.name AS name,
                    c.component_id AS component_id,
                    cv.version AS version,
                    cv.size AS size,
                    cv.metadata AS metadata
                FROM components c
                    JOIN component_versions cv ON c.component_id = cv.component_id
                WHERE c.namespace = $1
                "#,
        )
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
            r#"
                SELECT
                    c.namespace AS namespace,
                    c.name AS name,
                    c.component_id AS component_id,
                    cv.version AS version,
                    cv.size AS size,
                    cv.metadata AS metadata
                FROM components c
                    JOIN component_versions cv ON c.component_id = cv.component_id
                WHERE c.namespace = $1 AND c.component_id = $2
                ORDER BY cv.version DESC LIMIT 1
                "#,
        )
        .bind(namespace)
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
            r#"
                SELECT
                    c.namespace AS namespace,
                    c.name AS name,
                    c.component_id AS component_id,
                    cv.version AS version,
                    cv.size AS size,
                    cv.metadata AS metadata
                FROM components c
                    JOIN component_versions cv ON c.component_id = cv.component_id
                WHERE c.namespace = $1 AND c.component_id = $2 AND cv.version = $3
                "#,
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
            r#"
                SELECT
                    c.namespace AS namespace,
                    c.name AS name,
                    c.component_id AS component_id,
                    cv.version AS version,
                    cv.size AS size,
                    cv.metadata AS metadata
                FROM components c
                    JOIN component_versions cv ON c.component_id = cv.component_id
                WHERE c.namespace = $1 AND c.name = $2
                "#,
        )
        .bind(namespace)
        .bind(name)
        .fetch_all(self.db_pool.deref())
        .await
        .map_err(|e| e.into())
    }

    async fn get_id_by_name(&self, namespace: &str, name: &str) -> Result<Option<Uuid>, RepoError> {
        let result =
            sqlx::query("SELECT component_id FROM components WHERE namespace = $1 AND name = $2")
                .bind(namespace)
                .bind(name)
                .fetch_optional(self.db_pool.deref())
                .await?;

        Ok(result.map(|x| x.get("component_id")))
    }

    async fn get_namespace(&self, component_id: &Uuid) -> Result<Option<String>, RepoError> {
        let result = sqlx::query("SELECT namespace FROM components WHERE component_id = $1")
            .bind(component_id)
            .fetch_optional(self.db_pool.deref())
            .await?;

        Ok(result.map(|x| x.get("namespace")))
    }

    async fn delete(&self, namespace: &str, component_id: &Uuid) -> Result<(), RepoError> {
        let mut transaction = self.db_pool.begin().await?;
        sqlx::query(
            r#"
                DELETE FROM component_versions
                WHERE component_id IN (SELECT component_id FROM components WHERE namespace = $1 AND component_id = $2)
            "#
        )
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
    use bytes::{BufMut, Bytes, BytesMut};
    use golem_api_grpc::proto::golem::component::ComponentMetadata as ComponentMetadataProto;
    use golem_service_base::model::ComponentMetadata;
    use prost::Message;

    pub const SERIALIZATION_VERSION_V1: u8 = 1u8;

    pub fn serialize(value: &ComponentMetadata) -> Result<Bytes, String> {
        let proto_value: ComponentMetadataProto = value.clone().into();
        let mut bytes = BytesMut::new();
        bytes.put_u8(SERIALIZATION_VERSION_V1);
        bytes.extend_from_slice(&proto_value.encode_to_vec());
        Ok(bytes.freeze())
    }

    pub fn deserialize(bytes: &[u8]) -> Result<ComponentMetadata, String> {
        let (version, data) = bytes.split_at(1);

        match version[0] {
            SERIALIZATION_VERSION_V1 => {
                let proto_value: ComponentMetadataProto = Message::decode(data)
                    .map_err(|e| format!("Failed to deserialize value: {e}"))?;
                let value = proto_value.try_into()?;
                Ok(value)
            }
            _ => Err("Unsupported serialization version".to_string()),
        }
    }
}
