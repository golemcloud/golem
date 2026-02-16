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

use axum::extract::Path;
use axum::routing::get;
use axum::{Json, Router};
use golem_common::model::oplog::{OplogIndex, PublicOplogEntry};
use golem_common::model::{IdempotencyKey, WorkerId, WorkerStatus};
use golem_common::{agent_id, data_value, phantom_agent_id};
use golem_test_framework::config::{EnvBasedTestDependencies, TestDependencies};
use golem_test_framework::dsl::{TestDsl, TestDslExtended};
use golem_wasm::Value;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use test_r::{inherit_test_dep, test, timeout};
use tracing::{info, Instrument};
use uuid::Uuid;

inherit_test_dep!(Tracing);
inherit_test_dep!(EnvBasedTestDependencies);

#[test]
#[tracing::instrument]
#[timeout(120000)]
async fn fork_interrupted_worker(
    deps: &EnvBasedTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let (_, env) = user.app_and_env().await?;
    let response = Arc::new(Mutex::new("initial".to_string()));
    let host_http_port = 8586;

    let http_server = run_http_server(&response, host_http_port);

    let component = user
        .component(&env.id, "golem_it_http_tests_release")
        .name("golem-it:http-tests")
        .store()
        .await?;
    let mut env = HashMap::new();
    env.insert("PORT".to_string(), host_http_port.to_string());

    let source_agent_id = agent_id!("http-client2");
    let worker_id = user
        .start_agent_with(&component.id, source_agent_id.clone(), env, vec![])
        .await?;

    let target_agent_id = phantom_agent_id!("http-client2", Uuid::new_v4());
    let target_worker_name = target_agent_id.to_string();

    let target_worker_id = WorkerId {
        component_id: component.id,
        worker_name: target_worker_name.clone(),
    };

    user.log_output(&worker_id).await?;

    user.invoke_agent(
        &component.id,
        &source_agent_id,
        "start_polling",
        data_value!("first"),
    )
    .await?;

    user.wait_for_status(&worker_id, WorkerStatus::Running, Duration::from_secs(10))
        .await?;

    user.interrupt(&worker_id).await?;

    let oplog = user.get_oplog(&worker_id, OplogIndex::INITIAL).await?;

    let last_index = OplogIndex::from_u64(oplog.len() as u64);

    user.fork_worker(&worker_id, &target_worker_name, last_index)
        .await?;

    {
        let mut response = response.lock().unwrap();
        *response = "first".to_string();
    }

    user.wait_for_status(
        &target_worker_id,
        WorkerStatus::Idle,
        Duration::from_secs(10),
    )
    .await?;

    let result = user
        .search_oplog(&target_worker_id, "Received first")
        .await?;

    http_server.abort();

    assert_eq!(result.len(), 1);
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout(120000)]
async fn fork_running_worker_1(
    deps: &EnvBasedTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let (_, env) = user.app_and_env().await?;

    let component = user
        .component(&env.id, "it_agent_counters_release")
        .name("it:agent-counters")
        .store()
        .await?;

    let source_name = Uuid::new_v4().to_string();
    let source_agent_id = agent_id!("repository", source_name.clone());
    let source_worker_id = user
        .start_agent(&component.id, source_agent_id.clone())
        .await?;

    user.invoke_and_await_agent(
        &component.id,
        &source_agent_id,
        "add",
        data_value!("G1002", "Mud Golem"),
    )
    .await?;

    let target_name = Uuid::new_v4().to_string();
    let target_agent_id = agent_id!("repository", target_name.clone());
    let target_worker_id = WorkerId {
        component_id: component.id,
        worker_name: target_agent_id.to_string(),
    };

    let source_oplog = user
        .get_oplog(&source_worker_id, OplogIndex::INITIAL)
        .await?;

    let (idx, _) = source_oplog
        .iter()
        .enumerate()
        .rev()
        .find(|(_, entry)| matches!(&entry.entry, PublicOplogEntry::ExportedFunctionInvoked(_)))
        .expect("Expected ExportedFunctionInvoked in oplog");

    let oplog_index_of_function_invoked = OplogIndex::from_u64((idx + 1) as u64);

    user.fork_worker(
        &source_worker_id,
        &target_agent_id.to_string(),
        oplog_index_of_function_invoked,
    )
    .await?;

    user.wait_for_status(
        &target_worker_id,
        WorkerStatus::Idle,
        Duration::from_secs(10),
    )
    .await?;

    let total_invocations = user
        .search_oplog(&target_worker_id, "add AND invoke AND NOT pending")
        .await?;

    assert_eq!(total_invocations.len(), 1);

    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout(120000)]
async fn fork_running_worker_2(
    deps: &EnvBasedTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let (_, env) = user.app_and_env().await?;

    let response = Arc::new(Mutex::new("initial".to_string()));
    let host_http_port = 8587;
    let http_server = run_http_server(&response, host_http_port);

    let component = user
        .component(&env.id, "golem_it_http_tests_release")
        .name("golem-it:http-tests")
        .store()
        .await?;
    let mut env = HashMap::new();
    env.insert("PORT".to_string(), host_http_port.to_string());

    let source_agent_id = agent_id!("http-client2");
    let source_worker_id = user
        .start_agent_with(&component.id, source_agent_id.clone(), env, vec![])
        .await?;

    let target_agent_id = phantom_agent_id!("http-client2", Uuid::new_v4());
    let target_worker_name = target_agent_id.to_string();

    let target_worker_id = WorkerId {
        component_id: component.id,
        worker_name: target_worker_name.clone(),
    };

    user.log_output(&source_worker_id).await?;

    user.invoke_agent(
        &component.id,
        &source_agent_id,
        "start_polling",
        data_value!("first"),
    )
    .await?;

    user.wait_for_status(
        &source_worker_id,
        WorkerStatus::Running,
        Duration::from_secs(10),
    )
    .await?;

    let oplog = user
        .get_oplog(&source_worker_id, OplogIndex::INITIAL)
        .await?;

    let last_index = OplogIndex::from_u64(oplog.len() as u64);

    user.fork_worker(&source_worker_id, &target_worker_name, last_index)
        .await?;

    {
        let mut response = response.lock().unwrap();
        *response = "first".to_string();
    }

    user.wait_for_status(
        &target_worker_id,
        WorkerStatus::Idle,
        Duration::from_secs(20),
    )
    .await?;

    user.wait_for_status(
        &source_worker_id,
        WorkerStatus::Idle,
        Duration::from_secs(20),
    )
    .await?;

    let target_result = user
        .search_oplog(&target_worker_id, "Received first")
        .await?;
    let source_result = user
        .search_oplog(&source_worker_id, "Received first")
        .await?;

    http_server.abort();

    assert_eq!(target_result.len(), 1);
    assert_eq!(source_result.len(), 1);

    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout(120000)]
async fn fork_idle_worker(
    deps: &EnvBasedTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let (_, env) = user.app_and_env().await?;
    let component = user
        .component(&env.id, "it_agent_counters_release")
        .name("it:agent-counters")
        .store()
        .await?;

    let source_name = Uuid::new_v4().to_string();
    let source_agent_id = agent_id!("repository", source_name.clone());
    let source_worker_id = user
        .start_agent(&component.id, source_agent_id.clone())
        .await?;

    user.invoke_and_await_agent(
        &component.id,
        &source_agent_id,
        "add",
        data_value!("G1001", "Golem Cloud Subscription 1y"),
    )
    .await?;

    user.invoke_and_await_agent(
        &component.id,
        &source_agent_id,
        "add",
        data_value!("G1002", "Mud Golem"),
    )
    .await?;

    let target_name = Uuid::new_v4().to_string();
    let target_agent_id = agent_id!("repository", target_name.clone());
    let target_worker_id = WorkerId {
        component_id: component.id,
        worker_name: target_agent_id.to_string(),
    };

    let source_oplog = user
        .get_oplog(&source_worker_id, OplogIndex::INITIAL)
        .await?;

    let log_record = source_oplog
        .last()
        .expect("Expect at least one entry in source oplog");

    assert!(matches!(
        &log_record.entry,
        PublicOplogEntry::ExportedFunctionCompleted(_)
    ));

    user.fork_worker(
        &source_worker_id,
        &target_agent_id.to_string(),
        OplogIndex::from_u64(source_oplog.len() as u64),
    )
    .await?;

    user.invoke_and_await_agent(
        &component.id,
        &target_agent_id,
        "add",
        data_value!("G1002", "Mud Golem"),
    )
    .await?;

    let original_contents = user
        .invoke_and_await_agent(&component.id, &source_agent_id, "list", data_value!())
        .await?;

    let forked_contents = user
        .invoke_and_await_agent(&component.id, &target_agent_id, "list", data_value!())
        .await?;

    let original_value = original_contents
        .into_return_value()
        .expect("Expected return value");

    let forked_value = forked_contents
        .into_return_value()
        .expect("Expected return value");

    assert_eq!(
        original_value,
        Value::List(vec![
            Value::Record(vec![
                Value::String("G1001".to_string()),
                Value::String("Golem Cloud Subscription 1y".to_string()),
                Value::U64(1),
            ]),
            Value::Record(vec![
                Value::String("G1002".to_string()),
                Value::String("Mud Golem".to_string()),
                Value::U64(1),
            ]),
        ])
    );

    assert_eq!(
        forked_value,
        Value::List(vec![
            Value::Record(vec![
                Value::String("G1001".to_string()),
                Value::String("Golem Cloud Subscription 1y".to_string()),
                Value::U64(1),
            ]),
            Value::Record(vec![
                Value::String("G1002".to_string()),
                Value::String("Mud Golem".to_string()),
                Value::U64(2),
            ]),
        ])
    );

    let result1 = user
        .search_oplog(&target_worker_id, "G1002 AND NOT pending")
        .await?;

    let result2 = user
        .search_oplog(&target_worker_id, "G1001 AND NOT pending")
        .await?;

    assert!(!result1.is_empty());
    assert!(!result2.is_empty());
    assert!(result1.len() > result2.len());

    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout(120000)]
async fn fork_worker_when_target_already_exists(
    deps: &EnvBasedTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let (_, env) = user.app_and_env().await?;
    let component = user
        .component(&env.id, "it_agent_counters_release")
        .name("it:agent-counters")
        .store()
        .await?;

    let source_name = Uuid::new_v4().to_string();
    let source_agent_id = agent_id!("repository", source_name.clone());
    let source_worker_id = user
        .start_agent(&component.id, source_agent_id.clone())
        .await?;

    user.invoke_and_await_agent(
        &component.id,
        &source_agent_id,
        "add",
        data_value!("G1001", "Golem Cloud Subscription 1y"),
    )
    .await?;

    let oplog_entries = user.search_oplog(&source_worker_id, "invoke").await?;

    let index = oplog_entries
        .last()
        .expect("Expect at least one oplog entry")
        .oplog_index;

    let error = user
        .fork_worker(&source_worker_id, &source_worker_id.worker_name, index)
        .await
        .unwrap_err()
        .to_string();

    assert!(error.contains("Worker already exists"));

    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout(120000)]
async fn fork_worker_with_invalid_oplog_index_cut_off(
    deps: &EnvBasedTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let (_, env) = user.app_and_env().await?;
    let component = user
        .component(&env.id, "it_agent_counters_release")
        .name("it:agent-counters")
        .store()
        .await?;

    let source_name = Uuid::new_v4().to_string();
    let source_agent_id = agent_id!("repository", source_name.clone());
    let source_worker_id = user
        .start_agent(&component.id, source_agent_id.clone())
        .await?;

    user.invoke_and_await_agent(
        &component.id,
        &source_agent_id,
        "add",
        data_value!("G1001", "Golem Cloud Subscription 1y"),
    )
    .await?;

    let target_name = Uuid::new_v4().to_string();
    let target_agent_id = agent_id!("repository", target_name.clone());

    let error = user
        .fork_worker(
            &source_worker_id,
            &target_agent_id.to_string(),
            OplogIndex::INITIAL,
        )
        .await
        .unwrap_err()
        .to_string();

    assert!(error.contains("oplog_index_cut_off must be at least 2"));

    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout(120000)]
async fn fork_invalid_worker(
    deps: &EnvBasedTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let (_, env) = user.app_and_env().await?;
    let component = user
        .component(&env.id, "it_agent_counters_release")
        .name("it:agent-counters")
        .store()
        .await?;

    let source_name = Uuid::new_v4().to_string();
    let source_agent_id = agent_id!("repository", source_name.clone());
    let target_name = Uuid::new_v4().to_string();
    let target_agent_id = agent_id!("repository", target_name.clone());

    let source_worker_id = WorkerId {
        component_id: component.id,
        worker_name: source_agent_id.to_string(),
    };

    let error = user
        .fork_worker(
            &source_worker_id,
            &target_agent_id.to_string(),
            OplogIndex::from_u64(14),
        )
        .await
        .unwrap_err()
        .to_string();

    assert!(error.contains(&format!("Worker not found: {source_worker_id}")));
    Ok(())
}

// Divergence possibility is mainly respect to environment variables referring to worker-ids.
// Fork shouldn't change the original environment variable values of the source worker
// stored in oplog until cut off
#[test]
#[tracing::instrument]
#[timeout(120000)]
async fn fork_worker_ensures_zero_divergence_until_cut_off(
    deps: &EnvBasedTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let (_, env) = user.app_and_env().await?;
    let component = user
        .component(&env.id, "golem_it_host_api_tests_release")
        .name("golem-it:host-api-tests")
        .store()
        .await?;

    let source_name = Uuid::new_v4().to_string();
    let source_agent_id = agent_id!("environment", source_name.clone());
    let source_worker_id = user
        .start_agent(&component.id, source_agent_id.clone())
        .await?;

    let source_result = user
        .invoke_and_await_agent(
            &component.id,
            &source_agent_id,
            "get_environment",
            data_value!(),
        )
        .await?;

    let source_worker_name = source_agent_id.to_string();

    let target_name = Uuid::new_v4().to_string();
    let target_agent_id = agent_id!("environment", target_name.clone());

    let oplog = user
        .get_oplog(&source_worker_id, OplogIndex::INITIAL)
        .await?;

    // We fork the worker post the completion and see if oplog corresponding to environment value
    // has the same value as the source worker. As far as the fork cut off point is post the
    // completion, there shouldn't be any divergence for worker information even if forked
    // worker name is different from the source worker name
    user.fork_worker(
        &source_worker_id,
        &target_agent_id.to_string(),
        OplogIndex::from_u64(oplog.len() as u64),
    )
    .await?;

    let target_worker_id = WorkerId {
        component_id: component.id,
        worker_name: target_agent_id.to_string(),
    };

    // Verify the forked worker's oplog has the same last entry as the source
    let forked_oplog = user
        .get_oplog(&target_worker_id, OplogIndex::INITIAL)
        .await?;

    let source_last = oplog.last().unwrap();
    let forked_last = forked_oplog.last().unwrap();

    assert!(
        matches!(
            &source_last.entry,
            PublicOplogEntry::ExportedFunctionCompleted(_)
        ),
        "Expected ExportedFunctionCompleted in source oplog"
    );

    assert_eq!(
        source_last.entry, forked_last.entry,
        "Forked worker oplog should have the same entries as source until cut off"
    );

    // Also verify the result contains the source worker's identity
    let result_str = format!("{:?}", source_result);
    assert!(
        result_str.contains(&source_name),
        "Environment should contain source worker name {source_worker_name}, got: {result_str}"
    );

    Ok(())
}

fn run_http_server(
    response: &Arc<Mutex<String>>,
    host_http_port: u16,
) -> tokio::task::JoinHandle<()> {
    let response_clone = response.clone();

    tokio::spawn(
        async move {
            let route = Router::new().route(
                "/poll",
                get(move || async move {
                    let body = response_clone.lock().unwrap();
                    body.clone()
                }),
            );

            let listener = tokio::net::TcpListener::bind(
                format!("0.0.0.0:{host_http_port}")
                    .parse::<SocketAddr>()
                    .unwrap(),
            )
            .await
            .unwrap();
            axum::serve(listener, route).await.unwrap();
        }
        .in_current_span(),
    )
}

#[test]
#[tracing::instrument]
#[timeout(120000)]
async fn fork_self(deps: &EnvBasedTestDependencies, _tracing: &Tracing) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let (_, env) = user.app_and_env().await?;
    let component = user
        .component(&env.id, "golem_it_host_api_tests_release")
        .name("golem-it:host-api-tests")
        .store()
        .await?;

    let (fork_phantom_id_tx, fork_phantom_id_rx) = tokio::sync::oneshot::channel::<String>();
    let fork_phantom_id_tx = Arc::new(Mutex::new(Some(fork_phantom_id_tx)));

    let (port_tx, port_rx) = tokio::sync::oneshot::channel::<u16>();
    let http_server = tokio::spawn(
        async move {
            let route = Router::new()
                .route(
                    "/fork-test/step1/:name/:input",
                    get(move |args: Path<(String, String)>| async move {
                        Json(format!("{}-{}", args.0 .0, args.0 .1))
                    }),
                )
                .route(
                    "/fork-test/step2/:name/:fork/:phantom_id",
                    get(move |args: Path<(String, String, String)>| {
                        let fork_phantom_id_tx = fork_phantom_id_tx.clone();
                        async move {
                            if let Some(fork_phantom_id_tx) =
                                fork_phantom_id_tx.lock().unwrap().take()
                            {
                                fork_phantom_id_tx.send(args.2.clone()).unwrap();
                            }
                            Json(format!("{}-{}-{}", args.0 .0, args.0 .1, args.0 .2))
                        }
                    }),
                );

            let listener =
                tokio::net::TcpListener::bind("0.0.0.0:0".parse::<SocketAddr>().unwrap())
                    .await
                    .unwrap();

            port_tx.send(listener.local_addr().unwrap().port()).unwrap();
            axum::serve(listener, route).await.unwrap();
        }
        .in_current_span(),
    );

    let port = port_rx.await.unwrap();
    let mut env = HashMap::new();
    env.insert("PORT".to_string(), port.to_string());

    info!("Using environment: {:?}", env);

    let source_agent_id = agent_id!("golem-host-api", "source-worker");
    let source_worker_id = user
        .start_agent_with(&component.id, source_agent_id.clone(), env, vec![])
        .await?;

    user.log_output(&source_worker_id).await?;

    let idempotency_key = IdempotencyKey::fresh();
    let source_result = user
        .invoke_and_await_agent_with_key(
            &component.id,
            &source_agent_id,
            &idempotency_key,
            "fork_test",
            data_value!("hello"),
        )
        .await?
        .into_return_value()
        .expect("Expected return value");

    let forked_phantom_id = fork_phantom_id_rx.await.unwrap();
    let forked_phantom_uuid: Uuid = forked_phantom_id.parse().expect("Expected valid UUID");

    let target_agent_id = phantom_agent_id!("golem-host-api", forked_phantom_uuid, "source-worker");
    let target_worker_id = WorkerId {
        component_id: component.id,
        worker_name: target_agent_id.to_string(),
    };
    let target_result = user
        .invoke_and_await_agent_with_key(
            &component.id,
            &target_agent_id,
            &idempotency_key,
            "fork_test",
            data_value!("hello"),
        )
        .await?
        .into_return_value()
        .expect("Expected return value");

    http_server.abort();

    let source_name = source_agent_id.to_string();
    assert_eq!(
        source_result,
        Value::String(format!(
            "{source_name}-hello::{source_name}-original-{forked_phantom_id}"
        ))
    );
    assert_eq!(
        target_result,
        Value::String(format!(
            "{source_name}-hello::{}-forked-{forked_phantom_id}",
            target_worker_id.worker_name
        ))
    );

    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout(120000)]
async fn fork_and_sync_with_promise(
    deps: &EnvBasedTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let (_, env) = user.app_and_env().await?;
    let component = user
        .component(&env.id, "golem_it_agent_promise")
        .store()
        .await?;

    let uuid = Uuid::new_v4();
    let promise_agent_id = agent_id!("promise-agent", uuid.to_string());
    let _worker = user
        .start_agent(&component.id, promise_agent_id.clone())
        .await?;

    let result1 = user
        .invoke_and_await_agent(
            &component.id,
            &promise_agent_id,
            "forkAndSyncWithPromise",
            data_value!(),
        )
        .await?;

    assert_eq!(
        result1.into_return_value(),
        Some(Value::String("Hello from forked agent!".to_string()))
    );

    let result2 = user
        .invoke_and_await_agent(
            &component.id,
            &promise_agent_id,
            "forkAndSyncWithPromise",
            data_value!(),
        )
        .await?;

    assert_eq!(
        result2.into_return_value(),
        Some(Value::String("Hello from forked agent!".to_string()))
    );

    Ok(())
}
