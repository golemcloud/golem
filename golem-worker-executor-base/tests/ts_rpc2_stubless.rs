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
async fn counter_resource_test_2(
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
        .start_worker_with(&caller_component_id, "rpc-counters-2", vec![], env)
        .await;

    let result1 = executor
        .invoke_and_await(
            &caller_worker_id,
            "rpc:caller-exports/caller-inline-functions.{test2}",
            vec![],
        )
        .await;
    let result2 = executor
        .invoke_and_await(
            &caller_worker_id,
            "rpc:caller-exports/caller-inline-functions.{test2}",
            vec![],
        )
        .await;

    drop(executor);

    check!(result1 == Ok(vec![Value::U64(1)]));
    check!(result2 == Ok(vec![Value::U64(2)]));
}

#[test]
#[tracing::instrument]
async fn counter_resource_test_2_with_restart(
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
        .start_worker_with(&caller_component_id, "rpc-counters-2r", vec![], env)
        .await;

    let result1 = executor
        .invoke_and_await(
            &caller_worker_id,
            "rpc:caller-exports/caller-inline-functions.{test2}",
            vec![],
        )
        .await;

    drop(executor);
    let executor = common::start(deps, &context).await.unwrap();

    let result2 = executor
        .invoke_and_await(
            &caller_worker_id,
            "rpc:caller-exports/caller-inline-functions.{test2}",
            vec![],
        )
        .await;

    drop(executor);

    check!(result1 == Ok(vec![Value::U64(1)]));
    check!(result2 == Ok(vec![Value::U64(2)]));
}
