// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE

//! Retry-owning invocation sessions used by the custom HTTP adapter.

use async_trait::async_trait;
use futures::StreamExt;
use golem_api_grpc::invocation_session_protocol::InvocationSessionState;
use golem_api_grpc::proto::golem::schema::{ListValue, SchemaValue, schema_value};
use golem_api_grpc::proto::golem::worker::input_stream_item;
use golem_api_grpc::proto::golem::worker::{
    DurableStreamMapping, InputStreamEnd, InputStreamItem, InvocationAccepted, InvocationRequest,
    InvocationStart, ResumeAttach, ResumeOperation, StreamCancel, StreamCancelReason,
    StreamCancelRole, StreamCursor, StreamMappingRole, invocation_request, invocation_response,
    invocation_session_result,
};
use golem_common::model::{AgentId, IdempotencyKey as ModelIdempotencyKey};
use golem_service_base::model::auth::AuthCtx;
use prost::Message;
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use tokio::sync::{OwnedSemaphorePermit, Semaphore, mpsc};
use tokio::time::{Instant, sleep};
use tokio_util::sync::CancellationToken;
use tracing::{Instrument, debug, info_span};

use crate::service::worker::{
    InvocationRequestStream, InvocationResponseStream, WorkerService, WorkerServiceError,
};

const INPUT_STREAM_ID: u64 = 1;
const MAX_RAW_CHUNK: usize = 64 * 1024;

pub use crate::config::HttpSessionLimits;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HttpSessionError {
    InvalidStart(String),
    Rejected,
    Protocol(String),
    TransportUnavailable,
    RetainedInputLimit,
    ResumeExhausted,
    Cancelled,
    Deadline,
}

impl HttpSessionError {
    fn metric_label(&self) -> &'static str {
        match self {
            Self::InvalidStart(_) => "invalid_start",
            Self::Rejected => "rejected",
            Self::Protocol(_) => "protocol",
            Self::TransportUnavailable => "transport_unavailable",
            Self::RetainedInputLimit => "frame_limit",
            Self::ResumeExhausted => "resume_exhausted",
            Self::Cancelled => "cancelled",
            Self::Deadline => "deadline",
        }
    }
}

#[derive(Clone)]
pub enum HttpSessionEvent {
    Accepted(InvocationAccepted),
    Result(Box<golem_api_grpc::proto::golem::worker::InvocationSessionResult>),
    OutputItem(golem_api_grpc::proto::golem::worker::OutputStreamItem),
    OutputEnd(golem_api_grpc::proto::golem::worker::OutputStreamEnd),
    OutputError(golem_api_grpc::proto::golem::worker::OutputStreamError),
    StreamCancel(StreamCancel),
    Finished(golem_api_grpc::proto::golem::worker::InvocationSessionCompletion),
    Error(HttpSessionError),
}

impl std::fmt::Debug for HttpSessionEvent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Accepted(_) => "Accepted",
            Self::Result(_) => "Result",
            Self::OutputItem(_) => "OutputItem",
            Self::OutputEnd(_) => "OutputEnd",
            Self::OutputError(_) => "OutputError",
            Self::StreamCancel(_) => "StreamCancel",
            Self::Finished(_) => "Finished",
            Self::Error(_) => "Error",
        })
    }
}

struct InputCredits {
    _bytes: OwnedSemaphorePermit,
    _frame: OwnedSemaphorePermit,
}

enum Command {
    Input(Vec<u8>, InputCredits),
    InputEof(InputCredits),
    DisposeInput,
    DisposeOutput { stream_id: u64 },
}

struct PreparedCommand {
    request: InvocationRequest,
    credits: Option<InputCredits>,
}

impl Command {
    fn prepare(
        self,
        input: &mut InputProgress,
        output: &mut OutputProgress,
        epoch: u64,
    ) -> Result<Option<PreparedCommand>, HttpSessionError> {
        let (request, credits) = match self {
            Self::Input(bytes, credits) => {
                if input.terminal || input.disposal_pending {
                    return Ok(None);
                }
                let sequence = input.next_sequence;
                input.next_sequence += 1;
                let item = InputStreamItem {
                    transport_stream_id: INPUT_STREAM_ID,
                    sequence,
                    payload: Some(input_stream_item::Payload::Value(SchemaValue {
                        value: Some(schema_value::Value::ListValue(ListValue {
                            elements: bytes
                                .into_iter()
                                .map(|byte| SchemaValue {
                                    value: Some(schema_value::Value::U8Value(byte as u32)),
                                })
                                .collect(),
                        })),
                    })),
                    durable_stream_id: None,
                    epoch: 0,
                };
                (
                    InvocationRequest {
                        request: Some(invocation_request::Request::InputItem(item)),
                    },
                    Some(credits),
                )
            }
            Self::InputEof(credits) => {
                if input.terminal || input.disposal_pending {
                    return Ok(None);
                }
                input.terminal = true;
                (
                    InvocationRequest {
                        request: Some(invocation_request::Request::InputEnd(InputStreamEnd {
                            transport_stream_id: INPUT_STREAM_ID,
                            sequence: input.next_sequence,
                            durable_stream_id: None,
                            epoch: 0,
                        })),
                    },
                    Some(credits),
                )
            }
            Self::DisposeInput => {
                if !input.terminal {
                    input.disposal_pending = true;
                }
                return Ok(None);
            }
            Self::DisposeOutput { stream_id } => {
                let durable_id = *output.controls.get(&stream_id).ok_or_else(|| {
                    HttpSessionError::Protocol(format!("output stream {stream_id} is unknown"))
                })?;
                if output.terminals.contains(&durable_id)
                    || !output.pending_disposals.insert(stream_id)
                {
                    return Ok(None);
                }
                (
                    output_cancel_request(
                        stream_id,
                        durable_id,
                        epoch,
                        StreamCancelReason::ConsumerDrop,
                    ),
                    None,
                )
            }
        };
        Ok(Some(PreparedCommand { request, credits }))
    }
}

#[derive(Clone)]
pub struct HttpSessionInput {
    tx: mpsc::Sender<Command>,
    control_tx: mpsc::Sender<Command>,
    input_bytes: Arc<Semaphore>,
    input_frames: Arc<Semaphore>,
    max_chunk: usize,
}

impl HttpSessionInput {
    async fn credits(&self, raw_bytes: usize) -> Result<InputCredits, HttpSessionError> {
        // A canonical list<u8> uses a protobuf node per byte. Charge a conservative
        // encoded bound, including durable identity, sequence and envelope overhead.
        let bytes = raw_bytes * 8 + 128;
        let frame = self
            .input_frames
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| HttpSessionError::Cancelled)?;
        let bytes = self
            .input_bytes
            .clone()
            .acquire_many_owned(bytes as u32)
            .await
            .map_err(|_| HttpSessionError::Cancelled)?;
        Ok(InputCredits {
            _bytes: bytes,
            _frame: frame,
        })
    }

    pub async fn send_chunk(&self, bytes: impl Into<Vec<u8>>) -> Result<(), HttpSessionError> {
        let bytes = bytes.into();
        if bytes.is_empty() {
            let permit = self.credits(0).await?;
            return self
                .tx
                .send(Command::Input(bytes, permit))
                .await
                .map_err(|_| HttpSessionError::Cancelled);
        }
        for chunk in bytes.chunks(self.max_chunk) {
            let permit = self.credits(chunk.len()).await?;
            self.tx
                .send(Command::Input(chunk.to_vec(), permit))
                .await
                .map_err(|_| HttpSessionError::Cancelled)?;
        }
        Ok(())
    }

    pub async fn finish(&self) -> Result<(), HttpSessionError> {
        let permit = self.credits(0).await?;
        self.tx
            .send(Command::InputEof(permit))
            .await
            .map_err(|_| HttpSessionError::Cancelled)
    }

    pub async fn dispose(&self) -> Result<(), HttpSessionError> {
        self.control_tx
            .send(Command::DisposeInput)
            .await
            .map_err(|_| HttpSessionError::Cancelled)
    }

    pub async fn dispose_output(&self, stream_id: u64) -> Result<(), HttpSessionError> {
        self.control_tx
            .send(Command::DisposeOutput { stream_id })
            .await
            .map_err(|_| HttpSessionError::Cancelled)
    }
}

#[derive(Clone)]
pub struct HttpSessionCancellation {
    token: CancellationToken,
}

impl HttpSessionCancellation {
    pub fn cancel(&self) {
        self.token.cancel();
    }
}

/// A request/body lifetime guard. Dropping an armed guard cancels accepted ephemeral work.
pub struct HttpSessionDropGuard {
    cancellation: Option<HttpSessionCancellation>,
}

impl HttpSessionDropGuard {
    pub fn disarm(&mut self) {
        self.cancellation = None;
    }
}

impl Drop for HttpSessionDropGuard {
    fn drop(&mut self) {
        if let Some(cancellation) = &self.cancellation {
            cancellation.cancel();
        }
    }
}

pub struct HttpSession {
    pub input: HttpSessionInput,
    pub events: HttpSessionEvents,
    pub cancellation: HttpSessionCancellation,
}

/// The terminal error has its own slot so a full output queue cannot lose it.
pub struct HttpSessionEvents {
    receiver: mpsc::Receiver<HttpSessionEvent>,
    failure: Arc<OnceLock<HttpSessionError>>,
    reported_failure: bool,
}

impl HttpSessionEvents {
    pub async fn recv(&mut self) -> Option<HttpSessionEvent> {
        if let Some(event) = self.receiver.recv().await {
            return Some(event);
        }
        if !self.reported_failure {
            self.reported_failure = true;
            return self.failure.get().cloned().map(HttpSessionEvent::Error);
        }
        None
    }

    pub fn failure(&self) -> Option<&HttpSessionError> {
        self.failure.get()
    }
}

#[derive(Clone)]
struct SessionOutput {
    sender: mpsc::Sender<HttpSessionEvent>,
    failure: Arc<OnceLock<HttpSessionError>>,
}

impl SessionOutput {
    async fn send(
        &self,
        event: HttpSessionEvent,
    ) -> Result<(), mpsc::error::SendError<HttpSessionEvent>> {
        self.sender.send(event).await
    }

    fn try_send(
        &self,
        event: HttpSessionEvent,
    ) -> Result<(), mpsc::error::TrySendError<HttpSessionEvent>> {
        if let HttpSessionEvent::Error(error) = event {
            let _ = self.failure.set(error);
            Ok(())
        } else {
            self.sender.try_send(event)
        }
    }

    async fn closed(&self) {
        self.sender.closed().await;
    }
}

impl HttpSession {
    pub fn drop_guard(&self) -> HttpSessionDropGuard {
        HttpSessionDropGuard {
            cancellation: Some(self.cancellation.clone()),
        }
    }

    pub fn start(
        prepared_start: InvocationStart,
        worker_service: Arc<WorkerService>,
        limits: HttpSessionLimits,
    ) -> Result<Self, HttpSessionError> {
        Self::start_with_transport(
            prepared_start,
            Arc::new(WorkerServiceTransport(worker_service)),
            limits,
        )
    }

    fn start_with_transport(
        prepared_start: InvocationStart,
        transport: Arc<dyn SessionTransport>,
        limits: HttpSessionLimits,
    ) -> Result<Self, HttpSessionError> {
        validate_prepared_start(&prepared_start)?;
        limits.validate().map_err(HttpSessionError::InvalidStart)?;
        let (command_tx, command_rx) = mpsc::channel(limits.command_queue_frames);
        let (control_tx, control_rx) = mpsc::channel(2);
        let input_bytes = Arc::new(Semaphore::new(limits.retained_input_bytes));
        let input_frames = Arc::new(Semaphore::new(limits.retained_input_frames));
        let max_chunk = MAX_RAW_CHUNK.min((limits.retained_input_bytes - 128) / 8);
        let (event_tx, event_rx) = mpsc::channel(limits.event_queue_frames);
        let failure = Arc::new(OnceLock::new());
        let event_tx = SessionOutput {
            sender: event_tx,
            failure: failure.clone(),
        };
        let token = CancellationToken::new();
        let closed_bytes = input_bytes.clone();
        let closed_frames = input_frames.clone();
        let close_token = token.clone();
        let completion = Arc::new(AtomicBool::new(false));
        let completed = completion.clone();
        let cleanup_start = prepared_start.clone();
        let cleanup_transport = transport.clone();
        let cleanup_limits = limits.clone();
        let cleanup_events = event_tx.clone();
        let terminal_error = failure.clone();
        let session_span = info_span!(
            "http_invocation_session",
            agent = ?cleanup_start.agent_id,
            idempotency_key = cleanup_start.idempotency_key.as_ref().map(|key| key.value.as_str()),
        );
        tokio::spawn(async move {
            let outcome = {
                let session = run_session(
                    prepared_start,
                    transport,
                    limits,
                    command_rx,
                    control_rx,
                    event_tx,
                    completed,
                    close_token.clone(),
                );
                tokio::pin!(session);
                tokio::select! {
                    _ = &mut session => close_token
                        .is_cancelled()
                        .then_some(HttpSessionError::Cancelled),
                    _ = close_token.cancelled() => {
                        let _ = tokio::time::timeout(cleanup_limits.cleanup_timeout, &mut session).await;
                        Some(HttpSessionError::Cancelled)
                    },
                    _ = cleanup_events.closed() => {
                        close_token.cancel();
                        let _ = tokio::time::timeout(cleanup_limits.cleanup_timeout, &mut session).await;
                        Some(HttpSessionError::Cancelled)
                    },
                    _ = tokio::time::sleep(cleanup_limits.exchange_timeout) => {
                        close_token.cancel();
                        let _ = tokio::time::timeout(cleanup_limits.cleanup_timeout, &mut session).await;
                        Some(HttpSessionError::Deadline)
                    },
                }
            };
            closed_bytes.close();
            closed_frames.close();
            // Publish the failure and close the event sender before potentially slow cleanup.
            let record_cause = outcome.is_some() || !completion.load(Ordering::Acquire);
            if let Some(error) = outcome {
                debug!(category = error.metric_label(), "HTTP session terminated");
                let _ = cleanup_events.try_send(HttpSessionEvent::Error(error));
            }
            drop(cleanup_events);
            if record_cause {
                let cause = terminal_error.get().unwrap_or(&HttpSessionError::Cancelled);
                crate::metrics::record_http_session_terminal_cause(cause.metric_label());
                if matches!(cause, HttpSessionError::Cancelled) {
                    crate::metrics::record_http_session_explicit_cancellation();
                }
            }
            if !completion.load(Ordering::Acquire) {
                // Prepared identity is final and private, even before Accepted arrives.
                if let (Some(agent), Some(key)) =
                    (cleanup_start.agent_id, cleanup_start.idempotency_key)
                    && let Ok(agent) = AgentId::try_from(agent)
                {
                    let started = Instant::now();
                    let cleanup = tokio::time::timeout(
                        cleanup_limits.cleanup_timeout,
                        cleanup_transport.cleanup(agent, ModelIdempotencyKey::new(key.value)),
                    )
                    .await;
                    let outcome = match cleanup {
                        Ok(CleanupOutcome::FinishedUnconfirmed) => "rpc_finished_unconfirmed",
                        Ok(CleanupOutcome::RpcFailure) => "rpc_failure",
                        Err(_) => "timeout",
                    };
                    debug!(outcome, "HTTP session cleanup finished");
                    crate::metrics::record_http_session_cleanup_outcome(outcome, started.elapsed());
                }
            }
        }.instrument(session_span));
        Ok(Self {
            input: HttpSessionInput {
                tx: command_tx,
                control_tx,
                input_bytes,
                input_frames,
                max_chunk,
            },
            events: HttpSessionEvents {
                receiver: event_rx,
                failure,
                reported_failure: false,
            },
            cancellation: HttpSessionCancellation { token },
        })
    }
}

#[async_trait]
trait SessionTransport: Send + Sync {
    async fn start(
        &self,
        start: InvocationStart,
        tail: InvocationRequestStream,
    ) -> Result<InvocationResponseStream, WorkerServiceError>;
    async fn resume(
        &self,
        resume: ResumeAttach,
        tail: InvocationRequestStream,
    ) -> Result<InvocationResponseStream, WorkerServiceError>;
    async fn cleanup(&self, agent: AgentId, key: ModelIdempotencyKey) -> CleanupOutcome;
}

enum CleanupOutcome {
    FinishedUnconfirmed,
    RpcFailure,
}

struct WorkerServiceTransport(Arc<WorkerService>);

#[async_trait]
impl SessionTransport for WorkerServiceTransport {
    async fn start(
        &self,
        start: InvocationStart,
        tail: InvocationRequestStream,
    ) -> Result<InvocationResponseStream, WorkerServiceError> {
        self.0
            .invoke_agent_session(start, tail, true, AuthCtx::System)
            .await
    }

    async fn resume(
        &self,
        resume: ResumeAttach,
        tail: InvocationRequestStream,
    ) -> Result<InvocationResponseStream, WorkerServiceError> {
        self.0
            .resume_agent_session(resume, tail, AuthCtx::System)
            .await
    }

    async fn cleanup(&self, agent: AgentId, key: ModelIdempotencyKey) -> CleanupOutcome {
        let cancel = self.0.cancel_invocation(&agent, &key, AuthCtx::System);
        let interrupt = self.0.interrupt(&agent, false, AuthCtx::System);
        let (cancel, interrupt) = tokio::join!(cancel, interrupt);
        if cancel.is_ok() && interrupt.is_ok() {
            CleanupOutcome::FinishedUnconfirmed
        } else {
            CleanupOutcome::RpcFailure
        }
    }
}

struct RetainedInput {
    request: InvocationRequest,
    encoded_bytes: usize,
    end_sequence: u64,
    _permit: Option<InputCredits>,
}

struct AcceptedIdentity {
    accepted: InvocationAccepted,
    input_mapping: DurableStreamMapping,
}

#[derive(Default)]
struct InputProgress {
    retained: VecDeque<RetainedInput>,
    retained_bytes: usize,
    next_sequence: u64,
    terminal: bool,
    disposal_pending: bool,
}

impl InputProgress {
    fn can_read(&self, limits: &HttpSessionLimits) -> bool {
        self.retained.len() < limits.retained_input_frames
            && self.retained_bytes < limits.retained_input_bytes
    }

    fn acknowledge(&mut self, sequence: u64) {
        while self
            .retained
            .front()
            .is_some_and(|frame| frame.end_sequence <= sequence)
        {
            self.retained_bytes -= self.retained.pop_front().unwrap().encoded_bytes;
        }
    }

    fn clear(&mut self) {
        self.retained.clear();
        self.retained_bytes = 0;
    }

    fn apply_high_water(&mut self, mapping: &DurableStreamMapping) {
        if self.disposal_pending {
            self.terminal = mapping
                .high_water
                .as_ref()
                .is_some_and(|water| water.terminal);
        }
        if let Some(high_water) = mapping.high_water.as_ref() {
            if high_water.terminal {
                self.terminal = true;
                self.clear();
            }
            self.acknowledge(high_water.highest_contiguous_sequence);
        }
    }

    fn retain(
        &mut self,
        request: InvocationRequest,
        permit: Option<InputCredits>,
        limits: &HttpSessionLimits,
    ) -> Result<(), HttpSessionError> {
        if permit.is_some() && self.retained.len() >= limits.retained_input_frames {
            return Err(HttpSessionError::RetainedInputLimit);
        }
        let retain = permit.is_some()
            || matches!(
                request.request,
                Some(invocation_request::Request::InputEnd(_))
            );
        if retain {
            let encoded_bytes = request.encoded_len();
            self.retained.push_back(RetainedInput {
                end_sequence: request_end_sequence(&request),
                request,
                encoded_bytes,
                _permit: permit,
            });
            self.retained_bytes += encoded_bytes;
        }
        Ok(())
    }
}

#[derive(Default)]
struct OutputProgress {
    cursors: HashMap<(u64, u64), Vec<u8>>,
    controls: HashMap<u64, (u64, u64)>,
    terminals: HashSet<(u64, u64)>,
    pending_disposals: HashSet<u64>,
    result_value: Option<Option<invocation_session_result::Result>>,
}

impl OutputProgress {
    fn observe(&mut self, stream_id: u64, durable_id: (u64, u64), offset: Vec<u8>) {
        self.cursors.insert(durable_id, offset);
        self.controls.insert(stream_id, durable_id);
    }

    fn terminal(&mut self, stream_id: u64, durable_id: (u64, u64), offset: Vec<u8>) {
        self.observe(stream_id, durable_id, offset);
        self.terminals.insert(durable_id);
        self.pending_disposals.remove(&stream_id);
    }

    fn discover_result_streams(
        &mut self,
        mappings: &[DurableStreamMapping],
    ) -> Vec<(u64, (u64, u64))> {
        let mut discovered = Vec::new();
        for mapping in mappings {
            if mapping.role() == StreamMappingRole::Output
                && let Some(id) = mapping
                    .handle
                    .as_ref()
                    .and_then(|handle| handle.stream_id.as_ref())
            {
                let id = uuid_pair(id);
                if let std::collections::hash_map::Entry::Vacant(entry) =
                    self.controls.entry(mapping.transport_stream_id)
                {
                    entry.insert(id);
                    discovered.push((mapping.transport_stream_id, id));
                }
            }
        }
        discovered
    }
}

#[derive(Default)]
struct RecoveryProgress {
    deadline: Option<Instant>,
    started: Option<Instant>,
    attempts: usize,
}

async fn run_session(
    start: InvocationStart,
    transport: Arc<dyn SessionTransport>,
    limits: HttpSessionLimits,
    mut commands: mpsc::Receiver<Command>,
    mut controls: mpsc::Receiver<Command>,
    events: SessionOutput,
    completed: Arc<AtomicBool>,
    cancelled: CancellationToken,
) {
    let mut input = InputProgress::default();
    let mut accepted: Option<AcceptedIdentity> = None;
    let mut output = OutputProgress::default();
    let mut recovery = RecoveryProgress::default();
    let mut first = true;
    let mut commands_open = true;
    let mut controls_open = true;

    'attempts: loop {
        if cancelled.is_cancelled() {
            return;
        }
        let (request_tx, request_rx) =
            mpsc::channel::<InvocationRequest>(limits.retained_input_frames + 2);
        let tail: InvocationRequestStream =
            Box::pin(tokio_stream::wrappers::ReceiverStream::new(request_rx));
        let first_request;
        let call = if first {
            first = false;
            first_request = InvocationRequest {
                request: Some(invocation_request::Request::Start(start.clone())),
            };
            transport.start(start.clone(), tail).await
        } else {
            let Some(identity) = accepted.as_ref() else {
                send_error(&events, HttpSessionError::TransportUnavailable).await;
                return;
            };
            let deadline = *recovery
                .deadline
                .get_or_insert_with(|| Instant::now() + limits.retry_deadline);
            let started = *recovery.started.get_or_insert_with(Instant::now);
            if recovery.attempts >= limits.max_resume_attempts || Instant::now() >= deadline {
                crate::metrics::record_http_session_reattach_outcome(
                    "exhausted",
                    started.elapsed(),
                );
                send_error(&events, HttpSessionError::ResumeExhausted).await;
                return;
            }
            recovery.attempts += 1;
            crate::metrics::record_http_session_reattach_attempt();
            debug!(attempt = recovery.attempts, "HTTP session reattach attempt");
            let resume = make_resume(identity, &output.cursors, start.principal.clone());
            first_request = InvocationRequest {
                request: Some(invocation_request::Request::ResumeAttach(resume.clone())),
            };
            match tokio::time::timeout_at(deadline, async {
                sleep(limits.retry_delay).await;
                transport.resume(resume, tail).await
            })
            .await
            {
                Ok(result) => result,
                Err(_) => {
                    crate::metrics::record_http_session_reattach_outcome(
                        "expired",
                        started.elapsed(),
                    );
                    send_error(&events, HttpSessionError::ResumeExhausted).await;
                    return;
                }
            }
        };
        let mut responses = match call {
            Ok(responses) => responses,
            Err(_) if accepted.is_some() => continue,
            Err(_) => {
                send_error(&events, HttpSessionError::TransportUnavailable).await;
                return;
            }
        };
        let mut protocol = InvocationSessionState::default();
        if let Err(error) = protocol.validate_trusted_request(&first_request) {
            send_error(&events, HttpSessionError::Protocol(error)).await;
            return;
        }
        for durable_id in &output.terminals {
            if let Err(error) = protocol.mark_terminal_resume_cursor(*durable_id) {
                send_error(&events, HttpSessionError::Protocol(error)).await;
                return;
            }
        }
        let decision = if let Some(deadline) = recovery.deadline {
            match tokio::time::timeout_at(deadline, responses.next()).await {
                Ok(decision) => decision,
                Err(_) => {
                    if let Some(started) = recovery.started {
                        crate::metrics::record_http_session_reattach_outcome(
                            "expired",
                            started.elapsed(),
                        );
                    }
                    send_error(&events, HttpSessionError::ResumeExhausted).await;
                    return;
                }
            }
        } else {
            responses.next().await
        };
        let Some(decision) = decision else {
            if accepted.is_some() {
                continue;
            } else {
                send_error(&events, HttpSessionError::TransportUnavailable).await;
                return;
            }
        };
        let decision = match decision {
            Ok(response) => response,
            Err(_) if accepted.is_some() => continue,
            Err(_) => {
                send_error(&events, HttpSessionError::TransportUnavailable).await;
                return;
            }
        };
        if let Err(error) = protocol.validate_response(&decision) {
            if accepted.is_some()
                && matches!(
                    decision.response,
                    Some(invocation_response::Response::Accepted(_))
                )
            {
                crate::metrics::record_http_session_stale_fencing();
            }
            send_error(&events, HttpSessionError::Protocol(error)).await;
            return;
        }
        let invocation_response::Response::Accepted(current) = decision.response.unwrap() else {
            send_error(&events, HttpSessionError::Rejected).await;
            return;
        };
        if let Some(expected) = accepted.as_ref() {
            if current.agent_id != expected.accepted.agent_id
                || current.component_revision != expected.accepted.component_revision
                || current.callee_fingerprint != expected.accepted.callee_fingerprint
                || current.attachment_id != expected.accepted.attachment_id
                || current.epoch != expected.accepted.epoch + 1
            {
                crate::metrics::record_http_session_stale_fencing();
                send_error(
                    &events,
                    HttpSessionError::Protocol("resume acceptance identity changed".into()),
                )
                .await;
                return;
            }
        } else if current.agent_id != start.agent_id || current.method_name != start.method_name {
            send_error(
                &events,
                HttpSessionError::Protocol("acceptance changed target".into()),
            )
            .await;
            return;
        } else {
            match accepted_identity(current.clone()) {
                Ok(identity) => accepted = Some(identity),
                Err(error) => {
                    send_error(&events, error).await;
                    return;
                }
            }
            if !send_event(
                &events,
                &cancelled,
                HttpSessionEvent::Accepted(current.clone()),
            )
            .await
            {
                cancelled.cancel();
            }
        }
        if recovery.deadline.take().is_some() {
            crate::metrics::record_http_session_reattach_outcome(
                "accepted",
                recovery
                    .started
                    .take()
                    .unwrap_or_else(Instant::now)
                    .elapsed(),
            );
            debug!(
                attempts = recovery.attempts,
                outcome = "accepted",
                "HTTP session reattach finished"
            );
        }
        let identity = accepted.as_mut().unwrap();
        let Some(input_mapping) = current
            .stream_mappings
            .iter()
            .find(|mapping| {
                mapping.transport_stream_id == INPUT_STREAM_ID
                    && mapping.role() == StreamMappingRole::Input
            })
            .cloned()
        else {
            send_error(
                &events,
                HttpSessionError::Protocol("acceptance lost input mapping".into()),
            )
            .await;
            return;
        };
        if input_mapping.handle != identity.input_mapping.handle {
            send_error(
                &events,
                HttpSessionError::Protocol("acceptance changed input handle".into()),
            )
            .await;
            return;
        }
        identity.input_mapping = input_mapping;
        identity.accepted = current;
        let epoch = accepted.as_ref().unwrap().accepted.epoch;
        let mapping = accepted.as_ref().unwrap().input_mapping.clone();

        // A resumed attachment may have persisted frames whose acknowledgements were lost.
        input.apply_high_water(&mapping);
        for retained_frame in &input.retained {
            let request = with_attachment(retained_frame.request.clone(), &mapping, epoch);
            if let Err(error) = protocol.validate_trusted_request(&request) {
                send_error(&events, HttpSessionError::Protocol(error)).await;
                return;
            }
            if request_tx.send(request).await.is_err() {
                continue 'attempts;
            }
        }
        // Output disposal is durable only once its terminal response is observed. Reissue any
        // still-pending controls after every accepted attachment.
        for stream_id in output.pending_disposals.iter().copied().collect::<Vec<_>>() {
            let Some(&durable_id) = output.controls.get(&stream_id) else {
                continue;
            };
            if output.terminals.contains(&durable_id) {
                continue;
            }
            let request = output_cancel_request(
                stream_id,
                durable_id,
                epoch,
                StreamCancelReason::ConsumerDrop,
            );
            if let Err(error) = protocol.validate_trusted_request(&request) {
                send_error(&events, HttpSessionError::Protocol(error)).await;
                return;
            }
            if request_tx.send(request).await.is_err() {
                continue 'attempts;
            }
        }

        loop {
            if input.disposal_pending && !input.terminal && input.retained.is_empty() {
                let request = cancel_request(
                    INPUT_STREAM_ID,
                    input.next_sequence,
                    StreamCancelRole::InputProducer,
                    StreamCancelReason::ConsumerDrop,
                    &mapping,
                    epoch,
                );
                if let Err(error) = protocol.validate_trusted_request(&request) {
                    send_error(&events, HttpSessionError::Protocol(error)).await;
                    return;
                }
                input.terminal = true;
                if request_tx.send(request).await.is_err() {
                    continue 'attempts;
                }
            }
            let can_read_input = input.can_read(&limits);
            tokio::select! {
                _ = async {
                    tokio::select! {
                        _ = cancelled.cancelled() => {},
                        _ = events.closed() => { cancelled.cancel(); },
                    }
                } => {
                    commands.close();
                    controls.close();
                    cancel_and_drain(
                        &mut input, &mut output, &mut protocol, &request_tx,
                        &mut responses, &mapping, epoch, &completed,
                    ).await;
                    return;
                }
                response = async {
                    let permit = match events.sender.clone().reserve_owned().await {
                        Ok(permit) => permit,
                        // Owner drop must go through the cancellation branch, not detach.
                        Err(_) => std::future::pending().await,
                    };
                    (permit, responses.next().await)
                } => {
                    let (permit, response) = response;
                    let Some(response) = response else { continue 'attempts };
                    let response = match response { Ok(response) => response, Err(_) => continue 'attempts };
                    if response.encoded_len() > limits.max_event_bytes {
                        send_error(&events, HttpSessionError::RetainedInputLimit).await;
                        return;
                    }
                    if let Err(error) = protocol.validate_response(&response) {
                        send_error(&events, HttpSessionError::Protocol(error)).await;
                        return;
                    }
                    match response.response.unwrap() {
                        invocation_response::Response::InputAck(ack) => {
                            input.acknowledge(ack.highest_contiguous_sequence);
                        }
                        invocation_response::Response::OutputItem(item) => {
                            let id = uuid_pair(item.durable_stream_id.as_ref().unwrap());
                            output.observe(item.transport_stream_id, id, item.durable_offset.clone());
                            if output.pending_disposals.contains(&item.transport_stream_id) {
                                continue;
                            }
                            permit.send(HttpSessionEvent::OutputItem(item));
                        }
                        invocation_response::Response::OutputEnd(end) => {
                            let id = uuid_pair(end.durable_stream_id.as_ref().unwrap());
                            output.terminal(end.transport_stream_id, id, end.durable_offset.clone());
                            permit.send(HttpSessionEvent::OutputEnd(end));
                        }
                        invocation_response::Response::OutputError(error) => {
                            let id = uuid_pair(error.durable_stream_id.as_ref().unwrap());
                            output.terminal(error.transport_stream_id, id, error.durable_offset.clone());
                            permit.send(HttpSessionEvent::OutputError(error));
                        }
                        invocation_response::Response::Result(result) => {
                            output.discover_result_streams(&result.new_stream_mappings);
                            if let Some(expected) = &output.result_value {
                                if expected != &result.result {
                                    send_error(&events, HttpSessionError::Protocol("resumed result changed".into())).await;
                                    return;
                                }
                                continue;
                            }
                            output.result_value = Some(result.result.clone());
                            permit.send(HttpSessionEvent::Result(Box::new(result)));
                        }
                        invocation_response::Response::StreamCancel(cancel) => {
                            if cancel.role() == StreamCancelRole::InputConsumer {
                                input.terminal = true;
                                input.clear();
                            }
                            if cancel.role() == StreamCancelRole::OutputProducer {
                                let id = uuid_pair(cancel.durable_stream_id.as_ref().unwrap());
                                output.terminal(cancel.transport_stream_id, id, cancel.durable_offset.clone());
                            }
                            permit.send(HttpSessionEvent::StreamCancel(cancel));
                        }
                        invocation_response::Response::Finished(finished) => {
                            completed.store(true, Ordering::Release);
                            let category = if matches!(finished.outcome, Some(golem_api_grpc::proto::golem::worker::invocation_session_completion::Outcome::Success(_))) { "success" } else { "invocation_failure" };
                            debug!(category, "HTTP session terminated");
                            crate::metrics::record_http_session_terminal_cause(category);
                            permit.send(HttpSessionEvent::Finished(finished));
                            return;
                        }
                        invocation_response::Response::AttachmentRevoked(_) => continue 'attempts,
                        invocation_response::Response::Accepted(_) | invocation_response::Response::Rejected(_) => unreachable!(),
                    }
                }
                command = async {
                    tokio::select! {
                        biased;
                        command = controls.recv(), if controls_open => {
                            if command.is_none() { controls_open = false; }
                            command
                        },
                        command = commands.recv(), if commands_open && (can_read_input || input.terminal) => {
                            if command.is_none() { commands_open = false; }
                            command
                        },
                        else => std::future::pending().await,
                    }
                } => {
                    let Some(command) = command else {
                        continue;
                    };
                    let prepared = match command.prepare(&mut input, &mut output, epoch) {
                        Ok(Some(prepared)) => prepared,
                        Ok(None) => continue,
                        Err(error) => {
                            send_error(&events, error).await;
                            return;
                        }
                    };
                    let request = with_attachment(prepared.request, &mapping, epoch);
                    if let Err(error) = protocol.validate_trusted_request(&request) {
                        send_error(&events, HttpSessionError::Protocol(error)).await;
                        return;
                    }
                    if let Err(error) = input.retain(request.clone(), prepared.credits, &limits) {
                        send_error(&events, error).await;
                        return;
                    }
                    if request_tx.send(request).await.is_err() { continue 'attempts; }
                }
            }
        }
    }
}

async fn cancel_and_drain(
    input: &mut InputProgress,
    output: &mut OutputProgress,
    protocol: &mut InvocationSessionState,
    requests: &mpsc::Sender<InvocationRequest>,
    responses: &mut InvocationResponseStream,
    mapping: &DurableStreamMapping,
    epoch: u64,
    completed: &AtomicBool,
) {
    // Transport EOF only detaches. Send explicit stream terminals while this attachment
    // is alive, then drain their delivery within the owner's cleanup timeout.
    let mut input_cancel_sent = input.terminal;
    let mut cancellations = Vec::new();
    if !input.terminal && input.retained.is_empty() {
        cancellations.push(cancel_request(
            INPUT_STREAM_ID,
            input.next_sequence,
            StreamCancelRole::InputProducer,
            StreamCancelReason::Cancelled,
            mapping,
            epoch,
        ));
        input_cancel_sent = true;
    }
    for (&stream_id, &durable_id) in &output.controls {
        if !output.terminals.contains(&durable_id) && !output.pending_disposals.contains(&stream_id)
        {
            cancellations.push(output_cancel_request(
                stream_id,
                durable_id,
                epoch,
                StreamCancelReason::Cancelled,
            ));
        }
    }
    for request in cancellations {
        if protocol.validate_trusted_request(&request).is_err()
            || requests.send(request).await.is_err()
        {
            return;
        }
    }
    while let Some(Ok(response)) = responses.next().await {
        if protocol.validate_response(&response).is_err() {
            return;
        }
        match response.response {
            Some(invocation_response::Response::InputAck(ack)) => {
                input.acknowledge(ack.highest_contiguous_sequence);
            }
            Some(invocation_response::Response::StreamCancel(cancel))
                if cancel.role() == StreamCancelRole::InputConsumer =>
            {
                input_cancel_sent = true;
                input.clear();
            }
            Some(invocation_response::Response::Result(result)) => {
                // The guest may return its output after the HTTP owner has gone.
                for (stream_id, durable_id) in
                    output.discover_result_streams(&result.new_stream_mappings)
                {
                    let request = output_cancel_request(
                        stream_id,
                        durable_id,
                        epoch,
                        StreamCancelReason::Cancelled,
                    );
                    if protocol.validate_trusted_request(&request).is_err()
                        || requests.send(request).await.is_err()
                    {
                        return;
                    }
                }
            }
            Some(invocation_response::Response::Finished(_)) => {
                completed.store(true, Ordering::Release);
                return;
            }
            _ => {}
        }
        // An ACK may already have left the executor before we observe it.
        // Only cancel at a sequence both peers have acknowledged.
        if !input_cancel_sent && input.retained.is_empty() {
            let request = cancel_request(
                INPUT_STREAM_ID,
                input.next_sequence,
                StreamCancelRole::InputProducer,
                StreamCancelReason::Cancelled,
                mapping,
                epoch,
            );
            if protocol.validate_trusted_request(&request).is_err()
                || requests.send(request).await.is_err()
            {
                return;
            }
            input_cancel_sent = true;
        }
    }
}

fn validate_prepared_start(start: &InvocationStart) -> Result<(), HttpSessionError> {
    let request = InvocationRequest {
        request: Some(invocation_request::Request::Start(start.clone())),
    };
    InvocationSessionState::default()
        .validate_trusted_request(&request)
        .map_err(HttpSessionError::InvalidStart)?;
    if start.agent_id.is_none() || start.idempotency_key.is_none() {
        return Err(HttpSessionError::InvalidStart(
            "prepared start is missing cleanup identity".into(),
        ));
    }
    Ok(())
}

fn accepted_identity(accepted: InvocationAccepted) -> Result<AcceptedIdentity, HttpSessionError> {
    let _: AgentId = accepted
        .agent_id
        .clone()
        .ok_or_else(|| HttpSessionError::Protocol("acceptance has no agent".into()))?
        .try_into()
        .map_err(HttpSessionError::Protocol)?;
    accepted
        .idempotency_key
        .as_ref()
        .ok_or_else(|| HttpSessionError::Protocol("acceptance has no key".into()))?;
    let input_mapping = accepted
        .stream_mappings
        .iter()
        .find(|mapping| {
            mapping.transport_stream_id == INPUT_STREAM_ID
                && mapping.role() == StreamMappingRole::Input
        })
        .cloned()
        .ok_or_else(|| {
            HttpSessionError::Protocol("acceptance has no canonical input mapping".into())
        })?;
    Ok(AcceptedIdentity {
        accepted,
        input_mapping,
    })
}

fn make_resume(
    identity: &AcceptedIdentity,
    cursors: &HashMap<(u64, u64), Vec<u8>>,
    principal: Option<golem_api_grpc::proto::golem::component::Principal>,
) -> ResumeAttach {
    let mut cursors = cursors
        .iter()
        .map(|(&(high_bits, low_bits), offset)| StreamCursor {
            stream_id: Some(golem_api_grpc::proto::golem::common::Uuid {
                high_bits,
                low_bits,
            }),
            last_observed_offset: Some(offset.clone()),
        })
        .collect::<Vec<_>>();
    cursors.sort_by_key(|cursor| uuid_pair(cursor.stream_id.as_ref().unwrap()));
    ResumeAttach {
        idempotency_key: identity.accepted.idempotency_key.clone(),
        agent_id: identity.accepted.agent_id.clone(),
        environment_id: identity.accepted.environment_id,
        attachment_id: identity.accepted.attachment_id,
        attempt_id: Some(uuid::Uuid::new_v4().into()),
        expected_callee_fingerprint: identity.accepted.callee_fingerprint,
        expected_epoch: identity.accepted.epoch,
        operation: ResumeOperation::Takeover as i32,
        cursors,
        auth_ctx: None,
        principal,
    }
}

fn with_attachment(
    mut request: InvocationRequest,
    mapping: &DurableStreamMapping,
    epoch: u64,
) -> InvocationRequest {
    let durable_stream_id = mapping.handle.as_ref().and_then(|handle| handle.stream_id);
    match request.request.as_mut() {
        Some(invocation_request::Request::InputItem(item)) => {
            item.durable_stream_id = durable_stream_id;
            item.epoch = epoch;
        }
        Some(invocation_request::Request::InputEnd(end)) => {
            end.durable_stream_id = durable_stream_id;
            end.epoch = epoch;
        }
        _ => {}
    }
    request
}

fn cancel_request(
    stream_id: u64,
    sequence: u64,
    role: StreamCancelRole,
    reason: StreamCancelReason,
    mapping: &DurableStreamMapping,
    epoch: u64,
) -> InvocationRequest {
    InvocationRequest {
        request: Some(invocation_request::Request::StreamCancel(StreamCancel {
            transport_stream_id: stream_id,
            producer_sequence: sequence,
            role: role as i32,
            reason: reason as i32,
            details: None,
            durable_stream_id: mapping.handle.as_ref().and_then(|handle| handle.stream_id),
            epoch,
            durable_offset: Vec::new(),
        })),
    }
}

fn output_cancel_request(
    stream_id: u64,
    durable_id: (u64, u64),
    epoch: u64,
    reason: StreamCancelReason,
) -> InvocationRequest {
    InvocationRequest {
        request: Some(invocation_request::Request::StreamCancel(StreamCancel {
            transport_stream_id: stream_id,
            producer_sequence: 0,
            role: StreamCancelRole::OutputConsumer as i32,
            reason: reason as i32,
            details: None,
            durable_stream_id: Some(golem_api_grpc::proto::golem::common::Uuid {
                high_bits: durable_id.0,
                low_bits: durable_id.1,
            }),
            epoch,
            durable_offset: Vec::new(),
        })),
    }
}

fn request_end_sequence(request: &InvocationRequest) -> u64 {
    match request.request.as_ref() {
        Some(invocation_request::Request::InputItem(item)) => {
            item.sequence
                + match item.payload.as_ref() {
                    Some(input_stream_item::Payload::PackedU8(bytes)) => bytes.len() as u64,
                    _ => 1,
                }
                - 1
        }
        Some(invocation_request::Request::InputEnd(end)) => end.sequence,
        Some(invocation_request::Request::StreamCancel(cancel)) => cancel.producer_sequence,
        _ => 0,
    }
}

fn uuid_pair(uuid: &golem_api_grpc::proto::golem::common::Uuid) -> (u64, u64) {
    (uuid.high_bits, uuid.low_bits)
}

async fn send_event(
    events: &SessionOutput,
    cancelled: &CancellationToken,
    event: HttpSessionEvent,
) -> bool {
    tokio::select! { biased; _ = cancelled.cancelled() => false, result = events.send(event) => result.is_ok() }
}

async fn send_error(events: &SessionOutput, error: HttpSessionError) {
    let _ = events.try_send(HttpSessionEvent::Error(error));
}

#[cfg(test)]
mod tests;
