// Copyright 2024-2025 Golem Cloud
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

use test_r::{inherit_test_dep, test};

use crate::{common, LastUniqueId, Tracing, WorkerExecutorTestDependencies};
use assert2::check;
use golem_common::model::component_metadata::{DynamicLinkedInstance, DynamicLinkedWasmRpc};
use golem_test_framework::dsl::TestDslUnsafe;
use golem_wasm_rpc::Value;
use std::collections::HashMap;

inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(LastUniqueId);
inherit_test_dep!(Tracing);

static COUNTER_COMPONENT_NAME: &str = "counter-ts";
static CALLER_COMPONENT_NAME: &str = "caller-ts";

#[test]
#[tracing::instrument]
async fn counter_resource_test_1(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) {
    let context = common::TestContext::new(last_unique_id);
    let executor = common::start(deps, &context).await.unwrap();

    let counters_component_id = executor.store_component(COUNTER_COMPONENT_NAME).await;
    let caller_component_id = executor
        .store_component_with_dynamic_linking(
            CALLER_COMPONENT_NAME,
            &[(
                "rpc:counters-client/counters-client",
                DynamicLinkedInstance::WasmRpc(DynamicLinkedWasmRpc {
                    target_interface_name: HashMap::from_iter(vec![
                        ("api".to_string(), "rpc:counters-exports/api".to_string()),
                        (
                            "counter".to_string(),
                            "rpc:counters-exports/api".to_string(),
                        ),
                    ]),
                }),
            )],
        )
        .await;

    let mut env = HashMap::new();
    env.insert(
        "COUNTERS_COMPONENT_ID".to_string(),
        counters_component_id.to_string(),
    );
    let caller_worker_id = executor
        .start_worker_with(&caller_component_id, "rpc-counters-1", vec![], env)
        .await;

    let result1 = executor
        .invoke_and_await(
            &caller_worker_id,
            "rpc:caller-exports/caller-inline-functions.{test1}",
            vec![],
        )
        .await;
    let result2 = executor
        .invoke_and_await(
            &caller_worker_id,
            "rpc:caller-exports/caller-inline-functions.{test1}",
            vec![],
        )
        .await;

    drop(executor);

    check!(result1 == Ok(vec![Value::U64(1)]));
    check!(result2 == Ok(vec![Value::U64(2)]));
}

#[test]
#[tracing::instrument]
async fn counter_resource_test_1_with_restart(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) {
    let context = common::TestContext::new(last_unique_id);
    let executor = common::start(deps, &context).await.unwrap();

    let counters_component_id = executor.store_component(COUNTER_COMPONENT_NAME).await;
    let caller_component_id = executor
        .store_component_with_dynamic_linking(
            CALLER_COMPONENT_NAME,
            &[(
                "rpc:counters-client/counters-client",
                DynamicLinkedInstance::WasmRpc(DynamicLinkedWasmRpc {
                    target_interface_name: HashMap::from_iter(vec![
                        ("api".to_string(), "rpc:counters-exports/api".to_string()),
                        (
                            "counter".to_string(),
                            "rpc:counters-exports/api".to_string(),
                        ),
                    ]),
                }),
            )],
        )
        .await;

    let mut env = HashMap::new();
    env.insert(
        "COUNTERS_COMPONENT_ID".to_string(),
        counters_component_id.to_string(),
    );
    let caller_worker_id = executor
        .start_worker_with(&caller_component_id, "rpc-counters-1r", vec![], env)
        .await;

    let result1 = executor
        .invoke_and_await(
            &caller_worker_id,
            "rpc:caller-exports/caller-inline-functions.{test1}",
            vec![],
        )
        .await;

    drop(executor);
    let executor = common::start(deps, &context).await.unwrap();

    let result2 = executor
        .invoke_and_await(
            &caller_worker_id,
            "rpc:caller-exports/caller-inline-functions.{test1}",
            vec![],
        )
        .await;

    drop(executor);

    check!(result1 == Ok(vec![Value::U64(1)]));
    check!(result2 == Ok(vec![Value::U64(2)]));
}

#[test]
#[tracing::instrument]
async fn context_inheritance(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) {
    let context = common::TestContext::new(last_unique_id);
    let executor = common::start(deps, &context).await.unwrap();

    let counters_component_id = executor.store_component(COUNTER_COMPONENT_NAME).await;
    let caller_component_id = executor
        .store_component_with_dynamic_linking(
            CALLER_COMPONENT_NAME,
            &[(
                "rpc:counters-client/counters-client",
                DynamicLinkedInstance::WasmRpc(DynamicLinkedWasmRpc {
                    target_interface_name: HashMap::from_iter(vec![
                        ("api".to_string(), "rpc:counters-exports/api".to_string()),
                        (
                            "counter".to_string(),
                            "rpc:counters-exports/api".to_string(),
                        ),
                    ]),
                }),
            )],
        )
        .await;

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
        .invoke_and_await(
            &caller_worker_id,
            "rpc:caller-exports/caller-inline-functions.{test3}",
            vec![],
        )
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
