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
    async fn create(&self, deployments: Vec<ApiDeploymentRecord>) -> Result<(), RepoError>;

    async fn delete(&self, deployments: Vec<ApiDeploymentRecord>) -> Result<bool, RepoError>;

    async fn get(
        &self,
        namespace: &str,
        host: &str,
        subdomain: Option<&str>,
    ) -> Result<Vec<ApiDeploymentRecord>, RepoError>;

    async fn get_by_site(&self, site: &str) -> Result<Vec<ApiDeploymentRecord>, RepoError>;

    async fn get_by_id(
        &self,
        namespace: &str,
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
    async fn create(&self, deployments: Vec<ApiDeploymentRecord>) -> Result<(), RepoError> {
        if !deployments.is_empty() {
            let mut transaction = self.db_pool.begin().await?;
            for deployment in deployments {
                sqlx::query(
                    r#"
                      INSERT INTO api_deployments
                        (namespace, host, subdomain, definition_id, definition_version)
                      VALUES
                        ($1, $2, $3, $4, $5)
                       "#,
                )
                .bind(deployment.namespace.clone())
                .bind(deployment.host.clone())
                .bind(deployment.subdomain.clone())
                .bind(deployment.definition_id.clone())
                .bind(deployment.definition_version.clone())
                .execute(&mut *transaction)
                .await?;
            }
            transaction.commit().await?;
        }
        Ok(())
    }

    async fn delete(&self, deployments: Vec<ApiDeploymentRecord>) -> Result<bool, RepoError> {
        if !deployments.is_empty() {
            let mut transaction = self.db_pool.begin().await?;
            for deployment in deployments {
                sqlx::query(
                    "DELETE FROM api_deployments WHERE namespace = $1 AND host = $2 AND subdomain = $3 AND definition_id = $4 AND definition_version = $5",
                )
                    .bind(deployment.namespace.clone())
                    .bind(deployment.host.clone())
                    .bind(deployment.subdomain.clone())
                    .bind(deployment.definition_id.clone())
                    .bind(deployment.definition_version.clone())
                    .execute(&mut *transaction)
                    .await?;
            }
            transaction.commit().await?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    async fn get(
        &self,
        namespace: &str,
        host: &str,
        subdomain: Option<&str>,
    ) -> Result<Vec<ApiDeploymentRecord>, RepoError> {
        sqlx::query_as::<_, ApiDeploymentRecord>("SELECT namespace, host, subdomain, definition_id, definition_version FROM api_deployments WHERE namespace = $1 AND host = $2 AND subdomain = $3")
            .bind(namespace)
            .bind(host)
            .bind(subdomain)
            .fetch_all(self.db_pool.deref())
            .await
            .map_err(|e| e.into())
    }

    async fn get_by_id(
        &self,
        namespace: &str,
        definition_id: &str,
    ) -> Result<Vec<ApiDeploymentRecord>, RepoError> {
        sqlx::query_as::<_, ApiDeploymentRecord>("SELECT namespace, host, subdomain, definition_id, definition_version FROM api_deployments WHERE namespace = $1 AND definition_id = $2")
            .bind(namespace)
            .bind(definition_id)
            .fetch_all(self.db_pool.deref())
            .await
            .map_err(|e| e.into())
    }

    async fn get_by_site(&self, site: &str) -> Result<Vec<ApiDeploymentRecord>, RepoError> {
        sqlx::query_as::<_, ApiDeploymentRecord>(
            r#"
                SELECT namespace, host, subdomain, definition_id, definition_version
                FROM api_deployments
                WHERE
                 (submdomain IS NULL AND host = $1) OR (subdomain IS NOT NULL AND CONCAT(subdomain, '.', host) = $1)
                "#
        )
            .bind(site)
            .fetch_all(self.db_pool.deref())
            .await
            .map_err(|e| e.into())
    }
}

#[async_trait]
impl ApiDeploymentRepo for DbApiDeploymentRepoRepo<sqlx::Postgres> {
    async fn create(&self, deployments: Vec<ApiDeploymentRecord>) -> Result<(), RepoError> {
        if !deployments.is_empty() {
            let mut transaction = self.db_pool.begin().await?;
            for deployment in deployments {
                sqlx::query(
                    r#"
                      INSERT INTO api_deployments
                        (namespace, host, subdomain, definition_id, definition_version)
                      VALUES
                        ($1, $2, $3, $4, $5)
                       "#,
                )
                .bind(deployment.namespace.clone())
                .bind(deployment.host.clone())
                .bind(deployment.subdomain.clone())
                .bind(deployment.definition_id.clone())
                .bind(deployment.definition_version.clone())
                .execute(&mut *transaction)
                .await?;
            }
            transaction.commit().await?;
        }
        Ok(())
    }

    async fn get(
        &self,
        namespace: &str,
        host: &str,
        subdomain: Option<&str>,
    ) -> Result<Vec<ApiDeploymentRecord>, RepoError> {
        sqlx::query_as::<_, ApiDeploymentRecord>("SELECT namespace, host, subdomain, definition_id, definition_version FROM api_deployments WHERE namespace = $1 AND host = $2 AND subdomain = $3")
            .bind(namespace)
            .bind(host)
            .bind(subdomain)
            .fetch_all(self.db_pool.deref())
            .await
            .map_err(|e| e.into())
    }

    async fn delete(&self, deployments: Vec<ApiDeploymentRecord>) -> Result<bool, RepoError> {
        if !deployments.is_empty() {
            let mut transaction = self.db_pool.begin().await?;
            for deployment in deployments {
                sqlx::query(
                    "DELETE FROM api_deployments WHERE namespace = $1 AND host = $2 AND subdomain = $3 AND definition_id = $4 AND definition_version = $5",
                )
                    .bind(deployment.namespace.clone())
                    .bind(deployment.host.clone())
                    .bind(deployment.subdomain.clone())
                    .bind(deployment.definition_id.clone())
                    .bind(deployment.definition_version.clone())
                    .execute(&mut *transaction)
                    .await?;
            }
            transaction.commit().await?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    async fn get_by_site(&self, site: &str) -> Result<Vec<ApiDeploymentRecord>, RepoError> {
        sqlx::query_as::<_, ApiDeploymentRecord>(
            r#"
                SELECT namespace, host, subdomain, definition_id, definition_version
                FROM api_deployments
                WHERE
                 (submdomain IS NULL AND host = $1) OR (subdomain IS NOT NULL AND CONCAT(subdomain, '.', host) = $1)
                "#
        )
            .bind(site)
            .fetch_all(self.db_pool.deref())
            .await
            .map_err(|e| e.into())
    }

    async fn get_by_id(
        &self,
        namespace: &str,
        definition_id: &str,
    ) -> Result<Vec<ApiDeploymentRecord>, RepoError> {
        sqlx::query_as::<_, ApiDeploymentRecord>("SELECT namespace, host, subdomain, definition_id, definition_version FROM api_deployments WHERE namespace = $1 AND definition_id = $2")
            .bind(namespace)
            .bind(definition_id)
            .fetch_all(self.db_pool.deref())
            .await
            .map_err(|e| e.into())
    }
}
