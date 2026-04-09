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
use golem_common::model::quota::LeaseEpoch;
use golem_common::model::quota::{ResourceDefinition, ResourceLimit};
use golem_service_base::model::quota_lease::PendingReservation;
use golem_service_base::repo::Blob;
use std::collections::HashMap;
use std::time::Duration;
use tracing::debug;

pub(super) struct AcquireLeaseResult {
    pub epoch: LeaseEpoch,
    pub allocated_amount: u64,
    pub expires_at: DateTime<Utc>,
    pub expired: Vec<Pod>,
    pub total_available_amount: u64,
}

pub(super) struct RenewLeaseResult {
    pub new_epoch: LeaseEpoch,
    pub allocated_amount: u64,
    pub expires_at: DateTime<Utc>,
    pub expired: Vec<Pod>,
    pub total_available_amount: u64,
}

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

#[derive(Debug, Clone)]
pub(super) struct PodLease {
    pub epoch: LeaseEpoch,
    pub allocated: u64,
    pub granted_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub pending_reservations: Vec<PendingReservation>,
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

        let period = rate.period.duration();
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

    /// Computes the allocation share for a single executor.
    pub fn compute_allocation(&self, pod: &Pod, min_executors: u64) -> u64 {
        /// How much more than the exact pending demand to grant, to avoid
        /// immediately hitting the limit again on the next invocation.
        const OVERFETCH_FACTOR: f64 = 1.5;

        let active_count = self.leases.len() as u64;
        debug_assert!(active_count > 0);
        debug_assert!(min_executors > 0);

        fn priority_weighted_demand(reservations: &[PendingReservation]) -> f64 {
            reservations
                .iter()
                .map(|r| r.amount as f64 * r.priority.max(0.1))
                .sum()
        }

        fn total_pending_amount(reservations: &[PendingReservation]) -> u64 {
            reservations.iter().map(|r| r.amount).sum()
        }

        let pod_lease = self.leases.get(pod);

        let pod_score: f64 = pod_lease
            .map(|l| priority_weighted_demand(&l.pending_reservations))
            .unwrap_or(0.0);

        let pod_pending_amount: u64 = pod_lease
            .map(|l| total_pending_amount(&l.pending_reservations))
            .unwrap_or(0);

        let total_score: f64 = self
            .leases
            .values()
            .map(|l| priority_weighted_demand(&l.pending_reservations))
            .sum();

        // even-split baseline — also the floor when there is no demand.
        let baseline = self.remaining / active_count.max(min_executors);

        // proportional share; 0 when there is no demand (total_score=0).
        let proportional =
            (self.remaining as f64 * pod_score / total_score.max(f64::MIN_POSITIVE)) as u64;

        // cap at pending_amount * OVERFETCH_FACTOR, but never below
        // baseline so the cap is always a valid upper bound for clamp.
        let cap = ((pod_pending_amount as f64 * OVERFETCH_FACTOR) as u64).max(baseline);

        proportional.clamp(baseline, cap).min(self.remaining)
    }

    /// Total capacity for this resource: remaining pool + all outstanding
    /// lease allocations.
    pub fn total_available_amount(&self) -> u64 {
        let total_allocated: u64 = self.leases.values().map(|l| l.allocated).sum();
        self.remaining + total_allocated
    }

    pub fn acquire_lease(
        &mut self,
        pod: Pod,
        lease_duration: Duration,
        min_executors: u64,
    ) -> AcquireLeaseResult {
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
            pending_reservations: Vec::new(),
        });

        let allocated_amount = self.compute_allocation(&pod, min_executors);
        let total_available_amount = self.total_available_amount();

        let pod_lease = self.leases.get_mut(&pod).expect("just inserted");
        let epoch = pod_lease.epoch;
        pod_lease.epoch = epoch.next();
        self.remaining -= allocated_amount;
        pod_lease.allocated = allocated_amount;
        pod_lease.granted_at = now;
        pod_lease.expires_at = expires_at;

        AcquireLeaseResult {
            epoch,
            allocated_amount,
            expires_at,
            expired,
            total_available_amount,
        }
    }

    pub fn renew_lease(
        &mut self,
        pod: &Pod,
        epoch: LeaseEpoch,
        unused: u64,
        lease_duration: Duration,
        min_executors: u64,
        pending_reservations: Vec<PendingReservation>,
    ) -> Result<RenewLeaseResult, QuotaError> {
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
        self.remaining = self.remaining.saturating_add(returned);

        pod_lease.allocated = 0;
        pod_lease.granted_at = now;
        pod_lease.expires_at = expires_at;
        pod_lease.pending_reservations = pending_reservations;

        let expired = self.housekeep();

        let pod_lease = self
            .leases
            .get_mut(pod)
            .expect("just validated and refreshed");
        let new_epoch = pod_lease.epoch;
        pod_lease.epoch = new_epoch.next();

        let allocated_amount = self.compute_allocation(pod, min_executors);
        let total_available_amount = self.total_available_amount();

        let pod_lease = self
            .leases
            .get_mut(pod)
            .expect("just validated and refreshed");

        self.remaining -= allocated_amount;
        pod_lease.allocated = allocated_amount;

        Ok(RenewLeaseResult {
            new_epoch,
            allocated_amount,
            expires_at,
            expired,
            total_available_amount,
        })
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
            pod_ip: Blob::new(pod.ip),
            pod_port: pod.port.into(),
            epoch: lease.epoch.0.into(),
            allocated: lease.allocated.into(),
            granted_at: lease.granted_at.into(),
            expires_at: lease.expires_at.into(),
            pending_reservations: Blob::new(lease.pending_reservations.clone()),
        })
    }
}
