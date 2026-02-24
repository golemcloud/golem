// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
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
use axum::routing::post;
use axum::Router;
use bytes::Bytes;
use golem_common::model::IdempotencyKey;
use golem_common::{agent_id, data_value};
use golem_test_framework::dsl::TestDsl;
use golem_worker_executor_test_utils::{
    start, LastUniqueId, TestContext, WorkerExecutorTestDependencies,
};
use http::HeaderMap;
use pretty_assertions::assert_eq;
use serde_json::json;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use test_r::{inherit_test_dep, test};
use tokio::spawn;
use tracing::Instrument;

inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(LastUniqueId);
inherit_test_dep!(Tracing);

#[test]
#[tracing::instrument]
async fn http_client(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
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
        .component(
            &context.default_environment_id,
            "golem_it_http_tests_release",
        )
        .name("golem-it:http-tests")
        .store()
        .await?;
    let mut env = HashMap::new();
    env.insert("PORT".to_string(), host_http_port.to_string());
    env.insert("RUST_BACKTRACE".to_string(), "full".to_string());

    let agent_id = agent_id!("http-client");
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), env, HashMap::new())
        .await?;
    let rx = executor.capture_output(&worker_id).await?;

    let result = executor
        .invoke_and_await_agent(&component, &agent_id, "run", data_value!())
        .await?;

    executor.check_oplog_is_queryable(&worker_id).await?;

    drop(executor);
    drop(rx);
    http_server.abort();

    assert_eq!(result, data_value!("200 response is test-header test-body"));
    Ok(())
}

#[test]
#[tracing::instrument]
async fn http_client_using_reqwest(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
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
        .component(
            &context.default_environment_id,
            "golem_it_http_tests_release",
        )
        .name("golem-it:http-tests")
        .store()
        .await?;
    let mut env = HashMap::new();
    env.insert("PORT".to_string(), host_http_port.to_string());

    let agent_id = agent_id!("http-client2");
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), env, HashMap::new())
        .await?;

    let result = executor
        .invoke_and_await_agent(&component, &agent_id, "run", data_value!())
        .await?;

    let captured_body = captured_body.lock().unwrap().clone().unwrap();

    executor.check_oplog_is_queryable(&worker_id).await?;

    drop(executor);
    http_server.abort();

    assert_eq!(
        result,
        data_value!(
            "200 ExampleResponse { percentage: 0.25, message: Some(\"response message Golem\") }"
        )
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
        .component(
            &context.default_environment_id,
            "golem_it_http_tests_release",
        )
        .name("golem-it:http-tests")
        .store()
        .await?;
    let mut env = HashMap::new();
    env.insert("PORT".to_string(), host_http_port.to_string());

    let agent_id = agent_id!("http-client3");
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), env, HashMap::new())
        .await?;

    let result = executor
        .invoke_and_await_agent(&component, &agent_id, "run", data_value!())
        .await?;
    let captured_body = captured_body.lock().unwrap().clone().unwrap();

    executor.check_oplog_is_queryable(&worker_id).await?;

    drop(executor);
    http_server.abort();

    assert_eq!(
        result,
        data_value!(
            "200 ExampleResponse { percentage: 0.25, message: Some(\"response message Golem\") }"
        )
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
        .component(
            &context.default_environment_id,
            "golem_it_http_tests_release",
        )
        .name("golem-it:http-tests")
        .store()
        .await?;
    let mut env = HashMap::new();
    env.insert("PORT".to_string(), host_http_port.to_string());

    let agent_id = agent_id!("http-client3");
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), env, HashMap::new())
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
    let golem_wasm::Value::List(lst) = &return_value else {
        panic!("Expected List, got {:?}", &return_value)
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

#[test]
#[tracing::instrument]
async fn outgoing_http_contains_idempotency_key(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
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
        .component(
            &context.default_environment_id,
            "golem_it_http_tests_release",
        )
        .name("golem-it:http-tests")
        .store()
        .await?;
    let mut env = HashMap::new();
    env.insert("PORT".to_string(), host_http_port.to_string());

    let agent_id = agent_id!("http-client2");
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), env, HashMap::new())
        .await?;

    let key = IdempotencyKey::new("177db03d-3234-4a04-8d03-e8d042348abd".to_string());
    let result = executor
        .invoke_and_await_agent_with_key(&component, &agent_id, &key, "run", data_value!())
        .await?;

    executor.check_oplog_is_queryable(&worker_id).await?;

    drop(executor);
    http_server.abort();

    assert_eq!(
        result, data_value!(
                "200 ExampleResponse { percentage: 0.0, message: Some(\"29e89d8e-585f-519d-a57b-fd8650d59edb\") }"
            )
    );
    Ok(())
}
