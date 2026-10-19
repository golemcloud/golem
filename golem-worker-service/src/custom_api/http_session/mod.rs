// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE

//! Private invocation transport for the custom HTTP adapter.
//!
//! A session sends one [`InvocationStart`] and, only after acceptance, may reconnect with
//! [`ResumeAttach`]. Input frames retain their memory credits until an ACK or accepted resume
//! high-water mark proves that the executor persisted them. Output offsets are retained as resume
//! cursors so reconnects continue after the last value delivered to the HTTP adapter.
//!
//! Losing a transport only detaches an attachment; it does not cancel the invocation or either
//! durable stream. Cancellation is explicit: owner cancellation sends stream terminals when
//! possible and then invokes worker cleanup if no completion was confirmed. An ambiguous resume
//! loss replays the exact attempt. Only an authoritative attachment-state rejection changes
//! Resume to Takeover (or vice versa); epoch rejection is terminal rather than guessed around.

use golem_api_grpc::proto::golem::worker::{InvocationAccepted, InvocationStart, StreamCancel};
use golem_common::model::{AgentId, IdempotencyKey as ModelIdempotencyKey};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use tokio::sync::{Semaphore, mpsc};
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;
use tracing::{Instrument, debug, info_span};

use crate::service::worker::WorkerService;

mod driver;
mod progress;
mod transport;

use driver::run_session;
use driver::validate_prepared_start;
use progress::{Command, InputCredits};
use transport::{CleanupOutcome, SessionTransport};

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
    pub(super) failure: Arc<OnceLock<HttpSessionError>>,
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
pub(super) struct SessionOutput {
    pub(super) sender: mpsc::Sender<HttpSessionEvent>,
    pub(super) failure: Arc<OnceLock<HttpSessionError>>,
}

impl SessionOutput {
    pub(super) async fn send(
        &self,
        event: HttpSessionEvent,
    ) -> Result<(), mpsc::error::SendError<HttpSessionEvent>> {
        self.sender.send(event).await
    }

    pub(super) fn try_send(
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

    pub(super) async fn closed(&self) {
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
        Self::start_with_transport(prepared_start, worker_service, limits)
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

#[cfg(test)]
mod tests;
