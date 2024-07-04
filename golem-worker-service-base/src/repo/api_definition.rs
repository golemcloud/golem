use crate::api_definition::http::HttpApiDefinition;
use crate::repo::RepoError;
use async_trait::async_trait;
use sqlx::{Database, Pool, Row};
use std::collections::HashMap;
use std::fmt::Display;
use std::ops::Deref;
use std::sync::{Arc, Mutex};

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
        sqlx::query(
            "DELETE FROM api_definitions WHERE namespace = $1 AND id = $2 AND version = $3",
        )
        .bind(namespace)
        .bind(id)
        .bind(version)
        .execute(self.db_pool.deref())
        .await?;
        Ok(true)
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
        sqlx::query(
            "DELETE FROM api_definitions WHERE namespace = $1 AND id = $2 AND version = $3",
        )
        .bind(namespace)
        .bind(id)
        .bind(version)
        .execute(self.db_pool.deref())
        .await?;
        Ok(true)
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

pub struct InMemoryApiDefinitionRepo {
    registry: Mutex<HashMap<(String, String, String), ApiDefinitionRecord>>,
}

impl Default for InMemoryApiDefinitionRepo {
    fn default() -> Self {
        Self {
            registry: Mutex::new(HashMap::new()),
        }
    }
}

#[async_trait]
impl ApiDefinitionRepo for InMemoryApiDefinitionRepo {
    async fn create(&self, definition: &ApiDefinitionRecord) -> Result<(), RepoError> {
        let key = (
            definition.namespace.clone(),
            definition.id.clone(),
            definition.version.clone(),
        );
        let mut registry = self.registry.lock().unwrap();
        if let std::collections::hash_map::Entry::Vacant(e) = registry.entry(key.clone()) {
            e.insert(definition.clone());
            Ok(())
        } else {
            Err(RepoError::Internal(
                "ApiDefinition already exists".to_string(),
            ))
        }
    }

    async fn update(&self, definition: &ApiDefinitionRecord) -> Result<(), RepoError> {
        let key = (
            definition.namespace.clone(),
            definition.id.clone(),
            definition.version.clone(),
        );
        let mut registry = self.registry.lock().unwrap();
        registry.insert(key.clone(), definition.clone());
        Ok(())
    }

    async fn set_not_draft(
        &self,
        namespace: &str,
        id: &str,
        version: &str,
    ) -> Result<(), RepoError> {
        match self.get(namespace, id, version).await? {
            Some(v) if v.draft => {
                let mut registry = self.registry.lock().unwrap();
                let key = (namespace.to_string(), id.to_string(), version.to_string());
                registry.entry(key.clone()).and_modify(|v| v.draft = false);
                Ok(())
            }
            _ => Ok(()),
        }
    }

    async fn get(
        &self,
        namespace: &str,
        id: &str,
        version: &str,
    ) -> Result<Option<ApiDefinitionRecord>, RepoError> {
        let key = (namespace.to_string(), id.to_string(), version.to_string());
        let registry = self.registry.lock().unwrap();
        Ok(registry.get(&key).cloned())
    }

    async fn get_draft(
        &self,
        namespace: &str,
        id: &str,
        version: &str,
    ) -> Result<Option<bool>, RepoError> {
        let value = self.get(namespace, id, version).await?;
        Ok(value.map(|v| v.draft))
    }

    async fn delete(&self, namespace: &str, id: &str, version: &str) -> Result<bool, RepoError> {
        let key = (namespace.to_string(), id.to_string(), version.to_string());
        let mut registry = self.registry.lock().unwrap();
        let result = registry.remove(&key);
        Ok(result.is_some())
    }

    async fn get_all(&self, namespace: &str) -> Result<Vec<ApiDefinitionRecord>, RepoError> {
        let registry = self.registry.lock().unwrap();
        let result: Vec<ApiDefinitionRecord> = registry
            .iter()
            .filter(|(k, _)| k.0 == *namespace)
            .map(|(_, v)| v.clone())
            .collect();
        Ok(result)
    }

    async fn get_all_versions(
        &self,
        namespace: &str,
        id: &str,
    ) -> Result<Vec<ApiDefinitionRecord>, RepoError> {
        let registry = self.registry.lock().unwrap();
        let result = registry
            .iter()
            .filter(|(k, _)| k.0 == *namespace && k.1 == *id)
            .map(|(_, v)| v.clone())
            .collect();
        Ok(result)
    }
}

pub mod record_data_serde {
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
