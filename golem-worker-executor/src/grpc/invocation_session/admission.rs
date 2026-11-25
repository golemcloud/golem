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
    AgentInvocationResult, ComponentRevision, DateTime, Future, HasAll, HasComponentService,
    InvocationFreshnessDisposition, InvocationStart, InvocationStatus, Pin, ScopeCard, UsesAllDeps,
    Utc, Worker, WorkerCtx, WorkerExecutorError, WorkerExecutorImpl,
    build_durable_streaming_request, from_proto_invocation_context, golem,
};
use golem_common::model::InvocationRequestKey;
use golem_common::model::caller::Caller;

impl<Ctx: WorkerCtx, Svcs: HasAll<Ctx> + UsesAllDeps<Ctx = Ctx> + Send + Sync + 'static>
    WorkerExecutorImpl<Ctx, Svcs>
{
    pub(super) async fn invoke_agent_internal(
        &self,
        request: &InvocationStart,
        acceptance_committed: tokio::sync::oneshot::Sender<()>,
        accepted: tokio::sync::oneshot::Sender<AcceptedInvocation>,
        scope: &crate::worker::tasks::TaskScope,
    ) -> Result<AgentInvocationOutput, WorkerExecutorError> {
        Self::validate_auth_ctx(&request.auth_ctx)?;

        if request.external_tool.is_some() {
            return self
                .invoke_external_tool_internal(request, acceptance_committed, accepted, scope)
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

        let resolved = self.resolve_request_target(request).await?;
        let final_agent_id: AgentId = resolved.owned_agent_id.agent_id;

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
                .get_or_create_pending_for_lookup(request, &resolved, expected_fingerprint)
                .await?
            {
                Some((worker, _response_lease)) => {
                    let bound = scope
                        .bind(&worker.tasks)
                        .map_err(WorkerExecutorError::runtime);
                    worker
                        .deleted_incarnation_as_not_found(expected_fingerprint, bound)
                        .await?;
                    match worker.lookup_invocation_result(&ik).await? {
                        crate::model::LookupResult::Complete(Ok(mut output)) => {
                            publish_acceptance(
                                acceptance_committed,
                                accepted,
                                &final_agent_id,
                                &ik,
                                None,
                            )?;
                            output.agent_id = Some(final_agent_id);
                            output.idempotency_key = Some(ik);
                            output.invocation_status = Some(InvocationStatus::Complete);
                            return Ok(output);
                        }
                        crate::model::LookupResult::Complete(Err(
                            WorkerExecutorError::InvocationCancelled,
                        )) => InvocationStatus::Cancelled,
                        crate::model::LookupResult::Complete(Err(err)) => return Err(err),
                        crate::model::LookupResult::Pending => InvocationStatus::Pending,
                        crate::model::LookupResult::New
                        | crate::model::LookupResult::Interrupted => InvocationStatus::Unknown,
                    }
                }
                None => InvocationStatus::Unknown,
            };
            publish_acceptance(acceptance_committed, accepted, &final_agent_id, &ik, None)?;
            return Ok(AgentInvocationOutput {
                result: AgentInvocationResult::AgentInitialization,
                consumed_fuel: None,
                invocation_status: Some(inv_status),
                component_revision: None,
                agent_id: Some(final_agent_id),
                idempotency_key: Some(ik),
                oplog_index: None,
                agent_fingerprint: None,
                background_failure: None,
            });
        }

        let method_name =
            request
                .method_name
                .clone()
                .ok_or(WorkerExecutorError::invalid_request(
                    "method_name is required for non-lookup invocations",
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

        let owned_agent_id = resolved.owned_agent_id.clone();

        Worker::<Ctx>::validate_invocation_freshness(
            self,
            &owned_agent_id,
            &owner_key,
            freshness_disposition,
            resolved.new_owner.as_ref(),
        )
        .await?;

        let caller: Caller = crate::grpc::extract_caller(&request.caller)?;

        let invocation_context = self
            .limit_invocation_context_stack_depth(from_proto_invocation_context(&request.context));

        match mode {
            golem_api_grpc::proto::golem::worker::AgentInvocationMode::Await => {
                let acquisition = self
                    .get_or_create_pending_with_freshness(
                        request,
                        &resolved,
                        freshness_disposition,
                        expected_fingerprint,
                    )
                    .await;
                let (worker, _response_lease) = match acquisition {
                    // A same-key retry that the agent's health refuses observes the recorded
                    // success of its invocation: an invocation that recorded its result keeps it
                    // when the agent gave up during its background work.
                    Err(
                        error @ (WorkerExecutorError::PreviousInvocationFailed { .. }
                        | WorkerExecutorError::PreviousInvocationExited),
                    ) if freshness_disposition == InvocationFreshnessDisposition::MayExist => {
                        match self
                            .get_or_create_pending_for_lookup(
                                request,
                                &resolved,
                                expected_fingerprint,
                            )
                            .await?
                        {
                            Some((worker, lease)) => {
                                let metadata = worker.get_initial_worker_metadata();
                                let ik = request_key
                                    .finalize(metadata.agent_id, metadata.fingerprint, &method_name)
                                    .id();
                                // The scope is not bound yet, so a deletion can stop this
                                // generation's state actor; that read fails with the original
                                // error rather than panicking.
                                // Another caller observes only a recorded success; a recorded
                                // failure keeps the agent's health error. Only an RPC caller
                                // waits for accepted work without a result on a failed agent:
                                // it is attached to that work.
                                let observable =
                                    match worker.try_lookup_invocation_result(&ik).await {
                                        Ok(crate::model::LookupResult::Complete(Ok(_))) => true,
                                        Ok(crate::model::LookupResult::New)
                                        | Err(WorkerExecutorError::Runtime { .. }) => false,
                                        Err(lookup_error) => {
                                            return worker
                                                .deleted_incarnation_as_not_found(
                                                    expected_fingerprint,
                                                    Err(lookup_error),
                                                )
                                                .await;
                                        }
                                        Ok(_) => request.origin_invocation.is_some(),
                                    };
                                if !observable {
                                    return worker
                                        .deleted_incarnation_as_not_found(
                                            expected_fingerprint,
                                            Err(error),
                                        )
                                        .await;
                                }
                                // Acceptance still validates the exact origin and descriptor.
                                // Observing accepted work does not start a failed runtime.
                                (worker, lease)
                            }
                            _ => return Err(error),
                        }
                    }
                    acquisition => acquisition?,
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
                    .finalize(final_agent_id, metadata.fingerprint, &method_name)
                    .id();
                let invocation = AgentInvocation::AgentMethod {
                    idempotency_key: ik,
                    method_name: method_name.clone(),
                    allow_streams: request.allow_streams,
                    input: super::capture_method_input(request)?,
                    invocation_context,
                    caller,
                    scope_card,
                };
                let retained_origin =
                    if request.allow_streams && request.origin_invocation.is_some() {
                        if let Some((attached, streams)) = worker
                            .retained_queued_stream_observer(&invocation, || {
                                super::request::capture_session_acceptance(
                                    request,
                                    self.services
                                        .config()
                                        .limits
                                        .live_stream_event_broadcast_capacity
                                        .get(),
                                )
                            })
                            .await?
                        {
                            let _ = acceptance_committed.send(());
                            accepted
                                .send(AcceptedInvocation {
                                    agent_id: final_agent_id,
                                    idempotency_key: ik,
                                    component_revision: None,
                                    durable_streams: Some(streams),
                                    prepared: None,
                                    durable_replayed: true,
                                    joined_origin_observer: true,
                                    queued: Some((
                                        attached,
                                        Box::pin(worker.clone().wait_method_session_ready(ik)),
                                    )),
                                })
                                .map_err(|_| {
                                    WorkerExecutorError::runtime(
                                        "invocation session ended before observer acceptance",
                                    )
                                })?;
                            let mut output = worker.await_enqueued_invocation(ik).await?;
                            output.agent_id = Some(final_agent_id);
                            output.idempotency_key = Some(ik);
                            return Ok(output);
                        }
                        worker.prepared_stream_session(&ik).await?
                    } else {
                        None
                    };
                if request.allow_streams
                    && request.origin_invocation.is_some()
                    && let Some(prepared) = retained_origin
                {
                    // A same-origin fork observes the retained execution. Its descriptor and
                    // authority comparison belong to the existing stream-join path.
                    let revision = prepared.attempt.invocation.target_component_revision;
                    let component = self
                        .component_service()
                        .get_metadata(metadata.component_id, Some(revision))
                        .await?;
                    let durable = build_durable_streaming_request(
                        request,
                        final_agent_id,
                        metadata.agent_type_name.as_ref(),
                        &component.metadata,
                        revision,
                        metadata.fingerprint,
                        invocation,
                        request
                            .input
                            .as_ref()
                            .map(prost::Message::encoded_len)
                            .unwrap_or_default(),
                        acceptance_committed,
                        self.services
                            .config()
                            .limits
                            .live_stream_event_broadcast_capacity
                            .get(),
                    )?;
                    let acceptance = worker.accept_durable_streaming_invocation(durable).await?;
                    accepted
                        .send(AcceptedInvocation {
                            agent_id: final_agent_id,
                            idempotency_key: ik,
                            component_revision: Some(revision),
                            durable_streams: Some(acceptance.streams),
                            prepared: Some(acceptance.prepared),
                            durable_replayed: acceptance.replayed,
                            joined_origin_observer: acceptance.joined_origin_observer,
                            queued: None,
                        })
                        .map_err(|_| {
                            WorkerExecutorError::runtime(
                                "invocation session ended before retained acceptance",
                            )
                        })?;
                    let mut output = worker.await_enqueued_invocation(ik).await?;
                    output.agent_id = Some(final_agent_id);
                    output.idempotency_key = Some(ik);
                    return Ok(output);
                }
                let mut invocation_output = if request.allow_streams
                    && worker.invocation_needs_stream_session(&invocation)
                {
                    let attempt_id = golem_common::model::durable_stream::AttemptId(
                        request
                            .attempt_id
                            .ok_or_else(|| {
                                WorkerExecutorError::invalid_request(
                                    "streaming invocation requires an attempt ID",
                                )
                            })?
                            .into(),
                    );
                    if attempt_id.0.is_nil()
                        || attempt_id.0.get_version() != Some(uuid::Version::Random)
                    {
                        return Err(WorkerExecutorError::invalid_request(
                            "streaming attempt must be a non-nil UUIDv4",
                        ));
                    }
                    let mut descriptor = super::request::capture_session_acceptance(
                        request,
                        self.services
                            .config()
                            .limits
                            .live_stream_event_broadcast_capacity
                            .get(),
                    )?;
                    descriptor.public_session_id = ik.to_string();
                    let producer = worker.durable_stream_producer().await?;
                    let session_key = producer.qualify_session(
                        &golem_common::model::durable_stream::StreamRegistrationInvocation::Local(
                            ik,
                        ),
                    );
                    let attached =
                        golem_common::model::durable_stream::StreamSessionAttachedRecord {
                            format_version:
                                golem_common::model::durable_stream::DURABLE_STREAM_FORMAT_VERSION,
                            session_key: ik,
                            attachment_id:
                                golem_common::model::durable_stream::AttachmentId::primary(
                                    resolved.owned_agent_id.environment_id,
                                    &final_agent_id,
                                    &ik,
                                )
                                .map_err(|error| {
                                    WorkerExecutorError::invalid_request(error.to_string())
                                })?,
                            attempt_id,
                            epoch: producer.attachment_epoch_floor(),
                            pending_invocation_oplog_index:
                                golem_common::model::oplog::OplogIndex::NONE,
                            acceptance: descriptor,
                        };
                    let replayed = worker.try_lookup_invocation_result(&ik).await?
                        != crate::model::LookupResult::New;
                    let (admission_tx, mut admission_rx) = tokio::sync::oneshot::channel();
                    let invocation = worker.clone().invoke_and_await_with_attachment(
                        invocation,
                        Some(admission_tx),
                        Some(attached.clone()),
                    );
                    tokio::pin!(invocation);
                    use crate::worker::read_only_cache::AdmissionSignal;
                    let completed = tokio::select! {
                        biased;
                        signal = &mut admission_rx => match signal {
                            Ok(AdmissionSignal::Queued | AdmissionSignal::Immediate) => None,
                            Ok(AdmissionSignal::Failure(error)) => return Err(error),
                            Ok(AdmissionSignal::Waiting) => unreachable!("waiting admission is never published"),
                            Err(_) => return Err(invocation.await.err().unwrap_or_else(|| {
                                WorkerExecutorError::runtime("streaming invocation completed without an admission signal")
                            })),
                        },
                        result = &mut invocation => {
                            if !matches!(admission_rx.try_recv(), Ok(AdmissionSignal::Queued | AdmissionSignal::Immediate)) {
                                result?;
                                return Err(WorkerExecutorError::runtime("streaming invocation completed without an admission signal"));
                            }
                            Some(result)
                        },
                    };
                    let retained = worker.retained_stream_attachment(&session_key).await?;
                    let (streams, queued) = if let Some((_, retained)) = retained {
                        if retained.acceptance != attached.acceptance
                            || retained.attempt_id != attempt_id
                        {
                            return Err(WorkerExecutorError::invalid_request(
                                "AttemptConflict: retained stream acceptance differs",
                            ));
                        }
                        (
                            Some(
                                worker
                                    .queued_method_session(ik, retained.epoch, retained.attempt_id)
                                    .await?,
                            ),
                            Some((
                                retained,
                                Box::pin(worker.clone().wait_method_session_ready(ik))
                                    as Pin<
                                        Box<
                                            dyn Future<Output = Result<_, WorkerExecutorError>>
                                                + Send,
                                        >,
                                    >,
                            )),
                        )
                    } else {
                        (None, None)
                    };
                    let _ = acceptance_committed.send(());
                    accepted
                        .send(AcceptedInvocation {
                            agent_id: final_agent_id,
                            idempotency_key: ik,
                            component_revision: None,
                            durable_streams: streams,
                            prepared: None,
                            durable_replayed: replayed,
                            joined_origin_observer: false,
                            queued,
                        })
                        .map_err(|_| {
                            WorkerExecutorError::runtime(
                                "invocation session ended before durable acceptance",
                            )
                        })?;
                    match completed {
                        Some(result) => result?,
                        None => invocation.await?,
                    }
                } else {
                    let (admission_tx, admission_rx) = tokio::sync::oneshot::channel();
                    let invocation =
                        worker.invoke_and_await_with_notifier(invocation, Some(admission_tx));
                    crate::worker::await_scalar_invocation(invocation, admission_rx, || {
                        publish_acceptance(
                            acceptance_committed,
                            accepted,
                            &final_agent_id,
                            &ik,
                            None,
                        )
                    })
                    .await?
                };
                invocation_output.agent_id = Some(final_agent_id);
                invocation_output.idempotency_key = Some(ik);
                Ok(invocation_output)
            }
            golem_api_grpc::proto::golem::worker::AgentInvocationMode::Schedule => {
                // A caller that names an incarnation is answered for that incarnation alone, so
                // it is pinned first. The method and its input are validated only when the
                // invocation executes, against the revision that the agent selects then.
                let pinned = match (expected_fingerprint, schedule_at) {
                    // A delayed schedule for a named incarnation is not refused because that
                    // incarnation failed, the same as a guest's local scheduled call.
                    (Some(expected), Some(_)) => {
                        let pinned = Worker::get_exact_existing_suspended_for_response(
                            self,
                            &owned_agent_id,
                            Some(expected),
                        )
                        .await?;
                        pinned.0.ensure_owns_key(resolved.new_owner.as_ref())?;
                        Some(pinned)
                    }
                    (Some(_), None) => Some(
                        self.get_or_create_pending_with_freshness(
                            request,
                            &resolved,
                            freshness_disposition,
                            expected_fingerprint,
                        )
                        .await?,
                    ),
                    (None, _) => None,
                };
                // A pinned incarnation that is being deleted refuses the request before its
                // scope is bound, as a deletion can already have stopped its state actor.
                if let Some((worker, _)) = &pinned {
                    worker
                        .deleted_incarnation_as_not_found(
                            expected_fingerprint,
                            worker.try_get_latest_worker_metadata().await,
                        )
                        .await?;
                }
                let (worker, _response_lease) = match pinned {
                    Some(pinned) => pinned,
                    None => {
                        // A scheduled call never attaches streams, so the revision that a
                        // missing agent is created with validates it without them.
                        let detached = golem_api_grpc::proto::golem::worker::InvocationStart {
                            allow_streams: false,
                            ..request.clone()
                        };
                        self.get_or_create_pending_with_freshness(
                            &detached,
                            &resolved,
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
                    .finalize(final_agent_id, metadata.fingerprint, &method_name)
                    .id();
                let invocation = AgentInvocation::AgentMethod {
                    idempotency_key: ik,
                    method_name,
                    allow_streams: false,
                    input: super::capture_method_input(request)?,
                    invocation_context,
                    caller,
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
                            &final_agent_id,
                            &ik,
                            None,
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
                            background_failure: None,
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
                            &final_agent_id,
                            &ik,
                            None,
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
                            background_failure: None,
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
    agent_id: &AgentId,
    idempotency_key: &golem_common::model::InvocationId,
    component_revision: Option<ComponentRevision>,
) -> Result<(), WorkerExecutorError> {
    let _ = acceptance_committed.send(());
    accepted
        .send(AcceptedInvocation {
            agent_id: *agent_id,
            idempotency_key: *idempotency_key,
            component_revision,
            durable_streams: None,
            prepared: None,
            durable_replayed: false,
            joined_origin_observer: false,
            queued: None,
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
