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
use assert2::assert;
use assert2::check;
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
use golem_test_framework::config::{EnvBasedTestDependencies, TestDependencies};
use golem_test_framework::dsl::{update_counts, TestDsl, TestDslExtended, WorkerLogEventStream};
use golem_test_framework::model::IFSEntry;
use golem_wasm::analysis::AnalysedType;
use golem_wasm::analysis::{analysed_type, AnalysedResourceId, AnalysedResourceMode, TypeHandle};
use golem_wasm::json::ValueAndTypeJsonExtensions;
use golem_wasm::{IntoValue, IntoValueAndType, Record, Value, ValueAndType};
use rand::seq::IteratorRandom;
use serde_json::json;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};
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
        .component(&env.id, "environment-service")
        .store()
        .await?;
    let worker_id = WorkerId {
        component_id: component.id,
        worker_name: "dynamic-worker-creation-1".to_string(),
    };

    let args = user
        .invoke_and_await(&worker_id, "golem:it/api.{get-arguments}", vec![])
        .await?;

    let env = user
        .invoke_and_await(&worker_id, "golem:it/api.{get-environment}", vec![])
        .await?;

    check!(args == vec![Value::Result(Ok(Some(Box::new(Value::List(vec![])))))]);
    check!(
        env == vec![Value::Result(Ok(Some(Box::new(Value::List(vec![
            Value::Tuple(vec![
                Value::String("GOLEM_AGENT_ID".to_string()),
                Value::String("dynamic-worker-creation-1".to_string())
            ]),
            Value::Tuple(vec![
                Value::String("GOLEM_WORKER_NAME".to_string()),
                Value::String("dynamic-worker-creation-1".to_string())
            ]),
            Value::Tuple(vec![
                Value::String("GOLEM_COMPONENT_ID".to_string()),
                Value::String(format!("{}", component.id))
            ]),
            Value::Tuple(vec![
                Value::String("GOLEM_COMPONENT_REVISION".to_string()),
                Value::String("0".to_string())
            ]),
        ])))))]
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
    let component = user.component(&env.id, "counters").unique().store().await?;
    let worker_id = user.start_worker(&component.id, "counters-1").await?;
    user.log_output(&worker_id).await?;

    let counter1 = user
        .invoke_and_await_typed(
            &worker_id,
            "rpc:counters-exports/api.{[constructor]counter}",
            vec!["counter1".into_value_and_type()],
        )
        .await?
        .unwrap();

    user.invoke_and_await(
        &worker_id,
        "rpc:counters-exports/api.{[method]counter.inc-by}",
        vec![counter1.clone(), 5u64.into_value_and_type()],
    )
    .await?;

    let result1 = user
        .invoke_and_await(
            &worker_id,
            "rpc:counters-exports/api.{[method]counter.get-value}",
            vec![counter1.clone()],
        )
        .await?;

    let metadata1 = user.get_worker_metadata(&worker_id).await?;

    user.invoke_and_await(
        &worker_id,
        "rpc:counters-exports/api.{[drop]counter}",
        vec![counter1.clone()],
    )
    .await?;

    let result2 = user
        .invoke_and_await(
            &worker_id,
            "rpc:counters-exports/api.{get-all-dropped}",
            vec![],
        )
        .await?;

    let metadata2 = user.get_worker_metadata(&worker_id).await?;

    check!(result1 == vec![Value::U64(5)]);

    check!(
        result2
            == vec![Value::List(vec![Value::Tuple(vec![
                Value::String("counter1".to_string()),
                Value::U64(5)
            ])])]
    );

    let ts = Timestamp::now_utc();
    let mut resources1 = metadata1
        .exported_resource_instances
        .iter()
        .map(|erm| ExportedResourceMetadata {
            key: erm.key,
            description: WorkerResourceDescription {
                created_at: ts,
                ..erm.description.clone()
            },
        })
        .collect::<Vec<_>>();
    resources1.sort_by_key(|erm| erm.key);
    check!(
        resources1
            == vec![ExportedResourceMetadata {
                key: WorkerResourceId(0),
                description: WorkerResourceDescription {
                    created_at: ts,
                    resource_owner: "rpc:counters-exports/api".to_string(),
                    resource_name: "counter".to_string()
                }
            }]
    );

    let resources2 = metadata2
        .exported_resource_instances
        .iter()
        .map(|erm| ExportedResourceMetadata {
            key: erm.key,
            description: WorkerResourceDescription {
                created_at: ts,
                ..erm.description.clone()
            },
        })
        .collect::<Vec<_>>();
    check!(resources2 == vec![]);

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
    let component = user.component(&env.id, "counters").unique().store().await?;
    let worker_id = user.start_worker(&component.id, "counters-1j").await?;
    user.log_output(&worker_id).await?;

    let counter1 = user
        .invoke_and_await_json(
            &worker_id,
            "rpc:counters-exports/api.{[constructor]counter}",
            vec![json!("counter1")],
        )
        .await?;

    let counter1 = counter1
        .unwrap()
        .to_json_value()
        .map_err(|e| anyhow!("failed converting value: {e}"))?;

    info!("Using counter1 resource handle {counter1}");

    user.invoke_and_await_json(
        &worker_id,
        "rpc:counters-exports/api.{[method]counter.inc-by}",
        vec![counter1.clone(), json!(5)],
    )
    .await?;

    let result1 = user
        .invoke_and_await_json(
            &worker_id,
            "rpc:counters-exports/api.{[method]counter.get-value}",
            vec![counter1.clone()],
        )
        .await?;

    let metadata1 = user.get_worker_metadata(&worker_id).await?;

    user.invoke_and_await_json(
        &worker_id,
        "rpc:counters-exports/api.{[drop]counter}",
        vec![counter1.clone()],
    )
    .await?;

    let result2 = user
        .invoke_and_await_json(
            &worker_id,
            "rpc:counters-exports/api.{get-all-dropped}",
            vec![],
        )
        .await?;

    let metadata2 = user.get_worker_metadata(&worker_id).await?;

    assert2::assert!(result1 == Some(5u64.into_value_and_type()));
    assert2::assert!(result2 == Some(vec![("counter1".to_string(), 5u64)].into_value_and_type()));

    let ts = Timestamp::now_utc();
    let mut resources1 = metadata1
        .exported_resource_instances
        .iter()
        .map(|erm| ExportedResourceMetadata {
            key: erm.key,
            description: WorkerResourceDescription {
                created_at: ts,
                ..erm.description.clone()
            },
        })
        .collect::<Vec<_>>();
    resources1.sort_by_key(|erm| erm.key);
    check!(
        resources1
            == vec![ExportedResourceMetadata {
                key: WorkerResourceId(0),
                description: WorkerResourceDescription {
                    created_at: ts,
                    resource_owner: "rpc:counters-exports/api".to_string(),
                    resource_name: "counter".to_string()
                }
            }]
    );

    let resources2 = metadata2
        .exported_resource_instances
        .iter()
        .map(|erm| ExportedResourceMetadata {
            key: erm.key,
            description: WorkerResourceDescription {
                created_at: ts,
                ..erm.description.clone()
            },
        })
        .collect::<Vec<_>>();
    check!(resources2 == vec![]);

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
        .component(&env.id, "shopping-cart")
        .unique()
        .store()
        .await?;
    let worker_id = user.start_worker(&component.id, "shopping-cart-1").await?;
    user.log_output(&worker_id).await?;

    user.invoke_and_await(
        &worker_id,
        "golem:it/api.{initialize-cart}",
        vec!["test-user-1".into_value_and_type()],
    )
    .await?;

    user.invoke_and_await(
        &worker_id,
        "golem:it/api.{add-item}",
        vec![Record(vec![
            ("product-id", "G1000".into_value_and_type()),
            ("name", "Golem T-Shirt M".into_value_and_type()),
            ("price", 100.0f32.into_value_and_type()),
            ("quantity", 5u32.into_value_and_type()),
        ])
        .into_value_and_type()],
    )
    .await?;

    user.invoke_and_await(
        &worker_id,
        "golem:it/api.{add-item}",
        vec![Record(vec![
            ("product-id", "G1001".into_value_and_type()),
            ("name", "Golem Cloud Subscription 1y".into_value_and_type()),
            ("price", 999999.0f32.into_value_and_type()),
            ("quantity", 1u32.into_value_and_type()),
        ])
        .into_value_and_type()],
    )
    .await?;

    user.invoke_and_await(
        &worker_id,
        "golem:it/api.{add-item}",
        vec![Record(vec![
            ("product-id", "G1002".into_value_and_type()),
            ("name", "Mud Golem".into_value_and_type()),
            ("price", 11.0f32.into_value_and_type()),
            ("quantity", 10u32.into_value_and_type()),
        ])
        .into_value_and_type()],
    )
    .await?;

    user.invoke_and_await(
        &worker_id,
        "golem:it/api.{update-item-quantity}",
        vec!["G1002".into_value_and_type(), 20u32.into_value_and_type()],
    )
    .await?;

    let contents = user
        .invoke_and_await(&worker_id, "golem:it/api.{get-cart-contents}", vec![])
        .await?;

    user.invoke_and_await(&worker_id, "golem:it/api.{checkout}", vec![])
        .await?;

    check!(
        contents
            == vec![Value::List(vec![
                Value::Record(vec![
                    Value::String("G1000".to_string()),
                    Value::String("Golem T-Shirt M".to_string()),
                    Value::F32(100.0),
                    Value::U32(5),
                ]),
                Value::Record(vec![
                    Value::String("G1001".to_string()),
                    Value::String("Golem Cloud Subscription 1y".to_string()),
                    Value::F32(999999.0),
                    Value::U32(1),
                ]),
                Value::Record(vec![
                    Value::String("G1002".to_string()),
                    Value::String("Mud Golem".to_string()),
                    Value::F32(11.0),
                    Value::U32(20),
                ]),
            ])]
    );

    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout(120000)]
async fn auction_example_1(
    deps: &EnvBasedTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let (_, env) = user.app_and_env().await?;
    let registry_component = user
        .component(&env.id, "auction_registry_composed")
        .store()
        .await?;
    let auction_component = user.component(&env.id, "auction").store().await?;

    let mut env = HashMap::new();
    env.insert(
        "AUCTION_COMPONENT_ID".to_string(),
        auction_component.id.to_string(),
    );

    let registry_worker_id = user
        .start_worker_with(&registry_component.id, "auction-registry-1", env, vec![])
        .await?;

    user.log_output(&registry_worker_id).await?;

    let expiration = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_secs();

    let mut create_results = vec![];

    for _ in 1..100 {
        let create_auction_result = user
            .invoke_and_await(
                &registry_worker_id,
                "auction:registry-exports/api.{create-auction}",
                vec![
                    "test-auction".into_value_and_type(),
                    "this is a test".into_value_and_type(),
                    100.0f32.into_value_and_type(),
                    (expiration + 600).into_value_and_type(),
                ],
            )
            .await;

        create_results.push(create_auction_result);
    }

    let get_auctions_result = user
        .invoke_and_await(
            &registry_worker_id,
            "auction:registry-exports/api.{get-auctions}",
            vec![],
        )
        .await?;

    info!("result: {create_results:?}");
    info!("result: {get_auctions_result:?}");

    check!(create_results.iter().all(|r| r.is_ok()));

    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout(120000)]
async fn get_workers(deps: &EnvBasedTestDependencies, _tracing: &Tracing) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let (_, env) = user.app_and_env().await?;
    let component = user.component(&env.id, "shopping-cart").store().await?;

    let workers_count = 150;
    let mut worker_ids = HashSet::new();

    for i in 0..workers_count {
        let worker_id = user
            .start_worker(&component.id, &format!("get-workers-test-{i}"))
            .await?;

        worker_ids.insert(worker_id);
    }

    let check_worker_ids = worker_ids
        .iter()
        .choose_multiple(&mut rand::rng(), workers_count / 10);

    for worker_id in check_worker_ids {
        user.invoke_and_await(
            worker_id,
            "golem:it/api.{initialize-cart}",
            vec!["test-user-1".into_value_and_type()],
        )
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

        check!(values.len() == 1);
        check!(cursor.is_none());

        let ids: HashSet<WorkerId> = values.into_iter().map(|v| v.worker_id).collect();

        check!(ids.contains(worker_id));
    }

    let mut found_worker_ids = HashSet::new();
    let mut cursor = Some(ScanCursor::default());

    let count = workers_count / 5;

    let filter = Some(WorkerFilter::new_name(
        StringFilterComparator::Like,
        "get-workers-test-".to_string(),
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

        check!(values1.len() > 0); // Each page should contain at least one element, but it is not guaranteed that it has count elements

        let ids: HashSet<WorkerId> = values1.into_iter().map(|v| v.worker_id).collect();
        found_worker_ids.extend(ids);

        cursor = cursor1;
    }

    check!(found_worker_ids.eq(&worker_ids));

    if let Some(cursor) = cursor {
        let (_, values) = user
            .get_workers_metadata(&component.id, filter, cursor, workers_count as u64, true)
            .await?;
        check!(values.len() == 0);
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
        .component(&env.id, "http-client-2")
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
    let mut worker_ids = HashSet::new();

    for i in 0..workers_count {
        let worker_id = user
            .start_worker_with(
                &component.id,
                &format!("worker-http-client-{i}"),
                env.clone(),
                vec![],
            )
            .await?;

        worker_ids.insert(worker_id);
    }

    for worker_id in &worker_ids {
        user.invoke(
            worker_id,
            "golem:it/api.{start-polling}",
            vec!["stop".into_value_and_type()],
        )
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
    check!(enum_results.len() == 1);

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
    check!(enum_results.len() == workers_count as usize);

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
    check!(enum_results.len() <= workers_count as usize);
    check!(enum_results.len() > 0);

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
        .await?;

    info!("result: {result:?}");
    let metadata = user.get_worker_metadata(&worker_id).await?;

    // Expectation: the worker has no history so the update succeeds and then calling f2 returns
    // the current state which is 0
    check!(result[0] == Value::U64(0));
    check!(metadata.component_version == updated_component.revision);
    check!(update_counts(&metadata) == (0, 1, 0));
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
    .await?;

    let result = user
        .invoke_and_await(&worker_id, "golem:component/api.{f2}", vec![])
        .await?;

    let metadata = user.get_worker_metadata(&worker_id).await?;

    // Expectation: the worker has no history so the update succeeds and then calling f2 returns
    // the current state which is 0
    check!(result[0] == Value::U64(0));
    check!(metadata.component_version == updated_component.revision);
    check!(update_counts(&metadata) == (0, 1, 0));
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
    .await?;

    user.invoke_and_await_with_key(
        &worker_id,
        &idempotency_key1,
        "golem:it/api.{generate-idempotency-keys}",
        vec![],
    )
    .await?;

    user.invoke_and_await_with_key(
        &worker_id,
        &idempotency_key2,
        "golem:it/api.{generate-idempotency-keys}",
        vec![],
    )
    .await?;

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
    let component = user.component(&env.id, "shopping-cart").store().await?;

    let worker_id = WorkerId {
        component_id: component.id,
        worker_name: "searchoplog1".to_string(),
    };

    user.invoke_and_await(
        &worker_id,
        "golem:it/api.{initialize-cart}",
        vec!["test-user-1".into_value_and_type()],
    )
    .await?;

    user.invoke_and_await(
        &worker_id,
        "golem:it/api.{add-item}",
        vec![Record(vec![
            ("product-id", "G1000".into_value_and_type()),
            ("name", "Golem T-Shirt M".into_value_and_type()),
            ("price", 100.0f32.into_value_and_type()),
            ("quantity", 5u32.into_value_and_type()),
        ])
        .into_value_and_type()],
    )
    .await?;

    user.invoke_and_await(
        &worker_id,
        "golem:it/api.{add-item}",
        vec![Record(vec![
            ("product-id", "G1001".into_value_and_type()),
            ("name", "Golem Cloud Subscription 1y".into_value_and_type()),
            ("price", 999999.0f32.into_value_and_type()),
            ("quantity", 1u32.into_value_and_type()),
        ])
        .into_value_and_type()],
    )
    .await?;

    user.invoke_and_await(
        &worker_id,
        "golem:it/api.{add-item}",
        vec![Record(vec![
            ("product-id", "G1002".into_value_and_type()),
            ("name", "Mud Golem".into_value_and_type()),
            ("price", 11.0f32.into_value_and_type()),
            ("quantity", 10u32.into_value_and_type()),
        ])
        .into_value_and_type()],
    )
    .await?;

    user.invoke_and_await(
        &worker_id,
        "golem:it/api.{update-item-quantity}",
        vec!["G1002".into_value_and_type(), 20u32.into_value_and_type()],
    )
    .await?;

    user.invoke_and_await(&worker_id, "golem:it/api.{get-cart-contents}", vec![])
        .await?;

    user.invoke_and_await(&worker_id, "golem:it/api.{checkout}", vec![])
        .await?;

    user.get_oplog(&worker_id, OplogIndex::INITIAL).await?;

    let result1 = user.search_oplog(&worker_id, "G1002").await?;

    let result2 = user.search_oplog(&worker_id, "imported-function").await?;

    let result3 = user
        .search_oplog(&worker_id, "product-id:G1001 OR product-id:G1000")
        .await?;

    assert_eq!(result1.len(), 7); // two invocations and two log messages, and the get-cart-contents results
    assert_eq!(result2.len(), 1); // get_random_bytes
    assert_eq!(result3.len(), 5); // two invocations, and the get-cart-contents results
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
    let component = user.component(&env.id, "counters").store().await?;

    let worker_id = user
        .start_worker(&component.id, "counters-recreation")
        .await?;

    let counter1 = user
        .invoke_and_await(
            &worker_id,
            "rpc:counters-exports/api.{[constructor]counter}",
            vec!["counter1".into_value_and_type()],
        )
        .await?;

    let counter1 = ValueAndType::new(
        counter1[0].clone(),
        AnalysedType::Handle(TypeHandle {
            name: None,
            owner: None,
            resource_id: AnalysedResourceId(0),
            mode: AnalysedResourceMode::Borrowed,
        }),
    );

    // Doing many requests, so parts of the oplog gets archived
    for _ in 1..=1200 {
        user.invoke_and_await(
            &worker_id,
            "rpc:counters-exports/api.{[method]counter.inc-by}",
            vec![counter1.clone(), 1u64.into_value_and_type()],
        )
        .await?;
    }

    let result1 = user
        .invoke_and_await(
            &worker_id,
            "rpc:counters-exports/api.{[method]counter.get-value}",
            vec![counter1.clone()],
        )
        .await?;

    tokio::time::sleep(Duration::from_secs(2)).await;

    user.delete_worker(&worker_id).await?;

    // Invoking again should create a new worker
    let counter1 = user
        .invoke_and_await(
            &worker_id,
            "rpc:counters-exports/api.{[constructor]counter}",
            vec!["counter1".into_value_and_type()],
        )
        .await?;

    let counter1 = ValueAndType::new(
        counter1[0].clone(),
        AnalysedType::Handle(TypeHandle {
            name: None,
            owner: None,
            resource_id: AnalysedResourceId(0),
            mode: AnalysedResourceMode::Borrowed,
        }),
    );

    user.invoke_and_await(
        &worker_id,
        "rpc:counters-exports/api.{[method]counter.inc-by}",
        vec![counter1.clone(), 1u64.into_value_and_type()],
    )
    .await?;

    let result2 = user
        .invoke_and_await(
            &worker_id,
            "rpc:counters-exports/api.{[method]counter.get-value}",
            vec![counter1.clone()],
        )
        .await?;

    user.delete_worker(&worker_id).await?;

    // Also if we explicitly create a new one
    let worker_id = user
        .start_worker(&component.id, "counters-recreation")
        .await?;

    let counter1 = user
        .invoke_and_await(
            &worker_id,
            "rpc:counters-exports/api.{[constructor]counter}",
            vec!["counter1".into_value_and_type()],
        )
        .await?;

    let counter1 = ValueAndType::new(
        counter1[0].clone(),
        AnalysedType::Handle(TypeHandle {
            name: None,
            owner: None,
            resource_id: AnalysedResourceId(0),
            mode: AnalysedResourceMode::Borrowed,
        }),
    );

    let result3 = user
        .invoke_and_await(
            &worker_id,
            "rpc:counters-exports/api.{[method]counter.get-value}",
            vec![counter1.clone()],
        )
        .await?;

    check!(result1 == vec![Value::U64(1200)]);
    check!(result2 == vec![Value::U64(1)]);
    check!(result3 == vec![Value::U64(0)]);

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
        .component(&env.id, "initial-file-read-write")
        .unique()
        .with_files(&[
            IFSEntry {
                source_path: PathBuf::from("initial-file-read-write/files/foo.txt"),
                target_path: ComponentFilePath::from_abs_str("/foo.txt").unwrap(),
                permissions: ComponentFilePermissions::ReadOnly,
            },
            IFSEntry {
                source_path: PathBuf::from("initial-file-read-write/files/baz.txt"),
                target_path: ComponentFilePath::from_abs_str("/bar/baz.txt").unwrap(),
                permissions: ComponentFilePermissions::ReadWrite,
            },
        ])
        .store()
        .await?;

    let mut env = HashMap::new();
    env.insert("RUST_BACKTRACE".to_string(), "full".to_string());
    let worker_id = user
        .start_worker_with(&component.id, "initial-file-read-write-1", env, vec![])
        .await?;

    let result = user.invoke_and_await(&worker_id, "run", vec![]).await?;

    user.check_oplog_is_queryable(&worker_id).await?;

    check!(
        result
            == vec![Value::Tuple(vec![
                Value::Option(Some(Box::new(Value::String("foo\n".to_string())))),
                Value::Option(None),
                Value::Option(None),
                Value::Option(Some(Box::new(Value::String("baz\n".to_string())))),
                Value::Option(Some(Box::new(Value::String("hello world".to_string())))),
            ])]
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
        .component(&env.id, "initial-file-read-write")
        .unique()
        .with_files(&[
            IFSEntry {
                source_path: PathBuf::from("initial-file-read-write/files/foo.txt"),
                target_path: ComponentFilePath::from_abs_str("/foo.txt").unwrap(),
                permissions: ComponentFilePermissions::ReadOnly,
            },
            IFSEntry {
                source_path: PathBuf::from("initial-file-read-write/files/baz.txt"),
                target_path: ComponentFilePath::from_abs_str("/bar/baz.txt").unwrap(),
                permissions: ComponentFilePermissions::ReadWrite,
            },
            IFSEntry {
                source_path: PathBuf::from("initial-file-read-write/files/baz.txt"),
                target_path: ComponentFilePath::from_abs_str("/baz.txt").unwrap(),
                permissions: ComponentFilePermissions::ReadWrite,
            },
        ])
        .store()
        .await?;

    let worker_id = user
        .start_worker(&component.id, "initial-file-read-write-1")
        .await?;

    let result = user.get_file_system_node(&worker_id, "/").await?;

    let mut result = result
        .into_iter()
        .map(|e| FlatComponentFileSystemNode {
            last_modified: 0,
            ..e
        })
        .collect::<Vec<_>>();

    result.sort_by_key(|e| e.name.clone());

    check!(
        result
            == vec![
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

    check!(
        result
            == vec![FlatComponentFileSystemNode {
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

    check!(
        result
            == vec![FlatComponentFileSystemNode {
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
        .component(&env.id, "initial-file-read-write")
        .unique()
        .with_files(&[
            IFSEntry {
                source_path: PathBuf::from("initial-file-read-write/files/foo.txt"),
                target_path: ComponentFilePath::from_abs_str("/foo.txt").unwrap(),
                permissions: ComponentFilePermissions::ReadOnly,
            },
            IFSEntry {
                source_path: PathBuf::from("initial-file-read-write/files/baz.txt"),
                target_path: ComponentFilePath::from_abs_str("/bar/baz.txt").unwrap(),
                permissions: ComponentFilePermissions::ReadWrite,
            },
        ])
        .store()
        .await?;

    let mut env = HashMap::new();
    env.insert("RUST_BACKTRACE".to_string(), "full".to_string());
    let worker_id = user
        .start_worker_with(&component.id, "initial-file-read-write-3", env, vec![])
        .await?;

    // run the worker so it can update the files.
    user.invoke_and_await(&worker_id, "run", vec![]).await?;

    let result1 = user.get_file_contents(&worker_id, "/foo.txt").await?;
    let result1 = std::str::from_utf8(&result1).unwrap();

    let result2 = user.get_file_contents(&worker_id, "/bar/baz.txt").await?;
    let result2 = std::str::from_utf8(&result2).unwrap();

    user.check_oplog_is_queryable(&worker_id).await?;

    check!(result1 == "foo\n");
    check!(result2 == "hello world");

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
        .component(&env.id, "initial-file-read-write")
        .with_files(&[
            IFSEntry {
                source_path: PathBuf::from("initial-file-read-write/files/foo.txt"),
                target_path: ComponentFilePath::from_abs_str("/foo.txt").unwrap(),
                permissions: ComponentFilePermissions::ReadOnly,
            },
            IFSEntry {
                source_path: PathBuf::from("initial-file-read-write/files/baz.txt"),
                target_path: ComponentFilePath::from_abs_str("/bar/baz.txt").unwrap(),
                permissions: ComponentFilePermissions::ReadWrite,
            },
        ])
        .store()
        .await?;

    let worker_id = user
        .start_worker(&component.id, "initial-file-read-write-1")
        .await?;

    // run the worker so it can update the files.
    user.invoke_and_await(&worker_id, "run", vec![]).await?;

    let updated_component = user
        .update_component_with_files(
            &component.id,
            "initial-file-read-write",
            vec![
                IFSEntry {
                    source_path: PathBuf::from("initial-file-read-write/files/foo.txt"),
                    target_path: ComponentFilePath::from_abs_str("/foo.txt").unwrap(),
                    permissions: ComponentFilePermissions::ReadOnly,
                },
                IFSEntry {
                    source_path: PathBuf::from("initial-file-read-write/files/baz.txt"),
                    target_path: ComponentFilePath::from_abs_str("/bar/baz.txt").unwrap(),
                    permissions: ComponentFilePermissions::ReadWrite,
                },
                IFSEntry {
                    source_path: PathBuf::from("initial-file-read-write/files/baz.txt"),
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

    check!(result1 == "foo\n");
    check!(result2 == "hello world");
    check!(result3 == "baz\n");

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
        .component(&env.id, "counters")
        .name("component-resolve-target")
        .store()
        .await?;

    let resolver_component = user.component(&env.id, "component-resolve").store().await?;

    user.start_worker(&counter_component.id, "counter-1")
        .await?;

    let resolve_worker = user
        .start_worker(&resolver_component.id, "resolver-1")
        .await?;

    let result = user
        .invoke_and_await(
            &resolve_worker,
            "golem:it/component-resolve-api.{run}",
            vec![],
        )
        .await?;

    check!(result.len() == 1);

    let (high_bits, low_bits) = counter_component.id.0.as_u64_pair();
    let component_id_value = Value::Record(vec![Value::Record(vec![
        Value::U64(high_bits),
        Value::U64(low_bits),
    ])]);

    let worker_id_value = Value::Record(vec![
        component_id_value.clone(),
        Value::String("counter-1".to_string()),
    ]);

    check!(
        result[0]
            == Value::Tuple(vec![
                Value::Option(Some(Box::new(component_id_value))),
                Value::Option(Some(Box::new(worker_id_value))),
                Value::Option(None),
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
        .unwrap();

    assert!(result.len() == 1);

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
        oplog_idx: OplogIndex::from_u64(38),
    };

    user.complete_promise(&promise_id, b"hello".to_vec())
        .await?;

    let result = task.await??;
    check!(result == vec![Value::String("hello".to_string())]);

    Ok(())
}

#[test]
#[tracing::instrument]
async fn stream_high_volume_log_output(deps: &EnvBasedTestDependencies) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let (_, env) = user.app_and_env().await?;

    let component = user
        .component(&env.id, "golem_it_high_volume_logging")
        .store()
        .await?;

    let worker_id = user.start_worker(&component.id, "worker-1").await?;

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

    let result_future = user.invoke_and_await(
        &worker_id,
        "golem-it:high-volume-logging-exports/golem-it-high-volume-logging-api.{run}",
        vec![],
    );

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

    let component = user.component(&env.id, "http-client").store().await?;

    let mut env = HashMap::new();
    env.insert("PORT".to_string(), host_http_port.to_string());
    env.insert("RUST_BACKTRACE".to_string(), "full".to_string());

    let worker_id = user
        .start_worker_with(&component.id, "http-client-1", env, vec![])
        .await?;

    let invoker_task = tokio::spawn({
        let user = user.clone();
        let worker_id = worker_id.clone();
        async move {
            loop {
                user.invoke_and_await(&worker_id, "golem:it/api.{run}", vec![])
                    .await
                    .unwrap();
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
    let worker_id = user
        .start_worker(&component.id, &format!("test-agent(\"{unique_id}\")"))
        .await?;

    user.invoke_and_await(
        &worker_id,
        "golem-it:agent-rpc/test-agent.{run}",
        vec![20f64.into_value_and_type()],
    )
    .await?;

    Ok(())
}
