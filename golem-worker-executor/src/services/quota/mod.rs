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

use super::golem_config::QuotaServiceConfig;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use golem_common::model::environment::EnvironmentId;
use golem_common::model::quota::{
    EnforcementAction, LeaseEpoch, ResourceDefinitionId, ResourceLimit, ResourceName,
};
use golem_common::model::quota::{Reservation, ReserveResult};
use golem_service_base::clients::shard_manager::{
    BatchRenewalEntry, ExecutorEndpoint, QuotaError, ShardManager,
};
use golem_service_base::model::quota_lease::{PendingReservation, QuotaLease};
use itertools::Itertools;
use std::pin::Pin;
use std::sync::{Arc, Mutex as StdMutex, Weak};
use std::time::Duration;
use tokio::sync::{Mutex, RwLock, oneshot};
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
    async fn initialize(&self, _endpoint: ExecutorEndpoint) {}

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
    ) -> LeaseInterest;

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

pub struct LazyQuotaService {
    inner: RwLock<Option<Arc<dyn QuotaService>>>,
    #[allow(clippy::type_complexity)]
    make_inner: Mutex<
        Option<
            Box<
                dyn FnOnce(
                        ExecutorEndpoint,
                    )
                        -> Pin<Box<dyn Future<Output = Arc<dyn QuotaService>> + Send>>
                    + Send
                    + Sync,
            >,
        >,
    >,
}

impl LazyQuotaService {
    pub fn new<F, Fut>(f: F) -> Self
    where
        F: FnOnce(ExecutorEndpoint) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Arc<dyn QuotaService>> + Send,
    {
        Self {
            inner: RwLock::new(None),
            make_inner: Mutex::new(Some(Box::new(|endpoint| {
                Box::pin(async move { f(endpoint).await })
            }))),
        }
    }
}

#[async_trait]
impl QuotaService for LazyQuotaService {
    async fn initialize(&self, endpoint: ExecutorEndpoint) {
        let make_inner = self
            .make_inner
            .lock()
            .await
            .take()
            .expect("LazyQuotaService already initialized");
        let inner = make_inner(endpoint).await;
        let _ = self.inner.write().await.insert(inner);
    }

    async fn acquire(
        &self,
        environment_id: EnvironmentId,
        resource_name: ResourceName,
        expected_use: u64,
        previous_credit: i64,
        previous_credit_at: Option<DateTime<Utc>>,
    ) -> LeaseInterest {
        let lock = self.inner.read().await;
        lock.as_ref()
            .expect("LazyQuotaService not initialized")
            .acquire(
                environment_id,
                resource_name,
                expected_use,
                previous_credit,
                previous_credit_at,
            )
            .await
    }

    async fn try_reserve(&self, interest: &mut LeaseInterest, amount: u64) -> ReserveResult {
        let lock = self.inner.read().await;
        lock.as_ref()
            .expect("LazyQuotaService not initialized")
            .try_reserve(interest, amount)
            .await
    }

    async fn commit(&self, interest: &mut LeaseInterest, reservation: Reservation, used: u64) {
        let lock = self.inner.read().await;
        lock.as_ref()
            .expect("LazyQuotaService not initialized")
            .commit(interest, reservation, used)
            .await
    }
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
    /// Only read/written while holding the scc bucket lock via `retain_async`
    /// or `reuse_slot`.  Uses a `std::sync::Mutex` (non-async, uncontended in
    /// practice since the scc bucket lock already serialises access) so it can
    /// be accessed synchronously without async overhead.
    interest: StdMutex<Weak<()>>,
    /// Guards the mutable lease state and waiter queue.
    inner: Mutex<LeaseInner>,
}

struct LeaseInner {
    waiters: Vec<Waiter>,
    lease: TrackedLease,
}

impl LeaseInner {
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
    endpoint: ExecutorEndpoint,
    renewal_threshold: Duration,
    inline_wait_threshold: Duration,
    /// Per-resource state, each protected by its own Mutex so that
    /// `try_reserve` and `renew_all` for different resources are fully
    /// concurrent.
    state: scc::HashMap<ResourceKey, Arc<LeaseEntry>>,
}

impl GrpcQuotaService {
    pub fn new(
        client: Arc<dyn ShardManager>,
        endpoint: ExecutorEndpoint,
        config: QuotaServiceConfig,
        shutdown_token: CancellationToken,
    ) -> Arc<Self> {
        let svc = Self::new_inner(
            client,
            endpoint,
            config.renewal_threshold,
            config.inline_wait_threshold,
        );
        svc.start_renewal_loop(shutdown_token, config.renewal_interval);
        svc
    }

    fn new_inner(
        client: Arc<dyn ShardManager>,
        endpoint: ExecutorEndpoint,
        renewal_threshold: Duration,
        inline_wait_threshold: Duration,
    ) -> Arc<Self> {
        Arc::new(Self {
            client,
            endpoint,
            renewal_threshold,
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
        tokio::spawn(async move {
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
        });
    }

    async fn get_slot(&self, key: &ResourceKey) -> Option<Arc<LeaseEntry>> {
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

    fn process_waiters(&self, slot: &mut LeaseInner, key: &ResourceKey) {
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
                let estimated_wait_time =
                    Self::estimated_wait(waiter.amount, total_available, &resource_limit);
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
    async fn notify_demand(&self, key: &ResourceKey, slot: &mut LeaseInner) {
        let (environment_id, resource_name) = key;
        match &slot.lease {
            TrackedLease::Unlimited(_) => {
                // Unlimited leases always succeed — no demand notification needed.
            }
            TrackedLease::Lost => {
                // Re-acquire immediately so waiters can be served sooner.
                match self
                    .client
                    .acquire_quota_lease(*environment_id, resource_name.clone(), self.endpoint)
                    .await
                {
                    Ok(new_lease) => {
                        slot.lease = Self::from_quota_lease(&new_lease);
                        self.process_waiters(slot, key);
                    }
                    Err(err) => {
                        tracing::warn!(error = %err, "Re-acquire in notify_demand failed");
                        return;
                    }
                }
                // The acquire carries no pending reservations, so it can grant
                // only the baseline share of the pool. If waiters remain queued,
                // follow up with a demand-carrying renewal so they are served now
                // instead of at the lease's near-expiry renewal.
                if !slot.waiters.is_empty() {
                    self.demand_renewal(key, slot).await;
                }
            }
            TrackedLease::Bounded(_) => {
                self.demand_renewal(key, slot).await;
            }
        }
    }

    /// Renew the bounded lease carrying the queued demand so the shard manager
    /// can refund the unused allocation and re-grant one sized to the waiters.
    /// Called with the slot lock held; a no-op for non-bounded leases.
    async fn demand_renewal(&self, key: &ResourceKey, slot: &mut LeaseInner) {
        let TrackedLease::Bounded(b) = &slot.lease else {
            return;
        };
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
        match self
            .client
            .batch_renew_quota_leases(self.endpoint, vec![entry])
            .await
        {
            Ok(mut results) => {
                if let Some(result) = results.pop() {
                    match result {
                        Ok(new_lease) => {
                            if let TrackedLease::Bounded(b) = Self::from_quota_lease(&new_lease) {
                                debug!(
                                    resource_definition_id = %b.resource_definition_id,
                                    allocation = b.remaining,
                                    total_available = b.total_available_amount,
                                    "demand renewal: received new allocation"
                                );
                                slot.lease = TrackedLease::Bounded(b);
                            } else {
                                slot.lease = Self::from_quota_lease(&new_lease);
                            }
                            self.process_waiters(slot, key);
                        }
                        Err(QuotaError::LeaseNotFound(_) | QuotaError::StaleEpoch(_)) => {
                            slot.lease = TrackedLease::Lost;
                        }
                        Err(err) => {
                            tracing::warn!(error = %err, "Demand renewal failed");
                        }
                    }
                }
            }
            Err(err) => {
                tracing::warn!(error = %err, "Demand renewal batch renew failed");
            }
        }
    }

    /// Reuse an existing entry. Called while holding the scc bucket lock.
    /// The `StdMutex` on `interest` is uncontended in practice (the bucket
    /// lock already serialises access) so this never actually blocks.
    fn reuse_slot<F>(entry: &Arc<LeaseEntry>, make_interest: &F) -> LeaseInterest
    where
        F: Fn(Arc<()>) -> LeaseInterest,
    {
        let mut interest = entry.interest.lock().unwrap();
        if let Some(arc) = interest.upgrade() {
            make_interest(arc)
        } else {
            let arc = Arc::new(());
            *interest = Arc::downgrade(&arc);
            make_interest(arc)
        }
    }

    /// Performs one renewal pass, spanned per pass rather than per loop: a span
    /// wrapping the loop would never close, so it would never be exported.
    async fn renew_all(&self) {
        async {
            // phase 1: collect live and dead slots
            let mut entries_to_renew: Vec<(ResourceKey, Arc<LeaseEntry>)> = Vec::new();
            let mut leases_to_release: Vec<TrackedLease> = Vec::new();

            self.state
                .retain_async(|key, entry| {
                    let is_dead = entry.interest.lock().unwrap().strong_count() == 0;
                    if is_dead {
                        // Try to take the lease for release; skip if inner is locked.
                        if let Ok(mut inner) = entry.inner.try_lock() {
                            leases_to_release.push(inner.take_lease());
                            false
                        } else {
                            // Inner is locked — keep for next pass.
                            true
                        }
                    } else {
                        entries_to_renew.push((key.clone(), entry.clone()));
                        true
                    }
                })
                .await;

            // phase 2: release dead leases
            if !leases_to_release.is_empty() {
                debug!("releasing {} unneeded leases", leases_to_release.len());
            }

            for lease in leases_to_release {
                self.try_release_lease_if_needed(lease).await;
            }

            // phase 3: renew live leases before they expire.
            // Bounded leases are batched; unlimited leases are re-acquired individually.
            let mut bounded_to_renew: Vec<(ResourceKey, Arc<LeaseEntry>)> = Vec::new();
            let mut unlimited_to_renew: Vec<(ResourceKey, Arc<LeaseEntry>)> = Vec::new();

            for (key, slot_mutex) in &entries_to_renew {
                let slot = slot_mutex.inner.lock().await;
                match &slot.lease {
                    TrackedLease::Bounded(b) => {
                        if (b.expires_at - Utc::now()).to_std().unwrap_or_default()
                            >= self.renewal_threshold
                        {
                            continue; // Plenty of time left — skip until closer to expiry.
                        }
                        bounded_to_renew.push((key.clone(), slot_mutex.clone()));
                    }
                    TrackedLease::Unlimited(u) => {
                        if (u.expires_at - Utc::now()).to_std().unwrap_or_default()
                            >= self.renewal_threshold
                        {
                            continue;
                        }
                        unlimited_to_renew.push((key.clone(), slot_mutex.clone()));
                    }
                    TrackedLease::Lost => continue,
                }
            }

            // Batch-renew all bounded leases in one RPC.
            // Lock all slots first, hold the locks across the RPC, then apply
            // results. This prevents concurrent try_reserve/commit/notify_demand
            // from modifying state between when we snapshot it and when we write
            // the new allocation back.
            if !bounded_to_renew.is_empty() {
                let mut locked: Vec<(ResourceKey, tokio::sync::MutexGuard<LeaseInner>)> =
                    Vec::with_capacity(bounded_to_renew.len());
                for (key, entry) in &bounded_to_renew {
                    locked.push((key.clone(), entry.inner.lock().await));
                }

                let mut batch: Vec<BatchRenewalEntry> = Vec::with_capacity(locked.len());
                for (_, slot) in &locked {
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

                let rpc_result = self
                    .client
                    .batch_renew_quota_leases(self.endpoint, batch)
                    .await;

                match rpc_result {
                    Ok(results) => {
                        for ((key, mut slot), result) in locked.into_iter().zip(results) {
                            let (expires_at, resource_definition_id) =
                                if let TrackedLease::Bounded(b) = &slot.lease {
                                    (b.expires_at, b.resource_definition_id)
                                } else {
                                    continue;
                                };
                            match result {
                                Ok(new_lease) => {
                                    if let QuotaLease::Bounded {
                                        allocation,
                                        total_available_amount,
                                        ..
                                    } = &new_lease
                                    {
                                        debug!(
                                            resource_definition_id = %resource_definition_id,
                                            allocation,
                                            total_available = total_available_amount,
                                            waiters = slot.waiters.len(),
                                            "renew_all: received new allocation"
                                        );
                                    }
                                    slot.lease = Self::from_quota_lease(&new_lease);
                                    self.process_waiters(&mut slot, &key);
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
                let mut slot = slot_mutex.inner.lock().await;
                if let TrackedLease::Unlimited(u) = &slot.lease {
                    let expires_at = u.expires_at;
                    match self
                        .client
                        .acquire_quota_lease(*environment_id, resource_name.clone(), self.endpoint)
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
                // Keep acquisition serialized with notify_demand: acquiring twice
                // replaces the remote lease and can forfeit its unused allocation.
                let mut slot = slot_mutex.inner.lock().await;
                if !matches!(slot.lease, TrackedLease::Lost) {
                    continue;
                }

                debug!("trying to reacquire lost lease {environment_id}/{resource_name}");

                match self
                    .client
                    .acquire_quota_lease(*environment_id, resource_name.clone(), self.endpoint)
                    .await
                {
                    Ok(new_lease) => {
                        tracing::info!(
                            environment_id = %environment_id,
                            resource_name = %resource_name,
                            "Re-acquired lost lease"
                        );
                        slot.lease = Self::from_quota_lease(&new_lease);
                        self.process_waiters(&mut slot, key);
                        // The re-acquire carries no pending reservations, so it
                        // can grant only the baseline share of the pool. If waiters
                        // remain queued, follow up with a demand-carrying renewal
                        // so they are served now instead of at the lease's
                        // near-expiry renewal.
                        if !slot.waiters.is_empty() {
                            self.demand_renewal(key, &mut slot).await;
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
        .instrument(info_span!("quota_renewal"))
        .await
    }

    async fn try_release_lease_if_needed(&self, tracked_lease: TrackedLease) {
        // unlimited leases do not reserve any capacity, so we can just let them expire
        if let TrackedLease::Bounded(lease) = tracked_lease
            && let Err(err) = self
                .client
                .release_quota_lease(
                    lease.resource_definition_id,
                    self.endpoint,
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
    ) -> LeaseInterest {
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

        match self.state.entry_async(key.clone()).await {
            scc::hash_map::Entry::Occupied(occ) => {
                let slot = occ.get();
                return Self::reuse_slot(slot, &make_interest);
            }
            scc::hash_map::Entry::Vacant(vac) => {
                // We are first — insert a placeholder with a lost lease.
                // it will either be reacquired on the next renew_all loop or inline in the next reserve
                let arc = Arc::new(());
                let entry = Arc::new(LeaseEntry {
                    interest: StdMutex::new(Arc::downgrade(&arc)),
                    inner: Mutex::new(LeaseInner {
                        waiters: Vec::new(),
                        lease: TrackedLease::Lost,
                    }),
                });
                vac.insert_entry(entry);
                make_interest(arc)
            }
        }
    }

    async fn try_reserve(&self, interest: &mut LeaseInterest, amount: u64) -> ReserveResult {
        let key: ResourceKey = (interest.environment_id, interest.resource_name.clone());

        let slot_mutex = self
            .get_slot(&key)
            .await
            .expect("try_reserve called without a prior acquire for this resource");

        let mut slot = slot_mutex.inner.lock().await;

        debug!(
            "reserving {amount} for {}/{} ({:?})",
            interest.environment_id, interest.resource_name, slot.lease
        );

        // Enqueue and let process_waiters try to serve immediately.
        let (tx, rx) = oneshot::channel();
        slot.waiters
            .push(Waiter::from_interest(amount, interest, tx));
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
            let mut slot = slot_mutex.inner.lock().await;
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
    ) -> LeaseInterest {
        let max_credit = max_credit_for(expected_use);
        LeaseInterest {
            environment_id,
            resource_name,
            expected_use,
            last_credit_value: previous_credit.min(max_credit),
            last_credit_value_at: previous_credit_at.unwrap_or_else(Utc::now),
            credit_rate: expected_use as f64 * CREDIT_RATE_FACTOR,
            max_credit,
            _token: Arc::new(()),
        }
    }

    async fn try_reserve(&self, _interest: &mut LeaseInterest, _amount: u64) -> ReserveResult {
        ReserveResult::Ok(Reservation::Unlimited)
    }

    async fn commit(&self, _interest: &mut LeaseInterest, _reservation: Reservation, _used: u64) {}
}

#[cfg(test)]
mod tests;
