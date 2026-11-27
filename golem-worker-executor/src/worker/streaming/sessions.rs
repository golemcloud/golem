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

use golem_common::model::InvocationId;
use golem_common::model::caller::{Caller, Principal};
use golem_common::model::durable_stream::StreamInvocationId;

use crate::worker::*;

/// The persisted execution configuration of a stream session: the encoded `InvocationStart`
/// without its identity fields, the sorted environment, and the stdout and stderr flags.
type SessionExecutionConfig = (Vec<u8>, Option<Vec<(String, String)>>, bool, bool);

impl<Ctx: WorkerCtx> Worker<Ctx> {
    pub(crate) async fn selected_method_preparation(
        &self,
        invocation: &AgentInvocation,
        input: Option<&golem_common::schema::SchemaValue>,
        component: &golem_service_base::model::component::Component,
    ) -> Result<Option<(DurableStreamingInvocationRequest, Option<String>)>, WorkerExecutorError>
    {
        let AgentInvocation::AgentMethod {
            idempotency_key,
            method_name,
            input: original,
            caller,
            ..
        } = invocation
        else {
            return Ok(None);
        };
        if self
            .prepared_stream_session(idempotency_key)
            .await?
            .is_some()
        {
            return Ok(None);
        }
        let key = StreamInvocationId {
            callee_environment_id: self.owned_agent_id.environment_id,
            callee: self.owned_agent_id.agent_id,
            callee_fingerprint: self.initial_worker_metadata.fingerprint,
            idempotency_key: *idempotency_key,
        };
        let Some(attached) = self
            .stream_session_records(&key, None)
            .await?
            .into_iter()
            .find_map(|record| {
                if let StreamSessionRecord::Attached(attached) = record {
                    Some(attached)
                } else {
                    None
                }
            })
        else {
            return Ok(None);
        };
        let (configuration, environment, _, _): SessionExecutionConfig =
            golem_common::serialization::decode(&attached.acceptance.execution_config)
                .map_err(|error| WorkerExecutorError::runtime(error.to_string()))?;
        let mut request =
            golem_api_grpc::proto::golem::worker::InvocationStart::decode(configuration.as_slice())
                .map_err(|error| WorkerExecutorError::runtime(error.to_string()))?;
        if let Some(environment) = environment {
            request.context.get_or_insert_default().env = environment.into_iter().collect();
        }
        request.environment_id = Some(self.owned_agent_id.environment_id.into());
        request.attempt_id = Some(attached.attempt_id.0.into());
        request.expected_callee_fingerprint =
            Some(self.initial_worker_metadata.fingerprint.0.into());
        request.caller = Some(caller.clone().into());
        request.method_name = Some(method_name.clone());
        let agent_type = self
            .initial_worker_metadata
            .agent_type_name
            .as_ref()
            .and_then(|name| component.metadata.find_agent_type_by_name_ref(name))
            .ok_or_else(|| WorkerExecutorError::invalid_request("selected agent type not found"))?;
        let method = agent_type
            .methods
            .iter()
            .find(|method| &method.name == method_name)
            .ok_or_else(|| WorkerExecutorError::invalid_request("selected method not found"))?;
        let root = golem_schema::schema::SchemaType::record(
            method
                .input_schema
                .fields()
                .iter()
                .filter(|field| {
                    matches!(
                        field.source,
                        golem_common::schema::agent::FieldSource::UserSupplied
                    )
                })
                .map(|field| golem_schema::schema::NamedFieldType {
                    name: field.name.clone(),
                    body: field.schema.clone(),
                    metadata: field.metadata.clone(),
                })
                .collect(),
        );
        let transport_ids = &attached.acceptance.transport_stream_ids;
        request.input = Some(
            golem_common::schema::streaming::encode_recursive_stream_value_with_schema(
                input.ok_or_else(|| {
                    WorkerExecutorError::runtime("selected method has no decoded input")
                })?,
                &agent_type.schema,
                &root,
                |stream, _| {
                    stream.with_host_endpoint::<crate::grpc::TransportStreamId, _>(|id| {
                        transport_ids.get(id.0 as usize).copied().unwrap_or(id.0)
                    })
                },
            )
            .map_err(WorkerExecutorError::invalid_request)?,
        );
        if let golem_common::model::agent::AgentMethodInput::Native(original) = original {
            request.durable_input_mappings = original
                .streams()
                .iter()
                .enumerate()
                .filter_map(|(ordinal, handle)| {
                    handle.as_ref().map(|handle| {
                        crate::durable_host::durable_session::durable_stream_mapping_to_proto(
                            &StreamSessionMappingRecord {
                                transport_stream_id: transport_ids
                                    .get(ordinal)
                                    .copied()
                                    .unwrap_or(ordinal as u64),
                                handle: handle.clone(),
                                role: SessionStreamRole::Input,
                            },
                            None,
                        )
                    })
                })
                .collect();
        }
        let (committed, _) = tokio::sync::oneshot::channel();
        let mut prepared = crate::grpc::build_durable_streaming_request(
            &request,
            self.owned_agent_id.agent_id,
            self.initial_worker_metadata.agent_type_name.as_ref(),
            &component.metadata,
            component.revision,
            self.initial_worker_metadata.fingerprint,
            invocation.clone(),
            request
                .input
                .as_ref()
                .map(Message::encoded_len)
                .unwrap_or_default(),
            committed,
            attached.acceptance.live_join_buffer_events as usize,
        )?;
        prepared.attempt.effective_identity = attached.acceptance.effective_identity.clone();
        prepared.attempt.invocation.effective_identity = attached.acceptance.effective_identity;
        prepared.attempt.invocation.execution_config = attached.acceptance.execution_config;
        prepared.public_session_id = attached.acceptance.public_session_id;
        prepared.expiry_policy = attached.acceptance.expiry_policy;
        prepared.expiry_deadline_millis = attached.acceptance.expiry_deadline_millis;
        Ok(Some((prepared, attached.acceptance.route_scope)))
    }

    pub(crate) async fn stream_session_records(
        &self,
        session_key: &golem_common::base_model::durable_stream::StreamSessionKey,
        attempt: Option<AttemptId>,
    ) -> Result<Vec<StreamSessionRecord>, WorkerExecutorError> {
        let service = self.worker_service();
        let mut metadata = service
            .lookup_durable_stream_control_metadata(
                &self.owned_agent_id,
                self.agent_mode(),
                self.initial_worker_metadata.fingerprint,
                session_key,
            )
            .await
            .map_err(|error| error.into_worker_executor_error(WorkerExecutorError::runtime))?;
        let mut resume_offset = match attempt {
            Some(attempt) => service
                .lookup_durable_stream_resume_offset(
                    &self.owned_agent_id,
                    self.agent_mode(),
                    self.initial_worker_metadata.fingerprint,
                    session_key,
                    attempt,
                )
                .await
                .map_err(|error| error.into_worker_executor_error(WorkerExecutorError::runtime))?,
            None => None,
        };
        let current = self.oplog.current_oplog_index().await;
        'suffix: while metadata.covered_through() < current {
            let entries = self
                .oplog
                .read_exact(
                    metadata.covered_through().next(),
                    (current.as_u64() - metadata.covered_through().as_u64()).min(1024),
                )
                .await?;
            for (index, entry) in entries {
                if let OplogEntry::AgentInvocationReturned {
                    invocation_id,
                    result,
                    ..
                } = &entry
                    && *invocation_id == session_key.idempotency_key
                    && StreamRegistrationInvocation::Local(*invocation_id).qualify(
                        self.owned_agent_id.environment_id,
                        &self.owned_agent_id.agent_id,
                        self.initial_worker_metadata.fingerprint,
                    ) == *session_key
                {
                    let result = self
                        .oplog
                        .download_payload(result.clone())
                        .await
                        .map_err(WorkerExecutorError::runtime)?;
                    metadata.apply_invocation_returned(index, &result.stream_bindings);
                } else if let OplogEntry::AgentInvocationFinished { invocation_id, .. } = &entry
                    && *invocation_id == session_key.idempotency_key
                    && StreamRegistrationInvocation::Local(*invocation_id).qualify(
                        self.owned_agent_id.environment_id,
                        &self.owned_agent_id.agent_id,
                        self.initial_worker_metadata.fingerprint,
                    ) == *session_key
                {
                    metadata.apply_invocation_returned(index, &[]);
                } else if let OplogEntry::StreamSession { record, .. } = entry {
                    let record = self
                        .oplog
                        .download_payload(record)
                        .await
                        .map_err(WorkerExecutorError::runtime)?;
                    if matches!(record, StreamSessionRecord::ForkCut(_)) {
                        metadata = service
                            .lookup_durable_stream_control_metadata(
                                &self.owned_agent_id,
                                self.agent_mode(),
                                self.initial_worker_metadata.fingerprint,
                                session_key,
                            )
                            .await
                            .map_err(|error| {
                                error.into_worker_executor_error(WorkerExecutorError::runtime)
                            })?;
                        if metadata.covered_through() < index {
                            return Err(WorkerExecutorError::runtime(
                                "fork marker is not committed to the session index",
                            ));
                        }
                        resume_offset = match attempt {
                            Some(attempt) => service
                                .lookup_durable_stream_resume_offset(
                                    &self.owned_agent_id,
                                    self.agent_mode(),
                                    self.initial_worker_metadata.fingerprint,
                                    session_key,
                                    attempt,
                                )
                                .await
                                .map_err(|error| {
                                    error.into_worker_executor_error(WorkerExecutorError::runtime)
                                })?,
                            None => None,
                        };
                        continue 'suffix;
                    }
                    if let StreamSessionRecord::ResumeAttempt(record) = &record
                        && record.session_key == session_key.idempotency_key
                        && Some(record.attempt.attempt_id) == attempt
                    {
                        resume_offset.get_or_insert(index);
                    }
                    metadata.apply(
                        index,
                        session_key,
                        &record,
                        self.owned_agent_id.environment_id,
                        &self.owned_agent_id.agent_id,
                        self.initial_worker_metadata.fingerprint,
                    );
                } else {
                    metadata.advance_coverage(index);
                }
            }
        }
        let mut records = Vec::new();
        for offset in [
            metadata.prepared_position(),
            metadata.attached_position(),
            resume_offset,
            metadata.finished_position(),
        ]
        .into_iter()
        .flatten()
        {
            let record = self.read_stream_session_record(offset).await?;
            if stream_session_record_key(
                &record,
                self.owned_agent_id.environment_id,
                &self.owned_agent_id.agent_id,
                self.initial_worker_metadata.fingerprint,
            ) != Some(session_key.clone())
            {
                return Err(WorkerExecutorError::runtime(
                    "session index references another session",
                ));
            }
            records.push(record);
        }
        for mapping in metadata
            .acceptance_mappings()
            .map_err(WorkerExecutorError::runtime)?
        {
            records.push(StreamSessionRecord::Mapping(
                golem_common::base_model::durable_stream::StreamSessionMappingUpdateRecord {
                    format_version: 1,
                    session_key: StreamRegistrationInvocation::Remote(session_key.clone()),
                    mapping,
                },
            ));
        }
        Ok(records)
    }
    pub(in crate::worker) async fn durable_stream_session_status(
        &self,
        idempotency_key: &InvocationId,
    ) -> Result<Option<golem_common::model::DurableStreamSessionStatus>, WorkerExecutorError> {
        // Session expiry and stream-slot requests can reach a generation whose state actor a
        // deletion stopped without holding a task-scope bind.
        let status = self.try_get_last_known_status().await?;
        self.worker_service()
            .lookup_durable_stream_session(
                &self.owned_agent_id,
                self.agent_mode(),
                self.initial_worker_metadata.fingerprint,
                &status,
                idempotency_key,
            )
            .await
            .map_err(|error| error.into_worker_executor_error(WorkerExecutorError::runtime))
    }
    pub(in crate::worker) async fn read_stream_session_record(
        &self,
        index: OplogIndex,
    ) -> Result<StreamSessionRecord, WorkerExecutorError> {
        self.durable_stream_producer()
            .await?
            .read_session_record(index)
            .await
            .map_err(|error| error.into_worker_executor_error(WorkerExecutorError::runtime))
    }
    pub(crate) async fn prepared_stream_session(
        &self,
        idempotency_key: &InvocationId,
    ) -> Result<Option<StreamSessionPreparedRecord>, WorkerExecutorError> {
        let Some(index) = self
            .durable_stream_session_status(idempotency_key)
            .await?
            .and_then(|status| status.first_prepared)
        else {
            return Ok(None);
        };
        match self.read_stream_session_record(index).await? {
            StreamSessionRecord::Prepared(record) => Ok(Some(record)),
            _ => Err(WorkerExecutorError::runtime(
                "stream session index does not refer to Prepared",
            )),
        }
    }
    pub(crate) async fn native_tool_session(
        &self,
        idempotency_key: &InvocationId,
    ) -> Result<Option<(StreamSessionPreparedRecord, StreamSession)>, WorkerExecutorError> {
        let Some(prepared) = self.prepared_stream_session(idempotency_key).await? else {
            return Ok(None);
        };
        let streams = StreamSession::open(
            self.durable_stream_producer().await?,
            self.oplog.clone(),
            StreamRegistrationInvocation::Local(prepared.session_key),
            prepared.stream_mappings.iter().cloned(),
        )
        .await
        .map_err(|error| error.into_worker_executor_error(WorkerExecutorError::runtime))?
        .with_rpc(self.rpc())
        .with_consumer_journal(self.durable_stream_consumer_journal())
        .with_auth_ctx(self.durable_stream_consumer_auth_ctx()?);
        streams
            .recover_session_mappings()
            .await
            .map_err(|error| error.into_worker_executor_error(WorkerExecutorError::runtime))?;
        Ok(Some((prepared, streams)))
    }
    /// Reconstructs execution input without replacing the original request.
    pub async fn rehydrate_durable_streaming_input(
        &self,
        invocation: &AgentInvocation,
    ) -> Result<Option<TypedSchemaValue>, WorkerExecutorError> {
        let Some(idempotency_key) = invocation.idempotency_key().cloned() else {
            return Ok(None);
        };
        let Some(prepared) = self.prepared_stream_session(&idempotency_key).await? else {
            return Ok(None);
        };
        if prepared.stream_mappings.is_empty() {
            return Ok(None);
        }
        let producer = self.durable_stream_producer().await?;
        let session_reference = StreamRegistrationInvocation::Local(prepared.session_key);
        let _ = self
            .recover_durable_stream_topologies(
                false,
                Some(&producer.qualify_session(&session_reference)),
            )
            .await?;
        let mappings = producer
            .materialize_bindings(&prepared.stream_mappings)
            .await
            .map_err(|error| error.into_worker_executor_error(WorkerExecutorError::runtime))?;
        let streams = StreamSession::open(
            producer,
            self.oplog.clone(),
            session_reference,
            prepared.stream_mappings.iter().cloned(),
        )
        .await
        .map_err(|error| error.into_worker_executor_error(WorkerExecutorError::runtime))?
        .with_rpc(self.rpc())
        .with_consumer_journal(self.durable_stream_consumer_journal())
        .with_auth_ctx(self.durable_stream_consumer_auth_ctx()?);
        streams
            .recover_nested_input_mappings()
            .await
            .map_err(|error| error.into_worker_executor_error(WorkerExecutorError::runtime))?;
        streams
            .recover_session_mappings()
            .await
            .map_err(|error| error.into_worker_executor_error(WorkerExecutorError::runtime))?;
        let encoded = prepared.attempt.invocation.invocation_value.as_slice();
        let typed = golem_schema::proto::golem::schema::TypedSchemaValue::decode(encoded).map_err(
            |error| {
                WorkerExecutorError::runtime(format!(
                    "failed to decode persisted durable invocation input: {error}"
                ))
            },
        )?;
        let graph = typed
            .graph
            .ok_or_else(|| WorkerExecutorError::runtime("missing invocation input graph"))?
            .try_into()
            .map_err(WorkerExecutorError::runtime)?;
        let value = typed
            .value
            .ok_or_else(|| WorkerExecutorError::runtime("missing invocation input value"))?;
        let input = streams
            .decode_initial(value, &mappings, SessionStreamRole::Input)
            .await
            .map_err(|error| error.into_worker_executor_error(WorkerExecutorError::runtime))?;
        Ok(Some(TypedSchemaValue::new(graph, input)))
    }
    /// Persists stream-bearing result mappings and returns the transport value.
    pub(crate) async fn materialize_durable_streaming_result(
        &self,
        idempotency_key: &InvocationId,
        value: golem_common::schema::SchemaValue,
        graph: &golem_common::schema::SchemaGraph,
        root: &golem_common::schema::SchemaType,
        component_revision: ComponentRevision,
        runtime_source: Option<suspension::RuntimeSource>,
        preparation: Option<suspension::ExternalActivity>,
        tasks: &crate::worker::tasks::NativeReconstructionSpawner,
    ) -> Result<golem_common::schema::SchemaValue, WorkerExecutorError> {
        let Some(prepared) = self.prepared_stream_session(idempotency_key).await? else {
            if contains_stream(&value) {
                return Err(WorkerExecutorError::runtime(
                    "live stream at a materializing invocation boundary without a durable Stream Session",
                ));
            }
            return Ok(value);
        };
        // The original agent or tool consumer can reattach after revert, but never to a new fork.
        let requires_attachment =
            stream_effective_identity_is_internal(&prepared.attempt.effective_identity)
                && prepared.attempt.session_key.callee_fingerprint
                    == self.initial_worker_metadata.fingerprint;
        let mut streams = StreamSession::open(
            self.durable_stream_producer().await?,
            self.oplog.clone(),
            StreamRegistrationInvocation::Local(prepared.attempt.session_key.idempotency_key),
            prepared.stream_mappings.iter().cloned(),
        )
        .await
        .map_err(|error| error.into_worker_executor_error(WorkerExecutorError::runtime))?
        .with_rpc(self.rpc())
        .with_consumer_journal(self.durable_stream_consumer_journal())
        .with_auth_ctx(self.durable_stream_consumer_auth_ctx()?)
        .with_runtime_source(runtime_source);
        if requires_attachment {
            streams = streams.require_root_attachment_before_production();
        }
        streams
            .spawn_materialize_result(value, graph, root, component_revision, preparation, tasks)?
            .await
            .map_err(|_| WorkerExecutorError::runtime("schema output task owner was discarded"))?
    }
    /// Records a failed terminal for the invocation's durable stream session.
    pub async fn fail_durable_streaming_session(
        &self,
        idempotency_key: &InvocationId,
        details: String,
    ) -> Result<(), WorkerExecutorError> {
        self.finish_durable_streaming_session(idempotency_key, Err(details))
            .await
    }
    /// Records a successful terminal for the invocation's durable stream session.
    pub async fn complete_durable_streaming_session(
        &self,
        idempotency_key: &InvocationId,
    ) -> Result<(), WorkerExecutorError> {
        self.finish_durable_streaming_session(idempotency_key, Ok(()))
            .await
    }
    pub(in crate::worker) async fn finish_durable_streaming_session(
        &self,
        idempotency_key: &InvocationId,
        result: Result<(), String>,
    ) -> Result<(), WorkerExecutorError> {
        let Some(status) = self.durable_stream_session_status(idempotency_key).await? else {
            return Ok(());
        };
        if status.finished.is_some() {
            return Ok(());
        }
        let Some(index) = status.prepared else {
            return Ok(());
        };
        let StreamSessionRecord::Prepared(prepared) =
            self.read_stream_session_record(index).await?
        else {
            return Err(WorkerExecutorError::runtime(
                "stream session index does not refer to Prepared",
            ));
        };
        let result_mappings = match status.invocation_result {
            Some(index) => {
                self.durable_stream_producer()
                    .await?
                    .read_invocation_result_value(index, *idempotency_key)
                    .await
                    .map_err(|error| WorkerExecutorError::runtime(error.to_string()))?
                    .stream_bindings
            }
            None => Vec::new(),
        };
        let mappings = prepared
            .stream_mappings
            .iter()
            .chain(&result_mappings)
            .cloned();
        let streams = StreamSession::open(
            self.durable_stream_producer().await?,
            self.oplog.clone(),
            StreamRegistrationInvocation::Local(prepared.attempt.session_key.idempotency_key),
            mappings,
        )
        .await
        .map_err(|error| error.into_worker_executor_error(WorkerExecutorError::runtime))?
        .with_rpc(self.rpc())
        .with_consumer_journal(self.durable_stream_consumer_journal())
        .with_auth_ctx(self.durable_stream_consumer_auth_ctx()?);
        match result {
            Ok(()) => streams.complete_or_defer_for_forwarded_inputs().await,
            Err(details) => streams.fail(details).await,
        }
        .map_err(|error| error.into_worker_executor_error(WorkerExecutorError::runtime))?;
        Ok(())
    }
    pub(in crate::worker) async fn recover_finished_durable_streaming_sessions(
        &self,
    ) -> Result<(), WorkerExecutorError> {
        let status = self.state_actor.status().await;
        for (idempotency_key, session) in status.durable_stream_sessions.iter() {
            if session.prepared.is_none() || session.finished.is_some() {
                continue;
            }
            let result = self.lookup_invocation_result(&idempotency_key).await;
            // Lookup can observe a return committed after the maintenance snapshot.
            let latest = self.get_repaired_status().await?;
            if latest.settling_invocation == Some(idempotency_key)
                && let Some(prepared) = self.prepared_stream_session(&idempotency_key).await?
                && prepared
                    .stream_mappings
                    .iter()
                    .any(|binding| binding.role.direction() == SessionStreamRole::Input)
            {
                // Reconstruction must replay the background readers before it closes inputs.
                continue;
            }
            match result {
                LookupResult::Complete(Ok(_)) => {
                    self.complete_durable_streaming_session(&idempotency_key)
                        .await?;
                }
                LookupResult::Complete(Err(error)) => {
                    self.fail_durable_streaming_session(&idempotency_key, error.to_string())
                        .await?;
                }
                LookupResult::New | LookupResult::Pending | LookupResult::Interrupted => {}
            }
        }
        Ok(())
    }
}

pub(in crate::worker) fn durable_stream_attempt_error_outcome(
    error: &WorkerExecutorError,
) -> &'static str {
    let (details, fallback) = match error {
        WorkerExecutorError::InvalidRequest { details } => (details.as_str(), "rejected"),
        WorkerExecutorError::Runtime { details } => (details.as_str(), "error"),
        _ => return "error",
    };
    [
        ("AttemptConflict:", "attempt_conflict"),
        ("IdempotencyConflict:", "idempotency_conflict"),
        ("StaleEpoch:", "stale_epoch"),
        ("InvalidEpoch:", "future_epoch"),
        ("InvalidAttachmentState:", "invalid_attachment_state"),
        ("IncarnationMismatch:", "incarnation_mismatch"),
        ("Unauthorized:", "unauthorized"),
        ("NotFound:", "not_found"),
        ("ResourceExhausted:", "resource_exhausted"),
    ]
    .into_iter()
    .find_map(|(prefix, outcome)| details.starts_with(prefix).then_some(outcome))
    .unwrap_or(fallback)
}

/// Decides whether a repeated HTTP stream-session PUT targets the persisted invocation, not
/// whether it repeats an exact transport attempt. Attempt identity and live-join buffering may
/// differ. Execution configuration is creation-only here: the HTTP handler supplies fixed/default
/// settings, and re-PUT does not reconfigure an accepted invocation. Unlike tracing (already
/// excluded from the descriptor), execution configuration contains semantic settings such as
/// environment values; exposing configurable HTTP settings requires revisiting this policy.
/// Agent-RPC attempts use exact descriptor matching, including execution configuration.
/// Stream identity is checked separately against owner-relative bindings, not reissued handles.
pub(in crate::worker) fn stream_slot_session_matches(
    persisted: &StartAttemptDescriptor,
    requested: &StartAttemptDescriptor,
) -> bool {
    persisted.format_version == requested.format_version
        && persisted.session_key == requested.session_key
        && persisted.expected_callee_fingerprint == requested.expected_callee_fingerprint
        && persisted.effective_identity == requested.effective_identity
        && persisted.invocation.format_version == requested.invocation.format_version
        && persisted.invocation.session_key == requested.invocation.session_key
        && persisted.invocation.target_component_revision
            == requested.invocation.target_component_revision
        && persisted.invocation.target == requested.invocation.target
        && persisted.invocation.invocation_value == requested.invocation.invocation_value
        && persisted.invocation.effective_identity == requested.invocation.effective_identity
}

pub(in crate::worker) fn stream_attempt_matches(
    persisted: &StartAttemptDescriptor,
    requested: &StartAttemptDescriptor,
) -> bool {
    persisted.format_version == requested.format_version
        && persisted.session_key == requested.session_key
        && persisted.attachment_id == requested.attachment_id
        && persisted.expected_callee_fingerprint == requested.expected_callee_fingerprint
        && persisted.attempt_id == requested.attempt_id
        && persisted.effective_identity == requested.effective_identity
        && persisted.live_join_buffer_events == requested.live_join_buffer_events
        && persisted_stream_descriptor_matches(&persisted.invocation, &requested.invocation)
}

/// Stream handles are checked separately through bindings so a local generation change is not
/// mistaken for a different logical invocation.
pub(in crate::worker) fn persisted_stream_descriptor_matches(
    persisted: &PersistedStreamInvocationDescriptor,
    requested: &PersistedStreamInvocationDescriptor,
) -> bool {
    persisted.format_version == requested.format_version
        && persisted.session_key == requested.session_key
        && persisted.target_component_revision == requested.target_component_revision
        && persisted.target == requested.target
        && persisted.invocation_value == requested.invocation_value
        && persisted.execution_config == requested.execution_config
        && persisted.effective_identity == requested.effective_identity
}

pub(in crate::worker) fn stream_output_consumer_is_observer(
    accepted: &StartAttemptDescriptor,
    attachment: &golem_common::model::durable_stream::StreamAttachmentKey,
) -> Result<bool, WorkerExecutorError> {
    let (bytes, _environment): (Vec<u8>, Option<Vec<(String, String)>>) =
        golem_common::serialization::deserialize(&accepted.invocation.execution_config)
            .map_err(WorkerExecutorError::runtime)?;
    let start = golem_api_grpc::proto::golem::worker::InvocationStart::decode(bytes.as_slice())
        .map_err(|error| WorkerExecutorError::runtime(error.to_string()))?;
    let Some(origin) = start.origin_invocation else {
        return Ok(false);
    };
    let caller = start
        .context
        .and_then(|context| context.parent)
        .ok_or_else(|| {
            WorkerExecutorError::runtime("persisted streaming RPC has no accepted caller")
        })?;
    let environment = origin.callee_environment_id.ok_or_else(|| {
        WorkerExecutorError::runtime("persisted streaming RPC has no caller environment")
    })?;
    // The first accepted caller owns cancellation, even if a fork dispatched an
    // inherited call before its logical origin did. Other readers abandon only their slot.
    Ok(caller != attachment.consumer.into()
        || environment != attachment.consumer_environment_id.into())
}

/// True when an agent or a tool that an agent hosts started the stream.
pub(in crate::worker) fn stream_effective_identity_is_internal(effective_identity: &[u8]) -> bool {
    stream_internal_authority(effective_identity).is_some()
}

/// The auth-context entry of the identity of a stream that an agent, or a tool that an agent
/// hosts, started: the immediate caller is an agent or a tool. `None` for any other stream. The
/// identity is the auth-context entry followed by the principal entry, when the request has a
/// principal, so only a second entry is a principal: an auth context alone can also decode as an
/// agent principal.
pub(in crate::worker) fn stream_internal_authority(effective_identity: &[u8]) -> Option<&[u8]> {
    let mut cursor = effective_identity;
    let mut last = None;
    let mut authority = None;
    let mut entries = 0;
    while cursor.len() >= 8 {
        entries += 1;
        authority = Some(&effective_identity[..effective_identity.len() - cursor.len()]);
        let (length, rest) = cursor.split_at(8);
        let length = u64::from_be_bytes(
            length
                .try_into()
                .expect("effective identity length prefix has fixed width"),
        );
        let Ok(length) = usize::try_from(length) else {
            return None;
        };
        if rest.len() < length {
            return None;
        }
        let (value, rest) = rest.split_at(length);
        last = Some(value);
        cursor = rest;
    }
    if !cursor.is_empty() || entries != 2 {
        return None;
    }
    last.and_then(|value| {
        golem_api_grpc::proto::golem::component::Caller::decode(value)
            .ok()
            .and_then(|caller| Caller::try_from(caller).ok())
    })
    .is_some_and(|caller| matches!(caller.immediate, Principal::Agent(_) | Principal::Tool(_)))
    .then_some(authority?)
}

pub(crate) fn same_origin_stream_invocation(
    persisted: &StartAttemptDescriptor,
    requested: &StartAttemptDescriptor,
) -> bool {
    let original = &persisted.invocation;
    let retry = &requested.invocation;
    if persisted.format_version != requested.format_version
        || persisted.session_key != requested.session_key
        || persisted.expected_callee_fingerprint != requested.expected_callee_fingerprint
        || original.format_version != retry.format_version
        || original.session_key != retry.session_key
        || original.target_component_revision != retry.target_component_revision
        || original.target != retry.target
        || original.invocation_value != retry.invocation_value
        || original.stream_handles.len() != retry.stream_handles.len()
        || original
            .stream_handles
            .iter()
            .zip(&retry.stream_handles)
            .any(|(a, b)| a.element_schema_fingerprint != b.element_schema_fingerprint)
        || persisted.effective_identity != original.effective_identity
        || requested.effective_identity != retry.effective_identity
    {
        return false;
    }
    same_origin_stream_authority(
        &original.effective_identity,
        &retry.effective_identity,
        &original.execution_config,
        &retry.execution_config,
        &original.stream_handles,
        &retry.stream_handles,
    )
}

pub(crate) fn same_origin_queued_request(
    original: &AgentInvocationPayload,
    retry: &AgentInvocationPayload,
    accepted: &golem_common::model::durable_stream::StreamSessionAcceptance,
    requested: &golem_common::model::durable_stream::StreamSessionAcceptance,
) -> bool {
    use golem_common::model::agent::AgentMethodInput;
    let (
        AgentInvocationPayload::AgentMethod {
            method_name: a,
            input: AgentMethodInput::Native(input),
            allow_streams: sa,
            ..
        },
        AgentInvocationPayload::AgentMethod {
            method_name: b,
            input: AgentMethodInput::Native(other),
            allow_streams: sb,
            ..
        },
    ) = (original, retry)
    else {
        return false;
    };
    if a != b
        || sa != sb
        || !input.same_unbound_value(other)
        || accepted.transport_stream_ids != requested.transport_stream_ids
        || accepted.live_join_buffer_events != requested.live_join_buffer_events
    {
        return false;
    }
    let (Some(handles), Some(other_handles)) = (
        input.streams().iter().cloned().collect::<Option<Vec<_>>>(),
        other.streams().iter().cloned().collect::<Option<Vec<_>>>(),
    ) else {
        return false;
    };
    same_origin_stream_authority(
        &accepted.effective_identity,
        &requested.effective_identity,
        &accepted.execution_config,
        &requested.execution_config,
        &handles,
        &other_handles,
    )
}

fn same_origin_stream_authority(
    original_identity: &[u8],
    retry_identity: &[u8],
    original_execution: &[u8],
    retry_execution: &[u8],
    original_handles: &[golem_common::model::durable_stream::DurableStreamHandle],
    retry_handles: &[golem_common::model::durable_stream::DurableStreamHandle],
) -> bool {
    use golem_api_grpc::proto::golem::worker::InvocationStart;
    if original_handles.len() != retry_handles.len()
        || original_handles
            .iter()
            .zip(retry_handles)
            .any(|(a, b)| a.element_schema_fingerprint != b.element_schema_fingerprint)
    {
        return false;
    }
    let (Some(original_authority), Some(retry_authority)) = (
        stream_internal_authority(original_identity),
        stream_internal_authority(retry_identity),
    ) else {
        return false;
    };
    let decode = |bytes: &[u8]| {
        let (bytes, environment, stdout, stderr): SessionExecutionConfig =
            golem_common::serialization::deserialize(bytes).ok()?;
        Some((
            InvocationStart::decode(bytes.as_slice()).ok()?,
            (environment, stdout, stderr),
        ))
    };
    let (Some((mut original_config, original_env)), Some((mut retry_config, retry_env))) =
        (decode(original_execution), decode(retry_execution))
    else {
        return false;
    };
    if original_config.origin_invocation.is_none()
        || original_config.origin_invocation != retry_config.origin_invocation
    {
        return false;
    }
    for (a, b) in original_handles.iter().zip(retry_handles) {
        if a == b {
            continue;
        }
        for (handle, config) in [(a, &original_config), (b, &retry_config)] {
            let parent = config
                .context
                .as_ref()
                .and_then(|context| context.parent.as_ref());
            let environment = config
                .origin_invocation
                .as_ref()
                .and_then(|origin| origin.callee_environment_id);
            if parent != Some(&handle.producer.into())
                || environment != Some(handle.producer_environment_id.into())
            {
                return false;
            }
        }
    }
    let (Some(original_auth), Some(retry_auth)) = (
        normalize_stream_join_authority(original_authority, &mut original_config),
        normalize_stream_join_authority(retry_authority, &mut retry_config),
    ) else {
        return false;
    };
    if original_auth != retry_auth {
        return false;
    }
    for config in [&mut original_config, &mut retry_config] {
        if let Some(context) = &mut config.context {
            context.parent = None;
        }
    }
    original_config == retry_config && original_env == retry_env
}

pub(in crate::worker) fn normalize_stream_join_authority(
    authority: &[u8],
    config: &mut golem_api_grpc::proto::golem::worker::InvocationStart,
) -> Option<AuthCtx> {
    use golem_common::model::card::owner::{AgentOwnerLeafPattern, AgentOwnerPattern};
    use golem_common::model::card::{PermissionPattern, PermissionTarget, ScopeCard};

    // A fork keeps the type of its origin, so only the agent ID of the leaf changes.
    fn normalize_owner(owner: &mut AgentOwnerPattern, caller: &AgentId, origin: &AgentId) {
        if let AgentOwnerPattern::Agent {
            agent: AgentOwnerLeafPattern::Agent(leaf),
            ..
        } = owner
            && leaf.agent_id == *caller
        {
            leaf.agent_id = *origin;
        }
    }

    fn normalize_grants(grants: &mut [PermissionPattern], caller: &AgentId, origin: &AgentId) {
        for grant in grants {
            let owner = match grant {
                PermissionPattern::Agent(grant) => &mut grant.owner,
                PermissionPattern::Filesystem(grant) => &mut grant.owner,
                PermissionPattern::Env(grant) => &mut grant.owner,
                PermissionPattern::Oplog(grant) => &mut grant.owner,
                PermissionPattern::Config(grant) => &mut grant.owner,
                _ => continue,
            };
            normalize_owner(owner, caller, origin);
        }
    }

    let length = usize::try_from(u64::from_be_bytes(authority.get(..8)?.try_into().ok()?)).ok()?;
    let bytes = authority.get(8..)?;
    if bytes.len() != length {
        return None;
    }
    let mut auth =
        AuthCtx::try_from(golem_api_grpc::proto::golem::auth::AuthCtx::decode(bytes).ok()?).ok()?;
    let caller = &AgentId::try_from(config.context.as_ref()?.parent?).ok()?;
    let origin = &AgentId::try_from(config.origin_invocation.as_ref()?.callee?).ok()?;
    // Forks monomorphize the retained wallet under their own name. Compare self-scoped
    // grants under the common author, retaining card IDs, scopes, verbs and resources.
    if let AuthCtx::Agent(agent) = &mut auth {
        for surface in agent
            .effective_surface
            .lower
            .iter_mut()
            .chain(&mut agent.effective_surface.upper)
        {
            for target in surface.positive.iter_mut().chain(&mut surface.negative) {
                let owner = match target {
                    PermissionTarget::Agent(target) => &mut target.owner,
                    PermissionTarget::Filesystem(target) => &mut target.owner,
                    PermissionTarget::Env(target) => &mut target.owner,
                    PermissionTarget::Oplog(target) => &mut target.owner,
                    PermissionTarget::Config(target) => &mut target.owner,
                    _ => continue,
                };
                normalize_owner(owner, caller, origin);
            }
        }
        if let Some(delegation) = &mut agent.delegation_surface {
            for card in &mut delegation.cards {
                for grants in [
                    &mut card.lower_positive,
                    &mut card.lower_negative,
                    &mut card.upper_positive,
                    &mut card.upper_negative,
                ] {
                    normalize_grants(grants, caller, origin);
                }
            }
        }
    }
    if let Some(encoded) = config.scope_card.take() {
        let mut scope = ScopeCard::try_from(encoded).ok()?;
        for grants in [
            &mut scope.lower_positive,
            &mut scope.lower_negative,
            &mut scope.upper_positive,
            &mut scope.upper_negative,
        ] {
            normalize_grants(grants, caller, origin);
        }
        config.scope_card = Some((&scope).try_into().ok()?);
    }
    Some(auth)
}

pub(crate) fn stream_session_record_key(
    record: &StreamSessionRecord,
    owner_environment_id: golem_common::base_model::environment::EnvironmentId,
    owner: &AgentId,
    owner_fingerprint: AgentFingerprint,
) -> Option<golem_common::base_model::durable_stream::StreamSessionKey> {
    match record {
        StreamSessionRecord::CallerAttempt(record) => Some(record.session_key.qualify(
            owner_environment_id,
            owner,
            owner_fingerprint,
        )),
        StreamSessionRecord::Prepared(record) => Some(StreamSessionKey {
            callee_environment_id: owner_environment_id,
            callee: *owner,
            callee_fingerprint: owner_fingerprint,
            idempotency_key: record.session_key,
        }),
        StreamSessionRecord::ExpiryRefreshed(record) => Some(StreamSessionKey {
            callee_environment_id: owner_environment_id,
            callee: *owner,
            callee_fingerprint: owner_fingerprint,
            idempotency_key: record.session_key,
        }),
        StreamSessionRecord::Expired(record) => Some(StreamSessionKey {
            callee_environment_id: owner_environment_id,
            callee: *owner,
            callee_fingerprint: owner_fingerprint,
            idempotency_key: record.session_key,
        }),
        StreamSessionRecord::Attached(record) => Some(StreamSessionKey {
            callee_environment_id: owner_environment_id,
            callee: *owner,
            callee_fingerprint: owner_fingerprint,
            idempotency_key: record.session_key,
        }),
        StreamSessionRecord::ResumeAttempt(record) => Some(StreamSessionKey {
            callee_environment_id: owner_environment_id,
            callee: *owner,
            callee_fingerprint: owner_fingerprint,
            idempotency_key: record.session_key,
        }),
        StreamSessionRecord::Detached(record) => Some(StreamSessionKey {
            callee_environment_id: owner_environment_id,
            callee: *owner,
            callee_fingerprint: owner_fingerprint,
            idempotency_key: record.session_key,
        }),
        StreamSessionRecord::Mapping(record) => Some(record.session_key.qualify(
            owner_environment_id,
            owner,
            owner_fingerprint,
        )),
        StreamSessionRecord::AttachmentPrepared(record) => Some(record.key.session_key.clone()),
        StreamSessionRecord::AttachmentActivated(record) => Some(record.key.session_key.clone()),
        StreamSessionRecord::AttachmentFinalized(record) => Some(record.key.session_key.clone()),
        StreamSessionRecord::CascadeOutbox(record) => Some(record.key.session_key.clone()),
        StreamSessionRecord::SourceUnavailable(record) => Some(record.session_key.qualify(
            owner_environment_id,
            owner,
            owner_fingerprint,
        )),
        StreamSessionRecord::TopologyPrepared(record) => Some(record.session_key.clone()),
        StreamSessionRecord::TopologyActivated(record) => Some(record.session_key.clone()),
        StreamSessionRecord::InputHighWater(record) => Some(record.session_key.clone()),
        StreamSessionRecord::ExternalProducerState(record) => Some(record.session_key.clone()),
        StreamSessionRecord::ConsumerItemValue(record) => Some(record.session_key.qualify(
            owner_environment_id,
            owner,
            owner_fingerprint,
        )),
        StreamSessionRecord::ConsumerCancelIntent(record) => Some(record.session_key.qualify(
            owner_environment_id,
            owner,
            owner_fingerprint,
        )),
        StreamSessionRecord::ConsumerCancelApplied(record) => Some(
            record
                .intent
                .session_key
                .qualify(owner_environment_id, owner, owner_fingerprint),
        ),
        StreamSessionRecord::ConsumerTerminal(record)
        | StreamSessionRecord::ConsumerOutcome(record) => Some(record.session_key.qualify(
            owner_environment_id,
            owner,
            owner_fingerprint,
        )),
        StreamSessionRecord::ReaderForwardIntent(record) => Some(record.session_key.qualify(
            owner_environment_id,
            owner,
            owner_fingerprint,
        )),
        StreamSessionRecord::RemoteResult(record) => Some(record.session_key.qualify(
            owner_environment_id,
            owner,
            owner_fingerprint,
        )),
        StreamSessionRecord::Finished(record) => Some(record.session_key.qualify(
            owner_environment_id,
            owner,
            owner_fingerprint,
        )),
        StreamSessionRecord::Tombstoned(record) => Some(record.session_key.qualify(
            owner_environment_id,
            owner,
            owner_fingerprint,
        )),
        StreamSessionRecord::CancelRequested(record) => Some(record.session_key.qualify(
            owner_environment_id,
            owner,
            owner_fingerprint,
        )),
        StreamSessionRecord::ExportForkInitialized(record) => Some(StreamSessionKey {
            callee_environment_id: owner_environment_id,
            callee: *owner,
            callee_fingerprint: owner_fingerprint,
            idempotency_key: record.session_key,
        }),
        StreamSessionRecord::ProducerDeleting(_)
        | StreamSessionRecord::ConsumerDeleting(_)
        | StreamSessionRecord::ForkCut(_)
        | StreamSessionRecord::ExportForkAdmitted(_) => None,
    }
}

/// Control projections affected by a record; attribution remains on its primary source session.
pub(crate) fn stream_session_record_keys(
    record: &StreamSessionRecord,
    owner_environment_id: golem_common::base_model::environment::EnvironmentId,
    owner: &AgentId,
    owner_fingerprint: AgentFingerprint,
) -> Vec<StreamSessionKey> {
    let mut keys: Vec<_> =
        stream_session_record_key(record, owner_environment_id, owner, owner_fingerprint)
            .into_iter()
            .collect();
    if let StreamSessionRecord::ReaderForwardIntent(record) = record {
        let destination =
            record
                .destination
                .session_key(owner_environment_id, owner, owner_fingerprint);
        if !keys.contains(&destination) {
            keys.push(destination);
        }
    }
    keys
}

pub(crate) fn stream_session_record_reference(
    record: &StreamSessionRecord,
    reader_id: LocalStreamReaderId,
) -> Option<StreamRegistrationInvocation> {
    let (session, bindings): (&StreamRegistrationInvocation, &[StreamBindingRecord]) = match record
    {
        StreamSessionRecord::Prepared(record) => {
            return usize::try_from(reader_id.binding_slot)
                .ok()
                .filter(|slot| *slot < record.stream_mappings.len())
                .map(|_| StreamRegistrationInvocation::Local(record.session_key));
        }
        StreamSessionRecord::Mapping(record) => {
            (&record.session_key, std::slice::from_ref(&record.mapping))
        }
        StreamSessionRecord::RemoteResult(record) => (&record.session_key, &record.stream_mappings),
        StreamSessionRecord::ConsumerItemValue(record) if record.reobserves.is_none() => {
            (&record.session_key, &record.recursive_mappings)
        }
        _ => return None,
    };
    usize::try_from(reader_id.binding_slot)
        .ok()
        .filter(|slot| *slot < bindings.len())
        .map(|_| session.clone())
}

pub(in crate::worker) fn validate_stream_session_record(
    record: &StreamSessionRecord,
) -> Result<(), WorkerExecutorError> {
    if record.has_supported_format() {
        Ok(())
    } else {
        Err(WorkerExecutorError::runtime(
            "unsupported or malformed durable Stream Session record version",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::{stream_effective_identity_is_internal, stream_internal_authority};
    use golem_api_grpc::proto::golem::auth::{AuthCtx, UserAuthCtx, auth_ctx};
    use golem_common::model::AgentId;
    use prost::Message;
    use test_r::test;

    fn identity(entries: &[Vec<u8>]) -> Vec<u8> {
        let mut identity = Vec::new();
        for entry in entries {
            identity.extend_from_slice(&(entry.len() as u64).to_be_bytes());
            identity.extend_from_slice(entry);
        }
        identity
    }

    // A user auth context has the wire shape of an agent principal: its account ID decodes as
    // the principal's agent ID.
    #[test]
    fn a_user_auth_context_without_a_principal_is_not_an_agent_identity() {
        let user = AuthCtx {
            value: Some(auth_ctx::Value::User(UserAuthCtx {
                account_id: Some(
                    golem_common::model::account::AccountId(uuid::Uuid::from_u128(7)).into(),
                ),
                ..Default::default()
            })),
        }
        .encode_to_vec();
        let agent: golem_api_grpc::proto::golem::component::Caller =
            crate::test_callers::agent_caller(AgentId::from_parts(7, 1)).into();

        assert!(!stream_effective_identity_is_internal(&identity(
            std::slice::from_ref(&user)
        )));
        assert!(!stream_effective_identity_is_internal(&identity(&[
            agent.encode_to_vec()
        ])));
        assert!(stream_effective_identity_is_internal(&identity(&[
            user,
            agent.encode_to_vec()
        ])));
    }

    #[test]
    fn stream_internal_authority_reads_the_caller_identity() {
        use golem_common::model::caller::Caller;

        let caller_identity = |caller: &Caller| {
            let wire: golem_api_grpc::proto::golem::component::Caller = caller.clone().into();
            identity(&[AuthCtx::default().encode_to_vec(), wire.encode_to_vec()])
        };
        let agent = crate::test_callers::agent_caller(AgentId::from_parts(7, 1));
        let identity_of_agent = caller_identity(&agent);
        assert!(stream_internal_authority(&identity_of_agent).is_some());
        assert!(stream_effective_identity_is_internal(&identity_of_agent));

        let user = Caller::entry(crate::test_callers::user_principal());
        assert!(stream_internal_authority(&caller_identity(&user)).is_none());

        let mut agent_for_user = agent.clone();
        agent_for_user.original = user.original.clone();
        assert!(stream_internal_authority(&caller_identity(&agent_for_user)).is_some());
        let mut user_via_agent_root = user.clone();
        user_via_agent_root.original = agent.original;
        assert!(stream_internal_authority(&caller_identity(&user_via_agent_root)).is_none());

        // A tool that an agent hosts calls for the same user: its stream is as internal as the
        // agent's.
        let tool_for_user = Caller {
            original: user.original.clone(),
            immediate: golem_common::model::caller::Principal::Tool(
                golem_common::model::caller::ToolPrincipal {
                    issuer: crate::test_callers::TEST_ISSUER.to_string(),
                    environment_id: golem_common::model::environment::EnvironmentId(
                        uuid::Uuid::from_u128(0xe0),
                    ),
                    tool_name: golem_common::model::tool::ToolName::try_from("report").unwrap(),
                },
            ),
            tokens: Vec::new(),
        };
        assert!(stream_internal_authority(&caller_identity(&tool_for_user)).is_some());
        assert!(stream_effective_identity_is_internal(&caller_identity(
            &tool_for_user
        )));
        assert!(stream_internal_authority(&caller_identity(&Caller::anonymous())).is_none());

        let mut garbage = Vec::new();
        garbage.extend_from_slice(&3u64.to_be_bytes());
        garbage.extend_from_slice(&[0xff, 0xff, 0xff]);
        assert!(stream_internal_authority(&garbage).is_none());
    }
}
