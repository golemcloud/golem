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

//! A streaming session over an oplog that does not read gives no session outcome: a Start and a
//! Resume of the agent write no terminal, no cancellation and no detach. A queued Start whose
//! transport closes before readiness still ends its hold with a detach.

use super::*;
use crate::unreadable_call_paths::{StoredOplogRows, decoded_after, outcome_entries};
use futures::{StreamExt, TryStreamExt};
use golem_common::model::oplog::{OplogEntry, OplogPayload};
use golem_worker_executor::services::golem_config::OplogConfig;
use pretty_assertions::assert_eq;
use test_r::{test, timeout};

/// An executor that keeps every entry in the primary oplog.
async fn start_primary_only(
    deps: &WorkerExecutorTestDependencies,
    context: &TestContext,
) -> anyhow::Result<TestWorkerExecutor> {
    let oplog = OplogConfig {
        indexed_storage_layers: 1,
        blob_storage_layers: 0,
        ..Default::default()
    };
    start_with_overrides(
        deps,
        context,
        TestExecutorOverrides {
            configure: Some(Arc::new(move |runtime| {
                runtime.oplog = oplog.clone();
                runtime.suspend.suspend_after = Duration::from_secs(300);
            })),
            ..Default::default()
        },
    )
    .await
}

/// A Start of `drop_input` with one input stream, which the guest drops.
fn drop_input_start(
    executor: &TestWorkerExecutor,
    component: &ComponentDto,
    target: &OwnedAgentId,
    fingerprint: uuid::Uuid,
    key: &IdempotencyKey,
) -> InvocationStart {
    InvocationStart {
        allow_streams: true,
        target: Some(crate::agent::id_target(target)),
        method_name: Some("drop_input".to_string()),
        input: Some(golem_schema::proto::golem::schema::SchemaValue {
            value: Some(schema_value::Value::RecordValue(RecordValue {
                fields: vec![golem_schema::proto::golem::schema::SchemaValue {
                    value: Some(schema_value::Value::StreamReference(
                        SchemaValueStreamReference { stream_id: 1 },
                    )),
                }],
            })),
        }),
        request_key: Some(
            InvocationRequestKey::external(executor.auth_ctx().idempotency_scope(), Some(key))
                .into(),
        ),
        idempotency_key: Some(key.clone().into()),
        auth_ctx: Some(executor.auth_ctx().into()),
        environment_id: Some(component.environment_id.into()),
        component_owner_account_id: Some(component.account_id.into()),
        mode: golem_api_grpc::proto::golem::worker::AgentInvocationMode::Await as i32,
        freshness_disposition:
            golem_api_grpc::proto::golem::worker::InvocationFreshnessDisposition::MayExist as i32,
        attempt_id: Some(uuid::Uuid::new_v4().into()),
        expected_callee_fingerprint: Some(fingerprint.into()),
        caller: Some(callers::anonymous().into()),
        ..Default::default()
    }
}

/// Sends `request` on a new session and gives every response until the executor ends the
/// session. A status error ends the session too, and is the last element.
async fn session_responses(
    executor: &TestWorkerExecutor,
    request: InvocationRequest,
) -> anyhow::Result<Vec<Result<InvocationResponse, tonic::Status>>> {
    let (requests, receiver) = mpsc::channel(8);
    requests.send(request).await?;
    let responses = match executor
        .client
        .clone()
        .invoke_agent_session(ReceiverStream::new(receiver))
        .await
    {
        Ok(responses) => responses.into_inner(),
        Err(status) => return Ok(vec![Err(status)]),
    };
    let responses = tokio::time::timeout(Duration::from_secs(60), responses.collect::<Vec<_>>())
        .await
        .map_err(|_| anyhow::anyhow!("the session over an unreadable oplog did not end"))?;
    drop(requests);
    Ok(responses)
}

/// Whether `responses` hold a result or a successful completion.
fn has_an_outcome(responses: &[Result<InvocationResponse, tonic::Status>]) -> bool {
    responses.iter().any(|response| match response {
        Ok(InvocationResponse {
            response: Some(invocation_response::Response::Result(_)),
        }) => true,
        Ok(InvocationResponse {
            response: Some(invocation_response::Response::Finished(finished)),
        }) => matches!(
            finished.outcome,
            Some(invocation_session_completion::Outcome::Success(_))
        ),
        _ => false,
    })
}

/// The stream-session records of `entries` that end, cancel or settle a session.
fn settling_session_records(entries: &[(u64, OplogEntry)]) -> Vec<(u64, StreamSessionRecord)> {
    entries
        .iter()
        .filter_map(|(index, entry)| match entry {
            OplogEntry::StreamSession { record, .. } => Some((*index, record)),
            _ => None,
        })
        .map(|(index, record)| {
            let OplogPayload::SerializedInline { bytes, .. } = record else {
                panic!("the session record at {index} is stored inline: {record:?}");
            };
            let record = golem_common::serialization::decode::<StreamSessionRecord>(bytes)
                .unwrap_or_else(|error| panic!("the session record at {index} decodes: {error}"));
            (index, record)
        })
        .filter(|(_, record)| {
            matches!(
                record,
                StreamSessionRecord::Detached(_)
                    | StreamSessionRecord::Finished(_)
                    | StreamSessionRecord::CancelRequested(_)
                    | StreamSessionRecord::InvocationResult(_)
                    | StreamSessionRecord::ConsumerTerminal(_)
                    | StreamSessionRecord::ConsumerOutcome(_)
                    | StreamSessionRecord::ConsumerCancelIntent(_)
                    | StreamSessionRecord::ConsumerCancelApplied(_)
                    | StreamSessionRecord::AttachmentFinalized(_)
            )
        })
        .collect()
}

/// Whether the public oplog of `agent_id` holds the detach of the attachment at `epoch`.
async fn detached_at(
    executor: &TestWorkerExecutor,
    agent_id: &OwnedAgentId,
    epoch: u64,
) -> anyhow::Result<bool> {
    let oplog = executor.get_oplog(agent_id, OplogIndex::INITIAL).await?;
    oplog
        .into_iter()
        .filter_map(|entry| match entry.entry {
            PublicOplogEntry::StreamSession(session) => Some(session),
            _ => None,
        })
        .map(|session| {
            StreamSessionRecord::from_value(session.record.value()).map_err(anyhow::Error::msg)
        })
        .try_fold(false, |found, record| {
            Ok(found || matches!(record?, StreamSessionRecord::Detached(record) if record.epoch == epoch))
        })
}

/// Waits until the oplog of `agent_id` holds the detach of the attachment at `epoch`.
async fn wait_for_detach(
    executor: &TestWorkerExecutor,
    agent_id: &OwnedAgentId,
    epoch: u64,
) -> anyhow::Result<()> {
    let polls = tokio_stream::wrappers::IntervalStream::new(tokio::time::interval(
        Duration::from_millis(50),
    ));
    let mut detached = polls
        .then(|_| detached_at(executor, agent_id, epoch))
        .try_filter(|detached| futures::future::ready(*detached))
        .boxed();
    tokio::time::timeout(Duration::from_secs(30), detached.try_next())
        .await
        .map_err(|_| anyhow::anyhow!("the attachment at epoch {epoch} did not detach"))??;
    Ok(())
}

/// A session of the target completes and detaches, then an entry of its oplog before the head
/// stops to decode. A new Start and a Resume of the completed session both fail without a
/// result, and the oplog of the target gets no terminal, cancellation or detach because of them.
#[test]
#[timeout("4m")]
#[tracing::instrument]
async fn a_start_and_a_resume_over_an_unreadable_oplog_write_no_terminal_cancel_or_detach(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] agent_rpc_rust: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start_primary_only(deps, &context).await?;
    let component = executor
        .component_dep(&context.default_environment_id, agent_rpc_rust)
        .store()
        .await?;
    let target = executor
        .start_agent(
            &component.id,
            agent_key!("StreamingRpcTarget", "unreadable-session"),
        )
        .await?;
    let fingerprint = executor.get_worker_metadata(&target).await?.fingerprint.0;
    let first_key = IdempotencyKey::fresh();
    let first = drop_input_start(&executor, &component, &target, fingerprint, &first_key);
    let (requests, mut responses, accepted) = open_raw_session(&executor, first.clone()).await?;
    let completed = tokio::time::timeout(Duration::from_secs(60), async {
        (&mut responses)
            .try_filter(|response| {
                futures::future::ready(matches!(
                    response.response,
                    Some(invocation_response::Response::Finished(_))
                ))
            })
            .boxed()
            .try_next()
            .await
    })
    .await??
    .expect("the first session completes");
    assert!(matches!(
        completed.response,
        Some(invocation_response::Response::Finished(ref finished))
            if matches!(finished.outcome, Some(invocation_session_completion::Outcome::Success(_)))
    ));
    drop(requests);
    drop(responses);
    wait_for_detach(&executor, &target, accepted.epoch).await?;
    executor
        .wait_for_status(&target, AgentStatus::Idle, Duration::from_secs(30))
        .await?;
    executor.release().await?;

    let stored = StoredOplogRows::open(deps, &context, &target).await?;
    let head = stored
        .rows()
        .await?
        .last()
        .expect("the target has an oplog")
        .0;
    stored.make_unreadable(2).await?;
    let corrupt_rows = stored.rows().await?;

    let executor = start_primary_only(deps, &context).await?;
    let started = session_responses(
        &executor,
        InvocationRequest {
            request: Some(invocation_request::Request::Start(drop_input_start(
                &executor,
                &component,
                &target,
                fingerprint,
                &IdempotencyKey::fresh(),
            ))),
        },
    )
    .await?;
    assert!(
        !has_an_outcome(&started),
        "a Start over an unreadable oplog must give no outcome: {started:?}"
    );
    let resumed = session_responses(
        &executor,
        InvocationRequest {
            request: Some(invocation_request::Request::ResumeAttach(ResumeAttach {
                idempotency_key: accepted.idempotency_key,
                agent_id: crate::agent::start_target_id(&first),
                environment_id: first.environment_id,
                attachment_id: accepted.attachment_id,
                attempt_id: Some(uuid::Uuid::new_v4().into()),
                expected_callee_fingerprint: first.expected_callee_fingerprint,
                expected_epoch: accepted.epoch,
                operation: ResumeOperation::Resume as i32,
                cursors: Vec::new(),
                auth_ctx: first.auth_ctx.clone(),
                caller: first.caller.clone(),
            })),
        },
    )
    .await?;
    assert!(
        !has_an_outcome(&resumed),
        "a Resume over an unreadable oplog must give no outcome: {resumed:?}"
    );
    executor.release().await?;

    let after = stored.rows().await?;
    assert_eq!(
        after
            .iter()
            .filter(|(index, _)| *index <= head)
            .cloned()
            .collect::<Vec<_>>(),
        corrupt_rows,
        "the failed sessions must not change the stored history"
    );
    let appended = decoded_after(&after, head);
    assert!(
        outcome_entries(&appended).is_empty(),
        "the failed sessions must not record an outcome: {appended:?}"
    );
    assert!(
        settling_session_records(&appended).is_empty(),
        "the failed sessions must not end, cancel or detach a session: {:?}",
        settling_session_records(&appended)
    );
    Ok(())
}

/// A Start queues behind a running invocation, and its transport closes before readiness. The
/// queued hold ends with a detach of its attachment.
#[test]
#[timeout("2m")]
#[tracing::instrument]
async fn a_queued_start_whose_transport_closes_before_readiness_detaches(
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
    let target = executor
        .start_agent(
            &component.id,
            agent_key!("StreamingRpcTarget", "queued-transport-close"),
        )
        .await?;
    let caller = crate::agent::keyed_agent_id(
        component.environment_id,
        &agent_key!("StreamingRpcCaller", "queued-transport-close"),
    );
    let fingerprint = executor.get_worker_metadata(&target).await?.fingerprint.0;
    let start_request = || {
        origin_observer_start(
            &executor,
            &component,
            &target,
            &caller,
            fingerprint,
            &IdempotencyKey::fresh(),
            "consume",
        )
    };
    let (gate_tx, gate_rx, gate) = open_raw_session(&executor, start_request()).await?;

    let (frames, receiver) = mpsc::channel(8);
    frames
        .send(InvocationRequest {
            request: Some(invocation_request::Request::Start(start_request())),
        })
        .await?;
    let mut queued = executor
        .client
        .clone()
        .invoke_agent_session(ReceiverStream::new(receiver))
        .await?
        .into_inner();
    let response = tokio::time::timeout(Duration::from_secs(10), queued.message())
        .await??
        .expect("queued acceptance before the gate releases");
    let Some(invocation_response::Response::Accepted(accepted)) = response.response else {
        anyhow::bail!("expected queued acceptance: {response:?}");
    };
    assert!(
        accepted.component_revision.is_none(),
        "the Start must be queued, not ready: {accepted:?}"
    );
    assert!(!detached_at(&executor, &target, accepted.epoch).await?);
    drop(frames);
    drop(queued);

    wait_for_detach(&executor, &target, accepted.epoch).await?;

    gate_tx
        .send(InvocationRequest {
            request: Some(invocation_request::Request::StreamEnd(StreamEnd {
                transport_stream_id: 1,
                producer_sequence: 0,
                direction: StreamDirection::Input as i32,
                authored_by: StreamTerminalAuthor::Guest as i32,
                outcome: Some(StreamTerminal::Finished.try_into().unwrap()),
                durable_stream_id: gate.stream_mappings[0].handle.as_ref().unwrap().stream_id,
                epoch: gate.epoch,
                ..Default::default()
            })),
        })
        .await?;
    let mut gate_finished = gate_rx
        .try_filter(|response| {
            futures::future::ready(matches!(
                response.response,
                Some(invocation_response::Response::Finished(_))
            ))
        })
        .boxed();
    let finished =
        tokio::time::timeout(Duration::from_secs(60), gate_finished.try_next()).await??;
    assert!(finished.is_some(), "the gate invocation completes");
    Ok(())
}
