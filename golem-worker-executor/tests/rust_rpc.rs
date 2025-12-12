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
use golem_common::model::WorkerId;
use golem_test_framework::dsl::TestDsl;
use golem_wasm::analysis::analysed_type;
use golem_wasm::{IntoValueAndType, Value, ValueAndType};
use golem_worker_executor_test_utils::{
    start, LastUniqueId, TestContext, WorkerExecutorTestDependencies,
};
use std::collections::HashMap;
use std::time::SystemTime;
use test_r::{inherit_test_dep, test};

inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(LastUniqueId);
inherit_test_dep!(Tracing);

#[test]
#[tracing::instrument]
async fn auction_example_1(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let registry_component = executor
        .component(&context.default_environment_id, "auction_registry_composed")
        .store()
        .await?;

    let auction_component = executor
        .component(&context.default_environment_id, "auction")
        .store()
        .await?;

    let mut env = HashMap::new();
    env.insert(
        "AUCTION_COMPONENT_ID".to_string(),
        auction_component.id.to_string(),
    );
    let registry_worker_id = executor
        .start_worker_with(&registry_component.id, "auction-registry-1", env, vec![])
        .await?;

    executor.log_output(&registry_worker_id).await?;

    let expiration = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_secs();

    let create_auction_result = executor
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
        .await?;

    let get_auctions_result = executor
        .invoke_and_await(
            &registry_worker_id,
            "auction:registry-exports/api.{get-auctions}",
            vec![],
        )
        .await?;

    executor
        .check_oplog_is_queryable(&registry_worker_id)
        .await?;

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

    Ok(())
}

#[test]
#[tracing::instrument]
async fn auction_example_2(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let registry_component = executor
        .component(&context.default_environment_id, "auction_registry_composed")
        .store()
        .await?;

    let auction_component = executor
        .component(&context.default_environment_id, "auction")
        .store()
        .await?;

    let mut env = HashMap::new();
    env.insert(
        "AUCTION_COMPONENT_ID".to_string(),
        auction_component.id.to_string(),
    );
    let registry_worker_id = executor
        .start_worker_with(&registry_component.id, "auction-registry-2", env, vec![])
        .await?;

    executor.log_output(&registry_worker_id).await?;

    let expiration = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_secs();

    let create_auction_result = executor
        .invoke_and_await(
            &registry_worker_id,
            "auction:registry-exports/api.{create-auction-res}",
            vec![
                "test-auction".into_value_and_type(),
                "this is a test".into_value_and_type(),
                100.0f32.into_value_and_type(),
                (expiration + 600).into_value_and_type(),
            ],
        )
        .await?;

    let get_auctions_result = executor
        .invoke_and_await(
            &registry_worker_id,
            "auction:registry-exports/api.{get-auctions}",
            vec![],
        )
        .await??;

    executor
        .check_oplog_is_queryable(&registry_worker_id)
        .await?;

    check!(create_auction_result.is_ok());

    let auction_id = &create_auction_result.unwrap()[0];

    check!(
        get_auctions_result
            == vec![Value::List(vec![Value::Record(vec![
                auction_id.clone(),
                Value::String("test-auction".to_string()),
                Value::String("this is a test".to_string()),
                Value::F32(100.0),
                Value::U64(expiration + 600)
            ]),])]
    );

    Ok(())
}

#[test]
#[tracing::instrument]
async fn counter_resource_test_1(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let counters_component = executor
        .component(&context.default_environment_id, "counters")
        .store()
        .await?;
    let caller_component = executor
        .component(&context.default_environment_id, "caller_composed")
        .store()
        .await?;

    let mut env = HashMap::new();
    env.insert(
        "COUNTERS_COMPONENT_ID".to_string(),
        counters_component.id.to_string(),
    );
    let caller_worker_id = executor
        .start_worker_with(&caller_component.id, "rpc-counters-1", env, vec![])
        .await?;

    let result = executor
        .invoke_and_await(
            &caller_worker_id,
            "rpc:caller-exports/caller-inline-functions.{test1}",
            vec![],
        )
        .await??;

    executor.check_oplog_is_queryable(&caller_worker_id).await?;

    check!(
        result
            == vec![Value::List(vec![
                Value::Tuple(vec![Value::String("counter3".to_string()), Value::U64(3)]),
                Value::Tuple(vec![Value::String("counter2".to_string()), Value::U64(3)]),
                Value::Tuple(vec![Value::String("counter1".to_string()), Value::U64(3)])
            ])]
    );

    Ok(())
}

#[test]
#[tracing::instrument]
async fn counter_resource_test_2(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let counters_component = executor
        .component(&context.default_environment_id, "counters")
        .store()
        .await?;
    let caller_component = executor
        .component(&context.default_environment_id, "caller_composed")
        .store()
        .await?;

    let mut env = HashMap::new();
    env.insert(
        "COUNTERS_COMPONENT_ID".to_string(),
        counters_component.id.to_string(),
    );
    let caller_worker_id = executor
        .start_worker_with(&caller_component.id, "rpc-counters-2", env, vec![])
        .await?;

    let result1 = executor
        .invoke_and_await(
            &caller_worker_id,
            "rpc:caller-exports/caller-inline-functions.{test2}",
            vec![],
        )
        .await?;

    let result2 = executor
        .invoke_and_await(
            &caller_worker_id,
            "rpc:caller-exports/caller-inline-functions.{test2}",
            vec![],
        )
        .await?;

    executor.check_oplog_is_queryable(&caller_worker_id).await?;

    check!(result1 == Ok(vec![Value::U64(1)]));
    check!(result2 == Ok(vec![Value::U64(2)]));

    Ok(())
}

#[test]
#[tracing::instrument]
async fn counter_resource_test_2_with_restart(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let counters_component = executor
        .component(&context.default_environment_id, "counters")
        .store()
        .await?;
    let caller_component = executor
        .component(&context.default_environment_id, "caller_composed")
        .store()
        .await?;

    let mut env = HashMap::new();
    env.insert(
        "COUNTERS_COMPONENT_ID".to_string(),
        counters_component.id.to_string(),
    );

    let caller_worker_id = executor
        .start_worker_with(&caller_component.id, "rpc-counters-2r", env, vec![])
        .await?;

    let result1 = executor
        .invoke_and_await(
            &caller_worker_id,
            "rpc:caller-exports/caller-inline-functions.{test2}",
            vec![],
        )
        .await?;

    drop(executor);
    let executor = start(deps, &context).await?;

    let result2 = executor
        .invoke_and_await(
            &caller_worker_id,
            "rpc:caller-exports/caller-inline-functions.{test2}",
            vec![],
        )
        .await?;

    executor.check_oplog_is_queryable(&caller_worker_id).await?;

    check!(result1 == Ok(vec![Value::U64(1)]));
    check!(result2 == Ok(vec![Value::U64(2)]));

    Ok(())
}

#[test]
#[tracing::instrument]
async fn counter_resource_test_3(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let counters_component = executor
        .component(&context.default_environment_id, "counters")
        .store()
        .await?;
    let caller_component = executor
        .component(&context.default_environment_id, "caller_composed")
        .store()
        .await?;

    let mut env = HashMap::new();
    env.insert(
        "COUNTERS_COMPONENT_ID".to_string(),
        counters_component.id.to_string(),
    );

    let caller_worker_id = executor
        .start_worker_with(&caller_component.id, "rpc-counters-3", env, vec![])
        .await?;

    let result1 = executor
        .invoke_and_await(
            &caller_worker_id,
            "rpc:caller-exports/caller-inline-functions.{test3}",
            vec![],
        )
        .await?;

    let result2 = executor
        .invoke_and_await(
            &caller_worker_id,
            "rpc:caller-exports/caller-inline-functions.{test3}",
            vec![],
        )
        .await?;

    executor.check_oplog_is_queryable(&caller_worker_id).await?;

    check!(result1 == Ok(vec![Value::U64(1)]));
    check!(result2 == Ok(vec![Value::U64(2)]));

    Ok(())
}

#[test]
#[tracing::instrument]
async fn counter_resource_test_3_with_restart(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let counters_component = executor
        .component(&context.default_environment_id, "counters")
        .store()
        .await?;
    let caller_component = executor
        .component(&context.default_environment_id, "caller_composed")
        .store()
        .await?;

    let mut env = HashMap::new();
    env.insert(
        "COUNTERS_COMPONENT_ID".to_string(),
        counters_component.id.to_string(),
    );
    let caller_worker_id = executor
        .start_worker_with(&caller_component.id, "rpc-counters-3r", env, vec![])
        .await?;

    let result1 = executor
        .invoke_and_await(
            &caller_worker_id,
            "rpc:caller-exports/caller-inline-functions.{test3}",
            vec![],
        )
        .await?;

    drop(executor);
    let executor = start(deps, &context).await?;

    let result2 = executor
        .invoke_and_await(
            &caller_worker_id,
            "rpc:caller-exports/caller-inline-functions.{test3}",
            vec![],
        )
        .await?;

    executor.check_oplog_is_queryable(&caller_worker_id).await?;

    check!(result1 == Ok(vec![Value::U64(1)]));
    check!(result2 == Ok(vec![Value::U64(2)]));

    Ok(())
}

#[test]
#[tracing::instrument]
async fn context_inheritance(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let counters_component = executor
        .component(&context.default_environment_id, "counters")
        .store()
        .await?;
    let caller_component = executor
        .component(&context.default_environment_id, "caller_composed")
        .store()
        .await?;

    let mut env = HashMap::new();
    env.insert(
        "COUNTERS_COMPONENT_ID".to_string(),
        counters_component.id.to_string(),
    );
    env.insert("TEST_CONFIG".to_string(), "123".to_string());

    let caller_worker_id = executor
        .start_worker_with(&caller_component.id, "rpc-counters-4", env, vec![])
        .await?;

    let result = executor
        .invoke_and_await(
            &caller_worker_id,
            "rpc:caller-exports/caller-inline-functions.{test4}",
            vec![],
        )
        .await?;

    executor.check_oplog_is_queryable(&caller_worker_id).await?;

    let result = result.unwrap();
    let result_tuple = match &result[0] {
        Value::Tuple(result) => result,
        _ => panic!("Unexpected result: {result:?}"),
    };
    let args = match &result_tuple[0] {
        Value::List(args) => args.clone(),
        _ => panic!("Unexpected result: {result:?}"),
    };
    let mut env = match &result_tuple[1] {
        Value::List(env) => env
            .clone()
            .into_iter()
            .map(|value| match value {
                Value::Tuple(tuple) => match (&tuple[0], &tuple[1]) {
                    (Value::String(key), Value::String(value)) => (key.clone(), value.clone()),
                    _ => panic!("Unexpected result: {result:?}"),
                },
                _ => panic!("Unexpected result: {result:?}"),
            })
            .collect::<Vec<_>>(),
        _ => panic!("Unexpected result: {result:?}"),
    };
    env.sort_by_key(|(k, _v)| k.clone());

    check!(args == vec![]);
    check!(
        env == vec![
            (
                "COUNTERS_COMPONENT_ID".to_string(),
                counters_component.id.to_string()
            ),
            ("GOLEM_AGENT_ID".to_string(), "counters_test4".to_string()),
            (
                "GOLEM_COMPONENT_ID".to_string(),
                counters_component.id.to_string()
            ),
            ("GOLEM_COMPONENT_REVISION".to_string(), "0".to_string()),
            (
                "GOLEM_WORKER_NAME".to_string(),
                "counters_test4".to_string()
            ),
            ("TEST_CONFIG".to_string(), "123".to_string())
        ]
    );

    Ok(())
}

#[test]
#[tracing::instrument]
async fn counter_resource_test_5(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let counters_component = executor
        .component(&context.default_environment_id, "counters")
        .store()
        .await?;
    let caller_component = executor
        .component(&context.default_environment_id, "caller_composed")
        .store()
        .await?;

    let mut env = HashMap::new();
    env.insert(
        "COUNTERS_COMPONENT_ID".to_string(),
        counters_component.id.to_string(),
    );
    let caller_worker_id = executor
        .start_worker_with(&caller_component.id, "rpc-counters-5", env, vec![])
        .await?;

    executor.log_output(&caller_worker_id).await?;

    let result = executor
        .invoke_and_await(
            &caller_worker_id,
            "rpc:caller-exports/caller-inline-functions.{test5}",
            vec![],
        )
        .await?;

    executor.check_oplog_is_queryable(&caller_worker_id).await?;

    check!(
        result
            == Ok(vec![Value::List(vec![
                Value::U64(3),
                Value::U64(3),
                Value::U64(3),
            ]),])
    );

    Ok(())
}

#[test]
#[tracing::instrument]
async fn counter_resource_test_5_with_restart(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    // using store_unique_component to avoid collision with counter_resource_test_5
    let counters_component = executor
        .component(&context.default_environment_id, "counters")
        .unique()
        .store()
        .await?;
    let caller_component = executor
        .component(&context.default_environment_id, "caller_composed")
        .unique()
        .store()
        .await?;

    let mut env = HashMap::new();
    env.insert(
        "COUNTERS_COMPONENT_ID".to_string(),
        counters_component.id.to_string(),
    );
    let caller_worker_id = executor
        .start_worker_with(&caller_component.id, "rpc-counters-5r", env, vec![])
        .await?;

    executor.log_output(&caller_worker_id).await?;

    let result1 = executor
        .invoke_and_await(
            &caller_worker_id,
            "rpc:caller-exports/caller-inline-functions.{test5}",
            vec![],
        )
        .await?;

    drop(executor);
    let executor = start(deps, &context).await?;

    let result2 = executor
        .invoke_and_await(
            &caller_worker_id,
            "rpc:caller-exports/caller-inline-functions.{test5}",
            vec![],
        )
        .await?;

    executor.check_oplog_is_queryable(&caller_worker_id).await?;

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

    Ok(())
}

#[test]
#[tracing::instrument]
async fn wasm_rpc_bug_32_test(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let counters_component = executor
        .component(&context.default_environment_id, "counters")
        .store()
        .await?;
    let caller_component = executor
        .component(&context.default_environment_id, "caller_composed")
        .store()
        .await?;

    let mut env = HashMap::new();
    env.insert(
        "COUNTERS_COMPONENT_ID".to_string(),
        counters_component.id.to_string(),
    );
    let caller_worker_id = executor
        .start_worker_with(&caller_component.id, "rpc-counters-bug32", env, vec![])
        .await?;

    let result = executor
        .invoke_and_await(
            &caller_worker_id,
            "rpc:caller-exports/caller-inline-functions.{bug-wasm-rpc-i32}",
            vec![ValueAndType {
                value: Value::Variant {
                    case_idx: 0,
                    case_value: None,
                },
                typ: analysed_type::variant(vec![analysed_type::unit_case("leaf")]),
            }],
        )
        .await?;

    executor.check_oplog_is_queryable(&caller_worker_id).await?;

    check!(
        result
            == Ok(vec![Value::Variant {
                case_idx: 0,
                case_value: None,
            }])
    );

    Ok(())
}

#[test]
#[tracing::instrument]
async fn error_message_non_existing_target_component(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let registry_component = executor
        .component(&context.default_environment_id, "auction_registry_composed")
        .store()
        .await?;

    let mut env = HashMap::new();
    env.insert(
        "AUCTION_COMPONENT_ID".to_string(),
        "FB2F8E32-7B94-4699-B6EC-82BCE80FF9F2".to_string(), // valid UUID, but not an existing component
    );
    let registry_worker_id = executor
        .start_worker_with(
            &registry_component.id,
            "auction-registry-non-existing-target",
            env,
            vec![],
        )
        .await?;

    executor.log_output(&registry_worker_id).await?;

    let expiration = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)?
        .as_secs();

    let create_auction_result = executor
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
        .await?;

    executor
        .check_oplog_is_queryable(&registry_worker_id)
        .await?;

    assert!(format!("{:?}", create_auction_result.err().unwrap())
        .contains("Could not find any component with the given id"));

    Ok(())
}

#[test]
#[tracing::instrument]
async fn ephemeral_worker_invocation_via_rpc1(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component(&context.default_environment_id, "it_agent_counters_release")
        .name("it:agent-counters")
        .store()
        .await?;
    let worker_id = WorkerId {
        component_id: component.id,
        worker_name: "counter(\"ephemeral_worker_invocation_via_rpc1\")".to_string(),
    };

    let _ = executor
        .invoke_and_await(
            &worker_id,
            "it:agent-counters/counter.{increment-through-rpc-to-ephemeral}",
            vec![],
        )
        .await?;
    let result = executor
        .invoke_and_await(
            &worker_id,
            "it:agent-counters/counter.{increment-through-rpc-to-ephemeral}",
            vec![],
        )
        .await?;

    executor.check_oplog_is_queryable(&worker_id).await?;
    drop(executor);

    assert_eq!(result, Ok(vec![Value::U32(1)]));

    Ok(())
}

#[test]
#[tracing::instrument]
async fn ephemeral_worker_invocation_via_rpc2(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component(&context.default_environment_id, "it_agent_counters_release")
        .name("it:agent-counters")
        .store()
        .await?;
    let worker_id = WorkerId {
        component_id: component.id,
        worker_name: "counter(\"ephemeral_worker_invocation_via_rpc2\")".to_string(),
    };

    let _ = executor
        .invoke_and_await(
            &worker_id,
            "it:agent-counters/counter.{increment-through-rpc-to-ephemeral-phantom}",
            vec![],
        )
        .await;
    let result = executor
        .invoke_and_await(
            &worker_id,
            "it:agent-counters/counter.{increment-through-rpc-to-ephemeral-phantom}",
            vec![],
        )
        .await?;

    executor.check_oplog_is_queryable(&worker_id).await?;
    drop(executor);

    assert_eq!(result, Ok(vec![Value::U32(1)]));

    Ok(())
}
