use crate::model::{Pod, RoutingTable};
use crate::rebalancing::Rebalance;
use async_rwlock::RwLock;
use std::collections::HashSet;
use std::sync::Arc;
use tokio::sync::{Mutex, Notify};
use tokio::task::JoinHandle;

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
    /// a pending rebalance.
    pub async fn new(routing_table: RoutingTable, pending_rebalance: Rebalance) -> Self {
        let routing_table = Arc::new(RwLock::new(routing_table));

        if !pending_rebalance.is_empty() {
            // TODO: persist rebalance plan
            // TODO: execute rebalance
            routing_table.write().await.rebalance(pending_rebalance);
            // TODO: persist new routing table
        }

        let change = Arc::new(Notify::new());
        let updates = Arc::new(Mutex::new(ShardManagementChanges::new()));

        let routing_table_clone = routing_table.clone();
        let notify_clone = change.clone();
        let updates_clone = updates.clone();

        let worker_handle = Arc::new(WorkerHandle::new(tokio::spawn(async move {
            Self::worker(routing_table_clone, notify_clone, updates_clone).await
        })));

        ShardManagement {
            routing_table,
            change,
            worker_handle,
            updates,
        }
    }

    /// Registers a new pod to be added
    pub async fn register_pod(&self, pod: Pod) {
        self.updates.lock().await.add_new_pod(pod);
    }

    /// Marks a pod to be removed
    pub async fn unregister_pod(&self, pod: Pod) {
        self.updates.lock().await.remove_pod(pod);
    }

    /// Gets the current snapshot of the routing table
    pub async fn current_snapshot(&self) -> RoutingTable {
        self.routing_table.read().await.clone()
    }

    async fn worker(
        routing_table: Arc<RwLock<RoutingTable>>,
        change: Arc<Notify>,
        updates: Arc<Mutex<ShardManagementChanges>>,
    ) {
        loop {
            change.notified().await;
            let (new_pods, removed_pods) = updates.lock().await.reset();
            let mut current_routing_table = routing_table.read().await.clone();

            for pod in removed_pods {
                current_routing_table.remove_pod(&pod);
            }

            for pod in new_pods {
                current_routing_table.add_pod(&pod);
            }
            let rebalance = Rebalance::from_routing_table(&current_routing_table, 0.0); // TODO: configurable threshold

            // TODO: persist rebalance plan
            // TODO: execute rebalance
            routing_table.write().await.rebalance(rebalance);
            // TODO: persist new routing table
        }
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
