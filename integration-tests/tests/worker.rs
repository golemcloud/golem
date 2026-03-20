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
use anyhow::anyhow;
use axum::extract::Query;
use axum::http::HeaderMap;
use axum::routing::{get, post};
use axum::Router;
use bytes::Bytes;
use futures_concurrency::future::Join;
use golem_api_grpc::proto::golem::worker::{log_event, LogEvent};
use golem_client::api::RegistryServiceClient;
use golem_common::model::account::{AccountRevision, AccountSetPlan};
use golem_common::model::component::{ComponentFilePath, ComponentFilePermissions, ComponentId};
use golem_common::model::oplog::public_oplog_entry::AgentInvocationStartedParams;
use golem_common::model::oplog::{OplogIndex, PublicOplogEntry};
use golem_common::model::worker::{FlatComponentFileSystemNode, FlatComponentFileSystemNodeKind};
use golem_common::model::{
    AgentFilter, AgentId, AgentStatus, FilterComparator, IdempotencyKey, PromiseId, ScanCursor,
    StringFilterComparator,
};
use golem_common::{agent_id, data_value, phantom_agent_id};
use golem_test_framework::config::{EnvBasedTestDependencies, TestDependencies};
use golem_test_framework::dsl::{update_counts, TestDsl, TestDslExtended, WorkerLogEventStream};
use golem_test_framework::model::IFSEntry;
use golem_wasm::analysis::analysed_type;
use golem_wasm::{FromValue, IntoValueAndType, Record, UuidRecord, Value, ValueAndType};
use pretty_assertions::assert_eq;
use rand::seq::IteratorRandom;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use test_r::{inherit_test_dep, test, timeout};
use tokio::time::sleep;
use tracing::{info, Instrument};
use uuid::Uuid;

inherit_test_dep!(Tracing);
inherit_test_dep!(EnvBasedTestDependencies);

#[test]
#[tracing::instrument]
#[timeout("4m")]
async fn dynamic_worker_creation(
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
    let agent_id = agent_id!("Environment", "dynamic-worker-creation-1");
    let _agent_id = user.start_agent(&component.id, agent_id.clone()).await?;

    let args = user
        .invoke_and_await_agent(&component, &agent_id, "get_arguments", data_value!())
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected return value"))?;

    let env = user
        .invoke_and_await_agent(&component, &agent_id, "get_environment", data_value!())
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected return value"))?;

    let agent_name = agent_id.to_string();
    assert_eq!(args, Value::Result(Ok(Some(Box::new(Value::List(vec![]))))));
    assert_eq!(
        env,
        Value::Result(Ok(Some(Box::new(Value::List(vec![
            Value::Tuple(vec![
                Value::String("GOLEM_AGENT_ID".to_string()),
                Value::String(agent_name.clone())
            ]),
            Value::Tuple(vec![
                Value::String("GOLEM_WORKER_NAME".to_string()),
                Value::String(agent_name)
            ]),
            Value::Tuple(vec![
                Value::String("GOLEM_COMPONENT_ID".to_string()),
                Value::String(format!("{}", component.id))
            ]),
            Value::Tuple(vec![
                Value::String("GOLEM_COMPONENT_REVISION".to_string()),
                Value::String("0".to_string())
            ]),
            Value::Tuple(vec![
                Value::String("GOLEM_AGENT_TYPE".to_string()),
                Value::String("Environment".to_string()),
            ])
        ])))))
    );

    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("4m")]
async fn counter_resource_test_1(
    deps: &EnvBasedTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let (_, env) = user.app_and_env().await?;
    let component = user
        .component(&env.id, "golem_it_agent_rpc_rust_release")
        .name("golem-it:agent-rpc-rust")
        .unique()
        .store()
        .await?;

    let parsed_agent_id = agent_id!("RpcCounter", "counter1");
    let agent_id = user
        .start_agent(&component.id, parsed_agent_id.clone())
        .await?;
    user.log_output(&agent_id).await?;

    user.invoke_and_await_agent(&component, &parsed_agent_id, "inc_by", data_value!(5u64))
        .await?;

    let result = user
        .invoke_and_await_agent(&component, &parsed_agent_id, "get_value", data_value!())
        .await?;

    let result_value = result.into_return_value().expect("Expected a return value");

    assert_eq!(result_value, Value::U64(5));

    user.check_oplog_is_queryable(&agent_id).await?;
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("4m")]
async fn counter_resource_test_1_json(
    deps: &EnvBasedTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let (_, env) = user.app_and_env().await?;
    let component = user
        .component(&env.id, "golem_it_agent_rpc_rust_release")
        .name("golem-it:agent-rpc-rust")
        .unique()
        .store()
        .await?;

    let parsed_agent_id = agent_id!("RpcCounter", "counter1j");
    let agent_id = user
        .start_agent(&component.id, parsed_agent_id.clone())
        .await?;
    user.log_output(&agent_id).await?;

    user.invoke_and_await_agent(&component, &parsed_agent_id, "inc_by", data_value!(5u64))
        .await?;

    let result = user
        .invoke_and_await_agent(&component, &parsed_agent_id, "get_value", data_value!())
        .await?;

    let result_value = result.into_return_value().expect("Expected a return value");

    assert_eq!(result_value, Value::U64(5));

    user.check_oplog_is_queryable(&agent_id).await?;
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("4m")]
async fn shopping_cart_example(
    deps: &EnvBasedTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let (_, env) = user.app_and_env().await?;
    let component = user
        .component(&env.id, "it_agent_counters_release")
        .name("it:agent-counters")
        .unique()
        .store()
        .await?;
    let repo_id = agent_id!("Repository", "shopping-cart-1");
    let agent_id = user.start_agent(&component.id, repo_id.clone()).await?;
    user.log_output(&agent_id).await?;

    user.invoke_and_await_agent(
        &component,
        &repo_id,
        "add",
        data_value!("G1000", "Golem T-Shirt M"),
    )
    .await?;

    user.invoke_and_await_agent(
        &component,
        &repo_id,
        "add",
        data_value!("G1001", "Golem Cloud Subscription 1y"),
    )
    .await?;

    user.invoke_and_await_agent(
        &component,
        &repo_id,
        "add",
        data_value!("G1002", "Mud Golem"),
    )
    .await?;

    user.invoke_and_await_agent(
        &component,
        &repo_id,
        "add",
        data_value!("G1002", "Mud Golem"),
    )
    .await?;

    let contents = user
        .invoke_and_await_agent(&component, &repo_id, "list", data_value!())
        .await?;

    let contents_value = contents
        .into_return_value()
        .expect("Expected a single return value");

    assert_eq!(
        contents_value,
        Value::List(vec![
            Value::Record(vec![
                Value::String("G1000".to_string()),
                Value::String("Golem T-Shirt M".to_string()),
                Value::U64(1),
            ]),
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

    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("4m")]
async fn rust_rpc_with_payload(
    deps: &EnvBasedTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let (_, env) = user.app_and_env().await?;

    let component = user
        .component(&env.id, "golem_it_agent_rpc_rust_release")
        .name("golem-it:agent-rpc-rust")
        .store()
        .await?;

    let parent_agent_id = agent_id!("RustParent", "rust_rpc_with_payload");
    let parent = user
        .start_agent(&component.id, parent_agent_id.clone())
        .await?;

    user.log_output(&parent).await?;

    let spawn_result = user
        .invoke_and_await_agent(
            &component,
            &parent_agent_id,
            "spawn_child",
            data_value!("hello world"),
        )
        .await?;

    let uuid_as_value = spawn_result
        .into_return_value()
        .expect("Expected a single return value");

    let uuid = UuidRecord::from_value(uuid_as_value.clone()).expect("UUID expected");

    let child_agent_id = agent_id!("RustChild", uuid);

    let get_result = user
        .invoke_and_await_agent(&component, &child_agent_id, "get", data_value!())
        .await?;

    let option_payload_as_value = get_result
        .into_return_value()
        .expect("Expected a single return value");

    user.check_oplog_is_queryable(&parent).await?;

    assert_eq!(
        option_payload_as_value,
        Value::Option(Some(Box::new(Value::Record(vec![
            Value::String("hello world".to_string()),
            uuid_as_value.clone(),
            Value::Enum(0)
        ]))))
    );
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("4m")]
async fn get_workers(deps: &EnvBasedTestDependencies, _tracing: &Tracing) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let (_, env) = user.app_and_env().await?;
    let component = user
        .component(&env.id, "it_agent_counters_release")
        .name("it:agent-counters")
        .store()
        .await?;

    let workers_count = 150;
    let mut agent_ids = HashSet::new();
    let mut agent_map: Vec<(AgentId, golem_common::model::agent::ParsedAgentId)> = Vec::new();

    for i in 0..workers_count {
        let aid = agent_id!("Repository", format!("gw{i}"));
        let agent_id = user.start_agent(&component.id, aid.clone()).await?;
        agent_ids.insert(agent_id.clone());
        agent_map.push((agent_id, aid));
    }

    let check_indices: Vec<usize> =
        (0..workers_count).choose_multiple(&mut rand::rng(), workers_count / 10);

    for &idx in &check_indices {
        let (ref agent_id, ref aid) = agent_map[idx];

        user.invoke_and_await_agent(&component, aid, "list", data_value!())
            .await?;

        let (cursor, values) = user
            .get_workers_metadata(
                &component.id,
                Some(
                    AgentFilter::new_name(StringFilterComparator::Equal, agent_id.agent_id.clone())
                        .and(
                            AgentFilter::new_status(FilterComparator::Equal, AgentStatus::Idle).or(
                                AgentFilter::new_status(
                                    FilterComparator::Equal,
                                    AgentStatus::Running,
                                ),
                            ),
                        ),
                ),
                ScanCursor::default(),
                20,
                true,
            )
            .await?;

        assert_eq!(values.len(), 1);
        assert!(cursor.is_none());

        let ids: HashSet<AgentId> = values.into_iter().map(|v| v.agent_id).collect();

        assert!(ids.contains(agent_id));
    }

    let mut found_agent_ids = HashSet::new();
    let mut cursor = Some(ScanCursor::default());

    let count = workers_count / 5;

    let filter = Some(AgentFilter::new_name(
        StringFilterComparator::Like,
        "Repository(\"gw".to_string(),
    ));
    while found_agent_ids.len() < workers_count && cursor.is_some() {
        let (cursor1, values1) = user
            .get_workers_metadata(
                &component.id,
                filter.clone(),
                cursor.unwrap(),
                count as u64,
                true,
            )
            .await?;

        assert!(!values1.is_empty()); // Each page should contain at least one element, but it is not guaranteed that it has count elements

        let ids: HashSet<AgentId> = values1.into_iter().map(|v| v.agent_id).collect();
        found_agent_ids.extend(ids);

        cursor = cursor1;
    }

    assert!(found_agent_ids.eq(&agent_ids));

    if let Some(cursor) = cursor {
        let (_, values) = user
            .get_workers_metadata(&component.id, filter, cursor, workers_count as u64, true)
            .await?;
        assert_eq!(values.len(), 0);
    }

    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("5m")]
async fn get_running_workers(
    deps: &EnvBasedTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let (_, env) = user.app_and_env().await?;
    let component = user
        .component(&env.id, "golem_it_http_tests_release")
        .name("golem-it:http-tests")
        .unique()
        .store()
        .await?;

    let polling_agent_ids: Arc<Mutex<HashSet<AgentId>>> = Arc::new(Mutex::new(HashSet::new()));
    let polling_agent_ids_clone = polling_agent_ids.clone();

    let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await.unwrap();
    let host_http_port = listener.local_addr().unwrap().port();

    let response = Arc::new(Mutex::new("running".to_string()));
    let response_clone = response.clone();

    let http_server = tokio::spawn(async move {
        let route = Router::new().route(
            "/poll",
            get(move |query: Query<HashMap<String, String>>| {
                async move {
                    let component_id = query.get("component_id");
                    let agent_name = query.get("worker_name");
                    info!("Poll request received: component_id={component_id:?}, worker_name={agent_name:?}, all_keys={:?}", query.0.keys().collect::<Vec<_>>());
                    if let (Some(component_id), Some(agent_name)) = (component_id, agent_name) {
                        let component_id: ComponentId = component_id.as_str().try_into().unwrap();
                        let agent_id = AgentId {
                            component_id,
                            agent_id: agent_name.clone(),
                        };
                        let mut ids = polling_agent_ids_clone.lock().unwrap();
                        ids.insert(agent_id.clone());
                        info!("Registered polling agent: {agent_id:?}, total: {}", ids.len());
                    }
                    response_clone.lock().unwrap().clone()
                }
                .in_current_span()
            }),
        );

        axum::serve(listener, route).await.unwrap();
    });

    let mut env = HashMap::new();
    env.insert("PORT".to_string(), host_http_port.to_string());

    let workers_count = 15;
    let mut workers: Vec<(AgentId, golem_common::model::agent::ParsedAgentId)> = Vec::new();

    for _i in 0..workers_count {
        let aid = phantom_agent_id!("HttpClient2", Uuid::new_v4());
        let agent_id = user
            .start_agent_with(
                &component.id,
                aid.clone(),
                env.clone(),
                HashMap::new(),
                Vec::new(),
            )
            .await?;

        workers.push((agent_id, aid));
    }

    let agent_ids: HashSet<AgentId> = workers.iter().map(|(w, _)| w.clone()).collect();

    for (agent_id, aid) in &workers {
        user.invoke_agent(&component, aid, "start_polling", data_value!("stop"))
            .await?;

        user.wait_for_status(agent_id, AgentStatus::Running, Duration::from_secs(10))
            .await?;
    }

    let start = tokio::time::Instant::now();
    let polling_deadline = Duration::from_secs(60);
    loop {
        let ids = polling_agent_ids.lock().unwrap().clone();

        if agent_ids.eq(&ids) {
            info!("All the spawned workers have polled the server at least once.");
            break;
        }
        if start.elapsed() > polling_deadline {
            let missing: Vec<_> = agent_ids.difference(&ids).collect();
            return Err(anyhow!(
                "Timed out waiting for all spawned workers to poll. Only {}/{} polled. Missing: {:?}",
                ids.len(),
                workers_count,
                missing
            ));
        }

        sleep(Duration::from_secs(1)).await;
    }

    // Testing looking for a single worker - with retry for eventual consistency
    info!("Phase 1: Looking for a single worker by name");
    let single_agent_name = agent_ids.iter().next().unwrap().agent_id.clone();
    let start = tokio::time::Instant::now();
    let enum_deadline = Duration::from_secs(30);
    let single_result = loop {
        let mut cursor = ScanCursor::default();
        let mut enum_results = Vec::new();
        loop {
            let (next_cursor, values) = user
                .get_workers_metadata(
                    &component.id,
                    Some(AgentFilter::new_name(
                        StringFilterComparator::Equal,
                        single_agent_name.clone(),
                    )),
                    cursor,
                    1,
                    true,
                )
                .await?;
            enum_results.extend(values);
            if let Some(next_cursor) = next_cursor {
                cursor = next_cursor;
            } else {
                break;
            }
        }
        if enum_results.len() == 1 {
            break enum_results;
        }
        if start.elapsed() > enum_deadline {
            return Err(anyhow!(
                "Timed out waiting for single worker enumeration. Got {} results, expected 1",
                enum_results.len()
            ));
        }
        sleep(Duration::from_millis(200)).await;
    };
    assert_eq!(single_result.len(), 1);
    info!(
        "Phase 1 complete: found single worker in {:?}",
        start.elapsed()
    );

    // Testing looking for all the workers - with retry for eventual consistency
    info!("Phase 2: Looking for all {workers_count} workers");
    let start = tokio::time::Instant::now();
    loop {
        let mut cursor = ScanCursor::default();
        let mut enum_results = Vec::new();
        loop {
            let (next_cursor, values) = user
                .get_workers_metadata(&component.id, None, cursor, workers_count, true)
                .await?;
            enum_results.extend(values);
            if let Some(next_cursor) = next_cursor {
                cursor = next_cursor;
            } else {
                break;
            }
        }
        let returned: HashSet<_> = enum_results.iter().map(|m| m.agent_id.clone()).collect();
        if agent_ids.is_subset(&returned) {
            break;
        }
        if start.elapsed() > enum_deadline {
            let missing: Vec<_> = agent_ids.difference(&returned).collect();
            return Err(anyhow!(
                "Timed out waiting for all workers to be enumerated. Got {}/{}, missing: {:?}",
                returned.len(),
                workers_count,
                missing
            ));
        }
        sleep(Duration::from_millis(200)).await;
    }
    info!(
        "Phase 2 complete: found all workers in {:?}",
        start.elapsed()
    );

    // Testing looking for running workers - with retry for eventual consistency
    info!("Phase 3: Looking for running workers");
    let start = tokio::time::Instant::now();
    loop {
        let mut cursor = ScanCursor::default();
        let mut enum_results = Vec::new();
        loop {
            let (next_cursor, values) = user
                .get_workers_metadata(
                    &component.id,
                    Some(AgentFilter::new_status(
                        FilterComparator::Equal,
                        AgentStatus::Running,
                    )),
                    cursor,
                    workers_count,
                    true,
                )
                .await?;
            enum_results.extend(values);
            if let Some(next_cursor) = next_cursor {
                cursor = next_cursor;
            } else {
                break;
            }
        }
        if !enum_results.is_empty() && enum_results.len() <= workers_count as usize {
            break;
        }
        if start.elapsed() > enum_deadline {
            return Err(anyhow!(
                "Timed out waiting for at least one running worker. Got {} running workers",
                enum_results.len()
            ));
        }
        sleep(Duration::from_millis(200)).await;
    }
    info!(
        "Phase 3 complete: found running workers in {:?}",
        start.elapsed()
    );

    info!("Phase 4: Sending stop signal to workers");
    *response.lock().unwrap() = "stop".to_string();

    // Wait for all workers to become Idle in parallel with a generous timeout
    info!(
        "Phase 5: Waiting for all {} workers to become Idle",
        agent_ids.len()
    );
    let idle_start = tokio::time::Instant::now();
    let idle_futs: Vec<_> = agent_ids
        .iter()
        .map(|agent_id| user.wait_for_status(agent_id, AgentStatus::Idle, Duration::from_secs(30)))
        .collect();
    let idle_results = idle_futs.join().await;
    for result in &idle_results {
        result
            .as_ref()
            .map_err(|e| anyhow!("Worker failed to become Idle: {e}"))?;
    }
    info!(
        "Phase 5 complete: all workers idle in {:?}",
        idle_start.elapsed()
    );

    // Delete workers after all are idle
    info!("Phase 6: Deleting {} workers", agent_ids.len());
    for agent_id in &agent_ids {
        user.delete_worker(agent_id).await?;
    }
    info!("Phase 6 complete: all workers deleted");

    http_server.abort();
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout(300000)]
async fn auto_update_on_idle(
    deps: &EnvBasedTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let (_, env) = user.app_and_env().await?;
    let component = user
        .component(&env.id, "it_agent_update_v1_release")
        .name("it:agent-update")
        .unique()
        .store()
        .await?;
    let parsed_agent_id = agent_id!("UpdateTest");
    let agent_id = user
        .start_agent(&component.id, parsed_agent_id.clone())
        .await?;
    user.log_output(&agent_id).await?;

    let updated_component = user
        .update_component(&component.id, "it_agent_update_v2_release")
        .await?;

    info!(
        "Updated component to version {}",
        updated_component.revision
    );

    user.auto_update_worker(&agent_id, updated_component.revision, false)
        .await?;

    let result = user
        .invoke_and_await_agent(&component, &parsed_agent_id, "f2", data_value!())
        .await?;

    info!("result: {result:?}");
    let metadata = user.get_worker_metadata(&agent_id).await?;

    // Expectation: the worker has no history so the update succeeds and then calling f2 returns
    // the current state which is 0
    assert_eq!(result, data_value!(0u64));
    assert_eq!(metadata.component_revision, updated_component.revision);
    assert_eq!(update_counts(&metadata), (0, 1, 0));
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout(300000)]
async fn auto_update_on_idle_via_host_function(
    deps: &EnvBasedTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let (_, env) = user.app_and_env().await?;
    let component = user
        .component(&env.id, "it_agent_update_v1_release")
        .name("it:agent-update")
        .unique()
        .store()
        .await?;
    let parsed_agent_id = agent_id!("UpdateTest");
    let agent_id = user
        .start_agent(&component.id, parsed_agent_id.clone())
        .await?;
    user.log_output(&agent_id).await?;

    let updated_component = user
        .update_component(&component.id, "it_agent_update_v2_release")
        .await?;

    info!(
        "Updated component to version {}",
        updated_component.revision
    );

    let host_api_component = user
        .component(&env.id, "golem_it_host_api_tests_release")
        .name("golem-it:host-api-tests")
        .store()
        .await?;
    let host_api_agent_id = agent_id!("GolemHostApi", "updater-1");
    let _host_api_agent_id = user
        .start_agent(&host_api_component.id, host_api_agent_id.clone())
        .await?;

    let (high_bits, low_bits) = agent_id.component_id.0.as_u64_pair();
    user.invoke_and_await_agent(
        &host_api_component,
        &host_api_agent_id,
        "update_worker",
        data_value!(
            Record(vec![
                (
                    "component_id",
                    Record(vec![(
                        "uuid",
                        Record(vec![
                            ("high_bits", high_bits.into_value_and_type()),
                            ("low_bits", low_bits.into_value_and_type()),
                        ])
                        .into_value_and_type(),
                    )])
                    .into_value_and_type(),
                ),
                (
                    "agent_id",
                    parsed_agent_id.to_string().into_value_and_type(),
                ),
            ])
            .into_value_and_type(),
            updated_component.revision.into_value_and_type(),
            ValueAndType {
                value: Value::Variant {
                    case_idx: 0,
                    case_value: None,
                },
                typ: analysed_type::variant(vec![
                    analysed_type::unit_case("automatic"),
                    analysed_type::unit_case("snapshot-based"),
                ]),
            },
        ),
    )
    .await?
    .into_return_value();

    let result = user
        .invoke_and_await_agent(&component, &parsed_agent_id, "f2", data_value!())
        .await?;

    let metadata = user.get_worker_metadata(&agent_id).await?;

    // Expectation: the worker has no history so the update succeeds and then calling f2 returns
    // the current state which is 0
    assert_eq!(result, data_value!(0u64));
    assert_eq!(metadata.component_revision, updated_component.revision);
    assert_eq!(update_counts(&metadata), (0, 1, 0));
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("4m")]
async fn get_oplog_1(deps: &EnvBasedTestDependencies, _tracing: &Tracing) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let (_, env) = user.app_and_env().await?;
    let component = user
        .component(&env.id, "golem_it_host_api_tests_release")
        .name("golem-it:host-api-tests")
        .store()
        .await?;

    let parsed_agent_id = agent_id!("GolemHostApi", "getoplog1");
    let agent_id = user
        .start_agent(&component.id, parsed_agent_id.clone())
        .await?;

    let idempotency_key1 = IdempotencyKey::fresh();
    let idempotency_key2 = IdempotencyKey::fresh();

    user.invoke_and_await_agent(
        &component,
        &parsed_agent_id,
        "generate_idempotency_keys",
        data_value!(),
    )
    .await?;

    user.invoke_and_await_agent_with_key(
        &component,
        &parsed_agent_id,
        &idempotency_key1,
        "generate_idempotency_keys",
        data_value!(),
    )
    .await?;

    user.invoke_and_await_agent_with_key(
        &component,
        &parsed_agent_id,
        &idempotency_key2,
        "generate_idempotency_keys",
        data_value!(),
    )
    .await?;

    let oplog = user.get_oplog(&agent_id, OplogIndex::INITIAL).await?;

    assert_eq!(oplog[0].oplog_index, OplogIndex::INITIAL);
    assert!(matches!(&oplog[0].entry, PublicOplogEntry::Create(_)));

    let invoke_count = oplog
        .iter()
        .filter(|entry| {
            matches!(&entry.entry, PublicOplogEntry::AgentInvocationStarted(
                AgentInvocationStartedParams { invocation: golem_common::model::oplog::PublicAgentInvocation::AgentMethodInvocation(params), .. }
            ) if params.method_name == "generate_idempotency_keys")
        })
        .count();
    assert!(
        invoke_count >= 3,
        "Expected at least 3 AgentInvocationStarted entries for golem:agent/guest.{{invoke}}, got {invoke_count}"
    );

    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("4m")]
async fn search_oplog_1(deps: &EnvBasedTestDependencies, _tracing: &Tracing) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let (_, env) = user.app_and_env().await?;
    let component = user
        .component(&env.id, "it_agent_counters_release")
        .name("it:agent-counters")
        .store()
        .await?;

    let repo_id = agent_id!("Repository", "searchoplog1");
    let agent_id = user.start_agent(&component.id, repo_id.clone()).await?;

    user.invoke_and_await_agent(
        &component,
        &repo_id,
        "add",
        data_value!("G1000", "Golem T-Shirt M"),
    )
    .await?;

    user.invoke_and_await_agent(
        &component,
        &repo_id,
        "add",
        data_value!("G1001", "Golem Cloud Subscription 1y"),
    )
    .await?;

    user.invoke_and_await_agent(
        &component,
        &repo_id,
        "add",
        data_value!("G1002", "Mud Golem"),
    )
    .await?;

    user.invoke_and_await_agent(
        &component,
        &repo_id,
        "add",
        data_value!("G1002", "Mud Golem"),
    )
    .await?;

    user.invoke_and_await_agent(&component, &repo_id, "list", data_value!())
        .await?;

    user.get_oplog(&agent_id, OplogIndex::INITIAL).await?;

    let result1 = user.search_oplog(&agent_id, "G1002").await?;

    let result2 = user.search_oplog(&agent_id, "imported-function").await?;

    let result3 = user.search_oplog(&agent_id, "G1001 OR G1000").await?;

    assert_eq!(result1.len(), 2, "G1002"); // TODO: this is temporarily not working because of using the dynamic invoke API and not having structured information in the oplog
    assert_eq!(result2.len(), 2, "imported-function");
    assert_eq!(result3.len(), 2, "id:G1001 OR id:G1000");

    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout(600000)]
async fn worker_recreation(
    deps: &EnvBasedTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let (_, env) = user.app_and_env().await?;
    let component = user
        .component(&env.id, "golem_it_agent_rpc_rust_release")
        .name("golem-it:agent-rpc-rust")
        .store()
        .await?;

    let parsed_agent_id = agent_id!("RpcCounter", "recreation");
    let agent_id = user
        .start_agent(&component.id, parsed_agent_id.clone())
        .await?;

    // Doing many requests, so parts of the oplog gets archived
    for _ in 1..=1200 {
        user.invoke_and_await_agent(&component, &parsed_agent_id, "inc_by", data_value!(1u64))
            .await?;
    }

    let result1 = user
        .invoke_and_await_agent(&component, &parsed_agent_id, "get_value", data_value!())
        .await?;

    tokio::time::sleep(Duration::from_secs(2)).await;

    user.delete_worker(&agent_id).await?;

    // Invoking again should create a new worker
    user.invoke_and_await_agent(&component, &parsed_agent_id, "inc_by", data_value!(1u64))
        .await?;

    let result2 = user
        .invoke_and_await_agent(&component, &parsed_agent_id, "get_value", data_value!())
        .await?;

    user.delete_worker(&agent_id).await?;

    // Also if we explicitly create a new one
    let _agent_id = user
        .start_agent(&component.id, parsed_agent_id.clone())
        .await?;

    let result3 = user
        .invoke_and_await_agent(&component, &parsed_agent_id, "get_value", data_value!())
        .await?;

    let result1_value = result1
        .into_return_value()
        .expect("Expected a return value");
    let result2_value = result2
        .into_return_value()
        .expect("Expected a return value");
    let result3_value = result3
        .into_return_value()
        .expect("Expected a return value");

    assert_eq!(result1_value, Value::U64(1200));
    assert_eq!(result2_value, Value::U64(1));
    assert_eq!(result3_value, Value::U64(0));

    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout(600000)]
async fn worker_use_initial_files(
    deps: &EnvBasedTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let (_, env) = user.app_and_env().await?;

    let component = user
        .component(&env.id, "it_initial_file_system_release")
        .name("golem-it:initial-file-system")
        .unique()
        .with_files(&[
            IFSEntry {
                source_path: PathBuf::from("initial-file-system/files/foo.txt"),
                target_path: ComponentFilePath::from_abs_str("/foo.txt").unwrap(),
                permissions: ComponentFilePermissions::ReadOnly,
            },
            IFSEntry {
                source_path: PathBuf::from("initial-file-system/files/baz.txt"),
                target_path: ComponentFilePath::from_abs_str("/bar/baz.txt").unwrap(),
                permissions: ComponentFilePermissions::ReadWrite,
            },
        ])
        .store()
        .await?;

    let mut env = HashMap::new();
    env.insert("RUST_BACKTRACE".to_string(), "full".to_string());
    let parsed_agent_id = agent_id!("FileReadWrite", "initial-file-read-write-1");
    let agent_id = user
        .start_agent_with(
            &component.id,
            parsed_agent_id.clone(),
            env,
            HashMap::new(),
            Vec::new(),
        )
        .await?;

    let result = user
        .invoke_and_await_agent(&component, &parsed_agent_id, "run", data_value!())
        .await?;

    user.check_oplog_is_queryable(&agent_id).await?;

    let result = result
        .into_return_value()
        .ok_or_else(|| anyhow!("expected return value"))?;

    assert_eq!(
        result,
        Value::Tuple(vec![
            Value::Option(Some(Box::new(Value::String("foo\n".to_string())))),
            Value::Option(None),
            Value::Option(None),
            Value::Option(Some(Box::new(Value::String("baz\n".to_string())))),
            Value::Option(Some(Box::new(Value::String("hello world".to_string())))),
        ])
    );

    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout(600000)]
async fn worker_list_files(
    deps: &EnvBasedTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let (_, env) = user.app_and_env().await?;

    let component = user
        .component(&env.id, "it_initial_file_system_release")
        .name("golem-it:initial-file-system")
        .unique()
        .with_files(&[
            IFSEntry {
                source_path: PathBuf::from("initial-file-system/files/foo.txt"),
                target_path: ComponentFilePath::from_abs_str("/foo.txt").unwrap(),
                permissions: ComponentFilePermissions::ReadOnly,
            },
            IFSEntry {
                source_path: PathBuf::from("initial-file-system/files/baz.txt"),
                target_path: ComponentFilePath::from_abs_str("/bar/baz.txt").unwrap(),
                permissions: ComponentFilePermissions::ReadWrite,
            },
            IFSEntry {
                source_path: PathBuf::from("initial-file-system/files/baz.txt"),
                target_path: ComponentFilePath::from_abs_str("/baz.txt").unwrap(),
                permissions: ComponentFilePermissions::ReadWrite,
            },
        ])
        .store()
        .await?;

    let agent_id = agent_id!("FileReadWrite", "worker-list-files-1");
    let agent_id = user.start_agent(&component.id, agent_id).await?;

    let result = user.get_file_system_node(&agent_id, "/").await?;

    let mut result = result
        .into_iter()
        .map(|e| FlatComponentFileSystemNode {
            last_modified: 0,
            ..e
        })
        .collect::<Vec<_>>();

    result.sort_by_key(|e| e.name.clone());

    assert_eq!(
        result,
        vec![
            FlatComponentFileSystemNode {
                name: "bar".to_string(),
                last_modified: 0,
                kind: FlatComponentFileSystemNodeKind::Directory,
                permissions: None,
                size: None
            },
            FlatComponentFileSystemNode {
                name: "baz.txt".to_string(),
                last_modified: 0,
                kind: FlatComponentFileSystemNodeKind::File,
                permissions: Some(ComponentFilePermissions::ReadWrite),
                size: Some(4),
            },
            FlatComponentFileSystemNode {
                name: "foo.txt".to_string(),
                last_modified: 0,
                kind: FlatComponentFileSystemNodeKind::File,
                permissions: Some(ComponentFilePermissions::ReadOnly),
                size: Some(4)
            },
        ]
    );

    let result = user.get_file_system_node(&agent_id, "/bar").await?;

    let mut result = result
        .into_iter()
        .map(|e| FlatComponentFileSystemNode {
            last_modified: 0,
            ..e
        })
        .collect::<Vec<_>>();

    result.sort_by_key(|e| e.name.clone());

    assert_eq!(
        result,
        vec![FlatComponentFileSystemNode {
            name: "baz.txt".to_string(),
            last_modified: 0,
            kind: FlatComponentFileSystemNodeKind::File,
            permissions: Some(ComponentFilePermissions::ReadWrite),
            size: Some(4),
        },]
    );

    let result = user.get_file_system_node(&agent_id, "/baz.txt").await?;

    let mut result = result
        .into_iter()
        .map(|e| FlatComponentFileSystemNode {
            last_modified: 0,
            ..e
        })
        .collect::<Vec<_>>();

    result.sort_by_key(|e| e.name.clone());

    assert_eq!(
        result,
        vec![FlatComponentFileSystemNode {
            name: "baz.txt".to_string(),
            last_modified: 0,
            kind: FlatComponentFileSystemNodeKind::File,
            permissions: Some(ComponentFilePermissions::ReadWrite),
            size: Some(4),
        },]
    );

    user.check_oplog_is_queryable(&agent_id).await?;

    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout(600000)]
async fn worker_read_files(
    deps: &EnvBasedTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let (_, env) = user.app_and_env().await?;

    let component = user
        .component(&env.id, "it_initial_file_system_release")
        .name("golem-it:initial-file-system")
        .unique()
        .with_files(&[
            IFSEntry {
                source_path: PathBuf::from("initial-file-system/files/foo.txt"),
                target_path: ComponentFilePath::from_abs_str("/foo.txt").unwrap(),
                permissions: ComponentFilePermissions::ReadOnly,
            },
            IFSEntry {
                source_path: PathBuf::from("initial-file-system/files/baz.txt"),
                target_path: ComponentFilePath::from_abs_str("/bar/baz.txt").unwrap(),
                permissions: ComponentFilePermissions::ReadWrite,
            },
        ])
        .store()
        .await?;

    let mut env = HashMap::new();
    env.insert("RUST_BACKTRACE".to_string(), "full".to_string());
    let parsed_agent_id = agent_id!("FileReadWrite", "initial-file-read-write-3");
    let agent_id = user
        .start_agent_with(
            &component.id,
            parsed_agent_id.clone(),
            env,
            HashMap::new(),
            Vec::new(),
        )
        .await?;

    // run the worker so it can update the files.
    let _ = user
        .invoke_and_await_agent(&component, &parsed_agent_id, "run", data_value!())
        .await?;

    let result1 = user.get_file_contents(&agent_id, "/foo.txt").await?;
    let result1 = std::str::from_utf8(&result1).unwrap();

    let result2 = user.get_file_contents(&agent_id, "/bar/baz.txt").await?;
    let result2 = std::str::from_utf8(&result2).unwrap();

    user.check_oplog_is_queryable(&agent_id).await?;

    assert_eq!(result1, "foo\n");
    assert_eq!(result2, "hello world");

    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout(600000)]
async fn worker_initial_files_after_automatic_worker_update(
    deps: &EnvBasedTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let (_, env) = user.app_and_env().await?;

    let component = user
        .component(&env.id, "it_initial_file_system_release")
        .name("golem-it:initial-file-system")
        .with_files(&[
            IFSEntry {
                source_path: PathBuf::from("initial-file-system/files/foo.txt"),
                target_path: ComponentFilePath::from_abs_str("/foo.txt").unwrap(),
                permissions: ComponentFilePermissions::ReadOnly,
            },
            IFSEntry {
                source_path: PathBuf::from("initial-file-system/files/baz.txt"),
                target_path: ComponentFilePath::from_abs_str("/bar/baz.txt").unwrap(),
                permissions: ComponentFilePermissions::ReadWrite,
            },
        ])
        .store()
        .await?;

    let parsed_agent_id = agent_id!("FileReadWrite", "initial-file-read-write-1");
    let agent_id = user
        .start_agent(&component.id, parsed_agent_id.clone())
        .await?;

    // run the worker so it can update the files.
    let _ = user
        .invoke_and_await_agent(&component, &parsed_agent_id, "run", data_value!())
        .await?;

    let updated_component = user
        .update_component_with_files(
            &component.id,
            "it_initial_file_system_release",
            vec![
                IFSEntry {
                    source_path: PathBuf::from("initial-file-system/files/foo.txt"),
                    target_path: ComponentFilePath::from_abs_str("/foo.txt").unwrap(),
                    permissions: ComponentFilePermissions::ReadOnly,
                },
                IFSEntry {
                    source_path: PathBuf::from("initial-file-system/files/baz.txt"),
                    target_path: ComponentFilePath::from_abs_str("/bar/baz.txt").unwrap(),
                    permissions: ComponentFilePermissions::ReadWrite,
                },
                IFSEntry {
                    source_path: PathBuf::from("initial-file-system/files/baz.txt"),
                    target_path: ComponentFilePath::from_abs_str("/baz.txt").unwrap(),
                    permissions: ComponentFilePermissions::ReadWrite,
                },
            ],
        )
        .await?;

    user.auto_update_worker(&agent_id, updated_component.revision, false)
        .await?;

    let result1 = user.get_file_contents(&agent_id, "/foo.txt").await?;
    let result1 = std::str::from_utf8(&result1).unwrap();

    let result2 = user.get_file_contents(&agent_id, "/bar/baz.txt").await?;
    let result2 = std::str::from_utf8(&result2).unwrap();

    let result3 = user.get_file_contents(&agent_id, "/baz.txt").await?;
    let result3 = std::str::from_utf8(&result3).unwrap();

    assert_eq!(result1, "foo\n");
    assert_eq!(result2, "hello world");
    assert_eq!(result3, "baz\n");

    Ok(())
}

/// Test resolving a component_id from the name.
#[test]
#[tracing::instrument]
async fn resolve_components_from_name(
    deps: &EnvBasedTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let (_, env) = user.app_and_env().await?;

    let counter_component = user
        .component(&env.id, "golem_it_agent_rpc_rust_release")
        .name("component-resolve-target")
        .store()
        .await?;

    let resolver_component = user
        .component(&env.id, "golem_it_host_api_tests_release")
        .name("golem-it:host-api-tests")
        .store()
        .await?;

    let target_agent_id = agent_id!("RpcCounter", "counter-1");
    user.start_agent(&counter_component.id, target_agent_id)
        .await?;

    let agent_id = agent_id!("GolemHostApi", "resolver-1");
    let _resolve_worker = user
        .start_agent(&resolver_component.id, agent_id.clone())
        .await?;

    let result = user
        .invoke_and_await_agent(
            &resolver_component,
            &agent_id,
            "resolve_component",
            data_value!(),
        )
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected return value"))?;

    assert_eq!(
        result,
        Value::Record(vec![
            Value::Bool(true),
            Value::Bool(true),
            Value::Bool(false),
        ])
    );

    Ok(())
}

#[test]
#[tracing::instrument]
async fn agent_promise_await(
    deps: &EnvBasedTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let (_, env) = user.app_and_env().await?;

    let component = user
        .component(&env.id, "golem_it_agent_promise")
        .name("golem-it:agent-promise")
        .store()
        .await?;

    let promise_agent_id = agent_id!("PromiseAgent", "name");
    let worker = user
        .start_agent(&component.id, promise_agent_id.clone())
        .await?;

    let result = user
        .invoke_and_await_agent(&component, &promise_agent_id, "getPromise", data_value!())
        .await?;

    let promise_id_vat = result
        .into_return_value_and_type()
        .ok_or_else(|| anyhow!("expected return value"))?;
    let promise_id =
        PromiseId::from_value(promise_id_vat.value.clone()).map_err(|e| anyhow!("{e}"))?;

    let task = {
        let executor_clone = user.clone();
        let agent_id_clone = promise_agent_id.clone();
        let component_clone = component.clone();
        tokio::spawn(
            async move {
                executor_clone
                    .invoke_and_await_agent(
                        &component_clone,
                        &agent_id_clone,
                        "awaitPromise",
                        data_value!(promise_id_vat),
                    )
                    .await
            }
            .in_current_span(),
        )
    };

    user.wait_for_status(&worker, AgentStatus::Suspended, Duration::from_secs(10))
        .await?;

    user.complete_promise(&promise_id, b"hello".to_vec())
        .await?;

    let result = task.await??;
    assert_eq!(
        result.into_return_value(),
        Some(Value::String("hello".to_string()))
    );

    Ok(())
}

#[test]
#[tracing::instrument]
async fn stream_high_volume_log_output(deps: &EnvBasedTestDependencies) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let (_, env) = user.app_and_env().await?;

    let component = user
        .component(&env.id, "golem_it_host_api_tests_release")
        .name("golem-it:host-api-tests")
        .store()
        .await?;

    let parsed_agent_id = agent_id!("Logging", "worker-1");
    let agent_id = user
        .start_agent(&component.id, parsed_agent_id.clone())
        .await?;

    let mut output_stream = user.make_worker_log_event_stream(&agent_id).await?;

    // simulate a slow consumer
    let output_consumer = async {
        loop {
            let event = output_stream.message().await.unwrap();
            if let Some(LogEvent {
                event: Some(log_event::Event::Stdout(inner)),
            }) = event
            {
                if inner.message.contains("Iteration 100") {
                    break true;
                }
            }
            tokio::time::sleep(Duration::from_millis(10)).await
        }
    };

    let result_future = user.invoke_and_await_agent(
        &component,
        &parsed_agent_id,
        "run_high_volume",
        data_value!(),
    );

    let (found_log_entry, result) = (output_consumer, result_future).join().await;
    result?;

    assert!(found_log_entry);

    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("4m")]
async fn worker_suspends_when_running_out_of_fuel(
    deps: &EnvBasedTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let admin = deps.admin().await;
    let admin_client = admin.registry_service_client().await;

    let user = deps.user().await?;

    // set user to plan with low fuel so workers will be suspended
    admin_client
        .set_account_plan(
            &user.account_id.0,
            &AccountSetPlan {
                current_revision: AccountRevision::INITIAL,
                plan: deps.registry_service().low_fuel_plan(),
            },
        )
        .await?;

    let (_, env) = user.app_and_env().await?;

    let received_http_posts = Arc::new(AtomicU64::new(0));

    let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await.unwrap();
    let host_http_port = listener.local_addr().unwrap().port();

    let http_server_task = tokio::spawn({
        let received_http_posts = received_http_posts.clone();

        async move {
            let route = Router::new().route(
                "/",
                post(async move |headers: HeaderMap, body: Bytes| {
                    received_http_posts.fetch_add(1, Ordering::AcqRel);
                    let header = headers.get("X-Test").unwrap().to_str().unwrap();
                    let body = String::from_utf8(body.to_vec()).unwrap();
                    format!("response is {header} {body}")
                }),
            );

            axum::serve(listener, route).await.unwrap();
        }
        .in_current_span()
    });

    let component = user
        .component(&env.id, "golem_it_http_tests_release")
        .name("golem-it:http-tests")
        .store()
        .await?;

    let mut env = HashMap::new();
    env.insert("PORT".to_string(), host_http_port.to_string());
    env.insert("RUST_BACKTRACE".to_string(), "full".to_string());

    let http_agent_id = agent_id!("HttpClient");
    let agent_id = user
        .start_agent_with(
            &component.id,
            http_agent_id.clone(),
            env,
            HashMap::new(),
            Vec::new(),
        )
        .await?;

    let invoker_task = tokio::spawn({
        let user = user.clone();
        let component = component.clone();
        let agent_id = http_agent_id.clone();
        async move {
            loop {
                let _ = user
                    .invoke_and_await_agent(&component, &agent_id, "run", data_value!())
                    .await;
            }
        }
    });

    user.wait_for_status(&agent_id, AgentStatus::Suspended, Duration::from_secs(10))
        .await?;

    let http_post_count_1 = received_http_posts.load(Ordering::Acquire);
    assert!(http_post_count_1 < 5);

    // just resuming the worker does not allow it finish running.
    user.resume(&agent_id, false).await?;
    user.wait_for_status(&agent_id, AgentStatus::Suspended, Duration::from_secs(10))
        .await?;

    let http_post_count_2 = received_http_posts.load(Ordering::Acquire);
    assert!((http_post_count_2 - http_post_count_1) < 5);

    invoker_task.abort();
    http_server_task.abort();

    Ok(())
}

#[test]
#[tracing::instrument]
async fn agent_await_parallel_rpc_calls(
    deps: &EnvBasedTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let (_, env) = user.app_and_env().await?;

    let component = user
        .component(&env.id, "golem_it_agent_rpc")
        .name("golem-it:agent-rpc")
        .store()
        .await?;

    let unique_id = Uuid::new_v4();
    let agent_id = agent_id!("TestAgent", unique_id.to_string());
    let _agent_id = user.start_agent(&component.id, agent_id.clone()).await?;

    user.invoke_and_await_agent(&component, &agent_id, "run", data_value!(20f64))
        .await?;

    Ok(())
}

#[test]
#[tracing::instrument]
async fn agent_update_constructor_signature(
    deps: &EnvBasedTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let (_, env) = user.app_and_env().await?;

    let component = user
        .component(&env.id, "it_agent_update_v1_release")
        .name("it:agent-update")
        .store()
        .await?;

    let first_deployment_revision = user.get_last_deployment_revision(&env.id)?;

    let agent1_id = agent_id!("CounterAgent", "agent1");
    let agent1 = user.start_agent(&component.id, agent1_id.clone()).await?;
    let result1a = user
        .invoke_and_await_agent(&component, &agent1_id, "increment", data_value!())
        .await?;
    assert_eq!(result1a.into_return_value(), Some(Value::U32(1)));

    let old_singleton_id = agent_id!("Caller");
    let old_singleton = user
        .start_agent(&component.id, old_singleton_id.clone())
        .await?;
    user.log_output(&old_singleton).await?;

    let result1b = user
        .invoke_and_await_agent(&component, &old_singleton_id, "call", data_value!("agent1"))
        .await?;
    assert_eq!(result1b.into_return_value(), Some(Value::U32(2)));

    user.update_component(&component.id, "it_agent_update_v2_release")
        .await?;

    let agent2_id = agent_id!("CounterAgent", 123u64);
    let agent2 = user.start_agent(&component.id, agent2_id.clone()).await?;
    let result2a = user
        .invoke_and_await_agent(&component, &agent2_id, "increment", data_value!())
        .await?;
    assert_eq!(result2a.into_return_value(), Some(Value::U32(1)));

    let new_singleton_id = agent_id!("NewCaller");
    let _new_singleton = user
        .start_agent(&component.id, new_singleton_id.clone())
        .await?;
    let result2b = user
        .invoke_and_await_agent(&component, &new_singleton_id, "call", data_value!(123u64))
        .await?;
    assert_eq!(result2b.into_return_value(), Some(Value::U32(2)));

    // Still able to call both agents
    let result3a = user
        .invoke_and_await_agent_at_deployment(
            &component,
            &agent1_id,
            first_deployment_revision,
            "increment",
            data_value!(),
        )
        .await?;

    let result4a = user
        .invoke_and_await_agent(&component, &agent2_id, "increment", data_value!())
        .await?;

    assert_eq!(result3a.into_return_value(), Some(Value::U32(3)));
    assert_eq!(result4a.into_return_value(), Some(Value::U32(3)));

    // Still able to do RPC
    let result3b = user
        .invoke_and_await_agent_at_deployment(
            &component,
            &old_singleton_id,
            first_deployment_revision,
            "call",
            data_value!("agent1"),
        )
        .await?;
    assert_eq!(result3b.into_return_value(), Some(Value::U32(4)));

    let result4b = user
        .invoke_and_await_agent(&component, &new_singleton_id, "call", data_value!(123u64))
        .await?;
    assert_eq!(result4b.into_return_value(), Some(Value::U32(4)));

    // Enumerate agents
    let mut cursor = ScanCursor::default();
    let mut result = HashSet::new();
    loop {
        let (next_cursor, page) = user
            .get_workers_metadata(&component.id, None, cursor, 2, true)
            .await?;
        if let Some(next_cursor) = next_cursor {
            cursor = next_cursor;
            result.extend(page.into_iter().map(|agent| agent.agent_id.agent_id));
            continue;
        } else {
            break;
        }
    }

    assert_eq!(
        result,
        HashSet::from_iter(vec![
            "CounterAgent(\"agent1\")".to_string(),
            "CounterAgent(123)".to_string(),
            "NewCaller()".to_string(),
            "Caller()".to_string()
        ])
    );

    // Get their metadata
    let _metadata1 = user.get_worker_metadata(&agent1).await?;
    let _metadata2 = user.get_worker_metadata(&agent2).await?;

    Ok(())
}

#[test]
#[tracing::instrument]
async fn deployment_invalidates_agent_resolution_cache(
    deps: &EnvBasedTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let (_, env) = user.app_and_env().await?;

    // Deploy v1: it_agent_update_v1_release has CounterAgent with increment() but NO decrement()
    let component = user
        .component(&env.id, "it_agent_update_v1_release")
        .name("it:agent-update")
        .store()
        .await?;

    // Invoke CounterAgent.increment() — this populates the worker-service agent resolution
    // cache with the v1 deployment's component_id and revision.
    let agent_id = agent_id!("CounterAgent", "cache-test-1");
    let _agent = user.start_agent(&component.id, agent_id.clone()).await?;
    let result_v1 = user
        .invoke_and_await_agent(&component, &agent_id, "increment", data_value!())
        .await?;
    assert_eq!(result_v1.into_return_value(), Some(Value::U32(1)));

    // Update to v2: it_agent_update_v2_release has CounterAgent with both increment() AND
    // decrement(). This triggers a new deployment revision and invalidation event.
    // The agent type name "CounterAgent" is the same in both versions, so the cache
    // entry from v1 must be invalidated for v2's resolution to take effect.
    user.update_component(&component.id, "it_agent_update_v2_release")
        .await?;

    // Allow invalidation event to propagate through gRPC stream to worker-service cache
    sleep(Duration::from_secs(1)).await;

    // Invoke CounterAgent.decrement() — this method only exists in v2.
    // If the cache were stale (still pointing to v1's component), the worker-service
    // would resolve to v1's component which has no decrement(), and this call would fail.
    // Success proves the cache was invalidated and the new deployment is being used.
    let agent_v2_id = agent_id!("CounterAgent", 42u64);
    let _agent_v2 = user.start_agent(&component.id, agent_v2_id.clone()).await?;
    let result_v2 = user
        .invoke_and_await_agent(&component, &agent_v2_id, "decrement", data_value!())
        .await?;
    // Counter starts at 0, decrement returns option::none
    assert_eq!(result_v2.into_return_value(), Some(Value::Option(None)));

    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn websocket_echo_ts(deps: &EnvBasedTestDependencies, _tracing: &Tracing) -> anyhow::Result<()> {

let user = deps.user().await?;
    let (_, env) = user.app_and_env().await?;
    let component = user
        .component(&env.id, "golem_it_agent_sdk_ts")
        .name("golem-it:agent-sdk-ts")
        .unique()
        .store().await?;


    let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await.unwrap();
    let ws_port = listener.local_addr().unwrap().port();

    let ws_server = tokio::spawn(async move {
        if let Ok((stream, _)) = listener.accept().await {
            let ws_stream = tokio_tungstenite::accept_async(stream)
                .await
                .expect("WS handshake failed");
            let (mut write, mut read) = futures::StreamExt::split(ws_stream);
            while let Some(Ok(msg)) = futures::StreamExt::next(&mut read).await {
                if msg.is_close() {
                    break;
                }
                if msg.is_text() || msg.is_binary() {
                    futures::SinkExt::send(&mut write, msg).await.ok();
                }
            }
        }
    });

    let mut env_vars = HashMap::new();
    env_vars.insert("WS_PORT".to_string(), ws_port.to_string());

    let agent_id = agent_id!("WebsocketTest", "ws-echo-test");

    let _agent_id = user
        .start_agent_with(
            &component.id,
            agent_id.clone(),
            env_vars,
            HashMap::new(),
            Vec::new(),
        )
        .await?;

    let result = user
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "echo",
            data_value!(format!("ws://localhost:{ws_port}"), "hello websocket"),
        )
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected return value"))?;

    assert_eq!(result, Value::String("hello websocket".to_string()));

    ws_server.abort();

    Ok(())
}


#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn websocket_echo_rust(deps: &EnvBasedTestDependencies, _tracing: &Tracing) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let (_, env) = user.app_and_env().await?;
    let component = user
        .component(&env.id, "golem_it_host_api_tests_release")
        .name("golem-it:host-api-tests")
        .unique()
        .store()
        .await?;

    let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await.unwrap();
    let ws_port = listener.local_addr().unwrap().port();

    let ws_server = tokio::spawn(async move {
        if let Ok((stream, _)) = listener.accept().await {
            let ws_stream = tokio_tungstenite::accept_async(stream)
                .await
                .expect("WS handshake failed");
            let (mut write, mut read) = futures::StreamExt::split(ws_stream);
            while let Some(Ok(msg)) = futures::StreamExt::next(&mut read).await {
                if msg.is_close() {
                    break;
                }
                if msg.is_text() || msg.is_binary() {
                    futures::SinkExt::send(&mut write, msg).await.ok();
                }
            }
        }
    });

    let mut env_vars = HashMap::new();
    env_vars.insert("WS_PORT".to_string(), ws_port.to_string());

    let agent_id = agent_id!("WebsocketTest", "ws-echo-test");
    let _agent_id = user
        .start_agent_with(
            &component.id,
            agent_id.clone(),
            env_vars,
            HashMap::new(),
            Vec::new(),
        )
        .await?;

    let result = user
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "echo",
            data_value!(format!("ws://localhost:{ws_port}"), "hello websocket"),
        )
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected return value"))?;

    assert_eq!(result, Value::String("hello websocket".to_string()));

    ws_server.abort();

    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn websocket_receive_with_timeout(
    deps: &EnvBasedTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let (_, env) = user.app_and_env().await?;
    let component = user
        .component(&env.id, "golem_it_host_api_tests_release")
        .name("golem-it:host-api-tests")
        .unique()
        .store()
        .await?;

    // Start a WS server that accepts but never sends — so timeout will fire
    let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await.unwrap();
    let ws_port = listener.local_addr().unwrap().port();

    let ws_server = tokio::spawn(async move {
        if let Ok((stream, _)) = listener.accept().await {
            let ws_stream = tokio_tungstenite::accept_async(stream)
                .await
                .expect("WS handshake failed");
            // Hold the connection open but never send anything
            let (_write, mut read) = futures::StreamExt::split(ws_stream);
            while let Some(Ok(msg)) = futures::StreamExt::next(&mut read).await {
                if msg.is_close() {
                    break;
                }
            }
        }
    });

    let mut env_vars = HashMap::new();
    env_vars.insert("WS_PORT".to_string(), ws_port.to_string());

    let agent_id = agent_id!("WebsocketTest", "ws-timeout-test");
    let _agent_id = user
        .start_agent_with(
            &component.id,
            agent_id.clone(),
            env_vars,
            HashMap::new(),
            Vec::new(),
        )
        .await?;

    let result = user
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "receive_with_timeout_test",
            data_value!(format!("ws://localhost:{ws_port}"), 1000u64),
        )
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected return value"))?;

    // Should return None (timeout expired, no message received)
    assert_eq!(result, Value::Option(None));

    ws_server.abort();

    Ok(())
}
