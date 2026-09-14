use super::*;
use crate::durable_host::durable_stream::AttachedStreamSegmentSource;
use crate::durable_host::durable_stream::tests::{
    TestIdentity, TestOplog, attachment_key, identity, registration,
};
use crate::durable_host::stream_transport::{output_stream_pair, test_output_stream_pair};
use crate::services::oplog::CommitLevel;
use crate::services::rpc::{DurableStreamReadError, RpcDemand, RpcError};
use golem_api_grpc::proto::golem::schema::{ListValue, SchemaValueStreamReference, schema_value};
use golem_common::base_model::component::{ComponentId, ComponentRevision};
use golem_common::base_model::durable_stream::{
    AttachmentId, PersistedStreamInvocationDescriptorV1, ResumeAttemptDescriptorV1,
    StartAttemptDescriptorV1, StreamAttachmentKeyV1, StreamId, StreamInvocationIdV1,
    StreamOffsetV1, StreamSessionAttachedRecordV1, StreamSessionFinishedRecordV1,
    StreamSessionMappingV1, StreamSessionPreparedRecordV1,
};
use golem_common::base_model::environment::EnvironmentId;
use golem_common::base_model::{AgentFingerprint, AgentId, IdempotencyKey};
use golem_common::model::account::AccountId;
use golem_common::model::agent::InvocationFreshnessDisposition;
use golem_common::model::invocation_context::TraceId;
use golem_common::model::worker::AgentConfigEntryDto;
use golem_common::model::{AgentInvocationPayload, OplogIndex, OwnedAgentId};
use golem_schema::schema::schema_value::UnionValuePayload;
use golem_schema::schema::{
    DiscriminatorRule, FieldDiscriminator, NamedFieldType, UnionBranch, UnionSpec,
};
use test_r::test;
use uuid::Uuid;

#[test]
fn system_durable_stream_cancellation_is_a_permanent_stream_error() {
    let error = durable_stream_cancel_error(
        StreamCancelRoleV1::System,
        StreamCancelReasonV1::Cancelled,
        Some("source stopped".to_string()),
    );

    let classified = error
        .downcast_ref::<ClassifiedHostError>()
        .expect("system stream cancellation must retain its retry classification");
    assert_eq!(classified.kind, HostFailureKind::Permanent);
    assert_eq!(
        classified.message,
        "durable stream cancelled (System, Cancelled): source stopped"
    );
}

struct TestConsumerJournal(Arc<dyn Oplog>);

#[async_trait::async_trait]
impl DurableStreamConsumerJournal for TestConsumerJournal {
    async fn commit(&self) -> Result<(), String> {
        self.0.commit(CommitLevel::Always).await;
        Ok(())
    }

    async fn committed_finished_index(
        &self,
        _session: &StreamSessionKeyV1,
    ) -> Result<Option<OplogIndex>, String> {
        Ok(None)
    }
}

async fn append_prepared_pending(
    producer: &DurableStreamProducer,
    oplog: &TestOplog,
    identity: &TestIdentity,
    attachment_id: AttachmentId,
    attempt_id: AttemptId,
    handle: &DurableStreamHandleV1,
    role: SessionStreamRoleV1,
) -> OplogIndex {
    let stream_mappings = if role == SessionStreamRoleV1::Input {
        vec![StreamSessionMappingRecordV1 {
            transport_stream_id: 7,
            handle: handle.clone(),
            role,
        }]
    } else {
        Vec::new()
    };
    producer
        .append_session_record(StreamSessionRecordV1::Prepared(
            StreamSessionPreparedRecordV1 {
                format_version: DURABLE_STREAM_FORMAT_VERSION,
                attempt: StartAttemptDescriptorV1 {
                    format_version: DURABLE_STREAM_FORMAT_VERSION,
                    session_key: identity.invocation.clone(),
                    attachment_id,
                    expected_callee_fingerprint: identity.fingerprint,
                    attempt_id,
                    invocation: PersistedStreamInvocationDescriptorV1 {
                        format_version: DURABLE_STREAM_FORMAT_VERSION,
                        session_key: identity.invocation.clone(),
                        target_component_revision: ComponentRevision::INITIAL,
                        method_name: "consume".to_string(),
                        invocation_value: vec![1],
                        stream_handles: stream_mappings
                            .iter()
                            .map(|mapping| mapping.handle.clone())
                            .collect(),
                        execution_config: vec![2],
                        effective_identity: vec![3],
                    },
                    effective_identity: vec![3],
                    live_join_buffer_events: 8,
                },
                stream_mappings,
            },
        ))
        .await
        .unwrap();
    oplog
        .add(OplogEntry::pending_agent_invocation(
            identity.invocation.idempotency_key.clone(),
            OplogPayload::Inline(Box::new(AgentInvocationPayload::SaveSnapshot)),
            TraceId::generate(),
            Vec::new(),
            Vec::new(),
        ))
        .await
}

#[test]
fn private_cancellation_mapping_preserves_durable_failure_reasons() {
    use golem_api_grpc::proto::golem::worker::StreamCancelReason;

    for (durable, proto) in [
        (
            StreamCancelReasonV1::Cancelled,
            StreamCancelReason::Cancelled,
        ),
        (
            StreamCancelReasonV1::GuestDrop,
            StreamCancelReason::ConsumerDrop,
        ),
        (StreamCancelReasonV1::Protocol, StreamCancelReason::Protocol),
        (
            StreamCancelReasonV1::InvocationFailed,
            StreamCancelReason::InvocationFailed,
        ),
        (
            StreamCancelReasonV1::SourceUnavailable,
            StreamCancelReason::SourceUnavailable,
        ),
        (
            StreamCancelReasonV1::ProducerDeleting,
            StreamCancelReason::ProducerDeleting,
        ),
    ] {
        assert_eq!(stream_cancel_reason_to_proto(durable), proto);
    }
}

#[test]
async fn guest_owned_u8_output_uses_the_packed_durable_path() {
    let identity = identity();
    let oplog = Arc::new(TestOplog::default());
    let producer = DurableStreamProducer::load(
        oplog.clone(),
        identity.environment_id,
        identity.agent_id.clone(),
        identity.fingerprint,
        None,
    )
    .await
    .unwrap();
    let handle = producer
        .register(registration(
            &identity,
            StreamRegistrationCoordinateV1::Root {
                invocation_id: identity.invocation.clone(),
                root_kind: StreamRootKindV1::MethodResult,
                recursive_value_path: Vec::new(),
            },
            StreamSourceKindV1::InvocationOutput,
        ))
        .await
        .unwrap()
        .value;
    let entries_before_output = oplog.length().await;
    let streams =
        DurableSessionStreams::new(producer.clone(), oplog.clone(), identity.invocation, []);
    let (publisher, endpoint) = test_output_stream_pair(4).unwrap();
    let (nested_tx, _nested_rx) = mpsc::unbounded_channel();
    let drain = PendingOwnedStreamDrain {
        handle: handle.clone(),
        endpoint,
        element_type: SchemaType::u8(),
        role: SessionStreamRoleV1::Output,
    };
    let drain_task = tokio::spawn(async move {
        streams
            .drain_output(
                drain,
                Arc::new(SchemaGraph::anonymous(SchemaType::u8())),
                nested_tx,
            )
            .await
    });

    publisher.publish_item(SchemaValue::U8(10)).await.unwrap();
    publisher.publish_item(SchemaValue::U8(11)).await.unwrap();
    publisher.publish_end().await.unwrap();
    drain_task.await.unwrap().unwrap();
    assert_eq!(
        oplog.length().await,
        entries_before_output + 2,
        "consecutive guest u8 values must share one durable item batch"
    );

    let mut reader = producer.catch_up(handle, None).await.unwrap();
    assert!(matches!(
        reader.next().await.unwrap().unwrap().payload,
        CommittedProducerStreamEventPayloadV1::PackedU8(10)
    ));
    assert!(matches!(
        reader.next().await.unwrap().unwrap().payload,
        CommittedProducerStreamEventPayloadV1::PackedU8(11)
    ));
    assert!(matches!(
        reader.next().await.unwrap().unwrap().payload,
        CommittedProducerStreamEventPayloadV1::End(StreamEndResultV1::Ok)
    ));
}

#[test]
#[test_r::timeout("30s")]
async fn materialized_output_releases_admission_and_survives_abandoned_response() {
    let identity = identity();
    let oplog = Arc::new(TestOplog::default());
    let producer = DurableStreamProducer::load(
        oplog.clone(),
        identity.environment_id,
        identity.agent_id.clone(),
        identity.fingerprint,
        None,
    )
    .await
    .unwrap();
    let streams = DurableSessionStreams::new(producer.clone(), oplog, identity.invocation, []);
    let (publisher, endpoint) = test_output_stream_pair(4).unwrap();
    let task_streams = streams.clone();
    let materialization = tokio::spawn(async move {
        let root = SchemaType::stream(Some(SchemaType::u8()));
        task_streams
            .materialize_result(
                SchemaValue::Stream(SchemaValueStream::from_host_endpoint(endpoint)),
                &SchemaGraph::anonymous(root.clone()),
                &root,
                ComponentRevision::INITIAL,
            )
            .await
    });
    streams.wait_persisted_result().await.unwrap();
    let handle = streams
        .remote_result_record()
        .await
        .unwrap()
        .unwrap()
        .output_streams[0]
        .clone();

    // The live drain must leave all sixteen normal operation slots available.
    let admitted = Arc::new(tokio::sync::Barrier::new(17));
    let mut operations = tokio::task::JoinSet::new();
    for _ in 0..16 {
        let producer = producer.clone();
        let admitted = admitted.clone();
        operations.spawn(async move {
            producer
                .run_owned(0, move |_| async move {
                    admitted.wait().await;
                    Ok::<_, String>(())
                })
                .await
        });
    }
    tokio::time::timeout(std::time::Duration::from_secs(5), admitted.wait())
        .await
        .expect("a long-lived output drain retained mutation admission");
    while let Some(result) = operations.join_next().await {
        result.unwrap().unwrap();
    }
    materialization.abort();
    assert!(materialization.await.unwrap_err().is_cancelled());

    publisher.publish_item(SchemaValue::U8(37)).await.unwrap();
    publisher.publish_item(SchemaValue::U8(81)).await.unwrap();
    publisher.publish_end().await.unwrap();
    let mut reader = producer.catch_up(handle, None).await.unwrap();
    for expected in [37, 81] {
        assert_eq!(
            reader.next().await.unwrap().unwrap().payload,
            CommittedProducerStreamEventPayloadV1::PackedU8(expected)
        );
    }
    assert_eq!(
        reader.next().await.unwrap().unwrap().payload,
        CommittedProducerStreamEventPayloadV1::End(StreamEndResultV1::Ok)
    );
}

#[test]
#[test_r::timeout("30s")]
async fn concurrent_nested_mapping_reuses_identity_before_commit_callback_finishes() {
    use crate::durable_host::durable_stream::DurableStreamCommit;
    use std::sync::atomic::AtomicBool;

    let identity = identity();
    let oplog = Arc::new(TestOplog::default());
    let block = Arc::new(AtomicBool::new(false));
    let reached = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Semaphore::new(0));
    let commit: DurableStreamCommit = Arc::new({
        let oplog = oplog.clone();
        let block = block.clone();
        let reached = reached.clone();
        let release = release.clone();
        move |receipt| {
            let oplog = oplog.clone();
            let block = block.clone();
            let reached = reached.clone();
            let release = release.clone();
            Box::pin(async move {
                oplog.commit(CommitLevel::Always).await;
                if let Some(receipt) = receipt {
                    let _ = receipt.send(());
                }
                if block.load(Ordering::Acquire) {
                    reached.notify_one();
                    release.acquire().await.unwrap().forget();
                }
            })
        }
    });
    let producer = DurableStreamProducer::load_with_commit(
        oplog.clone(),
        identity.environment_id,
        identity.agent_id.clone(),
        identity.fingerprint,
        None,
        commit,
    )
    .await
    .unwrap();
    let handle = producer
        .register(registration(
            &identity,
            StreamRegistrationCoordinateV1::Root {
                invocation_id: identity.invocation.clone(),
                root_kind: StreamRootKindV1::MethodResult,
                recursive_value_path: Vec::new(),
            },
            StreamSourceKindV1::InvocationOutput,
        ))
        .await
        .unwrap()
        .value;
    let streams = DurableSessionStreams::new(producer, oplog, identity.invocation, []);
    block.store(true, Ordering::Release);
    let first = tokio::spawn({
        let streams = streams.clone();
        let handle = handle.clone();
        async move {
            streams
                .ensure_nested_mapping(handle, SessionStreamRoleV1::Output)
                .await
        }
    });
    reached.notified().await;
    let mut second = tokio::spawn({
        let streams = streams.clone();
        let handle = handle.clone();
        async move {
            streams
                .ensure_nested_mapping(handle, SessionStreamRoleV1::Output)
                .await
        }
    });
    let second_result = tokio::select! {
        result = &mut second => Some(result),
        _ = reached.notified() => None,
    };
    release.add_permits(2);
    let first = first.await.unwrap().unwrap();
    let second = match second_result {
        Some(result) => result,
        None => second.await,
    }
    .unwrap()
    .unwrap();
    assert_eq!(first, second);
    assert_eq!(first.handle, handle);
    assert_eq!(
        streams
            .current_control_metadata()
            .await
            .unwrap()
            .persisted_mappings
            .len(),
        1
    );
}

async fn activate_test_root_attachment(
    producer: &DurableStreamProducer,
    identity: &TestIdentity,
    handle: &DurableStreamHandleV1,
) {
    let mut consumer_invocation = identity.invocation.clone();
    consumer_invocation.callee.agent_id = "distinct-root-consumer".to_string();
    let attachment = StreamAttachmentKeyV1 {
        attachment_id: AttachmentId::primary(
            identity.environment_id,
            &identity.agent_id,
            &identity.invocation.idempotency_key,
        )
        .unwrap(),
        stream_id: handle.stream_id,
        epoch: 1,
        session_key: identity.invocation.clone(),
        producer_environment_id: handle.producer_environment_id,
        producer: handle.producer.clone(),
        expected_producer_fingerprint: handle.expected_producer_fingerprint,
        consumer_environment_id: consumer_invocation.callee_environment_id,
        consumer: consumer_invocation.callee.clone(),
        expected_consumer_fingerprint: consumer_invocation.callee_fingerprint,
        consumer_invocation,
    };
    let now = Timestamp::now_utc().to_millis();
    producer
        .prepare_attachment(attachment.clone(), now)
        .await
        .unwrap();
    producer.activate_attachment(attachment, now).await.unwrap();
}

async fn assert_local_nested_stream_drains_after_root_admission(root_kind: StreamRootKindV1) {
    let identity = identity();
    let oplog = Arc::new(TestOplog::default());
    let producer = DurableStreamProducer::load(
        oplog.clone(),
        identity.environment_id,
        identity.agent_id.clone(),
        identity.fingerprint,
        None,
    )
    .await
    .unwrap();
    let streams = DurableSessionStreams::new(
        producer.clone(),
        oplog.clone(),
        identity.invocation.clone(),
        [],
    )
    .with_consumer_journal(Arc::new(TestConsumerJournal(oplog)))
    .require_root_attachment_before_production();
    let child_type = SchemaType::stream(Some(SchemaType::u8()));
    let root_type = SchemaType::stream(Some(child_type));
    let graph = SchemaGraph::anonymous(root_type.clone());
    let (root_publisher, root_endpoint) = test_output_stream_pair(4).unwrap();

    let materialize_result = match root_kind {
        StreamRootKindV1::MethodInput => {
            streams
                .materialize_agent_input(
                    &SchemaValue::Stream(SchemaValueStream::from_host_endpoint(root_endpoint)),
                    &graph,
                    &root_type,
                    ComponentRevision::INITIAL,
                )
                .await
                .unwrap();
            None
        }
        StreamRootKindV1::MethodResult => {
            let streams = streams.clone();
            let graph = graph.clone();
            let root_type = root_type.clone();
            Some(tokio::spawn(async move {
                streams
                    .materialize_result(
                        SchemaValue::Stream(SchemaValueStream::from_host_endpoint(root_endpoint)),
                        &graph,
                        &root_type,
                        ComponentRevision::INITIAL,
                    )
                    .await
            }))
        }
    };
    let coordinate = StreamRegistrationCoordinateV1::Root {
        invocation_id: identity.invocation.clone(),
        root_kind,
        recursive_value_path: Vec::new(),
    };
    let root_handle = tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            if let Some(handle) = producer.handle_for_coordinate(&coordinate).await.unwrap() {
                break handle;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("root stream was not registered");
    assert_eq!(
        producer
            .input_high_water(root_handle.stream_id)
            .await
            .unwrap(),
        None
    );

    activate_test_root_attachment(&producer, &identity, &root_handle).await;
    let (first_publisher, first_endpoint) = test_output_stream_pair(2).unwrap();
    let (second_publisher, second_endpoint) = test_output_stream_pair(2).unwrap();
    root_publisher
        .publish_item(SchemaValue::Stream(SchemaValueStream::from_host_endpoint(
            first_endpoint,
        )))
        .await
        .unwrap();
    root_publisher
        .publish_item(SchemaValue::Stream(SchemaValueStream::from_host_endpoint(
            second_endpoint,
        )))
        .await
        .unwrap();
    root_publisher.publish_end().await.unwrap();
    wait_for_terminal_commit(&producer, root_handle.stream_id).await;

    // Both children are queued before the parent's final join, but remain live afterwards.
    let first_handle = producer
        .nested_handles(root_handle.stream_id, 0)
        .await
        .unwrap()[0]
        .clone();
    let second_handle = producer
        .nested_handles(root_handle.stream_id, 1)
        .await
        .unwrap()[0]
        .clone();
    first_publisher
        .publish_item(SchemaValue::U8(17))
        .await
        .unwrap();
    first_publisher.publish_end().await.unwrap();
    second_publisher
        .publish_item(SchemaValue::U8(29))
        .await
        .unwrap();
    second_publisher.publish_end().await.unwrap();

    for (handle, expected) in [(first_handle, 17), (second_handle, 29)] {
        let mut reader = DurableStreamReader::Owned {
            reader: Box::new(producer.catch_up(handle.clone(), None).await.unwrap()),
            source: producer.clone(),
            handle: Box::new(handle),
            next_journal_lag_sample: Instant::now(),
        };
        assert!(matches!(
            tokio::time::timeout(Duration::from_secs(1), reader.next()).await.unwrap().unwrap().unwrap().payload,
            CommittedProducerStreamEventPayloadV1::PackedU8(value) if value == expected
        ));
        assert!(matches!(
            reader.next().await.unwrap().unwrap().payload,
            CommittedProducerStreamEventPayloadV1::End(StreamEndResultV1::Ok)
        ));
    }
    assert_eq!(
        StreamAttachmentControl::inspect_attachments(producer.as_ref())
            .await
            .len(),
        1,
        "local nested streams must not require independent attachments"
    );
    if let Some(task) = materialize_result {
        task.await.unwrap().unwrap();
    }
}

#[test]
async fn caller_input_local_nested_streams_inherit_root_admission() {
    assert_local_nested_stream_drains_after_root_admission(StreamRootKindV1::MethodInput).await;
}

#[test]
async fn result_local_nested_streams_inherit_root_admission() {
    assert_local_nested_stream_drains_after_root_admission(StreamRootKindV1::MethodResult).await;
}

struct RecordingConsumerJournal {
    oplog: Arc<dyn Oplog>,
    commits: Arc<AtomicU64>,
}

type LagSample = (Option<StreamOffsetV1>, Result<usize, ()>);

struct LagRecordingSource {
    producer: Arc<DurableStreamProducer>,
    calls: Mutex<Vec<LagSample>>,
    failures_remaining: AtomicU64,
}

impl LagRecordingSource {
    fn new(producer: Arc<DurableStreamProducer>, failures: u64) -> Arc<Self> {
        Arc::new(Self {
            producer,
            calls: Mutex::new(Vec::new()),
            failures_remaining: AtomicU64::new(failures),
        })
    }
}

#[async_trait::async_trait]
impl AttachedStreamSegmentSource for LagRecordingSource {
    async fn journal_lag_events(
        &self,
        handle: &DurableStreamHandleV1,
        after: Option<StreamOffsetV1>,
    ) -> Result<usize, DurableStreamProducerError> {
        if self
            .failures_remaining
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |remaining| {
                remaining.checked_sub(1)
            })
            .is_ok()
        {
            self.calls.lock().await.push((after, Err(())));
            return Err(DurableStreamProducerError::InvalidHandle);
        }
        let lag = self.producer.journal_lag_events(handle, after).await?;
        self.calls.lock().await.push((after, Ok(lag)));
        Ok(lag)
    }

    async fn read_attached_segment(
        &self,
        attachment: &StreamAttachmentKeyV1,
        handle: &DurableStreamHandleV1,
        now_millis: u64,
        after: Option<StreamOffsetV1>,
        through: Option<StreamOffsetV1>,
    ) -> Result<Vec<CommittedProducerStreamEventV1>, DurableStreamProducerError> {
        self.producer
            .read_attached_segment(attachment, handle, now_millis, after, through)
            .await
    }

    async fn wait_for_attached_segment(
        &self,
        attachment: &StreamAttachmentKeyV1,
        handle: &DurableStreamHandleV1,
        now_millis: u64,
        after: Option<StreamOffsetV1>,
    ) -> Result<Vec<CommittedProducerStreamEventV1>, DurableStreamProducerError> {
        self.producer
            .wait_for_attached_segment(attachment, handle, now_millis, after)
            .await
    }
}

struct AttachedProducerRpc {
    producer: Arc<DurableStreamProducer>,
    cancellation_owner: Option<Arc<DurableStreamProducer>>,
    stall_next_cancel: std::sync::atomic::AtomicBool,
    scripted_reads: Mutex<VecDeque<Result<Vec<u8>, DurableStreamReadError<RpcError>>>>,
    pending_read: Mutex<Option<tokio::sync::oneshot::Receiver<()>>>,
    read_requests:
        Mutex<Vec<golem_common::base_model::durable_stream::AttachedStreamSegmentRequestV1>>,
}

#[async_trait::async_trait]
impl Rpc for AttachedProducerRpc {
    async fn control_durable_stream_attachment(
        &self,
        request: golem_common::base_model::durable_stream::StreamAttachmentControlRequestV1,
        _auth_ctx: &AuthCtx,
    ) -> Result<bool, RpcError> {
        if self.stall_next_cancel.swap(false, Ordering::SeqCst) {
            std::future::pending::<()>().await;
        }
        if self.cancellation_owner.is_none() {
            let golem_common::base_model::durable_stream::StreamAttachmentControlOperationV1::Cancel {
                key, role, reason, details,
            } = request.operation else { panic!("unexpected control RPC") };
            self.producer
                .cancel_open(key.stream_id, role, reason, details)
                .await
                .map_err(|error| RpcError::ProtocolError {
                    details: error.to_string(),
                })?;
            return Ok(false);
        }
        let owner = self.cancellation_owner.as_ref().unwrap();
        assert!(matches!(
            request.operation,
            golem_common::base_model::durable_stream::StreamAttachmentControlOperationV1::Cancel {
                role: StreamCancelRoleV1::InputConsumer,
                reason: StreamCancelReasonV1::GuestDrop,
                ..
            } | golem_common::base_model::durable_stream::StreamAttachmentControlOperationV1::Prepare { .. }
        ));
        owner.poison();
        owner.wait_durable_drained().await;
        Ok(true)
    }

    async fn create_demand(
        &self,
        _owned_agent_id: &OwnedAgentId,
        _method_name: &str,
        _self_created_by: AccountId,
        _self_agent_id: &AgentId,
        _self_env: &[(String, String)],
        _self_stack: golem_common::model::invocation_context::InvocationContextStack,
        _config: Vec<AgentConfigEntryDto>,
        _auth_ctx: &AuthCtx,
    ) -> Result<Box<dyn RpcDemand>, RpcError> {
        unreachable!("test RPC only serves attached stream segments")
    }

    async fn invoke_and_await(
        &self,
        _owned_agent_id: &OwnedAgentId,
        _idempotency_key: Option<IdempotencyKey>,
        _freshness_disposition: InvocationFreshnessDisposition,
        _method_name: String,
        _method_parameters: SchemaValue,
        _self_created_by: AccountId,
        _self_agent_id: &AgentId,
        _self_env: &[(String, String)],
        _self_stack: golem_common::model::invocation_context::InvocationContextStack,
        _config: Vec<AgentConfigEntryDto>,
        _auth_ctx: &AuthCtx,
        _scope_card: Option<golem_common::base_model::card::ScopeCard>,
    ) -> Result<SchemaValue, RpcError> {
        unreachable!("test RPC only serves attached stream segments")
    }

    async fn read_durable_stream_segment(
        &self,
        request: golem_common::base_model::durable_stream::DurableStreamReadRequestV1,
        _auth_ctx: &AuthCtx,
    ) -> Result<Vec<u8>, DurableStreamReadError<RpcError>> {
        let golem_common::base_model::durable_stream::DurableStreamReadRequestV1::AttachedConsumer(
            request,
        ) = request
        else {
            unreachable!("test RPC only serves attached stream segments")
        };
        self.read_requests.lock().await.push(*request.clone());
        if let Some(pending) = self.pending_read.lock().await.take() {
            pending.await.unwrap();
        }
        if let Some(response) = self.scripted_reads.lock().await.pop_front() {
            return response;
        }
        let events = if request.wait_for_events {
            self.producer
                .wait_for_attached_segment(
                    &request.attachment,
                    &request.mapping.handle,
                    113,
                    request.after,
                )
                .await
        } else {
            self.producer
                .read_attached_segment(
                    &request.attachment,
                    &request.mapping.handle,
                    113,
                    request.after,
                    request.through,
                )
                .await
        }
        .map_err(|error| {
            DurableStreamReadError::from_producer(error, |details| RpcError::ProtocolError {
                details,
            })
        })?;
        golem_common::serialization::serialize(&events)
            .map_err(|error| RpcError::ProtocolError {
                details: error.to_string(),
            })
            .map_err(Into::into)
    }

    async fn invoke(
        &self,
        _owned_agent_id: &OwnedAgentId,
        _idempotency_key: Option<IdempotencyKey>,
        _freshness_disposition: InvocationFreshnessDisposition,
        _method_name: String,
        _method_parameters: SchemaValue,
        _self_created_by: AccountId,
        _self_agent_id: &AgentId,
        _self_env: &[(String, String)],
        _self_stack: golem_common::model::invocation_context::InvocationContextStack,
        _config: Vec<AgentConfigEntryDto>,
        _auth_ctx: &AuthCtx,
    ) -> Result<(), RpcError> {
        unreachable!("test RPC only serves attached stream segments")
    }
}

#[test]
#[test_r::timeout("10s")]
async fn routed_attached_reads_retry_only_unavailable_without_changing_the_cursor() {
    let identity = identity();
    let producer = DurableStreamProducer::load(
        Arc::new(TestOplog::default()),
        identity.environment_id,
        identity.agent_id.clone(),
        identity.fingerprint,
        None,
    )
    .await
    .unwrap();
    let handle = producer
        .register(registration(
            &identity,
            StreamRegistrationCoordinateV1::Root {
                invocation_id: identity.invocation.clone(),
                root_kind: StreamRootKindV1::MethodResult,
                recursive_value_path: Vec::new(),
            },
            StreamSourceKindV1::InvocationOutput,
        ))
        .await
        .unwrap()
        .value;
    let attachment = attachment_key(&identity, handle.stream_id);
    let mapping = StreamSessionMappingRecordV1 {
        transport_stream_id: 17,
        handle: handle.clone(),
        role: SessionStreamRoleV1::Input,
    };
    let rpc = Arc::new(AttachedProducerRpc {
        producer: producer.clone(),
        cancellation_owner: None,
        stall_next_cancel: Default::default(),
        scripted_reads: Mutex::default(),
        pending_read: Mutex::default(),
        read_requests: Mutex::default(),
    });
    let source = RoutedAttachedStreamSegmentSource::new(
        rpc.clone(),
        mapping.clone(),
        AuthCtx::System,
        producer,
    );
    let after = Some(StreamOffsetV1::new(OplogIndex::from_u64(41), 3));
    let through = Some(StreamOffsetV1::new(OplogIndex::from_u64(59), 7));
    let expected = vec![CommittedProducerStreamEventV1 {
        stream_id: handle.stream_id,
        producer_sequence: 11,
        offset: StreamOffsetV1::new(OplogIndex::from_u64(43), 4),
        packed_u8_batch_end: None,
        terminal_author: None,
        nested_handles: Vec::new(),
        payload: CommittedProducerStreamEventPayloadV1::PackedU8(37),
    }];
    for wait_for_events in [false, true] {
        let read = || async {
            if wait_for_events {
                source
                    .wait_for_attached_segment(&attachment, &handle, 113, after)
                    .await
            } else {
                source
                    .read_attached_segment(&attachment, &handle, 113, after, through)
                    .await
            }
        };
        rpc.scripted_reads.lock().await.extend([
            Err(DurableStreamReadError::Unavailable),
            Err(DurableStreamReadError::Unavailable),
            Ok(golem_common::serialization::serialize(&expected).unwrap()),
        ]);
        assert_eq!(read().await.unwrap(), expected);
        let requests = std::mem::take(&mut *rpc.read_requests.lock().await);
        assert_eq!(requests.len(), 3);
        for request in requests {
            assert_eq!(request.attachment, attachment);
            assert_eq!(request.mapping, mapping);
            assert_eq!(request.after, after);
            assert_eq!(
                request.through,
                if wait_for_events { None } else { through }
            );
            assert_eq!(request.wait_for_events, wait_for_events);
        }

        rpc.scripted_reads
            .lock()
            .await
            .push_back(Err(DurableStreamReadError::Other(RpcError::Denied {
                details: "access revoked".to_string(),
            })));
        assert!(
            matches!(read().await, Err(DurableStreamProducerError::Oplog(message)) if message.contains("access revoked"))
        );
        assert_eq!(rpc.read_requests.lock().await.len(), 1);
        rpc.read_requests.lock().await.clear();

        rpc.scripted_reads
            .lock()
            .await
            .push_back(Err(DurableStreamReadError::Unavailable));
        let mut pending = Box::pin(read());
        assert!(futures::poll!(pending.as_mut()).is_pending());
        assert_eq!(rpc.read_requests.lock().await.len(), 1);
        drop(pending);
        tokio::time::sleep(Duration::from_millis(150)).await;
        assert_eq!(rpc.read_requests.lock().await.len(), 1);
        rpc.read_requests.lock().await.clear();

        let (sender, receiver) = tokio::sync::oneshot::channel();
        *rpc.pending_read.lock().await = Some(receiver);
        let mut pending = Box::pin(read());
        assert!(futures::poll!(pending.as_mut()).is_pending());
        assert!(!sender.is_closed());
        assert_eq!(rpc.read_requests.lock().await.len(), 1);
        drop(pending);
        assert!(sender.is_closed());
        rpc.read_requests.lock().await.clear();
    }
}

fn union_with_stream_in_second_branch() -> SchemaType {
    let field = |name: &str, body| NamedFieldType {
        name: name.to_string(),
        body,
        metadata: Default::default(),
    };
    SchemaType::union(UnionSpec {
        branches: vec![
            UnionBranch {
                tag: "plain".to_string(),
                body: SchemaType::record(vec![field("kind", SchemaType::string())]),
                discriminator: DiscriminatorRule::FieldEquals(FieldDiscriminator {
                    field_name: "kind".to_string(),
                    literal: Some("plain".to_string()),
                }),
                metadata: Default::default(),
            },
            UnionBranch {
                tag: "stream".to_string(),
                body: SchemaType::record(vec![
                    field("kind", SchemaType::string()),
                    field("values", SchemaType::stream(Some(SchemaType::u32()))),
                ]),
                discriminator: DiscriminatorRule::FieldEquals(FieldDiscriminator {
                    field_name: "kind".to_string(),
                    literal: Some("stream".to_string()),
                }),
                metadata: Default::default(),
            },
        ],
    })
}

fn stream_union_value(stream: SchemaValueStream) -> SchemaValue {
    SchemaValue::Union(UnionValuePayload {
        tag: "stream".to_string(),
        body: Box::new(SchemaValue::Record {
            fields: vec![
                SchemaValue::String("stream".to_string()),
                SchemaValue::Stream(stream),
            ],
        }),
    })
}

#[test]
fn late_input_is_discarded_after_the_session_or_consumer_terminates() {
    let session_key = identity().invocation;
    assert!(discards_input_after_terminal(
        &DurableStreamProducerError::SessionFinished(session_key.clone()),
        &session_key,
    ));
    assert!(discards_input_after_terminal(
        &DurableStreamProducerError::ClosedByOtherProducer,
        &session_key,
    ));
    assert!(discards_input_after_terminal(
        &DurableStreamProducerError::FencedByTerminal(
            CommittedProducerStreamEventPayloadV1::Cancel {
                role: StreamCancelRoleV1::InputConsumer,
                reason: StreamCancelReasonV1::GuestDrop,
                details: None,
            },
        ),
        &session_key,
    ));
    assert!(!discards_input_after_terminal(
        &DurableStreamProducerError::FencedByTerminal(CommittedProducerStreamEventPayloadV1::End(
            StreamEndResultV1::Ok
        ),),
        &session_key,
    ));
}

async fn backpressured_session_input() -> (
    Arc<DurableStreamProducer>,
    DurableSessionStreams,
    DurableCatchUpReader,
    DurableStreamHandleV1,
) {
    let identity = identity();
    let oplog = Arc::new(TestOplog::default());
    let producer = DurableStreamProducer::load(
        oplog.clone(),
        identity.environment_id,
        identity.agent_id.clone(),
        identity.fingerprint,
        Some(1),
    )
    .await
    .unwrap();
    let handle = producer
        .register(registration(
            &identity,
            StreamRegistrationCoordinateV1::Root {
                invocation_id: identity.invocation.clone(),
                root_kind: StreamRootKindV1::MethodInput,
                recursive_value_path: Vec::new(),
            },
            StreamSourceKindV1::AgentHostedInput,
        ))
        .await
        .unwrap()
        .value;
    let attachment_id = AttachmentId::primary(
        identity.environment_id,
        &identity.agent_id,
        &identity.invocation.idempotency_key,
    )
    .unwrap();
    let attempt_id = AttemptId::fresh();
    let pending_invocation_oplog_index = append_prepared_pending(
        producer.as_ref(),
        oplog.as_ref(),
        &identity,
        attachment_id,
        attempt_id,
        &handle,
        SessionStreamRoleV1::Input,
    )
    .await;
    producer
        .append_session_record(StreamSessionRecordV1::Attached(
            StreamSessionAttachedRecordV1 {
                format_version: DURABLE_STREAM_FORMAT_VERSION,
                session_key: identity.invocation.clone(),
                attachment_id,
                attempt_id,
                epoch: 1,
                pending_invocation_oplog_index,
            },
        ))
        .await
        .unwrap();
    let streams = DurableSessionStreams::new(
        producer.clone(),
        oplog.clone(),
        identity.invocation,
        [(7, handle.clone(), SessionStreamRoleV1::Input)],
    )
    .with_consumer_journal(Arc::new(TestConsumerJournal(oplog)));
    let reader = producer.catch_up(handle.clone(), None).await.unwrap();
    producer
        .write_items(handle.stream_id, 0, StreamItemsPayloadV1::PackedU8(vec![1]))
        .await
        .unwrap();
    (producer, streams, reader, handle)
}

async fn wait_for_terminal_commit(producer: &DurableStreamProducer, stream_id: StreamId) {
    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            if producer
                .input_high_water(stream_id)
                .await
                .unwrap()
                .is_some_and(|high_water| high_water.terminal)
            {
                return;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("stream terminal was not committed");
}

#[test]
#[test_r::timeout("15s")]
async fn foreign_cancellation_recovery_applies_persisted_intent_without_delete_retry() {
    let local = identity();
    let mut remote = identity();
    remote.agent_id.agent_id.push_str("-remote");
    remote.invocation.callee = remote.agent_id.clone();
    let remote_oplog = Arc::new(TestOplog::default());
    let remote_producer = DurableStreamProducer::load(
        remote_oplog.clone(),
        remote.environment_id,
        remote.agent_id.clone(),
        remote.fingerprint,
        None,
    )
    .await
    .unwrap();
    let handle = remote_producer
        .register(registration(
            &remote,
            StreamRegistrationCoordinateV1::Root {
                invocation_id: remote.invocation.clone(),
                root_kind: StreamRootKindV1::MethodResult,
                recursive_value_path: Vec::new(),
            },
            StreamSourceKindV1::InvocationOutput,
        ))
        .await
        .unwrap()
        .value;
    let oplog = Arc::new(TestOplog::default());
    let producer = DurableStreamProducer::load(
        oplog.clone(),
        local.environment_id,
        local.agent_id.clone(),
        local.fingerprint,
        None,
    )
    .await
    .unwrap();
    producer
        .append_session_record(StreamSessionRecordV1::Mapping(
            StreamSessionMappingUpdateRecordV1 {
                format_version: DURABLE_STREAM_FORMAT_VERSION,
                session_key: local.invocation.clone(),
                mapping: StreamSessionMappingRecordV1 {
                    transport_stream_id: 17,
                    handle: handle.clone(),
                    role: SessionStreamRoleV1::Output,
                },
            },
        ))
        .await
        .unwrap();
    producer
        .append_session_record(StreamSessionRecordV1::ConsumerCancelIntent(
            StreamConsumerCancelIntentRecordV1 {
                format_version: DURABLE_STREAM_FORMAT_VERSION,
                session_key: local.invocation.clone(),
                stream_id: handle.stream_id,
                epoch: 3,
                role: StreamCancelRoleV1::OutputConsumer,
                reason: StreamCancelReasonV1::GuestDrop,
                details: Some("persisted remote cancellation".into()),
            },
        ))
        .await
        .unwrap();
    drop(producer);
    let producer = DurableStreamProducer::load(
        oplog.clone(),
        local.environment_id,
        local.agent_id,
        local.fingerprint,
        None,
    )
    .await
    .unwrap();
    let streams = DurableSessionStreams::new(producer, oplog, local.invocation, [])
        .with_auth_ctx(AuthCtx::System)
        .with_rpc(Arc::new(AttachedProducerRpc {
            producer: remote_producer,
            cancellation_owner: None,
            stall_next_cancel: true.into(),
            scripted_reads: Mutex::default(),
            pending_read: Mutex::default(),
            read_requests: Mutex::default(),
        }));
    let before = remote_oplog.current_oplog_index().await;
    let error = streams
        .reconcile_foreign_cancellation_intents(Duration::from_millis(10))
        .await
        .unwrap_err();
    assert_eq!(error, "RecoveryRequired");
    assert_eq!(remote_oplog.current_oplog_index().await, before);
    assert!(
        streams
            .current_control_metadata()
            .await
            .unwrap()
            .has_cancellation_intents()
    );
    streams
        .producer
        .run_lifecycle(0, |_| async { Ok::<_, String>(()) })
        .await
        .expect("remote timeout must not poison the local producer");
    streams
        .reconcile_foreign_cancellation_intents(Duration::from_secs(1))
        .await
        .unwrap();
    let after = remote_oplog.current_oplog_index().await;
    assert_eq!(after, before.next());
    let OplogEntry::StreamCancel { record, .. } = remote_oplog.read(after).await else {
        panic!("remote cancellation was not committed");
    };
    let record = remote_oplog.download_payload(record).await.unwrap();
    assert_eq!(record.stream_id, handle.stream_id);
    assert_eq!(record.role, StreamCancelRoleV1::OutputConsumer);
    assert_eq!(
        record.details.as_deref(),
        Some("persisted remote cancellation")
    );
    assert!(
        !streams
            .current_control_metadata()
            .await
            .unwrap()
            .has_cancellation_intents()
    );
    let local_after = streams.oplog.current_oplog_index().await;
    streams
        .reconcile_foreign_cancellation_intents(Duration::from_secs(1))
        .await
        .unwrap();
    assert_eq!(remote_oplog.current_oplog_index().await, after);
    assert_eq!(streams.oplog.current_oplog_index().await, local_after);
}

#[test]
#[test_r::timeout("15s")]
async fn foreign_cancellation_can_drain_its_local_owner_from_the_rpc_callback() {
    retirement_from_foreign_rpc(false).await;
}

#[test]
#[test_r::timeout("15s")]
async fn foreign_preparation_releases_local_owner_when_rpc_requests_retirement() {
    retirement_from_foreign_rpc(true).await;
}

async fn retirement_from_foreign_rpc(prepare: bool) {
    let local = identity();
    let mut remote = identity();
    remote.agent_id.agent_id.push_str("-remote");
    remote.invocation.callee = remote.agent_id.clone();
    let remote_producer = DurableStreamProducer::load(
        Arc::new(TestOplog::default()),
        remote.environment_id,
        remote.agent_id.clone(),
        remote.fingerprint,
        None,
    )
    .await
    .unwrap();
    let handle = remote_producer
        .register(registration(
            &remote,
            StreamRegistrationCoordinateV1::Root {
                invocation_id: remote.invocation.clone(),
                root_kind: StreamRootKindV1::MethodResult,
                recursive_value_path: Vec::new(),
            },
            StreamSourceKindV1::InvocationOutput,
        ))
        .await
        .unwrap()
        .value;
    let oplog = Arc::new(TestOplog::default());
    let producer = DurableStreamProducer::load(
        oplog.clone(),
        local.environment_id,
        local.agent_id.clone(),
        local.fingerprint,
        None,
    )
    .await
    .unwrap();
    let attachment_id = AttachmentId::primary(
        local.environment_id,
        &local.agent_id,
        &local.invocation.idempotency_key,
    )
    .unwrap();
    let attempt_id = AttemptId::fresh();
    let pending_invocation_oplog_index = append_prepared_pending(
        &producer,
        &oplog,
        &local,
        attachment_id,
        attempt_id,
        &handle,
        SessionStreamRoleV1::Input,
    )
    .await;
    producer
        .append_session_record(StreamSessionRecordV1::Attached(
            StreamSessionAttachedRecordV1 {
                format_version: DURABLE_STREAM_FORMAT_VERSION,
                session_key: local.invocation.clone(),
                attachment_id,
                attempt_id,
                epoch: 1,
                pending_invocation_oplog_index,
            },
        ))
        .await
        .unwrap();
    let streams = DurableSessionStreams::new(
        producer.clone(),
        oplog.clone(),
        local.invocation.clone(),
        [(7, handle.clone(), SessionStreamRoleV1::Input)],
    )
    .with_consumer_journal(Arc::new(TestConsumerJournal(oplog.clone())))
    .with_attachment(1, attempt_id)
    .with_rpc(Arc::new(AttachedProducerRpc {
        producer: remote_producer,
        cancellation_owner: Some(producer.clone()),
        stall_next_cancel: Default::default(),
        scripted_reads: Mutex::default(),
        pending_read: Mutex::default(),
        read_requests: Mutex::default(),
    }))
    .with_auth_ctx(AuthCtx::System);
    if prepare {
        let error = tokio::time::timeout(
            Duration::from_secs(5),
            streams.prepare_foreign_mapping(
                StreamSessionMappingRecordV1 {
                    transport_stream_id: 7,
                    handle,
                    role: SessionStreamRoleV1::Input,
                },
                1,
            ),
        )
        .await
        .expect("remote preparation prevented local retirement")
        .unwrap_err();
        assert_eq!(
            error,
            DurableStreamProducerError::RecoveryRequired.to_string()
        );
        producer.wait_durable_drained().await;
        assert!(streams.session_lock.try_lock().is_ok());
        return;
    }
    tokio::time::timeout(
        Duration::from_secs(5),
        streams.cancel_stream(
            7,
            StreamCancelRoleV1::InputConsumer,
            StreamCancelReasonV1::GuestDrop,
            Some("reader dropped".into()),
            None,
        ),
    )
    .await
    .expect("RPC callback could not drain local producer")
    .unwrap();
    assert!(matches!(
        producer.ensure_healthy(),
        Err(DurableStreamProducerError::RecoveryRequired)
    ));
    let restarted = DurableStreamProducer::load(
        oplog.clone(),
        local.environment_id,
        local.agent_id,
        local.fingerprint,
        None,
    )
    .await
    .unwrap();
    let recovered = DurableSessionStreams::new(
        restarted,
        oplog,
        local.invocation,
        [(7, handle.clone(), SessionStreamRoleV1::Input)],
    );
    let metadata = recovered.current_control_metadata().await.unwrap();
    let intent = metadata.cancel_intents.get(&handle.stream_id).unwrap();
    assert_eq!(intent.epoch, 1);
    assert_eq!(intent.role, StreamCancelRoleV1::InputConsumer);
    assert_eq!(intent.reason, StreamCancelReasonV1::GuestDrop);
    assert_eq!(intent.details.as_deref(), Some("reader dropped"));
    assert!(streams.session_lock.try_lock().is_ok());
}

#[test]
async fn completed_output_reconstruction_does_not_require_an_attachment() {
    let identity = identity();
    let oplog = Arc::new(TestOplog::default());
    let producer = DurableStreamProducer::load(
        oplog.clone(),
        identity.environment_id,
        identity.agent_id.clone(),
        identity.fingerprint,
        None,
    )
    .await
    .unwrap();
    let handle = producer
        .register(registration(
            &identity,
            StreamRegistrationCoordinateV1::Root {
                invocation_id: identity.invocation.clone(),
                root_kind: StreamRootKindV1::MethodResult,
                recursive_value_path: Vec::new(),
            },
            StreamSourceKindV1::InvocationOutput,
        ))
        .await
        .unwrap()
        .value;
    let streams = DurableSessionStreams::new(
        producer.clone(),
        oplog.clone(),
        identity.invocation.clone(),
        [],
    );
    assert!(
        futures::poll!(std::pin::pin!(streams.wait_for_active_attachment(&handle))).is_pending(),
        "an open output still requires an attachment"
    );
    producer
        .end(handle.stream_id, 0, StreamEndResultV1::Ok)
        .await
        .unwrap();
    drop(streams);
    drop(producer);
    let producer = DurableStreamProducer::load(
        oplog.clone(),
        identity.environment_id,
        identity.agent_id,
        identity.fingerprint,
        None,
    )
    .await
    .unwrap();
    let streams = DurableSessionStreams::new(producer, oplog, identity.invocation, []);
    tokio::time::timeout(
        Duration::from_secs(1),
        streams.wait_for_active_attachment(&handle),
    )
    .await
    .expect("a terminal output must reconstruct without a consumer attachment")
    .unwrap();
}

#[test]
async fn local_cancellation_intent_recovers_without_retry_and_preserves_terminal() {
    for already_ended in [false, true] {
        let identity = identity();
        let oplog = Arc::new(TestOplog::default());
        let producer = DurableStreamProducer::load(
            oplog.clone(),
            identity.environment_id,
            identity.agent_id.clone(),
            identity.fingerprint,
            None,
        )
        .await
        .unwrap();
        let handle = producer
            .register(registration(
                &identity,
                StreamRegistrationCoordinateV1::Root {
                    invocation_id: identity.invocation.clone(),
                    root_kind: StreamRootKindV1::MethodResult,
                    recursive_value_path: Vec::new(),
                },
                StreamSourceKindV1::InvocationOutput,
            ))
            .await
            .unwrap()
            .value;
        producer
            .append_session_record(StreamSessionRecordV1::Mapping(
                StreamSessionMappingUpdateRecordV1 {
                    format_version: DURABLE_STREAM_FORMAT_VERSION,
                    session_key: identity.invocation.clone(),
                    mapping: StreamSessionMappingRecordV1 {
                        transport_stream_id: 17,
                        handle: handle.clone(),
                        role: SessionStreamRoleV1::Output,
                    },
                },
            ))
            .await
            .unwrap();
        if already_ended {
            producer
                .end(handle.stream_id, 0, StreamEndResultV1::Ok)
                .await
                .unwrap();
        }
        producer
            .append_session_record(StreamSessionRecordV1::ConsumerCancelIntent(
                StreamConsumerCancelIntentRecordV1 {
                    format_version: DURABLE_STREAM_FORMAT_VERSION,
                    session_key: identity.invocation.clone(),
                    stream_id: handle.stream_id,
                    epoch: 7,
                    role: StreamCancelRoleV1::OutputConsumer,
                    reason: StreamCancelReasonV1::GuestDrop,
                    details: Some("consumer gone before restart".into()),
                },
            ))
            .await
            .unwrap();
        let before = oplog.current_oplog_index().await;
        drop(producer);
        let producer = DurableStreamProducer::load(
            oplog.clone(),
            identity.environment_id,
            identity.agent_id,
            identity.fingerprint,
            None,
        )
        .await
        .unwrap();
        let recovered =
            DurableSessionStreams::new(producer, oplog.clone(), identity.invocation, []);
        let key = recovered.attachment_key(&handle, 7).unwrap();
        let mapping = StreamSessionMappingRecordV1 {
            transport_stream_id: 17,
            handle: handle.clone(),
            role: SessionStreamRoleV1::Output,
        };
        let mut metadata = recovered.current_control_metadata().await.unwrap().clone();
        metadata.topology_epoch = Some(9);
        let intent = metadata.cancel_intents[&handle.stream_id].clone();
        assert!(
            metadata
                .has_committed_cancellation(&key, &mapping, &intent)
                .unwrap()
        );
        let mut changed = intent.clone();
        changed.details = None;
        assert!(
            !metadata
                .has_committed_cancellation(&key, &mapping, &changed)
                .unwrap()
        );
        let mut stale_key = key.clone();
        stale_key.epoch = 6;
        assert!(
            !metadata
                .has_committed_cancellation(&stale_key, &mapping, &intent)
                .unwrap()
        );
        let mut wrong_mapping = mapping.clone();
        wrong_mapping.transport_stream_id = 18;
        assert!(
            !metadata
                .has_committed_cancellation(&key, &wrong_mapping, &intent)
                .unwrap()
        );
        metadata.cancel_intents.clear();
        assert!(
            !metadata
                .has_committed_cancellation(&key, &mapping, &intent)
                .unwrap()
        );
        recovered
            .reconcile_local_cancellation_intents()
            .await
            .unwrap();
        let after = oplog.current_oplog_index().await;
        assert_eq!(
            after.as_u64(),
            before.as_u64() + u64::from(!already_ended) + 1
        );
        let OplogEntry::StreamSession { record, .. } = oplog.read(after).await else {
            panic!("missing cancellation applied receipt");
        };
        let StreamSessionRecordV1::ConsumerCancelApplied(receipt) =
            oplog.download_payload(record).await.unwrap()
        else {
            panic!("unexpected cancellation applied receipt");
        };
        assert_eq!(receipt.intent, intent);
        assert!(
            !recovered
                .current_control_metadata()
                .await
                .unwrap()
                .has_cancellation_intents()
        );
        if !already_ended {
            let OplogEntry::StreamCancel { record, .. } = oplog.read(before.next()).await else {
                panic!("missing recovered cancellation terminal");
            };
            let record = oplog.download_payload(record).await.unwrap();
            assert_eq!(record.stream_id, handle.stream_id);
            assert_eq!(record.role, StreamCancelRoleV1::OutputConsumer);
            assert_eq!(
                record.details.as_deref(),
                Some("consumer gone before restart")
            );
        }
        recovered
            .reconcile_local_cancellation_intents()
            .await
            .unwrap();
        assert_eq!(oplog.current_oplog_index().await, after);
    }
}

#[test]
#[test_r::timeout("15s")]
async fn session_cancellation_retains_history_and_is_idempotent_under_backpressure() {
    let (producer, streams, mut reader, handle) = backpressured_session_input().await;
    let identity = identity();
    let mut outputs = Vec::new();
    for index in 0..2 {
        let output = producer
            .register(registration(
                &identity,
                StreamRegistrationCoordinateV1::Root {
                    invocation_id: identity.invocation.clone(),
                    root_kind: StreamRootKindV1::MethodResult,
                    recursive_value_path: vec![StreamValuePathStepV1::RecordField(index)],
                },
                StreamSourceKindV1::InvocationOutput,
            ))
            .await
            .unwrap()
            .value;
        streams
            .append_record(StreamSessionRecordV1::Mapping(
                StreamSessionMappingUpdateRecordV1 {
                    format_version: DURABLE_STREAM_FORMAT_VERSION,
                    session_key: streams.session_key.clone(),
                    mapping: StreamSessionMappingRecordV1 {
                        transport_stream_id: 17 + u64::from(index),
                        handle: output.clone(),
                        role: SessionStreamRoleV1::Output,
                    },
                },
            ))
            .await;
        if index == 1 {
            producer
                .end(output.stream_id, 0, StreamEndResultV1::Ok)
                .await
                .unwrap();
        }
        outputs.push(output);
    }
    assert!(streams.cancel_session_streams().await.unwrap());
    let committed = streams.oplog.current_oplog_index().await;
    let metadata = streams.current_control_metadata().await.unwrap();
    assert!(metadata.cancellation_requested);
    let intent = metadata.cancel_intents.get(&handle.stream_id).unwrap();
    assert_eq!(intent.role, StreamCancelRoleV1::InputProducer);
    assert_eq!(intent.reason, StreamCancelReasonV1::Cancelled);
    assert_eq!(intent.epoch, 1);
    drop(metadata);
    assert!(streams.cancel_session_streams().await.unwrap());
    assert_eq!(streams.oplog.current_oplog_index().await, committed);
    let item = reader.next().await.unwrap().unwrap();
    assert_eq!(
        item.payload,
        CommittedProducerStreamEventPayloadV1::PackedU8(1)
    );
    let terminal = reader.next().await.unwrap().unwrap();
    assert_eq!(terminal.producer_sequence, 1);
    assert_eq!(
        terminal.payload,
        CommittedProducerStreamEventPayloadV1::Cancel {
            role: StreamCancelRoleV1::InputProducer,
            reason: StreamCancelReasonV1::Cancelled,
            details: None,
        }
    );
    assert!(reader.next().await.unwrap().is_none());
    assert!(
        producer
            .input_high_water(handle.stream_id)
            .await
            .unwrap()
            .unwrap()
            .terminal
    );

    for (index, output) in outputs.into_iter().enumerate() {
        let mut output_reader = producer.catch_up(output, None).await.unwrap();
        let terminal = output_reader.next().await.unwrap().unwrap();
        assert_eq!(
            terminal.payload,
            if index == 0 {
                CommittedProducerStreamEventPayloadV1::Cancel {
                    role: StreamCancelRoleV1::OutputConsumer,
                    reason: StreamCancelReasonV1::Cancelled,
                    details: None,
                }
            } else {
                CommittedProducerStreamEventPayloadV1::End(StreamEndResultV1::Ok)
            }
        );
        assert!(output_reader.next().await.unwrap().is_none());
    }

    let mut unknown_key = streams.session_key.clone();
    unknown_key.idempotency_key =
        golem_common::model::IdempotencyKey::new("unknown-session".into());
    let unknown = DurableSessionStreams::new(producer, streams.oplog.clone(), unknown_key, []);
    assert!(!unknown.cancel_session_streams().await.unwrap());
    assert_eq!(streams.oplog.current_oplog_index().await, committed);
}

#[test]
#[test_r::timeout("15s")]
async fn late_output_cancellation_selects_fields_and_preserves_replay_drains() {
    for target in [
        None,
        Some(SessionStreamRoleV1::Input),
        Some(SessionStreamRoleV1::Output),
    ] {
        let (producer, streams, reader, _) = backpressured_session_input().await;
        drop(reader);
        match target {
            None => {
                streams.cancel_session_streams().await.unwrap();
            }
            Some(role) => {
                streams
                    .append_record(StreamSessionRecordV1::Tombstoned(
                        StreamSlotTombstonedRecordV1 {
                            format_version: DURABLE_STREAM_FORMAT_VERSION,
                            session_key: streams.session_key.clone(),
                            slot: "right".into(),
                            role,
                        },
                    ))
                    .await
            }
        }
        let root = SchemaType::record(
            ["left", "right"]
                .into_iter()
                .map(|name| NamedFieldType {
                    name: name.into(),
                    body: SchemaType::stream(Some(SchemaType::u8())),
                    metadata: Default::default(),
                })
                .collect(),
        );
        for replay in [false, true] {
            let (left_sender, left) = test_output_stream_pair(1).unwrap();
            let (right_sender, right) = test_output_stream_pair(1).unwrap();
            let value = SchemaValue::Record {
                fields: vec![
                    SchemaValue::Stream(SchemaValueStream::from_host_endpoint(left)),
                    SchemaValue::Stream(SchemaValueStream::from_host_endpoint(right)),
                ],
            };
            let session = streams.clone();
            let root = root.clone();
            let (_, drains) = producer
                .run_lifecycle(0, move |_| async move {
                    session
                        .materialize_result_owned(
                            value,
                            SchemaGraph::anonymous(root.clone()),
                            root,
                            ComponentRevision::INITIAL,
                        )
                        .await
                })
                .await
                .unwrap();
            let expected_cancelled = match target {
                None => 2,
                Some(SessionStreamRoleV1::Output) => 1,
                _ => 0,
            };
            assert_eq!(
                drains.len(),
                if replay { 2 } else { 2 - expected_cancelled }
            );
            let result = streams.remote_result_record().await.unwrap().unwrap();
            for (position, handle) in result.output_streams.iter().enumerate() {
                assert_eq!(
                    producer
                        .input_high_water(handle.stream_id)
                        .await
                        .unwrap()
                        .is_some_and(|watermark| watermark.terminal),
                    target.is_none()
                        || (target == Some(SessionStreamRoleV1::Output) && position == 1)
                );
            }
            drop(drains);
            drop((left_sender, right_sender));
        }
    }
}

#[test]
#[test_r::timeout("15s")]
async fn cancelled_forwarded_result_persists_intent_without_remote_activation() {
    let (producer, streams, reader, _) = backpressured_session_input().await;
    drop(reader);
    streams.cancel_session_streams().await.unwrap();
    let mut remote = identity();
    remote.agent_id.agent_id.push_str("-remote");
    remote.invocation.callee = remote.agent_id.clone();
    let remote_oplog = Arc::new(TestOplog::default());
    let remote_producer = DurableStreamProducer::load(
        remote_oplog.clone(),
        remote.environment_id,
        remote.agent_id.clone(),
        remote.fingerprint,
        None,
    )
    .await
    .unwrap();
    let root = SchemaType::stream(Some(SchemaType::u8()));
    let graph = SchemaGraph::anonymous(root.clone());
    let mut request = registration(
        &remote,
        StreamRegistrationCoordinateV1::Root {
            invocation_id: remote.invocation.clone(),
            root_kind: StreamRootKindV1::MethodResult,
            recursive_value_path: vec![],
        },
        StreamSourceKindV1::InvocationOutput,
    );
    request.element_schema_fingerprint =
        schema_fingerprint_v1(&graph, Some(&SchemaType::u8())).unwrap();
    let handle = remote_producer.register(request).await.unwrap().value;
    let before = remote_oplog.current_oplog_index().await;
    for _ in 0..2 {
        let result = SchemaValue::Stream(SchemaValueStream::from_host_endpoint(
            ForwardedDurableInput {
                handle: handle.clone(),
            },
        ));
        streams
            .materialize_result(result, &graph, &root, ComponentRevision::INITIAL)
            .await
            .unwrap();
    }
    let metadata = streams.current_control_metadata().await.unwrap();
    let intent = metadata.cancel_intents.get(&handle.stream_id).unwrap();
    assert_eq!(intent.role, StreamCancelRoleV1::OutputConsumer);
    assert_eq!(intent.reason, StreamCancelReasonV1::Cancelled);
    assert_eq!(metadata.topologies.len(), 0);
    assert!(
        metadata
            .persisted_mappings
            .iter()
            .any(|(_, saved, role)| saved == &handle && *role == SessionStreamRoleV1::Output)
    );
    assert_eq!(remote_oplog.current_oplog_index().await, before);
    drop(metadata);
    assert_eq!(
        streams
            .remote_result_record()
            .await
            .unwrap()
            .unwrap()
            .output_streams,
        vec![handle]
    );
    producer
        .run_lifecycle(0, |_| async { Ok::<_, String>(()) })
        .await
        .unwrap();
}

#[test]
#[test_r::timeout("15s")]
async fn slot_tombstone_persists_without_cancelling_other_slots() {
    for pending_output in [false, true] {
        let (producer, streams, reader, handle) = backpressured_session_input().await;
        let slot = if pending_output { "$result" } else { "input" };
        for first in [true, false] {
            let session = streams.clone();
            let stream = (!pending_output).then(|| (handle.clone(), SessionStreamRoleV1::Input));
            let before = streams.oplog.current_oplog_index().await;
            let changed = producer
                .run_lifecycle(0, move |_| async move {
                    let lock = session.producer.session_lock(&session.session_key);
                    let guard = lock.lock().await;
                    session
                        .tombstone_slot_owned(slot.into(), stream, guard)
                        .await
                })
                .await
                .unwrap();
            assert_eq!(changed, first);
            if !first {
                assert_eq!(streams.oplog.current_oplog_index().await, before);
            }
        }
        assert_eq!(
            producer
                .input_high_water(handle.stream_id)
                .await
                .unwrap()
                .unwrap()
                .terminal,
            !pending_output
        );
        let oplog = streams.oplog.clone();
        let key = streams.session_key.clone();
        drop(reader);
        drop(streams);
        drop(producer);
        let identity = identity();
        let recovered_producer = DurableStreamProducer::load(
            oplog.clone(),
            identity.environment_id,
            identity.agent_id,
            identity.fingerprint,
            None,
        )
        .await
        .unwrap();
        let recovered = DurableSessionStreams::new(recovered_producer, oplog, key, []);
        let metadata = recovered.current_control_metadata().await.unwrap();
        assert_eq!(
            metadata.tombstoned_slots,
            HashMap::from([(
                slot.to_string(),
                if pending_output {
                    SessionStreamRoleV1::Output
                } else {
                    SessionStreamRoleV1::Input
                }
            )])
        );
        assert!(!metadata.cancellation_requested);
        assert_eq!(metadata.cancel_intents.len(), usize::from(!pending_output));
    }
}

#[test]
async fn local_cancellation_recovery_does_not_wait_for_live_reader_capacity() {
    let (producer, streams, reader, handle) = backpressured_session_input().await;
    producer
        .append_session_record(StreamSessionRecordV1::ConsumerCancelIntent(
            StreamConsumerCancelIntentRecordV1 {
                format_version: DURABLE_STREAM_FORMAT_VERSION,
                session_key: streams.session_key.clone(),
                stream_id: handle.stream_id,
                epoch: 1,
                role: StreamCancelRoleV1::InputConsumer,
                reason: StreamCancelReasonV1::GuestDrop,
                details: None,
            },
        ))
        .await
        .unwrap();
    tokio::time::timeout(
        Duration::from_secs(1),
        streams.reconcile_local_cancellation_intents(),
    )
    .await
    .expect("recovery waited for a backpressured live reader")
    .unwrap();
    wait_for_terminal_commit(&producer, handle.stream_id).await;
    assert!(streams.session_lock.try_lock().is_ok());
    drop(reader);
}

#[test]
async fn local_cancellation_releases_session_lock_after_commit_before_live_publication() {
    let (producer, streams, mut reader, handle) = backpressured_session_input().await;
    let cancellation = tokio::spawn({
        let streams = streams.clone();
        async move {
            streams
                .cancel_stream(
                    7,
                    StreamCancelRoleV1::InputConsumer,
                    StreamCancelReasonV1::GuestDrop,
                    Some("guest dropped its durable readable stream end".to_string()),
                    None,
                )
                .await
        }
    });

    wait_for_terminal_commit(&producer, handle.stream_id).await;
    assert!(!cancellation.is_finished());
    let session_guard = tokio::time::timeout(Duration::from_secs(1), streams.session_lock.lock())
        .await
        .expect(
            "local cancellation must release session ownership after its terminal is committed",
        );
    drop(session_guard);

    assert_eq!(reader.next().await.unwrap().unwrap().producer_sequence, 0);
    cancellation.await.unwrap().unwrap();
    let terminal = reader.next().await.unwrap().unwrap();
    assert_eq!(terminal.producer_sequence, 1);
    assert!(matches!(
        terminal.payload,
        CommittedProducerStreamEventPayloadV1::Cancel {
            role: StreamCancelRoleV1::InputConsumer,
            reason: StreamCancelReasonV1::GuestDrop,
            ..
        }
    ));
    assert!(reader.next().await.unwrap().is_none());
}

#[test]
async fn root_union_stream_coordinates_use_the_selected_branch_and_survive_reload() {
    let identity = identity();
    let oplog = Arc::new(TestOplog::default());
    let producer = DurableStreamProducer::load(
        oplog.clone(),
        identity.environment_id,
        identity.agent_id.clone(),
        identity.fingerprint,
        None,
    )
    .await
    .unwrap();
    let streams = DurableSessionStreams::new(
        producer.clone(),
        oplog.clone(),
        identity.invocation.clone(),
        [],
    )
    .with_consumer_journal(Arc::new(TestConsumerJournal(oplog.clone())));
    let root = union_with_stream_in_second_branch();
    let graph = SchemaGraph::anonymous(root.clone());
    let path = vec![
        StreamValuePathStepV1::UnionBranch(1),
        StreamValuePathStepV1::RecordField(1),
    ];

    let (input_consumer, input_stream) = output_stream_pair(4, Arc::new(|| false)).unwrap();
    streams
        .materialize_agent_input(
            &stream_union_value(input_stream),
            &graph,
            &root,
            ComponentRevision::INITIAL,
        )
        .await
        .unwrap();
    let input_coordinate = StreamRegistrationCoordinateV1::Root {
        invocation_id: identity.invocation.clone(),
        root_kind: StreamRootKindV1::MethodInput,
        recursive_value_path: path.clone(),
    };
    let input_handle = producer
        .handle_for_coordinate(&input_coordinate)
        .await
        .unwrap()
        .expect("the caller input stream must use union branch 1");
    drop(input_consumer);

    let (output_consumer, output_stream) = output_stream_pair(4, Arc::new(|| false)).unwrap();
    drop(output_consumer);
    streams
        .materialize_result(
            stream_union_value(output_stream),
            &graph,
            &root,
            ComponentRevision::INITIAL,
        )
        .await
        .unwrap();
    let result_coordinate = StreamRegistrationCoordinateV1::Root {
        invocation_id: identity.invocation.clone(),
        root_kind: StreamRootKindV1::MethodResult,
        recursive_value_path: path,
    };
    let result_handle = producer
        .handle_for_coordinate(&result_coordinate)
        .await
        .unwrap()
        .expect("the callee result stream must use union branch 1");

    let reloaded = DurableStreamProducer::load(
        oplog,
        identity.environment_id,
        identity.agent_id,
        identity.fingerprint,
        None,
    )
    .await
    .unwrap();
    assert_eq!(
        reloaded
            .handle_for_coordinate(&input_coordinate)
            .await
            .unwrap(),
        Some(input_handle)
    );
    assert_eq!(
        reloaded
            .handle_for_coordinate(&result_coordinate)
            .await
            .unwrap(),
        Some(result_handle)
    );
}

#[async_trait::async_trait]
impl DurableStreamConsumerJournal for RecordingConsumerJournal {
    async fn commit(&self) -> Result<(), String> {
        self.oplog.commit(CommitLevel::Always).await;
        self.commits.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    async fn committed_finished_index(
        &self,
        _session: &StreamSessionKeyV1,
    ) -> Result<Option<OplogIndex>, String> {
        Ok(None)
    }
}

#[test]
async fn owned_tail_backlog_uses_source_history() {
    /*
    async fn owned_live_tail_journal_lag_counts_committed_source_events() {
        */
    let identity = identity();
    let oplog = Arc::new(TestOplog::default());
    let producer = DurableStreamProducer::load(
        oplog,
        identity.environment_id,
        identity.agent_id.clone(),
        identity.fingerprint,
        None,
    )
    .await
    .unwrap();
    let handle = producer
        .register(registration(
            &identity,
            StreamRegistrationCoordinateV1::Root {
                invocation_id: identity.invocation.clone(),
                root_kind: StreamRootKindV1::MethodInput,
                recursive_value_path: Vec::new(),
            },
            StreamSourceKindV1::AgentHostedInput,
        ))
        .await
        .unwrap()
        .value;
    let mut reader = DurableStreamReader::Owned {
        reader: Box::new(producer.catch_up(handle.clone(), None).await.unwrap()),
        source: producer.clone(),
        handle: Box::new(handle.clone()),
        next_journal_lag_sample: Instant::now(),
    };

    producer
        .write_items(
            handle.stream_id,
            0,
            StreamItemsPayloadV1::PackedU8(vec![1, 2, 3]),
        )
        .await
        .unwrap();

    assert_eq!(reader.journal_lag_events(None).await.unwrap(), 3);
    let first = reader.next().await.unwrap().unwrap();
    assert_eq!(
        reader.journal_lag_events(Some(first.offset)).await.unwrap(),
        2
    );
}

#[test]
async fn attached_preexisting_journal_lag_counts_committed_source_events() {
    let identity = identity();
    let oplog = Arc::new(TestOplog::default());
    let producer = DurableStreamProducer::load(
        oplog,
        identity.environment_id,
        identity.agent_id.clone(),
        identity.fingerprint,
        None,
    )
    .await
    .unwrap();
    let handle = producer
        .register(registration(
            &identity,
            StreamRegistrationCoordinateV1::Root {
                invocation_id: identity.invocation.clone(),
                root_kind: StreamRootKindV1::MethodResult,
                recursive_value_path: Vec::new(),
            },
            StreamSourceKindV1::InvocationOutput,
        ))
        .await
        .unwrap()
        .value;
    producer
        .write_items(
            handle.stream_id,
            0,
            StreamItemsPayloadV1::PackedU8(vec![1, 2, 3]),
        )
        .await
        .unwrap();
    let consumer_environment_id = EnvironmentId(Uuid::from_u128(41));
    let consumer = AgentId {
        component_id: ComponentId(Uuid::from_u128(42)),
        agent_id: "journal-lag-consumer".to_string(),
    };
    let consumer_fingerprint = AgentFingerprint(Uuid::from_u128(43));
    let consumer_invocation = StreamInvocationIdV1 {
        callee_environment_id: consumer_environment_id,
        callee: consumer.clone(),
        callee_fingerprint: consumer_fingerprint,
        idempotency_key: IdempotencyKey::new("journal-lag-consumer-invocation".to_string()),
    };
    let attachment = StreamAttachmentKeyV1 {
        attachment_id: AttachmentId::primary(
            consumer_environment_id,
            &consumer,
            &consumer_invocation.idempotency_key,
        )
        .unwrap(),
        stream_id: handle.stream_id,
        epoch: 1,
        session_key: consumer_invocation.clone(),
        producer_environment_id: identity.environment_id,
        producer: identity.agent_id,
        expected_producer_fingerprint: identity.fingerprint,
        consumer_environment_id,
        consumer,
        expected_consumer_fingerprint: consumer_fingerprint,
        consumer_invocation,
    };
    let now_millis = Timestamp::now_utc().to_millis();
    producer
        .prepare_attachment(attachment.clone(), now_millis)
        .await
        .unwrap();
    producer
        .activate_attachment(attachment.clone(), now_millis)
        .await
        .unwrap();
    let mut reader = DurableStreamReader::Attached(Box::new(AttachedDurableCatchUpReader {
        source: producer,
        attachment,
        handle,
        consumer_producer: None,
        after: None,
        buffered: VecDeque::new(),
        terminal: false,
        next_journal_lag_sample: Instant::now(),
    }));

    assert_eq!(reader.journal_lag_events(None).await.unwrap(), 3);
    let first = reader.next().await.unwrap().unwrap();
    assert_eq!(
        reader.journal_lag_events(Some(first.offset)).await.unwrap(),
        2
    );
}

async fn use_attached_lag_spy(
    consumer: &mut DurableInputProducer,
    producer: Arc<DurableStreamProducer>,
    identity: &TestIdentity,
    handle: &DurableStreamHandleV1,
    source: Arc<LagRecordingSource>,
) {
    let attachment = StreamAttachmentKeyV1 {
        attachment_id: AttachmentId::primary(
            identity.environment_id,
            &identity.agent_id,
            &identity.invocation.idempotency_key,
        )
        .unwrap(),
        stream_id: handle.stream_id,
        epoch: 1,
        session_key: identity.invocation.clone(),
        producer_environment_id: identity.environment_id,
        producer: identity.agent_id.clone(),
        expected_producer_fingerprint: identity.fingerprint,
        consumer_environment_id: identity.environment_id,
        consumer: identity.agent_id.clone(),
        expected_consumer_fingerprint: identity.fingerprint,
        consumer_invocation: identity.invocation.clone(),
    };
    let now_millis = Timestamp::now_utc().to_millis();
    producer
        .prepare_attachment(attachment.clone(), now_millis)
        .await
        .unwrap();
    producer
        .activate_attachment(attachment.clone(), now_millis)
        .await
        .unwrap();
    consumer.reader = Some(DurableStreamReader::Attached(Box::new(
        AttachedDurableCatchUpReader {
            source,
            attachment,
            handle: handle.clone(),
            consumer_producer: None,
            after: None,
            buffered: VecDeque::new(),
            terminal: false,
            next_journal_lag_sample: Instant::now(),
        },
    )));
}

async fn receive_for_test(
    consumer: &mut DurableInputProducer,
) -> (CommittedProducerStreamEventV1, bool, usize) {
    consumer.begin_receive();
    let (reader, event, _, journaled, queued) = consumer.pending.take().unwrap().await.unwrap();
    consumer.reader = reader;
    let queued_len = queued.len();
    consumer.journal.extend(queued);
    consumer.consumer_read_ordinal += 1;
    (event.unwrap(), journaled, queued_len)
}

#[test]
async fn consumer_value_is_committed_before_delivery_and_replay_is_a_no_op() {
    let identity = identity();
    let oplog = Arc::new(TestOplog::default());
    let producer = DurableStreamProducer::load(
        oplog.clone(),
        identity.environment_id,
        identity.agent_id.clone(),
        identity.fingerprint,
        None,
    )
    .await
    .unwrap();
    let handle = producer
        .register(registration(
            &identity,
            StreamRegistrationCoordinateV1::Root {
                invocation_id: identity.invocation.clone(),
                root_kind: StreamRootKindV1::MethodInput,
                recursive_value_path: Vec::new(),
            },
            StreamSourceKindV1::AgentHostedInput,
        ))
        .await
        .unwrap()
        .value;
    producer
        .write_items(
            handle.stream_id,
            0,
            StreamItemsPayloadV1::Values(vec![
                ProtoSchemaValue::try_from(SchemaValue::U32(42))
                    .unwrap()
                    .encode_to_vec(),
            ]),
        )
        .await
        .unwrap();

    let commits = Arc::new(AtomicU64::new(0));
    let streams = DurableSessionStreams::new(
        producer,
        oplog.clone(),
        identity.invocation,
        [(1, handle.clone(), SessionStreamRoleV1::Input)],
    )
    .with_consumer_journal(Arc::new(RecordingConsumerJournal {
        oplog,
        commits: commits.clone(),
    }));
    let mut first = DurableInputProducer::new(
        streams
            .endpoint(handle.clone(), 0, SessionStreamRoleV1::Input)
            .await
            .unwrap(),
    );
    first.begin_receive();
    let (_, event, _, journaled, _) = first.pending.take().unwrap().await.unwrap();
    assert!(!journaled);
    assert!(event.is_some());
    assert_eq!(commits.load(Ordering::Relaxed), 1);

    let mut replay = DurableInputProducer::new(
        streams
            .endpoint(handle, 0, SessionStreamRoleV1::Input)
            .await
            .unwrap(),
    );
    replay.begin_receive();
    let (_, event, _, journaled, _) = replay.pending.take().unwrap().await.unwrap();
    assert!(journaled);
    assert!(event.is_some());
    assert_eq!(commits.load(Ordering::Relaxed), 1);
}

#[test]
async fn consumer_journal_lag_sampling_is_deadline_gated_and_failure_is_throttled() {
    let identity = identity();
    let oplog = Arc::new(TestOplog::default());
    let producer = DurableStreamProducer::load(
        oplog.clone(),
        identity.environment_id,
        identity.agent_id.clone(),
        identity.fingerprint,
        None,
    )
    .await
    .unwrap();
    let handle = producer
        .register(registration(
            &identity,
            StreamRegistrationCoordinateV1::Root {
                invocation_id: identity.invocation.clone(),
                root_kind: StreamRootKindV1::MethodInput,
                recursive_value_path: Vec::new(),
            },
            StreamSourceKindV1::AgentHostedInput,
        ))
        .await
        .unwrap()
        .value;
    for value in 0..8 {
        producer
            .write_items(
                handle.stream_id,
                u64::from(value),
                StreamItemsPayloadV1::Values(vec![
                    ProtoSchemaValue::try_from(SchemaValue::U32(value))
                        .unwrap()
                        .encode_to_vec(),
                ]),
            )
            .await
            .unwrap();
    }
    let commits = Arc::new(AtomicU64::new(0));
    let streams = DurableSessionStreams::new(
        producer.clone(),
        oplog.clone(),
        identity.invocation.clone(),
        [(1, handle.clone(), SessionStreamRoleV1::Input)],
    )
    .with_consumer_journal(Arc::new(RecordingConsumerJournal {
        oplog,
        commits: commits.clone(),
    }));
    let mut consumer = DurableInputProducer::new(
        streams
            .endpoint(handle.clone(), 0, SessionStreamRoleV1::Input)
            .await
            .unwrap(),
    );
    let source = LagRecordingSource::new(producer.clone(), 0);
    use_attached_lag_spy(&mut consumer, producer, &identity, &handle, source.clone()).await;
    *consumer
        .reader
        .as_mut()
        .unwrap()
        .journal_lag_sample_deadline() = Instant::now() + Duration::from_secs(60);

    for expected_commits in 1..=5 {
        let (_, journaled, _) = receive_for_test(&mut consumer).await;
        assert!(!journaled);
        assert_eq!(commits.load(Ordering::Relaxed), expected_commits);
    }
    assert!(source.calls.lock().await.is_empty());

    *consumer
        .reader
        .as_mut()
        .unwrap()
        .journal_lag_sample_deadline() = Instant::now() - Duration::from_millis(1);
    let (sampled, journaled, _) = receive_for_test(&mut consumer).await;
    assert!(!journaled);
    assert_eq!(commits.load(Ordering::Relaxed), 6);
    assert_eq!(
        source.calls.lock().await.as_slice(),
        &[(Some(sampled.offset), Ok(2))]
    );

    source.failures_remaining.store(1, Ordering::Relaxed);
    let before_attempt = Instant::now();
    *consumer
        .reader
        .as_mut()
        .unwrap()
        .journal_lag_sample_deadline() = Instant::now() - Duration::from_millis(1);
    let (_, journaled, _) = receive_for_test(&mut consumer).await;
    assert!(!journaled);
    assert_eq!(commits.load(Ordering::Relaxed), 7);
    assert_eq!(source.calls.lock().await.len(), 2);
    assert!(source.calls.lock().await[1].1.is_err());
    assert!(
        before_attempt
            < *consumer
                .reader
                .as_mut()
                .unwrap()
                .journal_lag_sample_deadline()
    );
    *consumer
        .reader
        .as_mut()
        .unwrap()
        .journal_lag_sample_deadline() = Instant::now() + Duration::from_secs(60);
    let (_, journaled, _) = receive_for_test(&mut consumer).await;
    assert!(!journaled);
    assert_eq!(commits.load(Ordering::Relaxed), 8);
    assert_eq!(source.calls.lock().await.len(), 2);
}

#[test]
async fn packed_consumer_samples_after_final_queued_offset_and_terminal_forces_sampling() {
    let identity = identity();
    let oplog = Arc::new(TestOplog::default());
    let producer = DurableStreamProducer::load(
        oplog.clone(),
        identity.environment_id,
        identity.agent_id.clone(),
        identity.fingerprint,
        None,
    )
    .await
    .unwrap();
    let handle = producer
        .register(registration(
            &identity,
            StreamRegistrationCoordinateV1::Root {
                invocation_id: identity.invocation.clone(),
                root_kind: StreamRootKindV1::MethodInput,
                recursive_value_path: Vec::new(),
            },
            StreamSourceKindV1::AgentHostedInput,
        ))
        .await
        .unwrap()
        .value;
    let bytes = (0..4096).map(|value| value as u8).collect::<Vec<_>>();
    producer
        .write_items(
            handle.stream_id,
            0,
            StreamItemsPayloadV1::PackedU8(bytes.clone()),
        )
        .await
        .unwrap();
    producer
        .end(handle.stream_id, bytes.len() as u64, StreamEndResultV1::Ok)
        .await
        .unwrap();

    let commits = Arc::new(AtomicU64::new(0));
    let streams = DurableSessionStreams::new(
        producer.clone(),
        oplog.clone(),
        identity.invocation.clone(),
        [(1, handle.clone(), SessionStreamRoleV1::Input)],
    )
    .with_consumer_journal(Arc::new(RecordingConsumerJournal {
        oplog,
        commits: commits.clone(),
    }));
    let mut consumer = DurableInputProducer::new(
        streams
            .endpoint(handle.clone(), 0, SessionStreamRoleV1::Input)
            .await
            .unwrap(),
    );
    let source = LagRecordingSource::new(producer.clone(), 0);
    use_attached_lag_spy(&mut consumer, producer, &identity, &handle, source.clone()).await;
    *consumer
        .reader
        .as_mut()
        .unwrap()
        .journal_lag_sample_deadline() = Instant::now() - Duration::from_millis(1);

    let (first, journaled, queued) = receive_for_test(&mut consumer).await;
    assert!(!journaled);
    assert_eq!(queued, bytes.len() - 1);
    assert_eq!(commits.load(Ordering::Relaxed), 1);
    {
        let calls = source.calls.lock().await;
        assert_eq!(calls.len(), 1);
        let (sampled_after, lag) = calls[0];
        assert_eq!(
            sampled_after,
            Some(StreamOffsetV1::new(
                first.offset.producer_oplog_index(),
                bytes.len() as u32 - 1
            ))
        );
        assert_eq!(lag, Ok(1));
    }

    for _ in 1..bytes.len() {
        let (_, journaled, _) = receive_for_test(&mut consumer).await;
        assert!(journaled);
    }
    assert_eq!(commits.load(Ordering::Relaxed), 1);
    assert_eq!(source.calls.lock().await.len(), 1);

    *consumer
        .reader
        .as_mut()
        .unwrap()
        .journal_lag_sample_deadline() = Instant::now() + Duration::from_secs(60);
    let (terminal, journaled, _) = receive_for_test(&mut consumer).await;
    assert!(terminal.is_terminal());
    assert!(!journaled);
    assert_eq!(commits.load(Ordering::Relaxed), 2);
    let calls = source.calls.lock().await;
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[1], (Some(terminal.offset), Ok(0)));
}

#[test]
async fn packed_u8_consumer_values_share_one_durable_journal_record() {
    let identity = identity();
    let oplog = Arc::new(TestOplog::default());
    let producer = DurableStreamProducer::load(
        oplog.clone(),
        identity.environment_id,
        identity.agent_id.clone(),
        identity.fingerprint,
        None,
    )
    .await
    .unwrap();
    let handle = producer
        .register(registration(
            &identity,
            StreamRegistrationCoordinateV1::Root {
                invocation_id: identity.invocation.clone(),
                root_kind: StreamRootKindV1::MethodInput,
                recursive_value_path: Vec::new(),
            },
            StreamSourceKindV1::AgentHostedInput,
        ))
        .await
        .unwrap()
        .value;
    let bytes = (0..2048).map(|value| value as u8).collect::<Vec<_>>();

    let commits = Arc::new(AtomicU64::new(0));
    let streams = DurableSessionStreams::new(
        producer.clone(),
        oplog.clone(),
        identity.invocation.clone(),
        [(1, handle.clone(), SessionStreamRoleV1::Input)],
    )
    .with_consumer_journal(Arc::new(RecordingConsumerJournal {
        oplog: oplog.clone(),
        commits: commits.clone(),
    }));
    let mut consumer = DurableInputProducer::new(
        streams
            .endpoint(handle.clone(), 0, SessionStreamRoleV1::Input)
            .await
            .unwrap(),
    );
    consumer.begin_receive();
    let (write_result, receive_result) = tokio::join!(
        producer.write_items(
            handle.stream_id,
            0,
            StreamItemsPayloadV1::PackedU8(bytes.clone()),
        ),
        consumer.pending.take().unwrap(),
    );
    write_result.unwrap();
    let (_, event, _, journaled, queued) = receive_result.unwrap();
    assert!(!journaled);
    assert!(matches!(
        event.unwrap().payload,
        CommittedProducerStreamEventPayloadV1::PackedU8(0)
    ));
    assert_eq!(queued.len(), bytes.len() - 1);
    assert!(queued.iter().all(|event| matches!(
        &event.payload,
        CommittedProducerStreamEventPayloadV1::PackedU8(_)
    )));
    assert_eq!(commits.load(Ordering::Relaxed), 1);

    let (history, _, next_ordinal, terminal) =
        streams.consumer_history(handle.stream_id).await.unwrap();
    assert_eq!(history.len(), bytes.len());
    assert_eq!(next_ordinal, bytes.len() as u64);
    assert!(!terminal);
    assert_eq!(
        history
            .into_iter()
            .map(|event| match event.payload {
                CommittedProducerStreamEventPayloadV1::PackedU8(byte) => byte,
                other => panic!("expected packed-u8 history, got {other:?}"),
            })
            .collect::<Vec<_>>(),
        bytes
    );

    let reloaded = DurableStreamProducer::load(
        oplog,
        identity.environment_id,
        identity.agent_id,
        identity.fingerprint,
        None,
    )
    .await
    .unwrap();
    reloaded
        .append_session_record(StreamSessionRecordV1::ConsumerTerminal(
            StreamConsumerTerminalRecordV1 {
                format_version: 1,
                session_key: identity.invocation,
                stream_id: handle.stream_id,
                source_offset: StreamOffsetV1::new(OplogIndex::from_u64(100), 0),
                consumer_read_ordinal: 2048,
                terminal: StreamConsumerTerminalV1::End(StreamEndResultV1::Ok),
            },
        ))
        .await
        .expect("reloading must advance the consumer ordinal by every packed byte");
}

#[test]
async fn dropping_unfinished_input_producer_cancels_the_durable_source_after_cleanup_drain() {
    check_dropped_input_cancellation(false).await;
}

#[test]
async fn dropping_unread_input_resource_cancels_the_durable_source_after_cleanup_drain() {
    check_dropped_input_cancellation(true).await;
}

async fn check_dropped_input_cancellation(unread_resource: bool) {
    let identity = identity();
    let oplog = Arc::new(TestOplog::default());
    let producer = DurableStreamProducer::load(
        oplog.clone(),
        identity.environment_id,
        identity.agent_id.clone(),
        identity.fingerprint,
        None,
    )
    .await
    .unwrap();
    let handle = producer
        .register(registration(
            &identity,
            StreamRegistrationCoordinateV1::Root {
                invocation_id: identity.invocation.clone(),
                root_kind: StreamRootKindV1::MethodResult,
                recursive_value_path: Vec::new(),
            },
            StreamSourceKindV1::InvocationOutput,
        ))
        .await
        .unwrap()
        .value;
    let attempt_id = AttemptId::fresh();
    let attachment_id = AttachmentId::primary(
        identity.environment_id,
        &identity.agent_id,
        &identity.invocation.idempotency_key,
    )
    .unwrap();
    let pending_invocation_oplog_index = append_prepared_pending(
        producer.as_ref(),
        oplog.as_ref(),
        &identity,
        attachment_id,
        attempt_id,
        &handle,
        SessionStreamRoleV1::Output,
    )
    .await;
    producer
        .append_session_record(StreamSessionRecordV1::Attached(
            StreamSessionAttachedRecordV1 {
                format_version: DURABLE_STREAM_FORMAT_VERSION,
                session_key: identity.invocation.clone(),
                attachment_id,
                attempt_id,
                epoch: 1,
                pending_invocation_oplog_index,
            },
        ))
        .await
        .unwrap();
    let streams = DurableSessionStreams::new(
        producer.clone(),
        oplog.clone(),
        identity.invocation,
        [(7, handle.clone(), SessionStreamRoleV1::Output)],
    )
    .with_consumer_journal(Arc::new(TestConsumerJournal(oplog.clone())))
    .with_attachment(1, attempt_id);
    let source_cancelled = tokio_util::sync::CancellationToken::new();
    producer.register_source_cancellation(handle.stream_id, source_cancelled.clone());
    let (drop_event_sink, mut drop_events) = mpsc::unbounded_channel();
    let endpoint = streams
        .endpoint(handle.clone(), 0, SessionStreamRoleV1::Output)
        .await
        .unwrap();
    if unread_resource {
        let stream = SchemaValueStream::from_host_endpoint(endpoint);
        let moved = stream.take_for_transfer().unwrap();
        DurableInputProducer::drop_unread(stream, drop_event_sink.clone(), Arc::new(|| false));
        assert!(drop_events.try_recv().is_err());
        assert!(!source_cancelled.is_cancelled());
        DurableInputProducer::drop_unread(moved, drop_event_sink, Arc::new(|| false));
    } else {
        drop(
            DurableInputProducer::new(endpoint)
                .with_drop_cleanup(drop_event_sink, Arc::new(|| false)),
        );
    }
    let cancellation = match drop_events.recv().await.unwrap() {
        DropEvent::CancelDroppedDurableInput { cancellation } => cancellation,
        event => panic!("unexpected drop event: {event:?}"),
    };
    cancellation.cancel().await.unwrap();
    assert!(source_cancelled.is_cancelled());

    let current = oplog.current_oplog_index().await;
    let mut intents = 0;
    let mut cancellations = 0;
    for (_, entry) in oplog
        .read_exact(OplogIndex::INITIAL, current.as_u64())
        .await
    {
        match entry {
            OplogEntry::StreamSession { record, .. } => {
                if let StreamSessionRecordV1::ConsumerCancelIntent(intent) =
                    streams.download_record(record).await.unwrap()
                    && intent.stream_id == handle.stream_id
                {
                    intents += 1;
                    assert_eq!(intent.role, StreamCancelRoleV1::OutputConsumer);
                    assert_eq!(intent.reason, StreamCancelReasonV1::GuestDrop);
                }
            }
            OplogEntry::StreamCancel { record, .. } => {
                let cancellation = oplog.download_payload(record).await.unwrap();
                if cancellation.stream_id == handle.stream_id {
                    cancellations += 1;
                    assert_eq!(cancellation.role, StreamCancelRoleV1::OutputConsumer);
                    assert_eq!(cancellation.reason, StreamCancelReasonV1::GuestDrop);
                }
            }
            _ => {}
        }
    }
    assert_eq!(intents, 1);
    assert_eq!(cancellations, 1);
}

#[test]
async fn dropped_input_cancellation_is_skipped_once_the_attachment_is_fenced() {
    let identity = identity();
    let oplog = Arc::new(TestOplog::default());
    let producer = DurableStreamProducer::load(
        oplog.clone(),
        identity.environment_id,
        identity.agent_id.clone(),
        identity.fingerprint,
        None,
    )
    .await
    .unwrap();
    let handle = producer
        .register(registration(
            &identity,
            StreamRegistrationCoordinateV1::Root {
                invocation_id: identity.invocation.clone(),
                root_kind: StreamRootKindV1::MethodResult,
                recursive_value_path: Vec::new(),
            },
            StreamSourceKindV1::InvocationOutput,
        ))
        .await
        .unwrap()
        .value;
    let attempt_id = AttemptId::fresh();
    let attachment_id = AttachmentId::primary(
        identity.environment_id,
        &identity.agent_id,
        &identity.invocation.idempotency_key,
    )
    .unwrap();
    let pending_invocation_oplog_index = append_prepared_pending(
        producer.as_ref(),
        oplog.as_ref(),
        &identity,
        attachment_id,
        attempt_id,
        &handle,
        SessionStreamRoleV1::Output,
    )
    .await;
    producer
        .append_session_record(StreamSessionRecordV1::Attached(
            StreamSessionAttachedRecordV1 {
                format_version: DURABLE_STREAM_FORMAT_VERSION,
                session_key: identity.invocation.clone(),
                attachment_id,
                attempt_id,
                epoch: 1,
                pending_invocation_oplog_index,
            },
        ))
        .await
        .unwrap();
    let streams = DurableSessionStreams::new(
        producer.clone(),
        oplog.clone(),
        identity.invocation,
        [(7, handle.clone(), SessionStreamRoleV1::Output)],
    )
    .with_consumer_journal(Arc::new(TestConsumerJournal(oplog.clone())))
    .with_attachment(1, attempt_id);
    let source_cancelled = tokio_util::sync::CancellationToken::new();
    producer.register_source_cancellation(handle.stream_id, source_cancelled.clone());
    let (drop_event_sink, mut drop_events) = mpsc::unbounded_channel();
    let input = DurableInputProducer::new(
        streams
            .endpoint(handle.clone(), 0, SessionStreamRoleV1::Output)
            .await
            .unwrap(),
    )
    .with_drop_cleanup(drop_event_sink, Arc::new(|| false));

    drop(input);
    let cancellation = match drop_events.recv().await.unwrap() {
        DropEvent::CancelDroppedDurableInput { cancellation } => cancellation,
        event => panic!("unexpected drop event: {event:?}"),
    };
    assert!(streams.detach_current().await.unwrap());
    assert!(streams.ensure_current_attachment().await.is_err());
    let before_cancellation = oplog.current_oplog_index().await;

    cancellation.cancel().await.unwrap();

    assert!(!source_cancelled.is_cancelled());
    assert_eq!(oplog.current_oplog_index().await, before_cancellation);
}

#[test]
async fn guest_authored_input_cancellation_targets_the_current_attachment_epoch() {
    let identity = identity();
    let attachment_id = AttachmentId::primary(
        identity.environment_id,
        &identity.agent_id,
        &identity.invocation.idempotency_key,
    )
    .unwrap();
    let oplog = Arc::new(TestOplog::default());
    let producer = DurableStreamProducer::load(
        oplog.clone(),
        identity.environment_id,
        identity.agent_id.clone(),
        identity.fingerprint,
        None,
    )
    .await
    .unwrap();
    let handle = producer
        .register(registration(
            &identity,
            StreamRegistrationCoordinateV1::Root {
                invocation_id: identity.invocation.clone(),
                root_kind: StreamRootKindV1::MethodInput,
                recursive_value_path: Vec::new(),
            },
            StreamSourceKindV1::AgentHostedInput,
        ))
        .await
        .unwrap()
        .value;
    let start_attempt_id = AttemptId::fresh();
    let pending_invocation_oplog_index = append_prepared_pending(
        producer.as_ref(),
        oplog.as_ref(),
        &identity,
        attachment_id,
        start_attempt_id,
        &handle,
        SessionStreamRoleV1::Input,
    )
    .await;
    producer
        .append_session_record(StreamSessionRecordV1::Attached(
            StreamSessionAttachedRecordV1 {
                format_version: DURABLE_STREAM_FORMAT_VERSION,
                session_key: identity.invocation.clone(),
                attachment_id,
                attempt_id: start_attempt_id,
                epoch: 1,
                pending_invocation_oplog_index,
            },
        ))
        .await
        .unwrap();
    let transport = DurableSessionStreams::new(
        producer.clone(),
        oplog.clone(),
        identity.invocation.clone(),
        [(7, handle.clone(), SessionStreamRoleV1::Input)],
    )
    .with_consumer_journal(Arc::new(TestConsumerJournal(oplog.clone())))
    .with_attachment(1, start_attempt_id);
    let guest = DurableSessionStreams::new(
        producer.clone(),
        oplog.clone(),
        identity.invocation.clone(),
        [(7, handle.clone(), SessionStreamRoleV1::Input)],
    )
    .with_consumer_journal(Arc::new(TestConsumerJournal(oplog.clone())));
    let source_cancelled = tokio_util::sync::CancellationToken::new();
    producer.register_source_cancellation(handle.stream_id, source_cancelled.clone());

    assert!(transport.detach_current().await.unwrap());
    let resume_attempt_id = AttemptId::fresh();
    transport
        .commit_resume_attempt(StreamSessionResumeAttemptRecordV1 {
            format_version: DURABLE_STREAM_FORMAT_VERSION,
            attempt: ResumeAttemptDescriptorV1 {
                format_version: DURABLE_STREAM_FORMAT_VERSION,
                operation: StreamResumeOperationV1::Resume,
                session_key: identity.invocation.clone(),
                attachment_id,
                expected_callee_fingerprint: identity.fingerprint,
                attempt_id: resume_attempt_id,
                expected_epoch: 1,
                effective_identity: vec![3],
                cursors: Vec::new(),
                live_join_buffer_events: 8,
            },
            accepted_epoch: 2,
        })
        .await
        .unwrap();
    assert!(transport.ensure_current_attachment().await.is_err());
    assert!(guest.ensure_current_attachment().await.is_err());

    guest
        .cancel_stream(
            7,
            StreamCancelRoleV1::InputProducer,
            StreamCancelReasonV1::GuestDrop,
            Some("guest dropped input readable end".to_string()),
            None,
        )
        .await
        .unwrap();

    assert!(source_cancelled.is_cancelled());
    let current = oplog.current_oplog_index().await;
    let mut intents = Vec::new();
    for (_, entry) in oplog
        .read_exact(OplogIndex::INITIAL, current.as_u64())
        .await
    {
        if let OplogEntry::StreamSession { record, .. } = entry
            && let StreamSessionRecordV1::ConsumerCancelIntent(record) =
                guest.download_record(record).await.unwrap()
        {
            intents.push(record);
        }
    }
    assert_eq!(intents.len(), 1);
    assert_eq!(intents[0].stream_id, handle.stream_id);
    assert_eq!(intents[0].epoch, 2);
    assert_eq!(intents[0].role, StreamCancelRoleV1::InputProducer);
    assert_eq!(intents[0].reason, StreamCancelReasonV1::GuestDrop);

    for _ in 0..2050 {
        oplog.add(OplogEntry::interrupted()).await;
    }
    drop(guest.current_control_metadata().await.unwrap());
    oplog.take_read_ranges();
    let after_cancellation = oplog.current_oplog_index().await;
    guest
        .cancel_stream(
            7,
            StreamCancelRoleV1::InputProducer,
            StreamCancelReasonV1::GuestDrop,
            Some("guest dropped input readable end".to_string()),
            None,
        )
        .await
        .unwrap();
    assert_eq!(oplog.current_oplog_index().await, after_cancellation);
    assert!(
        oplog.take_read_ranges().is_empty(),
        "repeated cancellation must reuse the indexed intent"
    );
}

#[test]
async fn dropping_input_producer_during_runtime_teardown_does_not_schedule_cancellation() {
    let identity = identity();
    let oplog = Arc::new(TestOplog::default());
    let producer = DurableStreamProducer::load(
        oplog.clone(),
        identity.environment_id,
        identity.agent_id.clone(),
        identity.fingerprint,
        None,
    )
    .await
    .unwrap();
    let handle = producer
        .register(registration(
            &identity,
            StreamRegistrationCoordinateV1::Root {
                invocation_id: identity.invocation.clone(),
                root_kind: StreamRootKindV1::MethodResult,
                recursive_value_path: Vec::new(),
            },
            StreamSourceKindV1::InvocationOutput,
        ))
        .await
        .unwrap()
        .value;
    let streams = DurableSessionStreams::new(
        producer,
        oplog,
        identity.invocation,
        [(7, handle.clone(), SessionStreamRoleV1::Output)],
    );
    let (drop_event_sink, mut drop_events) = mpsc::unbounded_channel();
    let input = DurableInputProducer::new(
        streams
            .endpoint(handle, 0, SessionStreamRoleV1::Output)
            .await
            .unwrap(),
    )
    .with_drop_cleanup(drop_event_sink, Arc::new(|| true));

    drop(input);
    assert!(drop_events.try_recv().is_err());
}

#[test]
async fn closed_foreign_journal_replays_after_source_finalization_and_epoch_change() {
    let source = identity();
    let source_oplog = Arc::new(TestOplog::default());
    let producer = DurableStreamProducer::load(
        source_oplog.clone(),
        source.environment_id,
        source.agent_id.clone(),
        source.fingerprint,
        None,
    )
    .await
    .unwrap();
    let handle = producer
        .register(registration(
            &source,
            StreamRegistrationCoordinateV1::Root {
                invocation_id: source.invocation.clone(),
                root_kind: StreamRootKindV1::MethodResult,
                recursive_value_path: Vec::new(),
            },
            StreamSourceKindV1::InvocationOutput,
        ))
        .await
        .unwrap()
        .value;
    let mut consumer = identity();
    consumer.agent_id.agent_id = "closed-journal-consumer".into();
    consumer.fingerprint = AgentFingerprint::new();
    consumer.invocation.callee = consumer.agent_id.clone();
    consumer.invocation.callee_fingerprint = consumer.fingerprint;
    let oplog = Arc::new(TestOplog::default());
    let local = DurableStreamProducer::load(
        oplog.clone(),
        consumer.environment_id,
        consumer.agent_id.clone(),
        consumer.fingerprint,
        None,
    )
    .await
    .unwrap();
    let streams = DurableSessionStreams::new(local, oplog.clone(), source.invocation.clone(), [])
        .with_consumer_invocation(consumer.invocation.clone())
        .with_consumer_journal(Arc::new(TestConsumerJournal(oplog.clone())));
    let attachment = streams.attachment_key(&handle, 1).unwrap();
    let mapping = StreamSessionMappingRecordV1 {
        transport_stream_id: 1,
        handle: handle.clone(),
        role: SessionStreamRoleV1::Output,
    };
    for record in [
        StreamSessionRecordV1::TopologyPrepared(StreamTopologyPreparedRecordV1 {
            format_version: 1,
            session_key: source.invocation.clone(),
            attachment: attachment.clone(),
            mapping: mapping.clone(),
        }),
        StreamSessionRecordV1::TopologyActivated(StreamTopologyActivatedRecordV1 {
            format_version: 1,
            session_key: source.invocation.clone(),
            attachment: attachment.clone(),
            mapping: mapping.clone(),
        }),
        StreamSessionRecordV1::Mapping(
            golem_common::model::durable_stream::StreamSessionMappingUpdateRecordV1 {
                format_version: 1,
                session_key: source.invocation.clone(),
                mapping: mapping.clone(),
            },
        ),
    ] {
        assert!(record.has_supported_format());
        streams.append_record(record).await;
    }
    producer
        .prepare_attachment(attachment.clone(), 100)
        .await
        .unwrap();
    producer
        .activate_attachment(attachment.clone(), 100)
        .await
        .unwrap();
    producer
        .end(handle.stream_id, 0, StreamEndResultV1::Ok)
        .await
        .unwrap();
    let source_offset = producer
        .input_high_water(handle.stream_id)
        .await
        .unwrap()
        .unwrap()
        .resulting_offset;
    streams
        .append_record(StreamSessionRecordV1::ConsumerTerminal(
            StreamConsumerTerminalRecordV1 {
                format_version: 1,
                session_key: source.invocation.clone(),
                stream_id: handle.stream_id,
                source_offset,
                consumer_read_ordinal: 0,
                terminal: StreamConsumerTerminalV1::End(StreamEndResultV1::Ok),
            },
        ))
        .await;
    streams.commit_consumer_journal().await.unwrap();
    producer.finalize_attachment(attachment.clone(), golem_common::model::durable_stream::StreamAttachmentFinalizationReasonV1::ConsumerFinalized, 101).await.unwrap();
    let mut next_attachment = attachment;
    next_attachment.epoch = 2;
    assert!(
        producer
            .prepare_attachment(next_attachment, 102)
            .await
            .is_err()
    );
    drop(streams);
    drop(producer);
    drop(source_oplog);

    // Reconstruct after the journal commit but before delivering the terminal to the guest.
    let local = DurableStreamProducer::load(
        oplog.clone(),
        consumer.environment_id,
        consumer.agent_id,
        consumer.fingerprint,
        None,
    )
    .await
    .unwrap();
    let restarted = DurableSessionStreams::new(local, oplog.clone(), source.invocation, [])
        .with_consumer_invocation(consumer.invocation)
        .with_attachment(2, AttemptId(Uuid::new_v4()))
        .with_consumer_journal(Arc::new(TestConsumerJournal(oplog)));
    restarted.recover_session_mappings().await.unwrap();
    assert!(
        restarted
            .has_journaled_consumer_terminal(&mapping)
            .await
            .unwrap()
    );
    let mut wrong_mapping = mapping;
    wrong_mapping.handle.expected_producer_fingerprint = AgentFingerprint::new();
    assert!(
        restarted
            .has_journaled_consumer_terminal(&wrong_mapping)
            .await
            .is_err()
    );
    let endpoint = restarted
        .endpoint(handle, 0, SessionStreamRoleV1::Output)
        .await
        .unwrap();
    assert!(
        endpoint.reader.is_none(),
        "closed replay must not open a remote source"
    );
    let mut replay = DurableInputProducer::new(endpoint);
    replay.begin_receive();
    let (_, event, _, journaled, _) = replay.pending.take().unwrap().await.unwrap();
    assert!(journaled);
    assert!(matches!(
        event.unwrap().payload,
        CommittedProducerStreamEventPayloadV1::End(StreamEndResultV1::Ok)
    ));
}

#[test]
async fn source_unavailable_overlay_replays_without_reopening_the_source() {
    let identity = identity();
    let oplog = Arc::new(TestOplog::default());
    let producer = DurableStreamProducer::load(
        oplog.clone(),
        identity.environment_id,
        identity.agent_id.clone(),
        identity.fingerprint,
        None,
    )
    .await
    .unwrap();
    let handle = producer
        .register(registration(
            &identity,
            StreamRegistrationCoordinateV1::Root {
                invocation_id: identity.invocation.clone(),
                root_kind: StreamRootKindV1::MethodInput,
                recursive_value_path: Vec::new(),
            },
            StreamSourceKindV1::AgentHostedInput,
        ))
        .await
        .unwrap()
        .value;
    let streams = DurableSessionStreams::new(
        producer,
        oplog,
        identity.invocation,
        [(1, handle.clone(), SessionStreamRoleV1::Input)],
    );
    streams
        .append_record(StreamSessionRecordV1::SourceUnavailable(
            golem_common::base_model::durable_stream::StreamSourceUnavailableRecordV1 {
                format_version: DURABLE_STREAM_FORMAT_VERSION,
                key: streams.attachment_key(&handle, 1).unwrap(),
                source_offset: golem_common::model::durable_stream::StreamOffsetV1::new(
                    OplogIndex::INITIAL,
                    0,
                ),
                consumer_read_ordinal: 0,
            },
        ))
        .await;

    let endpoint = streams
        .endpoint(handle, 0, SessionStreamRoleV1::Input)
        .await
        .unwrap();
    assert!(endpoint.reader.is_none());
    let mut replay = DurableInputProducer::new(endpoint);
    replay.begin_receive();
    let (_, event, _, journaled, _) = replay.pending.take().unwrap().await.unwrap();
    assert!(journaled);
    assert!(matches!(
        event.unwrap().payload,
        CommittedProducerStreamEventPayloadV1::Cancel {
            role: StreamCancelRoleV1::System,
            reason: StreamCancelReasonV1::SourceUnavailable,
            details: None,
        }
    ));
}

#[test]
async fn resume_cursors_cannot_name_input_streams() {
    let identity = identity();
    let oplog = Arc::new(TestOplog::default());
    let producer = DurableStreamProducer::load(
        oplog.clone(),
        identity.environment_id,
        identity.agent_id.clone(),
        identity.fingerprint,
        None,
    )
    .await
    .unwrap();
    let handle = producer
        .register(registration(
            &identity,
            StreamRegistrationCoordinateV1::Root {
                invocation_id: identity.invocation.clone(),
                root_kind: StreamRootKindV1::MethodInput,
                recursive_value_path: Vec::new(),
            },
            StreamSourceKindV1::AgentHostedInput,
        ))
        .await
        .unwrap()
        .value;
    let streams = DurableSessionStreams::new(
        producer,
        oplog,
        identity.invocation,
        [(1, handle.clone(), SessionStreamRoleV1::Input)],
    );

    let error = streams
        .validate_resume_cursors(
            &[golem_common::model::durable_stream::StreamResumeCursorV1 {
                stream_id: handle.stream_id,
                last_observed_offset: None,
            }],
        )
        .await
        .unwrap_err();
    assert!(error.contains("no output mapping"));
}

#[test]
async fn output_resume_cursor_is_not_shadowed_by_same_handle_input_mapping() {
    let identity = identity();
    let oplog = Arc::new(TestOplog::default());
    let producer = DurableStreamProducer::load(
        oplog.clone(),
        identity.environment_id,
        identity.agent_id.clone(),
        identity.fingerprint,
        None,
    )
    .await
    .unwrap();
    let handle = producer
        .register(registration(
            &identity,
            StreamRegistrationCoordinateV1::Root {
                invocation_id: identity.invocation.clone(),
                root_kind: StreamRootKindV1::MethodInput,
                recursive_value_path: Vec::new(),
            },
            StreamSourceKindV1::AgentHostedInput,
        ))
        .await
        .unwrap()
        .value;
    let cursor = golem_common::model::durable_stream::StreamResumeCursorV1 {
        stream_id: handle.stream_id,
        last_observed_offset: None,
    };

    for _ in 0..32 {
        let streams = DurableSessionStreams::new(
            producer.clone(),
            oplog.clone(),
            identity.invocation.clone(),
            [
                (1, handle.clone(), SessionStreamRoleV1::Input),
                (2, handle.clone(), SessionStreamRoleV1::Output),
            ],
        );
        assert!(
            streams
                .validate_resume_cursors(std::slice::from_ref(&cursor))
                .await
                .is_ok(),
            "the output mapping must authorize its cursor regardless of the same handle's input mapping"
        );
    }
}

#[test]
async fn detach_resume_and_takeover_advance_authority_and_fence_old_epochs() {
    let identity = identity();
    let attachment_id = AttachmentId::primary(
        identity.environment_id,
        &identity.agent_id,
        &identity.invocation.idempotency_key,
    )
    .unwrap();
    let oplog = Arc::new(TestOplog::default());
    let producer = DurableStreamProducer::load(
        oplog.clone(),
        identity.environment_id,
        identity.agent_id.clone(),
        identity.fingerprint,
        None,
    )
    .await
    .unwrap();
    let handle = producer
        .register(registration(
            &identity,
            StreamRegistrationCoordinateV1::Root {
                invocation_id: identity.invocation.clone(),
                root_kind: StreamRootKindV1::MethodInput,
                recursive_value_path: Vec::new(),
            },
            StreamSourceKindV1::AgentHostedInput,
        ))
        .await
        .unwrap()
        .value;
    let mapping = StreamSessionMappingRecordV1 {
        transport_stream_id: 7,
        handle: handle.clone(),
        role: SessionStreamRoleV1::Input,
    };
    let output_handle = producer
        .register(registration(
            &identity,
            StreamRegistrationCoordinateV1::Root {
                invocation_id: identity.invocation.clone(),
                root_kind: StreamRootKindV1::MethodResult,
                recursive_value_path: Vec::new(),
            },
            StreamSourceKindV1::InvocationOutput,
        ))
        .await
        .unwrap()
        .value;
    let start_attempt_id = AttemptId::fresh();
    producer
        .append_session_record(StreamSessionRecordV1::Prepared(
            StreamSessionPreparedRecordV1 {
                format_version: DURABLE_STREAM_FORMAT_VERSION,
                attempt: StartAttemptDescriptorV1 {
                    format_version: DURABLE_STREAM_FORMAT_VERSION,
                    session_key: identity.invocation.clone(),
                    attachment_id,
                    expected_callee_fingerprint: identity.fingerprint,
                    attempt_id: start_attempt_id,
                    invocation: PersistedStreamInvocationDescriptorV1 {
                        format_version: DURABLE_STREAM_FORMAT_VERSION,
                        session_key: identity.invocation.clone(),
                        target_component_revision: ComponentRevision::INITIAL,
                        method_name: "consume".to_string(),
                        invocation_value: vec![1],
                        stream_handles: vec![handle.clone()],
                        execution_config: vec![2],
                        effective_identity: vec![3],
                    },
                    effective_identity: vec![3],
                    live_join_buffer_events: 8,
                },
                stream_mappings: vec![mapping.clone()],
            },
        ))
        .await
        .unwrap();
    let pending_invocation_oplog_index = oplog
        .add(OplogEntry::pending_agent_invocation(
            identity.invocation.idempotency_key.clone(),
            OplogPayload::Inline(Box::new(AgentInvocationPayload::SaveSnapshot)),
            TraceId::generate(),
            Vec::new(),
            Vec::new(),
        ))
        .await;
    producer
        .append_session_record(StreamSessionRecordV1::Attached(
            StreamSessionAttachedRecordV1 {
                format_version: DURABLE_STREAM_FORMAT_VERSION,
                session_key: identity.invocation.clone(),
                attachment_id,
                attempt_id: start_attempt_id,
                epoch: 1,
                pending_invocation_oplog_index,
            },
        ))
        .await
        .unwrap();
    let streams_for = |epoch, attempt_id| {
        DurableSessionStreams::new(
            producer.clone(),
            oplog.clone(),
            identity.invocation.clone(),
            [
                (7, handle.clone(), SessionStreamRoleV1::Input),
                (8, output_handle.clone(), SessionStreamRoleV1::Output),
            ],
        )
        .with_consumer_journal(Arc::new(TestConsumerJournal(oplog.clone())))
        .with_attachment(epoch, attempt_id)
    };
    let attempt = |operation, expected_epoch, attempt_id| ResumeAttemptDescriptorV1 {
        format_version: DURABLE_STREAM_FORMAT_VERSION,
        operation,
        session_key: identity.invocation.clone(),
        attachment_id,
        expected_callee_fingerprint: identity.fingerprint,
        attempt_id,
        expected_epoch,
        effective_identity: vec![3],
        cursors: Vec::new(),
        live_join_buffer_events: 8,
    };

    let epoch1 = streams_for(1, start_attempt_id);
    epoch1.ensure_current_attachment().await.unwrap();
    assert!(epoch1.detach_current().await.unwrap());
    assert!(epoch1.ensure_current_attachment().await.is_err());
    producer
        .write_items(
            output_handle.stream_id,
            0,
            StreamItemsPayloadV1::Values(vec![
                ProtoSchemaValue::try_from(SchemaValue::U32(42))
                    .unwrap()
                    .encode_to_vec(),
            ]),
        )
        .await
        .unwrap();
    let after_detach = oplog.current_oplog_index().await;
    assert!(!epoch1.detach_current().await.unwrap());
    assert_eq!(oplog.current_oplog_index().await, after_detach);

    for (operation, expected_epoch, accepted_epoch, expected_error) in [
        (
            StreamResumeOperationV1::Takeover,
            1,
            2,
            "InvalidAttachmentState",
        ),
        (StreamResumeOperationV1::Resume, 0, 1, "StaleEpoch"),
        (StreamResumeOperationV1::Resume, 2, 3, "InvalidEpoch"),
    ] {
        let error = epoch1
            .commit_resume_attempt(StreamSessionResumeAttemptRecordV1 {
                format_version: DURABLE_STREAM_FORMAT_VERSION,
                attempt: attempt(operation, expected_epoch, AttemptId::fresh()),
                accepted_epoch,
            })
            .await
            .unwrap_err();
        assert!(error.contains(expected_error), "unexpected error: {error}");
        assert_eq!(oplog.current_oplog_index().await, after_detach);
    }

    let resume_attempt_id = AttemptId::fresh();
    epoch1
        .commit_resume_attempt(StreamSessionResumeAttemptRecordV1 {
            format_version: DURABLE_STREAM_FORMAT_VERSION,
            attempt: attempt(StreamResumeOperationV1::Resume, 1, resume_attempt_id),
            accepted_epoch: 2,
        })
        .await
        .unwrap();
    let epoch2 = streams_for(2, resume_attempt_id);
    epoch2.ensure_current_attachment().await.unwrap();
    assert!(epoch1.ensure_current_attachment().await.is_err());
    assert!(
        epoch1
            .validate_frame(
                7,
                Some(handle.stream_id.0.into()),
                1,
                SessionStreamRoleV1::Input,
            )
            .await
            .unwrap_err()
            .contains("StaleEpoch")
    );
    let before_stale_cancellation = oplog.current_oplog_index().await;
    let error = epoch1
        .cancel_stream(
            7,
            StreamCancelRoleV1::InputProducer,
            StreamCancelReasonV1::Cancelled,
            Some("deferred cancellation from the old attachment".to_string()),
            None,
        )
        .await
        .unwrap_err();
    assert!(error.contains("StaleEpoch"), "unexpected error: {error}");
    assert_eq!(oplog.current_oplog_index().await, before_stale_cancellation);
    let before_old_detach = oplog.current_oplog_index().await;
    assert!(!epoch1.detach_current().await.unwrap());
    assert_eq!(oplog.current_oplog_index().await, before_old_detach);

    let takeover_attempt_id = AttemptId::fresh();
    let epoch2_monitor = epoch2.clone();
    let epoch2_revoked =
        tokio::spawn(async move { epoch2_monitor.wait_for_attachment_revocation().await });
    epoch2
        .commit_resume_attempt(StreamSessionResumeAttemptRecordV1 {
            format_version: DURABLE_STREAM_FORMAT_VERSION,
            attempt: attempt(StreamResumeOperationV1::Takeover, 2, takeover_attempt_id),
            accepted_epoch: 3,
        })
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(1), epoch2_revoked)
        .await
        .expect("idle attachment did not observe takeover revocation")
        .unwrap()
        .unwrap();
    let epoch3 = streams_for(3, takeover_attempt_id);
    epoch3.ensure_current_attachment().await.unwrap();
    assert!(epoch2.ensure_current_attachment().await.is_err());
    epoch3
        .validate_frame(
            7,
            Some(handle.stream_id.0.into()),
            3,
            SessionStreamRoleV1::Input,
        )
        .await
        .unwrap();

    epoch3
        .cancel_stream(
            7,
            StreamCancelRoleV1::InputProducer,
            StreamCancelReasonV1::Cancelled,
            Some("explicit input cancellation".to_string()),
            Some(3),
        )
        .await
        .unwrap();
    epoch3
        .cancel_stream(
            8,
            StreamCancelRoleV1::OutputConsumer,
            StreamCancelReasonV1::GuestDrop,
            Some("guest dropped output readable end".to_string()),
            Some(3),
        )
        .await
        .unwrap();

    let after_cancellations = oplog.current_oplog_index().await;
    epoch3
        .cancel_stream(
            7,
            StreamCancelRoleV1::InputProducer,
            StreamCancelReasonV1::Cancelled,
            Some("explicit input cancellation".to_string()),
            Some(3),
        )
        .await
        .unwrap();
    epoch3
        .cancel_stream(
            8,
            StreamCancelRoleV1::OutputConsumer,
            StreamCancelReasonV1::GuestDrop,
            Some("guest dropped output readable end".to_string()),
            Some(3),
        )
        .await
        .unwrap();
    assert_eq!(oplog.current_oplog_index().await, after_cancellations);

    let current = oplog.current_oplog_index().await;
    let mut intent_indexes = HashMap::new();
    let mut terminal_indexes = HashMap::new();
    for (index, entry) in oplog
        .read_exact(OplogIndex::INITIAL, current.as_u64())
        .await
    {
        match entry {
            OplogEntry::StreamSession { record, .. } => {
                if let StreamSessionRecordV1::ConsumerCancelIntent(record) =
                    epoch3.download_record(record).await.unwrap()
                {
                    intent_indexes.insert(record.stream_id, (index, record.role, record.reason));
                }
            }
            OplogEntry::StreamCancel { record, .. } => {
                let record = oplog.download_payload(record).await.unwrap();
                terminal_indexes.insert(record.stream_id, (index, record.role, record.reason));
            }
            _ => {}
        }
    }
    for (stream_id, expected_role, expected_reason) in [
        (
            handle.stream_id,
            StreamCancelRoleV1::InputProducer,
            StreamCancelReasonV1::Cancelled,
        ),
        (
            output_handle.stream_id,
            StreamCancelRoleV1::OutputConsumer,
            StreamCancelReasonV1::GuestDrop,
        ),
    ] {
        let (intent_index, intent_role, intent_reason) = intent_indexes[&stream_id];
        let (terminal_index, terminal_role, terminal_reason) = terminal_indexes[&stream_id];
        assert!(intent_index < terminal_index);
        assert_eq!(intent_role, expected_role);
        assert_eq!(terminal_role, expected_role);
        assert_eq!(intent_reason, expected_reason);
        assert_eq!(terminal_reason, expected_reason);
    }
}

#[test]
async fn nested_consumer_mappings_preserve_the_parent_input_or_output_role() {
    let identity = identity();
    let oplog = Arc::new(TestOplog::default());
    let producer = DurableStreamProducer::load(
        oplog.clone(),
        identity.environment_id,
        identity.agent_id.clone(),
        identity.fingerprint,
        None,
    )
    .await
    .unwrap();
    let mut roots = Vec::new();
    let mut nested_handles = Vec::new();
    for (root_kind, role, attachment_id) in [
        (
            StreamRootKindV1::MethodInput,
            SessionStreamRoleV1::Input,
            101,
        ),
        (
            StreamRootKindV1::MethodResult,
            SessionStreamRoleV1::Output,
            102,
        ),
    ] {
        let mut root_request = registration(
            &identity,
            StreamRegistrationCoordinateV1::Root {
                invocation_id: identity.invocation.clone(),
                root_kind,
                recursive_value_path: Vec::new(),
            },
            match role {
                SessionStreamRoleV1::Input => StreamSourceKindV1::AgentHostedInput,
                SessionStreamRoleV1::Output => StreamSourceKindV1::InvocationOutput,
            },
        );
        root_request.session_mapping = Some(StreamSessionMappingV1 {
            session_key: identity.invocation.clone(),
            attachment_id: AttachmentId(Uuid::from_u128(attachment_id)),
            role,
        });
        let root = producer.register(root_request).await.unwrap().value;
        let nested_request = registration(
            &identity,
            StreamRegistrationCoordinateV1::Nested {
                parent_stream_id: root.stream_id,
                parent_producer_sequence: 0,
                recursive_value_path: Vec::new(),
            },
            StreamSourceKindV1::Nested,
        );
        producer
            .write_items_with_nested(
                root.stream_id,
                0,
                StreamItemsPayloadV1::Values(vec![
                    ProtoSchemaValue {
                        value: Some(schema_value::Value::StreamReference(
                            SchemaValueStreamReference { stream_id: 0 },
                        )),
                    }
                    .encode_to_vec(),
                ]),
                vec![nested_request],
            )
            .await
            .unwrap();
        let nested = producer.nested_handles(root.stream_id, 0).await.unwrap()[0].clone();
        roots.push((role, root));
        nested_handles.push((role, nested));
    }

    let streams = DurableSessionStreams::new(
        producer.clone(),
        oplog.clone(),
        identity.invocation.clone(),
        roots
            .iter()
            .enumerate()
            .map(|(index, (role, handle))| (index as u64, handle.clone(), *role)),
    )
    .with_consumer_journal(Arc::new(TestConsumerJournal(oplog.clone())));
    for (role, root) in &roots {
        let mut consumer =
            DurableInputProducer::new(streams.endpoint(root.clone(), 0, *role).await.unwrap());
        consumer.begin_receive();
        let (_, event, nested, journaled, _) = consumer.pending.take().unwrap().await.unwrap();
        assert!(!journaled);
        assert!(event.is_some());
        assert_eq!(nested.len(), 1);
    }

    let mut persisted_roles = HashMap::new();
    let current = oplog.current_oplog_index().await;
    for (_, entry) in oplog
        .read_exact(OplogIndex::INITIAL, current.as_u64())
        .await
    {
        let OplogEntry::StreamSession { record, .. } = entry else {
            continue;
        };
        if let StreamSessionRecordV1::ConsumerItemValue(record) =
            streams.download_record(record).await.unwrap()
            && let [mapping] = record.recursive_mappings.as_slice()
        {
            persisted_roles.insert(record.stream_id, mapping.role);
        }
    }
    for ((role, root), (_, nested)) in roots.iter().zip(&nested_handles) {
        assert_eq!(persisted_roles.get(&root.stream_id), Some(role));
        assert!(streams.mapping_for_handle(nested, *role).is_some());
    }

    let restarted = DurableSessionStreams::new(
        producer,
        oplog,
        identity.invocation,
        roots
            .iter()
            .enumerate()
            .map(|(index, (role, handle))| (index as u64, handle.clone(), *role)),
    );
    restarted.recover_session_mappings().await.unwrap();
    for (role, nested) in nested_handles {
        assert!(restarted.mapping_for_handle(&nested, role).is_some());
        assert!(
            restarted
                .mapping_for_handle(
                    &nested,
                    match role {
                        SessionStreamRoleV1::Input => SessionStreamRoleV1::Output,
                        SessionStreamRoleV1::Output => SessionStreamRoleV1::Input,
                    },
                )
                .is_none()
        );
    }
}

#[test]
async fn forwarded_topology_is_committed_before_visibility_and_replays_exactly() {
    let consumer = identity();
    let producer_identity = TestIdentity {
        environment_id: EnvironmentId(Uuid::from_u128(21)),
        agent_id: AgentId {
            component_id: ComponentId(Uuid::from_u128(22)),
            agent_id: "remote-producer".to_string(),
        },
        fingerprint: AgentFingerprint(Uuid::from_u128(23)),
        invocation: StreamInvocationIdV1 {
            callee_environment_id: EnvironmentId(Uuid::from_u128(21)),
            callee: AgentId {
                component_id: ComponentId(Uuid::from_u128(22)),
                agent_id: "remote-producer".to_string(),
            },
            callee_fingerprint: AgentFingerprint(Uuid::from_u128(23)),
            idempotency_key: IdempotencyKey::new("remote-invocation".to_string()),
        },
    };
    let producer_oplog = Arc::new(TestOplog::default());
    let remote_producer = DurableStreamProducer::load(
        producer_oplog.clone(),
        producer_identity.environment_id,
        producer_identity.agent_id.clone(),
        producer_identity.fingerprint,
        None,
    )
    .await
    .unwrap();
    let handle = remote_producer
        .register(registration(
            &producer_identity,
            StreamRegistrationCoordinateV1::Root {
                invocation_id: producer_identity.invocation.clone(),
                root_kind: StreamRootKindV1::MethodResult,
                recursive_value_path: Vec::new(),
            },
            StreamSourceKindV1::InvocationOutput,
        ))
        .await
        .unwrap()
        .value;
    let consumer_oplog = Arc::new(TestOplog::default());
    let consumer_producer = DurableStreamProducer::load(
        consumer_oplog.clone(),
        consumer.environment_id,
        consumer.agent_id.clone(),
        consumer.fingerprint,
        None,
    )
    .await
    .unwrap();
    let streams = DurableSessionStreams::new(
        consumer_producer.clone(),
        consumer_oplog.clone(),
        consumer.invocation.clone(),
        [],
    )
    .with_consumer_journal(Arc::new(TestConsumerJournal(consumer_oplog.clone())))
    .with_rpc(Arc::new(AttachedProducerRpc {
        producer: remote_producer.clone(),
        cancellation_owner: None,
        stall_next_cancel: Default::default(),
        scripted_reads: Mutex::default(),
        pending_read: Mutex::default(),
        read_requests: Mutex::default(),
    }))
    .with_auth_ctx(AuthCtx::System);
    let attachment = StreamAttachmentKeyV1 {
        attachment_id: AttachmentId::primary(
            consumer.environment_id,
            &consumer.agent_id,
            &consumer.invocation.idempotency_key,
        )
        .unwrap(),
        stream_id: handle.stream_id,
        epoch: 1,
        session_key: consumer.invocation.clone(),
        producer_environment_id: producer_identity.environment_id,
        producer: producer_identity.agent_id.clone(),
        expected_producer_fingerprint: producer_identity.fingerprint,
        consumer_environment_id: consumer.environment_id,
        consumer: consumer.agent_id.clone(),
        expected_consumer_fingerprint: consumer.fingerprint,
        consumer_invocation: consumer.invocation.clone(),
    };
    let mapping = StreamSessionMappingRecordV1 {
        transport_stream_id: 17,
        handle: handle.clone(),
        role: SessionStreamRoleV1::Input,
    };
    let attempt_id = AttemptId::fresh();
    streams
        .append_record(StreamSessionRecordV1::Prepared(
            StreamSessionPreparedRecordV1 {
                format_version: DURABLE_STREAM_FORMAT_VERSION,
                attempt: StartAttemptDescriptorV1 {
                    format_version: DURABLE_STREAM_FORMAT_VERSION,
                    session_key: consumer.invocation.clone(),
                    attachment_id: attachment.attachment_id,
                    expected_callee_fingerprint: consumer.fingerprint,
                    attempt_id,
                    invocation: PersistedStreamInvocationDescriptorV1 {
                        format_version: DURABLE_STREAM_FORMAT_VERSION,
                        session_key: consumer.invocation.clone(),
                        target_component_revision: ComponentRevision::INITIAL,
                        method_name: "forward".to_string(),
                        invocation_value: vec![1],
                        stream_handles: vec![handle.clone()],
                        execution_config: vec![2],
                        effective_identity: vec![3],
                    },
                    effective_identity: vec![3],
                    live_join_buffer_events: 8,
                },
                stream_mappings: vec![mapping.clone()],
            },
        ))
        .await;
    let pending_invocation_oplog_index = consumer_oplog
        .add(OplogEntry::pending_agent_invocation(
            consumer.invocation.idempotency_key.clone(),
            OplogPayload::Inline(Box::new(AgentInvocationPayload::SaveSnapshot)),
            TraceId::generate(),
            Vec::new(),
            Vec::new(),
        ))
        .await;
    streams
        .append_record(StreamSessionRecordV1::Attached(
            golem_common::base_model::durable_stream::StreamSessionAttachedRecordV1 {
                format_version: DURABLE_STREAM_FORMAT_VERSION,
                session_key: consumer.invocation.clone(),
                attachment_id: attachment.attachment_id,
                attempt_id,
                epoch: 1,
                pending_invocation_oplog_index,
            },
        ))
        .await;
    let streams = streams.with_attachment(1, attempt_id);
    remote_producer
        .prepare_attachment(attachment.clone(), 100)
        .await
        .unwrap();
    assert!(streams.handle(17).is_none());
    streams
        .activate_forwarded_mapping(
            attachment.clone(),
            mapping.clone(),
            remote_producer.clone(),
            110,
        )
        .await
        .unwrap();
    assert_eq!(streams.handle(17), Some(handle.clone()));
    assert_eq!(
        StreamAttachmentConsumerProbe::status(&streams, &attachment)
            .await
            .unwrap(),
        ConsumerAttachmentStatus::Active
    );
    let output_mapping = StreamSessionMappingRecordV1 {
        transport_stream_id: 18,
        handle: handle.clone(),
        role: SessionStreamRoleV1::Output,
    };
    streams
        .activate_forwarded_mapping(
            attachment.clone(),
            output_mapping.clone(),
            remote_producer.clone(),
            112,
        )
        .await
        .unwrap();
    assert_eq!(streams.handle(18), Some(handle.clone()));
    assert!(
        streams
            .validate_frame(
                18,
                Some(handle.stream_id.0.into()),
                1,
                SessionStreamRoleV1::Input,
            )
            .await
            .is_err()
    );
    assert!(
        streams
            .validate_frame(
                17,
                Some(handle.stream_id.0.into()),
                1,
                SessionStreamRoleV1::Output,
            )
            .await
            .is_err()
    );
    assert_eq!(
        StreamAttachmentConsumerProbe::status_exact(&streams, &attachment, Some(&output_mapping),)
            .await
            .unwrap(),
        ConsumerAttachmentStatus::Active
    );
    assert!(streams.input_high_waters().await.unwrap().is_empty());
    assert!(
        remote_producer
            .read_attached_segment(&attachment, &handle, 111, None, None)
            .await
            .unwrap()
            .is_empty()
    );
    let consumer_length = consumer_oplog.current_oplog_index().await;
    let producer_length = producer_oplog.current_oplog_index().await;
    streams
        .activate_forwarded_mapping(
            attachment.clone(),
            mapping.clone(),
            remote_producer.clone(),
            120,
        )
        .await
        .unwrap();
    assert_eq!(consumer_oplog.current_oplog_index().await, consumer_length);
    assert_eq!(producer_oplog.current_oplog_index().await, producer_length);

    for _ in 0..2050 {
        consumer_oplog.add(OplogEntry::interrupted()).await;
    }
    assert_eq!(
        streams
            .topology_state(&attachment, Some(&mapping))
            .await
            .unwrap(),
        ConsumerAttachmentStatus::Active
    );
    consumer_oplog.take_read_ranges();
    assert_eq!(
        streams
            .topology_state(&attachment, Some(&mapping))
            .await
            .unwrap(),
        ConsumerAttachmentStatus::Active
    );
    streams.validate_topology_complete().await.unwrap();
    assert!(
        consumer_oplog.take_read_ranges().is_empty(),
        "warm topology checks must not revisit unrelated history"
    );

    let mut projection = streams.current_control_metadata().await.unwrap().clone();
    let initial_size = golem_common::serialization::serialize(&projection)
        .unwrap()
        .len();
    let mut resumed_attachment = attachment.clone();
    for epoch in 2..1002 {
        resumed_attachment.epoch = epoch;
        projection.apply(
            OplogIndex::from_u64(10_000 + epoch * 2),
            &consumer.invocation,
            &StreamSessionRecordV1::TopologyPrepared(StreamTopologyPreparedRecordV1 {
                format_version: 1,
                session_key: consumer.invocation.clone(),
                attachment: resumed_attachment.clone(),
                mapping: mapping.clone(),
            }),
        );
        assert_eq!(
            projection
                .topology_status(&resumed_attachment, Some(&mapping))
                .unwrap(),
            ConsumerAttachmentStatus::Prepared
        );
        projection.apply(
            OplogIndex::from_u64(10_001 + epoch * 2),
            &consumer.invocation,
            &StreamSessionRecordV1::TopologyActivated(StreamTopologyActivatedRecordV1 {
                format_version: 1,
                session_key: consumer.invocation.clone(),
                attachment: resumed_attachment.clone(),
                mapping: mapping.clone(),
            }),
        );
        assert_eq!(
            projection
                .topology_status(&resumed_attachment, Some(&mapping))
                .unwrap(),
            ConsumerAttachmentStatus::Active
        );
    }
    assert_eq!(
        projection
            .topology_status(&attachment, Some(&mapping))
            .unwrap(),
        ConsumerAttachmentStatus::EpochMismatch
    );
    assert!(
        projection.topology_error.is_none(),
        "successive epochs for the same attachment slot are not conflicting topology"
    );
    assert!(
        golem_common::serialization::serialize(&projection)
            .unwrap()
            .len()
            < initial_size + 1024,
        "topology metadata must not grow with resume count"
    );

    let mut conflicting = projection.clone();
    let mut conflicting_attachment = resumed_attachment.clone();
    conflicting_attachment.expected_consumer_fingerprint = AgentFingerprint::new();
    conflicting.apply(
        OplogIndex::from_u64(20_000),
        &consumer.invocation,
        &StreamSessionRecordV1::TopologyPrepared(StreamTopologyPreparedRecordV1 {
            format_version: 1,
            session_key: consumer.invocation.clone(),
            attachment: conflicting_attachment,
            mapping: mapping.clone(),
        }),
    );
    assert!(
        conflicting
            .topology_status(&resumed_attachment, Some(&mapping))
            .is_err(),
        "a conflicting same-epoch identity must not leave the old topology active"
    );

    remote_producer
        .write_items(
            handle.stream_id,
            0,
            StreamItemsPayloadV1::Values(vec![
                ProtoSchemaValue::try_from(SchemaValue::U32(42))
                    .unwrap()
                    .encode_to_vec(),
            ]),
        )
        .await
        .unwrap();
    remote_producer
        .end(handle.stream_id, 1, StreamEndResultV1::Ok)
        .await
        .unwrap();
    let (responses, mut response_stream) = mpsc::channel(2);
    assert!(
        streams
            .pump_output_stream_from(18, handle.clone(), None, &responses)
            .await
            .unwrap()
            .is_empty()
    );
    let Some(invocation_response::Response::OutputItem(item)) =
        response_stream.recv().await.unwrap().response
    else {
        panic!("forwarded output must be read through its attached producer")
    };
    assert_eq!(item.transport_stream_id, 18);
    assert_eq!(item.producer_sequence, 0);
    let Some(invocation_response::Response::OutputEnd(end)) =
        response_stream.recv().await.unwrap().response
    else {
        panic!("forwarded output terminal must be read through its attached producer")
    };
    assert_eq!(end.transport_stream_id, 18);
    assert_eq!(end.producer_sequence, 1);
    let producer_length_after_output = producer_oplog.current_oplog_index().await;

    let conflicting_mapping = StreamSessionMappingRecordV1 {
        transport_stream_id: 18,
        ..mapping
    };
    assert!(
        streams
            .activate_forwarded_mapping(
                attachment.clone(),
                conflicting_mapping,
                remote_producer.clone(),
                121,
            )
            .await
            .is_err()
    );
    assert_eq!(
        producer_oplog.current_oplog_index().await,
        producer_length_after_output
    );

    let restarted = DurableSessionStreams::new(
        consumer_producer,
        consumer_oplog.clone(),
        consumer.invocation,
        [],
    )
    .with_consumer_journal(Arc::new(TestConsumerJournal(consumer_oplog)));
    restarted.recover_session_mappings().await.unwrap();
    assert_eq!(restarted.handle(17), Some(handle));

    let mut future_epoch = attachment.clone();
    future_epoch.epoch = 2;
    assert_eq!(
        StreamAttachmentConsumerProbe::status(&restarted, &future_epoch)
            .await
            .unwrap(),
        ConsumerAttachmentStatus::EpochMismatch
    );

    let mut recreated = attachment.clone();
    recreated.expected_consumer_fingerprint = AgentFingerprint(Uuid::from_u128(24));
    assert_eq!(
        StreamAttachmentConsumerProbe::status(&restarted, &recreated)
            .await
            .unwrap(),
        ConsumerAttachmentStatus::IncarnationMismatch
    );

    let second_handle = remote_producer
        .register(ProducerRegistrationRequestV1 {
            coordinate: StreamRegistrationCoordinateV1::Root {
                invocation_id: producer_identity.invocation.clone(),
                root_kind: StreamRootKindV1::MethodResult,
                recursive_value_path: vec![StreamValuePathStepV1::ListElement(1)],
            },
            ..registration(
                &producer_identity,
                StreamRegistrationCoordinateV1::Root {
                    invocation_id: producer_identity.invocation.clone(),
                    root_kind: StreamRootKindV1::MethodResult,
                    recursive_value_path: Vec::new(),
                },
                StreamSourceKindV1::InvocationOutput,
            )
        })
        .await
        .unwrap()
        .value;
    let partial_attachment = StreamAttachmentKeyV1 {
        stream_id: second_handle.stream_id,
        epoch: 2,
        ..attachment.clone()
    };
    let partial_mapping = StreamSessionMappingRecordV1 {
        transport_stream_id: 19,
        handle: second_handle,
        role: SessionStreamRoleV1::Input,
    };
    restarted
        .append_record(StreamSessionRecordV1::TopologyPrepared(
            StreamTopologyPreparedRecordV1 {
                format_version: DURABLE_STREAM_FORMAT_VERSION,
                session_key: restarted.session_key.clone(),
                attachment: partial_attachment.clone(),
                mapping: partial_mapping.clone(),
            },
        ))
        .await;
    let consumer_length = restarted.oplog.current_oplog_index().await;
    let producer_length = producer_oplog.current_oplog_index().await;
    assert!(
        restarted
            .activate_forwarded_mapping(
                partial_attachment.clone(),
                partial_mapping.clone(),
                remote_producer.clone(),
                130,
            )
            .await
            .is_err()
    );
    assert_eq!(restarted.oplog.current_oplog_index().await, consumer_length);
    assert_eq!(producer_oplog.current_oplog_index().await, producer_length);
    assert!(restarted.handle(19).is_none());
    assert!(restarted.complete().await.is_err());
    restarted
        .append_record(StreamSessionRecordV1::TopologyActivated(
            StreamTopologyActivatedRecordV1 {
                format_version: DURABLE_STREAM_FORMAT_VERSION,
                session_key: restarted.session_key.clone(),
                attachment: partial_attachment,
                mapping: partial_mapping,
            },
        ))
        .await;
    assert!(restarted.complete().await.is_err());
}

#[test]
async fn local_topology_cannot_activate_before_exact_session_attachment() {
    let identity = identity();
    let oplog = Arc::new(TestOplog::default());
    let producer = DurableStreamProducer::load(
        oplog.clone(),
        identity.environment_id,
        identity.agent_id.clone(),
        identity.fingerprint,
        None,
    )
    .await
    .unwrap();
    let handle = producer
        .register(registration(
            &identity,
            StreamRegistrationCoordinateV1::Root {
                invocation_id: identity.invocation.clone(),
                root_kind: StreamRootKindV1::MethodInput,
                recursive_value_path: Vec::new(),
            },
            StreamSourceKindV1::AgentHostedInput,
        ))
        .await
        .unwrap()
        .value;
    let attachment_id = AttachmentId::primary(
        identity.environment_id,
        &identity.agent_id,
        &identity.invocation.idempotency_key,
    )
    .unwrap();
    let attempt_id = AttemptId::fresh();
    let mapping = StreamSessionMappingRecordV1 {
        transport_stream_id: 17,
        handle: handle.clone(),
        role: SessionStreamRoleV1::Input,
    };
    producer
        .append_session_record(StreamSessionRecordV1::Prepared(
            StreamSessionPreparedRecordV1 {
                format_version: DURABLE_STREAM_FORMAT_VERSION,
                attempt: StartAttemptDescriptorV1 {
                    format_version: DURABLE_STREAM_FORMAT_VERSION,
                    session_key: identity.invocation.clone(),
                    attachment_id,
                    expected_callee_fingerprint: identity.fingerprint,
                    attempt_id,
                    invocation: PersistedStreamInvocationDescriptorV1 {
                        format_version: DURABLE_STREAM_FORMAT_VERSION,
                        session_key: identity.invocation.clone(),
                        target_component_revision: ComponentRevision::INITIAL,
                        method_name: "consume".to_string(),
                        invocation_value: vec![1],
                        stream_handles: vec![handle.clone()],
                        execution_config: vec![2],
                        effective_identity: vec![3],
                    },
                    effective_identity: vec![3],
                    live_join_buffer_events: 8,
                },
                stream_mappings: vec![mapping.clone()],
            },
        ))
        .await
        .unwrap();
    let attachment = StreamAttachmentKeyV1 {
        attachment_id,
        stream_id: handle.stream_id,
        epoch: 1,
        session_key: identity.invocation.clone(),
        producer_environment_id: identity.environment_id,
        producer: identity.agent_id.clone(),
        expected_producer_fingerprint: identity.fingerprint,
        consumer_environment_id: identity.environment_id,
        consumer: identity.agent_id.clone(),
        expected_consumer_fingerprint: identity.fingerprint,
        consumer_invocation: identity.invocation.clone(),
    };
    let streams = DurableSessionStreams::new(
        producer.clone(),
        oplog.clone(),
        identity.invocation.clone(),
        [],
    )
    .with_consumer_journal(Arc::new(TestConsumerJournal(oplog.clone())));

    assert!(
        streams
            .activate_forwarded_mapping(attachment.clone(), mapping.clone(), producer.clone(), 100,)
            .await
            .is_err()
    );
    assert_eq!(
        streams
            .topology_state(&attachment, Some(&mapping))
            .await
            .unwrap(),
        ConsumerAttachmentStatus::Prepared
    );
    assert!(streams.handle(mapping.transport_stream_id).is_none());

    let pending_invocation_oplog_index = oplog
        .add(OplogEntry::pending_agent_invocation(
            identity.invocation.idempotency_key.clone(),
            OplogPayload::Inline(Box::new(AgentInvocationPayload::SaveSnapshot)),
            TraceId::generate(),
            Vec::new(),
            Vec::new(),
        ))
        .await;
    producer
        .append_session_record(StreamSessionRecordV1::Attached(
            StreamSessionAttachedRecordV1 {
                format_version: DURABLE_STREAM_FORMAT_VERSION,
                session_key: identity.invocation,
                attachment_id,
                attempt_id,
                epoch: 1,
                pending_invocation_oplog_index,
            },
        ))
        .await
        .unwrap();
    streams
        .activate_forwarded_mapping(attachment.clone(), mapping.clone(), producer.clone(), 110)
        .await
        .unwrap();
    assert_eq!(
        streams
            .topology_state(&attachment, Some(&mapping))
            .await
            .unwrap(),
        ConsumerAttachmentStatus::Active
    );
    assert_eq!(streams.handle(mapping.transport_stream_id), Some(handle));
}

#[test]
async fn oversized_remote_result_is_rejected_before_any_session_write() {
    let identity = identity();
    let oplog = Arc::new(TestOplog::default());
    let producer = DurableStreamProducer::load(
        oplog.clone(),
        identity.environment_id,
        identity.agent_id.clone(),
        identity.fingerprint,
        None,
    )
    .await
    .unwrap();
    let base_handle = producer
        .register(registration(
            &identity,
            StreamRegistrationCoordinateV1::Root {
                invocation_id: identity.invocation.clone(),
                root_kind: StreamRootKindV1::MethodResult,
                recursive_value_path: Vec::new(),
            },
            StreamSourceKindV1::InvocationOutput,
        ))
        .await
        .unwrap()
        .value;
    let streams = DurableSessionStreams::new(producer, oplog.clone(), identity.invocation, []);
    let stream_count = MAX_NEW_STREAM_HANDLES_PER_VALUE + 1;
    let mut mappings = Vec::with_capacity(stream_count);
    let mut elements = Vec::with_capacity(stream_count);
    for position in 0..stream_count {
        let mut handle = base_handle.clone();
        handle.stream_id = golem_common::base_model::durable_stream::StreamId(Uuid::from_u128(
            10_000 + position as u128,
        ));
        mappings.push(StreamSessionMappingRecordV1 {
            transport_stream_id: position as u64,
            handle,
            role: SessionStreamRoleV1::Output,
        });
        elements.push(ProtoSchemaValue {
            value: Some(schema_value::Value::StreamReference(
                SchemaValueStreamReference {
                    stream_id: position as u64,
                },
            )),
        });
    }
    let value = ProtoSchemaValue {
        value: Some(schema_value::Value::ListValue(ListValue { elements })),
    };
    let element = SchemaType::u8();
    let root = SchemaType::list(SchemaType::stream(Some(element)));
    let graph = SchemaGraph::anonymous(root.clone());
    let before = oplog.current_oplog_index().await;

    let error = streams
        .materialize_remote_result(value, mappings, &graph, &root)
        .await
        .unwrap_err();

    assert!(error.contains("ResourceExhausted"));
    assert_eq!(oplog.current_oplog_index().await, before);
}

#[test]
async fn remote_result_schema_mismatch_is_rejected_before_caller_mutation() {
    let identity = identity();
    let oplog = Arc::new(TestOplog::default());
    let producer = DurableStreamProducer::load(
        oplog.clone(),
        identity.environment_id,
        identity.agent_id.clone(),
        identity.fingerprint,
        None,
    )
    .await
    .unwrap();
    let wrong_element = SchemaType::string();
    let wrong_graph = SchemaGraph::anonymous(wrong_element.clone());
    let mut request = registration(
        &identity,
        StreamRegistrationCoordinateV1::Root {
            invocation_id: identity.invocation.clone(),
            root_kind: StreamRootKindV1::MethodResult,
            recursive_value_path: Vec::new(),
        },
        StreamSourceKindV1::InvocationOutput,
    );
    request.element_schema_fingerprint =
        schema_fingerprint_v1(&wrong_graph, Some(&wrong_element)).unwrap();
    let handle = producer.register(request).await.unwrap().value;
    let streams = DurableSessionStreams::new(producer, oplog.clone(), identity.invocation, []);
    let expected_root = SchemaType::stream(Some(SchemaType::u32()));
    let expected_graph = SchemaGraph::anonymous(expected_root.clone());
    let value = ProtoSchemaValue {
        value: Some(schema_value::Value::StreamReference(
            SchemaValueStreamReference { stream_id: 7 },
        )),
    };
    let before = oplog.current_oplog_index().await;

    let error = streams
        .materialize_remote_result(
            value,
            vec![StreamSessionMappingRecordV1 {
                transport_stream_id: 7,
                handle: handle.clone(),
                role: SessionStreamRoleV1::Output,
            }],
            &expected_graph,
            &expected_root,
        )
        .await
        .unwrap_err();

    assert!(error.contains("wrong schema fingerprint"));
    assert_eq!(oplog.current_oplog_index().await, before);
    assert!(
        streams
            .mapping_for_handle(&handle, SessionStreamRoleV1::Output)
            .is_none()
    );
    assert!(streams.remote_result_record().await.unwrap().is_none());
}

#[test]
async fn remote_result_schema_validation_accepts_a_stream_in_union_branch_one() {
    let identity = identity();
    let oplog = Arc::new(TestOplog::default());
    let producer = DurableStreamProducer::load(
        oplog.clone(),
        identity.environment_id,
        identity.agent_id.clone(),
        identity.fingerprint,
        None,
    )
    .await
    .unwrap();
    let root = union_with_stream_in_second_branch();
    let graph = SchemaGraph::anonymous(root.clone());
    let element = SchemaType::u32();
    let mut request = registration(
        &identity,
        StreamRegistrationCoordinateV1::Root {
            invocation_id: identity.invocation.clone(),
            root_kind: StreamRootKindV1::MethodResult,
            recursive_value_path: vec![
                StreamValuePathStepV1::UnionBranch(1),
                StreamValuePathStepV1::RecordField(1),
            ],
        },
        StreamSourceKindV1::InvocationOutput,
    );
    request.element_schema_fingerprint = schema_fingerprint_v1(&graph, Some(&element)).unwrap();
    let handle = producer.register(request).await.unwrap().value;
    let streams = DurableSessionStreams::new(producer, oplog.clone(), identity.invocation, [])
        .with_consumer_journal(Arc::new(TestConsumerJournal(oplog)));
    let value = encode_recursive_stream_value_with_schema(
        &stream_union_value(SchemaValueStream::from_host_endpoint(())),
        &graph,
        &root,
        |_, path| {
            assert_eq!(
                path,
                [
                    StreamValuePathStepV1::UnionBranch(1),
                    StreamValuePathStepV1::RecordField(1),
                ]
            );
            Ok(11)
        },
    )
    .unwrap();

    let result = streams
        .materialize_remote_result(
            value,
            vec![StreamSessionMappingRecordV1 {
                transport_stream_id: 11,
                handle: handle.clone(),
                role: SessionStreamRoleV1::Output,
            }],
            &graph,
            &root,
        )
        .await
        .unwrap();

    let SchemaValue::Union(result) = result else {
        panic!("expected union result")
    };
    assert_eq!(result.tag, "stream");
    assert_eq!(
        streams
            .mapping_for_handle(&handle, SessionStreamRoleV1::Output)
            .unwrap()
            .role,
        SessionStreamRoleV1::Output
    );
}

#[test]
async fn output_catch_up_persists_a_missing_nested_transport_mapping_before_emitting() {
    let identity = identity();
    let attachment_id = AttachmentId::primary(
        identity.environment_id,
        &identity.agent_id,
        &identity.invocation.idempotency_key,
    )
    .unwrap();
    let oplog = Arc::new(TestOplog::default());
    let producer = DurableStreamProducer::load(
        oplog.clone(),
        identity.environment_id,
        identity.agent_id.clone(),
        identity.fingerprint,
        None,
    )
    .await
    .unwrap();
    let session_key = identity.invocation.clone();
    let mut root_request = registration(
        &identity,
        StreamRegistrationCoordinateV1::Root {
            invocation_id: session_key.clone(),
            root_kind: StreamRootKindV1::MethodResult,
            recursive_value_path: Vec::new(),
        },
        StreamSourceKindV1::InvocationOutput,
    );
    root_request.session_mapping = Some(StreamSessionMappingV1 {
        session_key: session_key.clone(),
        attachment_id,
        role: SessionStreamRoleV1::Output,
    });
    let root = producer.register(root_request).await.unwrap().value;
    let nested_request = registration(
        &identity,
        StreamRegistrationCoordinateV1::Nested {
            parent_stream_id: root.stream_id,
            parent_producer_sequence: 0,
            recursive_value_path: Vec::new(),
        },
        StreamSourceKindV1::Nested,
    );
    let canonical_value = ProtoSchemaValue {
        value: Some(schema_value::Value::StreamReference(
            SchemaValueStreamReference { stream_id: 0 },
        )),
    };
    let root_written = producer
        .write_items_with_nested(
            root.stream_id,
            0,
            StreamItemsPayloadV1::Values(vec![canonical_value.encode_to_vec()]),
            vec![nested_request],
        )
        .await
        .unwrap();
    let root_ended = producer
        .end(root.stream_id, 1, StreamEndResultV1::Ok)
        .await
        .unwrap();
    let nested = producer.nested_handles(root.stream_id, 0).await.unwrap()[0].clone();
    let nested_written = producer
        .write_items(
            nested.stream_id,
            0,
            StreamItemsPayloadV1::PackedU8(vec![10, 11]),
        )
        .await
        .unwrap();
    producer
        .end(nested.stream_id, 2, StreamEndResultV1::Ok)
        .await
        .unwrap();
    let attempt_id = AttemptId::fresh();
    producer
        .append_session_record(StreamSessionRecordV1::Prepared(
            StreamSessionPreparedRecordV1 {
                format_version: DURABLE_STREAM_FORMAT_VERSION,
                attempt: StartAttemptDescriptorV1 {
                    format_version: DURABLE_STREAM_FORMAT_VERSION,
                    session_key: session_key.clone(),
                    attachment_id,
                    expected_callee_fingerprint: identity.fingerprint,
                    attempt_id,
                    invocation: PersistedStreamInvocationDescriptorV1 {
                        format_version: DURABLE_STREAM_FORMAT_VERSION,
                        session_key: session_key.clone(),
                        target_component_revision: ComponentRevision::INITIAL,
                        method_name: "produce".to_string(),
                        invocation_value: vec![1],
                        stream_handles: Vec::new(),
                        execution_config: vec![2],
                        effective_identity: vec![3],
                    },
                    effective_identity: vec![3],
                    live_join_buffer_events: 8,
                },
                stream_mappings: Vec::new(),
            },
        ))
        .await
        .unwrap();
    let pending_invocation_oplog_index = oplog
        .add(OplogEntry::pending_agent_invocation(
            session_key.idempotency_key.clone(),
            OplogPayload::Inline(Box::new(AgentInvocationPayload::SaveSnapshot)),
            TraceId::generate(),
            Vec::new(),
            Vec::new(),
        ))
        .await;
    let streams = DurableSessionStreams::new(
        producer.clone(),
        oplog.clone(),
        session_key.clone(),
        [(7, root.clone(), SessionStreamRoleV1::Output)],
    )
    .with_consumer_journal(Arc::new(TestConsumerJournal(oplog.clone())));
    streams
        .append_record(StreamSessionRecordV1::Attached(
            golem_common::base_model::durable_stream::StreamSessionAttachedRecordV1 {
                format_version: DURABLE_STREAM_FORMAT_VERSION,
                session_key: session_key.clone(),
                attachment_id,
                attempt_id,
                epoch: 1,
                pending_invocation_oplog_index,
            },
        ))
        .await;
    let streams = streams.with_attachment(1, attempt_id);
    assert!(
        streams
            .mapping_for_handle(&nested, SessionStreamRoleV1::Output)
            .is_none()
    );
    let (responses, mut receiver) = mpsc::channel(4);
    let discovered = streams
        .pump_output_stream_from(7, root.clone(), None, &responses)
        .await
        .unwrap();
    let [(nested_transport_stream_id, discovered_nested)] = discovered.as_slice() else {
        panic!("catch-up must discover exactly one nested output stream")
    };
    assert_eq!(discovered_nested, &nested);
    let item = receiver.recv().await.unwrap();
    let Some(invocation_response::Response::OutputItem(item)) = item.response else {
        panic!("catch-up must emit the enclosing output item first")
    };
    assert_eq!(item.new_stream_mappings.len(), 1);
    assert_eq!(
        item.new_stream_mappings[0].transport_stream_id,
        *nested_transport_stream_id
    );

    let (packed_responses, mut packed_receiver) = mpsc::channel(2);
    streams
        .pump_output_stream_from(
            *nested_transport_stream_id,
            nested.clone(),
            None,
            &packed_responses,
        )
        .await
        .unwrap();
    let Some(invocation_response::Response::OutputItem(packed)) =
        packed_receiver.recv().await.unwrap().response
    else {
        panic!("packed durable bytes must produce an output item")
    };
    assert_eq!(packed.producer_sequence, 0);
    assert_eq!(packed.logical_item_count, 2);
    assert_eq!(packed.packed_u8, vec![10, 11]);
    assert!(packed.value.is_none());
    assert!(packed.new_stream_mappings.is_empty());
    assert_eq!(
        packed.durable_offset,
        nested_written.value[1].as_bytes().to_vec()
    );
    let Some(invocation_response::Response::OutputEnd(end)) =
        packed_receiver.recv().await.unwrap().response
    else {
        panic!("packed durable bytes must preserve their terminal")
    };
    assert_eq!(end.producer_sequence, 2);

    let mut lone_request = registration(
        &identity,
        StreamRegistrationCoordinateV1::Root {
            invocation_id: session_key.clone(),
            root_kind: StreamRootKindV1::MethodResult,
            recursive_value_path: vec![StreamValuePathStepV1::RecordField(1)],
        },
        StreamSourceKindV1::InvocationOutput,
    );
    lone_request.session_mapping = Some(StreamSessionMappingV1 {
        session_key: session_key.clone(),
        attachment_id,
        role: SessionStreamRoleV1::Output,
    });
    let lone = producer.register(lone_request).await.unwrap().value;
    let lone_transport_stream_id = streams.allocate_transport_stream_id().unwrap();
    streams
        .insert_mapping(
            lone_transport_stream_id,
            lone.clone(),
            SessionStreamRoleV1::Output,
        )
        .unwrap();
    let (lone_responses, mut lone_receiver) = mpsc::channel(2);
    let lone_streams = streams.clone();
    let lone_for_pump = lone.clone();
    let lone_pump = tokio::spawn(async move {
        lone_streams
            .pump_output_stream_from(
                lone_transport_stream_id,
                lone_for_pump,
                None,
                &lone_responses,
            )
            .await
    });
    let lone_written = producer
        .write_items(lone.stream_id, 0, StreamItemsPayloadV1::PackedU8(vec![99]))
        .await
        .unwrap();
    let lone_item = tokio::time::timeout(
        PACKED_U8_OUTPUT_FLUSH_DELAY + Duration::from_millis(100),
        lone_receiver.recv(),
    )
    .await
    .expect("a lone packed byte exceeded the bounded output flush delay")
    .unwrap();
    let Some(invocation_response::Response::OutputItem(lone_item)) = lone_item.response else {
        panic!("a lone packed byte must produce an output item")
    };
    assert_eq!(lone_item.packed_u8, vec![99]);
    assert_eq!(lone_item.logical_item_count, 1);
    assert_eq!(
        lone_item.durable_offset,
        lone_written.value[0].as_bytes().to_vec()
    );
    producer
        .end(lone.stream_id, 1, StreamEndResultV1::Ok)
        .await
        .unwrap();
    assert!(matches!(
        lone_receiver.recv().await.unwrap().response,
        Some(invocation_response::Response::OutputEnd(_))
    ));
    lone_pump.await.unwrap().unwrap();

    let restarted = DurableSessionStreams::new(
        producer,
        oplog.clone(),
        session_key,
        [(7, root.clone(), SessionStreamRoleV1::Output)],
    )
    .with_consumer_journal(Arc::new(TestConsumerJournal(oplog)))
    .with_attachment(1, attempt_id);
    restarted.recover_session_mappings().await.unwrap();
    assert_eq!(
        restarted
            .mapping_for_handle(&nested, SessionStreamRoleV1::Output)
            .unwrap()
            .transport_stream_id,
        *nested_transport_stream_id
    );
    let nested_transport_stream_id = *nested_transport_stream_id;
    let cursors = HashMap::from([
        (root.stream_id, Some(root_written.value[0])),
        (nested.stream_id, Some(nested_written.value[0])),
    ]);
    let (responses, mut receiver) = mpsc::channel(8);
    restarted
        .pump_output_streams_from(&cursors, &[7], &[7, nested_transport_stream_id], &responses)
        .await
        .unwrap();
    drop(responses);
    let mut replayed_nested_offsets = Vec::new();
    let mut ended_streams = HashSet::new();
    while let Some(response) = receiver.recv().await {
        match response.response {
            Some(invocation_response::Response::OutputItem(item)) => {
                assert_eq!(item.transport_stream_id, nested_transport_stream_id);
                replayed_nested_offsets.push(item.durable_offset);
            }
            Some(invocation_response::Response::OutputEnd(end)) => {
                ended_streams.insert(end.transport_stream_id);
            }
            other => panic!("unexpected resumed nested output response: {other:?}"),
        }
    }
    assert_eq!(
        replayed_nested_offsets,
        vec![nested_written.value[1].as_bytes().to_vec()]
    );
    assert_eq!(
        ended_streams,
        HashSet::from([7, nested_transport_stream_id])
    );

    let terminal_parent_cursors = HashMap::from([
        (root.stream_id, Some(root_ended.value)),
        (nested.stream_id, Some(nested_written.value[0])),
    ]);
    let (responses, mut receiver) = mpsc::channel(8);
    tokio::time::timeout(
        Duration::from_secs(1),
        restarted.pump_output_streams_from(
            &terminal_parent_cursors,
            &[7],
            &[7, nested_transport_stream_id],
            &responses,
        ),
    )
    .await
    .expect("resume from a parent terminal cursor must complete")
    .unwrap();
    drop(responses);
    let mut replayed_nested = Vec::new();
    let mut ended_streams = HashSet::new();
    while let Some(response) = receiver.recv().await {
        match response.response {
            Some(invocation_response::Response::OutputItem(item)) => {
                replayed_nested.push((item.transport_stream_id, item.packed_u8));
            }
            Some(invocation_response::Response::OutputEnd(end)) => {
                ended_streams.insert(end.transport_stream_id);
            }
            other => panic!("unexpected terminal-parent resume response: {other:?}"),
        }
    }
    assert_eq!(
        replayed_nested,
        vec![(nested_transport_stream_id, vec![11])]
    );
    assert_eq!(ended_streams, HashSet::from([nested_transport_stream_id]));
}

#[test]
async fn resumed_foreign_parent_and_nested_output_cursors_use_the_accepted_epoch() {
    let consumer = identity();
    let producer_identity = TestIdentity {
        environment_id: EnvironmentId(Uuid::from_u128(71)),
        agent_id: AgentId {
            component_id: ComponentId(Uuid::from_u128(72)),
            agent_id: "resumed-foreign-producer".to_string(),
        },
        fingerprint: AgentFingerprint(Uuid::from_u128(73)),
        invocation: StreamInvocationIdV1 {
            callee_environment_id: EnvironmentId(Uuid::from_u128(71)),
            callee: AgentId {
                component_id: ComponentId(Uuid::from_u128(72)),
                agent_id: "resumed-foreign-producer".to_string(),
            },
            callee_fingerprint: AgentFingerprint(Uuid::from_u128(73)),
            idempotency_key: IdempotencyKey::new("resumed-foreign-producer-invocation".to_string()),
        },
    };
    let producer_oplog = Arc::new(TestOplog::default());
    let remote_producer = DurableStreamProducer::load(
        producer_oplog,
        producer_identity.environment_id,
        producer_identity.agent_id.clone(),
        producer_identity.fingerprint,
        None,
    )
    .await
    .unwrap();
    let root = remote_producer
        .register(registration(
            &producer_identity,
            StreamRegistrationCoordinateV1::Root {
                invocation_id: producer_identity.invocation.clone(),
                root_kind: StreamRootKindV1::MethodResult,
                recursive_value_path: Vec::new(),
            },
            StreamSourceKindV1::InvocationOutput,
        ))
        .await
        .unwrap()
        .value;
    let root_written = remote_producer
        .write_items_with_nested(
            root.stream_id,
            0,
            StreamItemsPayloadV1::Values(vec![
                ProtoSchemaValue {
                    value: Some(schema_value::Value::StreamReference(
                        SchemaValueStreamReference { stream_id: 0 },
                    )),
                }
                .encode_to_vec(),
            ]),
            vec![registration(
                &producer_identity,
                StreamRegistrationCoordinateV1::Nested {
                    parent_stream_id: root.stream_id,
                    parent_producer_sequence: 0,
                    recursive_value_path: Vec::new(),
                },
                StreamSourceKindV1::Nested,
            )],
        )
        .await
        .unwrap();
    remote_producer
        .end(root.stream_id, 1, StreamEndResultV1::Ok)
        .await
        .unwrap();
    let nested = remote_producer
        .nested_handles(root.stream_id, 0)
        .await
        .unwrap()[0]
        .clone();
    let nested_written = remote_producer
        .write_items(
            nested.stream_id,
            0,
            StreamItemsPayloadV1::PackedU8(vec![10, 11]),
        )
        .await
        .unwrap();
    remote_producer
        .end(nested.stream_id, 2, StreamEndResultV1::Ok)
        .await
        .unwrap();

    let consumer_oplog = Arc::new(TestOplog::default());
    let consumer_producer = DurableStreamProducer::load(
        consumer_oplog.clone(),
        consumer.environment_id,
        consumer.agent_id.clone(),
        consumer.fingerprint,
        None,
    )
    .await
    .unwrap();
    let attachment_id = AttachmentId::primary(
        consumer.environment_id,
        &consumer.agent_id,
        &consumer.invocation.idempotency_key,
    )
    .unwrap();
    let start_attempt_id = AttemptId::fresh();
    let streams = DurableSessionStreams::new(
        consumer_producer,
        consumer_oplog.clone(),
        consumer.invocation.clone(),
        [],
    )
    .with_consumer_journal(Arc::new(TestConsumerJournal(consumer_oplog.clone())))
    .with_rpc(Arc::new(AttachedProducerRpc {
        producer: remote_producer.clone(),
        cancellation_owner: None,
        stall_next_cancel: Default::default(),
        scripted_reads: Mutex::default(),
        pending_read: Mutex::default(),
        read_requests: Mutex::default(),
    }))
    .with_auth_ctx(AuthCtx::System);
    streams
        .append_record(StreamSessionRecordV1::Prepared(
            StreamSessionPreparedRecordV1 {
                format_version: DURABLE_STREAM_FORMAT_VERSION,
                attempt: StartAttemptDescriptorV1 {
                    format_version: DURABLE_STREAM_FORMAT_VERSION,
                    session_key: consumer.invocation.clone(),
                    attachment_id,
                    expected_callee_fingerprint: consumer.fingerprint,
                    attempt_id: start_attempt_id,
                    invocation: PersistedStreamInvocationDescriptorV1 {
                        format_version: DURABLE_STREAM_FORMAT_VERSION,
                        session_key: consumer.invocation.clone(),
                        target_component_revision: ComponentRevision::INITIAL,
                        method_name: "resume-foreign".to_string(),
                        invocation_value: vec![1],
                        stream_handles: Vec::new(),
                        execution_config: vec![2],
                        effective_identity: vec![3],
                    },
                    effective_identity: vec![3],
                    live_join_buffer_events: 8,
                },
                stream_mappings: Vec::new(),
            },
        ))
        .await;
    let pending_invocation_oplog_index = consumer_oplog
        .add(OplogEntry::pending_agent_invocation(
            consumer.invocation.idempotency_key.clone(),
            OplogPayload::Inline(Box::new(AgentInvocationPayload::SaveSnapshot)),
            TraceId::generate(),
            Vec::new(),
            Vec::new(),
        ))
        .await;
    streams
        .append_record(StreamSessionRecordV1::Attached(
            StreamSessionAttachedRecordV1 {
                format_version: DURABLE_STREAM_FORMAT_VERSION,
                session_key: consumer.invocation.clone(),
                attachment_id,
                attempt_id: start_attempt_id,
                epoch: 1,
                pending_invocation_oplog_index,
            },
        ))
        .await;
    let epoch1 = streams.with_attachment(1, start_attempt_id);
    let root_mapping = StreamSessionMappingRecordV1 {
        transport_stream_id: 17,
        handle: root.clone(),
        role: SessionStreamRoleV1::Output,
    };
    let nested_mapping = StreamSessionMappingRecordV1 {
        transport_stream_id: 18,
        handle: nested.clone(),
        role: SessionStreamRoleV1::Output,
    };
    for mapping in [&root_mapping, &nested_mapping] {
        let attachment = epoch1.attachment_key(&mapping.handle, 1).unwrap();
        epoch1
            .activate_forwarded_mapping(attachment, mapping.clone(), remote_producer.clone(), 100)
            .await
            .unwrap();
    }
    let resumed_attempt_id = AttemptId::fresh();
    epoch1
        .commit_resume_attempt(StreamSessionResumeAttemptRecordV1 {
            format_version: DURABLE_STREAM_FORMAT_VERSION,
            attempt: ResumeAttemptDescriptorV1 {
                format_version: DURABLE_STREAM_FORMAT_VERSION,
                operation: StreamResumeOperationV1::Takeover,
                session_key: consumer.invocation.clone(),
                attachment_id,
                expected_callee_fingerprint: consumer.fingerprint,
                attempt_id: resumed_attempt_id,
                expected_epoch: 1,
                effective_identity: vec![3],
                cursors: Vec::new(),
                live_join_buffer_events: 8,
            },
            accepted_epoch: 2,
        })
        .await
        .unwrap();
    let resumed = epoch1.with_attachment(2, resumed_attempt_id);
    for mapping in [&root_mapping, &nested_mapping] {
        let attachment = resumed.attachment_key(&mapping.handle, 2).unwrap();
        resumed
            .activate_forwarded_mapping(attachment, mapping.clone(), remote_producer.clone(), 200)
            .await
            .unwrap();
    }

    let cursors = HashMap::from([
        (root.stream_id, Some(root_written.value[0])),
        (nested.stream_id, Some(nested_written.value[0])),
    ]);
    let (responses, mut receiver) = mpsc::channel(8);
    resumed
        .pump_output_streams_from(
            &cursors,
            &[root_mapping.transport_stream_id],
            &[
                root_mapping.transport_stream_id,
                nested_mapping.transport_stream_id,
            ],
            &responses,
        )
        .await
        .unwrap();
    drop(responses);
    let mut replayed_items = Vec::new();
    let mut ended_streams = HashSet::new();
    while let Some(response) = receiver.recv().await {
        match response.response {
            Some(invocation_response::Response::OutputItem(item)) => {
                replayed_items.push((item.transport_stream_id, item.packed_u8));
            }
            Some(invocation_response::Response::OutputEnd(end)) => {
                ended_streams.insert(end.transport_stream_id);
            }
            other => panic!("unexpected resumed foreign output response: {other:?}"),
        }
    }
    assert_eq!(
        replayed_items,
        vec![(nested_mapping.transport_stream_id, vec![11])]
    );
    assert_eq!(
        ended_streams,
        HashSet::from([
            root_mapping.transport_stream_id,
            nested_mapping.transport_stream_id,
        ])
    );
}

#[test]
fn session_control_metadata_keeps_cancellation_scoped_to_its_session() {
    use golem_common::model::durable_stream::{
        StreamSessionCancelRequestedRecordV1, StreamSlotTombstonedRecordV1,
    };

    let key = identity().invocation;
    let mut other = key.clone();
    other.idempotency_key = golem_common::model::IdempotencyKey::new("other-session".into());
    let mut metadata = SessionControlMetadata::default();
    let owner = golem_common::model::OwnedAgentId::new(key.callee_environment_id, &key.callee);
    metadata.finished = Some(OplogIndex::INITIAL);
    assert!(!metadata.needs_recovery(&owner, &key));
    for (position, session_key) in [other, key.clone()].into_iter().enumerate() {
        metadata.apply(
            OplogIndex::from_u64(1 + position as u64 * 2),
            &key,
            &StreamSessionRecordV1::CancelRequested(StreamSessionCancelRequestedRecordV1 {
                format_version: DURABLE_STREAM_FORMAT_VERSION,
                session_key: session_key.clone(),
            }),
        );
        metadata.apply(
            OplogIndex::from_u64(2 + position as u64 * 2),
            &key,
            &StreamSessionRecordV1::Tombstoned(StreamSlotTombstonedRecordV1 {
                format_version: DURABLE_STREAM_FORMAT_VERSION,
                session_key,
                slot: "$result".into(),
                role: SessionStreamRoleV1::Output,
            }),
        );
        assert_eq!(metadata.cancellation_requested, position == 1);
        assert_eq!(
            metadata.tombstoned_slots.contains_key("$result"),
            position == 1
        );
        assert!(!metadata.needs_recovery(&owner, &key));
    }
    let encoded = golem_common::serialization::serialize(&metadata).unwrap();
    let restored: SessionControlMetadata =
        golem_common::serialization::deserialize(&encoded).unwrap();
    assert!(restored.cancellation_requested);
    assert_eq!(
        restored.tombstoned_slots,
        HashMap::from([("$result".into(), SessionStreamRoleV1::Output)])
    );
    assert_eq!(restored.covered_through, OplogIndex::from_u64(4));
}

#[test]
fn cancellation_applied_receipt_clears_only_the_exact_intent() {
    let key = identity().invocation;
    let owner = golem_common::model::OwnedAgentId::new(key.callee_environment_id, &key.callee);
    let intent = StreamConsumerCancelIntentRecordV1 {
        format_version: DURABLE_STREAM_FORMAT_VERSION,
        session_key: key.clone(),
        stream_id: golem_common::model::StreamId(uuid::Uuid::new_v4()),
        epoch: 3,
        role: StreamCancelRoleV1::OutputConsumer,
        reason: StreamCancelReasonV1::Cancelled,
        details: None,
    };
    let mut metadata = SessionControlMetadata {
        finished: Some(OplogIndex::INITIAL),
        ..Default::default()
    };
    metadata.apply(
        OplogIndex::from_u64(1),
        &key,
        &StreamSessionRecordV1::ConsumerCancelIntent(intent.clone()),
    );
    assert!(metadata.needs_recovery(&owner, &key));

    let mut stale = intent.clone();
    stale.epoch += 1;
    metadata.apply(
        OplogIndex::from_u64(2),
        &key,
        &StreamSessionRecordV1::ConsumerCancelApplied(StreamConsumerCancelAppliedRecordV1 {
            format_version: DURABLE_STREAM_FORMAT_VERSION,
            intent: stale,
        }),
    );
    assert!(metadata.needs_recovery(&owner, &key));
    metadata.apply(
        OplogIndex::from_u64(3),
        &key,
        &StreamSessionRecordV1::ConsumerCancelApplied(StreamConsumerCancelAppliedRecordV1 {
            format_version: DURABLE_STREAM_FORMAT_VERSION,
            intent,
        }),
    );
    assert!(!metadata.needs_recovery(&owner, &key));
    let encoded = golem_common::serialization::serialize(&metadata).unwrap();
    let restored: SessionControlMetadata =
        golem_common::serialization::deserialize(&encoded).unwrap();
    assert!(!restored.needs_recovery(&owner, &key));
    assert_eq!(restored.cancel_intents.len(), 1);
    assert_eq!(restored.applied_cancel_intents.len(), 1);
}

#[test]
async fn session_control_metadata_pages_history_and_reads_only_raw_suffix_after_warmup() {
    let identity = identity();
    let oplog = Arc::new(TestOplog::default());
    let producer = DurableStreamProducer::load(
        oplog.clone(),
        identity.environment_id,
        identity.agent_id.clone(),
        identity.fingerprint,
        None,
    )
    .await
    .unwrap();
    let streams =
        DurableSessionStreams::new(producer, oplog.clone(), identity.invocation.clone(), []);
    for _ in 0..2050 {
        oplog.add(OplogEntry::interrupted()).await;
    }
    assert!(
        streams
            .session_root_output_mapping_ids()
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        oplog.take_read_ranges(),
        vec![
            (OplogIndex::INITIAL, 1024),
            (OplogIndex::from_u64(1025), 1024),
            (OplogIndex::from_u64(2049), 2),
        ]
    );
    assert!(streams.persisted_finished().await.unwrap().is_none());
    assert!(streams.remote_result_record().await.unwrap().is_none());
    assert!(oplog.take_read_ranges().is_empty());

    let attempt_id = AttemptId::fresh();
    let index = oplog
        .add(OplogEntry::StreamSession {
            timestamp: Timestamp::now_utc(),
            entity_parent_start_index: None,
            record: OplogPayload::Inline(Box::new(StreamSessionRecordV1::CallerAttempt(
                StreamCallerAttemptRecordV1 {
                    format_version: DURABLE_STREAM_FORMAT_VERSION,
                    session_key: identity.invocation,
                    attempt_id,
                },
            ))),
        })
        .await;
    // No commit: another local append must already be visible.
    assert_eq!(streams.caller_attempt_id().await.unwrap(), attempt_id);
    assert_eq!(oplog.take_read_ranges(), vec![(index, 1)]);
    oplog.commit(CommitLevel::Always).await;
    assert_eq!(
        streams.clone().caller_attempt_id().await.unwrap(),
        attempt_id
    );
    assert!(oplog.take_read_ranges().is_empty());
}

#[test]
async fn session_mapping_recovery_pages_once_and_shares_coverage_with_clones() {
    let identity = identity();
    let oplog = Arc::new(TestOplog::default());
    let producer = DurableStreamProducer::load(
        oplog.clone(),
        identity.environment_id,
        identity.agent_id.clone(),
        identity.fingerprint,
        None,
    )
    .await
    .unwrap();
    let streams = DurableSessionStreams::new(producer, oplog.clone(), identity.invocation, []);
    for _ in 0..2050 {
        oplog.add(OplogEntry::interrupted()).await;
    }
    streams.recover_session_mappings().await.unwrap();
    assert_eq!(
        oplog.take_read_ranges().iter().map(|(_, n)| n).sum::<u64>(),
        2050
    );
    streams.clone().recover_session_mappings().await.unwrap();
    assert!(oplog.take_read_ranges().is_empty());
    let next = oplog.add(OplogEntry::interrupted()).await;
    streams.recover_session_mappings().await.unwrap();
    assert_eq!(oplog.take_read_ranges(), vec![(next, 1)]);
}

#[test]
async fn suspended_control_metadata_reader_does_not_reserve_the_shared_permit() {
    let identity = identity();
    let oplog = Arc::new(TestOplog::default());
    let producer = DurableStreamProducer::load(
        oplog.clone(),
        identity.environment_id,
        identity.agent_id.clone(),
        identity.fingerprint,
        None,
    )
    .await
    .unwrap();
    let streams = DurableSessionStreams::new(producer, oplog, identity.invocation, []);

    let guard = streams.control_metadata.lock().await;
    let mut suspended = Box::pin(streams.current_control_metadata());
    assert!(futures::poll!(&mut suspended).is_pending());
    drop(guard);

    let metadata = tokio::time::timeout(Duration::from_secs(1), streams.current_control_metadata())
        .await
        .expect("a suspended reader must not block an independently polled lookup")
        .unwrap();
    drop(metadata);
    drop(suspended);
}

#[test]
async fn invocation_winner_drops_queued_persisted_result_waiter_before_followup() {
    let identity = identity();
    let oplog = Arc::new(TestOplog::default());
    let producer = DurableStreamProducer::load(
        oplog.clone(),
        identity.environment_id,
        identity.agent_id.clone(),
        identity.fingerprint,
        None,
    )
    .await
    .unwrap();
    let streams = DurableSessionStreams::new(producer, oplog, identity.invocation, []);

    let guard = streams.control_metadata.lock().await;
    let mut invocation = Box::pin(streams.recover_nested_input_mappings());
    assert!(futures::poll!(&mut invocation).is_pending());
    let mut result = Box::pin(streams.wait_persisted_result());
    assert!(futures::poll!(&mut result).is_pending());
    drop(guard);

    enum Winner {
        Invocation,
        Result,
    }
    let winner = tokio::select! {
        biased;
        _ = &mut result => Winner::Result,
        recovered = &mut invocation => {
            recovered.unwrap();
            Winner::Invocation
        },
    };
    assert!(matches!(winner, Winner::Invocation));

    drop(result);
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(1), streams.persisted_result())
            .await
            .expect("the invocation follow-up must not remain behind the abandoned waiter")
            .unwrap(),
        None
    );
}

#[test]
async fn finalization_after_retirement_requires_matching_committed_finished() {
    struct FinishedJournal {
        index: Option<OplogIndex>,
        hide_first: bool,
        reads: AtomicU64,
    }

    #[async_trait::async_trait]
    impl DurableStreamConsumerJournal for FinishedJournal {
        async fn commit(&self) -> Result<(), String> {
            panic!("a repeated finalization must not commit buffered entries")
        }

        async fn committed_finished_index(
            &self,
            _session: &StreamSessionKeyV1,
        ) -> Result<Option<OplogIndex>, String> {
            let first = self.reads.fetch_add(1, Ordering::Relaxed) == 0;
            Ok(if first && self.hide_first {
                None
            } else {
                self.index
            })
        }
    }

    for (committed, hide_first, same_fingerprint) in [
        (true, false, true),
        (true, true, true),
        (false, false, true),
        (true, false, false),
    ] {
        let identity = identity();
        let oplog = Arc::new(TestOplog::default());
        let producer = DurableStreamProducer::load(
            oplog.clone(),
            identity.environment_id,
            identity.agent_id.clone(),
            identity.fingerprint,
            None,
        )
        .await
        .unwrap();
        let mut finished_key = identity.invocation.clone();
        if !same_fingerprint {
            finished_key.callee_fingerprint = AgentFingerprint::default();
            assert_ne!(finished_key, identity.invocation);
        }
        let index = oplog
            .add(OplogEntry::StreamSession {
                timestamp: Timestamp::now_utc(),
                entity_parent_start_index: None,
                record: OplogPayload::Inline(Box::new(StreamSessionRecordV1::Finished(
                    StreamSessionFinishedRecordV1 {
                        format_version: DURABLE_STREAM_FORMAT_VERSION,
                        session_key: finished_key,
                        result: Err(vec![1, 2, 3]),
                    },
                ))),
            })
            .await;
        if committed {
            oplog.commit(CommitLevel::Always).await;
        }
        producer.poison();
        let journal = Arc::new(FinishedJournal {
            index: committed.then_some(index),
            hide_first,
            reads: AtomicU64::new(0),
        });
        let streams = DurableSessionStreams::new(producer, oplog.clone(), identity.invocation, [])
            .with_consumer_journal(journal.clone());
        let result = streams.fail_invocation("execution failed".into()).await;
        assert_eq!(result.is_ok(), committed && same_fingerprint, "{result:?}");
        assert_eq!(oplog.current_oplog_index().await, index);
        assert_eq!(
            journal.reads.load(Ordering::Relaxed),
            if hide_first || !committed { 2 } else { 1 }
        );
        if !committed {
            assert_eq!(oplog.commit(CommitLevel::Always).await.len(), 1);
        }
    }
}

#[test]
async fn finished_in_raw_suffix_is_visible_and_cached() {
    let identity = identity();
    let oplog = Arc::new(TestOplog::default());
    let producer = DurableStreamProducer::load(
        oplog.clone(),
        identity.environment_id,
        identity.agent_id.clone(),
        identity.fingerprint,
        None,
    )
    .await
    .unwrap();
    let streams =
        DurableSessionStreams::new(producer, oplog.clone(), identity.invocation.clone(), []);
    let index = oplog
        .add(OplogEntry::StreamSession {
            timestamp: Timestamp::now_utc(),
            entity_parent_start_index: None,
            record: OplogPayload::Inline(Box::new(StreamSessionRecordV1::Finished(
                StreamSessionFinishedRecordV1 {
                    format_version: DURABLE_STREAM_FORMAT_VERSION,
                    session_key: identity.invocation,
                    result: Err(vec![1, 2, 3]),
                },
            ))),
        })
        .await;

    assert_eq!(
        streams.persisted_finished().await.unwrap(),
        Some(Err(vec![1, 2, 3]))
    );
    assert_eq!(oplog.take_read_ranges(), vec![(index, 1)]);
    assert_eq!(
        streams.persisted_finished().await.unwrap(),
        Some(Err(vec![1, 2, 3]))
    );
    assert!(oplog.take_read_ranges().is_empty());
}

#[test]
async fn caller_attempt_is_random_v4_persisted_and_reused_after_restart() {
    let identity = identity();
    let oplog = Arc::new(TestOplog::default());
    let producer = DurableStreamProducer::load(
        oplog.clone(),
        identity.environment_id,
        identity.agent_id.clone(),
        identity.fingerprint,
        None,
    )
    .await
    .unwrap();
    let streams = DurableSessionStreams::new(
        producer.clone(),
        oplog.clone(),
        identity.invocation.clone(),
        [],
    )
    .with_consumer_journal(Arc::new(TestConsumerJournal(oplog.clone())));

    let attempt = streams.caller_attempt_id().await.unwrap();
    assert_eq!(attempt.0.get_version(), Some(uuid::Version::Random));
    assert!(!attempt.0.is_nil());
    assert_eq!(streams.caller_attempt_id().await.unwrap(), attempt);

    let restarted = DurableSessionStreams::new(producer, oplog.clone(), identity.invocation, [])
        .with_consumer_journal(Arc::new(TestConsumerJournal(oplog)));
    assert_eq!(restarted.caller_attempt_id().await.unwrap(), attempt);
}

#[test]
async fn forwarded_root_input_and_direct_result_preserve_the_complete_handle() {
    let identity = identity();
    let oplog = Arc::new(TestOplog::default());
    let producer = DurableStreamProducer::load(
        oplog.clone(),
        identity.environment_id,
        identity.agent_id.clone(),
        identity.fingerprint,
        None,
    )
    .await
    .unwrap();
    let element_type = SchemaType::u32();
    let graph = SchemaGraph::anonymous(element_type.clone());
    let fingerprint = schema_fingerprint_v1(&graph, Some(&element_type)).unwrap();
    let source_coordinate = StreamRegistrationCoordinateV1::Root {
        invocation_id: identity.invocation.clone(),
        root_kind: StreamRootKindV1::MethodInput,
        recursive_value_path: vec![StreamValuePathStepV1::ListElement(7)],
    };
    let mut request = registration(
        &identity,
        source_coordinate,
        StreamSourceKindV1::AgentHostedInput,
    );
    request.element_schema_fingerprint = fingerprint;
    request.session_mapping = Some(StreamSessionMappingV1 {
        session_key: identity.invocation.clone(),
        attachment_id: AttachmentId::primary(
            identity.environment_id,
            &identity.agent_id,
            &identity.invocation.idempotency_key,
        )
        .unwrap(),
        role: SessionStreamRoleV1::Input,
    });
    let original = producer.register(request).await.unwrap().value;
    let streams = DurableSessionStreams::new(
        producer.clone(),
        oplog.clone(),
        identity.invocation.clone(),
        [(0, original.clone(), SessionStreamRoleV1::Input)],
    )
    .with_consumer_journal(Arc::new(TestConsumerJournal(oplog.clone())));
    let root_type = SchemaType::stream(Some(element_type));

    let input = SchemaValue::Stream(SchemaValueStream::from_host_endpoint(
        ForwardedDurableInput {
            handle: original.clone(),
        },
    ));
    let (_, input_mappings) = streams
        .materialize_agent_input(
            &input,
            &graph,
            &root_type,
            golem_common::model::component::ComponentRevision::INITIAL,
        )
        .await
        .unwrap();
    assert_eq!(input_mappings.len(), 1);
    assert_eq!(input_mappings[0].handle, original);
    assert!(
        producer
            .handle_for_coordinate(&StreamRegistrationCoordinateV1::Root {
                invocation_id: identity.invocation.clone(),
                root_kind: StreamRootKindV1::MethodInput,
                recursive_value_path: Vec::new(),
            })
            .await
            .unwrap()
            .is_none()
    );

    let result = SchemaValue::Stream(SchemaValueStream::from_host_endpoint(
        streams
            .endpoint(original.clone(), 0, SessionStreamRoleV1::Input)
            .await
            .unwrap(),
    ));
    streams
        .materialize_result(
            result,
            &graph,
            &root_type,
            golem_common::model::component::ComponentRevision::INITIAL,
        )
        .await
        .unwrap();
    let persisted = streams.remote_result_record().await.unwrap().unwrap();
    assert_eq!(persisted.output_streams, vec![original.clone()]);
    assert_eq!(persisted.stream_mappings[0].handle, original);

    streams
        .complete_or_defer_for_forwarded_inputs()
        .await
        .unwrap();
    assert_eq!(streams.persisted_finished().await.unwrap(), None);
    producer
        .end(original.stream_id, 0, StreamEndResultV1::Ok)
        .await
        .unwrap();
    streams
        .complete_or_defer_for_forwarded_inputs()
        .await
        .unwrap();
    assert_eq!(streams.persisted_finished().await.unwrap(), Some(Ok(())));

    assert!(
        producer
            .handle_for_coordinate(&StreamRegistrationCoordinateV1::Root {
                invocation_id: identity.invocation.clone(),
                root_kind: StreamRootKindV1::MethodResult,
                recursive_value_path: Vec::new(),
            })
            .await
            .unwrap()
            .is_none()
    );

    let read_endpoint = streams
        .endpoint(original.clone(), 1, SessionStreamRoleV1::Input)
        .await
        .unwrap();
    assert_eq!(
        read_endpoint.into_forwarded().err().unwrap(),
        "cannot forward a durable input stream after reading from it"
    );

    let mut journaled_endpoint = streams
        .endpoint(original.clone(), 0, SessionStreamRoleV1::Input)
        .await
        .unwrap();
    journaled_endpoint
        .journal
        .push_back(CommittedProducerStreamEventV1 {
            stream_id: original.stream_id,
            producer_sequence: 0,
            offset: golem_common::model::durable_stream::StreamOffsetV1::new(
                OplogIndex::INITIAL,
                0,
            ),
            packed_u8_batch_end: None,
            terminal_author: None,
            nested_handles: Vec::new(),
            payload: CommittedProducerStreamEventPayloadV1::PackedU8(0),
        });
    assert_eq!(
        journaled_endpoint.into_forwarded().err().unwrap(),
        "cannot forward a durable input stream after reading from it"
    );
}

#[test]
async fn schema_mismatch_does_not_consume_forwarded_stream() {
    let identity = identity();
    let oplog = Arc::new(TestOplog::default());
    let producer = DurableStreamProducer::load(
        oplog.clone(),
        identity.environment_id,
        identity.agent_id.clone(),
        identity.fingerprint,
        None,
    )
    .await
    .unwrap();
    let source_element_type = SchemaType::u32();
    let source_graph = SchemaGraph::anonymous(source_element_type.clone());
    let mut request = registration(
        &identity,
        StreamRegistrationCoordinateV1::Root {
            invocation_id: identity.invocation.clone(),
            root_kind: StreamRootKindV1::MethodResult,
            recursive_value_path: Vec::new(),
        },
        StreamSourceKindV1::InvocationOutput,
    );
    request.element_schema_fingerprint =
        schema_fingerprint_v1(&source_graph, Some(&source_element_type)).unwrap();
    let handle = producer.register(request).await.unwrap().value;
    let mut second_request = registration(
        &identity,
        StreamRegistrationCoordinateV1::Root {
            invocation_id: identity.invocation.clone(),
            root_kind: StreamRootKindV1::MethodResult,
            recursive_value_path: vec![StreamValuePathStepV1::TupleElement(1)],
        },
        StreamSourceKindV1::InvocationOutput,
    );
    second_request.element_schema_fingerprint =
        schema_fingerprint_v1(&source_graph, Some(&source_element_type)).unwrap();
    let second_handle = producer.register(second_request).await.unwrap().value;
    let streams = DurableSessionStreams::new(
        producer,
        oplog.clone(),
        identity.invocation,
        [
            (0, handle.clone(), SessionStreamRoleV1::Input),
            (1, second_handle.clone(), SessionStreamRoleV1::Input),
        ],
    )
    .with_consumer_journal(Arc::new(TestConsumerJournal(oplog)));
    let stream = SchemaValueStream::from_host_endpoint(ForwardedDurableInput {
        handle: handle.clone(),
    });
    let mismatched_element_type = SchemaType::string();
    let mismatched_graph = SchemaGraph::anonymous(mismatched_element_type.clone());

    let error = streams
        .materialize_result(
            SchemaValue::Stream(stream.clone()),
            &mismatched_graph,
            &SchemaType::stream(Some(mismatched_element_type.clone())),
            golem_common::model::component::ComponentRevision::INITIAL,
        )
        .await
        .unwrap_err();

    assert!(error.contains("does not match the result stream schema"));
    assert!(
        stream
            .with_host_endpoint::<ForwardedDurableInput, _>(|_| ())
            .is_ok()
    );

    let endpoint_stream = SchemaValueStream::from_host_endpoint(
        streams
            .endpoint(handle, 0, SessionStreamRoleV1::Input)
            .await
            .unwrap(),
    );
    let error = streams
        .materialize_result(
            SchemaValue::Stream(endpoint_stream.clone()),
            &mismatched_graph,
            &SchemaType::stream(Some(mismatched_element_type)),
            golem_common::model::component::ComponentRevision::INITIAL,
        )
        .await
        .unwrap_err();

    assert!(error.contains("does not match the result stream schema"));
    assert!(
        endpoint_stream
            .with_host_endpoint::<DurableInputEndpoint, _>(|_| ())
            .is_ok()
    );

    let second_stream = SchemaValueStream::from_host_endpoint(ForwardedDurableInput {
        handle: second_handle,
    });
    let tuple_root = SchemaType::tuple(vec![
        SchemaType::stream(Some(source_element_type.clone())),
        SchemaType::stream(Some(SchemaType::string())),
    ]);
    let tuple_graph = SchemaGraph::anonymous(tuple_root.clone());
    let error = streams
        .materialize_result(
            SchemaValue::Tuple {
                elements: vec![
                    SchemaValue::Stream(endpoint_stream.clone()),
                    SchemaValue::Stream(second_stream.clone()),
                ],
            },
            &tuple_graph,
            &tuple_root,
            golem_common::model::component::ComponentRevision::INITIAL,
        )
        .await
        .unwrap_err();

    assert!(error.contains("does not match the result stream schema"));
    assert!(
        endpoint_stream
            .with_host_endpoint::<DurableInputEndpoint, _>(|_| ())
            .is_ok()
    );
    assert!(
        second_stream
            .with_host_endpoint::<ForwardedDurableInput, _>(|_| ())
            .is_ok()
    );

    let consumed = SchemaValueStream::from_host_endpoint(());
    consumed.take_host_endpoint::<()>().unwrap();
    let valid_tuple_root = SchemaType::tuple(vec![
        SchemaType::stream(Some(source_element_type.clone())),
        SchemaType::stream(Some(source_element_type)),
    ]);
    let valid_tuple_graph = SchemaGraph::anonymous(valid_tuple_root.clone());

    let error = streams
        .materialize_result(
            SchemaValue::Tuple {
                elements: vec![
                    SchemaValue::Stream(second_stream.clone()),
                    SchemaValue::Stream(consumed),
                ],
            },
            &valid_tuple_graph,
            &valid_tuple_root,
            golem_common::model::component::ComponentRevision::INITIAL,
        )
        .await
        .unwrap_err();

    assert!(error.contains("schema value stream was already transferred"));
    assert!(
        second_stream
            .with_host_endpoint::<ForwardedDurableInput, _>(|_| ())
            .is_ok()
    );

    let error = streams
        .materialize_result(
            SchemaValue::Tuple {
                elements: vec![
                    SchemaValue::Stream(second_stream.clone()),
                    SchemaValue::Stream(second_stream.clone()),
                ],
            },
            &valid_tuple_graph,
            &valid_tuple_root,
            golem_common::model::component::ComponentRevision::INITIAL,
        )
        .await
        .unwrap_err();

    assert!(error.contains("the same affine stream appeared more than once"));
    assert!(
        second_stream
            .with_host_endpoint::<ForwardedDurableInput, _>(|_| ())
            .is_ok()
    );
}

#[test]
async fn forwarded_nested_stream_is_persisted_by_full_handle_without_re_registration() {
    let identity = identity();
    let oplog = Arc::new(TestOplog::default());
    let producer = DurableStreamProducer::load(
        oplog.clone(),
        identity.environment_id,
        identity.agent_id.clone(),
        identity.fingerprint,
        None,
    )
    .await
    .unwrap();
    let forwarded = producer
        .register(registration(
            &identity,
            StreamRegistrationCoordinateV1::Root {
                invocation_id: identity.invocation.clone(),
                root_kind: StreamRootKindV1::MethodInput,
                recursive_value_path: Vec::new(),
            },
            StreamSourceKindV1::AgentHostedInput,
        ))
        .await
        .unwrap()
        .value;
    let parent = producer
        .register(registration(
            &identity,
            StreamRegistrationCoordinateV1::Root {
                invocation_id: identity.invocation.clone(),
                root_kind: StreamRootKindV1::MethodResult,
                recursive_value_path: Vec::new(),
            },
            StreamSourceKindV1::InvocationOutput,
        ))
        .await
        .unwrap()
        .value;
    let streams =
        DurableSessionStreams::new(producer.clone(), oplog.clone(), identity.invocation, [])
            .with_consumer_journal(Arc::new(TestConsumerJournal(oplog)));
    streams
        .append_mapping_once(StreamSessionMappingRecordV1 {
            transport_stream_id: 3,
            handle: forwarded.clone(),
            role: SessionStreamRoleV1::Output,
        })
        .await
        .unwrap();
    let value = ProtoSchemaValue {
        value: Some(schema_value::Value::StreamReference(
            SchemaValueStreamReference { stream_id: 0 },
        )),
    };
    producer
        .write_items_with_nested_sources(
            parent.stream_id,
            0,
            StreamItemsPayloadV1::Values(vec![value.encode_to_vec()]),
            vec![NestedStreamWriteV1::Forward(forwarded.clone())],
        )
        .await
        .unwrap();

    let mut reader = producer.catch_up(parent, None).await.unwrap();
    let event = reader.next().await.unwrap().unwrap();
    assert_eq!(event.nested_handles, vec![forwarded]);
}
