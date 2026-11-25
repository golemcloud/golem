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

use super::{
    AcceptanceRace, AgentInvocationOutput, AgentInvocationResult, Arc, HasAll, HashMap,
    InvocationAccepted, InvocationRejectionReason, InvocationRequest, InvocationResponse,
    InvocationSessionCompletion, InvocationSessionResult, InvocationSessionState, ResponseLease,
    SessionError, SessionStreamRole, StreamSession, UsesAllDeps, WorkerCtx, WorkerExecutorError,
    WorkerExecutorImpl, detach_durable_attachment, durable_stream_mapping_to_proto,
    end_attachment_on_failure, ends_only_the_attachment, invocation_request, invocation_response,
    invocation_session_completion, invocation_session_result, mpsc, persisted_invocation_result,
    pre_acceptance_rejection_reason, proto_agent_id_string, race_invocation_acceptance,
    rejection_worker_error, request_agent_id, request_idempotency_key, route_durable_request,
    send_attachment_revocation, send_protocol_failure, send_rejection,
    send_rejection_with_worker_error, send_unvalidated_rejection, send_worker_failure,
    until_response_closed,
};

use crate::worker::InvocationObservationFailure;
use std::future::{Future, pending};

use super::attachment::preserve_attachment_on_read_failure;

enum InvocationObservation<F> {
    InFlight(F),
    Succeeded(Box<AgentInvocationOutput>),
    InvocationFailed(Box<WorkerExecutorError>),
    ObservationFailed(Box<WorkerExecutorError>),
}

impl<F> InvocationObservation<F>
where
    F: Future<Output = Result<AgentInvocationOutput, WorkerExecutorError>> + Unpin,
{
    fn accepted(
        invocation: F,
        early_output: Option<Result<AgentInvocationOutput, WorkerExecutorError>>,
    ) -> Self {
        match early_output {
            Some(output) => Self::completed(output),
            None => Self::InFlight(invocation),
        }
    }

    fn completed(result: Result<AgentInvocationOutput, WorkerExecutorError>) -> Self {
        match result {
            Ok(output) => Self::Succeeded(Box::new(output)),
            Err(error) if InvocationObservationFailure::classify(&error).is_some() => {
                Self::ObservationFailed(Box::new(error))
            }
            Err(error) => Self::InvocationFailed(Box::new(error)),
        }
    }

    fn is_in_flight(&self) -> bool {
        matches!(self, Self::InFlight(_))
    }

    fn check_observation(&self) -> Result<(), WorkerExecutorError> {
        match self {
            Self::ObservationFailed(error) => Err((**error).clone()),
            _ => Ok(()),
        }
    }

    async fn wait_for_completion(&mut self) {
        match self {
            Self::InFlight(invocation) => *self = Self::completed(invocation.await),
            _ => pending().await,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::FutureExt;
    use std::future::ready;
    use test_r::test;

    #[test]
    fn joined_observer_reports_read_errors_without_repolling_an_observed_invocation() {
        let cancelled = WorkerExecutorError::InvocationCancelled;
        let mut observation = InvocationObservation::accepted(ready(Err(cancelled.clone())), None);
        observation.wait_for_completion().now_or_never().unwrap();
        assert!(
            matches!(&observation, InvocationObservation::InvocationFailed(error) if **error == cancelled)
        );
        assert!(observation.check_observation().is_ok());
        assert!(observation.wait_for_completion().now_or_never().is_none());

        let outage = WorkerExecutorError::recovery_required("temporary result read failure");
        let mut observation = InvocationObservation::accepted(ready(Err(outage.clone())), None);
        observation.wait_for_completion().now_or_never().unwrap();
        assert_eq!(observation.check_observation(), Err(outage.clone()));
        assert!(observation.wait_for_completion().now_or_never().is_none());

        let mut early =
            InvocationObservation::accepted(ready(Err(cancelled)), Some(Err(outage.clone())));
        assert_eq!(early.check_observation(), Err(outage));
        assert!(early.wait_for_completion().now_or_never().is_none());
    }

    #[test]
    fn accepted_observer_keeps_scalar_output() {
        let output = AgentInvocationOutput {
            result: AgentInvocationResult::AgentInitialization,
            consumed_fuel: Some(42),
            invocation_status: None,
            component_revision: None,
            agent_id: None,
            idempotency_key: None,
            oplog_index: None,
            agent_fingerprint: None,
            background_failure: None,
        };
        let mut observation = InvocationObservation::accepted(
            ready(Err(WorkerExecutorError::InvocationCancelled)),
            Some(Ok(output.clone())),
        );
        assert!(observation.wait_for_completion().now_or_never().is_none());
        assert!(
            matches!(observation, InvocationObservation::Succeeded(observed) if *observed == output)
        );
    }

    #[test]
    async fn attachment_check_preserves_storage_errors_and_revokes_stale_attachments() {
        use crate::services::oplog::{OplogFence, OplogReadError, UnreadableOplog};
        use golem_common::model::sharding::ShardEpoch;
        use golem_common::model::{AgentId, OplogIndex};

        let (responses, mut rx) = mpsc::channel(1);
        send_attachment_revocation(
            &responses,
            Err(SessionError::from(
                "StaleEpoch: durable attachment has been fenced",
            )),
        )
        .await;
        assert!(matches!(
            rx.try_recv().unwrap().response,
            Some(invocation_response::Response::AttachmentRevoked(_))
        ));

        let agent_id = AgentId::from_parts(7, 7);
        [
            SessionError::from("durable session has no attachment authority"),
            SessionError::Unreadable(UnreadableOplog::new(
                &agent_id,
                OplogReadError::Gap {
                    start: OplogIndex::INITIAL,
                    end: OplogIndex::INITIAL,
                },
            )),
            SessionError::Fenced(OplogFence {
                agent_id,
                expected_epoch: ShardEpoch(1),
                actual_epoch: Some(ShardEpoch(2)),
            }),
        ]
        .into_iter()
        .for_each(|error| {
            let expected = (!matches!(&error, SessionError::Failed(_))).then(|| {
                error
                    .clone()
                    .into_worker_executor_error(WorkerExecutorError::runtime)
                    .into()
            });
            send_attachment_revocation(&responses, Err(error))
                .now_or_never()
                .unwrap();
            let Some(invocation_response::Response::Finished(InvocationSessionCompletion {
                outcome: Some(invocation_session_completion::Outcome::Failure(failure)),
            })) = rx.try_recv().unwrap().response
            else {
                panic!("expected a worker failure frame");
            };
            assert_eq!(failure.worker_error, expected);
        });
    }
}

impl<Ctx: WorkerCtx, Svcs: HasAll<Ctx> + UsesAllDeps<Ctx = Ctx> + Send + Sync + 'static>
    WorkerExecutorImpl<Ctx, Svcs>
{
    pub(super) async fn run_agent_session(
        &self,
        mut inbound: tonic::Streaming<InvocationRequest>,
        first: Option<InvocationRequest>,
        outward: mpsc::Sender<InvocationResponse>,
        response_lease: ResponseLease,
        scope: &crate::worker::tasks::TaskScope,
    ) {
        let mut state = InvocationSessionState::default();
        let first = match first {
            Some(request) => request,
            None => {
                send_unvalidated_rejection(
                    &outward,
                    InvocationRejectionReason::Protocol,
                    "invocation request transport closed before start".to_string(),
                    None,
                    None,
                    None,
                )
                .await;
                return;
            }
        };
        if let Err(error) = state.validate_trusted_request(&first) {
            send_unvalidated_rejection(
                &outward,
                InvocationRejectionReason::Protocol,
                error,
                request_idempotency_key(&first),
                request_agent_id(&first),
                None,
            )
            .await;
            return;
        }
        {
            let span = tracing::Span::current();
            if let Some(agent_id) = proto_agent_id_string(&request_agent_id(&first)) {
                span.record("agent_id", agent_id.as_str());
            }
            if let Some(key) = request_idempotency_key(&first) {
                span.record(
                    "idempotency_key",
                    uuid::Uuid::from(key).to_string().as_str(),
                );
            }
        }
        let first = first.request.expect("validated request has a payload");
        let start = match first {
            invocation_request::Request::Start(start) => start,
            invocation_request::Request::ResumeAttach(resume) => {
                self.run_resumed_agent_session(
                    resume,
                    inbound,
                    outward,
                    state,
                    response_lease,
                    scope,
                )
                .await;
                return;
            }
            _ => unreachable!("the session validator requires start or resume-attach first"),
        };

        let state = Arc::new(tokio::sync::Mutex::new(state));
        let (responses, mut response_rx) = mpsc::channel(32);
        let response_state = state.clone();
        let response_state_changed = Arc::new(tokio::sync::Notify::new());
        let forwarder_state_changed = response_state_changed.clone();
        let outward_forwarder = outward.clone();
        let forwarder_stopped = tokio_util::sync::CancellationToken::new();
        let forwarder_guard = forwarder_stopped.clone().drop_guard();
        tokio::spawn(async move {
            let _guard = forwarder_guard;
            while let Some(response) = response_rx.recv().await {
                if let Err(error) = response_state.lock().await.validate_response(&response) {
                    tracing::error!(error, ?response, "Invalid invocation session response");
                    return;
                }
                forwarder_state_changed.notify_waiters();
                if outward_forwarder.send(response).await.is_err() {
                    return;
                }
            }
        });

        let (acceptance_committed_tx, mut acceptance_committed_rx) =
            tokio::sync::oneshot::channel();
        let (accepted_tx, mut accepted_rx) = tokio::sync::oneshot::channel();
        // Keep dispatch state off the transport's stack across its nested select loops.
        let mut invocation = Box::pin(self.invoke_agent_internal(
            &start,
            acceptance_committed_tx,
            accepted_tx,
            scope,
        ));
        let acceptance = race_invocation_acceptance(
            &mut accepted_rx,
            &mut acceptance_committed_rx,
            invocation.as_mut(),
            async {
                tokio::select! {
                    request = inbound.message() => request,
                    _ = outward.closed() => Ok(None),
                    _ = forwarder_stopped.cancelled() => Ok(None),
                }
            },
        )
        .await;
        let (mut accepted, mut observation, mut early_inbound) = match acceptance {
            AcceptanceRace::Accepted {
                acceptance,
                early_output,
                early_inbound,
            } => (
                acceptance,
                InvocationObservation::accepted(invocation, early_output),
                early_inbound,
            ),
            AcceptanceRace::InvocationFinished(result) => {
                let (reason, error, worker_error) = match result {
                    Ok(_) => {
                        tracing::warn!(
                            target = ?start.target,
                            attempt_id = ?start.attempt_id,
                            error = "invocation completed before acceptance",
                            "Invocation session start was rejected"
                        );
                        (
                            InvocationRejectionReason::Internal,
                            "invocation completed before acceptance".to_string(),
                            None,
                        )
                    }
                    Err(error) => {
                        tracing::warn!(
                            target = ?start.target,
                            attempt_id = ?start.attempt_id,
                            error = %error,
                            "Invocation session start was rejected"
                        );
                        (
                            pre_acceptance_rejection_reason(&error),
                            error.to_string(),
                            Some(error.into()),
                        )
                    }
                };
                send_rejection_with_worker_error(&responses, reason, error, worker_error, &start)
                    .await;
                return;
            }
            AcceptanceRace::InboundBeforeAcceptance(request) => {
                let error = match request {
                    Ok(Some(request)) => state
                        .lock()
                        .await
                        .validate_trusted_request(&request)
                        .err()
                        .unwrap_or_else(|| {
                            "scalar invocation received an unexpected stream-control request"
                                .to_string()
                        }),
                    Ok(None) => "invocation request transport closed before acceptance".to_string(),
                    Err(error) => error.to_string(),
                };
                send_rejection(
                    &responses,
                    InvocationRejectionReason::Protocol,
                    error,
                    &start,
                )
                .await;
                return;
            }
        };

        let queued = accepted.queued.take();
        let was_queued = queued.is_some();
        if let Some((attached, mut readiness)) = queued {
            *response_lease.lock().unwrap() = accepted
                .durable_streams
                .as_ref()
                .and_then(StreamSession::response_lease);
            let pending = InvocationAccepted {
                agent_id: Some(accepted.agent_id.into()),
                idempotency_key: Some(accepted.idempotency_key.into()),
                attachment_id: (!accepted.joined_origin_observer)
                    .then_some(attached.attachment_id.0.into()),
                attempt_id: (!accepted.joined_origin_observer)
                    .then_some(attached.attempt_id.0.into()),
                epoch: if accepted.joined_origin_observer {
                    0
                } else {
                    attached.epoch
                },
                joined_origin_observer: accepted.joined_origin_observer,
                environment_id: accepted
                    .durable_streams
                    .as_ref()
                    .map(|streams| streams.session_key().callee_environment_id.into()),
                callee_fingerprint: accepted
                    .durable_streams
                    .as_ref()
                    .map(|streams| streams.session_key().callee_fingerprint.0.into()),
                method_name: start.method_name.clone(),
                ..Default::default()
            };
            if responses
                .send(InvocationResponse {
                    response: Some(invocation_response::Response::Accepted(pending)),
                })
                .await
                .is_err()
            {
                detach_durable_attachment(
                    accepted
                        .durable_streams
                        .take()
                        .filter(|_| !accepted.joined_origin_observer),
                )
                .await;
                return;
            }
            if let Err(error) = observation.check_observation() {
                send_worker_failure(&responses, error).await;
                return;
            }
            let mut inbound_open = true;
            let ready = loop {
                tokio::select! {
                    ready = &mut readiness => break ready,
                    () = observation.wait_for_completion() => {
                        if let Err(error) = observation.check_observation() {
                            send_worker_failure(&responses, error).await;
                            return;
                        }
                    },
                    _ = outward.closed() => {
                        detach_durable_attachment(accepted.durable_streams.take().filter(|_| !accepted.joined_origin_observer)).await;
                        return;
                    },
                    _ = forwarder_stopped.cancelled() => {
                        detach_durable_attachment(accepted.durable_streams.take().filter(|_| !accepted.joined_origin_observer)).await;
                        return;
                    },
                    request = inbound.message(), if inbound_open => {
                        if matches!(request, Ok(None)) && accepted.joined_origin_observer {
                            inbound_open = false;
                            continue;
                        }
                        if let Ok(Some(_)) = request {
                            send_protocol_failure(&responses, "stream controls require execution readiness").await;
                        }
                        detach_durable_attachment(accepted.durable_streams.take().filter(|_| !accepted.joined_origin_observer)).await;
                        return;
                    },
                }
            };
            match ready {
                Ok((prepared, streams)) => {
                    if let Some(queued) = &accepted.durable_streams
                        && !accepted.joined_origin_observer
                        && let Err(error) = queued.ensure_current_attachment().await
                    {
                        send_attachment_revocation(&responses, Err(error)).await;
                        return;
                    }
                    accepted.component_revision =
                        Some(prepared.attempt.invocation.target_component_revision);
                    accepted.prepared = Some(prepared);
                    accepted.durable_streams = Some(streams);
                }
                Err(error) => {
                    send_worker_failure(&responses, error).await;
                    return;
                }
            }
        }
        // Observation must never run attachment cleanup, even while the original is current.
        let mut durable_attachment = accepted
            .durable_streams
            .clone()
            .filter(|_| !accepted.joined_origin_observer);
        let native_tool = start.external_tool.is_some();
        *response_lease.lock().unwrap() = accepted
            .durable_streams
            .as_ref()
            .and_then(StreamSession::response_lease);
        until_response_closed(&outward, &forwarder_stopped, async {
        let high_waters = if let Some(durable_streams) = &accepted.durable_streams {
            match durable_streams.input_high_waters().await {
                Ok(high_waters) => high_waters,
                Err(error) => {
                    preserve_attachment_on_read_failure(&mut durable_attachment, &error);
                    tracing::warn!(
                        target = ?start.target,
                        attempt_id = ?start.attempt_id,
                        error = %error,
                        "Invocation session input high-water lookup failed"
                    );
                    if was_queued {
                        send_protocol_failure(&responses, error).await;
                    } else {
                        send_rejection_with_worker_error(
                            &responses,
                            InvocationRejectionReason::Internal,
                            error.to_string(),
                            rejection_worker_error(&error),
                            &start,
                        )
                        .await;
                    }
                    return;
                }
            }
        } else {
            HashMap::new()
        };

        let acceptance_forwarded = response_state_changed.notified();
        tokio::pin!(acceptance_forwarded);
        acceptance_forwarded.as_mut().enable();
        if responses
            .send(InvocationResponse {
                response: Some((if was_queued { invocation_response::Response::Ready
                } else { invocation_response::Response::Accepted })(
                    InvocationAccepted {
                        agent_id: Some(accepted.agent_id.into()),
                        idempotency_key: Some(accepted.idempotency_key.into()),
                        component_revision: accepted
                            .component_revision
                            .map(|revision| revision.get()),
                        attachment_id: accepted
                            .prepared
                            .as_ref()
                            .filter(|_| !accepted.joined_origin_observer)
                            .map(|prepared| prepared.attempt.attachment_id.0.into()),
                        attempt_id: accepted
                            .prepared
                            .as_ref()
                            .filter(|_| !accepted.joined_origin_observer)
                            .map(|prepared| prepared.attempt.attempt_id.0.into()),
                        epoch: accepted.durable_streams.as_ref()
                            .filter(|_| !accepted.joined_origin_observer)
                            .map(StreamSession::attachment_epoch).unwrap_or_default(),
                        stream_mappings: accepted
                            .durable_streams
                            .as_ref()
                            .map(|streams| {
                                streams
                                    .materialized_mappings()
                                    .iter()
                                    .map(|mapping| {
                                        durable_stream_mapping_to_proto(
                                            mapping,
                                            high_waters.get(&mapping.transport_stream_id),
                                        )
                                    })
                                    .collect()
                            })
                            .unwrap_or_default(),
                        environment_id: accepted.prepared.as_ref().map(|prepared| {
                            prepared.attempt.session_key.callee_environment_id.into()
                        }),
                        callee_fingerprint: accepted
                            .prepared
                            .as_ref()
                            .map(|prepared| prepared.attempt.expected_callee_fingerprint.0.into()),
                        method_name: start.method_name.clone(),
                        joined_origin_observer: accepted.joined_origin_observer,
                        tool_name: start.external_tool.as_ref().map(|tool| tool.tool_name.clone()),
                        command_path: start.external_tool.as_ref().map(|tool| tool.command_path.clone()).unwrap_or_default(),
                        terminal_cursor_stream_ids: Vec::new(),
                        captured_method_input: start.captured_method_input.clone(),
                    },
                )),
            })
            .await
            .is_err()
        {
            return;
        }
        tokio::select! {
            _ = acceptance_forwarded => {}
            _ = responses.closed() => return,
        }
        if let Err(error) = observation.check_observation() {
            durable_attachment = None;
            send_worker_failure(&responses, error).await;
            return;
        }
        if accepted.joined_origin_observer {
            let streams = accepted.durable_streams.as_ref().expect("durable observer");
            // No controls, pumps, finalization, or detach are permitted on this path.
            let observe = async {
                let result = loop {
                    if let Some(result) = streams.persisted_result().await.map_err(|error| error.into_worker_executor_error(WorkerExecutorError::runtime))? {
                        break result;
                    }
                    if let Some(outcome) = streams.persisted_finished().await.map_err(|error| error.into_worker_executor_error(WorkerExecutorError::runtime))? {
                        // A result can be committed between the two reads. Its handles must
                        // precede the terminal even when the invocation subsequently failed.
                        if let Some(result) = streams.persisted_result().await.map_err(|error| error.into_worker_executor_error(WorkerExecutorError::runtime))? {
                            break result;
                        }
                        outcome.map_err(|details| WorkerExecutorError::runtime(String::from_utf8_lossy(&details).into_owned()))?;
                        return Err(WorkerExecutorError::runtime("session finished successfully without a persisted result"));
                    }
                    if !observation.is_in_flight() {
                        if let Some(result) = streams.persisted_result().await.map_err(|error| error.into_worker_executor_error(WorkerExecutorError::runtime))? {
                            break result;
                        }
                        if let InvocationObservation::InvocationFailed(error) = &observation {
                            return Err((**error).clone());
                        }
                        return Err(WorkerExecutorError::runtime("invocation finished without a persisted result"));
                    }
                    tokio::select! {
                        result = streams.wait_persisted_result() => { result.map_err(|error| error.into_worker_executor_error(WorkerExecutorError::runtime))?; },
                        outcome = streams.wait_persisted_finished() => { outcome.map_err(|error| error.into_worker_executor_error(WorkerExecutorError::runtime))?.ok(); },
                        () = observation.wait_for_completion() => observation.check_observation()?,
                    }
                };
                let new_stream_mappings = result.proto_mappings();
                if responses.send(InvocationResponse {
                    response: Some(invocation_response::Response::Result(InvocationSessionResult {
                        result: Some(invocation_session_result::Result::MethodResult(result.value)),
                        component_revision: accepted.component_revision.map(|revision| revision.get()),
                        agent_id: Some(accepted.agent_id.into()),
                        idempotency_key: Some(accepted.idempotency_key.into()),
                        agent_fingerprint: start.expected_callee_fingerprint,
                        new_stream_mappings,
                        ..Default::default()
                    })),
                }).await.is_err() {
                    return Ok::<_, WorkerExecutorError>(());
                }
                let finished = streams.wait_persisted_finished();
                tokio::pin!(finished);
                let outcome = tokio::select! {
                    biased;
                    outcome = &mut finished => outcome,
                    () = observation.wait_for_completion() => {
                        observation.check_observation()?;
                        finished.await
                    },
                }.map_err(|error| error.into_worker_executor_error(WorkerExecutorError::runtime))?;
                match outcome {
                    Ok(()) => {
                        let _ = responses.send(InvocationResponse {
                            response: Some(invocation_response::Response::Finished(InvocationSessionCompletion {
                                outcome: Some(invocation_session_completion::Outcome::Success(golem_schema::proto::golem::common::Empty {})),
                            })),
                        }).await;
                    }
                    Err(details) => send_worker_failure(&responses,
                        WorkerExecutorError::runtime(String::from_utf8_lossy(&details).into_owned())).await,
                }
                Ok(())
            };
            let reject_controls = async {
                let next = match early_inbound.take() {
                    Some(request) => request,
                    None => inbound.message().await,
                };
                match next {
                    Ok(None) => std::future::pending::<()>().await,
                    Ok(Some(_)) | Err(_) => {
                        send_protocol_failure(&responses, "an invocation observer cannot send stream controls".to_string()).await;
                    }
                }
            };
            tokio::select! {
                result = observe => if let Err(error) = result {
                    send_worker_failure(&responses, error).await;
                },
                () = reject_controls => {},
            }
            return;
        }
        if accepted.durable_replayed
            && let Some(streams) = &accepted.durable_streams
            && let Err(error) = streams.ensure_current_attachment().await
        {
            // An unsuccessful authority check is not a transport detach.
            durable_attachment = None;
            send_attachment_revocation(&responses, Err(error)).await;
            return;
        }

        if let Some(durable_streams) = &accepted.durable_streams {
            let attachment_revoked = durable_streams.wait_for_attachment_revocation();
            tokio::pin!(attachment_revoked);
            if let Some(request) = early_inbound.take() {
                match request {
                    Ok(Some(request)) => {
                        if let Err(details) =
                            route_durable_request(durable_streams, &responses, &state, request)
                                .await
                        {
                            end_attachment_on_failure(durable_streams, &responses, &high_waters, &mut durable_attachment, details, async {
                                let _ = durable_streams.pump_output_streams(&responses).await;
                            }).await;
                            return;
                        }
                    }
                    Ok(None) | Err(_) => {
                        return;
                    }
                }
            }
            let early_output_ids = accepted.prepared.as_ref().map(|prepared| prepared.stream_mappings.iter()
                .filter(|mapping| mapping.role.direction() == SessionStreamRole::Output)
                .map(|mapping| mapping.transport_stream_id).collect::<Vec<_>>()).unwrap_or_default();
            let mut output_pump = durable_streams.producer.tasks().children();
            let pumped_outputs = Arc::new(std::sync::Mutex::new(std::collections::HashSet::new()));
            if !early_output_ids.is_empty() {
                let streams = durable_streams.clone();
                let responses = responses.clone();
                let ids = early_output_ids.clone();
                let pumped_outputs = pumped_outputs.clone();
                output_pump.spawn(async move {
                    streams.pump_output_streams_once(&HashMap::new(), &ids, &ids, &pumped_outputs, &responses).await
                });
            }
            let persisted_result = match persisted_invocation_result(durable_streams, native_tool).await {
                Ok(Some(result)) => Some(result),
                Ok(None) if !observation.is_in_flight() => None,
                Ok(None) => {
                    let mut result = Box::pin(async {
                        loop {
                            let changed = durable_streams.producer.session_records_changed().notified();
                            tokio::pin!(changed);
                            changed.as_mut().enable();
                            let persisted = persisted_invocation_result(durable_streams, native_tool).await?;
                            if let Some(result) = persisted { break Ok::<_, SessionError>(result); }
                            changed.await;
                        }
                    });
                    loop {
                        tokio::select! {
                            biased;
                            pumped = output_pump.join_next(), if !output_pump.is_empty() => {
                                let result = pumped.expect("output pump is not empty")
                                    .map_err(|error| SessionError::from(format!("durable output pump task failed: {error}")))
                                    .and_then(|result| result);
                                if let Err(error) = result {
                                    preserve_attachment_on_read_failure(&mut durable_attachment, &error);
                                    if !ends_only_the_attachment(&error) {
                                        send_protocol_failure(&responses, error).await;
                                    }
                                    return;
                                }
                            },
                            result = &mut result => match result {
                                Ok(result) => break Some(result),
                                Err(error) => {
                                    preserve_attachment_on_read_failure(&mut durable_attachment, &error);
                                    send_protocol_failure(&responses, error).await;
                                    return;
                                }
                            },
                            () = observation.wait_for_completion() => {
                                if let Err(error) = observation.check_observation() {
                                    durable_attachment = None;
                                    send_worker_failure(&responses, error).await;
                                    return;
                                }
                                drop(result);
                                match persisted_invocation_result(durable_streams, native_tool).await {
                                    Ok(result) => break result,
                                    Err(error) => {
                                        preserve_attachment_on_read_failure(&mut durable_attachment, &error);
                                        send_protocol_failure(&responses, error).await;
                                        return;
                                    }
                                }
                            },
                            request = inbound.message() => match request {
                                Ok(Some(request)) => {
                                    if let Err(details) = route_durable_request(
                                        durable_streams,
                                        &responses,
                                        &state,
                                        request,
                                    ).await {
                                        end_attachment_on_failure(durable_streams, &responses, &high_waters, &mut durable_attachment, details, async {
                                            while output_pump.join_next().await.is_some() {}
                                        }).await;
                                        return;
                                    }
                                }
                                Ok(None) | Err(_) => {
                                    return;
                                }
                            },
                            revocation = &mut attachment_revoked => {
                                durable_attachment = None;
                                send_attachment_revocation(&responses, revocation).await;
                                return;
                            },
                        }
                    }
                }
                Err(error) => {
                    preserve_attachment_on_read_failure(&mut durable_attachment, &error);
                    send_protocol_failure(&responses, error).await;
                    return;
                }
            };
            if let InvocationObservation::InvocationFailed(error) = &observation {
                let details = error.to_string();
                if let Err(finish_error) = durable_streams.fail_invocation(details).await {
                    preserve_attachment_on_read_failure(&mut durable_attachment, &finish_error);
                    send_protocol_failure(&responses, finish_error).await;
                    return;
                }
                if persisted_result.is_none() {
                    if let Err(error @ SessionError::Unreadable(_)) = durable_streams
                        .pump_input_cancellations(&responses, &high_waters).await
                    {
                        preserve_attachment_on_read_failure(&mut durable_attachment, &error);
                        send_protocol_failure(&responses, error).await;
                        return;
                    }
                    while output_pump.join_next().await.is_some() {}
                    send_worker_failure(&responses, (**error).clone()).await;
                    return;
                }
            }
            let Some(persisted_result) = persisted_result else {
                send_protocol_failure(
                    &responses,
                    "durable invocation completed without a persisted session result".to_string(),
                )
                .await;
                return;
            };
            if responses
                .send(InvocationResponse {
                    response: Some(invocation_response::Response::Result(
                        InvocationSessionResult {
                            result: Some(persisted_result.0),
                            component_revision: accepted
                                .component_revision
                                .map(|revision| revision.get()),
                            agent_id: Some(accepted.agent_id.into()),
                            idempotency_key: Some(accepted.idempotency_key.into()),
                            fuel_consumed: None,
                            status: None,
                            oplog_index: None,
                            agent_fingerprint: accepted.prepared.as_ref().map(|prepared| {
                                prepared.attempt.expected_callee_fingerprint.0.into()
                            }),
                            new_stream_mappings: persisted_result.1.clone(),
                            background_failure: None,
                        },
                    )),
                })
                .await
                .is_err()
            {
                return;
            }

            let output_streams = durable_streams.clone();
            let output_responses = responses.clone();
            let result_mapping_ids = persisted_result.1.iter().map(|mapping| mapping.transport_stream_id)
                .collect::<Vec<_>>();
            output_pump.spawn(async move {
                output_streams.pump_output_streams_once(&HashMap::new(), &result_mapping_ids, &result_mapping_ids, &pumped_outputs, &output_responses).await
            });
            while observation.is_in_flight() || !output_pump.is_empty() {
                tokio::select! {
                    () = observation.wait_for_completion() => {
                        if let Err(error) = observation.check_observation() {
                            durable_attachment = None;
                            send_worker_failure(&responses, error).await;
                            return;
                        }
                        if let InvocationObservation::InvocationFailed(error) = observation {
                            let details = error.to_string();
                            if let Err(finish_error) = durable_streams
                                .fail_invocation(details)
                                .await
                            {
                                preserve_attachment_on_read_failure(&mut durable_attachment, &finish_error);
                                send_protocol_failure(&responses, finish_error).await;
                                return;
                            }
                            if let Err(error @ SessionError::Unreadable(_)) = durable_streams
                                .pump_input_cancellations(&responses, &high_waters)
                                .await
                            {
                                preserve_attachment_on_read_failure(&mut durable_attachment, &error);
                                send_protocol_failure(&responses, error).await;
                                return;
                            }
                            while output_pump.join_next().await.is_some() {}
                            send_worker_failure(&responses, *error).await;
                            return;
                        }
                    },
                    result = output_pump.join_next(), if !output_pump.is_empty() => {
                        let result = match result {
                            Some(Ok(result)) => result,
                            Some(Err(error)) => Err(format!(
                                "durable output pump task failed: {error}"
                            ).into()),
                            None => Err("durable output pump task stopped unexpectedly".to_string().into()),
                        };
                        if let Err(error) = result {
                            end_attachment_on_failure(durable_streams, &responses, &high_waters, &mut durable_attachment, error, async {
                                while output_pump.join_next().await.is_some() {}
                            }).await;
                            return;
                        }
                    },
                    request = inbound.message() => match request {
                        Ok(Some(request)) => {
                            if let Err(details) = route_durable_request(
                                durable_streams,
                                &responses,
                                &state,
                                request,
                            ).await {
                                end_attachment_on_failure(durable_streams, &responses, &high_waters, &mut durable_attachment, details, async {
                                    while output_pump.join_next().await.is_some() {}
                                }).await;
                                return;
                            }
                        }
                        Ok(None) | Err(_) => {
                            output_pump.shutdown().await;
                            return;
                        }
                    },
                    revocation = &mut attachment_revoked => {
                        durable_attachment = None;
                        output_pump.shutdown().await;
                        send_attachment_revocation(&responses, revocation).await;
                        return;
                    }
                }
            }
            if let InvocationObservation::InvocationFailed(error) = observation {
                if let Err(error @ SessionError::Unreadable(_)) = durable_streams
                    .pump_input_cancellations(&responses, &high_waters).await
                {
                    preserve_attachment_on_read_failure(&mut durable_attachment, &error);
                    send_protocol_failure(&responses, error).await;
                    return;
                }
                send_worker_failure(&responses, *error).await;
                return;
            }
            if let Err(error) = durable_streams.complete().await {
                preserve_attachment_on_read_failure(&mut durable_attachment, &error);
                send_protocol_failure(&responses, error).await;
                return;
            }
            if let Err(error) = durable_streams.pump_input_cancellations(&responses, &high_waters).await {
                preserve_attachment_on_read_failure(&mut durable_attachment, &error);
                send_protocol_failure(&responses, error).await;
                return;
            }
            loop {
                let changed = response_state_changed.notified();
                tokio::pin!(changed);
                changed.as_mut().enable();
                if state.lock().await.all_inputs_terminal() {
                    break;
                }
                tokio::select! {
                    () = &mut changed => {}
                    request = inbound.message() => match request {
                        Ok(Some(request)) => {
                            if let Err(details) = route_durable_request(
                                durable_streams,
                                &responses,
                                &state,
                                request,
                            ).await {
                                end_attachment_on_failure(durable_streams, &responses, &high_waters, &mut durable_attachment, details, async {}).await;
                                return;
                            }
                        }
                        Ok(None) | Err(_) => {
                            return;
                        }
                    },
                    revocation = &mut attachment_revoked => {
                        durable_attachment = None;
                        send_attachment_revocation(&responses, revocation).await;
                        return;
                    },
                }
            }
            let _ = responses
                .send(InvocationResponse {
                    response: Some(invocation_response::Response::Finished(
                        InvocationSessionCompletion {
                            outcome: Some(invocation_session_completion::Outcome::Success(
                                golem_schema::proto::golem::common::Empty {},
                            )),
                        },
                    )),
                })
                .await;
            return;
        }

        if let Some(request) = early_inbound.take() {
            match request {
                Ok(Some(request)) => {
                    let details = state
                        .lock()
                        .await
                        .validate_trusted_request(&request)
                        .err()
                        .unwrap_or_else(|| {
                            "scalar invocation received an unexpected stream-control request"
                                .to_string()
                        });
                    send_protocol_failure(&responses, details).await;
                    return;
                }
                Ok(None) | Err(_) => return,
            }
        }

        if observation.is_in_flight() {
            tokio::select! {
                () = observation.wait_for_completion() => {},
                request = inbound.message() => {
                    if let Ok(Some(request)) = request {
                        let details = state
                            .lock()
                            .await
                            .validate_trusted_request(&request)
                            .err()
                            .unwrap_or_else(|| {
                                "scalar invocation received an unexpected stream-control request"
                                    .to_string()
                            });
                        send_protocol_failure(&responses, details).await;
                    }
                    return;
                }
            }
        }

        let output = match observation {
            InvocationObservation::Succeeded(output) => *output,
            InvocationObservation::InvocationFailed(error)
            | InvocationObservation::ObservationFailed(error) => {
                send_worker_failure(&responses, *error).await;
                return;
            }
            InvocationObservation::InFlight(_) => unreachable!("scalar invocation was not observed"),
        };
        if accepted.component_revision.is_none()
            && let Some(revision) = output.component_revision
            && responses.send(InvocationResponse {
                response: Some(invocation_response::Response::Ready(InvocationAccepted {
                    agent_id: Some(accepted.agent_id.into()),
                    idempotency_key: Some(accepted.idempotency_key.into()),
                    component_revision: Some(revision.get()),
                    method_name: start.method_name.clone(),
                    ..Default::default()
                })),
            }).await.is_err()
        {
            return;
        }
        let (result, new_stream_mappings) = match output.result {
            AgentInvocationResult::ExternalTool { result } => {
                let result = match result {
                    Ok(result) => golem_common::model::oplog::PublicExternalToolResult::Success(result),
                    Err(error) => golem_common::model::oplog::PublicExternalToolResult::Failure(error),
                };
                match result.try_into() {
                    Ok(result) => (Some(invocation_session_result::Result::ToolResult(result)), Vec::new()),
                    Err(error) => {
                        send_protocol_failure(&responses, error).await;
                        return;
                    }
                }
            }
            AgentInvocationResult::AgentMethod { output } => match output.try_into() {
                Ok(output) => (
                    Some(invocation_session_result::Result::MethodResult(output)),
                    Vec::new(),
                ),
                Err(error) => {
                    send_protocol_failure(&responses, error).await;
                    return;
                }
            },
            _ => (
                Some(invocation_session_result::Result::NoResult(
                    golem_schema::proto::golem::common::Empty {},
                )),
                Vec::new(),
            ),
        };
        if responses
            .send(InvocationResponse {
                response: Some(invocation_response::Response::Result(
                    InvocationSessionResult {
                        result,
                        component_revision: output
                            .component_revision
                            .map(|revision| revision.get()),
                        agent_id: output.agent_id.map(Into::into),
                        idempotency_key: output.idempotency_key.map(Into::into),
                        fuel_consumed: output.consumed_fuel,
                        status: output.invocation_status.map(|status| {
                            golem_api_grpc::proto::golem::worker::InvocationStatus::from(status)
                                as i32
                        }),
                        oplog_index: output.oplog_index.map(u64::from),
                        agent_fingerprint: output
                            .agent_fingerprint
                            .map(|fingerprint| fingerprint.0.into()),
                        new_stream_mappings,
                        background_failure: output.background_failure.clone().map(|failure| {
                            WorkerExecutorError::from(failure).into()
                        }),
                    },
                )),
            })
            .await
            .is_err()
        {
            return;
        }
        let _ = responses
            .send(InvocationResponse {
                response: Some(invocation_response::Response::Finished(
                    InvocationSessionCompletion {
                        outcome: Some(invocation_session_completion::Outcome::Success(
                            golem_schema::proto::golem::common::Empty {},
                        )),
                    },
                )),
            })
            .await;
        })
        .await;
        detach_durable_attachment(durable_attachment).await;
    }
}
