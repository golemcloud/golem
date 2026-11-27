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

use super::*;
use golem_common::model::oplog::{FinishedResult, OplogEntry};
use golem_worker_executor::storage::keyvalue::fault_injecting::{
    FaultInjectingKeyValueStorage, KeyValueStorageFaults,
};
use pretty_assertions::assert_eq;
use test_r::test;

inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(LastUniqueId);
inherit_test_dep!(Tracing);
inherit_test_dep!(
    #[tagged_as("agent_rpc_rust")]
    PrecompiledComponent
);
inherit_test_dep!(
    #[tagged_as("agent_sdk_rust")]
    PrecompiledComponent
);

async fn start_input(
    executor: &TestWorkerExecutor,
    component: &ComponentDto,
    agent: &OwnedAgentId,
    method: &str,
    extra: Vec<SchemaValue>,
) -> anyhow::Result<(
    mpsc::Sender<InvocationRequest>,
    tonic::Streaming<InvocationResponse>,
    InvocationAccepted,
)> {
    let key = IdempotencyKey::fresh();
    let metadata = executor.get_worker_metadata(agent).await?;
    let mut fields = vec![golem_schema::proto::golem::schema::SchemaValue {
        value: Some(schema_value::Value::StreamReference(
            SchemaValueStreamReference { stream_id: 1 },
        )),
    }];
    for value in extra {
        fields.push(value.try_into().map_err(anyhow::Error::msg)?);
    }
    let start = InvocationRequest {
        request: Some(invocation_request::Request::Start(InvocationStart {
            allow_streams: true,
            target: Some(crate::agent::id_target(agent)),
            method_name: Some(method.into()),
            input: Some(golem_schema::proto::golem::schema::SchemaValue {
                value: Some(schema_value::Value::RecordValue(RecordValue { fields })),
            }),
            request_key: Some(
                InvocationRequestKey::external(executor.auth_ctx().idempotency_scope(), Some(&key))
                    .into(),
            ),
            idempotency_key: Some(key.into()),
            auth_ctx: Some(executor.auth_ctx().into()),
            environment_id: Some(component.environment_id.into()),
            component_owner_account_id: Some(component.account_id.into()),
            mode: golem_api_grpc::proto::golem::worker::AgentInvocationMode::Await as i32,
            freshness_disposition:
                golem_api_grpc::proto::golem::worker::InvocationFreshnessDisposition::MayExist
                    as i32,
            attempt_id: Some(uuid::Uuid::new_v4().into()),
            expected_callee_fingerprint: Some(metadata.fingerprint.0.into()),
            caller: Some(callers::anonymous().into()),
            ..Default::default()
        })),
    };
    let (requests, receiver) = mpsc::channel(16);
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
        .ok_or_else(|| anyhow::anyhow!("no acceptance"))?;
    let Some(invocation_response::Response::Accepted(accepted)) = first.response else {
        anyhow::bail!("expected acceptance, got {first:?}");
    };
    let accepted = await_session_readiness(&mut responses, None, accepted).await?;
    Ok((requests, responses, accepted))
}

async fn send_input(
    requests: &mpsc::Sender<InvocationRequest>,
    accepted: &InvocationAccepted,
    sequence: u64,
    value: u32,
) -> anyhow::Result<()> {
    requests
        .send(InvocationRequest {
            request: Some(invocation_request::Request::InputItem(InputStreamItem {
                transport_stream_id: 1,
                sequence,
                payload: Some(input_stream_item::Payload::Value(
                    SchemaValue::U32(value)
                        .try_into()
                        .map_err(anyhow::Error::msg)?,
                )),
                durable_stream_id: accepted.stream_mappings[0]
                    .handle
                    .as_ref()
                    .unwrap()
                    .stream_id,
                epoch: accepted.epoch,
            })),
        })
        .await?;
    Ok(())
}

async fn read_result(
    responses: &mut tonic::Streaming<InvocationResponse>,
) -> anyhow::Result<SchemaValue> {
    loop {
        let response = responses
            .message()
            .await?
            .ok_or_else(|| anyhow::anyhow!("no result"))?;
        match response.response {
            Some(invocation_response::Response::Result(result)) => {
                let Some(invocation_session_result::Result::MethodResult(value)) = result.result
                else {
                    anyhow::bail!("not a method result");
                };
                return value.try_into().map_err(anyhow::Error::msg);
            }
            Some(invocation_response::Response::Rejected(failure)) => anyhow::bail!("{failure:?}"),
            _ => (),
        }
    }
}

#[test]
#[timeout("2 minutes")]
async fn stream_free_returns_do_not_look_up_sessions_after_stream_history(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_sdk_rust")] fixture: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let reads = KeyValueStorageFaults::default();
    let storage_reads = reads.clone();
    let executor = start_with_overrides(
        deps,
        &context,
        TestExecutorOverrides {
            wrap_key_value_storage: Some(Arc::new(move |inner| {
                Arc::new(FaultInjectingKeyValueStorage::new(
                    inner,
                    storage_reads.clone(),
                ))
            })),
            ..Default::default()
        },
    )
    .await?;
    let component = executor
        .component_dep(&context.default_environment_id, fixture)
        .store()
        .await?;
    let key = agent_key!("RustMethodFailure", "scalar-after-session");
    let agent = executor.start_agent(&component.id, key.clone()).await?;
    let (_requests, mut responses, _) =
        start_input(&executor, &component, &agent, "fail_input", vec![]).await?;
    loop {
        let response = responses
            .message()
            .await?
            .expect("stream session completion");
        if let Some(invocation_response::Response::Finished(finished)) = response.response {
            assert!(matches!(
                finished.outcome,
                Some(invocation_session_completion::Outcome::Failure(_))
            ));
            break;
        }
    }
    executor
        .wait_for_status(&agent, AgentStatus::Idle, Duration::from_secs(30))
        .await?;
    for _ in 0..8 {
        for method in ["fail", "increment"] {
            let mut gate = executor
                .gate_after_next_append(&agent.agent_id, |entry| {
                    matches!(
                        entry,
                        OplogEntry::AgentInvocationFinished { .. }
                            | OplogEntry::AgentInvocationFailed { .. }
                    )
                })
                .await;
            let invocation =
                executor.invoke_and_await_agent(&component, &key, method, data_value!());
            tokio::pin!(invocation);
            tokio::select! {
                _ = gate.entered() => (),
                result = &mut invocation => anyhow::bail!("{method} bypassed terminal gate: {result:?}"),
            }
            let before = reads.calls("lookup");
            drop(gate);
            let result = invocation.await;
            assert_eq!(result.is_err(), method == "fail");
            executor
                .wait_for_status(&agent, AgentStatus::Idle, Duration::from_secs(30))
                .await?;
            assert_eq!(
                reads.calls("lookup"),
                before,
                "{method}: stream-free drain must not look up an absent session"
            );
        }
    }
    assert_eq!(
        executor
            .invoke_and_await_agent(&component, &key, "value", data_value!())
            .await?
            .into_typed::<i64>()?,
        71
    );
    Ok(())
}

#[test]
#[timeout("2 minutes")]
async fn a_scalar_result_with_an_open_input_is_terminal_at_return(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] fixture: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;
    let component = executor
        .component_dep(&context.default_environment_id, fixture)
        .store()
        .await?;
    let agent_key = agent_key!("StreamingRpcTarget", "scalar-input-prefix");
    let agent = executor
        .start_agent(&component.id, agent_key.clone())
        .await?;
    let gate = executor
        .invoke_and_await_agent(&component, &agent_key, "create_output_gate", data_value!())
        .await?
        .into_typed::<PromiseId>()?;
    let (requests, mut responses, accepted) = start_input(
        &executor,
        &component,
        &agent,
        "consume_prefix",
        vec![gate.to_value()],
    )
    .await?;
    for (sequence, value) in [17, 29, 43, 61, 79].into_iter().enumerate() {
        send_input(&requests, &accepted, sequence as u64, value).await?;
    }
    let mut acknowledgements = 0;
    while acknowledgements < 5 {
        let response = responses.message().await?.unwrap();
        assert!(
            matches!(
                response.response,
                Some(invocation_response::Response::InputAck(_))
            ),
            "{response:?}"
        );
        acknowledgements += 1;
    }
    executor
        .complete_promise(&component.environment_id, &gate, vec![])
        .await?;
    // No stream-end is sent. Waiting for the input instead of task.return cannot pass this.
    let value = read_result(&mut responses).await?;
    assert_eq!(
        value,
        SchemaValue::List {
            elements: vec![SchemaValue::U32(17), SchemaValue::U32(29)]
        }
    );
    executor
        .wait_for_status(&agent, AgentStatus::Idle, Duration::from_secs(30))
        .await?;
    let history = executor.get_oplog(&agent, OplogIndex::INITIAL).await?;
    let terminal = history
        .iter()
        .find_map(|entry| match &entry.entry {
            PublicOplogEntry::AgentInvocationFinished(result)
                if result.method_name.as_deref() == Some("consume_prefix") =>
            {
                Some((entry.oplog_index, result.invocation_id))
            }
            _ => None,
        })
        .unwrap();
    let settled = history.iter().find(|entry| matches!(&entry.entry, PublicOplogEntry::AgentInvocationSettled(settled) if settled.invocation_id == terminal.1)).unwrap().oplog_index;
    let cancelled = history
        .iter()
        .filter_map(|entry| match &entry.entry {
            PublicOplogEntry::StreamTerminal(record) => {
                let record = StreamTerminalRecord::from_value(record.record.value()).unwrap();
                matches!(record.terminal, StreamTerminal::ConsumerCancelled { .. })
                    .then_some(entry.oplog_index)
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(cancelled.len(), 1);
    assert!(cancelled[0] < settled);
    let finished = history.iter().find(|entry| matches!(&entry.entry, PublicOplogEntry::StreamSession(record) if matches!(StreamSessionRecord::from_value(record.record.value()), Ok(StreamSessionRecord::Finished(_))))).unwrap().oplog_index;
    assert!(cancelled[0] < finished && finished < settled);
    let raw = executor.stored_oplog(&agent.agent_id).await;
    assert!(matches!(
        &raw[terminal.0.as_u64() as usize - 1],
        OplogEntry::AgentInvocationFinished {
            result: FinishedResult::Payload(_),
            awaits_settlement: true,
            ..
        }
    ));
    assert!(!raw.iter().any(|entry| matches!(entry, OplogEntry::AgentInvocationReturned { invocation_id, .. } if *invocation_id == terminal.1)));
    assert!(!history.iter().any(|entry| matches!(&entry.entry, PublicOplogEntry::StreamSession(record) if matches!(StreamSessionRecord::from_value(record.record.value()), Ok(StreamSessionRecord::RemoteResult(_))))));
    drop(requests);
    drop(responses);
    drop(executor);
    let executor = start(deps, &context).await?;
    assert_eq!(
        executor
            .invoke_and_await_agent(&component, &agent_key, "scalar_value", data_value!())
            .await?
            .into_typed::<u64>()?,
        1
    );
    Ok(())
}

#[test]
#[timeout("2 minutes")]
async fn an_absent_optional_stream_still_has_one_authoritative_early_result(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] fixture: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = crate::fork::start_with_local_resume(deps, &context, false).await?;
    let component = executor
        .component_dep(&context.default_environment_id, fixture)
        .store()
        .await?;
    let key = agent_key!("StreamingRpcTarget", "absent-stream");
    let agent = executor.start_agent(&component.id, key.clone()).await?;
    assert_eq!(
        invoke_agent_session(
            &executor,
            &component,
            &key,
            "optional_output",
            data_value!()
        )
        .await?
        .map_err(anyhow::Error::msg)?,
        SchemaValue::Option { inner: None }
    );
    executor
        .wait_for_status(&agent, AgentStatus::Idle, Duration::from_secs(30))
        .await?;
    let history = executor.stored_oplog(&agent.agent_id).await;
    let returns = history
        .iter()
        .enumerate()
        .filter_map(|(index, entry)| match entry {
            OplogEntry::AgentInvocationReturned {
                invocation_id,
                method_name,
                ..
            } if method_name.as_deref() == Some("optional_output") => {
                Some((OplogIndex::from_u64(index as u64 + 1), *invocation_id))
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(returns.len(), 1);
    assert_eq!(history.iter().filter(|entry| matches!(entry, OplogEntry::AgentInvocationFinished { invocation_id, result: FinishedResult::Returned(index), awaits_settlement: false, .. } if *invocation_id == returns[0].1 && *index == returns[0].0)).count(), 1);
    let fork = crate::fork::fresh_fork_target(&agent);
    crate::fork::fork_worker_to(&executor, &agent, &fork, returns[0].0).await?;
    executor
        .wait_for_status(&fork, AgentStatus::Idle, Duration::from_secs(30))
        .await?;
    let forked = executor.stored_oplog(&fork.agent_id).await;
    assert_eq!(
        forked[returns[0].0.as_u64() as usize - 1],
        history[returns[0].0.as_u64() as usize - 1]
    );
    assert_eq!(forked.iter().filter(|entry| matches!(entry, OplogEntry::AgentInvocationFinished { invocation_id, result: FinishedResult::Returned(index), .. } if *invocation_id == returns[0].1 && *index == returns[0].0)).count(), 1);
    drop(executor);
    let executor = start(deps, &context).await?;
    assert_eq!(
        executor
            .invoke_and_await_agent(&component, &key, "ping", data_value!())
            .await?
            .into_typed::<u64>()?,
        42
    );
    assert_eq!(executor.stored_oplog(&agent.agent_id).await.iter().filter(|entry| matches!(entry, OplogEntry::AgentInvocationReturned { invocation_id, .. } if *invocation_id == returns[0].1)).count(), 1);
    use golem_common::model::agent::{RevertLastInvocations, RevertWorkerTarget};
    executor
        .revert(
            &agent,
            RevertWorkerTarget::RevertLastInvocations(RevertLastInvocations {
                number_of_invocations: 2,
            }),
        )
        .await?;
    invoke_agent_session(
        &executor,
        &component,
        &key,
        "optional_output",
        data_value!(),
    )
    .await?
    .map_err(anyhow::Error::msg)?;
    executor
        .wait_for_status(&agent, AgentStatus::Idle, Duration::from_secs(30))
        .await?;
    let after = executor.stored_oplog(&agent.agent_id).await;
    let (new_index, new_id) = after
        .iter()
        .enumerate()
        .rev()
        .find_map(|(offset, entry)| match entry {
            OplogEntry::AgentInvocationReturned { invocation_id, .. } => {
                Some((OplogIndex::from_u64(offset as u64 + 1), *invocation_id))
            }
            _ => None,
        })
        .unwrap();
    assert_ne!(new_id, returns[0].1);
    assert!(new_index > returns[0].0);
    assert_eq!(after.iter().filter(|entry| matches!(entry, OplogEntry::AgentInvocationFinished { invocation_id, result: FinishedResult::Returned(index), .. } if *invocation_id == new_id && *index == new_index)).count(), 1);
    Ok(())
}

#[test]
#[timeout("2 minutes")]
async fn a_reader_kept_in_agent_state_is_cancelled_at_the_drain_end_not_the_return(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] fixture: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;
    let component = executor
        .component_dep(&context.default_environment_id, fixture)
        .store()
        .await?;
    let key = agent_key!("StreamingRpcTarget", "stored-reader");
    let agent = executor.start_agent(&component.id, key.clone()).await?;
    let gate = executor
        .invoke_and_await_agent(&component, &key, "create_output_gate", data_value!())
        .await?
        .into_typed::<PromiseId>()?;
    let (requests, mut responses, accepted) = start_input(
        &executor,
        &component,
        &agent,
        "retain_input",
        vec![gate.to_value()],
    )
    .await?;
    assert_eq!(read_result(&mut responses).await?, SchemaValue::U64(73));
    // This append must succeed after task.return while unrelated background work holds the drain.
    send_input(&requests, &accepted, 0, 43).await?;
    let response = responses.message().await?.unwrap();
    assert!(
        matches!(
            response.response,
            Some(invocation_response::Response::InputAck(_))
        ),
        "{response:?}"
    );
    let history = executor.get_oplog(&agent, OplogIndex::INITIAL).await?;
    assert!(
        !history
            .iter()
            .any(|entry| matches!(entry.entry, PublicOplogEntry::StreamTerminal(_)))
    );
    executor
        .complete_promise(&component.environment_id, &gate, vec![])
        .await?;
    executor
        .wait_for_status(&agent, AgentStatus::Idle, Duration::from_secs(30))
        .await?;
    let history = executor.get_oplog(&agent, OplogIndex::INITIAL).await?;
    let terminal = history
        .iter()
        .find_map(|entry| match &entry.entry {
            PublicOplogEntry::AgentInvocationFinished(result)
                if result.method_name.as_deref() == Some("retain_input") =>
            {
                Some((entry.oplog_index, result.invocation_id))
            }
            _ => None,
        })
        .unwrap();
    let cancelled = history.iter().find(|entry| matches!(&entry.entry, PublicOplogEntry::StreamTerminal(record) if matches!(StreamTerminalRecord::from_value(record.record.value()).unwrap().terminal, StreamTerminal::ConsumerCancelled { .. }))).unwrap().oplog_index;
    let settled = history.iter().find(|entry| matches!(&entry.entry, PublicOplogEntry::AgentInvocationSettled(record) if record.invocation_id == terminal.1)).unwrap().oplog_index;
    assert!(terminal.0 < cancelled && cancelled < settled);
    drop(requests);
    drop(responses);
    drop(executor);
    let executor = start(deps, &context).await?;
    let restored = executor.get_oplog(&agent, OplogIndex::INITIAL).await?;
    assert!(restored.iter().any(|entry| entry.oplog_index == cancelled && matches!(&entry.entry, PublicOplogEntry::StreamTerminal(record) if matches!(StreamTerminalRecord::from_value(record.record.value()).unwrap().terminal, StreamTerminal::ConsumerCancelled { .. }))));
    Ok(())
}

#[test]
#[timeout("3 minutes")]
async fn an_input_reader_trap_after_return_is_a_background_failure(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] fixture: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use golem_api_grpc::proto::golem::workerexecutor::v1::InvocationLifecycleState;

    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;
    let component = executor
        .component_dep(&context.default_environment_id, fixture)
        .store()
        .await?;
    let key = agent_key!("StreamingRpcTarget", "background-input-trap");
    let agent = executor.start_agent(&component.id, key.clone()).await?;
    let gate = executor
        .invoke_and_await_agent(&component, &key, "create_output_gate", data_value!())
        .await?
        .into_typed::<PromiseId>()?;
    let (requests, mut responses, accepted) = start_input(
        &executor,
        &component,
        &agent,
        "consume_in_background",
        vec![gate.to_value()],
    )
    .await?;
    assert_eq!(read_result(&mut responses).await?, SchemaValue::U64(1));
    requests
        .send(InvocationRequest {
            request: Some(invocation_request::Request::StreamEnd(StreamEnd {
                transport_stream_id: 1,
                producer_sequence: 0,
                direction: StreamDirection::Input as i32,
                authored_by: StreamTerminalAuthor::Guest as i32,
                outcome: Some(
                    StreamTerminal::Failed(StreamError::Trapped(
                        "input failed after return".into(),
                    ))
                    .try_into()
                    .unwrap(),
                ),
                durable_stream_id: accepted.stream_mappings[0]
                    .handle
                    .as_ref()
                    .unwrap()
                    .stream_id,
                epoch: accepted.epoch,
                ..Default::default()
            })),
        })
        .await?;
    executor
        .complete_promise(&component.environment_id, &gate, vec![])
        .await?;
    executor
        .wait_for_status(&agent, AgentStatus::Failed, Duration::from_secs(30))
        .await?;
    let invocation = accepted.idempotency_key.unwrap().into();
    let assert_failure = async |executor: &TestWorkerExecutor| -> anyhow::Result<()> {
        let summary = executor
            .get_invocation(&agent.agent_id, component.environment_id, invocation)
            .await?
            .map_err(|error| anyhow::anyhow!("{error:?}"))?
            .unwrap();
        assert_eq!(
            InvocationLifecycleState::try_from(summary.state).unwrap(),
            InvocationLifecycleState::BackgroundFailed
        );
        let entries = executor.stored_oplog(&agent.agent_id).await;
        assert_eq!(entries.iter().filter(|entry| matches!(entry, OplogEntry::AgentInvocationFinished { invocation_id, result: FinishedResult::Payload(_), awaits_settlement: true, .. } if *invocation_id == invocation)).count(), 1);
        assert!(!entries.iter().any(|entry| matches!(entry, OplogEntry::AgentInvocationFailed { invocation_id, .. } | OplogEntry::AgentInvocationReturned { invocation_id, .. } if *invocation_id == invocation)));
        Ok(())
    };
    assert_failure(&executor).await?;
    drop(requests);
    drop(responses);
    drop(executor);
    let executor = start(deps, &context).await?;
    assert_failure(&executor).await?;
    Ok(())
}

#[test]
#[timeout("2 minutes")]
async fn a_producer_trap_preserves_the_early_value_and_lookup_reports_failure_after_restart(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] fixture: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use golem_api_grpc::proto::golem::workerexecutor::v1::InvocationLifecycleState;
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;
    let component = executor
        .component_dep(&context.default_environment_id, fixture)
        .store()
        .await?;
    let key = agent_key!("StreamingRpcTarget", "producer-trap-after-result");
    let agent = executor.start_agent(&component.id, key.clone()).await?;
    let (_state, requests, mut responses) = open_invocation_session(
        &executor,
        &component,
        &key,
        "produce_outcome",
        data_value!(2u8),
    )
    .await?;
    loop {
        let response = responses.message().await?.unwrap();
        if let Some(invocation_response::Response::Result(result)) = response.response {
            let Some(invocation_session_result::Result::MethodResult(value)) = result.result else {
                anyhow::bail!("{result:?}");
            };
            assert!(matches!(
                value.value,
                Some(schema_value::Value::StreamReference(_))
            ));
            break;
        }
    }
    executor
        .wait_for_status(&agent, AgentStatus::Failed, Duration::from_secs(30))
        .await?;
    let before = executor.stored_oplog(&agent.agent_id).await;
    let returns = before
        .iter()
        .filter_map(|entry| match entry {
            OplogEntry::AgentInvocationReturned { invocation_id, .. } => {
                Some((*invocation_id, entry.clone()))
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(returns.len(), 1);
    let invocation = returns[0].0;
    let validate = async |executor: &TestWorkerExecutor| -> anyhow::Result<()> {
        let raw = executor.stored_oplog(&agent.agent_id).await;
        assert_eq!(raw.iter().filter(|entry| matches!(entry, OplogEntry::AgentInvocationReturned { invocation_id, .. } if *invocation_id == invocation)).cloned().collect::<Vec<_>>(), vec![returns[0].1.clone()]);
        assert!(!raw.iter().any(|entry| matches!(entry, OplogEntry::AgentInvocationFinished { invocation_id, .. } if *invocation_id == invocation)));
        let summary = executor
            .get_invocation(&agent.agent_id, component.environment_id, invocation)
            .await?
            .map_err(|error| anyhow::anyhow!("{error:?}"))?
            .unwrap();
        assert_eq!(
            InvocationLifecycleState::try_from(summary.state).unwrap(),
            InvocationLifecycleState::Failed
        );
        Ok(())
    };
    validate(&executor).await?;
    drop(responses);
    drop(requests);
    executor.shutdown_and_wait_for_invocation_loops().await?;
    drop(executor);
    let executor = start(deps, &context).await?;
    validate(&executor).await?;
    Ok(())
}

#[test]
#[timeout("3 minutes")]
async fn background_input_reading_replays_after_the_scalar_terminal_and_before_settlement(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] fixture: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;
    let component = executor
        .component_dep(&context.default_environment_id, fixture)
        .store()
        .await?;
    let key = agent_key!("StreamingRpcTarget", "background-reader");
    let agent = executor.start_agent(&component.id, key.clone()).await?;
    let fingerprint = executor.get_worker_metadata(&agent).await?.fingerprint;
    let gate = executor
        .invoke_and_await_agent(&component, &key, "create_output_gate", data_value!())
        .await?
        .into_typed::<PromiseId>()?;
    let (requests, mut responses, accepted) = start_input(
        &executor,
        &component,
        &agent,
        "consume_in_background",
        vec![gate.to_value()],
    )
    .await?;
    assert_eq!(read_result(&mut responses).await?, SchemaValue::U64(1));
    let invocation = accepted.idempotency_key.unwrap().into();
    assert!(!executor.cancel_invocation(&agent, &invocation).await?);
    send_input(&requests, &accepted, 0, 17).await?;
    send_input(&requests, &accepted, 1, 29).await?;
    executor
        .complete_promise(&component.environment_id, &gate, vec![])
        .await?;
    let before = tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            let history = executor.get_oplog(&agent, OplogIndex::INITIAL).await?;
            if input_consumer_history(&history)?
                .iter()
                .filter(|record| matches!(record, StreamSessionRecord::ConsumerItemValue(_)))
                .count()
                == 2
            {
                return Ok::<_, anyhow::Error>(history);
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await??;
    let terminal = before
        .iter()
        .find_map(|entry| match &entry.entry {
            PublicOplogEntry::AgentInvocationFinished(result)
                if result.method_name.as_deref() == Some("consume_in_background") =>
            {
                Some((entry.oplog_index, result.invocation_id))
            }
            _ => None,
        })
        .unwrap();
    assert!(!before.iter().any(|entry| matches!(&entry.entry, PublicOplogEntry::AgentInvocationSettled(record) if record.invocation_id == terminal.1)));
    assert!(before.iter().filter(|entry| matches!(&entry.entry, PublicOplogEntry::StreamSession(record) if matches!(StreamSessionRecord::from_value(record.record.value()), Ok(StreamSessionRecord::ConsumerItemValue(_))))).all(|entry| entry.oplog_index > terminal.0));
    executor.shutdown_and_wait_for_invocation_loops().await?;
    drop(requests);
    drop(responses);
    drop(executor);
    let executor = start(deps, &context).await?;
    let resume = InvocationRequest {
        request: Some(invocation_request::Request::ResumeAttach(ResumeAttach {
            idempotency_key: accepted.idempotency_key,
            agent_id: Some(agent.agent_id.into()),
            environment_id: Some(component.environment_id.into()),
            attachment_id: accepted.attachment_id,
            attempt_id: Some(uuid::Uuid::new_v4().into()),
            expected_callee_fingerprint: Some(fingerprint.0.into()),
            expected_epoch: accepted.epoch,
            operation: ResumeOperation::Takeover as i32,
            cursors: vec![],
            auth_ctx: Some(executor.auth_ctx().into()),
            caller: Some(callers::anonymous().into()),
        })),
    };
    let (requests, receiver) = mpsc::channel(16);
    requests.send(resume).await?;
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
    let accepted = await_session_readiness(&mut responses, None, accepted).await?;
    assert_eq!(read_result(&mut responses).await?, SchemaValue::U64(1));
    assert!(!executor.cancel_invocation(&agent, &invocation).await?);
    let next = tokio::spawn({
        let executor = executor.clone();
        let component = component.clone();
        let key = key.clone();
        async move {
            executor
                .invoke_and_await_agent(&component, &key, "observed_input", data_value!())
                .await
        }
    });
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            let history = executor.get_oplog(&agent, OplogIndex::INITIAL).await?;
            if history.iter().any(|entry| matches!(&entry.entry,
                PublicOplogEntry::PendingAgentInvocation(pending)
                    if matches!(&pending.invocation, PublicAgentInvocation::AgentMethodInvocation(method) if method.method_name == "observed_input")
            )) {
                assert!(!history.iter().any(|entry| matches!(&entry.entry,
                    PublicOplogEntry::AgentInvocationStarted(started)
                        if matches!(&started.invocation, PublicAgentInvocation::AgentMethodInvocation(method) if method.method_name == "observed_input")
                )), "the serial agent must not start queued work while the input reader drains");
                assert!(!next.is_finished());
                return Ok::<_, anyhow::Error>(());
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }).await??;
    send_input(&requests, &accepted, 2, 43).await?;
    requests
        .send(InvocationRequest {
            request: Some(invocation_request::Request::StreamEnd(StreamEnd {
                transport_stream_id: 1,
                producer_sequence: 3,
                direction: StreamDirection::Input as i32,
                authored_by: StreamTerminalAuthor::Guest as i32,
                outcome: Some(StreamTerminal::Finished.try_into().unwrap()),
                durable_stream_id: accepted.stream_mappings[0]
                    .handle
                    .as_ref()
                    .unwrap()
                    .stream_id,
                epoch: accepted.epoch,
                ..Default::default()
            })),
        })
        .await?;
    // A serial agent must finish the background reader before it can answer this next call.
    assert_eq!(next.await??.into_typed::<Vec<u32>>()?, vec![17, 29, 43]);
    assert_eq!(
        executor
            .invoke_and_await_agent(&component, &key, "scalar_value", data_value!())
            .await?
            .into_typed::<u64>()?,
        1
    );
    let history = executor.get_oplog(&agent, OplogIndex::INITIAL).await?;
    assert_eq!(history.iter().filter(|entry| matches!(&entry.entry, PublicOplogEntry::AgentInvocationFinished(record) if record.invocation_id == terminal.1)).count(), 1);
    let settled = history.iter().find(|entry| matches!(&entry.entry, PublicOplogEntry::AgentInvocationSettled(record) if record.invocation_id == terminal.1)).unwrap().oplog_index;
    assert!(history.iter().filter(|entry| matches!(&entry.entry, PublicOplogEntry::StreamSession(record) if matches!(StreamSessionRecord::from_value(record.record.value()), Ok(StreamSessionRecord::ConsumerItemValue(_))))).all(|entry| terminal.0 < entry.oplog_index && entry.oplog_index < settled));
    let input_end = history
        .iter()
        .find_map(|entry| match &entry.entry {
            PublicOplogEntry::StreamTerminal(record)
                if matches!(
                    StreamTerminalRecord::from_value(record.record.value())
                        .unwrap()
                        .terminal,
                    StreamTerminal::Finished
                ) =>
            {
                Some(entry.oplog_index)
            }
            _ => None,
        })
        .unwrap();
    let session_finished = history.iter().find_map(|entry| match &entry.entry {
        PublicOplogEntry::StreamSession(record) if matches!(StreamSessionRecord::from_value(record.record.value()), Ok(StreamSessionRecord::Finished(record)) if record.session_key == golem_common::model::durable_stream::StreamRegistrationInvocation::Local(invocation) && record.result.is_ok()) => Some(entry.oplog_index), _ => None,
    }).unwrap();
    assert!(input_end < session_finished && session_finished < settled);
    let raw = executor.stored_oplog(&agent.agent_id).await;
    assert_eq!(raw.iter().filter(|entry| matches!(entry, OplogEntry::AgentInvocationFinished { invocation_id, result: FinishedResult::Payload(_), awaits_settlement: true, .. } if *invocation_id == invocation)).count(), 1);
    assert!(!raw.iter().any(|entry| matches!(entry, OplogEntry::AgentInvocationFailed { invocation_id, .. } | OplogEntry::AgentInvocationReturned { invocation_id, .. } if *invocation_id == invocation)));
    Ok(())
}

#[test]
#[timeout("3 minutes")]
async fn a_scalar_result_does_not_finish_its_session_while_a_forwarded_input_is_open(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] fixture: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;
    let component = executor
        .component_dep(&context.default_environment_id, fixture)
        .store()
        .await?;
    let key = agent_key!("StreamingRpcTarget", "forwarded-background-input");
    let agent = executor.start_agent(&component.id, key.clone()).await?;
    let gate = executor
        .invoke_and_await_agent(&component, &key, "create_output_gate", data_value!())
        .await?
        .into_typed::<PromiseId>()?;
    let (requests, mut responses, accepted) = start_input(
        &executor,
        &component,
        &agent,
        "forward_in_background",
        vec![gate.to_value()],
    )
    .await?;
    assert_eq!(read_result(&mut responses).await?, SchemaValue::U64(1));
    send_input(&requests, &accepted, 0, 17).await?;
    send_input(&requests, &accepted, 1, 43).await?;
    executor
        .complete_promise(&component.environment_id, &gate, vec![])
        .await?;
    let history = tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            let history = executor.get_oplog(&agent, OplogIndex::INITIAL).await?;
            if history.iter().any(|entry| matches!(&entry.entry,
                PublicOplogEntry::StreamSession(record) if matches!(StreamSessionRecord::from_value(record.record.value()), Ok(StreamSessionRecord::ReaderForwardIntent(_)))))
            {
                return Ok::<_, anyhow::Error>(history);
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await??;
    let invocation: golem_common::model::InvocationId = accepted.idempotency_key.unwrap().into();
    assert!(history.iter().any(|entry| matches!(&entry.entry, PublicOplogEntry::AgentInvocationFinished(record) if record.invocation_id == invocation)));
    assert!(!history.iter().any(|entry| matches!(&entry.entry, PublicOplogEntry::AgentInvocationSettled(record) if record.invocation_id == invocation)));
    assert!(!history.iter().any(|entry| matches!(&entry.entry, PublicOplogEntry::StreamSession(record) if matches!(StreamSessionRecord::from_value(record.record.value()), Ok(StreamSessionRecord::Finished(record)) if record.session_key == golem_common::model::durable_stream::StreamRegistrationInvocation::Local(invocation)))));
    requests
        .send(InvocationRequest {
            request: Some(invocation_request::Request::StreamEnd(StreamEnd {
                transport_stream_id: 1,
                producer_sequence: 2,
                direction: StreamDirection::Input as i32,
                authored_by: StreamTerminalAuthor::Guest as i32,
                outcome: Some(StreamTerminal::Finished.try_into().unwrap()),
                durable_stream_id: accepted.stream_mappings[0]
                    .handle
                    .as_ref()
                    .unwrap()
                    .stream_id,
                epoch: accepted.epoch,
                ..Default::default()
            })),
        })
        .await?;
    assert_eq!(
        executor
            .invoke_and_await_agent(&component, &key, "observed_input", data_value!())
            .await?
            .into_typed::<Vec<u32>>()?,
        vec![17, 43]
    );
    let history = executor.get_oplog(&agent, OplogIndex::INITIAL).await?;
    assert!(history.iter().any(|entry| matches!(&entry.entry, PublicOplogEntry::StreamSession(record) if matches!(StreamSessionRecord::from_value(record.record.value()), Ok(StreamSessionRecord::Finished(record)) if record.session_key == golem_common::model::durable_stream::StreamRegistrationInvocation::Local(invocation) && record.result.is_ok()))));
    Ok(())
}

#[test]
#[timeout("3 minutes")]
async fn session_delete_preserves_the_early_result_and_its_terminal_after_restart(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_sdk_rust")] fixture: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use golem_api_grpc::proto::golem::workerexecutor::v1::InvocationLifecycleState;
    use golem_api_grpc::proto::golem::workerexecutor::v1::{
        CreateStreamSessionRequest, DurableStreamAttachmentControlRequest, ExportStreamControl,
        ReadStreamSlotRequest, StreamSessionCreationIntent, StreamSessionExpiryPolicy,
        StreamSlotReadAdmission, create_stream_session_response,
        durable_stream_attachment_control_response, read_stream_slot_response,
        stream_session_expiry_policy,
    };
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;
    let component = executor
        .component_dep(&context.default_environment_id, fixture)
        .store()
        .await?;
    let key = agent_key!("DurableStreamAgent", "delete-after-early-result");
    let agent = executor.start_agent(&component.id, key).await?;
    let session = uuid::Uuid::new_v4().to_string();
    let route = "[\"environment\",\"1\",\"route\"]";
    let created = executor
        .client
        .clone()
        .create_stream_session(CreateStreamSessionRequest {
            public_session_id: session.clone(),
            route_scope: route.into(),
            expiry_policy: Some(StreamSessionExpiryPolicy {
                kind: Some(stream_session_expiry_policy::Kind::None(Default::default())),
            }),
            creation_intent: StreamSessionCreationIntent::ExplicitPut as i32,
            invocation: Some(InvocationStart {
                target: Some(crate::agent::id_target(&agent)),
                environment_id: Some(component.environment_id.into()),
                auth_ctx: Some(AuthCtx::System.into()),
                component_owner_account_id: Some(component.account_id.into()),
                idempotency_key: Some(IdempotencyKey::new(session.clone()).into()),
                method_name: Some("cancellation_output".into()),
                input: Some(
                    data_value!(5u32, 5000u64)
                        .value()
                        .clone()
                        .try_into()
                        .map_err(anyhow::Error::msg)?,
                ),
                caller: Some(callers::anonymous().into()),
                ..Default::default()
            }),
        })
        .await?
        .into_inner();
    let Some(create_stream_session_response::Result::Success(created)) = created.result else {
        anyhow::bail!("{created:?}")
    };
    let invocation = golem_common::model::InvocationId::from(created.invocation_key.unwrap());
    let read = ReadStreamSlotRequest {
        agent_id: Some(agent.agent_id.into()),
        environment_id: Some(component.environment_id.into()),
        auth_ctx: Some(AuthCtx::System.into()),
        session: session.clone(),
        slot: "$result".into(),
        max_items: 10,
        max_bytes: 1024,
        wait_millis: 1000,
        expected_method: "cancellation_output".into(),
        admission: StreamSlotReadAdmission::TouchingOriginGet as i32,
        route_scope: route.into(),
        ..Default::default()
    };
    let first = executor
        .client
        .clone()
        .read_stream_slot(read.clone())
        .await?
        .into_inner()
        .message()
        .await?
        .unwrap();
    let Some(read_stream_slot_response::Result::Success(first)) = first.result else {
        anyhow::bail!("{first:?}")
    };
    assert_eq!(first.items.len(), 1);
    assert!(!first.closed);
    let before = executor.stored_oplog(&agent.agent_id).await;
    let returned = before.iter().enumerate().filter_map(|(offset, entry)| matches!(entry, OplogEntry::AgentInvocationReturned { invocation_id, .. } if *invocation_id == invocation).then_some(OplogIndex::from_u64(offset as u64 + 1))).collect::<Vec<_>>();
    assert_eq!(returned.len(), 1);
    assert!(!before.iter().any(|entry| matches!(entry, OplogEntry::AgentInvocationFinished { invocation_id, .. } if *invocation_id == invocation)));
    let deleted = executor
        .client
        .clone()
        .control_durable_stream_attachment(DurableStreamAttachmentControlRequest {
            producer_agent_id: Some(agent.agent_id.into()),
            producer_environment_id: Some(component.environment_id.into()),
            auth_ctx: Some(AuthCtx::System.into()),
            export_control: Some(ExportStreamControl {
                session,
                slot: None,
                expected_method: "cancellation_output".into(),
                route_scope: route.into(),
            }),
            ..Default::default()
        })
        .await?
        .into_inner();
    assert!(
        matches!(
            deleted.result,
            Some(durable_stream_attachment_control_response::Result::ExportResult(_))
        ),
        "{deleted:?}"
    );
    executor
        .wait_for_status(&agent, AgentStatus::Idle, Duration::from_secs(30))
        .await?;
    let validate = async |executor: &TestWorkerExecutor| -> anyhow::Result<()> {
        let raw = executor.stored_oplog(&agent.agent_id).await;
        assert_eq!(raw.iter().filter(|entry| matches!(entry, OplogEntry::AgentInvocationReturned { invocation_id, .. } if *invocation_id == invocation)).count(), 1);
        assert_eq!(
            raw[returned[0].as_u64() as usize - 1],
            before[returned[0].as_u64() as usize - 1]
        );
        assert_eq!(raw.iter().filter(|entry| matches!(entry, OplogEntry::AgentInvocationFinished { invocation_id, result: FinishedResult::Returned(index), .. } if *invocation_id == invocation && *index == returned[0])).count(), 1);
        let summary = executor
            .get_invocation(&agent.agent_id, component.environment_id, invocation)
            .await?
            .map_err(|error| anyhow::anyhow!("{error:?}"))?
            .unwrap();
        assert_eq!(
            InvocationLifecycleState::try_from(summary.state).unwrap(),
            InvocationLifecycleState::Completed
        );
        let history = executor.get_oplog(&agent, OplogIndex::INITIAL).await?;
        let terminals = history
            .iter()
            .filter_map(|entry| match &entry.entry {
                PublicOplogEntry::StreamTerminal(record) => {
                    Some(StreamTerminalRecord::from_value(record.record.value()).unwrap())
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(terminals.len(), 1);
        assert_eq!(
            terminals[0].terminal,
            StreamTerminal::Failed(StreamError::Interrupted("session deleted".into()))
        );
        let resumed = executor
            .client
            .clone()
            .read_stream_slot(read.clone())
            .await?
            .into_inner()
            .message()
            .await?
            .unwrap();
        let Some(read_stream_slot_response::Result::Success(resumed)) = resumed.result else {
            anyhow::bail!("{resumed:?}")
        };
        assert!(resumed.closed && resumed.cancelled);
        assert_eq!(resumed.items, first.items);
        Ok(())
    };
    validate(&executor).await?;
    executor.shutdown_and_wait_for_invocation_loops().await?;
    drop(executor);
    let executor = start(deps, &context).await?;
    validate(&executor).await?;
    Ok(())
}

#[test]
#[timeout("3 minutes")]
async fn a_retained_input_slot_rejects_appends_after_drain_or_existing_slot_delete(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] fixture: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use golem_api_grpc::proto::golem::workerexecutor::v1::{
        AppendToStreamSlotRequest, CreateStreamSessionRequest,
        DurableStreamAttachmentControlRequest, ExportStreamControl, ExternalStreamProducer,
        ReadStreamSlotRequest, StreamSessionCreationIntent, StreamSessionExpiryPolicy,
        StreamSlotReadAdmission, TypedStreamSlotItems, append_to_stream_slot_request,
        append_to_stream_slot_response, create_stream_session_response,
        durable_stream_attachment_control_response, read_stream_slot_response,
        stream_session_expiry_policy,
    };
    use prost::Message;
    let context = TestContext::new(last_unique_id);
    let mut executor = start(deps, &context).await?;
    let component = executor
        .component_dep(&context.default_environment_id, fixture)
        .store()
        .await?;
    for (delete_slot, restart) in [(false, false), (false, true), (true, false), (true, true)] {
        let key = agent_key!(
            "StreamingRpcTarget",
            format!("retained-slot-{delete_slot}-{restart}")
        );
        let agent = executor.start_agent(&component.id, key.clone()).await?;
        let gate = executor
            .invoke_and_await_agent(&component, &key, "create_output_gate", data_value!())
            .await?
            .into_typed::<PromiseId>()?;
        let session = uuid::Uuid::new_v4().to_string();
        let route = "[\"environment\",\"1\",\"route\"]";
        let gate_value: golem_schema::proto::golem::schema::SchemaValue =
            gate.to_value().try_into().map_err(anyhow::Error::msg)?;
        let input = golem_schema::proto::golem::schema::SchemaValue {
            value: Some(schema_value::Value::RecordValue(RecordValue {
                fields: vec![
                    golem_schema::proto::golem::schema::SchemaValue {
                        value: Some(schema_value::Value::StreamReference(
                            SchemaValueStreamReference { stream_id: 1 },
                        )),
                    },
                    gate_value,
                ],
            })),
        };
        let created = executor
            .client
            .clone()
            .create_stream_session(CreateStreamSessionRequest {
                public_session_id: session.clone(),
                route_scope: route.into(),
                expiry_policy: Some(StreamSessionExpiryPolicy {
                    kind: Some(stream_session_expiry_policy::Kind::None(Default::default())),
                }),
                creation_intent: StreamSessionCreationIntent::ExplicitPut as i32,
                invocation: Some(InvocationStart {
                    target: Some(crate::agent::id_target(&agent)),
                    environment_id: Some(component.environment_id.into()),
                    auth_ctx: Some(AuthCtx::System.into()),
                    component_owner_account_id: Some(component.account_id.into()),
                    idempotency_key: Some(IdempotencyKey::new(session.clone()).into()),
                    method_name: Some("retain_input".into()),
                    input: Some(input),
                    caller: Some(callers::anonymous().into()),
                    ..Default::default()
                }),
            })
            .await?
            .into_inner();
        let Some(create_stream_session_response::Result::Success(created)) = created.result else {
            anyhow::bail!("{created:?}")
        };
        let invocation = created.invocation_key.unwrap().into();
        let result = executor
            .client
            .clone()
            .read_stream_slot(ReadStreamSlotRequest {
                agent_id: Some(agent.agent_id.into()),
                environment_id: Some(component.environment_id.into()),
                auth_ctx: Some(AuthCtx::System.into()),
                session: session.clone(),
                slot: "$result".into(),
                max_items: 1,
                max_bytes: 1024,
                wait_millis: 30_000,
                expected_method: "retain_input".into(),
                admission: StreamSlotReadAdmission::TouchingOriginGet as i32,
                route_scope: route.into(),
                ..Default::default()
            })
            .await?
            .into_inner()
            .message()
            .await?
            .unwrap();
        let Some(read_stream_slot_response::Result::Success(result)) = result.result else {
            anyhow::bail!("{result:?}")
        };
        assert_eq!(result.items.len(), 1);
        let mut append = AppendToStreamSlotRequest {
            agent_id: Some(agent.agent_id.into()),
            environment_id: Some(component.environment_id.into()),
            auth_ctx: Some(AuthCtx::System.into()),
            session: session.clone(),
            slot: "input".into(),
            payload: Some(append_to_stream_slot_request::Payload::Values(
                TypedStreamSlotItems {
                    values: vec![
                        golem_schema::proto::golem::schema::SchemaValue::try_from(
                            SchemaValue::U32(43),
                        )
                        .unwrap()
                        .encode_to_vec(),
                    ],
                },
            )),
            producer: Some(ExternalStreamProducer {
                id: "retained-producer".into(),
                epoch: 0,
                sequence: 0,
            }),
            expected_method: "retain_input".into(),
            route_scope: route.into(),
            ..Default::default()
        };
        let accepted = executor
            .client
            .clone()
            .append_to_stream_slot(append.clone())
            .await?
            .into_inner();
        assert!(
            matches!(
                accepted.result,
                Some(append_to_stream_slot_response::Result::Accepted(_))
            ),
            "{accepted:?}"
        );
        if delete_slot {
            let deleted = executor
                .client
                .clone()
                .control_durable_stream_attachment(DurableStreamAttachmentControlRequest {
                    producer_agent_id: Some(agent.agent_id.into()),
                    producer_environment_id: Some(component.environment_id.into()),
                    auth_ctx: Some(AuthCtx::System.into()),
                    export_control: Some(ExportStreamControl {
                        session: session.clone(),
                        slot: Some("input".into()),
                        expected_method: "retain_input".into(),
                        route_scope: route.into(),
                    }),
                    ..Default::default()
                })
                .await?
                .into_inner();
            assert!(
                matches!(
                    deleted.result,
                    Some(durable_stream_attachment_control_response::Result::ExportResult(_))
                ),
                "{deleted:?}"
            );
        }
        // The gate holds the drain independently of the retained reader.
        let before_drain = executor.stored_oplog(&agent.agent_id).await;
        assert!(!before_drain.iter().any(|entry| matches!(entry, OplogEntry::AgentInvocationSettled { invocation_id, .. } if *invocation_id == invocation)));
        executor
            .complete_promise(&component.environment_id, &gate, vec![])
            .await?;
        executor
            .wait_for_status(&agent, AgentStatus::Idle, Duration::from_secs(30))
            .await?;
        append.producer.as_mut().unwrap().sequence = 1;
        let refused = executor
            .client
            .clone()
            .append_to_stream_slot(append)
            .await?
            .into_inner();
        assert!(
            matches!(
                refused.result,
                Some(
                    append_to_stream_slot_response::Result::Closed(_)
                        | append_to_stream_slot_response::Result::Gone(_)
                )
            ),
            "{refused:?}"
        );
        let raw = executor.stored_oplog(&agent.agent_id).await;
        assert_eq!(raw.iter().filter(|entry| matches!(entry, OplogEntry::AgentInvocationFinished { invocation_id, result: FinishedResult::Payload(_), .. } if *invocation_id == invocation)).count(), 1);
        assert!(!raw.iter().any(|entry| matches!(entry, OplogEntry::AgentInvocationFailed { invocation_id, .. } | OplogEntry::AgentInvocationReturned { invocation_id, .. } if *invocation_id == invocation)));
        if restart {
            executor.shutdown_and_wait_for_invocation_loops().await?;
            drop(executor);
            executor = start(deps, &context).await?;
        }
        let result = executor
            .invoke_and_await_agent(&component, &key, "read_retained_input", data_value!())
            .await?
            .into_typed::<Result<Option<u32>, String>>()?;
        assert_eq!(result, Ok(Some(43)));
    }
    Ok(())
}
