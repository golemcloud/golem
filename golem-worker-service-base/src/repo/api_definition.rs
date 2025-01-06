// Copyright 2024-2025 Golem Cloud
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

use crate::gateway_api_definition::http::{CompiledHttpApiDefinition, HttpApiDefinition};
use async_trait::async_trait;
use conditional_trait_gen::{trait_gen, when};
use golem_service_base::repo::RepoError;
use sqlx::{Database, Pool, Row};
use std::fmt::Display;
use std::ops::Deref;
use std::sync::Arc;
use tracing::{debug, error};

#[derive(sqlx::FromRow, Debug, Clone)]
pub struct ApiDefinitionRecord {
    pub namespace: String,
    pub id: String,
    pub version: String,
    pub draft: bool,
    pub data: Vec<u8>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

impl ApiDefinitionRecord {
    pub fn new<Namespace: Display>(
        definition: CompiledHttpApiDefinition<Namespace>,
        created_at: chrono::DateTime<chrono::Utc>,
    ) -> Result<Self, String> {
        let data = record_data_serde::serialize(&definition.routes)?;
        Ok(Self {
            namespace: definition.namespace.to_string(),
            id: definition.id.0,
            version: definition.version.0,
            draft: definition.draft,
            data: data.into(),
            created_at,
        })
    }
}

impl<Namespace> TryFrom<ApiDefinitionRecord> for CompiledHttpApiDefinition<Namespace>
where
    Namespace: TryFrom<String>,
    <Namespace as TryFrom<String>>::Error: Display,
{
    type Error = String;
    fn try_from(value: ApiDefinitionRecord) -> Result<Self, Self::Error> {
        let routes = record_data_serde::deserialize(&value.data)?;

        let namespace = Namespace::try_from(value.namespace)
            .map_err(|e| format!("Failed to convert namespace: {e}"))?;

        Ok(Self {
            id: value.id.into(),
            version: value.version.into(),
            routes,
            draft: value.draft,
            created_at: value.created_at,
            namespace,
        })
    }
}

impl TryFrom<ApiDefinitionRecord> for HttpApiDefinition {
    type Error = String;
    fn try_from(value: ApiDefinitionRecord) -> Result<Self, Self::Error> {
        let routes = record_data_serde::deserialize(&value.data)?;

        let routes = routes
            .into_iter()
            .map(crate::gateway_api_definition::http::Route::from)
            .collect();

        Ok(Self {
            id: value.id.into(),
            version: value.version.into(),
            routes,
            draft: value.draft,
            created_at: value.created_at,
        })
    }
}

#[async_trait]
pub trait ApiDefinitionRepo {
    async fn create(&self, definition: &ApiDefinitionRecord) -> Result<(), RepoError>;

    async fn update(&self, definition: &ApiDefinitionRecord) -> Result<(), RepoError>;

    async fn set_draft(
        &self,
        namespace: &str,
        id: &str,
        version: &str,
        draft: bool,
    ) -> Result<(), RepoError>;

    async fn get(
        &self,
        namespace: &str,
        id: &str,
        version: &str,
    ) -> Result<Option<ApiDefinitionRecord>, RepoError>;

    async fn get_draft(
        &self,
        namespace: &str,
        id: &str,
        version: &str,
    ) -> Result<Option<bool>, RepoError>;

    async fn delete(&self, namespace: &str, id: &str, version: &str) -> Result<bool, RepoError>;

    async fn get_all(&self, namespace: &str) -> Result<Vec<ApiDefinitionRecord>, RepoError>;

    async fn get_all_versions(
        &self,
        namespace: &str,
        id: &str,
    ) -> Result<Vec<ApiDefinitionRecord>, RepoError>;
}

pub struct LoggedApiDefinitionRepo<Repo: ApiDefinitionRepo> {
    repo: Repo,
}

impl<Repo: ApiDefinitionRepo> LoggedApiDefinitionRepo<Repo> {
    pub fn new(repo: Repo) -> Self {
        Self { repo }
    }

    fn logged_with_id<R>(
        message: &'static str,
        namespace: &str,
        api_definition_id: &str,
        result: Result<R, RepoError>,
    ) -> Result<R, RepoError> {
        match &result {
            Ok(_) => debug!(
                namespace = namespace,
                api_definition_id = api_definition_id.to_string(),
                "{}",
                message
            ),
            Err(error) => error!(
                namespace = namespace,
                api_definition_id = api_definition_id.to_string(),
                error = error.to_string(),
                "{message}"
            ),
        }
        result
    }
}

#[async_trait]
impl<Repo: ApiDefinitionRepo + Sync> ApiDefinitionRepo for LoggedApiDefinitionRepo<Repo> {
    async fn create(&self, definition: &ApiDefinitionRecord) -> Result<(), RepoError> {
        let result = self.repo.create(definition).await;
        Self::logged_with_id("create", &definition.namespace, &definition.id, result)
    }

    async fn update(&self, definition: &ApiDefinitionRecord) -> Result<(), RepoError> {
        let result = self.repo.update(definition).await;
        Self::logged_with_id("update", &definition.namespace, &definition.id, result)
    }

    async fn set_draft(
        &self,
        namespace: &str,
        id: &str,
        version: &str,
        draft: bool,
    ) -> Result<(), RepoError> {
        let result = self.repo.set_draft(namespace, id, version, draft).await;
        Self::logged_with_id("set_draft", namespace, id, result)
    }

    async fn get(
        &self,
        namespace: &str,
        id: &str,
        version: &str,
    ) -> Result<Option<ApiDefinitionRecord>, RepoError> {
        let result = self.repo.get(namespace, id, version).await;
        Self::logged_with_id("get", namespace, id, result)
    }

    async fn get_draft(
        &self,
        namespace: &str,
        id: &str,
        version: &str,
    ) -> Result<Option<bool>, RepoError> {
        let result = self.repo.get_draft(namespace, id, version).await;
        Self::logged_with_id("get_draft", namespace, id, result)
    }

    async fn delete(&self, namespace: &str, id: &str, version: &str) -> Result<bool, RepoError> {
        let result = self.repo.delete(namespace, id, version).await;
        Self::logged_with_id("delete", namespace, id, result)
    }

    async fn get_all(&self, namespace: &str) -> Result<Vec<ApiDefinitionRecord>, RepoError> {
        let result = self.repo.get_all(namespace).await;
        Self::logged_with_id("get_all", namespace, "*", result)
    }

    async fn get_all_versions(
        &self,
        namespace: &str,
        id: &str,
    ) -> Result<Vec<ApiDefinitionRecord>, RepoError> {
        let result = self.repo.get_all_versions(namespace, id).await;
        Self::logged_with_id("get_all_versions", namespace, id, result)
    }
}

pub struct DbApiDefinitionRepo<DB: Database> {
    db_pool: Arc<Pool<DB>>,
}

impl<DB: Database> DbApiDefinitionRepo<DB> {
    pub fn new(db_pool: Arc<Pool<DB>>) -> Self {
        Self { db_pool }
    }
}

#[trait_gen(sqlx::Postgres -> sqlx::Postgres, sqlx::Sqlite)]
#[async_trait]
impl ApiDefinitionRepo for DbApiDefinitionRepo<sqlx::Postgres> {
    async fn create(&self, definition: &ApiDefinitionRecord) -> Result<(), RepoError> {
        sqlx::query(
            r#"
              INSERT INTO api_definitions
                (namespace, id, version, draft, data, created_at)
              VALUES
                ($1, $2, $3, $4, $5, $6)
               "#,
        )
        .bind(definition.namespace.clone())
        .bind(definition.id.clone())
        .bind(definition.version.clone())
        .bind(definition.draft)
        .bind(definition.data.clone())
        .bind(definition.created_at)
        .execute(self.db_pool.deref())
        .await?;

        Ok(())
    }

    async fn update(&self, definition: &ApiDefinitionRecord) -> Result<(), RepoError> {
        sqlx::query(
            r#"
              UPDATE api_definitions
              SET draft = $4, data = $5
              WHERE namespace = $1 AND id = $2 AND version = $3
               "#,
        )
        .bind(definition.namespace.clone())
        .bind(definition.id.clone())
        .bind(definition.version.clone())
        .bind(definition.draft)
        .bind(definition.data.clone())
        .bind(definition.created_at)
        .execute(self.db_pool.deref())
        .await?;

        Ok(())
    }

    async fn set_draft(
        &self,
        namespace: &str,
        id: &str,
        version: &str,
        draft: bool,
    ) -> Result<(), RepoError> {
        sqlx::query(
            r#"
              UPDATE api_definitions
              SET draft = $4
              WHERE namespace = $1 AND id = $2 AND version = $3
               "#,
        )
        .bind(namespace)
        .bind(id)
        .bind(version)
        .bind(draft)
        .execute(self.db_pool.deref())
        .await?;

        Ok(())
    }

    #[when(sqlx::Postgres -> get)]
    async fn get_postgres(
        &self,
        namespace: &str,
        id: &str,
        version: &str,
    ) -> Result<Option<ApiDefinitionRecord>, RepoError> {
        sqlx::query_as::<_, ApiDefinitionRecord>("SELECT namespace, id, version, draft, data, created_at::timestamptz FROM api_definitions WHERE namespace = $1 AND id = $2 AND version = $3")
            .bind(namespace)
            .bind(id)
            .bind(version)
            .fetch_optional(self.db_pool.deref())
            .await
            .map_err(|e| e.into())
    }

    #[when(sqlx::Sqlite -> get)]
    async fn get_sqlite(
        &self,
        namespace: &str,
        id: &str,
        version: &str,
    ) -> Result<Option<ApiDefinitionRecord>, RepoError> {
        sqlx::query_as::<_, ApiDefinitionRecord>("SELECT namespace, id, version, draft, data, created_at FROM api_definitions WHERE namespace = $1 AND id = $2 AND version = $3")
            .bind(namespace)
            .bind(id)
            .bind(version)
            .fetch_optional(self.db_pool.deref())
            .await
            .map_err(|e| e.into())
    }

    async fn get_draft(
        &self,
        namespace: &str,
        id: &str,
        version: &str,
    ) -> Result<Option<bool>, RepoError> {
        let result = sqlx::query(
            "SELECT draft FROM api_definitions WHERE namespace = $1 AND id = $2 AND version = $3",
        )
        .bind(namespace)
        .bind(id)
        .bind(version)
        .fetch_optional(self.db_pool.deref())
        .await?;

        let draft: Option<bool> = result.map(|r| r.get("draft"));
        Ok(draft)
    }

    async fn delete(&self, namespace: &str, id: &str, version: &str) -> Result<bool, RepoError> {
        let result = sqlx::query(
            "DELETE FROM api_definitions WHERE namespace = $1 AND id = $2 AND version = $3",
        )
        .bind(namespace)
        .bind(id)
        .bind(version)
        .execute(self.db_pool.deref())
        .await?;

        Ok(result.rows_affected() > 0)
    }

    #[when(sqlx::Postgres -> get_all)]
    async fn get_all_postgres(
        &self,
        namespace: &str,
    ) -> Result<Vec<ApiDefinitionRecord>, RepoError> {
        sqlx::query_as::<_, ApiDefinitionRecord>(
            "SELECT namespace, id, version, draft, data, created_at::timestamptz FROM api_definitions WHERE namespace = $1",
        )
        .bind(namespace)
        .fetch_all(self.db_pool.deref())
        .await
        .map_err(|e| e.into())
    }

    #[when(sqlx::Sqlite -> get_all)]
    async fn get_all_sqlite(&self, namespace: &str) -> Result<Vec<ApiDefinitionRecord>, RepoError> {
        sqlx::query_as::<_, ApiDefinitionRecord>(
            "SELECT namespace, id, version, draft, data, created_at FROM api_definitions WHERE namespace = $1",
        )
            .bind(namespace)
            .fetch_all(self.db_pool.deref())
            .await
            .map_err(|e| e.into())
    }

    #[when(sqlx::Postgres -> get_all_versions)]
    async fn get_all_versions_postgres(
        &self,
        namespace: &str,
        id: &str,
    ) -> Result<Vec<ApiDefinitionRecord>, RepoError> {
        sqlx::query_as::<_, ApiDefinitionRecord>("SELECT namespace, id, version, draft, data, created_at::timestamptz FROM api_definitions WHERE namespace = $1 AND id = $2")
            .bind(namespace)
            .bind(id)
            .fetch_all(self.db_pool.deref())
            .await
            .map_err(|e| e.into())
    }

    #[when(sqlx::Sqlite -> get_all_versions)]
    async fn get_all_versions_sqlite(
        &self,
        namespace: &str,
        id: &str,
    ) -> Result<Vec<ApiDefinitionRecord>, RepoError> {
        sqlx::query_as::<_, ApiDefinitionRecord>("SELECT namespace, id, version, draft, data, created_at FROM api_definitions WHERE namespace = $1 AND id = $2")
            .bind(namespace)
            .bind(id)
            .fetch_all(self.db_pool.deref())
            .await
            .map_err(|e| e.into())
    }
}

pub mod record_data_serde {
    use crate::gateway_api_definition::http::CompiledRoute;
    use bytes::{BufMut, Bytes, BytesMut};
    use golem_api_grpc::proto::golem::apidefinition::{
        CompiledHttpApiDefinition, CompiledHttpRoute,
    };
    use prost::Message;

    pub const SERIALIZATION_VERSION_V1: u8 = 1u8;

    pub fn serialize(value: &[CompiledRoute]) -> Result<Bytes, String> {
        let routes: Vec<CompiledHttpRoute> = value
            .iter()
            .cloned()
            .map(CompiledHttpRoute::try_from)
            .collect::<Result<Vec<CompiledHttpRoute>, String>>()?;

        let proto_value: CompiledHttpApiDefinition = CompiledHttpApiDefinition { routes };

        let mut bytes = BytesMut::new();
        bytes.put_u8(SERIALIZATION_VERSION_V1);
        bytes.extend_from_slice(&proto_value.encode_to_vec());
        Ok(bytes.freeze())
    }

    pub fn deserialize(bytes: &[u8]) -> Result<Vec<CompiledRoute>, String> {
        let (version, data) = bytes.split_at(1);

        match version[0] {
            SERIALIZATION_VERSION_V1 => {
                let proto_value: CompiledHttpApiDefinition = Message::decode(data)
                    .map_err(|e| format!("Failed to deserialize value: {e}"))?;

                let value = proto_value
                    .routes
                    .into_iter()
                    .map(CompiledRoute::try_from)
                    .collect::<Result<Vec<CompiledRoute>, String>>()?;

                Ok(value)
            }
            _ => Err("Unsupported serialization version".to_string()),
        }
    }
}
