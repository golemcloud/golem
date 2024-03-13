use crate::error::ShardManagerError;
use crate::model::{Pod, RoutingTable};
use crate::persistence::PersistenceService;
use crate::rebalancing::Rebalance;
use crate::worker_executor::{
    assign_shards, get_unhealthy_pods, revoke_shards, WorkerExecutorService,
};
use async_rwlock::RwLock;
use std::collections::HashSet;
use std::ops::Deref;
use std::sync::Arc;
use tokio::sync::{Mutex, Notify};
use tokio::task::JoinHandle;
use tracing::info;

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
        persistence_service: Arc<dyn PersistenceService + Send + Sync>,
        worker_executors: Arc<dyn WorkerExecutorService + Send + Sync>,
        threshold: f64,
    ) -> Result<Self, ShardManagerError> {
        let (routing_table, mut pending_rebalance) = persistence_service.read().await.unwrap();
        let routing_table = Arc::new(RwLock::new(routing_table));

        if !pending_rebalance.is_empty() {
            info!("Conducting health check of pods involved in rebalance");
            let pods = routing_table.read().await.get_pods();
            let unhealthy_pods = get_unhealthy_pods(worker_executors.clone(), &pods).await;
            pending_rebalance.remove_pods(&unhealthy_pods);
            info!("The following pods were found to be unhealthy and have been removed from rebalance: {:?}", unhealthy_pods);

            info!(
                "Writing planned rebalance: {} to persistent storage",
                pending_rebalance
            );
            persistence_service
                .write(routing_table.read().await.deref(), &pending_rebalance)
                .await?;
            info!("Planned rebalance written to persistent storage");

            Self::execute_rebalance(worker_executors.clone(), &mut pending_rebalance).await?;

            routing_table.write().await.rebalance(pending_rebalance);
            persistence_service
                .write(&routing_table.read().await.clone(), &Rebalance::empty())
                .await
                .expect("Failed to persist routing table");
        }

        let change = Arc::new(Notify::new());
        let updates = Arc::new(Mutex::new(ShardManagementChanges::new()));

        let routing_table_clone = routing_table.clone();
        let notify_clone = change.clone();
        let updates_clone = updates.clone();

        let worker_handle = Arc::new(WorkerHandle::new(tokio::spawn(async move {
            Self::worker(
                routing_table_clone,
                notify_clone,
                updates_clone,
                persistence_service,
                worker_executors,
                threshold,
            )
            .await
        })));

        Ok(ShardManagement {
            routing_table,
            change,
            worker_handle,
            updates,
        })
    }

    /// Registers a new pod to be added
    pub async fn register_pod(&self, pod: Pod) {
        self.updates.lock().await.add_new_pod(pod);
        self.change.notify_one();
    }

    /// Marks a pod to be removed
    pub async fn unregister_pod(&self, pod: Pod) {
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
        persistence_service: Arc<dyn PersistenceService + Send + Sync>,
        worker_executors: Arc<dyn WorkerExecutorService + Send + Sync>,
        threshold: f64,
    ) {
        loop {
            change.notified().await;
            let (new_pods, removed_pods) = updates.lock().await.reset();
            let mut current_routing_table = routing_table.read().await.clone();

            for pod in removed_pods {
                current_routing_table.remove_pod(&pod);
            }

            let mut send_full_assignment = Vec::new();
            for pod in new_pods {
                if current_routing_table.has_pod(&pod) {
                    // This pod has already an assignment - we have to send the full list of assigned shards to it
                    send_full_assignment.push(pod.clone());

                    info!("Registered worker executor returned: {pod}")
                } else {
                    // New pod, adding with empty assignment
                    current_routing_table.add_pod(&pod);

                    info!("Registered new worker executor: {pod}")
                }
            }
            let mut rebalance = Rebalance::from_routing_table(&current_routing_table, threshold);

            for pod in send_full_assignment {
                let assignments = current_routing_table.get_shards(&pod).unwrap_or_default();
                rebalance.add_assignments(&pod, assignments);
            }

            if !rebalance.is_empty() {
                // Panicking in case any of the rebalancing steps fail (after some internal retries within those steps).
                // This causes the shard manager to get restarted and have and retry the rebalance on next startup.

                persistence_service
                    .write(&routing_table.read().await.clone(), &rebalance)
                    .await
                    .expect("Failed to persist routing table");

                Self::execute_rebalance(worker_executors.clone(), &mut rebalance)
                    .await
                    .expect("Failed to execute rebalance");

                routing_table.write().await.rebalance(rebalance);
                persistence_service
                    .write(&routing_table.read().await.clone(), &Rebalance::empty())
                    .await
                    .expect("Failed to persist routing table");
            } else {
                info!("No rebalance necessary");
            }
        }
    }

    async fn execute_rebalance(
        worker_executors: Arc<dyn WorkerExecutorService + Send + Sync>,
        rebalance: &mut Rebalance,
    ) -> Result<(), ShardManagerError> {
        info!("Shard manager beginning rebalance...");

        info!(
            "Executing shard unassignments: {}",
            rebalance.get_unassignments()
        );
        let failed_unassignments =
            revoke_shards(worker_executors.clone(), rebalance.get_unassignments()).await;
        let failed_shards = failed_unassignments
            .iter()
            .flat_map(|(_, shard_ids)| shard_ids.clone())
            .collect();
        rebalance.remove_shards(&failed_shards);
        info!("The following shards could not be unassigned and have been removed from rebalance: {:?}", failed_shards);

        info!(
            "Executing shard assignments: {}",
            rebalance.get_assignments()
        );
        assign_shards(worker_executors.clone(), rebalance.get_assignments()).await;

        Ok(())
    }
}

#[derive(Debug)]
struct ShardManagementChanges {
    new_pods: HashSet<Pod>,
    removed_pods: HashSet<Pod>,
}

impl ShardManagementChanges {
    pub fn new() -> Self {
        ShardManagementChanges {
            new_pods: HashSet::new(),
            removed_pods: HashSet::new(),
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
