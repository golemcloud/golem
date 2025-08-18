// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use crate::gateway_api_definition::http::{CompiledHttpApiDefinition, HttpApiDefinition};
use async_trait::async_trait;
use conditional_trait_gen::{trait_gen, when};
use golem_common::model::auth::Namespace;
use golem_service_base::db::Pool;
use golem_service_base::repo::RepoError;
use sqlx::Row;
use tracing::{info_span, Instrument, Span};

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
    pub fn new(
        definition: CompiledHttpApiDefinition,
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

impl TryFrom<ApiDefinitionRecord> for CompiledHttpApiDefinition {
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
pub trait ApiDefinitionRepo: Send + Sync {
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

    fn span(namespace: &str, api_definition_id: &str) -> Span {
        info_span!(
            "API definition repository",
            namespace = namespace,
            api_definition_id = api_definition_id
        )
    }
}

#[async_trait]
impl<Repo: ApiDefinitionRepo + Sync> ApiDefinitionRepo for LoggedApiDefinitionRepo<Repo> {
    async fn create(&self, definition: &ApiDefinitionRecord) -> Result<(), RepoError> {
        self.repo
            .create(definition)
            .instrument(Self::span(&definition.namespace, &definition.id))
            .await
    }

    async fn update(&self, definition: &ApiDefinitionRecord) -> Result<(), RepoError> {
        self.repo
            .update(definition)
            .instrument(Self::span(&definition.namespace, &definition.id))
            .await
    }

    async fn set_draft(
        &self,
        namespace: &str,
        id: &str,
        version: &str,
        draft: bool,
    ) -> Result<(), RepoError> {
        self.repo
            .set_draft(namespace, id, version, draft)
            .instrument(Self::span(namespace, id))
            .await
    }

    async fn get(
        &self,
        namespace: &str,
        id: &str,
        version: &str,
    ) -> Result<Option<ApiDefinitionRecord>, RepoError> {
        self.repo
            .get(namespace, id, version)
            .instrument(Self::span(namespace, id))
            .await
    }

    async fn get_draft(
        &self,
        namespace: &str,
        id: &str,
        version: &str,
    ) -> Result<Option<bool>, RepoError> {
        self.repo
            .get_draft(namespace, id, version)
            .instrument(Self::span(namespace, id))
            .await
    }

    async fn delete(&self, namespace: &str, id: &str, version: &str) -> Result<bool, RepoError> {
        self.repo
            .delete(namespace, id, version)
            .instrument(Self::span(namespace, id))
            .await
    }

    async fn get_all(&self, namespace: &str) -> Result<Vec<ApiDefinitionRecord>, RepoError> {
        self.repo
            .get_all(namespace)
            .instrument(Self::span(namespace, "*"))
            .await
    }

    async fn get_all_versions(
        &self,
        namespace: &str,
        id: &str,
    ) -> Result<Vec<ApiDefinitionRecord>, RepoError> {
        self.repo
            .get_all_versions(namespace, id)
            .instrument(Self::span(namespace, id))
            .await
    }
}

pub struct DbApiDefinitionRepo<DB: Pool> {
    db_pool: DB,
}

impl<DB: Pool> DbApiDefinitionRepo<DB> {
    pub fn new(db_pool: DB) -> Self {
        Self { db_pool }
    }
}

#[trait_gen(golem_service_base::db::postgres::PostgresPool -> golem_service_base::db::postgres::PostgresPool, golem_service_base::db::sqlite::SqlitePool
)]
#[async_trait]
impl ApiDefinitionRepo for DbApiDefinitionRepo<golem_service_base::db::postgres::PostgresPool> {
    async fn create(&self, definition: &ApiDefinitionRecord) -> Result<(), RepoError> {
        let query = sqlx::query(
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
        .bind(definition.created_at);

        self.db_pool
            .with_rw("api_definition", "create")
            .execute(query)
            .await?;

        Ok(())
    }

    async fn update(&self, definition: &ApiDefinitionRecord) -> Result<(), RepoError> {
        let query = sqlx::query(
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
        .bind(definition.created_at);

        self.db_pool
            .with_rw("api_definition", "update")
            .execute(query)
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
        let query = sqlx::query(
            r#"
              UPDATE api_definitions
              SET draft = $4
              WHERE namespace = $1 AND id = $2 AND version = $3
               "#,
        )
        .bind(namespace)
        .bind(id)
        .bind(version)
        .bind(draft);

        self.db_pool
            .with_rw("api_definition", "set_draft")
            .execute(query)
            .await?;

        Ok(())
    }

    #[when(golem_service_base::db::postgres::PostgresPool -> get)]
    async fn get_postgres(
        &self,
        namespace: &str,
        id: &str,
        version: &str,
    ) -> Result<Option<ApiDefinitionRecord>, RepoError> {
        let query = sqlx::query_as::<_, ApiDefinitionRecord>("SELECT namespace, id, version, draft, data, created_at::timestamptz FROM api_definitions WHERE namespace = $1 AND id = $2 AND version = $3")
            .bind(namespace)
            .bind(id)
            .bind(version);

        self.db_pool
            .with("api_definition", "get")
            .fetch_optional_as(query)
            .await
    }

    #[when(golem_service_base::db::sqlite::SqlitePool -> get)]
    async fn get_sqlite(
        &self,
        namespace: &str,
        id: &str,
        version: &str,
    ) -> Result<Option<ApiDefinitionRecord>, RepoError> {
        let query = sqlx::query_as::<_, ApiDefinitionRecord>("SELECT namespace, id, version, draft, data, created_at FROM api_definitions WHERE namespace = $1 AND id = $2 AND version = $3")
            .bind(namespace)
            .bind(id)
            .bind(version);

        self.db_pool
            .with_ro("api_definition", "get")
            .fetch_optional_as(query)
            .await
    }

    async fn get_draft(
        &self,
        namespace: &str,
        id: &str,
        version: &str,
    ) -> Result<Option<bool>, RepoError> {
        let query = sqlx::query(
            "SELECT draft FROM api_definitions WHERE namespace = $1 AND id = $2 AND version = $3",
        )
        .bind(namespace)
        .bind(id)
        .bind(version);

        let result = self
            .db_pool
            .with_ro("api_definition", "get_draft")
            .fetch_optional(query)
            .await?;

        let draft: Option<bool> = result.map(|r| r.get("draft"));
        Ok(draft)
    }

    async fn delete(&self, namespace: &str, id: &str, version: &str) -> Result<bool, RepoError> {
        let query = sqlx::query(
            "DELETE FROM api_definitions WHERE namespace = $1 AND id = $2 AND version = $3",
        )
        .bind(namespace)
        .bind(id)
        .bind(version);
        let result = self
            .db_pool
            .with_rw("api_definition", "delete")
            .execute(query)
            .await?;

        Ok(result.rows_affected() > 0)
    }

    #[when(golem_service_base::db::postgres::PostgresPool -> get_all)]
    async fn get_all_postgres(
        &self,
        namespace: &str,
    ) -> Result<Vec<ApiDefinitionRecord>, RepoError> {
        let query = sqlx::query_as::<_, ApiDefinitionRecord>(
            "SELECT namespace, id, version, draft, data, created_at::timestamptz FROM api_definitions WHERE namespace = $1 ORDER BY namespace, id, version",
        )
        .bind(namespace);

        self.db_pool
            .with("api_definition", "get_all")
            .fetch_all(query)
            .await
    }

    #[when(golem_service_base::db::sqlite::SqlitePool -> get_all)]
    async fn get_all_sqlite(&self, namespace: &str) -> Result<Vec<ApiDefinitionRecord>, RepoError> {
        let query = sqlx::query_as::<_, ApiDefinitionRecord>(
            "SELECT namespace, id, version, draft, data, created_at FROM api_definitions WHERE namespace = $1 ORDER BY namespace, id, version",
        )
            .bind(namespace);

        self.db_pool
            .with_ro("api_definition", "get_all")
            .fetch_all(query)
            .await
    }

    #[when(golem_service_base::db::postgres::PostgresPool -> get_all_versions)]
    async fn get_all_versions_postgres(
        &self,
        namespace: &str,
        id: &str,
    ) -> Result<Vec<ApiDefinitionRecord>, RepoError> {
        let query = sqlx::query_as::<_, ApiDefinitionRecord>("SELECT namespace, id, version, draft, data, created_at::timestamptz FROM api_definitions WHERE namespace = $1 AND id = $2 ORDER BY version")
            .bind(namespace)
            .bind(id);

        self.db_pool
            .with("api_definition", "get_all_versions")
            .fetch_all(query)
            .await
    }

    #[when(golem_service_base::db::sqlite::SqlitePool -> get_all_versions)]
    async fn get_all_versions_sqlite(
        &self,
        namespace: &str,
        id: &str,
    ) -> Result<Vec<ApiDefinitionRecord>, RepoError> {
        let query = sqlx::query_as::<_, ApiDefinitionRecord>("SELECT namespace, id, version, draft, data, created_at FROM api_definitions WHERE namespace = $1 AND id = $2 ORDER BY version")
            .bind(namespace)
            .bind(id);

        self.db_pool
            .with_ro("api_definition", "get_all_versions")
            .fetch_all(query)
            .await
    }
}

pub mod record_data_serde {
    use crate::gateway_api_definition::http::CompiledRoute;
    use bytes::{BufMut, Bytes, BytesMut};
    use golem_api_grpc::proto::golem::apidefinition::{
        CompiledHttpApiDefinition as ProtoCompiledHttpApiDefinition,
        CompiledHttpRoute as ProtoCompiledRoute,
    };
    use prost::Message;

    pub const SERIALIZATION_VERSION_V1: u8 = 1u8;

    pub fn serialize(value: &[CompiledRoute]) -> Result<Bytes, String> {
        let routes: Vec<ProtoCompiledRoute> = value
            .iter()
            .cloned()
            .map(ProtoCompiledRoute::try_from)
            .collect::<Result<Vec<ProtoCompiledRoute>, String>>()?;

        let proto_value: ProtoCompiledHttpApiDefinition = ProtoCompiledHttpApiDefinition { routes };

        let mut bytes = BytesMut::new();
        bytes.put_u8(SERIALIZATION_VERSION_V1);
        bytes.extend_from_slice(&proto_value.encode_to_vec());
        Ok(bytes.freeze())
    }

    pub fn deserialize(bytes: &[u8]) -> Result<Vec<CompiledRoute>, String> {
        let (version, data) = bytes.split_at(1);

        match version[0] {
            SERIALIZATION_VERSION_V1 => {
                let proto_value: ProtoCompiledHttpApiDefinition = Message::decode(data)
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
