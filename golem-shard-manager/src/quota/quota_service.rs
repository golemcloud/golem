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
use anyhow::anyhow;
use chrono::Utc;
use golem_common::SafeDisplay;
use golem_common::model::Pod;
use golem_common::model::environment::EnvironmentId;
use golem_common::model::quota::LeaseEpoch;
use golem_common::model::quota::{ResourceDefinitionId, ResourceName};
use golem_service_base::model::quota_lease::PendingReservation;
use sqlx::types::Json;
use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
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
    #[error(transparent)]
    InternalError(#[from] anyhow::Error),
}

impl SafeDisplay for QuotaError {
    fn to_safe_string(&self) -> String {
        match self {
            Self::LeaseNotFound { .. } => self.to_string(),
            Self::StaleEpoch { .. } => self.to_string(),
            Self::InternalError(_) => "Internal error".to_string(),
        }
    }
}

golem_common::error_forwarding!(QuotaError, QuotaRepoError);

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

/// None = tombstoned (DB-deleted, pending removal from map).
/// get_entry_handle returns None when it sees a tombstone, treating it
/// as if the entry doesn't exist.
type EntryHandle = Arc<RwLock<Option<QuotaState>>>;

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

    /// Restores quota state from the database on startup.
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

        for resource_record in resources {
            let id = ResourceDefinitionId(resource_record.resource_definition_id);
            let definition = resource_record.definition.into_value();

            let mut pod_leases = HashMap::new();
            if let Some(lease_records) =
                leases_by_resource.remove(&resource_record.resource_definition_id)
            {
                for lr in lease_records {
                    let pod = Pod {
                        ip: lr.pod_ip.0,
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

            let resource_revision = resource_record.revision.try_into()?;
            let state = QuotaState::from_persisted(
                definition,
                resource_record.remaining.into(),
                resource_record.last_refilled_at.into(),
                resource_record.last_refreshed_at.into(),
                resource_revision,
                pod_leases,
            );

            let _ = self
                .entries
                .insert_async(id, Arc::new(RwLock::new(Some(state))))
                .await;

            info!(%id, "restored quota resource from database");
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

                let mut guard = handle.write().await;
                let state = guard.as_mut().ok_or(QuotaError::LeaseNotFound {
                    resource_definition_id: id,
                })?;
                let snapshot = state.clone();
                let prev_rev = state.current_revision();
                let result = state.acquire_lease(pod, self.lease_duration, self.min_executors);

                if let Err(e) = state.bump_revision() {
                    warn!(error = %e, "failed to bump revision, rolling back");
                    *state = snapshot;
                    return Err(e.into());
                }

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

                if let Err(e) = self
                    .persist_after_lease_change(state, prev_rev, &pod, &result.expired)
                    .await
                {
                    log_on_failed_persistence(&e);
                    *state = snapshot;
                    return Err(e.into());
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

        let mut guard = handle.write().await;
        let state = guard.as_mut().ok_or(QuotaError::LeaseNotFound {
            resource_definition_id,
        })?;
        let snapshot = state.clone();
        let prev_rev = state.current_revision();
        let result = state.renew_lease(
            &pod,
            epoch,
            unused,
            self.lease_duration,
            self.min_executors,
            pending_reservations,
        )?;

        if let Err(e) = state.bump_revision() {
            warn!(error = %e, "failed to bump revision, rolling back");
            *state = snapshot;
            return Err(e.into());
        }

        if let Err(e) = self
            .persist_after_lease_change(state, prev_rev, &pod, &result.expired)
            .await
        {
            log_on_failed_persistence(&e);
            *state = snapshot;
            return Err(e.into());
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

        let mut guard = handle.write().await;
        let state = guard.as_mut().ok_or(QuotaError::LeaseNotFound {
            resource_definition_id,
        })?;
        let snapshot = state.clone();
        let prev_rev = state.current_revision();
        state.release_lease(&pod, epoch, unused)?;

        if let Err(e) = state.bump_revision() {
            warn!(error = %e, "failed to bump revision, rolling back");
            *state = snapshot;
            return Err(e.into());
        }

        if let Err(e) = self
            .persist_after_lease_release(state, prev_rev, &pod)
            .await
        {
            log_on_failed_persistence(&e);
            *state = snapshot;
            return Err(e.into());
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
        previous_revision: i64,
        pod: &Pod,
        expired_pods: &[Pod],
    ) -> Result<(), QuotaRepoError> {
        let resource_record = state.to_resource_record();

        let lease_record = state
            .to_lease_record(pod)
            .ok_or_else(|| anyhow::anyhow!("pod lease not found after mutation"))?;

        let expired: Vec<(Json<IpAddr>, i32)> = expired_pods
            .iter()
            .map(|p| (Json(p.ip), p.port.into()))
            .collect();

        self.repo
            .save_lease_change(&resource_record, previous_revision, &lease_record, &expired)
            .await
    }

    async fn persist_after_lease_release(
        &self,
        state: &QuotaState,
        previous_revision: i64,
        pod: &Pod,
    ) -> Result<(), QuotaRepoError> {
        let resource_record = state.to_resource_record();
        self.repo
            .save_lease_release(
                &resource_record,
                previous_revision,
                Json(pod.ip),
                pod.port.into(),
            )
            .await
    }

    async fn persist_resource(
        &self,
        state: &QuotaState,
        previous_revision: i64,
    ) -> Result<(), QuotaRepoError> {
        let record = state.to_resource_record();
        self.repo.save_resource(&record, previous_revision).await
    }

    async fn refresh_if_stale(&self, id: ResourceDefinitionId) {
        let is_stale = self
            .entries
            .read_async(&id, |_, handle| {
                handle
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
        if handle.read().await.is_none() {
            return None;
        }
        Some(handle)
    }

    async fn ensure_entry(&self, definition: &golem_common::model::quota::ResourceDefinition) {
        let _ = self
            .entries
            .entry_async(definition.id)
            .await
            .or_insert_with(|| Arc::new(RwLock::new(Some(QuotaState::new(definition.clone())))));
    }

    async fn refresh_entry(&self, id: ResourceDefinitionId) {
        let handle = match self.get_entry_handle(id).await {
            Some(h) => h,
            None => return,
        };

        match self.fetcher.fetch_by_id(id).await {
            Ok(definition) => {
                debug_assert_eq!(definition.id, id);
                let mut guard = handle.write().await;
                let state = match guard.as_mut() {
                    Some(s) => s,
                    None => return,
                };
                let snapshot = state.clone();
                let prev_rev = state.current_revision();
                state.update_definition(definition);
                if let Err(e) = state.bump_revision() {
                    warn!(error = %e, %id, "failed to bump revision, rolling back");
                    *state = snapshot;
                    return;
                }
                if let Err(e) = self.persist_resource(state, prev_rev).await {
                    log_on_failed_persistence(&e);
                    *state = snapshot;
                }
            }
            Err(FetchError::NotFound) => {
                debug!(%id, "resource definition no longer exists, removing");
                // Tombstone while holding the lock — other threads will see None
                // and treat it as non-existent. Then remove from map.
                let mut guard = handle.write().await;
                // Delete from DB first. If this fails, keep in-memory state
                // so they stay consistent. Staleness refresh will retry later.
                if let Err(e) = self.repo.delete_resource_and_leases(id).await {
                    warn!(error = %e, %id, "failed to delete resource from db, keeping in-memory state");
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
        QuotaRepoError::InternalError(_) => {
            warn!(error = %e, "Persisting state failed, rolling back")
        }
    }
}
