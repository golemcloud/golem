use crate::model::{Certificate, CertificateId};
use crate::service::auth::CloudNamespace;
use async_trait::async_trait;
use golem_common::model::AccountId;
use golem_worker_service_base::repo::RepoError;
use sqlx::{Database, Pool};
use std::ops::Deref;
use std::sync::Arc;
use uuid::Uuid;

#[derive(sqlx::FromRow, Debug, Clone)]
pub struct CertificateRecord {
    pub namespace: String,
    pub id: Uuid,
    pub domain_name: String,
    pub external_id: String,
}

impl CertificateRecord {
    pub fn new(account_id: AccountId, certificate: Certificate, external_id: String) -> Self {
        Self {
            namespace: CloudNamespace {
                account_id,
                project_id: certificate.project_id,
            }
            .to_string(),
            id: certificate.id.0,
            domain_name: certificate.domain_name,
            external_id,
        }
    }
}

impl TryFrom<CertificateRecord> for Certificate {
    type Error = String;

    fn try_from(value: CertificateRecord) -> Result<Self, Self::Error> {
        let namespace: CloudNamespace = value.namespace.try_into()?;

        Ok(Certificate {
            id: CertificateId(value.id),
            project_id: namespace.project_id,
            domain_name: value.domain_name,
        })
    }
}

#[async_trait]
pub trait ApiCertificateRepo {
    async fn create_or_update(&self, record: &CertificateRecord) -> Result<(), RepoError>;

    async fn get(&self, namespace: &str, id: &Uuid)
        -> Result<Option<CertificateRecord>, RepoError>;

    async fn delete(&self, namespace: &str, id: &Uuid) -> Result<bool, RepoError>;

    async fn get_all(&self, namespace: &str) -> Result<Vec<CertificateRecord>, RepoError>;
}

pub struct DbApiCertificateRepo<DB: Database> {
    db_pool: Arc<Pool<DB>>,
}

impl<DB: Database> DbApiCertificateRepo<DB> {
    pub fn new(db_pool: Arc<Pool<DB>>) -> Self {
        Self { db_pool }
    }
}

#[async_trait]
impl ApiCertificateRepo for DbApiCertificateRepo<sqlx::Sqlite> {
    async fn create_or_update(&self, record: &CertificateRecord) -> Result<(), RepoError> {
        sqlx::query(
            r#"
               INSERT INTO api_certificates
                (namespace, id, domain_name, external_id)
              VALUES
                ($1, $2, $3, $4)
              ON CONFLICT (namespace, id) DO UPDATE
              SET domain_name = $3,
                  external_id = $4
            "#,
        )
        .bind(record.namespace.clone())
        .bind(record.id)
        .bind(record.domain_name.clone())
        .bind(record.external_id.clone())
        .execute(self.db_pool.deref())
        .await?;

        Ok(())
    }

    async fn get(
        &self,
        namespace: &str,
        id: &Uuid,
    ) -> Result<Option<CertificateRecord>, RepoError> {
        sqlx::query_as::<_, CertificateRecord>(
            "SELECT * FROM api_certificates WHERE namespace = $1 AND id = $2",
        )
        .bind(namespace)
        .bind(id)
        .fetch_optional(self.db_pool.deref())
        .await
        .map_err(|e| e.into())
    }

    async fn delete(&self, namespace: &str, id: &Uuid) -> Result<bool, RepoError> {
        sqlx::query("DELETE FROM api_certificates WHERE namespace = $1 AND id = $2")
            .bind(namespace)
            .bind(id)
            .execute(self.db_pool.deref())
            .await?;
        Ok(true)
    }

    async fn get_all(&self, namespace: &str) -> Result<Vec<CertificateRecord>, RepoError> {
        sqlx::query_as::<_, CertificateRecord>(
            "SELECT * FROM api_certificates WHERE namespace = $1",
        )
        .bind(namespace)
        .fetch_all(self.db_pool.deref())
        .await
        .map_err(|e| e.into())
    }
}

#[async_trait]
impl ApiCertificateRepo for DbApiCertificateRepo<sqlx::Postgres> {
    async fn create_or_update(&self, record: &CertificateRecord) -> Result<(), RepoError> {
        sqlx::query(
            r#"
               INSERT INTO api_certificates
                (namespace, id, domain_name, external_id)
              VALUES
                ($1, $2, $3, $4)
              ON CONFLICT (namespace, id) DO UPDATE
              SET domain_name = $3,
                  external_id = $4
            "#,
        )
        .bind(record.namespace.clone())
        .bind(record.id)
        .bind(record.domain_name.clone())
        .bind(record.external_id.clone())
        .execute(self.db_pool.deref())
        .await?;

        Ok(())
    }

    async fn get(
        &self,
        namespace: &str,
        id: &Uuid,
    ) -> Result<Option<CertificateRecord>, RepoError> {
        sqlx::query_as::<_, CertificateRecord>(
            "SELECT * FROM api_certificates WHERE namespace = $1 AND id = $2",
        )
        .bind(namespace)
        .bind(id)
        .fetch_optional(self.db_pool.deref())
        .await
        .map_err(|e| e.into())
    }

    async fn delete(&self, namespace: &str, id: &Uuid) -> Result<bool, RepoError> {
        sqlx::query("DELETE FROM api_certificates WHERE namespace = $1 AND id = $2")
            .bind(namespace)
            .bind(id)
            .execute(self.db_pool.deref())
            .await?;
        Ok(true)
    }

    async fn get_all(&self, namespace: &str) -> Result<Vec<CertificateRecord>, RepoError> {
        sqlx::query_as::<_, CertificateRecord>(
            "SELECT * FROM api_certificates WHERE namespace = $1",
        )
        .bind(namespace)
        .fetch_all(self.db_pool.deref())
        .await
        .map_err(|e| e.into())
    }
}
