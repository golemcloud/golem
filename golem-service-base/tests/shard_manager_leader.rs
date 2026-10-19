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
use golem_service_base::clients::shard_manager_leader::{
    LeaderAddress, not_leader_status, refusal_status,
};
use golem_service_base::grpc::client::{
    EnabledGrpcClientTlsConfig, GrpcClientConfig, GrpcClientTlsConfig,
};
use rcgen::{
    BasicConstraints, CertificateParams, CertifiedIssuer, DnType, ExtendedKeyUsagePurpose, IsCa,
    Issuer, KeyPair, KeyUsagePurpose,
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
    /// Refuses every call before running it, naming no leader, as a leader that is stopping does.
    Refuses,
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
                Mode::Refuses => {
                    refusals.fetch_add(1, Ordering::SeqCst);
                    Ok(refusal_status("this shard manager is stopping").into_http())
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
    run_replica_at(mode, tls, 0).await
}

/// A replica on this loopback port, to bring a replica back where another one stood. Port 0 takes
/// any free port.
async fn run_replica_at(
    mode: Mode,
    tls: Option<tonic::transport::ServerTlsConfig>,
    port: u16,
) -> Running {
    // The test binary has no process-level crypto provider; the services install theirs in main.
    let _ = rustls::crypto::ring::default_provider().install_default();
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port))
        .await
        .unwrap();
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

/// The certificates of the TLS test, made in memory.
struct TestPki {
    ca_pem: String,
    server_pem: String,
    server_key_pem: String,
    client_pem: String,
    client_key_pem: String,
}

impl TestPki {
    /// The server certificate names only `localhost`, as a shard manager certificate names its
    /// service host and not a pod IP. It has no IP address name.
    fn new() -> Self {
        let ca = test_ca("shard manager test CA");
        let (server_pem, server_key_pem) =
            signed_by(&ca, "localhost", ExtendedKeyUsagePurpose::ServerAuth);
        let (client_pem, client_key_pem) =
            signed_by(&ca, "client", ExtendedKeyUsagePurpose::ClientAuth);
        Self {
            ca_pem: ca.pem(),
            server_pem,
            server_key_pem,
            client_pem,
            client_key_pem,
        }
    }
}

fn test_ca(name: &str) -> CertifiedIssuer<'static, KeyPair> {
    let mut params = CertificateParams::new(Vec::<String>::new()).unwrap();
    params.distinguished_name.push(DnType::CommonName, name);
    params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
    CertifiedIssuer::self_signed(params, KeyPair::generate().unwrap()).unwrap()
}

/// A certificate for `name` and its private key, both PEM.
fn signed_by(
    ca: &CertifiedIssuer<'static, KeyPair>,
    name: &str,
    usage: ExtendedKeyUsagePurpose,
) -> (String, String) {
    let mut params = CertificateParams::new(vec![name.to_string()]).unwrap();
    params.distinguished_name.push(DnType::CommonName, name);
    params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
    params.extended_key_usages = vec![usage];
    let key = KeyPair::generate().unwrap();
    let issuer: &Issuer<'static, KeyPair> = ca;
    let cert = params.signed_by(&key, issuer).unwrap();
    (cert.pem(), key.serialize_pem())
}

/// A loopback port that refuses connections at once: it was bound and then released. Another
/// process could take it before the test uses it, which would make a test fail and could not make
/// one pass wrongly. A socket that is bound and never listens is no substitute: connects to it get
/// no answer at all.
fn closed_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .expect("a loopback port")
        .local_addr()
        .expect("a bound address")
        .port()
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
// A standby that names a leader which itself stands by and names another is an answer, not a
// failure: the call has not run anywhere yet, so the chain is followed even for a one-try call.
async fn a_one_try_call_follows_a_chain_of_standbys_to_the_leader() {
    let leader = run_replica(Mode::Leader).await;
    let middle = run_replica(Mode::Standby(Some(leader.address()))).await;
    let first = run_replica(Mode::Standby(Some(middle.address()))).await;
    let client = client_for(&first);

    client
        .deregister(Uuid::new_v4(), BTreeMap::new())
        .await
        .expect("each standby names the next replica, and the last one leads");

    assert_eq!(leader.calls(), 1, "the call ran exactly once");
    assert_eq!(first.refusals(), 1);
    assert_eq!(middle.refusals(), 1);
    leader.stop().await;
    middle.stop().await;
    first.stop().await;
}

#[test]
// Two replicas that name each other must not keep the call going: it follows a bounded number of
// answers and then returns the last one.
async fn a_one_try_call_stops_following_standbys_that_name_each_other() {
    let first = run_replica(Mode::Standby(None)).await;
    let second = run_replica(Mode::Standby(None)).await;
    first.set(Mode::Standby(Some(second.address())));
    second.set(Mode::Standby(Some(first.address())));
    let client = client_for(&first);

    client
        .deregister(Uuid::new_v4(), BTreeMap::new())
        .await
        .expect_err("no replica leads");

    assert_eq!(
        first.refusals() + second.refusals(),
        4,
        "the configured address, then three followed answers"
    );
    first.stop().await;
    second.stop().await;
}

#[test]
// A call that must not be repeated may have reached the leader before it failed, so a known
// leader's failure goes back to the caller instead of being sent again to anyone.
async fn a_one_try_call_to_a_known_leader_that_fails_is_not_sent_again() {
    let leader = run_replica(Mode::Leader).await;
    let standby = run_replica(Mode::Standby(Some(leader.address()))).await;
    let client = client_for(&standby);
    client
        .get_routing_table()
        .await
        .expect("the standby names the leader");
    leader.script([Mode::Fails(tonic::Code::Unavailable)]);

    client
        .deregister(Uuid::new_v4(), BTreeMap::new())
        .await
        .expect_err("the failure of a one-try call is returned");

    assert_eq!(leader.calls(), 1, "only the discovery call was served");
    assert_eq!(leader.refusals(), 1, "the one try reached the leader once");
    assert_eq!(
        standby.refusals(),
        1,
        "the standby saw only the discovery call"
    );
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

    // A call that is tried once ends at the first connect that fails; a call that may be repeated
    // keeps waiting for a failover to end.
    let started = Instant::now();
    client
        .deregister(Uuid::new_v4(), BTreeMap::new())
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
async fn a_standby_that_names_no_ready_leader_is_waited_out_for_the_failover_budget() {
    let standby = run_replica(Mode::Standby(None)).await;
    let client = client_for(&standby);

    let started = Instant::now();
    client
        .get_routing_table()
        .await
        .expect_err("no replica can serve");

    assert!(
        started.elapsed() >= Duration::from_secs(14) && started.elapsed() < Duration::from_secs(20),
        "a plain Unavailable from a working replica is waited out for about 15s, took {:?}",
        started.elapsed()
    );
    assert!(standby.refusals() > 1);
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
    let dead_port = closed_port();
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

/// A client with the default client settings, as the services use it: no test-sized retry budget.
fn client_with_default_config(host: &str, port: u16) -> GrpcShardManager {
    GrpcShardManager::new(&GrpcShardManagerConfig {
        host: host.to_string(),
        port,
        client_config: GrpcClientConfig::default(),
        retries: RetryConfig::default(),
    })
}

#[test]
// The replicas name a dead leader until its lease expires, which takes longer than the default
// retry budget. A call that may be repeated waits that out with the router's own budget.
async fn a_retried_call_waits_out_a_failover_longer_than_the_default_retries() {
    let leader = run_replica(Mode::Leader).await;
    let standby = run_replica(Mode::Standby(Some(leader.address()))).await;
    // The named leader cannot serve for its first six calls. A pass tries it only every other
    // time, so the call needs twelve passes: about five seconds at the failover backoff.
    leader.script(std::iter::repeat_n(
        Mode::Fails(tonic::Code::Unavailable),
        6,
    ));
    let client = client_with_default_config("127.0.0.1", standby.addr.port());

    let started = Instant::now();
    client
        .get_routing_table()
        .await
        .expect("the call outlasts the time in which the named leader cannot serve");

    assert!(
        started.elapsed() > Duration::from_secs(3),
        "the test must outlast the default retries, took {:?}",
        started.elapsed()
    );
    assert_eq!(leader.calls(), 1);
    leader.stop().await;
    standby.stop().await;
}

#[test]
// Right after a leader is gone, the replicas may know no leader at all and answer `UNAVAILABLE`.
// That is an answer from a working replica about a failover, so the call keeps asking for longer
// than the default retries allow.
async fn a_retried_call_waits_out_replicas_that_know_no_leader_yet() {
    let leader = run_replica(Mode::Leader).await;
    let standby = run_replica(Mode::Standby(None)).await;
    // The first pass runs the client's default schedule: five asks, with pauses of 100, 200, 400
    // and 800 ms. Each later pass asks once, after a pause of 100, 200, 400, 500, 500 and 500 ms.
    // The first ten answers last about three and a half seconds in all, and the one after names
    // the leader.
    standby.script(
        std::iter::repeat_n(Mode::Standby(None), 10)
            .chain(std::iter::once(Mode::Standby(Some(leader.address())))),
    );
    let client = client_with_default_config("127.0.0.1", standby.addr.port());

    let started = Instant::now();
    client
        .get_routing_table()
        .await
        .expect("the call outlasts the time in which no leader is ready");

    assert!(
        started.elapsed() > Duration::from_secs(2),
        "the test must outlast the default retries, took {:?}",
        started.elapsed()
    );
    assert_eq!(leader.calls(), 1);
    leader.stop().await;
    standby.stop().await;
}

#[test]
// Only a failover earns the longer budget. A configured address that nothing listens on is the
// shard manager being unreachable, which keeps the configured retries.
async fn a_configured_address_that_refuses_connections_fails_within_the_configured_retries() {
    let client = client_with_default_config("127.0.0.1", closed_port());

    let started = Instant::now();
    client
        .get_routing_table()
        .await
        .expect_err("nothing listens at the configured address");

    assert!(
        started.elapsed() < Duration::from_secs(8),
        "the call waited {:?}, longer than the configured retries",
        started.elapsed()
    );
}

fn client_with_schedule(port: u16, retries_on_unavailable: RetryConfig) -> GrpcShardManager {
    GrpcShardManager::new(&GrpcShardManagerConfig {
        host: "127.0.0.1".to_string(),
        port,
        client_config: GrpcClientConfig {
            retries_on_unavailable,
            ..GrpcClientConfig::default()
        },
        retries: RetryConfig::default(),
    })
}

#[test]
// A client tuned for a long failover (a lease TTL of 30 s, say) is configured with a schedule that
// is longer than the router's own budget. The call must wait for the configured schedule's total,
// and the passes of the failover must not use up its attempts on the way.
async fn a_schedule_longer_than_the_failover_budget_is_waited_out() {
    let leader = run_replica(Mode::Leader).await;
    let standby = run_replica(Mode::Standby(Some(leader.address()))).await;
    // The named leader fails for its first eighteen calls, one in every second pass: about
    // seventeen seconds of passes, which is more than the failover budget and more than the
    // twenty attempts of the schedule.
    leader.script(std::iter::repeat_n(
        Mode::Fails(tonic::Code::Unavailable),
        18,
    ));
    let client = client_with_schedule(
        standby.addr.port(),
        RetryConfig {
            max_attempts: 20,
            min_delay: Duration::from_secs(1),
            max_delay: Duration::from_secs(1),
            multiplier: 1.0,
            max_jitter_factor: None,
        },
    );

    let started = Instant::now();
    tokio::time::timeout(Duration::from_secs(40), client.get_routing_table())
        .await
        .expect("the call ends")
        .expect("the configured schedule is longer than the failover");

    assert!(
        started.elapsed() > Duration::from_secs(15),
        "the test must outlast the failover budget, took {:?}",
        started.elapsed()
    );
    assert_eq!(leader.calls(), 1);
    leader.stop().await;
    standby.stop().await;
}

#[test]
// Neither a long configured schedule nor a pass that runs it may stretch the wait: once a failover
// is seen, the call ends at the larger of the failover budget and the configured schedule's
// total, plus the pass that is under way.
async fn a_failover_is_waited_out_for_a_bounded_time_with_many_configured_attempts() {
    let standby = run_replica(Mode::Standby(None)).await;
    // A hundred attempts of 100 ms: a schedule of about ten seconds.
    let client = client_with_schedule(
        standby.addr.port(),
        RetryConfig {
            max_attempts: 100,
            min_delay: Duration::from_millis(100),
            max_delay: Duration::from_millis(100),
            multiplier: 1.0,
            max_jitter_factor: None,
        },
    );

    let started = Instant::now();
    let result = tokio::time::timeout(Duration::from_secs(40), client.get_routing_table())
        .await
        .expect("the call ends within a bounded time");

    result.expect_err("no replica can serve");
    assert!(
        started.elapsed() < Duration::from_secs(20),
        "the call took {:?}",
        started.elapsed()
    );
    standby.stop().await;
}

#[test]
// A named leader whose port refuses connections at once is the ordinary picture of a crash: the
// replicas keep naming it until its lease expires. The default client settings must carry the call
// through.
async fn a_retried_call_waits_out_a_named_leader_that_refuses_connections() {
    let dead = LeaderAddress {
        host: "127.0.0.1".to_string(),
        port: closed_port(),
    };
    let leader = run_replica(Mode::Leader).await;
    let standby = run_replica(Mode::Standby(Some(leader.address()))).await;
    standby.script(std::iter::repeat_n(Mode::Standby(Some(dead)), 12));
    let client = client_with_default_config("127.0.0.1", standby.addr.port());

    let started = Instant::now();
    client
        .get_routing_table()
        .await
        .expect("the call outlasts the time in which the dead leader is still named");

    assert!(
        started.elapsed() > Duration::from_secs(3),
        "the test must outlast the default retries, took {:?}",
        started.elapsed()
    );
    assert_eq!(leader.calls(), 1);
    leader.stop().await;
    standby.stop().await;
}

#[test]
// Once a failover has been seen, the wait goes on when the configured address stops answering
// too: a service address may keep routing to the replica that went away, and connects to it fail
// until the new leader is up.
async fn a_failover_that_has_been_seen_is_waited_out_when_the_configured_address_goes_quiet() {
    let dead = LeaderAddress {
        host: "127.0.0.1".to_string(),
        port: closed_port(),
    };
    let standby = run_replica(Mode::Standby(Some(dead))).await;
    let client = Arc::new(client_with_default_config("127.0.0.1", standby.addr.port()));

    let started = Instant::now();
    let call = tokio::spawn({
        let client = client.clone();
        async move { client.get_routing_table().await }
    });
    // The standby has named the dead leader once: a failover has been seen. From here on, connects
    // to the configured address are refused.
    tokio::time::timeout(Duration::from_secs(10), async {
        while standby.refusals() == 0 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the standby is asked");
    standby.stop().await;

    tokio::time::timeout(Duration::from_secs(40), call)
        .await
        .expect("the call ends")
        .expect("the task does not panic")
        .expect_err("nothing answers any more");

    assert!(
        started.elapsed() >= Duration::from_secs(14),
        "the call gave up after {:?}, before the failover budget",
        started.elapsed()
    );
}

#[test]
// A leader that failed in the last pass is skipped in the next one even when it is named by a
// replica that was itself named: the chain is not a way around the rule.
async fn a_leader_that_failed_is_skipped_in_the_next_pass_along_a_chain() {
    let leader = run_replica(Mode::Leader).await;
    leader.script([Mode::Fails(tonic::Code::Unavailable)]);
    let middle = run_replica(Mode::Standby(Some(leader.address()))).await;
    let first = run_replica(Mode::Standby(Some(middle.address()))).await;
    let client = client_with_default_config("127.0.0.1", first.addr.port());

    client
        .get_routing_table()
        .await
        .expect("a later pass reaches the leader that failed once");

    assert_eq!(
        middle.refusals(),
        3,
        "the second pass reached the middle replica but did not try the leader it names"
    );
    assert_eq!(leader.refusals(), 1);
    assert_eq!(leader.calls(), 1);
    leader.stop().await;
    middle.stop().await;
    first.stop().await;
}

#[test]
// Clients reach the leader by its advertised host, a pod IP that the shard managers' certificate
// does not name. With no server name configured, the client must check the leader's certificate
// against the host it was configured with, the name the certificate is issued for.
async fn a_redirect_over_tls_checks_the_leader_against_the_configured_host() {
    let pki = TestPki::new();
    let server_tls = || {
        tonic::transport::ServerTlsConfig::new().identity(tonic::transport::Identity::from_pem(
            pki.server_pem.clone(),
            pki.server_key_pem.clone(),
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
                client_cert: Base64(pki.client_pem.clone().into_bytes()),
                client_key: Base64(pki.client_key_pem.clone().into_bytes()),
                server_ca_cert: Base64(pki.ca_pem.clone().into_bytes()),
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

#[test]
// A leader that has served a call is cached. When it and the configured address then both stop
// answering, the failed cached leader is the signal that a failover is under way, so the call
// keeps asking until the configured address is back and names the new leader, even though the
// configured address itself named nobody on the way.
async fn a_cached_leader_that_failed_starts_the_wait_when_the_configured_address_is_gone_too() {
    let first = run_replica(Mode::Leader).await;
    let standby = run_replica(Mode::Standby(Some(first.address()))).await;
    // The configured address is a forwarder that can refuse every connection and keeps its port,
    // so that no other socket can take it while the address is "gone".
    let gate = Freezable::in_front_of(standby.addr).await;
    let client = Arc::new(client_with_default_config("127.0.0.1", gate.addr.port()));
    client
        .get_routing_table()
        .await
        .expect("the standby names the first leader, which is now cached");
    first.stop().await;
    gate.refuse();

    let call = tokio::spawn({
        let client = client.clone();
        async move { client.get_routing_table().await }
    });
    // The default retries of the configured address are spent after about two seconds. The
    // address then comes back and names a leader that serves.
    tokio::time::sleep(Duration::from_secs(4)).await;
    let second = run_replica(Mode::Leader).await;
    standby.set(Mode::Standby(Some(second.address())));
    gate.allow();

    tokio::time::timeout(Duration::from_secs(30), call)
        .await
        .expect("the call ends")
        .expect("the task does not panic")
        .expect("the configured address names a live leader inside the failover window");

    assert_eq!(second.calls(), 1);
    second.stop().await;
    standby.stop().await;
}

#[test]
// An advertised host that is no URI authority names nobody a client could call. The answer counts
// as one that names no leader, and the call asks again; it must not take the process down, which
// building a URI from the host would.
async fn a_named_leader_whose_host_is_no_uri_authority_is_treated_as_no_leader() {
    let leader = run_replica(Mode::Leader).await;
    let standby = run_replica(Mode::Standby(Some(leader.address()))).await;
    let malformed = ["shard manager", "x # c", "fe80::1%eth0", "a:b:c"].map(|host| {
        Mode::Standby(Some(LeaderAddress {
            host: host.to_string(),
            port: 9002,
        }))
    });
    standby.script(malformed);
    let client = client_for(&standby);

    client
        .get_routing_table()
        .await
        .expect("the malformed answers are waited out and the next one names the leader");

    assert_eq!(leader.calls(), 1);
    assert_eq!(standby.refusals(), 5);
    leader.stop().await;
    standby.stop().await;
}

/// A TCP forwarder in front of a replica. The connections open at the time of `freeze` go silent:
/// they read what the peers send and forward nothing, in either direction, and close nothing,
/// which is how a node that lost power looks from the other end of a connection that was open
/// before; connections made afterwards pass. While it `refuse`s, every connection is closed as
/// soon as it is accepted or used, and the port stays bound, so that no other socket can take it.
struct Freezable {
    addr: SocketAddr,
    connections: Arc<Mutex<Vec<Arc<std::sync::atomic::AtomicBool>>>>,
    refusing: Arc<std::sync::atomic::AtomicBool>,
}

impl Freezable {
    async fn in_front_of(upstream: SocketAddr) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let connections: Arc<Mutex<Vec<Arc<std::sync::atomic::AtomicBool>>>> =
            Arc::new(Mutex::new(Vec::new()));
        let refusing = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (registry, refuse) = (connections.clone(), refusing.clone());
        tokio::spawn(async move {
            while let Ok((client, _)) = listener.accept().await {
                if refuse.load(Ordering::SeqCst) {
                    continue;
                }
                let Ok(server) = tokio::net::TcpStream::connect(upstream).await else {
                    continue;
                };
                let frozen = Arc::new(std::sync::atomic::AtomicBool::new(false));
                registry.lock().unwrap().push(frozen.clone());
                let (client_read, client_write) = client.into_split();
                let (server_read, server_write) = server.into_split();
                tokio::spawn(forward(
                    client_read,
                    server_write,
                    frozen.clone(),
                    refuse.clone(),
                ));
                tokio::spawn(forward(server_read, client_write, frozen, refuse.clone()));
            }
        });
        Self {
            addr,
            connections,
            refusing,
        }
    }

    fn address(&self) -> LeaderAddress {
        LeaderAddress {
            host: "127.0.0.1".to_string(),
            port: self.addr.port(),
        }
    }

    fn freeze(&self) {
        for connection in self.connections.lock().unwrap().iter() {
            connection.store(true, Ordering::SeqCst);
        }
    }

    fn refuse(&self) {
        self.refusing.store(true, Ordering::SeqCst);
    }

    fn allow(&self) {
        self.refusing.store(false, Ordering::SeqCst);
    }
}

async fn forward(
    mut from: tokio::net::tcp::OwnedReadHalf,
    mut to: tokio::net::tcp::OwnedWriteHalf,
    frozen: Arc<std::sync::atomic::AtomicBool>,
    refusing: Arc<std::sync::atomic::AtomicBool>,
) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut buffer = [0u8; 16 * 1024];
    while let Ok(read) = from.read(&mut buffer).await {
        if read == 0 || refusing.load(Ordering::SeqCst) {
            break;
        }
        if frozen.load(Ordering::SeqCst) {
            std::future::pending::<()>().await;
        }
        if to.write_all(&buffer[..read]).await.is_err() {
            break;
        }
    }
}

#[test]
// The connection to a leader is idle between renewals. When the leader's node loses power, nothing
// closes it, so the call would hang until the keep-alive gives up; the router must learn within a
// seven seconds (a ping every two, five to answer), not the twenty of the default settings, and
// then reach the new leader.
async fn a_kept_leader_connection_that_goes_silent_fails_the_call_within_the_keep_alive_bound() {
    let old = run_replica(Mode::Leader).await;
    let proxy = Freezable::in_front_of(old.addr).await;
    let new = run_replica(Mode::Leader).await;
    let standby = run_replica(Mode::Standby(Some(proxy.address()))).await;
    let client = client_with_default_config("127.0.0.1", standby.addr.port());
    client
        .get_routing_table()
        .await
        .expect("the standby names the leader behind the forwarder");
    assert_eq!(old.calls(), 1);

    proxy.freeze();
    standby.set(Mode::Standby(Some(new.address())));
    let started = Instant::now();
    client
        .deregister(Uuid::new_v4(), BTreeMap::new())
        .await
        .expect_err("the leader no longer answers");
    let failed_after = started.elapsed();
    assert!(
        failed_after < Duration::from_secs(10),
        "the call hung for {failed_after:?}"
    );

    client
        .get_routing_table()
        .await
        .expect("the silent leader is forgotten and the new one is found");
    assert_eq!(new.calls(), 1);
    // The forwarder keeps its connection to the old replica open, which a graceful stop would wait
    // for.
    old.server.abort();
    new.stop().await;
    standby.stop().await;
}

#[test]
// The configured address is kept as one connection. When the replica behind it goes silent, a call
// that the known leader refused must not hang on that connection for the twenty seconds of the
// default settings: it fails over to a new connection within the ping bound and reaches the leader
// that the replicas now name.
async fn a_silent_configured_connection_fails_a_retried_call_within_the_keep_alive_bound() {
    let old = run_replica(Mode::Leader).await;
    let new = run_replica(Mode::Leader).await;
    let standby = run_replica(Mode::Standby(Some(old.address()))).await;
    let gate = Freezable::in_front_of(standby.addr).await;
    let client = client_with_default_config("127.0.0.1", gate.addr.port());
    client
        .get_routing_table()
        .await
        .expect("the standby names the old leader, which is now known");
    old.set(Mode::Refuses);
    standby.set(Mode::Standby(Some(new.address())));
    gate.freeze();

    let started = Instant::now();
    client
        .get_routing_table()
        .await
        .expect("a new connection to the configured address names the new leader");
    let took = started.elapsed();
    assert!(
        took < Duration::from_secs(12),
        "the call hung on the silent connection for {took:?}"
    );
    assert_eq!(new.calls(), 1);
    old.stop().await;
    new.stop().await;
    // The forwarder keeps its frozen connection to the standby open, which a graceful stop would
    // wait for.
    standby.server.abort();
}

#[test]
// A leader that is stopping refuses the call before running it, and says so. Even a call that must
// not be repeated has not run, so it goes on to the configured address, whose replicas name the
// new leader, and runs there exactly once.
async fn a_one_try_call_refused_by_a_stopping_known_leader_runs_once_on_the_new_leader() {
    let old = run_replica(Mode::Leader).await;
    let new = run_replica(Mode::Leader).await;
    let standby = run_replica(Mode::Standby(Some(old.address()))).await;
    let client = client_for(&standby);
    client
        .get_routing_table()
        .await
        .expect("the standby names the old leader, which is now known");
    old.set(Mode::Refuses);
    standby.set(Mode::Standby(Some(new.address())));

    client
        .deregister(Uuid::new_v4(), BTreeMap::new())
        .await
        .expect("the refusal did not run the call, so the new leader runs it");

    assert_eq!(old.refusals(), 1, "the stopping leader was asked once");
    assert_eq!(old.calls(), 1, "and ran only the discovery call");
    assert_eq!(new.calls(), 1, "the call ran exactly once");
    old.stop().await;
    new.stop().await;
    standby.stop().await;
}

#[test]
// A refusal from the configured address itself is an answer from a replica that works, as before:
// a call that may be repeated keeps asking, and a one-try call returns it.
async fn a_refusal_from_the_configured_address_is_returned_to_a_one_try_call() {
    let replica = run_replica(Mode::Refuses).await;
    let client = client_for(&replica);

    client
        .deregister(Uuid::new_v4(), BTreeMap::new())
        .await
        .expect_err("the only replica refuses");

    assert_eq!(replica.refusals(), 1, "the one try asked once");
    replica.stop().await;
}
