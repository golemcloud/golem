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
            move |client| {
                Box::pin(client.assign_shards(AssignShardsRequest { shard_ids: vec![] }))
            },
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
        loop {
            match listener.accept().await {
                // Hold the socket open forever without writing anything.
                Ok((sock, _)) => held.push(sock),
                Err(_) => break,
            }
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

fn can_drop_packets() -> bool {
    std::process::Command::new("iptables")
        .arg("-S")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
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
    let first_err = first.err().expect("empty router replies Unimplemented");
    eprintln!("[REPRO] first call settled: code={:?}", first_err.code());
    assert_ne!(
        first_err.code(),
        tonic::Code::Unavailable,
        "first call must not be Unavailable, or the channel is dropped and the \
         repro degenerates into the fresh-connect case"
    );

    // 2. The pod vanishes: its packets are silently dropped in both directions.
    iptables(&["-A", "INPUT", "-p", "tcp", "--dport", &port, "-j", "DROP"]);
    iptables(&["-A", "INPUT", "-p", "tcp", "--sport", &port, "-j", "DROP"]);
    eprintln!("[REPRO] peer blackholed on port {port}");

    // 3. Next request on the cached, now-dead connection.
    let started = Instant::now();
    let result = client
        .call("assign_shards", uri, move |client| {
            Box::pin(client.assign_shards(AssignShardsRequest { shard_ids: vec![] }))
        })
        .await;
    let elapsed = started.elapsed();

    let err = result.err().expect("call to a vanished peer must fail");
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

    let (addr, server) = serve_grpc().await;
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
    iptables(&["-A", "INPUT", "-p", "tcp", "--dport", &port, "-j", "DROP"]);
    iptables(&["-A", "INPUT", "-p", "tcp", "--sport", &port, "-j", "DROP"]);
    let _ = server;
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
    let err = result.err().expect("call to a dead peer must fail");

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

/// tonic's `Channel` is a `tower::Buffer`: concurrent requests queue and are
/// serviced one at a time. If the peer is unreachable, every queued request
/// waits for the one ahead of it to burn a full `connect_timeout` before its own
/// connect is even attempted.
///
/// That would make a single logical attempt take N x connect_timeout, with the
/// error still reported as one ConnectError/Elapsed — which is what S5 showed:
/// 119,781ms on one attempt against a 10s connect_timeout, with 496 operations
/// in flight against the executor that had just been killed.
#[test]
async fn concurrent_calls_queue_behind_each_others_connect_attempts() {
    if !can_drop_packets() {
        eprintln!("[REPRO3] skipped: needs CAP_NET_ADMIN in a network namespace.");
        return;
    }

    const CONCURRENCY: usize = 8;
    let connect_timeout = Duration::from_secs(1);

    let (addr, _server) = serve_grpc().await;
    let port = addr.port().to_string();
    let uri: Uri = format!("http://{addr}").parse().unwrap();

    let mut cfg = no_keepalive(connect_timeout);
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

    iptables(&["-A", "INPUT", "-p", "tcp", "--dport", &port, "-j", "DROP"]);
    iptables(&["-A", "INPUT", "-p", "tcp", "--sport", &port, "-j", "DROP"]);

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

    // Characterisation, not a guarantee: this documents the amplification that
    // caused S5's 119,781ms stalls. tonic serialises requests on a channel, so
    // the worst case is queue_depth x connect_timeout. If a future change makes
    // queued requests fail fast instead, this assertion will fail — update it,
    // that is an improvement.
    assert!(
        slowest >= connect_timeout * (CONCURRENCY as u32 - 1),
        "expected queued requests to serialise behind each other's connect \
         attempts (~{CONCURRENCY}x{connect_timeout:?}), slowest was only {slowest:?}"
    );
}
