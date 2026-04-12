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
use super::model::RoutingTable;
use super::persistence::RoutingTablePersistence;
use super::rebalancing::Rebalance;
use super::worker_executor::{WorkerExecutorService, assign_shards, revoke_shards};
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
        //       this can happen with interleaved shard-manager and worker restarts
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
            // but the rebalance plan is NOT applied yet. The lock is then release for apply.
            let (mut rebalance, full_assignment_pods) = {
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
                let mut rebalance =
                    Rebalance::from_routing_table(&current_routing_table, threshold);

                let mut full_assignment_pods: HashSet<Pod> = HashSet::new();

                for pod in send_full_assignment {
                    let assignments = current_routing_table.get_shards(pod).unwrap_or_default();
                    rebalance.add_assignments(&pod, assignments);
                    full_assignment_pods.insert(pod);
                }

                for pod in retry_full_assignment_pods {
                    if current_routing_table.has_pod(pod) {
                        let assignments = current_routing_table.get_shards(pod).unwrap_or_default();
                        rebalance.add_assignments(&pod, assignments);
                        full_assignment_pods.insert(pod);
                    }
                }

                persistence_service
                    .write(&current_routing_table)
                    .await
                    .expect("Failed to persist routing table after pod changes");

                (rebalance, full_assignment_pods)
            };

            debug!(rebalance=%rebalance, "Applying rebalance plan");
            let failed_assignments =
                Self::execute_rebalance(worker_executors.clone(), &mut rebalance).await;

            let mut needs_retry = false;
            if !failed_assignments.is_empty() {
                let failed_shards: HashSet<ShardId> = failed_assignments
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
                    for (pod, _) in &failed_assignments {
                        if full_assignment_pods.contains(pod) {
                            updates_guard.retry_full_assignment(*pod);
                        }
                    }
                }
                needs_retry = true;
            }

            routing_table.write().await.rebalance(rebalance);
            persistence_service
                .write(&routing_table.read().await.clone())
                .await
                .expect("Failed to persist routing table after rebalance");

            if needs_retry {
                change.notify_one();
            }
        }
    }

    async fn execute_rebalance(
        worker_executors: Arc<dyn WorkerExecutorService + Send + Sync>,
        rebalance: &mut Rebalance,
    ) -> Vec<(Pod, BTreeSet<ShardId>)> {
        info!("Beginning rebalance...");

        if !rebalance.get_unassignments().is_empty() {
            info!(
                unassignments = %rebalance.get_unassignments(),
                "Executing shard unassignments",
            );
        }
        let failed_unassignments =
            revoke_shards(worker_executors.clone(), rebalance.get_unassignments()).await;
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

        assign_shards(worker_executors.clone(), rebalance.get_assignments()).await
    }
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
