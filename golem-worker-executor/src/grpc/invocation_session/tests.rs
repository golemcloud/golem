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
use crate::durable_host::durable_stream::tests::TestOplog;
use crate::services::oplog::{OplogReadError, UnreadableOplog};
use golem_common::model::AgentId;
use golem_common::model::component::ComponentId;
use golem_common::model::oplog::OplogIndex;
use test_r::test;

fn unreadable() -> UnreadableOplog {
    UnreadableOplog {
        agent_id: AgentId {
            component_id: ComponentId::new(),
            agent_id: "unreadable".to_string(),
        },
        error: OplogReadError::Gap {
            start: OplogIndex::INITIAL,
            end: OplogIndex::from_u64(2),
        },
    }
}

#[test]
fn a_rejection_for_an_unreadable_oplog_carries_the_agent_failure() {
    let unreadable = unreadable();
    let expected: WorkerExecutionError = WorkerExecutorError::from(unreadable.clone()).into();
    assert_eq!(
        rejection_worker_error(&SessionError::Unreadable(unreadable)),
        Some(expected)
    );
    assert_eq!(
        rejection_worker_error(&SessionError::Failed("boom".to_string())),
        None
    );
}

#[test]
async fn a_protocol_failure_for_an_unreadable_oplog_finishes_as_the_agent_failure() {
    let unreadable = unreadable();
    let (responses, mut received) = mpsc::channel(1);
    send_protocol_failure(&responses, SessionError::Unreadable(unreadable.clone())).await;
    drop(responses);

    let expected = WorkerExecutorError::from(unreadable);
    let Some(InvocationResponse {
        response:
            Some(invocation_response::Response::Finished(InvocationSessionCompletion {
                outcome: Some(invocation_session_completion::Outcome::Failure(failure)),
            })),
    }) = received.recv().await
    else {
        panic!("the session must finish with a failure");
    };
    assert_eq!(failure.kind, InvocationFailureKind::Execution as i32);
    assert_eq!(failure.message, expected.to_string());
    assert_eq!(failure.worker_error, Some(expected.into()));
}

async fn local_session(oplog: Arc<TestOplog>) -> StreamSession {
    use crate::durable_host::durable_stream::DurableStreamStore;
    use crate::durable_host::durable_stream::tests::identity;
    use crate::services::oplog::Oplog;
    use golem_common::base_model::durable_stream::StreamRegistrationInvocation;

    let identity = identity();
    let oplog: Arc<dyn Oplog> = oplog;
    let producer = DurableStreamStore::load(
        oplog.clone(),
        identity.environment_id,
        identity.agent_id,
        identity.fingerprint,
        None,
    )
    .await
    .unwrap();
    StreamSession::new(
        producer,
        oplog,
        StreamRegistrationInvocation::Local(identity.invocation.idempotency_key),
        [],
    )
}

/// Ends the attachment on `error` and returns the responses it sent, whether it drained the
/// outputs, and the session terminal it left in the history.
async fn end_attachment(
    error: SessionError,
) -> (Vec<InvocationResponse>, bool, Option<Result<(), Vec<u8>>>) {
    let streams = local_session(Arc::default()).await;
    let (responses, mut received) = mpsc::channel(8);
    let drained = std::sync::atomic::AtomicBool::new(false);
    end_attachment_on_failure(&streams, &responses, &HashMap::new(), error, async {
        drained.store(true, std::sync::atomic::Ordering::SeqCst);
    })
    .await;
    drop(responses);
    let mut sent = Vec::new();
    while let Some(response) = received.recv().await {
        sent.push(response);
    }
    (
        sent,
        drained.into_inner(),
        streams.persisted_finished().await.unwrap(),
    )
}

fn failure_of(responses: &[InvocationResponse]) -> &InvocationFailure {
    let [
        InvocationResponse {
            response:
                Some(invocation_response::Response::Finished(InvocationSessionCompletion {
                    outcome: Some(invocation_session_completion::Outcome::Failure(failure)),
                })),
        },
    ] = responses
    else {
        panic!("the attachment must finish with one failure, got {responses:?}");
    };
    failure
}

#[test]
async fn only_a_protocol_violation_records_a_failed_session_terminal() {
    let transient = UnreadableOplog {
        error: OplogReadError::source_unavailable(
            crate::services::oplog::OplogReadSource::Archive(0),
            "the archive did not answer",
        ),
        ..unreadable()
    };
    let (sent, drained, terminal) = end_attachment(SessionError::Unreadable(transient)).await;
    assert!(
        sent.is_empty(),
        "the client reattaches on the closed stream"
    );
    assert!(!drained);
    assert_eq!(terminal, None);

    let permanent = unreadable();
    let (sent, drained, terminal) =
        end_attachment(SessionError::Unreadable(permanent.clone())).await;
    let failure = failure_of(&sent);
    assert_eq!(failure.kind, InvocationFailureKind::Execution as i32);
    assert_eq!(
        failure.worker_error,
        Some(WorkerExecutorError::from(permanent).into())
    );
    assert!(!drained);
    assert_eq!(terminal, None);

    let fence = crate::durable_host::durable_stream::tests::test_fence();
    let (sent, drained, terminal) = end_attachment(SessionError::Fenced(fence.clone())).await;
    assert_eq!(
        failure_of(&sent).worker_error,
        Some(WorkerExecutorError::from(OplogError::Fenced(fence)).into())
    );
    assert!(!drained);
    assert_eq!(terminal, None);

    let violation = "input item arrived after its stream ended";
    let (sent, drained, terminal) = end_attachment(SessionError::from(violation)).await;
    let failure = failure_of(&sent);
    assert_eq!(failure.kind, InvocationFailureKind::Protocol as i32);
    assert_eq!(failure.message, violation);
    assert!(drained);
    assert_eq!(terminal, Some(Err(violation.as_bytes().to_vec())));
}

#[test]
async fn a_result_and_terminal_committed_between_the_reads_are_both_read() {
    use golem_common::base_model::durable_stream::{
        StreamSessionFinishedRecord, StreamSessionInvocationResultRecord, StreamSessionRecord,
    };
    use golem_common::model::Timestamp;
    use golem_common::model::oplog::OplogEntry;
    use golem_common::model::oplog::payload::OplogPayload;
    use golem_schema::proto::golem::schema::{SchemaValue as ProtoSchemaValue, schema_value};

    let value = ProtoSchemaValue {
        value: Some(schema_value::Value::U64Value(7)),
    };
    let oplog = Arc::new(TestOplog::default());
    let streams = local_session(oplog.clone()).await;
    let record = |record| OplogEntry::StreamSession {
        timestamp: Timestamp::now_utc(),
        entity_parent_start_index: None,
        record: OplogPayload::Inline(Box::new(record)),
        summary: None,
    };
    oplog.add_after_next_horizon(vec![
        record(StreamSessionRecord::InvocationResult(
            StreamSessionInvocationResultRecord {
                format_version: DURABLE_STREAM_FORMAT_VERSION,
                session_key: streams.session_reference.clone(),
                result: value.encode_to_vec(),
                stream_mappings: vec![],
            },
        )),
        record(StreamSessionRecord::Finished(StreamSessionFinishedRecord {
            format_version: DURABLE_STREAM_FORMAT_VERSION,
            session_key: streams.session_reference.clone(),
            result: Ok(()),
        })),
    ]);

    let (result, finished) = persisted_result_and_finished(&streams, false)
        .await
        .unwrap();

    assert!(
        result.is_some() || finished.is_none(),
        "a successful terminal must not be read without its result, got {finished:?}"
    );
    let (result, finished) = persisted_result_and_finished(&streams, false)
        .await
        .unwrap();
    assert_eq!(
        result,
        Some((
            invocation_session_result::Result::MethodResult(value),
            vec![]
        ))
    );
    assert_eq!(finished, Some(Ok(())));
}
