// Copyright 2024 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use test_r::{inherit_test_dep, test, timeout};

use assert2::check;

use golem_test_framework::dsl::TestDslUnsafe;
use golem_wasm_rpc::Value;
use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use crate::Tracing;
use golem_common::model::oplog::{OplogIndex, WorkerResourceId};
use golem_common::model::public_oplog::{ExportedFunctionInvokedParameters, PublicOplogEntry};
use golem_common::model::{
    ComponentId, FilterComparator, IdempotencyKey, ScanCursor, StringFilterComparator,
    TargetWorkerId, Timestamp, WorkerFilter, WorkerId, WorkerMetadata, WorkerResourceDescription,
    WorkerStatus,
};
use golem_test_framework::config::EnvBasedTestDependencies;
use rand::seq::IteratorRandom;
use serde_json::json;
use std::time::{Duration, SystemTime};
use tokio::time::sleep;
use tracing::log::info;
use warp::http::{Response, StatusCode};
use warp::hyper::Body;
use warp::Filter;

inherit_test_dep!(Tracing);
inherit_test_dep!(EnvBasedTestDependencies);

#[test]
#[tracing::instrument]
#[timeout(120000)]
async fn dynamic_worker_creation(deps: &EnvBasedTestDependencies, _tracing: &Tracing) {
    let component_id = deps.store_component("environment-service").await;
    let worker_id = WorkerId {
        component_id: component_id.clone(),
        worker_name: "dynamic-worker-creation-1".to_string(),
    };

    let args = deps
        .invoke_and_await(&worker_id, "golem:it/api.{get-arguments}", vec![])
        .await
        .unwrap();
    let env = deps
        .invoke_and_await(&worker_id, "golem:it/api.{get-environment}", vec![])
        .await
        .unwrap();

    check!(args == vec![Value::Result(Ok(Some(Box::new(Value::List(vec![])))))]);
    check!(
        env == vec![Value::Result(Ok(Some(Box::new(Value::List(vec![
            Value::Tuple(vec![
                Value::String("GOLEM_WORKER_NAME".to_string()),
                Value::String("dynamic-worker-creation-1".to_string())
            ]),
            Value::Tuple(vec![
                Value::String("GOLEM_COMPONENT_ID".to_string()),
                Value::String(format!("{}", component_id))
            ]),
            Value::Tuple(vec![
                Value::String("GOLEM_COMPONENT_VERSION".to_string()),
                Value::String("0".to_string())
            ]),
        ])))))]
    );
}

fn get_env_result(env: Vec<Value>) -> HashMap<String, String> {
    match env.into_iter().next() {
        Some(Value::Result(Ok(Some(inner)))) => match *inner {
            Value::List(items) => {
                let pairs = items
                    .into_iter()
                    .filter_map(|item| match item {
                        Value::Tuple(values) if values.len() == 2 => {
                            let mut iter = values.into_iter();
                            let key = iter.next();
                            let value = iter.next();
                            match (key, value) {
                                (Some(Value::String(key)), Some(Value::String(value))) => {
                                    Some((key, value))
                                }
                                _ => None,
                            }
                        }
                        _ => None,
                    })
                    .collect::<Vec<(String, String)>>();
                HashMap::from_iter(pairs)
            }
            _ => panic!("Unexpected result value"),
        },
        _ => panic!("Unexpected result value"),
    }
}

#[test]
#[tracing::instrument]
#[timeout(120000)]
async fn dynamic_worker_creation_without_name(deps: &EnvBasedTestDependencies, _tracing: &Tracing) {
    let component_id = deps.store_component("environment-service").await;
    let worker_id = TargetWorkerId {
        component_id: component_id.clone(),
        worker_name: None,
    };

    let env1 = deps
        .invoke_and_await(worker_id.clone(), "golem:it/api.{get-environment}", vec![])
        .await
        .unwrap();
    let env2 = deps
        .invoke_and_await(worker_id.clone(), "golem:it/api.{get-environment}", vec![])
        .await
        .unwrap();

    let env1 = get_env_result(env1);
    let env2 = get_env_result(env2);

    check!(env1.contains_key("GOLEM_WORKER_NAME"));
    check!(env1.get("GOLEM_COMPONENT_ID") == Some(&component_id.to_string()));
    check!(env1.get("GOLEM_COMPONENT_VERSION") == Some(&"0".to_string()));
    check!(env2.contains_key("GOLEM_WORKER_NAME"));
    check!(env2.get("GOLEM_COMPONENT_ID") == Some(&component_id.to_string()));
    check!(env2.get("GOLEM_COMPONENT_VERSION") == Some(&"0".to_string()));
    check!(env1.get("GOLEM_WORKER_NAME") != env2.get("GOLEM_WORKER_NAME"));
}

#[test]
#[tracing::instrument]
#[timeout(120000)]
async fn ephemeral_worker_creation_without_name(
    deps: &EnvBasedTestDependencies,
    _tracing: &Tracing,
) {
    let component_id = deps.store_ephemeral_component("environment-service").await;
    let worker_id = TargetWorkerId {
        component_id: component_id.clone(),
        worker_name: None,
    };

    let env1 = deps
        .invoke_and_await(worker_id.clone(), "golem:it/api.{get-environment}", vec![])
        .await
        .unwrap();
    let env2 = deps
        .invoke_and_await(worker_id.clone(), "golem:it/api.{get-environment}", vec![])
        .await
        .unwrap();

    let env1 = get_env_result(env1);
    let env2 = get_env_result(env2);

    check!(env1.contains_key("GOLEM_WORKER_NAME"));
    check!(env1.get("GOLEM_COMPONENT_ID") == Some(&component_id.to_string()));
    check!(env1.get("GOLEM_COMPONENT_VERSION") == Some(&"0".to_string()));
    check!(env2.contains_key("GOLEM_WORKER_NAME"));
    check!(env2.get("GOLEM_COMPONENT_ID") == Some(&component_id.to_string()));
    check!(env2.get("GOLEM_COMPONENT_VERSION") == Some(&"0".to_string()));
    check!(env1.get("GOLEM_WORKER_NAME") != env2.get("GOLEM_WORKER_NAME"));
}

#[test]
#[tracing::instrument]
#[timeout(120000)]
async fn ephemeral_worker_creation_with_name_is_not_persistent(
    deps: &EnvBasedTestDependencies,
    _tracing: &Tracing,
) {
    let component_id = deps.store_ephemeral_component("counters").await;
    let worker_id = TargetWorkerId {
        component_id: component_id.clone(),
        worker_name: Some("test".to_string()),
    };

    let _ = deps
        .invoke_and_await(
            worker_id.clone(),
            "rpc:counters/api.{inc-global-by}",
            vec![Value::U64(2)],
        )
        .await
        .unwrap();

    let result = deps
        .invoke_and_await(
            worker_id.clone(),
            "rpc:counters/api.{get-global-value}",
            vec![],
        )
        .await
        .unwrap();

    check!(result == vec![Value::U64(0)]);
}

#[test]
#[tracing::instrument]
#[timeout(120000)]
async fn counter_resource_test_1(deps: &EnvBasedTestDependencies, _tracing: &Tracing) {
    let component_id = deps.store_unique_component("counters").await;
    let worker_id = deps.start_worker(&component_id, "counters-1").await;
    deps.log_output(&worker_id).await;

    let counter1 = deps
        .invoke_and_await(
            &worker_id,
            "rpc:counters/api.{[constructor]counter}",
            vec![Value::String("counter1".to_string())],
        )
        .await
        .unwrap();

    let _ = deps
        .invoke_and_await(
            &worker_id,
            "rpc:counters/api.{[method]counter.inc-by}",
            vec![counter1[0].clone(), Value::U64(5)],
        )
        .await;

    let result1 = deps
        .invoke_and_await(
            &worker_id,
            "rpc:counters/api.{[method]counter.get-value}",
            vec![counter1[0].clone()],
        )
        .await;

    let (metadata1, _) = deps.get_worker_metadata(&worker_id).await.unwrap();

    let _ = deps
        .invoke_and_await(
            &worker_id,
            "rpc:counters/api.{[drop]counter}",
            vec![counter1[0].clone()],
        )
        .await;

    let result2 = deps
        .invoke_and_await(&worker_id, "rpc:counters/api.{get-all-dropped}", vec![])
        .await;

    let (metadata2, _) = deps.get_worker_metadata(&worker_id).await.unwrap();

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
                *k,
                WorkerResourceDescription {
                    created_at: ts,
                    ..v.clone()
                },
            )
        })
        .collect::<Vec<_>>();
    resources1.sort_by_key(|(k, _v)| *k);
    check!(
        resources1
            == vec![(
                WorkerResourceId(0),
                WorkerResourceDescription {
                    created_at: ts,
                    indexed_resource_key: None
                }
            ),]
    );

    let resources2 = metadata2
        .last_known_status
        .owned_resources
        .iter()
        .map(|(k, v)| {
            (
                *k,
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
    let component_id = deps.store_unique_component("counters").await;
    let worker_id = deps.start_worker(&component_id, "counters-1j").await;
    deps.log_output(&worker_id).await;

    let counter1 = deps
        .invoke_and_await_json(
            &worker_id,
            "rpc:counters/api.{[constructor]counter}",
            vec![json!({ "typ": { "type": "Str" }, "value": "counter1" })],
        )
        .await
        .unwrap();

    // counter1 is a JSON response containing a tuple with a single element holding the resource handle:
    // {
    //   "typ":  {"items":[{"mode":{"type":"Owned"},"resource_id":0,"type":"Handle"}],"type":"Tuple"},
    //   "value":["urn:worker:fcb5d2d4-d6db-4eca-99ec-6260ae9270db/CLI_short_name_worker_invoke_and_await/0"]}
    // }
    // we only need this inner element and it's type:

    let counter1_type = counter1
        .as_object()
        .unwrap()
        .get("typ")
        .unwrap()
        .as_object()
        .unwrap()
        .get("items")
        .unwrap()
        .as_array()
        .unwrap()
        .first()
        .unwrap();
    let counter1_value = counter1
        .as_object()
        .unwrap()
        .get("value")
        .unwrap()
        .as_array()
        .unwrap()
        .first()
        .unwrap();
    let counter1 = json!(
        {
            "typ": counter1_type,
            "value": counter1_value
        }
    );

    info!("Using counter1 resource handle {counter1}");

    let _ = deps
        .invoke_and_await_json(
            &worker_id,
            "rpc:counters/api.{[method]counter.inc-by}",
            vec![
                counter1.clone(),
                json!({ "typ": { "type": "U64"}, "value": 5 }),
            ],
        )
        .await;

    let result1 = deps
        .invoke_and_await_json(
            &worker_id,
            "rpc:counters/api.{[method]counter.get-value}",
            vec![counter1.clone()],
        )
        .await;

    let (metadata1, _) = deps.get_worker_metadata(&worker_id).await.unwrap();

    let _ = deps
        .invoke_and_await_json(
            &worker_id,
            "rpc:counters/api.{[drop]counter}",
            vec![counter1.clone()],
        )
        .await;

    let result2 = deps
        .invoke_and_await_json(&worker_id, "rpc:counters/api.{get-all-dropped}", vec![])
        .await;

    let (metadata2, _) = deps.get_worker_metadata(&worker_id).await.unwrap();

    check!(
        result1
            == Ok(json!(
                {
                    "typ": {
                        "type": "Tuple",
                        "items": [ { "type": "U64" } ]
                    },
                    "value": [5]
                }
            ))
    );

    check!(
        result2
            == Ok(json!(
            {
              "typ": {
                "type": "Tuple",
                "items": [
                  {
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
                  }
                ]
              },
                "value": [
                            [
                                ["counter1",5]
                            ]
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
                *k,
                WorkerResourceDescription {
                    created_at: ts,
                    ..v.clone()
                },
            )
        })
        .collect::<Vec<_>>();
    resources1.sort_by_key(|(k, _v)| *k);
    check!(
        resources1
            == vec![(
                WorkerResourceId(0),
                WorkerResourceDescription {
                    created_at: ts,
                    indexed_resource_key: None
                }
            ),]
    );

    let resources2 = metadata2
        .last_known_status
        .owned_resources
        .iter()
        .map(|(k, v)| {
            (
                *k,
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
async fn counter_resource_test_2(deps: &EnvBasedTestDependencies, _tracing: &Tracing) {
    let component_id = deps.store_unique_component("counters").await;
    let worker_id = deps.start_worker(&component_id, "counters-2").await;
    deps.log_output(&worker_id).await;

    let _ = deps
        .invoke_and_await(
            &worker_id,
            "rpc:counters/api.{counter(\"counter1\").inc-by}",
            vec![Value::U64(5)],
        )
        .await;

    let _ = deps
        .invoke_and_await(
            &worker_id,
            "rpc:counters/api.{counter(\"counter2\").inc-by}",
            vec![Value::U64(1)],
        )
        .await;
    let _ = deps
        .invoke_and_await(
            &worker_id,
            "rpc:counters/api.{counter(\"counter2\").inc-by}",
            vec![Value::U64(2)],
        )
        .await;

    let result1 = deps
        .invoke_and_await(
            &worker_id,
            "rpc:counters/api.{counter(\"counter1\").get-value}",
            vec![],
        )
        .await;
    let result2 = deps
        .invoke_and_await(
            &worker_id,
            "rpc:counters/api.{counter(\"counter2\").get-value}",
            vec![],
        )
        .await;

    let _ = deps
        .invoke_and_await(
            &worker_id,
            "rpc:counters/api.{counter(\"counter1\").drop}",
            vec![],
        )
        .await;
    let _ = deps
        .invoke_and_await(
            &worker_id,
            "rpc:counters/api.{counter(\"counter2\").drop}",
            vec![],
        )
        .await;

    let result3 = deps
        .invoke_and_await(&worker_id, "rpc:counters/api.{get-all-dropped}", vec![])
        .await;

    check!(result1 == Ok(vec![Value::U64(5)]));
    check!(result2 == Ok(vec![Value::U64(3)]));
    check!(
        result3
            == Ok(vec![Value::List(vec![
                Value::Tuple(vec![Value::String("counter1".to_string()), Value::U64(5)]),
                Value::Tuple(vec![Value::String("counter2".to_string()), Value::U64(3)])
            ])])
    );
}

#[test]
#[tracing::instrument]
#[timeout(120000)]
async fn counter_resource_test_2_json(deps: &EnvBasedTestDependencies, _tracing: &Tracing) {
    let component_id = deps.store_unique_component("counters").await;
    let worker_id = deps.start_worker(&component_id, "counters-2j").await;
    deps.log_output(&worker_id).await;

    let _ = deps
        .invoke_and_await_json(
            &worker_id,
            "rpc:counters/api.{counter(\"counter1\").inc-by}",
            vec![json!(
                {
                    "typ": { "type": "U64" }, "value": 5
                }
            )],
        )
        .await;

    let _ = deps
        .invoke_and_await_json(
            &worker_id,
            "rpc:counters/api.{counter(\"counter2\").inc-by}",
            vec![json!(
                {
                    "typ": { "type": "U64" }, "value": 1
                }
            )],
        )
        .await;
    let _ = deps
        .invoke_and_await_json(
            &worker_id,
            "rpc:counters/api.{counter(\"counter2\").inc-by}",
            vec![json!(
                {
                    "typ": { "type": "U64" }, "value": 2
                }
            )],
        )
        .await;

    let result1 = deps
        .invoke_and_await_json(
            &worker_id,
            "rpc:counters/api.{counter(\"counter1\").get-value}",
            vec![],
        )
        .await;
    let result2 = deps
        .invoke_and_await_json(
            &worker_id,
            "rpc:counters/api.{counter(\"counter2\").get-value}",
            vec![],
        )
        .await;

    let _ = deps
        .invoke_and_await_json(
            &worker_id,
            "rpc:counters/api.{counter(\"counter1\").drop}",
            vec![],
        )
        .await;
    let _ = deps
        .invoke_and_await_json(
            &worker_id,
            "rpc:counters/api.{counter(\"counter2\").drop}",
            vec![],
        )
        .await;

    let result3 = deps
        .invoke_and_await_json(&worker_id, "rpc:counters/api.{get-all-dropped}", vec![])
        .await;

    check!(
        result1
            == Ok(json!(
                {
                    "typ": {
                        "type": "Tuple",
                        "items": [ { "type": "U64" } ]
                    },
                    "value": [5]
                }
            ))
    );
    check!(
        result2
            == Ok(json!(
                {
                    "typ": {
                        "type": "Tuple",
                        "items": [ { "type": "U64" } ]
                    },
                    "value": [3]
                }
            ))
    );

    check!(
        result3
            == Ok(json!(
                {
              "typ": {
                "type": "Tuple",
                "items": [
                  {
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
                  }
                ]
              },
                "value": [
                            [
                                ["counter1",5],
                                ["counter2",3]
                            ]
                        ]
            }
            ))
    );
}

#[test]
#[tracing::instrument]
#[timeout(120000)]
async fn shopping_cart_example(deps: &EnvBasedTestDependencies, _tracing: &Tracing) {
    let component_id = deps.store_component("shopping-cart").await;
    let worker_id = deps.start_worker(&component_id, "shopping-cart-1").await;

    let _ = deps
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{initialize-cart}",
            vec![Value::String("test-user-1".to_string())],
        )
        .await;

    let _ = deps
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{add-item}",
            vec![Value::Record(vec![
                Value::String("G1000".to_string()),
                Value::String("Golem T-Shirt M".to_string()),
                Value::F32(100.0),
                Value::U32(5),
            ])],
        )
        .await;

    let _ = deps
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{add-item}",
            vec![Value::Record(vec![
                Value::String("G1001".to_string()),
                Value::String("Golem Cloud Subscription 1y".to_string()),
                Value::F32(999999.0),
                Value::U32(1),
            ])],
        )
        .await;

    let _ = deps
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{add-item}",
            vec![Value::Record(vec![
                Value::String("G1002".to_string()),
                Value::String("Mud Golem".to_string()),
                Value::F32(11.0),
                Value::U32(10),
            ])],
        )
        .await;

    let _ = deps
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{update-item-quantity}",
            vec![Value::String("G1002".to_string()), Value::U32(20)],
        )
        .await;

    let contents = deps
        .invoke_and_await(&worker_id, "golem:it/api.{get-cart-contents}", vec![])
        .await;

    let _ = deps
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
    let registry_component_id = deps.store_component("auction_registry_composed").await;
    let auction_component_id = deps.store_component("auction").await;

    let mut env = HashMap::new();
    env.insert(
        "AUCTION_COMPONENT_ID".to_string(),
        auction_component_id.to_string(),
    );
    let registry_worker_id = deps
        .start_worker_with(&registry_component_id, "auction-registry-1", vec![], env)
        .await;

    let _ = deps.log_output(&registry_worker_id).await;

    let expiration = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_secs();

    let mut create_results = vec![];

    for _ in 1..100 {
        let create_auction_result = deps
            .invoke_and_await(
                &registry_worker_id,
                "auction:registry/api.{create-auction}",
                vec![
                    Value::String("test-auction".to_string()),
                    Value::String("this is a test".to_string()),
                    Value::F32(100.0),
                    Value::U64(expiration + 600),
                ],
            )
            .await;

        create_results.push(create_auction_result);
    }

    let get_auctions_result = deps
        .invoke_and_await(
            &registry_worker_id,
            "auction:registry/api.{get-auctions}",
            vec![],
        )
        .await;

    println!("result: {:?}", create_results);
    println!("result: {:?}", get_auctions_result);

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
    let component_id = deps.store_component("shopping-cart").await;

    let workers_count = 150;
    let mut worker_ids = HashSet::new();

    for i in 0..workers_count {
        let worker_id = deps
            .start_worker(&component_id, &format!("get-workers-test-{}", i))
            .await;

        worker_ids.insert(worker_id);
    }

    let check_worker_ids = worker_ids
        .iter()
        .choose_multiple(&mut rand::thread_rng(), workers_count / 10);

    for worker_id in check_worker_ids {
        let _ = deps
            .invoke_and_await(
                worker_id,
                "golem:it/api.{initialize-cart}",
                vec![Value::String("test-user-1".to_string())],
            )
            .await;

        let (cursor, values) = deps
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
        let (cursor1, values1) = deps
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
        let (_, values) = deps
            .get_workers_metadata(&component_id, filter, cursor, workers_count as u64, true)
            .await;
        check!(values.len() == 0);
    }
}

#[test]
#[tracing::instrument]
#[timeout(120000)]
async fn get_running_workers(deps: &EnvBasedTestDependencies, _tracing: &Tracing) {
    let component_id = deps.store_unique_component("http-client-2").await;
    let host_http_port = 8585;

    let polling_worker_ids: Arc<Mutex<HashSet<WorkerId>>> = Arc::new(Mutex::new(HashSet::new()));
    let polling_worker_ids_clone = polling_worker_ids.clone();

    let http_server = tokio::spawn(async move {
        let route = warp::path::path("poll")
            .and(warp::get())
            .and(warp::query::<HashMap<String, String>>())
            .map(move |query: HashMap<String, String>| {
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
                Response::builder()
                    .status(StatusCode::OK)
                    .body(Body::from("initial"))
                    .unwrap()
            });

        warp::serve(route)
            .run(
                format!("0.0.0.0:{}", host_http_port)
                    .parse::<SocketAddr>()
                    .unwrap(),
            )
            .await;
    });

    let mut env = HashMap::new();
    env.insert("PORT".to_string(), host_http_port.to_string());

    let workers_count = 15;
    let mut worker_ids = HashSet::new();

    for i in 0..workers_count {
        let worker_id = deps
            .start_worker_with(
                &component_id,
                &format!("worker-http-client-{}", i),
                vec![],
                env.clone(),
            )
            .await;

        worker_ids.insert(worker_id);
    }

    let mut found_worker_ids = HashSet::new();

    for worker_id in worker_ids.clone() {
        let _ = deps
            .invoke(
                &worker_id,
                "golem:it/api.{start-polling}",
                vec![Value::String("first".to_string())],
            )
            .await;
    }

    let mut wait_counter = 0;

    loop {
        sleep(Duration::from_secs(2)).await;
        wait_counter += 1;
        let ids = polling_worker_ids.lock().unwrap().clone();

        if worker_ids.eq(&ids) || wait_counter >= 10 {
            break;
        }
    }

    for worker_id in worker_ids.clone() {
        let (_, values) = deps
            .get_workers_metadata(
                &component_id,
                Some(
                    WorkerFilter::new_name(
                        StringFilterComparator::Equal,
                        worker_id.worker_name.clone(),
                    )
                    .and(WorkerFilter::new_status(
                        FilterComparator::Equal,
                        WorkerStatus::Running,
                    )),
                ),
                ScanCursor::default(),
                workers_count as u64,
                true,
            )
            .await;

        found_worker_ids.extend(get_worker_ids(values));
    }

    let (_, values) = deps
        .get_workers_metadata(
            &component_id,
            Some(WorkerFilter::new_status(
                FilterComparator::Equal,
                WorkerStatus::Running,
            )),
            ScanCursor::default(),
            workers_count as u64,
            true,
        )
        .await;

    http_server.abort();

    check!(found_worker_ids.len() == workers_count);
    check!(&found_worker_ids == &worker_ids);

    let found_worker_ids2 = get_worker_ids(values);

    check!(found_worker_ids2.len() == workers_count);
    check!(&found_worker_ids2 == &worker_ids);
}

#[test]
#[tracing::instrument]
#[timeout(300000)]
async fn auto_update_on_idle(deps: &EnvBasedTestDependencies, _tracing: &Tracing) {
    let component_id = deps.store_unique_component("update-test-v1").await;
    let worker_id = deps
        .start_worker(&component_id, "auto_update_on_idle")
        .await;
    let _ = deps.log_output(&worker_id).await;

    let target_version = deps.update_component(&component_id, "update-test-v2").await;
    info!("Updated component to version {target_version}");

    deps.auto_update_worker(&worker_id, target_version).await;

    let result = deps
        .invoke_and_await(&worker_id, "golem:component/api.{f2}", vec![])
        .await
        .unwrap();

    info!("result: {:?}", result);
    let (metadata, _) = deps.get_worker_metadata(&worker_id).await.unwrap();

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
    let component_id = deps.store_unique_component("update-test-v1").await;
    let worker_id = deps
        .start_worker(&component_id, "auto_update_on_idle_via_host_function")
        .await;
    let _ = deps.log_output(&worker_id).await;

    let target_version = deps.update_component(&component_id, "update-test-v2").await;
    info!("Updated component to version {target_version}");

    let runtime_svc = deps.store_component("runtime-service").await;
    let runtime_svc_worker = WorkerId {
        component_id: runtime_svc,
        worker_name: "runtime-service".to_string(),
    };
    deps.invoke_and_await(
        &runtime_svc_worker,
        "golem:it/api.{update-worker}",
        vec![
            Value::Record(vec![
                Value::Record(vec![Value::Record(vec![
                    Value::U64(worker_id.component_id.0.as_u64_pair().0),
                    Value::U64(worker_id.component_id.0.as_u64_pair().1),
                ])]),
                Value::String(worker_id.worker_name.clone()),
            ]),
            Value::U64(target_version),
            Value::Enum(0),
        ],
    )
    .await
    .unwrap();

    let result = deps
        .invoke_and_await(&worker_id, "golem:component/api.{f2}", vec![])
        .await
        .unwrap();

    info!("result: {:?}", result);
    let (metadata, _) = deps.get_worker_metadata(&worker_id).await.unwrap();

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
    let component_id = deps.store_component("runtime-service").await;

    let worker_id = WorkerId {
        component_id,
        worker_name: "getoplog1".to_string(),
    };

    let idempotency_key1 = IdempotencyKey::fresh();
    let idempotency_key2 = IdempotencyKey::fresh();

    let _ = deps
        .invoke_and_await(
            worker_id.clone(),
            "golem:it/api.{generate-idempotency-keys}",
            vec![],
        )
        .await
        .unwrap();
    let _ = deps
        .invoke_and_await_with_key(
            worker_id.clone(),
            &idempotency_key1,
            "golem:it/api.{generate-idempotency-keys}",
            vec![],
        )
        .await
        .unwrap();
    let _ = deps
        .invoke_and_await_with_key(
            worker_id.clone(),
            &idempotency_key2,
            "golem:it/api.{generate-idempotency-keys}",
            vec![],
        )
        .await
        .unwrap();

    let oplog = deps.get_oplog(&worker_id, OplogIndex::INITIAL).await;

    // Whether there is an "enqueued invocation" entry or just directly started invocation
    // depends on oplog
    assert!(oplog.len() >= 12 && oplog.len() <= 14);
    assert!(matches!(oplog[0], PublicOplogEntry::Create(_)));
    assert_eq!(
        oplog
            .iter()
            .filter(
                |entry| matches!(entry, PublicOplogEntry::ExportedFunctionInvoked(
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
    let component_id = deps.store_component("shopping-cart").await;

    let worker_id = WorkerId {
        component_id,
        worker_name: "searchoplog1".to_string(),
    };

    let _ = deps
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{initialize-cart}",
            vec![Value::String("test-user-1".to_string())],
        )
        .await;

    let _ = deps
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{add-item}",
            vec![Value::Record(vec![
                Value::String("G1000".to_string()),
                Value::String("Golem T-Shirt M".to_string()),
                Value::F32(100.0),
                Value::U32(5),
            ])],
        )
        .await;

    let _ = deps
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{add-item}",
            vec![Value::Record(vec![
                Value::String("G1001".to_string()),
                Value::String("Golem Cloud Subscription 1y".to_string()),
                Value::F32(999999.0),
                Value::U32(1),
            ])],
        )
        .await;

    let _ = deps
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{add-item}",
            vec![Value::Record(vec![
                Value::String("G1002".to_string()),
                Value::String("Mud Golem".to_string()),
                Value::F32(11.0),
                Value::U32(10),
            ])],
        )
        .await;

    let _ = deps
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{update-item-quantity}",
            vec![Value::String("G1002".to_string()), Value::U32(20)],
        )
        .await;

    let _ = deps
        .invoke_and_await(&worker_id, "golem:it/api.{get-cart-contents}", vec![])
        .await;

    let _ = deps
        .invoke_and_await(&worker_id, "golem:it/api.{checkout}", vec![])
        .await;

    let _oplog = deps.get_oplog(&worker_id, OplogIndex::INITIAL).await;

    let result1 = deps.search_oplog(&worker_id, "G1002").await;

    let result2 = deps.search_oplog(&worker_id, "imported-function").await;

    let result3 = deps
        .search_oplog(&worker_id, "product-id:G1001 OR product-id:G1000")
        .await;

    assert_eq!(result1.len(), 4); // two invocations and two log messages
    assert_eq!(result2.len(), 2); // get_preopened_directories, get_random_bytes
    assert_eq!(result3.len(), 2); // two invocations
}

#[test]
#[tracing::instrument]
#[timeout(600000)]
async fn worker_recreation(deps: &EnvBasedTestDependencies, _tracing: &Tracing) {
    let component_id = deps.store_unique_component("counters").await;
    let worker_id = deps
        .start_worker(&component_id, "counters-recreation")
        .await;

    // Doing many requests, so parts of the oplog gets archived
    for _ in 1..=1200 {
        let _ = deps
            .invoke_and_await(
                &worker_id,
                "rpc:counters/api.{counter(\"counter1\").inc-by}",
                vec![Value::U64(1)],
            )
            .await;
    }

    let result1 = deps
        .invoke_and_await(
            &worker_id,
            "rpc:counters/api.{counter(\"counter1\").get-value}",
            vec![],
        )
        .await;

    tokio::time::sleep(Duration::from_secs(2)).await;

    deps.delete_worker(&worker_id).await;

    // Invoking again should create a new worker
    let _ = deps
        .invoke_and_await(
            &worker_id,
            "rpc:counters/api.{counter(\"counter1\").inc-by}",
            vec![Value::U64(1)],
        )
        .await;

    let result2 = deps
        .invoke_and_await(
            &worker_id,
            "rpc:counters/api.{counter(\"counter1\").get-value}",
            vec![],
        )
        .await;

    deps.delete_worker(&worker_id).await;

    // Also if we explicitly create a new one
    let worker_id = deps
        .start_worker(&component_id, "counters-recreation")
        .await;
    let result3 = deps
        .invoke_and_await(
            &worker_id,
            "rpc:counters/api.{counter(\"counter1\").get-value}",
            vec![],
        )
        .await;

    check!(result1 == Ok(vec![Value::U64(1200)]));
    check!(result2 == Ok(vec![Value::U64(1)]));
    check!(result3 == Ok(vec![Value::U64(0)]));
}
