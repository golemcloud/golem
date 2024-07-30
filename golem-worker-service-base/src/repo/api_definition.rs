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

use crate::api_definition::http::HttpApiDefinition;
use crate::repo::RepoError;
use async_trait::async_trait;
use sqlx::{Database, Pool, Row};
use std::fmt::Display;
use std::ops::Deref;
use std::sync::Arc;

#[derive(sqlx::FromRow, Debug, Clone)]
pub struct ApiDefinitionRecord {
    pub namespace: String,
    pub id: String,
    pub version: String,
    pub draft: bool,
    pub data: Vec<u8>,
}

impl ApiDefinitionRecord {
    pub fn new<Namespace: Display>(
        namespace: Namespace,
        definition: HttpApiDefinition,
    ) -> Result<Self, String> {
        let data = record_data_serde::serialize(&definition.routes)?;
        Ok(Self {
            namespace: namespace.to_string(),
            id: definition.id.0,
            version: definition.version.0,
            draft: definition.draft,
            data: data.into(),
        })
    }
}

impl TryFrom<ApiDefinitionRecord> for HttpApiDefinition {
    type Error = String;
    fn try_from(value: ApiDefinitionRecord) -> Result<Self, Self::Error> {
        let routes = record_data_serde::deserialize(&value.data)?;

        Ok(Self {
            id: value.id.into(),
            version: value.version.into(),
            routes,
            draft: value.draft,
        })
    }
}

#[async_trait]
pub trait ApiDefinitionRepo {
    async fn create(&self, definition: &ApiDefinitionRecord) -> Result<(), RepoError>;

    async fn update(&self, definition: &ApiDefinitionRecord) -> Result<(), RepoError>;

    async fn set_not_draft(
        &self,
        namespace: &str,
        id: &str,
        version: &str,
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

pub struct DbApiDefinitionRepo<DB: Database> {
    db_pool: Arc<Pool<DB>>,
}

impl<DB: Database> DbApiDefinitionRepo<DB> {
    pub fn new(db_pool: Arc<Pool<DB>>) -> Self {
        Self { db_pool }
    }
}

#[async_trait]
impl ApiDefinitionRepo for DbApiDefinitionRepo<sqlx::Sqlite> {
    async fn create(&self, definition: &ApiDefinitionRecord) -> Result<(), RepoError> {
        sqlx::query(
            r#"
              INSERT INTO api_definitions
                (namespace, id, version, draft, data)
              VALUES
                ($1, $2, $3, $4, $5)
               "#,
        )
        .bind(definition.namespace.clone())
        .bind(definition.id.clone())
        .bind(definition.version.clone())
        .bind(definition.draft)
        .bind(definition.data.clone())
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
        .execute(self.db_pool.deref())
        .await?;

        Ok(())
    }

    async fn set_not_draft(
        &self,
        namespace: &str,
        id: &str,
        version: &str,
    ) -> Result<(), RepoError> {
        sqlx::query(
            r#"
              UPDATE api_definitions
              SET draft = false
              WHERE namespace = $1 AND id = $2 AND version = $3
               "#,
        )
        .bind(namespace)
        .bind(id)
        .bind(version)
        .execute(self.db_pool.deref())
        .await?;

        Ok(())
    }

    async fn get(
        &self,
        namespace: &str,
        id: &str,
        version: &str,
    ) -> Result<Option<ApiDefinitionRecord>, RepoError> {
        sqlx::query_as::<_, ApiDefinitionRecord>("SELECT namespace, id, version, draft, data FROM api_definitions WHERE namespace = $1 AND id = $2 AND version = $3")
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

    async fn get_all(&self, namespace: &str) -> Result<Vec<ApiDefinitionRecord>, RepoError> {
        sqlx::query_as::<_, ApiDefinitionRecord>(
            "SELECT namespace, id, version, draft, data FROM api_definitions WHERE namespace = $1",
        )
        .bind(namespace)
        .fetch_all(self.db_pool.deref())
        .await
        .map_err(|e| e.into())
    }

    async fn get_all_versions(
        &self,
        namespace: &str,
        id: &str,
    ) -> Result<Vec<ApiDefinitionRecord>, RepoError> {
        sqlx::query_as::<_, ApiDefinitionRecord>("SELECT namespace, id, version, draft, data FROM api_definitions WHERE namespace = $1 AND id = $2")
            .bind(namespace)
            .bind(id)
            .fetch_all(self.db_pool.deref())
            .await
            .map_err(|e| e.into())
    }
}

#[async_trait]
impl ApiDefinitionRepo for DbApiDefinitionRepo<sqlx::Postgres> {
    async fn create(&self, definition: &ApiDefinitionRecord) -> Result<(), RepoError> {
        sqlx::query(
            r#"
              INSERT INTO api_definitions
                (namespace, id, version, draft, data)
              VALUES
                ($1, $2, $3, $4, $5)
               "#,
        )
        .bind(definition.namespace.clone())
        .bind(definition.id.clone())
        .bind(definition.version.clone())
        .bind(definition.draft)
        .bind(definition.data.clone())
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
        .execute(self.db_pool.deref())
        .await?;

        Ok(())
    }

    async fn set_not_draft(
        &self,
        namespace: &str,
        id: &str,
        version: &str,
    ) -> Result<(), RepoError> {
        sqlx::query(
            r#"
              UPDATE api_definitions
              SET draft = false
              WHERE namespace = $1 AND id = $2 AND version = $3
               "#,
        )
        .bind(namespace)
        .bind(id)
        .bind(version)
        .execute(self.db_pool.deref())
        .await?;

        Ok(())
    }

    async fn get(
        &self,
        namespace: &str,
        id: &str,
        version: &str,
    ) -> Result<Option<ApiDefinitionRecord>, RepoError> {
        sqlx::query_as::<_, ApiDefinitionRecord>("SELECT namespace, id, version, draft, data FROM api_definitions WHERE namespace = $1 AND id = $2 AND version = $3")
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

    async fn get_all(&self, namespace: &str) -> Result<Vec<ApiDefinitionRecord>, RepoError> {
        sqlx::query_as::<_, ApiDefinitionRecord>(
            "SELECT namespace, id, version, draft, data FROM api_definitions WHERE namespace = $1",
        )
        .bind(namespace)
        .fetch_all(self.db_pool.deref())
        .await
        .map_err(|e| e.into())
    }

    async fn get_all_versions(
        &self,
        namespace: &str,
        id: &str,
    ) -> Result<Vec<ApiDefinitionRecord>, RepoError> {
        sqlx::query_as::<_, ApiDefinitionRecord>("SELECT namespace, id, version, draft, data FROM api_definitions WHERE namespace = $1 AND id = $2")
            .bind(namespace)
            .bind(id)
            .fetch_all(self.db_pool.deref())
            .await
            .map_err(|e| e.into())
    }
}

pub mod record_data_serde {
    use crate::api_definition::http::Route;
    use bytes::{BufMut, Bytes, BytesMut};
    use golem_api_grpc::proto::golem::apidefinition::{HttpApiDefinition, HttpRoute};
    use prost::Message;

    pub const SERIALIZATION_VERSION_V1: u8 = 1u8;

    pub fn serialize(value: &[Route]) -> Result<Bytes, String> {
        let routes: Vec<HttpRoute> = value
            .iter()
            .cloned()
            .map(HttpRoute::try_from)
            .collect::<Result<Vec<HttpRoute>, String>>()?;

        let proto_value: HttpApiDefinition = HttpApiDefinition { routes };

        let mut bytes = BytesMut::new();
        bytes.put_u8(SERIALIZATION_VERSION_V1);
        bytes.extend_from_slice(&proto_value.encode_to_vec());
        Ok(bytes.freeze())
    }

    pub fn deserialize(bytes: &[u8]) -> Result<Vec<Route>, String> {
        let (version, data) = bytes.split_at(1);

        match version[0] {
            SERIALIZATION_VERSION_V1 => {
                let proto_value: HttpApiDefinition = Message::decode(data)
                    .map_err(|e| format!("Failed to deserialize value: {e}"))?;

                let value = proto_value
                    .routes
                    .into_iter()
                    .map(Route::try_from)
                    .collect::<Result<Vec<Route>, String>>()?;

                Ok(value)
            }
            _ => Err("Unsupported serialization version".to_string()),
        }
    }
}
