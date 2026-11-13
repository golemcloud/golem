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
    AcceptedInvocation, AccountId, AgentId, AgentInvocation, AgentInvocationOutput,
    AgentInvocationResult, AgentMode, ComponentRevision, DateTime, Future, HasAll,
    HasComponentService, InvocationFreshnessDisposition, InvocationStart, InvocationStatus,
    ParsedAgentKey, Pin, Principal, SchemaValue, ScopeCard, UsesAllDeps, Utc, Worker, WorkerCtx,
    WorkerExecutorError, WorkerExecutorImpl, build_durable_streaming_request,
    extract_owned_agent_id, from_proto_invocation_context, golem, validate_agent_method_invocation,
};
use golem_common::model::InvocationRequestKey;

impl<Ctx: WorkerCtx, Svcs: HasAll<Ctx> + UsesAllDeps<Ctx = Ctx> + Send + Sync + 'static>
    WorkerExecutorImpl<Ctx, Svcs>
{
    pub(super) async fn invoke_agent_internal(
        &self,
        request: &InvocationStart,
        method_parameters: Option<SchemaValue>,
        input_encoded_len: Option<usize>,
        acceptance_committed: tokio::sync::oneshot::Sender<()>,
        accepted: tokio::sync::oneshot::Sender<AcceptedInvocation>,
        scope: &crate::worker::tasks::TaskScope,
    ) -> Result<AgentInvocationOutput, WorkerExecutorError> {
        Self::validate_auth_ctx(&request.auth_ctx)?;

        if request.external_tool.is_some() {
            return self
                .invoke_external_tool_internal(request, acceptance_committed, accepted)
                .await;
        }

        let freshness_disposition =
            decode_invocation_freshness_disposition(request.freshness_disposition);

        // An expected fingerprint names one existing incarnation. Every mode acquires the agent
        // through the existing-only guard, so a missing agent or another incarnation fails as
        // not found before anything is created or enqueued.
        let expected_fingerprint = request
            .expected_callee_fingerprint
            .map(|fingerprint| golem_common::model::AgentFingerprint(fingerprint.into()));
        if expected_fingerprint.is_some()
            && freshness_disposition == InvocationFreshnessDisposition::KnownFresh
        {
            return Err(WorkerExecutorError::invalid_request(
                "KnownFresh cannot be used with an expected agent fingerprint",
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
            let ik = request
                .idempotency_key
                .clone()
                .ok_or_else(|| {
                    WorkerExecutorError::invalid_request("lookup requires an invocation identity")
                })?
                .value
                .parse::<golem_common::model::InvocationId>()
                .map_err(|error| {
                    WorkerExecutorError::invalid_request(format!(
                        "invalid invocation identity: {error}"
                    ))
                })?;
            if freshness_disposition == InvocationFreshnessDisposition::KnownFresh {
                return Err(WorkerExecutorError::invalid_request(
                    "KnownFresh cannot be used for an invocation lookup",
                ));
            }
            let inv_status = match self
                .get_or_create_pending_for_lookup(request, expected_fingerprint)
                .await?
            {
                Some((worker, _response_lease)) => {
                    let bound = scope
                        .bind(&worker.tasks)
                        .map_err(WorkerExecutorError::runtime);
                    worker
                        .deleted_incarnation_as_not_found(expected_fingerprint, bound)
                        .await?;
                    match worker.lookup_invocation_result(&ik).await {
                        crate::model::LookupResult::Complete(Ok(_)) => InvocationStatus::Complete,
                        crate::model::LookupResult::Complete(Err(err)) => return Err(err),
                        crate::model::LookupResult::Pending => InvocationStatus::Pending,
                        crate::model::LookupResult::New
                        | crate::model::LookupResult::Interrupted => InvocationStatus::Unknown,
                    }
                }
                None => InvocationStatus::Unknown,
            };
            publish_acceptance(acceptance_committed, accepted, &ik, None)?;
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

        let request_key: InvocationRequestKey = request
            .request_key
            .clone()
            .ok_or_else(|| WorkerExecutorError::invalid_request("request_key is required"))?
            .try_into()
            .map_err(WorkerExecutorError::invalid_request)?;
        let owner_key = request_key.owner_key(&method_name);

        let schedule_at: Option<DateTime<Utc>> = request
            .schedule_at
            .as_ref()
            .and_then(|ts| DateTime::from_timestamp(ts.seconds, ts.nanos as u32));

        let _account_id: AccountId = request
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
            &owner_key,
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

        match mode {
            golem_api_grpc::proto::golem::worker::AgentInvocationMode::Await => {
                let (worker, _response_lease) = self
                    .get_or_create_pending_with_freshness(
                        request,
                        freshness_disposition,
                        expected_fingerprint,
                    )
                    .await?;
                let bound = scope
                    .bind(&worker.tasks)
                    .map_err(WorkerExecutorError::runtime);
                worker
                    .deleted_incarnation_as_not_found(expected_fingerprint, bound)
                    .await?;
                let metadata = worker.get_initial_worker_metadata();
                let final_agent_id = metadata.agent_id.clone();
                let ik = request_key
                    .finalize(final_agent_id.clone(), metadata.fingerprint, &method_name)
                    .id();
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
                    .get_metadata(metadata.component_id, Some(status.component_revision))
                    .await?;
                let (component, streaming) = if let Some(pending_revision) = pending_update_revision
                    && pending_revision != status.component_revision
                    && let Ok(pending_component) = self
                        .component_service()
                        .get_metadata(metadata.component_id, Some(pending_revision))
                        .await
                    && let Ok(parsed_agent_id) =
                        metadata.parse_agent_id(&pending_component.metadata)
                    && let Ok(streaming) = validate_agent_method_invocation(
                        &pending_component.metadata,
                        Some(&parsed_agent_id),
                        &method_name,
                        &method_parameters,
                    ) {
                    (pending_component, streaming)
                } else {
                    let parsed_agent_id = metadata
                        .parse_agent_id(&current_component.metadata)
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
                    idempotency_key: ik,
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
                            idempotency_key: ik,
                            component_revision: Some(component.revision),
                            durable_streams: Some(streams.clone()),
                            prepared: Some(acceptance.prepared),
                            durable_replayed: acceptance.replayed,
                            joined_origin_observer: acceptance.joined_origin_observer,
                        })
                        .map_err(|_| {
                            WorkerExecutorError::runtime(
                                "invocation session ended before durable acceptance",
                            )
                        })?;
                    if !acceptance.joined_origin_observer {
                        streams
                            .recover_nested_input_mappings()
                            .await
                            .map_err(|error| {
                                error.into_worker_executor_error(WorkerExecutorError::runtime)
                            })?;
                    }
                    worker.await_enqueued_invocation(ik).await?
                } else {
                    let (admission_tx, admission_rx) = tokio::sync::oneshot::channel();
                    let invocation =
                        worker.invoke_and_await_with_notifier(invocation, Some(admission_tx));
                    crate::worker::await_scalar_invocation(invocation, admission_rx, || {
                        publish_acceptance(acceptance_committed, accepted, &ik, accepted_revision)
                    })
                    .await?
                };
                invocation_output.agent_id = Some(final_agent_id);
                invocation_output.idempotency_key = Some(ik);
                Ok(invocation_output)
            }
            golem_api_grpc::proto::golem::worker::AgentInvocationMode::Schedule => {
                // A caller that names an incarnation is answered for that incarnation alone, so
                // it is pinned first, and its revision and method are validated against it.
                let pinned = match (expected_fingerprint, schedule_at) {
                    // A delayed schedule for a named incarnation is not refused because that
                    // incarnation failed, the same as a guest's local scheduled call.
                    (Some(expected), Some(_)) => Some(
                        Worker::get_exact_existing_suspended_for_response(
                            self,
                            &owned_agent_id,
                            principal.clone(),
                            Some(expected),
                        )
                        .await?,
                    ),
                    (Some(_), None) => Some(
                        self.get_or_create_pending_with_freshness(
                            request,
                            freshness_disposition,
                            expected_fingerprint,
                        )
                        .await?,
                    ),
                    (None, _) => None,
                };
                let existing_metadata = if let Some((worker, _)) = &pinned {
                    Some(worker.get_latest_worker_metadata().await)
                } else if freshness_disposition == InvocationFreshnessDisposition::KnownFresh {
                    None
                } else {
                    Worker::<Ctx>::get_latest_metadata(self, &owned_agent_id).await?
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
                    .get_metadata(
                        existing_metadata.as_ref().map_or_else(
                            || owned_agent_id.component_id(),
                            |metadata| metadata.component_id,
                        ),
                        component_revision,
                    )
                    .await?;
                let parsed_agent_id = match &existing_metadata {
                    Some(metadata) => metadata.parse_agent_id(&component.metadata),
                    None => ParsedAgentKey::parse(
                        &owned_agent_id.agent_id.agent_id,
                        &component.metadata,
                    ),
                }
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
                let (worker, _response_lease) = match pinned {
                    Some(pinned) => pinned,
                    None => {
                        self.get_or_create_pending_with_freshness(
                            request,
                            freshness_disposition,
                            None,
                        )
                        .await?
                    }
                };
                let bound = scope
                    .bind(&worker.tasks)
                    .map_err(WorkerExecutorError::runtime);
                worker
                    .deleted_incarnation_as_not_found(expected_fingerprint, bound)
                    .await?;
                let metadata = worker.get_initial_worker_metadata();
                let final_agent_id = metadata.agent_id;
                let ik = request_key
                    .finalize(final_agent_id.clone(), metadata.fingerprint, &method_name)
                    .id();
                let invocation = AgentInvocation::AgentMethod {
                    idempotency_key: ik,
                    method_name,
                    input: method_parameters,
                    invocation_context,
                    principal,
                    scope_card,
                };

                match schedule_at {
                    Some(scheduled_time) => {
                        worker
                            .schedule_invocation(invocation, scheduled_time)
                            .await?;
                        let metadata = worker.get_initial_worker_metadata();
                        publish_acceptance(
                            acceptance_committed,
                            accepted,
                            &ik,
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
                            agent_fingerprint: Some(metadata.fingerprint),
                        })
                    }
                    None => {
                        let result = worker.invoke_and_start(invocation).await?;
                        if let crate::worker::ResultOrSubscription::Finished(Err(err)) = &result {
                            return Err(err.clone());
                        }
                        publish_acceptance(
                            acceptance_committed,
                            accepted,
                            &ik,
                            Some(component.revision),
                        )?;
                        match result {
                            crate::worker::ResultOrSubscription::Finished(Err(err)) => {
                                unreachable!(
                                    "finished errors are handled before acceptance: {err}"
                                );
                            }
                            crate::worker::ResultOrSubscription::Finished(Ok(_)) => {}
                            crate::worker::ResultOrSubscription::Pending(_) => {}
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
}

pub(super) enum AcceptanceRace<A, O, R> {
    Accepted {
        acceptance: A,
        early_output: Option<O>,
        early_inbound: Option<R>,
    },
    InvocationFinished(O),
    InboundBeforeAcceptance(R),
}

pub(super) fn decode_invocation_freshness_disposition(
    value: i32,
) -> InvocationFreshnessDisposition {
    if value == golem::worker::InvocationFreshnessDisposition::KnownFresh as i32 {
        InvocationFreshnessDisposition::KnownFresh
    } else {
        InvocationFreshnessDisposition::MayExist
    }
}

pub(super) fn publish_acceptance(
    acceptance_committed: tokio::sync::oneshot::Sender<()>,
    accepted: tokio::sync::oneshot::Sender<AcceptedInvocation>,
    idempotency_key: &golem_common::model::InvocationId,
    component_revision: Option<ComponentRevision>,
) -> Result<(), WorkerExecutorError> {
    let _ = acceptance_committed.send(());
    accepted
        .send(AcceptedInvocation {
            idempotency_key: *idempotency_key,
            component_revision,
            durable_streams: None,
            prepared: None,
            durable_replayed: false,
            joined_origin_observer: false,
        })
        .map_err(|_| WorkerExecutorError::runtime("invocation session ended before acceptance"))
}

pub(super) async fn race_invocation_acceptance<A, O, R, I, F>(
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
