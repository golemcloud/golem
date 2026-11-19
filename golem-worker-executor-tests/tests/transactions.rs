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
use axum::extract::{Path, Query};
use axum::http::HeaderMap;
use axum::routing::{delete, get};
use golem_common::model::IdempotencyKey;
use golem_common::model::oplog::{
    AgentError, OplogEntry, OplogIndex, PublicOplogEntry, PublicOplogEntryWithIndex,
};
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
use tokio::sync::Notify;
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

/// A peer that counts requests separately from keyed effects. The first selected
/// compensation takes effect but loses its response when the owner is stopped.
pub(crate) struct SagaPeer {
    handle: JoinHandle<()>,
    pub port: u16,
    pub attempts: Arc<Mutex<Vec<(String, String)>>>,
    pub effects: Arc<Mutex<Vec<(String, String)>>>,
    pub held: Arc<Notify>,
    pub release: Arc<Notify>,
}

impl SagaPeer {
    pub async fn start(hold: &str) -> Self {
        let attempts = Arc::new(Mutex::new(Vec::new()));
        let effects = Arc::new(Mutex::new(Vec::<(String, String)>::new()));
        let held = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let hold = hold.to_string();
        let record = {
            let attempts = attempts.clone();
            let effects = effects.clone();
            let held = held.clone();
            let release = release.clone();
            move |event: String, headers: HeaderMap| {
                let attempts = attempts.clone();
                let effects = effects.clone();
                let held = held.clone();
                let release = release.clone();
                let hold = hold.clone();
                async move {
                    let key = headers
                        .get("idempotency-key")
                        .unwrap()
                        .to_str()
                        .unwrap()
                        .to_string();
                    let first = {
                        let mut attempts = attempts.lock().unwrap();
                        let first = !attempts.iter().any(|(e, _)| e == &event);
                        attempts.push((event.clone(), key.clone()));
                        first
                    };
                    {
                        let mut effects = effects.lock().unwrap();
                        if !effects.iter().any(|(_, k)| k == &key) {
                            effects.push((event.clone(), key));
                        }
                    }
                    if event == hold && first {
                        held.notify_one();
                        release.notified().await;
                    }
                }
            }
        };
        let forward = record.clone();
        let undo = record.clone();
        let route = Router::new()
            .route(
                "/step/{step}",
                get(move |Path(step): Path<u64>, headers: HeaderMap| {
                    let record = forward.clone();
                    async move {
                        record(format!("=> {step}"), headers).await;
                        if step == 3 { "true" } else { "false" }
                    }
                })
                .delete(move |Path(step): Path<u64>, headers: HeaderMap| {
                    let record = undo.clone();
                    async move {
                        record(format!("<= {step}"), headers).await;
                        "false"
                    }
                }),
            )
            .route(
                "/callback",
                get(
                    move |Query(query): Query<HashMap<String, String>>, headers: HeaderMap| {
                        let record = record.clone();
                        async move {
                            let payload = query["payload"].clone();
                            record(payload.clone(), headers).await;
                            payload
                        }
                    },
                ),
            );
        let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let handle = tokio::spawn(
            async move { axum::serve(listener, route).await.unwrap() }.in_current_span(),
        );
        Self {
            handle,
            port,
            attempts,
            effects,
            held,
            release,
        }
    }

    pub fn assert_recovered(&self, expected: &[&str], held: &str) {
        let effects = self.effects.lock().unwrap().clone();
        let attempts = self.attempts.lock().unwrap().clone();
        assert_eq!(
            effects
                .iter()
                .map(|(event, _)| event.as_str())
                .collect::<Vec<_>>(),
            expected
        );
        for event in expected {
            let keys = attempts
                .iter()
                .filter(|(e, _)| e == event)
                .map(|(_, key)| key)
                .collect::<Vec<_>>();
            assert_eq!(
                keys.len(),
                if *event == held { 2 } else { 1 },
                "attempts for {event}: {attempts:?}"
            );
            assert!(
                keys.iter().all(|key| !key.is_empty() && *key == keys[0]),
                "lost key for {event}: {attempts:?}"
            );
        }
        assert_eq!(attempts.len(), expected.len() + 1);
    }

    pub fn assert_checkpoint(oplog: &[PublicOplogEntryWithIndex], completed: usize) {
        let sends = oplog
            .iter()
            .filter(|entry| {
                matches!(&entry.entry,
            PublicOplogEntry::Start(start) if start.function_name == "http::client::send")
            })
            .collect::<Vec<_>>();
        assert_eq!(sends.len(), completed + 1);
        for (index, send) in sends.iter().enumerate() {
            assert_eq!(
                oplog.iter().any(|entry| matches!(&entry.entry,
                PublicOplogEntry::End(end) if end.start_index == send.oplog_index)),
                index < completed,
                "HTTP completion at crash checkpoint: {send:?}"
            );
        }
    }
}

impl Drop for SagaPeer {
    fn drop(&mut self) {
        self.release.notify_waiters();
        self.handle.abort();
    }
}

#[test]
#[instrument]
#[timeout("4m")]
async fn golem_rust_compensation_survives_executor_restart(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let peer = SagaPeer::start("<= 2").await;
    let executor = start(deps, &context).await?;
    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;
    let agent_id = agent_id!("GolemHostApi", "compensation-owner-loss");
    let worker_id = executor
        .start_agent_with(
            &component.id,
            agent_id.clone(),
            HashMap::from([("PORT".to_string(), peer.port.to_string())]),
            Vec::new(),
        )
        .await?;
    let key = IdempotencyKey::fresh();
    let invocation = {
        let executor = executor.clone();
        let component = component.clone();
        let agent_id = agent_id.clone();
        let key = key.clone();
        tokio::spawn(async move {
            executor
                .invoke_and_await_agent_with_key(
                    &component,
                    &agent_id,
                    &key,
                    "fallible_transaction_test",
                    data_value!(),
                )
                .await
        })
    };
    peer.held.notified().await;
    assert_eq!(
        peer.effects
            .lock()
            .unwrap()
            .iter()
            .map(|(e, _)| e.clone())
            .collect::<Vec<_>>(),
        ["=> 1", "=> 2", "=> 3", "<= 3", "<= 2"]
    );
    SagaPeer::assert_checkpoint(
        &executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?,
        4,
    );
    invocation.abort();
    let _ = invocation.await;
    executor.release().await?;
    peer.release.notify_one();

    let executor = start(deps, &context).await?;
    let result = executor
        .invoke_and_await_agent_with_key(
            &component,
            &agent_id,
            &key,
            "fallible_transaction_test",
            data_value!(),
        )
        .await;
    assert!(result.is_err());
    let stored = executor.stored_oplog(&worker_id).await;
    let errors = stored
        .iter()
        .filter_map(|entry| match entry {
            OplogEntry::Error { error, .. } => Some(error),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert!(
        matches!(errors.as_slice(), [AgentError::DeterministicTrap(_)]),
        "{errors:?}"
    );
    peer.assert_recovered(&["=> 1", "=> 2", "=> 3", "<= 3", "<= 2", "<= 1"], "<= 2");
    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    let sends = oplog
        .iter()
        .filter(|entry| {
            matches!(&entry.entry,
        PublicOplogEntry::Start(start) if start.function_name == "http::client::send")
        })
        .collect::<Vec<_>>();
    assert_eq!(
        sends.len(),
        6,
        "repair must reuse the interrupted HTTP Start"
    );
    for send in sends {
        assert!(oplog.iter().any(|entry| matches!(&entry.entry,
            PublicOplogEntry::End(end) if end.start_index == send.oplog_index)));
    }
    executor.check_oplog_is_queryable(&worker_id).await?;
    Ok(())
}

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
