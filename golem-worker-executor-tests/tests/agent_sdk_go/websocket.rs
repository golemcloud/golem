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

//! The Go SDK's websocket wrapper drives a full client lifecycle — connect, send,
//! receive, close — against a real server. Mirrors `websocket::websocket_echo_ts`.

use crate::Tracing;
use futures::{SinkExt, StreamExt};
use golem_common::{agent_id, data_value};
use golem_test_framework::dsl::TestDsl;
use golem_worker_executor_test_utils::{
    LastUniqueId, PrecompiledComponent, TestContext, WorkerExecutorTestDependencies, start,
};
use std::collections::HashMap;
use test_r::{inherit_test_dep, test, timeout};
use tokio::spawn;
use tracing::Instrument;

inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(LastUniqueId);
inherit_test_dep!(Tracing);
inherit_test_dep!(
    #[tagged_as("agent_sdk_go")]
    PrecompiledComponent
);

/// A message sent through the wrapper comes back from the echo server, proving
/// connect/send/receive/close all work over the durable websocket transport.
#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn go_websocket_echo(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("agent_sdk_go")] agent_sdk_go: &PrecompiledComponent,
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
        .component_dep(&context.default_environment_id, agent_sdk_go)
        .store()
        .await?;
    let agent_id = agent_id!("WsAgent", "go-ws-1");
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), HashMap::new(), Vec::new())
        .await?;

    let echoed = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "echo",
            data_value!(format!("ws://localhost:{ws_port}"), "hello from go"),
        )
        .await?
        .into_typed::<String>()?;

    executor.check_oplog_is_queryable(&worker_id).await?;
    drop(executor);
    ws_server.abort();

    assert_eq!(echoed, "hello from go");
    Ok(())
}
