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
use golem_common::model::agent::{
    ParsedAgentKey, RevertToOplogIndex, RevertWorkerTarget, UpdateRecord,
};
use golem_common::model::component::ComponentDto;
use golem_common::model::oplog::{PublicAgentInvocation, PublicOplogEntry};
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

    /// Waits until the oplog holds a snapshot after the start of the latest invocation of
    /// `method`, and gives the number of snapshots. A snapshot of an earlier invocation, such as
    /// the initialization, does not count.
    async fn wait_for_snapshot_after_call(&self, method: &str) -> anyhow::Result<usize> {
        tokio::time::timeout(WAIT, async {
            loop {
                let oplog = self.oplog().await?;
                let start = oplog.iter().rposition(|entry| {
                    matches!(entry, PublicOplogEntry::AgentInvocationStarted(started)
                        if matches!(&started.invocation, PublicAgentInvocation::AgentMethodInvocation(invocation)
                            if invocation.method_name == method))
                });
                if let Some(start) = start
                    && oplog[start..]
                        .iter()
                        .any(|entry| matches!(entry, PublicOplogEntry::Snapshot(_)))
                {
                    return Ok::<_, anyhow::Error>(
                        oplog
                            .iter()
                            .filter(|entry| matches!(entry, PublicOplogEntry::Snapshot(_)))
                            .count(),
                    );
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await
        .map_err(|_| anyhow::anyhow!("no snapshot after the latest call of {method}"))?
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

    // A bind to port 0 is not recorded, so its port differs after replay. The agent binds a port
    // the test reserved, and never returns its address.
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

/// A completed P2 TCP connect replays from its recorded `finish-connect` result. Replay does not
/// connect again, and `local-address` returns the recorded address, so an invocation that
/// returned it replays without divergence. A second socket, created after the first replayed
/// `finish-connect`, keeps its own address too. The streams of a replayed connection are lost:
/// the next write fails with `connection-reset`, the P2 code for a lost connection, and the peer
/// gets nothing.
#[test]
#[timeout("3m")]
async fn p2_tcp_completed_connect_replays_without_connecting_again(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let peer = TcpPeer::start(b"", Reply::Silent).await?;
    let mut session =
        Session::start(deps, &context, host_api_tests, sockets(), "p2-tcp-replay").await?;

    let mut addresses = Vec::new();
    for slot in ["a", "b"] {
        let connected: Result<(), String> = session
            .call("p2_tcp_connect", data_value!(slot.to_string(), peer.port))
            .await?;
        assert_eq!(connected, Ok(()));
        let address: Result<String, String> = session
            .call("p2_tcp_local_address", data_value!(slot.to_string()))
            .await?;
        addresses.push(address.map_err(anyhow::Error::msg)?);
    }
    assert_ne!(addresses[0], addresses[1]);
    peer.wait_until("two connections", |connections| connections.len() == 2)
        .await?;

    session.restart_executor().await?;
    for (slot, before) in ["a", "b"].into_iter().zip(&addresses) {
        let after: Result<String, String> = session
            .call("p2_tcp_local_address", data_value!(slot.to_string()))
            .await?;
        assert_eq!(after.as_ref(), Ok(before), "local address of slot {slot}");
    }
    assert_eq!(peer.connections().len(), 2, "replay does not connect again");
    assert_eq!(
        session.starts("sockets::tcp::finish_connect").await?,
        2,
        "replay records no new finish-connect"
    );

    let sent: Result<(), String> = session
        .call(
            "p2_tcp_send",
            data_value!("a".to_string(), b"ping\n".to_vec()),
        )
        .await?;
    assert_eq!(
        sent,
        Err("last-operation-failed: connection reset".to_string()),
        "the lost connection reports connection-reset"
    );
    let received: Result<Vec<u8>, String> = session
        .call("p2_tcp_receive", data_value!("b".to_string(), 1u32))
        .await?;
    assert_eq!(
        received,
        Err("last-operation-failed: connection reset".to_string()),
        "the lost connection reports connection-reset"
    );
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(
        peer.connections()
            .iter()
            .all(|connection| connection.received.is_empty()),
        "a lost connection sends nothing"
    );
    session.finish().await
}

/// Each `finish-connect` result is recorded, `would-block` too. Replay returns the same results
/// in the same order, so the agent counts the same number of `would-block` results, with no native
/// connect.
#[test]
#[timeout("3m")]
async fn p2_tcp_connect_with_would_block_results_replays(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let peer = TcpPeer::start(b"", Reply::Silent).await?;
    let mut session = Session::start(
        deps,
        &context,
        host_api_tests,
        sockets(),
        "p2-tcp-would-block",
    )
    .await?;

    let would_block: Result<u32, String> = session
        .call(
            "p2_tcp_connect_counting",
            data_value!("a".to_string(), peer.port),
        )
        .await?;
    let would_block = would_block.map_err(anyhow::Error::msg)?;
    assert!(would_block >= 1, "the first finish-connect would block");
    let finishes = session.starts("sockets::tcp::finish_connect").await?;
    assert_eq!(finishes, would_block as usize + 1);
    let before: Result<String, String> = session
        .call("p2_tcp_local_address", data_value!("a".to_string()))
        .await?;
    let before = before.map_err(anyhow::Error::msg)?;

    session.restart_executor().await?;
    let after: Result<String, String> = session
        .call("p2_tcp_local_address", data_value!("a".to_string()))
        .await?;
    assert_eq!(after, Ok(before));
    assert_eq!(
        session.starts("sockets::tcp::finish_connect").await?,
        finishes
    );
    assert_eq!(peer.connections().len(), 1, "replay does not connect again");
    session.finish().await
}

/// S10 on P2: a read-only method cannot connect a P2 TCP socket. `start-connect` is a recorded
/// write, so the read-only guard refuses it before any poll can drive the native connect.
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
    let error = format!(
        "{:?}",
        result.expect_err("the read-only connect is refused")
    );
    assert!(
        error.contains("sockets::tcp::start_connect"),
        "the refusal names the host function: {error}"
    );
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(peer.connections().len(), 0, "the peer sees no connection");
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
    let baseline = session.wait_for_snapshot_after_call("received").await?;

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

/// A live P3 TCP send delivers each byte to the peer once and records only byte counts. Replay
/// of the completed conversation neither connects nor sends again.
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
    assert_eq!(
        session
            .starts("sockets::types::tcp-socket::send-chunk")
            .await?,
        1
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
    assert_eq!(peer.connections()[1].received, b"again\n".to_vec());
    assert_eq!(
        session
            .starts("sockets::types::tcp-socket::connect")
            .await?,
        2
    );
    session.finish().await
}

/// G4 and G1 on one pooled connection. A send stream kept open across invocations does not hold
/// them: the parked send parent leaves the agent idle, so each call returns well before the
/// tail-work deadline. After a restart, replay drains exactly the recorded writes, the next write
/// is refused, and the send completes with `connection-broken`. Each byte reaches the peer once.
#[test]
#[timeout("5m")]
async fn p3_tcp_send_kept_across_invocations_is_lost_after_replay(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let peer = TcpPeer::start(b"", Reply::Silent).await?;
    let mut session = Session::start(
        deps,
        &context,
        host_api_tests,
        sockets(),
        "p3-tcp-send-kept",
    )
    .await?;
    let _: Result<(), String> = session
        .call("p3_tcp_connect", data_value!("a".to_string(), peer.port))
        .await?;

    let quick = Duration::from_secs(10);
    let opened = tokio::time::timeout(
        quick,
        session.call::<Result<(), String>>("p3_tcp_open_send", data_value!("a".to_string())),
    )
    .await
    .map_err(|_| anyhow::anyhow!("opening the send stream held the invocation"))??;
    assert_eq!(opened, Ok(()));
    for part in [b"one".to_vec(), b"two".to_vec()] {
        let written = tokio::time::timeout(
            quick,
            session.call::<Result<u32, String>>("p3_tcp_write", data_value!("a".to_string(), part)),
        )
        .await
        .map_err(|_| anyhow::anyhow!("a write held the invocation"))??;
        assert_eq!(written, Ok(3));
    }
    peer.wait_until("both writes arrive", |connections| {
        connections.len() == 1 && connections[0].received == b"onetwo"
    })
    .await?;
    assert!(!oplog_shows(&session, b"onetwo").await?);

    session.restart_executor().await?;
    let written: Result<u32, String> = session
        .call(
            "p3_tcp_write",
            data_value!("a".to_string(), b"three".to_vec()),
        )
        .await?;
    assert_eq!(
        written,
        Ok(0),
        "the live write after replay is not accepted"
    );
    let closed: Result<(), String> = session
        .call("p3_tcp_close_send", data_value!("a".to_string()))
        .await?;
    assert_eq!(closed, Err("ErrorCode::ConnectionBroken".to_string()));
    peer.wait_until(
        "the connection from before the restart closes",
        |connections| connections.len() == 1 && connections[0].closed,
    )
    .await?;
    assert_eq!(peer.connections()[0].received, b"onetwo".to_vec());
    assert_eq!(
        session
            .starts("sockets::types::tcp-socket::send-chunk")
            .await?,
        3,
        "two replayed writes and the refused one"
    );
    session.finish().await
}

/// G1 across a crash inside an open send. The first part is sent live; the agent crashes during
/// the pause. Recovery drains the recorded first part without sending it again, and the second
/// part, written live after replay, is refused instead of being accepted and dropped.
#[test]
#[timeout("3m")]
async fn p3_tcp_send_open_at_a_crash_refuses_live_writes(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let peer = TcpPeer::start(b"", Reply::Silent).await?;
    let session = Session::start(
        deps,
        &context,
        host_api_tests,
        sockets(),
        "p3-tcp-send-crash",
    )
    .await?;
    let _: Result<(), String> = session
        .call("p3_tcp_connect", data_value!("a".to_string(), peer.port))
        .await?;
    let call = {
        let executor = session.executor().clone();
        let component = session.component.clone();
        let agent_id = session.agent_id.clone();
        tokio::spawn(async move {
            executor
                .invoke_and_await_agent(
                    &component,
                    &agent_id,
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
    let result = call.await??.into_typed::<Result<(), String>>()?;
    assert!(
        result
            .as_ref()
            .is_err_and(|error| error.contains("6 bytes were not accepted")),
        "the live write after replay must not be accepted: {result:?}"
    );
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(peer.connections().len(), 1, "no new connection");
    assert_eq!(peer.connections()[0].received, b"first".to_vec());
    session.finish().await
}

/// A guest that drops its socket right after opening the send stream. The stream owns the
/// connection, so the bytes written afterwards reach the peer. The `send` import takes the native
/// stream before it returns, while the task commits the parent `Start` later. Replay of the
/// completed send neither reconnects nor needs the dropped socket.
#[test]
#[timeout("3m")]
async fn p3_tcp_send_survives_dropping_the_socket(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let peer = TcpPeer::start(b"", Reply::Silent).await?;
    let mut session = Session::start(
        deps,
        &context,
        host_api_tests,
        sockets(),
        "p3-tcp-send-dropped-socket",
    )
    .await?;
    let _: Result<(), String> = session
        .call("p3_tcp_connect", data_value!("a".to_string(), peer.port))
        .await?;
    let sent: Result<(), String> = session
        .call(
            "p3_tcp_send_after_dropping_socket",
            data_value!("a".to_string(), b"after-drop-9c1e".to_vec()),
        )
        .await?;
    assert_eq!(sent, Ok(()));
    peer.wait_until(
        "the bytes arrive and the connection closes",
        |connections| connections.len() == 1 && connections[0].closed,
    )
    .await?;
    assert_eq!(peer.connections()[0].received, b"after-drop-9c1e".to_vec());

    session.restart_executor().await?;
    let _: Vec<(String, Vec<u8>)> = session.call("received", data_value!()).await?;
    assert!(
        !session
            .oplog()
            .await?
            .iter()
            .any(|entry| matches!(entry, PublicOplogEntry::Error(_))),
        "replay of the send after the socket drop failed"
    );
    assert_eq!(peer.connections().len(), 1, "replay does not reconnect");
    session.finish().await
}

/// A peer that does not read holds back the send, so a guest write stays pending. The guest
/// cancels it: the native write stops without a partial effect, its `send-chunk` child records
/// no accepted bytes, and the cancel completes instead of waiting for the peer. The peer later
/// reads exactly the accepted bytes, and the send replays after a restart without reconnecting.
#[test]
#[timeout("3m")]
async fn p3_tcp_send_cancels_a_write_the_peer_holds_back(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let listener = tokio::net::TcpSocket::new_v4()?;
    listener.set_recv_buffer_size(4096)?;
    listener.bind("127.0.0.1:0".parse()?)?;
    let listener = listener.listen(1)?;
    let port = listener.local_addr()?.port();
    let mut session = Session::start(
        deps,
        &context,
        host_api_tests,
        sockets(),
        "p3-tcp-send-held-back",
    )
    .await?;
    let connect =
        session.call::<Result<(), String>>("p3_tcp_connect", data_value!("a".to_string(), port));
    let ((mut peer, _), connected) = tokio::try_join!(
        async { Ok::<_, anyhow::Error>(listener.accept().await?) },
        connect
    )?;
    assert_eq!(connected, Ok(()));

    let sent: Result<(u64, String, Result<(), String>), String> = tokio::time::timeout(
        Duration::from_secs(60),
        session.call(
            "p3_tcp_send_until_held_back",
            data_value!("a".to_string(), 65536u32, 500u64),
        ),
    )
    .await
    .map_err(|_| anyhow::anyhow!("the cancelled write waited for the peer"))??;
    let (accepted, ending, result) = sent.map_err(anyhow::Error::msg)?;
    assert_eq!(ending, "cancelled");
    assert_eq!(result, Ok(()));
    assert!(accepted > 0, "the peer's buffers take the first writes");

    let mut received = Vec::new();
    tokio::time::timeout(Duration::from_secs(30), peer.read_to_end(&mut received)).await??;
    assert_eq!(
        received.len() as u64,
        accepted,
        "the peer gets exactly the accepted bytes"
    );

    let oplog = session
        .executor()
        .get_oplog(&session.worker_id, OplogIndex::INITIAL)
        .await?;
    let chunks = oplog
        .iter()
        .filter(|entry| {
            matches!(&entry.entry, PublicOplogEntry::Start(start)
            if start.function_name == "sockets::types::tcp-socket::send-chunk")
        })
        .map(|entry| entry.oplog_index)
        .collect::<Vec<_>>();
    assert!(
        chunks.len() >= 2,
        "the accepted writes and the cancelled one are recorded"
    );
    for start in &chunks {
        assert!(
            oplog.iter().any(|entry| matches!(&entry.entry,
                PublicOplogEntry::End(end) if end.start_index == *start)),
            "the send-chunk child at {start} has a terminal"
        );
    }

    session.restart_executor().await?;
    let _: Vec<(String, Vec<u8>)> = session.call("received", data_value!()).await?;
    assert!(
        !session
            .oplog()
            .await?
            .iter()
            .any(|entry| matches!(entry, PublicOplogEntry::Error(_))),
        "replay of the cancelled write failed"
    );
    assert!(
        tokio::time::timeout(Duration::from_millis(500), listener.accept())
            .await
            .is_err(),
        "replay does not reconnect"
    );
    session.finish().await
}

/// The guest drops a P3 TCP socket right after it opens the receive stream, before the receive
/// task runs. The stream owns the connection, so it still delivers the peer's bytes, and the
/// completed receive replays without the dropped socket.
#[test]
#[timeout("3m")]
async fn p3_tcp_receive_survives_dropping_the_socket(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let peer = TcpPeer::start(b"drop-7a3f", Reply::Silent).await?;
    let mut session = Session::start(
        deps,
        &context,
        host_api_tests,
        sockets(),
        "p3-tcp-receive-dropped-socket",
    )
    .await?;
    let received: Result<Vec<u8>, String> = session
        .call(
            "p3_tcp_receive_after_dropping_socket",
            data_value!(peer.port, 9u32),
        )
        .await?;
    assert_eq!(received, Ok(b"drop-7a3f".to_vec()));

    session.restart_executor().await?;
    let _: Vec<(String, Vec<u8>)> = session.call("received", data_value!()).await?;
    assert!(
        !session
            .oplog()
            .await?
            .iter()
            .any(|entry| matches!(entry, PublicOplogEntry::Error(_))),
        "replay of the receive after the socket drop failed"
    );
    assert_eq!(peer.connections().len(), 1, "replay does not reconnect");
    session.finish().await
}

/// A revert cut inside an open send parent. The cut keeps the send `Start` and its first write,
/// and deletes the invocation of the second write. Recovery replays the retained write, the
/// next write after the cut is refused instead of being sent, and the send parent completes
/// with `connection-broken` under its original `Start`. The revert does not undo bytes that
/// already reached the peer, and Golem does not reconnect.
#[test]
#[timeout("3m")]
async fn p3_tcp_revert_inside_an_open_send_recovers(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let peer = TcpPeer::start(b"", Reply::Silent).await?;
    let session = Session::start(
        deps,
        &context,
        host_api_tests,
        sockets(),
        "p3-tcp-send-revert",
    )
    .await?;
    let _: Result<(), String> = session
        .call("p3_tcp_connect", data_value!("a".to_string(), peer.port))
        .await?;
    let opened: Result<(), String> = session
        .call("p3_tcp_open_send", data_value!("a".to_string()))
        .await?;
    assert_eq!(opened, Ok(()));
    for part in [b"one".to_vec(), b"two".to_vec()] {
        let written: Result<u32, String> = session
            .call("p3_tcp_write", data_value!("a".to_string(), part))
            .await?;
        assert_eq!(written, Ok(3));
    }
    peer.wait_until("both writes arrive", |connections| {
        connections.len() == 1 && connections[0].received == b"onetwo"
    })
    .await?;

    let oplog = session
        .executor()
        .get_oplog(&session.worker_id, OplogIndex::INITIAL)
        .await?;
    let start_of = |name: &str| {
        oplog
            .iter()
            .filter(|entry| {
                matches!(&entry.entry, PublicOplogEntry::Start(start) if start.function_name == name)
            })
            .map(|entry| entry.oplog_index)
            .collect::<Vec<_>>()
    };
    let parent = start_of("sockets::types::tcp-socket::send");
    let chunks = start_of("sockets::types::tcp-socket::send-chunk");
    assert_eq!((parent.len(), chunks.len()), (1, 2));
    let parent = parent[0];
    // The last entry of the invocation that made the first write.
    let cut = oplog
        .iter()
        .find(|entry| {
            entry.oplog_index > chunks[0]
                && matches!(entry.entry, PublicOplogEntry::AgentInvocationFinished(_))
        })
        .map(|entry| entry.oplog_index)
        .expect("the first write finishes");
    assert!(cut < chunks[1], "the cut deletes the second write");

    session
        .executor()
        .revert(
            &session.worker_id,
            RevertWorkerTarget::RevertToOplogIndex(RevertToOplogIndex {
                last_oplog_index: cut,
            }),
        )
        .await?;

    let written: Result<u32, String> = session
        .call(
            "p3_tcp_write",
            data_value!("a".to_string(), b"three".to_vec()),
        )
        .await?;
    assert_eq!(
        written,
        Ok(0),
        "the live write after the cut is not accepted"
    );
    let closed: Result<(), String> = session
        .call("p3_tcp_close_send", data_value!("a".to_string()))
        .await?;
    assert_eq!(closed, Err("ErrorCode::ConnectionBroken".to_string()));
    peer.wait_until(
        "the connection from before the revert closes",
        |connections| connections.len() == 1 && connections[0].closed,
    )
    .await?;
    assert_eq!(peer.connections()[0].received, b"onetwo".to_vec());

    let oplog = session
        .executor()
        .get_oplog(&session.worker_id, OplogIndex::INITIAL)
        .await?;
    assert!(
        !oplog
            .iter()
            .any(|entry| matches!(entry.entry, PublicOplogEntry::Error(_))),
        "recovery after the cut failed: {oplog:?}"
    );
    assert_eq!(
        oplog
            .iter()
            .filter(|entry| matches!(&entry.entry, PublicOplogEntry::End(end) if end.start_index == parent))
            .count(),
        1,
        "the send parent completes once under its original Start"
    );
    let revert = oplog
        .iter()
        .find(|entry| matches!(entry.entry, PublicOplogEntry::Revert(_)))
        .map(|entry| entry.oplog_index)
        .expect("the revert is recorded");
    assert_eq!(
        oplog
            .iter()
            .filter(|entry| entry.oplog_index > revert
                && matches!(&entry.entry, PublicOplogEntry::Start(start)
                    if start.function_name == "sockets::types::tcp-socket::send-chunk"))
            .count(),
        1,
        "only the refused write records a chunk after the revert"
    );
    session.finish().await
}
