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

pub mod config;
pub mod error;
mod grpc;
mod leader_gate;
mod metrics;
pub mod quota;
mod registry_event_subscriber;
pub(crate) mod sharding;

use self::grpc::ShardManagerServiceImpl;
#[cfg(feature = "kubernetes")]
use crate::config::HealthCheckK8sConfig;
use crate::config::{EtcdConfig, HealthCheckMode, PersistenceConfig};
use crate::leader_gate::{GatedService, LeaderGate};
use crate::quota::{DbQuotaRepo, EtcdQuotaRepo, GrpcResourceDefinitionFetcher, QuotaService};
use crate::registry_event_subscriber::ShardManagerRegistryInvalidationHandler;
use crate::sharding::etcd_connection::connect_for_requests;
use crate::sharding::etcd_retry::{is_retriable_read, retry_retriable};
use crate::sharding::healthcheck::GrpcHealthCheck;
use crate::sharding::worker_executor::WorkerExecutorServiceDefault;
use anyhow::Context;
use config::ShardManagerConfig;
use etcd_client::Client;
use golem_api_grpc::proto;
use golem_api_grpc::proto::golem::shardmanager::v1::shard_manager_service_server::ShardManagerServiceServer;
use golem_common::base_model::shard_lease;
use golem_service_base::clients::registry::GrpcRegistryService;
use golem_service_base::clients::shard_manager_leader::LeaderAddress;
use golem_service_base::grpc::server::GrpcServerTlsConfig;
use include_dir::include_dir;
use prometheus::Registry;
pub use sharding::election_watch::{
    CampaignView, Campaigner, ElectionWatch, ElectionWatcher, LeaderView,
};
pub use sharding::error::{HealthCheckError, ShardManagerError};
pub use sharding::etcd_retry::{ReadRetry, never_reached_etcd};
pub use sharding::healthcheck::HealthCheck;
pub use sharding::leader_election::{
    Elected, ElectionValue, LEADER_ELECTION_NAME, LeaderElection, LeaderFence, LeaderLocator,
    LeadershipHandle, LeaseBelief, LeaseKeepAlive, LeaseLossReason, LeaseLost, RetryDelays,
};
pub use sharding::persistence::{
    DbRoutingTablePersistence, EtcdRoutingTablePersistence, ExternalRevision, NO_REVISION,
    RoutingTablePersistence, STATE_KEY,
};
pub use sharding::shard_management::ShardManagement;
pub use sharding::worker_executor::WorkerExecutorService;
pub use sharding::{
    ExecutorAddr, ExecutorAddrs, ExecutorId, ExecutorLease, ExecutorShards, RegisterAck,
    ShardAssignmentEntry, ShardAssignmentPush, ShardEpoch, ShardLeaseGrant, ShardLeaseRevision,
    ShardLeaseState,
};
use std::sync::Arc;
use std::time::Duration;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;
use tonic::codec::CompressionEncoding;
use tonic::transport::Server;
use tonic_tracing_opentelemetry::middleware;
use tonic_tracing_opentelemetry::middleware::filters;
use tracing::Instrument;
use tracing::{debug, error, info, warn};

#[cfg(test)]
test_r::enable!();

#[cfg(test)]
mod lease_timing_tests {
    use super::validate_timing_config;
    use crate::config::{EtcdConfig, PersistenceConfig, ShardManagerConfig};
    use golem_common::base_model::shard_lease;
    use std::time::Duration;
    use test_r::test;

    fn config_with(shard_lease_duration: Duration) -> ShardManagerConfig {
        ShardManagerConfig {
            shard_lease_duration,
            ..ShardManagerConfig::default()
        }
    }

    /// The shipped default is the shortest lease that delivers the full renewal budget, so it must
    /// start without so much as a warning - if it did not, every deployment would be told to tune
    /// a value it never set.
    #[test]
    fn the_default_lease_starts_silently() {
        let warnings = validate_timing_config(&ShardManagerConfig::default())
            .expect("the shipped default must be accepted");
        assert!(warnings.is_empty(), "unexpected warnings: {warnings:?}");
    }

    /// Between the minimum and the recommended minimum the protocol still works, it just survives
    /// fewer consecutive shard-manager hiccups. That is availability, not correctness, so it warns
    /// rather than refusing - and the warning names the key, or an operator is left guessing which
    /// of several durations to move.
    #[test]
    fn a_short_but_workable_lease_warns_and_names_the_key() {
        let warnings =
            validate_timing_config(&config_with(shard_lease::min_shard_lease_duration()))
                .expect("a lease at the minimum must still start");
        assert_eq!(warnings.len(), 1);
        assert!(
            warnings[0].contains("shard_lease_duration"),
            "the warning must name the key it is about: {}",
            warnings[0]
        );
    }

    /// Below the minimum an executor cannot finish one renewal attempt inside the gap between two
    /// renewals, so every unanswered call costs the lease outright. Refused rather than degraded:
    /// every replica shares the setting, so a cluster would fail the same way at the same time.
    #[test]
    fn a_lease_too_short_to_renew_is_refused() {
        let error = validate_timing_config(&config_with(
            shard_lease::min_shard_lease_duration() - Duration::from_millis(1),
        ))
        .expect_err("a lease below the minimum must be refused");
        let message = format!("{error:#}");
        assert!(
            message.contains("shard_lease_duration"),
            "the refusal must name the key: {message}"
        );
    }

    #[test]
    fn a_zero_lease_is_still_refused() {
        assert!(validate_timing_config(&config_with(Duration::ZERO)).is_err());
    }

    #[test]
    fn the_maximum_state_write_timeout_is_accepted() {
        let config = ShardManagerConfig {
            state_write_timeout: shard_lease::max_state_write_timeout(),
            ..ShardManagerConfig::default()
        };
        validate_timing_config(&config).expect("the supported maximum must be accepted");
    }

    #[test]
    fn state_write_timeouts_outside_the_protocol_range_are_refused() {
        for state_write_timeout in [
            Duration::ZERO,
            shard_lease::max_state_write_timeout() + Duration::from_millis(1),
        ] {
            let config = ShardManagerConfig {
                state_write_timeout,
                ..ShardManagerConfig::default()
            };
            let error = validate_timing_config(&config)
                .expect_err("an unsupported state write timeout must be refused");
            assert!(
                error.to_string().contains("state_write_timeout"),
                "the refusal must name the invalid key: {error:#}"
            );
        }
    }

    #[test]
    fn invalid_startup_timeouts_are_refused() {
        let invalid = [
            ("state_read_timeout", Duration::ZERO, None),
            (
                "state_read_timeout",
                shard_lease::default_state_write_timeout(),
                None,
            ),
            (
                "initial_health_check_timeout",
                ShardManagerConfig::default().state_read_timeout,
                Some(Duration::ZERO),
            ),
        ];

        for (key, state_read_timeout, initial_health_check_timeout) in invalid {
            let config = ShardManagerConfig {
                state_read_timeout,
                initial_health_check_timeout: initial_health_check_timeout
                    .unwrap_or_else(|| ShardManagerConfig::default().initial_health_check_timeout),
                ..ShardManagerConfig::default()
            };
            let error = validate_timing_config(&config)
                .expect_err("an invalid startup timeout must be refused");
            assert!(
                error.to_string().contains(key),
                "the refusal must name {key}: {error:#}"
            );
        }
    }

    #[test]
    fn invalid_etcd_retry_settings_are_refused() {
        let invalid = [
            (
                "read_retry_timeout",
                EtcdConfig {
                    read_retry_timeout: Duration::ZERO,
                    ..EtcdConfig::default()
                },
            ),
            (
                "retry_min_delay",
                EtcdConfig {
                    retry_min_delay: Duration::ZERO,
                    ..EtcdConfig::default()
                },
            ),
            (
                "retry_max_delay",
                EtcdConfig {
                    retry_min_delay: Duration::from_secs(2),
                    retry_max_delay: Duration::from_secs(1),
                    ..EtcdConfig::default()
                },
            ),
            (
                "retry_max_delay",
                EtcdConfig {
                    retry_max_delay: Duration::MAX,
                    ..EtcdConfig::default()
                },
            ),
            (
                "read_retry_timeout",
                EtcdConfig {
                    read_retry_timeout: Duration::from_secs(26),
                    ..EtcdConfig::default()
                },
            ),
        ];

        for (key, etcd) in invalid {
            let config = ShardManagerConfig {
                persistence: PersistenceConfig::Etcd(etcd),
                ..ShardManagerConfig::default()
            };
            let error = validate_timing_config(&config)
                .expect_err("an invalid etcd retry setting must be refused");
            assert!(
                error.to_string().contains(key),
                "the refusal must name {key}: {error:#}"
            );
        }
    }

    #[test]
    fn etcd_retry_timeout_that_cannot_form_an_instant_is_refused() {
        let request_timeout = Duration::from_secs(1);
        let etcd = EtcdConfig {
            request_timeout,
            read_retry_timeout: Duration::MAX - request_timeout,
            ..EtcdConfig::default()
        };

        let config = ShardManagerConfig {
            state_read_timeout: Duration::MAX,
            persistence: PersistenceConfig::Etcd(etcd),
            ..ShardManagerConfig::default()
        };

        assert!(
            std::time::Instant::now()
                .checked_add(Duration::MAX - request_timeout)
                .is_none(),
            "the test value must exceed this platform's Instant range"
        );
        validate_timing_config(&config)
            .expect_err("startup must reject a retry timeout that panics when added to Instant");
    }
}

#[cfg(test)]
mod accept_tests {
    use super::accept_until;
    use std::time::Duration;
    use test_r::test;
    use tokio_util::sync::CancellationToken;

    #[test]
    async fn a_connect_after_the_stop_is_refused_not_left_waiting() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let stop = CancellationToken::new();
        let _incoming = accept_until(listener, stop.clone());
        tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .expect("the listener accepts before the stop");

        stop.cancel();
        tokio::time::timeout(Duration::from_secs(2), async {
            while tokio::net::TcpStream::connect(("127.0.0.1", port))
                .await
                .is_ok()
            {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("a connect after the stop is refused");
    }
}

#[cfg(test)]
mod lease_health_tests {
    use super::{
        ShardManagerGrpcService, ShardManagerServiceImpl, ShardManagerServiceServer,
        report_lease_health,
    };
    use crate::leader_gate::LeaderGate;
    use crate::sharding::leader_election::LeaseBelief;
    use std::time::Duration;
    use test_r::test;
    use tokio::time::Instant;
    use tonic_health::pb::HealthCheckRequest;
    use tonic_health::pb::health_check_response::ServingStatus;
    use tonic_health::pb::health_client::HealthClient;

    const SERVICE: &str = "golem.shardmanager.v1.ShardManagerService";

    async fn status(client: &mut HealthClient<tonic::transport::Channel>) -> i32 {
        client
            .check(HealthCheckRequest {
                service: SERVICE.to_string(),
            })
            .await
            .expect("the health service answers")
            .into_inner()
            .status
    }

    #[test]
    async fn the_service_is_not_serving_once_the_lease_belief_lapses() {
        let (reporter, service) = tonic_health::server::health_reporter();
        reporter
            .set_serving::<ShardManagerServiceServer<ShardManagerServiceImpl>>()
            .await;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(
            tonic::transport::Server::builder()
                .add_service(service)
                .serve_with_incoming(tokio_stream::wrappers::TcpListenerStream::new(listener)),
        );
        let channel = tonic::transport::Endpoint::from_shared(format!("http://127.0.0.1:{port}"))
            .unwrap()
            .connect()
            .await
            .unwrap();
        let mut client = HealthClient::new(channel);

        let belief = LeaseBelief::until(Instant::now() + Duration::from_millis(600));
        tokio::spawn(report_lease_health(
            belief.clone(),
            reporter,
            LeaderGate::new(),
            Duration::from_millis(100),
        ));

        assert_eq!(status(&mut client).await, ServingStatus::Serving as i32);
        tokio::time::sleep(Duration::from_millis(1200)).await;
        assert_eq!(status(&mut client).await, ServingStatus::NotServing as i32);

        // A late acknowledgement or a reconnect brings the belief back, and the gate serves again.
        belief.rearm_for(Duration::from_secs(30));
        tokio::time::sleep(Duration::from_millis(400)).await;
        assert_eq!(status(&mut client).await, ServingStatus::Serving as i32);
    }

    #[test]
    async fn a_renewed_belief_keeps_the_service_serving() {
        let (reporter, service) = tonic_health::server::health_reporter();
        reporter
            .set_serving::<ShardManagerServiceServer<ShardManagerServiceImpl>>()
            .await;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(
            tonic::transport::Server::builder()
                .add_service(service)
                .serve_with_incoming(tokio_stream::wrappers::TcpListenerStream::new(listener)),
        );
        let channel = tonic::transport::Endpoint::from_shared(format!("http://127.0.0.1:{port}"))
            .unwrap()
            .connect()
            .await
            .unwrap();
        let mut client = HealthClient::new(channel);

        let belief = LeaseBelief::until(Instant::now() + Duration::from_secs(1));
        tokio::spawn(report_lease_health(
            belief.clone(),
            reporter,
            LeaderGate::new(),
            Duration::from_millis(100),
        ));
        for _ in 0..8 {
            tokio::time::sleep(Duration::from_millis(200)).await;
            belief.rearm_for(Duration::from_secs(1));
            assert_eq!(status(&mut client).await, ServingStatus::Serving as i32);
        }
    }

    #[test]
    async fn a_stopped_gate_stays_not_serving_when_the_belief_comes_back() {
        let (reporter, service) = tonic_health::server::health_reporter();
        reporter
            .set_serving::<ShardManagerServiceServer<ShardManagerServiceImpl>>()
            .await;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(
            tonic::transport::Server::builder()
                .add_service(service)
                .serve_with_incoming(tokio_stream::wrappers::TcpListenerStream::new(listener)),
        );
        let channel = tonic::transport::Endpoint::from_shared(format!("http://127.0.0.1:{port}"))
            .unwrap()
            .connect()
            .await
            .unwrap();
        let mut client = HealthClient::new(channel);

        let gate: LeaderGate<ShardManagerGrpcService> = LeaderGate::new();
        let belief = LeaseBelief::until(Instant::now() + Duration::from_millis(300));
        tokio::spawn(report_lease_health(
            belief.clone(),
            reporter.clone(),
            gate.clone(),
            Duration::from_millis(50),
        ));
        tokio::time::sleep(Duration::from_millis(600)).await;

        // The replica stops: the entry says so, and the belief then comes back, as a late
        // acknowledgement can bring it.
        gate.stop();
        reporter
            .set_not_serving::<ShardManagerServiceServer<ShardManagerServiceImpl>>()
            .await;
        belief.rearm_for(Duration::from_secs(30));
        tokio::time::sleep(Duration::from_millis(400)).await;
        assert_eq!(status(&mut client).await, ServingStatus::NotServing as i32);
    }
}

#[cfg(test)]
mod startup_supervision_tests {
    use super::unless_a_task_fails;
    use test_r::test;
    use tokio::task::JoinSet;

    /// A set holding a task that has already failed by the time the set is first polled.
    async fn a_set_with_a_failed_task() -> JoinSet<anyhow::Result<()>> {
        let mut join_set: JoinSet<anyhow::Result<()>> = JoinSet::new();
        join_set.spawn(async { Err(anyhow::anyhow!("the lease was lost")) });
        // Let the task run to its end, so that its result is waiting in the set.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        join_set
    }

    #[test]
    // Without the bias `select!` picks a ready branch at random, so one round would pass by luck
    // half of the time; sixty-four rounds make that negligible.
    async fn a_task_that_already_failed_wins_over_a_step_that_is_ready() {
        for round in 0..64 {
            let mut join_set = a_set_with_a_failed_task().await;

            let result =
                unless_a_task_fails(&mut join_set, std::future::ready(anyhow::Ok(7))).await;

            let err = result.expect_err("the failed task must end the step, not the step's value");
            assert!(
                format!("{err:#}").contains("the lease was lost"),
                "round {round}: the step's value won over a failure that was already there: \
                 {err:#}"
            );
        }
    }

    #[test]
    async fn a_step_completes_while_the_tasks_are_healthy() {
        let mut join_set: JoinSet<anyhow::Result<()>> = JoinSet::new();
        join_set.spawn(std::future::pending());

        let result = unless_a_task_fails(&mut join_set, async {
            tokio::task::yield_now().await;
            anyhow::Ok(7)
        })
        .await;

        assert_eq!(
            result.expect("a healthy task set must not fail the step"),
            7
        );
    }
}

#[cfg(test)]
mod advertised_host_tests {
    use super::{advertised_address, validate_advertised_host};
    use crate::config::{EtcdConfig, GrpcApiConfig, PersistenceConfig, ShardManagerConfig};
    use golem_service_base::clients::shard_manager_leader::LeaderAddress;
    use test_r::test;

    fn distributed(host: Option<&str>) -> ShardManagerConfig {
        ShardManagerConfig {
            persistence: PersistenceConfig::Etcd(EtcdConfig::default()),
            grpc: GrpcApiConfig {
                advertised_host: host.map(str::to_string),
                ..GrpcApiConfig::default()
            },
            ..ShardManagerConfig::default()
        }
    }

    #[test]
    fn a_missing_or_blank_host_is_refused() {
        for host in [None, Some(""), Some("   ")] {
            let error = validate_advertised_host(&distributed(host)).expect_err("must be refused");
            assert!(
                error.to_string().contains("grpc.advertised_host"),
                "{error}"
            );
        }
    }

    #[test]
    fn an_unspecified_address_is_refused() {
        for host in ["0.0.0.0", "::", "[::]"] {
            validate_advertised_host(&distributed(Some(host))).expect_err(host);
        }
    }

    #[test]
    fn a_loopback_host_starts_with_a_warning() {
        for host in ["127.0.0.1", "::1", "localhost", "LOCALHOST"] {
            let named = validate_advertised_host(&distributed(Some(host)))
                .expect(host)
                .expect("a loopback host must warn");
            assert_eq!(named, host.to_string());
        }
    }

    #[test]
    fn a_routable_host_is_accepted_silently() {
        for host in [
            "10.0.3.7",
            "fd00::7",
            "golem-shard-manager-1",
            "sm-0.sm.golem.svc",
        ] {
            assert_eq!(
                validate_advertised_host(&distributed(Some(host))).expect(host),
                None
            );
        }
    }

    #[test]
    fn an_ipv6_host_is_accepted_without_brackets() {
        let address = advertised_address(&distributed(Some("[fd00::7]")), 9002).unwrap();
        assert_eq!(address.host, "fd00::7");
        assert_eq!(address.port, 9002);
    }

    #[test]
    fn advertised_address_strips_brackets_and_uses_the_advertised_port() {
        let mut config = distributed(Some("[::1]"));
        config.grpc.advertised_port = Some(19092);
        let address = advertised_address(&config, 9092).unwrap();
        assert_eq!(
            address,
            LeaderAddress {
                host: "::1".to_string(),
                port: 19092
            }
        );
    }

    #[test]
    fn advertised_address_defaults_to_the_bound_port() {
        assert_eq!(
            advertised_address(&distributed(Some("10.0.0.7")), 9092)
                .unwrap()
                .port,
            9092
        );
    }

    #[test]
    fn an_advertised_port_of_zero_is_refused() {
        let mut config = distributed(Some("10.0.0.7"));
        config.grpc.advertised_port = Some(0);
        assert!(validate_advertised_host(&config).is_err());
    }

    #[test]
    fn local_mode_does_not_need_a_host() {
        assert_eq!(
            validate_advertised_host(&ShardManagerConfig::default()).unwrap(),
            None
        );
    }
}

pub static DB_MIGRATIONS: include_dir::Dir = include_dir!("$CARGO_MANIFEST_DIR/db/migration");

pub struct RunDetails {
    pub http_port: u16,
    pub grpc_port: u16,
    pub leadership: Option<LeadershipHandle>,
    /// What the replica's stop acts on; see [`serve_until_stopped`]. `None` is a value built by
    /// hand: a stop then refuses nothing and closes no server.
    pub stop: Option<ServerStop>,
}

/// The connections that the listener accepts, as a stream for the server. The listener is dropped
/// when `stop` fires, or when the stream is, so that a client which dials a stopping replica is
/// refused at once and not left waiting in a queue that nobody serves.
fn accept_until(
    listener: tokio::net::TcpListener,
    stop: CancellationToken,
) -> tokio_stream::wrappers::ReceiverStream<std::io::Result<tokio::net::TcpStream>> {
    let (accepted_tx, accepted_rx) = tokio::sync::mpsc::channel(1);
    tokio::spawn(async move {
        loop {
            tokio::select! {
                biased;
                () = stop.cancelled() => break,
                () = accepted_tx.closed() => break,
                accepted = listener.accept() => {
                    let connection = accepted.map(|(stream, _)| stream);
                    if accepted_tx.send(connection).await.is_err() {
                        break;
                    }
                }
            }
        }
    });
    tokio_stream::wrappers::ReceiverStream::new(accepted_rx)
}

/// How long a stopping replica waits for the calls that are running to finish before its server
/// is dropped. A shard manager call is short; this bounds a stuck one so that it cannot hold the
/// stop, and with it the pod's termination, for the grace period of the orchestrator.
const SERVER_DRAIN_LIMIT: Duration = Duration::from_secs(5);

struct ServerInner {
    /// Asks the server to stop accepting and to close its connections once their calls finish.
    shutdown: CancellationToken,
    closed: tokio::sync::watch::Receiver<bool>,
    /// Set by a stop that closes the server itself, after the leadership is released.
    closed_by_stop: std::sync::atomic::AtomicBool,
}

/// Asks the server to close when the task that holds this is dropped, unless a stop is in charge
/// of that: whoever drops or aborts the task set of a replica ends its server too, as it did when
/// the server ran in that set. A stop sets `closed_by_stop` first, because it aborts that task
/// while the server must still answer for the gate. The server then closes within
/// [`SERVER_DRAIN_LIMIT`].
struct CloseServerOnDrop(Arc<ServerInner>);

impl Drop for CloseServerOnDrop {
    fn drop(&mut self) {
        if !self
            .0
            .closed_by_stop
            .load(std::sync::atomic::Ordering::SeqCst)
        {
            self.0.shutdown.cancel();
        }
    }
}

/// The parts of a replica that its stop acts on, in the order it acts on them: the gate stops
/// answering, then the leadership is released, then the gRPC server closes its connections.
pub struct ServerStop {
    gate: LeaderGate<ShardManagerGrpcService>,
    health_reporter: tonic_health::server::HealthReporter,
    server: Arc<ServerInner>,
}

impl ServerStop {
    /// The replica answers no call, reports not ready and not serving. Done first, so that no
    /// replica serves from a connection it accepted earlier once another may be leading.
    async fn refuse_calls(&self) {
        self.gate.stop();
        self.health_reporter
            .set_not_serving::<ShardManagerServiceServer<ShardManagerServiceImpl>>()
            .await;
        metrics::record_not_serving();
    }

    fn close_after_release(&self) {
        self.server
            .closed_by_stop
            .store(true, std::sync::atomic::Ordering::SeqCst);
    }

    /// Closes the server: it stops accepting, and its connections close once the calls running on
    /// them are done. The server task bounds that wait itself by [`SERVER_DRAIN_LIMIT`].
    async fn close(&self) {
        self.server.shutdown.cancel();
        let mut closed = self.server.closed.clone();
        // An error means the server task is gone, which is as closed as it gets.
        let _ = closed.wait_for(|closed| *closed).await;
    }
}

/// Stops a replica in the order that keeps two replicas from serving: no more answers, the tasks
/// finished, the leadership released, the server closed.
///
/// Every task is awaited before the lease is released, so no executor command can be in flight
/// once another replica may be leading.
async fn stop_replica(
    stop: Option<&ServerStop>,
    join_set: &mut JoinSet<anyhow::Result<()>>,
    leadership: Option<&LeadershipHandle>,
) {
    // If this future is dropped before the server is closed below, the server still closes.
    let _close_on_drop = stop.map(|stop| stop.server.shutdown.clone().drop_guard());
    if let Some(stop) = stop {
        stop.refuse_calls().await;
        stop.close_after_release();
    }
    join_set.shutdown().await;
    release_leadership(leadership).await;
    if let Some(stop) = stop {
        stop.close().await;
    }
}

/// Startup's persistence wiring; the leadership handle is present only in distributed mode.
type Persistence = (
    Arc<dyn RoutingTablePersistence>,
    Arc<dyn crate::quota::QuotaRepo>,
    Option<LeadershipHandle>,
    Option<LeaseBelief>,
);

/// The shard manager service as the gate serves it once this replica leads.
type ShardManagerGrpcService = ShardManagerServiceServer<ShardManagerServiceImpl>;

/// Whether this process is a dedicated shard manager or is embedded in the single `golem` binary.
#[derive(Clone, Debug)]
pub enum Deployment {
    /// The `golem-shard-manager` binary. May block until elected.
    Standalone { shutdown: CancellationToken },
    /// The `golem` single binary. `run()` must return promptly.
    Embedded,
}

/// Campaigns for leadership, failing if an already-running task dies instead of waiting forever.
async fn campaign_watching_startup(
    election: &LeaderElection,
    join_set: &mut JoinSet<anyhow::Result<()>>,
) -> anyhow::Result<Elected> {
    // Scoped so the campaign future is dropped before the cleanup below: while it is alive it
    // borrows `election`, and it is precisely a dropped campaign that leaves a lease behind.
    let outcome = {
        let mut campaign = std::pin::pin!(election.campaign_until_elected());

        loop {
            tokio::select! {
                elected = &mut campaign => break elected.map_err(anyhow::Error::from),
                joined = join_set.join_next(), if !join_set.is_empty() => match joined {
                    Some(Err(err)) => {
                        break Err(anyhow::Error::new(err).context(
                            "a shard manager task panicked while campaigning for leadership",
                        ));
                    }
                    Some(Ok(Err(err))) => {
                        break Err(err
                            .context("a shard manager task failed while campaigning for leadership"));
                    }
                    Some(Ok(Ok(()))) => {
                        warn!("A shard manager task finished while campaigning for leadership")
                    }
                    None => {}
                }
            }
        }
    };

    if outcome.is_err() {
        election.revoke_pending_lease().await;
    }
    outcome
}

/// Runs one step of the startup, failing it as soon as an already-running task fails: a lost
/// leadership lease, reported by the keepalive task, must end the startup at once rather than
/// after the step completes.
async fn unless_a_task_fails<T>(
    join_set: &mut JoinSet<anyhow::Result<()>>,
    step: impl Future<Output = anyhow::Result<T>>,
) -> anyhow::Result<T> {
    let mut step = std::pin::pin!(step);
    loop {
        // Biased towards the tasks: a failure that is already there must win over a step that
        // completes in the same poll, or a lost lease could still be followed by a successful
        // readiness publication.
        tokio::select! {
            biased;
            joined = join_set.join_next(), if !join_set.is_empty() => match joined {
                Some(Err(err)) => {
                    return Err(anyhow::Error::new(err)
                        .context("a task panicked during the shard manager's startup"));
                }
                Some(Ok(Err(err))) => {
                    return Err(err.context("a task failed during the shard manager's startup"));
                }
                Some(Ok(Ok(()))) => warn!("A task finished during the shard manager's startup"),
                None => {}
            },
            done = &mut step => return done,
        }
    }
}

pub(crate) fn ensure_shard_count_matches(
    stored: usize,
    configured: usize,
) -> Result<(), ShardManagerError> {
    if stored != configured {
        return Err(ShardManagerError::Internal(format!(
            "the persisted shard lease state was written with {stored} shards, but this shard \
             manager is configured for {configured}. The stored value governs routing, so \
             starting would silently ignore the configuration; changing the shard count is not \
             supported."
        )));
    }
    Ok(())
}

/// Blocks until elected, returning the etcd client and the fence every state write is guarded on.
async fn start_distributed_mode(
    etcd: &EtcdConfig,
    deployment: &Deployment,
    number_of_shards: usize,
    gate: &LeaderGate<ShardManagerGrpcService>,
    own: LeaderAddress,
    join_set: &mut JoinSet<anyhow::Result<()>>,
) -> anyhow::Result<(Client, LeaderFence, LeadershipHandle, LeaseBelief)> {
    let shutdown = match deployment {
        Deployment::Standalone { shutdown } => shutdown.clone(),
        Deployment::Embedded => anyhow::bail!(
            "etcd persistence selects distributed mode, which campaigns for \
             leadership and blocks until elected. The single-binary server starts the \
             shard manager inline and would never finish starting. Use Sqlite or \
             Postgres persistence for the embedded server."
        ),
    };

    anyhow::ensure!(
        etcd.leader_lease_ttl.subsec_nanos() == 0,
        "persistence.config.leader_lease_ttl must be a whole number of seconds, but \
         is {:?}; etcd would silently truncate it.",
        etcd.leader_lease_ttl
    );
    anyhow::ensure!(
        etcd.leader_lease_ttl >= Duration::from_secs(2),
        "persistence.config.leader_lease_ttl must be at least 2s (etcd's MinLeaseTTL), \
         but is {:?}; etcd would silently clamp it up.",
        etcd.leader_lease_ttl
    );
    anyhow::ensure!(
        etcd.request_timeout <= etcd.leader_lease_ttl / 2,
        "persistence.config.request_timeout must be at most half of \
         persistence.config.leader_lease_ttl, but is {:?} against a {:?} lease TTL; a lease could \
         then expire before its first renewal.",
        etcd.request_timeout,
        etcd.leader_lease_ttl
    );

    let kv = connect_for_requests(etcd).await?;
    info!(
        endpoints = etcd.endpoints.join(", "),
        state_key = STATE_KEY,
        "Configured the etcd client for shard lease state persistence"
    );

    let election = LeaderElection::connect(etcd, LEADER_ELECTION_NAME, own.clone())
        .await?
        .with_shutdown(shutdown.clone());
    let election = match &etcd.test_version {
        Some(version) => election.with_version(version.clone()),
        None => election,
    };
    let (watch, watcher) = election.watch();
    // The campaign reads the same view, to find a replica that advertises this one's address.
    let election = election.with_watch(watch.clone());
    join_set.spawn(watcher.run().in_current_span());
    // From here a call to this replica is answered from the watched view of the election, which
    // costs no etcd read per call.
    gate.stand_by(Arc::new(watch.clone()), own.clone(), etcd.leader_lease_ttl);

    metrics::record_standing_by();

    let stored_count = retry_retriable(
        "reading the stored shard count",
        || async {
            EtcdRoutingTablePersistence::stored_number_of_shards(&kv)
                .await
                .inspect_err(|err| {
                    if is_retriable_read(err) {
                        metrics::record_campaign_attempt_failure();
                    }
                })
        },
        &shutdown,
        etcd.retry_min_delay,
        etcd.retry_max_delay,
    )
    .await?;
    if let Some(stored) = stored_count {
        ensure_shard_count_matches(stored, number_of_shards)?;
    }

    let elected = campaign_watching_startup(&election, join_set).await?;

    metrics::record_elected(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|since| since.as_secs_f64())
            .unwrap_or_default(),
    );

    // The keepalive can only end in an error, and that error ends the process: a lost lease must
    // stop this replica rather than leave it serving a table it can no longer write.
    let belief = elected.keepalive.belief();
    let keepalive = elected.keepalive;
    let leadership = elected.leadership;
    let stepping_down = leadership.clone();
    join_set.spawn(
        async move {
            let lost = keepalive.run().await;
            metrics::record_standing_by();
            if stepping_down.has_stepped_down() {
                info!(error = %lost, "The etcd leadership lease ended after stepping down");
            } else {
                error!(error = %lost, "Lost etcd leadership");
            }
            Err(anyhow::Error::new(lost).context(
                "the shard manager lost its etcd leader lease; exiting so a standby \
                 can take over",
            ))
        }
        .in_current_span(),
    );

    Ok((kv, elected.fence, leadership, belief))
}

/// The advertised host as written, without the brackets an IPv6 literal may carry.
fn advertised_host(config: &ShardManagerConfig) -> Option<&str> {
    config
        .grpc
        .advertised_host
        .as_deref()
        .map(|host| host.trim().trim_start_matches('[').trim_end_matches(']'))
        .filter(|host| !host.is_empty())
}

/// Checks the host a replica publishes for its gRPC port. Only distributed mode uses it: there
/// clients connect to the leader at that host. Returns the host when it works only when every
/// process runs on this machine, so that the caller can warn about it.
fn validate_advertised_host(config: &ShardManagerConfig) -> anyhow::Result<Option<String>> {
    if !matches!(config.persistence, PersistenceConfig::Etcd(_)) {
        return Ok(None);
    }
    let host = advertised_host(config).context(
        "grpc.advertised_host must be set in distributed mode: it is the address clients connect \
         to when this replica leads. On Kubernetes, set GOLEM__GRPC__ADVERTISED_HOST from the pod \
         IP (fieldRef status.podIP).",
    )?;
    let ip = host.parse::<std::net::IpAddr>().ok();
    anyhow::ensure!(
        !ip.is_some_and(|ip| ip.is_unspecified()),
        "grpc.advertised_host is {host}, which names no single host. Set it to the address other \
         processes reach this replica at, such as its pod IP."
    );
    anyhow::ensure!(
        config.grpc.advertised_port != Some(0),
        "grpc.advertised_port is 0, which no client can connect to. Leave it unset to advertise \
         the bound port."
    );
    let local_only = match ip {
        Some(ip) => ip.is_loopback(),
        None => host.eq_ignore_ascii_case("localhost"),
    };
    Ok(local_only.then(|| host.to_string()))
}

/// Where this replica serves gRPC to other processes: the advertised host, and the advertised port
/// or else the bound one.
fn advertised_address(
    config: &ShardManagerConfig,
    grpc_port: u16,
) -> anyhow::Result<LeaderAddress> {
    let host =
        advertised_host(config).context("grpc.advertised_host must be set in distributed mode")?;
    Ok(LeaderAddress {
        host: host.to_string(),
        port: config.grpc.advertised_port.unwrap_or(grpc_port),
    })
}

/// How long one attempt to reach one address of the replica's own advertised host may take.
const OWN_ADDRESS_ATTEMPT_TIMEOUT: Duration = Duration::from_secs(2);
/// The pause between two attempts to reach the replica's own advertised address.
const OWN_ADDRESS_RETRY_INTERVAL: Duration = Duration::from_secs(1);
/// The least time between two warnings that the advertised address does not reach the replica.
const OWN_ADDRESS_WARNING_INTERVAL: Duration = Duration::from_secs(30);

/// One attempt to reach the replica's own advertised address from the inside, the way a client
/// that was sent there would.
///
/// Without server TLS the attempt asks the gRPC health service, which confirms that a gRPC server
/// answers at the address. With server TLS it stops at the TCP connect: a TLS client would need
/// the CA certificate that clients are given, which this process does not hold.
async fn reach_own_address(own: &LeaderAddress, tls: bool) -> anyhow::Result<()> {
    // A name may resolve to several addresses, some of which this replica cannot reach; each one
    // gets its own short attempt, and one that works is enough.
    let mut last_error = anyhow::anyhow!("{own} resolves to no address");
    let addresses = tokio::time::timeout(
        OWN_ADDRESS_ATTEMPT_TIMEOUT,
        tokio::net::lookup_host((own.host.as_str(), own.port)),
    )
    .await
    .map_err(|_| anyhow::anyhow!("no answer to the lookup of {own}"))??;
    for address in addresses {
        let reached = tokio::time::timeout(
            OWN_ADDRESS_ATTEMPT_TIMEOUT,
            reach_resolved_address(address, tls),
        )
        .await
        .unwrap_or_else(|_| {
            Err(anyhow::anyhow!(
                "no answer from {address} within {OWN_ADDRESS_ATTEMPT_TIMEOUT:?}"
            ))
        });
        match reached {
            Ok(()) => return Ok(()),
            Err(error) => last_error = error,
        }
    }
    Err(last_error)
}

/// [`reach_own_address`] for one of the addresses that the advertised host resolves to.
async fn reach_resolved_address(address: std::net::SocketAddr, tls: bool) -> anyhow::Result<()> {
    if tls {
        tokio::net::TcpStream::connect(address).await?;
        return Ok(());
    }
    // The channel is made for the resolved address, so that the attempt tests that address and not
    // whichever one a second lookup would give first.
    let channel = tonic::transport::Endpoint::from_shared(format!("http://{address}"))?
        .connect()
        .await?;
    let answer = tonic_health::pb::health_client::HealthClient::new(channel)
        .check(tonic_health::pb::HealthCheckRequest {
            service: String::new(),
        })
        .await?
        .into_inner();
    anyhow::ensure!(
        answer.status == tonic_health::pb::health_check_response::ServingStatus::Serving as i32,
        "the gRPC server at the address does not report that it is serving"
    );
    Ok(())
}

/// Retries [`reach_own_address`] until it works and then lets the gate say that the replica is
/// ready. A wrong `grpc.advertised_host` or `grpc.advertised_port` passes every other check, and
/// the clients that are sent to such an address find nothing.
///
/// A replica whose network does not route a pod's own traffic to its externally advertised address
/// stays unready as well.
async fn confirm_own_address(
    own: LeaderAddress,
    tls: bool,
    gate: LeaderGate<ShardManagerGrpcService>,
) -> anyhow::Result<()> {
    let mut last_warning: Option<tokio::time::Instant> = None;
    loop {
        let attempt = reach_own_address(&own, tls).await;
        match attempt {
            Ok(()) => break,
            Err(error) => {
                if last_warning.is_none_or(|at| at.elapsed() >= OWN_ADDRESS_WARNING_INTERVAL) {
                    last_warning = Some(tokio::time::Instant::now());
                    warn!(
                        advertised_address = %own,
                        error = format!("{error:#}"),
                        "This replica cannot reach its own advertised address, so it stays not \
                         ready. Check grpc.advertised_host and grpc.advertised_port"
                    );
                }
            }
        }
        tokio::time::sleep(OWN_ADDRESS_RETRY_INTERVAL).await;
    }
    gate.own_address_reached();
    info!(advertised_address = %own, "This replica reaches its own advertised address");
    // A task that finishes during the startup is reported as a finished task; this one has
    // nothing left to do, and ends with the others.
    std::future::pending().await
}

/// Checks the configured startup, persistence, retry, and lease timings, returning warnings worth
/// logging and refusing outright combinations that cannot work.
///
/// The executor renews at a third of the lease and gives up on one attempt at half of that, never
/// below a floor, and the shard manager needs its own write budget to fit inside that attempt.
/// Those relations live in [`golem_common::base_model::shard_lease`]; this is where the configured
/// duration meets them, because the lease duration is the shard manager's config and the executor
/// only ever learns it from the wire.
fn validate_timing_config(config: &ShardManagerConfig) -> anyhow::Result<Vec<String>> {
    anyhow::ensure!(
        !config.state_read_timeout.is_zero(),
        "state_read_timeout must be greater than zero"
    );
    anyhow::ensure!(
        !config.state_write_timeout.is_zero()
            && config.state_write_timeout <= shard_lease::max_state_write_timeout(),
        "state_write_timeout must be greater than zero and at most {:?}, but is {:?}",
        shard_lease::max_state_write_timeout(),
        config.state_write_timeout
    );
    anyhow::ensure!(
        config.state_read_timeout > config.state_write_timeout,
        "state_read_timeout must be greater than state_write_timeout, but is {:?} against {:?}",
        config.state_read_timeout,
        config.state_write_timeout
    );
    anyhow::ensure!(
        !config.initial_health_check_timeout.is_zero(),
        "initial_health_check_timeout must be greater than zero"
    );
    if let PersistenceConfig::Etcd(etcd) = &config.persistence {
        anyhow::ensure!(
            !etcd.read_retry_timeout.is_zero(),
            "persistence.config.read_retry_timeout must be greater than zero"
        );
        anyhow::ensure!(
            std::time::Instant::now()
                .checked_add(etcd.read_retry_timeout)
                .is_some(),
            "persistence.config.read_retry_timeout is too large for a retry deadline"
        );
        anyhow::ensure!(
            !etcd.retry_min_delay.is_zero(),
            "persistence.config.retry_min_delay must be greater than zero"
        );
        anyhow::ensure!(
            etcd.retry_max_delay >= etcd.retry_min_delay,
            "persistence.config.retry_max_delay must be at least retry_min_delay, but is {:?} \
             against {:?}",
            etcd.retry_max_delay,
            etcd.retry_min_delay
        );
        anyhow::ensure!(
            tokio::time::Instant::now()
                .checked_add(etcd.retry_max_delay)
                .is_some(),
            "persistence.config.retry_max_delay is too large for a retry backoff"
        );
        anyhow::ensure!(
            etcd.read_retry_timeout
                .checked_add(etcd.request_timeout)
                .is_some_and(|read_with_final_attempt| {
                    read_with_final_attempt <= config.state_read_timeout
                }),
            "persistence.config.read_retry_timeout plus request_timeout must fit within \
             state_read_timeout, but {:?} plus {:?} exceeds {:?}",
            etcd.read_retry_timeout,
            etcd.request_timeout,
            config.state_read_timeout
        );
    }
    anyhow::ensure!(
        !config.shard_lease_duration.is_zero(),
        "shard_lease_duration must be greater than zero"
    );
    anyhow::ensure!(
        chrono::Duration::from_std(config.shard_lease_duration).is_ok(),
        "shard_lease_duration {:?} is out of range",
        config.shard_lease_duration
    );
    anyhow::ensure!(
        config.shard_lease_duration >= shard_lease::min_shard_lease_duration(),
        "shard_lease_duration must be at least {:?}, but is {:?}. An executor gives up on one \
         renewal after {:?} at the shortest, which is the whole gap between two renewals at this \
         lease: a single unanswered call would cost the lease with no attempt left to save it.",
        shard_lease::min_shard_lease_duration(),
        config.shard_lease_duration,
        shard_lease::rpc_deadline_floor(),
    );

    let mut warnings = Vec::new();
    if config.shard_lease_duration < shard_lease::recommended_min_shard_lease_duration() {
        warnings.push(format!(
            "shard_lease_duration is {:?}; an executor gets fewer than the {} renewal attempts a \
             lease is meant to survive, because the per-attempt deadline is held up by its {:?} \
             floor. {:?} is the shortest lease that delivers all of them.",
            config.shard_lease_duration,
            shard_lease::SHARD_LEASE_RENEWAL_ATTEMPTS,
            shard_lease::rpc_deadline_floor(),
            shard_lease::recommended_min_shard_lease_duration(),
        ));
    }
    Ok(warnings)
}

pub async fn run(
    shard_manager_config: &ShardManagerConfig,
    deployment: Deployment,
    registry: Registry,
    join_set: &mut JoinSet<anyhow::Result<()>>,
) -> anyhow::Result<RunDetails> {
    debug!("Initializing shard manager");

    for warning in validate_timing_config(shard_manager_config)? {
        warn!(
            warning,
            "The shard manager timing configuration is inconsistent"
        );
    }
    if let Some(host) = validate_advertised_host(shard_manager_config)? {
        warn!(
            advertised_host = host,
            "grpc.advertised_host is a loopback name: clients can reach this replica only if \
             they run on the same host"
        );
    }

    let gate: LeaderGate<ShardManagerGrpcService> = LeaderGate::new();

    let (health_reporter, health_service) = tonic_health::server::health_reporter();
    // NOT_SERVING until this replica serves: a standby accepts calls only to send them on. The
    // overall status ("") stays SERVING, because the process is healthy.
    health_reporter
        .set_not_serving::<ShardManagerServiceServer<ShardManagerServiceImpl>>()
        .await;

    let reflection_service = tonic_reflection::server::Builder::configure()
        .register_encoded_file_descriptor_set(proto::FILE_DESCRIPTOR_SET)
        .build_v1()?;

    let http_port = golem_service_base::observability::start_health_metrics_and_readiness_server(
        shard_manager_config.http_addr(),
        registry,
        "shard manager is running",
        {
            let gate = gate.clone();
            move || gate.is_ready()
        },
        join_set,
    )
    .await?;

    let shard_manager_config = Arc::new(shard_manager_config.clone());

    let worker_executors = Arc::new(WorkerExecutorServiceDefault::new(
        shard_manager_config.worker_executors.clone(),
    ));

    let health_check: Arc<dyn HealthCheck> = match &shard_manager_config.health_check.mode {
        HealthCheckMode::Grpc(_) => Arc::new(GrpcHealthCheck::new(
            worker_executors.clone(),
            shard_manager_config.worker_executors.retries.clone(),
            shard_manager_config.health_check.silent,
        )),
        #[cfg(feature = "kubernetes")]
        HealthCheckMode::K8s(HealthCheckK8sConfig { namespace }) => Arc::new(
            crate::sharding::healthcheck::kubernetes::KubernetesHealthCheck::new(
                namespace.clone(),
                shard_manager_config.worker_executors.retries.clone(),
                shard_manager_config.health_check.silent,
            )
            .await
            .context("failed to build the Kubernetes API client for the health checker")?,
        ),
    };

    // Opened before the persistence below, which in distributed mode blocks in the campaign.
    // Every replica serves this port from the start, so that an orchestrator sees each one as
    // ready; until this replica leads, the gate answers for the service.
    let listener = golem_service_base::net::bind_tcp_listener(shard_manager_config.grpc_addr())?;
    let grpc_port = listener.local_addr()?.port();

    let server_shutdown = CancellationToken::new();
    let server_gate = gate.clone();
    let (closed_tx, closed_rx) = tokio::sync::watch::channel(false);
    let mut server = Server::builder();
    if let GrpcServerTlsConfig::Enabled(tls) = &shard_manager_config.grpc.tls {
        server = server.tls_config(tls.to_tonic())?;
    }
    // Its own task, outside the task set: the set is stopped before the leadership is released,
    // and the server must still be answering for the gate then. It closes when the stop says so,
    // or when the task that watches it below is dropped.
    let task_shutdown = server_shutdown.clone();
    let server_task = tokio::spawn(
        async move {
            let serving = server
                .layer(
                    middleware::server::OtelGrpcLayer::default()
                        .filter(filters::reject_healthcheck),
                )
                .add_service(reflection_service)
                .add_service(GatedService::new(server_gate))
                .add_service(health_service)
                .serve_with_incoming_shutdown(accept_until(listener, task_shutdown.clone()), {
                    let shutdown = task_shutdown.clone();
                    async move { shutdown.cancelled().await }
                });
            let mut serving = std::pin::pin!(serving);
            // Once the stop has begun, the connections get `SERVER_DRAIN_LIMIT` to finish their
            // calls and close; then the server is dropped, with whatever is left.
            let drain_limit = async {
                task_shutdown.cancelled().await;
                tokio::time::sleep(SERVER_DRAIN_LIMIT).await;
            };
            let served = tokio::select! {
                served = &mut serving => served,
                () = drain_limit => {
                    warn!(
                        limit = ?SERVER_DRAIN_LIMIT,
                        "Connections of the shard manager's server did not close in time; dropping it"
                    );
                    Ok(())
                }
            };
            let _ = closed_tx.send(true);
            served.map_err(anyhow::Error::from)
        }
        .in_current_span(),
    );
    let server_inner = Arc::new(ServerInner {
        shutdown: server_shutdown,
        closed: closed_rx,
        closed_by_stop: std::sync::atomic::AtomicBool::new(false),
    });
    let stop = ServerStop {
        gate: gate.clone(),
        health_reporter: health_reporter.clone(),
        server: server_inner.clone(),
    };
    join_set.spawn({
        let close_on_drop = CloseServerOnDrop(server_inner.clone());
        async move {
            let _close_on_drop = close_on_drop;
            server_task.await?
        }
    });

    let (persistence_service, quota_repo, leadership, lease_belief): Persistence = {
        use golem_service_base::db;
        use golem_service_base::migration::{IncludedMigrationsDir, Migrations};

        let migrations = IncludedMigrationsDir::new(&DB_MIGRATIONS);

        match &shard_manager_config.persistence {
            PersistenceConfig::Postgres(postgres) => {
                db::postgres::migrate(postgres, migrations.postgres_migrations()).await?;
                let pool = db::postgres::PostgresPool::configured(postgres).await?;

                let pool_for_metrics = pool.clone();
                join_set
                    .spawn(async move { pool_for_metrics.run_metrics_loop("shard_manager").await });

                (
                    Arc::new(DbRoutingTablePersistence::new(
                        pool.clone(),
                        shard_manager_config.number_of_shards,
                    )),
                    Arc::new(DbQuotaRepo::logged(pool)),
                    None,
                    None,
                )
            }
            PersistenceConfig::Sqlite(sqlite) => {
                db::sqlite::migrate(sqlite, migrations.sqlite_migrations()).await?;
                let pool = db::sqlite::SqlitePool::configured(sqlite).await?;

                (
                    Arc::new(DbRoutingTablePersistence::new(
                        pool.clone(),
                        shard_manager_config.number_of_shards,
                    )),
                    Arc::new(DbQuotaRepo::logged(pool)),
                    None,
                    None,
                )
            }
            PersistenceConfig::Etcd(etcd) => {
                let own = advertised_address(&shard_manager_config, grpc_port)?;
                gate.require_own_address_check();
                join_set.spawn(
                    confirm_own_address(
                        own.clone(),
                        matches!(
                            shard_manager_config.grpc.tls,
                            GrpcServerTlsConfig::Enabled(_)
                        ),
                        gate.clone(),
                    )
                    .in_current_span(),
                );
                let (kv, fence, leadership, belief) = start_distributed_mode(
                    etcd,
                    &deployment,
                    shard_manager_config.number_of_shards,
                    &gate,
                    own,
                    join_set,
                )
                .await?;

                // Distributed mode: the shard lease state and the quota state both live in etcd,
                // over one connection, and every write to either carries the leadership fence.
                (
                    Arc::new(EtcdRoutingTablePersistence::with_client(
                        kv.clone(),
                        shard_manager_config.number_of_shards,
                        fence.clone(),
                        etcd.compaction_retention_revisions,
                        ReadRetry::from_config(etcd),
                    )),
                    Arc::new(EtcdQuotaRepo::logged(
                        kv,
                        fence,
                        ReadRetry::from_config(etcd),
                    )),
                    Some(leadership),
                    Some(belief),
                )
            }
        }
    };

    let startup = async {
        let registry_service = Arc::new(GrpcRegistryService::new(
            &shard_manager_config.registry_service,
        ));

        let fetcher: Arc<dyn crate::quota::ResourceDefinitionFetcher> =
            Arc::new(GrpcResourceDefinitionFetcher::new(
                registry_service.clone(),
                &shard_manager_config.resource_definition_fetcher,
            ));

        let quota_service = QuotaService::new(
            shard_manager_config.quota.clone(),
            fetcher.clone(),
            quota_repo,
        );
        unless_a_task_fails(join_set, async {
            quota_service.restore_state().await?;
            anyhow::Ok(())
        })
        .await?;

        join_set.spawn({
            let quota_service = quota_service.clone();
            async move {
                ShardManagerRegistryInvalidationHandler::run(
                    registry_service,
                    fetcher,
                    quota_service,
                )
                .await;
                Ok(())
            }
        });

        // Awaited under supervision; only the spawn of the worker, which cannot be interrupted
        // half done, takes the task set.
        let prepared = unless_a_task_fails(join_set, async {
            Ok(ShardManagement::prepare(
                persistence_service.clone(),
                worker_executors.clone(),
                health_check.clone(),
                shard_manager_config.rebalance_threshold,
                shard_manager_config.shard_lease_duration,
                shard_manager_config.state_read_timeout,
                shard_manager_config.state_write_timeout,
                shard_manager_config.initial_health_check_timeout,
                shard_manager_config.number_of_shards,
            )
            .await?)
        })
        .await?;
        let shard_management = Arc::new(prepared.start(join_set));

        self::sharding::healthcheck_loop::start_health_check_loop(
            shard_management.clone(),
            health_check.clone(),
            &shard_manager_config.health_check,
            join_set,
        );

        gate.serve(
            ShardManagerServiceServer::new(ShardManagerServiceImpl::new(
                shard_management,
                quota_service,
            ))
            .accept_compressed(CompressionEncoding::Gzip)
            .send_compressed(CompressionEncoding::Gzip),
            lease_belief.clone(),
        );
        unless_a_task_fails(join_set, async {
            health_reporter
                .set_serving::<ShardManagerServiceServer<ShardManagerServiceImpl>>()
                .await;
            anyhow::Ok(())
        })
        .await?;
        if let (Some(belief), PersistenceConfig::Etcd(etcd)) =
            (lease_belief.clone(), &shard_manager_config.persistence)
        {
            join_set.spawn(
                report_lease_health(
                    belief,
                    health_reporter.clone(),
                    gate.clone(),
                    LeaseKeepAlive::renewal_interval(etcd.leader_lease_ttl),
                )
                .in_current_span(),
            );
        }

        // After `serve`: a standby names this leader from the moment its value is ready, and the
        // leader must already serve by then.
        if let Some(leadership) = &leadership {
            unless_a_task_fails(join_set, async {
                leadership.publish_ready(lease_belief.as_ref()).await?;
                anyhow::Ok(())
            })
            .await?;
        }
        metrics::record_serving();

        anyhow::Ok(())
    };

    let started = match &deployment {
        Deployment::Standalone { shutdown } => tokio::select! {
            biased;
            started = startup => started,
            _ = shutdown.cancelled() => Err(anyhow::Error::new(ShardManagerError::ShutdownRequested)),
        },
        Deployment::Embedded => startup.await,
    };

    if let Err(err) = started {
        // Awaited, not just requested: a task still running could reach an executor after a
        // standby has taken over.
        stop_replica(Some(&stop), join_set, leadership.as_ref()).await;
        return Err(err);
    }

    info!(grpc_port, "Started shard manager");

    Ok(RunDetails {
        http_port,
        grpc_port,
        leadership,
        stop: Some(stop),
    })
}

/// Keeps the health entry of the shard manager service in step with the gate: NOT_SERVING while
/// the lease belief has lapsed, which is when the gate refuses calls, and SERVING again if the
/// belief comes back, as a late acknowledgement or a reconnect can bring it. Without it a health
/// probe would keep reporting a leader that serves nothing until the lost lease stops the
/// process. While the belief holds, it sleeps until the deadline, but never longer than `poll`,
/// because every renewal moves the deadline and a suspended machine wakes past it; while it has
/// lapsed it looks again every `poll`. It never returns, so that the task set does not take it for
/// a stopped task. The entry is SERVING when it starts, and NOT_SERVING for good once the gate
/// has stopped.
async fn report_lease_health(
    belief: LeaseBelief,
    health_reporter: tonic_health::server::HealthReporter,
    gate: LeaderGate<ShardManagerGrpcService>,
    poll: Duration,
) -> anyhow::Result<()> {
    let mut serving = true;
    loop {
        // A stopped replica serves nothing, whatever its belief says: a belief that comes back
        // after the stop began must not turn the entry to SERVING again.
        let held = !belief.remaining().is_zero() && !gate.is_stopped();
        if held != serving {
            serving = held;
            if held {
                health_reporter
                    .set_serving::<ShardManagerServiceServer<ShardManagerServiceImpl>>()
                    .await;
            } else {
                health_reporter
                    .set_not_serving::<ShardManagerServiceServer<ShardManagerServiceImpl>>()
                    .await;
            }
        }
        tokio::time::sleep(if held {
            belief.remaining().min(poll)
        } else {
            poll
        })
        .await;
    }
}

/// A failed revoke is never propagated: the lease expires on its own, so failing the shutdown
/// would report a problem that resolves itself.
async fn release_leadership(leadership: Option<&LeadershipHandle>) {
    if let Some(leadership) = leadership
        && let Err(err) = leadership.step_down().await
    {
        warn!(
            error = %err,
            "Cannot release the shard manager leadership; the lease will expire on its own"
        );
    }
}

/// Runs the shard manager until a task fails, until every task has finished, or until `shutdown`
/// fires, and stops the replica on the way out of all three, in the order of [`stop_replica`]:
/// the gate refuses calls first, the tasks are stopped and awaited, the leadership is released,
/// and the server closes its connections.
pub async fn serve_until_stopped(
    details: RunDetails,
    mut join_set: JoinSet<anyhow::Result<()>>,
    shutdown: CancellationToken,
) -> anyhow::Result<()> {
    let stop = details.stop.as_ref();
    let leadership = details.leadership.as_ref();
    loop {
        tokio::select! {
            joined = join_set.join_next() => match joined {
                Some(Ok(Ok(()))) => warn!("A shard manager task finished"),
                Some(Ok(Err(err))) => {
                    stop_replica(stop, &mut join_set, leadership).await;
                    return Err(err.context("a shard manager task failed"));
                }
                Some(Err(panicked)) => {
                    stop_replica(stop, &mut join_set, leadership).await;
                    return Err(anyhow::Error::new(panicked)
                        .context("a shard manager task panicked"));
                }
                None => {
                    stop_replica(stop, &mut join_set, leadership).await;
                    return Ok(());
                }
            },
            _ = shutdown.cancelled() => {
                stop_replica(stop, &mut join_set, leadership).await;
                return Ok(());
            }
        }
    }
}
