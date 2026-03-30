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

use super::quota_repo::{QuotaLeaseRecord, QuotaResourceRecord};
use super::quota_service::QuotaError;
use chrono::{DateTime, Utc};
use golem_common::model::Pod;
use golem_common::model::resource_definition::{ResourceDefinition, ResourceLimit, TimePeriod};
use golem_service_base::model::quota_lease::{LeaseEpoch, QuotaAllocation};
use golem_service_base::repo::Blob;
use sqlx::types::Json;
use std::collections::HashMap;
use std::time::Duration;
use tracing::debug;

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub(super) struct QuotaResourceRevision(u64);

impl QuotaResourceRevision {
    pub const INITIAL: Self = Self(0);

    pub fn next(self) -> anyhow::Result<Self> {
        if self.0 == i64::MAX as u64 {
            Err(anyhow::anyhow!(
                "Cannot increment QuotaResourceRevision beyond i64::MAX ({})",
                i64::MAX
            ))
        } else {
            Ok(Self(self.0 + 1))
        }
    }
}

impl TryFrom<i64> for QuotaResourceRevision {
    type Error = anyhow::Error;

    fn try_from(value: i64) -> Result<Self, Self::Error> {
        if value < 0 {
            Err(anyhow::anyhow!(
                "QuotaResourceRevision cannot be negative, got {}",
                value
            ))
        } else {
            Ok(Self(value as u64))
        }
    }
}

impl From<QuotaResourceRevision> for i64 {
    fn from(rev: QuotaResourceRevision) -> Self {
        // Safe: next() enforces rev.0 <= i64::MAX
        rev.0 as i64
    }
}

fn elapsed_since(dt: DateTime<Utc>) -> Duration {
    Utc::now()
        .signed_duration_since(dt)
        .to_std()
        .unwrap_or(Duration::ZERO)
}

#[derive(Clone)]
pub(super) struct PodLease {
    pub epoch: LeaseEpoch,
    pub allocated: u64,
    pub granted_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
}

#[derive(Clone)]
pub(super) struct QuotaState {
    pub definition: ResourceDefinition,
    pub last_refreshed: DateTime<Utc>,
    pub remaining: u64,
    pub last_refilled: DateTime<Utc>,
    pub revision: QuotaResourceRevision,
    pub leases: HashMap<Pod, PodLease>,
}

impl QuotaState {
    pub fn new(definition: ResourceDefinition) -> Self {
        let remaining = Self::initial_pool(&definition.limit);
        let now = Utc::now();
        Self {
            definition,
            last_refreshed: now,
            remaining,
            last_refilled: now,
            revision: QuotaResourceRevision::INITIAL,
            leases: HashMap::new(),
        }
    }

    pub fn from_persisted(
        definition: ResourceDefinition,
        remaining: u64,
        last_refilled: DateTime<Utc>,
        last_refreshed: DateTime<Utc>,
        revision: QuotaResourceRevision,
        leases: HashMap<Pod, PodLease>,
    ) -> Self {
        Self {
            definition,
            last_refreshed,
            remaining,
            last_refilled,
            revision,
            leases,
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

    pub fn update_definition(&mut self, definition: ResourceDefinition) {
        debug_assert_eq!(self.definition.id, definition.id);
        debug_assert_eq!(self.definition.environment_id, definition.environment_id);
        let new_pool = Self::initial_pool(&definition.limit);
        let total_allocated: u64 = self.leases.values().map(|l| l.allocated).sum();
        self.remaining = new_pool.saturating_sub(total_allocated);
        self.definition = definition;
        self.last_refreshed = Utc::now();
    }

    /// Returns the current revision as i64 for passing to the repo as previous_revision.
    pub fn current_revision(&self) -> i64 {
        self.revision.into()
    }

    /// Increments the revision. Must be called after each mutation, before persisting.
    pub fn bump_revision(&mut self) -> anyhow::Result<()> {
        self.revision = self.revision.next()?;
        Ok(())
    }

    pub fn is_stale(&self, ttl: Duration) -> bool {
        elapsed_since(self.last_refreshed) > ttl
    }

    pub fn reclaim_expired(&mut self) -> Vec<Pod> {
        let now = Utc::now();
        let expired: Vec<Pod> = self
            .leases
            .iter()
            .filter(|(_, lease)| now >= lease.expires_at)
            .map(|(pod, _)| *pod)
            .collect();
        for pod in &expired {
            if let Some(lease) = self.leases.remove(pod) {
                let returned = match &self.definition.limit {
                    ResourceLimit::Capacity(_) => 0,
                    ResourceLimit::Concurrency(_) => lease.allocated,
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
        expired
    }

    pub fn refill_rate(&mut self) {
        let rate = match &self.definition.limit {
            ResourceLimit::Rate(r) => r,
            _ => return,
        };

        let period = Self::period_duration(&rate.period);
        let elapsed = elapsed_since(self.last_refilled);
        let full_periods = elapsed.as_nanos() / period.as_nanos();

        if full_periods > 0 {
            let refill = (rate.value as u128 * full_periods).min(rate.max as u128) as u64;
            let total_allocated: u64 = self.leases.values().map(|l| l.allocated).sum();
            let cap = rate.max.saturating_sub(total_allocated);
            self.remaining = (self.remaining + refill).min(cap);
            let advance_nanos = period.as_nanos() * full_periods;
            self.last_refilled += chrono::Duration::nanoseconds(advance_nanos as i64);
        }
    }

    pub fn housekeep(&mut self) -> Vec<Pod> {
        let expired = self.reclaim_expired();
        self.refill_rate();
        expired
    }

    pub fn compute_allocation(
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

    fn time_until_next_refill(&self) -> Option<Duration> {
        let rate = match &self.definition.limit {
            ResourceLimit::Rate(r) => r,
            _ => return None,
        };
        let period = Self::period_duration(&rate.period);
        let elapsed = elapsed_since(self.last_refilled);
        Some(period.saturating_sub(elapsed))
    }

    pub fn acquire_lease(
        &mut self,
        pod: Pod,
        lease_duration: Duration,
        min_executors: u64,
        exhausted_retry_after: Duration,
    ) -> (LeaseEpoch, QuotaAllocation, DateTime<Utc>, Vec<Pod>) {
        let expired = self.housekeep();

        if let Some(existing) = self.leases.get(&pod) {
            let returned = match &self.definition.limit {
                ResourceLimit::Capacity(_) => 0,
                ResourceLimit::Concurrency(_) => existing.allocated,
                ResourceLimit::Rate(_) => 0,
            };
            self.remaining += returned;
        }

        let now = Utc::now();
        let expires_at = now + lease_duration;

        self.leases.entry(pod).or_insert_with(|| PodLease {
            epoch: LeaseEpoch::initial(),
            allocated: 0,
            granted_at: now,
            expires_at,
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
        pod_lease.granted_at = now;
        pod_lease.expires_at = expires_at;

        (epoch, allocation, expires_at, expired)
    }

    pub fn renew_lease(
        &mut self,
        pod: &Pod,
        epoch: LeaseEpoch,
        unused: u64,
        lease_duration: Duration,
        min_executors: u64,
        exhausted_retry_after: Duration,
    ) -> Result<(LeaseEpoch, QuotaAllocation, DateTime<Utc>, Vec<Pod>), QuotaError> {
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

        let now = Utc::now();
        let expires_at = now + lease_duration;

        let returned = unused.min(pod_lease.allocated);
        self.remaining += returned;
        pod_lease.allocated = 0;
        pod_lease.granted_at = now;
        pod_lease.expires_at = expires_at;

        let expired = self.housekeep();

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
        pod_lease.granted_at = now;
        pod_lease.expires_at = expires_at;

        Ok((new_epoch, allocation, expires_at, expired))
    }

    pub fn release_lease(
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
            ResourceLimit::Capacity(_) => unused.min(pod_lease.allocated),
            ResourceLimit::Concurrency(_) => pod_lease.allocated,
            ResourceLimit::Rate(_) => unused.min(pod_lease.allocated),
        };
        self.remaining += returned;
        self.leases.remove(pod);
        Ok(())
    }

    pub fn to_resource_record(&self) -> QuotaResourceRecord {
        QuotaResourceRecord {
            resource_definition_id: self.definition.id.0,
            revision: self.revision.into(),
            definition: Blob::new(self.definition.clone()),
            remaining: self.remaining.into(),
            last_refilled_at: self.last_refilled.into(),
            last_refreshed_at: self.last_refreshed.into(),
        }
    }

    pub fn to_lease_record(&self, pod: &Pod) -> Option<QuotaLeaseRecord> {
        self.leases.get(pod).map(|lease| QuotaLeaseRecord {
            resource_definition_id: self.definition.id.0,
            pod_ip: Json(pod.ip),
            pod_port: pod.port.into(),
            epoch: lease.epoch.0.into(),
            allocated: lease.allocated.into(),
            granted_at: lease.granted_at.into(),
            expires_at: lease.expires_at.into(),
        })
    }
}
