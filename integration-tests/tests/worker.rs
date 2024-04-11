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

use crate::DEPS;
use assert2::check;

use golem_test_framework::dsl::TestDsl;
use golem_wasm_rpc::Value;
use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use golem_common::model::{
    FilterComparator, StringFilterComparator, WorkerFilter, WorkerId, WorkerMetadata, WorkerStatus,
};
use rand::seq::IteratorRandom;
use std::time::SystemTime;
use warp::http::{Response, StatusCode};
use warp::hyper::Body;
use warp::Filter;

#[tokio::test]
#[tracing::instrument]
async fn shopping_cart_example() {
    let template_id = DEPS.store_template("shopping-cart").await;
    let worker_id = DEPS.start_worker(&template_id, "shopping-cart-1").await;

    let _ = DEPS
        .invoke_and_await(
            &worker_id,
            "golem:it/api/initialize-cart",
            vec![Value::String("test-user-1".to_string())],
        )
        .await;

    let _ = DEPS
        .invoke_and_await(
            &worker_id,
            "golem:it/api/add-item",
            vec![Value::Record(vec![
                Value::String("G1000".to_string()),
                Value::String("Golem T-Shirt M".to_string()),
                Value::F32(100.0),
                Value::U32(5),
            ])],
        )
        .await;

    let _ = DEPS
        .invoke_and_await(
            &worker_id,
            "golem:it/api/add-item",
            vec![Value::Record(vec![
                Value::String("G1001".to_string()),
                Value::String("Golem Cloud Subscription 1y".to_string()),
                Value::F32(999999.0),
                Value::U32(1),
            ])],
        )
        .await;

    let _ = DEPS
        .invoke_and_await(
            &worker_id,
            "golem:it/api/add-item",
            vec![Value::Record(vec![
                Value::String("G1002".to_string()),
                Value::String("Mud Golem".to_string()),
                Value::F32(11.0),
                Value::U32(10),
            ])],
        )
        .await;

    let _ = DEPS
        .invoke_and_await(
            &worker_id,
            "golem:it/api/update-item-quantity",
            vec![Value::String("G1002".to_string()), Value::U32(20)],
        )
        .await;

    let contents = DEPS
        .invoke_and_await(&worker_id, "golem:it/api/get-cart-contents", vec![])
        .await;

    let _ = DEPS
        .invoke_and_await(&worker_id, "golem:it/api/checkout", vec![])
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

#[tokio::test]
#[tracing::instrument]
async fn auction_example_1() {
    let registry_template_id = DEPS.store_template("auction_registry_composed").await;
    let auction_template_id = DEPS.store_template("auction").await;

    let mut env = HashMap::new();
    env.insert(
        "AUCTION_TEMPLATE_ID".to_string(),
        auction_template_id.to_string(),
    );
    let registry_worker_id = DEPS
        .start_worker_with(&registry_template_id, "auction-registry-1", vec![], env)
        .await;

    let _ = DEPS.log_output(&registry_worker_id).await;

    let expiration = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_secs();

    let mut create_results = vec![];

    for _ in 1..100 {
        let create_auction_result = DEPS
            .invoke_and_await(
                &registry_worker_id,
                "auction:registry/api/create-auction",
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

    let get_auctions_result = DEPS
        .invoke_and_await(
            &registry_worker_id,
            "auction:registry/api/get-auctions",
            vec![],
        )
        .await;

    println!("result: {:?}", create_results);
    println!("result: {:?}", get_auctions_result);

    check!(create_results.iter().all(|r| r.is_ok()));
}

fn get_worker_ids(workers: Vec<WorkerMetadata>) -> HashSet<WorkerId> {
    workers
        .into_iter()
        .map(|w| w.worker_id.worker_id)
        .collect::<HashSet<WorkerId>>()
}

#[tokio::test]
#[tracing::instrument]
async fn get_workers() {
    let template_id = DEPS.store_template("shopping-cart").await;

    let workers_count = 150;
    let mut worker_ids = HashSet::new();

    for i in 0..workers_count {
        let worker_id = DEPS
            .start_worker(&template_id, &format!("shopping-cart-test-{}", i))
            .await;

        worker_ids.insert(worker_id);
    }

    let check_worker_ids = worker_ids
        .iter()
        .choose_multiple(&mut rand::thread_rng(), workers_count / 10);

    for worker_id in check_worker_ids {
        let _ = DEPS
            .invoke_and_await(
                worker_id,
                "golem:it/api/initialize-cart",
                vec![Value::String("test-user-1".to_string())],
            )
            .await;

        let (cursor, values) = DEPS
            .get_workers_metadata(
                &template_id,
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
                0,
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
    let mut cursor = Some(0);

    let count = workers_count / 5;

    while found_worker_ids.len() < workers_count && cursor.is_some() {
        let (cursor1, values1) = DEPS
            .get_workers_metadata(&template_id, None, cursor.unwrap(), count as u64, true)
            .await;

        check!(values1.len() >= count);

        found_worker_ids.extend(get_worker_ids(values1.clone()));

        cursor = cursor1;
    }

    check!(found_worker_ids == worker_ids);

    if let Some(cursor) = cursor {
        let (_, values) = DEPS
            .get_workers_metadata(&template_id, None, cursor, workers_count as u64, true)
            .await;
        check!(values.len() == 0);
    }
}

#[tokio::test]
#[tracing::instrument]
async fn get_running_workers() {
    let template_id = DEPS.store_template("http-client-2").await;

    let response = Arc::new(Mutex::new("initial".to_string()));
    let response_clone = response.clone();
    let host_http_port = 8585;

    let http_server = tokio::spawn(async move {
        let route = warp::path::path("poll").and(warp::get()).map(move || {
            let body = response_clone.lock().unwrap();
            Response::builder()
                .status(StatusCode::OK)
                .body(Body::from(body.clone()))
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
        let worker_id = DEPS
            .start_worker_with(
                &template_id,
                &format!("worker-http-client-{}", i),
                vec![],
                env.clone(),
            )
            .await;

        worker_ids.insert(worker_id);
    }

    let mut found_worker_ids = HashSet::new();

    for worker_id in worker_ids.clone() {
        let _ = DEPS
            .invoke(
                &worker_id,
                "golem:it/api/start-polling",
                vec![Value::String("first".to_string())],
            )
            .await;

        let (_, values) = DEPS
            .get_workers_metadata(
                &template_id,
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
                0,
                workers_count as u64,
                true,
            )
            .await;

        found_worker_ids.extend(get_worker_ids(values));
    }

    let (_, values) = DEPS
        .get_workers_metadata(
            &template_id,
            Some(WorkerFilter::new_status(
                FilterComparator::Equal,
                WorkerStatus::Running,
            )),
            0,
            workers_count as u64,
            true,
        )
        .await;

    http_server.abort();

    check!(found_worker_ids == worker_ids);

    check!(get_worker_ids(values) == worker_ids);
}
