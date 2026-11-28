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

use crate::Tracing;
use anyhow::anyhow;
use futures::stream::{SplitSink, SplitStream};
use futures::{SinkExt, StreamExt};
use golem_common::model::agent::ParsedAgentKey;
use golem_common::model::component::ComponentDto;
use golem_common::model::oplog::payload::HostResponseWebsocketReceiveResponse;
use golem_common::model::oplog::payload::types::SerializableWebsocketError;
use golem_common::model::oplog::{
    OplogIndex, PublicAgentInvocation, PublicOplogEntry, PublicOplogEntryWithIndex,
};
use golem_common::model::{AgentStatus, IdempotencyKey, OwnedAgentId, PromiseId};
use golem_common::schema::{FromSchema, SchemaValue, TypedSchemaValue};
use golem_common::{agent_key, data_value};
use golem_test_framework::dsl::{
    GATE_TIMEOUT, SETTLE_TIMEOUT, TestDsl, WorkerLogEventStream, within,
};
use golem_worker_executor_test_utils::{
    AgentInitializationEnqueueGateHandle, LastUniqueId, OplogView, PrecompiledComponent,
    ReplayAdmissionStage, Teardown, TestContext, TestExecutorOverrides, TestWorkerExecutor,
    WorkerExecutorTestDependencies, start, start_with_overrides,
};
use pretty_assertions::assert_eq;
use std::collections::HashMap;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};
use std::time::{Duration, Instant};
use test_r::{inherit_test_dep, test, timeout};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::spawn;
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;
use tracing::Instrument;

inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(LastUniqueId);
inherit_test_dep!(Tracing);
inherit_test_dep!(
    #[tagged_as("host_api_tests")]
    PrecompiledComponent
);
inherit_test_dep!(
    #[tagged_as("agent_sdk_ts")]
    PrecompiledComponent
);
inherit_test_dep!(
    #[tagged_as("tool_streaming_effect_caller")]
    PrecompiledComponent
);

/// How long a test waits for a websocket pool permit that no call is expected to hold.
const POOL_PERMIT_TIMEOUT: Duration = Duration::from_secs(5);
/// How long a test polls the oplog for the calls of an invocation it started.
const OPLOG_PROGRESS_TIMEOUT: Duration = Duration::from_secs(10);
/// How long an invocation may take that must not wait for the pool permit the test holds.
const UNBLOCKED_INVOCATION_TIMEOUT: Duration = Duration::from_secs(10);
/// How long a test waits for a test peer to accept connections or observe frames.
const PEER_STATE_TIMEOUT: Duration = Duration::from_secs(10);
/// How long a test waits for an agent to reach the status that an interrupt or a parked call
/// leads to.
const STATUS_TIMEOUT: Duration = Duration::from_secs(10);
/// How long a test waits for an agent to finish its invocations and become idle.
const IDLE_TIMEOUT: Duration = Duration::from_secs(20);
/// How long a test waits for the echo server to reach a connection and message count.
const SERVER_STATE_TIMEOUT: Duration = Duration::from_secs(5);
/// How long a test lets calls park or queue inside the executor, where neither the peer nor
/// the oplog can observe them.
const QUEUE_SETTLE: Duration = Duration::from_millis(500);
/// How long a test lets a restarted executor run before it checks that nothing reconnected.
const RESTART_SETTLE: Duration = Duration::from_millis(100);

#[derive(Debug, Clone, PartialEq, Eq)]
struct ServerEvent {
    connection: usize,
    payload: String,
}

/// The context, executor and stored component that the tests in this file start with.
struct WebsocketTest {
    context: TestContext,
    executor: TestWorkerExecutor,
    component: ComponentDto,
}

impl WebsocketTest {
    /// A fresh context, an executor with the default configuration and the stored component.
    async fn start(
        deps: &WorkerExecutorTestDependencies,
        last_unique_id: &LastUniqueId,
        component: &PrecompiledComponent,
    ) -> anyhow::Result<Self> {
        let context = TestContext::new(last_unique_id);
        let executor = start(deps, &context).await?;
        Self::with_component(context, executor, component).await
    }

    /// As [`WebsocketTest::start`], with at most `limit` connections in the executor's
    /// websocket pool.
    async fn start_with_connection_limit(
        deps: &WorkerExecutorTestDependencies,
        last_unique_id: &LastUniqueId,
        component: &PrecompiledComponent,
        limit: usize,
    ) -> anyhow::Result<Self> {
        let context = TestContext::new(last_unique_id);
        let executor = start_with_overrides(deps, &context, connection_limit(limit)).await?;
        Self::with_component(context, executor, component).await
    }

    async fn with_component(
        context: TestContext,
        executor: TestWorkerExecutor,
        component: &PrecompiledComponent,
    ) -> anyhow::Result<Self> {
        let component = executor
            .component_dep(&context.default_environment_id, component)
            .store()
            .await?;
        Ok(Self {
            context,
            executor,
            component,
        })
    }
}

/// Executor overrides that allow at most `limit` connections in the websocket pool.
fn connection_limit(limit: usize) -> TestExecutorOverrides {
    TestExecutorOverrides::configured(move |config| config.max_websocket_connections = limit)
}

/// How a [`TestPeer`] accepts connections.
#[derive(Debug, Clone, Copy)]
enum Accept {
    /// Accepts one connection, serves it and stops listening.
    Once,
    /// Accepts connections one after the other and serves each before the next accept.
    Serially,
    /// Accepts connections in a loop and serves each in its own task.
    Concurrently,
}

/// What a [`TestPeer`] does on each connection after the handshake.
#[derive(Debug, Clone, Copy)]
enum PeerScript {
    /// Records each text frame in the transcript and echoes text and binary frames until Close.
    Echo,
    /// Reads frames until Close and sends nothing.
    Drain,
    /// Waits for `delay`, sends one text frame and closes the connection.
    Send { text: &'static str, delay: Duration },
    /// Sends one text frame and then a Close frame.
    SendThenClose(&'static str),
    /// Sends one text frame and then reads frames until Close.
    SendThenDrain(&'static str),
}

/// An in-process websocket server for tests that need a plain peer. It counts accepted
/// connections and records the text frames that [`PeerScript::Echo`] receives. Dropping it
/// stops the accept loop.
struct TestPeer {
    port: u16,
    accepted: Arc<AtomicUsize>,
    transcript: Arc<Mutex<Vec<ServerEvent>>>,
    accept_loop: JoinHandle<()>,
}

impl TestPeer {
    async fn start(accept: Accept, script: PeerScript) -> Self {
        let listener = TcpListener::bind("0.0.0.0:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let accepted = Arc::new(AtomicUsize::new(0));
        let transcript = Arc::new(Mutex::new(Vec::new()));
        let accept_loop = spawn({
            let accepted = Arc::clone(&accepted);
            let transcript = Arc::clone(&transcript);
            async move {
                while let Ok((stream, _)) = listener.accept().await {
                    let connection = accepted.fetch_add(1, Ordering::SeqCst);
                    let serve =
                        serve_test_peer_connection(stream, connection, script, transcript.clone());
                    match accept {
                        Accept::Once => {
                            serve.await;
                            break;
                        }
                        Accept::Serially => serve.await,
                        Accept::Concurrently => {
                            spawn(serve.in_current_span());
                        }
                    }
                }
            }
            .in_current_span()
        });
        Self {
            port,
            accepted,
            transcript,
            accept_loop,
        }
    }

    fn url(&self) -> String {
        format!("ws://localhost:{}", self.port)
    }

    /// The `WS_PORT` environment variable that points an agent at this peer.
    fn port_env(&self) -> HashMap<String, String> {
        HashMap::from([("WS_PORT".to_string(), self.port.to_string())])
    }

    fn accepted(&self) -> usize {
        self.accepted.load(Ordering::SeqCst)
    }

    fn transcript(&self) -> Vec<ServerEvent> {
        self.transcript.lock().unwrap().clone()
    }

    /// Stops accepting connections. Connections that are already served stay open.
    fn abort(&self) {
        self.accept_loop.abort();
    }
}

impl Drop for TestPeer {
    fn drop(&mut self) {
        self.accept_loop.abort();
    }
}

async fn serve_test_peer_connection(
    stream: TcpStream,
    connection: usize,
    script: PeerScript,
    transcript: Arc<Mutex<Vec<ServerEvent>>>,
) {
    let ws_stream = tokio_tungstenite::accept_async(stream)
        .await
        .expect("WS handshake failed");
    let (mut write, mut read) = StreamExt::split(ws_stream);
    match script {
        PeerScript::Echo => {
            while let Some(Ok(msg)) = read.next().await {
                if msg.is_close() {
                    break;
                }
                if msg.is_text() {
                    let payload = msg
                        .to_text()
                        .expect("text message should decode")
                        .to_string();
                    transcript.lock().unwrap().push(ServerEvent {
                        connection,
                        payload,
                    });
                }
                if msg.is_text() || msg.is_binary() {
                    write.send(msg).await.ok();
                }
            }
        }
        PeerScript::Drain => drain_until_close(&mut read).await,
        PeerScript::Send { text, delay } => {
            if !delay.is_zero() {
                tokio::time::sleep(delay).await;
            }
            write.send(Message::text(text)).await.ok();
        }
        PeerScript::SendThenClose(text) => {
            write.send(Message::text(text)).await.ok();
            write.send(Message::Close(None)).await.ok();
        }
        PeerScript::SendThenDrain(text) => {
            write.send(Message::text(text)).await.ok();
            drain_until_close(&mut read).await;
        }
    }
}

async fn drain_until_close(read: &mut SplitStream<WsServerStream>) {
    while let Some(Ok(msg)) = read.next().await {
        if msg.is_close() {
            break;
        }
    }
}

/// Asserts that every operation on the agent's lost report-loss handle reports `SessionLost`.
async fn assert_reports_session_loss(
    executor: &TestWorkerExecutor,
    component: &ComponentDto,
    agent_id: &ParsedAgentKey,
) -> anyhow::Result<()> {
    let reported: (bool, bool, bool, bool) = executor
        .invoke_and_await_typed(component, agent_id, "probe_report_loss", data_value!())
        .await?;
    assert_eq!(
        reported,
        (true, true, true, true),
        "receive, receive-with-timeout, send and close must each report SessionLost"
    );
    Ok(())
}

/// Crashes the executor while the gated call waits at its End gate: the gated call and every
/// other call that has not finished stay as the crash leaves them.
async fn crash_at_call_end(
    executor: &TestWorkerExecutor,
    gate: &AgentInitializationEnqueueGateHandle,
) -> anyhow::Result<()> {
    gate.abort();
    executor.shutdown_and_wait_for_invocation_loops().await
}

/// The function name of the `Start` entry at `index`, if there is one.
fn start_function(oplog: &[PublicOplogEntryWithIndex], index: OplogIndex) -> Option<&str> {
    oplog.iter().find_map(|entry| match &entry.entry {
        PublicOplogEntry::Start(params) if entry.oplog_index == index => {
            Some(params.function_name.as_str())
        }
        _ => None,
    })
}

/// A `WebsocketTest` agent of the stored component, with its worker.
struct WebsocketAgent {
    component: ComponentDto,
    agent_id: ParsedAgentKey,
    worker_id: OwnedAgentId,
}

impl WebsocketAgent {
    async fn start(
        executor: &TestWorkerExecutor,
        component: &ComponentDto,
        name: &str,
    ) -> anyhow::Result<Self> {
        let agent_id = agent_key!("WebsocketTest", name);
        let worker_id = executor
            .start_agent(&component.id, agent_id.clone())
            .await?;
        Ok(Self {
            component: component.clone(),
            agent_id,
            worker_id,
        })
    }

    async fn oplog(
        &self,
        executor: &TestWorkerExecutor,
    ) -> anyhow::Result<Vec<PublicOplogEntryWithIndex>> {
        executor
            .get_oplog(&self.worker_id, OplogIndex::INITIAL)
            .await
    }

    /// Enqueues the concurrent `receive` and `receive-with-timeout` on the persisted handle.
    async fn start_contention(
        &self,
        executor: &TestWorkerExecutor,
        key: &IdempotencyKey,
    ) -> anyhow::Result<()> {
        executor
            .invoke_agent_with_key(
                &self.component,
                &self.agent_id,
                key,
                CONTENTION_METHOD,
                data_value!(CONTENTION_TIMEOUT_MS),
            )
            .await
    }

    /// The results of the contention invocation with `key`: `receive`, then
    /// `receive-with-timeout`.
    async fn contention_result(
        &self,
        executor: &TestWorkerExecutor,
        key: &IdempotencyKey,
    ) -> anyhow::Result<(String, String)> {
        executor
            .invoke_and_await_typed_with_key(
                &self.component,
                &self.agent_id,
                key,
                CONTENTION_METHOD,
                data_value!(CONTENTION_TIMEOUT_MS),
            )
            .await
    }
}

#[test]
#[tracing::instrument]
#[timeout("3m")]
async fn websocket_report_loss_guest_discards_completion_and_replay_stays_terminal(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let WebsocketTest {
        context,
        mut executor,
        component,
    } = WebsocketTest::start(deps, last_unique_id, host_api_tests).await?;
    let mut peer = ReconnectTestServer::start().await;
    let agent_id = agent_key!("WebsocketTest", "guest-discarded-loss");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;
    executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "connect_report_loss",
            data_value!(peer.url()),
        )
        .await?;
    let cancel_value = executor
        .invoke_and_await_agent(&component, &agent_id, "create_promise", data_value!())
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected promise return value"))?;
    let cancel = promise_id_from_value(&cancel_value);
    executor = executor.restart(Teardown::Shutdown).await?;
    executor
        .invoke_and_await_agent(&component, &agent_id, "noop", data_value!())
        .await?;

    // The receive reaches its loss End; the guest has not polled it yet.
    let mut end = executor.gate_next_call_end(&worker_id.agent_id, true).await;
    let mut output = executor.make_worker_log_event_stream(&worker_id).await?;
    let key = IdempotencyKey::fresh();
    executor
        .invoke_agent_with_key(
            &component,
            &agent_id,
            &key,
            "discard_report_loss",
            crate::raw_params(vec![cancel_value.clone()]),
        )
        .await?;
    within(
        GATE_TIMEOUT,
        "the End gate of the discarded receive",
        end.entered(),
    )
    .await?;
    let discarded_start = end.start_index();
    let prefix = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    assert_eq!(
        start_function(&prefix, discarded_start),
        Some(WEBSOCKET_RECEIVE_FUNCTION),
        "the gated End belongs to a receive: {}",
        prefix.describe()
    );

    // The guest retains the unpolled receive until its loss End is durable, then drops it.
    executor
        .complete_promise(&context.default_environment_id, &cancel, vec![1])
        .await?;
    output
        .wait_for_stdout("report-loss receive discarded\n", GATE_TIMEOUT)
        .await?;
    drop(end);
    let discarded: bool = executor
        .invoke_and_await_typed_with_key(
            &component,
            &agent_id,
            &key,
            "discard_report_loss",
            crate::raw_params(vec![cancel_value]),
        )
        .await?;
    assert!(discarded, "the guest discarded the receive");

    let mut original_discard = None;
    for replay in [false, true] {
        if replay {
            executor = executor.restart(Teardown::Shutdown).await?;
        }
        assert_reports_session_loss(&executor, &component, &agent_id).await?;

        // The receive settles once, as a session loss, and the guest discards that completion
        // instead of observing it.
        let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
        let receive = oplog.call(discarded_start);
        assert_eq!(
            (
                receive.ends.len(),
                receive.discards.len(),
                receive.deliveries.len(),
                receive.cancellations.len()
            ),
            (1, 1, 0, 0),
            "one End and one CompletionDiscarded, no delivery or cancellation: {}",
            oplog.describe()
        );
        assert!(
            receive.ends[0].oplog_index < receive.discards[0].oplog_index,
            "the End is durable before the discard"
        );
        let response: HostResponseWebsocketReceiveResponse = receive.end_response()?;
        assert_eq!(
            response.result,
            Err(SerializableWebsocketError::SessionLost)
        );

        // Replay keeps the original discard instead of recording another one.
        if let Some(original) = &original_discard {
            assert_eq!(*receive.discards[0], *original);
        } else {
            original_discard = Some(receive.discards[0].clone());
        }
        assert_eq!(
            (
                peer.accepted(),
                peer.completed_handshakes(),
                peer.frames_received()
            ),
            (1, 1, vec![COMPLETED_SEND_FRAME.to_string()]),
            "the lost handle opens no new connection and sends no new frame"
        );
    }
    drop(executor);
    peer.stop().await?;
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("3m")]
async fn websocket_report_loss_replacement_connect_crashes_before_end(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    // The connect has no End, so the retry connects again; it is crashed a second time before
    // the final retry connects a third time and continues live.
    ReplacementCrash {
        step: ReplacementStep::Connect,
        after_end: false,
        expected_result: Ok(RECONNECT_MESSAGES[0]),
        replacement_handshakes: 3,
    }
    .run(deps, last_unique_id, host_api_tests)
    .await
}

#[test]
#[tracing::instrument]
#[timeout("3m")]
async fn websocket_report_loss_replacement_connect_crashes_after_end(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    // The connect End is durable, so the reconstructed connection reports the session loss.
    ReplacementCrash {
        step: ReplacementStep::Connect,
        after_end: true,
        expected_result: Err(REPLACEMENT_SESSION_LOST),
        replacement_handshakes: 1,
    }
    .run(deps, last_unique_id, host_api_tests)
    .await
}

#[test]
#[tracing::instrument]
#[timeout("3m")]
async fn websocket_report_loss_replacement_initialization_crashes_before_end(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    // The initialization receive has no End, so its retry reports the session loss.
    ReplacementCrash {
        step: ReplacementStep::Initialization,
        after_end: false,
        expected_result: Err(REPLACEMENT_SESSION_LOST),
        replacement_handshakes: 1,
    }
    .run(deps, last_unique_id, host_api_tests)
    .await
}

#[test]
#[tracing::instrument]
#[timeout("3m")]
async fn websocket_report_loss_replacement_initialization_crashes_after_end(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    // The initialization receive End is durable, so replay returns the received greeting.
    ReplacementCrash {
        step: ReplacementStep::Initialization,
        after_end: true,
        expected_result: Ok(RECONNECT_FIRST_MESSAGE),
        replacement_handshakes: 1,
    }
    .run(deps, last_unique_id, host_api_tests)
    .await
}

/// The guest error of a replacement whose connect or initialization lost its session.
const REPLACEMENT_SESSION_LOST: &str = "Initialize error: Error::SessionLost";

/// The call of `replace_report_loss` at which the executor crashes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReplacementStep {
    /// The connect to the replacement peer.
    Connect,
    /// The receive that initializes the replacement connection.
    Initialization,
}

/// One crash window of an explicit replacement of a lost report-loss handle, and its
/// expected outcome.
struct ReplacementCrash {
    step: ReplacementStep,
    /// Crash after the End of the call is durable instead of before it.
    after_end: bool,
    expected_result: Result<&'static str, &'static str>,
    /// The handshakes the replacement peer completes over every attempt.
    replacement_handshakes: usize,
}

impl ReplacementCrash {
    async fn run(
        self,
        deps: &WorkerExecutorTestDependencies,
        last_unique_id: &LastUniqueId,
        host_api_tests: &PrecompiledComponent,
    ) -> anyhow::Result<()> {
        let initialization = self.step == ReplacementStep::Initialization;
        let after_end = self.after_end;
        let WebsocketTest {
            mut executor,
            component,
            ..
        } = WebsocketTest::start(deps, last_unique_id, host_api_tests).await?;
        let mut original = ReconnectTestServer::start().await;
        let mut replacement = ReconnectTestServer::start().await;
        let agent_id = agent_key!(
            "WebsocketTest",
            format!("replacement-{initialization}-{after_end}")
        );
        let worker_id = executor
            .start_agent(&component.id, agent_id.clone())
            .await?;
        executor
            .invoke_and_await_agent(
                &component,
                &agent_id,
                "connect_report_loss",
                data_value!(original.url()),
            )
            .await?;
        executor = executor.restart(Teardown::Shutdown).await?;
        assert_reports_session_loss(&executor, &component, &agent_id).await?;

        // Crash at the gated End of the replacement's connect or initialization receive.
        let key = IdempotencyKey::fresh();
        let mut gate = executor
            .gate_next_call_end(&worker_id.agent_id, initialization || after_end)
            .await;
        executor
            .invoke_agent_with_key(
                &component,
                &agent_id,
                &key,
                "replace_report_loss",
                data_value!(replacement.url()),
            )
            .await?;
        within(GATE_TIMEOUT, "the gated call End", gate.entered()).await?;
        if initialization {
            let next = executor
                .gate_next_call_end(&worker_id.agent_id, after_end)
                .await;
            drop(gate);
            gate = next;
            within(GATE_TIMEOUT, "the gated call End", gate.entered()).await?;
        }
        let target = gate.start_index();
        crash_at_call_end(&executor, &gate).await?;
        let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
        let function = if initialization {
            "::receive"
        } else {
            "::connect"
        };
        assert!(
            start_function(&oplog, target).is_some_and(|name| name.ends_with(function)),
            "the crashed call is the replacement's {function}: {}",
            oplog.describe()
        );
        assert_call_terminal_counts(&oplog, target, usize::from(after_end), 0);
        drop(gate);

        // Crash a fresh connect a second time while its original Start is still incomplete.
        if !initialization && !after_end {
            executor = executor.restart(Teardown::Drop).await?;
            let mut gate = executor
                .gate_next_call_end(&worker_id.agent_id, false)
                .await;
            executor
                .invoke_agent_with_key(
                    &component,
                    &agent_id,
                    &key,
                    "replace_report_loss",
                    data_value!(replacement.url()),
                )
                .await?;
            within(GATE_TIMEOUT, "the gated call End", gate.entered()).await?;
            assert_eq!(
                gate.start_index(),
                target,
                "the retried connect reuses its recorded Start"
            );
            crash_at_call_end(&executor, &gate).await?;
            let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
            assert!(oplog.call(target).ends.is_empty(), "{}", oplog.describe());
            drop(gate);
        }

        // The retry settles the crashed call once and returns the outcome of its crash window.
        executor = executor.restart(Teardown::Drop).await?;
        let result: Result<String, String> = executor
            .invoke_and_await_typed_with_key(
                &component,
                &agent_id,
                &key,
                "replace_report_loss",
                data_value!(replacement.url()),
            )
            .await?;
        assert_eq!(
            result,
            self.expected_result
                .map(str::to_string)
                .map_err(str::to_string)
        );
        assert_eq!(
            (original.accepted(), replacement.completed_handshakes()),
            (1, self.replacement_handshakes),
            "the original peer is never reconnected; replacement handshakes over every attempt"
        );
        let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
        assert!(
            start_function(&oplog, target).is_some(),
            "the crashed call keeps its one Start at {target}: {}",
            oplog.describe()
        );
        assert_call_terminal_counts(&oplog, target, 1, 1);

        // A distinct explicit replacement can initialize normally after either loss outcome.
        let mut fresh = ReconnectTestServer::start().await;
        let greeting: Result<String, String> = executor
            .invoke_and_await_typed(
                &component,
                &agent_id,
                "replace_report_loss",
                data_value!(fresh.url()),
            )
            .await?;
        assert_eq!(greeting, Ok(RECONNECT_FIRST_MESSAGE.to_string()));
        executor = executor.restart(Teardown::Shutdown).await?;
        assert_reports_session_loss(&executor, &component, &agent_id).await?;
        assert_eq!(
            (fresh.completed_handshakes(), original.accepted()),
            (1, 1),
            "the fresh replacement connects once and the original peer is never reconnected"
        );
        drop(executor);
        original.stop().await?;
        replacement.stop().await?;
        fresh.stop().await?;
        Ok(())
    }
}

#[test]
#[tracing::instrument]
#[timeout("4m")]
async fn websocket_report_loss_exact_send_and_receive_crash_prefixes(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let WebsocketTest {
        executor,
        component,
        ..
    } = WebsocketTest::start_with_connection_limit(deps, last_unique_id, host_api_tests, 1).await?;
    let mut peer = ReconnectTestServer::start().await;
    let agent = WebsocketAgent::start(&executor, &component, "exact-loss-prefixes").await?;
    let greeting: String = executor
        .invoke_and_await_typed(
            &component,
            &agent.agent_id,
            "connect_report_loss",
            data_value!(peer.url()),
        )
        .await?;
    assert_eq!(greeting, RECONNECT_FIRST_MESSAGE);

    let send_key = IdempotencyKey::fresh();
    let crashed = crash_a_send_before_its_end(&executor, &agent, &peer, &send_key).await?;
    let executor =
        reconstruct_the_unfinished_send_twice(executor, &agent, &peer, &send_key, &crashed).await?;
    let executor = crash_receives_on_the_lost_handle(executor, &agent).await?;

    // Every call settles once: the siblings completed before the crash, the send and the
    // receives as session losses. The completed send reached the peer once, on the one
    // connection.
    let executor = executor.restart(Teardown::Shutdown).await?;
    assert_reports_session_loss(&executor, &component, &agent.agent_id).await?;
    let oplog = agent.oplog(&executor).await?;
    for sibling in &crashed.completed_siblings {
        assert_call_terminal_counts(&oplog, *sibling, 1, 1);
    }
    assert_call_terminal_counts(&oplog, crashed.send, 1, 1);
    assert_eq!(
        (
            peer.frames_received()
                .iter()
                .filter(|text| text.as_str() == COMPLETED_SEND_FRAME)
                .count(),
            peer.accepted(),
            peer.completed_handshakes()
        ),
        (1, 1, 1),
        "the completed send reached the peer once, on the one connection"
    );
    drop(executor);
    peer.stop().await?;
    Ok(())
}

/// The send that the executor crashed before its End, and the calls on the handle that
/// completed before it.
struct CrashedSend {
    send: OplogIndex,
    completed_siblings: Vec<OplogIndex>,
}

/// Crashes the executor while a send waits before its End. The send effect can have reached
/// the peer; only its absent End is authoritative.
async fn crash_a_send_before_its_end(
    executor: &TestWorkerExecutor,
    agent: &WebsocketAgent,
    peer: &ReconnectTestServer,
    send_key: &IdempotencyKey,
) -> anyhow::Result<CrashedSend> {
    let mut gate = executor
        .gate_next_call_end(&agent.worker_id.agent_id, false)
        .await;
    executor
        .invoke_agent_with_key(
            &agent.component,
            &agent.agent_id,
            send_key,
            "send_persisted_result",
            data_value!("ambiguous-send".to_string()),
        )
        .await?;
    within(GATE_TIMEOUT, "the gated call End", gate.entered()).await?;
    crash_at_call_end(executor, &gate).await?;

    let oplog = agent.oplog(executor).await?;
    let send = oplog
        .starts_of(WEBSOCKET_SEND_FUNCTION)
        .last()
        .map(|entry| entry.oplog_index)
        .expect("pending send Start");
    assert!(oplog.call(send).ends.is_empty(), "{}", oplog.describe());
    let completed_siblings = oplog
        .iter()
        .filter_map(|entry| match &entry.entry {
            PublicOplogEntry::Start(params)
                if entry.oplog_index < send
                    && (params.function_name == WEBSOCKET_SEND_FUNCTION
                        || params.function_name.ends_with("::receive")) =>
            {
                Some(entry.oplog_index)
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        completed_siblings.len(),
        2,
        "the connect's completed send and receive: {}",
        oplog.describe()
    );
    for sibling in &completed_siblings {
        assert_call_terminal_counts(&oplog, *sibling, 1, 1);
    }
    peer.wait_for_frame(
        COMPLETED_SEND_FRAME,
        "observe the historical completed send",
    )
    .await?;
    drop(gate);
    Ok(CrashedSend {
        send,
        completed_siblings,
    })
}

/// Reconstructs the same unfinished send twice, first crashing before its loss End, then with
/// that End durable but before guest completion delivery. The reconstruction takes no pool
/// permit and opens no connection; the retry then reports the session loss.
async fn reconstruct_the_unfinished_send_twice(
    mut executor: TestWorkerExecutor,
    agent: &WebsocketAgent,
    peer: &ReconnectTestServer,
    send_key: &IdempotencyKey,
    crashed: &CrashedSend,
) -> anyhow::Result<TestWorkerExecutor> {
    for after_end in [false, true] {
        executor = executor.restart(Teardown::Drop).await?;
        let mut admission = executor.gate_next_replay_access_admission(
            &agent.worker_id.agent_id,
            "client::connect",
            ReplayAdmissionStage::BeforeScope,
        );
        let mut gate = executor
            .gate_next_call_end(&agent.worker_id.agent_id, after_end)
            .await;
        executor
            .invoke_agent_with_key(
                &agent.component,
                &agent.agent_id,
                send_key,
                "send_persisted_result",
                data_value!("ambiguous-send".to_string()),
            )
            .await?;
        within(
            GATE_TIMEOUT,
            "the replay admission of the reconstructed connect",
            admission.entered(),
        )
        .await?;
        let pool = executor.websocket_connection_pool().expect("captured pool");
        let permit = within(
            POOL_PERMIT_TIMEOUT,
            "a websocket pool permit",
            pool.acquire(),
        )
        .await??;
        admission.release();
        within(GATE_TIMEOUT, "the gated call End", gate.entered()).await?;
        crash_at_call_end(&executor, &gate).await?;
        let oplog = agent.oplog(&executor).await?;
        assert_call_terminal_counts(&oplog, crashed.send, usize::from(after_end), 0);
        assert_eq!(peer.completed_handshakes(), 1, "no reconnect: {after_end}");
        drop(gate);
        drop(permit);
    }
    executor = executor.restart(Teardown::Drop).await?;
    let result: Result<(), String> = executor
        .invoke_and_await_typed_with_key(
            &agent.component,
            &agent.agent_id,
            send_key,
            "send_persisted_result",
            data_value!("ambiguous-send".to_string()),
        )
        .await?;
    assert_eq!(result, Err("Send error: Error::SessionLost".into()));
    Ok(executor)
}

/// Async receives on the lost handle keep the End and delivery split too: crashed before or
/// after their loss End, their retry settles each with one End and one delivery.
async fn crash_receives_on_the_lost_handle(
    mut executor: TestWorkerExecutor,
    agent: &WebsocketAgent,
) -> anyhow::Result<TestWorkerExecutor> {
    for after_end in [false, true] {
        executor = executor.restart(Teardown::Shutdown).await?;
        executor
            .invoke_and_await_agent(&agent.component, &agent.agent_id, "noop", data_value!())
            .await?;
        let pool = executor.websocket_connection_pool().expect("captured pool");
        let permit = within(
            POOL_PERMIT_TIMEOUT,
            "a websocket pool permit",
            pool.acquire(),
        )
        .await??;
        let key = IdempotencyKey::fresh();
        let mut gate = executor
            .gate_next_call_end(&agent.worker_id.agent_id, after_end)
            .await;
        agent.start_contention(&executor, &key).await?;
        within(GATE_TIMEOUT, "the gated call End", gate.entered()).await?;
        crash_at_call_end(&executor, &gate).await?;
        let oplog = agent.oplog(&executor).await?;
        let start = gate.start_index();
        assert_call_terminal_counts(&oplog, start, usize::from(after_end), 0);
        assert!(
            start_function(&oplog, start).is_some_and(
                |name| name.ends_with("::receive") || name.ends_with("::receive-with-timeout")
            ),
            "the crashed call is a receive: {}",
            oplog.describe()
        );
        drop(gate);
        drop(permit);

        executor = executor.restart(Teardown::Drop).await?;
        assert_eq!(
            agent.contention_result(&executor, &key).await?,
            ("session-lost".into(), "session-lost".into())
        );
        let oplog = agent.oplog(&executor).await?;
        assert_call_terminal_counts(&oplog, start, 1, 1);
    }
    Ok(executor)
}

#[test]
#[tracing::instrument]
#[timeout("6m")]
async fn websocket_report_loss_recovers_a_crash_prefix_with_one_sibling_start(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    // Every case starts its own executor in the same context.
    let context = TestContext::new(last_unique_id);
    for siblings in [
        ContentionSiblings {
            recorded: RECEIVE_SUFFIX,
            missing: TIMED_RECEIVE_SUFFIX,
        },
        ContentionSiblings {
            recorded: TIMED_RECEIVE_SUFFIX,
            missing: RECEIVE_SUFFIX,
        },
    ] {
        for after_end in [false, true] {
            let executor = start_with_overrides(deps, &context, connection_limit(1)).await?;
            let mut peer = ReconnectTestServer::start().await;
            let component = executor
                .component_dep(&context.default_environment_id, host_api_tests)
                .store()
                .await?;
            let name = format!(
                "one-sibling-{}-{after_end}",
                siblings.recorded.trim_start_matches("::")
            );
            let agent = WebsocketAgent::start(&executor, &component, &name).await?;
            executor
                .invoke_and_await_agent(
                    &component,
                    &agent.agent_id,
                    "connect_report_loss",
                    data_value!(peer.url()),
                )
                .await?;

            let (executor, crashed) =
                crash_with_one_sibling_start(executor, &agent, siblings, after_end).await?;
            let executor = replay_while_the_recorded_sibling_is_unclaimed(
                executor, &agent, siblings, &crashed,
            )
            .await?;

            // Both siblings settle once as session losses: the recorded one at its original
            // Start, the missing one at one new Start.
            let oplog = agent.oplog(&executor).await?;
            assert_call_terminal_counts(&oplog, crashed.recorded_start, 1, 1);
            let missing_starts = oplog
                .iter()
                .filter_map(|entry| match &entry.entry {
                    PublicOplogEntry::Start(params)
                        if entry.oplog_index > crashed.recorded_start
                            && params.function_name.ends_with(siblings.missing) =>
                    {
                        Some(entry.oplog_index)
                    }
                    _ => None,
                })
                .collect::<Vec<_>>();
            assert_eq!(missing_starts.len(), 1, "{}", oplog.describe());
            assert_call_terminal_counts(&oplog, missing_starts[0], 1, 1);
            assert_eq!(
                (peer.accepted(), peer.completed_handshakes()),
                (1, 1),
                "the lost handle opens no new connection"
            );
            executor.shutdown_and_wait_for_invocation_loops().await?;
            drop(executor);
            peer.stop().await?;
        }
    }
    Ok(())
}

const RECEIVE_SUFFIX: &str = "::receive";
const TIMED_RECEIVE_SUFFIX: &str = "::receive-with-timeout";

/// The two receives of the contention invocation: the one whose `Start` the crash leaves in
/// the oplog, and the one that has no `Start` yet.
#[derive(Debug, Clone, Copy)]
struct ContentionSiblings {
    recorded: &'static str,
    missing: &'static str,
}

/// The contention invocation that the executor crashed with only the recorded sibling's
/// `Start` in the oplog.
struct CrashedContention {
    key: IdempotencyKey,
    recorded_start: OplogIndex,
}

/// Live: the missing sibling stops before appending its `Start` while the recorded sibling
/// reaches its End, then the executor crashes.
async fn crash_with_one_sibling_start(
    executor: TestWorkerExecutor,
    agent: &WebsocketAgent,
    siblings: ContentionSiblings,
    after_end: bool,
) -> anyhow::Result<(TestWorkerExecutor, CrashedContention)> {
    let executor = executor.restart(Teardown::Shutdown).await?;
    executor
        .invoke_and_await_agent(&agent.component, &agent.agent_id, "noop", data_value!())
        .await?;
    let pool = executor.websocket_connection_pool().expect("captured pool");
    let permit = within(
        POOL_PERMIT_TIMEOUT,
        "a websocket pool permit",
        pool.acquire(),
    )
    .await??;
    let mut live_hold = executor.gate_next_replay_access_admission(
        &agent.worker_id.agent_id,
        siblings.missing,
        ReplayAdmissionStage::BeforeLiveStart,
    );
    let mut end_gate = executor
        .gate_next_call_end(&agent.worker_id.agent_id, after_end)
        .await;
    let key = IdempotencyKey::fresh();
    agent.start_contention(&executor, &key).await?;
    within(
        GATE_TIMEOUT,
        "the missing sibling to stop before its live Start",
        live_hold.entered(),
    )
    .await?;
    within(
        GATE_TIMEOUT,
        "the End gate of the recorded sibling",
        end_gate.entered(),
    )
    .await?;
    crash_at_call_end(&executor, &end_gate).await?;

    let oplog = agent.oplog(&executor).await?;
    let starts = oplog
        .iter()
        .filter(|entry| entry.oplog_index >= end_gate.start_index())
        .filter_map(|entry| match &entry.entry {
            PublicOplogEntry::Start(params)
                if params.function_name.ends_with(RECEIVE_SUFFIX)
                    || params.function_name.ends_with(TIMED_RECEIVE_SUFFIX) =>
            {
                Some(params.function_name.clone())
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        starts.len(),
        1,
        "only the recorded sibling has a Start: {}",
        oplog.describe()
    );
    assert!(
        starts[0].ends_with(siblings.recorded),
        "{}",
        oplog.describe()
    );
    let recorded_start = end_gate.start_index();
    drop(live_hold);
    drop(end_gate);
    drop(permit);
    Ok((
        executor,
        CrashedContention {
            key,
            recorded_start,
        },
    ))
}

/// Replay: the recorded sibling is held before its claim, and the missing sibling must
/// continue live while the recorded `Start` is still unclaimed. Both then report the session
/// loss.
async fn replay_while_the_recorded_sibling_is_unclaimed(
    executor: TestWorkerExecutor,
    agent: &WebsocketAgent,
    siblings: ContentionSiblings,
    crashed: &CrashedContention,
) -> anyhow::Result<TestWorkerExecutor> {
    let executor = executor.restart(Teardown::Drop).await?;
    // The earlier `connect_report_loss` invocation replays the same function first.
    let mut earlier_occurrence = executor.gate_next_replay_access_admission(
        &agent.worker_id.agent_id,
        siblings.recorded,
        ReplayAdmissionStage::BeforeScope,
    );
    let mut recorded_hold = executor.gate_next_replay_access_admission(
        &agent.worker_id.agent_id,
        siblings.recorded,
        ReplayAdmissionStage::BeforeScope,
    );
    let mut missing_live = executor.gate_next_replay_access_admission(
        &agent.worker_id.agent_id,
        siblings.missing,
        ReplayAdmissionStage::BeforeLiveStart,
    );
    agent.start_contention(&executor, &crashed.key).await?;
    within(
        GATE_TIMEOUT,
        "the replay admission of the earlier connect_report_loss occurrence",
        earlier_occurrence.entered(),
    )
    .await?;
    earlier_occurrence.release();
    within(
        GATE_TIMEOUT,
        "the recorded sibling to reach its replay claim",
        recorded_hold.entered(),
    )
    .await?;
    within(
        GATE_TIMEOUT,
        "the missing sibling to reach its live Start",
        missing_live.entered(),
    )
    .await?;
    missing_live.release();
    recorded_hold.release();
    assert_eq!(
        agent.contention_result(&executor, &crashed.key).await?,
        ("session-lost".to_string(), "session-lost".to_string())
    );
    Ok(executor)
}

fn assert_call_terminal_counts(
    oplog: &[PublicOplogEntryWithIndex],
    start: OplogIndex,
    ends: usize,
    delivered: usize,
) {
    let call = oplog.call(start);
    assert_eq!(
        (
            call.ends.len(),
            call.deliveries.len(),
            call.discards.len(),
            call.cancellations.len()
        ),
        (ends, delivered, 0, 0),
        "Ends, deliveries, discards and cancellations of the call at {start}: {}",
        oplog.describe()
    );
}

#[test]
#[tracing::instrument]
#[timeout("3m")]
async fn websocket_report_loss_preserves_live_disconnect_and_recorded_close(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let WebsocketTest {
        mut executor,
        component,
        ..
    } = WebsocketTest::start(deps, last_unique_id, host_api_tests).await?;
    for closed in [false, true] {
        let mut peer = ReconnectTestServer::start().await;
        let agent_id = agent_key!("WebsocketTest", format!("closed-{closed}"));
        executor
            .start_agent(&component.id, agent_id.clone())
            .await?;
        executor
            .invoke_and_await_agent(
                &component,
                &agent_id,
                "connect_report_loss",
                data_value!(peer.url()),
            )
            .await?;
        peer.wait_for_frame(COMPLETED_SEND_FRAME, "receive completed send")
            .await?;

        // A recorded close stays a close after a restart; a live disconnect is not a session
        // loss.
        let method = if closed {
            let closed: Result<(), String> = executor
                .invoke_and_await_typed(
                    &component,
                    &agent_id,
                    "close_persisted_result",
                    data_value!(),
                )
                .await?;
            assert_eq!(closed, Ok(()));
            executor = executor.restart(Teardown::Shutdown).await?;
            "receive_is_closed"
        } else {
            peer.stop().await?;
            "live_disconnect_is_not_session_lost"
        };
        let holds: bool = executor
            .invoke_and_await_typed(&component, &agent_id, method, data_value!())
            .await?;
        assert!(holds, "{method}");
        assert_eq!(
            (peer.accepted(), peer.completed_handshakes()),
            (1, 1),
            "the handle opens no new connection"
        );
        if closed {
            peer.stop().await?;
        }
    }
    drop(executor);
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("3m")]
async fn websocket_default_policy_reports_session_loss_after_restart(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let WebsocketTest {
        mut executor,
        component,
        ..
    } = WebsocketTest::start(deps, last_unique_id, host_api_tests).await?;
    for first_access in [
        "receive_next_from_persisted_result",
        "send_persisted_result",
    ] {
        let mut peer = ReconnectTestServer::start().await;
        let agent_id = agent_key!("WebsocketTest", format!("default-policy-{first_access}"));
        executor
            .start_agent(&component.id, agent_id.clone())
            .await?;
        let greeting: String = executor
            .invoke_and_await_typed(
                &component,
                &agent_id,
                "connect_default_and_receive_first",
                data_value!(peer.url()),
            )
            .await?;
        assert_eq!(greeting, RECONNECT_FIRST_MESSAGE);
        executor = executor.restart(Teardown::Shutdown).await?;

        let first = if first_access == "send_persisted_result" {
            executor
                .invoke_and_await_typed::<Result<(), String>>(
                    &component,
                    &agent_id,
                    first_access,
                    data_value!("must-not-reach-peer"),
                )
                .await?
                .map(|()| "sent".to_string())
        } else {
            executor
                .invoke_and_await_typed::<Result<String, String>>(
                    &component,
                    &agent_id,
                    first_access,
                    data_value!(),
                )
                .await?
        };
        let later: Result<String, String> = executor
            .invoke_and_await_typed(
                &component,
                &agent_id,
                "receive_next_from_persisted_result",
                data_value!(),
            )
            .await?;
        for result in [first, later] {
            let error = result.expect_err("a handle without a policy must not reconnect");
            assert!(error.ends_with("SessionLost"), "{error}");
        }
        assert_eq!(
            (
                peer.accepted(),
                peer.completed_handshakes(),
                peer.frames_received()
            ),
            (1, 1, Vec::<String>::new()),
            "no replacement connection, and the failed send reaches no peer"
        );
        peer.stop().await?;
    }
    drop(executor);
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("3m")]
async fn websocket_reconstruction_policies_are_independent_in_one_worker(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let WebsocketTest {
        executor,
        component,
        ..
    } = WebsocketTest::start(deps, last_unique_id, host_api_tests).await?;
    let mut report_peer = ReconnectTestServer::start().await;
    let mut automatic_peer = ReconnectTestServer::start().await;
    let agent_id = agent_key!("WebsocketTest", "independent-policies");
    executor
        .start_agent(&component.id, agent_id.clone())
        .await?;
    let greetings: (String, String) = executor
        .invoke_and_await_typed(
            &component,
            &agent_id,
            "connect_both_policies",
            data_value!(report_peer.url(), automatic_peer.url()),
        )
        .await?;
    assert_eq!(
        greetings,
        (
            RECONNECT_FIRST_MESSAGE.into(),
            RECONNECT_FIRST_MESSAGE.into()
        )
    );
    report_peer
        .wait_for_frame(COMPLETED_SEND_FRAME, "receive completed report-policy send")
        .await?;

    let executor = executor.restart(Teardown::Shutdown).await?;
    let probed: (bool, String) = executor
        .invoke_and_await_typed(&component, &agent_id, "probe_both_policies", data_value!())
        .await?;
    assert_eq!(
        probed,
        (true, RECONNECT_MESSAGES[0].into()),
        "the report-loss handle reports the loss and the automatic handle reconnects"
    );
    assert_eq!(
        (report_peer.accepted(), report_peer.frames_received()),
        (1, vec![COMPLETED_SEND_FRAME.to_string()]),
        "the report-loss handle does not reconnect"
    );
    assert_eq!(
        (
            automatic_peer.accepted(),
            automatic_peer.completed_handshakes()
        ),
        (2, 2),
        "the automatic handle reconnects once"
    );
    drop(executor);
    report_peer.stop().await?;
    automatic_peer.stop().await?;
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("3m")]
async fn websocket_report_loss_effect_joins_initialization_before_replacement(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("tool_streaming_effect_caller")] caller: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let WebsocketTest {
        executor,
        component,
        ..
    } = WebsocketTest::start(deps, last_unique_id, caller).await?;
    let mut old_peer = ReconnectTestServer::start().await;
    let mut replacement_peer = ReconnectTestServer::start().await;
    let agent_id = agent_key!("WebsocketRecoveryProbe", "effect-real-loss");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    // Crash while the old connection's initialization has completed its greeting receive and
    // a second read is pending.
    let key = IdempotencyKey::fresh();
    executor
        .invoke_agent_with_key(
            &component,
            &agent_id,
            &key,
            "recover",
            data_value!(old_peer.url(), replacement_peer.url()),
        )
        .await?;
    old_peer
        .wait_for_frame("initialize-old", "receive old initialization")
        .await?;
    within(
        OPLOG_PROGRESS_TIMEOUT,
        "the pending read's receive Start",
        async {
            loop {
                executor.commit_oplog(&worker_id.agent_id).await?;
                let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
                if starts_of_websocket_function(&oplog, WEBSOCKET_RECEIVE_FUNCTION).len() == 2 {
                    return Ok::<_, anyhow::Error>(());
                }
                tokio::task::yield_now().await;
            }
        },
    )
    .await??;
    executor.shutdown_and_wait_for_invocation_loops().await?;
    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    let receives = starts_of_websocket_function(&oplog, WEBSOCKET_RECEIVE_FUNCTION);
    assert_eq!(
        receives.len(),
        2,
        "completed greeting and pending read must both exist"
    );
    assert!(
        oplog.call(receives[1]).ends.is_empty(),
        "the pending read has no End: {}",
        oplog.describe()
    );

    // The retry joins the old initialization before it replaces the lost connection.
    let executor = executor.restart(Teardown::Drop).await?;
    let result: (bool, bool, String) = executor
        .invoke_and_await_typed_with_key(
            &component,
            &agent_id,
            &key,
            "recover",
            data_value!(old_peer.url(), replacement_peer.url()),
        )
        .await?;
    assert_eq!(result, (true, true, RECONNECT_FIRST_MESSAGE.into()));
    replacement_peer
        .wait_for_frame("initialize-new", "receive replacement initialization")
        .await?;
    assert_eq!(
        (old_peer.accepted(), old_peer.frames_received()),
        (1, vec!["initialize-old".to_string()]),
        "the old peer is initialized once and never reconnected"
    );
    assert_eq!(
        (
            replacement_peer.accepted(),
            replacement_peer.frames_received()
        ),
        (1, vec!["initialize-new".to_string()]),
        "the replacement peer is connected and initialized once"
    );
    drop(executor);
    old_peer.stop().await?;
    replacement_peer.stop().await?;
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("3m")]
async fn websocket_report_loss_ts_public_wrapper_replacement(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("agent_sdk_ts")] agent_sdk_ts: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let WebsocketTest {
        executor,
        component,
        ..
    } = WebsocketTest::start(deps, last_unique_id, agent_sdk_ts).await?;
    let mut old_peer = ReconnectTestServer::start().await;
    let mut replacement_peer = ReconnectTestServer::start().await;
    let agent_id = agent_key!("WebSocketTest", "public-wrapper-loss");
    executor
        .start_agent(&component.id, agent_id.clone())
        .await?;
    let greeting: String = executor
        .invoke_and_await_typed(
            &component,
            &agent_id,
            "connectReportLoss",
            data_value!(old_peer.url()),
        )
        .await?;
    assert_eq!(greeting, RECONNECT_FIRST_MESSAGE);

    // After a restart the TypeScript wrapper reports the loss on every operation, without
    // reconnecting.
    let executor = executor.restart(Teardown::Shutdown).await?;
    let reported: Vec<bool> = executor
        .invoke_and_await_typed(&component, &agent_id, "probeReportLoss", data_value!())
        .await?;
    assert_eq!(reported, vec![true; 4]);
    assert_eq!(
        (old_peer.accepted(), old_peer.frames_received()),
        (1, Vec::<String>::new()),
        "the lost handle neither reconnects nor sends"
    );

    // An explicit replacement connects to the new peer and initializes it.
    let greeting: String = executor
        .invoke_and_await_typed(
            &component,
            &agent_id,
            "replaceLost",
            data_value!(replacement_peer.url()),
        )
        .await?;
    assert_eq!(greeting, RECONNECT_FIRST_MESSAGE);
    replacement_peer
        .wait_for_frame(
            "initialize-new",
            "receive public-wrapper replacement initialization",
        )
        .await?;
    assert_eq!(
        (old_peer.accepted(), replacement_peer.completed_handshakes()),
        (1, 1),
        "the replacement connects once and the old peer is never reconnected"
    );
    drop(executor);
    old_peer.stop().await?;
    replacement_peer.stop().await?;
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("3m")]
async fn websocket_report_loss_absorbs_repeated_restart_without_pool_io(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let WebsocketTest {
        mut executor,
        component,
        ..
    } = WebsocketTest::start_with_connection_limit(deps, last_unique_id, host_api_tests, 1).await?;
    let mut peer = ReconnectTestServer::start().await;
    let agent_id = agent_key!("WebsocketTest", "report-loss-restarts");
    executor
        .start_agent(&component.id, agent_id.clone())
        .await?;
    let first: String = executor
        .invoke_and_await_typed(
            &component,
            &agent_id,
            "connect_report_loss",
            data_value!(peer.url()),
        )
        .await?;
    assert_eq!(first, RECONNECT_FIRST_MESSAGE);
    peer.wait_for_frame(
        COMPLETED_SEND_FRAME,
        "receive the completed send before crashing",
    )
    .await?;

    for _ in 0..3 {
        executor = executor.restart(Teardown::Shutdown).await?;
        // Activating first reconstructs completed receive and timeout calls.
        let noop: String = executor
            .invoke_and_await_typed(&component, &agent_id, "noop", data_value!())
            .await?;
        assert_eq!(noop, "ok");
        let pool = executor
            .websocket_connection_pool()
            .expect("captured websocket pool");
        let permit = within(
            POOL_PERMIT_TIMEOUT,
            "a websocket pool permit",
            pool.acquire(),
        )
        .await??;
        // Loss must not wait for even the only permit, including close.
        let reported = within(
            UNBLOCKED_INVOCATION_TIMEOUT,
            "probe_report_loss while the test holds the only pool permit",
            executor.invoke_and_await_typed::<(bool, bool, bool, bool)>(
                &component,
                &agent_id,
                "probe_report_loss",
                data_value!(),
            ),
        )
        .await??;
        assert_eq!(
            reported,
            (true, true, true, true),
            "receive, receive-with-timeout, send and close must each report SessionLost"
        );
        drop(permit);
        assert_eq!(
            (
                peer.accepted(),
                peer.completed_handshakes(),
                peer.frames_sent(),
                peer.frames_received()
            ),
            (
                1,
                1,
                vec![RECONNECT_FIRST_MESSAGE.to_string()],
                vec![COMPLETED_SEND_FRAME.to_string()]
            ),
            "the restarts open no connection and exchange no frame"
        );
    }

    // Dropping the lost handle does not wait for the only permit either.
    let pool = executor
        .websocket_connection_pool()
        .expect("captured websocket pool");
    let permit = within(
        POOL_PERMIT_TIMEOUT,
        "a websocket pool permit",
        pool.acquire(),
    )
    .await??;
    within(
        UNBLOCKED_INVOCATION_TIMEOUT,
        "drop_persisted while the test holds the only pool permit",
        executor.invoke_and_await_agent(&component, &agent_id, "drop_persisted", data_value!()),
    )
    .await??;
    drop(permit);
    assert_eq!(peer.accepted(), 1);
    drop(executor);
    peer.stop().await?;
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn websocket_report_loss_pending_receives_after_restart(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let WebsocketTest {
        executor,
        component,
        ..
    } = WebsocketTest::start(deps, last_unique_id, host_api_tests).await?;
    let mut peer = ReconnectTestServer::start().await;
    let agent = WebsocketAgent::start(&executor, &component, "report-loss-pending").await?;
    executor
        .invoke_and_await_agent(
            &component,
            &agent.agent_id,
            "connect_report_loss",
            data_value!(peer.url()),
        )
        .await?;

    // Crash while both contention receives are pending.
    let key = IdempotencyKey::fresh();
    agent.start_contention(&executor, &key).await?;
    within(
        OPLOG_PROGRESS_TIMEOUT,
        "both contention receive Starts",
        async {
            loop {
                executor.commit_oplog(&agent.worker_id.agent_id).await?;
                let oplog = agent.oplog(&executor).await?;
                if contention_receive_starts(&oplog).len() == 2 {
                    assert_contention_receives_incomplete(&oplog);
                    return Ok::<_, anyhow::Error>(());
                }
                tokio::task::yield_now().await;
            }
        },
    )
    .await??;
    peer.wait_for_frame(
        COMPLETED_SEND_FRAME,
        "receive the completed send before crashing",
    )
    .await?;
    executor.shutdown_and_wait_for_invocation_loops().await?;
    assert_contention_receives_incomplete(&agent.oplog(&executor).await?);

    // The retry settles both receives as session losses, without a new connection.
    let executor = executor.restart(Teardown::Drop).await?;
    let result = agent.contention_result(&executor, &key).await?;
    assert_eq!(result, ("session-lost".into(), "session-lost".into()));
    assert_contention_receive_correlation(&agent.oplog(&executor).await?);
    assert_reports_session_loss(&executor, &component, &agent.agent_id).await?;
    assert_eq!(
        (
            peer.accepted(),
            peer.completed_handshakes(),
            peer.frames_received()
        ),
        (1, 1, vec![COMPLETED_SEND_FRAME.to_string()]),
        "the lost handle opens no new connection and sends no new frame"
    );
    drop(executor);
    peer.stop().await?;
    Ok(())
}

/// The order in which two replayed receives reach their replay claims.
#[derive(Debug, Clone, Copy)]
enum TwoSocketClaimOrder {
    /// The host tasks claim in whatever order they are scheduled.
    Natural,
    /// The receive admitted first is held before its claim until the other one has claimed.
    Reversed,
}

/// A peer with two paths: `/first` greets every connection at once and `/second` greets its
/// connections only after the test releases it.
struct TwoPathPeer {
    port: u16,
    first_connections: Arc<AtomicUsize>,
    second_connections: Arc<AtomicUsize>,
    release_second: watch::Sender<bool>,
    accept_loop: JoinHandle<()>,
}

impl TwoPathPeer {
    async fn start() -> anyhow::Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let port = listener.local_addr()?.port();
        let first_connections = Arc::new(AtomicUsize::new(0));
        let second_connections = Arc::new(AtomicUsize::new(0));
        let (release_second, released) = watch::channel(false);
        let accept_loop = spawn({
            let first_connections = Arc::clone(&first_connections);
            let second_connections = Arc::clone(&second_connections);
            async move {
                loop {
                    let Ok((socket, _)) = listener.accept().await else {
                        break;
                    };
                    let first_connections = Arc::clone(&first_connections);
                    let second_connections = Arc::clone(&second_connections);
                    let mut released = released.clone();
                    spawn(
                        async move {
                            let mut first = false;
                            let Ok(mut socket) = tokio_tungstenite::accept_hdr_async(
                                socket,
                                |request: &tokio_tungstenite::tungstenite::handshake::server::Request,
                                 response| {
                                    first = request.uri().path() == "/first";
                                    Ok(response)
                                },
                            )
                            .await
                            else {
                                return;
                            };
                            let greeting = if first {
                                first_connections.fetch_add(1, Ordering::SeqCst);
                                "first-hello"
                            } else {
                                second_connections.fetch_add(1, Ordering::SeqCst);
                                if released.wait_for(|released| *released).await.is_err() {
                                    return;
                                }
                                "second-hello"
                            };
                            let _ = socket
                                .send(Message::Text(
                                    greeting.into(),
                                ))
                                .await;
                            while let Some(message) = socket.next().await {
                                if matches!(
                                    message,
                                    Ok(Message::Close(_)) | Err(_)
                                ) {
                                    break;
                                }
                            }
                        }
                        .in_current_span(),
                    );
                }
            }
            .in_current_span()
        });
        Ok(Self {
            port,
            first_connections,
            second_connections,
            release_second,
            accept_loop,
        })
    }

    fn url(&self, path: &str) -> String {
        format!("ws://127.0.0.1:{}{path}", self.port)
    }

    async fn wait_for_second_connections(&self, count: usize) -> anyhow::Result<()> {
        within(
            PEER_STATE_TIMEOUT,
            format!("the peer to accept {count} connection(s) on /second"),
            async {
                while self.second_connections.load(Ordering::SeqCst) < count {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            },
        )
        .await
    }
}

/// Two concurrent `receive` calls on different websocket handles record identical empty
/// requests. A crash while the first socket's receive is complete and delivered but the
/// second socket's receive is still pending must replay the recorded message to the socket
/// that received it, in either host claim order. Routing it to the other socket's receive
/// completes that receive with a foreign message and reconnects the wrong socket.
async fn receive_from_two_sockets_after_crash(
    executor: &TestWorkerExecutor,
    component: &ComponentDto,
    agent_name: &str,
    claim_order: TwoSocketClaimOrder,
) -> anyhow::Result<()> {
    let peer = TwoPathPeer::start().await?;
    let agent_id = agent_key!("WebsocketTest", agent_name);
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;
    let key = IdempotencyKey::fresh();
    executor
        .invoke_agent_with_key(
            component,
            &agent_id,
            &key,
            "receive_from_two_sockets",
            data_value!(peer.url("/first"), peer.url("/second")),
        )
        .await?;

    // Crash once the first socket's greeting is durable and delivered while the second
    // socket's receive is still open.
    let receive_starts = |oplog: &[PublicOplogEntryWithIndex]| {
        starts_of_websocket_function(oplog, WEBSOCKET_RECEIVE_FUNCTION)
    };
    let has_end =
        |oplog: &[PublicOplogEntryWithIndex], start: OplogIndex| !oplog.call(start).ends.is_empty();
    let delivered = |oplog: &[PublicOplogEntryWithIndex], start: OplogIndex| {
        !oplog.call(start).deliveries.is_empty()
    };
    within(
        OPLOG_PROGRESS_TIMEOUT,
        "the first receive to complete while the second stays open",
        async {
            loop {
                executor.commit_oplog(&worker_id.agent_id).await?;
                let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
                let starts = receive_starts(&oplog);
                if starts.len() == 2 {
                    let completed = starts
                        .iter()
                        .filter(|start| has_end(&oplog, **start) && delivered(&oplog, **start))
                        .count();
                    let open = starts
                        .iter()
                        .filter(|start| !has_end(&oplog, **start))
                        .count();
                    if completed == 1 && open == 1 {
                        return Ok::<_, anyhow::Error>(());
                    }
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        },
    )
    .await??;

    let gates = match claim_order {
        TwoSocketClaimOrder::Natural => None,
        TwoSocketClaimOrder::Reversed => {
            let gate = |stage| {
                executor.gate_next_replay_access_admission(
                    &worker_id.agent_id,
                    "client::receive",
                    stage,
                )
            };
            Some((
                gate(ReplayAdmissionStage::BeforeScope),
                gate(ReplayAdmissionStage::BeforeScope),
                gate(ReplayAdmissionStage::AfterStart),
            ))
        }
    };
    executor.simulated_crash(&worker_id).await?;
    if let Some((mut held, mut other, mut other_claimed)) = gates {
        within(
            GATE_TIMEOUT,
            "the first replayed receive to reach its claim",
            held.entered(),
        )
        .await?;
        within(
            GATE_TIMEOUT,
            "the second replayed receive to reach its claim",
            other.entered(),
        )
        .await?;
        other.release();
        within(
            GATE_TIMEOUT,
            "the second replayed receive to claim a Start",
            other_claimed.entered(),
        )
        .await?;
        held.release();
        other_claimed.release();
    }

    // The pending receive goes live on a reconnected second socket; only then may the peer
    // send its greeting.
    peer.wait_for_second_connections(2).await?;
    peer.release_second.send_replace(true);

    let result = within(
        SETTLE_TIMEOUT,
        "the replayed invocation to complete",
        executor.invoke_and_await_agent_with_key(
            component,
            &agent_id,
            &key,
            "receive_from_two_sockets",
            data_value!(peer.url("/first"), peer.url("/second")),
        ),
    )
    .await??
    .into_typed::<(String, String)>()?;
    assert_eq!(
        result,
        ("first-hello".to_string(), "second-hello".to_string()),
        "{claim_order:?}: each socket must receive its own message after the crash"
    );
    assert_eq!(
        peer.first_connections.load(Ordering::SeqCst),
        1,
        "{claim_order:?}: the completed receive must not reconnect the first socket"
    );
    assert_eq!(
        peer.second_connections.load(Ordering::SeqCst),
        2,
        "{claim_order:?}: the pending receive must reconnect the second socket once"
    );

    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    let starts = receive_starts(&oplog);
    assert_eq!(
        starts.len(),
        2,
        "{claim_order:?}: replay must reuse both recorded receive Starts"
    );
    for start in starts {
        assert_eq!(
            oplog.call(start).ends.len(),
            1,
            "{claim_order:?}: receive Start {start} must have exactly one End"
        );
        assert!(
            delivered(&oplog, start),
            "{claim_order:?}: receive Start {start} must have a delivery marker"
        );
    }
    let status = executor.get_worker_metadata(&worker_id).await?.status;
    assert_ne!(status, AgentStatus::Failed, "{claim_order:?}");
    executor.check_oplog_is_queryable(&worker_id).await?;
    peer.accept_loop.abort();
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("3m")]
async fn websocket_receives_on_two_sockets_replay_to_their_own_socket_after_crash(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let WebsocketTest {
        executor,
        component,
        ..
    } = WebsocketTest::start(deps, last_unique_id, host_api_tests).await?;
    for (name, claim_order) in [
        ("two-sockets-natural", TwoSocketClaimOrder::Natural),
        ("two-sockets-reversed", TwoSocketClaimOrder::Reversed),
    ] {
        receive_from_two_sockets_after_crash(&executor, &component, name, claim_order).await?;
    }
    drop(executor);
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn websocket_echo_rust(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let WebsocketTest {
        executor,
        component,
        ..
    } = WebsocketTest::start(deps, last_unique_id, host_api_tests).await?;
    let peer = TestPeer::start(Accept::Once, PeerScript::Echo).await;
    let agent_id = agent_key!("WebsocketTest", "ws-echo-test");
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), peer.port_env(), Vec::new())
        .await?;

    let echoed: String = executor
        .invoke_and_await_typed(
            &component,
            &agent_id,
            "echo",
            data_value!(peer.url(), "hello websocket"),
        )
        .await?;

    assert_eq!(echoed, "hello websocket");
    executor.check_oplog_is_queryable(&worker_id).await?;
    drop(executor);
    drop(peer);
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn websocket_echo_rust_oplog_replay(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let WebsocketTest {
        executor,
        component,
        ..
    } = WebsocketTest::start(deps, last_unique_id, host_api_tests).await?;
    // The second invocation after the restart connects live again, so the peer keeps
    // accepting.
    let peer = TestPeer::start(Accept::Serially, PeerScript::Echo).await;
    let agent_id = agent_key!("WebsocketTest", "ws-echo-oplog-replay");
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), peer.port_env(), Vec::new())
        .await?;

    // The first invocation runs a full websocket session and records one entry in the
    // agent-local `echo_history`.
    let first: String = executor
        .invoke_and_await_typed(
            &component,
            &agent_id,
            "echo_and_record",
            data_value!(peer.url(), "hello websocket"),
        )
        .await?;
    assert_eq!(first, "hello websocket");
    executor.check_oplog_is_queryable(&worker_id).await?;

    // After the restart, the next invocation replays the first one to rebuild the agent
    // state, then appends its own message.
    let executor = executor.restart(Teardown::Drop).await?;
    let second: String = executor
        .invoke_and_await_typed(
            &component,
            &agent_id,
            "echo_and_record",
            data_value!(peer.url(), "hello websocket 2"),
        )
        .await?;
    assert_eq!(second, "hello websocket|hello websocket 2");
    executor.check_oplog_is_queryable(&worker_id).await?;

    drop(executor);
    drop(peer);
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn websocket_reconnect_replays_completed_steps_and_continues_live(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let WebsocketTest {
        context,
        executor,
        component,
    } = WebsocketTest::start(deps, last_unique_id, host_api_tests).await?;
    let peer = TestPeer::start(Accept::Concurrently, PeerScript::Echo).await;
    let agent_id = agent_key!("WebsocketTest", "ws-phase2-reconnect");
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), peer.port_env(), Vec::new())
        .await?;
    let promise = create_promise(&executor, &component, &agent_id).await?;
    let params = replay_reconnect_roundtrip_params(peer.url(), &promise);
    let idempotency_key = IdempotencyKey::fresh();

    // The roundtrip sends two messages and suspends on the promise.
    executor
        .invoke_agent_with_key(
            &component,
            &agent_id,
            &idempotency_key,
            "replay_reconnect_roundtrip",
            params.clone(),
        )
        .await?;
    executor
        .wait_for_status(&worker_id, AgentStatus::Suspended, STATUS_TIMEOUT)
        .await?;
    wait_for_server_state(&peer, 1, 2).await?;
    assert_eq!(
        peer.transcript(),
        vec![server_event(0, "msg-1"), server_event(0, "msg-2")]
    );
    executor.check_oplog_is_queryable(&worker_id).await?;

    // The restart alone does not reconnect.
    let executor = executor.restart(Teardown::Drop).await?;
    tokio::time::sleep(RESTART_SETTLE).await;
    assert_eq!(
        (peer.accepted(), peer.transcript().len()),
        (1, 2),
        "the restart opens no connection"
    );

    // Completing the promise resumes the roundtrip: replay skips the completed sends, and the
    // live part reconnects once and sends the rest.
    executor
        .complete_promise(
            &context.default_environment_id,
            &promise_id_from_value(&promise),
            vec![1],
        )
        .await?;
    let result: Result<String, String> = executor
        .invoke_and_await_typed_with_key(
            &component,
            &agent_id,
            &idempotency_key,
            "replay_reconnect_roundtrip",
            params,
        )
        .await?;
    wait_for_server_state(&peer, 2, 4).await?;
    assert_eq!(result, Ok("msg-1|msg-2|msg-3|msg-4".to_string()));
    assert_eq!(
        (peer.accepted(), peer.transcript()),
        (
            2,
            vec![
                server_event(0, "msg-1"),
                server_event(0, "msg-2"),
                server_event(1, "msg-3"),
                server_event(1, "msg-4"),
            ]
        ),
        "one reconnect, and only the messages after the replayed steps are sent on it"
    );
    executor.check_oplog_is_queryable(&worker_id).await?;
    drop(executor);
    drop(peer);
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn websocket_reconnect_failure_returns_guest_error(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let WebsocketTest {
        context,
        executor,
        component,
    } = WebsocketTest::start(deps, last_unique_id, host_api_tests).await?;
    let peer = TestPeer::start(Accept::Concurrently, PeerScript::Echo).await;
    let agent_id = agent_key!("WebsocketTest", "ws-reconnect-failure");
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), peer.port_env(), Vec::new())
        .await?;
    let promise = create_promise(&executor, &component, &agent_id).await?;
    let params = replay_reconnect_roundtrip_params(peer.url(), &promise);
    let idempotency_key = IdempotencyKey::fresh();
    executor
        .invoke_agent_with_key(
            &component,
            &agent_id,
            &idempotency_key,
            "replay_reconnect_roundtrip",
            params.clone(),
        )
        .await?;
    executor
        .wait_for_status(&worker_id, AgentStatus::Suspended, STATUS_TIMEOUT)
        .await?;
    wait_for_server_state(&peer, 1, 2).await?;
    executor.check_oplog_is_queryable(&worker_id).await?;

    // The peer stops accepting, so the reconnect after the restart fails.
    peer.abort();
    let executor = executor.restart(Teardown::Drop).await?;
    executor
        .complete_promise(
            &context.default_environment_id,
            &promise_id_from_value(&promise),
            vec![1],
        )
        .await?;
    let result: Result<String, String> = executor
        .invoke_and_await_typed_with_key(
            &component,
            &agent_id,
            &idempotency_key,
            "replay_reconnect_roundtrip",
            params,
        )
        .await?;

    let error = result.expect_err("expected the reconnect failure to reach the guest");
    assert!(
        error.contains("ConnectionFailure"),
        "expected reconnect failure to reach the guest, got {error}"
    );
    assert_eq!(peer.accepted(), 1);
    executor.check_oplog_is_queryable(&worker_id).await?;
    drop(executor);
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn websocket_closed_connection_stays_terminal_after_replay(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let WebsocketTest {
        executor,
        component,
        ..
    } = WebsocketTest::start(deps, last_unique_id, host_api_tests).await?;
    let peer = TestPeer::start(Accept::Concurrently, PeerScript::SendThenClose("msg-0")).await;
    let agent_id = agent_key!("WebsocketTest", "ws-terminal-close-replay");
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), peer.port_env(), Vec::new())
        .await?;
    let first: String = executor
        .invoke_and_await_typed(
            &component,
            &agent_id,
            "connect_and_receive_first",
            data_value!(peer.url()),
        )
        .await?;
    assert_eq!((first.as_str(), peer.accepted()), ("msg-0", 1));

    // The peer closed the connection, so the persisted socket reports the closure.
    let closed_error = receive_next_error(&executor, &component, &agent_id).await?;
    assert!(
        closed_error.contains("Closed"),
        "expected persisted socket to report closure, got {closed_error}"
    );
    assert_eq!(peer.accepted(), 1);
    executor.check_oplog_is_queryable(&worker_id).await?;

    // Replay keeps the socket closed and does not reconnect.
    let executor = executor.restart(Teardown::Drop).await?;
    let replayed_error = receive_next_error(&executor, &component, &agent_id).await?;
    assert!(
        replayed_error.contains("Closed"),
        "expected replayed terminal socket to stay closed, got {replayed_error}"
    );
    assert_eq!(
        peer.accepted(),
        1,
        "terminal websocket should not reconnect after replay"
    );
    executor.check_oplog_is_queryable(&worker_id).await?;
    drop(executor);
    drop(peer);
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn websocket_successful_close_terminalizes_handle_and_prevents_reconnect(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let WebsocketTest {
        executor,
        component,
        ..
    } = WebsocketTest::start(deps, last_unique_id, host_api_tests).await?;
    let peer = TestPeer::start(Accept::Concurrently, PeerScript::SendThenDrain("msg-0")).await;
    let agent_id = agent_key!("WebsocketTest", "ws-close-terminal-replay");
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), peer.port_env(), Vec::new())
        .await?;
    let first: String = executor
        .invoke_and_await_typed(
            &component,
            &agent_id,
            "connect_and_receive_first",
            data_value!(peer.url()),
        )
        .await?;
    assert_eq!((first.as_str(), peer.accepted()), ("msg-0", 1));

    // The guest closes the handle, which makes it terminal at once.
    let close_result: Result<(), String> = executor
        .invoke_and_await_typed(
            &component,
            &agent_id,
            "close_persisted_result",
            data_value!(),
        )
        .await?;
    assert_eq!(close_result, Ok(()));
    let closed_error = receive_next_error(&executor, &component, &agent_id).await?;
    assert!(
        closed_error.contains("Closed"),
        "expected closed handle to become terminal immediately, got {closed_error}"
    );
    assert_eq!(peer.accepted(), 1);
    executor.check_oplog_is_queryable(&worker_id).await?;

    // Replay keeps the closed handle terminal and does not reconnect.
    let executor = executor.restart(Teardown::Drop).await?;
    let replayed_error = receive_next_error(&executor, &component, &agent_id).await?;
    assert!(
        replayed_error.contains("Closed"),
        "expected successfully closed websocket to remain terminal after replay, got {replayed_error}"
    );
    assert_eq!(
        peer.accepted(),
        1,
        "successfully closed websocket should not reconnect after replay"
    );
    executor.check_oplog_is_queryable(&worker_id).await?;
    drop(executor);
    drop(peer);
    Ok(())
}

fn replay_reconnect_roundtrip_params(
    url: String,
    promise_id_value: &SchemaValue,
) -> TypedSchemaValue {
    crate::raw_params(vec![SchemaValue::String(url), promise_id_value.clone()])
}

fn promise_id_from_value(value: &SchemaValue) -> PromiseId {
    PromiseId::from_value(value).expect("expected a promise ID returned by the guest")
}

/// The guest error of `receive_next_from_persisted_result`; fails when the receive succeeded.
async fn receive_next_error(
    executor: &TestWorkerExecutor,
    component: &ComponentDto,
    agent_id: &ParsedAgentKey,
) -> anyhow::Result<String> {
    let result: Result<String, String> = executor
        .invoke_and_await_typed(
            component,
            agent_id,
            "receive_next_from_persisted_result",
            data_value!(),
        )
        .await?;
    Ok(result.expect_err("expected the receive to fail"))
}

/// The promise ID value that the guest's `create_promise` returns.
async fn create_promise(
    executor: &TestWorkerExecutor,
    component: &ComponentDto,
    agent_id: &ParsedAgentKey,
) -> anyhow::Result<SchemaValue> {
    executor
        .invoke_and_await_agent(component, agent_id, "create_promise", data_value!())
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected return value"))
}

fn server_event(connection: usize, payload: &str) -> ServerEvent {
    ServerEvent {
        connection,
        payload: payload.to_string(),
    }
}

async fn wait_for_server_state(
    peer: &TestPeer,
    expected_connections: usize,
    expected_messages: usize,
) -> anyhow::Result<()> {
    let what = format!(
        "the websocket server to reach {expected_connections} connections and {expected_messages} messages"
    );
    within(SERVER_STATE_TIMEOUT, what, async {
        loop {
            let actual_connections = peer.accepted();
            let actual_messages = peer.transcript.lock().unwrap().len();

            if actual_connections > expected_connections || actual_messages > expected_messages {
                panic!(
                    "Websocket server state overshot: expected {expected_connections}/{expected_messages}, got {actual_connections}/{actual_messages}"
                );
            }

            if actual_connections == expected_connections && actual_messages == expected_messages {
                break;
            }

            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
}

#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn websocket_receive_with_timeout(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let WebsocketTest {
        executor,
        component,
        ..
    } = WebsocketTest::start(deps, last_unique_id, host_api_tests).await?;
    let peer = TestPeer::start(Accept::Once, PeerScript::Drain).await;
    let agent_id = agent_key!("WebsocketTest", "ws-timeout-test");
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), peer.port_env(), Vec::new())
        .await?;

    let received: Option<String> = executor
        .invoke_and_await_typed(
            &component,
            &agent_id,
            "receive_with_timeout_test",
            data_value!(peer.url(), 1000u64),
        )
        .await?;

    assert_eq!(received, None, "the silent peer lets the receive time out");
    executor.check_oplog_is_queryable(&worker_id).await?;
    drop(executor);
    drop(peer);
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn websocket_polling_test(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let WebsocketTest {
        executor,
        component,
        ..
    } = WebsocketTest::start(deps, last_unique_id, host_api_tests).await?;
    let message = "Hello from polling test";
    let peer = TestPeer::start(
        Accept::Once,
        PeerScript::Send {
            text: message,
            delay: Duration::ZERO,
        },
    )
    .await;
    let agent_id = agent_key!("WebsocketTest", "websocket-polling-test");
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), peer.port_env(), Vec::new())
        .await?;

    let polled: Result<String, String> = executor
        .invoke_and_await_typed(
            &component,
            &agent_id,
            "poll_for_message",
            data_value!(peer.url(), 1000u64),
        )
        .await?;

    assert_eq!(polled, Ok(message.to_string()));
    executor.check_oplog_is_queryable(&worker_id).await?;
    drop(executor);
    drop(peer);
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn websocket_polling_survives_repeated_timeouts(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let WebsocketTest {
        executor,
        component,
        ..
    } = WebsocketTest::start(deps, last_unique_id, host_api_tests).await?;
    let message = "delayed polling message";
    // The delay outlasts several of the guest's 10 ms poll timeouts.
    let peer = TestPeer::start(
        Accept::Once,
        PeerScript::Send {
            text: message,
            delay: Duration::from_millis(150),
        },
    )
    .await;
    let agent_id = agent_key!("WebsocketTest", "websocket-polling-timeout-race-test");
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), peer.port_env(), Vec::new())
        .await?;

    let polled: Result<String, String> = executor
        .invoke_and_await_typed(
            &component,
            &agent_id,
            "poll_until_message_after_timeouts",
            data_value!(peer.url(), 10u64, 30u32),
        )
        .await?;

    assert_eq!(polled, Ok(message.to_string()));
    executor.check_oplog_is_queryable(&worker_id).await?;
    drop(executor);
    drop(peer);
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn websocket_async_bidirectional_test(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let WebsocketTest {
        executor,
        component,
        ..
    } = WebsocketTest::start(deps, last_unique_id, host_api_tests).await?;
    // The echo peer serves several inbound and outbound messages on one connection.
    let peer = TestPeer::start(Accept::Once, PeerScript::Echo).await;
    let agent_id = agent_key!("WebsocketTest", "websocket-async-bidi-test");
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), peer.port_env(), Vec::new())
        .await?;

    let echoed: Result<String, String> = executor
        .invoke_and_await_typed(
            &component,
            &agent_id,
            "async_bidi_test",
            data_value!(peer.url()),
        )
        .await?;

    assert_eq!(echoed, Ok("msg-a|msg-b|msg-c".to_string()));
    executor.check_oplog_is_queryable(&worker_id).await?;
    drop(executor);
    drop(peer);
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn websocket_async_bidirectional_test_oplog_replay(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let WebsocketTest {
        executor,
        component,
        ..
    } = WebsocketTest::start(deps, last_unique_id, host_api_tests).await?;
    // The second invocation after the restart runs live again, so the peer keeps accepting.
    let peer = TestPeer::start(Accept::Serially, PeerScript::Echo).await;
    let agent_id = agent_key!("WebsocketTest", "websocket-async-bidi-oplog-replay");
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), peer.port_env(), Vec::new())
        .await?;

    let first: Result<String, String> = executor
        .invoke_and_await_typed(
            &component,
            &agent_id,
            "async_bidi_test",
            data_value!(peer.url()),
        )
        .await?;
    assert_eq!(first, Ok("msg-a|msg-b|msg-c".to_string()));
    executor.check_oplog_is_queryable(&worker_id).await?;

    let executor = executor.restart(Teardown::Drop).await?;
    let second: Result<String, String> = executor
        .invoke_and_await_typed(
            &component,
            &agent_id,
            "async_bidi_test",
            data_value!(peer.url()),
        )
        .await?;
    assert_eq!(second, Ok("msg-a|msg-b|msg-c".to_string()));
    executor.check_oplog_is_queryable(&worker_id).await?;

    drop(executor);
    drop(peer);
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn websocket_echo_ts(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("agent_sdk_ts")] agent_sdk_ts: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let WebsocketTest {
        executor,
        component,
        ..
    } = WebsocketTest::start(deps, last_unique_id, agent_sdk_ts).await?;
    let peer = TestPeer::start(Accept::Once, PeerScript::Echo).await;
    let agent_id = agent_key!("WebSocketTest", "ws-echo-test");
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), peer.port_env(), Vec::new())
        .await?;

    let echoed: String = executor
        .invoke_and_await_typed(
            &component,
            &agent_id,
            "echo",
            data_value!(peer.url(), "hello websocket"),
        )
        .await?;

    assert_eq!(echoed, "hello websocket");
    executor.check_oplog_is_queryable(&worker_id).await?;
    drop(executor);
    drop(peer);
    Ok(())
}

// ---- Per-handle reconnect coordination of concurrent receives ----
//
// The `receive` / `receive-with-timeout` helpers must coordinate the live
// reconnect of a replayed websocket handle per resource: at most one call
// performs the reconnect (pool permit + handshake + publish) while concurrent
// calls wait on the per-handle gate and re-read the entry afterwards. Without
// that coordination, concurrent receives each acquire pool permits
// independently; with a single permit, the first publishes its live connection
// holding the only permit for the connection's lifetime, the follower parks on
// the pool forever, and the worker never completes.
//
// The tests below observe the coordination through a real local websocket
// peer instead of instrumenting the executor: the production client runs one
// identical gate / pool permit / `connect_async` path in every test, and the
// peer withholds the 101 handshake response of the reconnect under test
// control, which parks the decided call inside its real `connect_async`
// handshake.

const RECONNECT_FIRST_MESSAGE: &str = "original";
const RECONNECT_MESSAGES: [&str; 2] = ["post-reconnect-a", "post-reconnect-b"];
const CONTENTION_METHOD: &str = "receive_lock_contention_from_persisted";
const CONTENTION_TIMEOUT_MS: u64 = 60_000;
const DIRECT_SEND_MESSAGE: &str = "direct-send-payload";
/// The frame that `connect_report_loss` sends and completes before the test crashes the
/// executor.
const COMPLETED_SEND_FRAME: &str = "completed-before-crash";
const WEBSOCKET_RECEIVE_FUNCTION: &str = "golem:websocket/client::receive";
const WEBSOCKET_SEND_FUNCTION: &str = "golem:websocket/client::send";
const WEBSOCKET_CLOSE_FUNCTION: &str = "golem:websocket/client::close";

/// Test-controlled resolution of a withheld websocket handshake.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HoldDirective {
    /// Upgrade requests read while holding park without a handshake response
    /// until the directive changes.
    Holding,
    /// A withheld handshake completes normally; later upgrade requests
    /// handshake immediately.
    Released,
    /// A withheld handshake is rejected with `403 Forbidden`; later upgrade
    /// requests handshake immediately.
    Rejected,
}

/// The real local peer's observable activity. A `FrameSent` event is recorded
/// only after its frame was successfully sent, so the recorded frame counts are
/// acknowledged per-frame evidence rather than send attempts.
#[derive(Clone, Debug, PartialEq, Eq)]
enum ReconnectServerEvent {
    /// A TCP connection was accepted.
    Accepted,
    /// A websocket upgrade request was fully read.
    UpgradeRequestRead,
    /// The 101 handshake response was written.
    HandshakeCompleted,
    /// The withheld handshake was rejected with an HTTP error.
    HandshakeRejected,
    /// The client of a withheld handshake vanished before the hold was
    /// resolved — an abandoned reconnect attempt.
    ClientVanishedWhileHeld,
    /// A text frame was sent and flushed successfully.
    FrameSent(String),
    /// A text frame was received.
    FrameReceived(String),
    /// A websocket close frame was received.
    CloseFrameReceived,
    /// The connection closed via error or EOF.
    ConnectionClosed,
}

type WsServerStream = WebSocketStream<TcpStream>;
type WsServerSink = SplitSink<WsServerStream, Message>;

/// State shared between the peer's listener and per-connection tasks.
#[derive(Clone)]
struct ServerShared {
    directive_tx: watch::Sender<HoldDirective>,
    directive_rx: watch::Receiver<HoldDirective>,
    events: Arc<Mutex<Vec<ReconnectServerEvent>>>,
    notify: Arc<tokio::sync::Notify>,
    errors: Arc<Mutex<Vec<String>>>,
    completed_handshakes: Arc<AtomicUsize>,
    reconnect_messages: &'static [&'static str],
}

fn emit(shared: &ServerShared, event: ReconnectServerEvent) {
    shared.events.lock().unwrap().push(event);
    shared.notify.notify_waiters();
}

/// A real local websocket peer with test-controlled handshake withholding.
///
/// The production client under test runs one identical gate / pool permit /
/// `connect_async` path in every test: instead of instrumenting the executor,
/// the peer reads the client's HTTP upgrade request and withholds the 101
/// response while the test holds it, which parks the client inside its real
/// `connect_async` handshake. Resolving the hold completes the withheld
/// handshake with a manually written 101 or rejects it with an HTTP error,
/// both under test control; the resolution applies to the withheld connection
/// and later upgrade requests handshake immediately.
///
/// Every accepted connection is served by an owned, joinable task, so stopping
/// the peer really closes every connection, and every task failure is
/// collected and surfaced to the test when the peer stops.
struct ReconnectTestServer {
    port: u16,
    shared: ServerShared,
    listener_task: Option<JoinHandle<()>>,
    connection_tasks: Arc<Mutex<Vec<JoinHandle<()>>>>,
}

impl ReconnectTestServer {
    async fn start() -> Self {
        Self::start_with_reconnect_messages(&RECONNECT_MESSAGES).await
    }

    async fn start_with_reconnect_messages(reconnect_messages: &'static [&'static str]) -> Self {
        let listener = TcpListener::bind("0.0.0.0:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let (directive_tx, directive_rx) = watch::channel(HoldDirective::Released);
        let shared = ServerShared {
            directive_tx: directive_tx.clone(),
            directive_rx,
            events: Arc::new(Mutex::new(Vec::new())),
            notify: Arc::new(tokio::sync::Notify::new()),
            errors: Arc::new(Mutex::new(Vec::new())),
            completed_handshakes: Arc::new(AtomicUsize::new(0)),
            reconnect_messages,
        };
        let connection_tasks = Arc::new(Mutex::new(Vec::new()));

        let listener_shared = shared.clone();
        let task_registry = connection_tasks.clone();
        let listener_task = spawn(
            async move {
                loop {
                    let Ok((stream, _)) = listener.accept().await else {
                        break;
                    };
                    emit(&listener_shared, ReconnectServerEvent::Accepted);
                    let connection_shared = listener_shared.clone();
                    let task = spawn(
                        async move {
                            let error_shared = connection_shared.clone();
                            if let Err(error) = serve_connection(stream, connection_shared).await {
                                error_shared
                                    .errors
                                    .lock()
                                    .unwrap()
                                    .push(format!("connection task failed: {error}"));
                                error_shared.notify.notify_waiters();
                            }
                        }
                        .in_current_span(),
                    );
                    task_registry.lock().unwrap().push(task);
                }
            }
            .in_current_span(),
        );

        Self {
            port,
            shared,
            listener_task: Some(listener_task),
            connection_tasks,
        }
    }

    fn url(&self) -> String {
        format!("ws://localhost:{}", self.port)
    }

    /// Withholds the handshake responses of upgrade requests that arrive from
    /// now on, until the hold is resolved.
    fn arm_hold(&self) {
        self.shared
            .directive_tx
            .send_replace(HoldDirective::Holding);
    }

    fn release_hold(&self) {
        self.shared
            .directive_tx
            .send_replace(HoldDirective::Released);
    }

    fn reject_hold(&self) {
        self.shared
            .directive_tx
            .send_replace(HoldDirective::Rejected);
    }

    fn count(&self, predicate: impl Fn(&ReconnectServerEvent) -> bool) -> usize {
        self.shared
            .events
            .lock()
            .unwrap()
            .iter()
            .filter(|event| predicate(event))
            .count()
    }

    fn accepted(&self) -> usize {
        self.count(|event| matches!(event, ReconnectServerEvent::Accepted))
    }

    fn completed_handshakes(&self) -> usize {
        self.count(|event| matches!(event, ReconnectServerEvent::HandshakeCompleted))
    }

    fn upgrade_requests_read(&self) -> usize {
        self.count(|event| matches!(event, ReconnectServerEvent::UpgradeRequestRead))
    }

    fn handshakes_rejected(&self) -> usize {
        self.count(|event| matches!(event, ReconnectServerEvent::HandshakeRejected))
    }

    fn clients_vanished_while_held(&self) -> usize {
        self.count(|event| matches!(event, ReconnectServerEvent::ClientVanishedWhileHeld))
    }

    fn close_frames_received(&self) -> usize {
        self.count(|event| matches!(event, ReconnectServerEvent::CloseFrameReceived))
    }

    fn frames_sent(&self) -> Vec<String> {
        self.shared
            .events
            .lock()
            .unwrap()
            .iter()
            .filter_map(|event| match event {
                ReconnectServerEvent::FrameSent(payload) => Some(payload.clone()),
                _ => None,
            })
            .collect()
    }

    fn frames_received(&self) -> Vec<String> {
        self.shared
            .events
            .lock()
            .unwrap()
            .iter()
            .filter_map(|event| match event {
                ReconnectServerEvent::FrameReceived(payload) => Some(payload.clone()),
                _ => None,
            })
            .collect()
    }

    /// Waits until at least `expected` recorded events match `predicate`.
    async fn wait_for_count(
        &self,
        expected: usize,
        predicate: impl Fn(&ReconnectServerEvent) -> bool,
        description: &str,
    ) -> anyhow::Result<()> {
        let deadline = Instant::now() + PEER_STATE_TIMEOUT;
        loop {
            // The notified future is created before the condition check so no
            // wake-up between the check and the await is missed.
            let notified = self.shared.notify.notified();
            {
                let errors = self.shared.errors.lock().unwrap();
                anyhow::ensure!(
                    errors.is_empty(),
                    "the reconnect test server recorded task errors while waiting to {description}: {errors:?}"
                );
            }
            let count = self.count(&predicate);
            if count >= expected {
                return Ok(());
            }
            let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
                anyhow::bail!(
                    "timed out waiting for the server to {description} \
                     (expected {expected} matching events, got {count}); \
                     events so far: {:?}",
                    self.shared.events.lock().unwrap()
                );
            };
            let _ = tokio::time::timeout(remaining, notified).await;
        }
    }

    /// Waits until the peer received the text frame `text`.
    async fn wait_for_frame(&self, text: &str, description: &str) -> anyhow::Result<()> {
        self.wait_for_count(
            1,
            |event| matches!(event, ReconnectServerEvent::FrameReceived(received) if received == text),
            description,
        )
        .await
    }

    /// Waits until the peer read `expected` websocket upgrade requests.
    async fn wait_for_upgrade_requests(
        &self,
        expected: usize,
        description: &str,
    ) -> anyhow::Result<()> {
        self.wait_for_count(
            expected,
            |event| matches!(event, ReconnectServerEvent::UpgradeRequestRead),
            description,
        )
        .await
    }

    /// Stops the peer: aborts and joins the listener and every owned connection
    /// task — which really closes every connection — and surfaces every task
    /// error recorded so far.
    async fn stop(&mut self) -> anyhow::Result<()> {
        if let Some(listener_task) = self.listener_task.take() {
            listener_task.abort();
            let _ = listener_task.await;
        }
        let connection_tasks = std::mem::take(&mut *self.connection_tasks.lock().unwrap());
        for task in connection_tasks {
            task.abort();
            let _ = task.await;
        }
        let errors = self.shared.errors.lock().unwrap();
        anyhow::ensure!(
            errors.is_empty(),
            "the reconnect test server recorded task errors: {errors:?}"
        );
        Ok(())
    }
}

/// Serves one accepted connection: reads the upgrade request, withholds or
/// completes the handshake according to the hold directive, sends the greeting
/// frames of the connection's handshake index and serves the frame loop until
/// the connection closes.
async fn serve_connection(mut stream: TcpStream, mut shared: ServerShared) -> Result<(), String> {
    let request = read_upgrade_request(&mut stream).await?;
    emit(&shared, ReconnectServerEvent::UpgradeRequestRead);
    let key = extract_websocket_key(&request)
        .ok_or_else(|| "the upgrade request carries no Sec-WebSocket-Key".to_string())?;

    let current_directive = *shared.directive_rx.borrow_and_update();
    let directive = match current_directive {
        HoldDirective::Holding => {
            // Withhold the handshake response while the test holds it. If the
            // client vanishes first — an abandoned reconnect attempt — the
            // withheld connection is simply dropped without a response.
            let mut vanish_probe = [0u8; 1];
            let resolved = tokio::select! {
                directive = shared
                    .directive_rx
                    .wait_for(|directive| *directive != HoldDirective::Holding) => {
                    Some(*directive.expect("the hold directive watch channel stays alive"))
                }
                read = stream.read(&mut vanish_probe) => {
                    let _ = read;
                    None
                }
            };
            match resolved {
                Some(directive) => directive,
                None => {
                    emit(&shared, ReconnectServerEvent::ClientVanishedWhileHeld);
                    return Ok(());
                }
            }
        }
        directive => directive,
    };

    match directive {
        HoldDirective::Rejected => {
            stream
                .write_all(b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\n\r\n")
                .await
                .map_err(|error| format!("writing the handshake rejection failed: {error}"))?;
            stream
                .flush()
                .await
                .map_err(|error| format!("flushing the handshake rejection failed: {error}"))?;
            emit(&shared, ReconnectServerEvent::HandshakeRejected);
            // The rejection applied to the withheld connection: later upgrade
            // requests handshake immediately.
            shared.directive_tx.send_replace(HoldDirective::Released);
            return Ok(());
        }
        HoldDirective::Holding => unreachable!("a withheld hold resolves to a different directive"),
        HoldDirective::Released => {
            let accept_key =
                tokio_tungstenite::tungstenite::handshake::derive_accept_key(key.as_bytes());
            let response = format!(
                "HTTP/1.1 101 Switching Protocols\r\n\
                 Upgrade: websocket\r\n\
                 Connection: Upgrade\r\n\
                 Sec-WebSocket-Accept: {accept_key}\r\n\
                 \r\n"
            );
            stream
                .write_all(response.as_bytes())
                .await
                .map_err(|error| format!("writing the 101 response failed: {error}"))?;
            stream
                .flush()
                .await
                .map_err(|error| format!("flushing the 101 response failed: {error}"))?;
        }
    }

    let handshake = shared.completed_handshakes.fetch_add(1, Ordering::SeqCst);
    emit(&shared, ReconnectServerEvent::HandshakeCompleted);

    let ws_stream = WebSocketStream::from_raw_socket(
        stream,
        tokio_tungstenite::tungstenite::protocol::Role::Server,
        None,
    )
    .await;
    let (mut write, mut read) = StreamExt::split(ws_stream);

    // The first completed handshake greets the initial connection; later ones
    // deliver the reconnected handle's messages.
    let greetings: &[&str] = if handshake == 0 {
        &[RECONNECT_FIRST_MESSAGE]
    } else {
        shared.reconnect_messages
    };
    for payload in greetings {
        if !send_frame(&mut write, &shared, payload).await? {
            return Ok(());
        }
    }

    loop {
        match StreamExt::next(&mut read).await {
            Some(Ok(message)) => match message {
                Message::Text(text) => {
                    emit(
                        &shared,
                        ReconnectServerEvent::FrameReceived(text.to_string()),
                    );
                }
                Message::Close(_) => {
                    let _ = SinkExt::send(&mut write, Message::Close(None)).await;
                    emit(&shared, ReconnectServerEvent::CloseFrameReceived);
                    return Ok(());
                }
                _ => {}
            },
            Some(Err(_)) | None => {
                emit(&shared, ReconnectServerEvent::ConnectionClosed);
                return Ok(());
            }
        }
    }
}

/// Sends one text frame and records it only after it was successfully sent.
///
/// Returns `false` and records `ConnectionClosed` when the client already went
/// away, as the frame loop does: a test can drop the executor, which closes its
/// socket, at any point after the handshake.
async fn send_frame(
    write: &mut WsServerSink,
    shared: &ServerShared,
    payload: &str,
) -> Result<bool, String> {
    use tokio_tungstenite::tungstenite::Error;
    match SinkExt::send(write, Message::text(payload)).await {
        Ok(()) => {
            emit(shared, ReconnectServerEvent::FrameSent(payload.to_string()));
            Ok(true)
        }
        Err(Error::ConnectionClosed | Error::AlreadyClosed) => {
            emit(shared, ReconnectServerEvent::ConnectionClosed);
            Ok(false)
        }
        Err(Error::Io(error))
            if matches!(
                error.kind(),
                std::io::ErrorKind::BrokenPipe | std::io::ErrorKind::ConnectionReset
            ) =>
        {
            emit(shared, ReconnectServerEvent::ConnectionClosed);
            Ok(false)
        }
        Err(error) => Err(format!("sending the frame {payload:?} failed: {error}")),
    }
}

/// Reads the client's HTTP websocket upgrade request up to its header
/// terminator.
async fn read_upgrade_request(stream: &mut TcpStream) -> Result<String, String> {
    let mut request = Vec::new();
    let mut chunk = [0u8; 1024];
    loop {
        if request.len() > 16 * 1024 {
            return Err("the websocket upgrade request exceeded 16 KiB".to_string());
        }
        if request.windows(4).any(|window| window == b"\r\n\r\n") {
            return Ok(String::from_utf8_lossy(&request).into_owned());
        }
        let read = stream
            .read(&mut chunk)
            .await
            .map_err(|error| format!("reading the websocket upgrade request failed: {error}"))?;
        if read == 0 {
            return Err(
                "the websocket upgrade request ended before its header terminator".to_string(),
            );
        }
        request.extend_from_slice(&chunk[..read]);
    }
}

fn extract_websocket_key(request: &str) -> Option<String> {
    request.split("\r\n").find_map(|line| {
        let (name, value) = line.split_once(':')?;
        if name.trim().eq_ignore_ascii_case("Sec-WebSocket-Key") {
            Some(value.trim().to_string())
        } else {
            None
        }
    })
}

/// The `AgentInvocationStarted` oplog index of the first run of the given
/// method, anchoring which durable-call and interrupt entries belong to that
/// invocation.
fn method_invocation_started(
    oplog: &[PublicOplogEntryWithIndex],
    method: &str,
) -> Option<OplogIndex> {
    oplog.iter().find_map(|entry| {
        let PublicOplogEntry::AgentInvocationStarted(params) = &entry.entry else {
            return None;
        };
        match &params.invocation {
            PublicAgentInvocation::AgentMethodInvocation(invocation)
                if invocation.method_name.replace('-', "_") == method.replace('-', "_") =>
            {
                Some(entry.oplog_index)
            }
            _ => None,
        }
    })
}

fn contention_invocation_started(oplog: &[PublicOplogEntryWithIndex]) -> Option<OplogIndex> {
    method_invocation_started(oplog, CONTENTION_METHOD)
}

/// The number of `Interrupted` oplog entries recorded after the given index:
/// one per delivered interrupt of the anchored invocation.
fn interrupted_entries_after(oplog: &[PublicOplogEntryWithIndex], after: OplogIndex) -> usize {
    oplog
        .iter()
        .filter(|entry| entry.oplog_index > after)
        .filter(|entry| matches!(&entry.entry, PublicOplogEntry::Interrupted(_)))
        .count()
}

/// The `Start` oplog indices of the given websocket client function's durable
/// calls, in oplog order.
fn starts_of_websocket_function(
    oplog: &[PublicOplogEntryWithIndex],
    function: &str,
) -> Vec<OplogIndex> {
    oplog
        .starts_of(function)
        .iter()
        .map(|entry| entry.oplog_index)
        .collect()
}

/// The receive-family durable `Start`s recorded after the contention run's
/// `AgentInvocationStarted`, as `(start index, function name)` pairs in oplog
/// order. A preserved durable session identity keeps exactly one `Start` per
/// function across interrupt and reconstruction.
fn contention_receive_starts(
    oplog: &[PublicOplogEntryWithIndex],
) -> Vec<(OplogIndex, &'static str)> {
    let Some(invocation_started) = contention_invocation_started(oplog) else {
        return Vec::new();
    };
    oplog
        .iter()
        .filter(|entry| entry.oplog_index > invocation_started)
        .filter_map(|entry| match &entry.entry {
            PublicOplogEntry::Start(params) => match params.function_name.as_str() {
                "golem:websocket/client::receive" => Some((entry.oplog_index, "receive")),
                "golem:websocket/client::receive-with-timeout" => {
                    Some((entry.oplog_index, "receive-with-timeout"))
                }
                _ => None,
            },
            _ => None,
        })
        .collect()
}

/// Asserts the contention run's oplog shows both receive-family `Start`s —
/// one `receive` and one `receive-with-timeout` — each still incomplete: no
/// `End` and no `Cancelled` terminal references them.
fn assert_contention_receives_incomplete(oplog: &[PublicOplogEntryWithIndex]) {
    let starts = contention_receive_starts(oplog);
    assert_eq!(
        starts.len(),
        2,
        "expected exactly one receive and one receive-with-timeout Start, got {starts:?}"
    );
    for (start, function) in starts {
        let call = oplog.call(start);
        assert!(
            call.ends.is_empty(),
            "the interrupted round must leave the {function} Start at {start:?} without an End"
        );
        assert!(
            call.cancellations.is_empty(),
            "the interrupted round must leave the {function} Start at {start:?} without a Cancelled"
        );
    }
}

/// Asserts the contention run's completed oplog keeps the durable session
/// identity of both receives and correlates them with their terminals and
/// deliveries: exactly one `Start` per receive function, exactly one `End` and
/// one `CompletionDelivered` per `Start` with no `Cancelled`, both deliveries
/// after their `End` and everything before the run's single
/// `AgentInvocationFinished`.
fn assert_contention_receive_correlation(oplog: &[PublicOplogEntryWithIndex]) {
    let starts = contention_receive_starts(oplog);
    assert_eq!(
        starts.len(),
        2,
        "expected exactly one receive and one receive-with-timeout Start, got {starts:?}"
    );
    let is_contention_finished = |entry: &&PublicOplogEntryWithIndex| {
        matches!(&entry.entry, PublicOplogEntry::AgentInvocationFinished(params)
            if params.method_name.as_deref().map(|name| name.replace('-', "_"))
                == Some(CONTENTION_METHOD.to_string()))
    };
    let finished = oplog
        .iter()
        .filter(|entry| is_contention_finished(entry))
        .collect::<Vec<_>>();
    assert_eq!(
        finished.len(),
        1,
        "expected exactly one finished contention invocation, got {}",
        finished.len()
    );
    let finished_index = finished[0].oplog_index;
    for (start, function) in &starts {
        let call = oplog.call(*start);
        let (ends, deliveries) = (&call.ends, &call.deliveries);
        assert_eq!(
            ends.len(),
            1,
            "expected exactly one End for the {function} Start at {start:?}"
        );
        assert_eq!(
            deliveries.len(),
            1,
            "expected exactly one CompletionDelivered for the {function} Start at {start:?}"
        );
        assert!(
            call.cancellations.is_empty(),
            "expected no Cancelled for the {function} Start at {start:?}"
        );
        assert!(
            ends[0].oplog_index < deliveries[0].oplog_index,
            "the {function} End must precede its CompletionDelivered"
        );
        assert!(
            deliveries[0].oplog_index < finished_index,
            "the {function} CompletionDelivered must precede the contention AgentInvocationFinished"
        );
    }
}

/// Asserts the given websocket client function's single durable call is still
/// incomplete: exactly one `Start`, with no `End` and no `Cancelled`
/// referencing it.
fn assert_single_websocket_call_incomplete(oplog: &[PublicOplogEntryWithIndex], function: &str) {
    let starts = starts_of_websocket_function(oplog, function);
    assert_eq!(
        starts.len(),
        1,
        "expected exactly one {function} Start, got {starts:?}"
    );
    let start = starts[0];
    let call = oplog.call(start);
    assert!(
        call.ends.is_empty(),
        "the interrupted {function} Start at {start:?} must have no End"
    );
    assert!(
        call.cancellations.is_empty(),
        "the interrupted {function} Start at {start:?} must have no Cancelled"
    );
}

/// Asserts the given websocket client function's single durable call kept its
/// durable session identity and completed: exactly one `Start` and one `End`
/// referencing it, with no `Cancelled`. The direct calls — `send` and `close` —
/// never record a `CompletionDelivered` marker even when the call spans an
/// interrupt: the guest task is synchronously blocked inside the host call, so
/// its delivery coincides with the host return and has no pre-delivery
/// divergence window. The marker records the accessor calls' separate
/// guest-facing delivery boundary, which the receive correlation asserts
/// cover.
fn assert_single_websocket_call_completed(oplog: &[PublicOplogEntryWithIndex], function: &str) {
    let starts = starts_of_websocket_function(oplog, function);
    assert_eq!(
        starts.len(),
        1,
        "expected exactly one {function} Start, got {starts:?}"
    );
    let start = starts[0];
    let call = oplog.call(start);
    assert_eq!(
        call.ends.len(),
        1,
        "expected exactly one End for the {function} Start at {start:?}"
    );
    assert!(
        call.cancellations.is_empty(),
        "expected no Cancelled for the {function} Start at {start:?}"
    );
}

/// The post-reconnect payloads both receives collect together, sorted so the
/// comparison does not depend on which receive won the reconnect coordination.
fn expected_reconnect_payloads() -> Vec<String> {
    let mut payloads = RECONNECT_MESSAGES
        .iter()
        .map(|payload| payload.to_string())
        .collect::<Vec<_>>();
    payloads.sort();
    payloads
}

/// Both receive results as a sorted multiset.
fn sorted_reconnect_payloads(result: &(String, String)) -> Vec<String> {
    let mut payloads = vec![result.0.clone(), result.1.clone()];
    payloads.sort();
    payloads
}

/// The greeting frames the peer must have sent: the initial connection's
/// greeting followed by the single reconnected connection's messages.
fn expected_reconnect_frames_sent() -> Vec<String> {
    [
        RECONNECT_FIRST_MESSAGE,
        RECONNECT_MESSAGES[0],
        RECONNECT_MESSAGES[1],
    ]
    .map(String::from)
    .to_vec()
}

/// Polls the worker's oplog until `predicate` holds. While the worker is
/// live, the oplog query API only exposes entries up to the running
/// invocation's committed protocol barrier, so polling is a deterministic
/// boundary only for committed entries such as `AgentInvocationStarted`; a
/// parked durable call's `Start` becomes queryable once an interrupt or
/// crash commits the oplog.
async fn wait_for_oplog(
    executor: &TestWorkerExecutor,
    worker_id: &OwnedAgentId,
    predicate: impl Fn(&[PublicOplogEntryWithIndex]) -> bool,
    description: &str,
    timeout: Duration,
) -> anyhow::Result<()> {
    let deadline = Instant::now() + timeout;
    loop {
        let oplog = executor.get_oplog(worker_id, OplogIndex::INITIAL).await?;
        if predicate(&oplog) {
            return Ok(());
        }
        anyhow::ensure!(
            Instant::now() < deadline,
            "timed out waiting for the worker's oplog to {description}; \
             oplog so far: {}",
            oplog.describe()
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// Waits for the worker to run the contention invocation and for that
/// invocation's committed `AgentInvocationStarted` entry, then settles
/// briefly.
///
/// The committed invocation boundary plus a short settle gives the guest time
/// to reach both receive parks: while the worker is live the parked
/// receive-family `Start`s are not exposed by the oplog query API, and the
/// interrupt that follows commits them, so the post-interrupt
/// incomplete-`Start` assertions prove loudly that both parks were open when
/// the interrupt landed.
async fn wait_for_contention_receives_to_park(
    executor: &TestWorkerExecutor,
    worker_id: &OwnedAgentId,
) -> anyhow::Result<()> {
    executor
        .wait_for_status(worker_id, AgentStatus::Running, STATUS_TIMEOUT)
        .await?;
    wait_for_oplog(
        executor,
        worker_id,
        |oplog| contention_invocation_started(oplog).is_some(),
        "commit the contention invocation's AgentInvocationStarted entry",
        OPLOG_PROGRESS_TIMEOUT,
    )
    .await?;
    tokio::time::sleep(QUEUE_SETTLE).await;
    Ok(())
}

/// Connects the agent's persisted handle to `server`: the first handshake greets the initial
/// live connection, whose handle the agent keeps in its state.
async fn connect_persisted_handle(
    executor: &TestWorkerExecutor,
    agent: &WebsocketAgent,
    server: &ReconnectTestServer,
) -> anyhow::Result<()> {
    let greeting: String = executor
        .invoke_and_await_typed(
            &agent.component,
            &agent.agent_id,
            "connect_and_receive_first",
            data_value!(server.url()),
        )
        .await?;
    assert_eq!(greeting, RECONNECT_FIRST_MESSAGE);
    assert_eq!(
        (server.accepted(), server.completed_handshakes()),
        (1, 1),
        "the initial connection"
    );
    Ok(())
}

/// Starts the contention invocation and waits until both receives park on the live
/// connection, on which the server sends nothing further. Returns the invocation's key.
async fn park_contention_receives(
    executor: &TestWorkerExecutor,
    agent: &WebsocketAgent,
) -> anyhow::Result<IdempotencyKey> {
    let key = IdempotencyKey::fresh();
    agent.start_contention(executor, &key).await?;
    wait_for_contention_receives_to_park(executor, &agent.worker_id).await?;
    Ok(key)
}

/// Interrupts the agent. The interrupt commits the oplog, so both abandoned receives are
/// recorded, and neither has an End or a Cancelled.
async fn interrupt_parked_receives(
    executor: &TestWorkerExecutor,
    agent: &WebsocketAgent,
) -> anyhow::Result<()> {
    executor.interrupt(&agent.worker_id).await?;
    executor
        .wait_for_status(&agent.worker_id, AgentStatus::Interrupted, STATUS_TIMEOUT)
        .await?;
    assert_contention_receives_incomplete(&agent.oplog(executor).await?);
    Ok(())
}

/// Concurrent `receive` and `receive-with-timeout` on a persisted websocket
/// handle must complete after an interrupt/reconstruct cycle, even with a
/// single-connection pool. Uncoordinated receives each take a pool permit
/// independently: the first publishes its live connection holding the only
/// permit, and the follower parks on the pool forever, so the retained
/// invocation never finishes and the worker never returns to idle. Per-handle
/// reconnect coordination lets one call reconnect while its concurrent peer
/// reuses the published entry, and both receives keep their durable session
/// identity: exactly one `Start` per function across the interrupt, completed
/// by one `End` each and delivered before the invocation finishes.
#[test]
#[tracing::instrument]
#[timeout("3m")]
async fn websocket_reconnect_concurrent_receives_complete_after_reconstruction(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let WebsocketTest {
        executor,
        component,
        ..
    } = WebsocketTest::start_with_connection_limit(deps, last_unique_id, host_api_tests, 1).await?;
    let mut ws_server = ReconnectTestServer::start().await;
    let agent = WebsocketAgent::start(&executor, &component, "ws-reconnect-contention").await?;
    connect_persisted_handle(&executor, &agent, &ws_server).await?;
    let contention_key = park_contention_receives(&executor, &agent).await?;

    // Interrupting abandons both receives with their `Start`s left incomplete.
    interrupt_parked_receives(&executor, &agent).await?;

    // Resuming replays the persisted handle as a `Replay` entry and retries the
    // retained invocation: both receives re-execute live and contend on
    // reconnecting the same handle. With a single-connection pool this only
    // completes if the reconnect is coordinated per handle; otherwise the
    // follower parks on the pool and the worker never returns to idle.
    executor.resume(&agent.worker_id, false).await?;
    executor
        .wait_for_status(&agent.worker_id, AgentStatus::Idle, IDLE_TIMEOUT)
        .await?;

    // The retained invocation finished exactly once; its recorded result holds
    // both post-reconnect payloads regardless of which receive reconnected.
    let result = agent.contention_result(&executor, &contention_key).await?;
    assert_eq!(
        sorted_reconnect_payloads(&result),
        expected_reconnect_payloads()
    );
    assert_contention_receive_correlation(&agent.oplog(&executor).await?);
    assert_eq!(
        (
            ws_server.accepted(),
            ws_server.completed_handshakes(),
            ws_server.frames_sent(),
            ws_server.frames_received()
        ),
        (2, 2, expected_reconnect_frames_sent(), Vec::<String>::new()),
        "exactly one reconnect connection, and both its greetings went to the receives"
    );

    // Re-invoking with the same key replays the recorded result without
    // opening new receive-family durable calls or a new connection.
    let replayed = agent.contention_result(&executor, &contention_key).await?;
    assert_eq!(replayed, result);
    assert_contention_receive_correlation(&agent.oplog(&executor).await?);
    assert_eq!(
        (ws_server.accepted(), ws_server.completed_handshakes()),
        (2, 2),
        "the replayed result opens no connection"
    );

    // Completed receives replay from their recorded `End`s without touching
    // the network: after evicting the idle worker and stopping the peer —
    // which really closes every connection — a fresh activation replays every
    // completed websocket call offline.
    assert!(executor.stop_worker_if_idle(&agent.worker_id).await?);
    ws_server.stop().await?;
    let noop: String = executor
        .invoke_and_await_typed(&component, &agent.agent_id, "noop", data_value!())
        .await?;
    assert_eq!(noop, "ok");
    assert_contention_receive_correlation(&agent.oplog(&executor).await?);
    assert_eq!(
        (ws_server.accepted(), ws_server.completed_handshakes()),
        (2, 2),
        "the offline replay opens no connection"
    );

    executor.check_oplog_is_queryable(&agent.worker_id).await?;
    drop(executor);
    Ok(())
}

/// The retry after an interrupt must coordinate the live reconnect of the
/// replayed handle per resource: exactly one accessor call decides to
/// reconnect while its concurrent peer queues on the per-handle gate, and the
/// decided call publishes the live connection for the queued call to reuse
/// instead of each call independently taking a pool permit and opening a
/// second connection. The real peer withholds the decided call's handshake
/// response under test control, which parks the decided call inside its real
/// `connect_async` handshake, so the coordination is observed at a
/// deterministic boundary: while the response is withheld exactly one
/// reconnect TCP connection exists and no handshake completed beyond the
/// initial one, and after the release both receives complete on the single
/// reconnected connection.
#[test]
#[tracing::instrument]
#[timeout("3m")]
async fn websocket_reconnect_coordination_admits_one_accessor_reconnect(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let WebsocketTest {
        executor,
        component,
        ..
    } = WebsocketTest::start_with_connection_limit(deps, last_unique_id, host_api_tests, 1).await?;
    let mut ws_server = ReconnectTestServer::start().await;
    let agent = WebsocketAgent::start(&executor, &component, "ws-reconnect-coordinate").await?;
    connect_persisted_handle(&executor, &agent, &ws_server).await?;
    let contention_key = park_contention_receives(&executor, &agent).await?;
    interrupt_parked_receives(&executor, &agent).await?;

    // Withhold the retry's reconnect handshake at the real peer: the decided
    // call parks inside its real `connect_async` handshake, and the queued
    // peer waits on the per-handle gate for the published entry.
    ws_server.arm_hold();
    executor.resume(&agent.worker_id, false).await?;
    ws_server
        .wait_for_upgrade_requests(2, "read the reconnect's upgrade request")
        .await?;
    // The queued call's position on the per-handle gate is internal to the
    // executor and cannot be observed from the peer; a short settle lets it
    // queue before the hold is resolved.
    tokio::time::sleep(QUEUE_SETTLE).await;

    // While the decided call's handshake is withheld, exactly one reconnect
    // TCP connection exists — the queued call did not open its own — and no
    // handshake completed beyond the initial one.
    assert_eq!(
        ws_server.accepted(),
        2,
        "only the decided call's reconnect connection may be accepted while its handshake is withheld"
    );
    assert_eq!(
        ws_server.completed_handshakes(),
        1,
        "only the initial connection's handshake may be completed while the reconnect's response is withheld"
    );
    assert_eq!(ws_server.upgrade_requests_read(), 2);

    // Completing the withheld handshake lets the decided call publish the live
    // connection and the queued call reuse it; both receives complete.
    ws_server.release_hold();
    executor
        .wait_for_status(&agent.worker_id, AgentStatus::Idle, IDLE_TIMEOUT)
        .await?;
    let result = agent.contention_result(&executor, &contention_key).await?;
    assert_eq!(
        sorted_reconnect_payloads(&result),
        expected_reconnect_payloads()
    );
    assert_contention_receive_correlation(&agent.oplog(&executor).await?);
    assert_eq!(
        (
            ws_server.accepted(),
            ws_server.completed_handshakes(),
            ws_server.frames_sent(),
            ws_server.frames_received()
        ),
        (2, 2, expected_reconnect_frames_sent(), Vec::<String>::new()),
        "exactly one reconnect connection, and both its greetings went to the receives"
    );

    // Re-invoking with the same key replays the recorded result without any
    // new connection.
    let replayed = agent.contention_result(&executor, &contention_key).await?;
    assert_eq!(replayed, result);
    assert_contention_receive_correlation(&agent.oplog(&executor).await?);
    assert_eq!(
        (ws_server.accepted(), ws_server.completed_handshakes()),
        (2, 2),
        "the replayed result opens no connection"
    );

    executor.check_oplog_is_queryable(&agent.worker_id).await?;
    drop(executor);
    ws_server.stop().await?;
    Ok(())
}

/// The reconnect coordination also governs the retry after an executor
/// restart: a crash while both receives are parked leaves their durable calls
/// incomplete, and the same-key invocation on a fresh executor recovers the
/// worker, replays the persisted handle as a `Replay` entry and re-runs both
/// receives. Only one accessor call reconnects the handle while its peer
/// queues on the per-handle gate and reuses the published entry: while the
/// peer withholds the reconnect's handshake response, exactly one reconnect
/// connection exists.
#[test]
#[tracing::instrument]
#[timeout("3m")]
async fn websocket_reconnect_coordination_after_executor_restart(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let WebsocketTest {
        executor,
        component,
        ..
    } = WebsocketTest::start_with_connection_limit(deps, last_unique_id, host_api_tests, 1).await?;
    let mut ws_server = ReconnectTestServer::start().await;
    let agent = WebsocketAgent::start(&executor, &component, "ws-reconnect-restart").await?;
    connect_persisted_handle(&executor, &agent, &ws_server).await?;
    let contention_key = park_contention_receives(&executor, &agent).await?;

    // Crashing the executor while both receives are parked abandons the
    // retained invocation with its durable calls left incomplete.
    let executor = executor.restart(Teardown::Shutdown).await?;

    // The same-key invocation on the fresh executor recovers the worker; the
    // recovered handle is a `Replay` entry, and withhold its reconnect
    // handshake at the real peer.
    ws_server.arm_hold();
    agent.start_contention(&executor, &contention_key).await?;
    ws_server
        .wait_for_upgrade_requests(2, "read the reconnect's upgrade request")
        .await?;
    // The queued call's position on the per-handle gate is internal to the
    // executor and cannot be observed from the peer; a short settle lets it
    // queue before the hold is resolved.
    tokio::time::sleep(QUEUE_SETTLE).await;

    assert_eq!(
        ws_server.accepted(),
        2,
        "only the decided call's reconnect connection may be accepted while its handshake is withheld"
    );
    assert_eq!(
        ws_server.completed_handshakes(),
        1,
        "only the initial connection's handshake may be completed while the reconnect's response is withheld"
    );
    assert_eq!(ws_server.upgrade_requests_read(), 2);

    ws_server.release_hold();
    executor
        .wait_for_status(&agent.worker_id, AgentStatus::Idle, IDLE_TIMEOUT)
        .await?;
    let result = agent.contention_result(&executor, &contention_key).await?;
    assert_eq!(
        sorted_reconnect_payloads(&result),
        expected_reconnect_payloads()
    );
    assert_contention_receive_correlation(&agent.oplog(&executor).await?);
    assert_eq!(
        (
            ws_server.accepted(),
            ws_server.completed_handshakes(),
            ws_server.frames_sent(),
            ws_server.frames_received()
        ),
        (2, 2, expected_reconnect_frames_sent(), Vec::<String>::new()),
        "exactly one reconnect connection, and both its greetings went to the receives"
    );

    executor.check_oplog_is_queryable(&agent.worker_id).await?;
    drop(executor);
    ws_server.stop().await?;
    Ok(())
}

/// The per-handle coordination bounds the reconnect to a single call even when
/// the pool has spare permits: without the coordination both concurrent
/// receives would each acquire a permit and open their own connection. With
/// it, one accessor call reconnects and its peer reuses the published entry,
/// so while the peer withholds the reconnect's handshake response only one
/// reconnect TCP connection exists even though a spare permit is available,
/// and after the release exactly one reconnect connection was opened.
#[test]
#[tracing::instrument]
#[timeout("3m")]
async fn websocket_reconnect_coordination_with_spare_pool_permits(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let WebsocketTest {
        executor,
        component,
        ..
    } = WebsocketTest::start_with_connection_limit(deps, last_unique_id, host_api_tests, 2).await?;
    let mut ws_server = ReconnectTestServer::start().await;
    let agent = WebsocketAgent::start(&executor, &component, "ws-reconnect-spare").await?;
    connect_persisted_handle(&executor, &agent, &ws_server).await?;
    let contention_key = park_contention_receives(&executor, &agent).await?;
    interrupt_parked_receives(&executor, &agent).await?;

    ws_server.arm_hold();
    executor.resume(&agent.worker_id, false).await?;
    ws_server
        .wait_for_upgrade_requests(2, "read the reconnect's upgrade request")
        .await?;
    tokio::time::sleep(QUEUE_SETTLE).await;

    // Even with a spare pool permit, only the decided call's reconnect
    // connection exists while its handshake is withheld: the queued call did
    // not open a connection of its own.
    assert_eq!(
        ws_server.accepted(),
        2,
        "even with a spare permit, only the decided call's reconnect connection may be accepted while its handshake is withheld"
    );
    assert_eq!(
        (
            ws_server.completed_handshakes(),
            ws_server.upgrade_requests_read()
        ),
        (1, 2),
        "the reconnect's handshake is still withheld"
    );

    ws_server.release_hold();
    executor
        .wait_for_status(&agent.worker_id, AgentStatus::Idle, IDLE_TIMEOUT)
        .await?;
    let result = agent.contention_result(&executor, &contention_key).await?;
    assert_eq!(
        sorted_reconnect_payloads(&result),
        expected_reconnect_payloads()
    );
    assert_contention_receive_correlation(&agent.oplog(&executor).await?);
    assert_eq!(
        (
            ws_server.accepted(),
            ws_server.completed_handshakes(),
            ws_server.frames_sent(),
            ws_server.frames_received()
        ),
        (2, 2, expected_reconnect_frames_sent(), Vec::<String>::new()),
        "even with a spare permit, only one new connection was opened: the queued call reused \
         the published entry, and the reconnected connection's greetings went to the receives"
    );

    executor.check_oplog_is_queryable(&agent.worker_id).await?;
    drop(executor);
    ws_server.stop().await?;
    Ok(())
}

/// A failing live reconnect must publish its terminal outcome per handle: the
/// decided accessor call parks its reconnect on the peer's withheld handshake
/// and the test rejects it with an HTTP error. The decided call terminally
/// closes the handle through the same re-verified publication window, its
/// queued peer re-reads the terminal entry and fails with the same error, the
/// pool permit the failed reconnect briefly held is released rather than
/// leaked — the next connection attempt on this executor succeeds — and later
/// connections to the same peer complete normally.
#[test]
#[tracing::instrument]
#[timeout("3m")]
async fn websocket_reconnect_failure_publishes_terminal_outcome(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let WebsocketTest {
        executor,
        component,
        ..
    } = WebsocketTest::start_with_connection_limit(deps, last_unique_id, host_api_tests, 1).await?;
    let mut ws_server = ReconnectTestServer::start().await;
    let agent = WebsocketAgent::start(&executor, &component, "ws-reconnect-terminal").await?;
    connect_persisted_handle(&executor, &agent, &ws_server).await?;
    let contention_key = park_contention_receives(&executor, &agent).await?;
    interrupt_parked_receives(&executor, &agent).await?;

    // Withhold the retry's reconnect handshake at the real peer and reject
    // it: the decided call's live handshake fails, and the failure is
    // published terminally per handle.
    ws_server.arm_hold();
    executor.resume(&agent.worker_id, false).await?;
    ws_server
        .wait_for_upgrade_requests(2, "read the reconnect's upgrade request")
        .await?;
    // The queued call's gate position is not observable from the peer; a
    // short settle lets it queue before the rejection.
    tokio::time::sleep(QUEUE_SETTLE).await;
    ws_server.reject_hold();
    executor
        .wait_for_status(&agent.worker_id, AgentStatus::Idle, IDLE_TIMEOUT)
        .await?;

    assert_eq!(ws_server.handshakes_rejected(), 1);
    let result = agent.contention_result(&executor, &contention_key).await?;
    for (side, payload) in [("receive", &result.0), ("receive-with-timeout", &result.1)] {
        assert!(
            payload.contains("Receive error") && payload.contains("ConnectionFailure"),
            "the {side} result must report the failed reconnect, got {payload:?}"
        );
    }
    assert_contention_receive_correlation(&agent.oplog(&executor).await?);
    assert_eq!(
        (
            ws_server.accepted(),
            ws_server.completed_handshakes(),
            ws_server.upgrade_requests_read(),
            ws_server.clients_vanished_while_held()
        ),
        (2, 1, 2, 0),
        "the rejected connection never completed its handshake, and the queued call never \
         opened one of its own"
    );

    // The failed reconnect released the pool permit it briefly held: the pool
    // admits a new connection.
    let pool = executor
        .websocket_connection_pool()
        .expect("the executor's websocket connection pool is captured");
    let permit = within(
        POOL_PERMIT_TIMEOUT,
        "the pool permit that the failed reconnect must not leak",
        pool.acquire(),
    )
    .await??;
    drop(permit);

    // A fresh connection to the same peer completes normally after the
    // rejection.
    let connected: Result<(), String> = executor
        .invoke_and_await_typed(
            &component,
            &agent.agent_id,
            "connect_result",
            data_value!(ws_server.url()),
        )
        .await?;
    assert_eq!(connected, Ok(()));
    assert_eq!(
        (ws_server.accepted(), ws_server.completed_handshakes()),
        (3, 2),
        "the fresh connection completes its handshake"
    );

    executor.check_oplog_is_queryable(&agent.worker_id).await?;
    drop(executor);
    ws_server.stop().await?;
    Ok(())
}

/// The reconnect coordination is scoped per resource handle: two workers, each
/// with its own persisted websocket on its own peer, reconnect their own
/// handles concurrently. While both reconnect handshakes are withheld at their
/// own peers, each peer sees exactly one reconnect connection beyond its
/// initial one and no completed handshake beyond the initial one — neither
/// worker's queued call opens a connection of its own — and after both
/// releases both pairs of receives complete.
#[test]
#[tracing::instrument]
#[timeout("4m")]
async fn websocket_reconnect_coordination_is_per_handle(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let WebsocketTest {
        executor,
        component,
        ..
    } = WebsocketTest::start_with_connection_limit(deps, last_unique_id, host_api_tests, 2).await?;
    let mut ws_server_a = ReconnectTestServer::start().await;
    let mut ws_server_b = ReconnectTestServer::start().await;
    let agent_a = WebsocketAgent::start(&executor, &component, "ws-reconnect-per-handle-a").await?;
    let agent_b = WebsocketAgent::start(&executor, &component, "ws-reconnect-per-handle-b").await?;
    connect_persisted_handle(&executor, &agent_a, &ws_server_a).await?;
    connect_persisted_handle(&executor, &agent_b, &ws_server_b).await?;

    let [key_a, key_b] = park_and_interrupt_both(&executor, [&agent_a, &agent_b]).await?;
    hold_both_reconnects(
        &executor,
        [(&agent_a, &ws_server_a, "a"), (&agent_b, &ws_server_b, "b")],
    )
    .await?;

    // After both releases, both pairs of receives complete on one reconnect each.
    ws_server_a.release_hold();
    ws_server_b.release_hold();
    for agent in [&agent_a, &agent_b] {
        executor
            .wait_for_status(&agent.worker_id, AgentStatus::Idle, IDLE_TIMEOUT)
            .await?;
    }
    for (agent, key) in [(&agent_a, &key_a), (&agent_b, &key_b)] {
        let result = agent.contention_result(&executor, key).await?;
        assert_eq!(
            sorted_reconnect_payloads(&result),
            expected_reconnect_payloads()
        );
    }
    for agent in [&agent_a, &agent_b] {
        assert_contention_receive_correlation(&agent.oplog(&executor).await?);
    }
    for server in [&ws_server_a, &ws_server_b] {
        assert_eq!(
            (
                server.accepted(),
                server.completed_handshakes(),
                server.frames_sent()
            ),
            (2, 2, expected_reconnect_frames_sent()),
            "each handle reconnects once and its greetings go to its receives"
        );
    }

    executor
        .check_oplog_is_queryable(&agent_a.worker_id)
        .await?;
    executor
        .check_oplog_is_queryable(&agent_b.worker_id)
        .await?;
    drop(executor);
    ws_server_a.stop().await?;
    ws_server_b.stop().await?;
    Ok(())
}

/// Starts the contention invocations of both agents, waits until all four receives park,
/// then interrupts both agents. Returns the invocations' keys.
async fn park_and_interrupt_both(
    executor: &TestWorkerExecutor,
    agents: [&WebsocketAgent; 2],
) -> anyhow::Result<[IdempotencyKey; 2]> {
    let keys = [IdempotencyKey::fresh(), IdempotencyKey::fresh()];
    for (agent, key) in agents.iter().zip(&keys) {
        agent.start_contention(executor, key).await?;
    }
    for agent in agents {
        wait_for_contention_receives_to_park(executor, &agent.worker_id).await?;
    }
    for agent in agents {
        executor.interrupt(&agent.worker_id).await?;
    }
    for agent in agents {
        executor
            .wait_for_status(&agent.worker_id, AgentStatus::Interrupted, STATUS_TIMEOUT)
            .await?;
    }
    for agent in agents {
        assert_contention_receives_incomplete(&agent.oplog(executor).await?);
    }
    Ok(keys)
}

/// Withholds both reconnect handshakes at their own peers and resumes both agents. Each
/// handle sees exactly one decided reconnect and one queued call: the coordination neither
/// starves nor merges the two handles.
async fn hold_both_reconnects(
    executor: &TestWorkerExecutor,
    handles: [(&WebsocketAgent, &ReconnectTestServer, &str); 2],
) -> anyhow::Result<()> {
    for (_, server, _) in handles {
        server.arm_hold();
    }
    for (agent, _, _) in handles {
        executor.resume(&agent.worker_id, false).await?;
    }
    for (_, server, worker) in handles {
        server
            .wait_for_upgrade_requests(
                2,
                &format!("read worker {worker}'s reconnect upgrade request"),
            )
            .await?;
    }
    // The queued calls' gate positions are internal to the executor and
    // cannot be observed from the peers; a short settle lets them queue
    // before the holds are resolved.
    tokio::time::sleep(QUEUE_SETTLE).await;
    for (_, server, worker) in handles {
        assert_eq!(
            server.accepted(),
            2,
            "the {worker} handle must see exactly one reconnect connection while its handshake is withheld"
        );
        assert_eq!(
            server.completed_handshakes(),
            1,
            "the {worker} handle's reconnect handshake must still be withheld"
        );
    }
    Ok(())
}

/// While the decided accessor call is parked acquiring the one-slot pool's
/// permit — the test itself holds the only permit — and its same-handle peer
/// is queued on the per-handle gate, an interrupt abandons both calls before
/// any reconnect connection is opened: no reconnect TCP connection appears,
/// the receives keep their original incomplete `Start`s with typed
/// `Interrupted` entries recorded, and after the test returns the permit the
/// retry after the resume reconnects on the same retained invocation and
/// completes both receives through the single pool slot: the capacity is
/// reused, not leaked.
#[test]
#[tracing::instrument]
#[timeout("4m")]
async fn websocket_reconnect_coordination_is_interruptible_while_pending_on_pool_permit(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let WebsocketTest {
        executor,
        component,
        ..
    } = WebsocketTest::start_with_connection_limit(deps, last_unique_id, host_api_tests, 1).await?;
    let mut ws_server = ReconnectTestServer::start().await;
    let agent =
        WebsocketAgent::start(&executor, &component, "ws-reconnect-interruptible-pool").await?;
    connect_persisted_handle(&executor, &agent, &ws_server).await?;
    let contention_key = park_contention_receives(&executor, &agent).await?;
    interrupt_parked_receives(&executor, &agent).await?;

    // Take the pool's only permit from the captured pool before the retry:
    // the decided call parks acquiring it, before opening any connection. The
    // initial connection's permit returned to the pool when the interrupt
    // dropped the worker instance.
    let pool = executor
        .websocket_connection_pool()
        .expect("the executor's websocket connection pool is captured");
    let held_permit = within(
        POOL_PERMIT_TIMEOUT,
        "the initial connection's permit, back in the pool after the interrupt",
        pool.acquire(),
    )
    .await??;
    executor.resume(&agent.worker_id, false).await?;
    // The decided call's pool-park position inside the executor is not
    // directly observable — no connection is opened before the permit — and
    // its peer's gate position is internal as well; a short settle lets both
    // park before the interrupt.
    tokio::time::sleep(QUEUE_SETTLE).await;
    // No reconnect connection was opened while the pool permit was
    // unavailable: the decided call parks on the pool and its peer queues on
    // the per-handle gate.
    assert_eq!(
        ws_server.accepted(),
        1,
        "no reconnect connection may be opened while the pool's only permit is held"
    );

    interrupt_parked_receives(&executor, &agent).await?;
    assert_two_interrupts_recorded(&agent.oplog(&executor).await?)?;
    assert_eq!(
        ws_server.accepted(),
        1,
        "no reconnect connection may be opened while the pool's only permit is held"
    );

    // Return the permit and resume: the same retained invocation retries, the
    // decided call acquires the freed permit and the reconnect completes.
    drop(held_permit);
    executor.resume(&agent.worker_id, false).await?;
    executor
        .wait_for_status(&agent.worker_id, AgentStatus::Idle, IDLE_TIMEOUT)
        .await?;
    let result = agent.contention_result(&executor, &contention_key).await?;
    assert_eq!(
        sorted_reconnect_payloads(&result),
        expected_reconnect_payloads()
    );
    assert_contention_receive_correlation(&agent.oplog(&executor).await?);
    assert_eq!(
        (
            ws_server.accepted(),
            ws_server.completed_handshakes(),
            ws_server.clients_vanished_while_held(),
            ws_server.frames_sent()
        ),
        (2, 2, 0, expected_reconnect_frames_sent()),
        "the retry reused the single pool slot exactly once: one reconnect connection, and no \
         interrupted reconnect attempt leaked or lingered as a held client at the peer"
    );

    executor.check_oplog_is_queryable(&agent.worker_id).await?;
    drop(executor);
    ws_server.stop().await?;
    Ok(())
}

/// Asserts that each of the two interrupts of the contention invocation recorded a typed
/// `Interrupted` entry.
fn assert_two_interrupts_recorded(oplog: &[PublicOplogEntryWithIndex]) -> anyhow::Result<()> {
    let contention_started = contention_invocation_started(oplog)
        .ok_or_else(|| anyhow!("the contention invocation never started"))?;
    assert!(
        interrupted_entries_after(oplog, contention_started) >= 2,
        "each of the two interrupts must record a typed Interrupted entry"
    );
    Ok(())
}

/// While the decided accessor call is parked on the real peer-held websocket
/// handshake and its same-handle peer is queued on the per-handle gate, an
/// interrupt abandons both calls: the peer observes the abandoned held
/// connection, the receives keep their original incomplete `Start`s with typed
/// `Interrupted` entries recorded, and the abandoned reconnect releases the
/// pool permit it held while parked on the handshake. The retry after the
/// resume reconnects on the same retained invocation and completes both
/// receives: the pool capacity is reused, not leaked.
#[test]
#[tracing::instrument]
#[timeout("4m")]
async fn websocket_reconnect_coordination_is_interruptible_while_pending_on_handshake(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let WebsocketTest {
        executor,
        component,
        ..
    } = WebsocketTest::start_with_connection_limit(deps, last_unique_id, host_api_tests, 1).await?;
    let mut ws_server = ReconnectTestServer::start().await;
    let agent = WebsocketAgent::start(&executor, &component, "ws-reconnect-interruptible").await?;
    connect_persisted_handle(&executor, &agent, &ws_server).await?;
    let contention_key = park_contention_receives(&executor, &agent).await?;
    interrupt_parked_receives(&executor, &agent).await?;

    interrupt_a_reconnect_held_on_its_handshake(&executor, &agent, &ws_server).await?;

    // Resolve the abandoned hold and resume: the same retained invocation
    // retries, the reconnect completes on a fresh connection and both
    // receives return.
    ws_server.release_hold();
    executor.resume(&agent.worker_id, false).await?;
    executor
        .wait_for_status(&agent.worker_id, AgentStatus::Idle, IDLE_TIMEOUT)
        .await?;
    let result = agent.contention_result(&executor, &contention_key).await?;
    assert_eq!(
        sorted_reconnect_payloads(&result),
        expected_reconnect_payloads()
    );
    assert_contention_receive_correlation(&agent.oplog(&executor).await?);
    assert_eq!(
        (
            ws_server.accepted(),
            ws_server.completed_handshakes(),
            ws_server.clients_vanished_while_held(),
            ws_server.frames_sent(),
            ws_server.frames_received()
        ),
        (
            3,
            2,
            1,
            expected_reconnect_frames_sent(),
            Vec::<String>::new()
        ),
        "the abandoned held connection never completed its handshake; the retry opened one \
         fresh connection, and its greetings went to the receives"
    );

    // Re-invoking with the same key replays the recorded result without any
    // new connection.
    let replayed = agent.contention_result(&executor, &contention_key).await?;
    assert_eq!(replayed, result);
    assert_contention_receive_correlation(&agent.oplog(&executor).await?);
    assert_eq!(
        (ws_server.accepted(), ws_server.completed_handshakes()),
        (3, 2),
        "the replayed result opens no connection"
    );

    executor.check_oplog_is_queryable(&agent.worker_id).await?;
    drop(executor);
    ws_server.stop().await?;
    Ok(())
}

/// Withholds the retry's reconnect handshake at the real peer, so the decided call parks
/// inside its real `connect_async` handshake and its peer queues on the per-handle gate, then
/// interrupts both. The peer observes the abandoned held connection, both receives keep their
/// incomplete `Start`s, and the abandoned reconnect returns its pool permit.
async fn interrupt_a_reconnect_held_on_its_handshake(
    executor: &TestWorkerExecutor,
    agent: &WebsocketAgent,
    ws_server: &ReconnectTestServer,
) -> anyhow::Result<()> {
    ws_server.arm_hold();
    executor.resume(&agent.worker_id, false).await?;
    ws_server
        .wait_for_upgrade_requests(2, "read the reconnect's upgrade request")
        .await?;
    // The queued call's gate position is internal to the executor and cannot
    // be observed from the peer; a short settle lets it queue before the
    // interrupt.
    tokio::time::sleep(QUEUE_SETTLE).await;
    assert_eq!(
        (ws_server.accepted(), ws_server.completed_handshakes()),
        (2, 1),
        "the decided call's reconnect connection waits for its withheld handshake"
    );

    // Interrupting abandons both parked calls without resolving the hold.
    executor.interrupt(&agent.worker_id).await?;
    executor
        .wait_for_status(&agent.worker_id, AgentStatus::Interrupted, STATUS_TIMEOUT)
        .await?;
    ws_server
        .wait_for_count(
            1,
            |event| matches!(event, ReconnectServerEvent::ClientVanishedWhileHeld),
            "observe the abandoned held connection",
        )
        .await?;
    let oplog = agent.oplog(executor).await?;
    assert_contention_receives_incomplete(&oplog);
    assert_two_interrupts_recorded(&oplog)?;

    // The abandoned reconnect released the pool permit it held while parked on
    // the withheld handshake.
    let pool = executor
        .websocket_connection_pool()
        .expect("the executor's websocket connection pool is captured");
    let permit = within(
        POOL_PERMIT_TIMEOUT,
        "the pool permit that the interrupted reconnect must not leak",
        pool.acquire(),
    )
    .await??;
    drop(permit);
    Ok(())
}

/// A direct `send` on a persisted handle reconstructed after an executor
/// restart reconnects through the same coordination as the accessors, and an
/// interrupt while its reconnect handshake is withheld at the real peer
/// abandons it: the peer observes the abandoned held connection, the send
/// keeps its original incomplete durable `Start` with a typed `Interrupted`
/// entry recorded, and the retry after the resume reconnects on the same
/// retained invocation, delivers the frame to the peer and keeps the send's
/// durable session identity.
#[test]
#[tracing::instrument]
#[timeout("3m")]
async fn websocket_reconnect_direct_send_is_interruptible_and_continues(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let WebsocketTest {
        executor,
        component,
        ..
    } = WebsocketTest::start_with_connection_limit(deps, last_unique_id, host_api_tests, 1).await?;
    let mut ws_server = ReconnectTestServer::start().await;
    let agent = WebsocketAgent::start(&executor, &component, "ws-reconnect-direct-send").await?;
    connect_persisted_handle(&executor, &agent, &ws_server).await?;

    // Crash the executor while the worker is idle: the persisted handle's
    // connection is lost, so the send below reconnects it on the fresh
    // executor.
    let executor = executor.restart(Teardown::Shutdown).await?;

    // Withhold the send's reconnect handshake at the real peer: the send parks
    // inside its real `connect_async` handshake.
    ws_server.arm_hold();
    let send_key = IdempotencyKey::fresh();
    executor
        .invoke_agent_with_key(
            &component,
            &agent.agent_id,
            &send_key,
            "send_persisted_result",
            data_value!(DIRECT_SEND_MESSAGE),
        )
        .await?;
    // The peer reading the send's reconnect upgrade request is the
    // deterministic boundary proving the send's durable call is parked in its
    // real `connect_async` handshake: the `Start` of a live parked durable
    // call is not exposed by the oplog query API, and the interrupt below
    // commits it for the assertions that follow.
    ws_server
        .wait_for_upgrade_requests(2, "read the send's reconnect upgrade request")
        .await?;
    executor.interrupt(&agent.worker_id).await?;
    executor
        .wait_for_status(&agent.worker_id, AgentStatus::Interrupted, STATUS_TIMEOUT)
        .await?;
    // The abandoned client of the withheld handshake is observed at the peer.
    ws_server
        .wait_for_count(
            1,
            |event| matches!(event, ReconnectServerEvent::ClientVanishedWhileHeld),
            "observe the abandoned held connection",
        )
        .await?;
    let oplog = agent.oplog(&executor).await?;
    assert_single_websocket_call_incomplete(&oplog, WEBSOCKET_SEND_FUNCTION);
    let send_started = starts_of_websocket_function(&oplog, WEBSOCKET_SEND_FUNCTION)
        .first()
        .copied()
        .ok_or_else(|| anyhow!("the send's durable Start is missing"))?;
    assert!(
        interrupted_entries_after(&oplog, send_started) >= 1,
        "the interrupt must record a typed Interrupted entry after the send's durable Start"
    );

    // The pool captured from the fresh executor only proves permit accounting
    // on that executor; the retry below reconnecting through the single pool
    // slot — exactly one further connection — is the reuse evidence.

    // Resolve the abandoned hold and resume: the same retained invocation
    // retries, the send reconnects and delivers its frame to the peer.
    ws_server.release_hold();
    executor.resume(&agent.worker_id, false).await?;
    executor
        .wait_for_status(&agent.worker_id, AgentStatus::Idle, IDLE_TIMEOUT)
        .await?;
    let sent: Result<(), String> = executor
        .invoke_and_await_typed_with_key(
            &component,
            &agent.agent_id,
            &send_key,
            "send_persisted_result",
            data_value!(DIRECT_SEND_MESSAGE),
        )
        .await?;
    assert_eq!(sent, Ok(()));
    // The send is a direct call: its guest is blocked inside the host call, so
    // the completed call records one Start and one End with no
    // CompletionDelivered marker even though it spans the interrupt.
    assert_single_websocket_call_completed(&agent.oplog(&executor).await?, WEBSOCKET_SEND_FUNCTION);
    assert_eq!(
        (
            ws_server.accepted(),
            ws_server.completed_handshakes(),
            ws_server.clients_vanished_while_held(),
            ws_server.frames_sent(),
            ws_server.frames_received()
        ),
        (
            3,
            2,
            1,
            expected_reconnect_frames_sent(),
            vec![DIRECT_SEND_MESSAGE.to_string()]
        ),
        "the abandoned held connection never completed its handshake; the retry opened one \
         fresh connection, sent its greetings and received the send's frame on it"
    );

    // Re-invoking with the same key replays the recorded result without any
    // new connection or frame.
    let replayed: Result<(), String> = executor
        .invoke_and_await_typed_with_key(
            &component,
            &agent.agent_id,
            &send_key,
            "send_persisted_result",
            data_value!(DIRECT_SEND_MESSAGE),
        )
        .await?;
    assert_eq!(replayed, Ok(()));
    assert_single_websocket_call_completed(&agent.oplog(&executor).await?, WEBSOCKET_SEND_FUNCTION);
    assert_eq!(
        (
            ws_server.accepted(),
            ws_server.completed_handshakes(),
            ws_server.frames_received()
        ),
        (3, 2, vec![DIRECT_SEND_MESSAGE.to_string()]),
        "the replayed result opens no connection and sends no frame"
    );

    executor.check_oplog_is_queryable(&agent.worker_id).await?;
    drop(executor);
    ws_server.stop().await?;
    Ok(())
}

/// A direct `close` on a persisted handle reconstructed after an executor
/// restart reconnects through the same coordination as the accessors, closes
/// the reconnected connection with a websocket close frame the peer observes
/// and publishes a terminal outcome: a later `receive` on the
/// same handle fails without opening any new connection — the terminalized
/// handle does not reconnect.
#[test]
#[tracing::instrument]
#[timeout("3m")]
async fn websocket_reconnect_direct_close_reconnects_and_terminalizes(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let WebsocketTest {
        executor,
        component,
        ..
    } = WebsocketTest::start_with_connection_limit(deps, last_unique_id, host_api_tests, 1).await?;
    // Close does not read unsolicited greetings on the reconnected transport.
    let mut ws_server = ReconnectTestServer::start_with_reconnect_messages(&[]).await;
    let agent = WebsocketAgent::start(&executor, &component, "ws-reconnect-direct-close").await?;
    connect_persisted_handle(&executor, &agent, &ws_server).await?;

    // Crash the executor while the worker is idle: the persisted handle's
    // connection is lost, so the close below reconnects it on the fresh
    // executor.
    let executor = executor.restart(Teardown::Shutdown).await?;
    let close_key = IdempotencyKey::fresh();
    let closed: Result<(), String> = executor
        .invoke_and_await_typed_with_key(
            &component,
            &agent.agent_id,
            &close_key,
            "close_persisted_result",
            data_value!(),
        )
        .await?;
    assert_eq!(closed, Ok(()));
    assert_single_websocket_call_completed(
        &agent.oplog(&executor).await?,
        WEBSOCKET_CLOSE_FUNCTION,
    );
    // Completing close does not wait for the peer to observe the frame.
    ws_server
        .wait_for_count(
            1,
            |event| matches!(event, ReconnectServerEvent::CloseFrameReceived),
            "observe the close frame on the reconnected connection",
        )
        .await?;
    assert_eq!(
        (
            ws_server.accepted(),
            ws_server.completed_handshakes(),
            ws_server.close_frames_received(),
            ws_server.frames_sent()
        ),
        (2, 2, 1, vec![RECONNECT_FIRST_MESSAGE.to_string()]),
        "the close reconnected on one fresh connection, and the peer observed its close frame"
    );

    // A receive on the terminalized handle fails with the closed-connection
    // error and must not reconnect: no new connection is opened.
    let message = receive_next_error(&executor, &component, &agent.agent_id).await?;
    assert!(
        message.contains("Receive error") && message.contains("Closed"),
        "expected a closed-connection receive error, got {message:?}"
    );
    assert_eq!(
        ws_server.accepted(),
        2,
        "the terminal handle must not reconnect"
    );
    assert_eq!(
        (ws_server.close_frames_received(), ws_server.frames_sent()),
        (1, vec![RECONNECT_FIRST_MESSAGE.to_string()]),
        "the terminal handle sends no frame"
    );

    executor.check_oplog_is_queryable(&agent.worker_id).await?;
    drop(executor);
    ws_server.stop().await?;
    Ok(())
}
