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
    InputStreamEnd, InputStreamItem, InvocationAccepted, InvocationFailureKind, InvocationRequest,
    InvocationResponse, InvocationStart, ResumeAttach, ResumeOperation, StreamCancel,
    StreamCancelReason, StreamCancelRole, StreamCursor, input_stream_item, invocation_request,
    invocation_response, invocation_session_completion, invocation_session_result,
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
use std::collections::{BTreeMap, BTreeSet};
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
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;
    let component = executor
        .component_dep(&context.default_environment_id, agent_rpc_rust)
        .store()
        .await?;
    let agent_id = agent_id!("StreamingRpcTarget", "output-restart");
    let worker_agent_id = executor.start_agent(&component.id, agent_id).await?;
    let metadata = executor.get_worker_metadata(&worker_agent_id).await?;
    let (_, input) = data_value!().into_parts();
    let start_request = InvocationRequest {
        request: Some(invocation_request::Request::Start(InvocationStart {
            agent_id: Some(worker_agent_id.into()),
            method_name: Some("produce_siblings".to_string()),
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
    while observed_output_items < 10 {
        let response = responses
            .message()
            .await?
            .ok_or_else(|| anyhow::anyhow!("streaming output ended before cursor checkpoint"))?;
        first_state
            .validate_response(&response)
            .map_err(anyhow::Error::msg)?;
        if let Some(invocation_response::Response::OutputItem(item)) = response.response {
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
    drop(requests);
    drop(responses);
    drop(executor);

    let executor = start(deps, &context).await?;
    let Some(invocation_request::Request::Start(start)) = start_request.request.as_ref() else {
        anyhow::bail!("durable output restart request is not Start");
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
    let mut mapped_outputs = 0;
    let mut output_items = 0;
    let mut output_ends = 0;
    let mut finished_successfully = false;
    while let Some(response) = responses.message().await? {
        recovered_state
            .validate_response(&response)
            .map_err(anyhow::Error::msg)?;
        match response.response {
            Some(invocation_response::Response::Accepted(_)) => {}
            Some(invocation_response::Response::Result(result)) => {
                mapped_outputs = result.new_stream_mappings.len();
            }
            Some(invocation_response::Response::OutputItem(_)) => output_items += 1,
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
            attempt_id: Some(uuid::Uuid::new_v4().into()),
            expected_callee_fingerprint: None,
            durable_input_mappings: Vec::new(),
            scope_card: None,
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

    let _ = executor.simulated_crash(&caller).await;
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
