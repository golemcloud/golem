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

//! Crash-and-replay baseline of agent sockets. An agent of the `SocketSession` type keeps P2 and
//! P3 sockets in named slots across invocations and talks to TCP and UDP peers in the test. The
//! peers count connections, bytes and datagrams, so each test checks what replay repeats, returns
//! from the oplog, or loses. Every test reconstructs the agent for real, with a simulated crash of a
//! running agent or with an executor restart over the same storage.
//!
//! These tests record the behaviour of the executor as it is. Where it is not yet the target of
//! the durable sockets plan, the test says so and names the ticket that changes it.

use crate::Tracing;
use golem_common::model::agent::{ParsedAgentKey, UpdateRecord};
use golem_common::model::component::ComponentDto;
use golem_common::model::oplog::PublicOplogEntry;
use golem_common::model::{AgentId, OplogIndex};
use golem_common::schema::{FromSchema, TypedSchemaValue};
use golem_common::{agent_id, data_value};
use golem_test_framework::dsl::TestDsl;
use golem_worker_executor::services::golem_config::SnapshotPolicy;
use golem_worker_executor_test_utils::{
    LastUniqueId, PrecompiledComponent, TestContext, TestExecutorOverrides, TestWorkerExecutor,
    WorkerExecutorTestDependencies, start_with_overrides,
};
use pretty_assertions::assert_eq;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use test_r::{inherit_test_dep, test, timeout};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, UdpSocket};
use tokio::sync::Notify;
use tokio_util::task::AbortOnDropHandle;

inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(LastUniqueId);
inherit_test_dep!(
    #[tagged_as("host_api_tests")]
    PrecompiledComponent
);
inherit_test_dep!(Tracing);

const AGENT_TYPE: &str = "SocketSession";
const WAIT: Duration = Duration::from_secs(30);

/// One accepted connection of a [`TcpPeer`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Connection {
    received: Vec<u8>,
    closed: bool,
}

#[derive(Default)]
struct PeerLog {
    connections: Vec<Connection>,
}

/// How a [`TcpPeer`] answers the first line it reads on a connection.
#[derive(Clone, Copy, Debug)]
enum Reply {
    Silent,
    /// `"{text}\n"`, the same on every connection.
    Fixed(&'static str),
}

/// A TCP peer on `127.0.0.1`. On each connection it first writes `greeting`, answers the first
/// line it reads as `reply` says and then closes its write side. It keeps every byte it reads
/// per connection, and notes when the agent closes a connection.
struct TcpPeer {
    port: u16,
    log: Arc<Mutex<PeerLog>>,
    changed: Arc<Notify>,
    _task: AbortOnDropHandle<()>,
}

impl TcpPeer {
    async fn start(greeting: &'static [u8], reply: Reply) -> anyhow::Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let port = listener.local_addr()?.port();
        let log = Arc::new(Mutex::new(PeerLog::default()));
        let changed = Arc::new(Notify::new());
        let task = {
            let log = log.clone();
            let changed = changed.clone();
            tokio::spawn(async move {
                while let Ok((mut stream, _)) = listener.accept().await {
                    let index = {
                        let mut log = log.lock().unwrap();
                        log.connections.push(Connection::default());
                        log.connections.len() - 1
                    };
                    changed.notify_waiters();
                    let log = log.clone();
                    let changed = changed.clone();
                    tokio::spawn(async move {
                        let _ = stream.write_all(greeting).await;
                        let mut replied = false;
                        let mut buffer = [0u8; 1024];
                        loop {
                            match stream.read(&mut buffer).await {
                                Ok(0) | Err(_) => break,
                                Ok(n) => {
                                    let has_line = {
                                        let mut log = log.lock().unwrap();
                                        let received = &mut log.connections[index].received;
                                        received.extend_from_slice(&buffer[..n]);
                                        received.contains(&b'\n')
                                    };
                                    changed.notify_waiters();
                                    let answer = match reply {
                                        Reply::Silent => None,
                                        Reply::Fixed(text) => Some(format!("{text}\n")),
                                    };
                                    if let Some(answer) = answer
                                        && has_line
                                        && !replied
                                    {
                                        replied = true;
                                        let _ = stream.write_all(answer.as_bytes()).await;
                                        let _ = stream.shutdown().await;
                                    }
                                }
                            }
                        }
                        log.lock().unwrap().connections[index].closed = true;
                        changed.notify_waiters();
                    });
                }
            })
        };
        Ok(Self {
            port,
            log,
            changed,
            _task: AbortOnDropHandle::new(task),
        })
    }

    fn connections(&self) -> Vec<Connection> {
        self.log.lock().unwrap().connections.clone()
    }

    /// Waits until `condition` holds for the connections of the peer.
    async fn wait_until(
        &self,
        what: &str,
        condition: impl Fn(&[Connection]) -> bool,
    ) -> anyhow::Result<()> {
        tokio::time::timeout(WAIT, async {
            loop {
                let notified = self.changed.notified();
                if condition(&self.log.lock().unwrap().connections) {
                    return;
                }
                notified.await;
            }
        })
        .await
        .map_err(|_| anyhow::anyhow!("{what}: peer connections {:?}", self.connections()))
    }
}

/// A UDP peer on `127.0.0.1`.
struct UdpPeer {
    socket: UdpSocket,
}

impl UdpPeer {
    async fn start() -> anyhow::Result<Self> {
        Ok(Self {
            socket: UdpSocket::bind("127.0.0.1:0").await?,
        })
    }

    fn port(&self) -> u16 {
        self.socket.local_addr().unwrap().port()
    }

    async fn receive(&self) -> anyhow::Result<(Vec<u8>, SocketAddr)> {
        let mut buffer = [0u8; 1024];
        let (len, source) = tokio::time::timeout(WAIT, self.socket.recv_from(&mut buffer))
            .await
            .map_err(|_| anyhow::anyhow!("the UDP peer received no datagram"))??;
        Ok((buffer[..len].to_vec(), source))
    }

    /// The datagrams that arrive within a short time. Used to show that none arrives.
    async fn received_soon(&self) -> Vec<Vec<u8>> {
        let mut datagrams = Vec::new();
        let mut buffer = [0u8; 1024];
        while let Ok(Ok((len, _))) = tokio::time::timeout(
            Duration::from_millis(500),
            self.socket.recv_from(&mut buffer),
        )
        .await
        {
            datagrams.push(buffer[..len].to_vec());
        }
        datagrams
    }
}

struct Session<'a> {
    deps: &'a WorkerExecutorTestDependencies,
    context: &'a TestContext,
    overrides: TestExecutorOverrides,
    executor: Option<TestWorkerExecutor>,
    component: ComponentDto,
    agent_id: ParsedAgentKey,
    worker_id: AgentId,
}

fn sockets() -> TestExecutorOverrides {
    TestExecutorOverrides::agent_sockets_enabled()
}

/// Sockets on, and a snapshot after every invocation.
fn sockets_and_snapshots() -> TestExecutorOverrides {
    TestExecutorOverrides {
        configure: Some(Arc::new(|config| {
            config.oplog.default_snapshotting = SnapshotPolicy::EveryNInvocation { count: 1 };
        })),
        ..TestExecutorOverrides::default()
    }
    .with_agent_sockets()
}

impl<'a> Session<'a> {
    async fn start(
        deps: &'a WorkerExecutorTestDependencies,
        context: &'a TestContext,
        component: &PrecompiledComponent,
        overrides: TestExecutorOverrides,
        name: &str,
    ) -> anyhow::Result<Self> {
        let executor = start_with_overrides(deps, context, overrides.clone()).await?;
        let component = executor
            .component_dep(&context.default_environment_id, component)
            .store()
            .await?;
        let agent_id = agent_id!(AGENT_TYPE, name);
        let worker_id = executor
            .start_agent(&component.id, agent_id.clone())
            .await?;
        Ok(Self {
            deps,
            context,
            overrides,
            executor: Some(executor),
            component,
            agent_id,
            worker_id,
        })
    }

    fn executor(&self) -> &TestWorkerExecutor {
        self.executor.as_ref().expect("executor is running")
    }

    async fn call<T: FromSchema>(
        &self,
        method: &str,
        params: TypedSchemaValue,
    ) -> anyhow::Result<T> {
        self.executor()
            .invoke_and_await_agent(&self.component, &self.agent_id, method, params)
            .await?
            .into_typed::<T>()
    }

    /// Drops the executor and starts a new one over the same storage, so the next invocation
    /// rebuilds the agent from its oplog. A simulated crash would not do for an idle agent: it
    /// restarts only a running one.
    async fn restart_executor(&mut self) -> anyhow::Result<()> {
        drop(self.executor.take());
        self.executor =
            Some(start_with_overrides(self.deps, self.context, self.overrides.clone()).await?);
        Ok(())
    }

    async fn oplog(&self) -> anyhow::Result<Vec<PublicOplogEntry>> {
        Ok(self
            .executor()
            .get_oplog(&self.worker_id, OplogIndex::INITIAL)
            .await?
            .into_iter()
            .map(|entry| entry.entry)
            .collect())
    }

    async fn starts(&self, function_name: &str) -> anyhow::Result<usize> {
        Ok(self
            .oplog()
            .await?
            .iter()
            .filter(|entry| {
                matches!(entry, PublicOplogEntry::Start(start) if start.function_name == function_name)
            })
            .count())
    }

    async fn snapshots(&self) -> anyhow::Result<usize> {
        Ok(self
            .oplog()
            .await?
            .iter()
            .filter(|entry| matches!(entry, PublicOplogEntry::Snapshot(_)))
            .count())
    }

    /// Waits until the oplog holds more than `count` snapshots. The invocation loop takes the
    /// snapshot after it returned the result of the invocation.
    async fn wait_for_snapshot_after(&self, count: usize) -> anyhow::Result<usize> {
        tokio::time::timeout(WAIT, async {
            loop {
                let snapshots = self.snapshots().await?;
                if snapshots > count {
                    return Ok::<_, anyhow::Error>(snapshots);
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await
        .map_err(|_| anyhow::anyhow!("no snapshot after {count}"))?
    }

    /// Waits for the outcome of the latest update attempt: `Ok` with nothing for a successful
    /// update, `Err` with the details of a failed one.
    async fn update_outcome(&self, attempts_before: usize) -> anyhow::Result<Result<(), String>> {
        tokio::time::timeout(WAIT, async {
            loop {
                let metadata = self.executor().get_worker_metadata(&self.worker_id).await?;
                let outcomes = metadata
                    .updates
                    .iter()
                    .filter_map(|update| match update {
                        UpdateRecord::SuccessfulUpdate(_) => Some(Ok(())),
                        UpdateRecord::FailedUpdate(update) => {
                            Some(Err(update.details.clone().unwrap_or_default()))
                        }
                        UpdateRecord::PendingUpdate(_) => None,
                    })
                    .collect::<Vec<_>>();
                if outcomes.len() > attempts_before {
                    return Ok::<_, anyhow::Error>(outcomes[attempts_before].clone());
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await
        .map_err(|_| anyhow::anyhow!("the update attempt never finished"))?
    }

    async fn finish(self) -> anyhow::Result<()> {
        self.executor()
            .check_oplog_is_queryable(&self.worker_id)
            .await
    }
}

/// A `127.0.0.1` UDP address that was free a moment ago.
async fn free_udp_address() -> anyhow::Result<SocketAddr> {
    Ok(UdpSocket::bind("127.0.0.1:0").await?.local_addr()?)
}

fn received(slot: &str, data: &[u8]) -> (String, Vec<u8>) {
    (slot.to_string(), data.to_vec())
}

/// A P3 TCP socket kept across invocations: after a reconstruction, replay returns the bytes the
/// earlier invocations received without reconnecting, and the next live read on that connection
/// fails with `connection-broken`. The native connection from before the crash is closed.
#[test]
#[timeout("3m")]
async fn p3_tcp_socket_kept_across_invocations_is_lost_after_replay(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let peer = TcpPeer::start(b"abcdef", Reply::Silent).await?;
    let mut session =
        Session::start(deps, &context, host_api_tests, sockets(), "p3-tcp-kept").await?;

    let _: Result<(), String> = session
        .call("p3_tcp_connect", data_value!("a".to_string(), peer.port))
        .await?;
    let first: Result<Vec<u8>, String> = session
        .call("p3_tcp_receive", data_value!("a".to_string(), 3u32))
        .await?;
    assert_eq!(first, Ok(b"abc".to_vec()));
    let second: Result<Vec<u8>, String> = session
        .call("p3_tcp_receive", data_value!("a".to_string(), 3u32))
        .await?;
    assert_eq!(second, Ok(b"def".to_vec()));
    assert_eq!(peer.connections().len(), 1);

    session.restart_executor().await?;
    let log: Vec<(String, Vec<u8>)> = session.call("received", data_value!()).await?;
    assert_eq!(
        log,
        vec![received("a", b"abc"), received("a", b"def")],
        "replay returns the recorded chunks"
    );
    peer.wait_until(
        "the connection from before the crash closes",
        |connections| connections.len() == 1 && connections[0].closed,
    )
    .await?;

    let after: Result<Vec<u8>, String> = session
        .call("p3_tcp_receive", data_value!("a".to_string(), 1u32))
        .await?;
    assert_eq!(
        after,
        Err("ErrorCode::ConnectionBroken".to_string()),
        "the next live read on the old connection reports the loss"
    );
    assert_eq!(
        peer.connections().len(),
        1,
        "Golem never recreates the connection"
    );
    assert_eq!(
        session
            .starts("sockets::types::tcp-socket::connect")
            .await?,
        1
    );
    session.finish().await
}

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
    let connected: Result<(), String> = session
        .call("p3_tcp_connect", data_value!("a".to_string(), peer.port))
        .await?;
    assert_eq!(connected, Ok(()));
    let duplicate: Result<(), String> = session
        .call("p3_tcp_receive_again", data_value!("a".to_string()))
        .await?;
    assert_eq!(duplicate, Err("ErrorCode::InvalidState".to_string()));
    session.restart_executor().await?;
    let sent: Result<(), String> = session
        .call("p3_tcp_send", data_value!("a".to_string(), b"x".to_vec()))
        .await?;
    assert_eq!(
        sent,
        Err("1 bytes were not accepted: Err(\"ErrorCode::ConnectionBroken\")".to_string())
    );
    let received: Result<(), String> = session
        .call("p3_tcp_receive_again", data_value!("a".to_string()))
        .await?;
    assert_eq!(received, Err("ErrorCode::ConnectionBroken".to_string()));
    assert_eq!(peer.connections().len(), 1);
    assert!(peer.connections()[0].received.is_empty());
    session.finish().await
}

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
    let connected: Result<(), String> = session
        .call("p3_tcp_connect", data_value!("a".to_string(), peer.port))
        .await?;
    assert_eq!(connected, Ok(()));
    let before: Result<(String, String), String> = session
        .call("p3_tcp_addresses", data_value!("a".to_string()))
        .await?;
    assert!(before.is_ok(), "{before:?}");
    session.restart_executor().await?;
    let after: Result<(String, String), String> = session
        .call("p3_tcp_addresses", data_value!("a".to_string()))
        .await?;
    assert_eq!(after, before);
    assert_eq!(peer.connections().len(), 1);
    session.finish().await
}

#[test]
#[timeout("3m")]
async fn p3_tcp_bind_and_configured_options_replay_without_native_bind(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let mut session = Session::start(deps, &context, host_api_tests, sockets(), "tcp-bind").await?;
    let bound: Result<(), String> = session
        .call("p3_tcp_bind", data_value!("a".to_string(), 0u16))
        .await?;
    assert_eq!(bound, Ok(()));
    let before: Result<(String, Vec<u64>), String> = session
        .call("p3_tcp_bound_state", data_value!("a".to_string()))
        .await?;
    let (address, options) = before
        .as_ref()
        .map_err(|error| anyhow::anyhow!(error.clone()))?;
    let address: SocketAddr = address.parse()?;
    assert!(address.ip().is_loopback());
    assert_ne!(address.port(), 0);
    assert_eq!(&options[..5], &[1, 3_000_000_000, 4_000_000_000, 7, 42]);
    for expected in [51u8, 42] {
        let changed: Result<u8, String> = session
            .call(
                "p3_tcp_hop_limit",
                data_value!("a".to_string(), Some(expected)),
            )
            .await?;
        assert_eq!(changed, Ok(expected));
        let cached: Result<u8, String> = session
            .call("p3_tcp_hop_limit", data_value!("a".to_string(), None::<u8>))
            .await?;
        assert_eq!(cached, Ok(expected));
    }
    session.restart_executor().await?;
    // Occupy the recorded endpoint before reconstruction. Replaying bind must not touch it.
    let _occupied = tokio::time::timeout(WAIT, async {
        loop {
            match TcpListener::bind(address).await {
                Ok(listener) => break Ok::<_, std::io::Error>(listener),
                Err(error) if error.kind() == std::io::ErrorKind::AddrInUse => {
                    tokio::task::yield_now().await
                }
                Err(error) => break Err(error),
            }
        }
    })
    .await??;
    let after: Result<(String, Vec<u64>), String> = session
        .call("p3_tcp_bound_state", data_value!("a".to_string()))
        .await?;
    assert_eq!(after, before);
    let listen: Result<(), String> = session
        .call("p3_tcp_listen", data_value!("a".to_string()))
        .await?;
    assert_eq!(listen, Err("ErrorCode::ConnectionBroken".to_string()));
    let after_listen: Result<(String, Vec<u64>), String> = session
        .call("p3_tcp_bound_state", data_value!("a".to_string()))
        .await?;
    assert_eq!(after_listen, before);
    let changed: Result<u8, String> = session
        .call("p3_tcp_hop_limit", data_value!("a".to_string(), Some(63u8)))
        .await?;
    assert_eq!(changed, Ok(63));
    assert_eq!(session.starts("sockets::types::tcp-socket::bind").await?, 1);
    let peer = TcpPeer::start(b"", Reply::Silent).await?;
    let connect: Result<(), String> = session
        .call(
            "p3_tcp_connect_existing",
            data_value!("a".to_string(), peer.port),
        )
        .await?;
    assert_eq!(connect, Err("ErrorCode::ConnectionBroken".to_string()));
    assert!(peer.connections().is_empty());
    session.finish().await
}

#[test]
#[timeout("3m")]
async fn p3_failed_tcp_connect_preserves_closed_state_after_replay(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let mut session =
        Session::start(deps, &context, host_api_tests, sockets(), "tcp-refused").await?;
    let bound: Result<(), String> = session
        .call("p3_tcp_bind", data_value!("a".to_string(), 0u16))
        .await?;
    assert_eq!(bound, Ok(()));
    let unused = TcpListener::bind("127.0.0.1:0").await?;
    let port = unused.local_addr()?.port();
    drop(unused);
    let result: Result<(), String> = session
        .call(
            "p3_tcp_connect_existing",
            data_value!("a".to_string(), port),
        )
        .await?;
    assert_eq!(result, Err("ErrorCode::ConnectionRefused".to_string()));
    let before: Result<(String, String), String> = session
        .call("p3_tcp_addresses", data_value!("a".to_string()))
        .await?;
    assert_eq!(before, Err("ErrorCode::ConnectionRefused".to_string()));
    for value in [None, Some(51u8)] {
        let option: Result<u8, String> = session
            .call("p3_tcp_hop_limit", data_value!("a".to_string(), value))
            .await?;
        assert_eq!(option, Err("ErrorCode::ConnectionRefused".to_string()));
    }
    session.restart_executor().await?;
    let after: Result<(String, String), String> = session
        .call("p3_tcp_addresses", data_value!("a".to_string()))
        .await?;
    assert_eq!(after, before);
    for value in [None, Some(63u8)] {
        let option: Result<u8, String> = session
            .call("p3_tcp_hop_limit", data_value!("a".to_string(), value))
            .await?;
        assert_eq!(option, Err("ErrorCode::ConnectionRefused".to_string()));
    }
    let receive: Result<(), String> = session
        .call("p3_tcp_receive_again", data_value!("a".to_string()))
        .await?;
    assert_eq!(receive, Err("ErrorCode::ConnectionRefused".to_string()));
    assert_eq!(
        session
            .starts("sockets::types::tcp-socket::connect")
            .await?,
        1
    );
    let peer = TcpPeer::start(b"", Reply::Silent).await?;
    let reconnect: Result<(), String> = session
        .call(
            "p3_tcp_connect_existing",
            data_value!("a".to_string(), peer.port),
        )
        .await?;
    assert_eq!(reconnect, Err("ErrorCode::InvalidState".to_string()));
    assert!(peer.connections().is_empty());
    session.finish().await
}

/// A P3 TCP receive that waits on a silent peer when the agent crashes: recovery replays the
/// completed receives from the oplog and completes the waiting one with `connection-broken`
/// under its original `Start`, without a new connection.
#[test]
#[timeout("3m")]
async fn p3_tcp_receive_in_flight_at_a_crash_completes_with_connection_broken(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let peer = TcpPeer::start(b"abc", Reply::Silent).await?;
    let session = Session::start(
        deps,
        &context,
        host_api_tests,
        sockets(),
        "p3-tcp-in-flight",
    )
    .await?;
    let _: Result<(), String> = session
        .call("p3_tcp_connect", data_value!("a".to_string(), peer.port))
        .await?;
    let first: Result<Vec<u8>, String> = session
        .call("p3_tcp_receive", data_value!("a".to_string(), 3u32))
        .await?;
    assert_eq!(first, Ok(b"abc".to_vec()));
    let chunks_before = session
        .starts("sockets::types::tcp-socket::receive-chunk")
        .await?;

    let waiting = {
        let executor = session.executor().clone();
        let component = session.component.clone();
        let agent_id = session.agent_id.clone();
        tokio::spawn(async move {
            executor
                .invoke_and_await_agent(
                    &component,
                    &agent_id,
                    "p3_tcp_receive",
                    data_value!("a".to_string(), 1u32),
                )
                .await
        })
    };
    // The waiting read has its own chunk Start once the guest asked for more bytes.
    tokio::time::timeout(WAIT, async {
        loop {
            session.executor().commit_oplog(&session.worker_id).await?;
            if session
                .starts("sockets::types::tcp-socket::receive-chunk")
                .await?
                > chunks_before
                && session
                    .executor()
                    .get_worker_metadata(&session.worker_id)
                    .await?
                    .status
                    == golem_common::model::AgentStatus::Running
            {
                return Ok::<_, anyhow::Error>(());
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .map_err(|_| anyhow::anyhow!("the read never started to wait"))??;

    session
        .executor()
        .simulated_crash(&session.worker_id)
        .await?;
    let result = waiting.await??.into_typed::<Result<Vec<u8>, String>>()?;
    assert_eq!(result, Err("ErrorCode::ConnectionBroken".to_string()));
    let log: Vec<(String, Vec<u8>)> = session.call("received", data_value!()).await?;
    assert_eq!(log, vec![received("a", b"abc")]);
    peer.wait_until(
        "the connection from before the crash closes",
        |connections| connections.len() == 1 && connections[0].closed,
    )
    .await?;
    assert_eq!(peer.connections().len(), 1, "no new connection");
    assert_eq!(
        session
            .starts("sockets::types::tcp-socket::receive-chunk")
            .await?,
        chunks_before + 1,
        "the waiting read keeps its one Start"
    );
    session.finish().await
}

/// Two P3 TCP sockets whose receives interleave, starting with the socket connected second.
/// Replay hands each recorded chunk back to its own socket, in the original order.
#[test]
#[timeout("3m")]
async fn p3_tcp_interleaved_receives_replay_to_their_own_sockets(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let upper = TcpPeer::start(b"ABC", Reply::Silent).await?;
    let lower = TcpPeer::start(b"xyz", Reply::Silent).await?;
    let mut session = Session::start(
        deps,
        &context,
        host_api_tests,
        sockets(),
        "p3-tcp-interleaved",
    )
    .await?;

    let _: Result<(), String> = session
        .call(
            "p3_tcp_connect",
            data_value!("upper".to_string(), upper.port),
        )
        .await?;
    let _: Result<(), String> = session
        .call(
            "p3_tcp_connect",
            data_value!("lower".to_string(), lower.port),
        )
        .await?;
    let result: Result<(Vec<u8>, Vec<u8>), String> = session
        .call(
            "p3_tcp_receive_interleaved",
            data_value!("lower".to_string(), "upper".to_string(), 3u32),
        )
        .await?;
    assert_eq!(result, Ok((b"xyz".to_vec(), b"ABC".to_vec())));
    let expected = vec![
        received("lower", b"x"),
        received("upper", b"A"),
        received("lower", b"y"),
        received("upper", b"B"),
        received("lower", b"z"),
        received("upper", b"C"),
    ];
    let log: Vec<(String, Vec<u8>)> = session.call("received", data_value!()).await?;
    assert_eq!(log, expected);

    session.restart_executor().await?;
    let log: Vec<(String, Vec<u8>)> = session.call("received", data_value!()).await?;
    assert_eq!(log, expected);
    assert_eq!(upper.connections().len(), 1);
    assert_eq!(lower.connections().len(), 1);
    session.finish().await
}

/// A completed P3 UDP exchange (bind, send, receive) is not repeated by replay: the peer gets no
/// second datagram, and the agent gets the datagram it received from the oplog.
#[test]
#[timeout("3m")]
async fn p3_udp_completed_exchange_replays_without_contacting_the_peer(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let peer = UdpPeer::start().await?;
    let mut session =
        Session::start(deps, &context, host_api_tests, sockets(), "p3-udp-exchange").await?;

    let local = free_udp_address().await?;
    let bound: Result<(), String> = session
        .call("p3_udp_bind", data_value!("u".to_string(), local.port()))
        .await?;
    assert_eq!(bound, Ok(()));
    let sent: Result<(), String> = session
        .call(
            "p3_udp_send",
            data_value!("u".to_string(), Some(peer.port()), b"hello".to_vec()),
        )
        .await?;
    assert_eq!(sent, Ok(()));
    let (datagram, source) = peer.receive().await?;
    assert_eq!((datagram, source), (b"hello".to_vec(), local));

    let (receive, answer) = tokio::join!(
        session.call::<Result<(Vec<u8>, String), String>>(
            "p3_udp_receive",
            data_value!("u".to_string())
        ),
        // The socket is bound, so the datagram waits for the receive.
        peer.socket.send_to(b"world", local)
    );
    answer?;
    assert_eq!(
        receive?,
        Ok((b"world".to_vec(), format!("127.0.0.1:{}", peer.port())))
    );

    session.restart_executor().await?;
    let log: Vec<(String, Vec<u8>)> = session.call("received", data_value!()).await?;
    assert_eq!(log, vec![received("u", b"world")]);
    assert_eq!(
        peer.received_soon().await,
        Vec::<Vec<u8>>::new(),
        "replay sends no datagram"
    );
    assert_eq!(session.starts("sockets::types::udp-socket::send").await?, 1);
    assert_eq!(
        session
            .starts("sockets::types::udp-socket::receive")
            .await?,
        1
    );
    session.finish().await
}

/// P2 TCP records only the permission check of a connect, and the reads and writes run
/// natively also on replay. Replay therefore connects again and sends again, and the replayed
/// read gets whatever the peer answers now. This replay succeeds only because the peer answers
/// the same and the agent returns no address: a different answer or a returned local port makes
/// the replayed invocation diverge. SK-09 and SK-10 (GOL-1182, GOL-1183) replace this with
/// recorded results and a visible loss.
#[test]
#[timeout("3m")]
async fn p2_tcp_replay_connects_and_sends_again(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let peer = TcpPeer::start(b"", Reply::Fixed("pong")).await?;
    let mut session =
        Session::start(deps, &context, host_api_tests, sockets(), "p2-tcp-replay").await?;

    let _: Result<(), String> = session
        .call("p2_tcp_connect", data_value!("a".to_string(), peer.port))
        .await?;
    let sent: Result<(), String> = session
        .call(
            "p2_tcp_send",
            data_value!("a".to_string(), b"ping\n".to_vec()),
        )
        .await?;
    assert_eq!(sent, Ok(()));
    let answer: Result<Vec<u8>, String> = session
        .call("p2_tcp_receive", data_value!("a".to_string(), 5u32))
        .await?;
    assert_eq!(answer, Ok(b"pong\n".to_vec()));
    assert_eq!(peer.connections().len(), 1);

    session.restart_executor().await?;
    let log: Vec<(String, Vec<u8>)> = session.call("received", data_value!()).await?;
    assert_eq!(log, vec![received("a", b"pong\n")]);
    let connections = peer.connections();
    assert_eq!(connections.len(), 2, "replay connects again");
    assert_eq!(
        connections[1].received,
        b"ping\n".to_vec(),
        "replay sends again"
    );
    session.finish().await
}

/// An open P3 TCP receive stream holds a live durable call, so the agent never takes a snapshot
/// while it is open: the periodic snapshot is skipped and a manual update fails with "cannot take
/// a snapshot for the update". Once the socket is closed, both work again. SK-16 (GOL-2122)
/// changes this to snapshot-then-reload.
#[test]
#[timeout("3m")]
async fn an_open_p3_tcp_receive_stream_blocks_snapshots_and_manual_updates(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let peer = TcpPeer::start(b"abc", Reply::Silent).await?;
    let session = Session::start(
        deps,
        &context,
        host_api_tests,
        sockets_and_snapshots(),
        "p3-tcp-snapshot",
    )
    .await?;

    let _: Vec<(String, Vec<u8>)> = session.call("received", data_value!()).await?;
    let baseline = session.wait_for_snapshot_after(0).await?;

    let _: Result<(), String> = session
        .call("p3_tcp_connect", data_value!("a".to_string(), peer.port))
        .await?;
    let first: Result<Vec<u8>, String> = session
        .call("p3_tcp_receive", data_value!("a".to_string(), 3u32))
        .await?;
    assert_eq!(first, Ok(b"abc".to_vec()));
    // The loop decides on the snapshot of an invocation before it starts the next one.
    let _: Vec<(String, Vec<u8>)> = session.call("received", data_value!()).await?;
    let _: Vec<(String, Vec<u8>)> = session.call("received", data_value!()).await?;
    assert_eq!(
        session.snapshots().await?,
        baseline,
        "no snapshot while the receive stream is open"
    );

    let target = session
        .executor()
        .update_component(&session.component.id, "golem_it_host_api_tests_release")
        .await?;
    session
        .executor()
        .manual_update_worker(&session.worker_id, target.revision, false)
        .await?;
    let refused = session.update_outcome(0).await?;
    assert!(
        refused
            .as_ref()
            .is_err_and(|details| details.contains("cannot take a snapshot for the update")),
        "the manual update is refused while the socket is open: {refused:?}"
    );

    let closed: bool = session.call("close", data_value!("a".to_string())).await?;
    assert!(closed);
    let _: Vec<(String, Vec<u8>)> = session.call("received", data_value!()).await?;
    session.wait_for_snapshot_after(baseline).await?;
    session
        .executor()
        .manual_update_worker(&session.worker_id, target.revision, false)
        .await?;
    assert_eq!(session.update_outcome(1).await?, Ok(()));
    let log: Vec<(String, Vec<u8>)> = session.call("received", data_value!()).await?;
    assert_eq!(
        log,
        vec![received("a", b"abc")],
        "the update keeps the receive history through the save and load hooks"
    );
    session.finish().await
}

/// An open P2 TCP socket holds no durable call, so a snapshot is taken while it is open. After
/// recovery from that snapshot the receive history is back, but the socket is gone without any
/// error: the load hook of the agent cannot restore it. SK-16 (GOL-2122) decides what an open socket means for a snapshot.
#[test]
#[timeout("3m")]
async fn an_open_p2_tcp_socket_does_not_block_snapshots(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let peer = TcpPeer::start(b"abc", Reply::Silent).await?;
    let mut session = Session::start(
        deps,
        &context,
        host_api_tests,
        sockets_and_snapshots(),
        "p2-tcp-snapshot",
    )
    .await?;

    let _: Result<(), String> = session
        .call("p2_tcp_connect", data_value!("a".to_string(), peer.port))
        .await?;
    let before = session.wait_for_snapshot_after(0).await?;
    let first: Result<Vec<u8>, String> = session
        .call("p2_tcp_receive", data_value!("a".to_string(), 1u32))
        .await?;
    assert_eq!(first, Ok(b"a".to_vec()));
    // A snapshot is taken while the P2 socket is open.
    session.wait_for_snapshot_after(before).await?;

    session.restart_executor().await?;
    let log: Vec<(String, Vec<u8>)> = session.call("received", data_value!()).await?;
    assert_eq!(
        log,
        vec![received("a", b"a")],
        "recovery from the snapshot keeps the receive history"
    );
    let after: Result<Vec<u8>, String> = session
        .call("p2_tcp_receive", data_value!("a".to_string(), 1u32))
        .await?;
    assert_eq!(after, Err("slot a is empty".to_string()));
    assert_eq!(
        peer.connections().len(),
        1,
        "recovery from the snapshot does not connect again"
    );
    session.finish().await
}

/// Whether the public oplog shows `bytes`, as text or as a byte list.
async fn oplog_shows(session: &Session<'_>, bytes: &[u8]) -> anyhow::Result<bool> {
    let rendered = format!("{:?}", session.oplog().await?);
    Ok(rendered.contains(std::str::from_utf8(bytes)?) || rendered.contains(&format!("{bytes:?}")))
}

/// A connected P3 UDP socket stays connected after replay, on the same local address, and its
/// send stores no bytes.
#[test]
#[timeout("3m")]
async fn p3_udp_connect_survives_replay_and_send_stores_no_bytes(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let peer = UdpPeer::start().await?;
    let mut session = Session::start(deps, &context, host_api_tests, sockets(), "udp-conn").await?;
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

    session.restart_executor().await?;
    let local_after: Result<String, String> = session
        .call("p3_udp_local_address", data_value!("u".to_string()))
        .await?;
    let remote_after: Result<String, String> = session
        .call("p3_udp_remote_address", data_value!("u".to_string()))
        .await?;
    let sent: Result<(), String> = session
        .call(
            "p3_udp_send",
            data_value!("u".to_string(), None::<u16>, b"after".to_vec()),
        )
        .await?;
    assert_eq!(
        (local_after, remote_after, sent),
        (
            Ok(local.to_string()),
            Ok(format!("127.0.0.1:{}", peer.port())),
            Ok(())
        )
    );
    assert_eq!(peer.receive().await?, (b"after".to_vec(), local));
    assert!(!oplog_shows(&session, b"udp-5c1e").await?);
    assert!(!oplog_shows(&session, b"after").await?);
    session.finish().await
}

/// A bind to port 0 records the port the OS chose, so replay returns the same address and the
/// rebuilt socket sends from it.
#[test]
#[timeout("3m")]
async fn p3_udp_ephemeral_bind_keeps_its_port_after_replay(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let peer = UdpPeer::start().await?;
    let mut session = Session::start(deps, &context, host_api_tests, sockets(), "udp-eph").await?;
    let bound: Result<(), String> = session
        .call("p3_udp_bind", data_value!("u".to_string(), 0u16))
        .await?;
    assert_eq!(bound, Ok(()));
    let local: String = session
        .call::<Result<String, String>>("p3_udp_local_address", data_value!("u".to_string()))
        .await?
        .map_err(anyhow::Error::msg)?;
    assert!(!local.ends_with(":0"), "{local}");

    session.restart_executor().await?;
    let local_after: Result<String, String> = session
        .call("p3_udp_local_address", data_value!("u".to_string()))
        .await?;
    assert_eq!(local_after, Ok(local.clone()));
    let sent: Result<(), String> = session
        .call(
            "p3_udp_send",
            data_value!("u".to_string(), Some(peer.port()), b"eph".to_vec()),
        )
        .await?;
    assert_eq!(sent, Ok(()));
    let (data, source) = peer.receive().await?;
    assert_eq!((data, source.to_string()), (b"eph".to_vec(), local));
    session.finish().await
}

/// A connect on an unbound socket binds it implicitly. The recorded address comes back after
/// replay, and the connected `send(None)` reaches the peer from it.
#[test]
#[timeout("3m")]
async fn p3_udp_implicit_bind_of_a_connect_survives_replay(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let peer = UdpPeer::start().await?;
    let mut session = Session::start(deps, &context, host_api_tests, sockets(), "udp-impl").await?;
    let created: Result<(), String> = session
        .call("p3_udp_create", data_value!("u".to_string()))
        .await?;
    assert_eq!(created, Ok(()));
    let connected: Result<(), String> = session
        .call("p3_udp_connect", data_value!("u".to_string(), peer.port()))
        .await?;
    assert_eq!(connected, Ok(()));
    let local: String = session
        .call::<Result<String, String>>("p3_udp_local_address", data_value!("u".to_string()))
        .await?
        .map_err(anyhow::Error::msg)?;

    session.restart_executor().await?;
    let local_after: Result<String, String> = session
        .call("p3_udp_local_address", data_value!("u".to_string()))
        .await?;
    assert_eq!(local_after, Ok(local.clone()));
    let sent: Result<(), String> = session
        .call(
            "p3_udp_send",
            data_value!("u".to_string(), None::<u16>, b"impl".to_vec()),
        )
        .await?;
    assert_eq!(sent, Ok(()));
    let (data, source) = peer.receive().await?;
    assert_eq!((data, source.to_string()), (b"impl".to_vec(), local));
    session.finish().await
}

/// When the recorded local port is taken after the crash, the first live operation fails with
/// `connection-broken`, and so does every later one; the getters still answer from the entry.
#[test]
#[timeout("3m")]
async fn p3_udp_taken_port_after_replay_reports_connection_broken(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let peer = UdpPeer::start().await?;
    let mut session =
        Session::start(deps, &context, host_api_tests, sockets(), "udp-taken").await?;
    let local = free_udp_address().await?;
    let bound: Result<(), String> = session
        .call("p3_udp_bind", data_value!("u".to_string(), local.port()))
        .await?;
    assert_eq!(bound, Ok(()));

    // The dropped executor releases the agent's native socket asynchronously; take the port as
    // soon as it is free.
    drop(session.executor.take());
    let _squatter = tokio::time::timeout(WAIT, async {
        loop {
            match UdpSocket::bind(local).await {
                Ok(socket) => break socket,
                Err(error) if error.kind() == std::io::ErrorKind::AddrInUse => {
                    tokio::time::sleep(Duration::from_millis(50)).await
                }
                Err(error) => panic!("binding the recorded port failed: {error}"),
            }
        }
    })
    .await
    .map_err(|_| anyhow::anyhow!("the old executor never released {local}"))?;
    session.restart_executor().await?;
    let sent: Result<(), String> = session
        .call(
            "p3_udp_send",
            data_value!("u".to_string(), Some(peer.port()), b"lost".to_vec()),
        )
        .await?;
    let sent_again: Result<(), String> = session
        .call(
            "p3_udp_send",
            data_value!("u".to_string(), Some(peer.port()), b"lost".to_vec()),
        )
        .await?;
    let local_after: Result<String, String> = session
        .call("p3_udp_local_address", data_value!("u".to_string()))
        .await?;
    assert_eq!(
        (sent, sent_again, local_after),
        (
            Err("ErrorCode::ConnectionBroken".to_string()),
            Err("ErrorCode::ConnectionBroken".to_string()),
            Ok(local.to_string())
        )
    );
    assert_eq!(peer.received_soon().await, Vec::<Vec<u8>>::new());

    // The loss is recorded: a second replay returns the same results without the squatter.
    drop(_squatter);
    session.restart_executor().await?;
    let sent_later: Result<(), String> = session
        .call(
            "p3_udp_send",
            data_value!("u".to_string(), Some(peer.port()), b"lost".to_vec()),
        )
        .await?;
    assert_eq!(sent_later, Err("ErrorCode::ConnectionBroken".to_string()));
    session.finish().await
}

/// `disconnect` is not recorded, but the entry follows it on replay: the rebuilt socket is bound
/// and not connected.
#[test]
#[timeout("3m")]
async fn p3_udp_disconnect_replays_from_the_entry(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let peer = UdpPeer::start().await?;
    let other = UdpPeer::start().await?;
    let mut session = Session::start(deps, &context, host_api_tests, sockets(), "udp-disc").await?;
    let local = free_udp_address().await?;
    let bound: Result<(), String> = session
        .call("p3_udp_bind", data_value!("u".to_string(), local.port()))
        .await?;
    assert_eq!(bound, Ok(()));
    let connected: Result<(), String> = session
        .call("p3_udp_connect", data_value!("u".to_string(), peer.port()))
        .await?;
    assert_eq!(connected, Ok(()));
    let disconnected: Result<(), String> = session
        .call("p3_udp_disconnect", data_value!("u".to_string()))
        .await?;
    assert_eq!(disconnected, Ok(()));

    session.restart_executor().await?;
    let remote: Result<String, String> = session
        .call("p3_udp_remote_address", data_value!("u".to_string()))
        .await?;
    let again: Result<(), String> = session
        .call("p3_udp_disconnect", data_value!("u".to_string()))
        .await?;
    let sent: Result<(), String> = session
        .call(
            "p3_udp_send",
            data_value!("u".to_string(), Some(other.port()), b"free".to_vec()),
        )
        .await?;
    assert_eq!(
        (remote, again, sent),
        (
            Err("ErrorCode::InvalidState".to_string()),
            Err("ErrorCode::InvalidState".to_string()),
            Ok(())
        )
    );
    assert_eq!(other.receive().await?, (b"free".to_vec(), local));
    session.finish().await
}

/// A completed P2 UDP exchange is not repeated by replay: the peer gets no second datagram, the
/// received datagram comes from the oplog, and no sent bytes are stored. `start-bind`, `stream`,
/// `check-send`, `send` and `receive` are recorded once each.
#[test]
#[timeout("3m")]
async fn p2_udp_completed_exchange_replays_without_contacting_the_peer(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let peer = UdpPeer::start().await?;
    let mut session =
        Session::start(deps, &context, host_api_tests, sockets(), "p2-udp-exchange").await?;
    let local = free_udp_address().await?;
    let bound: Result<(), String> = session
        .call("p2_udp_bind", data_value!("u".to_string(), local.port()))
        .await?;
    assert_eq!(bound, Ok(()));
    let sent: Result<u64, String> = session
        .call(
            "p2_udp_send_to",
            data_value!("u".to_string(), Some(peer.port()), b"p2-7a1f".to_vec()),
        )
        .await?;
    assert_eq!(sent, Ok(1));
    assert_eq!(peer.receive().await?, (b"p2-7a1f".to_vec(), local));

    let (receive, answer) = tokio::join!(
        session.call::<Result<(Vec<u8>, String), String>>(
            "p2_udp_receive",
            data_value!("u".to_string())
        ),
        peer.socket.send_to(b"world", local)
    );
    answer?;
    assert_eq!(
        receive?,
        Ok((b"world".to_vec(), format!("127.0.0.1:{}", peer.port())))
    );
    let receives_before = session.starts("sockets::udp::receive").await?;

    session.restart_executor().await?;
    let log: Vec<(String, Vec<u8>)> = session.call("received", data_value!()).await?;
    assert_eq!(log, vec![received("u", b"world")]);
    assert_eq!(
        peer.received_soon().await,
        Vec::<Vec<u8>>::new(),
        "replay sends no datagram"
    );
    assert_eq!(session.starts("sockets::udp::start_bind").await?, 1);
    assert_eq!(session.starts("sockets::udp::stream").await?, 1);
    assert_eq!(session.starts("sockets::udp::send").await?, 1);
    assert_eq!(session.starts("sockets::udp::check_send").await?, 1);
    assert_eq!(
        session.starts("sockets::udp::receive").await?,
        receives_before
    );
    assert!(!oplog_shows(&session, b"p2-7a1f").await?);
    session.finish().await
}

/// A connected P2 UDP socket gets its endpoint back after replay: the getters answer from the
/// entry, and a connected `send` with no address reaches the peer from the recorded local port.
#[test]
#[timeout("3m")]
async fn p2_udp_connected_stream_survives_replay(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let peer = UdpPeer::start().await?;
    let mut session =
        Session::start(deps, &context, host_api_tests, sockets(), "p2-udp-conn").await?;
    let local = free_udp_address().await?;
    let bound: Result<(), String> = session
        .call(
            "p2_udp_bind_streams",
            data_value!("u".to_string(), local.port(), Some(peer.port())),
        )
        .await?;
    assert_eq!(bound, Ok(()));
    let sent: Result<u64, String> = session
        .call(
            "p2_udp_send_to",
            data_value!("u".to_string(), None::<u16>, b"before".to_vec()),
        )
        .await?;
    assert_eq!(sent, Ok(1));
    assert_eq!(peer.receive().await?, (b"before".to_vec(), local));

    session.restart_executor().await?;
    let local_after: Result<String, String> = session
        .call("p2_udp_local_address", data_value!("u".to_string()))
        .await?;
    let remote_after: Result<String, String> = session
        .call("p2_udp_remote_address", data_value!("u".to_string()))
        .await?;
    let sent: Result<u64, String> = session
        .call(
            "p2_udp_send_to",
            data_value!("u".to_string(), None::<u16>, b"after".to_vec()),
        )
        .await?;
    assert_eq!(
        (local_after, remote_after, sent),
        (
            Ok(local.to_string()),
            Ok(format!("127.0.0.1:{}", peer.port())),
            Ok(1)
        )
    );
    assert_eq!(peer.receive().await?, (b"after".to_vec(), local));

    // A live receive after the rebuild reads the native socket.
    let (receive, answer) = tokio::join!(
        session.call::<Result<(Vec<u8>, String), String>>(
            "p2_udp_receive",
            data_value!("u".to_string())
        ),
        peer.socket.send_to(b"reply", local)
    );
    answer?;
    assert_eq!(
        receive?,
        Ok((b"reply".to_vec(), format!("127.0.0.1:{}", peer.port())))
    );
    session.finish().await
}

/// A bind to port 0 records the port the OS chose, so the rebuilt socket sends from it.
#[test]
#[timeout("3m")]
async fn p2_udp_ephemeral_bind_keeps_its_port_after_replay(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let peer = UdpPeer::start().await?;
    let mut session =
        Session::start(deps, &context, host_api_tests, sockets(), "p2-udp-eph").await?;
    let bound: Result<(), String> = session
        .call("p2_udp_bind", data_value!("u".to_string(), 0u16))
        .await?;
    assert_eq!(bound, Ok(()));
    let local: String = session
        .call::<Result<String, String>>("p2_udp_local_address", data_value!("u".to_string()))
        .await?
        .map_err(anyhow::Error::msg)?;
    assert!(!local.ends_with(":0"), "{local}");

    session.restart_executor().await?;
    let local_after: Result<String, String> = session
        .call("p2_udp_local_address", data_value!("u".to_string()))
        .await?;
    assert_eq!(local_after, Ok(local.clone()));
    let sent: Result<u64, String> = session
        .call(
            "p2_udp_send_to",
            data_value!("u".to_string(), Some(peer.port()), b"eph".to_vec()),
        )
        .await?;
    assert_eq!(sent, Ok(1));
    let (data, source) = peer.receive().await?;
    assert_eq!((data, source.to_string()), (b"eph".to_vec(), local));
    session.finish().await
}

/// The P2 `error-code` values as the fixture renders them with `Debug`.
const P2_CONNECTION_RESET: &str = "ErrorCode { code: 15, name: \"connection-reset\", message: \"The TCP connection was reset.\" }";
const P2_INVALID_STATE: &str = "ErrorCode { code: 9, name: \"invalid-state\", message: \"The operation is not valid in the socket's current state.\" }";

/// When the recorded local port is taken after the crash, the first live operation fails with
/// `connection-reset`, every later one does too, and the loss is recorded.
#[test]
#[timeout("3m")]
async fn p2_udp_taken_port_after_replay_reports_connection_reset(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let peer = UdpPeer::start().await?;
    let mut session =
        Session::start(deps, &context, host_api_tests, sockets(), "p2-udp-taken").await?;
    let local = free_udp_address().await?;
    let bound: Result<(), String> = session
        .call("p2_udp_bind", data_value!("u".to_string(), local.port()))
        .await?;
    assert_eq!(bound, Ok(()));

    drop(session.executor.take());
    let squatter = tokio::time::timeout(WAIT, async {
        loop {
            match UdpSocket::bind(local).await {
                Ok(socket) => break socket,
                Err(error) if error.kind() == std::io::ErrorKind::AddrInUse => {
                    tokio::time::sleep(Duration::from_millis(50)).await
                }
                Err(error) => panic!("binding the recorded port failed: {error}"),
            }
        }
    })
    .await
    .map_err(|_| anyhow::anyhow!("the old executor never released {local}"))?;
    session.restart_executor().await?;
    let checked: Result<u64, String> = session
        .call("p2_udp_check_send", data_value!("u".to_string()))
        .await?;
    let sent: Result<u64, String> = session
        .call(
            "p2_udp_send_to",
            data_value!("u".to_string(), Some(peer.port()), b"lost".to_vec()),
        )
        .await?;
    let local_after: Result<String, String> = session
        .call("p2_udp_local_address", data_value!("u".to_string()))
        .await?;
    assert_eq!(
        (checked, sent, local_after),
        (
            Err(P2_CONNECTION_RESET.to_string()),
            Err(P2_CONNECTION_RESET.to_string()),
            Ok(local.to_string())
        )
    );
    assert_eq!(peer.received_soon().await, Vec::<Vec<u8>>::new());

    drop(squatter);
    session.restart_executor().await?;
    let sent_later: Result<u64, String> = session
        .call(
            "p2_udp_send_to",
            data_value!("u".to_string(), Some(peer.port()), b"lost".to_vec()),
        )
        .await?;
    assert_eq!(sent_later, Err(P2_CONNECTION_RESET.to_string()));
    assert_eq!(peer.received_soon().await, Vec::<Vec<u8>>::new());
    session.finish().await
}

/// A `stream(None)` after a connected `stream` replays from the entry: the rebuilt socket is
/// bound and not connected, and sends to another peer.
#[test]
#[timeout("3m")]
async fn p2_udp_restream_unconnected_replays_from_the_entry(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let peer = UdpPeer::start().await?;
    let other = UdpPeer::start().await?;
    let mut session =
        Session::start(deps, &context, host_api_tests, sockets(), "p2-udp-disc").await?;
    let local = free_udp_address().await?;
    let bound: Result<(), String> = session
        .call(
            "p2_udp_bind_streams",
            data_value!("u".to_string(), local.port(), Some(peer.port())),
        )
        .await?;
    assert_eq!(bound, Ok(()));
    let restreamed: Result<(), String> = session
        .call("p2_udp_restream", data_value!("u".to_string(), None::<u16>))
        .await?;
    assert_eq!(restreamed, Ok(()));

    session.restart_executor().await?;
    let remote: Result<String, String> = session
        .call("p2_udp_remote_address", data_value!("u".to_string()))
        .await?;
    let sent: Result<u64, String> = session
        .call(
            "p2_udp_send_to",
            data_value!("u".to_string(), Some(other.port()), b"free".to_vec()),
        )
        .await?;
    assert_eq!((remote, sent), (Err(P2_INVALID_STATE.to_string()), Ok(1)));
    assert_eq!(other.receive().await?, (b"free".to_vec(), local));
    session.finish().await
}

/// A connect fixes a wildcard local address. The entry takes the connected local address, so the
/// getter answers the same after replay and the rebuilt socket sends from the same address.
#[test]
#[timeout("3m")]
async fn p2_udp_wildcard_bind_records_the_connected_local_address(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let peer = UdpPeer::start().await?;
    let mut session =
        Session::start(deps, &context, host_api_tests, sockets(), "p2-udp-wild").await?;
    let bound: Result<(), String> = session
        .call(
            "p2_udp_bind_streams_at",
            data_value!(
                "u".to_string(),
                "0.0.0.0:0".to_string(),
                Some(format!("127.0.0.1:{}", peer.port()))
            ),
        )
        .await?;
    assert_eq!(bound, Ok(()));
    let local: String = session
        .call::<Result<String, String>>("p2_udp_local_address", data_value!("u".to_string()))
        .await?
        .map_err(anyhow::Error::msg)?;
    assert!(
        local.starts_with("127.0.0.1:") && !local.ends_with(":0"),
        "{local}"
    );

    session.restart_executor().await?;
    let local_after: Result<String, String> = session
        .call("p2_udp_local_address", data_value!("u".to_string()))
        .await?;
    assert_eq!(local_after, Ok(local.clone()));
    let sent: Result<u64, String> = session
        .call(
            "p2_udp_send_to",
            data_value!("u".to_string(), None::<u16>, b"wild".to_vec()),
        )
        .await?;
    assert_eq!(sent, Ok(1));
    let (data, source) = peer.receive().await?;
    assert_eq!((data, source.to_string()), (b"wild".to_vec(), local));
    session.finish().await
}

/// A failed connect of a connected socket leaves the native socket disconnected. The entry
/// records the state the call left, so the getters answer the same after replay.
#[test]
#[timeout("3m")]
async fn p2_udp_failed_restream_records_the_state_it_leaves(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let peer = UdpPeer::start().await?;
    let mut session =
        Session::start(deps, &context, host_api_tests, sockets(), "p2-udp-fail").await?;
    let local = free_udp_address().await?;
    let bound: Result<(), String> = session
        .call(
            "p2_udp_bind_streams",
            data_value!("u".to_string(), local.port(), Some(peer.port())),
        )
        .await?;
    assert_eq!(bound, Ok(()));
    let restreamed: Result<(), String> = session
        .call(
            "p2_udp_restream_to",
            data_value!("u".to_string(), Some("240.0.0.1:12345".to_string())),
        )
        .await?;
    assert!(
        restreamed
            .as_ref()
            .is_err_and(|error| error.contains("invalid-argument")),
        "{restreamed:?}"
    );
    let live_local: Result<String, String> = session
        .call("p2_udp_local_address", data_value!("u".to_string()))
        .await?;
    let live_remote: Result<String, String> = session
        .call("p2_udp_remote_address", data_value!("u".to_string()))
        .await?;
    assert_eq!(
        (live_local.clone(), live_remote.clone()),
        (Ok(local.to_string()), Err(P2_INVALID_STATE.to_string()))
    );

    session.restart_executor().await?;
    let local_after: Result<String, String> = session
        .call("p2_udp_local_address", data_value!("u".to_string()))
        .await?;
    let remote_after: Result<String, String> = session
        .call("p2_udp_remote_address", data_value!("u".to_string()))
        .await?;
    assert_eq!((local_after, remote_after), (live_local, live_remote));
    session.finish().await
}

/// A disconnect can release an ephemeral port, and the next send binds a new one. The entry
/// records the port that send used, so the rebuilt socket sends from it after replay.
#[test]
#[timeout("3m")]
async fn p2_udp_send_after_disconnect_records_its_local_address(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let peer = UdpPeer::start().await?;
    let other = UdpPeer::start().await?;
    let mut session =
        Session::start(deps, &context, host_api_tests, sockets(), "p2-udp-rel").await?;
    let bound: Result<(), String> = session
        .call(
            "p2_udp_bind_streams",
            data_value!("u".to_string(), 0u16, Some(peer.port())),
        )
        .await?;
    assert_eq!(bound, Ok(()));
    let restreamed: Result<(), String> = session
        .call("p2_udp_restream", data_value!("u".to_string(), None::<u16>))
        .await?;
    assert_eq!(restreamed, Ok(()));
    let sent: Result<u64, String> = session
        .call(
            "p2_udp_send_to",
            data_value!("u".to_string(), Some(other.port()), b"one".to_vec()),
        )
        .await?;
    assert_eq!(sent, Ok(1));
    let (_, source) = other.receive().await?;
    let local: Result<String, String> = session
        .call("p2_udp_local_address", data_value!("u".to_string()))
        .await?;
    assert_eq!(local, Ok(source.to_string()));

    session.restart_executor().await?;
    let local_after: Result<String, String> = session
        .call("p2_udp_local_address", data_value!("u".to_string()))
        .await?;
    assert_eq!(local_after, Ok(source.to_string()));
    let sent: Result<u64, String> = session
        .call(
            "p2_udp_send_to",
            data_value!("u".to_string(), Some(other.port()), b"two".to_vec()),
        )
        .await?;
    assert_eq!(sent, Ok(1));
    assert_eq!(other.receive().await?, (b"two".to_vec(), source));
    session.finish().await
}
