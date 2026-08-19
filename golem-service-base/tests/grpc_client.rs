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
