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

use crate::compatibility::worker_recovery::save_recovery_golden_file;
use crate::Tracing;
use anyhow::anyhow;
use assert2::{check, let_assert};
use axum::routing::get;
use axum::Router;
use golem_common::model::component::{ComponentId, ComponentRevision};
use golem_common::model::component_metadata::{
    DynamicLinkedInstance, DynamicLinkedWasmRpc, WasmRpcTarget,
};
use golem_common::model::oplog::{OplogIndex, WorkerResourceId};
use golem_common::model::worker::{ExportedResourceMetadata, WorkerMetadataDto};
use golem_common::model::{
    FilterComparator, IdempotencyKey, PromiseId, RetryConfig, ScanCursor, StringFilterComparator,
    Timestamp, WorkerFilter, WorkerId, WorkerResourceDescription, WorkerStatus,
};
use golem_service_base::error::worker_executor::WorkerExecutorError;
use golem_test_framework::dsl::TestDsl;
use golem_test_framework::dsl::{
    drain_connection, stdout_event_matching, stdout_events, worker_error_logs, worker_error_message,
};
use golem_wasm::analysis::wit_parser::{SharedAnalysedTypeResolve, TypeName, TypeOwner};
use golem_wasm::analysis::{
    analysed_type, AnalysedResourceId, AnalysedResourceMode, AnalysedType, TypeHandle, TypeStr,
};
use golem_wasm::{IntoValue, Record};
use golem_wasm::{IntoValueAndType, Value, ValueAndType};
use golem_worker_executor_test_utils::{
    start, start_customized, LastUniqueId, TestContext, TestWorkerExecutor,
    WorkerExecutorTestDependencies,
};
use pretty_assertions::assert_eq;
use redis::Commands;
use std::collections::HashMap;
use std::env;
use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use system_interface::fs::FileIoExt;
use test_r::core::{DynamicTestRegistration, TestProperties};
use test_r::{add_test, inherit_test_dep, test, test_gen, timeout};
use tokio::task::JoinHandle;
use tokio::time::sleep;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, Instrument, Span};

inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(LastUniqueId);
inherit_test_dep!(Tracing);
inherit_test_dep!(
    #[tagged_as("golem_host")]
    SharedAnalysedTypeResolve
);

#[test]
#[tracing::instrument]
#[timeout(120_000)]
async fn interruption(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component(&context.default_environment_id, "interruption")
        .store()
        .await?;
    let worker_id = executor
        .start_worker(&component.id, "interruption-1")
        .await?;

    let executor_clone = executor.clone();
    let worker_id_clone = worker_id.clone();
    let fiber = tokio::spawn(
        async move {
            executor_clone
                .invoke_and_await(&worker_id_clone, "run", vec![])
                .await
        }
        .in_current_span(),
    );

    executor
        .wait_for_status(&worker_id, WorkerStatus::Running, Duration::from_secs(10))
        .await?;

    executor.interrupt(&worker_id).await?;
    let result = fiber.await??;

    drop(executor);

    check!(result.is_err());
    check!(worker_error_message(&result.err().unwrap()).contains("Interrupted via the Golem API"));
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("8m")]
async fn delete_interrupts_long_rpc_call(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component(&context.default_environment_id, "golem_it_agent_rpc")
        .name("golem-it:agent-rpc")
        .store()
        .await?;
    let unique_id = context.redis_prefix();
    let worker_id = executor
        .start_worker(&component.id, &format!("test-agent(\"{unique_id}\")"))
        .await?;

    executor
        .invoke(
            &worker_id,
            "golem-it:agent-rpc/test-agent.{long-rpc-call}",
            vec![600000f64.into_value_and_type()], // 10 minutes
        )
        .await??;

    tokio::time::sleep(Duration::from_secs(10)).await;
    executor.delete_worker(&worker_id).await?;

    let metadata = executor.get_worker_metadata(&worker_id).await;
    check!(metadata.is_err());

    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout(120_000)]
async fn simulated_crash(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component(&context.default_environment_id, "interruption")
        .store()
        .await?;
    let worker_id = executor
        .start_worker(&component.id, "simulated-crash-1")
        .await?;

    let mut rx = executor.capture_output(&worker_id).await?;

    let executor_clone = executor.clone();
    let worker_id_clone = worker_id.clone();
    let fiber = tokio::spawn(
        async move {
            executor_clone
                .invoke_and_await(&worker_id_clone, "run", vec![])
                .await
        }
        .in_current_span(),
    );

    tokio::time::sleep(Duration::from_secs(5)).await;

    let _ = executor.simulated_crash(&worker_id).await;
    let result = fiber.await??;

    let mut events = vec![];
    rx.recv_many(&mut events, 100).await;
    drop(executor);

    check!(result.is_ok());
    check!(result == Ok(vec![Value::String("done".to_string())]));
    check!(stdout_events(events.into_iter()) == vec!["Starting interruption test\n"]);
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout(120_000)]
async fn shopping_cart_example(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component(&context.default_environment_id, "shopping-cart")
        .store()
        .await?;
    let worker_id = executor
        .start_worker(&component.id, "shopping-cart-1")
        .await?;

    let _ = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{initialize-cart}",
            vec![ValueAndType {
                value: Value::String("test-user-1".to_string()),
                typ: analysed_type::str(),
            }],
        )
        .await?;

    let _ = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{add-item}",
            vec![Record(vec![
                ("product-id", "G1000".into_value_and_type()),
                ("name", "Golem T-Shirt M".into_value_and_type()),
                ("price", 100.0f32.into_value_and_type()),
                ("quantity", 5u32.into_value_and_type()),
            ])
            .into_value_and_type()],
        )
        .await?;

    let _ = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{add-item}",
            vec![Record(vec![
                ("product-id", "G1001".into_value_and_type()),
                ("name", "Golem Cloud Subscription 1y".into_value_and_type()),
                ("price", 999999.0f32.into_value_and_type()),
                ("quantity", 1u32.into_value_and_type()),
            ])
            .into_value_and_type()],
        )
        .await?;

    let _ = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{add-item}",
            vec![Record(vec![
                ("product-id", "G1002".into_value_and_type()),
                ("name", "Mud Golem".into_value_and_type()),
                ("price", 11.0f32.into_value_and_type()),
                ("quantity", 10u32.into_value_and_type()),
            ])
            .into_value_and_type()],
        )
        .await?;

    let _ = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{update-item-quantity}",
            vec!["G1002".into_value_and_type(), 20u32.into_value_and_type()],
        )
        .await?;

    let contents = executor
        .invoke_and_await(&worker_id, "golem:it/api.{get-cart-contents}", vec![])
        .await?;

    let _ = executor
        .invoke_and_await(&worker_id, "golem:it/api.{checkout}", vec![])
        .await?;

    save_recovery_golden_file(&executor, &context, "shopping_cart_example", &worker_id).await?;

    executor.check_oplog_is_queryable(&worker_id).await?;
    drop(executor);

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
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout(120_000)]
async fn dynamic_worker_creation(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component(&context.default_environment_id, "environment-service")
        .store()
        .await?;
    let worker_id = WorkerId {
        component_id: component.id,
        worker_name: "dynamic-worker-creation-1".to_string(),
    };

    let args = executor
        .invoke_and_await(&worker_id, "golem:it/api.{get-arguments}", vec![])
        .await??;

    let env = executor
        .invoke_and_await(&worker_id, "golem:it/api.{get-environment}", vec![])
        .await??;

    check!(args == vec![Value::Result(Ok(Some(Box::new(Value::List(vec![])))))]);
    check!(
        env == vec![Value::Result(Ok(Some(Box::new(Value::List(vec![
            Value::Tuple(vec![
                Value::String("GOLEM_AGENT_ID".to_string()),
                Value::String("dynamic-worker-creation-1".to_string())
            ]),
            Value::Tuple(vec![
                Value::String("GOLEM_WORKER_NAME".to_string()),
                Value::String("dynamic-worker-creation-1".to_string())
            ]),
            Value::Tuple(vec![
                Value::String("GOLEM_COMPONENT_ID".to_string()),
                Value::String(format!("{}", component.id))
            ]),
            Value::Tuple(vec![
                Value::String("GOLEM_COMPONENT_REVISION".to_string()),
                Value::String("0".to_string())
            ]),
        ])))))]
    );
    Ok(())
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
#[timeout(120_000)]
async fn ephemeral_worker_creation_with_name_is_not_persistent(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component(&context.default_environment_id, "it_agent_counters_release")
        .store()
        .await?;
    let worker_id = WorkerId {
        component_id: component.id,
        worker_name: "ephemeral-counter(\"test\")".to_string(),
    };

    executor
        .invoke_and_await(
            &worker_id,
            "it:agent-counters/ephemeral-counter.{increment}",
            vec![],
        )
        .await??;

    let result = executor
        .invoke_and_await(
            &worker_id,
            "it:agent-counters/ephemeral-counter.{increment}",
            vec![],
        )
        .await??;

    drop(executor);

    assert_eq!(result, vec![Value::U32(1)]);
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout(120_000)]
async fn promise(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component(&context.default_environment_id, "promise")
        .store()
        .await?;
    let worker_id = executor.start_worker(&component.id, "promise-1").await?;

    let result = executor
        .invoke_and_await(&worker_id, "golem:it/api.{create}", vec![])
        .await??;
    let promise_id = ValueAndType::new(result[0].clone(), PromiseId::get_type());

    let poll1 = executor
        .invoke_and_await(&worker_id, "golem:it/api.{poll}", vec![promise_id.clone()])
        .await?;

    let executor_clone = executor.clone();
    let worker_id_clone = worker_id.clone();
    let promise_id_clone = promise_id.clone();

    let fiber = tokio::spawn(
        async move {
            executor_clone
                .invoke_and_await(
                    &worker_id_clone,
                    "golem:it/api.{await}",
                    vec![promise_id_clone],
                )
                .await
        }
        .in_current_span(),
    );

    info!("Waiting for worker to be suspended on promise");

    // While waiting for the promise, the worker gets suspended
    executor
        .wait_for_status(&worker_id, WorkerStatus::Suspended, Duration::from_secs(10))
        .await?;

    info!("Completing promise to resume worker");

    executor
        .complete_promise(
            &PromiseId {
                worker_id: worker_id.clone(),
                oplog_idx: OplogIndex::from_u64(4),
            },
            vec![42],
        )
        .await?;

    let result = fiber.await??;

    let poll2 = executor
        .invoke_and_await(&worker_id, "golem:it/api.{poll}", vec![promise_id.clone()])
        .await?;

    executor.check_oplog_is_queryable(&worker_id).await?;

    check!(result == Ok(vec![Value::List(vec![Value::U8(42)])]));
    check!(poll1 == Ok(vec![Value::Option(None)]));
    check!(
        poll2
            == Ok(vec![Value::Option(Some(Box::new(Value::List(vec![
                Value::U8(42)
            ]))))])
    );
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout(120_000)]
async fn get_workers_from_worker(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("golem_host")] type_resolve: &SharedAnalysedTypeResolve,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component(&context.default_environment_id, "runtime-service")
        .store()
        .await?;

    let worker_id1 = executor
        .start_worker(&component.id, "runtime-service-3")
        .await?;

    let worker_id2 = executor
        .start_worker(&component.id, "runtime-service-4")
        .await?;

    async fn get_check(
        worker_id: &WorkerId,
        name_filter: Option<String>,
        expected_count: usize,
        executor: &TestWorkerExecutor,
        mut type_resolve: SharedAnalysedTypeResolve,
    ) -> anyhow::Result<()> {
        let component_id_val_and_type = {
            let (high, low) = worker_id.component_id.0.as_u64_pair();
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
                        Value::Enum(0),
                        Value::String(name.clone()),
                    ]))),
                }],
            )])])])
        });

        let result = executor
            .invoke_and_await(
                worker_id,
                "golem:it/api.{get-workers}",
                vec![
                    component_id_val_and_type,
                    ValueAndType {
                        value: Value::Option(filter_val.map(Box::new)),
                        typ: analysed_type::option(
                            type_resolve
                                .analysed_type(&TypeName {
                                    package: Some("golem:api@1.3.0".to_string()),
                                    owner: TypeOwner::Interface("host".to_string()),
                                    name: Some("agent-any-filter".to_string()),
                                })
                                .unwrap(),
                        ),
                    },
                    true.into_value_and_type(),
                ],
            )
            .await??;

        match result.first() {
            Some(Value::List(list)) => {
                check!(list.len() == expected_count);
            }
            _ => {
                check!(false);
            }
        }
        Ok(())
    }
    get_check(&worker_id1, None, 2, &executor, type_resolve.clone()).await?;
    get_check(
        &worker_id2,
        Some("runtime-service-3".to_string()),
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
#[timeout(120_000)]
async fn get_metadata_from_worker(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component(&context.default_environment_id, "runtime-service")
        .store()
        .await?;

    let worker_id1 = executor
        .start_worker(&component.id, "runtime-service-1")
        .await?;

    let worker_id2 = executor
        .start_worker(&component.id, "runtime-service-2")
        .await?;

    fn get_worker_id_val(worker_id: &WorkerId) -> Value {
        let component_id_val = {
            let (high, low) = worker_id.component_id.0.as_u64_pair();
            Value::Record(vec![Value::Record(vec![Value::U64(high), Value::U64(low)])])
        };

        Value::Record(vec![
            component_id_val,
            Value::String(worker_id.worker_name.clone()),
        ])
    }

    async fn get_check(
        worker_id1: &WorkerId,
        worker_id2: &WorkerId,
        executor: &TestWorkerExecutor,
    ) -> anyhow::Result<()> {
        let worker_id_val1 = get_worker_id_val(worker_id1);

        let result = executor
            .invoke_and_await(worker_id1, "golem:it/api.{get-self-metadata}", vec![])
            .await??;

        match result.first() {
            Some(Value::Record(values)) if !values.is_empty() => {
                let id_val = values.first().unwrap();
                check!(worker_id_val1 == *id_val);
            }
            _ => {
                check!(false);
            }
        }

        let worker_id_val2 = get_worker_id_val(worker_id2);

        let result = executor
            .invoke_and_await(
                worker_id1,
                "golem:it/api.{get-worker-metadata}",
                vec![ValueAndType {
                    value: worker_id_val2.clone(),
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
                        analysed_type::field("worker-name", analysed_type::str()),
                    ]),
                }],
            )
            .await??;

        match result.first() {
            Some(Value::Option(value)) if value.is_some() => {
                let result = *value.clone().unwrap();
                match result {
                    Value::Record(values) if !values.is_empty() => {
                        let id_val = values.first().unwrap();
                        check!(worker_id_val2 == *id_val);
                    }
                    _ => {
                        check!(false);
                    }
                }
            }
            _ => {
                check!(false);
            }
        }
        Ok(())
    }

    get_check(&worker_id1, &worker_id2, &executor).await?;
    get_check(&worker_id2, &worker_id1, &executor).await?;

    executor.check_oplog_is_queryable(&worker_id1).await?;
    executor.check_oplog_is_queryable(&worker_id2).await?;
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout(120_000)]
async fn invoking_with_same_idempotency_key_is_idempotent(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component(&context.default_environment_id, "shopping-cart")
        .store()
        .await?;
    let worker_id = executor
        .start_worker(&component.id, "shopping-cart-2")
        .await?;

    let idempotency_key = IdempotencyKey::fresh();
    let _result = executor
        .invoke_and_await_with_key(
            &worker_id,
            &idempotency_key,
            "golem:it/api.{add-item}",
            vec![Record(vec![
                ("product-id", "G1000".into_value_and_type()),
                ("name", "Golem T-Shirt M".into_value_and_type()),
                ("price", 100.0f32.into_value_and_type()),
                ("quantity", 5u32.into_value_and_type()),
            ])
            .into_value_and_type()],
        )
        .await??;

    let _result2 = executor
        .invoke_and_await_with_key(
            &worker_id,
            &idempotency_key,
            "golem:it/api.{add-item}",
            vec![Record(vec![
                ("product-id", "G1000".into_value_and_type()),
                ("name", "Golem T-Shirt M".into_value_and_type()),
                ("price", 100.0f32.into_value_and_type()),
                ("quantity", 5u32.into_value_and_type()),
            ])
            .into_value_and_type()],
        )
        .await??;

    let contents = executor
        .invoke_and_await(&worker_id, "golem:it/api.{get-cart-contents}", vec![])
        .await??;

    executor.check_oplog_is_queryable(&worker_id).await?;

    check!(
        contents
            == vec![Value::List(vec![Value::Record(vec![
                Value::String("G1000".to_string()),
                Value::String("Golem T-Shirt M".to_string()),
                Value::F32(100.0),
                Value::U32(5),
            ])])]
    );
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout(120_000)]
async fn invoking_with_same_idempotency_key_is_idempotent_after_restart(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component(&context.default_environment_id, "shopping-cart")
        .store()
        .await?;
    let worker_id = executor
        .start_worker(&component.id, "shopping-cart-4")
        .await?;

    let idempotency_key = IdempotencyKey::fresh();
    let _result = executor
        .invoke_and_await_with_key(
            &worker_id,
            &idempotency_key,
            "golem:it/api.{add-item}",
            vec![Record(vec![
                ("product-id", "G1000".into_value_and_type()),
                ("name", "Golem T-Shirt M".into_value_and_type()),
                ("price", 100.0f32.into_value_and_type()),
                ("quantity", 5u32.into_value_and_type()),
            ])
            .into_value_and_type()],
        )
        .await??;

    drop(executor);
    let executor = start(deps, &context).await?;

    let _result2 = executor
        .invoke_and_await_with_key(
            &worker_id,
            &idempotency_key,
            "golem:it/api.{add-item}",
            vec![Record(vec![
                ("product-id", "G1000".into_value_and_type()),
                ("name", "Golem T-Shirt M".into_value_and_type()),
                ("price", 100.0f32.into_value_and_type()),
                ("quantity", 5u32.into_value_and_type()),
            ])
            .into_value_and_type()],
        )
        .await??;

    let contents = executor
        .invoke_and_await(&worker_id, "golem:it/api.{get-cart-contents}", vec![])
        .await??;

    executor.check_oplog_is_queryable(&worker_id).await?;

    check!(
        contents
            == vec![Value::List(vec![Value::Record(vec![
                Value::String("G1000".to_string()),
                Value::String("Golem T-Shirt M".to_string()),
                Value::F32(100.0),
                Value::U32(5),
            ])])]
    );
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout(120_000)]
async fn component_env_variables(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component(&context.default_environment_id, "environment-service")
        .with_env(vec![("FOO".to_string(), "bar".to_string())])
        .store()
        .await?;

    let worker_id = WorkerId {
        component_id: component.id,
        worker_name: "component-env-variables-1".to_string(),
    };

    let env = executor
        .invoke_and_await(&worker_id, "golem:it/api.{get-environment}", vec![])
        .await??;

    check!(
        env == vec![Value::Result(Ok(Some(Box::new(Value::List(vec![
            Value::Tuple(vec![
                Value::String("FOO".to_string()),
                Value::String("bar".to_string())
            ]),
            Value::Tuple(vec![
                Value::String("GOLEM_AGENT_ID".to_string()),
                Value::String("component-env-variables-1".to_string())
            ]),
            Value::Tuple(vec![
                Value::String("GOLEM_WORKER_NAME".to_string()),
                Value::String("component-env-variables-1".to_string())
            ]),
            Value::Tuple(vec![
                Value::String("GOLEM_COMPONENT_ID".to_string()),
                Value::String(format!("{}", component.id))
            ]),
            Value::Tuple(vec![
                Value::String("GOLEM_COMPONENT_REVISION".to_string()),
                Value::String("0".to_string())
            ]),
        ])))))]
    );

    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout(120_000)]
async fn component_env_variables_update(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component(&context.default_environment_id, "environment-service")
        .with_env(vec![("FOO".to_string(), "bar".to_string())])
        .store()
        .await?;

    let worker_id = executor
        .start_worker(&component.id, "component-env-variables-1")
        .await?;

    let WorkerMetadataDto { mut env, .. } = executor.get_worker_metadata(&worker_id).await?;

    env.retain(|k, _| k == "FOO");

    assert_eq!(
        env,
        HashMap::from_iter(vec![("FOO".to_string(), "bar".to_string())])
    );

    let updated_component = executor
        .update_component_with_env(
            &component.id,
            "environment-service",
            &[("BAR".to_string(), "baz".to_string())],
        )
        .await?;

    executor
        .auto_update_worker(&worker_id, updated_component.revision)
        .await?;

    let env = executor
        .invoke_and_await(&worker_id, "golem:it/api.{get-environment}", vec![])
        .await??;

    let env = get_env_result(env);

    assert_eq!(env.get("FOO"), Some(&"bar".to_string()));
    assert_eq!(env.get("BAR"), Some(&"baz".to_string()));
    assert_eq!(
        env.get("GOLEM_AGENT_ID"),
        Some(&"component-env-variables-1".to_string())
    );

    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout(120_000)]
async fn component_env_and_worker_env_priority(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component(&context.default_environment_id, "environment-service")
        .with_env(vec![("FOO".to_string(), "bar".to_string())])
        .store()
        .await?;

    let worker_env = HashMap::from_iter(vec![("FOO".to_string(), "baz".to_string())]);

    let worker_id = executor
        .start_worker_with(
            &component.id,
            "component-env-variables-1",
            worker_env,
            vec![],
        )
        .await?;

    let WorkerMetadataDto { mut env, .. } = executor.get_worker_metadata(&worker_id).await?;
    env.retain(|k, _| k == "FOO");

    assert_eq!(
        env,
        HashMap::from_iter(vec![("FOO".to_string(), "baz".to_string())])
    );
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout(120_000)]
async fn optional_parameters(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component(&context.default_environment_id, "option-service")
        .store()
        .await?;
    let worker_id = executor
        .start_worker(&component.id, "optional-service-1")
        .await?;

    let echo_some = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{echo}",
            vec![Some("Hello").into_value_and_type()],
        )
        .await??;

    let echo_none = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{echo}",
            vec![None::<String>.into_value_and_type()],
        )
        .await??;

    let todo_some = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{todo}",
            vec![Record(vec![
                ("name", "todo".into_value_and_type()),
                ("description", Some("description").into_value_and_type()),
            ])
            .into_value_and_type()],
        )
        .await??;

    let todo_none = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{todo}",
            vec![Record(vec![
                ("name", "todo".into_value_and_type()),
                ("description", Some("description").into_value_and_type()),
            ])
            .into_value_and_type()],
        )
        .await??;

    check!(
        echo_some
            == vec![Value::Option(Some(Box::new(Value::String(
                "Hello".to_string()
            ))))]
    );
    check!(echo_none == vec![Value::Option(None)]);
    check!(todo_some == vec![Value::String("todo".to_string())]);
    check!(todo_none == vec![Value::String("todo".to_string())]);

    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout(120_000)]
async fn flags_parameters(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component(&context.default_environment_id, "flags-service")
        .store()
        .await?;
    let worker_id = executor
        .start_worker(&component.id, "flags-service-1")
        .await?;

    let create_task = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{create-task}",
            vec![Record(vec![
                ("name", "t1".into_value_and_type()),
                (
                    "permissions",
                    ValueAndType {
                        value: Value::Flags(vec![true, true, false, false]),
                        typ: analysed_type::flags(&["read", "write", "exec", "close"]),
                    },
                ),
            ])
            .into_value_and_type()],
        )
        .await??;

    let get_tasks = executor
        .invoke_and_await(&worker_id, "golem:it/api.{get-tasks}", vec![])
        .await??;

    check!(
        create_task
            == vec![Value::Record(vec![
                Value::String("t1".to_string()),
                Value::Flags(vec![true, true, true, false])
            ])]
    );
    check!(
        get_tasks
            == vec![Value::List(vec![
                Value::Record(vec![
                    Value::String("t1".to_string()),
                    Value::Flags(vec![true, false, false, false])
                ]),
                Value::Record(vec![
                    Value::String("t2".to_string()),
                    Value::Flags(vec![true, false, true, true])
                ])
            ])]
    );

    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout(120_000)]
async fn variants_with_no_payloads(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component(&context.default_environment_id, "variant-service")
        .store()
        .await?;
    let worker_id = executor
        .start_worker(&component.id, "variant-service-1")
        .await?;

    let result = executor
        .invoke_and_await(&worker_id, "golem:it/api.{bid}", vec![])
        .await?;

    check!(result.is_ok());
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout(120_000)]
async fn delete_worker(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component(&context.default_environment_id, "option-service")
        .store()
        .await?;
    let worker_id = executor
        .start_worker(&component.id, "delete-worker-1")
        .await?;

    let _ = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{echo}",
            vec![Some("Hello").into_value_and_type()],
        )
        .await?;

    let metadata1 = executor.get_worker_metadata(&worker_id).await;

    let (cursor1, values1) = executor
        .get_workers_metadata(
            &worker_id.component_id,
            Some(WorkerFilter::new_name(
                StringFilterComparator::Equal,
                worker_id.worker_name.clone(),
            )),
            ScanCursor::default(),
            10,
            true,
        )
        .await?;

    executor.delete_worker(&worker_id).await?;

    let metadata2 = executor.get_worker_metadata(&worker_id).await;

    check!(values1.len() == 1);
    check!(cursor1.is_none());
    check!(metadata1.is_ok());
    check!(metadata2.is_err());
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout(120_000)]
async fn get_workers(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    async fn get_check(
        component_id: &ComponentId,
        filter: Option<WorkerFilter>,
        expected_count: usize,
        executor: &TestWorkerExecutor,
    ) -> anyhow::Result<Vec<WorkerMetadataDto>> {
        let (cursor, values) = executor
            .get_workers_metadata(component_id, filter, ScanCursor::default(), 20, true)
            .await?;

        check!(values.len() == expected_count);
        check!(cursor.is_none());

        Ok(values)
    }

    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component(&context.default_environment_id, "option-service")
        .store()
        .await?;

    let workers_count = 10;
    let mut worker_ids = vec![];

    for i in 0..workers_count {
        let worker_id = executor
            .start_worker(&component.id, &format!("test-worker-{i}"))
            .await?;

        worker_ids.push(worker_id);
    }

    for worker_id in worker_ids.clone() {
        let _ = executor
            .invoke_and_await(
                &worker_id,
                "golem:it/api.{echo}",
                vec![Some("Hello").into_value_and_type()],
            )
            .await??;

        get_check(
            &component.id,
            Some(WorkerFilter::new_name(
                StringFilterComparator::Equal,
                worker_id.worker_name.clone(),
            )),
            1,
            &executor,
        )
        .await?;
    }

    get_check(
        &component.id,
        Some(WorkerFilter::new_name(
            StringFilterComparator::Like,
            "test".to_string(),
        )),
        workers_count,
        &executor,
    )
    .await?;

    get_check(
        &component.id,
        Some(
            WorkerFilter::new_name(StringFilterComparator::Like, "test".to_string())
                .and(
                    WorkerFilter::new_status(FilterComparator::Equal, WorkerStatus::Idle).or(
                        WorkerFilter::new_status(FilterComparator::Equal, WorkerStatus::Running),
                    ),
                )
                .and(WorkerFilter::new_version(
                    FilterComparator::Equal,
                    ComponentRevision(0),
                )),
        ),
        workers_count,
        &executor,
    )
    .await?;

    get_check(
        &component.id,
        Some(WorkerFilter::new_name(StringFilterComparator::Like, "test".to_string()).not()),
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

    check!(cursor1.is_some());
    check!(values1.len() >= workers_count / 2);

    let (cursor2, values2) = executor
        .get_workers_metadata(
            &component.id,
            None,
            cursor1.unwrap(),
            (workers_count - values1.len()) as u64,
            true,
        )
        .await?;

    check!(values2.len() == workers_count - values1.len());

    if let Some(cursor2) = cursor2 {
        let (_, values3) = executor
            .get_workers_metadata(&component.id, None, cursor2, workers_count as u64, true)
            .await?;
        check!(values3.len() == 0);
    }

    for worker_id in worker_ids {
        executor.delete_worker(&worker_id).await?;
    }

    get_check(&component.id, None, 0, &executor).await?;
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout(120_000)]
async fn error_handling_when_worker_is_invoked_with_fewer_than_expected_parameters(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component(&context.default_environment_id, "option-service")
        .store()
        .await?;
    let worker_id = executor
        .start_worker(&component.id, "fewer-than-expected-parameters-1")
        .await?;

    let failure = executor
        .invoke_and_await(&worker_id, "golem:it/api.{echo}", vec![])
        .await?;

    executor.check_oplog_is_queryable(&worker_id).await?;
    check!(failure.is_err());
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout(120_000)]
async fn error_handling_when_worker_is_invoked_with_more_than_expected_parameters(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component(&context.default_environment_id, "option-service")
        .store()
        .await?;
    let worker_id = executor
        .start_worker(&component.id, "more-than-expected-parameters-1")
        .await?;

    let failure = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{echo}",
            vec![
                Some("Hello").into_value_and_type(),
                "extra parameter".into_value_and_type(),
            ],
        )
        .await?;

    executor.check_oplog_is_queryable(&worker_id).await?;

    check!(failure.is_err());

    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout(120_000)]
async fn get_worker_metadata(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component(&context.default_environment_id, "clock-service")
        .store()
        .await?;

    let worker_id = executor
        .start_worker(&component.id, "get-worker-metadata-1")
        .await?;

    let worker_id_clone = worker_id.clone();
    let executor_clone = executor.clone();
    let fiber = tokio::spawn(
        async move {
            executor_clone
                .invoke_and_await(
                    &worker_id_clone,
                    "golem:it/api.{sleep}",
                    vec![2u64.into_value_and_type()],
                )
                .await
        }
        .in_current_span(),
    );

    let metadata1 = executor
        .wait_for_statuses(
            &worker_id,
            &[WorkerStatus::Running, WorkerStatus::Suspended],
            Duration::from_secs(5),
        )
        .await?;

    fiber.await???;

    let metadata2 = executor
        .wait_for_status(&worker_id, WorkerStatus::Idle, Duration::from_secs(10))
        .await?;

    executor.check_oplog_is_queryable(&worker_id).await?;

    check!(
        metadata1.status == WorkerStatus::Suspended || // it is sleeping - whether it is suspended or not is the server's decision
        metadata1.status == WorkerStatus::Running
    );
    check!(metadata2.status == WorkerStatus::Idle);
    check!(metadata1.component_version == ComponentRevision(0));
    check!(metadata1.worker_id == worker_id);
    check!(metadata1.created_by == context.account_id);

    check!(metadata2.component_size == 200406);
    check!(metadata2.total_linear_memory_size == 1245184);
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout(120_000)]
async fn create_invoke_delete_create_invoke(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component(&context.default_environment_id, "shopping-cart")
        .store()
        .await?;
    let worker_id = executor
        .start_worker(&component.id, "create-invoke-delete-create-invoke-1")
        .await?;

    let r1 = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{add-item}",
            vec![Record(vec![
                ("product-id", "G1000".into_value_and_type()),
                ("name", "Golem T-Shirt M".into_value_and_type()),
                ("price", 100.0f32.into_value_and_type()),
                ("quantity", 5u32.into_value_and_type()),
            ])
            .into_value_and_type()],
        )
        .await?;

    executor.delete_worker(&worker_id).await?;

    let worker_id = executor
        .start_worker(&component.id, "create-invoke-delete-create-invoke-1") // same name as before
        .await?;

    let r2 = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{add-item}",
            vec![Record(vec![
                ("product-id", "G1000".into_value_and_type()),
                ("name", "Golem T-Shirt M".into_value_and_type()),
                ("price", 100.0f32.into_value_and_type()),
                ("quantity", 5u32.into_value_and_type()),
            ])
            .into_value_and_type()],
        )
        .await?;

    executor.check_oplog_is_queryable(&worker_id).await?;

    check!(r1.is_ok());
    check!(r2.is_ok());
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout(120_000)]
async fn recovering_an_old_worker_after_updating_a_component(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component(&context.default_environment_id, "shopping-cart")
        .unique()
        .store()
        .await?;
    let worker_id = executor
        .start_worker(
            &component.id,
            "recovering-an-old-worker-after-updating-a-component-1",
        )
        .await?;

    let r1 = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{add-item}",
            vec![Record(vec![
                ("product-id", "G1000".into_value_and_type()),
                ("name", "Golem T-Shirt M".into_value_and_type()),
                ("price", 100.0f32.into_value_and_type()),
                ("quantity", 5u32.into_value_and_type()),
            ])
            .into_value_and_type()],
        )
        .await??;

    // Updating the component with an incompatible new version
    executor
        .update_component(&component.id, "option-service")
        .await?;

    // Creating a new worker of the updated component and call it
    let worker_id2 = executor
        .start_worker(
            &component.id,
            "recovering-an-old-worker-after-updating-a-component-2",
        )
        .await?;

    let r2 = executor
        .invoke_and_await(
            &worker_id2,
            "golem:it/api.{echo}",
            vec![Some("Hello").into_value_and_type()],
        )
        .await??;

    // Restarting the server to force worker recovery
    drop(executor);
    let executor = start(deps, &context).await?;

    // Call the first worker again to check if it is still working
    let r3 = executor
        .invoke_and_await(&worker_id, "golem:it/api.{get-cart-contents}", vec![])
        .await??;

    executor.check_oplog_is_queryable(&worker_id).await?;
    drop(executor);

    check!(r1 == vec![]);
    check!(
        r2 == vec![Value::Option(Some(Box::new(Value::String(
            "Hello".to_string()
        ))))]
    );
    check!(
        r3 == vec![Value::List(vec![Value::Record(vec![
            Value::String("G1000".to_string()),
            Value::String("Golem T-Shirt M".to_string()),
            Value::F32(100.0),
            Value::U32(5),
        ])])]
    );
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout(120_000)]
async fn recreating_a_worker_after_it_got_deleted_with_a_different_version(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component(&context.default_environment_id, "shopping-cart")
        .unique()
        .store()
        .await?;
    let worker_id = executor
        .start_worker(
            &component.id,
            "recreating-an-worker-after-it-got-deleted-with-a-different-version-1",
        )
        .await?;

    let r1 = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{add-item}",
            vec![Record(vec![
                ("product-id", "G1000".into_value_and_type()),
                ("name", "Golem T-Shirt M".into_value_and_type()),
                ("price", 100.0f32.into_value_and_type()),
                ("quantity", 5u32.into_value_and_type()),
            ])
            .into_value_and_type()],
        )
        .await??;

    // Updating the component with an incompatible new version
    executor
        .update_component(&component.id, "option-service")
        .await?;

    // Deleting the first instance
    executor.delete_worker(&worker_id).await?;

    // Create a new instance with the same name and call it the first instance again to check if it is still working
    let worker_id = executor
        .start_worker(
            &component.id,
            "recovering-an-old-worker-after-updating-a-component-1",
        )
        .await?;

    let r2 = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{echo}",
            vec![Some("Hello").into_value_and_type()],
        )
        .await??;

    executor.check_oplog_is_queryable(&worker_id).await?;
    drop(executor);

    check!(r1 == vec![]);
    check!(
        r2 == vec![Value::Option(Some(Box::new(Value::String(
            "Hello".to_string()
        ))))]
    );
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout(120_000)]
async fn trying_to_use_an_old_wasm_provides_good_error_message(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    // case: WASM is an old version, rejected by protector

    let executor = start(deps, &context).await?;

    let component = executor
        .component(&context.default_environment_id, "old-component")
        .unverified()
        .store()
        .await?;

    let result = executor
        .try_start_worker(&component.id, "old-component-1")
        .await?;

    check!(result.is_err());
    {
        assert2::let_assert!(
            Err(WorkerExecutorError::ComponentParseFailed {
                component_id,
                component_version,
                reason
            }) = result
        );
        assert!(component_id == component.id);
        assert!(component_version == ComponentRevision(0));
        assert!(reason == "failed to parse WebAssembly module");
    };

    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout(120_000)]
async fn trying_to_use_a_wasm_that_wasmtime_cannot_load_provides_good_error_message(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    // case: WASM can be parsed, but wasmtime does not support it
    let executor = start(deps, &context).await?;
    let component = executor
        .component(&context.default_environment_id, "write-stdout")
        .store()
        .await?;

    let cwd = env::current_dir()?;
    debug!("Current directory: {cwd:?}");
    let target_dir = cwd.join(Path::new("data/components"));
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

    let result = executor
        .try_start_worker(&component.id, "bad-wasm-1")
        .await?;

    check!(result.is_err());
    {
        assert2::let_assert!(
            Err(WorkerExecutorError::ComponentParseFailed {
                component_id,
                component_version,
                reason
            }) = result
        );
        assert!(component_id == component.id);
        assert!(component_version == ComponentRevision(0));
        assert!(reason == "failed to parse WebAssembly module");
    };
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout(120_000)]
async fn trying_to_use_a_wasm_that_wasmtime_cannot_load_provides_good_error_message_after_recovery(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;
    let component = executor
        .component(&context.default_environment_id, "write-stdout")
        .store()
        .await?;

    let worker_id = executor.start_worker(&component.id, "bad-wasm-2").await?;

    // worker is idle. if we restart the server, it will get recovered
    drop(executor);

    // corrupting the uploaded WASM
    let cwd = env::current_dir().expect("Failed to get current directory");
    debug!("Current directory: {cwd:?}");
    let component_path = cwd.join(format!("data/components/wasms/{}-0.wasm", component.id));
    let compiled_component_path = cwd.join(Path::new(&format!(
        "data/blobs/compilation_cache/{}/{}/0.cwasm",
        component.environment_id, component.id
    )));

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
    let result = executor.invoke_and_await(&worker_id, "run", vec![]).await?;

    check!(result.is_err());
    {
        assert2::let_assert!(
            Err(WorkerExecutorError::ComponentParseFailed {
                component_id,
                component_version,
                reason
            }) = result
        );
        assert!(component_id == component.id);
        assert!(component_version == ComponentRevision(0));
        assert!(reason == "failed to parse WebAssembly module");
    };

    executor.check_oplog_is_queryable(&worker_id).await?;

    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout(120_000)]
async fn long_running_poll_loop_works_as_expected(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
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
        .component(&context.default_environment_id, "http-client-2")
        .store()
        .await?;
    let mut env = HashMap::new();
    env.insert("PORT".to_string(), host_http_port.to_string());
    env.insert("RUST_BACKTRACE".to_string(), "1".to_string());

    let worker_id = executor
        .start_worker_with(&component.id, "poll-loop-component-0", env, vec![])
        .await?;

    executor.log_output(&worker_id).await?;

    executor
        .invoke(
            &worker_id,
            "golem:it/api.{start-polling}",
            vec!["first".into_value_and_type()],
        )
        .await??;

    executor
        .wait_for_status(&worker_id, WorkerStatus::Running, Duration::from_secs(10))
        .await?;

    {
        let mut response = response.lock().unwrap();
        *response = "first".to_string();
    }

    executor
        .wait_for_status(&worker_id, WorkerStatus::Idle, Duration::from_secs(10))
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
#[timeout(120_000)]
async fn long_running_poll_loop_http_failures_are_retried(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start_customized(
        deps,
        &context,
        None,
        Some(RetryConfig {
            max_attempts: 30,
            min_delay: Duration::from_millis(100),
            max_delay: Duration::from_millis(500),
            multiplier: 1.5,
            max_jitter_factor: None,
        }),
    )
    .await?;

    let response = Arc::new(Mutex::new("initial".to_string()));
    let poll_count = Arc::new(AtomicUsize::new(0));

    let (host_http_port, http_server) =
        start_http_poll_server(response.clone(), poll_count.clone(), None).await;

    let component = executor
        .component(&context.default_environment_id, "http-client-2")
        .store()
        .await?;
    let mut env = HashMap::new();
    env.insert("PORT".to_string(), host_http_port.to_string());
    env.insert("RUST_BACKTRACE".to_string(), "1".to_string());

    let worker_id = executor
        .start_worker_with(&component.id, "poll-loop-component-0", env, vec![])
        .await?;

    executor.log_output(&worker_id).await?;

    executor
        .invoke(
            &worker_id,
            "golem:it/api.{start-polling}",
            vec!["stop now".into_value_and_type()],
        )
        .await??;

    executor
        .wait_for_status(&worker_id, WorkerStatus::Running, Duration::from_secs(10))
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
        if begin.elapsed() > Duration::from_secs(2) {
            return Err(anyhow!("No polls in 2 seconds"));
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
        .wait_for_status(&worker_id, WorkerStatus::Idle, Duration::from_secs(10))
        .await?;

    executor.check_oplog_is_queryable(&worker_id).await?;
    drop(executor);
    http_server.abort();
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout(120_000)]
async fn long_running_poll_loop_works_as_expected_async_http(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
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
        .component(&context.default_environment_id, "http-client-3")
        .store()
        .await?;
    let mut env = HashMap::new();
    env.insert("PORT".to_string(), host_http_port.to_string());
    env.insert("RUST_BACKTRACE".to_string(), "1".to_string());

    let worker_id = executor
        .start_worker_with(&component.id, "poll-loop-component-0", env, vec![])
        .await?;

    executor.log_output(&worker_id).await?;

    executor
        .invoke(
            &worker_id,
            "golem:it/api.{start-polling}",
            vec!["first".into_value_and_type()],
        )
        .await??;

    executor
        .wait_for_status(&worker_id, WorkerStatus::Running, Duration::from_secs(10))
        .await?;

    {
        let mut response = response.lock().unwrap();
        *response = "first".to_string();
    }

    executor
        .wait_for_status(&worker_id, WorkerStatus::Idle, Duration::from_secs(10))
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
        .component(&context.default_environment_id, "http-client-2")
        .store()
        .await?;
    let mut env = HashMap::new();
    env.insert("PORT".to_string(), host_http_port.to_string());
    let worker_id = executor
        .start_worker_with(&component.id, "poll-loop-component-1", env, vec![])
        .await?;

    executor.log_output(&worker_id).await?;

    executor
        .invoke(
            &worker_id,
            "golem:it/api.{start-polling}",
            vec!["first".into_value_and_type()],
        )
        .await??;

    executor
        .wait_for_status(&worker_id, WorkerStatus::Running, Duration::from_secs(20))
        .await?;

    let values1 = executor
        .get_running_workers_metadata(
            &worker_id.component_id,
            Some(WorkerFilter::new_name(
                StringFilterComparator::Equal,
                worker_id.worker_name.clone(),
            )),
        )
        .await?;

    executor.interrupt(&worker_id).await?;

    executor
        .wait_for_status(
            &worker_id,
            WorkerStatus::Interrupted,
            Duration::from_secs(5),
        )
        .await?;

    let values2 = executor
        .get_running_workers_metadata(
            &worker_id.component_id,
            Some(WorkerFilter::new_name(
                StringFilterComparator::Equal,
                worker_id.worker_name.clone(),
            )),
        )
        .await?;

    executor
        .invoke(
            &worker_id,
            "golem:it/api.{start-polling}",
            vec!["second".into_value_and_type()],
        )
        .await??;

    executor
        .wait_for_status(&worker_id, WorkerStatus::Running, Duration::from_secs(20))
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
        .wait_for_status(&worker_id, WorkerStatus::Idle, Duration::from_secs(20))
        .await?;

    executor.check_oplog_is_queryable(&worker_id).await?;
    drop(executor);
    http_server.abort();

    check!(!values1.is_empty());
    // first running
    check!(values2.is_empty());
    // first interrupted
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout(120_000)]
async fn long_running_poll_loop_connection_breaks_on_interrupt(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
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
        .component(&context.default_environment_id, "http-client-2")
        .store()
        .await?;
    let mut env = HashMap::new();
    env.insert("PORT".to_string(), host_http_port.to_string());
    let worker_id = executor
        .start_worker_with(&component.id, "poll-loop-component-2", env, vec![])
        .await?;

    let (mut rx, _abort_capture) = executor.capture_output_with_termination(&worker_id).await?;

    executor
        .invoke(
            &worker_id,
            "golem:it/api.{start-polling}",
            vec!["first".into_value_and_type()],
        )
        .await??;

    executor
        .wait_for_status(&worker_id, WorkerStatus::Running, Duration::from_secs(10))
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
#[timeout(120_000)]
async fn long_running_poll_loop_connection_retry_does_not_resume_interrupted_worker(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
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
        .component(&context.default_environment_id, "http-client-2")
        .store()
        .await?;
    let mut env = HashMap::new();
    env.insert("PORT".to_string(), host_http_port.to_string());

    let worker_id = executor
        .start_worker_with(&component.id, "poll-loop-component-3", env, vec![])
        .await?;

    let (rx, _abort_capture) = executor.capture_output_with_termination(&worker_id).await?;

    executor
        .invoke(
            &worker_id,
            "golem:it/api.{start-polling}",
            vec!["first".into_value_and_type()],
        )
        .await??;

    executor
        .wait_for_status(&worker_id, WorkerStatus::Running, Duration::from_secs(10))
        .await?;

    executor.interrupt(&worker_id).await?;

    let _ = drain_connection(rx).await;
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

    check!(status1.status == WorkerStatus::Interrupted);
    check!(status2.status == WorkerStatus::Interrupted);

    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout(120_000)]
async fn long_running_poll_loop_connection_can_be_restored_after_resume(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
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
        .component(&context.default_environment_id, "http-client-2")
        .store()
        .await?;
    let mut env = HashMap::new();
    env.insert("PORT".to_string(), host_http_port.to_string());

    let worker_id = executor
        .start_worker_with(&component.id, "poll-loop-component-4", env, vec![])
        .await?;

    let (rx, _abort_capture) = executor.capture_output_with_termination(&worker_id).await?;

    executor
        .invoke(
            &worker_id,
            "golem:it/api.{start-polling}",
            vec!["first".into_value_and_type()],
        )
        .await??;

    executor
        .wait_for_status(&worker_id, WorkerStatus::Running, Duration::from_secs(10))
        .await?;

    executor.interrupt(&worker_id).await?;

    drain_connection(rx).await;
    let status2 = executor.get_worker_metadata(&worker_id).await?;

    executor.resume(&worker_id, false).await?;
    executor
        .wait_for_status(&worker_id, WorkerStatus::Running, Duration::from_secs(10))
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
        .wait_for_status(&worker_id, WorkerStatus::Idle, Duration::from_secs(5))
        .await?;

    let status4 = executor.get_worker_metadata(&worker_id).await?;

    executor.check_oplog_is_queryable(&worker_id).await?;
    drop(executor);
    http_server.abort();

    check!(status2.status == WorkerStatus::Interrupted);
    check!(status4.status == WorkerStatus::Idle);
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout(120_000)]
async fn long_running_poll_loop_worker_can_be_deleted_after_interrupt(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
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
        .component(&context.default_environment_id, "http-client-2")
        .store()
        .await?;
    let mut env = HashMap::new();
    env.insert("PORT".to_string(), host_http_port.to_string());

    let worker_id = executor
        .start_worker_with(&component.id, "poll-loop-component-5", env, vec![])
        .await?;

    let (rx, _abort_capture) = executor.capture_output_with_termination(&worker_id).await?;

    executor
        .invoke(
            &worker_id,
            "golem:it/api.{start-polling}",
            vec!["first".into_value_and_type()],
        )
        .await??;

    executor
        .wait_for_status(&worker_id, WorkerStatus::Running, Duration::from_secs(10))
        .await?;

    executor.interrupt(&worker_id).await?;

    drain_connection(rx).await;

    executor.check_oplog_is_queryable(&worker_id).await?;
    executor.delete_worker(&worker_id).await?;
    let metadata = executor.get_worker_metadata(&worker_id).await;

    drop(executor);
    http_server.abort();

    check!(metadata.is_err());
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout(120_000)]
async fn shopping_cart_resource_example(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component(&context.default_environment_id, "shopping-cart-resource")
        .store()
        .await?;
    let worker_id = executor
        .start_worker(&component.id, "shopping-cart-resource-1")
        .await?;

    let cart = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{[constructor]cart}",
            vec!["test-user-1".into_value_and_type()],
        )
        .await??;

    let _ = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{[method]cart.add-item}",
            vec![
                ValueAndType {
                    value: cart[0].clone(),
                    typ: analysed_type::u64(),
                },
                Record(vec![
                    ("product-id", "G1000".into_value_and_type()),
                    ("name", "Golem T-Shirt M".into_value_and_type()),
                    ("price", 100.0f32.into_value_and_type()),
                    ("quantity", 5u32.into_value_and_type()),
                ])
                .into_value_and_type(),
            ],
        )
        .await??;

    let _ = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{[method]cart.add-item}",
            vec![
                ValueAndType {
                    value: cart[0].clone(),
                    typ: analysed_type::u64(),
                },
                Record(vec![
                    ("product-id", "G1001".into_value_and_type()),
                    ("name", "Golem Cloud Subscription 1y".into_value_and_type()),
                    ("price", 999999.0f32.into_value_and_type()),
                    ("quantity", 1u32.into_value_and_type()),
                ])
                .into_value_and_type(),
            ],
        )
        .await?;

    let _ = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{[method]cart.add-item}",
            vec![
                ValueAndType {
                    value: cart[0].clone(),
                    typ: analysed_type::u64(),
                },
                Record(vec![
                    ("product-id", "G1002".into_value_and_type()),
                    ("name", "Mud Golem".into_value_and_type()),
                    ("price", 11.0f32.into_value_and_type()),
                    ("quantity", 10u32.into_value_and_type()),
                ])
                .into_value_and_type(),
            ],
        )
        .await?;

    let _ = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{[method]cart.update-item-quantity}",
            vec![
                ValueAndType {
                    value: cart[0].clone(),
                    typ: analysed_type::u64(),
                },
                "G1002".into_value_and_type(),
                20u32.into_value_and_type(),
            ],
        )
        .await?;

    let contents = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{[method]cart.get-cart-contents}",
            vec![ValueAndType {
                value: cart[0].clone(),
                typ: analysed_type::u64(),
            }],
        )
        .await?;

    let _ = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{[method]cart.checkout}",
            vec![ValueAndType {
                value: cart[0].clone(),
                typ: analysed_type::u64(),
            }],
        )
        .await?;

    assert_eq!(
        contents,
        Ok(vec![Value::List(vec![
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

    executor.check_oplog_is_queryable(&worker_id).await?;
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout(120_000)]
async fn counter_resource_test_1(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component(&context.default_environment_id, "counters")
        .store()
        .await?;
    let worker_id = executor.start_worker(&component.id, "counters-1").await?;
    executor.log_output(&worker_id).await?;

    let counter1 = executor
        .invoke_and_await(
            &worker_id,
            "rpc:counters-exports/api.{[constructor]counter}",
            vec!["counter1".into_value_and_type()],
        )
        .await??;

    let _ = executor
        .invoke_and_await(
            &worker_id,
            "rpc:counters-exports/api.{[method]counter.inc-by}",
            vec![
                ValueAndType {
                    value: counter1[0].clone(),
                    typ: analysed_type::u64(),
                },
                5u64.into_value_and_type(),
            ],
        )
        .await?;

    let result1 = executor
        .invoke_and_await(
            &worker_id,
            "rpc:counters-exports/api.{[method]counter.get-value}",
            vec![ValueAndType {
                value: counter1[0].clone(),
                typ: analysed_type::u64(),
            }],
        )
        .await?;

    let metadata1 = executor.get_worker_metadata(&worker_id).await?;

    executor
        .invoke_and_await(
            &worker_id,
            "rpc:counters-exports/api.{[drop]counter}",
            vec![ValueAndType {
                value: counter1[0].clone(),
                typ: analysed_type::u64(),
            }],
        )
        .await??;

    let result2 = executor
        .invoke_and_await(
            &worker_id,
            "rpc:counters-exports/api.{get-all-dropped}",
            vec![],
        )
        .await??;

    let metadata2 = executor.get_worker_metadata(&worker_id).await?;

    check!(result1 == Ok(vec![Value::U64(5)]));

    check!(
        result2
            == vec![Value::List(vec![Value::Tuple(vec![
                Value::String("counter1".to_string()),
                Value::U64(5)
            ])])]
    );

    let ts = Timestamp::now_utc();
    let mut resources1 = metadata1
        .exported_resource_instances
        .iter()
        .map(|erm| ExportedResourceMetadata {
            key: erm.key,
            description: WorkerResourceDescription {
                created_at: ts,
                ..erm.description.clone()
            },
        })
        .collect::<Vec<_>>();
    resources1.sort_by_key(|erm| erm.key);
    check!(
        resources1
            == vec![ExportedResourceMetadata {
                key: WorkerResourceId(0),
                description: WorkerResourceDescription {
                    created_at: ts,
                    resource_owner: "rpc:counters-exports/api".to_string(),
                    resource_name: "counter".to_string()
                }
            }]
    );

    let resources2 = metadata2
        .exported_resource_instances
        .iter()
        .map(|erm| ExportedResourceMetadata {
            key: erm.key,
            description: WorkerResourceDescription {
                created_at: ts,
                ..erm.description.clone()
            },
        })
        .collect::<Vec<_>>();
    check!(resources2 == vec![]);

    executor.check_oplog_is_queryable(&worker_id).await?;
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout(120_000)]
async fn reconstruct_interrupted_state(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component(&context.default_environment_id, "interruption")
        .store()
        .await?;
    let worker_id = executor
        .start_worker(&component.id, "interruption-1")
        .await?;

    let executor_clone = executor.clone();
    let worker_id_clone = worker_id.clone();
    let fiber = tokio::spawn(
        async move {
            executor_clone
                .invoke_and_await(&worker_id_clone, "run", vec![])
                .await
        }
        .in_current_span(),
    );

    executor
        .wait_for_status(&worker_id, WorkerStatus::Running, Duration::from_secs(10))
        .await?;

    executor.interrupt(&worker_id).await?;
    let result = fiber.await??;

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

    check!(result.is_err());
    check!(worker_error_message(&result.err().unwrap()).contains("Interrupted via the Golem API"));
    check!(status == WorkerStatus::Interrupted);

    executor.check_oplog_is_queryable(&worker_id).await?;
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout(120_000)]
async fn invocation_queue_is_persistent(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
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
        .component(&context.default_environment_id, "http-client-2")
        .store()
        .await?;
    let mut env = HashMap::new();
    env.insert("PORT".to_string(), host_http_port.to_string());

    let worker_id = executor
        .start_worker_with(&component.id, "invocation-queue-is-persistent", env, vec![])
        .await?;

    executor.log_output(&worker_id).await?;

    executor
        .invoke(
            &worker_id,
            "golem:it/api.{start-polling}",
            vec!["done".into_value_and_type()],
        )
        .await??;

    executor
        .wait_for_status(&worker_id, WorkerStatus::Running, Duration::from_secs(10))
        .await?;

    executor
        .invoke(&worker_id, "golem:it/api.{increment}", vec![])
        .await??;

    executor
        .invoke(&worker_id, "golem:it/api.{increment}", vec![])
        .await??;

    executor
        .invoke(&worker_id, "golem:it/api.{increment}", vec![])
        .await??;

    executor.interrupt(&worker_id).await?;

    executor
        .wait_for_status(
            &worker_id,
            WorkerStatus::Interrupted,
            Duration::from_secs(5),
        )
        .await?;

    executor.check_oplog_is_queryable(&worker_id).await?;

    drop(executor);
    let executor = start(deps, &context).await?;

    executor
        .invoke(&worker_id, "golem:it/api.{increment}", vec![])
        .await??;

    // executor.log_output(&worker_id).await?;

    executor
        .wait_for_status(&worker_id, WorkerStatus::Running, Duration::from_secs(10))
        .await?;
    {
        let mut response = response.lock().unwrap();
        *response = "done".to_string();
    }

    let result = executor
        .invoke_and_await(&worker_id, "golem:it/api.{get-count}", vec![])
        .await??;

    http_server.abort();

    check!(result == vec![Value::U64(4)]);

    executor.check_oplog_is_queryable(&worker_id).await?;
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout(120_000)]
async fn invoke_with_non_existing_function(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component(&context.default_environment_id, "option-service")
        .store()
        .await?;
    let worker_id = executor
        .start_worker(&component.id, "invoke_with_non_existing_function")
        .await?;

    // First we invoke a function that does not exist and expect a failure
    let failure = executor
        .invoke_and_await(&worker_id, "WRONG", vec![])
        .await?;

    // Then we invoke an existing function, to prove the worker should not be in failed state
    let success = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{echo}",
            vec![Some("Hello").into_value_and_type()],
        )
        .await?;

    check!(failure.is_err());
    check!(
        success
            == Ok(vec![Value::Option(Some(Box::new(Value::String(
                "Hello".to_string()
            ))))])
    );

    executor.check_oplog_is_queryable(&worker_id).await?;
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout(120_000)]
async fn invoke_with_wrong_parameters(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component(&context.default_environment_id, "option-service")
        .store()
        .await?;
    let worker_id = executor
        .start_worker(&component.id, "invoke_with_non_existing_function")
        .await?;

    // First we invoke an existing function with wrong parameters
    let failure = executor
        .invoke_and_await(&worker_id, "golem:it/api.{echo}", vec![])
        .await?;

    // Then we invoke an existing function, to prove the worker should not be in failed state
    let success = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{echo}",
            vec![Some("Hello").into_value_and_type()],
        )
        .await?;

    check!(failure.is_err());
    check!(
        success
            == Ok(vec![Value::Option(Some(Box::new(Value::String(
                "Hello".to_string()
            ))))])
    );

    executor.check_oplog_is_queryable(&worker_id).await?;
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout(120_000)]
async fn stderr_returned_for_failed_component(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component(&context.default_environment_id, "failing-component")
        .store()
        .await?;
    let worker_id = executor
        .start_worker(&component.id, "failing-worker-1")
        .await?;

    let result1 = executor
        .invoke_and_await(
            &worker_id,
            "golem:component/api.{add}",
            vec![5u64.into_value_and_type()],
        )
        .await?;

    let result2 = executor
        .invoke_and_await(
            &worker_id,
            "golem:component/api.{add}",
            vec![50u64.into_value_and_type()],
        )
        .await?;

    executor.check_oplog_is_queryable(&worker_id).await?;

    let result3 = executor
        .invoke_and_await(&worker_id, "golem:component/api.{get}", vec![])
        .await?;

    let metadata = executor.get_worker_metadata(&worker_id).await?;

    let (next, all) = executor
        .get_workers_metadata(&component.id, None, ScanCursor::default(), 100, true)
        .await?;

    info!(
        "result2: {:?}",
        worker_error_message(&result2.clone().err().unwrap())
    );
    info!(
        "result3: {:?}",
        worker_error_message(&result3.clone().err().unwrap())
    );

    check!(result1.is_ok());
    check!(result2.is_err());
    check!(result3.is_err());

    let expected_stderr = "error log message\n\nthread '<unnamed>' (1) panicked at src/lib.rs:31:17:\nvalue is too large\nnote: run with `RUST_BACKTRACE=1` environment variable to display a backtrace\n";

    check!(worker_error_logs(&result2.clone().err().unwrap())
        .unwrap()
        .ends_with(&expected_stderr));
    check!(worker_error_logs(&result3.clone().err().unwrap())
        .unwrap()
        .ends_with(&expected_stderr));

    check!(metadata.status == WorkerStatus::Failed);
    check!(metadata.last_error.is_some());
    check!(metadata.last_error.unwrap().ends_with(&expected_stderr));

    check!(next.is_none());
    check!(all.len() == 1);
    check!(all[0].last_error.is_some());
    check!(all[0]
        .last_error
        .clone()
        .unwrap()
        .ends_with(&expected_stderr));

    executor.check_oplog_is_queryable(&worker_id).await?;
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout(120_000)]
async fn cancelling_pending_invocations(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component(&context.default_environment_id, "counters")
        .store()
        .await?;
    let worker_id = executor
        .start_worker(&component.id, "cancel-pending-invocations")
        .await?;

    let ik1 = IdempotencyKey::fresh();
    let ik2 = IdempotencyKey::fresh();
    let ik3 = IdempotencyKey::fresh();
    let ik4 = IdempotencyKey::fresh();

    let counter1 = executor
        .invoke_and_await(
            &worker_id,
            "rpc:counters-exports/api.{[constructor]counter}",
            vec!["counter1".into_value_and_type()],
        )
        .await??;

    let counter_handle_type = AnalysedType::Handle(TypeHandle {
        name: None,
        owner: None,
        resource_id: AnalysedResourceId(0),
        mode: AnalysedResourceMode::Borrowed,
    });
    let counter_ref = ValueAndType::new(counter1[0].clone(), counter_handle_type);

    let _ = executor
        .invoke_and_await_with_key(
            &worker_id,
            &ik1,
            "rpc:counters-exports/api.{[method]counter.inc-by}",
            vec![counter_ref.clone(), 5u64.into_value_and_type()],
        )
        .await??;

    let promise_id = executor
        .invoke_and_await(
            &worker_id,
            "rpc:counters-exports/api.{[method]counter.create-promise}",
            vec![counter_ref.clone()],
        )
        .await??;

    executor
        .invoke(
            &worker_id,
            "rpc:counters-exports/api.{[method]counter.block-on-promise}",
            vec![
                counter_ref.clone(),
                ValueAndType {
                    value: promise_id[0].clone(),
                    typ: PromiseId::get_type(),
                },
            ],
        )
        .await??;

    executor
        .invoke_with_key(
            &worker_id,
            &ik2,
            "rpc:counters-exports/api.{[method]counter.inc-by}",
            vec![counter_ref.clone(), 6u64.into_value_and_type()],
        )
        .await??;

    executor
        .invoke_with_key(
            &worker_id,
            &ik3,
            "rpc:counters-exports/api.{[method]counter.inc-by}",
            vec![counter_ref.clone(), 7u64.into_value_and_type()],
        )
        .await??;

    let cancel1 = executor.cancel_invocation(&worker_id, &ik1).await;
    let cancel2 = executor.cancel_invocation(&worker_id, &ik2).await;
    let cancel4 = executor.cancel_invocation(&worker_id, &ik4).await;

    let Value::Record(fields) = &promise_id[0] else {
        panic!("Expected a record")
    };
    let Value::U64(oplog_idx) = fields[1] else {
        panic!("Expected a u64")
    };

    executor
        .complete_promise(
            &PromiseId {
                worker_id: worker_id.clone(),
                oplog_idx: OplogIndex::from_u64(oplog_idx),
            },
            vec![42],
        )
        .await?;

    let final_result = executor
        .invoke_and_await(
            &worker_id,
            "rpc:counters-exports/api.{[method]counter.get-value}",
            vec![counter_ref.clone()],
        )
        .await??;

    executor.check_oplog_is_queryable(&worker_id).await?;

    executor.check_oplog_is_queryable(&worker_id).await?;

    check!(cancel1.is_ok() && !cancel1.unwrap()); // cannot cancel a completed invocation
    check!(cancel2.is_ok() && cancel2.unwrap());
    check!(cancel4.is_err()); // cannot cancel a non-existing invocation
    assert_eq!(final_result, vec![Value::U64(12)]);
    Ok(())
}

/// Test resolving a component_id from the name.
#[test]
#[tracing::instrument]
#[timeout(120_000)]
async fn resolve_components_from_name(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    // Make sure the name is unique
    let counter_component = executor
        .component(&context.default_environment_id, "counters")
        .name("component-resolve-target")
        .store()
        .await?;

    let resolver_component = executor
        .component(&context.default_environment_id, "component-resolve")
        .store()
        .await?;

    executor
        .start_worker(&counter_component.id, "counter-1")
        .await?;

    let resolve_worker = executor
        .start_worker(&resolver_component.id, "resolver-1")
        .await?;

    let result = executor
        .invoke_and_await(
            &resolve_worker,
            "golem:it/component-resolve-api.{run}",
            vec![],
        )
        .await??;

    check!(result.len() == 1);

    let (high_bits, low_bits) = counter_component.id.0.as_u64_pair();
    let component_id_value = Value::Record(vec![Value::Record(vec![
        Value::U64(high_bits),
        Value::U64(low_bits),
    ])]);

    let worker_id_value = Value::Record(vec![
        component_id_value.clone(),
        Value::String("counter-1".to_string()),
    ]);

    check!(
        result[0]
            == Value::Tuple(vec![
                Value::Option(Some(Box::new(component_id_value))),
                Value::Option(Some(Box::new(worker_id_value))),
                Value::Option(None),
            ])
    );

    executor.check_oplog_is_queryable(&resolve_worker).await?;
    Ok(())
}

#[tracing::instrument]
async fn scheduled_invocation_test(
    server_component_name: &str,
    client_component_name: &str,
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let server_component = executor
        .component(&context.default_environment_id, server_component_name)
        .store()
        .await?;

    let server_worker = executor
        .start_worker(&server_component.id, "worker_1")
        .await?;

    let client_component = executor
        .component(&context.default_environment_id, client_component_name)
        .with_dynamic_linking(&[
            (
                "it:scheduled-invocation-server-client/server-client",
                DynamicLinkedInstance::WasmRpc(DynamicLinkedWasmRpc {
                    targets: HashMap::from_iter(vec![(
                        "server-api".to_string(),
                        WasmRpcTarget {
                            interface_name: "it:scheduled-invocation-server-exports/server-api"
                                .to_string(),
                            component_name: server_component_name.to_string(),
                        },
                    )]),
                }),
            ),
            (
                "it:scheduled-invocation-client-client/client-client",
                DynamicLinkedInstance::WasmRpc(DynamicLinkedWasmRpc {
                    targets: HashMap::from_iter(vec![(
                        "client-api".to_string(),
                        WasmRpcTarget {
                            interface_name: "it:scheduled-invocation-client-exports/client-api"
                                .to_string(),
                            component_name: server_component_name.to_string(),
                        },
                    )]),
                }),
            ),
        ])
        .store()
        .await?;

    let client_worker = executor
        .start_worker(&client_component.id, "worker_1")
        .await?;

    // first invocation: schedule increment in the future and poll
    {
        executor
            .invoke_and_await(
                &client_worker,
                "it:scheduled-invocation-client-exports/client-api.{test1}",
                vec![
                    ValueAndType::new(
                        Value::String(server_component_name.to_string()),
                        AnalysedType::Str(TypeStr),
                    ),
                    ValueAndType::new(
                        Value::String("worker_1".to_string()),
                        AnalysedType::Str(TypeStr),
                    ),
                ],
            )
            .await??;

        let mut done = false;
        while !done {
            let result = executor
                .invoke_and_await(
                    &server_worker,
                    "it:scheduled-invocation-server-exports/server-api.{get-global-value}",
                    vec![],
                )
                .await??;

            if result.len() == 1 && result[0] == Value::U64(1) {
                done = true;
            } else {
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }
    }

    // second invocation: schedule increment in the future and cancel beforehand
    {
        executor
            .invoke_and_await(
                &client_worker,
                "it:scheduled-invocation-client-exports/client-api.{test2}",
                vec![
                    ValueAndType::new(
                        Value::String(server_component_name.to_string()),
                        AnalysedType::Str(TypeStr),
                    ),
                    ValueAndType::new(
                        Value::String("worker_1".to_string()),
                        AnalysedType::Str(TypeStr),
                    ),
                ],
            )
            .await??;

        tokio::time::sleep(Duration::from_millis(300)).await;

        let result = executor
            .invoke_and_await(
                &server_worker,
                "it:scheduled-invocation-server-exports/server-api.{get-global-value}",
                vec![],
            )
            .await??;

        assert!(matches!(result.as_slice(), [Value::U64(1)]));
    }

    // third invocation: schedule increment on self in the future and poll
    {
        executor
            .invoke_and_await(
                &client_worker,
                "it:scheduled-invocation-client-exports/client-api.{test3}",
                vec![],
            )
            .await??;

        let mut done = false;
        while !done {
            let result = executor
                .invoke_and_await(
                    &client_worker,
                    "it:scheduled-invocation-client-exports/client-api.{get-global-value}",
                    vec![],
                )
                .await??;

            if result.len() == 1 && result[0] == Value::U64(1) {
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

#[test_gen]
async fn gen_scheduled_invocation_tests(r: &mut DynamicTestRegistration) {
    add_test!(
        r,
        "scheduled_invocation_stubbed",
        TestProperties {
            timeout: Some(Duration::from_secs(120)),
            ..Default::default()
        },
        move |last_unique_id: &LastUniqueId,
              deps: &WorkerExecutorTestDependencies,
              tracing: &Tracing| async {
            scheduled_invocation_test(
                "it_scheduled_invocation_server",
                "it_scheduled_invocation_client",
                last_unique_id,
                deps,
                tracing,
            )
            .await
        }
    );
    add_test!(
        r,
        "scheduled_invocation_stubless",
        TestProperties {
            timeout: Some(Duration::from_secs(120)),
            ..Default::default()
        },
        move |last_unique_id: &LastUniqueId,
              deps: &WorkerExecutorTestDependencies,
              tracing: &Tracing| async {
            scheduled_invocation_test(
                "it_scheduled_invocation_server_stubless",
                "it_scheduled_invocation_client_stubless",
                last_unique_id,
                deps,
                tracing,
            )
            .await
        }
    );
}

#[test]
#[tracing::instrument]
#[timeout(120_000)]
async fn error_handling_when_worker_is_invoked_with_wrong_parameter_type(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component(&context.default_environment_id, "option-service")
        .store()
        .await?;
    let worker_id = executor
        .start_worker(&component.id, "wrong-parameter-type-1")
        .await?;

    let failure = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{echo}",
            vec![100u64.into_value_and_type()],
        )
        .await?;

    let success = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{echo}",
            vec![Some("x").into_value_and_type()],
        )
        .await?;

    // TODO: the parameter type mismatch causes printing to fail due to a corrupted WasmValue.
    // executor.check_oplog_is_queryable(&worker_id).await;
    drop(executor);

    check!(failure.is_err());
    check!(success.is_ok());
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout(120_000)]
async fn delete_worker_during_invocation(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component(&context.default_environment_id, "clock-service")
        .store()
        .await?;
    let worker_id = executor
        .start_worker(&component.id, "delete-worker-during-invocation")
        .await?;

    info!("Enqueuing invocations");
    // Enqueuing a large number of invocations, each sleeping for 2 seconds
    for _ in 0..25 {
        executor
            .invoke(
                &worker_id,
                "golem:it/api.{sleep}",
                vec![2u64.into_value_and_type()],
            )
            .await??;
    }

    executor
        .wait_for_status(&worker_id, WorkerStatus::Running, Duration::from_secs(2))
        .await?;

    info!("Deleting the worker");
    executor.delete_worker(&worker_id).await?;

    info!("Invoking again");
    // Invoke it one more time - it should create a new instance and return successfully
    let result = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{sleep}",
            vec![1u64.into_value_and_type()],
        )
        .await?;

    let metadata = executor.get_worker_metadata(&worker_id).await?;

    executor.check_oplog_is_queryable(&worker_id).await?;

    check!(result == Ok(vec![Value::Result(Ok(None))]));
    check!(metadata.pending_invocation_count == 0);
    Ok(())
}

#[test]
#[tracing::instrument]
#[test_r::non_flaky(10)]
async fn invoking_worker_while_its_getting_deleted_works(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component(&context.default_environment_id, "counters")
        .unique()
        .store()
        .await?;
    let worker_id = executor.start_worker(&component.id, "worker").await?;

    let invoking_task = {
        let executor = executor.clone();
        let worker_id = worker_id.clone();
        tokio::spawn(async move {
            let mut result = None;
            while matches!(result, Some(Ok(Ok(_))) | None) {
                result = Some(
                    executor
                        .invoke_and_await(
                            &worker_id,
                            "rpc:counters-exports/api.{inc-global-by}",
                            vec![1u64.into_value_and_type()],
                        )
                        .await,
                );
            }
            result
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
                    _ = deleting_task_cancel_token.cancelled() => { break },
                    _ = executor.delete_worker(&worker_id) => { }
                }
            }
        })
    };

    let invocation_result = invoking_task.await?.unwrap()?;
    deleting_task_cancel_token.cancel();
    {
        // We tried invoking the worker while it was being deleted, we expect an invalid request
        let_assert!(Err(WorkerExecutorError::InvalidRequest { details }) = invocation_result);
        assert!(details.contains("being deleted"));
    };

    Ok(())
}
