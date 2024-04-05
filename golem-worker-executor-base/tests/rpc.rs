use crate::common;
use assert2::check;
use golem_test_framework::dsl::TestDsl;
use golem_wasm_rpc::Value;
use std::collections::HashMap;
use std::time::SystemTime;

#[tokio::test]
#[tracing::instrument]
async fn auction_example_1() {
    let context = common::TestContext::new();
    let executor = common::start(&context).await.unwrap();

    let registry_template_id = executor.store_template("auction_registry_composed").await;
    let auction_template_id = executor.store_template("auction").await;

    let mut env = HashMap::new();
    env.insert(
        "AUCTION_TEMPLATE_ID".to_string(),
        auction_template_id.to_string(),
    );
    let registry_worker_id = executor
        .start_worker_with(&registry_template_id, "auction-registry-1", vec![], env)
        .await;

    let _ = executor.log_output(&registry_worker_id).await;

    let expiration = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let create_auction_result = executor
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

    let get_auctions_result = executor
        .invoke_and_await(
            &registry_worker_id,
            "auction:registry/api/get-auctions",
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

    let registry_template_id = executor.store_template("auction_registry_composed").await;
    let auction_template_id = executor.store_template("auction").await;

    let mut env = HashMap::new();
    env.insert(
        "AUCTION_TEMPLATE_ID".to_string(),
        auction_template_id.to_string(),
    );
    let registry_worker_id = executor
        .start_worker_with(&registry_template_id, "auction-registry-1", vec![], env)
        .await;

    let _ = executor.log_output(&registry_worker_id).await;

    let expiration = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let create_auction_result = executor
        .invoke_and_await(
            &registry_worker_id,
            "auction:registry/api/create-auction-res",
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
            "auction:registry/api/get-auctions",
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

    let counters_template_id = executor.store_template("counters").await;
    let caller_template_id = executor.store_template("caller_composed").await;

    let mut env = HashMap::new();
    env.insert(
        "COUNTERS_TEMPLATE_ID".to_string(),
        counters_template_id.to_string(),
    );
    let caller_worker_id = executor
        .start_worker_with(&caller_template_id, "rpc-counters-1", vec![], env)
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

    let counters_template_id = executor.store_template("counters").await;
    let caller_template_id = executor.store_template("caller_composed").await;

    let mut env = HashMap::new();
    env.insert(
        "COUNTERS_TEMPLATE_ID".to_string(),
        counters_template_id.to_string(),
    );
    let caller_worker_id = executor
        .start_worker_with(&caller_template_id, "rpc-counters-2", vec![], env)
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

    let counters_template_id = executor.store_template("counters").await;
    let caller_template_id = executor.store_template("caller_composed").await;

    let mut env = HashMap::new();
    env.insert(
        "COUNTERS_TEMPLATE_ID".to_string(),
        counters_template_id.to_string(),
    );
    let caller_worker_id = executor
        .start_worker_with(&caller_template_id, "rpc-counters-2r", vec![], env)
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

    let counters_template_id = executor.store_template("counters").await;
    let caller_template_id = executor.store_template("caller_composed").await;

    let mut env = HashMap::new();
    env.insert(
        "COUNTERS_TEMPLATE_ID".to_string(),
        counters_template_id.to_string(),
    );
    let caller_worker_id = executor
        .start_worker_with(&caller_template_id, "rpc-counters-3", vec![], env)
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

    let counters_template_id = executor.store_template("counters").await;
    let caller_template_id = executor.store_template("caller_composed").await;

    let mut env = HashMap::new();
    env.insert(
        "COUNTERS_TEMPLATE_ID".to_string(),
        counters_template_id.to_string(),
    );
    let caller_worker_id = executor
        .start_worker_with(&caller_template_id, "rpc-counters-3r", vec![], env)
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
