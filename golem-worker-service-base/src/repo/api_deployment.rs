use crate::api_definition::ApiSite;
use crate::repo::api_definition::ApiDefinitionRecord;
use crate::repo::RepoError;
use crate::service::api_definition::ApiDefinitionIdWithVersion;
use async_trait::async_trait;
use sqlx::{Database, Pool};
use std::fmt::Display;
use std::ops::Deref;
use std::sync::Arc;

#[derive(sqlx::FromRow, Debug, Clone)]
pub struct ApiDeploymentRecord {
    pub namespace: String,
    pub site: String,
    pub host: String,
    pub subdomain: Option<String>,
    pub definition_id: String,
    pub definition_version: String,
}

impl ApiDeploymentRecord {
    pub fn new<Namespace: Display>(
        namespace: Namespace,
        site: ApiSite,
        definition_id: ApiDefinitionIdWithVersion,
    ) -> Self {
        Self {
            namespace: namespace.to_string(),
            site: site.clone().to_string(),
            host: site.host.clone(),
            subdomain: site.subdomain.clone(),
            definition_id: definition_id.id.0,
            definition_version: definition_id.version.0,
        }
    }
}

#[async_trait]
pub trait ApiDeploymentRepo {
    async fn create(&self, deployments: Vec<ApiDeploymentRecord>) -> Result<(), RepoError>;

    async fn delete(&self, deployments: Vec<ApiDeploymentRecord>) -> Result<bool, RepoError>;

    async fn get_by_id(
        &self,
        namespace: &str,
        definition_id: &str,
    ) -> Result<Vec<ApiDeploymentRecord>, RepoError>;

    async fn get_by_site(&self, site: &str) -> Result<Vec<ApiDeploymentRecord>, RepoError>;

    async fn get_definitions_by_site(
        &self,
        site: &str,
    ) -> Result<Vec<ApiDefinitionRecord>, RepoError>;
}

pub struct DbApiDeploymentRepo<DB: Database> {
    db_pool: Arc<Pool<DB>>,
}

impl<DB: Database> DbApiDeploymentRepo<DB> {
    pub fn new(db_pool: Arc<Pool<DB>>) -> Self {
        Self { db_pool }
    }
}

#[async_trait]
impl ApiDeploymentRepo for DbApiDeploymentRepo<sqlx::Sqlite> {
    async fn create(&self, deployments: Vec<ApiDeploymentRecord>) -> Result<(), RepoError> {
        if !deployments.is_empty() {
            let mut transaction = self.db_pool.begin().await?;
            for deployment in deployments {
                sqlx::query(
                    r#"
                      INSERT INTO api_deployments
                        (namespace, site, host, subdomain, definition_id, definition_version)
                      VALUES
                        ($1, $2, $3, $4, $5, $6)
                       "#,
                )
                .bind(deployment.namespace.clone())
                .bind(deployment.site.clone())
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
                    "DELETE FROM api_deployments WHERE namespace = $1 AND site = $2 AND definition_id = $3 AND definition_version = $4",
                )
                    .bind(deployment.namespace.clone())
                    .bind(deployment.site.clone())
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

    async fn get_by_id(
        &self,
        namespace: &str,
        definition_id: &str,
    ) -> Result<Vec<ApiDeploymentRecord>, RepoError> {
        sqlx::query_as::<_, ApiDeploymentRecord>("SELECT namespace, site, host, subdomain, definition_id, definition_version FROM api_deployments WHERE namespace = $1 AND definition_id = $2")
            .bind(namespace)
            .bind(definition_id)
            .fetch_all(self.db_pool.deref())
            .await
            .map_err(|e| e.into())
    }

    async fn get_by_site(&self, site: &str) -> Result<Vec<ApiDeploymentRecord>, RepoError> {
        sqlx::query_as::<_, ApiDeploymentRecord>(
            r#"
                SELECT namespace, site, host, subdomain, definition_id, definition_version
                FROM api_deployments
                WHERE site = $1
                "#,
        )
        .bind(site)
        .fetch_all(self.db_pool.deref())
        .await
        .map_err(|e| e.into())
    }

    async fn get_definitions_by_site(
        &self,
        site: &str,
    ) -> Result<Vec<ApiDefinitionRecord>, RepoError> {
        sqlx::query_as::<_, ApiDefinitionRecord>(
            r#"
                SELECT api_definitions.namespace, api_definitions.id, api_definitions.version, api_definitions.draft, api_definitions.data
                FROM api_deployments
                  JOIN api_definitions ON api_deployments.namespace = api_definitions.namespace AND api_deployments.definition_id = api_definitions.id AND api_deployments.definition_version = api_definitions.version
                WHERE
                 api_deployments.site = $1
                "#
        )
            .bind(site)
            .fetch_all(self.db_pool.deref())
            .await
            .map_err(|e| e.into())
    }
}

#[async_trait]
impl ApiDeploymentRepo for DbApiDeploymentRepo<sqlx::Postgres> {
    async fn create(&self, deployments: Vec<ApiDeploymentRecord>) -> Result<(), RepoError> {
        if !deployments.is_empty() {
            let mut transaction = self.db_pool.begin().await?;
            for deployment in deployments {
                sqlx::query(
                    r#"
                      INSERT INTO api_deployments
                        (namespace, site, host, subdomain, definition_id, definition_version)
                      VALUES
                        ($1, $2, $3, $4, $5, $6)
                       "#,
                )
                .bind(deployment.namespace.clone())
                .bind(deployment.site.clone())
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
                    "DELETE FROM api_deployments WHERE namespace = $1 AND site = $2 AND definition_id = $3 AND definition_version = $4",
                )
                    .bind(deployment.namespace.clone())
                    .bind(deployment.site.clone())
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

    async fn get_by_id(
        &self,
        namespace: &str,
        definition_id: &str,
    ) -> Result<Vec<ApiDeploymentRecord>, RepoError> {
        sqlx::query_as::<_, ApiDeploymentRecord>("SELECT namespace, site, host, subdomain, definition_id, definition_version FROM api_deployments WHERE namespace = $1 AND definition_id = $2")
            .bind(namespace)
            .bind(definition_id)
            .fetch_all(self.db_pool.deref())
            .await
            .map_err(|e| e.into())
    }

    async fn get_by_site(&self, site: &str) -> Result<Vec<ApiDeploymentRecord>, RepoError> {
        sqlx::query_as::<_, ApiDeploymentRecord>(
            r#"
                SELECT namespace, site, host, subdomain, definition_id, definition_version
                FROM api_deployments
                WHERE
                 site = $1
                "#,
        )
        .bind(site)
        .fetch_all(self.db_pool.deref())
        .await
        .map_err(|e| e.into())
    }

    async fn get_definitions_by_site(
        &self,
        site: &str,
    ) -> Result<Vec<ApiDefinitionRecord>, RepoError> {
        sqlx::query_as::<_, ApiDefinitionRecord>(
            r#"
                SELECT api_definitions.namespace, api_definitions.id, api_definitions.version, api_definitions.draft, api_definitions.data AS data
                FROM api_deployments
                  JOIN api_definitions ON api_deployments.namespace = api_definitions.namespace AND api_deployments.definition_id = api_definitions.id AND api_deployments.definition_version = api_definitions.version
                WHERE
                 api_deployments.site = $1
                "#
        )
            .bind(site)
            .fetch_all(self.db_pool.deref())
            .await
            .map_err(|e| e.into())
    }
}
