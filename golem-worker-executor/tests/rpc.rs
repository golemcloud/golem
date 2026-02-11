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
use golem_common::base_model::agent::ElementValue;
use golem_common::model::agent::{AgentId, AgentTypeName, DataValue, ElementValues};
use golem_common::model::component_metadata::{
    DynamicLinkedInstance, DynamicLinkedWasmRpc, WasmRpcTarget,
};
use golem_common::model::oplog::WorkerError;
use golem_common::model::WorkerId;
use golem_common::{agent_id, data_value};
use golem_test_framework::dsl::{worker_error_underlying_error, TestDsl};
use golem_wasm::analysis::analysed_type;
use golem_wasm::analysis::analysed_type::{field, record, str};
use golem_wasm::{FromValue, IntoValueAndType, UuidRecord, Value, ValueAndType};
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
async fn rust_rpc_with_payload(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component(
            &context.default_environment_id,
            "golem_it_agent_rpc_rust_release",
        )
        .name("golem-it:agent-rpc-rust")
        .store()
        .await?;

    let parent_agent_id = agent_id!("rust-parent", "rust_rpc_with_payload");
    let parent = executor
        .start_agent(&component.id, parent_agent_id.clone())
        .await?;

    executor.log_output(&parent).await?;

    let spawn_result = executor
        .invoke_and_await_agent(
            &component.id,
            &parent_agent_id,
            "spawn_child",
            data_value!("hello world"),
        )
        .await?;

    let uuid_as_value = spawn_result
        .into_return_value()
        .expect("Expected a single return value");

    let uuid = UuidRecord::from_value(uuid_as_value.clone()).expect("UUID expected");

    let child_agent_id = agent_id!("rust-child", uuid);

    let get_result = executor
        .invoke_and_await_agent(&component.id, &child_agent_id, "get", data_value!())
        .await?;

    let option_payload_as_value = get_result
        .into_return_value()
        .expect("Expected a single return value");

    executor.check_oplog_is_queryable(&parent).await?;

    assert_eq!(
        option_payload_as_value,
        Value::Option(Some(Box::new(Value::Record(vec![
            Value::String("hello world".to_string()),
            uuid_as_value.clone(),
            Value::Enum(0)
        ]))))
    );
    Ok(())
}

#[test]
#[tracing::instrument]
async fn rust_rpc_missing_target(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component(
            &context.default_environment_id,
            "golem_it_agent_rpc_rust_release",
        )
        .name("golem-it:agent-rpc-rust")
        .store()
        .await?;

    let parent_agent_id = agent_id!("rust-parent", "rust_rpc_with_payload");
    let parent = executor
        .start_agent(&component.id, parent_agent_id.clone())
        .await?;

    executor.log_output(&parent).await?;

    let call_result = executor
        .invoke_and_await_agent(
            &component.id,
            &parent_agent_id,
            "call_ts_agent",
            data_value!(),
        )
        .await?;

    // TODO: this test case is currently not working as expected; once that is fixed we should assert on a user-friendly failure message

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
        .component(&context.default_environment_id, "caller")
        .with_dynamic_linking(&[
            (
                "rpc:counters-client/counters-client",
                DynamicLinkedInstance::WasmRpc(DynamicLinkedWasmRpc {
                    targets: HashMap::from_iter(vec![
                        (
                            "api".to_string(),
                            WasmRpcTarget {
                                interface_name: "rpc:counters-exports/api".to_string(),
                                component_name: "rpc:counters".to_string(),
                            },
                        ),
                        (
                            "counter".to_string(),
                            WasmRpcTarget {
                                interface_name: "rpc:counters-exports/api".to_string(),
                                component_name: "rpc:counters".to_string(),
                            },
                        ),
                    ]),
                }),
            ),
            (
                "rpc:ephemeral-client/ephemeral-client",
                DynamicLinkedInstance::WasmRpc(DynamicLinkedWasmRpc {
                    targets: HashMap::from_iter(vec![(
                        "api".to_string(),
                        WasmRpcTarget {
                            interface_name: "rpc:ephemeral-exports/api".to_string(),
                            component_name: "rpc:ephemeral".to_string(),
                        },
                    )]),
                }),
            ),
        ])
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
        .await?;

    executor.check_oplog_is_queryable(&caller_worker_id).await?;

    check!(
        result
            == Ok(vec![Value::List(vec![
                Value::Tuple(vec![Value::String("counter3".to_string()), Value::U64(3)]),
                Value::Tuple(vec![Value::String("counter2".to_string()), Value::U64(3)]),
                Value::Tuple(vec![Value::String("counter1".to_string()), Value::U64(3)])
            ])])
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
        .component(&context.default_environment_id, "caller")
        .with_dynamic_linking(&[
            (
                "rpc:counters-client/counters-client",
                DynamicLinkedInstance::WasmRpc(DynamicLinkedWasmRpc {
                    targets: HashMap::from_iter(vec![
                        (
                            "api".to_string(),
                            WasmRpcTarget {
                                interface_name: "rpc:counters-exports/api".to_string(),
                                component_name: "rpc:counters".to_string(),
                            },
                        ),
                        (
                            "counter".to_string(),
                            WasmRpcTarget {
                                interface_name: "rpc:counters-exports/api".to_string(),
                                component_name: "rpc:counters".to_string(),
                            },
                        ),
                    ]),
                }),
            ),
            (
                "rpc:ephemeral-client/ephemeral-client",
                DynamicLinkedInstance::WasmRpc(DynamicLinkedWasmRpc {
                    targets: HashMap::from_iter(vec![(
                        "api".to_string(),
                        WasmRpcTarget {
                            interface_name: "rpc:ephemeral-exports/api".to_string(),
                            component_name: "rpc:ephemeral".to_string(),
                        },
                    )]),
                }),
            ),
        ])
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
        .component(&context.default_environment_id, "caller")
        .with_dynamic_linking(&[
            (
                "rpc:counters-client/counters-client",
                DynamicLinkedInstance::WasmRpc(DynamicLinkedWasmRpc {
                    targets: HashMap::from_iter(vec![
                        (
                            "api".to_string(),
                            WasmRpcTarget {
                                interface_name: "rpc:counters-exports/api".to_string(),
                                component_name: "rpc:counters".to_string(),
                            },
                        ),
                        (
                            "counter".to_string(),
                            WasmRpcTarget {
                                interface_name: "rpc:counters-exports/api".to_string(),
                                component_name: "rpc:counters".to_string(),
                            },
                        ),
                    ]),
                }),
            ),
            (
                "rpc:ephemeral-client/ephemeral-client",
                DynamicLinkedInstance::WasmRpc(DynamicLinkedWasmRpc {
                    targets: HashMap::from_iter(vec![(
                        "api".to_string(),
                        WasmRpcTarget {
                            interface_name: "rpc:ephemeral-exports/api".to_string(),
                            component_name: "rpc:ephemeral".to_string(),
                        },
                    )]),
                }),
            ),
        ])
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
        .component(&context.default_environment_id, "caller")
        .with_dynamic_linking(&[
            (
                "rpc:counters-client/counters-client",
                DynamicLinkedInstance::WasmRpc(DynamicLinkedWasmRpc {
                    targets: HashMap::from_iter(vec![
                        (
                            "api".to_string(),
                            WasmRpcTarget {
                                interface_name: "rpc:counters-exports/api".to_string(),
                                component_name: "rpc:counters".to_string(),
                            },
                        ),
                        (
                            "counter".to_string(),
                            WasmRpcTarget {
                                interface_name: "rpc:counters-exports/api".to_string(),
                                component_name: "rpc:counters".to_string(),
                            },
                        ),
                    ]),
                }),
            ),
            (
                "rpc:ephemeral-client/ephemeral-client",
                DynamicLinkedInstance::WasmRpc(DynamicLinkedWasmRpc {
                    targets: HashMap::from_iter(vec![(
                        "api".to_string(),
                        WasmRpcTarget {
                            interface_name: "rpc:ephemeral-exports/api".to_string(),
                            component_name: "rpc:ephemeral".to_string(),
                        },
                    )]),
                }),
            ),
        ])
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
        .component(&context.default_environment_id, "caller")
        .with_dynamic_linking(&[
            (
                "rpc:counters-client/counters-client",
                DynamicLinkedInstance::WasmRpc(DynamicLinkedWasmRpc {
                    targets: HashMap::from_iter(vec![
                        (
                            "api".to_string(),
                            WasmRpcTarget {
                                interface_name: "rpc:counters-exports/api".to_string(),
                                component_name: "rpc:counters".to_string(),
                            },
                        ),
                        (
                            "counter".to_string(),
                            WasmRpcTarget {
                                interface_name: "rpc:counters-exports/api".to_string(),
                                component_name: "rpc:counters".to_string(),
                            },
                        ),
                    ]),
                }),
            ),
            (
                "rpc:ephemeral-client/ephemeral-client",
                DynamicLinkedInstance::WasmRpc(DynamicLinkedWasmRpc {
                    targets: HashMap::from_iter(vec![(
                        "api".to_string(),
                        WasmRpcTarget {
                            interface_name: "rpc:ephemeral-exports/api".to_string(),
                            component_name: "rpc:ephemeral".to_string(),
                        },
                    )]),
                }),
            ),
        ])
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
        .component(&context.default_environment_id, "caller")
        .with_dynamic_linking(&[
            (
                "rpc:counters-client/counters-client",
                DynamicLinkedInstance::WasmRpc(DynamicLinkedWasmRpc {
                    targets: HashMap::from_iter(vec![
                        (
                            "api".to_string(),
                            WasmRpcTarget {
                                interface_name: "rpc:counters-exports/api".to_string(),
                                component_name: "rpc:counters".to_string(),
                            },
                        ),
                        (
                            "counter".to_string(),
                            WasmRpcTarget {
                                interface_name: "rpc:counters-exports/api".to_string(),
                                component_name: "rpc:counters".to_string(),
                            },
                        ),
                    ]),
                }),
            ),
            (
                "rpc:ephemeral-client/ephemeral-client",
                DynamicLinkedInstance::WasmRpc(DynamicLinkedWasmRpc {
                    targets: HashMap::from_iter(vec![(
                        "api".to_string(),
                        WasmRpcTarget {
                            interface_name: "rpc:ephemeral-exports/api".to_string(),
                            component_name: "rpc:ephemeral".to_string(),
                        },
                    )]),
                }),
            ),
        ])
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

    let result = result?;
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
        .component(&context.default_environment_id, "caller")
        .with_dynamic_linking(&[
            (
                "rpc:counters-client/counters-client",
                DynamicLinkedInstance::WasmRpc(DynamicLinkedWasmRpc {
                    targets: HashMap::from_iter(vec![
                        (
                            "api".to_string(),
                            WasmRpcTarget {
                                interface_name: "rpc:counters-exports/api".to_string(),
                                component_name: "rpc:counters".to_string(),
                            },
                        ),
                        (
                            "counter".to_string(),
                            WasmRpcTarget {
                                interface_name: "rpc:counters-exports/api".to_string(),
                                component_name: "rpc:counters".to_string(),
                            },
                        ),
                    ]),
                }),
            ),
            (
                "rpc:ephemeral-client/ephemeral-client",
                DynamicLinkedInstance::WasmRpc(DynamicLinkedWasmRpc {
                    targets: HashMap::from_iter(vec![(
                        "api".to_string(),
                        WasmRpcTarget {
                            interface_name: "rpc:ephemeral-exports/api".to_string(),
                            component_name: "rpc:ephemeral".to_string(),
                        },
                    )]),
                }),
            ),
        ])
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
        .component(&context.default_environment_id, "caller")
        .unique()
        .with_dynamic_linking(&[
            (
                "rpc:counters-client/counters-client",
                DynamicLinkedInstance::WasmRpc(DynamicLinkedWasmRpc {
                    targets: HashMap::from_iter(vec![
                        (
                            "api".to_string(),
                            WasmRpcTarget {
                                interface_name: "rpc:counters-exports/api".to_string(),
                                component_name: "rpc:counters".to_string(),
                            },
                        ),
                        (
                            "counter".to_string(),
                            WasmRpcTarget {
                                interface_name: "rpc:counters-exports/api".to_string(),
                                component_name: "rpc:counters".to_string(),
                            },
                        ),
                    ]),
                }),
            ),
            (
                "rpc:ephemeral-client/ephemeral-client",
                DynamicLinkedInstance::WasmRpc(DynamicLinkedWasmRpc {
                    targets: HashMap::from_iter(vec![(
                        "api".to_string(),
                        WasmRpcTarget {
                            interface_name: "rpc:ephemeral-exports/api".to_string(),
                            component_name: "rpc:ephemeral".to_string(),
                        },
                    )]),
                }),
            ),
        ])
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
        .component(&context.default_environment_id, "caller")
        .with_dynamic_linking(&[
            (
                "rpc:counters-client/counters-client",
                DynamicLinkedInstance::WasmRpc(DynamicLinkedWasmRpc {
                    targets: HashMap::from_iter(vec![
                        (
                            "api".to_string(),
                            WasmRpcTarget {
                                interface_name: "rpc:counters-exports/api".to_string(),
                                component_name: "rpc:counters".to_string(),
                            },
                        ),
                        (
                            "counter".to_string(),
                            WasmRpcTarget {
                                interface_name: "rpc:counters-exports/api".to_string(),
                                component_name: "rpc:counters".to_string(),
                            },
                        ),
                    ]),
                }),
            ),
            (
                "rpc:ephemeral-client/ephemeral-client",
                DynamicLinkedInstance::WasmRpc(DynamicLinkedWasmRpc {
                    targets: HashMap::from_iter(vec![(
                        "api".to_string(),
                        WasmRpcTarget {
                            interface_name: "rpc:ephemeral-exports/api".to_string(),
                            component_name: "rpc:ephemeral".to_string(),
                        },
                    )]),
                }),
            ),
        ])
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
async fn golem_bug_1265_test(
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
        .component(&context.default_environment_id, "caller")
        .with_dynamic_linking(&[
            (
                "rpc:counters-client/counters-client",
                DynamicLinkedInstance::WasmRpc(DynamicLinkedWasmRpc {
                    targets: HashMap::from_iter(vec![
                        (
                            "api".to_string(),
                            WasmRpcTarget {
                                interface_name: "rpc:counters-exports/api".to_string(),
                                component_name: "rpc:counters".to_string(),
                            },
                        ),
                        (
                            "counter".to_string(),
                            WasmRpcTarget {
                                interface_name: "rpc:counters-exports/api".to_string(),
                                component_name: "rpc:counters".to_string(),
                            },
                        ),
                    ]),
                }),
            ),
            (
                "rpc:ephemeral-client/ephemeral-client",
                DynamicLinkedInstance::WasmRpc(DynamicLinkedWasmRpc {
                    targets: HashMap::from_iter(vec![(
                        "api".to_string(),
                        WasmRpcTarget {
                            interface_name: "rpc:ephemeral-exports/api".to_string(),
                            component_name: "rpc:ephemeral".to_string(),
                        },
                    )]),
                }),
            ),
        ])
        .store()
        .await?;

    let mut env = HashMap::new();
    env.insert(
        "COUNTERS_COMPONENT_ID".to_string(),
        counters_component.id.to_string(),
    );
    let caller_worker_id = executor
        .start_worker_with(&caller_component.id, "rpc-counters-bug1265", env, vec![])
        .await?;

    let result = executor
        .invoke_and_await(
            &caller_worker_id,
            "rpc:caller-exports/caller-inline-functions.{bug-golem1265}",
            vec!["test".into_value_and_type()],
        )
        .await?;

    executor.check_oplog_is_queryable(&caller_worker_id).await?;

    check!(result == Ok(vec![Value::Result(Ok(None))]));

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
