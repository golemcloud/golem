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

use super::{WorkerExecutorImpl, extract_owned_agent_id};
use crate::durable_host::stream_session::LiveValueSession;
use crate::grpc::invocation::{CanStartWorker, from_proto_invocation_context};
use crate::services::{HasAll, HasComponentService, HasSchedulerService, UsesAllDeps};
use crate::worker::Worker;
use crate::worker::invocation::validate_agent_method_invocation;
use crate::workerctx::WorkerCtx;
use chrono::{DateTime, Utc};
use futures::{Stream, StreamExt};
use golem_api_grpc::invocation_session_protocol::InvocationSessionState;
use golem_api_grpc::proto::golem;
use golem_api_grpc::proto::golem::worker::v1::WorkerExecutionError;
use golem_api_grpc::proto::golem::worker::{
    InvocationAccepted, InvocationFailure, InvocationFailureKind, InvocationRejected,
    InvocationRejectionReason, InvocationRequest, InvocationResponse, InvocationSessionCompletion,
    InvocationSessionResult, InvocationStart, invocation_request, invocation_response,
    invocation_session_completion, invocation_session_result,
};
use golem_common::model::account::AccountId;
use golem_common::model::agent::{
    AgentMode, InvocationFreshnessDisposition, ParsedAgentId, Principal,
};
use golem_common::model::component::ComponentRevision;
use golem_common::model::{
    AgentId, AgentInvocation, AgentInvocationOutput, AgentInvocationResult, IdempotencyKey,
    InvocationStatus, ScheduledAction,
};
use golem_common::schema::SchemaValue;
use golem_service_base::error::worker_executor::WorkerExecutorError;
use std::pin::Pin;
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Request, Response, Status};

pub(super) type InvocationSessionStream =
    Pin<Box<dyn Stream<Item = Result<InvocationResponse, Status>> + Send + 'static>>;

pub(super) async fn invoke_agent_session<
    Ctx: WorkerCtx,
    Svcs: HasAll<Ctx> + UsesAllDeps<Ctx = Ctx> + Send + Sync + 'static,
>(
    executor: &WorkerExecutorImpl<Ctx, Svcs>,
    request: Request<tonic::Streaming<InvocationRequest>>,
) -> Result<Response<InvocationSessionStream>, Status> {
    let inbound = request.into_inner();
    let (responses, receiver) = mpsc::channel(32);
    let executor = (*executor).clone();
    tokio::spawn(async move {
        executor.run_agent_session(inbound, responses).await;
    });
    Ok(Response::new(Box::pin(
        ReceiverStream::new(receiver).map(Ok),
    )))
}

fn decode_invocation_freshness_disposition(value: i32) -> InvocationFreshnessDisposition {
    if value == golem::worker::InvocationFreshnessDisposition::KnownFresh as i32 {
        InvocationFreshnessDisposition::KnownFresh
    } else {
        InvocationFreshnessDisposition::MayExist
    }
}

fn publish_acceptance(
    accepted: tokio::sync::oneshot::Sender<Option<ComponentRevision>>,
    component_revision: Option<ComponentRevision>,
) -> Result<(), WorkerExecutorError> {
    accepted
        .send(component_revision)
        .map_err(|_| WorkerExecutorError::runtime("invocation session ended before acceptance"))
}

impl<Ctx: WorkerCtx, Svcs: HasAll<Ctx> + UsesAllDeps<Ctx = Ctx> + Send + Sync + 'static>
    WorkerExecutorImpl<Ctx, Svcs>
{
    async fn invoke_agent_internal(
        &self,
        request: &InvocationStart,
        method_parameters: Option<SchemaValue>,
        cancellation: tokio_util::sync::CancellationToken,
        accepted: tokio::sync::oneshot::Sender<Option<ComponentRevision>>,
    ) -> Result<AgentInvocationOutput, WorkerExecutorError> {
        Self::validate_auth_ctx(&request.auth_ctx)?;

        let freshness_disposition =
            decode_invocation_freshness_disposition(request.freshness_disposition);

        let idempotency_key: Option<IdempotencyKey> =
            request.idempotency_key.clone().map(|k| k.into());

        if freshness_disposition == InvocationFreshnessDisposition::KnownFresh
            && idempotency_key.is_none()
        {
            return Err(WorkerExecutorError::invalid_request(
                "KnownFresh requires an idempotency key",
            ));
        }

        let mode = request.mode();

        let ik = idempotency_key.unwrap_or(IdempotencyKey::fresh());
        let final_agent_id: AgentId = request
            .agent_id
            .clone()
            .ok_or(WorkerExecutorError::invalid_request("agent_id not found"))?
            .try_into()
            .map_err(WorkerExecutorError::invalid_request)?;

        if matches!(
            mode,
            golem_api_grpc::proto::golem::worker::AgentInvocationMode::Lookup
        ) {
            if freshness_disposition == InvocationFreshnessDisposition::KnownFresh {
                return Err(WorkerExecutorError::invalid_request(
                    "KnownFresh cannot be used for an invocation lookup",
                ));
            }
            let inv_status = match self.get_or_create_pending_for_lookup(request).await? {
                Some(worker) => match worker.lookup_invocation_result(&ik).await {
                    crate::model::LookupResult::Complete(Ok(_)) => InvocationStatus::Complete,
                    crate::model::LookupResult::Complete(Err(err)) => return Err(err),
                    crate::model::LookupResult::Pending => InvocationStatus::Pending,
                    crate::model::LookupResult::New | crate::model::LookupResult::Interrupted => {
                        InvocationStatus::Unknown
                    }
                },
                None => InvocationStatus::Unknown,
            };
            publish_acceptance(accepted, None)?;
            return Ok(AgentInvocationOutput {
                result: AgentInvocationResult::AgentInitialization,
                consumed_fuel: None,
                invocation_status: Some(inv_status),
                component_revision: None,
                agent_id: Some(final_agent_id),
                idempotency_key: Some(ik),
                oplog_index: None,
                agent_fingerprint: None,
            });
        }

        let method_name =
            request
                .method_name
                .clone()
                .ok_or(WorkerExecutorError::invalid_request(
                    "method_name is required for non-lookup invocations",
                ))?;

        let method_parameters = method_parameters.ok_or(WorkerExecutorError::invalid_request(
            "input is required for non-lookup invocations",
        ))?;

        let schedule_at: Option<DateTime<Utc>> = request
            .schedule_at
            .as_ref()
            .and_then(|ts| DateTime::from_timestamp(ts.seconds, ts.nanos as u32));

        let account_id: AccountId = request
            .component_owner_account_id
            .ok_or(WorkerExecutorError::invalid_request("account_id not found"))?
            .try_into()
            .map_err(|e| {
                WorkerExecutorError::invalid_request(format!("Invalid account id: {e}"))
            })?;

        let owned_agent_id =
            extract_owned_agent_id(request, |r| &r.agent_id, |r| &r.environment_id)?;

        Worker::<Ctx>::validate_invocation_freshness(
            self,
            &owned_agent_id,
            &ik,
            freshness_disposition,
        )
        .await?;

        let principal: Principal = request
            .principal
            .clone()
            .map(|p| p.try_into())
            .transpose()
            .map_err(|e: String| {
                WorkerExecutorError::invalid_request(format!("failed converting principal: {e}"))
            })?
            .unwrap_or_else(Principal::anonymous);

        let invocation_context = self
            .limit_invocation_context_stack_depth(from_proto_invocation_context(&request.context));
        let worker_creation_principal = principal.clone();

        let invocation = AgentInvocation::AgentMethod {
            idempotency_key: ik.clone(),
            method_name: method_name.clone(),
            input: method_parameters.clone(),
            invocation_context,
            principal,
        };

        match mode {
            golem_api_grpc::proto::golem::worker::AgentInvocationMode::Await => {
                let worker = self
                    .get_or_create_pending_with_freshness(request, freshness_disposition)
                    .await?;
                let status = worker.get_last_known_status().await;
                let queued_manual_update_revision = status
                    .pending_invocations
                    .iter()
                    .rev()
                    .find_map(|invocation| invocation.manual_update_target_revision);
                let pending_update_revision = queued_manual_update_revision.or_else(|| {
                    status
                        .pending_updates
                        .back()
                        .map(|update| update.target_revision)
                });
                let current_component = self
                    .component_service()
                    .get_metadata(
                        owned_agent_id.component_id(),
                        Some(status.component_revision),
                    )
                    .await?;
                let (component, streaming) = if let Some(pending_revision) = pending_update_revision
                    && pending_revision != status.component_revision
                    && let Ok(pending_component) = self
                        .component_service()
                        .get_metadata(owned_agent_id.component_id(), Some(pending_revision))
                        .await
                    && let Ok(parsed_agent_id) = ParsedAgentId::parse(
                        &owned_agent_id.agent_id.agent_id,
                        &pending_component.metadata,
                    )
                    && let Ok(streaming) = validate_agent_method_invocation(
                        &pending_component.metadata,
                        Some(&parsed_agent_id),
                        &method_name,
                        &method_parameters,
                    ) {
                    (pending_component, streaming)
                } else {
                    let parsed_agent_id = ParsedAgentId::parse(
                        &owned_agent_id.agent_id.agent_id,
                        &current_component.metadata,
                    )
                    .map_err(WorkerExecutorError::invalid_request)?;
                    let streaming = validate_agent_method_invocation(
                        &current_component.metadata,
                        Some(&parsed_agent_id),
                        &method_name,
                        &method_parameters,
                    )?;
                    (current_component, streaming)
                };
                let accepted_revision =
                    (worker.agent_mode() == AgentMode::Ephemeral).then_some(component.revision);
                let mut invocation_output = if streaming {
                    let fingerprint = worker.get_initial_worker_metadata().fingerprint;
                    let invocation = worker
                        .clone()
                        .enqueue_live_streaming(invocation, cancellation)
                        .await?;
                    publish_acceptance(accepted, accepted_revision)?;
                    AgentInvocationOutput {
                        result: AgentInvocationResult::AgentMethod {
                            output: invocation.result().await?,
                        },
                        consumed_fuel: None,
                        invocation_status: None,
                        component_revision: Some(component.revision),
                        agent_id: None,
                        idempotency_key: None,
                        oplog_index: None,
                        agent_fingerprint: Some(fingerprint),
                    }
                } else {
                    publish_acceptance(accepted, accepted_revision)?;
                    worker.invoke_and_await(invocation).await?
                };
                invocation_output.agent_id = Some(final_agent_id);
                invocation_output.idempotency_key = Some(ik);
                Ok(invocation_output)
            }
            golem_api_grpc::proto::golem::worker::AgentInvocationMode::Schedule => {
                let existing_metadata =
                    if freshness_disposition == InvocationFreshnessDisposition::KnownFresh {
                        None
                    } else {
                        Worker::<Ctx>::get_latest_metadata(self, &owned_agent_id).await
                    };
                let component_revision = existing_metadata.as_ref().map(|metadata| {
                    let status = &metadata.last_known_status;
                    status
                        .pending_invocations
                        .iter()
                        .rev()
                        .find_map(|invocation| invocation.manual_update_target_revision)
                        .or_else(|| {
                            status
                                .pending_updates
                                .back()
                                .map(|update| update.target_revision)
                        })
                        .unwrap_or(status.component_revision)
                });
                let component = self
                    .component_service()
                    .get_metadata(owned_agent_id.component_id(), component_revision)
                    .await?;
                let parsed_agent_id =
                    ParsedAgentId::parse(&owned_agent_id.agent_id.agent_id, &component.metadata)
                        .map_err(WorkerExecutorError::invalid_request)?;
                let streaming = validate_agent_method_invocation(
                    &component.metadata,
                    Some(&parsed_agent_id),
                    &method_name,
                    &method_parameters,
                )?;
                if streaming {
                    return Err(WorkerExecutorError::invalid_request(
                        "live streams require an attached Await invocation session",
                    ));
                }

                match schedule_at {
                    Some(scheduled_time) => {
                        let agent_mode = component
                            .metadata
                            .find_agent_type_by_name_ref(&parsed_agent_id.agent_type)
                            .map(|agent_type| agent_type.mode)
                            .ok_or_else(|| {
                                WorkerExecutorError::invalid_request(
                                    "Scheduled invocation target is not a registered agent type",
                                )
                            })?;
                        let action = if agent_mode == AgentMode::Ephemeral {
                            ScheduledAction::InvokeEphemeral {
                                account_id,
                                owned_agent_id,
                                invocation: Box::new(invocation),
                                component_revision: component.revision,
                                env: request.env().unwrap_or_default(),
                                config: request.config()?,
                                parent: request.parent(),
                                creation_principal: Box::new(worker_creation_principal),
                            }
                        } else {
                            let worker = self
                                .get_or_create_pending_with_freshness(
                                    request,
                                    freshness_disposition,
                                )
                                .await?;
                            let target_worker_fingerprint =
                                worker.get_initial_worker_metadata().fingerprint;
                            ScheduledAction::Invoke {
                                account_id,
                                owned_agent_id,
                                invocation: Box::new(invocation),
                                target_worker_fingerprint,
                            }
                        };
                        self.scheduler_service()
                            .schedule(scheduled_time, action)
                            .await;
                        publish_acceptance(accepted, Some(component.revision))?;
                        Ok(AgentInvocationOutput {
                            result: AgentInvocationResult::AgentInitialization,
                            consumed_fuel: None,
                            invocation_status: None,
                            component_revision: None,
                            agent_id: Some(final_agent_id),
                            idempotency_key: Some(ik),
                            oplog_index: None,
                            agent_fingerprint: None,
                        })
                    }
                    None => {
                        let worker = self
                            .get_or_create_pending_with_freshness(request, freshness_disposition)
                            .await?;
                        let result = worker.clone().invoke(invocation).await?;
                        if let crate::worker::ResultOrSubscription::Finished(Err(err)) = &result {
                            return Err(err.clone());
                        }
                        publish_acceptance(accepted, Some(component.revision))?;
                        match result {
                            crate::worker::ResultOrSubscription::Finished(Err(err)) => {
                                unreachable!(
                                    "finished errors are handled before acceptance: {err}"
                                );
                            }
                            crate::worker::ResultOrSubscription::Finished(Ok(_)) => {}
                            crate::worker::ResultOrSubscription::Pending(_) => {
                                Worker::start_if_needed(worker).await?;
                            }
                        }
                        Ok(AgentInvocationOutput {
                            result: AgentInvocationResult::AgentInitialization,
                            consumed_fuel: None,
                            invocation_status: None,
                            component_revision: None,
                            agent_id: Some(final_agent_id),
                            idempotency_key: Some(ik),
                            oplog_index: None,
                            agent_fingerprint: None,
                        })
                    }
                }
            }
            golem_api_grpc::proto::golem::worker::AgentInvocationMode::Lookup => {
                unreachable!("Lookup mode handled above")
            }
        }
    }

    async fn run_agent_session(
        &self,
        mut inbound: tonic::Streaming<InvocationRequest>,
        outward: mpsc::Sender<InvocationResponse>,
    ) {
        let mut state = InvocationSessionState::default();
        let first = match inbound.message().await {
            Ok(Some(request)) => request,
            Ok(None) => {
                send_unvalidated_rejection(
                    &outward,
                    InvocationRejectionReason::Protocol,
                    "invocation request transport closed before start".to_string(),
                    None,
                    None,
                )
                .await;
                return;
            }
            Err(error) => {
                send_unvalidated_rejection(
                    &outward,
                    InvocationRejectionReason::Protocol,
                    error.to_string(),
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
            )
            .await;
            return;
        }
        let first = first.request.expect("validated request has a payload");
        let start = match first {
            invocation_request::Request::Start(start) => start,
            invocation_request::Request::ResumeAttach(resume) => {
                let rejection = InvocationResponse {
                    response: Some(invocation_response::Response::Rejected(
                        InvocationRejected {
                            reason: InvocationRejectionReason::ResumeUnsupported as i32,
                            error: "resume-attach is not supported by live sessions".to_string(),
                            idempotency_key: resume.idempotency_key,
                            agent_id: None,
                            component_revision: None,
                        },
                    )),
                };
                if state.validate_response(&rejection).is_ok() {
                    let _ = outward.send(rejection).await;
                }
                return;
            }
            _ => unreachable!("the session validator requires start or resume-attach first"),
        };

        let state = Arc::new(tokio::sync::Mutex::new(state));
        let (responses, mut response_rx) = mpsc::channel(32);
        let response_state = state.clone();
        let outward_forwarder = outward.clone();
        let forwarder = tokio::spawn(async move {
            while let Some(response) = response_rx.recv().await {
                if let Err(error) = response_state.lock().await.validate_response(&response) {
                    tracing::error!(error, ?response, "Invalid invocation session response");
                    return;
                }
                if outward_forwarder.send(response).await.is_err() {
                    return;
                }
            }
        });

        let session = LiveValueSession::new_server_with_capacity(
            responses.clone(),
            self.services
                .config()
                .limits
                .live_stream_event_broadcast_capacity
                .get(),
        );
        let input =
            if start.mode() == golem_api_grpc::proto::golem::worker::AgentInvocationMode::Lookup {
                Ok(None)
            } else {
                match start.input.clone() {
                    Some(input) => session.decode_start(input).await.map(Some),
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
                session.cancel();
                drop(session);
                drop(responses);
                let _ = forwarder.await;
                return;
            }
        };

        let cancellation = tokio_util::sync::CancellationToken::new();
        let _cancel_on_drop = cancellation.clone().drop_guard();
        let (accepted_tx, mut accepted_rx) = tokio::sync::oneshot::channel();
        let invocation = self.invoke_agent_internal(&start, input, cancellation, accepted_tx);
        tokio::pin!(invocation);
        let mut early_output = None;
        let accepted_revision = tokio::select! {
            biased;
            accepted = &mut accepted_rx => match accepted {
                Ok(revision) => revision,
                Err(_) => {
                    send_rejection(
                        &responses,
                        InvocationRejectionReason::Internal,
                        "invocation ended without reaching acceptance".to_string(),
                        &start,
                    ).await;
                    session.cancel();
                    return;
                }
            },
            result = &mut invocation => {
                if let Ok(revision) = accepted_rx.try_recv() {
                    early_output = Some(result);
                    revision
                } else {
                    let (reason, error) = match result {
                        Ok(_) => (
                            InvocationRejectionReason::Internal,
                            "invocation completed before acceptance".to_string(),
                        ),
                        Err(error) => (
                            pre_acceptance_rejection_reason(&error),
                            error.to_string(),
                        ),
                    };
                    send_rejection(&responses, reason, error, &start).await;
                    session.cancel();
                    return;
                }
            }
            request = inbound.message() => {
                let error = match request {
                    Ok(Some(request)) => state
                        .lock()
                        .await
                        .validate_trusted_request(&request)
                        .unwrap_err(),
                    Ok(None) => "invocation request transport closed before acceptance".to_string(),
                    Err(error) => error.to_string(),
                };
                send_rejection(
                    &responses,
                    InvocationRejectionReason::Protocol,
                    error,
                    &start,
                ).await;
                session.cancel();
                return;
            }
        };

        if responses
            .send(InvocationResponse {
                response: Some(invocation_response::Response::Accepted(
                    InvocationAccepted {
                        agent_id: start.agent_id.clone(),
                        idempotency_key: start.idempotency_key.clone(),
                        component_revision: accepted_revision.map(|revision| revision.get()),
                    },
                )),
            })
            .await
            .is_err()
        {
            session.cancel();
            return;
        }

        let output = match early_output {
            Some(output) => output,
            None => loop {
                tokio::select! {
                    result = &mut invocation => break result,
                    request = inbound.message() => match request {
                        Ok(Some(request)) => {
                            if !route_live_request(
                                &session,
                                &responses,
                                &state,
                                request,
                            ).await {
                                return;
                            }
                        }
                        Ok(None) => {
                            fail_session_transport(
                                &session,
                                &responses,
                                "invocation request transport closed before the result".to_string(),
                            )
                            .await;
                            return;
                        }
                        Err(error) => {
                            fail_session_transport(&session, &responses, error.to_string()).await;
                            return;
                        }
                    }
                }
            },
        };

        let output = match output {
            Ok(output) => output,
            Err(error) => {
                session.terminate_for_failure(&error.to_string()).await;
                send_worker_failure(&responses, error).await;
                return;
            }
        };
        let (result, output_stream_ids) = match &output.result {
            AgentInvocationResult::AgentMethod { output } => match session.encode_pending(output) {
                Ok((output, stream_ids)) => (
                    Some(invocation_session_result::Result::MethodResult(output)),
                    stream_ids,
                ),
                Err(error) => {
                    session.terminate_for_failure(&error).await;
                    send_protocol_failure(&responses, error).await;
                    return;
                }
            },
            _ => (
                Some(invocation_session_result::Result::NoResult(
                    golem::common::Empty {},
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
                    },
                )),
            })
            .await
            .is_err()
        {
            session.cancel();
            return;
        }
        session.activate_exported_streams(&output_stream_ids);

        let idle = session.wait_idle();
        tokio::pin!(idle);
        loop {
            tokio::select! {
                () = &mut idle => {
                    if let Err(details) = session.finish_invocation().await {
                        send_protocol_failure(&responses, details).await;
                        return;
                    }
                    let _ = responses.send(InvocationResponse {
                        response: Some(invocation_response::Response::Finished(
                            InvocationSessionCompletion {
                                outcome: Some(invocation_session_completion::Outcome::Success(
                                    golem::common::Empty {},
                                )),
                            },
                        )),
                    }).await;
                    return;
                }
                request = inbound.message() => match request {
                    Ok(Some(request)) => {
                        if !route_live_request(
                            &session,
                            &responses,
                            &state,
                            request,
                        ).await {
                            return;
                        }
                    }
                    Ok(None) => {
                        fail_session_transport(
                            &session,
                            &responses,
                            "invocation request transport closed before completion".to_string(),
                        )
                        .await;
                        return;
                    }
                    Err(error) => {
                        fail_session_transport(&session, &responses, error.to_string()).await;
                        return;
                    }
                }
            }
        }
    }
}

fn request_idempotency_key(
    request: &InvocationRequest,
) -> Option<golem_api_grpc::proto::golem::worker::IdempotencyKey> {
    match request.request.as_ref() {
        Some(invocation_request::Request::Start(start)) => start.idempotency_key.clone(),
        Some(invocation_request::Request::ResumeAttach(resume)) => resume.idempotency_key.clone(),
        _ => None,
    }
}

fn request_agent_id(
    request: &InvocationRequest,
) -> Option<golem_api_grpc::proto::golem::worker::AgentId> {
    match request.request.as_ref() {
        Some(invocation_request::Request::Start(start)) => start.agent_id.clone(),
        _ => None,
    }
}

fn pre_acceptance_rejection_reason(error: &WorkerExecutorError) -> InvocationRejectionReason {
    match error {
        WorkerExecutorError::InvalidRequest { .. }
        | WorkerExecutorError::ParamTypeMismatch { .. }
        | WorkerExecutorError::NoValueInMessage
        | WorkerExecutorError::ValueMismatch { .. } => InvocationRejectionReason::Validation,
        WorkerExecutorError::AgentNotFound { .. }
        | WorkerExecutorError::ComponentNotFound { .. }
        | WorkerExecutorError::PromiseNotFound { .. } => InvocationRejectionReason::NotFound,
        WorkerExecutorError::InvalidAccount => InvocationRejectionReason::Unauthorized,
        _ => InvocationRejectionReason::Internal,
    }
}

async fn send_unvalidated_rejection(
    responses: &mpsc::Sender<InvocationResponse>,
    reason: InvocationRejectionReason,
    error: String,
    idempotency_key: Option<golem_api_grpc::proto::golem::worker::IdempotencyKey>,
    agent_id: Option<golem_api_grpc::proto::golem::worker::AgentId>,
) {
    let _ = responses
        .send(InvocationResponse {
            response: Some(invocation_response::Response::Rejected(
                InvocationRejected {
                    reason: reason as i32,
                    error,
                    idempotency_key,
                    agent_id,
                    component_revision: None,
                },
            )),
        })
        .await;
}

async fn send_rejection(
    responses: &mpsc::Sender<InvocationResponse>,
    reason: InvocationRejectionReason,
    error: String,
    start: &InvocationStart,
) {
    send_unvalidated_rejection(
        responses,
        reason,
        error,
        start.idempotency_key.clone(),
        start.agent_id.clone(),
    )
    .await;
}

async fn send_failure(
    responses: &mpsc::Sender<InvocationResponse>,
    kind: InvocationFailureKind,
    code: &str,
    message: String,
    worker_error: Option<WorkerExecutionError>,
) {
    let _ = responses
        .send(InvocationResponse {
            response: Some(invocation_response::Response::Finished(
                InvocationSessionCompletion {
                    outcome: Some(invocation_session_completion::Outcome::Failure(
                        InvocationFailure {
                            kind: kind as i32,
                            code: code.to_string(),
                            message,
                            worker_error,
                        },
                    )),
                },
            )),
        })
        .await;
}

async fn send_protocol_failure(responses: &mpsc::Sender<InvocationResponse>, details: String) {
    send_failure(
        responses,
        InvocationFailureKind::Protocol,
        "protocol",
        details,
        None,
    )
    .await;
}

async fn send_worker_failure(
    responses: &mpsc::Sender<InvocationResponse>,
    error: WorkerExecutorError,
) {
    let message = error.to_string();
    send_failure(
        responses,
        InvocationFailureKind::Execution,
        "worker-execution",
        message,
        Some(error.into()),
    )
    .await;
}

async fn fail_session_transport(
    session: &LiveValueSession,
    responses: &mpsc::Sender<InvocationResponse>,
    details: String,
) {
    session.terminate_for_failure(&details).await;
    send_failure(
        responses,
        InvocationFailureKind::Transport,
        "transport",
        details,
        None,
    )
    .await;
}

async fn route_live_request(
    session: &LiveValueSession,
    responses: &mpsc::Sender<InvocationResponse>,
    state: &Arc<tokio::sync::Mutex<InvocationSessionState>>,
    request: InvocationRequest,
) -> bool {
    let request = match state
        .lock()
        .await
        .validate_received_trusted_request(&request)
    {
        Ok(()) => request
            .request
            .expect("validated invocation request has a payload"),
        Err(details) => {
            session.terminate_for_failure(&details).await;
            send_protocol_failure(responses, details).await;
            return false;
        }
    };
    match session.route_request(request).await {
        Ok(true) => true,
        Ok(false) => {
            let details = "unexpected message on invocation request stream".to_string();
            session.terminate_for_failure(&details).await;
            send_protocol_failure(responses, details).await;
            false
        }
        Err(details) => {
            session.terminate_for_failure(&details).await;
            send_protocol_failure(responses, details).await;
            false
        }
    }
}

#[cfg(test)]
mod freshness_tests {
    use super::decode_invocation_freshness_disposition;
    use golem_common::model::agent::InvocationFreshnessDisposition;
    use test_r::test;

    #[test]
    fn invocation_freshness_defaults_unknown_values_to_may_exist() {
        assert_eq!(
            decode_invocation_freshness_disposition(0),
            InvocationFreshnessDisposition::MayExist
        );
        assert_eq!(
            decode_invocation_freshness_disposition(i32::MAX),
            InvocationFreshnessDisposition::MayExist
        );
    }

    #[test]
    fn invocation_freshness_decodes_known_fresh_explicitly() {
        assert_eq!(
            decode_invocation_freshness_disposition(
                golem_api_grpc::proto::golem::worker::InvocationFreshnessDisposition::KnownFresh
                    as i32
            ),
            InvocationFreshnessDisposition::KnownFresh
        );
    }
}
