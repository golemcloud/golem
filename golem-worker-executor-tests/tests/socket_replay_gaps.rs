// Reproductions of the durable sockets gaps (SK-01, GOL-1174). Each test asserts the target
// behaviour of the durable sockets plan and fails on the executor of today. They are not merged
// into `main`; the ticket that fixes a gap moves its test into `socket_replay.rs`.

use super::*;
use golem_common::model::IdempotencyKey;
use pretty_assertions::assert_eq;
use test_r::{test, timeout};

/// Whether the public oplog shows `bytes`, as text or as a byte list.
async fn oplog_shows(session: &Session<'_>, bytes: &[u8]) -> anyhow::Result<bool> {
    let rendered = format!("{:?}", session.oplog().await?);
    Ok(rendered.contains(std::str::from_utf8(bytes)?) || rendered.contains(&format!("{bytes:?}")))
}

/// G1 (SK-06, GOL-1179). A send that was open at the crash is replayed by draining its recorded
/// bytes. The guest's next write after the agent is live must fail with `connection-broken`
/// instead of being accepted and dropped.
#[test]
#[timeout("3m")]
async fn g1_replayed_send_refuses_live_writes(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let peer = TcpPeer::start(b"", Reply::Silent).await?;
    let session = Session::start(deps, &context, host_api_tests, sockets(), "g1").await?;
    let _: Result<(), String> = session
        .call("p3_tcp_connect", data_value!("a".to_string(), peer.port))
        .await?;
    let key = IdempotencyKey::fresh();
    let call = {
        let executor = session.executor().clone();
        let component = session.component.clone();
        let agent_id = session.agent_id.clone();
        let key = key.clone();
        tokio::spawn(async move {
            executor
                .invoke_and_await_agent_with_key(
                    &component,
                    &agent_id,
                    &key,
                    "p3_tcp_send_with_pause",
                    data_value!(
                        "a".to_string(),
                        b"first".to_vec(),
                        3000u64,
                        b"second".to_vec()
                    ),
                )
                .await
        })
    };
    peer.wait_until("the first part arrives", |connections| {
        connections.len() == 1 && connections[0].received == b"first"
    })
    .await?;
    session
        .executor()
        .simulated_crash(&session.worker_id)
        .await?;
    let _ = call.await?;
    let result: Result<(), String> = session
        .executor()
        .invoke_and_await_agent_with_key(
            &session.component,
            &session.agent_id,
            &key,
            "p3_tcp_send_with_pause",
            data_value!(
                "a".to_string(),
                b"first".to_vec(),
                3000u64,
                b"second".to_vec()
            ),
        )
        .await?
        .into_typed()?;
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(peer.connections().len(), 1);
    assert_eq!(peer.connections()[0].received, b"first".to_vec());
    assert!(
        result
            .as_ref()
            .is_err_and(|error| error.contains("were not accepted")),
        "the live write after replay must not be accepted: {result:?}"
    );
    Ok(())
}

/// G2 (SK-05, GOL-1178). A send stream acquired after replay on a replayed connection must fail
/// with `connection-broken`, not `invalid-state`.
#[test]
#[timeout("3m")]
async fn g2_send_on_a_replayed_connection_reports_connection_broken(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let peer = TcpPeer::start(b"", Reply::Silent).await?;
    let mut session = Session::start(deps, &context, host_api_tests, sockets(), "g2").await?;
    let _: Result<(), String> = session
        .call("p3_tcp_connect", data_value!("a".to_string(), peer.port))
        .await?;
    session.restart_executor().await?;
    let sent: Result<(), String> = session
        .call("p3_tcp_send", data_value!("a".to_string(), b"x".to_vec()))
        .await?;
    assert_eq!(sent, Err("ErrorCode::ConnectionBroken".to_string()));
    Ok(())
}

/// G3 (SK-05, GOL-1178). The getters of a socket return the same addresses before and after
/// replay.
#[test]
#[timeout("3m")]
async fn g3_getters_are_equal_before_and_after_replay(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let peer = TcpPeer::start(b"", Reply::Silent).await?;
    let mut session = Session::start(deps, &context, host_api_tests, sockets(), "g3").await?;
    let _: Result<(), String> = session
        .call("p3_tcp_connect", data_value!("a".to_string(), peer.port))
        .await?;
    let before: Result<(String, String), String> = session
        .call("p3_tcp_addresses", data_value!("a".to_string()))
        .await?;
    assert!(before.is_ok(), "{before:?}");
    session.restart_executor().await?;
    let after: Result<(String, String), String> = session
        .call("p3_tcp_addresses", data_value!("a".to_string()))
        .await?;
    assert_eq!(after, before);
    Ok(())
}

/// G4 (SK-06, GOL-1179). A send stream kept open across invocations must not hold each
/// invocation until the tail-work deadline.
#[test]
#[timeout("5m")]
async fn g4_send_kept_across_invocations_does_not_hold_invocations(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let peer = TcpPeer::start(b"", Reply::Silent).await?;
    let session = Session::start(deps, &context, host_api_tests, sockets(), "g4").await?;
    let _: Result<(), String> = session
        .call("p3_tcp_connect", data_value!("a".to_string(), peer.port))
        .await?;
    let started = std::time::Instant::now();
    let opened = tokio::time::timeout(
        Duration::from_secs(20),
        session.call::<Result<(), String>>("p3_tcp_open_send", data_value!("a".to_string())),
    )
    .await;
    let opened_after = started.elapsed();
    let written = tokio::time::timeout(
        Duration::from_secs(20),
        session.call::<Result<u32, String>>(
            "p3_tcp_write",
            data_value!("a".to_string(), b"one".to_vec()),
        ),
    )
    .await;
    let written_after = started.elapsed();
    assert!(
        matches!(&opened, Ok(Ok(Ok(())))),
        "open_send: {opened:?} after {opened_after:?}"
    );
    assert!(
        matches!(&written, Ok(Ok(Ok(3)))),
        "write: {written:?} after {written_after:?}"
    );
    peer.wait_until("the bytes arrive", |connections| {
        connections.len() == 1 && connections[0].received == b"one"
    })
    .await?;
    Ok(())
}

/// G5 (SK-07, GOL-1180). A connected P3 UDP socket stays connected after replay, on the same
/// local address, and its send stores no bytes.
#[test]
#[timeout("3m")]
async fn g5_udp_connect_survives_replay_and_send_stores_no_bytes(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let peer = UdpPeer::start().await?;
    let mut session = Session::start(deps, &context, host_api_tests, sockets(), "g5").await?;
    let local = free_udp_address().await?;
    let bound: Result<(), String> = session
        .call("p3_udp_bind", data_value!("u".to_string(), local.port()))
        .await?;
    assert_eq!(bound, Ok(()));
    let connected: Result<(), String> = session
        .call("p3_udp_connect", data_value!("u".to_string(), peer.port()))
        .await?;
    assert_eq!(connected, Ok(()));
    let sent: Result<(), String> = session
        .call(
            "p3_udp_send",
            data_value!("u".to_string(), None::<u16>, b"udp-5c1e".to_vec()),
        )
        .await?;
    assert_eq!(sent, Ok(()));
    assert_eq!(peer.receive().await?, (b"udp-5c1e".to_vec(), local));
    let stores = oplog_shows(&session, b"udp-5c1e").await?;

    session.restart_executor().await?;
    let local_after: Result<String, String> = session
        .call("p3_udp_local_address", data_value!("u".to_string()))
        .await?;
    let sent: Result<(), String> = session
        .call(
            "p3_udp_send",
            data_value!("u".to_string(), None::<u16>, b"after".to_vec()),
        )
        .await?;
    let arrived = peer.received_soon().await;
    assert_eq!(
        (local_after, sent, arrived, stores),
        (
            Ok(local.to_string()),
            Ok(()),
            vec![b"after".to_vec()],
            false
        )
    );
    Ok(())
}

/// S10 on P2 (SK-09, GOL-1182). A read-only method must not connect a P2 TCP socket. Today the
/// P2 connect records only a `ReadLocal` permission check, which the read-only guard allows.
#[test]
#[timeout("3m")]
async fn s10_p2_connect_in_a_read_only_method_is_refused(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let peer = TcpPeer::start(b"", Reply::Silent).await?;
    let session = Session::start(deps, &context, host_api_tests, sockets(), "s10-p2").await?;
    let result = session
        .executor()
        .invoke_and_await_agent(
            &session.component,
            &session.agent_id,
            "p2_tcp_connect_read_only",
            data_value!(peer.port),
        )
        .await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(
        (peer.connections().len(), result.is_err()),
        (0, true),
        "P2 read-only connect: {:?}",
        result.map(|value| format!("{value:?}"))
    );
    Ok(())
}

/// S10 on P3, the control for the P2 case: the P3 connect is `WriteRemote`, so the read-only
/// guard refuses it before it connects.
#[test]
#[timeout("3m")]
async fn s10_p3_connect_in_a_read_only_method_is_refused(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let peer = TcpPeer::start(b"", Reply::Silent).await?;
    let session = Session::start(deps, &context, host_api_tests, sockets(), "s10-p3").await?;
    let result = session
        .executor()
        .invoke_and_await_agent(
            &session.component,
            &session.agent_id,
            "p3_tcp_connect_read_only",
            data_value!(peer.port),
        )
        .await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(
        (peer.connections().len(), result.is_err()),
        (0, true),
        "P3 read-only connect: {:?}",
        result.map(|value| format!("{value:?}"))
    );
    Ok(())
}

/// Live P3 TCP send (SK-06, GOL-1179). Today the live send delivers no bytes and the guest gets
/// `connection-broken`; the target is that each sent byte reaches the peer once, a completed
/// conversation is not repeated by replay, and the sent bytes are not in the oplog.
#[test]
#[timeout("3m")]
async fn p3_tcp_completed_conversation_replays_without_contacting_the_peer(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let peer = TcpPeer::start(b"", Reply::Fixed("pong 1")).await?;
    let mut session = Session::start(
        deps,
        &context,
        host_api_tests,
        sockets(),
        "p3-tcp-conversation",
    )
    .await?;

    let response: Result<Vec<u8>, String> = session
        .call(
            "p3_tcp_conversation",
            data_value!(peer.port, b"ping-7f3a\n".to_vec()),
        )
        .await?;
    assert_eq!(response, Ok(b"pong 1\n".to_vec()));
    peer.wait_until("the agent closes its connection", |connections| {
        connections.iter().all(|connection| connection.closed)
    })
    .await?;
    assert!(
        !oplog_shows(&session, b"ping-7f3a").await?,
        "a P3 TCP send records no bytes"
    );

    session.restart_executor().await?;
    let log: Vec<(String, Vec<u8>)> = session.call("received", data_value!()).await?;
    assert_eq!(
        log,
        vec![received("conversation", b"pong 1\n")],
        "replay returns the recorded bytes"
    );
    assert_eq!(
        peer.connections(),
        vec![Connection {
            received: b"ping-7f3a\n".to_vec(),
            closed: true,
        }],
        "replay neither connects nor sends again"
    );

    // The next conversation is live and opens the second connection.
    let response: Result<Vec<u8>, String> = session
        .call(
            "p3_tcp_conversation",
            data_value!(peer.port, b"again\n".to_vec()),
        )
        .await?;
    assert_eq!(response, Ok(b"pong 1\n".to_vec()));
    assert_eq!(peer.connections().len(), 2);
    assert_eq!(
        session
            .starts("sockets::types::tcp-socket::connect")
            .await?,
        2
    );
    session.finish().await
}
