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
    /// Commits activation of a prepared cross-agent mapping and its attachment epoch.
    pub async fn activate_foreign_mapping(
        &self,
        mapping: StreamSessionMappingRecord,
        epoch: u64,
    ) -> Result<(), SessionError> {
        if self.producer.owns_handle_identity(&mapping.handle) {
            return Err(SessionError::from(
                "foreign durable mapping is owned by the local producer".to_string(),
            ));
        }
        let rpc = self
            .rpc
            .clone()
            .ok_or_else(|| "foreign durable stream control routing is unavailable".to_string())?;
        let auth_ctx = self.auth_ctx.clone().ok_or_else(|| {
            "foreign durable stream consumer authorization is unavailable".to_string()
        })?;
        let attachment = self.attachment_key(&mapping.handle, epoch)?;
        let control = RoutedStreamAttachmentControl::new(rpc, mapping.clone(), auth_ctx);
        self.activate_forwarded_mapping(
            attachment,
            mapping,
            Arc::new(control),
            Timestamp::now_utc().to_millis(),
        )
        .await
    }

    /// Prepares the producer attachment before recording the consumer topology.
    pub async fn prepare_foreign_mapping(
        &self,
        mapping: StreamSessionMappingRecord,
        epoch: u64,
    ) -> Result<(), SessionError> {
        let memory = golem_common::serialization::serialize(&mapping)?.len();
        let session = self.clone();
        self.producer
            .run_admitted(None, memory, false, move |_, admission| async move {
                session
                    .prepare_foreign_mapping_owned(&admission, mapping, epoch)
                    .await
            })
            .await
    }

    async fn prepare_foreign_mapping_owned(
        &self,
        admission: &Arc<StreamWriteAdmission>,
        mapping: StreamSessionMappingRecord,
        epoch: u64,
    ) -> Result<(), SessionError> {
        if self.producer.owns_handle_identity(&mapping.handle) {
            return Err(SessionError::from(
                "foreign durable mapping is owned by the local producer".to_string(),
            ));
        }
        let rpc = self
            .rpc
            .clone()
            .ok_or_else(|| "foreign durable stream control routing is unavailable".to_string())?;
        let auth_ctx = self.auth_ctx.clone().ok_or_else(|| {
            "foreign durable stream consumer authorization is unavailable".to_string()
        })?;
        let attachment = self.attachment_key(&mapping.handle, epoch)?;
        let control = RoutedStreamAttachmentControl::new(rpc, mapping.clone(), auth_ctx);
        let _session_guard = self.session_lock.lock().await;
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
        if topology == ConsumerAttachmentStatus::Active {
            return Ok(());
        }
        if topology == ConsumerAttachmentStatus::Missing {
            if !self.persisted_session_mapping(&mapping).await? {
                self.producer
                    .ensure_session_accepts_new_events(&self.session_key)
                    .await
                    .map_err(SessionError::from)?;
            }
            self.try_append_record(
                admission,
                StreamSessionRecord::TopologyPrepared(StreamTopologyPreparedRecord {
                    format_version: DURABLE_STREAM_FORMAT_VERSION,
                    session_key: self.session_key.clone(),
                    attachment: attachment.clone(),
                    mapping,
                }),
            )
            .await?;
            self.commit_consumer_journal().await?;
        }
        self.producer
            .remote_until_retired(
                control.prepare_attachment(attachment, Timestamp::now_utc().to_millis()),
            )
            .await
            .map_err(SessionError::from)?;
        Ok(())
    }

    async fn persisted_session_mapping(
        &self,
        expected: &StreamSessionMappingRecord,
    ) -> Result<bool, SessionError> {
        let binding = StreamBindingRecord::foreign(expected);
        Ok(self
            .current_control_metadata()
            .await?
            .has_persisted_mapping(&binding))
    }

    #[cfg(test)]
    pub(super) async fn ensure_nested_mapping(
        &self,
        context: Option<&Arc<StreamWriteAdmission>>,
        handle: DurableStreamHandle,
        role: SessionStreamRole,
    ) -> Result<StreamSessionMappingRecord, SessionError> {
        let memory = golem_common::serialization::serialize(&handle)?.len();
        let session = self.clone();
        self.producer
            .run_admitted(context, memory, false, move |_, admission| async move {
                let _session_guard = session.session_lock.lock().await;
                session
                    .ensure_nested_mapping_under_lock(&admission, handle, role)
                    .await
            })
            .await
    }

    #[cfg(test)]
    async fn ensure_nested_mapping_under_lock(
        &self,
        admission: &Arc<StreamWriteAdmission>,
        handle: DurableStreamHandle,
        role: SessionStreamRole,
    ) -> Result<StreamSessionMappingRecord, SessionError> {
        self.recover_session_mappings().await?;
        if let Some(mapping) =
            self.mapping_for_reference(&StreamRecordReference::Foreign(handle.clone()), role)
        {
            return Ok(mapping);
        }
        let mapping = StreamSessionMappingRecord {
            transport_stream_id: self.allocate_transport_stream_id()?,
            handle,
            role,
        };
        self.activate_nested_mapping_under_lock(admission, mapping.clone())
            .await?;
        Ok(mapping)
    }

    pub(super) async fn activate_nested_mapping_under_lock(
        &self,
        admission: &Arc<StreamWriteAdmission>,
        mapping: StreamSessionMappingRecord,
    ) -> Result<(), StreamStoreError> {
        if self.producer.owns_handle_identity(&mapping.handle) {
            self.producer.validate_handle(&mapping.handle).await?;
            let binding = StreamBindingRecord::foreign(&mapping);
            let session = self.clone();
            let persisted_binding = binding.clone();
            admission
                .submit(move |_, context| async move {
                    session
                        .append_mapping_once(&context, persisted_binding)
                        .await
                })
                .await?;
            self.insert_binding_mapping(binding, mapping.clone())?;
        } else {
            let rpc = self.rpc.clone().ok_or_else(|| {
                StreamStoreError::Oplog(
                    "foreign durable stream control routing is unavailable".to_string(),
                )
            })?;
            let auth_ctx = self.auth_ctx.clone().ok_or_else(|| {
                StreamStoreError::Oplog(
                    "foreign durable stream consumer authorization is unavailable".to_string(),
                )
            })?;
            let epoch = self.reader_epoch().await?;
            let attachment = self.attachment_key(&mapping.handle, epoch)?;
            let control = RoutedStreamAttachmentControl::new(rpc, mapping.clone(), auth_ctx);
            self.activate_forwarded_mapping_under_lock(
                admission,
                attachment,
                mapping.clone(),
                &control,
                Timestamp::now_utc().to_millis(),
            )
            .await?;
        }
        Ok(())
    }

    /// Attaches a fingerprint-validated foreign handle and returns its transport mapping.
    pub(super) async fn attach_foreign_handle(
        &self,
        handle: DurableStreamHandle,
        role: SessionStreamRole,
        epoch: u64,
    ) -> Result<StreamSessionMappingRecord, SessionError> {
        if self.producer.owns_handle_identity(&handle) {
            return Err(SessionError::from(
                "attached durable stream handle is owned by the consumer".to_string(),
            ));
        }
        if let Some(mapping) =
            self.mapping_for_reference(&StreamRecordReference::Foreign(handle.clone()), role)
        {
            return Ok(mapping);
        }
        let mapping = StreamSessionMappingRecord {
            transport_stream_id: self.allocate_transport_stream_id()?,
            handle,
            role,
        };
        self.activate_foreign_mapping(mapping.clone(), epoch)
            .await?;
        Ok(mapping)
    }

    /// Validates, commits, and acknowledges one input frame for the current attachment.
    pub async fn write_input(
        &self,
        context: Option<&Arc<StreamWriteAdmission>>,
        transport_stream_id: u64,
        first_sequence: u64,
        payload: StreamItemsPayload,
    ) -> Result<
        Option<(
            u64,
            u64,
            golem_common::model::durable_stream::StreamOffset,
            Vec<StreamSessionMappingRecord>,
        )>,
        SessionError,
    > {
        let memory = DurableStreamStore::retained_payload_bytes(&payload)?;
        let session = self.clone();
        self.producer
            .run_admitted(context, memory, false, move |_, admission| async move {
                let _session_guard = session.session_lock.lock().await;
                let session_for_write = session.clone();
                admission
                    .submit(move |_, context| async move {
                        session_for_write
                            .write_input_owned(
                                &context,
                                transport_stream_id,
                                first_sequence,
                                payload,
                            )
                            .await
                    })
                    .await
            })
            .await
    }

    async fn write_input_owned(
        &self,
        context: &StreamWriteContext,
        transport_stream_id: u64,
        first_sequence: u64,
        payload: StreamItemsPayload,
    ) -> Result<
        Option<(
            u64,
            u64,
            golem_common::model::durable_stream::StreamOffset,
            Vec<StreamSessionMappingRecord>,
        )>,
        SessionError,
    > {
        self.ensure_current_attachment().await?;
        let handle = self
            .handle(transport_stream_id)
            .ok_or_else(|| format!("unknown durable input stream {transport_stream_id}"))?;
        let logical_item_count = u64::try_from(payload.logical_item_count())
            .map_err(|_| "durable input item count does not fit in u64".to_string())?;
        let mut nested_transport_ids = Vec::new();
        let mut nested_requests = Vec::new();
        let mut nested_element_types = Vec::new();
        if let StreamItemsPayload::Values(values) = &payload {
            for (item_index, value) in values.iter().enumerate() {
                let value = ProtoSchemaValue::decode(value.as_slice())
                    .map_err(|error| format!("invalid durable input value: {error}"))?;
                let input_schema = self.input_schema.as_ref().ok_or_else(|| {
                    "nested durable input streams require the persisted input schema".to_string()
                })?;
                let parent_element = input_schema
                    .element_types
                    .read()
                    .expect("durable input schema lock poisoned")
                    .get(&transport_stream_id)
                    .cloned()
                    .ok_or_else(|| {
                        format!("missing element schema for durable input {transport_stream_id}")
                    })?;
                let nested = collect_stream_paths(&value, &input_schema.graph, &parent_element)?;
                if nested.is_empty() {
                    continue;
                }
                let parent_producer_sequence = first_sequence
                    .checked_add(item_index as u64)
                    .ok_or_else(|| "durable input sequence overflow".to_string())?;
                for (nested_transport_id, path) in nested {
                    if nested_transport_ids.contains(&nested_transport_id) {
                        return Err(SessionError::from(format!(
                            "duplicate nested durable transport stream id {nested_transport_id}"
                        )));
                    }
                    let element =
                        stream_element_schema(&input_schema.graph, &parent_element, &path)?
                            .cloned();
                    let element_schema_fingerprint =
                        schema_fingerprint_v1(&input_schema.graph, element.as_ref())
                            .map_err(|error| error.to_string())?;
                    let coordinate = StreamRegistrationCoordinate::Nested {
                        parent_stream_id: handle.stream_id,
                        parent_producer_sequence,
                        recursive_value_path: path,
                    };
                    if let Some(existing) = self.handle(nested_transport_id) {
                        let global_parent_sequence = self
                            .producer
                            .attached_global_sequence(
                                &self.session_key,
                                handle.stream_id,
                                first_sequence,
                            )
                            .await
                            .map_err(SessionError::from)?
                            .checked_add(item_index as u64)
                            .ok_or_else(|| "durable input sequence overflow".to_string())?;
                        let mut global_coordinate = coordinate.clone();
                        if let StreamRegistrationCoordinate::Nested {
                            parent_producer_sequence,
                            ..
                        } = &mut global_coordinate
                        {
                            *parent_producer_sequence = global_parent_sequence;
                        }
                        if self
                            .producer
                            .handle_for_coordinate(&global_coordinate)
                            .await
                            .map_err(SessionError::from)?
                            .as_ref()
                            != Some(&existing)
                        {
                            return Err(SessionError::from(format!(
                                "nested durable transport stream id {nested_transport_id} conflicts with its persisted coordinate"
                            )));
                        }
                    }
                    nested_transport_ids.push(nested_transport_id);
                    nested_element_types
                        .push((nested_transport_id, element.unwrap_or_else(SchemaType::u8)));
                    nested_requests.push(ProducerRegistrationRequest {
                        entity_parent_start_index: self.entity_parent_start_index,
                        coordinate,
                        source_invocation: self.session_reference.clone(),
                        component_revision: input_schema.component_revision,
                        element_schema_fingerprint,
                        source_kind: StreamSourceKind::Nested,
                        session_mapping: None,
                    });
                }
            }
        }
        let canonical_payload = match &payload {
            StreamItemsPayload::Values(values) => {
                let mut next_handle_index = 0usize;
                let mut canonical_values = Vec::with_capacity(values.len());
                for value in values {
                    let value = ProtoSchemaValue::decode(value.as_slice())
                        .map_err(|error| format!("invalid durable input value: {error}"))?;
                    let value =
                        remap_recursive_stream_references(value, |transport_stream_id, _| {
                            let expected = nested_transport_ids
                                .get(next_handle_index)
                                .copied()
                                .ok_or_else(|| {
                                    "durable input contains an unexpected nested stream".to_string()
                                })?;
                            if expected != transport_stream_id {
                                return Err(
                                    "durable input stream traversal changed during canonicalization"
                                        .to_string(),
                                );
                            }
                            let canonical_index = u64::try_from(next_handle_index)
                                .map_err(|_| "durable input handle index overflow".to_string())?;
                            next_handle_index += 1;
                            Ok(canonical_index)
                        })?;
                    canonical_values.push(value.encode_to_vec());
                }
                if next_handle_index != nested_transport_ids.len() {
                    return Err(SessionError::from(
                        "durable input stream topology changed during canonicalization".to_string(),
                    ));
                }
                StreamItemsPayload::Values(canonical_values)
            }
            StreamItemsPayload::PackedU8(bytes) => StreamItemsPayload::PackedU8(bytes.clone()),
        };
        let outcome = match self
            .producer
            .write_attached_items_with_nested(
                Some(context),
                &self.session_key,
                handle.stream_id,
                first_sequence,
                canonical_payload,
                nested_requests,
            )
            .await
        {
            Ok(outcome) => outcome,
            Err(error) if discards_input_after_terminal(&error, &self.session_key) => {
                tracing::debug!(
                    transport_stream_id,
                    first_sequence,
                    error = %error,
                    "Discarding late durable input items after terminal"
                );
                return Ok(None);
            }
            Err(error) => {
                tracing::warn!(
                    transport_stream_id,
                    first_sequence,
                    error = %error,
                    "Rejecting durable input items"
                );
                return Err(SessionError::from(error));
            }
        };
        let mut nested_mappings = Vec::with_capacity(nested_transport_ids.len());
        if !nested_transport_ids.is_empty() {
            let global_first_sequence = self
                .producer
                .attached_global_sequence(&self.session_key, handle.stream_id, first_sequence)
                .await
                .map_err(SessionError::from)?;
            let nested_handles = self
                .producer
                .nested_handles(handle.stream_id, global_first_sequence)
                .await
                .map_err(SessionError::from)?;
            if nested_handles.len() != nested_transport_ids.len() {
                return Err(SessionError::from(
                    "nested stream mapping count does not match durable item metadata".to_string(),
                ));
            }
            for (transport_stream_id, handle) in
                nested_transport_ids.into_iter().zip(nested_handles)
            {
                let mapping = StreamSessionMappingRecord {
                    transport_stream_id,
                    handle: handle.clone(),
                    role: SessionStreamRole::Input,
                };
                let binding = self
                    .producer
                    .local_binding(transport_stream_id, &handle, SessionStreamRole::Input)
                    .await
                    .map_err(SessionError::from)?;
                self.insert_binding_mapping(binding.clone(), mapping.clone())?;
                nested_mappings.push(mapping);
                self.append_mapping_once(context, binding).await?;
            }
            let input_schema = self
                .input_schema
                .as_ref()
                .expect("nested input schema was validated before durable commit");
            input_schema
                .element_types
                .write()
                .expect("durable input schema lock poisoned")
                .extend(nested_element_types);
        }
        let resulting_offset = outcome
            .value
            .last()
            .copied()
            .ok_or_else(|| "durable input batch contains no logical items".to_string())?;
        let highest_contiguous_sequence = first_sequence
            .checked_add(logical_item_count - 1)
            .ok_or_else(|| "durable input sequence overflow".to_string())?;
        Ok(Some((
            highest_contiguous_sequence,
            logical_item_count,
            resulting_offset,
            nested_mappings,
        )))
    }

    /// Commits the input terminal for the current attachment exactly once.
    pub async fn end_input(
        &self,
        transport_stream_id: u64,
        sequence: u64,
    ) -> Result<Option<golem_common::model::durable_stream::StreamOffset>, SessionError> {
        let session = self.clone();
        self.producer
            .run_admitted(None, 0, false, move |_, admission| async move {
                let _session_guard = session.session_lock.lock().await;
                let session_for_write = session.clone();
                admission
                    .submit(move |_, context| async move {
                        session_for_write
                            .end_input_owned(&context, transport_stream_id, sequence)
                            .await
                    })
                    .await
            })
            .await
    }

    async fn end_input_owned(
        &self,
        context: &StreamWriteContext,
        transport_stream_id: u64,
        sequence: u64,
    ) -> Result<Option<golem_common::model::durable_stream::StreamOffset>, SessionError> {
        self.ensure_current_attachment().await?;
        let handle = self
            .handle(transport_stream_id)
            .ok_or_else(|| format!("unknown durable input stream {transport_stream_id}"))?;
        match self
            .producer
            .append_external_input(
                Some(context),
                &self.session_key,
                handle.stream_id,
                None,
                true,
                Some(crate::durable_host::durable_stream::ExternalProducer {
                    id: golem_common::base_model::durable_stream::ExternalProducerId::Attached,
                    epoch: 0,
                    sequence,
                }),
            )
            .await
        {
            Ok(
                crate::durable_host::durable_stream::ExternalAppendOutcome::Accepted(offset)
                | crate::durable_host::durable_stream::ExternalAppendOutcome::Duplicate {
                    offset,
                    ..
                },
            ) => Ok(Some(offset)),
            Ok(crate::durable_host::durable_stream::ExternalAppendOutcome::Closed) => Ok(None),
            Ok(outcome) => Err(SessionError::from(format!(
                "unexpected attached input end outcome: {outcome:?}"
            ))),
            Err(error) if discards_input_after_terminal(&error, &self.session_key) => {
                tracing::debug!(
                    transport_stream_id,
                    sequence,
                    error = %error,
                    "Discarding late durable input end after terminal"
                );
                Ok(None)
            }
            Err(error) => {
                tracing::warn!(
                    transport_stream_id,
                    sequence,
                    error = %error,
                    "Rejecting durable input end"
                );
                Err(SessionError::from(error))
            }
        }
    }

    /// Returns committed resume boundaries for every mapped input stream.
    pub async fn input_high_waters(
        &self,
    ) -> Result<HashMap<u64, InputStreamHighWater>, SessionError> {
        let mappings = self
            .mappings
            .read()
            .expect("durable stream mapping lock poisoned")
            .clone();
        let mut result = HashMap::new();
        for (transport_stream_id, mapping) in mappings {
            if mapping.role.direction() != SessionStreamRole::Input
                || !self.producer.owns_handle_identity(&mapping.handle)
            {
                continue;
            }
            if let Some(high_water) = self
                .producer
                .attached_input_high_water(&self.session_key, mapping.handle.stream_id)
                .await
                .map_err(SessionError::from)?
            {
                result.insert(transport_stream_id, high_water);
            }
        }
        Ok(result)
    }

    /// Replaces input stream handles with consumers backed by this session journal.
    pub async fn materialize_agent_input(
        &self,
        value: &SchemaValue,
        graph: &SchemaGraph,
        root: &SchemaType,
        component_revision: golem_common::model::component::ComponentRevision,
    ) -> Result<SessionValue, SessionError> {
        preflight_recursive_stream_value(value)?;
        let mut next_stream_index = 0u64;
        let retained_bytes =
            encode_recursive_stream_value_with_schema(value, graph, root, |_stream, _path| {
                let index = next_stream_index;
                next_stream_index = next_stream_index
                    .checked_add(1)
                    .ok_or_else(|| "durable input handle index overflow".to_string())?;
                Ok(index)
            })?
            .encoded_len();
        let session = self.clone();
        let value = value.clone();
        let graph = graph.clone();
        let root = root.clone();
        self.producer
            .run_admitted(
                None,
                retained_bytes,
                false,
                move |_, admission| async move {
                    let _guard = session.session_lock.lock().await;
                    let session_for_write = session.clone();
                    admission
                        .submit(move |_, context| async move {
                            session_for_write
                                .materialize_agent_input_owned(
                                    &context,
                                    value,
                                    graph,
                                    root,
                                    component_revision,
                                )
                                .await
                        })
                        .await
                },
            )
            .await
    }

    async fn materialize_agent_input_owned(
        &self,
        context: &StreamWriteContext,
        value: SchemaValue,
        graph: SchemaGraph,
        root: SchemaType,
        component_revision: golem_common::model::component::ComponentRevision,
    ) -> Result<SessionValue, SessionError> {
        struct PendingInput {
            path: Vec<StreamValuePathStep>,
            endpoint: Option<LiveStreamEndpoint>,
            forwarded_input: Option<ForwardedDurableInput>,
            element_type: SchemaType,
            element_schema_fingerprint: SchemaFingerprintV1,
        }

        self.recover_session_mappings().await?;
        validate_forwarded_durable_input_schemas(
            &value,
            &graph,
            &root,
            "forwarded durable input handle does not match the invocation stream schema",
        )?;
        let mut pending = Vec::new();
        let encoded =
            encode_recursive_stream_value_with_schema(&value, &graph, &root, |stream, path| {
                let element = stream_element_schema(&graph, &root, path)?;
                let element_schema_fingerprint =
                    schema_fingerprint_v1(&graph, element).map_err(|error| error.to_string())?;
                let forwarded = forwarded_durable_input_reference(stream)?;
                let (endpoint, forwarded_input) = match forwarded {
                    Some(forwarded) => (None, Some(forwarded.take(stream)?)),
                    None => (
                        Some(stream.take_host_endpoint::<LiveStreamEndpoint>()?),
                        None,
                    ),
                };
                pending.push(PendingInput {
                    path: path.to_vec(),
                    endpoint,
                    forwarded_input,
                    element_type: element.cloned().unwrap_or_else(SchemaType::u8),
                    element_schema_fingerprint,
                });
                u64::try_from(pending.len() - 1)
                    .map_err(|_| "durable input handle index overflow".to_string())
            })?;
        self.producer
            .validate_new_session_stream_count(&self.session_key, pending.len())
            .await
            .map_err(SessionError::from)?;

        let session_mapping = StreamSessionMapping {
            session_key: self.session_key.clone(),
            attachment_id: golem_common::model::durable_stream::AttachmentId::primary(
                self.session_key.callee_environment_id,
                &self.session_key.callee,
                &self.session_key.idempotency_key,
            )
            .map_err(|error| error.to_string())?,
            role: SessionStreamRole::Input,
        };
        let mut mappings = Vec::with_capacity(pending.len());
        let mut drains = Vec::with_capacity(pending.len());
        for (transport_stream_id, pending) in pending.into_iter().enumerate() {
            let transport_stream_id = u64::try_from(transport_stream_id)
                .map_err(|_| "durable input transport stream id overflow".to_string())?;
            let local = pending.forwarded_input.is_none();
            let handle = if let Some(forwarded) = pending.forwarded_input {
                let mapping = StreamSessionMappingRecord {
                    transport_stream_id,
                    handle: forwarded.handle.clone(),
                    role: SessionStreamRole::Input,
                };
                let destination = match &self.session_reference {
                    StreamRegistrationInvocation::Local(_) => {
                        StreamReaderForwardDestination::SessionBinding {
                            session_key: self.session_reference.clone(),
                            binding: StreamBindingRecord::foreign(&mapping),
                            publication: StreamReaderForwardPublication::InvocationInput,
                        }
                    }
                    StreamRegistrationInvocation::Remote(_) => {
                        StreamReaderForwardDestination::InvocationInput {
                            invocation: self.session_key.clone(),
                            mapping,
                        }
                    }
                };
                let (_, intent) = forwarded
                    .prepare_forward_owned(context, self, destination)
                    .await?;
                match intent.destination {
                    StreamReaderForwardDestination::SessionBinding { binding, .. } => {
                        self.producer.materialize_binding(&binding).await?.handle
                    }
                    StreamReaderForwardDestination::InvocationInput { mapping, .. } => {
                        mapping.handle
                    }
                }
            } else {
                let request = ProducerRegistrationRequest {
                    entity_parent_start_index: self.entity_parent_start_index,
                    coordinate: StreamRegistrationCoordinate::Root {
                        invocation_id: self.session_key.clone(),
                        root_kind: StreamRootKind::MethodInput,
                        recursive_value_path: pending.path,
                    },
                    source_invocation: self.session_reference.clone(),
                    component_revision,
                    element_schema_fingerprint: pending.element_schema_fingerprint,
                    source_kind: StreamSourceKind::AgentHostedInput,
                    session_mapping: Some(session_mapping.clone()),
                };
                self.producer
                    .register(Some(context), request)
                    .await
                    .map_err(SessionError::from)?
                    .value
            };
            let mapping = StreamSessionMappingRecord {
                transport_stream_id,
                handle: handle.clone(),
                role: SessionStreamRole::Input,
            };
            let binding = if local {
                self.producer
                    .local_binding(transport_stream_id, &handle, SessionStreamRole::Input)
                    .await
                    .map_err(SessionError::from)?
            } else {
                StreamBindingRecord::foreign(&mapping)
            };
            self.append_mapping_once(context, binding.clone()).await?;
            self.insert_binding_mapping(binding, mapping.clone())?;
            mappings.push(mapping);
            if let Some(endpoint) = pending.endpoint {
                drains.push(PendingOwnedStreamDrain {
                    activity: self.drain_activity(&endpoint),
                    handle,
                    endpoint,
                    element_type: pending.element_type,
                    role: SessionStreamRole::Input,
                });
            }
        }
        if !mappings.is_empty() {
            self.commit_consumer_journal().await?;
        }

        if !drains.is_empty() {
            let streams = self.clone();
            let graph = Arc::new(graph);
            self.producer.tasks().spawn(async move {
                let (nested_tx, mut nested_rx) = mpsc::unbounded_channel();
                let mut tasks = streams.producer.tasks().children();
                for drain in drains {
                    let streams = streams.clone();
                    let graph = graph.clone();
                    let nested_tx = nested_tx.clone();
                    tasks.spawn(async move {
                        if streams.require_root_attachment_before_production {
                            streams.wait_for_active_attachment(&drain.handle).await?;
                        }
                        streams.drain_output(drain, graph, nested_tx).await
                    });
                }
                loop {
                    // A completed parent may have queued children before its final join.
                    while let Ok(drain) = nested_rx.try_recv() {
                        let streams = streams.clone();
                        let graph = graph.clone();
                        let nested_tx = nested_tx.clone();
                        tasks.spawn(
                            async move { streams.drain_output(drain, graph, nested_tx).await },
                        );
                    }
                    if tasks.is_empty() {
                        break;
                    }
                    tokio::select! {
                        Some(drain) = nested_rx.recv() => {
                            let streams = streams.clone();
                            let graph = graph.clone();
                            let nested_tx = nested_tx.clone();
                            tasks.spawn(async move {
                                streams.drain_output(drain, graph, nested_tx).await
                            });
                        }
                        result = tasks.join_next() => {
                            match result {
                                Some(Ok(Err(error))) => {
                                    tracing::warn!(%error, "durable caller input drain failed");
                                }
                                Some(Err(error)) => {
                                    tracing::warn!(%error, "durable caller input drain task failed");
                                }
                                Some(Ok(Ok(()))) | None => {}
                            }
                        }
                    }
                }
            });
        }
        Ok(SessionValue {
            value: encoded,
            mappings,
        })
    }

    #[cfg(test)]
    pub(super) async fn endpoint(
        &self,
        handle: DurableStreamHandle,
        consumer_read_ordinal: u64,
        role: SessionStreamRole,
    ) -> Result<DurableInputEndpoint, SessionError> {
        let mapping = self
            .mapping_for_handle(&handle, role)
            .ok_or_else(|| "durable input endpoint has no session mapping".to_string())?;
        self.endpoint_for_mapping(mapping, consumer_read_ordinal)
            .await
    }

    pub(super) async fn endpoint_for_mapping(
        &self,
        mapping: StreamSessionMappingRecord,
        consumer_read_ordinal: u64,
    ) -> Result<DurableInputEndpoint, SessionError> {
        let metadata = self.current_control_metadata().await?;
        let binding = self
            .binding(mapping.transport_stream_id)
            .unwrap_or_else(|| StreamBindingRecord::foreign(&mapping));
        let reader_id = metadata.reader_id(&binding)?;
        drop(metadata);
        let history = self.consumer_history(reader_id).await?;
        Ok(DurableInputEndpoint {
            reader: None,
            journal: history.events,
            nested_mappings: history.nested_mappings,
            observations: history.observations,
            source_after: history.after,
            source_terminal: history.terminal,
            streams: self.clone(),
            transport_stream_id: mapping.transport_stream_id,
            handle: mapping.handle,
            consumer_read_ordinal,
            role: mapping.role,
            reader_id: futures::future::ready(Ok(reader_id)).boxed().shared(),
        })
    }

    pub(super) async fn stream_reader(
        &self,
        mapping: StreamSessionMappingRecord,
        after: Option<golem_common::model::durable_stream::StreamOffset>,
    ) -> Result<DurableStreamReader, SessionError> {
        let handle = mapping.handle.clone();
        let foreign_binding = self
            .binding(mapping.transport_stream_id)
            .is_none_or(|binding| matches!(binding.source, StreamRecordReference::Foreign(_)));
        let reader = if self.producer.owns_handle_identity(&handle) {
            DurableStreamReader::Owned {
                source_wait_can_suspend: self.producer.source_wait_can_suspend(&handle).await?,
                reader: Box::new(
                    self.producer
                        .catch_up(handle.clone(), after)
                        .await
                        .map_err(SessionError::from)?,
                ),
                source: self.producer.clone(),
                handle: Box::new(handle),
                foreign_binding,
                next_journal_lag_sample: Instant::now(),
            }
        } else {
            let attachment = self.attachment_key(&handle, self.reader_epoch().await?)?;
            if self.topology_state(&attachment, Some(&mapping)).await?
                != ConsumerAttachmentStatus::Active
            {
                self.activate_foreign_mapping(mapping.clone(), attachment.epoch)
                    .await?;
            }
            let rpc = self.rpc.clone().ok_or_else(|| {
                "foreign durable stream source routing is unavailable".to_string()
            })?;
            let auth_ctx = self.auth_ctx.clone().ok_or_else(|| {
                "foreign durable stream consumer authorization is unavailable".to_string()
            })?;
            let consumer_producer = if self.consumer_journal.is_some() {
                let binding = self
                    .binding(mapping.transport_stream_id)
                    .unwrap_or_else(|| StreamBindingRecord::foreign(&mapping));
                let reader_id = self.current_control_metadata().await?.reader_id(&binding)?;
                Some((self.producer.clone(), reader_id))
            } else {
                None
            };
            DurableStreamReader::Attached(Box::new(AttachedDurableCatchUpReader {
                source: Arc::new(RoutedAttachedStreamSegmentSource::new(
                    rpc,
                    mapping,
                    auth_ctx,
                    self.producer.clone(),
                )),
                attachment,
                handle: handle.clone(),
                consumer_producer,
                after,
                buffered: VecDeque::new(),
                terminal: false,
                next_journal_lag_sample: Instant::now(),
            }))
        };
        Ok(reader)
    }

    pub(super) async fn consumer_history_positions(
        &self,
        reader_id: LocalStreamReaderId,
    ) -> Result<Vec<OplogIndex>, SessionError> {
        let (mut covered, mut positions) = self
            .producer
            .persisted_consumer_positions(&self.session_key, reader_id)
            .await?
            .unwrap_or((OplogIndex::NONE, Vec::new()));
        let horizon = self.oplog.current_oplog_index().await;
        while covered < horizon {
            let count = (horizon.as_u64() - covered.as_u64()).min(1024);
            for (index, entry) in self.oplog.read_exact(covered.next(), count).await? {
                if self
                    .producer
                    .fork_lineage
                    .deleted_regions()
                    .is_in_deleted_region(index)
                {
                    covered = index;
                    continue;
                }
                if let OplogEntry::StreamSession { record, .. } = entry {
                    let record = self.download_record(record).await?;
                    let matches = match record {
                        StreamSessionRecord::ConsumerItemValue(record) => {
                            record.session_key == self.session_reference
                                && record.reader_id == reader_id
                        }
                        StreamSessionRecord::ConsumerTerminal(record) => {
                            record.session_key == self.session_reference
                                && record.reader_id == reader_id
                        }
                        StreamSessionRecord::SourceUnavailable(record) => {
                            record.session_key == self.session_reference
                                && record.reader_id == reader_id
                        }
                        _ => false,
                    };
                    if matches {
                        positions.push(index);
                    }
                }
                covered = index;
            }
        }
        Ok(positions)
    }

    pub(super) async fn consumer_history(
        &self,
        reader_id: LocalStreamReaderId,
    ) -> Result<ConsumerHistory, SessionError> {
        let root_binding = self
            .current_control_metadata()
            .await?
            .reader_binding(reader_id)?
            .clone();
        let stream_id = self
            .producer
            .materialize_binding(&root_binding)
            .await
            .map_err(SessionError::from)?
            .handle
            .stream_id;
        let mut events = Vec::new();
        let mut nested_mappings = HashMap::new();
        let mut observations = HashMap::new();
        for index in self.consumer_history_positions(reader_id).await? {
            let record = self.session_record_at(index).await?;
            if let Some(original_index) = record.reobserves() {
                let original = self.session_record_at(original_index).await?;
                if original_index >= index || !record.matches_reobserved(&original) {
                    return Err(SessionError::from(
                        WorkerExecutorError::unexpected_oplog_entry(
                            "unchanged logical consumer input",
                            format!("invalid re-observation at {index} of {original_index}"),
                        ),
                    ));
                }
            }
            match &record {
                StreamSessionRecord::ConsumerItemValue(record) => {
                    for offset in 0..record.logical_item_count() {
                        let ordinal = record.consumer_read_ordinal + offset as u64;
                        let observed =
                            observations
                                .entry(ordinal)
                                .or_insert_with(|| ConsumerObservations {
                                    original_index: record.reobserves.unwrap_or(index),
                                    boundaries: Vec::new(),
                                });
                        observed
                            .boundaries
                            .push((index, record.observation.clone()));
                    }
                }
                StreamSessionRecord::ConsumerTerminal(record) => {
                    let observed = observations
                        .entry(record.consumer_read_ordinal)
                        .or_insert_with(|| ConsumerObservations {
                            original_index: record.reobserves.unwrap_or(index),
                            boundaries: Vec::new(),
                        });
                    observed
                        .boundaries
                        .push((index, record.observation.clone()));
                }
                _ => {}
            }
            if record.reobserves().is_some() {
                continue;
            }
            match record {
                StreamSessionRecord::ConsumerItemValue(record)
                    if record.session_key == self.session_reference
                        && record.reader_id == reader_id =>
                {
                    let mappings = self
                        .producer
                        .materialize_bindings(&record.recursive_mappings)
                        .await
                        .map_err(SessionError::from)?;
                    for (binding, mapping) in record
                        .recursive_mappings
                        .iter()
                        .cloned()
                        .zip(mappings.iter().cloned())
                    {
                        self.insert_binding_mapping(binding, mapping)?;
                    }
                    if !record.recursive_mappings.is_empty() {
                        nested_mappings.insert(record.source_offset, mappings.clone());
                    }
                    if record.packed_u8 {
                        if !record.recursive_mappings.is_empty() {
                            return Err(SessionError::from(
                                "packed-u8 consumer journal contains nested stream handles"
                                    .to_string(),
                            ));
                        }
                        let batch_end = record
                            .source_offset_at(record.logical_item_count().saturating_sub(1))
                            .ok_or_else(|| {
                                "packed-u8 consumer journal offset range is invalid".to_string()
                            })?;
                        for (index, byte) in record.value.iter().copied().enumerate() {
                            let ordinal = record
                                .consumer_read_ordinal
                                .checked_add(index as u64)
                                .ok_or_else(|| {
                                    "packed-u8 consumer journal ordinal overflow".to_string()
                                })?;
                            events.push((
                                ordinal,
                                CommittedProducerStreamEvent {
                                    stream_id,
                                    producer_sequence: ordinal,
                                    offset: record.source_offset_at(index).ok_or_else(|| {
                                        "packed-u8 consumer journal offset range is invalid"
                                            .to_string()
                                    })?,
                                    packed_u8_batch_end: Some(batch_end),
                                    terminal_author: None,
                                    nested_handles: Vec::new(),
                                    nested_references: Vec::new(),
                                    payload: CommittedProducerStreamEventPayload::PackedU8(byte),
                                },
                            ));
                        }
                    } else {
                        events.push((
                            record.consumer_read_ordinal,
                            CommittedProducerStreamEvent {
                                stream_id,
                                producer_sequence: record.consumer_read_ordinal,
                                offset: record.source_offset,
                                packed_u8_batch_end: None,
                                terminal_author: None,
                                nested_references: mappings
                                    .iter()
                                    .map(|mapping| {
                                        StreamRecordReference::Foreign(mapping.handle.clone())
                                    })
                                    .collect(),
                                nested_handles: mappings
                                    .into_iter()
                                    .map(|mapping| mapping.handle)
                                    .collect(),
                                payload: CommittedProducerStreamEventPayload::Value(record.value),
                            },
                        ));
                    }
                }
                StreamSessionRecord::ConsumerTerminal(record)
                    if record.session_key == self.session_reference
                        && record.reader_id == reader_id =>
                {
                    events.push((
                        record.consumer_read_ordinal,
                        CommittedProducerStreamEvent {
                            stream_id,
                            producer_sequence: record.consumer_read_ordinal,
                            offset: record.source_offset,
                            packed_u8_batch_end: None,
                            terminal_author: None,
                            nested_handles: Vec::new(),
                            nested_references: Vec::new(),
                            payload: match record.terminal {
                                StreamConsumerTerminal::End(result) => {
                                    CommittedProducerStreamEventPayload::End(result)
                                }
                                StreamConsumerTerminal::Cancel {
                                    role,
                                    reason,
                                    details,
                                } => CommittedProducerStreamEventPayload::Cancel {
                                    role,
                                    reason,
                                    details,
                                },
                            },
                        },
                    ));
                }
                StreamSessionRecord::SourceUnavailable(record)
                    if record.session_key == self.session_reference
                        && record.reader_id == reader_id =>
                {
                    events.push((
                        record.consumer_read_ordinal,
                        CommittedProducerStreamEvent {
                            stream_id,
                            producer_sequence: record.consumer_read_ordinal,
                            offset: record.source_offset,
                            packed_u8_batch_end: None,
                            terminal_author: None,
                            nested_handles: Vec::new(),
                            nested_references: Vec::new(),
                            payload: CommittedProducerStreamEventPayload::Cancel {
                                role: StreamCancelRole::System,
                                reason: StreamCancelReason::SourceUnavailable,
                                details: None,
                            },
                        },
                    ));
                }
                _ => {}
            }
        }
        events.sort_by_key(|(ordinal, _)| *ordinal);
        for (expected, (ordinal, _)) in events.iter().enumerate() {
            if *ordinal != expected as u64 {
                return Err(SessionError::from(
                    "consumer value journal contains a read-ordinal gap".to_string(),
                ));
            }
        }
        let after = events.last().map(|(_, event)| event.offset);
        let terminal = events.last().is_some_and(|(_, event)| event.is_terminal());
        Ok(ConsumerHistory {
            events: events.into_iter().map(|(_, event)| event).collect(),
            nested_mappings,
            observations,
            after,
            terminal,
        })
    }
}

pub(super) fn discards_input_after_terminal(
    error: &StreamStoreError,
    session_key: &StreamSessionKey,
) -> bool {
    matches!(
        error,
        StreamStoreError::SessionFinished(finished) if finished == session_key
    ) || matches!(
        error,
        StreamStoreError::ClosedByOtherProducer
            | StreamStoreError::FencedByTerminal(CommittedProducerStreamEventPayload::Cancel {
                role: StreamCancelRole::InputConsumer,
                ..
            })
    )
}
