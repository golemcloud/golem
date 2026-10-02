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

//! The quota state against the real stores: the [`QuotaRepo`] contract on every backend, and
//! [`QuotaService`] driven over it.
//!
//! The quota unit tests run the service over a repository that stores nothing. What is only
//! provable here is that both backends honour one compare-and-swap contract, that a lease change
//! and its expired-lease deletions commit together, that a new leader restores exactly what the
//! previous one stored, and that etcd accepts every write the service makes - including the ones
//! a transaction limit or a duplicate key would refuse.
//!
//! Every multiplied test runs as `<name>_sqlite`, `<name>_postgres` and `<name>_etcd`, over a quota
//! repository from the same stores the routing-table persistence tests use. The etcd-only tests at
//! the end cover what only distributed mode has: the leadership fence and paged reads.

use crate::etcd_backed::persistence::{
    EtcdRoutingTablePersistenceFactory, GetRoutingTablePersistence, PersistenceStore,
};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use golem_common::model::Pod;
use golem_common::model::environment::EnvironmentId;
use golem_common::model::quota::{
    EnforcementAction, LeaseEpoch, ResourceConcurrencyLimit, ResourceDefinition,
    ResourceDefinitionId, ResourceDefinitionRevision, ResourceLimit, ResourceName,
};
use golem_service_base::model::quota_lease::PendingReservation;
use golem_service_base::repo::{Blob, NumericU64, SqlDateTime};
use golem_shard_manager::config::QuotaServiceConfig;
use golem_shard_manager::quota::quota_repo::{
    QuotaLeaseRecord, QuotaRepoError, QuotaResourceRecord, StoredQuotaResource, StoredQuotaState,
};
use golem_shard_manager::quota::resource_definition_fetcher::FetchError;
use golem_shard_manager::quota::{
    EtcdQuotaRepo, QuotaError, QuotaLease, QuotaRepo, QuotaService, ResourceDefinitionFetcher,
};
use golem_shard_manager::{ExternalRevision, NO_REVISION, ReadRetry};
use golem_test_framework::components::etcd::docker_etcd::DockerEtcd;
use std::collections::BTreeSet;
use std::net::{IpAddr, Ipv4Addr};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::time::Duration;
use test_r::{define_matrix_dimension, inherit_test_dep, test};
use uuid::Uuid;

inherit_test_dep!(Arc<DockerEtcd>);
inherit_test_dep!(#[tagged_as("sqlite")] Arc<dyn GetRoutingTablePersistence>);
inherit_test_dep!(#[tagged_as("postgres")] Arc<dyn GetRoutingTablePersistence>);
inherit_test_dep!(#[tagged_as("etcd")] Arc<dyn GetRoutingTablePersistence>);

// Declared per module on purpose: the macro emits a module-local helper that the generated test
// cases call unqualified, so it cannot be shared from a sibling.
define_matrix_dimension!(persistence: Arc<dyn GetRoutingTablePersistence> -> "sqlite", "postgres", "etcd");

const SLOTS: u64 = 200;

fn definition() -> ResourceDefinition {
    ResourceDefinition {
        id: ResourceDefinitionId(Uuid::new_v4()),
        revision: ResourceDefinitionRevision::INITIAL,
        environment_id: EnvironmentId(Uuid::new_v4()),
        name: ResourceName("slots".to_string()),
        limit: ResourceLimit::Concurrency(ResourceConcurrencyLimit { value: SLOTS }),
        enforcement_action: EnforcementAction::Reject,
        unit: "slot".to_string(),
        units: "slots".to_string(),
    }
}

fn pod(n: usize) -> Pod {
    Pod {
        ip: IpAddr::V4(Ipv4Addr::new(10, 0, (n / 256) as u8, (n % 256) as u8)),
        port: 9000,
    }
}

fn resource(definition: &ResourceDefinition, remaining: u64) -> QuotaResourceRecord {
    QuotaResourceRecord {
        resource_definition_id: definition.id.0,
        definition: Blob::new(definition.clone()),
        remaining: NumericU64::new(remaining),
        last_refilled_at: SqlDateTime::new(Utc::now()),
        last_refreshed_at: SqlDateTime::new(Utc::now()),
    }
}

fn lease(
    definition: &ResourceDefinition,
    pod: Pod,
    epoch: u64,
    expires_at: DateTime<Utc>,
) -> QuotaLeaseRecord {
    QuotaLeaseRecord {
        resource_definition_id: definition.id.0,
        pod_ip: Blob::new(pod.ip),
        pod_port: pod.port.into(),
        epoch: NumericU64::new(epoch),
        allocated: NumericU64::new(1),
        granted_at: SqlDateTime::new(Utc::now()),
        expires_at: SqlDateTime::new(expires_at),
        pending_reservations: Blob::new(Vec::new()),
    }
}

fn in_a_minute() -> DateTime<Utc> {
    Utc::now() + chrono::Duration::minutes(1)
}

fn a_minute_ago() -> DateTime<Utc> {
    Utc::now() - chrono::Duration::minutes(1)
}

fn expired(pod: Pod) -> (Blob<IpAddr>, i32) {
    (Blob::new(pod.ip), pod.port.into())
}

async fn stored_resources(repo: &Arc<dyn QuotaRepo>) -> Vec<StoredQuotaResource> {
    repo.get_all()
        .await
        .expect("reading the stored quota state should succeed")
        .resources
}

/// The stored resource `definition` names, or `None` if it is not stored.
async fn stored_resource(
    repo: &Arc<dyn QuotaRepo>,
    definition: &ResourceDefinition,
) -> Option<StoredQuotaResource> {
    stored_resources(repo)
        .await
        .into_iter()
        .find(|stored| stored.record.resource_definition_id == definition.id.0)
}

async fn stored_leases(
    repo: &Arc<dyn QuotaRepo>,
    definition: &ResourceDefinition,
) -> Vec<QuotaLeaseRecord> {
    repo.get_all()
        .await
        .expect("reading the stored quota state should succeed")
        .leases
        .into_iter()
        .filter(|lease| lease.resource_definition_id == definition.id.0)
        .collect()
}

async fn stored_pods(repo: &Arc<dyn QuotaRepo>, definition: &ResourceDefinition) -> BTreeSet<Pod> {
    stored_leases(repo, definition)
        .await
        .into_iter()
        .map(|lease| Pod {
            ip: lease.pod_ip.into_value(),
            port: lease.pod_port.try_into().expect("stored port fits a u16"),
        })
        .collect()
}

async fn stored_epoch(repo: &Arc<dyn QuotaRepo>, definition: &ResourceDefinition, of: Pod) -> u64 {
    stored_leases(repo, definition)
        .await
        .into_iter()
        .find(|lease| lease.pod_ip.value() == &of.ip && lease.pod_port == i32::from(of.port))
        .map(|lease| lease.epoch.get())
        .unwrap_or_else(|| panic!("no lease is stored for {of}"))
}

/// Everything handed out plus everything left: a slot counted twice, or lost, shows up here.
async fn stored_total(repo: &Arc<dyn QuotaRepo>, definition: &ResourceDefinition) -> u64 {
    let remaining = stored_resource(repo, definition)
        .await
        .expect("the resource should be stored")
        .record
        .remaining
        .get();
    let allocated: u64 = stored_leases(repo, definition)
        .await
        .iter()
        .map(|lease| lease.allocated.get())
        .sum();
    remaining + allocated
}

fn is_conflict(result: &Result<ExternalRevision, QuotaRepoError>) -> bool {
    matches!(result, Err(QuotaRepoError::ConcurrentModification))
}

// -- the repository contract ------------------------------------------------------------------

#[test]
#[tracing::instrument]
async fn a_resource_and_its_lease_round_trip(
    #[dimension(persistence)] persistence: &Arc<dyn GetRoutingTablePersistence>,
) {
    let repo = persistence.new_store().await.quota_repo().await;
    let definition = definition();

    let revision = repo
        .save_lease_change(
            &resource(&definition, SLOTS - 1),
            NO_REVISION,
            &lease(&definition, pod(1), 7, in_a_minute()),
            &[],
        )
        .await
        .expect("creating a resource with its first lease should succeed");
    assert!(revision >= 1, "a stored revision must not read as absent");

    let stored = stored_resource(&repo, &definition)
        .await
        .expect("the resource should be stored");
    assert_eq!(stored.revision, revision);
    assert_eq!(stored.record.remaining.get(), SLOTS - 1);
    assert_eq!(stored.record.definition.value().id, definition.id);
    assert_eq!(
        stored.record.definition.value().limit,
        definition.limit,
        "the definition did not survive the round trip"
    );

    let leases = stored_leases(&repo, &definition).await;
    assert_eq!(leases.len(), 1);
    assert_eq!(leases[0].pod_ip.value(), &pod(1).ip);
    assert_eq!(leases[0].pod_port, 9000);
    assert_eq!(leases[0].epoch.get(), 7);
    assert_eq!(leases[0].allocated.get(), 1);
}

#[test]
#[tracing::instrument]
async fn each_write_is_stored_at_a_later_revision(
    #[dimension(persistence)] persistence: &Arc<dyn GetRoutingTablePersistence>,
) {
    let repo = persistence.new_store().await.quota_repo().await;
    let definition = definition();

    let created = repo
        .save_resource(&resource(&definition, SLOTS), NO_REVISION)
        .await
        .expect("creating the resource should succeed");
    let leased = repo
        .save_lease_change(
            &resource(&definition, SLOTS - 1),
            created,
            &lease(&definition, pod(1), 1, in_a_minute()),
            &[],
        )
        .await
        .expect("a lease change at the stored revision should succeed");
    let released = repo
        .save_lease_release(
            &resource(&definition, SLOTS),
            leased,
            Blob::new(pod(1).ip),
            9000,
        )
        .await
        .expect("a release at the stored revision should succeed");

    assert!(created < leased && leased < released);
    assert_eq!(
        stored_resource(&repo, &definition)
            .await
            .map(|s| s.revision),
        Some(released)
    );
}

#[test]
#[tracing::instrument]
// A write guarded by a revision that is no longer stored came from a writer that missed another
// one's change. Accepting it would overwrite that change; the store must refuse it whole.
async fn a_write_guarded_by_a_superseded_revision_is_refused(
    #[dimension(persistence)] persistence: &Arc<dyn GetRoutingTablePersistence>,
) {
    let repo = persistence.new_store().await.quota_repo().await;
    let definition = definition();

    let first = repo
        .save_resource(&resource(&definition, SLOTS), NO_REVISION)
        .await
        .expect("creating the resource should succeed");
    let second = repo
        .save_resource(&resource(&definition, 5), first)
        .await
        .expect("a write at the stored revision should succeed");

    assert!(is_conflict(
        &repo.save_resource(&resource(&definition, 1), first).await
    ));
    assert!(
        is_conflict(
            &repo
                .save_lease_change(
                    &resource(&definition, 1),
                    first,
                    &lease(&definition, pod(1), 1, in_a_minute()),
                    &[],
                )
                .await
        ),
        "a stale lease change must be refused too"
    );

    let stored = stored_resource(&repo, &definition)
        .await
        .expect("the resource should still be stored");
    assert_eq!(stored.revision, second);
    assert_eq!(stored.record.remaining.get(), 5);
    assert!(
        stored_leases(&repo, &definition).await.is_empty(),
        "a refused lease change stored its lease anyway"
    );
}

#[test]
#[tracing::instrument]
async fn creating_a_resource_that_is_already_stored_is_refused(
    #[dimension(persistence)] persistence: &Arc<dyn GetRoutingTablePersistence>,
) {
    let repo = persistence.new_store().await.quota_repo().await;
    let definition = definition();

    repo.save_resource(&resource(&definition, SLOTS), NO_REVISION)
        .await
        .expect("creating the resource should succeed");

    assert!(is_conflict(
        &repo
            .save_resource(&resource(&definition, 1), NO_REVISION)
            .await
    ));
}

#[test]
#[tracing::instrument]
// The SQL store's old upsert inserted whenever the row was missing, whatever revision the writer
// held; etcd refuses the same write. A writer that missed a deletion must not bring the resource
// back on either backend.
async fn a_deleted_resource_is_not_recreated_by_a_stale_writer(
    #[dimension(persistence)] persistence: &Arc<dyn GetRoutingTablePersistence>,
) {
    let repo = persistence.new_store().await.quota_repo().await;
    let definition = definition();

    let revision = repo
        .save_lease_change(
            &resource(&definition, SLOTS - 1),
            NO_REVISION,
            &lease(&definition, pod(1), 1, in_a_minute()),
            &[],
        )
        .await
        .expect("creating the resource should succeed");
    repo.delete_resource_and_leases(definition.id)
        .await
        .expect("deleting the resource should succeed");

    assert!(is_conflict(
        &repo
            .save_resource(&resource(&definition, SLOTS), revision)
            .await
    ));
    assert!(stored_resource(&repo, &definition).await.is_none());
    assert!(stored_leases(&repo, &definition).await.is_empty());
}

#[test]
#[tracing::instrument]
async fn expired_leases_are_deleted_by_the_write_that_reclaims_them(
    #[dimension(persistence)] persistence: &Arc<dyn GetRoutingTablePersistence>,
) {
    let repo = persistence.new_store().await.quota_repo().await;
    let definition = definition();

    let first = repo
        .save_lease_change(
            &resource(&definition, SLOTS - 1),
            NO_REVISION,
            &lease(&definition, pod(1), 1, in_a_minute()),
            &[],
        )
        .await
        .expect("the first lease should be stored");
    let second = repo
        .save_lease_change(
            &resource(&definition, SLOTS - 2),
            first,
            &lease(&definition, pod(2), 1, a_minute_ago()),
            &[],
        )
        .await
        .expect("the second lease should be stored");
    repo.save_lease_change(
        &resource(&definition, SLOTS - 1),
        second,
        &lease(&definition, pod(1), 2, in_a_minute()),
        &[expired(pod(2))],
    )
    .await
    .expect("a renewal that reclaims an expired lease should succeed");

    assert_eq!(
        stored_pods(&repo, &definition).await,
        BTreeSet::from([pod(1)])
    );
    assert_eq!(stored_epoch(&repo, &definition, pod(1)).await, 2);
}

#[test]
#[tracing::instrument]
async fn a_release_deletes_only_its_own_lease(
    #[dimension(persistence)] persistence: &Arc<dyn GetRoutingTablePersistence>,
) {
    let repo = persistence.new_store().await.quota_repo().await;
    let definition = definition();

    let first = repo
        .save_lease_change(
            &resource(&definition, SLOTS - 1),
            NO_REVISION,
            &lease(&definition, pod(1), 1, in_a_minute()),
            &[],
        )
        .await
        .expect("the first lease should be stored");
    let second = repo
        .save_lease_change(
            &resource(&definition, SLOTS - 2),
            first,
            &lease(&definition, pod(2), 1, in_a_minute()),
            &[],
        )
        .await
        .expect("the second lease should be stored");
    repo.save_lease_release(
        &resource(&definition, SLOTS - 1),
        second,
        Blob::new(pod(1).ip),
        9000,
    )
    .await
    .expect("the release should succeed");

    assert_eq!(
        stored_pods(&repo, &definition).await,
        BTreeSet::from([pod(2)])
    );
}

#[test]
#[tracing::instrument]
async fn deletes_reach_only_the_resource_they_name(
    #[dimension(persistence)] persistence: &Arc<dyn GetRoutingTablePersistence>,
) {
    let repo = persistence.new_store().await.quota_repo().await;
    let [kept, emptied, deleted] = [definition(), definition(), definition()];

    for definition in [&kept, &emptied, &deleted] {
        repo.save_lease_change(
            &resource(definition, SLOTS - 1),
            NO_REVISION,
            &lease(definition, pod(1), 1, in_a_minute()),
            &[],
        )
        .await
        .expect("each resource should be stored with a lease");
    }

    repo.delete_leases_for_resource(emptied.id)
        .await
        .expect("deleting one resource's leases should succeed");
    repo.delete_resource_and_leases(deleted.id)
        .await
        .expect("deleting one resource should succeed");

    assert!(stored_resource(&repo, &emptied).await.is_some());
    assert!(stored_leases(&repo, &emptied).await.is_empty());
    assert!(stored_resource(&repo, &deleted).await.is_none());
    assert!(stored_leases(&repo, &deleted).await.is_empty());
    assert!(stored_resource(&repo, &kept).await.is_some());
    assert_eq!(
        stored_pods(&repo, &kept).await,
        BTreeSet::from([pod(1)]),
        "a delete reached a resource it did not name"
    );
}

// -- the service over a real store ------------------------------------------------------------

/// Serves the definitions it was given, and nothing else.
struct Definitions(Vec<ResourceDefinition>);

#[async_trait]
impl ResourceDefinitionFetcher for Definitions {
    async fn fetch_by_id(
        &self,
        id: ResourceDefinitionId,
    ) -> Result<ResourceDefinition, FetchError> {
        self.0
            .iter()
            .find(|definition| definition.id == id)
            .cloned()
            .ok_or(FetchError::NotFound)
    }

    async fn resolve_by_name(
        &self,
        environment_id: EnvironmentId,
        name: ResourceName,
    ) -> Result<ResourceDefinition, FetchError> {
        self.0
            .iter()
            .find(|definition| {
                definition.environment_id == environment_id && definition.name == name
            })
            .cloned()
            .ok_or(FetchError::NotFound)
    }

    async fn invalidate(&self, _environment_id: EnvironmentId, _name: ResourceName) {}

    async fn invalidate_all(&self) {}
}

fn quota_service(
    repo: Arc<dyn QuotaRepo>,
    definition: &ResourceDefinition,
    lease_duration: Duration,
) -> Arc<QuotaService> {
    QuotaService::new(
        QuotaServiceConfig {
            lease_duration,
            definition_staleness_ttl: Duration::from_secs(3600),
            min_executors: 1,
        },
        Arc::new(Definitions(vec![definition.clone()])),
        repo,
    )
}

/// The epoch a granted lease carries, which its holder renews and releases with, and the amount it
/// was allocated. Renewing or releasing with that amount as `unused` hands it all back: anything
/// not reported unused counts as consumed.
fn granted(lease: &QuotaLease) -> (LeaseEpoch, u64) {
    match lease {
        QuotaLease::Bounded {
            epoch,
            allocated_amount,
            ..
        } => (*epoch, *allocated_amount),
        QuotaLease::Unlimited { .. } => panic!("expected a bounded lease, got an unlimited one"),
    }
}

async fn acquire(
    quota: &QuotaService,
    definition: &ResourceDefinition,
    pod: Pod,
) -> (LeaseEpoch, u64) {
    let lease = quota
        .acquire_lease(definition.environment_id, definition.name.clone(), pod)
        .await
        .unwrap_or_else(|err| panic!("acquiring a lease for {pod} should succeed: {err}"));
    granted(&lease)
}

#[test]
#[tracing::instrument]
// Each step is read back through another client: the service's own memory would agree with it
// even if nothing reached the store.
async fn acquire_renew_and_release_are_stored(
    #[dimension(persistence)] persistence: &Arc<dyn GetRoutingTablePersistence>,
) {
    let store = persistence.new_store().await;
    let reader = store.quota_repo().await;
    let definition = definition();
    let quota = quota_service(
        store.quota_repo().await,
        &definition,
        Duration::from_secs(60),
    );

    // The store keeps the epoch the holder must present next, one past the one it was handed.
    let (acquired, allocated) = acquire(&quota, &definition, pod(1)).await;
    assert_eq!(
        stored_epoch(&reader, &definition, pod(1)).await,
        acquired.0 + 1
    );

    let (renewed, allocated) = granted(
        &quota
            .renew_lease(definition.id, pod(1), acquired, allocated, vec![])
            .await
            .expect("renewing with the granted epoch should succeed"),
    );
    assert_eq!(
        stored_epoch(&reader, &definition, pod(1)).await,
        renewed.0 + 1
    );

    let batch = quota
        .batch_renew_leases(vec![(definition.id, pod(1), renewed, allocated, vec![])])
        .await;
    let (batch_renewed, allocated) = granted(
        batch[0]
            .as_ref()
            .expect("a batch renewal with the current epoch should succeed"),
    );
    assert_eq!(
        stored_epoch(&reader, &definition, pod(1)).await,
        batch_renewed.0 + 1
    );

    quota
        .release_lease(definition.id, pod(1), batch_renewed, allocated)
        .await
        .expect("releasing with the current epoch should succeed");
    assert!(stored_pods(&reader, &definition).await.is_empty());
    assert_eq!(stored_total(&reader, &definition).await, SLOTS);
}

#[test]
#[tracing::instrument]
async fn a_stale_epoch_is_refused_and_stores_nothing(
    #[dimension(persistence)] persistence: &Arc<dyn GetRoutingTablePersistence>,
) {
    let store = persistence.new_store().await;
    let reader = store.quota_repo().await;
    let definition = definition();
    let quota = quota_service(
        store.quota_repo().await,
        &definition,
        Duration::from_secs(60),
    );

    let (acquired, _) = acquire(&quota, &definition, pod(1)).await;
    quota
        .renew_lease(definition.id, pod(1), acquired, 0, vec![])
        .await
        .expect("the first renewal should succeed");
    let before = stored_resource(&reader, &definition)
        .await
        .map(|s| s.revision);

    let refused = quota
        .renew_lease(definition.id, pod(1), acquired, 0, vec![])
        .await;
    assert!(
        matches!(refused, Err(QuotaError::StaleEpoch { .. })),
        "renewing twice with the same epoch should be refused as stale, got {refused:?}"
    );
    assert_eq!(
        stored_resource(&reader, &definition)
            .await
            .map(|s| s.revision),
        before,
        "a refused renewal wrote to the store"
    );
}

#[test]
#[tracing::instrument]
// The service holds the revision its last write was stored at. If anything else stores the
// resource in between, the service's next write must be refused rather than overwrite it - and the
// one after that must build on what the store now holds, not stay refused.
async fn a_change_stored_by_another_writer_is_not_overwritten_and_is_built_on(
    #[dimension(persistence)] persistence: &Arc<dyn GetRoutingTablePersistence>,
) {
    let store = persistence.new_store().await;
    let other = store.quota_repo().await;
    let definition = definition();
    let quota = quota_service(
        store.quota_repo().await,
        &definition,
        Duration::from_secs(60),
    );

    let (acquired, _) = acquire(&quota, &definition, pod(1)).await;
    let current = stored_resource(&other, &definition)
        .await
        .expect("the acquisition should have stored the resource");
    let theirs = other
        .save_resource(&resource(&definition, 3), current.revision)
        .await
        .expect("the other writer's change should be stored");

    let refused = quota
        .renew_lease(definition.id, pod(1), acquired, 0, vec![])
        .await;
    assert!(
        matches!(refused, Err(QuotaError::InternalError(_))),
        "the service overwrote another writer's change, or failed for the wrong reason: \
         {refused:?}"
    );

    let stored = stored_resource(&other, &definition)
        .await
        .expect("the resource should still be stored");
    assert_eq!(stored.revision, theirs);
    assert_eq!(stored.record.remaining.get(), 3);

    quota
        .renew_lease(definition.id, pod(1), acquired, 0, vec![])
        .await
        .expect("the retried renewal should succeed on the state reloaded from the store");
    let stored = stored_resource(&other, &definition)
        .await
        .expect("the resource should still be stored");
    assert!(stored.revision > theirs);
    assert!(
        stored.record.remaining.get() <= 3,
        "the renewal was built on the service's old state, not on the other writer's"
    );
}

/// What [`Unreliable`] does to the next write it forwards.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Fault {
    None,
    /// The write is stored, and then reported as failed - a reply lost to a timeout.
    FailAfterStoring,
    /// The write is stored, and then nothing is reported - the caller gives up and drops it.
    HangAfterStoring,
}

/// A real repository whose next write can land and still look, to the service, as if it had not.
struct Unreliable {
    inner: Arc<dyn QuotaRepo>,
    fault: std::sync::Mutex<Fault>,
}

impl Unreliable {
    fn new(inner: Arc<dyn QuotaRepo>) -> Arc<Self> {
        Arc::new(Self {
            inner,
            fault: std::sync::Mutex::new(Fault::None),
        })
    }

    fn arm(&self, fault: Fault) {
        *self.fault.lock().unwrap() = fault;
    }

    async fn after_storing(
        &self,
        stored: Result<ExternalRevision, QuotaRepoError>,
    ) -> Result<ExternalRevision, QuotaRepoError> {
        let fault = std::mem::replace(&mut *self.fault.lock().unwrap(), Fault::None);
        match fault {
            Fault::None => stored,
            Fault::FailAfterStoring => {
                stored?;
                Err(QuotaRepoError::InternalError(anyhow::anyhow!(
                    "timed out waiting for the store"
                )))
            }
            Fault::HangAfterStoring => {
                stored?;
                std::future::pending().await
            }
        }
    }
}

#[async_trait]
impl QuotaRepo for Unreliable {
    async fn save_lease_change(
        &self,
        resource: &QuotaResourceRecord,
        previous_revision: ExternalRevision,
        lease: &QuotaLeaseRecord,
        expired_pods: &[(Blob<IpAddr>, i32)],
    ) -> Result<ExternalRevision, QuotaRepoError> {
        let stored = self
            .inner
            .save_lease_change(resource, previous_revision, lease, expired_pods)
            .await;
        self.after_storing(stored).await
    }

    async fn save_lease_release(
        &self,
        resource: &QuotaResourceRecord,
        previous_revision: ExternalRevision,
        pod_ip: Blob<IpAddr>,
        pod_port: i32,
    ) -> Result<ExternalRevision, QuotaRepoError> {
        let stored = self
            .inner
            .save_lease_release(resource, previous_revision, pod_ip, pod_port)
            .await;
        self.after_storing(stored).await
    }

    async fn save_resource(
        &self,
        record: &QuotaResourceRecord,
        previous_revision: ExternalRevision,
    ) -> Result<ExternalRevision, QuotaRepoError> {
        let stored = self.inner.save_resource(record, previous_revision).await;
        self.after_storing(stored).await
    }

    async fn delete_resource_and_leases(
        &self,
        resource_definition_id: ResourceDefinitionId,
    ) -> Result<(), QuotaRepoError> {
        self.inner
            .delete_resource_and_leases(resource_definition_id)
            .await
    }

    async fn get_all(&self) -> Result<StoredQuotaState, QuotaRepoError> {
        self.inner.get_all().await
    }

    async fn get_resource(
        &self,
        resource_definition_id: ResourceDefinitionId,
    ) -> Result<Option<(StoredQuotaResource, Vec<QuotaLeaseRecord>)>, QuotaRepoError> {
        self.inner.get_resource(resource_definition_id).await
    }

    async fn delete_leases_for_resource(
        &self,
        resource_definition_id: ResourceDefinitionId,
    ) -> Result<(), QuotaRepoError> {
        self.inner
            .delete_leases_for_resource(resource_definition_id)
            .await
    }
}

#[test]
#[tracing::instrument]
// A write can land in the store and still come back as a failure: etcd committed it, but the
// reply was lost to a timeout. The service cannot tell which happened, so the next operation on
// the resource must reload it rather than guard its write with a revision the store has passed.
async fn a_write_that_landed_but_reported_failure_does_not_block_the_next_one(
    #[dimension(persistence)] persistence: &Arc<dyn GetRoutingTablePersistence>,
) {
    let store = persistence.new_store().await;
    let reader = store.quota_repo().await;
    let definition = definition();
    let unreliable = Unreliable::new(store.quota_repo().await);
    let quota = quota_service(unreliable.clone(), &definition, Duration::from_secs(60));

    let (acquired, allocated) = acquire(&quota, &definition, pod(1)).await;
    unreliable.arm(Fault::FailAfterStoring);
    let failed = quota
        .renew_lease(definition.id, pod(1), acquired, allocated, vec![])
        .await;
    assert!(
        matches!(failed, Err(QuotaError::InternalError(_))),
        "the renewal should have been reported as failed, got {failed:?}"
    );
    assert_eq!(
        stored_epoch(&reader, &definition, pod(1)).await,
        acquired.0 + 2,
        "the renewal should have landed in the store despite the reported failure"
    );

    // Guarded by the revision from before the failure, this would be refused as a conflict.
    acquire(&quota, &definition, pod(2)).await;
    assert_eq!(
        stored_pods(&reader, &definition).await,
        BTreeSet::from([pod(1), pod(2)])
    );
    assert_eq!(stored_total(&reader, &definition).await, SLOTS);
}

#[test]
#[tracing::instrument]
// A request dropped while its write is in flight - its caller gave up, or the RPC deadline passed -
// stops the service between the store committing and the service recording the new revision.
// Nothing reports the outcome, so the next operation must not trust the old revision either.
async fn a_write_cancelled_after_it_landed_does_not_block_the_next_one(
    #[dimension(persistence)] persistence: &Arc<dyn GetRoutingTablePersistence>,
) {
    let store = persistence.new_store().await;
    let reader = store.quota_repo().await;
    let definition = definition();
    let unreliable = Unreliable::new(store.quota_repo().await);
    let quota = quota_service(unreliable.clone(), &definition, Duration::from_secs(60));

    let (acquired, allocated) = acquire(&quota, &definition, pod(1)).await;
    unreliable.arm(Fault::HangAfterStoring);
    let cancelled = tokio::time::timeout(
        Duration::from_millis(500),
        quota.renew_lease(definition.id, pod(1), acquired, allocated, vec![]),
    )
    .await;
    assert!(
        cancelled.is_err(),
        "the renewal should still have been waiting when it was dropped"
    );
    assert_eq!(
        stored_epoch(&reader, &definition, pod(1)).await,
        acquired.0 + 2,
        "the renewal should have landed in the store before it was dropped"
    );

    acquire(&quota, &definition, pod(2)).await;
    assert_eq!(
        stored_pods(&reader, &definition).await,
        BTreeSet::from([pod(1), pod(2)])
    );
    assert_eq!(stored_total(&reader, &definition).await, SLOTS);
}

#[test]
#[tracing::instrument]
// What distributed mode exists for: a newly elected leader restores the quota state the previous
// one stored, and the executors carry on renewing and releasing the leases they already hold.
async fn a_new_leader_serves_the_leases_the_previous_one_stored(
    #[dimension(persistence)] persistence: &Arc<dyn GetRoutingTablePersistence>,
) {
    let store = persistence.new_store().await;
    let definition = definition();

    let previous = quota_service(
        store.quota_repo().await,
        &definition,
        Duration::from_secs(60),
    );
    let (first, first_allocated) = acquire(&previous, &definition, pod(1)).await;
    let (second, second_allocated) = acquire(&previous, &definition, pod(2)).await;

    let next = quota_service(
        store.quota_repo().await,
        &definition,
        Duration::from_secs(60),
    );
    next.restore_state()
        .await
        .expect("restoring the stored quota state should succeed");

    let renewed = next
        .renew_lease(definition.id, pod(1), first, first_allocated, vec![])
        .await
        .expect("a lease granted by the previous leader should renew on the new one");
    assert_eq!(granted(&renewed).0.0, first.0 + 1);
    next.release_lease(definition.id, pod(2), second, second_allocated)
        .await
        .expect("a lease granted by the previous leader should release on the new one");
    // Guarded by the revision restored from the store, so it would be refused as a conflict if
    // the new leader had started from anything else.
    acquire(&next, &definition, pod(3)).await;

    let reader = store.quota_repo().await;
    assert_eq!(
        stored_pods(&reader, &definition).await,
        BTreeSet::from([pod(1), pod(3)])
    );
    assert_eq!(stored_total(&reader, &definition).await, SLOTS);
}

#[test]
#[tracing::instrument]
// An executor re-acquiring after its own lease lapsed is reclaimed and granted again by one
// change. That change must store the new lease and not delete it: etcd refuses a transaction
// that writes and deletes the same key, and the SQL store would drop the lease it just wrote.
async fn a_lease_acquired_again_after_it_expired_is_stored(
    #[dimension(persistence)] persistence: &Arc<dyn GetRoutingTablePersistence>,
) {
    let store = persistence.new_store().await;
    let reader = store.quota_repo().await;
    let definition = definition();
    let quota = quota_service(
        store.quota_repo().await,
        &definition,
        Duration::from_secs(1),
    );

    acquire(&quota, &definition, pod(1)).await;
    tokio::time::sleep(Duration::from_millis(1200)).await;
    let (again, _) = acquire(&quota, &definition, pod(1)).await;

    assert_eq!(
        stored_pods(&reader, &definition).await,
        BTreeSet::from([pod(1)]),
        "the lease granted by the re-acquisition was not stored"
    );
    assert_eq!(
        stored_epoch(&reader, &definition, pod(1)).await,
        again.0 + 1
    );
    assert_eq!(stored_total(&reader, &definition).await, SLOTS);
}

#[test]
#[tracing::instrument]
// 130 leases that expired together: reclaiming them in one write would need 132 operations, over
// etcd's limit of 128 per transaction, and the resource could then never be written again. The
// service reclaims them over two writes instead, and neither loses nor double-counts a slot.
async fn a_mass_expiry_is_reclaimed_across_writes(
    #[dimension(persistence)] persistence: &Arc<dyn GetRoutingTablePersistence>,
) {
    const EXPIRED: usize = 130;

    let store = persistence.new_store().await;
    let seeder = store.quota_repo().await;
    let definition = definition();

    let mut revision = NO_REVISION;
    for n in 0..EXPIRED {
        revision = seeder
            .save_lease_change(
                &resource(&definition, SLOTS - (n as u64 + 1)),
                revision,
                &lease(&definition, pod(n), 1, a_minute_ago()),
                &[],
            )
            .await
            .expect("seeding an expired lease should succeed");
    }

    let quota = quota_service(
        store.quota_repo().await,
        &definition,
        Duration::from_secs(60),
    );
    quota
        .restore_state()
        .await
        .expect("restoring the seeded quota state should succeed");

    let reader = store.quota_repo().await;
    acquire(&quota, &definition, pod(1000)).await;
    assert_eq!(
        stored_pods(&reader, &definition).await.len(),
        EXPIRED - 120 + 1,
        "the first write should reclaim exactly 120 expired leases"
    );

    acquire(&quota, &definition, pod(1001)).await;
    assert_eq!(
        stored_pods(&reader, &definition).await,
        BTreeSet::from([pod(1000), pod(1001)]),
        "the second write should reclaim the rest"
    );
    assert_eq!(stored_total(&reader, &definition).await, SLOTS);
}

// -- etcd only ----------------------------------------------------------------------------------

#[test]
#[tracing::instrument]
// A replica that lost the leadership may still be answering quota requests until it notices. Its
// writes must be refused - naming lost leadership, not a conflict - or it would overwrite the
// state its successor restored.
async fn a_quota_write_after_the_leader_key_is_deleted_is_refused_as_leadership_lost(
    etcd: &Arc<DockerEtcd>,
) {
    let store = EtcdRoutingTablePersistenceFactory { etcd: etcd.clone() }
        .new_etcd_store()
        .await;
    let (leader_key, fence) = store.mint_leader_key().await;
    let repo = store.quota_repo_with(fence).await;
    let definition = definition();

    let revision = repo
        .save_lease_change(
            &resource(&definition, SLOTS - 1),
            NO_REVISION,
            &lease(&definition, pod(1), 1, in_a_minute()),
            &[],
        )
        .await
        .expect("a write under a held leadership should succeed");

    store
        .kv()
        .await
        .delete(leader_key, None)
        .await
        .expect("deleting the leader key should succeed");

    assert_refused_as_leadership_lost(&repo, &definition, revision).await;
}

#[test]
#[tracing::instrument]
// The leader key existing is not enough: a recreated key belongs to a later campaign.
async fn a_quota_write_after_the_leader_key_is_recreated_is_refused_as_leadership_lost(
    etcd: &Arc<DockerEtcd>,
) {
    let store = EtcdRoutingTablePersistenceFactory { etcd: etcd.clone() }
        .new_etcd_store()
        .await;
    let (leader_key, fence) = store.mint_leader_key().await;
    let repo = store.quota_repo_with(fence).await;
    let definition = definition();

    let revision = repo
        .save_lease_change(
            &resource(&definition, SLOTS - 1),
            NO_REVISION,
            &lease(&definition, pod(1), 1, in_a_minute()),
            &[],
        )
        .await
        .expect("a write under a held leadership should succeed");

    let mut kv = store.kv().await;
    kv.delete(leader_key.clone(), None)
        .await
        .expect("deleting the leader key should succeed");
    kv.put(leader_key, "a later campaign", None)
        .await
        .expect("recreating the leader key should succeed");

    assert_refused_as_leadership_lost(&repo, &definition, revision).await;
}

async fn assert_refused_as_leadership_lost(
    repo: &Arc<dyn QuotaRepo>,
    definition: &ResourceDefinition,
    revision: ExternalRevision,
) {
    let lost = |result: Result<(), QuotaRepoError>| {
        matches!(result, Err(QuotaRepoError::LeadershipLost { .. }))
    };
    let lost_write = |result: Result<ExternalRevision, QuotaRepoError>| lost(result.map(|_| ()));

    assert!(lost_write(
        repo.save_resource(&resource(definition, 1), revision).await
    ));
    assert!(lost_write(
        repo.save_lease_change(
            &resource(definition, 1),
            revision,
            &lease(definition, pod(2), 1, in_a_minute()),
            &[],
        )
        .await
    ));
    assert!(lost_write(
        repo.save_lease_release(
            &resource(definition, 1),
            revision,
            Blob::new(pod(1).ip),
            9000
        )
        .await
    ));
    assert!(lost(repo.delete_leases_for_resource(definition.id).await));
    assert!(lost(repo.delete_resource_and_leases(definition.id).await));

    let stored = stored_resource(repo, definition)
        .await
        .expect("the resource should still be stored");
    assert_eq!(
        stored.revision, revision,
        "a refused write reached the store"
    );
    assert_eq!(
        stored_pods(repo, definition).await,
        BTreeSet::from([pod(1)])
    );
}

#[test]
#[tracing::instrument]
// Reads come back a page at a time. More keys than fit in one page must all come back, each exactly
// once - and so must pages whose keys fit the page size but whose values do not fit etcd-client's
// 4 MiB response limit: here a first page of 1,000 leases of about 6 KiB each, carrying long
// pending-reservation lists.
async fn reads_page_through_more_keys_and_bytes_than_fit_in_one_response(etcd: &Arc<DockerEtcd>) {
    const LEASES: usize = 1001;
    const RESERVATIONS: usize = 400;

    let store = EtcdRoutingTablePersistenceFactory { etcd: etcd.clone() }
        .new_etcd_store()
        .await;
    let repo = store.quota_repo().await;
    let definition = definition();

    let mut revision = NO_REVISION;
    for n in 0..LEASES {
        let mut lease = lease(&definition, pod(n), 1, in_a_minute());
        lease.pending_reservations = Blob::new(
            (0..RESERVATIONS)
                .map(|i| PendingReservation {
                    amount: i as u64,
                    priority: 1.0,
                })
                .collect(),
        );
        revision = repo
            .save_lease_change(&resource(&definition, SLOTS), revision, &lease, &[])
            .await
            .expect("storing a lease should succeed");
    }

    let leases = repo
        .get_all()
        .await
        .expect("reading the leases back should succeed")
        .leases;
    let pods: BTreeSet<Pod> = leases
        .iter()
        .map(|lease| Pod {
            ip: *lease.pod_ip.value(),
            port: 9000,
        })
        .collect();
    assert_eq!(leases.len(), LEASES, "a lease was read twice or not at all");
    assert_eq!(pods, (0..LEASES).map(pod).collect());
    assert!(
        leases
            .iter()
            .all(|lease| lease.pending_reservations.value().len() == RESERVATIONS),
        "a lease came back without its pending reservations"
    );
    assert_eq!(stored_resources(&repo).await.len(), 1);
}

/// Stores `definition` with leases for pods 1, 2 and 3 and returns the revision it is stored at.
/// Read two keys a page, the leases of pods 1 and 2 come first; pod 3's lease and the resource's
/// `state` key follow on the second page.
async fn three_leases(
    repo: &Arc<dyn QuotaRepo>,
    definition: &ResourceDefinition,
) -> ExternalRevision {
    let mut revision = NO_REVISION;
    for n in 1..=3 {
        revision = repo
            .save_lease_change(
                &resource(definition, SLOTS - n as u64),
                revision,
                &lease(definition, pod(n), 1, in_a_minute()),
                &[],
            )
            .await
            .expect("storing a lease should succeed");
    }
    revision
}

/// A reader of two keys a page that, the first time it is between pages, commits a change through
/// `writer`: pod 1's lease reclaimed, pod 2's renewed, the balance changed. That is the write a
/// cancelled request can still commit while the next operation is reloading the resource. With
/// `compact`, it also compacts the history up to that write. Returns the reader and where the write
/// was stored, once it has been.
fn reader_with_a_write_between_pages(
    client: etcd_client::Client,
    fence: golem_shard_manager::LeaderFence,
    read_retry: ReadRetry,
    writer: Arc<dyn QuotaRepo>,
    definition: &ResourceDefinition,
    guarded_by: ExternalRevision,
    compact: Option<etcd_client::Client>,
) -> (EtcdQuotaRepo, Arc<AtomicI64>) {
    let fired = Arc::new(AtomicBool::new(false));
    let written = Arc::new(AtomicI64::new(NO_REVISION));
    let definition = definition.clone();
    let written_at = written.clone();
    let reader = EtcdQuotaRepo::new(client, fence, read_retry).with_read_pages(2, move || {
        let (writer, definition, fired, written_at, compact) = (
            writer.clone(),
            definition.clone(),
            fired.clone(),
            written_at.clone(),
            compact.clone(),
        );
        Box::pin(async move {
            if fired.swap(true, Ordering::SeqCst) {
                return;
            }
            let revision = writer
                .save_lease_change(
                    &resource(&definition, 150),
                    guarded_by,
                    &lease(&definition, pod(2), 2, in_a_minute()),
                    &[expired(pod(1))],
                )
                .await
                .expect("the write between pages should commit");
            written_at.store(revision, Ordering::SeqCst);
            if let Some(client) = compact {
                client
                    .kv_client()
                    .compact(revision, None)
                    .await
                    .expect("compacting up to the write should succeed");
            }
        })
    });
    (reader, written)
}

#[test]
#[tracing::instrument]
// A read spanning several pages must see one revision. Were later pages read at the latest one, a
// write committing between pages would pair the leases from before it with the resource from
// after it: here pod 1's reclaimed lease would reappear next to the balance that already returned
// its allocation, and the next write - guarded by the new revision - would store that mix.
async fn a_read_spanning_pages_sees_one_revision(etcd: &Arc<DockerEtcd>) {
    let store = EtcdRoutingTablePersistenceFactory { etcd: etcd.clone() }
        .new_etcd_store()
        .await;
    let (_, fence) = store.mint_leader_key().await;
    let writer = store.quota_repo_with(fence.clone()).await;
    let definition = definition();
    let before = three_leases(&writer, &definition).await;

    let (reader, written) = reader_with_a_write_between_pages(
        store.client().await,
        fence,
        store.read_retry(),
        writer,
        &definition,
        before,
        None,
    );
    let (resource, leases) = reader
        .get_resource(definition.id)
        .await
        .expect("the read should succeed")
        .expect("the resource should be stored");

    assert_ne!(
        written.load(Ordering::SeqCst),
        NO_REVISION,
        "the read should have spanned more than one page, with the write between them"
    );
    assert_eq!(resource.revision, before, "the read mixed two revisions");
    assert_eq!(resource.record.remaining.get(), SLOTS - 3);
    assert_eq!(
        leases
            .iter()
            .map(|lease| *lease.pod_ip.value())
            .collect::<BTreeSet<_>>(),
        BTreeSet::from([pod(1).ip, pod(2).ip, pod(3).ip])
    );
}

#[test]
#[tracing::instrument]
// A read pinned to a revision that is compacted before its last page arrives cannot finish at that
// revision. It must start over and return one consistent, newer view, rather than fail the restore
// or the reload that needed it.
async fn a_read_whose_revision_is_compacted_starts_over(etcd: &Arc<DockerEtcd>) {
    let store = EtcdRoutingTablePersistenceFactory { etcd: etcd.clone() }
        .new_etcd_store()
        .await;
    let (_, fence) = store.mint_leader_key().await;
    let writer = store.quota_repo_with(fence.clone()).await;
    let definition = definition();
    let before = three_leases(&writer, &definition).await;

    let (reader, written) = reader_with_a_write_between_pages(
        store.client().await,
        fence,
        store.read_retry(),
        writer,
        &definition,
        before,
        Some(store.client().await),
    );
    let (resource, leases) = reader
        .get_resource(definition.id)
        .await
        .expect("the read should start over rather than fail")
        .expect("the resource should be stored");

    let after = written.load(Ordering::SeqCst);
    assert_ne!(after, NO_REVISION, "the write between pages never ran");
    assert_eq!(
        resource.revision, after,
        "the restarted read should see the newer state"
    );
    assert_eq!(resource.record.remaining.get(), 150);
    assert_eq!(
        leases
            .iter()
            .map(|lease| *lease.pod_ip.value())
            .collect::<BTreeSet<_>>(),
        BTreeSet::from([pod(2).ip, pod(3).ip])
    );
}
