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
use golem_common::model::IdempotencyKey;
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
/// must replay them claim-based rather than positionally. Runs an invocation
/// with many parallel outgoing requests, restarts the executor (forcing a full
/// oplog replay of the interleaved records), and runs a fresh invocation on
/// the recovered worker.
#[test]
#[tracing::instrument]
async fn http_client_using_reqwest_async_parallel_replay(
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
        .invoke_and_await_agent(&component, &agent_id, "run_parallel", data_value!(16u16))
        .await?;
    let return_value = result.into_return_value().expect("Expected a return value");
    let SchemaValue::List { elements: lst } = &return_value else {
        panic!("Expected List, got {return_value:?}")
    };
    assert_eq!(lst.len(), 16);
    assert_eq!(captured_body.lock().unwrap().len(), 16);

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

async fn recv_close_event(
    rx: &mut mpsc::UnboundedReceiver<anyhow::Result<bool>>,
) -> anyhow::Result<bool> {
    timeout(Duration::from_secs(10), rx.recv())
        .await?
        .transpose()
        .map(|closed| closed.unwrap_or(false))
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
    executor.check_oplog_is_queryable(&worker_id).await?;

    drop(executor);
    http_server.abort();

    Ok(())
}

/// Dropping a still-pending P3 response future must cancel the durable send
/// (`Cancelled` on replay for the in-flight write) and abort the underlying
/// HTTP request instead of leaving the socket parked waiting for response
/// headers. The restarted invocation replays the cancellation and then runs a
/// fresh cancellation live.
#[test]
#[tracing::instrument]
async fn outgoing_http_response_future_cancel_aborts_request_and_replays(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("http_tests")] http_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await.unwrap();
    let host_http_port = listener.local_addr().unwrap().port();
    let (closed_tx, mut closed_rx) = mpsc::unbounded_channel();

    let http_server = spawn(
        async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                let closed_tx = closed_tx.clone();
                spawn(async move {
                    let result = async {
                        let request = read_request_headers(&mut stream).await?;
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
    assert!(
        recv_close_event(&mut closed_rx).await?,
        "post-restart cancellation must still close the fresh live request connection"
    );

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
