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

use super::quota_lease::QuotaLease;
use super::quota_repo::{QuotaLeaseRecord, QuotaRepo, QuotaRepoError};
use super::quota_state::{PodLease, QuotaState};
use super::resource_definition_fetcher::{FetchError, ResourceDefinitionFetcher};
use crate::config::QuotaServiceConfig;
use crate::sharding::persistence::{ExternalRevision, NO_REVISION};
use anyhow::anyhow;
use chrono::Utc;
use golem_common::model::Pod;
use golem_common::model::environment::EnvironmentId;
use golem_common::model::quota::LeaseEpoch;
use golem_common::model::quota::{ResourceDefinitionId, ResourceName};
use golem_common::{IntoAnyhow, SafeDisplay};
use golem_service_base::model::quota_lease::PendingReservation;
use golem_service_base::repo::Blob;
use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Mutex, RwLock};
use tracing::{debug, info, warn};

#[derive(Debug, thiserror::Error)]
pub enum QuotaError {
    #[error("No active lease for pod on resource {resource_definition_id}")]
    LeaseNotFound {
        resource_definition_id: ResourceDefinitionId,
    },
    #[error("Stale epoch {provided} for resource {resource_definition_id} (current: {current})")]
    StaleEpoch {
        resource_definition_id: ResourceDefinitionId,
        provided: LeaseEpoch,
        current: LeaseEpoch,
    },
    #[error(
        "This shard manager is no longer the leader (election key {leader_key} at creation \
         revision {create_revision}), so it cannot record quota changes"
    )]
    LeadershipLost {
        leader_key: String,
        create_revision: i64,
    },
    #[error(transparent)]
    InternalError(#[from] anyhow::Error),
}

impl SafeDisplay for QuotaError {
    fn to_safe_string(&self) -> String {
        match self {
            Self::LeaseNotFound { .. } => self.to_string(),
            Self::StaleEpoch { .. } => self.to_string(),
            Self::LeadershipLost { .. } => "This shard manager is no longer the leader".to_string(),
            Self::InternalError(_) => "Internal error".to_string(),
        }
    }
}

golem_common::error_forwarding!(QuotaError);

impl From<QuotaRepoError> for QuotaError {
    fn from(err: QuotaRepoError) -> Self {
        match err {
            QuotaRepoError::LeadershipLost {
                leader_key,
                create_revision,
            } => QuotaError::LeadershipLost {
                leader_key,
                create_revision,
            },
            other => QuotaError::InternalError(other.into_anyhow()),
        }
    }
}

impl From<FetchError> for QuotaError {
    fn from(err: FetchError) -> Self {
        match err {
            FetchError::NotFound => {
                QuotaError::InternalError(anyhow::anyhow!("unexpected NotFound from fetcher"))
            }
            FetchError::InternalError(e) => QuotaError::InternalError(anyhow::anyhow!(e)),
        }
    }
}

/// One resource's quota state, and the revision its stored copy is at.
struct Entry {
    /// `None` once the resource was deleted from the store, until the entry is removed from the
    /// map. `get_entry_handle` treats such an entry as absent.
    state: RwLock<Option<QuotaState>>,
    /// The revision the store last reported for this resource; the next write is guarded by it.
    /// Locked only while `state` is write-locked, so the lock order is `state`, then this. Only a
    /// write the store accepted advances it.
    stored_revision: Mutex<ExternalRevision>,
}

impl Entry {
    fn new(state: QuotaState, stored_revision: ExternalRevision) -> EntryHandle {
        Arc::new(Self {
            state: RwLock::new(Some(state)),
            stored_revision: Mutex::new(stored_revision),
        })
    }
}

type EntryHandle = Arc<Entry>;

pub struct QuotaService {
    entries: scc::HashMap<ResourceDefinitionId, EntryHandle>,
    fetcher: Arc<dyn ResourceDefinitionFetcher>,
    repo: Arc<dyn QuotaRepo>,
    ttl: Duration,
    lease_duration: Duration,
    min_executors: u64,
}

impl QuotaService {
    pub fn new(
        config: QuotaServiceConfig,
        fetcher: Arc<dyn ResourceDefinitionFetcher>,
        repo: Arc<dyn QuotaRepo>,
    ) -> Arc<Self> {
        assert!(config.min_executors > 0, "min_executors must be at least 1");
        Arc::new(Self {
            entries: scc::HashMap::new(),
            fetcher,
            repo,
            ttl: config.definition_staleness_ttl,
            lease_duration: config.lease_duration,
            min_executors: config.min_executors,
        })
    }

    /// Restores quota state from the store on startup.
    /// Must be called before serving any requests.
    pub async fn restore_state(&self) -> Result<(), QuotaError> {
        let resources = self
            .repo
            .get_all_resources()
            .await
            .map_err(|e| QuotaError::InternalError(e.into()))?;
        let all_leases = self
            .repo
            .get_all_leases()
            .await
            .map_err(|e| QuotaError::InternalError(e.into()))?;

        let mut leases_by_resource: HashMap<uuid::Uuid, Vec<QuotaLeaseRecord>> = HashMap::new();
        for lease in all_leases {
            leases_by_resource
                .entry(lease.resource_definition_id)
                .or_default()
                .push(lease);
        }

        for stored in resources {
            let resource_record = stored.record;
            let id = ResourceDefinitionId(resource_record.resource_definition_id);
            let definition = resource_record.definition.into_value();

            let mut pod_leases = HashMap::new();
            if let Some(lease_records) =
                leases_by_resource.remove(&resource_record.resource_definition_id)
            {
                for lr in lease_records {
                    let pod = Pod {
                        ip: lr.pod_ip.into_value(),
                        port: lr
                            .pod_port
                            .try_into()
                            .map_err(|_| anyhow!("Failed deserializing port"))?,
                    };
                    pod_leases.insert(
                        pod,
                        PodLease {
                            epoch: LeaseEpoch(lr.epoch.into()),
                            allocated: lr.allocated.into(),
                            granted_at: lr.granted_at.into(),
                            expires_at: lr.expires_at.into(),
                            pending_reservations: lr.pending_reservations.into_value(),
                        },
                    );
                }
            }

            let state = QuotaState::from_persisted(
                definition,
                resource_record.remaining.into(),
                resource_record.last_refilled_at.into(),
                resource_record.last_refreshed_at.into(),
                pod_leases,
            );

            let _ = self
                .entries
                .insert_async(id, Entry::new(state, stored.revision))
                .await;

            info!(%id, "restored quota resource from the store");
        }

        for (orphaned_resource_id, _) in leases_by_resource {
            let id = ResourceDefinitionId(orphaned_resource_id);
            warn!(%id, "cleaning up orphaned leases for deleted resource");
            let _ = self.repo.delete_leases_for_resource(id).await;
        }

        Ok(())
    }

    pub async fn acquire_lease(
        &self,
        environment_id: EnvironmentId,
        name: ResourceName,
        pod: Pod,
    ) -> Result<QuotaLease, QuotaError> {
        let definition = match self
            .fetcher
            .resolve_by_name(environment_id, name.clone())
            .await
        {
            Ok(def) => Some(def),
            Err(FetchError::NotFound) => None,
            Err(other) => return Err(other.into()),
        };

        match definition {
            Some(definition) => {
                let id = definition.id;
                self.ensure_entry(&definition).await;
                self.refresh_if_stale(id).await;

                let handle = self
                    .get_entry_handle(id)
                    .await
                    .expect("entry was just ensured");

                let mut guard = handle.state.write().await;
                let state = guard.as_mut().ok_or(QuotaError::LeaseNotFound {
                    resource_definition_id: id,
                })?;
                let snapshot = state.clone();
                let result = state.acquire_lease(pod, self.lease_duration, self.min_executors)?;

                let lease = QuotaLease::Bounded {
                    resource_definition_id: id,
                    pod,
                    epoch: result.epoch,
                    allocated_amount: result.allocated_amount,
                    expires_at: result.expires_at,
                    resource_limit: state.definition.limit.clone(),
                    enforcement_action: state.definition.enforcement_action,
                    total_available_amount: result.total_available_amount,
                };

                let mut stored_revision = handle.stored_revision.lock().await;
                match self
                    .persist_after_lease_change(state, *stored_revision, &pod, &result.expired)
                    .await
                {
                    Ok(revision) => *stored_revision = revision,
                    Err(e) => {
                        log_on_failed_persistence(&e);
                        *state = snapshot;
                        return Err(e.into());
                    }
                }

                Ok(lease)
            }
            None => Ok(self.unlimited_lease(pod)),
        }
    }

    pub async fn renew_lease(
        &self,
        resource_definition_id: ResourceDefinitionId,
        pod: Pod,
        epoch: LeaseEpoch,
        unused: u64,
        pending_reservations: Vec<PendingReservation>,
    ) -> Result<QuotaLease, QuotaError> {
        self.refresh_if_stale(resource_definition_id).await;

        let handle = match self.get_entry_handle(resource_definition_id).await {
            Some(h) => h,
            None => {
                return Err(QuotaError::LeaseNotFound {
                    resource_definition_id,
                });
            }
        };

        let mut guard = handle.state.write().await;
        let state = guard.as_mut().ok_or(QuotaError::LeaseNotFound {
            resource_definition_id,
        })?;
        let snapshot = state.clone();
        let result = state.renew_lease(
            &pod,
            epoch,
            unused,
            self.lease_duration,
            self.min_executors,
            pending_reservations,
        )?;

        let mut stored_revision = handle.stored_revision.lock().await;
        match self
            .persist_after_lease_change(state, *stored_revision, &pod, &result.expired)
            .await
        {
            Ok(revision) => *stored_revision = revision,
            Err(e) => {
                log_on_failed_persistence(&e);
                *state = snapshot;
                return Err(e.into());
            }
        }

        let lease = QuotaLease::Bounded {
            resource_definition_id,
            pod,
            epoch: result.new_epoch,
            allocated_amount: result.allocated_amount,
            expires_at: result.expires_at,
            resource_limit: state.definition.limit.clone(),
            enforcement_action: state.definition.enforcement_action,
            total_available_amount: result.total_available_amount,
        };

        Ok(lease)
    }

    /// Renew multiple leases in one call.  Results are in the same order as
    /// the input entries.  Each entry is processed independently — failure of
    /// one does not affect others.
    pub async fn batch_renew_leases(
        &self,
        renewals: Vec<(
            ResourceDefinitionId,
            Pod,
            LeaseEpoch,
            u64,
            Vec<PendingReservation>,
        )>,
    ) -> Vec<Result<QuotaLease, QuotaError>> {
        let mut results = Vec::with_capacity(renewals.len());
        for (rid, pod, epoch, unused, pending) in renewals {
            results.push(self.renew_lease(rid, pod, epoch, unused, pending).await);
        }
        results
    }

    pub async fn release_lease(
        &self,
        resource_definition_id: ResourceDefinitionId,
        pod: Pod,
        epoch: LeaseEpoch,
        unused: u64,
    ) -> Result<(), QuotaError> {
        let handle = match self.get_entry_handle(resource_definition_id).await {
            Some(h) => h,
            None => {
                return Err(QuotaError::LeaseNotFound {
                    resource_definition_id,
                });
            }
        };

        let mut guard = handle.state.write().await;
        let state = guard.as_mut().ok_or(QuotaError::LeaseNotFound {
            resource_definition_id,
        })?;
        let snapshot = state.clone();
        state.release_lease(&pod, epoch, unused)?;

        let mut stored_revision = handle.stored_revision.lock().await;
        match self
            .persist_after_lease_release(state, *stored_revision, &pod)
            .await
        {
            Ok(revision) => *stored_revision = revision,
            Err(e) => {
                log_on_failed_persistence(&e);
                *state = snapshot;
                return Err(e.into());
            }
        }
        Ok(())
    }

    pub async fn on_resource_definition_changed(
        &self,
        resource_definition_id: ResourceDefinitionId,
    ) {
        if self
            .get_entry_handle(resource_definition_id)
            .await
            .is_some()
        {
            self.refresh_entry(resource_definition_id).await;
        }
    }

    pub async fn on_cursor_expired(&self) {
        let mut ids = Vec::new();
        self.entries
            .iter_async(|id, _| {
                ids.push(*id);
                true
            })
            .await;

        for id in ids {
            self.refresh_entry(id).await;
        }
    }

    fn unlimited_lease(&self, pod: Pod) -> QuotaLease {
        QuotaLease::Unlimited {
            pod,
            expires_at: Utc::now() + self.lease_duration,
        }
    }

    async fn persist_after_lease_change(
        &self,
        state: &QuotaState,
        previous_revision: ExternalRevision,
        pod: &Pod,
        expired_pods: &[Pod],
    ) -> Result<ExternalRevision, QuotaRepoError> {
        let resource_record = state.to_resource_record();

        let lease_record = state
            .to_lease_record(pod)
            .ok_or_else(|| anyhow::anyhow!("pod lease not found after mutation"))?;

        // A pod re-acquiring after its own lease expired is both reclaimed and granted a fresh
        // lease by the same change. Its lease is written, so it must not also be deleted: the SQL
        // store would drop the lease it just wrote, and etcd refuses a transaction that writes and
        // deletes the same key.
        let expired: Vec<(Blob<IpAddr>, i32)> = expired_pods
            .iter()
            .filter(|expired| *expired != pod)
            .map(|p| (Blob::new(p.ip), p.port.into()))
            .collect();

        self.repo
            .save_lease_change(&resource_record, previous_revision, &lease_record, &expired)
            .await
    }

    async fn persist_after_lease_release(
        &self,
        state: &QuotaState,
        previous_revision: ExternalRevision,
        pod: &Pod,
    ) -> Result<ExternalRevision, QuotaRepoError> {
        let resource_record = state.to_resource_record();
        self.repo
            .save_lease_release(
                &resource_record,
                previous_revision,
                Blob::new(pod.ip),
                pod.port.into(),
            )
            .await
    }

    async fn persist_resource(
        &self,
        state: &QuotaState,
        previous_revision: ExternalRevision,
    ) -> Result<ExternalRevision, QuotaRepoError> {
        let record = state.to_resource_record();
        self.repo.save_resource(&record, previous_revision).await
    }

    async fn refresh_if_stale(&self, id: ResourceDefinitionId) {
        let is_stale = self
            .entries
            .read_async(&id, |_, handle| {
                handle
                    .state
                    .try_read()
                    .ok()
                    .and_then(|guard| guard.as_ref().map(|s| s.is_stale(self.ttl)))
                    .unwrap_or(false)
            })
            .await
            .unwrap_or(false);

        if is_stale {
            self.refresh_entry(id).await;
        }
    }

    async fn get_entry_handle(&self, id: ResourceDefinitionId) -> Option<EntryHandle> {
        let handle = self
            .entries
            .read_async(&id, |_, handle| handle.clone())
            .await?;
        // Return None for tombstoned entries.
        if handle.state.read().await.is_none() {
            return None;
        }
        Some(handle)
    }

    async fn ensure_entry(&self, definition: &golem_common::model::quota::ResourceDefinition) {
        let _ = self
            .entries
            .entry_async(definition.id)
            .await
            .or_insert_with(|| Entry::new(QuotaState::new(definition.clone()), NO_REVISION));
    }

    async fn refresh_entry(&self, id: ResourceDefinitionId) {
        let handle = match self.get_entry_handle(id).await {
            Some(h) => h,
            None => return,
        };

        match self.fetcher.fetch_by_id(id).await {
            Ok(definition) => {
                debug_assert_eq!(definition.id, id);
                let mut guard = handle.state.write().await;
                let state = match guard.as_mut() {
                    Some(s) => s,
                    None => return,
                };
                let snapshot = state.clone();
                state.update_definition(definition);
                let mut stored_revision = handle.stored_revision.lock().await;
                match self.persist_resource(state, *stored_revision).await {
                    Ok(revision) => *stored_revision = revision,
                    Err(e) => {
                        log_on_failed_persistence(&e);
                        *state = snapshot;
                    }
                }
            }
            Err(FetchError::NotFound) => {
                debug!(%id, "resource definition no longer exists, removing");
                // Tombstone while holding the lock — other threads will see None
                // and treat it as non-existent. Then remove from map.
                let mut guard = handle.state.write().await;
                // Delete from the store first. If this fails, keep in-memory state
                // so they stay consistent. Staleness refresh will retry later.
                if let Err(e) = self.repo.delete_resource_and_leases(id).await {
                    warn!(error = %e, %id, "failed to delete resource from the store, keeping in-memory state");
                    return;
                }
                *guard = None;
                drop(guard);
                drop(handle);
                // Remove tombstone
                self.entries.remove_async(&id).await;
            }
            Err(err) => {
                warn!(%id, error = %err, "failed to refresh resource definition, keeping stale entry");
            }
        }
    }
}

fn log_on_failed_persistence(e: &QuotaRepoError) {
    match e {
        QuotaRepoError::ConcurrentModification => {
            warn!(error = %e, "Revision conflict, another process might have written to the database. Rolling back")
        }
        QuotaRepoError::LeadershipLost { .. } => {
            warn!(error = %e, "Leadership lost, the quota change was not recorded. Rolling back")
        }
        QuotaRepoError::InternalError(_) => {
            warn!(error = %e, "Persisting state failed, rolling back")
        }
    }
}
