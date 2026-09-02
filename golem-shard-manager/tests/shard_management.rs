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
use chrono::{DateTime, Utc};
use golem_common::model::{Pod, ShardId};
use golem_shard_manager::{
    ExecutorAddr, ExecutorId, HealthCheck, HealthCheckError, RoutingTablePersistence, ShardEpoch,
    ShardLeaseState, ShardManagement, ShardManagerError, WorkerExecutorService,
};
use std::collections::{BTreeSet, HashMap};
use std::net::{IpAddr, Ipv4Addr};
use std::sync::Arc;
use std::time::Duration;
use test_r::test;
use tokio::sync::Mutex;
use tokio::task::JoinSet;
use tokio::time::Instant;
use uuid::Uuid;

const LEASE_TTL: Duration = Duration::from_secs(60);

#[derive(Clone, Debug)]
struct TestPersistence {
    shard_state: Arc<Mutex<ShardLeaseState>>,
    writes: Arc<Mutex<Vec<ShardLeaseState>>>,
}

impl TestPersistence {
    fn new(initial: ShardLeaseState) -> Self {
        Self {
            shard_state: Arc::new(Mutex::new(initial)),
            writes: Arc::new(Mutex::new(Vec::new())),
        }
    }

    async fn latest(&self) -> ShardLeaseState {
        self.shard_state.lock().await.clone()
    }
}

#[async_trait]
impl RoutingTablePersistence for TestPersistence {
    async fn write(&self, shard_state: &ShardLeaseState) -> Result<(), ShardManagerError> {
        *self.shard_state.lock().await = shard_state.clone();
        self.writes.lock().await.push(shard_state.clone());
        Ok(())
    }

    async fn read(&self) -> Result<ShardLeaseState, ShardManagerError> {
        Ok(self.shard_state.lock().await.clone())
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

fn executor(idx: u128) -> ExecutorId {
    ExecutorId(Uuid::from_u128(idx))
}

fn granted_at() -> DateTime<Utc> {
    DateTime::from_timestamp(1_700_000_000, 0).expect("valid timestamp")
}

fn shard_state_with_executors(
    number_of_shards: usize,
    pods: Vec<(ExecutorId, Pod, &str, &[i64])>,
) -> ShardLeaseState {
    let mut shard_state = ShardLeaseState::new(number_of_shards);
    for (executor_id, pod, pod_name, shard_ids) in pods {
        shard_state.add_executor(
            executor_id,
            ExecutorAddr::from(pod),
            Some(pod_name.to_string()),
            granted_at(),
            LEASE_TTL,
        );
        for shard_id in shard_ids {
            shard_state.assign_shard(executor_id, ShardId::new(*shard_id));
        }
    }
    shard_state
}

/// Shards of the executor registered at `pod` in the persisted state (there is exactly one).
fn shards_at(shard_state: &ShardLeaseState, pod: Pod) -> BTreeSet<ShardId> {
    let executor_id = shard_state
        .executor_for_addr(ExecutorAddr::from(pod))
        .unwrap_or_else(|| panic!("no executor registered at {pod}"));
    shard_state
        .shards_for_executor(executor_id)
        .expect("executor should hold a lease")
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
    shard_state: ShardLeaseState,
    worker_executors: Arc<TestWorkerExecutors>,
) -> (
    ShardManagement,
    TestPersistence,
    JoinSet<anyhow::Result<()>>,
) {
    let persistence = TestPersistence::new(shard_state);
    let health_check = Arc::new(TestHealthCheck::all_healthy());
    let mut join_set = JoinSet::new();

    let shard_management = ShardManagement::new(
        Arc::new(persistence.clone()),
        worker_executors,
        health_check,
        0.0,
        LEASE_TTL,
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
        shard_state_with_executors(
            1,
            vec![
                (executor(1), authoritative_pod, "worker-executor-0", &[0]),
                (executor(2), stale_pod, "worker-executor-1", &[]),
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
        shard_state_with_executors(
            1,
            vec![
                (executor(1), persisted_owner, "worker-executor-0", &[0]),
                (executor(2), stale_new_owner, "worker-executor-1", &[]),
            ],
        ),
        worker_executors.clone(),
    )
    .await;

    let shard_state = persistence.latest().await;
    assert_eq!(
        shards_at(&shard_state, persisted_owner),
        [0].into_iter().map(ShardId::new).collect()
    );
    assert!(shards_at(&shard_state, stale_new_owner).is_empty());
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
        shard_state_with_executors(
            1,
            vec![
                (executor(1), existing_pod, "worker-executor-0", &[]),
                (executor(2), pod(2, 9001), "worker-executor-1", &[0]),
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

    let new_executor_id = shard_management
        .register_executor(existing_pod.into(), Some("worker-executor-0".to_string()))
        .await;
    tokio::time::sleep(Duration::from_millis(50)).await;

    assert_eq!(
        worker_executors.local_assignment(existing_pod).await,
        BTreeSet::new()
    );

    let shard_state = persistence.latest().await;
    assert!(shards_at(&shard_state, existing_pod).is_empty());
    // the re-registered instance replaced the previous lease at the same address
    assert_ne!(new_executor_id, executor(1));
    assert!(!shard_state.has_executor(executor(1)));
    assert_eq!(
        shard_state.executor_for_addr(existing_pod.into()),
        Some(new_executor_id)
    );
    assert_eq!(shard_state.executor_count(), 2);

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
        shard_state_with_executors(
            1,
            vec![
                (executor(1), authoritative_pod, "worker-executor-0", &[0]),
                (executor(2), stale_pod, "worker-executor-1", &[]),
            ],
        ),
        worker_executors.clone(),
    )
    .await;

    let shard_state = persistence.latest().await;
    assert_eq!(
        shards_at(&shard_state, authoritative_pod),
        [0].into_iter().map(ShardId::new).collect()
    );
    assert!(shards_at(&shard_state, stale_pod).is_empty());
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
        shard_state_with_executors(
            4,
            vec![(executor(1), old_pod, "worker-executor-0", &[0, 1, 2, 3])],
        ),
        worker_executors.clone(),
    )
    .await;

    shard_management
        .register_executor(new_pod.into(), Some("worker-executor-1".to_string()))
        .await;

    wait_for_local_assignment(&worker_executors, old_pod, shard_ids(&[2, 3])).await;
    wait_for_local_assignment(&worker_executors, new_pod, shard_ids(&[0, 1])).await;

    let shard_state = persistence.latest().await;
    assert_eq!(shards_at(&shard_state, old_pod), shard_ids(&[2, 3]));
    assert_eq!(shards_at(&shard_state, new_pod), shard_ids(&[0, 1]));

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
        shard_state_with_executors(
            4,
            vec![(executor(1), old_pod, "worker-executor-0", &[0, 1, 2, 3])],
        ),
        worker_executors.clone(),
    )
    .await;

    shard_management
        .register_executor(new_pod.into(), Some("worker-executor-1".to_string()))
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

    let shard_state = persistence.latest().await;
    assert_eq!(shards_at(&shard_state, old_pod), shard_ids(&[2, 3]));
    assert_eq!(shards_at(&shard_state, new_pod), shard_ids(&[0, 1]));

    join_set.abort_all();
}

#[test]
// Reconnect reconciliation failures are retried by the shard-manager worker.
async fn failed_reconnect_reconciliation_is_retried() {
    let existing_pod = pod(1, 9000);
    let worker_executors = Arc::new(TestWorkerExecutors::default());

    let (shard_management, _persistence, mut join_set) = new_shard_management(
        shard_state_with_executors(
            1,
            vec![
                (executor(1), existing_pod, "worker-executor-0", &[]),
                (executor(2), pod(2, 9001), "worker-executor-1", &[0]),
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
        .register_executor(existing_pod.into(), Some("worker-executor-0".to_string()))
        .await;

    wait_for_local_assignment(&worker_executors, existing_pod, BTreeSet::new()).await;

    join_set.abort_all();
}

#[test]
// A new executor instance registering at an address that already holds a lease replaces the
// previous lease, inherits its shards with advanced epochs and receives the authoritative
// assignment; shards never become unassigned and no other executor is disturbed.
async fn same_address_reregistration_transfers_shards_and_reconciles() {
    let restarted_pod = pod(1, 9000);
    let other_pod = pod(2, 9001);
    let worker_executors = Arc::new(TestWorkerExecutors::default());
    worker_executors
        .set_local_assignment(restarted_pod, &[0, 1])
        .await;
    worker_executors
        .set_local_assignment(other_pod, &[2, 3])
        .await;

    let (shard_management, persistence, mut join_set) = new_shard_management(
        shard_state_with_executors(
            4,
            vec![
                (executor(1), restarted_pod, "worker-executor-0", &[0, 1]),
                (executor(2), other_pod, "worker-executor-1", &[2, 3]),
            ],
        ),
        worker_executors.clone(),
    )
    .await;

    // the restarted process comes up with an empty local assignment
    worker_executors
        .set_local_assignment(restarted_pod, &[])
        .await;

    let new_executor_id = shard_management
        .register_executor(restarted_pod.into(), Some("worker-executor-0".to_string()))
        .await;

    wait_for_local_assignment(&worker_executors, restarted_pod, shard_ids(&[0, 1])).await;
    assert_eq!(
        worker_executors.local_assignment(other_pod).await,
        shard_ids(&[2, 3])
    );

    let shard_state = persistence.latest().await;
    assert!(!shard_state.has_executor(executor(1)));
    assert_eq!(
        shard_state.executor_for_addr(restarted_pod.into()),
        Some(new_executor_id)
    );
    assert_eq!(
        shard_state.shards_for_executor(new_executor_id),
        Some(shard_ids(&[0, 1]))
    );
    assert_eq!(
        shard_state.shards_for_executor(executor(2)),
        Some(shard_ids(&[2, 3]))
    );
    assert_eq!(
        shard_state.epoch_for_shard(ShardId::new(0)),
        Some(ShardEpoch(1))
    );
    assert_eq!(
        shard_state.epoch_for_shard(ShardId::new(2)),
        Some(ShardEpoch(0))
    );
    assert!(shard_state.pending_rebalance.is_empty());
    assert!(shard_state.get_unassigned_shards().is_empty());

    // every persisted state along the way kept all four shards routable
    for written in persistence.writes.lock().await.iter() {
        assert!(
            written.get_unassigned_shards().is_empty(),
            "shards became unassigned during re-registration: {written}"
        );
    }

    join_set.abort_all();
}
