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

use crate::model::{Certificate, CertificateId, CertificateRequest};
use async_trait::async_trait;
use conditional_trait_gen::{trait_gen, when};
use golem_common::model::auth::Namespace;
use golem_common::model::AccountId;
use golem_service_base::db::Pool;
use golem_service_base::repo::RepoError;
use uuid::Uuid;

#[derive(sqlx::FromRow, Debug, Clone)]
pub struct CertificateRecord {
    pub namespace: String,
    pub id: Uuid,
    pub domain_name: String,
    pub external_id: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

impl CertificateRecord {
    pub fn new(
        account_id: AccountId,
        certificate: CertificateRequest,
        external_id: String,
        created_at: chrono::DateTime<chrono::Utc>,
    ) -> Self {
        Self {
            namespace: Namespace {
                account_id,
                project_id: certificate.project_id,
            }
            .to_string(),
            id: Uuid::new_v4(),
            domain_name: certificate.domain_name,
            external_id,
            created_at,
        }
    }
}

impl TryFrom<CertificateRecord> for Certificate {
    type Error = String;

    fn try_from(value: CertificateRecord) -> Result<Self, Self::Error> {
        let namespace: Namespace = value.namespace.try_into()?;

        Ok(Certificate {
            id: CertificateId(value.id),
            project_id: namespace.project_id,
            domain_name: value.domain_name,
            created_at: Some(value.created_at),
        })
    }
}

#[async_trait]
pub trait ApiCertificateRepo: Send + Sync {
    async fn create_or_update(&self, record: &CertificateRecord) -> Result<(), RepoError>;

    async fn get(&self, namespace: &str, id: &Uuid)
        -> Result<Option<CertificateRecord>, RepoError>;

    async fn delete(&self, namespace: &str, id: &Uuid) -> Result<bool, RepoError>;

    async fn get_all(&self, namespace: &str) -> Result<Vec<CertificateRecord>, RepoError>;
}

pub struct DbApiCertificateRepo<DB: Pool> {
    db_pool: DB,
}

impl<DB: Pool> DbApiCertificateRepo<DB> {
    pub fn new(db_pool: DB) -> Self {
        Self { db_pool }
    }
}

#[trait_gen(golem_service_base::db::postgres::PostgresPool -> golem_service_base::db::postgres::PostgresPool, golem_service_base::db::sqlite::SqlitePool
)]
#[async_trait]
impl ApiCertificateRepo for DbApiCertificateRepo<golem_service_base::db::postgres::PostgresPool> {
    async fn create_or_update(&self, record: &CertificateRecord) -> Result<(), RepoError> {
        let query = sqlx::query(
            r#"
               INSERT INTO api_certificates
                (namespace, id, domain_name, external_id, created_at)
              VALUES
                ($1, $2, $3, $4, $5)
              ON CONFLICT (namespace, id) DO UPDATE
              SET domain_name = $3,
                  external_id = $4
            "#,
        )
        .bind(record.namespace.clone())
        .bind(record.id)
        .bind(record.domain_name.clone())
        .bind(record.external_id.clone())
        .bind(record.created_at);

        self.db_pool
            .with_rw("api_certificate", "create_or_update")
            .execute(query)
            .await?;

        Ok(())
    }

    #[when(golem_service_base::db::sqlite::SqlitePool -> get)]
    async fn get_sqlite(
        &self,
        namespace: &str,
        id: &Uuid,
    ) -> Result<Option<CertificateRecord>, RepoError> {
        let query = sqlx::query_as::<_, CertificateRecord>(
            "SELECT namespace, id, domain_name, external_id, created_at FROM api_certificates WHERE namespace = $1 AND id = $2",
        )
        .bind(namespace)
        .bind(id);

        self.db_pool
            .with_ro("api_certificate", "get")
            .fetch_optional_as(query)
            .await
    }

    #[when(golem_service_base::db::postgres::PostgresPool -> get)]
    async fn get_postgres(
        &self,
        namespace: &str,
        id: &Uuid,
    ) -> Result<Option<CertificateRecord>, RepoError> {
        let query = sqlx::query_as::<_, CertificateRecord>(
            "SELECT namespace, id, domain_name, external_id, created_at::timestamptz FROM api_certificates WHERE namespace = $1 AND id = $2",
        )
            .bind(namespace)
            .bind(id);

        self.db_pool
            .with_ro("api_certificate", "get")
            .fetch_optional_as(query)
            .await
    }

    async fn delete(&self, namespace: &str, id: &Uuid) -> Result<bool, RepoError> {
        let query = sqlx::query("DELETE FROM api_certificates WHERE namespace = $1 AND id = $2")
            .bind(namespace)
            .bind(id);

        self.db_pool
            .with_rw("api_certificate", "delete")
            .execute(query)
            .await?;

        Ok(true)
    }

    #[when(golem_service_base::db::sqlite::SqlitePool -> get_all)]
    async fn get_all_sqlite(&self, namespace: &str) -> Result<Vec<CertificateRecord>, RepoError> {
        let query = sqlx::query_as::<_, CertificateRecord>(
            "SELECT namespace, id, domain_name, external_id, created_at FROM api_certificates WHERE namespace = $1",
        )
        .bind(namespace);

        self.db_pool
            .with_ro("api_certificate", "get_all")
            .fetch_all(query)
            .await
    }

    #[when(golem_service_base::db::postgres::PostgresPool -> get_all)]
    async fn get_all_postgres(&self, namespace: &str) -> Result<Vec<CertificateRecord>, RepoError> {
        let query = sqlx::query_as::<_, CertificateRecord>(
            "SELECT namespace, id, domain_name, external_id, created_at::timestamptz FROM api_certificates WHERE namespace = $1",
        )
            .bind(namespace);

        self.db_pool
            .with_ro("api_certificate", "get_all")
            .fetch_all(query)
            .await
    }
}
