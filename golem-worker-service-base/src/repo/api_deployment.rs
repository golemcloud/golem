use crate::repo::api_deployment_repo::ApiDeploymentRepoError;
use crate::repo::RepoError;
use async_trait::async_trait;
use sqlx::{Database, Pool};
use std::ops::Deref;
use std::sync::Arc;

#[derive(sqlx::FromRow, Debug, Clone)]
pub struct ApiDeploymentRecord {
    pub namespace: String,
    pub host: String,
    pub subdomain: Option<String>,
    pub definition_id: String,
    pub definition_version: String,
}

#[async_trait]
pub trait ApiDeploymentRepo {
    async fn create(&self, deployment: &ApiDeploymentRecord) -> Result<(), ApiDeploymentRepoError>;

    async fn get(
        &self,
        namespace: &str,
        host: &str,
        subdomain: &str,
    ) -> Result<Vec<ApiDeploymentRecord>, RepoError>;

    async fn delete(&self, namespace: &str, host: &str, subdomain: &str)
        -> Result<bool, RepoError>;

    async fn get_by_id(
        &self,
        namespace: &str,
        host: &str,
        subdomain: &str,
        definition_id: &str,
    ) -> Result<Vec<ApiDeploymentRecord>, RepoError>;
}

pub struct DbApiDeploymentRepoRepo<DB: Database> {
    db_pool: Arc<Pool<DB>>,
}

impl<DB: Database> DbApiDeploymentRepoRepo<DB> {
    pub fn new(db_pool: Arc<Pool<DB>>) -> Self {
        Self { db_pool }
    }
}

#[async_trait]
impl ApiDeploymentRepo for DbApiDeploymentRepoRepo<sqlx::Sqlite> {
    async fn create(&self, definition: &ApiDeploymentRecord) -> Result<(), RepoError> {
        sqlx::query(
            r#"
              INSERT INTO api_deployments
                (namespace, host, subdomain, definition_id, definition_version)
              VALUES
                ($1, $2, $3, $4, $5)
               "#,
        )
        .bind(definition.namespace.clone())
        .bind(definition.host.clone())
        .bind(definition.subdomain.clone())
        .bind(definition.definition_id.clone())
        .bind(definition.definition_version.clone())
        .execute(self.db_pool.deref())
        .await?;

        Ok(())
    }

    async fn get(
        &self,
        namespace: &str,
        host: &str,
        subdomain: &str,
    ) -> Result<Vec<ApiDeploymentRecord>, RepoError> {
        sqlx::query_as::<_, ApiDeploymentRecord>("SELECT namespace, host, subdomain, definition_id, definition_version FROM api_deployments WHERE namespace = $1 AND host = $2 AND subdomain = $3")
            .bind(namespace)
            .bind(host)
            .bind(subdomain)
            .fetch_all(self.db_pool.deref())
            .await
            .map_err(|e| e.into())
    }

    async fn delete(
        &self,
        namespace: &str,
        host: &str,
        subdomain: &str,
    ) -> Result<bool, RepoError> {
        sqlx::query(
            "DELETE FROM api_deployments WHERE namespace = $1 AND host = $2 AND subdomain = $3",
        )
        .bind(namespace)
        .bind(host)
        .bind(subdomain)
        .execute(self.db_pool.deref())
        .await?;
        Ok(true)
    }

    async fn get_by_id(
        &self,
        namespace: &str,
        host: &str,
        subdomain: &str,
        definition_id: &str,
    ) -> Result<Vec<ApiDeploymentRecord>, RepoError> {
        sqlx::query_as::<_, ApiDeploymentRecord>("SELECT namespace, host, subdomain, definition_id, definition_version FROM api_deployments WHERE namespace = $1 AND host = $2 AND subdomain = $3, definition_id = $4")
            .bind(namespace)
            .bind(host)
            .bind(subdomain)
            .bind(definition_id)
            .fetch_all(self.db_pool.deref())
            .await
            .map_err(|e| e.into())
    }
}
