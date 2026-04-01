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
use golem_common::{agent_id, data_value};
use golem_test_framework::dsl::TestDsl;
use golem_wasm::Value;
use golem_worker_executor_test_utils::{
    LastUniqueId, PrecompiledComponent, TestContext, WorkerExecutorTestDependencies, start,
};
use pretty_assertions::assert_eq;
use std::collections::HashMap;
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
async fn websocket_reconnect_after_replay_continues_stream(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await.unwrap();
    let ws_port = listener.local_addr().unwrap().port();

    // Stateless server: every new websocket connection emits msg-0, msg-1, ...
    // If reconnect+fast-forward works, post-restart reads should continue from
    // the last observed message index instead of repeating msg-0.
    let ws_server = spawn(
        async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    break;
                };
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

    // First invocation creates a websocket in agent state and reads first frame.
    let first = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "connect_and_receive_first",
            data_value!(format!("ws://localhost:{ws_port}")),
        )
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected return value"))?;
    assert_eq!(first, Value::String("msg-0".to_string()));

    executor.check_oplog_is_queryable(&worker_id).await?;
    // Simulate executor crash/restart.
    drop(executor);

    // Next invocation activates the worker again:
    // 1) replay restores persisted websocket metadata
    // 2) host reconnects and fast-forwards before serving read
    let executor = start(deps, &context).await?;
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
    // Must be msg-1 (not msg-0): proves replay+reconnect resumed stream position.
    assert_eq!(next, Value::String("msg-1".to_string()));

    executor.check_oplog_is_queryable(&worker_id).await?;
    drop(executor);
    ws_server.abort();

    Ok(())
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
