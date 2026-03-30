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
use super::resource_definition_fetcher::{FetchError, ResourceDefinitionFetcher};
use crate::model::Pod;
use crate::shard_manager_config::QuotaServiceConfig;
use golem_common::SafeDisplay;
use golem_common::model::environment::EnvironmentId;
use golem_common::model::resource_definition::{
    ResourceDefinition, ResourceDefinitionId, ResourceLimit, ResourceName, TimePeriod,
};
use golem_service_base::model::quota_lease::{LeaseEpoch, QuotaAllocation};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use tokio::time::Instant;
use tracing::{debug, warn};

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

golem_common::error_forwarding!(QuotaError);

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

enum QuotaEntry {
    Live(Box<LiveQuotaState>),
    Tombstoned,
}

struct PodLease {
    epoch: LeaseEpoch,
    allocated: u64,
    granted_at: Instant,
}

struct LiveQuotaState {
    definition: ResourceDefinition,
    last_refreshed: Instant,
    remaining: u64,
    last_refilled: Instant,
    leases: HashMap<Pod, PodLease>,
}

impl LiveQuotaState {
    fn new(definition: ResourceDefinition) -> Self {
        let remaining = Self::initial_pool(&definition.limit);
        let now = Instant::now();
        Self {
            definition,
            last_refreshed: now,
            remaining,
            last_refilled: now,
            leases: HashMap::new(),
        }
    }

    fn initial_pool(limit: &ResourceLimit) -> u64 {
        match limit {
            ResourceLimit::Capacity(c) => c.value,
            ResourceLimit::Concurrency(c) => c.value,
            ResourceLimit::Rate(r) => r.max,
        }
    }

    fn period_duration(period: &TimePeriod) -> Duration {
        match period {
            TimePeriod::Second => Duration::from_secs(1),
            TimePeriod::Minute => Duration::from_secs(60),
            TimePeriod::Hour => Duration::from_secs(3600),
            TimePeriod::Day => Duration::from_secs(86400),
            TimePeriod::Month => Duration::from_secs(30 * 86400),
            TimePeriod::Year => Duration::from_secs(365 * 86400),
        }
    }

    fn update_definition(&mut self, definition: ResourceDefinition) {
        debug_assert_eq!(self.definition.id, definition.id);
        debug_assert_eq!(self.definition.environment_id, definition.environment_id);
        let new_pool = Self::initial_pool(&definition.limit);
        let total_allocated: u64 = self.leases.values().map(|l| l.allocated).sum();
        self.remaining = new_pool.saturating_sub(total_allocated);
        self.definition = definition;
        self.last_refreshed = Instant::now();
    }

    fn is_stale(&self, ttl: Duration) -> bool {
        self.last_refreshed.elapsed() > ttl
    }

    fn reclaim_expired(&mut self, lease_duration: Duration) {
        let expired: Vec<Pod> = self
            .leases
            .iter()
            .filter(|(_, lease)| lease.granted_at.elapsed() > lease_duration)
            .map(|(pod, _)| pod.clone())
            .collect();
        for pod in expired {
            if let Some(lease) = self.leases.remove(&pod) {
                let returned = match &self.definition.limit {
                    // Capacity: consumed units are gone — assume executor used everything.
                    ResourceLimit::Capacity(_) => 0,
                    // Concurrency: slots are freed when the executor goes away.
                    ResourceLimit::Concurrency(_) => lease.allocated,
                    // Rate: assume executor consumed all tokens. The refill
                    // mechanism will replenish over time.
                    ResourceLimit::Rate(_) => 0,
                };
                self.remaining += returned;
                debug!(
                    pod = %pod,
                    allocated = lease.allocated,
                    returned,
                    "reclaiming expired lease"
                );
            }
        }
    }

    /// For rate limits, refills the pool based on elapsed time since the last refill.
    /// No-op for capacity and concurrency limits.
    fn refill_rate(&mut self) {
        let rate = match &self.definition.limit {
            ResourceLimit::Rate(r) => r,
            _ => return,
        };

        let period = Self::period_duration(&rate.period);
        let elapsed = self.last_refilled.elapsed();
        let full_periods = elapsed.as_nanos() / period.as_nanos();

        if full_periods > 0 {
            let refill = (rate.value as u128 * full_periods).min(rate.max as u128) as u64;
            let total_allocated: u64 = self.leases.values().map(|l| l.allocated).sum();
            let cap = rate.max.saturating_sub(total_allocated);
            self.remaining = (self.remaining + refill).min(cap);
            // Advance by whole periods only, preserving sub-period remainder.
            let advance = Duration::from_nanos((period.as_nanos() * full_periods) as u64);
            self.last_refilled += advance;
        }
    }

    fn housekeep(&mut self, lease_duration: Duration) {
        self.reclaim_expired(lease_duration);
        self.refill_rate();
    }

    fn compute_allocation(
        &self,
        active_count: u64,
        min_executors: u64,
        exhausted_retry_after: Duration,
    ) -> QuotaAllocation {
        debug_assert!(active_count > 0);
        debug_assert!(min_executors > 0);
        let divisor = active_count.max(min_executors);
        let share = self.remaining / divisor;
        if share > 0 {
            QuotaAllocation::Budget { amount: share }
        } else {
            QuotaAllocation::Exhausted {
                retry_after: self
                    .time_until_next_refill()
                    .unwrap_or(exhausted_retry_after),
            }
        }
    }

    /// For rate limits, returns the duration until the next full period refill.
    /// Returns None for capacity/concurrency limits (no time-based refill).
    fn time_until_next_refill(&self) -> Option<Duration> {
        let rate = match &self.definition.limit {
            ResourceLimit::Rate(r) => r,
            _ => return None,
        };
        let period = Self::period_duration(&rate.period);
        let elapsed = self.last_refilled.elapsed();
        let remaining_in_period = period.saturating_sub(elapsed);
        Some(remaining_in_period)
    }

    fn acquire_lease(
        &mut self,
        pod: Pod,
        lease_duration: Duration,
        min_executors: u64,
        exhausted_retry_after: Duration,
    ) -> (LeaseEpoch, QuotaAllocation) {
        self.housekeep(lease_duration);

        // If the pod already has a lease, return its previous allocation
        // to the pool before issuing a new one.
        if let Some(existing) = self.leases.get(&pod) {
            let returned = match &self.definition.limit {
                ResourceLimit::Capacity(_) => 0,
                ResourceLimit::Concurrency(_) => existing.allocated,
                ResourceLimit::Rate(_) => 0,
            };
            self.remaining += returned;
        }

        self.leases.entry(pod.clone()).or_insert_with(|| PodLease {
            epoch: LeaseEpoch::initial(),
            allocated: 0,
            granted_at: Instant::now(),
        });

        let active_count = self.leases.len() as u64;
        let allocation =
            self.compute_allocation(active_count, min_executors, exhausted_retry_after);
        let allocated_amount = allocation.amount();

        let pod_lease = self.leases.get_mut(&pod).expect("just inserted");
        let epoch = pod_lease.epoch;
        pod_lease.epoch = epoch.next();
        self.remaining -= allocated_amount;
        pod_lease.allocated = allocated_amount;
        pod_lease.granted_at = Instant::now();

        (epoch, allocation)
    }

    fn renew_lease(
        &mut self,
        pod: &Pod,
        epoch: LeaseEpoch,
        unused: u64,
        lease_duration: Duration,
        min_executors: u64,
        exhausted_retry_after: Duration,
    ) -> Result<(LeaseEpoch, QuotaAllocation), QuotaError> {
        let pod_lease = self.leases.get_mut(pod).ok_or(QuotaError::LeaseNotFound {
            resource_definition_id: self.definition.id,
        })?;
        if epoch.next() != pod_lease.epoch {
            return Err(QuotaError::StaleEpoch {
                resource_definition_id: self.definition.id,
                provided: epoch,
                current: pod_lease.epoch,
            });
        }

        // Return unused and reset the grant timestamp so housekeep
        // won't reclaim this pod's lease as expired.
        let returned = unused.min(pod_lease.allocated);
        self.remaining += returned;
        pod_lease.allocated = 0;
        pod_lease.granted_at = Instant::now();

        self.housekeep(lease_duration);

        let pod_lease = self
            .leases
            .get_mut(pod)
            .expect("just validated and refreshed");
        let new_epoch = pod_lease.epoch;
        pod_lease.epoch = new_epoch.next();

        let active_count = self.leases.len() as u64;
        let allocation =
            self.compute_allocation(active_count, min_executors, exhausted_retry_after);
        let allocated_amount = allocation.amount();

        let pod_lease = self
            .leases
            .get_mut(pod)
            .expect("just validated and refreshed");
        self.remaining -= allocated_amount;
        pod_lease.allocated = allocated_amount;
        pod_lease.granted_at = Instant::now();

        Ok((new_epoch, allocation))
    }

    fn release_lease(
        &mut self,
        pod: &Pod,
        epoch: LeaseEpoch,
        unused: u64,
    ) -> Result<(), QuotaError> {
        let pod_lease = self.leases.get(pod).ok_or(QuotaError::LeaseNotFound {
            resource_definition_id: self.definition.id,
        })?;
        if epoch.next() != pod_lease.epoch {
            return Err(QuotaError::StaleEpoch {
                resource_definition_id: self.definition.id,
                provided: epoch,
                current: pod_lease.epoch,
            });
        }
        let returned = match &self.definition.limit {
            // Capacity: only the executor-reported unused portion is returned.
            ResourceLimit::Capacity(_) => unused.min(pod_lease.allocated),
            // Concurrency: all slots are freed when the executor releases.
            ResourceLimit::Concurrency(_) => pod_lease.allocated,
            // Rate: unused tokens return to the pool.
            ResourceLimit::Rate(_) => unused.min(pod_lease.allocated),
        };
        self.remaining += returned;
        self.leases.remove(pod);
        Ok(())
    }
}

type EntryHandle = Arc<RwLock<QuotaEntry>>;

pub struct QuotaService {
    entries: scc::HashMap<ResourceDefinitionId, EntryHandle>,
    fetcher: Arc<dyn ResourceDefinitionFetcher>,
    ttl: Duration,
    lease_duration: Duration,
    min_executors: u64,
    exhausted_retry_after: Duration,
}

impl QuotaService {
    pub fn new(
        config: QuotaServiceConfig,
        fetcher: Arc<dyn ResourceDefinitionFetcher>,
    ) -> Arc<Self> {
        assert!(config.min_executors > 0, "min_executors must be at least 1");
        Arc::new(Self {
            entries: scc::HashMap::new(),
            fetcher,
            ttl: config.definition_staleness_ttl,
            lease_duration: config.lease_duration,
            min_executors: config.min_executors,
            exhausted_retry_after: config.exhausted_retry_after,
        })
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

                let mut entry = handle.write().await;
                match &mut *entry {
                    QuotaEntry::Live(live) => {
                        let (epoch, allocation) = live.acquire_lease(
                            pod.clone(),
                            self.lease_duration,
                            self.min_executors,
                            self.exhausted_retry_after,
                        );
                        Ok(QuotaLease::Bounded {
                            resource_definition_id: id,
                            pod,
                            epoch,
                            allocation,
                            expires_after: self.lease_duration,
                            resource_limit: live.definition.limit.clone(),
                            enforcement_action: live.definition.enforcement_action,
                        })
                    }
                    QuotaEntry::Tombstoned => Ok(self.unlimited_lease(pod)),
                }
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

        let mut entry = handle.write().await;
        match &mut *entry {
            QuotaEntry::Live(live) => {
                let (new_epoch, allocation) = live.renew_lease(
                    &pod,
                    epoch,
                    unused,
                    self.lease_duration,
                    self.min_executors,
                    self.exhausted_retry_after,
                )?;
                Ok(QuotaLease::Bounded {
                    resource_definition_id,
                    pod,
                    epoch: new_epoch,
                    allocation,
                    expires_after: self.lease_duration,
                    resource_limit: live.definition.limit.clone(),
                    enforcement_action: live.definition.enforcement_action,
                })
            }
            // Resource was deleted — executor should re-acquire by name
            // to pick up any newly created resource.
            QuotaEntry::Tombstoned => Err(QuotaError::LeaseNotFound {
                resource_definition_id,
            }),
        }
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

        let mut entry = handle.write().await;
        match &mut *entry {
            QuotaEntry::Live(live) => {
                live.release_lease(&pod, epoch, unused)?;
                Ok(())
            }
            // Resource was deleted — no lease to release.
            QuotaEntry::Tombstoned => Err(QuotaError::LeaseNotFound {
                resource_definition_id,
            }),
        }
    }

    pub async fn on_resource_definition_changed(
        &self,
        resource_definition_id: ResourceDefinitionId,
    ) {
        if let Some(handle) = self.get_entry_handle(resource_definition_id).await {
            let is_live = {
                let entry = handle.read().await;
                matches!(&*entry, QuotaEntry::Live(_))
            };
            if is_live {
                self.refresh_entry(resource_definition_id).await;
            }
        }
    }

    pub async fn on_cursor_expired(&self) {
        let mut live_ids = Vec::new();
        self.entries
            .iter_async(|id, handle| {
                if let Ok(entry) = handle.try_read() {
                    if matches!(&*entry, QuotaEntry::Live(_)) {
                        live_ids.push(*id);
                    }
                } else {
                    live_ids.push(*id);
                }
                true
            })
            .await;

        for id in live_ids {
            self.refresh_entry(id).await;
        }
    }

    fn unlimited_lease(&self, pod: Pod) -> QuotaLease {
        QuotaLease::Unlimited {
            pod,
            expires_after: self.lease_duration,
        }
    }

    async fn refresh_if_stale(&self, id: ResourceDefinitionId) {
        let is_stale = self
            .entries
            .read_async(&id, |_, handle| {
                handle
                    .try_read()
                    .map(|entry| match &*entry {
                        QuotaEntry::Live(live) => live.is_stale(self.ttl),
                        QuotaEntry::Tombstoned => false,
                    })
                    .unwrap_or(false)
            })
            .await
            .unwrap_or(false);

        if is_stale {
            self.refresh_entry(id).await;
        }
    }

    async fn get_entry_handle(&self, id: ResourceDefinitionId) -> Option<EntryHandle> {
        self.entries
            .read_async(&id, |_, handle| handle.clone())
            .await
    }

    async fn ensure_entry(&self, definition: &ResourceDefinition) {
        let _ = self
            .entries
            .entry_async(definition.id)
            .await
            .or_insert_with(|| {
                Arc::new(RwLock::new(QuotaEntry::Live(Box::new(
                    LiveQuotaState::new(definition.clone()),
                ))))
            });
    }

    async fn refresh_entry(&self, id: ResourceDefinitionId) {
        let handle = match self.get_entry_handle(id).await {
            Some(h) => h,
            None => return,
        };

        match self.fetcher.fetch_by_id(id).await {
            Ok(definition) => {
                debug_assert_eq!(definition.id, id);
                let mut entry = handle.write().await;
                if let QuotaEntry::Live(live) = &mut *entry {
                    live.update_definition(definition);
                }
            }
            Err(FetchError::NotFound) => {
                debug!(%id, "resource definition no longer exists, tombstoning");
                let mut entry = handle.write().await;
                *entry = QuotaEntry::Tombstoned;
            }
            Err(err) => {
                warn!(%id, error = %err, "failed to refresh resource definition, keeping stale entry");
            }
        }
    }
}
