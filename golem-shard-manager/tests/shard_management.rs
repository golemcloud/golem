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
    RoutingTablePersistence, ShardAssignmentPush, ShardEpoch, ShardLeaseState, ShardManagement,
    ShardManagerError, WorkerExecutorService,
};
use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::net::{IpAddr, Ipv4Addr};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
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
    /// Every full-replace push, in order, with the epochs and expiry it carried. `commands`
    /// records only the shard ids, so this is where a test looks at what else travelled.
    pushes: Arc<Mutex<Vec<(Pod, ShardAssignmentPush)>>>,
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

    async fn pushes_to(&self, pod: Pod) -> Vec<ShardAssignmentPush> {
        self.pushes
            .lock()
            .await
            .iter()
            .filter(|(pushed_to, _)| *pushed_to == pod)
            .map(|(_, push)| push.clone())
            .collect()
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
        assignment: &ShardAssignmentPush,
    ) -> Result<(), ShardManagerError> {
        let shard_ids: BTreeSet<ShardId> = assignment.shard_epochs.keys().copied().collect();
        self.record("assign", *pod, &shard_ids).await;
        self.pushes.lock().await.push((*pod, assignment.clone()));
        if Self::should_fail(&self.failed_assignments, *pod).await {
            return Err(ShardManagerError::Timeout);
        }

        // A full replace: the executor holds exactly what the push names and drops the rest.
        // Extending instead of inserting is what this stops.
        self.local_assignments.lock().await.insert(*pod, shard_ids);
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
}

#[derive(Clone, Debug)]
struct TestHealthCheck {
    healthy: Arc<Mutex<HashMap<Pod, bool>>>,
    /// Pod whose check never answers, standing in for an executor that went silent.
    never_answers: Option<Pod>,
    /// Every probe made, whatever it answered: a replica that must not contact executors yet is
    /// caught by this, not by what the probes returned.
    probes: Arc<AtomicUsize>,
}

impl TestHealthCheck {
    fn all_healthy() -> Self {
        Self {
            healthy: Arc::new(Mutex::new(HashMap::new())),
            never_answers: None,
            probes: Arc::new(AtomicUsize::new(0)),
        }
    }

    fn probe_count(&self) -> usize {
        self.probes.load(Ordering::SeqCst)
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
        self.probes.fetch_add(1, Ordering::SeqCst);
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

/// The claim an executor holding exactly what `shard_state` records for it would send.
fn claim_of(
    shard_state: &ShardLeaseState,
    executor_id: ExecutorId,
) -> BTreeMap<ShardId, ShardEpoch> {
    shard_state
        .shard_assignments
        .iter()
        .filter(|(_, entry)| entry.executor_id == executor_id)
        .map(|(shard_id, entry)| (*shard_id, entry.epoch))
        .collect()
}

fn expiry_of(shard_state: &ShardLeaseState, executor_id: ExecutorId) -> DateTime<Utc> {
    shard_state.executor_leases[&executor_id].expires_at
}

/// Two executors sharing four shards evenly - a balanced cluster, so nothing is planned and the
/// only writes a test sees are the ones it causes.
fn balanced_pair() -> ShardLeaseState {
    shard_state_with_executors(
        4,
        vec![
            (executor(1), pod(1, 9000), "worker-executor-0", &[0, 1]),
            (executor(2), pod(2, 9001), "worker-executor-1", &[2, 3]),
        ],
    )
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
    let (shard_management, persistence, join_set) =
        start_shard_management(shard_state, worker_executors, LEASE_TTL).await;
    // With the tick derived from a 60s lease there is nothing periodic to confuse this: once the
    // startup pass has stopped writing it is over, and a test may script the next write.
    wait_for_quiescence(&persistence).await;
    (shard_management, persistence, join_set)
}

/// [`new_shard_management`] with the lease duration - and therefore the loop's tick period, which
/// is a third of it - under the test's control.
async fn start_shard_management(
    shard_state: ShardLeaseState,
    worker_executors: Arc<TestWorkerExecutors>,
    lease_ttl: Duration,
) -> (
    ShardManagement,
    TestPersistence,
    JoinSet<anyhow::Result<()>>,
) {
    let executor_count = shard_state.executor_count();
    let number_of_shards = shard_state.number_of_shards;
    let persistence = TestPersistence::new(shard_state);
    let health_check = Arc::new(TestHealthCheck::all_healthy());
    let mut join_set = JoinSet::new();

    let shard_management = ShardManagement::new(
        Arc::new(persistence.clone()),
        worker_executors.clone(),
        health_check,
        0.0,
        lease_ttl,
        number_of_shards,
        &mut join_set,
    )
    .await
    .expect("failed to create shard management");

    // A write count is no longer a barrier: a balanced startup pass re-grants the leases it found
    // healthy in one write and then, thanks to the no-op guard, writes nothing at all. What always
    // happens is the authoritative push to every healthy executor, and it is the last thing the
    // pass does - so waiting for one push per executor is waiting for the pass to finish.
    wait_for_pushes(&worker_executors, executor_count).await;

    (shard_management, persistence, join_set)
}

/// Waits until at least `count` full-replace pushes have been sent, to any executor.
async fn wait_for_pushes(worker_executors: &TestWorkerExecutors, count: usize) {
    let start = Instant::now();
    loop {
        let sent = worker_executors.pushes.lock().await.len();
        if sent >= count {
            return;
        }
        if start.elapsed() > Duration::from_secs(5) {
            panic!("timed out waiting for {count} pushes, saw {sent}");
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// Waits until the loop has stopped writing.
async fn wait_for_quiescence(persistence: &TestPersistence) {
    let start = Instant::now();
    let mut last = persistence.write_count().await;
    loop {
        tokio::time::sleep(Duration::from_millis(50)).await;
        let now = persistence.write_count().await;
        if now == last {
            return;
        }
        last = now;
        if start.elapsed() > Duration::from_secs(5) {
            panic!("the shard management loop never stopped writing");
        }
    }
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

    let new_executor_id = executor(3);
    shard_management
        .register_executor(
            new_executor_id,
            existing_pod.into(),
            Some("worker-executor-0".to_string()),
        )
        .await
        .expect("the registration should have been persisted");
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
        .register_executor(
            executor(2),
            new_pod.into(),
            Some("worker-executor-1".to_string()),
        )
        .await
        .expect("the registration should have been persisted");

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
        .register_executor(
            executor(2),
            new_pod.into(),
            Some("worker-executor-1".to_string()),
        )
        .await
        .expect("the registration should have been persisted");

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
    // The full-replace push *is* the reconciliation now, so a failed assign to this pod is what
    // the retry path has to recover from.
    worker_executors
        .fail_next_assignments(existing_pod, 1)
        .await;

    shard_management
        .register_executor(
            executor(3),
            existing_pod.into(),
            Some("worker-executor-0".to_string()),
        )
        .await
        .expect("the registration should have been persisted");

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

    let new_executor_id = executor(3);
    let ack = shard_management
        .register_executor(
            new_executor_id,
            restarted_pod.into(),
            Some("worker-executor-0".to_string()),
        )
        .await
        .expect("the registration should have been persisted");

    // The acknowledgement is the lease itself: the inherited shards with their advanced epochs,
    // and a real expiry.
    assert_eq!(ack.number_of_shards, 4);
    assert_eq!(
        ack.grant.shard_epochs,
        [
            (ShardId::new(0), ShardEpoch(1)),
            (ShardId::new(1), ShardEpoch(1)),
        ]
        .into_iter()
        .collect()
    );
    assert!(ack.grant.expires_at > granted_at());

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

    // The push that reconciled the restarted instance carried the epochs it is now fenced on,
    // and the cluster shard count its routing needs - not just the shard ids.
    let last_push = worker_executors
        .pushes_to(restarted_pod)
        .await
        .pop()
        .expect("the restarted executor should have been pushed its full set");
    assert_eq!(
        last_push.shard_epochs,
        [
            (ShardId::new(0), ShardEpoch(1)),
            (ShardId::new(1), ShardEpoch(1)),
        ]
        .into_iter()
        .collect()
    );
    assert_eq!(last_push.number_of_shards, 4);
    assert!(last_push.expires_at > granted_at());

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
// The client retries `Register` on every error, so a lost response re-sends the same executor_id.
// That retry has to refresh the lease it already granted: creating a second one, or treating the
// executor as a replacement of itself, would advance the epoch of every shard it never stopped
// owning and fence it out of them.
async fn a_repeated_registration_of_the_same_executor_refreshes_its_lease() {
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

    let new_executor = executor(2);
    let first = shard_management
        .register_executor(
            new_executor,
            ExecutorAddr::from(new_pod),
            Some("worker-executor-1".into()),
        )
        .await
        .expect("the registration should have been persisted");
    // Nothing is assigned yet: `AssignShards` delivers the initial set.
    assert!(first.grant.shard_epochs.is_empty());

    wait_for_local_assignment(&worker_executors, new_pod, shard_ids(&[0, 1])).await;

    let retried = shard_management
        .register_executor(
            new_executor,
            ExecutorAddr::from(new_pod),
            Some("worker-executor-1".into()),
        )
        .await
        .expect("the retried registration should have been persisted");

    // The same shards, at the same epochs, and a lease that was extended rather than replaced.
    assert_eq!(
        retried.grant.shard_epochs,
        [
            (ShardId::new(0), ShardEpoch(1)),
            (ShardId::new(1), ShardEpoch(1)),
        ]
        .into_iter()
        .collect()
    );
    assert!(retried.grant.expires_at >= first.grant.expires_at);

    let shard_state = persistence.latest().await;
    assert_eq!(shard_state.executor_count(), 2);
    assert_eq!(
        shard_state.executor_for_addr(new_pod.into()),
        Some(new_executor)
    );
    assert_eq!(shards_at(&shard_state, new_pod), shard_ids(&[0, 1]));
    assert_eq!(
        shard_state.epoch_for_shard(ShardId::new(0)),
        Some(ShardEpoch(1)),
        "the retry advanced the epoch, so it was treated as a replacement"
    );

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
    let new_executor = executor(2);
    let registered = shard_management
        .register_executor(
            new_executor,
            ExecutorAddr::from(new_pod),
            Some("worker-executor-1".into()),
        )
        .await;
    assert!(
        registered.is_err(),
        "a registration whose persist was refused must not be acknowledged, got {registered:?}"
    );

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

    // Let the registration's own write through, then fail the persist of the applied rebalance.
    // Two entries, not three: the pass the registration wakes up finds nothing changed before the
    // plan is executed, so the no-op guard skips that write entirely and the second entry lands on
    // the apply.
    persistence
        .fail_writes(vec![None, Some(ShardManagerError::ConcurrentModification)])
        .await;
    let new_executor = executor(2);
    shard_management
        .register_executor(
            new_executor,
            ExecutorAddr::from(new_pod),
            Some("worker-executor-1".into()),
        )
        .await
        .expect("the registration's own write was let through");

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

    // Spawned: `register_executor` performs the blocked write itself and does not return until it
    // completes.
    let registering = tokio::spawn({
        let shard_management = shard_management.clone();
        async move {
            shard_management
                .register_executor(
                    executor(2),
                    ExecutorAddr::from(new_pod),
                    Some("worker-executor-1".into()),
                )
                .await
        }
    });

    // Signalled from inside `write`, which the caller runs while holding the guard.
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
    registering
        .await
        .expect("the registration task should not panic")
        .expect("the registration should have been persisted");

    // Three: the startup re-grant, the registration's own write, then the one write of the pass it
    // wakes up - the applied rebalance; the pass's first mutation changes nothing and is skipped.
    // Waiting for all of them is what keeps the comparison below off an in-flight write.
    wait_for_writes(&persistence, 3).await;
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

    let new_executor = executor(2);
    let registering = tokio::spawn({
        let shard_management = shard_management.clone();
        async move {
            shard_management
                .register_executor(
                    new_executor,
                    ExecutorAddr::from(new_pod),
                    Some("worker-executor-1".into()),
                )
                .await
        }
    });

    tokio::time::timeout(Duration::from_secs(5), entered)
        .await
        .expect("the registration should have reached a write")
        .expect("the gate should have been signalled");

    // The interrupted caller: `Register` writes out of the loop now, so aborting its task is the
    // request handler dropped when its client disconnects. Draining to `None` is what makes the
    // abort observable - the future, and with it the write guard, is only dropped once the task
    // has actually been reaped.
    registering.abort();
    let _ = registering.await;
    join_set.abort_all();
    while join_set.join_next().await.is_some() {}

    let after = tokio::time::timeout(Duration::from_secs(5), shard_management.current_snapshot())
        .await
        .expect("the aborted write must have released the write lock");
    assert_eq!(
        after, before,
        "the interrupted write left its mutation in the live state. Every reader now serves a \
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
        4,
        &mut join_set,
    )
    .await
    .expect("failed to create shard management");

    // A cold cluster has nothing to re-grant and nothing to plan, so the pass changes nothing - but
    // the no-op guard deliberately lets the very first write through, because until it lands there
    // is no stored `number_of_shards` for a mismatched replica to be refused against.
    wait_for_writes(&persistence, 1).await;

    let writes = persistence.writes.lock().await;
    let (first_prev_revision, _) = writes.first().expect("the loop should have written once");
    assert_eq!(*first_prev_revision, NO_REVISION);
    drop(writes);

    // ...and only the first: the store now holds a revision, so an idle pass is skipped again.
    wait_for_quiescence(&persistence).await;
    assert_eq!(persistence.write_count().await, 1);

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

    let number_of_shards = shard_state.number_of_shards;
    let persistence = TestPersistence::at_revision(shard_state, 7);
    let health_check = Arc::new(TestHealthCheck::all_healthy());
    let mut join_set = JoinSet::new();

    let shard_management = ShardManagement::new(
        Arc::new(persistence.clone()),
        worker_executors,
        health_check,
        0.0,
        LEASE_TTL,
        number_of_shards,
        &mut join_set,
    )
    .await
    .expect("failed to create shard management");

    // The startup re-grant is the first write; a registration is what produces the second, since
    // the balanced startup pass itself changes nothing and is skipped by the no-op guard.
    wait_for_writes(&persistence, 1).await;
    shard_management
        .register_executor(
            executor(2),
            ExecutorAddr::from(pod(2, 9001)),
            Some("worker-executor-1".into()),
        )
        .await
        .expect("the registration should have been persisted");
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
    let new_executor = executor(2);
    let registered = shard_management
        .register_executor(
            new_executor,
            ExecutorAddr::from(new_pod),
            Some("worker-executor-1".into()),
        )
        .await;
    assert!(
        registered.is_err(),
        "a registration whose persist was refused must not be acknowledged, got {registered:?}"
    );

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
    let number_of_shards = shard_state.number_of_shards;
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
            number_of_shards,
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
    let registered = shard_management
        .register_executor(
            executor(2),
            ExecutorAddr::from(new_pod),
            Some("worker-executor-1".into()),
        )
        .await;
    assert!(
        registered.is_err(),
        "a write refused for a lost fence must not acknowledge the registration, got {registered:?}"
    );

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
// The gap ticket 3 characterised, closed: `register_executor` persists the lease and only then
// acknowledges, so a registration whose write is refused is refused to the executor too. Nothing
// is stored, nothing is pushed, and the leader whose fenced write was rejected stops - the write
// happens outside the loop now, so the error reaches the loop through the fail-stop slot rather
// than out of the pass that would otherwise have performed it.
async fn a_registration_is_refused_when_its_persist_fails_and_stops_the_leader() {
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

    let new_executor = executor(2);
    let registered = shard_management
        .register_executor(
            new_executor,
            ExecutorAddr::from(new_pod),
            Some("worker-executor-1".into()),
        )
        .await;

    // No acknowledgement: the executor learns that it is not registered, and retries.
    let err = registered.expect_err(
        "the registration was acknowledged although its write was refused, so the executor now \
         believes it is registered with a leader that has been replaced",
    );
    assert!(
        matches!(err, ShardManagerError::LeadershipLost { .. }),
        "the caller should see the refusal that actually happened, got {err:?}"
    );

    let outcome = tokio::time::timeout(Duration::from_secs(5), join_set.join_next())
        .await
        .expect("the shard management loop should have stopped")
        .expect("the loop task should exist")
        .expect("the loop task should not panic");
    let loop_err = outcome.expect_err("a lost fence must end the loop");
    assert!(
        matches!(
            loop_err.downcast_ref::<ShardManagerError>(),
            Some(ShardManagerError::LeadershipLost { .. })
        ),
        "the loop ended, but not with the lost fence that ended it: {loop_err:#}"
    );

    assert!(
        !shard_management
            .current_snapshot()
            .await
            .has_executor(new_executor),
        "the refused registration is in the in-memory state"
    );
    assert!(
        !persistence.latest().await.has_executor(new_executor),
        "the refused registration reached the store"
    );
    assert!(
        worker_executors.local_assignment(new_pod).await.is_empty(),
        "the executor whose registration was refused was given shards, so it is a second owner of \
         shards the routing table does not record"
    );
}

#[test]
async fn a_shard_count_mismatch_is_refused_before_the_worker_can_act() {
    let worker_executors = Arc::new(TestWorkerExecutors::default());
    let existing_pod = pod(1, 9000);
    let shard_state = shard_state_with_executors(
        4,
        vec![(
            executor(1),
            existing_pod,
            "worker-executor-0",
            &[0, 1, 2, 3],
        )],
    );
    let persistence = TestPersistence::new(shard_state);
    let health_check = Arc::new(TestHealthCheck::all_healthy());
    let mut join_set = JoinSet::new();

    let started = ShardManagement::new(
        Arc::new(persistence.clone()),
        worker_executors.clone(),
        health_check.clone(),
        0.0,
        LEASE_TTL,
        8,
        &mut join_set,
    )
    .await;

    let err = started.err().expect(
        "A replica configured for a different shard count than the stored state must refuse to \
         start: the stored value governs routing, so continuing would route at a count this \
         replica does not believe in.",
    );
    assert!(
        err.to_string().contains("configured for 8"),
        "The refusal should name the configured count, but was: {err}"
    );

    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(
        persistence.attempt_count().await,
        0,
        "The mismatched replica wrote to the store before it was refused, so the check ran after \
         the worker had already begun acting on state it should never have loaded."
    );
    assert!(
        worker_executors.commands_sent().await.is_empty(),
        "The mismatched replica commanded an executor before it was refused."
    );
    assert_eq!(
        health_check.probe_count(),
        0,
        "The mismatched replica probed an executor before it was refused: the check ran after the \
         health check rather than straight after the state was read."
    );
    assert!(
        join_set.is_empty(),
        "The mismatched replica spawned its worker before the check refused it."
    );
}

#[test]
// A renewal asserts the set the executor holds, it does not re-grant it. Advancing the epoch would
// make the protocol non-idempotent: one lost response and the executor's next renewal claims an
// epoch one behind, and is refused for shards it never stopped owning.
async fn renewing_twice_with_the_same_epochs_moves_nothing() {
    let worker_executors = Arc::new(TestWorkerExecutors::default());
    let (shard_management, persistence, mut join_set) =
        new_shard_management(balanced_pair(), worker_executors.clone()).await;

    let before = persistence.latest().await;
    let claimed = claim_of(&before, executor(1));
    assert_eq!(
        claimed.keys().copied().collect::<BTreeSet<_>>(),
        shard_ids(&[0, 1])
    );

    let first = shard_management
        .renew_shard_lease(executor(1), &claimed)
        .await
        .expect("the first renewal should have been granted");
    let second = shard_management
        .renew_shard_lease(executor(1), &claimed)
        .await
        .expect("the second renewal of the same set should have been granted too");

    assert_eq!(first.shard_epochs, claimed);
    assert_eq!(
        second.shard_epochs, claimed,
        "a renewal advanced an epoch, so the executor is now one behind for a shard it still owns"
    );
    assert!(second.expires_at >= first.expires_at);
    assert!(second.expires_at > expiry_of(&before, executor(1)));

    let after = persistence.latest().await;
    assert_eq!(claim_of(&after, executor(1)), claimed);
    assert_eq!(
        after.shards_for_executor(executor(1)),
        Some(shard_ids(&[0, 1]))
    );
    assert_eq!(
        after.shards_for_executor(executor(2)),
        Some(shard_ids(&[2, 3])),
        "renewing one executor's lease disturbed another's shards"
    );
    assert_eq!(
        claim_of(&after, executor(2)),
        claim_of(&before, executor(2))
    );

    join_set.abort_all();
}

#[test]
// The whole claim is validated before anything is renewed, and a claim that does not match is
// refused in one piece: a revoked shard, a shard that moved elsewhere and a wrong epoch all mean
// the executor's picture is stale, and renewing the parts that happen to still be right would
// extend a lease on a picture the manager knows is wrong.
async fn a_stale_claim_renews_nothing_and_leaves_the_expiry_untouched() {
    let worker_executors = Arc::new(TestWorkerExecutors::default());
    let (shard_management, persistence, mut join_set) =
        new_shard_management(balanced_pair(), worker_executors.clone()).await;

    let before = persistence.latest().await;
    let expiry_before = expiry_of(&before, executor(1));
    let writes_before = persistence.write_count().await;

    // an executor the manager has never heard of
    let err = shard_management
        .renew_shard_lease(executor(9), &BTreeMap::new())
        .await
        .expect_err("an unknown executor holds no lease to renew");
    assert!(
        matches!(
            err,
            ShardManagerError::ShardLeaseNotFound { executor_id } if executor_id == executor(9)
        ),
        "got {err:?}"
    );

    // a wrong epoch, alongside a claim entry that is perfectly valid
    let mut wrong_epoch = claim_of(&before, executor(1));
    wrong_epoch.insert(ShardId::new(1), ShardEpoch(7));
    let err = shard_management
        .renew_shard_lease(executor(1), &wrong_epoch)
        .await
        .expect_err("a claim at the wrong epoch must be refused");
    assert!(
        matches!(
            err,
            ShardManagerError::StaleShardEpoch {
                shard_id,
                expected: Some(ShardEpoch(0)),
                provided: ShardEpoch(7),
                ..
            } if shard_id == ShardId::new(1)
        ),
        "got {err:?}"
    );

    // a shard that belongs to another executor
    let moved = BTreeMap::from([(ShardId::new(2), ShardEpoch(0))]);
    let err = shard_management
        .renew_shard_lease(executor(1), &moved)
        .await
        .expect_err("claiming another executor's shard must be refused");
    assert!(
        matches!(
            err,
            ShardManagerError::StaleShardEpoch {
                shard_id,
                expected: None,
                ..
            } if shard_id == ShardId::new(2)
        ),
        "got {err:?}"
    );

    // ...and a shard nobody owns any more. Deregistering executor 2 releases its shards without
    // waking the loop, so they stay unassigned for the rest of this test.
    shard_management
        .deregister_executor(executor(2), &claim_of(&before, executor(2)))
        .await
        .expect("a graceful deregistration should have been persisted");
    let revoked = BTreeMap::from([
        (ShardId::new(0), ShardEpoch(0)),
        (ShardId::new(2), ShardEpoch(0)),
    ]);
    let err = shard_management
        .renew_shard_lease(executor(1), &revoked)
        .await
        .expect_err("claiming a released shard must be refused");
    assert!(
        matches!(
            err,
            ShardManagerError::StaleShardEpoch {
                shard_id,
                expected: None,
                ..
            } if shard_id == ShardId::new(2)
        ),
        "got {err:?}"
    );

    // Nothing was renewed by any of them: the expiry is exactly where it was, shard 0 - valid in
    // two of the refused claims - never moved, and the only write was the deregistration's.
    let after = persistence.latest().await;
    assert_eq!(
        expiry_of(&after, executor(1)),
        expiry_before,
        "a refused renewal extended the lease anyway"
    );
    assert_eq!(
        claim_of(&after, executor(1)),
        claim_of(&before, executor(1))
    );
    assert_eq!(
        persistence.write_count().await,
        writes_before + 1,
        "a refused renewal was persisted"
    );

    join_set.abort_all();
}

#[test]
// The claim is checked in the direction the executor can be wrong about: a shard the manager has
// assigned to it that it does not claim is an executor that has not received its last push yet, and
// the push - not a refused renewal - is what corrects that. The full set comes back either way.
async fn an_owned_shard_the_executor_did_not_claim_is_not_an_error() {
    let worker_executors = Arc::new(TestWorkerExecutors::default());
    let (shard_management, persistence, mut join_set) =
        new_shard_management(balanced_pair(), worker_executors.clone()).await;

    let before = persistence.latest().await;
    let partial = BTreeMap::from([(ShardId::new(0), ShardEpoch(0))]);

    let grant = shard_management
        .renew_shard_lease(executor(1), &partial)
        .await
        .expect("a claim that is behind the manager must still renew");

    assert_eq!(
        grant.shard_epochs,
        claim_of(&before, executor(1)),
        "the renewal answered the claim rather than the executor's full current set"
    );
    assert!(grant.expires_at > expiry_of(&before, executor(1)));

    join_set.abort_all();
}

#[test]
// Housekeeping runs before the lease is looked up, so a lease that had already lapsed when its
// renewal arrived is gone rather than silently resurrected: its shards may already be on their way
// to another executor. The loop is stopped first so that the lapse is observed by the renewal and
// not by a tick.
async fn a_lease_that_lapsed_before_its_renewal_is_not_found() {
    let worker_executors = Arc::new(TestWorkerExecutors::default());
    let (shard_management, persistence, mut join_set) = start_shard_management(
        balanced_pair(),
        worker_executors.clone(),
        Duration::from_secs(1),
    )
    .await;

    let before = persistence.latest().await;
    let claimed = claim_of(&before, executor(1));

    join_set.abort_all();
    while join_set.join_next().await.is_some() {}
    assert!(persistence.latest().await.has_executor(executor(1)));

    tokio::time::sleep(Duration::from_millis(1200)).await;

    let err = shard_management
        .renew_shard_lease(executor(1), &claimed)
        .await
        .expect_err("a lease that had already lapsed must not be renewable");
    assert!(
        matches!(
            err,
            ShardManagerError::ShardLeaseNotFound { executor_id } if executor_id == executor(1)
        ),
        "got {err:?}"
    );
}

#[test]
// Lease expiries are persisted and absolute, so after any outage longer than one lease every stored
// expiry is in the past. Without the startup re-grant the first pass's housekeeping would evict a
// cluster whose executors had just answered the health check.
async fn past_expiries_do_not_evict_a_healthy_cluster_on_restart() {
    let worker_executors = Arc::new(TestWorkerExecutors::default());
    let seeded = balanced_pair();
    assert!(
        expiry_of(&seeded, executor(1)) < Utc::now(),
        "this test only means something if the seeded expiries are already in the past"
    );

    let (_shard_management, persistence, mut join_set) =
        new_shard_management(seeded.clone(), worker_executors.clone()).await;

    let after = persistence.latest().await;
    assert!(
        after.has_executor(executor(1)) && after.has_executor(executor(2)),
        "a healthy cluster was evicted on restart because its persisted leases had expired"
    );
    assert_eq!(
        after.shards_for_executor(executor(1)),
        Some(shard_ids(&[0, 1]))
    );
    assert_eq!(
        after.shards_for_executor(executor(2)),
        Some(shard_ids(&[2, 3]))
    );
    assert!(after.get_unassigned_shards().is_empty());
    assert!(expiry_of(&after, executor(1)) > Utc::now());
    assert!(expiry_of(&after, executor(2)) > Utc::now());
    // the re-grant moves the lease clock only: no shard changed owner, so no epoch moved
    assert_eq!(
        claim_of(&after, executor(1)),
        claim_of(&seeded, executor(1))
    );
    assert_eq!(
        claim_of(&after, executor(2)),
        claim_of(&seeded, executor(2))
    );

    join_set.abort_all();
}

#[test]
// The lease paths never wake the loop, so the timer is the only thing that can notice an expiry in
// a cluster where nothing else is happening. Without it a lapsed lease is held forever and its
// shards are never re-homed.
async fn an_expired_lease_is_reclaimed_within_one_tick() {
    let worker_executors = Arc::new(TestWorkerExecutors::default());
    let (_shard_management, persistence, mut join_set) = start_shard_management(
        balanced_pair(),
        worker_executors.clone(),
        Duration::from_secs(1),
    )
    .await;

    // Nothing else happens from here: no registration, no unregistration, no failed push. Only the
    // tick can wake the loop.
    let start = Instant::now();
    loop {
        let shard_state = persistence.latest().await;
        if shard_state.executor_count() == 0 {
            assert_eq!(
                shard_state.get_unassigned_shards(),
                shard_ids(&[0, 1, 2, 3])
            );
            assert!(shard_state.pending_rebalance.is_empty());
            break;
        }
        if start.elapsed() > Duration::from_secs(5) {
            panic!("the expired leases were never reclaimed: {shard_state}");
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    join_set.abort_all();
}

#[test]
// A graceful shutdown hands the lease back and the shards with it. `Deregister` deliberately does
// not notify the loop - the ticket makes the lease protocol pull-based - so the tick is what has to
// re-home them, and it bounds the hand-off by one period.
async fn deregistering_an_executor_re_homes_its_shards_within_one_tick() {
    let leaving_pod = pod(1, 9000);
    let staying_pod = pod(2, 9001);
    let worker_executors = Arc::new(TestWorkerExecutors::default());
    let (shard_management, persistence, mut join_set) = start_shard_management(
        balanced_pair(),
        worker_executors.clone(),
        Duration::from_secs(3),
    )
    .await;

    let before = persistence.latest().await;
    shard_management
        .deregister_executor(executor(1), &claim_of(&before, executor(1)))
        .await
        .expect("a graceful deregistration should have been persisted");

    wait_for_local_assignment(&worker_executors, staying_pod, shard_ids(&[0, 1, 2, 3])).await;

    let after = persistence.latest().await;
    assert!(!after.has_executor(executor(1)));
    assert_eq!(
        after.shards_for_executor(executor(2)),
        Some(shard_ids(&[0, 1, 2, 3]))
    );
    assert!(after.pending_rebalance.is_empty());
    assert!(after.get_unassigned_shards().is_empty());
    // the re-homed shards changed owner, so their epochs advanced; the ones that stayed did not
    assert_eq!(after.epoch_for_shard(ShardId::new(0)), Some(ShardEpoch(1)));
    assert_eq!(after.epoch_for_shard(ShardId::new(2)), Some(ShardEpoch(0)));
    // Nothing is sent to the executor that left: it asked to be released because it is shutting
    // down, and the manager holds no lease to revoke against any more. Its epochs have advanced
    // under it, which is what fences it if it comes back believing it still owns those shards.
    assert!(
        worker_executors.pushes_to(leaving_pod).await.len() <= 1,
        "the deregistered executor was pushed a new set after it had left"
    );

    join_set.abort_all();
}

#[test]
// Deregistering an executor the manager does not know is not a failure: a shutdown must never fail
// on bookkeeping, and neither must a stale claim.
async fn deregistering_an_unknown_executor_succeeds() {
    let worker_executors = Arc::new(TestWorkerExecutors::default());
    let (shard_management, persistence, mut join_set) =
        new_shard_management(balanced_pair(), worker_executors.clone()).await;

    let writes_before = persistence.write_count().await;
    shard_management
        .deregister_executor(
            executor(9),
            &BTreeMap::from([(ShardId::new(0), ShardEpoch(4))]),
        )
        .await
        .expect("deregistering an unknown executor must succeed");

    assert_eq!(
        persistence.write_count().await,
        writes_before,
        "deregistering an executor that holds no lease changed nothing, so nothing should be stored"
    );
    assert_eq!(persistence.latest().await.executor_count(), 2);

    join_set.abort_all();
}

#[test]
// With a timer, an idle cluster would otherwise store a fresh full blob every period forever - each
// one a new revision the backend has to compact. That the timer is really firing is what
// `an_expired_lease_is_reclaimed_within_one_tick` pins; this pins what it must not do.
async fn an_idle_tick_does_not_advance_the_revision() {
    let worker_executors = Arc::new(TestWorkerExecutors::default());
    let (_shard_management, persistence, mut join_set) = start_shard_management(
        balanced_pair(),
        worker_executors.clone(),
        Duration::from_secs(3),
    )
    .await;

    let writes_before = persistence.write_count().await;
    let revision_before = persistence.latest().await.revision;

    // One tick is a second; nothing lapses before three.
    tokio::time::sleep(Duration::from_millis(1500)).await;

    assert_eq!(
        persistence.write_count().await,
        writes_before,
        "an idle pass persisted the state although nothing had changed"
    );
    assert_eq!(persistence.latest().await.revision, revision_before);

    join_set.abort_all();
}

#[test]
// The boundary of the no-op guard: taking work off `pending_rebalance` *is* a change, so a pass
// that drained it is never skipped. Nothing has to special-case the queue for that to hold - the
// comparison is against the state as it was before the mutation, and the queue is part of it.
async fn draining_pending_rebalance_alone_is_persisted() {
    let worker_executors = Arc::new(TestWorkerExecutors::default());
    // No executors, so there is nothing to plan and nothing to push: draining the queue is the only
    // thing this pass does.
    let mut shard_state = ShardLeaseState::new(4);
    shard_state.pending_rebalance.insert(ShardId::new(3));

    let (_shard_management, persistence, mut join_set) =
        start_shard_management(shard_state, worker_executors.clone(), LEASE_TTL).await;

    wait_for_writes(&persistence, 1).await;
    assert!(persistence.latest().await.pending_rebalance.is_empty());

    // ...and only that: with the queue empty there is nothing left for a later pass to change.
    wait_for_quiescence(&persistence).await;
    assert_eq!(persistence.write_count().await, 1);

    join_set.abort_all();
}

#[test]
// A renewal writes outside the loop, so its refused write only ends its own request. The store may
// or may not hold what it tried to write, and a refused fenced write means another shard manager
// owns the topology - so the failure is parked for the loop, which stops the process with it.
async fn a_persist_failure_while_renewing_stops_the_loop() {
    let worker_executors = Arc::new(TestWorkerExecutors::default());
    let (shard_management, persistence, mut join_set) =
        new_shard_management(balanced_pair(), worker_executors.clone()).await;

    let before = persistence.latest().await;
    let claimed = claim_of(&before, executor(1));

    persistence
        .fail_writes(vec![Some(ShardManagerError::LeadershipLost {
            leader_key: "/golem/shard-manager/leader/6c2f".to_string(),
            create_revision: 41,
        })])
        .await;

    let err = shard_management
        .renew_shard_lease(executor(1), &claimed)
        .await
        .expect_err("a renewal whose persist was refused must not be granted");
    assert!(
        matches!(err, ShardManagerError::LeadershipLost { .. }),
        "the caller should see the refusal that actually happened, got {err:?}"
    );

    let outcome = tokio::time::timeout(Duration::from_secs(5), join_set.join_next())
        .await
        .expect(
            "the shard management loop is still running after a renewal's write was refused for a \
             lost fence, so it is about to command executors on a topology it no longer owns",
        )
        .expect("the loop task should exist")
        .expect("the loop task should not panic");
    let loop_err = outcome.expect_err("a lost fence must end the loop");
    assert!(
        matches!(
            loop_err.downcast_ref::<ShardManagerError>(),
            Some(ShardManagerError::LeadershipLost { .. })
        ),
        "the loop ended, but not with the lost fence that ended it: {loop_err:#}"
    );

    assert_eq!(persistence.latest().await, before);
    assert_eq!(
        expiry_of(&persistence.latest().await, executor(1)),
        expiry_of(&before, executor(1))
    );
}
