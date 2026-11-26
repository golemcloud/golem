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

use axum::extract::Path;
use axum::routing::get;
use axum::{Json, Router};
use golem_common::model::agent::OwnedAgentId;
use golem_common::model::oplog::{
    OplogIndex, PublicAgentInvocation, PublicAgentInvocationResult, PublicOplogEntry,
    PublicOplogEntryWithIndex,
};
use golem_common::model::{AgentId, AgentStatus, IdempotencyKey};
use golem_common::schema::SchemaValue;
use golem_common::{agent_key, data_value};
use golem_test_framework::config::{EnvBasedTestDependencies, TestDependencies};
use golem_test_framework::dsl::{TestDsl, TestDslExtended};
use integration_tests::agents::{agent_type_id, invoke_and_await_agent_by_id, keyed_agent_id};
use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use test_r::{inherit_test_dep, test, timeout};
use tracing::{Instrument, info};
use uuid::Uuid;

inherit_test_dep!(Tracing);
inherit_test_dep!(EnvBasedTestDependencies);

#[test]
#[tracing::instrument]
#[timeout("4m")]
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

    let source_agent_id = agent_key!("HttpClient2");
    let agent_id = user
        .start_agent_with(&component.id, source_agent_id.clone(), env, Vec::new())
        .await?;

    user.log_output(&agent_id).await?;

    user.invoke_agent(
        &component,
        &source_agent_id,
        "start_polling",
        data_value!("first"),
    )
    .await?;

    user.wait_for_status(&agent_id, AgentStatus::Running, Duration::from_secs(10))
        .await?;

    user.interrupt(&agent_id).await?;

    let oplog = user.get_oplog(&agent_id, OplogIndex::INITIAL).await?;

    let last_index = OplogIndex::from_u64(oplog.len() as u64);

    let target_agent_id = user.fork_worker(&agent_id, last_index).await?;

    {
        let mut response = response.lock().unwrap();
        *response = "first".to_string();
    }

    user.wait_for_status(&target_agent_id, AgentStatus::Idle, Duration::from_secs(10))
        .await?;

    let result = user
        .search_oplog(&target_agent_id, "Received first")
        .await?;

    http_server.abort();

    assert_eq!(result.len(), 1);
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("4m")]
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
    let parsed_source_agent_id = agent_key!("Repository", source_name.clone());
    let source_agent_id = user
        .start_agent(&component.id, parsed_source_agent_id.clone())
        .await?;

    user.invoke_and_await_agent(
        &component,
        &parsed_source_agent_id,
        "add",
        data_value!("G1002", "Mud Golem"),
    )
    .await?;

    let source_oplog = user
        .get_oplog(&source_agent_id, OplogIndex::INITIAL)
        .await?;

    let (idx, _) = source_oplog
        .iter()
        .enumerate()
        .rev()
        .find(|(_, entry)| matches!(&entry.entry, PublicOplogEntry::AgentInvocationStarted(_)))
        .expect("Expected AgentInvocationStarted in oplog");

    let oplog_index_of_function_invoked = OplogIndex::from_u64((idx + 1) as u64);

    let target_agent_id = user
        .fork_worker(&source_agent_id, oplog_index_of_function_invoked)
        .await?;

    user.wait_for_status(&target_agent_id, AgentStatus::Idle, Duration::from_secs(10))
        .await?;

    user.check_oplog_is_queryable(&source_agent_id).await?;
    user.check_oplog_is_queryable(&target_agent_id).await?;

    let total_invocations = user
        // Oplog search uses case-insensitive substring matching, and idempotency keys are
        // random hex UUIDs which can contain "add" as a substring, so the query must also
        // require the method-invocation marker to avoid spuriously matching the agent's
        // constructor (agent-initialization) entry.
        .search_oplog(
            &target_agent_id,
            "add AND agent-method-invocation AND NOT pending",
        )
        .await?;

    assert_eq!(total_invocations.len(), 1);

    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("4m")]
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

    let parsed_source_agent_id = agent_key!("HttpClient2");
    let source_agent_id = user
        .start_agent_with(
            &component.id,
            parsed_source_agent_id.clone(),
            env,
            Vec::new(),
        )
        .await?;

    user.log_output(&source_agent_id).await?;

    user.invoke_agent(
        &component,
        &parsed_source_agent_id,
        "start_polling",
        data_value!("first"),
    )
    .await?;

    user.wait_for_status(
        &source_agent_id,
        AgentStatus::Running,
        Duration::from_secs(10),
    )
    .await?;

    // The current oplog tail can lie between a call's End and its delivery marker, or between a
    // nested call's marker and its parent's End. Fork only at a delivery marker where every call
    // started by the polling invocation has reached its terminal.
    let wait_started = tokio::time::Instant::now();
    let last_delivered_index = loop {
        let oplog = user
            .get_oplog(&source_agent_id, OplogIndex::INITIAL)
            .await?;
        let invocation_started_index = oplog.iter().rev().find_map(|entry| {
            matches!(&entry.entry, PublicOplogEntry::AgentInvocationStarted(_))
                .then_some(entry.oplog_index)
        });
        let delivered_index = invocation_started_index.and_then(|invocation_started_index| {
            let mut open_calls = HashSet::new();
            let mut delivered_index = None;
            for entry in oplog
                .iter()
                .filter(|entry| entry.oplog_index > invocation_started_index)
            {
                match &entry.entry {
                    PublicOplogEntry::Start(_) => {
                        open_calls.insert(entry.oplog_index);
                    }
                    PublicOplogEntry::End(end) => {
                        open_calls.remove(&end.start_index);
                    }
                    PublicOplogEntry::Cancelled(cancelled) => {
                        open_calls.remove(&cancelled.start_index);
                    }
                    PublicOplogEntry::CompletionDelivered(_) if open_calls.is_empty() => {
                        delivered_index = Some(entry.oplog_index);
                    }
                    _ => {}
                }
            }
            delivered_index
        });

        if let Some(delivered_index) = delivered_index {
            break delivered_index;
        }
        if wait_started.elapsed() > Duration::from_secs(10) {
            anyhow::bail!("Timed out waiting for the polling call's delivered completion");
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    };

    let target_agent_id = user
        .fork_worker(&source_agent_id, last_delivered_index)
        .await?;

    {
        let mut response = response.lock().unwrap();
        *response = "first".to_string();
    }

    user.wait_for_status(&target_agent_id, AgentStatus::Idle, Duration::from_secs(20))
        .await?;

    user.wait_for_status(&source_agent_id, AgentStatus::Idle, Duration::from_secs(20))
        .await?;

    let target_result = user
        .search_oplog(&target_agent_id, "Received first")
        .await?;
    let source_result = user
        .search_oplog(&source_agent_id, "Received first")
        .await?;

    http_server.abort();

    assert_eq!(target_result.len(), 1);
    assert_eq!(source_result.len(), 1);

    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("4m")]
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
    let parsed_source_agent_id = agent_key!("Repository", source_name.clone());
    let source_agent_id = user
        .start_agent(&component.id, parsed_source_agent_id.clone())
        .await?;

    user.invoke_and_await_agent(
        &component,
        &parsed_source_agent_id,
        "add",
        data_value!("G1001", "Golem Cloud Subscription 1y"),
    )
    .await?;

    user.invoke_and_await_agent(
        &component,
        &parsed_source_agent_id,
        "add",
        data_value!("G1002", "Mud Golem"),
    )
    .await?;

    let source_oplog = settled_oplog(&user, &source_agent_id).await?;
    let cut = source_oplog.last().unwrap().oplog_index;

    let target_agent_id = user.fork_worker(&source_agent_id, cut).await?;

    invoke_and_await_agent_by_id(
        &user,
        &parsed_source_agent_id.agent_type,
        &target_agent_id,
        "add",
        data_value!("G1002", "Mud Golem"),
    )
    .await?;

    let original_contents = user
        .invoke_and_await_agent(&component, &parsed_source_agent_id, "list", data_value!())
        .await?;

    let forked_contents = invoke_and_await_agent_by_id(
        &user,
        &parsed_source_agent_id.agent_type,
        &target_agent_id,
        "list",
        data_value!(),
    )
    .await?;

    let original_value = original_contents
        .into_return_value()
        .expect("Expected return value");

    let forked_value = forked_contents
        .into_return_value()
        .expect("Expected return value");

    assert_eq!(
        original_value,
        SchemaValue::List {
            elements: vec![
                SchemaValue::Record {
                    fields: vec![
                        SchemaValue::String("G1001".to_string()),
                        SchemaValue::String("Golem Cloud Subscription 1y".to_string()),
                        SchemaValue::U64(1),
                    ]
                },
                SchemaValue::Record {
                    fields: vec![
                        SchemaValue::String("G1002".to_string()),
                        SchemaValue::String("Mud Golem".to_string()),
                        SchemaValue::U64(1),
                    ]
                },
            ]
        }
    );

    assert_eq!(
        forked_value,
        SchemaValue::List {
            elements: vec![
                SchemaValue::Record {
                    fields: vec![
                        SchemaValue::String("G1001".to_string()),
                        SchemaValue::String("Golem Cloud Subscription 1y".to_string()),
                        SchemaValue::U64(1),
                    ]
                },
                SchemaValue::Record {
                    fields: vec![
                        SchemaValue::String("G1002".to_string()),
                        SchemaValue::String("Mud Golem".to_string()),
                        SchemaValue::U64(2),
                    ]
                },
            ]
        }
    );

    let result1 = user
        .search_oplog(&target_agent_id, "G1002 AND NOT pending")
        .await?;

    let result2 = user
        .search_oplog(&target_agent_id, "G1001 AND NOT pending")
        .await?;

    assert!(!result1.is_empty());
    assert!(!result2.is_empty());
    assert!(result1.len() > result2.len());

    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("4m")]
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
    let parsed_source_agent_id = agent_key!("Repository", source_name.clone());
    let source_agent_id = user
        .start_agent(&component.id, parsed_source_agent_id.clone())
        .await?;

    user.invoke_and_await_agent(
        &component,
        &parsed_source_agent_id,
        "add",
        data_value!("G1001", "Golem Cloud Subscription 1y"),
    )
    .await?;

    let error = user
        .fork_worker(&source_agent_id, OplogIndex::INITIAL)
        .await
        .unwrap_err()
        .to_string();

    assert!(error.contains("oplog_index_cut_off must be at least 2"));

    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("4m")]
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
    let parsed_source_agent_id = agent_key!("Repository", source_name.clone());
    let type_id = agent_type_id(&user, &component, &parsed_source_agent_id.agent_type).await?;
    let source_agent_id = keyed_agent_id(&component, type_id, &parsed_source_agent_id)?;

    let error = user
        .fork_worker(&source_agent_id, OplogIndex::from_u64(14))
        .await
        .unwrap_err()
        .to_string();

    assert!(
        error.contains("AGENT_NOT_FOUND")
            && error.contains("Worker not found")
            && error.contains(&source_agent_id.agent_id.to_string()),
        "actual error: {error}"
    );
    Ok(())
}

// Divergence possibility is mainly respect to environment variables referring to worker-ids.
// Fork shouldn't change the original environment variable values of the source worker
// stored in oplog until cut off
#[test]
#[tracing::instrument]
#[timeout("4m")]
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
    let parsed_source_agent_id = agent_key!("Environment", source_name.clone());
    let source_agent_id = user
        .start_agent(&component.id, parsed_source_agent_id.clone())
        .await?;

    let source_result = user
        .invoke_and_await_agent(
            &component,
            &parsed_source_agent_id,
            "get_environment",
            data_value!(),
        )
        .await?;

    let source_agent_name = source_agent_id.to_string();

    let oplog = settled_oplog(&user, &source_agent_id).await?;
    let cut = oplog.last().unwrap().oplog_index;

    // We fork the worker post the completion and see if oplog corresponding to environment value
    // has the same value as the source worker. As far as the fork cut off point is post the
    // completion, there shouldn't be any divergence for worker information even if forked
    // worker ID is different from the source worker ID
    let target_agent_id = user.fork_worker(&source_agent_id, cut).await?;

    // The fork retains every entry of the inclusive prefix, and fork provenance is appended after
    // it. The fork rewrites the target's identity in `Create` and its wallet pins, so the kinds of
    // the entries are compared, and the retained cut entry is compared in full.
    let forked_oplog = user
        .get_oplog(&target_agent_id, OplogIndex::INITIAL)
        .await?;
    let forked_prefix = forked_oplog
        .iter()
        .take_while(|entry| entry.oplog_index <= cut)
        .collect::<Vec<_>>();
    assert_eq!(
        oplog
            .iter()
            .map(|entry| (entry.oplog_index, std::mem::discriminant(&entry.entry)))
            .collect::<Vec<_>>(),
        forked_prefix
            .iter()
            .map(|entry| (entry.oplog_index, std::mem::discriminant(&entry.entry)))
            .collect::<Vec<_>>(),
        "Forked worker oplog should retain every entry of the source until cut off"
    );
    assert_eq!(
        oplog.last().unwrap().entry,
        forked_prefix.last().unwrap().entry,
        "Forked worker oplog should have the same entries as source until cut off"
    );

    // Also verify the result contains the source worker's identity
    let result_str = format!("{:?}", source_result);
    assert!(
        result_str.contains(&source_name),
        "Environment should contain source worker name {source_agent_name}, got: {result_str}"
    );

    Ok(())
}

/// The oplog up to the `AgentInvocationSettled` that closes the last invocation. A method's
/// result is returned at `AgentInvocationFinished`; the invocation's background work then drains
/// and the agent becomes idle at its `AgentInvocationSettled`.
async fn settled_oplog(
    user: &impl TestDsl,
    agent_id: &OwnedAgentId,
) -> anyhow::Result<Vec<PublicOplogEntryWithIndex>> {
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    loop {
        let oplog = user.get_oplog(agent_id, OplogIndex::INITIAL).await?;
        let finished = oplog
            .iter()
            .rev()
            .find_map(|entry| match &entry.entry {
                PublicOplogEntry::AgentInvocationFinished(params) => {
                    Some(params.invocation_id.clone())
                }
                _ => None,
            })
            .expect("Expected an AgentInvocationFinished in the source oplog");
        if matches!(
            &oplog.last().unwrap().entry,
            PublicOplogEntry::AgentInvocationSettled(params) if params.invocation_id == finished
        ) {
            return Ok(oplog);
        }
        if std::time::Instant::now() >= deadline {
            anyhow::bail!(
                "The last invocation did not settle as the last entry: {:?}",
                oplog.last().unwrap()
            );
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
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
#[timeout("4m")]
async fn fork_self(deps: &EnvBasedTestDependencies, _tracing: &Tracing) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let (_, env) = user.app_and_env().await?;
    let component = user
        .component(&env.id, "golem_it_host_api_tests_release")
        .name("golem-it:host-api-tests")
        .store()
        .await?;

    let (forked_agent_id_tx, forked_agent_id_rx) = tokio::sync::oneshot::channel::<String>();
    let forked_agent_id_tx = Arc::new(Mutex::new(Some(forked_agent_id_tx)));

    let (port_tx, port_rx) = tokio::sync::oneshot::channel::<u16>();
    let http_server = tokio::spawn(
        async move {
            let route = Router::new()
                .route(
                    "/fork-test/step1/{name}/{input}",
                    get(move |args: Path<(String, String)>| async move {
                        Json(format!("{}-{}", args.0.0, args.0.1))
                    }),
                )
                .route(
                    "/fork-test/step2/{name}/{fork}/{forked_agent_id}",
                    get(move |args: Path<(String, String, String)>| {
                        let forked_agent_id_tx = forked_agent_id_tx.clone();
                        async move {
                            if let Some(forked_agent_id_tx) =
                                forked_agent_id_tx.lock().unwrap().take()
                            {
                                forked_agent_id_tx.send(args.2.clone()).unwrap();
                            }
                            Json(format!("{}-{}-{}", args.0.0, args.0.1, args.0.2))
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

    let parsed_source_agent_id = agent_key!("GolemHostApi", "source-worker");
    let source_agent_id = user
        .start_agent_with(
            &component.id,
            parsed_source_agent_id.clone(),
            env,
            Vec::new(),
        )
        .await?;

    user.log_output(&source_agent_id).await?;

    let idempotency_key = IdempotencyKey::fresh();
    let source_result = user
        .invoke_and_await_agent_with_key(
            &component,
            &parsed_source_agent_id,
            &idempotency_key,
            "fork_test",
            data_value!("hello"),
        )
        .await?
        .into_return_value()
        .expect("Expected return value");

    let forked_agent_id = forked_agent_id_rx.await.unwrap();
    let forked_agent_uuid: Uuid = forked_agent_id.parse().expect("Expected valid UUID");

    let target_agent_id =
        OwnedAgentId::new(source_agent_id.environment_id, &AgentId(forked_agent_uuid));
    let source_oplog = user
        .get_oplog(&source_agent_id, OplogIndex::INITIAL)
        .await?;
    let source_invocation_id = source_oplog
        .iter()
        .find_map(|entry| match &entry.entry {
            PublicOplogEntry::AgentInvocationFinished(finished)
                if finished.method_name.as_deref() == Some("fork_test") =>
            {
                Some(finished.invocation_id)
            }
            _ => None,
        })
        .expect("Expected the source fork_test invocation to finish");

    // The fork resumes the copied invocation. Submitting the external key again would
    // create a new invocation because the target has a different fingerprint.
    let target_oplog = tokio::time::timeout(Duration::from_secs(60), async {
        loop {
            let oplog = user
                .get_oplog(&target_agent_id, OplogIndex::INITIAL)
                .await?;
            if oplog.iter().any(|entry| {
                matches!(&entry.entry,
                    PublicOplogEntry::AgentInvocationFinished(finished)
                        if finished.invocation_id == source_invocation_id)
            }) {
                break anyhow::Ok(oplog);
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    })
    .await
    .map_err(|_| anyhow::anyhow!("The copied fork_test invocation did not finish"))??;

    let target_starts: Vec<_> = target_oplog
        .iter()
        .filter_map(|entry| match &entry.entry {
            PublicOplogEntry::AgentInvocationStarted(started) => match &started.invocation {
                PublicAgentInvocation::AgentMethodInvocation(method)
                    if method.method_name == "fork_test" =>
                {
                    Some(method.invocation_id)
                }
                _ => None,
            },
            _ => None,
        })
        .collect();
    assert_eq!(target_starts, vec![source_invocation_id]);
    let target_finishes: Vec<_> = target_oplog
        .iter()
        .filter_map(|entry| match &entry.entry {
            PublicOplogEntry::AgentInvocationFinished(finished)
                if finished.method_name.as_deref() == Some("fork_test") =>
            {
                Some(finished)
            }
            _ => None,
        })
        .collect();
    assert_eq!(target_finishes.len(), 1);
    assert_eq!(target_finishes[0].invocation_id, source_invocation_id);
    let PublicAgentInvocationResult::AgentMethod(target_result) = &target_finishes[0].result else {
        panic!("Expected a fork_test method result");
    };

    http_server.abort();

    let source_name = source_agent_id.agent_id.to_string();
    assert_eq!(
        source_result,
        SchemaValue::String(format!(
            "{source_name}-hello::{source_name}-original-{forked_agent_id}"
        ))
    );
    assert_eq!(
        target_result.output.value(),
        &SchemaValue::String(format!(
            "{source_name}-hello::{}-forked-{forked_agent_id}",
            target_agent_id.agent_id
        ))
    );

    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("8m")]
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
    let promise_agent_id = agent_key!("PromiseAgent", uuid.to_string());
    let _worker = user
        .start_agent(&component.id, promise_agent_id.clone())
        .await?;

    let result1 = user
        .invoke_and_await_agent(
            &component,
            &promise_agent_id,
            "forkAndSyncWithPromise",
            data_value!(),
        )
        .await?;

    assert_eq!(
        result1.into_return_value(),
        Some(SchemaValue::String("Hello from forked agent!".to_string()))
    );

    let result2 = user
        .invoke_and_await_agent(
            &component,
            &promise_agent_id,
            "forkAndSyncWithPromise",
            data_value!(),
        )
        .await?;

    assert_eq!(
        result2.into_return_value(),
        Some(SchemaValue::String("Hello from forked agent!".to_string()))
    );

    Ok(())
}
