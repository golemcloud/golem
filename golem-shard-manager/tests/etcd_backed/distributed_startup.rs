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

//! Coverage for `run()`'s distributed-mode wiring: that it campaigns before it serves, that a
//! standby never opens its gRPC port, and that the startup guards fire.
//!
//! In-process rather than spawned binaries: distributed mode needs no database, both listeners
//! accept port 0 and report what they bound, and the test framework cannot point a spawned shard
//! manager at etcd.

use super::proxy::BreakableProxy;
use anyhow::anyhow;
use chrono::DateTime;
use golem_shard_manager::config::{
    EtcdConfig, GrpcApiConfig, PersistenceConfig, ShardManagerConfig,
};
use golem_shard_manager::{
    Deployment, EtcdRoutingTablePersistence, ExecutorAddr, ExecutorId, LEADER_ELECTION_NAME,
    LeaderElection, LeaderFence, LeaseKeepAlive, LeaseLossReason, LeaseLost, NO_REVISION,
    RoutingTablePersistence, RunDetails, STATE_KEY, ShardLeaseState, ShardManagerError,
};
use golem_test_framework::components::etcd::docker_etcd::DockerEtcd;
use std::collections::BTreeSet;
use std::net::Ipv4Addr;
use std::sync::Arc;
use std::time::Duration;
use test_r::{inherit_test_dep, test};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::oneshot;
use tokio::task::{JoinHandle, JoinSet};
use tokio::time::{Instant, timeout};
use tokio_util::sync::CancellationToken;
use tracing::info;
use uuid::Uuid;

inherit_test_dep!(Arc<DockerEtcd>);

const LEASE_TTL: Duration = Duration::from_secs(2);
/// Two of these must fit inside `LEASE_TTL` or startup refuses to run, so it cannot inherit the
/// five-second default.
const REQUEST_TIMEOUT: Duration = Duration::from_millis(500);
const STARTUP_TIMEOUT: Duration = Duration::from_secs(20);
/// For failures that must be reported *before* the campaign: one that reached the campaign
/// instead would block behind the leader indefinitely.
const REFUSAL_TIMEOUT: Duration = Duration::from_secs(5);
/// Well inside `LEASE_TTL`: a leadership freed only by its lease lapsing is still there when this
/// closes, which is what distinguishes a revoke from an expiry.
const REVOKE_WINDOW: Duration = Duration::from_millis(500);
const NUMBER_OF_SHARDS: usize = 1024;
const IS_LEADER_METRIC: &str = "shard_manager_is_leader";

fn etcd_config(etcd: &DockerEtcd) -> EtcdConfig {
    EtcdConfig {
        endpoints: vec![etcd.client_url()],
        leader_lease_ttl: LEASE_TTL,
        request_timeout: REQUEST_TIMEOUT,
        ..EtcdConfig::default()
    }
}

fn distributed_config(etcd: &DockerEtcd) -> ShardManagerConfig {
    ShardManagerConfig {
        // Spelled out rather than defaulted: one test starts a second replica differing only
        // in this field.
        number_of_shards: NUMBER_OF_SHARDS,
        // Both are 0 because these replicas coexist in one process: a standby binds its HTTP
        // port even though it never reaches its gRPC one.
        http_port: 0,
        grpc: GrpcApiConfig {
            port: 0,
            ..GrpcApiConfig::default()
        },
        persistence: PersistenceConfig::Etcd(etcd_config(etcd)),
        ..ShardManagerConfig::default()
    }
}

/// Clears any shard lease state left behind by another test file.
///
/// The state key is fixed and the etcd server shared, so a state left at a different shard count
/// would trip the startup guard for reasons unrelated to what is under test.
async fn wipe_state(etcd: &DockerEtcd) {
    etcd_client::Client::connect([etcd.client_url()], None)
        .await
        .expect("Cannot connect to etcd")
        .kv_client()
        .delete(STATE_KEY, None)
        .await
        .expect("Cannot wipe the shard lease state");
}

/// Waits until a shard lease state is stored.
///
/// A leader reports its ports before its worker has persisted anything, so a test that needs the
/// shard count check to fire must wait for the write, not the ports.
async fn await_state_written(etcd: &DockerEtcd) {
    let mut kv = etcd_client::Client::connect([etcd.client_url()], None)
        .await
        .expect("Cannot connect to etcd")
        .kv_client();

    timeout(STARTUP_TIMEOUT, async {
        while kv
            .get(STATE_KEY, None)
            .await
            .expect("Cannot read the shard lease state")
            .kvs()
            .is_empty()
        {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("The leader should persist its shard lease state shortly after being elected");
}

/// The campaigner keys currently queued under the production election name.
///
/// A set rather than a count: an earlier test's aborted replica leaves its key behind until its
/// lease expires, so a new campaigner is only identifiable by difference.
async fn campaigner_keys(etcd: &DockerEtcd) -> BTreeSet<Vec<u8>> {
    etcd_client::Client::connect([etcd.client_url()], None)
        .await
        .expect("Cannot connect to etcd")
        .kv_client()
        .get(
            format!("{LEADER_ELECTION_NAME}/"),
            Some(etcd_client::GetOptions::new().with_prefix()),
        )
        .await
        .expect("Cannot read the election prefix")
        .kvs()
        .iter()
        .map(|kv| kv.key().to_vec())
        .collect()
}

/// Starts a replica in the background, reporting its bound ports the moment `run()` returns.
///
/// That moment is the signal every test here rests on: a leader's `run()` returns with its ports,
/// a standby's blocks in the campaign and never returns.
fn spawn_replica(
    config: ShardManagerConfig,
    shutdown: CancellationToken,
) -> (
    JoinHandle<anyhow::Result<()>>,
    oneshot::Receiver<RunDetails>,
) {
    let (started, ports) = oneshot::channel();

    let handle = tokio::spawn(async move {
        let mut join_set: JoinSet<anyhow::Result<()>> = JoinSet::new();
        let details = match golem_shard_manager::run(
            &config,
            Deployment::Standalone { shutdown },
            prometheus::Registry::new(),
            &mut join_set,
        )
        .await
        {
            Ok(details) => details,
            // Otherwise the dropped sender reaches the test as a bare `RecvError`, losing this
            // error.
            Err(err) => {
                join_set.abort_all();
                panic!("The replica failed to start: {err:#}");
            }
        };
        let _ = started.send(details);

        // Keep the replica up until aborted, as `serve_until_stopped` would.
        while let Some(result) = join_set.join_next().await {
            result??;
        }
        Ok(())
    });

    (handle, ports)
}

#[test]
#[tracing::instrument(skip_all)]
// The whole distributed path in one pass: the leader serves, the standby blocks in its campaign,
// and takeover costs a lease expiry rather than a revoke.
async fn a_standby_does_not_serve_until_the_leader_goes_away(etcd: &Arc<DockerEtcd>) {
    wipe_state(etcd).await;
    let config = distributed_config(etcd);

    let (leader, leader_ports) = spawn_replica(config.clone(), CancellationToken::new());
    let leader_details = timeout(STARTUP_TIMEOUT, leader_ports)
        .await
        .expect(
            "The first replica should win an uncontested campaign and finish starting. A timeout \
             here means `run()` campaigns but never completes - check that the keepalive is \
             spawned and that the campaign is not running on the request-timeout client.",
        )
        .expect("The leader's startup should report its ports");

    assert!(
        TcpStream::connect(("127.0.0.1", leader_details.grpc_port))
            .await
            .is_ok(),
        "The leader reported gRPC port {} but nothing is listening on it.",
        leader_details.grpc_port
    );

    let (standby, mut standby_ports) = spawn_replica(config.clone(), CancellationToken::new());

    // Longer than one lease TTL: a standby that stopped renewing during its campaign would
    // already have lost its lease by now.
    let observed_for = LEASE_TTL * 2;
    assert!(
        timeout(observed_for, &mut standby_ports).await.is_err(),
        "The second replica finished starting while the first still held the leadership. Both \
         would be serving a routing table and driving topology at once, which is precisely what \
         leader election exists to prevent."
    );
    assert!(
        !standby.is_finished(),
        "The standby's startup ended instead of blocking in the campaign."
    );

    // No port to probe, because it never reached its gRPC bind - which is itself the readiness
    // signal: a probe against gRPC is refused, so the Service routes only to the leader.
    info!("Standby stayed blocked for {observed_for:?}; killing the leader");

    let killed_at = Instant::now();
    leader.abort();

    let standby_details = timeout(STARTUP_TIMEOUT, standby_ports)
        .await
        .expect("The standby should be elected once the leader stops renewing its lease")
        .expect("The standby's startup should report its ports");
    let failover = killed_at.elapsed();

    // Aborted, not resigned, so the leadership can only free up on expiry - one renewal interval
    // short of the TTL, since etcd counts from the last renewal it received.
    let renewal_interval = LeaseKeepAlive::renewal_interval(LEASE_TTL);
    assert!(
        failover >= LEASE_TTL - renewal_interval,
        "The standby was elected {failover:?} after the leader was killed, sooner than its \
         {LEASE_TTL:?} lease could lapse. Something released the leadership instead of letting it \
         expire."
    );

    assert!(
        TcpStream::connect(("127.0.0.1", standby_details.grpc_port))
            .await
            .is_ok(),
        "The promoted standby reported gRPC port {} but nothing is listening on it.",
        standby_details.grpc_port
    );

    standby.abort();
    info!(?failover, "Standby took over and began serving");
}

#[test]
#[tracing::instrument(skip_all)]
// `launch.rs` awaits `run()` inline and the executor and worker service both need its gRPC port,
// so a campaign there would hang the whole dev stack with no error at all. Only `launch.rs`
// hard-wiring SQLite keeps that unreachable today; the guard is what makes it structural.
async fn distributed_mode_is_refused_in_the_embedded_binary(etcd: &Arc<DockerEtcd>) {
    let config = distributed_config(etcd);
    let mut join_set: JoinSet<anyhow::Result<()>> = JoinSet::new();

    let result = golem_shard_manager::run(
        &config,
        Deployment::Embedded,
        prometheus::Registry::new(),
        &mut join_set,
    )
    .await;
    join_set.abort_all();

    let err = result
        .err()
        .expect("etcd persistence must be refused in the embedded binary, not silently campaign");
    let message = format!("{err:#}");
    assert!(
        message.contains("embedded"),
        "The refusal should explain why the embedded binary cannot campaign, but was: {message}"
    );
}

/// Starts a shard manager that is expected to be refused, and returns why.
///
/// Bounded by `REFUSAL_TIMEOUT`: a config caught only after the campaign would otherwise block
/// here behind the current leader.
async fn startup_refusal(config: &ShardManagerConfig) -> String {
    let mut join_set: JoinSet<anyhow::Result<()>> = JoinSet::new();
    let result = timeout(
        REFUSAL_TIMEOUT,
        golem_shard_manager::run(
            config,
            Deployment::Standalone {
                shutdown: CancellationToken::new(),
            },
            prometheus::Registry::new(),
            &mut join_set,
        ),
    )
    .await
    .expect("A configuration this broken must be refused before the campaign, not after it");
    join_set.abort_all();

    let err = result
        .err()
        .expect("The configuration should be refused at startup");
    format!("{err:#}")
}

#[test]
#[tracing::instrument(skip_all)]
// etcd's lease TTL has one-second resolution and a 2s floor, so a finer or smaller value is
// silently truncated or clamped and the operator gets a failover time they did not configure.
async fn an_unusable_leader_lease_ttl_is_refused_at_startup(etcd: &Arc<DockerEtcd>) {
    for (ttl, expected) in [
        (Duration::from_millis(1500), "whole number of seconds"),
        (Duration::from_secs(1), "at least 2s"),
    ] {
        let mut config = distributed_config(etcd);
        config.persistence = PersistenceConfig::Etcd(EtcdConfig {
            endpoints: vec![etcd.client_url()],
            leader_lease_ttl: ttl,
            request_timeout: REQUEST_TIMEOUT,
            ..EtcdConfig::default()
        });

        let message = startup_refusal(&config).await;
        assert!(
            message.contains(expected),
            "The refusal for a {ttl:?} leader lease TTL should explain `{expected}`, but was: \
             {message}"
        );
    }
}

#[test]
#[tracing::instrument(skip_all)]
// A campaign grants its lease then opens the keepalive, each bounded by `request_timeout`, so a
// timeout that two of could outlast the TTL expires the lease before it is ever renewed.
async fn a_request_timeout_that_could_outlast_the_lease_is_refused_at_startup(
    etcd: &Arc<DockerEtcd>,
) {
    let mut config = distributed_config(etcd);
    config.persistence = PersistenceConfig::Etcd(EtcdConfig {
        endpoints: vec![etcd.client_url()],
        leader_lease_ttl: Duration::from_secs(10),
        request_timeout: Duration::from_secs(6),
        ..EtcdConfig::default()
    });

    let message = startup_refusal(&config).await;
    assert!(
        message.contains("request_timeout"),
        "The refusal should name `request_timeout`, or the operator is left to guess which of the \
         two durations to change. Was: {message}"
    );
}

#[test]
#[tracing::instrument(skip_all)]
// Every replica shares the misconfiguration, so catching it after the campaign would have each
// winner exit still holding the lease - a deployment-wide crash loop at lease cadence.
async fn a_shard_count_mismatch_is_refused_before_campaigning(etcd: &Arc<DockerEtcd>) {
    wipe_state(etcd).await;

    let (leader, leader_ports) = spawn_replica(distributed_config(etcd), CancellationToken::new());
    timeout(STARTUP_TIMEOUT, leader_ports)
        .await
        .expect("The first replica should win an uncontested campaign and finish starting")
        .expect("The leader's startup should report its ports");
    await_state_written(etcd).await;

    let mismatched_shards = NUMBER_OF_SHARDS / 2;
    let mut config = distributed_config(etcd);
    config.number_of_shards = mismatched_shards;

    let mut join_set: JoinSet<anyhow::Result<()>> = JoinSet::new();
    let result = timeout(
        REFUSAL_TIMEOUT,
        golem_shard_manager::run(
            &config,
            Deployment::Standalone {
                shutdown: CancellationToken::new(),
            },
            prometheus::Registry::new(),
            &mut join_set,
        ),
    )
    .await;
    join_set.abort_all();

    // The timing is the assertion: the leader below is still holding the leadership, so a replica
    // that only checked after being elected would still be campaigning when this fires.
    let result = result.expect(
        "The mismatched replica neither started nor failed within the refusal timeout, which means \
         it reached the campaign and is blocked behind the leader. The shard count must be checked \
         before campaigning, not after.",
    );

    let err = result
        .err()
        .expect("A shard count that disagrees with the stored state must be refused at startup");
    let message = format!("{err:#}");
    assert!(
        message.contains(&format!("configured for {mismatched_shards}")),
        "The refusal should name the configured shard count it disagreed with, but was: {message}"
    );

    assert!(
        !leader.is_finished(),
        "The leader stopped during the test, so the refusal above may have been uncontested."
    );
    leader.abort();
}

#[test]
#[tracing::instrument(skip_all)]
// Nothing drains the JoinSet until `run()` returns, and a standby's never does, so a task that
// dies during the campaign would leave the replica alive, unobservable and campaigning forever.
async fn a_task_that_dies_during_the_campaign_fails_startup(etcd: &Arc<DockerEtcd>) {
    wipe_state(etcd).await;
    let config = distributed_config(etcd);

    let (leader, leader_ports) = spawn_replica(config.clone(), CancellationToken::new());
    timeout(STARTUP_TIMEOUT, leader_ports)
        .await
        .expect("The first replica should win an uncontested campaign and finish starting")
        .expect("The leader's startup should report its ports");
    let already_queued = campaigner_keys(etcd).await;

    let mut join_set: JoinSet<anyhow::Result<()>> = JoinSet::new();
    // Fails only once the standby is already inside the campaign.
    join_set.spawn(async {
        tokio::time::sleep(Duration::from_millis(500)).await;
        Err(anyhow!("simulated background task failure"))
    });

    let result = timeout(
        REFUSAL_TIMEOUT,
        golem_shard_manager::run(
            &config,
            Deployment::Standalone {
                shutdown: CancellationToken::new(),
            },
            prometheus::Registry::new(),
            &mut join_set,
        ),
    )
    .await;
    join_set.abort_all();
    leader.abort();

    let err = result
        .expect(
            "The standby kept campaigning after one of its tasks failed, so the failure would go \
             unnoticed for as long as another replica held the leadership.",
        )
        .err()
        .expect("A task that failed during the campaign must fail startup");
    let message = format!("{err:#}");
    assert!(
        message.contains("while campaigning for leadership"),
        "The failure should say that it happened during the campaign, but was: {message}"
    );
    assert!(
        message.contains("simulated background task failure"),
        "The failure should carry the original task error as its cause, but was: {message}"
    );

    timeout(Duration::from_millis(500), async {
        while campaigner_keys(etcd)
            .await
            .difference(&already_queued)
            .next()
            .is_some()
        {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect(
        "The abandoned campaign left its key in etcd's queue on a lease nothing renews, so the \
         next election waits out that lease behind a replica that never started.",
    );
}

#[test]
#[tracing::instrument(skip_all)]
// etcd's election is a FIFO queue of one key per campaigner, each on that campaigner's lease, so
// a standby that leaves its key behind makes the next election wait a full TTL on a dead replica.
async fn a_standby_that_is_shut_down_leaves_the_campaign_immediately(etcd: &Arc<DockerEtcd>) {
    wipe_state(etcd).await;
    let config = distributed_config(etcd);

    let (leader, leader_ports) = spawn_replica(config.clone(), CancellationToken::new());
    timeout(STARTUP_TIMEOUT, leader_ports)
        .await
        .expect("The first replica should win an uncontested campaign and finish starting")
        .expect("The leader's startup should report its ports");

    let already_queued = campaigner_keys(etcd).await;

    let standby_shutdown = CancellationToken::new();
    // Not `spawn_replica`, which panics on a failed start: the returned error is under test.
    let standby = tokio::spawn({
        let config = config.clone();
        let shutdown = standby_shutdown.clone();
        async move {
            let mut join_set: JoinSet<anyhow::Result<()>> = JoinSet::new();
            let result = golem_shard_manager::run(
                &config,
                Deployment::Standalone { shutdown },
                prometheus::Registry::new(),
                &mut join_set,
            )
            .await;
            join_set.abort_all();
            result.map(|_| ())
        }
    });

    let standby_key = timeout(STARTUP_TIMEOUT, async {
        loop {
            let queued = campaigner_keys(etcd).await;
            if let Some(key) = queued.difference(&already_queued).next() {
                return key.clone();
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("The standby should enqueue a campaign key under the election prefix");

    standby_shutdown.cancel();

    let err = timeout(Duration::from_secs(5), standby)
        .await
        .expect(
            "The standby was still campaigning five seconds after it was asked to stop. Nothing \
             inside the campaign observes the shutdown, so a pod being rolled would sit there \
             until its kill grace period ran out and it was killed outright.",
        )
        .expect("The standby task should not panic")
        .expect_err("A standby that was asked to stop must report that, not finish starting");
    assert!(
        matches!(
            err.downcast_ref::<ShardManagerError>(),
            Some(ShardManagerError::ShutdownRequested)
        ),
        "The standby ended with `{err:#}` rather than a shutdown, so `server.rs` would report a \
         clean stop as a startup failure and exit non-zero."
    );

    // Revoked, not expired: at a two-second lease nothing could have lapsed inside this window.
    timeout(Duration::from_millis(500), async {
        while campaigner_keys(etcd).await.contains(&standby_key) {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect(
        "The standby's campaign key outlived its shutdown. It is still parked in etcd's FIFO \
         queue on a lease nothing renews, so the next election waits out that lease behind a \
         replica that has already exited.",
    );

    leader.abort();
}

/// Waits until nothing at all is queued under the production election prefix.
///
/// Both precondition and assertion below: as a precondition it waits out earlier tests' abandoned
/// leases; at `REVOKE_WINDOW` only a revoke can have emptied the prefix in time.
async fn await_election_empty(etcd: &DockerEtcd, within: Duration, complaint: &str) {
    let emptied = timeout(within, async {
        while !campaigner_keys(etcd).await.is_empty() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await;

    assert!(emptied.is_ok(), "{complaint}");
}

/// Waits out the leases of any campaigner an earlier test left behind.
async fn await_election_idle(etcd: &DockerEtcd) {
    await_election_empty(
        etcd,
        LEASE_TTL * 3,
        "A campaigner from an earlier test is still queued, so this test cannot assume that the \
         replica it starts is the only one in the election.",
    )
    .await;
}

/// Wins the leadership in this task, and hands back what `server.rs` would then drain.
async fn start_leader(
    config: &ShardManagerConfig,
    shutdown: CancellationToken,
) -> (RunDetails, JoinSet<anyhow::Result<()>>) {
    start_leader_within(config, shutdown, STARTUP_TIMEOUT).await
}

/// [`start_leader`] for the one replica whose startup is legitimately slow.
async fn start_leader_within(
    config: &ShardManagerConfig,
    shutdown: CancellationToken,
    within: Duration,
) -> (RunDetails, JoinSet<anyhow::Result<()>>) {
    let mut join_set: JoinSet<anyhow::Result<()>> = JoinSet::new();
    let details = timeout(
        within,
        golem_shard_manager::run(
            config,
            Deployment::Standalone { shutdown },
            prometheus::Registry::new(),
            &mut join_set,
        ),
    )
    .await
    .expect("An uncontested campaign should finish starting well inside the startup timeout")
    .expect("The replica should win an uncontested campaign and finish starting");

    (details, join_set)
}

/// Stores a shard lease state holding one executor at `addr`.
///
/// The fence is a real key read back at its real creation revision: the guarded write compares
/// that revision, so a stub fence would be refused.
async fn write_state_with_executor(etcd: &DockerEtcd, addr: ExecutorAddr) {
    let mut kv = etcd_client::Client::connect([etcd.client_url()], None)
        .await
        .expect("Cannot connect to etcd")
        .kv_client();

    let leader_key = format!("/golem/test/leader/{}", Uuid::new_v4());
    kv.put(leader_key.clone(), "test-leader", None)
        .await
        .expect("Cannot put the sentinel leader key");
    let created = kv
        .get(leader_key.clone(), None)
        .await
        .expect("Cannot read the sentinel leader key back")
        .kvs()
        .first()
        .expect("The sentinel leader key should exist immediately after being written")
        .create_revision();

    let persistence = EtcdRoutingTablePersistence::new(
        &etcd_config(etcd),
        NUMBER_OF_SHARDS,
        LeaderFence::for_test(leader_key, created),
    )
    .await
    .expect("Cannot connect to etcd");

    let mut shard_state = ShardLeaseState::new(NUMBER_OF_SHARDS);
    shard_state.add_executor(
        ExecutorId(Uuid::from_u128(1)),
        addr,
        None,
        DateTime::from_timestamp(1_700_000_000, 0).expect("valid timestamp"),
        LEASE_TTL,
    );
    shard_state
        .bump_revision()
        .expect("The revision should advance");

    persistence
        .write(&shard_state, NO_REVISION)
        .await
        .expect("Cannot store the shard lease state");
}

#[test]
#[tracing::instrument(skip_all)]
// Winning and then failing leaves the deployment down twice over: this replica is gone but its
// lease still names it the leader, and every replica shares the configuration that failed.
async fn a_startup_failure_after_winning_releases_the_leadership(etcd: &Arc<DockerEtcd>) {
    wipe_state(etcd).await;
    await_election_idle(etcd).await;

    // Wildcard rather than 127.0.0.1, which is what `run()` binds: on macOS a loopback listener
    // does not collide with a wildcard bind and the startup would simply succeed.
    let occupied = TcpListener::bind((Ipv4Addr::UNSPECIFIED, 0))
        .await
        .expect("Cannot occupy a port for the test");
    let occupied_port = occupied
        .local_addr()
        .expect("The occupied listener should report its port")
        .port();

    let mut config = distributed_config(etcd);
    config.grpc.port = occupied_port;

    let mut join_set: JoinSet<anyhow::Result<()>> = JoinSet::new();
    let result = timeout(
        STARTUP_TIMEOUT,
        golem_shard_manager::run(
            &config,
            Deployment::Standalone {
                shutdown: CancellationToken::new(),
            },
            prometheus::Registry::new(),
            &mut join_set,
        ),
    )
    .await
    .expect("The replica should fail at its gRPC bind rather than hang");
    join_set.abort_all();

    let err = result
        .err()
        .expect("Binding a port that is already taken must fail startup");
    let message = format!("{err:#}");
    assert!(
        message.contains("in use"),
        "The startup should have failed at the gRPC bind, which is the failure this test arranges \
         to happen after the campaign. Anything else means the replica never got that far, and \
         what follows would prove nothing. Was: {message}"
    );

    await_election_empty(
        etcd,
        REVOKE_WINDOW,
        "The failed replica exited still holding its leadership lease. It is gone, but etcd still \
         names it the leader, so no standby can be elected until the lease expires.",
    )
    .await;
}

#[test]
#[tracing::instrument(skip_all)]
// A failing background task takes the process down; without a step-down on that path the lease
// outlives the replica and the deployment pays a full TTL with no routing table.
async fn a_failing_task_releases_the_leadership(etcd: &Arc<DockerEtcd>) {
    wipe_state(etcd).await;
    await_election_idle(etcd).await;

    let shutdown = CancellationToken::new();
    let (details, mut join_set) = start_leader(&distributed_config(etcd), shutdown.clone()).await;

    join_set.spawn(async {
        tokio::time::sleep(Duration::from_millis(300)).await;
        Err(anyhow!("simulated task failure"))
    });

    let err = timeout(
        Duration::from_secs(2),
        golem_shard_manager::serve_until_stopped(details, join_set, shutdown),
    )
    .await
    .expect("A failed task should stop the leader promptly")
    .expect_err("A task that failed must stop the leader, not be swallowed");
    assert!(
        format!("{err:#}").contains("simulated task failure"),
        "The failure should carry the original task error as its cause, but was: {err:#}"
    );

    await_election_empty(
        etcd,
        REVOKE_WINDOW,
        "The leader exited on a failed task still holding its leadership lease, so the standby \
         that should take over immediately waits out the lease instead.",
    )
    .await;
}

#[test]
#[tracing::instrument(skip_all)]
// A SIGTERM between winning the campaign and serving. Unobserved, the initial health check runs
// its whole retry budget and a rolled pod holds the leadership to its kill grace period.
async fn a_shutdown_during_the_initial_health_check_exits_cleanly(etcd: &Arc<DockerEtcd>) {
    wipe_state(etcd).await;
    await_election_idle(etcd).await;

    // Accepts and then says nothing, so each health check attempt runs out its timeout: an
    // unroutable address is refused in milliseconds and startup would be past the check already.
    let silent_executor = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .await
        .expect("Cannot bind the stand-in executor");
    let silent_addr = silent_executor
        .local_addr()
        .expect("The stand-in executor should report its address");
    write_state_with_executor(
        etcd,
        ExecutorAddr {
            ip: silent_addr.ip(),
            port: silent_addr.port(),
        },
    )
    .await;

    let shutdown = CancellationToken::new();
    let replica = tokio::spawn({
        let config = distributed_config(etcd);
        let shutdown = shutdown.clone();
        async move {
            let mut join_set: JoinSet<anyhow::Result<()>> = JoinSet::new();
            let result = golem_shard_manager::run(
                &config,
                Deployment::Standalone { shutdown },
                prometheus::Registry::new(),
                &mut join_set,
            )
            .await;
            join_set.abort_all();
            result.map(|_| ())
        }
    });

    // The dialled health check is the only signal that places the replica past the campaign: etcd
    // names a leader before the campaign call returns, so a shutdown sent on that would land
    // inside it. The connection is held for the rest of the test, so the attempt stays stuck.
    let _dialled = timeout(STARTUP_TIMEOUT, silent_executor.accept())
        .await
        .expect("The elected replica should reach its initial health check")
        .expect("Cannot accept the health check connection");

    shutdown.cancel();

    let err = timeout(Duration::from_secs(3), replica)
        .await
        .expect(
            "The replica was still starting three seconds after it was asked to stop. Nothing \
             between winning the campaign and serving observes the shutdown, so it sits out the \
             health check's whole retry budget holding the leadership.",
        )
        .expect("The replica task should not panic")
        .expect_err("A replica asked to stop mid-startup must report that, not finish starting");
    assert!(
        matches!(
            err.downcast_ref::<ShardManagerError>(),
            Some(ShardManagerError::ShutdownRequested)
        ),
        "The replica ended with `{err:#}` rather than a shutdown, so `server.rs` would report a \
         clean stop as a startup failure and exit non-zero."
    );

    await_election_empty(
        etcd,
        REVOKE_WINDOW,
        "The interrupted replica exited still holding its leadership lease, so the standby that \
         should take over immediately waits out the lease instead.",
    )
    .await;
}

#[test]
#[tracing::instrument(skip_all)]
// The lease may already be gone when a shutdown goes to release it, and nothing is left behind if
// it is, so failing the exit over the revoke would restart a pod that stopped correctly.
async fn a_step_down_whose_lease_is_already_gone_still_exits_cleanly(etcd: &Arc<DockerEtcd>) {
    await_election_idle(etcd).await;

    let election = LeaderElection::connect(&etcd_config(etcd), LEADER_ELECTION_NAME)
        .await
        .expect("Cannot connect to etcd");
    let elected = timeout(STARTUP_TIMEOUT, election.campaign_until_elected())
        .await
        .expect("An uncontested campaign should be won promptly")
        .expect("The campaign should succeed");

    // Campaigned for directly rather than through `run()`: nothing renews this lease, so the
    // revoke cannot race a keepalive noticing the loss first.
    etcd_client::Client::connect([etcd.client_url()], None)
        .await
        .expect("Cannot connect to etcd")
        .lease_client()
        .revoke(elected.lease_id)
        .await
        .expect("Cannot revoke the lease out of band");

    let details = RunDetails {
        http_port: 0,
        grpc_port: 0,
        leadership: Some(elected.leadership),
    };

    let mut join_set: JoinSet<anyhow::Result<()>> = JoinSet::new();
    join_set.spawn(std::future::pending::<anyhow::Result<()>>());

    let shutdown = CancellationToken::new();
    shutdown.cancel();

    timeout(
        Duration::from_secs(5),
        golem_shard_manager::serve_until_stopped(details, join_set, shutdown),
    )
    .await
    .expect("The shutdown should not hang on a lease that is already gone")
    .expect(
        "A shutdown whose lease had already gone was reported as a failure. There was nothing left \
         to release, so the process exits non-zero for having done exactly what was asked of it.",
    );
}

#[test]
#[tracing::instrument(skip_all)]
// Nothing in the JoinSet is meant to finish, but one that does is not a reason to take the leader
// down: the deployment would pay a failover for a task that ended without an error.
async fn a_task_that_finishes_cleanly_does_not_stop_the_leader(etcd: &Arc<DockerEtcd>) {
    wipe_state(etcd).await;
    await_election_idle(etcd).await;

    let shutdown = CancellationToken::new();
    let (details, mut join_set) = start_leader(&distributed_config(etcd), shutdown.clone()).await;

    join_set.spawn(async {
        tokio::time::sleep(Duration::from_millis(200)).await;
        Ok(())
    });

    let serving = tokio::spawn(golem_shard_manager::serve_until_stopped(
        details,
        join_set,
        shutdown.clone(),
    ));

    // Well past the finished task. Both assertions are needed: a leader that stopped without
    // standing down still holds its lease, so the election prefix alone would not notice.
    tokio::time::sleep(Duration::from_millis(700)).await;
    assert!(
        !serving.is_finished(),
        "The leader stopped when a background task finished cleanly, so a task that simply ran out \
         of work takes the whole replica down."
    );
    assert!(
        !campaigner_keys(etcd).await.is_empty(),
        "The leader stood down when a background task finished cleanly, so a task that simply ran \
         out of work costs the deployment a failover."
    );

    shutdown.cancel();
    timeout(Duration::from_secs(5), serving)
        .await
        .expect("The leader should stop promptly once it is asked to")
        .expect("The serving task should not panic")
        .expect("A requested shutdown is a clean stop");
}

/// Refuses connections rather than black-holing them: tonic keeps a refusing member in the
/// balancer's rotation and fails fast, where a black hole would be waited out instead.
const DEAD_MEMBER: &str = "http://127.0.0.1:1";

/// Reads made after startup, through a client configured exactly like the one `run()` uses.
const PROBE_READS: usize = 20;

/// Far more generous than [`STARTUP_TIMEOUT`]: the campaign's own requests meet the dead member
/// too, and each retry backs off up to five seconds.
const DEAD_MEMBER_STARTUP_TIMEOUT: Duration = Duration::from_secs(120);

#[test]
#[tracing::instrument(skip_all)]
// One member of an etcd cluster is down routinely, and tonic keeps it in the balancer's rotation,
// so a share of requests fail fast. Single-shot, the pre-campaign read would turn a member being
// restarted into a replica that never starts and never campaigns.
async fn a_dead_member_in_the_endpoint_list_does_not_fail_startup(etcd: &Arc<DockerEtcd>) {
    wipe_state(etcd).await;
    await_election_idle(etcd).await;

    let endpoints = vec![etcd.client_url(), DEAD_MEMBER.to_string()];
    let mut config = distributed_config(etcd);
    config.persistence = PersistenceConfig::Etcd(EtcdConfig {
        endpoints: endpoints.clone(),
        ..etcd_config(etcd)
    });

    let shutdown = CancellationToken::new();
    let (details, mut join_set) =
        start_leader_within(&config, shutdown.clone(), DEAD_MEMBER_STARTUP_TIMEOUT).await;
    assert_ne!(
        details.grpc_port, 0,
        "The replica reported no gRPC port, so it never finished starting."
    );

    // Built the way `connect_for_requests` does, so what is measured is the endpoint list and not
    // a client this test invented.
    let probe = etcd_client::Client::connect(
        &endpoints,
        Some(
            etcd_client::ConnectOptions::new()
                .with_connect_timeout(EtcdConfig::default().connect_timeout)
                .with_timeout(REQUEST_TIMEOUT),
        ),
    )
    .await
    .expect("Cannot connect a probe client to the endpoint list");

    // Reported, not asserted: how often the balancer picks the dead member is tonic's business.
    // On tonic 0.14 it was 5 of 20, which is what makes the retries below matter.
    let mut single_shot_failures = 0;
    for _ in 0..PROBE_READS {
        if EtcdRoutingTablePersistence::stored_number_of_shards(&probe)
            .await
            .is_err()
        {
            single_shot_failures += 1;
        }
    }
    info!(
        failed = single_shot_failures,
        of = PROBE_READS,
        "Single-shot reads through an endpoint list with one dead member"
    );

    // The retrying read the leader's worker depends on: a failed read is a fail-stop, so it has
    // to survive what the single-shot reads above do not. A read never consults the fence.
    let persistence = EtcdRoutingTablePersistence::with_client(
        probe,
        NUMBER_OF_SHARDS,
        LeaderFence::for_test("/golem/test/unread-by-a-read", 1),
    );
    for read in 0..PROBE_READS {
        persistence.read().await.unwrap_or_else(|err| {
            panic!(
                "Read {read} of {PROBE_READS} through the endpoint list failed: {err}. \
                 {single_shot_failures} of {PROBE_READS} single-shot reads over the same list \
                 failed, so this read met a dead member and gave up on it - and in the leader's \
                 worker that is a fail-stop over an etcd member being restarted."
            )
        });
    }

    // Not stepped down: the revoke is single-shot on the lease client and would meet the dead
    // member as often as the reads did. Waiting the lease out clears the election either way.
    join_set.abort_all();
    await_election_empty(
        etcd,
        LEASE_TTL * 3,
        "The leadership lease of a replica whose tasks were aborted has not lapsed, so the next \
         test would queue behind it.",
    )
    .await;
}

/// How long the black hole below lasts: four request timeouts, so several attempts are certain to
/// have met it and given up.
const STALL: Duration = Duration::from_millis(2000);

#[test]
#[tracing::instrument(skip_all)]
// Compaction, a disk stall or etcd's own raft election makes a GET slow rather than impossible.
// `etcd-client` turns `request_timeout` into a tonic endpoint timeout, and tonic reports an
// expired one as `Cancelled`, which this workspace otherwise classifies as fatal - so a slow
// answer would fail-stop the leader's worker exactly as a lost etcd would.
async fn a_read_through_a_stalled_etcd_survives_it(etcd: &Arc<DockerEtcd>) {
    let proxy = BreakableProxy::start(&etcd.client_url()).await;

    // Built the way `connect_for_requests` does: the endpoint timeout is the whole point here, so
    // a client without one would test nothing.
    let client = etcd_client::Client::connect(
        &[proxy.url()],
        Some(
            etcd_client::ConnectOptions::new()
                .with_connect_timeout(EtcdConfig::default().connect_timeout)
                .with_timeout(REQUEST_TIMEOUT),
        ),
    )
    .await
    .expect("Cannot connect a client through the proxy");

    let persistence = EtcdRoutingTablePersistence::with_client(
        client,
        NUMBER_OF_SHARDS,
        LeaderFence::for_test("/golem/test/unread-by-a-read", 1),
    );
    persistence
        .read()
        .await
        .expect("A read through an unbroken proxy should succeed");

    // Held rather than closed: the client sees no fault, only an answer that never comes, so the
    // endpoint timeout is what ends the attempt.
    proxy.black_hole();
    let stalled_at = Instant::now();

    // Joined rather than sequenced: the read has to be in flight *while* the black hole lasts, or
    // it would only ever make its first attempt against an etcd that was already answering again.
    let (read, ()) = tokio::join!(
        timeout(STALL * 5, async {
            let result = persistence.read().await;
            (result, stalled_at.elapsed())
        }),
        async {
            tokio::time::sleep(STALL).await;
            proxy.restore();
        }
    );

    let (result, waited) =
        read.expect("A read that outlives five stalls is not retrying, it is stuck");
    result.unwrap_or_else(|err| {
        panic!(
            "A read across a {STALL:?} stall failed with `{err}` instead of waiting it out. The \
             leader's worker fail-stops on a failed read, so an etcd that was slow for two seconds \
             would take the leader down with it."
        )
    });
    assert!(
        waited >= STALL,
        "The read returned after {waited:?}, inside the {STALL:?} stall - so it was answered by \
         something other than the etcd behind the proxy, and this test is measuring nothing."
    );
}

/// The current `shard_manager_is_leader` reading, or `None` while the gauge has never been set.
///
/// The leadership metrics live on the process-wide default registry, not the one `run()` is
/// handed; the reading is this replica's only because this suite runs sequentially.
fn is_leader_gauge() -> Option<f64> {
    prometheus::default_registry()
        .gather()
        .into_iter()
        .find(|family| family.name() == IS_LEADER_METRIC)
        .and_then(|family| {
            family
                .get_metric()
                .first()
                .and_then(|metric| metric.get_gauge().as_ref())
                .map(|gauge| gauge.value())
        })
}

#[test]
#[tracing::instrument(skip_all)]
// `sum(shard_manager_is_leader) != 1` is the alert this gauge exists for, so a replica that steps
// down still exporting 1 hides the very state the alert is meant to catch.
async fn the_leader_gauge_drops_on_step_down(etcd: &Arc<DockerEtcd>) {
    wipe_state(etcd).await;
    await_election_idle(etcd).await;

    let shutdown = CancellationToken::new();
    let (details, mut join_set) = start_leader(&distributed_config(etcd), shutdown.clone()).await;

    assert_eq!(
        is_leader_gauge(),
        Some(1.0),
        "A replica that won the campaign and finished starting is not exporting itself as the \
         leader, so an alert on the sum reads a healthy deployment as leaderless."
    );

    // The keepalive resets the same gauge once it notices the lease go, so it is stopped first
    // and the reading below reflects the step-down rather than a race with it.
    join_set.abort_all();

    details
        .leadership
        .as_ref()
        .expect("A leader in distributed mode holds a leadership handle")
        .step_down()
        .await
        .expect("A leader should be able to release its own lease");

    assert_eq!(
        is_leader_gauge(),
        Some(0.0),
        "The replica stood down still exporting itself as the leader, so the deployment looks led \
         by a replica that has already handed the leadership back."
    );
}

/// The one campaigner key currently queued, with the lease it hangs from.
///
/// Read out of etcd rather than from the `LeadershipHandle`, which does not expose its lease: the
/// revoke has to arrive from outside, the way a failing etcd would.
async fn sole_campaigner(etcd: &DockerEtcd) -> (Vec<u8>, i64) {
    let response = etcd_client::Client::connect([etcd.client_url()], None)
        .await
        .expect("Cannot connect to etcd")
        .kv_client()
        .get(
            format!("{LEADER_ELECTION_NAME}/"),
            Some(etcd_client::GetOptions::new().with_prefix()),
        )
        .await
        .expect("Cannot read the election prefix");

    let keys = response.kvs();
    assert_eq!(
        keys.len(),
        1,
        "Expected exactly one campaigner queued, found {}.",
        keys.len()
    );
    (keys[0].key().to_vec(), keys[0].lease())
}

#[test]
#[tracing::instrument(skip_all)]
// The lease is the only thing making this replica the leader, so losing it has to stop it rather
// than log: a keepalive error that never reaches the drain leaves a second shard manager serving
// a routing table while a standby has already been elected.
async fn an_elected_leader_that_loses_its_lease_stops(etcd: &Arc<DockerEtcd>) {
    wipe_state(etcd).await;
    await_election_idle(etcd).await;

    let shutdown = CancellationToken::new();
    let (details, join_set) = start_leader(&distributed_config(etcd), shutdown.clone()).await;
    let (leader_key, lease_id) = sole_campaigner(etcd).await;

    etcd_client::Client::connect([etcd.client_url()], None)
        .await
        .expect("Cannot connect to etcd")
        .lease_client()
        .revoke(lease_id)
        .await
        .expect("Revoking the leader's lease out of band should succeed");
    let revoked_at = Instant::now();

    // etcd answers a keepalive stream, it does not push to it, so the loss can only be noticed on
    // the next renewal - one interval away at worst. The extra second is transport and the drain.
    let renewal_interval = LeaseKeepAlive::renewal_interval(LEASE_TTL);
    let budget = renewal_interval + Duration::from_secs(1);
    let stopped = timeout(
        budget,
        golem_shard_manager::serve_until_stopped(details, join_set, shutdown),
    )
    .await
    .unwrap_or_else(|_| {
        panic!(
            "The replica was still serving {budget:?} after its leadership lease was revoked. Its \
             lease is gone, so a standby can already have been elected, and every command it \
             sends an executor from here on is a second shard manager driving the same topology."
        )
    });
    let noticed = revoked_at.elapsed();

    let err = stopped.expect_err(
        "A replica that lost its leadership lease must stop with an error, not report \
                     an orderly shutdown - the process has to restart so a standby can take over",
    );
    // Two paths notice the loss, and which wins is a race with the startup pass's own writes: the
    // keepalive's next renewal is answered "lease gone", or a fenced write is refused. Either is
    // the leader stopping over the lease it lost.
    let leader_key_str = String::from_utf8_lossy(leader_key.as_ref()).into_owned();
    let noticed_by = err
        .chain()
        .find_map(|cause| {
            if let Some(lost) = cause.downcast_ref::<LeaseLost>() {
                assert_eq!(
                    lost.lease_id, lease_id,
                    "The replica reported losing lease {:#x}, but the leadership it held was on \
                     {lease_id:#x}.",
                    lost.lease_id
                );
                return Some(format!("keepalive: {}", lost.reason));
            }
            if let Some(ShardManagerError::LeadershipLost {
                leader_key: key, ..
            }) = cause.downcast_ref::<ShardManagerError>()
            {
                assert_eq!(
                    *key, leader_key_str,
                    "The fenced write was refused for a different election key than the one this \
                     leader held."
                );
                return Some("fenced write".to_string());
            }
            None
        })
        .unwrap_or_else(|| panic!("The replica stopped, but not over the lease it lost: {err:#}"));
    info!(
        ?noticed,
        noticed_by, "The leader stopped after losing its lease"
    );

    assert!(
        !campaigner_keys(etcd).await.contains(&leader_key),
        "The revoked leader's election key is still queued, so the deployment has no leader and \
         the key of the replica that cannot be one is what the next campaigner waits behind."
    );
}

/// Starts a replica and keeps it serving until its token is cancelled, the way `server.rs` does,
/// reporting its gRPC port once it is elected and started.
///
/// Unlike [`spawn_replica`] the drain is the production one, so cancelling exercises the
/// step-down as well as the stop.
fn spawn_serving_replica(
    config: ShardManagerConfig,
    shutdown: CancellationToken,
) -> (JoinHandle<anyhow::Result<()>>, oneshot::Receiver<u16>) {
    let (started, elected) = oneshot::channel();

    let handle = tokio::spawn(async move {
        let mut join_set: JoinSet<anyhow::Result<()>> = JoinSet::new();
        let details = match golem_shard_manager::run(
            &config,
            Deployment::Standalone {
                shutdown: shutdown.clone(),
            },
            prometheus::Registry::new(),
            &mut join_set,
        )
        .await
        {
            Ok(details) => details,
            Err(err) => {
                join_set.abort_all();
                panic!("The replica failed to start: {err:#}");
            }
        };
        let _ = started.send(details.grpc_port);
        golem_shard_manager::serve_until_stopped(details, join_set, shutdown).await
    });

    (handle, elected)
}

/// Waits until exactly `count` campaigners are queued.
///
/// etcd hands the leadership on in creation order, so starting the next replica only once the
/// previous one's key exists is what makes the queue order known.
async fn await_campaigners(etcd: &DockerEtcd, count: usize) {
    timeout(STARTUP_TIMEOUT, async {
        while campaigner_keys(etcd).await.len() != count {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap_or_else(|_| {
        panic!("Only {count} campaigners should be queued, and they never were");
    });
}

/// What a handover may cost. Under `LEASE_TTL - renewal_interval`, the soonest an unrevoked
/// leadership can free up, so a step-down that does not revoke cannot pass by being fast.
const HANDOVER_BUDGET: Duration = Duration::from_secs(1);

#[test]
#[tracing::instrument(skip_all)]
// Every pod of a rolling restart that goes down without handing the leadership back costs a full
// TTL with no routing table. Two handovers rather than one, because a replica has to be able to
// give up a leadership it was promoted into rather than started with.
async fn a_rolling_restart_hands_over_without_a_gap(etcd: &Arc<DockerEtcd>) {
    wipe_state(etcd).await;
    await_election_idle(etcd).await;
    let config = distributed_config(etcd);

    let first = CancellationToken::new();
    let (first_replica, first_started) = spawn_serving_replica(config.clone(), first.clone());
    timeout(STARTUP_TIMEOUT, first_started)
        .await
        .expect("The first replica should win an uncontested campaign")
        .expect("The first replica's startup should report its ports");

    // Queued one at a time, so `second` is ahead of `third` in etcd's FIFO order.
    let second = CancellationToken::new();
    let (second_replica, second_started) = spawn_serving_replica(config.clone(), second.clone());
    await_campaigners(etcd, 2).await;
    let third = CancellationToken::new();
    let (third_replica, third_started) = spawn_serving_replica(config.clone(), third.clone());
    await_campaigners(etcd, 3).await;

    let restarted_at = Instant::now();
    first.cancel();
    timeout(HANDOVER_BUDGET, second_started)
        .await
        .unwrap_or_else(|_| {
            panic!(
                "The second replica was not serving {HANDOVER_BUDGET:?} after the leader was asked \
                 to stop. A leader that stops without revoking its lease leaves the deployment \
                 with no routing table until the lease lapses, so a rolling restart of three pods \
                 costs three lease TTLs of downtime."
            )
        })
        .expect("The promoted replica's startup should report its ports");
    let first_handover = restarted_at.elapsed();

    timeout(STARTUP_TIMEOUT, first_replica)
        .await
        .expect("The replica that stood down should stop")
        .expect("The replica that stood down should not panic")
        .expect("A replica stopped by its shutdown token exits cleanly, not with an error");

    let promoted_at = Instant::now();
    second.cancel();
    timeout(HANDOVER_BUDGET, third_started)
        .await
        .unwrap_or_else(|_| {
            panic!(
                "The third replica was not serving {HANDOVER_BUDGET:?} after the promoted leader \
                 was asked to stop, so a leadership taken over from a predecessor cannot be handed \
                 on - only the first pod of a rolling restart would be free."
            )
        })
        .expect("The second promoted replica's startup should report its ports");
    let second_handover = promoted_at.elapsed();

    timeout(STARTUP_TIMEOUT, second_replica)
        .await
        .expect("The promoted replica that stood down should stop")
        .expect("The promoted replica that stood down should not panic")
        .expect("A replica stopped by its shutdown token exits cleanly, not with an error");

    // Both predecessors revoked, so what is left is the one replica actually leading. A key left
    // behind is a lease still alive, which is the downtime this test exists to rule out.
    let queued = campaigner_keys(etcd).await;
    assert_eq!(
        queued.len(),
        1,
        "{} campaigner keys are queued after two handovers; only the replica now leading should \
         have one.",
        queued.len()
    );

    info!(
        ?first_handover,
        ?second_handover,
        "Two handovers completed without waiting out a lease"
    );

    third.cancel();
    timeout(STARTUP_TIMEOUT, third_replica)
        .await
        .expect("The last replica should stop")
        .expect("The last replica should not panic")
        .expect("A replica stopped by its shutdown token exits cleanly, not with an error");
}

#[test]
#[tracing::instrument(skip_all)]
// Characterises etcd's queue rather than our code: a standby that dies while queued leaves its
// key for a full TTL, and the replicas behind it wait on that lease rather than on the dead
// replica. Nothing in this crate decides this; it is a bound to notice changing.
async fn a_standby_queue_survives_a_standby_kill(etcd: &Arc<DockerEtcd>) {
    wipe_state(etcd).await;
    await_election_idle(etcd).await;
    let config = distributed_config(etcd);

    let leader_token = CancellationToken::new();
    let (leader, leader_started) = spawn_serving_replica(config.clone(), leader_token.clone());
    timeout(STARTUP_TIMEOUT, leader_started)
        .await
        .expect("The first replica should win an uncontested campaign")
        .expect("The leader's startup should report its ports");

    let doomed_token = CancellationToken::new();
    let (doomed, _doomed_started) = spawn_serving_replica(config.clone(), doomed_token);
    await_campaigners(etcd, 2).await;
    let survivor_token = CancellationToken::new();
    let (survivor, survivor_started) =
        spawn_serving_replica(config.clone(), survivor_token.clone());
    await_campaigners(etcd, 3).await;

    // Aborted, not cancelled: a killed pod revokes nothing, so both leases have to lapse on their
    // own - the queued standby's as well as the leader's.
    let killed_at = Instant::now();
    doomed.abort();
    leader.abort();

    let budget = LEASE_TTL + Duration::from_secs(1);
    timeout(budget, survivor_started)
        .await
        .unwrap_or_else(|_| {
            panic!(
                "The last standby was not elected {budget:?} after both the leader and the standby \
                 ahead of it were killed. Its wait is supposed to be bounded by the {LEASE_TTL:?} \
                 leases those two left behind."
            )
        })
        .expect("The surviving standby's startup should report its ports");
    let failover = killed_at.elapsed();

    let renewal_interval = LeaseKeepAlive::renewal_interval(LEASE_TTL);
    assert!(
        failover >= LEASE_TTL - renewal_interval,
        "The standby was elected {failover:?} after two replicas were killed, sooner than their \
         {LEASE_TTL:?} leases could lapse - so something released a leadership that was supposed \
         to be indistinguishable from a crash."
    );
    info!(?failover, "Failover past a dead standby");

    survivor_token.cancel();
    timeout(STARTUP_TIMEOUT, survivor)
        .await
        .expect("The surviving replica should stop")
        .expect("The surviving replica should not panic")
        .expect("A replica stopped by its shutdown token exits cleanly, not with an error");
}

#[test]
#[tracing::instrument(skip_all)]
// A black-holed connection is neither closed nor slow: nothing in the transport reports it, so
// without the watchdog the replica believes it is the leader while etcd elects somebody else.
// What this adds over the keepalive's own coverage is the wiring - the keepalive has to be spawned
// into the join set the drain watches, or the detected loss is discarded.
async fn the_watchdog_fires_within_one_ttl_on_a_black_hole(etcd: &Arc<DockerEtcd>) {
    wipe_state(etcd).await;
    await_election_idle(etcd).await;

    let proxy = BreakableProxy::start(&etcd.client_url()).await;
    let mut config = distributed_config(etcd);
    config.persistence = PersistenceConfig::Etcd(EtcdConfig {
        endpoints: vec![proxy.url()],
        ..etcd_config(etcd)
    });

    let shutdown = CancellationToken::new();
    let (details, join_set) = start_leader(&config, shutdown.clone()).await;

    // One renewal interval of healthy operation first, so what breaks is a keepalive that was
    // working and the last acknowledged renewal is at most that far back.
    let renewal_interval = LeaseKeepAlive::renewal_interval(LEASE_TTL);
    tokio::time::sleep(renewal_interval).await;

    // No connection is dropped: the sockets stay open and simply stop carrying bytes, which is
    // the case a request timeout cannot see. The deadline is the only thing left that can end it.
    proxy.black_hole();
    let broken_at = Instant::now();

    // Detection is bounded by the lease TTL; the step-down that follows goes into the same black
    // hole and costs one more request timeout before it gives up.
    let budget = LEASE_TTL + REQUEST_TIMEOUT + Duration::from_millis(500);
    let stopped = timeout(
        budget,
        golem_shard_manager::serve_until_stopped(details, join_set, shutdown),
    )
    .await
    .unwrap_or_else(|_| {
        panic!(
            "The replica was still serving {budget:?} after its connection to etcd went silent. \
             etcd has expired its {LEASE_TTL:?} lease and elected somebody else by now, so this \
             replica is a second shard manager driving the same executors."
        )
    });
    let noticed = broken_at.elapsed();

    let err = stopped.expect_err(
        "A replica that can no longer renew its lease must stop with an error rather than report \
         an orderly shutdown",
    );
    let lost = err
        .chain()
        .find_map(|cause| cause.downcast_ref::<LeaseLost>())
        .unwrap_or_else(|| {
            panic!(
                "The replica stopped, but not over the lease it could no longer renew: {err:#}. \
                 Something else failed first, so the watchdog is not what this measured."
            )
        });
    assert_eq!(
        lost.reason,
        LeaseLossReason::RenewalDeadlineExceeded,
        "A silent connection produces no error and no close, so the only thing that may end this \
         lease is the watchdog deadline; `{}` means something else did.",
        lost.reason
    );

    // The other half of the bound: giving the leadership up before the lease could have expired
    // is a failover the deployment did not need.
    let floor = LEASE_TTL - renewal_interval - Duration::from_millis(500);
    assert!(
        noticed >= floor,
        "The leadership was given up {noticed:?} after the connection went silent, sooner than \
         {floor:?}. The lease was still valid, so a reconnect still had time to succeed."
    );
    info!(?noticed, "The leader stopped after a black-holed keepalive");
}

const CAMPAIGN_FAILURES_METRIC: &str = "shard_manager_campaign_attempt_failures_total";

/// The current `shard_manager_campaign_attempt_failures_total` reading, or `None` while the
/// counter has never been registered.
fn campaign_attempt_failures() -> Option<u64> {
    prometheus::default_registry()
        .gather()
        .into_iter()
        .find(|family| family.name() == CAMPAIGN_FAILURES_METRIC)
        .and_then(|family| {
            family
                .get_metric()
                .first()
                .and_then(|metric| metric.get_counter().as_ref())
                .map(|counter| counter.value() as u64)
        })
}

/// How long the wedged replica below is watched for. Several failed attempts and their backoffs
/// fit inside it, so a replica that gave up on the first failure would have returned by now.
const WEDGED_SETTLE: Duration = Duration::from_secs(2);

#[test]
#[tracing::instrument(skip_all)]
// A replica that cannot reach etcd has to say so on both series: the gauge at 0, because the sum
// is what pages, and the counter rising, because a healthy standby queued behind a healthy leader
// also exports 0.
//
// The gauge is first forced to 1 with nothing leading, so the assertion sees the startup's own
// reset rather than a series that was never touched.
async fn a_standby_that_cannot_reach_etcd_does_not_export_itself_as_the_leader(
    etcd: &Arc<DockerEtcd>,
) {
    wipe_state(etcd).await;
    await_election_idle(etcd).await;

    let leader_token = CancellationToken::new();
    let (details, mut join_set) = start_leader(&distributed_config(etcd), leader_token).await;
    assert_eq!(
        is_leader_gauge(),
        Some(1.0),
        "A replica that won the campaign and finished starting is not exporting itself as the \
         leader, so this test cannot show a startup resetting the gauge."
    );

    // Aborted rather than stood down, then revoked from outside: either a step-down or a live
    // keepalive would reset the gauge, and what is wanted is a 1 left by a replica that is not
    // leading anything.
    let (_, lease_id) = sole_campaigner(etcd).await;
    join_set.abort_all();
    drop(details);
    etcd_client::Client::connect([etcd.client_url()], None)
        .await
        .expect("Cannot connect to etcd")
        .lease_client()
        .revoke(lease_id)
        .await
        .expect("Revoking the abandoned leader's lease should succeed");
    assert_eq!(
        is_leader_gauge(),
        Some(1.0),
        "Something reset the gauge before the wedged replica started, so what the assertion below \
         reads would be that reset rather than this replica's own startup."
    );

    let before = campaign_attempt_failures().unwrap_or(0);
    let mut wedged_config = distributed_config(etcd);
    wedged_config.persistence = PersistenceConfig::Etcd(EtcdConfig {
        endpoints: vec![DEAD_MEMBER.to_string()],
        // Short enough that several attempts fail inside `WEDGED_SETTLE`, and still under half the
        // lease TTL, which startup refuses to exceed.
        connect_timeout: Duration::from_millis(200),
        request_timeout: Duration::from_millis(200),
        leader_lease_ttl: LEASE_TTL,
    });

    let shutdown = CancellationToken::new();
    let mut wedged_tasks: JoinSet<anyhow::Result<()>> = JoinSet::new();
    // Scoped so the borrow `run()` holds on the join set ends before it is drained.
    {
        let mut wedged = std::pin::pin!(golem_shard_manager::run(
            &wedged_config,
            Deployment::Standalone {
                shutdown: shutdown.clone()
            },
            prometheus::Registry::new(),
            &mut wedged_tasks,
        ));
        assert!(
            timeout(WEDGED_SETTLE, &mut wedged).await.is_err(),
            "`run()` returned inside {WEDGED_SETTLE:?} against `{DEAD_MEMBER}`, so this replica is \
             not wedged and the readings below are of something else."
        );

        assert_eq!(
            is_leader_gauge(),
            Some(0.0),
            "A replica that has not reached its campaign, let alone won it, is exporting itself \
             as the leader. Alerting on the sum then reads a leaderless deployment as healthy - \
             and reads two leaders as one."
        );

        let after = campaign_attempt_failures().unwrap_or_else(|| {
            panic!(
                "`{CAMPAIGN_FAILURES_METRIC}` was never registered, so the 0 on the gauge is all \
                 this replica exports and it is the same 0 a healthy standby exports."
            )
        });
        assert!(
            after > before,
            "The gauge reads 0 and `{CAMPAIGN_FAILURES_METRIC}` has not moved from {before}, \
             which is exactly what a replica queued behind a healthy leader looks like."
        );

        shutdown.cancel();
        let _ = timeout(STARTUP_TIMEOUT, wedged).await;
    }
    wedged_tasks.abort_all();
}

#[test]
#[tracing::instrument(skip_all)]
async fn the_drain_waits_for_its_tasks_before_releasing_the_leadership(etcd: &Arc<DockerEtcd>) {
    wipe_state(etcd).await;
    let shutdown = CancellationToken::new();
    let (details, mut join_set) = start_leader(&distributed_config(etcd), shutdown.clone()).await;

    const BUSY: Duration = Duration::from_millis(600);
    join_set.spawn(async move {
        std::thread::sleep(BUSY);
        std::future::pending::<()>().await;
        Ok(())
    });
    tokio::time::sleep(Duration::from_millis(50)).await;

    shutdown.cancel();
    let draining_since = Instant::now();
    golem_shard_manager::serve_until_stopped(details, join_set, shutdown)
        .await
        .expect("A shutdown is a clean stop, not a failure");
    let drained = draining_since.elapsed();

    assert!(
        drained >= BUSY - Duration::from_millis(150),
        "The drain returned after {drained:?}, before a task that was still running could stop. \
         The leadership is released as soon as it returns, so that task could command an executor \
         after a standby had already taken over."
    );
}
