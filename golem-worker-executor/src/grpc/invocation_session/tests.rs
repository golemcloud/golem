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
use golem_common::base_model::durable_stream::StreamError;
use golem_common::model::AgentId;
use golem_common::model::component::ComponentId;
use golem_common::model::oplog::OplogIndex;
use test_r::test;

struct TestConsumerJournal(Arc<TestOplog>);

#[async_trait::async_trait]
impl crate::durable_host::durable_session::DurableStreamConsumerJournal for TestConsumerJournal {
    async fn commit(&self) -> Result<(), SessionError> {
        use crate::services::oplog::{CommitLevel, Oplog};
        self.0.commit(CommitLevel::Always).await.unwrap();
        Ok(())
    }

    async fn committed_finished_index(
        &self,
        _session: &golem_common::model::durable_stream::StreamSessionKey,
    ) -> Result<Option<OplogIndex>, SessionError> {
        Ok(None)
    }
}

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

async fn routed_stream_session(
    role: SessionStreamRole,
) -> (
    StreamSession,
    Arc<tokio::sync::Mutex<InvocationSessionState>>,
    golem_common::model::durable_stream::DurableStreamHandle,
) {
    use crate::durable_host::durable_stream::DurableStreamStore;
    use crate::durable_host::durable_stream::tests::{identity, registration};
    use crate::services::oplog::Oplog;
    use golem_common::model::durable_stream::{
        StreamSessionAttachedRecord, StreamSessionExpiryPolicy, StreamSessionMappingRecord,
        StreamSessionPreparedRecord, StreamSessionRecord,
    };
    use golem_common::model::invocation_context::TraceId;
    use golem_common::model::oplog::{OplogEntry, payload::OplogPayload};

    let identity = identity();
    let oplog = Arc::new(TestOplog::default());
    let producer = DurableStreamStore::load(
        oplog.clone(),
        identity.environment_id,
        identity.agent_id.clone(),
        identity.fingerprint,
        None,
    )
    .await
    .unwrap();
    let attachment_id = AttachmentId::primary(
        identity.environment_id,
        &identity.agent_id,
        &identity.invocation.idempotency_key,
    )
    .unwrap();
    let attempt_id = AttemptId::fresh();
    let mut registration = registration(
        &identity,
        StreamRegistrationCoordinate::Root {
            invocation_id: identity.invocation.clone(),
            root_kind: if role == SessionStreamRole::Input {
                StreamRootKind::MethodInput
            } else {
                StreamRootKind::MethodResult
            },
            recursive_value_path: Vec::new(),
        },
        if role == SessionStreamRole::Input {
            StreamSourceKind::ExternalInlineInput
        } else {
            StreamSourceKind::InvocationOutput
        },
    );
    registration.session_mapping = Some(StreamSessionMapping {
        session_key: identity.invocation.clone(),
        attachment_id,
        role,
    });
    let handle = producer.register(None, registration).await.unwrap().value;
    let mapping = StreamSessionMappingRecord {
        transport_stream_id: 7,
        handle: handle.clone(),
        role,
    };
    let binding = producer.local_binding(7, &handle, role).await.unwrap();
    producer
        .append_session_record(
            None,
            StreamSessionRecord::Prepared(StreamSessionPreparedRecord {
                format_version: DURABLE_STREAM_FORMAT_VERSION,
                public_session_id: identity.invocation.idempotency_key.to_string(),
                route_scope: None,
                session_key: identity.invocation.idempotency_key,
                expiry_policy: StreamSessionExpiryPolicy::None,
                expiry_deadline_millis: None,
                attempt: StartAttemptDescriptor {
                    format_version: DURABLE_STREAM_FORMAT_VERSION,
                    session_key: identity.invocation.clone(),
                    attachment_id,
                    expected_callee_fingerprint: identity.fingerprint,
                    attempt_id,
                    invocation: PersistedStreamInvocationDescriptor {
                        format_version: DURABLE_STREAM_FORMAT_VERSION,
                        session_key: identity.invocation.clone(),
                        target_component_revision: ComponentRevision::INITIAL,
                        target: PersistedInvocationTarget::AgentMethod {
                            method_name: "run".into(),
                        },
                        invocation_value: vec![1],
                        stream_handles: if role.direction() == SessionStreamRole::Input {
                            vec![handle.clone()]
                        } else {
                            Vec::new()
                        },
                        execution_config: vec![],
                        effective_identity: vec![],
                    },
                    effective_identity: vec![],
                    live_join_buffer_events: 8,
                },
                stream_mappings: vec![binding.clone()],
            }),
        )
        .await
        .unwrap();
    let pending_invocation_oplog_index = oplog
        .add(OplogEntry::pending_agent_invocation(
            None,
            identity.invocation.idempotency_key.clone(),
            OplogPayload::Inline(Box::new(
                golem_common::model::AgentInvocationPayload::SaveSnapshot,
            )),
            TraceId::generate(),
            vec![],
            vec![],
        ))
        .await
        .unwrap();
    producer
        .append_session_record(
            None,
            StreamSessionRecord::Attached(StreamSessionAttachedRecord {
                format_version: DURABLE_STREAM_FORMAT_VERSION,
                session_key: identity.invocation.idempotency_key.clone(),
                attachment_id,
                attempt_id,
                epoch: 1,
                pending_invocation_oplog_index,
            }),
        )
        .await
        .unwrap();
    let streams = StreamSession::open(
        producer,
        oplog.clone(),
        StreamRegistrationInvocation::Remote(identity.invocation.clone()),
        [binding],
    )
    .await
    .unwrap()
    .with_consumer_journal(Arc::new(TestConsumerJournal(oplog)))
    .with_attachment(1, attempt_id);
    let mut state = InvocationSessionState::default();
    let input = if role == SessionStreamRole::Input {
        golem_schema::proto::golem::schema::SchemaValue {
            value: Some(
                golem_schema::proto::golem::schema::schema_value::Value::StreamReference(
                    golem_schema::proto::golem::schema::SchemaValueStreamReference { stream_id: 7 },
                ),
            ),
        }
    } else {
        golem_schema::proto::golem::schema::SchemaValue::try_from(SchemaValue::Tuple {
            elements: vec![],
        })
        .unwrap()
    };
    state
        .validate_trusted_request(&InvocationRequest {
            request: Some(invocation_request::Request::Start(InvocationStart {
                method_name: Some("run".into()),
                input: Some(input),
                idempotency_key: Some(
                    golem_common::model::IdempotencyKey::new("routed-stream-session".into()).into(),
                ),
                ..Default::default()
            })),
        })
        .unwrap();
    state
        .validate_response(&InvocationResponse {
            response: Some(invocation_response::Response::Accepted(
                InvocationAccepted {
                    agent_id: Some(identity.agent_id.into()),
                    idempotency_key: Some(identity.invocation.idempotency_key.into()),
                    component_revision: Some(ComponentRevision::INITIAL.get()),
                    attachment_id: Some(attachment_id.0.into()),
                    attempt_id: Some(attempt_id.0.into()),
                    epoch: 1,
                    environment_id: Some(identity.environment_id.into()),
                    callee_fingerprint: Some(identity.fingerprint.0.into()),
                    stream_mappings: vec![durable_stream_mapping_to_proto(&mapping, None)],
                    ..Default::default()
                },
            )),
        })
        .unwrap();
    (streams, Arc::new(tokio::sync::Mutex::new(state)), handle)
}

#[test]
#[test_r::timeout("30s")]
async fn explicit_input_failure_records_the_exact_source_terminal_and_ack() {
    use crate::durable_host::durable_stream::CommittedProducerStreamEventPayload;
    use golem_api_grpc::proto::golem::worker::{
        InputStreamItem, StreamDirection, StreamEnd, input_stream_item::Payload,
    };
    use golem_common::model::durable_stream::StreamTerminalOrigin;
    use golem_schema::schema::IntoTypedSchemaValue;

    for terminal in [
        StreamTerminal::Finished,
        StreamTerminal::Failed(StreamError::Failed(
            (73u32, "input rejected".to_string())
                .into_typed_schema_value()
                .unwrap()
                .into(),
        )),
        StreamTerminal::Failed(StreamError::Interrupted("writer stopped".into())),
        StreamTerminal::Failed(StreamError::Trapped("trap".into())),
        StreamTerminal::Failed(StreamError::Abandoned),
        StreamTerminal::Failed(StreamError::ResourceExhausted("memory".into())),
    ] {
        for author in [
            StreamTerminalAuthor::Guest,
            StreamTerminalAuthor::Host,
            StreamTerminalAuthor::Protocol,
        ] {
            let (streams, state, handle) = routed_stream_session(SessionStreamRole::Input).await;
            let (responses, mut received) = mpsc::channel(2);
            request::route_durable_request(
                &streams,
                &responses,
                &state,
                InvocationRequest {
                    request: Some(invocation_request::Request::InputItem(InputStreamItem {
                        transport_stream_id: 7,
                        sequence: 0,
                        durable_stream_id: Some(handle.stream_id.0.into()),
                        epoch: 1,
                        payload: Some(Payload::PackedU8(vec![17, 255])),
                    })),
                },
            )
            .await
            .unwrap();
            state
                .lock()
                .await
                .validate_response(&received.recv().await.unwrap())
                .unwrap();
            let request = InvocationRequest {
                request: Some(invocation_request::Request::StreamEnd(StreamEnd {
                    transport_stream_id: 7,
                    producer_sequence: 2,
                    durable_stream_id: Some(handle.stream_id.0.into()),
                    epoch: 1,
                    direction: StreamDirection::Input as i32,
                    authored_by: golem_api_grpc::proto::golem::worker::StreamTerminalAuthor::from(
                        author,
                    ) as i32,
                    outcome: Some(terminal.clone().try_into().unwrap()),
                    ..Default::default()
                })),
            };
            let request = InvocationRequest::decode(request.encode_to_vec().as_slice()).unwrap();
            request::route_durable_request(&streams, &responses, &state, request)
                .await
                .unwrap();
            let response = received.recv().await.unwrap();
            state.lock().await.validate_response(&response).unwrap();
            let Some(invocation_response::Response::InputAck(ack)) = response.response else {
                panic!("producer terminal must acknowledge its exact sequence");
            };
            assert_eq!(ack.transport_stream_id, 7);
            assert_eq!(ack.highest_contiguous_sequence, 2);
            assert_eq!(ack.logical_item_count, 1);
            assert_eq!(ack.durable_stream_id, Some(handle.stream_id.0.into()));
            let mut reader = streams.producer.catch_up(handle, None).await.unwrap();
            for byte in [17, 255] {
                assert_eq!(
                    reader.next().await.unwrap().unwrap().payload,
                    CommittedProducerStreamEventPayload::PackedU8(byte)
                );
            }
            let event = reader.next().await.unwrap().unwrap();
            assert_eq!(ack.resulting_offset, event.offset.as_bytes().to_vec());
            assert_eq!(event.producer_sequence, 2);
            assert_eq!(event.terminal_author, Some(author));
            assert_eq!(event.terminal_origin, Some(StreamTerminalOrigin::Source));
            assert_eq!(
                event.payload,
                CommittedProducerStreamEventPayload::Terminal(terminal.clone())
            );
            assert!(reader.next().await.unwrap().is_none());
            assert!(
                !matches!(
                    event.payload,
                    CommittedProducerStreamEventPayload::Terminal(StreamTerminal::Failed(
                        StreamError::Cancelled
                    ))
                ),
                "a writer ending does not synthesize invocation cancellation"
            );
        }
    }
}

#[test]
#[test_r::timeout("30s")]
async fn client_output_consumer_cannot_forge_a_producer_terminal() {
    use golem_api_grpc::proto::golem::worker::{StreamDirection, StreamEnd};
    for terminal in [
        StreamTerminal::Finished,
        StreamTerminal::Failed(StreamError::Cancelled),
    ] {
        let (streams, state, handle) = routed_stream_session(SessionStreamRole::Output).await;
        let before = streams.oplog.current_oplog_index().await;
        let (responses, mut received) = mpsc::channel(1);
        let result = request::route_durable_request(
            &streams,
            &responses,
            &state,
            InvocationRequest {
                request: Some(invocation_request::Request::StreamEnd(StreamEnd {
                    transport_stream_id: 7,
                    durable_stream_id: Some(handle.stream_id.0.into()),
                    epoch: 1,
                    direction: StreamDirection::Output as i32,
                    authored_by: golem_api_grpc::proto::golem::worker::StreamTerminalAuthor::Guest
                        as i32,
                    outcome: Some(terminal.try_into().unwrap()),
                    ..Default::default()
                })),
            },
        )
        .await;
        assert!(result.is_err());
        assert_eq!(streams.oplog.current_oplog_index().await, before);
        assert!(received.try_recv().is_err());
    }
}

#[test]
#[test_r::timeout("30s")]
async fn client_output_cancellation_preserves_author_and_control_origin() {
    use crate::durable_host::durable_stream::CommittedProducerStreamEventPayload;
    use golem_api_grpc::proto::golem::worker::{StreamDirection, StreamEnd};
    use golem_common::model::durable_stream::StreamTerminalOrigin;

    for author in [
        StreamTerminalAuthor::Guest,
        StreamTerminalAuthor::Host,
        StreamTerminalAuthor::Protocol,
    ] {
        let (streams, state, handle) = routed_stream_session(SessionStreamRole::Output).await;
        let terminal = StreamTerminal::ConsumerCancelled {
            details: Some("reader stopped".into()),
        };
        let (responses, mut received) = mpsc::channel(1);
        request::route_durable_request(
            &streams,
            &responses,
            &state,
            InvocationRequest {
                request: Some(invocation_request::Request::StreamEnd(StreamEnd {
                    transport_stream_id: 7,
                    durable_stream_id: Some(handle.stream_id.0.into()),
                    epoch: 1,
                    direction: StreamDirection::Output as i32,
                    authored_by: golem_api_grpc::proto::golem::worker::StreamTerminalAuthor::from(
                        author,
                    ) as i32,
                    outcome: Some(terminal.clone().try_into().unwrap()),
                    ..Default::default()
                })),
            },
        )
        .await
        .unwrap();
        let mut reader = streams.producer.catch_up(handle, None).await.unwrap();
        let event = reader.next().await.unwrap().unwrap();
        assert_eq!(event.terminal_origin, Some(StreamTerminalOrigin::Control));
        assert_eq!(event.terminal_author, Some(author));
        assert_eq!(
            event.payload,
            CommittedProducerStreamEventPayload::Terminal(terminal)
        );
        assert!(reader.next().await.unwrap().is_none());
        assert!(
            received.try_recv().is_err(),
            "consumer control must not synthesize an input ack"
        );
    }
}

#[test]
#[test_r::timeout("30s")]
async fn malformed_stream_end_is_rejected_before_any_durable_write() {
    use golem_api_grpc::proto::golem::worker::{StreamDirection, StreamEnd};
    for invalid in [
        "missing outcome",
        "empty outcome",
        "missing author",
        "unknown author",
        "missing direction",
        "unknown direction",
    ] {
        let (streams, state, handle) = routed_stream_session(SessionStreamRole::Input).await;
        let mut end = StreamEnd {
            transport_stream_id: 7,
            durable_stream_id: Some(handle.stream_id.0.into()),
            epoch: 1,
            direction: StreamDirection::Input as i32,
            authored_by: golem_api_grpc::proto::golem::worker::StreamTerminalAuthor::Guest as i32,
            outcome: Some(StreamTerminal::Finished.try_into().unwrap()),
            ..Default::default()
        };
        match invalid {
            "missing outcome" => end.outcome = None,
            "empty outcome" => end.outcome = Some(Default::default()),
            "missing author" => end.authored_by = 0,
            "unknown author" => end.authored_by = i32::MAX,
            "missing direction" => end.direction = 0,
            "unknown direction" => end.direction = i32::MAX,
            _ => unreachable!(),
        }
        let before = streams.oplog.current_oplog_index().await;
        let (responses, mut received) = mpsc::channel(1);
        assert!(
            request::route_durable_request(
                &streams,
                &responses,
                &state,
                InvocationRequest {
                    request: Some(invocation_request::Request::StreamEnd(end)),
                }
            )
            .await
            .is_err(),
            "{invalid}"
        );
        assert_eq!(
            streams.oplog.current_oplog_index().await,
            before,
            "{invalid}"
        );
        assert!(received.try_recv().is_err());
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
