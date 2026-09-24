// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE

//! Session protocol driver and cancellation drain.
//!
//! A transport loss after a resume request is ambiguous: `RecoveryProgress::pending` retains the
//! exact attempt ID and operation until acceptance or an authoritative attachment-state rejection.
//! Retained input credits are released only after an ACK or accepted high-water mark proves that
//! the executor persisted the corresponding frame.

use futures::StreamExt;
use golem_api_grpc::invocation_session_protocol::InvocationSessionState;
use golem_api_grpc::proto::golem::worker::{
    DurableStreamMapping, InvocationAccepted, InvocationRejectionReason, InvocationRequest,
    InvocationStart, ResumeAttach, ResumeOperation, StreamCancel, StreamCancelReason,
    StreamCancelRole, StreamCursor, StreamMappingRole, input_stream_item, invocation_request,
    invocation_response,
};
use golem_common::model::AgentId;
use prost::Message;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::mpsc;
use tokio::time::{Instant, sleep};
use tokio_util::sync::CancellationToken;
use tracing::debug;

use super::progress::{AcceptedIdentity, Command, InputProgress, OutputProgress, RecoveryProgress};
use super::transport::SessionTransport;
use super::{
    HttpSessionError, HttpSessionEvent, HttpSessionLimits, INPUT_STREAM_ID, SessionOutput,
};
use crate::service::worker::{InvocationRequestStream, InvocationResponseStream};

pub(super) async fn run_session(
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
            let resume = recovery
                .pending
                .get_or_insert_with(|| {
                    make_resume(identity, &output.cursors, start.principal.clone())
                })
                .clone();
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
        let current = match decision.response.unwrap() {
            invocation_response::Response::Accepted(current) => current,
            invocation_response::Response::Rejected(rejected)
                if accepted.is_some()
                    && rejected.reason() == InvocationRejectionReason::InvalidAttachmentState =>
            {
                let Some(previous) = recovery.pending.take() else {
                    send_error(&events, HttpSessionError::Rejected).await;
                    return;
                };
                let operation = if previous.operation() == ResumeOperation::Resume {
                    ResumeOperation::Takeover
                } else {
                    ResumeOperation::Resume
                };
                recovery.pending = Some(make_resume(
                    accepted.as_ref().unwrap(),
                    &output.cursors,
                    start.principal.clone(),
                ));
                recovery.pending.as_mut().unwrap().operation = operation as i32;
                continue;
            }
            invocation_response::Response::Rejected(_) => {
                send_error(&events, HttpSessionError::Rejected).await;
                return;
            }
            _ => unreachable!(),
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
            recovery.pending = None;
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

pub(super) fn validate_prepared_start(start: &InvocationStart) -> Result<(), HttpSessionError> {
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
        operation: ResumeOperation::Resume as i32,
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

pub(super) fn output_cancel_request(
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

pub(super) fn request_end_sequence(request: &InvocationRequest) -> u64 {
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

pub(super) fn uuid_pair(uuid: &golem_api_grpc::proto::golem::common::Uuid) -> (u64, u64) {
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
