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

use golem_api_grpc::proto::golem::workerexecutor::v1::AssignShardsRequest;
use golem_api_grpc::proto::golem::workerexecutor::v1::worker_executor_client::WorkerExecutorClient;
use golem_common::model::{Empty, RetryConfig};
use golem_service_base::grpc::client::{
    GrpcClient, GrpcClientConfig, GrpcClientTlsConfig, MultiTargetGrpcClient,
};
use http::Uri;
use std::time::{Duration, Instant};
use test_r::test;
use tonic::transport::Channel;
use tonic_tracing_opentelemetry::middleware::client::OtelGrpcService;

/// Small enough to keep the test quick, large enough that failing inside it
/// cannot be confused with failing instantly for an unrelated reason.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);

/// The call must fail within a small multiple of CONNECT_TIMEOUT. The defect this
/// guards against produced ~120s (the OS TCP timeout), so the gap is unambiguous.
const MUST_FAIL_WITHIN: Duration = Duration::from_secs(15);

/// Keep-alive settings, small so the test is quick. Production uses 10s/10s.
const KEEP_ALIVE_INTERVAL: Duration = Duration::from_secs(1);
const KEEP_ALIVE_TIMEOUT: Duration = Duration::from_secs(2);

/// Mirrors the config worker-service actually loads for its executor client:
/// a bounded connect timeout, and no inner retries so the routing table can be
/// invalidated as soon as possible.
fn config() -> GrpcClientConfig {
    GrpcClientConfig {
        connect_timeout: CONNECT_TIMEOUT,
        request_timeout: None,
        http2_keep_alive_interval: Some(KEEP_ALIVE_INTERVAL),
        http2_keep_alive_timeout: Some(KEEP_ALIVE_TIMEOUT),
        http2_keep_alive_while_idle: Some(true),
        retries_on_unavailable: RetryConfig {
            max_attempts: 0,
            min_delay: Duration::from_millis(100),
            max_delay: Duration::from_secs(2),
            multiplier: 2.0,
            max_jitter_factor: None,
        },
        tls: GrpcClientTlsConfig::Disabled(Empty {}),
        max_message_size: 32 * 1024 * 1024,
    }
}

/// The client type the incident involved: worker-service's client for the
/// worker-executor API.
type ExecutorClient = WorkerExecutorClient<OtelGrpcService<Channel>>;

fn new_executor_client(
    channel: OtelGrpcService<Channel>,
    max_message_size: usize,
) -> ExecutorClient {
    WorkerExecutorClient::new(channel)
        .max_decoding_message_size(max_message_size)
        .max_encoding_message_size(max_message_size)
}

fn executor_client(config: GrpcClientConfig) -> MultiTargetGrpcClient<ExecutorClient> {
    MultiTargetGrpcClient::new("worker_executor", new_executor_client, config)
}

fn single_target_executor_client(
    endpoint: Uri,
    config: GrpcClientConfig,
) -> GrpcClient<ExecutorClient> {
    GrpcClient::new("worker_executor", new_executor_client, endpoint, config)
}

/// One trivial round trip, used throughout as a probe.
///
/// The test servers here route nothing, so a peer that is reachable at all
/// answers `Unimplemented`. Anything else means the call never got through.
async fn ping(
    client: &MultiTargetGrpcClient<ExecutorClient>,
    target: Uri,
) -> Result<(), tonic::Status> {
    client
        .call("assign_shards", target, move |executor| {
            Box::pin(executor.assign_shards(AssignShardsRequest { shard_ids: vec![] }))
        })
        .await
        .map(|_| ())
}

async fn ping_single(client: &GrpcClient<ExecutorClient>) -> Result<(), tonic::Status> {
    client
        .call("assign_shards", move |executor| {
            Box::pin(executor.assign_shards(AssignShardsRequest { shard_ids: vec![] }))
        })
        .await
        .map(|_| ())
}

/// A worker-executor pod deleted mid-flight leaves its IP blackholed. Calls
/// already routed to it must give up after `connect_timeout` so the caller can
/// re-resolve and retry against the executor that took over its shards.
///
/// Observed on golem-dev 2026-08-19 (chaos S5): one attempt against a deleted pod
/// took 119,781ms against a configured 10s connect_timeout, stalling 496
/// operations for two minutes each.
#[test]
async fn multi_target_call_to_blackholed_pod_gives_up_within_connect_timeout() {
    if delegated_to_namespace("multi_target_call_to_blackholed_pod_gives_up_within_connect_timeout")
    {
        return;
    }
    let (uri, _blackhole) = blackholed_target();
    let client = executor_client(config());

    let started = Instant::now();
    let result = ping(&client, uri).await;
    let elapsed = started.elapsed();

    assert!(
        result.is_err(),
        "expected the call to a blackholed address to fail, got success"
    );
    assert!(
        elapsed < MUST_FAIL_WITHIN,
        "call to blackholed address took {elapsed:?}, but connect_timeout is \
         {CONNECT_TIMEOUT:?} — the connect timeout is not being applied"
    );
}

/// Same guarantee for the single-target client, which the shard-manager and
/// registry clients use.
#[test]
async fn single_target_call_to_blackholed_pod_gives_up_within_connect_timeout() {
    if delegated_to_namespace(
        "single_target_call_to_blackholed_pod_gives_up_within_connect_timeout",
    ) {
        return;
    }
    let (uri, _blackhole) = blackholed_target();
    let client = single_target_executor_client(uri, config());

    let started = Instant::now();
    let result = ping_single(&client).await;
    let elapsed = started.elapsed();

    assert!(
        result.is_err(),
        "expected the call to a blackholed address to fail, got success"
    );
    assert!(
        elapsed < MUST_FAIL_WITHIN,
        "call to blackholed address took {elapsed:?}, but connect_timeout is \
         {CONNECT_TIMEOUT:?} — the connect timeout is not being applied"
    );
}

/// A pod that is being torn down can still complete a TCP handshake while its
/// process is already gone or wedged: the socket accepts, then nothing is ever
/// written. `connect_timeout` bounds the TCP connect, but the HTTP/2 handshake
/// that follows is a separate phase — if nothing bounds it, and `request_timeout`
/// is None (which is the deployed worker-service config), the call can hang
/// indefinitely.
///
/// This is the second candidate mechanism for the S5 incident: one attempt that
/// consumed 119,781ms and surfaced as a connect error.
/// Tried and rejected as fixes (each verified by running this test):
///   - eager `Endpoint::connect()` — performs the TCP connect only, handshake is deferred
///   - driving the channel to readiness — tonic's Channel is a Buffer, ready() returns at once
///   - `request_timeout` — bounds it, but caps every RPC including long agent invocations
///
/// What works is HTTP/2 keep-alive: `http2_keep_alive_interval` + `keep_alive_timeout`
/// bound detection at roughly their sum, without capping request duration.
#[test]
async fn call_to_peer_that_accepts_but_never_speaks_http2_gives_up() {
    // Accept connections and then stay completely silent: no HTTP/2 preface,
    // no SETTINGS frame, no close.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let _accepter = tokio::spawn(async move {
        let mut held = Vec::new();
        // Hold every socket open forever without writing anything.
        while let Ok((sock, _)) = listener.accept().await {
            held.push(sock);
        }
    });

    let uri: Uri = format!("http://{addr}").parse().unwrap();
    let client = executor_client(config());

    let started = Instant::now();
    let result = tokio::time::timeout(
        MUST_FAIL_WITHIN,
        client.call("assign_shards", uri, move |client| {
            Box::pin(client.assign_shards(AssignShardsRequest { shard_ids: vec![] }))
        }),
    )
    .await;
    let elapsed = started.elapsed();

    match result {
        Err(_) => panic!(
            "call to a silent peer was still hanging after {elapsed:?}; \
             connect_timeout is {CONNECT_TIMEOUT:?} and request_timeout is None, \
             so nothing bounds the HTTP/2 handshake"
        ),
        Ok(inner) => assert!(
            inner.is_err(),
            "expected the call to a silent peer to fail, got success"
        ),
    }
}

// ---------------------------------------------------------------------------
// Reproduction harness for the executor stall.
//
// These tests drop packets, because that is the only way to imitate what a
// deleted pod does to the connections already pointed at it: the IP stops
// answering without ever sending an RST. A refused connection fails fast and
// reproduces nothing.
//
// Dropping packets needs CAP_NET_ADMIN, so each of these tests re-runs itself
// inside a network namespace it creates for the purpose (see
// `delegated_to_namespace`). Plain `cargo test` runs them, `sudo` is not
// involved, and no rule is ever installed anywhere that outlives the test. On a
// machine that cannot do it at all, they say so rather than passing quietly.
// ---------------------------------------------------------------------------

fn iptables(args: &[&str]) {
    let out = std::process::Command::new("iptables")
        .args(args)
        .output()
        .expect("iptables must be available; run this test under `unshare -rn`");
    assert!(
        out.status.success(),
        "iptables {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn no_keepalive(connect_timeout: Duration) -> GrpcClientConfig {
    let mut cfg = config();
    cfg.connect_timeout = connect_timeout;
    cfg.http2_keep_alive_interval = None;
    cfg.http2_keep_alive_timeout = None;
    cfg.http2_keep_alive_while_idle = None;
    cfg
}

/// A peer that routes nothing, and counts the TCP connections made to it.
struct TestPeer {
    addr: std::net::SocketAddr,
    connections: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    _server: tokio::task::JoinHandle<()>,
}

async fn serve_grpc() -> TestPeer {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let connections = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));

    let counter = connections.clone();
    let incoming = futures::StreamExt::inspect(
        tokio_stream::wrappers::TcpListenerStream::new(listener),
        move |accepted| {
            if accepted.is_ok() {
                counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            }
        },
    );

    let server = tokio::spawn(async move {
        // Empty router: any request gets a real HTTP/2 response carrying
        // grpc-status 12 (Unimplemented). That is not Unavailable, so the client
        // keeps the channel cached — exactly as in production.
        let _ = tonic::transport::Server::builder()
            .add_routes(tonic::service::Routes::default())
            .serve_with_incoming(incoming)
            .await;
    });

    TestPeer {
        addr,
        connections,
        _server: server,
    }
}

/// The marker the parent sets on the child it spawns in [`delegated_to_namespace`].
const IN_OWN_NAMESPACE: &str = "GOLEM_TEST_NET_NAMESPACE";

/// Runs a test that needs to drop packets inside a network namespace of its own.
///
/// Dropping packets needs CAP_NET_ADMIN. Rather than make that someone else's
/// problem, or quietly skip for want of it, the test arranges it: `unshare
/// --user --map-root-user` makes the current user root inside a new user
/// namespace, and `--net` gives that namespace an empty network stack. The
/// capability is real but reaches nothing except that namespace's own loopback,
/// and all of it disappears when the child exits. No privilege on the host is
/// involved, and `sudo` is never needed.
///
/// Returns true when the work has been handed to a child process and the caller
/// should return, false when we are that child and the body should run.
#[must_use]
fn delegated_to_namespace(test: &str) -> bool {
    if std::env::var_os(IN_OWN_NAMESPACE).is_some() {
        assert!(
            is_isolated_namespace(),
            "{test} was spawned into a namespace that still has a default route, \
             so its firewall rules could reach a real network"
        );
        return false;
    }

    if !cfg!(target_os = "linux") {
        eprintln!(
            "[{test}] not run: dropping packets needs Linux network namespaces. \
             CI runs Linux, so this is covered there."
        );
        return true;
    }

    let binary = std::env::current_exe().expect("the running test binary");
    let child = std::process::Command::new("unshare")
        .args(["--user", "--map-root-user", "--net", "--"])
        .arg("sh")
        .arg("-c")
        .arg(format!(
            // tcp_retries2 is per namespace. Left at its default, the kernel
            // spends about fifteen minutes giving up on an established
            // connection whose peer has gone silent, and that wait is precisely
            // what some of these tests are timing.
            "ip link set lo up \
             && echo 4 > /proc/sys/net/ipv4/tcp_retries2 \
             && exec {} {test} --nocapture",
            shell_quoted(&binary)
        ))
        .env(IN_OWN_NAMESPACE, "1")
        .output()
        .unwrap_or_else(|err| {
            panic!("could not run `unshare` to isolate {test}: {err}. It ships with util-linux.")
        });

    // Held back unless it is needed, so concurrent children do not interleave
    // their output into nonsense.
    assert!(
        child.status.success(),
        "{test} failed inside its own network namespace. If `unshare` itself \
         failed, this kernel may have unprivileged user namespaces switched off: \
         check `sysctl kernel.unprivileged_userns_clone`.\n\n{}\n{}",
        String::from_utf8_lossy(&child.stdout),
        String::from_utf8_lossy(&child.stderr),
    );
    true
}

fn shell_quoted(path: &std::path::Path) -> String {
    format!("'{}'", path.display().to_string().replace('\'', r"'\''"))
}

/// A network namespace with no default route, which is what the child above gets.
///
/// Read from `/proc/net/route`, which resolves per namespace, unlike
/// `/sys/class/net`, which `unshare` leaves pointing at the host's interfaces.
fn is_isolated_namespace() -> bool {
    let Ok(routes) = std::fs::read_to_string("/proc/net/route") else {
        return false;
    };
    // Columns are Iface, Destination, Gateway, ...; destination 00000000 is the
    // default route.
    !routes
        .lines()
        .skip(1)
        .any(|route| route.split_whitespace().nth(1) == Some("00000000"))
}

/// An iptables rule that lasts only as long as it is held. Removing it on drop
/// keeps a panicking test from leaving the namespace's firewall in a state that
/// misleads every test after it.
#[must_use = "the rule is removed as soon as this is dropped"]
struct FirewallRule {
    rule: Vec<String>,
}

impl FirewallRule {
    fn add(rule: &[&str]) -> Self {
        let mut args = vec!["-A"];
        args.extend_from_slice(rule);
        iptables(&args);
        Self {
            rule: rule.iter().map(|part| part.to_string()).collect(),
        }
    }
}

impl Drop for FirewallRule {
    fn drop(&mut self) {
        let mut args = vec!["-D".to_string()];
        args.extend(self.rule.iter().cloned());
        let _ = std::process::Command::new("iptables").args(&args).output();
    }
}

/// Silences a port in both directions, so the peer behaves like a deleted pod:
/// no RST, no ICMP, just silence.
fn blackhole_port(port: &str) -> Vec<FirewallRule> {
    ["--dport", "--sport"]
        .into_iter()
        .map(|direction| FirewallRule::add(&["INPUT", "-p", "tcp", direction, port, "-j", "DROP"]))
        .collect()
}

/// An address that swallows packets the way a deleted pod's IP does: the connect
/// hangs instead of being refused.
///
/// A merely closed port will not do. It answers with RST immediately, so a
/// connect against it fails fast whether or not any timeout is applied, and the
/// bug this file exists for stays invisible.
fn blackholed_target() -> (Uri, Vec<FirewallRule>) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let rules = blackhole_port(&port.to_string());
    (format!("http://127.0.0.1:{port}").parse().unwrap(), rules)
}

/// A sysctl restored to its previous value on drop.
#[must_use = "the previous value is restored as soon as this is dropped"]
struct Sysctl {
    path: &'static str,
    previous: String,
}

impl Sysctl {
    fn set(path: &'static str, value: &str) -> Self {
        let previous = std::fs::read_to_string(path).unwrap_or_default();
        std::fs::write(path, value).expect("sysctl must be writable in the namespace");
        Self { path, previous }
    }
}

impl Drop for Sysctl {
    fn drop(&mut self) {
        let _ = std::fs::write(self.path, self.previous.trim());
    }
}

#[test]
async fn an_established_connection_to_a_blackholed_peer_still_stalls() {
    if delegated_to_namespace("established_connection_to_blackholed_peer") {
        return;
    }

    let peer = serve_grpc().await;
    let addr = peer.addr;
    let port = addr.port().to_string();
    let uri: Uri = format!("http://{addr}").parse().unwrap();

    let connect_timeout = Duration::from_secs(3);
    let client = executor_client(no_keepalive(connect_timeout));

    // 1. Establish and cache the connection, as production had done.
    let first = ping(&client, uri.clone()).await;
    let first_err = first.expect_err("empty router replies Unimplemented");
    eprintln!("[REPRO] first call settled: code={:?}", first_err.code());
    assert_ne!(
        first_err.code(),
        tonic::Code::Unavailable,
        "first call must not be Unavailable, or the channel is dropped and the \
         repro degenerates into the fresh-connect case"
    );

    // 2. The pod vanishes: its packets are silently dropped in both directions.
    let _blackhole = blackhole_port(&port);
    eprintln!("[REPRO] peer blackholed on port {port}");

    // 3. Next request on the cached, now-dead connection.
    let started = Instant::now();
    let result = ping(&client, uri).await;
    let elapsed = started.elapsed();

    let err = result.expect_err("call to a vanished peer must fail");
    eprintln!(
        "[REPRO] elapsed={:?} connect_timeout={:?} code={:?}\n[REPRO] error={:?}",
        elapsed,
        connect_timeout,
        err.code(),
        err
    );

    // Deliberately asserts the stall is still there. `connect_timeout` bounds
    // the TCP connect and nothing else, so a connection already established when
    // the peer went silent waits on the kernel instead. Keep-alive is what bounds
    // this in production, and it is switched off here to show the difference.
    // If this ever goes red, the stall was fixed and this test should be replaced
    // rather than repaired.
    assert!(
        elapsed > connect_timeout,
        "expected the stall to exceed connect_timeout ({connect_timeout:?}), got {elapsed:?}"
    );
    // The kernel is what ends this wait, so give it an outer bound rather than
    // letting a misconfigured tcp_retries2 hang the suite for a quarter of an hour.
    assert!(
        elapsed < Duration::from_secs(60),
        "the stall ran to {elapsed:?}; tcp_retries2 is not being applied in the namespace"
    );
}

/// Production's actual sequence, which the previous test does not capture: the
/// pod's process dies first (sockets close, client sees ConnectionReset /
/// BrokenPipe), and only then does its IP stop routing. The next call therefore
/// attempts a *fresh* TCP connect into a blackhole — which is what S5's error
/// says happened:
///   ConnectError("tcp connect error", 172.17.217.145:9093,
///                Custom { kind: TimedOut, error: Elapsed(()) })
#[test]
async fn peer_dies_then_blackholes_fresh_connect_is_bounded() {
    if delegated_to_namespace("peer_dies_then_blackholes_fresh_connect_is_bounded") {
        return;
    }

    let peer = serve_grpc().await;
    let addr = peer.addr;
    let port = addr.port().to_string();
    let uri: Uri = format!("http://{addr}").parse().unwrap();

    let connect_timeout = Duration::from_secs(3);
    let client = executor_client(no_keepalive(connect_timeout));

    let first = ping(&client, uri.clone()).await;
    eprintln!(
        "[REPRO2] first call settled: code={:?}",
        first.err().map(|e| e.code())
    );

    // Pod IP stops routing.
    let _blackhole = blackhole_port(&port);
    eprintln!("[REPRO2] peer blackholed on port {port}");

    // Second call dies on the stale socket once the kernel gives up.
    let t2 = Instant::now();
    let r2 = ping(&client, uri.clone()).await;
    eprintln!(
        "[REPRO2] call2 (stale socket) elapsed={:?} code={:?}",
        t2.elapsed(),
        r2.err().map(|e| e.code())
    );

    // Third call: the connection is now gone, so the cached channel must
    // establish a NEW one into the blackhole. This is the production path —
    // the one that produced ConnectError/Elapsed. Is it bounded by
    // connect_timeout?
    let started = Instant::now();
    let result = ping(&client, uri).await;
    let elapsed = started.elapsed();
    let err = result.expect_err("call to a dead peer must fail");

    eprintln!(
        "[REPRO2] call3 (reconnect) elapsed={:?} connect_timeout={:?} code={:?}\n[REPRO2] error={:?}",
        elapsed,
        connect_timeout,
        err.code(),
        err
    );

    // S5 saw ~12x the configured connect_timeout on a single attempt.
    assert!(
        elapsed < connect_timeout * 3,
        "one attempt took {elapsed:?} with connect_timeout={connect_timeout:?} — \
         connection establishment is not bounded by connect_timeout on this path"
    );
}

/// When an executor is unreachable, every request queued against it must fail
/// after ONE connect_timeout, so the caller can invalidate the routing table and
/// re-resolve to the executor that took over the shards.
///
/// tonic's `Channel` does not give us this: it is a `tower::Buffer` that services
/// requests one at a time, so each queued request waits for every request ahead
/// of it to burn a full connect_timeout before its own connect is attempted —
/// making the worst case queue_depth x connect_timeout.
///
/// That is what produced chaos run S5's 119,781ms stalls on 2026-08-19: a 10s
/// connect_timeout with roughly twelve requests queued against a deleted
/// executor. Reproduced here at 8 x connect_timeout before the fix.
#[test]
async fn concurrent_calls_to_unreachable_peer_fail_within_one_connect_timeout() {
    if delegated_to_namespace(
        "concurrent_calls_to_unreachable_peer_fail_within_one_connect_timeout",
    ) {
        return;
    }

    const CONCURRENCY: usize = 8;
    let connect_timeout = Duration::from_secs(1);

    let peer = serve_grpc().await;
    let addr = peer.addr;
    let port = addr.port().to_string();
    let uri: Uri = format!("http://{addr}").parse().unwrap();

    let cfg = no_keepalive(connect_timeout);
    let client = executor_client(cfg);

    // Establish and cache the channel.
    let _ = ping(&client, uri.clone()).await;

    let _blackhole = blackhole_port(&port);

    // Retire the stale socket so every request below takes the connect path.
    let _ = ping(&client, uri.clone()).await;
    eprintln!("[REPRO3] stale socket retired; firing {CONCURRENCY} concurrent calls");

    let started = Instant::now();
    let mut tasks = Vec::new();
    for i in 0..CONCURRENCY {
        let client = client.clone();
        let uri = uri.clone();
        tasks.push(tokio::spawn(async move {
            let t = Instant::now();
            let r = ping(&client, uri).await;
            (i, t.elapsed(), r.err().map(|e| e.code()))
        }));
    }

    let mut elapsed_each = Vec::new();
    for t in tasks {
        let (i, elapsed, code) = t.await.unwrap();
        eprintln!("[REPRO3] call {i}: elapsed={elapsed:?} code={code:?}");
        elapsed_each.push(elapsed);
    }
    let slowest = *elapsed_each.iter().max().unwrap();
    eprintln!(
        "[REPRO3] connect_timeout={:?} concurrency={CONCURRENCY} slowest={:?} wall={:?}",
        connect_timeout,
        slowest,
        started.elapsed()
    );

    assert!(
        slowest < connect_timeout * 2,
        "slowest of {CONCURRENCY} concurrent calls took {slowest:?} with \
         connect_timeout={connect_timeout:?} — queued requests are serialising \
         behind each other's connect attempts instead of sharing one"
    );
}

/// The fix must not cost connection reuse: concurrent calls to a healthy peer
/// must all succeed, and must share a single connection rather than each opening
/// their own.
#[test]
async fn concurrent_calls_to_healthy_peer_share_one_connection() {
    const CONCURRENCY: usize = 16;

    let peer = serve_grpc().await;
    let uri: Uri = format!("http://{}", peer.addr).parse().unwrap();
    let client = executor_client(config());

    let started = Instant::now();
    let calls: Vec<_> = (0..CONCURRENCY)
        .map(|_| {
            let client = client.clone();
            let uri = uri.clone();
            tokio::spawn(async move { ping(&client, uri).await.err().map(|e| e.code()) })
        })
        .collect();

    for call in calls {
        // The empty router answers Unimplemented; what matters is that the call
        // reached the peer rather than failing to connect.
        assert_eq!(call.await.unwrap(), Some(tonic::Code::Unimplemented));
    }
    let elapsed = started.elapsed();
    let connections = peer.connections.load(std::sync::atomic::Ordering::SeqCst);
    eprintln!(
        "[HEALTHY] {CONCURRENCY} concurrent calls over {connections} connection(s) in {elapsed:?}"
    );

    // The point of the test, and the half of it that can go red without any
    // special privileges: a cohort arriving together shares one connection
    // rather than opening one each.
    assert_eq!(
        connections, 1,
        "{CONCURRENCY} concurrent calls opened {connections} connections; they \
         should have shared a single connection attempt"
    );
    // Measured at ~43ms. Loose enough not to flake on a loaded runner, tight
    // enough that serialising the cohort could not slip past.
    assert!(
        elapsed < Duration::from_secs(2),
        "healthy concurrent calls took {elapsed:?}; they should not serialise"
    );
}

/// If the caller that started a connection attempt goes away (upstream timeout,
/// client disconnect), everyone else waiting on that same attempt must still get
/// an answer within connect_timeout rather than stalling on a future nobody
/// drives.
#[test]
async fn waiter_survives_cancellation_of_the_caller_that_started_the_connect() {
    if delegated_to_namespace("waiter_survives_cancellation_of_the_caller_that_started_the_connect")
    {
        return;
    }
    let connect_timeout = Duration::from_secs(2);
    let client = executor_client(no_keepalive(connect_timeout));
    let (uri, _blackhole) = blackholed_target();

    // A starts the attempt.
    let a_client = client.clone();
    let a_uri = uri.clone();
    let a = tokio::spawn(async move { ping(&a_client, a_uri).await });
    tokio::time::sleep(Duration::from_millis(300)).await;

    // B joins the in-flight attempt, then A is cancelled.
    let b_client = client.clone();
    let b_uri = uri.clone();
    let started = Instant::now();
    let b = tokio::spawn(async move { ping(&b_client, b_uri).await });
    tokio::time::sleep(Duration::from_millis(100)).await;
    a.abort();

    let b_result = tokio::time::timeout(connect_timeout * 3, b).await;
    let elapsed = started.elapsed();
    eprintln!(
        "[CANCEL] B settled after {elapsed:?}: {:?}",
        b_result
            .as_ref()
            .map(|r| r.as_ref().map(|i| i.as_ref().err().map(|e| e.code())))
    );
    assert!(
        b_result.is_ok(),
        "waiter B stalled after its driver was cancelled ({elapsed:?}) — Shared future left undriven"
    );
    assert!(
        elapsed < connect_timeout * 2,
        "waiter B took {elapsed:?} with connect_timeout={connect_timeout:?}"
    );
}

/// When every waiter is cancelled, nothing reaches the cleanup that removes the
/// in-flight attempt. The next caller for that target then inherits it.
#[test]
async fn abandoned_connect_attempt_is_not_replayed_to_the_next_caller() {
    if delegated_to_namespace("abandoned_connect_attempt_is_not_replayed_to_the_next_caller") {
        return;
    }
    let connect_timeout = Duration::from_secs(1);
    let client = executor_client(no_keepalive(connect_timeout));
    let (uri, _blackhole) = blackholed_target();

    let a_client = client.clone();
    let a_uri = uri.clone();
    let a = tokio::spawn(async move { ping(&a_client, a_uri).await });
    tokio::time::sleep(Duration::from_millis(100)).await;
    a.abort();

    // Well past the abandoned attempt's deadline.
    tokio::time::sleep(connect_timeout * 2).await;

    let started = Instant::now();
    let _ = ping(&client, uri).await;
    let elapsed = started.elapsed();
    eprintln!("[STALE] next caller settled after {elapsed:?}");

    assert!(
        elapsed >= connect_timeout / 2,
        "next caller returned in {elapsed:?}, far under connect_timeout \
         ({connect_timeout:?}) — it replayed the abandoned attempt's stale \
         outcome instead of making its own"
    );
}

/// Kills a peer cleanly and brings it back on the same address.
///
/// Note what this does and does not cover. A killed process closes its sockets,
/// so the client gets an RST and tonic's own `Reconnect` layer recovers without
/// help: this test passes with our channel eviction disabled entirely, which was
/// verified by removing it. It is here because that recovery is worth holding
/// onto, not as evidence for eviction. Eviction proper needs a peer that goes
/// silent instead of refusing, which is
/// `blackholed_channel_is_evicted_and_recovers_when_the_peer_answers_again`, and
/// that one cannot run without CAP_NET_ADMIN.
#[test]
async fn client_recovers_when_a_cleanly_killed_peer_returns() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);

    fn start(
        addr: std::net::SocketAddr,
    ) -> (
        tokio::sync::oneshot::Sender<()>,
        std::thread::JoinHandle<()>,
    ) {
        let (tx, rx) = tokio::sync::oneshot::channel::<()>();
        let handle = std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .unwrap();
            rt.block_on(async move {
                let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
                let incoming = tokio_stream::wrappers::TcpListenerStream::new(listener);
                tokio::select! {
                    _ = tonic::transport::Server::builder()
                        .add_routes(tonic::service::Routes::default())
                        .serve_with_incoming(incoming) => {}
                    _ = rx => {}
                }
            });
            // Drops the per-connection tasks tonic spawned, closing the sockets.
            rt.shutdown_timeout(Duration::ZERO);
        });
        (tx, handle)
    }

    let client = executor_client(config());
    let uri: Uri = format!("http://{addr}").parse().unwrap();

    // A live peer answers. The empty router replies Unimplemented, which proves
    // the transport works and is deliberately not Unavailable, so the channel is
    // cached exactly as it is in production.
    let (kill, joined) = start(addr);
    tokio::time::sleep(Duration::from_millis(300)).await;
    let first = ping(&client, uri.clone()).await;
    eprintln!(
        "[EVICT] live peer: {:?}",
        first.as_ref().err().map(|e| e.code())
    );
    assert_eq!(
        first.err().map(|e| e.code()),
        Some(tonic::Code::Unimplemented),
        "peer should be reachable before the kill"
    );

    // Kill it. The cached channel is now dead.
    let _ = kill.send(());
    joined.join().unwrap();
    let during = ping(&client, uri.clone()).await;
    let code = during.as_ref().err().map(|e| e.code());
    eprintln!(
        "[EVICT] dead peer: {code:?} msg={:?}",
        during.as_ref().err().map(|e| e.message().to_string())
    );

    // Same address, fresh peer. Only an evicted channel can reach it.
    let (kill2, joined2) = start(addr);
    tokio::time::sleep(Duration::from_millis(300)).await;
    let after = ping(&client, uri.clone()).await;
    let after_code = after.as_ref().err().map(|e| e.code());
    eprintln!(
        "[EVICT] restarted peer: {after_code:?} msg={:?}",
        after.as_ref().err().map(|e| e.message().to_string())
    );
    let _ = kill2.send(());
    joined2.join().unwrap();

    assert_eq!(
        after_code,
        Some(tonic::Code::Unimplemented),
        "client did not recover after the peer returned on the same address — \
         the dead channel was never evicted"
    );
}

/// The blackhole path, which is what production actually hit: a cached channel
/// whose peer stops answering without sending RST. If the resulting error is not
/// recognised as needing a reconnect, the channel stays cached forever and every
/// later request queues onto a connection that can never work again.
#[test]
async fn blackholed_channel_is_evicted_and_recovers_when_the_peer_answers_again() {
    if delegated_to_namespace(
        "blackholed_channel_is_evicted_and_recovers_when_the_peer_answers_again",
    ) {
        return;
    }

    let peer = serve_grpc().await;
    let addr = peer.addr;
    let port = addr.port().to_string();
    let uri: Uri = format!("http://{addr}").parse().unwrap();
    let connect_timeout = Duration::from_secs(2);
    let client = executor_client(no_keepalive(connect_timeout));

    let first = ping(&client, uri.clone()).await;
    assert_eq!(
        first.err().map(|e| e.code()),
        Some(tonic::Code::Unimplemented)
    );

    let blackhole = blackhole_port(&port);

    let dead = ping(&client, uri.clone()).await;
    let dead_err = dead.expect_err("blackholed peer must fail");
    let has_transport_source = std::error::Error::source(&dead_err)
        .map(|s| s.is::<tonic::transport::Error>())
        .unwrap_or(false);
    eprintln!(
        "[EVICT2] blackholed: code={:?} msg={:?} transport_source={has_transport_source}",
        dead_err.code(),
        dead_err.message()
    );
    // The predicate that decides eviction keys off exactly this, so pin it: a
    // dead transport must stay recognisable even though its code is not
    // Unavailable.
    assert!(
        has_transport_source,
        "a dead transport no longer carries a tonic::transport::Error source, so \
         requires_reconnect cannot recognise it and the channel would stay cached"
    );

    drop(blackhole);

    let recovered = ping(&client, uri).await;
    let code = recovered.as_ref().err().map(|e| e.code());
    eprintln!("[EVICT2] after un-blackholing: code={code:?}");
    assert_eq!(
        code,
        Some(tonic::Code::Unimplemented),
        "channel was not evicted: the client never recovered even though the peer answers again"
    );
}

/// Counts how many TCP connections a cohort of callers actually opens.
///
/// Latency alone cannot see the difference: duplicate attempts run concurrently,
/// so the wall clock stays flat while the connect count multiplies. Only a count
/// distinguishes "one attempt shared by everyone" from "one attempt each".
#[test]
async fn a_cohort_that_retries_opens_one_connection_per_round() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let accepted = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counter = accepted.clone();
    let _accepter = tokio::spawn(async move {
        let mut held = Vec::new();
        // Accept, count, and then stay silent, so every attempt has to be given
        // up on rather than completing.
        while let Ok((sock, _)) = listener.accept().await {
            counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            held.push(sock);
        }
    });

    // shard-manager's executor client really does run with retries enabled
    // (max_attempts = 5 in shard-manager.toml), so the retry path is not
    // hypothetical. Two retries keep the test short.
    const CALLERS: usize = 6;
    const MAX_ATTEMPTS: u64 = 2;
    let mut cfg = config();
    cfg.connect_timeout = Duration::from_secs(1);
    cfg.http2_keep_alive_interval = Some(Duration::from_millis(300));
    cfg.http2_keep_alive_timeout = Some(Duration::from_millis(300));
    cfg.retries_on_unavailable = RetryConfig {
        max_attempts: MAX_ATTEMPTS as u32,
        min_delay: Duration::from_millis(10),
        max_delay: Duration::from_millis(50),
        multiplier: 1.0,
        max_jitter_factor: None,
    };

    let client = executor_client(cfg);
    let uri: Uri = format!("http://{addr}").parse().unwrap();

    let started = Instant::now();
    let calls = (0..CALLERS).map(|_| {
        let client = client.clone();
        let uri = uri.clone();
        tokio::spawn(async move { ping(&client, uri).await })
    });
    for call in calls.collect::<Vec<_>>() {
        let _ = call.await;
    }

    let connects = accepted.load(std::sync::atomic::Ordering::SeqCst);
    let rounds = MAX_ATTEMPTS as usize + 1;
    eprintln!(
        "[COHORT] {CALLERS} callers, {rounds} rounds: {connects} TCP connects in {:?}",
        started.elapsed()
    );
    assert!(
        connects <= rounds,
        "{CALLERS} callers opened {connects} connections across {rounds} retry rounds; \
         single-flight should have opened at most one per round"
    );
}

/// The same cohort question, but where the connects *fail*. The previous test
/// cannot see this: there the TCP connect succeeds, so the channel is cached and
/// later rounds open nothing at all. Here every attempt has to be given up on,
/// which is the case the incident was about.
///
/// Counted by dropping outbound SYNs and reading the rule's packet counter,
/// because a blackholed peer accepts nothing that could be counted at the far end.
#[test]
async fn a_cohort_retrying_into_a_blackhole_opens_one_connection_per_round() {
    if delegated_to_namespace("a_cohort_retrying_into_a_blackhole_opens_one_connection_per_round") {
        return;
    }
    // One SYN per attempt, so the count is attempts rather than retransmits.
    let _syn_retries = Sysctl::set("/proc/sys/net/ipv4/tcp_syn_retries", "1");

    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port().to_string();
    drop(listener);

    let _drop_syns = FirewallRule::add(&[
        "OUTPUT",
        "-p",
        "tcp",
        "-d",
        "127.0.0.1",
        "--dport",
        &port,
        "--syn",
        "-j",
        "DROP",
    ]);

    const CALLERS: usize = 6;
    const MAX_ATTEMPTS: u32 = 2;
    let mut cfg = no_keepalive(Duration::from_millis(800));
    cfg.retries_on_unavailable = RetryConfig {
        max_attempts: MAX_ATTEMPTS,
        min_delay: Duration::from_millis(10),
        max_delay: Duration::from_millis(50),
        multiplier: 1.0,
        max_jitter_factor: None,
    };

    let client = executor_client(cfg);
    let uri: Uri = format!("http://127.0.0.1:{port}").parse().unwrap();

    let started = Instant::now();
    let calls: Vec<_> = (0..CALLERS)
        .map(|_| {
            let client = client.clone();
            let uri = uri.clone();
            tokio::spawn(async move { ping(&client, uri).await })
        })
        .collect();
    for call in calls {
        let _ = call.await;
    }
    let elapsed = started.elapsed();

    let counters = std::process::Command::new("iptables")
        .args(["-L", "OUTPUT", "-v", "-x", "-n"])
        .output()
        .unwrap();
    let counters = String::from_utf8_lossy(&counters.stdout);
    let syns: usize = counters
        .lines()
        .find(|line| line.contains(&format!("dpt:{port}")))
        .and_then(|line| line.split_whitespace().next())
        .and_then(|pkts| pkts.parse().ok())
        .expect("the DROP rule must be present with a packet counter");

    let rounds = MAX_ATTEMPTS as usize + 1;
    eprintln!(
        "[SYN] {CALLERS} callers, {rounds} rounds: {syns} SYNs sent in {elapsed:?} \
         (single-flight expects ~{rounds}, one attempt each expects ~{})",
        CALLERS * rounds
    );
    assert!(
        syns <= rounds * 2,
        "{CALLERS} callers sent {syns} SYNs across {rounds} retry rounds; single-flight \
         should have opened about one connection per round, not one per caller per round"
    );
}

/// A connection attempt outlives its callers by design, so that losing them does
/// not strand it mid-connect. This is the other half of that bargain: when the
/// attempt succeeds after everyone has gone, the connection it made must end up
/// somewhere the next caller can find it. Otherwise it sits open with nobody able
/// to use it, pinging away, until some later call happens to replace it.
#[test]
async fn a_connection_nobody_waited_for_is_still_reused() {
    if delegated_to_namespace("a_connection_nobody_waited_for_is_still_reused") {
        return;
    }
    // Slow the loopback so a connect can be cancelled while it is still in
    // flight. Without this the handshake completes far too fast to interleave.
    let tc = std::process::Command::new("tc")
        .args([
            "qdisc", "add", "dev", "lo", "root", "netem", "delay", "300ms",
        ])
        .output()
        .expect("tc must be available; it comes with iproute2");
    assert!(
        tc.status.success(),
        "tc: {}",
        String::from_utf8_lossy(&tc.stderr)
    );

    let peer = serve_grpc().await;
    let uri: Uri = format!("http://{}", peer.addr).parse().unwrap();
    let mut cfg = config();
    cfg.connect_timeout = Duration::from_secs(5);
    let client = executor_client(cfg);

    let abandoned = tokio::spawn({
        let client = client.clone();
        let uri = uri.clone();
        async move { ping(&client, uri).await }
    });
    // Cancel while the handshake is still crossing the delayed loopback.
    tokio::time::sleep(Duration::from_millis(200)).await;
    abandoned.abort();

    // Long enough for the attempt to finish on its own.
    tokio::time::sleep(Duration::from_secs(2)).await;

    let outcome = ping(&client, uri).await;
    let connections = peer.connections.load(std::sync::atomic::Ordering::SeqCst);
    eprintln!(
        "[ORPHAN] call after the abandoned one used {connections} connection(s): {outcome:?}"
    );

    let _ = std::process::Command::new("tc")
        .args(["qdisc", "del", "dev", "lo", "root"])
        .output();

    assert_eq!(
        outcome.err().map(|e| e.code()),
        Some(tonic::Code::Unimplemented),
        "the call after the abandoned one should have reached the peer"
    );
    assert_eq!(
        connections, 1,
        "the abandoned attempt's connection was not reused; it opened \
         {connections} connections where one would do"
    );
}

/// A request that ran out of time says nothing about the connection it ran on.
///
/// tonic reports a `request_timeout` as `Cancelled` and still attaches a
/// transport error to it, which is indistinguishable by source alone from a
/// genuinely dead connection. Treating it as one would tear down a healthy
/// channel that every other caller is sharing, and worker-executor's registry
/// client really does set request_timeout (30s).
#[test]
async fn a_request_timeout_does_not_tear_down_the_shared_connection() {
    let peer = serve_grpc().await;
    let uri: Uri = format!("http://{}", peer.addr).parse().unwrap();

    let mut cfg = config();
    cfg.request_timeout = Some(Duration::from_millis(300));
    let client = executor_client(cfg);

    // The empty router answers immediately, so this establishes and caches a
    // connection without timing anything out.
    assert_eq!(
        ping(&client, uri.clone()).await.err().map(|e| e.code()),
        Some(tonic::Code::Unimplemented)
    );

    // A call that does time out, against a peer that is perfectly healthy.
    let timed_out = single_target_executor_client(uri.clone(), {
        let mut cfg = no_keepalive(Duration::from_secs(2));
        cfg.request_timeout = Some(Duration::from_millis(300));
        cfg
    });
    let _ = ping_single(&timed_out).await;

    assert_eq!(
        ping(&client, uri).await.err().map(|e| e.code()),
        Some(tonic::Code::Unimplemented)
    );
    let connections = peer.connections.load(std::sync::atomic::Ordering::SeqCst);
    eprintln!("[TIMEOUT] connections opened by the client that timed out: {connections}");
    assert!(
        connections <= 2,
        "a request timeout reconnected instead of reusing the channel: \
         {connections} connections opened"
    );
}
