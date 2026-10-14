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
use axum::Router;
use axum::body::{Body, Bytes};
use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::Response;
use axum::routing::get;
use golem_common::model::IdempotencyKey;
use golem_common::model::agent_secret::{
    AgentSecretId, AgentSecretRevision, CanonicalAgentSecretPath,
};
use golem_common::model::oplog::host_functions::host_request_from_typed_schema_value;
use golem_common::model::oplog::payload::{
    HostRequestDurableStreamAppend, HostRequestDurableStreamWriterNew,
};
use golem_common::model::oplog::{OplogIndex, PublicOplogEntry};
use golem_common::schema::{SchemaGraph, SchemaType, SchemaValue};
use golem_common::{agent_id, data_value};
use golem_service_base::model::agent_secret::AgentSecret;
use golem_test_framework::dsl::TestDsl;
use golem_worker_executor_test_utils::agent_deployments_service::TestEnvironmentStateService;
use golem_worker_executor_test_utils::{
    LastUniqueId, PrecompiledComponent, TestContext, TestExecutorOverrides,
    WorkerExecutorTestDependencies, start, start_with_overrides,
};
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use test_r::{inherit_test_dep, test, timeout};
use tokio::sync::Notify;
use tokio::task::JoinHandle;

inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(LastUniqueId);
inherit_test_dep!(Tracing);
inherit_test_dep!(
    #[tagged_as("external_durable_streams")]
    PrecompiledComponent
);

#[derive(Clone, Debug, PartialEq, Eq)]
struct AppendAttempt {
    producer: String,
    epoch: String,
    sequence: String,
    body: Vec<u8>,
    close: bool,
}

struct Peer {
    reads: AtomicUsize,
    authenticated_requests: AtomicUsize,
    attempts: Mutex<Vec<AppendAttempt>>,
    effects: Mutex<Vec<AppendAttempt>>,
    hold_first_append: bool,
    accepted: Notify,
    release: Notify,
}

struct Server {
    peer: Arc<Peer>,
    url: String,
    task: JoinHandle<()>,
}

impl Drop for Server {
    fn drop(&mut self) {
        self.peer.release.notify_waiters();
        self.task.abort();
    }
}

impl Server {
    async fn start(hold_first_append: bool) -> anyhow::Result<Self> {
        let peer = Arc::new(Peer {
            reads: AtomicUsize::new(0),
            authenticated_requests: AtomicUsize::new(0),
            attempts: Mutex::new(Vec::new()),
            effects: Mutex::new(Vec::new()),
            hold_first_append,
            accepted: Notify::new(),
            release: Notify::new(),
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let url = format!("http://{}/stream", listener.local_addr()?);
        let router = Router::new()
            .route("/stream", get(read).post(append))
            .with_state(peer.clone());
        let task = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        Ok(Self { peer, url, task })
    }
}

async fn read(
    State(peer): State<Arc<Peer>>,
    Query(query): Query<HashMap<String, String>>,
    headers: HeaderMap,
) -> Response {
    peer.reads.fetch_add(1, Ordering::SeqCst);
    if headers
        .get("authorization")
        .is_some_and(|value| value == "Bearer test-ds-secret")
    {
        peer.authenticated_requests.fetch_add(1, Ordering::SeqCst);
    }
    if query.get("hold").is_some_and(|hold| hold == "true") {
        peer.accepted.notify_one();
        peer.release.notified().await;
    }
    let offset = query.get("offset").map(String::as_str).unwrap_or("");
    let body = match offset {
        "-1" if query.get("bytes").is_some_and(|bytes| bytes == "true") => {
            return Response::builder()
                .header("content-type", "application/octet-stream")
                .header("stream-next-offset", "tail-opaque")
                .header("stream-up-to-date", "true")
                .header("stream-closed", "true")
                .body(Body::from(vec![0, 255, 17, 128, 3]))
                .unwrap();
        }
        "-1" if query
            .get("strings")
            .is_some_and(|strings| strings == "true") =>
        {
            r#"["alpha","βeta","gamma"]"#
        }
        "-1" if query.get("label").is_some_and(|label| label == "a") => r#"["branch-a"]"#,
        "-1" if query.get("label").is_some_and(|label| label == "b") => r#"["branch-b"]"#,
        "-1" => r#"[[7,3],9007199254740993,{"a":false}]"#,
        "now" | "tail-opaque" => "[]",
        _ => {
            return Response::builder()
                .status(StatusCode::GONE)
                .body(Body::empty())
                .unwrap();
        }
    };
    Response::builder()
        .header("content-type", "application/json")
        .header("stream-next-offset", "tail-opaque")
        .header("stream-up-to-date", "true")
        .header("stream-closed", "true")
        .body(Body::from(body))
        .unwrap()
}

async fn append(
    State(peer): State<Arc<Peer>>,
    Query(query): Query<HashMap<String, String>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if headers
        .get("authorization")
        .is_some_and(|value| value == "Bearer test-ds-secret")
    {
        peer.authenticated_requests.fetch_add(1, Ordering::SeqCst);
    }
    let attempt = AppendAttempt {
        producer: headers["producer-id"].to_str().unwrap().to_owned(),
        epoch: headers["producer-epoch"].to_str().unwrap().to_owned(),
        sequence: headers["producer-seq"].to_str().unwrap().to_owned(),
        body: body.to_vec(),
        close: headers
            .get("stream-closed")
            .is_some_and(|value| value == "true"),
    };
    let first = {
        let mut attempts = peer.attempts.lock().unwrap();
        attempts.push(attempt.clone());
        attempts.len() == 1
    };
    let (duplicate, offset) = {
        let mut effects = peer.effects.lock().unwrap();
        let previous = effects.iter().position(|previous| {
            previous.producer == attempt.producer
                && previous.epoch == attempt.epoch
                && previous.sequence == attempt.sequence
        });
        if let Some(index) = previous {
            assert_eq!(
                effects[index], attempt,
                "a producer tuple must never change its payload or close flag"
            );
            (true, index + 1)
        } else {
            effects.push(attempt.clone());
            (false, effects.len())
        }
    };
    if (first && peer.hold_first_append) || query.get("hold").is_some_and(|hold| hold == "true") {
        peer.accepted.notify_one();
        peer.release.notified().await;
    }
    let mut response = Response::builder()
        .status(if duplicate {
            StatusCode::NO_CONTENT
        } else {
            StatusCode::OK
        })
        .header("producer-epoch", attempt.epoch)
        .header("producer-seq", attempt.sequence);
    if !duplicate || attempt.close {
        response = response.header("stream-next-offset", format!("receipt-{offset}"));
    }
    if attempt.close {
        response = response.header("stream-closed", "true");
    }
    response.body(Body::empty()).unwrap()
}

#[test]
#[timeout("120s")]
async fn completed_read_replays_without_http_and_preserves_json_lexemes(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("external_durable_streams")] fixture: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let server = Server::start(false).await?;
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;
    let component = executor
        .component_dep(&context.default_environment_id, fixture)
        .store()
        .await?;
    let agent = agent_id!("ExternalDurableStreams", "read-replay");
    executor.start_agent(&component.id, agent.clone()).await?;
    let expected = vec![
        "[7,3]".to_string(),
        "9007199254740993".to_string(),
        r#"{"a":false}"#.to_string(),
    ];
    let result = executor
        .invoke_and_await_agent(
            &component,
            &agent,
            "consume_json",
            data_value!(server.url.clone(), "-1", "catch-up", 10u32),
        )
        .await?
        .into_typed::<Result<Vec<String>, String>>()?;
    assert_eq!(result, Ok(expected.clone()));
    assert_eq!(server.peer.reads.load(Ordering::SeqCst), 1);
    drop(executor);
    let executor = start(deps, &context).await?;
    let result = executor
        .invoke_and_await_agent(
            &component,
            &agent,
            "consume_json",
            data_value!(server.url.clone(), "-1", "catch-up", 10u32),
        )
        .await?
        .into_typed::<Result<Vec<String>, String>>()?;
    assert_eq!(result, Ok(expected));
    assert_eq!(
        server.peer.reads.load(Ordering::SeqCst),
        2,
        "one fresh call after restart, zero HTTP for completed replay"
    );
    Ok(())
}

#[test]
#[timeout("120s")]
async fn external_streams_forward_through_native_agent_rpc_and_replay(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("external_durable_streams")] fixture: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let server = Server::start(false).await?;
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;
    let component = executor
        .component_dep(&context.default_environment_id, fixture)
        .store()
        .await?;
    let caller = agent_id!("ExternalDurableStreams", "forward-caller");
    let source = agent_id!("ExternalDurableStreams", "forward-source");
    executor.start_agent(&component.id, caller.clone()).await?;
    executor.start_agent(&component.id, source.clone()).await?;
    let json = executor
        .invoke_and_await_agent(
            &component,
            &caller,
            "consume_forwarded_json",
            data_value!("forward-source", format!("{}?strings=true", server.url)),
        )
        .await?
        .into_typed::<Result<Vec<String>, String>>()?;
    assert_eq!(
        json,
        Ok(vec!["alpha".into(), "βeta".into(), "gamma".into()])
    );
    let bytes = executor
        .invoke_and_await_agent(
            &component,
            &caller,
            "consume_forwarded_bytes",
            data_value!("forward-source", format!("{}?bytes=true", server.url)),
        )
        .await?
        .into_typed::<Result<Vec<u8>, String>>()?;
    assert_eq!(bytes, Ok(vec![0, 255, 17, 128, 3]));
    assert_eq!(server.peer.reads.load(Ordering::SeqCst), 2);
    drop(executor);
    let executor = start(deps, &context).await?;
    // Reconstruct both sides independently; only these two fresh reads may hit HTTP.
    for agent in [&caller, &source] {
        let result = executor
            .invoke_and_await_agent(
                &component,
                agent,
                "consume_json",
                data_value!(server.url.clone(), "now", "catch-up", 1u32),
            )
            .await?
            .into_typed::<Result<Vec<String>, String>>()?;
        assert_eq!(result, Ok(vec![]));
    }
    assert_eq!(server.peer.reads.load(Ordering::SeqCst), 4);
    Ok(())
}

#[test]
#[timeout("120s")]
async fn append_crash_after_remote_commit_reuses_tuple_and_body(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("external_durable_streams")] fixture: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let server = Server::start(true).await?;
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;
    let component = executor
        .component_dep(&context.default_environment_id, fixture)
        .store()
        .await?;
    let agent = agent_id!("ExternalDurableStreams", "append-crash");
    let id = executor.start_agent(&component.id, agent.clone()).await?;
    let pending = {
        let executor = executor.clone();
        let component = component.clone();
        let agent = agent.clone();
        let url = server.url.clone();
        tokio::spawn(async move {
            executor
                .invoke_and_await_agent(
                    &component,
                    &agent,
                    "append_json",
                    data_value!(
                        url,
                        "stable-writer",
                        vec!["[7,3]".to_string(), "9007199254740993".to_string()],
                        true
                    ),
                )
                .await
        })
    };
    server.peer.accepted.notified().await;
    executor.commit_oplog(&id).await?;
    executor.simulated_crash(&id).await?;
    let receipts = pending
        .await??
        .into_typed::<Result<Vec<Option<String>>, String>>()?;
    assert_eq!(receipts, Ok(vec![None, Some("receipt-2".to_string())]));
    {
        let attempts = server.peer.attempts.lock().unwrap();
        let effects = server.peer.effects.lock().unwrap();
        assert_eq!(
            attempts.len(),
            3,
            "one uncertain retry plus the next append"
        );
        assert_eq!(
            effects.len(),
            2,
            "the uncertain retry must not append a second message"
        );
        assert_eq!(attempts[0], attempts[1]);
        assert_eq!(effects[0].body, b"[[7,3]]");
        assert_eq!(effects[1].body, b"[9007199254740993]");
        assert_eq!(effects[0].sequence, "0");
        assert_eq!(effects[1].sequence, "1");
        assert!(!effects[0].close);
        assert!(effects[1].close);
    }
    let history = executor.get_oplog(&id, OplogIndex::INITIAL).await?;
    let mut constructors = Vec::new();
    let mut appends = Vec::new();
    for entry in &history {
        if let PublicOplogEntry::Start(start) = &entry.entry {
            match start.function_name.as_str() {
                "golem::agent::durable-streams::durable-stream-writer::new" => {
                    let request = host_request_from_typed_schema_value(
                        &start.function_name,
                        start.request.clone().expect("constructor descriptor"),
                    )
                    .map_err(anyhow::Error::msg)?;
                    constructors.push(
                        HostRequestDurableStreamWriterNew::try_from(request)
                            .map_err(anyhow::Error::msg)?,
                    );
                }
                "golem::agent::durable-streams::durable-stream-writer::append" => {
                    let request = host_request_from_typed_schema_value(
                        &start.function_name,
                        start.request.clone().expect("compact append request"),
                    )
                    .map_err(anyhow::Error::msg)?;
                    appends.push(
                        HostRequestDurableStreamAppend::try_from(request)
                            .map_err(anyhow::Error::msg)?,
                    );
                }
                _ => {}
            }
        }
    }
    assert_eq!(
        constructors.len(),
        1,
        "replay recreates, not re-journals, the resource"
    );
    assert_eq!(constructors[0].options.url, server.url);
    assert_eq!(constructors[0].options.producer_id, "stable-writer");
    let resource_id = constructors[0]
        .options
        .resource_id(constructors[0].auth.as_ref())
        .map_err(anyhow::Error::msg)?;
    assert_eq!(
        appends.len(),
        2,
        "incomplete repair retains the original Start"
    );
    assert_eq!(appends[0].resource_id, resource_id);
    assert_eq!(appends[1].resource_id, resource_id);
    assert_eq!((appends[0].sequence, appends[1].sequence), (0, 1));
    drop(executor);
    let executor = start(deps, &context).await?;
    // Force reconstruction with a read; no POST from either completed append may repeat.
    executor
        .invoke_and_await_agent(
            &component,
            &agent,
            "consume_json",
            data_value!(server.url.clone(), "now", "catch-up", 1u32),
        )
        .await?;
    assert_eq!(server.peer.attempts.lock().unwrap().len(), 3);
    Ok(())
}

#[test]
#[timeout("120s")]
async fn ordinary_forks_recover_exact_read_prefix_and_pending_batch(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("external_durable_streams")] fixture: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let server = Server::start(false).await?;
    let context = TestContext::new(last_unique_id);
    let executor = crate::fork::start_with_local_resume(deps, &context, false).await?;
    let component = executor
        .component_dep(&context.default_environment_id, fixture)
        .store()
        .await?;
    let agent = agent_id!("ExternalDurableStreams", "fork-reader");
    let id = executor.start_agent(&component.id, agent.clone()).await?;
    let key = IdempotencyKey::fresh();
    let parameters = data_value!(server.url.clone(), "-1", "catch-up", 10u32, 25u64);
    let expected = vec![
        "[7,3]".to_string(),
        "9007199254740993".to_string(),
        r#"{"a":false}"#.to_string(),
    ];
    assert_eq!(
        executor
            .invoke_and_await_agent_with_key(
                &component,
                &agent,
                &key,
                "consume_json_delayed",
                parameters.clone()
            )
            .await?
            .into_typed::<Result<Vec<String>, String>>()?,
        Ok(expected.clone())
    );
    let history = executor.get_oplog(&id, OplogIndex::INITIAL).await?;
    let constructor_start = history
        .iter()
        .find_map(|entry| match &entry.entry {
            PublicOplogEntry::Start(start)
                if start.function_name
                    == "golem::agent::durable-streams::durable-stream-reader::new" =>
            {
                Some(entry.oplog_index)
            }
            _ => None,
        })
        .expect("reader constructor Start");
    let constructor_end = history
        .iter()
        .find_map(|entry| match &entry.entry {
            PublicOplogEntry::End(end) if end.start_index == constructor_start => {
                Some(entry.oplog_index)
            }
            _ => None,
        })
        .expect("reader constructor End");
    let start_index = history
        .iter()
        .find_map(|entry| match &entry.entry {
            PublicOplogEntry::Start(start)
                if start.function_name
                    == "golem::agent::durable-streams::durable-stream-reader::read" =>
            {
                Some(entry.oplog_index)
            }
            _ => None,
        })
        .expect("external read Start");
    let end_index = history
        .iter()
        .find_map(|entry| match &entry.entry {
            PublicOplogEntry::End(end) if end.start_index == start_index => Some(entry.oplog_index),
            _ => None,
        })
        .expect("external read End");
    let buffered_cut = history
        .iter()
        .find_map(|entry| match &entry.entry {
            PublicOplogEntry::Start(start)
                if entry.oplog_index > end_index && start.function_name.contains("wait-for") =>
            {
                Some(entry.oplog_index)
            }
            _ => None,
        })
        .expect("durable timer after first buffered item");
    for (cut, additional_reads) in [
        (constructor_start, 1),
        (constructor_end, 1),
        (start_index, 1),
        (end_index, 0),
        (buffered_cut, 0),
    ] {
        let before = server.peer.reads.load(Ordering::SeqCst);
        let fork = golem_common::phantom_agent_id!(
            "ExternalDurableStreams",
            uuid::Uuid::new_v4(),
            "fork-reader"
        );
        executor.fork_worker(&id, &fork.to_string(), cut).await?;
        let result = executor
            .invoke_and_await_agent_with_key(
                &component,
                &fork,
                &key,
                "consume_json_delayed",
                parameters.clone(),
            )
            .await?
            .into_typed::<Result<Vec<String>, String>>()?;
        assert_eq!(result, Ok(expected.clone()));
        assert_eq!(
            server.peer.reads.load(Ordering::SeqCst),
            before + additional_reads
        );
        assert!(
            server.peer.attempts.lock().unwrap().is_empty(),
            "Golem forks must not create or fork an external stream"
        );
    }
    Ok(())
}

#[test]
#[timeout("120s")]
async fn concurrent_reads_keep_request_identity_in_both_completion_orders(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("external_durable_streams")] fixture: &PrecompiledComponent,
) -> anyhow::Result<()> {
    for hold_a in [true, false] {
        let server = Server::start(false).await?;
        let context = TestContext::new(last_unique_id);
        let executor = start(deps, &context).await?;
        let component = executor
            .component_dep(&context.default_environment_id, fixture)
            .store()
            .await?;
        let agent = agent_id!("ExternalDurableStreams", "concurrent-reads");
        let id = executor.start_agent(&component.id, agent.clone()).await?;
        let pending = {
            let executor = executor.clone();
            let component = component.clone();
            let agent = agent.clone();
            let url_a = format!("{}?label=a&hold={hold_a}", server.url);
            let url_b = format!("{}?label=b&hold={}", server.url, !hold_a);
            tokio::spawn(async move {
                executor
                    .invoke_and_await_agent(
                        &component,
                        &agent,
                        "concurrent_reads",
                        data_value!(url_a, url_b),
                    )
                    .await
            })
        };
        server.peer.accepted.notified().await;
        // Release the slower response only after the other completion reached the guest.
        loop {
            executor.commit_oplog(&id).await?;
            let history = executor.get_oplog(&id, OplogIndex::INITIAL).await?;
            let read_starts: Vec<_> = history
                .iter()
                .filter_map(|entry| match &entry.entry {
                    PublicOplogEntry::Start(start)
                        if start.function_name
                            == "golem::agent::durable-streams::durable-stream-reader::read" =>
                    {
                        Some(entry.oplog_index)
                    }
                    _ => None,
                })
                .collect();
            if history.iter().any(|entry| match &entry.entry {
                PublicOplogEntry::CompletionDelivered(delivery) => {
                    read_starts.contains(&delivery.start_index)
                }
                _ => false,
            }) {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        server.peer.release.notify_one();
        assert_eq!(
            pending
                .await??
                .into_typed::<Result<Vec<Vec<String>>, String>>()?,
            Ok(vec![
                vec!["\"branch-a\"".to_string()],
                vec!["\"branch-b\"".to_string()]
            ])
        );
        assert_eq!(server.peer.reads.load(Ordering::SeqCst), 2);
        drop(executor);
        let executor = start(deps, &context).await?;
        executor
            .invoke_and_await_agent(
                &component,
                &agent,
                "consume_json",
                data_value!(server.url.clone(), "now", "catch-up", 1u32),
            )
            .await?;
        assert_eq!(
            server.peer.reads.load(Ordering::SeqCst),
            3,
            "completed concurrent replay must not reconnect either reader"
        );
    }
    Ok(())
}

#[test]
#[timeout("120s")]
async fn cancelled_append_retains_exact_pending_body(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("external_durable_streams")] fixture: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let server = Server::start(true).await?;
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;
    let component = executor
        .component_dep(&context.default_environment_id, fixture)
        .store()
        .await?;
    let agent = agent_id!("ExternalDurableStreams", "cancel-append");
    executor.start_agent(&component.id, agent.clone()).await?;
    let result = executor
        .invoke_and_await_agent(
            &component,
            &agent,
            "cancelled_append",
            data_value!(server.url.clone(), "cancel-writer", 500u64),
        )
        .await?
        .into_typed::<Result<Vec<Option<String>>, String>>()?;
    assert_eq!(
        result,
        Ok(vec![
            Some("cancelled".into()),
            Some("DurableStreamErrorKind::SequenceConflict".into()),
            None,
            Some("receipt-2".into())
        ])
    );
    let attempts = server.peer.attempts.lock().unwrap();
    assert_eq!(attempts.len(), 3);
    assert_eq!(attempts[0], attempts[1]);
    assert_eq!(attempts[0].body, br#"["original"]"#);
    assert_eq!(attempts[2].body, br#"["different"]"#);
    assert_eq!(attempts[2].sequence, "1");
    assert_eq!(server.peer.effects.lock().unwrap().len(), 2);
    Ok(())
}

#[test]
#[timeout("120s")]
async fn authentication_stays_on_wire_and_completed_replay_skips_secret_fetch(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("external_durable_streams")] fixture: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let server = Server::start(false).await?;
    let context = TestContext::new(last_unique_id);
    let secrets = Arc::new(TestEnvironmentStateService::default());
    secrets.set_agent_secret(AgentSecret {
        id: AgentSecretId::new(),
        environment_id: context.default_environment_id,
        path: CanonicalAgentSecretPath(vec!["bearer".into()]),
        revision: AgentSecretRevision::INITIAL,
        secret_type: SchemaGraph::anonymous(SchemaType::string()),
        secret_value: Some(SchemaValue::String("test-ds-secret".into())),
    });
    let overrides = TestExecutorOverrides {
        environment_state_service: Some(secrets.clone()),
        ..Default::default()
    };
    let executor = start_with_overrides(deps, &context, overrides.clone()).await?;
    let component = executor
        .component_dep(&context.default_environment_id, fixture)
        .store()
        .await?;
    let agent = agent_id!("AuthenticatedDurableStreams", "auth");
    let id = executor.start_agent(&component.id, agent.clone()).await?;
    for method in ["read", "append"] {
        let result = executor
            .invoke_and_await_agent(&component, &agent, method, data_value!(server.url.clone()))
            .await?;
        if method == "read" {
            assert_eq!(
                result.into_typed::<Result<Vec<String>, String>>()?,
                Ok(vec![
                    "[7,3]".into(),
                    "9007199254740993".into(),
                    r#"{"a":false}"#.into()
                ])
            );
        } else {
            assert_eq!(
                result.into_typed::<Result<Option<String>, String>>()?,
                Ok(Some("receipt-1".into()))
            );
        }
    }
    assert_eq!(secrets.agent_secret_revision_calls(), 2);
    assert_eq!(server.peer.authenticated_requests.load(Ordering::SeqCst), 2);
    let history = executor.get_oplog(&id, OplogIndex::INITIAL).await?;
    assert!(!serde_json::to_string(&history)?.contains("test-ds-secret"));
    drop(executor);
    let executor = start_with_overrides(deps, &context, overrides).await?;
    executor
        .invoke_and_await_agent(&component, &agent, "read", data_value!(server.url.clone()))
        .await?;
    assert_eq!(
        secrets.agent_secret_revision_calls(),
        3,
        "only the fresh read may fetch a secret"
    );
    assert_eq!(server.peer.authenticated_requests.load(Ordering::SeqCst), 3);
    assert_eq!(server.peer.attempts.lock().unwrap().len(), 1);
    Ok(())
}

#[test]
#[timeout("120s")]
async fn incomplete_append_respects_disabled_idempotence(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("external_durable_streams")] fixture: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let server = Server::start(true).await?;
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;
    let component = executor
        .component_dep(&context.default_environment_id, fixture)
        .store()
        .await?;
    let agent = agent_id!("ExternalDurableStreams", "non-idempotent");
    let id = executor.start_agent(&component.id, agent.clone()).await?;
    let pending = {
        let executor = executor.clone();
        let component = component.clone();
        let agent = agent.clone();
        let url = server.url.clone();
        tokio::spawn(async move {
            executor
                .invoke_and_await_agent(
                    &component,
                    &agent,
                    "append_non_idempotent",
                    data_value!(url, "non-idempotent-writer"),
                )
                .await
        })
    };
    server.peer.accepted.notified().await;
    executor.commit_oplog(&id).await?;
    executor.simulated_crash(&id).await?;
    assert!(
        pending.await?.is_err(),
        "incomplete WriteRemote must fail closed with idempotence disabled"
    );
    assert_eq!(server.peer.attempts.lock().unwrap().len(), 1);
    assert_eq!(server.peer.effects.lock().unwrap().len(), 1);
    Ok(())
}

#[test]
#[timeout("120s")]
async fn concurrent_appends_keep_identity_in_both_completion_orders(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("external_durable_streams")] fixture: &PrecompiledComponent,
) -> anyhow::Result<()> {
    for hold_a in [true, false] {
        let server = Server::start(false).await?;
        let context = TestContext::new(last_unique_id);
        let executor = start(deps, &context).await?;
        let component = executor
            .component_dep(&context.default_environment_id, fixture)
            .store()
            .await?;
        let agent = agent_id!("ExternalDurableStreams", "concurrent-appends");
        let id = executor.start_agent(&component.id, agent.clone()).await?;
        let pending = {
            let executor = executor.clone();
            let component = component.clone();
            let agent = agent.clone();
            let url_a = format!("{}?hold={hold_a}", server.url);
            let url_b = format!("{}?hold={}", server.url, !hold_a);
            tokio::spawn(async move {
                executor
                    .invoke_and_await_agent(
                        &component,
                        &agent,
                        "concurrent_appends",
                        data_value!(url_a, url_b, "writer-a", "writer-b"),
                    )
                    .await
            })
        };
        server.peer.accepted.notified().await;
        loop {
            executor.commit_oplog(&id).await?;
            let history = executor.get_oplog(&id, OplogIndex::INITIAL).await?;
            let starts: Vec<_> = history
                .iter()
                .filter_map(|entry| match &entry.entry {
                    PublicOplogEntry::Start(start)
                        if start.function_name
                            == "golem::agent::durable-streams::durable-stream-writer::append" =>
                    {
                        Some(entry.oplog_index)
                    }
                    _ => None,
                })
                .collect();
            if history.iter().any(|entry| match &entry.entry {
                PublicOplogEntry::CompletionDelivered(delivery) => {
                    starts.contains(&delivery.start_index)
                }
                _ => false,
            }) {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        server.peer.release.notify_one();
        let result = pending
            .await??
            .into_typed::<Result<Vec<Option<String>>, String>>()?
            .map_err(anyhow::Error::msg)?;
        let expected: Vec<_> = {
            let effects = server.peer.effects.lock().unwrap();
            assert_eq!(effects.len(), 2);
            ["writer-a", "writer-b"]
                .iter()
                .map(|producer| {
                    let index = effects
                        .iter()
                        .position(|effect| &effect.producer == producer)
                        .unwrap();
                    let effect = &effects[index];
                    assert_eq!(effect.sequence, "0");
                    assert!(effect.close);
                    assert_eq!(
                        effect.body,
                        if *producer == "writer-a" {
                            br#"["left"]"#.as_slice()
                        } else {
                            br#"["right"]"#.as_slice()
                        }
                    );
                    Some(format!("receipt-{}", index + 1))
                })
                .collect()
        };
        assert_eq!(result, expected);
        drop(executor);
        let executor = start(deps, &context).await?;
        executor
            .invoke_and_await_agent(
                &component,
                &agent,
                "consume_json",
                data_value!(server.url.clone(), "now", "catch-up", 1u32),
            )
            .await?;
        assert_eq!(
            server.peer.attempts.lock().unwrap().len(),
            2,
            "completed append replay must not POST"
        );
    }
    Ok(())
}
