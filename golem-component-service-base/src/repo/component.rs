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

use crate::model::{Component, ComponentConstraints};
use async_trait::async_trait;
use conditional_trait_gen::{trait_gen, when};
use golem_common::model::component_constraint::FunctionConstraintCollection;
use golem_common::model::component_metadata::ComponentMetadata;
use golem_common::model::{ComponentId, ComponentType};
use golem_service_base::model::{ComponentName, VersionedComponentId};
use golem_service_base::repo::RepoError;
use sqlx::{Database, Pool, Row};
use std::fmt::Display;
use std::ops::Deref;
use std::result::Result;
use std::sync::Arc;
use tracing::{debug, error};
use uuid::Uuid;

#[derive(sqlx::FromRow, Debug, Clone)]
pub struct ComponentRecord {
    pub namespace: String,
    pub component_id: Uuid,
    pub name: String,
    pub size: i32,
    pub version: i64,
    pub metadata: Vec<u8>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub component_type: i32,
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
        let namespace = Namespace::try_from(value.namespace).map_err(|e| e.to_string())?;
        Ok(Component {
            namespace,
            component_name: ComponentName(value.name),
            component_size: value.size as u64,
            metadata,
            versioned_component_id,
            created_at: value.created_at,
            component_type: ComponentType::try_from(value.component_type)?,
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
            created_at: value.created_at,
            component_type: value.component_type as i32,
        })
    }
}

#[derive(sqlx::FromRow, Debug, Clone)]
pub struct ComponentConstraintsRecord {
    pub namespace: String,
    pub component_id: Uuid,
    pub constraints: Vec<u8>,
}

impl<Namespace> TryFrom<ComponentConstraints<Namespace>> for ComponentConstraintsRecord
where
    Namespace: Display,
{
    type Error = String;

    fn try_from(value: ComponentConstraints<Namespace>) -> Result<Self, Self::Error> {
        let metadata = constraint_serde::serialize(&value.constraints)?;
        Ok(Self {
            namespace: value.namespace.to_string(),
            component_id: value.component_id.0,
            constraints: metadata.into(),
        })
    }
}

impl<Namespace> TryFrom<ComponentConstraintsRecord> for ComponentConstraints<Namespace>
where
    Namespace: Display + TryFrom<String> + Eq + Clone + Send + Sync,
    <Namespace as TryFrom<String>>::Error: Display + Send + Sync + 'static,
{
    type Error = String;
    fn try_from(value: ComponentConstraintsRecord) -> Result<Self, Self::Error> {
        let function_constraints: FunctionConstraintCollection =
            constraint_serde::deserialize(&value.constraints)?;
        let namespace = Namespace::try_from(value.namespace).map_err(|e| e.to_string())?;
        Ok(ComponentConstraints {
            namespace,
            component_id: ComponentId(value.component_id),
            constraints: function_constraints,
        })
    }
}

#[async_trait]
pub trait ComponentRepo {
    async fn create(&self, component: &ComponentRecord) -> Result<(), RepoError>;

    async fn get(&self, component_id: &Uuid) -> Result<Vec<ComponentRecord>, RepoError>;

    async fn get_all(&self, namespace: &str) -> Result<Vec<ComponentRecord>, RepoError>;

    async fn get_latest_version(
        &self,
        component_id: &Uuid,
    ) -> Result<Option<ComponentRecord>, RepoError>;

    async fn get_by_version(
        &self,
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

    async fn create_or_update_constraint(
        &self,
        component_constraint_record: &ComponentConstraintsRecord,
    ) -> Result<(), RepoError>;

    async fn get_constraint(
        &self,
        component_id: &ComponentId,
    ) -> Result<Option<FunctionConstraintCollection>, RepoError>;
}

pub struct DbComponentRepo<DB: Database> {
    db_pool: Arc<Pool<DB>>,
}

impl<DB: Database> DbComponentRepo<DB> {
    pub fn new(db_pool: Arc<Pool<DB>>) -> Self {
        Self { db_pool }
    }
}

pub struct LoggedComponentRepo<Repo: ComponentRepo> {
    repo: Repo,
}

impl<Repo: ComponentRepo> LoggedComponentRepo<Repo> {
    pub fn new(repo: Repo) -> Self {
        Self { repo }
    }

    fn logged<R>(message: &'static str, result: Result<R, RepoError>) -> Result<R, RepoError> {
        match &result {
            Ok(_) => debug!("{}", message),
            Err(error) => error!(error = error.to_string(), "{message}"),
        }
        result
    }

    fn logged_with_id<R>(
        message: &'static str,
        component_id: &Uuid,
        result: Result<R, RepoError>,
    ) -> Result<R, RepoError> {
        match &result {
            Ok(_) => debug!(component_id = component_id.to_string(), "{}", message),
            Err(error) => error!(
                component_id = component_id.to_string(),
                error = error.to_string(),
                "{message}"
            ),
        }
        result
    }
}

#[async_trait]
impl<Repo: ComponentRepo + Send + Sync> ComponentRepo for LoggedComponentRepo<Repo> {
    async fn create(&self, component: &ComponentRecord) -> Result<(), RepoError> {
        let result = self.repo.create(component).await;
        Self::logged_with_id("create", &component.component_id, result)
    }

    async fn get(&self, component_id: &Uuid) -> Result<Vec<ComponentRecord>, RepoError> {
        let result = self.repo.get(component_id).await;
        Self::logged_with_id("get", component_id, result)
    }

    async fn get_all(&self, namespace: &str) -> Result<Vec<ComponentRecord>, RepoError> {
        let result = self.repo.get_all(namespace).await;
        Self::logged("get_all", result)
    }

    async fn get_latest_version(
        &self,
        component_id: &Uuid,
    ) -> Result<Option<ComponentRecord>, RepoError> {
        let result = self.repo.get_latest_version(component_id).await;
        Self::logged_with_id("get_latest_version", component_id, result)
    }

    async fn get_by_version(
        &self,
        component_id: &Uuid,
        version: u64,
    ) -> Result<Option<ComponentRecord>, RepoError> {
        let result = self.repo.get_by_version(component_id, version).await;
        Self::logged_with_id("get_by_version", component_id, result)
    }

    async fn get_by_name(
        &self,
        namespace: &str,
        name: &str,
    ) -> Result<Vec<ComponentRecord>, RepoError> {
        let result = self.repo.get_by_name(namespace, name).await;
        Self::logged("get_by_name", result)
    }

    async fn get_id_by_name(&self, namespace: &str, name: &str) -> Result<Option<Uuid>, RepoError> {
        let result = self.repo.get_id_by_name(namespace, name).await;
        Self::logged("get_id_by_name", result)
    }

    async fn get_namespace(&self, component_id: &Uuid) -> Result<Option<String>, RepoError> {
        let result = self.repo.get_namespace(component_id).await;
        Self::logged_with_id("get_namespace", component_id, result)
    }

    async fn delete(&self, namespace: &str, component_id: &Uuid) -> Result<(), RepoError> {
        let result = self.repo.delete(namespace, component_id).await;
        Self::logged_with_id("delete", component_id, result)
    }

    async fn create_or_update_constraint(
        &self,
        component_constraint_record: &ComponentConstraintsRecord,
    ) -> Result<(), RepoError> {
        let result = self
            .repo
            .create_or_update_constraint(component_constraint_record)
            .await;
        Self::logged("create_component_constraint", result)
    }

    async fn get_constraint(
        &self,
        component_id: &ComponentId,
    ) -> Result<Option<FunctionConstraintCollection>, RepoError> {
        let result = self.repo.get_constraint(component_id).await;

        Self::logged("get_component_constraint", result)
    }
}

#[trait_gen(sqlx::Postgres -> sqlx::Postgres, sqlx::Sqlite)]
#[async_trait]
impl ComponentRepo for DbComponentRepo<sqlx::Postgres> {
    async fn create(&self, component: &ComponentRecord) -> Result<(), RepoError> {
        let mut transaction = self.db_pool.begin().await?;

        let result = sqlx::query("SELECT namespace, name FROM components WHERE component_id = $1")
            .bind(component.component_id)
            .fetch_optional(&mut *transaction)
            .await?;

        if let Some(result) = result {
            let namespace: String = result.get("namespace");
            let name: String = result.get("name");
            if namespace != component.namespace || name != component.name {
                return Err(RepoError::Internal(
                    "Component namespace and name invalid".to_string(),
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
                (component_id, version, size, metadata, created_at, component_type)
              VALUES
                ($1, $2, $3, $4, $5, $6)
               "#,
        )
        .bind(component.component_id)
        .bind(component.version)
        .bind(component.size)
        .bind(component.metadata.clone())
        .bind(component.created_at)
        .bind(component.component_type)
        .execute(&mut *transaction)
        .await?;

        transaction.commit().await?;

        Ok(())
    }

    async fn get(&self, component_id: &Uuid) -> Result<Vec<ComponentRecord>, RepoError> {
        sqlx::query_as::<_, ComponentRecord>(
            r#"
                SELECT
                    c.namespace AS namespace,
                    c.name AS name,
                    c.component_id AS component_id,
                    cv.version AS version,
                    cv.size AS size,
                    cv.metadata AS metadata,
                    cv.created_at::timestamptz AS created_at,
                    cv.component_type AS component_type
                FROM components c
                    JOIN component_versions cv ON c.component_id = cv.component_id
                WHERE c.component_id = $1
                "#,
        )
        .bind(component_id)
        .fetch_all(self.db_pool.deref())
        .await
        .map_err(|e| e.into())
    }

    #[when(sqlx::Postgres -> get_all)]
    async fn get_all_postgres(&self, namespace: &str) -> Result<Vec<ComponentRecord>, RepoError> {
        sqlx::query_as::<_, ComponentRecord>(
            r#"
                SELECT
                    c.namespace AS namespace,
                    c.name AS name,
                    c.component_id AS component_id,
                    cv.version AS version,
                    cv.size AS size,
                    cv.metadata AS metadata,
                    cv.created_at::timestamptz AS created_at,
                    cv.component_type AS component_type
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

    #[when(sqlx::Sqlite -> get_all)]
    async fn get_all_sqlite(&self, namespace: &str) -> Result<Vec<ComponentRecord>, RepoError> {
        sqlx::query_as::<_, ComponentRecord>(
            r#"
                SELECT
                    c.namespace AS namespace,
                    c.name AS name,
                    c.component_id AS component_id,
                    cv.version AS version,
                    cv.size AS size,
                    cv.metadata AS metadata,
                    cv.created_at AS created_at,
                    cv.component_type AS component_type
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

    #[when(sqlx::Postgres -> get_latest_version)]
    async fn get_latest_version_postgres(
        &self,
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
                    cv.metadata AS metadata,
                    cv.created_at::timestamptz AS created_at,
                    cv.component_type AS component_type
                FROM components c
                    JOIN component_versions cv ON c.component_id = cv.component_id
                WHERE c.component_id = $1
                ORDER BY cv.version DESC LIMIT 1
                "#,
        )
        .bind(component_id)
        .fetch_optional(self.db_pool.deref())
        .await
        .map_err(|e| e.into())
    }

    #[when(sqlx::Sqlite -> get_latest_version)]
    async fn get_latest_version_sqlite(
        &self,
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
                    cv.metadata AS metadata,
                    cv.created_at AS created_at,
                    cv.component_type AS component_type
                FROM components c
                    JOIN component_versions cv ON c.component_id = cv.component_id
                WHERE c.component_id = $1
                ORDER BY cv.version DESC LIMIT 1
                "#,
        )
        .bind(component_id)
        .fetch_optional(self.db_pool.deref())
        .await
        .map_err(|e| e.into())
    }

    #[when(sqlx::Postgres -> get_by_version)]
    async fn get_by_version_postgres(
        &self,
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
                    cv.metadata AS metadata,
                    cv.created_at::timestamptz AS created_at,
                    cv.component_type AS component_type
                FROM components c
                    JOIN component_versions cv ON c.component_id = cv.component_id
                WHERE c.component_id = $1 AND cv.version = $2
                "#,
        )
        .bind(component_id)
        .bind(version as i64)
        .fetch_optional(self.db_pool.deref())
        .await
        .map_err(|e| e.into())
    }

    #[when(sqlx::Sqlite -> get_by_version)]
    async fn get_by_version_sqlite(
        &self,
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
                    cv.metadata AS metadata,
                    cv.created_at AS created_at,
                    cv.component_type AS component_type
                FROM components c
                    JOIN component_versions cv ON c.component_id = cv.component_id
                WHERE c.component_id = $1 AND cv.version = $2
                "#,
        )
        .bind(component_id)
        .bind(version as i64)
        .fetch_optional(self.db_pool.deref())
        .await
        .map_err(|e| e.into())
    }

    #[when(sqlx::Postgres -> get_by_name)]
    async fn get_by_name_postgres(
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
                    cv.metadata AS metadata,
                    cv.created_at::timestamptz AS created_at,
                    cv.component_type AS component_type
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

    #[when(sqlx::Sqlite -> get_by_name)]
    async fn get_by_name_sqlite(
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
                    cv.metadata AS metadata,
                    cv.created_at AS created_at,
                    cv.component_type AS component_type
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

    async fn create_or_update_constraint(
        &self,
        component_constraint_record: &ComponentConstraintsRecord,
    ) -> Result<(), RepoError> {
        let component_constraint_record = component_constraint_record.clone();
        let mut transaction = self.db_pool.begin().await?;

        let existing_record = sqlx::query_as::<_, ComponentConstraintsRecord>(
            r#"
                SELECT
                    namespace,
                    component_id,
                    constraints
                FROM component_constraints WHERE component_id = $1
                "#,
        )
        .bind(component_constraint_record.component_id)
        .fetch_optional(&mut *transaction)
        .await
        .map_err(|e| RepoError::Internal(e.to_string()))?;

        if let Some(existing_record) = existing_record {
            let existing_worker_calls_used =
                constraint_serde::deserialize(&existing_record.constraints)
                    .map_err(RepoError::Internal)?;
            let new_worker_calls_used =
                constraint_serde::deserialize(&component_constraint_record.constraints)
                    .map_err(RepoError::Internal)?;

            // This shouldn't happen as it is validated in service layers.
            // However, repo gives us more transactional guarantee.
            let merged_worker_calls = FunctionConstraintCollection::try_merge(vec![
                existing_worker_calls_used,
                new_worker_calls_used,
            ])
            .map_err(RepoError::Internal)?;

            // Serialize the merged result back to store in the database
            let merged_constraint_data: Vec<u8> = constraint_serde::serialize(&merged_worker_calls)
                .map_err(RepoError::Internal)?
                .into();

            // Update the existing record in the database
            sqlx::query(
                r#"
                 UPDATE
                   component_constraints
                    SET constraints = $1
                    WHERE namespace = $2 AND component_id = $3
                    "#,
            )
            .bind(merged_constraint_data)
            .bind(component_constraint_record.namespace)
            .bind(component_constraint_record.component_id)
            .execute(&mut *transaction)
            .await
            .map_err(RepoError::from)?;
        } else {
            sqlx::query(
                r#"
              INSERT INTO component_constraints
                (namespace, component_id, constraints)
              VALUES
                ($1, $2, $3)
               "#,
            )
            .bind(component_constraint_record.namespace)
            .bind(component_constraint_record.component_id)
            .bind(component_constraint_record.constraints)
            .execute(&mut *transaction)
            .await?;
        }

        transaction.commit().await?;

        Ok(())
    }

    async fn get_constraint(
        &self,
        component_id: &ComponentId,
    ) -> Result<Option<FunctionConstraintCollection>, RepoError> {
        let existing_record = sqlx::query_as::<_, ComponentConstraintsRecord>(
            r#"
                SELECT
                    namespace,
                    component_id,
                    constraints
                FROM component_constraints WHERE component_id = $1
                "#,
        )
        .bind(component_id.0)
        .fetch_optional(self.db_pool.deref())
        .await
        .map_err(|e| RepoError::Internal(e.to_string()))?;

        if let Some(existing_record) = existing_record {
            let existing_worker_calls_used =
                constraint_serde::deserialize(&existing_record.constraints)
                    .map_err(RepoError::Internal)?;
            Ok(Some(existing_worker_calls_used))
        } else {
            Ok(None)
        }
    }
}

pub mod record_metadata_serde {
    use bytes::{BufMut, Bytes, BytesMut};
    use golem_api_grpc::proto::golem::component::ComponentMetadata as ComponentMetadataProto;
    use golem_common::model::component_metadata::ComponentMetadata;
    use prost::Message;

    pub const SERIALIZATION_VERSION_V1: u8 = 1u8;

    pub fn serialize(value: &ComponentMetadata) -> Result<Bytes, String> {
        let proto_value: ComponentMetadataProto = ComponentMetadataProto::from(value.clone());
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
                let value = ComponentMetadata::try_from(proto_value)?;
                Ok(value)
            }
            _ => Err("Unsupported serialization version".to_string()),
        }
    }
}

pub mod constraint_serde {
    use bytes::{BufMut, Bytes, BytesMut};
    use golem_api_grpc::proto::golem::component::FunctionConstraintCollection as FunctionConstraintCollectionProto;
    use golem_common::model::component_constraint::FunctionConstraintCollection;
    use prost::Message;

    pub const SERIALIZATION_VERSION_V1: u8 = 1u8;

    pub fn serialize(value: &FunctionConstraintCollection) -> Result<Bytes, String> {
        let proto_value: FunctionConstraintCollectionProto =
            FunctionConstraintCollectionProto::from(value.clone());

        let mut bytes = BytesMut::new();
        bytes.put_u8(SERIALIZATION_VERSION_V1);
        bytes.extend_from_slice(&proto_value.encode_to_vec());
        Ok(bytes.freeze())
    }

    pub fn deserialize(bytes: &[u8]) -> Result<FunctionConstraintCollection, String> {
        let (version, data) = bytes.split_at(1);

        match version[0] {
            SERIALIZATION_VERSION_V1 => {
                let proto_value: FunctionConstraintCollectionProto = Message::decode(data)
                    .map_err(|e| format!("Failed to deserialize value: {e}"))?;

                let value = FunctionConstraintCollection::try_from(proto_value.clone())?;

                Ok(value)
            }
            _ => Err("Unsupported serialization version".to_string()),
        }
    }
}
