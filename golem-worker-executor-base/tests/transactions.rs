// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use test_r::{inherit_test_dep, test};

use crate::common::{start, TestContext};
use crate::{LastUniqueId, Tracing, WorkerExecutorTestDependencies};
use assert2::check;
use axum::extract::Path;
use axum::routing::{delete, get, post};
use axum::Router;
use bytes::Bytes;
use golem_common::model::{IdempotencyKey, TargetWorkerId};
use golem_test_framework::dsl::{
    drain_connection, stdout_event_starting_with, stdout_events, worker_error_message,
    TestDslUnsafe,
};
use golem_wasm_rpc::Value;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};
use tokio::task::JoinHandle;
use tracing::{debug, instrument};

inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(LastUniqueId);
inherit_test_dep!(Tracing);

struct TestHttpServer {
    handle: JoinHandle<()>,
    events: Arc<Mutex<Vec<String>>>,
}

impl TestHttpServer {
    pub fn start(host_http_port: u16, fail_per_step: u64) -> Self {
        Self::start_custom(host_http_port, Arc::new(move |_| fail_per_step), false)
    }

    pub fn start_custom(
        host_http_port: u16,
        fail_per_step: Arc<impl Fn(u64) -> u64 + Send + Sync + 'static>,
        log_steps: bool,
    ) -> Self {
        let events = Arc::new(Mutex::new(Vec::new()));
        let events_clone = events.clone();
        let events_clone2 = events.clone();
        let events_clone3 = events.clone();
        let handle = tokio::spawn(async move {
            let call_count_per_step = Arc::new(Mutex::new(HashMap::<u64, u64>::new()));
            let route = Router::new()
                .route(
                    "/step/:step",
                    get(move |step: Path<u64>| async move {
                        let step = step.0;
                        let mut steps = call_count_per_step.lock().unwrap();
                        let step_count = steps.entry(step).and_modify(|e| *e += 1).or_insert(0);

                        debug!("step: {} occurrence {step_count}", step);
                        if log_steps {
                            events_clone.lock().unwrap().push(format!("=> {step}"));
                        }

                        match step_count {
                            n if *n < fail_per_step(step) => "true",
                            _ => "false",
                        }
                    }),
                )
                .route(
                    "/step/:step",
                    delete(move |step: Path<u64>| async move {
                        let step = step.0;
                        debug!("step: undo {step}");
                        if log_steps {
                            events_clone2.lock().unwrap().push(format!("<= {step}"));
                        }
                        "false"
                    }),
                )
                .route(
                    "/side-effect",
                    post(move |body: Bytes| async move {
                        let body = String::from_utf8(body.to_vec()).unwrap();
                        debug!("received POST message: {body}");
                        events_clone3.lock().unwrap().push(body.clone());
                        "OK"
                    }),
                );
            let listener = tokio::net::TcpListener::bind(
                format!("0.0.0.0:{}", host_http_port)
                    .parse::<SocketAddr>()
                    .unwrap(),
            )
            .await
            .unwrap();
            axum::serve(listener, route).await.unwrap();
        });
        Self { handle, events }
    }

    pub fn abort(&self) {
        self.handle.abort()
    }

    pub fn get_events(&self) -> Vec<String> {
        self.events.lock().unwrap().clone()
    }
}

#[test]
#[tracing::instrument]
async fn jump(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await.unwrap();

    let host_http_port = context.host_http_port();

    let http_server = TestHttpServer::start(host_http_port, 1);

    let component_id = executor.store_component("runtime-service").await;

    let mut env = HashMap::new();
    env.insert("PORT".to_string(), context.host_http_port().to_string());

    let worker_id = executor
        .start_worker_with(&component_id, "runtime-service-jump", vec![], env)
        .await;

    let (rx, abort_capture) = executor.capture_output_forever(&worker_id).await;

    let result = executor
        .invoke_and_await(&worker_id, "golem:it/api.{jump}", vec![])
        .await
        .unwrap();

    while (rx.len() as u64) < 19 {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    drop(executor);
    http_server.abort();

    abort_capture.send(()).unwrap();
    let mut events = drain_connection(rx).await;
    events.retain(|e| match e {
        Some(e) => {
            !stdout_event_starting_with(e, "Sending") && !stdout_event_starting_with(e, "Received")
        }
        None => false,
    });

    println!("events: {:?}", events);

    check!(result == vec![Value::U64(5)]);
    check!(
        stdout_events(events.into_iter().flatten())
            == vec![
                "started: 0\n",
                "second: 2\n",
                "second: 2\n",
                "third: 3\n",
                "fourth: 4\n",
                "fourth: 4\n",
                "fifth: 5\n",
            ]
    );
}

#[test]
#[instrument]
async fn explicit_oplog_commit(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await.unwrap();

    let component_id = executor.store_component("runtime-service").await;

    let worker_id = executor
        .start_worker(&component_id, "runtime-service-explicit-oplog-commit")
        .await;

    executor.log_output(&worker_id).await;

    // Note: we can only test with replicas=0 because we don't have redis slaves in the test environment currently
    let result = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{explicit-commit}",
            vec![Value::U8(0)],
        )
        .await;

    drop(executor);
    check!(result.is_ok());
}

#[test]
#[instrument]
async fn set_retry_policy(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await.unwrap();

    let component_id = executor.store_component("runtime-service").await;
    let worker_id = executor
        .start_worker(&component_id, "set-retry-policy-1")
        .await;

    executor.log_output(&worker_id).await;

    let start = SystemTime::now();
    let result1 = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{fail-with-custom-max-retries}",
            vec![Value::U64(2)],
        )
        .await;
    let elapsed = start.elapsed().unwrap();

    let result2 = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{fail-with-custom-max-retries}",
            vec![Value::U64(1)],
        )
        .await;

    drop(executor);

    check!(elapsed < Duration::from_secs(3)); // 2 retry attempts, 1s delay
    check!(result1.is_err());
    check!(result2.is_err());
    check!(worker_error_message(&result1.clone().err().unwrap())
        .starts_with("Runtime error: error while executing at wasm backtrace:"));
    check!(worker_error_message(&result2.err().unwrap()).starts_with("Previous invocation failed"));
}

#[test]
#[tracing::instrument]
async fn atomic_region(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await.unwrap();

    let host_http_port = context.host_http_port();

    let http_server = TestHttpServer::start(host_http_port, 2);
    let component_id = executor.store_component("runtime-service").await;

    let mut env = HashMap::new();
    env.insert("PORT".to_string(), context.host_http_port().to_string());

    let worker_id = executor
        .start_worker_with(&component_id, "atomic-region", vec![], env)
        .await;

    let _ = executor
        .invoke_and_await(&worker_id, "golem:it/api.{atomic-region}", vec![])
        .await
        .unwrap();

    drop(executor);
    http_server.abort();

    let events = http_server.get_events();
    println!("events:\n - {}", events.join("\n - "));

    check!(events == vec!["1", "2", "1", "2", "1", "2", "3", "4", "5", "5", "5", "6"]);
}

#[test]
#[tracing::instrument]
async fn idempotence_on(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await.unwrap();

    let host_http_port = context.host_http_port();
    let http_server = TestHttpServer::start(host_http_port, 1);

    let component_id = executor.store_component("runtime-service").await;

    let mut env = HashMap::new();
    env.insert("PORT".to_string(), context.host_http_port().to_string());

    let worker_id = executor
        .start_worker_with(&component_id, "idempotence-flag", vec![], env)
        .await;

    let _ = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{idempotence-flag}",
            vec![Value::Bool(true)],
        )
        .await
        .unwrap();

    drop(executor);
    http_server.abort();

    let events = http_server.get_events();
    println!("events:\n - {}", events.join("\n - "));

    check!(events == vec!["1", "1"]);
}

#[test]
#[tracing::instrument]
async fn idempotence_off(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await.unwrap();

    let host_http_port = context.host_http_port();
    let http_server = TestHttpServer::start(host_http_port, 1);

    let component_id = executor.store_component("runtime-service").await;

    let mut env = HashMap::new();
    env.insert("PORT".to_string(), context.host_http_port().to_string());

    let worker_id = executor
        .start_worker_with(&component_id, "idempotence-flag", vec![], env)
        .await;

    let result = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{idempotence-flag}",
            vec![Value::Bool(false)],
        )
        .await;

    drop(executor);
    http_server.abort();

    let events = http_server.get_events();
    println!("events:\n - {}", events.join("\n - "));
    println!("result: {:?}", result);

    check!(events == vec!["1"]);
    check!(result.is_err());
}

#[test]
#[tracing::instrument]
async fn persist_nothing(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await.unwrap();

    let host_http_port = context.host_http_port();
    let http_server = TestHttpServer::start(host_http_port, 2);

    let component_id = executor.store_component("runtime-service").await;

    let mut env = HashMap::new();
    env.insert("PORT".to_string(), context.host_http_port().to_string());

    let worker_id = executor
        .start_worker_with(&component_id, "persist-nothing", vec![], env)
        .await;

    let result = executor
        .invoke_and_await(&worker_id, "golem:it/api.{persist-nothing}", vec![])
        .await;

    drop(executor);
    http_server.abort();

    let events = http_server.get_events();
    println!("events:\n - {}", events.join("\n - "));
    println!("result: {:?}", result);

    check!(events == vec!["1", "2", "3", "2", "2", "4"]);
    check!(result.is_ok());
}

// golem-rust library tests

#[test]
#[instrument]
async fn golem_rust_explicit_oplog_commit(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await.unwrap();

    let component_id = executor.store_component("golem-rust-tests").await;

    let worker_id = executor
        .start_worker(&component_id, "golem-rust-tests-explicit-oplog-commit")
        .await;

    executor.log_output(&worker_id).await;

    // Note: we can only test with replicas=0 because we don't have redis slaves in the test environment currently
    let result = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{explicit-commit}",
            vec![Value::U8(0)],
        )
        .await;

    drop(executor);
    check!(result.is_ok());
}

#[test]
#[instrument]
async fn golem_rust_set_retry_policy(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await.unwrap();

    let component_id = executor.store_component("golem-rust-tests").await;
    let worker_id = executor
        .start_worker(&component_id, "golem-rust-tests-set-retry-policy-1")
        .await;

    executor.log_output(&worker_id).await;

    let start = SystemTime::now();
    let result1 = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{fail-with-custom-max-retries}",
            vec![Value::U64(2)],
        )
        .await;
    let elapsed = start.elapsed().unwrap();

    let result2 = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{fail-with-custom-max-retries}",
            vec![Value::U64(1)],
        )
        .await;

    drop(executor);

    check!(elapsed < Duration::from_secs(3)); // 2 retry attempts, 1s delay
    check!(result1.is_err());
    check!(result2.is_err());
    check!(worker_error_message(&result1.clone().err().unwrap())
        .starts_with("Runtime error: error while executing at wasm backtrace:"));
    check!(worker_error_message(&result2.err().unwrap()).starts_with("Previous invocation failed"));
}

#[test]
#[tracing::instrument]
async fn golem_rust_atomic_region(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await.unwrap();

    let host_http_port = context.host_http_port();

    let http_server = TestHttpServer::start(host_http_port, 2);
    let component_id = executor.store_component("golem-rust-tests").await;

    let mut env = HashMap::new();
    env.insert("PORT".to_string(), context.host_http_port().to_string());

    let worker_id = executor
        .start_worker_with(&component_id, "golem-rust-tests-atomic-region", vec![], env)
        .await;

    let _ = executor
        .invoke_and_await(&worker_id, "golem:it/api.{atomic-region}", vec![])
        .await
        .unwrap();

    drop(executor);
    http_server.abort();

    let events = http_server.get_events();
    println!("events:\n - {}", events.join("\n - "));

    check!(events == vec!["1", "2", "1", "2", "1", "2", "3", "4", "5", "5", "5", "6"]);
}

#[test]
#[tracing::instrument]
async fn golem_rust_idempotence_on(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await.unwrap();

    let host_http_port = context.host_http_port();
    let http_server = TestHttpServer::start(host_http_port, 1);

    let component_id = executor.store_component("golem-rust-tests").await;

    let mut env = HashMap::new();
    env.insert("PORT".to_string(), context.host_http_port().to_string());

    let worker_id = executor
        .start_worker_with(
            &component_id,
            "golem-rust-tests-idempotence-flag-on",
            vec![],
            env,
        )
        .await;

    let _ = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{idempotence-flag}",
            vec![Value::Bool(true)],
        )
        .await
        .unwrap();

    drop(executor);
    http_server.abort();

    let events = http_server.get_events();
    println!("events:\n - {}", events.join("\n - "));

    check!(events == vec!["1", "1"]);
}

#[test]
#[tracing::instrument]
async fn golem_rust_idempotence_off(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await.unwrap();

    let host_http_port = context.host_http_port();
    let http_server = TestHttpServer::start(host_http_port, 1);

    let component_id = executor.store_component("golem-rust-tests").await;

    let mut env = HashMap::new();
    env.insert("PORT".to_string(), context.host_http_port().to_string());

    let worker_id = executor
        .start_worker_with(
            &component_id,
            "golem-rust-tests-idempotence-flag-off",
            vec![],
            env,
        )
        .await;

    let result = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{idempotence-flag}",
            vec![Value::Bool(false)],
        )
        .await;

    drop(executor);
    http_server.abort();

    let events = http_server.get_events();
    println!("events:\n - {}", events.join("\n - "));
    println!("result: {:?}", result);

    check!(events == vec!["1"]);
    check!(result.is_err());
}

#[test]
#[tracing::instrument]
async fn golem_rust_persist_nothing(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await.unwrap();

    let host_http_port = context.host_http_port();
    let http_server = TestHttpServer::start(host_http_port, 2);

    let component_id = executor.store_component("golem-rust-tests").await;

    let mut env = HashMap::new();
    env.insert("PORT".to_string(), context.host_http_port().to_string());

    let worker_id = executor
        .start_worker_with(
            &component_id,
            "golem-rust-tests-persist-nothing",
            vec![],
            env,
        )
        .await;

    let result = executor
        .invoke_and_await(&worker_id, "golem:it/api.{persist-nothing}", vec![])
        .await;

    drop(executor);
    http_server.abort();

    let events = http_server.get_events();
    println!("events:\n - {}", events.join("\n - "));
    println!("result: {:?}", result);

    check!(events == vec!["1", "2", "3", "2", "2", "4"]);
    check!(result.is_ok());
}

#[test]
#[tracing::instrument]
async fn golem_rust_fallible_transaction(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await.unwrap();

    let host_http_port = context.host_http_port();
    let http_server = TestHttpServer::start_custom(
        host_http_port,
        Arc::new(|step| match step {
            3 => 1, // step 3 returns true once
            _ => 0, // other steps always return false
        }),
        true,
    );

    let component_id = executor.store_component("golem-rust-tests").await;

    let mut env = HashMap::new();
    env.insert("PORT".to_string(), context.host_http_port().to_string());
    let worker_id = executor
        .start_worker_with(
            &component_id,
            "golem-rust-tests-fallible-transaction",
            vec![],
            env,
        )
        .await;

    executor.log_output(&worker_id).await;

    let result = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{fallible-transaction-test}",
            vec![],
        )
        .await;

    let events = http_server.get_events();

    drop(executor);
    http_server.abort();

    check!(result.is_err());
    check!(
        events
            == vec![
                "=> 1".to_string(),
                "=> 2".to_string(),
                "=> 3".to_string(),
                "<= 3".to_string(),
                "<= 2".to_string(),
                "<= 1".to_string()
            ]
    );
}

#[test]
#[tracing::instrument]
async fn golem_rust_infallible_transaction(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await.unwrap();

    let host_http_port = context.host_http_port();
    let http_server = TestHttpServer::start_custom(
        host_http_port,
        Arc::new(|step| match step {
            3 => 1, // step 3 returns true once
            _ => 0, // other steps always return false
        }),
        true,
    );

    let component_id = executor.store_component("golem-rust-tests").await;

    let mut env = HashMap::new();
    env.insert("PORT".to_string(), context.host_http_port().to_string());
    let worker_id = executor
        .start_worker_with(
            &component_id,
            "golem-rust-tests-infallible-transaction",
            vec![],
            env,
        )
        .await;

    executor.log_output(&worker_id).await;

    let result = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{infallible-transaction-test}",
            vec![],
        )
        .await;

    let events = http_server.get_events();

    drop(executor);
    http_server.abort();

    check!(result == Ok(vec![Value::U64(11)]));
    check!(
        events
            == vec![
                "=> 1".to_string(),
                "=> 2".to_string(),
                "=> 3".to_string(),
                "=> 1".to_string(),
                "=> 2".to_string(),
                "=> 3".to_string(),
                "=> 4".to_string(),
            ]
    );
}

#[test]
#[tracing::instrument]
async fn idempotency_keys_in_ephemeral_workers(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await.unwrap();

    let component_id = executor.store_ephemeral_component("runtime-service").await;

    let target_worker_id = TargetWorkerId {
        component_id,
        worker_name: None,
    };

    let idempotency_key1 = IdempotencyKey::fresh();
    let idempotency_key2 = IdempotencyKey::fresh();

    let result11 = executor
        .invoke_and_await(
            target_worker_id.clone(),
            "golem:it/api.{generate-idempotency-keys}",
            vec![],
        )
        .await
        .unwrap();
    let result21 = executor
        .invoke_and_await_with_key(
            target_worker_id.clone(),
            &idempotency_key1,
            "golem:it/api.{generate-idempotency-keys}",
            vec![],
        )
        .await
        .unwrap();
    let result31 = executor
        .invoke_and_await_with_key(
            target_worker_id.clone(),
            &idempotency_key2,
            "golem:it/api.{generate-idempotency-keys}",
            vec![],
        )
        .await
        .unwrap();
    let result12 = executor
        .invoke_and_await(
            target_worker_id.clone(),
            "golem:it/api.{generate-idempotency-keys}",
            vec![],
        )
        .await
        .unwrap();
    let result22 = executor
        .invoke_and_await_with_key(
            target_worker_id.clone(),
            &idempotency_key1,
            "golem:it/api.{generate-idempotency-keys}",
            vec![],
        )
        .await
        .unwrap();
    let result32 = executor
        .invoke_and_await_with_key(
            target_worker_id.clone(),
            &idempotency_key2,
            "golem:it/api.{generate-idempotency-keys}",
            vec![],
        )
        .await
        .unwrap();

    drop(executor);

    fn returned_keys_are_different(value: &[Value]) -> bool {
        if value.len() == 1 {
            if let Value::Tuple(items) = &value[0] {
                if items.len() == 2 {
                    items[0] != items[1]
                } else {
                    false
                }
            } else {
                false
            }
        } else {
            false
        }
    }

    check!(returned_keys_are_different(&result11));
    check!(returned_keys_are_different(&result21));
    check!(returned_keys_are_different(&result31));
    check!(returned_keys_are_different(&result12));
    check!(returned_keys_are_different(&result22));
    check!(returned_keys_are_different(&result32));

    check!(result11 != result12); // when not providing idempotency key it should return different keys
    check!(result11 != result21);
    check!(result11 != result31);
    check!(result21 == result22); // same idempotency key should lead to the same result
    check!(result31 == result32);
}
