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
    ExecutorAddr, ExecutorId, ExternalRevision, HealthCheck, HealthCheckError, NO_REVISION,
    RoutingTablePersistence, ShardEpoch, ShardLeaseState, ShardManagement, ShardManagerError,
    WorkerExecutorService,
};
use std::collections::{BTreeSet, HashMap, VecDeque};
use std::net::{IpAddr, Ipv4Addr};
use std::sync::Arc;
use std::time::Duration;
use test_r::test;
use tokio::sync::{Mutex, oneshot};
use tokio::task::JoinSet;
use tokio::time::Instant;
use uuid::Uuid;

const LEASE_TTL: Duration = Duration::from_secs(60);

#[derive(Debug)]
struct TestStore {
    shard_state: ShardLeaseState,
    revision: ExternalRevision,
}

/// An in-memory [`RoutingTablePersistence`] with the same compare-and-swap semantics as the real
/// backends, plus a hook for failing writes.
#[derive(Clone, Debug)]
struct TestPersistence {
    store: Arc<Mutex<TestStore>>,
    /// `(prev_revision, written_state)` for every accepted write.
    writes: Arc<Mutex<Vec<(ExternalRevision, ShardLeaseState)>>>,
    /// Every `write` call, accepted or not. The gap between this and `writes` is how a test sees
    /// that a write was rejected.
    attempts: Arc<Mutex<usize>>,
    /// Per-write script: a `None` entry lets a write through, a `Some(err)` entry fails it before
    /// it reaches the store. Writes past the end of the script always succeed.
    injected: Arc<Mutex<VecDeque<Option<ShardManagerError>>>>,
    gate: Arc<Mutex<Option<WriteGate>>>,
}

/// Suspends a single write: `entered` fires when it is reached, then it waits for `release`.
#[derive(Debug)]
struct WriteGate {
    entered: oneshot::Sender<()>,
    release: oneshot::Receiver<()>,
}

impl TestPersistence {
    /// Seeds a store that already holds state. Revision 1 rather than `NO_REVISION`, because the
    /// real backends can never hold state at revision 0 - that value means "nothing stored".
    fn new(initial: ShardLeaseState) -> Self {
        Self::at_revision(initial, 1)
    }

    fn at_revision(initial: ShardLeaseState, revision: ExternalRevision) -> Self {
        Self {
            store: Arc::new(Mutex::new(TestStore {
                shard_state: initial,
                revision,
            })),
            writes: Arc::new(Mutex::new(Vec::new())),
            attempts: Arc::new(Mutex::new(0)),
            injected: Arc::new(Mutex::new(VecDeque::new())),
            gate: Arc::new(Mutex::new(None)),
        }
    }

    async fn latest(&self) -> ShardLeaseState {
        self.store.lock().await.shard_state.clone()
    }

    /// Scripts the outcome of the next writes. `vec![None, Some(err)]` fails only the second one.
    async fn fail_writes(&self, script: Vec<Option<ShardManagerError>>) {
        *self.injected.lock().await = script.into();
    }

    /// Suspends the next write. Returns a receiver that resolves once that write is in flight,
    /// and the sender that lets it finish.
    async fn block_next_write(&self) -> (oneshot::Receiver<()>, oneshot::Sender<()>) {
        let (entered_tx, entered_rx) = oneshot::channel();
        let (release_tx, release_rx) = oneshot::channel();
        *self.gate.lock().await = Some(WriteGate {
            entered: entered_tx,
            release: release_rx,
        });
        (entered_rx, release_tx)
    }

    async fn write_count(&self) -> usize {
        self.writes.lock().await.len()
    }

    async fn attempt_count(&self) -> usize {
        *self.attempts.lock().await
    }
}

#[async_trait]
impl RoutingTablePersistence for TestPersistence {
    async fn write(
        &self,
        shard_state: &ShardLeaseState,
        prev_revision: ExternalRevision,
    ) -> Result<ExternalRevision, ShardManagerError> {
        *self.attempts.lock().await += 1;
        if let Some(err) = self.injected.lock().await.pop_front().flatten() {
            return Err(err);
        }

        // Taken out of the mutex before awaiting, so the gate never holds a lock.
        let gate = self.gate.lock().await.take();
        if let Some(gate) = gate {
            let _ = gate.entered.send(());
            let _ = gate.release.await;
        }

        let mut store = self.store.lock().await;
        if prev_revision != store.revision {
            return Err(ShardManagerError::ConcurrentModification);
        }
        store.revision += 1;
        store.shard_state = shard_state.clone();
        self.writes
            .lock()
            .await
            .push((prev_revision, shard_state.clone()));
        Ok(store.revision)
    }

    async fn read(&self) -> Result<(ShardLeaseState, ExternalRevision), ShardManagerError> {
        let store = self.store.lock().await;
        Ok((store.shard_state.clone(), store.revision))
    }
}

#[derive(Clone, Debug, Default)]
struct TestWorkerExecutors {
    local_assignments: Arc<Mutex<HashMap<Pod, BTreeSet<ShardId>>>>,
    failed_assignments: Arc<Mutex<HashMap<Pod, usize>>>,
    failed_revocations: Arc<Mutex<HashMap<Pod, usize>>>,
    failed_reconciliations: Arc<Mutex<HashMap<Pod, usize>>>,
    /// Every command sent to an executor, in order, whether or not it succeeded.
    ///
    /// `local_assignments` records only the net effect, so no-op commands - exactly what a leader
    /// that has lost the fence would still issue - leave no trace there.
    commands: Arc<Mutex<Vec<String>>>,
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

    async fn record(&self, command: &str, pod: Pod, shard_ids: &BTreeSet<ShardId>) {
        self.commands
            .lock()
            .await
            .push(format!("{command} {pod} {shard_ids:?}"));
    }

    async fn commands_sent(&self) -> Vec<String> {
        self.commands.lock().await.clone()
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
        self.record("assign", *pod, shard_ids).await;
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
        self.record("revoke", *pod, shard_ids).await;
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
        self.record("set-assignment", *pod, shard_ids).await;
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
    /// Pod whose check never answers, standing in for an executor that went silent.
    never_answers: Option<Pod>,
}

impl TestHealthCheck {
    fn all_healthy() -> Self {
        Self {
            healthy: Arc::new(Mutex::new(HashMap::new())),
            never_answers: None,
        }
    }

    fn never_answering_at(pod: Pod) -> Self {
        Self {
            never_answers: Some(pod),
            ..Self::all_healthy()
        }
    }
}

#[async_trait]
impl HealthCheck for TestHealthCheck {
    async fn health_check(&self, pod: Pod, _pod_name: Option<String>) -> bool {
        if self.never_answers == Some(pod) {
            std::future::pending::<()>().await;
        }
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
    // The startup pass persists exactly twice (executor changes, then the applied rebalance).
    // Wait for both before handing the fixture over, so a test that scripts write failures cannot
    // have one of them swallowed by a startup write that had not landed yet.
    wait_for_writes(&persistence, 2).await;

    (shard_management, persistence, join_set)
}

/// Waits until the loop has performed at least `count` accepted writes.
async fn wait_for_writes(persistence: &TestPersistence, count: usize) {
    let start = Instant::now();
    while persistence.write_count().await < count {
        if start.elapsed() > Duration::from_secs(5) {
            panic!(
                "timed out waiting for {count} writes, saw {}",
                persistence.write_count().await
            );
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
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
    for (_, written) in persistence.writes.lock().await.iter() {
        assert!(
            written.get_unassigned_shards().is_empty(),
            "shards became unassigned during re-registration: {written}"
        );
    }

    join_set.abort_all();
}

#[test]
// The loop ends with the error rather than carry on against a store it can no longer trust, so
// the process restarts and re-reads. On a conflict the cached revision is deliberately not
// refreshed: it is the fencing token, and the loser of it has to stop.
async fn a_persistence_failure_leaves_the_state_untouched_and_stops_the_loop() {
    let existing_pod = pod(1, 9000);
    let new_pod = pod(2, 9001);
    let worker_executors = Arc::new(TestWorkerExecutors::default());

    let (shard_management, persistence, mut join_set) = new_shard_management(
        shard_state_with_executors(
            4,
            vec![(
                executor(1),
                existing_pod,
                "worker-executor-0",
                &[0, 1, 2, 3],
            )],
        ),
        worker_executors.clone(),
    )
    .await;
    let before = shard_management.current_snapshot().await;
    let persisted_before = persistence.latest().await;

    persistence
        .fail_writes(vec![Some(ShardManagerError::ConcurrentModification)])
        .await;
    let new_executor = shard_management
        .register_executor(
            ExecutorAddr::from(new_pod),
            Some("worker-executor-1".into()),
        )
        .await;

    let outcome = tokio::time::timeout(Duration::from_secs(5), join_set.join_next())
        .await
        .expect("the shard management loop should have stopped")
        .expect("the loop task should exist")
        .expect("the loop task should not panic");
    assert!(
        outcome.is_err(),
        "the loop must end with the persistence error, got {outcome:?}"
    );

    let after = shard_management.current_snapshot().await;
    assert!(!after.has_executor(new_executor));
    assert_eq!(after, before);
    assert_eq!(persistence.latest().await, persisted_before);
    assert!(worker_executors.local_assignment(new_pod).await.is_empty());
}

#[test]
// The second persist of a pass runs *after* the rebalance reached the executors over gRPC, so
// rolling it back leaves them holding shards the routing table no longer records - and a rebalance
// planned from the rolled-back state then sees a balanced table and plans nothing, forever. Ending
// the task is what saves it: the process restarts and re-sends every authoritative assignment.
async fn a_persistence_failure_after_the_rebalance_was_executed_stops_the_loop() {
    let existing_pod = pod(1, 9000);
    let new_pod = pod(2, 9001);
    let worker_executors = Arc::new(TestWorkerExecutors::default());

    let (shard_management, persistence, mut join_set) = new_shard_management(
        shard_state_with_executors(
            4,
            vec![(
                executor(1),
                existing_pod,
                "worker-executor-0",
                &[0, 1, 2, 3],
            )],
        ),
        worker_executors.clone(),
    )
    .await;

    // Let the registration persist, then fail the persist of the applied rebalance.
    persistence
        .fail_writes(vec![None, Some(ShardManagerError::ConcurrentModification)])
        .await;
    let new_executor = shard_management
        .register_executor(
            ExecutorAddr::from(new_pod),
            Some("worker-executor-1".into()),
        )
        .await;

    let outcome = tokio::time::timeout(Duration::from_secs(5), join_set.join_next())
        .await
        .expect("the shard management loop should have stopped")
        .expect("the loop task should exist")
        .expect("the loop task should not panic");
    assert!(
        outcome.is_err(),
        "the loop must end with the persistence error, got {outcome:?}"
    );

    // The first persist landed, so the registration survived both in memory and in the store.
    let after = shard_management.current_snapshot().await;
    assert!(
        after.has_executor(new_executor),
        "the registration was persisted by the first write and must not be rolled back"
    );
    assert_eq!(
        after,
        persistence.latest().await,
        "the in-memory state must match the last state that was actually stored"
    );

    // ... but the rebalance it triggered was rolled back after the executor was told about it.
    // That divergence is why the loop must stop rather than carry on.
    assert!(
        shards_at(&after, new_pod).is_empty(),
        "the rolled-back rebalance must not appear in the routing table"
    );
    assert!(
        !worker_executors.local_assignment(new_pod).await.is_empty(),
        "the rebalance should have reached the executor before the persist failed - without that, \
         this test is not exercising the second persist site"
    );
}

#[test]
// The write lock is held across the persistence round-trip, so nobody can read a state that is
// still being stored and that a failed write is about to roll back. Releasing it early would let
// `GetRoutingTable` hand out a routing table that never reached the store, then go backwards.
async fn readers_cannot_observe_a_state_that_is_still_being_persisted() {
    let existing_pod = pod(1, 9000);
    let new_pod = pod(2, 9001);
    let worker_executors = Arc::new(TestWorkerExecutors::default());

    let (shard_management, persistence, _join_set) = new_shard_management(
        shard_state_with_executors(
            4,
            vec![(
                executor(1),
                existing_pod,
                "worker-executor-0",
                &[0, 1, 2, 3],
            )],
        ),
        worker_executors.clone(),
    )
    .await;

    let (entered, release) = persistence.block_next_write().await;

    let _ = shard_management
        .register_executor(
            ExecutorAddr::from(new_pod),
            Some("worker-executor-1".into()),
        )
        .await;

    // Signalled from inside `write`, which the loop calls while holding the guard.
    tokio::time::timeout(Duration::from_secs(5), entered)
        .await
        .expect("the loop should have reached a write")
        .expect("the gate should have been signalled");

    let blocked = tokio::time::timeout(
        Duration::from_millis(200),
        shard_management.current_snapshot(),
    )
    .await;
    assert!(
        blocked.is_err(),
        "current_snapshot() must not be able to read a state that is still being persisted"
    );

    release.send(()).expect("the write should still be waiting");

    wait_for_writes(&persistence, 4).await;
    let after = tokio::time::timeout(Duration::from_secs(5), shard_management.current_snapshot())
        .await
        .expect("the read lock must be free once the persist completed");
    assert_eq!(after, persistence.latest().await);
}

#[test]
// A caller dropped mid-persist - an aborted task, a handler whose client disconnected - releases
// the write lock through the guard's `Drop` and never runs the code after the await. Mutating a
// clone is what keeps that harmless: there is nothing left to undo by code that will not run.
async fn a_persist_interrupted_mid_flight_leaves_the_state_untouched() {
    let existing_pod = pod(1, 9000);
    let new_pod = pod(2, 9001);
    let worker_executors = Arc::new(TestWorkerExecutors::default());

    let (shard_management, persistence, mut join_set) = new_shard_management(
        shard_state_with_executors(
            4,
            vec![(
                executor(1),
                existing_pod,
                "worker-executor-0",
                &[0, 1, 2, 3],
            )],
        ),
        worker_executors.clone(),
    )
    .await;
    let before = shard_management.current_snapshot().await;
    let persisted_before = persistence.latest().await;

    // `_release` is held to the end of the test: dropping it would let the write finish instead of
    // leaving it suspended for the abort to interrupt.
    let (entered, _release) = persistence.block_next_write().await;

    let new_executor = shard_management
        .register_executor(
            ExecutorAddr::from(new_pod),
            Some("worker-executor-1".into()),
        )
        .await;

    tokio::time::timeout(Duration::from_secs(5), entered)
        .await
        .expect("the loop should have reached a write")
        .expect("the gate should have been signalled");

    // Draining is what makes the abort observable: the loop future - and with it the write guard -
    // is dropped only once the task is reaped.
    join_set.abort_all();
    while join_set.join_next().await.is_some() {}

    let after = tokio::time::timeout(Duration::from_secs(5), shard_management.current_snapshot())
        .await
        .expect("the aborted pass must have released the write lock");
    assert_eq!(
        after, before,
        "the interrupted pass left its mutation in the live state. Every reader now serves a \
         registration that was never persisted, and the cached external revision still refers to \
         the state before it, so every later compare-and-swap fails."
    );
    assert!(!after.has_executor(new_executor));
    assert_eq!(persistence.latest().await, persisted_before);
}

#[test]
// First boot through the loop: nothing is stored, so the very first write must be guarded on
// NO_REVISION. Getting this wrong is rejected by both real backends, not silently tolerated.
async fn the_first_write_on_an_empty_store_uses_no_revision() {
    let worker_executors = Arc::new(TestWorkerExecutors::default());
    let persistence = TestPersistence::at_revision(ShardLeaseState::new(4), NO_REVISION);
    let health_check = Arc::new(TestHealthCheck::all_healthy());
    let mut join_set = JoinSet::new();

    let _shard_management = ShardManagement::new(
        Arc::new(persistence.clone()),
        worker_executors,
        health_check,
        0.0,
        LEASE_TTL,
        &mut join_set,
    )
    .await
    .expect("failed to create shard management");

    wait_for_writes(&persistence, 1).await;

    let writes = persistence.writes.lock().await;
    let (first_prev_revision, _) = writes.first().expect("the loop should have written once");
    assert_eq!(*first_prev_revision, NO_REVISION);
    drop(writes);

    join_set.abort_all();
}

#[test]
// The loop must write with the revision it read at startup, not with a hardcoded one.
async fn the_first_write_uses_the_revision_read_at_startup() {
    let existing_pod = pod(1, 9000);
    let worker_executors = Arc::new(TestWorkerExecutors::default());
    let shard_state = shard_state_with_executors(
        1,
        vec![(executor(1), existing_pod, "worker-executor-0", &[0])],
    );

    let persistence = TestPersistence::at_revision(shard_state, 7);
    let health_check = Arc::new(TestHealthCheck::all_healthy());
    let mut join_set = JoinSet::new();

    let _shard_management = ShardManagement::new(
        Arc::new(persistence.clone()),
        worker_executors,
        health_check,
        0.0,
        LEASE_TTL,
        &mut join_set,
    )
    .await
    .expect("failed to create shard management");

    wait_for_writes(&persistence, 2).await;

    let prev_revisions: Vec<ExternalRevision> = persistence
        .writes
        .lock()
        .await
        .iter()
        .map(|(prev, _)| *prev)
        .collect();

    // The first write is guarded on the revision `read()` returned at startup, the second on what
    // the first write returned.
    assert_eq!(prev_revisions, vec![7, 8]);

    // ...and both were accepted first time. This pins the cached revision being advanced on
    // success: without it the second write of the startup pass would conflict and stop the loop.
    assert_eq!(
        persistence.attempt_count().await,
        2,
        "a conflict-free startup pass must not need a retry"
    );

    join_set.abort_all();
}

#[test]
// The failure arm is not specific to a revision conflict. A backend failure that says nothing
// about revisions leaves the store in an unknown state - the write may or may not have landed -
// so it fail-stops like a conflict, surfacing the backend's own error rather than a generic one.
async fn a_non_conflict_persistence_error_stops_the_loop_the_same_way() {
    let existing_pod = pod(1, 9000);
    let new_pod = pod(2, 9001);
    let worker_executors = Arc::new(TestWorkerExecutors::default());

    let (shard_management, persistence, mut join_set) = new_shard_management(
        shard_state_with_executors(
            4,
            vec![(
                executor(1),
                existing_pod,
                "worker-executor-0",
                &[0, 1, 2, 3],
            )],
        ),
        worker_executors.clone(),
    )
    .await;
    let before = shard_management.current_snapshot().await;
    let persisted_before = persistence.latest().await;

    persistence
        .fail_writes(vec![Some(ShardManagerError::Internal(
            "injected backend failure".into(),
        ))])
        .await;
    let new_executor = shard_management
        .register_executor(
            ExecutorAddr::from(new_pod),
            Some("worker-executor-1".into()),
        )
        .await;

    let outcome = tokio::time::timeout(Duration::from_secs(5), join_set.join_next())
        .await
        .expect("the shard management loop should have stopped")
        .expect("the loop task should exist")
        .expect("the loop task should not panic");
    let err = outcome.expect_err("the loop must end with the persistence error");
    assert!(
        err.to_string().contains("injected backend failure"),
        "the loop should surface the backend's own error, got: {err:#}"
    );

    let after = shard_management.current_snapshot().await;
    assert!(!after.has_executor(new_executor));
    assert_eq!(after, before);
    assert_eq!(persistence.latest().await, persisted_before);
    assert!(worker_executors.local_assignment(new_pod).await.is_empty());
}

#[test]
// Without the bound, a leader elected while an executor is silent never reaches its gRPC bind,
// while its leader gauge already reads 1.
async fn the_initial_health_check_cannot_pin_startup() {
    let silent_pod = pod(1, 9000);
    let shard_state = shard_state_with_executors(
        4,
        vec![(executor(1), silent_pod, "worker-executor-1", &[0, 1, 2, 3])],
    );
    let worker_executors = Arc::new(TestWorkerExecutors::default());
    let persistence = TestPersistence::new(shard_state);
    let health_check = Arc::new(TestHealthCheck::never_answering_at(silent_pod));
    let mut join_set = JoinSet::new();

    let started = tokio::time::timeout(
        Duration::from_secs(5),
        ShardManagement::new_with_initial_health_check_timeout(
            Arc::new(persistence.clone()),
            worker_executors,
            health_check,
            0.0,
            LEASE_TTL,
            &mut join_set,
            Duration::from_secs(1),
        ),
    )
    .await
    .expect("startup was pinned by the health check that never answers");
    started.expect("failed to create shard management");

    join_set.abort_all();
}

#[test]
// A refused write is a demoted leader's only notice of demotion, so it has to stop it before any
// command reaches an executor: the new leader is already planning from the state that write was
// refused against, so shards handed out after it are ones it believes it has placed elsewhere.
async fn a_demoted_leaders_fenced_write_fails_before_any_executor_command() {
    let existing_pod = pod(1, 9000);
    let new_pod = pod(2, 9001);
    let worker_executors = Arc::new(TestWorkerExecutors::default());

    let (shard_management, persistence, mut join_set) = new_shard_management(
        shard_state_with_executors(
            4,
            vec![(
                executor(1),
                existing_pod,
                "worker-executor-0",
                &[0, 1, 2, 3],
            )],
        ),
        worker_executors.clone(),
    )
    .await;

    // The startup pass has already commanded the executor it found; only what follows this mark
    // belongs to the demoted pass.
    let before = worker_executors.commands_sent().await;

    persistence
        .fail_writes(vec![Some(ShardManagerError::LeadershipLost {
            leader_key: "/golem/shard-manager/leader/6c2f".to_string(),
            create_revision: 41,
        })])
        .await;
    shard_management
        .register_executor(
            ExecutorAddr::from(new_pod),
            Some("worker-executor-1".into()),
        )
        .await;

    let outcome = tokio::time::timeout(Duration::from_secs(5), join_set.join_next())
        .await
        .expect(
            "the shard management loop is still running after a write was refused for a lost \
             fence, so it is about to command executors on a topology it no longer owns",
        )
        .expect("the loop task should exist")
        .expect("the loop task should not panic");

    let err = outcome.expect_err("a lost fence must end the loop");
    assert!(
        matches!(
            err.downcast_ref::<ShardManagerError>(),
            Some(ShardManagerError::LeadershipLost { .. })
        ),
        "the loop ended, but not with the lost fence that ended it: {err:#}"
    );

    // The ordering under test: persist before apply, so an unpersisted plan is never applied.
    let after = worker_executors.commands_sent().await;
    assert_eq!(
        after,
        before,
        "a leader whose fenced write was refused still sent {:?} to executors. It has already \
         been replaced, so those shards are ones the new leader believes it has placed elsewhere.",
        &after[before.len()..]
    );
}

#[test]
// A known gap, asserted rather than fixed: `register_executor` acknowledges as soon as the
// registration is queued, so an executor can be told it is registered by a leader whose next write
// is refused - left holding no shards, unrecorded, and waiting on a routing table it is not in.
// Fixing it means acknowledging after the persist; until then this test pins the broken shape.
async fn a_registration_acknowledged_before_a_failed_persist_orphans_the_executor() {
    let existing_pod = pod(1, 9000);
    let new_pod = pod(2, 9001);
    let worker_executors = Arc::new(TestWorkerExecutors::default());

    let (shard_management, persistence, mut join_set) = new_shard_management(
        shard_state_with_executors(
            4,
            vec![(
                executor(1),
                existing_pod,
                "worker-executor-0",
                &[0, 1, 2, 3],
            )],
        ),
        worker_executors.clone(),
    )
    .await;

    persistence
        .fail_writes(vec![Some(ShardManagerError::LeadershipLost {
            leader_key: "/golem/shard-manager/leader/6c2f".to_string(),
            create_revision: 41,
        })])
        .await;

    let new_executor = shard_management
        .register_executor(
            ExecutorAddr::from(new_pod),
            Some("worker-executor-1".into()),
        )
        .await;

    let outcome = tokio::time::timeout(Duration::from_secs(5), join_set.join_next())
        .await
        .expect("the shard management loop should have stopped")
        .expect("the loop task should exist")
        .expect("the loop task should not panic");
    assert!(
        outcome.is_err(),
        "the loop must end with the persistence error, got {outcome:?}"
    );

    assert!(
        !persistence.latest().await.has_executor(new_executor),
        "the persisted state now holds the executor whose registration was refused. That is the \
         fix, not the gap - if `Register` has started acknowledging after the persist, this test \
         has to say so rather than keep asserting the old shape."
    );
    assert!(
        worker_executors.local_assignment(new_pod).await.is_empty(),
        "the orphaned executor was given shards, so it is not orphaned - it is a second owner of \
         shards the routing table does not record."
    );
}
