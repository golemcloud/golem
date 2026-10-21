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
    AgentId, AgentInvocation, Arc, AttachmentId, AttemptId, ComponentRevision,
    DURABLE_STREAM_FORMAT_VERSION, DurableStreamingInvocationRequest, EnvironmentId, FieldSource,
    HashMap, HashSet, IdempotencyKey, InputStreamAck, IntoTypedSchemaValue, InvocationRequest,
    InvocationResponse, InvocationSessionState, InvocationStart, MAX_DURABLE_STREAM_ITEM_SIZE,
    MAX_NEW_STREAM_HANDLES_PER_VALUE, Message, NamedFieldType, ParsedAgentId,
    PersistedInvocationTarget, PersistedStreamInvocationDescriptor, Principal,
    ProducerRegistrationRequest, ResumeAttach, ResumeAttemptDescriptor, ResumeOperation,
    SchemaGraph, SchemaType, SchemaValue, SchemaValueStream, SessionError, SessionStreamRole,
    StartAttemptDescriptor, StreamCancelReason, StreamCancelRole, StreamInvocationId,
    StreamItemsPayload, StreamRegistrationCoordinate, StreamRegistrationInvocation,
    StreamResumeCursor, StreamResumeOperation, StreamRootKind, StreamSession, StreamSessionMapping,
    StreamSourceKind, StreamValuePathStep, ToolByteStreamRole, ToolInvocationInput,
    ToolInvocationOutput, TransportStreamId, WorkerExecutorError, decode_recursive_stream_value,
    decode_recursive_stream_value_with_schema, durable_stream_mapping_from_proto,
    durable_stream_mapping_to_proto, encode_recursive_stream_value_with_schema, invocation_request,
    invocation_response, mpsc, schema_fingerprint_v1,
};

pub(crate) fn decode_invocation_input(
    input: golem_schema::proto::golem::schema::SchemaValue,
) -> Result<SchemaValue, String> {
    decode_recursive_stream_value(input, |stream_id, _| {
        Ok(SchemaValueStream::from_host_endpoint(TransportStreamId(
            stream_id,
        )))
    })
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
    let tool_input = match &invocation {
        AgentInvocation::ExternalTool { input, stdin, .. } => Some(
            ToolInvocationInput {
                arguments: (**input).clone(),
                stdin: stdin.then(|| SchemaValueStream::from_host_endpoint(TransportStreamId(0))),
            }
            .into_typed_schema_value()
            .map_err(|error| WorkerExecutorError::invalid_request(error.to_string()))?,
        ),
        _ => None,
    };
    let (graph, input_root, invocation_input, target, has_stdout, has_stderr) = match &invocation {
        AgentInvocation::AgentMethod {
            method_name, input, ..
        } => {
            let parsed = ParsedAgentId::parse(&callee.agent_id, component_metadata)
                .map_err(WorkerExecutorError::invalid_request)?;
            let agent_type = component_metadata
                .find_agent_type_by_name_ref(&parsed.agent_type)
                .ok_or_else(|| WorkerExecutorError::invalid_request("agent type not found"))?;
            let method = agent_type
                .methods
                .iter()
                .find(|method| &method.name == method_name)
                .ok_or_else(|| WorkerExecutorError::invalid_request("agent method not found"))?;
            let root = SchemaType::record(
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
            (
                &agent_type.schema,
                root,
                input,
                PersistedInvocationTarget::AgentMethod {
                    method_name: method_name.clone(),
                },
                false,
                false,
            )
        }
        AgentInvocation::ExternalTool {
            tool_name,
            command_path,
            stdout,
            stderr,
            ..
        } => {
            let input = tool_input
                .as_ref()
                .expect("tool input envelope was constructed");
            (
                input.graph(),
                input.root_type().clone(),
                input.value(),
                PersistedInvocationTarget::ExternalTool {
                    tool_name: tool_name.to_string(),
                    command_path: command_path.clone(),
                },
                *stdout,
                *stderr,
            )
        }
        _ => {
            return Err(WorkerExecutorError::invalid_request(
                "durable session requires a method or external tool invocation",
            ));
        }
    };
    let session_key = StreamInvocationId {
        callee_environment_id: environment_id,
        callee: callee.clone(),
        callee_fingerprint,
        idempotency_key: idempotency_key.clone(),
    };
    let attachment_id = AttachmentId::primary(environment_id, &callee, &idempotency_key)
        .map_err(|error| WorkerExecutorError::invalid_request(error.to_string()))?;
    let session_mapping = StreamSessionMapping {
        session_key: session_key.clone(),
        attachment_id,
        role: if matches!(
            invocation,
            AgentInvocation::ExternalTool { stdin: true, .. }
        ) {
            SessionStreamRole::ToolStdin
        } else {
            SessionStreamRole::Input
        },
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
        .any(|mapping| mapping.role.direction() != SessionStreamRole::Input)
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
        graph,
        &input_root,
        |stream, path| {
            let transport_stream_id = stream
                .with_host_endpoint::<TransportStreamId, _>(|stream_id| stream_id.0)?;
            let element = stream_element_schema(graph, &input_root, path)?;
            let element_schema_fingerprint = schema_fingerprint_v1(graph, element)
                .map_err(|error| error.to_string())?;
            input_element_types.push((
                transport_stream_id,
                element.cloned().unwrap_or_else(SchemaType::u8),
            ));
            if foreign_mappings.is_empty() {
                registrations.push((
                    transport_stream_id,
                    ProducerRegistrationRequest {
                        entity_parent_start_index: None,
                        coordinate: StreamRegistrationCoordinate::Root {
                            invocation_id: session_key.clone(),
                            root_kind: StreamRootKind::MethodInput,
                            recursive_value_path: path.to_vec(),
                        },
                        source_kind: StreamSourceKind::ExternalInlineInput,
                        source_invocation: StreamRegistrationInvocation::Local(
                            session_key.idempotency_key.clone(),
                        ),
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

    if let Some(origin) = &request.origin_invocation {
        let invalid = || WorkerExecutorError::invalid_request("invalid executor RPC origin");
        let _: EnvironmentId = origin
            .callee_environment_id
            .ok_or_else(invalid)?
            .try_into()
            .map_err(WorkerExecutorError::invalid_request)?;
        let _: AgentId = origin
            .callee
            .clone()
            .ok_or_else(invalid)?
            .try_into()
            .map_err(WorkerExecutorError::invalid_request)?;
        let fingerprint: uuid::Uuid = origin.callee_fingerprint.ok_or_else(invalid)?.into();
        let principal = request.principal.clone().ok_or_else(invalid)?;
        let principal =
            Principal::try_from(principal).map_err(WorkerExecutorError::invalid_request)?;
        let parent: AgentId = request
            .context
            .as_ref()
            .and_then(|context| context.parent.clone())
            .ok_or_else(invalid)?
            .try_into()
            .map_err(WorkerExecutorError::invalid_request)?;
        if fingerprint.is_nil()
            || origin
                .idempotency_key
                .as_ref()
                .is_none_or(|key| key.value.is_empty())
            || !matches!(principal, Principal::Agent(agent) if agent.agent_id == parent)
        {
            return Err(invalid());
        }
    }
    // Outputs declared before execution use their normal result-leaf coordinates.
    let mut next_transport_id = input_element_types.iter().map(|(id, _)| *id).max();
    for byte_stream_role in [
        has_stdout.then_some(ToolByteStreamRole::Stdout),
        has_stderr.then_some(ToolByteStreamRole::Stderr),
    ]
    .into_iter()
    .flatten()
    {
        let early_output = ToolInvocationOutput {
            outcome: Ok(
                golem_common::model::tool::SerializableToolInvocationResult { result: None },
            ),
            stdout: (byte_stream_role == ToolByteStreamRole::Stdout)
                .then(|| SchemaValueStream::from_host_endpoint(())),
            stderr: (byte_stream_role == ToolByteStreamRole::Stderr)
                .then(|| SchemaValueStream::from_host_endpoint(())),
        }
        .into_typed_schema_value()
        .map_err(|error| WorkerExecutorError::invalid_request(error.to_string()))?;
        encode_recursive_stream_value_with_schema(
            early_output.value(),
            early_output.graph(),
            early_output.root_type(),
            |_, path| {
                let id = match next_transport_id {
                    Some(id) => id
                        .checked_add(1)
                        .ok_or_else(|| "output stream ID overflow".to_string())?,
                    None => 0,
                };
                next_transport_id = Some(id);
                registrations.push((
                    id,
                    ProducerRegistrationRequest {
                        entity_parent_start_index: None,
                        coordinate: StreamRegistrationCoordinate::Root {
                            invocation_id: session_key.clone(),
                            root_kind: StreamRootKind::MethodResult,
                            recursive_value_path: path.to_vec(),
                        },
                        source_kind: StreamSourceKind::InvocationOutput,
                        source_invocation: StreamRegistrationInvocation::Local(
                            session_key.idempotency_key.clone(),
                        ),
                        component_revision,
                        element_schema_fingerprint: schema_fingerprint_v1(
                            early_output.graph(),
                            stream_element_schema(
                                early_output.graph(),
                                early_output.root_type(),
                                path,
                            )?,
                        )
                        .map_err(|error| error.to_string())?,
                        session_mapping: Some(StreamSessionMapping {
                            role: match byte_stream_role {
                                ToolByteStreamRole::Stdout => SessionStreamRole::ToolStdout,
                                ToolByteStreamRole::Stderr => SessionStreamRole::ToolStderr,
                                ToolByteStreamRole::Stdin => unreachable!(),
                            },
                            ..session_mapping.clone()
                        }),
                    },
                ));
                Ok(id)
            },
        )
        .map_err(WorkerExecutorError::invalid_request)?;
    }
    if registrations.len() + foreign_mappings.len() > MAX_NEW_STREAM_HANDLES_PER_VALUE {
        return Err(WorkerExecutorError::invalid_request(
            "invocation materializes more than 256 streams",
        ));
    }
    let mut input_graph = graph.clone();
    input_graph.root = input_root;
    let invocation_value = golem_schema::proto::golem::schema::TypedSchemaValue {
        graph: Some(input_graph.into()),
        value: Some(canonical_input),
    }
    .encode_to_vec();
    let input_schema = Arc::new(graph.clone());

    let mut execution = request.clone();
    execution.agent_id = None;
    execution.method_name = None;
    execution.input = None;
    execution.external_tool = None;
    execution.idempotency_key = None;
    execution.auth_ctx = None;
    execution.principal = None;
    execution.environment_id = None;
    execution.component_owner_account_id = None;
    execution.attempt_id = None;
    execution.expected_callee_fingerprint = None;
    execution.durable_input_mappings.clear();
    // The first accepted invocation retains its tracing context; a new attempt's spans do
    // not change its logical identity. Environment values do, but protobuf HashMap encoding
    // is not canonical, so encode them separately in key order.
    let mut environment = execution.context.as_mut().map(|context| {
        context.tracing = None;
        std::mem::take(&mut context.env)
            .into_iter()
            .collect::<Vec<_>>()
    });
    if let Some(environment) = environment.as_mut() {
        environment.sort();
    }
    let execution_config = golem_common::serialization::serialize(&(
        execution.encode_to_vec(),
        environment,
        has_stdout,
        has_stderr,
    ))
    .map_err(WorkerExecutorError::runtime)?;
    let effective_identity = effective_session_identity(&request.auth_ctx, &request.principal)?;
    let attempt = StartAttemptDescriptor {
        format_version: DURABLE_STREAM_FORMAT_VERSION,
        session_key: session_key.clone(),
        attachment_id,
        expected_callee_fingerprint: callee_fingerprint,
        attempt_id,
        invocation: PersistedStreamInvocationDescriptor {
            format_version: DURABLE_STREAM_FORMAT_VERSION,
            session_key,
            target_component_revision: component_revision,
            target,
            invocation_value,
            stream_handles: Vec::new(),
            execution_config,
            effective_identity: effective_identity.clone(),
        },
        effective_identity,
        live_join_buffer_events: u32::try_from(live_join_buffer_events).map_err(|_| {
            WorkerExecutorError::invalid_request("live join buffer capacity does not fit in u32")
        })?,
    };
    Ok(DurableStreamingInvocationRequest {
        public_session_id: idempotency_key.value.clone(),
        expiry_policy: golem_common::base_model::durable_stream::StreamSessionExpiryPolicy::None,
        expiry_deadline_millis: None,
        attempt,
        registrations,
        foreign_mappings,
        input_schema,
        input_element_types,
        invocation: replace_streams_for_persistence(invocation),
        acceptance_committed,
    })
}

pub(super) fn resumed_input_schema(
    prepared: &golem_common::model::durable_stream::StreamSessionPreparedRecord,
) -> Result<(SchemaGraph, Vec<(u64, SchemaType)>), WorkerExecutorError> {
    let typed = golem_schema::proto::golem::schema::TypedSchemaValue::decode(
        prepared.attempt.invocation.invocation_value.as_slice(),
    )
    .map_err(|error| WorkerExecutorError::runtime(error.to_string()))?;
    let graph: SchemaGraph = typed
        .graph
        .ok_or_else(|| WorkerExecutorError::runtime("session input has no graph"))?
        .try_into()
        .map_err(WorkerExecutorError::runtime)?;
    let canonical_input = typed
        .value
        .ok_or_else(|| WorkerExecutorError::runtime("session input has no value"))?;
    let input_root = graph.root.clone();
    let input_mappings = prepared
        .stream_mappings
        .iter()
        .filter(|mapping| mapping.role.direction() == SessionStreamRole::Input)
        .collect::<Vec<_>>();
    let mut input_element_types = Vec::with_capacity(input_mappings.len());
    decode_recursive_stream_value_with_schema(
        canonical_input,
        &graph,
        &input_root,
        |canonical_stream_id, path| {
            let mapping = input_mappings
                .get(usize::try_from(canonical_stream_id).map_err(|_| {
                    "persisted canonical input stream ID does not fit in usize".to_string()
                })?)
                .ok_or_else(|| {
                    "persisted canonical input references an unmapped stream".to_string()
                })?;
            let element = stream_element_schema(&graph, &input_root, path)?;
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
    Ok((graph, input_element_types))
}

pub(super) fn build_resume_attempt(
    request: &ResumeAttach,
    live_join_buffer_events: usize,
) -> Result<ResumeAttemptDescriptor, WorkerExecutorError> {
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
    let session_key = StreamInvocationId {
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
        ResumeOperation::Resume => StreamResumeOperation::Resume,
        ResumeOperation::Takeover => StreamResumeOperation::Takeover,
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
                    golem_common::model::durable_stream::StreamOffset::from_bytes(offset)
                        .map_err(|error| WorkerExecutorError::invalid_request(error.to_string()))
                })
                .transpose()?;
            Ok(StreamResumeCursor {
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
    Ok(ResumeAttemptDescriptor {
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

pub(super) fn effective_session_identity(
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

pub(super) fn require_expected_callee_fingerprint(
    expected: Option<golem_schema::proto::golem::common::Uuid>,
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
    path: &[StreamValuePathStep],
) -> Result<Option<&'a SchemaType>, String> {
    let mut current = root;
    for step in path {
        current = graph
            .resolve_ref(current)
            .map_err(|error| error.to_string())?;
        current = match (step, current) {
            (StreamValuePathStep::RecordField(index), SchemaType::Record { fields, .. }) => {
                &fields
                    .get(*index as usize)
                    .ok_or_else(|| "stream record path is out of range".to_string())?
                    .body
            }
            (StreamValuePathStep::VariantCasePayload(index), SchemaType::Variant { cases, .. }) => {
                cases
                    .get(*index as usize)
                    .and_then(|case| case.payload.as_ref())
                    .ok_or_else(|| "stream variant path has no payload".to_string())?
            }
            (StreamValuePathStep::TupleElement(index), SchemaType::Tuple { elements, .. }) => {
                elements
                    .get(*index as usize)
                    .ok_or_else(|| "stream tuple path is out of range".to_string())?
            }
            (StreamValuePathStep::ListElement(_), SchemaType::List { element, .. })
            | (StreamValuePathStep::FixedListElement(_), SchemaType::FixedList { element, .. }) => {
                element
            }
            (
                StreamValuePathStep::MapEntry {
                    side: golem_common::model::durable_stream::StreamMapSide::Key,
                    ..
                },
                SchemaType::Map { key, .. },
            ) => key,
            (
                StreamValuePathStep::MapEntry {
                    side: golem_common::model::durable_stream::StreamMapSide::Value,
                    ..
                },
                SchemaType::Map { value, .. },
            ) => value,
            (StreamValuePathStep::OptionSome, SchemaType::Option { inner, .. }) => inner,
            (StreamValuePathStep::ResultOk, SchemaType::Result { spec, .. }) => spec
                .ok
                .as_deref()
                .ok_or_else(|| "stream result ok path has no payload".to_string())?,
            (StreamValuePathStep::ResultErr, SchemaType::Result { spec, .. }) => spec
                .err
                .as_deref()
                .ok_or_else(|| "stream result error path has no payload".to_string())?,
            (StreamValuePathStep::UnionBranch(index), SchemaType::Union { spec, .. }) => {
                &spec
                    .branches
                    .get(*index as usize)
                    .ok_or_else(|| "stream union path is out of range".to_string())?
                    .body
            }
            _ => {
                return Err("stream value path does not match the pinned input schema".to_string());
            }
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
        AgentInvocation::ExternalTool {
            idempotency_key,
            tool_name,
            command_path,
            input,
            stdin,
            stdout,
            stderr,
            activation,
            invocation_context,
            principal,
            scope_card,
        } => {
            let (graph, value) = input.into_parts();
            AgentInvocation::ExternalTool {
                idempotency_key,
                tool_name,
                command_path,
                input: Box::new(golem_common::schema::TypedSchemaValue::new(
                    graph,
                    erase_streams(value),
                )),
                stdin,
                stdout,
                stderr,
                activation,
                invocation_context,
                principal,
                scope_card,
            }
        }
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

pub(super) fn request_idempotency_key(
    request: &InvocationRequest,
) -> Option<golem_api_grpc::proto::golem::worker::IdempotencyKey> {
    match request.request.as_ref() {
        Some(invocation_request::Request::Start(start)) => start.idempotency_key.clone(),
        Some(invocation_request::Request::ResumeAttach(resume)) => resume.idempotency_key.clone(),
        _ => None,
    }
}

pub(super) fn request_agent_id(
    request: &InvocationRequest,
) -> Option<golem_api_grpc::proto::golem::worker::AgentId> {
    match request.request.as_ref() {
        Some(invocation_request::Request::Start(start)) => start.agent_id.clone(),
        _ => None,
    }
}

pub(super) async fn route_durable_request(
    streams: &StreamSession,
    responses: &mpsc::Sender<InvocationResponse>,
    state: &Arc<tokio::sync::Mutex<InvocationSessionState>>,
    request: InvocationRequest,
) -> Result<(), SessionError> {
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
                    SessionStreamRole::Input,
                )
                .await?;
            let payload = match item.payload {
                Some(golem_api_grpc::proto::golem::worker::input_stream_item::Payload::Value(
                    value,
                )) => StreamItemsPayload::Values(vec![value.encode_to_vec()]),
                Some(
                    golem_api_grpc::proto::golem::worker::input_stream_item::Payload::PackedU8(
                        bytes,
                    ),
                ) => StreamItemsPayload::PackedU8(bytes),
                None => return Err("durable input item has no payload".to_string().into()),
            };
            streams
                .write_input(None, item.transport_stream_id, item.sequence, payload)
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
                    SessionStreamRole::Input,
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
                golem_api_grpc::proto::golem::worker::StreamCancelRole::InputProducer => {
                    (StreamCancelRole::InputProducer, SessionStreamRole::Input)
                }
                golem_api_grpc::proto::golem::worker::StreamCancelRole::InputConsumer => {
                    (StreamCancelRole::InputConsumer, SessionStreamRole::Input)
                }
                golem_api_grpc::proto::golem::worker::StreamCancelRole::OutputProducer => {
                    (StreamCancelRole::OutputProducer, SessionStreamRole::Output)
                }
                golem_api_grpc::proto::golem::worker::StreamCancelRole::OutputConsumer => {
                    (StreamCancelRole::OutputConsumer, SessionStreamRole::Output)
                }
                golem_api_grpc::proto::golem::worker::StreamCancelRole::System => {
                    return Err("system-authored durable stream cancellation is internal"
                        .to_string()
                        .into());
                }
                golem_api_grpc::proto::golem::worker::StreamCancelRole::Unspecified => {
                    return Err("durable stream cancel role is unspecified"
                        .to_string()
                        .into());
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
                    StreamCancelReason::Cancelled
                }
                golem_api_grpc::proto::golem::worker::StreamCancelReason::Protocol => {
                    StreamCancelReason::Protocol
                }
                golem_api_grpc::proto::golem::worker::StreamCancelReason::ConsumerDrop => {
                    StreamCancelReason::GuestDrop
                }
                golem_api_grpc::proto::golem::worker::StreamCancelReason::Transport => {
                    return Err(
                        "transport loss detaches a durable session and cannot cancel a stream"
                            .to_string()
                            .into(),
                    );
                }
                golem_api_grpc::proto::golem::worker::StreamCancelReason::SourceUnavailable
                    if role == StreamCancelRole::InputProducer =>
                {
                    StreamCancelReason::SourceUnavailable
                }
                golem_api_grpc::proto::golem::worker::StreamCancelReason::SourceUnavailable
                | golem_api_grpc::proto::golem::worker::StreamCancelReason::ProducerDeleting
                | golem_api_grpc::proto::golem::worker::StreamCancelReason::InvocationFailed => {
                    return Err(
                        "system-authored durable stream cancellation reason is internal"
                            .to_string()
                            .into(),
                    );
                }
                golem_api_grpc::proto::golem::worker::StreamCancelReason::Unspecified => {
                    return Err("durable stream cancel reason is unspecified"
                        .to_string()
                        .into());
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
            return Err("unexpected message on durable invocation request stream"
                .to_string()
                .into());
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
