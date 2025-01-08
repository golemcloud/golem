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

use crate::gateway_api_deployment::ApiSite;
use crate::repo::api_definition::ApiDefinitionRecord;
use crate::service::gateway::api_definition::ApiDefinitionIdWithVersion;
use async_trait::async_trait;
use conditional_trait_gen::{trait_gen, when};
use golem_service_base::repo::RepoError;
use sqlx::{Database, Pool};
use std::fmt::Display;
use std::ops::Deref;
use std::sync::Arc;
use tracing::{debug, error};

#[derive(sqlx::FromRow, Debug, Clone)]
pub struct ApiDeploymentRecord {
    pub namespace: String,
    pub site: String,
    pub host: String,
    pub subdomain: Option<String>,
    pub definition_id: String,
    pub definition_version: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

impl ApiDeploymentRecord {
    pub fn new<Namespace: Display>(
        namespace: Namespace,
        site: ApiSite,
        definition_id: ApiDefinitionIdWithVersion,
        created_at: chrono::DateTime<chrono::Utc>,
    ) -> Self {
        Self {
            namespace: namespace.to_string(),
            site: site.clone().to_string(),
            host: site.host.clone(),
            subdomain: site.subdomain.clone(),
            definition_id: definition_id.id.0,
            definition_version: definition_id.version.0,
            created_at,
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

    async fn get_by_id_and_version(
        &self,
        namespace: &str,
        definition_id: &str,
        definition_version: &str,
    ) -> Result<Vec<ApiDeploymentRecord>, RepoError>;

    async fn get_by_site(&self, site: &str) -> Result<Vec<ApiDeploymentRecord>, RepoError>;

    async fn get_definitions_by_site(
        &self,
        site: &str,
    ) -> Result<Vec<ApiDefinitionRecord>, RepoError>;
}

pub struct LoggedDeploymentRepo<Repo: ApiDeploymentRepo> {
    repo: Repo,
}

impl<Repo: ApiDeploymentRepo> LoggedDeploymentRepo<Repo> {
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
impl<Repo: ApiDeploymentRepo + Sync> ApiDeploymentRepo for LoggedDeploymentRepo<Repo> {
    async fn create(&self, deployments: Vec<ApiDeploymentRecord>) -> Result<(), RepoError> {
        let result = self.repo.create(deployments).await;
        Self::logged("create", result)
    }

    async fn delete(&self, deployments: Vec<ApiDeploymentRecord>) -> Result<bool, RepoError> {
        let result = self.repo.delete(deployments).await;
        Self::logged("delete", result)
    }

    async fn get_by_id(
        &self,
        namespace: &str,
        definition_id: &str,
    ) -> Result<Vec<ApiDeploymentRecord>, RepoError> {
        let result = self.repo.get_by_id(namespace, definition_id).await;
        Self::logged_with_id("get_by_id", namespace, definition_id, result)
    }

    async fn get_by_id_and_version(
        &self,
        namespace: &str,
        definition_id: &str,
        definition_version: &str,
    ) -> Result<Vec<ApiDeploymentRecord>, RepoError> {
        let result = self
            .repo
            .get_by_id_and_version(namespace, definition_id, definition_version)
            .await;
        Self::logged_with_id("get_by_id_and_version", namespace, definition_id, result)
    }

    async fn get_by_site(&self, site: &str) -> Result<Vec<ApiDeploymentRecord>, RepoError> {
        let result = self.repo.get_by_site(site).await;
        Self::logged("get_by_site", result)
    }

    async fn get_definitions_by_site(
        &self,
        site: &str,
    ) -> Result<Vec<ApiDefinitionRecord>, RepoError> {
        let result = self.repo.get_definitions_by_site(site).await;
        Self::logged("get_definitions_by_site", result)
    }
}

pub struct DbApiDeploymentRepo<DB: Database> {
    db_pool: Arc<Pool<DB>>,
}

impl<DB: Database> DbApiDeploymentRepo<DB> {
    pub fn new(db_pool: Arc<Pool<DB>>) -> Self {
        Self { db_pool }
    }
}

#[trait_gen(sqlx::Postgres -> sqlx::Postgres, sqlx::Sqlite)]
#[async_trait]
impl ApiDeploymentRepo for DbApiDeploymentRepo<sqlx::Postgres> {
    async fn create(&self, deployments: Vec<ApiDeploymentRecord>) -> Result<(), RepoError> {
        if !deployments.is_empty() {
            let mut transaction = self.db_pool.begin().await?;
            for deployment in deployments {
                sqlx::query(
                    r#"
                      INSERT INTO api_deployments
                        (namespace, site, host, subdomain, definition_id, definition_version, created_at)
                      VALUES
                        ($1, $2, $3, $4, $5, $6, $7)
                       "#,
                )
                .bind(deployment.namespace.clone())
                .bind(deployment.site.clone())
                .bind(deployment.host.clone())
                .bind(deployment.subdomain.clone())
                .bind(deployment.definition_id.clone())
                .bind(deployment.definition_version.clone())
                .bind(deployment.created_at)
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

    #[when(sqlx::Postgres -> get_by_id)]
    async fn get_by_id_postgres(
        &self,
        namespace: &str,
        definition_id: &str,
    ) -> Result<Vec<ApiDeploymentRecord>, RepoError> {
        sqlx::query_as::<_, ApiDeploymentRecord>(
            r#"
                SELECT namespace, site, host, subdomain, definition_id, definition_version, created_at::timestamptz
                FROM api_deployments
                WHERE namespace = $1 AND definition_id = $2
                "#,
        )
        .bind(namespace)
        .bind(definition_id)
        .fetch_all(self.db_pool.deref())
        .await
        .map_err(|e| e.into())
    }

    #[when(sqlx::Sqlite -> get_by_id)]
    async fn get_by_id_sqlite(
        &self,
        namespace: &str,
        definition_id: &str,
    ) -> Result<Vec<ApiDeploymentRecord>, RepoError> {
        sqlx::query_as::<_, ApiDeploymentRecord>(
            r#"
                SELECT namespace, site, host, subdomain, definition_id, definition_version, created_at
                FROM api_deployments
                WHERE namespace = $1 AND definition_id = $2
                "#,
        )
            .bind(namespace)
            .bind(definition_id)
            .fetch_all(self.db_pool.deref())
            .await
            .map_err(|e| e.into())
    }

    #[when(sqlx::Postgres -> get_by_id_and_version)]
    async fn get_by_id_and_version_postgres(
        &self,
        namespace: &str,
        definition_id: &str,
        definition_version: &str,
    ) -> Result<Vec<ApiDeploymentRecord>, RepoError> {
        sqlx::query_as::<_, ApiDeploymentRecord>(
            r#"
                SELECT namespace, site, host, subdomain, definition_id, definition_version, created_at::timestamptz
                FROM api_deployments
                WHERE namespace = $1 AND definition_id = $2 AND definition_version = $3
                "#,
        )
        .bind(namespace)
        .bind(definition_id)
        .bind(definition_version)
        .fetch_all(self.db_pool.deref())
        .await
        .map_err(|e| e.into())
    }

    #[when(sqlx::Sqlite -> get_by_id_and_version)]
    async fn get_by_id_and_version_sqlite(
        &self,
        namespace: &str,
        definition_id: &str,
        definition_version: &str,
    ) -> Result<Vec<ApiDeploymentRecord>, RepoError> {
        sqlx::query_as::<_, ApiDeploymentRecord>(
            r#"
                SELECT namespace, site, host, subdomain, definition_id, definition_version, created_at
                FROM api_deployments
                WHERE namespace = $1 AND definition_id = $2 AND definition_version = $3
                "#,
        )
            .bind(namespace)
            .bind(definition_id)
            .bind(definition_version)
            .fetch_all(self.db_pool.deref())
            .await
            .map_err(|e| e.into())
    }

    #[when(sqlx::Postgres -> get_by_site)]
    async fn get_by_site_postgres(
        &self,
        site: &str,
    ) -> Result<Vec<ApiDeploymentRecord>, RepoError> {
        sqlx::query_as::<_, ApiDeploymentRecord>(
            r#"
                SELECT namespace, site, host, subdomain, definition_id, definition_version, created_at::timestamptz
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

    #[when(sqlx::Sqlite -> get_by_site)]
    async fn get_by_site_sqlite(&self, site: &str) -> Result<Vec<ApiDeploymentRecord>, RepoError> {
        sqlx::query_as::<_, ApiDeploymentRecord>(
            r#"
                SELECT namespace, site, host, subdomain, definition_id, definition_version, created_at
                FROM api_deployments
                WHERE site = $1
                "#,
        )
            .bind(site)
            .fetch_all(self.db_pool.deref())
            .await
            .map_err(|e| e.into())
    }

    #[when(sqlx::Postgres -> get_definitions_by_site)]
    async fn get_definitions_by_site_postgres(
        &self,
        site: &str,
    ) -> Result<Vec<ApiDefinitionRecord>, RepoError> {
        sqlx::query_as::<_, ApiDefinitionRecord>(
            r#"
                SELECT api_definitions.namespace, api_definitions.id, api_definitions.version, api_definitions.draft, api_definitions.data AS data, api_definitions.created_at::timestamptz
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

    #[when(sqlx::Sqlite -> get_definitions_by_site)]
    async fn get_definitions_by_site_sqlite(
        &self,
        site: &str,
    ) -> Result<Vec<ApiDefinitionRecord>, RepoError> {
        sqlx::query_as::<_, ApiDefinitionRecord>(
            r#"
                SELECT api_definitions.namespace, api_definitions.id, api_definitions.version, api_definitions.draft, api_definitions.data, api_definitions.created_at
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
