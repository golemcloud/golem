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
    AcceptanceRace, AgentInvocationResult, Arc, HasAll, HashMap, InvocationAccepted,
    InvocationRejectionReason, InvocationRequest, InvocationResponse, InvocationSessionCompletion,
    InvocationSessionResult, InvocationSessionState, Message, ResponseLease, SessionError,
    SessionStreamRole, StreamSession, UsesAllDeps, WorkerCtx, WorkerExecutorError,
    WorkerExecutorImpl, decode_invocation_input, detach_durable_attachment,
    durable_stream_mapping_to_proto, end_attachment_on_failure, ends_only_the_attachment,
    invocation_request, invocation_response, invocation_session_completion,
    invocation_session_result, mpsc, persisted_invocation_result, pre_acceptance_rejection_reason,
    proto_agent_id_string, race_invocation_acceptance, rejection_worker_error, request_agent_id,
    request_idempotency_key, route_durable_request, send_attachment_revocation,
    send_protocol_failure, send_rejection, send_rejection_with_worker_error,
    send_unvalidated_rejection, send_worker_failure, until_response_closed,
};

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
                span.record("idempotency_key", key.value.as_str());
            }
        }
        let first = first.request.expect("validated request has a payload");
        let mut start = match first {
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
        let forwarder = tokio::spawn(async move {
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

        let input_encoded_len = start.input.as_ref().map(Message::encoded_len);
        let input = if start.external_tool.is_some()
            || start.mode() == golem_api_grpc::proto::golem::worker::AgentInvocationMode::Lookup
        {
            Ok(None)
        } else {
            match start.input.take() {
                Some(input) => decode_invocation_input(input).map(Some),
                None => Err("invocation start has no input".to_string()),
            }
        };
        let input = match input {
            Ok(input) => input,
            Err(error) => {
                send_rejection(
                    &responses,
                    InvocationRejectionReason::Protocol,
                    error,
                    &start,
                )
                .await;
                drop(responses);
                let _ = forwarder.await;
                return;
            }
        };

        let (acceptance_committed_tx, mut acceptance_committed_rx) =
            tokio::sync::oneshot::channel();
        let (accepted_tx, mut accepted_rx) = tokio::sync::oneshot::channel();
        // Keep dispatch state off the transport's stack across its nested select loops.
        let mut invocation = Box::pin(self.invoke_agent_internal(
            &start,
            input,
            input_encoded_len,
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
        let (accepted, early_output, mut early_inbound) = match acceptance {
            AcceptanceRace::Accepted {
                acceptance,
                early_output,
                early_inbound,
            } => (acceptance, early_output, early_inbound),
            AcceptanceRace::InvocationFinished(result) => {
                let (reason, error, worker_error) = match result {
                    Ok(_) => {
                        tracing::warn!(
                            agent_id = ?start.agent_id,
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
                            agent_id = ?start.agent_id,
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

        // Observation must never run attachment cleanup, even while the original is current.
        let durable_attachment = accepted
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
                    tracing::warn!(
                        agent_id = ?start.agent_id,
                        attempt_id = ?start.attempt_id,
                        error = %error,
                        "Invocation session input high-water lookup failed"
                    );
                    send_rejection_with_worker_error(
                        &responses,
                        InvocationRejectionReason::Internal,
                        error.to_string(),
                        rejection_worker_error(&error),
                        &start,
                    )
                    .await;
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
                response: Some(invocation_response::Response::Accepted(
                    InvocationAccepted {
                        agent_id: start.agent_id.clone(),
                        idempotency_key: start.idempotency_key.clone(),
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
        if accepted.joined_origin_observer {
            let streams = accepted.durable_streams.as_ref().expect("durable observer");
            // No controls, pumps, finalization, or detach are permitted on this path.
            let observe = async {
                let mut completed = early_output;
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
                    if let Some(output) = completed.take() {
                        if let Some(result) = streams.persisted_result().await.map_err(|error| error.into_worker_executor_error(WorkerExecutorError::runtime))? {
                            break result;
                        }
                        output?;
                        return Err(WorkerExecutorError::runtime("invocation finished without a persisted result"));
                    }
                    tokio::select! {
                        result = streams.wait_persisted_result() => { result.map_err(|error| error.into_worker_executor_error(WorkerExecutorError::runtime))?; },
                        outcome = streams.wait_persisted_finished() => { outcome.map_err(|error| error.into_worker_executor_error(WorkerExecutorError::runtime))?.ok(); },
                        output = &mut invocation => completed = Some(output),
                    }
                };
                let new_stream_mappings = result.proto_mappings();
                if responses.send(InvocationResponse {
                    response: Some(invocation_response::Response::Result(InvocationSessionResult {
                        result: Some(invocation_session_result::Result::MethodResult(result.value)),
                        component_revision: accepted.component_revision.map(|revision| revision.get()),
                        agent_id: start.agent_id.clone(),
                        idempotency_key: start.idempotency_key.clone(),
                        agent_fingerprint: start.expected_callee_fingerprint,
                        new_stream_mappings,
                        ..Default::default()
                    })),
                }).await.is_err() {
                    return Ok::<_, WorkerExecutorError>(());
                }
                let outcome = streams.wait_persisted_finished().await
                    .map_err(|error| error.into_worker_executor_error(WorkerExecutorError::runtime))?;
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
            && streams.ensure_current_attachment().await.is_err()
        {
            let _ = responses
                .send(InvocationResponse {
                    response: Some(invocation_response::Response::AttachmentRevoked(
                        golem_api_grpc::proto::golem::worker::AttachmentRevoked {
                            details:
                                "the replayed start attachment has been fenced by a later epoch"
                                    .to_string(),
                        },
                    )),
                })
                .await;
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
                            end_attachment_on_failure(durable_streams, &responses, &high_waters, details, async {
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
            let mut completed_output = early_output;
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
                Ok(None) if completed_output.is_some() => None,
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
                                    if !ends_only_the_attachment(&error) {
                                        send_protocol_failure(&responses, error).await;
                                    }
                                    return;
                                }
                            },
                            result = &mut result => match result {
                                Ok(result) => break Some(result),
                                Err(error) => {
                                    send_protocol_failure(&responses, error).await;
                                    return;
                                }
                            },
                            output = &mut invocation => match output {
                                Ok(output) => {
                                    completed_output = Some(Ok(output));
                                    drop(result);
                                    match persisted_invocation_result(durable_streams, native_tool).await {
                                        Ok(result) => break result,
                                        Err(error) => {
                                            send_protocol_failure(&responses, error).await;
                                            return;
                                        }
                                    }
                                }
                                Err(error) => {
                                    completed_output = Some(Err(error));
                                    drop(result);
                                    match persisted_invocation_result(durable_streams, native_tool).await {
                                        Ok(result) => break result,
                                        Err(error) => {
                                            send_protocol_failure(&responses, error).await;
                                            return;
                                        }
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
                                        end_attachment_on_failure(durable_streams, &responses, &high_waters, details, async {
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
                                send_attachment_revocation(&responses, revocation).await;
                                return;
                            },
                        }
                    }
                }
                Err(error) => {
                    send_protocol_failure(&responses, error).await;
                    return;
                }
            };
            if matches!(completed_output, Some(Err(_))) {
                let Some(Err(error)) = completed_output.take() else {
                    unreachable!("completed output was checked to contain an error");
                };
                let details = error.to_string();
                if let Err(finish_error) = durable_streams.fail_invocation(details).await {
                    send_protocol_failure(&responses, finish_error).await;
                    return;
                }
                if persisted_result.is_none() {
                    let _ = durable_streams.pump_input_cancellations(&responses, &high_waters).await;
                    while output_pump.join_next().await.is_some() {}
                    send_worker_failure(&responses, error).await;
                    return;
                }
                completed_output = Some(Err(error));
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
                            agent_id: start.agent_id.clone(),
                            idempotency_key: start.idempotency_key.clone(),
                            fuel_consumed: None,
                            status: None,
                            oplog_index: None,
                            agent_fingerprint: accepted.prepared.as_ref().map(|prepared| {
                                prepared.attempt.expected_callee_fingerprint.0.into()
                            }),
                            new_stream_mappings: persisted_result.1.clone(),
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
            let mut output_pump_finished = false;
            while completed_output.is_none() || !output_pump_finished {
                tokio::select! {
                    output = &mut invocation, if completed_output.is_none() => match output {
                        Ok(output) => {
                            completed_output = Some(Ok(output));
                        },
                        Err(error) => {
                            let details = error.to_string();
                            if let Err(finish_error) = durable_streams
                                .fail_invocation(details)
                                .await
                            {
                                send_protocol_failure(&responses, finish_error).await;
                                return;
                            }
                            let _ = durable_streams
                                .pump_input_cancellations(&responses, &high_waters)
                                .await;
                            while output_pump.join_next().await.is_some() {}
                            send_worker_failure(&responses, error).await;
                            return;
                        }
                    },
                    result = output_pump.join_next(), if !output_pump_finished => {
                        let result = match result {
                            Some(Ok(result)) => result,
                            Some(Err(error)) => Err(format!(
                                "durable output pump task failed: {error}"
                            ).into()),
                            None => Err("durable output pump task stopped unexpectedly".to_string().into()),
                        };
                        match result {
                            Ok(()) => {
                                output_pump_finished = output_pump.is_empty();
                            },
                            Err(error) => {
                                end_attachment_on_failure(durable_streams, &responses, &high_waters, error, async {
                                    while output_pump.join_next().await.is_some() {}
                                }).await;
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
                                end_attachment_on_failure(durable_streams, &responses, &high_waters, details, async {
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
                        output_pump.shutdown().await;
                        send_attachment_revocation(&responses, revocation).await;
                        return;
                    }
                }
            }
            if let Some(Err(error)) = completed_output.take() {
                let _ = durable_streams.pump_input_cancellations(&responses, &high_waters).await;
                send_worker_failure(&responses, error).await;
                return;
            }
            if let Err(error) = durable_streams.complete().await {
                send_protocol_failure(&responses, error).await;
                return;
            }
            if let Err(error) = durable_streams.pump_input_cancellations(&responses, &high_waters).await {
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
                                end_attachment_on_failure(durable_streams, &responses, &high_waters, details, async {}).await;
                                return;
                            }
                        }
                        Ok(None) | Err(_) => {
                            return;
                        }
                    },
                    revocation = &mut attachment_revoked => {
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

        let output = match early_output {
            Some(output) => output,
            None => {
                tokio::select! {
                    result = &mut invocation => result,
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
        };

        let output = match output {
            Ok(output) => output,
            Err(error) => {
                send_worker_failure(&responses, error).await;
                return;
            }
        };
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
