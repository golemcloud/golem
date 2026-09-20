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
use async_trait::async_trait;
use axum::Router;
use axum::routing::post;
use golem_api_grpc::invocation_session_protocol::InvocationSessionState;
use golem_api_grpc::proto::golem::schema::{RecordValue, SchemaValueStreamReference, schema_value};
use golem_api_grpc::proto::golem::worker::{
    InputStreamEnd, InputStreamItem, InvocationAccepted, InvocationFailureKind, InvocationRequest,
    InvocationResponse, InvocationStart, ResumeAttach, ResumeOperation, StreamCancel,
    StreamCancelReason, StreamCancelRole, StreamCursor, StreamInvocationIdentity,
    input_stream_item, invocation_request, invocation_response, invocation_session_completion,
    invocation_session_result,
};
use golem_common::model::account::AccountId;
use golem_common::model::agent::{AgentPrincipal, ParsedAgentId, Principal};
use golem_common::model::card::{AgentResourcePattern, AgentVerb};
use golem_common::model::component::ComponentDto;
use golem_common::model::durable_stream::{
    StreamAttachmentFinalizationReason, StreamCancelRole as DurableStreamCancelRole,
    StreamItemsRecord, StreamSessionRecord,
};
use golem_common::model::oplog::payload::HostRequestGolemRpcInvoke;
use golem_common::model::oplog::{
    OplogIndex, PublicAgentInvocation, PublicOplogEntry, PublicOplogEntryWithIndex,
};
use golem_common::model::{AgentId, AgentStatus, IdempotencyKey, OwnedAgentId, PromiseId};
use golem_common::schema::schema_value::ResultValuePayload;
use golem_common::schema::{FromSchema, SchemaValue, TypedSchemaValue};
use golem_common::{agent_id, data_value};
use golem_service_base::model::auth::AuthCtx;
use golem_test_framework::dsl::{AgentResult, TestDsl};
use golem_worker_executor::services::direct_invocation_auth::{
    DirectInvocationAuthService, EnvironmentOwnerAccountId,
};
use golem_worker_executor::services::rpc::RpcError;
use golem_worker_executor::worker::EvictionClass;
use golem_worker_executor_test_utils::{
    FireAndForgetRpcCheckpoint, LastUniqueId, PrecompiledComponent, RecordingRpc, TestContext,
    TestExecutorOverrides, TestWorkerExecutor, WorkerExecutorTestDependencies, start,
    start_with_concurrent_agent_limit_and_overrides, start_with_overrides,
};
use pretty_assertions::assert_eq;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use test_r::{inherit_test_dep, test, timeout};
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tracing::Instrument;

inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(LastUniqueId);
inherit_test_dep!(
    #[tagged_as("agent_rpc_rust")]
    PrecompiledComponent
);
inherit_test_dep!(
    #[tagged_as("agent_rpc")]
    PrecompiledComponent
);
inherit_test_dep!(
    #[tagged_as("agent_counters")]
    PrecompiledComponent
);
inherit_test_dep!(
    #[tagged_as("host_api_tests")]
    PrecompiledComponent
);
inherit_test_dep!(
    #[tagged_as("large_dynamic_memory")]
    PrecompiledComponent
);
inherit_test_dep!(Tracing);

#[test]
#[timeout("2 minutes")]
async fn memory_growth_before_rpc_activation_control(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] component: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    memory_growth_before_rpc_activation(last_unique_id, deps, component, false).await
}

#[test]
#[timeout("2 minutes")]
async fn memory_growth_before_rpc_activation_recovers_after_oom(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] component: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    memory_growth_before_rpc_activation(last_unique_id, deps, component, true).await
}

async fn memory_growth_before_rpc_activation(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    fixture: &PrecompiledComponent,
    inject_oom: bool,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;
    let component = executor
        .component_dep(&context.default_environment_id, fixture)
        .store()
        .await?;
    let caller = agent_id!("CancelTester", "memory-growth");
    let worker = executor.start_agent(&component.id, caller.clone()).await?;
    if inject_oom {
        executor.fail_memory_growth_after_rpc_creation(&worker);
    }
    let key = IdempotencyKey::fresh();
    let result = executor
        .invoke_and_await_agent_with_key(
            &component,
            &caller,
            &key,
            "grow_memory_before_rpc_activation",
            data_value!("memory-growth-target"),
        )
        .await;
    let triggered = executor.rpc_memory_failure_triggered(&worker);
    let oplog = executor.get_oplog(&worker, OplogIndex::INITIAL).await?;
    let boundary_matches = oplog.windows(3).any(|entries| {
        matches!(&entries[0].entry, PublicOplogEntry::Start(start)
            if start.function_name == "golem::rpc::wasm-rpc::new")
            && matches!(&entries[1].entry, PublicOplogEntry::End(end)
                if end.start_index == entries[0].oplog_index)
            && matches!(&entries[2].entry, PublicOplogEntry::Error(error)
                if error.error == "Out of memory")
    });
    eprintln!("Before restart: result={result:?}, injected={triggered}");
    print_rpc_memory_oplog(&oplog);
    drop(executor);

    let executor = start(deps, &context).await?;
    let restarted = executor
        .invoke_and_await_agent_with_key(
            &component,
            &caller,
            &key,
            "grow_memory_before_rpc_activation",
            data_value!("memory-growth-target"),
        )
        .await;
    let oplog = executor.get_oplog(&worker, OplogIndex::INITIAL).await?;
    eprintln!("After restart: result={restarted:?}");
    print_rpc_memory_oplog(&oplog);
    assert_eq!(triggered, inject_oom);
    if inject_oom {
        assert!(
            boundary_matches,
            "OOM did not immediately follow RPC creation"
        );
    }
    result?;
    restarted?;
    let count = executor
        .invoke_and_await_agent(
            &component,
            &agent_id!("RpcCounter", "memory-growth-target"),
            "get_value",
            data_value!(),
        )
        .await?
        .into_typed::<u64>()?;
    assert_eq!(count, 1);
    Ok(())
}

#[derive(Clone, Copy)]
enum RpcMemoryTestBoundary {
    Completion,
    Delivery,
}

#[test]
#[timeout("2 minutes")]
async fn large_rpc_result_delivery_control(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] component: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    rpc_memory_boundary(
        last_unique_id,
        deps,
        component,
        "receive_large_rpc_result",
        RpcMemoryTestBoundary::Completion,
        false,
        Some(4 * 1024 * 1024),
    )
    .await
}

#[test]
#[timeout("2 minutes")]
async fn large_rpc_result_delivery_recovers_after_oom(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] component: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    rpc_memory_boundary(
        last_unique_id,
        deps,
        component,
        "receive_large_rpc_result",
        RpcMemoryTestBoundary::Completion,
        true,
        Some(4 * 1024 * 1024),
    )
    .await
}

#[test]
#[timeout("2 minutes")]
async fn rpc_span_finish_control(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] component: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    rpc_memory_boundary(
        last_unique_id,
        deps,
        component,
        "grow_memory_after_rpc_result",
        RpcMemoryTestBoundary::Delivery,
        false,
        None,
    )
    .await
}

#[test]
#[timeout("2 minutes")]
async fn rpc_span_finish_recovers_after_oom(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] component: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    rpc_memory_boundary(
        last_unique_id,
        deps,
        component,
        "grow_memory_after_rpc_result",
        RpcMemoryTestBoundary::Delivery,
        true,
        None,
    )
    .await
}

async fn rpc_memory_boundary(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    fixture: &PrecompiledComponent,
    method: &str,
    boundary: RpcMemoryTestBoundary,
    inject_oom: bool,
    expected: Option<u64>,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;
    let component = executor
        .component_dep(&context.default_environment_id, fixture)
        .store()
        .await?;
    let caller = agent_id!("CancelTester", method);
    let target_name = format!("{method}-target");
    let worker = executor.start_agent(&component.id, caller.clone()).await?;
    if inject_oom {
        match boundary {
            RpcMemoryTestBoundary::Completion => {
                executor.fail_memory_growth_after_rpc_completion(&worker)
            }
            RpcMemoryTestBoundary::Delivery => {
                executor.fail_memory_growth_after_rpc_delivery(&worker)
            }
        }
    }
    let key = IdempotencyKey::fresh();
    let result = executor
        .invoke_and_await_agent_with_key(
            &component,
            &caller,
            &key,
            method,
            data_value!(target_name.clone()),
        )
        .await;
    let triggered = executor.rpc_memory_failure_triggered(&worker);
    let oplog = executor.get_oplog(&worker, OplogIndex::INITIAL).await?;
    eprintln!("Before restart: result={result:?}, injected={triggered}");
    print_rpc_memory_oplog(&oplog);
    if inject_oom {
        assert!(rpc_memory_boundary_matches(&oplog, boundary));
    }
    assert_eq!(triggered, inject_oom);
    check_rpc_memory_result(result?, expected)?;
    drop(executor);

    let executor = start(deps, &context).await?;
    let restarted = executor
        .invoke_and_await_agent_with_key(
            &component,
            &caller,
            &key,
            method,
            data_value!(target_name.clone()),
        )
        .await?;
    check_rpc_memory_result(restarted, expected)?;
    let count = executor
        .invoke_and_await_agent(
            &component,
            &agent_id!("RpcCounter", target_name),
            "get_value",
            data_value!(),
        )
        .await?
        .into_typed::<u64>()?;
    assert_eq!(count, 1);
    Ok(())
}

fn check_rpc_memory_result(result: AgentResult, expected: Option<u64>) -> anyhow::Result<()> {
    if let Some(expected) = expected {
        assert_eq!(result.into_typed::<u64>()?, expected);
    }
    Ok(())
}

fn rpc_memory_boundary_matches(
    oplog: &[PublicOplogEntryWithIndex],
    boundary: RpcMemoryTestBoundary,
) -> bool {
    let rpc_start = oplog.iter().find_map(|entry| match &entry.entry {
        PublicOplogEntry::Start(start)
            if start.function_name == "golem::rpc::wasm-rpc::invoke_and_await" =>
        {
            Some(entry.oplog_index)
        }
        _ => None,
    });
    match boundary {
        RpcMemoryTestBoundary::Completion => oplog.windows(3).any(|entries| {
            matches!(&entries[0].entry, PublicOplogEntry::End(end)
                if Some(end.start_index) == rpc_start)
                && matches!(&entries[1].entry, PublicOplogEntry::FinishSpan(_))
                && matches!(&entries[2].entry, PublicOplogEntry::Error(error)
                    if error.error == "Out of memory")
        }),
        RpcMemoryTestBoundary::Delivery => oplog.windows(2).any(|entries| {
            matches!(&entries[0].entry, PublicOplogEntry::CompletionDelivered(delivered)
                if Some(delivered.start_index) == rpc_start)
                && matches!(&entries[1].entry, PublicOplogEntry::Error(error)
                    if error.error == "Out of memory")
        }),
    }
}

fn print_rpc_memory_oplog(oplog: &[PublicOplogEntryWithIndex]) {
    for entry in oplog {
        let description = match &entry.entry {
            PublicOplogEntry::Start(start) => format!("Start {}", start.function_name),
            PublicOplogEntry::End(end) => format!("End {:?}", end.start_index),
            PublicOplogEntry::Error(error) => format!("Error {}", error.error),
            _ => continue,
        };
        eprintln!("{:?}: {description}", entry.oplog_index);
    }
}

#[test]
#[timeout("8 minutes")]
async fn durable_operations_after_rpc_delivery_recover_at_live_tail(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] component: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    for operation in [
        "kv-get",
        "config-get",
        "fire-and-forget-rpc",
        "async-rpc",
        "span",
        "atomic",
        "commit",
        "retry-policy",
    ] {
        durable_operation_after_rpc_delivery(last_unique_id, deps, component, operation).await?;
    }
    Ok(())
}

async fn durable_operation_after_rpc_delivery(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    fixture: &PrecompiledComponent,
    operation: &str,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;
    let component = executor
        .component_dep(&context.default_environment_id, fixture)
        .store()
        .await?;
    let caller = agent_id!("CancelTester", format!("tail-{operation}"));
    let target_name = format!("tail-{operation}-target");
    let worker = executor.start_agent(&component.id, caller.clone()).await?;
    executor.fail_memory_growth_after_rpc_delivery(&worker);
    let key = IdempotencyKey::fresh();

    executor
        .invoke_and_await_agent_with_key(
            &component,
            &caller,
            &key,
            "durable_operation_after_rpc_result",
            data_value!(target_name.clone(), operation),
        )
        .await?;
    assert!(
        executor.rpc_memory_failure_triggered(&worker),
        "delivery fault did not trigger for {operation}: {:#?}",
        executor.get_oplog(&worker, OplogIndex::INITIAL).await?
    );
    drop(executor);

    let executor = start(deps, &context).await?;
    executor
        .invoke_and_await_agent_with_key(
            &component,
            &caller,
            &key,
            "durable_operation_after_rpc_result",
            data_value!(target_name.clone(), operation),
        )
        .await?;

    let count = executor
        .invoke_and_await_agent(
            &component,
            &agent_id!("RpcCounter", target_name.clone()),
            "get_value",
            data_value!(),
        )
        .await?
        .into_typed::<u64>()?;
    let expected = if matches!(operation, "fire-and-forget-rpc" | "async-rpc") {
        8
    } else {
        1
    };
    assert_eq!(count, expected, "RPC side effect repeated for {operation}");

    let oplog = executor.get_oplog(&worker, OplogIndex::INITIAL).await?;
    assert_live_tail_operation(&oplog, operation);
    Ok(())
}

fn assert_live_tail_operation(oplog: &[PublicOplogEntryWithIndex], operation: &str) {
    let oom_index = oplog
        .iter()
        .rposition(|entry| {
            matches!(&entry.entry, PublicOplogEntry::Error(error)
            if error.error == "Out of memory")
        })
        .unwrap_or_else(|| panic!("missing injected OOM for {operation}"));
    let live_tail = &oplog[oom_index + 1..];
    let found = live_tail
        .iter()
        .any(|entry| match (operation, &entry.entry) {
            ("kv-get", PublicOplogEntry::Start(start)) => start.function_name.contains("keyvalue"),
            ("config-get", PublicOplogEntry::Start(start)) => {
                start.function_name.contains("config")
            }
            ("fire-and-forget-rpc" | "async-rpc", PublicOplogEntry::Start(start)) => {
                start.function_name.contains("rpc")
            }
            ("span", PublicOplogEntry::StartSpan(_)) => true,
            ("atomic", PublicOplogEntry::BeginAtomicRegion(_)) => true,
            ("commit", PublicOplogEntry::NoOp(_)) => true,
            ("retry-policy", PublicOplogEntry::SetRetryPolicy(_)) => true,
            _ => false,
        });
    assert!(found, "missing live {operation} oplog boundary after OOM");
}

struct DenyDirectInvocationAuth;

#[async_trait]
impl DirectInvocationAuthService for DenyDirectInvocationAuth {
    async fn check(
        &self,
        _caller_account_id: AccountId,
        _owned_agent_id: &OwnedAgentId,
        _verb: AgentVerb,
        _resource: AgentResourcePattern,
        _auth_ctx: &AuthCtx,
    ) -> Result<EnvironmentOwnerAccountId, RpcError> {
        Err(RpcError::Denied {
            details: "direct invocation denied before schema lookup".to_string(),
        })
    }
}

#[test]
#[timeout("60s")]
#[tracing::instrument]
async fn invalid_start_is_rejected_before_acceptance(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;
    let (requests, receiver) = mpsc::channel(1);
    requests
        .send(InvocationRequest {
            request: Some(invocation_request::Request::Start(InvocationStart {
                agent_id: Some(golem_api_grpc::proto::golem::worker::AgentId {
                    component_id: None,
                    name: "agent".to_string(),
                }),
                method_name: Some("run".to_string()),
                input: Some(golem_api_grpc::proto::golem::schema::SchemaValue {
                    value: Some(
                        golem_api_grpc::proto::golem::schema::schema_value::Value::U8Value(1),
                    ),
                }),
                idempotency_key: Some(IdempotencyKey::fresh().into()),
                auth_ctx: None,
                ..Default::default()
            })),
        })
        .await?;

    let mut responses = executor
        .client
        .clone()
        .invoke_agent_session(ReceiverStream::new(receiver))
        .await?
        .into_inner();
    let response = responses
        .message()
        .await?
        .ok_or_else(|| anyhow::anyhow!("invalid invocation start returned no rejection"))?;

    match response.response {
        Some(invocation_response::Response::Rejected(_)) => {}
        other => {
            panic!("invalid invocation start must be rejected before acceptance, got {other:?}")
        }
    }
    assert!(responses.message().await?.is_none());
    Ok(())
}

#[test]
#[timeout("2 minutes")]
#[tracing::instrument]
async fn output_consumer_cancel_after_result_remains_a_valid_terminal_session(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] agent_rpc_rust: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;
    let component = executor
        .component_dep(&context.default_environment_id, agent_rpc_rust)
        .store()
        .await?;
    let agent_id = agent_id!("StreamingRpcTarget", "cancel-sibling-output");
    let worker_agent_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;
    let metadata = executor.get_worker_metadata(&worker_agent_id).await?;
    let (_, input) = data_value!().into_parts();
    let key = Some(IdempotencyKey::fresh().into());
    let start = InvocationRequest {
        request: Some(invocation_request::Request::Start(InvocationStart {
            agent_id: Some(worker_agent_id.into()),
            method_name: Some("produce_siblings".to_string()),
            input: Some(input.try_into().map_err(anyhow::Error::msg)?),
            idempotency_key: key,
            auth_ctx: Some(executor.auth_ctx().into()),
            environment_id: Some(component.environment_id.into()),
            component_owner_account_id: Some(component.account_id.into()),
            mode: golem_api_grpc::proto::golem::worker::AgentInvocationMode::Await as i32,
            freshness_disposition:
                golem_api_grpc::proto::golem::worker::InvocationFreshnessDisposition::MayExist
                    as i32,
            attempt_id: Some(uuid::Uuid::new_v4().into()),
            expected_callee_fingerprint: Some(metadata.fingerprint.0.into()),
            ..Default::default()
        })),
    };
    let mut state = InvocationSessionState::default();
    state
        .validate_trusted_request(&start)
        .map_err(anyhow::Error::msg)?;
    let (requests, receiver) = mpsc::channel(8);
    requests.send(start).await?;
    let mut responses = executor
        .client
        .clone()
        .invoke_agent_session(ReceiverStream::new(receiver))
        .await?
        .into_inner();
    let mut cancellation_sent = false;

    while let Some(response) = responses.message().await? {
        state
            .validate_response(&response)
            .map_err(anyhow::Error::msg)?;
        match response.response {
            Some(invocation_response::Response::Result(result)) => {
                let value = match &result.result {
                    Some(invocation_session_result::Result::MethodResult(value)) => value,
                    other => anyhow::bail!("expected a method result, got {other:?}"),
                };
                let stream_id = match &value.value {
                    Some(schema_value::Value::TupleValue(tuple)) => match tuple.elements.first() {
                        Some(golem_api_grpc::proto::golem::schema::SchemaValue {
                            value: Some(schema_value::Value::StreamReference(reference)),
                        }) => reference.stream_id,
                        other => anyhow::bail!("expected first sibling stream, got {other:?}"),
                    },
                    other => anyhow::bail!("expected sibling tuple result, got {other:?}"),
                };
                let mapping = result
                    .new_stream_mappings
                    .iter()
                    .find(|mapping| mapping.transport_stream_id == stream_id)
                    .ok_or_else(|| anyhow::anyhow!("result omitted the durable output mapping"))?;
                let cancel = InvocationRequest {
                    request: Some(invocation_request::Request::StreamCancel(StreamCancel {
                        transport_stream_id: stream_id,
                        producer_sequence: 0,
                        role: StreamCancelRole::OutputConsumer as i32,
                        reason: StreamCancelReason::Cancelled as i32,
                        details: Some("consumer stopped reading".to_string()),
                        durable_stream_id: mapping
                            .handle
                            .as_ref()
                            .and_then(|handle| handle.stream_id),
                        epoch: 1,
                        durable_offset: Vec::new(),
                    })),
                };
                state
                    .validate_trusted_request(&cancel)
                    .map_err(anyhow::Error::msg)?;
                requests.send(cancel).await?;
                cancellation_sent = true;
            }
            Some(invocation_response::Response::Finished(finished)) => {
                assert!(cancellation_sent, "session finished before cancellation");
                if let Some(invocation_session_completion::Outcome::Failure(failure)) =
                    finished.outcome
                {
                    assert_ne!(
                        failure.kind,
                        InvocationFailureKind::Protocol as i32,
                        "a validator-approved output-consumer cancellation is not a protocol error: {}",
                        failure.message
                    );
                }
                assert!(state.is_complete());
                assert!(responses.message().await?.is_none());
                return Ok(());
            }
            Some(invocation_response::Response::Rejected(rejected)) => {
                anyhow::bail!("invocation rejected: {}", rejected.error)
            }
            _ => {}
        }
    }

    anyhow::bail!("invocation response closed without InvocationFinished")
}

#[test]
#[timeout("2 minutes")]
async fn ephemeral_output_producer_can_be_interrupted_after_result(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] agent_rpc_rust: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    interrupt_output_producer_after_result(last_unique_id, deps, agent_rpc_rust, true, false).await
}

#[test]
#[timeout("2 minutes")]
async fn durable_output_producer_can_be_interrupted_after_result(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] agent_rpc_rust: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    interrupt_output_producer_after_result(last_unique_id, deps, agent_rpc_rust, false, false).await
}

#[test]
#[timeout("2 minutes")]
async fn replayed_output_producer_can_be_interrupted_after_result(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] agent_rpc_rust: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    interrupt_output_producer_after_result(last_unique_id, deps, agent_rpc_rust, false, true).await
}

async fn interrupt_output_producer_after_result(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    agent_rpc_rust: &PrecompiledComponent,
    ephemeral: bool,
    restart_before_input: bool,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;
    let component = executor
        .component_dep(&context.default_environment_id, agent_rpc_rust)
        .store()
        .await?;
    let key = IdempotencyKey::fresh();
    let agent_id = if ephemeral {
        agent_id!("EphemeralStreamingRpcTarget", "interrupt-output")
            .with_ephemeral_invocation_phantom(&key)
            .map_err(anyhow::Error::msg)?
    } else {
        agent_id!("StreamingRpcTarget", "interrupt-output")
    };
    let agent_id = executor.start_agent(&component.id, agent_id).await?;
    let metadata = executor.get_worker_metadata(&agent_id).await?;
    let start = InvocationRequest {
        request: Some(invocation_request::Request::Start(InvocationStart {
            agent_id: Some(agent_id.clone().into()),
            method_name: Some("produce_then_spin".to_string()),
            input: Some(golem_api_grpc::proto::golem::schema::SchemaValue {
                value: Some(schema_value::Value::RecordValue(RecordValue {
                    fields: vec![golem_api_grpc::proto::golem::schema::SchemaValue {
                        value: Some(schema_value::Value::StreamReference(
                            SchemaValueStreamReference { stream_id: 1 },
                        )),
                    }],
                })),
            }),
            idempotency_key: Some(key.clone().into()),
            auth_ctx: Some(executor.auth_ctx().into()),
            environment_id: Some(component.environment_id.into()),
            component_owner_account_id: Some(component.account_id.into()),
            mode: golem_api_grpc::proto::golem::worker::AgentInvocationMode::Await as i32,
            attempt_id: Some(uuid::Uuid::new_v4().into()),
            expected_callee_fingerprint: Some(metadata.fingerprint.0.into()),
            ..Default::default()
        })),
    };
    let mut state = InvocationSessionState::default();
    state
        .validate_trusted_request(&start)
        .map_err(anyhow::Error::msg)?;
    let (mut requests, receiver) = mpsc::channel(8);
    requests.send(start.clone()).await?;
    let mut responses = executor
        .client
        .clone()
        .invoke_agent_session(ReceiverStream::new(receiver))
        .await?
        .into_inner();
    tokio::time::timeout(Duration::from_secs(30), async {
        let mut result_seen = false;
        let mut first_item_seen = false;
        let mut accepted = None;
        let mut restarted = false;
        loop {
            let response = responses
                .message()
                .await?
                .ok_or_else(|| anyhow::anyhow!("producer ended before the spin handshake"))?;
            state
                .validate_response(&response)
                .map_err(anyhow::Error::msg)?;
            let next_input = match response.response {
                Some(invocation_response::Response::Accepted(value)) => {
                    accepted = Some(value);
                    None
                }
                Some(invocation_response::Response::Result(_)) => {
                    let acceptance = accepted.as_ref().expect("acceptance before result");
                    if restart_before_input && !restarted {
                        executor.simulated_crash(&agent_id).await?;
                        let resume = InvocationRequest {
                            request: Some(invocation_request::Request::ResumeAttach(
                                ResumeAttach {
                                    idempotency_key: Some(key.clone().into()),
                                    agent_id: Some(agent_id.clone().into()),
                                    environment_id: Some(component.environment_id.into()),
                                    attachment_id: acceptance.attachment_id,
                                    attempt_id: Some(uuid::Uuid::new_v4().into()),
                                    expected_callee_fingerprint: Some(
                                        metadata.fingerprint.0.into(),
                                    ),
                                    expected_epoch: acceptance.epoch,
                                    operation: ResumeOperation::Resume as i32,
                                    cursors: Vec::new(),
                                    auth_ctx: Some(executor.auth_ctx().into()),
                                    principal: None,
                                },
                            )),
                        };
                        state = InvocationSessionState::default();
                        state
                            .validate_trusted_request(&resume)
                            .map_err(anyhow::Error::msg)?;
                        let (resumed_requests, receiver) = mpsc::channel(8);
                        resumed_requests.send(resume).await?;
                        requests = resumed_requests;
                        responses = executor
                            .client
                            .clone()
                            .invoke_agent_session(ReceiverStream::new(receiver))
                            .await?
                            .into_inner();
                        accepted = None;
                        restarted = true;
                        continue;
                    }
                    result_seen = true;
                    Some((0, 7))
                }
                Some(invocation_response::Response::OutputItem(item)) => {
                    assert!(result_seen, "output arrived before the return value");
                    assert_eq!(
                        item.value.unwrap().value,
                        Some(schema_value::Value::U32Value(if first_item_seen {
                            8
                        } else {
                            7
                        }))
                    );
                    if first_item_seen {
                        return Ok::<(), anyhow::Error>(());
                    }
                    // A resumed Result can precede reconstruction. Fresh output proves that
                    // the reconstructed producer reached materialization before it may spin.
                    first_item_seen = true;
                    Some((1, 8))
                }
                Some(invocation_response::Response::Finished(_)) => {
                    anyhow::bail!("producer finished before interruption")
                }
                _ => None,
            };
            if let Some((sequence, value)) = next_input {
                let acceptance = accepted.as_ref().expect("acceptance before input");
                let input = acceptance
                    .stream_mappings
                    .iter()
                    .find(|mapping| mapping.transport_stream_id == 1)
                    .expect("input mapping");
                let item = InvocationRequest {
                    request: Some(invocation_request::Request::InputItem(InputStreamItem {
                        transport_stream_id: 1,
                        sequence,
                        payload: Some(input_stream_item::Payload::Value(
                            SchemaValue::U32(value)
                                .try_into()
                                .map_err(anyhow::Error::msg)?,
                        )),
                        durable_stream_id: input
                            .handle
                            .as_ref()
                            .and_then(|handle| handle.stream_id),
                        epoch: acceptance.epoch,
                    })),
                };
                state
                    .validate_trusted_request(&item)
                    .map_err(anyhow::Error::msg)?;
                requests.send(item).await?;
            }
        }
    })
    .await
    .map_err(|_| anyhow::anyhow!("producer did not complete its post-result spin handshake"))??;

    // The producer now spins without another stream write or cooperative guest wait.
    tokio::time::timeout(Duration::from_secs(5), executor.interrupt(&agent_id))
        .await
        .map_err(|_| anyhow::anyhow!("post-result interruption was not acknowledged"))??;
    assert_eq!(
        executor.get_worker_metadata(&agent_id).await?.status,
        AgentStatus::Interrupted
    );
    let oplog = executor.get_oplog(&agent_id, OplogIndex::INITIAL).await?;
    assert!(
        oplog
            .iter()
            .any(|entry| matches!(entry.entry, PublicOplogEntry::Interrupted(_)))
    );
    assert!(
        !oplog
            .iter()
            .any(|entry| matches!(entry.entry, PublicOplogEntry::Error(_))),
        "interruption must not become an invocation error or retry"
    );
    assert_interrupted_transport_closed(&mut responses, &mut state).await?;
    drop(requests);
    drop(responses);
    assert_failed_redispatch(&executor, start).await?;
    let method_starts = executor
        .get_oplog(&agent_id, OplogIndex::INITIAL)
        .await?
        .into_iter()
        .filter(|entry| {
            matches!(
                &entry.entry,
                PublicOplogEntry::AgentInvocationStarted(started)
                    if matches!(
                        &started.invocation,
                        PublicAgentInvocation::AgentMethodInvocation(method)
                            if method.method_name == "produce_then_spin"
                    )
            )
        })
        .count();
    assert_eq!(
        method_starts, 1,
        "redispatch restarted the interrupted producer"
    );
    Ok(())
}

async fn assert_interrupted_transport_closed(
    responses: &mut tonic::Streaming<InvocationResponse>,
    state: &mut InvocationSessionState,
) -> anyhow::Result<()> {
    // Owner retirement does not wait for transport delivery. Durable interruption and
    // same-key redispatch are checked separately from this disposable attachment.
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let response = match responses.message().await {
                Ok(Some(response)) => response,
                Ok(None) => return Ok(()),
                Err(error)
                    if matches!(
                        error.code(),
                        tonic::Code::Cancelled | tonic::Code::Unavailable
                    ) =>
                {
                    return Ok(());
                }
                Err(error) => return Err(error.into()),
            };
            state
                .validate_response(&response)
                .map_err(anyhow::Error::msg)?;
            if let Some(invocation_response::Response::Finished(finished)) = response.response {
                let Some(invocation_session_completion::Outcome::Failure(failure)) =
                    finished.outcome
                else {
                    anyhow::bail!("interrupted producer completed successfully");
                };
                assert_ne!(failure.kind, InvocationFailureKind::Protocol as i32);
                return Ok(());
            }
        }
    })
    .await
    .map_err(|_| anyhow::anyhow!("interrupted transport remained open"))?
}

async fn assert_failed_redispatch(
    executor: &TestWorkerExecutor,
    mut start: InvocationRequest,
) -> anyhow::Result<String> {
    let Some(invocation_request::Request::Start(request)) = start.request.as_mut() else {
        unreachable!("redispatch requires the original Start");
    };
    request.attempt_id = Some(uuid::Uuid::new_v4().into());
    let mut state = InvocationSessionState::default();
    state
        .validate_trusted_request(&start)
        .map_err(anyhow::Error::msg)?;
    let (requests, receiver) = mpsc::channel(1);
    requests.send(start).await?;
    let mut responses = executor
        .client
        .clone()
        .invoke_agent_session(ReceiverStream::new(receiver))
        .await?
        .into_inner();
    let failure = tokio::time::timeout(Duration::from_secs(10), async {
        let failure = loop {
            let response = responses
                .message()
                .await?
                .ok_or_else(|| anyhow::anyhow!("same-key redispatch closed without a terminal"))?;
            state
                .validate_response(&response)
                .map_err(anyhow::Error::msg)?;
            match response.response {
                Some(invocation_response::Response::Finished(finished)) => {
                    let Some(invocation_session_completion::Outcome::Failure(failure)) =
                        finished.outcome
                    else {
                        anyhow::bail!("same-key redispatch completed successfully");
                    };
                    assert_ne!(failure.kind, InvocationFailureKind::Protocol as i32);
                    break failure.message;
                }
                Some(invocation_response::Response::Rejected(rejected)) => break rejected.error,
                _ => {}
            }
        };
        assert!(state.is_complete());
        Ok::<_, anyhow::Error>(failure)
    })
    .await
    .map_err(|_| anyhow::anyhow!("same-key redispatch did not terminalize"))??;
    drop(requests);
    Ok(failure)
}

#[test]
#[timeout("2 minutes")]
#[tracing::instrument]
async fn stream_local_output_failure_does_not_fail_sibling_or_invocation(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] agent_rpc_rust: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;
    let component = executor
        .component_dep(&context.default_environment_id, agent_rpc_rust)
        .store()
        .await?;
    let agent_id = agent_id!("StreamingRpcTarget", "stream-local-sibling-error");
    let worker_agent_id = executor.start_agent(&component.id, agent_id).await?;
    let metadata = executor.get_worker_metadata(&worker_agent_id).await?;
    let (_, input) = data_value!().into_parts();
    let start = InvocationRequest {
        request: Some(invocation_request::Request::Start(InvocationStart {
            agent_id: Some(worker_agent_id.into()),
            method_name: Some("produce_sibling_error".to_string()),
            input: Some(input.try_into().map_err(anyhow::Error::msg)?),
            idempotency_key: Some(IdempotencyKey::fresh().into()),
            auth_ctx: Some(executor.auth_ctx().into()),
            environment_id: Some(component.environment_id.into()),
            component_owner_account_id: Some(component.account_id.into()),
            mode: golem_api_grpc::proto::golem::worker::AgentInvocationMode::Await as i32,
            freshness_disposition:
                golem_api_grpc::proto::golem::worker::InvocationFreshnessDisposition::MayExist
                    as i32,
            attempt_id: Some(uuid::Uuid::new_v4().into()),
            expected_callee_fingerprint: Some(metadata.fingerprint.0.into()),
            ..Default::default()
        })),
    };
    let mut state = InvocationSessionState::default();
    state
        .validate_trusted_request(&start)
        .map_err(anyhow::Error::msg)?;
    let (requests, receiver) = mpsc::channel(8);
    requests.send(start).await?;
    let mut responses = executor
        .client
        .clone()
        .invoke_agent_session(ReceiverStream::new(receiver))
        .await?
        .into_inner();
    let mut mapped_outputs = 0;
    let mut output_items = 0;
    let mut output_errors = 0;
    let mut output_ends = 0;
    let mut finished_successfully = false;

    while let Some(response) = responses.message().await? {
        state
            .validate_response(&response)
            .map_err(anyhow::Error::msg)?;
        match response.response {
            Some(invocation_response::Response::Accepted(_)) => {}
            Some(invocation_response::Response::Result(result)) => {
                mapped_outputs = result.new_stream_mappings.len();
            }
            Some(invocation_response::Response::OutputItem(_)) => output_items += 1,
            Some(invocation_response::Response::OutputError(_)) => output_errors += 1,
            Some(invocation_response::Response::OutputEnd(_)) => output_ends += 1,
            Some(invocation_response::Response::Finished(finished)) => {
                finished_successfully = matches!(
                    finished.outcome,
                    Some(invocation_session_completion::Outcome::Success(_))
                );
            }
            Some(invocation_response::Response::Rejected(rejected)) => {
                anyhow::bail!("invocation rejected: {}", rejected.error)
            }
            Some(other) => anyhow::bail!("unexpected durable output response: {other:?}"),
            None => anyhow::bail!("empty durable output response"),
        }
    }

    assert!(state.is_complete());
    assert_eq!(mapped_outputs, 2);
    assert_eq!(output_items, 65);
    assert_eq!(output_errors, 1);
    assert_eq!(output_ends, 1);
    assert!(finished_successfully);
    Ok(())
}

#[test]
#[timeout("2 minutes")]
#[tracing::instrument]
async fn durable_streaming_output_recovers_after_executor_restart(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] agent_rpc_rust: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    streaming_output_resume_restores_exact_cursors(last_unique_id, deps, agent_rpc_rust, true).await
}

#[test]
#[timeout("2 minutes")]
#[tracing::instrument]
async fn resident_ephemeral_streaming_output_resume_restores_cursors_without_duplicates(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] agent_rpc_rust: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    streaming_output_resume_restores_exact_cursors(last_unique_id, deps, agent_rpc_rust, false)
        .await
}

async fn streaming_output_resume_restores_exact_cursors(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    agent_rpc_rust: &PrecompiledComponent,
    restart_executor: bool,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let overrides = TestExecutorOverrides {
        configure: Some(Arc::new(|config| {
            config.invocation_results.recent_capacity = 0;
            config.invocation_results.bloom_bits = 1;
            config.invocation_results.bloom_hashes = 1;
        })),
        ..Default::default()
    };
    let executor = start_with_overrides(deps, &context, overrides.clone()).await?;
    let component = executor
        .component_dep(&context.default_environment_id, agent_rpc_rust)
        .store()
        .await?;
    let idempotency_key = IdempotencyKey::fresh();
    let (worker_agent_id, method_name, input, gate) = if restart_executor {
        let agent_id = agent_id!("StreamingRpcTarget", "output-restart");
        let worker_agent_id = executor
            .start_agent(&component.id, agent_id.clone())
            .await?;
        let gate = executor
            .invoke_and_await_agent(&component, &agent_id, "create_output_gate", data_value!())
            .await?
            .into_typed::<PromiseId>()?;
        (
            worker_agent_id,
            "produce_gated_siblings",
            data_value!(gate.clone()),
            Some(gate),
        )
    } else {
        let final_agent_id = agent_id!("EphemeralStreamingRpcTarget", "resident-output-resume")
            .with_ephemeral_invocation_phantom(&idempotency_key)
            .map_err(anyhow::Error::msg)?;
        (
            executor.start_agent(&component.id, final_agent_id).await?,
            "produce_siblings",
            data_value!(),
            None,
        )
    };
    let metadata = executor.get_worker_metadata(&worker_agent_id).await?;
    let (_, input) = input.into_parts();
    let start_request = InvocationRequest {
        request: Some(invocation_request::Request::Start(InvocationStart {
            agent_id: Some(worker_agent_id.clone().into()),
            method_name: Some(method_name.to_string()),
            input: Some(input.try_into().map_err(anyhow::Error::msg)?),
            idempotency_key: Some(idempotency_key.into()),
            auth_ctx: Some(executor.auth_ctx().into()),
            environment_id: Some(component.environment_id.into()),
            component_owner_account_id: Some(component.account_id.into()),
            mode: golem_api_grpc::proto::golem::worker::AgentInvocationMode::Await as i32,
            freshness_disposition:
                golem_api_grpc::proto::golem::worker::InvocationFreshnessDisposition::MayExist
                    as i32,
            attempt_id: Some(uuid::Uuid::new_v4().into()),
            expected_callee_fingerprint: Some(metadata.fingerprint.0.into()),
            ..Default::default()
        })),
    };
    let mut first_state = InvocationSessionState::default();
    first_state
        .validate_trusted_request(&start_request)
        .map_err(anyhow::Error::msg)?;
    let (requests, receiver) = mpsc::channel(1);
    requests.send(start_request.clone()).await?;
    let mut responses = executor
        .client
        .clone()
        .invoke_agent_session(ReceiverStream::new(receiver))
        .await?
        .into_inner();
    let accepted = responses
        .message()
        .await?
        .ok_or_else(|| anyhow::anyhow!("streaming output ended before acceptance"))?;
    first_state
        .validate_response(&accepted)
        .map_err(anyhow::Error::msg)?;
    let accepted = match accepted.response {
        Some(invocation_response::Response::Accepted(accepted)) => accepted,
        other => anyhow::bail!("expected durable output acceptance, got {other:?}"),
    };
    let mut observed_output_items = 0;
    let mut cursors = BTreeMap::new();
    let mut observed_values = BTreeMap::<u64, Vec<SchemaValue>>::new();
    let mut observed_offsets = BTreeMap::<u64, BTreeSet<Vec<u8>>>::new();
    while observed_output_items < 10 {
        let response = responses
            .message()
            .await?
            .ok_or_else(|| anyhow::anyhow!("streaming output ended before cursor checkpoint"))?;
        first_state
            .validate_response(&response)
            .map_err(anyhow::Error::msg)?;
        if let Some(invocation_response::Response::OutputItem(item)) = response.response {
            let value = item
                .value
                .ok_or_else(|| anyhow::anyhow!("durable output item omitted its value"))?
                .try_into()
                .map_err(anyhow::Error::msg)?;
            assert!(
                observed_offsets
                    .entry(item.transport_stream_id)
                    .or_default()
                    .insert(item.durable_offset.clone()),
                "duplicate offset before resume"
            );
            observed_values
                .entry(item.transport_stream_id)
                .or_default()
                .push(value);
            let stream_id = item
                .durable_stream_id
                .ok_or_else(|| anyhow::anyhow!("durable output item omitted its stream ID"))?;
            cursors.insert(
                (stream_id.high_bits, stream_id.low_bits),
                StreamCursor {
                    stream_id: Some(stream_id),
                    last_observed_offset: Some(item.durable_offset),
                },
            );
            observed_output_items += 1;
        }
    }
    if restart_executor {
        executor.shutdown_and_wait_for_invocation_loops().await?;
    }
    drop(requests);
    let resident_response_lease = (!restart_executor).then_some(responses);
    let executor = if restart_executor {
        drop(executor);
        start_with_overrides(deps, &context, overrides).await?
    } else {
        executor
    };
    if !restart_executor {
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                if executor
                    .get_oplog(&worker_agent_id, OplogIndex::INITIAL)
                    .await?
                    .into_iter()
                    .any(|entry| {
                        matches!(
                            entry.entry,
                            PublicOplogEntry::StreamSession(session)
                                if matches!(
                                    StreamSessionRecord::from_value(session.record.value()),
                                    Ok(StreamSessionRecord::Detached(record))
                                        if record.epoch == accepted.epoch
                                )
                        )
                    })
                {
                    return Ok::<(), anyhow::Error>(());
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .map_err(|_| anyhow::anyhow!("ephemeral output session did not detach before resume"))??;
    }
    let Some(invocation_request::Request::Start(start)) = start_request.request.as_ref() else {
        anyhow::bail!("streaming output request is not Start");
    };
    let resume_request = InvocationRequest {
        request: Some(invocation_request::Request::ResumeAttach(ResumeAttach {
            idempotency_key: start.idempotency_key.clone(),
            agent_id: if restart_executor {
                start.agent_id.clone()
            } else {
                accepted.agent_id.clone()
            },
            environment_id: start.environment_id,
            attachment_id: accepted.attachment_id,
            attempt_id: Some(uuid::Uuid::new_v4().into()),
            expected_callee_fingerprint: start.expected_callee_fingerprint,
            expected_epoch: accepted.epoch,
            operation: if restart_executor {
                ResumeOperation::Takeover
            } else {
                ResumeOperation::Resume
            } as i32,
            cursors: cursors.into_values().collect(),
            auth_ctx: start.auth_ctx.clone(),
            principal: start.principal.clone(),
        })),
    };
    let mut recovered_state = InvocationSessionState::default();
    recovered_state
        .validate_trusted_request(&resume_request)
        .map_err(anyhow::Error::msg)?;
    let (requests, receiver) = mpsc::channel(1);
    requests.send(resume_request).await?;
    let mut responses = executor
        .client
        .clone()
        .invoke_agent_session(ReceiverStream::new(receiver))
        .await?
        .into_inner();
    drop(resident_response_lease);
    let mut mapped_outputs = 0;
    let mut output_items = 0;
    let mut output_ends = 0;
    let mut finished_successfully = false;
    while let Some(response) = responses.message().await? {
        recovered_state
            .validate_response(&response)
            .map_err(anyhow::Error::msg)?;
        match response.response {
            Some(invocation_response::Response::Accepted(resumed)) => {
                assert_eq!(resumed.epoch, accepted.epoch + 1);
                if let Some(gate) = &gate {
                    executor.complete_promise(gate, Vec::new()).await?;
                }
            }
            Some(invocation_response::Response::Result(result)) => {
                mapped_outputs = result.new_stream_mappings.len();
            }
            Some(invocation_response::Response::OutputItem(item)) => {
                let value = item
                    .value
                    .ok_or_else(|| anyhow::anyhow!("resumed output item omitted its value"))?
                    .try_into()
                    .map_err(anyhow::Error::msg)?;
                assert!(
                    observed_offsets
                        .entry(item.transport_stream_id)
                        .or_default()
                        .insert(item.durable_offset),
                    "resume redelivered an observed durable offset"
                );
                observed_values
                    .entry(item.transport_stream_id)
                    .or_default()
                    .push(value);
                output_items += 1;
            }
            Some(invocation_response::Response::OutputEnd(_)) => output_ends += 1,
            Some(invocation_response::Response::Finished(finished)) => {
                finished_successfully = matches!(
                    finished.outcome,
                    Some(invocation_session_completion::Outcome::Success(_))
                );
            }
            Some(invocation_response::Response::Rejected(rejected)) => {
                anyhow::bail!("recovered invocation rejected: {}", rejected.error)
            }
            Some(other) => anyhow::bail!("unexpected recovered output response: {other:?}"),
            None => anyhow::bail!("empty recovered output response"),
        }
    }
    assert!(recovered_state.is_complete());
    assert_eq!(mapped_outputs, 2);
    assert_eq!(output_items, 66 - observed_output_items);
    assert_eq!(output_ends, 2);
    assert!(finished_successfully);
    let mut actual_stream_values: Vec<Vec<SchemaValue>> = observed_values.into_values().collect();
    actual_stream_values.sort_by_key(Vec::len);
    assert_eq!(
        actual_stream_values,
        vec![
            vec![
                SchemaValue::String("a".into()),
                SchemaValue::String("b".into())
            ],
            (0..64).map(SchemaValue::U32).collect(),
        ],
        "resume must deliver every exact sibling value once"
    );
    if restart_executor {
        return Ok(());
    }
    let final_agent_id: AgentId = accepted
        .agent_id
        .ok_or_else(|| anyhow::anyhow!("ephemeral acceptance omitted final agent ID"))?
        .try_into()
        .map_err(anyhow::Error::msg)?;
    let starts = executor
        .get_oplog(&final_agent_id, OplogIndex::INITIAL)
        .await?
        .into_iter()
        .filter(|entry| matches!(entry.entry, PublicOplogEntry::AgentInvocationStarted(_)))
        .count();
    assert_eq!(
        starts, 2,
        "expected initialization plus one method start; ResumeAttach must not execute again"
    );
    Ok(())
}

#[test]
#[timeout("2 minutes")]
#[tracing::instrument]
async fn active_ephemeral_compute_interrupt_same_key_does_not_restart_invocation(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] agent_rpc_rust: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;
    let component = executor
        .component_dep(&context.default_environment_id, agent_rpc_rust)
        .store()
        .await?;
    let logical_agent_id = agent_id!("EphemeralStreamingRpcTarget", "interrupt-spin");
    let idempotency_key = IdempotencyKey::fresh();
    let final_agent_id = AgentId::from_agent_id(
        component.id,
        &logical_agent_id
            .with_ephemeral_invocation_phantom(&idempotency_key)
            .map_err(anyhow::Error::msg)?,
    )
    .map_err(anyhow::Error::msg)?;
    executor
        .start_agent(
            &component.id,
            logical_agent_id
                .with_ephemeral_invocation_phantom(&idempotency_key)
                .map_err(anyhow::Error::msg)?,
        )
        .await?;

    let invocation_executor = executor.clone();
    let invocation_component = component.clone();
    let invocation_agent_id = logical_agent_id.clone();
    let invocation_key = idempotency_key.clone();
    let invocation = tokio::spawn(async move {
        invocation_executor
            .invoke_and_await_agent_with_key(
                &invocation_component,
                &invocation_agent_id,
                &invocation_key,
                "spin",
                data_value!(),
            )
            .await
    });
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if executor
                .get_oplog(&final_agent_id, OplogIndex::INITIAL)
                .await?
                .into_iter()
                .any(|entry| {
                    matches!(
                        &entry.entry,
                        PublicOplogEntry::AgentInvocationStarted(started)
                            if matches!(
                                &started.invocation,
                                PublicAgentInvocation::AgentMethodInvocation(method)
                                    if method.method_name == "spin"
                            )
                    )
                })
            {
                return Ok::<(), anyhow::Error>(());
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .map_err(|_| anyhow::anyhow!("spin did not reach its method-start marker"))??;
    executor.interrupt(&final_agent_id).await?;
    let first = tokio::time::timeout(Duration::from_secs(10), invocation)
        .await
        .map_err(|_| {
            anyhow::anyhow!("active ephemeral invocation did not observe interruption")
        })??;
    assert!(first.is_err(), "interrupted compute unexpectedly succeeded");

    let redispatch = executor
        .invoke_and_await_agent_with_key(
            &component,
            &logical_agent_id,
            &idempotency_key,
            "spin",
            data_value!(),
        )
        .await;
    assert!(
        redispatch.is_err(),
        "same-key redispatch restarted interrupted ephemeral compute"
    );
    assert_eq!(
        executor
            .get_oplog(&final_agent_id, OplogIndex::INITIAL)
            .await?
            .into_iter()
            .filter(|entry| matches!(
                &entry.entry,
                PublicOplogEntry::AgentInvocationStarted(started)
                    if matches!(
                        &started.invocation,
                        PublicAgentInvocation::AgentMethodInvocation(method)
                            if method.method_name == "spin"
                    )
            ))
            .count(),
        1,
        "same-key redispatch must not start a second method invocation"
    );
    Ok(())
}

#[test]
#[timeout("2 minutes")]
#[tracing::instrument]
async fn pending_ephemeral_interrupt_after_acceptance_prevents_method_start(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] agent_rpc_rust: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;
    let component = executor
        .component_dep(&context.default_environment_id, agent_rpc_rust)
        .store()
        .await?;

    let target_key = IdempotencyKey::fresh();
    let target_logical = agent_id!("EphemeralStreamingRpcTarget", "pending-cancel-target");
    let target_parsed = target_logical
        .with_ephemeral_invocation_phantom(&target_key)
        .map_err(anyhow::Error::msg)?;
    let target_final =
        AgentId::from_agent_id(component.id, &target_parsed).map_err(anyhow::Error::msg)?;

    // An interrupt before the target exists is intentionally a no-op. It must not
    // pre-arm a stop that consumes the later accepted invocation.
    executor.interrupt(&target_final).await?;
    let mut initialization_success = executor.gate_next_agent_invocation_success(&target_final);
    let (mut state, _frames, mut inbound) = open_invocation_session_with_key(
        &executor,
        &component,
        &target_parsed,
        &target_key,
        "spin",
        data_value!(),
    )
    .await?;
    let accepted = tokio::time::timeout(Duration::from_secs(10), inbound.message())
        .await
        .map_err(|_| anyhow::anyhow!("pending invocation was not accepted"))??
        .ok_or_else(|| anyhow::anyhow!("pending invocation ended before acceptance"))?;
    state
        .validate_response(&accepted)
        .map_err(anyhow::Error::msg)?;
    if !matches!(
        accepted.response,
        Some(invocation_response::Response::Accepted(_))
    ) {
        anyhow::bail!("expected pending acceptance after absent interrupt, got {accepted:?}");
    }

    tokio::time::timeout(Duration::from_secs(10), initialization_success.entered())
        .await
        .map_err(|_| anyhow::anyhow!("initialization did not reach its success barrier"))?;
    let active = executor
        .active_agent(&OwnedAgentId::new(component.environment_id, &target_final))
        .await
        .expect("initialization is resident");
    let execution_status = active.resources().execution_status();
    let interrupt = tokio::spawn({
        let executor = executor.clone();
        let target_final = target_final.clone();
        async move { executor.interrupt(&target_final).await }
    });
    tokio::time::timeout(Duration::from_secs(10), async {
        while !matches!(
            &*execution_status.read().unwrap(),
            golem_worker_executor::model::ExecutionStatus::Interrupting { .. }
        ) {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .map_err(|_| anyhow::anyhow!("initialization interrupt was not signalled"))?;
    assert!(
        !interrupt.is_finished(),
        "interrupt acknowledged before initialization settled"
    );
    initialization_success.release();
    tokio::time::timeout(Duration::from_secs(10), interrupt)
        .await
        .map_err(|_| anyhow::anyhow!("initialization interrupt was not acknowledged"))???;
    let finished = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let response = inbound
                .message()
                .await?
                .ok_or_else(|| anyhow::anyhow!("pending invocation closed without a terminal"))?;
            state
                .validate_response(&response)
                .map_err(anyhow::Error::msg)?;
            if let Some(invocation_response::Response::Finished(finished)) = response.response {
                return Ok::<_, anyhow::Error>(finished);
            }
        }
    })
    .await
    .map_err(|_| anyhow::anyhow!("pending cancellation did not terminalize"))??;
    assert!(matches!(
        finished.outcome,
        Some(invocation_session_completion::Outcome::Failure(_))
    ));
    assert!(state.is_complete());
    executor
        .wait_for_status(
            &target_final,
            AgentStatus::Interrupted,
            Duration::from_secs(10),
        )
        .await?;
    let method_starts = executor
        .get_oplog(&target_final, OplogIndex::INITIAL)
        .await?
        .into_iter()
        .filter(|entry| {
            matches!(
                &entry.entry,
                PublicOplogEntry::AgentInvocationStarted(started)
                    if matches!(
                        &started.invocation,
                        PublicAgentInvocation::AgentMethodInvocation(method)
                            if method.method_name == "spin"
                    )
            )
        })
        .count();
    assert_eq!(
        method_starts, 0,
        "pending cancellation allowed spin to start after the permit was released"
    );
    Ok(())
}

#[test]
#[timeout("2 minutes")]
#[tracing::instrument]
async fn active_ephemeral_streaming_input_interrupt_resume_same_key_does_not_restart_method(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] agent_rpc_rust: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;
    let component = executor
        .component_dep(&context.default_environment_id, agent_rpc_rust)
        .store()
        .await?;
    let idempotency_key = IdempotencyKey::fresh();
    let final_agent_id = agent_id!("EphemeralStreamingRpcTarget", "interrupt-held-input")
        .with_ephemeral_invocation_phantom(&idempotency_key)
        .map_err(anyhow::Error::msg)?;
    let worker_agent_id = executor.start_agent(&component.id, final_agent_id).await?;
    let metadata = executor.get_worker_metadata(&worker_agent_id).await?;
    let start_request = InvocationRequest {
        request: Some(invocation_request::Request::Start(InvocationStart {
            agent_id: Some(worker_agent_id.clone().into()),
            method_name: Some("hold_input".to_string()),
            input: Some(golem_api_grpc::proto::golem::schema::SchemaValue {
                value: Some(schema_value::Value::RecordValue(RecordValue {
                    fields: vec![golem_api_grpc::proto::golem::schema::SchemaValue {
                        value: Some(schema_value::Value::StreamReference(
                            SchemaValueStreamReference { stream_id: 1 },
                        )),
                    }],
                })),
            }),
            idempotency_key: Some(idempotency_key.clone().into()),
            auth_ctx: Some(executor.auth_ctx().into()),
            environment_id: Some(component.environment_id.into()),
            component_owner_account_id: Some(component.account_id.into()),
            mode: golem_api_grpc::proto::golem::worker::AgentInvocationMode::Await as i32,
            freshness_disposition:
                golem_api_grpc::proto::golem::worker::InvocationFreshnessDisposition::MayExist
                    as i32,
            attempt_id: Some(uuid::Uuid::new_v4().into()),
            expected_callee_fingerprint: Some(metadata.fingerprint.0.into()),
            ..Default::default()
        })),
    };
    let mut state = InvocationSessionState::default();
    state
        .validate_trusted_request(&start_request)
        .map_err(anyhow::Error::msg)?;
    let (requests, receiver) = mpsc::channel(8);
    requests.send(start_request.clone()).await?;
    let mut responses = executor
        .client
        .clone()
        .invoke_agent_session(ReceiverStream::new(receiver))
        .await?
        .into_inner();
    let accepted = tokio::time::timeout(Duration::from_secs(10), responses.message())
        .await
        .map_err(|_| anyhow::anyhow!("held-input invocation was not accepted"))??
        .ok_or_else(|| anyhow::anyhow!("held-input session ended before acceptance"))?;
    state
        .validate_response(&accepted)
        .map_err(anyhow::Error::msg)?;
    assert!(matches!(
        accepted.response,
        Some(invocation_response::Response::Accepted(_))
    ));

    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let starts = executor
                .get_oplog(&worker_agent_id, OplogIndex::INITIAL)
                .await?
                .into_iter()
                .filter(|entry| {
                    matches!(
                        &entry.entry,
                        PublicOplogEntry::AgentInvocationStarted(started)
                            if matches!(
                                &started.invocation,
                                PublicAgentInvocation::AgentMethodInvocation(method)
                                    if method.method_name == "hold_input"
                            )
                    )
                })
                .count();
            if starts == 1 {
                return Ok::<(), anyhow::Error>(());
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .map_err(|_| anyhow::anyhow!("hold_input did not reach its method-start marker"))??;

    assert!(
        tokio::time::timeout(Duration::from_millis(100), responses.message())
            .await
            .is_err(),
        "host-wait invocation terminated before interruption"
    );
    executor.interrupt(&worker_agent_id).await?;
    assert_interrupted_transport_closed(&mut responses, &mut state).await?;
    assert_eq!(
        executor.get_worker_metadata(&worker_agent_id).await?.status,
        AgentStatus::Interrupted
    );
    drop(requests);
    drop(responses);

    let resume = executor.resume(&worker_agent_id, false).await;
    assert!(
        resume.is_err(),
        "interrupted ephemeral agent unexpectedly resumed"
    );
    let resume_error = resume.unwrap_err().to_string();
    let inactive_ephemeral_error =
        "An ephemeral agent cannot accept another invocation or be resumed";
    assert!(
        resume_error.contains(inactive_ephemeral_error),
        "explicit resume failed with the wrong category: {resume_error}"
    );

    // Start with a new attempt cannot replace the persisted streaming session;
    // it is rejected at attachment admission before ephemeral lifecycle checks.
    let redispatch_error = assert_failed_redispatch(&executor, start_request).await?;
    assert!(
        redispatch_error.contains("AttemptConflict"),
        "same-key redispatch failed with the wrong category: {redispatch_error}"
    );
    let method_starts = executor
        .get_oplog(&worker_agent_id, OplogIndex::INITIAL)
        .await?
        .into_iter()
        .filter(|entry| {
            matches!(
                &entry.entry,
                PublicOplogEntry::AgentInvocationStarted(started)
                    if matches!(
                        &started.invocation,
                        PublicAgentInvocation::AgentMethodInvocation(method)
                            if method.method_name == "hold_input"
                    )
            )
        })
        .count();
    assert_eq!(
        method_starts, 1,
        "resume or redispatch restarted hold_input"
    );
    Ok(())
}

#[derive(Debug, Eq, PartialEq)]
struct NestedSiblingOutputSnapshot {
    durable_stream_ids: BTreeMap<String, uuid::Uuid>,
    values: BTreeMap<String, Vec<u32>>,
}

async fn read_nested_sibling_output(
    executor: &TestWorkerExecutor,
    request: &InvocationRequest,
) -> anyhow::Result<(NestedSiblingOutputSnapshot, InvocationAccepted)> {
    let mut state = InvocationSessionState::default();
    state
        .validate_trusted_request(request)
        .map_err(anyhow::Error::msg)?;
    let (requests, receiver) = mpsc::channel(1);
    requests.send(request.clone()).await?;
    let mut responses = executor
        .client
        .clone()
        .invoke_agent_session(ReceiverStream::new(receiver))
        .await?
        .into_inner();
    let mut root_stream_ids = BTreeSet::new();
    let mut observed_root_stream_ids = BTreeSet::new();
    let mut labels_by_nested_stream = BTreeMap::new();
    let mut pending_values_by_nested_stream = BTreeMap::<u64, Vec<u32>>::new();
    let mut durable_stream_ids = BTreeMap::new();
    let mut values = BTreeMap::<String, Vec<u32>>::new();
    let mut terminal_count = 0;
    let mut finished_successfully = false;
    let mut acceptance = None;

    while let Some(response) = responses.message().await? {
        state
            .validate_response(&response)
            .map_err(anyhow::Error::msg)?;
        match response.response {
            Some(invocation_response::Response::Accepted(accepted)) => {
                acceptance = Some(accepted);
            }
            Some(invocation_response::Response::Result(result)) => {
                let value = match result.result {
                    Some(invocation_session_result::Result::MethodResult(value)) => value,
                    other => anyhow::bail!("expected a nested sibling result, got {other:?}"),
                };
                let Some(schema_value::Value::TupleValue(tuple)) = value.value else {
                    anyhow::bail!("expected the nested sibling result to be a tuple");
                };
                if tuple.elements.len() != 2 {
                    anyhow::bail!(
                        "expected two root nested sibling streams, got {}",
                        tuple.elements.len()
                    );
                }
                for element in tuple.elements {
                    let Some(schema_value::Value::StreamReference(reference)) = element.value
                    else {
                        anyhow::bail!("nested sibling result contains a non-stream value");
                    };
                    root_stream_ids.insert(reference.stream_id);
                }
                let mapped_stream_ids = result
                    .new_stream_mappings
                    .iter()
                    .map(|mapping| mapping.transport_stream_id)
                    .collect::<BTreeSet<_>>();
                assert_eq!(mapped_stream_ids, root_stream_ids);
            }
            Some(invocation_response::Response::OutputItem(item)) => {
                let Some(value) = item.value.and_then(|value| value.value) else {
                    anyhow::bail!("nested sibling item has no value");
                };
                if let schema_value::Value::RecordValue(record) = value {
                    observed_root_stream_ids.insert(item.transport_stream_id);
                    let [label, nested] = record.fields.as_slice() else {
                        anyhow::bail!("nested sibling item does not have two fields");
                    };
                    let Some(schema_value::Value::StringValue(label)) = label.value.as_ref() else {
                        anyhow::bail!("nested sibling label is not a string");
                    };
                    let Some(schema_value::Value::StreamReference(nested)) = nested.value.as_ref()
                    else {
                        anyhow::bail!("nested sibling value is not a stream");
                    };
                    let [mapping] = item.new_stream_mappings.as_slice() else {
                        anyhow::bail!("nested sibling item does not introduce exactly one stream");
                    };
                    assert_eq!(mapping.transport_stream_id, nested.stream_id);
                    let durable_stream_id = mapping
                        .handle
                        .as_ref()
                        .and_then(|handle| handle.stream_id)
                        .map(uuid::Uuid::from)
                        .ok_or_else(|| {
                            anyhow::anyhow!("nested sibling mapping has no durable stream ID")
                        })?;
                    if labels_by_nested_stream
                        .insert(nested.stream_id, label.clone())
                        .is_some()
                    {
                        anyhow::bail!("nested sibling transport stream was mapped twice");
                    }
                    if durable_stream_ids
                        .insert(label.clone(), durable_stream_id)
                        .is_some()
                    {
                        anyhow::bail!("nested sibling label was mapped twice");
                    }
                    values.insert(
                        label.clone(),
                        pending_values_by_nested_stream
                            .remove(&nested.stream_id)
                            .unwrap_or_default(),
                    );
                } else {
                    let value = match value {
                        schema_value::Value::U32Value(value) => value,
                        other => {
                            anyhow::bail!("expected a nested sibling u32 item, got {other:?}")
                        }
                    };
                    if let Some(label) = labels_by_nested_stream.get(&item.transport_stream_id) {
                        values
                            .get_mut(label)
                            .expect("known nested sibling label has a value list")
                            .push(value);
                    } else {
                        pending_values_by_nested_stream
                            .entry(item.transport_stream_id)
                            .or_default()
                            .push(value);
                    }
                }
            }
            Some(invocation_response::Response::OutputEnd(_)) => terminal_count += 1,
            Some(invocation_response::Response::Finished(finished)) => {
                finished_successfully = matches!(
                    finished.outcome,
                    Some(invocation_session_completion::Outcome::Success(_))
                );
            }
            Some(invocation_response::Response::Rejected(rejected)) => {
                anyhow::bail!("nested sibling invocation rejected: {}", rejected.error)
            }
            Some(other) => anyhow::bail!("unexpected nested sibling response: {other:?}"),
            None => anyhow::bail!("empty nested sibling response"),
        }
    }

    assert!(state.is_complete());
    assert!(finished_successfully);
    assert_eq!(terminal_count, 4);
    assert_eq!(observed_root_stream_ids, root_stream_ids);
    assert!(pending_values_by_nested_stream.is_empty());
    assert_eq!(durable_stream_ids.len(), 2);
    assert_eq!(values.get("left"), Some(&vec![1, 2]));
    assert_eq!(values.get("right"), Some(&vec![10, 20, 30]));
    Ok((
        NestedSiblingOutputSnapshot {
            durable_stream_ids,
            values,
        },
        acceptance.ok_or_else(|| anyhow::anyhow!("nested sibling invocation was not accepted"))?,
    ))
}

#[test]
#[timeout("2 minutes")]
#[tracing::instrument]
async fn concurrent_nested_sibling_output_replays_after_executor_restart(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] agent_rpc_rust: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;
    let component = executor
        .component_dep(&context.default_environment_id, agent_rpc_rust)
        .store()
        .await?;
    let agent_id = agent_id!("StreamingRpcTarget", "nested-sibling-output-restart");
    let worker_agent_id = executor.start_agent(&component.id, agent_id).await?;
    let metadata = executor.get_worker_metadata(&worker_agent_id).await?;
    let (_, input) = data_value!().into_parts();
    let start_request = InvocationRequest {
        request: Some(invocation_request::Request::Start(InvocationStart {
            agent_id: Some(worker_agent_id.into()),
            method_name: Some("produce_nested_siblings".to_string()),
            input: Some(input.try_into().map_err(anyhow::Error::msg)?),
            idempotency_key: Some(IdempotencyKey::fresh().into()),
            auth_ctx: Some(executor.auth_ctx().into()),
            environment_id: Some(component.environment_id.into()),
            component_owner_account_id: Some(component.account_id.into()),
            mode: golem_api_grpc::proto::golem::worker::AgentInvocationMode::Await as i32,
            freshness_disposition:
                golem_api_grpc::proto::golem::worker::InvocationFreshnessDisposition::MayExist
                    as i32,
            attempt_id: Some(uuid::Uuid::new_v4().into()),
            expected_callee_fingerprint: Some(metadata.fingerprint.0.into()),
            ..Default::default()
        })),
    };

    let (first, accepted) = read_nested_sibling_output(&executor, &start_request).await?;
    drop(executor);

    let executor = start(deps, &context).await?;
    let Some(invocation_request::Request::Start(start)) = start_request.request.as_ref() else {
        anyhow::bail!("nested sibling output request is not Start");
    };
    let resume_request = InvocationRequest {
        request: Some(invocation_request::Request::ResumeAttach(ResumeAttach {
            idempotency_key: start.idempotency_key.clone(),
            agent_id: start.agent_id.clone(),
            environment_id: start.environment_id,
            attachment_id: accepted.attachment_id,
            attempt_id: Some(uuid::Uuid::new_v4().into()),
            expected_callee_fingerprint: start.expected_callee_fingerprint,
            expected_epoch: accepted.epoch,
            operation: ResumeOperation::Resume as i32,
            cursors: Vec::new(),
            auth_ctx: start.auth_ctx.clone(),
            principal: start.principal.clone(),
        })),
    };
    let (replayed, _) = read_nested_sibling_output(&executor, &resume_request).await?;
    assert_eq!(replayed, first);
    Ok(())
}

#[test]
#[timeout("2 minutes")]
#[tracing::instrument]
async fn malformed_request_after_streaming_result_terminalizes_open_streams(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] agent_rpc_rust: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;
    let component = executor
        .component_dep(&context.default_environment_id, agent_rpc_rust)
        .store()
        .await?;
    let agent_id = agent_id!("StreamingRpcTarget", "malformed-after-result");
    let worker_agent_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;
    let metadata = executor.get_worker_metadata(&worker_agent_id).await?;
    let input = golem_api_grpc::proto::golem::schema::SchemaValue {
        value: Some(schema_value::Value::RecordValue(RecordValue {
            fields: vec![golem_api_grpc::proto::golem::schema::SchemaValue {
                value: Some(schema_value::Value::StreamReference(
                    SchemaValueStreamReference { stream_id: 1 },
                )),
            }],
        })),
    };
    let start = InvocationRequest {
        request: Some(invocation_request::Request::Start(InvocationStart {
            agent_id: Some(worker_agent_id.clone().into()),
            method_name: Some("transform".to_string()),
            input: Some(input),
            idempotency_key: Some(IdempotencyKey::fresh().into()),
            auth_ctx: Some(executor.auth_ctx().into()),
            environment_id: Some(component.environment_id.into()),
            component_owner_account_id: Some(component.account_id.into()),
            mode: golem_api_grpc::proto::golem::worker::AgentInvocationMode::Await as i32,
            freshness_disposition:
                golem_api_grpc::proto::golem::worker::InvocationFreshnessDisposition::MayExist
                    as i32,
            attempt_id: Some(uuid::Uuid::new_v4().into()),
            expected_callee_fingerprint: Some(metadata.fingerprint.0.into()),
            ..Default::default()
        })),
    };
    let mut state = InvocationSessionState::default();
    state
        .validate_trusted_request(&start)
        .map_err(anyhow::Error::msg)?;
    let (requests, receiver) = mpsc::channel(8);
    requests.send(start.clone()).await?;
    let mut responses = executor
        .client
        .clone()
        .invoke_agent_session(ReceiverStream::new(receiver))
        .await?
        .into_inner();
    let mut expected_stream_ids = Vec::new();
    let mut terminal_stream_ids = Vec::new();
    let mut accepted = false;
    let mut result_received = false;

    while let Some(response) = responses.message().await? {
        state
            .validate_response(&response)
            .map_err(anyhow::Error::msg)?;
        match response.response {
            Some(invocation_response::Response::Accepted(_)) => {
                assert!(!accepted, "invocation was accepted more than once");
                accepted = true;
            }
            Some(invocation_response::Response::Result(result)) => {
                assert!(accepted, "streaming result preceded volatile acceptance");
                result_received = true;
                let value = match result.result {
                    Some(invocation_session_result::Result::MethodResult(value)) => value,
                    other => anyhow::bail!("expected a method result, got {other:?}"),
                };
                expected_stream_ids = match value.value {
                    Some(schema_value::Value::StreamReference(reference)) => {
                        vec![reference.stream_id]
                    }
                    other => anyhow::bail!("expected transform stream, got {other:?}"),
                };
                requests.send(start.clone()).await?;
            }
            Some(invocation_response::Response::OutputEnd(end)) => {
                terminal_stream_ids.push(end.transport_stream_id);
            }
            Some(invocation_response::Response::OutputError(error)) => {
                terminal_stream_ids.push(error.transport_stream_id);
            }
            Some(invocation_response::Response::Finished(finished)) => {
                assert!(accepted, "post-enqueue failure preceded acceptance");
                assert!(
                    result_received,
                    "protocol failure preceded the streaming result"
                );
                let failure = match finished.outcome {
                    Some(invocation_session_completion::Outcome::Failure(failure)) => failure,
                    other => anyhow::bail!("expected protocol failure, got {other:?}"),
                };
                assert_eq!(failure.kind, InvocationFailureKind::Protocol as i32);
                expected_stream_ids.sort_unstable();
                terminal_stream_ids.sort_unstable();
                assert_eq!(terminal_stream_ids, expected_stream_ids);
                assert!(state.is_complete());
                assert!(responses.message().await?.is_none());
                let oplog = executor
                    .get_oplog(&worker_agent_id, OplogIndex::INITIAL)
                    .await?;
                assert!(
                    oplog.iter().any(|entry| matches!(
                        &entry.entry,
                        PublicOplogEntry::AgentInvocationStarted(started)
                            if matches!(
                                &started.invocation,
                                PublicAgentInvocation::AgentMethodInvocation(method)
                                    if method.method_name == "transform"
                            )
                    )),
                    "durable streaming invocation was not journaled"
                );
                return Ok(());
            }
            Some(invocation_response::Response::Rejected(rejected)) => {
                anyhow::bail!("invocation rejected: {}", rejected.error)
            }
            _ => {}
        }
    }

    anyhow::bail!("invocation response closed without InvocationFinished")
}

fn origin_observer_start(
    executor: &TestWorkerExecutor,
    component: &ComponentDto,
    target: &AgentId,
    caller: &AgentId,
    fingerprint: uuid::Uuid,
    key: &IdempotencyKey,
    method: &str,
) -> InvocationStart {
    InvocationStart {
        agent_id: Some(target.clone().into()),
        method_name: Some(method.to_string()),
        input: Some(golem_api_grpc::proto::golem::schema::SchemaValue {
            value: Some(schema_value::Value::RecordValue(RecordValue {
                fields: vec![golem_api_grpc::proto::golem::schema::SchemaValue {
                    value: Some(schema_value::Value::StreamReference(
                        SchemaValueStreamReference { stream_id: 1 },
                    )),
                }],
            })),
        }),
        idempotency_key: Some(key.clone().into()),
        context: Some(golem_api_grpc::proto::golem::worker::InvocationContext {
            parent: Some(caller.clone().into()),
            ..Default::default()
        }),
        auth_ctx: Some(executor.auth_ctx().into()),
        principal: Some(
            Principal::Agent(AgentPrincipal {
                agent_id: caller.clone(),
            })
            .into(),
        ),
        environment_id: Some(component.environment_id.into()),
        component_owner_account_id: Some(component.account_id.into()),
        mode: golem_api_grpc::proto::golem::worker::AgentInvocationMode::Await as i32,
        freshness_disposition:
            golem_api_grpc::proto::golem::worker::InvocationFreshnessDisposition::MayExist as i32,
        attempt_id: Some(uuid::Uuid::new_v4().into()),
        expected_callee_fingerprint: Some(fingerprint.into()),
        origin_invocation: Some(StreamInvocationIdentity {
            callee_environment_id: Some(component.environment_id.into()),
            callee: Some(caller.clone().into()),
            callee_fingerprint: Some(uuid::Uuid::new_v4().into()),
            idempotency_key: Some(IdempotencyKey::fresh().into()),
        }),
        ..Default::default()
    }
}

async fn open_raw_session(
    executor: &TestWorkerExecutor,
    start: InvocationStart,
) -> anyhow::Result<(
    mpsc::Sender<InvocationRequest>,
    tonic::Streaming<InvocationResponse>,
    InvocationAccepted,
)> {
    let (requests, receiver) = mpsc::channel(8);
    requests
        .send(InvocationRequest {
            request: Some(invocation_request::Request::Start(start)),
        })
        .await?;
    let mut responses = executor
        .client
        .clone()
        .invoke_agent_session(ReceiverStream::new(receiver))
        .await?
        .into_inner();
    let first = responses
        .message()
        .await?
        .ok_or_else(|| anyhow::anyhow!("session closed before acceptance"))?;
    let Some(invocation_response::Response::Accepted(accepted)) = first.response else {
        anyhow::bail!("expected acceptance, got {first:?}");
    };
    Ok((requests, responses, accepted))
}

#[test]
#[timeout("2 minutes")]
async fn joined_origin_observer_receives_result_before_original_protocol_failure(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] agent_rpc_rust: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;
    let component = executor
        .component_dep(&context.default_environment_id, agent_rpc_rust)
        .store()
        .await?;
    let target = AgentId::from_agent_id(
        component.id,
        &agent_id!("StreamingRpcTarget", "observer-result-failure"),
    )
    .map_err(anyhow::Error::msg)?;
    let caller = AgentId::from_agent_id(
        component.id,
        &agent_id!("StreamingRpcCaller", "observer-result-failure"),
    )
    .map_err(anyhow::Error::msg)?;
    executor
        .start_agent(
            &component.id,
            agent_id!("StreamingRpcTarget", "observer-result-failure"),
        )
        .await?;
    let fingerprint = executor.get_worker_metadata(&target).await?.fingerprint.0;
    let start = origin_observer_start(
        &executor,
        &component,
        &target,
        &caller,
        fingerprint,
        &IdempotencyKey::fresh(),
        "transform",
    );
    let (original_tx, mut original_rx, original_accepted) =
        open_raw_session(&executor, start.clone()).await?;
    let original_result = tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            let response = original_rx
                .message()
                .await?
                .ok_or_else(|| anyhow::anyhow!("original closed before Result"))?;
            if matches!(
                response.response,
                Some(invocation_response::Response::Result(_))
            ) {
                return Ok::<_, anyhow::Error>(response);
            }
        }
    })
    .await??;
    let mut observer = start.clone();
    observer.attempt_id = Some(uuid::Uuid::new_v4().into());
    observer.durable_input_mappings = original_accepted.stream_mappings.clone();
    original_tx
        .send(InvocationRequest {
            request: Some(invocation_request::Request::Start(start)),
        })
        .await?;
    tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            let response = original_rx
                .message()
                .await?
                .ok_or_else(|| anyhow::anyhow!("original closed before failed Finished"))?;
            if let Some(invocation_response::Response::Finished(finished)) = response.response {
                assert!(matches!(
                    finished.outcome,
                    Some(invocation_session_completion::Outcome::Failure(_))
                ));
                return Ok::<_, anyhow::Error>(());
            }
        }
    })
    .await??;
    // Both records must already exist when the observer joins: scheduling must not
    // let the failed terminal hide the result's durable stream handles.
    let (_observer_tx, mut observer_rx, observer_accepted) =
        open_raw_session(&executor, observer).await?;
    assert!(observer_accepted.joined_origin_observer);
    assert!(observer_accepted.attachment_id.is_none());
    let mut saw_result = false;
    let failure = tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            let response = observer_rx
                .message()
                .await?
                .ok_or_else(|| anyhow::anyhow!("observer closed before failed Finished"))?;
            match response.response {
                Some(invocation_response::Response::Result(result)) => {
                    saw_result = true;
                    assert!(
                        !result.new_stream_mappings.is_empty(),
                        "observer Result omitted stream handles"
                    );
                }
                Some(invocation_response::Response::Finished(finished)) => {
                    break Ok::<_, anyhow::Error>(finished);
                }
                _ => {}
            }
        }
    })
    .await??;
    assert!(saw_result, "failed Finished preceded persisted Result");
    assert!(matches!(
        failure.outcome,
        Some(invocation_session_completion::Outcome::Failure(_))
    ));
    assert!(matches!(
        original_result.response,
        Some(invocation_response::Response::Result(_))
    ));
    Ok(())
}

#[test]
#[timeout("2 minutes")]
async fn joined_origin_observer_promptly_receives_failure_without_result(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] agent_rpc_rust: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;
    let component = executor
        .component_dep(&context.default_environment_id, agent_rpc_rust)
        .store()
        .await?;
    let parsed = agent_id!("StreamingRpcTarget", "observer-failure-first");
    let target = executor.start_agent(&component.id, parsed).await?;
    let caller = AgentId::from_agent_id(
        component.id,
        &agent_id!("StreamingRpcCaller", "observer-failure-first"),
    )
    .map_err(anyhow::Error::msg)?;
    let fingerprint = executor.get_worker_metadata(&target).await?.fingerprint.0;
    let start = origin_observer_start(
        &executor,
        &component,
        &target,
        &caller,
        fingerprint,
        &IdempotencyKey::fresh(),
        "hold_input",
    );
    let (original_tx, _original_rx, accepted) = open_raw_session(&executor, start.clone()).await?;
    let mut observer = start.clone();
    observer.attempt_id = Some(uuid::Uuid::new_v4().into());
    observer.durable_input_mappings = accepted.stream_mappings;
    let (_observer_tx, mut observer_rx, observer_accepted) =
        open_raw_session(&executor, observer).await?;
    assert!(observer_accepted.joined_origin_observer);
    original_tx
        .send(InvocationRequest {
            request: Some(invocation_request::Request::Start(start)),
        })
        .await?;
    let response = tokio::time::timeout(Duration::from_secs(10), observer_rx.message())
        .await??
        .ok_or_else(|| anyhow::anyhow!("observer closed before Finished"))?;
    assert!(
        matches!(response.response, Some(invocation_response::Response::Finished(ref finished)) if matches!(finished.outcome, Some(invocation_session_completion::Outcome::Failure(_))))
    );
    Ok(())
}

#[test]
#[timeout("2 minutes")]
async fn joined_origin_observer_disconnect_and_control_do_not_detach_original(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] agent_rpc_rust: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    for forbidden_control in [false, true] {
        let context = TestContext::new(last_unique_id);
        let executor = start(deps, &context).await?;
        let component = executor
            .component_dep(&context.default_environment_id, agent_rpc_rust)
            .store()
            .await?;
        let name = format!("observer-control-{forbidden_control}");
        let target = executor
            .start_agent(&component.id, agent_id!("StreamingRpcTarget", name.clone()))
            .await?;
        let caller = AgentId::from_agent_id(component.id, &agent_id!("StreamingRpcCaller", name))
            .map_err(anyhow::Error::msg)?;
        let fingerprint = executor.get_worker_metadata(&target).await?.fingerprint.0;
        let start = origin_observer_start(
            &executor,
            &component,
            &target,
            &caller,
            fingerprint,
            &IdempotencyKey::fresh(),
            "consume",
        );
        let (original_tx, mut original_rx, accepted) =
            open_raw_session(&executor, start.clone()).await?;
        let mapping = accepted.stream_mappings[0].clone();
        let stream_id = mapping
            .handle
            .as_ref()
            .and_then(|handle| handle.stream_id)
            .expect("stream id");
        let mut observer = start;
        observer.attempt_id = Some(uuid::Uuid::new_v4().into());
        observer.durable_input_mappings = accepted.stream_mappings;
        let (observer_tx, mut observer_rx, observer_accepted) =
            open_raw_session(&executor, observer).await?;
        assert!(observer_accepted.joined_origin_observer);
        if forbidden_control {
            observer_tx
                .send(InvocationRequest {
                    request: Some(invocation_request::Request::InputEnd(InputStreamEnd {
                        transport_stream_id: 1,
                        sequence: 0,
                        durable_stream_id: Some(stream_id),
                        epoch: accepted.epoch,
                    })),
                })
                .await?;
            let failed = tokio::time::timeout(Duration::from_secs(10), observer_rx.message())
                .await??
                .expect("observer protocol failure");
            assert!(
                matches!(failed.response, Some(invocation_response::Response::Finished(ref finished)) if matches!(finished.outcome, Some(invocation_session_completion::Outcome::Failure(_))))
            );
        } else {
            drop(observer_tx);
            drop(observer_rx);
        }
        original_tx
            .send(InvocationRequest {
                request: Some(invocation_request::Request::InputItem(InputStreamItem {
                    transport_stream_id: 1,
                    sequence: 0,
                    payload: Some(input_stream_item::Payload::Value(
                        SchemaValue::U32(7).try_into().map_err(anyhow::Error::msg)?,
                    )),
                    durable_stream_id: Some(stream_id),
                    epoch: accepted.epoch,
                })),
            })
            .await?;
        original_tx
            .send(InvocationRequest {
                request: Some(invocation_request::Request::InputEnd(InputStreamEnd {
                    transport_stream_id: 1,
                    sequence: 1,
                    durable_stream_id: Some(stream_id),
                    epoch: accepted.epoch,
                })),
            })
            .await?;
        let result = tokio::time::timeout(Duration::from_secs(20), async {
            loop {
                let response = original_rx
                    .message()
                    .await?
                    .ok_or_else(|| anyhow::anyhow!("original closed before success"))?;
                if let Some(invocation_response::Response::Result(result)) = response.response {
                    return Ok::<_, anyhow::Error>(result);
                }
            }
        })
        .await??;
        let value = match result.result {
            Some(invocation_session_result::Result::MethodResult(value)) => {
                SchemaValue::try_from(value).map_err(anyhow::Error::msg)?
            }
            other => anyhow::bail!("unexpected result: {other:?}"),
        };
        assert_eq!(
            value,
            SchemaValue::List {
                elements: vec![SchemaValue::U32(7)],
            }
        );
    }
    Ok(())
}

#[test]
#[timeout("2 minutes")]
#[tracing::instrument]
async fn resident_ephemeral_streaming_input_resume_restores_lost_ack_high_water(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] agent_rpc_rust: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;
    let component = executor
        .component_dep(&context.default_environment_id, agent_rpc_rust)
        .store()
        .await?;
    let idempotency_key = IdempotencyKey::fresh();
    let final_agent_id = agent_id!("EphemeralStreamingRpcTarget", "resident-input-resume")
        .with_ephemeral_invocation_phantom(&idempotency_key)
        .map_err(anyhow::Error::msg)?;
    let worker_agent_id = executor.start_agent(&component.id, final_agent_id).await?;
    let metadata = executor.get_worker_metadata(&worker_agent_id).await?;
    let input = golem_api_grpc::proto::golem::schema::SchemaValue {
        value: Some(schema_value::Value::RecordValue(RecordValue {
            fields: vec![golem_api_grpc::proto::golem::schema::SchemaValue {
                value: Some(schema_value::Value::StreamReference(
                    SchemaValueStreamReference { stream_id: 1 },
                )),
            }],
        })),
    };
    let start_request = InvocationRequest {
        request: Some(invocation_request::Request::Start(InvocationStart {
            agent_id: Some(worker_agent_id.clone().into()),
            method_name: Some("transform".to_string()),
            input: Some(input),
            idempotency_key: Some(idempotency_key.into()),
            auth_ctx: Some(executor.auth_ctx().into()),
            environment_id: Some(component.environment_id.into()),
            component_owner_account_id: Some(component.account_id.into()),
            mode: golem_api_grpc::proto::golem::worker::AgentInvocationMode::Await as i32,
            freshness_disposition:
                golem_api_grpc::proto::golem::worker::InvocationFreshnessDisposition::MayExist
                    as i32,
            attempt_id: Some(uuid::Uuid::new_v4().into()),
            expected_callee_fingerprint: Some(metadata.fingerprint.0.into()),
            ..Default::default()
        })),
    };
    let mut first_state = InvocationSessionState::default();
    first_state
        .validate_trusted_request(&start_request)
        .map_err(anyhow::Error::msg)?;
    let (requests, receiver) = mpsc::channel(8);
    requests.send(start_request.clone()).await?;
    let mut responses = executor
        .client
        .clone()
        .invoke_agent_session(ReceiverStream::new(receiver))
        .await?
        .into_inner();
    let accepted = responses
        .message()
        .await?
        .ok_or_else(|| anyhow::anyhow!("ephemeral input ended before acceptance"))?;
    first_state
        .validate_response(&accepted)
        .map_err(anyhow::Error::msg)?;
    let accepted = match accepted.response {
        Some(invocation_response::Response::Accepted(accepted)) => accepted,
        other => anyhow::bail!("expected ephemeral input acceptance, got {other:?}"),
    };
    let mapping = accepted
        .stream_mappings
        .first()
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("ephemeral acceptance omitted its input mapping"))?;
    let durable_stream_id = mapping
        .handle
        .as_ref()
        .and_then(|handle| handle.stream_id)
        .ok_or_else(|| anyhow::anyhow!("ephemeral input mapping omitted its stream ID"))?;
    let durable_stream_uuid: uuid::Uuid = durable_stream_id.into();
    let producer_fingerprint = golem_common::model::AgentFingerprint(
        mapping
            .handle
            .as_ref()
            .unwrap()
            .expected_producer_fingerprint
            .unwrap()
            .into(),
    );

    let make_item = |sequence, value| InvocationRequest {
        request: Some(invocation_request::Request::InputItem(InputStreamItem {
            transport_stream_id: 1,
            sequence,
            payload: Some(input_stream_item::Payload::Value(
                SchemaValue::U32(value)
                    .try_into()
                    .expect("u32 schema value"),
            )),
            durable_stream_id: Some(durable_stream_id),
            epoch: accepted.epoch,
        })),
    };
    let first_item = make_item(0, 7);
    first_state
        .validate_trusted_request(&first_item)
        .map_err(anyhow::Error::msg)?;
    requests.send(first_item).await?;
    let mut values = Vec::<SchemaValue>::new();
    let mut offsets = BTreeSet::new();
    let first_ack = loop {
        let response = responses
            .message()
            .await?
            .ok_or_else(|| anyhow::anyhow!("ephemeral input ended before its first ACK"))?;
        first_state
            .validate_response(&response)
            .map_err(anyhow::Error::msg)?;
        match response.response {
            Some(invocation_response::Response::InputAck(ack)) => break ack,
            // The empty-cursor resume below observes output from the beginning.
            Some(invocation_response::Response::Result(_))
            | Some(invocation_response::Response::OutputItem(_)) => {}
            other => {
                anyhow::bail!("unexpected response before first ephemeral input ACK: {other:?}")
            }
        }
    };
    assert_eq!(first_ack.highest_contiguous_sequence, 0);
    assert_eq!(first_ack.logical_item_count, 1);
    assert!(!first_ack.resulting_offset.is_empty());

    let lost_ack_item = make_item(1, 19);
    first_state
        .validate_trusted_request(&lost_ack_item)
        .map_err(anyhow::Error::msg)?;
    requests.send(lost_ack_item).await?;
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let second_input_was_recorded = executor
                .get_oplog(&worker_agent_id, OplogIndex::INITIAL)
                .await?
                .into_iter()
                .any(|entry| {
                    matches!(
                        entry.entry,
                        PublicOplogEntry::StreamItems(params)
                            if matches!(
                                StreamItemsRecord::from_value(params.record.value()),
                                Ok(record)
                                    if golem_common::model::durable_stream::StreamId::derive(
                                        context.default_environment_id,
                                        &worker_agent_id,
                                        producer_fingerprint,
                                        record.stream_id.0,
                                    ).map(|id| id.0).ok() == Some(durable_stream_uuid)
                                        && record.first_sequence == 1
                            )
                    )
                });
            if second_input_was_recorded {
                return Ok::<(), anyhow::Error>(());
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .map_err(|_| anyhow::anyhow!("second input was not accepted before transport loss"))??;
    drop(requests);
    drop(responses);

    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if executor
                .get_oplog(&worker_agent_id, OplogIndex::INITIAL)
                .await?
                .into_iter()
                .any(|entry| {
                    matches!(
                        entry.entry,
                        PublicOplogEntry::StreamSession(session)
                            if matches!(
                                StreamSessionRecord::from_value(session.record.value()),
                                Ok(StreamSessionRecord::Detached(record))
                                    if record.epoch == accepted.epoch
                            )
                    )
                })
            {
                return Ok::<(), anyhow::Error>(());
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .map_err(|_| anyhow::anyhow!("ephemeral input session did not detach before resume"))??;

    let Some(invocation_request::Request::Start(start)) = start_request.request.as_ref() else {
        anyhow::bail!("ephemeral input request is not Start");
    };
    let resume_request = InvocationRequest {
        request: Some(invocation_request::Request::ResumeAttach(ResumeAttach {
            idempotency_key: start.idempotency_key.clone(),
            agent_id: accepted.agent_id.clone(),
            environment_id: start.environment_id,
            attachment_id: accepted.attachment_id,
            attempt_id: Some(uuid::Uuid::new_v4().into()),
            expected_callee_fingerprint: start.expected_callee_fingerprint,
            expected_epoch: accepted.epoch,
            operation: ResumeOperation::Resume as i32,
            cursors: Vec::new(),
            auth_ctx: start.auth_ctx.clone(),
            principal: start.principal.clone(),
        })),
    };
    let mut resumed_state = InvocationSessionState::default();
    resumed_state
        .validate_trusted_request(&resume_request)
        .map_err(anyhow::Error::msg)?;
    let (requests, receiver) = mpsc::channel(8);
    requests.send(resume_request).await?;
    let mut responses = executor
        .client
        .clone()
        .invoke_agent_session(ReceiverStream::new(receiver))
        .await?
        .into_inner();
    let resumed = responses
        .message()
        .await?
        .ok_or_else(|| anyhow::anyhow!("ephemeral resume ended before acceptance"))?;
    resumed_state
        .validate_response(&resumed)
        .map_err(anyhow::Error::msg)?;
    let resumed = match resumed.response {
        Some(invocation_response::Response::Accepted(accepted)) => accepted,
        other => anyhow::bail!("expected resumed ephemeral acceptance, got {other:?}"),
    };
    assert_eq!(resumed.attachment_id, accepted.attachment_id);
    assert_eq!(resumed.epoch, accepted.epoch + 1);
    let resumed_mapping = resumed
        .stream_mappings
        .first()
        .ok_or_else(|| anyhow::anyhow!("resumed acceptance omitted its input mapping"))?;
    assert_eq!(resumed_mapping.handle, mapping.handle);
    assert!(matches!(
        &resumed_mapping.high_water,
        Some(high_water)
            if high_water.highest_contiguous_sequence == 1
                && high_water.resulting_offset != first_ack.resulting_offset
                && !high_water.terminal
    ));

    let final_item = InvocationRequest {
        request: Some(invocation_request::Request::InputItem(InputStreamItem {
            transport_stream_id: 1,
            sequence: 2,
            payload: Some(input_stream_item::Payload::Value(
                SchemaValue::U32(3).try_into().map_err(anyhow::Error::msg)?,
            )),
            durable_stream_id: Some(durable_stream_id),
            epoch: resumed.epoch,
        })),
    };
    resumed_state
        .validate_trusted_request(&final_item)
        .map_err(anyhow::Error::msg)?;
    requests.send(final_item).await?;
    let end = InvocationRequest {
        request: Some(invocation_request::Request::InputEnd(InputStreamEnd {
            transport_stream_id: 1,
            sequence: 3,
            durable_stream_id: Some(durable_stream_id),
            epoch: resumed.epoch,
        })),
    };
    resumed_state
        .validate_trusted_request(&end)
        .map_err(anyhow::Error::msg)?;
    requests.send(end).await?;

    while !resumed_state.is_complete() {
        let response = responses
            .message()
            .await?
            .ok_or_else(|| anyhow::anyhow!("resumed ephemeral input closed before completion"))?;
        resumed_state
            .validate_response(&response)
            .map_err(anyhow::Error::msg)?;
        match response.response {
            Some(invocation_response::Response::OutputItem(item)) => {
                assert!(
                    offsets.insert(item.durable_offset),
                    "duplicate output offset"
                );
                values.push(
                    item.value
                        .ok_or_else(|| anyhow::anyhow!("output omitted its value"))?
                        .try_into()
                        .map_err(anyhow::Error::msg)?,
                );
            }
            Some(
                invocation_response::Response::Accepted(_)
                | invocation_response::Response::InputAck(_)
                | invocation_response::Response::Result(_)
                | invocation_response::Response::OutputEnd(_),
            ) => {}
            Some(invocation_response::Response::Finished(finished)) => assert!(matches!(
                finished.outcome,
                Some(invocation_session_completion::Outcome::Success(_))
            )),
            Some(other) => anyhow::bail!("unexpected resumed input response: {other:?}"),
            None => anyhow::bail!("empty resumed input response"),
        }
    }
    assert_eq!(
        values,
        vec![
            SchemaValue::U32(70),
            SchemaValue::U32(190),
            SchemaValue::U32(30)
        ]
    );
    assert_eq!(offsets.len(), 3);
    let starts = executor
        .get_oplog(&worker_agent_id, OplogIndex::INITIAL)
        .await?
        .into_iter()
        .filter(|entry| {
            matches!(
                &entry.entry,
                PublicOplogEntry::AgentInvocationStarted(started)
                    if matches!(
                        &started.invocation,
                        PublicAgentInvocation::AgentMethodInvocation(method)
                            if method.method_name == "transform"
                    )
            )
        })
        .count();
    assert_eq!(starts, 1, "ResumeAttach must not restart transform");
    Ok(())
}

#[test]
#[timeout("2 minutes")]
#[tracing::instrument]
async fn durable_streaming_input_recovers_after_executor_restart(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] agent_rpc_rust: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;
    let component = executor
        .component_dep(&context.default_environment_id, agent_rpc_rust)
        .store()
        .await?;
    let agent_id = agent_id!("StreamingRpcTarget", "disconnect-after-acceptance");
    let worker_agent_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;
    let metadata = executor.get_worker_metadata(&worker_agent_id).await?;
    let input = golem_api_grpc::proto::golem::schema::SchemaValue {
        value: Some(schema_value::Value::RecordValue(RecordValue {
            fields: vec![golem_api_grpc::proto::golem::schema::SchemaValue {
                value: Some(schema_value::Value::StreamReference(
                    SchemaValueStreamReference { stream_id: 1 },
                )),
            }],
        })),
    };
    let idempotency_key = Some(IdempotencyKey::fresh().into());
    let attempt_id = Some(uuid::Uuid::new_v4().into());
    let start_request = InvocationRequest {
        request: Some(invocation_request::Request::Start(InvocationStart {
            agent_id: Some(worker_agent_id.clone().into()),
            method_name: Some("consume".to_string()),
            input: Some(input),
            idempotency_key,
            auth_ctx: Some(executor.auth_ctx().into()),
            environment_id: Some(component.environment_id.into()),
            component_owner_account_id: Some(component.account_id.into()),
            mode: golem_api_grpc::proto::golem::worker::AgentInvocationMode::Await as i32,
            freshness_disposition:
                golem_api_grpc::proto::golem::worker::InvocationFreshnessDisposition::MayExist
                    as i32,
            attempt_id,
            expected_callee_fingerprint: Some(metadata.fingerprint.0.into()),
            ..Default::default()
        })),
    };

    let mut state = InvocationSessionState::default();
    state
        .validate_trusted_request(&start_request)
        .map_err(anyhow::Error::msg)?;
    let (requests, receiver) = mpsc::channel(8);
    requests.send(start_request.clone()).await?;
    let mut responses = executor
        .client
        .clone()
        .invoke_agent_session(ReceiverStream::new(receiver))
        .await?
        .into_inner();
    let first = responses
        .message()
        .await?
        .ok_or_else(|| anyhow::anyhow!("streaming invocation ended before acceptance"))?;
    state
        .validate_response(&first)
        .map_err(anyhow::Error::msg)?;
    let first_accepted = match first.response {
        Some(invocation_response::Response::Accepted(accepted)) => accepted,
        other => anyhow::bail!("expected durable acceptance, got {other:?}"),
    };
    let first_mapping = first_accepted
        .stream_mappings
        .first()
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("durable acceptance omitted its input mapping"))?;
    let first_durable_stream_id = first_mapping
        .handle
        .as_ref()
        .and_then(|handle| handle.stream_id)
        .ok_or_else(|| anyhow::anyhow!("durable input mapping omitted its stream ID"))?;
    let first_item = InvocationRequest {
        request: Some(invocation_request::Request::InputItem(InputStreamItem {
            transport_stream_id: 1,
            sequence: 0,
            payload: Some(input_stream_item::Payload::Value(
                SchemaValue::U32(1).try_into().map_err(anyhow::Error::msg)?,
            )),
            durable_stream_id: Some(first_durable_stream_id),
            epoch: first_accepted.epoch,
        })),
    };
    state
        .validate_trusted_request(&first_item)
        .map_err(anyhow::Error::msg)?;
    requests.send(first_item.clone()).await?;
    let first_ack = responses
        .message()
        .await?
        .ok_or_else(|| anyhow::anyhow!("durable input ended before its first ACK"))?;
    state
        .validate_response(&first_ack)
        .map_err(anyhow::Error::msg)?;
    let first_ack_value = match &first_ack.response {
        Some(invocation_response::Response::InputAck(ack)) => ack.clone(),
        other => anyhow::bail!("expected first durable input ACK, got {other:?}"),
    };

    for (sequence, value) in [(1_u64, 2_u32), (2, 3)] {
        let item = InvocationRequest {
            request: Some(invocation_request::Request::InputItem(InputStreamItem {
                transport_stream_id: 1,
                sequence,
                payload: Some(input_stream_item::Payload::Value(
                    SchemaValue::U32(value)
                        .try_into()
                        .map_err(anyhow::Error::msg)?,
                )),
                durable_stream_id: Some(first_durable_stream_id),
                epoch: first_accepted.epoch,
            })),
        };
        state
            .validate_trusted_request(&item)
            .map_err(anyhow::Error::msg)?;
        requests.send(item.clone()).await?;
        let ack = responses
            .message()
            .await?
            .ok_or_else(|| anyhow::anyhow!("durable input ended before its ACK"))?;
        state.validate_response(&ack).map_err(anyhow::Error::msg)?;
        assert!(matches!(
            &ack.response,
            Some(invocation_response::Response::InputAck(ack))
                if ack.highest_contiguous_sequence == sequence
                    && ack.logical_item_count == 1
                    && !ack.resulting_offset.is_empty()
        ));
    }
    let end = InvocationRequest {
        request: Some(invocation_request::Request::InputEnd(InputStreamEnd {
            transport_stream_id: 1,
            sequence: 3,
            durable_stream_id: Some(first_durable_stream_id),
            epoch: first_accepted.epoch,
        })),
    };
    state
        .validate_trusted_request(&end)
        .map_err(anyhow::Error::msg)?;
    requests.send(end).await?;
    drop(requests);

    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let oplog = executor
                .get_oplog(&worker_agent_id, OplogIndex::INITIAL)
                .await?;
            let terminal_committed = oplog
                .iter()
                .any(|entry| matches!(entry.entry, PublicOplogEntry::StreamEnd(_)));
            let invocation_finished = oplog
                .iter()
                .any(|entry| matches!(entry.entry, PublicOplogEntry::AgentInvocationFinished(_)));
            if terminal_committed && invocation_finished {
                return Ok::<(), anyhow::Error>(());
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .map_err(|_| anyhow::anyhow!("invocation did not finish with its terminal ACK unread"))??;
    drop(responses);
    drop(executor);

    let executor = golem_worker_executor_test_utils::start(deps, &context).await?;
    let Some(invocation_request::Request::Start(start)) = start_request.request.as_ref() else {
        anyhow::bail!("durable input restart request is not Start");
    };
    let resume_request = InvocationRequest {
        request: Some(invocation_request::Request::ResumeAttach(ResumeAttach {
            idempotency_key: start.idempotency_key.clone(),
            agent_id: start.agent_id.clone(),
            environment_id: start.environment_id,
            attachment_id: first_accepted.attachment_id,
            attempt_id: Some(uuid::Uuid::new_v4().into()),
            expected_callee_fingerprint: start.expected_callee_fingerprint,
            expected_epoch: first_accepted.epoch,
            operation: ResumeOperation::Resume as i32,
            cursors: Vec::new(),
            auth_ctx: start.auth_ctx.clone(),
            principal: start.principal.clone(),
        })),
    };
    let mut final_state = InvocationSessionState::default();
    final_state
        .validate_trusted_request(&resume_request)
        .map_err(anyhow::Error::msg)?;
    let (requests, receiver) = mpsc::channel(8);
    requests.send(resume_request).await?;
    let mut responses = executor
        .client
        .clone()
        .invoke_agent_session(ReceiverStream::new(receiver))
        .await?
        .into_inner();
    let accepted = responses
        .message()
        .await?
        .ok_or_else(|| anyhow::anyhow!("terminal retry ended before acceptance"))?;
    final_state
        .validate_response(&accepted)
        .map_err(anyhow::Error::msg)?;
    let accepted = match accepted.response {
        Some(invocation_response::Response::Accepted(accepted)) => accepted,
        other => anyhow::bail!("expected terminal retry acceptance, got {other:?}"),
    };
    assert_eq!(accepted.attachment_id, first_accepted.attachment_id);
    assert_ne!(accepted.attempt_id, first_accepted.attempt_id);
    assert_eq!(accepted.epoch, first_accepted.epoch + 1);
    assert_eq!(accepted.stream_mappings.len(), 1);
    let mapping = accepted
        .stream_mappings
        .first()
        .ok_or_else(|| anyhow::anyhow!("terminal retry acceptance omitted its input mapping"))?;
    assert_eq!(mapping.transport_stream_id, 1);
    assert_eq!(mapping.handle, first_mapping.handle);
    assert!(matches!(
        &mapping.high_water,
        Some(high_water)
            if high_water.highest_contiguous_sequence == 3
                && high_water.resulting_offset != first_ack_value.resulting_offset
                && high_water.terminal
    ));

    let mut result = None;
    let mut terminal_ack_count = 0;
    while !final_state.is_complete() {
        let response = responses
            .message()
            .await?
            .ok_or_else(|| anyhow::anyhow!("terminal retry closed before completion"))?;
        final_state
            .validate_response(&response)
            .map_err(anyhow::Error::msg)?;
        match response.response {
            Some(invocation_response::Response::InputAck(ack)) => {
                terminal_ack_count += 1;
                assert_eq!(ack.highest_contiguous_sequence, 3);
                assert_eq!(ack.logical_item_count, 1);
                assert!(!ack.resulting_offset.is_empty());
            }
            Some(invocation_response::Response::Result(invocation_result)) => {
                let value = match invocation_result.result {
                    Some(invocation_session_result::Result::MethodResult(value)) => value,
                    other => anyhow::bail!("expected method result, got {other:?}"),
                };
                result = Some(SchemaValue::try_from(value).map_err(anyhow::Error::msg)?);
            }
            Some(invocation_response::Response::Finished(finished)) => assert!(
                matches!(
                    finished.outcome,
                    Some(invocation_session_completion::Outcome::Success(_))
                ),
                "unexpected terminal retry completion: {finished:?}"
            ),
            Some(other) => anyhow::bail!("unexpected terminal retry response: {other:?}"),
            None => anyhow::bail!("empty terminal retry response"),
        }
    }
    assert_eq!(terminal_ack_count, 0);
    assert_eq!(
        result,
        Some(SchemaValue::List {
            elements: vec![
                SchemaValue::U32(1),
                SchemaValue::U32(2),
                SchemaValue::U32(3)
            ],
        })
    );

    let oplog = executor
        .get_oplog(&worker_agent_id, OplogIndex::INITIAL)
        .await?;
    assert!(
        oplog.iter().any(|entry| matches!(
            &entry.entry,
            PublicOplogEntry::AgentInvocationStarted(started)
                if matches!(
                    &started.invocation,
                    PublicAgentInvocation::AgentMethodInvocation(method)
                        if method.method_name == "consume"
                )
        )),
        "recovered durable invocation was not journaled"
    );
    Ok(())
}

#[test]
#[timeout("2 minutes")]
#[tracing::instrument]
async fn resuming_a_finished_session_with_guest_cancelled_input_replays_completion(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] agent_rpc_rust: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;
    let component = executor
        .component_dep(&context.default_environment_id, agent_rpc_rust)
        .store()
        .await?;
    let agent_id = agent_id!("StreamingRpcTarget", "resume-after-guest-drop");
    let worker_agent_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;
    let metadata = executor.get_worker_metadata(&worker_agent_id).await?;
    let input = golem_api_grpc::proto::golem::schema::SchemaValue {
        value: Some(schema_value::Value::RecordValue(RecordValue {
            fields: vec![golem_api_grpc::proto::golem::schema::SchemaValue {
                value: Some(schema_value::Value::StreamReference(
                    SchemaValueStreamReference { stream_id: 1 },
                )),
            }],
        })),
    };
    let start_request = InvocationRequest {
        request: Some(invocation_request::Request::Start(InvocationStart {
            agent_id: Some(worker_agent_id.clone().into()),
            method_name: Some("drop_input".to_string()),
            input: Some(input),
            idempotency_key: Some(IdempotencyKey::fresh().into()),
            auth_ctx: Some(executor.auth_ctx().into()),
            environment_id: Some(component.environment_id.into()),
            component_owner_account_id: Some(component.account_id.into()),
            mode: golem_api_grpc::proto::golem::worker::AgentInvocationMode::Await as i32,
            freshness_disposition:
                golem_api_grpc::proto::golem::worker::InvocationFreshnessDisposition::MayExist
                    as i32,
            attempt_id: Some(uuid::Uuid::new_v4().into()),
            expected_callee_fingerprint: Some(metadata.fingerprint.0.into()),
            ..Default::default()
        })),
    };

    let mut state = InvocationSessionState::default();
    state
        .validate_trusted_request(&start_request)
        .map_err(anyhow::Error::msg)?;
    let (requests, receiver) = mpsc::channel(8);
    requests.send(start_request.clone()).await?;
    let mut responses = executor
        .client
        .clone()
        .invoke_agent_session(ReceiverStream::new(receiver))
        .await?
        .into_inner();
    let first = responses
        .message()
        .await?
        .ok_or_else(|| anyhow::anyhow!("streaming invocation ended before acceptance"))?;
    state
        .validate_response(&first)
        .map_err(anyhow::Error::msg)?;
    let first_accepted = match first.response {
        Some(invocation_response::Response::Accepted(accepted)) => accepted,
        other => anyhow::bail!("expected durable acceptance, got {other:?}"),
    };
    let first_mapping = first_accepted
        .stream_mappings
        .first()
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("durable acceptance omitted its input mapping"))?;

    let mut first_input_cancelled = false;
    let mut first_result = None;
    while !state.is_complete() {
        let response = responses
            .message()
            .await?
            .ok_or_else(|| anyhow::anyhow!("first attempt closed before completion"))?;
        state
            .validate_response(&response)
            .map_err(anyhow::Error::msg)?;
        match response.response {
            Some(invocation_response::Response::StreamCancel(cancel)) => {
                assert_eq!(cancel.transport_stream_id, 1);
                assert_eq!(cancel.role, StreamCancelRole::InputConsumer as i32);
                first_input_cancelled = true;
            }
            Some(invocation_response::Response::Result(invocation_result)) => {
                let value = match invocation_result.result {
                    Some(invocation_session_result::Result::MethodResult(value)) => value,
                    other => anyhow::bail!("expected method result, got {other:?}"),
                };
                first_result = Some(SchemaValue::try_from(value).map_err(anyhow::Error::msg)?);
            }
            Some(invocation_response::Response::Finished(finished)) => assert!(
                matches!(
                    finished.outcome,
                    Some(invocation_session_completion::Outcome::Success(_))
                ),
                "unexpected first attempt completion: {finished:?}"
            ),
            Some(other) => anyhow::bail!("unexpected first attempt response: {other:?}"),
            None => anyhow::bail!("empty first attempt response"),
        }
    }
    assert!(
        first_input_cancelled,
        "the guest drop did not cancel the input stream"
    );
    assert_eq!(first_result, Some(SchemaValue::U64(42)));
    drop(requests);
    drop(responses);

    // Finished precedes asynchronous attachment cleanup; Resume requires the persisted detach.
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            for entry in executor
                .get_oplog(&worker_agent_id, OplogIndex::INITIAL)
                .await?
            {
                if let PublicOplogEntry::StreamSession(session) = entry.entry
                    && matches!(
                        StreamSessionRecord::from_value(session.record.value())
                            .map_err(anyhow::Error::msg)?,
                        StreamSessionRecord::Detached(record)
                            if record.epoch == first_accepted.epoch
                    )
                {
                    return Ok::<(), anyhow::Error>(());
                }
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .map_err(|_| anyhow::anyhow!("finished session did not detach before resume"))??;

    let Some(invocation_request::Request::Start(start)) = start_request.request.as_ref() else {
        anyhow::bail!("durable input resume request is not Start");
    };
    let resume_request = InvocationRequest {
        request: Some(invocation_request::Request::ResumeAttach(ResumeAttach {
            idempotency_key: start.idempotency_key.clone(),
            agent_id: start.agent_id.clone(),
            environment_id: start.environment_id,
            attachment_id: first_accepted.attachment_id,
            attempt_id: Some(uuid::Uuid::new_v4().into()),
            expected_callee_fingerprint: start.expected_callee_fingerprint,
            expected_epoch: first_accepted.epoch,
            operation: ResumeOperation::Resume as i32,
            cursors: Vec::new(),
            auth_ctx: start.auth_ctx.clone(),
            principal: start.principal.clone(),
        })),
    };
    let mut final_state = InvocationSessionState::default();
    final_state
        .validate_trusted_request(&resume_request)
        .map_err(anyhow::Error::msg)?;
    let (requests, receiver) = mpsc::channel(8);
    requests.send(resume_request).await?;
    let mut responses = executor
        .client
        .clone()
        .invoke_agent_session(ReceiverStream::new(receiver))
        .await?
        .into_inner();
    let accepted = responses
        .message()
        .await?
        .ok_or_else(|| anyhow::anyhow!("resume ended before acceptance"))?;
    final_state
        .validate_response(&accepted)
        .map_err(anyhow::Error::msg)?;
    let accepted = match accepted.response {
        Some(invocation_response::Response::Accepted(accepted)) => accepted,
        other => anyhow::bail!("expected resume acceptance, got {other:?}"),
    };
    assert_eq!(accepted.attachment_id, first_accepted.attachment_id);
    assert_ne!(accepted.attempt_id, first_accepted.attempt_id);
    assert_eq!(accepted.epoch, first_accepted.epoch + 1);
    assert_eq!(accepted.stream_mappings.len(), 1);
    let mapping = accepted
        .stream_mappings
        .first()
        .ok_or_else(|| anyhow::anyhow!("resume acceptance omitted its input mapping"))?;
    assert_eq!(mapping.transport_stream_id, 1);
    assert_eq!(mapping.handle, first_mapping.handle);
    assert!(
        matches!(&mapping.high_water, Some(high_water) if high_water.terminal),
        "resume acceptance did not announce the cancelled input as terminal: {mapping:?}"
    );

    let mut result = None;
    while !final_state.is_complete() {
        let response = responses
            .message()
            .await?
            .ok_or_else(|| anyhow::anyhow!("resumed session closed before completion"))?;
        final_state
            .validate_response(&response)
            .map_err(anyhow::Error::msg)?;
        match response.response {
            Some(invocation_response::Response::Result(invocation_result)) => {
                let value = match invocation_result.result {
                    Some(invocation_session_result::Result::MethodResult(value)) => value,
                    other => anyhow::bail!("expected method result, got {other:?}"),
                };
                result = Some(SchemaValue::try_from(value).map_err(anyhow::Error::msg)?);
            }
            Some(invocation_response::Response::Finished(finished)) => assert!(
                matches!(
                    finished.outcome,
                    Some(invocation_session_completion::Outcome::Success(_))
                ),
                "unexpected resumed completion: {finished:?}"
            ),
            Some(other) => anyhow::bail!("unexpected resumed session response: {other:?}"),
            None => anyhow::bail!("empty resumed session response"),
        }
    }
    assert_eq!(result, Some(SchemaValue::U64(42)));
    Ok(())
}

#[test]
#[timeout("2 minutes")]
#[tracing::instrument]
async fn reacquire_permits_restart_preserves_accepted_queued_live_invocation(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("large_dynamic_memory")] large_dynamic_memory: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    const EXECUTOR_MEMORY_BYTES: u64 = 32 * 1024 * 1024;
    const GROWTH_MIB: u64 = 30;
    const QUEUE_GATE_MILLIS: u64 = 5_000;

    let context = TestContext::new(last_unique_id);
    let executor = start_with_overrides(
        deps,
        &context,
        TestExecutorOverrides {
            configure: Some(Arc::new(|config| {
                config.memory.system_memory_override = Some(EXECUTOR_MEMORY_BYTES);
                config.memory.worker_memory_ratio = 1.0;
                config.memory.component_size_coefficient = 0.0;
            })),
            ..Default::default()
        },
    )
    .await?;
    let component = executor
        .component_dep(&context.default_environment_id, large_dynamic_memory)
        .store()
        .await?;

    let victim_agent = agent_id!("LargeDynamicMemoryAgent", "live-queue-reacquire-victim");
    let victim_worker = executor
        .start_agent(&component.id, victim_agent.clone())
        .await?;
    let victim_owned = OwnedAgentId::new(context.default_environment_id, &victim_worker);
    tokio::time::timeout(Duration::from_secs(10), async {
        while executor.worker_eviction_class(&victim_owned).await != Some(EvictionClass::LoadedIdle)
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the co-resident worker must become idle and evictable");

    let target_agent = agent_id!("LargeDynamicMemoryAgent", "live-queue-reacquire-target");
    let target_worker = executor
        .start_agent(&component.id, target_agent.clone())
        .await?;
    let target_owned = OwnedAgentId::new(context.default_environment_id, &target_worker);
    let victim_bytes = executor.worker_memory_requirement(&victim_owned).await?;
    let target_bytes = executor.worker_memory_requirement(&target_owned).await?;
    let growth_bytes = GROWTH_MIB * 1024 * 1024;
    assert!(victim_bytes + target_bytes <= EXECUTOR_MEMORY_BYTES);
    assert!(target_bytes + growth_bytes <= EXECUTOR_MEMORY_BYTES);
    assert!(victim_bytes + target_bytes + growth_bytes > EXECUTOR_MEMORY_BYTES);

    let blocker_executor = executor.clone();
    let blocker_component = component.clone();
    let blocker_agent = target_agent.clone();
    let blocker = tokio::spawn(async move {
        blocker_executor
            .invoke_and_await_agent(
                &blocker_component,
                &blocker_agent,
                "run_with_memory_and_work",
                data_value!(0u64, QUEUE_GATE_MILLIS),
            )
            .await
    });
    executor
        .wait_for_status(
            &target_worker,
            AgentStatus::Running,
            Duration::from_secs(10),
        )
        .await?;

    executor
        .invoke_agent(
            &component,
            &target_agent,
            "run_with_memory_and_work",
            data_value!(GROWTH_MIB, 0u64),
        )
        .await?;

    let (mut state, _frames, mut inbound) = open_invocation_session(
        &executor,
        &component,
        &target_agent,
        "run_with_memory_and_work",
        data_value!(0u64, 0u64),
    )
    .await?;
    let accepted = tokio::time::timeout(Duration::from_secs(3), inbound.message())
        .await
        .map_err(|_| anyhow::anyhow!("live invocation was not accepted while durable work ran"))??
        .ok_or_else(|| anyhow::anyhow!("live invocation ended before acceptance"))?;
    state
        .validate_response(&accepted)
        .map_err(anyhow::Error::msg)?;
    assert!(matches!(
        accepted.response,
        Some(invocation_response::Response::Accepted(_))
    ));
    assert!(
        !blocker.is_finished(),
        "the queue gate ended before the live invocation was accepted"
    );

    let blocker_result = blocker.await??;
    assert_eq!(blocker_result.into_typed::<u64>()?, 0);

    let live_result = tokio::time::timeout(
        Duration::from_secs(60),
        receive_invocation_session(&mut state, &mut inbound),
    )
    .await
    .map_err(|_| {
        anyhow::anyhow!("accepted live invocation was stranded by permit reacquisition restart")
    })??
    .map_err(anyhow::Error::msg)?;
    assert_eq!(live_result, SchemaValue::U64(0));
    assert!(
        !executor.worker_is_loaded(&victim_owned).await,
        "the durable growth must force permit reacquisition and evict the idle worker"
    );

    Ok(())
}

#[test]
#[timeout("120s")]
async fn fork_and_revert_streaming_rpc_join_the_original_remote_invocation(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] agent_rpc_rust: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    for remote in [false, true] {
        let context = TestContext::new(last_unique_id);
        let (mut executor, mut remote_evidence) = if remote {
            let (executor, evidence) =
                crate::fork::start_with_remote_streaming_rpc(deps, &context).await?;
            (executor, Some(evidence))
        } else {
            (
                crate::fork::start_with_local_resume(deps, &context, false).await?,
                None,
            )
        };
        let component = executor
            .component_dep(&context.default_environment_id, agent_rpc_rust)
            .store()
            .await?;
        for (synchronous, cut_point) in [
            (false, "start"),
            (false, "caller-attempt"),
            (false, "end"),
            (true, "start"),
            (true, "caller-attempt"),
            (true, "end"),
        ] {
            let name = format!("fork-rpc-{remote}-{synchronous}-{cut_point}");
            let caller = agent_id!("StreamingRpcCaller", name.clone());
            let provider = agent_id!("StreamingRpcTarget", name.clone());
            let caller_id = executor.start_agent(&component.id, caller.clone()).await?;
            let provider_id = executor
                .start_agent(&component.id, provider.clone())
                .await?;
            let key = IdempotencyKey::fresh();
            assert_eq!(
                executor
                    .invoke_and_await_agent_with_key(
                        &component,
                        &caller,
                        &key,
                        "streaming_increment",
                        data_value!(synchronous),
                    )
                    .await?
                    .into_typed::<Vec<u64>>()?,
                vec![1]
            );
            let history = executor.get_oplog(&caller_id, OplogIndex::INITIAL).await?;
            let (start_index, request) = history
                .iter()
                .find_map(|entry| match &entry.entry {
                    PublicOplogEntry::Start(start)
                        if start.function_name == "golem::rpc::wasm-rpc::invoke_and_await" =>
                    {
                        Some((
                            entry.oplog_index,
                            HostRequestGolemRpcInvoke::from_value(
                                start.request.as_ref().unwrap().value(),
                            )
                            .unwrap(),
                        ))
                    }
                    _ => None,
                })
                .expect("streaming RPC Start");
            let cut = match cut_point {
                "end" => history
                    .iter()
                    .find_map(|entry| match &entry.entry {
                        PublicOplogEntry::End(end) if end.start_index == start_index => {
                            Some(entry.oplog_index)
                        }
                        _ => None,
                    })
                    .expect("streaming RPC End"),
                "caller-attempt" => history
                    .iter()
                    .find_map(|entry| match &entry.entry {
                        PublicOplogEntry::StreamSession(session)
                            if matches!(
                                StreamSessionRecord::from_value(session.record.value()).unwrap(),
                                StreamSessionRecord::CallerAttempt(_)
                            ) =>
                        {
                            Some(entry.oplog_index)
                        }
                        _ => None,
                    })
                    .expect("streaming RPC caller attempt"),
                _ => start_index,
            };
            let fork =
                golem_common::phantom_agent_id!("StreamingRpcCaller", uuid::Uuid::new_v4(), name);
            executor
                .fork_worker(&caller_id, &fork.to_string(), cut)
                .await?;
            assert_eq!(
                executor
                    .invoke_and_await_agent_with_key(
                        &component,
                        &fork,
                        &key,
                        "streaming_increment",
                        data_value!(synchronous),
                    )
                    .await?
                    .into_typed::<Vec<u64>>()?,
                vec![1],
                "the fork must join the original provider invocation"
            );
            if let Some(evidence) = &remote_evidence {
                assert!(evidence.sessions.load(Ordering::SeqCst) > 0);
                if cut_point != "end" {
                    assert!(
                        evidence.joined_observer_acceptances.load(Ordering::SeqCst) > 0,
                        "an incomplete forked call must observe the existing invocation"
                    );
                }
                assert_eq!(
                    evidence.resume_attach_requests.load(Ordering::SeqCst),
                    0,
                    "a fork must not take over the original caller attachment"
                );
            }
            executor
                .revert(
                    &caller_id,
                    golem_common::model::worker::RevertWorkerTarget::RevertToOplogIndex(
                        golem_common::model::worker::RevertToOplogIndex {
                            last_oplog_index: cut,
                        },
                    ),
                )
                .await?;
            assert_eq!(
                executor
                    .invoke_and_await_agent_with_key(
                        &component,
                        &caller,
                        &key,
                        "streaming_increment",
                        data_value!(synchronous),
                    )
                    .await?
                    .into_typed::<Vec<u64>>()?,
                vec![1],
                "the reverted caller must join the same provider invocation"
            );
            drop(executor);
            if remote {
                let (restarted, evidence) =
                    crate::fork::start_with_remote_streaming_rpc(deps, &context).await?;
                executor = restarted;
                remote_evidence = Some(evidence);
            } else {
                executor = crate::fork::start_with_local_resume(deps, &context, false).await?;
            }
            for agent in [&caller, &fork] {
                executor
                    .invoke_and_await_agent(&component, agent, "create_input_gate", data_value!())
                    .await?;
                assert_eq!(
                    executor
                        .invoke_and_await_agent_with_key(
                            &component,
                            agent,
                            &key,
                            "streaming_increment",
                            data_value!(synchronous),
                        )
                        .await?
                        .into_typed::<Vec<u64>>()?,
                    vec![1]
                );
            }
            assert_eq!(
                executor
                    .invoke_and_await_agent(&component, &provider, "scalar_value", data_value!())
                    .await?
                    .into_typed::<u64>()?,
                1,
            );
            let provider_history = executor
                .get_oplog(&provider_id, OplogIndex::INITIAL)
                .await?;
            let executions = provider_history
                .iter()
                .filter_map(|entry| match &entry.entry {
                    PublicOplogEntry::AgentInvocationStarted(started) => {
                        match &started.invocation {
                            PublicAgentInvocation::AgentMethodInvocation(method)
                                if method.method_name == "increment_stream_input"
                                    || method.method_name == "increment_stream" =>
                            {
                                Some(method.idempotency_key.clone())
                            }
                            _ => None,
                        }
                    }
                    _ => None,
                })
                .collect::<Vec<_>>();
            assert_eq!(executions, vec![request.idempotency_key]);
            assert_eq!(
                executor
                    .invoke_and_await_agent(
                        &component,
                        &provider,
                        "increment_scalar",
                        data_value!()
                    )
                    .await?
                    .into_typed::<u64>()?,
                2,
            );
            let fork_id =
                AgentId::from_agent_id(component.id, &fork).map_err(anyhow::Error::msg)?;
            for agent in [&caller_id, &fork_id, &provider_id] {
                let history = executor.get_oplog(agent, OplogIndex::INITIAL).await?;
                let errors = history
                    .iter()
                    .filter(|entry| matches!(entry.entry, PublicOplogEntry::Error(_)))
                    .collect::<Vec<_>>();
                assert!(
                    errors.is_empty(),
                    "reconstruction must succeed without trap retries: {agent}: {errors:?}"
                );
            }
        }
    }
    Ok(())
}

#[test]
#[timeout("120s")]
async fn forks_of_agent_rpc_outputs_finish_without_inherited_attachments(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] agent_rpc_rust: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use golem_api_grpc::proto::golem::workerexecutor::v1::{
        ForkStreamSlotRequest, ReadStreamSlotRequest, fork_stream_slot_response,
        read_stream_slot_response, stream_slot_item,
    };
    use prost::Message;

    let context = TestContext::new(last_unique_id);
    let executor = crate::fork::start_with_local_resume(deps, &context, false).await?;
    let component = executor
        .component_dep(&context.default_environment_id, agent_rpc_rust)
        .store()
        .await?;
    let name = "fork-agent-output";
    let caller = agent_id!("StreamingRpcCaller", name);
    let source = agent_id!("StreamingRpcTarget", name);
    executor.start_agent(&component.id, caller.clone()).await?;
    let source_id = executor.start_agent(&component.id, source.clone()).await?;
    assert_eq!(
        executor
            .invoke_and_await_agent(
                &component,
                &caller,
                "streaming_increment",
                data_value!(false)
            )
            .await?
            .into_typed::<Vec<u64>>()?,
        vec![1]
    );
    let history = executor.get_oplog(&source_id, OplogIndex::INITIAL).await?;
    let cut = history
        .iter()
        .find_map(|entry| {
            matches!(entry.entry, PublicOplogEntry::StreamItems(_)).then_some(entry.oplog_index)
        })
        .expect("committed output item");
    let session = history
        .iter()
        .find_map(|entry| {
            if let PublicOplogEntry::StreamSession(session) = &entry.entry
                && let StreamSessionRecord::Prepared(record) =
                    StreamSessionRecord::from_value(session.record.value()).unwrap()
            {
                return Some(record.session_key.value);
            }
            None
        })
        .expect("prepared streaming invocation");
    for exported in [false, true] {
        let target =
            golem_common::phantom_agent_id!("StreamingRpcTarget", uuid::Uuid::new_v4(), name);
        let target_id =
            AgentId::from_agent_id(component.id, &target).map_err(anyhow::Error::msg)?;
        if exported {
            let response = executor
                .client
                .clone()
                .fork_stream_slot(ForkStreamSlotRequest {
                    source_agent_id: Some(source_id.clone().into()),
                    target_agent_id: Some(target_id.clone().into()),
                    environment_id: Some(component.environment_id.into()),
                    auth_ctx: Some(AuthCtx::System.into()),
                    session: session.clone(),
                    slot: "$result".into(),
                    expected_method: "increment_stream_input".into(),
                    source_path: "/source/output".into(),
                    max_forks_per_session: 10,
                    max_forks_per_second: 100,
                    max_copied_bytes: 64 * 1024 * 1024,
                    ..Default::default()
                })
                .await?
                .into_inner();
            assert!(
                matches!(
                    response.result,
                    Some(fork_stream_slot_response::Result::Success(_))
                ),
                "{response:?}"
            );
        } else {
            executor
                .fork_worker(&source_id, &target.to_string(), cut)
                .await?;
        }
        // This queued invocation can finish only after the fork's open output has drained.
        assert_eq!(
            tokio::time::timeout(
                Duration::from_secs(20),
                executor.invoke_and_await_agent(
                    &component,
                    &target,
                    "increment_scalar",
                    data_value!()
                )
            )
            .await??
            .into_typed::<u64>()?,
            2
        );
        let read = executor
            .client
            .clone()
            .read_stream_slot(ReadStreamSlotRequest {
                agent_id: Some(target_id.clone().into()),
                environment_id: Some(component.environment_id.into()),
                auth_ctx: Some(AuthCtx::System.into()),
                session: session.clone(),
                slot: "$result".into(),
                expected_method: "increment_stream_input".into(),
                max_items: 100,
                max_bytes: 1_000_000,
                ..Default::default()
            })
            .await?
            .into_inner()
            .message()
            .await?
            .unwrap();
        let Some(read_stream_slot_response::Result::Success(read)) = read.result else {
            anyhow::bail!("{read:?}");
        };
        assert_eq!(read.items.len(), 1);
        let Some(stream_slot_item::Content::Value(bytes)) = &read.items[0].content else {
            anyhow::bail!("missing output value");
        };
        assert_eq!(
            SchemaValue::try_from(golem_api_grpc::proto::golem::schema::SchemaValue::decode(
                bytes.as_slice()
            )?)
            .map_err(anyhow::Error::msg)?,
            SchemaValue::U64(1)
        );
        let history = executor.get_oplog(&target_id, OplogIndex::INITIAL).await?;
        let fork_cut = history
            .iter()
            .find_map(|entry| {
                matches!(&entry.entry, PublicOplogEntry::StreamSession(session)
                    if matches!(StreamSessionRecord::from_value(session.record.value()).unwrap(),
                        StreamSessionRecord::ForkCut(_)))
                .then_some(entry.oplog_index)
            })
            .expect("fork cut marker");
        // Export forks retain the source terminal before reopening the selected stream.
        assert_eq!(
            history
                .iter()
                .filter(|entry| entry.oplog_index > fork_cut
                    && matches!(entry.entry, PublicOplogEntry::StreamEnd(_)))
                .count(),
            1
        );
        assert!(
            !history.iter().any(|entry| {
                matches!(&entry.entry, PublicOplogEntry::StreamSession(session)
                if matches!(StreamSessionRecord::from_value(session.record.value()).unwrap(),
                    StreamSessionRecord::AttachmentPrepared(_)
                    | StreamSessionRecord::AttachmentActivated(_)) && entry.oplog_index > fork_cut)
            }),
            "fork output must finish without a new consumer attachment"
        );
    }
    assert_eq!(
        executor
            .invoke_and_await_agent(&component, &source, "increment_scalar", data_value!())
            .await?
            .into_typed::<u64>()?,
        2
    );
    Ok(())
}

#[test]
#[timeout("120s")]
async fn fork_dropping_inherited_rpc_output_does_not_cancel_original_reader(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] agent_rpc_rust: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    for (cut_at_start, forwarded_drop) in
        [(true, false), (false, false), (true, true), (false, true)]
    {
        let context = TestContext::new(last_unique_id);
        let (executor, _) = crate::fork::start_with_remote_streaming_rpc(deps, &context).await?;
        let component = executor
            .component_dep(&context.default_environment_id, agent_rpc_rust)
            .store()
            .await?;
        let name = format!(
            "fork-drop-inherited-rpc-output-{}-{}",
            if cut_at_start { "start" } else { "end" },
            if forwarded_drop {
                "forwarded"
            } else {
                "direct"
            }
        );
        let original = agent_id!("StreamingRpcCaller", name);
        let provider = agent_id!("StreamingRpcTarget", name);
        let downstream = agent_id!("StreamingRpcTarget", format!("{name}-forwarded-drop"));
        let original_id = executor
            .start_agent(&component.id, original.clone())
            .await?;
        let provider_id = executor
            .start_agent(&component.id, provider.clone())
            .await?;
        let downstream_id = executor
            .start_agent(&component.id, downstream.clone())
            .await?;
        let gate = executor
            .invoke_and_await_agent(&component, &provider, "create_output_gate", data_value!())
            .await?
            .into_typed::<PromiseId>()?;
        let invocation_key = IdempotencyKey::fresh();
        let invocation = tokio::spawn({
            let executor = executor.clone();
            let component = component.clone();
            let original_for_invocation = original.clone();
            let gate = gate.clone();
            let invocation_key = invocation_key.clone();
            async move {
                executor
                    .invoke_and_await_agent_with_key(
                        &component,
                        &original_for_invocation,
                        &invocation_key,
                        "fork_drop_inherited_output",
                        data_value!(gate, original_for_invocation.to_string(), forwarded_drop),
                    )
                    .await
            }
        });

        let (cut, rpc_key) = tokio::time::timeout(Duration::from_secs(30), async {
            loop {
                let caller_history = executor
                    .get_oplog(&original_id, OplogIndex::INITIAL)
                    .await?;
                let rpc = caller_history.iter().find_map(|entry| match &entry.entry {
                    PublicOplogEntry::Start(start)
                        if start.function_name == "golem::rpc::wasm-rpc::invoke_and_await" =>
                    {
                        Some((
                            entry.oplog_index,
                            HostRequestGolemRpcInvoke::from_value(
                                start.request.as_ref().unwrap().value(),
                            )
                            .unwrap()
                            .idempotency_key,
                        ))
                    }
                    _ => None,
                });
                if let Some((start, rpc_key)) = rpc {
                    let end = caller_history.iter().find_map(|entry| {
                    matches!(&entry.entry, PublicOplogEntry::End(end) if end.start_index == start)
                        .then_some(entry.oplog_index)
                });
                    let provider_has_item = executor
                        .get_oplog(&provider_id, OplogIndex::INITIAL)
                        .await?
                        .iter()
                        .any(|entry| matches!(entry.entry, PublicOplogEntry::StreamItems(_)));
                    if let Some(end) = end
                        && provider_has_item
                    {
                        break Ok::<_, anyhow::Error>((
                            if cut_at_start { start } else { end },
                            rpc_key,
                        ));
                    }
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .map_err(|_| anyhow::anyhow!("RPC did not reach End with an open output"))??;

        let fork =
            golem_common::phantom_agent_id!("StreamingRpcCaller", uuid::Uuid::new_v4(), name);
        let fork_id = AgentId::from_agent_id(component.id, &fork).map_err(anyhow::Error::msg)?;
        executor
            .fork_worker(&original_id, &fork.to_string(), cut)
            .await?;
        assert_eq!(
            executor
                .invoke_and_await_agent_with_key(
                    &component,
                    &fork,
                    &invocation_key,
                    "fork_drop_inherited_output",
                    data_value!(gate.clone(), original.to_string(), forwarded_drop),
                )
                .await?
                .into_typed::<Vec<u64>>()?,
            Vec::<u64>::new(),
            "the fork must drop its inherited output reader"
        );
        tokio::time::timeout(Duration::from_secs(30), async {
            loop {
                let provider_history = executor
                    .get_oplog(&provider_id, OplogIndex::INITIAL)
                    .await?;
                let expected_consumer = if forwarded_drop {
                    &downstream_id
                } else {
                    &fork_id
                };
                let finalized_stream = provider_history.iter().find_map(|entry| {
                    let PublicOplogEntry::StreamSession(session) = &entry.entry else {
                        return None;
                    };
                    match StreamSessionRecord::from_value(session.record.value()).unwrap() {
                        StreamSessionRecord::AttachmentFinalized(record)
                            if record.reason
                                == StreamAttachmentFinalizationReason::ConsumerFinalized
                                && record.key.consumer == *expected_consumer =>
                        {
                            Some(record.key.stream_id)
                        }
                        _ => None,
                    }
                });
                let downstream_input_cancelled = !forwarded_drop
                    || executor
                        .get_oplog(&downstream_id, OplogIndex::INITIAL)
                        .await?
                        .iter()
                        .any(|entry| {
                            matches!(&entry.entry, PublicOplogEntry::StreamSession(session)
                            if matches!(
                                StreamSessionRecord::from_value(session.record.value()).unwrap(),
                                StreamSessionRecord::ConsumerCancelIntent(record)
                                    if record.role == DurableStreamCancelRole::InputConsumer
                            ))
                        });
                if finalized_stream.is_some() && downstream_input_cancelled {
                    break Ok::<_, anyhow::Error>(());
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .map_err(|_| anyhow::anyhow!("fork reader finalization was not committed"))??;

        assert!(
            !executor
                .get_oplog(&provider_id, OplogIndex::INITIAL)
                .await?
                .iter()
                .any(|entry| matches!(entry.entry, PublicOplogEntry::StreamCancel(_))),
            "dropping an inherited output must not cancel the shared source"
        );

        invocation.abort();
        let _ = invocation.await;
        drop(executor);
        let (executor, _) = crate::fork::start_with_remote_streaming_rpc(deps, &context).await?;
        executor.complete_promise(&gate, Vec::new()).await?;
        assert_eq!(
            executor
                .invoke_and_await_agent_with_key(
                    &component,
                    &original,
                    &invocation_key,
                    "fork_drop_inherited_output",
                    data_value!(gate.clone(), original.to_string(), forwarded_drop),
                )
                .await?
                .into_typed::<Vec<u64>>()?,
            vec![1, 1],
            "the original consumer must drain the source after cold recovery"
        );

        let cancel_count_before_new_rpc = executor
            .get_oplog(&provider_id, OplogIndex::INITIAL)
            .await?
            .iter()
            .filter(|entry| matches!(entry.entry, PublicOplogEntry::StreamCancel(_)))
            .count();
        executor
            .invoke_and_await_agent(
                &component,
                &fork,
                "drop_new_increment_output",
                data_value!(),
            )
            .await?;
        let provider_history = executor
            .get_oplog(&provider_id, OplogIndex::INITIAL)
            .await?;
        assert_eq!(
            provider_history
                .iter()
                .filter(|entry| matches!(entry.entry, PublicOplogEntry::StreamCancel(_)))
                .count(),
            cancel_count_before_new_rpc + 1,
            "a new RPC made by the fork must retain source cancellation"
        );
        let executions = provider_history
            .iter()
            .filter_map(|entry| match &entry.entry {
                PublicOplogEntry::AgentInvocationStarted(started) => match &started.invocation {
                    PublicAgentInvocation::AgentMethodInvocation(method)
                        if method.method_name == "increment_gated_stream"
                            || method.method_name == "increment_many_stream" =>
                    {
                        Some(method.idempotency_key.clone())
                    }
                    _ => None,
                },
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(executions.len(), 2);
        assert_eq!(executions[0], rpc_key);
        assert_ne!(executions[0], executions[1]);
        assert_eq!(
            executor
                .invoke_and_await_agent(&component, &provider, "increment_scalar", data_value!())
                .await?
                .into_typed::<u64>()?,
            3,
            "the inherited RPC and the fork's new RPC must each execute exactly once"
        );
    }
    Ok(())
}

#[test]
#[timeout("120s")]
async fn fork_completed_stream_reconstructs_state_without_repeating_output(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] agent_rpc_rust: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = crate::fork::start_with_local_resume(deps, &context, false).await?;
    let component = executor
        .component_dep(&context.default_environment_id, agent_rpc_rust)
        .store()
        .await?;
    let source = agent_id!("StreamingRpcTarget", "fork-completed-stream");
    let source_id = executor.start_agent(&component.id, source.clone()).await?;
    let metadata = executor.get_worker_metadata(&source_id).await?;
    let (_, input) = data_value!().into_parts();
    let start = InvocationRequest {
        request: Some(invocation_request::Request::Start(InvocationStart {
            agent_id: Some(source_id.clone().into()),
            method_name: Some("increment_stream".into()),
            input: Some(input.try_into().map_err(anyhow::Error::msg)?),
            idempotency_key: Some(IdempotencyKey::fresh().into()),
            auth_ctx: Some(executor.auth_ctx().into()),
            environment_id: Some(component.environment_id.into()),
            component_owner_account_id: Some(component.account_id.into()),
            attempt_id: Some(uuid::Uuid::new_v4().into()),
            expected_callee_fingerprint: Some(metadata.fingerprint.0.into()),
            ..Default::default()
        })),
    };
    let mut state = InvocationSessionState::default();
    state
        .validate_trusted_request(&start)
        .map_err(anyhow::Error::msg)?;
    let (frames, receiver) = mpsc::channel(1);
    frames.send(start).await?;
    let mut responses = executor
        .client
        .clone()
        .invoke_agent_session(ReceiverStream::new(receiver))
        .await?
        .into_inner();
    let mut items = Vec::new();
    let mut ends = 0;
    let mut finished = false;
    while let Some(response) = responses.message().await? {
        state
            .validate_response(&response)
            .map_err(anyhow::Error::msg)?;
        match response.response {
            Some(invocation_response::Response::OutputItem(item)) => {
                items.push(
                    SchemaValue::try_from(item.value.expect("output value"))
                        .map_err(anyhow::Error::msg)?,
                );
            }
            Some(invocation_response::Response::OutputEnd(_)) => ends += 1,
            Some(invocation_response::Response::Finished(completion)) => {
                assert!(matches!(
                    completion.outcome,
                    Some(invocation_session_completion::Outcome::Success(_))
                ));
                finished = true;
            }
            Some(invocation_response::Response::Accepted(_))
            | Some(invocation_response::Response::Result(_)) => {}
            other => anyhow::bail!("unexpected output response: {other:?}"),
        }
    }
    assert_eq!(items, vec![SchemaValue::U64(1)]);
    assert_eq!(ends, 1);
    assert!(finished && state.is_complete());
    drop(frames);
    drop(responses);
    let prefix = executor.get_oplog(&source_id, OplogIndex::INITIAL).await?;
    let cut = prefix.last().unwrap().oplog_index;
    let target = golem_common::phantom_agent_id!(
        "StreamingRpcTarget",
        uuid::Uuid::new_v4(),
        "fork-completed-stream"
    );
    let target_id = AgentId::from_agent_id(component.id, &target).map_err(anyhow::Error::msg)?;
    executor
        .fork_worker(&source_id, &target.to_string(), cut)
        .await?;
    let scalar = executor
        .invoke_and_await_agent(&component, &target, "increment_scalar", data_value!())
        .await?
        .into_typed::<u64>()?;
    assert_eq!(scalar, 2);
    executor
        .revert(
            &target_id,
            golem_common::model::worker::RevertWorkerTarget::RevertToOplogIndex(
                golem_common::model::worker::RevertToOplogIndex {
                    last_oplog_index: cut,
                },
            ),
        )
        .await?;
    let scalar_after_revert = executor
        .invoke_and_await_agent(&component, &target, "increment_scalar", data_value!())
        .await?
        .into_typed::<u64>()?;
    assert_eq!(scalar_after_revert, 2);
    // The fork's copied output and guest state must no longer depend on the source oplog.
    executor.delete_worker(&source_id).await?;
    drop(executor);
    let executor = crate::fork::start_with_local_resume(deps, &context, false).await?;
    let scalar = executor
        .invoke_and_await_agent(&component, &target, "increment_scalar", data_value!())
        .await?
        .into_typed::<u64>()?;
    assert_eq!(scalar, 3);
    let history = executor.get_oplog(&target_id, OplogIndex::INITIAL).await?;
    let output_records = history
        .iter()
        .filter(|entry| matches!(entry.entry, PublicOplogEntry::StreamItems(_)))
        .collect::<Vec<_>>();
    assert_eq!(output_records.len(), 1);
    assert!(output_records[0].oplog_index <= cut);
    Ok(())
}

#[test]
#[timeout("120s")]
async fn reverted_retained_stream_start_reports_current_epoch_for_resume(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] agent_rpc_rust: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;
    let component = executor
        .component_dep(&context.default_environment_id, agent_rpc_rust)
        .store()
        .await?;
    let agent = agent_id!("StreamingRpcTarget", "retained-start-revert");
    let worker_id = executor.start_agent(&component.id, agent).await?;
    wait_for_agent_initialization(&executor, &worker_id).await?;
    let metadata = executor.get_worker_metadata(&worker_id).await?;
    let key = IdempotencyKey::fresh();
    let start = InvocationStart {
        agent_id: Some(worker_id.clone().into()),
        method_name: Some("consume".into()),
        input: Some(golem_api_grpc::proto::golem::schema::SchemaValue {
            value: Some(schema_value::Value::RecordValue(RecordValue {
                fields: vec![golem_api_grpc::proto::golem::schema::SchemaValue {
                    value: Some(schema_value::Value::StreamReference(
                        SchemaValueStreamReference { stream_id: 1 },
                    )),
                }],
            })),
        }),
        idempotency_key: Some(key.clone().into()),
        auth_ctx: Some(executor.auth_ctx().into()),
        environment_id: Some(component.environment_id.into()),
        component_owner_account_id: Some(component.account_id.into()),
        attempt_id: Some(uuid::Uuid::new_v4().into()),
        expected_callee_fingerprint: Some(metadata.fingerprint.0.into()),
        ..Default::default()
    };
    let request = InvocationRequest {
        request: Some(invocation_request::Request::Start(start.clone())),
    };
    let (old_frames, receiver) = mpsc::channel(8);
    old_frames.send(request.clone()).await?;
    let mut old_responses = executor
        .client
        .clone()
        .invoke_agent_session(ReceiverStream::new(receiver))
        .await?
        .into_inner();
    let accepted = old_responses.message().await?.unwrap();
    assert!(
        matches!(accepted.response, Some(invocation_response::Response::Accepted(ref accepted)) if accepted.epoch == 1)
    );
    // Revert must retain the running invocation, not delete its start: processed invocations
    // are deliberately not put back on the pending queue by ordinary revert.
    let cut = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let history = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
            if let Some(index) = history.iter().find_map(|entry| {
                matches!(&entry.entry,
                    PublicOplogEntry::AgentInvocationStarted(record)
                        if matches!(&record.invocation,
                            PublicAgentInvocation::AgentMethodInvocation(method)
                                if method.idempotency_key == key))
                .then_some(entry.oplog_index)
            }) {
                break Ok::<_, anyhow::Error>(index);
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await??;
    executor
        .revert(
            &worker_id,
            golem_common::model::worker::RevertWorkerTarget::RevertToOplogIndex(
                golem_common::model::worker::RevertToOplogIndex {
                    last_oplog_index: cut,
                },
            ),
        )
        .await?;
    drop(old_frames);
    drop(old_responses);

    let (frames, receiver) = mpsc::channel(8);
    frames.send(request.clone()).await?;
    let mut responses = executor
        .client
        .clone()
        .invoke_agent_session(ReceiverStream::new(receiver))
        .await?
        .into_inner();
    let response = responses.message().await?.unwrap();
    let Some(invocation_response::Response::Accepted(accepted)) = response.response else {
        anyhow::bail!("{response:?}");
    };
    assert_eq!(accepted.epoch, 2);
    assert!(matches!(
        responses.message().await?.unwrap().response,
        Some(invocation_response::Response::AttachmentRevoked(_))
    ));
    drop(frames);
    drop(responses);

    let resume = InvocationRequest {
        request: Some(invocation_request::Request::ResumeAttach(ResumeAttach {
            agent_id: start.agent_id,
            environment_id: start.environment_id,
            idempotency_key: start.idempotency_key,
            expected_callee_fingerprint: start.expected_callee_fingerprint,
            auth_ctx: start.auth_ctx,
            attachment_id: accepted.attachment_id,
            attempt_id: Some(uuid::Uuid::new_v4().into()),
            expected_epoch: accepted.epoch,
            operation: ResumeOperation::Resume as i32,
            ..Default::default()
        })),
    };
    let mut state = InvocationSessionState::default();
    state
        .validate_trusted_request(&resume)
        .map_err(anyhow::Error::msg)?;
    let (frames, receiver) = mpsc::channel(8);
    frames.send(resume.clone()).await?;
    let mut responses = executor
        .client
        .clone()
        .invoke_agent_session(ReceiverStream::new(receiver))
        .await?
        .into_inner();
    let response = responses.message().await?.unwrap();
    state
        .validate_response(&response)
        .map_err(anyhow::Error::msg)?;
    let Some(invocation_response::Response::Accepted(accepted)) = response.response else {
        anyhow::bail!("{response:?}");
    };
    assert_eq!(accepted.epoch, 3);

    // Replaying Start while another attempt is still attached must report its current epoch.
    let (replayed_frames, receiver) = mpsc::channel(8);
    replayed_frames.send(request).await?;
    let mut replayed_responses = executor
        .client
        .clone()
        .invoke_agent_session(ReceiverStream::new(receiver))
        .await?
        .into_inner();
    let response = replayed_responses.message().await?.unwrap();
    let Some(invocation_response::Response::Accepted(replayed)) = response.response else {
        anyhow::bail!("{response:?}");
    };
    assert_eq!(replayed.epoch, 3);
    assert!(matches!(
        replayed_responses.message().await?.unwrap().response,
        Some(invocation_response::Response::AttachmentRevoked(_))
    ));
    let mut takeover = resume;
    let Some(invocation_request::Request::ResumeAttach(attach)) = &mut takeover.request else {
        unreachable!();
    };
    attach.expected_epoch = replayed.epoch;
    attach.attempt_id = Some(uuid::Uuid::new_v4().into());
    attach.operation = ResumeOperation::Takeover as i32;
    let old_frames = frames;
    let old_responses = responses;
    let (frames, receiver) = mpsc::channel(8);
    state = InvocationSessionState::default();
    state
        .validate_trusted_request(&takeover)
        .map_err(anyhow::Error::msg)?;
    frames.send(takeover).await?;
    let mut responses = executor
        .client
        .clone()
        .invoke_agent_session(ReceiverStream::new(receiver))
        .await?
        .into_inner();
    let response = responses.message().await?.unwrap();
    state
        .validate_response(&response)
        .map_err(anyhow::Error::msg)?;
    let Some(invocation_response::Response::Accepted(accepted)) = response.response else {
        anyhow::bail!("{response:?}");
    };
    assert_eq!(accepted.epoch, 4);
    drop(old_frames);
    drop(old_responses);
    drop(replayed_frames);
    drop(replayed_responses);

    let stream = accepted.stream_mappings[0]
        .handle
        .as_ref()
        .unwrap()
        .stream_id;
    for frame in [
        InvocationRequest {
            request: Some(invocation_request::Request::InputItem(InputStreamItem {
                transport_stream_id: 1,
                sequence: 0,
                durable_stream_id: stream,
                epoch: accepted.epoch,
                payload: Some(input_stream_item::Payload::Value(
                    SchemaValue::U32(29)
                        .try_into()
                        .map_err(anyhow::Error::msg)?,
                )),
            })),
        },
        InvocationRequest {
            request: Some(invocation_request::Request::InputEnd(InputStreamEnd {
                transport_stream_id: 1,
                sequence: 1,
                durable_stream_id: stream,
                epoch: accepted.epoch,
            })),
        },
    ] {
        state
            .validate_trusted_request(&frame)
            .map_err(anyhow::Error::msg)?;
        frames.send(frame).await?;
    }
    let mut result = None;
    while !state.is_complete() {
        let response = responses
            .message()
            .await?
            .expect("resumed session completion");
        state
            .validate_response(&response)
            .map_err(anyhow::Error::msg)?;
        if let Some(invocation_response::Response::Result(result_frame)) = response.response
            && let Some(invocation_session_result::Result::MethodResult(value)) =
                result_frame.result
        {
            result = Some(SchemaValue::try_from(value).map_err(anyhow::Error::msg)?);
        }
    }
    assert_eq!(
        result,
        Some(SchemaValue::List {
            elements: vec![SchemaValue::U32(29)]
        })
    );
    let history = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    assert_eq!(
        history
            .iter()
            .filter(|entry| matches!(&entry.entry,
                PublicOplogEntry::PendingAgentInvocation(record)
                    if matches!(&record.invocation,
                        PublicAgentInvocation::AgentMethodInvocation(method)
                            if method.idempotency_key == key)))
            .count(),
        1
    );
    Ok(())
}

#[test]
#[timeout("120s")]
async fn revert_stream_acceptance_removes_pending_inputs_and_fences_old_connections(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] agent_rpc_rust: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use golem_common::model::worker::{RevertToOplogIndex, RevertWorkerTarget};

    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;
    let component = executor
        .component_dep(&context.default_environment_id, agent_rpc_rust)
        .store()
        .await?;
    let agent = agent_id!("StreamingRpcTarget", "revert-stream-acceptance");
    let worker_id = executor.start_agent(&component.id, agent.clone()).await?;
    wait_for_agent_initialization(&executor, &worker_id).await?;
    let metadata = executor.get_worker_metadata(&worker_id).await?;
    let cut = metadata.last_oplog_index;
    let input = golem_api_grpc::proto::golem::schema::SchemaValue {
        value: Some(schema_value::Value::RecordValue(RecordValue {
            fields: vec![golem_api_grpc::proto::golem::schema::SchemaValue {
                value: Some(schema_value::Value::StreamReference(
                    SchemaValueStreamReference { stream_id: 1 },
                )),
            }],
        })),
    };
    let request = InvocationRequest {
        request: Some(invocation_request::Request::Start(InvocationStart {
            agent_id: Some(worker_id.clone().into()),
            method_name: Some("consume".into()),
            input: Some(input),
            idempotency_key: Some(IdempotencyKey::fresh().into()),
            auth_ctx: Some(executor.auth_ctx().into()),
            environment_id: Some(component.environment_id.into()),
            component_owner_account_id: Some(component.account_id.into()),
            attempt_id: Some(uuid::Uuid::new_v4().into()),
            expected_callee_fingerprint: Some(metadata.fingerprint.0.into()),
            ..Default::default()
        })),
    };
    let mut old_connections = Vec::new();
    for queued in [false, true] {
        let mut request = request.clone();
        if queued && let Some(invocation_request::Request::Start(start)) = &mut request.request {
            start.idempotency_key = Some(IdempotencyKey::fresh().into());
        }
        let (frames, receiver) = mpsc::channel(8);
        frames.send(request).await?;
        let mut responses = executor
            .client
            .clone()
            .invoke_agent_session(ReceiverStream::new(receiver))
            .await?
            .into_inner();
        let response = responses.message().await?.expect("acceptance response");
        let Some(invocation_response::Response::Accepted(accepted)) = response.response else {
            anyhow::bail!("expected acceptance: {response:?}");
        };
        assert_eq!(accepted.epoch, 1);
        let stream = accepted.stream_mappings[0]
            .handle
            .as_ref()
            .unwrap()
            .stream_id;
        frames
            .send(InvocationRequest {
                request: Some(invocation_request::Request::InputItem(InputStreamItem {
                    transport_stream_id: 1,
                    sequence: 0,
                    durable_stream_id: stream,
                    epoch: accepted.epoch,
                    payload: Some(input_stream_item::Payload::Value(
                        SchemaValue::U32(13)
                            .try_into()
                            .map_err(anyhow::Error::msg)?,
                    )),
                })),
            })
            .await?;
        assert!(matches!(
            responses.message().await?.unwrap().response,
            Some(invocation_response::Response::InputAck(_))
        ));
        old_connections.push((frames, responses, stream));
    }
    executor
        .revert(
            &worker_id,
            RevertWorkerTarget::RevertToOplogIndex(RevertToOplogIndex {
                last_oplog_index: cut,
            }),
        )
        .await?;
    assert_eq!(
        executor
            .get_worker_metadata(&worker_id)
            .await?
            .pending_invocation_count,
        0
    );
    for (frames, _, stream) in &old_connections {
        let _ = frames
            .send(InvocationRequest {
                request: Some(invocation_request::Request::InputItem(InputStreamItem {
                    transport_stream_id: 1,
                    sequence: 1,
                    durable_stream_id: *stream,
                    epoch: 1,
                    payload: Some(input_stream_item::Payload::Value(
                        SchemaValue::U32(71)
                            .try_into()
                            .map_err(anyhow::Error::msg)?,
                    )),
                })),
            })
            .await;
    }
    drop(old_connections);
    drop(executor);
    let executor = start(deps, &context).await?;
    let mut state = InvocationSessionState::default();
    state
        .validate_trusted_request(&request)
        .map_err(anyhow::Error::msg)?;
    let (frames, receiver) = mpsc::channel(8);
    frames.send(request).await?;
    let mut responses = executor
        .client
        .clone()
        .invoke_agent_session(ReceiverStream::new(receiver))
        .await?
        .into_inner();
    let response = responses.message().await?.expect("reacceptance response");
    state
        .validate_response(&response)
        .map_err(anyhow::Error::msg)?;
    let Some(invocation_response::Response::Accepted(accepted)) = response.response else {
        anyhow::bail!("expected reacceptance: {response:?}");
    };
    assert_eq!(accepted.epoch, 2);
    let stream = accepted.stream_mappings[0]
        .handle
        .as_ref()
        .unwrap()
        .stream_id;
    let item = InvocationRequest {
        request: Some(invocation_request::Request::InputItem(InputStreamItem {
            transport_stream_id: 1,
            sequence: 0,
            durable_stream_id: stream,
            epoch: accepted.epoch,
            payload: Some(input_stream_item::Payload::Value(
                SchemaValue::U32(29)
                    .try_into()
                    .map_err(anyhow::Error::msg)?,
            )),
        })),
    };
    state
        .validate_trusted_request(&item)
        .map_err(anyhow::Error::msg)?;
    frames.send(item).await?;
    let end = InvocationRequest {
        request: Some(invocation_request::Request::InputEnd(InputStreamEnd {
            transport_stream_id: 1,
            sequence: 1,
            durable_stream_id: stream,
            epoch: accepted.epoch,
        })),
    };
    state
        .validate_trusted_request(&end)
        .map_err(anyhow::Error::msg)?;
    frames.send(end).await?;
    let mut result = None;
    while !state.is_complete() {
        let response = responses.message().await?.expect("session completion");
        state
            .validate_response(&response)
            .map_err(anyhow::Error::msg)?;
        match response.response {
            Some(invocation_response::Response::InputAck(_)) => {}
            Some(invocation_response::Response::Result(value)) => {
                let Some(invocation_session_result::Result::MethodResult(value)) = value.result
                else {
                    anyhow::bail!("missing method result")
                };
                result = Some(SchemaValue::try_from(value).map_err(anyhow::Error::msg)?);
            }
            Some(invocation_response::Response::Finished(completion)) => assert!(matches!(
                completion.outcome,
                Some(invocation_session_completion::Outcome::Success(_))
            )),
            other => anyhow::bail!("unexpected response: {other:?}"),
        }
    }
    assert_eq!(
        result,
        Some(SchemaValue::List {
            elements: vec![SchemaValue::U32(29)]
        })
    );
    Ok(())
}

async fn invoke_agent_session(
    executor: &TestWorkerExecutor,
    component: &ComponentDto,
    agent_id: &ParsedAgentId,
    method_name: &str,
    params: TypedSchemaValue,
) -> anyhow::Result<Result<SchemaValue, String>> {
    let (mut state, _frames, mut inbound) =
        open_invocation_session(executor, component, agent_id, method_name, params).await?;
    receive_invocation_session(&mut state, &mut inbound).await
}

async fn wait_for_agent_initialization(
    executor: &TestWorkerExecutor,
    agent_id: &AgentId,
) -> anyhow::Result<()> {
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            let metadata = executor.get_worker_metadata(agent_id).await?;
            if metadata.status == AgentStatus::Idle
                && metadata.pending_invocation_count == 0
                && metadata.last_oplog_index > OplogIndex::INITIAL
            {
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .map_err(|_| anyhow::anyhow!("timed out waiting for {agent_id} initialization"))?
}

async fn open_invocation_session(
    executor: &TestWorkerExecutor,
    component: &ComponentDto,
    agent_id: &ParsedAgentId,
    method_name: &str,
    params: TypedSchemaValue,
) -> anyhow::Result<(
    InvocationSessionState,
    mpsc::Sender<InvocationRequest>,
    tonic::Streaming<InvocationResponse>,
)> {
    open_invocation_session_with_key(
        executor,
        component,
        agent_id,
        &IdempotencyKey::fresh(),
        method_name,
        params,
    )
    .await
}

async fn open_invocation_session_with_key(
    executor: &TestWorkerExecutor,
    component: &ComponentDto,
    agent_id: &ParsedAgentId,
    idempotency_key: &IdempotencyKey,
    method_name: &str,
    params: TypedSchemaValue,
) -> anyhow::Result<(
    InvocationSessionState,
    mpsc::Sender<InvocationRequest>,
    tonic::Streaming<InvocationResponse>,
)> {
    let worker_agent_id = AgentId::from_agent_id(component.id, agent_id)
        .map_err(|error| anyhow::anyhow!("invalid agent id: {error}"))?;
    let (_, input) = params.into_parts();
    let input = input.try_into().map_err(anyhow::Error::msg)?;
    let (frames, receiver) = mpsc::channel(8);
    let request = InvocationRequest {
        request: Some(invocation_request::Request::Start(InvocationStart {
            agent_id: Some(worker_agent_id.into()),
            method_name: Some(method_name.to_string()),
            input: Some(input),
            idempotency_key: Some(idempotency_key.clone().into()),
            context: None,
            auth_ctx: Some(executor.auth_ctx().into()),
            principal: None,
            environment_id: Some(component.environment_id.into()),
            config: Vec::new(),
            component_owner_account_id: Some(component.account_id.into()),
            mode: golem_api_grpc::proto::golem::worker::AgentInvocationMode::Await as i32,
            schedule_at: None,
            freshness_disposition:
                golem_api_grpc::proto::golem::worker::InvocationFreshnessDisposition::MayExist
                    as i32,
            attempt_id: Some(uuid::Uuid::new_v4().into()),
            expected_callee_fingerprint: None,
            durable_input_mappings: Vec::new(),
            scope_card: None,
            origin_invocation: None,
            external_tool: None,
        })),
    };
    let mut state = InvocationSessionState::default();
    state
        .validate_trusted_request(&request)
        .map_err(anyhow::Error::msg)?;
    frames.send(request).await?;
    let inbound = executor
        .client
        .clone()
        .invoke_agent_session(ReceiverStream::new(receiver))
        .await?
        .into_inner();
    Ok((state, frames, inbound))
}

async fn receive_invocation_session(
    state: &mut InvocationSessionState,
    inbound: &mut tonic::Streaming<InvocationResponse>,
) -> anyhow::Result<Result<SchemaValue, String>> {
    let mut result = None;
    let mut terminal = None;
    while let Some(response) = inbound.message().await? {
        state
            .validate_response(&response)
            .map_err(anyhow::Error::msg)?;
        match response.response {
            Some(invocation_response::Response::Accepted(_)) => {}
            Some(invocation_response::Response::Rejected(rejected)) => {
                terminal = Some(Err(rejected.error));
            }
            Some(invocation_response::Response::Result(value)) => {
                if result.is_some() {
                    anyhow::bail!("invocation session returned more than one result");
                }
                result = match value.result {
                    Some(invocation_session_result::Result::MethodResult(value)) => {
                        Some(value.try_into().map_err(anyhow::Error::msg)?)
                    }
                    Some(invocation_session_result::Result::ToolResult(_)) => {
                        anyhow::bail!("method invocation session returned a tool result")
                    }
                    Some(invocation_session_result::Result::NoResult(_)) | None => {
                        anyhow::bail!("invocation session returned no method result")
                    }
                };
            }
            Some(invocation_response::Response::Finished(finished)) => {
                terminal = Some(match finished.outcome {
                    Some(invocation_session_completion::Outcome::Success(_)) => {
                        result.take().map(Ok).ok_or_else(|| {
                            anyhow::anyhow!("invocation session ended without a result")
                        })?
                    }
                    Some(invocation_session_completion::Outcome::Failure(failure)) => {
                        Err(failure.message)
                    }
                    None => anyhow::bail!("invocation session completion has no outcome"),
                });
            }
            Some(other) => {
                anyhow::bail!("unexpected outer invocation session frame: {other:?}")
            }
            None => anyhow::bail!("empty outer invocation session frame"),
        }
    }
    terminal.ok_or_else(|| anyhow::anyhow!("invocation session response ended before completion"))
}

#[test]
#[timeout("2 minutes")]
#[tracing::instrument]
async fn typescript_streaming_guest_abi_e2e(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc")] agent_rpc: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;
    let component = executor
        .component_dep(&context.default_environment_id, agent_rpc)
        .store()
        .await?;
    let agent_id = agent_id!("TsStreamingRpcTarget", "typescript-guest-abi");
    let worker_agent_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;
    let metadata = executor.get_worker_metadata(&worker_agent_id).await?;
    let input = golem_api_grpc::proto::golem::schema::SchemaValue {
        value: Some(schema_value::Value::RecordValue(RecordValue {
            fields: vec![golem_api_grpc::proto::golem::schema::SchemaValue {
                value: Some(schema_value::Value::StreamReference(
                    SchemaValueStreamReference { stream_id: 1 },
                )),
            }],
        })),
    };
    let start = InvocationRequest {
        request: Some(invocation_request::Request::Start(InvocationStart {
            agent_id: Some(worker_agent_id.into()),
            method_name: Some("transform".to_string()),
            input: Some(input),
            idempotency_key: Some(IdempotencyKey::fresh().into()),
            auth_ctx: Some(executor.auth_ctx().into()),
            environment_id: Some(component.environment_id.into()),
            component_owner_account_id: Some(component.account_id.into()),
            mode: golem_api_grpc::proto::golem::worker::AgentInvocationMode::Await as i32,
            freshness_disposition:
                golem_api_grpc::proto::golem::worker::InvocationFreshnessDisposition::MayExist
                    as i32,
            attempt_id: Some(uuid::Uuid::new_v4().into()),
            expected_callee_fingerprint: Some(metadata.fingerprint.0.into()),
            ..Default::default()
        })),
    };
    let mut state = InvocationSessionState::default();
    state
        .validate_trusted_request(&start)
        .map_err(anyhow::Error::msg)?;
    let (requests, receiver) = mpsc::channel(8);
    requests.send(start).await?;
    let mut responses = executor
        .client
        .clone()
        .invoke_agent_session(ReceiverStream::new(receiver))
        .await?
        .into_inner();

    let accepted = responses.message().await?.ok_or_else(|| {
        anyhow::anyhow!("TypeScript streaming invocation ended before acceptance")
    })?;
    state
        .validate_response(&accepted)
        .map_err(anyhow::Error::msg)?;
    let accepted = match accepted.response {
        Some(invocation_response::Response::Accepted(accepted)) => accepted,
        other => anyhow::bail!("expected TypeScript streaming acceptance, got {other:?}"),
    };
    let [input_mapping] = accepted.stream_mappings.as_slice() else {
        anyhow::bail!(
            "expected one TypeScript input stream mapping, got {}",
            accepted.stream_mappings.len()
        );
    };
    assert_eq!(input_mapping.transport_stream_id, 1);
    let durable_stream_id = input_mapping
        .handle
        .as_ref()
        .and_then(|handle| handle.stream_id)
        .ok_or_else(|| anyhow::anyhow!("TypeScript input mapping omitted its durable stream ID"))?;

    for (sequence, value) in [(0_u64, 2_u32), (1, 3)] {
        let item = InvocationRequest {
            request: Some(invocation_request::Request::InputItem(InputStreamItem {
                transport_stream_id: 1,
                sequence,
                payload: Some(input_stream_item::Payload::Value(
                    SchemaValue::U32(value)
                        .try_into()
                        .map_err(anyhow::Error::msg)?,
                )),
                durable_stream_id: Some(durable_stream_id),
                epoch: accepted.epoch,
            })),
        };
        state
            .validate_trusted_request(&item)
            .map_err(anyhow::Error::msg)?;
        requests.send(item).await?;
    }
    let end = InvocationRequest {
        request: Some(invocation_request::Request::InputEnd(InputStreamEnd {
            transport_stream_id: 1,
            sequence: 2,
            durable_stream_id: Some(durable_stream_id),
            epoch: accepted.epoch,
        })),
    };
    state
        .validate_trusted_request(&end)
        .map_err(anyhow::Error::msg)?;
    requests.send(end).await?;

    let mut output_stream_id = None;
    let mut output_values = Vec::new();
    let mut input_acks = 0;
    let mut output_ends = 0;
    let mut finished_successfully = false;
    while let Some(response) = responses.message().await? {
        state
            .validate_response(&response)
            .map_err(anyhow::Error::msg)?;
        match response.response {
            Some(invocation_response::Response::InputAck(_)) => input_acks += 1,
            Some(invocation_response::Response::Result(result)) => {
                let value = match result.result {
                    Some(invocation_session_result::Result::MethodResult(value)) => value,
                    other => anyhow::bail!("expected TypeScript transform result, got {other:?}"),
                };
                let stream_id = match value.value {
                    Some(schema_value::Value::StreamReference(reference)) => reference.stream_id,
                    other => anyhow::bail!("expected TypeScript transform stream, got {other:?}"),
                };
                let [mapping] = result.new_stream_mappings.as_slice() else {
                    anyhow::bail!(
                        "expected one TypeScript output stream mapping, got {}",
                        result.new_stream_mappings.len()
                    );
                };
                assert_eq!(mapping.transport_stream_id, stream_id);
                output_stream_id = Some(stream_id);
            }
            Some(invocation_response::Response::OutputItem(item)) => {
                assert_eq!(Some(item.transport_stream_id), output_stream_id);
                let value = match item.value.and_then(|value| value.value) {
                    Some(schema_value::Value::U32Value(value)) => value,
                    other => anyhow::bail!("expected TypeScript transform u32 item, got {other:?}"),
                };
                output_values.push(value);
            }
            Some(invocation_response::Response::OutputEnd(end)) => {
                assert_eq!(Some(end.transport_stream_id), output_stream_id);
                output_ends += 1;
            }
            Some(invocation_response::Response::Finished(finished)) => {
                finished_successfully = matches!(
                    finished.outcome,
                    Some(invocation_session_completion::Outcome::Success(_))
                );
            }
            Some(invocation_response::Response::Rejected(rejected)) => {
                anyhow::bail!(
                    "TypeScript streaming invocation rejected: {}",
                    rejected.error
                )
            }
            Some(other) => anyhow::bail!("unexpected TypeScript streaming response: {other:?}"),
            None => anyhow::bail!("empty TypeScript streaming response"),
        }
    }

    assert!(state.is_complete());
    assert_eq!(input_acks, 3);
    assert_eq!(output_values, vec![20, 30]);
    assert_eq!(output_ends, 1);
    assert!(finished_successfully);
    Ok(())
}

#[test]
#[timeout("2 minutes")]
#[tracing::instrument]
async fn generated_rust_client_streaming_rpc_e2e(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] agent_rpc_rust: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;
    let component = executor
        .component_dep(&context.default_environment_id, agent_rpc_rust)
        .store()
        .await?;
    let caller_agent_id = agent_id!("StreamingRpcCaller", "generated_streaming_rpc_e2e");
    let caller = executor
        .start_agent(&component.id, caller_agent_id.clone())
        .await?;

    let result = invoke_agent_session(
        &executor,
        &component,
        &caller_agent_id,
        "run",
        data_value!(),
    )
    .await?
    .map_err(anyhow::Error::msg)?;
    let SchemaValue::Record { fields } = result else {
        panic!("expected streaming RPC report record");
    };
    assert_eq!(fields.len(), 10);
    assert_eq!(
        fields[0],
        SchemaValue::List {
            elements: vec![
                SchemaValue::U32(1),
                SchemaValue::U32(2),
                SchemaValue::U32(3)
            ]
        }
    );
    assert_eq!(
        fields[1],
        SchemaValue::List {
            elements: vec![
                SchemaValue::U32(4),
                SchemaValue::U32(5),
                SchemaValue::U32(6)
            ]
        }
    );
    assert_eq!(
        fields[2],
        SchemaValue::List {
            elements: vec![
                SchemaValue::U32(70),
                SchemaValue::U32(80),
                SchemaValue::U32(90)
            ]
        }
    );
    assert_eq!(
        fields[3],
        SchemaValue::List {
            elements: vec![
                SchemaValue::String("left".to_string()),
                SchemaValue::String("right".to_string())
            ]
        }
    );
    assert_eq!(
        fields[4],
        SchemaValue::List {
            elements: vec![SchemaValue::U32(10), SchemaValue::U32(11)]
        }
    );
    assert_eq!(
        fields[5],
        SchemaValue::List {
            elements: vec![
                SchemaValue::String("first".to_string()),
                SchemaValue::String("second".to_string())
            ]
        }
    );
    assert_eq!(
        fields[6],
        SchemaValue::List {
            elements: vec![
                SchemaValue::List {
                    elements: vec![SchemaValue::U32(1), SchemaValue::U32(2)]
                },
                SchemaValue::List {
                    elements: vec![
                        SchemaValue::U32(3),
                        SchemaValue::U32(4),
                        SchemaValue::U32(5)
                    ]
                }
            ]
        }
    );
    assert_eq!(
        fields[7],
        SchemaValue::List {
            elements: vec![
                SchemaValue::String("a".to_string()),
                SchemaValue::String("b".to_string())
            ]
        }
    );
    assert_eq!(
        fields[8],
        SchemaValue::List {
            elements: (0..64).map(SchemaValue::U32).collect()
        }
    );
    assert_eq!(fields[9], SchemaValue::U64(42));

    let producer_error = invoke_agent_session(
        &executor,
        &component,
        &caller_agent_id,
        "call_producer_error",
        data_value!(),
    )
    .await?
    .expect_err("producer stream error must fail the invocation session");
    assert!(
        producer_error.contains("Component trapped"),
        "unexpected producer error: {producer_error}"
    );

    let stream_free_caller_id = agent_id!("StreamingRpcCaller", "stream_free_after_stream_error");
    executor
        .start_agent(&component.id, stream_free_caller_id.clone())
        .await?;
    let first = executor
        .invoke_and_await_agent(
            &component,
            &stream_free_caller_id,
            "call_stream_free",
            data_value!(),
        )
        .await?
        .into_typed::<u64>()?;
    let second = executor
        .invoke_and_await_agent(
            &component,
            &stream_free_caller_id,
            "call_stream_free",
            data_value!(),
        )
        .await?
        .into_typed::<u64>()?;
    assert_eq!((first, second), (1, 2));
    executor.check_oplog_is_queryable(&caller).await?;
    Ok(())
}

#[test]
#[timeout("2 minutes")]
#[tracing::instrument]
async fn rpc_suspension_retries_after_concurrent_http_wait_finishes(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] agent_rpc_rust: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let rpc_attempt_keys = Arc::new(Mutex::new(Vec::new()));
    let executor = start_with_concurrent_agent_limit_and_overrides(
        deps,
        &context,
        1,
        TestExecutorOverrides {
            wrap_rpc: Some(Arc::new({
                let rpc_attempt_keys = rpc_attempt_keys.clone();
                move |rpc| {
                    Arc::new(RecordingRpc::new(
                        rpc,
                        "increment_scalar",
                        rpc_attempt_keys.clone(),
                    ))
                }
            })),
            ..Default::default()
        },
    )
    .await?;
    let component = executor
        .component_dep(&context.default_environment_id, agent_rpc_rust)
        .store()
        .await?;

    let name = "rpc-suspension-retry-after-http";
    let target_id = agent_id!("StreamingRpcTarget", name);
    let caller_id = agent_id!("StreamingRpcCaller", name);
    let target = executor
        .start_agent(&component.id, target_id.clone())
        .await?;
    wait_for_agent_initialization(&executor, &target).await?;
    let caller = executor
        .start_agent(&component.id, caller_id.clone())
        .await?;
    wait_for_agent_initialization(&executor, &caller).await?;

    let request_received = Arc::new(tokio::sync::Semaphore::new(0));
    let response_gate = Arc::new(tokio::sync::Semaphore::new(0));
    let request_count = Arc::new(AtomicUsize::new(0));
    let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await?;
    let port = listener.local_addr()?.port();
    let server = {
        let request_received = request_received.clone();
        let response_gate = response_gate.clone();
        let request_count = request_count.clone();
        tokio::spawn(async move {
            let app = Router::new().route(
                "/gate",
                post(move || {
                    let request_received = request_received.clone();
                    let response_gate = response_gate.clone();
                    let request_count = request_count.clone();
                    async move {
                        request_count.fetch_add(1, Ordering::AcqRel);
                        request_received.add_permits(1);
                        response_gate
                            .acquire()
                            .await
                            .expect("response gate closed")
                            .forget();
                        "released"
                    }
                }),
            );
            axum::serve(listener, app).await.unwrap();
        })
    };

    let invocation = {
        let executor = executor.clone();
        let component = component.clone();
        let caller_id = caller_id.clone();
        tokio::spawn(async move {
            executor
                .invoke_and_await_agent(
                    &component,
                    &caller_id,
                    "call_stream_free_while_fetching",
                    data_value!("127.0.0.1", port),
                )
                .await
        })
    };
    tokio::time::timeout(Duration::from_secs(10), request_received.acquire())
        .await
        .map_err(|_| anyhow::anyhow!("caller did not issue the HTTP request"))??
        .forget();

    tokio::time::sleep(Duration::from_secs(31)).await;
    assert_eq!(
        executor.get_worker_metadata(&caller).await?.status,
        AgentStatus::Running,
        "the concurrent HTTP wait must conservatively block RPC suspension"
    );
    let caller_oplog = executor.get_oplog(&caller, OplogIndex::INITIAL).await?;
    assert!(
        caller_oplog
            .iter()
            .all(|entry| !matches!(entry.entry, PublicOplogEntry::Suspend(_))),
        "the first suspension attempt must not persist a Suspend entry"
    );
    let target_oplog = executor.get_oplog(&target, OplogIndex::INITIAL).await?;
    assert!(
        target_oplog.iter().all(|entry| !matches!(
            &entry.entry,
            PublicOplogEntry::AgentInvocationStarted(started)
                if matches!(&started.invocation,
                    PublicAgentInvocation::AgentMethodInvocation(method)
                        if method.method_name == "increment_scalar")
        )),
        "the target increment ran while it was waiting for the only active-agent slot"
    );

    response_gate.add_permits(1);
    tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            let oplog = executor
                .get_oplog(&caller, OplogIndex::INITIAL)
                .await
                .expect("failed to read caller oplog");
            if oplog
                .iter()
                .any(|entry| matches!(entry.entry, PublicOplogEntry::Suspend(_)))
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .map_err(|_| anyhow::anyhow!("RPC suspension was not retried after HTTP completed"))?;

    let first = tokio::time::timeout(Duration::from_secs(10), invocation)
        .await
        .map_err(|_| anyhow::anyhow!("RPC did not complete after its scheduled wake"))???
        .into_typed::<u64>()?;
    assert_eq!(first, 1);
    assert_eq!(request_count.load(Ordering::Acquire), 1);

    let rpc_attempt_keys = rpc_attempt_keys.lock().unwrap().clone();
    assert!(
        rpc_attempt_keys.len() >= 2,
        "RPC suspension must cause at least two dispatch attempts"
    );
    let rpc_idempotency_key = rpc_attempt_keys[0]
        .as_ref()
        .expect("durable RPC attempt must have an idempotency key");
    assert!(
        rpc_attempt_keys
            .iter()
            .all(|key| key.as_ref() == Some(rpc_idempotency_key)),
        "all RPC attempts must reuse the same idempotency key"
    );

    let target_oplog = executor.get_oplog(&target, OplogIndex::INITIAL).await?;
    let started = target_oplog
        .iter()
        .filter(|entry| {
            matches!(
                &entry.entry,
                PublicOplogEntry::AgentInvocationStarted(started)
                    if matches!(&started.invocation,
                        PublicAgentInvocation::AgentMethodInvocation(method)
                            if method.method_name == "increment_scalar"
                                && &method.idempotency_key == rpc_idempotency_key)
            )
        })
        .count();
    let finished = target_oplog
        .iter()
        .filter(|entry| {
            matches!(
                &entry.entry,
                PublicOplogEntry::AgentInvocationFinished(finished)
                    if finished.method_name.as_deref() == Some("increment_scalar")
            )
        })
        .count();
    assert_eq!(started, 1, "target must start the logical RPC exactly once");
    assert_eq!(
        finished, 1,
        "target must finish the logical RPC exactly once"
    );

    let second = executor
        .invoke_and_await_agent(&component, &target_id, "increment_scalar", data_value!())
        .await?
        .into_typed::<u64>()?;
    assert_eq!(second, 2);

    server.abort();
    Ok(())
}

#[test]
#[timeout("2 minutes")]
#[tracing::instrument]
async fn typescript_client_streaming_rpc_e2e(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc")] agent_rpc: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;
    let component = executor
        .component_dep(&context.default_environment_id, agent_rpc)
        .store()
        .await?;
    let caller_agent_id = agent_id!("TsStreamingRpcCaller", "typescript-client-streaming");
    let caller = executor
        .start_agent(&component.id, caller_agent_id.clone())
        .await?;

    let result = invoke_agent_session(
        &executor,
        &component,
        &caller_agent_id,
        "run",
        data_value!(),
    )
    .await?
    .map_err(anyhow::Error::msg)?;
    let SchemaValue::Record { fields } = result else {
        panic!("expected TypeScript streaming RPC report record");
    };
    assert_eq!(fields.len(), 12);
    assert_eq!(
        fields[0],
        SchemaValue::List {
            elements: vec![
                SchemaValue::U32(1),
                SchemaValue::U32(2),
                SchemaValue::U32(3)
            ]
        }
    );
    assert_eq!(
        fields[1],
        SchemaValue::List {
            elements: vec![
                SchemaValue::U32(4),
                SchemaValue::U32(5),
                SchemaValue::U32(6)
            ]
        }
    );
    assert_eq!(
        fields[2],
        SchemaValue::List {
            elements: vec![
                SchemaValue::U32(70),
                SchemaValue::U32(80),
                SchemaValue::U32(90)
            ]
        }
    );
    assert_eq!(
        fields[3],
        SchemaValue::List {
            elements: vec![
                SchemaValue::U32(12),
                SchemaValue::U32(13),
                SchemaValue::U32(14)
            ]
        }
    );
    assert_eq!(
        fields[4],
        SchemaValue::List {
            elements: vec![
                SchemaValue::String("left".to_string()),
                SchemaValue::String("right".to_string())
            ]
        }
    );
    assert_eq!(
        fields[5],
        SchemaValue::List {
            elements: vec![SchemaValue::U32(10), SchemaValue::U32(11)]
        }
    );
    assert_eq!(
        fields[6],
        SchemaValue::List {
            elements: vec![
                SchemaValue::String("first".to_string()),
                SchemaValue::String("second".to_string())
            ]
        }
    );
    assert_eq!(
        fields[7],
        SchemaValue::List {
            elements: vec![
                SchemaValue::List {
                    elements: vec![SchemaValue::U32(1), SchemaValue::U32(2)]
                },
                SchemaValue::List {
                    elements: vec![
                        SchemaValue::U32(3),
                        SchemaValue::U32(4),
                        SchemaValue::U32(5)
                    ]
                }
            ]
        }
    );
    assert_eq!(
        fields[8],
        SchemaValue::List {
            elements: vec![
                SchemaValue::String("a".to_string()),
                SchemaValue::String("b".to_string())
            ]
        }
    );
    assert_eq!(
        fields[9],
        SchemaValue::List {
            elements: (0..64).map(SchemaValue::U32).collect()
        }
    );
    assert_eq!(fields[10], SchemaValue::U32(100));
    assert_eq!(fields[11], SchemaValue::U32(42));

    let producer_error = invoke_agent_session(
        &executor,
        &component,
        &caller_agent_id,
        "callProducerError",
        data_value!(),
    )
    .await?
    .expect_err("TypeScript producer stream error must fail the invocation session");
    assert!(
        producer_error.contains("Component trapped")
            || producer_error.contains("ts-producer-failed"),
        "unexpected TypeScript producer error: {producer_error}"
    );

    let stream_free_caller_id =
        agent_id!("TsStreamingRpcCaller", "typescript-stream-free-after-error");
    executor
        .start_agent(&component.id, stream_free_caller_id.clone())
        .await?;
    let first = executor
        .invoke_and_await_agent(
            &component,
            &stream_free_caller_id,
            "callStreamFree",
            data_value!(),
        )
        .await?
        .into_typed::<u32>()?;
    let second = executor
        .invoke_and_await_agent(
            &component,
            &stream_free_caller_id,
            "callStreamFree",
            data_value!(),
        )
        .await?
        .into_typed::<u32>()?;
    assert_eq!((first, second), (1, 2));
    executor.check_oplog_is_queryable(&caller).await?;
    Ok(())
}

#[test]
#[timeout("2 minutes")]
#[tracing::instrument]
async fn typescript_early_output_drop_releases_streaming_cleanup_before_next_invocation(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc")] agent_rpc: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;
    let component = executor
        .component_dep(&context.default_environment_id, agent_rpc)
        .store()
        .await?;
    let name = "early-output-drop-cleanup";
    let caller_agent_id = agent_id!("TsStreamingRpcCaller", name);
    executor
        .start_agent(&component.id, caller_agent_id.clone())
        .await?;
    let target_agent_id = agent_id!("TsStreamingRpcTarget", name);
    let target = executor
        .start_agent(&component.id, target_agent_id.clone())
        .await?;
    wait_for_agent_initialization(&executor, &target).await?;
    let mut transform_success = executor.gate_next_agent_invocation_success(&target);

    let invocation = {
        let executor = executor.clone();
        let component = component.clone();
        let caller_agent_id = caller_agent_id.clone();
        tokio::spawn(async move {
            executor
                .invoke_and_await_agent(
                    &component,
                    &caller_agent_id,
                    "earlyOutputDropThenPing",
                    data_value!(),
                )
                .await
        })
    };
    tokio::time::timeout(Duration::from_secs(30), transform_success.entered())
        .await
        .map_err(|_| anyhow::anyhow!("transform did not reach its success barrier"))?;
    transform_success.release();

    let result = invocation.await??.into_typed::<(u32, u32)>()?;
    assert_eq!(result, (0, 42));

    let oplog = executor.get_oplog(&target, OplogIndex::INITIAL).await?;
    let (transform_started, transform_key) = oplog
        .iter()
        .enumerate()
        .find_map(|(position, entry)| match &entry.entry {
            PublicOplogEntry::AgentInvocationStarted(started) => match &started.invocation {
                PublicAgentInvocation::AgentMethodInvocation(method)
                    if method.method_name == "transform" =>
                {
                    Some((position, method.idempotency_key.clone()))
                }
                _ => None,
            },
            _ => None,
        })
        .ok_or_else(|| anyhow::anyhow!("target transform has no Started entry"))?;
    let transform_finished = oplog
        .iter()
        .enumerate()
        .skip(transform_started + 1)
        .find_map(|(position, entry)| {
            matches!(
                &entry.entry,
                PublicOplogEntry::AgentInvocationFinished(finished)
                    if finished.method_name.as_deref() == Some("transform")
            )
            .then_some(position)
        })
        .ok_or_else(|| {
            anyhow::anyhow!("target transform {transform_key} has no matching Finished entry")
        })?;
    let ping_started = oplog
        .iter()
        .enumerate()
        .skip(transform_finished + 1)
        .find_map(|(position, entry)| {
            matches!(
                &entry.entry,
                PublicOplogEntry::AgentInvocationStarted(started)
                    if matches!(
                        &started.invocation,
                        PublicAgentInvocation::AgentMethodInvocation(method)
                            if method.method_name == "ping"
                    )
            )
            .then_some(position)
        })
        .ok_or_else(|| anyhow::anyhow!("target ping has no Started entry"))?;
    assert!(
        oplog[transform_finished + 1..ping_started]
            .iter()
            .all(|entry| !matches!(
                entry.entry,
                PublicOplogEntry::Start(_) | PublicOplogEntry::End(_)
            )),
        "stream cleanup appended positional durable calls after the streaming invocation finished: {:#?}",
        &oplog[transform_finished..=ping_started]
    );
    assert!(
        oplog[transform_finished + 1..ping_started]
            .iter()
            .any(|entry| matches!(
                &entry.entry,
                PublicOplogEntry::StreamSession(session)
                    if matches!(
                        StreamSessionRecord::from_value(session.record.value()),
                        Ok(StreamSessionRecord::Finished(_))
                    )
            )),
        "streaming session was not durably finished before the next invocation started: {:#?}",
        &oplog[transform_finished..=ping_started]
    );

    let ping_finished = oplog
        .iter()
        .enumerate()
        .skip(ping_started + 1)
        .find_map(|(position, entry)| {
            matches!(
                &entry.entry,
                PublicOplogEntry::AgentInvocationFinished(finished)
                    if finished.method_name.as_deref() == Some("ping")
            )
            .then_some(position)
        })
        .ok_or_else(|| anyhow::anyhow!("subsequent invocation has no Finished entry"))?;
    assert!(
        oplog[ping_finished + 1..].iter().all(|entry| !matches!(
            entry.entry,
            PublicOplogEntry::Start(_) | PublicOplogEntry::End(_)
        )),
        "positional durable calls were appended after the final Finished entry: {:#?}",
        &oplog[ping_finished..]
    );
    Ok(())
}

#[test]
#[timeout("2 minutes")]
#[tracing::instrument]
async fn streaming_rpc_identity_survives_atomic_rollback(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] agent_rpc_rust: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let mut executor = start(deps, &context).await?;
    let component = executor
        .component_dep(&context.default_environment_id, agent_rpc_rust)
        .store()
        .await?;
    for (synchronous, with_input) in [(false, false), (true, false), (false, true)] {
        let name = format!("atomic-streaming-{synchronous}-{with_input}");
        let caller_id = agent_id!("StreamingRpcCaller", name.clone());
        let target_id = agent_id!("StreamingRpcTarget", name);
        let caller = executor
            .start_agent(&component.id, caller_id.clone())
            .await?;
        let target = executor
            .start_agent(&component.id, target_id.clone())
            .await?;
        wait_for_agent_initialization(&executor, &caller).await?;
        wait_for_agent_initialization(&executor, &target).await?;
        let gate = executor
            .invoke_and_await_agent(&component, &caller_id, "create_input_gate", data_value!())
            .await?
            .into_typed::<PromiseId>()?;
        let invocation = {
            let executor = executor.clone();
            let component = component.clone();
            let caller_id = caller_id.clone();
            let gate = gate.clone();
            tokio::spawn(async move {
                executor
                    .invoke_and_await_agent(
                        &component,
                        &caller_id,
                        "atomic_streaming_increment",
                        data_value!(gate, synchronous, with_input),
                    )
                    .await
            })
        };
        executor
            .wait_for_status(&caller, AgentStatus::Suspended, Duration::from_secs(30))
            .await?;
        assert_eq!(
            executor
                .invoke_and_await_agent(&component, &target_id, "scalar_value", data_value!())
                .await?
                .into_typed::<u64>()?,
            1,
            "the provider must mutate before the caller crashes"
        );
        let before = executor.get_oplog(&caller, OplogIndex::INITIAL).await?;
        let requests = |entries: &[golem_common::model::oplog::PublicOplogEntryWithIndex]| {
            entries
                .iter()
                .filter_map(|entry| match &entry.entry {
                    PublicOplogEntry::Start(start)
                        if start.function_name == "golem::rpc::wasm-rpc::invoke_and_await" =>
                    {
                        start.request.as_ref().map(|request| {
                            HostRequestGolemRpcInvoke::from_value(request.value())
                                .map(|request| (entry.oplog_index, request))
                        })
                    }
                    _ => None,
                })
                .collect::<Result<Vec<_>, _>>()
        };
        let first = requests(&before)?;
        assert_eq!(first.len(), 1);
        let _ = executor.simulated_crash(&caller).await;
        executor.complete_promise(&gate, Vec::new()).await?;
        invocation.await??;

        let after = executor.get_oplog(&caller, OplogIndex::INITIAL).await?;
        assert!(
            after
                .iter()
                .any(|entry| matches!(&entry.entry, PublicOplogEntry::Jump(_)))
        );
        let attempts = requests(&after)?;
        let second = attempts.last().expect("missing retried RPC Start");
        assert_ne!(
            first[0].0, second.0,
            "rollback must create a new physical Start"
        );
        let mutations = executor
            .invoke_and_await_agent(&component, &target_id, "scalar_value", data_value!())
            .await?
            .into_typed::<u64>()?;
        let provider = executor.get_oplog(&target, OplogIndex::INITIAL).await?;
        let executions = provider
            .iter()
            .filter_map(|entry| match &entry.entry {
                PublicOplogEntry::AgentInvocationStarted(started) => match &started.invocation {
                    PublicAgentInvocation::AgentMethodInvocation(method)
                        if method.method_name == "increment_stream"
                            || method.method_name == "increment_stream_input" =>
                    {
                        Some(&method.idempotency_key)
                    }
                    _ => None,
                },
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            (&second.1.idempotency_key, mutations, executions),
            (
                &first[0].1.idempotency_key,
                1,
                vec![&first[0].1.idempotency_key]
            ),
            "two caller attempts must retain one target identity and execute one provider mutation (sync={synchronous}, input={with_input})"
        );

        // Reconstruct both agents after the region and its streams have completed.
        drop(executor);
        executor = start(deps, &context).await?;
        executor
            .invoke_and_await_agent(&component, &caller_id, "create_input_gate", data_value!())
            .await?;
        assert_eq!(
            executor
                .invoke_and_await_agent(&component, &target_id, "scalar_value", data_value!())
                .await?
                .into_typed::<u64>()?,
            1
        );
        assert_eq!(
            executor
                .invoke_and_await_agent(&component, &target_id, "increment_scalar", data_value!())
                .await?
                .into_typed::<u64>()?,
            2,
            "a fresh provider invocation must continue from the single committed mutation"
        );
    }
    Ok(())
}

#[test]
#[timeout("2 minutes")]
#[tracing::instrument]
async fn caller_recovery_restarts_input_drain_after_rpc_result_commit(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] agent_rpc_rust: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;
    let component = executor
        .component_dep(&context.default_environment_id, agent_rpc_rust)
        .store()
        .await?;
    let caller_agent_id = agent_id!("StreamingRpcCaller", "caller-input-recovery");
    let caller = executor
        .start_agent(&component.id, caller_agent_id.clone())
        .await?;
    wait_for_agent_initialization(&executor, &caller).await?;
    let gate = executor
        .invoke_and_await_agent(
            &component,
            &caller_agent_id,
            "create_input_gate",
            data_value!(),
        )
        .await
        .map_err(|error| anyhow::anyhow!("failed to create caller input gate: {error}"))?
        .into_typed::<PromiseId>()?;
    let executor_for_invocation = executor.clone();
    let component_for_invocation = component.clone();
    let caller_for_invocation = caller_agent_id.clone();
    let gate_for_invocation = gate.clone();
    let invocation = tokio::spawn(
        async move {
            executor_for_invocation
                .invoke_and_await_agent(
                    &component_for_invocation,
                    &caller_for_invocation,
                    "recover_input_after_caller_crash",
                    data_value!(gate_for_invocation),
                )
                .await
        }
        .in_current_span(),
    );
    executor
        .wait_for_status(&caller, AgentStatus::Suspended, Duration::from_secs(30))
        .await?;

    executor.simulated_crash(&caller).await?;
    let oplog = executor.get_oplog(&caller, OplogIndex::INITIAL).await?;
    assert!(
        !oplog
            .iter()
            .any(|entry| matches!(entry.entry, PublicOplogEntry::Interrupted(_))),
        "a simulated crash of a suspended caller must not permanently interrupt it"
    );
    assert!(!invocation.is_finished());
    executor.complete_promise(&gate, Vec::new()).await?;

    let result = invocation
        .await?
        .map_err(|error| anyhow::anyhow!("caller recovery invocation failed: {error}"))?
        .into_typed::<Vec<u32>>()?;
    assert_eq!(result, vec![10, 20, 30]);
    Ok(())
}

#[test]
#[timeout("2 minutes")]
#[tracing::instrument]
async fn callee_recovery_continues_output_after_committed_item(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] agent_rpc_rust: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;
    let component = executor
        .component_dep(&context.default_environment_id, agent_rpc_rust)
        .store()
        .await?;
    let name = "callee-output-recovery";
    let target_agent_id = agent_id!("StreamingRpcTarget", name);
    let target = executor
        .start_agent(&component.id, target_agent_id.clone())
        .await?;
    wait_for_agent_initialization(&executor, &target).await?;

    let caller_agent_id = agent_id!("StreamingRpcCaller", name);
    let caller = executor
        .start_agent(&component.id, caller_agent_id.clone())
        .await?;
    wait_for_agent_initialization(&executor, &caller).await?;
    let gate = executor
        .invoke_and_await_agent(
            &component,
            &caller_agent_id,
            "create_input_gate",
            data_value!(),
        )
        .await?
        .into_typed::<PromiseId>()?;
    let executor_for_invocation = executor.clone();
    let component_for_invocation = component.clone();
    let caller_for_invocation = caller_agent_id.clone();
    let gate_for_invocation = gate.clone();
    let invocation = tokio::spawn(
        async move {
            executor_for_invocation
                .invoke_and_await_agent(
                    &component_for_invocation,
                    &caller_for_invocation,
                    "recover_input_after_caller_crash",
                    data_value!(gate_for_invocation),
                )
                .await
        }
        .in_current_span(),
    );
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            let oplog = executor.get_oplog(&target, OplogIndex::INITIAL).await?;
            if oplog
                .iter()
                .any(|entry| matches!(entry.entry, PublicOplogEntry::StreamItems(_)))
            {
                return Ok::<(), anyhow::Error>(());
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .map_err(|_| anyhow::anyhow!("callee did not commit its first output item"))??;

    let _ = executor.simulated_crash(&target).await;
    executor.complete_promise(&gate, Vec::new()).await?;

    let result = invocation
        .await?
        .map_err(|error| anyhow::anyhow!("callee recovery invocation failed: {error}"))?
        .into_typed::<Vec<u32>>()?;
    assert_eq!(result, vec![10, 20, 30]);
    Ok(())
}

#[test]
#[timeout("2 minutes")]
#[tracing::instrument]
async fn durable_agent_live_await_streaming_is_allowed(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] agent_rpc_rust: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;
    let component = executor
        .component_dep(&context.default_environment_id, agent_rpc_rust)
        .store()
        .await?;
    let caller_agent_id = agent_id!("StreamingRpcCaller", "durable-live-await");

    let result = invoke_agent_session(
        &executor,
        &component,
        &caller_agent_id,
        "run",
        data_value!(),
    )
    .await?
    .map_err(anyhow::Error::msg)?;

    let SchemaValue::Record { fields } = result else {
        panic!("expected streaming RPC report record");
    };
    assert_eq!(fields.len(), 10);
    assert_eq!(fields[9], SchemaValue::U64(42));
    Ok(())
}

#[test]
#[timeout("2 minutes")]
#[tracing::instrument]
async fn direct_rpc_classification_uses_the_existing_target_revision(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc")] agent_rpc: &PrecompiledComponent,
    #[tagged_as("agent_rpc_rust")] agent_rpc_rust: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let ts_component = executor
        .component_dep(&context.default_environment_id, agent_rpc)
        .store()
        .await?;
    let ts_caller_id = agent_id!("TestAgent", "pinned-ts-rpc-caller");
    let ts_target_id = agent_id!("ChildAgent", 0_f64);
    executor
        .start_agent(&ts_component.id, ts_caller_id.clone())
        .await?;
    executor.start_agent(&ts_component.id, ts_target_id).await?;
    executor
        .update_component(&ts_component.id, &agent_rpc_rust.wasm_name)
        .await?;

    let ts_result = executor
        .invoke_and_await_agent(&ts_component, &ts_caller_id, "run", data_value!(1_f64))
        .await?;
    assert_eq!(
        ts_result.into_return_value(),
        Some(SchemaValue::List {
            elements: vec![SchemaValue::F64(0.0)]
        })
    );

    let rust_component = executor
        .component_dep(&context.default_environment_id, agent_rpc_rust)
        .store()
        .await?;
    let rust_name = "pinned-rust-rpc-target";
    let rust_caller_id = agent_id!("StreamingRpcCaller", rust_name);
    let rust_target_id = agent_id!("StreamingRpcTarget", rust_name);
    executor
        .start_agent(&rust_component.id, rust_caller_id.clone())
        .await?;
    executor
        .start_agent(&rust_component.id, rust_target_id)
        .await?;
    executor
        .update_component(&rust_component.id, &agent_rpc.wasm_name)
        .await?;

    let rust_result = executor
        .invoke_and_await_agent(
            &rust_component,
            &rust_caller_id,
            "call_stream_free",
            data_value!(),
        )
        .await?
        .into_typed::<u64>()?;
    assert_eq!(rust_result, 1);
    Ok(())
}

#[test]
#[timeout("2 minutes")]
#[tracing::instrument]
async fn direct_rpc_authorizes_before_execution_revision_lookup(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc")] agent_rpc: &PrecompiledComponent,
    #[tagged_as("agent_rpc_rust")] agent_rpc_rust: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start_with_overrides(
        deps,
        &context,
        TestExecutorOverrides {
            create_direct_invocation_auth: Some(Arc::new(|| Arc::new(DenyDirectInvocationAuth))),
            ..Default::default()
        },
    )
    .await?;
    let component = executor
        .component_dep(&context.default_environment_id, agent_rpc)
        .store()
        .await?;
    let caller_id = agent_id!("TestAgent", "denied-rpc-caller");
    executor
        .start_agent(&component.id, caller_id.clone())
        .await?;
    executor
        .update_component(&component.id, &agent_rpc_rust.wasm_name)
        .await?;

    let error = executor
        .invoke_and_await_agent(&component, &caller_id, "run", data_value!(1_f64))
        .await
        .expect_err("direct RPC must be denied");
    assert!(
        error
            .to_string()
            .contains("direct invocation denied before schema lookup"),
        "unexpected error: {error}"
    );
    Ok(())
}

#[test]
#[tracing::instrument]
async fn rust_rpc_with_payload(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] agent_rpc_rust: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component_dep(&context.default_environment_id, agent_rpc_rust)
        .store()
        .await?;

    let parent_agent_id = agent_id!("RustParent", "rust_rpc_with_payload");
    let parent = executor
        .start_agent(&component.id, parent_agent_id.clone())
        .await?;

    executor.log_output(&parent).await?;

    let spawn_result = executor
        .invoke_and_await_agent(
            &component,
            &parent_agent_id,
            "spawn_child",
            data_value!("hello world"),
        )
        .await?;

    let uuid_as_value = spawn_result
        .into_return_value()
        .expect("Expected a single return value");

    let uuid = <uuid::Uuid as FromSchema>::from_value(&uuid_as_value).expect("UUID expected");

    let child_agent_id = agent_id!("RustChild", uuid);

    let get_result = executor
        .invoke_and_await_agent(&component, &child_agent_id, "get", data_value!())
        .await?;

    let option_payload_as_value = get_result
        .into_return_value()
        .expect("Expected a single return value");

    executor.check_oplog_is_queryable(&parent).await?;

    assert_eq!(
        option_payload_as_value,
        SchemaValue::Option {
            inner: Some(Box::new(SchemaValue::Record {
                fields: vec![
                    SchemaValue::String("hello world".to_string()),
                    uuid_as_value.clone(),
                    SchemaValue::Enum { case: 0 }
                ],
            }))
        }
    );
    Ok(())
}

#[test]
#[tracing::instrument]
async fn rust_rpc_missing_target(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] agent_rpc_rust: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component_dep(&context.default_environment_id, agent_rpc_rust)
        .store()
        .await?;

    let parent_agent_id = agent_id!("RustParent", "rust_rpc_with_payload");
    let parent = executor
        .start_agent(&component.id, parent_agent_id.clone())
        .await?;

    executor.log_output(&parent).await?;

    let call_result = executor
        .invoke_and_await_agent(
            &component,
            &parent_agent_id,
            "call_ts_agent",
            data_value!("example"),
        )
        .await;

    assert!(
        call_result
            .err()
            .unwrap()
            .to_string()
            .contains("Agent type not registered")
    );

    let oplog = executor
        .get_oplog(&parent, golem_common::model::oplog::OplogIndex::INITIAL)
        .await?;
    assert!(oplog.iter().any(|entry| matches!(
        entry.entry,
        golem_common::model::oplog::PublicOplogEntry::Error(_)
    )));

    Ok(())
}

#[test]
#[tracing::instrument]
async fn rust_rpc_missing_target_is_recoverable_with_fallible_create(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] agent_rpc_rust: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component_dep(&context.default_environment_id, agent_rpc_rust)
        .store()
        .await?;

    let parent_agent_id = agent_id!("RustParent", "fallible-create-missing-target");
    let parent = executor
        .start_agent(&component.id, parent_agent_id.clone())
        .await?;

    let result = executor
        .invoke_and_await_agent(
            &component,
            &parent_agent_id,
            "inspect_missing_rpc_type",
            data_value!(),
        )
        .await?
        .into_typed::<String>()?;

    assert!(result.contains("RemoteAgentError"));
    assert!(result.contains("InvalidType"));
    assert!(result.contains("MissingReflectedType"));

    let oplog = executor
        .get_oplog(&parent, golem_common::model::oplog::OplogIndex::INITIAL)
        .await?;
    assert!(oplog.iter().all(|entry| !matches!(
        entry.entry,
        golem_common::model::oplog::PublicOplogEntry::Error(_)
    )));

    Ok(())
}

#[test]
#[tracing::instrument]
async fn counter_resource_test_1(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] agent_rpc_rust: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component_dep(&context.default_environment_id, agent_rpc_rust)
        .store()
        .await?;

    let agent_id = agent_id!("RpcCaller", "counter_resource_test_1");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    let result = executor
        .invoke_and_await_agent(&component, &agent_id, "test1", data_value!())
        .await?;

    executor.check_oplog_is_queryable(&worker_id).await?;

    let result_value = result
        .into_return_value()
        .expect("Expected a single return value");

    assert_eq!(
        result_value,
        SchemaValue::List {
            elements: vec![
                SchemaValue::Tuple {
                    elements: vec![
                        SchemaValue::String("counter_resource_test_1_test1_counter3".to_string()),
                        SchemaValue::U64(3)
                    ]
                },
                SchemaValue::Tuple {
                    elements: vec![
                        SchemaValue::String("counter_resource_test_1_test1_counter2".to_string()),
                        SchemaValue::U64(3)
                    ]
                },
                SchemaValue::Tuple {
                    elements: vec![
                        SchemaValue::String("counter_resource_test_1_test1_counter1".to_string()),
                        SchemaValue::U64(3)
                    ]
                }
            ]
        }
    );

    Ok(())
}

#[test]
#[tracing::instrument]
async fn counter_resource_test_2(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] agent_rpc_rust: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component_dep(&context.default_environment_id, agent_rpc_rust)
        .store()
        .await?;

    let agent_id = agent_id!("RpcCaller", "counter_resource_test_2");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    let result1 = executor
        .invoke_and_await_agent(&component, &agent_id, "test2", data_value!())
        .await?;

    let result2 = executor
        .invoke_and_await_agent(&component, &agent_id, "test2", data_value!())
        .await?;

    executor.check_oplog_is_queryable(&worker_id).await?;

    let result_value1 = result1.into_typed::<u64>()?;
    let result_value2 = result2.into_typed::<u64>()?;

    assert_eq!(result_value1, 1);
    assert_eq!(result_value2, 2);

    Ok(())
}

#[test]
#[tracing::instrument]
async fn counter_resource_test_2_with_restart(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] agent_rpc_rust: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component_dep(&context.default_environment_id, agent_rpc_rust)
        .store()
        .await?;

    let agent_id = agent_id!("RpcCaller", "counter_resource_test_2_with_restart");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    let result1 = executor
        .invoke_and_await_agent(&component, &agent_id, "test2", data_value!())
        .await?;

    drop(executor);
    let executor = start(deps, &context).await?;

    let result2 = executor
        .invoke_and_await_agent(&component, &agent_id, "test2", data_value!())
        .await?;

    executor.check_oplog_is_queryable(&worker_id).await?;

    let result_value1 = result1.into_typed::<u64>()?;
    let result_value2 = result2.into_typed::<u64>()?;

    assert_eq!(result_value1, 1);
    assert_eq!(result_value2, 2);

    Ok(())
}

#[test]
#[timeout("2m")]
#[tracing::instrument]
async fn completed_fire_and_forget_rpc_replays_span_before_result(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    #[tagged_as("agent_counters")] agent_counters: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let caller_component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;
    let counter_component = executor
        .component_dep(&context.default_environment_id, agent_counters)
        .store()
        .await?;
    let caller = agent_id!("GolemHostApi", "fire-and-forget-replay-caller");
    let counter = agent_id!("Counter", "fire-and-forget-replay-target");

    executor
        .start_agent(&caller_component.id, caller.clone())
        .await?;
    let invoked = executor
        .invoke_and_await_agent(
            &caller_component,
            &caller,
            "outbound_agent_rpc_invoke_result",
            data_value!("Counter", "fire-and-forget-replay-target", "increment"),
        )
        .await?
        .into_typed::<Result<(), String>>()?;
    assert_eq!(invoked, Ok(()));

    let after_delivery = executor
        .invoke_and_await_agent(&counter_component, &counter, "increment", data_value!())
        .await?
        .into_typed::<u32>()?;
    assert_eq!(after_delivery, 2);

    drop(executor);
    let executor = start(deps, &context).await?;

    executor
        .invoke_and_await_agent(&caller_component, &caller, "get_self_uri", data_value!())
        .await?;

    let after_replay = executor
        .invoke_and_await_agent(&counter_component, &counter, "increment", data_value!())
        .await?
        .into_typed::<u32>()?;
    assert_eq!(after_replay, 3);

    Ok(())
}

#[test]
#[timeout("6m")]
#[tracing::instrument]
async fn fire_and_forget_rpc_recovers_from_committed_crash_prefixes(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    #[tagged_as("agent_counters")] agent_counters: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    for (checkpoint, suffix) in [
        (FireAndForgetRpcCheckpoint::Start, "start"),
        (FireAndForgetRpcCheckpoint::StartSpan, "start-span"),
        (FireAndForgetRpcCheckpoint::End, "end"),
    ] {
        let context = TestContext::new(last_unique_id);
        let executor = start(deps, &context).await?;
        let caller_component = executor
            .component_dep(&context.default_environment_id, host_api_tests)
            .store()
            .await?;
        let counter_component = executor
            .component_dep(&context.default_environment_id, agent_counters)
            .store()
            .await?;
        let caller_id = agent_id!("GolemHostApi", format!("fire-and-forget-{suffix}-caller"));
        let caller = executor
            .start_agent(&caller_component.id, caller_id.clone())
            .await?;
        let counter_name = format!("fire-and-forget-{suffix}-target");
        let counter_id = agent_id!("Counter", counter_name.clone());
        let invocation_key = IdempotencyKey::fresh();
        let mut gate = executor
            .gate_next_fire_and_forget_rpc_commit(&caller, checkpoint)
            .await;

        let invocation = {
            let executor = executor.clone();
            let caller_component = caller_component.clone();
            let caller_id = caller_id.clone();
            let invocation_key = invocation_key.clone();
            let counter_name = counter_name.clone();
            tokio::spawn(async move {
                executor
                    .invoke_and_await_agent_with_key(
                        &caller_component,
                        &caller_id,
                        &invocation_key,
                        "outbound_agent_rpc_invoke_result",
                        data_value!("Counter", counter_name, "increment"),
                    )
                    .await
            })
        };
        gate.committed().await;

        let prefix = executor.get_oplog(&caller, OplogIndex::INITIAL).await?;
        let rpc_starts = rpc_start_indices(&prefix);
        assert_eq!(rpc_starts.len(), 1, "checkpoint {checkpoint:?}");
        let prefix_span_count = prefix
            .iter()
            .filter(|entry| {
                entry.oplog_index > rpc_starts[0]
                    && matches!(&entry.entry, PublicOplogEntry::StartSpan(_))
            })
            .count();
        let prefix_end_count = prefix
            .iter()
            .filter(|entry| matches!(&entry.entry, PublicOplogEntry::End(end) if end.start_index == rpc_starts[0]))
            .count();
        assert_eq!(
            prefix_span_count,
            usize::from(checkpoint != FireAndForgetRpcCheckpoint::Start),
            "wrong span prefix at {checkpoint:?}: {prefix:#?}"
        );
        assert_eq!(
            prefix_end_count,
            usize::from(checkpoint == FireAndForgetRpcCheckpoint::End),
            "wrong terminal prefix at {checkpoint:?}: {prefix:#?}"
        );
        assert_eq!(
            prefix
                .iter()
                .filter(|entry| {
                    entry.oplog_index > rpc_starts[0]
                        && matches!(&entry.entry, PublicOplogEntry::FinishSpan(_))
                })
                .count(),
            0,
            "checkpoint {checkpoint:?} must precede FinishSpan: {prefix:#?}"
        );

        gate.abort_return();
        invocation.abort();
        drop(gate);
        drop(executor);

        let executor = start(deps, &context).await?;

        let reused = executor
            .invoke_and_await_agent_with_key(
                &caller_component,
                &caller_id,
                &invocation_key,
                "outbound_agent_rpc_invoke_result",
                data_value!("Counter", counter_name, "increment"),
            )
            .await?
            .into_typed::<Result<(), String>>()?;
        assert_eq!(reused, Ok(()), "checkpoint {checkpoint:?}");

        let next_counter_value = executor
            .invoke_and_await_agent(&counter_component, &counter_id, "increment", data_value!())
            .await?
            .into_typed::<u32>()?;
        assert_eq!(
            next_counter_value, 2,
            "recovery must execute one callee effect at {checkpoint:?}"
        );

        let recovered = executor.get_oplog(&caller, OplogIndex::INITIAL).await?;
        assert_eq!(
            rpc_start_indices(&recovered),
            rpc_starts,
            "recovery must retain the original RPC identity at {checkpoint:?}"
        );
        assert_eq!(
            recovered
                .iter()
                .filter(|entry| matches!(&entry.entry, PublicOplogEntry::End(end) if end.start_index == rpc_starts[0]))
                .count(),
            1,
            "recovery must attach one terminal to the original RPC Start at {checkpoint:?}"
        );
        let span_ids = recovered
            .iter()
            .filter_map(|entry| match &entry.entry {
                PublicOplogEntry::StartSpan(span) if entry.oplog_index > rpc_starts[0] => {
                    Some(span.span_id.clone())
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(span_ids.len(), 1, "checkpoint {checkpoint:?}");
        assert_eq!(
            recovered
                .iter()
                .filter(|entry| matches!(&entry.entry, PublicOplogEntry::FinishSpan(span) if span.span_id == span_ids[0]))
                .count(),
            1,
            "recovery must repair the original invocation span at {checkpoint:?}"
        );
    }

    Ok(())
}

fn rpc_start_indices(
    oplog: &[golem_common::model::oplog::PublicOplogEntryWithIndex],
) -> Vec<OplogIndex> {
    oplog
        .iter()
        .filter_map(|entry| {
            matches!(
                &entry.entry,
                PublicOplogEntry::Start(start)
                    if start.function_name == "golem::rpc::wasm-rpc::invoke"
            )
            .then_some(entry.oplog_index)
        })
        .collect()
}

#[test]
#[tracing::instrument]
async fn counter_resource_test_3(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] agent_rpc_rust: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component_dep(&context.default_environment_id, agent_rpc_rust)
        .store()
        .await?;

    let agent_id = agent_id!("RpcCaller", "counter_resource_test_3");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    let result1 = executor
        .invoke_and_await_agent(&component, &agent_id, "test3", data_value!())
        .await?;

    let result2 = executor
        .invoke_and_await_agent(&component, &agent_id, "test3", data_value!())
        .await?;

    executor.check_oplog_is_queryable(&worker_id).await?;

    let result_value1 = result1.into_typed::<u64>()?;
    let result_value2 = result2.into_typed::<u64>()?;

    assert_eq!(result_value1, 1);
    assert_eq!(result_value2, 2);

    Ok(())
}

#[test]
#[tracing::instrument]
async fn counter_resource_test_3_with_restart(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] agent_rpc_rust: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component_dep(&context.default_environment_id, agent_rpc_rust)
        .store()
        .await?;

    let agent_id = agent_id!("RpcCaller", "counter_resource_test_3_with_restart");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    let result1 = executor
        .invoke_and_await_agent(&component, &agent_id, "test3", data_value!())
        .await?;

    drop(executor);
    let executor = start(deps, &context).await?;

    let result2 = executor
        .invoke_and_await_agent(&component, &agent_id, "test3", data_value!())
        .await?;

    executor.check_oplog_is_queryable(&worker_id).await?;

    let result_value1 = result1.into_typed::<u64>()?;
    let result_value2 = result2.into_typed::<u64>()?;

    assert_eq!(result_value1, 1);
    assert_eq!(result_value2, 2);

    Ok(())
}

#[test]
#[tracing::instrument]
async fn context_inheritance(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] agent_rpc_rust: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component_dep(&context.default_environment_id, agent_rpc_rust)
        .store()
        .await?;

    let agent_id = agent_id!("RpcCaller", "context_inheritance");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    let result = executor
        .invoke_and_await_agent(&component, &agent_id, "test4", data_value!())
        .await?;

    executor.check_oplog_is_queryable(&worker_id).await?;

    let result_value = result
        .into_return_value()
        .expect("Expected a single return value");

    let result_tuple = match &result_value {
        SchemaValue::Tuple { elements } => elements,
        _ => panic!("Unexpected result: {result_value:?}"),
    };
    let args = match &result_tuple[0] {
        SchemaValue::List { elements } => elements.clone(),
        _ => panic!("Unexpected result: {result_value:?}"),
    };
    let mut env = match &result_tuple[1] {
        SchemaValue::List { elements } => elements
            .clone()
            .into_iter()
            .map(|value| match value {
                SchemaValue::Tuple { elements } => match (&elements[0], &elements[1]) {
                    (SchemaValue::String(key), SchemaValue::String(value)) => {
                        (key.clone(), value.clone())
                    }
                    _ => panic!("Unexpected result: {result_value:?}"),
                },
                _ => panic!("Unexpected result: {result_value:?}"),
            })
            .collect::<Vec<_>>(),
        _ => panic!("Unexpected result: {result_value:?}"),
    };
    env.sort_by_key(|(k, _v)| k.clone());

    assert_eq!(args, vec![] as Vec<SchemaValue>);

    let env_keys: Vec<&str> = env.iter().map(|(k, _)| k.as_str()).collect();
    assert!(
        env_keys.contains(&"GOLEM_AGENT_ID"),
        "Expected GOLEM_AGENT_ID in env, got: {env:?}"
    );
    assert!(
        env_keys.contains(&"GOLEM_WORKER_NAME"),
        "Expected GOLEM_WORKER_NAME in env, got: {env:?}"
    );
    assert!(
        env_keys.contains(&"GOLEM_COMPONENT_ID"),
        "Expected GOLEM_COMPONENT_ID in env, got: {env:?}"
    );
    assert!(
        env_keys.contains(&"GOLEM_COMPONENT_REVISION"),
        "Expected GOLEM_COMPONENT_REVISION in env, got: {env:?}"
    );
    assert!(
        env_keys.contains(&"GOLEM_AGENT_TYPE"),
        "Expected GOLEM_AGENT_TYPE in env, got: {env:?}"
    );

    Ok(())
}

#[test]
#[tracing::instrument]
async fn counter_resource_test_5(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] agent_rpc_rust: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component_dep(&context.default_environment_id, agent_rpc_rust)
        .store()
        .await?;

    let agent_id = agent_id!("RpcCaller", "counter_resource_test_5");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    let result = executor
        .invoke_and_await_agent(&component, &agent_id, "test5", data_value!())
        .await?;

    executor.check_oplog_is_queryable(&worker_id).await?;

    let result_value = result
        .into_return_value()
        .expect("Expected a single return value");

    assert_eq!(
        result_value,
        SchemaValue::List {
            elements: vec![
                SchemaValue::U64(3),
                SchemaValue::U64(3),
                SchemaValue::U64(3),
            ]
        }
    );

    Ok(())
}

#[test]
#[tracing::instrument]
async fn wasm_rpc_bug_32_test(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] agent_rpc_rust: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component_dep(&context.default_environment_id, agent_rpc_rust)
        .store()
        .await?;

    let agent_id = agent_id!("RpcCaller", "wasm_rpc_bug_32_test");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    let input = crate::raw_params(vec![SchemaValue::Enum { case: 0 }]);

    let result = executor
        .invoke_and_await_agent(&component, &agent_id, "bug_wasm_rpc_i32", input)
        .await?;

    executor.check_oplog_is_queryable(&worker_id).await?;

    let result_value = result
        .into_return_value()
        .expect("Expected a single return value");

    assert_eq!(result_value, SchemaValue::Enum { case: 0 });

    Ok(())
}

#[test]
#[tracing::instrument]
async fn golem_bug_1265_test(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] agent_rpc_rust: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component_dep(&context.default_environment_id, agent_rpc_rust)
        .store()
        .await?;

    let agent_id = agent_id!("RpcCaller", "golem_bug_1265_test");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    let result = executor
        .invoke_and_await_agent(&component, &agent_id, "bug_golem1265", data_value!("test"))
        .await?;

    executor.check_oplog_is_queryable(&worker_id).await?;

    let result_value = result
        .into_return_value()
        .expect("Expected a single return value");

    assert_eq!(
        result_value,
        SchemaValue::Result(ResultValuePayload::Ok { value: None })
    );

    Ok(())
}

#[test]
#[tracing::instrument]
async fn ephemeral_worker_invocation_via_rpc1(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_counters")] agent_counters: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component_dep(&context.default_environment_id, agent_counters)
        .store()
        .await?;
    let agent_id = agent_id!("Counter", "ephemeral_worker_invocation_via_rpc1");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    let _ = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "increment_through_rpc_to_ephemeral",
            data_value!(),
        )
        .await?;
    let result = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "increment_through_rpc_to_ephemeral",
            data_value!(),
        )
        .await?;

    executor.check_oplog_is_queryable(&worker_id).await?;
    drop(executor);

    let value = result.into_typed::<u32>()?;
    assert_eq!(value, 1);

    Ok(())
}

#[test]
#[tracing::instrument]
async fn ephemeral_worker_invocation_via_rpc2(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_counters")] agent_counters: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component_dep(&context.default_environment_id, agent_counters)
        .store()
        .await?;
    let agent_id = agent_id!("Counter", "ephemeral_worker_invocation_via_rpc2");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    let _ = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "increment_through_rpc_to_ephemeral_phantom",
            data_value!(),
        )
        .await;
    let result = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "increment_through_rpc_to_ephemeral_phantom",
            data_value!(),
        )
        .await?;

    executor.check_oplog_is_queryable(&worker_id).await?;
    drop(executor);

    let value = result.into_typed::<u32>()?;
    assert_eq!(value, 1);

    Ok(())
}

#[test]
#[tracing::instrument]
async fn ephemeral_rpc_invocations_get_distinct_final_identities(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_counters")] agent_counters: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component_dep(&context.default_environment_id, agent_counters)
        .store()
        .await?;
    let agent_id = agent_id!("Counter", "ephemeral_rpc_distinct_ids");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    // Two sequential RPC method calls on the same ephemeral client proxy must
    // each derive a distinct final agent identity from their own durable
    // idempotency key.
    let result = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "ephemeral_ids_through_rpc",
            data_value!(),
        )
        .await?;

    executor.check_oplog_is_queryable(&worker_id).await?;
    drop(executor);

    let (id1, id2) = result.into_typed::<(String, String)>()?;
    assert!(!id1.is_empty());
    assert!(!id2.is_empty());
    assert_ne!(id1, id2);

    Ok(())
}

#[test]
#[tracing::instrument]
async fn ephemeral_agent_self_rpc_is_allowed(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_counters")] agent_counters: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component_dep(&context.default_environment_id, agent_counters)
        .store()
        .await?;

    // An ephemeral agent invoking its own logical identity via RPC is allowed:
    // the callee is always a fresh instance with a freshly derived identity,
    // never the caller's own invocation queue.
    let agent_id = agent_id!("EphemeralCounter", "ephemeral_self_rpc");
    let result = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "increment_via_self_rpc",
            data_value!(),
        )
        .await?;

    drop(executor);

    let value = result.into_typed::<u32>()?;
    assert_eq!(value, 2);

    Ok(())
}

#[test]
#[tracing::instrument]
async fn failed_ephemeral_invocation_retry_does_not_reexecute(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_counters")] agent_counters: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component_dep(&context.default_environment_id, agent_counters)
        .store()
        .await?;

    let agent_id = agent_id!("EphemeralCounter", "ephemeral_crash_retry");
    let idempotency_key = IdempotencyKey::fresh();

    // The ephemeral agent increments a durable counter via RPC and then
    // panics, so the durable counter observes how many times the method body
    // actually executed.
    let first = executor
        .invoke_and_await_agent_with_key(
            &component,
            &agent_id,
            &idempotency_key,
            "increment_remote_then_fail",
            data_value!("ephemeral_crash_retry_target"),
        )
        .await;
    assert!(first.is_err());

    // Retrying the failed invocation with the same idempotency key must not
    // execute the method body again.
    let second = executor
        .invoke_and_await_agent_with_key(
            &component,
            &agent_id,
            &idempotency_key,
            "increment_remote_then_fail",
            data_value!("ephemeral_crash_retry_target"),
        )
        .await;
    assert!(second.is_err());

    let target_agent_id = agent_id!("Counter", "ephemeral_crash_retry_target");
    let count = executor
        .invoke_and_await_agent(&component, &target_agent_id, "increment", data_value!())
        .await?
        .into_typed::<u32>()?;

    drop(executor);

    // 1 increment from the single execution of the failing method + 1 from
    // the observation call itself; a re-executed method body would make it 3.
    assert_eq!(count, 2);

    Ok(())
}

#[test]
#[tracing::instrument]
async fn cancel_pending_async_rpc_returns_error(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] agent_rpc_rust: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component_dep(&context.default_environment_id, agent_rpc_rust)
        .store()
        .await?;

    let agent_id = agent_id!("CancelTester", "cancel_pending_test");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    // Call test_cancel_before_await - initiates async RPC to inc_by, then cancels
    executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "test_cancel_before_await",
            data_value!("cancel_pending_counter"),
        )
        .await?;

    executor.check_oplog_is_queryable(&worker_id).await?;

    // The test verifies that cancel() doesn't panic/trap and completes successfully.
    // Cancel is "best-effort" — the remote invocation may or may not have already
    // executed by the time cancel is processed.

    Ok(())
}

#[test]
#[tracing::instrument]
async fn cancel_completed_async_rpc_is_noop(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] agent_rpc_rust: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component_dep(&context.default_environment_id, agent_rpc_rust)
        .store()
        .await?;

    let agent_id = agent_id!("CancelTester", "cancel_completed_test");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    let result = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "test_cancel_completed",
            data_value!("cancel_completed_counter"),
        )
        .await?;

    executor.check_oplog_is_queryable(&worker_id).await?;

    let result_value = result.into_typed::<u64>()?;

    // The counter was incremented by 5, so get_value should return 5
    assert_eq!(result_value, 5);

    Ok(())
}

#[test]
#[tracing::instrument]
async fn ts_abort_before_await_returns_aborted(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc")] agent_rpc: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component_dep(&context.default_environment_id, agent_rpc)
        .store()
        .await?;

    let agent_id = agent_id!("TsCancelTester", "ts_abort_test1");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    let result = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "testAbortBeforeAwait",
            data_value!("ts_abort_counter1"),
        )
        .await?;

    executor.check_oplog_is_queryable(&worker_id).await?;

    let result_value = result.into_typed::<String>()?;

    assert_eq!(result_value, "aborted".to_string());

    Ok(())
}

#[test]
#[tracing::instrument]
async fn ts_abort_after_complete_is_noop(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc")] agent_rpc: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component_dep(&context.default_environment_id, agent_rpc)
        .store()
        .await?;

    let agent_id = agent_id!("TsCancelTester", "ts_abort_test2");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    let result = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "testAbortAfterComplete",
            data_value!("ts_abort_counter2"),
        )
        .await?;

    executor.check_oplog_is_queryable(&worker_id).await?;

    let result_value = result.into_typed::<f64>()?;

    // The counter was incremented by 5, so getValue should return 5.0
    assert_eq!(result_value, 5.0);

    Ok(())
}

#[test]
#[timeout("60s")]
#[tracing::instrument]
async fn ts_ephemeral_final_identity_cannot_be_reused(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc")] agent_rpc: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;
    let component = executor
        .component_dep(&context.default_environment_id, agent_rpc)
        .store()
        .await?;
    let agent_id = agent_id!("TestAgent", "ts_ephemeral_final_identity_cannot_be_reused");

    let report = executor
        .invoke_and_await_agent(&component, &agent_id, "ephemeralReuseTest", data_value!())
        .await?
        .into_return_value()
        .expect("expected an ephemeral reuse report");
    let SchemaValue::Record { fields } = report else {
        panic!("expected an ephemeral reuse report record");
    };
    let [
        value,
        final_agent_id,
        idempotency_key,
        category,
        error_tag,
        details,
    ] = fields.as_slice()
    else {
        panic!("expected six fields in the ephemeral reuse report");
    };

    assert_eq!(value, &SchemaValue::String("captured".to_string()));
    assert!(
        matches!(final_agent_id, SchemaValue::String(value) if !value.is_empty()),
        "final ephemeral agent ID must be non-empty"
    );
    assert!(
        matches!(idempotency_key, SchemaValue::String(value) if !value.is_empty()),
        "ephemeral invocation idempotency key must be non-empty"
    );
    assert_eq!(
        category,
        &SchemaValue::String("remote-agent-error".to_string())
    );
    assert_eq!(error_tag, &SchemaValue::String("invalid-input".to_string()));
    assert!(
        matches!(details, SchemaValue::String(value) if value.contains("An ephemeral agent cannot accept another invocation or be resumed")),
        "unexpected ephemeral reuse details: {details:?}"
    );

    Ok(())
}

#[test]
#[timeout("60s")]
#[tracing::instrument]
async fn ts_reflection_discovers_binds_and_invokes_durable_agent(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc")] agent_rpc: &PrecompiledComponent,
    #[tagged_as("agent_rpc_rust")] agent_rpc_rust: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;
    let component = executor
        .component_dep(&context.default_environment_id, agent_rpc)
        .store()
        .await?;
    let target_component = executor
        .component_dep(&context.default_environment_id, agent_rpc_rust)
        .store()
        .await?;
    assert_ne!(component.id, target_component.id);
    let agent_id = agent_id!(
        "TestAgent",
        "ts_reflection_discovers_binds_and_invokes_durable_agent"
    );

    let report = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "reflectionDiscoveryTest",
            data_value!(),
        )
        .await?
        .into_return_value()
        .expect("expected a reflection discovery report");
    let SchemaValue::Record { fields } = report else {
        panic!("expected a reflection discovery report record");
    };
    let [
        listed,
        type_name,
        method_name,
        first_value,
        second_value,
        missing_name,
        missing_id,
    ] = fields.as_slice()
    else {
        panic!("expected seven fields in the reflection discovery report");
    };

    assert_eq!(listed, &SchemaValue::Bool(true));
    assert_eq!(type_name, &SchemaValue::String("Counter".to_string()));
    assert_eq!(method_name, &SchemaValue::String("get_value".to_string()));
    assert_eq!(
        first_value,
        &SchemaValue::String(
            "counter-reflection-ts_reflection_discovers_binds_and_invokes_durable_agent"
                .to_string()
        )
    );
    assert_eq!(second_value, first_value);
    assert_eq!(missing_name, &SchemaValue::Bool(true));
    assert_eq!(missing_id, &SchemaValue::Bool(true));

    Ok(())
}

#[test]
#[timeout("60s")]
#[tracing::instrument]
async fn ts_reflected_ephemeral_invocation_returns_final_metadata(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc")] agent_rpc: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;
    let component = executor
        .component_dep(&context.default_environment_id, agent_rpc)
        .store()
        .await?;
    let agent_id = agent_id!(
        "TestAgent",
        "ts_reflected_ephemeral_invocation_returns_final_metadata"
    );

    let report = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "reflectedEphemeralTest",
            data_value!(),
        )
        .await?
        .into_return_value()
        .expect("expected a reflected ephemeral report");
    let SchemaValue::Record { fields } = report else {
        panic!("expected a reflected ephemeral report record");
    };
    let [value, final_agent_id, idempotency_key, proxy_has_agent_id] = fields.as_slice() else {
        panic!("expected four fields in the reflected ephemeral report");
    };

    assert_eq!(value, &SchemaValue::String("reflected".to_string()));
    assert!(
        matches!(final_agent_id, SchemaValue::String(value) if !value.is_empty()),
        "final reflected ephemeral agent ID must be non-empty"
    );
    assert!(
        matches!(idempotency_key, SchemaValue::String(value) if !value.is_empty()),
        "reflected ephemeral idempotency key must be non-empty"
    );
    assert_eq!(proxy_has_agent_id, &SchemaValue::Bool(false));

    Ok(())
}

fn extract_oplog_idx_from_promise_id(promise_id_value: &SchemaValue) -> OplogIndex {
    let SchemaValue::Record { fields } = promise_id_value else {
        panic!("Expected a record for PromiseId");
    };
    let SchemaValue::U64(oplog_idx) = fields[1] else {
        panic!("Expected u64 oplog-idx field");
    };
    OplogIndex::from_u64(oplog_idx)
}

#[test]
#[tracing::instrument]
async fn ts_cancel_unblocks_caller_while_callee_blocked(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc")] agent_rpc: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component_dep(&context.default_environment_id, agent_rpc)
        .store()
        .await?;

    // Start agent B (TsBlockingAgent) and prepare a promise
    let b_name = "cancel_unblocks_b";
    let b_agent_id = agent_id!("TsBlockingAgent", b_name);
    let b_worker_id = executor
        .start_agent(&component.id, b_agent_id.clone())
        .await?;

    let prepare_result = executor
        .invoke_and_await_agent(&component, &b_agent_id, "prepareBlock", data_value!())
        .await?;

    let promise_id_value = prepare_result
        .into_return_value()
        .expect("Expected a single return value from prepareBlock");

    let oplog_idx = extract_oplog_idx_from_promise_id(&promise_id_value);

    // Start agent A (TsCancelCallerAgent)
    let a_name = "cancel_unblocks_a";
    let a_agent_id = agent_id!("TsCancelCallerAgent", a_name);
    let _a_worker_id = executor
        .start_agent(&component.id, a_agent_id.clone())
        .await?;

    // Spawn fiber: A.callAndAbort(bName, 3000ms delay before abort)
    let executor_clone = executor.clone();
    let component_clone = component.clone();
    let a_agent_id_clone = a_agent_id.clone();

    let mut fiber = tokio::spawn(
        async move {
            executor_clone
                .invoke_and_await_agent(
                    &component_clone,
                    &a_agent_id_clone,
                    "callAndAbort",
                    data_value!(b_name, 3000.0),
                )
                .await
        }
        .in_current_span(),
    );

    // Wait for B to suspend on the promise
    tokio::select! {
        result = &mut fiber => {
            let invoke_result = result??;
            return Err(anyhow::anyhow!("callAndAbort returned before B suspended: {:?}", invoke_result));
        }
        status = executor.wait_for_status(&b_worker_id, AgentStatus::Suspended, Duration::from_secs(30)) => {
            status?;
        }
    }

    // Now wait for A's result (abort fires at 3s, so A should complete relatively soon)
    let a_result = fiber.await??;
    let a_value = a_result.into_typed::<String>()?;
    assert_eq!(a_value, "aborted".to_string());

    // B should still be suspended (cancel unblocked caller but NOT callee)
    let b_status = executor.get_worker_metadata(&b_worker_id).await?.status;
    assert_eq!(b_status, AgentStatus::Suspended);

    // Complete the promise to unblock B
    executor
        .complete_promise(
            &PromiseId {
                agent_id: b_worker_id.clone(),
                oplog_idx,
            },
            vec![],
        )
        .await?;

    // Wait for B to return to Idle
    executor
        .wait_for_status(&b_worker_id, AgentStatus::Idle, Duration::from_secs(10))
        .await?;

    // Verify B processed the call
    let count_result = executor
        .invoke_and_await_agent(&component, &b_agent_id, "getCompletedCount", data_value!())
        .await?;

    let count_value = count_result.into_typed::<f64>()?;
    assert_eq!(count_value, 1.0);

    Ok(())
}

#[test]
#[tracing::instrument]
async fn ts_cancel_survives_executor_restart(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc")] agent_rpc: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component_dep(&context.default_environment_id, agent_rpc)
        .store()
        .await?;

    // Start agent B (TsBlockingAgent) and prepare a promise
    let b_name = "cancel_restart_b";
    let b_agent_id = agent_id!("TsBlockingAgent", b_name);
    let b_worker_id = executor
        .start_agent(&component.id, b_agent_id.clone())
        .await?;

    let prepare_result = executor
        .invoke_and_await_agent(&component, &b_agent_id, "prepareBlock", data_value!())
        .await?;

    let promise_id_value = prepare_result
        .into_return_value()
        .expect("Expected a single return value from prepareBlock");

    let oplog_idx = extract_oplog_idx_from_promise_id(&promise_id_value);

    // Start agent A (TsCancelCallerAgent)
    let a_name = "cancel_restart_a";
    let a_agent_id = agent_id!("TsCancelCallerAgent", a_name);
    let _a_worker_id = executor
        .start_agent(&component.id, a_agent_id.clone())
        .await?;

    // Spawn fiber: A.callAndAbort(bName, 3000ms)
    let executor_clone = executor.clone();
    let component_clone = component.clone();
    let a_agent_id_clone = a_agent_id.clone();

    let mut fiber = tokio::spawn(
        async move {
            executor_clone
                .invoke_and_await_agent(
                    &component_clone,
                    &a_agent_id_clone,
                    "callAndAbort",
                    data_value!(b_name, 3000.0),
                )
                .await
        }
        .in_current_span(),
    );

    // Wait for B to suspend on the promise
    tokio::select! {
        result = &mut fiber => {
            let invoke_result = result??;
            return Err(anyhow::anyhow!("callAndAbort returned before B suspended: {:?}", invoke_result));
        }
        status = executor.wait_for_status(&b_worker_id, AgentStatus::Suspended, Duration::from_secs(30)) => {
            status?;
        }
    }

    // Wait for A's result
    let a_result = fiber.await??;
    let a_value = a_result.into_typed::<String>()?;
    assert_eq!(a_value, "aborted".to_string());

    // Restart executor
    drop(executor);
    let executor = start(deps, &context).await?;

    // After restart, B should still be suspended (replayed from oplog)
    executor
        .wait_for_status(
            &b_worker_id,
            AgentStatus::Suspended,
            Duration::from_secs(30),
        )
        .await?;

    // Verify A's state survived restart
    let outcome_result = executor
        .invoke_and_await_agent(&component, &a_agent_id, "getLastOutcome", data_value!())
        .await?;

    let outcome_value = outcome_result.into_typed::<String>()?;
    assert_eq!(outcome_value, "aborted".to_string());

    // Complete the promise to unblock B
    executor
        .complete_promise(
            &PromiseId {
                agent_id: b_worker_id.clone(),
                oplog_idx,
            },
            vec![],
        )
        .await?;

    // Wait for B to return to Idle
    executor
        .wait_for_status(&b_worker_id, AgentStatus::Idle, Duration::from_secs(10))
        .await?;

    // Verify B processed the call
    let count_result = executor
        .invoke_and_await_agent(&component, &b_agent_id, "getCompletedCount", data_value!())
        .await?;

    let count_value = count_result.into_typed::<f64>()?;
    assert_eq!(count_value, 1.0);

    Ok(())
}
