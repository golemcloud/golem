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

use crate::common;
use assert2::check;
use golem_test_framework::dsl::{worker_error_message, TestDslUnsafe};
use golem_wasm_rpc::Value;
use std::collections::HashMap;
use std::time::SystemTime;

#[tokio::test]
#[tracing::instrument]
async fn auction_example_1() {
    let context = common::TestContext::new();
    let executor = common::start(&context).await.unwrap();

    let registry_component_id = executor.store_component("auction_registry_composed").await;
    let auction_component_id = executor.store_component("auction").await;

    let mut env = HashMap::new();
    env.insert(
        "AUCTION_COMPONENT_ID".to_string(),
        auction_component_id.to_string(),
    );
    let registry_worker_id = executor
        .start_worker_with(&registry_component_id, "auction-registry-1", vec![], env)
        .await;

    let _ = executor.log_output(&registry_worker_id).await;

    let expiration = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let create_auction_result = executor
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

    let get_auctions_result = executor
        .invoke_and_await(
            &registry_worker_id,
            "auction:registry/api.{get-auctions}",
            vec![],
        )
        .await;

    drop(executor);

    println!("result: {:?}", create_auction_result);
    println!("result: {:?}", get_auctions_result);
    check!(create_auction_result.is_ok());

    let auction_id = &create_auction_result.unwrap()[0];

    check!(
        get_auctions_result
            == Ok(vec![Value::List(vec![Value::Record(vec![
                auction_id.clone(),
                Value::String("test-auction".to_string()),
                Value::String("this is a test".to_string()),
                Value::F32(100.0),
                Value::U64(expiration + 600)
            ]),])])
    );
}

#[tokio::test]
#[tracing::instrument]
async fn auction_example_2() {
    let context = common::TestContext::new();
    let executor = common::start(&context).await.unwrap();

    let registry_component_id = executor.store_component("auction_registry_composed").await;
    let auction_component_id = executor.store_component("auction").await;

    let mut env = HashMap::new();
    env.insert(
        "AUCTION_COMPONENT_ID".to_string(),
        auction_component_id.to_string(),
    );
    let registry_worker_id = executor
        .start_worker_with(&registry_component_id, "auction-registry-2", vec![], env)
        .await;

    let _ = executor.log_output(&registry_worker_id).await;

    let expiration = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let create_auction_result = executor
        .invoke_and_await(
            &registry_worker_id,
            "auction:registry/api.{create-auction-res}",
            vec![
                Value::String("test-auction".to_string()),
                Value::String("this is a test".to_string()),
                Value::F32(100.0),
                Value::U64(expiration + 600),
            ],
        )
        .await;

    let get_auctions_result = executor
        .invoke_and_await(
            &registry_worker_id,
            "auction:registry/api.{get-auctions}",
            vec![],
        )
        .await;

    drop(executor);

    println!("result: {:?}", create_auction_result);
    println!("result: {:?}", get_auctions_result);
    check!(create_auction_result.is_ok());

    let auction_id = &create_auction_result.unwrap()[0];

    check!(
        get_auctions_result
            == Ok(vec![Value::List(vec![Value::Record(vec![
                auction_id.clone(),
                Value::String("test-auction".to_string()),
                Value::String("this is a test".to_string()),
                Value::F32(100.0),
                Value::U64(expiration + 600)
            ]),])])
    );
}

#[tokio::test]
#[tracing::instrument]
async fn counter_resource_test_1() {
    let context = common::TestContext::new();
    let executor = common::start(&context).await.unwrap();

    let counters_component_id = executor.store_component("counters").await;
    let caller_component_id = executor.store_component("caller_composed").await;

    let mut env = HashMap::new();
    env.insert(
        "COUNTERS_COMPONENT_ID".to_string(),
        counters_component_id.to_string(),
    );
    let caller_worker_id = executor
        .start_worker_with(&caller_component_id, "rpc-counters-1", vec![], env)
        .await;

    let result = executor
        .invoke_and_await(&caller_worker_id, "test1", vec![])
        .await;

    drop(executor);

    check!(
        result
            == Ok(vec![Value::List(vec![
                Value::Tuple(vec![Value::String("counter3".to_string()), Value::U64(3)]),
                Value::Tuple(vec![Value::String("counter2".to_string()), Value::U64(3)]),
                Value::Tuple(vec![Value::String("counter1".to_string()), Value::U64(3)])
            ])])
    );
}

#[tokio::test]
#[tracing::instrument]
async fn counter_resource_test_2() {
    let context = common::TestContext::new();
    let executor = common::start(&context).await.unwrap();

    let counters_component_id = executor.store_component("counters").await;
    let caller_component_id = executor.store_component("caller_composed").await;

    let mut env = HashMap::new();
    env.insert(
        "COUNTERS_COMPONENT_ID".to_string(),
        counters_component_id.to_string(),
    );
    let caller_worker_id = executor
        .start_worker_with(&caller_component_id, "rpc-counters-2", vec![], env)
        .await;

    let result1 = executor
        .invoke_and_await(&caller_worker_id, "test2", vec![])
        .await;
    let result2 = executor
        .invoke_and_await(&caller_worker_id, "test2", vec![])
        .await;

    drop(executor);

    check!(result1 == Ok(vec![Value::U64(1)]));
    check!(result2 == Ok(vec![Value::U64(2)]));
}

#[tokio::test]
#[tracing::instrument]
async fn counter_resource_test_2_with_restart() {
    let context = common::TestContext::new();
    let executor = common::start(&context).await.unwrap();

    let counters_component_id = executor.store_component("counters").await;
    let caller_component_id = executor.store_component("caller_composed").await;

    let mut env = HashMap::new();
    env.insert(
        "COUNTERS_COMPONENT_ID".to_string(),
        counters_component_id.to_string(),
    );
    let caller_worker_id = executor
        .start_worker_with(&caller_component_id, "rpc-counters-2r", vec![], env)
        .await;

    let result1 = executor
        .invoke_and_await(&caller_worker_id, "test2", vec![])
        .await;

    drop(executor);
    let executor = common::start(&context).await.unwrap();

    let result2 = executor
        .invoke_and_await(&caller_worker_id, "test2", vec![])
        .await;

    drop(executor);

    check!(result1 == Ok(vec![Value::U64(1)]));
    check!(result2 == Ok(vec![Value::U64(2)]));
}

#[tokio::test]
#[tracing::instrument]
async fn counter_resource_test_3() {
    let context = common::TestContext::new();
    let executor = common::start(&context).await.unwrap();

    let counters_component_id = executor.store_component("counters").await;
    let caller_component_id = executor.store_component("caller_composed").await;

    let mut env = HashMap::new();
    env.insert(
        "COUNTERS_COMPONENT_ID".to_string(),
        counters_component_id.to_string(),
    );
    let caller_worker_id = executor
        .start_worker_with(&caller_component_id, "rpc-counters-3", vec![], env)
        .await;

    let result1 = executor
        .invoke_and_await(&caller_worker_id, "test3", vec![])
        .await;
    let result2 = executor
        .invoke_and_await(&caller_worker_id, "test3", vec![])
        .await;

    drop(executor);

    check!(result1 == Ok(vec![Value::U64(1)]));
    check!(result2 == Ok(vec![Value::U64(2)]));
}

#[tokio::test]
#[tracing::instrument]
async fn counter_resource_test_3_with_restart() {
    let context = common::TestContext::new();
    let executor = common::start(&context).await.unwrap();

    let counters_component_id = executor.store_component("counters").await;
    let caller_component_id = executor.store_component("caller_composed").await;

    let mut env = HashMap::new();
    env.insert(
        "COUNTERS_COMPONENT_ID".to_string(),
        counters_component_id.to_string(),
    );
    let caller_worker_id = executor
        .start_worker_with(&caller_component_id, "rpc-counters-3r", vec![], env)
        .await;

    let result1 = executor
        .invoke_and_await(&caller_worker_id, "test3", vec![])
        .await;

    drop(executor);
    let executor = common::start(&context).await.unwrap();

    let result2 = executor
        .invoke_and_await(&caller_worker_id, "test3", vec![])
        .await;

    drop(executor);

    check!(result1 == Ok(vec![Value::U64(1)]));
    check!(result2 == Ok(vec![Value::U64(2)]));
}

#[tokio::test]
#[tracing::instrument]
async fn context_inheritance() {
    let context = common::TestContext::new();
    let executor = common::start(&context).await.unwrap();

    let counters_component_id = executor.store_component("counters").await;
    let caller_component_id = executor.store_component("caller_composed").await;

    let mut env = HashMap::new();
    env.insert(
        "COUNTERS_COMPONENT_ID".to_string(),
        counters_component_id.to_string(),
    );
    env.insert("TEST_CONFIG".to_string(), "123".to_string());
    let caller_worker_id = executor
        .start_worker_with(
            &caller_component_id,
            "rpc-counters-4",
            vec!["a".to_string(), "b".to_string(), "c".to_string()],
            env,
        )
        .await;

    let result = executor
        .invoke_and_await(&caller_worker_id, "test4", vec![])
        .await;

    drop(executor);

    let result = result.unwrap();
    let result_tuple = match &result[0] {
        Value::Tuple(result) => result,
        _ => panic!("Unexpected result: {:?}", result),
    };
    let args = match &result_tuple[0] {
        Value::List(args) => args.clone(),
        _ => panic!("Unexpected result: {:?}", result),
    };
    let mut env = match &result_tuple[1] {
        Value::List(env) => env
            .clone()
            .into_iter()
            .map(|value| match value {
                Value::Tuple(tuple) => match (&tuple[0], &tuple[1]) {
                    (Value::String(key), Value::String(value)) => (key.clone(), value.clone()),
                    _ => panic!("Unexpected result: {:?}", result),
                },
                _ => panic!("Unexpected result: {:?}", result),
            })
            .collect::<Vec<_>>(),
        _ => panic!("Unexpected result: {:?}", result),
    };
    env.sort_by_key(|(k, _v)| k.clone());

    check!(
        args == vec![
            Value::String("a".to_string()),
            Value::String("b".to_string()),
            Value::String("c".to_string())
        ]
    );
    check!(
        env == vec![
            (
                "COUNTERS_COMPONENT_ID".to_string(),
                counters_component_id.to_string()
            ),
            (
                "GOLEM_COMPONENT_ID".to_string(),
                counters_component_id.to_string()
            ),
            ("GOLEM_COMPONENT_VERSION".to_string(), "0".to_string()),
            (
                "GOLEM_WORKER_NAME".to_string(),
                "counters_test4".to_string()
            ),
            ("TEST_CONFIG".to_string(), "123".to_string())
        ]
    );
}

#[tokio::test]
#[tracing::instrument]
async fn counter_resource_test_5() {
    let context = common::TestContext::new();
    let executor = common::start(&context).await.unwrap();

    let counters_component_id = executor.store_component("counters").await;
    let caller_component_id = executor.store_component("caller_composed").await;

    let mut env = HashMap::new();
    env.insert(
        "COUNTERS_COMPONENT_ID".to_string(),
        counters_component_id.to_string(),
    );
    let caller_worker_id = executor
        .start_worker_with(&caller_component_id, "rpc-counters-5", vec![], env)
        .await;

    executor.log_output(&caller_worker_id).await;

    let result = executor
        .invoke_and_await(&caller_worker_id, "test5", vec![])
        .await;

    drop(executor);

    check!(
        result
            == Ok(vec![Value::List(vec![
                Value::U64(3),
                Value::U64(3),
                Value::U64(3),
            ]),])
    );
}

#[tokio::test]
#[tracing::instrument]
async fn counter_resource_test_5_with_restart() {
    let context = common::TestContext::new();
    let executor = common::start(&context).await.unwrap();

    // using store_unique_component to avoid collision with counter_resource_test_5
    let counters_component_id = executor.store_unique_component("counters").await;
    let caller_component_id = executor.store_unique_component("caller_composed").await;

    let mut env = HashMap::new();
    env.insert(
        "COUNTERS_COMPONENT_ID".to_string(),
        counters_component_id.to_string(),
    );
    let caller_worker_id = executor
        .start_worker_with(&caller_component_id, "rpc-counters-5r", vec![], env)
        .await;

    executor.log_output(&caller_worker_id).await;

    let result1 = executor
        .invoke_and_await(&caller_worker_id, "test5", vec![])
        .await;

    drop(executor);

    let executor = common::start(&context).await.unwrap();

    let result2 = executor
        .invoke_and_await(&caller_worker_id, "test5", vec![])
        .await;

    drop(executor);

    check!(
        result1
            == Ok(vec![Value::List(vec![
                Value::U64(3),
                Value::U64(3),
                Value::U64(3),
            ]),])
    );
    // The second call has the same result because new resources are created within test5()
    check!(
        result2
            == Ok(vec![Value::List(vec![
                Value::U64(3),
                Value::U64(3),
                Value::U64(3),
            ]),]),
    );
}

#[tokio::test]
#[tracing::instrument]
async fn wasm_rpc_bug_32_test() {
    let context = common::TestContext::new();
    let executor = common::start(&context).await.unwrap();

    let counters_component_id = executor.store_component("counters").await;
    let caller_component_id = executor.store_component("caller_composed").await;

    let mut env = HashMap::new();
    env.insert(
        "COUNTERS_COMPONENT_ID".to_string(),
        counters_component_id.to_string(),
    );
    let caller_worker_id = executor
        .start_worker_with(&caller_component_id, "rpc-counters-bug32", vec![], env)
        .await;

    let result = executor
        .invoke_and_await(
            &caller_worker_id,
            "bug-wasm-rpc-i32",
            vec![Value::Variant {
                case_idx: 0,
                case_value: None,
            }],
        )
        .await;

    drop(executor);

    check!(
        result
            == Ok(vec![Value::Variant {
                case_idx: 0,
                case_value: None,
            }])
    );
}

#[tokio::test]
#[tracing::instrument]
async fn error_message_invalid_uri() {
    let context = common::TestContext::new();
    let executor = common::start(&context).await.unwrap();

    let registry_component_id = executor.store_component("auction_registry_composed").await;

    let mut env = HashMap::new();
    env.insert(
        "AUCTION_COMPONENT_ID".to_string(),
        "invalid-component-id".to_string(),
    );
    let registry_worker_id = executor
        .start_worker_with(
            &registry_component_id,
            "auction-registry-invalid-uri",
            vec![],
            env,
        )
        .await;

    let _ = executor.log_output(&registry_worker_id).await;

    let expiration = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let create_auction_result = executor
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

    drop(executor);

    check!(worker_error_message(&create_auction_result.err().unwrap())
        .contains("Invalid URI: urn:worker:invalid-component-id"));
}

#[tokio::test]
#[tracing::instrument]
async fn error_message_non_existing_target_component() {
    let context = common::TestContext::new();
    let executor = common::start(&context).await.unwrap();

    let registry_component_id = executor.store_component("auction_registry_composed").await;

    let mut env = HashMap::new();
    env.insert(
        "AUCTION_COMPONENT_ID".to_string(),
        "FB2F8E32-7B94-4699-B6EC-82BCE80FF9F2".to_string(), // valid UUID, but not a an existing component
    );
    let registry_worker_id = executor
        .start_worker_with(
            &registry_component_id,
            "auction-registry-non-existing-target",
            vec![],
            env,
        )
        .await;

    let _ = executor.log_output(&registry_worker_id).await;

    let expiration = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let create_auction_result = executor
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

    drop(executor);

    check!(worker_error_message(&create_auction_result.err().unwrap())
        .contains("Could not find any component with the given id"));
}
