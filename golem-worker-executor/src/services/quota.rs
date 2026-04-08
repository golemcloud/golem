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
use chrono::{DateTime, Utc};
use golem_common::model::environment::EnvironmentId;
use golem_common::model::quota::{
    EnforcementAction, LeaseEpoch, ResourceDefinitionId, ResourceLimit, ResourceName,
};
use golem_common::model::quota::{Reservation, ReserveResult};
use golem_service_base::clients::shard_manager::{BatchRenewalEntry, QuotaError, ShardManager};
use golem_service_base::model::quota_lease::{PendingReservation, QuotaLease};
use itertools::Itertools;
use std::sync::{Arc, Weak};
use std::time::Duration;
use tokio::sync::{Mutex, oneshot};
use tokio_util::sync::CancellationToken;
use tracing::debug;
use tracing::{Instrument, info, info_span};

type ResourceKey = (EnvironmentId, ResourceName);

/// Scale factors for deriving credit parameters from expected_use.
///
/// credit_rate  = expected_use * CREDIT_RATE_FACTOR  (credits per ms)
/// max_credit   = expected_use * CREDIT_MAX_FACTOR
pub const CREDIT_RATE_FACTOR: f64 = 0.1;
pub const CREDIT_MAX_FACTOR: i64 = 100;

/// Derive `max_credit` from `expected_use` without overflow.
///
/// `expected_use` is `u64` but `max_credit` is `i64`; saturate at `i64::MAX`
/// so that very large `expected_use` values never produce a negative cap.
pub fn max_credit_for(expected_use: u64) -> i64 {
    (expected_use as u128)
        .saturating_mul(CREDIT_MAX_FACTOR as u128)
        .min(i64::MAX as u128) as i64
}

#[derive(Clone)]
pub struct LeaseInterest {
    pub environment_id: EnvironmentId,
    pub resource_name: ResourceName,
    /// Expected units per reservation; stored so the interest is self-contained
    /// for re-acquire when the lease is lost.
    pub expected_use: u64,
    /// Credit value at `last_credit_value_at`
    pub last_credit_value: i64,
    /// When `last_credit_value` was last updated
    pub last_credit_value_at: DateTime<Utc>,
    /// Credits earned per millisecond while waiting
    pub credit_rate: f64,
    /// Maximum credit that can accumulate
    pub max_credit: i64,
    /// Reference counter for the lease backing this interest
    _token: Arc<()>,
}

impl LeaseInterest {
    pub fn current_credit(&self) -> i64 {
        let elapsed_ms = (Utc::now() - self.last_credit_value_at)
            .num_milliseconds()
            .max(0) as f64;
        let accrued =
            (elapsed_ms * self.credit_rate).clamp(i64::MIN as f64, i64::MAX as f64) as i64;
        self.last_credit_value
            .saturating_add(accrued)
            .min(self.max_credit)
    }

    fn debit(&mut self, amount: u64) {
        let amount_i64 = amount.min(i64::MAX as u64) as i64;
        self.last_credit_value = self.current_credit().saturating_sub(amount_i64);
        self.last_credit_value_at = Utc::now();
    }

    fn credit_back(&mut self, amount: u64) {
        let amount_i64 = amount.min(i64::MAX as u64) as i64;
        self.last_credit_value = self
            .current_credit()
            .saturating_add(amount_i64)
            .min(self.max_credit);
        self.last_credit_value_at = Utc::now();
    }

    /// Split off a child `LeaseInterest` with `child_expected_use` units.
    ///
    /// Both the parent and the child share the same underlying lease (the
    /// same `Arc<()>` token), so neither of them will release the lease until
    /// both are dropped.
    ///
    /// Credits are split proportionally by `expected_use` ratio.  The parent's
    /// `expected_use` is reduced by `child_expected_use`.  Both `credit_rate`
    /// and `max_credit` are re-derived from the updated `expected_use` values.
    ///
    /// Returns an error string if `child_expected_use > self.expected_use`.
    pub fn split(&mut self, child_expected_use: u64) -> Result<LeaseInterest, String> {
        if child_expected_use > self.expected_use {
            return Err(format!(
                "cannot split {} units from a token with only {} expected-use",
                child_expected_use, self.expected_use
            ));
        }

        let parent_expected_use = self.expected_use - child_expected_use;
        let now = Utc::now();
        let current = self.current_credit();

        // Proportional credit split: child gets its share, parent keeps the rest.
        let child_credit = if self.expected_use > 0 {
            (current as i128 * child_expected_use as i128 / self.expected_use as i128) as i64
        } else {
            0
        };
        let parent_credit = current - child_credit;

        // Update parent in place.
        self.expected_use = parent_expected_use;
        self.credit_rate = parent_expected_use as f64 * CREDIT_RATE_FACTOR;
        self.max_credit = max_credit_for(parent_expected_use);
        self.last_credit_value = parent_credit;
        self.last_credit_value_at = now;

        let child_max_credit = max_credit_for(child_expected_use);
        let child = LeaseInterest {
            environment_id: self.environment_id,
            resource_name: self.resource_name.clone(),
            expected_use: child_expected_use,
            last_credit_value: child_credit.min(child_max_credit),
            last_credit_value_at: now,
            credit_rate: child_expected_use as f64 * CREDIT_RATE_FACTOR,
            max_credit: child_max_credit,
            _token: self._token.clone(),
        };

        Ok(child)
    }

    /// Merge `other` into `self`, combining `expected_use` and credits.
    ///
    /// Both tokens must refer to the same resource (`environment_id` and
    /// `resource_name` must match).  `other` is consumed.
    ///
    /// Returns an error string if the tokens refer to different resources.
    pub fn merge(&mut self, other: LeaseInterest) -> Result<(), String> {
        if self.environment_id != other.environment_id || self.resource_name != other.resource_name
        {
            return Err(format!(
                "cannot merge tokens for different resources: `{}` vs `{}`",
                self.resource_name, other.resource_name
            ));
        }

        let now = Utc::now();
        let merged_expected_use = self.expected_use.saturating_add(other.expected_use);
        let merged_credit = self.current_credit().saturating_add(other.current_credit());

        self.expected_use = merged_expected_use;
        self.credit_rate = merged_expected_use as f64 * CREDIT_RATE_FACTOR;
        self.max_credit = max_credit_for(merged_expected_use);
        self.last_credit_value = merged_credit.min(self.max_credit);
        self.last_credit_value_at = now;

        // `other` is dropped here, releasing its Arc<()> reference count.
        Ok(())
    }
}

#[async_trait]
pub trait QuotaService: Send + Sync {
    /// Declare interest in a resource. If a lease already exists it is
    /// reused; otherwise one is acquired from the shard manager.
    ///
    /// - `expected_use`: the agent's expected units per reservation, used
    ///   to derive `credit_rate` and `max_credit` on the returned interest.
    /// - `previous_credit`: credit value saved from a prior session
    ///   (e.g. restored from the oplog after a suspend/resume cycle).
    ///   Pass `0` for a fresh agent.
    /// - `previous_credit_at`: the timestamp at which `previous_credit` was
    ///   recorded.  When `Some`, used as the baseline for credit accrual so that
    ///   replayed invocations see exactly the same credit trajectory as the
    ///   original execution.  Pass `None` for a fresh agent (baseline = now).
    ///
    /// Returns a `LeaseInterest` token that keeps the lease alive and
    /// carries the agent's credit state.
    async fn acquire(
        &self,
        environment_id: EnvironmentId,
        resource_name: ResourceName,
        expected_use: u64,
        previous_credit: i64,
        previous_credit_at: Option<DateTime<Utc>>,
    ) -> Result<LeaseInterest, QuotaError>;

    /// Try to reserve `amount` units from the local allocation.
    ///
    /// Takes `&mut LeaseInterest` so that credit updates are reflected on
    /// the caller's token — callers should persist `last_credit_value` to
    /// the oplog for replay continuity.
    ///
    /// This will wait internally up until `inline_wait_threshold` without failing the reservation
    /// if there is a chance the reservation can be completed successfully.
    async fn try_reserve(&self, interest: &mut LeaseInterest, amount: u64) -> ReserveResult;

    /// Commit actual usage after a reservation.
    async fn commit(&self, interest: &mut LeaseInterest, reservation: Reservation, used: u64);
}

/// Outcome sent through the waiter channel.
#[derive(Debug)]
enum WaiterOutcome {
    Granted(Reservation),
    Insufficient {
        enforcement_action: EnforcementAction,
        estimated_wait_time: Option<Duration>,
    },
}

/// A pending `try_reserve` call that is waiting in the queue.
///
/// Stores enough credit state to recompute priority on each
/// `process_waiters` pass, so that time spent waiting is properly
/// reflected in the ordering.
struct Waiter {
    amount: u64,
    last_credit_value: i64,
    last_credit_value_at: DateTime<Utc>,
    credit_rate: f64,
    max_credit: i64,
    tx: oneshot::Sender<WaiterOutcome>,
}

impl Waiter {
    fn from_interest(
        amount: u64,
        interest: &LeaseInterest,
        tx: oneshot::Sender<WaiterOutcome>,
    ) -> Self {
        Self {
            amount,
            last_credit_value: interest.last_credit_value,
            last_credit_value_at: interest.last_credit_value_at,
            credit_rate: interest.credit_rate,
            max_credit: interest.max_credit,
            tx,
        }
    }

    /// Current credit, recomputed from the waiter's accumulated state.
    fn current_credit(&self) -> i64 {
        let elapsed_ms = (Utc::now() - self.last_credit_value_at)
            .num_milliseconds()
            .max(0) as f64;
        let accrued = (elapsed_ms * self.credit_rate) as i64;
        (self.last_credit_value + accrued).min(self.max_credit)
    }

    /// Priority score: credit / amount. Higher is more deserving.
    fn priority(&self) -> f64 {
        if self.amount == 0 {
            f64::MAX
        } else {
            self.current_credit() as f64 / self.amount as f64
        }
    }

    /// construct a waiter with a fixed credit value for testing.
    #[cfg(test)]
    fn with_credit(amount: u64, credit: i64, tx: oneshot::Sender<WaiterOutcome>) -> Self {
        Self {
            amount,
            last_credit_value: credit,
            last_credit_value_at: Utc::now(),
            credit_rate: 0.0,
            max_credit: i64::MAX,
            tx,
        }
    }
}

struct LeaseEntry {
    interest: Weak<()>,
    waiters: Vec<Waiter>,
    lease: TrackedLease,
}

impl LeaseEntry {
    // take the lease, replacing it with a lost lease. Should only be called immediately
    // before releasing the lease back to the shard manager.
    fn take_lease(&mut self) -> TrackedLease {
        std::mem::replace(&mut self.lease, TrackedLease::Lost)
    }
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
enum TrackedLease {
    Bounded(BoundedLease),
    Unlimited(UnlimitedLease),
    // lease was lost, treat the same as not found
    Lost,
}

#[derive(Debug, Clone)]
struct BoundedLease {
    resource_definition_id: ResourceDefinitionId,
    epoch: LeaseEpoch,
    remaining: u64,
    expires_at: DateTime<Utc>,
    resource_limit: ResourceLimit,
    enforcement_action: EnforcementAction,
    total_available_amount: u64,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
struct UnlimitedLease {
    expires_at: DateTime<Utc>,
}

pub struct GrpcQuotaService {
    client: Arc<dyn ShardManager>,
    port: u16,
    renewal_interval: Duration,
    inline_wait_threshold: Duration,
    /// Per-resource state, each protected by its own Mutex so that
    /// `try_reserve` and `renew_all` for different resources are fully
    /// concurrent.
    state: scc::HashMap<ResourceKey, Arc<Mutex<LeaseEntry>>>,
}

impl GrpcQuotaService {
    pub fn new(
        client: Arc<dyn ShardManager>,
        port: u16,
        shutdown_token: CancellationToken,
        renewal_interval: Duration,
        inline_wait_threshold: Duration,
    ) -> Arc<Self> {
        let svc = Self::new_inner(client, port, renewal_interval, inline_wait_threshold);
        svc.start_renewal_loop(shutdown_token, renewal_interval);
        svc
    }

    fn new_inner(
        client: Arc<dyn ShardManager>,
        port: u16,
        renewal_interval: Duration,
        inline_wait_threshold: Duration,
    ) -> Arc<Self> {
        Arc::new(Self {
            client,
            port,
            renewal_interval,
            inline_wait_threshold,
            state: scc::HashMap::new(),
        })
    }

    fn start_renewal_loop(
        self: &Arc<Self>,
        shutdown_token: CancellationToken,
        renewal_interval: Duration,
    ) {
        let svc_weak = Arc::downgrade(self);
        tokio::spawn(
            async move {
                loop {
                    tokio::select! {
                        _ = shutdown_token.cancelled() => break,
                        _ = tokio::time::sleep(renewal_interval) => {}
                    }
                    let svc = match svc_weak.upgrade() {
                        Some(s) => s,
                        None => {
                            info!("QuotaService was dropped, stopping renewal loop");
                            break;
                        }
                    };
                    svc.renew_all().await;
                }
            }
            .instrument(info_span!("Quota renewal loop"))
            .instrument(tracing::Span::current()),
        );
    }

    async fn get_slot(&self, key: &ResourceKey) -> Option<Arc<Mutex<LeaseEntry>>> {
        self.state.read_async(key, |_, v| v.clone()).await
    }

    fn from_quota_lease(lease: &QuotaLease) -> TrackedLease {
        match lease {
            QuotaLease::Bounded {
                resource_definition_id,
                epoch,
                allocation,
                expires_at,
                resource_limit,
                enforcement_action,
                total_available_amount,
                ..
            } => TrackedLease::Bounded(BoundedLease {
                resource_definition_id: *resource_definition_id,
                epoch: *epoch,
                remaining: *allocation,
                expires_at: *expires_at,
                resource_limit: resource_limit.clone(),
                enforcement_action: *enforcement_action,
                total_available_amount: *total_available_amount,
            }),
            QuotaLease::Unlimited { expires_at, .. } => TrackedLease::Unlimited(UnlimitedLease {
                expires_at: *expires_at,
            }),
        }
    }

    fn try_grant(
        lease: &mut BoundedLease,
        key: &ResourceKey,
        amount: u64,
        resource_definition_id: ResourceDefinitionId,
    ) -> Option<Reservation> {
        if lease.remaining >= amount {
            lease.remaining -= amount;
            Some(Reservation::Bounded {
                environment_id: key.0,
                resource_name: key.1.clone(),
                resource_definition_id,
                epoch: lease.epoch,
                reserved: amount,
            })
        } else {
            None
        }
    }

    /// estimates how long it will take for `amount` units
    /// to become available, given the current global deficit and the
    /// resource's refill characteristics.
    fn estimated_wait(
        amount: u64,
        total_available: u64,
        limit: &ResourceLimit,
    ) -> Option<Duration> {
        if amount <= total_available {
            return Some(Duration::ZERO);
        }
        let deficit = amount - total_available;
        match limit {
            ResourceLimit::Rate(rate) => {
                let period_duration = rate.period.duration();
                let tokens_per_refill = rate.value;
                let refills_needed = deficit.div_ceil(tokens_per_refill);
                let refills_needed_u32 = if refills_needed < u32::MAX as u64 {
                    refills_needed as u32
                } else {
                    u32::MAX
                };
                Some(
                    period_duration
                        .checked_mul(refills_needed_u32)
                        .unwrap_or(Duration::MAX),
                )
            }
            // Capacity and concurrency limits have no refill.
            ResourceLimit::Capacity(_) | ResourceLimit::Concurrency(_) => None,
        }
    }

    fn process_waiters(&self, slot: &mut LeaseEntry, key: &ResourceKey) {
        info!("processing quota waiters");
        let lease = match &mut slot.lease {
            TrackedLease::Bounded(b) => b,
            TrackedLease::Unlimited(_) => {
                // drain waiters — unlimited means everyone succeeds.
                for w in slot.waiters.drain(..) {
                    let _ = w.tx.send(WaiterOutcome::Granted(Reservation::Unlimited));
                }
                return;
            }
            TrackedLease::Lost => {
                return;
            }
        };

        let enforcement_action = lease.enforcement_action;
        let total_available = lease.total_available_amount;
        let resource_limit = lease.resource_limit.clone();
        let resource_definition_id = lease.resource_definition_id;

        // sort by descending priority, computed fresh at dispatch time so
        // that time spent waiting is correctly reflected in ordering.
        let waiters: Vec<Waiter> = std::mem::take(&mut slot.waiters)
            .into_iter()
            .map(|w| (w.priority(), w))
            .sorted_unstable_by(|(a, _), (b, _)| b.total_cmp(a))
            .map(|(_, w)| w)
            .collect();

        let mut remaining_waiters: Vec<Waiter> = Vec::with_capacity(waiters.len());

        for waiter in waiters {
            if let Some(reservation) =
                Self::try_grant(lease, key, waiter.amount, resource_definition_id)
            {
                let _ = waiter.tx.send(WaiterOutcome::Granted(reservation));
            } else {
                let estimated_wait_time = Self::estimated_wait(waiter.amount, total_available, &resource_limit);
                let keep_inline =
                    estimated_wait_time.is_some_and(|ewt| ewt < self.inline_wait_threshold);
                debug!(
                    amount = waiter.amount,
                    remaining = lease.remaining,
                    total_available,
                    estimated_wait_nanos = estimated_wait_time.map(|d| d.as_nanos() as u64),
                    inline_wait_threshold_nanos = self.inline_wait_threshold.as_nanos() as u64,
                    keep_inline,
                    "waiter cannot be granted"
                );
                if keep_inline {
                    remaining_waiters.push(waiter);
                } else {
                    let _ = waiter.tx.send(WaiterOutcome::Insufficient {
                        enforcement_action,
                        estimated_wait_time,
                    });
                }
            }
        }

        slot.waiters = remaining_waiters;
    }

    /// Notify the shard manager of pending demand immediately.
    ///
    /// Called from `try_reserve` while still holding the slot lock, so the
    /// renewal sees the complete waiter list. The renewed lease is applied
    /// and `process_waiters` is called so the waiter may be served
    /// before `rx.await` is even reached.
    async fn notify_demand(&self, key: &ResourceKey, slot: &mut LeaseEntry) {
        let (environment_id, resource_name) = key;
        match &slot.lease {
            TrackedLease::Bounded(b) => {
                let entry = BatchRenewalEntry {
                    resource_definition_id: b.resource_definition_id,
                    epoch: b.epoch.0,
                    unused: b.remaining,
                    pending_reservations: slot
                        .waiters
                        .iter()
                        .map(|w| PendingReservation {
                            amount: w.amount,
                            priority: w.priority(),
                        })
                        .collect(),
                };
                match self.client.batch_renew_quota_leases(self.port, vec![entry]).await {
                    Ok(mut results) => {
                        if let Some(result) = results.pop() {
                            match result {
                        Ok(new_lease) => {
                                    if let TrackedLease::Bounded(b) = Self::from_quota_lease(&new_lease) {
                                        debug!(
                                            resource_definition_id = %b.resource_definition_id,
                                            allocation = b.remaining,
                                            total_available = b.total_available_amount,
                                            "notify_demand: received new allocation"
                                        );
                                        slot.lease = TrackedLease::Bounded(b);
                                    } else {
                                        slot.lease = Self::from_quota_lease(&new_lease);
                                    }
                                    self.process_waiters(
                                        slot,
                                        key,
                                    );
                                }
                                Err(QuotaError::LeaseNotFound(_) | QuotaError::StaleEpoch(_)) => {
                                    slot.lease = TrackedLease::Lost;
                                }
                                Err(err) => {
                                    tracing::warn!(error = %err, "Demand notification renewal failed");
                                }
                            }
                        }
                    }
                    Err(err) => {
                        tracing::warn!(error = %err, "Demand notification batch renew failed");
                    }
                }
            }
            TrackedLease::Unlimited(_) => {
                // Unlimited leases always succeed — no demand notification needed.
            }
            TrackedLease::Lost => {
                // Re-acquire immediately so waiters can be served sooner.
                match self
                    .client
                    .acquire_quota_lease(*environment_id, resource_name.clone(), self.port)
                    .await
                {
                    Ok(new_lease) => {
                        slot.lease = Self::from_quota_lease(&new_lease);
                        self.process_waiters(slot, key);
                    }
                    Err(err) => {
                        tracing::warn!(error = %err, "Re-acquire in notify_demand failed");
                    }
                }
            }
        }
    }

    async fn renew_all(&self) {
        info!("running renew_all loop");

        // phase 1: collect live and dead slots
        let mut entries_to_renew: Vec<(ResourceKey, Arc<Mutex<LeaseEntry>>)> = Vec::new();
        let mut leases_to_release: Vec<TrackedLease> = Vec::new();

        self.state
            .retain_async(|key, slot_mutex| {
                if let Ok(mut slot) = slot_mutex.try_lock() {
                    if slot.interest.strong_count() == 0 {
                        leases_to_release.push(slot.take_lease());
                        false
                    } else {
                        entries_to_renew.push((key.clone(), slot_mutex.clone()));
                        true
                    }
                } else {
                    // keep leases we couldn't lock, we are going to proccess them on the next pass
                    true
                }
            })
            .await;

        debug!("releasing {} unneeded leases", leases_to_release.len());

        // phase 2: release dead leases
        for lease in leases_to_release {
            self.try_release_lease_if_needed(lease).await;
        }

        // phase 3: renew live leases before they expire.
        // Bounded leases are batched; unlimited leases are re-acquired individually.
        let renewal_threshold = chrono::Duration::from_std(self.renewal_interval * 2)
            .expect("renewal_interval should result in valid renewal_threshold");

        let mut bounded_to_renew: Vec<(ResourceKey, Arc<Mutex<LeaseEntry>>)> = Vec::new();
        let mut unlimited_to_renew: Vec<(ResourceKey, Arc<Mutex<LeaseEntry>>)> = Vec::new();

        for (key, slot_mutex) in &entries_to_renew {
            let slot = slot_mutex.lock().await;
            match &slot.lease {
                TrackedLease::Bounded(b) => {
                    if b.expires_at - Utc::now() >= renewal_threshold {
                        continue; // Plenty of time left — skip until closer to expiry.
                    }
                    bounded_to_renew.push((key.clone(), slot_mutex.clone()));
                }
                TrackedLease::Unlimited(u) => {
                    if u.expires_at - Utc::now() >= renewal_threshold {
                        continue;
                    }
                    unlimited_to_renew.push((key.clone(), slot_mutex.clone()));
                }
                TrackedLease::Lost => continue,
            }
        }

        // Batch-renew all bounded leases in one RPC.
        if !bounded_to_renew.is_empty() {
            let mut batch: Vec<BatchRenewalEntry> = Vec::with_capacity(bounded_to_renew.len());
            for (_, slot_mutex) in &bounded_to_renew {
                let slot = slot_mutex.lock().await;
                if let TrackedLease::Bounded(b) = &slot.lease {
                    batch.push(BatchRenewalEntry {
                        resource_definition_id: b.resource_definition_id,
                        epoch: b.epoch.0,
                        unused: b.remaining,
                        pending_reservations: slot
                            .waiters
                            .iter()
                            .map(|w| PendingReservation {
                                amount: w.amount,
                                priority: w.priority(),
                            })
                            .collect(),
                    });
                }
            }

            match self
                .client
                .batch_renew_quota_leases(self.port, batch)
                .await
            {
                Ok(results) => {
                    for ((key, slot_mutex), result) in
                        bounded_to_renew.iter().zip(results.into_iter())
                    {
                        let mut slot = slot_mutex.lock().await;
                        let expires_at = if let TrackedLease::Bounded(b) = &slot.lease {
                            b.expires_at
                        } else {
                            continue;
                        };
                        let resource_definition_id =
                            if let TrackedLease::Bounded(b) = &slot.lease {
                                b.resource_definition_id
                            } else {
                                continue;
                            };
                        match result {
                            Ok(new_lease) => {
                                if let QuotaLease::Bounded { allocation, total_available_amount, .. } = &new_lease {
                                    debug!(
                                        resource_definition_id = %resource_definition_id,
                                        allocation,
                                        total_available = total_available_amount,
                                        waiters = slot.waiters.len(),
                                        "renew_all: received new allocation"
                                    );
                                }
                                slot.lease = Self::from_quota_lease(&new_lease);
                                self.process_waiters(&mut slot, key);
                            }
                            Err(QuotaError::LeaseNotFound(_) | QuotaError::StaleEpoch(_)) => {
                                tracing::warn!(
                                    resource_definition_id = %resource_definition_id,
                                    "Lease lost during batch renewal"
                                );
                                slot.lease = TrackedLease::Lost;
                            }
                            Err(err) => {
                                tracing::error!(
                                    resource_definition_id = %resource_definition_id,
                                    error = %err,
                                    "Failed to renew bounded quota lease in batch"
                                );
                                if Utc::now() >= expires_at {
                                    slot.lease = TrackedLease::Lost;
                                }
                            }
                        }
                    }
                }
                Err(err) => {
                    tracing::error!(error = %err, "batch_renew_quota_leases failed entirely");
                }
            }
        }

        // Renew unlimited leases individually
        for (key, slot_mutex) in &unlimited_to_renew {
            let (environment_id, resource_name) = key;
            let mut slot = slot_mutex.lock().await;
            if let TrackedLease::Unlimited(u) = &slot.lease {
                let expires_at = u.expires_at;
                match self
                    .client
                    .acquire_quota_lease(*environment_id, resource_name.clone(), self.port)
                    .await
                {
                    Ok(new_lease) => {
                        slot.lease = Self::from_quota_lease(&new_lease);
                        self.process_waiters(&mut slot, key);
                    }
                    Err(err) => {
                        tracing::error!(error = %err, "Failed to renew unlimited quota lease");
                        if Utc::now() >= expires_at {
                            slot.lease = TrackedLease::Lost;
                        }
                    }
                }
            }
        }

        // phase 4: re-acquire all Lost leases that still have live interest.
        // Waiters are kept in place and served once a new lease arrives.
        // If re-acquire fails we leave the entry as Lost and retry next loop.
        for (key, slot_mutex) in &entries_to_renew {
            let (environment_id, resource_name) = key;
            let is_lost = matches!(slot_mutex.lock().await.lease, TrackedLease::Lost);
            if !is_lost {
                continue;
            }

            debug!("trying to reacquire lost lease {environment_id}/{resource_name}");

            match self
                .client
                .acquire_quota_lease(*environment_id, resource_name.clone(), self.port)
                .await
            {
                Ok(new_lease) => {
                    let mut slot = slot_mutex.lock().await;
                    // Only update if still Lost — another concurrent path won't exist
                    // given the single renewal task, but guard defensively.
                    if matches!(slot.lease, TrackedLease::Lost) {
                        tracing::info!(
                            environment_id = %environment_id,
                            resource_name = %resource_name,
                            "Re-acquired lost lease"
                        );
                        slot.lease = Self::from_quota_lease(&new_lease);
                        self.process_waiters(&mut slot, key);
                    }
                }
                Err(err) => {
                    tracing::warn!(
                        environment_id = %environment_id,
                        resource_name = %resource_name,
                        error = %err,
                        "Re-acquire of lost lease failed, will retry next loop"
                    );
                    // Leave as Lost — the next renewal cycle will try again.
                }
            }
        }
    }

    async fn try_release_lease_if_needed(&self, tracked_lease: TrackedLease) {
        // unlimited leases do not reserve any capacity, so we can just let them expire
        if let TrackedLease::Bounded(lease) = tracked_lease
            && let Err(err) = self
                .client
                .release_quota_lease(
                    lease.resource_definition_id,
                    self.port,
                    lease.epoch.0,
                    lease.remaining,
                )
                .await
        {
            tracing::warn!(
                resource_definition_id = %lease.resource_definition_id,
                error = %err,
                "Failed to release dead quota lease"
            );
        }
    }
}

#[async_trait]
impl QuotaService for GrpcQuotaService {
    async fn acquire(
        &self,
        environment_id: EnvironmentId,
        resource_name: ResourceName,
        expected_use: u64,
        previous_credit: i64,
        previous_credit_at: Option<DateTime<Utc>>,
    ) -> Result<LeaseInterest, QuotaError> {
        debug!("acquiring lease for {}/{}", environment_id, resource_name);

        let key: ResourceKey = (environment_id, resource_name.clone());

        let credit_rate = expected_use as f64 * CREDIT_RATE_FACTOR;
        let max_credit = max_credit_for(expected_use);
        let make_interest = |arc: Arc<()>| LeaseInterest {
            environment_id: key.0,
            resource_name: key.1.clone(),
            expected_use,
            last_credit_value: previous_credit.min(max_credit),
            last_credit_value_at: previous_credit_at.unwrap_or_else(Utc::now),
            credit_rate,
            max_credit,
            _token: arc,
        };

        // fast path: existing live entry -> reuse the lease with fresh credit state
        if let Some(slot_mutex) = self.get_slot(&key).await {
            debug!("Reusing existing lease");
            let mut slot = slot_mutex.lock().await;
            if let Some(arc) = slot.interest.upgrade() {
                return Ok(make_interest(arc));
            }

            // interest is dead, but the lease has not been collected yet. We are holding the lock, so we can revive it and prevent
            // a pointless reacquire
            if !matches!(slot.lease, TrackedLease::Lost) {
                let arc = Arc::new(());
                slot.interest = Arc::downgrade(&arc);
                return Ok(make_interest(arc));
            }

            // entry is completely dead; fall through to re-acquire
        }

        // Slow path: acquire from shard manager.
        debug!("Acquiring new lease from shard manager");

        let arc = Arc::new(());
        let placeholder_mutex = Arc::new(Mutex::new(LeaseEntry {
            interest: Arc::downgrade(&arc),
            waiters: Vec::new(),
            lease: TrackedLease::Lost, // filled in below
        }));

        // Lock the entry before inserting so concurrent waiters block on the mutex.
        let mut slot = placeholder_mutex.lock().await;

        match self.state.entry_async(key.clone()).await {
            scc::hash_map::Entry::Occupied(occ) => {
                // Another concurrent acquire inserted first — drop our placeholder
                // and reuse theirs.
                drop(slot);
                debug!("Concurrent acquire raced — reusing existing entry");
                let slot_mutex = occ.get().clone();
                // Release the scc bucket lock before locking the per-entry mutex.
                drop(occ);
                let mut slot = slot_mutex.lock().await;
                if let Some(existing_arc) = slot.interest.upgrade() {
                    return Ok(make_interest(existing_arc));
                }
                if !matches!(slot.lease, TrackedLease::Lost) {
                    let new_arc = Arc::new(());
                    slot.interest = Arc::downgrade(&new_arc);
                    return Ok(make_interest(new_arc));
                }
                // Entry is lost — proceed with our own acquire below,
                // but we need to re-insert.  Use the existing slot_mutex.
                let new_lease = self
                    .client
                    .acquire_quota_lease(environment_id, resource_name, self.port)
                    .await?;
                let new_arc = Arc::new(());
                let interest = make_interest(new_arc.clone());
                slot.interest = Arc::downgrade(&new_arc);
                slot.lease = Self::from_quota_lease(&new_lease);
                return Ok(interest);
            }
            scc::hash_map::Entry::Vacant(vac) => {
                // We are the first — insert the prelocked placeholder, then
                // release the scc bucket lock before making the RPC.
                vac.insert_entry(placeholder_mutex.clone());
                // scc bucket lock released here as `vac` is dropped.
            }
        }

        // Make the RPC without holding any scc lock.
        let rpc_result = self
            .client
            .acquire_quota_lease(environment_id, resource_name, self.port)
            .await;

        match rpc_result {
            Ok(new_lease) => {
                let interest = make_interest(arc.clone());
                slot.interest = Arc::downgrade(&arc);
                slot.lease = Self::from_quota_lease(&new_lease);
                Ok(interest)
            }
            Err(e) => {
                // RPC failed — mark the placeholder as Lost so concurrent
                // waiters don't spin forever, then remove the entry.
                slot.lease = TrackedLease::Lost;
                drop(slot);
                self.state.remove_async(&key).await;
                Err(e)
            }
        }
    }

    async fn try_reserve(&self, interest: &mut LeaseInterest, amount: u64) -> ReserveResult {
        debug!(
            "reserving {amount} for {}/{}",
            interest.environment_id, interest.resource_name
        );

        let key: ResourceKey = (interest.environment_id, interest.resource_name.clone());

        let slot_mutex = self
            .get_slot(&key)
            .await
            .expect("try_reserve called without a prior acquire for this resource");

        let mut slot = slot_mutex.lock().await;

        // Enqueue and let process_waiters try to serve immediately.
        let (tx, rx) = oneshot::channel();
        slot.waiters.push(Waiter::from_interest(amount, interest, tx));
        self.process_waiters(&mut slot, &key);

        // If we're still waiting, notify the shard manager immediately so it
        // can factor this executor's demand into allocation for other executors.
        // We still hold the lock so the renewal sees the complete waiter list.
        if !rx.is_terminated() && rx.is_empty() {
            self.notify_demand(&key, &mut slot).await;
        }

        drop(slot);

        debug!("awaiting waiter outcome");
        match rx.await {
            Ok(WaiterOutcome::Granted(reservation)) => {
                interest.debit(amount);
                ReserveResult::Ok(reservation)
            }
            Ok(WaiterOutcome::Insufficient {
                enforcement_action,
                estimated_wait_time,
            }) => ReserveResult::InsufficientAllocation {
                enforcement_action,
                estimated_wait_nanos: estimated_wait_time.map(|d| d.as_nanos() as u64),
            },
            Err(e) => panic!("quota waiter channel dropped unexpectedly: {e}"),
        }
    }

    async fn commit(&self, interest: &mut LeaseInterest, reservation: Reservation, used: u64) {
        let (key, reserved, reservation_epoch) = match reservation {
            Reservation::Unlimited => return,
            Reservation::Bounded {
                environment_id,
                resource_name,
                epoch,
                reserved,
                resource_definition_id: _,
            } => ((environment_id, resource_name), reserved, epoch),
        };

        assert_eq!(
            key.0, interest.environment_id,
            "commit must be called with consistent interest and reservation"
        );
        assert_eq!(
            key.1, interest.resource_name,
            "commit must be called with consistent interest and reservation"
        );

        if let Some(slot_mutex) = self.get_slot(&key).await {
            let mut slot = slot_mutex.lock().await;
            let credit_back_amount = match &mut slot.lease {
                TrackedLease::Bounded(lease) => match used.cmp(&reserved) {
                    std::cmp::Ordering::Greater => {
                        let excess = used - reserved;
                        lease.remaining = lease.remaining.saturating_sub(excess);
                        None
                    }
                    std::cmp::Ordering::Less => {
                        // we can only process refunds if the epochs match, otherwise the capacity
                        // might have already been lost due to bucket limits etc.
                        if lease.epoch == reservation_epoch {
                            let credit = reserved - used;
                            debug!("Returning {credit} credits to {}/{}", key.0, key.1);
                            lease.remaining += credit;
                            Some(credit)
                        } else {
                            None
                        }
                    }
                    std::cmp::Ordering::Equal => None,
                },
                TrackedLease::Unlimited(_) | TrackedLease::Lost => None,
            };

            if let Some(amount) = credit_back_amount {
                interest.credit_back(amount);
            }

            if credit_back_amount.is_some() && !slot.waiters.is_empty() {
                self.process_waiters(&mut slot, &key);
            }
        }
    }
}

pub struct UnlimitedQuotaService;

#[async_trait]
impl QuotaService for UnlimitedQuotaService {
    async fn acquire(
        &self,
        environment_id: EnvironmentId,
        resource_name: ResourceName,
        expected_use: u64,
        previous_credit: i64,
        previous_credit_at: Option<DateTime<Utc>>,
    ) -> Result<LeaseInterest, QuotaError> {
        let max_credit = max_credit_for(expected_use);
        Ok(LeaseInterest {
            environment_id,
            resource_name,
            expected_use,
            last_credit_value: previous_credit.min(max_credit),
            last_credit_value_at: previous_credit_at.unwrap_or_else(Utc::now),
            credit_rate: expected_use as f64 * CREDIT_RATE_FACTOR,
            max_credit,
            _token: Arc::new(()),
        })
    }

    async fn try_reserve(&self, _interest: &mut LeaseInterest, _amount: u64) -> ReserveResult {
        ReserveResult::Ok(Reservation::Unlimited)
    }

    async fn commit(&self, _interest: &mut LeaseInterest, _reservation: Reservation, _used: u64) {}
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration as ChronoDuration;
    use golem_common::model::quota::{ResourceLimit, ResourceRateLimit, TimePeriod};
    use golem_common::model::{Pod, RoutingTable};
    use golem_service_base::clients::shard_manager::ShardManagerError;
    use pretty_assertions::assert_eq;
    use pretty_assertions::assert_matches;
    use std::net::{IpAddr, Ipv4Addr};
    use std::sync::Mutex as StdMutex;
    use test_r::test;

    test_r::enable!();

    fn test_env_id() -> EnvironmentId {
        EnvironmentId::new()
    }

    fn test_resource_name() -> ResourceName {
        ResourceName("test-resource".to_string())
    }

    fn test_pod() -> Pod {
        Pod {
            ip: IpAddr::V4(Ipv4Addr::LOCALHOST),
            port: 9093,
        }
    }

    fn test_resource_definition_id() -> ResourceDefinitionId {
        ResourceDefinitionId::new()
    }

    fn bounded_lease(
        resource_definition_id: ResourceDefinitionId,
        epoch: u64,
        amount: u64,
    ) -> QuotaLease {
        bounded_lease_with_total(resource_definition_id, epoch, amount, amount)
    }

    fn bounded_lease_with_total(
        resource_definition_id: ResourceDefinitionId,
        epoch: u64,
        amount: u64,
        total_available_amount: u64,
    ) -> QuotaLease {
        QuotaLease::Bounded {
            resource_definition_id,
            pod: test_pod(),
            epoch: LeaseEpoch(epoch),
            allocation: amount,
            // Use an expiry just within the 2×renewal_interval threshold
            // (test services use 100ms interval → 200ms threshold) so that
            // tests calling renew_all() always trigger a renewal.
            expires_at: Utc::now() + ChronoDuration::milliseconds(150),
            resource_limit: ResourceLimit::Rate(ResourceRateLimit {
                value: 1000,
                period: TimePeriod::Second,
                max: 1000,
            }),
            enforcement_action: EnforcementAction::Throttle,
            total_available_amount,
        }
    }

    /// Lease with a very slow refill rate (1 token/hour).
    /// Any deficit ≥ 1 → estimated_wait ≥ 3600s, well above a 60s threshold.
    fn slow_rate_lease_with_total(
        resource_definition_id: ResourceDefinitionId,
        epoch: u64,
        amount: u64,
        total_available_amount: u64,
    ) -> QuotaLease {
        QuotaLease::Bounded {
            resource_definition_id,
            pod: test_pod(),
            epoch: LeaseEpoch(epoch),
            allocation: amount,
            expires_at: Utc::now() + ChronoDuration::milliseconds(150),
            resource_limit: ResourceLimit::Rate(ResourceRateLimit {
                value: 1,
                period: TimePeriod::Hour,
                max: 1,
            }),
            enforcement_action: EnforcementAction::Throttle,
            total_available_amount,
        }
    }

    /// Capacity lease (no refill — deficit can only be resolved by release).
    fn capacity_lease_with_total(
        resource_definition_id: ResourceDefinitionId,
        epoch: u64,
        amount: u64,
        total_available_amount: u64,
    ) -> QuotaLease {
        use golem_common::model::quota::ResourceCapacityLimit;
        QuotaLease::Bounded {
            resource_definition_id,
            pod: test_pod(),
            epoch: LeaseEpoch(epoch),
            allocation: amount,
            expires_at: Utc::now() + ChronoDuration::milliseconds(150),
            resource_limit: ResourceLimit::Capacity(ResourceCapacityLimit { value: 100 }),
            enforcement_action: EnforcementAction::Throttle,
            total_available_amount,
        }
    }

    fn unlimited_lease() -> QuotaLease {
        QuotaLease::Unlimited {
            pod: test_pod(),
            expires_at: Utc::now() + ChronoDuration::minutes(5),
        }
    }

    type RenewFn =
        Box<dyn Fn(ResourceDefinitionId, u64, u64) -> Result<QuotaLease, QuotaError> + Send + Sync>;

    struct MockShardManager {
        acquire_response: StdMutex<Option<Result<QuotaLease, QuotaError>>>,
        renew_fn: StdMutex<Option<RenewFn>>,
        released: StdMutex<Vec<(ResourceDefinitionId, u64, u64)>>,
    }

    impl MockShardManager {
        fn new() -> Self {
            Self {
                acquire_response: StdMutex::new(None),
                renew_fn: StdMutex::new(None),
                released: StdMutex::new(Vec::new()),
            }
        }

        fn with_acquire(self, response: Result<QuotaLease, QuotaError>) -> Self {
            *self.acquire_response.lock().unwrap() = Some(response);
            self
        }

        fn with_renew_fn(
            self,
            f: impl Fn(ResourceDefinitionId, u64, u64) -> Result<QuotaLease, QuotaError>
            + Send
            + Sync
            + 'static,
        ) -> Self {
            *self.renew_fn.lock().unwrap() = Some(Box::new(f));
            self
        }

        fn releases(&self) -> Vec<(ResourceDefinitionId, u64, u64)> {
            self.released.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl ShardManager for MockShardManager {
        async fn get_routing_table(&self) -> Result<RoutingTable, ShardManagerError> {
            unimplemented!()
        }

        async fn register(
            &self,
            _port: u16,
            _pod_name: Option<String>,
        ) -> Result<u32, ShardManagerError> {
            unimplemented!()
        }

        async fn acquire_quota_lease(
            &self,
            _environment_id: EnvironmentId,
            _resource_name: ResourceName,
            _port: u16,
        ) -> Result<QuotaLease, QuotaError> {
            self.acquire_response
                .lock()
                .unwrap()
                .clone()
                .expect("acquire_response not configured")
        }

        async fn renew_quota_lease(
            &self,
            resource_definition_id: ResourceDefinitionId,
            _port: u16,
            epoch: u64,
            unused: u64,
            _pending_reservations: Vec<PendingReservation>,
        ) -> Result<QuotaLease, QuotaError> {
            let guard = self.renew_fn.lock().unwrap();
            let f = guard.as_ref().expect("renew_fn not configured");
            f(resource_definition_id, epoch, unused)
        }

        async fn batch_renew_quota_leases(
            &self,
            port: u16,
            renewals: Vec<BatchRenewalEntry>,
        ) -> Result<Vec<Result<QuotaLease, QuotaError>>, ShardManagerError> {
            let mut results = Vec::with_capacity(renewals.len());
            for entry in renewals {
                results.push(
                    self.renew_quota_lease(
                        entry.resource_definition_id,
                        port,
                        entry.epoch,
                        entry.unused,
                        entry.pending_reservations,
                    )
                    .await,
                );
            }
            Ok(results)
        }

        async fn release_quota_lease(
            &self,
            resource_definition_id: ResourceDefinitionId,
            _port: u16,
            epoch: u64,
            unused: u64,
        ) -> Result<(), QuotaError> {
            self.released
                .lock()
                .unwrap()
                .push((resource_definition_id, epoch, unused));
            Ok(())
        }
    }

    /// Service without a running renewal loop. Tests drive `renew_all()` manually.
    fn make_service(mock: MockShardManager) -> Arc<GrpcQuotaService> {
        GrpcQuotaService::new_inner(
            Arc::new(mock),
            9093,
            Duration::from_millis(100),
            Duration::from_secs(60),
        )
    }

    #[test]
    async fn acquire_bounded_lease() {
        let rid = test_resource_definition_id();
        let mock = MockShardManager::new().with_acquire(Ok(bounded_lease(rid, 1, 100)));
        let svc = make_service(mock);

        let interest = svc
            .acquire(test_env_id(), test_resource_name(), 100, 0, None)
            .await
            .unwrap();
        assert_eq!(interest.resource_name, test_resource_name());
    }

    #[test]
    async fn acquire_is_idempotent() {
        let rid = test_resource_definition_id();
        let mock = MockShardManager::new().with_acquire(Ok(bounded_lease(rid, 1, 100)));
        let svc = make_service(mock);

        let env_id = test_env_id();
        let name = test_resource_name();

        let mut interest1 = svc
            .acquire(env_id, name.clone(), 100, 0, None)
            .await
            .unwrap();
        let mut interest2 = svc
            .acquire(env_id, name.clone(), 100, 0, None)
            .await
            .unwrap();

        let r1 = svc.try_reserve(&mut interest1, 50).await;
        assert_matches!(
            r1,
            ReserveResult::Ok(Reservation::Bounded { reserved: 50, .. })
        );

        let r2 = svc.try_reserve(&mut interest2, 50).await;
        assert_matches!(
            r2,
            ReserveResult::Ok(Reservation::Bounded { reserved: 50, .. })
        );
        // 100 units fully consumed — exhaustion behaviour is tested in reserve_drains_allocation.
    }

    #[test]
    async fn reserve_bounded_success() {
        let rid = test_resource_definition_id();
        let mock = MockShardManager::new().with_acquire(Ok(bounded_lease(rid, 1, 100)));
        let svc = make_service(mock);

        let mut interest = svc
            .acquire(test_env_id(), test_resource_name(), 100, 0, None)
            .await
            .unwrap();
        let result = svc.try_reserve(&mut interest, 30).await;
        match result {
            ReserveResult::Ok(Reservation::Bounded {
                resource_definition_id,
                epoch,
                reserved,
                ..
            }) => {
                assert_eq!(resource_definition_id, rid);
                assert_eq!(epoch, LeaseEpoch(1));
                assert_eq!(reserved, 30);
            }
            other => panic!("Expected Bounded reservation, got {:?}", other),
        }
    }

    #[test]
    async fn reserve_unlimited() {
        let mock = MockShardManager::new().with_acquire(Ok(unlimited_lease()));
        let svc = make_service(mock);
        let mut interest = svc
            .acquire(test_env_id(), test_resource_name(), 100, 0, None)
            .await
            .unwrap();
        assert_matches!(
            svc.try_reserve(&mut interest, 999).await,
            ReserveResult::Ok(Reservation::Unlimited)
        );
    }

    #[test]
    async fn reserve_drains_allocation() {
        let rid = test_resource_definition_id();
        // Capacity resource. Renewal returns total_available=0 so the
        // waiter is rejected (no refill, estimated_wait=None → always reject).
        let mock = Arc::new(
            MockShardManager::new()
                .with_acquire(Ok(capacity_lease_with_total(rid, 1, 100, 100)))
                .with_renew_fn(move |_, _, _| Ok(capacity_lease_with_total(rid, 2, 0, 0))),
        );
        let svc = GrpcQuotaService::new_inner(
            mock.clone(),
            9093,
            Duration::from_millis(100),
            Duration::from_secs(60),
        );
        let mut interest = svc
            .acquire(test_env_id(), test_resource_name(), 100, 0, None)
            .await
            .unwrap();

        assert_matches!(
            svc.try_reserve(&mut interest, 40).await,
            ReserveResult::Ok(_)
        );
        assert_matches!(
            svc.try_reserve(&mut interest, 40).await,
            ReserveResult::Ok(_)
        );
        assert_matches!(
            svc.try_reserve(&mut interest, 20).await,
            ReserveResult::Ok(_)
        );

        // Allocation exhausted. Spawn a waiter for 1 unit — it enters the queue.
        let svc2 = svc.clone();
        let mut interest2 = interest.clone();
        let task = tokio::spawn(async move { svc2.try_reserve(&mut interest2, 1).await });

        // Wait until it enters the queue.
        let key: ResourceKey = (interest.environment_id, interest.resource_name.clone());
        loop {
            tokio::task::yield_now().await;
            let slot_mutex = svc.get_slot(&key).await.unwrap();
            let slot = slot_mutex.lock().await;
            if !slot.waiters.is_empty() {
                break;
            }
        }

        // Renewal returns total_available=0, capacity resource → reject.
        svc.renew_all().await;

        let result = task.await.unwrap();
        assert!(
            matches!(result, ReserveResult::InsufficientAllocation { .. }),
            "got {:?}",
            result
        );
    }

    #[test]
    async fn reserve_globally_impossible_rejected_after_renewal() {
        let rid = test_resource_definition_id();
        // Slow rate (1/hour), total_available=10 < amount=11.
        // deficit=1 → estimated_wait=3600s >> 60s threshold → rejected by process_waiters.
        let svc = GrpcQuotaService::new_inner(
            Arc::new(
                MockShardManager::new()
                    .with_acquire(Ok(slow_rate_lease_with_total(rid, 1, 0, 10)))
                    .with_renew_fn(move |_, _, _| Ok(slow_rate_lease_with_total(rid, 2, 0, 10))),
            ),
            9093,
            Duration::from_millis(100),
            Duration::from_secs(60),
        );
        let interest = svc
            .acquire(test_env_id(), test_resource_name(), 100, 0, None)
            .await
            .unwrap();

        let svc2 = svc.clone();
        let mut interest2 = interest.clone();
        let task = tokio::spawn(async move { svc2.try_reserve(&mut interest2, 11).await });

        let key: ResourceKey = (interest.environment_id, interest.resource_name.clone());
        loop {
            tokio::task::yield_now().await;
            let slot = svc.get_slot(&key).await.unwrap();
            if !slot.lock().await.waiters.is_empty() {
                break;
            }
        }

        svc.renew_all().await;

        let result = task.await.unwrap();
        assert_matches!(
            result,
            ReserveResult::InsufficientAllocation {
                estimated_wait_nanos: Some(_),
                ..
            }
        );
    }

    #[test]
    async fn waiter_rejected_when_globally_impossible_after_renewal() {
        let rid = test_resource_definition_id();
        // Acquire: total_available = 100 → waiter can enter queue.
        // Renewal: total_available drops to 5, slow rate (1/hour) →
        //          estimated wait = 5 × 3600s = 18000s >> 60s threshold → reject.
        let mock = Arc::new(
            MockShardManager::new()
                .with_acquire(Ok(bounded_lease_with_total(rid, 1, 0, 100)))
                .with_renew_fn(move |_, _, _| Ok(slow_rate_lease_with_total(rid, 2, 0, 5))),
        );

        let svc = GrpcQuotaService::new_inner(
            mock.clone(),
            9093,
            Duration::from_millis(100),
            Duration::from_secs(60),
        );

        let interest = svc
            .acquire(test_env_id(), test_resource_name(), 100, 0, None)
            .await
            .unwrap();

        // Add a waiter for 10 units — possible at acquire time, rejected after renewal.
        let (tx, rx) = oneshot::channel::<WaiterOutcome>();
        {
            let key: ResourceKey = (interest.environment_id, interest.resource_name.clone());
            let slot_mutex = svc.get_slot(&key).await.unwrap();
            let mut slot = slot_mutex.lock().await;
            slot.waiters.push(Waiter::with_credit(10, 0, tx));
        }

        svc.renew_all().await;

        let result = rx.await.unwrap();
        assert_matches!(
            result,
            WaiterOutcome::Insufficient {
                estimated_wait_time: Some(_),
                ..
            }
        );
    }

    #[test]
    async fn waiters_served_in_priority_order() {
        let rid = test_resource_definition_id();
        // Initial: 0 allocation, total = 100.
        // Renewal gives 30 — enough for one 20-unit waiter.
        let mock = Arc::new(
            MockShardManager::new()
                .with_acquire(Ok(bounded_lease_with_total(rid, 1, 0, 100)))
                .with_renew_fn(move |_, _, _| Ok(bounded_lease(rid, 2, 30))),
        );

        let svc = GrpcQuotaService::new_inner(
            mock.clone(),
            9093,
            Duration::from_millis(100),
            Duration::from_secs(60),
        );

        let interest = svc
            .acquire(test_env_id(), test_resource_name(), 100, 0, None)
            .await
            .unwrap();

        // Enqueue two waiters directly to avoid ordering races.
        let (tx_a, mut rx_a) = oneshot::channel::<WaiterOutcome>();
        let (tx_b, rx_b) = oneshot::channel::<WaiterOutcome>();
        {
            let key: ResourceKey = (interest.environment_id, interest.resource_name.clone());
            let slot_mutex = svc.get_slot(&key).await.unwrap();
            let mut slot = slot_mutex.lock().await;
            // Low priority: credit=0, amount=20 → priority 0.0
            slot.waiters.push(Waiter::with_credit(20, 0, tx_a));
            // High priority: credit=1000, amount=20 → priority 50.0
            slot.waiters.push(Waiter::with_credit(20, 1000, tx_b));
        }

        svc.renew_all().await;

        // B (high priority) should be granted.
        let result_b = rx_b.await.unwrap();
        assert_matches!(
            result_b,
            WaiterOutcome::Granted(_),
            "high-priority waiter should be granted"
        );

        // A is still waiting (only 10 units left after B took 20 of 30).
        // It stays in the queue — sender not yet dropped.
        assert!(rx_a.try_recv().is_err());
    }

    #[test]
    async fn spawned_waiter_granted_by_renew_all() {
        let rid = test_resource_definition_id();
        // Acquire gives 0 allocation, 100 available globally — so the
        // waiter enters the queue. Renewal brings 50 allocation.
        let mock = Arc::new(
            MockShardManager::new()
                .with_acquire(Ok(bounded_lease_with_total(rid, 1, 0, 100)))
                .with_renew_fn(move |_, _, _| Ok(bounded_lease(rid, 2, 50))),
        );
        let svc = GrpcQuotaService::new_inner(
            mock.clone(),
            9093,
            Duration::from_millis(100),
            Duration::from_secs(60),
        );

        let interest = svc
            .acquire(test_env_id(), test_resource_name(), 100, 0, None)
            .await
            .unwrap();

        // Spawn try_reserve — it will block because allocation = 0.
        let svc2 = svc.clone();
        let mut interest2 = interest.clone();
        let task = tokio::spawn(async move { svc2.try_reserve(&mut interest2, 30).await });

        // Wait until the waiter is in the queue before calling renew_all.
        let key: ResourceKey = (interest.environment_id, interest.resource_name.clone());
        loop {
            tokio::task::yield_now().await;
            let slot_mutex = svc.get_slot(&key).await.unwrap();
            let slot = slot_mutex.lock().await;
            if !slot.waiters.is_empty() {
                break;
            }
        }

        svc.renew_all().await;

        let result = task.await.unwrap();
        assert_matches!(
            result,
            ReserveResult::Ok(Reservation::Bounded { reserved: 30, .. })
        );
    }

    #[test]
    async fn spawned_waiter_rejected_by_renew_all() {
        let rid = test_resource_definition_id();
        // Acquire: total_available = 100 → waiter can enter queue.
        // Renewal: total_available = 5, slow rate (1/hour) →
        //          estimated wait = 5 × 3600s >> 60s threshold → reject.
        let mock = Arc::new(
            MockShardManager::new()
                .with_acquire(Ok(bounded_lease_with_total(rid, 1, 0, 100)))
                .with_renew_fn(move |_, _, _| Ok(slow_rate_lease_with_total(rid, 2, 0, 5))),
        );
        let svc = GrpcQuotaService::new_inner(
            mock.clone(),
            9093,
            Duration::from_millis(100),
            Duration::from_secs(60),
        );

        let interest = svc
            .acquire(test_env_id(), test_resource_name(), 100, 0, None)
            .await
            .unwrap();

        // Spawn try_reserve — enters the queue because allocation = 0.
        let svc2 = svc.clone();
        let mut interest2 = interest.clone();
        let task = tokio::spawn(async move { svc2.try_reserve(&mut interest2, 10).await });

        // Wait until the waiter is in the queue.
        let key: ResourceKey = (interest.environment_id, interest.resource_name.clone());
        loop {
            tokio::task::yield_now().await;
            let slot_mutex = svc.get_slot(&key).await.unwrap();
            let slot = slot_mutex.lock().await;
            if !slot.waiters.is_empty() {
                break;
            }
        }

        svc.renew_all().await;

        let result = task.await.unwrap();
        assert_matches!(
            result,
            ReserveResult::InsufficientAllocation {
                estimated_wait_nanos: Some(_),
                ..
            }
        );
    }

    #[test]
    async fn waiter_served_on_commit_without_renewal() {
        let rid = test_resource_definition_id();
        // Allocation = 50, all consumed up front.
        let mock = MockShardManager::new().with_acquire(Ok(bounded_lease(rid, 1, 50)));
        let svc = make_service(mock);
        let mut interest = svc
            .acquire(test_env_id(), test_resource_name(), 100, 0, None)
            .await
            .unwrap();

        // Drain the allocation.
        let big_reservation = match svc.try_reserve(&mut interest, 50).await {
            ReserveResult::Ok(r) => r,
            other => panic!("Expected Ok, got {:?}", other),
        };

        // Enqueue a waiter for 20 units.
        let (tx, mut rx) = oneshot::channel::<WaiterOutcome>();
        {
            let key: ResourceKey = (interest.environment_id, interest.resource_name.clone());
            let slot_mutex = svc.get_slot(&key).await.unwrap();
            let mut slot = slot_mutex.lock().await;
            slot.waiters.push(Waiter::with_credit(20, 0, tx));
        }

        // Commit with underuse — 30 units credited back. No renewal needed.
        svc.commit(&mut interest, big_reservation, 20).await;

        // Waiter should be resolved immediately.
        let result = rx
            .try_recv()
            .expect("waiter should have been resolved by commit");
        assert!(
            matches!(
                result,
                WaiterOutcome::Granted(Reservation::Bounded { reserved: 20, .. })
            ),
            "got {:?}",
            result
        );
    }

    #[test]
    async fn credit_debited_on_immediate_grant() {
        let rid = test_resource_definition_id();
        let mock = MockShardManager::new().with_acquire(Ok(bounded_lease(rid, 1, 100)));
        let svc = make_service(mock);

        // expected_use=100 → credit_rate=10/ms, max_credit=10_000
        let mut interest = svc
            .acquire(test_env_id(), test_resource_name(), 100, 500, None)
            .await
            .unwrap();

        // Record credit before reserve.
        let credit_before = interest.current_credit();

        let result = svc.try_reserve(&mut interest, 30).await;
        assert_matches!(result, ReserveResult::Ok(_));

        // Credit should have been debited by 30.
        // (time may have ticked slightly, so we check the snapshot value)
        assert_eq!(interest.last_credit_value, credit_before - 30);
    }

    #[test]
    async fn credit_back_on_underuse_commit() {
        let rid = test_resource_definition_id();
        let mock = MockShardManager::new().with_acquire(Ok(bounded_lease(rid, 1, 100)));
        let svc = make_service(mock);

        let mut interest = svc
            .acquire(test_env_id(), test_resource_name(), 100, 0, None)
            .await
            .unwrap();

        let reservation = match svc.try_reserve(&mut interest, 50).await {
            ReserveResult::Ok(r) => r,
            other => panic!("{:?}", other),
        };

        let credit_after_reserve = interest.current_credit();

        // Used only 10 of 50 reserved — 40 should be credited back.
        svc.commit(&mut interest, reservation, 10).await;

        // Credit after commit should be higher than after reserve.
        assert!(
            interest.last_credit_value > credit_after_reserve,
            "expected credit to increase: before={credit_after_reserve}, after={}",
            interest.last_credit_value
        );
        // Specifically: credited back 40 (on top of whatever time accrued).
        assert!(interest.last_credit_value >= credit_after_reserve + 40);
    }

    #[test]
    async fn credit_not_back_on_epoch_mismatch_commit() {
        let rid = test_resource_definition_id();
        let mock = MockShardManager::new()
            .with_acquire(Ok(bounded_lease(rid, 1, 100)))
            .with_renew_fn(move |_, _, _| Ok(bounded_lease(rid, 2, 200)));
        let svc = make_service(mock);

        let mut interest = svc
            .acquire(test_env_id(), test_resource_name(), 100, 0, None)
            .await
            .unwrap();

        let reservation = match svc.try_reserve(&mut interest, 50).await {
            ReserveResult::Ok(r) => r,
            other => panic!("{:?}", other),
        };
        let credit_after_reserve = interest.last_credit_value;

        // Renewal bumps the epoch.
        svc.renew_all().await;

        // Commit with underuse but stale epoch — no credit back.
        svc.commit(&mut interest, reservation, 10).await;

        // Credit should be unchanged (modulo time accrual, which we ignore
        // by comparing the snapshot value directly).
        assert_eq!(
            interest.last_credit_value, credit_after_reserve,
            "credit should not change on epoch-mismatch commit"
        );
    }

    #[test]
    async fn priority_reflects_time_in_queue() {
        let rid = test_resource_definition_id();
        // Renewal gives 20 units — enough for one 20-unit waiter.
        let mock = Arc::new(
            MockShardManager::new()
                .with_acquire(Ok(bounded_lease_with_total(rid, 1, 0, 100)))
                .with_renew_fn(move |_, _, _| Ok(bounded_lease(rid, 2, 20))),
        );
        let svc = GrpcQuotaService::new_inner(
            mock.clone(),
            9093,
            Duration::from_millis(100),
            Duration::from_secs(60),
        );

        let interest = svc
            .acquire(test_env_id(), test_resource_name(), 100, 0, None)
            .await
            .unwrap();

        // Waiter A: high initial credit (1000), just arrived.
        let (tx_a, mut rx_a) = oneshot::channel::<WaiterOutcome>();
        // Waiter B: low initial credit (-500) but been waiting 200ms
        // (credit_rate = 100 * 0.1 = 10/ms → accrues 2000 credits in 200ms).
        let (tx_b, rx_b) = oneshot::channel::<WaiterOutcome>();
        {
            let key: ResourceKey = (interest.environment_id, interest.resource_name.clone());
            let slot_mutex = svc.get_slot(&key).await.unwrap();
            let mut slot = slot_mutex.lock().await;
            slot.waiters.push(Waiter::with_credit(20, 1000, tx_a));
            // Simulate 200ms wait by backdating last_credit_value_at.
            slot.waiters.push(Waiter {
                amount: 20,
                last_credit_value: -500,
                last_credit_value_at: Utc::now() - ChronoDuration::milliseconds(200),
                credit_rate: 10.0, // 100 * 0.1
                max_credit: 10_000,
                tx: tx_b,
            });
        }

        svc.renew_all().await;

        // B should win because its current_credit ≈ -500 + 200*10 = 1500 > A's 1000.
        let result_b = rx_b.await.unwrap();
        assert_matches!(
            result_b,
            WaiterOutcome::Granted(_),
            "long-waiting waiter B should be granted over A, got {:?}",
            result_b
        );
        // A stays in the queue.
        assert!(rx_a.try_recv().is_err(), "A should still be waiting");
    }

    #[test]
    async fn waiter_stays_queued_when_refill_is_imminent() {
        let rid = test_resource_definition_id();
        // Use a call counter to return different responses on successive renewals.
        let call_count = Arc::new(std::sync::atomic::AtomicU32::new(0));
        let call_count2 = call_count.clone();
        let svc = GrpcQuotaService::new_inner(
            Arc::new(
                MockShardManager::new()
                    .with_acquire(Ok(bounded_lease_with_total(rid, 1, 0, 5)))
                    .with_renew_fn(move |_, _, _| {
                        let n = call_count2.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                        if n == 0 {
                            // First renewal: still insufficient, refill imminent.
                            Ok(bounded_lease_with_total(rid, 2, 0, 5))
                        } else {
                            // Second renewal: capacity available.
                            Ok(bounded_lease(rid, 3, 20))
                        }
                    }),
            ),
            9093,
            Duration::from_millis(100),
            Duration::from_secs(60),
        );

        let interest = svc
            .acquire(test_env_id(), test_resource_name(), 100, 0, None)
            .await
            .unwrap();

        let (tx, mut rx) = oneshot::channel::<WaiterOutcome>();
        {
            let key: ResourceKey = (interest.environment_id, interest.resource_name.clone());
            let slot_mutex = svc.get_slot(&key).await.unwrap();
            let mut slot = slot_mutex.lock().await;
            slot.waiters.push(Waiter::with_credit(10, 0, tx));
        }

        // First renewal: total_available = 5 < 10, but refill is soon → stay queued.
        svc.renew_all().await;
        assert!(
            rx.try_recv().is_err(),
            "waiter should still be queued after first renewal"
        );

        // Second renewal brings 20 allocation — waiter is now served.
        svc.renew_all().await;
        let result = rx.try_recv().expect("waiter should have been resolved");
        assert_matches!(
            result,
            WaiterOutcome::Granted(Reservation::Bounded { reserved: 10, .. })
        );
    }

    #[test]
    async fn waiter_rejected_when_estimated_wait_exceeds_threshold() {
        let rid = test_resource_definition_id();
        // Slow rate: 1 token/hour. deficit = 10 - 5 = 5 → 5 refills × 3600s = 18000s >> 60s.
        let svc = GrpcQuotaService::new_inner(
            Arc::new(
                MockShardManager::new()
                    .with_acquire(Ok(slow_rate_lease_with_total(rid, 1, 0, 5)))
                    .with_renew_fn(move |_, _, _| Ok(slow_rate_lease_with_total(rid, 2, 0, 5))),
            ),
            9093,
            Duration::from_millis(100),
            Duration::from_secs(60),
        );

        let interest = svc
            .acquire(test_env_id(), test_resource_name(), 100, 0, None)
            .await
            .unwrap();

        let (tx, rx) = oneshot::channel::<WaiterOutcome>();
        {
            let key: ResourceKey = (interest.environment_id, interest.resource_name.clone());
            let slot_mutex = svc.get_slot(&key).await.unwrap();
            let mut slot = slot_mutex.lock().await;
            slot.waiters.push(Waiter::with_credit(10, 0, tx));
        }

        // Estimated wait >> 60s threshold → reject.
        svc.renew_all().await;
        let result = rx.await.unwrap();
        assert_matches!(result, WaiterOutcome::Insufficient { .. });
    }

    #[test]
    async fn waiter_rejected_immediately_when_no_refill() {
        let rid = test_resource_definition_id();
        // Capacity resource — no refill, so None → always reject.
        let svc = GrpcQuotaService::new_inner(
            Arc::new(
                MockShardManager::new()
                    .with_acquire(Ok(capacity_lease_with_total(rid, 1, 0, 5)))
                    .with_renew_fn(move |_, _, _| Ok(capacity_lease_with_total(rid, 2, 0, 5))),
            ),
            9093,
            Duration::from_millis(100),
            Duration::from_secs(60),
        );

        let interest = svc
            .acquire(test_env_id(), test_resource_name(), 100, 0, None)
            .await
            .unwrap();

        let (tx, rx) = oneshot::channel::<WaiterOutcome>();
        {
            let key: ResourceKey = (interest.environment_id, interest.resource_name.clone());
            let slot_mutex = svc.get_slot(&key).await.unwrap();
            let mut slot = slot_mutex.lock().await;
            slot.waiters.push(Waiter::with_credit(10, 0, tx));
        }

        // No refill → always reject regardless of threshold.
        svc.renew_all().await;
        let result = rx.await.unwrap();
        assert_matches!(
            result,
            WaiterOutcome::Insufficient {
                estimated_wait_time: None,
                ..
            }
        );
    }

    #[test]
    fn estimated_wait_already_satisfied() {
        use golem_common::model::quota::ResourceRateLimit;
        let limit = ResourceLimit::Rate(ResourceRateLimit {
            value: 100,
            period: TimePeriod::Second,
            max: 100,
        });
        assert_eq!(
            GrpcQuotaService::estimated_wait(50, 100, &limit),
            Some(Duration::ZERO)
        );
    }

    #[test]
    fn estimated_wait_one_refill() {
        use golem_common::model::quota::ResourceRateLimit;
        // deficit = 10, tokens/refill = 100 → 1 refill → 1 second
        let limit = ResourceLimit::Rate(ResourceRateLimit {
            value: 100,
            period: TimePeriod::Second,
            max: 100,
        });
        assert_eq!(
            GrpcQuotaService::estimated_wait(110, 100, &limit),
            Some(Duration::from_secs(1))
        );
    }

    #[test]
    fn estimated_wait_multiple_refills() {
        use golem_common::model::quota::ResourceRateLimit;
        // deficit = 250, tokens/refill = 100 → 3 refills → 3 minutes
        let limit = ResourceLimit::Rate(ResourceRateLimit {
            value: 100,
            period: TimePeriod::Minute,
            max: 1000,
        });
        assert_eq!(
            GrpcQuotaService::estimated_wait(350, 100, &limit),
            Some(Duration::from_mins(3))
        );
    }

    #[test]
    fn estimated_wait_capacity_resource_returns_none() {
        use golem_common::model::quota::ResourceCapacityLimit;
        let limit = ResourceLimit::Capacity(ResourceCapacityLimit { value: 100 });
        assert_eq!(GrpcQuotaService::estimated_wait(200, 50, &limit), None);
    }

    #[test]
    fn estimated_wait_concurrency_resource_returns_none() {
        use golem_common::model::quota::ResourceConcurrencyLimit;
        let limit = ResourceLimit::Concurrency(ResourceConcurrencyLimit { value: 10 });
        assert_eq!(GrpcQuotaService::estimated_wait(20, 5, &limit), None);
    }

    #[test]
    async fn dead_bounded_lease_is_released() {
        let rid = test_resource_definition_id();
        let mock = Arc::new(MockShardManager::new().with_acquire(Ok(bounded_lease(rid, 1, 100))));
        let svc = GrpcQuotaService::new_inner(
            mock.clone(),
            9093,
            Duration::from_millis(100),
            Duration::from_secs(60),
        );

        let mut interest = svc
            .acquire(test_env_id(), test_resource_name(), 100, 0, None)
            .await
            .unwrap();
        // Consume some allocation so unused = 70 on release.
        let _ = svc.try_reserve(&mut interest, 30).await;
        drop(interest);

        svc.renew_all().await;

        let releases = mock.releases();
        assert_eq!(releases.len(), 1);
        assert_eq!(releases[0].0, rid);
        assert_eq!(releases[0].1, 1); // epoch
        assert_eq!(releases[0].2, 70); // unused
    }

    #[test]
    async fn live_lease_is_not_released() {
        let rid = test_resource_definition_id();
        let mock = Arc::new(
            MockShardManager::new()
                .with_acquire(Ok(bounded_lease(rid, 1, 100)))
                .with_renew_fn(move |_, _, _| Ok(bounded_lease(rid, 2, 200))),
        );
        let svc = GrpcQuotaService::new_inner(
            mock.clone(),
            9093,
            Duration::from_millis(100),
            Duration::from_secs(60),
        );

        let interest = svc
            .acquire(test_env_id(), test_resource_name(), 100, 0, None)
            .await
            .unwrap();
        svc.renew_all().await;

        assert!(mock.releases().is_empty());
        // Lease was renewed — new epoch 2, allocation 200.
        let mut interest = interest;
        assert_matches!(
            svc.try_reserve(&mut interest, 200).await,
            ReserveResult::Ok(Reservation::Bounded { epoch, .. }) if epoch == LeaseEpoch(2)
        );
    }

    #[test]
    async fn acquire_with_previous_credit_at_sets_baseline() {
        let rid = test_resource_definition_id();
        let mock = MockShardManager::new().with_acquire(Ok(bounded_lease(rid, 1, 100)));
        let svc = make_service(mock);

        // Set credit baseline 1000ms in the past with credit_rate=10/ms.
        // Expected current_credit = min(max, -100 + 1000 * 10) = min(10000, 9900) = 9900.
        let past = Utc::now() - ChronoDuration::milliseconds(1000);
        let interest = svc
            .acquire(test_env_id(), test_resource_name(), 100, -100, Some(past))
            .await
            .unwrap();

        // Credit should reflect ~1000ms of accrual from the past baseline.
        let credit = interest.current_credit();
        assert!(credit >= 9800, "expected credit ~9900, got {credit}");
    }

    #[test]
    async fn lost_lease_is_reacquired_and_waiters_served() {
        let rid = test_resource_definition_id();
        let call_count = Arc::new(std::sync::atomic::AtomicU32::new(0));
        let call_count2 = call_count.clone();
        let svc = GrpcQuotaService::new_inner(
            Arc::new(
                MockShardManager::new()
                    .with_acquire(Ok(bounded_lease(rid, 1, 50)))
                    .with_renew_fn(move |_, _, _| {
                        // First renew: simulate lease-not-found → triggers Lost.
                        let n = call_count2.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                        if n == 0 {
                            Err(QuotaError::LeaseNotFound("gone".to_string()))
                        } else {
                            // Re-acquire (phase 4) returns a fresh lease.
                            Ok(bounded_lease(rid, 2, 30))
                        }
                    }),
            ),
            9093,
            Duration::from_millis(100),
            Duration::from_secs(60),
        );

        let interest = svc
            .acquire(test_env_id(), test_resource_name(), 100, 0, None)
            .await
            .unwrap();

        // Enqueue a waiter while the lease is still valid.
        let (tx, rx) = oneshot::channel::<WaiterOutcome>();
        {
            let key: ResourceKey = (interest.environment_id, interest.resource_name.clone());
            let slot_mutex = svc.get_slot(&key).await.unwrap();
            let mut slot = slot_mutex.lock().await;
            slot.waiters.push(Waiter::with_credit(20, 0, tx));
        }

        // renew_all: phase 3 marks lease as Lost; phase 4 re-acquires.
        svc.renew_all().await;

        let result = rx.await.unwrap();
        assert_matches!(
            result,
            WaiterOutcome::Granted(Reservation::Bounded { reserved: 20, .. })
        );
    }

    #[test]
    async fn commit_on_lost_lease_is_noop() {
        // Configure so that renewal fails (LeaseNotFound) but re-acquire also
        // fails, keeping the lease in Lost state through the commit.
        let svc = GrpcQuotaService::new_inner(
            Arc::new(
                MockShardManager::new()
                    .with_acquire(Err(QuotaError::LeaseNotFound("gone".to_string())))
                    .with_renew_fn(move |_, _, _| {
                        Err(QuotaError::LeaseNotFound("gone".to_string()))
                    }),
            ),
            9093,
            Duration::from_millis(100),
            Duration::from_secs(60),
        );

        // Manually insert a bounded lease so try_reserve can succeed.
        let key: ResourceKey = (test_env_id(), test_resource_name());
        let arc = Arc::new(());
        let max_credit = 100i64 * CREDIT_MAX_FACTOR;
        let interest_arc = Arc::new(());
        svc.state
            .upsert_async(
                key.clone(),
                Arc::new(Mutex::new(LeaseEntry {
                    interest: Arc::downgrade(&interest_arc),
                    waiters: Vec::new(),
                    lease: TrackedLease::Bounded(BoundedLease {
                        resource_definition_id: test_resource_definition_id(),
                        epoch: LeaseEpoch(1),
                        remaining: 100,
                        expires_at: Utc::now() + ChronoDuration::milliseconds(150),
                        resource_limit: ResourceLimit::Rate(ResourceRateLimit {
                            value: 1000,
                            period: TimePeriod::Minute,
                            max: 1000,
                        }),
                        enforcement_action: EnforcementAction::Throttle,
                        total_available_amount: 100,
                    }),
                })),
            )
            .await;
        let _ = arc; // keep interest alive

        let mut interest = LeaseInterest {
            environment_id: key.0,
            resource_name: key.1.clone(),
            expected_use: 100,
            last_credit_value: 0,
            last_credit_value_at: Utc::now(),
            credit_rate: 100.0 * CREDIT_RATE_FACTOR,
            max_credit,
            _token: interest_arc,
        };

        let reservation = match svc.try_reserve(&mut interest, 30).await {
            ReserveResult::Ok(r) => r,
            other => panic!("{:?}", other),
        };
        let credit_after_reserve = interest.last_credit_value;

        // Renew fails → lease becomes Lost. Re-acquire also fails → stays Lost.
        svc.renew_all().await;

        // Commit on a Lost lease: should not panic, should not credit back.
        svc.commit(&mut interest, reservation, 10).await;

        assert_eq!(
            interest.last_credit_value, credit_after_reserve,
            "commit on Lost lease should not modify credit"
        );
    }
}
