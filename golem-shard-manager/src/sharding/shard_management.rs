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
use super::model::{Assignments, ExecutorAddr, ExecutorAddrs, ExecutorId, ShardLeaseState};
use super::persistence::RoutingTablePersistence;
use super::rebalancing::Rebalance;
use super::worker_executor::{
    WorkerExecutorService, assign_shards, revoke_shards, set_shard_assignments,
};
use async_rwlock::RwLock;
use chrono::Utc;
use golem_common::model::ShardId;
use itertools::Itertools;
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Mutex, Notify};
use tokio::task::JoinSet;
use tracing::{Instrument, debug, info, warn};

#[derive(Clone)]
pub struct ShardManagement {
    shard_state: Arc<RwLock<ShardLeaseState>>,
    change: Arc<Notify>,
    updates: Arc<Mutex<ShardManagementChanges>>,
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
        join_set: &mut JoinSet<anyhow::Result<()>>,
    ) -> Result<Self, ShardManagerError> {
        let shard_state = persistence_service.read().await?;

        info!("Initial healthcheck started");

        let executors = shard_state.get_executors_with_addrs();
        let unhealthy_executors = get_unhealthy_executors(&health_check, &executors).await;
        let healthy_executors: HashSet<ExecutorId> = executors
            .iter()
            .map(|(id, _, _)| *id)
            .filter(|id| !unhealthy_executors.contains(id))
            .collect();

        info!("Initial healthcheck finished");

        let change = Arc::new(Notify::new());
        let updates = Arc::new(Mutex::new(ShardManagementChanges::new(
            healthy_executors,
            unhealthy_executors,
        )));
        let shard_state = Arc::new(RwLock::new(shard_state));

        {
            let change = change.clone();
            let updates = updates.clone();
            let shard_state = shard_state.clone();

            join_set.spawn(
                async move {
                    Self::worker(
                        shard_state,
                        change,
                        updates,
                        persistence_service,
                        worker_executors,
                        threshold,
                        lease_ttl,
                    )
                    .await;
                    Ok(())
                }
                .in_current_span(),
            );
        };

        change.notify_one();

        Ok(ShardManagement {
            shard_state,
            change,
            updates,
        })
    }

    /// Registers a new executor instance listening at `addr`.
    ///
    /// Every registration is a new executor instance and receives a fresh [`ExecutorId`],
    /// which is returned. If another instance is still registered at the same address it is
    /// replaced on the next rebalance pass.
    pub async fn register_executor(
        &self,
        addr: ExecutorAddr,
        pod_name: Option<String>,
    ) -> ExecutorId {
        let executor_id = ExecutorId::generate();
        debug!(executor_id = %executor_id, addr = %addr, "Registering executor");
        self.updates
            .lock()
            .await
            .add_executor_registration(executor_id, addr, pod_name);
        self.change.notify_one();
        executor_id
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
        shard_state: Arc<RwLock<ShardLeaseState>>,
        change: Arc<Notify>,
        updates: Arc<Mutex<ShardManagementChanges>>,
        persistence_service: Arc<dyn RoutingTablePersistence>,
        worker_executors: Arc<dyn WorkerExecutorService>,
        threshold: f64,
        lease_ttl: Duration,
    ) {
        loop {
            debug!("Shard management loop awaiting changes");
            change.notified().await;

            let (new_executors, removed_executors, full_assignment_requests) =
                updates.lock().await.reset();
            debug!(
                new_executors = new_executors
                    .values()
                    .map(|r| format!("{} ({})", r.executor_id, r.addr))
                    .join(", "),
                removed_executors = removed_executors.iter().join(", "),
                full_assignment_requests = full_assignment_requests.iter().join(", "),
                "Shard management loop woken up",
            );
            // Getting a write lock while
            //   - registrations and removals are applied to the state and got persisted,
            //   - the rebalance plan is calculated,
            // but the rebalance plan is NOT applied yet. The lock is then released for apply.
            let (mut rebalance, full_assignment_executors, addrs) = {
                let mut current_shard_state = shard_state.write().await;

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
                    &mut current_shard_state,
                    new_executors,
                    removed_executors,
                    full_assignment_requests,
                    lease_ttl,
                );

                let rebalance = Rebalance::from_shard_state(&current_shard_state, threshold);
                let addrs = current_shard_state.executor_addrs();

                current_shard_state
                    .bump_revision()
                    .expect("Failed to bump shard lease state revision");
                persistence_service
                    .write(&current_shard_state)
                    .await
                    .expect("Failed to persist shard lease state after executor changes");

                (rebalance, full_assignment_executors, addrs)
            };

            debug!(rebalance=%rebalance, "Applying rebalance plan");
            let rebalance_failures =
                Self::execute_rebalance(worker_executors.clone(), &mut rebalance, &addrs).await;

            let mut needs_retry = false;
            if !rebalance_failures.failed_assignments.is_empty() {
                let failed_shards: HashSet<ShardId> = rebalance_failures
                    .failed_assignments
                    .iter()
                    .flat_map(|(_, shard_ids)| shard_ids.clone())
                    .collect();
                rebalance.remove_assignment_shards(&failed_shards);

                warn!(
                    failed_shards = failed_shards.iter().join(", "),
                    "Some shards could not be assigned and will be left unassigned for retry"
                );

                {
                    let mut updates_guard = updates.lock().await;
                    for (executor_id, _) in &rebalance_failures.failed_assignments {
                        if full_assignment_executors.contains(executor_id) {
                            updates_guard.retry_full_assignment(*executor_id);
                        }
                    }
                }
                needs_retry = true;
            }

            if !rebalance_failures.failed_unassignments.is_empty() {
                warn!(
                    failed_executors = rebalance_failures
                        .failed_unassignments
                        .iter()
                        .map(|(executor_id, _)| executor_id)
                        .join(", "),
                    "Some shards could not be unassigned and rebalance will be retried"
                );
                needs_retry = true;
            }

            let shard_state_snapshot = {
                let mut current_shard_state = shard_state.write().await;
                current_shard_state.apply_rebalance(&rebalance);
                current_shard_state
                    .bump_revision()
                    .expect("Failed to bump shard lease state revision");
                current_shard_state.clone()
            };
            persistence_service
                .write(&shard_state_snapshot)
                .await
                .expect("Failed to persist shard lease state after rebalance");

            let mut full_assignments = Assignments::new();
            for executor_id in &full_assignment_executors {
                if let Some(mut shard_ids) = shard_state_snapshot.shards_for_executor(*executor_id)
                {
                    full_assignments
                        .assignments
                        .entry(*executor_id)
                        .or_default()
                        .append(&mut shard_ids);
                }
            }

            let failed_full_assignments = if full_assignments.is_empty() {
                Vec::new()
            } else {
                set_shard_assignments(
                    worker_executors.clone(),
                    shard_state_snapshot.number_of_shards,
                    &full_assignments,
                    &addrs,
                )
                .await
            };

            if !failed_full_assignments.is_empty() {
                warn!(
                    failed_executors = failed_full_assignments
                        .iter()
                        .map(|(executor_id, _)| executor_id)
                        .join(", "),
                    "Some executors could not receive authoritative shard assignment and will be retried"
                );

                {
                    let mut updates_guard = updates.lock().await;
                    for (executor_id, _) in &failed_full_assignments {
                        updates_guard.retry_full_assignment(*executor_id);
                    }
                }
                needs_retry = true;
            }

            if needs_retry {
                change.notify_one();
            }
        }
    }

    async fn execute_rebalance(
        worker_executors: Arc<dyn WorkerExecutorService + Send + Sync>,
        rebalance: &mut Rebalance,
        addrs: &ExecutorAddrs,
    ) -> RebalanceFailures {
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
        let failed_shards = failed_unassignments
            .iter()
            .flat_map(|(_, shard_ids)| shard_ids.clone())
            .collect();
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

        let failed_assignments =
            assign_shards(worker_executors.clone(), rebalance.get_assignments(), addrs).await;

        RebalanceFailures {
            failed_assignments,
            failed_unassignments,
        }
    }
}

/// Applies the executor changes queued since the last pass and returns the executors that must
/// receive their full shard assignment.
///
/// New executors are added before removed ones are dropped, so that a restart re-registering at
/// an address whose previous instance was also reported unhealthy still takes over its shards;
/// the removal of the replaced instance is then a no-op.
fn apply_executor_changes(
    shard_state: &mut ShardLeaseState,
    new_executors: BTreeMap<ExecutorAddr, ExecutorRegistration>,
    removed_executors: HashSet<ExecutorId>,
    full_assignment_requests: HashSet<ExecutorId>,
    lease_ttl: Duration,
) -> HashSet<ExecutorId> {
    // Every lease granted in this pass starts at the same instant.
    let now = Utc::now();
    let mut full_assignment_executors: HashSet<ExecutorId> = HashSet::new();

    for registration in new_executors.into_values() {
        let ExecutorRegistration {
            executor_id,
            addr,
            pod_name,
        } = registration;
        match shard_state.add_executor(executor_id, addr, pod_name, now, lease_ttl) {
            Some(replaced) => {
                // A new instance at a known address: its predecessor's shards were transferred
                // to it, and it has to receive the full list of its assigned shards.
                full_assignment_executors.insert(executor_id);
                info!(
                    executor_id = %executor_id,
                    replaced_executor_id = %replaced,
                    addr = %addr,
                    "Executor replaced at address"
                );
            }
            None => {
                info!(executor_id = %executor_id, addr = %addr, "Executor added");
            }
        }
    }

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

#[derive(Debug)]
struct RebalanceFailures {
    failed_assignments: Vec<(ExecutorId, BTreeSet<ShardId>)>,
    failed_unassignments: Vec<(ExecutorId, BTreeSet<ShardId>)>,
}

#[derive(Debug, Clone)]
struct ExecutorRegistration {
    executor_id: ExecutorId,
    addr: ExecutorAddr,
    pod_name: Option<String>,
}

/// Changes accumulated between two passes of the shard management loop.
#[derive(Debug)]
struct ShardManagementChanges {
    new_executors: BTreeMap<ExecutorAddr, ExecutorRegistration>,
    removed_executors: HashSet<ExecutorId>,
    full_assignment_requests: HashSet<ExecutorId>,
}

impl ShardManagementChanges {
    pub fn new(
        full_assignment_requests: HashSet<ExecutorId>,
        removed_executors: HashSet<ExecutorId>,
    ) -> Self {
        ShardManagementChanges {
            new_executors: BTreeMap::new(),
            removed_executors,
            full_assignment_requests,
        }
    }

    /// Queues a registration. `executor_id` is always freshly minted, so it cannot already be
    /// queued for removal or for a full assignment.
    pub fn add_executor_registration(
        &mut self,
        executor_id: ExecutorId,
        addr: ExecutorAddr,
        pod_name: Option<String>,
    ) {
        self.new_executors.insert(
            addr,
            ExecutorRegistration {
                executor_id,
                addr,
                pod_name,
            },
        );
    }

    pub fn remove_executor(&mut self, executor_id: ExecutorId) {
        self.new_executors
            .retain(|_, registration| registration.executor_id != executor_id);
        self.full_assignment_requests.remove(&executor_id);
        self.removed_executors.insert(executor_id);
    }

    pub fn retry_full_assignment(&mut self, executor_id: ExecutorId) {
        if !self.removed_executors.contains(&executor_id) {
            self.full_assignment_requests.insert(executor_id);
        }
    }

    #[allow(clippy::type_complexity)]
    pub fn reset(
        &mut self,
    ) -> (
        BTreeMap<ExecutorAddr, ExecutorRegistration>,
        HashSet<ExecutorId>,
        HashSet<ExecutorId>,
    ) {
        let new_executors = std::mem::take(&mut self.new_executors);
        let removed = std::mem::take(&mut self.removed_executors);
        let full = std::mem::take(&mut self.full_assignment_requests);
        (new_executors, removed, full)
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

    fn executor_registration(executor_id: ExecutorId, addr: ExecutorAddr) -> ExecutorRegistration {
        ExecutorRegistration {
            executor_id,
            addr,
            pod_name: None,
        }
    }

    #[test]
    fn removal_and_reregistration_at_the_same_address_in_one_pass_transfer_the_shards() {
        let mut shard_state = ShardLeaseState::new(4);
        shard_state.add_executor(executor(1), addr(1), None, t0(), TTL);
        shard_state.add_executor(executor(2), addr(2), None, t0(), TTL);
        for shard_id in [0, 1] {
            shard_state.assign_shard(executor(1), ShardId::new(shard_id));
        }
        for shard_id in [2, 3] {
            shard_state.assign_shard(executor(2), ShardId::new(shard_id));
        }

        // the health check reported executor 1 unhealthy and the restarted process at the same
        // address registered (as executor 3) before the loop woke up
        let new_executors =
            BTreeMap::from([(addr(1), executor_registration(executor(3), addr(1)))]);
        let removed = HashSet::from([executor(1)]);

        let full = apply_executor_changes(
            &mut shard_state,
            new_executors,
            removed,
            HashSet::new(),
            TTL,
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
            BTreeMap::new(),
            HashSet::from([executor(1)]),
            HashSet::from([executor(1), executor(2), executor(9)]),
            TTL,
        );

        assert_eq!(full, HashSet::from([executor(2)]));
        assert!(!shard_state.has_executor(executor(1)));
        assert_eq!(shard_state.pending_rebalance, shards(&[0]));
        assert_eq!(shard_state.get_unassigned_shards(), shards(&[0, 2, 3]));
    }

    #[test]
    fn brand_new_executor_is_added_without_a_full_assignment() {
        let mut shard_state = ShardLeaseState::new(4);
        shard_state.add_executor(executor(1), addr(1), None, t0(), TTL);

        let full = apply_executor_changes(
            &mut shard_state,
            BTreeMap::from([(addr(2), executor_registration(executor(2), addr(2)))]),
            HashSet::new(),
            HashSet::new(),
            TTL,
        );

        assert!(full.is_empty());
        assert!(shard_state.has_executor(executor(2)));
        assert_eq!(shard_state.executor_count(), 2);
    }

    #[test]
    fn unregistering_drops_a_queued_registration_of_the_same_executor() {
        let mut changes = ShardManagementChanges::new(HashSet::new(), HashSet::new());
        changes.add_executor_registration(executor(1), addr(1), None);
        changes.add_executor_registration(executor(2), addr(1), None); // last registration per address wins
        changes.add_executor_registration(executor(3), addr(3), None);
        changes.remove_executor(executor(3));
        changes.retry_full_assignment(executor(3)); // ignored: queued for removal
        changes.retry_full_assignment(executor(4));

        let (new_executors, removed, full) = changes.reset();
        assert_eq!(new_executors.len(), 1);
        assert_eq!(new_executors[&addr(1)].executor_id, executor(2));
        assert_eq!(removed, HashSet::from([executor(3)]));
        assert_eq!(full, HashSet::from([executor(4)]));

        let (new_executors, removed, full) = changes.reset();
        assert!(new_executors.is_empty() && removed.is_empty() && full.is_empty());
    }
}
