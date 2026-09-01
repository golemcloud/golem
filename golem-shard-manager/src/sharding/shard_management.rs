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
use super::healthcheck::{HealthCheck, get_unhealthy_pods};
use super::model::{Assignments, RoutingTable};
use super::persistence::RoutingTablePersistence;
use super::rebalancing::Rebalance;
use super::worker_executor::{
    WorkerExecutorService, assign_shards, revoke_shards, set_shard_assignments,
};
use async_rwlock::RwLock;
use golem_common::model::{Pod, ShardId};
use itertools::Itertools;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::Arc;
use tokio::sync::{Mutex, Notify};
use tokio::task::JoinSet;
use tracing::{Instrument, debug, info, warn};

#[derive(Clone)]
pub struct ShardManagement {
    routing_table: Arc<RwLock<RoutingTable>>,
    change: Arc<Notify>,
    updates: Arc<Mutex<ShardManagementChanges>>,
}

impl ShardManagement {
    /// Initializes the shard management with an initial routing table and optionally
    /// a pending rebalance, both read from the persistence service.
    pub async fn new(
        persistence_service: Arc<dyn RoutingTablePersistence>,
        worker_executors: Arc<dyn WorkerExecutorService>,
        health_check: Arc<dyn HealthCheck>,
        threshold: f64,
        join_set: &mut JoinSet<anyhow::Result<()>>,
    ) -> Result<Self, ShardManagerError> {
        let routing_table = persistence_service.read().await?;

        info!("Initial healthcheck started");

        let pods = routing_table.get_pods_with_names();

        let unhealthy_pods = get_unhealthy_pods(&health_check, &pods).await;
        let healthy_pods = pods
            .into_iter()
            .filter(|(p, _)| !unhealthy_pods.contains(p))
            .collect();

        info!("Initial healthcheck finished");

        let change = Arc::new(Notify::new());
        // NOTE: We consider all healthy pods as new pods to trigger full assigment, given they might be lagging:
        //       this can happen with interleaved shard-manager and worker restarts.
        //       The first pass reconciles them BEFORE its rebalance (the pre-rebalance stage in
        //       `worker`), so a lagging pod drops stale shards before they can be reassigned.
        let updates = Arc::new(Mutex::new(ShardManagementChanges::new(
            healthy_pods,
            unhealthy_pods,
        )));
        let routing_table = Arc::new(RwLock::new(routing_table));

        {
            let change = change.clone();
            let updates = updates.clone();
            let routing_table = routing_table.clone();

            join_set.spawn(
                async move {
                    Self::worker(
                        routing_table,
                        change,
                        updates,
                        persistence_service,
                        worker_executors,
                        threshold,
                    )
                    .await;
                    Ok(())
                }
                .in_current_span(),
            );
        };

        change.notify_one();

        Ok(ShardManagement {
            routing_table,
            change,
            updates,
        })
    }

    /// Registers a new pod to be added
    pub async fn register_pod(&self, pod: Pod, pod_name: Option<String>) {
        debug!(pod=%pod, "Registering pod");
        self.updates.lock().await.add_new_pod(pod, pod_name);
        self.change.notify_one();
    }

    /// Marks a pod to be removed
    pub async fn unregister_pod(&self, pod: Pod) {
        debug!(pod=%pod, "Unregistering pod");
        self.updates.lock().await.remove_pod(pod);
        self.change.notify_one();
    }

    /// Gets the current snapshot of the routing table
    pub async fn current_snapshot(&self) -> RoutingTable {
        self.routing_table.read().await.clone()
    }

    async fn worker(
        routing_table: Arc<RwLock<RoutingTable>>,
        change: Arc<Notify>,
        updates: Arc<Mutex<ShardManagementChanges>>,
        persistence_service: Arc<dyn RoutingTablePersistence>,
        worker_executors: Arc<dyn WorkerExecutorService>,
        threshold: f64,
    ) {
        loop {
            debug!("Shard management loop awaiting changes");
            change.notified().await;

            let (new_pods, removed_pods, retry_full_assignment_pods) = updates.lock().await.reset();
            debug!(
                new_pods = new_pods.keys().join(", "),
                removed_pods = removed_pods.iter().join(", "),
                retry_pods = retry_full_assignment_pods.iter().join(", "),
                "Shard management loop woken up",
            );

            // Getting a write lock while
            //   - the rebalance plan is calculated,
            //   - new and removed pods are added to the routing table and got persisted,
            //   - a snapshot of that persisted state is taken for the pre-rebalance reconciles,
            // but the rebalance plan is NOT applied yet. The lock is then release for apply.
            let (mut rebalance, carried_over_pods, pass_start_snapshot) = {
                let mut current_routing_table = routing_table.write().await;

                for pod in removed_pods {
                    current_routing_table.remove_pod(pod);
                    info!(pod= %pod, "Pod removed");
                }

                let mut send_full_assignment = Vec::new();
                for (pod, pod_name) in new_pods {
                    if current_routing_table.has_pod(pod) {
                        // This pod has already an assignment - we have to send the full list of assigned shards to it
                        send_full_assignment.push(pod);
                        info!(pod= %pod, "Pod returned");
                    } else {
                        // New pod, adding with empty assignment
                        current_routing_table.add_pod(pod, pod_name);
                        info!(pod= %pod, "Pod added");
                    }
                }
                let rebalance = Rebalance::from_routing_table(&current_routing_table, threshold);

                let mut carried_over_pods: HashSet<Pod> = HashSet::new();

                for pod in send_full_assignment {
                    carried_over_pods.insert(pod);
                }

                for pod in retry_full_assignment_pods {
                    if current_routing_table.has_pod(pod) {
                        carried_over_pods.insert(pod);
                    }
                }

                persistence_service
                    .write(&current_routing_table)
                    .await
                    .expect("Failed to persist routing table after pod changes");

                (rebalance, carried_over_pods, current_routing_table.clone())
            };

            // Pods carried over from a previous pass (failed reconciles) or (re)connecting pods
            // may still hold shards the routing table no longer credits them with; they get their
            // authoritative assignment BEFORE the rebalance below can hand any of those shards to
            // another pod. Failures are retried once more this pass from the post-rebalance
            // snapshot (the pod is still in the table mid-pass), and a failure of that retry is
            // re-queued for the next pass by the handler further down.
            let mut full_assignment_pods: HashSet<Pod> = HashSet::new();
            if !carried_over_pods.is_empty() {
                let pre_assignments =
                    Self::full_assignments_for(&pass_start_snapshot, &carried_over_pods);
                let failed_pre_sets = if pre_assignments.is_empty() {
                    Vec::new()
                } else {
                    set_shard_assignments(
                        worker_executors.clone(),
                        pass_start_snapshot.number_of_shards,
                        &pre_assignments,
                    )
                    .await
                };
                if !failed_pre_sets.is_empty() {
                    warn!(
                        failed_pods = failed_pre_sets.iter().map(|(pod, _)| pod).join(", "),
                        "Some pods could not receive their authoritative shard assignment before the rebalance; retrying after it"
                    );
                    for (pod, _) in &failed_pre_sets {
                        full_assignment_pods.insert(*pod);
                    }
                }
            }

            debug!(rebalance=%rebalance, "Applying rebalance plan");
            let rebalance_failures =
                Self::execute_rebalance(worker_executors.clone(), &mut rebalance).await;

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
                    "Some shards could not be assigned; they are left unassigned and the pods get their authoritative assignment"
                );

                // The executor may have applied the assignment even though the call failed
                // (for example a timeout), so the pod receives its authoritative assignment in
                // this pass - the post-rebalance snapshot no longer credits it with the failed
                // shards - before the next pass can hand them to another pod. If that
                // reconciliation fails too, it is re-queued below and re-sent at the START of the
                // next pass, before that pass's rebalance.
                for (pod, _) in &rebalance_failures.failed_assignments {
                    full_assignment_pods.insert(*pod);
                }
                needs_retry = true;
            }

            if !rebalance_failures.failed_unassignments.is_empty() {
                warn!(
                    failed_pods = rebalance_failures
                        .failed_unassignments
                        .iter()
                        .map(|(pod, _)| pod)
                        .join(", "),
                    "Some shards could not be unassigned; they are left unassigned and the pods get their authoritative assignment"
                );
                // A failed revoke does not mean the executor still holds the shards - it may have
                // dropped them and only the response was lost. The shards are left unassigned in
                // the routing table (the unassignment stays in the plan), and the pod receives its
                // authoritative assignment in this pass, before the next pass hands the shards to
                // another pod. If that reconciliation fails too, it is re-queued below and re-sent
                // at the START of the next pass, before that pass's rebalance.
                for (pod, _) in &rebalance_failures.failed_unassignments {
                    full_assignment_pods.insert(*pod);
                }
                needs_retry = true;
            }

            routing_table.write().await.rebalance(rebalance);

            let routing_table_snapshot = routing_table.read().await.clone();
            persistence_service
                .write(&routing_table_snapshot)
                .await
                .expect("Failed to persist routing table after rebalance");

            let full_assignments =
                Self::full_assignments_for(&routing_table_snapshot, &full_assignment_pods);

            let failed_full_assignments = if full_assignments.is_empty() {
                Vec::new()
            } else {
                set_shard_assignments(
                    worker_executors.clone(),
                    routing_table_snapshot.number_of_shards,
                    &full_assignments,
                )
                .await
            };

            if !failed_full_assignments.is_empty() {
                warn!(
                    failed_pods = failed_full_assignments
                        .iter()
                        .map(|(pod, _)| pod)
                        .join(", "),
                    "Some pods could not receive authoritative shard assignment and will be retried"
                );

                {
                    let mut updates_guard = updates.lock().await;
                    for (pod, _) in &failed_full_assignments {
                        updates_guard.retry_full_assignment(*pod);
                    }
                }
                needs_retry = true;
            }

            if needs_retry {
                change.notify_one();
            }
        }
    }

    /// The full authoritative assignment message for each of `pods`, read from `snapshot`.
    /// A pod not in the snapshot gets none (it was removed meanwhile); a pod without shards
    /// gets an explicit empty assignment, which tells it to drop everything it still holds.
    fn full_assignments_for(snapshot: &RoutingTable, pods: &HashSet<Pod>) -> Assignments {
        let mut full_assignments = Assignments::new();
        for pod in pods {
            if let Some(mut shard_ids) = snapshot.get_shards(*pod) {
                full_assignments
                    .assignments
                    .entry(*pod)
                    .or_default()
                    .append(&mut shard_ids);
            }
        }
        full_assignments
    }

    async fn execute_rebalance(
        worker_executors: Arc<dyn WorkerExecutorService + Send + Sync>,
        rebalance: &mut Rebalance,
    ) -> RebalanceFailures {
        info!("Beginning rebalance...");

        if !rebalance.get_unassignments().is_empty() {
            info!(
                unassignments = %rebalance.get_unassignments(),
                "Executing shard unassignments",
            );
        }
        let failed_unassignments =
            revoke_shards(worker_executors.clone(), rebalance.get_unassignments()).await;
        let failed_shards: HashSet<ShardId> = failed_unassignments
            .iter()
            .flat_map(|(_, shard_ids)| shard_ids.clone())
            .collect();
        rebalance.remove_assignment_shards(&failed_shards);
        if !failed_shards.is_empty() {
            warn!(
                failed_shards = failed_shards.iter().join(", "),
                "Some shards could not be unassigned and are left unassigned for retry"
            );
        }

        if !rebalance.get_assignments().is_empty() {
            info!(
                assignments=%rebalance.get_assignments(),
                "Executing shard assignments",
            );
        }

        let failed_assignments =
            assign_shards(worker_executors.clone(), rebalance.get_assignments()).await;

        RebalanceFailures {
            failed_assignments,
            failed_unassignments,
        }
    }
}

#[derive(Debug)]
struct RebalanceFailures {
    failed_assignments: Vec<(Pod, BTreeSet<ShardId>)>,
    failed_unassignments: Vec<(Pod, BTreeSet<ShardId>)>,
}

#[derive(Debug)]
struct ShardManagementChanges {
    new_pods: HashMap<Pod, Option<String>>,
    removed_pods: HashSet<Pod>,
    retry_full_assignment_pods: HashSet<Pod>,
}

impl ShardManagementChanges {
    pub fn new(new_pods: HashMap<Pod, Option<String>>, removed_pods: HashSet<Pod>) -> Self {
        ShardManagementChanges {
            new_pods,
            removed_pods,
            retry_full_assignment_pods: HashSet::new(),
        }
    }

    pub fn add_new_pod(&mut self, pod: Pod, pod_name: Option<String>) {
        self.removed_pods.remove(&pod);
        self.retry_full_assignment_pods.remove(&pod);
        self.new_pods.insert(pod, pod_name);
    }

    pub fn remove_pod(&mut self, pod: Pod) {
        self.new_pods.remove(&pod);
        self.retry_full_assignment_pods.remove(&pod);
        self.removed_pods.insert(pod);
    }

    pub fn retry_full_assignment(&mut self, pod: Pod) {
        if !self.removed_pods.contains(&pod) {
            self.retry_full_assignment_pods.insert(pod);
        }
    }

    pub fn reset(&mut self) -> (HashMap<Pod, Option<String>>, HashSet<Pod>, HashSet<Pod>) {
        let new = self.new_pods.clone();
        let removed = self.removed_pods.clone();
        let retry = self.retry_full_assignment_pods.clone();
        self.new_pods.clear();
        self.removed_pods.clear();
        self.retry_full_assignment_pods.clear();
        (new, removed, retry)
    }
}
