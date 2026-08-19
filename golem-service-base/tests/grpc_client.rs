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

/// RFC 5737 TEST-NET-1. Reserved for documentation and guaranteed never routable,
/// so a TCP connect to it is silently dropped rather than refused. That is exactly
/// what a deleted Kubernetes pod IP does: no RST, no ICMP, just silence.
///
/// A closed port on localhost would NOT do: it answers with RST immediately, so
/// the connect fails fast and any timeout bug stays invisible.
const BLACKHOLE: &str = "http://192.0.2.1:9093";

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
        tcp_keepalive: None,
        buffer_size: None,
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

/// A worker-executor pod deleted mid-flight leaves its IP blackholed. Calls
/// already routed to it must give up after `connect_timeout` so the caller can
/// re-resolve and retry against the executor that took over its shards.
///
/// Observed on golem-dev 2026-08-19 (chaos S5): one attempt against a deleted pod
/// took 119,781ms against a configured 10s connect_timeout, stalling 496
/// operations for two minutes each.
#[test]
async fn multi_target_call_to_blackholed_pod_gives_up_within_connect_timeout() {
    let client = MultiTargetGrpcClient::new(
        "worker_executor",
        |channel, max_message_size| {
            WorkerExecutorClient::new(channel)
                .max_decoding_message_size(max_message_size)
                .max_encoding_message_size(max_message_size)
        },
        config(),
    );

    let started = Instant::now();
    let result = client
        .call(
            "assign_shards",
            BLACKHOLE.parse::<Uri>().unwrap(),
            move |client| Box::pin(client.assign_shards(AssignShardsRequest { shard_ids: vec![] })),
        )
        .await;
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
    let client = GrpcClient::new(
        "worker_executor",
        |channel, max_message_size| {
            WorkerExecutorClient::new(channel)
                .max_decoding_message_size(max_message_size)
                .max_encoding_message_size(max_message_size)
        },
        BLACKHOLE.parse::<Uri>().unwrap(),
        config(),
    );

    let started = Instant::now();
    let result = client
        .call("assign_shards", move |client| {
            Box::pin(client.assign_shards(AssignShardsRequest { shard_ids: vec![] }))
        })
        .await;
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
///   - `tcp_keepalive` — never fires; the peer's kernel still ACKs, it just says nothing
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
    let client = MultiTargetGrpcClient::new(
        "worker_executor",
        |channel, max_message_size| {
            WorkerExecutorClient::new(channel)
                .max_decoding_message_size(max_message_size)
                .max_encoding_message_size(max_message_size)
        },
        config(),
    );

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
// Reproduction harness for the S5 stall (chaos run 32272077341, 2026-08-19).
//
// Requires a network namespace with CAP_NET_ADMIN so packets can be dropped
// rather than refused — a deleted pod IP blackholes, it does not send RST.
// Ignored by default; run with:
//
//   unshare -rn bash -c 'ip link set lo up; echo 4 > /proc/sys/net/ipv4/tcp_retries2;
//     exec target/debug/deps/integration-<hash> established_connection'
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

async fn serve_grpc() -> (std::net::SocketAddr, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let incoming = tokio_stream::wrappers::TcpListenerStream::new(listener);
    let handle = tokio::spawn(async move {
        // Empty router: any request gets a real HTTP/2 response carrying
        // grpc-status 12 (Unimplemented). That is not Unavailable, so the client
        // keeps the channel cached — exactly as in production.
        let _ = tonic::transport::Server::builder()
            .add_routes(tonic::service::Routes::default())
            .serve_with_incoming(incoming)
            .await;
    });
    (addr, handle)
}

/// Whether it is safe to drop packets here.
///
/// These tests install firewall rules, so they must only ever run inside a
/// throwaway network namespace. Two things would otherwise go wrong: on macOS
/// there is no iptables at all, and running the suite as root on a Linux host
/// would install DROP rules in that host's real firewall.
///
/// "Throwaway" is taken to mean loopback is the only interface, which is what
/// `unshare -rn` gives you and what a developer machine or CI runner is not.
fn can_drop_packets() -> bool {
    if !cfg!(target_os = "linux") || !loopback_is_the_only_interface() {
        return false;
    }
    std::process::Command::new("iptables")
        .arg("-S")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Reads `/proc/net/dev` rather than `/sys/class/net`: sysfs is not remounted by
/// `unshare -rn`, so it keeps reporting the host's interfaces, while `/proc/net`
/// resolves per network namespace.
fn loopback_is_the_only_interface() -> bool {
    let Ok(devices) = std::fs::read_to_string("/proc/net/dev") else {
        return false;
    };
    let mut names = devices
        .lines()
        .skip(2)
        .filter_map(|line| line.split(':').next())
        .map(str::trim);
    names.next() == Some("lo") && names.next().is_none()
}

/// Whether a connect to [`BLACKHOLE`] actually hangs here.
///
/// TEST-NET-1 behaves like a deleted pod only where a default route exists to
/// swallow the packets. Inside `unshare -rn` there is no route to it at all, so
/// the connect fails instantly with "network unreachable" rather than hanging,
/// and a test that measures how long a hang lasts would pass without ever
/// reproducing what it claims to.
fn blackhole_hangs() -> bool {
    !loopback_is_the_only_interface()
}

/// Silently drops every packet to and from `port` for as long as it is held, so
/// the peer behaves like a deleted pod: no RST, no ICMP, just silence. Removing
/// the rules on drop keeps a panicking test from leaving the namespace's
/// firewall in a state that fails every test after it.
#[must_use = "the rules are removed as soon as this is dropped"]
struct Blackhole {
    port: String,
}

impl Blackhole {
    fn new(port: &str) -> Self {
        for direction in ["--dport", "--sport"] {
            iptables(&["-A", "INPUT", "-p", "tcp", direction, port, "-j", "DROP"]);
        }
        Self {
            port: port.to_string(),
        }
    }
}

impl Drop for Blackhole {
    fn drop(&mut self) {
        for direction in ["--dport", "--sport"] {
            let _ = std::process::Command::new("iptables")
                .args([
                    "-D", "INPUT", "-p", "tcp", direction, &self.port, "-j", "DROP",
                ])
                .output();
        }
    }
}

#[test]
async fn established_connection_to_blackholed_peer() {
    if !can_drop_packets() {
        eprintln!(
            "[REPRO] skipped: needs CAP_NET_ADMIN in a network namespace. \
             Run under `unshare -rn`."
        );
        return;
    }

    let (addr, _server) = serve_grpc().await;
    let port = addr.port().to_string();
    let uri: Uri = format!("http://{addr}").parse().unwrap();

    let connect_timeout = Duration::from_secs(3);
    let client = MultiTargetGrpcClient::new(
        "worker_executor",
        |channel, max_message_size| {
            WorkerExecutorClient::new(channel)
                .max_decoding_message_size(max_message_size)
                .max_encoding_message_size(max_message_size)
        },
        no_keepalive(connect_timeout),
    );

    // 1. Establish and cache the connection, as production had done.
    let first = client
        .call("assign_shards", uri.clone(), move |client| {
            Box::pin(client.assign_shards(AssignShardsRequest { shard_ids: vec![] }))
        })
        .await;
    let first_err = first.expect_err("empty router replies Unimplemented");
    eprintln!("[REPRO] first call settled: code={:?}", first_err.code());
    assert_ne!(
        first_err.code(),
        tonic::Code::Unavailable,
        "first call must not be Unavailable, or the channel is dropped and the \
         repro degenerates into the fresh-connect case"
    );

    // 2. The pod vanishes: its packets are silently dropped in both directions.
    let _blackhole = Blackhole::new(&port);
    eprintln!("[REPRO] peer blackholed on port {port}");

    // 3. Next request on the cached, now-dead connection.
    let started = Instant::now();
    let result = client
        .call("assign_shards", uri, move |client| {
            Box::pin(client.assign_shards(AssignShardsRequest { shard_ids: vec![] }))
        })
        .await;
    let elapsed = started.elapsed();

    let err = result.expect_err("call to a vanished peer must fail");
    eprintln!(
        "[REPRO] elapsed={:?} connect_timeout={:?} code={:?}\n[REPRO] error={:?}",
        elapsed,
        connect_timeout,
        err.code(),
        err
    );

    assert!(
        elapsed > connect_timeout,
        "expected the stall to exceed connect_timeout ({connect_timeout:?}), got {elapsed:?} — \
         no stall reproduced"
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
    if !can_drop_packets() {
        eprintln!("[REPRO2] skipped: needs CAP_NET_ADMIN in a network namespace.");
        return;
    }

    let (addr, _server) = serve_grpc().await;
    let port = addr.port().to_string();
    let uri: Uri = format!("http://{addr}").parse().unwrap();

    let connect_timeout = Duration::from_secs(3);
    let client = MultiTargetGrpcClient::new(
        "worker_executor",
        |channel, max_message_size| {
            WorkerExecutorClient::new(channel)
                .max_decoding_message_size(max_message_size)
                .max_encoding_message_size(max_message_size)
        },
        no_keepalive(connect_timeout),
    );

    let first = client
        .call("assign_shards", uri.clone(), move |client| {
            Box::pin(client.assign_shards(AssignShardsRequest { shard_ids: vec![] }))
        })
        .await;
    eprintln!(
        "[REPRO2] first call settled: code={:?}",
        first.err().map(|e| e.code())
    );

    // Pod IP stops routing.
    let _blackhole = Blackhole::new(&port);
    eprintln!("[REPRO2] peer blackholed on port {port}");

    // Second call dies on the stale socket once the kernel gives up.
    let t2 = Instant::now();
    let r2 = client
        .call("assign_shards", uri.clone(), move |client| {
            Box::pin(client.assign_shards(AssignShardsRequest { shard_ids: vec![] }))
        })
        .await;
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
    let result = client
        .call("assign_shards", uri, move |client| {
            Box::pin(client.assign_shards(AssignShardsRequest { shard_ids: vec![] }))
        })
        .await;
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
    if !can_drop_packets() {
        eprintln!("[REPRO3] skipped: needs CAP_NET_ADMIN in a network namespace.");
        return;
    }

    const CONCURRENCY: usize = 8;
    let connect_timeout = Duration::from_secs(1);

    let (addr, _server) = serve_grpc().await;
    let port = addr.port().to_string();
    let uri: Uri = format!("http://{addr}").parse().unwrap();

    let cfg = no_keepalive(connect_timeout);
    let client = MultiTargetGrpcClient::new(
        "worker_executor",
        |channel, max_message_size| {
            WorkerExecutorClient::new(channel)
                .max_decoding_message_size(max_message_size)
                .max_encoding_message_size(max_message_size)
        },
        cfg,
    );

    // Establish and cache the channel.
    let _ = client
        .call("assign_shards", uri.clone(), move |client| {
            Box::pin(client.assign_shards(AssignShardsRequest { shard_ids: vec![] }))
        })
        .await;

    let _blackhole = Blackhole::new(&port);

    // Retire the stale socket so every request below takes the connect path.
    let _ = client
        .call("assign_shards", uri.clone(), move |client| {
            Box::pin(client.assign_shards(AssignShardsRequest { shard_ids: vec![] }))
        })
        .await;
    eprintln!("[REPRO3] stale socket retired; firing {CONCURRENCY} concurrent calls");

    let started = Instant::now();
    let mut tasks = Vec::new();
    for i in 0..CONCURRENCY {
        let client = client.clone();
        let uri = uri.clone();
        tasks.push(tokio::spawn(async move {
            let t = Instant::now();
            let r = client
                .call("assign_shards", uri, move |client| {
                    Box::pin(client.assign_shards(AssignShardsRequest { shard_ids: vec![] }))
                })
                .await;
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
    let (addr, _server) = serve_grpc().await;
    let uri: Uri = format!("http://{addr}").parse().unwrap();

    let client = MultiTargetGrpcClient::new(
        "worker_executor",
        |channel, max_message_size| {
            WorkerExecutorClient::new(channel)
                .max_decoding_message_size(max_message_size)
                .max_encoding_message_size(max_message_size)
        },
        config(),
    );

    let started = Instant::now();
    let mut tasks = Vec::new();
    for _ in 0..16 {
        let client = client.clone();
        let uri = uri.clone();
        tasks.push(tokio::spawn(async move {
            client
                .call("assign_shards", uri, move |client| {
                    Box::pin(client.assign_shards(AssignShardsRequest { shard_ids: vec![] }))
                })
                .await
                .err()
                .map(|e| e.code())
        }));
    }

    for t in tasks {
        // The empty router answers Unimplemented; what matters is that the call
        // reached the peer rather than failing to connect.
        assert_eq!(t.await.unwrap(), Some(tonic::Code::Unimplemented));
    }
    eprintln!(
        "[HEALTHY] 16 concurrent calls settled in {:?}",
        started.elapsed()
    );
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "healthy concurrent calls should not serialise"
    );
}

/// If the caller that started a connection attempt goes away (upstream timeout,
/// client disconnect), everyone else waiting on that same attempt must still get
/// an answer within connect_timeout rather than stalling on a future nobody
/// drives.
#[test]
async fn waiter_survives_cancellation_of_the_caller_that_started_the_connect() {
    if !blackhole_hangs() {
        eprintln!(
            "[CANCEL] skipped: no route to TEST-NET-1 here, so the connect fails \
             instantly instead of hanging. Run outside `unshare -rn`."
        );
        return;
    }
    let connect_timeout = Duration::from_secs(2);
    let client = MultiTargetGrpcClient::new(
        "worker_executor",
        |channel, max_message_size| {
            WorkerExecutorClient::new(channel)
                .max_decoding_message_size(max_message_size)
                .max_encoding_message_size(max_message_size)
        },
        no_keepalive(connect_timeout),
    );
    let uri: Uri = BLACKHOLE.parse().unwrap();

    // A starts the attempt.
    let a_client = client.clone();
    let a_uri = uri.clone();
    let a = tokio::spawn(async move {
        a_client
            .call("assign_shards", a_uri, move |c| {
                Box::pin(c.assign_shards(AssignShardsRequest { shard_ids: vec![] }))
            })
            .await
    });
    tokio::time::sleep(Duration::from_millis(300)).await;

    // B joins the in-flight attempt, then A is cancelled.
    let b_client = client.clone();
    let b_uri = uri.clone();
    let started = Instant::now();
    let b = tokio::spawn(async move {
        b_client
            .call("assign_shards", b_uri, move |c| {
                Box::pin(c.assign_shards(AssignShardsRequest { shard_ids: vec![] }))
            })
            .await
    });
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
    if !blackhole_hangs() {
        eprintln!(
            "[STALE] skipped: no route to TEST-NET-1 here, so the connect fails \
             instantly instead of hanging. Run outside `unshare -rn`."
        );
        return;
    }
    let connect_timeout = Duration::from_secs(1);
    let client = MultiTargetGrpcClient::new(
        "worker_executor",
        |channel, max_message_size| {
            WorkerExecutorClient::new(channel)
                .max_decoding_message_size(max_message_size)
                .max_encoding_message_size(max_message_size)
        },
        no_keepalive(connect_timeout),
    );
    let uri: Uri = BLACKHOLE.parse().unwrap();

    let a_client = client.clone();
    let a_uri = uri.clone();
    let a = tokio::spawn(async move {
        a_client
            .call("assign_shards", a_uri, move |c| {
                Box::pin(c.assign_shards(AssignShardsRequest { shard_ids: vec![] }))
            })
            .await
    });
    tokio::time::sleep(Duration::from_millis(100)).await;
    a.abort();

    // Well past the abandoned attempt's deadline.
    tokio::time::sleep(connect_timeout * 2).await;

    let started = Instant::now();
    let _ = client
        .call("assign_shards", uri, move |c| {
            Box::pin(c.assign_shards(AssignShardsRequest { shard_ids: vec![] }))
        })
        .await;
    let elapsed = started.elapsed();
    eprintln!("[STALE] next caller settled after {elapsed:?}");

    assert!(
        elapsed >= connect_timeout / 2,
        "next caller returned in {elapsed:?}, far under connect_timeout \
         ({connect_timeout:?}) — it replayed the abandoned attempt's stale \
         outcome instead of making its own"
    );
}

/// Kills a peer and brings it back on the same address. The dead channel must be
/// evicted, otherwise every later request is routed onto a connection that can
/// never work again — the first of the two defects this PR claims to fix.
#[test]
async fn dead_channel_is_evicted_when_the_peer_returns_on_the_same_address() {
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

    let client = MultiTargetGrpcClient::new(
        "worker_executor",
        |channel, max_message_size| {
            WorkerExecutorClient::new(channel)
                .max_decoding_message_size(max_message_size)
                .max_encoding_message_size(max_message_size)
        },
        config(),
    );
    let uri: Uri = format!("http://{addr}").parse().unwrap();
    let ping = |uri: Uri| {
        let client = client.clone();
        async move {
            client
                .call("assign_shards", uri, move |c| {
                    Box::pin(c.assign_shards(AssignShardsRequest { shard_ids: vec![] }))
                })
                .await
        }
    };

    // A live peer answers. The empty router replies Unimplemented, which proves
    // the transport works and is deliberately not Unavailable, so the channel is
    // cached exactly as it is in production.
    let (kill, joined) = start(addr);
    tokio::time::sleep(Duration::from_millis(300)).await;
    let first = ping(uri.clone()).await;
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
    let during = ping(uri.clone()).await;
    let code = during.as_ref().err().map(|e| e.code());
    eprintln!(
        "[EVICT] dead peer: {code:?} msg={:?}",
        during.as_ref().err().map(|e| e.message().to_string())
    );

    // Same address, fresh peer. Only an evicted channel can reach it.
    let (kill2, joined2) = start(addr);
    tokio::time::sleep(Duration::from_millis(300)).await;
    let after = ping(uri.clone()).await;
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
    if !can_drop_packets() {
        eprintln!("[EVICT2] skipped: needs CAP_NET_ADMIN. Run under `unshare -rn`.");
        return;
    }

    let (addr, _server) = serve_grpc().await;
    let port = addr.port().to_string();
    let uri: Uri = format!("http://{addr}").parse().unwrap();
    let connect_timeout = Duration::from_secs(2);
    let client = MultiTargetGrpcClient::new(
        "worker_executor",
        |channel, max_message_size| {
            WorkerExecutorClient::new(channel)
                .max_decoding_message_size(max_message_size)
                .max_encoding_message_size(max_message_size)
        },
        no_keepalive(connect_timeout),
    );
    let ping = |uri: Uri| {
        let client = client.clone();
        async move {
            client
                .call("assign_shards", uri, move |c| {
                    Box::pin(c.assign_shards(AssignShardsRequest { shard_ids: vec![] }))
                })
                .await
        }
    };

    let first = ping(uri.clone()).await;
    assert_eq!(
        first.err().map(|e| e.code()),
        Some(tonic::Code::Unimplemented)
    );

    let blackhole = Blackhole::new(&port);

    let dead = ping(uri.clone()).await;
    let dead_err = dead.expect_err("blackholed peer must fail");
    let has_transport_source = std::error::Error::source(&dead_err)
        .map(|s| s.is::<tonic::transport::Error>())
        .unwrap_or(false);
    eprintln!(
        "[EVICT2] blackholed: code={:?} msg={:?} transport_source={has_transport_source}",
        dead_err.code(),
        dead_err.message()
    );

    drop(blackhole);

    let recovered = ping(uri).await;
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

    let client = MultiTargetGrpcClient::new(
        "worker_executor",
        |channel, max_message_size| {
            WorkerExecutorClient::new(channel)
                .max_decoding_message_size(max_message_size)
                .max_encoding_message_size(max_message_size)
        },
        cfg,
    );
    let uri: Uri = format!("http://{addr}").parse().unwrap();

    let started = Instant::now();
    let calls = (0..CALLERS).map(|_| {
        let client = client.clone();
        let uri = uri.clone();
        tokio::spawn(async move {
            client
                .call("assign_shards", uri, move |c| {
                    Box::pin(c.assign_shards(AssignShardsRequest { shard_ids: vec![] }))
                })
                .await
        })
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
    if !can_drop_packets() {
        eprintln!("[SYN] skipped: needs CAP_NET_ADMIN. Run under `unshare -rn`.");
        return;
    }
    // One SYN per attempt, so the count is attempts rather than retransmits.
    std::fs::write("/proc/sys/net/ipv4/tcp_syn_retries", "1").unwrap();

    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port().to_string();
    drop(listener);

    let drop_syns = [
        "-p",
        "tcp",
        "-d",
        "127.0.0.1",
        "--dport",
        &port,
        "--syn",
        "-j",
        "DROP",
    ];
    let mut rule = vec!["-A", "OUTPUT"];
    rule.extend_from_slice(&drop_syns);
    iptables(&rule);

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

    let client = MultiTargetGrpcClient::new(
        "worker_executor",
        |channel, max_message_size| {
            WorkerExecutorClient::new(channel)
                .max_decoding_message_size(max_message_size)
                .max_encoding_message_size(max_message_size)
        },
        cfg,
    );
    let uri: Uri = format!("http://127.0.0.1:{port}").parse().unwrap();

    let started = Instant::now();
    let calls: Vec<_> = (0..CALLERS)
        .map(|_| {
            let client = client.clone();
            let uri = uri.clone();
            tokio::spawn(async move {
                client
                    .call("assign_shards", uri, move |c| {
                        Box::pin(c.assign_shards(AssignShardsRequest { shard_ids: vec![] }))
                    })
                    .await
            })
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

    let mut remove = vec!["-D", "OUTPUT"];
    remove.extend_from_slice(&drop_syns);
    iptables(&remove);

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
