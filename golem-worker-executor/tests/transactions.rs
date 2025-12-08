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
use assert2::check;
use axum::extract::Path;
use axum::routing::{delete, get, post};
use axum::Router;
use bytes::Bytes;
use golem_common::model::oplog::WorkerError;
use golem_common::model::{IdempotencyKey, WorkerId};
use golem_test_framework::dsl::{
    drain_connection, stdout_event_starting_with, stdout_events, worker_error_logs,
    worker_error_message, worker_error_underlying_error, TestDsl,
};
use golem_wasm::{IntoValueAndType, Value};
use golem_worker_executor_test_utils::{
    start, LastUniqueId, TestContext, WorkerExecutorTestDependencies,
};
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

#[test]
#[tracing::instrument]
#[timeout(120_000)]
async fn jump(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let http_server = TestHttpServer::start(1).await;

    let component = executor
        .component(&context.default_environment_id, "runtime-service")
        .store()
        .await?;

    let mut env = HashMap::new();
    env.insert("PORT".to_string(), http_server.port().to_string());

    let worker_id = executor
        .start_worker_with(&component.id, "runtime-service-jump", env, vec![])
        .await?;

    let (rx, abort_capture) = executor.capture_output_with_termination(&worker_id).await?;

    let result = executor
        .invoke_and_await(&worker_id, "golem:it/api.{jump}", vec![])
        .await??;

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

    Ok(())
}

#[test]
#[instrument]
async fn explicit_oplog_commit(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component(&context.default_environment_id, "runtime-service")
        .store()
        .await?;

    let worker_id = executor
        .start_worker(&component.id, "runtime-service-explicit-oplog-commit")
        .await?;

    executor.log_output(&worker_id).await?;

    // Note: we can only test with replicas=0 because we don't have redis slaves in the test environment currently
    let result = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{explicit-commit}",
            vec![0u8.into_value_and_type()],
        )
        .await?;

    executor.check_oplog_is_queryable(&worker_id).await?;

    check!(result.is_ok());

    Ok(())
}

#[test]
#[instrument]
async fn set_retry_policy(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component(&context.default_environment_id, "runtime-service")
        .store()
        .await?;
    let worker_id = executor
        .start_worker(&component.id, "set-retry-policy-1")
        .await?;

    executor.log_output(&worker_id).await?;

    let start = SystemTime::now();
    let result1 = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{fail-with-custom-max-retries}",
            vec![2u64.into_value_and_type()],
        )
        .await?;
    let elapsed = start.elapsed().unwrap();

    let result2 = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{fail-with-custom-max-retries}",
            vec![1u64.into_value_and_type()],
        )
        .await?;

    executor.check_oplog_is_queryable(&worker_id).await?;

    assert!(elapsed < Duration::from_secs(3)); // 2 retry attempts, 1s delay
    assert!(result1.is_err());
    assert!(result2.is_err());

    let result1_err = result1.err().unwrap();
    assert_eq!(worker_error_message(&result1_err), "Invocation failed");
    assert!(
        matches!(worker_error_underlying_error(&result1_err), Some(WorkerError::Unknown(error)) if error.starts_with("error while executing at wasm backtrace:"))
    );
    assert_eq!(worker_error_logs(&result1_err), Some("\nthread '<unnamed>' (1) panicked at src/lib.rs:68:9:\nFail now\nnote: run with `RUST_BACKTRACE=1` environment variable to display a backtrace\n".to_string()));
    let result2_err = result2.err().unwrap();
    assert_eq!(
        worker_error_message(&result2_err),
        "Previous invocation failed"
    );
    assert!(
        matches!(worker_error_underlying_error(&result2_err), Some(WorkerError::Unknown(error)) if error.starts_with("error while executing at wasm backtrace:"))
    );
    assert_eq!(worker_error_logs(&result2_err), Some("\nthread '<unnamed>' (1) panicked at src/lib.rs:68:9:\nFail now\nnote: run with `RUST_BACKTRACE=1` environment variable to display a backtrace\n".to_string()));

    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout(120_000)]
async fn atomic_region(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let http_server = TestHttpServer::start(2).await;
    let component = executor
        .component(&context.default_environment_id, "runtime-service")
        .store()
        .await?;

    let mut env = HashMap::new();
    env.insert("PORT".to_string(), http_server.port().to_string());

    let worker_id = executor
        .start_worker_with(&component.id, "atomic-region", env, vec![])
        .await?;

    executor
        .invoke_and_await(&worker_id, "golem:it/api.{atomic-region}", vec![])
        .await??;

    executor.check_oplog_is_queryable(&worker_id).await?;

    drop(executor);
    http_server.abort();

    let events = http_server.get_events();
    info!("events:\n - {}", events.join("\n - "));

    check!(events == vec!["1", "2", "1", "2", "1", "2", "3", "4", "5", "5", "5", "6"]);

    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout(120_000)]
async fn idempotence_on(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let http_server = TestHttpServer::start(1).await;

    let component = executor
        .component(&context.default_environment_id, "runtime-service")
        .store()
        .await?;

    let mut env = HashMap::new();
    env.insert("PORT".to_string(), http_server.port().to_string());

    let worker_id = executor
        .start_worker_with(&component.id, "idempotence-flag", env, vec![])
        .await?;

    executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{idempotence-flag}",
            vec![true.into_value_and_type()],
        )
        .await??;

    executor.check_oplog_is_queryable(&worker_id).await?;

    drop(executor);
    http_server.abort();

    let events = http_server.get_events();
    info!("events:\n - {}", events.join("\n - "));

    check!(events == vec!["1", "1"]);

    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout(120_000)]
async fn idempotence_off(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let http_server = TestHttpServer::start(1).await;

    let component = executor
        .component(&context.default_environment_id, "runtime-service")
        .store()
        .await?;

    let mut env = HashMap::new();
    env.insert("PORT".to_string(), http_server.port().to_string());

    let worker_id = executor
        .start_worker_with(&component.id, "idempotence-flag", env, vec![])
        .await?;

    let result = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{idempotence-flag}",
            vec![false.into_value_and_type()],
        )
        .await?;

    executor.check_oplog_is_queryable(&worker_id).await?;

    drop(executor);
    http_server.abort();

    let events = http_server.get_events();
    info!("events:\n - {}", events.join("\n - "));
    info!("result: {:?}", result);

    check!(events == vec!["1"]);
    check!(result.is_err());

    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout(120_000)]
async fn persist_nothing(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let http_server = TestHttpServer::start(2).await;

    let component = executor
        .component(&context.default_environment_id, "runtime-service")
        .store()
        .await?;

    let mut env = HashMap::new();
    env.insert("PORT".to_string(), http_server.port().to_string());

    let worker_id = executor
        .start_worker_with(&component.id, "persist-nothing", env, vec![])
        .await?;

    let result = executor
        .invoke_and_await(&worker_id, "golem:it/api.{persist-nothing}", vec![])
        .await?;

    executor.check_oplog_is_queryable(&worker_id).await?;

    drop(executor);
    http_server.abort();

    let events = http_server.get_events();
    info!("events:\n - {}", events.join("\n - "));
    info!("result: {:?}", result);

    check!(events == vec!["1", "2", "3"]);
    check!(result.is_err());

    Ok(())
}

// golem-rust library tests

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
        .component(&context.default_environment_id, "golem-rust-tests")
        .store()
        .await?;

    let worker_id = executor
        .start_worker(&component.id, "golem-rust-tests-explicit-oplog-commit")
        .await?;

    executor.log_output(&worker_id).await?;

    // Note: we can only test with replicas=0 because we don't have redis slaves in the test environment currently
    let result = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{explicit-commit}",
            vec![0u8.into_value_and_type()],
        )
        .await?;

    executor.check_oplog_is_queryable(&worker_id).await?;

    check!(result.is_ok());
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
        .component(&context.default_environment_id, "golem-rust-tests")
        .store()
        .await?;
    let worker_id = executor
        .start_worker(&component.id, "golem-rust-tests-set-retry-policy-1")
        .await?;

    executor.log_output(&worker_id).await?;

    let start = SystemTime::now();
    let result1 = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{fail-with-custom-max-retries}",
            vec![2u64.into_value_and_type()],
        )
        .await?;
    let elapsed = start.elapsed().unwrap();

    let result2 = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{fail-with-custom-max-retries}",
            vec![1u64.into_value_and_type()],
        )
        .await?;

    executor.check_oplog_is_queryable(&worker_id).await?;

    assert!(elapsed < Duration::from_secs(3)); // 2 retry attempts, 1s delay
    assert!(result1.is_err());
    assert!(result2.is_err());
    let result1_err = result1.err().unwrap();
    assert_eq!(worker_error_message(&result1_err), "Invocation failed");
    assert!(
        matches!(worker_error_underlying_error(&result1_err), Some(WorkerError::Unknown(error)) if error.starts_with("error while executing at wasm backtrace:"))
    );
    assert_eq!(worker_error_logs(&result1_err), Some("\nthread '<unnamed>' (1) panicked at src/lib.rs:26:9:\nFail now\nnote: run with `RUST_BACKTRACE=1` environment variable to display a backtrace\n".to_string()));
    let result2_err = result2.err().unwrap();
    assert_eq!(
        worker_error_message(&result2_err),
        "Previous invocation failed"
    );
    assert!(
        matches!(worker_error_underlying_error(&result2_err), Some(WorkerError::Unknown(error)) if error.starts_with("error while executing at wasm backtrace:"))
    );
    assert_eq!(worker_error_logs(&result2_err), Some("\nthread '<unnamed>' (1) panicked at src/lib.rs:26:9:\nFail now\nnote: run with `RUST_BACKTRACE=1` environment variable to display a backtrace\n".to_string()));

    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout(120_000)]
async fn golem_rust_atomic_region(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let http_server = TestHttpServer::start(2).await;
    let component = executor
        .component(&context.default_environment_id, "golem-rust-tests")
        .store()
        .await?;

    let mut env = HashMap::new();
    env.insert("PORT".to_string(), http_server.port().to_string());

    let worker_id = executor
        .start_worker_with(&component.id, "golem-rust-tests-atomic-region", env, vec![])
        .await?;

    executor
        .invoke_and_await(&worker_id, "golem:it/api.{atomic-region}", vec![])
        .await??;

    executor.check_oplog_is_queryable(&worker_id).await?;

    drop(executor);
    http_server.abort();

    let events = http_server.get_events();
    info!("events:\n - {}", events.join("\n - "));

    check!(events == vec!["1", "2", "1", "2", "1", "2", "3", "4", "5", "5", "5", "6"]);

    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout(120_000)]
async fn golem_rust_idempotence_on(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let http_server = TestHttpServer::start(1).await;

    let component = executor
        .component(&context.default_environment_id, "golem-rust-tests")
        .store()
        .await?;

    let mut env = HashMap::new();
    env.insert("PORT".to_string(), http_server.port().to_string());

    let worker_id = executor
        .start_worker_with(
            &component.id,
            "golem-rust-tests-idempotence-flag-on",
            env,
            vec![],
        )
        .await?;

    executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{idempotence-flag}",
            vec![true.into_value_and_type()],
        )
        .await??;

    executor.check_oplog_is_queryable(&worker_id).await?;

    drop(executor);
    http_server.abort();

    let events = http_server.get_events();
    info!("events:\n - {}", events.join("\n - "));

    check!(events == vec!["1", "1"]);

    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout(120_000)]
async fn golem_rust_idempotence_off(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let http_server = TestHttpServer::start(1).await;

    let component = executor
        .component(&context.default_environment_id, "golem-rust-tests")
        .store()
        .await?;

    let mut env = HashMap::new();
    env.insert("PORT".to_string(), http_server.port().to_string());

    let worker_id = executor
        .start_worker_with(
            &component.id,
            "golem-rust-tests-idempotence-flag-off",
            env,
            vec![],
        )
        .await?;

    let result = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{idempotence-flag}",
            vec![false.into_value_and_type()],
        )
        .await?;

    executor.check_oplog_is_queryable(&worker_id).await?;

    drop(executor);
    http_server.abort();

    let events = http_server.get_events();
    info!("events:\n - {}", events.join("\n - "));
    info!("result: {:?}", result);

    check!(events == vec!["1"]);
    check!(result.is_err());

    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout(120_000)]
async fn golem_rust_persist_nothing(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let http_server = TestHttpServer::start(2).await;

    let component = executor
        .component(&context.default_environment_id, "golem-rust-tests")
        .store()
        .await?;

    let mut env = HashMap::new();
    env.insert("PORT".to_string(), http_server.port().to_string());

    let worker_id = executor
        .start_worker_with(
            &component.id,
            "golem-rust-tests-persist-nothing",
            env,
            vec![],
        )
        .await?;

    let result = executor
        .invoke_and_await(&worker_id, "golem:it/api.{persist-nothing}", vec![])
        .await?;

    executor.check_oplog_is_queryable(&worker_id).await?;

    drop(executor);
    http_server.abort();

    let events = http_server.get_events();
    info!("events:\n - {}", events.join("\n - "));
    info!("result: {:?}", result);

    check!(events == vec!["1", "2", "3"]);
    check!(result.is_err());

    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout(120_000)]
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
        .component(&context.default_environment_id, "golem-rust-tests")
        .store()
        .await?;

    let mut env = HashMap::new();
    env.insert("PORT".to_string(), http_server.port().to_string());
    let worker_id = executor
        .start_worker_with(
            &component.id,
            "golem-rust-tests-fallible-transaction",
            env,
            vec![],
        )
        .await?;

    executor.log_output(&worker_id).await?;

    let result = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{fallible-transaction-test}",
            vec![],
        )
        .await?;

    let events = http_server.get_events();

    executor.check_oplog_is_queryable(&worker_id).await?;

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

    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout(120_000)]
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
        .component(&context.default_environment_id, "golem-rust-tests")
        .store()
        .await?;

    let mut env = HashMap::new();
    env.insert("PORT".to_string(), http_server.port().to_string());
    let worker_id = executor
        .start_worker_with(
            &component.id,
            "golem-rust-tests-infallible-transaction",
            env,
            vec![],
        )
        .await?;

    executor.log_output(&worker_id).await?;

    let result = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{infallible-transaction-test}",
            vec![],
        )
        .await?;

    let events = http_server.get_events();

    executor.check_oplog_is_queryable(&worker_id).await?;

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

    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout(120_000)]
async fn idempotency_keys_in_ephemeral_workers(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component(&context.default_environment_id, "it_agent_counters_release")
        .store()
        .await?;

    let worker_id = WorkerId {
        component_id: component.id,
        worker_name: "host-function-tests(\"idempotency_keys_in_ephemeral_workers\")".to_string(),
    };

    let idempotency_key1 = IdempotencyKey::fresh();
    let idempotency_key2 = IdempotencyKey::fresh();

    let result11 = executor
        .invoke_and_await(
            &worker_id,
            "it:agent-counters/host-function-tests.{generate-idempotency-keys}",
            vec![],
        )
        .await??;

    let result21 = executor
        .invoke_and_await_with_key(
            &worker_id,
            &idempotency_key1,
            "it:agent-counters/host-function-tests.{generate-idempotency-keys}",
            vec![],
        )
        .await??;

    let result31 = executor
        .invoke_and_await_with_key(
            &worker_id,
            &idempotency_key2,
            "it:agent-counters/host-function-tests.{generate-idempotency-keys}",
            vec![],
        )
        .await??;

    let result12 = executor
        .invoke_and_await(
            &worker_id,
            "it:agent-counters/host-function-tests.{generate-idempotency-keys}",
            vec![],
        )
        .await??;

    let result22 = executor
        .invoke_and_await_with_key(
            &worker_id,
            &idempotency_key1,
            "it:agent-counters/host-function-tests.{generate-idempotency-keys}",
            vec![],
        )
        .await??;

    let result32 = executor
        .invoke_and_await_with_key(
            &worker_id,
            &idempotency_key2,
            "it:agent-counters/host-function-tests.{generate-idempotency-keys}",
            vec![],
        )
        .await??;

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

    Ok(())
}
