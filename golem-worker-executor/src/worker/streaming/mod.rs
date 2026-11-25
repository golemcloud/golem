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

use crate::worker::*;
use golem_common::model::durable_stream::{AttachmentId, StreamInvocationId};
use std::ops::ControlFlow;

use super::ingress::EnqueueInvocationOutcome;

enum StreamAttachment {
    FreshCommitted,
    Replay {
        joined_origin: bool,
        committed: tokio::sync::oneshot::Sender<()>,
    },
    PreparedOnly {
        invocation: Box<AgentInvocation>,
        committed: tokio::sync::oneshot::Sender<()>,
    },
}

enum StreamReceiptAction {
    Notify(tokio::sync::oneshot::Sender<()>),
    CommitAndNotify(tokio::sync::oneshot::Sender<()>),
}

enum StreamStatusObservation {
    LastKnown,
    AfterCommit,
}

struct StreamAttachmentCompletion {
    observation: StreamStatusObservation,
    acceptance: Result<DurableStreamingInvocationAcceptance, WorkerExecutorError>,
}

struct StreamAttachmentActions {
    stage_pending: Option<Box<AgentInvocation>>,
    receipt: Option<StreamReceiptAction>,
    observation: StreamStatusObservation,
    use_foreign_bindings: bool,
    replayed: bool,
    joined_origin_observer: bool,
}

fn transition_stream_attachment(
    attachment: StreamAttachment,
    current_attempt: bool,
) -> StreamAttachmentActions {
    match attachment {
        StreamAttachment::FreshCommitted => StreamAttachmentActions {
            stage_pending: None,
            receipt: None,
            observation: StreamStatusObservation::AfterCommit,
            use_foreign_bindings: true,
            replayed: false,
            joined_origin_observer: false,
        },
        StreamAttachment::Replay {
            joined_origin,
            committed,
        } => StreamAttachmentActions {
            stage_pending: None,
            receipt: Some(StreamReceiptAction::Notify(committed)),
            observation: StreamStatusObservation::LastKnown,
            use_foreign_bindings: !joined_origin && current_attempt,
            replayed: true,
            joined_origin_observer: joined_origin,
        },
        StreamAttachment::PreparedOnly {
            invocation,
            committed,
        } => StreamAttachmentActions {
            stage_pending: Some(invocation),
            receipt: Some(StreamReceiptAction::CommitAndNotify(committed)),
            observation: StreamStatusObservation::LastKnown,
            use_foreign_bindings: true,
            replayed: false,
            joined_origin_observer: false,
        },
    }
}

struct StreamAdmissionOwnership {
    lost_shard: Option<WorkerExecutorError>,
    deleting: bool,
    retired: Option<WorkerExecutorError>,
    assignment: Result<(), WorkerExecutorError>,
}

impl StreamAdmissionOwnership {
    fn refusal(self) -> Option<WorkerExecutorError> {
        self.lost_shard
            .or_else(|| {
                self.deleting
                    .then(|| WorkerExecutorError::invalid_request("Worker is being deleted"))
            })
            .or(self.retired)
            .or_else(|| {
                self.assignment
                    .err()
                    .filter(|error| matches!(error, WorkerExecutorError::InvalidShardId { .. }))
            })
    }
}

struct StreamRecoveryWake(WorkerExecutorError);

struct FailedStreamRecoveryWake {
    read_error: WorkerExecutorError,
    start_error: WorkerExecutorError,
}

impl StreamRecoveryWake {
    fn after_committed_read(error: WorkerExecutorError) -> Result<Self, WorkerExecutorError> {
        match InvocationObservationFailure::classify(&error) {
            Some(
                InvocationObservationFailure::PermanentRead
                | InvocationObservationFailure::RetryableRead,
            ) => Ok(Self(error)),
            _ => Err(error),
        }
    }

    fn after_start(
        self,
        start: Result<(), WorkerExecutorError>,
    ) -> ControlFlow<WorkerExecutorError, FailedStreamRecoveryWake> {
        match start {
            Ok(()) => ControlFlow::Break(self.0),
            Err(error) if RetirementReason::of_error(&error).is_some() => ControlFlow::Break(error),
            Err(start_error) => ControlFlow::Continue(FailedStreamRecoveryWake {
                read_error: self.0,
                start_error,
            }),
        }
    }
}

fn stream_admission_needs_wake(status: &AgentStatusRecord, key: &InvocationId) -> bool {
    let needs_replay = status.current_idempotency_key() == Some(key)
        && status.invocation_results.get(key).is_none()
        && status.status != AgentStatus::Interrupted;
    needs_replay
        || status
            .pending_invocations
            .iter()
            .any(|entry| entry.has_idempotency_key(key))
}

/// Whether a request replays the retained stream attachment. A route session is created again
/// with a fresh attempt and an expiry deadline computed at that creation, so it matches every
/// other part of the retained acceptance, including the execution configuration. The binding
/// checks the expiry policy. Any other session start must repeat the retained attempt exactly.
fn retained_acceptance_matches(
    retained: &StreamSessionAttachedRecord,
    requested: &golem_common::model::durable_stream::StreamSessionAcceptance,
    attempt_id: AttemptId,
) -> bool {
    match &requested.route_scope {
        Some(_) => {
            retained.acceptance
                == golem_common::model::durable_stream::StreamSessionAcceptance {
                    expiry_deadline_millis: retained.acceptance.expiry_deadline_millis,
                    ..requested.clone()
                }
        }
        None => retained.acceptance == *requested && retained.attempt_id == attempt_id,
    }
}

impl<Ctx: WorkerCtx> Worker<Ctx> {
    pub(in crate::worker) async fn ensure_matching_stream_attachment(
        &self,
        requested: &StreamSessionAttachedRecord,
    ) -> Result<(), WorkerExecutorError> {
        let producer = self.durable_stream_producer().await?;
        let key =
            producer.qualify_session(&StreamRegistrationInvocation::Local(requested.session_key));
        if let Some((_, retained)) = self.retained_stream_attachment(&key).await?
            && !retained_acceptance_matches(&retained, &requested.acceptance, requested.attempt_id)
        {
            return Err(WorkerExecutorError::invalid_request(
                "AttemptConflict: stream acceptance differs from the retained request",
            ));
        }
        Ok(())
    }

    pub(crate) async fn accept_queued_method(
        self: &Arc<Self>,
        invocation: AgentInvocation,
        mut acceptance: golem_common::model::durable_stream::StreamSessionAcceptance,
        attempt_id: AttemptId,
        committed: tokio::sync::oneshot::Sender<()>,
    ) -> Result<(StreamSessionAttachedRecord, bool), WorkerExecutorError> {
        let worker = self.clone();
        self.tasks.finish_on_drop(tokio::spawn(async move {
            let key = Self::require_idempotency_key(&invocation)?;
            let producer = worker.durable_stream_producer().await?;
            let session_key = StreamInvocationId {
                callee_environment_id: worker.owned_agent_id.environment_id,
                callee: worker.owned_agent_id.agent_id,
                callee_fingerprint: worker.initial_worker_metadata.fingerprint,
                idempotency_key: key,
            };
            let _session = producer.session_lock(&session_key).lock_owned().await;
            let lookup = worker.lookup_matching_invocation(&invocation).await?;
            if acceptance.public_session_id.is_empty() {
                acceptance.public_session_id = key.to_string();
            }
            let existing = worker.retained_stream_attachment(&session_key).await?;
            let (attached, replayed) = if let Some((_, attached)) = existing {
                if !retained_acceptance_matches(&attached, &acceptance, attempt_id) {
                    return Err(WorkerExecutorError::invalid_request("AttemptConflict: stream acceptance does not match the retained request"));
                }
                (attached, true)
            } else {
                if lookup != LookupResult::New {
                    return Err(WorkerExecutorError::invalid_request("IdempotencyConflict: retained invocation has no stream attachment"));
                }
                if let AgentInvocation::AgentMethod { scope_card: Some(card), .. } = &invocation {
                    crate::services::card::validate_scope_card(worker.card_service().as_ref(), card).await?;
                }
                let mut attached = StreamSessionAttachedRecord {
                    format_version: DURABLE_STREAM_FORMAT_VERSION,
                    session_key: key,
                    attachment_id: AttachmentId::primary(worker.owned_agent_id.environment_id, &worker.owned_agent_id.agent_id, &key)
                        .map_err(|error| WorkerExecutorError::invalid_request(error.to_string()))?,
                    attempt_id,
                    epoch: producer.attachment_epoch_floor(),
                    pending_invocation_oplog_index: OplogIndex::NONE,
                    acceptance,
                };
                worker.schedule_stream_session_expiry(attached.acceptance.public_session_id.clone(), key,
                    attached.acceptance.expiry_deadline_millis).await?;
                match worker.enqueue_worker_invocation_with_effect(invocation,
                    read_only_cache::InvocationEffect::UnknownAssumeMutating, None, Some(attached.clone())).await? {
                    EnqueueInvocationOutcome::Stored { index, .. } => {
                        attached.pending_invocation_oplog_index = index;
                        (attached, false)
                    }
                    EnqueueInvocationOutcome::Existing => {
                        let (_, retained) = worker.retained_stream_attachment(&session_key).await?
                            .ok_or_else(|| WorkerExecutorError::runtime("committed invocation has no stream attachment"))?;
                        (retained, true)
                    }
                    EnqueueInvocationOutcome::StoredButUnreadable(error) => {
                        drop(_session);
                        return Err(worker.resolve_committed_invocation_failure(error.into()).await);
                    }
                }
            };
            worker.schedule_stream_session_expiry(attached.acceptance.public_session_id.clone(), key,
                attached.acceptance.expiry_deadline_millis).await?;
            let _ = committed.send(());
            drop(_session);
            Worker::start_if_needed(worker.clone()).await?;
            Ok((attached, replayed))
        })).await.map_err(|error| WorkerExecutorError::runtime(error.to_string()))?
    }

    pub(crate) async fn queued_method_session(
        &self,
        key: InvocationId,
        epoch: u64,
        attempt: AttemptId,
    ) -> Result<StreamSession, WorkerExecutorError> {
        let response_lease = if self.agent_mode() == AgentMode::Ephemeral {
            Some(
                self.durable_stream_producer
                    .retain_response()
                    .map_err(|error| {
                        error.into_worker_executor_error(WorkerExecutorError::runtime)
                    })?,
            )
        } else {
            None
        };
        let producer = self.durable_stream_producer().await?;
        // A revert or a takeover can advance the attachment past the retained record, so the
        // session reports the current epoch that a resume must name.
        let epoch = self
            .oplog
            .raw_durable_stream_session_status(
                &producer.qualify_session(&StreamRegistrationInvocation::Local(key)),
            )
            .await
            .status
            .map_err(|error| error.into_worker_executor_error(WorkerExecutorError::runtime))?
            .and_then(|status| status.attachment_epoch)
            .unwrap_or(epoch);
        Ok(StreamSession::open(
            producer,
            self.oplog.clone(),
            StreamRegistrationInvocation::Local(key),
            std::iter::empty(),
        )
        .await
        .map_err(|error| error.into_worker_executor_error(WorkerExecutorError::runtime))?
        .with_attachment(epoch, attempt)
        .with_response_lease(response_lease))
    }

    pub(crate) async fn resume_queued_method(
        self: &Arc<Self>,
        attempt: &ResumeAttemptDescriptor,
    ) -> Result<Option<(StreamSessionAttachedRecord, u64, StreamSession)>, WorkerExecutorError>
    {
        let producer = self.durable_stream_producer().await?;
        let worker = self.clone();
        let attempt = attempt.clone();
        let memory = golem_common::serialization::serialize(&attempt)
            .map_err(WorkerExecutorError::runtime)?
            .len();
        producer
            .run_admitted(None, memory, true, move |producer, admission| async move {
                Ok::<_, crate::durable_host::durable_stream::SessionError>(
                    worker
                        .resume_queued_method_owned(&attempt, producer, &admission)
                        .await,
                )
            })
            .await
            .map_err(|error| error.into_worker_executor_error(WorkerExecutorError::runtime))?
    }

    async fn resume_queued_method_owned(
        &self,
        attempt: &ResumeAttemptDescriptor,
        producer: Arc<crate::durable_host::durable_stream::DurableStreamStore>,
        admission: &Arc<crate::durable_host::durable_stream::StreamWriteAdmission>,
    ) -> Result<Option<(StreamSessionAttachedRecord, u64, StreamSession)>, WorkerExecutorError>
    {
        if self.owner_retirement_requested.is_cancelled() {
            return Err(self.retirement_error());
        }
        let session_guard = producer
            .session_lock(&attempt.session_key)
            .lock_owned()
            .await;
        let instance_guard = self.lock_non_stopping_worker().await;
        instance_guard
            .ensure_not_deleting()
            .map_err(|_| WorkerExecutorError::invalid_request("Cannot resume a deleting worker"))?;
        let records = self
            .stream_session_records(&attempt.session_key, Some(attempt.attempt_id))
            .await?
            .records;
        if records
            .iter()
            .any(|(_, record)| matches!(record, StreamSessionRecord::Prepared(_)))
        {
            return Ok(None);
        }
        let attached = records.iter().find_map(|(_, record)| {
            if let StreamSessionRecord::Attached(record) = record {
                Some(record.clone())
            } else {
                None
            }
        });
        let Some(attached) = attached else {
            return Ok(None);
        };
        if attempt.expected_callee_fingerprint != self.initial_worker_metadata.fingerprint
            || attached.attachment_id != attempt.attachment_id
        {
            return Err(WorkerExecutorError::invalid_request(
                "IncarnationMismatch: resume does not identify the persisted callee attachment",
            ));
        }
        if attached.acceptance.effective_identity != attempt.effective_identity {
            return Err(WorkerExecutorError::invalid_request(
                "Unauthorized: resume identity differs from the pinned caller and grant",
            ));
        }
        if attached.attempt_id == attempt.attempt_id {
            return Err(WorkerExecutorError::invalid_request(
                "AttemptConflict: Start attempt cannot be reused for resume",
            ));
        }
        if !attempt.cursors.is_empty() {
            return Err(WorkerExecutorError::invalid_request(
                "queued invocation has no output cursors",
            ));
        }
        for (_, record) in &records {
            if let StreamSessionRecord::ResumeAttempt(record) = record
                && record.attempt.attempt_id == attempt.attempt_id
            {
                if &record.attempt != attempt {
                    return Err(WorkerExecutorError::invalid_request(
                        "AttemptConflict: retained resume attempt differs",
                    ));
                }
                let streams = self
                    .queued_method_session(
                        attempt.session_key.idempotency_key,
                        record.accepted_epoch,
                        attempt.attempt_id,
                    )
                    .await?;
                return Ok(Some((attached, streams.attachment_epoch(), streams)));
            }
        }
        let epoch = attempt.expected_epoch.checked_add(1).ok_or_else(|| {
            WorkerExecutorError::invalid_request("ResourceExhausted: attachment epoch cannot wrap")
        })?;
        let streams = self
            .queued_method_session(
                attempt.session_key.idempotency_key,
                attempt.expected_epoch,
                attempt.attempt_id,
            )
            .await?;
        streams
            .commit_resume_attempt_locked(
                StreamSessionResumeAttemptRecord {
                    format_version: DURABLE_STREAM_FORMAT_VERSION,
                    session_key: attempt.session_key.idempotency_key,
                    attempt: attempt.clone(),
                    accepted_epoch: epoch,
                },
                session_guard,
                admission,
            )
            .await
            .map_err(|error| {
                error.into_worker_executor_error(WorkerExecutorError::invalid_request)
            })?;
        Ok(Some((
            attached,
            epoch,
            streams.with_attachment(epoch, attempt.attempt_id),
        )))
    }

    pub(crate) async fn wait_method_session_ready(
        self: Arc<Self>,
        key: InvocationId,
    ) -> Result<(StreamSessionPreparedRecord, StreamSession), WorkerExecutorError> {
        let producer = self.durable_stream_producer().await?;
        let mut events = self.events().subscribe();
        loop {
            let changed = producer.session_records_changed().notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            if self
                .durable_stream_session_status(&key)
                .await?
                .is_some_and(|status| status.expired || status.cancellation_requested)
            {
                return Err(WorkerExecutorError::invalid_request(
                    "stream session was cancelled or expired",
                ));
            }
            if let Some((prepared, streams)) = self.native_tool_session(&key).await? {
                let status = self
                    .durable_stream_session_status(&key)
                    .await?
                    .ok_or_else(|| {
                        WorkerExecutorError::runtime("ready session has no attachment status")
                    })?;
                let (graph, elements) = crate::grpc::resumed_input_schema(&prepared)?;
                let streams = streams
                    .with_attachment(
                        status.attachment_epoch.unwrap_or_default(),
                        status
                            .attachment_attempt_id
                            .unwrap_or(prepared.attempt.attempt_id),
                    )
                    .with_input_schema(
                        Arc::new(graph),
                        prepared.attempt.invocation.target_component_revision,
                        elements,
                    );
                return Ok((prepared, streams));
            }
            match self.try_lookup_invocation_result(&key).await? {
                LookupResult::Complete(Err(error)) => return Err(error),
                LookupResult::Complete(Ok(_)) | LookupResult::Interrupted => {
                    return Err(WorkerExecutorError::runtime(
                        "queued session ended before preparation",
                    ));
                }
                _ => {}
            }
            tokio::select! {
                _ = &mut changed => {},
                _ = events.wait_for(|_| Some(())) => {},
                _ = self.owner_retirement_requested.cancelled() => return Err(self.retirement_error()),
            }
        }
    }

    pub(in crate::worker) async fn pending_streaming_invocation_entry(
        &self,
        invocation: AgentInvocation,
    ) -> Result<OplogEntry, WorkerExecutorError> {
        self.accept_ephemeral_invocation(&invocation)?;
        let (idempotency_key, invocation_payload, invocation_context) = invocation.into_parts();
        let invocation_context = invocation_context
            .limit_depth(self.deps.config().limits.max_invocation_context_stack_depth);
        let (operation, payload) = self
            .oplog
            .upload_request(invocation_payload)
            .await
            .map_err(|error| {
                WorkerExecutorError::invalid_request(format!(
                    "Failed to upload invocation payload: {error}"
                ))
            })?;
        let invocation_context_spans = invocation_context.to_oplog_data();
        Ok(OplogEntry::pending_agent_invocation(
            None,
            idempotency_key,
            Box::new(operation),
            payload,
            invocation_context.trace_id,
            invocation_context.trace_states,
            invocation_context_spans,
        ))
    }
    /// Accepts a durable streaming invocation with metering and attempt reporting.
    pub async fn accept_durable_streaming_invocation(
        self: &Arc<Self>,
        request: DurableStreamingInvocationRequest,
    ) -> Result<DurableStreamingInvocationAcceptance, WorkerExecutorError> {
        let worker = self.clone();
        let result = self
            .tasks
            .finish_on_drop(tokio::spawn(async move {
                worker
                    .accept_durable_streaming_invocation_unmetered(
                        request,
                        DurableStreamingAcceptanceMatch::ExactAttempt,
                        None,
                    )
                    .await
            }))
            .await
            .map_err(|error| WorkerExecutorError::runtime(error.to_string()))?;
        match &result {
            Ok(acceptance) => crate::metrics::durable_stream::record_attempt(
                "start",
                if acceptance.replayed {
                    "replayed"
                } else {
                    "accepted"
                },
                (!acceptance.replayed).then_some(1),
            ),
            Err(error) => crate::metrics::durable_stream::record_attempt(
                "start",
                durable_stream_attempt_error_outcome(error),
                None,
            ),
        }
        result
    }
    /// Accepts a stream-slot invocation that has already been metered by its transport.
    pub async fn accept_durable_stream_slot_invocation(
        self: &Arc<Self>,
        request: DurableStreamingInvocationRequest,
        creation_admission: durable_stream_slots::StreamSessionCreationAdmission,
    ) -> Result<DurableStreamingInvocationAcceptance, WorkerExecutorError> {
        self.accept_durable_streaming_invocation_unmetered(
            request,
            DurableStreamingAcceptanceMatch::StreamSlotSession,
            Some(creation_admission),
        )
        .await
    }
    pub(in crate::worker) async fn accept_durable_streaming_invocation_unmetered(
        self: &Arc<Self>,
        request: DurableStreamingInvocationRequest,
        acceptance_match: DurableStreamingAcceptanceMatch,
        creation_admission: Option<durable_stream_slots::StreamSessionCreationAdmission>,
    ) -> Result<DurableStreamingInvocationAcceptance, WorkerExecutorError> {
        let producer = self.durable_stream_producer().await?;
        let instance_guard = self.lock_non_stopping_worker_owned().await;
        if self.owner_retirement_requested.is_cancelled() {
            return Err(self.retirement_error());
        }
        if instance_guard.ensure_not_deleting().is_err() {
            return Err(WorkerExecutorError::invalid_request(
                "Cannot invoke a deleting worker",
            ));
        }
        let worker = self.clone();
        tokio::spawn(async move {
            let result = worker
                .accept_durable_streaming_invocation_owned(
                    request,
                    acceptance_match,
                    producer,
                    instance_guard,
                    creation_admission
                        .as_ref()
                        .map(|admission| admission.route_scope.clone()),
                )
                .await;
            drop(creation_admission);
            result
        })
        .await
        .map_err(|error| WorkerExecutorError::runtime(error.to_string()))?
    }
    pub(in crate::worker) async fn accept_durable_streaming_invocation_owned(
        self: &Arc<Self>,
        request: DurableStreamingInvocationRequest,
        acceptance_match: DurableStreamingAcceptanceMatch,
        producer: Arc<DurableStreamStore>,
        instance_guard: OwnedMutexGuard<WorkerLifecycle>,
        route_scope: Option<String>,
    ) -> Result<DurableStreamingInvocationAcceptance, WorkerExecutorError> {
        let StreamAttachmentCompletion {
            observation,
            acceptance,
        } = self
            .attach_durable_streaming_invocation(
                request,
                acceptance_match,
                producer,
                instance_guard,
                route_scope,
            )
            .await?;

        // Attachment work has released the instance guard. Startup takes it again.
        let (acceptance, status) = match observation {
            StreamStatusObservation::LastKnown => {
                (acceptance?, self.try_get_last_known_status().await?)
            }
            StreamStatusObservation::AfterCommit => {
                let acceptance = match acceptance {
                    Ok(acceptance) => acceptance,
                    Err(error) => {
                        return Err(self.resolve_committed_invocation_failure(error).await);
                    }
                };
                // A fresh durable receipt proves storage, not a successful status fold. Repair
                // detached status before checking for the key. For ephemeral agents, the repair
                // can refuse an archive write with the same variant as a permanent read error.
                let status = if self.agent_mode() == AgentMode::Durable {
                    match self.state_actor.observe_repaired_status().await {
                        Ok(Some(status)) => status,
                        Ok(None) => {
                            return Err(self
                                .observe_stream_admission_ownership()
                                .await
                                .refusal()
                                .unwrap_or_else(|| {
                                    WorkerExecutorError::runtime("Worker status actor stopped")
                                }));
                        }
                        Err(error) => {
                            return Err(self.resolve_committed_invocation_failure(error).await);
                        }
                    }
                } else {
                    self.try_get_last_known_status().await?
                };
                (acceptance, status)
            }
        };
        if stream_admission_needs_wake(
            &status,
            &acceptance.prepared.attempt.session_key.idempotency_key,
        ) {
            Worker::start_if_needed(self.clone()).await?;
        }
        Ok(acceptance)
    }

    // Errors after preparation use the status observation selected for the attachment.
    async fn attach_durable_streaming_invocation(
        &self,
        request: DurableStreamingInvocationRequest,
        acceptance_match: DurableStreamingAcceptanceMatch,
        producer: Arc<DurableStreamStore>,
        instance_guard: OwnedMutexGuard<WorkerLifecycle>,
        route_scope: Option<String>,
    ) -> Result<StreamAttachmentCompletion, WorkerExecutorError> {
        let response_lease = if self.agent_mode() == AgentMode::Ephemeral {
            Some(
                self.durable_stream_producer
                    .retain_response()
                    .map_err(|error| {
                        error.into_worker_executor_error(WorkerExecutorError::runtime)
                    })?,
            )
        } else {
            None
        };
        if self.owner_retirement_requested.is_cancelled() {
            return Err(self.retirement_error());
        }
        let session_key = request.attempt.session_key.clone();
        let records = self
            .stream_session_records(&session_key, None)
            .await?
            .records;
        let status = self
            .oplog
            .raw_durable_stream_session_status(&session_key)
            .await
            .status
            .map_err(|error| error.into_worker_executor_error(WorkerExecutorError::runtime))?;
        let mut attached_records = records.iter().filter_map(|(_, record)| match record {
            StreamSessionRecord::Attached(attached) => Some(attached),
            _ => None,
        });
        let attached = attached_records.next();
        let initial_epoch = match attached {
            Some(attached) => status
                .as_ref()
                .and_then(|status| status.attachment_epoch)
                .unwrap_or(attached.epoch),
            None => producer.attachment_epoch_floor(),
        };
        let mut prepared_records = records.iter().filter_map(|(index, record)| match record {
            StreamSessionRecord::Prepared(prepared) => Some((*index, prepared.clone())),
            _ => None,
        });
        let existing_prepared = prepared_records.next();
        if existing_prepared.is_some() && prepared_records.next().is_some() {
            return Err(WorkerExecutorError::runtime(
                "durable Stream Session contains multiple Prepared records",
            ));
        }
        if existing_prepared
            .as_ref()
            .is_some_and(|(_, prepared)| prepared.route_scope != route_scope)
        {
            return Err(WorkerExecutorError::invalid_request(
                "IdempotencyConflict: the durable stream session belongs to a different route",
            ));
        }
        if existing_prepared.is_none()
            && let Some(error) = instance_guard.startup_failure()
        {
            return Err(error.clone());
        }
        let roles = request
            .registrations
            .iter()
            .map(|(id, registration)| {
                registration
                    .session_mapping
                    .as_ref()
                    .map(|mapping| (*id, mapping.role))
                    .ok_or_else(|| {
                        WorkerExecutorError::invalid_request(
                            "session registration has no stream role",
                        )
                    })
            })
            .collect::<Result<HashMap<_, _>, _>>()?;
        if !request.registrations.is_empty() && !request.foreign_mappings.is_empty() {
            return Err(WorkerExecutorError::invalid_request(
                "durable invocation inputs cannot mix inline and agent-hosted stream mappings",
            ));
        }
        let foreign_mappings = request.foreign_mappings.as_slice();
        for mapping in foreign_mappings {
            if producer.owns_handle_identity(&mapping.handle) {
                producer
                    .validate_handle(&mapping.handle)
                    .await
                    .map_err(|error| WorkerExecutorError::invalid_request(error.to_string()))?;
            }
        }
        let expiry_publication_guard = if existing_prepared.is_none() {
            if attached.is_some() {
                return Err(WorkerExecutorError::invalid_request(
                    "IdempotencyConflict: fresh streaming preparation cannot replace a retained attachment",
                ));
            }
            let guard = producer.session_lock(&session_key).lock_owned().await;
            self.schedule_stream_session_expiry(
                request.public_session_id.clone(),
                session_key.idempotency_key,
                request.expiry_deadline_millis,
            )
            .await?;
            Some(guard)
        } else {
            None
        };

        let (prepared, attachment) = if let Some((prepared_index, prepared)) = existing_prepared {
            let mut requested_attempt = request.attempt.clone();
            let requested_handles = if foreign_mappings.is_empty() {
                let mut handles = Vec::with_capacity(request.registrations.len());
                for (_, registration) in &request.registrations {
                    handles.push(
                        producer
                            .validate_registration(registration)
                            .await
                            .map_err(|error| {
                                SessionError::from(error).into_worker_executor_error(|_| {
                                    WorkerExecutorError::invalid_request(
                                        "IdempotencyConflict: persisted durable stream registration does not match the invocation descriptor",
                                    )
                                })
                            })?,
                    );
                }
                handles
            } else {
                foreign_mappings
                    .iter()
                    .map(|mapping| mapping.handle.clone())
                    .collect()
            };
            requested_attempt.invocation.stream_handles = if foreign_mappings.is_empty() {
                request
                    .registrations
                    .iter()
                    .zip(&requested_handles)
                    .filter(|((id, _), _)| roles[id].direction() == SessionStreamRole::Input)
                    .map(|(_, handle)| handle.clone())
                    .collect()
            } else {
                requested_handles.clone()
            };
            let joined_origin = matches!(
                acceptance_match,
                DurableStreamingAcceptanceMatch::ExactAttempt
            ) && request.registrations.is_empty()
                && (!stream_attempt_matches(&prepared.attempt, &requested_attempt)
                    || prepared.attempt.invocation.stream_handles
                        != requested_attempt.invocation.stream_handles)
                && same_origin_stream_invocation(&prepared.attempt, &requested_attempt).map_err(
                    |error| self.stream_execution_config_read_error(prepared_index, error),
                )?
                && prepared.stream_mappings.len() == foreign_mappings.len()
                && prepared
                    .stream_mappings
                    .iter()
                    .zip(foreign_mappings)
                    .all(|(a, b)| {
                        a.transport_stream_id == b.transport_stream_id && a.role == b.role
                    });
            let descriptor_matches = match acceptance_match {
                DurableStreamingAcceptanceMatch::ExactAttempt => {
                    persisted_stream_descriptor_matches(
                        &prepared.attempt.invocation,
                        &requested_attempt.invocation,
                    )
                }
                DurableStreamingAcceptanceMatch::StreamSlotSession => true,
            };
            if !descriptor_matches && !joined_origin {
                self.validate_retained_stream_descriptor_config(
                    prepared_index,
                    &prepared.attempt.invocation,
                    &requested_attempt.invocation,
                )?;
                return Err(WorkerExecutorError::invalid_request(
                    "IdempotencyConflict: the invocation key is already bound to a different durable stream descriptor",
                ));
            }
            match acceptance_match {
                DurableStreamingAcceptanceMatch::ExactAttempt
                    if !joined_origin
                        && (prepared.attempt.attempt_id != request.attempt.attempt_id
                            || !stream_attempt_matches(&prepared.attempt, &requested_attempt)) =>
                {
                    return Err(WorkerExecutorError::invalid_request(
                        "AttemptConflict: the durable session start attempt does not exactly match the persisted attempt",
                    ));
                }
                DurableStreamingAcceptanceMatch::StreamSlotSession
                    if !stream_slot_session_matches(&prepared.attempt, &requested_attempt) =>
                {
                    return Err(WorkerExecutorError::invalid_request(
                        "IdempotencyConflict: the invocation key is already bound to a different durable stream invocation",
                    ));
                }
                _ => {}
            }
            let requested_mappings = if foreign_mappings.is_empty() {
                let mut bindings = Vec::with_capacity(requested_handles.len());
                for ((transport_stream_id, _), handle) in
                    request.registrations.iter().zip(&requested_handles)
                {
                    bindings.push(
                        producer
                            .local_binding(*transport_stream_id, handle, roles[transport_stream_id])
                            .await
                            .map_err(|error| {
                                error.into_worker_executor_error(WorkerExecutorError::runtime)
                            })?,
                    );
                }
                bindings
            } else {
                foreign_mappings
                    .iter()
                    .map(StreamBindingRecord::foreign)
                    .collect()
            };
            if prepared.stream_mappings != requested_mappings && !joined_origin {
                return Err(WorkerExecutorError::invalid_request(
                    "AttemptConflict: the durable session stream mappings do not exactly match the persisted attempt",
                ));
            }
            if attached_records.next().is_some() {
                return Err(WorkerExecutorError::runtime(
                    "durable Stream Session contains multiple Attached records",
                ));
            }
            let attachment = match attached {
                Some(attached) => {
                    if attached.session_key != prepared.session_key
                        || attached.attachment_id != prepared.attempt.attachment_id
                        || attached.attempt_id != prepared.attempt.attempt_id
                        || attached.epoch == 0
                        || !matches!(
                            self.oplog.read(attached.pending_invocation_oplog_index).await?,
                            OplogEntry::PendingAgentInvocation { invocation_id, .. }
                                if invocation_id == prepared.attempt.session_key.idempotency_key
                        )
                    {
                        return Err(WorkerExecutorError::runtime(
                            "durable Attached record does not exactly identify its Prepared attempt and pending invocation",
                        ));
                    }
                    StreamAttachment::Replay {
                        joined_origin,
                        committed: request.acceptance_committed,
                    }
                }
                None if joined_origin => {
                    return Err(WorkerExecutorError::runtime(
                        "cannot join a streaming invocation without its retained pending invocation",
                    ));
                }
                None => StreamAttachment::PreparedOnly {
                    invocation: Box::new(request.invocation),
                    committed: request.acceptance_committed,
                },
            };
            (prepared, attachment)
        } else if !foreign_mappings.is_empty() {
            let mut attempt = request.attempt.clone();
            attempt.invocation.stream_handles = foreign_mappings
                .iter()
                .map(|mapping| mapping.handle.clone())
                .collect();
            let prepared = StreamSessionPreparedRecord {
                format_version: 1,
                session_key: session_key.idempotency_key,
                public_session_id: request.public_session_id.clone(),
                route_scope: route_scope.clone(),
                expiry_policy: request.expiry_policy,
                expiry_deadline_millis: request.expiry_deadline_millis,
                attempt,
                stream_mappings: foreign_mappings
                    .iter()
                    .map(StreamBindingRecord::foreign)
                    .collect(),
            };
            let streams = StreamSession::new(
                producer.clone(),
                self.oplog.clone(),
                StreamRegistrationInvocation::Local(session_key.idempotency_key),
                std::iter::empty(),
            );
            let topologies = foreign_mappings
                .iter()
                .filter(|mapping| !producer.owns_handle_identity(&mapping.handle))
                .map(|mapping| {
                    Ok(
                        golem_common::model::durable_stream::StreamTopologyPreparedRecord {
                            format_version: DURABLE_STREAM_FORMAT_VERSION,
                            session_key: session_key.clone(),
                            attachment: streams
                                .attachment_key(&mapping.handle, initial_epoch)
                                .map_err(|error| {
                                    error.into_worker_executor_error(WorkerExecutorError::runtime)
                                })?,
                            mapping: mapping.clone(),
                        },
                    )
                })
                .collect::<Result<Vec<_>, WorkerExecutorError>>()?;
            let pending = self
                .pending_streaming_invocation_entry(request.invocation)
                .await?;
            let prepared = producer
                .prepare_session(
                    None,
                    Vec::new(),
                    topologies,
                    pending,
                    request.acceptance_committed,
                    move |_| prepared,
                )
                .await
                .map_err(|error| {
                    error.into_worker_executor_error(WorkerExecutorError::invalid_request)
                })?;
            (prepared, StreamAttachment::FreshCommitted)
        } else {
            let pending = self
                .pending_streaming_invocation_entry(request.invocation)
                .await?;
            let attempt = request.attempt.clone();
            let local_session_key = session_key.idempotency_key;
            let public_session_id = request.public_session_id.clone();
            let expiry_policy = request.expiry_policy;
            let expiry_deadline_millis = request.expiry_deadline_millis;
            let prepared = producer
                .prepare_session(
                    None,
                    request.registrations,
                    Vec::new(),
                    pending,
                    request.acceptance_committed,
                    move |bindings| StreamSessionPreparedRecord {
                        format_version: 1,
                        public_session_id,
                        route_scope,
                        session_key: local_session_key,
                        expiry_policy,
                        expiry_deadline_millis,
                        stream_mappings: bindings,
                        attempt,
                    },
                )
                .await
                .map_err(|error| error.into_worker_executor_error(WorkerExecutorError::runtime))?;
            (prepared, StreamAttachment::FreshCommitted)
        };
        drop(expiry_publication_guard);

        let current_attempt = status.as_ref().is_some_and(|status| {
            status.attachment_attached == Some(true)
                && status.attachment_epoch == Some(initial_epoch)
                && status.attachment_attempt_id == Some(prepared.attempt.attempt_id)
        });
        let StreamAttachmentActions {
            stage_pending,
            receipt,
            observation,
            use_foreign_bindings,
            replayed,
            joined_origin_observer,
        } = transition_stream_attachment(attachment, current_attempt);
        let skip_foreign_bindings = !use_foreign_bindings
            || foreign_mappings.is_empty()
            || records
                .iter()
                .any(|(_, record)| matches!(record, StreamSessionRecord::Finished(_)));

        let acceptance = async move {
            let persisted_foreign_bindings = if skip_foreign_bindings {
                &[]
            } else {
                prepared.stream_mappings.as_slice()
            };
            let streams = StreamSession::open(
                producer.clone(),
                self.oplog.clone(),
                StreamRegistrationInvocation::Local(session_key.idempotency_key),
                prepared.stream_mappings.iter().cloned(),
            )
            .await
            .map_err(|error| error.into_worker_executor_error(WorkerExecutorError::runtime))?
            .with_attachment(initial_epoch, prepared.attempt.attempt_id)
            .with_rpc(self.rpc())
            .with_consumer_journal(self.durable_stream_consumer_journal())
            .with_auth_ctx(self.durable_stream_consumer_auth_ctx()?)
            .with_input_schema(
                request.input_schema,
                prepared.attempt.invocation.target_component_revision,
                request.input_element_types,
            )
            .with_response_lease(response_lease);
            let persisted_foreign_mappings = producer
                .materialize_bindings(persisted_foreign_bindings)
                .await
                .map_err(|error| error.into_worker_executor_error(WorkerExecutorError::runtime))?
                .into_iter()
                .filter(|mapping| !producer.owns_handle_identity(&mapping.handle))
                .collect::<Box<[_]>>();
            for mapping in persisted_foreign_mappings.iter() {
                if streams
                    .has_journaled_consumer_terminal(mapping)
                    .await
                    .map_err(|error| error.into_worker_executor_error(WorkerExecutorError::runtime))?
                {
                    continue;
                }
                streams
                    .prepare_foreign_mapping(mapping.clone(), initial_epoch)
                    .await
                    .map_err(|error| error.into_worker_executor_error(WorkerExecutorError::runtime))?;
            }
            if let Some(invocation) = stage_pending {
                let pending = self.pending_streaming_invocation_entry(*invocation).await?;
                let attached_attempt = prepared.attempt.clone();
                let attached_acceptance = prepared.acceptance();
                self.oplog
                    .add_pair(
                        pending,
                        Box::new(move |pending_invocation_oplog_index| {
                            let record =
                                StreamSessionRecord::Attached(StreamSessionAttachedRecord {
                                    format_version: 1,
                                    session_key: attached_attempt.session_key.idempotency_key,
                                    attachment_id: attached_attempt.attachment_id,
                                    attempt_id: attached_attempt.attempt_id,
                                    epoch: initial_epoch,
                                    pending_invocation_oplog_index,
                                    acceptance: attached_acceptance,
                                });
                            let summary = DurableStreamEventSummary::session(&record);
                            OplogEntry::stream_session(
                                None,
                                OplogPayload::Inline(Box::new(record)),
                                summary,
                            )
                        }),
                    )
                    .await?;
                streams.commit_consumer_journal().await.map_err(|error| {
                    error.into_worker_executor_error(WorkerExecutorError::runtime)
                })?;
            }
            for mapping in persisted_foreign_mappings.iter() {
                if streams
                    .has_journaled_consumer_terminal(mapping)
                    .await
                    .map_err(|error| error.into_worker_executor_error(WorkerExecutorError::runtime))?
                {
                    continue;
                }
                let mut retry_delay = Duration::from_millis(10);
                // Leave this guard before recovering from unreadable history. Retry ordinary
                // activation errors while the owner still has authority.
                loop {
                    if self.owner_retirement_requested.is_cancelled() {
                        return Err(self.retirement_error());
                    }
                    producer.ensure_healthy().map_err(|error| {
                        error.into_worker_executor_error(WorkerExecutorError::runtime)
                    })?;
                    match streams
                        .activate_foreign_mapping(mapping.clone(), initial_epoch)
                        .await
                    {
                        Ok(()) => break,
                        Err(error @ (SessionError::Fenced(_) | SessionError::Unreadable(_))) => {
                            return Err(
                                error.into_worker_executor_error(WorkerExecutorError::runtime)
                            );
                        }
                        Err(error) => {
                            warn!(
                                session = %prepared.attempt.session_key.idempotency_key,
                                stream = %mapping.handle.stream_id,
                                %error,
                                "retrying durable foreign topology activation after attachment commit"
                            );
                            tokio::time::sleep(retry_delay).await;
                            retry_delay = (retry_delay * 2).min(Duration::from_secs(1));
                        }
                    }
                }
            }
            match receipt {
                None => {}
                Some(StreamReceiptAction::Notify(committed)) => {
                    let _ = committed.send(());
                }
                Some(StreamReceiptAction::CommitAndNotify(committed)) => {
                    self.state_actor
                        .commit_and_update_state_notifying(
                            CommitLevel::Always,
                            state_actor::CommitReceipt::Plain(committed),
                        )
                        .await
                        .map_err(|refusal| self.refused_by(refusal))?;
                }
            }
            if !replayed && let WorkerInstance::Running(running) = instance_guard.state() {
                running.sender.send(WorkerCommand::WorkAvailable).unwrap();
            }
            Ok(DurableStreamingInvocationAcceptance {
                prepared,
                streams,
                replayed,
                joined_origin_observer,
            })
        }
        .await;
        Ok(StreamAttachmentCompletion {
            observation,
            acceptance,
        })
    }

    async fn observe_stream_admission_ownership(&self) -> StreamAdmissionOwnership {
        let deleting = self.deletion_owns_retirement().await;
        StreamAdmissionOwnership {
            lost_shard: self
                .retired_for_lost_shard()
                .then(|| self.retirement_error()),
            deleting,
            retired: self
                .owner_retirement_requested
                .is_cancelled()
                .then(|| self.retirement_error()),
            assignment: self.shard_service().check_worker(&self.agent_id()),
        }
    }

    pub(super) async fn resolve_committed_invocation_failure(
        self: &Arc<Self>,
        error: WorkerExecutorError,
    ) -> WorkerExecutorError {
        let wake = match StreamRecoveryWake::after_committed_read(error) {
            Ok(wake) => wake,
            Err(refusal) => return refusal,
        };
        if let Some(refusal) = self.observe_stream_admission_ownership().await.refusal() {
            return refusal;
        }

        // The owned admission task runs this wake even if its waiter is gone.
        let start = Worker::start_if_needed(self.clone()).await.map(|_| ());
        let failure = match wake.after_start(start) {
            ControlFlow::Continue(failure) => failure,
            ControlFlow::Break(refusal) => return refusal,
        };
        if let Some(refusal) = self.observe_stream_admission_ownership().await.refusal() {
            return refusal;
        }
        warn!(
            agent_id = %self.agent_id(),
            error = %failure.start_error,
            "Failed to wake a committed invocation after a history read error"
        );
        failure.read_error
    }

    /// Resumes or takes over an existing durable streaming invocation.
    pub async fn resume_durable_streaming_invocation(
        self: &Arc<Self>,
        attempt: ResumeAttemptDescriptor,
    ) -> Result<DurableStreamingResumeAcceptance, WorkerExecutorError> {
        let operation = match attempt.operation {
            golem_common::model::durable_stream::StreamResumeOperation::Resume => "resume",
            golem_common::model::durable_stream::StreamResumeOperation::Takeover => "takeover",
        };
        let response_lease = if self.agent_mode() == AgentMode::Ephemeral {
            Some(
                self.durable_stream_producer
                    .retain_response()
                    .map_err(|error| {
                        error.into_worker_executor_error(WorkerExecutorError::runtime)
                    })?,
            )
        } else {
            None
        };
        let result = self
            .resume_durable_streaming_invocation_unmetered(attempt)
            .await
            .map(|mut acceptance| {
                acceptance.streams = acceptance.streams.with_response_lease(response_lease);
                acceptance
            });
        match &result {
            Ok(acceptance) => crate::metrics::durable_stream::record_attempt(
                operation,
                if acceptance.replayed {
                    "replayed"
                } else {
                    "accepted"
                },
                (!acceptance.replayed).then_some(acceptance.epoch),
            ),
            Err(error) => crate::metrics::durable_stream::record_attempt(
                operation,
                durable_stream_attempt_error_outcome(error),
                None,
            ),
        }
        result
    }
    pub(in crate::worker) async fn resume_durable_streaming_invocation_unmetered(
        self: &Arc<Self>,
        attempt: ResumeAttemptDescriptor,
    ) -> Result<DurableStreamingResumeAcceptance, WorkerExecutorError> {
        let producer = self.durable_stream_producer().await?;
        let worker = self.clone();
        let memory = golem_common::serialization::serialize(&attempt)
            .map_err(WorkerExecutorError::runtime)?
            .len();
        let acceptance = producer
            .run_admitted(None, memory, true, move |producer, admission| async move {
                Ok::<_, crate::durable_host::durable_stream::SessionError>(
                    worker
                        .resume_durable_streaming_invocation_owned(attempt, producer, &admission)
                        .await,
                )
            })
            .await
            .map_err(|error| error.into_worker_executor_error(WorkerExecutorError::runtime))??;
        let activate_foreign = if acceptance.replayed {
            match acceptance.streams.ensure_current_attachment().await {
                Ok(()) => true,
                Err(error @ (SessionError::Unreadable(_) | SessionError::Fenced(_))) => {
                    return Err(error.into_worker_executor_error(WorkerExecutorError::runtime));
                }
                Err(_) => false,
            }
        } else {
            true
        };
        if activate_foreign {
            for mapping in &acceptance.mappings {
                if !producer.owns_handle_identity(&mapping.handle) {
                    if mapping.role.direction() == SessionStreamRole::Input
                        && acceptance
                            .streams
                            .has_journaled_consumer_terminal(mapping)
                            .await
                            .map_err(|error| {
                                error.into_worker_executor_error(WorkerExecutorError::runtime)
                            })?
                    {
                        continue;
                    }
                    acceptance
                        .streams
                        .prepare_foreign_mapping(mapping.clone(), acceptance.epoch)
                        .await
                        .map_err(|error| {
                            error.into_worker_executor_error(WorkerExecutorError::runtime)
                        })?;
                    acceptance
                        .streams
                        .activate_foreign_mapping(mapping.clone(), acceptance.epoch)
                        .await
                        .map_err(|error| {
                            error.into_worker_executor_error(WorkerExecutorError::runtime)
                        })?;
                }
            }
        }
        Ok(acceptance)
    }

    async fn resume_durable_streaming_invocation_owned(
        self: &Arc<Self>,
        attempt: ResumeAttemptDescriptor,
        producer: Arc<crate::durable_host::durable_stream::DurableStreamStore>,
        admission: &Arc<crate::durable_host::durable_stream::StreamWriteAdmission>,
    ) -> Result<DurableStreamingResumeAcceptance, WorkerExecutorError> {
        {
            let instance = self.lock_non_stopping_worker().await;
            if instance.ensure_not_deleting().is_err() {
                return Err(WorkerExecutorError::invalid_request(
                    "Cannot resume a deleting worker",
                ));
            }
            if let Some(error) = instance.startup_failure() {
                return Err(error.clone());
            }
        }
        let session_guard = producer
            .session_lock(&attempt.session_key)
            .lock_owned()
            .await;
        let instance_guard = self.lock_non_stopping_worker().await;
        if instance_guard.ensure_not_deleting().is_err() {
            return Err(WorkerExecutorError::invalid_request(
                "Cannot resume a deleting worker",
            ));
        }
        if let Some(error) = instance_guard.startup_failure() {
            return Err(error.clone());
        }

        let sessions::RetainedStreamSessionRecords {
            records,
            acceptance_mappings,
        } = self
            .stream_session_records(&attempt.session_key, Some(attempt.attempt_id))
            .await?;
        let prepared = records
            .iter()
            .find_map(|(_, record)| match record {
                StreamSessionRecord::Prepared(record) => Some(record.clone()),
                _ => None,
            })
            .ok_or_else(|| {
                WorkerExecutorError::invalid_request(
                    "NotFound: durable Stream Session preparation was not found",
                )
            })?;
        if prepared.attempt.attempt_id == attempt.attempt_id {
            return Err(WorkerExecutorError::invalid_request(
                "AttemptConflict: a Start attempt ID cannot be reused for resume or takeover",
            ));
        }

        let mut mappings = prepared.stream_mappings.clone();
        for (_, record) in &records {
            match record {
                StreamSessionRecord::Mapping(record) => {
                    if !mappings.contains(&record.mapping) {
                        mappings.push(record.mapping.clone());
                    }
                }
                StreamSessionRecord::InvocationResult(record) => {
                    for mapping in &record.stream_mappings {
                        if !mappings.contains(mapping) {
                            mappings.push(mapping.clone());
                        }
                    }
                }
                StreamSessionRecord::TopologyPrepared(record) => {
                    let binding = StreamBindingRecord::foreign(&record.mapping);
                    if !mappings.contains(&binding) {
                        mappings.push(binding);
                    }
                }
                StreamSessionRecord::TopologyActivated(record) => {
                    let binding = StreamBindingRecord::foreign(&record.mapping);
                    if !mappings.contains(&binding) {
                        mappings.push(binding);
                    }
                }
                StreamSessionRecord::ConsumerItemValue(record) if record.reobserves.is_none() => {
                    for mapping in &record.recursive_mappings {
                        if !mappings.contains(mapping) {
                            mappings.push(mapping.clone());
                        }
                    }
                }
                _ => {}
            }
        }
        acceptance_mappings.into_iter().for_each(|mapping| {
            if !mappings.contains(&mapping) {
                mappings.push(mapping);
            }
        });

        let materialized_mappings = producer
            .materialize_bindings(&mappings)
            .await
            .map_err(|error| error.into_worker_executor_error(WorkerExecutorError::runtime))?;
        let base_streams = StreamSession::open(
            producer.clone(),
            self.oplog.clone(),
            StreamRegistrationInvocation::Local(attempt.session_key.idempotency_key),
            mappings.iter().cloned(),
        )
        .await
        .map_err(|error| error.into_worker_executor_error(WorkerExecutorError::runtime))?
        .with_rpc(self.rpc())
        .with_consumer_journal(self.durable_stream_consumer_journal())
        .with_auth_ctx(self.durable_stream_consumer_auth_ctx()?);
        let make_streams = |epoch, attempt_id| -> Result<StreamSession, WorkerExecutorError> {
            Ok(base_streams.clone().with_attachment(epoch, attempt_id))
        };

        for (_, record) in &records {
            if let StreamSessionRecord::ResumeAttempt(existing) = record
                && existing.attempt.attempt_id == attempt.attempt_id
            {
                if existing.attempt != attempt {
                    return Err(WorkerExecutorError::invalid_request(
                        "AttemptConflict: the resume attempt does not exactly match its persisted descriptor",
                    ));
                }
                let streams = make_streams(existing.accepted_epoch, attempt.attempt_id)?;
                drop(instance_guard);
                drop(session_guard);
                return Ok(DurableStreamingResumeAcceptance {
                    prepared,
                    mappings: materialized_mappings,
                    streams,
                    epoch: existing.accepted_epoch,
                    replayed: true,
                });
            }
        }

        if attempt.expected_callee_fingerprint != self.get_initial_worker_metadata().fingerprint
            || attempt.expected_callee_fingerprint != prepared.attempt.expected_callee_fingerprint
            || attempt.attachment_id != prepared.attempt.attachment_id
        {
            return Err(WorkerExecutorError::invalid_request(
                "IncarnationMismatch: resume does not identify the persisted callee attachment",
            ));
        }
        if attempt.effective_identity != prepared.attempt.effective_identity {
            return Err(WorkerExecutorError::invalid_request(
                "Unauthorized: resume identity differs from the pinned caller and grant",
            ));
        }

        let crate::durable_host::durable_session::AuthoritativeAttachmentState {
            epoch: current_epoch,
            attempt_id: current_attempt_id,
            attached,
        } = make_streams(attempt.expected_epoch, attempt.attempt_id)?
            .authoritative_attachment_state()
            .await
            .map_err(|error| error.into_worker_executor_error(WorkerExecutorError::runtime))?;
        if attempt.expected_epoch < current_epoch {
            return Err(WorkerExecutorError::invalid_request(format!(
                "StaleEpoch: current attachment epoch is {current_epoch}"
            )));
        }
        if attempt.expected_epoch > current_epoch {
            return Err(WorkerExecutorError::invalid_request(format!(
                "InvalidEpoch: current attachment epoch is {current_epoch}"
            )));
        }
        match (attempt.operation, attached) {
            (golem_common::model::durable_stream::StreamResumeOperation::Resume, false)
            | (golem_common::model::durable_stream::StreamResumeOperation::Takeover, true) => {}
            _ => {
                return Err(WorkerExecutorError::invalid_request(
                    "InvalidAttachmentState: resume requires Detached and takeover requires Attached",
                ));
            }
        }
        let current_streams = make_streams(current_epoch, current_attempt_id)?;
        current_streams
            .validate_resume_cursors(&attempt.cursors)
            .await
            .map_err(|error| {
                error.into_worker_executor_error(WorkerExecutorError::invalid_request)
            })?;

        let accepted_epoch = current_epoch.checked_add(1).ok_or_else(|| {
            WorkerExecutorError::invalid_request(
                "ResourceExhausted: durable attachment epoch cannot wrap",
            )
        })?;
        current_streams
            .commit_resume_attempt_locked(
                StreamSessionResumeAttemptRecord {
                    format_version: DURABLE_STREAM_FORMAT_VERSION,
                    session_key: attempt.session_key.idempotency_key,
                    attempt: attempt.clone(),
                    accepted_epoch,
                },
                session_guard,
                admission,
            )
            .await
            .map_err(|error| {
                error.into_worker_executor_error(WorkerExecutorError::invalid_request)
            })?;
        drop(instance_guard);

        let streams = make_streams(accepted_epoch, attempt.attempt_id)?;
        Ok(DurableStreamingResumeAcceptance {
            prepared,
            mappings: materialized_mappings,
            streams,
            epoch: accepted_epoch,
            replayed: false,
        })
    }
}

pub(in crate::worker) mod sessions;

#[cfg(test)]
mod tests {
    use super::retained_acceptance_matches;
    use golem_common::model::durable_stream::{
        AttachmentId, AttemptId, StreamSessionAcceptance, StreamSessionAttachedRecord,
        StreamSessionExpiryPolicy,
    };
    use golem_common::model::{InvocationId, OplogIndex};
    use test_r::test;

    fn acceptance(route_scope: Option<&str>) -> StreamSessionAcceptance {
        StreamSessionAcceptance {
            public_session_id: "session".to_string(),
            route_scope: route_scope.map(str::to_string),
            expiry_policy: StreamSessionExpiryPolicy::Sliding { ttl_seconds: 60 },
            expiry_deadline_millis: Some(1_000),
            effective_identity: vec![1, 2],
            execution_config: vec![3, 4],
            live_join_buffer_events: 16,
            transport_stream_ids: vec![1],
        }
    }

    fn retained(
        acceptance: StreamSessionAcceptance,
        attempt_id: AttemptId,
    ) -> StreamSessionAttachedRecord {
        StreamSessionAttachedRecord {
            format_version: 1,
            session_key: InvocationId(uuid::Uuid::from_u128(1)),
            attachment_id: AttachmentId(uuid::Uuid::from_u128(2)),
            attempt_id,
            epoch: 1,
            pending_invocation_oplog_index: OplogIndex::from_u64(3),
            acceptance,
        }
    }

    #[test]
    fn a_route_session_replay_with_a_fresh_attempt_and_deadline_matches() {
        let retained = retained(
            acceptance(Some("route")),
            AttemptId(uuid::Uuid::from_u128(10)),
        );
        let requested = StreamSessionAcceptance {
            expiry_deadline_millis: Some(2_000),
            ..acceptance(Some("route"))
        };
        assert!(retained_acceptance_matches(
            &retained,
            &requested,
            AttemptId(uuid::Uuid::from_u128(11))
        ));
    }

    #[test]
    fn a_route_session_with_another_execution_configuration_does_not_match() {
        let retained = retained(
            acceptance(Some("route")),
            AttemptId(uuid::Uuid::from_u128(10)),
        );
        for requested in [
            StreamSessionAcceptance {
                execution_config: vec![3, 5],
                ..acceptance(Some("route"))
            },
            StreamSessionAcceptance {
                expiry_policy: StreamSessionExpiryPolicy::Sliding { ttl_seconds: 61 },
                ..acceptance(Some("route"))
            },
            StreamSessionAcceptance {
                live_join_buffer_events: 17,
                ..acceptance(Some("route"))
            },
            StreamSessionAcceptance {
                effective_identity: vec![1, 3],
                ..acceptance(Some("route"))
            },
        ] {
            assert!(
                !retained_acceptance_matches(
                    &retained,
                    &requested,
                    AttemptId(uuid::Uuid::from_u128(10))
                ),
                "{requested:?}"
            );
        }
    }

    #[test]
    fn a_session_without_a_route_must_repeat_the_attempt_and_the_deadline() {
        let attempt = AttemptId(uuid::Uuid::from_u128(10));
        let retained = retained(acceptance(None), attempt);
        assert!(retained_acceptance_matches(
            &retained,
            &acceptance(None),
            attempt
        ));
        assert!(!retained_acceptance_matches(
            &retained,
            &acceptance(None),
            AttemptId(uuid::Uuid::from_u128(11))
        ));
        assert!(!retained_acceptance_matches(
            &retained,
            &StreamSessionAcceptance {
                expiry_deadline_millis: Some(2_000),
                ..acceptance(None)
            },
            attempt
        ));
    }
}
