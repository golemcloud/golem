use crate::model::{AccountApiDomain, ApiDomain};
use crate::service::auth::CloudNamespace;
use async_trait::async_trait;
use golem_common::model::AccountId;
use golem_worker_service_base::repo::RepoError;
use sqlx::{Database, Pool};
use std::ops::Deref;
use std::sync::Arc;

#[derive(sqlx::FromRow, Debug, Clone)]
pub struct ApiDomainRecord {
    pub namespace: String,
    pub domain_name: String,
    pub name_servers: String,
}

impl ApiDomainRecord {
    pub fn new(account_id: AccountId, domain: ApiDomain) -> Self {
        Self {
            namespace: CloudNamespace {
                account_id,
                project_id: domain.project_id,
            }
            .to_string(),
            domain_name: domain.domain_name,
            name_servers: domain.name_servers.join(","),
        }
    }
}

impl TryFrom<ApiDomainRecord> for AccountApiDomain {
    type Error = String;

    fn try_from(value: ApiDomainRecord) -> Result<Self, Self::Error> {
        let namespace: CloudNamespace = value.namespace.try_into()?;

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
pub trait ApiDomainRepo {
    async fn create_or_update(&self, record: &ApiDomainRecord) -> Result<(), RepoError>;

    async fn get(&self, domain_name: &str) -> Result<Option<ApiDomainRecord>, RepoError>;

    async fn delete(&self, domain_name: &str) -> Result<bool, RepoError>;

    async fn get_all(&self, namespace: &str) -> Result<Vec<ApiDomainRecord>, RepoError>;
}

pub struct DbApiDomainRepo<DB: Database> {
    db_pool: Arc<Pool<DB>>,
}

impl<DB: Database> DbApiDomainRepo<DB> {
    pub fn new(db_pool: Arc<Pool<DB>>) -> Self {
        Self { db_pool }
    }
}

#[async_trait]
impl ApiDomainRepo for DbApiDomainRepo<sqlx::Sqlite> {
    async fn create_or_update(&self, record: &ApiDomainRecord) -> Result<(), RepoError> {
        sqlx::query(
            r#"
               INSERT INTO api_domains
                (namespace, domain_name, name_servers)
              VALUES
                ($1, $2, $3)
              ON CONFLICT (namespace, domain_name) DO UPDATE
              SET name_servers = $3
            "#,
        )
        .bind(record.namespace.clone())
        .bind(record.domain_name.clone())
        .bind(record.name_servers.clone())
        .execute(self.db_pool.deref())
        .await?;

        Ok(())
    }

    async fn get(&self, domain_name: &str) -> Result<Option<ApiDomainRecord>, RepoError> {
        sqlx::query_as::<_, ApiDomainRecord>("SELECT * FROM api_domains WHERE domain_name = $1")
            .bind(domain_name)
            .fetch_optional(self.db_pool.deref())
            .await
            .map_err(|e| e.into())
    }

    async fn delete(&self, domain_name: &str) -> Result<bool, RepoError> {
        sqlx::query("DELETE FROM api_domains WHERE domain_name = $1")
            .bind(domain_name)
            .execute(self.db_pool.deref())
            .await?;
        Ok(true)
    }

    async fn get_all(&self, namespace: &str) -> Result<Vec<ApiDomainRecord>, RepoError> {
        sqlx::query_as::<_, ApiDomainRecord>("SELECT * FROM api_domains WHERE namespace = $1")
            .bind(namespace)
            .fetch_all(self.db_pool.deref())
            .await
            .map_err(|e| e.into())
    }
}

#[async_trait]
impl ApiDomainRepo for DbApiDomainRepo<sqlx::Postgres> {
    async fn create_or_update(&self, record: &ApiDomainRecord) -> Result<(), RepoError> {
        sqlx::query(
            r#"
               INSERT INTO api_domains
                (namespace, domain_name, name_servers)
              VALUES
                ($1, $2, $3)
              ON CONFLICT (namespace, domain_name) DO UPDATE
              SET name_servers = $3
            "#,
        )
        .bind(record.namespace.clone())
        .bind(record.domain_name.clone())
        .bind(record.name_servers.clone())
        .execute(self.db_pool.deref())
        .await?;

        Ok(())
    }

    async fn get(&self, domain_name: &str) -> Result<Option<ApiDomainRecord>, RepoError> {
        sqlx::query_as::<_, ApiDomainRecord>("SELECT * FROM api_domains WHERE domain_name = $1")
            .bind(domain_name)
            .fetch_optional(self.db_pool.deref())
            .await
            .map_err(|e| e.into())
    }

    async fn delete(&self, domain_name: &str) -> Result<bool, RepoError> {
        sqlx::query("DELETE FROM api_domains WHERE domain_name = $1")
            .bind(domain_name)
            .execute(self.db_pool.deref())
            .await?;
        Ok(true)
    }

    async fn get_all(&self, namespace: &str) -> Result<Vec<ApiDomainRecord>, RepoError> {
        sqlx::query_as::<_, ApiDomainRecord>("SELECT * FROM api_domains WHERE namespace = $1")
            .bind(namespace)
            .fetch_all(self.db_pool.deref())
            .await
            .map_err(|e| e.into())
    }
}
