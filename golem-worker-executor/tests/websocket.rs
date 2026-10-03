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
use futures::{SinkExt, StreamExt};
use golem_common::model::oplog::{
    OplogIndex, PublicAgentInvocation, PublicOplogEntry, PublicOplogEntryWithIndex,
};
use golem_common::model::{AgentStatus, IdempotencyKey, OwnedAgentId, PromiseId};
use golem_common::schema::SchemaValue;
use golem_common::schema::schema_value::ResultValuePayload;
use golem_common::{agent_id, data_value};
use golem_test_framework::dsl::TestDsl;
use golem_worker_executor::durable_host::websocket::reconnect_test::{
    WebSocketReconnectEventForTest, WebSocketReconnectEventKindForTest,
    WebSocketReconnectObservationForTest, WebSocketReconnectOutcomeForTest,
    WebSocketReconnectPathForTest, WebSocketReconnectWaitForTest,
    WebSocketReconnectWaitStateForTest,
};
use golem_worker_executor_test_utils::{
    LastUniqueId, PrecompiledComponent, TestContext, TestExecutorOverrides,
    WorkerExecutorTestDependencies, start, start_with_overrides,
};
use pretty_assertions::assert_eq;
use std::collections::HashMap;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};
use std::time::Duration;
use test_r::{inherit_test_dep, test, timeout};
use tokio::spawn;
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

#[derive(Debug, Clone, PartialEq, Eq)]
struct ServerEvent {
    connection: usize,
    payload: String,
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
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await.unwrap();
    let ws_port = listener.local_addr().unwrap().port();

    let ws_server = spawn(
        async move {
            if let Ok((stream, _)) = listener.accept().await {
                let ws_stream = tokio_tungstenite::accept_async(stream)
                    .await
                    .expect("WS handshake failed");
                let (mut write, mut read) = StreamExt::split(ws_stream);
                while let Some(Ok(msg)) = StreamExt::next(&mut read).await {
                    if msg.is_close() {
                        break;
                    }
                    if msg.is_text() || msg.is_binary() {
                        SinkExt::send(&mut write, msg).await.ok();
                    }
                }
            }
        }
        .in_current_span(),
    );

    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;

    let mut env_vars = HashMap::new();
    env_vars.insert("WS_PORT".to_string(), ws_port.to_string());

    let agent_id = agent_id!("WebsocketTest", "ws-echo-test");
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), env_vars, Vec::new())
        .await?;

    let result = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "echo",
            data_value!(format!("ws://localhost:{ws_port}"), "hello websocket"),
        )
        .await?;

    assert_eq!(result.into_typed::<String>()?, "hello websocket");

    executor.check_oplog_is_queryable(&worker_id).await?;

    drop(executor);
    ws_server.abort();

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
    let context = TestContext::new(last_unique_id);

    // First executor instance + WebSocket echo server
    let executor = start(deps, &context).await?;

    let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await.unwrap();
    let ws_port = listener.local_addr().unwrap().port();

    // Accept many connections: the second invocation after executor restart
    // performs a new live connect; a single accept() would leave nothing listening.
    let ws_server = spawn(
        async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    break;
                };
                let ws_stream = tokio_tungstenite::accept_async(stream)
                    .await
                    .expect("WS handshake failed");
                let (mut write, mut read) = StreamExt::split(ws_stream);
                while let Some(Ok(msg)) = StreamExt::next(&mut read).await {
                    if msg.is_close() {
                        break;
                    }
                    if msg.is_text() || msg.is_binary() {
                        SinkExt::send(&mut write, msg).await.ok();
                    }
                }
            }
        }
        .in_current_span(),
    );

    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;

    let mut env_vars = HashMap::new();
    env_vars.insert("WS_PORT".to_string(), ws_port.to_string());

    let agent_id = agent_id!("WebsocketTest", "ws-echo-oplog-replay");
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), env_vars, Vec::new())
        .await?;

    // First invocation: full WebSocket session (connect/send/receive/drop) and
    // one entry persisted in agent-local `echo_history`.
    let first_result = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "echo_and_record",
            data_value!(format!("ws://localhost:{ws_port}"), "hello websocket"),
        )
        .await?;

    assert_eq!(first_result.into_typed::<String>()?, "hello websocket");

    executor.check_oplog_is_queryable(&worker_id).await?;

    // Drop the executor to force replay on next activation. Keep server running
    // so the second invocation can still do live websocket I/O.
    drop(executor);

    // Restarting does not directly invoke guest functions; replay is performed
    // when this worker is activated by the next invocation.
    let executor = start(deps, &context).await?;

    // Second invocation: replay reconstructs agent state from the first invoke.
    // We append another message and assert we now observe "m1|m2".
    let second_result = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "echo_and_record",
            data_value!(format!("ws://localhost:{ws_port}"), "hello websocket 2"),
        )
        .await?;

    assert_eq!(
        second_result.into_typed::<String>()?,
        "hello websocket|hello websocket 2"
    );

    executor.check_oplog_is_queryable(&worker_id).await?;

    drop(executor);
    ws_server.abort();

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
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await.unwrap();
    let ws_port = listener.local_addr().unwrap().port();
    let accepted_connections = Arc::new(AtomicUsize::new(0));
    let transcript = Arc::new(Mutex::new(Vec::<ServerEvent>::new()));
    let accepted_connections_for_server = Arc::clone(&accepted_connections);
    let transcript_for_server = Arc::clone(&transcript);

    let ws_server = spawn(
        async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    break;
                };
                let connection = accepted_connections_for_server.fetch_add(1, Ordering::SeqCst);
                let transcript_for_connection = Arc::clone(&transcript_for_server);
                spawn(
                    async move {
                        let ws_stream = tokio_tungstenite::accept_async(stream)
                            .await
                            .expect("WS handshake failed");
                        let (mut write, mut read) = StreamExt::split(ws_stream);
                        while let Some(Ok(msg)) = StreamExt::next(&mut read).await {
                            if msg.is_close() {
                                break;
                            }
                            if msg.is_text() {
                                let payload = msg
                                    .to_text()
                                    .expect("text message should decode")
                                    .to_string();
                                transcript_for_connection.lock().unwrap().push(ServerEvent {
                                    connection,
                                    payload,
                                });
                            }
                            if msg.is_text() || msg.is_binary() {
                                SinkExt::send(&mut write, msg).await.ok();
                            }
                        }
                    }
                    .in_current_span(),
                );
            }
        }
        .in_current_span(),
    );

    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;

    let mut env_vars = HashMap::new();
    env_vars.insert("WS_PORT".to_string(), ws_port.to_string());

    let agent_id = agent_id!("WebsocketTest", "ws-phase2-reconnect");
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), env_vars, Vec::new())
        .await?;

    let promise_id_value = executor
        .invoke_and_await_agent(&component, &agent_id, "create_promise", data_value!())
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected return value"))?;
    let params =
        replay_reconnect_roundtrip_params(format!("ws://localhost:{ws_port}"), &promise_id_value);
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
        .wait_for_status(&worker_id, AgentStatus::Suspended, Duration::from_secs(10))
        .await?;
    wait_for_server_state(&accepted_connections, &transcript, 1, 2).await?;
    assert_eq!(
        transcript.lock().unwrap().clone(),
        vec![
            ServerEvent {
                connection: 0,
                payload: "msg-1".to_string(),
            },
            ServerEvent {
                connection: 0,
                payload: "msg-2".to_string(),
            },
        ]
    );

    executor.check_oplog_is_queryable(&worker_id).await?;
    drop(executor);

    let executor = start(deps, &context).await?;
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(accepted_connections.load(Ordering::SeqCst), 1);
    assert_eq!(transcript.lock().unwrap().len(), 2);

    let oplog_idx = extract_oplog_idx_from_promise_id(&promise_id_value);
    executor
        .complete_promise(
            &PromiseId {
                agent_id: worker_id.clone(),
                oplog_idx,
            },
            vec![1],
        )
        .await?;

    let result = executor
        .invoke_and_await_agent_with_key(
            &component,
            &agent_id,
            &idempotency_key,
            "replay_reconnect_roundtrip",
            params,
        )
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected return value"))?;
    wait_for_server_state(&accepted_connections, &transcript, 2, 4).await?;
    assert_eq!(
        result,
        SchemaValue::Result(ResultValuePayload::Ok {
            value: Some(Box::new(SchemaValue::String(
                "msg-1|msg-2|msg-3|msg-4".to_string(),
            ))),
        })
    );
    assert_eq!(accepted_connections.load(Ordering::SeqCst), 2);
    assert_eq!(
        transcript.lock().unwrap().clone(),
        vec![
            ServerEvent {
                connection: 0,
                payload: "msg-1".to_string(),
            },
            ServerEvent {
                connection: 0,
                payload: "msg-2".to_string(),
            },
            ServerEvent {
                connection: 1,
                payload: "msg-3".to_string(),
            },
            ServerEvent {
                connection: 1,
                payload: "msg-4".to_string(),
            },
        ]
    );

    executor.check_oplog_is_queryable(&worker_id).await?;
    drop(executor);
    ws_server.abort();

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
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await.unwrap();
    let ws_port = listener.local_addr().unwrap().port();
    let accepted_connections = Arc::new(AtomicUsize::new(0));
    let transcript = Arc::new(Mutex::new(Vec::<ServerEvent>::new()));
    let accepted_connections_for_server = Arc::clone(&accepted_connections);
    let transcript_for_server = Arc::clone(&transcript);

    let ws_server = spawn(
        async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    break;
                };
                let connection = accepted_connections_for_server.fetch_add(1, Ordering::SeqCst);
                let transcript_for_connection = Arc::clone(&transcript_for_server);
                spawn(
                    async move {
                        let ws_stream = tokio_tungstenite::accept_async(stream)
                            .await
                            .expect("WS handshake failed");
                        let (mut write, mut read) = StreamExt::split(ws_stream);
                        while let Some(Ok(msg)) = StreamExt::next(&mut read).await {
                            if msg.is_close() {
                                break;
                            }
                            if msg.is_text() {
                                let payload = msg
                                    .to_text()
                                    .expect("text message should decode")
                                    .to_string();
                                transcript_for_connection.lock().unwrap().push(ServerEvent {
                                    connection,
                                    payload,
                                });
                            }
                            if msg.is_text() || msg.is_binary() {
                                SinkExt::send(&mut write, msg).await.ok();
                            }
                        }
                    }
                    .in_current_span(),
                );
            }
        }
        .in_current_span(),
    );

    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;

    let mut env_vars = HashMap::new();
    env_vars.insert("WS_PORT".to_string(), ws_port.to_string());

    let agent_id = agent_id!("WebsocketTest", "ws-reconnect-failure");
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), env_vars, Vec::new())
        .await?;

    let promise_id_value = executor
        .invoke_and_await_agent(&component, &agent_id, "create_promise", data_value!())
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected return value"))?;
    let params =
        replay_reconnect_roundtrip_params(format!("ws://localhost:{ws_port}"), &promise_id_value);
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
        .wait_for_status(&worker_id, AgentStatus::Suspended, Duration::from_secs(10))
        .await?;
    wait_for_server_state(&accepted_connections, &transcript, 1, 2).await?;

    executor.check_oplog_is_queryable(&worker_id).await?;
    ws_server.abort();
    drop(executor);

    let executor = start(deps, &context).await?;

    let oplog_idx = extract_oplog_idx_from_promise_id(&promise_id_value);
    executor
        .complete_promise(
            &PromiseId {
                agent_id: worker_id.clone(),
                oplog_idx,
            },
            vec![1],
        )
        .await?;

    let result = executor
        .invoke_and_await_agent_with_key(
            &component,
            &agent_id,
            &idempotency_key,
            "replay_reconnect_roundtrip",
            params,
        )
        .await?;
    let error = get_result_error_string(result);
    assert!(
        error.contains("ConnectionFailure"),
        "expected reconnect failure to reach the guest, got {error}"
    );
    assert_eq!(accepted_connections.load(Ordering::SeqCst), 1);

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
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await.unwrap();
    let ws_port = listener.local_addr().unwrap().port();
    let accepted_connections = Arc::new(AtomicUsize::new(0));
    let accepted_connections_for_server = Arc::clone(&accepted_connections);

    let ws_server = spawn(
        async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    break;
                };
                accepted_connections_for_server.fetch_add(1, Ordering::SeqCst);
                spawn(
                    async move {
                        let ws_stream = tokio_tungstenite::accept_async(stream)
                            .await
                            .expect("WS handshake failed");
                        let (mut write, _read) = StreamExt::split(ws_stream);
                        SinkExt::send(
                            &mut write,
                            tokio_tungstenite::tungstenite::Message::text("msg-0"),
                        )
                        .await
                        .ok();
                        SinkExt::send(
                            &mut write,
                            tokio_tungstenite::tungstenite::Message::Close(None),
                        )
                        .await
                        .ok();
                    }
                    .in_current_span(),
                );
            }
        }
        .in_current_span(),
    );

    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;

    let mut env_vars = HashMap::new();
    env_vars.insert("WS_PORT".to_string(), ws_port.to_string());

    let agent_id = agent_id!("WebsocketTest", "ws-terminal-close-replay");
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), env_vars, Vec::new())
        .await?;

    let first = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "connect_and_receive_first",
            data_value!(format!("ws://localhost:{ws_port}")),
        )
        .await?;
    assert_eq!(first.into_typed::<String>()?, "msg-0");
    assert_eq!(accepted_connections.load(Ordering::SeqCst), 1);

    let closed_result = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "receive_next_from_persisted_result",
            data_value!(),
        )
        .await?;
    let closed_error = get_result_error_string(closed_result);
    assert!(
        closed_error.contains("Closed"),
        "expected persisted socket to report closure, got {closed_error}"
    );
    assert_eq!(accepted_connections.load(Ordering::SeqCst), 1);

    executor.check_oplog_is_queryable(&worker_id).await?;
    drop(executor);

    let executor = start(deps, &context).await?;
    let replayed_result = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "receive_next_from_persisted_result",
            data_value!(),
        )
        .await?;
    let replayed_error = get_result_error_string(replayed_result);
    assert!(
        replayed_error.contains("Closed"),
        "expected replayed terminal socket to stay closed, got {replayed_error}"
    );
    assert_eq!(
        accepted_connections.load(Ordering::SeqCst),
        1,
        "terminal websocket should not reconnect after replay"
    );

    executor.check_oplog_is_queryable(&worker_id).await?;
    drop(executor);
    ws_server.abort();

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
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await.unwrap();
    let ws_port = listener.local_addr().unwrap().port();
    let accepted_connections = Arc::new(AtomicUsize::new(0));
    let accepted_connections_for_server = Arc::clone(&accepted_connections);

    let ws_server = spawn(
        async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    break;
                };
                accepted_connections_for_server.fetch_add(1, Ordering::SeqCst);
                spawn(
                    async move {
                        let ws_stream = tokio_tungstenite::accept_async(stream)
                            .await
                            .expect("WS handshake failed");
                        let (mut write, mut read) = StreamExt::split(ws_stream);
                        SinkExt::send(
                            &mut write,
                            tokio_tungstenite::tungstenite::Message::text("msg-0"),
                        )
                        .await
                        .ok();

                        while let Some(Ok(msg)) = StreamExt::next(&mut read).await {
                            if msg.is_close() {
                                break;
                            }
                        }
                    }
                    .in_current_span(),
                );
            }
        }
        .in_current_span(),
    );

    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;

    let mut env_vars = HashMap::new();
    env_vars.insert("WS_PORT".to_string(), ws_port.to_string());

    let agent_id = agent_id!("WebsocketTest", "ws-close-terminal-replay");
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), env_vars, Vec::new())
        .await?;

    let first = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "connect_and_receive_first",
            data_value!(format!("ws://localhost:{ws_port}")),
        )
        .await?;
    assert_eq!(first.into_typed::<String>()?, "msg-0");
    assert_eq!(accepted_connections.load(Ordering::SeqCst), 1);

    let close_result = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "close_persisted_result",
            data_value!(),
        )
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected return value"))?;
    assert_eq!(
        close_result,
        SchemaValue::Result(ResultValuePayload::Ok { value: None })
    );

    let closed_result = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "receive_next_from_persisted_result",
            data_value!(),
        )
        .await?;
    let closed_error = get_result_error_string(closed_result);
    assert!(
        closed_error.contains("Closed"),
        "expected closed handle to become terminal immediately, got {closed_error}"
    );
    assert_eq!(accepted_connections.load(Ordering::SeqCst), 1);

    executor.check_oplog_is_queryable(&worker_id).await?;
    drop(executor);

    let executor = start(deps, &context).await?;
    let replayed_result = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "receive_next_from_persisted_result",
            data_value!(),
        )
        .await?;
    let replayed_error = get_result_error_string(replayed_result);
    assert!(
        replayed_error.contains("Closed"),
        "expected successfully closed websocket to remain terminal after replay, got {replayed_error}"
    );
    assert_eq!(
        accepted_connections.load(Ordering::SeqCst),
        1,
        "successfully closed websocket should not reconnect after replay"
    );

    executor.check_oplog_is_queryable(&worker_id).await?;
    drop(executor);
    ws_server.abort();

    Ok(())
}

fn replay_reconnect_roundtrip_params(
    url: String,
    promise_id_value: &SchemaValue,
) -> golem_common::schema::TypedSchemaValue {
    crate::raw_params(vec![SchemaValue::String(url), promise_id_value.clone()])
}

fn extract_oplog_idx_from_promise_id(promise_id_value: &SchemaValue) -> OplogIndex {
    let SchemaValue::Record { fields } = promise_id_value else {
        panic!("Expected a record for PromiseId");
    };
    let SchemaValue::U64(oplog_idx) = fields[1] else {
        panic!("Expected second PromiseId field to be oplog_idx");
    };

    OplogIndex::from_u64(oplog_idx)
}

fn get_result_error_string(result: golem_test_framework::dsl::AgentResult) -> String {
    let return_value = result
        .into_return_value()
        .expect("expected a single return value");

    match return_value {
        SchemaValue::Result(ResultValuePayload::Err {
            value: Some(err_value),
        }) => match *err_value {
            SchemaValue::String(s) => s,
            other => panic!("expected error String, got {other:?}"),
        },
        other => panic!("expected Result(Err(...)), got {other:?}"),
    }
}

async fn wait_for_server_state(
    accepted_connections: &Arc<AtomicUsize>,
    transcript: &Arc<Mutex<Vec<ServerEvent>>>,
    expected_connections: usize,
    expected_messages: usize,
) -> anyhow::Result<()> {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let actual_connections = accepted_connections.load(Ordering::SeqCst);
            let actual_messages = transcript.lock().unwrap().len();

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
    .map_err(|_| {
        anyhow!(
            "Timed out waiting for websocket server state: expected {expected_connections} connections and {expected_messages} messages"
        )
    })
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
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await.unwrap();
    let ws_port = listener.local_addr().unwrap().port();

    let ws_server = spawn(
        async move {
            if let Ok((stream, _)) = listener.accept().await {
                let ws_stream = tokio_tungstenite::accept_async(stream)
                    .await
                    .expect("WS handshake failed");
                let (_write, mut read) = StreamExt::split(ws_stream);
                while let Some(Ok(msg)) = StreamExt::next(&mut read).await {
                    if msg.is_close() {
                        break;
                    }
                }
            }
        }
        .in_current_span(),
    );

    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;

    let mut env_vars = HashMap::new();
    env_vars.insert("WS_PORT".to_string(), ws_port.to_string());

    let agent_id = agent_id!("WebsocketTest", "ws-timeout-test");
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), env_vars, Vec::new())
        .await?;

    let result = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "receive_with_timeout_test",
            data_value!(format!("ws://localhost:{ws_port}"), 1000u64),
        )
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected return value"))?;

    assert_eq!(result, SchemaValue::Option { inner: None });

    executor.check_oplog_is_queryable(&worker_id).await?;

    drop(executor);
    ws_server.abort();

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
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await.unwrap();
    let ws_port = listener.local_addr().unwrap().port();
    let message = "Hello from polling test";

    let ws_server = spawn(
        async move {
            if let Ok((stream, _)) = listener.accept().await {
                let ws_stream = tokio_tungstenite::accept_async(stream)
                    .await
                    .expect("WS handshake failed");
                let (mut write, _read) = StreamExt::split(ws_stream);
                let msg = tokio_tungstenite::tungstenite::Message::text(message);
                SinkExt::send(&mut write, msg).await.ok();
            }
        }
        .in_current_span(),
    );

    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;

    let mut env_vars = HashMap::new();
    env_vars.insert("WS_PORT".to_string(), ws_port.to_string());

    let agent_id = agent_id!("WebsocketTest", "websocket-polling-test");
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), env_vars, Vec::new())
        .await?;

    let result = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "poll_for_message",
            data_value!(format!("ws://localhost:{ws_port}"), 1000u64),
        )
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected return value"))?;

    assert_eq!(
        result,
        SchemaValue::Result(ResultValuePayload::Ok {
            value: Some(Box::new(SchemaValue::String(message.to_string())))
        })
    );

    executor.check_oplog_is_queryable(&worker_id).await?;

    drop(executor);
    ws_server.abort();

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
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await.unwrap();
    let ws_port = listener.local_addr().unwrap().port();
    let message = "delayed polling message";

    let ws_server = spawn(
        async move {
            if let Ok((stream, _)) = listener.accept().await {
                let ws_stream = tokio_tungstenite::accept_async(stream)
                    .await
                    .expect("WS handshake failed");
                let (mut write, _read) = StreamExt::split(ws_stream);
                tokio::time::sleep(Duration::from_millis(150)).await;
                let msg = tokio_tungstenite::tungstenite::Message::text(message);
                SinkExt::send(&mut write, msg).await.ok();
            }
        }
        .in_current_span(),
    );

    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;

    let mut env_vars = HashMap::new();
    env_vars.insert("WS_PORT".to_string(), ws_port.to_string());

    let agent_id = agent_id!("WebsocketTest", "websocket-polling-timeout-race-test");
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), env_vars, Vec::new())
        .await?;

    let result = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "poll_until_message_after_timeouts",
            data_value!(format!("ws://localhost:{ws_port}"), 10u64, 30u32),
        )
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected return value"))?;

    assert_eq!(
        result,
        SchemaValue::Result(ResultValuePayload::Ok {
            value: Some(Box::new(SchemaValue::String(message.to_string())))
        })
    );

    executor.check_oplog_is_queryable(&worker_id).await?;

    drop(executor);
    ws_server.abort();

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
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await.unwrap();
    let ws_port = listener.local_addr().unwrap().port();

    // Echo server that supports multiple inbound/outbound messages on one connection.
    let ws_server = spawn(
        async move {
            if let Ok((stream, _)) = listener.accept().await {
                let ws_stream = tokio_tungstenite::accept_async(stream)
                    .await
                    .expect("WS handshake failed");
                let (mut write, mut read) = StreamExt::split(ws_stream);
                while let Some(Ok(msg)) = StreamExt::next(&mut read).await {
                    if msg.is_close() {
                        break;
                    }
                    if msg.is_text() || msg.is_binary() {
                        SinkExt::send(&mut write, msg).await.ok();
                    }
                }
            }
        }
        .in_current_span(),
    );

    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;

    let mut env_vars = HashMap::new();
    env_vars.insert("WS_PORT".to_string(), ws_port.to_string());

    let agent_id = agent_id!("WebsocketTest", "websocket-async-bidi-test");
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), env_vars, Vec::new())
        .await?;

    let result = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "async_bidi_test",
            data_value!(format!("ws://localhost:{ws_port}")),
        )
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected return value"))?;

    assert_eq!(
        result,
        SchemaValue::Result(ResultValuePayload::Ok {
            value: Some(Box::new(SchemaValue::String(
                "msg-a|msg-b|msg-c".to_string()
            ))),
        })
    );

    executor.check_oplog_is_queryable(&worker_id).await?;

    drop(executor);
    ws_server.abort();

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
    let context = TestContext::new(last_unique_id);

    let executor = start(deps, &context).await?;

    let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await.unwrap();
    let ws_port = listener.local_addr().unwrap().port();

    // Keep accepting connections so the second invocation after restart can run live.
    let ws_server = spawn(
        async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    break;
                };

                let ws_stream = tokio_tungstenite::accept_async(stream)
                    .await
                    .expect("WS handshake failed");
                let (mut write, mut read) = StreamExt::split(ws_stream);
                while let Some(Ok(msg)) = StreamExt::next(&mut read).await {
                    if msg.is_close() {
                        break;
                    }
                    if msg.is_text() || msg.is_binary() {
                        SinkExt::send(&mut write, msg).await.ok();
                    }
                }
            }
        }
        .in_current_span(),
    );

    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;

    let mut env_vars = HashMap::new();
    env_vars.insert("WS_PORT".to_string(), ws_port.to_string());

    let agent_id = agent_id!("WebsocketTest", "websocket-async-bidi-oplog-replay");
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), env_vars, Vec::new())
        .await?;

    let first_result = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "async_bidi_test",
            data_value!(format!("ws://localhost:{ws_port}")),
        )
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected return value"))?;

    assert_eq!(
        first_result,
        SchemaValue::Result(ResultValuePayload::Ok {
            value: Some(Box::new(SchemaValue::String(
                "msg-a|msg-b|msg-c".to_string()
            ))),
        })
    );

    executor.check_oplog_is_queryable(&worker_id).await?;

    drop(executor);

    let executor = start(deps, &context).await?;

    let second_result = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "async_bidi_test",
            data_value!(format!("ws://localhost:{ws_port}")),
        )
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected return value"))?;

    assert_eq!(
        second_result,
        SchemaValue::Result(ResultValuePayload::Ok {
            value: Some(Box::new(SchemaValue::String(
                "msg-a|msg-b|msg-c".to_string()
            ))),
        })
    );

    executor.check_oplog_is_queryable(&worker_id).await?;

    drop(executor);
    ws_server.abort();

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
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await.unwrap();
    let ws_port = listener.local_addr().unwrap().port();

    let ws_server = spawn(
        async move {
            if let Ok((stream, _)) = listener.accept().await {
                let ws_stream = tokio_tungstenite::accept_async(stream)
                    .await
                    .expect("WS handshake failed");
                let (mut write, mut read) = StreamExt::split(ws_stream);
                while let Some(Ok(msg)) = StreamExt::next(&mut read).await {
                    if msg.is_close() {
                        break;
                    }
                    if msg.is_text() || msg.is_binary() {
                        SinkExt::send(&mut write, msg).await.ok();
                    }
                }
            }
        }
        .in_current_span(),
    );

    let component = executor
        .component_dep(&context.default_environment_id, agent_sdk_ts)
        .store()
        .await?;

    let mut env_vars = HashMap::new();
    env_vars.insert("WS_PORT".to_string(), ws_port.to_string());

    let agent_id = agent_id!("WebSocketTest", "ws-echo-test");
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), env_vars, Vec::new())
        .await?;

    let result = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "echo",
            data_value!(format!("ws://localhost:{ws_port}"), "hello websocket"),
        )
        .await?;

    assert_eq!(result.into_typed::<String>()?, "hello websocket");

    executor.check_oplog_is_queryable(&worker_id).await?;

    drop(executor);
    ws_server.abort();

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

const RECONNECT_FIRST_MESSAGE: &str = "original";
const RECONNECT_MESSAGES: [&str; 2] = ["post-reconnect-a", "post-reconnect-b"];
const CONTENTION_METHOD: &str = "receive_lock_contention_from_persisted";
const CONTENTION_TIMEOUT_MS: u64 = 60_000;

/// A websocket server whose first completed handshake greets the initial
/// connection and whose later handshakes deliver the reconnected handle's
/// messages. TCP accepts are counted separately from completed handshakes so
/// tests can tell an aborted connection attempt from a full handshake.
struct ReconnectTestServer {
    port: u16,
    accepted: Arc<AtomicUsize>,
    completed_handshakes: Arc<AtomicUsize>,
    task: tokio::task::JoinHandle<()>,
}

impl ReconnectTestServer {
    async fn start() -> Self {
        let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let accepted = Arc::new(AtomicUsize::new(0));
        let completed_handshakes = Arc::new(AtomicUsize::new(0));
        let accepted_for_server = Arc::clone(&accepted);
        let completed_for_server = Arc::clone(&completed_handshakes);
        let task = spawn(
            async move {
                loop {
                    let Ok((stream, _)) = listener.accept().await else {
                        break;
                    };
                    accepted_for_server.fetch_add(1, Ordering::SeqCst);
                    let completed_for_connection = Arc::clone(&completed_for_server);
                    spawn(
                        async move {
                            let ws_stream = tokio_tungstenite::accept_async(stream)
                                .await
                                .expect("WS handshake failed");
                            let handshake = completed_for_connection.fetch_add(1, Ordering::SeqCst);
                            let (mut write, mut read) = StreamExt::split(ws_stream);
                            if handshake == 0 {
                                SinkExt::send(
                                    &mut write,
                                    tokio_tungstenite::tungstenite::Message::text(
                                        RECONNECT_FIRST_MESSAGE,
                                    ),
                                )
                                .await
                                .ok();
                            } else {
                                for payload in RECONNECT_MESSAGES {
                                    SinkExt::send(
                                        &mut write,
                                        tokio_tungstenite::tungstenite::Message::text(payload),
                                    )
                                    .await
                                    .ok();
                                }
                            }
                            while let Some(Ok(msg)) = StreamExt::next(&mut read).await {
                                if msg.is_close() {
                                    break;
                                }
                            }
                        }
                        .in_current_span(),
                    );
                }
            }
            .in_current_span(),
        );
        Self {
            port,
            accepted,
            completed_handshakes,
            task,
        }
    }

    fn url(&self) -> String {
        format!("ws://localhost:{}", self.port)
    }

    fn assert_counts(&self, accepted: usize, completed_handshakes: usize) {
        assert_eq!(
            self.accepted.load(Ordering::SeqCst),
            accepted,
            "unexpected number of TCP accepts"
        );
        assert_eq!(
            self.completed_handshakes.load(Ordering::SeqCst),
            completed_handshakes,
            "unexpected number of completed websocket handshakes"
        );
    }

    fn abort(&self) {
        self.task.abort();
    }
}

/// The `AgentInvocationStarted` oplog index of the first
/// `receive_lock_contention_from_persisted` run, anchoring which receive-family
/// `Start`s belong to that run: the initial connection's own receive belongs to
/// the earlier `connect_and_receive_first` invocation.
fn contention_invocation_started(oplog: &[PublicOplogEntryWithIndex]) -> Option<OplogIndex> {
    oplog.iter().find_map(|entry| {
        let PublicOplogEntry::AgentInvocationStarted(params) = &entry.entry else {
            return None;
        };
        match &params.invocation {
            PublicAgentInvocation::AgentMethodInvocation(method)
                if method.method_name.replace('-', "_") == CONTENTION_METHOD =>
            {
                Some(entry.oplog_index)
            }
            _ => None,
        }
    })
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
        assert!(
            !oplog
                .iter()
                .any(|entry| matches!(&entry.entry, PublicOplogEntry::End(params)
                    if params.start_index == start)),
            "the interrupted round must leave the {function} Start at {start} without an End"
        );
        assert!(
            !oplog.iter().any(
                |entry| matches!(&entry.entry, PublicOplogEntry::Cancelled(params)
                    if params.start_index == start)
            ),
            "the interrupted round must leave the {function} Start at {start} without a Cancelled"
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
        let ends = oplog
            .iter()
            .filter(|entry| {
                matches!(&entry.entry, PublicOplogEntry::End(params)
                if params.start_index == *start)
            })
            .collect::<Vec<_>>();
        assert_eq!(
            ends.len(),
            1,
            "expected exactly one End for the {function} Start at {start}"
        );
        let deliveries = oplog
            .iter()
            .filter(|entry| {
                matches!(&entry.entry, PublicOplogEntry::CompletionDelivered(params)
                if params.start_index == *start)
            })
            .collect::<Vec<_>>();
        assert_eq!(
            deliveries.len(),
            1,
            "expected exactly one CompletionDelivered for the {function} Start at {start}"
        );
        assert!(
            !oplog.iter().any(
                |entry| matches!(&entry.entry, PublicOplogEntry::Cancelled(params)
                    if params.start_index == *start)
            ),
            "expected no Cancelled for the {function} Start at {start}"
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

/// Reconnect observation tallies as `(decided calls, gate waits, pool waits,
/// handshake waits, published live outcomes, published terminal outcomes)`.
fn coordination_tallies<'a>(
    events: impl IntoIterator<Item = &'a WebSocketReconnectEventForTest>,
) -> (usize, usize, usize, usize, usize, usize) {
    let mut tallies = (0, 0, 0, 0, 0, 0);
    for event in events {
        match event.kind {
            WebSocketReconnectEventKindForTest::Decided => tallies.0 += 1,
            WebSocketReconnectEventKindForTest::WaitPending {
                wait: WebSocketReconnectWaitForTest::Gate,
            } => tallies.1 += 1,
            WebSocketReconnectEventKindForTest::WaitPending {
                wait: WebSocketReconnectWaitForTest::Pool,
            } => tallies.2 += 1,
            WebSocketReconnectEventKindForTest::WaitPending {
                wait: WebSocketReconnectWaitForTest::Handshake,
            } => tallies.3 += 1,
            WebSocketReconnectEventKindForTest::Published {
                outcome: WebSocketReconnectOutcomeForTest::Live,
            } => tallies.4 += 1,
            WebSocketReconnectEventKindForTest::Published {
                outcome: WebSocketReconnectOutcomeForTest::Terminal,
            } => tallies.5 += 1,
        }
    }
    tallies
}

/// Waits, starting from the already collected `events`, until the observation
/// records at least `decided` decided reconnects and `gated` gate waits, and
/// returns every event received so far. The decided calls park on the
/// test-held handshake admission, so this is the deterministic point where
/// every reconnecting call has either decided and parked on the held
/// handshake or queued on the per-handle gate.
async fn wait_for_coordination_events(
    observation: &mut WebSocketReconnectObservationForTest,
    mut events: Vec<WebSocketReconnectEventForTest>,
    decided: usize,
    gated: usize,
    timeout: Duration,
) -> anyhow::Result<Vec<WebSocketReconnectEventForTest>> {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        let tallies = coordination_tallies(&events);
        if tallies.0 >= decided && tallies.1 >= gated {
            return Ok(events);
        }
        let Some(remaining) = deadline.checked_duration_since(std::time::Instant::now()) else {
            anyhow::bail!(
                "timed out waiting for {decided} decided and {gated} gated reconnects; \
                 events so far: {events:?}"
            );
        };
        match tokio::time::timeout(remaining, observation.events.recv()).await {
            Ok(Some(event)) => events.push(event),
            Ok(None) => anyhow::bail!("the reconnect observation event channel closed"),
            Err(_) => {}
        }
    }
}

/// Collects every reconnect event recorded after the observed reconnect
/// activity completed. Every reconnect event is recorded before the
/// invocation that performed it finishes, so a short settle suffices for the
/// channel to hold all of them.
async fn drain_coordination_events(
    observation: &mut WebSocketReconnectObservationForTest,
    mut events: Vec<WebSocketReconnectEventForTest>,
) -> Vec<WebSocketReconnectEventForTest> {
    tokio::time::sleep(Duration::from_millis(100)).await;
    while let Ok(event) = observation.events.try_recv() {
        events.push(event);
    }
    events
}

/// Asserts the recorded reconnect observation: the given tallies, every
/// reconnect taking the accessor path, and every event attributed to one of
/// the given contention invocation keys.
fn assert_coordination_events(
    events: &[WebSocketReconnectEventForTest],
    invocation_keys: &[IdempotencyKey],
    tallies: (usize, usize, usize, usize, usize, usize),
) {
    assert_eq!(
        coordination_tallies(events),
        tallies,
        "unexpected reconnect event tallies: {events:?}"
    );
    for event in events {
        assert_eq!(
            event.path,
            WebSocketReconnectPathForTest::Accessor,
            "every observed reconnect must take the accessor path: {event:?}"
        );
        let key = event
            .invocation_key
            .as_ref()
            .unwrap_or_else(|| panic!("reconnect event without an invocation key: {event:?}"));
        assert!(
            invocation_keys.contains(key),
            "unexpected reconnect invocation key {key} \
             (expected one of {invocation_keys:?}): {event:?}"
        );
    }
}

/// Asserts every recorded reconnect wait returned while its observer was
/// alive: no observed wait was abandoned.
fn assert_coordination_waits_returned(events: &[WebSocketReconnectEventForTest]) {
    for event in events {
        if let Some(state) = event.wait_state() {
            assert_eq!(
                state,
                WebSocketReconnectWaitStateForTest::Returned,
                "every observed reconnect wait must return: {event:?}"
            );
        }
    }
}

/// True for the reconnect events whose wait was abandoned while parked.
fn is_dropped_reconnect_wait(event: &WebSocketReconnectEventForTest) -> bool {
    matches!(
        event.kind,
        WebSocketReconnectEventKindForTest::WaitPending { .. }
    ) && event.wait_state() == Some(WebSocketReconnectWaitStateForTest::Dropped)
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
    let context = TestContext::new(last_unique_id);
    let executor = start_with_overrides(
        deps,
        &context,
        TestExecutorOverrides {
            configure: Some(Arc::new(|config| {
                config.max_websocket_connections = 1;
            })),
            ..Default::default()
        },
    )
    .await?;

    let ws_server = ReconnectTestServer::start().await;
    let url = ws_server.url();

    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;

    let agent_id = agent_id!("WebsocketTest", "ws-reconnect-contention");
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), HashMap::new(), Vec::new())
        .await?;

    // The first handshake greets the initial live connection, whose handle is
    // persisted in agent state.
    let first = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "connect_and_receive_first",
            data_value!(url.clone()),
        )
        .await?;
    assert_eq!(first.into_typed::<String>()?, RECONNECT_FIRST_MESSAGE);
    ws_server.assert_counts(1, 1);

    // Both receives park on the live connection: the server sends nothing
    // further on it.
    let contention_key = IdempotencyKey::fresh();
    executor
        .invoke_agent_with_key(
            &component,
            &agent_id,
            &contention_key,
            CONTENTION_METHOD,
            data_value!(CONTENTION_TIMEOUT_MS),
        )
        .await?;

    // The invocation's `AgentInvocationStarted` entry is committed as a
    // protocol barrier, so the worker reports running while it is parked on
    // the live connection. Letting the guest reach both receives before the
    // interrupt keeps both of their durable calls open when it lands: the
    // interrupt is delivered at their await points.
    executor
        .wait_for_status(&worker_id, AgentStatus::Running, Duration::from_secs(10))
        .await?;
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Interrupting abandons both receives with their `Start`s left incomplete.
    executor.interrupt(&worker_id).await?;
    executor
        .wait_for_status(
            &worker_id,
            AgentStatus::Interrupted,
            Duration::from_secs(10),
        )
        .await?;

    // The interrupt commits the oplog, so both abandoned receive-family
    // `Start`s are recorded and still incomplete: no `End` and no `Cancelled`
    // terminal references either of them.
    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    assert_contention_receives_incomplete(&oplog);

    // Resuming replays the persisted handle as a `Replay` entry and retries the
    // retained invocation: both receives re-execute live and contend on
    // reconnecting the same handle. With a single-connection pool this only
    // completes if the reconnect is coordinated per handle; otherwise the
    // follower parks on the pool and the worker never returns to idle.
    executor.resume(&worker_id, false).await?;
    executor
        .wait_for_status(&worker_id, AgentStatus::Idle, Duration::from_secs(20))
        .await?;

    // The retained invocation finished exactly once; its recorded result holds
    // both post-reconnect payloads regardless of which receive reconnected.
    let result = executor
        .invoke_and_await_agent_with_key(
            &component,
            &agent_id,
            &contention_key,
            CONTENTION_METHOD,
            data_value!(CONTENTION_TIMEOUT_MS),
        )
        .await?
        .into_typed::<(String, String)>()?;
    assert_eq!(
        sorted_reconnect_payloads(&result),
        expected_reconnect_payloads()
    );

    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    assert_contention_receive_correlation(&oplog);
    ws_server.assert_counts(2, 2);

    // Re-invoking with the same key replays the recorded result without
    // opening new receive-family durable calls or a new connection.
    let replayed = executor
        .invoke_and_await_agent_with_key(
            &component,
            &agent_id,
            &contention_key,
            CONTENTION_METHOD,
            data_value!(CONTENTION_TIMEOUT_MS),
        )
        .await?
        .into_typed::<(String, String)>()?;
    assert_eq!(replayed, result);
    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    assert_contention_receive_correlation(&oplog);
    ws_server.assert_counts(2, 2);

    // Completed receives replay from their recorded `End`s without touching
    // the network: after evicting the idle worker and killing the server, a
    // fresh activation replays every completed websocket call offline.
    assert!(
        executor
            .stop_worker_if_idle(&OwnedAgentId::new(
                context.default_environment_id,
                &worker_id
            ))
            .await?
    );
    ws_server.abort();
    let noop = executor
        .invoke_and_await_agent(&component, &agent_id, "noop", data_value!())
        .await?;
    assert_eq!(noop.into_typed::<String>()?, "ok");
    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    assert_contention_receive_correlation(&oplog);
    ws_server.assert_counts(2, 2);

    executor.check_oplog_is_queryable(&worker_id).await?;

    drop(executor);

    Ok(())
}

/// The retry after an interrupt must coordinate the live reconnect of the
/// replayed handle per resource: exactly one accessor call decides to
/// reconnect while its concurrent peer queues on the per-handle gate, and the
/// decided call publishes the live connection for the queued call to reuse
/// instead of each call independently taking a pool permit and opening a
/// second connection. The test holds the decided call's websocket handshake
/// at a deterministic boundary and releases it after both calls reached their
/// parked positions, so the coordination is observed rather than raced.
#[test]
#[tracing::instrument]
#[timeout("3m")]
async fn websocket_reconnect_coordination_admits_one_accessor_reconnect(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start_with_overrides(
        deps,
        &context,
        TestExecutorOverrides {
            configure: Some(Arc::new(|config| {
                config.max_websocket_connections = 1;
            })),
            ..Default::default()
        },
    )
    .await?;

    let ws_server = ReconnectTestServer::start().await;
    let url = ws_server.url();

    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;

    let agent_id = agent_id!("WebsocketTest", "ws-reconnect-coordinate");
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), HashMap::new(), Vec::new())
        .await?;

    let first = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "connect_and_receive_first",
            data_value!(url.clone()),
        )
        .await?;
    assert_eq!(first.into_typed::<String>()?, RECONNECT_FIRST_MESSAGE);
    ws_server.assert_counts(1, 1);

    let contention_key = IdempotencyKey::fresh();
    executor
        .invoke_agent_with_key(
            &component,
            &agent_id,
            &contention_key,
            CONTENTION_METHOD,
            data_value!(CONTENTION_TIMEOUT_MS),
        )
        .await?;
    executor
        .wait_for_status(&worker_id, AgentStatus::Running, Duration::from_secs(10))
        .await?;
    tokio::time::sleep(Duration::from_millis(500)).await;

    executor.interrupt(&worker_id).await?;
    executor
        .wait_for_status(
            &worker_id,
            AgentStatus::Interrupted,
            Duration::from_secs(10),
        )
        .await?;
    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    assert_contention_receives_incomplete(&oplog);

    // The observation is installed before the retry: every reconnect event of
    // the retry belongs to it.
    let mut observation = executor.observe_websocket_reconnects_for_test();

    executor.resume(&worker_id, false).await?;

    // One receive wins the per-handle gate and decides to reconnect, parking
    // on the held websocket handshake; the other queues on the gate. No pool
    // wait and no connection appear while the handshake is held.
    let events =
        wait_for_coordination_events(&mut observation, Vec::new(), 1, 1, Duration::from_secs(10))
            .await?;
    ws_server.assert_counts(1, 1);

    observation.control.release_handshake();

    executor
        .wait_for_status(&worker_id, AgentStatus::Idle, Duration::from_secs(20))
        .await?;

    let events = drain_coordination_events(&mut observation, events).await;
    assert_coordination_events(
        &events,
        std::slice::from_ref(&contention_key),
        (1, 1, 0, 1, 1, 0),
    );
    assert_coordination_waits_returned(&events);

    let result = executor
        .invoke_and_await_agent_with_key(
            &component,
            &agent_id,
            &contention_key,
            CONTENTION_METHOD,
            data_value!(CONTENTION_TIMEOUT_MS),
        )
        .await?
        .into_typed::<(String, String)>()?;
    assert_eq!(
        sorted_reconnect_payloads(&result),
        expected_reconnect_payloads()
    );
    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    assert_contention_receive_correlation(&oplog);
    ws_server.assert_counts(2, 2);

    executor.check_oplog_is_queryable(&worker_id).await?;

    drop(executor);
    ws_server.abort();

    Ok(())
}

/// The reconnect coordination also governs the retry after an executor
/// restart: a crash while both receives are parked leaves their durable calls
/// incomplete, and the same-key invocation on a fresh executor recovers the
/// worker, replays the persisted handle as a `Replay` entry and re-runs both
/// receives. Only one accessor call reconnects the handle; its peer queues on
/// the per-handle gate and reuses the published entry. The observation is
/// requested before any worker context exists on the fresh executor, so it is
/// installed pending and captures every reconnect of the recovered worker.
#[test]
#[tracing::instrument]
#[timeout("3m")]
async fn websocket_reconnect_coordination_after_executor_restart(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let overrides = TestExecutorOverrides {
        configure: Some(Arc::new(|config| {
            config.max_websocket_connections = 1;
        })),
        ..Default::default()
    };
    let context = TestContext::new(last_unique_id);
    let executor = start_with_overrides(deps, &context, overrides.clone()).await?;

    let ws_server = ReconnectTestServer::start().await;
    let url = ws_server.url();

    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;

    let agent_id = agent_id!("WebsocketTest", "ws-reconnect-restart");
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), HashMap::new(), Vec::new())
        .await?;

    let first = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "connect_and_receive_first",
            data_value!(url.clone()),
        )
        .await?;
    assert_eq!(first.into_typed::<String>()?, RECONNECT_FIRST_MESSAGE);
    ws_server.assert_counts(1, 1);

    let contention_key = IdempotencyKey::fresh();
    executor
        .invoke_agent_with_key(
            &component,
            &agent_id,
            &contention_key,
            CONTENTION_METHOD,
            data_value!(CONTENTION_TIMEOUT_MS),
        )
        .await?;
    executor
        .wait_for_status(&worker_id, AgentStatus::Running, Duration::from_secs(10))
        .await?;
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Crashing the executor while both receives are parked abandons the
    // retained invocation with its durable calls left incomplete.
    executor.shutdown_and_wait_for_invocation_loops().await?;
    drop(executor);
    let executor = start_with_overrides(deps, &context, overrides).await?;

    // Requested before any worker context exists on the fresh executor, the
    // observation is installed pending by the recovered worker's context.
    let mut observation = executor.observe_websocket_reconnects_for_test();

    executor
        .invoke_agent_with_key(
            &component,
            &agent_id,
            &contention_key,
            CONTENTION_METHOD,
            data_value!(CONTENTION_TIMEOUT_MS),
        )
        .await?;

    let events =
        wait_for_coordination_events(&mut observation, Vec::new(), 1, 1, Duration::from_secs(10))
            .await?;
    ws_server.assert_counts(1, 1);

    observation.control.release_handshake();

    executor
        .wait_for_status(&worker_id, AgentStatus::Idle, Duration::from_secs(20))
        .await?;

    let events = drain_coordination_events(&mut observation, events).await;
    assert_coordination_events(
        &events,
        std::slice::from_ref(&contention_key),
        (1, 1, 0, 1, 1, 0),
    );
    assert_coordination_waits_returned(&events);

    let result = executor
        .invoke_and_await_agent_with_key(
            &component,
            &agent_id,
            &contention_key,
            CONTENTION_METHOD,
            data_value!(CONTENTION_TIMEOUT_MS),
        )
        .await?
        .into_typed::<(String, String)>()?;
    assert_eq!(
        sorted_reconnect_payloads(&result),
        expected_reconnect_payloads()
    );
    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    assert_contention_receive_correlation(&oplog);
    ws_server.assert_counts(2, 2);

    executor.check_oplog_is_queryable(&worker_id).await?;

    drop(executor);
    ws_server.abort();

    Ok(())
}

/// The per-handle coordination bounds the reconnect to a single call even when
/// the pool has spare permits: without the coordination both concurrent
/// receives would each acquire a permit and open their own connection. With
/// it, one accessor call reconnects and its peer reuses the published entry,
/// so only one new connection is opened.
#[test]
#[tracing::instrument]
#[timeout("3m")]
async fn websocket_reconnect_coordination_with_spare_pool_permits(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start_with_overrides(
        deps,
        &context,
        TestExecutorOverrides {
            configure: Some(Arc::new(|config| {
                config.max_websocket_connections = 2;
            })),
            ..Default::default()
        },
    )
    .await?;

    let ws_server = ReconnectTestServer::start().await;
    let url = ws_server.url();

    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;

    let agent_id = agent_id!("WebsocketTest", "ws-reconnect-spare");
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), HashMap::new(), Vec::new())
        .await?;

    let first = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "connect_and_receive_first",
            data_value!(url.clone()),
        )
        .await?;
    assert_eq!(first.into_typed::<String>()?, RECONNECT_FIRST_MESSAGE);
    ws_server.assert_counts(1, 1);

    let contention_key = IdempotencyKey::fresh();
    executor
        .invoke_agent_with_key(
            &component,
            &agent_id,
            &contention_key,
            CONTENTION_METHOD,
            data_value!(CONTENTION_TIMEOUT_MS),
        )
        .await?;
    executor
        .wait_for_status(&worker_id, AgentStatus::Running, Duration::from_secs(10))
        .await?;
    tokio::time::sleep(Duration::from_millis(500)).await;

    executor.interrupt(&worker_id).await?;
    executor
        .wait_for_status(
            &worker_id,
            AgentStatus::Interrupted,
            Duration::from_secs(10),
        )
        .await?;
    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    assert_contention_receives_incomplete(&oplog);

    let mut observation = executor.observe_websocket_reconnects_for_test();

    executor.resume(&worker_id, false).await?;

    let events =
        wait_for_coordination_events(&mut observation, Vec::new(), 1, 1, Duration::from_secs(10))
            .await?;
    ws_server.assert_counts(1, 1);

    observation.control.release_handshake();

    executor
        .wait_for_status(&worker_id, AgentStatus::Idle, Duration::from_secs(20))
        .await?;

    let events = drain_coordination_events(&mut observation, events).await;
    assert_coordination_events(
        &events,
        std::slice::from_ref(&contention_key),
        (1, 1, 0, 1, 1, 0),
    );
    assert_coordination_waits_returned(&events);

    let result = executor
        .invoke_and_await_agent_with_key(
            &component,
            &agent_id,
            &contention_key,
            CONTENTION_METHOD,
            data_value!(CONTENTION_TIMEOUT_MS),
        )
        .await?
        .into_typed::<(String, String)>()?;
    assert_eq!(
        sorted_reconnect_payloads(&result),
        expected_reconnect_payloads()
    );
    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    assert_contention_receive_correlation(&oplog);
    // Even with a spare permit, only one new connection was opened: the
    // queued call reused the published entry instead of reconnecting.
    ws_server.assert_counts(2, 2);

    executor.check_oplog_is_queryable(&worker_id).await?;

    drop(executor);
    ws_server.abort();

    Ok(())
}

/// The reconnect coordination is scoped per resource handle: two workers, each
/// with its own persisted websocket on its own server, reconnect their own
/// handles concurrently. Neither worker's queued call blocks on the other
/// worker's reconnect, both decided calls park on the held handshake
/// admission, and with two pool permits both handshakes proceed after their
/// releases. Every event is attributed to the handle's own contention
/// invocation key.
#[test]
#[tracing::instrument]
#[timeout("4m")]
async fn websocket_reconnect_coordination_is_per_handle(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start_with_overrides(
        deps,
        &context,
        TestExecutorOverrides {
            configure: Some(Arc::new(|config| {
                config.max_websocket_connections = 2;
            })),
            ..Default::default()
        },
    )
    .await?;

    let ws_server_a = ReconnectTestServer::start().await;
    let ws_server_b = ReconnectTestServer::start().await;

    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;

    let agent_id_a = agent_id!("WebsocketTest", "ws-reconnect-per-handle-a");
    let agent_id_b = agent_id!("WebsocketTest", "ws-reconnect-per-handle-b");
    let worker_id_a = executor
        .start_agent_with(
            &component.id,
            agent_id_a.clone(),
            HashMap::new(),
            Vec::new(),
        )
        .await?;
    let worker_id_b = executor
        .start_agent_with(
            &component.id,
            agent_id_b.clone(),
            HashMap::new(),
            Vec::new(),
        )
        .await?;

    let first_a = executor
        .invoke_and_await_agent(
            &component,
            &agent_id_a,
            "connect_and_receive_first",
            data_value!(ws_server_a.url()),
        )
        .await?;
    assert_eq!(first_a.into_typed::<String>()?, RECONNECT_FIRST_MESSAGE);
    let first_b = executor
        .invoke_and_await_agent(
            &component,
            &agent_id_b,
            "connect_and_receive_first",
            data_value!(ws_server_b.url()),
        )
        .await?;
    assert_eq!(first_b.into_typed::<String>()?, RECONNECT_FIRST_MESSAGE);
    ws_server_a.assert_counts(1, 1);
    ws_server_b.assert_counts(1, 1);

    let contention_key_a = IdempotencyKey::fresh();
    let contention_key_b = IdempotencyKey::fresh();
    executor
        .invoke_agent_with_key(
            &component,
            &agent_id_a,
            &contention_key_a,
            CONTENTION_METHOD,
            data_value!(CONTENTION_TIMEOUT_MS),
        )
        .await?;
    executor
        .invoke_agent_with_key(
            &component,
            &agent_id_b,
            &contention_key_b,
            CONTENTION_METHOD,
            data_value!(CONTENTION_TIMEOUT_MS),
        )
        .await?;
    executor
        .wait_for_status(&worker_id_a, AgentStatus::Running, Duration::from_secs(10))
        .await?;
    executor
        .wait_for_status(&worker_id_b, AgentStatus::Running, Duration::from_secs(10))
        .await?;
    tokio::time::sleep(Duration::from_millis(500)).await;

    executor.interrupt(&worker_id_a).await?;
    executor.interrupt(&worker_id_b).await?;
    executor
        .wait_for_status(
            &worker_id_a,
            AgentStatus::Interrupted,
            Duration::from_secs(10),
        )
        .await?;
    executor
        .wait_for_status(
            &worker_id_b,
            AgentStatus::Interrupted,
            Duration::from_secs(10),
        )
        .await?;
    let oplog_a = executor
        .get_oplog(&worker_id_a, OplogIndex::INITIAL)
        .await?;
    assert_contention_receives_incomplete(&oplog_a);
    let oplog_b = executor
        .get_oplog(&worker_id_b, OplogIndex::INITIAL)
        .await?;
    assert_contention_receives_incomplete(&oplog_b);

    let mut observation = executor.observe_websocket_reconnects_for_test();

    executor.resume(&worker_id_a, false).await?;
    executor.resume(&worker_id_b, false).await?;

    // Both handles' decided calls park on the held handshake admission and
    // both peers queue on their own per-handle gates.
    let events =
        wait_for_coordination_events(&mut observation, Vec::new(), 2, 2, Duration::from_secs(10))
            .await?;
    ws_server_a.assert_counts(1, 1);
    ws_server_b.assert_counts(1, 1);

    observation.control.release_handshake();
    observation.control.release_handshake();

    executor
        .wait_for_status(&worker_id_a, AgentStatus::Idle, Duration::from_secs(20))
        .await?;
    executor
        .wait_for_status(&worker_id_b, AgentStatus::Idle, Duration::from_secs(20))
        .await?;

    let events = drain_coordination_events(&mut observation, events).await;
    assert_coordination_events(
        &events,
        &[contention_key_a.clone(), contention_key_b.clone()],
        (2, 2, 0, 2, 2, 0),
    );
    assert_coordination_waits_returned(&events);
    // Each handle saw exactly one decided reconnect and one queued peer: the
    // coordination neither starved nor merged the two handles.
    for (key, worker) in [(&contention_key_a, "a"), (&contention_key_b, "b")] {
        let per_handle: Vec<&WebSocketReconnectEventForTest> = events
            .iter()
            .filter(|event| event.invocation_key.as_ref() == Some(key))
            .collect();
        assert_eq!(
            coordination_tallies(per_handle.iter().copied()),
            (1, 1, 0, 1, 1, 0),
            "unexpected per-handle reconnect tallies for handle {worker}: {per_handle:?}"
        );
    }

    let result_a = executor
        .invoke_and_await_agent_with_key(
            &component,
            &agent_id_a,
            &contention_key_a,
            CONTENTION_METHOD,
            data_value!(CONTENTION_TIMEOUT_MS),
        )
        .await?
        .into_typed::<(String, String)>()?;
    assert_eq!(
        sorted_reconnect_payloads(&result_a),
        expected_reconnect_payloads()
    );
    let result_b = executor
        .invoke_and_await_agent_with_key(
            &component,
            &agent_id_b,
            &contention_key_b,
            CONTENTION_METHOD,
            data_value!(CONTENTION_TIMEOUT_MS),
        )
        .await?
        .into_typed::<(String, String)>()?;
    assert_eq!(
        sorted_reconnect_payloads(&result_b),
        expected_reconnect_payloads()
    );
    let oplog_a = executor
        .get_oplog(&worker_id_a, OplogIndex::INITIAL)
        .await?;
    assert_contention_receive_correlation(&oplog_a);
    let oplog_b = executor
        .get_oplog(&worker_id_b, OplogIndex::INITIAL)
        .await?;
    assert_contention_receive_correlation(&oplog_b);
    ws_server_a.assert_counts(2, 2);
    ws_server_b.assert_counts(2, 2);

    executor.check_oplog_is_queryable(&worker_id_a).await?;
    executor.check_oplog_is_queryable(&worker_id_b).await?;

    drop(executor);
    ws_server_a.abort();
    ws_server_b.abort();

    Ok(())
}

/// A failing live reconnect must publish its terminal outcome per handle: the
/// decided accessor call marks the handle terminal instead of publishing a
/// live connection, its queued peer re-reads the terminal entry and fails
/// with the same error, and the pool permit the failed reconnect briefly held
/// is released rather than leaked — the next connection attempt on this
/// executor succeeds.
#[test]
#[tracing::instrument]
#[timeout("3m")]
async fn websocket_reconnect_failure_publishes_terminal_outcome(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start_with_overrides(
        deps,
        &context,
        TestExecutorOverrides {
            configure: Some(Arc::new(|config| {
                config.max_websocket_connections = 1;
            })),
            ..Default::default()
        },
    )
    .await?;

    let ws_server = ReconnectTestServer::start().await;
    let url = ws_server.url();

    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;

    let agent_id = agent_id!("WebsocketTest", "ws-reconnect-terminal");
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), HashMap::new(), Vec::new())
        .await?;

    let first = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "connect_and_receive_first",
            data_value!(url.clone()),
        )
        .await?;
    assert_eq!(first.into_typed::<String>()?, RECONNECT_FIRST_MESSAGE);
    ws_server.assert_counts(1, 1);

    let contention_key = IdempotencyKey::fresh();
    executor
        .invoke_agent_with_key(
            &component,
            &agent_id,
            &contention_key,
            CONTENTION_METHOD,
            data_value!(CONTENTION_TIMEOUT_MS),
        )
        .await?;
    executor
        .wait_for_status(&worker_id, AgentStatus::Running, Duration::from_secs(10))
        .await?;
    tokio::time::sleep(Duration::from_millis(500)).await;

    executor.interrupt(&worker_id).await?;
    executor
        .wait_for_status(
            &worker_id,
            AgentStatus::Interrupted,
            Duration::from_secs(10),
        )
        .await?;
    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    assert_contention_receives_incomplete(&oplog);

    let mut observation = executor.observe_websocket_reconnects_for_test();

    // The server is gone before the retry: the reconnect's live handshake
    // fails, and the failure is published terminally per handle.
    ws_server.abort();

    executor.resume(&worker_id, false).await?;

    let events =
        wait_for_coordination_events(&mut observation, Vec::new(), 1, 1, Duration::from_secs(10))
            .await?;
    ws_server.assert_counts(1, 1);

    observation.control.release_handshake();

    executor
        .wait_for_status(&worker_id, AgentStatus::Idle, Duration::from_secs(20))
        .await?;

    let events = drain_coordination_events(&mut observation, events).await;
    // The failing handshake may or may not record a wait before it resolves,
    // so its tally is checked separately from the deterministic ones.
    let tallies = coordination_tallies(&events);
    assert_eq!(
        (tallies.0, tallies.1, tallies.2, tallies.4, tallies.5),
        (1, 1, 0, 0, 1),
        "the failed reconnect must decide once, queue its peer on the gate, \
         take no pool wait and publish one terminal outcome: {events:?}"
    );
    assert!(
        tallies.3 <= 1,
        "at most one handshake wait may be recorded: {events:?}"
    );
    for event in &events {
        assert_eq!(
            event.path,
            WebSocketReconnectPathForTest::Accessor,
            "every observed reconnect must take the accessor path: {event:?}"
        );
        assert_eq!(
            event.invocation_key.as_ref(),
            Some(&contention_key),
            "every observed reconnect must belong to the contention invocation: {event:?}"
        );
    }
    assert_coordination_waits_returned(&events);

    let result = executor
        .invoke_and_await_agent_with_key(
            &component,
            &agent_id,
            &contention_key,
            CONTENTION_METHOD,
            data_value!(CONTENTION_TIMEOUT_MS),
        )
        .await?
        .into_typed::<(String, String)>()?;
    for (side, payload) in [("receive", &result.0), ("receive-with-timeout", &result.1)] {
        assert!(
            payload.contains("Receive error") && payload.contains("ConnectionFailure"),
            "the {side} result must report the failed reconnect, got {payload:?}"
        );
    }
    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    assert_contention_receive_correlation(&oplog);
    ws_server.assert_counts(1, 1);

    // The failed reconnect released the pool permit it briefly held: the pool
    // admits a new connection, and a fresh websocket on this executor
    // succeeds.
    let pool = executor
        .websocket_connection_pool()
        .expect("the executor's websocket connection pool is captured");
    let permit = tokio::time::timeout(Duration::from_secs(5), pool.acquire())
        .await
        .expect("the failed reconnect must not leak its pool permit")?;
    drop(permit);

    let ws_server_fresh = ReconnectTestServer::start().await;
    let connect_result = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "connect_result",
            data_value!(ws_server_fresh.url()),
        )
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected return value"))?;
    assert_eq!(
        connect_result,
        SchemaValue::Result(ResultValuePayload::Ok { value: None })
    );

    drop(executor);
    ws_server_fresh.abort();

    Ok(())
}

/// The reconnect coordination is interruptible at every park point and does not
/// leak pool permits across an interrupt: while the decided call is parked on
/// the held websocket handshake and its peer is queued on the per-handle gate,
/// an interrupt abandons both calls — the queued call's gate wait is observed
/// dropped, neither a handshake wait nor a published outcome is recorded, and
/// the interrupted reconnect releases its pool permit. The retry after
/// resuming repeats the coordination on the same retained invocation, and
/// releasing the held handshake then lets the reconnect complete.
#[test]
#[tracing::instrument]
#[timeout("4m")]
async fn websocket_reconnect_coordination_is_interruptible(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start_with_overrides(
        deps,
        &context,
        TestExecutorOverrides {
            configure: Some(Arc::new(|config| {
                config.max_websocket_connections = 1;
            })),
            ..Default::default()
        },
    )
    .await?;

    let ws_server = ReconnectTestServer::start().await;
    let url = ws_server.url();

    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;

    let agent_id = agent_id!("WebsocketTest", "ws-reconnect-interruptible");
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), HashMap::new(), Vec::new())
        .await?;

    let first = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "connect_and_receive_first",
            data_value!(url.clone()),
        )
        .await?;
    assert_eq!(first.into_typed::<String>()?, RECONNECT_FIRST_MESSAGE);
    ws_server.assert_counts(1, 1);

    let contention_key = IdempotencyKey::fresh();
    executor
        .invoke_agent_with_key(
            &component,
            &agent_id,
            &contention_key,
            CONTENTION_METHOD,
            data_value!(CONTENTION_TIMEOUT_MS),
        )
        .await?;
    executor
        .wait_for_status(&worker_id, AgentStatus::Running, Duration::from_secs(10))
        .await?;
    tokio::time::sleep(Duration::from_millis(500)).await;

    executor.interrupt(&worker_id).await?;
    executor
        .wait_for_status(
            &worker_id,
            AgentStatus::Interrupted,
            Duration::from_secs(10),
        )
        .await?;
    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    assert_contention_receives_incomplete(&oplog);

    let mut observation = executor.observe_websocket_reconnects_for_test();

    executor.resume(&worker_id, false).await?;

    // One receive decides to reconnect and parks on the held websocket
    // handshake; its peer queues on the per-handle gate.
    let events =
        wait_for_coordination_events(&mut observation, Vec::new(), 1, 1, Duration::from_secs(10))
            .await?;
    ws_server.assert_counts(1, 1);

    // Interrupting abandons both parked calls without releasing the held
    // handshake: no handshake wait and no published outcome may be recorded.
    executor.interrupt(&worker_id).await?;
    executor
        .wait_for_status(
            &worker_id,
            AgentStatus::Interrupted,
            Duration::from_secs(10),
        )
        .await?;

    let events = drain_coordination_events(&mut observation, events).await;
    assert_coordination_events(
        &events,
        std::slice::from_ref(&contention_key),
        (1, 1, 0, 0, 0, 0),
    );
    let dropped: Vec<&WebSocketReconnectEventForTest> = events
        .iter()
        .filter(|event| is_dropped_reconnect_wait(event))
        .collect();
    assert_eq!(
        dropped.len(),
        1,
        "exactly the queued gate wait must be abandoned by the interrupt: {events:?}"
    );
    assert!(
        matches!(
            dropped[0].kind,
            WebSocketReconnectEventKindForTest::WaitPending {
                wait: WebSocketReconnectWaitForTest::Gate
            }
        ),
        "the abandoned wait must be the per-handle gate wait: {events:?}"
    );
    ws_server.assert_counts(1, 1);

    // The interrupted reconnect released the pool permit it held while parked
    // on the handshake admission.
    let pool = executor
        .websocket_connection_pool()
        .expect("the executor's websocket connection pool is captured");
    let permit = tokio::time::timeout(Duration::from_secs(5), pool.acquire())
        .await
        .expect("the interrupted reconnect must not leak its pool permit")?;
    drop(permit);

    // The retry repeats the coordination on the same retained invocation.
    executor.resume(&worker_id, false).await?;

    let events =
        wait_for_coordination_events(&mut observation, events, 2, 2, Duration::from_secs(10))
            .await?;
    ws_server.assert_counts(1, 1);

    observation.control.release_handshake();

    executor
        .wait_for_status(&worker_id, AgentStatus::Idle, Duration::from_secs(20))
        .await?;

    let events = drain_coordination_events(&mut observation, events).await;
    assert_coordination_events(
        &events,
        std::slice::from_ref(&contention_key),
        (2, 2, 0, 1, 1, 0),
    );
    // Every wait of the completed round returned; the interrupted round's
    // gate wait stayed dropped.
    for event in &events {
        if let Some(state) = event.wait_state() {
            if state == WebSocketReconnectWaitStateForTest::Dropped {
                assert!(
                    is_dropped_reconnect_wait(event)
                        && matches!(
                            event.kind,
                            WebSocketReconnectEventKindForTest::WaitPending {
                                wait: WebSocketReconnectWaitForTest::Gate
                            }
                        ),
                    "only the interrupted round's gate wait may stay dropped: {event:?}"
                );
            } else {
                assert_eq!(
                    state,
                    WebSocketReconnectWaitStateForTest::Returned,
                    "every other observed reconnect wait must return: {event:?}"
                );
            }
        }
    }

    let result = executor
        .invoke_and_await_agent_with_key(
            &component,
            &agent_id,
            &contention_key,
            CONTENTION_METHOD,
            data_value!(CONTENTION_TIMEOUT_MS),
        )
        .await?
        .into_typed::<(String, String)>()?;
    assert_eq!(
        sorted_reconnect_payloads(&result),
        expected_reconnect_payloads()
    );
    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    assert_contention_receive_correlation(&oplog);
    ws_server.assert_counts(2, 2);

    executor.check_oplog_is_queryable(&worker_id).await?;

    drop(executor);
    ws_server.abort();

    Ok(())
}
