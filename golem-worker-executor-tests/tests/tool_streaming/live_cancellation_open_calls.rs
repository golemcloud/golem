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

//! Live cancellation of a Wasm tool body that holds an open call: the caller gets the cancelled
//! result while the call is still in flight.

use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};
use test_r::test;

/// Accepts TCP connections, answers each request at once with the headers of a chunked `200`
/// response, then reads everything the client sends and never sends a body or finishes.
async fn start_held_http_peer() -> (
    u16,
    Arc<AtomicUsize>,
    tokio_util::task::AbortOnDropHandle<()>,
) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind held HTTP peer");
    let port = listener.local_addr().expect("held HTTP address").port();
    let connections = Arc::new(AtomicUsize::new(0));
    let accepted = connections.clone();
    let peer = tokio_util::task::AbortOnDropHandle::new(tokio::spawn(async move {
        let mut held = tokio::task::JoinSet::new();
        loop {
            let (mut socket, _) = listener.accept().await.expect("accept a held request");
            accepted.fetch_add(1, Ordering::SeqCst);
            held.spawn(async move {
                let mut buffer = [0u8; 4096];
                if socket.read(&mut buffer).await.is_ok_and(|read| read > 0) {
                    let _ = socket
                        .write_all(b"HTTP/1.1 200 OK\r\ntransfer-encoding: chunked\r\n\r\n")
                        .await;
                }
                while socket.read(&mut buffer).await.is_ok_and(|read| read > 0) {}
            });
        }
    }));
    (port, connections, peer)
}

/// A caller agent whose tool body ran the provider `mode` through `cancel_run_then_receive` and
/// was cancelled live once it waited at its `open-call-held` checkpoint, while its call
/// `open_call` was in flight under the body's entity `Start` at `body`.
struct CancelledBody {
    context: TestContext,
    overrides: TestExecutorOverrides,
    executor: TestWorkerExecutor,
    caller_component: golem_common::model::component::ComponentDto,
    agent_id: golem_common::model::agent::ParsedAgentKey,
    worker_id: golem_common::model::OwnedAgentId,
    result: String,
    body: OplogIndex,
    open_call: OplogIndex,
    http_connections: Arc<AtomicUsize>,
    _peers: (
        tokio_util::task::AbortOnDropHandle<()>,
        tokio_util::task::AbortOnDropHandle<()>,
    ),
}

async fn cancel_body_with_open_call(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    provider: &PrecompiledComponent,
    caller: &PrecompiledComponent,
    mode: &str,
    open_function: &str,
) -> anyhow::Result<CancelledBody> {
    let (peer_port, _peer_connections, websocket_peer) = start_held_websocket_peer().await;
    let (http_port, http_connections, http_peer) = start_held_http_peer().await;
    let context = TestContext::new(last_unique_id);
    let environment_state = Arc::new(TestEnvironmentStateService::default());
    let overrides = TestExecutorOverrides {
        environment_state_service: Some(environment_state.clone()),
        ..Default::default()
    };
    let executor = start_with_agent_sockets(deps, &context, overrides.clone()).await?;
    let provider_component = executor
        .component_dep(&context.default_environment_id, provider)
        .store()
        .await?;
    let caller_component = executor
        .component_dep(&context.default_environment_id, caller)
        .store()
        .await?;
    let metadata = extract_component_metadata(
        &deps
            .component_directory
            .join(format!("{}.wasm", provider.wasm_name)),
        false,
        true,
    )
    .await?;
    environment_state.set_tool_deployment(
        context.default_environment_id,
        caller_component.id,
        caller_component.revision,
        Some(deployment_state(
            context.account_id,
            provider_component.id,
            provider_component.revision,
            "golem-it:tool-streaming-rust-provider",
            "ToolStreamingCaller",
            metadata.tools,
        )),
    );
    let (checkpoint_port, checkpoint_server, mut checkpoints) =
        start_promise_checkpoint_server().await;
    let agent_id = agent_key!("ToolStreamingCaller", mode);
    let worker_id = executor
        .start_agent_with(
            &caller_component.id,
            agent_id.clone(),
            HashMap::from([
                (
                    "PROVIDER_PROMISE_CHECKPOINT_PORT".to_string(),
                    checkpoint_port.to_string(),
                ),
                ("WEBSOCKET_HOLD_PORT".to_string(), peer_port.to_string()),
                ("HTTP_HOLD_PORT".to_string(), http_port.to_string()),
            ]),
            Vec::new(),
        )
        .await?;
    let call_executor = executor.clone();
    let call_component = caller_component.clone();
    let call_agent = agent_id.clone();
    let call_mode = mode.to_string();
    let call = tokio::spawn(async move {
        call_executor
            .invoke_and_await_agent(
                &call_component,
                &call_agent,
                "cancel_run_then_receive",
                data_value!(call_mode, format!("ws://127.0.0.1:{peer_port}/marker")),
            )
            .await?
            .into_typed::<String>()
    });

    let checkpoint = next_promise_checkpoint(&mut checkpoints, "open-call-held").await?;
    wait_for_promise_checkpoint_to_await(&executor, &worker_id.agent_id, &checkpoint).await?;
    let (body, open_call) =
        await_open_call(&executor, &worker_id, checkpoint.oplog_idx, open_function).await?;
    executor
        .complete_promise(
            &worker_id.environment_id,
            &executor
                .promise_id(&worker_id, checkpoint.oplog_idx)
                .await?,
            vec![],
        )
        .await?;
    let result = tokio::time::timeout(Duration::from_secs(60), call)
        .await
        .map_err(|_| {
            anyhow::anyhow!("the cancelled tool call returned no result within 60 s")
        })???;
    checkpoint_server.abort();
    Ok(CancelledBody {
        context,
        overrides,
        executor,
        caller_component,
        agent_id,
        worker_id,
        result,
        body,
        open_call,
        http_connections,
        _peers: (websocket_peer, http_peer),
    })
}

fn terminals(oplog: &[PublicOplogEntryWithIndex], start: OplogIndex) -> Vec<&PublicOplogEntry> {
    oplog
        .iter()
        .map(|entry| &entry.entry)
        .filter(|entry| match entry {
            PublicOplogEntry::End(end) => end.start_index == start,
            PublicOplogEntry::Cancelled(cancelled) => cancelled.start_index == start,
            _ => false,
        })
        .collect()
}

/// Waits until a `Start` of `open_function` without a terminal is recorded under the tool body
/// that holds the checkpoint at `checkpoint`, and returns the body's and the call's `Start`.
async fn await_open_call(
    executor: &TestWorkerExecutor,
    worker_id: &golem_common::model::OwnedAgentId,
    checkpoint: OplogIndex,
    open_function: &str,
) -> anyhow::Result<(OplogIndex, OplogIndex)> {
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            executor.commit_oplog(&worker_id.agent_id).await?;
            let oplog = executor.get_oplog(worker_id, OplogIndex::INITIAL).await?;
            let body = nearest_entity_body(&oplog, checkpoint)
                .expect("the checkpoint runs in the tool body");
            let open = oplog
                .iter()
                .filter(|entry| {
                    matches!(
                        &entry.entry,
                        PublicOplogEntry::Start(start) if start.function_name == open_function
                    ) && nearest_entity_body(&oplog, entry.oplog_index) == Some(body)
                        && terminals(&oplog, entry.oplog_index).is_empty()
                })
                .map(|entry| entry.oplog_index)
                .collect::<Vec<_>>();
            match open.as_slice() {
                [] => tokio::time::sleep(Duration::from_millis(50)).await,
                [open_call] => return Ok::<_, anyhow::Error>((body, *open_call)),
                _ => anyhow::bail!("expected one {open_function} in flight: {open:?}"),
            }
        }
    })
    .await
    .map_err(|_| anyhow::anyhow!("no {open_function} was in flight under the tool body"))?
}

/// A Wasm tool body sends a request to a peer that answers with headers and then holds the
/// connection, keeps the request body open, and is cancelled by its caller after its first stdout
/// chunk. The request-body transmission is still in flight: its result can arrive only when the
/// body's Store unwinds, so the cancellation must not wait for it. The transmission's
/// `LeaveIncompleteOnDrop` `Start` stays without a terminal under the body's `Cancelled`. Two
/// restarts replay the settled history without sending the request again, and a fresh invocation
/// runs live after each restart.
#[test]
#[tracing::instrument]
#[timeout("4m")]
async fn live_cancellation_of_a_wasm_tool_body_with_an_open_request_body_transmission(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_provider")] provider: &PrecompiledComponent,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let cancelled = cancel_body_with_open_call(
        last_unique_id,
        deps,
        provider,
        caller,
        "open-transmit-wait",
        "http::types::request::body-transmission",
    )
    .await?;
    assert_eq!(
        cancelled.result,
        "open-call-held:Err(ToolRpcError::Cancelled):None"
    );
    let CancelledBody {
        context,
        overrides,
        mut executor,
        caller_component,
        agent_id,
        worker_id,
        body,
        open_call,
        http_connections,
        ..
    } = cancelled;

    let live = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    assert_cancelled_open_call_history(&live, body, open_call);
    let body_cancelled = live
        .iter()
        .find_map(|entry| match &entry.entry {
            PublicOplogEntry::Cancelled(cancelled) if cancelled.start_index == body => {
                Some(entry.oplog_index)
            }
            _ => None,
        })
        .expect("the body is cancelled");
    let caller_receive = live
        .iter()
        .find_map(|entry| match &entry.entry {
            PublicOplogEntry::Start(start)
                if start.function_name == "golem:websocket/client::receive-with-timeout" =>
            {
                Some(entry.oplog_index)
            }
            _ => None,
        })
        .expect("the caller's receive is recorded");
    assert!(
        open_call < body_cancelled && body_cancelled < caller_receive,
        "the caller's receive at {caller_receive} follows the body's Cancelled at \
         {body_cancelled}, which follows the body's open transmission at {open_call}"
    );
    assert_eq!(http_connections.load(Ordering::SeqCst), 1);

    let mut history = live;
    for restart in 1..=2 {
        drop(executor);
        executor = start_with_agent_sockets(deps, &context, overrides.clone()).await?;
        let probe: String = tokio::time::timeout(
            Duration::from_secs(60),
            executor.invoke_and_await_agent(
                &caller_component,
                &agent_id,
                "replay_probe",
                data_value!(),
            ),
        )
        .await
        .map_err(|_| anyhow::anyhow!("the fresh invocation did not complete (restart {restart})"))??
        .into_typed()?;
        assert_eq!(probe, "replayed");
        let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
        let fresh = history.len();
        assert_eq!(
            oplog[..fresh]
                .iter()
                .map(|entry| entry.oplog_index)
                .collect::<Vec<_>>(),
            history
                .iter()
                .map(|entry| entry.oplog_index)
                .collect::<Vec<_>>(),
            "the replay kept the recorded history (restart {restart})"
        );
        assert!(
            !oplog
                .iter()
                .any(|entry| matches!(entry.entry, PublicOplogEntry::Error(_))),
            "the replay recorded no error (restart {restart})"
        );
        assert_cancelled_open_call_history(&oplog, body, open_call);
        assert_eq!(
            http_connections.load(Ordering::SeqCst),
            1,
            "replaying the cancelled body sent the held request again (restart {restart})"
        );
        history = oplog;
    }
    executor.delete_worker(&worker_id).await?;
    Ok(())
}

/// A Wasm tool body holds a nested tool call whose stdin stays open and is cancelled by its
/// caller after its first stdout chunk. The caller gets the cancelled result, and the
/// cancellation gives the nested call and the body one `Cancelled` each and leaves no `Start`
/// without a terminal.
#[test]
#[tracing::instrument]
#[timeout("3m")]
async fn live_cancellation_of_a_wasm_tool_body_with_an_open_nested_invoke(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_provider")] provider: &PrecompiledComponent,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let cancelled = cancel_body_with_open_call(
        last_unique_id,
        deps,
        provider,
        caller,
        "open-nested-invoke-wait",
        "golem::entity::invoke",
    )
    .await?;
    assert_eq!(
        cancelled.result,
        "open-call-held:Err(ToolRpcError::Cancelled):None"
    );
    let live = cancelled
        .executor
        .get_oplog(&cancelled.worker_id, OplogIndex::INITIAL)
        .await?;
    for (name, start) in [
        ("body", cancelled.body),
        ("nested call", cancelled.open_call),
    ] {
        let found = terminals(&live, start);
        assert!(
            matches!(found.as_slice(), [PublicOplogEntry::Cancelled(_)]),
            "the {name} at {start} has one terminal, a Cancelled: {found:?}"
        );
    }
    for entry in &live {
        if let PublicOplogEntry::Start(start) = &entry.entry {
            assert!(
                !terminals(&live, entry.oplog_index).is_empty(),
                "the Start of {} at {} has no terminal",
                start.function_name,
                entry.oplog_index
            );
        }
    }
    cancelled
        .executor
        .delete_worker(&cancelled.worker_id)
        .await?;
    Ok(())
}
