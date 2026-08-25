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

    async fn writes(&self) -> Vec<RoutingTable> {
        self.writes.lock().await.clone()
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

/// One call the shard manager made to a worker executor, in order.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Call {
    Assign(Pod, BTreeSet<ShardId>),
    Revoke(Pod, BTreeSet<ShardId>),
    Set(Pod, BTreeSet<ShardId>),
}

#[derive(Clone, Debug, Default)]
struct TestWorkerExecutors {
    local_assignments: Arc<Mutex<HashMap<Pod, BTreeSet<ShardId>>>>,
    calls: Arc<Mutex<Vec<Call>>>,
    failed_assignments: Arc<Mutex<HashMap<Pod, usize>>>,
    failed_revocations: Arc<Mutex<HashMap<Pod, usize>>>,
    applied_then_failed_revocations: Arc<Mutex<HashMap<Pod, usize>>>,
    failed_reconciliations: Arc<Mutex<HashMap<Pod, usize>>>,
}

impl TestWorkerExecutors {
    async fn calls(&self) -> Vec<Call> {
        self.calls.lock().await.clone()
    }

    async fn record(&self, call: Call) {
        self.calls.lock().await.push(call);
    }

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

    async fn apply_then_fail_next_revocations(&self, pod: Pod, count: usize) {
        self.applied_then_failed_revocations
            .lock()
            .await
            .insert(pod, count);
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
        self.record(Call::Assign(*pod, shard_ids.clone())).await;
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
        self.record(Call::Revoke(*pod, shard_ids.clone())).await;
        if Self::should_fail(&self.failed_revocations, *pod).await {
            return Err(ShardManagerError::Timeout);
        }

        if let Some(local_assignment) = self.local_assignments.lock().await.get_mut(pod) {
            local_assignment.retain(|shard_id| !shard_ids.contains(shard_id));
        }

        if Self::should_fail(&self.applied_then_failed_revocations, *pod).await {
            return Err(ShardManagerError::Timeout);
        }
        Ok(())
    }

    async fn set_shard_assignment(
        &self,
        pod: &Pod,
        _number_of_shards: usize,
        shard_ids: &BTreeSet<ShardId>,
    ) -> Result<(), ShardManagerError> {
        self.record(Call::Set(*pod, shard_ids.clone())).await;
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

/// Asserts the state every failed-revoke scenario has to reach: the routing table released
/// the shards from the old pod *before* the new pod was recorded as their owner, the old pod
/// was told its authoritative assignment before the new pod was assigned, and the revoke was
/// not retried against a pod the routing table no longer credits with the shards.
async fn assert_failed_revoke_converged(
    worker_executors: &TestWorkerExecutors,
    persistence: &TestPersistence,
    old_pod: Pod,
    new_pod: Pod,
) {
    wait_for_local_assignment(worker_executors, old_pod, shard_ids(&[2, 3])).await;
    wait_for_local_assignment(worker_executors, new_pod, shard_ids(&[0, 1])).await;

    let routing_table = persistence.latest().await;
    assert_eq!(routing_table.get_shards(old_pod), Some(shard_ids(&[2, 3])));
    assert_eq!(routing_table.get_shards(new_pod), Some(shard_ids(&[0, 1])));
    assert!(routing_table.get_unassigned_shards().is_empty());

    let calls = worker_executors.calls().await;
    let revokes = calls
        .iter()
        .filter(|call| matches!(call, Call::Revoke(pod, _) if *pod == old_pod))
        .count();
    assert_eq!(
        revokes, 1,
        "the failed revoke must not be retried against the old pod: {calls:#?}"
    );
    let reconcile_idx = calls
        .iter()
        .position(|call| *call == Call::Set(old_pod, shard_ids(&[2, 3])))
        .unwrap_or_else(|| panic!("old pod never got its authoritative assignment: {calls:#?}"));
    let assign_idx = calls
        .iter()
        .position(|call| *call == Call::Assign(new_pod, shard_ids(&[0, 1])))
        .unwrap_or_else(|| panic!("new pod never got the shards: {calls:#?}"));
    assert!(
        reconcile_idx < assign_idx,
        "old pod must be reconciled before the shards are assigned elsewhere: {calls:#?}"
    );

    let writes = persistence.writes().await;
    let released_idx = writes
        .iter()
        .position(|routing_table| {
            routing_table.get_shards(old_pod) == Some(shard_ids(&[2, 3]))
                && routing_table.get_unassigned_shards() == shard_ids(&[0, 1])
        })
        .expect("shards were never persisted as unassigned after the failed revoke");
    let reassigned_idx = writes
        .iter()
        .position(|routing_table| routing_table.get_shards(new_pod) == Some(shard_ids(&[0, 1])))
        .expect("shards were never persisted as owned by the new pod");
    assert!(
        released_idx < reassigned_idx,
        "shards must be persisted as unassigned before they are persisted as reassigned"
    );
}

#[test]
// The production failure: the executor drops the shards and only the response is lost. The
// routing table must stop crediting the old pod with the shards instead of retrying the same
// revoke forever, so the new pod actually receives them.
async fn revoke_timeout_after_executor_applied_it_does_not_strand_shards() {
    let old_pod = pod(1, 9000);
    let new_pod = pod(2, 9001);
    let worker_executors = Arc::new(TestWorkerExecutors::default());
    worker_executors
        .set_local_assignment(old_pod, &[0, 1, 2, 3])
        .await;
    worker_executors
        .apply_then_fail_next_revocations(old_pod, usize::MAX)
        .await;

    let (shard_management, persistence, mut join_set) = new_shard_management(
        routing_table_with_pods(4, vec![(old_pod, "worker-executor-0", &[0, 1, 2, 3])]),
        worker_executors.clone(),
    )
    .await;

    shard_management
        .register_pod(new_pod, Some("worker-executor-1".to_string()))
        .await;

    assert_failed_revoke_converged(&worker_executors, &persistence, old_pod, new_pod).await;

    join_set.abort_all();
}

#[test]
// The revoke never reached the executor: the shards are still released in the routing table,
// and the old pod is brought in line by its authoritative assignment before the new pod is
// assigned - not by retrying the revoke.
async fn failed_revoke_reconciles_old_executor_before_reassigning() {
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

    assert_failed_revoke_converged(&worker_executors, &persistence, old_pod, new_pod).await;

    join_set.abort_all();
}

#[test]
// The old executor is unreachable: neither the revoke nor its authoritative assignment gets
// through. The shards are still released and handed to the new executor, and the old executor
// keeps being sent its authoritative assignment (until the health check removes it) rather than
// the revoke being retried. Its local state stays stale until then - the accepted trade-off.
async fn unreachable_executor_still_releases_its_revoked_shards() {
    let old_pod = pod(1, 9000);
    let new_pod = pod(2, 9001);
    let worker_executors = Arc::new(TestWorkerExecutors::default());
    worker_executors
        .set_local_assignment(old_pod, &[0, 1, 2, 3])
        .await;
    worker_executors
        .fail_next_revocations(old_pod, usize::MAX)
        .await;
    worker_executors
        .fail_next_reconciliations(old_pod, usize::MAX)
        .await;

    let (shard_management, persistence, mut join_set) = new_shard_management(
        routing_table_with_pods(4, vec![(old_pod, "worker-executor-0", &[0, 1, 2, 3])]),
        worker_executors.clone(),
    )
    .await;

    shard_management
        .register_pod(new_pod, Some("worker-executor-1".to_string()))
        .await;

    wait_for_local_assignment(&worker_executors, new_pod, shard_ids(&[0, 1])).await;
    // The loop keeps retrying the reconciliation without backoff; stop it before inspecting.
    join_set.abort_all();

    assert_eq!(
        worker_executors.local_assignment(old_pod).await,
        shard_ids(&[0, 1, 2, 3]),
        "an unreachable executor cannot be told anything; its local state stays stale"
    );

    let routing_table = persistence.latest().await;
    assert_eq!(routing_table.get_shards(old_pod), Some(shard_ids(&[2, 3])));
    assert_eq!(routing_table.get_shards(new_pod), Some(shard_ids(&[0, 1])));

    let calls = worker_executors.calls().await;
    let revokes = calls
        .iter()
        .filter(|call| matches!(call, Call::Revoke(pod, _) if *pod == old_pod))
        .count();
    assert_eq!(
        revokes, 1,
        "the revoke must not be retried once the routing table released the shards: {calls:#?}"
    );
    let reconciles = calls
        .iter()
        .filter(|call| *call == &Call::Set(old_pod, shard_ids(&[2, 3])))
        .count();
    assert!(
        reconciles >= 2,
        "the authoritative assignment must keep being retried: {calls:#?}"
    );
}
