use super::*;
use futures::Stream;
use golem_api_grpc::proto::golem::common::{Empty, EnvironmentId, Uuid};
use golem_api_grpc::proto::golem::component::ComponentId;
use golem_api_grpc::proto::golem::schema::{ListValue, SchemaValueStreamReference};
use golem_api_grpc::proto::golem::worker::{
    AgentId as ProtoAgentId, DurableStreamHandle, IdempotencyKey, InputStreamAck,
    InputStreamHighWater, InvocationFailure, InvocationFailureKind, InvocationRejected,
    InvocationRejectionReason, InvocationResponse, InvocationSessionCompletion, OutputStreamEnd,
    OutputStreamItem, StreamInvocationIdentity,
};
use golem_api_grpc::proto::golem::worker::{
    invocation_session_completion, invocation_session_result,
};
use serde_json::Value;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::task::{Context, Poll};
use tokio::sync::{Mutex, Semaphore, mpsc};
use tokio::time::{Duration, timeout};
use tonic::Status;

use test_r::{test, timeout as test_timeout};

type ResponseSender = mpsc::Sender<Result<InvocationResponse, Status>>;

enum Call {
    Start(Box<InvocationStart>, InvocationRequestStream),
    Resume(Box<ResumeAttach>, InvocationRequestStream),
}

struct ScriptTransport {
    calls: mpsc::UnboundedSender<Call>,
    responses: Mutex<VecDeque<InvocationResponseStream>>,
    cleanups: mpsc::UnboundedSender<(AgentId, ModelIdempotencyKey)>,
    cleanup_gate: Arc<Semaphore>,
}

#[async_trait]
impl SessionTransport for ScriptTransport {
    async fn start(
        &self,
        start: InvocationStart,
        tail: InvocationRequestStream,
    ) -> Result<InvocationResponseStream, WorkerServiceError> {
        self.calls.send(Call::Start(Box::new(start), tail)).unwrap();
        self.responses
            .lock()
            .await
            .pop_front()
            .ok_or_else(|| WorkerServiceError::Internal("missing start script".into()))
    }

    async fn resume(
        &self,
        resume: ResumeAttach,
        tail: InvocationRequestStream,
    ) -> Result<InvocationResponseStream, WorkerServiceError> {
        self.calls
            .send(Call::Resume(Box::new(resume), tail))
            .unwrap();
        self.responses
            .lock()
            .await
            .pop_front()
            .ok_or_else(|| WorkerServiceError::Internal("missing resume script".into()))
    }

    async fn cleanup(&self, agent: AgentId, key: ModelIdempotencyKey) -> CleanupOutcome {
        let _ = self.cleanups.send((agent, key));
        let _ = self.cleanup_gate.acquire().await;
        CleanupOutcome::FinishedUnconfirmed
    }
}

struct Harness {
    transport: Arc<ScriptTransport>,
    calls: mpsc::UnboundedReceiver<Call>,
    cleanups: mpsc::UnboundedReceiver<(AgentId, ModelIdempotencyKey)>,
    response_txs: Vec<ResponseSender>,
}

struct ReadyCountingStream {
    inner: tokio_stream::wrappers::ReceiverStream<Result<InvocationResponse, Status>>,
    ready: Arc<AtomicUsize>,
}

impl Stream for ReadyCountingStream {
    type Item = Result<InvocationResponse, Status>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let result = Pin::new(&mut self.inner).poll_next(cx);
        if matches!(result, Poll::Ready(Some(_))) {
            self.ready.fetch_add(1, Ordering::SeqCst);
        }
        result
    }
}

fn counting_harness() -> (Harness, Arc<AtomicUsize>) {
    let (call_tx, calls) = mpsc::unbounded_channel();
    let (cleanup_tx, cleanups) = mpsc::unbounded_channel();
    let (tx, rx) = mpsc::channel(16);
    let ready = Arc::new(AtomicUsize::new(0));
    let stream = ReadyCountingStream {
        inner: tokio_stream::wrappers::ReceiverStream::new(rx),
        ready: ready.clone(),
    };
    let harness = Harness {
        transport: Arc::new(ScriptTransport {
            calls: call_tx,
            responses: Mutex::new(VecDeque::from([
                Box::pin(stream) as InvocationResponseStream
            ])),
            cleanups: cleanup_tx,
            cleanup_gate: Arc::new(Semaphore::new(1)),
        }),
        calls,
        cleanups,
        response_txs: vec![tx],
    };
    (harness, ready)
}

fn harness(attempts: usize) -> Harness {
    harness_with_cleanup_permits(attempts, 1)
}

fn harness_with_cleanup_permits(attempts: usize, cleanup_permits: usize) -> Harness {
    let (call_tx, calls) = mpsc::unbounded_channel();
    let (cleanup_tx, cleanups) = mpsc::unbounded_channel();
    let mut response_txs = Vec::new();
    let mut responses = VecDeque::new();
    for _ in 0..attempts {
        let (tx, rx) = mpsc::channel(16);
        response_txs.push(tx);
        responses.push_back(Box::pin(tokio_stream::wrappers::ReceiverStream::new(rx))
            as Pin<Box<dyn Stream<Item = Result<InvocationResponse, Status>> + Send>>);
    }
    Harness {
        transport: Arc::new(ScriptTransport {
            calls: call_tx,
            responses: Mutex::new(responses),
            cleanups: cleanup_tx,
            cleanup_gate: Arc::new(Semaphore::new(cleanup_permits)),
        }),
        calls,
        cleanups,
        response_txs,
    }
}

fn uuid(n: u64) -> Uuid {
    Uuid {
        high_bits: 0,
        low_bits: n,
    }
}

fn proto_agent() -> ProtoAgentId {
    ProtoAgentId {
        component_id: Some(ComponentId {
            value: Some(uuid(20)),
        }),
        name: "actor".into(),
    }
}

fn key() -> IdempotencyKey {
    IdempotencyKey {
        value: "private-final-key".into(),
    }
}

fn stream_value(id: u64) -> SchemaValue {
    SchemaValue {
        value: Some(schema_value::Value::StreamReference(
            SchemaValueStreamReference { stream_id: id },
        )),
    }
}

fn start() -> InvocationStart {
    InvocationStart {
        agent_id: Some(proto_agent()),
        method_name: Some("run".into()),
        input: Some(stream_value(INPUT_STREAM_ID)),
        idempotency_key: Some(key()),
        ..Default::default()
    }
}

fn mapping(high_water: Option<u64>) -> DurableStreamMapping {
    DurableStreamMapping {
        transport_stream_id: INPUT_STREAM_ID,
        handle: Some(DurableStreamHandle {
            format_version: 1,
            stream_id: Some(uuid(101)),
            producer_generation: 0,
            producer_environment_id: Some(EnvironmentId {
                value: Some(uuid(3)),
            }),
            producer: Some(proto_agent()),
            expected_producer_fingerprint: Some(uuid(4)),
            source_invocation: Some(StreamInvocationIdentity {
                callee_environment_id: Some(EnvironmentId {
                    value: Some(uuid(3)),
                }),
                callee: Some(proto_agent()),
                callee_fingerprint: Some(uuid(4)),
                idempotency_key: Some(key()),
            }),
            component_revision: Some(12),
            element_schema_fingerprint: vec![5; 32],
        }),
        high_water: high_water.map(|sequence| InputStreamHighWater {
            highest_contiguous_sequence: sequence,
            resulting_offset: offset(sequence),
            terminal: false,
        }),
        role: StreamMappingRole::Input as i32,
    }
}

fn output_mapping() -> DurableStreamMapping {
    let mut mapping = mapping(None);
    mapping.transport_stream_id = 9;
    mapping.role = StreamMappingRole::Output as i32;
    mapping.handle.as_mut().unwrap().stream_id = Some(uuid(109));
    mapping
}

fn stream_result() -> InvocationResponse {
    response(invocation_response::Response::Result(
        golem_api_grpc::proto::golem::worker::InvocationSessionResult {
            result: Some(invocation_session_result::Result::MethodResult(
                stream_value(9),
            )),
            agent_id: Some(proto_agent()),
            idempotency_key: Some(key()),
            component_revision: Some(12),
            new_stream_mappings: vec![output_mapping()],
            ..Default::default()
        },
    ))
}

fn scalar_result() -> InvocationResponse {
    response(invocation_response::Response::Result(
        golem_api_grpc::proto::golem::worker::InvocationSessionResult {
            result: Some(invocation_session_result::Result::MethodResult(
                SchemaValue {
                    value: Some(schema_value::Value::U8Value(1)),
                },
            )),
            agent_id: Some(proto_agent()),
            idempotency_key: Some(key()),
            component_revision: Some(12),
            ..Default::default()
        },
    ))
}

fn output_item(sequence: u64) -> InvocationResponse {
    response(invocation_response::Response::OutputItem(
        OutputStreamItem {
            transport_stream_id: 9,
            producer_sequence: sequence,
            value: Some(SchemaValue {
                value: Some(schema_value::Value::U8Value(7)),
            }),
            durable_stream_id: Some(uuid(109)),
            durable_offset: offset(sequence),
            epoch: 1,
            logical_item_count: 1,
            ..Default::default()
        },
    ))
}

fn producer_cancel(sequence: u64) -> InvocationResponse {
    response(invocation_response::Response::StreamCancel(StreamCancel {
        transport_stream_id: 9,
        producer_sequence: sequence,
        role: StreamCancelRole::OutputProducer as i32,
        reason: StreamCancelReason::Cancelled as i32,
        durable_stream_id: Some(uuid(109)),
        durable_offset: offset(sequence),
        epoch: 1,
        ..Default::default()
    }))
}

fn accepted(epoch: u64, high_water: Option<u64>) -> InvocationResponse {
    response(invocation_response::Response::Accepted(
        InvocationAccepted {
            agent_id: Some(proto_agent()),
            idempotency_key: Some(key()),
            component_revision: Some(12),
            attachment_id: Some(uuid(1)),
            attempt_id: Some(uuid(epoch + 1)),
            epoch,
            stream_mappings: vec![mapping(high_water)],
            environment_id: Some(EnvironmentId {
                value: Some(uuid(3)),
            }),
            callee_fingerprint: Some(uuid(4)),
            method_name: Some("run".into()),
            tool_name: None,
            command_path: vec![],
            joined_origin_observer: false,
        },
    ))
}

fn resumed_accepted(resume: &ResumeAttach, high_water: Option<u64>) -> InvocationResponse {
    let mut response = accepted(resume.expected_epoch + 1, high_water);
    let Some(invocation_response::Response::Accepted(accepted)) = response.response.as_mut() else {
        unreachable!()
    };
    accepted.attempt_id = resume.attempt_id;
    response
}

fn response(value: invocation_response::Response) -> InvocationResponse {
    InvocationResponse {
        response: Some(value),
    }
}
fn offset(n: u64) -> Vec<u8> {
    let mut v = vec![0; 24];
    v[0] = 1;
    v[8..16].copy_from_slice(&n.to_be_bytes());
    v
}
fn ack(sequence: u64, epoch: u64) -> InvocationResponse {
    response(invocation_response::Response::InputAck(InputStreamAck {
        transport_stream_id: 1,
        highest_contiguous_sequence: sequence,
        logical_item_count: 1,
        durable_stream_id: Some(uuid(101)),
        resulting_offset: offset(sequence),
        epoch,
        new_stream_mappings: vec![],
    }))
}
fn finished() -> InvocationResponse {
    response(invocation_response::Response::Finished(
        InvocationSessionCompletion {
            outcome: Some(invocation_session_completion::Outcome::Success(Empty {})),
        },
    ))
}
fn failed() -> InvocationResponse {
    response(invocation_response::Response::Finished(
        InvocationSessionCompletion {
            outcome: Some(invocation_session_completion::Outcome::Failure(
                InvocationFailure {
                    kind: InvocationFailureKind::Execution as i32,
                    code: "failed".into(),
                    message: "failed".into(),
                    worker_error: None,
                },
            )),
        },
    ))
}
fn limits() -> HttpSessionLimits {
    HttpSessionLimits {
        retry_delay: Duration::ZERO,
        retry_deadline: Duration::from_secs(2),
        exchange_timeout: Duration::from_secs(5),
        cleanup_timeout: Duration::from_millis(100),
        ..Default::default()
    }
}
async fn send(tx: &ResponseSender, r: InvocationResponse) {
    tx.send(Ok(r)).await.unwrap();
}
async fn event(session: &mut HttpSession) -> HttpSessionEvent {
    timeout(Duration::from_secs(1), session.events.recv())
        .await
        .unwrap()
        .unwrap()
}
async fn call(calls: &mut mpsc::UnboundedReceiver<Call>) -> Call {
    timeout(Duration::from_secs(1), calls.recv())
        .await
        .unwrap()
        .unwrap()
}
fn input(req: InvocationRequest) -> invocation_request::Request {
    req.request.unwrap()
}

fn corpus_case(id: &str) -> Value {
    let corpus: Value = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../golem-service-base/tests/fixtures/http-handlers/corpus.json"
    )))
    .unwrap();
    corpus["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|case| case["id"] == id)
        .unwrap_or_else(|| panic!("missing shared vector {id}"))
        .clone()
}

fn decode_hex(hex: &str) -> Vec<u8> {
    hex.as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
        .collect()
}

fn output_bytes(sequence: u64, bytes: &[u8], epoch: u64) -> InvocationResponse {
    let mut response = output_item(sequence);
    let Some(invocation_response::Response::OutputItem(item)) = response.response.as_mut() else {
        unreachable!()
    };
    item.value = Some(SchemaValue {
        value: Some(schema_value::Value::ListValue(ListValue {
            elements: bytes
                .iter()
                .map(|byte| SchemaValue {
                    value: Some(schema_value::Value::U8Value(u32::from(*byte))),
                })
                .collect(),
        })),
    });
    item.epoch = epoch;
    response
}

fn output_end(sequence: u64, epoch: u64) -> InvocationResponse {
    response(invocation_response::Response::OutputEnd(OutputStreamEnd {
        transport_stream_id: 9,
        producer_sequence: sequence,
        durable_stream_id: Some(uuid(109)),
        durable_offset: offset(sequence),
        epoch,
    }))
}

fn event_bytes(event: HttpSessionEvent) -> Option<Vec<u8>> {
    let HttpSessionEvent::OutputItem(item) = event else {
        return None;
    };
    let Some(schema_value::Value::ListValue(value)) = item.value.and_then(|value| value.value)
    else {
        panic!("canonical HTTP output was not list<u8>")
    };
    Some(
        value
            .elements
            .into_iter()
            .map(|value| match value.value.unwrap() {
                schema_value::Value::U8Value(byte) => byte as u8,
                _ => panic!("canonical HTTP output contained a non-byte"),
            })
            .collect(),
    )
}

#[test]
#[test_timeout("10s")]
async fn input_is_held_until_acceptance_and_empty_chunk_is_not_eof() {
    let mut h = harness(1);
    let mut session =
        HttpSession::start_with_transport(start(), h.transport.clone(), limits()).unwrap();
    session.input.send_chunk(vec![7]).await.unwrap();
    session.input.send_chunk(Vec::new()).await.unwrap();
    session.input.finish().await.unwrap();
    let Call::Start(sent_start, mut tail) = call(&mut h.calls).await else {
        panic!()
    };
    assert_eq!(*sent_start, start());
    assert!(
        timeout(Duration::from_millis(30), tail.next())
            .await
            .is_err(),
        "input escaped before Accepted"
    );
    send(&h.response_txs[0], accepted(1, None)).await;
    assert!(matches!(
        event(&mut session).await,
        HttpSessionEvent::Accepted(_)
    ));
    for (sequence, expected) in [(0, vec![7]), (1, vec![])] {
        let invocation_request::Request::InputItem(item) = input(
            timeout(Duration::from_secs(1), tail.next())
                .await
                .unwrap()
                .unwrap(),
        ) else {
            panic!("expected item")
        };
        assert_eq!(item.sequence, sequence);
        let input_stream_item::Payload::Value(value) = item.payload.unwrap() else {
            panic!("packed payload")
        };
        let Some(schema_value::Value::ListValue(value)) = value.value else {
            panic!("not list<u8>")
        };
        assert_eq!(
            value
                .elements
                .into_iter()
                .map(|v| match v.value.unwrap() {
                    schema_value::Value::U8Value(v) => v as u8,
                    _ => panic!(),
                })
                .collect::<Vec<_>>(),
            expected
        );
    }
    let invocation_request::Request::InputEnd(end) = input(tail.next().await.unwrap()) else {
        panic!("expected EOF")
    };
    assert_eq!(end.sequence, 2);
}

#[test]
#[test_timeout("10s")]
async fn credits_bound_producers_until_ack_releases_a_frame() {
    let mut h = harness(1);
    let mut l = limits();
    l.retained_input_frames = 1;
    l.retained_input_bytes = 136;
    let mut session = HttpSession::start_with_transport(start(), h.transport.clone(), l).unwrap();
    let Call::Start(_, mut tail) = call(&mut h.calls).await else {
        panic!()
    };
    send(&h.response_txs[0], accepted(1, None)).await;
    let _ = event(&mut session).await;
    session.input.send_chunk(vec![1]).await.unwrap();
    let invocation_request::Request::InputItem(_) = input(tail.next().await.unwrap()) else {
        panic!()
    };
    let mut producer = tokio::spawn({
        let i = session.input.clone();
        async move { i.send_chunk(vec![]).await }
    });
    assert!(
        timeout(Duration::from_millis(30), &mut producer)
            .await
            .is_err()
    );
    send(&h.response_txs[0], ack(0, 1)).await;
    timeout(Duration::from_secs(1), producer)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
}

#[test]
#[test_timeout("10s")]
async fn lost_ack_resume_high_water_suppresses_only_persisted_frames() {
    let mut h = harness(2);
    let mut session =
        HttpSession::start_with_transport(start(), h.transport.clone(), limits()).unwrap();
    let Call::Start(_, mut first_tail) = call(&mut h.calls).await else {
        panic!()
    };
    send(&h.response_txs[0], accepted(1, None)).await;
    let _ = event(&mut session).await;
    session.input.send_chunk(vec![10]).await.unwrap();
    session.input.send_chunk(vec![11]).await.unwrap();
    for _ in 0..2 {
        first_tail.next().await.unwrap();
    }
    drop(h.response_txs.remove(0));
    let Call::Resume(resume, mut resumed_tail) = call(&mut h.calls).await else {
        panic!()
    };
    assert_eq!(resume.expected_epoch, 1);
    send(&h.response_txs[0], resumed_accepted(&resume, Some(0))).await;
    let invocation_request::Request::InputItem(item) = input(
        timeout(Duration::from_secs(1), resumed_tail.next())
            .await
            .unwrap()
            .unwrap(),
    ) else {
        panic!()
    };
    assert_eq!((item.sequence, item.epoch), (1, 2));
    assert!(
        timeout(Duration::from_millis(30), resumed_tail.next())
            .await
            .is_err()
    );
}

#[test]
#[test_timeout("10s")]
async fn stale_resume_epoch_fails_and_never_starts_again() {
    let mut h = harness(2);
    let mut session =
        HttpSession::start_with_transport(start(), h.transport.clone(), limits()).unwrap();
    let Call::Start(_, _) = call(&mut h.calls).await else {
        panic!()
    };
    send(&h.response_txs[0], accepted(1, None)).await;
    let _ = event(&mut session).await;
    drop(h.response_txs.remove(0));
    let Call::Resume(_, _) = call(&mut h.calls).await else {
        panic!()
    };
    send(&h.response_txs[0], accepted(1, None)).await;
    assert!(matches!(
        event(&mut session).await,
        HttpSessionEvent::Error(HttpSessionError::Protocol(_))
    ));
    assert!(
        timeout(Duration::from_millis(30), h.calls.recv())
            .await
            .is_err()
    );
}

#[test]
#[test_timeout("10s")]
async fn first_acceptance_lost_fails_without_resume_or_second_start() {
    let mut h = harness(1);
    let mut session =
        HttpSession::start_with_transport(start(), h.transport.clone(), limits()).unwrap();
    let Call::Start(_, _) = call(&mut h.calls).await else {
        panic!()
    };
    drop(h.response_txs.remove(0));
    assert!(matches!(
        event(&mut session).await,
        HttpSessionEvent::Error(HttpSessionError::TransportUnavailable)
    ));
    assert!(
        timeout(Duration::from_millis(30), h.calls.recv())
            .await
            .is_err()
    );
}

#[test]
#[test_timeout("10s")]
async fn cancellation_while_awaiting_acceptance_cleans_prepared_identity() {
    let mut h = harness(1);
    let mut session =
        HttpSession::start_with_transport(start(), h.transport.clone(), limits()).unwrap();
    let Call::Start(_, _) = call(&mut h.calls).await else {
        panic!()
    };
    session.cancellation.cancel();
    // Cancellation keeps the one Start exchange alive briefly so cleanup cannot race a worker
    // that has not yet been created.
    send(&h.response_txs[0], accepted(1, None)).await;
    assert!(matches!(
        event(&mut session).await,
        HttpSessionEvent::Error(HttpSessionError::Cancelled)
    ));
    let (agent, key) = timeout(Duration::from_secs(1), h.cleanups.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(agent, AgentId::try_from(proto_agent()).unwrap());
    assert_eq!(
        key,
        ModelIdempotencyKey::new("private-final-key".to_string())
    );
}

#[test]
#[test_timeout("10s")]
async fn sender_eof_is_not_cancellation() {
    let mut h = harness(1);
    let session =
        HttpSession::start_with_transport(start(), h.transport.clone(), limits()).unwrap();
    let Call::Start(_, _) = call(&mut h.calls).await else {
        panic!()
    };
    let HttpSession {
        input,
        mut events,
        cancellation,
    } = session;
    drop(input);
    send(&h.response_txs[0], accepted(1, None)).await;
    assert!(matches!(
        timeout(Duration::from_secs(1), events.recv())
            .await
            .unwrap()
            .unwrap(),
        HttpSessionEvent::Accepted(_)
    ));
    assert!(
        timeout(Duration::from_millis(50), events.recv())
            .await
            .is_err()
    );
    assert!(
        timeout(Duration::from_millis(50), h.cleanups.recv())
            .await
            .is_err()
    );
    cancellation.cancel();
    assert!(matches!(
        timeout(Duration::from_secs(1), events.recv())
            .await
            .unwrap()
            .unwrap(),
        HttpSessionEvent::Error(HttpSessionError::Cancelled)
    ));
    timeout(Duration::from_secs(1), h.cleanups.recv())
        .await
        .unwrap()
        .unwrap();
}

#[test]
#[test_timeout("10s")]
async fn explicit_success_does_not_cleanup() {
    let mut h = harness(1);
    let mut session =
        HttpSession::start_with_transport(start(), h.transport.clone(), limits()).unwrap();
    let Call::Start(_, mut tail) = call(&mut h.calls).await else {
        panic!()
    };
    send(&h.response_txs[0], accepted(1, None)).await;
    let _ = event(&mut session).await;
    session.input.dispose().await.unwrap();
    assert!(matches!(
        input(tail.next().await.unwrap()),
        invocation_request::Request::StreamCancel(_)
    ));
    let result = golem_api_grpc::proto::golem::worker::InvocationSessionResult {
        result: Some(invocation_session_result::Result::MethodResult(
            SchemaValue {
                value: Some(schema_value::Value::U8Value(1)),
            },
        )),
        agent_id: Some(proto_agent()),
        idempotency_key: Some(key()),
        component_revision: Some(12),
        ..Default::default()
    };
    send(
        &h.response_txs[0],
        response(invocation_response::Response::Result(result)),
    )
    .await;
    assert!(matches!(
        event(&mut session).await,
        HttpSessionEvent::Result(_)
    ));
    send(&h.response_txs[0], finished()).await;
    assert!(matches!(
        event(&mut session).await,
        HttpSessionEvent::Finished(_)
    ));
    assert!(
        timeout(Duration::from_millis(50), h.cleanups.recv())
            .await
            .is_err()
    );
}

#[test]
#[test_timeout("10s")]
async fn finished_failure_does_not_cleanup() {
    let mut h = harness(1);
    let mut session =
        HttpSession::start_with_transport(start(), h.transport.clone(), limits()).unwrap();
    let Call::Start(_, mut tail) = call(&mut h.calls).await else {
        panic!()
    };
    send(&h.response_txs[0], accepted(1, None)).await;
    let _ = event(&mut session).await;
    session.input.dispose().await.unwrap();
    let _ = tail.next().await.unwrap();
    send(&h.response_txs[0], scalar_result()).await;
    let _ = event(&mut session).await;
    send(&h.response_txs[0], failed()).await;
    assert!(matches!(
        event(&mut session).await,
        HttpSessionEvent::Finished(_)
    ));
    assert!(
        timeout(Duration::from_millis(50), h.cleanups.recv())
            .await
            .is_err()
    );
}

#[test]
#[test_timeout("10s")]
async fn rejected_start_cleans_up_exactly_once() {
    let mut h = harness(1);
    let mut session =
        HttpSession::start_with_transport(start(), h.transport.clone(), limits()).unwrap();
    let _ = call(&mut h.calls).await;
    send(
        &h.response_txs[0],
        response(invocation_response::Response::Rejected(
            InvocationRejected {
                reason: InvocationRejectionReason::Validation as i32,
                error: "rejected".into(),
                idempotency_key: Some(key()),
                agent_id: Some(proto_agent()),
                component_revision: None,
                worker_error: None,
            },
        )),
    )
    .await;
    assert!(matches!(
        event(&mut session).await,
        HttpSessionEvent::Error(HttpSessionError::Rejected)
    ));
    timeout(Duration::from_secs(1), h.cleanups.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(
        timeout(Duration::from_millis(50), h.cleanups.recv())
            .await
            .is_err()
    );
}

#[test]
#[test_timeout("10s")]
async fn dropping_events_cancels_and_cleans_up_exactly_once() {
    let mut h = harness(1);
    let session =
        HttpSession::start_with_transport(start(), h.transport.clone(), limits()).unwrap();
    let _ = call(&mut h.calls).await;
    send(&h.response_txs[0], accepted(1, None)).await;
    drop(session.events);
    timeout(Duration::from_secs(1), h.cleanups.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(
        timeout(Duration::from_millis(50), h.cleanups.recv())
            .await
            .is_err()
    );
}

#[test]
#[test_timeout("10s")]
async fn resume_attempt_cap_cleans_up_exactly_once() {
    let mut h = harness(2);
    let mut l = limits();
    l.max_resume_attempts = 1;
    let mut session = HttpSession::start_with_transport(start(), h.transport.clone(), l).unwrap();
    let _ = call(&mut h.calls).await;
    send(&h.response_txs[0], accepted(1, None)).await;
    let _ = event(&mut session).await;
    drop(h.response_txs.remove(0));
    let _ = call(&mut h.calls).await;
    drop(h.response_txs.remove(0));
    assert!(matches!(
        event(&mut session).await,
        HttpSessionEvent::Error(HttpSessionError::ResumeExhausted)
    ));
    timeout(Duration::from_secs(1), h.cleanups.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(
        timeout(Duration::from_millis(50), h.cleanups.recv())
            .await
            .is_err()
    );
}

#[test]
#[test_timeout("10s")]
async fn resume_deadline_cleans_up_exactly_once() {
    let mut h = harness(2);
    let mut l = limits();
    l.retry_delay = Duration::from_secs(1);
    l.retry_deadline = Duration::from_millis(20);
    let mut session = HttpSession::start_with_transport(start(), h.transport.clone(), l).unwrap();
    let _ = call(&mut h.calls).await;
    send(&h.response_txs[0], accepted(1, None)).await;
    let _ = event(&mut session).await;
    drop(h.response_txs.remove(0));
    assert!(matches!(
        event(&mut session).await,
        HttpSessionEvent::Error(HttpSessionError::ResumeExhausted)
    ));
    timeout(Duration::from_secs(1), h.cleanups.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(
        timeout(Duration::from_millis(50), h.cleanups.recv())
            .await
            .is_err()
    );
}

#[test]
#[test_timeout("10s")]
async fn oversized_output_fails_and_cleans_up() {
    let mut h = harness(1);
    let mut l = limits();
    l.max_event_bytes = 1;
    let mut session = HttpSession::start_with_transport(start(), h.transport.clone(), l).unwrap();
    let _ = call(&mut h.calls).await;
    send(&h.response_txs[0], accepted(1, None)).await;
    let _ = event(&mut session).await;
    send(&h.response_txs[0], output_item(0)).await;
    assert!(matches!(
        event(&mut session).await,
        HttpSessionEvent::Error(HttpSessionError::RetainedInputLimit)
    ));
    timeout(Duration::from_secs(1), h.cleanups.recv())
        .await
        .unwrap()
        .unwrap();
}

#[test]
#[test_timeout("10s")]
async fn output_end_does_not_complete_session_and_it_remains_cancelable() {
    let mut h = harness(1);
    let mut session =
        HttpSession::start_with_transport(start(), h.transport.clone(), limits()).unwrap();
    let Call::Start(_, mut tail) = call(&mut h.calls).await else {
        panic!()
    };
    send(&h.response_txs[0], accepted(1, None)).await;
    let _ = event(&mut session).await;
    let end = OutputStreamEnd {
        transport_stream_id: 9,
        producer_sequence: 0,
        durable_stream_id: Some(uuid(109)),
        durable_offset: offset(0),
        epoch: 1,
    };
    // Unknown output streams are intentionally rejected; establish it through a result mapping first.
    let mut out = mapping(None);
    out.transport_stream_id = 9;
    out.role = StreamMappingRole::Output as i32;
    out.handle.as_mut().unwrap().stream_id = Some(uuid(109));
    let result = golem_api_grpc::proto::golem::worker::InvocationSessionResult {
        result: Some(invocation_session_result::Result::MethodResult(
            stream_value(9),
        )),
        agent_id: Some(proto_agent()),
        idempotency_key: Some(key()),
        component_revision: Some(12),
        new_stream_mappings: vec![out],
        ..Default::default()
    };
    send(
        &h.response_txs[0],
        response(invocation_response::Response::Result(result)),
    )
    .await;
    let _ = event(&mut session).await;
    send(
        &h.response_txs[0],
        response(invocation_response::Response::OutputEnd(end)),
    )
    .await;
    assert!(matches!(
        event(&mut session).await,
        HttpSessionEvent::OutputEnd(_)
    ));
    session.input.dispose_output(9).await.unwrap();
    assert!(
        timeout(Duration::from_millis(30), tail.next())
            .await
            .is_err(),
        "disposing a terminal output must be a no-op"
    );
    session.cancellation.cancel();
    assert!(matches!(
        event(&mut session).await,
        HttpSessionEvent::Error(HttpSessionError::Cancelled)
    ));
    timeout(Duration::from_secs(1), h.cleanups.recv())
        .await
        .unwrap()
        .unwrap();
}

#[test]
#[test_timeout("10s")]
async fn disposing_delivered_output_uses_consumer_control_and_terminal_disposal_is_a_noop() {
    let mut h = harness(1);
    let mut session =
        HttpSession::start_with_transport(start(), h.transport.clone(), limits()).unwrap();
    let Call::Start(_, mut tail) = call(&mut h.calls).await else {
        panic!()
    };
    send(&h.response_txs[0], accepted(1, None)).await;
    let _ = event(&mut session).await;
    send(&h.response_txs[0], stream_result()).await;
    let _ = event(&mut session).await;
    send(&h.response_txs[0], output_item(0)).await;
    assert!(matches!(
        event(&mut session).await,
        HttpSessionEvent::OutputItem(_)
    ));

    session.input.dispose_output(9).await.unwrap();
    let invocation_request::Request::StreamCancel(cancel) = input(tail.next().await.unwrap())
    else {
        panic!()
    };
    assert_eq!(cancel.transport_stream_id, 9);
    assert_eq!(cancel.producer_sequence, 0);
    assert_eq!(cancel.role(), StreamCancelRole::OutputConsumer);
    assert_eq!(cancel.reason(), StreamCancelReason::ConsumerDrop);
    assert_eq!(cancel.durable_stream_id, Some(uuid(109)));
    assert_eq!(cancel.epoch, 1);
    assert!(cancel.durable_offset.is_empty());

    send(&h.response_txs[0], producer_cancel(1)).await;
    assert!(matches!(
        event(&mut session).await,
        HttpSessionEvent::StreamCancel(_)
    ));
    session.input.dispose_output(9).await.unwrap();
    assert!(
        timeout(Duration::from_millis(30), tail.next())
            .await
            .is_err()
    );
}

#[test]
#[test_timeout("10s")]
async fn observed_producer_cancel_is_a_terminal_resume_cursor_after_transport_loss() {
    let mut h = harness(2);
    let mut session =
        HttpSession::start_with_transport(start(), h.transport.clone(), limits()).unwrap();
    let Call::Start(_, _) = call(&mut h.calls).await else {
        panic!()
    };
    send(&h.response_txs[0], accepted(1, None)).await;
    let _ = event(&mut session).await;
    send(&h.response_txs[0], stream_result()).await;
    let _ = event(&mut session).await;
    send(&h.response_txs[0], output_item(0)).await;
    let _ = event(&mut session).await;
    send(&h.response_txs[0], producer_cancel(1)).await;
    let _ = event(&mut session).await;
    drop(h.response_txs.remove(0));

    let Call::Resume(resume, _) = call(&mut h.calls).await else {
        panic!()
    };
    assert_eq!(resume.cursors.len(), 1);
    assert_eq!(resume.cursors[0].stream_id, Some(uuid(109)));
    assert_eq!(resume.cursors[0].last_observed_offset, Some(offset(1)));
    send(&h.response_txs[0], resumed_accepted(&resume, None)).await;
}

#[test]
#[test_timeout("10s")]
async fn output_disposal_lost_before_confirmation_is_reissued_after_resume() {
    let mut h = harness(2);
    let mut session =
        HttpSession::start_with_transport(start(), h.transport.clone(), limits()).unwrap();
    let Call::Start(_, mut tail) = call(&mut h.calls).await else {
        panic!()
    };
    send(&h.response_txs[0], accepted(1, None)).await;
    let _ = event(&mut session).await;
    send(&h.response_txs[0], stream_result()).await;
    let _ = event(&mut session).await;
    session.input.dispose_output(9).await.unwrap();
    assert!(matches!(
        input(tail.next().await.unwrap()),
        invocation_request::Request::StreamCancel(_)
    ));
    drop(h.response_txs.remove(0));

    let Call::Resume(resume, mut resumed_tail) = call(&mut h.calls).await else {
        panic!()
    };
    let mut decision = resumed_accepted(&resume, None);
    let Some(invocation_response::Response::Accepted(accepted)) = decision.response.as_mut() else {
        panic!()
    };
    accepted.stream_mappings.push(output_mapping());
    send(&h.response_txs[0], decision).await;
    let invocation_request::Request::StreamCancel(cancel) =
        input(resumed_tail.next().await.unwrap())
    else {
        panic!()
    };
    assert_eq!(cancel.role(), StreamCancelRole::OutputConsumer);
    assert_eq!(cancel.durable_stream_id, Some(uuid(109)));
    assert_eq!(cancel.epoch, 2);
    assert!(cancel.durable_offset.is_empty());
}

#[test]
#[test_timeout("10s")]
async fn terminal_input_high_water_releases_retention_and_replays_nothing() {
    let mut h = harness(2);
    let mut l = limits();
    l.retained_input_frames = 1;
    l.retained_input_bytes = 136;
    let mut session = HttpSession::start_with_transport(start(), h.transport.clone(), l).unwrap();
    let Call::Start(_, mut first_tail) = call(&mut h.calls).await else {
        panic!()
    };
    send(&h.response_txs[0], accepted(1, None)).await;
    let _ = event(&mut session).await;
    session.input.send_chunk(vec![1]).await.unwrap();
    let _ = first_tail.next().await.unwrap();
    drop(h.response_txs.remove(0));
    let Call::Resume(resume, mut resumed_tail) = call(&mut h.calls).await else {
        panic!()
    };
    let mut decision = resumed_accepted(&resume, Some(0));
    let Some(invocation_response::Response::Accepted(value)) = decision.response.as_mut() else {
        panic!()
    };
    value.stream_mappings[0]
        .high_water
        .as_mut()
        .unwrap()
        .terminal = true;
    send(&h.response_txs[0], decision).await;

    session.input.send_chunk(vec![2]).await.unwrap();
    assert!(
        timeout(Duration::from_millis(30), resumed_tail.next())
            .await
            .is_err()
    );
}

#[test]
#[test_timeout("10s")]
async fn disposal_control_bypasses_a_full_output_event_queue() {
    let mut h = harness(1);
    let mut l = limits();
    l.event_queue_frames = 1;
    let mut session = HttpSession::start_with_transport(start(), h.transport.clone(), l).unwrap();
    let Call::Start(_, mut tail) = call(&mut h.calls).await else {
        panic!()
    };
    send(&h.response_txs[0], accepted(1, None)).await;
    let _ = event(&mut session).await;
    send(&h.response_txs[0], stream_result()).await;
    assert!(matches!(
        event(&mut session).await,
        HttpSessionEvent::Result(_)
    ));
    send(&h.response_txs[0], output_item(0)).await;
    timeout(Duration::from_secs(1), async {
        while session.events.receiver.is_empty() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    session.input.dispose_output(9).await.unwrap();
    let invocation_request::Request::StreamCancel(cancel) = input(
        timeout(Duration::from_secs(1), tail.next())
            .await
            .unwrap()
            .unwrap(),
    ) else {
        panic!()
    };
    assert_eq!(cancel.role(), StreamCancelRole::OutputConsumer);
}

#[test]
#[test_timeout("10s")]
async fn preacceptance_cancel_waits_for_decision_then_cleans_up_exactly_once() {
    let mut h = harness(1);
    let mut session =
        HttpSession::start_with_transport(start(), h.transport.clone(), limits()).unwrap();
    let Call::Start(_, _) = call(&mut h.calls).await else {
        panic!()
    };
    session.cancellation.cancel();
    assert!(
        timeout(Duration::from_millis(30), h.cleanups.recv())
            .await
            .is_err()
    );
    send(&h.response_txs[0], accepted(1, None)).await;
    assert!(matches!(
        event(&mut session).await,
        HttpSessionEvent::Error(HttpSessionError::Cancelled)
    ));
    let _ = timeout(Duration::from_secs(1), h.cleanups.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(
        timeout(Duration::from_millis(30), h.cleanups.recv())
            .await
            .is_err()
    );
}

#[test]
fn body_bytes_are_absent_from_session_trace_logs() {
    struct LogWriter(Arc<std::sync::Mutex<Vec<u8>>>);
    impl std::io::Write for LogWriter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let logs = Arc::new(std::sync::Mutex::new(Vec::new()));
    let writer = logs.clone();
    // tracing-core's singleton-dispatch optimization uses the registering thread's
    // subscriber. A second dispatch prevents concurrent unsubscribed tests from
    // caching shared callsites as disabled for this capture subscriber.
    let _callsite_registry_anchor = tracing::Dispatch::new(
        tracing_subscriber::fmt()
            .with_max_level(tracing::level_filters::LevelFilter::OFF)
            .with_writer(std::io::sink)
            .finish(),
    );
    let subscriber = tracing_subscriber::fmt()
        .without_time()
        .with_max_level(tracing::Level::TRACE)
        .with_writer(move || LogWriter(writer.clone()))
        .finish();
    let _subscriber = tracing::subscriber::set_default(subscriber);
    fn shared_callsite_probe() {
        tracing::debug!("shared callsite probe");
    }
    std::thread::spawn(shared_callsite_probe).join().unwrap();
    shared_callsite_probe();
    assert!(
        String::from_utf8(logs.lock().unwrap().clone())
            .unwrap()
            .contains("shared callsite probe")
    );
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            let mut h = harness(1);
            let mut session =
                HttpSession::start_with_transport(start(), h.transport.clone(), limits()).unwrap();
            let Call::Start(_, mut tail) = call(&mut h.calls).await else {
                panic!()
            };
            send(&h.response_txs[0], accepted(1, None)).await;
            let _ = event(&mut session).await;
            session
                .input
                .send_chunk(b"private-request-sentinel".to_vec())
                .await
                .unwrap();
            assert!(matches!(
                input(tail.next().await.unwrap()),
                invocation_request::Request::InputItem(_)
            ));
            send(&h.response_txs[0], ack(0, 1)).await;
            session.input.dispose().await.unwrap();
            assert!(matches!(
                input(tail.next().await.unwrap()),
                invocation_request::Request::StreamCancel(_)
            ));
            send(&h.response_txs[0], stream_result()).await;
            let _ = event(&mut session).await;
            send(
                &h.response_txs[0],
                output_bytes(0, b"private-response-sentinel", 1),
            )
            .await;
            let output = event(&mut session).await;
            tracing::debug!(?output, "HTTP body diagnostic probe");
            assert_eq!(event_bytes(output).unwrap(), b"private-response-sentinel");
            send(&h.response_txs[0], output_end(1, 1)).await;
            let _ = event(&mut session).await;
            send(&h.response_txs[0], finished()).await;
            assert!(matches!(
                event(&mut session).await,
                HttpSessionEvent::Finished(_)
            ));
        });
    let rendered = String::from_utf8(logs.lock().unwrap().clone()).unwrap();
    assert!(rendered.contains("HTTP session terminated"));
    assert!(rendered.contains("HTTP body diagnostic probe"));
    for sentinel in ["private-request-sentinel", "private-response-sentinel"] {
        assert!(
            !rendered.contains(sentinel),
            "body content leaked into trace logs"
        );
    }
}

#[test]
#[test_timeout("10s")]
async fn blocked_cleanup_does_not_delay_terminal_error_availability() {
    let mut h = harness_with_cleanup_permits(1, 0);
    let mut session =
        HttpSession::start_with_transport(start(), h.transport.clone(), limits()).unwrap();
    let Call::Start(_, _) = call(&mut h.calls).await else {
        panic!()
    };
    send(&h.response_txs[0], accepted(1, None)).await;
    assert!(matches!(
        event(&mut session).await,
        HttpSessionEvent::Accepted(_)
    ));
    session.cancellation.cancel();
    let _ = timeout(Duration::from_secs(1), h.cleanups.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(
        event(&mut session).await,
        HttpSessionEvent::Error(HttpSessionError::Cancelled)
    ));
}

#[test]
#[test_timeout("10s")]
async fn lifecycle_early_response_is_independent_of_request_eof() {
    let case = corpus_case("lifecycle-early-response");
    let mut h = harness(1);
    let mut session =
        HttpSession::start_with_transport(start(), h.transport.clone(), limits()).unwrap();
    let Call::Start(_, mut tail) = call(&mut h.calls).await else {
        panic!()
    };
    send(&h.response_txs[0], accepted(1, None)).await;
    let _ = event(&mut session).await;
    session.input.send_chunk(vec![0x61]).await.unwrap();
    assert!(matches!(
        input(tail.next().await.unwrap()),
        invocation_request::Request::InputItem(_)
    ));
    session.input.dispose().await.unwrap();
    assert!(
        timeout(Duration::from_millis(20), tail.next())
            .await
            .is_err(),
        "early response disposal must wait for in-flight input ACKs"
    );
    send(&h.response_txs[0], ack(0, 1)).await;
    let invocation_request::Request::StreamCancel(cancel) = input(tail.next().await.unwrap())
    else {
        panic!()
    };
    assert_eq!(cancel.producer_sequence, 1);
    assert_eq!(cancel.reason(), StreamCancelReason::ConsumerDrop);
    send(&h.response_txs[0], stream_result()).await;
    let _ = event(&mut session).await;
    send(&h.response_txs[0], output_bytes(0, b"bc", 1)).await;
    let body = event_bytes(event(&mut session).await).unwrap();
    send(&h.response_txs[0], output_end(1, 1)).await;
    assert!(matches!(
        event(&mut session).await,
        HttpSessionEvent::OutputEnd(_)
    ));
    send(&h.response_txs[0], finished()).await;
    assert!(matches!(
        event(&mut session).await,
        HttpSessionEvent::Finished(_)
    ));

    assert_eq!(body, b"bc");
    assert_eq!(
        body,
        decode_hex(case["expect"]["body_hex"].as_str().unwrap())
    );
    assert!(
        timeout(Duration::from_millis(30), h.calls.recv())
            .await
            .is_err()
    );
    assert!(
        timeout(Duration::from_millis(30), h.cleanups.recv())
            .await
            .is_err()
    );
}

#[test]
#[test_timeout("10s")]
async fn input_disposal_is_reissued_only_when_resume_high_water_is_not_terminal() {
    for terminal in [false, true] {
        let mut h = harness(2);
        let mut session =
            HttpSession::start_with_transport(start(), h.transport.clone(), limits()).unwrap();
        let Call::Start(_, mut tail) = call(&mut h.calls).await else {
            panic!()
        };
        send(&h.response_txs[0], accepted(1, None)).await;
        let _ = event(&mut session).await;
        session.input.send_chunk(vec![0x73]).await.unwrap();
        let _ = tail.next().await.unwrap();
        send(&h.response_txs[0], ack(0, 1)).await;
        session.input.dispose().await.unwrap();
        let _ = tail.next().await.unwrap();
        drop(h.response_txs.remove(0));
        let Call::Resume(resume, mut tail) = call(&mut h.calls).await else {
            panic!()
        };
        let mut acceptance = resumed_accepted(&resume, Some(0));
        let Some(invocation_response::Response::Accepted(accepted)) = &mut acceptance.response
        else {
            panic!()
        };
        accepted.stream_mappings[0]
            .high_water
            .as_mut()
            .unwrap()
            .terminal = terminal;
        send(&h.response_txs[0], acceptance).await;
        if terminal {
            assert!(
                timeout(Duration::from_millis(30), tail.next())
                    .await
                    .is_err()
            );
        } else {
            let invocation_request::Request::StreamCancel(cancel) =
                input(tail.next().await.unwrap())
            else {
                panic!()
            };
            assert_eq!(cancel.producer_sequence, 1);
            assert_eq!(cancel.epoch, 2);
            assert_eq!(cancel.reason(), StreamCancelReason::ConsumerDrop);
        }
        session.cancellation.cancel();
    }
}

#[test]
#[test_timeout("10s")]
async fn lifecycle_reattach_deduplicates_replayed_public_output() {
    let case = corpus_case("lifecycle-reattach-deduplicates");
    let mut h = harness(2);
    let mut session =
        HttpSession::start_with_transport(start(), h.transport.clone(), limits()).unwrap();
    let Call::Start(_, _) = call(&mut h.calls).await else {
        panic!()
    };
    send(&h.response_txs[0], accepted(7, None)).await;
    let _ = event(&mut session).await;
    send(&h.response_txs[0], stream_result()).await;
    let _ = event(&mut session).await;
    send(&h.response_txs[0], output_bytes(0, b"b", 7)).await;
    let mut body = event_bytes(event(&mut session).await).unwrap();
    drop(h.response_txs.remove(0));

    let Call::Resume(resume, _) = call(&mut h.calls).await else {
        panic!()
    };
    assert_eq!(resume.expected_epoch, 7);
    assert_eq!(resume.cursors[0].last_observed_offset, Some(offset(0)));
    let mut decision = resumed_accepted(&resume, None);
    let Some(invocation_response::Response::Accepted(accepted)) = decision.response.as_mut() else {
        panic!()
    };
    accepted.stream_mappings.push(output_mapping());
    send(&h.response_txs[0], decision).await;
    send(&h.response_txs[0], stream_result()).await;
    // The executor's replay uses the supplied cursor exclusively: the already
    // observed logical frame remains in its producer log but is not retransmitted.
    send(&h.response_txs[0], output_bytes(1, b"c", 8)).await;
    body.extend(event_bytes(event(&mut session).await).unwrap());
    send(&h.response_txs[0], output_end(2, 8)).await;
    assert!(matches!(
        event(&mut session).await,
        HttpSessionEvent::OutputEnd(_)
    ));
    send(
        &h.response_txs[0],
        response(invocation_response::Response::StreamCancel(StreamCancel {
            transport_stream_id: INPUT_STREAM_ID,
            producer_sequence: 0,
            role: StreamCancelRole::InputConsumer as i32,
            reason: StreamCancelReason::ConsumerDrop as i32,
            durable_stream_id: Some(uuid(101)),
            durable_offset: offset(0),
            epoch: 8,
            ..Default::default()
        })),
    )
    .await;
    assert!(matches!(
        event(&mut session).await,
        HttpSessionEvent::StreamCancel(_)
    ));
    send(&h.response_txs[0], finished()).await;
    assert!(matches!(
        event(&mut session).await,
        HttpSessionEvent::Finished(_)
    ));

    assert_eq!(
        body,
        decode_hex(case["expect"]["body_hex"].as_str().unwrap())
    );
    assert!(
        timeout(Duration::from_millis(30), h.calls.recv())
            .await
            .is_err()
    );
    assert!(
        timeout(Duration::from_millis(30), h.cleanups.recv())
            .await
            .is_err()
    );
}

#[test]
#[test_timeout("10s")]
async fn output_repeated_at_the_resume_cursor_is_a_protocol_error() {
    let mut h = harness(2);
    let mut session =
        HttpSession::start_with_transport(start(), h.transport.clone(), limits()).unwrap();
    let _ = call(&mut h.calls).await;
    send(&h.response_txs[0], accepted(1, None)).await;
    let _ = event(&mut session).await;
    send(&h.response_txs[0], stream_result()).await;
    let _ = event(&mut session).await;
    send(&h.response_txs[0], output_bytes(0, b"b", 1)).await;
    assert_eq!(event_bytes(event(&mut session).await).unwrap(), b"b");
    drop(h.response_txs.remove(0));
    let Call::Resume(resume, _) = call(&mut h.calls).await else {
        panic!()
    };
    assert_eq!(resume.cursors[0].last_observed_offset, Some(offset(0)));
    let mut decision = resumed_accepted(&resume, None);
    let Some(invocation_response::Response::Accepted(accepted)) = decision.response.as_mut() else {
        panic!()
    };
    accepted.stream_mappings.push(output_mapping());
    send(&h.response_txs[0], decision).await;
    send(&h.response_txs[0], output_bytes(0, b"b", 2)).await;
    assert!(matches!(
        event(&mut session).await,
        HttpSessionEvent::Error(HttpSessionError::Protocol(_))
    ));
}

#[test]
#[test_timeout("10s")]
async fn lifecycle_input_ack_lost_replays_actual_unacknowledged_request() {
    let case = corpus_case("lifecycle-input-ack-lost");
    assert_eq!(case["expect"]["reattachments"], 1);
    let mut h = harness(2);
    let mut session =
        HttpSession::start_with_transport(start(), h.transport.clone(), limits()).unwrap();
    let Call::Start(_, mut first_tail) = call(&mut h.calls).await else {
        panic!()
    };
    send(&h.response_txs[0], accepted(7, None)).await;
    let _ = event(&mut session).await;
    session.input.send_chunk(b"ab".to_vec()).await.unwrap();
    let invocation_request::Request::InputItem(first) = input(first_tail.next().await.unwrap())
    else {
        panic!()
    };
    drop(h.response_txs.remove(0));
    let Call::Resume(resume, mut resumed_tail) = call(&mut h.calls).await else {
        panic!()
    };
    send(&h.response_txs[0], resumed_accepted(&resume, None)).await;
    let invocation_request::Request::InputItem(replayed) =
        input(resumed_tail.next().await.unwrap())
    else {
        panic!()
    };
    assert_eq!(replayed.sequence, first.sequence);
    assert_eq!(replayed.payload, first.payload);
    assert_eq!(replayed.epoch, 8);
    session.input.send_chunk(b"c".to_vec()).await.unwrap();
    session.input.finish().await.unwrap();
    let invocation_request::Request::InputItem(next) = input(resumed_tail.next().await.unwrap())
    else {
        panic!()
    };
    assert_eq!(next.sequence, first.sequence + 1);
    assert!(matches!(
        input(resumed_tail.next().await.unwrap()),
        invocation_request::Request::InputEnd(_)
    ));
    assert!(
        timeout(Duration::from_millis(30), h.calls.recv())
            .await
            .is_err()
    );
}

#[test]
#[test_timeout("10s")]
async fn lifecycle_output_backpressure_reserves_capacity_before_polling() {
    let case = corpus_case("lifecycle-output-backpressure");
    let frame_budget = case["input"]["frame_budget"].as_u64().unwrap() as usize;
    let chunks = case["input"]["producer_chunks_hex"]
        .as_array()
        .unwrap()
        .iter()
        .map(|chunk| decode_hex(chunk.as_str().unwrap()))
        .collect::<Vec<_>>();
    let (mut h, ready) = counting_harness();
    let mut l = limits();
    l.event_queue_frames = frame_budget;
    let mut session = HttpSession::start_with_transport(start(), h.transport.clone(), l).unwrap();
    let Call::Start(_, _) = call(&mut h.calls).await else {
        panic!()
    };
    send(&h.response_txs[0], accepted(1, None)).await;
    let _ = event(&mut session).await;
    send(&h.response_txs[0], stream_result()).await;
    let _ = event(&mut session).await;
    let baseline = ready.load(Ordering::SeqCst);
    for (sequence, chunk) in chunks.iter().enumerate() {
        send(&h.response_txs[0], output_bytes(sequence as u64, chunk, 1)).await;
    }
    timeout(Duration::from_secs(1), async {
        while ready.load(Ordering::SeqCst) < baseline + frame_budget {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    tokio::time::sleep(Duration::from_millis(30)).await;
    assert_eq!(
        ready.load(Ordering::SeqCst),
        baseline + frame_budget,
        "the transport polled a frame without first reserving an event slot"
    );

    let mut body = Vec::new();
    for _ in 0..chunks.len() {
        body.extend(event_bytes(event(&mut session).await).unwrap());
    }
    assert_eq!(
        body,
        decode_hex(case["expect"]["body_hex"].as_str().unwrap())
    );
    assert_eq!(
        case["expect"]["maximum_buffered_bytes"].as_u64().unwrap(),
        chunks[..frame_budget].iter().map(Vec::len).sum::<usize>() as u64
    );
}

#[test]
#[test_timeout("10s")]
async fn lifecycle_input_eof_is_not_invocation_cancellation() {
    let case = corpus_case("lifecycle-input-eof-not-cancellation");
    let mut h = harness(1);
    let mut session =
        HttpSession::start_with_transport(start(), h.transport.clone(), limits()).unwrap();
    let Call::Start(_, mut tail) = call(&mut h.calls).await else {
        panic!()
    };
    send(&h.response_txs[0], accepted(1, None)).await;
    assert!(matches!(
        event(&mut session).await,
        HttpSessionEvent::Accepted(_)
    ));

    session.input.finish().await.unwrap();
    let invocation_request::Request::InputEnd(end) = input(tail.next().await.unwrap()) else {
        panic!("input EOF was not forwarded")
    };
    assert_eq!(end.sequence, 0);
    send(&h.response_txs[0], ack(0, 1)).await;
    send(&h.response_txs[0], scalar_result()).await;
    assert!(matches!(
        event(&mut session).await,
        HttpSessionEvent::Result(_)
    ));
    send(&h.response_txs[0], finished()).await;
    let terminal = event(&mut session).await;
    assert!(
        matches!(terminal, HttpSessionEvent::Finished(_)),
        "unexpected terminal event: {terminal:?}"
    );

    assert_eq!(case["expect"]["invocations"], 1);
    assert_eq!(case["expect"]["invocation_cancellations"], 0);
    assert!(
        timeout(Duration::from_millis(30), h.calls.recv())
            .await
            .is_err()
    );
    assert!(
        timeout(Duration::from_millis(30), h.cleanups.recv())
            .await
            .is_err()
    );
}

#[test]
#[test_timeout("10s")]
async fn lifecycle_disconnect_active_compute_cancels_once_and_releases_blocked_input() {
    let case = corpus_case("lifecycle-disconnect-active-compute");
    let mut h = harness(1);
    let mut l = limits();
    l.retained_input_frames = 1;
    l.retained_input_bytes = 136;
    l.event_queue_frames = 1;
    let mut session = HttpSession::start_with_transport(start(), h.transport.clone(), l).unwrap();
    let Call::Start(_, mut tail) = call(&mut h.calls).await else {
        panic!()
    };
    send(&h.response_txs[0], accepted(1, None)).await;
    let _ = event(&mut session).await;
    session.input.send_chunk(vec![1]).await.unwrap();
    let _ = tail.next().await.unwrap();
    send(&h.response_txs[0], stream_result()).await;

    let mut blocked_input = tokio::spawn({
        let input = session.input.clone();
        async move { input.send_chunk(vec![2]).await }
    });
    assert!(
        timeout(Duration::from_millis(30), &mut blocked_input)
            .await
            .is_err()
    );
    send(&h.response_txs[0], output_item(0)).await;
    session.cancellation.cancel();
    session.cancellation.cancel();

    let mut cancelled_roles = Vec::new();
    for expected in [
        StreamCancelRole::OutputConsumer,
        StreamCancelRole::InputProducer,
    ] {
        let request = timeout(Duration::from_secs(1), tail.next())
            .await
            .unwrap()
            .unwrap();
        let invocation_request::Request::StreamCancel(cancel) = input(request) else {
            panic!("disconnect must cancel both stream directions");
        };
        assert_eq!(cancel.reason(), StreamCancelReason::Cancelled);
        assert_eq!(cancel.epoch, 1);
        assert_eq!(cancel.role(), expected);
        if expected == StreamCancelRole::OutputConsumer {
            assert!(
                timeout(Duration::from_millis(20), tail.next())
                    .await
                    .is_err(),
                "input cancellation must wait for the in-flight ACK"
            );
            send(&h.response_txs[0], ack(0, 1)).await;
        } else {
            assert_eq!(cancel.producer_sequence, 1);
        }
        cancelled_roles.push(cancel.role());
    }
    assert!(cancelled_roles.contains(&StreamCancelRole::InputProducer));
    assert!(cancelled_roles.contains(&StreamCancelRole::OutputConsumer));

    let result = timeout(Duration::from_secs(1), blocked_input)
        .await
        .unwrap()
        .unwrap();
    assert!(
        result.is_err(),
        "a blocked request-body producer retained its credit"
    );
    timeout(Duration::from_secs(1), h.cleanups.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(
        timeout(Duration::from_millis(30), h.cleanups.recv())
            .await
            .is_err()
    );
    assert!(
        timeout(Duration::from_millis(30), h.calls.recv())
            .await
            .is_err()
    );
    assert_eq!(case["expect"]["invocations"], 1);
    assert_eq!(case["expect"]["invocation_cancellations"], 1);
    assert_eq!(
        case["expect"]["cancelled_streams"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
}

#[test]
#[test_timeout("10s")]
async fn dropping_response_owner_sends_cancellation_before_closing_transport() {
    for _ in 0..16 {
        let mut h = harness(1);
        let mut limits = limits();
        limits.cleanup_timeout = Duration::from_secs(1);
        let mut session =
            HttpSession::start_with_transport(start(), h.transport.clone(), limits).unwrap();
        let Call::Start(_, mut tail) = call(&mut h.calls).await else {
            panic!()
        };
        send(&h.response_txs[0], accepted(1, None)).await;
        let _ = event(&mut session).await;
        send(&h.response_txs[0], stream_result()).await;
        let _ = event(&mut session).await;
        let guard = session.drop_guard();
        drop(session);
        drop(guard);

        let mut roles = Vec::new();
        for _ in 0..2 {
            let request = timeout(Duration::from_secs(1), tail.next())
                .await
                .expect("cancellation must not wait for the cleanup deadline")
                .expect("owner drop must send cancellations before closing transport");
            let invocation_request::Request::StreamCancel(cancel) = input(request) else {
                panic!("expected stream cancellation")
            };
            assert_eq!(cancel.reason(), StreamCancelReason::Cancelled);
            assert_eq!(cancel.epoch, 1);
            roles.push(cancel.role());
        }
        assert!(roles.contains(&StreamCancelRole::InputProducer));
        assert!(roles.contains(&StreamCancelRole::OutputConsumer));
        send(&h.response_txs[0], producer_cancel(0)).await;
        send(&h.response_txs[0], finished()).await;
        assert!(
            timeout(Duration::from_millis(150), h.cleanups.recv())
                .await
                .is_err()
        );
    }
}

#[test]
#[test_timeout("10s")]
async fn dropping_owner_before_acceptance_cancels_late_output_mapping() {
    let mut h = harness(1);
    let mut limits = limits();
    limits.cleanup_timeout = Duration::from_secs(1);
    let session = HttpSession::start_with_transport(start(), h.transport.clone(), limits).unwrap();
    let Call::Start(_, mut tail) = call(&mut h.calls).await else {
        panic!()
    };
    let guard = session.drop_guard();
    drop(session);
    drop(guard);
    send(&h.response_txs[0], accepted(1, None)).await;
    let request = timeout(Duration::from_secs(1), tail.next())
        .await
        .unwrap()
        .unwrap();
    let invocation_request::Request::StreamCancel(cancel) = input(request) else {
        panic!("expected input cancellation")
    };
    assert_eq!(cancel.role(), StreamCancelRole::InputProducer);
    send(&h.response_txs[0], stream_result()).await;
    let request = timeout(Duration::from_secs(1), tail.next())
        .await
        .unwrap()
        .unwrap();
    let invocation_request::Request::StreamCancel(cancel) = input(request) else {
        panic!("expected output cancellation")
    };
    assert_eq!(cancel.role(), StreamCancelRole::OutputConsumer);
    assert_eq!(cancel.reason(), StreamCancelReason::Cancelled);
    send(&h.response_txs[0], producer_cancel(0)).await;
    send(&h.response_txs[0], finished()).await;
    assert!(
        timeout(Duration::from_millis(150), h.cleanups.recv())
            .await
            .is_err()
    );
}

#[test]
#[test_timeout("10s")]
async fn disconnect_finished_during_drain_skips_fallback_cleanup() {
    let mut h = harness(1);
    let mut session =
        HttpSession::start_with_transport(start(), h.transport.clone(), limits()).unwrap();
    let Call::Start(_, mut tail) = call(&mut h.calls).await else {
        panic!()
    };
    send(&h.response_txs[0], accepted(1, None)).await;
    let _ = event(&mut session).await;
    send(&h.response_txs[0], stream_result()).await;
    let _ = event(&mut session).await;
    session.cancellation.cancel();
    for _ in 0..2 {
        assert!(matches!(
            input(tail.next().await.unwrap()),
            invocation_request::Request::StreamCancel(_)
        ));
    }
    send(&h.response_txs[0], producer_cancel(0)).await;
    send(&h.response_txs[0], finished()).await;
    assert!(matches!(
        event(&mut session).await,
        HttpSessionEvent::Error(HttpSessionError::Cancelled)
    ));
    assert!(
        timeout(Duration::from_millis(150), h.cleanups.recv())
            .await
            .is_err()
    );
    assert!(
        timeout(Duration::from_millis(30), h.calls.recv())
            .await
            .is_err()
    );
}

#[test]
#[test_timeout("10s")]
async fn lifecycle_epoch_loss_fails_without_reinvocation() {
    let case = corpus_case("lifecycle-epoch-loss-no-reinvoke");
    let mut h = harness(2);
    let mut session =
        HttpSession::start_with_transport(start(), h.transport.clone(), limits()).unwrap();
    let Call::Start(_, _) = call(&mut h.calls).await else {
        panic!()
    };
    send(&h.response_txs[0], accepted(7, None)).await;
    let _ = event(&mut session).await;
    drop(h.response_txs.remove(0));
    let Call::Resume(resume, _) = call(&mut h.calls).await else {
        panic!()
    };
    assert_eq!(
        resume.expected_epoch,
        case["input"]["epoch"].as_u64().unwrap()
    );
    send(&h.response_txs[0], accepted(9, None)).await;
    assert!(matches!(
        event(&mut session).await,
        HttpSessionEvent::Error(HttpSessionError::Protocol(_))
    ));
    timeout(Duration::from_secs(1), h.cleanups.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(case["expect"]["invocations"], 1);
    assert_eq!(case["expect"]["replacement_invocations"], 0);
    assert!(
        timeout(Duration::from_millis(30), h.calls.recv())
            .await
            .is_err()
    );
}

#[test]
#[test_timeout("10s")]
async fn lifecycle_executor_loss_after_output_fails_without_second_start() {
    let case = corpus_case("lifecycle-executor-loss-after-head");
    let mut h = harness(2);
    let mut l = limits();
    l.max_resume_attempts = 1;
    let mut session = HttpSession::start_with_transport(start(), h.transport.clone(), l).unwrap();
    let _ = call(&mut h.calls).await;
    send(&h.response_txs[0], accepted(1, None)).await;
    let _ = event(&mut session).await;
    send(&h.response_txs[0], stream_result()).await;
    let _ = event(&mut session).await;
    send(&h.response_txs[0], output_bytes(0, b"a", 1)).await;
    assert_eq!(event_bytes(event(&mut session).await).unwrap(), b"a");
    drop(h.response_txs.remove(0));
    let Call::Resume(_, _) = call(&mut h.calls).await else {
        panic!()
    };
    drop(h.response_txs.remove(0));
    assert!(matches!(
        event(&mut session).await,
        HttpSessionEvent::Error(HttpSessionError::ResumeExhausted)
    ));
    timeout(Duration::from_secs(1), h.cleanups.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(case["expect"]["invocations"], 1);
    assert_eq!(case["expect"]["replacement_invocations"], 0);
    assert!(
        timeout(Duration::from_millis(30), h.calls.recv())
            .await
            .is_err()
    );
}

#[test]
#[test_timeout("10s")]
async fn lifecycle_serving_owner_drop_releases_resources_without_second_start() {
    let case = corpus_case("lifecycle-serving-process-loss");
    let mut h = harness(1);
    let session =
        HttpSession::start_with_transport(start(), h.transport.clone(), limits()).unwrap();
    let Call::Start(_, _) = call(&mut h.calls).await else {
        panic!()
    };
    send(&h.response_txs[0], accepted(1, None)).await;
    drop(session.events);
    timeout(Duration::from_secs(1), h.cleanups.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(case["expect"]["replacement_invocations"], 0);
    assert!(
        timeout(Duration::from_millis(30), h.calls.recv())
            .await
            .is_err()
    );
    assert!(
        timeout(Duration::from_millis(30), h.cleanups.recv())
            .await
            .is_err()
    );
}
