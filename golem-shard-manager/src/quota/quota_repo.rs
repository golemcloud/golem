// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
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

use async_trait::async_trait;
use conditional_trait_gen::trait_gen;
use futures::FutureExt;
use golem_common::error_forwarding;
use golem_common::model::quota::{ResourceDefinition, ResourceDefinitionId};
use golem_service_base::db::postgres::PostgresPool;
use golem_service_base::db::sqlite::SqlitePool;
use golem_service_base::db::{LabelledPoolApi, Pool};
use golem_service_base::model::quota_lease::PendingReservation;
use golem_service_base::repo::{Blob, NumericU64, RepoError, SqlDateTime};
use indoc::indoc;
use sqlx::types::Json;
use std::fmt::Debug;
use std::net::IpAddr;
use tracing::{Instrument, info_span};
use uuid::Uuid;

#[derive(Debug, thiserror::Error)]
pub enum QuotaRepoError {
    #[error(
        "Concurrent modification: revision conflict — another process may have written to the database"
    )]
    ConcurrentModification,
    #[error(transparent)]
    InternalError(#[from] anyhow::Error),
}

error_forwarding!(QuotaRepoError, RepoError);

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct QuotaResourceRecord {
    pub resource_definition_id: Uuid,
    pub revision: i64,
    pub definition: Blob<ResourceDefinition>,
    pub remaining: NumericU64,
    pub last_refilled_at: SqlDateTime,
    pub last_refreshed_at: SqlDateTime,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct QuotaLeaseRecord {
    pub resource_definition_id: Uuid,
    pub pod_ip: Json<IpAddr>,
    pub pod_port: i32,
    pub epoch: NumericU64,
    pub allocated: NumericU64,
    pub granted_at: SqlDateTime,
    pub expires_at: SqlDateTime,
    pub pending_reservations: Blob<Vec<PendingReservation>>,
}

#[async_trait]
pub trait QuotaRepo: Send + Sync {
    async fn save_lease_change(
        &self,
        resource: &QuotaResourceRecord,
        previous_resource_revision: i64,
        lease: &QuotaLeaseRecord,
        expired_pods: &[(Json<IpAddr>, i32)],
    ) -> Result<(), QuotaRepoError>;

    async fn save_lease_release(
        &self,
        resource: &QuotaResourceRecord,
        previous_resource_revision: i64,
        pod_ip: Json<IpAddr>,
        pod_port: i32,
    ) -> Result<(), QuotaRepoError>;

    async fn save_resource(
        &self,
        record: &QuotaResourceRecord,
        previous_revision: i64,
    ) -> Result<(), QuotaRepoError>;

    async fn delete_resource_and_leases(
        &self,
        resource_definition_id: ResourceDefinitionId,
    ) -> Result<(), QuotaRepoError>;

    async fn get_all_resources(&self) -> Result<Vec<QuotaResourceRecord>, QuotaRepoError>;
    async fn get_all_leases(&self) -> Result<Vec<QuotaLeaseRecord>, QuotaRepoError>;

    async fn delete_leases_for_resource(
        &self,
        resource_definition_id: ResourceDefinitionId,
    ) -> Result<(), QuotaRepoError>;
}

static SPAN_NAME: &str = "quota repository";

pub struct LoggedQuotaRepo<Repo: QuotaRepo> {
    repo: Repo,
}

impl<Repo: QuotaRepo> LoggedQuotaRepo<Repo> {
    pub fn new(repo: Repo) -> Self {
        Self { repo }
    }

    fn span_resource(resource_definition_id: ResourceDefinitionId) -> tracing::Span {
        info_span!(SPAN_NAME, resource_definition_id = %resource_definition_id)
    }

    fn span() -> tracing::Span {
        info_span!(SPAN_NAME)
    }
}

#[async_trait]
impl<Repo: QuotaRepo> QuotaRepo for LoggedQuotaRepo<Repo> {
    async fn save_lease_change(
        &self,
        resource: &QuotaResourceRecord,
        previous_resource_revision: i64,
        lease: &QuotaLeaseRecord,
        expired_pods: &[(Json<IpAddr>, i32)],
    ) -> Result<(), QuotaRepoError> {
        self.repo
            .save_lease_change(resource, previous_resource_revision, lease, expired_pods)
            .instrument(Self::span_resource(ResourceDefinitionId(
                resource.resource_definition_id,
            )))
            .await
    }

    async fn save_lease_release(
        &self,
        resource: &QuotaResourceRecord,
        previous_resource_revision: i64,
        pod_ip: Json<IpAddr>,
        pod_port: i32,
    ) -> Result<(), QuotaRepoError> {
        self.repo
            .save_lease_release(resource, previous_resource_revision, pod_ip, pod_port)
            .instrument(Self::span_resource(ResourceDefinitionId(
                resource.resource_definition_id,
            )))
            .await
    }

    async fn save_resource(
        &self,
        record: &QuotaResourceRecord,
        previous_revision: i64,
    ) -> Result<(), QuotaRepoError> {
        self.repo
            .save_resource(record, previous_revision)
            .instrument(Self::span_resource(ResourceDefinitionId(
                record.resource_definition_id,
            )))
            .await
    }

    async fn delete_resource_and_leases(
        &self,
        resource_definition_id: ResourceDefinitionId,
    ) -> Result<(), QuotaRepoError> {
        self.repo
            .delete_resource_and_leases(resource_definition_id)
            .instrument(Self::span_resource(resource_definition_id))
            .await
    }

    async fn get_all_resources(&self) -> Result<Vec<QuotaResourceRecord>, QuotaRepoError> {
        self.repo.get_all_resources().instrument(Self::span()).await
    }

    async fn get_all_leases(&self) -> Result<Vec<QuotaLeaseRecord>, QuotaRepoError> {
        self.repo.get_all_leases().instrument(Self::span()).await
    }

    async fn delete_leases_for_resource(
        &self,
        resource_definition_id: ResourceDefinitionId,
    ) -> Result<(), QuotaRepoError> {
        self.repo
            .delete_leases_for_resource(resource_definition_id)
            .instrument(Self::span_resource(resource_definition_id))
            .await
    }
}

const SVC_NAME: &str = "quota_repo";

pub struct DbQuotaRepo<DBP: Pool> {
    pool: DBP,
}

impl<DBP: Pool> DbQuotaRepo<DBP> {
    pub fn new(pool: DBP) -> Self {
        Self { pool }
    }

    pub fn logged(pool: DBP) -> LoggedQuotaRepo<Self>
    where
        Self: QuotaRepo,
    {
        LoggedQuotaRepo::new(Self::new(pool))
    }
}

#[trait_gen(PostgresPool -> PostgresPool, SqlitePool)]
#[async_trait]
impl QuotaRepo for DbQuotaRepo<PostgresPool> {
    async fn save_lease_change(
        &self,
        resource: &QuotaResourceRecord,
        previous_resource_revision: i64,
        lease: &QuotaLeaseRecord,
        expired_pods: &[(Json<IpAddr>, i32)],
    ) -> Result<(), QuotaRepoError> {
        let resource = resource.clone();
        let lease = lease.clone();
        let expired_pods = expired_pods.to_vec();

        self.pool
            .with_tx_err(SVC_NAME, "save_lease_change", |tx| {
                async move {
                    Self::upsert_resource_in_tx(tx, &resource, previous_resource_revision).await?;
                    Self::upsert_lease_in_tx(tx, &lease).await?;
                    for (ip, port) in &expired_pods {
                        Self::delete_lease_in_tx(tx, resource.resource_definition_id, *ip, *port)
                            .await?;
                    }
                    Ok(())
                }
                .boxed()
            })
            .await
    }

    async fn save_lease_release(
        &self,
        resource: &QuotaResourceRecord,
        previous_resource_revision: i64,
        pod_ip: Json<IpAddr>,
        pod_port: i32,
    ) -> Result<(), QuotaRepoError> {
        let resource = resource.clone();

        self.pool
            .with_tx_err(SVC_NAME, "save_lease_release", |tx| {
                async move {
                    Self::upsert_resource_in_tx(tx, &resource, previous_resource_revision).await?;
                    Self::delete_lease_in_tx(tx, resource.resource_definition_id, pod_ip, pod_port)
                        .await?;
                    Ok(())
                }
                .boxed()
            })
            .await
    }

    async fn save_resource(
        &self,
        record: &QuotaResourceRecord,
        previous_revision: i64,
    ) -> Result<(), QuotaRepoError> {
        let record = record.clone();
        self.pool
            .with_tx_err(SVC_NAME, "save_resource", |tx| {
                async move {
                    Self::upsert_resource_in_tx(tx, &record, previous_revision).await?;
                    Ok(())
                }
                .boxed()
            })
            .await
    }

    async fn delete_resource_and_leases(
        &self,
        resource_definition_id: ResourceDefinitionId,
    ) -> Result<(), QuotaRepoError> {
        self.pool
            .with_tx_err(SVC_NAME, "delete_resource_and_leases", |tx| {
                async move {
                    tx.execute(
                        sqlx::query(indoc! { r#"
                            DELETE FROM quota_leases
                            WHERE resource_definition_id = $1
                        "#})
                        .bind(resource_definition_id.0),
                    )
                    .await?;
                    tx.execute(
                        sqlx::query(indoc! { r#"
                            DELETE FROM quota_resources
                            WHERE resource_definition_id = $1
                        "#})
                        .bind(resource_definition_id.0),
                    )
                    .await?;
                    Ok(())
                }
                .boxed()
            })
            .await
    }

    async fn get_all_resources(&self) -> Result<Vec<QuotaResourceRecord>, QuotaRepoError> {
        let result = self
            .pool
            .with_ro(SVC_NAME, "get_all_resources")
            .fetch_all_as(sqlx::query_as(indoc! { r#"
                SELECT resource_definition_id, revision, definition, remaining,
                       last_refilled_at, last_refreshed_at
                FROM quota_resources
            "#}))
            .await?;

        Ok(result)
    }

    async fn get_all_leases(&self) -> Result<Vec<QuotaLeaseRecord>, QuotaRepoError> {
        let result = self
            .pool
            .with_ro(SVC_NAME, "get_all_leases")
            .fetch_all_as(sqlx::query_as(indoc! { r#"
                SELECT resource_definition_id, pod_ip, pod_port,
                       epoch, allocated, granted_at, expires_at,
                       pending_reservations
                FROM quota_leases
            "#}))
            .await?;
        Ok(result)
    }

    async fn delete_leases_for_resource(
        &self,
        resource_definition_id: ResourceDefinitionId,
    ) -> Result<(), QuotaRepoError> {
        self.pool
            .with_rw(SVC_NAME, "delete_leases_for_resource")
            .execute(
                sqlx::query(indoc! { r#"
                    DELETE FROM quota_leases
                    WHERE resource_definition_id = $1
                "#})
                .bind(resource_definition_id.0),
            )
            .await?;
        Ok(())
    }
}

#[trait_gen(PostgresPool -> PostgresPool, SqlitePool)]
impl DbQuotaRepo<PostgresPool> {
    async fn upsert_resource_in_tx(
        tx: &mut <<PostgresPool as Pool>::LabelledApi as LabelledPoolApi>::LabelledTransaction,
        record: &QuotaResourceRecord,
        previous_revision: i64,
    ) -> Result<(), QuotaRepoError> {
        let result = tx
            .execute(
                sqlx::query(indoc! { r#"
                    INSERT INTO quota_resources
                        (resource_definition_id, revision, definition, remaining,
                         last_refilled_at, last_refreshed_at)
                    VALUES ($1, $2, $3, $4, $5, $6)
                    ON CONFLICT (resource_definition_id)
                    DO UPDATE SET
                        definition = $3,
                        remaining = $4,
                        last_refilled_at = $5,
                        last_refreshed_at = $6,
                        revision = $2
                    WHERE quota_resources.revision = $7
                "#})
                .bind(record.resource_definition_id)
                .bind(record.revision)
                .bind(&record.definition)
                .bind(record.remaining)
                .bind(record.last_refilled_at.clone())
                .bind(record.last_refreshed_at.clone())
                .bind(previous_revision),
            )
            .await?;

        if result.rows_affected() == 0 {
            return Err(QuotaRepoError::ConcurrentModification);
        }
        Ok(())
    }

    async fn upsert_lease_in_tx(
        tx: &mut <<PostgresPool as Pool>::LabelledApi as LabelledPoolApi>::LabelledTransaction,
        record: &QuotaLeaseRecord,
    ) -> Result<(), RepoError> {
        tx.execute(
            sqlx::query(indoc! { r#"
                INSERT INTO quota_leases
                    (resource_definition_id, pod_ip, pod_port,
                     epoch, allocated, granted_at, expires_at,
                     pending_reservations)
                VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
                ON CONFLICT (resource_definition_id, pod_ip, pod_port)
                DO UPDATE SET
                    epoch = $4,
                    allocated = $5,
                    granted_at = $6,
                    expires_at = $7,
                    pending_reservations = $8
            "#})
            .bind(record.resource_definition_id)
            .bind(record.pod_ip)
            .bind(record.pod_port)
            .bind(record.epoch)
            .bind(record.allocated)
            .bind(record.granted_at.clone())
            .bind(record.expires_at.clone())
            .bind(&record.pending_reservations),
        )
        .await?;
        Ok(())
    }

    async fn delete_lease_in_tx(
        tx: &mut <<PostgresPool as Pool>::LabelledApi as LabelledPoolApi>::LabelledTransaction,
        resource_definition_id: Uuid,
        pod_ip: Json<IpAddr>,
        pod_port: i32,
    ) -> Result<(), RepoError> {
        tx.execute(
            sqlx::query(indoc! { r#"
                DELETE FROM quota_leases
                WHERE resource_definition_id = $1
                  AND pod_ip = $2
                  AND pod_port = $3
            "#})
            .bind(resource_definition_id)
            .bind(pod_ip)
            .bind(pod_port),
        )
        .await?;
        Ok(())
    }
}
