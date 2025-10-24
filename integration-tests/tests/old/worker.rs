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
use axum::extract::Query;
use axum::routing::get;
use axum::Router;
use golem_client::model::AnalysedType;
use golem_common::model::oplog::OplogIndex;
use golem_common::model::public_oplog::{ExportedFunctionInvokedParameters, PublicOplogEntry};
use golem_common::model::{
    ComponentFilePermissions, ComponentFileSystemNode, ComponentFileSystemNodeDetails, ComponentId,
    FilterComparator, IdempotencyKey, PromiseId, ScanCursor, StringFilterComparator, Timestamp,
    WorkerFilter, WorkerId, WorkerMetadata, WorkerResourceDescription, WorkerStatus,
};
use golem_test_framework::config::{EnvBasedTestDependencies, TestDependencies};
use golem_test_framework::dsl::TestDslUnsafe;
use golem_wasm::analysis::{analysed_type, AnalysedResourceId, AnalysedResourceMode, TypeHandle};
use golem_wasm::IntoValue;
use golem_wasm::{IntoValueAndType, Record, Value, ValueAndType};
use rand::seq::IteratorRandom;
use serde_json::json;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};
use test_r::{inherit_test_dep, test, timeout};
use tokio::time::sleep;
use tracing::log::info;
use tracing::{warn, Instrument};

inherit_test_dep!(Tracing);
inherit_test_dep!(EnvBasedTestDependencies);

#[test]
#[tracing::instrument]
#[timeout(120000)]
async fn dynamic_worker_creation(deps: &EnvBasedTestDependencies, _tracing: &Tracing) {
    let admin = deps.admin().await;
    let component_id = admin.component("environment-service").store().await;
    let worker_id = WorkerId {
        component_id: component_id.clone(),
        worker_name: "dynamic-worker-creation-1".to_string(),
    };

    let args = admin
        .invoke_and_await(&worker_id, "golem:it/api.{get-arguments}", vec![])
        .await
        .unwrap();
    let env = admin
        .invoke_and_await(&worker_id, "golem:it/api.{get-environment}", vec![])
        .await
        .unwrap();

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
                Value::String(format!("{component_id}"))
            ]),
            Value::Tuple(vec![
                Value::String("GOLEM_COMPONENT_VERSION".to_string()),
                Value::String("0".to_string())
            ]),
        ])))))]
    );
}

#[test]
#[tracing::instrument]
#[timeout(120000)]
async fn counter_resource_test_1(deps: &EnvBasedTestDependencies, _tracing: &Tracing) {
    let admin = deps.admin().await;
    let component_id = admin.component("counters").unique().store().await;
    let worker_id = admin.start_worker(&component_id, "counters-1").await;
    admin.log_output(&worker_id).await;

    let counter1 = admin
        .invoke_and_await(
            &worker_id,
            "rpc:counters-exports/api.{[constructor]counter}",
            vec!["counter1".into_value_and_type()],
        )
        .await
        .unwrap();

    let _ = admin
        .invoke_and_await(
            &worker_id,
            "rpc:counters-exports/api.{[method]counter.inc-by}",
            vec![
                ValueAndType {
                    value: counter1[0].clone(),
                    typ: analysed_type::u64(),
                },
                5u64.into_value_and_type(),
            ],
        )
        .await;

    let result1 = admin
        .invoke_and_await(
            &worker_id,
            "rpc:counters-exports/api.{[method]counter.get-value}",
            vec![ValueAndType {
                value: counter1[0].clone(),
                typ: analysed_type::u64(),
            }],
        )
        .await;

    let (metadata1, _) = admin.get_worker_metadata(&worker_id).await.unwrap();

    let _ = admin
        .invoke_and_await(
            &worker_id,
            "rpc:counters-exports/api.{[drop]counter}",
            vec![ValueAndType {
                value: counter1[0].clone(),
                typ: analysed_type::u64(),
            }],
        )
        .await;

    let result2 = admin
        .invoke_and_await(
            &worker_id,
            "rpc:counters-exports/api.{get-all-dropped}",
            vec![],
        )
        .await;

    let (metadata2, _) = admin.get_worker_metadata(&worker_id).await.unwrap();

    check!(result1 == Ok(vec![Value::U64(5)]));

    check!(
        result2
            == Ok(vec![Value::List(vec![Value::Tuple(vec![
                Value::String("counter1".to_string()),
                Value::U64(5)
            ])])])
    );

    let ts = Timestamp::now_utc();
    let mut resources1 = metadata1
        .last_known_status
        .owned_resources
        .iter()
        .map(|(k, v)| {
            (
                k.to_string(),
                WorkerResourceDescription {
                    created_at: ts,
                    ..v.clone()
                },
            )
        })
        .collect::<Vec<_>>();
    resources1.sort_by_key(|(k, _v)| k.clone());
    check!(
        resources1
            == vec![(
                "0".to_string(),
                WorkerResourceDescription {
                    created_at: ts,
                    resource_owner: "rpc:counters-exports/api".to_string(),
                    resource_name: "counter".to_string(),
                }
            ),]
    );

    let resources2 = metadata2
        .last_known_status
        .owned_resources
        .iter()
        .map(|(k, v)| {
            (
                k.to_string(),
                WorkerResourceDescription {
                    created_at: ts,
                    ..v.clone()
                },
            )
        })
        .collect::<Vec<_>>();
    check!(resources2 == vec![]);
}

#[test]
#[tracing::instrument]
#[timeout(120000)]
async fn counter_resource_test_1_json(deps: &EnvBasedTestDependencies, _tracing: &Tracing) {
    let admin = deps.admin().await;
    let component_id = admin.component("counters").unique().store().await;
    let worker_id = admin.start_worker(&component_id, "counters-1j").await;
    admin.log_output(&worker_id).await;

    let counter1 = admin
        .invoke_and_await_json(
            &worker_id,
            "rpc:counters-exports/api.{[constructor]counter}",
            vec![json!({ "typ": { "type": "Str" }, "value": "counter1" })],
        )
        .await
        .unwrap();

    // counter1 is a JSON response containing a tuple with a single element holding the resource handle:
    // {
    //   "typ":{"mode":"Owned","resource_id":0,"type":"Handle"}
    //   "value":"urn:worker:3e8cc61d-7490-4d23-a516-36bc825365a5/counters-1j/0"
    // }
    // we only need this inner element, and it's type:

    println!("Counter1: {counter1}");

    let counter1_type = counter1.as_object().unwrap().get("typ").unwrap();
    let counter1_value = counter1.as_object().unwrap().get("value").unwrap();
    let counter1 = json!(
        {
            "typ": counter1_type,
            "value": counter1_value
        }
    );

    info!("Using counter1 resource handle {counter1}");

    let _ = admin
        .invoke_and_await_json(
            &worker_id,
            "rpc:counters-exports/api.{[method]counter.inc-by}",
            vec![
                counter1.clone(),
                json!({ "typ": { "type": "U64"}, "value": 5 }),
            ],
        )
        .await;

    let result1 = admin
        .invoke_and_await_json(
            &worker_id,
            "rpc:counters-exports/api.{[method]counter.get-value}",
            vec![counter1.clone()],
        )
        .await;

    let (metadata1, _) = admin.get_worker_metadata(&worker_id).await.unwrap();

    let _ = admin
        .invoke_and_await_json(
            &worker_id,
            "rpc:counters-exports/api.{[drop]counter}",
            vec![counter1.clone()],
        )
        .await;

    let result2 = admin
        .invoke_and_await_json(
            &worker_id,
            "rpc:counters-exports/api.{get-all-dropped}",
            vec![],
        )
        .await;

    let (metadata2, _) = admin.get_worker_metadata(&worker_id).await.unwrap();

    check!(
        result1
            == Ok(json!(
                {
                    "typ": {
                        "type": "U64",
                    },
                    "value": 5
                }
            ))
    );

    check!(
        result2
            == Ok(json!(
                {
                  "typ": {
                        "type": "List",
                        "inner": {
                          "type": "Tuple",
                          "items": [
                            {
                              "type": "Str"
                            },
                            {
                              "type": "U64"
                            }
                          ]
                        }
                  },
                  "value": [
                    ["counter1",5]
                  ]
            }))
    );

    let ts = Timestamp::now_utc();
    let mut resources1 = metadata1
        .last_known_status
        .owned_resources
        .iter()
        .map(|(k, v)| {
            (
                k.to_string(),
                WorkerResourceDescription {
                    created_at: ts,
                    ..v.clone()
                },
            )
        })
        .collect::<Vec<_>>();
    resources1.sort_by_key(|(k, _v)| k.clone());
    check!(
        resources1
            == vec![(
                "0".to_string(),
                WorkerResourceDescription {
                    created_at: ts,
                    resource_owner: "rpc:counters-exports/api".to_string(),
                    resource_name: "counter".to_string(),
                }
            ),]
    );

    let resources2 = metadata2
        .last_known_status
        .owned_resources
        .iter()
        .map(|(k, v)| {
            (
                k.to_string(),
                WorkerResourceDescription {
                    created_at: ts,
                    ..v.clone()
                },
            )
        })
        .collect::<Vec<_>>();
    check!(resources2 == vec![]);
}

#[test]
#[tracing::instrument]
#[timeout(120000)]
async fn counter_resource_test_2_json_no_types(
    deps: &EnvBasedTestDependencies,
    _tracing: &Tracing,
) {
    let admin = deps.admin().await;
    let component_id = admin.component("counters").unique().store().await;
    let worker_id = admin.start_worker(&component_id, "counters-2j").await;
    admin.log_output(&worker_id).await;

    let counter1 = admin
        .invoke_and_await_json(
            &worker_id,
            "rpc:counters-exports/api.{[constructor]counter}",
            vec![json!({ "typ": { "type": "Str" }, "value": "counter1" })],
        )
        .await
        .unwrap();

    let counter1_value = counter1.as_object().unwrap().get("value").unwrap();
    let counter1 = json!(
        {
            "value": counter1_value
        }
    );

    let counter2 = admin
        .invoke_and_await_json(
            &worker_id,
            "rpc:counters-exports/api.{[constructor]counter}",
            vec![json!({ "typ": { "type": "Str" }, "value": "counter2" })],
        )
        .await
        .unwrap();

    let counter2_value = counter2.as_object().unwrap().get("value").unwrap();
    let counter2 = json!(
        {
            "value": counter2_value
        }
    );

    let _ = admin
        .invoke_and_await_json(
            &worker_id,
            "rpc:counters-exports/api.{[method]counter.inc-by}",
            vec![
                counter1.clone(),
                json!(
                    {
                        "value": 5
                    }
                ),
            ],
        )
        .await;

    let _ = admin
        .invoke_and_await_json(
            &worker_id,
            "rpc:counters-exports/api.{[method]counter.inc-by}",
            vec![
                counter2.clone(),
                json!(
                    {
                        "value": 1
                    }
                ),
            ],
        )
        .await;
    let _ = admin
        .invoke_and_await_json(
            &worker_id,
            "rpc:counters-exports/api.{[method]counter.inc-by}",
            vec![
                counter2.clone(),
                json!(
                    {
                        "value": 2
                    }
                ),
            ],
        )
        .await;

    let result1 = admin
        .invoke_and_await_json(
            &worker_id,
            "rpc:counters-exports/api.{[method]counter.get-value}",
            vec![counter1.clone()],
        )
        .await;
    let result2 = admin
        .invoke_and_await_json(
            &worker_id,
            "rpc:counters-exports/api.{[method]counter.get-value}",
            vec![counter2.clone()],
        )
        .await;

    let _ = admin
        .invoke_and_await_json(
            &worker_id,
            "rpc:counters-exports/api.{[drop]counter}",
            vec![counter1.clone()],
        )
        .await;
    let _ = admin
        .invoke_and_await_json(
            &worker_id,
            "rpc:counters-exports/api.{[drop]counter}",
            vec![counter2.clone()],
        )
        .await;

    let result3 = admin
        .invoke_and_await_json(
            &worker_id,
            "rpc:counters-exports/api.{get-all-dropped}",
            vec![],
        )
        .await;

    check!(
        result1
            == Ok(json!(
                {
                    "typ": {
                        "type": "U64",
                    },
                    "value": 5
                }
            ))
    );
    check!(
        result2
            == Ok(json!(
                {
                    "typ": {
                        "type": "U64",
                    },
                    "value": 3
                }
            ))
    );

    check!(
        result3
            == Ok(json!({
              "typ": {
                    "type": "List",
                    "inner": {
                      "type": "Tuple",
                      "items": [
                        {
                          "type": "Str"
                        },
                        {
                          "type": "U64"
                        }
                      ]
                    }
              },
              "value": [
                ["counter1",5],
                ["counter2",3]
              ]
            }))
    );
}

#[test]
#[tracing::instrument]
#[timeout(120000)]
async fn shopping_cart_example(deps: &EnvBasedTestDependencies, _tracing: &Tracing) {
    let admin = deps.admin().await;
    let component_id = admin.component("shopping-cart").store().await;
    let worker_id = admin.start_worker(&component_id, "shopping-cart-1").await;

    let _ = admin
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{initialize-cart}",
            vec!["test-user-1".into_value_and_type()],
        )
        .await;

    let _ = admin
        .invoke_and_await(
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
        .await;

    let _ = admin
        .invoke_and_await(
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
        .await;

    let _ = admin
        .invoke_and_await(
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
        .await;

    let _ = admin
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{update-item-quantity}",
            vec!["G1002".into_value_and_type(), 20u32.into_value_and_type()],
        )
        .await;

    let contents = admin
        .invoke_and_await(&worker_id, "golem:it/api.{get-cart-contents}", vec![])
        .await;

    let _ = admin
        .invoke_and_await(&worker_id, "golem:it/api.{checkout}", vec![])
        .await;

    check!(
        contents
            == Ok(vec![Value::List(vec![
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
            ])])
    );
}

#[test]
#[tracing::instrument]
#[timeout(120000)]
async fn auction_example_1(deps: &EnvBasedTestDependencies, _tracing: &Tracing) {
    let admin = deps.admin().await;
    let registry_component_id = admin.component("auction_registry_composed").store().await;
    let auction_component_id = admin.component("auction").store().await;

    let mut env = HashMap::new();
    env.insert(
        "AUCTION_COMPONENT_ID".to_string(),
        auction_component_id.to_string(),
    );
    let registry_worker_id = admin
        .start_worker_with(
            &registry_component_id,
            "auction-registry-1",
            vec![],
            env,
            vec![],
        )
        .await;

    let _ = admin.log_output(&registry_worker_id).await;

    let expiration = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_secs();

    let mut create_results = vec![];

    for _ in 1..100 {
        let create_auction_result = admin
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

    let get_auctions_result = admin
        .invoke_and_await(
            &registry_worker_id,
            "auction:registry-exports/api.{get-auctions}",
            vec![],
        )
        .await;

    info!("result: {create_results:?}");
    info!("result: {get_auctions_result:?}");

    check!(create_results.iter().all(|r| r.is_ok()));
}

fn get_worker_ids(workers: Vec<(WorkerMetadata, Option<String>)>) -> HashSet<WorkerId> {
    workers
        .into_iter()
        .map(|w| w.0.worker_id)
        .collect::<HashSet<WorkerId>>()
}

#[test]
#[tracing::instrument]
#[timeout(120000)]
async fn get_workers(deps: &EnvBasedTestDependencies, _tracing: &Tracing) {
    let admin = deps.admin().await;
    let component_id = admin.component("shopping-cart").store().await;

    let workers_count = 150;
    let mut worker_ids = HashSet::new();

    for i in 0..workers_count {
        let worker_id = admin
            .start_worker(&component_id, &format!("get-workers-test-{i}"))
            .await;

        worker_ids.insert(worker_id);
    }

    let check_worker_ids = worker_ids
        .iter()
        .choose_multiple(&mut rand::rng(), workers_count / 10);

    for worker_id in check_worker_ids {
        let _ = admin
            .invoke_and_await(
                worker_id,
                "golem:it/api.{initialize-cart}",
                vec!["test-user-1".into_value_and_type()],
            )
            .await;

        let (cursor, values) = admin
            .get_workers_metadata(
                &component_id,
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
            .await;

        check!(values.len() == 1);
        check!(cursor.is_none());

        let ids = get_worker_ids(values);

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
        let (cursor1, values1) = admin
            .get_workers_metadata(
                &component_id,
                filter.clone(),
                cursor.unwrap(),
                count as u64,
                true,
            )
            .await;

        check!(values1.len() > 0); // Each page should contain at least one element, but it is not guaranteed that it has count elements

        found_worker_ids.extend(get_worker_ids(values1.clone()));

        cursor = cursor1;
    }

    check!(found_worker_ids.eq(&worker_ids));

    if let Some(cursor) = cursor {
        let (_, values) = admin
            .get_workers_metadata(&component_id, filter, cursor, workers_count as u64, true)
            .await;
        check!(values.len() == 0);
    }
}

#[test]
#[tracing::instrument]
#[timeout("5m")]
async fn get_running_workers(deps: &EnvBasedTestDependencies, _tracing: &Tracing) {
    let admin = deps.admin().await;
    let component_id = admin.component("http-client-2").unique().store().await;

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
        let worker_id = admin
            .start_worker_with(
                &component_id,
                &format!("worker-http-client-{i}"),
                vec![],
                env.clone(),
                vec![],
            )
            .await;

        worker_ids.insert(worker_id);
    }

    for worker_id in &worker_ids {
        let _ = admin
            .invoke(
                worker_id,
                "golem:it/api.{start-polling}",
                vec!["stop".into_value_and_type()],
            )
            .await;
        admin
            .wait_for_status(worker_id, WorkerStatus::Running, Duration::from_secs(10))
            .await;
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
        let (next_cursor, values) = admin
            .get_workers_metadata(
                &component_id,
                Some(WorkerFilter::new_name(
                    StringFilterComparator::Equal,
                    worker_ids.iter().next().unwrap().worker_name.clone(),
                )),
                cursor,
                1,
                true,
            )
            .await;
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
        let (next_cursor, values) = admin
            .get_workers_metadata(&component_id, None, cursor, workers_count, true)
            .await;
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
        let (next_cursor, values) = admin
            .get_workers_metadata(
                &component_id,
                Some(WorkerFilter::new_status(
                    FilterComparator::Equal,
                    WorkerStatus::Running,
                )),
                cursor,
                workers_count,
                true,
            )
            .await;
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
        admin
            .wait_for_status(worker_id, WorkerStatus::Idle, Duration::from_secs(10))
            .await;
        admin.delete_worker(worker_id).await;
    }

    http_server.abort();
}

#[test]
#[tracing::instrument]
#[timeout(300000)]
async fn auto_update_on_idle(deps: &EnvBasedTestDependencies, _tracing: &Tracing) {
    let admin = deps.admin().await;
    let component_id = admin.component("update-test-v1").unique().store().await;
    let worker_id = admin
        .start_worker(&component_id, "auto_update_on_idle")
        .await;
    let _ = admin.log_output(&worker_id).await;

    let target_version = admin
        .update_component(&component_id, "update-test-v2")
        .await;
    info!("Updated component to version {target_version}");

    admin.auto_update_worker(&worker_id, target_version).await;

    let result = admin
        .invoke_and_await(&worker_id, "golem:component/api.{f2}", vec![])
        .await
        .unwrap();

    info!("result: {result:?}");
    let (metadata, _) = admin.get_worker_metadata(&worker_id).await.unwrap();

    // Expectation: the worker has no history so the update succeeds and then calling f2 returns
    // the current state which is 0
    check!(result[0] == Value::U64(0));
    check!(metadata.last_known_status.component_version == target_version);
    check!(metadata.last_known_status.pending_updates.is_empty());
    check!(metadata.last_known_status.failed_updates.is_empty());
    check!(metadata.last_known_status.successful_updates.len() == 1);
}

#[test]
#[tracing::instrument]
#[timeout(300000)]
async fn auto_update_on_idle_via_host_function(
    deps: &EnvBasedTestDependencies,
    _tracing: &Tracing,
) {
    let admin = deps.admin().await;
    let component_id = admin.component("update-test-v1").unique().store().await;
    let worker_id = admin
        .start_worker(&component_id, "auto_update_on_idle_via_host_function")
        .await;
    let _ = admin.log_output(&worker_id).await;

    let target_version = admin
        .update_component(&component_id, "update-test-v2")
        .await;
    info!("Updated component to version {target_version}");

    let runtime_svc = admin.component("runtime-service").store().await;
    let runtime_svc_worker = WorkerId {
        component_id: runtime_svc,
        worker_name: "runtime-service".to_string(),
    };

    let (high_bits, low_bits) = worker_id.component_id.0.as_u64_pair();
    admin
        .invoke_and_await(
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
                target_version.into_value_and_type(),
                ValueAndType {
                    value: Value::Enum(0),
                    typ: analysed_type::r#enum(&["automatic", "snapshot-based"]),
                },
            ],
        )
        .await
        .unwrap();

    let result = admin
        .invoke_and_await(&worker_id, "golem:component/api.{f2}", vec![])
        .await
        .unwrap();

    let (metadata, _) = admin.get_worker_metadata(&worker_id).await.unwrap();

    // Expectation: the worker has no history so the update succeeds and then calling f2 returns
    // the current state which is 0
    check!(result[0] == Value::U64(0));
    check!(metadata.last_known_status.component_version == target_version);
    check!(metadata.last_known_status.pending_updates.is_empty());
    check!(metadata.last_known_status.failed_updates.is_empty());
    check!(metadata.last_known_status.successful_updates.len() == 1);
}

#[test]
#[tracing::instrument]
#[timeout(120000)]
async fn get_oplog_1(deps: &EnvBasedTestDependencies, _tracing: &Tracing) {
    let admin = deps.admin().await;
    let component_id = admin.component("runtime-service").store().await;

    let worker_id = WorkerId {
        component_id,
        worker_name: "getoplog1".to_string(),
    };

    let idempotency_key1 = IdempotencyKey::fresh();
    let idempotency_key2 = IdempotencyKey::fresh();

    let _ = admin
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{generate-idempotency-keys}",
            vec![],
        )
        .await
        .unwrap();
    let _ = admin
        .invoke_and_await_with_key(
            &worker_id,
            &idempotency_key1,
            "golem:it/api.{generate-idempotency-keys}",
            vec![],
        )
        .await
        .unwrap();
    let _ = admin
        .invoke_and_await_with_key(
            &worker_id,
            &idempotency_key2,
            "golem:it/api.{generate-idempotency-keys}",
            vec![],
        )
        .await
        .unwrap();

    let oplog = admin.get_oplog(&worker_id, OplogIndex::INITIAL).await;

    assert_eq!(oplog.len(), 16);
    assert_eq!(oplog[0].oplog_index, OplogIndex::INITIAL);
    assert!(matches!(&oplog[0].entry, PublicOplogEntry::Create(_)));
    assert_eq!(
        oplog
            .iter()
            .filter(
                |entry| matches!(&entry.entry, PublicOplogEntry::ExportedFunctionInvoked(
        ExportedFunctionInvokedParameters { function_name, .. }
    ) if function_name == "golem:it/api.{generate-idempotency-keys}")
            )
            .count(),
        3
    );
}

#[test]
#[tracing::instrument]
#[timeout(120000)]
async fn search_oplog_1(deps: &EnvBasedTestDependencies, _tracing: &Tracing) {
    let admin = deps.admin().await;
    let component_id = admin.component("shopping-cart").store().await;

    let worker_id = WorkerId {
        component_id,
        worker_name: "searchoplog1".to_string(),
    };

    let _ = admin
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{initialize-cart}",
            vec!["test-user-1".into_value_and_type()],
        )
        .await;

    let _ = admin
        .invoke_and_await(
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
        .await;

    let _ = admin
        .invoke_and_await(
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
        .await;

    let _ = admin
        .invoke_and_await(
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
        .await;

    let _ = admin
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{update-item-quantity}",
            vec!["G1002".into_value_and_type(), 20u32.into_value_and_type()],
        )
        .await;

    let _ = admin
        .invoke_and_await(&worker_id, "golem:it/api.{get-cart-contents}", vec![])
        .await;

    let _ = admin
        .invoke_and_await(&worker_id, "golem:it/api.{checkout}", vec![])
        .await;

    let _oplog = admin.get_oplog(&worker_id, OplogIndex::INITIAL).await;

    let result1 = admin.search_oplog(&worker_id, "G1002").await;

    let result2 = admin.search_oplog(&worker_id, "imported-function").await;

    let result3 = admin
        .search_oplog(&worker_id, "product-id:G1001 OR product-id:G1000")
        .await;

    assert_eq!(result1.len(), 7); // two invocations and two log messages, and the get-cart-contents results
    assert_eq!(result2.len(), 1); // get_random_bytes
    assert_eq!(result3.len(), 5); // two invocations, and the get-cart-contents results
}

#[test]
#[tracing::instrument]
#[timeout(600000)]
async fn worker_recreation(deps: &EnvBasedTestDependencies, _tracing: &Tracing) {
    let admin = deps.admin().await;
    let component_id = admin.component("counters").unique().store().await;
    let worker_id = admin
        .start_worker(&component_id, "counters-recreation")
        .await;

    let counter1 = admin
        .invoke_and_await(
            &worker_id,
            "rpc:counters-exports/api.{[constructor]counter}",
            vec!["counter1".into_value_and_type()],
        )
        .await
        .unwrap();
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
        let _ = admin
            .invoke_and_await(
                &worker_id,
                "rpc:counters-exports/api.{[method]counter.inc-by}",
                vec![counter1.clone(), 1u64.into_value_and_type()],
            )
            .await;
    }

    let result1 = admin
        .invoke_and_await(
            &worker_id,
            "rpc:counters-exports/api.{[method]counter.get-value}",
            vec![counter1.clone()],
        )
        .await;

    tokio::time::sleep(Duration::from_secs(2)).await;

    admin.delete_worker(&worker_id).await;

    // Invoking again should create a new worker
    let counter1 = admin
        .invoke_and_await(
            &worker_id,
            "rpc:counters-exports/api.{[constructor]counter}",
            vec!["counter1".into_value_and_type()],
        )
        .await
        .unwrap();
    let counter1 = ValueAndType::new(
        counter1[0].clone(),
        AnalysedType::Handle(TypeHandle {
            name: None,
            owner: None,
            resource_id: AnalysedResourceId(0),
            mode: AnalysedResourceMode::Borrowed,
        }),
    );

    let _ = admin
        .invoke_and_await(
            &worker_id,
            "rpc:counters-exports/api.{[method]counter.inc-by}",
            vec![counter1.clone(), 1u64.into_value_and_type()],
        )
        .await;

    let result2 = admin
        .invoke_and_await(
            &worker_id,
            "rpc:counters-exports/api.{[method]counter.get-value}",
            vec![counter1.clone()],
        )
        .await;

    admin.delete_worker(&worker_id).await;

    // Also if we explicitly create a new one
    let worker_id = admin
        .start_worker(&component_id, "counters-recreation")
        .await;

    let counter1 = admin
        .invoke_and_await(
            &worker_id,
            "rpc:counters-exports/api.{[constructor]counter}",
            vec!["counter1".into_value_and_type()],
        )
        .await
        .unwrap();
    let counter1 = ValueAndType::new(
        counter1[0].clone(),
        AnalysedType::Handle(TypeHandle {
            name: None,
            owner: None,
            resource_id: AnalysedResourceId(0),
            mode: AnalysedResourceMode::Borrowed,
        }),
    );

    let result3 = admin
        .invoke_and_await(
            &worker_id,
            "rpc:counters-exports/api.{[method]counter.get-value}",
            vec![counter1.clone()],
        )
        .await;

    check!(result1 == Ok(vec![Value::U64(1200)]));
    check!(result2 == Ok(vec![Value::U64(1)]));
    check!(result3 == Ok(vec![Value::U64(0)]));
}

#[test]
#[tracing::instrument]
#[timeout(600000)]
async fn worker_use_initial_files(deps: &EnvBasedTestDependencies, _tracing: &Tracing) {
    let admin = deps.admin().await;
    let component_files = admin
        .add_initial_component_files(&[
            (
                "initial-file-read-write/files/foo.txt",
                "/foo.txt",
                ComponentFilePermissions::ReadOnly,
            ),
            (
                "initial-file-read-write/files/baz.txt",
                "/bar/baz.txt",
                ComponentFilePermissions::ReadWrite,
            ),
        ])
        .await;

    let component_id = admin
        .component("initial-file-read-write")
        .unique()
        .with_files(&component_files)
        .store()
        .await;

    let worker_id = admin
        .start_worker(&component_id, "initial-file-read-write-1")
        .await;

    let result = admin
        .invoke_and_await(&worker_id, "run", vec![])
        .await
        .unwrap();

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
}

#[test]
#[tracing::instrument]
#[timeout(600000)]
async fn worker_list_files(deps: &EnvBasedTestDependencies, _tracing: &Tracing) {
    let admin = deps.admin().await;
    let component_files = admin
        .add_initial_component_files(&[
            (
                "initial-file-read-write/files/foo.txt",
                "/foo.txt",
                ComponentFilePermissions::ReadOnly,
            ),
            (
                "initial-file-read-write/files/baz.txt",
                "/bar/baz.txt",
                ComponentFilePermissions::ReadWrite,
            ),
            (
                "initial-file-read-write/files/baz.txt",
                "/baz.txt",
                ComponentFilePermissions::ReadWrite,
            ),
        ])
        .await;

    let component_id = admin
        .component("initial-file-read-write")
        .unique()
        .with_files(&component_files)
        .store()
        .await;

    let worker_id = admin
        .start_worker(&component_id, "initial-file-read-write-1")
        .await;

    let result = admin.get_file_system_node(&worker_id, "/").await;

    let mut result = result
        .into_iter()
        .map(|e| ComponentFileSystemNode {
            last_modified: SystemTime::UNIX_EPOCH,
            ..e
        })
        .collect::<Vec<_>>();

    result.sort_by_key(|e| e.name.clone());

    check!(
        result
            == vec![
                ComponentFileSystemNode {
                    name: "bar".to_string(),
                    last_modified: SystemTime::UNIX_EPOCH,
                    details: ComponentFileSystemNodeDetails::Directory
                },
                ComponentFileSystemNode {
                    name: "baz.txt".to_string(),
                    last_modified: SystemTime::UNIX_EPOCH,
                    details: ComponentFileSystemNodeDetails::File {
                        permissions: ComponentFilePermissions::ReadWrite,
                        size: 4,
                    }
                },
                ComponentFileSystemNode {
                    name: "foo.txt".to_string(),
                    last_modified: SystemTime::UNIX_EPOCH,
                    details: ComponentFileSystemNodeDetails::File {
                        permissions: ComponentFilePermissions::ReadOnly,
                        size: 4,
                    }
                },
            ]
    );

    let result = admin.get_file_system_node(&worker_id, "/bar").await;

    let mut result = result
        .into_iter()
        .map(|e| ComponentFileSystemNode {
            last_modified: SystemTime::UNIX_EPOCH,
            ..e
        })
        .collect::<Vec<_>>();

    result.sort_by_key(|e| e.name.clone());

    check!(
        result
            == vec![ComponentFileSystemNode {
                name: "baz.txt".to_string(),
                last_modified: SystemTime::UNIX_EPOCH,
                details: ComponentFileSystemNodeDetails::File {
                    permissions: ComponentFilePermissions::ReadWrite,
                    size: 4,
                }
            },]
    );

    let result = admin.get_file_system_node(&worker_id, "/baz.txt").await;

    let mut result = result
        .into_iter()
        .map(|e| ComponentFileSystemNode {
            last_modified: SystemTime::UNIX_EPOCH,
            ..e
        })
        .collect::<Vec<_>>();

    result.sort_by_key(|e| e.name.clone());

    check!(
        result
            == vec![ComponentFileSystemNode {
                name: "baz.txt".to_string(),
                last_modified: SystemTime::UNIX_EPOCH,
                details: ComponentFileSystemNodeDetails::File {
                    permissions: ComponentFilePermissions::ReadWrite,
                    size: 4,
                }
            },]
    );
}

#[test]
#[tracing::instrument]
#[timeout(600000)]
async fn worker_read_files(deps: &EnvBasedTestDependencies, _tracing: &Tracing) {
    let admin = deps.admin().await;
    let component_files = admin
        .add_initial_component_files(&[
            (
                "initial-file-read-write/files/foo.txt",
                "/foo.txt",
                ComponentFilePermissions::ReadOnly,
            ),
            (
                "initial-file-read-write/files/baz.txt",
                "/bar/baz.txt",
                ComponentFilePermissions::ReadWrite,
            ),
        ])
        .await;

    let component_id = admin
        .component("initial-file-read-write")
        .unique()
        .with_files(&component_files)
        .store()
        .await;

    let worker_id = admin
        .start_worker(&component_id, "initial-file-read-write-1")
        .await;

    // run the worker so it can update the files.
    admin
        .invoke_and_await(&worker_id, "run", vec![])
        .await
        .unwrap();

    let result1 = admin.get_file_contents(&worker_id, "/foo.txt").await;

    let result1 = std::str::from_utf8(&result1).unwrap();

    let result2 = admin.get_file_contents(&worker_id, "/bar/baz.txt").await;
    let result2 = std::str::from_utf8(&result2).unwrap();

    check!(result1 == "foo\n");
    check!(result2 == "hello world");
}

#[test]
#[tracing::instrument]
#[timeout(600000)]
async fn worker_initial_files_after_automatic_worker_update(
    deps: &EnvBasedTestDependencies,
    _tracing: &Tracing,
) {
    let admin = deps.admin().await;
    let component_files_1 = admin
        .add_initial_component_files(&[
            (
                "initial-file-read-write/files/foo.txt",
                "/foo.txt",
                ComponentFilePermissions::ReadOnly,
            ),
            (
                "initial-file-read-write/files/baz.txt",
                "/bar/baz.txt",
                ComponentFilePermissions::ReadWrite,
            ),
        ])
        .await;

    let component_id = admin
        .component("initial-file-read-write")
        .unique()
        .with_files(&component_files_1)
        .store()
        .await;

    let worker_id = admin
        .start_worker(&component_id, "initial-file-read-write-1")
        .await;

    // run the worker so it can update the files.
    admin
        .invoke_and_await(&worker_id, "run", vec![])
        .await
        .unwrap();

    let component_files_2 = admin
        .add_initial_component_files(&[
            (
                "initial-file-read-write/files/foo.txt",
                "/foo.txt",
                ComponentFilePermissions::ReadOnly,
            ),
            (
                "initial-file-read-write/files/baz.txt",
                "/bar/baz.txt",
                ComponentFilePermissions::ReadWrite,
            ),
            (
                "initial-file-read-write/files/baz.txt",
                "/baz.txt",
                ComponentFilePermissions::ReadWrite,
            ),
        ])
        .await;

    let target_version = admin
        .update_component_with_files(
            &component_id,
            "initial-file-read-write",
            Some(&component_files_2),
        )
        .await;
    admin.auto_update_worker(&worker_id, target_version).await;

    let result1 = admin.get_file_contents(&worker_id, "/foo.txt").await;

    let result1 = std::str::from_utf8(&result1).unwrap();

    let result2 = admin.get_file_contents(&worker_id, "/bar/baz.txt").await;
    let result2 = std::str::from_utf8(&result2).unwrap();

    let result3 = admin.get_file_contents(&worker_id, "/baz.txt").await;
    let result3 = std::str::from_utf8(&result3).unwrap();

    check!(result1 == "foo\n");
    check!(result2 == "hello world");
    check!(result3 == "baz\n");
}

/// Test resolving a component_id from the name.
#[test]
#[tracing::instrument]
async fn resolve_components_from_name(deps: &EnvBasedTestDependencies, _tracing: &Tracing) {
    let admin = deps.admin().await;

    // Make sure the name is unique
    let counter_component_id = admin
        .component("counters")
        .name("component-resolve-target")
        .store()
        .await;

    let resolver_component_id = admin.component("component-resolve").store().await;

    admin.start_worker(&counter_component_id, "counter-1").await;

    let resolve_worker = admin
        .start_worker(&resolver_component_id, "resolver-1")
        .await;

    let result = admin
        .invoke_and_await(
            &resolve_worker,
            "golem:it/component-resolve-api.{run}",
            vec![],
        )
        .await
        .unwrap();

    check!(result.len() == 1);

    let (high_bits, low_bits) = counter_component_id.0.as_u64_pair();
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
}

#[test]
#[tracing::instrument]
async fn agent_promise_await(
    deps: &EnvBasedTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let admin = deps.clone().into_admin().await;

    let component_id = admin
        .component("golem_it_agent_promise")
        .unique()
        .store()
        .await;

    let worker_name = "promise-agent(\"name\")";
    let worker = admin.start_worker(&component_id, worker_name).await;

    let mut result = admin
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
        let executor_clone = admin.clone();
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

    admin
        .wait_for_status(&worker, WorkerStatus::Suspended, Duration::from_secs(10))
        .await;

    admin
        .deps
        .worker_service()
        .complete_promise(
            &admin.token,
            PromiseId {
                worker_id: WorkerId {
                    component_id,
                    worker_name: worker_name.to_string(),
                },
                oplog_idx: OplogIndex::from_u64(35),
            },
            b"hello".to_vec(),
        )
        .await
        .unwrap();

    let result = task.await.unwrap();
    check!(result == Ok(vec![Value::String("hello".to_string())]));

    Ok(())
}
