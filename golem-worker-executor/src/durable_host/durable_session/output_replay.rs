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
    /// Drives output streams from committed producer history into transport callbacks.
    pub async fn pump_output_streams(
        &self,
        responses: &mpsc::Sender<InvocationResponse>,
    ) -> Result<(), SessionError> {
        self.recover_session_mappings().await?;
        let root_output_mapping_ids = self.session_root_output_mapping_ids().await?;
        self.pump_output_streams_once(
            &HashMap::new(),
            &root_output_mapping_ids,
            &[],
            &Default::default(),
            responses,
        )
        .await
    }

    /// Replays output streams for a resumed attachment.
    ///
    /// The root output streams are pumped first, and nested streams are pumped only after the
    /// enclosing item that announces their transport mapping has been emitted, so the client
    /// observes the same tree order as during the original invocation. Output streams that were
    /// already mapped but are not reached by that traversal, because a cursor skipped the parent
    /// item that introduced them, are pumped afterwards.
    pub async fn pump_output_streams_from(
        &self,
        cursors: &HashMap<
            golem_common::model::durable_stream::StreamId,
            Option<golem_common::model::durable_stream::StreamOffset>,
        >,
        root_output_mapping_ids: &[u64],
        known_output_mapping_ids: &[u64],
        responses: &mpsc::Sender<InvocationResponse>,
    ) -> Result<(), SessionError> {
        self.pump_output_streams_once(
            cursors,
            root_output_mapping_ids,
            known_output_mapping_ids,
            &Default::default(),
            responses,
        )
        .await
    }

    /// Early and result-time pumps for one attachment share `seen`, including nested mappings.
    pub(crate) async fn pump_output_streams_once(
        &self,
        cursors: &HashMap<
            golem_common::model::durable_stream::StreamId,
            Option<golem_common::model::durable_stream::StreamOffset>,
        >,
        root_output_mapping_ids: &[u64],
        known_output_mapping_ids: &[u64],
        seen: &std::sync::Mutex<HashSet<u64>>,
        responses: &mpsc::Sender<InvocationResponse>,
    ) -> Result<(), SessionError> {
        self.recover_session_mappings().await?;
        self.pump_output_stream_trees(cursors, root_output_mapping_ids, seen, responses)
            .await?;
        self.pump_output_stream_trees(cursors, known_output_mapping_ids, seen, responses)
            .await
    }

    async fn pump_output_stream_trees(
        &self,
        cursors: &HashMap<
            golem_common::model::durable_stream::StreamId,
            Option<golem_common::model::durable_stream::StreamOffset>,
        >,
        output_mapping_ids: &[u64],
        seen: &std::sync::Mutex<HashSet<u64>>,
        responses: &mpsc::Sender<InvocationResponse>,
    ) -> Result<(), SessionError> {
        let mut pending = output_mapping_ids
            .iter()
            .copied()
            .filter(|transport_stream_id| seen.lock().unwrap().insert(*transport_stream_id))
            .map(|transport_stream_id| {
                let mapping = self.mapping(transport_stream_id).ok_or_else(|| {
                    format!("unknown durable output stream {transport_stream_id}")
                })?;
                Ok(mapping)
            })
            .collect::<Result<Vec<_>, SessionError>>()?;
        while !pending.is_empty() {
            let nested = try_join_all(pending.into_iter().map(|mapping| {
                let after = cursors.get(&mapping.handle.stream_id).copied().flatten();
                self.pump_output_stream_from(
                    mapping.transport_stream_id,
                    mapping.handle,
                    after,
                    responses,
                )
            }))
            .await?;
            pending = nested
                .into_iter()
                .flatten()
                .filter(|mapping| seen.lock().unwrap().insert(mapping.transport_stream_id))
                .collect();
        }
        Ok(())
    }

    /// Sends the guest-authored cancellation of every callee-owned input stream to the attached
    /// client. Inputs whose acceptance already announced a terminal high water are skipped: the
    /// client learned about that cancellation from the acceptance, and re-sending the same
    /// durable offset would violate the strictly increasing offset rule of the session protocol.
    pub async fn pump_input_cancellations(
        &self,
        responses: &mpsc::Sender<InvocationResponse>,
        announced_high_waters: &HashMap<u64, InputStreamHighWater>,
    ) -> Result<(), SessionError> {
        self.recover_session_mappings().await?;
        let inputs = self
            .mappings
            .read()
            .expect("durable stream mapping lock poisoned")
            .iter()
            .filter_map(|(transport_stream_id, mapping)| {
                (mapping.role.direction() == SessionStreamRole::Input
                    && mapping.handle.producer_environment_id
                        == self.session_key.callee_environment_id
                    && mapping.handle.producer == self.session_key.callee
                    && mapping.handle.expected_producer_fingerprint
                        == self.session_key.callee_fingerprint
                    && !announced_high_waters
                        .get(transport_stream_id)
                        .is_some_and(|high_water| high_water.terminal))
                .then_some((*transport_stream_id, mapping.handle.clone()))
            })
            .collect::<Vec<_>>();
        for (transport_stream_id, handle) in inputs {
            let mut reader = self
                .producer
                .catch_up(handle.clone(), None)
                .await
                .map_err(SessionError::from)?;
            while let Some(event) = reader.next().await.map_err(SessionError::from)? {
                match event.payload {
                    CommittedProducerStreamEventPayload::Cancel {
                        role: StreamCancelRole::InputConsumer,
                        reason,
                        details,
                    } => {
                        responses
                            .send(InvocationResponse {
                                response: Some(invocation_response::Response::StreamCancel(
                                    StreamCancel {
                                        transport_stream_id,
                                        producer_sequence: event.producer_sequence,
                                        role: golem_api_grpc::proto::golem::worker::StreamCancelRole::InputConsumer as i32,
                                        reason: stream_cancel_reason_to_proto(reason) as i32,
                                        details,
                                        durable_stream_id: Some(handle.stream_id.0.into()),
                                        epoch: self.attachment_epoch,
                                        durable_offset: event.offset.0.to_vec(),
                                    },
                                )),
                            })
                            .await
                            .map_err(|_| "invocation response stream closed".to_string())?;
                        break;
                    }
                    CommittedProducerStreamEventPayload::End(_)
                    | CommittedProducerStreamEventPayload::Cancel { .. } => break,
                    CommittedProducerStreamEventPayload::Value(_)
                    | CommittedProducerStreamEventPayload::PackedU8(_) => {}
                }
            }
        }
        Ok(())
    }

    pub(super) async fn pump_output_stream_from(
        &self,
        transport_stream_id: u64,
        handle: DurableStreamHandle,
        after: Option<golem_common::model::durable_stream::StreamOffset>,
        responses: &mpsc::Sender<InvocationResponse>,
    ) -> Result<Vec<StreamSessionMappingRecord>, SessionError> {
        let durable_stream_id = handle.stream_id;
        let role = self
            .mapping(transport_stream_id)
            .ok_or_else(|| format!("unknown durable output stream {transport_stream_id}"))?
            .role;
        if role.direction() != SessionStreamRole::Output {
            return Err(SessionError::from(format!(
                "durable stream {transport_stream_id} is not an output"
            )));
        }
        let OutputReplay {
            mappings: mut nested_streams,
            terminal_cursor,
        } = if let Some(through) = after {
            self.recover_session_mappings().await?;
            self.output_mappings_introduced_through(transport_stream_id, &handle, through)
                .await?
        } else {
            OutputReplay {
                mappings: Vec::new(),
                terminal_cursor: false,
            }
        };
        if terminal_cursor {
            return Ok(nested_streams);
        }
        let mut reader = self
            .stream_reader(
                StreamSessionMappingRecord {
                    transport_stream_id,
                    handle,
                    role,
                },
                after,
            )
            .await?;
        let mut pending_event = None;
        loop {
            let event = match pending_event.take() {
                Some(event) => event,
                None => match reader.next().await.map_err(SessionError::from)? {
                    Some(event) => event,
                    None => break,
                },
            };
            self.ensure_current_attachment().await?;
            let response = match event.payload {
                CommittedProducerStreamEventPayload::Value(bytes) => {
                    let memory = bytes.len()
                        + golem_common::serialization::serialize(&event.nested_handles)?.len();
                    let session = self.clone();
                    let (response, introduced) = self
                        .producer
                        .run_admitted(None, memory, false, move |_, admission| async move {
                            let _session_guard = session.session_lock.lock().await;
                            let session_for_write = session.clone();
                            admission.submit(move |_: Arc<DurableStreamStore>, context: StreamWriteContext| async move {
                            let session = session_for_write;
                            session.ensure_current_attachment().await?;
                            session.recover_session_mappings().await?;
                            let mut nested_streams = Vec::new();
                            let value =
                                ProtoSchemaValue::decode(bytes.as_slice()).map_err(|error| {
                                    format!("invalid durable output value: {error}")
                                })?;
                            let handle_indices = preflight_proto_recursive_stream_value(&value)?;
                            let nested_handles = event.nested_handles.clone();
                            let nested_references = event.nested_references.clone();
                            if nested_handles.len() != handle_indices.len()
                                || nested_references.len() != nested_handles.len()
                            {
                                return Err(
                                    "nested output reference count does not match the canonical value"
                                        .to_string().into(),
                                );
                            }
                            let mut mappings = Vec::with_capacity(nested_handles.len());
                            for (position, ((handle_index, handle), reference)) in handle_indices
                                .into_iter()
                                .zip(nested_handles)
                                .zip(nested_references)
                                .enumerate()
                            {
                                if handle_index != position as u64 {
                                    return Err(format!(
                                        "invalid canonical nested output handle index {handle_index}"
                                    ).into());
                                }
                                let mapping = match session
                                    .mapping_for_reference(&reference, SessionStreamRole::Output)
                                {
                                    Some(mapping) => mapping,
                                    None => {
                                        let transport_stream_id =
                                            session.allocate_transport_stream_id()?;
                                        let mapping = StreamSessionMappingRecord {
                                            transport_stream_id,
                                            handle: handle.clone(),
                                            role: SessionStreamRole::Output,
                                        };
                                        let binding = StreamBindingRecord {
                                            transport_stream_id,
                                            source: reference,
                                            role: SessionStreamRole::Output,
                                        };
                                        session.insert_binding_mapping(
                                            binding.clone(),
                                            mapping.clone(),
                                        )?;
                                        session
                                            .append_mapping_once(&context, binding)
                                            .await?;
                                        mapping
                                    }
                                };
                                nested_streams.push(mapping.clone());
                                mappings.push(mapping);
                            }
                            if !mappings.is_empty() {
                                session.commit_consumer_journal().await?;
                            }
                            let value =
                                remap_recursive_stream_references(value, |handle_index, _| {
                                    let index = usize::try_from(handle_index).map_err(|_| {
                            format!(
                                "durable nested output handle index {handle_index} is too large"
                            )
                        })?;
                                    mappings
                            .get(index)
                            .map(|mapping| mapping.transport_stream_id)
                            .ok_or_else(|| {
                                format!("unknown durable nested output handle index {handle_index}")
                            })
                                })?;
                            let new_stream_mappings = mappings
                                .iter()
                                .map(|mapping| durable_stream_mapping_to_proto(mapping, None))
                                .collect();
                            let response =
                                invocation_response::Response::OutputItem(OutputStreamItem {
                                    transport_stream_id,
                                    producer_sequence: event.producer_sequence,
                                    value: Some(value),
                                    durable_stream_id: Some(durable_stream_id.0.into()),
                                    durable_offset: event.offset.0.to_vec(),
                                    epoch: session.attachment_epoch,
                                    new_stream_mappings,
                                    packed_u8: Vec::new(),
                                    logical_item_count: 1,
                                });
                            Ok::<_, SessionError>((response, nested_streams))
                            }).await
                        })
                        .await?;
                    nested_streams.extend(introduced);
                    response
                }
                CommittedProducerStreamEventPayload::PackedU8(value) => {
                    if !event.nested_handles.is_empty() {
                        return Err(SessionError::from(
                            "packed-u8 durable output item contains nested stream handles"
                                .to_string(),
                        ));
                    }
                    let first_sequence = event.producer_sequence;
                    let mut final_offset = event.offset;
                    let mut bytes = Vec::with_capacity(1024);
                    bytes.push(value);
                    let flush_deadline = tokio::time::Instant::now() + PACKED_U8_OUTPUT_FLUSH_DELAY;
                    while bytes.len() < MAX_PACKED_U8_STREAM_ITEM_SIZE {
                        let next =
                            match tokio::time::timeout_at(flush_deadline, reader.next()).await {
                                Ok(Ok(Some(next))) => next,
                                Ok(Ok(None)) | Err(_) => break,
                                Ok(Err(error)) => return Err(SessionError::from(error)),
                            };
                        let expected_sequence = first_sequence
                            .checked_add(bytes.len() as u64)
                            .ok_or_else(|| "packed-u8 output sequence overflow".to_string())?;
                        match &next.payload {
                            CommittedProducerStreamEventPayload::PackedU8(byte)
                                if next.producer_sequence == expected_sequence
                                    && next.nested_handles.is_empty() =>
                            {
                                bytes.push(*byte);
                                final_offset = next.offset;
                            }
                            _ => {
                                pending_event = Some(next);
                                break;
                            }
                        }
                    }
                    self.ensure_current_attachment().await?;
                    invocation_response::Response::OutputItem(OutputStreamItem {
                        transport_stream_id,
                        producer_sequence: first_sequence,
                        value: None,
                        durable_stream_id: Some(durable_stream_id.0.into()),
                        durable_offset: final_offset.0.to_vec(),
                        epoch: self.attachment_epoch,
                        new_stream_mappings: Vec::new(),
                        logical_item_count: bytes.len() as u64,
                        packed_u8: bytes,
                    })
                }
                CommittedProducerStreamEventPayload::End(StreamEndResult::Ok) => {
                    invocation_response::Response::OutputEnd(OutputStreamEnd {
                        transport_stream_id,
                        producer_sequence: event.producer_sequence,
                        durable_stream_id: Some(durable_stream_id.0.into()),
                        durable_offset: event.offset.0.to_vec(),
                        epoch: self.attachment_epoch,
                    })
                }
                CommittedProducerStreamEventPayload::End(StreamEndResult::ErrorContext(
                    details,
                )) => invocation_response::Response::OutputError(OutputStreamError {
                    transport_stream_id,
                    producer_sequence: event.producer_sequence,
                    details: String::from_utf8_lossy(&details).into_owned(),
                    durable_stream_id: Some(durable_stream_id.0.into()),
                    durable_offset: event.offset.0.to_vec(),
                    epoch: self.attachment_epoch,
                }),
                CommittedProducerStreamEventPayload::Cancel {
                    role: _,
                    reason,
                    details,
                } => invocation_response::Response::StreamCancel(StreamCancel {
                    transport_stream_id,
                    producer_sequence: event.producer_sequence,
                    role: golem_api_grpc::proto::golem::worker::StreamCancelRole::OutputProducer
                        as i32,
                    reason: stream_cancel_reason_to_proto(reason) as i32,
                    details,
                    durable_stream_id: Some(durable_stream_id.0.into()),
                    epoch: self.attachment_epoch,
                    durable_offset: event.offset.0.to_vec(),
                }),
            };
            let terminal = matches!(
                response,
                invocation_response::Response::OutputEnd(_)
                    | invocation_response::Response::OutputError(_)
                    | invocation_response::Response::StreamCancel(_)
            );
            responses
                .send(InvocationResponse {
                    response: Some(response),
                })
                .await
                .map_err(|_| "invocation response stream closed".to_string())?;
            if terminal {
                break;
            }
        }
        Ok(nested_streams)
    }

    pub(super) async fn output_mappings_introduced_through(
        &self,
        transport_stream_id: u64,
        handle: &DurableStreamHandle,
        through: golem_common::model::durable_stream::StreamOffset,
    ) -> Result<OutputReplay, SessionError> {
        let local_binding = self
            .binding(transport_stream_id)
            .is_some_and(|binding| matches!(binding.source, StreamRecordReference::Local(_)));
        let mut after = None;
        let mut nested_streams = Vec::new();
        let mut seen = HashSet::new();
        loop {
            let events = if self.producer.owns_handle_identity(handle) {
                self.producer
                    .read_segment(handle, after, Some(through))
                    .await
                    .map_err(SessionError::from)?
            } else {
                let mapping = self
                    .mapping(transport_stream_id)
                    .ok_or_else(|| "foreign durable stream has no session mapping".to_string())?;
                let attachment = self.attachment_key(handle, self.reader_epoch().await?)?;
                if self.topology_state(&attachment, Some(&mapping)).await?
                    != ConsumerAttachmentStatus::Active
                {
                    return Err(SessionError::from(
                        "foreign durable stream mapping is not topology-activated".to_string(),
                    ));
                }
                if !self.producer.fork_lineage.cuts().is_empty() {
                    self.activate_foreign_mapping(mapping.clone(), attachment.epoch)
                        .await?;
                }
                let rpc = self.rpc.clone().ok_or_else(|| {
                    "foreign durable stream source routing is unavailable".to_string()
                })?;
                let auth_ctx = self.auth_ctx.clone().ok_or_else(|| {
                    "foreign durable stream consumer authorization is unavailable".to_string()
                })?;
                RoutedAttachedStreamSegmentSource::new(
                    rpc,
                    mapping,
                    auth_ctx,
                    self.producer.clone(),
                )
                .read_attached_segment(
                    &attachment,
                    handle,
                    Timestamp::now_utc().to_millis(),
                    after,
                    Some(through),
                )
                .await
                .map_err(SessionError::from)?
            };
            let last = events
                .last()
                .ok_or_else(|| "stream replay ended before its requested cursor".to_string())?;
            if after.is_some_and(|after| last.offset <= after) {
                return Err("stream replay page did not advance its cursor".into());
            }
            after = Some(last.offset);
            let terminal_cursor = last.is_terminal();
            let page = events
                .into_iter()
                .flat_map(|event| {
                    event
                        .nested_handles
                        .into_iter()
                        .zip(event.nested_references)
                })
                .map(|(nested_handle, reference)| {
                    let reference = if local_binding {
                        reference
                    } else {
                        StreamRecordReference::Foreign(nested_handle.clone())
                    };
                    (nested_handle, reference)
                })
                .filter(|(_, reference)| seen.insert(reference.clone()))
                .map(|(nested_handle, reference)| {
                    let mapping = self
                        .mapping_for_reference(&reference, SessionStreamRole::Output)
                        .ok_or_else(|| {
                            format!(
                                "durable nested output stream {} has no session mapping",
                                nested_handle.stream_id
                            )
                        })?;
                    Ok(mapping)
                })
                .collect::<Result<Vec<_>, SessionError>>()?;
            nested_streams.extend(page);
            if after == Some(through) {
                return Ok(OutputReplay {
                    mappings: nested_streams,
                    terminal_cursor,
                });
            }
        }
    }

    /// Returns output IDs whose terminal cursors can be reconstructed from committed records.
    pub async fn terminal_output_cursor_stream_ids(
        &self,
        cursors: &HashMap<
            golem_common::model::durable_stream::StreamId,
            Option<golem_common::model::durable_stream::StreamOffset>,
        >,
    ) -> Result<HashSet<golem_common::model::durable_stream::StreamId>, SessionError> {
        self.recover_session_mappings().await?;
        let candidates = self
            .mappings
            .read()
            .expect("durable stream mapping lock poisoned")
            .values()
            .filter_map(|mapping| {
                if mapping.role.direction() != SessionStreamRole::Output {
                    return None;
                }
                cursors
                    .get(&mapping.handle.stream_id)
                    .copied()
                    .flatten()
                    .map(|cursor| (mapping.transport_stream_id, mapping.handle.clone(), cursor))
            })
            .collect::<Vec<_>>();
        let mut terminal = HashSet::new();
        for (transport_stream_id, handle, cursor) in candidates {
            let OutputReplay {
                terminal_cursor, ..
            } = self
                .output_mappings_introduced_through(transport_stream_id, &handle, cursor)
                .await?;
            if terminal_cursor {
                terminal.insert(handle.stream_id);
            }
        }
        Ok(terminal)
    }

    /// Returns transport IDs durably classified as root outputs.
    pub async fn session_root_output_mapping_ids(&self) -> Result<Vec<u64>, SessionError> {
        Ok(self
            .current_control_metadata()
            .await?
            .root_outputs()
            .to_vec())
    }

    /// Decodes initial transport input while preserving durable stream placeholders.
    pub async fn decode_initial(
        &self,
        value: ProtoSchemaValue,
        mappings: &[StreamSessionMappingRecord],
        role: SessionStreamRole,
    ) -> Result<SchemaValue, SessionError> {
        let ids = preflight_proto_recursive_stream_value(&value)?;
        let mut endpoints = HashMap::with_capacity(ids.len());
        for handle_index in ids {
            let index = usize::try_from(handle_index)
                .map_err(|_| format!("durable input handle index {handle_index} is too large"))?;
            let mapping = mappings
                .get(index)
                .cloned()
                .ok_or_else(|| format!("unknown durable input handle index {handle_index}"))?;
            if self.mapping(mapping.transport_stream_id).as_ref() != Some(&mapping)
                || mapping.role.direction() != role.direction()
            {
                return Err(SessionError::from(format!(
                    "durable input mapping {handle_index} does not match its handle"
                )));
            }
            endpoints.insert(handle_index, self.endpoint_for_mapping(mapping, 0).await?);
        }
        decode_recursive_stream_value(value, |handle_index, _| {
            endpoints
                .remove(&handle_index)
                .map(SchemaValueStream::from_host_endpoint)
                .ok_or_else(|| {
                    format!("duplicate or unknown durable input handle index {handle_index}")
                })
        })
        .map_err(SessionError::from)
    }
}
