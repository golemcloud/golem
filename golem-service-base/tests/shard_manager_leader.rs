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

//! The shard manager client against fake replicas: it follows a standby's answer to the leader,
//! keeps the leader, and asks its configured address again when the leader goes away.

use golem_api_grpc::proto::golem::shardmanager::RoutingTable as GrpcRoutingTable;
use golem_api_grpc::proto::golem::shardmanager::v1::shard_manager_service_server::{
    ShardManagerService, ShardManagerServiceServer,
};
use golem_api_grpc::proto::golem::shardmanager::v1::{
    AcquireQuotaLeaseRequest, AcquireQuotaLeaseResponse, BatchRenewQuotaLeasesRequest,
    BatchRenewQuotaLeasesResponse, DeregisterRequest, DeregisterResponse, GetRoutingTableRequest,
    GetRoutingTableResponse, RegisterRequest, RegisterResponse, ReleaseQuotaLeaseRequest,
    ReleaseQuotaLeaseResponse, RenewQuotaLeaseRequest, RenewQuotaLeaseResponse,
    RenewShardLeaseRequest, RenewShardLeaseResponse, deregister_response,
    get_routing_table_response,
};
use golem_common::model::RetryConfig;
use golem_common::model::base64::Base64;
use golem_schema::proto::golem::common::Empty;
use golem_service_base::clients::shard_manager::{
    GrpcShardManager, GrpcShardManagerConfig, ShardManager,
};
use golem_service_base::clients::shard_manager_leader::{LeaderAddress, not_leader_status};
use golem_service_base::grpc::client::{
    EnabledGrpcClientTlsConfig, GrpcClientConfig, GrpcClientTlsConfig,
};
use std::collections::BTreeMap;
use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use test_r::test;
use tonic::body::Body;
use tonic::codec::CompressionEncoding;
use tonic::codegen::Service;
use tonic::server::NamedService;
use tonic::{Request, Response, Status};
use uuid::Uuid;

/// Serves the two calls these tests make, and counts them.
#[derive(Clone, Default)]
struct FakeLeader {
    calls: Arc<AtomicUsize>,
    /// Delays the next `get_routing_table` by this much, once.
    delay_next: Arc<Mutex<Option<Duration>>>,
}

#[tonic::async_trait]
impl ShardManagerService for FakeLeader {
    async fn get_routing_table(
        &self,
        _: Request<GetRoutingTableRequest>,
    ) -> Result<Response<GetRoutingTableResponse>, Status> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let delay = self.delay_next.lock().unwrap().take();
        if let Some(delay) = delay {
            tokio::time::sleep(delay).await;
        }
        Ok(Response::new(GetRoutingTableResponse {
            result: Some(get_routing_table_response::Result::Success(
                GrpcRoutingTable {
                    number_of_shards: 1,
                    shard_assignments: vec![],
                },
            )),
        }))
    }

    async fn deregister(
        &self,
        _: Request<DeregisterRequest>,
    ) -> Result<Response<DeregisterResponse>, Status> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(Response::new(DeregisterResponse {
            result: Some(deregister_response::Result::Success(Empty {})),
        }))
    }

    async fn register(
        &self,
        _: Request<RegisterRequest>,
    ) -> Result<Response<RegisterResponse>, Status> {
        Err(Status::unimplemented("register"))
    }

    async fn renew_shard_lease(
        &self,
        _: Request<RenewShardLeaseRequest>,
    ) -> Result<Response<RenewShardLeaseResponse>, Status> {
        Err(Status::unimplemented("renew_shard_lease"))
    }

    async fn acquire_quota_lease(
        &self,
        _: Request<AcquireQuotaLeaseRequest>,
    ) -> Result<Response<AcquireQuotaLeaseResponse>, Status> {
        Err(Status::unimplemented("acquire_quota_lease"))
    }

    async fn renew_quota_lease(
        &self,
        _: Request<RenewQuotaLeaseRequest>,
    ) -> Result<Response<RenewQuotaLeaseResponse>, Status> {
        Err(Status::unimplemented("renew_quota_lease"))
    }

    async fn batch_renew_quota_leases(
        &self,
        _: Request<BatchRenewQuotaLeasesRequest>,
    ) -> Result<Response<BatchRenewQuotaLeasesResponse>, Status> {
        Err(Status::unimplemented("batch_renew_quota_leases"))
    }

    async fn release_quota_lease(
        &self,
        _: Request<ReleaseQuotaLeaseRequest>,
    ) -> Result<Response<ReleaseQuotaLeaseResponse>, Status> {
        Err(Status::unimplemented("release_quota_lease"))
    }
}

#[derive(Clone)]
enum Mode {
    Leader,
    /// Names this leader, or none.
    Standby(Option<LeaderAddress>),
    /// Answers every call with this code and no detail, as a process that is not a working
    /// shard manager does.
    Fails(tonic::Code),
}

/// One replica: serves as the leader or answers as a standby. The mode can change at run time.
#[derive(Clone)]
struct FakeReplica {
    mode: Arc<Mutex<Mode>>,
    leader: ShardManagerServiceServer<FakeLeader>,
    calls: Arc<AtomicUsize>,
    refusals: Arc<AtomicUsize>,
    /// Modes for the next calls, one per call, before `mode` applies again: a test steps the
    /// replica through a sequence by call count instead of by time.
    script: Arc<Mutex<std::collections::VecDeque<Mode>>>,
    delay_next: Arc<Mutex<Option<Duration>>>,
}

impl NamedService for FakeReplica {
    const NAME: &'static str = <ShardManagerServiceServer<FakeLeader> as NamedService>::NAME;
}

impl Service<http::Request<Body>> for FakeReplica {
    type Response = http::Response<Body>;
    type Error = Infallible;
    type Future = std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<Self::Response, Infallible>> + Send>,
    >;

    fn poll_ready(
        &mut self,
        _: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Infallible>> {
        std::task::Poll::Ready(Ok(()))
    }

    fn call(&mut self, request: http::Request<Body>) -> Self::Future {
        let mode = self
            .script
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_else(|| self.mode.lock().unwrap().clone());
        let mut leader = self.leader.clone();
        let refusals = self.refusals.clone();
        Box::pin(async move {
            match mode {
                Mode::Leader => leader.call(request).await,
                Mode::Standby(named) => {
                    refusals.fetch_add(1, Ordering::SeqCst);
                    let status = match named {
                        Some(address) => not_leader_status(&address),
                        None => Status::unavailable("no shard manager leader is ready yet; retry"),
                    };
                    Ok(status.into_http())
                }
                Mode::Fails(code) => {
                    refusals.fetch_add(1, Ordering::SeqCst);
                    Ok(Status::new(code, "not a shard manager").into_http())
                }
            }
        })
    }
}

struct Running {
    addr: SocketAddr,
    replica: FakeReplica,
    stop: tokio::sync::oneshot::Sender<()>,
    server: tokio::task::JoinHandle<()>,
}

impl Running {
    fn address(&self) -> LeaderAddress {
        LeaderAddress {
            host: "127.0.0.1".to_string(),
            port: self.addr.port(),
        }
    }

    fn set(&self, mode: Mode) {
        *self.replica.mode.lock().unwrap() = mode;
    }

    /// The next calls take these modes, one per call.
    fn script(&self, modes: impl IntoIterator<Item = Mode>) {
        self.replica.script.lock().unwrap().extend(modes);
    }

    fn delay_next_call(&self, delay: Duration) {
        *self.replica.delay_next.lock().unwrap() = Some(delay);
    }

    fn calls(&self) -> usize {
        self.replica.calls.load(Ordering::SeqCst)
    }

    fn refusals(&self) -> usize {
        self.replica.refusals.load(Ordering::SeqCst)
    }

    async fn stop(self) {
        let _ = self.stop.send(());
        let _ = self.server.await;
    }
}

async fn run_replica(mode: Mode) -> Running {
    run_replica_with(mode, None).await
}

async fn run_replica_with(mode: Mode, tls: Option<tonic::transport::ServerTlsConfig>) -> Running {
    // The test binary has no process-level crypto provider; the services install theirs in main.
    let _ = rustls::crypto::ring::default_provider().install_default();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let delay = Arc::new(Mutex::new(None));
    let fake = FakeLeader {
        calls: Arc::new(AtomicUsize::new(0)),
        delay_next: delay.clone(),
    };
    let replica = FakeReplica {
        mode: Arc::new(Mutex::new(mode)),
        calls: fake.calls.clone(),
        leader: ShardManagerServiceServer::new(fake)
            .accept_compressed(CompressionEncoding::Gzip)
            .send_compressed(CompressionEncoding::Gzip),
        refusals: Arc::new(AtomicUsize::new(0)),
        script: Arc::new(Mutex::new(std::collections::VecDeque::new())),
        delay_next: delay,
    };
    let (stop, stopped) = tokio::sync::oneshot::channel();
    let service = replica.clone();
    let server = tokio::spawn(async move {
        let mut builder = tonic::transport::Server::builder();
        if let Some(tls) = tls {
            builder = builder.tls_config(tls).expect("a valid server TLS config");
        }
        let _ = builder
            .add_service(service)
            .serve_with_incoming_shutdown(
                tokio_stream::wrappers::TcpListenerStream::new(listener),
                async move {
                    let _ = stopped.await;
                },
            )
            .await;
    });
    Running {
        addr,
        replica,
        stop,
        server,
    }
}

const TLS_FIXTURES: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/shard-manager-tls"
);

fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(format!("{TLS_FIXTURES}/{name}"))
        .unwrap_or_else(|err| panic!("cannot read the TLS fixture {name}: {err}"))
}

/// A loopback port that refuses connections and that no other process can take while the socket
/// lives: bound, never listening. A port found by binding a listener and dropping it can be taken
/// by another process before the test uses it.
fn refusing_port() -> (socket2::Socket, u16) {
    let socket = socket2::Socket::new(socket2::Domain::IPV4, socket2::Type::STREAM, None)
        .expect("a TCP socket");
    socket
        .bind(&SocketAddr::from(([127, 0, 0, 1], 0)).into())
        .expect("a loopback port");
    let port = socket
        .local_addr()
        .expect("a bound address")
        .as_socket()
        .expect("an IP address")
        .port();
    (socket, port)
}

fn client_for(configured: &Running) -> GrpcShardManager {
    GrpcShardManager::new(&GrpcShardManagerConfig {
        host: "127.0.0.1".to_string(),
        port: configured.addr.port(),
        client_config: GrpcClientConfig {
            connect_timeout: Duration::from_secs(10),
            retries_on_unavailable: RetryConfig {
                max_attempts: 2,
                min_delay: Duration::from_millis(10),
                max_delay: Duration::from_millis(50),
                multiplier: 2.0,
                max_jitter_factor: None,
            },
            ..GrpcClientConfig::default()
        },
        retries: RetryConfig::default(),
    })
}

#[test]
async fn a_call_through_a_standby_reaches_the_leader_and_stays_there() {
    let leader = run_replica(Mode::Leader).await;
    let standby = run_replica(Mode::Standby(Some(leader.address()))).await;
    let client = client_for(&standby);

    client
        .get_routing_table()
        .await
        .expect("the standby names the leader");
    client
        .get_routing_table()
        .await
        .expect("the client now calls the leader directly");

    assert_eq!(leader.calls(), 2);
    assert_eq!(
        standby.refusals(),
        1,
        "only the first call needs the standby"
    );
    leader.stop().await;
    standby.stop().await;
}

#[test]
async fn a_one_try_call_through_a_standby_reaches_the_leader() {
    let leader = run_replica(Mode::Leader).await;
    let standby = run_replica(Mode::Standby(Some(leader.address()))).await;
    let client = client_for(&standby);

    client
        .deregister(Uuid::new_v4(), BTreeMap::new())
        .await
        .expect("following the standby's answer is part of the one try");

    assert_eq!(leader.calls(), 1);
    assert_eq!(standby.refusals(), 1);
    leader.stop().await;
    standby.stop().await;
}

#[test]
async fn after_the_leader_goes_away_the_client_asks_the_configured_address_again() {
    let first = run_replica(Mode::Leader).await;
    let standby = run_replica(Mode::Standby(Some(first.address()))).await;
    let client = client_for(&standby);
    client
        .get_routing_table()
        .await
        .expect("reaches the first leader");
    first.stop().await;

    let second = run_replica(Mode::Leader).await;
    standby.set(Mode::Standby(Some(second.address())));

    client
        .get_routing_table()
        .await
        .expect("reaches the second leader");

    assert_eq!(second.calls(), 1);
    assert_eq!(standby.refusals(), 2);
    second.stop().await;
    standby.stop().await;
}

#[test]
async fn a_known_leader_that_now_stands_by_sends_the_client_on() {
    let first = run_replica(Mode::Leader).await;
    let second = run_replica(Mode::Leader).await;
    let standby = run_replica(Mode::Standby(Some(first.address()))).await;
    let client = client_for(&standby);
    client
        .get_routing_table()
        .await
        .expect("reaches the first leader");

    first.set(Mode::Standby(Some(second.address())));
    client
        .get_routing_table()
        .await
        .expect("the old leader names the new one");

    assert_eq!(second.calls(), 1);
    assert_eq!(
        standby.refusals(),
        1,
        "the configured address is not needed for this"
    );
    first.stop().await;
    second.stop().await;
    standby.stop().await;
}

#[test]
async fn a_standby_naming_an_unreachable_leader_fails_within_the_short_connect_timeout() {
    // A documentation address (RFC 5737) that no network routes: a connection to it gets no
    // answer at all, like one to the IP of a pod whose node is gone.
    let unreachable = LeaderAddress {
        host: "192.0.2.1".to_string(),
        port: 9092,
    };
    let standby = run_replica(Mode::Standby(Some(unreachable))).await;
    let client = client_for(&standby);

    let started = Instant::now();
    client
        .get_routing_table()
        .await
        .expect_err("nothing serves at the named address");

    assert!(
        started.elapsed() < Duration::from_secs(6),
        "the client waited {:?}; the leader connection must give up after about 2s, not the \
         configured 10s",
        started.elapsed()
    );
    standby.stop().await;
}

#[test]
async fn a_standby_that_knows_no_leader_is_retried_as_before() {
    let standby = run_replica(Mode::Standby(None)).await;
    let client = client_for(&standby);

    client
        .get_routing_table()
        .await
        .expect_err("no replica can serve");

    assert!(
        standby.refusals() > 1,
        "a plain Unavailable keeps its retries"
    );
    standby.stop().await;
}

fn client_with_retries(
    configured: &Running,
    retries_on_unavailable: RetryConfig,
) -> GrpcShardManager {
    GrpcShardManager::new(&GrpcShardManagerConfig {
        host: "127.0.0.1".to_string(),
        port: configured.addr.port(),
        client_config: GrpcClientConfig {
            connect_timeout: Duration::from_secs(10),
            retries_on_unavailable,
            ..GrpcClientConfig::default()
        },
        retries: RetryConfig::default(),
    })
}

fn client_with_request_timeout(
    configured: &Running,
    request_timeout: Duration,
) -> GrpcShardManager {
    GrpcShardManager::new(&GrpcShardManagerConfig {
        host: "127.0.0.1".to_string(),
        port: configured.addr.port(),
        client_config: GrpcClientConfig {
            connect_timeout: Duration::from_secs(10),
            request_timeout: Some(request_timeout),
            retries_on_unavailable: RetryConfig {
                max_attempts: 2,
                min_delay: Duration::from_millis(10),
                max_delay: Duration::from_millis(50),
                multiplier: 2.0,
                max_jitter_factor: None,
            },
            ..GrpcClientConfig::default()
        },
        retries: RetryConfig::default(),
    })
}

#[test]
// A request that runs out of its own time says nothing about the leader: the next call must go
// straight to it, not back through a standby.
async fn a_request_timeout_on_the_leader_keeps_the_leader() {
    let leader = run_replica(Mode::Leader).await;
    let standby = run_replica(Mode::Standby(Some(leader.address()))).await;
    let client = client_with_request_timeout(&standby, Duration::from_millis(200));
    client
        .get_routing_table()
        .await
        .expect("the standby names the leader");

    leader.delay_next_call(Duration::from_secs(1));
    client
        .get_routing_table()
        .await
        .expect_err("the slow call runs out of its time");
    client
        .get_routing_table()
        .await
        .expect("the leader answers again");

    assert_eq!(
        standby.refusals(),
        1,
        "only the first call needs the standby; a timeout is no reason to leave the leader"
    );
    assert_eq!(leader.calls(), 3);
    leader.stop().await;
    standby.stop().await;
}

#[test]
// A leader that refused one call may serve the next: a later pass of the same call tries it again
// instead of waiting until the replicas name another leader.
async fn a_named_leader_that_failed_is_tried_again_after_a_backoff() {
    let leader = run_replica(Mode::Leader).await;
    leader.script([Mode::Fails(tonic::Code::Unavailable)]);
    let standby = run_replica(Mode::Standby(Some(leader.address()))).await;
    let client = client_with_retries(
        &standby,
        RetryConfig {
            max_attempts: 5,
            min_delay: Duration::from_millis(10),
            max_delay: Duration::from_millis(50),
            multiplier: 2.0,
            max_jitter_factor: None,
        },
    );

    client
        .get_routing_table()
        .await
        .expect("a later pass reaches the leader that refused once");
    assert_eq!(leader.calls(), 1);
    leader.stop().await;
    standby.stop().await;
}

#[test]
// A pod IP that a new pod reuses for another service answers `Unimplemented`: the client must
// stop calling it and ask the configured address again.
async fn a_known_leader_address_that_now_serves_something_else_is_forgotten() {
    let first = run_replica(Mode::Leader).await;
    let second = run_replica(Mode::Leader).await;
    let standby = run_replica(Mode::Standby(Some(first.address()))).await;
    let client = client_for(&standby);
    client
        .get_routing_table()
        .await
        .expect("reaches the first leader");

    first.set(Mode::Fails(tonic::Code::Unimplemented));
    standby.set(Mode::Standby(Some(second.address())));
    client
        .get_routing_table()
        .await
        .expect("the configured address names the second leader");

    assert_eq!(second.calls(), 1);
    first.stop().await;
    second.stop().await;
    standby.stop().await;
}

#[test]
// After an ungraceful kill, the replicas name the dead leader until its etcd lease expires. A call
// that may be repeated must ride that window out with the client's retry budget, as it rode out a
// closed port before, instead of failing at the first refused connection.
async fn a_retried_call_rides_out_a_dead_leader_that_is_still_named() {
    let (_reserved, dead_port) = refusing_port();
    let dead = LeaderAddress {
        host: "127.0.0.1".to_string(),
        port: dead_port,
    };
    let leader = run_replica(Mode::Leader).await;
    let standby = run_replica(Mode::Standby(Some(leader.address()))).await;
    // The first two answers still name the dead leader, as a standby does until the dead
    // leader's lease expires; then the standby names the new one.
    standby.script([Mode::Standby(Some(dead.clone())), Mode::Standby(Some(dead))]);
    let client = client_with_retries(
        &standby,
        RetryConfig {
            max_attempts: 5,
            min_delay: Duration::from_millis(10),
            max_delay: Duration::from_millis(50),
            multiplier: 2.0,
            max_jitter_factor: None,
        },
    );

    client
        .get_routing_table()
        .await
        .expect("the call rides out the time in which the dead leader is still named");
    assert_eq!(leader.calls(), 1);
    leader.stop().await;
    standby.stop().await;
}

#[test]
// Clients reach the leader by its advertised host, a pod IP that the shard managers' certificate
// does not name. With no server name configured, the client must check the leader's certificate
// against the host it was configured with, the name the certificate is issued for.
async fn a_redirect_over_tls_checks_the_leader_against_the_configured_host() {
    let server_tls = || {
        tonic::transport::ServerTlsConfig::new().identity(tonic::transport::Identity::from_pem(
            fixture("server.crt"),
            fixture("server.key"),
        ))
    };
    let leader = run_replica_with(Mode::Leader, Some(server_tls())).await;
    // The leader's address is an IP literal, as a pod IP is.
    let standby = run_replica_with(Mode::Standby(Some(leader.address())), Some(server_tls())).await;
    let client = GrpcShardManager::new(&GrpcShardManagerConfig {
        host: "localhost".to_string(),
        port: standby.addr.port(),
        client_config: GrpcClientConfig {
            connect_timeout: Duration::from_secs(10),
            retries_on_unavailable: RetryConfig {
                max_attempts: 2,
                min_delay: Duration::from_millis(10),
                max_delay: Duration::from_millis(50),
                multiplier: 2.0,
                max_jitter_factor: None,
            },
            tls: GrpcClientTlsConfig::Enabled(EnabledGrpcClientTlsConfig {
                client_cert: Base64(fixture("client.crt")),
                client_key: Base64(fixture("client.key")),
                server_ca_cert: Base64(fixture("ca.crt")),
                server_domain_name: None,
            }),
            ..GrpcClientConfig::default()
        },
        retries: RetryConfig::default(),
    });

    client
        .get_routing_table()
        .await
        .expect("the leader's certificate names the configured host");
    assert_eq!(leader.calls(), 1);
    leader.stop().await;
    standby.stop().await;
}
