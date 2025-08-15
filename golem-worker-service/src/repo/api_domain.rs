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

use crate::model::{AccountApiDomain, ApiDomain, DomainRequest};
use async_trait::async_trait;
use conditional_trait_gen::{trait_gen, when};
use golem_common::model::auth::Namespace;
use golem_common::model::AccountId;
use golem_service_base::db::Pool;
use golem_service_base::repo::RepoError;

#[derive(sqlx::FromRow, Debug, Clone)]
pub struct ApiDomainRecord {
    pub namespace: String,
    pub domain_name: String,
    pub name_servers: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

impl ApiDomainRecord {
    pub fn new(
        account_id: AccountId,
        domain: DomainRequest,
        name_servers: Vec<String>,
        created_at: chrono::DateTime<chrono::Utc>,
    ) -> Self {
        Self {
            namespace: Namespace {
                account_id,
                project_id: domain.project_id,
            }
            .to_string(),
            domain_name: domain.domain_name,
            name_servers: name_servers.join(","),
            created_at,
        }
    }
}

impl TryFrom<ApiDomainRecord> for AccountApiDomain {
    type Error = String;

    fn try_from(value: ApiDomainRecord) -> Result<Self, Self::Error> {
        let namespace: Namespace = value.namespace.try_into()?;

        let name_servers = if value.name_servers.is_empty() {
            vec![]
        } else {
            value
                .name_servers
                .split(',')
                .map(|s| s.to_string())
                .collect()
        };

        Ok(AccountApiDomain {
            account_id: namespace.account_id,
            domain: ApiDomain {
                project_id: namespace.project_id,
                domain_name: value.domain_name,
                name_servers,
                created_at: Some(value.created_at),
            },
        })
    }
}

impl TryFrom<ApiDomainRecord> for ApiDomain {
    type Error = String;

    fn try_from(value: ApiDomainRecord) -> Result<Self, Self::Error> {
        let value: AccountApiDomain = value.try_into()?;
        Ok(value.domain)
    }
}

#[async_trait]
pub trait ApiDomainRepo: Send + Sync {
    async fn create_or_update(&self, record: &ApiDomainRecord) -> Result<(), RepoError>;

    async fn get(&self, domain_name: &str) -> Result<Option<ApiDomainRecord>, RepoError>;

    async fn delete(&self, domain_name: &str) -> Result<bool, RepoError>;

    async fn get_all(&self, namespace: &str) -> Result<Vec<ApiDomainRecord>, RepoError>;
}

pub struct DbApiDomainRepo<DB: Pool> {
    db_pool: DB,
}

impl<DB: Pool> DbApiDomainRepo<DB> {
    pub fn new(db_pool: DB) -> Self {
        Self { db_pool }
    }
}

#[trait_gen(golem_service_base::db::postgres::PostgresPool -> golem_service_base::db::postgres::PostgresPool, golem_service_base::db::sqlite::SqlitePool
)]
#[async_trait]
impl ApiDomainRepo for DbApiDomainRepo<golem_service_base::db::postgres::PostgresPool> {
    async fn create_or_update(&self, record: &ApiDomainRecord) -> Result<(), RepoError> {
        let query = sqlx::query(
            r#"
               INSERT INTO api_domains
                (namespace, domain_name, name_servers, created_at)
              VALUES
                ($1, $2, $3, $4)
              ON CONFLICT (namespace, domain_name) DO UPDATE
              SET name_servers = $3
            "#,
        )
        .bind(record.namespace.clone())
        .bind(record.domain_name.clone())
        .bind(record.name_servers.clone())
        .bind(record.created_at);

        self.db_pool
            .with_rw("api_domain", "create_or_update")
            .execute(query)
            .await?;

        Ok(())
    }

    #[when(golem_service_base::db::sqlite::SqlitePool -> get)]
    async fn get_sqlite(&self, domain_name: &str) -> Result<Option<ApiDomainRecord>, RepoError> {
        let query = sqlx::query_as::<_, ApiDomainRecord>("SELECT namespace, domain_name, name_servers, created_at FROM api_domains WHERE domain_name = $1")
            .bind(domain_name);

        self.db_pool
            .with_ro("api_domain", "get")
            .fetch_optional_as(query)
            .await
    }

    #[when(golem_service_base::db::postgres::PostgresPool -> get)]
    async fn get_postgres(&self, domain_name: &str) -> Result<Option<ApiDomainRecord>, RepoError> {
        let query = sqlx::query_as::<_, ApiDomainRecord>("SELECT namespace, domain_name, name_servers, created_at::timestamptz FROM api_domains WHERE domain_name = $1")
            .bind(domain_name);

        self.db_pool
            .with_ro("api_domain", "get")
            .fetch_optional_as(query)
            .await
    }

    async fn delete(&self, domain_name: &str) -> Result<bool, RepoError> {
        let query = sqlx::query("DELETE FROM api_domains WHERE domain_name = $1").bind(domain_name);

        self.db_pool
            .with_rw("api_domain", "delete")
            .execute(query)
            .await?;

        Ok(true)
    }

    #[when(golem_service_base::db::sqlite::SqlitePool -> get_all)]
    async fn get_all_sqlite(&self, namespace: &str) -> Result<Vec<ApiDomainRecord>, RepoError> {
        let query = sqlx::query_as::<_, ApiDomainRecord>("SELECT namespace, domain_name, name_servers, created_at FROM api_domains WHERE namespace = $1")
            .bind(namespace);

        self.db_pool
            .with_ro("api_domain", "get_all")
            .fetch_all(query)
            .await
    }

    #[when(golem_service_base::db::postgres::PostgresPool -> get_all)]
    async fn get_all_postgres(&self, namespace: &str) -> Result<Vec<ApiDomainRecord>, RepoError> {
        let query = sqlx::query_as::<_, ApiDomainRecord>("SELECT namespace, domain_name, name_servers, created_at::timestamptz FROM api_domains WHERE namespace = $1")
            .bind(namespace);

        self.db_pool
            .with_ro("api_domain", "get_all")
            .fetch_all(query)
            .await
    }
}
