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
    Arc, HasAll, HashMap, InvocationAccepted, InvocationRejected, InvocationRequest,
    InvocationResponse, InvocationSessionResult, InvocationSessionState, InvocationStart,
    PersistedInvocationTarget, ResponseLease, ResumeAttach, SessionError, SessionStreamRole,
    UsesAllDeps, WorkerCtx, WorkerExecutorError, WorkerExecutorImpl, build_resume_attempt,
    detach_durable_attachment, durable_stream_mapping_to_proto, ends_only_the_attachment,
    invocation_response, is_attachment_termination, mpsc, persisted_invocation_result,
    pre_acceptance_rejection_reason, resumed_input_schema, route_durable_request,
    send_attachment_revocation, send_protocol_failure, send_resumed_finished,
    until_response_closed,
};

impl<Ctx: WorkerCtx, Svcs: HasAll<Ctx> + UsesAllDeps<Ctx = Ctx> + Send + Sync + 'static>
    WorkerExecutorImpl<Ctx, Svcs>
{
    pub(super) async fn run_resumed_agent_session(
        &self,
        resume: ResumeAttach,
        mut inbound: tonic::Streaming<InvocationRequest>,
        outward: mpsc::Sender<InvocationResponse>,
        mut protocol_state: InvocationSessionState,
        response_lease: ResponseLease,
        scope: &crate::worker::tasks::TaskScope,
    ) {
        let rejection_identity = (resume.idempotency_key.clone(), resume.agent_id.clone());
        let result = async {
            Self::validate_auth_ctx(&resume.auth_ctx)?;
            let attempt = build_resume_attempt(
                &resume,
                self.services
                    .config()
                    .limits
                    .live_stream_event_broadcast_capacity
                    .get(),
            )?;
            let lookup = InvocationStart {
                agent_id: resume.agent_id.clone(),
                idempotency_key: resume.idempotency_key.clone(),
                auth_ctx: resume.auth_ctx.clone(),
                principal: resume.principal.clone(),
                environment_id: resume.environment_id,
                mode: golem_api_grpc::proto::golem::worker::AgentInvocationMode::Lookup as i32,
                ..Default::default()
            };
            let (worker, _response_lease) = self
                .get_or_create_pending_for_lookup(&lookup)
                .await?
                .ok_or_else(|| {
                WorkerExecutorError::invalid_request(
                    "NotFound: durable Stream Session worker was not found",
                )
            })?;
            scope
                .bind(&worker.tasks)
                .map_err(WorkerExecutorError::runtime)?;
            worker.resume_durable_streaming_invocation(attempt).await
        }
        .await;
        let mut acceptance = match result {
            Ok(acceptance) => acceptance,
            Err(error) => {
                tracing::warn!(
                    agent_id = ?resume.agent_id,
                    attempt_id = ?resume.attempt_id,
                    error = %error,
                    "Durable invocation session resume was rejected"
                );
                let rejection = InvocationResponse {
                    response: Some(invocation_response::Response::Rejected(
                        InvocationRejected {
                            reason: pre_acceptance_rejection_reason(&error) as i32,
                            error: error.to_string(),
                            idempotency_key: rejection_identity.0,
                            agent_id: rejection_identity.1,
                            component_revision: None,
                            worker_error: Some(error.into()),
                        },
                    )),
                };
                if protocol_state.validate_response(&rejection).is_ok() {
                    let _ = outward.send(rejection).await;
                }
                return;
            }
        };
        let durable_attachment = acceptance.streams.clone();
        *response_lease.lock().unwrap() = durable_attachment.response_lease();
        let forwarder_stopped = tokio_util::sync::CancellationToken::new();
        until_response_closed(&outward, &forwarder_stopped, async {
            let result = async {
                let component_revision = acceptance
                    .prepared
                    .attempt
                    .invocation
                    .target_component_revision;
                let (input_schema, input_element_types) =
                    resumed_input_schema(&acceptance.prepared)?;
                acceptance.streams = acceptance.streams.clone().with_input_schema(
                    Arc::new(input_schema),
                    component_revision,
                    input_element_types,
                );
                let cursor_map = resume
                    .cursors
                    .iter()
                    .filter_map(|cursor| {
                        let stream_id = cursor.stream_id.map(|stream_id| {
                            golem_common::model::durable_stream::StreamId(stream_id.into())
                        })?;
                        let offset = cursor
                            .last_observed_offset
                            .as_ref()
                            .and_then(|offset| offset.as_slice().try_into().ok())
                            .and_then(|offset| {
                                golem_common::model::durable_stream::StreamOffset::from_bytes(
                                    offset,
                                )
                                .ok()
                            });
                        Some((stream_id, offset))
                    })
                    .collect::<HashMap<_, _>>();
                let terminal_cursor_stream_ids = acceptance
                    .streams
                    .terminal_output_cursor_stream_ids(&cursor_map)
                    .await
                    .map_err(|error| error.into_worker_executor_error(WorkerExecutorError::invalid_request))?;
                let mut terminal_cursor_stream_ids = terminal_cursor_stream_ids.into_iter().collect::<Vec<_>>();
                terminal_cursor_stream_ids.sort();
                Ok::<_, WorkerExecutorError>((acceptance, cursor_map, terminal_cursor_stream_ids))
            }
            .await;
        let (acceptance, cursor_map, terminal_cursor_stream_ids) = match result {
            Ok(result) => result,
            Err(error) => {
                tracing::warn!(
                    agent_id = ?resume.agent_id,
                    attempt_id = ?resume.attempt_id,
                    error = %error,
                    "Durable invocation session resume preparation was rejected"
                );
                let rejection = InvocationResponse {
                    response: Some(invocation_response::Response::Rejected(
                        InvocationRejected {
                            reason: pre_acceptance_rejection_reason(&error) as i32,
                            error: error.to_string(),
                            idempotency_key: rejection_identity.0,
                            agent_id: rejection_identity.1,
                            component_revision: None,
                            worker_error: Some(error.into()),
                        },
                    )),
                };
                if protocol_state.validate_response(&rejection).is_ok() {
                    let _ = outward.send(rejection).await;
                }
                return;
            }
        };

        let state = Arc::new(tokio::sync::Mutex::new(protocol_state));
        let (responses, mut response_rx) = mpsc::channel(32);
        let response_state = state.clone();
        let outward_forwarder = outward.clone();
        let forwarder_guard = forwarder_stopped.clone().drop_guard();
        let forwarder = tokio::spawn(async move {
            let _guard = forwarder_guard;
            while let Some(response) = response_rx.recv().await {
                if let Err(error) = response_state.lock().await.validate_response(&response) {
                    tracing::error!(
                        error,
                        ?response,
                        "Invalid resumed invocation session response"
                    );
                    return;
                }
                if outward_forwarder.send(response).await.is_err() {
                    return;
                }
            }
        });
        let streams = acceptance.streams;
        let known_output_mapping_ids = acceptance
            .mappings
            .iter()
            .filter(|mapping| mapping.role.direction() == SessionStreamRole::Output)
            .map(|mapping| mapping.transport_stream_id)
            .collect::<Vec<_>>();
        let high_waters = match streams.input_high_waters().await {
            Ok(high_waters) => high_waters,
            Err(error) => {
                send_protocol_failure(&responses, error).await;
                drop(responses);
                let _ = forwarder.await;
                return;
            }
        };
        if responses
            .send(InvocationResponse {
                response: Some(invocation_response::Response::Accepted(
                    InvocationAccepted {
                        agent_id: resume.agent_id.clone(),
                        idempotency_key: resume.idempotency_key.clone(),
                        component_revision: Some(
                            acceptance
                                .prepared
                                .attempt
                                .invocation
                                .target_component_revision
                                .get(),
                        ),
                        attachment_id: Some(acceptance.prepared.attempt.attachment_id.0.into()),
                        attempt_id: resume.attempt_id,
                        epoch: acceptance.epoch,
                        stream_mappings: acceptance
                            .mappings
                            .iter()
                            .map(|mapping| {
                                durable_stream_mapping_to_proto(
                                    mapping,
                                    high_waters.get(&mapping.transport_stream_id),
                                )
                            })
                            .collect(),
                        environment_id: Some(
                            acceptance
                                .prepared
                                .attempt
                                .session_key
                                .callee_environment_id
                                .into(),
                        ),
                        callee_fingerprint: Some(
                            acceptance
                                .prepared
                                .attempt
                                .expected_callee_fingerprint
                                .0
                                .into(),
                        ),
                        joined_origin_observer: false,
                        method_name: match &acceptance.prepared.attempt.invocation.target {
                            PersistedInvocationTarget::AgentMethod { method_name } => Some(method_name.clone()),
                            PersistedInvocationTarget::ExternalTool { .. } => None,
                        },
                        tool_name: match &acceptance.prepared.attempt.invocation.target {
                            PersistedInvocationTarget::ExternalTool { tool_name, .. } => Some(tool_name.clone()),
                            PersistedInvocationTarget::AgentMethod { .. } => None,
                        },
                        command_path: match &acceptance.prepared.attempt.invocation.target {
                            PersistedInvocationTarget::ExternalTool { command_path, .. } => command_path.clone(),
                            PersistedInvocationTarget::AgentMethod { .. } => Vec::new(),
                        },
                        terminal_cursor_stream_ids: terminal_cursor_stream_ids.into_iter().map(|id| id.0.into()).collect(),
                    },
                )),
            })
            .await
            .is_err()
        {
            return;
        }
        if acceptance.replayed && streams.ensure_current_attachment().await.is_err() {
            let _ = responses
                .send(InvocationResponse {
                    response: Some(invocation_response::Response::AttachmentRevoked(
                        golem_api_grpc::proto::golem::worker::AttachmentRevoked {
                            details:
                                "the replayed attachment attempt has been fenced by a later epoch"
                                    .to_string(),
                        },
                    )),
                })
                .await;
            drop(responses);
            let _ = forwarder.await;
            return;
        }
        let attachment_revoked = streams.wait_for_attachment_revocation();
        tokio::pin!(attachment_revoked);
        let native_tool = matches!(
            acceptance.prepared.attempt.invocation.target,
            PersistedInvocationTarget::ExternalTool { .. }
        );
        let early_output_ids = acceptance.prepared.stream_mappings.iter()
            .filter(|mapping| mapping.role.direction() == SessionStreamRole::Output)
            .map(|mapping| mapping.transport_stream_id).collect::<Vec<_>>();
        let mut output_pump = streams.producer.tasks().children();
        let pumped_outputs = Arc::new(std::sync::Mutex::new(std::collections::HashSet::new()));
        if !early_output_ids.is_empty() {
            let output_streams = streams.clone();
            let output_responses = responses.clone();
            let output_cursors = cursor_map.clone();
            let ids = early_output_ids.clone();
            let pumped_outputs = pumped_outputs.clone();
            output_pump.spawn(async move {
                output_streams.pump_output_streams_once(&output_cursors, &ids, &ids, &pumped_outputs, &output_responses).await
            });
        }

        let (persisted_result, already_finished) = loop {
            let changed = streams.producer.session_records_changed().notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            let persisted_result = match persisted_invocation_result(&streams, native_tool).await {
                Ok(result) => result,
                Err(error) => {
                    send_protocol_failure(&responses, error).await;
                    return;
                }
            };
            let already_finished = match streams.persisted_finished().await {
                Ok(finished) => finished,
                Err(error) => {
                    send_protocol_failure(&responses, error).await;
                    return;
                }
            };
            if persisted_result.is_some() || already_finished.is_some() {
                break (persisted_result, already_finished);
            }
            tokio::select! {
                () = &mut changed => {}
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
                request = inbound.message() => match request {
                    Ok(Some(request)) => {
                        if let Err(error) = route_durable_request(&streams, &responses, &state, request).await {
                            if !ends_only_the_attachment(&error) {
                                send_protocol_failure(&responses, error).await;
                            }
                            return;
                        }
                    }
                    Ok(None) | Err(_) => return,
                },
                revocation = &mut attachment_revoked => {
                    send_attachment_revocation(&responses, revocation).await;
                    return;
                },
            }
        };
        if let Some(result) = persisted_result
            && responses
                .send(InvocationResponse {
                    response: Some(invocation_response::Response::Result(
                        InvocationSessionResult {
                            new_stream_mappings: result.1.clone(),
                            result: Some(result.0),
                            component_revision: Some(
                                acceptance
                                    .prepared
                                    .attempt
                                    .invocation
                                    .target_component_revision
                                    .get(),
                            ),
                            agent_id: resume.agent_id.clone(),
                            idempotency_key: resume.idempotency_key.clone(),
                            fuel_consumed: None,
                            status: None,
                            oplog_index: None,
                            agent_fingerprint: resume.expected_callee_fingerprint,
                        },
                    )),
                })
                .await
                .is_err()
            {
                return;
            }

        let root_output_mapping_ids = match streams.session_root_output_mapping_ids().await {
            Ok(root_output_mapping_ids) => root_output_mapping_ids,
            Err(error) => {
                send_protocol_failure(&responses, error).await;
                return;
            }
        };
        {
            let output_streams = streams.clone();
            let output_responses = responses.clone();
            output_pump.spawn(async move {
                output_streams
                    .pump_output_streams_once(
                        &cursor_map,
                        &root_output_mapping_ids,
                        &known_output_mapping_ids,
                        &pumped_outputs,
                        &output_responses,
                    )
                    .await
            });
            loop {
                tokio::select! {
                    result = output_pump.join_next() => {
                        let result = match result {
                            Some(Ok(result)) => result,
                            Some(Err(error)) => Err(format!(
                                "durable output pump task failed: {error}"
                            ).into()),
                            None => Err("durable output pump task stopped unexpectedly".to_string().into()),
                        };
                        match result {
                            Err(error) if is_attachment_termination(&error) => {}
                            Err(error) => {
                                if !ends_only_the_attachment(&error) {
                                    send_protocol_failure(&responses, error).await;
                                }
                                return;
                            }
                            Ok(()) => {}
                        }
                        if output_pump.is_empty() { break; }
                    }
                    request = inbound.message() => match request {
                        Ok(Some(request)) => {
                            if let Err(error) = route_durable_request(&streams, &responses, &state, request).await {
                                if !ends_only_the_attachment(&error) {
                                    send_protocol_failure(&responses, error).await;
                                }
                                return;
                            }
                        }
                        Ok(None) | Err(_) => {
                            output_pump.shutdown().await;
                            return;
                        },
                    },
                    revocation = &mut attachment_revoked => {
                        output_pump.shutdown().await;
                        send_attachment_revocation(&responses, revocation).await;
                        return;
                    }
                }
            }
        }
        if let Some(result) = already_finished {
            send_resumed_finished(&responses, result).await;
            drop(responses);
            let _ = forwarder.await;
            return;
        }
        if let Err(error) = streams.complete().await {
            send_protocol_failure(&responses, error).await;
            drop(responses);
            let _ = forwarder.await;
            return;
        }
        if let Err(error) = streams.pump_input_cancellations(&responses, &high_waters).await {
            send_protocol_failure(&responses, error).await;
            drop(responses);
            let _ = forwarder.await;
            return;
        }
        let finished = streams.wait_persisted_finished();
        tokio::pin!(finished);
        let result = loop {
            tokio::select! {
                result = &mut finished => break result,
                request = inbound.message() => match request {
                    Ok(Some(request)) => {
                        if let Err(error) = route_durable_request(&streams, &responses, &state, request).await {
                            if !ends_only_the_attachment(&error) {
                                send_protocol_failure(&responses, error).await;
                            }
                            return;
                        }
                    }
                    Ok(None) | Err(_) => return,
                },
                revocation = &mut attachment_revoked => {
                    send_attachment_revocation(&responses, revocation).await;
                    return;
                },
            }
        };
        match result {
            Ok(result) => send_resumed_finished(&responses, result).await,
            Err(error) => send_protocol_failure(&responses, error).await,
        }
        drop(responses);
        let _ = forwarder.await;
        })
        .await;
        detach_durable_attachment(Some(durable_attachment)).await;
    }
}
