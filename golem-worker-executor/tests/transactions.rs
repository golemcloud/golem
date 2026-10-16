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
use axum::extract::Path;
use axum::routing::{delete, get};
use golem_common::model::IdempotencyKey;
use golem_common::schema::SchemaValue;
use golem_common::{agent_id, data_value};
use golem_test_framework::dsl::TestDsl;
use golem_worker_executor_test_utils::{
    LastUniqueId, PrecompiledComponent, TestContext, WorkerExecutorTestDependencies, start,
};
use pretty_assertions::{assert_eq, assert_ne};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use test_r::{inherit_test_dep, test, timeout};
use tokio::task::JoinHandle;
use tracing::{Instrument, debug, instrument};

inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(LastUniqueId);
inherit_test_dep!(
    #[tagged_as("host_api_tests")]
    PrecompiledComponent
);
inherit_test_dep!(
    #[tagged_as("agent_counters")]
    PrecompiledComponent
);
inherit_test_dep!(Tracing);

struct TestHttpServer {
    handle: JoinHandle<()>,
    events: Arc<Mutex<Vec<String>>>,
    port: u16,
}

impl TestHttpServer {
    pub async fn start_custom(
        fail_per_step: Arc<impl Fn(u64) -> u64 + Send + Sync + 'static>,
        log_steps: bool,
    ) -> Self {
        let events = Arc::new(Mutex::new(Vec::new()));
        let events_clone = events.clone();
        let events_clone2 = events.clone();

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
#[instrument]
async fn golem_rust_explicit_oplog_commit(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;

    let agent_id = agent_id!("GolemHostApi", "explicit-oplog-commit");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    let mut _log_output_guards = Vec::new();
    _log_output_guards.push(executor.log_output_scoped(&worker_id).await?);

    // Test environments use non-replicated indexed storage, so committing to 0 replicas must
    // succeed immediately.
    let result = executor
        .invoke_and_await_agent(&component, &agent_id, "explicit_commit", data_value!(0u8))
        .await;

    executor.check_oplog_is_queryable(&worker_id).await?;

    assert!(result.is_ok());
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("4m")]
async fn golem_rust_fallible_transaction(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
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
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;

    let mut env = HashMap::new();
    env.insert("PORT".to_string(), http_server.port().to_string());

    let agent_id = agent_id!("GolemHostApi", "fallible-transaction");
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), env, Vec::new())
        .await?;

    let mut _log_output_guards = Vec::new();
    _log_output_guards.push(executor.log_output_scoped(&worker_id).await?);

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
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
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
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;

    let mut env = HashMap::new();
    env.insert("PORT".to_string(), http_server.port().to_string());

    let agent_id = agent_id!("GolemHostApi", "infallible-transaction");
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), env, Vec::new())
        .await?;

    let mut _log_output_guards = Vec::new();
    _log_output_guards.push(executor.log_output_scoped(&worker_id).await?);

    let result = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "infallible_transaction_test",
            data_value!(),
        )
        .await?
        .into_typed::<u64>()?;

    let events = http_server.get_events();

    executor.check_oplog_is_queryable(&worker_id).await?;

    drop(executor);
    http_server.abort();

    assert_eq!(result, 11);
    assert_eq!(
        events,
        vec![
            "=> 1".to_string(),
            "=> 2".to_string(),
            "=> 3".to_string(),
            "<= 3".to_string(),
            "<= 2".to_string(),
            "<= 1".to_string(),
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
    #[tagged_as("agent_counters")] agent_counters: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component_dep(&context.default_environment_id, agent_counters)
        .store()
        .await?;

    let agent_id = agent_id!("HostFunctionTests", "idempotency_keys_in_ephemeral_workers");
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

    fn returned_keys_are_different(value: &SchemaValue) -> bool {
        if let SchemaValue::Tuple { elements: items } = value {
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
