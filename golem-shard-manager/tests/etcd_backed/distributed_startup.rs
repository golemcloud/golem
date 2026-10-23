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

use super::DEAD_MEMBER;
use super::proxy::BreakableProxy;
use anyhow::anyhow;
use chrono::DateTime;
use golem_api_grpc::proto::golem::shardmanager::v1::GetRoutingTableRequest;
use golem_api_grpc::proto::golem::shardmanager::v1::shard_manager_service_client::ShardManagerServiceClient;
use golem_common::model::RetryConfig;
use golem_service_base::clients::shard_manager::{
    GrpcShardManager, GrpcShardManagerConfig, ShardManager,
};
use golem_service_base::clients::shard_manager_leader::{
    LeaderAddress, leader_of, refused_before_running,
};
use golem_service_base::grpc::client::GrpcClientConfig;
use golem_shard_manager::RequestClient;
use golem_shard_manager::config::{
    EtcdConfig, GrpcApiConfig, PersistenceConfig, ShardManagerConfig,
};
use golem_shard_manager::{
    Deployment, ElectionValue, EtcdRoutingTablePersistence, ExecutorAddr, ExecutorId,
    LEADER_ELECTION_NAME, LeaderElection, LeaderFence, LeaseKeepAlive, LeaseLossReason, LeaseLost,
    NO_REVISION, ReadRetry, RoutingTablePersistence, RunDetails, STATE_KEY, ShardLeaseState,
    ShardManagerError,
};
use golem_test_framework::components::etcd::docker_etcd::DockerEtcd;
use std::collections::{BTreeMap, BTreeSet};
use std::net::Ipv4Addr;
use std::sync::Arc;
use std::time::Duration;
use test_r::{inherit_test_dep, test};
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tokio::task::{JoinHandle, JoinSet};
use tokio::time::{Instant, timeout};
use tokio_util::sync::CancellationToken;
use tonic_health::pb::HealthCheckRequest;
use tonic_health::pb::health_check_response::ServingStatus;
use tonic_health::pb::health_client::HealthClient;
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
            // Every replica of these tests runs in this process.
            advertised_host: Some("127.0.0.1".to_string()),
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

/// Every campaigner's election value under the production election name, by key.
async fn campaigner_values(etcd: &DockerEtcd) -> BTreeMap<Vec<u8>, ElectionValue> {
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
        .filter_map(|kv| Some((kv.key().to_vec(), ElectionValue::decode(kv.value())?)))
        .collect()
}

/// The advertised gRPC ports of the campaigners, in election queue order: etcd hands the
/// leadership to the key with the lowest create revision.
async fn queue_order(etcd: &DockerEtcd) -> Vec<u16> {
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
    let mut campaigners: Vec<(i64, u16)> = response
        .kvs()
        .iter()
        .filter_map(|kv| {
            let port = ElectionValue::decode(kv.value())?.grpc_address.port;
            Some((kv.create_revision(), port))
        })
        .collect();
    campaigners.sort();
    campaigners.into_iter().map(|(_, port)| port).collect()
}

/// Fails if the election gains a campaigner at any moment within `window`. Looked at all along,
/// not at the end: a replica that campaigned and left again must be caught as well.
async fn assert_no_campaigner_within(
    etcd: &DockerEtcd,
    before: &BTreeSet<Vec<u8>>,
    window: Duration,
    complaint: &str,
) {
    let deadline = Instant::now() + window;
    while Instant::now() < deadline {
        assert_eq!(&campaigner_keys(etcd).await, before, "{complaint}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// The gRPC address of the campaigner that joined after `before` was taken.
async fn await_new_campaigner(etcd: &DockerEtcd, before: &BTreeSet<Vec<u8>>) -> LeaderAddress {
    timeout(STARTUP_TIMEOUT, async {
        loop {
            if let Some(value) = campaigner_values(etcd)
                .await
                .into_iter()
                .find_map(|(key, value)| (!before.contains(&key)).then_some(value))
            {
                return value.grpc_address;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("The second replica should join the campaign")
}

/// One `GetRoutingTable` call straight to a replica's port, with no client routing.
async fn routing_table_status(port: u16) -> Result<(), tonic::Status> {
    let mut client = ShardManagerServiceClient::connect(format!("http://127.0.0.1:{port}"))
        .await
        .expect("Every replica serves its gRPC port");
    client
        .get_routing_table(GetRoutingTableRequest {})
        .await
        .map(|_| ())
}

/// The status `grpc.health.v1.Health/Check` reports for `service` on a replica's gRPC port.
async fn health_status(port: u16, service: &str) -> i32 {
    let channel = tonic::transport::Endpoint::from_shared(format!("http://127.0.0.1:{port}"))
        .expect("The replica's address is a valid endpoint")
        .connect()
        .await
        .expect("Every replica serves its gRPC port");
    HealthClient::new(channel)
        .check(HealthCheckRequest {
            service: service.to_string(),
        })
        .await
        .expect("The health service answers for a known service")
        .into_inner()
        .status
}

#[test]
#[tracing::instrument(skip_all)]
// The whole distributed path in one pass: the leader serves; the standby serves its port but
// sends callers to the leader; takeover costs a lease expiry rather than a revoke; the promoted
// standby then serves on the port it had all along.
async fn a_standby_sends_callers_to_the_leader_until_the_leader_goes_away(etcd: &Arc<DockerEtcd>) {
    wipe_state(etcd).await;
    let config = distributed_config(etcd);

    let (leader, leader_ports) = spawn_replica(config.clone(), CancellationToken::new());
    let leader_details = timeout(STARTUP_TIMEOUT, leader_ports)
        .await
        .expect("The first replica should win an uncontested campaign and finish starting")
        .expect("The leader's startup should report its ports");
    routing_table_status(leader_details.grpc_port)
        .await
        .expect("The leader serves the routing table");

    let before = campaigner_keys(etcd).await;
    let (standby, mut standby_ports) = spawn_replica(config.clone(), CancellationToken::new());
    let standby_address = await_new_campaigner(etcd, &before).await;

    let refusal = routing_table_status(standby_address.port)
        .await
        .expect_err("A standby must not serve the routing table");
    assert_eq!(
        leader_of(&refusal),
        Some(LeaderAddress {
            host: "127.0.0.1".to_string(),
            port: leader_details.grpc_port
        }),
        "The standby should name the ready leader, but answered {refusal:?}"
    );

    // Load balancers that probe per service must send traffic only to the leader, while the
    // process-wide status stays healthy on both replicas.
    const SERVICE: &str = "golem.shardmanager.v1.ShardManagerService";
    let serving = ServingStatus::Serving as i32;
    let not_serving = ServingStatus::NotServing as i32;
    assert_eq!(
        health_status(leader_details.grpc_port, SERVICE).await,
        serving
    );
    assert_eq!(
        health_status(standby_address.port, SERVICE).await,
        not_serving
    );
    assert_eq!(health_status(leader_details.grpc_port, "").await, serving);
    assert_eq!(health_status(standby_address.port, "").await, serving);

    // Longer than one lease TTL: a standby that stopped renewing during its campaign would
    // already have lost its lease by now.
    let observed_for = LEASE_TTL * 2;
    assert!(
        timeout(observed_for, &mut standby_ports).await.is_err(),
        "The second replica finished starting while the first still held the leadership."
    );
    assert!(
        !standby.is_finished(),
        "The standby's startup ended instead of blocking in the campaign."
    );

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
         {LEASE_TTL:?} lease could lapse."
    );
    assert_eq!(
        standby_details.grpc_port, standby_address.port,
        "A replica serves the same port as a standby and as the leader"
    );
    routing_table_status(standby_details.grpc_port)
        .await
        .expect("The promoted standby serves the routing table");

    standby.abort();
    info!(?failover, "Standby took over and began serving");
}

#[test]
#[tracing::instrument(skip_all)]
// A standby sends clients only to a value marked ready, so only a leader past its startup may
// carry the mark.
async fn only_a_started_leader_is_marked_ready(etcd: &Arc<DockerEtcd>) {
    wipe_state(etcd).await;
    let config = distributed_config(etcd);
    let (leader, leader_ports) = spawn_replica(config.clone(), CancellationToken::new());
    let leader_details = timeout(STARTUP_TIMEOUT, leader_ports)
        .await
        .expect("The first replica should finish starting")
        .expect("The leader's startup should report its ports");

    let before = campaigner_keys(etcd).await;
    let (standby, _standby_ports) = spawn_replica(config, CancellationToken::new());
    let standby_address = await_new_campaigner(etcd, &before).await;

    let values: Vec<ElectionValue> = campaigner_values(etcd).await.into_values().collect();
    let leader_value = values
        .iter()
        .find(|value| value.grpc_address.port == leader_details.grpc_port)
        .expect("The leader's value is under the election prefix");
    let standby_value = values
        .iter()
        .find(|value| value.grpc_address == standby_address)
        .expect("The standby's value is under the election prefix");
    assert!(
        leader_value.ready,
        "A leader whose run() returned is ready: {leader_value:?}"
    );
    assert!(
        !standby_value.ready,
        "A standby is never ready: {standby_value:?}"
    );

    leader.abort();
    standby.abort();
}

const IS_SERVING_METRIC: &str = "shard_manager_is_serving";

/// The current reading of a gauge on the process-wide default registry, or `None` while it has
/// never been set. This replica's only because this suite runs sequentially.
fn gauge(name: &str) -> Option<f64> {
    prometheus::default_registry()
        .gather()
        .into_iter()
        .find(|family| family.name() == name)
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
// `shard_manager_is_leader` turns on at the election, before the state restore and the executor
// health check; only `shard_manager_is_serving` says the replica takes calls.
async fn the_serving_gauge_is_on_while_the_leader_serves_and_drops_on_step_down(
    etcd: &Arc<DockerEtcd>,
) {
    wipe_state(etcd).await;
    let (leader, leader_ports) = spawn_replica(distributed_config(etcd), CancellationToken::new());
    let details = timeout(STARTUP_TIMEOUT, leader_ports)
        .await
        .expect("The replica should finish starting")
        .expect("The leader's startup should report its ports");

    assert_eq!(
        gauge(IS_SERVING_METRIC),
        Some(1.0),
        "A leader whose startup completed serves"
    );

    details
        .leadership
        .as_ref()
        .expect("A distributed leader has a leadership handle")
        .step_down()
        .await
        .expect("Stepping down should succeed");
    timeout(STARTUP_TIMEOUT, async {
        while gauge(IS_SERVING_METRIC) != Some(0.0) {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("A replica that stepped down no longer serves");
    leader.abort();
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
//
// The failure is a task of the caller's that dies once the initial health check has reached its
// executor, which only a replica that has won the election does. The startup is then held by that
// check: that is a failure after the win, and the only way out of the held startup before the
// check ends is the supervision of the tasks.
async fn a_startup_failure_after_winning_releases_the_leadership(etcd: &Arc<DockerEtcd>) {
    wipe_state(etcd).await;
    await_election_idle(etcd).await;

    // Accepts and then says nothing, so the initial health check against it holds the startup for
    // its whole timeout, and its lease was granted long ago, so the check cannot be skipped.
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

    let health_check_timeout = Duration::from_secs(8);
    let config = ShardManagerConfig {
        initial_health_check_timeout: health_check_timeout,
        ..distributed_config(etcd)
    };

    let (leader_seen, leader_was_seen) = oneshot::channel();
    let mut join_set: JoinSet<anyhow::Result<()>> = JoinSet::new();
    join_set.spawn({
        let etcd_url = etcd.client_url();
        async move {
            // The health check reaching the stand-in executor is what proves that the startup is
            // past the election and held by it, so the failure below lands in a startup that is
            // still running. Kept open so that the check keeps waiting.
            let (_health_check, _) = timeout(STARTUP_TIMEOUT, silent_executor.accept())
                .await
                .map_err(|_| anyhow!("the new leader never reached its initial health check"))??;

            let campaigners = etcd_client::Client::connect([etcd_url], None)
                .await?
                .kv_client()
                .get(
                    format!("{LEADER_ELECTION_NAME}/"),
                    Some(etcd_client::GetOptions::new().with_prefix()),
                )
                .await?
                .kvs()
                .len();
            anyhow::ensure!(
                campaigners == 1,
                "exactly this replica should be in the election, but {campaigners} are"
            );
            let _ = leader_seen.send(());
            Err(anyhow!("simulated task failure after the win"))
        }
    });

    // Shorter than the health check: a startup that ended only when the check did would not have
    // been ended by the failed task.
    let result = timeout(
        health_check_timeout - Duration::from_secs(3),
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
    .expect("The startup should end when a task fails, not wait out its health check");
    join_set.abort_all();

    leader_was_seen
        .await
        .expect("etcd should have named the replica the leader before the startup failed");

    let err = result
        .err()
        .expect("A task that failed must fail the startup, not be swallowed");
    let message = format!("{err:#}");
    assert!(
        message.contains("simulated task failure after the win"),
        "The startup should have failed over the task that died. Anything else means it ended for \
         another reason, and what follows would prove nothing. Was: {message}"
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

    let election = LeaderElection::connect(
        &etcd_config(etcd),
        LEADER_ELECTION_NAME,
        LeaderAddress {
            host: "127.0.0.1".to_string(),
            port: 9,
        },
    )
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
        stop: None,
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
        // Caps the campaign backoff, so attempts that hit the dead member retry quickly.
        retry_max_delay: Duration::from_millis(500),
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
        RequestClient::fixed(probe),
        NUMBER_OF_SHARDS,
        LeaderFence::for_test("/golem/test/unread-by-a-read", 1),
        0,
        ReadRetry::from_config(&EtcdConfig::default()),
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
        RequestClient::fixed(client),
        NUMBER_OF_SHARDS,
        LeaderFence::for_test("/golem/test/unread-by-a-read", 1),
        0,
        ReadRetry::from_config(&EtcdConfig::default()),
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
                // A stop requested during the startup is the caller's doing, not a failure.
                if matches!(
                    err.downcast_ref::<ShardManagerError>(),
                    Some(ShardManagerError::ShutdownRequested)
                ) {
                    return Err(err);
                }
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

/// Polls `condition` until it holds, and panics if it does not within `within`.
async fn await_until<F, Fut>(within: Duration, mut condition: F)
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    timeout(within, async {
        while !condition().await {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("A condition did not hold within {within:?}"));
}

/// A port that was free a moment ago. A replica's HTTP port is not reported while it campaigns,
/// so a test that needs it must choose it first; a collision fails loudly with "address in use"
/// at startup and cannot make a test pass wrongly.
fn free_port() -> u16 {
    std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .expect("A local port is free")
        .local_addr()
        .expect("The listener has an address")
        .port()
}

/// GET `/readyz` on a replica's HTTP port; `true` for 200. Raw HTTP/1.1: the crate has no HTTP
/// client among its dependencies.
async fn readyz(http_port: u16) -> bool {
    http_ok(http_port, "/readyz").await
}

/// GET `path` on a replica's HTTP port; `true` for 200.
async fn http_ok(http_port: u16, path: &str) -> bool {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let Ok(mut stream) = tokio::net::TcpStream::connect(("127.0.0.1", http_port)).await else {
        return false;
    };
    stream
        .write_all(
            format!("GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
                .as_bytes(),
        )
        .await
        .expect("the request is written");
    let mut response = String::new();
    stream
        .read_to_string(&mut response)
        .await
        .expect("the response is read");
    response.starts_with("HTTP/1.1 200")
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
        read_retry_timeout: Duration::from_secs(10),
        retry_min_delay: Duration::from_millis(100),
        retry_max_delay: Duration::from_secs(5),
        leader_lease_ttl: LEASE_TTL,
        compaction_retention_revisions: 0,
        tls: None,
        username: None,
        password: None,
        password_file: None,
        test_version: None,
        test_tls_reload_interval: None,
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

#[test]
#[tracing::instrument(skip_all)]
async fn a_replica_without_an_advertised_host_is_refused_before_campaigning(
    etcd: &Arc<DockerEtcd>,
) {
    let mut config = distributed_config(etcd);
    config.grpc.advertised_host = None;

    let message = startup_refusal(&config).await;

    assert!(
        message.contains("grpc.advertised_host"),
        "The refusal should name the missing setting, but was: {message}"
    );
}

#[test]
#[tracing::instrument(skip_all)]
// The worker executor's and worker service's client, pointed at a standby, gets its answers from
// the leader: the standby's answer and the client's routing agree on the wire.
async fn a_client_pointed_at_a_standby_reaches_the_leader(etcd: &Arc<DockerEtcd>) {
    wipe_state(etcd).await;
    let config = distributed_config(etcd);
    let (leader, leader_ports) = spawn_replica(config.clone(), CancellationToken::new());
    let leader_details = timeout(STARTUP_TIMEOUT, leader_ports)
        .await
        .expect("The first replica should finish starting")
        .expect("The leader's startup should report its ports");

    let before = campaigner_keys(etcd).await;
    let (standby, _standby_ports) = spawn_replica(config, CancellationToken::new());
    let standby_address = await_new_campaigner(etcd, &before).await;

    // The standby itself serves nothing; it names the leader, which is where the client's call
    // must end up.
    // Its view of the election may not be read yet when its key appears, so the answer is polled
    // until it names a leader.
    await_until(STARTUP_TIMEOUT, || async {
        let refusal = routing_table_status(standby_address.port)
            .await
            .expect_err("a standby never serves");
        leader_of(&refusal).map(|address| address.port) == Some(leader_details.grpc_port)
    })
    .await;

    let client = GrpcShardManager::new(&GrpcShardManagerConfig {
        host: "127.0.0.1".to_string(),
        port: standby_address.port,
        ..GrpcShardManagerConfig::default()
    });
    let table = client
        .get_routing_table()
        .await
        .expect("The standby sends the client to the leader");
    assert_eq!(table.number_of_shards.value, NUMBER_OF_SHARDS);

    leader.abort();
    standby.abort();
}

#[test]
#[tracing::instrument(skip_all)]
// A replica cut off from etcd must leave the rollout's count of ready replicas, and must still
// answer callers from what it last saw.
async fn a_standby_cut_off_from_etcd_stops_being_ready_but_still_names_the_leader(
    etcd: &Arc<DockerEtcd>,
) {
    wipe_state(etcd).await;
    let config = distributed_config(etcd);
    let (leader, leader_ports) = spawn_replica(config.clone(), CancellationToken::new());
    let leader_details = timeout(STARTUP_TIMEOUT, leader_ports)
        .await
        .unwrap()
        .unwrap();

    let proxy = BreakableProxy::start(&etcd.client_url()).await;
    let mut standby_config = config.clone();
    standby_config.http_port = free_port();
    if let PersistenceConfig::Etcd(etcd_config) = &mut standby_config.persistence {
        etcd_config.endpoints = vec![proxy.url()];
    }
    let before = campaigner_keys(etcd).await;
    let (standby, _) = spawn_replica(standby_config.clone(), CancellationToken::new());
    let standby_address = await_new_campaigner(etcd, &before).await;
    await_until(STARTUP_TIMEOUT, || readyz(standby_config.http_port)).await;

    proxy.black_hole();
    let refusal = routing_table_status(standby_address.port)
        .await
        .expect_err("a standby never serves");
    assert_eq!(
        leader_of(&refusal).map(|address| address.port),
        Some(leader_details.grpc_port),
        "the standby answers from its last view while etcd is unreachable"
    );
    await_until(LEASE_TTL * 3, || async {
        !readyz(standby_config.http_port).await
    })
    .await;
    // Not ready means the watch has been quiet for its whole limit; one more limit later it has
    // given up that stream and restarted into the hole at least once, so this is the answer after
    // a failed restart, not the old view by construction.
    tokio::time::sleep(LEASE_TTL + Duration::from_secs(1)).await;
    let refusal = routing_table_status(standby_address.port)
        .await
        .expect_err("a standby never serves");
    assert_eq!(
        leader_of(&refusal).map(|address| address.port),
        Some(leader_details.grpc_port),
        "the standby still names the leader after its watch failed"
    );

    proxy.restore();
    await_until(STARTUP_TIMEOUT, || readyz(standby_config.http_port)).await;
    leader.abort();
    standby.abort();
}

#[test]
#[tracing::instrument(skip_all)]
// A connection to etcd that goes silent without a close, such as one through a node that lost
// power, is never given up by waiting on it. The standby must replace it by itself, become ready
// again and name the leader, with nobody lifting the fault.
async fn a_standby_whose_etcd_connection_goes_silent_recovers_by_itself(etcd: &Arc<DockerEtcd>) {
    wipe_state(etcd).await;
    let config = distributed_config(etcd);
    let (leader, leader_ports) = spawn_replica(config.clone(), CancellationToken::new());
    let leader_details = timeout(STARTUP_TIMEOUT, leader_ports)
        .await
        .unwrap()
        .unwrap();

    let proxy = BreakableProxy::start(&etcd.client_url()).await;
    let mut standby_config = config.clone();
    standby_config.http_port = free_port();
    if let PersistenceConfig::Etcd(etcd_config) = &mut standby_config.persistence {
        etcd_config.endpoints = vec![proxy.url()];
    }
    let before = campaigner_keys(etcd).await;
    let (standby, _) = spawn_replica(standby_config.clone(), CancellationToken::new());
    let standby_address = await_new_campaigner(etcd, &before).await;
    await_until(STARTUP_TIMEOUT, || readyz(standby_config.http_port)).await;

    proxy.black_hole_existing_connections().await;
    await_until(LEASE_TTL * 3, || async {
        !readyz(standby_config.http_port).await
    })
    .await;

    // The keepalive of the connection finds it dead after its interval and timeout, and the
    // replacement connection is not black-holed.
    await_until(Duration::from_secs(40), || async {
        readyz(standby_config.http_port).await
    })
    .await;
    let refusal = routing_table_status(standby_address.port)
        .await
        .expect_err("a standby never serves");
    assert_eq!(
        leader_of(&refusal).map(|address| address.port),
        Some(leader_details.grpc_port),
        "the recovered standby names the leader"
    );
    leader.abort();
    standby.abort();
}

#[test]
#[tracing::instrument(skip_all)]
// A replica is ready once it reaches the address it advertises: a leader and a standby alike.
async fn replicas_that_reach_their_advertised_address_become_ready(etcd: &Arc<DockerEtcd>) {
    wipe_state(etcd).await;
    let mut leader_config = distributed_config(etcd);
    leader_config.http_port = free_port();
    let (leader, leader_ports) = spawn_replica(leader_config.clone(), CancellationToken::new());
    timeout(STARTUP_TIMEOUT, leader_ports)
        .await
        .unwrap()
        .unwrap();
    await_until(STARTUP_TIMEOUT, || readyz(leader_config.http_port)).await;

    let mut standby_config = distributed_config(etcd);
    standby_config.http_port = free_port();
    let before = campaigner_keys(etcd).await;
    let (standby, _) = spawn_replica(standby_config.clone(), CancellationToken::new());
    await_new_campaigner(etcd, &before).await;
    await_until(STARTUP_TIMEOUT, || readyz(standby_config.http_port)).await;

    leader.abort();
    standby.abort();
}

#[test]
#[tracing::instrument(skip_all)]
// A wrong advertised address passes every other check: etcd answers, the port is bound. Clients
// that are sent there would find nothing, so the replica must not count as ready for a rollout,
// and it must not join the election either: once it led, every standby would name that address.
async fn a_replica_whose_advertised_address_does_not_reach_it_stays_unready_and_never_campaigns(
    etcd: &Arc<DockerEtcd>,
) {
    wipe_state(etcd).await;
    let mut leader_config = distributed_config(etcd);
    leader_config.http_port = free_port();
    let (leader, leader_ports) = spawn_replica(leader_config.clone(), CancellationToken::new());
    timeout(STARTUP_TIMEOUT, leader_ports)
        .await
        .unwrap()
        .unwrap();

    let mut standby_config = distributed_config(etcd);
    standby_config.http_port = free_port();
    // Nothing listens here: the standby serves on its own bound port, not on the one it names.
    standby_config.grpc.advertised_port = Some(free_port());
    let before = campaigner_keys(etcd).await;
    let (standby, _) = spawn_replica(standby_config.clone(), CancellationToken::new());

    // Several attempts of the replica's own check fit in this window, and a campaigner key is
    // looked for during the whole of it, from right after the spawn: a replica that joins and
    // leaves would otherwise be missed.
    let window = Duration::from_secs(4);
    let (became_ready, ()) = tokio::join!(
        timeout(window, async {
            while !readyz(standby_config.http_port).await {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        }),
        assert_no_campaigner_within(
            etcd,
            &before,
            window,
            "a replica that cannot reach its advertised address must not join the election",
        )
    );
    assert!(
        became_ready.is_err(),
        "a replica that cannot reach its advertised address must not become ready"
    );
    assert!(!readyz(standby_config.http_port).await);
    assert!(
        http_ok(standby_config.http_port, "/healthcheck").await,
        "the replica itself is healthy"
    );
    assert!(
        readyz(leader_config.http_port).await,
        "the replica with a working address is ready"
    );

    leader.abort();
    standby.abort();
}

#[test]
#[tracing::instrument(skip_all)]
// A replica that waits for its advertised address to reach it is stopped like one that waits in
// the campaign: the startup ends with the stop that was asked for, and nothing is left queued.
async fn a_shutdown_while_waiting_for_the_own_address_ends_the_startup(etcd: &Arc<DockerEtcd>) {
    wipe_state(etcd).await;
    await_election_idle(etcd).await;
    let mut config = distributed_config(etcd);
    config.grpc.advertised_port = Some(free_port());
    let shutdown = CancellationToken::new();
    let mut join_set: JoinSet<anyhow::Result<()>> = JoinSet::new();
    let stopper = shutdown.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(2)).await;
        stopper.cancel();
    });
    let started = timeout(
        REFUSAL_TIMEOUT,
        golem_shard_manager::run(
            &config,
            Deployment::Standalone { shutdown },
            prometheus::Registry::new(),
            &mut join_set,
        ),
    )
    .await
    .expect("a stop must end a startup that waits for the own address");
    join_set.abort_all();
    let err = started.err().expect("the startup was stopped");
    assert!(
        matches!(
            err.downcast_ref::<ShardManagerError>(),
            Some(ShardManagerError::ShutdownRequested)
        ),
        "{err:#}"
    );
    assert!(
        campaigner_keys(etcd).await.is_empty(),
        "nothing is left queued in the election after the stop"
    );
}

#[test]
#[tracing::instrument(skip_all)]
// An address that some other shard manager answers at is not this replica's: the check must tell
// the two processes apart, or the replica would lead under an address that reaches the other one.
async fn a_replica_that_advertises_another_shard_managers_address_never_campaigns(
    etcd: &Arc<DockerEtcd>,
) {
    wipe_state(etcd).await;
    let mut leader_config = distributed_config(etcd);
    leader_config.http_port = free_port();
    let (leader, leader_ports) = spawn_replica(leader_config.clone(), CancellationToken::new());
    let leader_details = timeout(STARTUP_TIMEOUT, leader_ports)
        .await
        .unwrap()
        .unwrap();

    let mut impostor_config = distributed_config(etcd);
    impostor_config.http_port = free_port();
    impostor_config.grpc.advertised_port = Some(leader_details.grpc_port);
    let before = campaigner_keys(etcd).await;
    let (impostor, _) = spawn_replica(impostor_config.clone(), CancellationToken::new());

    assert_no_campaigner_within(
        etcd,
        &before,
        Duration::from_secs(4),
        "a replica whose advertised address reaches another shard manager must not campaign",
    )
    .await;
    assert!(
        !readyz(impostor_config.http_port).await,
        "a shard manager at the advertised address is not this replica"
    );

    leader.abort();
    impostor.abort();
}

#[test]
#[tracing::instrument(skip_all)]
// A just-elected leader runs its startup, here held in the executors' initial health check, and a
// rollout counts its readiness: a leader that turned unready for the length of its startup would
// stall the rollout, which waits for every new pod to be ready before it stops the next old one.
// The replica advertises its real gRPC address, because a replica is ready only once it reaches
// the address it advertises; a replica that never became ready would fail the wait below instead
// of passing the assertion that follows it.
async fn a_just_elected_leader_stays_ready_during_its_startup(etcd: &Arc<DockerEtcd>) {
    wipe_state(etcd).await;
    await_election_idle(etcd).await;

    // Accepts and then says nothing, and its lease was granted long ago, so the initial health
    // check cannot be skipped. Each attempt against the executor waits out its own timeout; that
    // timeout is raised to 10 s here so that the five attempts of the default retry schedule would
    // take longer than the 30 s limit of the initial check, which then holds the startup for the
    // full 30 s. With the default 2 s the retries would end the check after about 11 s, which a
    // slow run could use up before the assertions below are done.
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

    let mut config = ShardManagerConfig {
        initial_health_check_timeout: Duration::from_secs(30),
        ..distributed_config(etcd)
    };
    config.worker_executors.health_check_timeout = Duration::from_secs(10);
    config.http_port = free_port();
    config.grpc.port = free_port();
    let http_port = config.http_port;

    let shutdown = CancellationToken::new();
    let replica = tokio::spawn({
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

    // The health check reaching the stand-in executor proves that the replica won the election and
    // is inside its startup. The connection is kept, so the check stays stuck.
    let _dialled = timeout(STARTUP_TIMEOUT, silent_executor.accept())
        .await
        .expect("The elected replica should reach its initial health check")
        .expect("Cannot accept the health check connection");

    // The reach to its own address is retried every second, so the replica may need a moment to
    // report ready; that wait is bounded and happens while the startup is still held.
    await_until(STARTUP_TIMEOUT, || readyz(http_port)).await;
    assert!(
        !replica.is_finished(),
        "the leader became ready only after its startup finished"
    );

    // From here the replica is ready and its startup is held. It must stay ready for a while: a
    // gate that went unready for the leader's startup would answer 503 on one of these polls.
    let held_until = Instant::now() + Duration::from_secs(2);
    while Instant::now() < held_until {
        assert!(
            timeout(Duration::from_secs(1), readyz(http_port))
                .await
                .expect("the readiness probe should answer within a second"),
            "the leader turned unready while it was starting"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    // Those answers came from a leader that was still starting: it has not returned from `run()`,
    // and its election value does not yet say that it is ready.
    assert!(
        !replica.is_finished(),
        "the startup finished early, so the polls above did not cover it"
    );
    let values = timeout(Duration::from_secs(1), campaigner_values(etcd))
        .await
        .expect("etcd should answer the election read within a second");
    assert_eq!(values.len(), 1, "exactly this replica is in the election");
    assert!(
        values.values().all(|value| !value.ready),
        "the leader already published itself as ready, so its startup was not held"
    );

    shutdown.cancel();
    let result = timeout(Duration::from_secs(5), replica)
        .await
        .expect("the replica should stop within five seconds of being asked to")
        .expect("the replica task should not panic");
    assert!(
        matches!(
            result
                .expect_err("a replica asked to stop mid-startup does not finish starting")
                .downcast_ref::<ShardManagerError>(),
            Some(ShardManagerError::ShutdownRequested)
        ),
        "the replica should end on the requested shutdown"
    );
}

#[test]
#[tracing::instrument(skip_all)]
// A leader cut off from etcd must stop serving within one lease TTL: from then on another replica
// may lead.
async fn a_leader_cut_off_from_etcd_stops_serving_within_one_lease_ttl(etcd: &Arc<DockerEtcd>) {
    wipe_state(etcd).await;
    let proxy = BreakableProxy::start(&etcd.client_url()).await;
    let mut config = distributed_config(etcd);
    if let PersistenceConfig::Etcd(etcd_config) = &mut config.persistence {
        etcd_config.endpoints = vec![proxy.url()];
    }
    let (leader, ports) = spawn_replica(config, CancellationToken::new());
    let details = timeout(STARTUP_TIMEOUT, ports).await.unwrap().unwrap();
    routing_table_status(details.grpc_port)
        .await
        .expect("the leader serves");

    let cut_at = Instant::now();
    proxy.black_hole();
    // Either the gate refuses the call, or the replica has already stopped as a lost lease makes
    // it do, and its port refuses the connection: in both cases it serves nothing.
    await_until(LEASE_TTL * 2, || async {
        match ShardManagerServiceClient::connect(format!("http://127.0.0.1:{}", details.grpc_port))
            .await
        {
            Err(_) => true,
            Ok(mut client) => client
                .get_routing_table(GetRoutingTableRequest {})
                .await
                .is_err(),
        }
    })
    .await;
    assert!(cut_at.elapsed() <= LEASE_TTL + Duration::from_millis(500));
    leader.abort();
}

#[test]
#[tracing::instrument(skip_all)]
// The leadership lease can lapse while the new leader is still starting. The keepalive reports
// that into the task set, which nobody reads until the startup is over, so without supervision a
// replica that lost its lease would finish a slow startup, mark itself ready and only then stop.
// The lease is lost here while the startup is held in the executors' initial health check, which
// is what the supervision of the startup steps covers; a lease that lapses during the marking as
// ready is covered by the check before each attempt to mark.
async fn a_leader_that_loses_its_lease_during_its_startup_stops_at_once(etcd: &Arc<DockerEtcd>) {
    wipe_state(etcd).await;
    await_election_idle(etcd).await;

    // Accepts and then says nothing, so the initial health check against it runs out its whole
    // timeout, and its lease was granted long ago, so the check cannot be skipped.
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

    let proxy = BreakableProxy::start(&etcd.client_url()).await;
    let health_check_timeout = Duration::from_secs(10);
    let mut config = ShardManagerConfig {
        initial_health_check_timeout: health_check_timeout,
        ..distributed_config(etcd)
    };
    if let PersistenceConfig::Etcd(etcd_config) = &mut config.persistence {
        etcd_config.endpoints = vec![proxy.url()];
    }

    let replica = tokio::spawn(async move {
        let mut join_set: JoinSet<anyhow::Result<()>> = JoinSet::new();
        let result = golem_shard_manager::run(
            &config,
            Deployment::Standalone {
                shutdown: CancellationToken::new(),
            },
            prometheus::Registry::new(),
            &mut join_set,
        )
        .await;
        join_set.abort_all();
        (result.map(|_| ()), Instant::now())
    });

    // The health check reaching the stand-in executor is what proves the replica won the
    // election and is now in its startup, which is the state this test breaks.
    // Kept open until the end of the test, so that the check keeps waiting for an answer.
    let (_health_check, _) = timeout(STARTUP_TIMEOUT, silent_executor.accept())
        .await
        .expect("The new leader should reach its initial health check")
        .expect("The stand-in executor should accept the health check");
    let before_the_cut = campaigner_values(etcd).await;
    assert_eq!(
        before_the_cut.len(),
        1,
        "Exactly this replica should be in the election"
    );
    assert!(
        before_the_cut.values().all(|value| !value.ready),
        "The replica marked itself ready while its health check was still running, so the lease \
         is not lost during the startup this test is about"
    );

    proxy.black_hole();
    let cut_at = Instant::now();

    // Detection is bounded by the lease TTL, and the step-down that follows goes into the same
    // black hole and costs one more request timeout before it gives up. The health check would
    // hold the startup for ten seconds if nothing watched the task that reports the loss.
    let budget = LEASE_TTL + REQUEST_TIMEOUT + Duration::from_secs(1);
    let (result, returned_at) = timeout(budget + health_check_timeout, replica)
        .await
        .expect("The replica should stop at the latest when its health check ends")
        .expect("The replica task should not panic");
    let stopped_after = returned_at.duration_since(cut_at);
    assert!(
        stopped_after <= budget,
        "The replica needed {stopped_after:?} to stop after it lost its lease, longer than \
         {budget:?}. It kept starting on a leadership it no longer held."
    );

    let err = result.expect_err("A replica that lost its lease during its startup must fail");
    assert!(
        err.chain()
            .any(|cause| cause.downcast_ref::<LeaseLost>().is_some()),
        "The replica stopped, but not over the lease it lost: {err:#}"
    );
}

#[test]
#[tracing::instrument(skip_all)]
// Two replicas that advertise one address would each name the other as the leader, and clients
// would never find the real one: the second must refuse to go on, and say why. The holder of the
// address is a key whose lease is renewed all the time; the new replica reaches itself at that
// address, which is the case the duplicate rule exists for.
async fn a_replica_that_advertises_a_live_replicas_address_is_refused(etcd: &Arc<DockerEtcd>) {
    wipe_state(etcd).await;
    await_election_idle(etcd).await;
    let mut config = distributed_config(etcd);
    let port = free_port();
    config.grpc.port = port;

    let mut client = etcd_client::Client::connect([etcd.client_url()], None)
        .await
        .unwrap();
    let lease = client
        .lease_grant(LEASE_TTL.as_secs() as i64, None)
        .await
        .unwrap()
        .id();
    let holder = ElectionValue {
        identity: "live-holder".to_string(),
        grpc_address: LeaderAddress {
            host: "127.0.0.1".to_string(),
            port,
        },
        ready: true,
        version: String::new(),
    };
    client
        .put(
            format!("{LEADER_ELECTION_NAME}/{lease:x}"),
            holder.encode(),
            Some(etcd_client::PutOptions::new().with_lease(lease)),
        )
        .await
        .unwrap();
    let (mut keeper, mut answers) = client.lease_keep_alive(lease).await.unwrap();
    let renewing = tokio::spawn(async move {
        loop {
            if keeper.keep_alive().await.is_err() || answers.message().await.is_err() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    });

    let mut join_set: JoinSet<anyhow::Result<()>> = JoinSet::new();
    let started = timeout(
        LEASE_TTL * 6,
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
    .expect("the duplicate replica must stop within a few lease TTLs");
    join_set.abort_all();
    renewing.abort();
    let message = match started {
        Ok(_) => panic!("a replica that duplicates a live replica's address must not start"),
        Err(err) => err.root_cause().to_string(),
    };
    assert!(
        message.starts_with("Invalid configuration: another shard manager replica (")
            && message.contains("advertises the same gRPC address"),
        "{message}"
    );
}

#[test]
#[tracing::instrument(skip_all)]
// A restarted process finds the key of its own previous process, which carries the same address
// until its lease expires. That key is no duplicate: the replica must wait it out and lead.
async fn an_earlier_process_key_with_the_same_address_is_waited_out(etcd: &Arc<DockerEtcd>) {
    wait_out_an_earlier_process_key(etcd, LEASE_TTL, None).await;
}

#[test]
#[tracing::instrument(skip_all)]
// The operator may have shortened the lease TTL before the restart, so the earlier process's key
// can outlive the new replica's own patience several times over. Nothing renews it, so it still
// goes away, and the replica must wait for that instead of taking the key for a live replica.
async fn an_earlier_process_key_with_a_longer_lease_is_waited_out(etcd: &Arc<DockerEtcd>) {
    wait_out_an_earlier_process_key(etcd, LEASE_TTL * 4, None).await;
}

#[test]
#[tracing::instrument(skip_all)]
// A change of etcd's raft leader extends every lease once, abandoned ones too, so a key that
// nothing renews can outlive any fixed patience. One extension is no sign of a live replica: the
// replica must still wait the key out instead of refusing it.
async fn an_earlier_process_key_whose_lease_was_extended_once_is_waited_out(
    etcd: &Arc<DockerEtcd>,
) {
    wait_out_an_earlier_process_key(etcd, LEASE_TTL * 2, Some(LEASE_TTL + LEASE_TTL / 2)).await;
}

/// Leaves a key with a replica's address under a lease of `earlier_ttl` that nothing renews, and
/// expects a replica started over it to lead once the lease expires. With `extend_once_after`, the
/// lease is kept alive a single time after that long, as an etcd leader change would do.
async fn wait_out_an_earlier_process_key(
    etcd: &Arc<DockerEtcd>,
    earlier_ttl: Duration,
    extend_once_after: Option<Duration>,
) {
    wipe_state(etcd).await;
    await_election_idle(etcd).await;
    let mut config = distributed_config(etcd);
    // The replica serves on the port that the earlier process had, as a restarted one does: its
    // advertised address must reach it, or it would not campaign at all.
    let reserved_port = free_port();
    config.grpc.port = reserved_port;

    let mut client = etcd_client::Client::connect([etcd.client_url()], None)
        .await
        .unwrap();
    let lease = client
        .lease_grant(earlier_ttl.as_secs() as i64, None)
        .await
        .unwrap()
        .id();
    let stale = ElectionValue {
        identity: "earlier-process".to_string(),
        grpc_address: LeaderAddress {
            host: "127.0.0.1".to_string(),
            port: reserved_port,
        },
        ready: true,
        version: String::new(),
    };
    client
        .put(
            format!("{LEADER_ELECTION_NAME}/{lease:x}"),
            stale.encode(),
            Some(etcd_client::PutOptions::new().with_lease(lease)),
        )
        .await
        .unwrap();

    let (replica, ports) = spawn_replica(config, CancellationToken::new());
    if let Some(after) = extend_once_after {
        tokio::time::sleep(after).await;
        let (mut keeper, mut answers) = client.lease_keep_alive(lease).await.unwrap();
        keeper.keep_alive().await.unwrap();
        answers.message().await.unwrap();
    }
    timeout(STARTUP_TIMEOUT, ports)
        .await
        .expect("the replica leads once the earlier key's lease expires")
        .expect("the replica's startup reports its ports");
    replica.abort();
}

fn with_version(mut config: ShardManagerConfig, version: &str) -> ShardManagerConfig {
    if let PersistenceConfig::Etcd(etcd) = &mut config.persistence {
        etcd.test_version = Some(version.to_string());
    }
    config
}

#[test]
#[tracing::instrument(skip_all)]
// In a rolling update, a new replica queues behind old standbys, and each old one that leads next
// is replaced next: one handover per old pod. An old standby must step back behind a newer
// replica, so that the old leader hands over to a new replica, once.
async fn an_old_standby_moves_behind_a_newer_version(etcd: &Arc<DockerEtcd>) {
    wipe_state(etcd).await;
    await_election_idle(etcd).await;
    let config = distributed_config(etcd);

    let old_leader = CancellationToken::new();
    let (old_leader_task, old_leader_started) =
        spawn_serving_replica(with_version(config.clone(), "1.0.0"), old_leader.clone());
    timeout(STARTUP_TIMEOUT, old_leader_started)
        .await
        .unwrap()
        .unwrap();

    let before = campaigner_keys(etcd).await;
    let (old_standby, mut old_standby_started) = spawn_serving_replica(
        with_version(config.clone(), "1.0.0"),
        CancellationToken::new(),
    );
    let old_standby_address = await_new_campaigner(etcd, &before).await;

    let before = campaigner_keys(etcd).await;
    let (new_replica, new_started) = spawn_serving_replica(
        with_version(config.clone(), "2.0.0"),
        CancellationToken::new(),
    );
    let new_address = await_new_campaigner(etcd, &before).await;

    // The old standby leaves and queues again, behind the new replica.
    timeout(STARTUP_TIMEOUT, async {
        loop {
            let order: Vec<u16> = queue_order(etcd).await;
            if order.len() == 3
                && order.iter().position(|port| *port == new_address.port)
                    < order
                        .iter()
                        .position(|port| *port == old_standby_address.port)
            {
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("the old standby should move behind the newer version");

    old_leader.cancel();
    let new_port = timeout(STARTUP_TIMEOUT, new_started)
        .await
        .expect("the new replica leads after the old leader steps down")
        .expect("the new replica reports its port");
    assert_eq!(new_port, new_address.port);
    assert!(
        timeout(Duration::from_millis(500), &mut old_standby_started)
            .await
            .is_err(),
        "the old standby must not have led"
    );

    let _ = old_leader_task.await;
    old_standby.abort();
    new_replica.abort();
}

/// The client that executors and worker services use, pointed at `port`, with a retry budget that
/// covers one lease TTL plus a startup.
fn failover_client(port: u16) -> GrpcShardManager {
    GrpcShardManager::new(&GrpcShardManagerConfig {
        host: "127.0.0.1".to_string(),
        port,
        client_config: GrpcClientConfig {
            retries_on_unavailable: RetryConfig {
                max_attempts: 30,
                min_delay: Duration::from_millis(100),
                max_delay: Duration::from_millis(500),
                multiplier: 2.0,
                max_jitter_factor: None,
            },
            ..GrpcClientConfig::default()
        },
        ..GrpcShardManagerConfig::default()
    })
}

#[test]
#[tracing::instrument(skip_all)]
// An executor's client, pointed at a standby, must ride out a crash of the leader with its default
// settings: the standby names the dead leader until its lease expires, then the promoted standby
// starts and serves. No retry tuning is needed.
//
// The client is made only after the leader is gone: an aborted replica keeps serving the
// connections that it had accepted until its lease belief lapses, so a client that had called it
// before would be answered by the dead leader and see no failover at all.
async fn the_client_rides_out_an_ungraceful_failover_without_any_tuning(etcd: &Arc<DockerEtcd>) {
    wipe_state(etcd).await;
    let config = distributed_config(etcd);
    let (leader, leader_ports) = spawn_replica(config.clone(), CancellationToken::new());
    let leader_details = timeout(STARTUP_TIMEOUT, leader_ports)
        .await
        .unwrap()
        .unwrap();
    let before = campaigner_keys(etcd).await;
    let (standby, mut standby_ports) = spawn_replica(config, CancellationToken::new());
    let standby_address = await_new_campaigner(etcd, &before).await;

    leader.abort();
    // An abort takes effect only after the task is polled again, and the leader keeps answering
    // until then. The call must start after that, or the dying leader answers it and nothing is
    // shown.
    await_until(STARTUP_TIMEOUT, || async {
        let url = format!("http://127.0.0.1:{}", leader_details.grpc_port);
        match ShardManagerServiceClient::connect(url).await {
            Ok(mut dead_or_alive) => dead_or_alive
                .get_routing_table(GetRoutingTableRequest {})
                .await
                .is_err(),
            Err(_) => true,
        }
    })
    .await;
    assert!(
        matches!(
            standby_ports.try_recv(),
            Err(oneshot::error::TryRecvError::Empty)
        ),
        "the standby was elected before the call began, so no failover is left to ride out"
    );

    let client = GrpcShardManager::new(&GrpcShardManagerConfig {
        host: "127.0.0.1".to_string(),
        port: standby_address.port,
        ..GrpcShardManagerConfig::default()
    });
    let table = timeout(STARTUP_TIMEOUT, client.get_routing_table())
        .await
        .expect("the call must end within the startup timeout")
        .expect("the call rides out the failover");
    assert_eq!(table.number_of_shards.value, NUMBER_OF_SHARDS);
    // The only replica left is the standby, and it serves only once it leads.
    timeout(Duration::from_secs(1), standby_ports)
        .await
        .expect("the standby was elected by the time the call was answered")
        .expect("the standby's startup reports its ports");
    standby.abort();
}

#[test]
#[tracing::instrument(skip_all)]
// A leader that stops gives up the leadership at once, but a connection that it accepted before
// would go on being served until its lease belief lapsed: for a moment two replicas answered. A
// stopped replica must answer nothing, on old connections too.
async fn a_leader_that_stops_answers_nothing_on_the_connections_it_had_accepted(
    etcd: &Arc<DockerEtcd>,
) {
    wipe_state(etcd).await;
    await_election_idle(etcd).await;
    let shutdown = CancellationToken::new();
    let (replica, started) = spawn_serving_replica(distributed_config(etcd), shutdown.clone());
    let port = timeout(STARTUP_TIMEOUT, started).await.unwrap().unwrap();

    // One channel, made before the stop, so that the call after it really rides the old connection.
    let channel = tonic::transport::Endpoint::from_shared(format!("http://127.0.0.1:{port}"))
        .expect("The replica's address is a valid endpoint")
        .connect()
        .await
        .expect("The leader accepts the connection");
    let mut client = ShardManagerServiceClient::new(channel);
    client
        .get_routing_table(GetRoutingTableRequest {})
        .await
        .expect("The leader serves before it stops");

    shutdown.cancel();
    timeout(STARTUP_TIMEOUT, replica)
        .await
        .expect("The replica should stop")
        .expect("The replica should not panic")
        .expect("A replica stopped by its shutdown token exits cleanly");

    // Well inside the lease TTL, so a lease belief that still holds cannot be the reason. The
    // connection itself is closed, not just answered with a refusal.
    let status = client
        .get_routing_table(GetRoutingTableRequest {})
        .await
        .expect_err(
            "a stopped replica answered a call on a connection that it had accepted before",
        );
    assert!(
        !status.message().contains("stopping"),
        "the connection was still open and the gate answered on it: {status:?}"
    );
}

#[test]
#[tracing::instrument(skip_all)]
// A leader that is stopping refuses new calls before it gives the leadership up, so that no call
// begins that the next leader could also run. Observed from outside: while the stop is held in
// the drain, a call on a connection opened before the stop is refused with the mark that says it
// did not run, and the election key is still in etcd.
async fn a_stopping_leader_refuses_calls_before_it_releases_the_leadership(etcd: &Arc<DockerEtcd>) {
    wipe_state(etcd).await;
    let shutdown = CancellationToken::new();
    let (details, mut join_set) = start_leader(&distributed_config(etcd), shutdown.clone()).await;
    let port = details.grpc_port;

    let channel = tonic::transport::Endpoint::from_shared(format!("http://127.0.0.1:{port}"))
        .expect("The replica's address is a valid endpoint")
        .connect()
        .await
        .expect("The leader accepts the connection");
    let mut client = ShardManagerServiceClient::new(channel);
    client
        .get_routing_table(GetRoutingTableRequest {})
        .await
        .expect("The leader serves before it stops");

    // A task that holds a runtime thread and cannot be aborted until it returns holds the drain.
    const HELD: Duration = Duration::from_secs(3);
    join_set.spawn(async move {
        std::thread::sleep(HELD);
        std::future::pending::<()>().await;
        Ok(())
    });
    tokio::time::sleep(Duration::from_millis(50)).await;

    shutdown.cancel();
    let stopping = tokio::spawn(golem_shard_manager::serve_until_stopped(
        details, join_set, shutdown,
    ));
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(!stopping.is_finished(), "the drain should still be held");

    let status = client
        .get_routing_table(GetRoutingTableRequest {})
        .await
        .expect_err("a stopping leader answered a call on a connection from before the stop");
    assert_eq!(status.code(), tonic::Code::Unavailable);
    assert!(
        refused_before_running(&status),
        "the refusal should carry the mark that the call did not run: {status:?}"
    );
    assert!(
        !campaigner_keys(etcd).await.is_empty(),
        "the refusal came after the leadership was released, not before"
    );

    timeout(STARTUP_TIMEOUT, stopping)
        .await
        .expect("The stop should finish once the held task returns")
        .expect("The stop should not panic")
        .expect("A shutdown is a clean stop, not a failure");
}

#[test]
#[tracing::instrument(skip_all)]
// A rolling update without a pause between pods can stop a leader that has just been elected.
// The second leader is stopped while its startup is held by the initial health check (the stored
// lease has expired and the executor never answers), so it is never serving when it goes. The
// client must still find the replica that ends up leading.
async fn the_client_rides_out_two_handovers_in_a_row(etcd: &Arc<DockerEtcd>) {
    wipe_state(etcd).await;
    await_election_idle(etcd).await;
    let config = ShardManagerConfig {
        // Short enough to keep the test quick, long enough to hold a startup open while the
        // test cancels it.
        initial_health_check_timeout: Duration::from_secs(2),
        ..distributed_config(etcd)
    };

    let first = CancellationToken::new();
    let (first_task, first_started) = spawn_serving_replica(config.clone(), first.clone());
    timeout(STARTUP_TIMEOUT, first_started)
        .await
        .unwrap()
        .unwrap();

    // Stored only now, so that the first leader started without a check. Accepts and then says
    // nothing, so a check against it runs out its timeout, and its lease was granted long ago,
    // so every leader that starts from here on must run that check.
    let silent_executor = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .await
        .expect("Cannot bind the stand-in executor");
    let silent_addr = silent_executor
        .local_addr()
        .expect("The stand-in executor should report its address");
    wipe_state(etcd).await;
    write_state_with_executor(
        etcd,
        ExecutorAddr {
            ip: silent_addr.ip(),
            port: silent_addr.port(),
        },
    )
    .await;

    let before = campaigner_keys(etcd).await;
    let second = CancellationToken::new();
    let (second_task, mut second_started) = spawn_serving_replica(config.clone(), second.clone());
    let second_address = await_new_campaigner(etcd, &before).await;
    let before = campaigner_keys(etcd).await;
    let (third_task, third_started) =
        spawn_serving_replica(config.clone(), CancellationToken::new());
    let third_address = await_new_campaigner(etcd, &before).await;

    let client = failover_client(third_address.port);
    client
        .get_routing_table()
        .await
        .expect("reaches the first leader");

    first.cancel();
    // Stopped as soon as etcd names it the leader, as a pod is that Kubernetes stops next.
    timeout(STARTUP_TIMEOUT, async {
        while queue_order(etcd).await.first() != Some(&second_address.port) {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the second replica should be elected after the first steps down");
    second.cancel();
    // Waited for, so that the call below cannot be answered by the second leader while it drains.
    let second_result = timeout(STARTUP_TIMEOUT, second_task)
        .await
        .expect("The second replica should stop")
        .expect("The second replica's task should not panic");
    let second_err = second_result
        .expect_err("A replica stopped during its startup must report that, not finish starting");
    assert!(
        matches!(
            second_err.downcast_ref::<ShardManagerError>(),
            Some(ShardManagerError::ShutdownRequested)
        ),
        "The second replica ended with `{second_err:#}` rather than a shutdown during its startup."
    );
    assert!(
        second_started.try_recv().is_err(),
        "The second replica reported its port, so it was serving when it was stopped."
    );

    // The first replica has stopped for good, and the client holds a connection that it accepted:
    // the call below cannot be answered by anyone but the third replica.
    timeout(STARTUP_TIMEOUT, first_task)
        .await
        .expect("The first replica should stop")
        .expect("The first replica's task should not panic")
        .expect("The first replica stops cleanly");

    let table = timeout(STARTUP_TIMEOUT, client.get_routing_table())
        .await
        .expect("the call must end within the startup timeout")
        .expect("the third replica serves after two handovers");
    assert_eq!(table.number_of_shards.value, NUMBER_OF_SHARDS);
    // The third replica takes the initial health check's time to start. A call answered earlier
    // was answered by a replica that had stopped.
    assert_eq!(
        timeout(Duration::from_secs(1), third_started)
            .await
            .expect("the third replica was serving when the call was answered")
            .unwrap(),
        third_address.port
    );
    third_task.abort();
}

#[test]
#[tracing::instrument(skip_all)]
// Clients can reach a standby but not the leader (a network policy, a partition): the call must
// fail within its budget instead of hanging, and succeed once the leadership moves to a replica
// that clients can reach.
async fn a_client_that_cannot_reach_the_named_leader_recovers_when_the_leadership_moves(
    etcd: &Arc<DockerEtcd>,
) {
    wipe_state(etcd).await;
    await_election_idle(etcd).await;
    // The leader advertises the address of a proxy in front of its gRPC port. The leader reaches
    // itself through it, as every replica must to lead; once it leads, the proxy is cut, so that a
    // connection to the address the standby names gets no answer at all, like one to the IP of a
    // pod that clients cannot reach. A refused port would fail at once and not exercise the
    // client's connect timeout.
    let leader_port = free_port();
    let partition = BreakableProxy::start(&format!("http://127.0.0.1:{leader_port}")).await;
    let mut leader_config = distributed_config(etcd);
    leader_config.grpc.port = leader_port;
    leader_config.grpc.advertised_port = Some(partition.port());
    let leader_token = CancellationToken::new();
    let (leader_task, leader_started) = spawn_serving_replica(leader_config, leader_token.clone());
    timeout(STARTUP_TIMEOUT, leader_started)
        .await
        .unwrap()
        .unwrap();
    partition.black_hole();
    let before = campaigner_keys(etcd).await;
    let (standby_task, standby_started) =
        spawn_serving_replica(distributed_config(etcd), CancellationToken::new());
    let standby_address = await_new_campaigner(etcd, &before).await;

    let client = GrpcShardManager::new(&GrpcShardManagerConfig {
        host: "127.0.0.1".to_string(),
        port: standby_address.port,
        client_config: GrpcClientConfig {
            retries_on_unavailable: RetryConfig {
                max_attempts: 3,
                min_delay: Duration::from_millis(50),
                max_delay: Duration::from_millis(100),
                multiplier: 2.0,
                max_jitter_factor: None,
            },
            ..GrpcClientConfig::default()
        },
        ..GrpcShardManagerConfig::default()
    });
    let started = Instant::now();
    // A call that is tried once, so that it ends at the first failed connect; a call that may be
    // repeated would keep waiting for a failover to end.
    timeout(
        STARTUP_TIMEOUT,
        client.deregister(Uuid::new_v4(), BTreeMap::new()),
    )
    .await
    .expect("the call must end within its budget instead of hanging")
    .expect_err("nothing serves at the address the standby names");
    // The client's connect timeout is 10 s by default, but towards a named leader it is capped at
    // 2 s. One try costs about 2 s; a 10 s connect would cost 10 s.
    assert!(
        started.elapsed() < Duration::from_secs(9),
        "the call took {:?}; an unreachable leader must cost the short leader connect timeout, \
         not the configured one",
        started.elapsed()
    );

    leader_token.cancel();
    timeout(STARTUP_TIMEOUT, standby_started)
        .await
        .unwrap()
        .unwrap();
    timeout(STARTUP_TIMEOUT, client.get_routing_table())
        .await
        .expect("the call must end within the startup timeout")
        .expect("the promoted standby serves the same client");
    let _ = leader_task.await;
    standby_task.abort();
}
