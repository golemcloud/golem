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
use golem_api_grpc::invocation_session_protocol::InvocationSessionState;
use golem_api_grpc::proto::golem::schema::{RecordValue, SchemaValueStreamReference, schema_value};
use golem_api_grpc::proto::golem::worker::{
    InvocationFailureKind, InvocationRejectionReason, InvocationRequest, InvocationResponse,
    InvocationStart, ResumeAttach, StreamCancel, StreamCancelReason, StreamCancelRole,
    invocation_request, invocation_response, invocation_session_completion,
    invocation_session_result,
};
use golem_common::model::account::AccountId;
use golem_common::model::agent::ParsedAgentId;
use golem_common::model::card::{AgentResourcePattern, AgentVerb};
use golem_common::model::component::ComponentDto;
use golem_common::model::oplog::{OplogIndex, PublicAgentInvocation, PublicOplogEntry};
use golem_common::model::{AgentId, AgentStatus, IdempotencyKey, OwnedAgentId, PromiseId};
use golem_common::schema::schema_value::ResultValuePayload;
use golem_common::schema::{FromSchema, SchemaValue, TypedSchemaValue};
use golem_common::{agent_id, data_value};
use golem_service_base::model::auth::AuthCtx;
use golem_test_framework::dsl::TestDsl;
use golem_worker_executor::services::direct_invocation_auth::{
    DirectInvocationAuthService, EnvironmentOwnerAccountId,
};
use golem_worker_executor::services::rpc::RpcError;
use golem_worker_executor::worker::EvictionClass;
use golem_worker_executor_test_utils::{
    LastUniqueId, PrecompiledComponent, TestContext, TestExecutorOverrides, TestWorkerExecutor,
    WorkerExecutorTestDependencies, start, start_with_overrides,
};
use pretty_assertions::assert_eq;
use std::sync::Arc;
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
    #[tagged_as("large_dynamic_memory")]
    PrecompiledComponent
);
inherit_test_dep!(Tracing);

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
async fn resume_attach_is_terminally_rejected_without_finish(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;
    let idempotency_key = Some(IdempotencyKey::fresh().into());
    let (requests, receiver) = mpsc::channel(1);
    requests
        .send(InvocationRequest {
            request: Some(invocation_request::Request::ResumeAttach(ResumeAttach {
                idempotency_key: idempotency_key.clone(),
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
        .ok_or_else(|| anyhow::anyhow!("resume-attach returned no rejection"))?;
    assert!(matches!(
        response.response,
        Some(invocation_response::Response::Rejected(rejected))
            if rejected.reason == InvocationRejectionReason::ResumeUnsupported as i32
                && rejected.idempotency_key == idempotency_key
    ));
    assert!(responses.message().await?.is_none());
    Ok(())
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
    let worker_agent_id = AgentId::from_agent_id(component.id, &agent_id)
        .map_err(|error| anyhow::anyhow!("invalid agent id: {error}"))?;
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
                let value = match result.result {
                    Some(invocation_session_result::Result::MethodResult(value)) => value,
                    other => anyhow::bail!("expected a method result, got {other:?}"),
                };
                let stream_id = match value.value {
                    Some(schema_value::Value::TupleValue(tuple)) => match tuple.elements.first() {
                        Some(golem_api_grpc::proto::golem::schema::SchemaValue {
                            value: Some(schema_value::Value::StreamReference(reference)),
                        }) => reference.stream_id,
                        other => anyhow::bail!("expected first sibling stream, got {other:?}"),
                    },
                    other => anyhow::bail!("expected sibling tuple result, got {other:?}"),
                };
                let cancel = InvocationRequest {
                    request: Some(invocation_request::Request::StreamCancel(StreamCancel {
                        stream_id,
                        offset: 0,
                        role: StreamCancelRole::OutputConsumer as i32,
                        reason: StreamCancelReason::Cancelled as i32,
                        details: Some("consumer stopped reading".to_string()),
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
    let worker_agent_id = AgentId::from_agent_id(component.id, &agent_id)
        .map_err(|error| anyhow::anyhow!("invalid agent id: {error}"))?;
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
                terminal_stream_ids.push(end.stream_id);
            }
            Some(invocation_response::Response::OutputError(error)) => {
                terminal_stream_ids.push(error.stream_id);
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
                    !oplog.iter().any(|entry| matches!(
                        &entry.entry,
                        PublicOplogEntry::AgentInvocationStarted(started)
                            if matches!(
                                &started.invocation,
                                PublicAgentInvocation::AgentMethodInvocation(method)
                                    if method.method_name == "transform"
                            )
                    )),
                    "volatile streaming invocation leaked into the oplog"
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

#[test]
#[timeout("2 minutes")]
#[tracing::instrument]
async fn disconnect_immediately_after_acceptance_cancels_volatile_invocation(
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
            ..Default::default()
        })),
    };
    let (requests, receiver) = mpsc::channel(8);
    requests.send(start).await?;
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
    assert!(matches!(
        first.response,
        Some(invocation_response::Response::Accepted(_))
    ));

    drop(requests);
    drop(responses);

    let ping = tokio::time::timeout(
        Duration::from_secs(30),
        invoke_agent_session(&executor, &component, &agent_id, "ping", data_value!()),
    )
    .await
    .map_err(|_| anyhow::anyhow!("cancelled live invocation blocked the next invocation"))??
    .map_err(anyhow::Error::msg)?;
    assert_eq!(ping, SchemaValue::U64(42));

    let oplog = executor
        .get_oplog(&worker_agent_id, OplogIndex::INITIAL)
        .await?;
    assert!(
        !oplog.iter().any(|entry| matches!(
            &entry.entry,
            PublicOplogEntry::AgentInvocationStarted(started)
                if matches!(
                    &started.invocation,
                    PublicAgentInvocation::AgentMethodInvocation(method)
                        if method.method_name == "transform"
                )
        )),
        "disconnected live invocation leaked into the oplog"
    );
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
            idempotency_key: Some(IdempotencyKey::fresh().into()),
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
async fn output_drop_cancels_target_blocked_on_stream_input(
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
    let caller_agent_id = agent_id!("StreamingRpcCaller", "blocked-stream-input-cancellation");
    executor
        .start_agent(&component.id, caller_agent_id.clone())
        .await?;

    let result = invoke_agent_session(
        &executor,
        &component,
        &caller_agent_id,
        "cancel_transform_blocked_on_input",
        data_value!(),
    )
    .await?
    .map_err(anyhow::Error::msg)?;
    assert_eq!(
        result,
        SchemaValue::U64(42),
        "dropping output must cancel a target invocation blocked on its input before ping can run"
    );
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
