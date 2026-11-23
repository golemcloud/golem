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
use golem_common::model::{OplogIndex, OwnedAgentId};
use golem_common::schema::{FromSchema, TypedSchemaValue};
use golem_common::{agent_key, data_value};
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
    worker_id: OwnedAgentId,
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
        let agent_id = agent_key!(AGENT_TYPE, name);
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
            session
                .executor()
                .commit_oplog(&session.worker_id.agent_id)
                .await?;
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

/// The End entries of the durable calls named `function_name`, rendered.
fn ends_of(entries: &[PublicOplogEntry], function_name: &str) -> Vec<String> {
    let starts: Vec<_> = entries
        .iter()
        .enumerate()
        .filter_map(|(index, entry)| match entry {
            PublicOplogEntry::Start(start) if start.function_name == function_name => {
                Some(OplogIndex::from_u64(index as u64 + 1))
            }
            _ => None,
        })
        .collect();
    entries
        .iter()
        .filter_map(|entry| match entry {
            PublicOplogEntry::End(end) if starts.contains(&end.start_index) => {
                Some(format!("{end:?}"))
            }
            _ => None,
        })
        .collect()
}

/// A P2 write right after the connect, with `blocking-write-and-flush`, reaches the peer once.
/// The oplog records only the count of the written bytes. After a restart, replay returns the
/// recorded result without sending again, and the peer still has one copy on one connection.
#[test]
#[timeout("3m")]
async fn p2_tcp_sent_bytes_reach_the_peer_once_across_a_restart(
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
        "p2-tcp-send-once",
    )
    .await?;

    let connected: Result<(), String> = session
        .call("p2_tcp_connect", data_value!("a".to_string(), peer.port))
        .await?;
    assert_eq!(connected, Ok(()));
    let sent: Result<(), String> = session
        .call(
            "p2_tcp_send_blocking",
            data_value!("a".to_string(), b"hello".to_vec()),
        )
        .await?;
    assert_eq!(
        sent,
        Ok(()),
        "the first write after the connect needs no budget wait"
    );
    peer.wait_until("the peer received hello", |connections| {
        connections.len() == 1 && connections[0].received == b"hello"
    })
    .await?;
    let write = "sockets::tcp::output_stream::blocking_write_and_flush";
    let ends = ends_of(&session.oplog().await?, write);
    assert_eq!(ends.len(), 1, "one recorded write");
    assert!(
        !ends[0].contains("104, 101, 108, 108, 111"),
        "the write records no bytes: {}",
        ends[0]
    );

    session.restart_executor().await?;
    let sends: Vec<Result<(), String>> = session.call("sends", data_value!()).await?;
    assert_eq!(sends, vec![Ok(())], "replay returns the recorded write");
    assert_eq!(session.starts(write).await?, 1, "replay writes nothing");
    tokio::time::sleep(Duration::from_millis(200)).await;
    let connections = peer.connections();
    assert_eq!(connections.len(), 1, "replay does not connect again");
    assert_eq!(connections[0].received, b"hello", "the peer got one copy");
    session.finish().await
}

/// A P2 `blocking-splice` into a TCP output sends its bytes before it returns, as Wasmtime's
/// does. The agent splices a file of 1 MiB into a connection with a small send buffer and drops
/// the socket right after the last splice. The peer has a receive buffer of 4 KiB and reads 4 KiB
/// every 10 ms, so the last write is still in progress at the drop unless the splice waited for it.
/// The peer gets every byte.
#[test]
#[timeout("3m")]
async fn p2_tcp_blocking_splice_sends_all_bytes_before_it_returns(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    const LEN: usize = 1024 * 1024;
    let context = TestContext::new(last_unique_id);
    let listener = tokio::net::TcpSocket::new_v4()?;
    listener.set_recv_buffer_size(4096)?;
    listener.bind("127.0.0.1:0".parse()?)?;
    let listener = listener.listen(1)?;
    let port = listener.local_addr()?.port();
    let peer = AbortOnDropHandle::new(tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await?;
        let mut received = Vec::new();
        let mut buffer = [0u8; 4096];
        loop {
            match stream.read(&mut buffer).await? {
                0 => break,
                n => received.extend_from_slice(&buffer[..n]),
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        anyhow::Ok(received)
    }));
    let session = Session::start(
        deps,
        &context,
        host_api_tests,
        sockets(),
        "p2-tcp-splice-flush",
    )
    .await?;

    let spliced: Result<u64, String> = session
        .call("p2_tcp_blocking_splice_file", data_value!(port, LEN as u32))
        .await?;
    assert_eq!(spliced, Ok(LEN as u64));
    let received = tokio::time::timeout(WAIT, peer).await???;
    let expected: Vec<u8> = (0..LEN).map(|i| (i % 251) as u8).collect();
    assert_eq!(received.len(), LEN, "the peer got every spliced byte");
    assert!(received == expected, "the peer got the bytes in order");
    session.finish().await
}

/// A completed P2 read replays from the oplog with the peer gone: the agent gets back the bytes
/// it read, and replay starts no read and no connection.
#[test]
#[timeout("3m")]
async fn p2_tcp_completed_read_replays_with_the_peer_silent(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let peer = TcpPeer::start(b"abc", Reply::Silent).await?;
    let port = peer.port;
    let mut session = Session::start(
        deps,
        &context,
        host_api_tests,
        sockets(),
        "p2-tcp-read-replay",
    )
    .await?;

    let _: Result<(), String> = session
        .call("p2_tcp_connect", data_value!("a".to_string(), port))
        .await?;
    let first: Result<Vec<u8>, String> = session
        .call("p2_tcp_receive", data_value!("a".to_string(), 3u32))
        .await?;
    assert_eq!(first, Ok(b"abc".to_vec()));
    let read = "sockets::tcp::input_stream::blocking_read";
    let reads = session.starts(read).await?;
    assert!(reads >= 1, "the read is recorded");
    drop(peer);

    session.restart_executor().await?;
    let log: Vec<(String, Vec<u8>)> = session.call("received", data_value!()).await?;
    assert_eq!(
        log,
        vec![received("a", b"abc")],
        "replay returns the bytes read"
    );
    assert_eq!(session.starts(read).await?, reads, "replay reads nothing");
    assert!(
        tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .is_err(),
        "the peer is gone"
    );
    session.finish().await
}

/// A write error is recorded and returned. After the peer resets the connection, a write fails,
/// and the oplog records the socket error code of the native stream; replay after a restart
/// returns the same results, with the same error, and the peer saw the first message once and
/// nothing after it.
#[test]
#[timeout("3m")]
async fn p2_tcp_write_error_replays_and_is_not_sent_again(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    let peer = AbortOnDropHandle::new(tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await?;
        let mut first = [0u8; 3];
        stream.read_exact(&mut first).await?;
        // Linger zero: the close resets the connection.
        stream.set_zero_linger()?;
        drop(stream);
        let again = tokio::time::timeout(Duration::from_millis(500), listener.accept()).await;
        Ok::<_, anyhow::Error>((first.to_vec(), again.is_ok()))
    }));
    let mut session = Session::start(
        deps,
        &context,
        host_api_tests,
        sockets(),
        "p2-tcp-write-error",
    )
    .await?;

    let _: Result<(), String> = session
        .call("p2_tcp_connect", data_value!("a".to_string(), port))
        .await?;
    let first: Result<(), String> = session
        .call(
            "p2_tcp_send_blocking",
            data_value!("a".to_string(), b"one".to_vec()),
        )
        .await?;
    assert_eq!(first, Ok(()));
    let mut attempts = 0;
    loop {
        attempts += 1;
        let next: Result<(), String> = session
            .call(
                "p2_tcp_send_blocking",
                data_value!("a".to_string(), b"two".to_vec()),
            )
            .await?;
        if next.is_err() {
            break;
        }
        assert!(attempts < 20, "a write after the reset fails");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let live: Vec<Result<(), String>> = session.call("sends", data_value!()).await?;
    let error = live.last().unwrap().clone().unwrap_err();
    let (got, reconnected) = peer.await??;
    assert_eq!(got, b"one".to_vec());
    assert!(!reconnected, "no second connection");
    let ends = ends_of(
        &session.oplog().await?,
        "sockets::tcp::output_stream::blocking_write_and_flush",
    );
    let failed = ends
        .iter()
        .find(|end| end.contains("Result(Err {"))
        .expect("the failed write is recorded");
    // Linux reports the reset to the next write; macOS may report the stream closed instead,
    // which has no code.
    if cfg!(target_os = "linux") {
        assert_ne!(error, "closed", "Linux reports the reset");
    }
    if error != "closed" {
        assert!(
            error.starts_with("last-operation-failed: "),
            "the write error is returned: {error}"
        );
        assert_records_reset_code(failed);
    }

    session.restart_executor().await?;
    let replayed: Vec<Result<(), String>> = session.call("sends", data_value!()).await?;
    assert_eq!(replayed, live, "replay returns the same results and error");
    session.finish().await
}

/// Asserts that a rendered End records `connection-reset` or `connection-aborted`, the cases 15
/// and 16 of the socket error code; `None` would mean that only the message was recorded.
fn assert_records_reset_code(end: &str) {
    assert!(
        end.contains("Enum { case: 15 }") || end.contains("Enum { case: 16 }"),
        "the failure records the socket error code of the native stream: {end}"
    );
}

/// A read error is recorded with the socket error code of the native stream. The peer resets
/// the connection while the agent waits in a read. A restart replays the recorded failure, so
/// the next read of the lost stream reports it closed, as the native stream did.
#[test]
#[timeout("3m")]
async fn p2_tcp_read_error_records_its_code_and_replays(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    let _peer = AbortOnDropHandle::new(tokio::spawn(async move {
        let (stream, _) = listener.accept().await?;
        tokio::time::sleep(Duration::from_millis(300)).await;
        // Linger zero: the close resets the connection.
        stream.set_zero_linger()?;
        drop(stream);
        tokio::time::sleep(Duration::from_secs(60)).await;
        Ok::<_, anyhow::Error>(())
    }));
    let mut session = Session::start(
        deps,
        &context,
        host_api_tests,
        sockets(),
        "p2-tcp-read-error",
    )
    .await?;

    let connected: Result<(), String> = session
        .call("p2_tcp_connect", data_value!("a".to_string(), port))
        .await?;
    assert_eq!(connected, Ok(()));
    let read: Result<Vec<u8>, String> = session
        .call("p2_tcp_receive", data_value!("a".to_string(), 3u32))
        .await?;
    // Linux reports the reset to the read; another OS may report the end of the stream, which
    // the component returns as the bytes read so far.
    if cfg!(target_os = "linux") {
        assert!(read.is_err(), "the read reports the reset: {read:?}");
    }
    if read.is_err() {
        let ends = ends_of(
            &session.oplog().await?,
            "sockets::tcp::input_stream::blocking_read",
        );
        let failed = ends
            .iter()
            .find(|end| end.contains("Result(Err {"))
            .expect("the failed read is recorded");
        assert_records_reset_code(failed);
    }

    session.restart_executor().await?;
    let replayed: Result<Vec<u8>, String> = session
        .call("p2_tcp_receive", data_value!("a".to_string(), 3u32))
        .await?;
    // The component returns a closed stream as the bytes read so far.
    assert_eq!(
        replayed,
        Ok(Vec::new()),
        "after the restart the replayed read failure leaves the lost stream closed"
    );
    session.finish().await
}

/// A `write` beyond the permit of `check-write` is a bug of the guest. The native stream traps,
/// and the trap fails the invocation permanently: no retry replays the connect into a lost
/// stream that would turn the bug into a lost connection.
#[test]
#[timeout("3m")]
async fn p2_tcp_write_beyond_the_permit_fails_the_invocation(
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
        "p2-tcp-unpermitted",
    )
    .await?;

    let connected: Result<(), String> = session
        .call("p2_tcp_connect", data_value!("a".to_string(), peer.port))
        .await?;
    assert_eq!(connected, Ok(()));
    let result = session
        .executor()
        .invoke_and_await_agent(
            &session.component,
            &session.agent_id,
            "p2_tcp_write_unpermitted",
            data_value!("a".to_string(), b"hello".to_vec()),
        )
        .await;
    let error = format!(
        "{:?}",
        result.expect_err("the unpermitted write fails the invocation")
    );
    assert!(
        error.contains("not permitted to write 5 bytes"),
        "the invocation fails with the trap: {error}"
    );
    let ends = ends_of(
        &session.oplog().await?,
        "sockets::tcp::output_stream::write",
    );
    assert!(
        ends.is_empty(),
        "the unpermitted write completes with no recorded result: {ends:?}"
    );
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(
        peer.connections()
            .iter()
            .all(|connection| connection.received.is_empty()),
        "the peer gets nothing"
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

/// A peer that resets a P2 connection after the native connect and before `finish-connect` does
/// not trap the call. The guest waits on the pollable in one invocation and calls `finish-connect`
/// in the next, after the peer reset the connection with a zero linger. `finish-connect` returns
/// the connection with the recorded connect target as its remote address, and the reset reaches
/// the guest as an error of the connection, without an `Error` entry. Another P2 connection of the
/// agent keeps working.
#[test]
#[timeout("3m")]
async fn p2_tcp_peer_reset_before_finish_connect_does_not_trap(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let other = TcpPeer::start(b"", Reply::Silent).await?;
    let resetting = TcpListener::bind("127.0.0.1:0").await?;
    let resetting_port = resetting.local_addr()?.port();
    let session = Session::start(
        deps,
        &context,
        host_api_tests,
        sockets(),
        "p2-reset-before-finish",
    )
    .await?;

    let connected: Result<(), String> = session
        .call(
            "p2_tcp_connect",
            data_value!("other".to_string(), other.port),
        )
        .await?;
    assert_eq!(connected, Ok(()));

    let started: Result<(), String> = session
        .call(
            "p2_tcp_start_connect",
            data_value!("reset".to_string(), resetting_port),
        )
        .await?;
    assert_eq!(started, Ok(()));
    let (stream, _) = tokio::time::timeout(WAIT, resetting.accept()).await??;
    stream.set_zero_linger()?;
    drop(stream);
    tokio::time::sleep(Duration::from_millis(200)).await;

    let finished: Result<(), String> = session
        .call("p2_tcp_finish_connect", data_value!("reset".to_string()))
        .await?;
    if finished.is_ok() {
        let received: Result<Vec<u8>, String> = session
            .call("p2_tcp_receive", data_value!("reset".to_string(), 1u32))
            .await?;
        assert!(
            received.is_err(),
            "the reset reaches the guest: {received:?}"
        );
    }
    assert!(
        !session
            .oplog()
            .await?
            .iter()
            .any(|entry| matches!(entry, PublicOplogEntry::Error(_))),
        "finish-connect after a peer reset does not trap"
    );

    let sent: Result<(), String> = session
        .call(
            "p2_tcp_send",
            data_value!("other".to_string(), b"still-here\n".to_vec()),
        )
        .await?;
    assert_eq!(sent, Ok(()), "the other connection keeps working");
    other
        .wait_until("the other peer gets the bytes", |connections| {
            connections.len() == 1 && connections[0].received == b"still-here\n"
        })
        .await?;
    session.finish().await
}

fn no_error_entry(oplog: &[PublicOplogEntry]) -> bool {
    !oplog
        .iter()
        .any(|entry| matches!(entry, PublicOplogEntry::Error(_)))
}

/// A `finish-connect` whose connect started in an earlier Store completes with `connection-reset`,
/// and that result replays: a further restart records no new `finish-connect` and does not
/// connect.
#[test]
#[timeout("3m")]
async fn p2_tcp_connect_started_before_a_restart_finishes_with_connection_reset(
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
        "p2-connect-across-restart",
    )
    .await?;
    let started: Result<(), String> = session
        .call(
            "p2_tcp_start_connect",
            data_value!("c".to_string(), peer.port),
        )
        .await?;
    assert_eq!(started, Ok(()));
    peer.wait_until("one connection", |connections| connections.len() == 1)
        .await?;

    session.restart_executor().await?;
    let finished: Result<(), String> = session
        .call("p2_tcp_finish_connect", data_value!("c".to_string()))
        .await?;
    assert!(
        finished
            .as_ref()
            .is_err_and(|error| error.contains("connection-reset")),
        "expected connection-reset, got {finished:?}"
    );
    assert!(no_error_entry(&session.oplog().await?));
    let finishes = session.starts("sockets::tcp::finish_connect").await?;

    session.restart_executor().await?;
    let log: Vec<(String, Vec<u8>)> = session.call("received", data_value!()).await?;
    assert!(log.is_empty());
    assert_eq!(
        session.starts("sockets::tcp::finish_connect").await?,
        finishes,
        "replay records no new finish-connect"
    );
    assert_eq!(peer.connections().len(), 1, "replay does not connect");
    assert!(no_error_entry(&session.oplog().await?));
    session.finish().await
}

/// The first operation on a stand-in stream of a replayed connection fails with
/// `connection-reset`, and the next ones report `closed`. A further restart replays that sequence
/// and a later operation still reports `closed`.
#[test]
#[timeout("3m")]
async fn p2_tcp_lost_stream_reports_closed_after_its_first_error(
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
        "p2-lost-stream-closed",
    )
    .await?;
    let connected: Result<(), String> = session
        .call("p2_tcp_connect", data_value!("a".to_string(), peer.port))
        .await?;
    assert_eq!(connected, Ok(()));

    session.restart_executor().await?;
    let mut results = Vec::new();
    for _ in 0..2 {
        let sent: Result<(), String> = session
            .call("p2_tcp_send", data_value!("a".to_string(), b"x".to_vec()))
            .await?;
        results.push(sent);
    }
    assert_eq!(
        results,
        vec![
            Err("last-operation-failed: connection reset".to_string()),
            Err("closed".to_string())
        ]
    );

    session.restart_executor().await?;
    let third: Result<(), String> = session
        .call("p2_tcp_send", data_value!("a".to_string(), b"x".to_vec()))
        .await?;
    assert_eq!(third, Err("closed".to_string()));
    assert!(no_error_entry(&session.oplog().await?));
    assert_eq!(peer.connections().len(), 1, "replay does not connect");
    session.finish().await
}

/// `start-bind` records the selected address and replay binds nothing. After a restart, the
/// local addresses of a finished and of a pending bind do not change, `finish-bind` of the
/// replayed pending bind succeeds once, a replayed bound socket refuses `start-connect` with
/// `connection-reset`, and a replayed pending bind refuses it with `concurrency-conflict`. The
/// peer sees no connection, and a further restart replays all of it.
#[test]
#[timeout("3m")]
async fn p2_tcp_bind_replays_and_reconstructed_states_refuse_connect(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let peer = TcpPeer::start(b"", Reply::Silent).await?;
    let mut session =
        Session::start(deps, &context, host_api_tests, sockets(), "p2-bind-replay").await?;
    let mut bound = Vec::new();
    for (slot, finish) in [("done", true), ("pending", false), ("pending2", false)] {
        let address: Result<String, String> = session
            .call("p2_tcp_bind", data_value!(slot.to_string(), finish))
            .await?;
        bound.push((slot, address.map_err(anyhow::Error::msg)?));
    }

    session.restart_executor().await?;
    for (slot, expected) in &bound {
        let after: Result<String, String> = session
            .call("p2_tcp_bound_local_address", data_value!(slot.to_string()))
            .await?;
        assert_eq!(after.as_ref(), Ok(expected), "local address of {slot}");
    }
    let finish_bind: Result<(), String> = session
        .call("p2_tcp_finish_bind", data_value!("pending".to_string()))
        .await?;
    assert_eq!(
        finish_bind,
        Ok(()),
        "finish-bind of a replayed pending bind"
    );
    let again: Result<(), String> = session
        .call("p2_tcp_finish_bind", data_value!("pending".to_string()))
        .await?;
    assert!(
        again
            .as_ref()
            .is_err_and(|error| error.contains("not-in-progress")),
        "a second finish-bind: {again:?}"
    );

    let connect_done: Result<(), String> = session
        .call(
            "p2_tcp_connect_bound",
            data_value!("done".to_string(), peer.port),
        )
        .await?;
    assert!(
        connect_done
            .as_ref()
            .is_err_and(|error| error.contains("connection-reset")),
        "connect on a replayed bound socket: {connect_done:?}"
    );
    let connect_pending: Result<(), String> = session
        .call(
            "p2_tcp_connect_bound",
            data_value!("pending2".to_string(), peer.port),
        )
        .await?;
    assert!(
        connect_pending
            .as_ref()
            .is_err_and(|error| error.contains("concurrency-conflict")),
        "connect on a replayed pending bind: {connect_pending:?}"
    );
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(peer.connections().len(), 0, "no native connect");

    session.restart_executor().await?;
    let log: Vec<(String, Vec<u8>)> = session.call("received", data_value!()).await?;
    assert!(log.is_empty());
    assert!(no_error_entry(&session.oplog().await?));
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
            data_value!("u".to_string(), Some("8.8.8.8:12345".to_string())),
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

async fn local_address(session: &Session<'_>, slot: &str) -> anyhow::Result<String> {
    session
        .call::<Result<String, String>>("p3_udp_local_address", data_value!(slot.to_string()))
        .await?
        .map_err(anyhow::Error::msg)
}

async fn send_to(
    session: &Session<'_>,
    slot: &str,
    port: Option<u16>,
    data: &[u8],
) -> anyhow::Result<()> {
    session
        .call::<Result<(), String>>(
            "p3_udp_send",
            data_value!(slot.to_string(), port, data.to_vec()),
        )
        .await?
        .map_err(anyhow::Error::msg)
}

/// A disconnect after an implicit bind releases the native port, so the entry no longer
/// reports it. The next live send binds again on the unspecified address and records that
/// address, and the getter reports it, live and after replay. The socket address policy allows a
/// fixed port only on a loopback address, so the rebuild of that unconnected endpoint is refused
/// and the socket is lost.
#[test]
#[timeout("3m")]
async fn p3_udp_disconnect_after_an_implicit_bind_records_the_next_endpoint(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let peer = UdpPeer::start().await?;
    let mut session =
        Session::start(deps, &context, host_api_tests, sockets(), "udp-impl-disc").await?;
    let created: Result<(), String> = session
        .call("p3_udp_create", data_value!("u".to_string()))
        .await?;
    assert_eq!(created, Ok(()));
    let connected: Result<(), String> = session
        .call("p3_udp_connect", data_value!("u".to_string(), peer.port()))
        .await?;
    assert_eq!(connected, Ok(()));
    let disconnected: Result<(), String> = session
        .call("p3_udp_disconnect", data_value!("u".to_string()))
        .await?;
    assert_eq!(disconnected, Ok(()));
    assert_eq!(local_address(&session, "u").await?, "0.0.0.0:0");

    send_to(&session, "u", Some(peer.port()), b"live").await?;
    let (data, source) = peer.receive().await?;
    assert_eq!(data, b"live".to_vec());
    let local = local_address(&session, "u").await?;
    assert_eq!(local, format!("0.0.0.0:{}", source.port()));

    session.restart_executor().await?;
    assert_eq!(local_address(&session, "u").await?, local);
    let sent: Result<(), String> = session
        .call(
            "p3_udp_send",
            data_value!("u".to_string(), Some(peer.port()), b"replayed".to_vec()),
        )
        .await?;
    assert_eq!(sent, Err("ErrorCode::ConnectionBroken".to_string()));
    session.finish().await
}

/// A connect selects the interface address of a wildcard bind, also when a send bound the
/// socket implicitly. The entry records that address, and the rebuilt socket sends from it.
#[test]
#[timeout("3m")]
async fn p3_udp_connect_after_a_wildcard_bind_records_the_interface_address(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let peer = UdpPeer::start().await?;
    let mut session =
        Session::start(deps, &context, host_api_tests, sockets(), "udp-wildcard").await?;

    let bound: Result<(), String> = session
        .call("p3_udp_bind_any", data_value!("b".to_string(), 0u16))
        .await?;
    assert_eq!(bound, Ok(()));
    let created: Result<(), String> = session
        .call("p3_udp_create", data_value!("s".to_string()))
        .await?;
    assert_eq!(created, Ok(()));
    send_to(&session, "s", Some(peer.port()), b"first").await?;
    peer.receive().await?;

    let mut expected = Vec::new();
    for slot in ["b", "s"] {
        let wildcard = local_address(&session, slot).await?;
        let port = wildcard
            .strip_prefix("0.0.0.0:")
            .expect("wildcard address")
            .to_string();
        let connected: Result<(), String> = session
            .call("p3_udp_connect", data_value!(slot.to_string(), peer.port()))
            .await?;
        assert_eq!(connected, Ok(()));
        let local = local_address(&session, slot).await?;
        assert_eq!(local, format!("127.0.0.1:{port}"));
        expected.push(local);
    }

    session.restart_executor().await?;
    for (slot, local) in ["b", "s"].into_iter().zip(expected) {
        assert_eq!(local_address(&session, slot).await?, local);
        send_to(&session, slot, None, slot.as_bytes()).await?;
        let (data, source) = peer.receive().await?;
        assert_eq!(
            (data, source.to_string()),
            (slot.as_bytes().to_vec(), local)
        );
    }
    session.finish().await
}

/// On Linux a UDP connect that fails with `EINVAL` dissolves the old association before it tries
/// again, so the failed connect leaves the socket disconnected. The getter and a send to the
/// connected peer then fail, live and after reconstruction, and the rebuild does not connect the
/// socket to the old peer again.
#[test]
#[timeout("3m")]
async fn p3_udp_failed_connect_that_disconnects_replays_as_disconnected(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let peer = UdpPeer::start().await?;
    let other = UdpPeer::start().await?;
    let mut session = Session::start(
        deps,
        &context,
        host_api_tests,
        sockets(),
        "udp-failed-connect",
    )
    .await?;
    let local = free_udp_address().await?;
    let bound: Result<(), String> = session
        .call("p3_udp_bind", data_value!("u".to_string(), local.port()))
        .await?;
    assert_eq!(bound, Ok(()));
    let connected: Result<(), String> = session
        .call("p3_udp_connect", data_value!("u".to_string(), peer.port()))
        .await?;
    assert_eq!(connected, Ok(()));
    let failed: Result<(), String> = session
        .call(
            "p3_udp_connect_ipv4",
            data_value!("u".to_string(), "8.8.8.8:9".to_string()),
        )
        .await?;
    assert_eq!(failed, Err("ErrorCode::InvalidArgument".to_string()));

    let disconnected = (
        Err::<String, String>("ErrorCode::InvalidState".to_string()),
        Err::<(), String>("ErrorCode::InvalidArgument".to_string()),
    );
    let live = (
        session
            .call::<Result<String, String>>("p3_udp_remote_address", data_value!("u".to_string()))
            .await?,
        session
            .call::<Result<(), String>>(
                "p3_udp_send",
                data_value!("u".to_string(), None::<u16>, b"live".to_vec()),
            )
            .await?,
    );
    assert_eq!(live, disconnected);

    session.restart_executor().await?;
    let replayed = (
        session
            .call::<Result<String, String>>("p3_udp_remote_address", data_value!("u".to_string()))
            .await?,
        session
            .call::<Result<(), String>>(
                "p3_udp_send",
                data_value!("u".to_string(), None::<u16>, b"replayed".to_vec()),
            )
            .await?,
    );
    assert_eq!(replayed, disconnected);
    send_to(&session, "u", Some(other.port()), b"free").await?;
    assert_eq!(other.receive().await?, (b"free".to_vec(), local));
    session.finish().await
}

/// The endpoint rebuild binds the native socket on the recorded interface address and port, so
/// Linux keeps both on a later disconnect, unlike the implicit bind or the wildcard bind on port 0
/// that selected them. The entry records the rebuild and keeps them too: the getter reports the
/// address the peer sees after the disconnect, live and after a second reconstruction.
#[test]
#[timeout("3m")]
async fn p3_udp_disconnect_after_an_endpoint_rebuild_keeps_the_rebuilt_port(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let peer = UdpPeer::start().await?;
    let other = UdpPeer::start().await?;
    let mut session = Session::start(
        deps,
        &context,
        host_api_tests,
        sockets(),
        "udp-rebuilt-disc",
    )
    .await?;

    let created: Result<(), String> = session
        .call("p3_udp_create", data_value!("i".to_string()))
        .await?;
    assert_eq!(created, Ok(()));
    let bound: Result<(), String> = session
        .call("p3_udp_bind_any", data_value!("w".to_string(), 0u16))
        .await?;
    assert_eq!(bound, Ok(()));
    let mut ports = Vec::new();
    for slot in ["i", "w"] {
        let connected: Result<(), String> = session
            .call("p3_udp_connect", data_value!(slot.to_string(), peer.port()))
            .await?;
        assert_eq!(connected, Ok(()));
        let local = local_address(&session, slot).await?;
        let port = local
            .strip_prefix("127.0.0.1:")
            .expect("interface address")
            .to_string();
        ports.push(port);
    }

    session.restart_executor().await?;
    for (slot, port) in ["i", "w"].into_iter().zip(&ports) {
        send_to(&session, slot, None, slot.as_bytes()).await?;
        let (_, source) = peer.receive().await?;
        assert_eq!(source.to_string(), format!("127.0.0.1:{port}"));
        let disconnected: Result<(), String> = session
            .call("p3_udp_disconnect", data_value!(slot.to_string()))
            .await?;
        assert_eq!(disconnected, Ok(()));
        assert_eq!(
            local_address(&session, slot).await?,
            format!("127.0.0.1:{port}")
        );
        send_to(&session, slot, Some(other.port()), b"live").await?;
        let (_, source) = other.receive().await?;
        assert_eq!(source.to_string(), format!("127.0.0.1:{port}"));
    }

    session.restart_executor().await?;
    for (slot, port) in ["i", "w"].into_iter().zip(&ports) {
        assert_eq!(
            local_address(&session, slot).await?,
            format!("127.0.0.1:{port}")
        );
        send_to(&session, slot, Some(other.port()), b"replayed").await?;
        let (_, source) = other.receive().await?;
        assert_eq!(source.to_string(), format!("127.0.0.1:{port}"));
    }
    session.finish().await
}

/// P2 disconnects with a live `stream(none)`, which records the native local address it leaves.
/// After an endpoint rebuild on the recorded interface address and port, the native socket keeps
/// both on the disconnect, and the entry records them, live and after replay.
#[test]
#[timeout("3m")]
async fn p2_udp_disconnect_after_an_endpoint_rebuild_keeps_the_rebuilt_port(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let peer = UdpPeer::start().await?;
    let other = UdpPeer::start().await?;
    let mut session =
        Session::start(deps, &context, host_api_tests, sockets(), "p2-udp-rebuilt").await?;
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
    let port = local
        .strip_prefix("127.0.0.1:")
        .expect("interface address")
        .to_string();

    session.restart_executor().await?;
    let sent: Result<u64, String> = session
        .call(
            "p2_udp_send_to",
            data_value!("u".to_string(), None::<u16>, b"rebuilt".to_vec()),
        )
        .await?;
    assert_eq!(sent, Ok(1));
    assert_eq!(peer.receive().await?.1.to_string(), local);
    let restreamed: Result<(), String> = session
        .call("p2_udp_restream", data_value!("u".to_string(), None::<u16>))
        .await?;
    assert_eq!(restreamed, Ok(()));
    let after_disconnect: Result<String, String> = session
        .call("p2_udp_local_address", data_value!("u".to_string()))
        .await?;
    assert_eq!(after_disconnect, Ok(format!("127.0.0.1:{port}")));
    let sent: Result<u64, String> = session
        .call(
            "p2_udp_send_to",
            data_value!("u".to_string(), Some(other.port()), b"live".to_vec()),
        )
        .await?;
    assert_eq!(sent, Ok(1));
    assert_eq!(other.receive().await?.1.to_string(), local);

    session.restart_executor().await?;
    let replayed: Result<String, String> = session
        .call("p2_udp_local_address", data_value!("u".to_string()))
        .await?;
    assert_eq!(replayed, Ok(format!("127.0.0.1:{port}")));
    let sent: Result<u64, String> = session
        .call(
            "p2_udp_send_to",
            data_value!("u".to_string(), Some(other.port()), b"again".to_vec()),
        )
        .await?;
    assert_eq!(sent, Ok(1));
    assert_eq!(other.receive().await?.1.to_string(), local);
    session.finish().await
}

/// A disconnect releases a port that a bind on port 0 selected, so the entry keeps the bind
/// address with port 0. A rebuild then selects a new port at the first live operation, here
/// `check-send`, which records it, so the getter and the source the peer sees agree, live and
/// after replay.
#[test]
#[timeout("3m")]
async fn p2_udp_check_send_records_the_port_a_rebuild_on_port_zero_selects(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let peer = UdpPeer::start().await?;
    let other = UdpPeer::start().await?;
    let mut session =
        Session::start(deps, &context, host_api_tests, sockets(), "p2-udp-zero").await?;
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
    let released: Result<String, String> = session
        .call("p2_udp_local_address", data_value!("u".to_string()))
        .await?;
    assert_eq!(released, Ok("127.0.0.1:0".to_string()));

    session.restart_executor().await?;
    let permit: Result<u64, String> = session
        .call("p2_udp_check_send", data_value!("u".to_string()))
        .await?;
    assert!(permit.is_ok(), "{permit:?}");
    let local: String = session
        .call::<Result<String, String>>("p2_udp_local_address", data_value!("u".to_string()))
        .await?
        .map_err(anyhow::Error::msg)?;
    assert!(
        local.starts_with("127.0.0.1:") && !local.ends_with(":0"),
        "{local}"
    );
    let sent: Result<u64, String> = session
        .call(
            "p2_udp_send_to",
            data_value!("u".to_string(), Some(other.port()), b"one".to_vec()),
        )
        .await?;
    assert_eq!(sent, Ok(1));
    assert_eq!(other.receive().await?.1.to_string(), local);

    session.restart_executor().await?;
    let replayed: Result<String, String> = session
        .call("p2_udp_local_address", data_value!("u".to_string()))
        .await?;
    assert_eq!(replayed, Ok(local.clone()));
    let sent: Result<u64, String> = session
        .call(
            "p2_udp_send_to",
            data_value!("u".to_string(), Some(other.port()), b"two".to_vec()),
        )
        .await?;
    assert_eq!(sent, Ok(1));
    assert_eq!(other.receive().await?.1.to_string(), local);
    session.finish().await
}

/// The rebuild that a live receive performs binds the recorded port, and Linux keeps that port on
/// a later disconnect. The guest drops the receive before it completes, so only the disconnect can
/// record the port: the getter and the source of the next datagram agree with the native socket,
/// live and after a second reconstruction.
#[test]
#[timeout("3m")]
async fn p3_udp_disconnect_after_an_abandoned_receive_keeps_the_rebuilt_port(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let peer = UdpPeer::start().await?;
    let other = UdpPeer::start().await?;
    let mut session =
        Session::start(deps, &context, host_api_tests, sockets(), "udp-abandoned").await?;
    let created: Result<(), String> = session
        .call("p3_udp_create", data_value!("u".to_string()))
        .await?;
    assert_eq!(created, Ok(()));
    let connected: Result<(), String> = session
        .call("p3_udp_connect", data_value!("u".to_string(), peer.port()))
        .await?;
    assert_eq!(connected, Ok(()));
    let local = local_address(&session, "u").await?;
    assert!(local.starts_with("127.0.0.1:"), "{local}");

    session.restart_executor().await?;
    let abandoned: Result<bool, String> = session
        .call(
            "p3_udp_receive_abandoned",
            data_value!("u".to_string(), 200u64),
        )
        .await?;
    assert_eq!(abandoned, Ok(true));
    // The rebuild bound the recorded port before the guest dropped the receive.
    let taken = std::net::UdpSocket::bind(local.as_str());
    assert_eq!(
        taken.map(|_| ()).map_err(|error| error.kind()),
        Err(std::io::ErrorKind::AddrInUse)
    );
    let disconnected: Result<(), String> = session
        .call("p3_udp_disconnect", data_value!("u".to_string()))
        .await?;
    assert_eq!(disconnected, Ok(()));
    assert_eq!(local_address(&session, "u").await?, local);
    send_to(&session, "u", Some(other.port()), b"live").await?;
    assert_eq!(other.receive().await?.1.to_string(), local);

    session.restart_executor().await?;
    assert_eq!(local_address(&session, "u").await?, local);
    send_to(&session, "u", Some(other.port()), b"replayed").await?;
    assert_eq!(other.receive().await?.1.to_string(), local);
    session.finish().await
}

/// A disconnect after an implicit bind releases the port, and the socket stays bound without one:
/// a receive waits. The rebuild after a reconstruction restores that state instead of leaving an
/// unbound socket, whose receive fails with `invalid-state`.
#[test]
#[timeout("3m")]
async fn p3_udp_receive_after_a_released_disconnect_waits_after_replay(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let peer = UdpPeer::start().await?;
    let mut session =
        Session::start(deps, &context, host_api_tests, sockets(), "udp-released").await?;
    let created: Result<(), String> = session
        .call("p3_udp_create", data_value!("u".to_string()))
        .await?;
    assert_eq!(created, Ok(()));
    let connected: Result<(), String> = session
        .call("p3_udp_connect", data_value!("u".to_string(), peer.port()))
        .await?;
    assert_eq!(connected, Ok(()));
    let disconnected: Result<(), String> = session
        .call("p3_udp_disconnect", data_value!("u".to_string()))
        .await?;
    assert_eq!(disconnected, Ok(()));
    assert_eq!(local_address(&session, "u").await?, "0.0.0.0:0");
    let live: Result<bool, String> = session
        .call(
            "p3_udp_receive_abandoned",
            data_value!("u".to_string(), 200u64),
        )
        .await?;
    assert_eq!(live, Ok(true));

    session.restart_executor().await?;
    let replayed: Result<bool, String> = session
        .call(
            "p3_udp_receive_abandoned",
            data_value!("u".to_string(), 200u64),
        )
        .await?;
    assert_eq!(replayed, Ok(true));
    send_to(&session, "u", Some(peer.port()), b"after").await?;
    let (data, source) = peer.receive().await?;
    assert_eq!(data, b"after".to_vec());
    assert_eq!(
        local_address(&session, "u").await?,
        format!("0.0.0.0:{}", source.port())
    );
    session.finish().await
}

/// A disconnect releases the port of a socket bound on a loopback address with port 0 and keeps
/// the address. The endpoint rebuild restores both: after a reconstruction a receive waits, a
/// send leaves from the loopback address on a new port, and a connect to a remote address fails
/// from the loopback source as it does live.
#[test]
#[timeout("3m")]
async fn p3_udp_released_loopback_bind_keeps_its_address_after_replay(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let peer = UdpPeer::start().await?;
    let mut session = Session::start(
        deps,
        &context,
        host_api_tests,
        sockets(),
        "udp-released-loopback",
    )
    .await?;
    for slot in ["live", "rebuilt"] {
        let bound: Result<(), String> = session
            .call("p3_udp_bind", data_value!(slot.to_string(), 0u16))
            .await?;
        assert_eq!(bound, Ok(()));
        let connected: Result<(), String> = session
            .call("p3_udp_connect", data_value!(slot.to_string(), peer.port()))
            .await?;
        assert_eq!(connected, Ok(()));
        let disconnected: Result<(), String> = session
            .call("p3_udp_disconnect", data_value!(slot.to_string()))
            .await?;
        assert_eq!(disconnected, Ok(()));
        assert_eq!(local_address(&session, slot).await?, "127.0.0.1:0");
    }
    let live = session
        .call::<Result<(), String>>(
            "p3_udp_connect_ipv4",
            data_value!("live".to_string(), "8.8.8.8:9".to_string()),
        )
        .await?;
    assert_eq!(live, Err("ErrorCode::InvalidArgument".to_string()));

    session.restart_executor().await?;
    let waits: Result<bool, String> = session
        .call(
            "p3_udp_receive_abandoned",
            data_value!("rebuilt".to_string(), 200u64),
        )
        .await?;
    assert_eq!(waits, Ok(true));
    assert_eq!(local_address(&session, "rebuilt").await?, "127.0.0.1:0");
    let replayed = session
        .call::<Result<(), String>>(
            "p3_udp_connect_ipv4",
            data_value!("rebuilt".to_string(), "8.8.8.8:9".to_string()),
        )
        .await?;
    assert_eq!(replayed, live);
    send_to(&session, "rebuilt", Some(peer.port()), b"after").await?;
    let (data, source) = peer.receive().await?;
    assert_eq!(data, b"after".to_vec());
    assert_eq!(source.ip().to_string(), "127.0.0.1");
    assert_eq!(
        local_address(&session, "rebuilt").await?,
        source.to_string()
    );
    session.finish().await
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
