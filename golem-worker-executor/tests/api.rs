// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
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
use anyhow::anyhow;
use axum::Router;
use axum::routing::get;
use golem_common::model::account::AccountId;
use golem_common::model::agent::{
    ComponentModelElementValue, DataValue, ElementValue, ElementValues,
};
use golem_common::model::component::{ComponentDto, ComponentId, ComponentRevision};
use golem_common::model::oplog::OplogIndex;
use golem_common::model::worker::AgentMetadataDto;
use golem_common::model::{
    AgentFilter, AgentId, AgentStatus, FilterComparator, IdempotencyKey, PromiseId, RetryConfig,
    ScanCursor, StringFilterComparator,
};
use golem_common::{agent_id, data_value};
use golem_service_base::error::worker_executor::WorkerExecutorError;
use golem_test_framework::dsl::TestDsl;
use golem_test_framework::dsl::{drain_connection, stdout_event_matching, stdout_events};
use golem_wasm::analysis::analysed_type;
use golem_wasm::analysis::wit_parser::{SharedAnalysedTypeResolve, TypeName, TypeOwner};
use golem_wasm::{IntoValue, Record};
use golem_wasm::{IntoValueAndType, Value, ValueAndType};
use golem_worker_executor_test_utils::{
    LastUniqueId, PrecompiledComponent, TestContext, TestWorkerExecutor,
    WorkerExecutorTestDependencies, start, start_customized, start_with_redis_storage,
};
use pretty_assertions::assert_eq;
use redis::Commands;
use std::collections::HashMap;

use std::io::Write;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use system_interface::fs::FileIoExt;
use test_r::{inherit_test_dep, test, timeout};
use tokio::task::JoinHandle;
use tokio::time::sleep;
use tokio_util::sync::CancellationToken;
use tracing::{Instrument, Span, debug, info};

inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(LastUniqueId);
inherit_test_dep!(Tracing);
inherit_test_dep!(
    #[tagged_as("golem_host")]
    SharedAnalysedTypeResolve
);
inherit_test_dep!(
    #[tagged_as("host_api_tests")]
    PrecompiledComponent
);
inherit_test_dep!(
    #[tagged_as("agent_rpc")]
    PrecompiledComponent
);
inherit_test_dep!(
    #[tagged_as("agent_rpc_rust")]
    PrecompiledComponent
);
inherit_test_dep!(
    #[tagged_as("agent_rpc_rust_as_resolve_target")]
    PrecompiledComponent
);
inherit_test_dep!(
    #[tagged_as("agent_counters")]
    PrecompiledComponent
);
inherit_test_dep!(
    #[tagged_as("http_tests")]
    PrecompiledComponent
);

#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn dropping_executor_releases_services(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;
    let leak_detector = executor.leak_detector();

    // Verify the sentinel is alive while the executor is running
    assert!(
        leak_detector.upgrade().is_some(),
        "Leak sentinel should be alive while executor is running"
    );

    drop(executor);

    // Give background tasks a moment to be aborted and cleaned up
    tokio::time::sleep(Duration::from_secs(2)).await;

    // After dropping the executor, the service graph should be fully deallocated.
    // If this assertion fails, there is a circular Arc reference preventing cleanup.
    assert!(
        leak_detector.upgrade().is_none(),
        "Service graph leaked! The All struct was not deallocated after dropping the executor. \
         This indicates a circular Arc reference in the service dependency graph."
    );

    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("4m")]
async fn interruption(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;
    let agent_id = agent_id!("Clocks", "interruption-1");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    // Warmup: ensure the agent constructor has completed before we invoke the
    // long-running function we intend to interrupt.
    executor
        .invoke_and_await_agent(&component, &agent_id, "sleep_for", data_value!(0.0f64))
        .await?;

    let executor_clone = executor.clone();
    let component_clone = component.clone();
    let agent_id_clone = agent_id.clone();
    let fiber = tokio::spawn(
        async move {
            executor_clone
                .invoke_and_await_agent(
                    &component_clone,
                    &agent_id_clone,
                    "interruption",
                    data_value!(),
                )
                .await
        }
        .in_current_span(),
    );

    executor
        .wait_for_status(&worker_id, AgentStatus::Running, Duration::from_secs(10))
        .await?;

    executor.interrupt(&worker_id).await?;
    let result = fiber.await?;

    drop(executor);

    assert!(result.is_err());
    let err_msg = format!("{}", result.err().unwrap());
    assert!(err_msg.contains("Interrupted via the Golem API"));
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("8m")]
async fn delete_interrupts_long_rpc_call(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("agent_rpc")] agent_rpc: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component_dep(&context.default_environment_id, agent_rpc)
        .store()
        .await?;
    let unique_id = context.redis_prefix();
    let agent_id = agent_id!("TestAgent", unique_id);
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    executor
        .invoke_agent(
            &component,
            &agent_id,
            "long-rpc-call",
            data_value!(600000f64), // 10 minutes
        )
        .await?;

    tokio::time::sleep(Duration::from_secs(10)).await;
    executor.delete_worker(&worker_id).await?;

    let metadata = executor.get_worker_metadata(&worker_id).await;
    assert!(metadata.is_err());

    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("4m")]
async fn simulated_crash(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;
    let agent_id = agent_id!("Clocks", "simulated-crash-1");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    let mut rx = executor.capture_output(&worker_id).await?;

    let executor_clone = executor.clone();
    let component_clone = component.clone();
    let agent_id_clone = agent_id.clone();
    let fiber = tokio::spawn(
        async move {
            executor_clone
                .invoke_and_await_agent(
                    &component_clone,
                    &agent_id_clone,
                    "interruption",
                    data_value!(),
                )
                .await
        }
        .in_current_span(),
    );

    tokio::time::sleep(Duration::from_secs(5)).await;

    let _ = executor.simulated_crash(&worker_id).await;
    let result = fiber.await?;

    let mut events = vec![];
    rx.recv_many(&mut events, 100).await;
    drop(executor);

    assert!(
        result.is_ok(),
        "Expected successful result after simulated crash recovery, got: {:?}",
        result.err()
    );
    assert_eq!(
        stdout_events(events.into_iter()),
        vec!["Starting interruption test\n"]
    );
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("4m")]
async fn shopping_cart_example(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("agent_counters")] agent_counters: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component_dep(&context.default_environment_id, agent_counters)
        .store()
        .await?;

    let repo_id = agent_id!("Repository", "test-repo");
    let worker_id = executor.start_agent(&component.id, repo_id.clone()).await?;

    executor
        .invoke_and_await_agent(
            &component,
            &repo_id,
            "add",
            data_value!("G1000", "Golem T-Shirt M"),
        )
        .await?;

    executor
        .invoke_and_await_agent(
            &component,
            &repo_id,
            "add",
            data_value!("G1001", "Golem Cloud Subscription 1y"),
        )
        .await?;

    executor
        .invoke_and_await_agent(
            &component,
            &repo_id,
            "add",
            data_value!("G1002", "Mud Golem"),
        )
        .await?;

    executor
        .invoke_and_await_agent(
            &component,
            &repo_id,
            "add",
            data_value!("G1002", "Mud Golem"),
        )
        .await?;

    let contents = executor
        .invoke_and_await_agent(&component, &repo_id, "list", data_value!())
        .await?;

    executor.check_oplog_is_queryable(&worker_id).await?;
    drop(executor);

    let contents_value = contents
        .into_return_value()
        .expect("Expected a single return value");

    assert_eq!(
        contents_value,
        Value::List(vec![
            Value::Record(vec![
                Value::String("G1000".to_string()),
                Value::String("Golem T-Shirt M".to_string()),
                Value::U64(1),
            ]),
            Value::Record(vec![
                Value::String("G1001".to_string()),
                Value::String("Golem Cloud Subscription 1y".to_string()),
                Value::U64(1),
            ]),
            Value::Record(vec![
                Value::String("G1002".to_string()),
                Value::String("Mud Golem".to_string()),
                Value::U64(2),
            ]),
        ])
    );
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("4m")]
async fn dynamic_worker_creation(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;
    let agent_id = agent_id!("Environment", "dynamic-worker-creation-1");

    let args = executor
        .invoke_and_await_agent(&component, &agent_id, "get_arguments", data_value!())
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected return value"))?;

    let env = executor
        .invoke_and_await_agent(&component, &agent_id, "get_environment", data_value!())
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected return value"))?;

    let worker_name = agent_id.to_string();
    assert_eq!(args, Value::Result(Ok(Some(Box::new(Value::List(vec![]))))));
    assert_eq!(
        env,
        Value::Result(Ok(Some(Box::new(Value::List(vec![
            Value::Tuple(vec![
                Value::String("GOLEM_AGENT_ID".to_string()),
                Value::String(worker_name.clone())
            ]),
            Value::Tuple(vec![
                Value::String("GOLEM_WORKER_NAME".to_string()),
                Value::String(worker_name)
            ]),
            Value::Tuple(vec![
                Value::String("GOLEM_COMPONENT_ID".to_string()),
                Value::String(format!("{}", component.id))
            ]),
            Value::Tuple(vec![
                Value::String("GOLEM_COMPONENT_REVISION".to_string()),
                Value::String("0".to_string())
            ]),
            Value::Tuple(vec![
                Value::String("GOLEM_AGENT_TYPE".to_string()),
                Value::String("Environment".to_string()),
            ])
        ])))))
    );
    Ok(())
}

fn get_env_result_from_value(env: Value) -> HashMap<String, String> {
    match env {
        Value::Result(Ok(Some(inner))) => match *inner {
            Value::List(items) => {
                let pairs = items
                    .into_iter()
                    .filter_map(|item| match item {
                        Value::Tuple(values) if values.len() == 2 => {
                            let mut iter = values.into_iter();
                            match (iter.next(), iter.next()) {
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
#[timeout("4m")]
async fn ephemeral_worker_creation_with_name_is_not_persistent(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_counters")] agent_counters: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component_dep(&context.default_environment_id, agent_counters)
        .store()
        .await?;
    let agent_id = agent_id!("EphemeralCounter", "test");
    let _worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    let _ = executor
        .invoke_and_await_agent(&component, &agent_id, "increment", data_value!())
        .await?;

    let result = executor
        .invoke_and_await_agent(&component, &agent_id, "increment", data_value!())
        .await?
        .into_return_value();

    drop(executor);

    assert_eq!(result, Some(Value::U32(1)));
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("4m")]
async fn concurrent_ephemeral_invocations_all_complete(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_counters")] agent_counters: &PrecompiledComponent,
) -> anyhow::Result<()> {
    // Regression test: concurrent invocations on the same ephemeral agent must all
    // complete, and each must observe fresh state (counter starts at 0 each time).
    const N: usize = 8;

    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component_dep(&context.default_environment_id, agent_counters)
        .store()
        .await?;

    let agent_id = agent_id!("EphemeralCounter", "concurrent-test");
    executor.start_agent(&component.id, agent_id.clone()).await?;

    let mut handles: Vec<JoinHandle<anyhow::Result<Option<Value>>>> = Vec::with_capacity(N);
    for _ in 0..N {
        let executor_clone = executor.clone();
        let component_clone = component.clone();
        let agent_id_clone = agent_id.clone();
        handles.push(tokio::spawn(
            async move {
                let result = executor_clone
                    .invoke_and_await_agent(
                        &component_clone,
                        &agent_id_clone,
                        "increment",
                        data_value!(),
                    )
                    .await?;
                Ok(result.into_return_value())
            }
            .in_current_span(),
        ));
    }

    let mut results = Vec::with_capacity(N);
    for handle in handles {
        results.push(handle.await??);
    }

    drop(executor);

    // Every concurrent invocation must complete and return 1 — ephemeral agents
    // start with fresh state for every invocation.
    assert_eq!(results.len(), N, "all concurrent invocations must complete");
    for result in &results {
        assert_eq!(
            *result,
            Some(Value::U32(1)),
            "each ephemeral invocation must see fresh state"
        );
    }

    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("4m")]
async fn promise(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;

    let agent_id = agent_id!("GolemHostApi", "promise-1");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    let promise_id_value = executor
        .invoke_and_await_agent(&component, &agent_id, "create_promise", data_value!())
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected return value"))?;

    let promise_id_vat = ValueAndType::new(promise_id_value.clone(), PromiseId::get_type());

    let promise_data = DataValue::Tuple(ElementValues {
        elements: vec![ElementValue::ComponentModel(ComponentModelElementValue {
            value: promise_id_vat,
        })],
    });

    let poll1 = executor
        .invoke_and_await_agent(&component, &agent_id, "poll_promise", promise_data.clone())
        .await;

    let executor_clone = executor.clone();
    let component_clone = component.clone();
    let agent_id_clone = agent_id.clone();
    let promise_data_clone = promise_data.clone();

    let mut fiber = tokio::spawn(
        async move {
            executor_clone
                .invoke_and_await_agent(
                    &component_clone,
                    &agent_id_clone,
                    "await_promise",
                    promise_data_clone,
                )
                .await
        }
        .in_current_span(),
    );

    info!("Waiting for worker to be suspended on promise");

    tokio::select! {
        result = &mut fiber => {
            let invoke_result = result??;
            return Err(anyhow!("await_promise returned immediately instead of suspending: {:?}", invoke_result));
        }
        status = executor.wait_for_status(&worker_id, AgentStatus::Suspended, Duration::from_secs(10)) => {
            status?;
        }
    }

    info!("Completing promise to resume worker");

    let oplog_idx = extract_oplog_idx_from_promise_id(&promise_id_value);
    executor
        .complete_promise(
            &PromiseId {
                agent_id: worker_id.clone(),
                oplog_idx,
            },
            vec![42],
        )
        .await?;

    let result = fiber.await??;

    let poll2 = executor
        .invoke_and_await_agent(&component, &agent_id, "poll_promise", promise_data)
        .await;

    executor.check_oplog_is_queryable(&worker_id).await?;

    let result_value = result
        .into_return_value()
        .ok_or_else(|| anyhow!("expected return value"))?;
    assert_eq!(result_value, Value::List(vec![Value::U8(42)]));

    let poll1_value = poll1?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected return value"))?;
    assert_eq!(poll1_value, Value::Option(None));

    let poll2_value = poll2?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected return value"))?;
    assert_eq!(
        poll2_value,
        Value::Option(Some(Box::new(Value::List(vec![Value::U8(42)]))))
    );
    Ok(())
}

fn extract_oplog_idx_from_promise_id(promise_id_value: &Value) -> OplogIndex {
    let Value::Record(fields) = promise_id_value else {
        panic!("Expected a record for PromiseId");
    };
    let Value::U64(oplog_idx) = fields[1] else {
        panic!("Expected u64 oplog-idx field");
    };
    OplogIndex::from_u64(oplog_idx)
}

#[test]
#[tracing::instrument]
#[timeout("4m")]
async fn get_workers_from_worker(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("golem_host")] type_resolve: &SharedAnalysedTypeResolve,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;

    let agent_id1 = agent_id!("GolemHostApi", "worker-3");
    let worker_id1 = executor
        .start_agent(&component.id, agent_id1.clone())
        .await?;

    let agent_id2 = agent_id!("GolemHostApi", "worker-4");
    let worker_id2 = executor
        .start_agent(&component.id, agent_id2.clone())
        .await?;

    async fn get_check(
        component: &ComponentDto,
        caller_agent_id: &golem_common::base_model::agent::ParsedAgentId,
        name_filter: Option<String>,
        expected_count: usize,
        executor: &TestWorkerExecutor,
        mut type_resolve: SharedAnalysedTypeResolve,
    ) -> anyhow::Result<()> {
        let component_id_val_and_type = {
            let (high, low) = component.id.0.as_u64_pair();
            Record(vec![(
                "uuid",
                Record(vec![
                    ("high-bits", high.into_value_and_type()),
                    ("low-bits", low.into_value_and_type()),
                ])
                .into_value_and_type(),
            )])
            .into_value_and_type()
        };

        let filter_val = name_filter.map(|name| {
            Value::Record(vec![Value::List(vec![Value::Record(vec![Value::List(
                vec![Value::Variant {
                    case_idx: 0,
                    case_value: Some(Box::new(Value::Record(vec![
                        Value::Variant {
                            case_idx: 0,
                            case_value: None,
                        },
                        Value::String(name),
                    ]))),
                }],
            )])])])
        });

        let filter_val_and_type = ValueAndType {
            value: Value::Option(filter_val.map(Box::new)),
            typ: analysed_type::option(
                type_resolve
                    .analysed_type(&TypeName {
                        package: Some("golem:api@1.5.0".to_string()),
                        owner: TypeOwner::Interface("host".to_string()),
                        name: Some("agent-any-filter".to_string()),
                    })
                    .unwrap(),
            ),
        };

        let params = DataValue::Tuple(ElementValues {
            elements: vec![
                ElementValue::ComponentModel(ComponentModelElementValue {
                    value: component_id_val_and_type,
                }),
                ElementValue::ComponentModel(ComponentModelElementValue {
                    value: filter_val_and_type,
                }),
                ElementValue::ComponentModel(ComponentModelElementValue {
                    value: true.into_value_and_type(),
                }),
            ],
        });

        let result = executor
            .invoke_and_await_agent(component, caller_agent_id, "get_workers", params)
            .await?;

        let result_value = result
            .into_return_value()
            .ok_or_else(|| anyhow!("expected return value"))?;

        match result_value {
            Value::List(list) => {
                assert_eq!(list.len(), expected_count);
            }
            _ => {
                panic!("unexpected");
            }
        }
        Ok(())
    }
    get_check(
        &component,
        &agent_id1,
        None,
        2,
        &executor,
        type_resolve.clone(),
    )
    .await?;
    get_check(
        &component,
        &agent_id2,
        Some("GolemHostApi(\"worker-3\")".to_string()),
        1,
        &executor,
        type_resolve.clone(),
    )
    .await?;

    executor.check_oplog_is_queryable(&worker_id1).await?;
    executor.check_oplog_is_queryable(&worker_id2).await?;
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("4m")]
async fn get_metadata_from_worker(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;

    let agent_id1 = agent_id!("GolemHostApi", "worker-1");
    let worker_id1 = executor
        .start_agent(&component.id, agent_id1.clone())
        .await?;

    let agent_id2 = agent_id!("GolemHostApi", "worker-2");
    let worker_id2 = executor
        .start_agent(&component.id, agent_id2.clone())
        .await?;

    fn get_agent_id_val(
        component_id: &ComponentId,
        agent_id: &golem_common::base_model::agent::ParsedAgentId,
    ) -> Value {
        let component_id_val = {
            let (high, low) = component_id.0.as_u64_pair();
            Value::Record(vec![Value::Record(vec![Value::U64(high), Value::U64(low)])])
        };

        Value::Record(vec![component_id_val, Value::String(agent_id.to_string())])
    }

    async fn get_check(
        component: &ComponentDto,
        caller_agent_id: &golem_common::base_model::agent::ParsedAgentId,
        other_agent_id: &golem_common::base_model::agent::ParsedAgentId,
        executor: &TestWorkerExecutor,
    ) -> anyhow::Result<()> {
        let agent_id_val1 = get_agent_id_val(&component.id, caller_agent_id);

        let result = executor
            .invoke_and_await_agent(component, caller_agent_id, "get_self_uri", data_value!())
            .await?;

        let result_value = result
            .into_return_value()
            .ok_or_else(|| anyhow!("expected return value"))?;

        match result_value {
            Value::Record(values) if !values.is_empty() => {
                let id_val = values.first().unwrap();
                assert_eq!(agent_id_val1, *id_val);
            }
            _ => {
                panic!("unexpected");
            }
        }

        let agent_id_val2 = get_agent_id_val(&component.id, other_agent_id);

        let other_agent_id_val_and_type = ValueAndType {
            value: agent_id_val2.clone(),
            typ: analysed_type::record(vec![
                analysed_type::field(
                    "component-id",
                    analysed_type::record(vec![analysed_type::field(
                        "uuid",
                        analysed_type::record(vec![
                            analysed_type::field("high-bits", analysed_type::u64()),
                            analysed_type::field("low-bits", analysed_type::u64()),
                        ]),
                    )]),
                ),
                analysed_type::field("agent-id", analysed_type::str()),
            ]),
        };

        let params = DataValue::Tuple(ElementValues {
            elements: vec![ElementValue::ComponentModel(ComponentModelElementValue {
                value: other_agent_id_val_and_type,
            })],
        });

        let result = executor
            .invoke_and_await_agent(component, caller_agent_id, "get_worker_metadata", params)
            .await?;

        let result_value = result
            .into_return_value()
            .ok_or_else(|| anyhow!("expected return value"))?;

        match result_value {
            Value::Option(value) if value.is_some() => {
                let result = *value.unwrap();
                match result {
                    Value::Record(values) if !values.is_empty() => {
                        let id_val = values.first().unwrap();
                        assert_eq!(agent_id_val2, *id_val);
                    }
                    _ => {
                        panic!("unexpected");
                    }
                }
            }
            _ => {
                panic!("unexpected");
            }
        }
        Ok(())
    }

    get_check(&component, &agent_id1, &agent_id2, &executor).await?;
    get_check(&component, &agent_id2, &agent_id1, &executor).await?;

    executor.check_oplog_is_queryable(&worker_id1).await?;
    executor.check_oplog_is_queryable(&worker_id2).await?;
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("4m")]
async fn invoking_with_same_idempotency_key_is_idempotent(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("agent_counters")] agent_counters: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component_dep(&context.default_environment_id, agent_counters)
        .store()
        .await?;

    let repo_id = agent_id!("Repository", "test-repo-2");
    let worker_id = executor.start_agent(&component.id, repo_id.clone()).await?;

    let idempotency_key = IdempotencyKey::fresh();
    executor
        .invoke_and_await_agent_with_key(
            &component,
            &repo_id,
            &idempotency_key,
            "add",
            data_value!("G1000", "Golem T-Shirt M"),
        )
        .await?;

    executor
        .invoke_and_await_agent_with_key(
            &component,
            &repo_id,
            &idempotency_key,
            "add",
            data_value!("G1000", "Golem T-Shirt M"),
        )
        .await?;

    let contents = executor
        .invoke_and_await_agent(&component, &repo_id, "list", data_value!())
        .await?;

    executor.check_oplog_is_queryable(&worker_id).await?;

    let contents_value = contents
        .into_return_value()
        .expect("Expected a single return value");

    assert_eq!(
        contents_value,
        Value::List(vec![Value::Record(vec![
            Value::String("G1000".to_string()),
            Value::String("Golem T-Shirt M".to_string()),
            Value::U64(1),
        ])])
    );
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("4m")]
async fn invoking_with_same_idempotency_key_is_idempotent_after_restart(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("agent_counters")] agent_counters: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component_dep(&context.default_environment_id, agent_counters)
        .store()
        .await?;

    let repo_id = agent_id!("Repository", "test-repo-3");
    let worker_id = executor.start_agent(&component.id, repo_id.clone()).await?;

    let idempotency_key = IdempotencyKey::fresh();
    executor
        .invoke_and_await_agent_with_key(
            &component,
            &repo_id,
            &idempotency_key,
            "add",
            data_value!("G1000", "Golem T-Shirt M"),
        )
        .await?;

    drop(executor);
    let executor = start(deps, &context).await?;

    executor
        .invoke_and_await_agent_with_key(
            &component,
            &repo_id,
            &idempotency_key,
            "add",
            data_value!("G1000", "Golem T-Shirt M"),
        )
        .await?;

    let contents = executor
        .invoke_and_await_agent(&component, &repo_id, "list", data_value!())
        .await?;

    executor.check_oplog_is_queryable(&worker_id).await?;

    let contents_value = contents
        .into_return_value()
        .expect("Expected a single return value");

    assert_eq!(
        contents_value,
        Value::List(vec![Value::Record(vec![
            Value::String("G1000".to_string()),
            Value::String("Golem T-Shirt M".to_string()),
            Value::U64(1),
        ])])
    );
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("4m")]
async fn component_env_variables(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .with_env("Environment", vec![("FOO".to_string(), "bar".to_string())])
        .store()
        .await?;

    let agent_id = agent_id!("Environment", "component-env-variables-1");
    let worker_name = agent_id.to_string();

    let env = executor
        .invoke_and_await_agent(&component, &agent_id, "get_environment", data_value!())
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected return value"))?;

    assert_eq!(
        env,
        Value::Result(Ok(Some(Box::new(Value::List(vec![
            Value::Tuple(vec![
                Value::String("FOO".to_string()),
                Value::String("bar".to_string())
            ]),
            Value::Tuple(vec![
                Value::String("GOLEM_AGENT_ID".to_string()),
                Value::String(worker_name.clone())
            ]),
            Value::Tuple(vec![
                Value::String("GOLEM_WORKER_NAME".to_string()),
                Value::String(worker_name)
            ]),
            Value::Tuple(vec![
                Value::String("GOLEM_COMPONENT_ID".to_string()),
                Value::String(format!("{}", component.id))
            ]),
            Value::Tuple(vec![
                Value::String("GOLEM_COMPONENT_REVISION".to_string()),
                Value::String("0".to_string())
            ]),
            Value::Tuple(vec![
                Value::String("GOLEM_AGENT_TYPE".to_string()),
                Value::String("Environment".to_string())
            ]),
        ])))))
    );

    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("4m")]
async fn component_env_variables_update(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .with_env("Environment", vec![("FOO".to_string(), "bar".to_string())])
        .store()
        .await?;

    let agent_id = agent_id!("Environment", "component-env-variables-1");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    let AgentMetadataDto { mut env, .. } = executor.get_worker_metadata(&worker_id).await?;

    env.retain(|k, _| k == "FOO");

    assert_eq!(
        env,
        HashMap::from_iter(vec![("FOO".to_string(), "bar".to_string())])
    );

    let updated_component = executor
        .update_component_with_env(
            &component.id,
            "Environment",
            "golem_it_host_api_tests_release",
            &[("BAR".to_string(), "baz".to_string())],
        )
        .await?;

    executor
        .auto_update_worker(&worker_id, updated_component.revision, false)
        .await?;

    executor
        .wait_for_component_revision(
            &worker_id,
            updated_component.revision,
            Duration::from_secs(30),
        )
        .await?;

    let env = executor
        .invoke_and_await_agent(&component, &agent_id, "get_environment", data_value!())
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected return value"))?;

    let env = get_env_result_from_value(env);

    assert_eq!(env.get("FOO"), Some(&"bar".to_string()));
    assert_eq!(env.get("BAR"), Some(&"baz".to_string()));
    let worker_name = agent_id.to_string();
    assert_eq!(env.get("GOLEM_AGENT_ID"), Some(&worker_name));

    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("4m")]
async fn component_env_and_worker_env_priority(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .with_env("Environment", vec![("FOO".to_string(), "bar".to_string())])
        .store()
        .await?;

    let agent_id = agent_id!("Environment", "component-env-variables-1");
    let worker_env = HashMap::from_iter(vec![("FOO".to_string(), "baz".to_string())]);

    let worker_id = executor
        .start_agent_with(
            &component.id,
            agent_id.clone(),
            worker_env,
            HashMap::new(),
            Vec::new(),
        )
        .await?;

    let AgentMetadataDto { mut env, .. } = executor.get_worker_metadata(&worker_id).await?;
    env.retain(|k, _| k == "FOO");

    assert_eq!(
        env,
        HashMap::from_iter(vec![("FOO".to_string(), "baz".to_string())])
    );
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("4m")]
async fn delete_worker(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("agent_counters")] agent_counters: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component_dep(&context.default_environment_id, agent_counters)
        .store()
        .await?;

    let counter_id = agent_id!("Counter", "delete-worker-1");
    let worker_id = executor
        .start_agent(&component.id, counter_id.clone())
        .await?;

    executor
        .invoke_and_await_agent(&component, &counter_id, "increment", data_value!())
        .await?;

    let metadata1 = executor.get_worker_metadata(&worker_id).await;

    let (cursor1, values1) = executor
        .get_workers_metadata(
            &worker_id.component_id,
            Some(AgentFilter::new_name(
                StringFilterComparator::Equal,
                worker_id.agent_id.clone(),
            )),
            ScanCursor::default(),
            10,
            true,
        )
        .await?;

    executor.delete_worker(&worker_id).await?;

    let metadata2 = executor.get_worker_metadata(&worker_id).await;

    assert_eq!(values1.len(), 1);
    assert!(cursor1.is_none());
    assert!(metadata1.is_ok());
    assert!(metadata2.is_err());
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("4m")]
async fn delete_nonexistent_worker(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("agent_counters")] agent_counters: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component_dep(&context.default_environment_id, agent_counters)
        .store()
        .await?;

    let counter_id = agent_id!("Counter", "delete-nonexistent-1");
    let worker_id = AgentId {
        component_id: component.id,
        agent_id: counter_id.to_string(),
    };

    let result = executor.delete_worker(&worker_id).await;

    let err = result.expect_err("Deleting a non-existent worker should fail");
    let err_msg = format!("{err:?}");
    assert!(
        err_msg.contains("AgentNotFound"),
        "Expected AgentNotFound error, got: {err_msg}"
    );
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("4m")]
async fn get_workers(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("agent_counters")] agent_counters: &PrecompiledComponent,
) -> anyhow::Result<()> {
    async fn get_check(
        component_id: &ComponentId,
        filter: Option<AgentFilter>,
        expected_count: usize,
        executor: &TestWorkerExecutor,
    ) -> anyhow::Result<Vec<AgentMetadataDto>> {
        let (cursor, values) = executor
            .get_workers_metadata(component_id, filter, ScanCursor::default(), 20, true)
            .await?;

        assert_eq!(values.len(), expected_count);
        assert!(cursor.is_none());

        Ok(values)
    }

    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component_dep(&context.default_environment_id, agent_counters)
        .store()
        .await?;

    let workers_count = 10;
    let mut worker_ids = vec![];

    for i in 0..workers_count {
        let agent_id = agent_id!("Counter", format!("test-worker-{i}"));
        let worker_id = executor
            .start_agent(&component.id, agent_id.clone())
            .await?;

        worker_ids.push((worker_id, agent_id));
    }

    for (worker_id, agent_id) in worker_ids.clone() {
        executor
            .invoke_and_await_agent(&component, &agent_id, "increment", data_value!())
            .await?;

        get_check(
            &component.id,
            Some(AgentFilter::new_name(
                StringFilterComparator::Equal,
                worker_id.agent_id.clone(),
            )),
            1,
            &executor,
        )
        .await?;
    }

    get_check(
        &component.id,
        Some(AgentFilter::new_name(
            StringFilterComparator::Like,
            "test-worker".to_string(),
        )),
        workers_count,
        &executor,
    )
    .await?;

    get_check(
        &component.id,
        Some(
            AgentFilter::new_name(StringFilterComparator::Like, "test-worker".to_string())
                .and(
                    AgentFilter::new_status(FilterComparator::Equal, AgentStatus::Idle).or(
                        AgentFilter::new_status(FilterComparator::Equal, AgentStatus::Running),
                    ),
                )
                .and(AgentFilter::new_revision(
                    FilterComparator::Equal,
                    ComponentRevision::INITIAL,
                )),
        ),
        workers_count,
        &executor,
    )
    .await?;

    get_check(
        &component.id,
        Some(AgentFilter::new_name(StringFilterComparator::Like, "test-worker".to_string()).not()),
        0,
        &executor,
    )
    .await?;

    get_check(&component.id, None, workers_count, &executor).await?;

    let (cursor1, values1) = executor
        .get_workers_metadata(
            &component.id,
            None,
            ScanCursor::default(),
            (workers_count / 2) as u64,
            true,
        )
        .await?;

    assert!(cursor1.is_some());
    assert!(values1.len() >= workers_count / 2);

    let (cursor2, values2) = executor
        .get_workers_metadata(
            &component.id,
            None,
            cursor1.unwrap(),
            (workers_count - values1.len()) as u64,
            true,
        )
        .await?;

    assert_eq!(values2.len(), workers_count - values1.len());

    if let Some(cursor2) = cursor2 {
        let (_, values3) = executor
            .get_workers_metadata(&component.id, None, cursor2, workers_count as u64, true)
            .await?;
        assert_eq!(values3.len(), 0);
    }

    for (worker_id, _) in worker_ids {
        executor.delete_worker(&worker_id).await?;
    }

    get_check(&component.id, None, 0, &executor).await?;
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("4m")]
async fn error_handling_when_worker_is_invoked_with_fewer_than_expected_parameters(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_counters")] agent_counters: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component_dep(&context.default_environment_id, agent_counters)
        .store()
        .await?;

    let agent_id = agent_id!("FailingCounter", "fewer-than-expected-parameters-1");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    let failure = executor
        .invoke_and_await_agent(&component, &agent_id, "add", data_value!())
        .await;

    executor.check_oplog_is_queryable(&worker_id).await?;
    assert!(failure.is_err());
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("4m")]
#[ignore] // TODO: Fix this test as part of the
async fn error_handling_when_worker_is_invoked_with_more_than_expected_parameters(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_counters")] agent_counters: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component_dep(&context.default_environment_id, agent_counters)
        .store()
        .await?;

    let agent_id = agent_id!("Counter", "more-than-expected-parameters-1");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    let failure = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "increment",
            data_value!("extra parameter"),
        )
        .await;

    executor.check_oplog_is_queryable(&worker_id).await?;

    assert!(failure.is_err());

    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("4m")]
async fn get_worker_metadata(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;

    let agent_id = agent_id!("Clock", "get-worker-metadata-1");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    let executor_clone = executor.clone();
    let component_clone = component.clone();
    let agent_id_clone = agent_id.clone();
    let fiber = tokio::spawn(
        async move {
            executor_clone
                .invoke_and_await_agent(
                    &component_clone,
                    &agent_id_clone,
                    "sleep",
                    data_value!(2u64),
                )
                .await
        }
        .in_current_span(),
    );

    let metadata1 = executor
        .wait_for_statuses(
            &worker_id,
            &[AgentStatus::Running, AgentStatus::Suspended],
            Duration::from_secs(5),
        )
        .await?;

    fiber.await??;

    let metadata2 = executor
        .wait_for_status(&worker_id, AgentStatus::Idle, Duration::from_secs(10))
        .await?;

    executor.check_oplog_is_queryable(&worker_id).await?;

    assert!(
        metadata1.status == AgentStatus::Suspended || // it is sleeping - whether it is suspended or not is the server's decision
        metadata1.status == AgentStatus::Running
    );
    assert_eq!(metadata2.status, AgentStatus::Idle);
    assert_eq!(metadata1.component_revision, ComponentRevision::INITIAL);
    assert_eq!(metadata1.agent_id, worker_id);
    assert_eq!(metadata1.created_by, context.account_id);

    let component_file_size = std::fs::metadata(
        executor
            .deps
            .component_directory
            .join("golem_it_host_api_tests_release.wasm"),
    )?
    .len();
    assert_eq!(metadata2.component_size, component_file_size);
    assert_eq!(metadata2.total_linear_memory_size, 1703936);
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("4m")]
async fn create_invoke_delete_create_invoke(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("agent_counters")] agent_counters: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component_dep(&context.default_environment_id, agent_counters)
        .store()
        .await?;

    let counter_id = agent_id!("Counter", "delete-recreate-test");
    let worker_id = executor
        .start_agent(&component.id, counter_id.clone())
        .await?;

    let r1 = executor
        .invoke_and_await_agent(&component, &counter_id, "increment", data_value!())
        .await?;
    assert_eq!(r1.into_return_value(), Some(Value::U32(1)));

    let r2 = executor
        .invoke_and_await_agent(&component, &counter_id, "increment", data_value!())
        .await?;
    assert_eq!(r2.into_return_value(), Some(Value::U32(2)));

    executor.delete_worker(&worker_id).await?;

    let worker_id = executor
        .start_agent(&component.id, counter_id.clone())
        .await?;

    let r3 = executor
        .invoke_and_await_agent(&component, &counter_id, "increment", data_value!())
        .await?;
    assert_eq!(r3.into_return_value(), Some(Value::U32(1)));

    executor.check_oplog_is_queryable(&worker_id).await?;

    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("6m")]
async fn recovering_an_old_worker_after_updating_a_component(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("agent_counters")] agent_counters: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component_dep(&context.default_environment_id, agent_counters)
        .store()
        .await?;

    let counter_id = agent_id!("Counter", "recover-test");
    let _worker_id = executor
        .start_agent(&component.id, counter_id.clone())
        .await?;

    let r1 = executor
        .invoke_and_await_agent(&component, &counter_id, "increment", data_value!())
        .await?;

    // Updating the component with an incompatible new version
    executor
        .update_component(&component.id, "golem_it_agent_rpc")
        .await?;

    // Creating a new worker of the updated component and call it
    let new_agent_id = agent_id!("SimpleChildAgent", "recover-test-new");
    let _worker_id2 = executor
        .start_agent(&component.id, new_agent_id.clone())
        .await?;

    let r2 = executor
        .invoke_and_await_agent(&component, &new_agent_id, "value", data_value!())
        .await?;

    // Restarting the server to force worker recovery
    drop(executor);
    let executor = start(deps, &context).await?;

    // Call the first worker again to check if it is still working
    let r3 = executor
        .invoke_and_await_agent(&component, &counter_id, "increment", data_value!())
        .await?;

    let worker_id = AgentId::from_agent_id(component.id, &counter_id)
        .map_err(|err| anyhow!("Invalid agent id: {err}"))?;
    executor.check_oplog_is_queryable(&worker_id).await?;
    drop(executor);

    assert_eq!(r1.into_return_value(), Some(Value::U32(1)));
    assert_eq!(r2.into_return_value(), Some(Value::F64(1.0)));
    assert_eq!(r3.into_return_value(), Some(Value::U32(2)));
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("4m")]
async fn recreating_a_worker_after_it_got_deleted_with_a_different_version(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("agent_counters")] agent_counters: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component_dep(&context.default_environment_id, agent_counters)
        .store()
        .await?;

    let counter_id = agent_id!("Counter", "recreate-after-delete");
    let worker_id = executor
        .start_agent(&component.id, counter_id.clone())
        .await?;

    let r1 = executor
        .invoke_and_await_agent(&component, &counter_id, "increment", data_value!())
        .await?;

    // Updating the component with an incompatible new version that also has a "counter" agent
    // but with different methods (get-value -> string instead of increment -> u32)
    executor
        .update_component(&component.id, "golem_it_agent_rpc_rust_release")
        .await?;

    // Deleting the first instance
    executor.delete_worker(&worker_id).await?;

    // Recreate the same agent (same type name and constructor params) on the updated component
    let worker_id = executor
        .start_agent(&component.id, counter_id.clone())
        .await?;

    let r2 = executor
        .invoke_and_await_agent(&component, &counter_id, "get_value", data_value!())
        .await?;

    executor.check_oplog_is_queryable(&worker_id).await?;
    drop(executor);

    assert_eq!(r1.into_return_value(), Some(Value::U32(1)));
    assert_eq!(
        r2.into_return_value(),
        Some(Value::String("counter-recreate-after-delete".to_string()))
    );
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("4m")]
async fn trying_to_use_a_wasm_that_wasmtime_cannot_load_provides_good_error_message(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    // case: WASM can be parsed, but wasmtime does not support it
    let executor = start(deps, &context).await?;
    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;

    let target_dir = deps.component_service_directory.clone();
    let component_path = target_dir.join(format!("wasms/{}-0.wasm", component.id));

    let span = Span::current();
    tokio::task::spawn_blocking(move || {
        let _enter = span.enter();

        let mut file = std::fs::File::options()
            .write(true)
            .truncate(false)
            .open(&component_path)
            .expect("Failed to open component file");
        file.write_at(&[1, 2, 3, 4], 0)
            .expect("Failed to write to component file");
        file.flush().expect("Failed to flush component file");
    })
    .await
    .unwrap();

    let agent_id = agent_id!("Clocks", "bad-wasm-1");
    let result = executor.try_start_agent(&component.id, agent_id).await?;

    let Err(WorkerExecutorError::ComponentParseFailed {
        component_id,
        component_revision,
        reason,
    }) = result
    else {
        panic!("Expected ComponentParseFailed, got {:?}", result)
    };
    assert_eq!(component_id, component.id);
    assert_eq!(component_revision, ComponentRevision::INITIAL);
    assert_eq!(reason, "failed to parse WebAssembly module");
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("4m")]
async fn trying_to_use_a_wasm_that_wasmtime_cannot_load_provides_good_error_message_after_recovery(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;
    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;

    let agent_id = agent_id!("Clocks", "bad-wasm-2");
    let worker_id = executor
        .try_start_agent(&component.id, agent_id.clone())
        .await??;

    // worker is idle. if we restart the server, it will get recovered
    drop(executor);

    // corrupting the uploaded WASM
    let component_path = deps
        .component_service_directory
        .join(format!("wasms/{}-0.wasm", component.id));
    let compiled_component_path = deps.blob_storage_root().join(format!(
        "compilation_cache/{}/{}/0.cwasm",
        component.environment_id, component.id
    ));

    let span = Span::current();
    tokio::task::spawn_blocking(move || {
        let _enter = span.enter();
        debug!("Corrupting {:?}", component_path);
        let mut file = std::fs::File::options()
            .write(true)
            .truncate(false)
            .open(&component_path)
            .expect("Failed to open component file");
        file.write_at(&[1, 2, 3, 4], 0)
            .expect("Failed to write to component file");
        file.flush().expect("Failed to flush component file");

        debug!("Deleting {:?}", compiled_component_path);
        std::fs::remove_file(&compiled_component_path)
            .expect("Failed to delete compiled component");
    })
    .await
    .unwrap();

    let executor = start(deps, &context).await?;

    debug!("Trying to invoke recovered worker");

    // trying to invoke the previously created worker
    let result = executor
        .invoke_and_await_agent(&component, &agent_id, "run", data_value!())
        .await;

    let err = result.expect_err("Expected ComponentParseFailed error");
    let err_msg = format!("{err}");
    assert!(
        err_msg.contains("failed to parse WebAssembly module"),
        "Expected ComponentParseFailed error, got: {err_msg}"
    );

    executor.check_oplog_is_queryable(&worker_id).await?;

    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("6m")]
async fn long_running_poll_loop_works_as_expected(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("http_tests")] http_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let response = Arc::new(Mutex::new("initial".to_string()));
    let response_clone = response.clone();

    let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await.unwrap();

    let host_http_port = listener.local_addr().unwrap().port();

    let http_server = tokio::spawn(
        async move {
            let route = Router::new().route(
                "/poll",
                get(move || async move {
                    let body = response_clone.lock().unwrap();
                    body.clone()
                }),
            );

            axum::serve(listener, route).await.unwrap();
        }
        .in_current_span(),
    );

    let component = executor
        .component_dep(&context.default_environment_id, http_tests)
        .store()
        .await?;
    let agent_id = agent_id!("HttpClient2");
    let mut env = HashMap::new();
    env.insert("PORT".to_string(), host_http_port.to_string());
    env.insert("RUST_BACKTRACE".to_string(), "1".to_string());

    let worker_id = executor
        .start_agent_with(
            &component.id,
            agent_id.clone(),
            env,
            HashMap::new(),
            Vec::new(),
        )
        .await?;

    executor.log_output(&worker_id).await?;

    executor
        .invoke_agent(&component, &agent_id, "start_polling", data_value!("first"))
        .await?;

    executor
        .wait_for_status(&worker_id, AgentStatus::Running, Duration::from_secs(10))
        .await?;

    {
        let mut response = response.lock().unwrap();
        *response = "first".to_string();
    }

    executor
        .wait_for_status(&worker_id, AgentStatus::Idle, Duration::from_secs(10))
        .await?;

    executor.check_oplog_is_queryable(&worker_id).await?;
    drop(executor);
    http_server.abort();
    Ok(())
}

async fn start_http_poll_server(
    response: Arc<Mutex<String>>,
    poll_count: Arc<AtomicUsize>,
    forced_port: Option<u16>,
) -> (u16, JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{}", forced_port.unwrap_or(0)))
        .await
        .unwrap();

    let host_http_port = listener.local_addr().unwrap().port();

    let http_server = tokio::spawn(
        async move {
            let route = Router::new().route(
                "/poll",
                get(move || async move {
                    let body = response.lock().unwrap();
                    poll_count.fetch_add(1, Ordering::Release);
                    body.clone()
                }),
            );

            axum::serve(listener, route).await.unwrap();
        }
        .in_current_span(),
    );

    (host_http_port, http_server)
}

#[test]
#[tracing::instrument]
#[timeout("6m")]
async fn long_running_poll_loop_http_failures_are_retried(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("http_tests")] http_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start_customized(
        deps,
        &context,
        None,
        None,
        Some(RetryConfig {
            max_attempts: 30,
            min_delay: Duration::from_millis(100),
            max_delay: Duration::from_millis(500),
            multiplier: 1.5,
            max_jitter_factor: None,
        }),
        None,
        None,
        None,
    )
    .await?;

    let response = Arc::new(Mutex::new("initial".to_string()));
    let poll_count = Arc::new(AtomicUsize::new(0));

    let (host_http_port, http_server) =
        start_http_poll_server(response.clone(), poll_count.clone(), None).await;

    let component = executor
        .component_dep(&context.default_environment_id, http_tests)
        .store()
        .await?;
    let agent_id = agent_id!("HttpClient2");
    let mut env = HashMap::new();
    env.insert("PORT".to_string(), host_http_port.to_string());
    env.insert("RUST_BACKTRACE".to_string(), "1".to_string());

    let worker_id = executor
        .start_agent_with(
            &component.id,
            agent_id.clone(),
            env,
            HashMap::new(),
            Vec::new(),
        )
        .await?;

    executor.log_output(&worker_id).await?;

    executor
        .invoke_agent(
            &component,
            &agent_id,
            "start_polling",
            data_value!("stop now"),
        )
        .await?;

    executor
        .wait_for_status(&worker_id, AgentStatus::Running, Duration::from_secs(10))
        .await?;
    // Poll loop is running. Wait until a given poll count
    let begin = Instant::now();
    loop {
        if begin.elapsed() > Duration::from_secs(2) {
            return Err(anyhow!("No polls in 2 seconds"));
        }

        if poll_count.load(Ordering::Acquire) >= 3 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    // Kill the HTTP server
    http_server.abort();

    // Wait more than the poll cycle time
    tokio::time::sleep(Duration::from_secs(1)).await;

    // Restart the HTTP server (TODO: another test could have taken the port for now - we need to retry until we can bind again)
    let (_, http_server) =
        start_http_poll_server(response.clone(), poll_count.clone(), Some(host_http_port)).await;

    // Wait until more polls are coming in
    let begin = Instant::now();
    loop {
        if begin.elapsed() > Duration::from_secs(30) {
            return Err(anyhow!(
                "No polls in 30 seconds (poll_count={})",
                poll_count.load(Ordering::Acquire)
            ));
        }

        if poll_count.load(Ordering::Acquire) >= 6 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    // Finish signal

    {
        let mut response = response.lock().unwrap();
        *response = "stop now".to_string();
    }

    executor
        .wait_for_status(&worker_id, AgentStatus::Idle, Duration::from_secs(30))
        .await?;

    executor.check_oplog_is_queryable(&worker_id).await?;
    drop(executor);
    http_server.abort();
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("6m")]
async fn long_running_poll_loop_works_as_expected_async_http(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("http_tests")] http_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;
    let response = Arc::new(Mutex::new("initial".to_string()));
    let response_clone = response.clone();

    let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await.unwrap();

    let host_http_port = listener.local_addr().unwrap().port();

    let http_server = tokio::spawn(
        async move {
            let route = Router::new().route(
                "/poll",
                get(move || async move {
                    let body = response_clone.lock().unwrap();
                    body.clone()
                }),
            );

            axum::serve(listener, route).await.unwrap();
        }
        .in_current_span(),
    );

    let component = executor
        .component_dep(&context.default_environment_id, http_tests)
        .store()
        .await?;
    let agent_id = agent_id!("HttpClient3");
    let mut env = HashMap::new();
    env.insert("PORT".to_string(), host_http_port.to_string());
    env.insert("RUST_BACKTRACE".to_string(), "1".to_string());

    let worker_id = executor
        .start_agent_with(
            &component.id,
            agent_id.clone(),
            env,
            HashMap::new(),
            Vec::new(),
        )
        .await?;

    executor.log_output(&worker_id).await?;

    executor
        .invoke_agent(&component, &agent_id, "start_polling", data_value!("first"))
        .await?;

    executor
        .wait_for_status(&worker_id, AgentStatus::Running, Duration::from_secs(10))
        .await?;

    {
        let mut response = response.lock().unwrap();
        *response = "first".to_string();
    }

    executor
        .wait_for_status(&worker_id, AgentStatus::Idle, Duration::from_secs(10))
        .await?;

    executor.check_oplog_is_queryable(&worker_id).await?;
    drop(executor);
    http_server.abort();
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout(300_000)]
async fn long_running_poll_loop_interrupting_and_resuming_by_second_invocation(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("http_tests")] http_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let response = Arc::new(Mutex::new("initial".to_string()));
    let response_clone = response.clone();

    let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await.unwrap();

    let host_http_port = listener.local_addr().unwrap().port();

    let http_server = tokio::spawn(
        async move {
            let route = Router::new().route(
                "/poll",
                get(move || async move {
                    let body = response_clone.lock().unwrap();
                    body.clone()
                }),
            );

            axum::serve(listener, route).await.unwrap();
        }
        .in_current_span(),
    );

    let component = executor
        .component_dep(&context.default_environment_id, http_tests)
        .store()
        .await?;
    let agent_id = agent_id!("HttpClient2");
    let mut env = HashMap::new();
    env.insert("PORT".to_string(), host_http_port.to_string());
    let worker_id = executor
        .start_agent_with(
            &component.id,
            agent_id.clone(),
            env,
            HashMap::new(),
            Vec::new(),
        )
        .await?;

    executor.log_output(&worker_id).await?;

    executor
        .invoke_agent(&component, &agent_id, "start_polling", data_value!("first"))
        .await?;

    executor
        .wait_for_status(&worker_id, AgentStatus::Running, Duration::from_secs(20))
        .await?;

    let values1 = executor
        .get_running_workers_metadata(
            &worker_id.component_id,
            Some(AgentFilter::new_name(
                StringFilterComparator::Equal,
                worker_id.agent_id.clone(),
            )),
        )
        .await?;

    executor.interrupt(&worker_id).await?;

    executor
        .wait_for_status(&worker_id, AgentStatus::Interrupted, Duration::from_secs(5))
        .await?;

    let values2 = executor
        .get_running_workers_metadata(
            &worker_id.component_id,
            Some(AgentFilter::new_name(
                StringFilterComparator::Equal,
                worker_id.agent_id.clone(),
            )),
        )
        .await?;

    executor
        .invoke_agent(
            &component,
            &agent_id,
            "start_polling",
            data_value!("second"),
        )
        .await?;

    executor
        .wait_for_status(&worker_id, AgentStatus::Running, Duration::from_secs(20))
        .await?;

    let (mut rx, _abort_capture) = executor.capture_output_with_termination(&worker_id).await?;

    {
        let mut response = response.lock().unwrap();
        *response = "first".to_string();
    }

    // wait for the first invocation to finish
    {
        let mut found = false;
        while !found {
            match rx.recv().await {
                Some(Some(event)) => {
                    if stdout_event_matching(&event, "Poll loop finished\n") {
                        found = true;
                    }
                }
                _ => {
                    return Err(anyhow!("Did not receive the expected log events"));
                }
            }
        }
    }

    {
        let mut response = response.lock().unwrap();
        *response = "second".to_string();
    }

    executor
        .wait_for_status(&worker_id, AgentStatus::Idle, Duration::from_secs(20))
        .await?;

    executor.check_oplog_is_queryable(&worker_id).await?;
    drop(executor);
    http_server.abort();

    assert!(!values1.is_empty());
    // first running
    assert!(values2.is_empty());
    // first interrupted
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("6m")]
async fn long_running_poll_loop_connection_breaks_on_interrupt(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("http_tests")] http_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let response = Arc::new(Mutex::new("initial".to_string()));
    let response_clone = response.clone();

    let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await.unwrap();

    let host_http_port = listener.local_addr().unwrap().port();

    let http_server = tokio::spawn(
        async move {
            let route = Router::new().route(
                "/poll",
                get(move || async move {
                    let body = response_clone.lock().unwrap();
                    body.clone()
                }),
            );

            axum::serve(listener, route).await.unwrap();
        }
        .in_current_span(),
    );

    let component = executor
        .component_dep(&context.default_environment_id, http_tests)
        .store()
        .await?;
    let agent_id = agent_id!("HttpClient2");
    let mut env = HashMap::new();
    env.insert("PORT".to_string(), host_http_port.to_string());
    let worker_id = executor
        .start_agent_with(
            &component.id,
            agent_id.clone(),
            env,
            HashMap::new(),
            Vec::new(),
        )
        .await?;

    let (mut rx, _abort_capture) = executor.capture_output_with_termination(&worker_id).await?;

    executor
        .invoke_agent(&component, &agent_id, "start_polling", data_value!("first"))
        .await?;

    executor
        .wait_for_status(&worker_id, AgentStatus::Running, Duration::from_secs(10))
        .await?;

    {
        let mut found1 = false;
        let mut found2 = false;
        while !(found1 && found2) {
            match rx.recv().await {
                Some(Some(event)) => {
                    if stdout_event_matching(&event, "Calling the poll endpoint\n") {
                        found1 = true;
                    } else if stdout_event_matching(&event, "Received initial\n") {
                        found2 = true;
                    }
                }
                _ => {
                    return Err(anyhow!("Did not receive the expected log events"));
                }
            }
        }
    }

    executor.interrupt(&worker_id).await?;

    drain_connection(rx).await;

    executor.check_oplog_is_queryable(&worker_id).await?;
    drop(executor);
    http_server.abort();
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("4m")]
async fn long_running_poll_loop_connection_retry_does_not_resume_interrupted_worker(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("http_tests")] http_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let response = Arc::new(Mutex::new("initial".to_string()));
    let response_clone = response.clone();

    let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await.unwrap();

    let host_http_port = listener.local_addr().unwrap().port();

    let http_server = tokio::spawn(
        async move {
            let route = Router::new().route(
                "/poll",
                get(move || async move {
                    let body = response_clone.lock().unwrap();
                    body.clone()
                }),
            );

            axum::serve(listener, route).await.unwrap();
        }
        .in_current_span(),
    );

    let component = executor
        .component_dep(&context.default_environment_id, http_tests)
        .store()
        .await?;
    let agent_id = agent_id!("HttpClient2");
    let mut env = HashMap::new();
    env.insert("PORT".to_string(), host_http_port.to_string());

    let worker_id = executor
        .start_agent_with(
            &component.id,
            agent_id.clone(),
            env,
            HashMap::new(),
            Vec::new(),
        )
        .await?;

    let (mut rx, _abort_capture) = executor.capture_output_with_termination(&worker_id).await?;

    executor
        .invoke_agent(&component, &agent_id, "start_polling", data_value!("first"))
        .await?;

    executor
        .wait_for_status(&worker_id, AgentStatus::Running, Duration::from_secs(30))
        .await?;

    // Wait for the poll loop to actually start before interrupting
    tokio::time::timeout(Duration::from_secs(30), async {
        let mut saw_call = false;
        let mut saw_initial = false;
        while !(saw_call && saw_initial) {
            match rx.recv().await {
                Some(Some(event)) => {
                    if stdout_event_matching(&event, "Calling the poll endpoint\n") {
                        saw_call = true;
                    } else if stdout_event_matching(&event, "Received initial\n") {
                        saw_initial = true;
                    }
                }
                _ => {
                    return Err(anyhow!("Did not receive expected poll-loop log events"));
                }
            }
        }
        Ok(())
    })
    .await
    .map_err(|_| anyhow!("Timed out waiting for poll loop to start"))??;

    executor.interrupt(&worker_id).await?;

    // Drain the log stream with a timeout to verify it terminates naturally after interrupt
    tokio::time::timeout(Duration::from_secs(30), drain_connection(rx))
        .await
        .map_err(|_| anyhow!("Timed out waiting for log stream termination after interrupt"))?;

    let status1 = executor.get_worker_metadata(&worker_id).await?;

    {
        let result = executor.capture_output_with_termination(&worker_id).await;
        assert!(result.is_err());
    };

    sleep(Duration::from_secs(2)).await;
    let status2 = executor.get_worker_metadata(&worker_id).await?;

    executor.check_oplog_is_queryable(&worker_id).await?;
    drop(executor);
    http_server.abort();

    assert_eq!(status1.status, AgentStatus::Interrupted);
    assert_eq!(status2.status, AgentStatus::Interrupted);

    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("4m")]
async fn long_running_poll_loop_connection_can_be_restored_after_resume(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("http_tests")] http_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let response = Arc::new(Mutex::new("initial".to_string()));
    let response_clone = response.clone();

    let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await.unwrap();

    let host_http_port = listener.local_addr().unwrap().port();

    let http_server = tokio::spawn(
        async move {
            let route = Router::new().route(
                "/poll",
                get(move || async move {
                    let body = response_clone.lock().unwrap();
                    body.clone()
                }),
            );

            axum::serve(listener, route).await.unwrap();
        }
        .in_current_span(),
    );

    let component = executor
        .component_dep(&context.default_environment_id, http_tests)
        .store()
        .await?;
    let agent_id = agent_id!("HttpClient2");
    let mut env = HashMap::new();
    env.insert("PORT".to_string(), host_http_port.to_string());

    let worker_id = executor
        .start_agent_with(
            &component.id,
            agent_id.clone(),
            env,
            HashMap::new(),
            Vec::new(),
        )
        .await?;

    let (mut rx, _abort_capture) = executor.capture_output_with_termination(&worker_id).await?;

    executor
        .invoke_agent(&component, &agent_id, "start_polling", data_value!("first"))
        .await?;

    executor
        .wait_for_status(&worker_id, AgentStatus::Running, Duration::from_secs(10))
        .await?;

    // AgentStatus::Running is set when the invocation is recorded, before the guest
    // reaches the poll loop. Wait for the first loop iteration so interrupting this
    // worker reliably exercises the interrupted-connection path instead of racing a
    // not-yet-started invocation.
    tokio::time::timeout(Duration::from_secs(30), async {
        let mut saw_call = false;
        let mut saw_initial = false;
        while !(saw_call && saw_initial) {
            match rx.recv().await {
                Some(Some(event)) => {
                    if stdout_event_matching(&event, "Calling the poll endpoint\n") {
                        saw_call = true;
                    } else if stdout_event_matching(&event, "Received initial\n") {
                        saw_initial = true;
                    }
                }
                _ => {
                    return Err(anyhow!("Did not receive expected poll-loop log events"));
                }
            }
        }
        Ok(())
    })
    .await
    .map_err(|_| anyhow!("Timed out waiting for poll loop to start"))??;

    executor.interrupt(&worker_id).await?;

    tokio::time::timeout(Duration::from_secs(30), drain_connection(rx))
        .await
        .map_err(|_| anyhow!("Timed out waiting for log stream termination after interrupt"))?;
    let status2 = executor.get_worker_metadata(&worker_id).await?;

    executor.resume(&worker_id, false).await?;
    executor
        .wait_for_status(&worker_id, AgentStatus::Running, Duration::from_secs(10))
        .await?;

    let (mut rx, _abort_capture) = executor.capture_output_with_termination(&worker_id).await?;

    // wait for one loop to finish
    {
        let mut found1 = false;
        let mut found2 = false;
        while !(found1 && found2) {
            match rx.recv().await {
                Some(Some(event)) => {
                    if stdout_event_matching(&event, "Calling the poll endpoint\n") {
                        found1 = true;
                    } else if stdout_event_matching(&event, "Received initial\n") {
                        found2 = true;
                    }
                }
                _ => {
                    return Err(anyhow!("Did not receive the expected log events"));
                }
            }
        }
    }

    // check we are getting the full last loop
    {
        let mut found1 = false;
        let mut found2 = false;
        let mut found3 = false;
        while !(found1 && found2 && found3) {
            match rx.recv().await {
                Some(Some(event)) => {
                    if stdout_event_matching(&event, "Calling the poll endpoint\n") {
                        found1 = true;
                    } else if stdout_event_matching(&event, "Received first\n") {
                        found2 = true;
                    } else if stdout_event_matching(&event, "Poll loop finished\n") {
                        found3 = true;
                    }

                    if found1 && !found2 {
                        // change the response. Next loop will be the last
                        {
                            let mut response = response.lock().unwrap();
                            *response = "first".to_string();
                        }
                    }
                }
                _ => {
                    return Err(anyhow!("Did not receive the expected log events"));
                }
            }
        }
    }

    executor
        .wait_for_status(&worker_id, AgentStatus::Idle, Duration::from_secs(5))
        .await?;

    let status4 = executor.get_worker_metadata(&worker_id).await?;

    executor.check_oplog_is_queryable(&worker_id).await?;
    drop(executor);
    http_server.abort();

    assert_eq!(status2.status, AgentStatus::Interrupted);
    assert_eq!(status4.status, AgentStatus::Idle);
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("4m")]
async fn long_running_poll_loop_worker_can_be_deleted_after_interrupt(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("http_tests")] http_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let response = Arc::new(Mutex::new("initial".to_string()));
    let response_clone = response.clone();

    let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await.unwrap();

    let host_http_port = listener.local_addr().unwrap().port();

    let http_server = tokio::spawn(
        async move {
            let route = Router::new().route(
                "/poll",
                get(move || async move {
                    let body = response_clone.lock().unwrap();
                    body.clone()
                }),
            );

            axum::serve(listener, route).await.unwrap();
        }
        .in_current_span(),
    );

    let component = executor
        .component_dep(&context.default_environment_id, http_tests)
        .store()
        .await?;
    let agent_id = agent_id!("HttpClient2");
    let mut env = HashMap::new();
    env.insert("PORT".to_string(), host_http_port.to_string());

    let worker_id = executor
        .start_agent_with(
            &component.id,
            agent_id.clone(),
            env,
            HashMap::new(),
            Vec::new(),
        )
        .await?;

    let (rx, _abort_capture) = executor.capture_output_with_termination(&worker_id).await?;

    executor
        .invoke_agent(&component, &agent_id, "start_polling", data_value!("first"))
        .await?;

    executor
        .wait_for_status(&worker_id, AgentStatus::Running, Duration::from_secs(10))
        .await?;

    executor.interrupt(&worker_id).await?;

    drain_connection(rx).await;

    executor.check_oplog_is_queryable(&worker_id).await?;
    executor.delete_worker(&worker_id).await?;
    let metadata = executor.get_worker_metadata(&worker_id).await;

    drop(executor);
    http_server.abort();

    assert!(metadata.is_err());
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("4m")]
async fn counter_resource_test_1(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] agent_rpc_rust: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component_dep(&context.default_environment_id, agent_rpc_rust)
        .store()
        .await?;

    let agent_id = agent_id!("RpcCounter", "counter1");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    executor
        .invoke_and_await_agent(&component, &agent_id, "inc_by", data_value!(5u64))
        .await?;

    let result = executor
        .invoke_and_await_agent(&component, &agent_id, "get_value", data_value!())
        .await?;

    let result_value = result
        .into_return_value()
        .expect("Expected a single return value");

    executor.check_oplog_is_queryable(&worker_id).await?;

    assert_eq!(result_value, Value::U64(5));
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("4m")]
async fn reconstruct_interrupted_state(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start_with_redis_storage(deps, &context).await?;

    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;
    let agent_id = agent_id!("Clocks", "interruption-2");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    // Warmup: ensure the agent constructor has completed before we invoke the
    // long-running function we intend to interrupt.
    executor
        .invoke_and_await_agent(&component, &agent_id, "sleep_for", data_value!(0.0f64))
        .await?;

    let executor_clone = executor.clone();
    let component_clone = component.clone();
    let agent_id_clone = agent_id.clone();
    let fiber = tokio::spawn(
        async move {
            executor_clone
                .invoke_and_await_agent(
                    &component_clone,
                    &agent_id_clone,
                    "interruption",
                    data_value!(),
                )
                .await
        }
        .in_current_span(),
    );

    executor
        .wait_for_status(&worker_id, AgentStatus::Running, Duration::from_secs(10))
        .await?;

    executor.interrupt(&worker_id).await?;
    let result = fiber.await?;

    // Explicitly deleting the status information from Redis to check if it can be
    // reconstructed from Redis

    let mut redis = executor.redis().get_connection(0);
    let _: () = redis
        .del(format!(
            "{}instance:status:{}",
            context.redis_prefix(),
            worker_id.to_redis_key()
        ))
        .unwrap();
    debug!("Deleted status information from Redis");

    let status = executor.get_worker_metadata(&worker_id).await?.status;

    assert!(result.is_err());
    let err_msg = format!("{}", result.err().unwrap());
    assert!(err_msg.contains("Interrupted via the Golem API"));
    assert_eq!(status, AgentStatus::Interrupted);

    executor.check_oplog_is_queryable(&worker_id).await?;
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("6m")]
async fn invocation_queue_is_persistent(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("http_tests")] http_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let response = Arc::new(Mutex::new("initial".to_string()));
    let response_clone = response.clone();

    let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await.unwrap();

    let host_http_port = listener.local_addr().unwrap().port();

    let http_server = tokio::spawn(
        async move {
            let route = Router::new().route(
                "/poll",
                get(move || async move {
                    let body = response_clone.lock().unwrap();
                    body.clone()
                }),
            );

            axum::serve(listener, route).await.unwrap();
        }
        .in_current_span(),
    );

    let component = executor
        .component_dep(&context.default_environment_id, http_tests)
        .store()
        .await?;
    let agent_id = agent_id!("HttpClient2");
    let mut env = HashMap::new();
    env.insert("PORT".to_string(), host_http_port.to_string());

    let worker_id = executor
        .start_agent_with(
            &component.id,
            agent_id.clone(),
            env,
            HashMap::new(),
            Vec::new(),
        )
        .await?;

    executor.log_output(&worker_id).await?;

    executor
        .invoke_agent(&component, &agent_id, "start_polling", data_value!("done"))
        .await?;

    executor
        .wait_for_status(&worker_id, AgentStatus::Running, Duration::from_secs(10))
        .await?;

    executor
        .invoke_agent(&component, &agent_id, "increment", data_value!())
        .await?;

    executor
        .invoke_agent(&component, &agent_id, "increment", data_value!())
        .await?;

    executor
        .invoke_agent(&component, &agent_id, "increment", data_value!())
        .await?;

    executor.interrupt(&worker_id).await?;

    executor
        .wait_for_status(&worker_id, AgentStatus::Interrupted, Duration::from_secs(5))
        .await?;

    executor.check_oplog_is_queryable(&worker_id).await?;

    drop(executor);
    let executor = start(deps, &context).await?;

    executor
        .invoke_agent(&component, &agent_id, "increment", data_value!())
        .await?;

    // executor.log_output(&worker_id).await?;

    executor
        .wait_for_status(&worker_id, AgentStatus::Running, Duration::from_secs(10))
        .await?;
    {
        let mut response = response.lock().unwrap();
        *response = "done".to_string();
    }

    let result = executor
        .invoke_and_await_agent(&component, &agent_id, "get_count", data_value!())
        .await?;

    http_server.abort();

    assert_eq!(result, data_value!(4u64));

    executor.check_oplog_is_queryable(&worker_id).await?;
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("4m")]
async fn invoke_with_non_existing_function(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_counters")] agent_counters: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component_dep(&context.default_environment_id, agent_counters)
        .store()
        .await?;

    let counter_id = agent_id!("Counter", "invoke-with-non-existing-function");
    let worker_id = executor
        .start_agent(&component.id, counter_id.clone())
        .await?;

    // First we invoke a function that does not exist and expect a failure
    let failure = executor
        .invoke_and_await_agent(&component, &counter_id, "WRONG", data_value!())
        .await;

    // Then we invoke an existing function, to prove the worker should not be in failed state
    let success = executor
        .invoke_and_await_agent(&component, &counter_id, "increment", data_value!())
        .await?;

    assert!(failure.is_err());
    assert_eq!(success.into_return_value(), Some(Value::U32(1)));

    executor.check_oplog_is_queryable(&worker_id).await?;
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("4m")]
#[ignore] // TODO: FIX DURING THE FIRST CLASS AGENT SUPPORT EPIC
async fn invoke_with_wrong_parameters(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_counters")] agent_counters: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component_dep(&context.default_environment_id, agent_counters)
        .store()
        .await?;

    let counter_id = agent_id!("Counter", "invoke-with-wrong-parameters");
    let worker_id = executor
        .start_agent(&component.id, counter_id.clone())
        .await?;

    // First we invoke an existing function with wrong parameters
    let failure = executor
        .invoke_and_await_agent(
            &component,
            &counter_id,
            "increment",
            data_value!("unexpected"),
        )
        .await;

    // Then we invoke an existing function to prove the worker should not be in failed state
    let success = executor
        .invoke_and_await_agent(&component, &counter_id, "increment", data_value!())
        .await?;

    assert!(failure.is_err());
    assert_eq!(success.into_return_value(), Some(Value::U32(1)));

    executor.check_oplog_is_queryable(&worker_id).await?;
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("4m")]
async fn stderr_returned_for_failed_component(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_counters")] agent_counters: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component_dep(&context.default_environment_id, agent_counters)
        .store()
        .await?;
    let agent_id = agent_id!("FailingCounter", "failing-worker-1");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    executor
        .invoke_and_await_agent(&component, &agent_id, "add", data_value!(5u64))
        .await?;

    let result2 = executor
        .invoke_and_await_agent(&component, &agent_id, "add", data_value!(50u64))
        .await;

    executor.check_oplog_is_queryable(&worker_id).await?;

    let result3 = executor
        .invoke_and_await_agent(&component, &agent_id, "get", data_value!())
        .await;

    let metadata = executor.get_worker_metadata(&worker_id).await?;

    let (next, all) = executor
        .get_workers_metadata(&component.id, None, ScanCursor::default(), 100, true)
        .await?;

    assert!(result2.is_err());
    assert!(result3.is_err());

    let err2 = format!("{}", result2.err().unwrap());
    assert!(
        err2.contains("error log message"),
        "Expected 'error log message' in error: {err2}"
    );
    assert!(
        err2.contains("value is too large"),
        "Expected 'value is too large' in error: {err2}"
    );

    let err3 = format!("{}", result3.err().unwrap());
    assert!(
        err3.contains("error log message"),
        "Expected 'error log message' in error: {err3}"
    );
    assert!(
        err3.contains("value is too large"),
        "Expected 'value is too large' in error: {err3}"
    );

    assert_eq!(metadata.status, AgentStatus::Failed);
    assert!(metadata.last_error.is_some());
    let last_error = metadata.last_error.unwrap();
    assert!(
        last_error.contains("error log message"),
        "Expected 'error log message' in last_error: {last_error}"
    );
    assert!(
        last_error.contains("value is too large"),
        "Expected 'value is too large' in last_error: {last_error}"
    );

    assert!(next.is_none());
    assert_eq!(all.len(), 1);
    assert!(all[0].last_error.is_some());
    let all_last_error = all[0].last_error.clone().unwrap();
    assert!(
        all_last_error.contains("error log message"),
        "Expected 'error log message' in all[0].last_error: {all_last_error}"
    );
    assert!(
        all_last_error.contains("value is too large"),
        "Expected 'value is too large' in all[0].last_error: {all_last_error}"
    );

    executor.check_oplog_is_queryable(&worker_id).await?;
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("4m")]
async fn cancelling_pending_invocations(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] agent_rpc_rust: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component_dep(&context.default_environment_id, agent_rpc_rust)
        .store()
        .await?;

    let agent_id = agent_id!("RpcBlockingCounter", "cancel-pending");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    let ik1 = IdempotencyKey::fresh();
    let ik2 = IdempotencyKey::fresh();
    let ik3 = IdempotencyKey::fresh();
    let ik4 = IdempotencyKey::fresh();

    // inc_by(5) with ik1 - completes immediately
    executor
        .invoke_and_await_agent_with_key(&component, &agent_id, &ik1, "inc_by", data_value!(5u64))
        .await?;

    // create_promise - returns a PromiseId
    let promise_id_value = executor
        .invoke_and_await_agent(&component, &agent_id, "create_promise", data_value!())
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected return value"))?;

    let promise_id_vat = ValueAndType::new(promise_id_value.clone(), PromiseId::get_type());

    // await_promise (fire-and-forget) - worker suspends
    executor
        .invoke_agent(
            &component,
            &agent_id,
            "await_promise",
            DataValue::Tuple(ElementValues {
                elements: vec![ElementValue::ComponentModel(ComponentModelElementValue {
                    value: promise_id_vat,
                })],
            }),
        )
        .await?;

    // Queue inc_by(6) with ik2 and inc_by(7) with ik3 while suspended
    executor
        .invoke_agent_with_key(&component, &agent_id, &ik2, "inc_by", data_value!(6u64))
        .await?;

    executor
        .invoke_agent_with_key(&component, &agent_id, &ik3, "inc_by", data_value!(7u64))
        .await?;

    let cancel1 = executor.cancel_invocation(&worker_id, &ik1).await;
    let cancel2 = executor.cancel_invocation(&worker_id, &ik2).await;
    let cancel4 = executor.cancel_invocation(&worker_id, &ik4).await;

    // Extract oplog_idx from promise value to complete it
    let Value::Record(fields) = &promise_id_value else {
        panic!("Expected a record for PromiseId")
    };
    let Value::U64(oplog_idx) = fields[1] else {
        panic!("Expected a u64 for oplog_idx")
    };

    executor
        .complete_promise(
            &PromiseId {
                agent_id: worker_id.clone(),
                oplog_idx: OplogIndex::from_u64(oplog_idx),
            },
            vec![42],
        )
        .await?;

    // get_value should be 5 + 7 = 12 (ik1 already done, ik2 cancelled, ik3 goes through)
    let final_result = executor
        .invoke_and_await_agent(&component, &agent_id, "get_value", data_value!())
        .await?;

    executor.check_oplog_is_queryable(&worker_id).await?;

    let final_value = final_result
        .into_return_value()
        .expect("Expected a single return value");

    assert!(cancel1.is_ok() && !cancel1.unwrap()); // cannot cancel a completed invocation
    assert!(cancel2.is_ok() && cancel2.unwrap());
    assert!(cancel4.is_err()); // cannot cancel a non-existing invocation
    assert_eq!(final_value, Value::U64(12));
    Ok(())
}

/// Test resolving a component_id from the name.
#[test]
#[tracing::instrument]
#[timeout("4m")]
async fn resolve_components_from_name(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    #[tagged_as("agent_rpc_rust_as_resolve_target")]
    agent_rpc_rust_as_resolve_target: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    // Make sure the name is unique
    let counter_component = executor
        .component_dep(
            &context.default_environment_id,
            agent_rpc_rust_as_resolve_target,
        )
        .store()
        .await?;

    let resolver_component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;

    let target_agent_id = agent_id!("RpcCounter", "counter-1");
    executor
        .start_agent(&counter_component.id, target_agent_id)
        .await?;

    let agent_id = agent_id!("GolemHostApi", "resolver-1");
    let resolve_worker = executor
        .start_agent(&resolver_component.id, agent_id.clone())
        .await?;

    let result = executor
        .invoke_and_await_agent(
            &resolver_component,
            &agent_id,
            "resolve_component",
            data_value!(),
        )
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected return value"))?;

    assert_eq!(
        result,
        Value::Record(vec![
            Value::Bool(true),
            Value::Bool(true),
            Value::Bool(false),
        ])
    );

    executor.check_oplog_is_queryable(&resolve_worker).await?;
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("4m")]
async fn scheduled_invocation(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("agent_rpc_rust")] agent_rpc_rust: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component_dep(&context.default_environment_id, agent_rpc_rust)
        .store()
        .await?;

    let server_agent_name = format!("scheduled-server-{}", context.redis_prefix());
    let client_agent_name = format!("scheduled-client-{}", context.redis_prefix());

    let server_agent_id = agent_id!("ScheduledInvocationServer", server_agent_name.clone());
    let client_agent_id = agent_id!("ScheduledInvocationClient", client_agent_name.clone());

    let server_worker = executor
        .start_agent(&component.id, server_agent_id.clone())
        .await?;
    let client_worker = executor
        .start_agent(&component.id, client_agent_id.clone())
        .await?;

    // first invocation: schedule increment in the future and poll
    {
        executor
            .invoke_and_await_agent(
                &component,
                &client_agent_id,
                "test1",
                data_value!(server_agent_name.clone()),
            )
            .await?;

        let mut done = false;
        while !done {
            let result = executor
                .invoke_and_await_agent(
                    &component,
                    &server_agent_id,
                    "get_global_value",
                    data_value!(),
                )
                .await?;

            if result.into_return_value() == Some(Value::U64(1)) {
                done = true;
            } else {
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }
    }

    // second invocation: schedule increment in the future and cancel beforehand
    {
        executor
            .invoke_and_await_agent(
                &component,
                &client_agent_id,
                "test2",
                data_value!(server_agent_name.clone()),
            )
            .await?;

        tokio::time::sleep(Duration::from_millis(300)).await;

        let result = executor
            .invoke_and_await_agent(
                &component,
                &server_agent_id,
                "get_global_value",
                data_value!(),
            )
            .await?;

        assert_eq!(result.into_return_value(), Some(Value::U64(1)));
    }

    // third invocation: schedule increment on self in the future and poll
    {
        executor
            .invoke_and_await_agent(&component, &client_agent_id, "test3", data_value!())
            .await?;

        let mut done = false;
        while !done {
            let result = executor
                .invoke_and_await_agent(
                    &component,
                    &client_agent_id,
                    "get_global_value",
                    data_value!(),
                )
                .await?;

            if result.into_return_value() == Some(Value::U64(1)) {
                done = true;
            } else {
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }
    }

    executor.check_oplog_is_queryable(&client_worker).await?;
    executor.check_oplog_is_queryable(&server_worker).await?;
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("4m")]
async fn error_handling_when_worker_is_invoked_with_wrong_parameter_type(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_counters")] agent_counters: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component_dep(&context.default_environment_id, agent_counters)
        .store()
        .await?;

    let agent_id = agent_id!("FailingCounter", "wrong-parameter-type-1");
    let _worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    let failure = executor
        .invoke_and_await_agent(&component, &agent_id, "add", data_value!("not-a-number"))
        .await;

    let success = executor
        .invoke_and_await_agent(&component, &agent_id, "add", data_value!(5u64))
        .await;

    executor.check_oplog_is_queryable(&_worker_id).await?;
    drop(executor);

    assert!(failure.is_err());
    assert!(success.is_ok());
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("4m")]
async fn delete_worker_during_invocation(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;

    let agent_id = agent_id!("Clock", "delete-worker-during-invocation");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    info!("Enqueuing invocations");
    // Enqueuing a large number of invocations, each sleeping for 2 seconds
    for _ in 0..25 {
        executor
            .invoke_agent(&component, &agent_id, "sleep", data_value!(2u64))
            .await?;
    }

    executor
        .wait_for_status(&worker_id, AgentStatus::Running, Duration::from_secs(2))
        .await?;

    info!("Deleting the worker");
    executor.delete_worker(&worker_id).await?;

    info!("Invoking again");
    // Invoke it one more time - it should create a new instance and return successfully
    let result = executor
        .invoke_and_await_agent(&component, &agent_id, "sleep", data_value!(1u64))
        .await?;

    let metadata = executor.get_worker_metadata(&worker_id).await?;

    executor.check_oplog_is_queryable(&worker_id).await?;

    let result_value = result.into_return_value();
    assert_eq!(result_value, Some(Value::Result(Ok(None))));
    assert_eq!(metadata.pending_invocation_count, 0);
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("120s")]
async fn invoking_worker_while_its_getting_deleted_works(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] agent_rpc_rust: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component_dep(&context.default_environment_id, agent_rpc_rust)
        .store()
        .await?;

    let agent_id = agent_id!("RpcGlobalState", "worker");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    // Spawns a task that repeatedly invokes inc_global_by(1) and checks the counter.
    // Exits when either:
    // - the counter resets (worker was deleted and recreated), or
    // - an invocation fails (caught the deletion in-flight)
    let invoking_task = {
        let executor = executor.clone();
        let component_clone = component.clone();
        let agent_id = agent_id.clone();
        tokio::spawn(async move {
            let mut expected_counter: u64 = 0;
            let mut iteration: u64 = 0;
            loop {
                iteration += 1;
                match executor
                    .invoke_and_await_agent(
                        &component_clone,
                        &agent_id,
                        "inc_global_by",
                        data_value!(1u64),
                    )
                    .await
                {
                    Ok(_) => {
                        expected_counter += 1;
                    }
                    Err(error) => {
                        break Err(error);
                    }
                }

                match executor
                    .invoke_and_await_agent(
                        &component_clone,
                        &agent_id,
                        "get_global_value",
                        data_value!(),
                    )
                    .await
                {
                    Ok(result) => {
                        let value = result.into_return_value();
                        match value {
                            Some(Value::U64(v)) => {
                                if v < expected_counter {
                                    break Ok(());
                                }
                                expected_counter = v;
                            }
                            other => {
                                tracing::warn!(iteration, expected_counter, value = ?other, "Unexpected value while checking global counter")
                            }
                        }
                    }
                    Err(error) => {
                        break Err(error);
                    }
                }
            }
        })
    };

    tokio::time::sleep(Duration::from_millis(100)).await;
    let deleting_task_cancel_token = CancellationToken::new();
    {
        let executor = executor.clone();
        let worker_id = worker_id.clone();
        let deleting_task_cancel_token = deleting_task_cancel_token.clone();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = deleting_task_cancel_token.cancelled() => {
                        break
                    },
                    result = executor.delete_worker(&worker_id) => {
                        let _ = result;
                    }
                }
            }
        })
    };

    let _invocation_result = invoking_task.await?;
    deleting_task_cancel_token.cancel();

    // Either the counter reset was detected (Ok) or the invocation failed (Err).
    // Both are valid outcomes of invoking while deleting.
    Ok(())
}

/// Same environment, different account: when account A lazily creates a worker
/// for a component owned by account B (e.g. via cross-account RPC), the newly
/// created worker's `created_by` must be account B (the component owner), not
/// account A (the caller).
///
/// This simulates the `create_worker` gRPC path where `auth_ctx` carries the
/// caller's account (A) but the component is owned by a different account (B).
/// The `get_or_create_worker_metadata` code must use the component's
/// `account_id` for `created_by`, not the caller-provided account.
#[test]
#[tracing::instrument]
async fn worker_created_by_reflects_component_owner_not_caller(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    use golem_api_grpc::proto::golem::workerexecutor::v1::{
        CreateWorkerRequest, create_worker_response,
    };
    use golem_service_base::model::auth::{AuthCtx, UserAuthCtx};

    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    // Store the component in the default environment.
    // The component is owned by context.account_id (= account B, the owner).
    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;

    let component_owner_account_id = component.account_id;

    // This is account A — the caller who is lazily creating a worker for
    // account B's component. It must differ from the component owner.
    let caller_account_id = AccountId::new();
    assert_ne!(caller_account_id, component_owner_account_id);

    // Build an auth_ctx that carries the caller's account (A), simulating
    // what happens during cross-account RPC.
    let caller_auth_ctx: golem_api_grpc::proto::golem::auth::AuthCtx = AuthCtx::User(UserAuthCtx {
        account_id: caller_account_id,
        account_plan_id: context.account_plan_id,
        account_roles: context.account_roles.clone(),
    })
    .into();

    let agent_id = AgentId {
        component_id: component.id,
        agent_id: agent_id!("Clock", "cross-account-attribution-test").to_string(),
    };

    let response = executor
        .client
        .clone()
        .create_worker(CreateWorkerRequest {
            agent_id: Some(agent_id.clone().into()),
            component_owner_account_id: Some(component_owner_account_id.into()),
            environment_id: Some(component.environment_id.into()),
            env: Default::default(),
            wasi_config: Default::default(),
            config: Vec::new(),
            ignore_already_existing: false,
            auth_ctx: Some(caller_auth_ctx),
            principal: None,
            invocation_context: None,
        })
        .await?
        .into_inner();

    match response.result {
        Some(create_worker_response::Result::Success(_)) => {}
        Some(create_worker_response::Result::Failure(error)) => {
            return Err(anyhow!("create_worker failed: {error:?}"));
        }
        None => return Err(anyhow!("No response from create_worker")),
    }

    // Retrieve the worker metadata and verify created_by is the component
    // owner (account B), not the caller (account A).
    let metadata = executor.get_worker_metadata(&agent_id).await?;

    drop(executor);

    assert_eq!(
        metadata.created_by, component_owner_account_id,
        "Worker created_by should be the component owner's account (B), not the caller's (A)"
    );

    Ok(())
}

/// Different environment, different account: when a caller in environment A
/// creates a worker for a component owned by account B in environment B,
/// the worker must be stored in environment B's namespace and be retrievable
/// using the component's environment_id. Both `created_by` and `environment_id`
/// in the worker metadata must reflect the component owner, not the caller.
#[test]
#[tracing::instrument]
async fn worker_environment_reflects_component_not_caller(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    use golem_api_grpc::proto::golem::workerexecutor::v1::{
        CreateWorkerRequest, create_worker_response,
    };
    use golem_common::model::environment::EnvironmentId;
    use golem_service_base::model::auth::{AuthCtx, UserAuthCtx};

    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    // Store the component in the default environment (= environment B).
    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;

    let component_owner_account_id = component.account_id;
    let component_environment_id = component.environment_id;

    // These are the caller's values (environment A, account A).
    let caller_account_id = AccountId::new();
    let caller_environment_id = EnvironmentId::new();
    assert_ne!(caller_account_id, component_owner_account_id);
    assert_ne!(caller_environment_id, component_environment_id);

    let caller_auth_ctx: golem_api_grpc::proto::golem::auth::AuthCtx = AuthCtx::User(UserAuthCtx {
        account_id: caller_account_id,
        account_plan_id: context.account_plan_id,
        account_roles: context.account_roles.clone(),
    })
    .into();

    let agent_id = AgentId {
        component_id: component.id,
        agent_id: agent_id!("Clock", "cross-env-attribution-test").to_string(),
    };

    // Create the worker with the CALLER's environment_id and account.
    // This simulates what construct_wasm_rpc_resource does: it builds an
    // OwnedAgentId using the caller's environment_id rather than the target
    // component's.
    let response = executor
        .client
        .clone()
        .create_worker(CreateWorkerRequest {
            agent_id: Some(agent_id.clone().into()),
            component_owner_account_id: Some(caller_account_id.into()),
            environment_id: Some(caller_environment_id.into()),
            env: Default::default(),
            wasi_config: Default::default(),
            config: Vec::new(),
            ignore_already_existing: false,
            auth_ctx: Some(caller_auth_ctx),
            principal: None,
            invocation_context: None,
        })
        .await?
        .into_inner();

    match response.result {
        Some(create_worker_response::Result::Success(_)) => {}
        Some(create_worker_response::Result::Failure(error)) => {
            return Err(anyhow!("create_worker failed: {error:?}"));
        }
        None => return Err(anyhow!("No response from create_worker")),
    }

    // The worker must be findable using the component's environment_id, not
    // the caller's. get_worker_metadata looks up using the component's
    // environment, so if the worker was stored under the wrong namespace this
    // call will fail with "Worker not found".
    let metadata = executor.get_worker_metadata(&agent_id).await?;

    drop(executor);

    assert_eq!(
        metadata.environment_id, component_environment_id,
        "Worker environment_id should be the component's environment (B), not the caller's (A)"
    );
    assert_eq!(
        metadata.created_by, component_owner_account_id,
        "Worker created_by should be the component owner's account (B), not the caller's (A)"
    );

    Ok(())
}

/// When a worker is created with a caller account that differs from the
/// component owner, the resource limits (fuel, storage quotas) must be
/// initialized for the **component owner's** account, not the caller's.
/// Otherwise, all resource consumption is attributed to the wrong account.
#[test]
#[tracing::instrument]
#[timeout("30s")]
async fn resource_limits_initialized_for_component_owner_not_caller(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    use async_trait::async_trait;
    use golem_api_grpc::proto::golem::workerexecutor::v1::{
        CreateWorkerRequest, create_worker_response,
    };
    use golem_service_base::model::auth::{AuthCtx, UserAuthCtx};
    use golem_worker_executor::services::resource_limits::{AtomicResourceEntry, ResourceLimits};
    use golem_worker_executor_test_utils::start_with_resource_limits;
    use std::collections::HashSet;
    use std::sync::Mutex;

    /// Records every `account_id` passed to `initialize_account`, while
    /// returning unlimited resources so the worker can run normally.
    struct RecordingResourceLimits {
        initialized_accounts: Mutex<Vec<AccountId>>,
    }

    impl RecordingResourceLimits {
        fn new() -> Self {
            Self {
                initialized_accounts: Mutex::new(Vec::new()),
            }
        }

        fn initialized_accounts(&self) -> Vec<AccountId> {
            self.initialized_accounts.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl ResourceLimits for RecordingResourceLimits {
        async fn initialize_account(
            &self,
            account_id: AccountId,
        ) -> Result<
            Arc<AtomicResourceEntry>,
            golem_service_base::error::worker_executor::WorkerExecutorError,
        > {
            self.initialized_accounts.lock().unwrap().push(account_id);
            Ok(Arc::new(AtomicResourceEntry::new(
                u64::MAX,
                usize::MAX,
                usize::MAX,
                u64::MAX,
                u64::MAX,
            )))
        }
    }

    let context = TestContext::new(last_unique_id);
    let recording = Arc::new(RecordingResourceLimits::new());
    let executor = start_with_resource_limits(deps, &context, recording.clone()).await?;

    // Store the component — it is owned by context.account_id (= account B).
    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;

    let component_owner_account_id = component.account_id;

    // This is account A — the "caller" who creates the worker on behalf of
    // account B's component.
    let caller_account_id = AccountId::new();
    assert_ne!(caller_account_id, component_owner_account_id);

    let caller_auth_ctx: golem_api_grpc::proto::golem::auth::AuthCtx = AuthCtx::User(UserAuthCtx {
        account_id: caller_account_id,
        account_plan_id: context.account_plan_id,
        account_roles: context.account_roles.clone(),
    })
    .into();

    let agent_id = AgentId {
        component_id: component.id,
        agent_id: agent_id!("Clock", "resource-limits-attribution-test").to_string(),
    };

    // Create the worker with the CALLER's auth_ctx (account A).
    let response = executor
        .client
        .clone()
        .create_worker(CreateWorkerRequest {
            agent_id: Some(agent_id.clone().into()),
            component_owner_account_id: Some(component_owner_account_id.into()),
            environment_id: Some(component.environment_id.into()),
            env: Default::default(),
            wasi_config: Default::default(),
            config: Vec::new(),
            ignore_already_existing: false,
            auth_ctx: Some(caller_auth_ctx),
            principal: None,
            invocation_context: None,
        })
        .await?
        .into_inner();

    match response.result {
        Some(create_worker_response::Result::Success(_)) => {}
        Some(create_worker_response::Result::Failure(error)) => {
            return Err(anyhow!("create_worker failed: {error:?}"));
        }
        None => return Err(anyhow!("No response from create_worker")),
    }

    // The resource limits service must have been initialized for the
    // component owner (account B), not the caller (account A).
    let initialized: HashSet<AccountId> = recording.initialized_accounts().into_iter().collect();

    drop(executor);

    assert_eq!(
        initialized,
        HashSet::from([component_owner_account_id]),
        "Resource limits must be initialized only for the component owner (B), \
         not the caller (A = {caller_account_id})"
    );

    Ok(())
}
