// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use std::collections::HashSet;
use std::sync::Arc;

use async_rwlock::RwLock;
use itertools::Itertools;
use tokio::sync::{Mutex, Notify};
use tokio::task::JoinHandle;
use tracing::{debug, info, warn, Instrument};

use crate::error::ShardManagerError;
use crate::healthcheck::{get_unhealthy_pods, HealthCheck};
use crate::model::{Pod, RoutingTable};
use crate::persistence::RoutingTablePersistence;
use crate::rebalancing::Rebalance;
use crate::worker_executor::{assign_shards, revoke_shards, WorkerExecutorService};

#[derive(Clone)]
pub struct ShardManagement {
    routing_table: Arc<RwLock<RoutingTable>>,
    change: Arc<Notify>,
    #[allow(dead_code)]
    worker_handle: Arc<WorkerHandle>, // Just kept here for abort on dropping
    updates: Arc<Mutex<ShardManagementChanges>>,
}

impl ShardManagement {
    /// Initializes the shard management with an initial routing table and optionally
    /// a pending rebalance, both read from the persistence service.
    pub async fn new(
        persistence_service: Arc<dyn RoutingTablePersistence + Send + Sync>,
        worker_executors: Arc<dyn WorkerExecutorService + Send + Sync>,
        health_check: Arc<dyn HealthCheck + Send + Sync>,
        threshold: f64,
    ) -> Result<Self, ShardManagerError> {
        let routing_table = persistence_service.read().await.unwrap();

        info!("Initial healthcheck started");

        let mut pods = routing_table.get_pods();
        let unhealthy_pods = get_unhealthy_pods(health_check, &pods).await;
        pods.retain(|pod| !unhealthy_pods.contains(pod));

        info!("Initial healthcheck finished");

        let change = Arc::new(Notify::new());
        // NOTE: We consider all healthy pods as new pods to trigger full assigment, given they might be lagging:
        //       this can happen with interleaved shard-manager and worker restarts
        let updates = Arc::new(Mutex::new(ShardManagementChanges::new(
            pods,
            unhealthy_pods,
        )));
        let routing_table = Arc::new(RwLock::new(routing_table));

        let worker_handle = {
            let change = change.clone();
            let updates = updates.clone();
            let routing_table = routing_table.clone();

            Arc::new(WorkerHandle::new(tokio::spawn(async move {
                Self::worker(
                    routing_table,
                    change,
                    updates,
                    persistence_service,
                    worker_executors,
                    threshold,
                )
                .in_current_span()
                .await
            })))
        };

        change.notify_one();

        Ok(ShardManagement {
            routing_table,
            change,
            worker_handle,
            updates,
        })
    }

    /// Registers a new pod to be added
    pub async fn register_pod(&self, pod: Pod) {
        debug!(pod=%pod, "Registering pod");
        self.updates.lock().await.add_new_pod(pod);
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
        persistence_service: Arc<dyn RoutingTablePersistence + Send + Sync>,
        worker_executors: Arc<dyn WorkerExecutorService + Send + Sync>,
        threshold: f64,
    ) {
        loop {
            debug!("Shard management loop awaiting changes");
            change.notified().await;

            let (new_pods, removed_pods) = updates.lock().await.reset();
            debug!(
                new_pods = new_pods.iter().join(", "),
                removed_pods = removed_pods.iter().join(", "),
                "Shard management loop woken up",
            );

            // Getting a write lock while
            //   - the rebalance plan is calculated,
            //   - new and removed pods are added to the routing table and got persisted,
            // but the rebalance plan is NOT applied yet. The lock is then release for apply.
            let mut rebalance = {
                let mut current_routing_table = routing_table.write().await;

                for pod in removed_pods {
                    current_routing_table.remove_pod(&pod);
                    info!(pod= %pod, "Pod removed");
                }

                let mut send_full_assignment = Vec::new();
                for pod in new_pods {
                    if current_routing_table.has_pod(&pod) {
                        // This pod has already an assignment - we have to send the full list of assigned shards to it
                        send_full_assignment.push(pod.clone());
                        info!(pod= %pod, "Pod returned");
                    } else {
                        // New pod, adding with empty assignment
                        current_routing_table.add_pod(&pod);
                        info!(pod= %pod, "Pod added");
                    }
                }
                let mut rebalance =
                    Rebalance::from_routing_table(&current_routing_table, threshold);

                for pod in send_full_assignment {
                    let assignments = current_routing_table.get_shards(&pod).unwrap_or_default();
                    rebalance.add_assignments(&pod, assignments);
                }

                persistence_service
                    .write(&current_routing_table)
                    .await
                    .expect("Failed to persist routing table after pod changes");

                rebalance
            };

            debug!(rebalance=%rebalance, "Applying rebalance plan");
            Self::execute_rebalance(worker_executors.clone(), &mut rebalance).await;

            routing_table.write().await.rebalance(rebalance);
            persistence_service
                .write(&routing_table.read().await.clone())
                .await
                .expect("Failed to persist routing table after rebalance");
        }
    }

    async fn execute_rebalance(
        worker_executors: Arc<dyn WorkerExecutorService + Send + Sync>,
        rebalance: &mut Rebalance,
    ) {
        info!("Shard manager beginning rebalance...");

        info!(
            unassignments = %rebalance.get_unassignments(),
            "Executing shard unassignments",
        );
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

        info!(
            assignments=%rebalance.get_assignments(),
            "Executing shard assignments",
        );
        assign_shards(worker_executors.clone(), rebalance.get_assignments()).await;
    }
}

#[derive(Debug)]
struct ShardManagementChanges {
    new_pods: HashSet<Pod>,
    removed_pods: HashSet<Pod>,
}

impl ShardManagementChanges {
    pub fn new(new_pods: HashSet<Pod>, removed_pods: HashSet<Pod>) -> Self {
        ShardManagementChanges {
            new_pods,
            removed_pods,
        }
    }

    pub fn add_new_pod(&mut self, pod: Pod) {
        self.removed_pods.remove(&pod);
        self.new_pods.insert(pod);
    }

    pub fn remove_pod(&mut self, pod: Pod) {
        self.new_pods.remove(&pod);
        self.removed_pods.insert(pod);
    }

    pub fn reset(&mut self) -> (HashSet<Pod>, HashSet<Pod>) {
        let new = self.new_pods.clone();
        let removed = self.removed_pods.clone();
        self.new_pods.clear();
        self.removed_pods.clear();
        (new, removed)
    }
}

struct WorkerHandle(JoinHandle<()>);

impl WorkerHandle {
    pub fn new(handle: JoinHandle<()>) -> Self {
        WorkerHandle(handle)
    }
}

impl Drop for WorkerHandle {
    fn drop(&mut self) {
        self.0.abort();
    }
}
