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

use crate::sharding::error::ShardManagerError;
use crate::sharding::etcd_retry::retry_retriable_until;
use crate::sharding::leader_election::LeaderFence;
use crate::sharding::persistence::{ExternalRevision, NO_REVISION};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use conditional_trait_gen::trait_gen;
use desert_rust::BinaryCodec;
use etcd_client::{
    Client, Compare, CompareOp, DeleteOptions, GetOptions, KeyValue, Txn, TxnOp, TxnResponse,
};
use futures::FutureExt;
use golem_common::error_forwarding;
use golem_common::model::quota::{ResourceDefinition, ResourceDefinitionId};
use golem_common::serialization::{serialize, try_deserialize};
use golem_service_base::db::postgres::PostgresPool;
use golem_service_base::db::sqlite::SqlitePool;
use golem_service_base::db::{LabelledPoolApi, Pool, PoolApi};
use golem_service_base::model::quota_lease::PendingReservation;
use golem_service_base::repo::{Blob, NumericU64, RepoError, SqlDateTime};
use indoc::indoc;
use std::fmt::Debug;
use std::net::{IpAddr, SocketAddr};
use std::time::Duration;
use tokio::time::Instant;
use tracing::{Instrument, info_span};
use uuid::Uuid;

#[derive(Debug, thiserror::Error)]
pub enum QuotaRepoError {
    #[error(
        "Concurrent modification: revision conflict — another process may have written to the database"
    )]
    ConcurrentModification,
    #[error(
        "Leadership lost: the election key {leader_key} is no longer held at creation revision \
         {create_revision}, so this shard manager may not record quota changes"
    )]
    LeadershipLost {
        leader_key: String,
        create_revision: i64,
    },
    #[error(transparent)]
    InternalError(#[from] anyhow::Error),
}

error_forwarding!(QuotaRepoError, RepoError);

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct QuotaResourceRecord {
    pub resource_definition_id: Uuid,
    pub definition: Blob<ResourceDefinition>,
    pub remaining: NumericU64,
    pub last_refilled_at: SqlDateTime,
    pub last_refreshed_at: SqlDateTime,
}

/// A resource as read back from the store, together with the revision it is stored at.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct StoredQuotaResource {
    #[sqlx(flatten)]
    pub record: QuotaResourceRecord,
    pub revision: ExternalRevision,
}

/// Everything the store holds: every resource with its revision, and every lease.
#[derive(Debug, Clone, Default)]
pub struct StoredQuotaState {
    pub resources: Vec<StoredQuotaResource>,
    pub leases: Vec<QuotaLeaseRecord>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct QuotaLeaseRecord {
    pub resource_definition_id: Uuid,
    pub pod_ip: Blob<IpAddr>,
    pub pod_port: i32,
    pub epoch: NumericU64,
    pub allocated: NumericU64,
    pub granted_at: SqlDateTime,
    pub expires_at: SqlDateTime,
    pub pending_reservations: Blob<Vec<PendingReservation>>,
}

/// Durable storage for the quota state, one compare-and-swap unit per resource.
///
/// Every write that stores a resource is guarded by the revision the caller last saw for it, with
/// the same contract as [`crate::RoutingTablePersistence::write`]:
///
/// * `previous_revision == NO_REVISION` means the resource must not be stored yet; the write
///   creates it, and fails if it exists at any revision.
/// * `previous_revision > NO_REVISION` means the stored resource must still be at that revision.
///   A write against a resource that has since been deleted fails; it never recreates it.
///
/// A refused write returns [`QuotaRepoError::ConcurrentModification`] (or, in distributed mode,
/// [`QuotaRepoError::LeadershipLost`]) having stored nothing, and a successful one returns the new
/// revision, which is always `>= 1` and greater than `previous_revision`. The lease writes and
/// deletions that accompany a resource write commit or roll back with it.
#[async_trait]
pub trait QuotaRepo: Send + Sync {
    async fn save_lease_change(
        &self,
        resource: &QuotaResourceRecord,
        previous_revision: ExternalRevision,
        lease: &QuotaLeaseRecord,
        expired_pods: &[(Blob<IpAddr>, i32)],
    ) -> Result<ExternalRevision, QuotaRepoError>;

    async fn save_lease_release(
        &self,
        resource: &QuotaResourceRecord,
        previous_revision: ExternalRevision,
        pod_ip: Blob<IpAddr>,
        pod_port: i32,
    ) -> Result<ExternalRevision, QuotaRepoError>;

    async fn save_resource(
        &self,
        record: &QuotaResourceRecord,
        previous_revision: ExternalRevision,
    ) -> Result<ExternalRevision, QuotaRepoError>;

    async fn delete_resource_and_leases(
        &self,
        resource_definition_id: ResourceDefinitionId,
    ) -> Result<(), QuotaRepoError>;

    /// Everything stored, read once at startup to rebuild the in-memory state.
    async fn get_all(&self) -> Result<StoredQuotaState, QuotaRepoError>;

    /// One resource as stored, with its leases, or `None` if the resource is not stored. Used to
    /// catch up with the store after a write whose outcome is unknown.
    async fn get_resource(
        &self,
        resource_definition_id: ResourceDefinitionId,
    ) -> Result<Option<(StoredQuotaResource, Vec<QuotaLeaseRecord>)>, QuotaRepoError>;

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
        previous_revision: ExternalRevision,
        lease: &QuotaLeaseRecord,
        expired_pods: &[(Blob<IpAddr>, i32)],
    ) -> Result<ExternalRevision, QuotaRepoError> {
        self.repo
            .save_lease_change(resource, previous_revision, lease, expired_pods)
            .instrument(Self::span_resource(ResourceDefinitionId(
                resource.resource_definition_id,
            )))
            .await
    }

    async fn save_lease_release(
        &self,
        resource: &QuotaResourceRecord,
        previous_revision: ExternalRevision,
        pod_ip: Blob<IpAddr>,
        pod_port: i32,
    ) -> Result<ExternalRevision, QuotaRepoError> {
        self.repo
            .save_lease_release(resource, previous_revision, pod_ip, pod_port)
            .instrument(Self::span_resource(ResourceDefinitionId(
                resource.resource_definition_id,
            )))
            .await
    }

    async fn save_resource(
        &self,
        record: &QuotaResourceRecord,
        previous_revision: ExternalRevision,
    ) -> Result<ExternalRevision, QuotaRepoError> {
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

    async fn get_all(&self) -> Result<StoredQuotaState, QuotaRepoError> {
        self.repo.get_all().instrument(Self::span()).await
    }

    async fn get_resource(
        &self,
        resource_definition_id: ResourceDefinitionId,
    ) -> Result<Option<(StoredQuotaResource, Vec<QuotaLeaseRecord>)>, QuotaRepoError> {
        self.repo
            .get_resource(resource_definition_id)
            .instrument(Self::span_resource(resource_definition_id))
            .await
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

/// Rejects a `previous_revision` that cannot have come from a store.
fn check_previous_revision(previous_revision: ExternalRevision) -> Result<(), QuotaRepoError> {
    if previous_revision < NO_REVISION {
        return Err(QuotaRepoError::InternalError(anyhow::anyhow!(
            "negative previous quota resource revision {previous_revision}"
        )));
    }
    Ok(())
}

/// Rejects a revision a store claims to have written at, if it would be indistinguishable from
/// "absent" or would not follow `previous_revision`.
fn check_stored_revision(
    revision: ExternalRevision,
    previous_revision: ExternalRevision,
) -> Result<ExternalRevision, QuotaRepoError> {
    if revision < 1 || revision <= previous_revision {
        return Err(QuotaRepoError::InternalError(anyhow::anyhow!(
            "quota store reported revision {revision} after a write guarded on \
             {previous_revision}, which is not a valid successor"
        )));
    }
    Ok(revision)
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

/// Creates a resource row. `ON CONFLICT DO NOTHING` turns "it already exists" into 0 rows, which
/// is the refusal a create-only write owes.
const INSERT_RESOURCE_SQL: &str = indoc! { r#"
    INSERT INTO quota_resources
        (resource_definition_id, revision, definition, remaining,
         last_refilled_at, last_refreshed_at)
    VALUES ($1, $2, $3, $4, $5, $6)
    ON CONFLICT (resource_definition_id) DO NOTHING
"#};

/// Replaces a resource row only while it is present and still at `$7`. An absent row matches
/// nothing, the same answer etcd gives for a compare against a deleted key.
const UPDATE_RESOURCE_SQL: &str = indoc! { r#"
    UPDATE quota_resources
    SET revision = $2,
        definition = $3,
        remaining = $4,
        last_refilled_at = $5,
        last_refreshed_at = $6
    WHERE resource_definition_id = $1 AND revision = $7
"#};

#[trait_gen(PostgresPool -> PostgresPool, SqlitePool)]
#[async_trait]
impl QuotaRepo for DbQuotaRepo<PostgresPool> {
    async fn save_lease_change(
        &self,
        resource: &QuotaResourceRecord,
        previous_revision: ExternalRevision,
        lease: &QuotaLeaseRecord,
        expired_pods: &[(Blob<IpAddr>, i32)],
    ) -> Result<ExternalRevision, QuotaRepoError> {
        check_previous_revision(previous_revision)?;
        let resource = resource.clone();
        let lease = lease.clone();
        let expired_pods = expired_pods.to_vec();

        let revision = self
            .pool
            .with_tx_err(SVC_NAME, "save_lease_change", |tx| {
                async move {
                    let revision =
                        Self::compare_and_swap_resource_in_tx(tx, &resource, previous_revision)
                            .await?;
                    Self::upsert_lease_in_tx(tx, &lease).await?;
                    for (ip, port) in &expired_pods {
                        Self::delete_lease_in_tx(tx, resource.resource_definition_id, ip, *port)
                            .await?;
                    }
                    Ok::<ExternalRevision, QuotaRepoError>(revision)
                }
                .boxed()
            })
            .await?;

        check_stored_revision(revision, previous_revision)
    }

    async fn save_lease_release(
        &self,
        resource: &QuotaResourceRecord,
        previous_revision: ExternalRevision,
        pod_ip: Blob<IpAddr>,
        pod_port: i32,
    ) -> Result<ExternalRevision, QuotaRepoError> {
        check_previous_revision(previous_revision)?;
        let resource = resource.clone();

        let revision = self
            .pool
            .with_tx_err(SVC_NAME, "save_lease_release", |tx| {
                async move {
                    let revision =
                        Self::compare_and_swap_resource_in_tx(tx, &resource, previous_revision)
                            .await?;
                    Self::delete_lease_in_tx(
                        tx,
                        resource.resource_definition_id,
                        &pod_ip,
                        pod_port,
                    )
                    .await?;
                    Ok::<ExternalRevision, QuotaRepoError>(revision)
                }
                .boxed()
            })
            .await?;

        check_stored_revision(revision, previous_revision)
    }

    async fn save_resource(
        &self,
        record: &QuotaResourceRecord,
        previous_revision: ExternalRevision,
    ) -> Result<ExternalRevision, QuotaRepoError> {
        check_previous_revision(previous_revision)?;
        let record = record.clone();

        let revision = self
            .pool
            .with_tx_err(SVC_NAME, "save_resource", |tx| {
                async move {
                    Self::compare_and_swap_resource_in_tx(tx, &record, previous_revision).await
                }
                .boxed()
            })
            .await?;

        check_stored_revision(revision, previous_revision)
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

    async fn get_all(&self) -> Result<StoredQuotaState, QuotaRepoError> {
        let resources = self
            .pool
            .with_ro(SVC_NAME, "get_all_resources")
            .fetch_all_as(sqlx::query_as(indoc! { r#"
                SELECT resource_definition_id, revision, definition, remaining,
                       last_refilled_at, last_refreshed_at
                FROM quota_resources
            "#}))
            .await?;
        let leases = self
            .pool
            .with_ro(SVC_NAME, "get_all_leases")
            .fetch_all_as(sqlx::query_as(indoc! { r#"
                SELECT resource_definition_id, pod_ip, pod_port,
                       epoch, allocated, granted_at, expires_at,
                       pending_reservations
                FROM quota_leases
            "#}))
            .await?;

        Ok(StoredQuotaState { resources, leases })
    }

    async fn get_resource(
        &self,
        resource_definition_id: ResourceDefinitionId,
    ) -> Result<Option<(StoredQuotaResource, Vec<QuotaLeaseRecord>)>, QuotaRepoError> {
        let resource: Option<StoredQuotaResource> = self
            .pool
            .with_ro(SVC_NAME, "get_resource")
            .fetch_optional_as(
                sqlx::query_as(indoc! { r#"
                    SELECT resource_definition_id, revision, definition, remaining,
                           last_refilled_at, last_refreshed_at
                    FROM quota_resources
                    WHERE resource_definition_id = $1
                "#})
                .bind(resource_definition_id.0),
            )
            .await?;
        let Some(resource) = resource else {
            return Ok(None);
        };

        let leases = self
            .pool
            .with_ro(SVC_NAME, "get_resource_leases")
            .fetch_all_as(
                sqlx::query_as(indoc! { r#"
                    SELECT resource_definition_id, pod_ip, pod_port,
                           epoch, allocated, granted_at, expires_at,
                           pending_reservations
                    FROM quota_leases
                    WHERE resource_definition_id = $1
                "#})
                .bind(resource_definition_id.0),
            )
            .await?;

        Ok(Some((resource, leases)))
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
    /// Stores `record` at `previous_revision + 1`, but only if it is still at `previous_revision`.
    ///
    /// Two statements, not one `INSERT ... ON CONFLICT DO UPDATE ... WHERE revision = $prev`: that
    /// statement's INSERT branch is unguarded, so it would recreate a resource deleted underneath
    /// a writer holding `previous_revision > NO_REVISION`, which etcd refuses.
    async fn compare_and_swap_resource_in_tx(
        tx: &mut <<PostgresPool as Pool>::LabelledApi as LabelledPoolApi>::LabelledTransaction,
        record: &QuotaResourceRecord,
        previous_revision: ExternalRevision,
    ) -> Result<ExternalRevision, QuotaRepoError> {
        let next_revision = previous_revision.checked_add(1).ok_or_else(|| {
            QuotaRepoError::InternalError(anyhow::anyhow!(
                "quota resource revision overflow after {previous_revision}"
            ))
        })?;

        let query = if previous_revision == NO_REVISION {
            sqlx::query(INSERT_RESOURCE_SQL)
                .bind(record.resource_definition_id)
                .bind(next_revision)
                .bind(&record.definition)
                .bind(record.remaining)
                .bind(record.last_refilled_at.clone())
                .bind(record.last_refreshed_at.clone())
        } else {
            sqlx::query(UPDATE_RESOURCE_SQL)
                .bind(record.resource_definition_id)
                .bind(next_revision)
                .bind(&record.definition)
                .bind(record.remaining)
                .bind(record.last_refilled_at.clone())
                .bind(record.last_refreshed_at.clone())
                .bind(previous_revision)
        };

        let result = tx.execute(query).await?;
        if result.rows_affected() == 0 {
            return Err(QuotaRepoError::ConcurrentModification);
        }
        Ok(next_revision)
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
            .bind(&record.pod_ip)
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
        pod_ip: &Blob<IpAddr>,
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

/// Prefix of every key the quota state is stored under in distributed mode.
pub const QUOTA_KEY_PREFIX: &str = "/golem/quota/";

/// How many keys one read of the quota state returns. Startup reads page through the prefix, so
/// the whole quota state never has to fit in one response under etcd-client's message size limit.
const READ_PAGE_SIZE: i64 = 1000;

/// How long a read of the quota state may spend retrying transient failures.
const READ_RETRY_BUDGET: Duration = Duration::from_secs(10);

/// The value stored at a resource's `state` key.
#[derive(Debug, Clone, BinaryCodec)]
#[desert(evolution())]
struct QuotaResourceState {
    definition: ResourceDefinition,
    remaining: u64,
    last_refilled_at: DateTime<Utc>,
    last_refreshed_at: DateTime<Utc>,
}

/// The value stored at a lease's key. The resource and the pod it belongs to are in the key.
#[derive(Debug, Clone, BinaryCodec)]
#[desert(evolution())]
struct QuotaLeaseState {
    epoch: u64,
    allocated: u64,
    granted_at: DateTime<Utc>,
    expires_at: DateTime<Utc>,
    pending_reservations: Vec<PendingReservation>,
}

fn resource_prefix(resource_definition_id: Uuid) -> String {
    format!("{QUOTA_KEY_PREFIX}{resource_definition_id}/")
}

fn state_key(resource_definition_id: Uuid) -> String {
    format!("{}state", resource_prefix(resource_definition_id))
}

fn leases_prefix(resource_definition_id: Uuid) -> String {
    format!("{}leases/", resource_prefix(resource_definition_id))
}

/// A lease key ends in the pod's socket address, which brackets an IPv6 address, so it parses
/// back unambiguously.
fn lease_key(resource_definition_id: Uuid, pod_ip: IpAddr, pod_port: u16) -> String {
    format!(
        "{}{}",
        leases_prefix(resource_definition_id),
        SocketAddr::new(pod_ip, pod_port)
    )
}

/// The first key after every key that starts with `prefix`, for a range read over it.
fn prefix_range_end(prefix: &str) -> Vec<u8> {
    let mut end = prefix.as_bytes().to_vec();
    let last = end.last_mut().expect("prefix is not empty");
    *last += 1;
    end
}

/// What a key under [`QUOTA_KEY_PREFIX`] holds.
enum QuotaKey {
    Resource(Uuid),
    Lease(Uuid, SocketAddr),
}

fn parse_key(key: &[u8]) -> Result<QuotaKey, QuotaRepoError> {
    let unrecognised = || {
        QuotaRepoError::InternalError(anyhow::anyhow!(
            "unrecognised quota key {}",
            String::from_utf8_lossy(key)
        ))
    };

    let rest = std::str::from_utf8(key)
        .ok()
        .and_then(|key| key.strip_prefix(QUOTA_KEY_PREFIX))
        .ok_or_else(unrecognised)?;
    let (id, rest) = rest.split_once('/').ok_or_else(unrecognised)?;
    let id = Uuid::parse_str(id).map_err(|_| unrecognised())?;

    if rest == "state" {
        Ok(QuotaKey::Resource(id))
    } else if let Some(pod) = rest.strip_prefix("leases/") {
        Ok(QuotaKey::Lease(
            id,
            pod.parse().map_err(|_| unrecognised())?,
        ))
    } else {
        Err(unrecognised())
    }
}

fn encode<T: desert_rust::BinarySerializer>(value: &T) -> Result<Vec<u8>, QuotaRepoError> {
    serialize(value).map_err(|err| QuotaRepoError::InternalError(anyhow::anyhow!(err)))
}

fn decode<T: desert_rust::BinaryDeserializer>(kv: &KeyValue) -> Result<T, QuotaRepoError> {
    try_deserialize(kv.value())
        .map_err(|err| QuotaRepoError::InternalError(anyhow::anyhow!(err)))?
        .ok_or_else(|| {
            QuotaRepoError::InternalError(anyhow::anyhow!(
                "quota key {} holds an empty value or an unknown serialization version",
                String::from_utf8_lossy(kv.key())
            ))
        })
}

fn etcd_error(err: etcd_client::Error) -> QuotaRepoError {
    QuotaRepoError::InternalError(anyhow::Error::from(err))
}

/// The quota state in etcd, for distributed mode.
///
/// Each resource is one compare-and-swap unit: its `state` key's `mod_revision` is the revision
/// the [`QuotaRepo`] contract talks about, and every write that stores a resource compares it.
/// Its leases sit under the resource's `leases/` prefix and are written in the same transaction.
/// Every write also carries the [`LeaderFence`], so a replica that lost the leadership cannot
/// overwrite what its successor restored.
pub struct EtcdQuotaRepo {
    client: Client,
    fence: LeaderFence,
}

impl EtcdQuotaRepo {
    pub fn new(client: Client, fence: LeaderFence) -> Self {
        Self { client, fence }
    }

    pub fn logged(client: Client, fence: LeaderFence) -> LoggedQuotaRepo<Self> {
        LoggedQuotaRepo::new(Self::new(client, fence))
    }

    fn state_value(record: &QuotaResourceRecord) -> Result<Vec<u8>, QuotaRepoError> {
        encode(&QuotaResourceState {
            definition: record.definition.value().clone(),
            remaining: record.remaining.get(),
            last_refilled_at: *record.last_refilled_at.as_utc(),
            last_refreshed_at: *record.last_refreshed_at.as_utc(),
        })
    }

    fn lease_key_of(
        resource_definition_id: Uuid,
        pod_ip: &Blob<IpAddr>,
        pod_port: i32,
    ) -> Result<String, QuotaRepoError> {
        let port = u16::try_from(pod_port).map_err(|_| {
            QuotaRepoError::InternalError(anyhow::anyhow!("invalid lease port {pod_port}"))
        })?;
        Ok(lease_key(resource_definition_id, *pod_ip.value(), port))
    }

    /// Commits `ops` if this replica still holds the leadership and the resource's `state` key is
    /// still at `previous_revision`, and returns the revision they were stored at.
    ///
    /// Not retried: a retry re-sends the same expected revision, so an attempt that did land
    /// would come back as a conflict.
    async fn compare_and_swap(
        &self,
        resource_definition_id: Uuid,
        previous_revision: ExternalRevision,
        ops: Vec<TxnOp>,
    ) -> Result<ExternalRevision, QuotaRepoError> {
        check_previous_revision(previous_revision)?;

        // etcd reports mod_revision 0 for an absent key, so `previous_revision == NO_REVISION`
        // already means "the resource must not exist yet", and a positive one refuses a deleted
        // resource instead of recreating it.
        let txn = Txn::new()
            .when([
                self.fence.compare(),
                Compare::mod_revision(
                    state_key(resource_definition_id),
                    CompareOp::Equal,
                    previous_revision,
                ),
            ])
            .and_then(ops)
            .or_else([self.fence.read_back()]);

        let response = self.client.kv_client().txn(txn).await.map_err(etcd_error)?;

        if !response.succeeded() {
            return Err(if self.fence.held_after_refusal(&response) {
                QuotaRepoError::ConcurrentModification
            } else {
                self.leadership_lost()
            });
        }

        check_stored_revision(Self::header_revision(&response)?, previous_revision)
    }

    /// Commits `ops` if this replica still holds the leadership. For deletions, which the SQL
    /// store does not guard by revision either.
    async fn fenced(&self, ops: Vec<TxnOp>) -> Result<(), QuotaRepoError> {
        let txn = Txn::new()
            .when([self.fence.compare()])
            .and_then(ops)
            .or_else([self.fence.read_back()]);

        let response = self.client.kv_client().txn(txn).await.map_err(etcd_error)?;

        if response.succeeded() {
            Ok(())
        } else {
            Err(self.leadership_lost())
        }
    }

    fn leadership_lost(&self) -> QuotaRepoError {
        QuotaRepoError::LeadershipLost {
            leader_key: self.fence.key_str(),
            create_revision: self.fence.create_revision(),
        }
    }

    fn header_revision(response: &TxnResponse) -> Result<ExternalRevision, QuotaRepoError> {
        Ok(response
            .header()
            .ok_or_else(|| {
                QuotaRepoError::InternalError(anyhow::anyhow!(
                    "etcd transaction response carried no header"
                ))
            })?
            .revision())
    }

    /// Sorts the keys of a read into resources and leases, decoding each. Anything under the
    /// prefix that is not in the layout, or does not decode, is an error rather than skipped:
    /// quota state that cannot be read is never silently dropped.
    fn decode_state(kvs: Vec<KeyValue>) -> Result<StoredQuotaState, QuotaRepoError> {
        let mut state = StoredQuotaState::default();
        for kv in kvs {
            match parse_key(kv.key())? {
                QuotaKey::Resource(id) => {
                    // etcd's revisions start at 1, so a live key cannot carry mod_revision 0,
                    // which the contract reserves for "not stored".
                    let revision = kv.mod_revision();
                    if revision < 1 {
                        return Err(QuotaRepoError::InternalError(anyhow::anyhow!(
                            "etcd returned quota resource {id} with mod_revision {revision}, \
                             which is reserved for absent keys"
                        )));
                    }

                    let resource: QuotaResourceState = decode(&kv)?;
                    state.resources.push(StoredQuotaResource {
                        record: QuotaResourceRecord {
                            resource_definition_id: id,
                            definition: Blob::new(resource.definition),
                            remaining: resource.remaining.into(),
                            last_refilled_at: resource.last_refilled_at.into(),
                            last_refreshed_at: resource.last_refreshed_at.into(),
                        },
                        revision,
                    });
                }
                QuotaKey::Lease(id, pod) => {
                    let lease: QuotaLeaseState = decode(&kv)?;
                    state.leases.push(QuotaLeaseRecord {
                        resource_definition_id: id,
                        pod_ip: Blob::new(pod.ip()),
                        pod_port: pod.port().into(),
                        epoch: lease.epoch.into(),
                        allocated: lease.allocated.into(),
                        granted_at: lease.granted_at.into(),
                        expires_at: lease.expires_at.into(),
                        pending_reservations: Blob::new(lease.pending_reservations),
                    });
                }
            }
        }
        Ok(state)
    }

    /// Every key under `prefix`, a page at a time. Unfenced, like the shard state read: only the
    /// leader writes, and it holds the lock of whatever it is reading.
    async fn read_prefix(&self, prefix: &str) -> Result<Vec<KeyValue>, QuotaRepoError> {
        let end = prefix_range_end(prefix);
        let mut next_key = prefix.as_bytes().to_vec();
        let mut kvs = Vec::new();

        loop {
            let options = GetOptions::new()
                .with_range(end.clone())
                .with_limit(READ_PAGE_SIZE);
            let page = retry_retriable_until(
                "reading the quota state",
                || {
                    let mut kv = self.client.kv_client();
                    let key = next_key.clone();
                    let options = options.clone();
                    async move { Ok::<_, ShardManagerError>(kv.get(key, Some(options)).await?) }
                },
                Instant::now() + READ_RETRY_BUDGET,
            )
            .await
            .map_err(|err| QuotaRepoError::InternalError(anyhow::Error::from(err)))?;

            let more = page.more();
            if let Some(last) = page.kvs().last() {
                // The smallest key after `last`.
                next_key = last.key().to_vec();
                next_key.push(0);
            }
            kvs.extend(page.kvs().iter().cloned());

            if !more {
                return Ok(kvs);
            }
        }
    }
}

#[async_trait]
impl QuotaRepo for EtcdQuotaRepo {
    async fn save_lease_change(
        &self,
        resource: &QuotaResourceRecord,
        previous_revision: ExternalRevision,
        lease: &QuotaLeaseRecord,
        expired_pods: &[(Blob<IpAddr>, i32)],
    ) -> Result<ExternalRevision, QuotaRepoError> {
        let id = resource.resource_definition_id;
        let lease_value = encode(&QuotaLeaseState {
            epoch: lease.epoch.get(),
            allocated: lease.allocated.get(),
            granted_at: *lease.granted_at.as_utc(),
            expires_at: *lease.expires_at.as_utc(),
            pending_reservations: lease.pending_reservations.value().clone(),
        })?;

        let mut ops = vec![
            TxnOp::put(state_key(id), Self::state_value(resource)?, None),
            TxnOp::put(
                Self::lease_key_of(id, &lease.pod_ip, lease.pod_port)?,
                lease_value,
                None,
            ),
        ];
        for (ip, port) in expired_pods {
            ops.push(TxnOp::delete(Self::lease_key_of(id, ip, *port)?, None));
        }

        self.compare_and_swap(id, previous_revision, ops).await
    }

    async fn save_lease_release(
        &self,
        resource: &QuotaResourceRecord,
        previous_revision: ExternalRevision,
        pod_ip: Blob<IpAddr>,
        pod_port: i32,
    ) -> Result<ExternalRevision, QuotaRepoError> {
        let id = resource.resource_definition_id;
        let ops = vec![
            TxnOp::put(state_key(id), Self::state_value(resource)?, None),
            TxnOp::delete(Self::lease_key_of(id, &pod_ip, pod_port)?, None),
        ];

        self.compare_and_swap(id, previous_revision, ops).await
    }

    async fn save_resource(
        &self,
        record: &QuotaResourceRecord,
        previous_revision: ExternalRevision,
    ) -> Result<ExternalRevision, QuotaRepoError> {
        let id = record.resource_definition_id;
        let ops = vec![TxnOp::put(state_key(id), Self::state_value(record)?, None)];

        self.compare_and_swap(id, previous_revision, ops).await
    }

    async fn delete_resource_and_leases(
        &self,
        resource_definition_id: ResourceDefinitionId,
    ) -> Result<(), QuotaRepoError> {
        self.fenced(vec![TxnOp::delete(
            resource_prefix(resource_definition_id.0),
            Some(DeleteOptions::new().with_prefix()),
        )])
        .await
    }

    async fn get_all(&self) -> Result<StoredQuotaState, QuotaRepoError> {
        Self::decode_state(self.read_prefix(QUOTA_KEY_PREFIX).await?)
    }

    async fn get_resource(
        &self,
        resource_definition_id: ResourceDefinitionId,
    ) -> Result<Option<(StoredQuotaResource, Vec<QuotaLeaseRecord>)>, QuotaRepoError> {
        let mut state = Self::decode_state(
            self.read_prefix(&resource_prefix(resource_definition_id.0))
                .await?,
        )?;
        // Leases without their resource are orphans: the resource is not stored.
        Ok(state
            .resources
            .pop()
            .map(|resource| (resource, state.leases)))
    }

    async fn delete_leases_for_resource(
        &self,
        resource_definition_id: ResourceDefinitionId,
    ) -> Result<(), QuotaRepoError> {
        self.fenced(vec![TxnOp::delete(
            leases_prefix(resource_definition_id.0),
            Some(DeleteOptions::new().with_prefix()),
        )])
        .await
    }
}

#[cfg(test)]
mod tests {
    use test_r::test;

    use super::*;
    use std::net::{Ipv4Addr, Ipv6Addr};

    #[test]
    fn keys_parse_back_to_what_they_were_built_from() {
        let id = Uuid::new_v4();
        assert!(matches!(
            parse_key(state_key(id).as_bytes()),
            Ok(QuotaKey::Resource(parsed)) if parsed == id
        ));

        for ip in [
            IpAddr::V4(Ipv4Addr::new(10, 0, 0, 7)),
            IpAddr::V6(Ipv6Addr::new(0xfd00, 0, 0, 0, 0, 0, 0, 7)),
        ] {
            let key = lease_key(id, ip, 9000);
            assert!(
                matches!(
                    parse_key(key.as_bytes()),
                    Ok(QuotaKey::Lease(parsed, pod))
                        if parsed == id && pod == SocketAddr::new(ip, 9000)
                ),
                "{key} did not parse back"
            );
        }
    }

    #[test]
    fn a_key_outside_the_layout_is_refused_rather_than_skipped() {
        let id = Uuid::new_v4();
        for key in [
            format!("{QUOTA_KEY_PREFIX}{id}/something"),
            format!("{QUOTA_KEY_PREFIX}not-a-uuid/state"),
            format!("{QUOTA_KEY_PREFIX}{id}/leases/not-an-address"),
            "/golem/elsewhere".to_string(),
        ] {
            assert!(parse_key(key.as_bytes()).is_err(), "{key} was accepted");
        }
    }

    #[test]
    fn the_range_end_covers_exactly_the_prefix() {
        let end = prefix_range_end(QUOTA_KEY_PREFIX);
        let inside = format!("{QUOTA_KEY_PREFIX}{}/state", Uuid::from_u128(u128::MAX));
        assert!(inside.as_bytes() < end.as_slice());
        assert!(b"/golem/quota0".as_slice() >= end.as_slice());
        assert!(b"/golem/quota".as_slice() < QUOTA_KEY_PREFIX.as_bytes());
    }
}
