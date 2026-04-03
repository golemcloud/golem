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
use golem_common::model::agent::{
    ComponentModelElementValue, DataValue, ElementValue, ElementValues,
};
use golem_common::model::oplog::OplogIndex;
use golem_common::model::{AgentStatus, IdempotencyKey, PromiseId};
use golem_common::{agent_id, data_value};
use golem_test_framework::dsl::TestDsl;
use golem_wasm::{IntoValue, IntoValueAndType, Value, ValueAndType};
use golem_worker_executor_test_utils::{
    LastUniqueId, PrecompiledComponent, TestContext, WorkerExecutorTestDependencies, start,
};
use pretty_assertions::assert_eq;
use std::collections::HashMap;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
    Mutex,
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
        .start_agent_with(
            &component.id,
            agent_id.clone(),
            env_vars,
            HashMap::new(),
            Vec::new(),
        )
        .await?;

    let result = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "echo",
            data_value!(format!("ws://localhost:{ws_port}"), "hello websocket"),
        )
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected return value"))?;

    assert_eq!(result, Value::String("hello websocket".to_string()));

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
        .start_agent_with(
            &component.id,
            agent_id.clone(),
            env_vars,
            HashMap::new(),
            Vec::new(),
        )
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
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected return value"))?;

    assert_eq!(first_result, Value::String("hello websocket".to_string()));

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
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected return value"))?;

    assert_eq!(
        second_result,
        Value::String("hello websocket|hello websocket 2".to_string())
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
        .start_agent_with(
            &component.id,
            agent_id.clone(),
            env_vars,
            HashMap::new(),
            Vec::new(),
        )
        .await?;

    let promise_id_value = executor
        .invoke_and_await_agent(&component, &agent_id, "create_promise", data_value!())
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected return value"))?;
    let params = replay_reconnect_roundtrip_params(
        format!("ws://localhost:{ws_port}"),
        &promise_id_value,
    );
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
        Value::Result(Ok(Some(Box::new(Value::String(
            "msg-1|msg-2|msg-3|msg-4".to_string()
        )))))
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
async fn websocket_subscribe_does_not_reconnect_during_replay(
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
                let ws_stream = tokio_tungstenite::accept_async(stream)
                    .await
                    .expect("WS handshake failed");
                let (mut write, _read) = StreamExt::split(ws_stream);
                for i in 0..5u32 {
                    let msg = tokio_tungstenite::tungstenite::Message::text(format!("msg-{i}"));
                    if SinkExt::send(&mut write, msg).await.is_err() {
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

    let agent_id = agent_id!("WebsocketTest", "ws-subscribe-replay-guard");
    let worker_id = executor
        .start_agent_with(
            &component.id,
            agent_id.clone(),
            env_vars,
            HashMap::new(),
            Vec::new(),
        )
        .await?;

    let first = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "connect_subscribe_and_receive_first",
            data_value!(format!("ws://localhost:{ws_port}")),
        )
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected return value"))?;
    assert_eq!(first, Value::String("msg-0".to_string()));
    assert_eq!(accepted_connections.load(Ordering::SeqCst), 1);

    executor.check_oplog_is_queryable(&worker_id).await?;
    drop(executor);

    let executor = start(deps, &context).await?;
    let activation_result = executor
        .invoke_and_await_agent(&component, &agent_id, "noop", data_value!())
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected return value"))?;
    assert_eq!(activation_result, Value::String("ok".to_string()));
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(
        accepted_connections.load(Ordering::SeqCst),
        1,
        "replay-time subscribe must not reconnect before replay finishes"
    );

    let next = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "receive_next_from_persisted",
            data_value!(),
        )
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected return value"))?;
    assert_eq!(next, Value::String("msg-0".to_string()));
    assert_eq!(accepted_connections.load(Ordering::SeqCst), 2);

    executor.check_oplog_is_queryable(&worker_id).await?;
    drop(executor);
    ws_server.abort();

    Ok(())
}

fn replay_reconnect_roundtrip_params(url: String, promise_id_value: &Value) -> DataValue {
    let promise_id_vat = ValueAndType::new(promise_id_value.clone(), PromiseId::get_type());

    DataValue::Tuple(ElementValues {
        elements: vec![
            ElementValue::ComponentModel(ComponentModelElementValue {
                value: url.into_value_and_type(),
            }),
            ElementValue::ComponentModel(ComponentModelElementValue {
                value: promise_id_vat,
            }),
        ],
    })
}

fn extract_oplog_idx_from_promise_id(promise_id_value: &Value) -> OplogIndex {
    let Value::Record(fields) = promise_id_value else {
        panic!("Expected a record for PromiseId");
    };
    let Value::U64(oplog_idx) = fields[1] else {
        panic!("Expected second PromiseId field to be oplog_idx");
    };

    OplogIndex::from_u64(oplog_idx)
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
        .start_agent_with(
            &component.id,
            agent_id.clone(),
            env_vars,
            HashMap::new(),
            Vec::new(),
        )
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

    assert_eq!(result, Value::Option(None));

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
        .start_agent_with(
            &component.id,
            agent_id.clone(),
            env_vars,
            HashMap::new(),
            Vec::new(),
        )
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
        Value::Result(Ok(Some(Box::new(Value::String(message.to_string())))))
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
        .start_agent_with(
            &component.id,
            agent_id.clone(),
            env_vars,
            HashMap::new(),
            Vec::new(),
        )
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
        Value::Result(Ok(Some(Box::new(Value::String(message.to_string())))))
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
        .start_agent_with(
            &component.id,
            agent_id.clone(),
            env_vars,
            HashMap::new(),
            Vec::new(),
        )
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
        Value::Result(Ok(Some(Box::new(Value::String(
            "msg-a|msg-b|msg-c".to_string()
        )))))
    );

    executor.check_oplog_is_queryable(&worker_id).await?;

    drop(executor);
    ws_server.abort();

    Ok(())
}
