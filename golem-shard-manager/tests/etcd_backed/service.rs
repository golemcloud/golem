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

//! [`ShardManagement`] driven against the real persistence backends.
//!
//! `shard_management.rs` covers the service's behaviour in depth, but every one of those tests runs
//! against an in-memory double. What is only provable here is that the same lifecycle survives a
//! real store: that a registration is durable, that a renewal moves the stored expiry rather than
//! only the returned grant, that a lapsed lease is reclaimed from state that was read back out of
//! the backend, and that a write refused by the compare-and-swap stops the loop on every backend.
//!
//! Every test is multiplied over the `persistence` dimension declared below, so each one runs as
//! `<name>_sqlite`, `<name>_postgres` and `<name>_etcd`. The fixtures behind the dimension live in
//! [`super`]; the etcd case shares the per-worker container, which is why this module sits under
//! `etcd_backed` and runs in its sequential suite.

use crate::etcd_backed::persistence::{
    GetRoutingTablePersistence, LEASE_TTL, NUMBER_OF_SHARDS, PersistenceStore,
};
use crate::shard_management::{TestHealthCheck, TestWorkerExecutors, executor, pod, shard_ids};
use golem_common::model::ShardId;
use golem_shard_manager::{
    ExecutorAddr, RoutingTablePersistence, ShardEpoch, ShardLeaseState, ShardManagement,
    ShardManagerError,
};
use golem_test_framework::components::etcd::docker_etcd::DockerEtcd;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::Duration;
use test_r::{define_matrix_dimension, inherit_test_dep, test};
use tokio::task::JoinSet;
use tokio::time::Instant;

inherit_test_dep!(Arc<DockerEtcd>);
inherit_test_dep!(#[tagged_as("sqlite")] Arc<dyn GetRoutingTablePersistence>);
inherit_test_dep!(#[tagged_as("postgres")] Arc<dyn GetRoutingTablePersistence>);
inherit_test_dep!(#[tagged_as("etcd")] Arc<dyn GetRoutingTablePersistence>);

// Declared per module on purpose: the macro emits a module-local helper that the generated test
// cases call unqualified, so it cannot be shared from a sibling.
define_matrix_dimension!(persistence: Arc<dyn GetRoutingTablePersistence> -> "sqlite", "postgres", "etcd");

/// Short enough that the loop's tick - a third of it - reclaims a lapsed lease inside a test, long
/// enough that the registration and its rebalance cannot themselves be outrun by the expiry.
const RECLAIM_LEASE_TTL: Duration = Duration::from_secs(3);

/// A `ShardManagement` over `persistence`, plus the join set holding its loop.
async fn start(
    persistence: Arc<dyn RoutingTablePersistence>,
    worker_executors: Arc<TestWorkerExecutors>,
    lease_ttl: Duration,
) -> (ShardManagement, JoinSet<anyhow::Result<()>>) {
    let mut join_set = JoinSet::new();
    let shard_management = ShardManagement::new(
        persistence,
        worker_executors,
        Arc::new(TestHealthCheck::all_healthy()),
        0.0,
        lease_ttl,
        NUMBER_OF_SHARDS,
        &mut join_set,
    )
    .await
    .expect("the shard management loop should have started over a real store");
    (shard_management, join_set)
}

/// A second client over the same store, for reading what the service actually persisted rather
/// than what its own cache holds.
///
/// Taken once per test and reused: `connect` is expensive on every backend - two pools on SQLite, a
/// connection and a round trip on Postgres, and on etcd two clients plus a leader key that is
/// written and never deleted - so connecting per poll would litter the shared etcd server.
async fn client(store: &Arc<dyn PersistenceStore>) -> Arc<dyn RoutingTablePersistence> {
    store.connect().await
}

async fn stored(reader: &Arc<dyn RoutingTablePersistence>) -> ShardLeaseState {
    reader
        .read()
        .await
        .expect("reading the stored shard lease state should succeed")
        .0
}

/// Polls the store until `done` accepts the state it holds.
async fn wait_for_stored(
    reader: &Arc<dyn RoutingTablePersistence>,
    what: &str,
    done: impl Fn(&ShardLeaseState) -> bool,
) -> ShardLeaseState {
    let start = Instant::now();
    loop {
        let shard_state = stored(reader).await;
        if done(&shard_state) {
            return shard_state;
        }
        if start.elapsed() > Duration::from_secs(10) {
            panic!("timed out waiting for {what}; stored state: {shard_state}");
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

fn all_shards() -> BTreeSet<ShardId> {
    shard_ids(&(0..NUMBER_OF_SHARDS as i64).collect::<Vec<_>>())
}

fn holds_every_shard(shard_state: &ShardLeaseState) -> bool {
    shard_state.shards_for_executor(executor(1)) == Some(all_shards())
}

#[test]
#[tracing::instrument]
// A registration is only useful if it outlives the process that served it: the next leader reads
// its state from the store, not from the handler that granted the lease.
async fn a_registration_is_durable(
    #[dimension(persistence)] persistence: &Arc<dyn GetRoutingTablePersistence>,
) {
    let store = persistence.new_store().await;
    let reader = client(&store).await;
    let worker_executors = Arc::new(TestWorkerExecutors::default());
    let (shard_management, mut join_set) =
        start(client(&store).await, worker_executors.clone(), LEASE_TTL).await;

    let registering = pod(1, 9000);
    let ack = shard_management
        .register_executor(
            executor(1),
            ExecutorAddr::from(registering),
            Some("worker-executor-0".into()),
        )
        .await
        .expect("the registration should have been persisted");
    assert_eq!(ack.number_of_shards, NUMBER_OF_SHARDS);

    // The whole cluster is one executor, so the rebalance that follows gives it every shard.
    let shard_state = wait_for_stored(
        &reader,
        "the registered executor to hold every shard",
        holds_every_shard,
    )
    .await;

    assert_eq!(shard_state.executor_count(), 1);
    assert_eq!(
        shard_state.executor_for_addr(registering.into()),
        Some(executor(1)),
        "the address the executor registered under was not stored"
    );
    assert!(shard_state.get_unassigned_shards().is_empty());
    assert!(
        shard_state.check_invariants().is_ok(),
        "the stored state broke its own invariants: {shard_state}"
    );

    join_set.abort_all();
}

#[test]
#[tracing::instrument]
// The grant a renewal returns is read off the state it just persisted, so the stored expiry has to
// move with it. A renewal that only extended the reply would leave the next leader reclaiming a
// lease the executor believes it holds.
async fn a_renewal_extends_the_stored_lease(
    #[dimension(persistence)] persistence: &Arc<dyn GetRoutingTablePersistence>,
) {
    let store = persistence.new_store().await;
    let reader = client(&store).await;
    let worker_executors = Arc::new(TestWorkerExecutors::default());
    let (shard_management, mut join_set) =
        start(client(&store).await, worker_executors.clone(), LEASE_TTL).await;

    shard_management
        .register_executor(executor(1), ExecutorAddr::from(pod(1, 9000)), None)
        .await
        .expect("the registration should have been persisted");
    let before = wait_for_stored(&reader, "the initial assignment", holds_every_shard).await;
    let held: BTreeMap<ShardId, ShardEpoch> = before
        .shard_assignments
        .iter()
        .filter(|(_, entry)| entry.executor_id == executor(1))
        .map(|(shard_id, entry)| (*shard_id, entry.epoch))
        .collect();
    let expiry_before = before.executor_leases[&executor(1)].expires_at;

    let grant = shard_management
        .renew_shard_lease(executor(1), held.clone())
        .await
        .expect("held epochs matching the manager's set should have been renewed");

    assert_eq!(
        grant.shard_epochs, held,
        "the renewal moved an epoch for a shard the executor still owns"
    );

    let after = stored(&reader).await;
    assert!(
        after.executor_leases[&executor(1)].expires_at > expiry_before,
        "the renewal extended the returned grant but not the stored lease"
    );
    assert_eq!(after.shards_for_executor(executor(1)), Some(all_shards()));

    join_set.abort_all();
}

#[test]
#[tracing::instrument]
// Reclamation is driven by the loop's timer against state it reads back from the store. Nothing
// else happens here - no deregistration, no failed push - so only the tick can notice the expiry.
async fn a_lapsed_lease_is_reclaimed_from_the_store(
    #[dimension(persistence)] persistence: &Arc<dyn GetRoutingTablePersistence>,
) {
    let store = persistence.new_store().await;
    let reader = client(&store).await;
    let worker_executors = Arc::new(TestWorkerExecutors::default());
    let (shard_management, mut join_set) = start(
        client(&store).await,
        worker_executors.clone(),
        RECLAIM_LEASE_TTL,
    )
    .await;

    shard_management
        .register_executor(executor(1), ExecutorAddr::from(pod(1, 9000)), None)
        .await
        .expect("the registration should have been persisted");

    // Either state is a legitimate observation: on a machine slow enough for the lease to lapse
    // before the first poll, the reclamation this test is about has simply already happened.
    // Requiring the assignment here would turn that into a timeout blamed on the wrong step.
    wait_for_stored(
        &reader,
        "the initial assignment, or the lease already reclaimed",
        |s| holds_every_shard(s) || s.executor_count() == 0,
    )
    .await;

    // Nothing renews it from here.
    let reclaimed = wait_for_stored(&reader, "the lapsed lease to be reclaimed", |s| {
        s.executor_count() == 0
    })
    .await;

    assert_eq!(
        reclaimed.get_unassigned_shards(),
        all_shards(),
        "the lapsed lease was reclaimed but its shards were not released"
    );
    assert!(
        reclaimed.shard_assignments.is_empty(),
        "an assignment survived the executor that held it: {reclaimed}"
    );
    assert!(
        reclaimed.check_invariants().is_ok(),
        "the stored state broke its own invariants: {reclaimed}"
    );

    join_set.abort_all();
}

#[test]
#[tracing::instrument]
// A write refused by the compare-and-swap is what the stored revision exists for, and the service
// does not retry it: the revision it cached is its fencing token, so the write is reported to the
// caller and the loop stops rather than reapplying anything on top of the winner's state.
//
// The second writer here is another client over the same store, not a second shard manager - on
// etcd it mints its own leader key, so both writers pass their own fence and what is exercised is
// the revision check, not leadership. Two real shard managers cannot both hold the election key.
async fn a_write_that_lost_the_race_is_refused(
    #[dimension(persistence)] persistence: &Arc<dyn GetRoutingTablePersistence>,
) {
    let store = persistence.new_store().await;
    let reader = client(&store).await;
    let worker_executors = Arc::new(TestWorkerExecutors::default());
    let (shard_management, mut join_set) =
        start(client(&store).await, worker_executors.clone(), LEASE_TTL).await;

    shard_management
        .register_executor(executor(1), ExecutorAddr::from(pod(1, 9000)), None)
        .await
        .expect("the registration should have been persisted");
    wait_for_stored(&reader, "the initial assignment", holds_every_shard).await;

    // The other writer commits, which advances the stored revision. The content is deliberately
    // unchanged: what the service's next write collides with is the revision, nothing else.
    let other = client(&store).await;
    let (state, revision) = other
        .read()
        .await
        .expect("the second client should be able to read the store");
    other
        .write(&state, revision)
        .await
        .expect("the second client should win the race it is the only entrant in");

    let refused = shard_management
        .register_executor(executor(2), ExecutorAddr::from(pod(2, 9001)), None)
        .await;

    assert!(
        matches!(refused, Err(ShardManagerError::ConcurrentModification)),
        "expected the losing write to be refused as a concurrent modification, got {refused:?}"
    );

    // Read before the loop is awaited below: this is the state the winner left behind, and nothing
    // of the refused registration may appear in it.
    let after = stored(&reader).await;
    assert_eq!(after.executor_count(), 1);
    assert_eq!(after.executor_for_addr(pod(2, 9001).into()), None);

    // The refusal is a fail-stop, not a retry: a leader that lost its revision must not go on
    // commanding executors. Same shape as the fail-stop assertions in `shard_management.rs`.
    let outcome = tokio::time::timeout(Duration::from_secs(5), join_set.join_next())
        .await
        .expect("the shard management loop should have stopped after its write was refused")
        .expect("the loop task should exist")
        .expect("the loop task should not panic");
    let loop_err = outcome.expect_err("the loop must end with the refused write");
    assert!(
        matches!(
            loop_err.downcast_ref::<ShardManagerError>(),
            Some(ShardManagerError::ConcurrentModification)
        ),
        "the loop ended, but not with the concurrent modification that ended it: {loop_err:#}"
    );
}
