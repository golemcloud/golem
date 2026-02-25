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
use anyhow::anyhow;
use axum::extract::Path;
use axum::routing::{delete, get, post};
use axum::Router;
use bytes::Bytes;
use golem_common::model::IdempotencyKey;
use golem_common::{agent_id, data_value};
use golem_test_framework::dsl::{
    drain_connection, stdout_event_starting_with, stdout_events, TestDsl,
};
use golem_wasm::Value;
use golem_worker_executor_test_utils::{
    start, LastUniqueId, TestContext, WorkerExecutorTestDependencies,
};
use pretty_assertions::{assert_eq, assert_ne};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};
use test_r::{inherit_test_dep, test, timeout};
use tokio::task::JoinHandle;
use tracing::info;
use tracing::{debug, instrument, Instrument};

inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(LastUniqueId);
inherit_test_dep!(Tracing);

struct TestHttpServer {
    handle: JoinHandle<()>,
    events: Arc<Mutex<Vec<String>>>,
    port: u16,
}

impl TestHttpServer {
    pub async fn start(fail_per_step: u64) -> Self {
        Self::start_custom(Arc::new(move |_| fail_per_step), false).await
    }

    pub async fn start_custom(
        fail_per_step: Arc<impl Fn(u64) -> u64 + Send + Sync + 'static>,
        log_steps: bool,
    ) -> Self {
        let events = Arc::new(Mutex::new(Vec::new()));
        let events_clone = events.clone();
        let events_clone2 = events.clone();
        let events_clone3 = events.clone();

        let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();

        let handle = tokio::spawn(
            async move {
                let call_count_per_step = Arc::new(Mutex::new(HashMap::<u64, u64>::new()));
                let route = Router::new()
                    .route(
                        "/step/{step}",
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
                        "/step/{step}",
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

                axum::serve(listener, route).await.unwrap();
            }
            .in_current_span(),
        );
        Self {
            handle,
            events,
            port,
        }
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    pub fn abort(&self) {
        self.handle.abort()
    }

    pub fn get_events(&self) -> Vec<String> {
        self.events.lock().unwrap().clone()
    }
}

// golem-rust library tests

#[test]
#[tracing::instrument]
#[timeout("4m")]
async fn golem_rust_jump(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let http_server = TestHttpServer::start(1).await;

    let component = executor
        .component(
            &context.default_environment_id,
            "golem_it_host_api_tests_release",
        )
        .name("golem-it:host-api-tests")
        .store()
        .await?;

    let mut env = HashMap::new();
    env.insert("PORT".to_string(), http_server.port().to_string());

    let agent_id = agent_id!("golem-host-api", "jump");
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), env, HashMap::new())
        .await?;

    let (rx, abort_capture) = executor.capture_output_with_termination(&worker_id).await?;

    let result = executor
        .invoke_and_await_agent(&component, &agent_id, "jump", data_value!())
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected return value"))?;

    while (rx.len() as u64) < 17 {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    executor.check_oplog_is_queryable(&worker_id).await?;

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

    info!("events: {:?}", events);

    assert_eq!(result, Value::U64(5));
    assert_eq!(
        stdout_events(events.into_iter().flatten()),
        vec![
            "started: 0\n",
            "second: 2\n",
            "second: 2\n",
            "third: 3\n",
            "fourth: 4\n",
            "fourth: 4\n",
            "fifth: 5\n",
        ]
    );

    Ok(())
}

#[test]
#[instrument]
async fn golem_rust_explicit_oplog_commit(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component(
            &context.default_environment_id,
            "golem_it_host_api_tests_release",
        )
        .name("golem-it:host-api-tests")
        .store()
        .await?;

    let agent_id = agent_id!("golem-host-api", "explicit-oplog-commit");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    executor.log_output(&worker_id).await?;

    // Note: we can only test with replicas=0 because we don't have redis slaves in the test environment currently
    let result = executor
        .invoke_and_await_agent(&component, &agent_id, "explicit_commit", data_value!(0u8))
        .await;

    executor.check_oplog_is_queryable(&worker_id).await?;

    assert!(result.is_ok());
    Ok(())
}

#[test]
#[instrument]
async fn golem_rust_set_retry_policy(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component(
            &context.default_environment_id,
            "golem_it_host_api_tests_release",
        )
        .name("golem-it:host-api-tests")
        .store()
        .await?;

    let agent_id = agent_id!("golem-host-api", "set-retry-policy-1");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    executor.log_output(&worker_id).await?;

    let start = SystemTime::now();
    let result1 = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "fail_with_custom_max_retries",
            data_value!(2u64),
        )
        .await;
    let elapsed = start.elapsed().unwrap();

    let result2 = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "fail_with_custom_max_retries",
            data_value!(1u64),
        )
        .await;

    executor.check_oplog_is_queryable(&worker_id).await?;

    assert!(elapsed < Duration::from_secs(3)); // 2 retry attempts, 1s delay
    assert!(result1.is_err());
    assert!(result2.is_err());
    let result1_err = format!("{}", result1.unwrap_err());
    assert!(
        result1_err.contains("error while executing at wasm backtrace:")
            || result1_err.contains("Invocation failed"),
        "Unexpected error: {result1_err}"
    );
    let result2_err = format!("{}", result2.unwrap_err());
    assert!(
        result2_err.contains("Previous invocation failed")
            || result2_err.contains("error while executing at wasm backtrace:"),
        "Unexpected error: {result2_err}"
    );

    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("4m")]
async fn golem_rust_atomic_region(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let http_server = TestHttpServer::start(2).await;
    let component = executor
        .component(
            &context.default_environment_id,
            "golem_it_host_api_tests_release",
        )
        .name("golem-it:host-api-tests")
        .store()
        .await?;

    let mut env = HashMap::new();
    env.insert("PORT".to_string(), http_server.port().to_string());

    let agent_id = agent_id!("golem-host-api", "atomic-region");
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), env, HashMap::new())
        .await?;

    executor
        .invoke_and_await_agent(&component, &agent_id, "atomic_region", data_value!())
        .await?;

    executor.check_oplog_is_queryable(&worker_id).await?;

    drop(executor);
    http_server.abort();

    let events = http_server.get_events();
    info!("events:\n - {}", events.join("\n - "));

    assert_eq!(
        events,
        vec!["1", "2", "1", "2", "1", "2", "3", "4", "5", "5", "5", "6"]
    );

    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("4m")]
async fn golem_rust_idempotence_on(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let http_server = TestHttpServer::start(1).await;

    let component = executor
        .component(
            &context.default_environment_id,
            "golem_it_host_api_tests_release",
        )
        .name("golem-it:host-api-tests")
        .store()
        .await?;

    let mut env = HashMap::new();
    env.insert("PORT".to_string(), http_server.port().to_string());

    let agent_id = agent_id!("golem-host-api", "idempotence-flag-on");
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), env, HashMap::new())
        .await?;

    executor
        .invoke_and_await_agent(&component, &agent_id, "idempotence_flag", data_value!(true))
        .await?;

    executor.check_oplog_is_queryable(&worker_id).await?;

    drop(executor);
    http_server.abort();

    let events = http_server.get_events();
    info!("events:\n - {}", events.join("\n - "));

    assert_eq!(events, vec!["1", "1"]);

    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("4m")]
async fn golem_rust_idempotence_off(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let http_server = TestHttpServer::start(1).await;

    let component = executor
        .component(
            &context.default_environment_id,
            "golem_it_host_api_tests_release",
        )
        .name("golem-it:host-api-tests")
        .store()
        .await?;

    let mut env = HashMap::new();
    env.insert("PORT".to_string(), http_server.port().to_string());

    let agent_id = agent_id!("golem-host-api", "idempotence-flag-off");
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), env, HashMap::new())
        .await?;

    let result = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "idempotence_flag",
            data_value!(false),
        )
        .await;

    executor.check_oplog_is_queryable(&worker_id).await?;

    drop(executor);
    http_server.abort();

    let events = http_server.get_events();
    info!("events:\n - {}", events.join("\n - "));
    info!("result: {:?}", result);

    assert_eq!(events, vec!["1"]);
    assert!(result.is_err());

    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("4m")]
async fn golem_rust_persist_nothing(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let http_server = TestHttpServer::start(2).await;

    let component = executor
        .component(
            &context.default_environment_id,
            "golem_it_host_api_tests_release",
        )
        .name("golem-it:host-api-tests")
        .store()
        .await?;

    let mut env = HashMap::new();
    env.insert("PORT".to_string(), http_server.port().to_string());

    let agent_id = agent_id!("golem-host-api", "persist-nothing");
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), env, HashMap::new())
        .await?;

    let result = executor
        .invoke_and_await_agent(&component, &agent_id, "persist_nothing", data_value!())
        .await;

    executor.check_oplog_is_queryable(&worker_id).await?;

    drop(executor);
    http_server.abort();

    let events = http_server.get_events();
    info!("events:\n - {}", events.join("\n - "));
    info!("result: {:?}", result);

    assert_eq!(events, vec!["1", "2", "3"]);
    assert!(result.is_err());

    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("4m")]
async fn golem_rust_fallible_transaction(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let http_server = TestHttpServer::start_custom(
        Arc::new(|step| match step {
            3 => 1, // step 3 returns true once
            _ => 0, // other steps always return false
        }),
        true,
    )
    .await;

    let component = executor
        .component(
            &context.default_environment_id,
            "golem_it_host_api_tests_release",
        )
        .name("golem-it:host-api-tests")
        .store()
        .await?;

    let mut env = HashMap::new();
    env.insert("PORT".to_string(), http_server.port().to_string());

    let agent_id = agent_id!("golem-host-api", "fallible-transaction");
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), env, HashMap::new())
        .await?;

    executor.log_output(&worker_id).await?;

    let result = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "fallible_transaction_test",
            data_value!(),
        )
        .await;

    let events = http_server.get_events();

    executor.check_oplog_is_queryable(&worker_id).await?;

    drop(executor);
    http_server.abort();

    assert!(result.is_err());
    assert_eq!(
        events,
        vec![
            "=> 1".to_string(),
            "=> 2".to_string(),
            "=> 3".to_string(),
            "<= 3".to_string(),
            "<= 2".to_string(),
            "<= 1".to_string()
        ]
    );

    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("4m")]
async fn golem_rust_infallible_transaction(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let http_server = TestHttpServer::start_custom(
        Arc::new(|step| match step {
            3 => 1, // step 3 returns true once
            _ => 0, // other steps always return false
        }),
        true,
    )
    .await;

    let component = executor
        .component(
            &context.default_environment_id,
            "golem_it_host_api_tests_release",
        )
        .name("golem-it:host-api-tests")
        .store()
        .await?;

    let mut env = HashMap::new();
    env.insert("PORT".to_string(), http_server.port().to_string());

    let agent_id = agent_id!("golem-host-api", "infallible-transaction");
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), env, HashMap::new())
        .await?;

    executor.log_output(&worker_id).await?;

    let result = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "infallible_transaction_test",
            data_value!(),
        )
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected return value"))?;

    let events = http_server.get_events();

    executor.check_oplog_is_queryable(&worker_id).await?;

    drop(executor);
    http_server.abort();

    assert_eq!(result, Value::U64(11));
    assert_eq!(
        events,
        vec![
            "=> 1".to_string(),
            "=> 2".to_string(),
            "=> 3".to_string(),
            "=> 1".to_string(),
            "=> 2".to_string(),
            "=> 3".to_string(),
            "=> 4".to_string(),
        ]
    );

    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("4m")]
async fn idempotency_keys_in_ephemeral_workers(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component(&context.default_environment_id, "it_agent_counters_release")
        .name("it:agent-counters")
        .store()
        .await?;

    let agent_id = agent_id!(
        "host-function-tests",
        "idempotency_keys_in_ephemeral_workers"
    );
    let _worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    let idempotency_key1 = IdempotencyKey::fresh();
    let idempotency_key2 = IdempotencyKey::fresh();

    let result11 = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "generate_idempotency_keys",
            data_value!(),
        )
        .await?
        .into_return_value()
        .expect("Expected a return value");

    let result21 = executor
        .invoke_and_await_agent_with_key(
            &component,
            &agent_id,
            &idempotency_key1,
            "generate_idempotency_keys",
            data_value!(),
        )
        .await?
        .into_return_value()
        .expect("Expected a return value");

    let result31 = executor
        .invoke_and_await_agent_with_key(
            &component,
            &agent_id,
            &idempotency_key2,
            "generate_idempotency_keys",
            data_value!(),
        )
        .await?
        .into_return_value()
        .expect("Expected a return value");

    let result12 = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "generate_idempotency_keys",
            data_value!(),
        )
        .await?
        .into_return_value()
        .expect("Expected a return value");

    let result22 = executor
        .invoke_and_await_agent_with_key(
            &component,
            &agent_id,
            &idempotency_key1,
            "generate_idempotency_keys",
            data_value!(),
        )
        .await?
        .into_return_value()
        .expect("Expected a return value");

    let result32 = executor
        .invoke_and_await_agent_with_key(
            &component,
            &agent_id,
            &idempotency_key2,
            "generate_idempotency_keys",
            data_value!(),
        )
        .await?
        .into_return_value()
        .expect("Expected a return value");

    fn returned_keys_are_different(value: &Value) -> bool {
        if let Value::Tuple(items) = value {
            if items.len() == 2 {
                items[0] != items[1]
            } else {
                false
            }
        } else {
            false
        }
    }

    assert!(returned_keys_are_different(&result11));
    assert!(returned_keys_are_different(&result21));
    assert!(returned_keys_are_different(&result31));
    assert!(returned_keys_are_different(&result12));
    assert!(returned_keys_are_different(&result22));
    assert!(returned_keys_are_different(&result32));

    assert_ne!(result11, result12); // when not providing idempotency key it should return different keys
    assert_ne!(result11, result21);
    assert_ne!(result11, result31);
    assert_eq!(result21, result22); // same idempotency key should lead to the same result
    assert_eq!(result31, result32);

    Ok(())
}
