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
use crate::durable_host::durable_session::{
    DurableSessionStreams, durable_stream_mapping_from_proto, durable_stream_mapping_to_proto,
};
use crate::durable_host::durable_stream::ProducerRegistrationRequestV1;
use crate::durable_host::stream_session::{
    decode_recursive_stream_value, decode_recursive_stream_value_with_schema,
    encode_recursive_stream_value_with_schema,
};
use crate::grpc::invocation::{CanStartWorker, from_proto_invocation_context};
use crate::services::{HasAll, HasComponentService, HasSchedulerService, UsesAllDeps};
use crate::worker::invocation::validate_agent_method_invocation;
use crate::worker::{DurableStreamingInvocationRequest, Worker};
use crate::workerctx::WorkerCtx;
use chrono::{DateTime, Utc};
use futures::{Stream, StreamExt};
use golem_api_grpc::invocation_session_protocol::InvocationSessionState;
use golem_api_grpc::proto::golem;
use golem_api_grpc::proto::golem::worker::v1::WorkerExecutionError;
use golem_api_grpc::proto::golem::worker::{
    InputStreamAck, InvocationAccepted, InvocationFailure, InvocationFailureKind,
    InvocationRejected, InvocationRejectionReason, InvocationRequest, InvocationResponse,
    InvocationSessionCompletion, InvocationSessionResult, InvocationStart, ResumeAttach,
    ResumeOperation, invocation_request, invocation_response, invocation_session_completion,
    invocation_session_result,
};
use golem_common::base_model::durable_stream::{
    MAX_DURABLE_STREAM_ITEM_SIZE, MAX_NEW_STREAM_HANDLES_PER_VALUE, ResumeAttemptDescriptorV1,
    StreamCancelReasonV1, StreamCancelRoleV1, StreamItemsPayloadV1, StreamResumeCursorV1,
    StreamResumeOperationV1,
};
use golem_common::model::account::AccountId;
use golem_common::model::agent::{
    AgentMode, InvocationFreshnessDisposition, ParsedAgentId, Principal,
};
use golem_common::model::card::ScopeCard;
use golem_common::model::component::ComponentRevision;
use golem_common::model::durable_stream::{
    AttachmentId, AttemptId, DURABLE_STREAM_FORMAT_VERSION, PersistedStreamInvocationDescriptorV1,
    SessionStreamRoleV1, StartAttemptDescriptorV1, StreamInvocationIdV1,
    StreamRegistrationCoordinateV1, StreamRootKindV1, StreamSessionMappingV1, StreamSourceKindV1,
    StreamValuePathStepV1,
};
use golem_common::model::{
    AgentId, AgentInvocation, AgentInvocationOutput, AgentInvocationResult, IdempotencyKey,
    InvocationStatus, ScheduledAction,
};
use golem_common::schema::SchemaValue;
use golem_common::schema::agent::FieldSource;
use golem_schema::schema::{
    NamedFieldType, SchemaGraph, SchemaType, SchemaValueStream, schema_fingerprint_v1,
};
use golem_service_base::error::worker_executor::WorkerExecutorError;
use golem_service_base::grpc::proto_agent_id_string;
use prost::Message;
use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Request, Response, Status};
use tracing::Instrument;

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
    let span = tracing::info_span!(
        "invoke_agent_session",
        agent_id = tracing::field::Empty,
        idempotency_key = tracing::field::Empty
    );
    tokio::spawn(
        async move {
            executor.run_agent_session(inbound, responses).await;
        }
        .instrument(span),
    );
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
    acceptance_committed: tokio::sync::oneshot::Sender<()>,
    accepted: tokio::sync::oneshot::Sender<AcceptedInvocation>,
    component_revision: Option<ComponentRevision>,
) -> Result<(), WorkerExecutorError> {
    let _ = acceptance_committed.send(());
    accepted
        .send(AcceptedInvocation {
            component_revision,
            durable_streams: None,
            prepared: None,
            durable_replayed: false,
        })
        .map_err(|_| WorkerExecutorError::runtime("invocation session ended before acceptance"))
}

struct AcceptedInvocation {
    component_revision: Option<ComponentRevision>,
    durable_streams: Option<DurableSessionStreams>,
    prepared: Option<golem_common::model::durable_stream::StreamSessionPreparedRecordV1>,
    durable_replayed: bool,
}

struct TransportStreamId(u64);

pub(crate) fn decode_invocation_input(
    input: golem_api_grpc::proto::golem::schema::SchemaValue,
) -> Result<SchemaValue, String> {
    decode_recursive_stream_value(input, |stream_id, _| {
        Ok(SchemaValueStream::from_host_endpoint(TransportStreamId(
            stream_id,
        )))
    })
}

async fn detach_durable_attachment(streams: Option<DurableSessionStreams>) {
    if let Some(streams) = streams
        && let Err(error) = streams.detach_current().await
    {
        tracing::warn!(%error, "failed to persist durable invocation transport detach");
    }
}

fn is_attachment_termination(error: &str) -> bool {
    error.contains("response stream closed") || error.starts_with("StaleEpoch:")
}

async fn send_attachment_revocation(
    responses: &mpsc::Sender<InvocationResponse>,
    revocation: Result<(), String>,
) {
    match revocation {
        Ok(()) => {
            let _ = responses
                .send(InvocationResponse {
                    response: Some(invocation_response::Response::AttachmentRevoked(
                        golem_api_grpc::proto::golem::worker::AttachmentRevoked {
                            details: "the durable attachment was fenced by a later epoch"
                                .to_string(),
                        },
                    )),
                })
                .await;
        }
        Err(error) => send_protocol_failure(responses, error).await,
    }
}

enum AcceptanceRace<A, O, R> {
    Accepted {
        acceptance: A,
        early_output: Option<O>,
        early_inbound: Option<R>,
    },
    InvocationFinished(O),
    InboundBeforeAcceptance(R),
}

async fn race_invocation_acceptance<A, O, R, I, F>(
    accepted: &mut tokio::sync::oneshot::Receiver<A>,
    acceptance_committed: &mut tokio::sync::oneshot::Receiver<()>,
    mut invocation: Pin<&mut I>,
    inbound: F,
) -> AcceptanceRace<A, O, R>
where
    I: Future<Output = O>,
    F: Future<Output = R>,
{
    tokio::pin!(inbound);
    let mut commit_observed = false;
    let mut commit_channel_open = true;
    let mut early_output = None;
    let mut early_inbound = None;

    loop {
        tokio::select! {
            biased;
            accepted = &mut *accepted => return match accepted {
                Ok(acceptance) => AcceptanceRace::Accepted {
                    acceptance,
                    early_output,
                    early_inbound,
                },
                Err(_) => AcceptanceRace::InvocationFinished(match early_output {
                    Some(output) => output,
                    None => invocation.await,
                }),
            },
            committed = &mut *acceptance_committed, if !commit_observed && commit_channel_open => {
                match committed {
                    Ok(()) => commit_observed = true,
                    Err(_) => commit_channel_open = false,
                }
            },
            output = &mut invocation, if early_output.is_none() => {
                match accepted.try_recv() {
                    Ok(acceptance) => return AcceptanceRace::Accepted {
                        acceptance,
                        early_output: Some(output),
                        early_inbound,
                    },
                    Err(tokio::sync::oneshot::error::TryRecvError::Closed) => {
                        return AcceptanceRace::InvocationFinished(output);
                    }
                    Err(tokio::sync::oneshot::error::TryRecvError::Empty) => {}
                }
                if !commit_observed && commit_channel_open {
                    match acceptance_committed.try_recv() {
                        Ok(()) => commit_observed = true,
                        Err(tokio::sync::oneshot::error::TryRecvError::Closed) => {
                            commit_channel_open = false;
                        }
                        Err(tokio::sync::oneshot::error::TryRecvError::Empty) => {}
                    }
                }
                if commit_observed {
                    early_output = Some(output);
                } else {
                    return AcceptanceRace::InvocationFinished(output);
                }
            },
            request = &mut inbound, if early_inbound.is_none() => {
                match accepted.try_recv() {
                    Ok(acceptance) => return AcceptanceRace::Accepted {
                        acceptance,
                        early_output,
                        early_inbound: Some(request),
                    },
                    Err(tokio::sync::oneshot::error::TryRecvError::Closed) => {
                        return AcceptanceRace::InvocationFinished(match early_output.take() {
                            Some(output) => output,
                            None => invocation.await,
                        });
                    }
                    Err(tokio::sync::oneshot::error::TryRecvError::Empty) => {}
                }
                if !commit_observed && commit_channel_open {
                    match acceptance_committed.try_recv() {
                        Ok(()) => commit_observed = true,
                        Err(tokio::sync::oneshot::error::TryRecvError::Closed) => {
                            commit_channel_open = false;
                        }
                        Err(tokio::sync::oneshot::error::TryRecvError::Empty) => {}
                    }
                }
                if commit_observed || commit_channel_open {
                    early_inbound = Some(request);
                } else {
                    return AcceptanceRace::InboundBeforeAcceptance(request);
                }
            },
        }
    }
}

impl<Ctx: WorkerCtx, Svcs: HasAll<Ctx> + UsesAllDeps<Ctx = Ctx> + Send + Sync + 'static>
    WorkerExecutorImpl<Ctx, Svcs>
{
    async fn invoke_agent_internal(
        &self,
        request: &InvocationStart,
        method_parameters: Option<SchemaValue>,
        input_encoded_len: Option<usize>,
        acceptance_committed: tokio::sync::oneshot::Sender<()>,
        accepted: tokio::sync::oneshot::Sender<AcceptedInvocation>,
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

        let scope_card: Option<ScopeCard> = request
            .scope_card
            .clone()
            .map(TryInto::try_into)
            .transpose()
            .map_err(WorkerExecutorError::permission_denied)?;
        if scope_card.is_some()
            && mode != golem_api_grpc::proto::golem::worker::AgentInvocationMode::Await
        {
            return Err(WorkerExecutorError::permission_denied(
                "scope cards are supported only for invoke-and-await",
            ));
        }

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
            publish_acceptance(acceptance_committed, accepted, None)?;
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
                let invocation = AgentInvocation::AgentMethod {
                    idempotency_key: ik.clone(),
                    method_name: method_name.clone(),
                    input: method_parameters,
                    invocation_context,
                    principal,
                    scope_card,
                };
                let mut invocation_output = if streaming {
                    let input_encoded_len = input_encoded_len.ok_or_else(|| {
                        WorkerExecutorError::invalid_request(
                            "durable streaming invocation is missing its encoded input length",
                        )
                    })?;
                    let request = build_durable_streaming_request(
                        request,
                        &component.metadata,
                        component.revision,
                        worker.get_initial_worker_metadata().fingerprint,
                        invocation,
                        input_encoded_len,
                        acceptance_committed,
                        self.services
                            .config()
                            .limits
                            .live_stream_event_broadcast_capacity
                            .get(),
                    )?;
                    let acceptance = worker
                        .clone()
                        .accept_durable_streaming_invocation(request)
                        .await?;
                    let streams = acceptance.streams;
                    accepted
                        .send(AcceptedInvocation {
                            component_revision: Some(component.revision),
                            durable_streams: Some(streams.clone()),
                            prepared: Some(acceptance.prepared),
                            durable_replayed: acceptance.replayed,
                        })
                        .map_err(|_| {
                            WorkerExecutorError::runtime(
                                "invocation session ended before durable acceptance",
                            )
                        })?;
                    streams
                        .recover_nested_input_mappings()
                        .await
                        .map_err(WorkerExecutorError::runtime)?;
                    worker.await_enqueued_invocation(ik.clone()).await?
                } else {
                    publish_acceptance(acceptance_committed, accepted, accepted_revision)?;
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
                let invocation = AgentInvocation::AgentMethod {
                    idempotency_key: ik.clone(),
                    method_name,
                    input: method_parameters,
                    invocation_context,
                    principal,
                    scope_card,
                };

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
                        publish_acceptance(
                            acceptance_committed,
                            accepted,
                            Some(component.revision),
                        )?;
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
                        publish_acceptance(
                            acceptance_committed,
                            accepted,
                            Some(component.revision),
                        )?;
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
                self.run_resumed_agent_session(resume, inbound, outward, state)
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
        let forwarder = tokio::spawn(async move {
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
        let input =
            if start.mode() == golem_api_grpc::proto::golem::worker::AgentInvocationMode::Lookup {
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
        let invocation = self.invoke_agent_internal(
            &start,
            input,
            input_encoded_len,
            acceptance_committed_tx,
            accepted_tx,
        );
        tokio::pin!(invocation);
        let (accepted, early_output, mut early_inbound) = match race_invocation_acceptance(
            &mut accepted_rx,
            &mut acceptance_committed_rx,
            invocation.as_mut(),
            inbound.message(),
        )
        .await
        {
            AcceptanceRace::Accepted {
                acceptance,
                early_output,
                early_inbound,
            } => (acceptance, early_output, early_inbound),
            AcceptanceRace::InvocationFinished(result) => {
                let (reason, error) = match result {
                    Ok(_) => (
                        InvocationRejectionReason::Internal,
                        "invocation completed before acceptance".to_string(),
                    ),
                    Err(error) => (pre_acceptance_rejection_reason(&error), error.to_string()),
                };
                send_rejection(&responses, reason, error, &start).await;
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

        let durable_attachment = accepted.durable_streams.clone();
        async {
        let high_waters = if let Some(durable_streams) = &accepted.durable_streams {
            match durable_streams.input_high_waters().await {
                Ok(high_waters) => high_waters,
                Err(error) => {
                    send_rejection(
                        &responses,
                        InvocationRejectionReason::Internal,
                        error,
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
                            .map(|prepared| prepared.attempt.attachment_id.0.into()),
                        attempt_id: accepted
                            .prepared
                            .as_ref()
                            .map(|prepared| prepared.attempt.attempt_id.0.into()),
                        epoch: accepted.prepared.as_ref().map(|_| 1).unwrap_or_default(),
                        stream_mappings: accepted
                            .prepared
                            .as_ref()
                            .map(|prepared| {
                                prepared
                                    .stream_mappings
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
                    },
                )),
            })
            .await
            .is_err()
        {
            return;
        }
        acceptance_forwarded.await;
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
                            if let Err(finish_error) =
                                durable_streams.fail_protocol(details.clone()).await
                            {
                                send_protocol_failure(&responses, finish_error).await;
                                return;
                            }
                            let _ = durable_streams.pump_input_cancellations(&responses, &high_waters).await;
                            let _ = durable_streams.pump_output_streams(&responses).await;
                            send_protocol_failure(&responses, details).await;
                            return;
                        }
                    }
                    Ok(None) | Err(_) => {
                        return;
                    }
                }
            }
            let mut completed_output = early_output;
            let persisted_result = match durable_streams.persisted_result().await {
                Ok(Some(result)) => Some(result),
                Ok(None) if completed_output.is_some() => None,
                Ok(None) => {
                    let result = durable_streams.wait_persisted_result();
                    tokio::pin!(result);
                    loop {
                        tokio::select! {
                            biased;
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
                                    match durable_streams.persisted_result().await {
                                        Ok(result) => break result,
                                        Err(error) => {
                                            send_protocol_failure(&responses, error).await;
                                            return;
                                        }
                                    }
                                }
                                Err(error) => {
                                    completed_output = Some(Err(error));
                                    match durable_streams.persisted_result().await {
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
                                        if let Err(finish_error) =
                                            durable_streams.fail_protocol(details.clone()).await
                                        {
                                            send_protocol_failure(&responses, finish_error).await;
                                            return;
                                        }
                                        let _ = durable_streams
                                            .pump_input_cancellations(&responses, &high_waters)
                                            .await;
                                        let _ = durable_streams.pump_output_streams(&responses).await;
                                        send_protocol_failure(&responses, details).await;
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
                            result: Some(invocation_session_result::Result::MethodResult(
                                persisted_result.0,
                            )),
                            component_revision: accepted
                                .component_revision
                                .map(|revision| revision.get()),
                            agent_id: start.agent_id.clone(),
                            idempotency_key: start.idempotency_key.clone(),
                            fuel_consumed: None,
                            status: None,
                            oplog_index: None,
                            agent_fingerprint: start.expected_callee_fingerprint,
                            new_stream_mappings: persisted_result.1,
                        },
                    )),
                })
                .await
                .is_err()
            {
                return;
            }

            let mut output_pump = tokio::task::JoinSet::new();
            let output_streams = durable_streams.clone();
            let output_responses = responses.clone();
            output_pump
                .spawn(async move { output_streams.pump_output_streams(&output_responses).await });
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
                            if !output_pump_finished {
                                let _ = output_pump.join_next().await;
                            }
                            send_worker_failure(&responses, error).await;
                            return;
                        }
                    },
                    result = output_pump.join_next(), if !output_pump_finished => {
                        let result = match result {
                            Some(Ok(result)) => result,
                            Some(Err(error)) => Err(format!(
                                "durable output pump task failed: {error}"
                            )),
                            None => Err("durable output pump task stopped unexpectedly".to_string()),
                        };
                        match result {
                            Ok(()) => {
                                output_pump_finished = true;
                            },
                            Err(error) if is_attachment_termination(&error) => {
                                return;
                            }
                            Err(error) => {
                                if let Err(finish_error) = durable_streams
                                    .fail_protocol(error.clone())
                                    .await
                                {
                                    send_protocol_failure(&responses, finish_error).await;
                                    return;
                                }
                                let _ = durable_streams
                                    .pump_input_cancellations(&responses, &high_waters)
                                    .await;
                                let _ = durable_streams.pump_output_streams(&responses).await;
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
                                if let Err(finish_error) =
                                    durable_streams.fail_protocol(details.clone()).await
                                {
                                    send_protocol_failure(&responses, finish_error).await;
                                    return;
                                }
                                let _ = durable_streams
                                    .pump_input_cancellations(&responses, &high_waters)
                                    .await;
                                if !output_pump_finished {
                                    let _ = output_pump.join_next().await;
                                }
                                send_protocol_failure(&responses, details).await;
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
                                if let Err(finish_error) =
                                    durable_streams.fail_protocol(details.clone()).await
                                {
                                    send_protocol_failure(&responses, finish_error).await;
                                    return;
                                }
                                let _ = durable_streams
                                    .pump_input_cancellations(&responses, &high_waters)
                                    .await;
                                send_protocol_failure(&responses, details).await;
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
                                golem::common::Empty {},
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
        let (result, new_stream_mappings) = match &output.result {
            AgentInvocationResult::AgentMethod { output } => match output.clone().try_into() {
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
                            golem::common::Empty {},
                        )),
                    },
                )),
            })
            .await;
        }
        .await;
        detach_durable_attachment(durable_attachment).await;
    }
}

impl<Ctx: WorkerCtx, Svcs: HasAll<Ctx> + UsesAllDeps<Ctx = Ctx> + Send + Sync + 'static>
    WorkerExecutorImpl<Ctx, Svcs>
{
    async fn run_resumed_agent_session(
        &self,
        resume: ResumeAttach,
        mut inbound: tonic::Streaming<InvocationRequest>,
        outward: mpsc::Sender<InvocationResponse>,
        mut protocol_state: InvocationSessionState,
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
            let worker = self
                .get_or_create_pending_for_lookup(&lookup)
                .await?
                .ok_or_else(|| {
                    WorkerExecutorError::invalid_request(
                        "NotFound: durable Stream Session worker was not found",
                    )
                })?;
            worker.resume_durable_streaming_invocation(attempt).await
        }
        .await;
        let mut acceptance = match result {
            Ok(acceptance) => acceptance,
            Err(error) => {
                let rejection = InvocationResponse {
                    response: Some(invocation_response::Response::Rejected(
                        InvocationRejected {
                            reason: pre_acceptance_rejection_reason(&error) as i32,
                            error: error.to_string(),
                            idempotency_key: rejection_identity.0,
                            agent_id: rejection_identity.1,
                            component_revision: None,
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
        async {
            let result = async {
                let component_revision = acceptance
                    .prepared
                    .attempt
                    .invocation
                    .target_component_revision;
                let component = self
                    .component_service()
                    .get_metadata(
                        acceptance.prepared.attempt.session_key.callee.component_id,
                        Some(component_revision),
                    )
                    .await?;
                let (input_schema, input_element_types) =
                    resumed_input_schema(&acceptance.prepared, &component.metadata)?;
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
                                golem_common::model::durable_stream::StreamOffsetV1::from_bytes(
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
                    .map_err(WorkerExecutorError::invalid_request)?;
                for stream_id in terminal_cursor_stream_ids {
                    protocol_state
                        .mark_terminal_resume_cursor(stream_id.0.as_u64_pair())
                        .map_err(WorkerExecutorError::invalid_request)?;
                }
                Ok::<_, WorkerExecutorError>((acceptance, cursor_map))
            }
            .await;
        let (acceptance, cursor_map) = match result {
            Ok(result) => result,
            Err(error) => {
                let rejection = InvocationResponse {
                    response: Some(invocation_response::Response::Rejected(
                        InvocationRejected {
                            reason: pre_acceptance_rejection_reason(&error) as i32,
                            error: error.to_string(),
                            idempotency_key: rejection_identity.0,
                            agent_id: rejection_identity.1,
                            component_revision: None,
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
        let forwarder = tokio::spawn(async move {
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
            .filter(|mapping| mapping.role == SessionStreamRoleV1::Output)
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
                        method_name: Some(
                            acceptance
                                .prepared
                                .attempt
                                .invocation
                                .method_name
                                .clone(),
                        ),
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

        let (persisted_result, already_finished) = loop {
            let persisted_result = match streams.persisted_result().await {
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
            let changed = streams.producer.session_records_changed().notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            tokio::select! {
                () = &mut changed => {}
                request = inbound.message() => match request {
                    Ok(Some(request)) => {
                        if let Err(error) = route_durable_request(&streams, &responses, &state, request).await {
                            if error.starts_with("StaleEpoch:") {
                                return;
                            }
                            send_protocol_failure(&responses, error).await;
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
        if let Some((result, new_stream_mappings)) = persisted_result
            && responses
                .send(InvocationResponse {
                    response: Some(invocation_response::Response::Result(
                        InvocationSessionResult {
                            result: Some(invocation_session_result::Result::MethodResult(result)),
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
                            new_stream_mappings,
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
            let mut output_pump = tokio::task::JoinSet::new();
            let output_streams = streams.clone();
            let output_responses = responses.clone();
            output_pump.spawn(async move {
                output_streams
                    .pump_output_streams_from(
                        &cursor_map,
                        &root_output_mapping_ids,
                        &known_output_mapping_ids,
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
                            )),
                            None => Err("durable output pump task stopped unexpectedly".to_string()),
                        };
                        if let Err(error) = result
                            && !is_attachment_termination(&error)
                        {
                            send_protocol_failure(&responses, error).await;
                            return;
                        }
                        break;
                    }
                    request = inbound.message() => match request {
                        Ok(Some(request)) => {
                            if let Err(error) = route_durable_request(&streams, &responses, &state, request).await {
                                if error.starts_with("StaleEpoch:") {
                                    return;
                                }
                                send_protocol_failure(&responses, error).await;
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
                            if error.starts_with("StaleEpoch:") {
                                return;
                            }
                            send_protocol_failure(&responses, error).await;
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
        }
        .await;
        detach_durable_attachment(Some(durable_attachment)).await;
    }
}

async fn send_resumed_finished(
    responses: &mpsc::Sender<InvocationResponse>,
    result: Result<(), Vec<u8>>,
) {
    let outcome = match result {
        Ok(()) => invocation_session_completion::Outcome::Success(golem::common::Empty {}),
        Err(details) => invocation_session_completion::Outcome::Failure(InvocationFailure {
            kind: InvocationFailureKind::Execution as i32,
            code: "persisted-invocation-failure".to_string(),
            message: String::from_utf8_lossy(&details).into_owned(),
            worker_error: None,
        }),
    };
    let _ = responses
        .send(InvocationResponse {
            response: Some(invocation_response::Response::Finished(
                InvocationSessionCompletion {
                    outcome: Some(outcome),
                },
            )),
        })
        .await;
}

pub(crate) fn build_durable_streaming_request(
    request: &InvocationStart,
    component_metadata: &golem_common::model::component_metadata::ComponentMetadata,
    component_revision: ComponentRevision,
    callee_fingerprint: golem_common::model::AgentFingerprint,
    invocation: AgentInvocation,
    input_encoded_len: usize,
    acceptance_committed: tokio::sync::oneshot::Sender<()>,
    live_join_buffer_events: usize,
) -> Result<DurableStreamingInvocationRequest, WorkerExecutorError> {
    let callee: AgentId = request
        .agent_id
        .clone()
        .ok_or_else(|| WorkerExecutorError::invalid_request("agent_id not found"))?
        .try_into()
        .map_err(WorkerExecutorError::invalid_request)?;
    let environment_id = request
        .environment_id
        .ok_or_else(|| WorkerExecutorError::invalid_request("environment_id not found"))?
        .try_into()
        .map_err(WorkerExecutorError::invalid_request)?;
    let idempotency_key: IdempotencyKey = request
        .idempotency_key
        .clone()
        .ok_or_else(|| {
            WorkerExecutorError::invalid_request(
                "durable streaming invocations require an idempotency key",
            )
        })?
        .into();
    let method_name = request
        .method_name
        .clone()
        .ok_or_else(|| WorkerExecutorError::invalid_request("method_name is required"))?;
    let attempt_id = AttemptId(
        request
            .attempt_id
            .ok_or_else(|| {
                WorkerExecutorError::invalid_request(
                    "durable streaming invocations require a client attempt ID",
                )
            })?
            .into(),
    );
    if attempt_id.0.is_nil() || attempt_id.0.get_version() != Some(uuid::Version::Random) {
        return Err(WorkerExecutorError::invalid_request(
            "durable streaming invocation attempt ID must be a non-nil UUIDv4",
        ));
    }
    require_expected_callee_fingerprint(request.expected_callee_fingerprint, callee_fingerprint)?;
    let parsed_agent_id = ParsedAgentId::parse(&callee.agent_id, component_metadata)
        .map_err(WorkerExecutorError::invalid_request)?;
    let agent_type = component_metadata
        .find_agent_type_by_name_ref(&parsed_agent_id.agent_type)
        .ok_or_else(|| WorkerExecutorError::invalid_request("agent type not found"))?;
    let method = agent_type
        .methods
        .iter()
        .find(|method| method.name == method_name)
        .ok_or_else(|| WorkerExecutorError::invalid_request("agent method not found"))?;
    let input_root = SchemaType::record(
        method
            .input_schema
            .fields()
            .iter()
            .filter(|field| matches!(field.source, FieldSource::UserSupplied))
            .map(|field| NamedFieldType {
                name: field.name.clone(),
                body: field.schema.clone(),
                metadata: field.metadata.clone(),
            })
            .collect(),
    );
    let invocation_input = match &invocation {
        AgentInvocation::AgentMethod { input, .. } => input,
        _ => {
            return Err(WorkerExecutorError::invalid_request(
                "durable streaming request requires an agent method invocation",
            ));
        }
    };
    let session_key = StreamInvocationIdV1 {
        callee_environment_id: environment_id,
        callee: callee.clone(),
        callee_fingerprint,
        idempotency_key: idempotency_key.clone(),
    };
    let attachment_id = AttachmentId::primary(environment_id, &callee, &idempotency_key)
        .map_err(|error| WorkerExecutorError::invalid_request(error.to_string()))?;
    let session_mapping = StreamSessionMappingV1 {
        session_key: session_key.clone(),
        attachment_id,
        role: SessionStreamRoleV1::Input,
    };
    if input_encoded_len > MAX_DURABLE_STREAM_ITEM_SIZE {
        return Err(WorkerExecutorError::invalid_request(
            "ResourceExhausted: durable invocation input exceeds the 16 MiB logical value limit",
        ));
    }
    let mut registrations = Vec::new();
    let mut foreign_mappings = request
        .durable_input_mappings
        .iter()
        .cloned()
        .map(durable_stream_mapping_from_proto)
        .collect::<Result<Vec<_>, _>>()
        .map_err(WorkerExecutorError::invalid_request)?;
    if foreign_mappings
        .iter()
        .any(|mapping| mapping.role != SessionStreamRoleV1::Input)
    {
        return Err(WorkerExecutorError::invalid_request(
            "durable invocation input mapping has a non-input role",
        ));
    }
    let foreign_by_transport = foreign_mappings
        .iter()
        .map(|mapping| (mapping.transport_stream_id, mapping.clone()))
        .collect::<HashMap<_, _>>();
    if foreign_by_transport.len() != foreign_mappings.len() {
        return Err(WorkerExecutorError::invalid_request(
            "durable invocation input contains duplicate transport stream IDs",
        ));
    }
    let mut input_element_types = Vec::new();
    let mut canonical_foreign_mappings = Vec::new();
    let mut canonical_handle_index = 0u64;
    let canonical_input = encode_recursive_stream_value_with_schema(
        invocation_input,
        &agent_type.schema,
        &input_root,
        |stream, path| {
            let transport_stream_id = stream
                .with_host_endpoint::<TransportStreamId, _>(|stream_id| stream_id.0)?;
            let element = stream_element_schema(&agent_type.schema, &input_root, path)?;
            let element_schema_fingerprint = schema_fingerprint_v1(&agent_type.schema, element)
                .map_err(|error| error.to_string())?;
            input_element_types.push((
                transport_stream_id,
                element.cloned().unwrap_or_else(SchemaType::u8),
            ));
            if foreign_mappings.is_empty() {
                registrations.push((
                    transport_stream_id,
                    ProducerRegistrationRequestV1 {
                        coordinate: StreamRegistrationCoordinateV1::Root {
                            invocation_id: session_key.clone(),
                            root_kind: StreamRootKindV1::MethodInput,
                            recursive_value_path: path.to_vec(),
                        },
                        source_kind: StreamSourceKindV1::ExternalInlineInput,
                        source_invocation: session_key.clone(),
                        component_revision,
                        element_schema_fingerprint,
                        session_mapping: Some(session_mapping.clone()),
                    },
                ));
            } else {
                let mapping = foreign_by_transport
                    .get(&transport_stream_id)
                    .ok_or_else(|| {
                        format!(
                            "durable invocation input references unmapped transport stream {transport_stream_id}"
                        )
                    })?
                    .clone();
                if mapping.handle.element_schema_fingerprint != element_schema_fingerprint {
                    return Err(format!(
                        "durable invocation input stream {transport_stream_id} has the wrong schema fingerprint"
                    ));
                }
                canonical_foreign_mappings.push(mapping);
            }

            let index = canonical_handle_index;
            canonical_handle_index = canonical_handle_index
                .checked_add(1)
                .ok_or_else(|| "durable input handle index overflow".to_string())?;
            Ok(index)
        },
    )
    .map_err(WorkerExecutorError::invalid_request)?;
    if !foreign_mappings.is_empty() {
        if canonical_foreign_mappings.len() != foreign_mappings.len() {
            return Err(WorkerExecutorError::invalid_request(
                "durable invocation input contains unreferenced stream mappings",
            ));
        }
        foreign_mappings = canonical_foreign_mappings;
        if foreign_mappings
            .iter()
            .map(|mapping| (mapping.handle.clone(), mapping.role))
            .collect::<HashSet<_>>()
            .len()
            != foreign_mappings.len()
        {
            return Err(WorkerExecutorError::invalid_request(
                "durable invocation input contains duplicate durable stream handles for one role",
            ));
        }
    }
    let expected_handle_count = registrations.len() + foreign_mappings.len();
    if expected_handle_count > MAX_NEW_STREAM_HANDLES_PER_VALUE {
        return Err(WorkerExecutorError::invalid_request(
            "ResourceExhausted: durable invocation input materializes more than 256 streams",
        ));
    }
    if canonical_handle_index != expected_handle_count as u64 {
        return Err(WorkerExecutorError::invalid_request(
            "durable invocation input stream topology changed during canonicalization",
        ));
    }

    let mut execution = request.clone();
    execution.agent_id = None;
    execution.method_name = None;
    execution.input = None;
    execution.idempotency_key = None;
    execution.auth_ctx = None;
    execution.principal = None;
    execution.environment_id = None;
    execution.component_owner_account_id = None;
    execution.attempt_id = None;
    execution.expected_callee_fingerprint = None;
    execution.durable_input_mappings.clear();
    let effective_identity = effective_session_identity(&request.auth_ctx, &request.principal)?;
    let attempt = StartAttemptDescriptorV1 {
        format_version: DURABLE_STREAM_FORMAT_VERSION,
        session_key: session_key.clone(),
        attachment_id,
        expected_callee_fingerprint: callee_fingerprint,
        attempt_id,
        invocation: PersistedStreamInvocationDescriptorV1 {
            format_version: DURABLE_STREAM_FORMAT_VERSION,
            session_key,
            target_component_revision: component_revision,
            method_name,
            invocation_value: canonical_input.encode_to_vec(),
            stream_handles: Vec::new(),
            execution_config: execution.encode_to_vec(),
            effective_identity: effective_identity.clone(),
        },
        effective_identity,
        live_join_buffer_events: u32::try_from(live_join_buffer_events).map_err(|_| {
            WorkerExecutorError::invalid_request("live join buffer capacity does not fit in u32")
        })?,
    };
    Ok(DurableStreamingInvocationRequest {
        attempt,
        registrations,
        foreign_mappings,
        input_schema: Arc::new(agent_type.schema.clone()),
        input_element_types,
        invocation: replace_streams_for_persistence(invocation),
        acceptance_committed,
    })
}

fn resumed_input_schema(
    prepared: &golem_common::model::durable_stream::StreamSessionPreparedRecordV1,
    component_metadata: &golem_common::model::component_metadata::ComponentMetadata,
) -> Result<(SchemaGraph, Vec<(u64, SchemaType)>), WorkerExecutorError> {
    let parsed_agent_id = ParsedAgentId::parse(
        &prepared.attempt.session_key.callee.agent_id,
        component_metadata,
    )
    .map_err(WorkerExecutorError::invalid_request)?;
    let agent_type = component_metadata
        .find_agent_type_by_name_ref(&parsed_agent_id.agent_type)
        .ok_or_else(|| WorkerExecutorError::invalid_request("resume agent type not found"))?;
    let method = agent_type
        .methods
        .iter()
        .find(|method| method.name == prepared.attempt.invocation.method_name)
        .ok_or_else(|| WorkerExecutorError::invalid_request("resume agent method not found"))?;
    let input_root = SchemaType::record(
        method
            .input_schema
            .fields()
            .iter()
            .filter(|field| matches!(field.source, FieldSource::UserSupplied))
            .map(|field| NamedFieldType {
                name: field.name.clone(),
                body: field.schema.clone(),
                metadata: field.metadata.clone(),
            })
            .collect(),
    );
    let canonical_input = golem_api_grpc::proto::golem::schema::SchemaValue::decode(
        prepared.attempt.invocation.invocation_value.as_slice(),
    )
    .map_err(|error| {
        WorkerExecutorError::runtime(format!(
            "failed to decode persisted durable invocation input: {error}"
        ))
    })?;
    let input_mappings = prepared
        .stream_mappings
        .iter()
        .filter(|mapping| mapping.role == SessionStreamRoleV1::Input)
        .collect::<Vec<_>>();
    let mut input_element_types = Vec::with_capacity(input_mappings.len());
    decode_recursive_stream_value_with_schema(
        canonical_input,
        &agent_type.schema,
        &input_root,
        |canonical_stream_id, path| {
            let mapping = input_mappings
                .get(usize::try_from(canonical_stream_id).map_err(|_| {
                    "persisted canonical input stream ID does not fit in usize".to_string()
                })?)
                .ok_or_else(|| {
                    "persisted canonical input references an unmapped stream".to_string()
                })?;
            let element = stream_element_schema(&agent_type.schema, &input_root, path)?;
            input_element_types.push((
                mapping.transport_stream_id,
                element.cloned().unwrap_or_else(SchemaType::u8),
            ));
            Ok(SchemaValueStream::from_host_endpoint(()))
        },
    )
    .map_err(WorkerExecutorError::runtime)?;
    if input_element_types.len() != input_mappings.len() {
        return Err(WorkerExecutorError::runtime(
            "persisted durable invocation input mappings do not match its schema value",
        ));
    }
    Ok((agent_type.schema.clone(), input_element_types))
}

fn build_resume_attempt(
    request: &ResumeAttach,
    live_join_buffer_events: usize,
) -> Result<ResumeAttemptDescriptorV1, WorkerExecutorError> {
    let callee: AgentId = request
        .agent_id
        .clone()
        .ok_or_else(|| WorkerExecutorError::invalid_request("resume agent_id not found"))?
        .try_into()
        .map_err(WorkerExecutorError::invalid_request)?;
    let environment_id = request
        .environment_id
        .ok_or_else(|| WorkerExecutorError::invalid_request("resume environment_id not found"))?
        .try_into()
        .map_err(WorkerExecutorError::invalid_request)?;
    let idempotency_key: IdempotencyKey = request
        .idempotency_key
        .clone()
        .ok_or_else(|| WorkerExecutorError::invalid_request("resume requires an idempotency key"))?
        .into();
    let expected_callee_fingerprint = golem_common::model::AgentFingerprint(
        request
            .expected_callee_fingerprint
            .ok_or_else(|| {
                WorkerExecutorError::invalid_request(
                    "resume requires an expected callee fingerprint",
                )
            })?
            .into(),
    );
    let session_key = StreamInvocationIdV1 {
        callee_environment_id: environment_id,
        callee,
        callee_fingerprint: expected_callee_fingerprint,
        idempotency_key,
    };
    let attachment_id = AttachmentId(
        request
            .attachment_id
            .ok_or_else(|| {
                WorkerExecutorError::invalid_request("resume requires an attachment ID")
            })?
            .into(),
    );
    let attempt_id = AttemptId(
        request
            .attempt_id
            .ok_or_else(|| {
                WorkerExecutorError::invalid_request("resume requires a client attempt ID")
            })?
            .into(),
    );
    if attempt_id.0.is_nil() || attempt_id.0.get_version() != Some(uuid::Version::Random) {
        return Err(WorkerExecutorError::invalid_request(
            "resume attempt ID must be a non-nil UUIDv4",
        ));
    }
    let operation = match request.operation() {
        ResumeOperation::Resume => StreamResumeOperationV1::Resume,
        ResumeOperation::Takeover => StreamResumeOperationV1::Takeover,
        ResumeOperation::Unspecified => {
            return Err(WorkerExecutorError::invalid_request(
                "resume operation is unspecified",
            ));
        }
    };
    let mut cursors = request
        .cursors
        .iter()
        .map(|cursor| {
            let stream_id = golem_common::model::durable_stream::StreamId(
                cursor
                    .stream_id
                    .ok_or_else(|| {
                        WorkerExecutorError::invalid_request("resume cursor has no stream ID")
                    })?
                    .into(),
            );
            let last_observed_offset = cursor
                .last_observed_offset
                .as_ref()
                .map(|offset| {
                    let offset: [u8; 24] = offset.clone().try_into().map_err(|_| {
                        WorkerExecutorError::invalid_request(
                            "resume cursor offset must contain 24 bytes",
                        )
                    })?;
                    golem_common::model::durable_stream::StreamOffsetV1::from_bytes(offset)
                        .map_err(|error| WorkerExecutorError::invalid_request(error.to_string()))
                })
                .transpose()?;
            Ok(StreamResumeCursorV1 {
                stream_id,
                last_observed_offset,
            })
        })
        .collect::<Result<Vec<_>, WorkerExecutorError>>()?;
    cursors.sort_by_key(|cursor| *cursor.stream_id.0.as_bytes());
    if cursors
        .windows(2)
        .any(|pair| pair[0].stream_id == pair[1].stream_id)
    {
        return Err(WorkerExecutorError::invalid_request(
            "resume contains duplicate stream cursors",
        ));
    }
    Ok(ResumeAttemptDescriptorV1 {
        format_version: DURABLE_STREAM_FORMAT_VERSION,
        operation,
        session_key,
        attachment_id,
        expected_callee_fingerprint,
        attempt_id,
        expected_epoch: request.expected_epoch,
        effective_identity: effective_session_identity(&request.auth_ctx, &request.principal)?,
        cursors,
        live_join_buffer_events: u32::try_from(live_join_buffer_events).map_err(|_| {
            WorkerExecutorError::invalid_request("live join buffer capacity does not fit in u32")
        })?,
    })
}

fn effective_session_identity(
    auth_ctx: &Option<golem_api_grpc::proto::golem::auth::AuthCtx>,
    principal: &Option<golem_api_grpc::proto::golem::component::Principal>,
) -> Result<Vec<u8>, WorkerExecutorError> {
    let auth_ctx = auth_ctx
        .as_ref()
        .map(|auth_ctx| {
            use golem_api_grpc::proto::golem::auth::auth_ctx::Value;

            let auth_ctx = golem_service_base::model::auth::AuthCtx::try_from(auth_ctx.clone())
                .map_err(|error| {
                    WorkerExecutorError::invalid_request(format!(
                        "failed converting auth_ctx: {error}"
                    ))
                })?;
            let mut auth_ctx: golem_api_grpc::proto::golem::auth::AuthCtx = auth_ctx.into();
            match auth_ctx.value.as_mut() {
                Some(Value::User(user)) => user.account_email.clear(),
                Some(Value::Agent(agent)) => agent.account_email.clear(),
                Some(Value::AdminImpersonation(admin)) => admin.target_account_email.clear(),
                Some(Value::System(_)) | None => {}
            }
            Ok::<_, WorkerExecutorError>(auth_ctx.encode_to_vec())
        })
        .transpose()?;
    let mut effective_identity = Vec::new();
    for identity in [auth_ctx, principal.as_ref().map(Message::encode_to_vec)]
        .into_iter()
        .flatten()
    {
        effective_identity.extend_from_slice(&(identity.len() as u64).to_be_bytes());
        effective_identity.extend_from_slice(&identity);
    }
    Ok(effective_identity)
}

fn require_expected_callee_fingerprint(
    expected: Option<golem_api_grpc::proto::golem::common::Uuid>,
    actual: golem_common::model::AgentFingerprint,
) -> Result<(), WorkerExecutorError> {
    let expected = expected.ok_or_else(|| {
        WorkerExecutorError::invalid_request(
            "durable streaming invocations require the expected callee fingerprint",
        )
    })?;
    if uuid::Uuid::from(expected) != actual.0 {
        return Err(WorkerExecutorError::invalid_request(
            "expected callee fingerprint does not match the active agent incarnation",
        ));
    }
    Ok(())
}

fn stream_element_schema<'a>(
    graph: &'a SchemaGraph,
    root: &'a SchemaType,
    path: &[StreamValuePathStepV1],
) -> Result<Option<&'a SchemaType>, String> {
    let mut current = root;
    for step in path {
        current = graph
            .resolve_ref(current)
            .map_err(|error| error.to_string())?;
        current = match (step, current) {
            (StreamValuePathStepV1::RecordField(index), SchemaType::Record { fields, .. }) => {
                &fields
                    .get(*index as usize)
                    .ok_or_else(|| "stream record path is out of range".to_string())?
                    .body
            }
            (
                StreamValuePathStepV1::VariantCasePayload(index),
                SchemaType::Variant { cases, .. },
            ) => cases
                .get(*index as usize)
                .and_then(|case| case.payload.as_ref())
                .ok_or_else(|| "stream variant path has no payload".to_string())?,
            (StreamValuePathStepV1::TupleElement(index), SchemaType::Tuple { elements, .. }) => {
                elements
                    .get(*index as usize)
                    .ok_or_else(|| "stream tuple path is out of range".to_string())?
            }
            (StreamValuePathStepV1::ListElement(_), SchemaType::List { element, .. })
            | (StreamValuePathStepV1::FixedListElement(_), SchemaType::FixedList { element, .. }) => {
                element
            }
            (
                StreamValuePathStepV1::MapEntry {
                    side: golem_common::model::durable_stream::StreamMapSideV1::Key,
                    ..
                },
                SchemaType::Map { key, .. },
            ) => key,
            (
                StreamValuePathStepV1::MapEntry {
                    side: golem_common::model::durable_stream::StreamMapSideV1::Value,
                    ..
                },
                SchemaType::Map { value, .. },
            ) => value,
            (StreamValuePathStepV1::OptionSome, SchemaType::Option { inner, .. }) => inner,
            (StreamValuePathStepV1::ResultOk, SchemaType::Result { spec, .. }) => spec
                .ok
                .as_deref()
                .ok_or_else(|| "stream result ok path has no payload".to_string())?,
            (StreamValuePathStepV1::ResultErr, SchemaType::Result { spec, .. }) => spec
                .err
                .as_deref()
                .ok_or_else(|| "stream result error path has no payload".to_string())?,
            (StreamValuePathStepV1::UnionBranch(index), SchemaType::Union { spec, .. }) => {
                &spec
                    .branches
                    .get(*index as usize)
                    .ok_or_else(|| "stream union path is out of range".to_string())?
                    .body
            }
            _ => return Err("stream value path does not match the pinned input schema".to_string()),
        };
    }
    match graph
        .resolve_ref(current)
        .map_err(|error| error.to_string())?
    {
        SchemaType::Stream { inner, .. } => Ok(inner.as_deref()),
        _ => Err("stream reference is not at a stream node in the pinned input schema".to_string()),
    }
}

fn replace_streams_for_persistence(invocation: AgentInvocation) -> AgentInvocation {
    match invocation {
        AgentInvocation::AgentMethod {
            idempotency_key,
            method_name,
            input,
            invocation_context,
            principal,
            scope_card,
        } => AgentInvocation::AgentMethod {
            idempotency_key,
            method_name,
            input: erase_streams(input),
            invocation_context,
            principal,
            scope_card,
        },
        other => other,
    }
}

fn erase_streams(value: SchemaValue) -> SchemaValue {
    match value {
        SchemaValue::Stream(_) => SchemaValue::Tuple {
            elements: Vec::new(),
        },
        SchemaValue::Record { fields } => SchemaValue::Record {
            fields: fields.into_iter().map(erase_streams).collect(),
        },
        SchemaValue::Variant(mut value) => {
            value.payload = value
                .payload
                .map(|payload| Box::new(erase_streams(*payload)));
            SchemaValue::Variant(value)
        }
        SchemaValue::Tuple { elements } => SchemaValue::Tuple {
            elements: elements.into_iter().map(erase_streams).collect(),
        },
        SchemaValue::List { elements } => SchemaValue::List {
            elements: elements.into_iter().map(erase_streams).collect(),
        },
        SchemaValue::FixedList { elements } => SchemaValue::FixedList {
            elements: elements.into_iter().map(erase_streams).collect(),
        },
        SchemaValue::Map { entries } => SchemaValue::Map {
            entries: entries
                .into_iter()
                .map(|(key, value)| (erase_streams(key), erase_streams(value)))
                .collect(),
        },
        SchemaValue::Option { inner } => SchemaValue::Option {
            inner: inner.map(|inner| Box::new(erase_streams(*inner))),
        },
        SchemaValue::Result(mut result) => {
            match &mut result {
                golem_common::schema::schema_value::ResultValuePayload::Ok { value }
                | golem_common::schema::schema_value::ResultValuePayload::Err { value } => {
                    *value = value.take().map(|value| Box::new(erase_streams(*value)));
                }
            }
            SchemaValue::Result(result)
        }
        SchemaValue::Union(mut value) => {
            value.body = Box::new(erase_streams(*value.body));
            SchemaValue::Union(value)
        }
        other => other,
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
        WorkerExecutorError::InvalidRequest { details } => {
            if details.starts_with("IdempotencyConflict:") {
                InvocationRejectionReason::IdempotencyConflict
            } else if details.starts_with("AttemptConflict:") {
                InvocationRejectionReason::AttemptConflict
            } else if details.starts_with("ResourceExhausted:") {
                InvocationRejectionReason::ResourceExhausted
            } else if details.starts_with("StaleEpoch:") {
                InvocationRejectionReason::StaleEpoch
            } else if details.starts_with("InvalidEpoch:") {
                InvocationRejectionReason::InvalidEpoch
            } else if details.starts_with("InvalidAttachmentState:") {
                InvocationRejectionReason::InvalidAttachmentState
            } else if details.starts_with("Unauthorized:") {
                InvocationRejectionReason::Unauthorized
            } else if details.starts_with("NotFound:") {
                InvocationRejectionReason::NotFound
            } else {
                InvocationRejectionReason::Validation
            }
        }
        WorkerExecutorError::ParamTypeMismatch { .. }
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
    tracing::warn!(
        kind = ?kind,
        code,
        message = %message,
        worker_error = ?worker_error,
        "Invocation session finished with failure"
    );
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

async fn route_durable_request(
    streams: &DurableSessionStreams,
    responses: &mpsc::Sender<InvocationResponse>,
    state: &Arc<tokio::sync::Mutex<InvocationSessionState>>,
    request: InvocationRequest,
) -> Result<(), String> {
    state
        .lock()
        .await
        .validate_received_trusted_request(&request)?;
    let request = request
        .request
        .expect("validated invocation request has a payload");
    let ack = match request {
        invocation_request::Request::InputItem(item) => {
            let handle = streams
                .validate_frame(
                    item.transport_stream_id,
                    item.durable_stream_id,
                    item.epoch,
                    SessionStreamRoleV1::Input,
                )
                .await?;
            let payload = match item.payload {
                Some(golem_api_grpc::proto::golem::worker::input_stream_item::Payload::Value(
                    value,
                )) => StreamItemsPayloadV1::Values(vec![value.encode_to_vec()]),
                Some(
                    golem_api_grpc::proto::golem::worker::input_stream_item::Payload::PackedU8(
                        bytes,
                    ),
                ) => StreamItemsPayloadV1::PackedU8(bytes),
                None => return Err("durable input item has no payload".to_string()),
            };
            streams
                .write_input(item.transport_stream_id, item.sequence, payload)
                .await
                .map(|outcome| {
                    outcome.map(
                        |(
                            highest_contiguous_sequence,
                            logical_item_count,
                            resulting_offset,
                            new_stream_mappings,
                        )| InputStreamAck {
                            transport_stream_id: item.transport_stream_id,
                            highest_contiguous_sequence,
                            logical_item_count,
                            durable_stream_id: Some(handle.stream_id.0.into()),
                            resulting_offset: resulting_offset.0.to_vec(),
                            epoch: streams.attachment_epoch(),
                            new_stream_mappings: new_stream_mappings
                                .iter()
                                .map(|mapping| durable_stream_mapping_to_proto(mapping, None))
                                .collect(),
                        },
                    )
                })?
        }
        invocation_request::Request::InputEnd(end) => {
            let handle = streams
                .validate_frame(
                    end.transport_stream_id,
                    end.durable_stream_id,
                    end.epoch,
                    SessionStreamRoleV1::Input,
                )
                .await?;
            let resulting_offset = streams
                .end_input(end.transport_stream_id, end.sequence)
                .await?;
            resulting_offset.map(|resulting_offset| InputStreamAck {
                transport_stream_id: end.transport_stream_id,
                highest_contiguous_sequence: end.sequence,
                logical_item_count: 1,
                durable_stream_id: Some(handle.stream_id.0.into()),
                resulting_offset: resulting_offset.0.to_vec(),
                epoch: streams.attachment_epoch(),
                new_stream_mappings: Vec::new(),
            })
        }
        invocation_request::Request::StreamCancel(cancel) => {
            let (role, stream_role) = match cancel.role() {
                golem_api_grpc::proto::golem::worker::StreamCancelRole::InputProducer => (
                    StreamCancelRoleV1::InputProducer,
                    SessionStreamRoleV1::Input,
                ),
                golem_api_grpc::proto::golem::worker::StreamCancelRole::InputConsumer => (
                    StreamCancelRoleV1::InputConsumer,
                    SessionStreamRoleV1::Input,
                ),
                golem_api_grpc::proto::golem::worker::StreamCancelRole::OutputProducer => (
                    StreamCancelRoleV1::OutputProducer,
                    SessionStreamRoleV1::Output,
                ),
                golem_api_grpc::proto::golem::worker::StreamCancelRole::OutputConsumer => (
                    StreamCancelRoleV1::OutputConsumer,
                    SessionStreamRoleV1::Output,
                ),
                golem_api_grpc::proto::golem::worker::StreamCancelRole::System => {
                    return Err(
                        "system-authored durable stream cancellation is internal".to_string()
                    );
                }
                golem_api_grpc::proto::golem::worker::StreamCancelRole::Unspecified => {
                    return Err("durable stream cancel role is unspecified".to_string());
                }
            };
            streams
                .validate_frame(
                    cancel.transport_stream_id,
                    cancel.durable_stream_id,
                    cancel.epoch,
                    stream_role,
                )
                .await?;
            let reason = match cancel.reason() {
                golem_api_grpc::proto::golem::worker::StreamCancelReason::Cancelled => {
                    StreamCancelReasonV1::Cancelled
                }
                golem_api_grpc::proto::golem::worker::StreamCancelReason::Protocol => {
                    StreamCancelReasonV1::Protocol
                }
                golem_api_grpc::proto::golem::worker::StreamCancelReason::ConsumerDrop => {
                    StreamCancelReasonV1::GuestDrop
                }
                golem_api_grpc::proto::golem::worker::StreamCancelReason::Transport => {
                    return Err(
                        "transport loss detaches a durable session and cannot cancel a stream"
                            .to_string(),
                    );
                }
                golem_api_grpc::proto::golem::worker::StreamCancelReason::SourceUnavailable
                    if role == StreamCancelRoleV1::InputProducer =>
                {
                    StreamCancelReasonV1::SourceUnavailable
                }
                golem_api_grpc::proto::golem::worker::StreamCancelReason::SourceUnavailable
                | golem_api_grpc::proto::golem::worker::StreamCancelReason::ProducerDeleting
                | golem_api_grpc::proto::golem::worker::StreamCancelReason::InvocationFailed => {
                    return Err(
                        "system-authored durable stream cancellation reason is internal"
                            .to_string(),
                    );
                }
                golem_api_grpc::proto::golem::worker::StreamCancelReason::Unspecified => {
                    return Err("durable stream cancel reason is unspecified".to_string());
                }
            };
            streams
                .cancel_stream(
                    cancel.transport_stream_id,
                    role,
                    reason,
                    cancel.details,
                    Some(cancel.epoch),
                )
                .await?;
            None
        }
        invocation_request::Request::Start(_) | invocation_request::Request::ResumeAttach(_) => {
            return Err("unexpected message on durable invocation request stream".to_string());
        }
    };
    if let Some(ack) = ack {
        responses
            .send(InvocationResponse {
                response: Some(invocation_response::Response::InputAck(ack)),
            })
            .await
            .map_err(|_| "invocation response stream closed".to_string())?;
    }
    Ok(())
}

#[cfg(test)]
mod freshness_tests {
    use super::{
        AcceptanceRace, TransportStreamId, build_durable_streaming_request,
        decode_invocation_freshness_disposition, decode_invocation_input,
        effective_session_identity, is_attachment_termination, pre_acceptance_rejection_reason,
        race_invocation_acceptance, require_expected_callee_fingerprint,
    };
    use crate::durable_host::durable_session::durable_stream_mapping_to_proto;
    use crate::durable_host::stream_session::{
        decode_recursive_stream_value, decode_recursive_stream_value_with_schema,
        encode_recursive_stream_value_with_schema, remap_recursive_stream_references,
    };
    use futures::future;
    use golem_api_grpc::proto::golem::account::PlanId;
    use golem_api_grpc::proto::golem::auth::{
        AuthCtx, AuthEffectiveSurface, UserAuthCtx, auth_ctx,
    };
    use golem_api_grpc::proto::golem::common::{AccountId, Uuid};
    use golem_api_grpc::proto::golem::schema::{
        BinaryValue, SchemaValue as ProtoSchemaValue, schema_value as proto_schema_value,
    };
    use golem_api_grpc::proto::golem::worker::{InvocationRejectionReason, InvocationStart};
    use golem_common::base_model::Empty;
    use golem_common::base_model::agent::Snapshotting;
    use golem_common::base_model::component::{ComponentId, ComponentRevision};
    use golem_common::base_model::component_metadata::{ComponentMetadata, KnownExports};
    use golem_common::base_model::durable_stream::{
        DURABLE_STREAM_FORMAT_VERSION, DurableStreamHandleV1, MAX_DURABLE_STREAM_ITEM_SIZE,
        SessionStreamRoleV1, StreamId, StreamInvocationIdV1, StreamMapSideV1,
        StreamSessionMappingRecordV1, StreamValuePathStepV1,
    };
    use golem_common::base_model::environment::EnvironmentId;
    use golem_common::model::agent::{
        AgentMode, AgentTypeName, InvocationFreshnessDisposition, Principal,
    };
    use golem_common::model::invocation_context::InvocationContextStack;
    use golem_common::model::{AgentFingerprint, AgentId, AgentInvocation, IdempotencyKey};
    use golem_common::schema::SchemaValue;
    use golem_common::schema::agent::{
        AgentConstructorSchema, AgentMethodSchema, AgentTypeSchema, InputSchema, NamedField,
        OutputSchema,
    };
    use golem_schema::schema::schema_value::{ResultValuePayload, UnionValuePayload};
    use golem_schema::schema::{
        DiscriminatorRule, FieldDiscriminator, NamedFieldType, ResultSpec, SchemaFingerprintV1,
        SchemaGraph, SchemaType, SchemaTypeDef, SchemaValueStream, TypeId, UnionBranch, UnionSpec,
        VariantCaseType, schema_fingerprint_v1,
    };
    use golem_service_base::error::worker_executor::WorkerExecutorError;
    use prost::Message;
    use std::collections::BTreeMap;
    use std::task::Poll;
    use test_r::test;

    #[test]
    fn invocation_input_decode_moves_binary_payload() {
        let bytes = vec![7; 1024];
        let bytes_ptr = bytes.as_ptr();
        let input = ProtoSchemaValue {
            value: Some(proto_schema_value::Value::BinaryValue(BinaryValue {
                bytes,
                mime_type: None,
            })),
        };

        let decoded = decode_invocation_input(input).unwrap();
        let SchemaValue::Binary(binary) = decoded else {
            panic!("expected binary input")
        };

        assert_eq!(binary.bytes.as_ptr(), bytes_ptr);
    }

    #[test]
    fn one_pass_input_canonicalization_matches_two_pass_traversal() {
        fn field(name: &str, body: SchemaType) -> NamedFieldType {
            NamedFieldType {
                name: name.to_string(),
                body,
                metadata: Default::default(),
            }
        }

        let map_type_id = TypeId::new("stream-map");
        let union = SchemaType::union(UnionSpec {
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
                        field("values", SchemaType::stream(Some(SchemaType::u64()))),
                    ]),
                    discriminator: DiscriminatorRule::FieldEquals(FieldDiscriminator {
                        field_name: "kind".to_string(),
                        literal: Some("stream".to_string()),
                    }),
                    metadata: Default::default(),
                },
            ],
        });
        let root = SchemaType::record(vec![
            field("mapping", SchemaType::ref_to(map_type_id.clone())),
            field(
                "variant",
                SchemaType::variant(vec![
                    VariantCaseType {
                        name: "none".to_string(),
                        payload: None,
                        metadata: Default::default(),
                    },
                    VariantCaseType {
                        name: "some".to_string(),
                        payload: Some(SchemaType::option(SchemaType::stream(Some(
                            SchemaType::string(),
                        )))),
                        metadata: Default::default(),
                    },
                ]),
            ),
            field(
                "result",
                SchemaType::result(ResultSpec {
                    ok: Some(Box::new(SchemaType::u64())),
                    err: Some(Box::new(SchemaType::stream(Some(SchemaType::string())))),
                }),
            ),
            field("union", union),
        ]);
        let graph = SchemaGraph {
            defs: vec![SchemaTypeDef {
                id: map_type_id,
                name: None,
                body: SchemaType::map(
                    SchemaType::stream(Some(SchemaType::u64())),
                    SchemaType::stream(Some(SchemaType::string())),
                ),
            }],
            root: root.clone(),
        };
        let input = SchemaValue::Record {
            fields: vec![
                SchemaValue::Map {
                    entries: vec![(
                        SchemaValue::Stream(SchemaValueStream::from_host_endpoint(101_u64)),
                        SchemaValue::Stream(SchemaValueStream::from_host_endpoint(7_u64)),
                    )],
                },
                SchemaValue::Variant(golem_schema::schema::schema_value::VariantValuePayload {
                    case: 1,
                    payload: Some(Box::new(SchemaValue::Option {
                        inner: Some(Box::new(SchemaValue::Stream(
                            SchemaValueStream::from_host_endpoint(55_u64),
                        ))),
                    })),
                }),
                SchemaValue::Result(ResultValuePayload::Err {
                    value: Some(Box::new(SchemaValue::Stream(
                        SchemaValueStream::from_host_endpoint(300_u64),
                    ))),
                }),
                SchemaValue::Union(UnionValuePayload {
                    tag: "stream".to_string(),
                    body: Box::new(SchemaValue::Record {
                        fields: vec![
                            SchemaValue::String("stream".to_string()),
                            SchemaValue::Stream(SchemaValueStream::from_host_endpoint(2_u64)),
                        ],
                    }),
                }),
            ],
        };
        let original =
            encode_recursive_stream_value_with_schema(&input, &graph, &root, |stream, _| {
                stream.with_host_endpoint::<u64, _>(|stream_id| *stream_id)
            })
            .unwrap();

        let mut old_observations = Vec::new();
        decode_recursive_stream_value_with_schema(
            original.clone(),
            &graph,
            &root,
            |stream_id, path| {
                old_observations.push((stream_id, path.to_vec()));
                Ok(SchemaValueStream::from_host_endpoint(()))
            },
        )
        .unwrap();
        let mut old_index = 0_u64;
        let old_canonical = remap_recursive_stream_references(original.clone(), |_, _| {
            let index = old_index;
            old_index += 1;
            Ok(index)
        })
        .unwrap();

        let decoded = decode_invocation_input(original).unwrap();
        let canonicalize = |decoded: &SchemaValue| {
            let mut observations = Vec::new();
            let mut index = 0_u64;
            let canonical = encode_recursive_stream_value_with_schema(
                decoded,
                &graph,
                &root,
                |stream, path| {
                    let stream_id = stream
                        .with_host_endpoint::<TransportStreamId, _>(|stream_id| stream_id.0)?;
                    observations.push((stream_id, path.to_vec()));
                    let canonical = index;
                    index += 1;
                    Ok(canonical)
                },
            )
            .unwrap();
            (canonical, observations)
        };
        let (new_canonical, new_observations) = canonicalize(&decoded);

        assert_eq!(new_canonical, old_canonical);
        assert_eq!(new_observations, old_observations);
        assert_eq!(
            new_observations,
            vec![
                (
                    101,
                    vec![
                        StreamValuePathStepV1::RecordField(0),
                        StreamValuePathStepV1::MapEntry {
                            index: 0,
                            side: StreamMapSideV1::Key,
                        },
                    ],
                ),
                (
                    7,
                    vec![
                        StreamValuePathStepV1::RecordField(0),
                        StreamValuePathStepV1::MapEntry {
                            index: 0,
                            side: StreamMapSideV1::Value,
                        },
                    ],
                ),
                (
                    55,
                    vec![
                        StreamValuePathStepV1::RecordField(1),
                        StreamValuePathStepV1::VariantCasePayload(1),
                        StreamValuePathStepV1::OptionSome,
                    ],
                ),
                (
                    300,
                    vec![
                        StreamValuePathStepV1::RecordField(2),
                        StreamValuePathStepV1::ResultErr,
                    ],
                ),
                (
                    2,
                    vec![
                        StreamValuePathStepV1::RecordField(3),
                        StreamValuePathStepV1::UnionBranch(1),
                        StreamValuePathStepV1::RecordField(1),
                    ],
                ),
            ]
        );
        assert_eq!(canonicalize(&decoded).0, old_canonical);
    }

    fn builder_fixture() -> (InvocationStart, ComponentMetadata, AgentInvocation) {
        let mapping_type = SchemaType::map(
            SchemaType::stream(Some(SchemaType::u64())),
            SchemaType::stream(Some(SchemaType::string())),
        );
        let tail_type = SchemaType::stream(Some(SchemaType::bool()));
        let method = AgentMethodSchema {
            name: "run".to_string(),
            description: String::new(),
            prompt_hint: None,
            input_schema: InputSchema::Parameters(vec![
                NamedField::user_supplied("mapping", mapping_type),
                NamedField::user_supplied("tail", tail_type),
            ]),
            output_schema: OutputSchema::Unit,
            http_endpoint: Vec::new(),
            read_only: None,
        };
        let metadata = ComponentMetadata::from_parts(
            KnownExports::default(),
            Vec::new(),
            None,
            None,
            vec![AgentTypeSchema {
                type_name: AgentTypeName("test-agent".to_string()),
                description: String::new(),
                source_language: String::new(),
                schema: SchemaGraph::empty(),
                constructor: AgentConstructorSchema {
                    name: None,
                    description: String::new(),
                    prompt_hint: None,
                    input_schema: InputSchema::Parameters(Vec::new()),
                },
                methods: vec![method],
                dependencies: Vec::new(),
                mode: AgentMode::Durable,
                http_mount: None,
                snapshotting: Snapshotting::Disabled(Empty {}),
                config: Vec::new(),
            }],
            BTreeMap::new(),
        );
        let component_id = ComponentId(uuid::Uuid::from_u128(1));
        let environment_id = EnvironmentId(uuid::Uuid::from_u128(2));
        let callee = AgentId {
            component_id,
            agent_id: "test-agent()".to_string(),
        };
        let idempotency_key = IdempotencyKey::new("test-key".to_string());
        let fingerprint = AgentFingerprint(uuid::Uuid::from_u128(3));
        let request = InvocationStart {
            agent_id: Some(callee.into()),
            method_name: Some("run".to_string()),
            idempotency_key: Some(idempotency_key.clone().into()),
            environment_id: Some(environment_id.into()),
            attempt_id: Some(uuid::Uuid::new_v4().into()),
            expected_callee_fingerprint: Some(fingerprint.0.into()),
            ..Default::default()
        };
        let invocation = AgentInvocation::AgentMethod {
            idempotency_key,
            method_name: "run".to_string(),
            input: SchemaValue::Record {
                fields: vec![
                    SchemaValue::Map {
                        entries: vec![(
                            SchemaValue::Stream(SchemaValueStream::from_host_endpoint(
                                TransportStreamId(101),
                            )),
                            SchemaValue::Stream(SchemaValueStream::from_host_endpoint(
                                TransportStreamId(7),
                            )),
                        )],
                    },
                    SchemaValue::Stream(SchemaValueStream::from_host_endpoint(TransportStreamId(
                        55,
                    ))),
                ],
            },
            invocation_context: InvocationContextStack::fresh(),
            principal: Principal::anonymous(),
            scope_card: None,
        };
        (request, metadata, invocation)
    }

    fn foreign_mapping(
        transport_stream_id: u64,
        element_schema_fingerprint: SchemaFingerprintV1,
        request: &InvocationStart,
    ) -> StreamSessionMappingRecordV1 {
        let environment_id: EnvironmentId = request.environment_id.unwrap().try_into().unwrap();
        let callee: AgentId = request.agent_id.clone().unwrap().try_into().unwrap();
        let callee_fingerprint =
            AgentFingerprint(request.expected_callee_fingerprint.unwrap().into());
        let idempotency_key = request.idempotency_key.clone().unwrap().into();
        StreamSessionMappingRecordV1 {
            transport_stream_id,
            handle: DurableStreamHandleV1 {
                format_version: DURABLE_STREAM_FORMAT_VERSION,
                stream_id: StreamId(uuid::Uuid::from_u128(
                    1_000 + u128::from(transport_stream_id),
                )),
                producer_environment_id: environment_id,
                producer: callee.clone(),
                expected_producer_fingerprint: callee_fingerprint,
                source_invocation: StreamInvocationIdV1 {
                    callee_environment_id: environment_id,
                    callee,
                    callee_fingerprint,
                    idempotency_key,
                },
                component_revision: ComponentRevision::INITIAL,
                element_schema_fingerprint,
            },
            role: SessionStreamRoleV1::Input,
        }
    }

    #[test]
    fn durable_request_builder_preserves_size_precedence_and_stream_metadata_order() {
        let (mut request, metadata, invocation) = builder_fixture();
        request.durable_input_mappings.push(Default::default());
        let (acceptance_committed, _) = tokio::sync::oneshot::channel();
        let error = match build_durable_streaming_request(
            &request,
            &metadata,
            ComponentRevision::INITIAL,
            AgentFingerprint(uuid::Uuid::from_u128(3)),
            invocation,
            MAX_DURABLE_STREAM_ITEM_SIZE + 1,
            acceptance_committed,
            8,
        ) {
            Ok(_) => panic!("oversized input must be rejected"),
            Err(error) => error,
        };
        assert!(
            error
                .to_string()
                .contains("input exceeds the 16 MiB logical value limit")
        );

        let (request, metadata, invocation) = builder_fixture();
        let (acceptance_committed, _) = tokio::sync::oneshot::channel();
        let built = build_durable_streaming_request(
            &request,
            &metadata,
            ComponentRevision::INITIAL,
            AgentFingerprint(uuid::Uuid::from_u128(3)),
            invocation,
            1,
            acceptance_committed,
            8,
        )
        .unwrap();
        let expected_paths = [
            vec![
                StreamValuePathStepV1::RecordField(0),
                StreamValuePathStepV1::MapEntry {
                    index: 0,
                    side: StreamMapSideV1::Key,
                },
            ],
            vec![
                StreamValuePathStepV1::RecordField(0),
                StreamValuePathStepV1::MapEntry {
                    index: 0,
                    side: StreamMapSideV1::Value,
                },
            ],
            vec![StreamValuePathStepV1::RecordField(1)],
        ];
        assert_eq!(
            built
                .registrations
                .iter()
                .map(|(transport_stream_id, registration)| {
                    let path = match &registration.coordinate {
                        super::StreamRegistrationCoordinateV1::Root {
                            recursive_value_path,
                            ..
                        } => recursive_value_path.clone(),
                        _ => panic!("expected a root registration"),
                    };
                    (
                        *transport_stream_id,
                        path,
                        registration.element_schema_fingerprint,
                    )
                })
                .collect::<Vec<_>>(),
            vec![
                (
                    101,
                    expected_paths[0].clone(),
                    schema_fingerprint_v1(&SchemaGraph::empty(), Some(&SchemaType::u64())).unwrap(),
                ),
                (
                    7,
                    expected_paths[1].clone(),
                    schema_fingerprint_v1(&SchemaGraph::empty(), Some(&SchemaType::string()))
                        .unwrap(),
                ),
                (
                    55,
                    expected_paths[2].clone(),
                    schema_fingerprint_v1(&SchemaGraph::empty(), Some(&SchemaType::bool()))
                        .unwrap(),
                ),
            ]
        );
        assert_eq!(
            built.input_element_types,
            vec![
                (101, SchemaType::u64()),
                (7, SchemaType::string()),
                (55, SchemaType::bool()),
            ]
        );
        let canonical =
            ProtoSchemaValue::decode(built.attempt.invocation.invocation_value.as_slice()).unwrap();
        let mut canonical_ids = Vec::new();
        decode_recursive_stream_value(canonical, |stream_id, _| {
            canonical_ids.push(stream_id);
            Ok(SchemaValueStream::from_host_endpoint(()))
        })
        .unwrap();
        assert_eq!(canonical_ids, vec![0, 1, 2]);

        let (mut request, metadata, invocation) = builder_fixture();
        let fingerprints = [
            schema_fingerprint_v1(&SchemaGraph::empty(), Some(&SchemaType::u64())).unwrap(),
            schema_fingerprint_v1(&SchemaGraph::empty(), Some(&SchemaType::string())).unwrap(),
            schema_fingerprint_v1(&SchemaGraph::empty(), Some(&SchemaType::bool())).unwrap(),
        ];
        let mappings = [
            foreign_mapping(55, fingerprints[2], &request),
            foreign_mapping(101, fingerprints[0], &request),
            foreign_mapping(7, fingerprints[1], &request),
        ];
        request.durable_input_mappings = mappings
            .iter()
            .map(|mapping| durable_stream_mapping_to_proto(mapping, None))
            .collect();
        let (acceptance_committed, _) = tokio::sync::oneshot::channel();
        let built = build_durable_streaming_request(
            &request,
            &metadata,
            ComponentRevision::INITIAL,
            AgentFingerprint(uuid::Uuid::from_u128(3)),
            invocation,
            1,
            acceptance_committed,
            8,
        )
        .unwrap();

        assert!(built.registrations.is_empty());
        assert_eq!(
            built
                .foreign_mappings
                .iter()
                .map(|mapping| mapping.transport_stream_id)
                .collect::<Vec<_>>(),
            vec![101, 7, 55]
        );
        assert_eq!(
            built.input_element_types,
            vec![
                (101, SchemaType::u64()),
                (7, SchemaType::string()),
                (55, SchemaType::bool()),
            ]
        );
    }

    #[test]
    fn output_pump_distinguishes_attachment_termination_from_protocol_failure() {
        assert!(is_attachment_termination(
            "invocation response stream closed"
        ));
        assert!(is_attachment_termination("StaleEpoch: attachment fenced"));
        assert!(!is_attachment_termination(
            "durable output journal is corrupt"
        ));
    }

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

    #[test]
    fn durable_streaming_requires_matching_callee_fingerprint() {
        let actual = AgentFingerprint::new();
        let missing = require_expected_callee_fingerprint(None, actual).unwrap_err();
        assert!(
            missing
                .to_string()
                .contains("require the expected callee fingerprint")
        );

        let mismatch =
            require_expected_callee_fingerprint(Some(AgentFingerprint::new().0.into()), actual)
                .unwrap_err();
        assert!(mismatch.to_string().contains("does not match"));

        require_expected_callee_fingerprint(Some(actual.0.into()), actual).unwrap();
    }

    #[test]
    fn durable_stream_identity_ignores_non_authoritative_account_email() {
        let auth_ctx = |account_email: &str| {
            Some(AuthCtx {
                value: Some(auth_ctx::Value::User(UserAuthCtx {
                    account_id: Some(AccountId {
                        value: Some(Uuid {
                            high_bits: 1,
                            low_bits: 2,
                        }),
                    }),
                    plan_id: Some(PlanId {
                        value: Some(Uuid {
                            high_bits: 3,
                            low_bits: 4,
                        }),
                    }),
                    effective_surface: Some(AuthEffectiveSurface::default()),
                    account_email: account_email.to_string(),
                    ..Default::default()
                })),
            })
        };

        assert_eq!(
            effective_session_identity(&auth_ctx("old@example.com"), &None).unwrap(),
            effective_session_identity(&auth_ctx("new@example.com"), &None).unwrap(),
            "resume identity must pin the effective principal/grant, not mutable account metadata"
        );
    }

    #[test]
    fn durable_stream_identity_ignores_account_role_wire_order() {
        let auth_ctx = |account_roles| {
            Some(AuthCtx {
                value: Some(auth_ctx::Value::User(UserAuthCtx {
                    account_id: Some(AccountId {
                        value: Some(Uuid {
                            high_bits: 1,
                            low_bits: 2,
                        }),
                    }),
                    plan_id: Some(PlanId {
                        value: Some(Uuid {
                            high_bits: 3,
                            low_bits: 4,
                        }),
                    }),
                    account_roles,
                    effective_surface: Some(AuthEffectiveSurface::default()),
                    ..Default::default()
                })),
            })
        };
        let admin = golem_api_grpc::proto::golem::auth::AccountRole::Admin as i32;
        let marketing = golem_api_grpc::proto::golem::auth::AccountRole::MarketingAdmin as i32;
        let ordered = auth_ctx(vec![admin, marketing]);
        let reversed = auth_ctx(vec![marketing, admin]);
        let ordered_auth: golem_service_base::model::auth::AuthCtx =
            ordered.clone().unwrap().try_into().unwrap();
        let reversed_auth: golem_service_base::model::auth::AuthCtx =
            reversed.clone().unwrap().try_into().unwrap();
        assert_eq!(ordered_auth, reversed_auth);

        assert_eq!(
            effective_session_identity(&ordered, &None).unwrap(),
            effective_session_identity(&reversed, &None).unwrap(),
            "account roles are a set after auth validation, so protobuf ordering must not change the pinned resume identity"
        );
    }

    #[test]
    fn durable_start_failures_use_their_explicit_protocol_reasons() {
        for (details, expected) in [
            (
                "IdempotencyConflict: descriptor changed",
                InvocationRejectionReason::IdempotencyConflict,
            ),
            (
                "AttemptConflict: attempt changed",
                InvocationRejectionReason::AttemptConflict,
            ),
            (
                "ResourceExhausted: too many streams",
                InvocationRejectionReason::ResourceExhausted,
            ),
            (
                "StaleEpoch: old transport",
                InvocationRejectionReason::StaleEpoch,
            ),
            (
                "InvalidEpoch: future epoch",
                InvocationRejectionReason::InvalidEpoch,
            ),
            (
                "InvalidAttachmentState: already attached",
                InvocationRejectionReason::InvalidAttachmentState,
            ),
            (
                "Unauthorized: principal changed",
                InvocationRejectionReason::Unauthorized,
            ),
            (
                "NotFound: durable session is absent",
                InvocationRejectionReason::NotFound,
            ),
        ] {
            assert_eq!(
                pre_acceptance_rejection_reason(&WorkerExecutorError::invalid_request(details)),
                expected
            );
        }
    }

    #[test]
    async fn inflight_committed_acceptance_wins_after_inbound_becomes_ready() {
        let (acceptance_committed_tx, mut acceptance_committed_rx) =
            tokio::sync::oneshot::channel();
        let (accepted_tx, mut accepted_rx) = tokio::sync::oneshot::channel();
        let (commit_started_tx, commit_started_rx) = tokio::sync::oneshot::channel();
        let (commit_finished_tx, commit_finished_rx) = tokio::sync::oneshot::channel();
        let mut commit_started_tx = Some(commit_started_tx);
        let actor = tokio::spawn(async move {
            commit_started_rx.await.unwrap();
            acceptance_committed_tx.send(()).unwrap();
            commit_finished_tx.send(()).unwrap();
        });
        let invocation = async move {
            commit_finished_rx.await.unwrap();
            accepted_tx.send(42_u64).unwrap();
            future::pending::<()>().await
        };
        let inbound = future::poll_fn(move |_| {
            commit_started_tx.take().unwrap().send(()).unwrap();
            Poll::Ready("transport closed")
        });
        tokio::pin!(invocation);

        let race = race_invocation_acceptance(
            &mut accepted_rx,
            &mut acceptance_committed_rx,
            invocation.as_mut(),
            inbound,
        )
        .await;
        actor.await.unwrap();

        assert!(matches!(
            race,
            AcceptanceRace::Accepted {
                acceptance: 42,
                early_output: None,
                early_inbound: Some("transport closed"),
            }
        ));
    }

    #[test]
    async fn dropped_acceptance_sender_preserves_invocation_output() {
        let (acceptance_committed_tx, mut acceptance_committed_rx) =
            tokio::sync::oneshot::channel();
        let (accepted_tx, mut accepted_rx) = tokio::sync::oneshot::channel::<()>();
        let invocation = async move {
            drop(accepted_tx);
            tokio::task::yield_now().await;
            drop(acceptance_committed_tx);
            "invocation error"
        };
        tokio::pin!(invocation);

        let race = race_invocation_acceptance(
            &mut accepted_rx,
            &mut acceptance_committed_rx,
            invocation.as_mut(),
            future::pending::<()>(),
        )
        .await;

        assert!(matches!(
            race,
            AcceptanceRace::InvocationFinished("invocation error")
        ));
    }

    #[test]
    async fn dropped_acceptance_sender_does_not_repoll_cached_invocation_output() {
        let (acceptance_committed_tx, mut acceptance_committed_rx) =
            tokio::sync::oneshot::channel();
        let (accepted_tx, mut accepted_rx) = tokio::sync::oneshot::channel::<()>();
        let actor = tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            drop(accepted_tx);
        });
        let invocation = async move {
            acceptance_committed_tx.send(()).unwrap();
            "invocation output"
        };
        tokio::pin!(invocation);

        let race = race_invocation_acceptance(
            &mut accepted_rx,
            &mut acceptance_committed_rx,
            invocation.as_mut(),
            future::pending::<()>(),
        )
        .await;
        actor.await.unwrap();

        assert!(matches!(
            race,
            AcceptanceRace::InvocationFinished("invocation output")
        ));
    }
}
