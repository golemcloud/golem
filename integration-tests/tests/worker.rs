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
use golem_test_framework::config::TestDependencies;
use golem_test_framework::dsl::TestDsl;
use golem_wasm_rpc::Value;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::SystemTime;

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
