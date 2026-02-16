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
use golem_common::model::oplog::public_oplog_entry::ExportedFunctionInvokedParams;
use golem_common::model::oplog::{OplogIndex, PublicOplogEntry, WorkerResourceId};
use golem_common::model::worker::{
    ExportedResourceMetadata, FlatComponentFileSystemNode, FlatComponentFileSystemNodeKind,
};
use golem_common::model::{
    FilterComparator, IdempotencyKey, PromiseId, ScanCursor, StringFilterComparator, Timestamp,
    WorkerFilter, WorkerId, WorkerResourceDescription, WorkerStatus,
};
use golem_common::{agent_id, data_value, phantom_agent_id};
use golem_test_framework::config::{EnvBasedTestDependencies, TestDependencies};
use golem_test_framework::dsl::{
    update_counts, TestDsl, TestDslExtended, WorkerInvocationResultOps, WorkerLogEventStream,
};
use golem_test_framework::model::IFSEntry;
use golem_wasm::analysis::AnalysedType;
use golem_wasm::analysis::{analysed_type, AnalysedResourceId, AnalysedResourceMode, TypeHandle};
use golem_wasm::json::ValueAndTypeJsonExtensions;
use golem_wasm::{FromValue, IntoValue, IntoValueAndType, Record, UuidRecord, Value, ValueAndType};
use pretty_assertions::assert_eq;
use rand::seq::IteratorRandom;
use serde_json::json;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use test_r::{inherit_test_dep, test, timeout};
use tokio::time::sleep;
use tracing::{info, warn, Instrument};
use uuid::Uuid;

inherit_test_dep!(Tracing);
inherit_test_dep!(EnvBasedTestDependencies);

#[test]
#[tracing::instrument]
#[timeout(120000)]
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
    let agent_id = agent_id!("environment", "dynamic-worker-creation-1");
    let worker_id = user.start_agent(&component.id, agent_id.clone()).await?;

    let args = user
        .invoke_and_await_agent(&component.id, &agent_id, "get_arguments", data_value!())
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected return value"))?;

    let env = user
        .invoke_and_await_agent(&component.id, &agent_id, "get_environment", data_value!())
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected return value"))?;

    let worker_name = agent_id.to_string();
    assert_eq!(args, Value::Result(Ok(Some(Box::new(Value::List(vec![]))))));
    assert_eq!(
        env,
        Value::Result(Ok(Some(Box::new(Value::List(vec![
            Value::Tuple(vec![
                Value::String("GOLEM_AGENT_ID".to_string()),
                Value::String(worker_name.clone())
            ]),
            Value::Tuple(vec![
                Value::String("GOLEM_WORKER_NAME".to_string()),
                Value::String(worker_name)
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
#[timeout(120000)]
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

    let agent_id = agent_id!("rpc-counter", "counter1");
    let worker_id = user.start_agent(&component.id, agent_id.clone()).await?;
    user.log_output(&worker_id).await?;

    user.invoke_and_await_agent(&component.id, &agent_id, "inc_by", data_value!(5u64))
        .await?;

    let result = user
        .invoke_and_await_agent(&component.id, &agent_id, "get_value", data_value!())
        .await?;

    let result_value = result.into_return_value().expect("Expected a return value");

    assert_eq!(result_value, Value::U64(5));

    user.check_oplog_is_queryable(&worker_id).await?;
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout(120000)]
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

    let agent_id = agent_id!("rpc-counter", "counter1j");
    let worker_id = user.start_agent(&component.id, agent_id.clone()).await?;
    user.log_output(&worker_id).await?;

    user.invoke_and_await_agent(&component.id, &agent_id, "inc_by", data_value!(5u64))
        .await?;

    let result = user
        .invoke_and_await_agent(&component.id, &agent_id, "get_value", data_value!())
        .await?;

    let result_value = result.into_return_value().expect("Expected a return value");

    assert_eq!(result_value, Value::U64(5));

    user.check_oplog_is_queryable(&worker_id).await?;
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout(120000)]
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
    let repo_id = agent_id!("repository", "shopping-cart-1");
    let worker_id = user.start_agent(&component.id, repo_id.clone()).await?;
    user.log_output(&worker_id).await?;

    user.invoke_and_await_agent(
        &component.id,
        &repo_id,
        "add",
        data_value!("G1000", "Golem T-Shirt M"),
    )
    .await?;

    user.invoke_and_await_agent(
        &component.id,
        &repo_id,
        "add",
        data_value!("G1001", "Golem Cloud Subscription 1y"),
    )
    .await?;

    user.invoke_and_await_agent(
        &component.id,
        &repo_id,
        "add",
        data_value!("G1002", "Mud Golem"),
    )
    .await?;

    user.invoke_and_await_agent(
        &component.id,
        &repo_id,
        "add",
        data_value!("G1002", "Mud Golem"),
    )
    .await?;

    let contents = user
        .invoke_and_await_agent(&component.id, &repo_id, "list", data_value!())
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
#[timeout(120000)]
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

    let parent_agent_id = agent_id!("rust-parent", "rust_rpc_with_payload");
    let parent = user
        .start_agent(&component.id, parent_agent_id.clone())
        .await?;

    user.log_output(&parent).await?;

    let spawn_result = user
        .invoke_and_await_agent(
            &component.id,
            &parent_agent_id,
            "spawn_child",
            data_value!("hello world"),
        )
        .await?;

    let uuid_as_value = spawn_result
        .into_return_value()
        .expect("Expected a single return value");

    let uuid = UuidRecord::from_value(uuid_as_value.clone()).expect("UUID expected");

    let child_agent_id = agent_id!("rust-child", uuid);

    let get_result = user
        .invoke_and_await_agent(&component.id, &child_agent_id, "get", data_value!())
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
#[timeout(120000)]
async fn get_workers(deps: &EnvBasedTestDependencies, _tracing: &Tracing) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let (_, env) = user.app_and_env().await?;
    let component = user
        .component(&env.id, "it_agent_counters_release")
        .name("it:agent-counters")
        .store()
        .await?;

    let workers_count = 150;
    let mut worker_ids = HashSet::new();
    let mut agent_map: Vec<(WorkerId, golem_common::model::agent::AgentId)> = Vec::new();

    for i in 0..workers_count {
        let aid = agent_id!("repository", format!("gw{i}"));
        let worker_id = user.start_agent(&component.id, aid.clone()).await?;
        worker_ids.insert(worker_id.clone());
        agent_map.push((worker_id, aid));
    }

    let check_indices: Vec<usize> =
        (0..workers_count).choose_multiple(&mut rand::rng(), workers_count / 10);

    for &idx in &check_indices {
        let (ref worker_id, ref aid) = agent_map[idx];

        user.invoke_and_await_agent(&component.id, aid, "list", data_value!())
            .await?;

        let (cursor, values) = user
            .get_workers_metadata(
                &component.id,
                Some(
                    WorkerFilter::new_name(
                        StringFilterComparator::Equal,
                        worker_id.worker_name.clone(),
                    )
                    .and(
                        WorkerFilter::new_status(FilterComparator::Equal, WorkerStatus::Idle).or(
                            WorkerFilter::new_status(
                                FilterComparator::Equal,
                                WorkerStatus::Running,
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

        let ids: HashSet<WorkerId> = values.into_iter().map(|v| v.worker_id).collect();

        assert!(ids.contains(worker_id));
    }

    let mut found_worker_ids = HashSet::new();
    let mut cursor = Some(ScanCursor::default());

    let count = workers_count / 5;

    let filter = Some(WorkerFilter::new_name(
        StringFilterComparator::Like,
        "repository(\"gw".to_string(),
    ));
    while found_worker_ids.len() < workers_count && cursor.is_some() {
        let (cursor1, values1) = user
            .get_workers_metadata(
                &component.id,
                filter.clone(),
                cursor.unwrap(),
                count as u64,
                true,
            )
            .await?;

        assert!(values1.len() > 0); // Each page should contain at least one element, but it is not guaranteed that it has count elements

        let ids: HashSet<WorkerId> = values1.into_iter().map(|v| v.worker_id).collect();
        found_worker_ids.extend(ids);

        cursor = cursor1;
    }

    assert!(found_worker_ids.eq(&worker_ids));

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

    let polling_worker_ids: Arc<Mutex<HashSet<WorkerId>>> = Arc::new(Mutex::new(HashSet::new()));
    let polling_worker_ids_clone = polling_worker_ids.clone();

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
                    let worker_name = query.get("worker_name");
                    if let (Some(component_id), Some(worker_name)) = (component_id, worker_name) {
                        let component_id: ComponentId = component_id.as_str().try_into().unwrap();
                        let worker_id = WorkerId {
                            component_id,
                            worker_name: worker_name.clone(),
                        };
                        let mut ids = polling_worker_ids_clone.lock().unwrap();
                        ids.insert(worker_id.clone());
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
    let mut workers: Vec<(WorkerId, golem_common::model::agent::AgentId)> = Vec::new();

    for _i in 0..workers_count {
        let aid = phantom_agent_id!("http-client2", Uuid::new_v4());
        let worker_id = user
            .start_agent_with(&component.id, aid.clone(), env.clone(), vec![])
            .await?;

        workers.push((worker_id, aid));
    }

    let worker_ids: HashSet<WorkerId> = workers.iter().map(|(w, _)| w.clone()).collect();

    for (worker_id, aid) in &workers {
        user.invoke_agent(&component.id, aid, "start_polling", data_value!("stop"))
            .await?;

        user.wait_for_status(worker_id, WorkerStatus::Running, Duration::from_secs(10))
            .await?;
    }

    let mut wait_counter = 0;
    loop {
        wait_counter += 1;
        let ids = polling_worker_ids.lock().unwrap().clone();

        if worker_ids.eq(&ids) {
            info!("All the spawned workers have polled the server at least once.");
            break;
        }
        if wait_counter >= 30 {
            warn!(
                "Waiting for all spawned workers timed out. Only {}/{} workers polled the server",
                ids.len(),
                workers_count
            );
            break;
        }

        sleep(Duration::from_secs(1)).await;
    }

    // Testing looking for a single worker
    let mut cursor = ScanCursor::default();
    let mut enum_results = Vec::new();
    loop {
        let (next_cursor, values) = user
            .get_workers_metadata(
                &component.id,
                Some(WorkerFilter::new_name(
                    StringFilterComparator::Equal,
                    worker_ids.iter().next().unwrap().worker_name.clone(),
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
    assert_eq!(enum_results.len(), 1);

    // Testing looking for all the workers
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
    assert_eq!(enum_results.len(), workers_count as usize);

    // Testing looking for running workers
    let mut cursor = ScanCursor::default();
    let mut enum_results = Vec::new();
    loop {
        let (next_cursor, values) = user
            .get_workers_metadata(
                &component.id,
                Some(WorkerFilter::new_status(
                    FilterComparator::Equal,
                    WorkerStatus::Running,
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
    // At least one worker should be running; we cannot guarantee that all of them are running simultaneously
    assert!(enum_results.len() <= workers_count as usize);
    assert!(enum_results.len() > 0);

    *response.lock().unwrap() = "stop".to_string();

    for worker_id in &worker_ids {
        user.wait_for_status(worker_id, WorkerStatus::Idle, Duration::from_secs(10))
            .await?;
        user.delete_worker(worker_id).await?;
    }

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
    let component = user.component(&env.id, "update-test-v1").store().await?;

    let worker_id = user
        .start_worker(&component.id, "auto_update_on_idle")
        .await?;

    user.log_output(&worker_id).await?;

    let updated_component = user
        .update_component(&component.id, "update-test-v2")
        .await?;

    info!(
        "Updated component to version {}",
        updated_component.revision
    );

    user.auto_update_worker(&worker_id, updated_component.revision)
        .await?;

    let result = user
        .invoke_and_await(&worker_id, "golem:component/api.{f2}", vec![])
        .await
        .collapse()?;

    info!("result: {result:?}");
    let metadata = user.get_worker_metadata(&worker_id).await?;

    // Expectation: the worker has no history so the update succeeds and then calling f2 returns
    // the current state which is 0
    assert_eq!(result[0], Value::U64(0));
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
    let component = user.component(&env.id, "update-test-v1").store().await?;

    let worker_id = user
        .start_worker(&component.id, "auto_update_on_idle_via_host_function")
        .await?;

    user.log_output(&worker_id).await?;

    let updated_component = user
        .update_component(&component.id, "update-test-v2")
        .await?;

    info!(
        "Updated component to version {}",
        updated_component.revision
    );

    let runtime_svc = user.component(&env.id, "runtime-service").store().await?;
    let runtime_svc_worker = WorkerId {
        component_id: runtime_svc.id,
        worker_name: "runtime-service".to_string(),
    };

    let (high_bits, low_bits) = worker_id.component_id.0.as_u64_pair();
    user.invoke_and_await(
        &runtime_svc_worker,
        "golem:it/api.{update-worker}",
        vec![
            Record(vec![
                (
                    "component-id",
                    Record(vec![(
                        "uuid",
                        Record(vec![
                            ("high-bits", high_bits.into_value_and_type()),
                            ("low-bits", low_bits.into_value_and_type()),
                        ])
                        .into_value_and_type(),
                    )])
                    .into_value_and_type(),
                ),
                (
                    "worker-name",
                    worker_id.worker_name.clone().into_value_and_type(),
                ),
            ])
            .into_value_and_type(),
            updated_component.revision.into_value_and_type(),
            ValueAndType {
                value: Value::Enum(0),
                typ: analysed_type::r#enum(&["automatic", "snapshot-based"]),
            },
        ],
    )
    .await
    .collapse()?;

    let result = user
        .invoke_and_await(&worker_id, "golem:component/api.{f2}", vec![])
        .await
        .collapse()?;

    let metadata = user.get_worker_metadata(&worker_id).await?;

    // Expectation: the worker has no history so the update succeeds and then calling f2 returns
    // the current state which is 0
    assert_eq!(result[0], Value::U64(0));
    assert_eq!(metadata.component_revision, updated_component.revision);
    assert_eq!(update_counts(&metadata), (0, 1, 0));
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout(120000)]
async fn get_oplog_1(deps: &EnvBasedTestDependencies, _tracing: &Tracing) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let (_, env) = user.app_and_env().await?;
    let component = user.component(&env.id, "runtime-service").store().await?;

    let worker_id = WorkerId {
        component_id: component.id,
        worker_name: "getoplog1".to_string(),
    };

    let idempotency_key1 = IdempotencyKey::fresh();
    let idempotency_key2 = IdempotencyKey::fresh();

    user.invoke_and_await(
        &worker_id,
        "golem:it/api.{generate-idempotency-keys}",
        vec![],
    )
    .await
    .collapse()?;

    user.invoke_and_await_with_key(
        &worker_id,
        &idempotency_key1,
        "golem:it/api.{generate-idempotency-keys}",
        vec![],
    )
    .await
    .collapse()?;

    user.invoke_and_await_with_key(
        &worker_id,
        &idempotency_key2,
        "golem:it/api.{generate-idempotency-keys}",
        vec![],
    )
    .await
    .collapse()?;

    let oplog = user.get_oplog(&worker_id, OplogIndex::INITIAL).await?;

    assert_eq!(oplog.len(), 16);
    assert_eq!(oplog[0].oplog_index, OplogIndex::INITIAL);
    assert!(matches!(&oplog[0].entry, PublicOplogEntry::Create(_)));
    assert_eq!(
        oplog
            .iter()
            .filter(
                |entry| matches!(&entry.entry, PublicOplogEntry::ExportedFunctionInvoked(
        ExportedFunctionInvokedParams { function_name, .. }
    ) if function_name == "golem:it/api.{generate-idempotency-keys}")
            )
            .count(),
        3
    );

    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout(120000)]
async fn search_oplog_1(deps: &EnvBasedTestDependencies, _tracing: &Tracing) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let (_, env) = user.app_and_env().await?;
    let component = user
        .component(&env.id, "it_agent_counters_release")
        .name("it:agent-counters")
        .store()
        .await?;

    let repo_id = agent_id!("repository", "searchoplog1");
    let worker_id = user.start_agent(&component.id, repo_id.clone()).await?;

    user.invoke_and_await_agent(
        &component.id,
        &repo_id,
        "add",
        data_value!("G1000", "Golem T-Shirt M"),
    )
    .await?;

    user.invoke_and_await_agent(
        &component.id,
        &repo_id,
        "add",
        data_value!("G1001", "Golem Cloud Subscription 1y"),
    )
    .await?;

    user.invoke_and_await_agent(
        &component.id,
        &repo_id,
        "add",
        data_value!("G1002", "Mud Golem"),
    )
    .await?;

    user.invoke_and_await_agent(
        &component.id,
        &repo_id,
        "add",
        data_value!("G1002", "Mud Golem"),
    )
    .await?;

    user.invoke_and_await_agent(&component.id, &repo_id, "list", data_value!())
        .await?;

    user.get_oplog(&worker_id, OplogIndex::INITIAL).await?;

    let result1 = user.search_oplog(&worker_id, "G1002").await?;

    let result2 = user.search_oplog(&worker_id, "imported-function").await?;

    let result3 = user.search_oplog(&worker_id, "G1001 OR G1000").await?;

    assert_eq!(result1.len(), 2, "G1002"); // TODO: this is temporarily not working because of using the dynamic invoke API and not having structured information in the oplog
    assert_eq!(result2.len(), 2, "imported-function");
    assert_eq!(result3.len(), 0, "id:G1001 OR id:G1000"); // TODO: this is temporarily not working because of using the dynamic invoke API and not having structured information in the oplog

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

    let agent_id = agent_id!("rpc-counter", "recreation");
    let worker_id = user.start_agent(&component.id, agent_id.clone()).await?;

    // Doing many requests, so parts of the oplog gets archived
    for _ in 1..=1200 {
        user.invoke_and_await_agent(&component.id, &agent_id, "inc_by", data_value!(1u64))
            .await?;
    }

    let result1 = user
        .invoke_and_await_agent(&component.id, &agent_id, "get_value", data_value!())
        .await?;

    tokio::time::sleep(Duration::from_secs(2)).await;

    user.delete_worker(&worker_id).await?;

    // Invoking again should create a new worker
    user.invoke_and_await_agent(&component.id, &agent_id, "inc_by", data_value!(1u64))
        .await?;

    let result2 = user
        .invoke_and_await_agent(&component.id, &agent_id, "get_value", data_value!())
        .await?;

    user.delete_worker(&worker_id).await?;

    // Also if we explicitly create a new one
    let worker_id = user.start_agent(&component.id, agent_id.clone()).await?;

    let result3 = user
        .invoke_and_await_agent(&component.id, &agent_id, "get_value", data_value!())
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
    let agent_id = agent_id!("file-read-write", "initial-file-read-write-1");
    let worker_id = user
        .start_agent_with(&component.id, agent_id.clone(), env, vec![])
        .await?;

    let result = user
        .invoke_and_await_agent(&component.id, &agent_id, "run", data_value!())
        .await?;

    user.check_oplog_is_queryable(&worker_id).await?;

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

    let agent_id = agent_id!("file-read-write", "worker-list-files-1");
    let worker_id = user.start_agent(&component.id, agent_id).await?;

    let result = user.get_file_system_node(&worker_id, "/").await?;

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

    let result = user.get_file_system_node(&worker_id, "/bar").await?;

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

    let result = user.get_file_system_node(&worker_id, "/baz.txt").await?;

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

    user.check_oplog_is_queryable(&worker_id).await?;

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
    let agent_id = agent_id!("file-read-write", "initial-file-read-write-3");
    let worker_id = user
        .start_agent_with(&component.id, agent_id.clone(), env, vec![])
        .await?;

    // run the worker so it can update the files.
    let _ = user
        .invoke_and_await_agent(&component.id, &agent_id, "run", data_value!())
        .await?;

    let result1 = user.get_file_contents(&worker_id, "/foo.txt").await?;
    let result1 = std::str::from_utf8(&result1).unwrap();

    let result2 = user.get_file_contents(&worker_id, "/bar/baz.txt").await?;
    let result2 = std::str::from_utf8(&result2).unwrap();

    user.check_oplog_is_queryable(&worker_id).await?;

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

    let agent_id = agent_id!("file-read-write", "initial-file-read-write-1");
    let worker_id = user.start_agent(&component.id, agent_id.clone()).await?;

    // run the worker so it can update the files.
    let _ = user
        .invoke_and_await_agent(&component.id, &agent_id, "run", data_value!())
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

    user.auto_update_worker(&worker_id, updated_component.revision)
        .await?;

    let result1 = user.get_file_contents(&worker_id, "/foo.txt").await?;
    let result1 = std::str::from_utf8(&result1).unwrap();

    let result2 = user.get_file_contents(&worker_id, "/bar/baz.txt").await?;
    let result2 = std::str::from_utf8(&result2).unwrap();

    let result3 = user.get_file_contents(&worker_id, "/baz.txt").await?;
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

    let target_agent_id = agent_id!("rpc-counter", "counter-1");
    user.start_agent(&counter_component.id, target_agent_id)
        .await?;

    let agent_id = agent_id!("golem-host-api", "resolver-1");
    let _resolve_worker = user
        .start_agent(&resolver_component.id, agent_id.clone())
        .await?;

    let result = user
        .invoke_and_await_agent(
            &resolver_component.id,
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

    let worker_name = "promise-agent(\"name\")";
    let worker = user.start_worker(&component.id, worker_name).await?;

    let mut result = user
        .invoke_and_await(
            &worker,
            "golem-it:agent-promise/promise-agent.{get-promise}",
            vec![],
        )
        .await
        .collapse()?;

    assert_eq!(result.len(), 1);

    let promise_id = ValueAndType::new(result.swap_remove(0), PromiseId::get_type());

    let task = {
        let executor_clone = user.clone();
        let worker_clone = worker.clone();
        let promise_id_clone = promise_id.clone();
        tokio::spawn(
            async move {
                executor_clone
                    .invoke_and_await(
                        &worker_clone,
                        "golem-it:agent-promise/promise-agent.{await-promise}",
                        vec![promise_id_clone],
                    )
                    .await
            }
            .in_current_span(),
        )
    };

    user.wait_for_status(&worker, WorkerStatus::Suspended, Duration::from_secs(10))
        .await?;

    let promise_id = PromiseId {
        worker_id: worker.clone(),
        oplog_idx: OplogIndex::from_u64(40),
    };

    user.complete_promise(&promise_id, b"hello".to_vec())
        .await?;

    let result = task.await?.collapse()?;
    assert_eq!(result, vec![Value::String("hello".to_string())]);

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

    let agent_id = agent_id!("logging", "worker-1");
    let worker_id = user.start_agent(&component.id, agent_id.clone()).await?;

    let mut output_stream = user.make_worker_log_event_stream(&worker_id).await?;

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

    let result_future =
        user.invoke_and_await_agent(&component.id, &agent_id, "run_high_volume", data_value!());

    let (found_log_entry, result) = (output_consumer, result_future).join().await;
    result?;

    assert!(found_log_entry);

    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout(120000)]
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

    let http_agent_id = agent_id!("http-client");
    let worker_id = user
        .start_agent_with(&component.id, http_agent_id.clone(), env, vec![])
        .await?;

    let invoker_task = tokio::spawn({
        let user = user.clone();
        let component_id = component.id.clone();
        let agent_id = http_agent_id.clone();
        async move {
            loop {
                let _ = user
                    .invoke_and_await_agent(&component_id, &agent_id, "run", data_value!())
                    .await;
            }
        }
    });

    user.wait_for_status(&worker_id, WorkerStatus::Suspended, Duration::from_secs(10))
        .await?;

    let http_post_count_1 = received_http_posts.load(Ordering::Acquire);
    assert!(http_post_count_1 < 5);

    // just resuming the worker does not allow it finish running.
    user.resume(&worker_id, false).await?;
    user.wait_for_status(&worker_id, WorkerStatus::Suspended, Duration::from_secs(10))
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
    let agent_id = agent_id!("test-agent", unique_id.to_string());
    let _worker_id = user
        .start_agent(&component.id, agent_id.clone())
        .await?;

    user.invoke_and_await_agent(
        &component.id,
        &agent_id,
        "run",
        data_value!(20f64),
    )
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

    let agent1 = user
        .start_worker(&component.id, "counter-agent(\"agent1\")")
        .await?;
    let result1a = user
        .invoke_and_await(&agent1, "it:agent-update/counter-agent.{increment}", vec![])
        .await
        .collapse()?;
    assert_eq!(result1a, vec![Value::U32(1)]);

    let old_singleton = user.start_worker(&component.id, "caller()").await?;
    user.log_output(&old_singleton).await?;

    let result1b = user
        .invoke_and_await(
            &old_singleton,
            "it:agent-update/caller.{call}",
            vec!["agent1".into_value_and_type()],
        )
        .await
        .collapse()?;
    assert_eq!(result1b, vec![Value::U32(2)]);

    user.update_component(&component.id, "it_agent_update_v2_release")
        .await?;

    let agent2 = user
        .start_worker(&component.id, "counter-agent(123)")
        .await?;
    let result2a = user
        .invoke_and_await(&agent2, "it:agent-update/counter-agent.{increment}", vec![])
        .await
        .collapse()?;
    assert_eq!(result2a, vec![Value::U32(1)]);

    let new_singleton = user.start_worker(&component.id, "new-caller()").await?;
    let result2b = user
        .invoke_and_await(
            &new_singleton,
            "it:agent-update/new-caller.{call}",
            vec![123u64.into_value_and_type()],
        )
        .await
        .collapse()?;
    assert_eq!(result2b, vec![Value::U32(2)]);

    // Still able to call both agents
    let result3a = user
        .invoke_and_await(&agent1, "it:agent-update/counter-agent.{increment}", vec![])
        .await
        .collapse()?;

    let result4a = user
        .invoke_and_await(&agent2, "it:agent-update/counter-agent.{increment}", vec![])
        .await
        .collapse()?;

    assert_eq!(result3a, vec![Value::U32(3)]);
    assert_eq!(result4a, vec![Value::U32(3)]);

    // Still able to do RPC
    let result3b = user
        .invoke_and_await(
            &old_singleton,
            "it:agent-update/caller.{call}",
            vec!["agent1".into_value_and_type()],
        )
        .await
        .collapse()?;
    assert_eq!(result3b, vec![Value::U32(4)]);

    let result4b = user
        .invoke_and_await(
            &new_singleton,
            "it:agent-update/new-caller.{call}",
            vec![123u64.into_value_and_type()],
        )
        .await
        .collapse()?;
    assert_eq!(result4b, vec![Value::U32(4)]);

    // Enumerate agents
    let mut cursor = ScanCursor::default();
    let mut result = HashSet::new();
    loop {
        let (next_cursor, page) = user
            .get_workers_metadata(&component.id, None, cursor, 2, true)
            .await?;
        if let Some(next_cursor) = next_cursor {
            cursor = next_cursor;
            result.extend(page.into_iter().map(|agent| agent.worker_id.worker_name));
            continue;
        } else {
            break;
        }
    }

    assert_eq!(
        result,
        HashSet::from_iter(vec![
            "counter-agent(\"agent1\")".to_string(),
            "counter-agent(123)".to_string(),
            "new-caller()".to_string(),
            "caller()".to_string()
        ])
    );

    // Get their metadata
    let _metadata1 = user.get_worker_metadata(&agent1).await?;
    let _metadata2 = user.get_worker_metadata(&agent2).await?;

    Ok(())
}
