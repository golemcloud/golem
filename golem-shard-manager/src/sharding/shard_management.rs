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

use super::error::ShardManagerError;
use super::healthcheck::{HealthCheck, get_unhealthy_executors};
use super::model::{
    ExecutorAddr, ExecutorAddrs, ExecutorId, RegisterAck, ShardAssignmentPush, ShardEpoch,
    ShardLeaseGrant, ShardLeaseState,
};
use super::persistence::{ExternalRevision, NO_REVISION, RoutingTablePersistence};
use super::rebalancing::Rebalance;
use super::worker_executor::{WorkerExecutorService, assign_shards, revoke_shards};
use async_rwlock::RwLock;
use chrono::Utc;
use golem_common::model::ShardId;
use itertools::Itertools;
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Mutex, Notify};
use tokio::task::JoinSet;
use tokio::time::timeout;
use tracing::{Instrument, debug, error, info, warn};

/// Bounds a persistence round-trip, so a wedged backend cannot hold the shard state lock forever,
/// leaving every [`ShardManagement::current_snapshot`] reader waiting while the fail-stop that
/// should end the process never runs.
pub(crate) const PERSISTENCE_TIMEOUT: Duration = Duration::from_secs(30);

const INITIAL_HEALTH_CHECK_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Clone)]
pub struct ShardManagement {
    shard_state: Arc<RwLock<ShardLeaseState>>,
    change: Arc<Notify>,
    updates: Arc<Mutex<ShardManagementChanges>>,
    persistence: Arc<dyn RoutingTablePersistence>,
    /// Compare-and-swap token of the persisted state; see [`Self::mutate_and_persist`].
    external_revision: Arc<Mutex<ExternalRevision>>,
    /// How long a granted shard lease lasts. Read by every writer that grants one, so they all
    /// use the same value.
    lease_ttl: Duration,
    /// The persistence failure of an out-of-loop writer, waiting to be picked up by the loop.
    ///
    /// A failed write means the state may or may not have been stored and, for a lost fence, that
    /// another shard manager owns the topology. That has to end the process, but an out-of-loop
    /// writer only ends its own request. It records the error here and wakes the loop, which
    /// checks the slot at the top of every pass and returns it.
    fatal: Arc<Mutex<Option<ShardManagerError>>>,
}

impl ShardManagement {
    /// Initializes the shard management with the persisted shard lease state.
    ///
    /// Executors found in the persisted state are health checked once: unhealthy ones are
    /// removed, healthy ones receive their authoritative full shard assignment (they might be
    /// lagging after interleaved shard-manager and executor restarts).
    pub async fn new(
        persistence_service: Arc<dyn RoutingTablePersistence>,
        worker_executors: Arc<dyn WorkerExecutorService>,
        health_check: Arc<dyn HealthCheck>,
        threshold: f64,
        lease_ttl: Duration,
        number_of_shards: usize,
        join_set: &mut JoinSet<anyhow::Result<()>>,
    ) -> Result<Self, ShardManagerError> {
        Self::new_with_initial_health_check_timeout(
            persistence_service,
            worker_executors,
            health_check,
            threshold,
            lease_ttl,
            number_of_shards,
            join_set,
            INITIAL_HEALTH_CHECK_TIMEOUT,
        )
        .await
    }

    /// [`Self::new`] with the startup health check bound taken as a parameter, so that a test does
    /// not have to wait out the production budget.
    #[allow(clippy::too_many_arguments)]
    pub async fn new_with_initial_health_check_timeout(
        persistence_service: Arc<dyn RoutingTablePersistence>,
        worker_executors: Arc<dyn WorkerExecutorService>,
        health_check: Arc<dyn HealthCheck>,
        threshold: f64,
        lease_ttl: Duration,
        number_of_shards: usize,
        join_set: &mut JoinSet<anyhow::Result<()>>,
        initial_health_check_timeout: Duration,
    ) -> Result<Self, ShardManagerError> {
        let (shard_state, external_revision) =
            match timeout(PERSISTENCE_TIMEOUT, persistence_service.read()).await {
                Ok(read) => read?,
                Err(_) => {
                    return Err(ShardManagerError::Internal(format!(
                        "reading the shard lease state timed out after {PERSISTENCE_TIMEOUT:?}"
                    )));
                }
            };

        // Before the health check and before the worker is spawned: past this point the worker can
        // persist state and command executors, and a replica that disagrees with the stored shard
        // count must do neither.
        crate::ensure_shard_count_matches(shard_state.number_of_shards, number_of_shards)?;

        info!("Initial healthcheck started");

        let executors: Vec<(ExecutorId, ExecutorAddr, Option<String>)> =
            shard_state.get_executors_with_addrs();
        let unhealthy_executors = match timeout(
            initial_health_check_timeout,
            get_unhealthy_executors(&health_check, &executors),
        )
        .await
        {
            Ok(unhealthy_executors) => unhealthy_executors,
            Err(_) => {
                // Dropping every executor because their probes were slow would empty the routing
                // table; the periodic health check loop removes the ones that are really gone.
                error!(
                    "Initial healthcheck timed out after {initial_health_check_timeout:?}, treating all executors as healthy"
                );
                HashSet::new()
            }
        };
        let healthy_executors: HashSet<ExecutorId> = executors
            .iter()
            .map(|(id, _, _)| *id)
            .filter(|id| !unhealthy_executors.contains(id))
            .collect();

        info!("Initial healthcheck finished");

        let shard_management = ShardManagement {
            shard_state: Arc::new(RwLock::new(shard_state)),
            change: Arc::new(Notify::new()),
            updates: Arc::new(Mutex::new(ShardManagementChanges::new(
                healthy_executors.clone(),
                unhealthy_executors,
            ))),
            persistence: persistence_service,
            external_revision: Arc::new(Mutex::new(external_revision)),
            lease_ttl,
            fatal: Arc::new(Mutex::new(None)),
        };

        // Lease expiries are persisted and absolute, so after any outage longer than one lease
        // every stored expiry is in the past and the first pass's housekeeping would evict the
        // whole cluster. The health check above has just proved these executors are alive, so their
        // lease clock restarts here - in one write, before the loop can run.
        if !healthy_executors.is_empty() {
            let now = Utc::now();
            let regranted = shard_management
                .mutate_and_persist(move |shard_state| {
                    shard_state.regrant_leases(&healthy_executors, now, lease_ttl)
                })
                .await?;
            info!(
                executors = regranted,
                "Re-granted shard leases to the executors that answered the initial health check"
            );
        }

        {
            let shard_management = shard_management.clone();
            join_set.spawn(
                async move {
                    shard_management
                        .worker(worker_executors, threshold)
                        .await
                        .map_err(anyhow::Error::from)
                }
                .in_current_span(),
            );
        }

        shard_management.change.notify_one();

        Ok(shard_management)
    }

    /// Registers the executor instance `executor_id`, listening at `addr`, and grants it a lease.
    ///
    /// The lease is written before this returns, so an acknowledged registration is a durable one:
    /// an executor is never told it is registered by a leader whose write is then refused.
    ///
    /// Idempotent on retry. `executor_id` is generated by the executor and stable across retries of
    /// the same registration, so the same id at the same address refreshes that lease and returns
    /// it; it neither creates a second lease nor counts as a replacement. A *different* id at a
    /// known address is a restarted instance, and inherits its predecessor's shards.
    pub async fn register_executor(
        &self,
        executor_id: ExecutorId,
        addr: ExecutorAddr,
        pod_name: Option<String>,
    ) -> Result<RegisterAck, ShardManagerError> {
        debug!(executor_id = %executor_id, addr = %addr, "Registering executor");
        let now = Utc::now();
        let lease_ttl = self.lease_ttl;

        let (already_known, replaced, ack) = self
            .try_mutate_and_persist(move |shard_state| {
                let already_known = shard_state.has_executor(executor_id);
                let replaced =
                    shard_state.add_executor(executor_id, addr, pod_name, now, lease_ttl);
                let grant = shard_state.lease_grant_for(executor_id).ok_or_else(|| {
                    ShardManagerError::Internal(format!(
                        "executor {executor_id} holds no lease right after being registered"
                    ))
                })?;
                Ok((
                    already_known,
                    replaced,
                    RegisterAck {
                        number_of_shards: shard_state.number_of_shards,
                        grant,
                    },
                ))
            })
            .await?;

        if let Some(replaced) = replaced {
            // A restarted instance inherited its predecessor's shards, so it has to be told the
            // whole set it now holds.
            info!(
                executor_id = %executor_id,
                replaced_executor_id = %replaced,
                addr = %addr,
                "Executor replaced at address"
            );
            self.updates.lock().await.retry_full_assignment(executor_id);
        } else if already_known {
            // A retried registration. The lease clock restarts and the same set comes back; the
            // shards are not touched, so their epochs do not move.
            info!(executor_id = %executor_id, addr = %addr, "Executor lease refreshed");
        } else {
            info!(executor_id = %executor_id, addr = %addr, "Executor added");
        }

        self.change.notify_one();
        Ok(ack)
    }

    /// Extends `executor_id`'s shard lease, asserting the set of shards it believes it holds.
    ///
    /// Every claimed shard must exist, be assigned to this executor and be at exactly the claimed
    /// epoch; if any one of them is not, the whole renewal fails with
    /// [`ShardManagerError::StaleShardEpoch`], **nothing** is renewed and the expiry is left where
    /// it was. That atomicity is by construction rather than by unwinding: the mutation runs on a
    /// clone that is dropped the moment the closure refuses.
    ///
    /// Only the *claimed* shards are checked. A shard the manager has assigned to this executor
    /// that the executor does not claim is not an error - it is an executor that has not received
    /// its last push yet - and the full set returned here is what corrects it.
    ///
    /// A renewal never advances an epoch: the epoch is an ownership generation, and moving it on a
    /// renewal would make a lost response permanently fatal for a shard the executor still owns.
    ///
    /// Leases that have already lapsed are reaped *before* this one is looked up, so an executor
    /// whose lease expired while its renewal was in flight is told
    /// [`ShardManagerError::ShardLeaseNotFound`] rather than silently resurrected. This does not
    /// notify the loop; the shards that reaping freed are picked up by the next tick.
    pub async fn renew_shard_lease(
        &self,
        executor_id: ExecutorId,
        claimed: &BTreeMap<ShardId, ShardEpoch>,
    ) -> Result<ShardLeaseGrant, ShardManagerError> {
        debug!(
            executor_id = %executor_id,
            claimed_shards = claimed.len(),
            "Renewing shard lease"
        );
        let now = Utc::now();
        let lease_ttl = self.lease_ttl;

        self.try_mutate_and_persist(move |shard_state| {
            // First, so that a lease which lapsed before this renewal arrived is gone by the time
            // it is looked up. `remove_executor` puts the freed shards on `pending_rebalance`.
            for (expired_id, released) in shard_state.housekeep(now) {
                warn!(
                    executor_id = %expired_id,
                    released_shards = released.len(),
                    "Shard lease expired; releasing its shards"
                );
            }

            if !shard_state.has_executor(executor_id) {
                return Err(ShardManagerError::ShardLeaseNotFound { executor_id });
            }

            // The whole claim is validated before anything is renewed. Revoked, moved and
            // wrong-epoch all land here: in each case there is no epoch this claimant could have
            // sent that would still be current for that shard.
            for (shard_id, provided) in claimed {
                let expected = shard_state
                    .shard_assignments
                    .get(shard_id)
                    .filter(|entry| entry.executor_id == executor_id)
                    .map(|entry| entry.epoch);
                if expected != Some(*provided) {
                    return Err(ShardManagerError::StaleShardEpoch {
                        executor_id,
                        shard_id: *shard_id,
                        expected,
                        provided: *provided,
                    });
                }
            }

            if !shard_state.renew_lease(executor_id, now, lease_ttl) {
                return Err(ShardManagerError::Internal(format!(
                    "executor {executor_id} holds no lease right after it was found"
                )));
            }

            // Read off the mutated clone, so the grant this returns is exactly the state that is
            // about to be stored - never a state that a failed write then rolls back.
            shard_state.lease_grant_for(executor_id).ok_or_else(|| {
                ShardManagerError::Internal(format!(
                    "executor {executor_id} holds no lease right after it was renewed"
                ))
            })
        })
        .await
    }

    /// Releases `executor_id`'s shard lease on a graceful shutdown.
    ///
    /// Lenient by contract: an executor the manager does not know, and a `claimed` set that no
    /// longer matches what it records, are both `Ok`. A shutdown must never fail on bookkeeping,
    /// so the claim is only logged.
    ///
    /// Removing the lease drops its shard assignments and leaves them on `pending_rebalance`.
    /// This does not notify the loop: the next tick re-homes them, which bounds a graceful
    /// shutdown's hand-off by one tick without an extra wake-up.
    pub async fn deregister_executor(
        &self,
        executor_id: ExecutorId,
        claimed: &BTreeMap<ShardId, ShardEpoch>,
    ) -> Result<(), ShardManagerError> {
        debug!(
            executor_id = %executor_id,
            claimed_shards = claimed.len(),
            "Deregistering executor"
        );

        self.mutate_and_persist(move |shard_state| {
            if !shard_state.has_executor(executor_id) {
                debug!(
                    executor_id = %executor_id,
                    "Deregistered executor holds no lease; nothing to release"
                );
                return;
            }

            let stale: Vec<ShardId> = claimed
                .iter()
                .filter(|(shard_id, epoch)| {
                    shard_state
                        .shard_assignments
                        .get(shard_id)
                        .filter(|entry| entry.executor_id == executor_id)
                        .map(|entry| entry.epoch)
                        != Some(**epoch)
                })
                .map(|(shard_id, _)| *shard_id)
                .collect();
            if !stale.is_empty() {
                warn!(
                    executor_id = %executor_id,
                    shards = stale.iter().join(", "),
                    "Deregistering executor handed back shards it no longer owns; releasing its \
                     lease anyway"
                );
            }

            let released = shard_state.remove_executor(executor_id);
            info!(
                executor_id = %executor_id,
                released_shards = released.len(),
                "Executor deregistered"
            );
        })
        .await
    }

    /// Marks an executor to be removed
    pub async fn unregister_executor(&self, executor_id: ExecutorId) {
        debug!(executor_id = %executor_id, "Unregistering executor");
        self.updates.lock().await.remove_executor(executor_id);
        self.change.notify_one();
    }

    /// Gets the current snapshot of the shard lease state
    pub async fn current_snapshot(&self) -> ShardLeaseState {
        self.shard_state.read().await.clone()
    }

    async fn worker(
        self,
        worker_executors: Arc<dyn WorkerExecutorService>,
        threshold: f64,
    ) -> Result<(), ShardManagerError> {
        // The timer is what makes the pull-based half of the lease protocol work. `RenewShardLease`
        // and `Deregister` never wake the loop - they only leave shards behind - so without a tick
        // an expired lease in a quiet cluster would never be reaped and a graceful shutdown's
        // shards would never be re-homed. A third of the lease is the same cadence the executors
        // renew at, and it is derived rather than configured so there is no second knob to keep
        // consistent with the lease duration.
        let tick_period = std::cmp::max(self.lease_ttl / 3, Duration::from_millis(1));
        // `interval_at`, not `interval`: the latter's first tick completes immediately and would
        // add a redundant pass on top of the startup notification. `Delay` so that a pass slower
        // than the period cannot queue a burst of catch-up ticks behind it.
        let mut tick =
            tokio::time::interval_at(tokio::time::Instant::now() + tick_period, tick_period);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

        loop {
            debug!("Shard management loop awaiting changes");
            tokio::select! {
                _ = self.change.notified() => {}
                _ = tick.tick() => {}
            }

            // An out-of-loop writer's persist failed. The store may or may not hold what it tried
            // to write, and a refused fenced write means another shard manager owns the topology,
            // so this process has to stop rather than command executors from here on.
            if let Some(fatal) = self.fatal.lock().await.take() {
                error!(
                    error = %fatal,
                    "Persisting the shard lease state failed outside the shard management loop; \
                     stopping"
                );
                return Err(fatal);
            }

            let (removed_executors, full_assignment_requests) = self.updates.lock().await.reset();
            debug!(
                removed_executors = removed_executors.iter().join(", "),
                full_assignment_requests = full_assignment_requests.iter().join(", "),
                "Shard management loop woken up",
            );
            // The write lock is held while
            //   - removals are applied to the state and got persisted,
            //   - the rebalance plan is calculated,
            // but the rebalance plan is NOT applied yet. The lock is then released for apply.
            let now = Utc::now();
            let (base, mut rebalance, full_assignment_executors, addrs) = self
                .mutate_and_persist(|current_shard_state| {
                    // Every pass begins by reaping the leases that lapsed since the last one.
                    // Removing a lease drops its shard assignments, so the plan computed below
                    // sees those shards as unassigned and re-homes them.
                    for (executor_id, released) in current_shard_state.housekeep(now) {
                        warn!(
                            executor_id = %executor_id,
                            released_shards = released.len(),
                            "Shard lease expired; releasing its shards"
                        );
                    }

                    // Shards orphaned by lease removals since the last pass. The rebalance plan
                    // below recomputes all unassigned shards from scratch, so this is only logged.
                    let pending = current_shard_state.take_pending_rebalance();
                    if !pending.is_empty() {
                        debug!(
                            shards = pending.iter().join(", "),
                            "Redistributing shards orphaned since the last pass"
                        );
                    }

                    let full_assignment_executors = apply_executor_changes(
                        current_shard_state,
                        removed_executors,
                        full_assignment_requests,
                    );

                    let rebalance = Rebalance::from_shard_state(current_shard_state, threshold);
                    let addrs = current_shard_state.executor_addrs();

                    // The state the plan was computed against, and the state `apply_rebalance`
                    // below is applied to. `execute_rebalance` builds the pushes from it, so what
                    // an executor is told matches what is then stored.
                    (
                        current_shard_state.clone(),
                        rebalance,
                        full_assignment_executors,
                        addrs,
                    )
                })
                .await?;

            debug!(rebalance=%rebalance, "Applying rebalance plan");
            let rebalance_failures =
                Self::execute_rebalance(worker_executors.clone(), &base, &mut rebalance, &addrs)
                    .await?;

            let mut needs_retry = false;
            if !rebalance_failures.failed_assignments.is_empty() {
                let failed_shards: HashSet<ShardId> = rebalance_failures
                    .failed_assignments
                    .iter()
                    .filter_map(|executor_id| {
                        rebalance.get_assignments().assignments.get(executor_id)
                    })
                    .flatten()
                    .copied()
                    .collect();
                rebalance.remove_assignment_shards(&failed_shards);

                warn!(
                    failed_shards = failed_shards.iter().join(", "),
                    "Some shards could not be assigned and will be left unassigned for retry"
                );

                {
                    let mut updates_guard = self.updates.lock().await;
                    for executor_id in &rebalance_failures.failed_assignments {
                        if full_assignment_executors.contains(executor_id) {
                            updates_guard.retry_full_assignment(*executor_id);
                        }
                    }
                }
                needs_retry = true;
            }

            if !rebalance_failures.failed_unassignments.is_empty() {
                warn!(
                    failed_executors = rebalance_failures.failed_unassignments.iter().join(", "),
                    "Some shards could not be unassigned and rebalance will be retried"
                );
                needs_retry = true;
            }

            // A planned epoch that was overtaken between the push and the apply is re-minted, and
            // the executor that was pushed the stale one has to be told the epoch it really holds.
            let stale_pushes = self
                .mutate_and_persist(|current_shard_state| {
                    current_shard_state.apply_rebalance(&rebalance)
                })
                .await?;
            if !stale_pushes.is_empty() {
                warn!(
                    executors = stale_pushes.iter().join(", "),
                    "Some executors were pushed a shard epoch that was overtaken before it was \
                     applied and will be pushed their full set again"
                );
                {
                    let mut updates_guard = self.updates.lock().await;
                    for executor_id in &stale_pushes {
                        updates_guard.retry_full_assignment(*executor_id);
                    }
                }
                needs_retry = true;
            }

            // Read after the persist rather than out of the closure, so the snapshot's
            // `revision` field is consistent with what was stored.
            let shard_state_snapshot = self.shard_state.read().await.clone();

            let pushes = pushes_for(
                &shard_state_snapshot,
                full_assignment_executors.iter().copied(),
            )?;

            let failed_full_assignments = if pushes.is_empty() {
                BTreeSet::new()
            } else {
                assign_shards(worker_executors.clone(), &pushes, &addrs).await
            };

            if !failed_full_assignments.is_empty() {
                warn!(
                    failed_executors = failed_full_assignments.iter().join(", "),
                    "Some executors could not receive authoritative shard assignment and will be retried"
                );

                {
                    let mut updates_guard = self.updates.lock().await;
                    for executor_id in &failed_full_assignments {
                        updates_guard.retry_full_assignment(*executor_id);
                    }
                }
                needs_retry = true;
            }

            if needs_retry {
                self.change.notify_one();
            }
        }
    }

    /// Applies `mutate` to a *clone* of the shard lease state, persists it compare-and-swap style
    /// guarded on the cached external revision, and swaps it into the live state only once the
    /// write is durable.
    ///
    /// A clone rather than mutate-in-place with rollback: a caller dropped mid-persist never runs
    /// its rollback, but the guard's `Drop` publishes the mutation anyway - and the next write
    /// either stores that stray mutation as though intended, or, if the abandoned write landed,
    /// fails forever on a cached revision that is now behind.
    ///
    /// The write lock is held across the persistence round-trip so that readers of
    /// [`Self::current_snapshot`] can never observe a state that was not durably stored and then
    /// watch it go backwards. Lock order is `shard_state`, then `external_revision`. Every writer
    /// of the persisted state must go through here, which is what makes in-process conflicts
    /// impossible.
    ///
    /// A [`ShardManagerError::ConcurrentModification`] therefore means another shard manager
    /// *process* wrote the state. The cached revision is deliberately not refreshed on failure: it
    /// is the fencing token, and a writer that lost it must stop, not adopt the winner's and go on.
    async fn mutate_and_persist<T, F>(&self, mutate: F) -> Result<T, ShardManagerError>
    where
        F: FnOnce(&mut ShardLeaseState) -> T,
    {
        self.try_mutate_and_persist(|shard_state| Ok(mutate(shard_state)))
            .await
    }

    /// [`Self::mutate_and_persist`] for a mutation that can refuse.
    ///
    /// When `mutate` returns `Err`, the clone is dropped: the revision is not bumped, nothing is
    /// written, and the live state is untouched. That is what makes a validating writer atomic
    /// without the caller having to unwind anything - the alternative, validating and then
    /// mutating, would persist a half-applied change the moment a future edit mutates before it
    /// decides to fail.
    async fn try_mutate_and_persist<T, F>(&self, mutate: F) -> Result<T, ShardManagerError>
    where
        F: FnOnce(&mut ShardLeaseState) -> Result<T, ShardManagerError>,
    {
        let mut current_shard_state = self.shard_state.write().await;
        let mut external_revision = self.external_revision.lock().await;

        let mut next_shard_state = current_shard_state.clone();
        let prev_external_revision = *external_revision;
        let outcome = mutate(&mut next_shard_state)?;

        // Nothing changed, so there is nothing to store. Without this the periodic tick would
        // write a fresh full blob for an idle cluster every period forever, each one a new
        // revision the backend then has to compact. Draining `pending_rebalance` is a mutation
        // like any other, so a pass that took work off it is never skipped here.
        //
        // The exception is the very first write against a store that holds nothing. The persisted
        // state is also the only record of `number_of_shards`, and a replica configured for a
        // different count is refused by comparing against it, so a cluster whose first executor has
        // not registered yet would have nothing for that check to disagree with. One write on first
        // boot; from then on the revision is set and an idle pass writes nothing.
        if next_shard_state == *current_shard_state && *external_revision != NO_REVISION {
            return Ok(outcome);
        }

        let written = match next_shard_state.bump_revision() {
            Ok(_) => {
                let write = self
                    .persistence
                    .write(&next_shard_state, prev_external_revision);

                match timeout(PERSISTENCE_TIMEOUT, write).await {
                    Ok(written) => written,
                    Err(_) => Err(ShardManagerError::Internal(format!(
                        "persisting the shard lease state timed out after {PERSISTENCE_TIMEOUT:?}"
                    ))),
                }
            }
            Err(err) => Err(err),
        };

        match written {
            Ok(new_external_revision) => {
                *current_shard_state = next_shard_state;
                *external_revision = new_external_revision;
                Ok(outcome)
            }
            Err(err) => {
                match &err {
                    ShardManagerError::ConcurrentModification => error!(
                        prev_external_revision,
                        "Revision conflict: another shard manager wrote the shard lease state; \
                         the in-memory state is unchanged"
                    ),
                    other => error!(
                        error = %other,
                        "Persisting the shard lease state failed; the in-memory state is unchanged"
                    ),
                }
                // Fail-stop. The loop returns this at the top of its next pass, whether the writer
                // was the loop itself (which also gets it back here) or an out-of-loop one whose
                // own error only ends its request.
                self.record_fatal(err.duplicate()).await;
                Err(err)
            }
        }
    }

    /// Hands a persistence failure to the loop and wakes it. The first one wins: what stops the
    /// process is the first refused write, and a later error is a consequence of it.
    async fn record_fatal(&self, err: ShardManagerError) {
        let mut fatal = self.fatal.lock().await;
        if fatal.is_none() {
            *fatal = Some(err);
        }
        drop(fatal);
        self.change.notify_one();
    }

    /// Revokes first, then pushes each gaining executor its complete new shard set.
    ///
    /// `base` is the state the plan was computed against and the one `apply_rebalance` is applied
    /// to afterwards, so applying the (already stripped) plan to a copy of it yields exactly the
    /// sets and epochs that will be stored - nothing here predicts them.
    async fn execute_rebalance(
        worker_executors: Arc<dyn WorkerExecutorService + Send + Sync>,
        base: &ShardLeaseState,
        rebalance: &mut Rebalance,
        addrs: &ExecutorAddrs,
    ) -> Result<RebalanceFailures, ShardManagerError> {
        info!("Beginning rebalance...");

        if !rebalance.get_unassignments().is_empty() {
            info!(
                unassignments = %rebalance.get_unassignments(),
                "Executing shard unassignments",
            );
        }
        let failed_unassignments = revoke_shards(
            worker_executors.clone(),
            rebalance.get_unassignments(),
            addrs,
        )
        .await;
        let failed_shards: HashSet<ShardId> = failed_unassignments
            .iter()
            .filter_map(|executor_id| rebalance.get_unassignments().unassignments.get(executor_id))
            .flatten()
            .copied()
            .collect();
        // Before the assignments are built, so a shard whose revoke failed is never pushed to a
        // second owner.
        rebalance.remove_shards(&failed_shards);
        if !failed_shards.is_empty() {
            warn!(
                failed_shards = failed_shards.iter().join(", "),
                "Some shards could not be unassigned and have been removed from rebalance"
            );
        }

        if !rebalance.get_assignments().is_empty() {
            info!(
                assignments=%rebalance.get_assignments(),
                "Executing shard assignments",
            );
        }

        let mut planned = base.clone();
        // The probe: the plan applied to a copy of the state it will be applied to for real, which
        // is what turns "plan + base" into the exact set each gaining executor is to hold. Its
        // stale-push report is the live apply's to make, not this copy's.
        let _ = planned.apply_rebalance(rebalance);
        let pushes = pushes_for(
            &planned,
            rebalance.get_assignments().assignments.keys().copied(),
        )?;

        let failed_assignments = if pushes.is_empty() {
            BTreeSet::new()
        } else {
            assign_shards(worker_executors.clone(), &pushes, addrs).await
        };

        Ok(RebalanceFailures {
            failed_assignments,
            failed_unassignments,
        })
    }
}

/// The full-replace payloads for `executor_ids`, read off `shard_state`.
///
/// The one place a push is built, so the never-zero `number_of_shards` guard has a single home.
/// An executor's `set_shards` divides by that count, so a push that latched zero would make it
/// divide by zero on its next routing decision. The count is validated against the configuration
/// at startup, which makes a zero here an impossible state rather than a bad request - hence an
/// error that stops the loop rather than a skipped push.
fn pushes_for(
    shard_state: &ShardLeaseState,
    executor_ids: impl Iterator<Item = ExecutorId>,
) -> Result<BTreeMap<ExecutorId, ShardAssignmentPush>, ShardManagerError> {
    let pushes: BTreeMap<ExecutorId, ShardAssignmentPush> = executor_ids
        .filter_map(|executor_id| {
            shard_state
                .assignment_push_for(executor_id)
                .map(|push| (executor_id, push))
        })
        .collect();

    if !pushes.is_empty() && shard_state.number_of_shards == 0 {
        return Err(ShardManagerError::Internal(
            "refusing to push a shard assignment with number_of_shards = 0".to_string(),
        ));
    }

    Ok(pushes)
}

/// Applies the executor removals queued since the last pass and returns the executors that must
/// receive their full shard assignment.
///
/// Registrations are not queued: since `Register` acknowledges only after its own persist, an
/// executor is already in the state by the time the loop runs, and a replacement queues its full
/// assignment through [`ShardManagementChanges::retry_full_assignment`] like any other repair.
fn apply_executor_changes(
    shard_state: &mut ShardLeaseState,
    removed_executors: HashSet<ExecutorId>,
    full_assignment_requests: HashSet<ExecutorId>,
) -> HashSet<ExecutorId> {
    let mut full_assignment_executors: HashSet<ExecutorId> = HashSet::new();

    for executor_id in removed_executors {
        if !shard_state.has_executor(executor_id) {
            debug!(
                executor_id = %executor_id,
                "Executor to be removed is no longer registered"
            );
            continue;
        }
        let released = shard_state.remove_executor(executor_id);
        info!(
            executor_id = %executor_id,
            released_shards = released.len(),
            "Executor removed"
        );
    }

    for executor_id in full_assignment_requests {
        if shard_state.has_executor(executor_id) {
            full_assignment_executors.insert(executor_id);
        }
    }

    full_assignment_executors
}

/// The executors an operation of the pass did not reach. The shard sets they were about are read
/// back from the plan, which is where they came from.
#[derive(Debug)]
struct RebalanceFailures {
    failed_assignments: BTreeSet<ExecutorId>,
    failed_unassignments: BTreeSet<ExecutorId>,
}

/// Changes accumulated between two passes of the shard management loop.
#[derive(Debug)]
struct ShardManagementChanges {
    removed_executors: HashSet<ExecutorId>,
    full_assignment_requests: HashSet<ExecutorId>,
}

impl ShardManagementChanges {
    pub fn new(
        full_assignment_requests: HashSet<ExecutorId>,
        removed_executors: HashSet<ExecutorId>,
    ) -> Self {
        ShardManagementChanges {
            removed_executors,
            full_assignment_requests,
        }
    }

    pub fn remove_executor(&mut self, executor_id: ExecutorId) {
        self.full_assignment_requests.remove(&executor_id);
        self.removed_executors.insert(executor_id);
    }

    pub fn retry_full_assignment(&mut self, executor_id: ExecutorId) {
        if !self.removed_executors.contains(&executor_id) {
            self.full_assignment_requests.insert(executor_id);
        }
    }

    pub fn reset(&mut self) -> (HashSet<ExecutorId>, HashSet<ExecutorId>) {
        let removed = std::mem::take(&mut self.removed_executors);
        let full = std::mem::take(&mut self.full_assignment_requests);
        (removed, full)
    }
}

#[cfg(test)]
mod tests {
    use test_r::test;

    use super::*;
    use crate::sharding::model::ShardEpoch;
    use golem_common::model::ShardId;
    use std::net::{IpAddr, Ipv4Addr};
    use uuid::Uuid;

    const TTL: Duration = Duration::from_secs(60);

    fn t0() -> chrono::DateTime<Utc> {
        chrono::DateTime::from_timestamp(1_700_000_000, 0).unwrap()
    }

    fn executor(idx: u128) -> ExecutorId {
        ExecutorId(Uuid::from_u128(idx))
    }

    fn addr(idx: u8) -> ExecutorAddr {
        ExecutorAddr {
            ip: IpAddr::V4(Ipv4Addr::new(10, 0, 0, idx)),
            port: 9000 + idx as u16,
        }
    }

    fn shards(ids: &[i64]) -> BTreeSet<ShardId> {
        ids.iter().copied().map(ShardId::new).collect()
    }

    #[test]
    // Registration is applied inline by `register_executor`, before the loop runs, so by the time
    // the pass applies the queued removal of the instance that was replaced, that instance is
    // already gone and the removal must not take the transferred shards away again.
    fn a_removal_queued_for_an_instance_replaced_inline_releases_nothing() {
        let mut shard_state = ShardLeaseState::new(4);
        shard_state.add_executor(executor(1), addr(1), None, t0(), TTL);
        shard_state.add_executor(executor(2), addr(2), None, t0(), TTL);
        for shard_id in [0, 1] {
            shard_state.assign_shard(executor(1), ShardId::new(shard_id));
        }
        for shard_id in [2, 3] {
            shard_state.assign_shard(executor(2), ShardId::new(shard_id));
        }

        // the health check reported executor 1 unhealthy, and the restarted process at the same
        // address registered (as executor 3) - which persists immediately and asks for a full
        // assignment - before the loop woke up
        shard_state.add_executor(executor(3), addr(1), None, t0(), TTL);
        assert_eq!(
            shard_state.shards_for_executor(executor(3)),
            Some(shards(&[0, 1]))
        );

        let full = apply_executor_changes(
            &mut shard_state,
            HashSet::from([executor(1)]),
            HashSet::from([executor(3)]),
        );

        assert_eq!(full, HashSet::from([executor(3)]));
        assert!(!shard_state.has_executor(executor(1)));
        assert_eq!(
            shard_state.shards_for_executor(executor(3)),
            Some(shards(&[0, 1]))
        );
        assert_eq!(
            shard_state.shards_for_executor(executor(2)),
            Some(shards(&[2, 3]))
        );
        assert_eq!(
            shard_state.epoch_for_shard(ShardId::new(0)),
            Some(ShardEpoch(1))
        );
        assert!(shard_state.get_unassigned_shards().is_empty());
        assert!(shard_state.pending_rebalance.is_empty());
        assert!(shard_state.check_invariants().is_ok());
    }

    #[test]
    fn removal_of_an_executor_releases_its_shards_and_full_requests_are_filtered() {
        let mut shard_state = ShardLeaseState::new(4);
        shard_state.add_executor(executor(1), addr(1), None, t0(), TTL);
        shard_state.add_executor(executor(2), addr(2), None, t0(), TTL);
        shard_state.assign_shard(executor(1), ShardId::new(0));
        shard_state.assign_shard(executor(2), ShardId::new(1));

        let full = apply_executor_changes(
            &mut shard_state,
            HashSet::from([executor(1)]),
            HashSet::from([executor(1), executor(2), executor(9)]),
        );

        assert_eq!(full, HashSet::from([executor(2)]));
        assert!(!shard_state.has_executor(executor(1)));
        assert_eq!(shard_state.pending_rebalance, shards(&[0]));
        assert_eq!(shard_state.get_unassigned_shards(), shards(&[0, 2, 3]));
    }

    #[test]
    // The other interleaving: the removal is applied while the previous instance is still
    // registered, so its shards are released and the restart that registers afterwards starts
    // empty. Nothing is lost - the released shards are unassigned and the next plan re-homes them.
    fn a_removal_applied_before_the_restart_registers_releases_the_shards() {
        let mut shard_state = ShardLeaseState::new(4);
        shard_state.add_executor(executor(1), addr(1), None, t0(), TTL);
        shard_state.assign_shard(executor(1), ShardId::new(0));
        shard_state.assign_shard(executor(1), ShardId::new(1));

        let full = apply_executor_changes(
            &mut shard_state,
            HashSet::from([executor(1)]),
            HashSet::new(),
        );
        assert!(full.is_empty());
        assert_eq!(shard_state.pending_rebalance, shards(&[0, 1]));

        // the restart registers at the same address afterwards: no predecessor to inherit from
        assert_eq!(
            shard_state.add_executor(executor(3), addr(1), None, t0(), TTL),
            None
        );
        assert_eq!(
            shard_state.shards_for_executor(executor(3)),
            Some(BTreeSet::new())
        );
        assert_eq!(shard_state.get_unassigned_shards(), shards(&[0, 1, 2, 3]));
        assert!(shard_state.check_invariants().is_ok());
    }

    #[test]
    fn unregistering_drops_a_queued_full_assignment_of_the_same_executor() {
        let mut changes = ShardManagementChanges::new(HashSet::new(), HashSet::new());
        changes.retry_full_assignment(executor(3));
        changes.remove_executor(executor(3));
        changes.retry_full_assignment(executor(3)); // ignored: queued for removal
        changes.retry_full_assignment(executor(4));

        let (removed, full) = changes.reset();
        assert_eq!(removed, HashSet::from([executor(3)]));
        assert_eq!(full, HashSet::from([executor(4)]));

        let (removed, full) = changes.reset();
        assert!(removed.is_empty() && full.is_empty());
    }
}
