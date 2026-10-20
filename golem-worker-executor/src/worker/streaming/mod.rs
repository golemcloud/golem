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

impl<Ctx: WorkerCtx> Worker<Ctx> {
    pub(in crate::worker) async fn pending_streaming_invocation_entry(
        &self,
        invocation: AgentInvocation,
    ) -> Result<OplogEntry, WorkerExecutorError> {
        self.accept_ephemeral_invocation(&invocation)?;
        let (idempotency_key, invocation_payload, invocation_context) = invocation.into_parts();
        let invocation_context = invocation_context
            .limit_depth(self.deps.config().limits.max_invocation_context_stack_depth);
        let payload = self
            .oplog
            .upload_payload_owned(invocation_payload)
            .await
            .map_err(|error| {
                WorkerExecutorError::invalid_request(format!(
                    "Failed to upload invocation payload: {error}"
                ))
            })?;
        let invocation_context_spans = invocation_context.to_oplog_data();
        Ok(OplogEntry::pending_agent_invocation(
            idempotency_key,
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
        instance_guard: OwnedMutexGuard<WorkerInstance>,
    ) -> Result<DurableStreamingInvocationAcceptance, WorkerExecutorError> {
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
        let records = self.stream_session_records(&session_key, None).await?;
        let status = self
            .oplog
            .raw_durable_stream_session_status(&session_key)
            .await
            .status
            .map_err(WorkerExecutorError::runtime)?;
        let initial_epoch = match records.iter().find_map(|record| match record {
            StreamSessionRecord::Attached(attached) => Some(attached.epoch),
            _ => None,
        }) {
            Some(epoch) => status
                .as_ref()
                .and_then(|status| status.attachment_epoch)
                .unwrap_or(epoch),
            None => producer.attachment_epoch_floor(),
        };
        let mut prepared_records = records.iter().filter_map(|record| match record {
            StreamSessionRecord::Prepared(prepared) => Some(prepared.clone()),
            _ => None,
        });
        let existing_prepared = prepared_records.next();
        if existing_prepared.is_some() && prepared_records.next().is_some() {
            return Err(WorkerExecutorError::runtime(
                "durable Stream Session contains multiple Prepared records",
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
        let mut invocation = Some(request.invocation);
        let mut acceptance_committed = Some(request.acceptance_committed);
        let mut attached_during_prepare = false;
        let mut joined_origin = false;
        if !request.registrations.is_empty() && !request.foreign_mappings.is_empty() {
            return Err(WorkerExecutorError::invalid_request(
                "durable invocation inputs cannot mix inline and agent-hosted stream mappings",
            ));
        }
        let foreign_mappings = request.foreign_mappings.clone();
        for mapping in &foreign_mappings {
            if producer.owns_handle_identity(&mapping.handle) {
                producer
                    .validate_handle(&mapping.handle)
                    .await
                    .map_err(|error| WorkerExecutorError::invalid_request(error.to_string()))?;
            }
        }
        let expiry_publication_guard = if existing_prepared.is_none() {
            let guard = producer.session_lock(&session_key).lock_owned().await;
            self.schedule_stream_session_expiry(
                request.public_session_id.clone(),
                session_key.idempotency_key.clone(),
                request.expiry_deadline_millis,
            )
            .await?;
            Some(guard)
        } else {
            None
        };

        let prepared = if let Some(prepared) = existing_prepared {
            let mut requested_attempt = request.attempt.clone();
            let requested_handles = if foreign_mappings.is_empty() {
                let mut handles = Vec::with_capacity(request.registrations.len());
                for (_, registration) in &request.registrations {
                    handles.push(
                        producer
                            .validate_registration(registration)
                            .await
                            .map_err(|_| {
                                WorkerExecutorError::invalid_request(
                                    "IdempotencyConflict: persisted durable stream registration does not match the invocation descriptor",
                                )
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
            joined_origin = matches!(
                acceptance_match,
                DurableStreamingAcceptanceMatch::ExactAttempt
            ) && request.registrations.is_empty()
                && (!stream_attempt_matches(&prepared.attempt, &requested_attempt)
                    || prepared.attempt.invocation.stream_handles
                        != requested_attempt.invocation.stream_handles)
                && same_origin_stream_invocation(&prepared.attempt, &requested_attempt)
                && prepared.stream_mappings.len() == foreign_mappings.len()
                && prepared
                    .stream_mappings
                    .iter()
                    .zip(&foreign_mappings)
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
            prepared
        } else if !foreign_mappings.is_empty() {
            let mut attempt = request.attempt.clone();
            attempt.invocation.stream_handles = foreign_mappings
                .iter()
                .map(|mapping| mapping.handle.clone())
                .collect();
            let prepared = StreamSessionPreparedRecord {
                format_version: 1,
                session_key: session_key.idempotency_key.clone(),
                public_session_id: request.public_session_id.clone(),
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
                StreamRegistrationInvocation::Local(session_key.idempotency_key.clone()),
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
                .pending_streaming_invocation_entry(
                    invocation
                        .take()
                        .expect("fresh durable session has an invocation"),
                )
                .await?;
            let prepared = producer
                .prepare_session(
                    None,
                    Vec::new(),
                    topologies,
                    pending,
                    acceptance_committed
                        .take()
                        .expect("fresh durable session has a commit notification"),
                    move |_| prepared,
                )
                .await
                .map_err(|error| {
                    error.into_worker_executor_error(WorkerExecutorError::invalid_request)
                })?;
            attached_during_prepare = true;
            prepared
        } else {
            let pending = self
                .pending_streaming_invocation_entry(
                    invocation
                        .take()
                        .expect("fresh durable session has an invocation"),
                )
                .await?;
            let attempt = request.attempt.clone();
            let local_session_key = session_key.idempotency_key.clone();
            let public_session_id = request.public_session_id.clone();
            let expiry_policy = request.expiry_policy;
            let expiry_deadline_millis = request.expiry_deadline_millis;
            let prepared = producer
                .prepare_session(
                    None,
                    request.registrations.clone(),
                    Vec::new(),
                    pending,
                    acceptance_committed
                        .take()
                        .expect("fresh durable session has a commit notification"),
                    move |bindings| StreamSessionPreparedRecord {
                        format_version: 1,
                        public_session_id,
                        session_key: local_session_key,
                        expiry_policy,
                        expiry_deadline_millis,
                        stream_mappings: bindings,
                        attempt,
                    },
                )
                .await
                .map_err(|error| error.into_worker_executor_error(WorkerExecutorError::runtime))?;
            attached_during_prepare = true;
            prepared
        };
        drop(expiry_publication_guard);

        let mut attached_records = records.iter().filter_map(|record| match record {
            StreamSessionRecord::Attached(attached) => Some(attached),
            _ => None,
        });
        let attached = attached_records.next();
        if attached.is_some() && attached_records.next().is_some() {
            return Err(WorkerExecutorError::runtime(
                "durable Stream Session contains multiple Attached records",
            ));
        }
        if let Some(attached) = attached
            && (attached.session_key != prepared.session_key
                || attached.attachment_id != prepared.attempt.attachment_id
                || attached.attempt_id != prepared.attempt.attempt_id
                || attached.epoch == 0
                || !matches!(
                    self.oplog.read(attached.pending_invocation_oplog_index).await?,
                    OplogEntry::PendingAgentInvocation { idempotency_key, .. }
                        if idempotency_key == prepared.attempt.session_key.idempotency_key
                ))
        {
            return Err(WorkerExecutorError::runtime(
                "durable Attached record does not exactly identify its Prepared attempt and pending invocation",
            ));
        }
        let already_attached = attached.is_some();
        let current_attempt = status.as_ref().is_some_and(|status| {
            status.attachment_attached == Some(true)
                && status.attachment_epoch == Some(initial_epoch)
                && status.attachment_attempt_id == Some(prepared.attempt.attempt_id)
        });
        let retained_acceptance = already_attached;
        if joined_origin && !retained_acceptance {
            return Err(WorkerExecutorError::runtime(
                "cannot join a streaming invocation without its retained pending invocation",
            ));
        }

        let persisted_foreign_bindings = if (retained_acceptance && !current_attempt)
            || joined_origin
            || foreign_mappings.is_empty()
            || records
                .iter()
                .any(|record| matches!(record, StreamSessionRecord::Finished(_)))
        {
            &[][..]
        } else {
            prepared.stream_mappings.as_slice()
        };
        let streams = StreamSession::open(
            producer.clone(),
            self.oplog.clone(),
            StreamRegistrationInvocation::Local(session_key.idempotency_key.clone()),
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
            .collect::<Vec<_>>();
        for mapping in &persisted_foreign_mappings {
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
        if !retained_acceptance && !attached_during_prepare {
            let pending = self
                .pending_streaming_invocation_entry(
                    invocation
                        .take()
                        .expect("unattached durable session has an invocation"),
                )
                .await?;
            let attached_attempt = prepared.attempt.clone();
            self.oplog
                .add_pair(
                    pending,
                    Box::new(move |pending_invocation_oplog_index| {
                        let record = StreamSessionRecord::Attached(StreamSessionAttachedRecord {
                            format_version: 1,
                            session_key: attached_attempt.session_key.idempotency_key,
                            attachment_id: attached_attempt.attachment_id,
                            attempt_id: attached_attempt.attempt_id,
                            epoch: initial_epoch,
                            pending_invocation_oplog_index,
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
            streams
                .commit_consumer_journal()
                .await
                .map_err(|error| error.into_worker_executor_error(WorkerExecutorError::runtime))?;
        }
        for mapping in &persisted_foreign_mappings {
            if streams
                .has_journaled_consumer_terminal(mapping)
                .await
                .map_err(|error| error.into_worker_executor_error(WorkerExecutorError::runtime))?
            {
                continue;
            }
            let mut retry_delay = Duration::from_millis(10);
            // Not bounded: the pending invocation and its attachment are already committed, so
            // stopping would report a failure for an invocation the agent still runs. Only an
            // agent this executor no longer owns stops retrying.
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
                    // A refusal is permanent. Retrying it would hold the worker lifecycle lock
                    // forever, and the retirement that has to take that lock could never stop the
                    // agent.
                    Err(error @ SessionError::Fenced(_)) => {
                        return Err(error.into_worker_executor_error(WorkerExecutorError::runtime));
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
        if retained_acceptance {
            let _ = acceptance_committed
                .take()
                .expect("persisted durable session has a commit notification")
                .send(());
        } else if !attached_during_prepare {
            // Refused, the invocation is not accepted: the dropped notification tells the waiter
            // nothing was committed, and the error keeps `Accepted` from being sent.
            self.state_actor
                .commit_and_update_state_notifying(
                    CommitLevel::Always,
                    state_actor::CommitReceipt::Plain(
                        acceptance_committed
                            .take()
                            .expect("legacy durable session has a commit notification"),
                    ),
                )
                .await
                .map_err(|refusal| self.refused_by(refusal))?;
        }
        if !retained_acceptance && let WorkerInstance::Running(running) = &*instance_guard {
            running.sender.send(WorkerCommand::WorkAvailable).unwrap();
        }
        drop(instance_guard);
        if matches!(
            self.lookup_invocation_result(&prepared.attempt.session_key.idempotency_key)
                .await,
            LookupResult::Pending
        ) {
            Worker::start_if_needed(self.clone()).await?;
        }
        Ok(DurableStreamingInvocationAcceptance {
            prepared,
            streams,
            replayed: retained_acceptance,
            joined_origin_observer: joined_origin,
        })
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
        let producer = self.durable_stream_producer().await?;
        let instance_guard = self.lock_non_stopping_worker().await;
        if instance_guard.ensure_not_deleting().is_err() {
            return Err(WorkerExecutorError::invalid_request(
                "Cannot resume a deleting worker",
            ));
        }
        if let Some(error) = instance_guard.startup_failure() {
            return Err(error.clone());
        }

        let records = self
            .stream_session_records(&attempt.session_key, Some(attempt.attempt_id))
            .await?;
        let prepared = records
            .iter()
            .find_map(|record| match record {
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
        for record in &records {
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

        let materialized_mappings = producer
            .materialize_bindings(&mappings)
            .await
            .map_err(|error| error.into_worker_executor_error(WorkerExecutorError::runtime))?;
        let base_streams = StreamSession::open(
            producer.clone(),
            self.oplog.clone(),
            StreamRegistrationInvocation::Local(attempt.session_key.idempotency_key.clone()),
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

        for record in &records {
            if let StreamSessionRecord::ResumeAttempt(existing) = record
                && existing.attempt.attempt_id == attempt.attempt_id
            {
                if existing.attempt != attempt {
                    return Err(WorkerExecutorError::invalid_request(
                        "AttemptConflict: the resume attempt does not exactly match its persisted descriptor",
                    ));
                }
                let streams = make_streams(existing.accepted_epoch, attempt.attempt_id)?;
                if streams.ensure_current_attachment().await.is_ok() {
                    for (binding, mapping) in mappings.iter().zip(&materialized_mappings) {
                        if matches!(binding.source, StreamRecordReference::Foreign(_))
                            && !producer.owns_handle_identity(&mapping.handle)
                        {
                            if mapping.role.direction() == SessionStreamRole::Input
                                && streams
                                    .has_journaled_consumer_terminal(mapping)
                                    .await
                                    .map_err(|error| {
                                        error.into_worker_executor_error(
                                            WorkerExecutorError::runtime,
                                        )
                                    })?
                            {
                                continue;
                            }
                            streams
                                .prepare_foreign_mapping(mapping.clone(), existing.accepted_epoch)
                                .await
                                .map_err(|error| {
                                    error.into_worker_executor_error(WorkerExecutorError::runtime)
                                })?;
                            streams
                                .activate_foreign_mapping(mapping.clone(), existing.accepted_epoch)
                                .await
                                .map_err(|error| {
                                    error.into_worker_executor_error(WorkerExecutorError::runtime)
                                })?;
                        }
                    }
                }
                drop(instance_guard);
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
                "Unauthorized: resume identity differs from the pinned principal and grant",
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
            .commit_resume_attempt(StreamSessionResumeAttemptRecord {
                format_version: DURABLE_STREAM_FORMAT_VERSION,
                session_key: attempt.session_key.idempotency_key.clone(),
                attempt: attempt.clone(),
                accepted_epoch,
            })
            .await
            .map_err(|error| {
                error.into_worker_executor_error(WorkerExecutorError::invalid_request)
            })?;

        let streams = make_streams(accepted_epoch, attempt.attempt_id)?;
        for (binding, mapping) in mappings.iter().zip(&materialized_mappings) {
            if matches!(binding.source, StreamRecordReference::Foreign(_))
                && !producer.owns_handle_identity(&mapping.handle)
            {
                if mapping.role.direction() == SessionStreamRole::Input
                    && streams
                        .has_journaled_consumer_terminal(mapping)
                        .await
                        .map_err(|error| {
                            error.into_worker_executor_error(WorkerExecutorError::runtime)
                        })?
                {
                    continue;
                }
                streams
                    .prepare_foreign_mapping(mapping.clone(), accepted_epoch)
                    .await
                    .map_err(|error| {
                        error.into_worker_executor_error(WorkerExecutorError::runtime)
                    })?;
                streams
                    .activate_foreign_mapping(mapping.clone(), accepted_epoch)
                    .await
                    .map_err(|error| {
                        error.into_worker_executor_error(WorkerExecutorError::runtime)
                    })?;
            }
        }
        drop(instance_guard);
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
