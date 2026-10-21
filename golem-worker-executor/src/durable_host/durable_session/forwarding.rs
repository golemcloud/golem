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

use super::*;

impl StreamSession {
    /// Activates a prepared forwarded topology after validating producer and consumer identities.
    pub async fn activate_forwarded_mapping(
        &self,
        attachment: StreamAttachmentKey,
        mapping: StreamSessionMappingRecord,
        producer_control: Arc<dyn StreamAttachmentControl + Send + Sync>,
        now_millis: u64,
    ) -> Result<(), SessionError> {
        let memory = golem_common::serialization::serialize(&attachment)?.len()
            + golem_common::serialization::serialize(&mapping)?.len();
        let session = self.clone();
        self.producer
            .run_admitted(None, memory, false, move |_, admission| async move {
                let _session_guard = session.session_lock.lock().await;
                session
                    .activate_forwarded_mapping_under_lock(
                        &admission,
                        attachment,
                        mapping,
                        producer_control.as_ref(),
                        now_millis,
                    )
                    .await
            })
            .await
    }

    pub(super) async fn activate_forwarded_mapping_under_lock(
        &self,
        admission: &Arc<StreamWriteAdmission>,
        attachment: StreamAttachmentKey,
        mapping: StreamSessionMappingRecord,
        producer_control: &(dyn StreamAttachmentControl + Send + Sync),
        now_millis: u64,
    ) -> Result<(), SessionError> {
        self.validate_forwarded_mapping(&attachment, &mapping)?;
        if self
            .mapping(mapping.transport_stream_id)
            .is_some_and(|existing| existing != mapping)
        {
            return Err(SessionError::from(
                "forwarded durable stream mapping conflicts with session topology".to_string(),
            ));
        }
        let topology = self.topology_state(&attachment, Some(&mapping)).await?;
        if matches!(
            topology,
            ConsumerAttachmentStatus::IncarnationMismatch | ConsumerAttachmentStatus::EpochMismatch
        ) {
            return Err(SessionError::from(
                "forwarded stream attachment conflicts with durable topology".to_string(),
            ));
        }
        if topology != ConsumerAttachmentStatus::Active {
            self.producer
                .ensure_session_accepts_new_events(&self.session_key)
                .await
                .map_err(SessionError::from)?;
        }
        if topology == ConsumerAttachmentStatus::Missing {
            self.try_append_record(
                admission,
                StreamSessionRecord::TopologyPrepared(StreamTopologyPreparedRecord {
                    format_version: DURABLE_STREAM_FORMAT_VERSION,
                    session_key: self.session_key.clone(),
                    attachment: attachment.clone(),
                    mapping: mapping.clone(),
                }),
            )
            .await?;
            self.commit_consumer_journal().await?;
        }
        self.producer
            .remote_until_retired(
                producer_control.prepare_attachment(attachment.clone(), now_millis),
            )
            .await
            .map_err(SessionError::from)?;
        if topology != ConsumerAttachmentStatus::Active {
            self.require_local_session_attachment(&attachment).await?;
            self.try_append_record(
                admission,
                StreamSessionRecord::TopologyActivated(StreamTopologyActivatedRecord {
                    format_version: DURABLE_STREAM_FORMAT_VERSION,
                    session_key: self.session_key.clone(),
                    attachment: attachment.clone(),
                    mapping: mapping.clone(),
                }),
            )
            .await?;
            self.commit_consumer_journal().await?;
        }
        self.producer
            .remote_until_retired(producer_control.activate_attachment(attachment, now_millis))
            .await
            .map_err(SessionError::from)?;
        let session = self.clone();
        let binding = StreamBindingRecord::foreign(&mapping);
        admission
            .submit(move |_, context| async move {
                session.append_mapping_once(&context, binding).await
            })
            .await?;
        self.commit_consumer_journal().await?;
        self.insert_mapping(mapping)
    }
}

/// Encodes a fingerprint-bound session mapping for transport.
pub fn durable_stream_mapping_to_proto(
    mapping: &StreamSessionMappingRecord,
    high_water: Option<&InputStreamHighWater>,
) -> DurableStreamMapping {
    DurableStreamMapping {
        transport_stream_id: mapping.transport_stream_id,
        handle: Some(ProtoDurableStreamHandle {
            format_version: u32::from(mapping.handle.format_version),
            stream_id: Some(mapping.handle.stream_id.0.into()),
            producer_environment_id: Some(mapping.handle.producer_environment_id.into()),
            producer: Some(mapping.handle.producer.clone().into()),
            expected_producer_fingerprint: Some(
                mapping.handle.expected_producer_fingerprint.0.into(),
            ),
            producer_generation: mapping.handle.producer_generation.as_u64(),
            source_invocation: Some(StreamInvocationIdentity {
                callee_environment_id: Some(
                    mapping
                        .handle
                        .source_invocation
                        .callee_environment_id
                        .into(),
                ),
                callee: Some(mapping.handle.source_invocation.callee.clone().into()),
                callee_fingerprint: Some(
                    mapping.handle.source_invocation.callee_fingerprint.0.into(),
                ),
                idempotency_key: Some(
                    mapping
                        .handle
                        .source_invocation
                        .idempotency_key
                        .clone()
                        .into(),
                ),
            }),
            component_revision: Some(mapping.handle.component_revision.get()),
            element_schema_fingerprint: mapping.handle.element_schema_fingerprint.0.to_vec(),
        }),
        high_water: high_water.map(|high_water| ProtoInputStreamHighWater {
            highest_contiguous_sequence: high_water.highest_contiguous_sequence,
            resulting_offset: high_water.resulting_offset.as_bytes().to_vec(),
            terminal: high_water.terminal,
        }),
        role: match mapping.role.direction() {
            SessionStreamRole::Input => StreamMappingRole::Input as i32,
            SessionStreamRole::Output => StreamMappingRole::Output as i32,
            SessionStreamRole::ToolStdin
            | SessionStreamRole::ToolStdout
            | SessionStreamRole::ToolStderr => unreachable!(),
        },
        tool_byte_stream_role: mapping.role.tool_byte_stream_role().map(|role| match role {
            ToolByteStreamRole::Stdin => ProtoToolByteStreamRole::Stdin as i32,
            ToolByteStreamRole::Stdout => ProtoToolByteStreamRole::Stdout as i32,
            ToolByteStreamRole::Stderr => ProtoToolByteStreamRole::Stderr as i32,
        }),
    }
}

/// Decodes and validates the complete durable identity carried by a mapping.
pub fn durable_stream_mapping_from_proto(
    mapping: DurableStreamMapping,
) -> Result<StreamSessionMappingRecord, String> {
    let handle = mapping
        .handle
        .ok_or_else(|| "durable stream mapping has no handle".to_string())?;
    if handle.format_version != u32::from(DURABLE_STREAM_FORMAT_VERSION) {
        return Err("unsupported durable stream handle format version".to_string());
    }
    let source = handle
        .source_invocation
        .ok_or_else(|| "durable stream handle has no source invocation".to_string())?;
    let element_schema_fingerprint: [u8; 32] = handle
        .element_schema_fingerprint
        .try_into()
        .map_err(|_| "durable stream schema fingerprint must contain 32 bytes".to_string())?;
    let direction =
        match StreamMappingRole::try_from(mapping.role).unwrap_or(StreamMappingRole::Unspecified) {
            StreamMappingRole::Input => SessionStreamRole::Input,
            StreamMappingRole::Output => SessionStreamRole::Output,
            StreamMappingRole::Unspecified => {
                return Err("durable stream mapping has no role".to_string());
            }
        };
    let byte_stream_role = mapping
        .tool_byte_stream_role
        .map(|role| {
            ProtoToolByteStreamRole::try_from(role)
                .map_err(|_| "durable stream mapping has an invalid tool byte role".to_string())
                .map(|role| match role {
                    ProtoToolByteStreamRole::Stdin => ToolByteStreamRole::Stdin,
                    ProtoToolByteStreamRole::Stdout => ToolByteStreamRole::Stdout,
                    ProtoToolByteStreamRole::Stderr => ToolByteStreamRole::Stderr,
                })
        })
        .transpose()?;
    if matches!(byte_stream_role, Some(ToolByteStreamRole::Stdin))
        != matches!(direction, SessionStreamRole::Input)
        && byte_stream_role.is_some()
    {
        return Err("tool byte stream role conflicts with mapping direction".to_string());
    }
    let role = match byte_stream_role {
        Some(ToolByteStreamRole::Stdin) => SessionStreamRole::ToolStdin,
        Some(ToolByteStreamRole::Stdout) => SessionStreamRole::ToolStdout,
        Some(ToolByteStreamRole::Stderr) => SessionStreamRole::ToolStderr,
        None => direction,
    };
    Ok(StreamSessionMappingRecord {
        transport_stream_id: mapping.transport_stream_id,
        handle: DurableStreamHandle {
            format_version: DURABLE_STREAM_FORMAT_VERSION,
            stream_id: golem_common::model::durable_stream::StreamId(
                handle
                    .stream_id
                    .ok_or_else(|| "durable stream handle has no stream ID".to_string())?
                    .into(),
            ),
            producer_environment_id: handle
                .producer_environment_id
                .ok_or_else(|| "durable stream handle has no producer environment".to_string())?
                .try_into()?,
            producer: handle
                .producer
                .ok_or_else(|| "durable stream handle has no producer".to_string())?
                .try_into()?,
            expected_producer_fingerprint: golem_common::model::AgentFingerprint(
                handle
                    .expected_producer_fingerprint
                    .ok_or_else(|| "durable stream handle has no producer fingerprint".to_string())?
                    .into(),
            ),
            producer_generation: OplogIndex::from_u64(handle.producer_generation),
            source_invocation: StreamInvocationId {
                callee_environment_id: source
                    .callee_environment_id
                    .ok_or_else(|| "durable stream source has no callee environment".to_string())?
                    .try_into()?,
                callee: source
                    .callee
                    .ok_or_else(|| "durable stream source has no callee".to_string())?
                    .try_into()?,
                callee_fingerprint: golem_common::model::AgentFingerprint(
                    source
                        .callee_fingerprint
                        .ok_or_else(|| {
                            "durable stream source has no callee fingerprint".to_string()
                        })?
                        .into(),
                ),
                idempotency_key: source
                    .idempotency_key
                    .ok_or_else(|| "durable stream source has no idempotency key".to_string())?
                    .into(),
            },
            component_revision: golem_common::model::component::ComponentRevision::new(
                handle
                    .component_revision
                    .ok_or_else(|| "durable stream handle has no component revision".to_string())?,
            )
            .map_err(|error| error.to_string())?,
            element_schema_fingerprint: SchemaFingerprintV1(element_schema_fingerprint),
        },
        role,
    })
}

pub(super) fn stream_cancel_reason_to_proto(
    reason: StreamCancelReason,
) -> golem_api_grpc::proto::golem::worker::StreamCancelReason {
    match reason {
        StreamCancelReason::Cancelled => {
            golem_api_grpc::proto::golem::worker::StreamCancelReason::Cancelled
        }
        StreamCancelReason::GuestDrop => {
            golem_api_grpc::proto::golem::worker::StreamCancelReason::ConsumerDrop
        }
        StreamCancelReason::Protocol => {
            golem_api_grpc::proto::golem::worker::StreamCancelReason::Protocol
        }
        StreamCancelReason::InvocationFailed => {
            golem_api_grpc::proto::golem::worker::StreamCancelReason::InvocationFailed
        }
        StreamCancelReason::SourceUnavailable => {
            golem_api_grpc::proto::golem::worker::StreamCancelReason::SourceUnavailable
        }
        StreamCancelReason::ProducerDeleting => {
            golem_api_grpc::proto::golem::worker::StreamCancelReason::ProducerDeleting
        }
    }
}

impl ForwardedDurableInput {
    /// Selects a stable owner-journal destination without activating its attachment.
    pub(super) async fn prepare_mapping_owned(
        self,
        context: &StreamWriteContext,
        destination: &StreamSession,
        role: SessionStreamRole,
        publication: StreamReaderForwardPublication,
    ) -> Result<StreamSessionMappingRecord, SessionError> {
        destination.recover_session_mappings().await?;
        let reader_id = self.reader_id.clone().await?;
        let metadata = self.origin.current_control_metadata().await?;
        let existing = metadata.reader_forward_intent(reader_id)?.cloned();
        drop(metadata);
        let transport_stream_id = if let Some((_, intent)) = existing {
            match intent.destination {
                StreamReaderForwardDestination::SessionBinding {
                    session_key,
                    binding,
                    ..
                } if session_key == destination.session_reference && binding.role == role => {
                    binding.transport_stream_id
                }
                _ => {
                    return Err(
                        "durable reader already has a different forwarding destination".into(),
                    );
                }
            }
        } else {
            destination
                .mapping_for_reference(&StreamRecordReference::Foreign(self.handle.clone()), role)
                .map(|mapping| Ok(mapping.transport_stream_id))
                .unwrap_or_else(|| destination.allocate_transport_stream_id())?
        };
        let mapping = StreamSessionMappingRecord {
            transport_stream_id,
            handle: self.handle.clone(),
            role,
        };
        let (_, intent) = self
            .prepare_forward_owned(
                context,
                destination,
                StreamReaderForwardDestination::SessionBinding {
                    session_key: destination.session_reference.clone(),
                    binding: StreamBindingRecord::foreign(&mapping),
                    publication,
                },
            )
            .await?;
        let StreamReaderForwardDestination::SessionBinding { binding, .. } = intent.destination
        else {
            unreachable!("mapping preparation validates its destination kind")
        };
        destination
            .producer
            .materialize_binding(&binding)
            .await
            .map_err(SessionError::from)
    }

    /// Resolves historical destination identity without granting live attachment authority.
    pub(super) async fn prepare_forward_owned(
        &self,
        context: &StreamWriteContext,
        destination_session: &StreamSession,
        mut destination: StreamReaderForwardDestination,
    ) -> Result<(OplogIndex, StreamReaderForwardIntentRecord), SessionError> {
        if !Arc::ptr_eq(&self.origin.producer, &destination_session.producer) {
            return Err("forwarded reader belongs to a different stream store".into());
        }
        context.assert_owner(&self.origin.producer);
        let reader_id = self.reader_id.clone().await?;
        let metadata = self.origin.current_control_metadata().await?;
        let source = metadata.reader_binding(reader_id)?.clone();
        let existing = metadata.reader_forward_intent(reader_id)?.cloned();
        if metadata.consumer_record_count(reader_id) != 0 {
            return Err("cannot forward a durable input stream after reading from it".into());
        }
        drop(metadata);
        let current = self.origin.producer.materialize_binding(&source).await?;
        if current.handle != self.handle {
            return Err("forwarded handle does not match its original reader binding".into());
        }
        if let Some((_, intent)) = &existing {
            // Local readers are requalified on fork/revert; their recorded destinations are not.
            if matches!(source.source, StreamRecordReference::Local(_)) {
                match (&mut destination, &intent.destination) {
                    (
                        StreamReaderForwardDestination::SessionBinding { binding, .. },
                        StreamReaderForwardDestination::SessionBinding {
                            binding: recorded, ..
                        },
                    ) => binding.source = recorded.source.clone(),
                    (
                        StreamReaderForwardDestination::InvocationInput { mapping, .. },
                        StreamReaderForwardDestination::InvocationInput {
                            mapping: recorded, ..
                        },
                    ) => mapping.handle = recorded.handle.clone(),
                    _ => {}
                }
            }
            if destination != intent.destination {
                return Err("durable reader already has a different forwarding destination".into());
            }
        }
        let binding = match &destination {
            StreamReaderForwardDestination::SessionBinding { binding, .. } => binding.clone(),
            StreamReaderForwardDestination::InvocationInput { mapping, .. } => {
                StreamBindingRecord::foreign(mapping)
            }
        };
        if destination_session
            .binding(binding.transport_stream_id)
            .is_some_and(|existing| existing != binding)
        {
            return Err("forwarding destination slot is already bound to another stream".into());
        }
        let destination_metadata = destination_session.current_control_metadata().await?;
        for reserved in destination_metadata.incoming_reader_forwards().values() {
            let same_binding = match (&reserved.destination, &destination) {
                (
                    StreamReaderForwardDestination::SessionBinding {
                        session_key: reserved_session,
                        binding: reserved_binding,
                        ..
                    },
                    StreamReaderForwardDestination::SessionBinding {
                        session_key,
                        binding,
                        ..
                    },
                ) => reserved_session == session_key && reserved_binding == binding,
                _ => reserved.destination == destination,
            };
            if reserved.destination.transport_stream_id() == binding.transport_stream_id
                && !same_binding
            {
                return Err("forwarding destination slot is reserved for another stream".into());
            }
        }
        drop(destination_metadata);
        if let Some(existing) = existing {
            return Ok(existing);
        }
        let intent = StreamReaderForwardIntentRecord {
            format_version: DURABLE_STREAM_FORMAT_VERSION,
            session_key: self.origin.session_reference.clone(),
            reader_id,
            destination,
        };
        let indices = self
            .origin
            .producer
            .append_session_records_owned(
                context,
                self.origin.entity_parent_start_index,
                vec![StreamSessionRecord::ReaderForwardIntent(intent.clone())],
            )
            .await?;
        Ok((indices[0], intent))
    }
}

impl DurableInputEndpoint {
    pub(super) fn forwarded_handle(&self) -> Result<DurableStreamHandle, String> {
        if self.consumer_read_ordinal != 0 || !self.journal.is_empty() {
            return Err("cannot forward a durable input stream after reading from it".to_string());
        }
        Ok(self.handle.clone())
    }

    pub(super) fn into_forwarded(self) -> Result<ForwardedDurableInput, String> {
        self.forwarded_handle()?;
        Ok(ForwardedDurableInput {
            handle: self.handle,
            origin: self.streams,
            reader_id: self.reader_id,
        })
    }
}

impl ForwardedDurableInputReference {
    pub(super) fn handle(&self) -> &DurableStreamHandle {
        match self {
            Self::Forwarded(handle) | Self::Endpoint(handle) => handle,
        }
    }

    pub(super) fn take(self, stream: &SchemaValueStream) -> Result<ForwardedDurableInput, String> {
        match self {
            Self::Forwarded(_) => stream.take_host_endpoint::<ForwardedDurableInput>(),
            Self::Endpoint(_) => stream
                .take_host_endpoint::<DurableInputEndpoint>()?
                .into_forwarded(),
        }
    }
}

pub(super) fn forwarded_durable_input_reference(
    stream: &SchemaValueStream,
) -> Result<Option<ForwardedDurableInputReference>, String> {
    if stream
        .with_host_endpoint::<ForwardedDurableInput, _>(|_| ())
        .is_ok()
    {
        return stream
            .with_host_endpoint::<ForwardedDurableInput, _>(|forwarded| forwarded.handle.clone())
            .map(ForwardedDurableInputReference::Forwarded)
            .map(Some);
    }
    if stream
        .with_host_endpoint::<DurableInputEndpoint, _>(|_| ())
        .is_ok()
    {
        return stream
            .with_host_endpoint::<DurableInputEndpoint, _>(DurableInputEndpoint::forwarded_handle)?
            .map(ForwardedDurableInputReference::Endpoint)
            .map(Some);
    }
    Ok(None)
}

pub(super) fn validate_forwarded_durable_input_schemas(
    value: &SchemaValue,
    graph: &SchemaGraph,
    root: &SchemaType,
    mismatch_error: &str,
) -> Result<(), String> {
    let mut seen_streams = HashSet::new();
    encode_recursive_stream_value_with_schema(value, graph, root, |stream, path| {
        if !seen_streams.insert(stream.cell_id()) {
            return Err(
                "the same affine stream appeared more than once in one value tree".to_string(),
            );
        }
        let element = stream_element_schema(graph, root, path)?;
        let element_schema_fingerprint =
            schema_fingerprint_v1(graph, element).map_err(|error| error.to_string())?;
        match forwarded_durable_input_reference(stream)? {
            Some(forwarded)
                if forwarded.handle().format_version != DURABLE_STREAM_FORMAT_VERSION
                    || forwarded.handle().element_schema_fingerprint
                        != element_schema_fingerprint =>
            {
                return Err(mismatch_error.into());
            }
            Some(_) => {}
            None if stream
                .with_host_endpoint::<RegisteredOutputStream, _>(|_| ())
                .is_ok() => {}
            None => stream.with_host_endpoint::<LiveStreamEndpoint, _>(|_| ())?,
        }
        Ok(0)
    })?;
    Ok(())
}

pub(super) fn stream_element_schema<'a>(
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
                return Err("stream value path does not match the pinned schema".to_string());
            }
        };
    }
    match graph
        .resolve_ref(current)
        .map_err(|error| error.to_string())?
    {
        SchemaType::Stream { inner, .. } => Ok(inner.as_deref()),
        _ => Err("stream reference is not at a stream node in the pinned schema".to_string()),
    }
}

/// Removes runtime stream resources from a result before persisting the RPC value.
pub fn strip_streams(value: SchemaValue) -> SchemaValue {
    match value {
        SchemaValue::Stream(_) => SchemaValue::Tuple {
            elements: Vec::new(),
        },
        SchemaValue::Record { fields } => SchemaValue::Record {
            fields: fields.into_iter().map(strip_streams).collect(),
        },
        SchemaValue::Variant(mut value) => {
            value.payload = value
                .payload
                .map(|payload| Box::new(strip_streams(*payload)));
            SchemaValue::Variant(value)
        }
        SchemaValue::Tuple { elements } => SchemaValue::Tuple {
            elements: elements.into_iter().map(strip_streams).collect(),
        },
        SchemaValue::List { elements } => SchemaValue::List {
            elements: elements.into_iter().map(strip_streams).collect(),
        },
        SchemaValue::FixedList { elements } => SchemaValue::FixedList {
            elements: elements.into_iter().map(strip_streams).collect(),
        },
        SchemaValue::Map { entries } => SchemaValue::Map {
            entries: entries
                .into_iter()
                .map(|(key, value)| (strip_streams(key), strip_streams(value)))
                .collect(),
        },
        SchemaValue::Option { inner } => SchemaValue::Option {
            inner: inner.map(|inner| Box::new(strip_streams(*inner))),
        },
        SchemaValue::Result(mut result) => {
            match &mut result {
                golem_common::schema::schema_value::ResultValuePayload::Ok { value }
                | golem_common::schema::schema_value::ResultValuePayload::Err { value } => {
                    *value = value.take().map(|value| Box::new(strip_streams(*value)));
                }
            }
            SchemaValue::Result(result)
        }
        SchemaValue::Union(mut value) => {
            value.body = Box::new(strip_streams(*value.body));
            SchemaValue::Union(value)
        }
        other => other,
    }
}

/// Removes runtime stream resources while preserving a value's pinned schema graph.
pub(crate) fn strip_typed_streams(value: &TypedSchemaValue) -> TypedSchemaValue {
    TypedSchemaValue::new(value.graph().clone(), strip_streams(value.value().clone()))
}

pub(super) fn collect_stream_paths(
    value: &ProtoSchemaValue,
    graph: &SchemaGraph,
    root: &SchemaType,
) -> Result<Vec<(u64, Vec<StreamValuePathStep>)>, SessionError> {
    let mut result = Vec::new();
    decode_recursive_stream_value_with_schema(value.clone(), graph, root, |stream_id, path| {
        result.push((stream_id, path.to_vec()));
        Ok(SchemaValueStream::from_host_endpoint(()))
    })?;
    Ok(result)
}
