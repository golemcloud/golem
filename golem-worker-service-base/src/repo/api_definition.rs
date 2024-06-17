use std::fmt::Display;
use std::ops::Deref;
use std::sync::Arc;
use async_trait::async_trait;
use sqlx::{Database, Pool};
use crate::api_definition::ApiDefinitionId;
use crate::api_definition::http::HttpApiDefinition;
use crate::repo::api_definition_repo::ApiRegistrationRepoError;
use crate::repo::api_namespace::ApiNamespace;
use crate::service::api_definition::ApiDefinitionKey;

#[derive(sqlx::FromRow, Debug, Clone)]
pub struct ApiDefinitionRecord {
    pub namespace: String,
    pub id: String,
    pub version: String,
    pub draft: bool,
    pub data: String,
}

impl ApiDefinitionRecord {
    pub fn new<Namespace: Display>(namespace: Namespace, definition: HttpApiDefinition) -> Self {
        Self {
            namespace: namespace.to_string(),
            id: definition.id.0,
            version: definition.version.0,
            draft: definition.draft,
            data: serde_json::to_string(&definition.routes).unwrap(),
        }
    }
}

impl From<ApiDefinitionRecord> for HttpApiDefinition {
    fn from(value: ApiDefinitionRecord) -> Self {
        Self {
            id: value.id.into(),
            version: value.version.into(),
            routes: serde_json::from_str(&value.data).unwrap(),
            draft: value.draft,
        }
    }
}


#[async_trait]
pub trait ApiDefinitionRepo {
    async fn create(
        &self,
        definition: &ApiDefinitionRecord,
    ) -> Result<(), ApiRegistrationRepoError>;

    async fn update(
        &self,
        definition: &ApiDefinitionRecord,
    ) -> Result<(), ApiRegistrationRepoError>;

    async fn set_not_draft(
        &self,
        namespace: &str,
        id: &str,
        version: &str,
    ) -> Result<(), ApiRegistrationRepoError>;

    async fn get(
        &self,
        namespace: &str,
        id: &str,
        version: &str,
    ) -> Result<Option<ApiDefinitionRecord>, ApiRegistrationRepoError>;

    async fn delete(
        &self,
        namespace: &str,
        id: &str,
        version: &str,
    ) -> Result<bool, ApiRegistrationRepoError>;

    async fn get_all(
        &self,
        namespace: &str,
    ) -> Result<Vec<ApiDefinitionRecord>, ApiRegistrationRepoError>;

    async fn get_all_versions(
        &self,
        namespace: &str,
        id: &str,
    ) -> Result<Vec<ApiDefinitionRecord>, ApiRegistrationRepoError>;
}



pub struct DbApiDefinitionRepoRepo<DB: Database> {
    db_pool: Arc<Pool<DB>>,
}

impl<DB: Database> DbApiDefinitionRepoRepo<DB> {
    pub fn new(db_pool: Arc<Pool<DB>>) -> Self {
        Self { db_pool }
    }
}



#[async_trait]
impl ApiDefinitionRepo for DbApiDefinitionRepoRepo<sqlx::Sqlite> {
    async fn create(&self, definition: &ApiDefinitionRecord) -> Result<(), ApiRegistrationRepoError> {
        sqlx::query(
            r#"
              INSERT INTO api_definitions
                (namespace, id, version, draft, data)
              VALUES
                ($1, $2, $3, $4, $5::jsonb)
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

    async fn update(&self, definition: &ApiDefinitionRecord) -> Result<(), ApiRegistrationRepoError> {
        sqlx::query(
            r#"
              UPATE api_definitions
                (namespace, id, version, draft, data)
              VALUES
                ($1, $2, $3, $4, $5::jsonb)
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

    async fn set_not_draft(&self, namespace: &str, id: &str, version: &str) -> Result<(), ApiRegistrationRepoError> {
        todo!()
    }

    async fn get(&self, namespace: &str, id: &str, version: &str) -> Result<Option<ApiDefinitionRecord>, ApiRegistrationRepoError> {
        sqlx::query_as::<_, ApiDefinitionRecord>("SELECT namespace, id, version, draft, CAST(data AS TEXT) AS data FROM api_definitions WHERE namespace = $1 AND id = $2 AND version = $3")
            .bind(namespace)
            .bind(id)
            .bind(version)
            .fetch_optional(self.db_pool.deref())
            .await
            .map_err(|e| e.into())
    }

    async fn delete(&self, namespace: &str, id: &str, version: &str) -> Result<bool, ApiRegistrationRepoError> {
        sqlx::query("DELETE FROM api_definitions WHERE namespace = $1 AND id = $2 AND version = $3")
            .bind(namespace)
            .bind(id)
            .bind(version)
            .execute(self.db_pool.deref())
            .await?;
        Ok(true)
    }

    async fn get_all(&self, namespace: &str) -> Result<Vec<ApiDefinitionRecord>, ApiRegistrationRepoError> {
        sqlx::query_as::<_, ApiDefinitionRecord>("SELECT namespace, id, version, draft, CAST(data AS TEXT) AS data FROM api_definitions WHERE namespace = $1")
            .bind(namespace)
            .fetch_all(self.db_pool.deref())
            .await
            .map_err(|e| e.into())
    }

    async fn get_all_versions(&self, namespace: &str, id: &str) -> Result<Vec<ApiDefinitionRecord>, ApiRegistrationRepoError> {
        sqlx::query_as::<_, ApiDefinitionRecord>("SELECT namespace, id, version, draft, CAST(data AS TEXT) AS data FROM api_definitions WHERE namespace = $1 AND id = $2")
            .bind(namespace)
            .bind(id)
            .fetch_all(self.db_pool.deref())
            .await
            .map_err(|e| e.into())
    }
}