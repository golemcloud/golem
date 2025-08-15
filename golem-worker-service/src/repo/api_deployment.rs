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

use crate::gateway_api_deployment::ApiSite;
use crate::repo::api_definition::ApiDefinitionRecord;
use crate::service::gateway::api_definition::ApiDefinitionIdWithVersion;
use async_trait::async_trait;
use conditional_trait_gen::{trait_gen, when};
use golem_service_base::db::Pool;
use golem_service_base::repo::RepoError;
use std::fmt::Display;
use tracing::{info_span, Instrument, Span};

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
pub trait ApiDeploymentRepo: Send + Sync {
    async fn create(
        &self,
        namespace: &str,
        deployments: Vec<ApiDeploymentRecord>,
    ) -> Result<(), RepoError>;

    async fn delete(
        &self,
        namespace: &str,
        deployments: Vec<ApiDeploymentRecord>,
    ) -> Result<bool, RepoError>;

    async fn get_all(&self, namespace: &str) -> Result<Vec<ApiDeploymentRecord>, RepoError>;

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

    // A site (subdomain.domain) is always owned by a namespace
    async fn get_by_site(
        &self,
        namespace: &str,
        site: &str,
    ) -> Result<Vec<ApiDeploymentRecord>, RepoError>;

    async fn get_definitions_by_site(
        &self,
        namespace: &str,
        site: &str,
    ) -> Result<Vec<ApiDefinitionRecord>, RepoError>;

    async fn get_all_definitions_by_site(
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

    fn span(namespace: &str, api_definition_id: &str) -> Span {
        info_span!(
            "API deployment repository",
            namespace = namespace,
            api_definition_id = api_definition_id
        )
    }
}

#[async_trait]
impl<Repo: ApiDeploymentRepo + Sync> ApiDeploymentRepo for LoggedDeploymentRepo<Repo> {
    async fn create(
        &self,
        namespace: &str,
        deployments: Vec<ApiDeploymentRecord>,
    ) -> Result<(), RepoError> {
        self.repo.create(namespace, deployments).await
    }

    async fn delete(
        &self,
        namespace: &str,
        deployments: Vec<ApiDeploymentRecord>,
    ) -> Result<bool, RepoError> {
        self.repo.delete(namespace, deployments).await
    }

    async fn get_all(&self, namespace: &str) -> Result<Vec<ApiDeploymentRecord>, RepoError> {
        self.repo
            .get_all(namespace)
            .instrument(Self::span(namespace, "*"))
            .await
    }

    async fn get_by_id(
        &self,
        namespace: &str,
        definition_id: &str,
    ) -> Result<Vec<ApiDeploymentRecord>, RepoError> {
        self.repo
            .get_by_id(namespace, definition_id)
            .instrument(Self::span(namespace, definition_id))
            .await
    }

    async fn get_by_id_and_version(
        &self,
        namespace: &str,
        definition_id: &str,
        definition_version: &str,
    ) -> Result<Vec<ApiDeploymentRecord>, RepoError> {
        self.repo
            .get_by_id_and_version(namespace, definition_id, definition_version)
            .instrument(Self::span(namespace, definition_id))
            .await
    }

    async fn get_by_site(
        &self,
        namespace: &str,
        site: &str,
    ) -> Result<Vec<ApiDeploymentRecord>, RepoError> {
        self.repo.get_by_site(namespace, site).await
    }

    async fn get_definitions_by_site(
        &self,
        namespace: &str,
        site: &str,
    ) -> Result<Vec<ApiDefinitionRecord>, RepoError> {
        self.repo.get_definitions_by_site(namespace, site).await
    }

    async fn get_all_definitions_by_site(
        &self,
        site: &str,
    ) -> Result<Vec<ApiDefinitionRecord>, RepoError> {
        self.repo.get_all_definitions_by_site(site).await
    }
}

pub struct DbApiDeploymentRepo<DB: Pool> {
    db_pool: DB,
}

impl<DB: Pool> DbApiDeploymentRepo<DB> {
    pub fn new(db_pool: DB) -> Self {
        Self { db_pool }
    }
}

#[trait_gen(golem_service_base::db::postgres::PostgresPool -> golem_service_base::db::postgres::PostgresPool, golem_service_base::db::sqlite::SqlitePool
)]
#[async_trait]
impl ApiDeploymentRepo for DbApiDeploymentRepo<golem_service_base::db::postgres::PostgresPool> {
    async fn create(
        &self,
        namespace: &str,
        deployments: Vec<ApiDeploymentRecord>,
    ) -> Result<(), RepoError> {
        if !deployments.is_empty() {
            let mut transaction = self
                .db_pool
                .with_rw("api_deployment", "create")
                .begin()
                .await?;
            for deployment in deployments {
                let query = sqlx::query(
                    r#"
                      INSERT INTO api_deployments
                        (namespace, site, host, subdomain, definition_id, definition_version, created_at)
                      VALUES
                        ($1, $2, $3, $4, $5, $6, $7)
                       "#,
                )
                .bind(namespace)
                .bind(deployment.site.clone())
                .bind(deployment.host.clone())
                .bind(deployment.subdomain.clone())
                .bind(deployment.definition_id.clone())
                .bind(deployment.definition_version.clone())
                .bind(deployment.created_at);

                transaction.execute(query).await?;
            }

            self.db_pool
                .with_rw("api_deployment", "create")
                .commit(transaction)
                .await?;
        }
        Ok(())
    }

    async fn delete(
        &self,
        namespace: &str,
        deployments: Vec<ApiDeploymentRecord>,
    ) -> Result<bool, RepoError> {
        if !deployments.is_empty() {
            let mut transaction = self
                .db_pool
                .with_rw("api_deployment", "delete")
                .begin()
                .await?;
            for deployment in deployments {
                let query = sqlx::query(
                    "DELETE FROM api_deployments WHERE namespace = $1 AND site = $2 AND definition_id = $3 AND definition_version = $4",
                )
                .bind(namespace)
                .bind(deployment.site.clone())
                .bind(deployment.definition_id.clone())
                .bind(deployment.definition_version.clone());
                transaction.execute(query).await?;
            }
            self.db_pool
                .with_rw("api_deployment", "delete")
                .commit(transaction)
                .await?;
            Ok(true)
        } else {
            Ok(false)
        }
    }
    #[when(golem_service_base::db::postgres::PostgresPool -> get_all)]
    async fn get_all_postgres(
        &self,
        namespace: &str,
    ) -> Result<Vec<ApiDeploymentRecord>, RepoError> {
        let query = sqlx::query_as::<_, ApiDeploymentRecord>(
            r#"
                SELECT namespace, site, host, subdomain, definition_id, definition_version, created_at::timestamptz
                FROM api_deployments
                WHERE namespace = $1
                ORDER BY site, host, subdomain, definition_id, definition_version
                "#,
        )
        .bind(namespace);

        self.db_pool
            .with("api_deployment", "get_all")
            .fetch_all(query)
            .await
    }
    #[when(golem_service_base::db::sqlite::SqlitePool -> get_all)]
    async fn get_all_sqlite(&self, namespace: &str) -> Result<Vec<ApiDeploymentRecord>, RepoError> {
        let query = sqlx::query_as::<_, ApiDeploymentRecord>(
            r#"
                SELECT namespace, site, host, subdomain, definition_id, definition_version, created_at
                FROM api_deployments
                WHERE namespace = $1
                ORDER BY site, host, subdomain, definition_id, definition_version
                "#,
        )
        .bind(namespace);

        self.db_pool
            .with_ro("api_deployment", "get_all")
            .fetch_all(query)
            .await
    }

    #[when(golem_service_base::db::postgres::PostgresPool -> get_by_id)]
    async fn get_by_id_postgres(
        &self,
        namespace: &str,
        definition_id: &str,
    ) -> Result<Vec<ApiDeploymentRecord>, RepoError> {
        let query = sqlx::query_as::<_, ApiDeploymentRecord>(
            r#"
                SELECT namespace, site, host, subdomain, definition_id, definition_version, created_at::timestamptz
                FROM api_deployments
                WHERE namespace = $1 AND definition_id = $2
                ORDER BY site, host, subdomain, definition_version
                "#,
        )
        .bind(namespace)
        .bind(definition_id);

        self.db_pool
            .with("api_deployment", "get_by_id")
            .fetch_all(query)
            .await
    }

    #[when(golem_service_base::db::sqlite::SqlitePool -> get_by_id)]
    async fn get_by_id_sqlite(
        &self,
        namespace: &str,
        definition_id: &str,
    ) -> Result<Vec<ApiDeploymentRecord>, RepoError> {
        let query = sqlx::query_as::<_, ApiDeploymentRecord>(
            r#"
                SELECT namespace, site, host, subdomain, definition_id, definition_version, created_at
                FROM api_deployments
                WHERE namespace = $1 AND definition_id = $2
                ORDER BY site, host, subdomain, definition_version
                "#,
        )
        .bind(namespace)
        .bind(definition_id);

        self.db_pool
            .with_ro("api_deployment", "get_by_id")
            .fetch_all(query)
            .await
    }

    #[when(golem_service_base::db::postgres::PostgresPool -> get_by_id_and_version)]
    async fn get_by_id_and_version_postgres(
        &self,
        namespace: &str,
        definition_id: &str,
        definition_version: &str,
    ) -> Result<Vec<ApiDeploymentRecord>, RepoError> {
        let query = sqlx::query_as::<_, ApiDeploymentRecord>(
            r#"
                SELECT namespace, site, host, subdomain, definition_id, definition_version, created_at::timestamptz
                FROM api_deployments
                WHERE namespace = $1 AND definition_id = $2 AND definition_version = $3
                ORDER BY site, host, subdomain
                "#,
        )
        .bind(namespace)
        .bind(definition_id)
        .bind(definition_version);

        self.db_pool
            .with("api_deployment", "get_by_id_and_version")
            .fetch_all(query)
            .await
    }

    #[when(golem_service_base::db::sqlite::SqlitePool -> get_by_id_and_version)]
    async fn get_by_id_and_version_sqlite(
        &self,
        namespace: &str,
        definition_id: &str,
        definition_version: &str,
    ) -> Result<Vec<ApiDeploymentRecord>, RepoError> {
        let query = sqlx::query_as::<_, ApiDeploymentRecord>(
            r#"
                SELECT namespace, site, host, subdomain, definition_id, definition_version, created_at
                FROM api_deployments
                WHERE namespace = $1 AND definition_id = $2 AND definition_version = $3
                ORDER BY site, host, subdomain
                "#,
        )
        .bind(namespace)
        .bind(definition_id)
        .bind(definition_version);

        self.db_pool
            .with_ro("api_deployment", "get_by_id_and_version")
            .fetch_all(query)
            .await
    }

    #[when(golem_service_base::db::postgres::PostgresPool -> get_by_site)]
    async fn get_by_site_postgres(
        &self,
        namespace: &str,
        site: &str,
    ) -> Result<Vec<ApiDeploymentRecord>, RepoError> {
        let query = sqlx::query_as::<_, ApiDeploymentRecord>(
            r#"
                SELECT namespace, site, host, subdomain, definition_id, definition_version, created_at::timestamptz
                FROM api_deployments
                WHERE namespace = $1 and site = $2
                ORDER BY namespace, host, subdomain, definition_id, definition_version
                "#,
        )
        .bind(namespace).bind(site);

        self.db_pool
            .with("api_deployment", "get_by_site")
            .fetch_all(query)
            .await
    }

    #[when(golem_service_base::db::sqlite::SqlitePool -> get_by_site)]
    async fn get_by_site_sqlite(
        &self,
        namespace: &str,
        site: &str,
    ) -> Result<Vec<ApiDeploymentRecord>, RepoError> {
        let query = sqlx::query_as::<_, ApiDeploymentRecord>(
            r#"
                SELECT namespace, site, host, subdomain, definition_id, definition_version, created_at
                FROM api_deployments
                WHERE namespace = $1 and site = $2
                ORDER BY namespace, host, subdomain, definition_id, definition_version
                "#,
        )
        .bind(namespace).bind(site);

        self.db_pool
            .with_ro("api_deployment", "get_by_site")
            .fetch_all(query)
            .await
    }

    #[when(golem_service_base::db::postgres::PostgresPool -> get_definitions_by_site)]
    async fn get_definitions_by_site_postgres(
        &self,
        namespace: &str,
        site: &str,
    ) -> Result<Vec<ApiDefinitionRecord>, RepoError> {
        let query = sqlx::query_as::<_, ApiDefinitionRecord>(
            r#"
                SELECT api_definitions.namespace, api_definitions.id, api_definitions.version, api_definitions.draft, api_definitions.data AS data, api_definitions.created_at::timestamptz
                FROM api_deployments
                  JOIN api_definitions ON api_deployments.namespace = api_definitions.namespace AND api_deployments.definition_id = api_definitions.id AND api_deployments.definition_version = api_definitions.version
                WHERE api_deployments.namespace = $1 AND api_deployments.site = $2
                ORDER BY api_definitions.namespace, api_definitions.id, api_definitions.version
                "#
        )
        .bind(namespace)
        .bind(site);

        self.db_pool
            .with("api_deployment", "get_definitions_by_site")
            .fetch_all(query)
            .await
    }

    #[when(golem_service_base::db::sqlite::SqlitePool -> get_definitions_by_site)]
    async fn get_definitions_by_site_sqlite(
        &self,
        namespace: &str,
        site: &str,
    ) -> Result<Vec<ApiDefinitionRecord>, RepoError> {
        let query = sqlx::query_as::<_, ApiDefinitionRecord>(
            r#"
                SELECT api_definitions.namespace, api_definitions.id, api_definitions.version, api_definitions.draft, api_definitions.data, api_definitions.created_at
                FROM api_deployments
                  JOIN api_definitions ON api_deployments.namespace = api_definitions.namespace AND api_deployments.definition_id = api_definitions.id AND api_deployments.definition_version = api_definitions.version
                WHERE api_deployments.namespace = $1 AND api_deployments.site = $2
                ORDER BY api_definitions.namespace, api_definitions.id, api_definitions.version
                "#
        )
        .bind(namespace)
        .bind(site);

        self.db_pool
            .with_ro("api_deployment", "get_definitions_by_site")
            .fetch_all(query)
            .await
    }

    #[when(golem_service_base::db::postgres::PostgresPool -> get_all_definitions_by_site)]
    async fn get_all_definitions_by_site_postgres(
        &self,
        site: &str,
    ) -> Result<Vec<ApiDefinitionRecord>, RepoError> {
        let query = sqlx::query_as::<_, ApiDefinitionRecord>(
            r#"
                SELECT api_definitions.namespace, api_definitions.id, api_definitions.version, api_definitions.draft, api_definitions.data AS data, api_definitions.created_at::timestamptz
                FROM api_deployments
                  JOIN api_definitions ON api_deployments.namespace = api_definitions.namespace AND api_deployments.definition_id = api_definitions.id AND api_deployments.definition_version = api_definitions.version
                WHERE api_deployments.site = $1
                ORDER BY api_definitions.namespace, api_definitions.id, api_definitions.version
                "#
        )
        .bind(site);

        self.db_pool
            .with("api_deployment", "get_all_definitions_by_site")
            .fetch_all(query)
            .await
    }

    #[when(golem_service_base::db::sqlite::SqlitePool -> get_all_definitions_by_site)]
    async fn get_all_definitions_by_site_sqlite(
        &self,
        site: &str,
    ) -> Result<Vec<ApiDefinitionRecord>, RepoError> {
        let query = sqlx::query_as::<_, ApiDefinitionRecord>(
            r#"
                SELECT api_definitions.namespace, api_definitions.id, api_definitions.version, api_definitions.draft, api_definitions.data, api_definitions.created_at
                FROM api_deployments
                  JOIN api_definitions ON api_deployments.namespace = api_definitions.namespace AND api_deployments.definition_id = api_definitions.id AND api_deployments.definition_version = api_definitions.version
                WHERE api_deployments.site = $1
                ORDER BY api_definitions.namespace, api_definitions.id, api_definitions.version
                "#
        )
        .bind(site);

        self.db_pool
            .with_ro("api_deployment", "get_all_definitions_by_site")
            .fetch_all(query)
            .await
    }
}
