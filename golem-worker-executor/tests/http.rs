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
use axum::routing::post;
use bytes::Bytes;
use golem_common::model::{AgentStatus, IdempotencyKey};
use golem_common::schema::SchemaValue;
use golem_common::{agent_id, data_value};
use golem_test_framework::dsl::TestDsl;
use golem_worker_executor_test_utils::{
    LastUniqueId, PrecompiledComponent, TestContext, WorkerExecutorTestDependencies, start,
};
use http::HeaderMap;
use pretty_assertions::assert_eq;
use serde_json::json;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use test_r::{inherit_test_dep, test};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::spawn;
use tokio::sync::mpsc;
use tokio::time::{Duration, timeout};
use tracing::Instrument;

inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(LastUniqueId);
inherit_test_dep!(Tracing);
inherit_test_dep!(
    #[tagged_as("http_tests")]
    PrecompiledComponent
);

#[test]
#[tracing::instrument]
async fn http_client(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("http_tests")] http_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await.unwrap();
    let host_http_port = listener.local_addr().unwrap().port();

    let http_server = spawn(
        async move {
            let route = Router::new().route(
                "/",
                post(move |headers: HeaderMap, body: Bytes| async move {
                    let header = headers.get("X-Test").unwrap().to_str().unwrap();
                    let body = String::from_utf8(body.to_vec()).unwrap();
                    format!("response is {header} {body}")
                }),
            );

            axum::serve(listener, route).await.unwrap();
        }
        .in_current_span(),
    );

    let component = executor
        .component_dep(&context.default_environment_id, http_tests)
        .store()
        .await?;
    let mut env = HashMap::new();
    env.insert("PORT".to_string(), host_http_port.to_string());
    env.insert("RUST_BACKTRACE".to_string(), "full".to_string());

    let agent_id = agent_id!("HttpClient");
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), env, Vec::new())
        .await?;
    let rx = executor.capture_output(&worker_id).await?;

    let result = executor
        .invoke_and_await_agent(&component, &agent_id, "run", data_value!())
        .await?;

    executor.check_oplog_is_queryable(&worker_id).await?;

    drop(executor);
    drop(rx);
    http_server.abort();

    assert_eq!(
        result.into_typed::<String>()?,
        "200 response is test-header test-body"
    );
    Ok(())
}

#[test]
#[tracing::instrument]
async fn http_client_using_reqwest(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("http_tests")] http_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let captured_body: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    let captured_body_clone = captured_body.clone();

    let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await.unwrap();
    let host_http_port = listener.local_addr().unwrap().port();

    let http_server = spawn(
        async move {
            let route = Router::new().route(
                "/post-example",
                post(move |headers: HeaderMap, body: Bytes| async move {
                    let header = headers
                        .get("X-Test")
                        .map(|h| h.to_str().unwrap().to_string())
                        .unwrap_or("no X-Test header".to_string());
                    let body = String::from_utf8(body.to_vec()).unwrap();
                    {
                        let mut capture = captured_body_clone.lock().unwrap();
                        *capture = Some(body.clone());
                    }
                    format!(
                        "{{ \"percentage\" : 0.25, \"message\": \"response message {header}\" }}"
                    )
                }),
            );

            axum::serve(listener, route).await.unwrap();
        }
        .in_current_span(),
    );

    let component = executor
        .component_dep(&context.default_environment_id, http_tests)
        .store()
        .await?;
    let mut env = HashMap::new();
    env.insert("PORT".to_string(), host_http_port.to_string());

    let agent_id = agent_id!("HttpClient2");
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), env, Vec::new())
        .await?;

    let result = executor
        .invoke_and_await_agent(&component, &agent_id, "run", data_value!())
        .await?;

    let captured_body = captured_body.lock().unwrap().clone().unwrap();

    executor.check_oplog_is_queryable(&worker_id).await?;

    drop(executor);
    http_server.abort();

    assert_eq!(
        result.into_typed::<String>()?,
        "200 ExampleResponse { percentage: 0.25, message: Some(\"response message Golem\") }"
    );
    assert_eq!(
        captured_body,
        "{\"name\":\"Something\",\"amount\":42,\"comments\":[\"Hello\",\"World\"]}".to_string()
    );
    Ok(())
}

#[test]
#[tracing::instrument]
async fn http_client_using_reqwest_async(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("http_tests")] http_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let captured_body: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    let captured_body_clone = captured_body.clone();

    let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await.unwrap();
    let host_http_port = listener.local_addr().unwrap().port();

    let http_server = spawn(
        async move {
            let route = Router::new().route(
                "/post-example",
                post(move |headers: HeaderMap, body: Bytes| async move {
                    let header = headers
                        .get("X-Test")
                        .map(|h| h.to_str().unwrap().to_string())
                        .unwrap_or("no X-Test header".to_string());
                    let body = String::from_utf8(body.to_vec()).unwrap();
                    {
                        let mut capture = captured_body_clone.lock().unwrap();
                        *capture = Some(body.clone());
                    }
                    format!(
                        "{{ \"percentage\" : 0.25, \"message\": \"response message {header}\" }}"
                    )
                }),
            );

            axum::serve(listener, route).await.unwrap();
        }
        .in_current_span(),
    );

    let component = executor
        .component_dep(&context.default_environment_id, http_tests)
        .store()
        .await?;
    let mut env = HashMap::new();
    env.insert("PORT".to_string(), host_http_port.to_string());

    let agent_id = agent_id!("HttpClient3");
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), env, Vec::new())
        .await?;

    let result = executor
        .invoke_and_await_agent(&component, &agent_id, "run", data_value!())
        .await?;
    let captured_body = captured_body.lock().unwrap().clone().unwrap();

    executor.check_oplog_is_queryable(&worker_id).await?;

    drop(executor);
    http_server.abort();

    assert_eq!(
        result.into_typed::<String>()?,
        "200 ExampleResponse { percentage: 0.25, message: Some(\"response message Golem\") }"
    );
    assert_eq!(
        captured_body,
        "{\"name\":\"Something\",\"amount\":42,\"comments\":[\"Hello\",\"World\"]}".to_string()
    );

    Ok(())
}

#[test]
#[tracing::instrument]
async fn http_client_using_reqwest_async_parallel(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("http_tests")] http_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;
    let captured_body: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let captured_body_clone = captured_body.clone();

    let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await.unwrap();
    let host_http_port = listener.local_addr().unwrap().port();

    let http_server = spawn(
        async move {
            let route = Router::new().route(
                "/post-example",
                post(move |headers: HeaderMap, body: Bytes| async move {
                    let header = headers
                        .get("X-Test")
                        .map(|h| h.to_str().unwrap().to_string())
                        .unwrap_or("no X-Test header".to_string());
                    let body = String::from_utf8(body.to_vec()).unwrap();
                    {
                        let mut capture = captured_body_clone.lock().unwrap();
                        capture.push(body.clone());
                    }
                    format!(
                        "{{ \"percentage\" : 0.25, \"message\": \"response message {header}\" }}"
                    )
                }),
            );

            axum::serve(listener, route).await.unwrap();
        }
        .in_current_span(),
    );

    let component = executor
        .component_dep(&context.default_environment_id, http_tests)
        .store()
        .await?;
    let mut env = HashMap::new();
    env.insert("PORT".to_string(), host_http_port.to_string());

    let agent_id = agent_id!("HttpClient3");
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), env, Vec::new())
        .await?;

    let result = executor
        .invoke_and_await_agent(&component, &agent_id, "run_parallel", data_value!(32u16))
        .await?;
    let mut captured_body = captured_body.lock().unwrap().clone();
    captured_body.sort();

    executor.check_oplog_is_queryable(&worker_id).await?;

    drop(executor);
    http_server.abort();

    let return_value = result.into_return_value().expect("Expected a return value");
    let SchemaValue::List { elements: lst } = &return_value else {
        panic!("Expected List, got {:?}", return_value)
    };
    assert_eq!(lst.len(), 32);
    assert_eq!(
        captured_body,
        vec![
            r#"{"name":"Something","amount":0,"comments":["Hello","World"]}"#.to_string(),
            r#"{"name":"Something","amount":1,"comments":["Hello","World"]}"#.to_string(),
            r#"{"name":"Something","amount":10,"comments":["Hello","World"]}"#.to_string(),
            r#"{"name":"Something","amount":11,"comments":["Hello","World"]}"#.to_string(),
            r#"{"name":"Something","amount":12,"comments":["Hello","World"]}"#.to_string(),
            r#"{"name":"Something","amount":13,"comments":["Hello","World"]}"#.to_string(),
            r#"{"name":"Something","amount":14,"comments":["Hello","World"]}"#.to_string(),
            r#"{"name":"Something","amount":15,"comments":["Hello","World"]}"#.to_string(),
            r#"{"name":"Something","amount":16,"comments":["Hello","World"]}"#.to_string(),
            r#"{"name":"Something","amount":17,"comments":["Hello","World"]}"#.to_string(),
            r#"{"name":"Something","amount":18,"comments":["Hello","World"]}"#.to_string(),
            r#"{"name":"Something","amount":19,"comments":["Hello","World"]}"#.to_string(),
            r#"{"name":"Something","amount":2,"comments":["Hello","World"]}"#.to_string(),
            r#"{"name":"Something","amount":20,"comments":["Hello","World"]}"#.to_string(),
            r#"{"name":"Something","amount":21,"comments":["Hello","World"]}"#.to_string(),
            r#"{"name":"Something","amount":22,"comments":["Hello","World"]}"#.to_string(),
            r#"{"name":"Something","amount":23,"comments":["Hello","World"]}"#.to_string(),
            r#"{"name":"Something","amount":24,"comments":["Hello","World"]}"#.to_string(),
            r#"{"name":"Something","amount":25,"comments":["Hello","World"]}"#.to_string(),
            r#"{"name":"Something","amount":26,"comments":["Hello","World"]}"#.to_string(),
            r#"{"name":"Something","amount":27,"comments":["Hello","World"]}"#.to_string(),
            r#"{"name":"Something","amount":28,"comments":["Hello","World"]}"#.to_string(),
            r#"{"name":"Something","amount":29,"comments":["Hello","World"]}"#.to_string(),
            r#"{"name":"Something","amount":3,"comments":["Hello","World"]}"#.to_string(),
            r#"{"name":"Something","amount":30,"comments":["Hello","World"]}"#.to_string(),
            r#"{"name":"Something","amount":31,"comments":["Hello","World"]}"#.to_string(),
            r#"{"name":"Something","amount":4,"comments":["Hello","World"]}"#.to_string(),
            r#"{"name":"Something","amount":5,"comments":["Hello","World"]}"#.to_string(),
            r#"{"name":"Something","amount":6,"comments":["Hello","World"]}"#.to_string(),
            r#"{"name":"Something","amount":7,"comments":["Hello","World"]}"#.to_string(),
            r#"{"name":"Something","amount":8,"comments":["Hello","World"]}"#.to_string(),
            r#"{"name":"Something","amount":9,"comments":["Hello","World"]}"#.to_string(),
        ]
    );

    Ok(())
}

/// Regression test for G35/T48: concurrent HTTP sends interleave their durable
/// records in the oplog in network/scheduling order, so an executor restart
/// must replay them claim-based rather than positionally. The server holds
/// every response of the first invocation until all 16 requests have arrived
/// (forcing all 16 sends to overlap), then releases the responses in reverse
/// request-id order (forcing completions out of initiation order). The durable
/// record must show the overlap (every send `Start` precedes every send `End`)
/// and the inversion (`End` order differs from `Start` order). A restart then
/// forces a full oplog replay of the interleaved records before a fresh
/// invocation runs live.
#[test]
#[tracing::instrument]
async fn http_client_using_reqwest_async_parallel_replay(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("http_tests")] http_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    use golem_common::model::oplog::{OplogIndex, PublicOplogEntry};
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use tokio::sync::oneshot;

    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;
    let captured_body: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let captured_body_clone = captured_body.clone();

    // The first 16 requests (the first invocation) are held: each handler
    // registers a release channel keyed by the guest-assigned `X-Test` request
    // id (0..16) and answers only when the test releases it, so the release
    // order is tied to request identity rather than network arrival order.
    // Later requests (the post-restart invocation) respond immediately, as
    // does everything once `holding` is cleared (the releaser gave up), so a
    // straggler cannot block forever after a release timeout.
    let held: Arc<Mutex<std::collections::HashMap<u16, oneshot::Sender<()>>>> =
        Arc::new(Mutex::new(std::collections::HashMap::new()));
    let held_in_server = held.clone();
    let arrivals = Arc::new(AtomicUsize::new(0));
    let arrivals_in_server = arrivals.clone();
    let holding = Arc::new(AtomicBool::new(true));
    let holding_in_server = holding.clone();

    let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await.unwrap();
    let host_http_port = listener.local_addr().unwrap().port();

    let http_server = spawn(
        async move {
            let route = Router::new().route(
                "/post-example",
                post(move |headers: HeaderMap, body: Bytes| {
                    let held = held_in_server.clone();
                    let arrivals = arrivals_in_server.clone();
                    let holding = holding_in_server.clone();
                    let captured_body = captured_body_clone.clone();
                    async move {
                        let header = headers
                            .get("X-Test")
                            .map(|h| h.to_str().unwrap().to_string())
                            .unwrap_or("no X-Test header".to_string());
                        let body = String::from_utf8(body.to_vec()).unwrap();
                        {
                            let mut capture = captured_body.lock().unwrap();
                            capture.push(body.clone());
                        }
                        if arrivals.fetch_add(1, Ordering::SeqCst) < 16
                            && holding.load(Ordering::SeqCst)
                        {
                            let id: u16 = header
                                .parse()
                                .expect("the first invocation's requests carry numeric X-Test ids");
                            let (release_tx, release_rx) = oneshot::channel();
                            let previous = held.lock().unwrap().insert(id, release_tx);
                            assert!(
                                previous.is_none(),
                                "request id {id} must arrive exactly once while responses are held"
                            );
                            let _ = release_rx.await;
                        }
                        format!(
                            "{{ \"percentage\" : 0.25, \"message\": \"response message {header}\" }}"
                        )
                    }
                }),
            );

            axum::serve(listener, route).await.unwrap();
        }
        .in_current_span(),
    );

    // Releases the held responses once all 16 concurrent requests (ids 0..16)
    // are in flight, in reverse request-id order, pacing the releases so the
    // completions land in the oplog out of initiation order. On timeout it
    // first stops the holding and drops every held channel so the blocked
    // handlers answer and the invocation cannot hang the test.
    let held_in_releaser = held.clone();
    let holding_in_releaser = holding.clone();
    let response_releaser = spawn(
        async move {
            let all_arrived = timeout(Duration::from_secs(30), async {
                loop {
                    if (0..16u16).all(|id| held_in_releaser.lock().unwrap().contains_key(&id)) {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            })
            .await;
            if all_arrived.is_err() {
                holding_in_releaser.store(false, Ordering::SeqCst);
                held_in_releaser.lock().unwrap().clear();
                panic!(
                    "all 16 concurrent requests (distinct X-Test ids 0..16) must arrive while \
                     every response is held"
                );
            }
            for id in (0..16u16).rev() {
                let release_tx = held_in_releaser
                    .lock()
                    .unwrap()
                    .remove(&id)
                    .expect("a held release channel exists for every request id");
                let _ = release_tx.send(());
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        }
        .in_current_span(),
    );

    let component = executor
        .component_dep(&context.default_environment_id, http_tests)
        .store()
        .await?;
    let mut env = HashMap::new();
    env.insert("PORT".to_string(), host_http_port.to_string());

    let agent_id = agent_id!("HttpClient3");
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), env, Vec::new())
        .await?;

    let result = timeout(
        Duration::from_secs(120),
        executor.invoke_and_await_agent(&component, &agent_id, "run_parallel", data_value!(16u16)),
    )
    .await
    .expect("the first parallel invocation must complete once the held responses are released")?;
    let return_value = result.into_return_value().expect("Expected a return value");
    let SchemaValue::List { elements: lst } = &return_value else {
        panic!("Expected List, got {return_value:?}")
    };
    assert_eq!(lst.len(), 16);
    assert_eq!(captured_body.lock().unwrap().len(), 16);
    response_releaser.await?;

    // The durable record must prove both the overlap and the out-of-order
    // completion the server enforced: all 16 send `Start` entries precede
    // every send `End` entry, and the `End` order is not the `Start` order.
    {
        let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
        let sends = partition_starts(&oplog, "http::client::send");
        assert_eq!(
            sends.counts(),
            (0, 16, 0),
            "all 16 concurrent sends must complete with an End: {sends:?}"
        );
        let send_starts: std::collections::HashSet<_> = sends.ended.iter().copied().collect();
        let max_start_index = *sends.ended.iter().max().unwrap();
        let ends_in_oplog_order: Vec<_> = oplog
            .iter()
            .filter_map(|e| match &e.entry {
                PublicOplogEntry::End(p) if send_starts.contains(&p.start_index) => {
                    Some((e.oplog_index, p.start_index))
                }
                _ => None,
            })
            .collect();
        let min_end_entry_index = ends_in_oplog_order
            .iter()
            .map(|(idx, _)| *idx)
            .min()
            .unwrap();
        assert!(
            min_end_entry_index > max_start_index,
            "all 16 sends must overlap: every send Start (last at {max_start_index}) must \
             precede every send End (first at {min_end_entry_index})"
        );
        let ends_by_start: Vec<_> = ends_in_oplog_order
            .iter()
            .map(|(_, start_index)| *start_index)
            .collect();
        assert!(
            !ends_by_start.is_sorted(),
            "the reverse-order releases must complete the sends out of initiation order, but \
             the End entries follow the Start order exactly: {ends_by_start:?}"
        );
    }

    executor.check_oplog_is_queryable(&worker_id).await?;

    drop(executor);
    let executor = start(deps, &context).await?;

    // The fresh invocation first forces a full replay of the previous
    // invocation's interleaved concurrent-send records, then runs live.
    let result2 = executor
        .invoke_and_await_agent(&component, &agent_id, "run_parallel", data_value!(16u16))
        .await?;
    let return_value2 = result2
        .into_return_value()
        .expect("Expected a return value");
    let SchemaValue::List { elements: lst2 } = &return_value2 else {
        panic!("Expected List, got {return_value2:?}")
    };
    assert_eq!(lst2.len(), 16);
    // The replayed sends must be served from the oplog, not re-issued: only
    // the fresh invocation's 16 requests reach the server.
    assert_eq!(captured_body.lock().unwrap().len(), 32);

    executor.check_oplog_is_queryable(&worker_id).await?;

    drop(executor);
    http_server.abort();

    Ok(())
}

#[test]
#[tracing::instrument]
async fn outgoing_http_contains_trace_context_headers(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("http_tests")] http_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await.unwrap();
    let host_http_port = listener.local_addr().unwrap().port();

    let traceparents = Arc::new(Mutex::new(Vec::new()));
    let traceparents_clone = traceparents.clone();

    let http_server = spawn(
        async move {
            let route = Router::new().route(
                "/post-example",
                post(move |headers: HeaderMap| async move {
                    traceparents_clone.lock().unwrap().push(
                        headers
                            .get("traceparent")
                            .map(|h| h.to_str().unwrap().to_string()),
                    );
                    json!({
                        "percentage": 0.0,
                        "message": null
                    })
                    .to_string()
                }),
            );

            axum::serve(listener, route).await.unwrap();
        }
        .in_current_span(),
    );

    let component = executor
        .component_dep(&context.default_environment_id, http_tests)
        .store()
        .await?;
    let mut env = HashMap::new();
    env.insert("PORT".to_string(), host_http_port.to_string());

    let agent_id = agent_id!("HttpClient2");
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), env, Vec::new())
        .await?;

    executor
        .invoke_and_await_agent(&component, &agent_id, "run", data_value!())
        .await?;

    executor.check_oplog_is_queryable(&worker_id).await?;

    // Restart the executor to force a full oplog replay, then run a fresh invocation: the replay
    // must not re-send the recorded request, and the new live request must again carry a
    // trace-context header.
    drop(executor);
    let executor = start(deps, &context).await?;

    executor
        .invoke_and_await_agent(&component, &agent_id, "run", data_value!())
        .await?;

    executor.check_oplog_is_queryable(&worker_id).await?;

    drop(executor);
    http_server.abort();

    let traceparents = traceparents.lock().unwrap();
    assert_eq!(traceparents.len(), 2);
    for traceparent in traceparents.iter() {
        let traceparent = traceparent
            .as_ref()
            .expect("the outgoing p3 HTTP request must carry a traceparent header");
        // W3C trace context: version-traceid-spanid-flags
        let parts: Vec<&str> = traceparent.split('-').collect();
        assert_eq!(
            parts.len(),
            4,
            "traceparent header is not in W3C format: {traceparent}"
        );
        assert_eq!(parts[1].len(), 32);
        assert_eq!(parts[2].len(), 16);
    }

    Ok(())
}

/// A response created by a P3 `client::send` and dropped without consuming its
/// body finishes its `outgoing-http-request` span through a deferred drop
/// event; the `FinishSpan` entry it records must replay symmetrically. The
/// restart + re-invocation would fail with an unexpected-oplog-entry error if
/// the replay-side drain did not consume the recorded entry at the same point.
#[test]
#[tracing::instrument]
async fn outgoing_http_response_dropped_without_consuming_body_replays(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("http_tests")] http_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await.unwrap();
    let host_http_port = listener.local_addr().unwrap().port();

    let http_server = spawn(
        async move {
            let route = Router::new().route("/", axum::routing::get(|| async { "hello" }));
            axum::serve(listener, route).await.unwrap();
        }
        .in_current_span(),
    );

    let component = executor
        .component_dep(&context.default_environment_id, http_tests)
        .store()
        .await?;
    let mut env = HashMap::new();
    env.insert("PORT".to_string(), host_http_port.to_string());

    let agent_id = agent_id!("HttpClient4");
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), env, Vec::new())
        .await?;

    let result = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "get_and_drop_response",
            data_value!(),
        )
        .await?
        .into_typed::<String>()?;
    assert_eq!(result, "200");

    executor.check_oplog_is_queryable(&worker_id).await?;

    // Restart and invoke again: the first invocation (send + unconsumed drop +
    // deferred span finish) replays fully before the second runs live.
    drop(executor);
    let executor = start(deps, &context).await?;

    let result2 = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "get_and_drop_response",
            data_value!(),
        )
        .await?
        .into_typed::<String>()?;
    assert_eq!(result2, "200");

    executor.check_oplog_is_queryable(&worker_id).await?;

    drop(executor);
    http_server.abort();

    Ok(())
}

async fn read_request_headers(stream: &mut tokio::net::TcpStream) -> anyhow::Result<Vec<u8>> {
    let mut request = Vec::new();
    let mut buffer = [0u8; 256];
    loop {
        let read = stream.read(&mut buffer).await?;
        if read == 0 {
            break;
        }
        request.extend_from_slice(&buffer[..read]);
        if request.windows(4).any(|window| window == b"\r\n\r\n") {
            break;
        }
    }
    Ok(request)
}

async fn wait_for_peer_close(stream: &mut tokio::net::TcpStream) -> anyhow::Result<bool> {
    let mut byte = [0u8; 1];
    let closed = timeout(Duration::from_secs(5), stream.read(&mut byte)).await?? == 0;
    Ok(closed)
}

/// Like [`wait_for_peer_close`], but tolerates the peer still writing data
/// (e.g. a streamed request body) before aborting: reads and discards bytes
/// until the peer closes the connection or the deadline expires.
async fn wait_for_peer_close_draining_data(
    stream: &mut tokio::net::TcpStream,
) -> anyhow::Result<bool> {
    let mut buffer = [0u8; 4096];
    let closed = timeout(Duration::from_secs(5), async {
        loop {
            if stream.read(&mut buffer).await? == 0 {
                break Ok::<bool, std::io::Error>(true);
            }
        }
    })
    .await??;
    Ok(closed)
}

async fn recv_close_event(
    rx: &mut mpsc::UnboundedReceiver<anyhow::Result<bool>>,
) -> anyhow::Result<bool> {
    timeout(Duration::from_secs(10), rx.recv())
        .await?
        .transpose()
        .map(|closed| closed.unwrap_or(false))
}

/// Partitions every durable `Start` entry with the given public function name
/// by its terminal in the oplog: terminated by `Cancelled`, terminated by
/// `End`, or left without a terminal (`incomplete` — the durable shape of an
/// idempotent call dropped mid-flight, re-executed live on replay).
struct PartitionedStarts {
    cancelled: Vec<golem_common::model::oplog::OplogIndex>,
    ended: Vec<golem_common::model::oplog::OplogIndex>,
    incomplete: Vec<golem_common::model::oplog::OplogIndex>,
}

impl PartitionedStarts {
    fn counts(&self) -> (usize, usize, usize) {
        (
            self.cancelled.len(),
            self.ended.len(),
            self.incomplete.len(),
        )
    }
}

impl std::fmt::Debug for PartitionedStarts {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PartitionedStarts")
            .field("cancelled", &self.cancelled)
            .field("ended", &self.ended)
            .field("incomplete", &self.incomplete)
            .finish()
    }
}

fn partition_starts(
    oplog: &[golem_common::model::oplog::PublicOplogEntryWithIndex],
    function_name: &str,
) -> PartitionedStarts {
    use golem_common::model::oplog::PublicOplogEntry;

    let mut partitioned = PartitionedStarts {
        cancelled: Vec::new(),
        ended: Vec::new(),
        incomplete: Vec::new(),
    };
    for entry in oplog {
        let PublicOplogEntry::Start(params) = &entry.entry else {
            continue;
        };
        if params.function_name != function_name {
            continue;
        }
        let start_index = entry.oplog_index;
        let terminals: Vec<bool> = oplog
            .iter()
            .filter_map(|e| match &e.entry {
                PublicOplogEntry::End(p) if p.start_index == start_index => Some(true),
                PublicOplogEntry::Cancelled(p) if p.start_index == start_index => Some(false),
                _ => None,
            })
            .collect();
        match terminals.as_slice() {
            [] => partitioned.incomplete.push(start_index),
            [true] => partitioned.ended.push(start_index),
            [false] => partitioned.cancelled.push(start_index),
            multiple => panic!(
                "the durable Start at {start_index} has multiple terminals ({multiple:?}); \
                 every Start must have at most one End or Cancelled"
            ),
        }
    }
    partitioned
}

/// Asserts the durable record of `expected_reads` guest-cancelled pending body
/// reads (each a full `get_and_cancel_pending_body_read` run): the send and the
/// `consume-body` parent complete with `End`, and each pending chunk read is
/// terminated by an `End` persisting the `Cancelled` chunk marker — no orphaned
/// `Start` anywhere.
fn assert_cancelled_body_read_entries(
    oplog: &[golem_common::model::oplog::PublicOplogEntryWithIndex],
    expected_reads: usize,
) {
    use golem_common::model::oplog::payload::types::SerializableP3HttpBodyChunk;
    use golem_common::model::oplog::{
        HostResponse, HostResponseP3HttpClientConsumeBodyChunk, PublicOplogEntry,
    };

    let sends = partition_starts(oplog, "http::client::send");
    assert_eq!(
        sends.counts(),
        (0, expected_reads, 0),
        "each send must complete with an End: {sends:?}"
    );
    let bodies = partition_starts(oplog, "http::types::response::consume-body");
    assert_eq!(
        bodies.counts(),
        (0, expected_reads, 0),
        "each consume-body parent must complete with an End: {bodies:?}"
    );
    let chunks = partition_starts(oplog, "http::types::response::consume-body-chunk");
    assert_eq!(
        chunks.counts(),
        (0, expected_reads, 0),
        "each guest-cancelled chunk read must complete with an End carrying the Cancelled \
         marker: {chunks:?}"
    );
    let chunk_ended = chunks.ended;

    let expected_marker: HostResponse = HostResponseP3HttpClientConsumeBodyChunk {
        chunk: SerializableP3HttpBodyChunk::Cancelled,
    }
    .into();
    let expected_marker = expected_marker
        .into_typed_schema_value()
        .expect("rendering the expected Cancelled chunk marker failed");
    for start_index in chunk_ended {
        let response = oplog
            .iter()
            .find_map(|e| match &e.entry {
                PublicOplogEntry::End(p) if p.start_index == start_index => p.response.clone(),
                _ => None,
            })
            .unwrap_or_else(|| panic!("missing End response for the chunk read at {start_index}"));
        assert_eq!(
            response, expected_marker,
            "the guest-cancelled chunk read at {start_index} must persist the Cancelled chunk \
             marker as its recorded terminal"
        );
    }
}

async fn recv_request_event(
    rx: &mut mpsc::UnboundedReceiver<anyhow::Result<()>>,
) -> anyhow::Result<()> {
    timeout(Duration::from_secs(10), rx.recv())
        .await?
        .transpose()?
        .unwrap_or(());
    Ok(())
}

fn assert_partial_body_drop_result(result: &str) {
    let len = result
        .strip_prefix("200 first-chunk=")
        .and_then(|len| len.parse::<usize>().ok())
        .expect("partial body drop result must report a 200 status and first chunk length");
    assert!(len > 0, "partial body drop must read a non-empty chunk");
}

#[test]
#[tracing::instrument]
async fn outgoing_http_pending_body_read_can_be_cancelled(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("http_tests")] http_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await.unwrap();
    let host_http_port = listener.local_addr().unwrap().port();
    let (request_tx, mut request_rx) = mpsc::unbounded_channel();

    let http_server = spawn(
        async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                let request_tx = request_tx.clone();
                spawn(async move {
                    let result = async {
                        let request = read_request_headers(&mut stream).await?;
                        anyhow::ensure!(
                            request.starts_with(b"GET /stalled-body "),
                            "unexpected request: {}",
                            String::from_utf8_lossy(&request)
                        );
                        stream
                            .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 1048576\r\n\r\n")
                            .await?;
                        stream.flush().await?;
                        let _ = request_tx.send(Ok(()));
                        futures::future::pending::<()>().await;
                        #[allow(unreachable_code)]
                        Ok(())
                    }
                    .await;
                    if let Err(error) = result {
                        let _ = request_tx.send(Err(error));
                    }
                });
            }
        }
        .in_current_span(),
    );

    let component = executor
        .component_dep(&context.default_environment_id, http_tests)
        .store()
        .await?;
    let mut env = HashMap::new();
    env.insert("PORT".to_string(), host_http_port.to_string());

    let agent_id = agent_id!("HttpClient4");
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), env, Vec::new())
        .await?;

    let result = timeout(
        Duration::from_secs(10),
        executor.invoke_and_await_agent(
            &component,
            &agent_id,
            "get_and_cancel_pending_body_read",
            data_value!(),
        ),
    )
    .await??
    .into_typed::<String>()?;

    assert_eq!(result, "cancelled-during-body-read(200)");
    recv_request_event(&mut request_rx).await?;
    executor.check_oplog_is_queryable(&worker_id).await?;

    drop(executor);
    http_server.abort();

    Ok(())
}

#[test]
#[tracing::instrument]
async fn outgoing_http_pending_body_read_cancellation_replays(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("http_tests")] http_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await.unwrap();
    let host_http_port = listener.local_addr().unwrap().port();
    let (request_tx, mut request_rx) = mpsc::unbounded_channel();

    let http_server = spawn(
        async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                let request_tx = request_tx.clone();
                spawn(async move {
                    let result = async {
                        let request = read_request_headers(&mut stream).await?;
                        anyhow::ensure!(
                            request.starts_with(b"GET /stalled-body "),
                            "unexpected request: {}",
                            String::from_utf8_lossy(&request)
                        );
                        stream
                            .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 1048576\r\n\r\n")
                            .await?;
                        stream.flush().await?;
                        let _ = request_tx.send(Ok(()));
                        futures::future::pending::<()>().await;
                        #[allow(unreachable_code)]
                        Ok(())
                    }
                    .await;
                    if let Err(error) = result {
                        let _ = request_tx.send(Err(error));
                    }
                });
            }
        }
        .in_current_span(),
    );

    let component = executor
        .component_dep(&context.default_environment_id, http_tests)
        .store()
        .await?;
    let mut env = HashMap::new();
    env.insert("PORT".to_string(), host_http_port.to_string());

    let agent_id = agent_id!("HttpClient4");
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), env, Vec::new())
        .await?;

    let result = timeout(
        Duration::from_secs(10),
        executor.invoke_and_await_agent(
            &component,
            &agent_id,
            "get_and_cancel_pending_body_read",
            data_value!(),
        ),
    )
    .await??
    .into_typed::<String>()?;
    assert_eq!(result, "cancelled-during-body-read(200)");
    recv_request_event(&mut request_rx).await?;

    // The cancelled pending chunk read must be terminated durably: an `End`
    // persisting the `Cancelled` chunk marker, with the send and consume-body
    // parent completed normally and no orphaned `Start`.
    {
        use golem_common::model::oplog::OplogIndex;
        let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
        assert_cancelled_body_read_entries(&oplog, 1);
    }

    executor.check_oplog_is_queryable(&worker_id).await?;

    drop(executor);
    let executor = start(deps, &context).await?;

    let result2 = timeout(
        Duration::from_secs(30),
        executor.invoke_and_await_agent(
            &component,
            &agent_id,
            "get_and_cancel_pending_body_read",
            data_value!(),
        ),
    )
    .await??
    .into_typed::<String>()?;
    assert_eq!(result2, "cancelled-during-body-read(200)");
    recv_request_event(&mut request_rx).await?;

    // The first cancelled read replayed positionally and the second ran live:
    // both must be recorded with the same shape.
    {
        use golem_common::model::oplog::OplogIndex;
        let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
        assert_cancelled_body_read_entries(&oplog, 2);
    }

    executor.check_oplog_is_queryable(&worker_id).await?;

    drop(executor);
    http_server.abort();

    Ok(())
}

/// Dropping a still-pending P3 response future of an idempotent (GET) send
/// must abort the underlying HTTP request instead of leaving the socket
/// parked waiting for response headers, and leave the committed `Start`
/// without a terminal (`LeaveIncompleteOnDrop`): the restarted invocation
/// re-executes the incomplete send live and the deterministic guest cancels
/// it again, then a fresh cancellation runs live.
#[test]
#[tracing::instrument]
async fn outgoing_http_response_future_cancel_aborts_request_and_replays(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("http_tests")] http_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    use std::sync::atomic::{AtomicUsize, Ordering};

    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await.unwrap();
    let host_http_port = listener.local_addr().unwrap().port();
    let (closed_tx, mut closed_rx) = mpsc::unbounded_channel();
    let accepted = Arc::new(AtomicUsize::new(0));
    let accepted_in_server = accepted.clone();

    let http_server = spawn(
        async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                accepted_in_server.fetch_add(1, Ordering::SeqCst);
                let closed_tx = closed_tx.clone();
                spawn(async move {
                    let result = async {
                        let request = read_request_headers(&mut stream).await?;
                        if request.is_empty() {
                            // The peer aborted the connection before sending the
                            // request head (a send dropped mid-connect); already
                            // closed.
                            return Ok(true);
                        }
                        anyhow::ensure!(
                            request.starts_with(b"GET /delayed-response "),
                            "unexpected request: {}",
                            String::from_utf8_lossy(&request)
                        );
                        wait_for_peer_close(&mut stream).await
                    }
                    .await;
                    let _ = closed_tx.send(result);
                });
            }
        }
        .in_current_span(),
    );

    let component = executor
        .component_dep(&context.default_environment_id, http_tests)
        .store()
        .await?;
    let mut env = HashMap::new();
    env.insert("PORT".to_string(), host_http_port.to_string());

    let agent_id = agent_id!("HttpClient4");
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), env, Vec::new())
        .await?;

    let result = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "get_and_cancel_before_response",
            data_value!(),
        )
        .await?
        .into_typed::<String>()?;
    assert_eq!(result, "cancelled-before-response");
    assert!(
        recv_close_event(&mut closed_rx).await?,
        "dropping the pending response future must close the in-flight request connection"
    );
    assert_eq!(
        accepted.load(Ordering::SeqCst),
        1,
        "the first invocation must issue exactly one live request"
    );

    // An idempotent (GET) send uses the `LeaveIncompleteOnDrop` drop policy:
    // dropping the pending response future leaves its committed
    // `Start`(http::client::send) without a terminal, so replay re-executes
    // the send live and the deterministic guest cancels it again.
    {
        use golem_common::model::oplog::OplogIndex;
        let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
        let sends = partition_starts(&oplog, "http::client::send");
        assert_eq!(
            sends.counts(),
            (0, 0, 1),
            "the dropped pending idempotent send must leave exactly one incomplete Start: \
             {sends:?}"
        );
    }

    executor.check_oplog_is_queryable(&worker_id).await?;

    drop(executor);
    let executor = start(deps, &context).await?;

    let result2 = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "get_and_cancel_before_response",
            data_value!(),
        )
        .await?
        .into_typed::<String>()?;
    assert_eq!(result2, "cancelled-before-response");
    // Post-restart connections: the replayed invocation re-executes the
    // historical incomplete send live and the deterministic guest drops it
    // again, then the fresh invocation issues its own send (also dropped).
    // During replay the recorded cancel timer resolves instantly from the
    // oplog, so the re-executed send may be aborted before or after its TCP
    // connect reaches the server: the fresh invocation's connection is
    // guaranteed, the re-executed one contributes at most one more. Every
    // connection that did land must be aborted at the socket level.
    let post_restart_accepted = accepted.load(Ordering::SeqCst) - 1;
    assert!(
        (1..=2).contains(&post_restart_accepted),
        "post-restart there must be the fresh invocation's connection plus at most one from \
         the re-executed incomplete send, got {post_restart_accepted}"
    );
    for _ in 0..post_restart_accepted {
        assert!(
            recv_close_event(&mut closed_rx).await?,
            "every post-restart request connection must be aborted by the dropped send"
        );
    }

    // The replayed first send re-executed live from its claimed incomplete
    // `Start` and was cancelled again (no new entries); the second live send
    // appended another `Start` that was likewise left incomplete on drop.
    {
        use golem_common::model::oplog::OplogIndex;
        let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
        let sends = partition_starts(&oplog, "http::client::send");
        assert_eq!(
            sends.counts(),
            (0, 0, 2),
            "the replayed and the fresh cancelled idempotent sends must both leave \
             incomplete Starts: {sends:?}"
        );
    }

    executor.check_oplog_is_queryable(&worker_id).await?;

    drop(executor);
    http_server.abort();

    Ok(())
}

/// Dropping a still-pending P3 response future of a non-idempotent (POST)
/// send must record a durable `Start` + `Cancelled` pair (`Cancellable` drop
/// policy) and abort the underlying HTTP request. On replay after restart the
/// recorded `Cancelled` parks the send without touching the network — the
/// POST is never re-issued — and a second cancellation then runs live.
#[test]
#[tracing::instrument]
async fn outgoing_http_post_cancel_records_cancelled_and_replays(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("http_tests")] http_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    use std::sync::atomic::{AtomicUsize, Ordering};

    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await.unwrap();
    let host_http_port = listener.local_addr().unwrap().port();
    let (closed_tx, mut closed_rx) = mpsc::unbounded_channel();
    let connections = Arc::new(AtomicUsize::new(0));
    let connections_server = connections.clone();

    let http_server = spawn(
        async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                connections_server.fetch_add(1, Ordering::SeqCst);
                let closed_tx = closed_tx.clone();
                spawn(async move {
                    let result = async {
                        let request = read_request_headers(&mut stream).await?;
                        anyhow::ensure!(
                            request.starts_with(b"POST /delayed-response "),
                            "unexpected request: {}",
                            String::from_utf8_lossy(&request)
                        );
                        // Never respond; drain any request-body bytes until
                        // the peer aborts the request.
                        wait_for_peer_close_draining_data(&mut stream).await
                    }
                    .await;
                    let _ = closed_tx.send(result);
                });
            }
        }
        .in_current_span(),
    );

    let component = executor
        .component_dep(&context.default_environment_id, http_tests)
        .store()
        .await?;
    let mut env = HashMap::new();
    env.insert("PORT".to_string(), host_http_port.to_string());

    let agent_id = agent_id!("HttpClient4");
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), env, Vec::new())
        .await?;

    let result = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "post_and_cancel_before_response",
            data_value!(),
        )
        .await?
        .into_typed::<String>()?;
    assert_eq!(result, "cancelled-before-response");
    assert!(
        recv_close_event(&mut closed_rx).await?,
        "dropping the pending response future must close the in-flight request connection"
    );
    assert_eq!(connections.load(Ordering::SeqCst), 1);

    // A non-idempotent send uses the `Cancellable` drop policy: dropping the
    // pending response future records a `Cancelled` terminal for the
    // committed `Start`, so replay never re-issues the POST.
    {
        use golem_common::model::oplog::OplogIndex;
        let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
        let sends = partition_starts(&oplog, "http::client::send");
        assert_eq!(
            sends.counts(),
            (1, 0, 0),
            "the dropped pending non-idempotent send must record exactly one Start + \
             Cancelled pair: {sends:?}"
        );
    }

    executor.check_oplog_is_queryable(&worker_id).await?;

    drop(executor);
    let executor = start(deps, &context).await?;

    let result2 = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "post_and_cancel_before_response",
            data_value!(),
        )
        .await?
        .into_typed::<String>()?;
    assert_eq!(result2, "cancelled-before-response");
    assert!(
        recv_close_event(&mut closed_rx).await?,
        "post-restart cancellation must still close the fresh live request connection"
    );

    // The replay of the first invocation resolved its send from the recorded
    // `Cancelled` entry without network I/O, so only the second live
    // invocation opened a new connection.
    assert_eq!(
        connections.load(Ordering::SeqCst),
        2,
        "replaying the cancelled POST send must not re-issue the request"
    );

    // Both the replayed and the fresh cancelled sends are recorded as
    // `Start` + `Cancelled` pairs.
    {
        use golem_common::model::oplog::OplogIndex;
        let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
        let sends = partition_starts(&oplog, "http::client::send");
        assert_eq!(
            sends.counts(),
            (2, 0, 0),
            "the replayed and the fresh cancelled non-idempotent sends must both record \
             Start + Cancelled pairs: {sends:?}"
        );
    }

    executor.check_oplog_is_queryable(&worker_id).await?;

    drop(executor);
    http_server.abort();

    Ok(())
}

/// Interrupting a worker while the guest is parked in a still-pending P3 HTTP
/// response wait must deliver the interrupt promptly: the parked send races
/// the worker's interrupt signal, abandons its durable call handle (leaving
/// the `Start` incomplete for replay) and unwinds the event loop cooperatively
/// with the interrupt, aborting the in-flight request. The caller gets the
/// regular interruption error. After resume, the retained invocation is
/// retried live and completes against the now-responding server.
#[test]
#[tracing::instrument]
async fn interrupt_while_parked_in_p3_http_response_wait(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("http_tests")] http_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await.unwrap();
    let host_http_port = listener.local_addr().unwrap().port();
    let (request_tx, mut request_rx) = mpsc::unbounded_channel();
    let (closed_tx, mut closed_rx) = mpsc::unbounded_channel();

    let http_server = spawn(
        async move {
            let mut first = true;
            while let Ok((mut stream, _)) = listener.accept().await {
                let is_first = first;
                first = false;
                let request_tx = request_tx.clone();
                let closed_tx = closed_tx.clone();
                spawn(async move {
                    if is_first {
                        // Never respond; report when the peer aborts the request.
                        let result = async {
                            let _ = read_request_headers(&mut stream).await?;
                            let _ = request_tx.send(Ok(()));
                            wait_for_peer_close(&mut stream).await
                        }
                        .await;
                        let _ = closed_tx.send(result);
                    } else {
                        let _ = async {
                            let _ = read_request_headers(&mut stream).await?;
                            stream
                                .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\n\r\nhi")
                                .await?;
                            anyhow::Ok(())
                        }
                        .await;
                    }
                });
            }
        }
        .in_current_span(),
    );

    let component = executor
        .component_dep(&context.default_environment_id, http_tests)
        .store()
        .await?;
    let mut env = HashMap::new();
    env.insert("PORT".to_string(), host_http_port.to_string());

    let agent_id = agent_id!("HttpClient4");
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), env, Vec::new())
        .await?;

    let executor_clone = executor.clone();
    let component_clone = component.clone();
    let agent_id_clone = agent_id.clone();
    let fiber = spawn(
        async move {
            executor_clone
                .invoke_and_await_agent(
                    &component_clone,
                    &agent_id_clone,
                    "get_and_read_body_chunked",
                    data_value!(),
                )
                .await
        }
        .in_current_span(),
    );

    // Wait until the guest is parked in the pending P3 response wait: the
    // server has read the request headers but will never respond.
    recv_request_event(&mut request_rx).await?;

    executor.interrupt(&worker_id).await?;

    let result = fiber.await?;
    assert!(result.is_err());
    let err_msg = format!("{}", result.err().unwrap());
    assert!(
        err_msg.contains("Interrupted via the Golem API"),
        "Expected interruption error, got: {err_msg}"
    );

    executor
        .wait_for_status(
            &worker_id,
            AgentStatus::Interrupted,
            Duration::from_secs(10),
        )
        .await?;

    assert!(
        recv_close_event(&mut closed_rx).await?,
        "interrupting the worker must abort the in-flight HTTP request connection"
    );

    // Resuming the worker retries the interrupted invocation live; the server
    // responds this time.
    executor.resume(&worker_id, false).await?;
    executor
        .wait_for_status(&worker_id, AgentStatus::Idle, Duration::from_secs(30))
        .await?;

    let result2 = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "get_and_read_body_chunked",
            data_value!(),
        )
        .await?
        .into_typed::<String>()?;
    assert_eq!(result2, "200 hi");

    executor.check_oplog_is_queryable(&worker_id).await?;

    drop(executor);
    http_server.abort();

    Ok(())
}

/// Dropping the P3 response body stream after a partial read must stop the
/// consume-body task and release the pooled-connection permits instead of
/// keeping the host pinned on unread bytes.
#[test]
#[tracing::instrument]
async fn outgoing_http_body_stream_drop_releases_connection_permit(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("http_tests")] http_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    use golem_worker_executor::services::golem_config::{
        HttpClientConfig, HttpClientEnabledConfig,
    };
    use golem_worker_executor_test_utils::start_with_http_client_config;

    let context = TestContext::new(last_unique_id);
    let executor = start_with_http_client_config(
        deps,
        &context,
        HttpClientConfig::Enabled(HttpClientEnabledConfig {
            max_idle_per_host: 1,
            max_connections_per_host: 1,
            max_total_connections: 1,
            ..Default::default()
        }),
    )
    .await?;

    let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await.unwrap();
    let host_http_port = listener.local_addr().unwrap().port();
    let (request_tx, mut request_rx) = mpsc::unbounded_channel();
    let (release_tx, release_rx) = tokio::sync::watch::channel(false);

    let http_server = spawn(
        async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                let request_tx = request_tx.clone();
                let mut release_rx = release_rx.clone();
                spawn(async move {
                    let result = async {
                        let request = read_request_headers(&mut stream).await?;
                        anyhow::ensure!(
                            request.starts_with(b"GET /slow-body "),
                            "unexpected request: {}",
                            String::from_utf8_lossy(&request)
                        );
                        stream
                            .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 1048576\r\n\r\nhello")
                            .await?;
                        stream.flush().await?;
                        let _ = request_tx.send(Ok(()));
                        let _ = release_rx.changed().await;
                        Ok(())
                    }
                    .await;
                    if let Err(error) = result {
                        let _ = request_tx.send(Err(error));
                    }
                });
            }
        }
        .in_current_span(),
    );

    let component = executor
        .component_dep(&context.default_environment_id, http_tests)
        .store()
        .await?;
    let mut env = HashMap::new();
    env.insert("PORT".to_string(), host_http_port.to_string());

    let agent_id = agent_id!("HttpClient4");
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), env, Vec::new())
        .await?;

    let result = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "get_and_drop_body_after_first_chunk",
            data_value!(),
        )
        .await?
        .into_typed::<String>()?;
    assert_partial_body_drop_result(&result);
    recv_request_event(&mut request_rx).await?;
    let _ = release_tx.send(true);

    let result2_live = timeout(
        Duration::from_secs(10),
        executor.invoke_and_await_agent(
            &component,
            &agent_id,
            "get_and_drop_body_after_first_chunk",
            data_value!(),
        ),
    )
    .await??
    .into_typed::<String>()?;
    assert_partial_body_drop_result(&result2_live);
    recv_request_event(&mut request_rx).await?;

    executor.check_oplog_is_queryable(&worker_id).await?;

    drop(executor);
    let executor = start_with_http_client_config(
        deps,
        &context,
        HttpClientConfig::Enabled(HttpClientEnabledConfig {
            max_idle_per_host: 1,
            max_connections_per_host: 1,
            max_total_connections: 1,
            ..Default::default()
        }),
    )
    .await?;

    let result3_after_restart = timeout(
        Duration::from_secs(10),
        executor.invoke_and_await_agent(
            &component,
            &agent_id,
            "get_and_drop_body_after_first_chunk",
            data_value!(),
        ),
    )
    .await??
    .into_typed::<String>()?;
    assert_partial_body_drop_result(&result3_after_restart);
    recv_request_event(&mut request_rx).await?;

    executor.check_oplog_is_queryable(&worker_id).await?;

    drop(executor);
    http_server.abort();

    Ok(())
}

/// G8 regression: the request-body transmission result of a P3 `client::send`
/// is recorded durably and replays to the guest unchanged. The guest posts a
/// body shorter than its declared `content-length`, which deterministically
/// fails the transmission future with `HttpRequestBodySize`; that error must
/// be observed live (recorded), and the restarted worker must replay the
/// invocation — including the recorded `body-transmission` entries — and see
/// the same error again on a fresh live invocation.
#[test]
#[tracing::instrument]
async fn outgoing_http_request_body_transmission_error_is_recorded_and_replayed(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("http_tests")] http_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await.unwrap();
    let host_http_port = listener.local_addr().unwrap().port();

    let http_server = spawn(
        async move {
            // Respond immediately without reading the request body, so the
            // response head can arrive while the (short) upload is still open.
            let route = Router::new().route("/", post(|| async { "ok" }));
            axum::serve(listener, route).await.unwrap();
        }
        .in_current_span(),
    );

    let component = executor
        .component_dep(&context.default_environment_id, http_tests)
        .store()
        .await?;
    let mut env = HashMap::new();
    env.insert("PORT".to_string(), host_http_port.to_string());

    let agent_id = agent_id!("HttpClient4");
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), env, Vec::new())
        .await?;

    let result = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "post_with_short_body_transmission_error",
            data_value!(),
        )
        .await?
        .into_typed::<String>()?;
    assert!(
        result.contains("transmit=Err(ErrorCode::HttpRequestBodySize(Some(5)))"),
        "the live transmission future must observe the content-length mismatch, got: {result}"
    );

    executor.check_oplog_is_queryable(&worker_id).await?;

    // Restart and invoke again: the first invocation — including the recorded
    // `body-transmission` Start/End — replays fully before the second runs
    // live. Replay fails with an unexpected-oplog-entry error if the
    // transmission entries are not claimed at the same positions, and the
    // replayed guest awaits the transmission future, so it must resolve from
    // the recorded terminal for the replay to complete at all.
    drop(executor);
    let executor = start(deps, &context).await?;

    let result2 = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "post_with_short_body_transmission_error",
            data_value!(),
        )
        .await?
        .into_typed::<String>()?;
    assert!(
        result2.contains("transmit=Err(ErrorCode::HttpRequestBodySize(Some(5)))"),
        "the post-restart transmission future must observe the same recorded error, got: {result2}"
    );

    executor.check_oplog_is_queryable(&worker_id).await?;

    drop(executor);
    http_server.abort();

    Ok(())
}

/// A `client::send` failing with a *permanent* `ErrorCode` (a TLS handshake
/// against a plain-HTTP server) must be recorded durably: the guest observes
/// the error exactly once live (permanent HTTP errors are not retried at the
/// worker level, so the server sees a single connection), the oplog contains a
/// completed `Start`/`End` pair for the send, and after a restart the recorded
/// invocation replays the same `ErrorCode` from the oplog without any new
/// network attempt.
#[test]
#[tracing::instrument]
async fn outgoing_http_send_permanent_error_is_recorded_and_replayed(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("http_tests")] http_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    use golem_common::model::oplog::OplogIndex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await.unwrap();
    let host_http_port = listener.local_addr().unwrap().port();
    let accepted = Arc::new(AtomicUsize::new(0));
    let accepted_in_server = accepted.clone();

    let http_server = spawn(
        async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                accepted_in_server.fetch_add(1, Ordering::SeqCst);
                spawn(async move {
                    // Answer the TLS ClientHello with plain-HTTP bytes: rustls
                    // rejects them as an invalid TLS message, which maps to the
                    // permanent `TlsProtocolError`. Keep the socket open until
                    // the client closes so the guest observes the bogus TLS
                    // bytes rather than a connection reset (which would be
                    // classified as transient and retried).
                    let _ = stream
                        .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 0\r\n\r\n")
                        .await;
                    let _ = stream.flush().await;
                    let _ = wait_for_peer_close(&mut stream).await;
                });
            }
        }
        .in_current_span(),
    );

    let component = executor
        .component_dep(&context.default_environment_id, http_tests)
        .store()
        .await?;
    let mut env = HashMap::new();
    env.insert("PORT".to_string(), host_http_port.to_string());

    let agent_id = agent_id!("HttpClient4");
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), env, Vec::new())
        .await?;

    let result = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "send_with_permanent_error",
            data_value!(),
        )
        .await?
        .into_typed::<String>()?;
    assert_eq!(
        result, "send-error(ErrorCode::TlsProtocolError)",
        "the live send must fail with the exact permanent TLS ErrorCode the bogus TLS bytes \
         are classified as"
    );
    assert_eq!(
        accepted.load(Ordering::SeqCst),
        1,
        "a permanent send error must not be retried at the worker level"
    );

    // The failed send must be durable: a completed `Start`(http::client::send)
    // with a matching `End` carrying the recorded error, and no orphaned or
    // cancelled send `Start`.
    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    let sends = partition_starts(&oplog, "http::client::send");
    assert_eq!(
        sends.counts(),
        (0, 1, 0),
        "the failed send must be recorded as exactly one Start completed by an End — not \
         cancelled, not orphaned: {sends:?}"
    );

    executor.check_oplog_is_queryable(&worker_id).await?;

    // Restart: the recorded invocation (including the failed send) replays
    // from the oplog before the next invocation runs. The stored agent state
    // must be rebuilt to the same error string without any new connection to
    // the server.
    drop(executor);
    let executor = start(deps, &context).await?;

    let stored = executor
        .invoke_and_await_agent(&component, &agent_id, "stored_send_error", data_value!())
        .await?
        .into_typed::<String>()?;
    assert_eq!(
        stored, result,
        "replay must rebuild the same recorded send error"
    );
    assert_eq!(
        accepted.load(Ordering::SeqCst),
        1,
        "replaying the failed send must not hit the network"
    );

    executor.check_oplog_is_queryable(&worker_id).await?;

    drop(executor);
    http_server.abort();

    Ok(())
}

/// Replay roundtrip of a successful send: the guest performs a GET and stores
/// the full response — status, a distinctive response header, and the body —
/// in agent state. After an executor restart, the recorded invocation replays
/// from the oplog and must rebuild the identical status/header/body without
/// any new network request.
#[test]
#[tracing::instrument]
async fn outgoing_http_full_response_is_replayed_without_network(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("http_tests")] http_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    use golem_common::model::oplog::OplogIndex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await.unwrap();
    let host_http_port = listener.local_addr().unwrap().port();
    let requests = Arc::new(AtomicUsize::new(0));
    let requests_in_server = requests.clone();

    let http_server = spawn(
        async move {
            let route = Router::new().route(
                "/full-response",
                axum::routing::get(move || {
                    let requests = requests_in_server.clone();
                    async move {
                        requests.fetch_add(1, Ordering::SeqCst);
                        (
                            [("x-resp-test", "distinctive-header-value")],
                            "full-response-body",
                        )
                    }
                }),
            );
            axum::serve(listener, route).await.unwrap();
        }
        .in_current_span(),
    );

    let component = executor
        .component_dep(&context.default_environment_id, http_tests)
        .store()
        .await?;
    let mut env = HashMap::new();
    env.insert("PORT".to_string(), host_http_port.to_string());

    let agent_id = agent_id!("HttpClient4");
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), env, Vec::new())
        .await?;

    let result = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "get_and_store_full_response",
            data_value!(),
        )
        .await?
        .into_typed::<String>()?;
    assert_eq!(
        result,
        "status=200;x-resp-test=distinctive-header-value;body=full-response-body"
    );
    assert_eq!(requests.load(Ordering::SeqCst), 1);

    // The send and its body consumption must be durable and complete.
    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    let sends = partition_starts(&oplog, "http::client::send");
    assert_eq!(
        sends.counts(),
        (0, 1, 0),
        "the successful send must be recorded as a completed Start/End pair: {sends:?}"
    );

    executor.check_oplog_is_queryable(&worker_id).await?;

    // Restart: replaying the recorded invocation must rebuild the identical
    // stored status/header/body from the oplog without hitting the network.
    drop(executor);
    let executor = start(deps, &context).await?;

    let stored = executor
        .invoke_and_await_agent(&component, &agent_id, "stored_full_response", data_value!())
        .await?
        .into_typed::<String>()?;
    assert_eq!(
        stored, result,
        "replay must rebuild the same recorded status, response header, and body"
    );
    assert_eq!(
        requests.load(Ordering::SeqCst),
        1,
        "replaying the recorded send must not issue a new network request"
    );

    executor.check_oplog_is_queryable(&worker_id).await?;

    drop(executor);
    http_server.abort();

    Ok(())
}

#[test]
#[tracing::instrument]
async fn outgoing_http_contains_idempotency_key(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("http_tests")] http_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await.unwrap();
    let host_http_port = listener.local_addr().unwrap().port();

    let http_server = spawn(
        async move {
            let route = Router::new().route(
                "/post-example",
                post(move |headers: HeaderMap| async move {
                    let idempotency_key = headers
                        .get("idempotency-key")
                        .map(|h| h.to_str().unwrap().to_string());
                    let idempotency_key_str = idempotency_key.map(|i| i.to_string());
                    json!({
                        "percentage": 0.0,
                        "message": idempotency_key_str
                    })
                    .to_string()
                }),
            );

            axum::serve(listener, route).await.unwrap();
        }
        .in_current_span(),
    );

    let component = executor
        .component_dep(&context.default_environment_id, http_tests)
        .store()
        .await?;
    let mut env = HashMap::new();
    env.insert("PORT".to_string(), host_http_port.to_string());

    let agent_id = agent_id!("HttpClient2");
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), env, Vec::new())
        .await?;

    let key = IdempotencyKey::new("177db03d-3234-4a04-8d03-e8d042348abd".to_string());
    let result = executor
        .invoke_and_await_agent_with_key(&component, &agent_id, &key, "run", data_value!())
        .await?;

    executor.check_oplog_is_queryable(&worker_id).await?;

    // The injected key is derived from the invocation idempotency key and the send's own
    // host-call `Start` index, so it is a deterministic value for the first call of this
    // invocation.
    let expected_response = "200 ExampleResponse { percentage: 0.0, message: Some(\"e7158c39-c997-5318-9d0d-a3c47f406e12\") }";
    assert_eq!(result.into_typed::<String>()?, expected_response);

    // Restart the executor to force a full oplog replay and repeat the invocation with the same
    // idempotency key: the replayed invocation must observe the same injected key (recorded in
    // the durable request), without re-sending the HTTP request.
    drop(executor);
    let executor = start(deps, &context).await?;

    let replayed_result = executor
        .invoke_and_await_agent_with_key(&component, &agent_id, &key, "run", data_value!())
        .await?;
    assert_eq!(replayed_result.into_typed::<String>()?, expected_response);

    // A fresh invocation after the restart replays the first invocation (including the durable
    // send) and then performs a new live request, which derives a different key from its own
    // durable call position.
    let fresh_result = executor
        .invoke_and_await_agent(&component, &agent_id, "run", data_value!())
        .await?
        .into_typed::<String>()?;
    assert!(
        fresh_result.starts_with("200 ExampleResponse { percentage: 0.0, message: Some("),
        "unexpected response for the post-restart invocation: {fresh_result}"
    );
    assert_ne!(fresh_result, expected_response);

    executor.check_oplog_is_queryable(&worker_id).await?;

    drop(executor);
    http_server.abort();

    Ok(())
}

/// A guest task `spawn`ed during an invocation that performs its durable HTTP call only after
/// the export has returned must be drained before the invocation completes: the durable
/// `Start`/`End` entries of the spawned task's `http::client::send` land *before* the
/// `AgentInvocationFinished` entry, and the recorded run replays successfully after a restart.
#[test]
#[tracing::instrument]
async fn spawned_guest_task_durable_call_lands_before_invocation_finished(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("http_tests")] http_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    use golem_common::model::oplog::{OplogIndex, PublicOplogEntry};

    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await.unwrap();
    let host_http_port = listener.local_addr().unwrap().port();

    let http_server = spawn(
        async move {
            let route = Router::new().route(
                "/spawned",
                axum::routing::get(|| async { "spawned-response" }),
            );
            axum::serve(listener, route).await.unwrap();
        }
        .in_current_span(),
    );

    let component = executor
        .component_dep(&context.default_environment_id, http_tests)
        .store()
        .await?;
    let mut env = HashMap::new();
    env.insert("PORT".to_string(), host_http_port.to_string());

    let agent_id = agent_id!("HttpClient4");
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), env, Vec::new())
        .await?;

    let result = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "get_in_spawned_task_after_return",
            data_value!(),
        )
        .await?
        .into_typed::<String>()?;
    assert_eq!(result, "spawned");

    // The spawned task's durable send must be recorded *within* the method invocation: a
    // completed `Start`(http::client::send)/`End` pair between the method's
    // `AgentInvocationStarted` and its `AgentInvocationFinished` entry (there is also an earlier
    // agent-initialization invocation pair in the oplog, so anchor on the method's own window).
    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    let started_index = oplog
        .iter()
        .find_map(|e| match &e.entry {
            PublicOplogEntry::AgentInvocationStarted(params) => match &params.invocation {
                golem_common::model::oplog::PublicAgentInvocation::AgentMethodInvocation(m)
                    if m.method_name.replace('-', "_") == "get_in_spawned_task_after_return" =>
                {
                    Some(e.oplog_index)
                }
                _ => None,
            },
            _ => None,
        })
        .expect("expected an AgentInvocationStarted entry for the method invocation");
    let finished_index = oplog
        .iter()
        .find_map(|e| {
            (e.oplog_index > started_index
                && matches!(e.entry, PublicOplogEntry::AgentInvocationFinished(_)))
            .then_some(e.oplog_index)
        })
        .expect("expected an AgentInvocationFinished entry for the method invocation");
    let send_start_index = oplog
        .iter()
        .find_map(|e| match &e.entry {
            PublicOplogEntry::Start(params)
                if params.function_name == "http::client::send"
                    && e.oplog_index > started_index
                    && e.oplog_index < finished_index =>
            {
                Some(e.oplog_index)
            }
            _ => None,
        })
        .expect(
            "expected a durable http::client::send Start entry within the method invocation \
             window",
        );
    let send_end_index = oplog
        .iter()
        .find_map(|e| match &e.entry {
            PublicOplogEntry::End(params) if params.start_index == send_start_index => {
                Some(e.oplog_index)
            }
            _ => None,
        })
        .expect("expected a matching End entry for the spawned task's send");
    assert!(
        send_end_index < finished_index,
        "the spawned task's durable send must complete within the method invocation \
         (AgentInvocationStarted at {started_index}, Start at {send_start_index}, End at \
         {send_end_index}, AgentInvocationFinished at {finished_index})"
    );

    // Restart the executor to force a full oplog replay of the recorded invocation (including
    // the spawned task's durable calls), then run a fresh live invocation on top of it.
    drop(executor);
    let executor = start(deps, &context).await?;

    let replayed_result = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "get_in_spawned_task_after_return",
            data_value!(),
        )
        .await?
        .into_typed::<String>()?;
    assert_eq!(replayed_result, "spawned");

    executor.check_oplog_is_queryable(&worker_id).await?;

    drop(executor);
    http_server.abort();

    Ok(())
}
