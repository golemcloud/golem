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
use golem_common::model::{Pod, ShardId};
use golem_shard_manager::{
    HealthCheck, HealthCheckError, PodState, RoutingTable, RoutingTablePersistence,
    ShardManagement, ShardManagerError, WorkerExecutorService,
};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::net::{IpAddr, Ipv4Addr};
use std::sync::Arc;
use std::time::Duration;
use test_r::test;
use tokio::sync::Mutex;
use tokio::task::JoinSet;
use tokio::time::Instant;

#[derive(Clone, Debug)]
struct TestPersistence {
    state: Arc<Mutex<RoutingTable>>,
    writes: Arc<Mutex<Vec<RoutingTable>>>,
}

impl TestPersistence {
    fn new(initial: RoutingTable) -> Self {
        Self {
            state: Arc::new(Mutex::new(initial)),
            writes: Arc::new(Mutex::new(Vec::new())),
        }
    }

    async fn latest(&self) -> RoutingTable {
        self.state.lock().await.clone()
    }
}

#[async_trait]
impl RoutingTablePersistence for TestPersistence {
    async fn write(&self, routing_table: &RoutingTable) -> Result<(), ShardManagerError> {
        *self.state.lock().await = routing_table.clone();
        self.writes.lock().await.push(routing_table.clone());
        Ok(())
    }

    async fn read(&self) -> Result<RoutingTable, ShardManagerError> {
        Ok(self.state.lock().await.clone())
    }
}

#[derive(Clone, Debug, Default)]
struct TestWorkerExecutors {
    local_assignments: Arc<Mutex<HashMap<Pod, BTreeSet<ShardId>>>>,
    failed_assignments: Arc<Mutex<HashMap<Pod, usize>>>,
    failed_revocations: Arc<Mutex<HashMap<Pod, usize>>>,
    failed_reconciliations: Arc<Mutex<HashMap<Pod, usize>>>,
}

impl TestWorkerExecutors {
    async fn set_local_assignment(&self, pod: Pod, shard_ids: &[i64]) {
        self.local_assignments
            .lock()
            .await
            .insert(pod, shard_ids.iter().copied().map(ShardId::new).collect());
    }

    async fn local_assignment(&self, pod: Pod) -> BTreeSet<ShardId> {
        self.local_assignments
            .lock()
            .await
            .get(&pod)
            .cloned()
            .unwrap_or_default()
    }

    async fn fail_next_assignments(&self, pod: Pod, count: usize) {
        self.failed_assignments.lock().await.insert(pod, count);
    }

    async fn fail_next_revocations(&self, pod: Pod, count: usize) {
        self.failed_revocations.lock().await.insert(pod, count);
    }

    async fn fail_next_reconciliations(&self, pod: Pod, count: usize) {
        self.failed_reconciliations.lock().await.insert(pod, count);
    }

    async fn should_fail(failures: &Arc<Mutex<HashMap<Pod, usize>>>, pod: Pod) -> bool {
        let mut failures = failures.lock().await;
        if let Some(count) = failures.get_mut(&pod)
            && *count > 0
        {
            *count -= 1;
            return true;
        }
        false
    }
}

#[async_trait]
impl WorkerExecutorService for TestWorkerExecutors {
    async fn assign_shards(
        &self,
        pod: &Pod,
        shard_ids: &BTreeSet<ShardId>,
    ) -> Result<(), ShardManagerError> {
        if Self::should_fail(&self.failed_assignments, *pod).await {
            return Err(ShardManagerError::Timeout);
        }

        self.local_assignments
            .lock()
            .await
            .entry(*pod)
            .or_default()
            .extend(shard_ids.iter().copied());
        Ok(())
    }

    async fn health_check(&self, _pod: &Pod) -> Result<(), HealthCheckError> {
        Ok(())
    }

    async fn revoke_shards(
        &self,
        pod: &Pod,
        shard_ids: &BTreeSet<ShardId>,
    ) -> Result<(), ShardManagerError> {
        if Self::should_fail(&self.failed_revocations, *pod).await {
            return Err(ShardManagerError::Timeout);
        }

        if let Some(local_assignment) = self.local_assignments.lock().await.get_mut(pod) {
            local_assignment.retain(|shard_id| !shard_ids.contains(shard_id));
        }
        Ok(())
    }

    async fn set_shard_assignment(
        &self,
        pod: &Pod,
        _number_of_shards: usize,
        shard_ids: &BTreeSet<ShardId>,
    ) -> Result<(), ShardManagerError> {
        if Self::should_fail(&self.failed_reconciliations, *pod).await {
            return Err(ShardManagerError::Timeout);
        }

        self.local_assignments
            .lock()
            .await
            .insert(*pod, shard_ids.clone());
        Ok(())
    }
}

#[derive(Clone, Debug)]
struct TestHealthCheck {
    healthy: Arc<Mutex<HashMap<Pod, bool>>>,
}

impl TestHealthCheck {
    fn all_healthy() -> Self {
        Self {
            healthy: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

#[async_trait]
impl HealthCheck for TestHealthCheck {
    async fn health_check(&self, pod: Pod, _pod_name: Option<String>) -> bool {
        self.healthy.lock().await.get(&pod).copied().unwrap_or(true)
    }
}

fn pod(last_octet: u8, port: u16) -> Pod {
    Pod {
        ip: IpAddr::V4(Ipv4Addr::new(10, 0, 0, last_octet)),
        port,
    }
}

fn routing_table_with_pods(
    number_of_shards: usize,
    pods: Vec<(Pod, &str, &[i64])>,
) -> RoutingTable {
    let mut pod_states = BTreeMap::new();
    for (pod, pod_name, shard_ids) in pods {
        pod_states.insert(
            pod,
            PodState {
                pod_name: Some(pod_name.to_string()),
                assigned_shards: shard_ids.iter().copied().map(ShardId::new).collect(),
            },
        );
    }

    RoutingTable {
        number_of_shards,
        pod_states,
    }
}

fn shard_ids(ids: &[i64]) -> BTreeSet<ShardId> {
    ids.iter().copied().map(ShardId::new).collect()
}

async fn wait_for_local_assignment(
    worker_executors: &TestWorkerExecutors,
    pod: Pod,
    expected: BTreeSet<ShardId>,
) {
    let start = Instant::now();
    loop {
        let actual = worker_executors.local_assignment(pod).await;
        if actual == expected {
            return;
        }

        if start.elapsed() > Duration::from_secs(2) {
            panic!("timed out waiting for local assignment {expected:?}, actual: {actual:?}");
        }

        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

async fn new_shard_management(
    routing_table: RoutingTable,
    worker_executors: Arc<TestWorkerExecutors>,
) -> (
    ShardManagement,
    TestPersistence,
    JoinSet<anyhow::Result<()>>,
) {
    let persistence = TestPersistence::new(routing_table);
    let health_check = Arc::new(TestHealthCheck::all_healthy());
    let mut join_set = JoinSet::new();

    let shard_management = ShardManagement::new(
        Arc::new(persistence.clone()),
        worker_executors,
        health_check,
        0.0,
        &mut join_set,
    )
    .await
    .expect("failed to create shard management");

    tokio::time::sleep(Duration::from_millis(50)).await;

    (shard_management, persistence, join_set)
}

#[test]
// On shard-manager restart, live executors are reset to the routing table.
async fn shard_manager_restart_clears_stale_executor_shards() {
    let authoritative_pod = pod(1, 9000);
    let stale_pod = pod(2, 9001);
    let worker_executors = Arc::new(TestWorkerExecutors::default());
    worker_executors.set_local_assignment(stale_pod, &[0]).await;

    let (_shard_management, _persistence, mut join_set) = new_shard_management(
        routing_table_with_pods(
            1,
            vec![
                (authoritative_pod, "worker-executor-0", &[0]),
                (stale_pod, "worker-executor-1", &[]),
            ],
        ),
        worker_executors.clone(),
    )
    .await;

    assert_eq!(
        worker_executors.local_assignment(authoritative_pod).await,
        [0].into_iter().map(ShardId::new).collect()
    );
    assert_eq!(
        worker_executors.local_assignment(stale_pod).await,
        BTreeSet::new()
    );

    join_set.abort_all();
}

#[test]
// If executor updates happened but were not persisted, restart rolls executors
// back to the persisted routing table.
async fn shard_manager_restart_recovers_from_partially_applied_rebalance() {
    let persisted_owner = pod(1, 9000);
    let stale_new_owner = pod(2, 9001);
    let worker_executors = Arc::new(TestWorkerExecutors::default());
    worker_executors
        .set_local_assignment(stale_new_owner, &[0])
        .await;

    let (_shard_management, persistence, mut join_set) = new_shard_management(
        routing_table_with_pods(
            1,
            vec![
                (persisted_owner, "worker-executor-0", &[0]),
                (stale_new_owner, "worker-executor-1", &[]),
            ],
        ),
        worker_executors.clone(),
    )
    .await;

    let routing_table = persistence.latest().await;
    assert_eq!(
        routing_table
            .pod_states
            .get(&persisted_owner)
            .expect("persisted owner missing")
            .assigned_shards,
        [0].into_iter().map(ShardId::new).collect()
    );
    assert!(
        routing_table
            .pod_states
            .get(&stale_new_owner)
            .expect("stale new owner missing")
            .assigned_shards
            .is_empty()
    );
    assert_eq!(
        worker_executors.local_assignment(persisted_owner).await,
        [0].into_iter().map(ShardId::new).collect()
    );
    assert_eq!(
        worker_executors.local_assignment(stale_new_owner).await,
        BTreeSet::new()
    );

    join_set.abort_all();
}

#[test]
// When a known pod reconnects, stale local shards are cleared.
async fn reconnecting_pod_clears_stale_local_shards() {
    let existing_pod = pod(1, 9000);
    let worker_executors = Arc::new(TestWorkerExecutors::default());
    worker_executors
        .set_local_assignment(existing_pod, &[0])
        .await;

    let (shard_management, persistence, mut join_set) = new_shard_management(
        routing_table_with_pods(
            1,
            vec![
                (existing_pod, "worker-executor-0", &[]),
                (pod(2, 9001), "worker-executor-1", &[0]),
            ],
        ),
        worker_executors.clone(),
    )
    .await;

    worker_executors
        .set_local_assignment(existing_pod, &[0])
        .await;

    assert_eq!(
        worker_executors.local_assignment(existing_pod).await,
        [0].into_iter().map(ShardId::new).collect()
    );

    shard_management
        .register_pod(existing_pod, Some("worker-executor-0".to_string()))
        .await;
    tokio::time::sleep(Duration::from_millis(50)).await;

    assert_eq!(
        worker_executors.local_assignment(existing_pod).await,
        BTreeSet::new()
    );

    let routing_table = persistence.latest().await;
    assert!(
        routing_table
            .pod_states
            .get(&existing_pod)
            .expect("existing pod missing")
            .assigned_shards
            .is_empty()
    );

    join_set.abort_all();
}

#[test]
// If a shard is assigned to one pod, reconciliation removes it from other pods.
async fn reconciliation_clears_duplicate_local_shard_owner() {
    let authoritative_pod = pod(1, 9000);
    let stale_pod = pod(2, 9001);
    let worker_executors = Arc::new(TestWorkerExecutors::default());
    worker_executors.set_local_assignment(stale_pod, &[0]).await;

    let (_shard_management, persistence, mut join_set) = new_shard_management(
        routing_table_with_pods(
            1,
            vec![
                (authoritative_pod, "worker-executor-0", &[0]),
                (stale_pod, "worker-executor-1", &[]),
            ],
        ),
        worker_executors.clone(),
    )
    .await;

    let routing_table = persistence.latest().await;
    assert_eq!(
        routing_table
            .pod_states
            .get(&authoritative_pod)
            .expect("authoritative pod missing")
            .assigned_shards,
        [0].into_iter().map(ShardId::new).collect()
    );
    assert!(
        routing_table
            .pod_states
            .get(&stale_pod)
            .expect("stale pod missing")
            .assigned_shards
            .is_empty()
    );
    assert_eq!(
        worker_executors.local_assignment(authoritative_pod).await,
        [0].into_iter().map(ShardId::new).collect()
    );
    assert_eq!(
        worker_executors.local_assignment(stale_pod).await,
        BTreeSet::new()
    );

    join_set.abort_all();
}

#[test]
// A transient assign failure leaves shards unassigned for one loop, then the
// next loop assigns them from the routing table's unassigned set.
async fn failed_assignment_is_retried_from_unassigned_shards() {
    let old_pod = pod(1, 9000);
    let new_pod = pod(2, 9001);
    let worker_executors = Arc::new(TestWorkerExecutors::default());
    worker_executors
        .set_local_assignment(old_pod, &[0, 1, 2, 3])
        .await;
    worker_executors.fail_next_assignments(new_pod, 1).await;

    let (shard_management, persistence, mut join_set) = new_shard_management(
        routing_table_with_pods(4, vec![(old_pod, "worker-executor-0", &[0, 1, 2, 3])]),
        worker_executors.clone(),
    )
    .await;

    shard_management
        .register_pod(new_pod, Some("worker-executor-1".to_string()))
        .await;

    wait_for_local_assignment(&worker_executors, old_pod, shard_ids(&[2, 3])).await;
    wait_for_local_assignment(&worker_executors, new_pod, shard_ids(&[0, 1])).await;

    let routing_table = persistence.latest().await;
    assert_eq!(
        routing_table
            .pod_states
            .get(&old_pod)
            .expect("old pod missing")
            .assigned_shards,
        shard_ids(&[2, 3])
    );
    assert_eq!(
        routing_table
            .pod_states
            .get(&new_pod)
            .expect("new pod missing")
            .assigned_shards,
        shard_ids(&[0, 1])
    );

    join_set.abort_all();
}

#[test]
// A failed revoke must not assign the shard elsewhere, but it should be retried
// and eventually converge without another shard-manager event.
async fn failed_revoke_is_retried_without_assigning_to_new_executor_first() {
    let old_pod = pod(1, 9000);
    let new_pod = pod(2, 9001);
    let worker_executors = Arc::new(TestWorkerExecutors::default());
    worker_executors
        .set_local_assignment(old_pod, &[0, 1, 2, 3])
        .await;
    worker_executors.fail_next_revocations(old_pod, 1).await;

    let (shard_management, persistence, mut join_set) = new_shard_management(
        routing_table_with_pods(4, vec![(old_pod, "worker-executor-0", &[0, 1, 2, 3])]),
        worker_executors.clone(),
    )
    .await;

    shard_management
        .register_pod(new_pod, Some("worker-executor-1".to_string()))
        .await;

    wait_for_local_assignment(&worker_executors, old_pod, shard_ids(&[2, 3])).await;
    wait_for_local_assignment(&worker_executors, new_pod, shard_ids(&[0, 1])).await;

    assert_eq!(
        worker_executors.local_assignment(old_pod).await,
        shard_ids(&[2, 3])
    );
    assert_eq!(
        worker_executors.local_assignment(new_pod).await,
        shard_ids(&[0, 1])
    );

    let routing_table = persistence.latest().await;
    assert_eq!(
        routing_table
            .pod_states
            .get(&old_pod)
            .expect("old pod missing")
            .assigned_shards,
        shard_ids(&[2, 3])
    );
    assert_eq!(
        routing_table
            .pod_states
            .get(&new_pod)
            .expect("new pod missing")
            .assigned_shards,
        shard_ids(&[0, 1])
    );

    join_set.abort_all();
}

#[test]
// Reconnect reconciliation failures are retried by the shard-manager worker.
async fn failed_reconnect_reconciliation_is_retried() {
    let existing_pod = pod(1, 9000);
    let worker_executors = Arc::new(TestWorkerExecutors::default());

    let (shard_management, _persistence, mut join_set) = new_shard_management(
        routing_table_with_pods(
            1,
            vec![
                (existing_pod, "worker-executor-0", &[]),
                (pod(2, 9001), "worker-executor-1", &[0]),
            ],
        ),
        worker_executors.clone(),
    )
    .await;

    worker_executors
        .set_local_assignment(existing_pod, &[0])
        .await;
    worker_executors
        .fail_next_reconciliations(existing_pod, 1)
        .await;

    shard_management
        .register_pod(existing_pod, Some("worker-executor-0".to_string()))
        .await;

    wait_for_local_assignment(&worker_executors, existing_pod, BTreeSet::new()).await;

    join_set.abort_all();
}
