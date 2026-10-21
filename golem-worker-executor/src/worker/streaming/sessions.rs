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
    pub(in crate::worker) async fn stream_session_records(
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
                if let OplogEntry::StreamSession { record, .. } = entry {
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
        idempotency_key: &IdempotencyKey,
    ) -> Result<Option<golem_common::model::DurableStreamSessionStatus>, WorkerExecutorError> {
        let status = self.state_actor.status().await;
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
        idempotency_key: &IdempotencyKey,
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
        idempotency_key: &IdempotencyKey,
    ) -> Result<Option<(StreamSessionPreparedRecord, StreamSession)>, WorkerExecutorError> {
        let Some(prepared) = self.prepared_stream_session(idempotency_key).await? else {
            return Ok(None);
        };
        let streams = StreamSession::open(
            self.durable_stream_producer().await?,
            self.oplog.clone(),
            StreamRegistrationInvocation::Local(prepared.session_key.clone()),
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
    /// Reconstructs stream-bearing invocation input from committed session mappings.
    pub async fn rehydrate_durable_streaming_invocation(
        &self,
        invocation: AgentInvocation,
    ) -> Result<AgentInvocation, WorkerExecutorError> {
        let Some(idempotency_key) = invocation.idempotency_key().cloned() else {
            return Ok(invocation);
        };
        let Some(prepared) = self.prepared_stream_session(&idempotency_key).await? else {
            return Ok(invocation);
        };
        if prepared.stream_mappings.is_empty() {
            return Ok(invocation);
        }
        let producer = self.durable_stream_producer().await?;
        let session_reference = StreamRegistrationInvocation::Local(prepared.session_key.clone());
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
        Ok(replace_invocation_input(
            invocation,
            TypedSchemaValue::new(graph, input),
        ))
    }
    /// Persists stream-bearing result mappings and returns the transport value.
    pub(crate) async fn materialize_durable_streaming_result(
        &self,
        idempotency_key: &IdempotencyKey,
        value: golem_common::schema::SchemaValue,
        graph: &golem_common::schema::SchemaGraph,
        root: &golem_common::schema::SchemaType,
        component_revision: ComponentRevision,
        runtime_source: Option<suspension::RuntimeSource>,
        preparation: Option<suspension::ExternalActivity>,
    ) -> Result<golem_common::schema::SchemaValue, WorkerExecutorError> {
        let Some(prepared) = self.prepared_stream_session(idempotency_key).await? else {
            if contains_stream(&value) {
                return Err(WorkerExecutorError::runtime(
                    "live stream at a materializing invocation boundary without a durable Stream Session",
                ));
            }
            return Ok(value);
        };
        // The original agent consumer can reattach after revert, but never to a new fork.
        let requires_attachment =
            stream_effective_identity_is_agent(&prepared.attempt.effective_identity)
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
            .materialize_result_accounted(value, graph, root, component_revision, preparation)
            .await
            .map_err(|error| error.into_worker_executor_error(WorkerExecutorError::runtime))
    }
    /// Records a failed terminal for the invocation's durable stream session.
    pub async fn fail_durable_streaming_session(
        &self,
        idempotency_key: &IdempotencyKey,
        details: String,
    ) -> Result<(), WorkerExecutorError> {
        self.finish_durable_streaming_session(idempotency_key, Err(details))
            .await
    }
    /// Records a successful terminal for the invocation's durable stream session.
    pub async fn complete_durable_streaming_session(
        &self,
        idempotency_key: &IdempotencyKey,
    ) -> Result<(), WorkerExecutorError> {
        self.finish_durable_streaming_session(idempotency_key, Ok(()))
            .await
    }
    pub(in crate::worker) async fn finish_durable_streaming_session(
        &self,
        idempotency_key: &IdempotencyKey,
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
            Some(index) => match self.read_stream_session_record(index).await? {
                StreamSessionRecord::InvocationResult(record) => record.stream_mappings,
                _ => {
                    return Err(WorkerExecutorError::runtime(
                        "stream session index does not refer to InvocationResult",
                    ));
                }
            },
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
            match self.lookup_invocation_result(&idempotency_key).await {
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
    Ok(caller != attachment.consumer.clone().into()
        || environment != attachment.consumer_environment_id.into())
}

pub(in crate::worker) fn stream_effective_identity_is_agent(effective_identity: &[u8]) -> bool {
    stream_agent_authority(effective_identity).is_some()
}

pub(in crate::worker) fn stream_agent_authority(effective_identity: &[u8]) -> Option<&[u8]> {
    let mut cursor = effective_identity;
    let mut last = None;
    let mut authority = None;
    while cursor.len() >= 8 {
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
    if !cursor.is_empty() {
        return None;
    }
    last.and_then(|value| {
        golem_api_grpc::proto::golem::component::Principal::decode(value)
            .ok()
            .and_then(|principal| Principal::try_from(principal).ok())
    })
    .is_some_and(|principal| matches!(principal, Principal::Agent(_)))
    .then_some(authority?)
}

pub(crate) fn same_origin_stream_invocation(
    persisted: &StartAttemptDescriptor,
    requested: &StartAttemptDescriptor,
) -> bool {
    use golem_api_grpc::proto::golem::worker::InvocationStart;

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
    let (Some(original_authority), Some(retry_authority)) = (
        stream_agent_authority(&original.effective_identity),
        stream_agent_authority(&retry.effective_identity),
    ) else {
        return false;
    };
    let decode = |bytes: &[u8]| {
        let (bytes, environment): (Vec<u8>, Option<Vec<(String, String)>>) =
            golem_common::serialization::deserialize(bytes).ok()?;
        Some((InvocationStart::decode(bytes.as_slice()).ok()?, environment))
    };
    let (Some((mut original_config, original_env)), Some((mut retry_config, retry_env))) = (
        decode(&original.execution_config),
        decode(&retry.execution_config),
    ) else {
        return false;
    };
    if original_config.origin_invocation.is_none()
        || original_config.origin_invocation != retry_config.origin_invocation
    {
        return false;
    }
    for (a, b) in original.stream_handles.iter().zip(&retry.stream_handles) {
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
            if parent != Some(&handle.producer.clone().into())
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

    fn normalize_owner(owner: &mut AgentOwnerPattern, caller: &str, origin: &str) {
        if let AgentOwnerPattern::Agent {
            agent: AgentOwnerLeafPattern::Agent(name),
            ..
        } = owner
            && name == caller
        {
            *name = origin.to_string();
        }
    }

    fn normalize_grants(grants: &mut [PermissionPattern], caller: &str, origin: &str) {
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
    let caller = &config.context.as_ref()?.parent.as_ref()?.name;
    let origin = &config.origin_invocation.as_ref()?.callee.as_ref()?.name;
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
            callee: owner.clone(),
            callee_fingerprint: owner_fingerprint,
            idempotency_key: record.session_key.clone(),
        }),
        StreamSessionRecord::ExpiryRefreshed(record) => Some(StreamSessionKey {
            callee_environment_id: owner_environment_id,
            callee: owner.clone(),
            callee_fingerprint: owner_fingerprint,
            idempotency_key: record.session_key.clone(),
        }),
        StreamSessionRecord::Expired(record) => Some(StreamSessionKey {
            callee_environment_id: owner_environment_id,
            callee: owner.clone(),
            callee_fingerprint: owner_fingerprint,
            idempotency_key: record.session_key.clone(),
        }),
        StreamSessionRecord::Attached(record) => Some(StreamSessionKey {
            callee_environment_id: owner_environment_id,
            callee: owner.clone(),
            callee_fingerprint: owner_fingerprint,
            idempotency_key: record.session_key.clone(),
        }),
        StreamSessionRecord::ResumeAttempt(record) => Some(StreamSessionKey {
            callee_environment_id: owner_environment_id,
            callee: owner.clone(),
            callee_fingerprint: owner_fingerprint,
            idempotency_key: record.session_key.clone(),
        }),
        StreamSessionRecord::Detached(record) => Some(StreamSessionKey {
            callee_environment_id: owner_environment_id,
            callee: owner.clone(),
            callee_fingerprint: owner_fingerprint,
            idempotency_key: record.session_key.clone(),
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
        StreamSessionRecord::ConsumerTerminal(record) => Some(record.session_key.qualify(
            owner_environment_id,
            owner,
            owner_fingerprint,
        )),
        StreamSessionRecord::ReaderForwardIntent(record) => Some(record.session_key.qualify(
            owner_environment_id,
            owner,
            owner_fingerprint,
        )),
        StreamSessionRecord::InvocationResult(record) => Some(record.session_key.qualify(
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
            callee: owner.clone(),
            callee_fingerprint: owner_fingerprint,
            idempotency_key: record.session_key.clone(),
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
                .map(|_| StreamRegistrationInvocation::Local(record.session_key.clone()));
        }
        StreamSessionRecord::Mapping(record) => {
            (&record.session_key, std::slice::from_ref(&record.mapping))
        }
        StreamSessionRecord::InvocationResult(record) => {
            (&record.session_key, &record.stream_mappings)
        }
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
